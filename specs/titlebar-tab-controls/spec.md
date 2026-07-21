# Spec: titlebar-tab-controls

## Meta
- **Created**: 2026-07-21
- **Type**: dev
- **Status**: drafting
- **Batch**: sidebar-titlebar (partition-manifest.md)
- **Amended 2026-07-21**: also relocate the sidebar collapse/expand toggle to the title bar (Warp-style) — D11/D14, R0/R2.3/R5/R7, T2–T4 (user-approved via /specify).

## Goal
Move the tab open (new) / close controls AND the sidebar collapse/expand (fold/unfold) toggle to a top title bar (Warp-style), relocated out of the sidebar header.

## Non-goals
- Replacing or hiding the native macOS window title bar, or making the new strip a window-drag region (needs objc2 titlebar customization — deferred).
- Per-tab close (×) buttons inside the sidebar tab rows (the title bar hosts a single close-active control).
- Persisting any title-bar state to config.

## Confirmed Goal
A full-width **title bar** occupies the topmost band of the client area (`y ∈ [0, top_inset]`), reserved via the existing (currently unwired) `set_top_inset`. It hosts a **new-tab (`+`)** button, a **close-active-tab (`×`)** button, and a **sidebar collapse/expand toggle**. Clicking `+` opens a new tab (as `TabAction::New`); `×` closes the active tab (as `TabAction::CloseActive`, exiting on the last tab — identical to Cmd+W); the **toggle folds/unfolds the sidebar** (`toggle_sidebar(!collapsed)` via the existing `set_sidebar_collapsed`). All three are **removed** from the sidebar: the header's `NewTab`/`HideSidebar` buttons AND the collapsed `Restore` button — the title bar is always visible, so its toggle serves both collapse and restore. The sidebar + terminal grid begin **below** the title bar (`y >= top_inset`) so the bar is topmost.

## Research
- No title-bar band exists today: `top_inset` is a `Renderer` field initialized to `0.0` and is always 0; `set_top_inset(px)` exists but has **zero callers** — this feature's hook (`crates/renderer/src/lib.rs:193,525,531`).
- `grid_dims` already reserves rows for `top_inset` (`crates/renderer/src/lib.rs:1283-1288`), and shaders/layers already offset by `FrameCtx.top_inset` (`cells.wgsl:11/33`, `glyph.wgsl:7/35`, `cursor_layer`/`selection_layer`/`image_layer`) — so a non-zero `top_inset` shifts the terminal grid down automatically.
- The sidebar chrome to relocate: `sidebar_header::build_shapes` draws `NewTab→Icon::Plus` and `HideSidebar→Icon::Panel` (`crates/renderer/src/sidebar_header.rs:155-183`); the collapsed `Restore` button at `:107-127`. `SidebarChromeLayer`/`SIDEBAR_CHROME_Z = 30` (`:66,27`).
- Chrome regions + geometry: `ChromeRegion { NewTab, HideSidebar, Restore }` and `SidebarRegion` (`crates/renderer/src/sidebar_layout.rs:67,76`); `button_rect` right-aligns buttons at `sidebar_w - header_pad_x` (`:146`); `header_rect` (`:135`); `chrome_hit`/`hit_test` (`:299,316`). The sidebar's vertical origin is `y = 0` today (header at top) — it must shift to `top_inset`.
- Tabs API: `do_tab_action(TabAction::New | CloseActive)` (`crates/app/src/terminal_app.rs:617`) already opens / closes tabs and exits on the last close; `Tabs::open`/`close_active` (`crates/app/src/tabs.rs:80,91`) need no change.
- `add_layer` registration sites (`crates/app/src/terminal_app.rs:219-246`); a new titlebar `DrawLayer` registers here. `set_top_inset` would be called at startup + on resize/scale change.
- Icons: `sidebar_icons::Icon::{Plus, Panel}` + `push_icon` geometry (`crates/renderer/src/sidebar_icons.rs:17,105`) — reusable for the title-bar `+`; a close `×` icon is added.
- App mouse press arm (`crates/app/src/terminal_app.rs:870`), `sidebar_input()` (`:532`); input seam `sidebar_press`/`SidebarAction` (`crates/app/src/input.rs:82,103`).
- Ordered-share note: this feature `depends_on: [sidebar-resize]`; it edits `sidebar_layout.rs`, `renderer/lib.rs`, `input.rs`, `terminal_app.rs` **after** that feature, per the partition.

