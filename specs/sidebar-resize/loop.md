# loop.md — sidebar-resize

## Goal
Implement `specs/sidebar-resize/spec.md`: make the left tab sidebar resizable by dragging a
thin grid-side grab-band at its right edge, live, clamped to the existing Warp-style minimum
(`SIDEBAR_WIDTH_MIN`) via the existing `renderer::set_sidebar_width`; the tab list + terminal
grid re-fit each frame; no handle when collapsed (draw + hit-test single-sourced). Tasks T1→T2→T3.

## Gate 1 — Pass/Fail (binary, 100%)
- `cargo build` succeeds
- `cargo clippy --workspace --all-targets -- -D warnings` clean
- `cargo test --workspace` green

## Gate 2 — Quantitative (numbers vs thresholds)
- New unit tests ≥ **3**, all passing: `resize_handle_rect` returns the grid-side band
  `[sidebar_w, sidebar_w+grab]` when expanded and `None` when collapsed; `hit_test` returns
  `SidebarRegion::ResizeHandle` for a band point and the unchanged region for `x < sidebar_w`.
- Files match the declared surface: NEW `crates/renderer/src/sidebar_resize.rs` +
  `crates/app/src/sidebar_resize.rs`; edits to `sidebar_layout.rs`, `renderer/lib.rs`,
  `input.rs`, `terminal_app.rs`. **Zero** edits under `crates/term-core/**` (SD2).

## Gate 3 — Qualitative (judged, with grounds + action)
- **geometry-single-source** — `resize_handle_rect(inp) -> Option<Rect>` is the ONE source for
  BOTH the draw layer and the hit-test, and the collapsed guard (`None`) is honored by both
  (SD3, D5, D10). No second collapsed check.
- **clamp-reuse** — resizing routes width through the existing `set_sidebar_width` (which clamps
  to `[SIDEBAR_WIDTH_MIN, SIDEBAR_WIDTH_MAX]`); the Warp-style minimum is reused, not reinvented,
  and the app adds no parallel bound (D7, SD4).
- **no-regression** — collapse/expand toggle, sidebar scroll, tab-click switch, and the existing
  text-selection drag still work; the grab-band is grid-side/thin so it never steals sidebar-body
  or chrome presses (D5 constraint).
- **dependency-direction** — `term-core` untouched (SD2); no GUI crates added (SD1);
  the change stays renderer + app.

## Autonomy note
No STOP-list items (no schema/auth/payment/migration). Mouse-drag + GPU highlight can't be
unit-tested; the gates cover the pure geometry + build/clippy/tests, and final visual/interaction
confirmation is a live run (`/run-term`).
