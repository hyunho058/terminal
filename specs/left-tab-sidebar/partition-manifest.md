# partition-manifest.md — left-tab-sidebar

- generated_at: 2026-07-19T07:03:09Z
- goal: Move the tab list from the top horizontal bar to a left vertical sidebar. Add a `left_inset` to the renderer (mirroring `set_top_inset`) so the grid narrows by the sidebar width; rework `tab_bar.rs` to a vertical (top→bottom) layout; keep the Cmd+T/W, next/prev, and Cmd+1..9 chords; add click-to-switch (clicking a tab activates it); keep `cargo build` and the insta snapshot suite green.

## Shared Decisions

### SD1: The reserved tab band moves from a top inset (reserves rows) to a left inset (reserves columns)
- **Status**: resolved
- **Rationale**: The sidebar is vertical on the left, so the grid must shrink in *width*, not height. A `left_inset` (physical px reserved left of the grid) mirrors the existing `top_inset` seam exactly: `grid_dims` subtracts it from usable width so `cols` shrink while `rows` stay full; the bg/glyph shaders shift `px.x` right by `left_inset` just as they shift `px.y` by `top_inset`; `FrameCtx` carries it so the tab layer reads one authoritative value. `top_inset` is kept in the renderer (additive, unremoved) but driven to `0.0` by the app once the sidebar is on the left. Rejected: repurposing/renaming `top_inset` (breaks the additive-seam precedent and the cursor/image layers that already read `top_inset`); a separate width-only code path (duplicates the proven inset arithmetic).

### SD2: Tab-strip geometry is defined ONCE in `tab_bar.rs` as pure functions; the app's click hit-test consumes the SAME functions
- **Status**: resolved
- **Rationale**: Click-to-switch must map a cursor (x, y) to a tab index using the exact geometry the layer draws with. Duplicating the layout math in the app would drift. So `tab_bar.rs` owns the geometry as pure, unit-testable free functions (extending the existing `height_px(cell_h)` precedent), named authoritatively so the consumer imports the right symbols: `width_px(cell_w) -> f32` (the sidebar width the app feeds to `set_left_inset`), `tab_height_px(cell_h) -> f32` (the per-tab slot height / stride), a shared forward `slot_y(index, tab_h) -> f32` the draw path uses to place each slot's top, and its exact inverse `tab_at(x, y, sidebar_w, tab_h, count) -> Option<usize>` the click path uses. Because slots have no top pad or inter-slot gap, `slot_y` (`index*tab_h`) and `tab_at` (`floor(y/tab_h)`) are exact inverses; a round-trip unit test pins their agreement so drawing and hit-testing cannot silently diverge. The app calls `set_left_inset(width_px(cell_w))` and, on a left-button press inside the sidebar, routes through the hit-test → `Tabs::jump(i)`; the SAME `set_left_inset(width_px(..))` wiring runs on the config-reload path so the reserved band never goes stale against the drawn sidebar. Rejected: computing tab bounds independently in the app (two sources of truth, guaranteed drift); exposing only raw slot rectangles (pushes the floor/bounds logic back into the caller, re-opening the drift this closes).

### SD3: Preserve the existing tab architecture and chords; click-to-switch is purely additive
- **Status**: resolved
- **Rationale**: The tab set (`Tabs`), the `TabAction` chord table (Cmd+T/W, Cmd+Shift+[/], Cmd+1..9), the one-way `TabModelHandle` data flow (app writes `TabBarModel`; renderer only reads), the `DrawLayer` registration seam (`add_layer`, z-order), and the enforced dependency direction `common ← term-core ← renderer ← app` all stay intact. The renderer never reaches into `crates/app`; the app never hands the renderer a live `Grid`. Click routing is a new `WindowEvent::MouseInput`/`CursorMoved` handler in the app that reuses the existing `Tabs::jump` + `sync_tab_model` + `redraw` path — no new switching logic. Rejected: adding a widget/GUI crate for hit-testing (violates the `no-gui-crates` rule — draw and hit-test on the cell grid ourselves).

### SD4: Every feature lands green under the project's fixed gates, including the insta suite
- **Status**: resolved
- **Rationale**: Each feature must leave `cargo build`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace` (which includes the `term-core` insta snapshot tests) passing, with no `unwrap()` outside tests and per-crate `thiserror` error enums. The vte→grid insta snapshots construct grids at fixed sizes and are not expected to change; if any snapshot output shifts, it is reviewed and accepted via `/insta-review` rather than blindly overwritten. Renderer inset arithmetic stays a unit-testable free function (no GPU needed), matching the `grid_dims`/`height_px` test precedent. Rejected: skipping clippy/tests between features (each spec is independently verifiable and build-order-ready).

## Features  (ordered — a dependency appears before its dependents)

### renderer-left-inset
- declared-surface:
  - crates/renderer/src/lib.rs
  - crates/renderer/src/cells.wgsl
  - crates/renderer/src/glyph.wgsl
- depends_on: []
- accept-seed: The renderer reserves a caller-set left band — `set_left_inset(px)` shifts the grid and glyphs right by that many px and `grid_size()` reports fewer columns (rows unchanged), verified by a headless `grid_dims` unit test; with the inset at 0.0 rendering is byte-unchanged.
- pre-approved-batch: yes

### vertical-tab-sidebar
- declared-surface:
  - crates/renderer/src/tab_bar.rs
- depends_on: [renderer-left-inset]
- accept-seed: The tab bar draws as a vertical left sidebar (tabs stacked top→bottom in the reserved left inset, active tab highlighted, titles truncated to the sidebar width) and exposes pure geometry functions (sidebar width + a cursor→tab-index hit-test) covered by unit tests.
- pre-approved-batch: yes

### tab-click-switch
- declared-surface:
  - crates/app/src/terminal_app.rs
  - crates/app/src/input.rs
  - crates/app/src/settings.rs
- depends_on: [renderer-left-inset, vertical-tab-sidebar]
- accept-seed: The app reserves the left sidebar via `set_left_inset` on both startup and config-reload, sizes the grid to it, and clicking a tab in the sidebar switches to that tab (via the shared hit-test → `Tabs::jump`); Cmd+T/W, next/prev, and Cmd+1..9 chords all still work.
- pre-approved-batch: yes
