# Spec: sidebar-resize

## Meta
- **Created**: 2026-07-21
- **Type**: dev
- **Status**: drafting (T1–T3 implemented; T4 resize-cursor pending)
- **Batch**: sidebar-titlebar (partition-manifest.md)
- **Amended 2026-07-21**: add a horizontal-resize cursor on hover over the grab-band — D8a/D11, R7, T4 (user-approved via /specify).

## Goal
Make the left tab sidebar resizable by dragging its right edge with the mouse, with a minimum width (Warp-style).

## Non-goals
- Persisting the resized width to `config.toml` across app restarts (config is read-only + file-watched; a new writer is out of scope — the runtime override survives config reloads but not a restart).
- Resizing by keyboard or by a numeric setting.
- Changing the existing maximum width bound or the collapse/expand toggle behavior.
- A hover *highlight/tint* on the grab-band — optional polish needing a separate colored hover feed to the draw layer; not required for v1 (D8a). (The horizontal-resize *cursor* on hover IS in scope — see D11.)

## Confirmed Goal
A user can drag a thin grab-band on the **grid side** of the sidebar's right edge (the band occupies `[left_inset, left_inset + grab]`, so it never overlaps the sidebar body/chrome) to resize the sidebar width **live**; the width is clamped to `[SIDEBAR_WIDTH_MIN, SIDEBAR_WIDTH_MAX]` (the existing Warp-style minimum is honored — the sidebar never shrinks below `SIDEBAR_WIDTH_MIN`); the tab list and terminal grid re-fit to the new width on each drag frame; releasing the button ends the drag. When the sidebar is collapsed there is no resize handle (draw and hit-test both derive this from one function).

## Research
- `renderer::Renderer::set_sidebar_width(logical_px)` already clamps to `SIDEBAR_WIDTH_MIN..=MAX` and writes the runtime `SidebarOverride.width_px`, then `sync_left_inset()`; it is the real runtime width setter but is called only from tests today — this feature's hook (`crates/renderer/src/lib.rs:585`).
- `SIDEBAR_WIDTH_MIN = 260.0`, `SIDEBAR_WIDTH_MAX = 440.0` consts; `SidebarConfig.width_px` is clamped on load by `normalize()` (`crates/common/src/config.rs:78,81,169`).
- `left_inset()` getter (physical px) and the single writer `sync_left_inset()` = `sidebar_row::sidebar_width_px(effective_sidebar, scale)` (`crates/renderer/src/lib.rs:594,569`).
- `sidebar_layout::hit_test(p, s, inp, collapsed)` is the ONE ordered hit-test; its step-2 branch `p[0] >= inp.sidebar_w → None` defines the sidebar's right edge — the natural place to intercept a resize grab-band, before that branch (`crates/renderer/src/sidebar_layout.rs:316,329`). `SidebarLayoutInput.sidebar_w` is the band width (`:86`).
- App mouse arms exist: `MouseInput` Pressed (`crates/app/src/terminal_app.rs:870`), Released (`:904`), `CursorMoved` (`:860`); `sidebar_input()` reads `left_inset` live (`:532`); `collapsed()` = `left_inset <= 0.0` (`:561`).
- `crate::settings::refit(&renderer, &mut tabs)` re-fits all tabs (incl. background) to the current inset (`crates/app/src/settings.rs`), already called from the `Resized` arm.
- Input seam: `sidebar_press` maps a press to a `SidebarAction`; `SidebarAction` enum + `over_grid` (`crates/app/src/input.rs:82,103,299`).
- Config live reload (`settings::apply`) does NOT clear the runtime override (D4), so a runtime-resized width survives a reload (`crates/app/src/settings.rs`).
- DrawLayer template: `cursor_layer.rs` / `selection_layer.rs` (alpha quad, shared handle, re-recorded each frame); geometry-as-pure-fn pattern in `sidebar_layout.rs` (draw == hit-test).

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