## Decisions

### SD1: Pure wgpu DrawLayers + hand-rolled hit geometry (no GUI crates)
- **Status**: resolved
- **Rationale**: Both the sidebar resize grab-band and the new title bar (and its buttons) are drawn as wgpu `DrawLayer`s (quads/icons) and hit-tested with pure geometry helpers — never a widget toolkit. Enforces the project's `no-gui-crates` rule (`.claude/rules/no-gui-crates.md`). Rejected: egui/iced overlays, native NSToolbar.

### SD2: renderer + app only — term-core stays GPU/window-free
- **Status**: resolved
- **Rationale**: Neither feature touches `term-core`; the grid/PTY/snapshot engine is unaffected. Preserves the enforced dependency direction (`common ← term-core ← renderer ← app`, `.claude/rules/dependency-direction.md`). Rejected: threading sidebar/titlebar UI state through `GridSnapshot`.

### SD3: One geometry source of truth (draw == hit-test)
- **Status**: resolved
- **Rationale**: Each new interactive region (the resize grab-band; the title-bar buttons) is a single pure rect / `hit_test` function consumed by BOTH the draw layer and the mouse hit-test, mirroring the existing `renderer::sidebar_layout` pattern. Rejected: duplicating geometry between the draw layer and the input path (drift risk).

### SD4: Reuse existing width bounds + inset setters (no new mechanisms)
- **Status**: resolved
- **Rationale**: The Warp-style minimum width is the existing `SIDEBAR_WIDTH_MIN` clamp inside `renderer::Renderer::set_sidebar_width`; the top band uses the existing (currently unwired) `set_top_inset`. Features WIRE these existing setters — they do not add parallel setters or redefine bounds. Rejected: a new min-width config field, a second inset mechanism.

### D5: A full-width top band reserved via the existing `set_top_inset`
- **Status**: resolved
- **Rationale**: The title bar occupies `y ∈ [0, top_inset]` across the full window width; the band height is set by calling the existing `set_top_inset(titlebar_h)` (SD4) at startup and on resize/scale change. Because `grid_dims` and the shaders/layers already honor `top_inset`, a non-zero value shifts the terminal grid down with no per-shader edit. Rejected: a second/parallel top-inset field; rejected: an in-grid-only bar spanning `x >= left_inset` only (not "topmost / full-width" per the goal, and would misalign with the sidebar top).

### D6: EVERY top-band-relative y shifts by `top_inset` — from one source
- **Status**: resolved
- **Rationale**: So the bar is genuinely topmost, EVERYTHING that renders/hit-tests from the window top begins at `top_inset`. `SidebarLayoutInput` gains a `top_inset` field (fed from `renderer.top_inset()`); it is added to the y of the true position sources, not just `header_rect`. Concretely, ALL of these consumers must add `top_inset` (each verified as the real y-origin, not `header_rect.y`): (1) the sidebar list y — `header_h(scale)` as it flows through `list_viewport().y`, the published `list_top` (`SidebarGeom.list_top` at `terminal_app.rs:545`), and `publish_scroll`'s `set_list_scroll(header_h, …)` (`:571`) that the `sidebar_row` draw layer reads (`row_top = list_top + i·stride`) — so DRAWN rows and HIT-TEST rows shift together (SD3); (2) `header_rect` and `restore_button_rect`; (3) the sibling `sidebar-resize` feature's full-height `resize_handle_rect` — its band must become `y ∈ [top_inset, viewport_h]` (not `[0, viewport_h]`), else the resize rule paints up through the title band (violating R6.1) and a press on the band's right edge falls through to `ResizeHandle`. Because `top_inset` lives on `SidebarLayoutInput`, all consumers read ONE value (SD3). Rejected: offsetting only `header_rect.y` (rows derive from `header_h`/`list_top`, not `header_rect`, so they would desync by exactly `top_inset`); rejected: a fixed magic offset (must equal the live `top_inset`); rejected: leaving the resize band full-height (title-band collision above).

