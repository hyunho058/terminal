# loop.md — agent-classifier-deleted-binary

> Verification contract for the implementation of
> `specs/agent-classifier-deleted-binary/spec.md` (approved L4, 2026-08-29).
> The implementation is already complete; this loop verifies it against the spec's
> `## Requirements` (R0–R4, 23 GWT sub-requirements).

- created: 2026-08-29
- type: verification (post-implementation gate for agent-orchestrate Phase 3.5)
- autonomy: Auto-fix class only — lint/format/type errors, missing tests for code already in
  scope, doc/comment updates, local renames, behaviour-preserving refactors. Anything that
  changes a spec decision (D1–D10) crosses the fence and escalates.

## Goal
Every requirement in the spec is satisfied by the working tree, proven by commands and by a
checker that did not write the code — or the gap is reported honestly rather than closed.

## Gate 1 — Pass/Fail (100% required)

The project's own CI set (`.github/workflows/ci.yml:28-37`), plus the two checks this spec
adds as hard requirements:

1. `cargo fmt --all --check` — R4.3
2. `cargo clippy --workspace --all-targets -- -D warnings` — R4.3, and the specific corner
   R2.7 names: `classify_with` becomes non-test-dead and must carry `#[allow(dead_code)]`
3. `cargo test --workspace` — R4.2 (the untouched suite) and R4.3
4. `cargo build --workspace --release` — R4.3
5. **The regression test must fail on the pre-fix code** — R0.1's "where on the current code
   it returns `None`". A test that passes both before and after proves nothing. Runnable,
   and it restores the tree on any exit path:

   ```bash
   set -eu
   F=crates/app/src/tabmeta/agent.rs
   B=$(mktemp); cp "$F" "$B"; trap 'cp "$B" "$F"; rm -f "$B"' EXIT
   # Repoint classify_pid at the pre-fix entry point (no name fallback).
   python3 - "$F" <<'PY'
   import sys
   p = sys.argv[1]
   s = open(p).read()
   new = ("    classify_with_name(\n"
          "        path.as_deref(),\n"
          "        || super::proc::name_of_pid(pid),\n"
          "        || super::proc::proc_args(pid),\n"
          "    )")
   old = "    classify_with(path.as_deref(), || super::proc::proc_args(pid))"
   assert s.count(new) == 1, "classify_pid no longer has the shape this check mutates"
   open(p, "w").write(s.replace(new, old))
   PY
   # The live test MUST fail without the fallback…
   if cargo test -p app --bin app a_live_agent_survives >/dev/null 2>&1; then
     echo "FAIL: the regression test passes on the pre-fix code — it proves nothing"; exit 1
   fi
   cp "$B" "$F"
   # …and MUST pass with it.
   cargo test -p app --bin app a_live_agent_survives >/dev/null 2>&1 \
     || { echo "FAIL: the regression test does not pass on the fixed code"; exit 1; }
   echo "OK: the regression test discriminates fixed from pre-fix"
   ```
6. **No pre-existing test was modified** — the spec's Constraints ("no pre-existing test may
   be modified — not its assertions, not its inputs, and not even its call syntax") and R2.7:

   ```bash
   test -z "$(git diff -U0 | grep -E '^-[^-]' | grep -E '#\[test\]|assert|classify_with\(Some|classify_with\(None')"
   ```

## Gate 2 — Quantitative

| metric | threshold | source |
|---|---|---|
| test suites failing | 0 | R4.2/R4.3 |
| `PROC_NAME` reads when `exe_path` is readable | exactly 0 | R4.1, D8, D10 |
| `PROC_EXE` reads per `classify_pid` on a readable path | exactly 1 | R4.1 |
| `PROC_ARGS` reads on the pathless non-interpreter branch | exactly 0 | R2.3 |
| pre-existing test lines changed | 0 | R4.2, Constraints |
| spec sub-requirements with a named verification (test, command, or explicit manual note) | 23 of 23 | R0–R4 |

## Gate 3 — Qualitative (scored by a checker subagent, not by the maker)

- **spec-fidelity** — does the code implement D1–D10 *as written*, especially D3's wrapper
  (arity preserved, `#[allow(dead_code)]`, not `#[cfg(test)]`), D4's deliberate divergence
  from the `unwrap_or(0)` idiom, and D2's stage mapping?
- **test-faithfulness** — does the live test reproduce the *production sequence*
  (exec as `agy` → rename → unlink), or has it degraded into a proxy that would pass
  without the bug being fixed?
- **fence** — did the implementation stay inside the spec's declared files
  (`tabmeta/proc.rs`, `tabmeta/mod.rs`, `tabmeta/agent.rs`) with no scope creep?
- **voice** — do the new comments explain *why* at the density of the surrounding module,
  without restating the code?

## Lessons

### L1 — a test that passes for every implementation is not a test
- cause: gate-fail
- source: human-approved
- form: gate3-item
- status: active
- check: "No new or changed test asserts something that holds for *any* implementation of the
  code under test — in particular an equality between two identical calls to a pure function,
  or an assertion whose both sides go `None` together when the feature is removed. Every new
  test must be shown to fail when the behaviour it covers is disabled at its source."
- note: R2.8's first test compared `classify_with_name(None, name, argv)` with itself and
  therefore passed even with the classifier broken; the requirement read as covered while
  nothing verified it. Its replacement is one of six tests that fail when the fallback is
  disabled.

### L2 — a regression test must be proven to fail on the pre-fix code
- cause: gate-fail
- source: human-approved
- form: gate1-command
- status: active
- check: the mutation script in Gate 1 item 5 — repoint `classify_pid` at `classify_with`,
  require `cargo test -p app --bin app a_live_agent_survives` to exit **non-zero**, restore,
  require it to exit **zero**.
- note: this bug shipped twice before behind tests that were green both before and after the
  fix (`antigravity-agy-detection`, `agent-skin-detection-fix` both closed on a manual probe).
  A test that cannot fail on the old code proves nothing about the new code.
