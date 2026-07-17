# Spec: settings

## Meta
- **Created**: 2026-07-17
- **Type**: dev
- **Status**: drafting
- **Set**: mac-terminal-v1 (`specs/mac-terminal-v1/partition-manifest.md`)

## Declared Surface
- feature-id: settings
- depends_on: [glyph-rendering, tabs]
- declared-surface:
  - crates/common/src/config.rs
  - crates/common/src/watch.rs
  - crates/app/src/settings.rs

## Goal
Make font, color theme, and ligature settings live: watch `~/.config/mac-terminal/config.toml`, reload it on change, and push the new values into the running renderer/terminal without a restart. Editing the file re-themes the terminal immediately.

## Non-goals
- A graphical settings panel (v1 edits the TOML file directly; a UI can come later)
- New config fields beyond the existing font/theme/grid (this feature wires reload, not schema growth)
- Per-tab settings (config is app-wide in v1)

## Confirmed Goal
`common` gains a config file watcher; the app subscribes (via the SD4 seam), and on a valid change re-applies font size/family/ligatures and theme colors to the renderer and to each tab's default cells live — invalid edits are rejected with the previous config kept.

## Research
- `Config` (font/theme/grid) loads from `~/.config/mac-terminal/config.toml` with serde + `deny_unknown_fields`; `load()` falls back to defaults if absent (`crates/common/src/config.rs`).
- Renderer derives cell metrics + clear color from `Config` at construction (`cell_size_px`, `to_wgpu_color` in `crates/renderer/src/lib.rs`); these must be re-derivable on reload.
- Font family/size/ligatures feed `glyph-rendering`'s atlas + shaping; a change invalidates cached glyphs.
- The `tabs` SD4 seam lets a feature register loop participation without editing `terminal_app.rs`; `notify` (already used by ai-monitor's watcher pattern) tails files.

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
### D1: `ConfigWatcher` watches the parent DIR (survives atomic-rename saves)
- **Status**: resolved
- **Rationale**: `ConfigWatcher` watches the config file's **parent directory** with `notify` and filters events to `config.toml`, then re-runs `Config::load_from`. This is load-bearing: editors (vim `:w`, VS Code) save atomically by writing a temp file and `rename()`-ing it over the target, which changes the inode — a file-level watch would follow the dead inode and go silent after the first save (re-theme once, then never again). Directory-watching (or re-arming after each event) is the standard fix. Lives in `common` (UI-free, SD2). Rejected: watching the file inode directly (breaks on the first atomic save — the common case), watching from `app` (untestable), returning raw text (callers re-parse).

### D2: Invalid config is rejected, previous kept — watcher emits only `Ok`
- **Status**: resolved
- **Rationale**: The watcher's channel contract is **emit only a valid `Config`**: on a parse/validation error (bad TOML, unknown key via `deny_unknown_fields`, non-positive size) it logs a warning and emits nothing, so the app's last-applied `Config` remains in effect (last-good lives in the app, which holds the currently-applied config). A typo never blanks the terminal. `Config::load_from`'s typed errors distinguish the case. Rejected: emitting a `Result`/error variant (pushes last-good bookkeeping into every consumer — the app already holds the applied config), falling back to defaults on error (silently discards the whole user config), crashing (hostile for a daily driver).

### D3: App applies changes through an `apply_config` path (SD4)
- **Status**: resolved
- **Rationale**: `app/src/settings.rs` registers a loop participant via the SD4 seam that, on a `Config` change from the watcher (delivered by channel + `EventLoopProxy` wake, SD6), calls `renderer.apply_config(&cfg)` (re-derive metrics/clear color, invalidate glyph cache) and updates each tab's default cell colors, then redraws. Rejected: reconstructing the `Renderer` (drops GPU state, flicker), editing `terminal_app.rs` (SD4 collision).

### D4: `renderer` gains `apply_config` (owned by glyph-rendering's surface)
- **Status**: assumed
- **Rationale**: Live re-theming needs the renderer to accept new font/theme values and invalidate the glyph atlas. That method lives in `crates/renderer/**` (owned by `glyph-rendering`); this feature depends_on `glyph-rendering` and consumes the method. Recorded as a cross-feature interface expectation, surfaced to coherence-audit. Rejected: this feature editing the renderer (would violate the disjoint surface — it depends on glyph-rendering exposing the seam instead).

## Constraints
- C1: `common` stays UI/GPU-free — SD2; the watcher emits `Config`, nothing renderer-typed.
- C2: `app/src/settings.rs` registers via the SD4 seam — no `terminal_app.rs` edits.
- C3: A live font-size change re-derives cell metrics and re-fits every tab's grid (cols×rows) + resizes each PTY.
- C4: Invalid config must never leave the terminal unusable (D2).

## Known Gaps
- Debounce window for rapid successive saves (editors write twice) — a short coalescing delay, value TBD in implementation.
- `apply_config` in the renderer is an interface this feature expects from `glyph-rendering` (D4); confirmed by coherence-audit and the build order.

## Requirements

### R0: Live settings reload
#### R0.1: Editing config re-themes without restart
- **Given**: The terminal is running with the default dark theme
- **When**: The user edits `config.toml` to a new `theme.background` and saves
- **Then**: Within a moment the terminal's background clears to the new color, no restart

### R1: Config watcher (D1)
#### R1.1: Watcher emits on change
- **Given**: A `ConfigWatcher` on an existing config file
- **When**: The file is modified with valid TOML
- **Then**: The watcher emits a new `Config` reflecting the change
#### R1.2: Watcher survives a missing file
- **Given**: No config file exists yet
- **When**: The user creates one
- **Then**: The watcher picks it up and emits the parsed `Config` (no earlier panic)
#### R1.3: Watcher survives an atomic-rename save
- **Given**: A `ConfigWatcher` is active and the user saves the file via an editor that writes-temp-then-renames (changing the inode)
- **When**: The save completes
- **Then**: The watcher still detects the change and emits the new `Config` — and continues to emit on subsequent saves (not just the first)

### R2: Validation safety (D2)
#### R2.1: Invalid edit keeps the previous config
- **Given**: A valid config is loaded
- **When**: The file is saved with invalid TOML (or an unknown key)
- **Then**: The change is rejected with a logged warning and the previously-applied `Config` stays in effect (terminal unchanged)

### R3: Apply to renderer + tabs (D3, D4)
#### R3.1: Font size change re-fits the grid
- **Given**: The terminal shows a 100×30 grid
- **When**: `font.size_pt` is increased and saved
- **Then**: Cell metrics grow, the grid re-fits to fewer cols×rows, and each tab's PTY is resized to match
#### R3.2: Ligature toggle takes effect
- **Given**: Ligatures are on and `!=` renders as a ligature
- **When**: `font.ligatures = false` is saved
- **Then**: `!=` re-renders as two glyphs (glyph cache invalidated via `apply_config`)
#### R3.3: Theme change repaints
- **Given**: A running terminal
- **When**: `theme.foreground` changes and is saved
- **Then**: Newly-default cells use the new foreground and the next frame reflects it

## Tasks

### T1: ConfigWatcher in common (notify + revalidate) [infra]
- **Fulfills**: R1, R2
- **Depends on**: (none)

### T2: App settings module — subscribe + apply via SD4 seam [vertical]
- **Fulfills**: R0, R3
- **Depends on**: T1

## External Dependencies
### Pre-work
- `glyph-rendering` exposes `Renderer::apply_config` (D4 interface expectation); `tabs` SD4 seam + per-tab resize.
- Add `notify` to `crates/common`.
### Post-work
- (none)
