# Spec: glyph-rendering

## Meta
- **Created**: 2026-07-17
- **Type**: dev
- **Status**: drafting
- **Set**: mac-terminal-v1 (`specs/mac-terminal-v1/partition-manifest.md`)

## Declared Surface
- feature-id: glyph-rendering
- depends_on: []
- declared-surface:
  - crates/renderer/**

## Goal
Replace the scaffold's block-font renderer with real anti-aliased glyph rendering: shape and rasterize font glyphs with `swash`, pack them into a GPU atlas, and draw each cell's character textured on the wgpu/Metal pipeline — honoring programming ligatures when enabled.

## Non-goals
- Terminal state / VT parsing changes (owned by `term-core`; this feature only reads `GridSnapshot`)
- Scrollback, tabs, images, overlays (separate features; they register as draw layers per SD3)
- Color emoji / complex-script (CJK, RTL) shaping — deferred; Latin + ligatures first
- Finalizing performance numbers (owned by `performance-harness`)

## Confirmed Goal
The renderer draws zsh output as real glyphs (not colored blocks) via a `swash`-rasterized GPU glyph atlas, establishes the pluggable draw-layer API (SD3) that later visual features extend, and honors `config.font.ligatures`.

## Research
- Block-font renderer exists: non-blank cells drawn as fg-colored instanced quads (`crates/renderer/src/lib.rs`), one draw call per frame from a `GridSnapshot`.
- `swash` rasterization already proven in `crates/renderer/src/glyph.rs` (`GlyphRasterizer::rasterize` → alpha-coverage `GlyphBitmap`); atlas + shader wiring is the remaining work.
- Instanced-quad pipeline + uniform (`viewport`, `cell`) + `cells.wgsl` in place (`crates/renderer/src/lib.rs`, `crates/renderer/src/cells.wgsl`).
- Cell metrics derived from `config.font.size_pt × scale` (`cell_size_px`, `crates/renderer/src/lib.rs`).
- Renderer is window-agnostic (raw-window-handle), consumes only `term_core::GridSnapshot` (`crates/renderer/src/lib.rs`).

## Decisions

<!-- Shared Decisions SD1–SD7 inherited verbatim from the partition manifest -->
### SD1: Stack & platform — Rust, pure wgpu/Metal, macOS ARM64 + zsh only
- **Status**: resolved
- **Rationale**: Requirement (순정 GPU 스택, macOS 전용, zsh 로컬 전용). Hand-rolled cell rendering, no GUI/widget crates (winit + objc2 are the only allowed windowing/platform crates). No cross-platform `#[cfg]`, no SSH/remote, no other shells. Rejected: GUI toolkits (egui/iced — defeat the manual-cell goal), cross-platform abstraction (out of scope).

### SD2: Dependency direction — `common ← term-core ← renderer ← app`
- **Status**: resolved
- **Rationale**: `term-core` (grid, vte parsing, PTY) MUST stay GPU-free so terminal logic is headless-testable; `renderer` is window-agnostic (raw-window-handle, no winit). Every feature preserves this direction. Rejected: letting the renderer own terminal state (couples GPU to logic, kills headless tests).

### SD3: Renderer extension seam — pluggable draw layers, not edits to `renderer/src/lib.rs`
- **Status**: resolved
- **Rationale**: `glyph-rendering` establishes a draw-layer API in `crates/renderer`; every later visual feature (tab bar, image layer, ai-monitor overlay) implements a layer in its OWN module and registers through that API rather than editing the central `Renderer`. Keeps feature surfaces disjoint and makes the renderer feature-extensible. Rejected: each feature editing `lib.rs` (creates an unordered hub collision).

### SD4: App event-loop extension seam — owned by `tabs`, extended via registration
- **Status**: resolved
- **Rationale**: `tabs` owns the winit event loop (`crates/app`) and exposes an input/overlay registration seam. Other app-touching features (clipboard paste, ai-monitor overlay, settings reload) add self-contained modules under `crates/app/` and register through the seam instead of editing `terminal_app.rs`. Rejected: multiple features editing `terminal_app.rs` directly (unordered hub collision).

### SD5: Terminal contract — extend `GridSnapshot` via side-channel layers, reuse `vte`
- **Status**: resolved
- **Rationale**: The `term-core → renderer` boundary is `GridSnapshot`. Features add side-channel data (image layer keyed by cell coords, scrollback viewport offset) rather than overloading cell semantics; escape parsing reuses the `vte` crate, grid control stays hand-rolled. Rejected: encoding images/overlays as special cells (pollutes the grid model).

### SD6: Config & concurrency — TOML+serde, threads+channels, event-driven redraw
- **Status**: resolved
- **Rationale**: Config is TOML+serde in `common`. Concurrency is threads + channels (PTY reader thread → channel → UI thread); the render loop is winit-event-driven and woken via `EventLoopProxy` (`ControlFlow::Wait`) — no async runtime. Rejected: tokio/async (overkill for the few I/O sources).

### SD7: Verification & performance gates
- **Status**: resolved
- **Rationale**: Headless `term-core` behavior is tested with `insta` snapshots; GPU paths get a headless smoke test. Performance is judged by numeric targets (candidate: 120fps sustained / input→screen ≤ 8ms / 100k-line scroll with no frame drops) — the exact numbers are OPEN (gap G1) and are confirmed inside the `performance-harness` feature. Until confirmed, other features treat them as goals, not pass/fail gates.

<!-- Feature-local decisions -->
### D1: Dynamic glyph atlas — a single R8Unorm coverage texture, filled on demand
- **Status**: resolved
- **Rationale**: Rasterize each `(glyph_id, px_size)` once via `swash`, pack into a growing shelf-allocated atlas texture, cache the UV rect by key. A textured second pipeline samples coverage as alpha and multiplies by cell fg. Rejected: pre-baking a full ASCII sheet (misses ligatures/Unicode), one-texture-per-glyph (bind-group churn), MSDF (overkill for pixel-grid terminal text).

### D2: Shaping keyed on `config.font.ligatures`
- **Status**: resolved
- **Rationale**: With ligatures on, shape each run with `swash`'s shaper so `!=`/`=>`/`->` map to their ligature glyphs; with ligatures off, map char→glyph 1:1 via the charmap (current path). Rejected: always-1:1 (no ligatures — a stated v1 requirement), always-shape (unnecessary cost when disabled).

### D3: Two-pass draw — background quads, then textured glyph quads
- **Status**: resolved
- **Rationale**: Pass 1 clears to theme bg + draws per-cell bg quads (existing pipeline, kept for non-default backgrounds); pass 2 draws one textured instanced quad **per glyph cluster** (not strictly per cell) sampling the atlas — see D5 for multi-cell clusters. Keeps the hot path one buffer-upload + two draws. Rejected: single-pass with per-cell texture (can't batch), CPU compositing (defeats GPU purpose).

### D4: DrawLayer registry is the SD3 seam (provisional contract)
- **Status**: provisional
- **Rationale**: Define `trait DrawLayer { fn z_order(&self) -> i32; fn record(&mut self, ctx: &mut FrameCtx<'_>); }` and `Renderer::add_layer(Box<dyn DrawLayer>)`; the glyph pass is the first layer (z=0). **`&mut self`** so a layer can own per-frame state (animation, its own atlas/texture cache, tab/overlay state) without interior-mutability hacks. **`z_order`** makes draw order explicit and deterministic (glyph 0 < tab bar 10 < image 20 < overlay 100), not registration-order-dependent — SD3's own layering (overlay above tab bar above glyphs) requires this. `FrameCtx` exposes exactly: `&Device`, `&Queue`, the active `&mut RenderPass`, the shared glyph atlas + metrics, viewport size, cell metrics, and the current `&GridSnapshot`. Marked **provisional**: the contract may be refined when the first downstream consumer (`tabs`) lands, per the maker≠checker note that freezing a seam before a consumer exists is a risk. Rejected: hard-coded pass list (forces `lib.rs` edits per feature — the hub collision SD3 prevents); `&self` record (blocks per-layer mutable state — a breaking change once consumers exist); registration-order draw order (non-deterministic layering).

### D5: A glyph cluster (incl. multi-cell ligatures) is one positioned quad
- **Status**: resolved
- **Rationale**: Shaping (D2) yields glyph *clusters*; each cluster is drawn as ONE quad whose width = the cluster's advance (which may span 2+ cells for a ligature like `!=`). The cells a ligature covers are still real grid cells (their chars remain for copy/selection), but only the cluster's lead cell emits a draw; trailing covered cells emit no glyph quad. This resolves the D2×D3 tension: the glyph pass instances per *cluster*, not per *cell*. Rejected: strict per-cell quads (can't represent a ligature spanning cells), splitting a ligature back into per-cell slices (defeats the ligature).

### D6: Glyph metrics cached alongside coverage; baseline-aligned placement
- **Status**: resolved
- **Rationale**: The atlas entry for `(glyph_id, px)` stores not just the coverage rect but the glyph's `left`/`top` bearing and `advance` (already produced by `swash`). The quad is positioned within the cell by the font baseline + bearings, not naively at the cell origin — otherwise descenders/ascenders misalign. Rejected: assuming glyphs fill the cell box (breaks alignment for proportional-height glyphs and ligatures).

## Constraints
- C1: `crates/renderer` must not depend on `winit` (raw-window-handle only) — SD2.
- C2: No GUI/widget crates — SD1.
- C3: Glyph raster cache must be bounded (evict least-recently-used entries) to avoid unbounded VRAM growth under many distinct glyphs.
- C4: sRGB correctness preserved — coverage blends in the surface's sRGB space as today (`to_linear_f32`).

## Known Gaps
- Color-emoji and complex-script (CJK/RTL) shaping deferred; ASCII + Latin ligatures only in v1.
- Atlas eviction policy (LRU threshold, atlas resize vs evict) to be tuned against real workloads under `performance-harness`.
- Subpixel vs grayscale AA choice left to implementation; grayscale coverage assumed adequate for a dark terminal.

## Requirements

### R0: Real glyph rendering replaces block-font
#### R0.1: zsh output renders as glyphs
- **Given**: The terminal shows a shell prompt and typed command `echo hi`
- **When**: A frame is rendered
- **Then**: The characters `e c h o   h i` are drawn as their font glyphs (recognizable letterforms), not solid rectangles

### R1: Glyph atlas (D1)
#### R1.1: Rasterize-and-cache a glyph
- **Given**: A cell contains char `A` at the configured size, not yet rasterized
- **When**: The renderer needs its glyph
- **Then**: `swash` rasterizes it once, the coverage bitmap is packed into the atlas texture, and its UV rect is cached under `(glyph_id, px)`
#### R1.2: Reuse a cached glyph
- **Given**: `A` was rasterized on a prior frame
- **When**: `A` appears again
- **Then**: The cached UV rect is used with no re-rasterization (no new atlas write)
#### R1.3: Bounded atlas under churn
- **Given**: More distinct glyphs are requested than the atlas holds
- **When**: The atlas is full
- **Then**: Least-recently-used entries are evicted (or the atlas grows within a cap) — VRAM stays bounded, no panic

### R2: Ligature shaping (D2)
#### R2.1: Ligatures on
- **Given**: `config.font.ligatures = true` and the buffer contains `!=`
- **When**: The run is shaped and rendered
- **Then**: The `!=` ligature glyph is drawn (single combined glyph), spanning its two cells
#### R2.2: Ligatures off
- **Given**: `config.font.ligatures = false` and the buffer contains `!=`
- **When**: Rendered
- **Then**: `!` and `=` are drawn as two separate glyphs

### R3: Textured two-pass draw (D3)
#### R3.1: Foreground glyph color
- **Given**: A cell has fg = ANSI red and char `R`
- **When**: Rendered
- **Then**: The `R` glyph is drawn in red (coverage × fg), over the cell background
#### R3.2: Non-default cell background
- **Given**: A cell has a non-default bg color
- **When**: Rendered
- **Then**: The cell's background quad is filled with that color behind the glyph

### R4: DrawLayer seam (D4, SD3)
#### R4.1: Register an extra layer
- **Given**: A caller constructs a `DrawLayer` implementer
- **When**: It calls `Renderer::add_layer(layer)`
- **Then**: The layer's `record` runs each frame after the glyph pass, without any edit to `renderer/src/lib.rs`
#### R4.2: Headless smoke test
- **Given**: A headless wgpu device
- **When**: A frame renders a small snapshot with one glyph and one registered no-op layer
- **Then**: `render` returns `Ok` and the layer's `record` was invoked (SD7 smoke test)
#### R4.3: Layers draw in z-order, not registration order
- **Given**: Two layers registered `[z=100, z=10]` in that order
- **When**: A frame records
- **Then**: The z=10 layer records before the z=100 layer (deterministic ordering regardless of registration order)

### R5: Baseline-aligned placement (D5, D6)
#### R5.1: Glyph positioned by metrics
- **Given**: A cell holds `g` (a glyph with a descender)
- **When**: Rendered
- **Then**: The `g` quad is placed by the font baseline + bearing so the descender extends below the baseline (not clipped to the cell top)
#### R5.2: Ligature quad spans its cells from the lead cell
- **Given**: `config.font.ligatures = true` and cells `[!, =]` shape to one `!=` ligature
- **When**: Rendered
- **Then**: A single quad of the cluster's advance width is drawn from the lead cell; the trailing cell emits no glyph quad; both underlying cell chars remain in the grid

### R6: Missing-glyph fallback (edge)
#### R6.1: Unsupported char renders a visible placeholder
- **Given**: A cell holds a char with no glyph in the configured font (charmap → `.notdef`)
- **When**: Rendered
- **Then**: A `.notdef`/tofu placeholder glyph is drawn (not a panic, not an invisible gap), and the cache does not thrash on repeated misses

## Tasks

### T1: Glyph atlas module [infra]
- **Fulfills**: R1
- **Depends on**: (none)

### T2: DrawLayer seam + FrameCtx [infra]
- **Fulfills**: R4
- **Depends on**: (none)  ← parallel with T1

### T3: Textured glyph pass (shader + two-pass draw + metrics placement + .notdef) [vertical]
- **Fulfills**: R0, R3, R5.1, R6
- **Depends on**: T1, T2

### T4: Ligature-aware shaping + cluster quads [vertical]
- **Fulfills**: R2, R5.2
- **Depends on**: T3

### T5: Headless render smoke test [vertical]
- **Fulfills**: R4.2
- **Depends on**: T3

## External Dependencies
### Pre-work
- (none) — `swash`, `bytemuck`, `wgpu` already in `crates/renderer/Cargo.toml`
### Post-work
- Later visual features (`tabs`, `image-paste`, `ai-monitor`) consume the `DrawLayer` seam from this feature.
