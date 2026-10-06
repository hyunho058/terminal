# Spec: selection-highlight

## Meta
- **Created**: 2026-07-23
- **Type**: dev
- **Status**: planned (batch-approved; not yet executed)
- **Set**: clipboard-copy-paste (see `specs/clipboard-copy-paste/partition-manifest.md`)
- **Generated**: via decompose → specify `mode: batch`
- **Approved by**: decompose partition gate (batch — human owns the bar at the ONE partition gate)
- **Approved at**: 2026-07-23

## Goal
Render the live selection: a hand-rolled per-cell highlight `DrawLayer` in `renderer`
that reads `GridSnapshot.selection` (from `selection-model`) and tints the selected
cell range, registered through the existing `DrawLayer`/`Overlay` seam — no GUI crate,
no edit to the central draw path.

## Non-goals
- No selection *state* or text extraction (owned by `selection-model`).
- No mouse/pixel handling (owned by `selection-input`) — this layer only draws what the
  snapshot already carries.
- No clipboard I/O.
- No block/rectangular highlight; no per-theme selection color tuning (a fixed
  translucent tint is the baseline — see Known Gaps).

## Confirmed Goal
Add `crates/renderer/src/selection_layer.rs` (a `DrawLayer` mirroring `image_layer.rs`)
plus its module wiring in `crates/renderer/src/lib.rs`, drawing a translucent per-cell
tint over every cell in `snapshot.selection`, with the selected-cell→instance mapping
factored as a pure function unit-tested without a GPU.

## Declared Surface
- feature-id: selection-highlight
- depends_on: [selection-model]
- declared-surface:
  - crates/renderer/src/selection_layer.rs
  - crates/renderer/src/lib.rs

## Research
- The `DrawLayer` seam: `record(&mut self, ctx: &mut FrameCtx)` records the layer's own pass on `ctx.encoder` with `Load` (composites over the terminal), ordered by `z_order` — "glyph=0 < tab bar=10 < image=20 < overlay=100" (`crates/renderer/src/lib.rs:142-151`).
- `FrameCtx` hands a layer everything it needs: `device/queue/encoder/view`, `viewport`, `cell` (cell px), `top_inset`, `left_inset`, `format`, `snapshot` (which carries `.selection` after `selection-model`), and `atlas` (`lib.rs:112-140`).
- `image_layer.rs` is the exemplar registered `DrawLayer`: it owns its `Vertex`/`QUAD`, `Uniforms`, `RectInstance`, an inline `WGSL`, its own pipeline, and maps a cell rectangle to clip space as `px = rect.xy + corner*rect.zw; clip = (px/viewport)*2-1` flipping Y (`image_layer.rs:1-60+`). `IMAGE_Z = 20` "above the tab bar (10), below overlays (100)".
- The cell-bg technique to reuse (SD2): `BgInstance { grid_pos, color }` + `BgUniforms { viewport, cell, top_inset, left_inset }` drive `cells.wgsl` to paint a per-cell quad at `px = grid_pos*cell + (left_inset, top_inset)` (`lib.rs:64-93`). The selection layer reuses this grid-space mapping.
- Translucent-overlay precedent: the IME preedit composition draws as a "0.5-alpha" block above the cursor (`crates/app/src/terminal_app.rs:231`, `renderer/src/preedit_layer.rs`) — a Load-after-glyph layer can alpha-blend over text.
- A layer is registered by an app `Overlay` whose `layer()` returns the boxed layer — `ImagePresenter::layer()` returns `Box::new(ImageLayer::new())` (`crates/app/src/clipboard.rs:128-130`), registered in `main.rs`. `selection-highlight` exposes `SelectionLayer` the same way; the app-side registration is `copy-clipboard`'s job.
- Headless render exists for GPU-free-ish tests: `Renderer::new_headless` draws to an offscreen view (`lib.rs:272`); pure instance-building (like `build_glyph_instances`, `docs/architecture.md` G3) is unit-testable with no device.
- `GridSnapshot.selection: Option<Selection>` and `Selection::contains(col, row, cols) -> bool` come from `selection-model` (its D6/D9); the snapshot carries a selection only in the live view.
- Constraints: no-gui-crates (wgpu only), macos-only (Metal), `renderer` stays window-agnostic (raw-window-handle, no winit); `cargo clippy … -D warnings`.

## Decisions

> SD1–SD5 inherited **verbatim** from `specs/clipboard-copy-paste/partition-manifest.md`.
> D6+ are this feature's local decisions.