### D5: Resize grab-band geometry lives in `sidebar_layout` (collapse-aware, grid-side)
- **Status**: resolved
- **Rationale**: A new pure `resize_handle_rect(inp) -> Option<Rect>` returns the band, or `None` when collapsed (`left_inset <= 0`) — so draw AND hit-test derive "no handle" from ONE function (SD3, resolves the collapsed-draw gap). When present, the band is a thin (~6px physical, scale-aware), full-height rect on the **grid side** of the edge: `x ∈ [sidebar_w, sidebar_w + grab]`. Because it lives at `x >= sidebar_w`, `hit_test` can check it BEFORE the `>= sidebar_w → None` grid branch **without ever reaching or overlapping** the header/card checks (which only run for `x < sidebar_w`), returning `SidebarRegion::ResizeHandle`. Rejected: a band *straddling* `x = sidebar_w` (its inner half would steal the rightmost pixels of cards / the HideSidebar button); rejected: hard-coding the band in the app (drift from the drawn rule); rejected: a wide band that would swallow first-column terminal clicks (kept thin, and the ≤grab-px intrusion into the grid's left margin is accepted).

### D6: App-side drag state machine on `Running` (mirrors the selection drag)
- **Status**: resolved
- **Rationale**: A press resolving to `ResizeHandle` sets `resizing = true`; `CursorMoved` while `resizing` sets the width from the cursor x; release clears it. This mirrors the just-shipped text-selection drag state machine (press→move→release on `Running`), so the pattern and its edge-handling (clear on release) are already proven. Rejected: a renderer-owned drag (renderer is window/event-free, SD2); rejected: a modal drag loop.

### D7: Width comes from the cursor x via the existing clamped setter
- **Status**: resolved
- **Rationale**: While resizing, width = cursor physical-x converted to logical px (÷ `scale_factor`), passed to the existing `set_sidebar_width(logical_px)`, which already clamps to `[SIDEBAR_WIDTH_MIN, SIDEBAR_WIDTH_MAX]`. The Warp-style minimum is therefore enforced by reuse, not reinvented (SD4). Steelman (absolute cursor-x vs. grab-offset delta): using the absolute cursor-x means the edge "jumps" to the cursor on press instead of tracking a grab offset — but because the grab-band is thin (D5, ~6px), the press-x is always within `grab` px of `sidebar_w`, so the initial jump is bounded by ≤ `grab` px and is visually negligible. This bound couples D7 to D5's thin band: widening the band would reintroduce a visible snap, so the band width is a shared invariant, not a free parameter. Rejected: computing/clamping the width in the app (duplicates the renderer's bound); rejected: grab-offset delta tracking (extra state for a sub-pixel-perceptible gain given the thin band).

### D8: The handle is its own thin DrawLayer (`renderer::sidebar_resize`)
- **Status**: resolved
- **Rationale**: A dedicated `DrawLayer` draws a subtle 1–2px vertical rule at `x = sidebar_w`, derived from the SAME `resize_handle_rect(inp) -> Option<Rect>` (D5) — so it draws nothing when that returns `None` (collapsed), keeping draw and hit-test single-sourced (SD3, R6.1). It carries a `SidebarResizeHandle` (shared handle, like `selection_layer`) that the app sets to `true` while dragging, so the rule may brighten *during a drag* — no separate hover-cursor feed is wired (hover tint is a v1 non-goal, D8a). Registered alongside the other layers so `sidebar_header.rs` stays untouched (surface disjoint from the titlebar feature). Rejected: drawing inside `sidebar_header::build_shapes` (co-edits a titlebar-owned file); rejected: a raw-cursor hover feed for a hover tint (orphaned plumbing for v1 — deferred).

### D8a: No hover *tint* in v1 (the cursor IS in scope, D11)
- **Status**: assumed
- **Rationale**: A hover *color* highlight would need a separate colored hover feed into the draw layer. For v1 the rule is static plus a *drag-active* brighten driven by the existing drag handle (no new draw-side input path). The hover *tint* stays deferred, but the horizontal-resize *cursor* on hover is now in scope (D11) — it is an app-side `window.set_cursor` call, not a draw-layer feed, so it adds no renderer plumbing. Rejected: wiring a colored hover feed to the layer now (cosmetic-only).

### D11: Horizontal-resize cursor while hovering the grab-band
- **Status**: resolved
- **Rationale**: On `CursorMoved` the app hit-tests the grab-band — reusing the ONE geometry source (`resize_handle_rect`/`hit_test → SidebarRegion::ResizeHandle`, so hover and press agree, SD3) — and sets the window cursor to a horizontal-resize icon (`winit::window::CursorIcon::EwResize`) when over it, reverting to `CursorIcon::Default` when off it. To avoid a `set_cursor` call every frame, the app tracks a `resize_hover: bool` and calls `window.set_cursor` ONLY on a state change. While an active drag is in flight the resize cursor stays set (the pointer may leave the thin band mid-drag). This is a winit call (allowed — not a GUI crate, SD1) confined to the `app` crate (SD2). Rejected: an objc2 `NSCursor` push (winit already exposes the icon); rejected: calling `set_cursor` every `CursorMoved` (needless churn); rejected: a renderer-side cursor (renderer is window-free, SD2).

