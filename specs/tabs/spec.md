# Spec: tabs

## Meta
- **Created**: 2026-07-17
- **Type**: dev
- **Status**: drafting
- **Set**: mac-terminal-v1 (`specs/mac-terminal-v1/partition-manifest.md`)

## Declared Surface
- feature-id: tabs
- depends_on: [glyph-rendering]
- declared-surface:
  - crates/app/**
  - crates/renderer/src/tab_bar.rs

## Goal
Turn the single-session app into a tabbed terminal: multiple independent zsh sessions (each its own `Pty` + `Terminal`), a rendered tab bar (as a `DrawLayer`), and keyboard shortcuts to create, close, and switch tabs. In doing so, own the winit event loop and expose the SD4 registration seam (input handlers + overlay hosts) that later app-touching features plug into.

## Non-goals
- Split panes / tiling (tabs only in v1)
- Session persistence across app restarts
- Renderer internals beyond the tab-bar layer (owned by `glyph-rendering`)
- Clipboard, overlay, or settings behavior (separate features that register through the seam)

## Confirmed Goal
The app hosts N zsh sessions as tabs, renders a tab bar via a registered `DrawLayer`, routes input to the active tab, supports create/close/switch via keyboard, and exposes an input/overlay registration seam so `image-paste`, `ai-monitor`, and `settings` extend the event loop without editing `terminal_app.rs`.

## Research
- `terminal_app.rs` holds a single `Running { window, renderer, terminal, pty, writer, rx, reader }` created in `resumed`; keyboard/resize/redraw handled inline (`crates/app/src/terminal_app.rs`).
- PTY reader thread → `mpsc` channel → `EventLoopProxy::send_event(UserEvent::PtyOutput)` wakes the loop; `ControlFlow::Wait` (SD6) (`crates/app/src/main.rs`, `terminal_app.rs`).
- `renderer::Renderer` will expose `add_layer` + a `DrawLayer` trait after `glyph-rendering` (dependency) — the tab bar is a layer.
- Input encoding (`encode_key`) maps keys → PTY bytes (`crates/app/src/terminal_app.rs`).

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
### D1: A `Tab` owns one session; `Session` bundles Pty + Terminal + reader thread
- **Status**: resolved
- **Rationale**: Each tab is a `{ Terminal, Pty, writer, reader-thread }`; the app holds `Vec<Tab>` + `active: usize`. Only the active tab renders and receives input; background tabs keep advancing their grid from their reader thread. Rejected: one shared Terminal (can't hold N sessions), lazily spawning on switch (loses background output).

### D2: PtyOutput wake carries the originating tab id
- **Status**: resolved
- **Rationale**: Each reader thread tags its wake with a tab id (via the channel payload); on wake the app advances that tab's grid and only requests redraw if it is the active tab. Rejected: one global "something changed" wake (would advance the wrong grid / redraw needlessly).

### D3: Tab bar is a registered `DrawLayer` (SD3)
- **Status**: resolved
- **Rationale**: `renderer/src/tab_bar.rs` implements `DrawLayer`, drawing tab titles in a strip; the app registers it via `Renderer::add_layer`. The grid viewport is offset down by the bar height. Rejected: drawing the bar in `lib.rs` (SD3 hub collision), a separate window (not native-feeling).

### D4: SD4 seam = `InputRouter` + `OverlayHost` registries (provisional contract)
- **Status**: provisional
- **Rationale**: `tabs` defines two registries consulted by `terminal_app.rs`:
  - `trait InputHandler { fn on_key(&mut self, ev: &KeyEvent, ctx: &mut AppCtx) -> Handled }` where `Handled` is an enum `{ Consumed, Passthrough }` — `Consumed` stops both default tab input and lower-priority handlers; `AppCtx` exposes the active tab's writer, the tab list/active index, and a redraw request.
  - `trait Overlay { fn priority(&self) -> i32; fn on_key(&mut self, ev, ctx) -> Handled; fn layer(&self) -> Box<dyn DrawLayer>; }` — an overlay both consumes input (when visible) and contributes a renderer `DrawLayer` (SD3).
  Handlers/overlays register from their own module — no `terminal_app.rs` edits. Marked **provisional**: the exact `AppCtx`/`Handled` shape and overlay ordering may be refined when the first real consumers (`image-paste`, `ai-monitor`, `settings`) land — freezing a seam before a consumer exists is a known risk; the contract is versioned by this spec until then. Rejected: a giant match in `window_event` (every feature edits it — SD4 prevents this); freezing the trait silently (this note makes the churn expectation explicit).

### D5: Tab keybindings — Cmd+T / Cmd+W / Cmd+Shift+[ ] / Cmd+1..9
- **Status**: resolved
- **Rationale**: macOS-native chords: Cmd+T new, Cmd+W close (last tab closing quits), Cmd+Shift+[ / ] previous/next, Cmd+1..9 jump. These are intercepted before `encode_key` so they never reach the shell. Rejected: Ctrl-based (collides with shell control chars), config-only (v1 needs sane defaults; settings feature can later override).

### D6: Tab-bar data flow — app pushes a `TabBarModel` into the layer each frame
- **Status**: resolved
- **Rationale**: The renderer-side `tab_bar.rs` `DrawLayer` holds its own `TabBarModel { titles: Vec<String>, active: usize }`; the app updates it (via the layer's `&mut self` from D-glyph's `DrawLayer`) before each frame from app-owned tab state. The renderer never reaches into `crates/app` — the app pushes plain data down, preserving SD2. Rejected: the renderer reading app state (couples renderer to app, breaks SD2), a shared global (hidden coupling).

### D7: Tab title = the shell process's cwd basename (app-side), `Tab N` fallback
- **Status**: resolved
- **Rationale**: The title is the basename of the tab's foreground-process working directory, obtained app-side from the PTY child pid (macOS `proc_pidinfo`/`libproc`) — NOT via OSC parsing, so `tabs` stays off the shared `parser.rs` (owned by `image-paste`). Falls back to `Tab N` when the cwd is unavailable. OSC-2 title override is deferred (it would require a parser.rs dispatch arm — a separate cross-feature coordination we avoid in v1). Rejected: OSC-2 in v1 (drags in the shared parser surface), static `Tab N` only (uninformative), full path (too long for the strip).

## Constraints
- C1: Non-owner features MUST NOT edit `terminal_app.rs` — they register via the SD4 seam (this feature provides it).
- C2: `crates/app` may depend on `winit`; `crates/renderer` still must not (tab_bar is a renderer-side `DrawLayer` with no winit).
- C3: Closing the last tab exits cleanly (drops all sessions → reader threads end, per scaffold shutdown model).

## Known Gaps
- Tab-bar visual design (height, active-tab styling) is a first cut; refined once `glyph-rendering` text metrics land.
- Max tab count unbounded in v1; a cap can be added later.

## Requirements

### R0: Multiple zsh sessions in tabs
#### R0.1: Two independent sessions
- **Given**: The app is running with one tab
- **When**: The user presses Cmd+T and runs `echo A` in tab 1 and `echo B` in tab 2
- **Then**: Each tab's grid shows only its own output; switching tabs shows the other session unchanged

### R1: Session lifecycle (D1, D2)
#### R1.1: New tab spawns a session
- **Given**: N tabs exist
- **When**: Cmd+T
- **Then**: A new `Session` (Pty+Terminal+reader thread) is created, becomes active, sized to the viewport
#### R1.2: Background tab keeps receiving output
- **Given**: Tab 2 is active; tab 1 runs a command producing output
- **When**: Output arrives for tab 1
- **Then**: Tab 1's grid advances (its reader thread + tagged wake) without redrawing the screen; switching to tab 1 shows the accumulated output
#### R1.3: Close tab ends its session
- **Given**: 2 tabs
- **When**: Cmd+W on the active tab
- **Then**: That session's Pty/threads are dropped, the other tab becomes active; closing the last tab exits the app

### R2: Tab bar layer (D3, SD3)
#### R2.1: Tab bar renders via a registered layer
- **Given**: 2 tabs, tab 2 active
- **When**: A frame renders
- **Then**: A tab bar strip shows two tab titles with tab 2 highlighted, drawn by the registered `DrawLayer` (no edit to `renderer/src/lib.rs`)
#### R2.2: Grid viewport offset by bar height
- **Given**: The tab bar occupies the top strip
- **When**: Grid cells render
- **Then**: The terminal grid begins below the bar (no overlap)

### R3: Input routing + SD4 seam (D4, D5)
#### R3.1: Input goes to the active tab
- **Given**: Tab 2 is active
- **When**: The user types `ls`
- **Then**: Bytes are written to tab 2's PTY only
#### R3.2: Tab chords are intercepted
- **Given**: Any tab is active
- **When**: The user presses Cmd+Shift+]
- **Then**: The next tab activates and no bytes are sent to any shell
#### R3.3: Registered handler runs before default input
- **Given**: A feature registers an `InputHandler` that consumes Cmd+V
- **When**: The user presses Cmd+V
- **Then**: The registered handler runs and returns `Consumed`, so `encode_key`/tab logic does not also process it — all without editing `terminal_app.rs`

### R4: Overlay registry (D4)
#### R4.1: A registered overlay draws and consumes input
- **Given**: A feature registers an `Overlay` (via the seam) that is currently visible
- **When**: A frame renders and a key is pressed
- **Then**: The overlay's `DrawLayer` is drawn above the grid AND its `on_key` sees the key first; returning `Consumed` stops the key from reaching the shell — without editing `terminal_app.rs`
#### R4.2: A passthrough overlay does not swallow input
- **Given**: A registered overlay returns `Passthrough` for a key
- **When**: That key is pressed
- **Then**: Default tab input processes it (bytes reach the active shell)

### R5: Resize handling (D1)
#### R5.1: Window resize re-fits every tab's PTY
- **Given**: 3 tabs exist (1 active, 2 background)
- **When**: The window is resized
- **Then**: All 3 tabs' grids re-fit to the new cols×rows and all 3 PTYs are resized (background shells get correct winsize, not just the active one)

### R6: Tab bar model (D6, D7)
#### R6.1: Tab bar reflects titles + active index
- **Given**: 2 tabs whose cwds are `/a/foo` and `/a/bar`, tab 2 active
- **When**: A frame renders
- **Then**: The tab bar shows `foo` and `bar` with `bar` highlighted, driven by the app-pushed `TabBarModel` (renderer reads no app state)

## Tasks

### T1: Session struct + multi-session app state [infra]
- **Fulfills**: R1
- **Depends on**: (none)

### T2: SD4 seam — InputRouter + OverlayHost registries [infra]
- **Fulfills**: R3.3, R4
- **Depends on**: T1

### T3: Tab keybindings + input routing + resize propagation [vertical]
- **Fulfills**: R0, R3.1, R3.2, R5
- **Depends on**: T2

### T4: Tab-bar DrawLayer + TabBarModel + cwd titles + viewport offset [vertical]
- **Fulfills**: R2, R6
- **Depends on**: T1

## External Dependencies
### Pre-work
- `glyph-rendering` must provide the `DrawLayer`/`add_layer` seam (declared dependency).
### Post-work
- `image-paste`, `ai-monitor`, `settings` register through the SD4 seam this feature provides.