### SD1: Selection state rides on the versioned GridSnapshot contract; term-core stays GPU-free and window-free
- **Status**: resolved
- **Rationale**: The dependency-direction rule forbids term-core from touching wgpu/winit and forbids the renderer from seeing a live `Grid`. So the selection is a value type (anchor/focus cell coords) owned in `term-core`; the `Grid` holds the live selection; `GridSnapshot` is EXTENDED (its versioned contract) to carry the selection for the renderer; and cell→String extraction (trailing-whitespace trim, soft-wrapped rows joined) lives in `term-core` beside the existing `GridSnapshot::to_text` (which already skips double-width continuation cells). The renderer only ever reads `GridSnapshot`, never a live `Grid`. Rejected: holding anchor/focus in the app and passing raw coords to the renderer (would leak selection semantics + text extraction out of the headless core, defeating headless testability).

### SD2: The highlight is hand-rolled per-cell drawing on the existing wgpu cell-bg path — no GUI/widget crate
- **Status**: resolved
- **Rationale**: The no-gui-crates rule is the project's reason to exist. The highlight is a `renderer` `DrawLayer` that reads `GridSnapshot`'s selection field and paints the selected cell range by reusing the existing per-cell background quad path (`cells.wgsl`/the bg pipeline), registered through the existing `DrawLayer`/`Overlay` seam (the same seam `ImageLayer`/`ImagePresenter` use). Rejected: any egui/iced/overlay-widget approach, and any bespoke selection shader when the cell-bg path already draws per-cell quads.

### SD3: Clipboard access is objc2 NSPasteboard, consistent with clipboard.rs
- **Status**: resolved
- **Rationale**: The macos-only rule + the existing `clipboard.rs` already use `objc2_app_kit::NSPasteboard` on the UI thread. READ text via `stringForType(NSPasteboardTypeString)`; WRITE via `clearContents` + `setString` (or `writeObjects`). No cross-platform clipboard abstraction, no new clipboard crate. Runs on the UI thread like the existing image read. Rejected: the `arboard`/`copypasta` crates (unnecessary dependency; inconsistent with the established objc2 usage).

### SD4: Input goes through the SD4 InputHandler/Overlay seam; the ONLY event-loop edit is mouse press/drag/release + pixel→cell
- **Status**: resolved
- **Rationale**: The SD4 seam (`InputRouter`/`OverlayHost`, `AppCtx`) exists so features extend input/overlays from their own modules without editing `terminal_app.rs`. Cmd+V (paste) and Cmd+C (copy) are `InputHandler`s dispatched through the router; the highlight is an `Overlay`-provided `DrawLayer`. The necessary exception is the pointer pipeline: `WindowEvent::MouseInput` (Left Pressed/Released) + drag-while-`CursorMoved` + pixel→cell mapping MUST touch the `terminal_app.rs` event loop. Existing sidebar press/hover/wheel and image-paste stay unaffected. Rejected: routing Cmd+C/Cmd+V through new bespoke event-loop branches instead of the seam.

### SD5: Bracketed-paste mode (DECSET/DECRST 2004) follows the existing DECCKM/app_cursor_keys convention
- **Status**: resolved
- **Rationale**: term-core already tracks a private mode this exact way: a `bool` flag on `Grid` (`grid.rs`), a `Terminal` accessor (`lib.rs`), set from `parser.rs` `csi_dispatch` on `?<n> h|l`. Bracketed-paste mode uses the same path: a `bracketed_paste` flag on `Grid`, a `Terminal::bracketed_paste()` accessor, set on `?2004 h|l`. The app reads it via that accessor and surfaces it to input handlers. Rejected: tracking mode 2004 in the app layer or on the `Processor` instead of `Grid`.

### D6: `SelectionLayer` is a self-contained `DrawLayer` mirroring `ImageLayer`
- **Status**: resolved
- **Rationale**: A new `selection_layer.rs` defines `pub struct SelectionLayer` with `new()`, its own `Vertex/QUAD`, instance type, inline `WGSL`, pipeline (built lazily on first `record` from `ctx.device`/`ctx.format`, like `image_layer`), and `impl DrawLayer`. `lib.rs` gains only `pub mod selection_layer;` — sibling layers are **not** type-re-exported (only `font_set` is, `lib.rs:38`); the app reaches the type via the module path `renderer::selection_layer::SelectionLayer`, exactly as `clipboard.rs:22` reaches `renderer::image_layer::ImageLayer`. The app registers it via an `Overlay` (its `layer()` returns `Box::new(SelectionLayer::new())`), exactly as `ImagePresenter` registers `ImageLayer` — so no edit to the central `Renderer` draw path. Rejected: injecting selection into the base bg pass (`draw_to`) — that edits the central path the seam exists to avoid, and would need the selection baked into cell `bg` (violating SD1).

