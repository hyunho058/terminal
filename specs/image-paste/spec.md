# Spec: image-paste

## Meta
- **Created**: 2026-07-17
- **Type**: dev
- **Status**: drafting
- **Set**: mac-terminal-v1 (`specs/mac-terminal-v1/partition-manifest.md`)

## Declared Surface
- feature-id: image-paste
- depends_on: [glyph-rendering, tabs, scrollback]
- declared-surface:
  - crates/term-core/src/parser.rs
  - crates/term-core/src/image.rs
  - crates/term-core/src/snapshot.rs
  - crates/renderer/src/image_layer.rs
  - crates/app/src/clipboard.rs

## Goal
Support images two ways (both, per `insights.md`): (a) inline display — decode an image-transfer escape (iTerm2/Kitty-style) into an image the renderer draws in the grid; and (b) clipboard-to-path — when the user pastes a clipboard image, write it to a file and insert that file path into the shell input, so external AI CLIs (claude/gemini) can consume it.

## Non-goals
- Animated images / video, image editing
- Sixel (Kitty/iTerm2 protocol chosen; sixel deferred)
- Being an AI client itself — we only hand a path to the external CLI

## Confirmed Goal
`term-core` parses an inline-image escape into an `ImageLayer` side-channel (keyed by cell coords, per SD5), the renderer draws those images as a registered `DrawLayer`, and a clipboard paste of an image writes a temp file and types its path into the active tab's PTY — both paths reachable in normal use.

## Research
- `Processor`/`Performer` dispatches `vte` callbacks into the grid; OSC/APC hooks (`osc_dispatch`, `hook/put/unhook`) are currently unused (`crates/term-core/src/parser.rs`) — the inline-image protocol rides these.
- The renderer consumes only `GridSnapshot`; per SD5 images are a side channel, drawn by a registered `DrawLayer` (seam from `glyph-rendering`).
- The app writes to the active tab's PTY via `writer` and registers input handlers via the SD4 seam (from `tabs`); clipboard needs `objc2`/AppKit (`NSPasteboard`) — allowed by SD1.

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
### D1: Inline images ride the Kitty graphics protocol over APC escapes
- **Status**: resolved
- **Rationale**: Parse the Kitty `APC G ... ST` transmit escape via `vte`'s `hook/put/unhook` (payload = base64 image chunks); decode with the `image` crate into RGBA. Kitty's protocol is well-documented and cell-anchored, matching our grid. Rejected: iTerm2 OSC 1337 (less precise cell placement), sixel (palette-limited, deferred), a custom protocol (no external-tool interop).

### D2: Images are an `ImageLayer` side channel, anchored to cell coords (SD5)
- **Status**: resolved
- **Rationale**: `term-core/src/image.rs` holds `ImageLayer { placements: Vec<{ id, top_left_cell, cols, rows, rgba }> }`, exposed on the snapshot alongside cells; scrolling moves placements with the grid. Cells under an image stay blank text-wise. Rejected: storing pixels in cells (breaks the char model — SD5), a separate absolute-pixel overlay (wouldn't scroll with content).

### D3: Renderer draws images as a registered `DrawLayer` (SD3)
- **Status**: resolved
- **Rationale**: `renderer/src/image_layer.rs` uploads each placement's RGBA to a texture and draws a quad at the cell rect, registered via `add_layer` under the glyph pass. Rejected: editing `lib.rs`’s cell pass (SD3 collision), CPU blitting (defeats GPU).

### D4: Clipboard paste writes a temp file and types its path (path mode)
- **Status**: resolved
- **Rationale**: `app/src/clipboard.rs` registers an `InputHandler` (SD4 seam) for Cmd+V; if `NSPasteboard` holds an image, write it to `~/.cache/mac-terminal/pasted/<ts>.png` and write that path (+ space) to the active tab's PTY — so `claude`/`gemini` on the CLI receive a real file path. If the pasteboard holds text, fall through to normal paste. Rejected: only inline display (external AI CLIs can't read our GPU buffer — the exact Warp limitation in `insights.md`), always-inline (breaks the CLI-handoff use case).

### D5: The two modes are independent and both always available
- **Status**: resolved
- **Rationale**: Inline display (D1–D3) handles images the *shell/program emits*; path mode (D4) handles images the *user pastes*. Neither disables the other — "both" per the requirement. Rejected: a mode toggle (the interview explicitly chose both simultaneously).

## Constraints
- C1: `term-core` (`parser.rs`, `image.rs`) stays GPU/UI-free — SD2; decoding produces RGBA bytes only.
- C2: `image_layer.rs` registers via the `DrawLayer` seam; no `renderer/src/lib.rs` edits — SD3.
- C3: `clipboard.rs` registers via the SD4 seam; no `terminal_app.rs` edits — SD4.
- C4: Pasted temp files land under a cache dir the user can clear; never overwrite an existing path.

