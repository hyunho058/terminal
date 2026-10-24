# Loop Contract: session-flow-viewer verification

## Goal
Verify the implementation of `specs/session-flow-viewer/spec.md` (T1–T4, TT1–TT9) against its
`## Requirements` R0–R9. Implementation is complete; the loop verifies first and fixes only
inside the Auto-fix boundary.

## Gate 1 — Pass/Fail (100%)
- `cargo build --workspace`
- `cargo test --workspace`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo fmt --check`
- `shasum -a 256 -c specs/session-flow-viewer/tt-hashes.txt` (the acceptance tests are the ones written before the code)

## Gate 2 — Quantitative
| metric | threshold |
|---|---|
| `cargo mutants -f crates/app/src/flow.rs -j 4` non-equivalent survivors | 0 |
| `cargo mutants -f crates/app/src/flow_telemetry.rs -j 4` non-equivalent survivors | 0 |
| `cargo mutants -f crates/renderer/src/flow_layout.rs -j 4` non-equivalent survivors | 0 |
| `cargo mutants -f crates/renderer/src/flow_panel.rs -j 4` non-equivalent survivors | 0 |
| `cargo mutants -f crates/app/src/tabmeta/tmux.rs -j 4` non-equivalent survivors | 0 |

A survivor counts as equivalent only with a written argument; per CLAUDE.md, prefer rewriting
the condition over an `exclude_re` entry. Survivor counts are recorded, not ratios.

## Gate 3 — Qualitative (checker-scored, each ≥ 8/10 with grounds + action)
1. **Spec alignment**: every R1–R9 sub-requirement's Then is implemented as written (R0.1/R1.6 are manual and listed as such).
2. **Single classifier (D2/Constraint)**: session status flows only through `row_meta` / `resolve_status`; no second status derivation.
3. **Idle cost (D11/G1)**: no model rebuild or relayout on an unchanged key; no pane or telemetry work while `dock != Flow`.
4. **Boundaries**: renderer imports no app/winit types; term-core untouched; no GUI crates; no `unwrap()` outside tests.
5. **Code and docs fit**: naming, comment density and doc comments match the surrounding code; `docs/architecture.md` G10 and `docs/domain.md` describe what was built.

## Manual checks (not gated; reported)
- R0.1 — working count changes live across two Claude tabs.
- R1.6 — Cmd+Shift+F replaces the monitor in the same band at the same width.

## Lessons

### L1 — a flaky test fakes mutant kills
- cause: gate-fail
- source: human-approved
- form: context
- status: active
- check: —
- note: Under `cargo mutants`, `tabmeta::agent::tests::a_live_agent_survives_its_binary_being_deleted` fails in every mutant's sandbox, so a mutant "caught" only by it is a false kill (it hid `Memo::fresh`/`PaneMemo::get` `<`→`<=` and two `flow.rs` survivors). Read each caught mutant's log and confirm a test aimed at the mutated code failed.

## Results (2026-10-19)
- Gate 1: ✅ build, clippy -D warnings, fmt, tt hashes, `cargo test --workspace` 2182 passed / 0 failed.
- Gate 2 survivors (first run → final): flow_layout.rs 34 → 0; flow_panel.rs 22 → 0; flow.rs 22 → 0; flow_telemetry.rs 9 → 0; tabmeta/tmux.rs 5 → 2 equivalent (`Memo::fresh` and `PaneMemo::get` `elapsed() < TICK` → `<=`: differ only at elapsed == TICK to the nanosecond on a monotonic clock, unobservable; the `Memo::fresh` one predates this branch).
- Gate 3: 8/9/8/9/8 (checker-scored, iteration 1).