### D7: One quad instance per selected cell, from `snapshot.selection` + `Selection::contains`, in grid space
- **Status**: resolved
- **Rationale**: `record` reads `ctx.snapshot.selection`; for each cell `(col,row)` in `0..cols × 0..rows` it emits a highlight instance iff `sel.contains(col,row,cols)`, mapping to pixels with the **cell-bg grid mapping** `px = (col,row)*ctx.cell + (ctx.left_inset, ctx.top_inset)` (SD2 — same as `BgInstance`/`cells.wgsl`). The layer highlights **every** selected coordinate **including double-width continuation cells** (so a selected 한글 tints its full 2-cell width) — deliberately unlike text extraction, which *skips* continuations. Rejected: per-run merged spans (micro-optimization; per-cell instances match the existing bg pipeline and are trivially correct); reading a live `Grid` (forbidden — snapshot only).

### D8: Composite as a translucent tint at `SELECTION_Z = 4` — above glyph, below the cursor
- **Status**: resolved
- **Rationale**: `DrawLayer`s draw *after* the glyph pass (Load), so an **opaque** fill would occlude text. The highlight instead alpha-blends a translucent tint (baseline: a fixed selection color at ~0.3 alpha) so glyphs read through — the same technique as the 0.5-alpha preedit block. The **real** z landscape (the lib.rs:147 "glyph=0 < tab bar=10 < image=20 < overlay=100" comment is stale — it omits cursor/preedit/sidebar-chrome) is: glyph=0, `CURSOR_Z=5` (`cursor_layer.rs:18`), `PREEDIT_Z=6`, `TAB_BAR_Z=10`, `IMAGE_Z=20`, `SIDEBAR_CHROME_Z=30`, `OVERLAY_Z=100`. `SELECTION_Z = 4` sits **above the glyph pass (0) and below the cursor (5)** — so the cursor block and IME preedit draw *over* the selection tint (the cursor stays visible during a drag), and all chrome (tab bar/images/sidebar/overlays) is painted after → never tinted. To lock this against future drift, add a compile-time guard mirroring `sidebar_header.rs:32-34`: `const _: () = assert!(SELECTION_Z > 0 && SELECTION_Z < crate::cursor_layer::CURSOR_Z);`. Rejected: `z=5` (ties `CURSOR_Z` → draw order falls to registration order, unspecified); true reverse-video (needs per-cell fg/bg swap in the *base* pass, not a post-layer); an opaque fill (hides the selected text).

### D9: Empty/absent selection records nothing
- **Status**: resolved
- **Rationale**: When `snapshot.selection` is `None` (no selection, or scrolled into history per `selection-model` D9) or yields zero member cells, `record` early-returns without a render pass — no cost, no stray tint. This also means scrolling away from a selection hides its highlight for free. Rejected: always recording an (empty) pass (wasteful and needless).
- **Note**: `specs/selection-scroll-anchor` reversed the "hidden while scrolled into history" policy this relies on. `snapshot.selection` is now projected at every `view_offset`, so it is `None` only when no part of the selection is on screen. The early return is unchanged.

## Constraints
- No GUI/widget crate; wgpu/`bytemuck`/`wgpu::util` only. `renderer` must not import `winit` (window-agnostic, C1/SD2).
- No `#[cfg(target_os)]`; no `unwrap()`/`expect()` outside tests; `cargo clippy --workspace --all-targets -- -D warnings` clean; `rustfmt` on save.
- Must not edit the central `Renderer::draw_to`/base pass (seam-only, D6) and must not alter the idle-CPU snapshot-equality cache — the highlight reacts to `snapshot.selection` changes, which already make snapshots compare unequal (`selection-model` constraint), so a selection change naturally triggers a redraw + re-record.
- The instance-building mapping must be a pure function (no `wgpu` types in its signature) so it is unit-testable without a device.

## Known Gaps
- The selection tint color is a fixed constant for the baseline; making it theme-/config-driven (like the sidebar theme) is deferred.
- Translucent-over-text (not reverse-video) is the baseline compositing choice; crisper reverse-video selection would require touching the base pass and is deferred.

## Requirements

### R0: A hand-rolled selection highlight layer, no GUI crate, headlessly testable
#### R0.1: No GUI/widget crate or winit import is added
- **Given**: the `renderer` crate after this feature
- **When**: its dependencies/imports are inspected
- **Then**: no GUI/widget crate is present and `selection_layer.rs` imports no `winit`; it uses only `wgpu`/`bytemuck` and `crate::{DrawLayer, FrameCtx}`
#### R0.2: The selected-cell→instance mapping is a pure, GPU-free function
- **Given**: the instance-building logic
- **When**: `cargo test -p renderer` runs headlessly
- **Then**: a pure function maps `(snapshot, cell, left_inset, top_inset)` → `Vec<instance>` and is asserted without constructing a `wgpu::Device`

