# partition-manifest.md — sidebar-titlebar
- generated_at: 2026-07-21
- goal: Make the left tab sidebar resizable by dragging with the mouse (with a minimum width, Warp-style), and move the tab open/close controls to a top title bar.

## Shared Decisions
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

## Features  (ordered — a dependency appears before its dependents)
### sidebar-resize
- declared-surface:
  - crates/renderer/src/sidebar_resize.rs
  - crates/app/src/sidebar_resize.rs
  - crates/renderer/src/sidebar_layout.rs
  - crates/renderer/src/lib.rs
  - crates/app/src/input.rs
  - crates/app/src/terminal_app.rs
- depends_on: []
- accept-seed: Dragging the sidebar's right edge resizes it live, clamped to a minimum width (Warp-style), and the tab list re-fits to the new width.
- pre-approved-batch: yes

### titlebar-tab-controls
- declared-surface:
  - crates/renderer/src/titlebar.rs
  - crates/app/src/titlebar.rs
  - crates/renderer/src/sidebar_header.rs
  - crates/renderer/src/sidebar_layout.rs
  - crates/renderer/src/lib.rs
  - crates/app/src/input.rs
  - crates/app/src/terminal_app.rs
- depends_on: [sidebar-resize]
- accept-seed: A top title bar hosts the tab open (new) and close controls, relocated out of the sidebar header, and creating/closing tabs works from there.
- pre-approved-batch: yes
