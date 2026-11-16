# Loop contract — local-command-not-a-prompt

pre-approved-unattended-spec: yes

## Goal
Verify the implementation of `specs/local-command-not-a-prompt/spec.md` against its
`## Requirements` (R0–R4). Implementation is already complete; this loop verifies, and only
fixes within the Auto-fix list.

## Out of Scope (parked)
(none — no task was parked)

## Gate 1 — Pass/Fail (100%)
- build: `cargo build`
- tests: `cargo test --workspace` (includes the acceptance tests listed in
  `specs/local-command-not-a-prompt/gwt-tests.md`)
- lint: `cargo clippy --workspace --all-targets -- -D warnings`
- format: `cargo fmt --all -- --check`
- typecheck: covered by `cargo build` (Rust has no separate typecheck step)

## Gate 2 — Quantitative
| metric | threshold | how measured |
|--------|-----------|--------------|
| acceptance-test files unchanged since the requirement check | 4 of 4 hashes match `gwt-tests.md` | `shasum -a 256` |
| mutation survivors in the functions this change modified | 0 | `cargo mutants -j 4 --test-package ai-monitor` scoped as in the run report (re-run only if those files changed after the recorded run) |
| `ai-monitor` test failures | 0 | `cargo test -p ai-monitor` |

## Gate 3 — Qualitative (checker-scored)
- **Spec alignment** — the diff implements D1–D11 as written (caveat-before OR output-after
  marking of a `user` wrapper; skip in every walk; D9 widening only after a skipped `user`
  record; digest caveat flag; G8 docs), and builds no Non-goal.
- **Safety bias** — no path in the diff can turn a reading into `Waiting` that the
  pre-change code did not (spec Constraint 1).
- **Clarity** — names and doc comments say what each recogniser matches and why, citing the
  spec decisions, in the codebase's existing style.

Bar: each item ≥ 8/10, with grounds and action.

## Constraints carried from the spec
- Do not modify any test file listed in `gwt-tests.md`.
- No change outside `crates/ai-monitor` and `docs/architecture.md` (run artefacts under
  `specs/local-command-not-a-prompt/` excepted); `Cargo.lock` untouched.

## Lessons

### L1 — a text-prefix recogniser must not match a typed human prompt
- cause: escalation
- source: auto-unattended
- form: gate3-item
- status: active
- check: Safety bias — every record recognised as local-command (caveat, output, wrapper) by text prefix alone is also refused when it carries `origin.kind: "human"` (or `turnOrigin`), and a `tail_state` test pins a human prompt beginning `<local-command-stdout>` as `UserPrompt`.
- note: D1 marks records by text prefix only, so a typed prompt starting with a tag would be skipped and read as a false Waiting; measured 0 such prompts (2026-11-15T01:05Z), but the spec's own Constraint 1 forbids the path.
