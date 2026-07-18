# partition-manifest.md — mac-terminal-v1
- generated_at: 2026-07-17
- goal: Turn the scaffolded Rust/wgpu macOS terminal foundation into a daily-driver v1 — real glyph text rendering, scrollback that works under tmux, tabbed zsh sessions, image paste (inline display + clipboard-to-file path), a native ai-monitor toggle overlay, and live font/theme/ligature settings — meeting numeric performance targets. (from requirements-interview-outputs/mac-terminal/insights.md)

## Shared Decisions
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
- **Rationale**: Headless `term-core` behavior is tested with `insta` snapshots; GPU paths get a headless smoke test. Performance is judged by numeric targets (candidate: 120fps sustained / input→screen ≤ 8ms / 100k-line scroll with no frame drops) — the exact numbers are OPEN (gap G1) and are confirmed inside the `performance-harness` feature, which owns the benches and the CI gate. Until confirmed, other features treat them as goals, not pass/fail gates.

## Features  (ordered — a dependency appears before its dependents)
### glyph-rendering
- declared-surface:
  - crates/renderer/**
- depends_on: []
- accept-seed: zsh output renders as real anti-aliased glyphs (not colored blocks); programming ligatures are honored when enabled in config.
- pre-approved-batch: yes

### scrollback
- declared-surface:
  - crates/term-core/src/grid.rs
  - crates/term-core/src/snapshot.rs
  - crates/term-core/src/scrollback.rs
  - crates/term-core/src/parser.rs
  - crates/term-core/tests/scrollback.rs
- depends_on: []
- accept-seed: The user scrolls up and down through terminal history, including inside a tmux session; a 100k-line buffer scrolls without content loss.
- pre-approved-batch: yes
<!-- scrollback owns term-core screen-state: grid + snapshot + scrollback + the VT dispatch (parser.rs, incl. alt-screen ?1049h/l). image-paste extends parser.rs + snapshot.rs AFTER it (ordered share via image-paste's depends_on: scrollback). -->



### tabs
- declared-surface:
  - crates/app/**
  - crates/renderer/src/tab_bar.rs
- depends_on: [glyph-rendering]
- accept-seed: Multiple zsh sessions run in tabs; a tab bar renders; keyboard shortcuts create, close, and switch between tabs.
- pre-approved-batch: yes

### image-paste
- declared-surface:
  - crates/term-core/src/parser.rs
  - crates/term-core/src/image.rs
  - crates/term-core/src/snapshot.rs
  - crates/renderer/src/image_layer.rs
  - crates/app/src/clipboard.rs
- depends_on: [glyph-rendering, tabs, scrollback]
- accept-seed: Pasting an image displays it inline in the grid AND writes it to a file whose path is inserted into the shell for external AI CLIs.
- pre-approved-batch: yes
<!-- Ordered extension of scrollback's term-core surface: adds Kitty-image dispatch to parser.rs and an image-layer field to snapshot.rs AFTER scrollback (depends_on: scrollback). -->



### ai-monitor
- declared-surface:
  - crates/ai-monitor/**
  - crates/renderer/src/overlay_layer.rs
  - crates/app/src/overlay.rs
- depends_on: [glyph-rendering, tabs]
- accept-seed: A hotkey toggles an overlay that natively renders the active session's Claude/Gemini log events (from ~/.claude/projects/**/*.jsonl and Gemini CLI logs).
- pre-approved-batch: yes

### settings
- declared-surface:
  - crates/common/src/config.rs
  - crates/common/src/watch.rs
  - crates/app/src/settings.rs
- depends_on: [glyph-rendering, tabs]
- accept-seed: Editing config.toml live-updates font, color theme, and ligature settings without a restart.
- pre-approved-batch: yes

### performance-harness
- declared-surface:
  - benches/**
  - .github/workflows/perf.yml
  - docs/performance.md
- depends_on: [glyph-rendering, scrollback]
- accept-seed: Benchmarks assert the confirmed numeric targets (sustained fps, input→screen latency, 100k-line scroll with no frame drops) and gate CI.
- pre-approved-batch: yes