### D7: Title-bar geometry is a pure `titlebar` hit/rect module
- **Status**: resolved
- **Rationale**: `titlebar_button_rect(kind, viewport, top_inset, scale)` + `titlebar_hit(p, viewport, top_inset, scale) -> Option<TitlebarButton>` are pure functions consumed by BOTH the draw layer and the app hit-test (SD3), mirroring `sidebar_layout`. `TitlebarButton { NewTab, CloseTab }`. Buttons are right-aligned in the band (or a fixed corner), sized from the band height. Rejected: geometry hard-coded in the layer and duplicated in the app (drift).

### D8: A dedicated `renderer::titlebar` DrawLayer, pinned topmost
- **Status**: resolved
- **Rationale**: A new `TitlebarLayer` (+ `TitlebarHandle` for hover/active state, mirroring `SidebarChromeLayer`) draws the band background and the `+`/`×` glyphs, reusing `sidebar_icons::Icon::Plus` and a new close (`×`) icon; registered via `add_layer`. Its z-order is pinned **topmost among content layers** — above the grid, the `sidebar-resize` rule, and the cursor/selection/chrome layers (highest z / registered last) — so its opaque band paints over anything that might bleed into `y ∈ [0, top_inset]` and its buttons draw over the band background. It is a separate file so the title-bar surface stays disjoint from the resize feature. Rejected: a vague "above the grid" z (ambiguous vs. the resize rule that also draws near the top); rejected: extending `sidebar_header.rs` to draw a top bar (wrong axis, co-edits chrome this feature is stripping).

### D9: Buttons route through the existing `do_tab_action`
- **Status**: resolved
- **Rationale**: `+` → `do_tab_action(TabAction::New)`, `×` → `do_tab_action(TabAction::CloseActive)`. Reusing the exact chord/sidebar path means the title bar inherits the settled semantics — including exit-on-last-tab and composition-cancel — with no new tab logic (`tabs.rs`/`session.rs` untouched). Rejected: calling `tabs.open`/`close_active` directly (bypasses `do_tab_action`'s cancel-composition + hover/scroll re-fit side effects).

### D10: `×` closes the active tab, exiting on the last (mirrors Cmd+W)
- **Status**: resolved
- **Rationale**: `TabAction::CloseActive` returns `false` on the last tab, which the caller turns into `event_loop.exit()` — the title-bar `×` reuses that return so its behavior is byte-identical to Cmd+W (no divergent "can't close last tab" rule). Rejected: disabling `×` on the last tab (inconsistent with Cmd+W, extra state).

### D11: Relocate the new-tab control AND the collapse/expand toggle out of the sidebar
- **Status**: resolved
- **Rationale**: The sidebar's `ChromeRegion::NewTab` (drawn `Icon::Plus`), `HideSidebar` (collapse), AND the collapsed `Restore` button are all removed from `sidebar_header::build_shapes` / `sidebar_layout` — the title bar now hosts `+`, `×`, and the collapse/expand toggle. Because the title bar is always on screen, one toggle serves BOTH collapse (when expanded) and restore (when collapsed), so the separate `Restore` affordance is retired. Rejected: keeping Hide/Restore in the sidebar (the goal now moves them up, Warp-style); rejected: leaving `Restore` as a floating button (redundant with the always-visible title-bar toggle).

### D12: In-content strip, not a native title bar (v1)
- **Status**: assumed
- **Rationale**: The band is an in-content strip below the OS window title bar; the native macOS title bar and its window-drag stay as-is (winit default). A custom/borderless title bar with window-drag needs objc2 work and is deferred (non-goal). Rejected: hiding the native title bar now (larger, platform-fragile change).

### D13: The app consumes EVERY press in the title-bar band
- **Status**: resolved
- **Rationale**: On a `MouseInput` Pressed, the app checks `cursor.y < top_inset` FIRST — before the sidebar hit-test, the resize handle, and the `over_grid → begin_selection` path. A band press routes to `titlebar_hit`: a button → its `do_tab_action`; empty band → swallowed (no-op). Either way the press is CONSUMED and never falls through. Without this early-return, an empty-band press over the grid columns (`x ≥ left_inset`) would satisfy `over_grid` and start a text selection, and one over the resize x would start a resize — both wrong. This makes the band's ownership total, not just button-local. Rejected: consuming only button hits (empty-band fall-through starts selection/resize).

### D14: The title-bar collapse toggle reflects state and flips it
- **Status**: resolved
- **Rationale**: `TitlebarButton` gains a `ToggleSidebar` variant. Pressing it runs `toggle_sidebar(!collapsed)` (reading `renderer.left_inset() <= 0` for the current state) — reusing the existing collapse mechanism (`set_sidebar_collapsed`), so no new collapse logic is added; only the button's *location* moves. The drawn glyph reflects state: a "collapse" affordance when expanded, an "expand" affordance when collapsed (one button, two labels), which is why the separate `Restore` button is retired (D11). The toggle is a title-bar button like `+`/`×`, hit-tested by `titlebar_hit` and consumed by D13. Rejected: two separate title-bar buttons (collapse + restore) — only one is ever meaningful given the state; rejected: re-deriving collapse state in the app (the renderer's inset is the single source, matching `collapsed()`).

