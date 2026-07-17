# Spec: scrollback

## Meta
- **Created**: 2026-07-17
- **Type**: dev
- **Status**: drafting
- **Set**: mac-terminal-v1 (`specs/mac-terminal-v1/partition-manifest.md`)

## Declared Surface
- feature-id: scrollback
- depends_on: []
- declared-surface:
  - crates/term-core/src/grid.rs
  - crates/term-core/src/snapshot.rs
  - crates/term-core/src/scrollback.rs
  - crates/term-core/src/parser.rs
  - crates/term-core/tests/scrollback.rs

## Goal
Add a scrollback history buffer to `term-core`: lines that scroll off the top are retained in a ring buffer, and a viewport offset lets the user scroll up/down through history — working the same whether the local shell is bare zsh or running tmux (which paints a full-screen alternate buffer). The snapshot exposes the offset so the renderer draws the scrolled viewport with no renderer changes.

## Non-goals
- Rendering / input handling (renderer reads the offset; the app binds scroll keys — those live in `glyph-rendering` and `tabs`/app respectively)
- Search-in-scrollback, reflow-on-resize of history lines (deferred)
- Alternate-screen semantics beyond retaining primary-screen history

## Confirmed Goal
`term-core` retains scrolled-off lines in a bounded ring buffer and supports a viewport offset (`scroll_up(n)`/`scroll_down(n)`/`scroll_to_bottom`); `GridSnapshot` carries the visible window for whatever offset is active, so scrolling works even inside a tmux session, up to a configurable history cap (target: 100k lines) without content loss.

## Research
- `Grid` is a flat `cols*rows` cell vec with a cursor; `scroll_up()` currently discards the top line (`crates/term-core/src/grid.rs`) — no history is kept.
- `GridSnapshot::from_grid` clones the visible cells + cursor (`crates/term-core/src/snapshot.rs`); it is the only renderer contract (SD5).
- Snapshot tests use `insta` over `to_text()` (`crates/term-core/tests/grid_snapshot.rs`) — the pattern to reuse.
- tmux repaints a full alternate screen; the pain point (from `insights.md`) is that scroll doesn't work there — so scroll must operate on our retained history, independent of what the app draws.

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
### D1: History as a bounded `VecDeque<Row>` ring buffer — PRIMARY screen only
- **Status**: resolved
- **Rationale**: On `line_feed` at the bottom **while the primary screen is active** (see D5), push the departing top row into a `VecDeque` capped at `history_limit` (default 100k); popping the front when full is O(1). The primary-screen condition is load-bearing: alt-screen `line_feed`s (tmux/vim full-screen repaints) MUST NOT enter history or they pollute it with redraw frames. Rejected: unconditional push (tmux frames flood history), growing `Vec` (unbounded memory), rope/gap buffer (over-engineered).

### D2: Viewport offset counts rows above the live bottom; auto-increments to pin content
- **Status**: resolved
- **Rationale**: `view_offset: usize` = rows scrolled up from the live view (0 = live). `scroll_up(n)` clamps to `history.len()`. To keep the user pinned to the *same content* when new output arrives while `offset>0`, `view_offset` **auto-increments by the number of new lines** (so the viewport shows the same rows, not a shifting window) until `scroll_to_bottom`. If the pinned content is **evicted** (history full at cap and the viewed rows fall off the front), `view_offset` clamps to the oldest available row and sets an `at_top` flag rather than showing garbage. Rejected: fixed rows-from-bottom without auto-increment (content shifts under the user on every new line), auto-follow-on-output (loses place — the tmux pain point), panicking on eviction.

### D3: Snapshot composes history + live rows for the current offset
- **Status**: resolved
- **Rationale**: `GridSnapshot::from_grid` fills the visible window from `history` (for the top `view_offset` rows) then live cells, and carries `view_offset` + `at_bottom` so the renderer/app can show a scroll indicator — a side-channel field, not new cell semantics (SD5). Rejected: a separate snapshot type per mode (renderer would need two paths).

