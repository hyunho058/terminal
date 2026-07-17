# Spec: ai-monitor

## Meta
- **Created**: 2026-07-17
- **Type**: dev
- **Status**: drafting
- **Set**: mac-terminal-v1 (`specs/mac-terminal-v1/partition-manifest.md`)

## Declared Surface
- feature-id: ai-monitor
- depends_on: [glyph-rendering, tabs]
- declared-surface:
  - crates/ai-monitor/**
  - crates/renderer/src/overlay_layer.rs
  - crates/app/src/overlay.rs

## Goal
Rewrite ai-monitor natively in Rust as a toggle overlay: a hotkey shows/hides a translucent overlay that reads the active session's AI logs, parses the events, and renders them in the terminal's own UI — not by shelling out to a TUI. **v1 targets Claude Code JSONL logs (`~/.claude/projects/**/*.jsonl`); Gemini CLI logs are best-effort/next** (its format is less documented — see Known Gaps).

## Non-goals
- Being an AI client / sending prompts — read-only observation of logs
- Watching sessions other than the currently active tab's
- A separate window or process (it is an in-app overlay)
- Historical analytics / persistence beyond what the log files contain

## Confirmed Goal
A new `ai-monitor` crate tails the active session's Claude/Gemini JSONL logs on a background thread (SD6), and an app overlay (registered via the SD4 seam, drawn via an SD3 `DrawLayer`) toggles with a hotkey to render the latest events natively over the terminal.

## Research
- `insights.md`: ai-monitor is a native integration (not just launching a TUI) — the terminal reads session logs directly and renders in its own UI; scope is the active tab's session only.
- Claude Code writes session transcripts to `~/.claude/projects/<encoded-cwd>/<session>.jsonl` (one JSON object per line) — the pattern this feature tails.
- `tabs` provides the SD4 seam (`InputHandler` for the toggle key, `Overlay` registry) and knows the active tab; `glyph-rendering` provides the `DrawLayer` seam for drawing the overlay.
- Concurrency is threads + channels + `EventLoopProxy` wake (SD6) — the log watcher mirrors the PTY reader thread pattern.

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
### D1: `ai-monitor` is a UI-free crate: locate + backfill + tail + parse
- **Status**: resolved
- **Rationale**: The crate exposes `LogWatcher` and a typed `AiEvent` model (role, text summary, tool calls, timestamps) parsed with serde. On start it **reads the existing tail of the log first** (the last N events already in the file) — because `notify`/FSEvents only signals *future* appends and delivers no pre-existing content — then tails new appends. No GPU/UI (SD2), headless-testable. Rejected: tail-only (the overlay would be empty until the next new event — R0.1 needs the already-present recent events), parsing in `app` (untestable), rendering from the crate (violates SD2/SD3).

### D2: Session→log resolution from the active tab's cwd
- **Status**: resolved
- **Rationale**: Map the active tab's working directory to Claude's encoded `~/.claude/projects/<encoded-cwd>/` dir and pick the newest `*.jsonl`; Gemini logs resolved analogously. The active tab is provided by `tabs`. Rejected: watching all sessions (scope is active-only per `insights.md`), a fixed path (breaks per-project logs).

### D3: A background watcher thread tails via notify + wakes the loop (SD6)
- **Status**: resolved
- **Rationale**: A watcher thread uses filesystem notification (`notify`) on the log file, parses new lines into `AiEvent`s, sends them over a channel, and wakes the event loop via `EventLoopProxy` — mirroring the PTY reader thread. Rejected: polling on a timer (latency vs CPU tradeoff worse), async runtime (SD6 says threads).

### D4: Overlay = registered `Overlay` (SD4) + `overlay_layer.rs` DrawLayer (SD3), hotkey Cmd+Shift+M
- **Status**: resolved
- **Rationale**: `app/src/overlay.rs` registers an `Overlay` + a Cmd+Shift+M `InputHandler` toggle through the `tabs` seam; when visible, `renderer/src/overlay_layer.rs` draws a translucent panel of recent `AiEvent`s. Only the active tab's events show. Rejected: editing `terminal_app.rs` (SD4 collision), always-visible panel (the requirement is a toggle overlay).

### D5: On active-tab switch, re-point the watcher
- **Status**: resolved
- **Rationale**: When the overlay is open, switching tabs re-resolves the log path (D2), backfills (D1), and restarts the watcher, so the overlay always reflects the *active* session. Rejected: one watcher for the first session (stale after switching — contradicts "active session" scope).

### D6: Watcher lifecycle is visibility-gated
- **Status**: resolved
- **Rationale**: The watcher (and its background thread) runs only **while the overlay is visible**: opening it starts the watcher (backfill + tail); hiding it stops the watcher. This bounds cost (no filesystem watching or parsing when the overlay is closed) and guarantees that what the user sees on open is fresh (backfilled), not a stale accumulation. Re-opening re-backfills. Rejected: always-on watcher (wastes CPU/handles while hidden, and D5's "restart on switch" would fire even when closed), accumulate-while-hidden (unbounded buffer + stale-vs-fresh ambiguity the reviewer flagged).

## Constraints
- C1: `ai-monitor` crate is UI/GPU-free (SD2); depends only on `common` + serde + `notify` + `image`? no — logs only.
- C2: Overlay draws via the `DrawLayer` seam (SD3) and registers via the SD4 seam — no edits to `renderer/src/lib.rs` or `terminal_app.rs`.
- C3: Read-only: never writes to the log files or the PTY.
- C4: A missing/permission-denied log path degrades gracefully (empty overlay + a notice), never panics.

## Known Gaps
- Gemini CLI log format/location less documented than Claude's — exact schema confirmed during implementation; Claude JSONL is the primary target for v1.
- Overlay layout (how many events, wrapping, scroll within overlay) is a first cut.

## Requirements

### R0: Toggle overlay shows active-session AI events
#### R0.1: Hotkey reveals recent events
- **Given**: The active tab is in a directory with a Claude session log that has 3 recent events
- **When**: The user presses Cmd+Shift+M
- **Then**: A translucent overlay appears listing those 3 events (role + summary), newest last

### R1: Log watching (D1, D2, D3)
#### R1.1: Resolve the active session log
- **Given**: The active tab's cwd is `/x/y`
- **When**: The watcher starts
- **Then**: It selects the newest `*.jsonl` under `~/.claude/projects/<encoded-/x/y>/`
#### R1.2: Backfill existing events on open
- **Given**: The active session log already contains 3 events and the overlay was closed
- **When**: The user opens the overlay (watcher starts)
- **Then**: The last N (≥3) existing events are read from the file and shown immediately — before any new append occurs
#### R1.3: Tail new events
- **Given**: The overlay is open and the log gains a new line
- **When**: The watcher notices the append
- **Then**: The new line is parsed into an `AiEvent`, sent over the channel, the loop wakes, and the overlay shows the new event
#### R1.4: Read-only (C3)
- **Given**: The watcher is running against a session log
- **When**: It backfills and tails
- **Then**: It only opens the log for reading — no writes to the log file, and no bytes are ever written to the PTY
#### R1.5: Malformed / missing log is safe
- **Given**: The resolved log path does not exist or a line is not valid JSON
- **When**: The watcher runs
- **Then**: It skips the bad line / shows an empty overlay with a notice — no panic

### R2: Overlay presentation (D4, SD3/SD4)
#### R2.1: Toggle visibility
- **Given**: The overlay is hidden
- **When**: Cmd+Shift+M is pressed twice
- **Then**: It shows, then hides; the toggle `InputHandler` returns `Handled` both times (no bytes to the shell)
#### R2.2: Drawn via a registered layer
- **Given**: The overlay is visible
- **When**: A frame renders
- **Then**: The panel is drawn by the registered `overlay_layer` `DrawLayer` over the grid, without editing `renderer/src/lib.rs`

### R3: Follows the active tab (D5)
#### R3.1: Switching tabs re-points the watcher
- **Given**: The overlay is open showing tab 1's session
- **When**: The user switches to tab 2 (different cwd/session)
- **Then**: The watcher re-resolves to tab 2's log and the overlay shows tab 2's events

## Tasks

### T1: ai-monitor crate — LogWatcher + AiEvent parsing [infra]
- **Fulfills**: R1
- **Depends on**: (none)

### T2: Overlay DrawLayer (renderer) [vertical]
- **Fulfills**: R2.2
- **Depends on**: T1

### T3: App overlay module — visibility-gated watcher lifecycle + toggle via SD4 seam [vertical]
- **Fulfills**: R0, R2.1, R3
- **Depends on**: T1, T2

## External Dependencies
### Pre-work
- `glyph-rendering` `DrawLayer` seam + `tabs` SD4 seam and active-tab accessor (declared dependencies).
- New crate `crates/ai-monitor` added to the workspace; `notify` + `serde_json` deps.
### Post-work
- (none)
