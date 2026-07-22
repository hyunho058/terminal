# loop.md — Verification contract: left-sidebar-waiting-area

**Goal:** Verify the already-complete implementation of
`specs/left-sidebar-waiting-area/spec.md` against its `## Requirements` (R0–R7).
Implementation is complete (T1–T4); this loop VERIFIES, it does not rebuild.

## Gate 1 — Pass/Fail (binary, 100%)
- **build**: `cargo build --workspace`
- **tests**: `cargo test --workspace` — 0 failures
- **lint**: `cargo clippy --workspace --all-targets -- -D warnings`
- **fmt**: `cargo fmt --all -- --check`

## Gate 2 — Quantitative (measured vs threshold)
| metric | threshold |
|--------|-----------|
| test failures | = 0 |
| clippy warnings | = 0 |
| policy requirements with ≥1 automated test (R2,R3,R5,R6) | = 4 / 4 |
| new/added tests across T1–T4 | ≥ 30 |

## Gate 3 — Qualitative (checker-scored via separate subagent)
- **spec-alignment**: every requirement R0–R7 is satisfied by the diff (or, if runtime-only, has a test/plumbing that establishes it). grounds = file:line per requirement; action.
- **false-positive-defense**: the top quality bar (minimize false positives) is actually enforced — error ignore-list {1,130}, conservative-on-truncated jsonl, active-tab exclusion, rising-edge Waiting, 30s completion threshold. grounds = file:line; action.
- **dependency-direction**: `term-core` stays free of wgpu/winit and of UI policy (thresholds/ignore-list/keying live in `app`, per D21); renderer owns its own waiting types (no app dependency). grounds; action.
- **rising-edge-edge-case**: the known edge (agent enters Waiting while the tab is active, then left without a further state change → no band item) is either consistent with R2.3/R0.1 or a real defect. grounds; action.

## Lessons
(none yet)