## Constraints
- Reuse the existing `set_top_inset` (SD4); do not add a parallel top-inset mechanism.
- `term-core` must not be touched (SD2); no GUI crates (SD1).
- The title-bar `×` must exit the app on the last tab, exactly like Cmd+W (reuse `do_tab_action(CloseActive)`).
- The sidebar and terminal grid must begin at `y = top_inset` so the bar is topmost and nothing overlaps. ALL top-relative y-sources shift by `top_inset` from one value (D6): `header_h`/`list_viewport`/`list_top`/`set_list_scroll` (drawn rows AND hit-test rows), `header_rect`, `restore_button_rect`, and the sibling `resize_handle_rect`.
- Every press with `cursor.y < top_inset` is consumed by the title bar before any sidebar/resize/selection handling (D13) — no fall-through.
- The collapse/expand behavior itself is unchanged (`set_sidebar_collapsed`) — only the toggle's location moves to the title bar (D14). Must not regress sidebar scroll, tab-click switch, or the sibling resize handle (shared files are edited after `sidebar-resize`, per the ordered dependency).

## Known Gaps
- Whether the strip should later replace/integrate the native macOS title bar and become a window-drag region — deferred (D12).
- Exact title-bar height, button placement/spacing, and the `×` icon glyph — settled in implementation.

## Requirements

### R0: Tab + sidebar controls live in a top title bar
#### R0.1: The title bar hosts new-tab, close-tab, and the collapse toggle
- **Given**: The app is running with one or more tabs
- **When**: The window is drawn
- **Then**: A full-width band at the top (`y ∈ [0, top_inset]`) shows a `+` (new-tab), a `×` (close-active), and a collapse/expand toggle; and none of these remain in the sidebar header

### R1: Top band reservation + layout shift
#### R1.1: The title-bar band is reserved
- **Given**: The app starts (or the window resizes / scale changes)
- **When**: Startup/refit runs
- **Then**: `renderer.set_top_inset(titlebar_h)` is called so `top_inset > 0` and `grid_size()` reflects the reserved band
#### R1.2: The sidebar begins below the title bar
- **Given**: `top_inset > 0`
- **When**: The sidebar is laid out / hit-tested
- **Then**: Its header band and rows are offset by `top_inset` (nothing is drawn under the bar); the collapsed-restore affordance no longer exists (the title-bar toggle replaces it, D11)

