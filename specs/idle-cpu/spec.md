# Spec: idle-cpu

## Meta
- **Created**: 2026-07-18
- **Type**: dev
- **Status**: planned (decompose batch — L2-reviewer PASS; execution owned by build-order)
- **Set**: post-pr2-backlog (`specs/post-pr2-backlog/partition-manifest.md`)

## Goal
Cut idle CPU on the mac-terminal. The continuous-redraw loop (kept for the
winit#2640 macOS compositor workaround) currently re-shapes the whole grid and
re-clones a full snapshot every frame even when nothing changed, burning ~65%
CPU at an idle prompt. Skip the per-frame re-shape and re-present cached buffers
whenever the visible grid is unchanged.

## Non-goals
- Removing or throttling the continuous-redraw loop (SD4 — regresses winit#2640).
- Any change to VT parsing, colors, or cell attributes (that is the sibling
  feature `vt-color-attrs`).
- Doc/known-gaps updates about this perf debt (owned by the sibling `housekeeping`).
- A new benchmark harness or `benches/` target (out of declared surface; the
  perf crate already owns `benches/`).

## Confirmed Goal
An idle terminal (shell at prompt, no PTY output) drops from ~65% CPU toward
idle. On each redraw the renderer skips `build_bg_instances` /
`build_glyph_instances` and re-presents resident GPU buffers whenever the
incoming `GridSnapshot` equals the last one rendered; the app avoids re-cloning
the grid into a fresh snapshot when the active grid did not advance. The redraw
cadence and occluded-pause behavior are unchanged, so a frame is always in
flight and content changes still appear within one frame.

## Declared Surface
- feature-id: idle-cpu
- depends_on: []
- declared-surface:
  - crates/renderer/src/lib.rs
  - crates/app/src/terminal_app.rs

## Research
- Continuous redraw is unconditional: `WindowEvent::RedrawRequested` always calls
  `inner.window.request_redraw()` when not occluded — the deliberate winit#2640
  workaround (`crates/app/src/terminal_app.rs:354-368`).
- The app re-clones the whole visible grid every frame: `let snapshot =
  inner.tabs.active().terminal.snapshot();` on each `RedrawRequested`
  (`crates/app/src/terminal_app.rs:355`).
- `Terminal::snapshot()` / `GridSnapshot::from_grid` allocate a row-major
  `Vec<Cell>` of length `cols*rows` each call (`crates/term-core/src/snapshot.rs:23,31`).
- `GridSnapshot` already `#[derive(Clone, Debug, PartialEq)]` — snapshot equality
  is available with no change to `term-core` (`crates/term-core/src/snapshot.rs:11`).
- `draw_to` re-shapes the entire grid every frame: `build_bg_instances(snapshot)`
  + `build_glyph_instances(&mut self.atlas, …, snapshot)`, then two
  `create_buffer_init` allocations (`bg-instances`, `glyph-instances`) — every
  frame, idle or not (`crates/renderer/src/lib.rs:570-594`).
- `build_glyph_instances` iterates all cells and does atlas lookups/rasterization;
  `build_bg_instances` iterates all cells (`crates/renderer/src/lib.rs:665-…`).
- The `Renderer` struct holds `device`, `queue`, `atlas`, pipelines and per-frame
  uniforms but no resident per-cell instance buffer and no last-snapshot
  (`crates/renderer/src/lib.rs:41-64`).
- The app already knows when content advanced: `PtyOutput` calls
  `s.drain()` which returns an `advanced: bool`, and only redraws when
  `advanced && is_active` (`crates/app/src/terminal_app.rs:288-293`).
- Registered `DrawLayer`s (tab bar, overlays) record every frame via `FrameCtx`;
  they are driven by their own change signals and are out of scope here
  (`crates/renderer/src/lib.rs:636-658`).
- Dependency rule: `term-core` must stay GPU/winit-free; the renderer only ever
  reads `GridSnapshot` (`.claude/rules/dependency-direction.md`,
  `docs/architecture.md:30-38`).

## Decisions

<!-- SD1–SD4 inherited verbatim from specs/post-pr2-backlog/partition-manifest.md -->
### SD1: Preserve the enforced crate dependency direction; `term-core` stays GPU/winit-free
- **Status**: resolved
- **Rationale**: The workspace graph `common ← term-core ← renderer ← app` is a hard
  invariant (`.claude/rules/dependency-direction.md`). New per-cell data (VT color +
  attributes) lives in `term-core` (`Cell`/`Color`/`parser`); the renderer only ever
  reads it through `GridSnapshot`. Rejected: pushing any wgpu/winit type into
  `term-core`, or letting the renderer see a live `Grid` — either would make the
  engine un-testable headlessly and break the rule.

### SD2: `GridSnapshot` is the versioned renderer contract AND the frame-equality basis
- **Status**: resolved
- **Rationale**: `GridSnapshot` already derives `PartialEq` and is the immutable
  contract the renderer consumes (never a live `Grid`). The idle-CPU fix keys its
  skip-reshape cache on **snapshot equality**; VT attributes are added as `Cell`
  fields, so `Cell: PartialEq`/`Eq` makes attribute-only changes invalidate that
  cache automatically. This is the load-bearing coordination point between the two
  code features. Rejected: a side-channel dirty flag in `app` (splits the source of
  truth from the snapshot), or comparing rendered text (misses color/attr-only
  changes).

### SD3: No GUI/widget crates, macOS-ARM64-only; attribute rendering is hand-rolled via `swash`
- **Status**: resolved
- **Rationale**: `no-gui-crates` + `macos-only` rules. bold/dim/italic/underline/
  reverse are drawn on the existing hand-rolled two-pass cell renderer — synthesized
  through `swash` (embolden/skew) or font faces, with underline/reverse composed in
  the instance builders. Rejected: any widget/text-layout toolkit (egui/iced/…) and
  any `#[cfg(target_os = "…")]` portability branch.

### SD4: Keep the continuous-redraw loop (winit#2640); cut idle CPU by making each idle frame cheap
- **Status**: resolved
- **Rationale**: On this wgpu/winit/macOS stack the compositor drops the presented
  frame the moment the run loop sleeps, so a frame must always be in flight (see the
  `RedrawRequested` comment in `terminal_app.rs`). The idle-CPU fix therefore keeps
  the continuous redraw and makes each idle frame cheap (skip re-shape, re-present
  cached buffers) — it does **not** disable redraws. VT-attribute work must not
  reintroduce unconditional per-frame reshaping. Rejected: on-demand / `ControlFlow::
  Wait`-only redraws (blank window) or dropping `request_redraw` (regresses #2640).

<!-- feature-local decisions -->
### D5: Snapshot-equality cache in the renderer — skip re-shape on a hit
- **Status**: resolved
- **Rationale**: Store the last-rendered `GridSnapshot` on `Renderer`. In `draw_to`,
  if the incoming snapshot `==` the cached one, skip `build_bg_instances` /
  `build_glyph_instances` entirely and re-present the resident buffers (D6);
  otherwise rebuild, then replace the cached snapshot. SD2 makes `GridSnapshot`'s
  derived `PartialEq` the correct, attribute-aware equality basis, and it already
  exists — no `term-core` change (SD1). Rejected: hashing the snapshot (collision
  risk and still O(cells) to hash), a per-cell diff (more complex than whole-buffer
  reuse for buffers this small), a dirty flag threaded from `app` (splits the source
  of truth, SD2).
- **Cache-invalidation caveat**: the snapshot equality basis is content-only, so any
  renderer-state change that alters shaping WITHOUT changing the cell content —
  theme/font/atlas/cell-metrics via `apply_config`, or viewport dims via `resize` —
  must force a miss by resetting `last_snapshot = None`. Both `apply_config` and
  `resize` already invalidate downstream GPU state (glyph cache / surface config);
  resetting the snapshot cache alongside them keeps a content-equal frame from
  masking a theme/size change (the D7×config-reload / D7×resize hazard the
  L2-reviewer surfaced).

### D6: Resident GPU vertex buffers reused across frames — allocation-free idle path
- **Status**: resolved
- **Rationale**: Keep the `bg` and `glyph` instance vertex buffers on `Renderer`
  instead of `create_buffer_init`-ing two buffers every frame. On a cache miss,
  re-upload the rebuilt instances (reallocating only when the instance count exceeds
  the resident capacity); on a cache hit, bind the resident buffers with no
  allocation. This is what makes the idle re-present genuinely cheap rather than
  merely skipping the CPU-side shaping. Rejected: `create_buffer_init` per frame
  (today's behavior — allocates even when idle); a persistent-mapped ring buffer
  (unneeded complexity at this grid size).

### D7: App re-snapshots on any real content change, not only on PTY advance
- **Status**: resolved
- **Rationale**: `RedrawRequested` fires continuously (SD4) but `terminal.snapshot()`
  clones the whole grid each call. Keep the last `GridSnapshot` plus a
  `snapshot_dirty` flag in `Running`. Set the flag in the existing `redraw()` method
  — which is *already* the documented "request a redraw for a real content change"
  entry point (its doc comment) and is called for PTY advance, tab switch/open/close,
  resize, config reload, and overlay/router redraws. The self-perpetuating continuous
  `window.request_redraw()` at the tail of `RedrawRequested` deliberately does NOT go
  through `redraw()`, so it does not set the flag. In `RedrawRequested`: if
  `snapshot_dirty` (or no snapshot cached yet) take a fresh `terminal.snapshot()`,
  cache it, and clear the flag; otherwise reuse the cached snapshot. The renderer then
  sees an equal snapshot on idle frames and takes the D5 cheap path.
- **Why not the narrow `drain()→advanced` signal**: that signal fires only on PTY
  output, so tab switch / resize / config reload (which call `redraw()` with no PTY
  drain) would reuse a stale cached snapshot and the D5 renderer cache would then
  freeze the wrong tab / old size / old theme on screen — a regression versus today's
  every-frame re-snapshot. Gating on `redraw()` (every real content change) instead of
  on `advanced` closes all three cases. `OverlayPending` and overlay/tab-bar redraws
  are safely covered too: they go through `redraw()`, so a fresh snapshot is taken (a
  harmless superset — those change only DrawLayers, but re-snapshotting is correct and
  cheap relative to the win). Rejected: re-snapshotting every frame (residual O(cells)
  clone even when idle); a narrow advance-only flag (the defect above); moving
  advance-tracking into the renderer (the app owns the grid/tab lifecycle, SD1).
- **Not the SD2-rejected dirty flag.** `snapshot_dirty` governs only whether the *app*
  re-clones the grid into a fresh `GridSnapshot`; it never feeds the *renderer's* skip
  decision, which still keys purely on `GridSnapshot ==` (D5). The dirty flag SD2/D5
  reject is a different mechanism — one that would make the renderer's skip depend on
  an app-side signal instead of snapshot equality. These do not conflict: the app may
  hand the renderer a fresh-but-equal snapshot, and the renderer still correctly takes
  the D5 cache hit.

## Constraints
- The continuous `request_redraw` cadence and the `Occluded` pause must remain
  behaviorally identical (SD4, winit#2640).
- No new dependency in `term-core`; no wgpu/winit type crosses into it (SD1).
- No new GUI/widget crate; macOS-ARM64 only, no `#[cfg(target_os)]` branch (SD3).
- Correctness parity: a cached frame must be pixel-identical to re-shaping the
  same snapshot — the cache invalidates on any `GridSnapshot` inequality.
- **No stale frame on non-PTY changes.** Tab switch, resize, and config reload
  must force a fresh snapshot (app, D7) AND a renderer cache miss (D5) — a
  content-equal snapshot must never mask a different active tab, viewport size, or
  theme/font. This is the load-bearing correctness constraint of the feature.
- Registered `DrawLayer`s keep recording every frame (their own change signals);
  this feature does not gate them.
- **Cache-key completeness (sibling contract, SD2).** The D5 cache is correct only
  while every render-affecting cell attribute lives inside `Cell` / `GridSnapshot`
  and `Cell`'s `PartialEq` stays field-complete (derived, not hand-written). The
  sibling `vt-color-attrs` must preserve this when it extends `Cell`; an attribute
  stored outside the snapshot, or a hand-rolled `PartialEq` that omits a field, would
  silently stale this cache.

## Known Gaps
- A blinking cursor or other time-driven grid effect (if added later) would need
  its own redraw trigger or its blink phase folded into `GridSnapshot`; not in
  scope here (today's cursor state travels in the snapshot, so it is covered by
  equality).
- A future text-**selection** or search highlight drawn inside the *terminal glyph
  pass* (rather than as a `DrawLayer` or a `GridSnapshot` field) would be frozen by
  the D5 cache. No selection exists in the codebase today; if one is added it must
  travel in `GridSnapshot` or invalidate the cache — flagged so it is not a silent
  trap.
- Idle-CPU numbers are measured manually (Activity Monitor / `top`); a `benches/`
  regression guard is deferred to the perf-harness crate (out of declared surface).

## Requirements

### R0: Idle terminal CPU drops toward idle with no visual regression
#### R0.1: Idle CPU falls well below the ~65% baseline
- **Given**: an idle terminal at the shell prompt with no PTY output, window visible
- **When**: the continuous-redraw loop runs for ≥10 s
- **Then**: measured process CPU is substantially below the ~65% baseline because
  neither the grid re-shape nor the snapshot clone runs on the idle frames
#### R0.2: The window still holds its content while idle
- **Given**: the idle CPU optimization is active
- **When**: the run loop sleeps between frames
- **Then**: a frame is still in flight (winit#2640 workaround intact) and the
  terminal content remains visible — no blanking

### R1: Renderer skips re-shape when the snapshot is unchanged
#### R1.1: Cache hit skips both instance builders
- **Given**: the renderer has drawn a frame from snapshot S and cached S
- **When**: `render()` is called with a snapshot S2 where `S2 == S`
- **Then**: `build_bg_instances` and `build_glyph_instances` are NOT called and the
  frame is drawn from the resident buffers built for S
#### R1.2: Cache miss rebuilds and replaces the cache
- **Given**: the renderer's cached snapshot is S
- **When**: `render()` is called with S2 where `S2 != S` (e.g. a new char printed)
- **Then**: instances are rebuilt from S2, the resident buffers are updated, the
  cached snapshot becomes S2, and the presented frame reflects S2
#### R1.3: First frame is always a miss
- **Given**: the renderer has never drawn (no cached snapshot)
- **When**: `render()` is called with snapshot S
- **Then**: instances are built from S and S is cached (no panic on the empty cache)
#### R1.4: Renderer state change forces a miss even on an equal snapshot
- **Given**: the renderer has cached snapshot S and resident buffers built for the
  old theme/font/size
- **When**: `apply_config` (theme/font/atlas/cell-metrics) or `resize` (viewport
  dims) runs, then `render()` is called with a snapshot content-equal to S
- **Then**: the snapshot cache is treated as a miss (reset), instances are rebuilt
  with the new config/size, and the change is visible on the very next frame — a
  content-equal snapshot does not re-present stale buffers

### R2: Resident GPU buffers replace per-frame allocation
#### R2.1: Cache hit allocates no vertex buffer
- **Given**: a cache hit for the terminal glyph pass
- **When**: `draw_to` records the pass
- **Then**: no `create_buffer_init` runs for that frame; the resident bg/glyph
  buffers are bound and drawn
#### R2.2: Buffers grow to fit a larger frame
- **Given**: resident buffers sized for the previous instance counts
- **When**: a cache miss produces more instances than the resident capacity (e.g.
  after a resize to a larger grid)
- **Then**: the buffers are reallocated to fit and every instance is drawn (no
  truncation, no stale tail)

### R3: App re-clones the grid only on a real content change (never a stale frame)
#### R3.1: No re-snapshot on an idle continuous frame
- **Given**: no real content change occurred since the last presented frame (only the
  self-perpetuating continuous `request_redraw` fired)
- **When**: `RedrawRequested` fires
- **Then**: `terminal.snapshot()` is NOT called; the cached `GridSnapshot` is passed
  to `render()`
#### R3.2: Fresh snapshot on PTY advance
- **Given**: a `PtyOutput` event advanced the active tab's grid (`redraw()` was called)
- **When**: the next `RedrawRequested` fires
- **Then**: a fresh snapshot is taken, cached in `Running`, passed to `render()`, and
  the new content appears on screen
#### R3.3: Fresh snapshot on tab switch
- **Given**: two tabs exist and tab B (idle, no PTY output) is not active
- **When**: the user switches the active tab to B (a `redraw()` with no PTY advance)
- **Then**: a fresh snapshot of tab B is taken and rendered — B's screen is shown
  immediately, not tab A's stale cached frame
#### R3.4: Fresh snapshot + renderer miss on resize
- **Given**: the terminal is idle at a given size
- **When**: the window is resized (`Resized` → `resize_all` → `redraw()`)
- **Then**: a fresh snapshot at the new dimensions is taken and the renderer rebuilds
  (R1.4) so the resized grid fills the new viewport with no stale-size frame
#### R3.5: Fresh snapshot + renderer miss on config reload
- **Given**: the terminal is idle and the config file is edited (theme/font change)
- **When**: `ConfigReloaded` applies the config (`apply_config` + `redraw()`) with the
  grid cells unchanged
- **Then**: a fresh snapshot is taken and the renderer treats it as a miss (R1.4), so
  the new theme/font is visible immediately rather than after the next PTY output

### R4: Redraw cadence and latency are preserved
#### R4.1: Continuous redraw stays while visible
- **Given**: the window is visible (not occluded)
- **When**: a frame is presented in `RedrawRequested`
- **Then**: `request_redraw` is still called so a frame is always in flight
#### R4.2: Occluded pause is unchanged
- **Given**: the window is occluded
- **When**: `RedrawRequested` handling completes
- **Then**: `request_redraw` is NOT called (the loop pauses to save power), identical
  to today
#### R4.3: Content-change latency is not increased
- **Given**: an idle terminal drawing from cached buffers
- **When**: the user types a key that produces shell output
- **Then**: the change is rendered within one frame of the grid advancing — no added
  latency versus the pre-optimization path

## Tasks

### T1: Renderer snapshot-equality cache + resident buffers + state-change invalidation [vertical]
- **Fulfills**: R0, R1 (incl. R1.4), R2
- **Depends on**: (none)   ← parallel with T2 (disjoint file: `renderer/src/lib.rs`)
- Add `last_snapshot: Option<GridSnapshot>` and resident `bg`/`glyph`
  `wgpu::Buffer` (+ counts/capacity) to `Renderer`. In `draw_to`, compare the
  incoming snapshot to `last_snapshot`; on a hit, bind the resident buffers and
  skip both instance builders; on a miss, rebuild instances, upload into the
  resident buffers (reallocating on growth), and store the snapshot. In
  `apply_config` and `resize`, reset `last_snapshot = None` so a content-equal
  snapshot cannot re-present stale theme/font/size buffers (R1.4). The render
  pass, layer recording, submit, and present are unchanged.

### T2: App caches the last snapshot; re-snapshots on every real content change [vertical]
- **Fulfills**: R0, R3, R4
- **Depends on**: (none)   ← parallel with T1 (disjoint file: `app/src/terminal_app.rs`)
- Hold the last `GridSnapshot` and a `snapshot_dirty` flag in `Running`. Set the
  flag inside the existing `redraw()` method (the "real content change" entry point
  already called for PTY advance, tab switch/open/close, resize, config reload, and
  overlay/router redraws). In `RedrawRequested`, take a fresh `terminal.snapshot()`
  when `snapshot_dirty` is set or no snapshot is cached (then cache it and clear the
  flag); otherwise pass the cached snapshot to `render()`. The self-perpetuating
  continuous `window.request_redraw()` at the tail stays a bare call (does not set the
  flag). Leave the `request_redraw` cadence and the `Occluded` pause exactly as they
  are. (R3.4/R3.5 resize/config correctness pairs with T1's R1.4 renderer miss —
  disjoint files, no shared interface, so still parallel.)

## External Dependencies

### Pre-work
- (none)

### Post-work
- Measure idle CPU before/after (Activity Monitor or `top -pid`) at a static shell
  prompt to confirm the ~65% → low-idle drop; this is the R0 acceptance evidence,
  verified in the execute pipeline.
