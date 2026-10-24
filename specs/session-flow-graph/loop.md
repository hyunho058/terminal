# Loop Contract: session-flow-graph verification

## Goal
Verify the implementation of `specs/session-flow-graph/spec.md` (T1–T4, TT1–TT12) against its
`## Requirements` R0–R13. Implementation is complete; the loop verifies first and fixes only
inside the Auto-fix boundary.

## Gate 1 — Pass/Fail (100%)
- `cargo build --workspace`
- `cargo test --workspace`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo fmt --check`
- `shasum -a 256 -c specs/session-flow-graph/tt-hashes.txt` (the acceptance tests are the ones written before the code)

## Gate 2 — Quantitative
| metric | threshold |
|---|---|
| `cargo mutants -f crates/app/src/flow.rs -j 4` non-equivalent survivors | 0 |
| `cargo mutants -f crates/renderer/src/graph_layout.rs -j 4 -- --lib` non-equivalent survivors (full pass) | 0 |
| `cargo mutants -f crates/renderer/src/graph_panel.rs -j 4 -- --lib` non-equivalent survivors | 0 |
| `cargo mutants -f crates/app/src/tabmeta/git.rs -j 4` non-equivalent survivors | 0 |
| `cargo mutants -f crates/app/src/tabmeta/worker.rs -j 4` non-equivalent survivors | 0 |
| `cargo mutants -f crates/app/src/tabmeta/mod.rs -j 4` non-equivalent survivors | 0 |
| `cargo mutants -f crates/app/src/seam.rs -j 4` non-equivalent survivors | 0 |
| `cargo mutants -f crates/app/src/input.rs -F pointer_owner -j 4` non-equivalent survivors | 0 |

A survivor counts as equivalent only with a written argument; per CLAUDE.md, prefer rewriting
the condition over an `exclude_re` entry. Survivor counts are recorded, not ratios. A timeout
is re-run serially before it is counted either way.

## Gate 3 — Qualitative (checker-scored, each ≥ 8/10 with grounds + action)
1. **Spec alignment**: every R1–R13 sub-requirement's Then is implemented as written (R0.1 is manual and listed as such).
2. **Single classifier (D3)**: session status flows only through `row_meta` / `resolve_status`; no second status derivation.
3. **Idle cost (D11/G1)**: no graph rebuild or relayout on an unchanged key; no pane, repo or telemetry work while the graph is closed.
4. **Boundaries**: renderer imports no app/winit types; term-core untouched; no GUI crates; no `unwrap()` outside tests.
5. **Code and docs fit**: naming, comment density and doc comments match the surrounding code; `docs/architecture.md` G10 and `docs/domain.md` describe what was built.

## Manual checks (not gated; reported)
- R0.1 — project card with two worktree session cards under it, live working count, in the running app.

## Known open item (for the human, not auto-fixed)
- **Resolved 2026-10-21** by the spec's D9 amendment (`EDGE_ALPHA` 0.45, `graph_layout.rs`). Was: D9 edge colour (`card_border_active` at `CARD_BORDER_ALPHA` 0.10) measures ~10 sRGB counts above the
  backdrop: TT12 passes ("differs from backdrop") but edges are barely visible. Changing it changes an
  approved decision.

## Lessons

### L1 — a flaky test fakes mutant kills
- cause: gate-fail
- source: human-approved
- form: context
- status: active
- check: —
- note: Under `cargo mutants`, `tabmeta::agent::tests::a_live_agent_survives_its_binary_being_deleted` fails in every mutant's sandbox, so a mutant "caught" only by it is a false kill. Read each caught mutant's log and confirm a test aimed at the mutated code failed (or re-run with `-- -- --skip a_live_agent_survives_its_binary_being_deleted`).

### L2 — renderer GPU tests time out under parallel mutants
- cause: gate-fail
- source: human-approved
- form: context
- status: active
- check: —
- note: With every renderer test target at `-j 4`, GPU integration tests report as timeouts, not catches; renderer mutant runs use lib tests (`-- --lib`), and the pixel half is covered by Gate 1's `cargo test`.

### L3 — added doc comments stay within 100 columns
- cause: gate-fail
- source: human-approved
- form: gate1-command
- status: active
- check: git diff -U0 -- '*.rs' | awk '/^\+[^+]/ && length>101 && /^\+ *\/\/[\/!]/{n++} END{exit n>0}'
- note: Gate 3 item 5 failed three times on stale wording and over-long doc comments; this catches the width half mechanically. It reads the tracked diff only, so untracked new files are covered once they are added.

### L4 — cargo-mutants flags that abort a run
- cause: gate-fail
- source: human-approved
- form: context
- status: active
- check: —
- note: `--timeout` also caps the unmutated baseline, which under load aborts every file untested; use `--minimum-test-timeout 300 --timeout-multiplier 3`. For `app` files pass `--cargo-test-arg -p --cargo-test-arg app` (nothing depends on `app`). In zsh scripts put `-o` before `--` and write skip flags literally (`$VAR` is not word-split).