### D9: No config-file persistence in v1 (runtime override only)
- **Status**: assumed
- **Rationale**: `set_sidebar_width` writes the session `SidebarOverride`, which survives config reloads (reload does not clear it, D4) but not an app restart. Persisting a dragged width to `config.toml` needs a new config writer that does not exist today (config is read-only + file-watched) — deferred as a non-goal to keep v1 scoped. Rejected: adding a config writer now (new surface, race with the file-watcher).

### D10: The resize handle exists only when the sidebar is expanded (single-sourced)
- **Status**: resolved
- **Rationale**: The collapsed guard lives in ONE place — `resize_handle_rect(inp)` returns `None` when `left_inset <= 0` (D5). Both consumers honor it from that single source (SD3): `hit_test` never returns `ResizeHandle`, and the draw layer (D8) draws nothing. So there is nothing to drag and nothing to draw until the sidebar is restored. Rejected: separate collapsed checks in the hit-test and the layer (two sources → drift, the exact gap this closes); rejected: a resize-from-collapsed affordance (ambiguous with the restore button).

## Constraints
- Never shrink below `SIDEBAR_WIDTH_MIN` or grow past `SIDEBAR_WIDTH_MAX` (clamp via `set_sidebar_width`).
- Must re-fit ALL tabs (including background tabs) to the new inset each width change — the grid cols/rows depend on `left_inset` (reuse `settings::refit`).
- `term-core` must not be touched (SD2); no GUI crates (SD1).
- Must not regress the existing collapse/expand toggle, sidebar scroll, or tab-click switch.
- The grab-band lives on the grid side (`[sidebar_w, sidebar_w + grab]`), so it never swallows presses meant for the sidebar body or chrome; keep it thin (~6px) so it intrudes at most `grab` px into the terminal's left margin.

## Known Gaps
- Persisting the resized width across app restart (needs a `config.toml` writer) — deferred (D9).
- Exact grab-band width — settled in implementation. (The resize cursor on hover is now decided — D11/R7.)

## Requirements

### R0: The sidebar is resizable by mouse drag with a minimum width
#### R0.1: Drag the right edge to resize live
- **Given**: The sidebar is expanded at some width W within `[MIN, MAX]`
- **When**: The user presses on the right-edge grab-band and drags horizontally
- **Then**: The sidebar width follows the cursor live (clamped to `[MIN, MAX]`) and the tab list + terminal grid re-fit each frame

### R1: Resize-handle hit region (geometry)
#### R1.1: A press on the grid-side grab-band is the ResizeHandle region
- **Given**: An expanded sidebar of band width `sidebar_w`
- **When**: `hit_test` is called with a point whose x is in `[sidebar_w, sidebar_w + grab]`
- **Then**: It returns `SidebarRegion::ResizeHandle` (checked before the `>= sidebar_w → None` grid branch, and — being on the grid side — never reaching the header/card checks)
#### R1.2: A press away from the edge is not a resize
- **Given**: An expanded sidebar
- **When**: `hit_test` is called with a point inside the sidebar body/chrome (`x < sidebar_w`) or well into the grid (`x > sidebar_w + grab`)
- **Then**: It returns the existing region (Card/Panel/Chrome or None) — never `ResizeHandle`

### R2: Drag state machine (app)
#### R2.1: Press on the handle begins a resize
- **Given**: The pointer is over the resize grab-band
- **When**: The left mouse button is pressed
- **Then**: `resizing` becomes true and no tab-switch / selection is started
#### R2.2: Moving while resizing updates the width
- **Given**: `resizing` is true
- **When**: The cursor moves to physical x = Px
- **Then**: `renderer.set_sidebar_width(Px / scale_factor)` is called and the tabs are re-fit, then a redraw is requested
#### R2.3: Release ends the resize
- **Given**: `resizing` is true
- **When**: The left mouse button is released
- **Then**: `resizing` becomes false and further cursor moves no longer resize

### R3: Minimum / maximum clamp (Warp-style min)
#### R3.1: Dragging past the minimum clamps at MIN
- **Given**: `resizing` is true
- **When**: The cursor moves left of `SIDEBAR_WIDTH_MIN`
- **Then**: The width stays at exactly `SIDEBAR_WIDTH_MIN` (never smaller)
#### R3.2: Dragging past the maximum clamps at MAX
- **Given**: `resizing` is true
- **When**: The cursor moves right of `SIDEBAR_WIDTH_MAX`
- **Then**: The width stays at exactly `SIDEBAR_WIDTH_MAX`