## Known Gaps
- Kitty-protocol coverage subset for v1 (transmit + place; delete/animation deferred) — exact opcodes TBD in implementation.
- Max inline-image size / total image VRAM budget to be validated under `performance-harness`.
- ~~Shared-parser coordination~~ **RESOLVED:** `scrollback` owns the term-core screen-state files (`parser.rs`, `snapshot.rs`, `grid.rs`); this feature adds its Kitty-image dispatch to `parser.rs` and an image-layer field to `snapshot.rs` as an **ordered** extension via `depends_on: scrollback`. No unordered collision.

## Requirements

### R0: Images work both ways
#### R0.1: Inline display
- **Given**: A program emits a Kitty graphics escape for a 4×2-cell image at the cursor
- **When**: The frame renders
- **Then**: The decoded image is drawn spanning those cells, in place, over the grid
#### R0.2: Clipboard-to-path
- **Given**: The macOS clipboard holds a PNG image and the active shell prompt is focused
- **When**: The user presses Cmd+V
- **Then**: A file is written under the cache dir and its absolute path (plus a trailing space) is inserted into the shell input

### R1: Inline-image parsing (D1, D2)
#### R1.1: Parse a graphics escape into a placement
- **Given**: A complete Kitty transmit-and-place APC escape arrives via the PTY
- **When**: `Processor::advance` consumes it
- **Then**: The base64 payload is decoded to RGBA and an `ImageLayer` placement is added at the cursor cell with the right cols×rows; no image bytes are stored in cells
#### R1.2: Placement scrolls with content
- **Given**: An image placement exists and the grid scrolls up one line
- **When**: The next snapshot is taken
- **Then**: The placement's top-left cell moves up by one row (and is dropped when it scrolls off the retained view)

### R2: Inline-image rendering (D3, SD3)
#### R2.1: Draw a placement
- **Given**: A snapshot carries one image placement
- **When**: The registered image `DrawLayer` records the frame
- **Then**: The image texture is drawn at the placement's cell rectangle, without any edit to `renderer/src/lib.rs`

#### R1.3: Malformed image payload is dropped safely
- **Given**: A graphics escape arrives whose base64 payload is truncated or not a decodable image format
- **When**: `Processor::advance` consumes it
- **Then**: The placement is dropped (no `ImageLayer` entry added), the parser state stays valid for subsequent bytes, and no panic occurs

### R3: Clipboard path mode (D4, SD4)
#### R3.1: Image on the pasteboard → file + path
- **Given**: `NSPasteboard` has image data
- **When**: The registered Cmd+V `InputHandler` fires
- **Then**: A uniquely-named PNG is written under `~/.cache/mac-terminal/pasted/` (dir created if missing) and its absolute path + space is written to the active tab's PTY; the handler returns `Consumed`
#### R3.2: Text on the pasteboard → normal paste
- **Given**: The pasteboard holds text, not an image
- **When**: Cmd+V fires
- **Then**: The handler returns `Passthrough` so the default text paste proceeds (no file written)
#### R3.3: Rapid pastes never overwrite (C4)
- **Given**: Two image pastes occur within the same clock second
- **When**: Both write files
- **Then**: The second uses a distinct filename (e.g. timestamp + counter/nonce) — no existing file is overwritten

### R4: Modes coexist (D5)
#### R4.1: Inline display and path paste are independent
- **Given**: An inline image is already displayed in the grid
- **When**: The user does a Cmd+V clipboard-image paste
- **Then**: The paste writes a file + path as usual and the existing inline image remains displayed — neither mode disables the other

## Tasks

### T1: Kitty-protocol parse → ImageLayer in term-core (+ decode-failure safety) [infra]
- **Fulfills**: R1
- **Depends on**: (none)

### T2: Image DrawLayer in renderer [vertical]
- **Fulfills**: R0.1, R2, R4.1
- **Depends on**: T1

### T3: Clipboard InputHandler (path mode, unique filenames) via SD4 seam [vertical]
- **Fulfills**: R0.2, R3
- **Depends on**: (none)  ← parallel with T1/T2 (separate surface)

## External Dependencies
### Pre-work
- `glyph-rendering` `DrawLayer` seam, `tabs` SD4 input seam, and `scrollback` (owns `parser.rs` + `snapshot.rs`, which this feature extends for image dispatch/placements) must exist (declared dependencies).
- Add `image` (decode) to `crates/term-core`; `objc2-app-kit` (NSPasteboard) to `crates/app`.
### Post-work
- (none)
