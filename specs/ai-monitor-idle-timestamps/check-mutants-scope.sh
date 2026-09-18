#!/bin/sh
# Lesson L5 — every function this feature CHANGED must be inside the mutation-testing scope.
#
# The mistake this exists to stop: the mutation run was scoped with `-F` to the functions the
# implementer WROTE, which silently excluded `tabmeta::worker::run` — a function the feature
# changed (it gained the deadline local, the `Cmd::SetPanelDeadline` arm and the wake block).
# `replace run with ()` survived, meaning the two lines that turn an armed deadline into a wake
# could be deleted with the whole suite green and the feature wholly inert. The run then claimed
# "0 survivors on every changed function", which was false.
#
# This does NOT run cargo mutants (minutes). It compares the functions the diff touches against
# a scope the run must record, so a changed-but-unscoped function fails fast and loudly.
#
# HOW A FUNCTION IS FOUND, and why this is the second version of that answer.
#
# The first version read the `fn` name out of each diff HUNK HEADER. That was wrong in a way
# that defeated the whole purpose of the script, and it was found the same way the original
# miss was — by running `cargo mutants` on something the scope file did not list:
#
#     @@ -246,0 +258,17 @@ impl TabMetaHandle {
#
# A hunk header names the nearest preceding top-level item, which for a method is the `impl`
# block, NOT the method. The header carries no `fn` at all, so the census matched nothing and
# `TabMetaHandle::set_panel_deadline` — the panel's entire route to the worker — was never in
# scope. `replace set_panel_deadline with ()` survived: no command is ever sent, nothing is
# ever armed, and the pill never leaves CONNECTED, with every test green. That is the same
# failure as the `run` survivor, reached through the same blind spot, by a guard written to
# catch exactly it.
#
# So the census now works on LINE RANGES instead of headers: it computes each production
# function's body span from the source and reports the function when a hunk overlaps it. That
# sees methods, and it is also tighter — a hunk landing BETWEEN two functions now belongs to
# neither, where the header heuristic would have blamed whichever came first.
#
# ATTRIBUTION IS STILL AN UPPER BOUND at one edge: a function whose body merely CONTAINS an
# added item is reported. Rather than quietly widen the scope (which would claim mutation
# coverage that was never run) or quietly narrow the census (which is how both misses
# happened), every reported name must appear in one of two sections — and `## not-changed`
# entries carry a reason.
set -eu
BASE=${1:-HEAD~1}
SCOPE=specs/ai-monitor-idle-timestamps/mutants-scope.txt
test -f "$SCOPE" || { echo "FAIL: $SCOPE missing — record the mutation scope actually run"; exit 1; }

mutated=$(sed -n '/^## mutated/,/^## /p' "$SCOPE" | grep -E '^[a-z_]' || true)
unchanged=$(sed -n '/^## not-changed/,$p' "$SCOPE" | sed -n 's/^\([a-z_][a-z0-9_]*\)[[:space:]]*—.*/\1/p' || true)

# Production only: an integration-test file is all test functions, which cargo mutants skips.
PATHSPEC="crates/**/*.rs"
EXCLUDE=":(exclude)crates/*/tests/**"

hunks=$(mktemp)
trap 'rm -f "$hunks"' EXIT

# New-side line ranges, one per hunk: "<path> <first> <last>". A pure deletion (len 0) is
# recorded as the single line it sits between, so removing a function's guts still reports it.
git diff -U0 "$BASE" -- "$PATHSPEC" "$EXCLUDE" \
  | awk '
      /^\+\+\+ b\// { path = substr($0, 7); next }
      /^@@/ {
        match($0, /\+[0-9]+(,[0-9]+)?/)
        spec = substr($0, RSTART + 1, RLENGTH - 1)
        n = split(spec, a, ",")
        s = a[1] + 0
        len = (n > 1) ? a[2] + 0 : 1
        if (s < 1) s = 1
        print path, s, (len == 0 ? s : s + len - 1)
      }
    ' > "$hunks"

changed=$(
  cut -d' ' -f1 "$hunks" | sort -u | while read -r path; do
    [ -f "$path" ] || continue
    awk -v target="$path" '
      # Pass 1: this file argument is the hunk list.
      FNR == NR { if ($1 == target) { hs[++hn] = $2; he[hn] = $3 } ; next }

      # Pass 2: slurp the source, noting where the test half begins.
      #
      # The boundary is `#[cfg(test)]` followed by `mod tests {`, which is the same two-line
      # marker `production_half` splits on in the Rust sources — NOT a bare `#[cfg(test)]`.
      # A file may carry test-only items at column 0 long before its test module (`seam.rs`
      # has a `#[cfg(test)] pub const` at line 65), and treating the first one as the boundary
      # silently drops every function below it. That hid `notify_active_changed` here.
      { L[FNR] = $0; last = FNR }
      /^mod tests \{/ { if (!cut && FNR > 1 && L[FNR - 1] == "#[cfg(test)]") cut = FNR - 1 }

      END {
        if (!cut) cut = last + 1
        for (i = 1; i < cut; i++) {
          if (!match(L[i], /^[[:space:]]*(pub[^ ]* )?(async )?(unsafe )?fn [a-z_][a-z0-9_]*/)) continue
          # A trait method declaration has no body to overlap.
          if (L[i] ~ /;[[:space:]]*$/) continue
          # A `#[cfg(test)]` helper sitting in the production half is still test code, and
          # cargo mutants generates no mutant for it — so reporting it would demand a scope
          # entry that no run could ever satisfy. (`AiMonitorOverlay::detached` is one.)
          if (i > 1 && L[i - 1] ~ /^[[:space:]]*#\[cfg\(test\)\][[:space:]]*$/) continue
          indent = match(L[i], /[^ ]/) - 1
          match(L[i], /fn [a-z_][a-z0-9_]*/)
          name = substr(L[i], RSTART + 3, RLENGTH - 3)
          if (L[i] ~ /\}[[:space:]]*$/) {
            # A body written inline on the signature line, e.g. the empty `fn record(..) {}`
            # stubs seam.rs nests inside `Overlay::layer`. Searching forward for a closer
            # would run past the end of the real function and swallow every hunk in between,
            # which is how a hunk in the TRAIT got attributed to one of those stubs.
            endl = i
          } else {
            # Otherwise rustfmt closes the body with `}` at the indent of its own signature.
            closer = "}"
            for (k = 0; k < indent; k++) closer = " " closer
            endl = i
            for (j = i + 1; j < cut; j++) if (L[j] == closer) { endl = j; break }
          }
          for (h = 1; h <= hn; h++) if (!(he[h] < i || hs[h] > endl)) { print name; break }
        }
      }
    ' "$hunks" "$path"
  done | sort -u
)

test -n "$changed" || { echo "PASS: no changed Rust functions to scope"; exit 0; }

fail=0
for f in $changed; do
  if echo "$mutated" | grep -qx "$f"; then
    printf '  mutated      %s\n' "$f"
  elif echo "$unchanged" | grep -qx "$f"; then
    printf '  not-changed  %s\n' "$f"
  else
    printf '  UNACCOUNTED  %s\n' "$f"
    fail=1
  fi
done

if [ $fail -ne 0 ]; then
  echo "FAIL: a function the diff touches is neither mutation-tested nor recorded as unchanged."
  echo "      Run: cargo mutants -j 4 -f <file> -F '<fn>'  then add it under '## mutated',"
  echo "      or — if only an item nested inside it moved — add it under '## not-changed' with why."
  exit 1
fi
echo "PASS: every function the diff touches is accounted for"
