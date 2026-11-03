# Loop contract: mouse-drag-reporting

## Goal
Verify the implementation of `specs/mouse-drag-reporting/spec.md` against its `## Requirements`. The implementation is already complete.

Approved by the user on 2026-10-30.

## Gate 1 — Pass/Fail (100%)
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace --no-fail-fast -- --skip test_live_session_29b623ae_file_if_present`
  - The excluded test replays a live `~/.gemini` agy transcript. Its crate (`ai-monitor`) is untouched and does not depend on `app`, so the exclusion is environmental.

## Gate 2 — Quantitative
| metric | threshold |
|--------|-----------|
| surviving mutants: `cargo mutants -f crates/app/src/input.rs -F 'encode_mouse_motion_report\|reported_drag_motion\|pixel_to_cell_clamped\|drag_owns_pointer\|reports_drag_motion\|grid_release_action'` | 0 |
| tests passing across `tt1_mouse_motion_encoding`, `tt2_reported_drag_motion`, `tt3_drag_cell_clamp` and `tt4_drag_bypass` | 22/22 |

## Gate 3 — Qualitative (checker-scored)
- (a) The implementation matches decisions D1–D7 of the spec.
- (b) C1 holds: no change under `crates/term-core` or `crates/renderer`.
- (c) The code reads like its surroundings: comment density, naming, idiom.

## Lessons