### R2: Title-bar geometry (hit-test)
#### R2.1: A point on the `+` button hits NewTab
- **Given**: The title bar is drawn with a `+` button rect
- **When**: `titlebar_hit(p, ...)` is called with a point inside the `+` rect
- **Then**: It returns `Some(TitlebarButton::NewTab)`
#### R2.2: A point on the `×` button hits CloseTab
- **Given**: The title bar is drawn with a `×` button rect
- **When**: `titlebar_hit(p, ...)` is called with a point inside the `×` rect
- **Then**: It returns `Some(TitlebarButton::CloseTab)`
#### R2.3: A point on the collapse toggle hits ToggleSidebar
- **Given**: The title bar is drawn with a collapse-toggle rect
- **When**: `titlebar_hit(p, ...)` is called with a point inside the toggle rect
- **Then**: It returns `Some(TitlebarButton::ToggleSidebar)`
#### R2.4: A point on the empty band hits no button but is still consumed
- **Given**: The title bar
- **When**: The user presses in the band (`y < top_inset`) but not on a button
- **Then**: `titlebar_hit` returns `None` AND the app consumes the press (D13) — it does not start a selection, resize, or sidebar action

### R3: New-tab button
#### R3.1: Clicking `+` opens a tab
- **Given**: N tabs are open
- **When**: The user presses inside the `+` button
- **Then**: `do_tab_action(TabAction::New)` runs and there are N+1 tabs with the new one active

### R4: Close-tab button
#### R4.1: Clicking `×` closes the active tab
- **Given**: N ≥ 2 tabs are open
- **When**: The user presses inside the `×` button
- **Then**: `do_tab_action(TabAction::CloseActive)` runs and there are N−1 tabs
#### R4.2: Clicking `×` on the last tab exits
- **Given**: Exactly 1 tab is open
- **When**: The user presses inside the `×` button
- **Then**: `do_tab_action(TabAction::CloseActive)` returns false and the event loop exits (identical to Cmd+W)

### R5: Remove the relocated controls from the sidebar
#### R5.1: The sidebar no longer offers new-tab, hide, or restore
- **Given**: The title bar now hosts `+`, `×`, and the collapse toggle
- **When**: The sidebar header is drawn and hit-tested
- **Then**: No `+`/hide icons are drawn, no floating restore button exists, and `hit_test` never returns `ChromeRegion::NewTab`/`HideSidebar`/`Restore` (those regions are removed)

### R7: The title-bar toggle folds and unfolds the sidebar
#### R7.1: Clicking the toggle while expanded collapses the sidebar
- **Given**: The sidebar is expanded (`left_inset > 0`)
- **When**: The user presses the title-bar collapse toggle
- **Then**: `toggle_sidebar(true)` runs, the sidebar collapses, and the grid reclaims the width
#### R7.2: Clicking the toggle while collapsed restores the sidebar
- **Given**: The sidebar is collapsed (`left_inset <= 0`)
- **When**: The user presses the title-bar toggle
- **Then**: `toggle_sidebar(false)` runs and the sidebar reappears at its prior width
#### R7.3: The toggle glyph reflects the current state
- **Given**: The sidebar is expanded vs. collapsed
- **When**: The title bar is drawn
- **Then**: The toggle shows a "collapse" affordance when expanded and an "expand" affordance when collapsed (one button, two labels)

### R6: No overlap / topmost
#### R6.1: The title bar is topmost and unobstructed
- **Given**: `top_inset > 0`, an expanded sidebar, terminal output, and the sibling resize rule
- **When**: A frame is drawn
- **Then**: The title bar occupies the top band alone; the sidebar rows, the resize rule, and the grid all render entirely below `y = top_inset` with no overlap (the titlebar layer is pinned topmost, D8)
#### R6.2: Drawn rows and hit-test rows stay aligned under the shift
- **Given**: `top_inset > 0` and the sidebar list scrolled to some offset
- **When**: A card is drawn at row i and the same point is hit-tested
- **Then**: Both use the `top_inset`-shifted origin (D6), so a click lands on the card under the cursor (no `top_inset` desync)