### D4: Scroll is viewport-only; the shell's own screen is untouched
- **Status**: resolved
- **Rationale**: Scrolling changes only what we display from our retained buffer; it never writes to the PTY or moves the shell cursor. Rejected: forwarding scroll to the shell (tmux's copy-mode — the exact behavior users found broken in Warp).

### D5: Alt-screen (DECSET ?1049h) is tracked; scrollback shows frozen primary history while it is active
- **Status**: resolved
- **Rationale**: `term-core` tracks alternate-screen state via the `?1049h`/`?1049l` (and `?47`) private modes parsed by `vte`. While the alt screen is active (tmux, vim, less): (a) the alt screen has its OWN non-scrolling buffer per the VT spec — no history is captured from it (D1); (b) `scroll_up` reveals the **frozen primary-screen history** captured before the program took over — so the user can still scroll back to the pre-tmux shell output, which is the concrete win over Warp (where tmux scroll did nothing at all). This makes explicit *what the buffer contains* under tmux, not just that we don't forward to copy-mode. On `?1049l` (program exits) the primary screen + its live scrollback resume. Rejected: capturing alt-screen frames into history (redraw pollution), disabling scroll entirely under alt-screen (throws away the reachable primary history — the very thing users wanted).

## Constraints
- C1: `term-core` stays GPU/UI-free — SD2. No renderer/winit references.
- C2: History memory bounded by `history_limit` — a full 100k buffer must not OOM (rows stored compactly).
- C3: Existing `grid_snapshot.rs` tests must still pass (offset 0 == today's behavior).

## Known Gaps
- `history_limit` default (100k candidate) and per-row memory budget to be validated under `performance-harness`.
- History reflow on column-count change is out of scope for v1 (resized history may wrap differently).
- ~~Cross-feature coordination on `parser.rs`~~ **RESOLVED:** `scrollback` now owns `crates/term-core/src/parser.rs` (it owns the term-core screen-state group: grid + snapshot + scrollback + VT dispatch, of which alt-screen `?1049h/l` is a natural part). `image-paste` extends `parser.rs` (Kitty image escapes) and `snapshot.rs` (image placements) as an **ordered** share via its `depends_on: scrollback` edge — no unordered collision.

## Requirements

### R0: Scroll through retained history
#### R0.1: Scroll up reveals prior output
- **Given**: 200 lines have been printed into a 30-row grid (170 scrolled off, retained)
- **When**: `scroll_up(10)` then `snapshot()`
- **Then**: The snapshot's top rows show lines from history (10 rows earlier), `view_offset == 10`, `at_bottom == false`

### R1: History ring buffer (D1)
#### R1.1: Scrolled-off primary-screen lines are retained
- **Given**: The primary screen is active and a full grid receives one more `line_feed`
- **When**: The top row departs the live view
- **Then**: That row is pushed to history; `history.len()` increases by 1
#### R1.2: History is bounded
- **Given**: `history_limit = N` and history is full
- **When**: Another row is pushed
- **Then**: The oldest row is dropped; `history.len() == N` (no unbounded growth)

### R2: Viewport offset (D2)
#### R2.1: Clamp at the top
- **Given**: History has 5 rows and `view_offset == 5`
- **When**: `scroll_up(10)`
- **Then**: `view_offset` stays 5 (clamped, no panic)
#### R2.2: New output preserves the pinned content (auto-increment)
- **Given**: `view_offset == 10` and the viewport shows a specific set of history rows
- **When**: The shell emits 3 new lines
- **Then**: `view_offset` becomes 13 so the viewport shows the SAME rows (content does not shift under the user); `at_bottom == false`
#### R2.3: Jump to bottom
- **Given**: `view_offset == 10`
- **When**: `scroll_to_bottom()`
- **Then**: `view_offset == 0`, `at_bottom == true`, snapshot shows the live view
#### R2.4: Pinned content evicted clamps to top
- **Given**: History is at its cap and the user is scrolled near the oldest rows
- **When**: Enough new lines arrive that the viewed rows are evicted from the front
- **Then**: `view_offset` clamps to the oldest available row and `at_top == true` — the snapshot shows the oldest retained content, never garbage or a panic

### R5: Works under tmux / alt-screen (D5)
#### R5.1: Alt-screen output is not captured
- **Given**: A program sends `?1049h` (enter alt screen) and then repaints full-screen frames
- **When**: Those frames scroll internally
- **Then**: `history.len()` does not grow from the alt-screen output (no redraw pollution)
#### R5.2: Scrollback reveals frozen primary history under tmux
- **Given**: 100 primary-screen lines were printed, then a program entered the alt screen
- **When**: The user `scroll_up`s while the alt screen is active
- **Then**: The snapshot shows the retained pre-alt-screen primary history (the reachable content Warp could not scroll to)
#### R5.3: Exiting alt-screen resumes primary scrollback
- **Given**: The alt screen is active
- **When**: The program sends `?1049l`
- **Then**: The primary screen and its live scrollback resume; `scroll_up` again reflects ongoing shell output

### R3: Snapshot composition (D3, SD5)
#### R3.1: Offset snapshot mixes history and live rows
- **Given**: `view_offset == 3` on a 5-row grid with ≥3 history rows
- **When**: `snapshot()`
- **Then**: Top 3 rows are history rows, bottom 2 are live cells, in order; `view_offset == 3` is carried on the snapshot
#### R3.2: Offset 0 is unchanged behavior
- **Given**: `view_offset == 0`
- **When**: `snapshot()`
- **Then**: Output is byte-identical to the pre-scrollback snapshot (existing `grid_snapshot.rs` tests pass)

### R4: Works under tmux (D4)
#### R4.1: Scroll never writes to the shell
- **Given**: The grid is at `view_offset == 0`
- **When**: `scroll_up(5)` is called
- **Then**: No bytes are written toward the PTY and the grid cursor position is unchanged (scrolling is display-only)

## Tasks

### T1: Ring-buffer history in Grid [infra]
- **Fulfills**: R1
- **Depends on**: (none)

### T2: Viewport offset API (scroll_up/down/to_bottom) [vertical]
- **Fulfills**: R2, R4
- **Depends on**: T1

### T3: Snapshot composition with offset + fields (view_offset, at_bottom, at_top) [vertical]
- **Fulfills**: R0, R2.4, R3
- **Depends on**: T2

### T4: Alt-screen dispatch (`?1049h/l` in parser.rs) + tracking + primary-only history gating [vertical]
- **Fulfills**: R5
- **Depends on**: T1

### T5: insta scrollback tests (scroll, eviction, alt-screen) [vertical]
- **Fulfills**: R0.1, R2, R5
- **Depends on**: T3, T4

## External Dependencies
### Pre-work
- (none)
### Post-work
- `tabs`/app binds scroll keybindings to the new API; `glyph-rendering` renderer reads `view_offset` (no code dependency — the snapshot field carries it).