### R4: Re-fit on width change
#### R4.1: Tabs and grid re-fit to the new inset
- **Given**: The width changed to a new value
- **When**: The width setter has run
- **Then**: `settings::refit` re-fits the active and background tabs so `grid_size()` reflects the new `left_inset`

### R5: Handle rendering
#### R5.1: A visible affordance marks the right edge
- **Given**: The sidebar is expanded (`resize_handle_rect` returns `Some`)
- **When**: A frame is drawn
- **Then**: The `sidebar_resize` DrawLayer draws a subtle 1–2px vertical rule at `x = sidebar_w`, brightened while a drag is active (no hover tint in v1, D8a)

### R6: Collapsed guard (single-sourced)
#### R6.1: No handle when collapsed — one source
- **Given**: The sidebar is collapsed (`left_inset <= 0`), so `resize_handle_rect` returns `None`
- **When**: `hit_test` runs and a frame is drawn
- **Then**: `hit_test` returns no `ResizeHandle` AND the resize layer draws nothing — both derived from the single `None` return

### R7: Horizontal-resize cursor on hover
#### R7.1: Hovering the grab-band shows the resize cursor
- **Given**: The sidebar is expanded and the pointer is not dragging
- **When**: The cursor moves over the grab-band (`hit_test → ResizeHandle`)
- **Then**: The window cursor becomes `CursorIcon::EwResize` (↔)
#### R7.2: Leaving the grab-band reverts the cursor
- **Given**: The cursor was showing `EwResize` over the band
- **When**: The cursor moves off the band (and no drag is active)
- **Then**: The window cursor reverts to `CursorIcon::Default`
#### R7.3: The cursor call fires only on a hover-state change
- **Given**: The pointer moves within the band across several `CursorMoved` events
- **When**: The hover state (`resize_hover`) does not change
- **Then**: `window.set_cursor` is NOT called again (only called on a transition)

## Tasks

### T1: Resize-handle geometry + hit-test region [infra]
- **Fulfills**: R1, R6
- **Depends on**: (none)
- Add `resize_handle_rect(&SidebarLayoutInput) -> Option<Rect>` (returns `None` when collapsed / `left_inset <= 0`; else the grid-side band `[sidebar_w, sidebar_w + grab]`) and `SidebarRegion::ResizeHandle` in `crates/renderer/src/sidebar_layout.rs`; intercept it in `hit_test` before the `>= sidebar_w → None` branch. Unit-test: a point in the band → `ResizeHandle`; `x < sidebar_w` → unchanged region; collapsed → `None`.

### T2: Resize-handle draw layer [vertical]
- **Fulfills**: R5
- **Depends on**: T1
- New `crates/renderer/src/sidebar_resize.rs` (`SidebarResizeLayer` + `SidebarResizeHandle` carrying only drag-active state, mirroring `cursor_layer`/`selection_layer`), draws the vertical rule from `resize_handle_rect` (draws nothing when it returns `None`); add `pub mod sidebar_resize;` in `crates/renderer/src/lib.rs`.

### T3: App drag wiring + width setter + re-fit [vertical]
- **Fulfills**: R0, R2, R3, R4
- **Depends on**: T2
- `crates/app/src/input.rs`: `sidebar_press` maps `ResizeHandle → SidebarAction::BeginResize`. `crates/app/src/sidebar_resize.rs` (new): the drag state helper. `crates/app/src/terminal_app.rs`: add `resizing` field to `Running`, register the resize layer, handle `BeginResize` on press, resize on `CursorMoved`, end on release, call `renderer.set_sidebar_width` + `settings::refit` + `request_redraw`.

### T4: Resize cursor on hover [vertical]
- **Fulfills**: R7
- **Depends on**: T3
- `crates/app/src/terminal_app.rs`: add a `resize_hover: bool` field to `Running`; in `CursorMoved`, hit-test the grab-band (reuse `sidebar_input` + `hit_test → ResizeHandle`, or `resize_handle_rect().contains`), OR `self.resizing`, into a desired hover state; when it flips, call `self.window.set_cursor(CursorIcon::EwResize | Default)` (winit) — once per transition (R7.3). Keep `EwResize` while `resizing`.

## External Dependencies
### Pre-work
- (none)
### Post-work
- (none)

## Declared Surface
- feature-id: sidebar-resize
- depends_on: []
- declared-surface:
  - crates/renderer/src/sidebar_resize.rs
  - crates/app/src/sidebar_resize.rs
  - crates/renderer/src/sidebar_layout.rs
  - crates/renderer/src/lib.rs
  - crates/app/src/input.rs
  - crates/app/src/terminal_app.rs
