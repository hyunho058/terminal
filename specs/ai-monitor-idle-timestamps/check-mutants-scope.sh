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
# ATTRIBUTION IS AN UPPER BOUND, and the scope file is explicit about it. A diff hunk header
# names the nearest PRECEDING `fn`, so a hunk added just after a function is attributed to it.
# Rather than quietly widen the scope (which would claim mutation coverage that was never run)
# or quietly narrow the heuristic (which is how the real miss happened), every reported name
# must appear in one of two sections — and `## not-changed` entries carry a reason.
set -eu
BASE=${1:-HEAD~1}
SCOPE=specs/ai-monitor-idle-timestamps/mutants-scope.txt
test -f "$SCOPE" || { echo "FAIL: $SCOPE missing — record the mutation scope actually run"; exit 1; }

mutated=$(sed -n '/^## mutated/,/^## /p' "$SCOPE" | grep -E '^[a-z_]' || true)
unchanged=$(sed -n '/^## not-changed/,$p' "$SCOPE" | sed -n 's/^\([a-z_][a-z0-9_]*\)[[:space:]]*—.*/\1/p' || true)

# Production only: an integration-test file is all test functions, which cargo mutants skips.
changed=$(git diff -U0 "$BASE" -- 'crates/**/*.rs' ':(exclude)crates/*/tests/**' \
  | grep -E '^@@.*@@' \
  | sed -n 's/.*@@[^@]*@@[[:space:]]*//p' \
  | grep -oE '\bfn [a-z_][a-z0-9_]*' \
  | sed 's/^fn //' \
  | sort -u)

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
  echo "      or — if the hunk header merely named it — add it under '## not-changed' with why."
  exit 1
fi
echo "PASS: every function the diff touches is accounted for"