## Tasks

### T1: Reserve the title-bar band + shift EVERY top-relative y down [infra]
- **Fulfills**: R1, R6
- **Depends on**: (none)
- `crates/app/src/terminal_app.rs`: call `renderer.set_top_inset(titlebar_h)` at startup and in the resize/refit path; add `top_inset` into `sidebar_input()`, and offset the published `SidebarGeom.list_top` (`:545`) + `publish_scroll`'s `set_list_scroll(header_h, …)` (`:571`) by `top_inset` so DRAWN rows shift. `crates/renderer/src/sidebar_layout.rs`: add a `top_inset` field to `SidebarLayoutInput`; add it to the y of `header_rect`, `list_viewport`/`header_h`-derived row band, `restore_button_rect`, AND the sibling `resize_handle_rect` (band → `y ∈ [top_inset, viewport_h]`). `crates/renderer/src/lib.rs`: expose the title-bar height and ensure `top_inset()` feeds `sidebar_input()`. Unit-test: with `top_inset = k`, a card's drawn-row y == its hit-test-row y (no desync), and `resize_handle_rect` starts at `k`.

### T2: Title-bar draw layer + geometry [vertical]
- **Fulfills**: R0, R2
- **Depends on**: T1
- New `crates/renderer/src/titlebar.rs` (`TitlebarLayer` + `TitlebarHandle`, `TitlebarButton { NewTab, CloseTab, ToggleSidebar }`, pure `titlebar_button_rect` + `titlebar_hit`); draws the band + `+`/`×`/toggle (reusing `sidebar_icons::Icon::{Plus, Panel}`, adding a `×` glyph; the toggle glyph reflects collapsed state via the handle, R7.3). `pub mod titlebar;` in `crates/renderer/src/lib.rs`. Unit-test `titlebar_hit` for `+`/`×`/toggle/empty.

### T3: App wiring — consume band + route to tab/sidebar actions [vertical]
- **Fulfills**: R2 (R2.4 consume), R3, R4, R6.1, R7
- **Depends on**: T2
- New `crates/app/src/titlebar.rs` (hit helper / handle glue). `crates/app/src/input.rs`: map a title-bar hit to an action. `crates/app/src/terminal_app.rs`: register the titlebar layer topmost; in `MouseInput` Pressed, FIRST check `cursor.y < top_inset` (D13) — route through `titlebar_hit`: `NewTab`/`CloseTab` → `do_tab_action(TabAction::New | CloseActive)` (exit on last-tab false); `ToggleSidebar` → `toggle_sidebar(!collapsed())` (D14) and push the new state to the titlebar handle so the glyph updates; empty band → consume and return; only below the band fall through to the existing sidebar/resize/selection paths.

### T4: Remove the relocated controls from the sidebar [cleanup]
- **Fulfills**: R5
- **Depends on**: T3
- `crates/renderer/src/sidebar_layout.rs`: remove `ChromeRegion::NewTab`/`HideSidebar`/`Restore` and their `button_rect`/`restore_button_rect`/`chrome_hit`/`hit_test` handling (the whole `ChromeRegion` enum may retire). `crates/renderer/src/sidebar_header.rs`: remove the `Icon::Plus`/`Icon::Panel` header draws and the collapsed restore button in `build_shapes`. `crates/app/src/input.rs`: drop the `SidebarAction::{Collapse, Expand}` mappings from `sidebar_press`/`sidebar_hover` (now driven by the title bar). Done after T3 so the title bar provides the controls before the sidebar loses them.

## External Dependencies
### Pre-work
- (none)
### Post-work
- (none)

## Declared Surface
- feature-id: titlebar-tab-controls
- depends_on: [sidebar-resize]
- declared-surface:
  - crates/renderer/src/titlebar.rs
  - crates/app/src/titlebar.rs
  - crates/renderer/src/sidebar_header.rs
  - crates/renderer/src/sidebar_layout.rs
  - crates/renderer/src/lib.rs
  - crates/app/src/input.rs
  - crates/app/src/terminal_app.rs