### R1: `SelectionLayer` is a registered `DrawLayer` mirroring `ImageLayer`
#### R1.1: `SelectionLayer` implements `DrawLayer` at the selection z-order
- **Given**: `SelectionLayer::new()`
- **When**: it is used as a `Box<dyn DrawLayer>`
- **Then**: `z_order()` returns `SELECTION_Z (4)` — above glyph (0), below `CURSOR_Z (5)`; a compile-time assert guards `0 < SELECTION_Z < CURSOR_Z`
#### R1.2: The layer is exposed for app registration
- **Given**: `crates/renderer/src/lib.rs`
- **When**: an app module (`copy-clipboard`) constructs the layer
- **Then**: `SelectionLayer` is reachable via the module path `renderer::selection_layer::SelectionLayer` and constructible via `new()`, mirroring `ImageLayer`
#### R1.3: The pipeline is built lazily and the central draw path is untouched
- **Given**: a first `record` call with a `FrameCtx`
- **When**: the layer draws
- **Then**: it builds its pipeline from `ctx.device`/`ctx.format` on first use (caching it) and records its own pass; `Renderer::draw_to` / the base bg pass are unchanged

### R2: The layer emits one highlight instance per selected cell in grid space
#### R2.1: Each selected cell yields an instance at the cell's pixel rect
- **Given**: a snapshot (`cols=10`, `cell=(8,16)`, `left_inset=40`, `top_inset=24`) with a selection whose `contains` is true for `(3,1)`
- **When**: instances are built
- **Then**: an instance exists at `px = (3*8 + 40, 1*16 + 24) = (64, 40)` with size `(8, 16)`
#### R2.2: A double-width selected cell is highlighted across both its cells
- **Given**: a selection covering a 한글 leader at `(2,0)` and its continuation at `(3,0)`
- **When**: instances are built
- **Then**: instances exist for **both** `(2,0)` and `(3,0)` (continuation cells are highlighted, unlike text extraction)
#### R2.3: Instance count equals the number of selected cells
- **Given**: a normalized selection spanning start `(8,0)`→end `(1,1)` on a 10-column grid (2 cells on row 0 cols 8–9, then cols 0–1 on row 1)
- **When**: instances are built
- **Then**: exactly 4 instances are produced, one per member cell

### R3: The highlight composites as a translucent tint over the terminal
#### R3.1: The tint is alpha-blended so glyphs remain visible
- **Given**: the layer's pipeline
- **When**: it is configured
- **Then**: it uses alpha blending with a translucent selection color (glyphs beneath show through), not an opaque fill
#### R3.2: The highlight sits below the cursor and does not tint chrome
- **Given**: `SELECTION_Z = 4` with `CURSOR_Z=5`, `PREEDIT_Z=6`, `TAB_BAR_Z=10`, `IMAGE_Z=20`, `SIDEBAR_CHROME_Z=30`, `OVERLAY_Z=100`
- **When**: layers are ordered by `z_order`
- **Then**: the selection tint records after the glyph pass but before the cursor/preedit and all chrome, so the cursor stays visible over a selection and chrome painted later is not tinted

### R4: No selection renders nothing
#### R4.1: `None` selection records no pass
- **Given**: a snapshot with `selection == None` (no selection, or scrolled into history)
- **When**: `record` is called
- **Then**: it early-returns and issues no draw / zero instances
#### R4.2: An empty membership records nothing
- **Given**: a selection that yields zero member cells
- **When**: instances are built
- **Then**: the result is empty and no pass is recorded

## Tasks

### T1: Pure selected-cell→instance builder + unit tests [infra]
- **Fulfills**: R0.2, R2, R4.2
- **Depends on**: (none)
- In `crates/renderer/src/selection_layer.rs`, define the instance type and a pure `fn selection_instances(snapshot: &GridSnapshot, cell: [f32;2], left_inset: f32, top_inset: f32) -> Vec<Instance>` that iterates cells, tests `snapshot.selection`'s `contains` (incl. continuation cells), and maps to pixel rects. Unit tests for R2.1–R2.3 and R4.2 — no `wgpu::Device`.

### T2: `SelectionLayer` DrawLayer (pipeline/shader/blend) + module wiring [core]
- **Fulfills**: R0.1, R1, R3
- **Depends on**: T1
- Add `pub struct SelectionLayer` + `new()`, inline `WGSL`, lazily-built alpha-blended pipeline (mirroring `image_layer.rs`), `pub const SELECTION_Z: i32 = 4;` with the compile-time guard `const _: () = assert!(SELECTION_Z > 0 && SELECTION_Z < crate::cursor_layer::CURSOR_Z);`, and `impl DrawLayer` whose `record` builds instances via T1's function and early-returns on empty (R4.1). Add `pub mod selection_layer;` to `lib.rs`. Do not touch `Renderer::draw_to`. Optional smoke via `Renderer::new_headless`.

## External Dependencies

### Pre-work
- (none) — depends only on `selection-model`'s `GridSnapshot.selection` + `Selection::contains`, already in the same workspace.

### Post-work
- (none) — `copy-clipboard` registers this layer via an `Overlay` in `main.rs`.
