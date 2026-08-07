# Spec: ai-monitor-right-panel

## Meta
- **Created**: 2026-08-06
- **Type**: dev
- **Status**: approved
- **Approved by**: user
- **Approved at**: 2026-08-07
- **Depends on**: `specs/grid-reflow/spec.md`
- **Design source**: `Terminal + AI Monitor.dc.html` (claude.ai/design project `e4cae043-8c6e-4800-a21c-e561205322be`)

## Goal
Change the existing ai-monitor feature's design to match `Terminal + AI Monitor.dc.html`:
move it from a full-width, bottom-anchored translucent text overlay to a **right-docked,
drag-resizable panel**, and replace the single-colour line list with the design's visual
structure — header + status pill, stat grid, section cards, coloured verb columns, and
usage bars.

## Non-goals
- Scrolling or click interaction *inside* the panel (the design shows `overflow:auto`; the
  panel renders a composed viewport, and pointer handling stays out of scope).
- Any change to what ai-monitor reads or how it resolves a session — the log resolver,
  watcher lifecycle, rival set, and attachment rule are untouched.
- The left tab sidebar's own design and the terminal body's rendering.
- Reflow-on-resize itself: this feature *requires* it and *depends on* it, but the mechanism
  is specified and built in `specs/grid-reflow/spec.md` (D22). No `term-core` file is touched
  by this spec.
- Cross-platform or non-Metal rendering paths (`macos-only`).
- A GUI/widget crate to lay the panel out (`no-gui-crates`) — layout stays hand-rolled.

## Confirmed Goal
The ai-monitor becomes a panel docked to the **right edge** of the window that **reserves
columns** from the terminal grid rather than covering it: opening it reflows the terminal to
the remaining width, and dragging the panel's left edge resizes both live. Its contents are
drawn from the design's ten regions (header, status pill, session identity, 3x2 stat grid
with a context gauge, TASKS, ACTIVE AGENTS, SKILLS, FILE ACTIVITY, RECENT CALLS, TOOL USAGE,
footer), using per-run colour and shape primitives instead of one monochrome text colour.

Two consequences follow from the request rather than being stated in it, and both are in
scope:

1. **Docking is a renderer change, not a paint change.** The design shows the terminal
   column shrinking (`flex:1;min-width:520px`) with an 8px drag handle between it and the
   panel. Reserving that band means a `right_inset` in the renderer's grid arithmetic,
   symmetric with the existing `left_inset` the tab sidebar already uses.
2. **Reflow must become non-destructive first — in a separate feature.** `Grid::resize`
   truncates rather than rewraps, so docking-as-specified would destroy the right-hand side
   of on-screen output every time the panel is toggled. That is a prerequisite, but its
   mechanism belongs to `term-core` and has its own six open questions, so it is specified as
   `specs/grid-reflow/spec.md` and declared a dependency (D22).
3. **The width-blindness premise disappears.** `crates/app/src/dashboard.rs` is built on the
   app not knowing the panel's width — hence `RULE_COLS = 104` and "open-right" rules that
   simply end when clipped. A docked panel has a known width, so layout responsibility moves
   from the app's fixed-column string assembly to the renderer, which already knows the
   pixels. This is a re-architecture of the presentation seam, not a re-skin.

### Done when
- `Cmd+Shift+M` opens a right-docked panel; the terminal reflows to the remaining columns
  and the PTY is resized to match.
- Dragging the panel's left edge resizes it live, clamped, and the terminal reflows with it.
- All ten design regions render from real telemetry, with the design's colour roles.
- Toggling the panel is non-destructive: a line wider than the narrowed grid re-wraps rather
  than truncating, and re-widening restores it — verified headlessly in `term-core`.
- `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings` pass.

### Open ambiguities carried into L2
- **Pulse animation.** The design animates the CONNECTED dot, the agent ring, and the footer
  dot. The event loop is `ControlFlow::Wait` (event-driven, no free-running frames), and the
  renderer holds an explicit idle-CPU guarantee (G1). Animation requires a repaint cadence
  that directly contests that guarantee.
- **Absolute timestamps.** The design shows SKILLS entries as `at 16:21:18`.
  `dashboard.rs:262` documents a deliberate rejection of wall-clock display: `std` alone
  cannot resolve a local timezone, and a UTC clock on a local-session dashboard was judged
  worse than no clock.
- **Data the snapshot does not carry.** `TelemetrySnapshot` has no cumulative per-tool call
  counts (TOOL USAGE shows 187 across 11 tools with bars) and no failed-tool-call counter
  (the design's `ERRORS` tile); its `parse_errors` family counts *parser* faults, which is a
  different fact.

## Research

### What the design actually specifies (right panel)
- Panel `width:430px` default, drag-clamped `300..760`, bg `#232320`, left border `#3a3934`;
  an 8px `col-resize` handle sits **between** the terminal column and the panel
  (`Terminal + AI Monitor.dc.html`, the `{{ startResize }}` div).
- Toggle `⌘M` / `Ctrl+M`; `Escape` or `q` closes; header `✕` closes (the `.dc.html` script's
  `componentDidMount` key handler).
- Ten regions in order: header (3px accent + title + CONNECTED pill + ✕), session identity
  (`ai-monitor` + 8-char id + `[Claude]` + model), 3x2 stat grid (UPTIME / IDLE / ERRORS /
  IN·OUT / CACHE / CONTEXT-with-gauge), TASKS, ACTIVE AGENTS, SKILLS, FILE ACTIVITY,
  RECENT CALLS, TOOL USAGE, footer ("Streaming live · updated 6s ago" + `q` keycap), and a
  bottom status bar showing the live width + "drag edge to resize".
- Section headings are `LABEL ──────── N`: label left, 1px hairline filling, count right.
- Colour roles used: accent green `#7cb87c`/`#8fcb8f`, live-lime `#a9c94a`, teal `#6fb3b8`
  (READ / cache / bars), amber `#d8a35c` (active agent, slow call), magenta `#c98bb0`
  (WRITE), rose `#d9647a` (failure), text `#e7e2d8` / `#d3cec4` / `#a8a297` / `#6d6a62`,
  card `#282824`, hairline `#34332e`.

### Current implementation — what has to change
- **Bottom overlay, not a dock.** `panel_rect()` returns `[0, vh*0.45, vw, vh-top]` — the
  lower 55% at full width (`crates/renderer/src/overlay_layer.rs:48`).
- **One text colour, no shapes.** `TEXT_FG: u32 = 0xE6E1CF` is the only colour, applied to
  every glyph (`overlay_layer.rs:43,318`); the model is `OverlayModel { visible: bool,
  lines: Vec<String> }` (`overlay_layer.rs:23`) and the only rect drawn is the panel
  background (`overlay_layer.rs:349`).
- **App composes fixed-width strings.** `dashboard::compose` returns `Vec<String>` built
  against `RULE_COLS = 104` (`crates/app/src/dashboard.rs:29,144`); the module header states
  the app is deliberately width- and height-blind and that sections are "open-right" so a
  clip is inconspicuous (`dashboard.rs:8-19`).
- **Tail-keeping is the degradation order.** The layer draws the *last* `max_lines`
  (`overlay_layer.rs:314`), which is why `compose` emits the summary last
  (`dashboard.rs:138-143`). A docked panel with a real height budget changes this contract.
- **Toggle is `Cmd+Shift+M`,** not the design's `Cmd+M` (`crates/app/src/overlay.rs:98`).

### Existing machinery this can reuse (the important finding)
- **`left_inset` is the exact structural precedent.** `grid_dims(width, height, cell,
  top_inset, left_inset)` computes `usable_w = width - left_inset`
  (`crates/renderer/src/lib.rs:1410-1422`); `grid_size()` reports it
  (`lib.rs:510`); `FrameCtx.left_inset` hands it to layers (`lib.rs:131`). A `right_inset`
  is the same subtraction and needs **no shader change**: the grid is laid out
  left-to-right from `left_inset`, so reserving a right band only means fewer columns.
  (`BgUniforms`/`GlyphUniforms` carry `left_inset` because the grid is *shifted*; nothing is
  shifted by a right band.)
- **`SidebarShapePipeline` already draws every shape the design needs.**
  `KIND_ROUNDED_BOX` (cards, radius 5, with border), `KIND_CAPSULE` (the CONNECTED pill, the
  progress/usage bars, the accent bars), `KIND_RING` (the agent dot), with solid or 3-stop
  gradient fills (`crates/renderer/src/sidebar_shapes.rs:24-28,114-210`). It is consumed via
  `SidebarShapePipeline::ensure(&mut slot, ctx)` + `.draw(ctx, &shapes)`
  (`sidebar_shapes.rs:391-416`) and is not sidebar-specific in anything but its name.
- **Model → layout → layer is an established split.** `crates/app/src/sidebar_model.rs`
  produces semantic `RowMeta`; `crates/renderer/src/sidebar_layout.rs` does the pixel layout
  (`row_rect`, `header_rect`, `list_viewport`, …); `sidebar_row.rs` paints. This is exactly
  the seam the panel needs so the app stays pixel-free and the renderer owns width.
- **Drag-resize has a complete precedent.** `TerminalApp::drive_resize`
  (`crates/app/src/terminal_app.rs:1305`) sets the width first, refits, then re-clamps —
  and critically **only refits when the column count actually changed**
  (`input::needs_refit`), because `Grid::resize` reallocates cells, clears the selection and
  fires SIGWINCH. `sidebar_layout::GRAB_BAND_PX = 6.0` + `grab_band_rect` + `hit_test`
  (`sidebar_layout.rs:543-621`) are the hit-test pattern; `input::resize_grab_offset` /
  `resize_raw_width` / `clamp_sidebar_width` / `next_collapsed`
  (`crates/app/src/input.rs:259-311`) are the pure gesture arithmetic.
- **Grid reflow is one call.** `settings::refit(&renderer, &mut tabs)` reads `grid_size()`
  and calls `tabs.resize_all(cols, rows)` (`crates/app/src/settings.rs:31`).
- **Shared text helpers.** `char_cols` (unicode-width, `프` = 2), `truncate_cols`,
  `pen_layout`, `layout_line` in `crates/renderer/src/tab_bar.rs:30-102` — `pub(crate)`, so
  a new renderer module can use them directly. The overlay layer already routes through
  them so Hangul does not overrun (`overlay_layer.rs:297-329`).
- **Resident instance buffers.** `upload_resident` (`lib.rs:923`) is the grow-only
  buffer helper the shape pipeline uses; avoids a per-frame `create_buffer_init`.

### Data gaps in `TelemetrySnapshot`
- Stores are **rings with caps**, not cumulative: `TOOLS_CAP = 32`, `SKILLS_CAP = 16`,
  `FILES_CAP = 32`, `SUBAGENTS_CAP = 32` (`crates/ai-monitor/src/telemetry.rs:33-41`). The
  design's TOOL USAGE shows a 187-call total across 11 tool names with proportional bars —
  that is a cumulative `name → count` map the crate does not keep.
- `ok: bool` exists per `CompletedTool` / `FileActivity` (`telemetry.rs:97,119`, folded from
  `is_error` at `telemetry.rs:531`), but only for the 32 entries still in the ring, so the
  design's `ERRORS` tile needs a cumulative failure counter.
- `parse_errors` / `usage_conflicts` / `orphan_tool_results` / … are **parser-health**
  counters, a different fact from "a tool call failed" — using them for the ERRORS tile
  would be a false label.
- Everything else the design shows already exists: `session_id`, `model`, `total_input`,
  `total_output`, `total_cache_read` + `total_cache_creation`, `context_used`,
  `first_event_ms`, `last_event_ms`, plus the five ring stores.

### Constraints from the project's own rules and docs
- `no-gui-crates`: layout stays hand-rolled on wgpu; the shape pipeline + glyph atlas are
  the only drawing primitives (`.claude/rules/no-gui-crates.md`).
- `dependency-direction`: `term-core` stays GPU-free; the panel model must live in
  `renderer` (or `common`) and be filled by `app`, never pull wgpu into `term-core`
  (`.claude/rules/dependency-direction.md`).
- **G1 idle-CPU is an explicit, tested guarantee**: `docs/architecture.md` records the
  renderer skipping re-shape on an unchanged snapshot, and `overlay_layer.rs:674-724`
  asserts "4 frames on one atlas must build the bind group exactly once". The event loop
  waits (`ControlFlow::Wait`, `docs/architecture.md` Runtime model C6). Animation contests
  this directly.
- Config already has `[ai_monitor]` with `context_window`
  (`crates/common/src/config.rs:26,32`) — the natural home for a persisted panel width,
  mirroring how the sidebar persists `width_px` / `collapsed`.
- Snapshot tests to migrate: 9 `insta` snapshots under `crates/app/src/snapshots/` plus the
  section-level `boundary_*` tests, all asserting `Vec<String>` joins
  (`dashboard.rs:941-1142`). The `/insta-review` project skill exists for accepting these.

## Decisions

### D1: `Cmd+Shift+M` is the **only** key the panel consumes — no `Esc`, no `q`
- **Status**: resolved
- **Rationale**: The design opens with `Cmd+M` and closes with `Esc` or `q`; all three are
  rejected. `Cmd+M` collides with the macOS window-minimise convention. `q` is ordinary
  terminal input (it quits `less`, pages out of `git log`, and is a literal character in an
  editor). `Esc` is worse than `q`: the panel never takes focus — it is a painted band, not
  a focusable view — so a panel-level `Esc` handler fires while the user is in `vim`'s
  insert mode, leaving them stuck in insert while the panel closes underneath them. One
  chord that toggles both ways is the only rule under which **no terminal keystroke is ever
  intercepted**, which is the invariant that matters for a terminal. This also keeps the
  established binding (`crates/app/src/overlay.rs:98`) and existing muscle memory.
  **Rejected**: `Cmd+M` (minimise collision), `q` (steals input), `Esc` unconditionally
  (breaks `vim`), and `Esc` gated on alt-screen (behaviour that changes per foreground app
  makes the key unpredictable, which is worse than not having it). The design's `q` keycap
  is decoration and is dropped rather than drawn as a promise the panel does not keep.

### D2: No animation — the panel is fully static
- **Status**: resolved
- **Rationale**: The design pulses the CONNECTED dot, the agent ring, and the footer dot. The
  event loop waits (`ControlFlow::Wait`, `docs/architecture.md` C6) and the renderer holds a
  tested idle-CPU guarantee (G1; `overlay_layer.rs:674-724` asserts zero rebuilds across
  4 idle frames). Liveness is carried by **colour**, not motion — lime for a live session,
  grey for a stopped one. **Rejected**: a ~2Hz repaint timer (breaks G1 and would require
  rewriting that guarantee in `docs/architecture.md`), and blinking only on snapshot arrival
  (works, but produces a single-frame flash the user will usually miss, so it buys the cost
  of a mechanism without buying the signal).

### D3: TOOL USAGE and ERRORS read **cumulative session counters** added to `ai-monitor`
- **Status**: resolved
- **Rationale**: The design's TOOL USAGE bars are proportions of a 187-call session total and
  its ERRORS tile is a session failure count. The crate keeps only rings — `TOOLS_CAP = 32`,
  `FILES_CAP = 32` (`crates/ai-monitor/src/telemetry.rs:33-41`) — so on a 3-hour session
  everything older than the last 32 calls is gone. Counting the ring would print a number
  that *looks* cumulative and is not. So `SessionTelemetry` gains a `name -> count` tally
  and a failed-call counter, folded at the same point `ok` is already derived
  (`telemetry.rs:531`). Additive only: the rings, their caps, and every existing field keep
  their present behaviour, so no current test changes meaning.
  **Rejected**: counting the 32-entry ring (a false total; it would force the heading to
  read "recent 32", which is not what the design communicates), and dropping the two regions
  from scope (they are 2 of the 10 regions the confirmed goal names).

### D4: The **terminal** holds a floor of 20 columns; the panel yields, then closes
- **Status**: resolved
- **Rationale**: With the sidebar (340px) and the panel (430px) both open, a narrowed window
  runs out of width. The terminal is the application; the panel is an accessory, so the
  panel is what gives way. `right_inset` is therefore capped at whatever leaves the terminal
  20 columns, and when even the panel's 300px minimum cannot fit inside that cap the panel
  closes itself rather than rendering a sliver. 20 columns is the smallest width at which a
  shell prompt plus a short command is still readable, and it is checked against
  `grid_dims`'s post-inset column count so the sidebar's width is automatically accounted
  for. **Rejected**: fixing the panel width and letting the terminal shrink to
  `grid_dims`'s existing 1-column floor (`lib.rs:1418` — simplest, but leaves a terminal
  nothing can be typed into, and the terminal is the point of the app), and auto-collapsing
  the sidebar first (most space-efficient, but it makes a second piece of UI disappear that
  the user never touched, in response to a window drag).

### D5: The status pill has three states; the session row keeps all four detached wordings
- **Status**: resolved
- **Rationale**: The design carries one CONNECTED pill; the current dashboard distinguishes
  four detached cases in prose — stale-since, undated, "this tab's session has not written a
  log yet", and "no Claude session in this tab" (`dashboard.rs:398-433`). Those four are the
  entire diagnostic story for "why is this panel empty", which is the panel's most common
  confusing state. So the pill compresses to CONNECTED / IDLE / NO SESSION as an at-a-glance
  badge, and the session identity row underneath still prints the specific wording verbatim.
  **Rejected**: the pill alone (loses the only signal that separates "you have no Claude
  here" from "your Claude has not logged yet" — the two get diagnosed completely
  differently), and a dedicated empty-state screen (the layout would jump wholesale between
  attached and detached, and the panel would look broken rather than empty).

### D6: Panel width persists to `config.toml` under `[ai_monitor]`
- **Status**: resolved
- **Rationale**: A dragged width that resets on restart is the kind of small betrayal that
  makes a resize feel broken. The sidebar already solved exactly this problem — a runtime
  override that outranks the file for the session, an echo-suppression rule so the app's own
  write is not misread as a human edit, and `clear_sidebar_override` when a real human edit
  arrives (tab-resize D15/D15a, `crates/app/src/input.rs:134-225`). `[ai_monitor]` already
  exists in the config (`crates/common/src/config.rs:26,32`), so this adds `width_px` and
  `open` beside `context_window` and reuses that machinery rather than inventing a second
  persistence rule. **Rejected**: session-only memory (simpler, but the width is a
  once-and-forget preference, not a per-session one), and a fixed 430px with no resize (the
  design's drag handle and its live width readout in the bottom bar would both become
  decoration).

### D7: SKILLS shows an absolute local time, resolved via `objc2`
- **Status**: resolved
- **Rationale**: The design shows `at 16:21:18`. The existing relative form (`12s ago`) was
  chosen because `std` alone cannot resolve a local timezone and a UTC clock on a
  local-session dashboard is worse than no clock (`dashboard.rs:255-262`) — but that
  reasoning assumed `std` was the only option. This is a macOS-only app that already depends
  on `objc2` for platform bindings (`.claude/rules/no-gui-crates.md` lists it as an explicit
  exception, D4 in `docs/architecture.md`), so `NSTimeZone`'s offset is available for the
  asking. The original constraint is removed rather than worked around. **Rejected**: keeping
  the relative form (consistent with D2's static panel, but the design's absolute time is
  strictly more useful for correlating a skill run against a terminal scrollback), and
  formatting UTC (the failure mode the original comment correctly refused).

### D8: Only a **user gesture** writes persisted state; a window-forced clamp never does
- **Status**: resolved
- **Rationale**: D4 lets a narrow window squeeze the panel below the user's chosen width. If
  that squeezed value were persisted, one afternoon on a laptop would permanently destroy a
  width chosen on a large display — the user would have to re-drag it every time. So the
  persisted value is the *intent* (what a drag set), and the squeeze is a transient display
  clamp applied on top of it at layout time. Re-widening the window restores the intended
  width with no user action.

  The same rule governs the `open` flag, and must be stated rather than left to the
  implementer — the obvious implementation (persist on every visibility change) is the wrong
  one. **Only the `Cmd+Shift+M` chord writes `[ai_monitor].open`.** D4's width-forced close is
  a transient display clamp applied at layout time, so the persisted flag keeps recording
  what the user last asked for and re-widening restores the panel with no keystroke. The
  stakes are lower than the width case (the worst outcome is one keystroke, not an
  unrecoverable value), which is why it is one rule rather than two decisions.
  **Rejected**: persisting the last effective width (simpler — one value instead of
  intent-plus-clamp — but it silently loses the user's choice, and the loss is invisible
  until they next reconnect a monitor), and persisting `open` on every visibility change (a
  narrow window would turn the feature off permanently).

### D9: The footer states an **absolute** last-update time, never a relative age
- **Status**: resolved
- **Rationale**: The design reads "Streaming live · updated 6s ago". Under D2 nothing
  repaints on a timer, so a relative age freezes: while Claude spends five minutes composing
  a reply with no new log line, the panel would keep insisting "updated 6s ago" — a statement
  that is actively false rather than merely stale. An absolute `16:21:18` stays correct no
  matter how long it sits on screen, and D7 already pays for local-time formatting.
  **Rejected**: appending "(as of last update)" in the style of the existing activity row
  (`dashboard.rs:459` — truthful, but it spends a scarce panel row explaining a defect
  instead of removing it), and dropping the footer (it is the only place the panel says when
  its data is from).

### D10: Fixed chrome, elastic sections — the panel never drops a region
- **Status**: resolved
- **Rationale**: The current layer keeps the *last* `max_lines` (`overlay_layer.rs:314`), so
  a short window decapitates the top of the panel; `dashboard.rs:138-143` compensates by
  composing the summary last. That inverts once the content is a docked panel with real
  chrome: the header, session row, stat grid and footer are the panel's identity and are
  pinned, while the six middle sections share the remaining height, each shrinking its row
  count and saying `+N more`. Every region stays present at every size, so the panel's shape
  is stable and a section showing zero rows still tells the user the section exists.
  **Rejected**: hiding whole low-priority sections as height runs out (regions blink out of
  existence on a window drag, and "TOOL USAGE is missing" is indistinguishable from "TOOL
  USAGE is not built"), and keeping the tail-clipping rule (it is what this decision replaces).

### D11: `compose() -> Vec<String>` is replaced by a semantic model; snapshots migrate
- **Status**: resolved
- **Rationale**: Layout moves to the renderer, so the app hands over structure, not
  pre-formatted 104-column strings. `dashboard.rs` keeps every formatting helper it has —
  `num`, `dur`, `dur_short`, `age`, `tok`, `short_path`, `op_label`, `context_pct`,
  `Detached`, `Env` — and its section functions return typed rows instead of `String`s.
  `RULE_COLS`, `fit_row` and the `rule*` family are deleted: they exist only to compensate
  for not knowing the width. The nine `insta` snapshots re-pin against a text projection of
  the model, so they keep asserting composition policy (ordering, caps, truncation,
  placeholders) rather than glyph geometry. **Rejected**: keeping `compose` alongside the
  model (two composers drift, and the string one would have no consumer), and deleting the
  snapshot tests outright (they encode hard-won ordering and placeholder rules — see
  `dashboard.rs:213-234, 325-332`).

### D12: The cumulative tool tally is capped at 64 names; the panel shows the top 9
- **Status**: assumed
- **Rationale**: D3's `name -> count` map is keyed by a string the log supplies, and MCP tool
  names are long and effectively unbounded, so an uncapped map is a slow leak on a long
  session. 64 distinct names mirrors the crate's existing `DEDUPE_CAP = 64`
  (`telemetry.rs:48`) rather than inventing a new magnitude. On overflow the smallest **incumbent** entries — never the arriving one — are
  merged into a single `other` bucket, in one batch down to a 48-entry watermark so the scan
  is amortised rather than run on every new name. Eviction by lowest
  count starves: once 64 names sit at count >= 2, every genuinely new tool enters at 1 and is
  dropped immediately, forever, no matter how heavily it is later used. Merging keeps the map
  bounded, keeps every call represented, and keeps the map's sum equal to D3's grand total.
  `other` is excluded from the top-9 ranking and from the bar-normalisation denominator: it
  accumulates monotonically and never drains, so ranking it would permanently occupy a slot
  with an aggregate that names nothing, and normalising against it would rescale every real
  tool's bar. The panel draws the top 9 **named** tools, bars normalised to the highest of
  those, matching the design's proportions. **Rejected**: an unbounded map (a slow leak in a long-lived accumulator),
  capping by insertion order like the display rings (it would evict `bash` — the most-used
  tool — in favour of a one-off), and evicting the lowest count (the starvation case above).

### D13: The panel stays per-window and re-points on tab switch (unchanged)
- **Status**: assumed
- **Rationale**: The existing overlay is one per window, bound to the active tab's session,
  re-pointing when the active tab changes (`Overlay::on_active_changed`,
  `crates/app/src/seam.rs:352`). Docking changes where it is painted, not what it is attached
  to, and the attachment rule was settled by two prior specs (`overlay-tab-binding`,
  `ai-monitor-session-isolation`). Carried forward untouched. **Rejected**: a per-tab panel
  with independent widths (it multiplies the watcher lifecycle by the tab count, which those
  specs deliberately avoided).

### D14: The panel is a new renderer module; `overlay_layer.rs` retires with the feature
- **Status**: assumed
- **Rationale**: `overlay_layer.rs` describes itself as a *generic translucent text-panel
  overlay* and its only consumer is ai-monitor. The panel needs a shape pipeline, per-run
  colour and pinned chrome — nothing of the "last N monochrome lines" contract survives — so
  model, layout and layer are written fresh, mirroring `sidebar_model` / `sidebar_layout` /
  `sidebar_row`. Its two GPU regression tests (atlas-swap bind invalidation; zero rebuilds
  across 4 idle frames — `overlay_layer.rs:609-724`) are **ported, not dropped**: both encode
  bugs that already shipped once. **Rejected**: mutating it in place (its name and doc
  comment would describe none of what it does), and keeping it alongside a new module (dead
  code with live tests).

### D15: The sidebar's `⌘ M ai-monitor` row is out of scope
- **Status**: resolved
- **Rationale**: The design adds a toggle affordance at the bottom of the left sidebar. The
  confirmed goal already excludes the sidebar's own design, and adding a row means a new
  `ChromeRegion`, a `hit_test` branch and layout changes in `sidebar_layout.rs` — a second
  feature wearing this one's clothes. The panel is reachable by `Cmd+Shift+M` (D1).
  **Rejected**: drawing it non-interactively (a control that looks clickable and is not is
  worse than no control), and building it fully (scope creep into a file this spec otherwise
  never touches).

### D16: The panel band is pointer-**inert**; `pixel_to_cell` gains a right bound
- **Status**: resolved
- **Rationale**: Found by the L2 review and verified in source. `pixel_to_cell` ends
  `Some((col.min(cols - 1), row.min(rows - 1)))` (`crates/app/src/input.rs`), and
  `terminal_app.rs:2095` resolves a left press as sidebar-or-else-grid. With a docked panel
  and no change, a click anywhere in the panel anchors a terminal selection at column
  `cols - 1`, a drag paints an invisible selection down the terminal's last column, and a
  Cmd-click can follow a URL that happens to sit in that column. The non-goal "pointer
  handling stays out of scope" is the *cause* of this, not an answer to it. So
  `pixel_to_cell` returns `None` for `px >= viewport_w - right_inset` **when
  `right_inset > 0`**, and a press in the band resolves to an inert region. The guard is
  load-bearing: an unconditional bound also removes the far-edge clamp with the panel closed,
  so a selection drag overshooting the window's right edge would stop extending instead of
  clamping to the last column — a regression in an unrelated feature — the same swallow-it-as-`Panel` pattern
  `sidebar_layout::hit_test` already uses (`sidebar_layout.rs:256`). The grab band sits
  **inside** that boundary — `[viewport_w - right_inset, viewport_w - right_inset + 6·scale)`
  — mirroring the sidebar, whose band is likewise inside its own edge
  (`sidebar_layout.rs:539`) rather than straddling it. The design draws its 8px handle
  between the two columns; putting it inside keeps the terminal's last column selectable,
  which straddling would take away. "Out of scope" means the
  panel has no interactions of its own, not that presses fall through to the terminal.
  **Rejected**: leaving the clamp (ships three user-visible bugs on day one), and making
  panel rows selectable (that is the in-panel interaction the goal excludes).

### D17: The panel's grab band is a second resize edge; the sidebar's edge wins a tie
- **Status**: resolved
- **Rationale**: Also from the review. `resize_drag: Option<ResizeDrag>` is a single slot and
  `terminal_app.rs:1298` asserts a resize and a selection are never both armed. A second
  draggable edge does not need a second slot — only one edge can be under the pointer at
  press time — but `ResizeDrag` must record **which** edge it grabbed so `drive_resize`
  applies the pointer to the right target, and the hit test must be ordered. On a narrow
  window a 440px sidebar and a 300px panel can put both 6px bands close together; the
  sidebar's edge is tested first and wins, because it is the older, more frequently used
  affordance. `release_target`'s three variants are unchanged — a release still belongs to
  at most one resize. **Rejected**: a second `Option<ResizeDrag>` field (makes the
  never-both-armed invariant a 2x2 matrix instead of a bool), and an unordered hit test
  (which edge you get would depend on evaluation order — a coin flip at the overlap).

### D18: Sections divide the remaining height by **priority order with a one-row floor**
- **Status**: resolved
- **Rationale**: D10 said the six sections "share" the leftover height, which the review
  correctly flagged as underivable — equal-split, content-proportional, and priority-ordered
  produce completely different panels. The rule is **three passes, headings first**:

  1. Reserve **every** section's heading, in the design's order (TASKS, ACTIVE AGENTS,
     SKILLS, FILE ACTIVITY, RECENT CALLS, TOOL USAGE), dropping from the *bottom* of that
     order if they do not all fit — the vertical analogue of D4's horizontal floor, and the
     one case where D10's "never drops a region" yields.
  2. Grant one row to each surviving section, in the same order, while height remains. The
     grant is **splittable**: a section that fits its heading but not its row keeps the
     heading alone, which D10 already treats as a valid state.
  3. Distribute what is left **round-robin, one row per section per pass**, each section
     capped by how many entries it actually holds. Round-robin rather than greedy: greedy
     would hand a 40-entry TASKS every spare row and leave the other five at one apiece.

  `+N more` is drawn **on the section heading**, never as a row, so it cannot compete with
  data for a one-row grant — the same placement rule the current dashboard uses and for the
  same reason (`dashboard.rs:511-518`).

  **Headings-before-rows is load-bearing, and the first draft of this decision got it
  wrong.** It read "heading plus one row per section, in order, until the height is spent",
  which at a four-line budget across six sections yields *two* sections with a row each —
  while R8.5 requires *four* headings and none. The contradiction was caught during
  implementation, by the agent building the allocator, which implemented R8.5 over the
  decision text. Reserving headings first satisfies R8.1 through R8.6 simultaneously; the
  strict in-order reading cannot satisfy R8.1 and R8.5 at once.
  **Rejected**: equal split (shows `TASKS 1 of 1` beside `RECENT CALLS 1 of 40`), and
  content-proportional (starves the small section the user opened the panel to read).

### D19: The UTC offset is **injected into `Env`**, exactly like `now_ms`
- **Status**: resolved
- **Rationale**: The sharpest finding of the review. `dashboard.rs:17-19` states the module
  reads "no clock, no filesystem, no globals — which is what makes the `insta` snapshots
  byte-stable". D7 puts an `NSTimeZone`-resolved local wall clock into SKILLS rows and D9 puts
  one in the footer; D11 then re-pins the snapshots against the model. Read together, those
  three would render differently on a UTC CI runner than on the author's machine, and
  differently again across a DST boundary — flaky snapshots, caused by the spec. The fix is
  the one the module already uses for the clock: `Env` gains a `utc_offset_secs`, resolved
  in `overlay.rs` (which is impure by design and already reads config and the process table)
  each time the model is rebuilt, and passed in. It is the offset **now**, applied to every
  timestamp on the frame — so a session spanning a DST transition renders its pre-transition
  SKILLS rows an hour off. That is accepted rather than solved: a per-timestamp historical
  offset needs a full tz database, which is a dependency this project does not carry, and the
  error is bounded at one hour twice a year against values whose purpose is ordering. `compose`/`model` stays pure and the snapshots stay byte-stable.
  **Rejected**: calling `NSTimeZone` from `dashboard.rs` (breaks the stated purity
  constraint and the snapshot stability that depends on it), and pinning snapshots to UTC
  (hides the defect rather than removing it).

### D20: One absolute timestamp qualifies the whole panel; UPTIME/IDLE are not re-derived
- **Status**: resolved
- **Rationale**: The review noted D9's argument — a frozen relative age is "actively false" —
  applies verbatim to the UPTIME and IDLE tiles, which are `now_ms - first_event_ms` and
  `now_ms - last_event_ms` (`dashboard.rs:451-459`), and that a duration has no absolute
  form so D9's fix cannot be applied to them. Rather than qualify each tile (D9 rejected
  spending rows on that) or recompute per frame (D2/G1 forbid it), the footer's absolute
  `Last update 16:21:18` is stated as qualifying **every** value on the panel — it is the
  one timestamp the whole frame was computed at. That makes UPTIME/IDLE readings rather than
  counters, which is what they are. **Rejected**: a per-tile "(as of ...)" qualifier (six
  copies of one fact, in the panel's scarcest space), and dropping the two tiles (they are
  the two the design puts first).

### D21: `right_inset` is derived by `sync_right_inset`, re-run by `apply_config`
- **Status**: resolved
- **Rationale**: The review caught that "symmetric with `left_inset`" glosses a trap:
  `set_left_inset` is a **no-op** (`lib.rs:547`, asserted at `lib.rs:1559-1562`) because the
  inset is derived from config-plus-runtime-override by `sync_left_inset`, which
  `apply_config` re-runs (`settings.rs:38-40`). A panel inset written by a plain setter would
  be silently dropped by the next live config reload while the panel kept painting — the
  exact bug the sidebar already hit and fixed. So the panel gets the same shape: a
  config value, a runtime override, a single `sync_right_inset` that derives the inset from
  both, and a call to it inside `apply_config`. `FrameCtx` grows the panel width alongside
  `sidebar: &SidebarConfig`. **Rejected**: a bare `set_right_inset(px)` (re-introduces a
  known bug class in a file that documents having already fixed it).

### D22: Non-destructive reflow is a **dependency**, specified separately
- **Status**: resolved
- **Rationale**: The L2 review's steelman, verified in source: `Grid::resize`
  (`crates/term-core/src/grid.rs:1283-1310`) copies only `col < cols.min(self.cols)`, so
  narrowing permanently discards the right-hand side of every row and widening back fills
  blanks. D1 deliberately makes the panel a one-chord toggle so toggling is cheap — which
  means this feature's primary gesture would be the one destroying output. Today's overlay is
  non-destructive, so docking without reflow is a regression in the property a monitor most
  needs: that looking at it costs nothing. Reflow is therefore a **prerequisite**, not a
  nicety.

  It is **not, however, part of this feature.** A first draft specified it inline and the
  re-review found the mechanism itself wrong — it named `grid.rs:505` as "the single autowrap
  site", but that line *arms* `pending_wrap`; the wrap is consumed at two other sites and the
  flag is cleared without wrapping by CR/LF/BS/TAB and every cursor move, so flagging there
  marks unwrapped rows as wrapped and splices unrelated lines. Six further semantics were
  undecided (overflow direction, cursor rule, alt-screen split, retiring the existing
  `row_soft_wraps` heuristic, a `GridSnapshot` contract extension, and a cost bound against a
  100k-row history times every background tab). Its file set — `grid.rs`, `scrollback.rs`,
  `snapshot.rs`, `links.rs` — has **zero overlap** with this feature's.

  So it is extracted to `specs/grid-reflow/spec.md` with its own L2, and this spec declares
  `depends_on: grid-reflow`. This spec inherits the acceptance criterion ("toggling the panel
  is non-destructive") without owning the mechanism. **Rejected**: specifying it inline (one
  spec with two goals, two crates, and six open questions in its largest item — the reviewer
  found it incoherent as one feature), and accepting the truncation with a Known Gap (it
  knowingly ships data loss on the feature's headline gesture, which the user declined).

## Declared Surface
- `depends_on`: `grid-reflow`
- This spec's files: `crates/renderer/src/lib.rs`, `crates/renderer/src/monitor_*.rs`,
  `crates/renderer/src/overlay_layer.rs` (retired), `crates/app/src/dashboard.rs`,
  `crates/app/src/overlay.rs`, `crates/app/src/input.rs`, `crates/app/src/terminal_app.rs`,
  `crates/app/src/seam.rs` (added during T4: `overlay.rs` has no route to the `Renderer` —
  `AppCtx` holds `&mut` the active writer and cannot also hold `&mut Renderer` — so the
  chord travels the existing deferred path, `AppCtx::set_monitor_open` →
  `AppRequest::SetMonitorOpen(bool)` → applied in `terminal_app::apply_requests`.
  **Corrected during Wave 3**: T4 first sent a *width* here and `AppRequest` dropped its `Eq`
  derive for the `f32` payload. Nothing ever read that width — T10 made
  `[ai_monitor].width_px` plus the runtime override the authority, so the applier reduced the
  payload to `px > 0.0` — leaving an API whose name lied about what it carried. The variant
  now carries the only thing the chord decides, and `Eq` is derived again. Wave 3 also added
  `OverlayCommand::RestoreMonitor { cwd, shell_pid }` on the app → overlay direction, which is
  how R6.6's launch restore reaches `open()` without being a keystroke),
  `crates/app/src/settings.rs`, `crates/ai-monitor/src/telemetry.rs`,
  `crates/common/src/config.rs`.
- **Not** this spec's files (they belong to `grid-reflow`): `crates/term-core/src/grid.rs`,
  `crates/term-core/src/scrollback.rs`, `crates/term-core/src/snapshot.rs`,
  `crates/term-core/src/links.rs`.

## Constraints
- `term-core` must not gain a wgpu/winit dependency; the panel model lives in `renderer`
  and is filled by `app` (`.claude/rules/dependency-direction.md`).
- No GUI/widget crate may be added; the panel is drawn with the existing shape pipeline and
  glyph atlas only (`.claude/rules/no-gui-crates.md`).
- macOS ARM64 only — no `#[cfg(target_os)]` branches, no portability layer
  (`.claude/rules/macos-only.md`).
- No repaint timer may be introduced (D2). The ported idle test guarantees **bind groups**
  only — its own scope note excludes per-frame instance buffers, and `OverlayLayer::record`
  re-lays-out its text every frame today, so there is no re-shape guarantee in that file to
  port. The panel must therefore add one: shaped output is cached on the panel model, so a
  drag (which calls `redraw()` on every `CursorMoved`) does not re-shape all ten regions per
  pointer move.
- No terminal keystroke may be intercepted by the panel except the `Cmd+Shift+M` chord (D1).
- The grid may only be refit when the column/row count actually changes — `Grid::resize`
  reallocates cells, clears the selection, and fires SIGWINCH (`input::needs_refit`,
  `terminal_app.rs:1348`).
- The log resolver, watcher lifecycle, rival set, and attachment latch are untouched; the
  additions to `ai-monitor` are strictly additive counters (D3).
- `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings` must
  pass; `rustfmt` runs on every edited `*.rs` via the PostToolUse hook.
- `Cargo.lock` must not be hand-edited (PreToolUse hook blocks it).

## Known Gaps
- **Toggling the panel costs a TUI redraw.** Opening or closing changes the column count, so
  zsh gets SIGWINCH and any running full-screen app (`vim`, `tmux`, `less`) repaints — the
  toggle is not free. Accepted: the sidebar's collapse and drag already pay exactly this
  cost and it has proven tolerable. Not mitigated, and deliberately not special-cased on
  alt-screen — behaviour that changes per foreground app is harder to predict than a
  consistent cost.
- **Snapshot-test churn is larger than the nine `.snap` files.** D11 also rewrites those of
  `dashboard.rs`'s 91 tests that assert string assembly. Tests that pin *policy* (ordering,
  caps, truncation direction, placeholders) migrate to the model; tests that pin
  104-column geometry are deleted with the mechanism they describe.
- **In-panel scrolling is out of scope**, so on a short window some rows are only reachable
  by making the window taller. D10 keeps every section present and marks the shortfall with
  `+N more`, but there is no way to read the hidden rows.
- **A short panel loses rows, then loses the stat grid.** Two distinct shortfalls, both
  intended, and the second was invisible until the Gate-3 checker measured for it:
  - **Sections.** With enough sections and little enough height, a section shows zero rows
    and only its heading, so `+N more` is not exhaustive. The region still announces itself,
    but the panel can be opened at a size where it carries almost no data.
  - **The stat grid.** Below the height the floor plus a tile row needs — ~155px for the
    first row and ~217px for the second, at 430px wide, a 16px cell and scale 1 — the grid
    yields, **whole rows at a time, from the bottom** (R5.1). So a short panel shows
    UPTIME/IDLE/ERRORS without IN·OUT/CACHE/CONTEXT, and a shorter one shows no tiles at
    all. The grid stopping at the pinned footer rather than painting across it is what
    makes this a yield instead of a collision; `monitor_layout::Painter::footer_top` is the
    single ceiling both the grid and the sections read.
  - **Below ~65px even the floor breaks down, and there is no rule for what it does.** A
    header band plus a session line plus a footer line do not fit in that height in any
    arrangement, and what the layout does under it is arithmetic rather than a decision: the
    header stops fitting below ~38px, the session row below ~65px, and where two of the
    three do fit they overlap. Left as is deliberately — that is a window the terminal
    itself is unusable in, and a fourth degradation rule for it would be more mechanism than
    the case is worth. The R5.1 sweep test therefore starts at 80px and asserts nothing
    below it.
- **The UTC offset is the offset *now*, applied to historical timestamps.** Resolved per
  model rebuild in `overlay.rs` (D19), never per timestamp and never watched: a session
  spanning a DST transition renders pre-transition SKILLS times an hour off, and a timezone
  change mid-session is not observed until the next rebuild.
- **R0.2's PTY clause is not covered by any test, and cannot be under `cargo test`.**
  Raised by the Gate-3 check, which found `settings::refit` and `Tabs::resize_all`
  referenced by zero tests. Two of R0.2's three clauses are now pinned by
  `settings::tests::r0_2_refit_hands_the_live_grid_size_to_the_tabs` — `refit` runs, and it
  hands `resize_all` the renderer's *new* `grid_size()` rather than a repeat of the last
  one — through a `RefitTarget` spy. The third ("the PTY received the matching window
  size") and R5.1's "background tabs too" fan-out both live inside `Session`, which needs an
  `EventLoopProxy`; that needs an `EventLoop`, which winit builds only on the process's main
  thread, while libtest runs tests on worker threads. So a `Session` — and therefore a
  `Tabs` — cannot be constructed in this test harness at all. Both clauses are verified by
  running the app, not by CI. The spy test itself also needs a GPU adapter and a system font
  for `Renderer::new_headless` and skips without them, like the renderer crate's GPU tests.
- **Test churn reaches beyond `dashboard.rs`.** D16 invalidates
  `pixel_to_cell_clamps_past_the_far_edge` (`crates/app/src/input.rs:719`), which asserts the
  far-edge clamp D16 removes; it is rewritten to assert `None` past the panel boundary.
- **Reflow lands in a separate feature.** Until `specs/grid-reflow/spec.md` ships, toggling the panel
  truncates on-screen rows wider than the new column count. This spec's "toggling is
  non-destructive" acceptance criterion is therefore gated on its dependency, not on its own
  tasks.

## Requirements

### R0: The panel docks to the right edge and the terminal reflows to fit beside it

#### R0.1: Opening the panel reduces the terminal's column count
- **Given**: An 1800px-wide framebuffer, a 340px sidebar, cell width 17px, panel closed
- **When**: The user presses `Cmd+Shift+M`
- **Then**: `Renderer::grid_size()` returns fewer columns than before, reduced by
  `floor(right_inset / cell_w)`, and rows are unchanged

#### R0.2: The reflow reaches the PTY, not just the renderer
- **Given**: The panel has just opened and `grid_size()` reports a new column count
- **When**: The frame completes
- **Then**: `settings::refit` has run, `tabs.resize_all(cols, rows)` was called with the new
  count, and the PTY received the matching window size

#### R0.3: Closing the panel returns the columns
- **Given**: The panel is open at 430px and the grid is fitted to the reduced width
- **When**: The user presses `Cmd+Shift+M` again
- **Then**: `right_inset` is 0.0 and `grid_size()` returns the same `(cols, rows)` it reported
  before the panel was ever opened

#### R0.4: Non-destructive toggling is inherited from `grid-reflow`, not satisfied here
- **Given**: `specs/grid-reflow/spec.md` has not yet shipped
- **When**: The panel is toggled over a line wider than the narrowed grid
- **Then**: The line truncates — this spec's tasks do not change `term-core`, and the
  acceptance criterion is recorded as gated on the dependency

#### R0.5: The left sidebar is untouched
- **Given**: The full feature is implemented
- **When**: `git diff` is taken over `crates/renderer/src/sidebar_*.rs`
- **Then**: No `ChromeRegion` variant, `hit_test` branch, or layout row was added for an
  ai-monitor affordance (D15)

### R1: `Cmd+Shift+M` is the only keystroke the panel consumes

#### R1.1: The chord toggles the panel both ways
- **Given**: The panel is closed
- **When**: `Cmd+Shift+M` is delivered
- **Then**: The panel opens; delivering it again closes it

#### R1.2: `Esc` reaches the PTY while the panel is open
- **Given**: The panel is open and `vim` is running in insert mode
- **When**: The user presses `Esc`
- **Then**: `0x1b` is written to the PTY, the panel stays open, and `Overlay::on_key`
  returns `Handled::Passthrough`

#### R1.3: `q` reaches the PTY while the panel is open
- **Given**: The panel is open and `less` is paging
- **When**: The user presses `q`
- **Then**: `q` is written to the PTY and the panel stays open

#### R1.4: No `q` keycap is drawn
- **Given**: The panel renders its footer
- **When**: The footer text runs are inspected
- **Then**: No `q` key affordance is present (D1 — the design's keycap is not drawn)

### R2: `ai-monitor` accumulates cumulative session counters

#### R2.1: Every folded tool call increments the named tally
- **Given**: A fresh `SessionTelemetry`
- **When**: 200 `Bash` tool_use/tool_result pairs are folded
- **Then**: The tally reports `Bash = 200`, even though `TOOLS_CAP = 32` bounds the display
  ring

#### R2.2: The grand total is independent of the tally's bound
- **Given**: A session that folded 187 calls across 70 distinct tool names
- **When**: The snapshot is read
- **Then**: The total reads 187 and equals the sum of the tally plus its `other` bucket

#### R2.3: Failed calls increment a cumulative error counter
- **Given**: A fold sequence where 3 tool_results carry `is_error: true`
- **When**: The snapshot is read
- **Then**: The error counter is 3, and it is a different field from `parse_errors`

#### R2.4: The tally is bounded at 64 names by merging incumbents
- **Given**: A tally holding 64 distinct names, all at count >= 2
- **When**: A 65th name is folded at count 1
- **Then**: The new name is retained, the smallest incumbents are merged into `other` down to
  a 48-entry watermark, and the sum is unchanged

#### R2.5: The additions do not disturb existing fields
- **Given**: The existing `crates/ai-monitor/tests/telemetry.rs` suite
- **When**: It runs against the extended `SessionTelemetry`
- **Then**: Every existing assertion passes unchanged (the counters are purely additive)

### R3: The app hands the renderer a semantic model, not formatted strings

#### R3.1: `dashboard` emits a typed model
- **Given**: A `TelemetrySnapshot` and an `Env`
- **When**: `dashboard::model(snap, &env)` is called
- **Then**: It returns a panel model whose sections carry typed rows with colour roles, and
  contains no pre-padded or rule-filled `String`

#### R3.2: The composer stays pure
- **Given**: The same snapshot and the same `Env` (including `now_ms` and `utc_offset_secs`)
- **When**: `dashboard::model` is called twice in different processes
- **Then**: The two models are equal — no clock, filesystem, timezone or global is read
  inside the function

#### R3.3: Fixed-width machinery is gone
- **Given**: The implemented `dashboard.rs`
- **When**: It is searched
- **Then**: `RULE_COLS`, `fit_row`, `rule`, `rule_with` and `rule_empty` no longer exist

#### R3.4: The renderer performs the pixel layout
- **Given**: A panel model and a panel rect
- **When**: The layout function runs
- **Then**: It emits shape instances and positioned text runs, and the app crate contains no
  pixel arithmetic for the panel

#### R3.5: Migrated snapshots still pin composition policy
- **Given**: The nine migrated `insta` snapshots
- **When**: Section ordering, per-section caps, truncation direction or a placeholder rule is
  changed
- **Then**: At least one snapshot fails

### R4: `right_inset` is derived, clamped, and re-derived on config reload

#### R4.1: `grid_dims` subtracts both side insets
- **Given**: `grid_dims(1200, 800, (10,20), 0.0, 200.0, 300.0)`
- **When**: It is evaluated
- **Then**: Columns are `floor((1200 - 200 - 300)/10) = 70`, and rows are unaffected

#### R4.2: A zero right inset reproduces the pre-feature result
- **Given**: `grid_dims` with `right_inset = 0.0`
- **When**: Compared against the two-inset form for the same arguments
- **Then**: The results are identical (no regression for a closed panel)

#### R4.3: The terminal keeps a 20-column floor
- **Given**: A window narrow enough that a 430px panel would leave 12 columns
- **When**: The inset is derived
- **Then**: `right_inset` is capped so `grid_size().0 >= 20`

#### R4.4: The panel closes when it cannot meet its own minimum
- **Given**: A window so narrow that the 20-column floor leaves less than 300px
- **When**: The inset is derived
- **Then**: `right_inset` is 0.0, the panel is not drawn, and `[ai_monitor].open` is **not**
  written to

#### R4.5: Re-widening restores the panel and its width
- **Given**: The panel was force-closed by R4.4 and the persisted width is 520px
- **When**: The window is widened enough to fit 520px plus 20 columns
- **Then**: The panel reappears at 520px with no user keystroke

#### R4.6: A live config reload does not drop the inset
- **Given**: The panel is open at a runtime-overridden width
- **When**: `Renderer::apply_config` runs for an unrelated config change
- **Then**: `sync_right_inset` re-derives the inset and the panel keeps its width — the
  `set_left_inset` no-op trap (`lib.rs:547`) is not reproduced

#### R4.7: `FrameCtx` carries the panel width to layers
- **Given**: A frame is recorded with the panel open
- **When**: A `DrawLayer` reads `FrameCtx`
- **Then**: `right_inset` is present and equals `Renderer::right_inset()`

### R5: The panel renders the design's ten regions from live telemetry

#### R5.1: Chrome regions are pinned — and the stat grid is the one that yields
> **Corrected after the Gate-3 check, and the first draft of this requirement was
> unsatisfiable.** It read "**Given**: Any panel height → header, session row, stat grid and
> footer are always present", and no implementation ever met it: measured against the panel's
> own metrics (430px wide, 16px cell, scale 1), the pinned header, session row and footer
> already spend the first ~65px, the first stat row cannot be drawn clear of the footer below
> ~155px, and the second below ~217px. The Gate-3 checker found this by sweeping heights;
> before it, nothing in the workspace touched a short panel at all.
>
> D10's *intent* is unchanged and is not being softened here — the panel does not drop a
> region to save space. What changes is that "any height" was a promise the arithmetic cannot
> keep, and a requirement no code can satisfy tests nothing. So this requirement now names the
> floor, and gives the yielding a **direction** — the same move D18 made for the section
> headings, and for the same reason: which region goes has to be a rule, not whatever the
> loop happened to do.
- **Given**: A panel tall enough for the floor to stack — ~65px at a 16px cell and scale 1
- **When**: The panel is laid out
- **Then**: The header, the session row and the footer are drawn. That is the **floor** and
  it never yields. The **stat grid** is what yields, and it yields **whole rows from the
  bottom** of the design's order — CONTEXT before UPTIME — mirroring D18's
  headings-drop-from-the-bottom rule and D4's "the accessory gives way, the identity does
  not". The tiles drawn are always a prefix of that order: never a partial row, never a gap,
  and never fewer than a shorter panel drew.

#### R5.2: The status pill has exactly three states
> **Threshold pinned during T4**: "stale" was left unquantified here. The implementation uses
> `IDLE_AFTER_MS = 300_000` — five minutes, longer than the longest routine gap between tool
> calls, and chosen knowing that under D2 nothing repaints to un-blink it.
- **Given**: An attached session, a stale log, and a tab with no Claude process
- **When**: Each is rendered
- **Then**: The pill reads CONNECTED, IDLE and NO SESSION respectively

#### R5.3: The session row keeps the four detached wordings
- **Given**: Each of `Detached::StaleSince`, `Detached::Undated`, `NoLog` with
  `tab_has_claude`, and `NoLog` without
- **When**: Each is rendered
- **Then**: The session row prints that case's specific existing wording verbatim

#### R5.4: The context tile draws a proportional gauge
- **Given**: `context_pct` returns 45
- **When**: The CONTEXT tile is laid out
- **Then**: A bar is emitted whose filled width is 45% of the track, and the tile reads `45%`

#### R5.5: An unknown context window omits the gauge entirely
- **Given**: A model matching no prefix in `MODEL_WINDOWS` and no configured window
- **When**: The CONTEXT tile is laid out
- **Then**: No percentage and no bar are emitted — not `0%`

#### R5.6: File activity verbs carry the design's colour roles
- **Given**: A `FileOp::Write` row and a `FileOp::Read` row
- **When**: They are laid out
- **Then**: The verb runs carry the magenta and teal roles respectively

#### R5.7: Call duration colour is bucketed by magnitude
- **Given**: Calls of 32.8s, 1.8s and 139ms
- **When**: RECENT CALLS is laid out
- **Then**: Their duration runs carry the amber, neutral and green roles respectively

#### R5.8: Tool usage bars are normalised to the top named tool
- **Given**: A tally of `bash=76, read=27, other=900`
- **When**: TOOL USAGE is laid out
- **Then**: `other` appears in neither the top-9 list nor the denominator; `bash`'s bar is
  full width and `read`'s is 27/76 of it

#### R5.9: SKILLS rows show a local absolute time
- **Given**: `start_ms` = a known epoch value and `utc_offset_secs` = +32400
- **When**: The row is composed
- **Then**: It reads `at 16:21:18` for that offset

#### R5.10: The footer states an absolute last-update time
- **Given**: A snapshot whose newest event is at a known epoch value
- **When**: The footer is composed
- **Then**: It reads an absolute clock time and contains no relative age

### R10: The panel stays per-window and re-points on tab switch

#### R10.1: One panel per window, bound to the active tab's session
- **Given**: A window with three tabs, each running its own `claude`
- **When**: The panel is opened
- **Then**: Exactly one panel is drawn, showing the *active* tab's session

#### R10.2: Switching tabs re-points the panel
- **Given**: The panel is open showing tab A's session
- **When**: The user switches to tab B
- **Then**: `Overlay::on_active_changed` fires and the panel shows tab B's session

#### R10.3: A re-point resets the cumulative counters
- **Given**: The panel showed tab A with a tally of `bash = 200`
- **When**: It re-points to tab B, whose log is folded from scratch
- **Then**: The tally reflects tab B's log only — tab A's counts do not leak across, and no
  double-count occurs from the backfill

#### R10.4: The panel's width is a window property, not a tab property
- **Given**: The panel is 520px wide showing tab A
- **When**: The user switches to tab B
- **Then**: The width stays 520px and no grid refit is triggered by the switch alone

### R6: Panel width and open state persist through `[ai_monitor]`

#### R6.1: A released drag writes the width
- **Given**: The user drags the edge from 430px to 520px and releases
- **When**: The release is handled
- **Then**: `[ai_monitor].width_px = 520` is written to `config.toml`

#### R6.2: A width-forced clamp never writes
- **Given**: The persisted width is 520px and the window narrows so only 360px fits
- **When**: The inset is re-derived
- **Then**: The panel draws at 360px and `config.toml` still reads 520

#### R6.3: Only the chord writes the open flag
- **Given**: The panel is open and the window narrows past R4.4's threshold
- **When**: The panel force-closes
- **Then**: `[ai_monitor].open` still reads `true`

#### R6.6: A persisted `open` is honoured at launch
- **Given**: `[ai_monitor].open = true` in `config.toml` from a previous session
- **When**: the terminal starts
- **Then**: the panel opens on the active tab at the persisted `width_px`, with no keystroke
- **Found during Wave 3.** T10 wrote the flag and nothing read it back, so quitting with the
  panel open reopened closed. R6.1–R6.5 were all satisfiable without a restore path, which is
  how the gap survived L3 — the requirement that makes persistence *mean* something was the
  one nobody wrote. Persisting state no reader consumes is worse than not persisting it.

#### R6.4: The app's own write is not misread as a human edit
- **Given**: A drag has just written `width_px`
- **When**: The config watcher delivers that same file back
- **Then**: The runtime override is retained, not cleared (tab-resize's echo rule)

#### R6.5: A genuine human edit wins
- **Given**: A runtime override is active and the user hand-edits `width_px` in the file
- **When**: The watcher delivers it
- **Then**: The override is cleared and the file's value takes effect

### R7: Rendering cost is bounded and the ported regressions stay fixed

#### R7.1: An unchanged atlas never rebuilds the panel's bind group
- **Given**: Four consecutive frames on one atlas with a visible panel
- **When**: They are recorded
- **Then**: The text bind group was built exactly once

#### R7.2: A swapped atlas does invalidate it
- **Given**: A frame recorded against atlas A, then one against atlas B
- **When**: The bind key and rebuild count are read
- **Then**: The key changed and the bind group was actually rebuilt, not merely re-stamped

#### R7.3: A drag does not re-shape the panel per pointer move
- **Given**: The panel model is unchanged and the pointer moves 50 times during a drag
- **When**: The frames are recorded
- **Then**: Text shaping ran once, not 50 times — the shaped output is cached on the model

#### R7.4: No repaint timer exists
- **Given**: The panel is open and no PTY output, input or watcher event occurs
- **When**: 5 seconds pass
- **Then**: No frame is drawn and `ControlFlow::Wait` is not replaced by a timeout

### R8: Sections divide the remaining height by an explicit allocator

#### R8.1: Every section gets a heading plus one row while height allows
- **Given**: Panel height fitting chrome plus 12 section lines, and six populated sections
- **When**: The allocator runs
- **Then**: All six sections have a heading and at least one row

#### R8.2: Leftover rows are distributed round-robin
- **Given**: Six leftover rows, TASKS holding 40 entries and TOOL USAGE holding 11
- **When**: The allocator runs
- **Then**: Each section gains one row per pass in design order — not all six to TASKS

#### R8.3: A section is capped by its own entry count
- **Given**: Ten leftover rows and a SKILLS section holding 2 entries
- **When**: The allocator runs
- **Then**: SKILLS receives at most 2 rows and the surplus goes to later sections

#### R8.4: A heading may be granted without a row
- **Given**: Height that fits six headings but only five first rows
- **When**: The allocator runs
- **Then**: The sixth section renders its heading alone — a valid state, not an omission

#### R8.5: Headings drop from the bottom only when even they do not fit
- **Given**: Height that fits chrome plus four headings
- **When**: The allocator runs
- **Then**: The first four sections in design order render headings and the last two are
  dropped

#### R8.6: `+N more` is drawn on the heading
- **Given**: A section granted one row while holding five entries
- **When**: It is laid out
- **Then**: One data row is drawn and `+4 more` appears on the heading, not in place of the row

### R9: Pointer input over the panel is inert except at the grab band

#### R9.1: A click inside the panel does not touch the terminal
- **Given**: The panel is open and occupies the right 430px
- **When**: The user presses at a point inside the panel
- **Then**: `pixel_to_cell` returns `None`, no selection is armed, and `dragging` stays false

#### R9.2: A Cmd-click inside the panel opens nothing
- **Given**: The panel is open and a URL sits in the terminal's last column
- **When**: The user Cmd-clicks inside the panel
- **Then**: No hyperlink is opened

#### R9.3: The far-edge clamp survives a closed panel
- **Given**: `right_inset == 0.0`
- **When**: A selection drag overshoots the window's right edge
- **Then**: `pixel_to_cell` still clamps to the last column — the bound applies only when the
  panel band is non-empty

#### R9.4: A press in the grab band arms a panel resize
- **Given**: The panel is open at 430px
- **When**: The user presses within 6 logical px inside the panel's left edge
- **Then**: A resize drag is armed carrying the panel edge as its target, and no selection is
  armed

#### R9.5: The sidebar's edge wins an overlap
- **Given**: A window narrow enough that both grab bands contain the pointer
- **When**: A press is resolved
- **Then**: The sidebar's resize is armed, not the panel's

#### R9.6: The drag grows the panel leftward
- **Given**: A panel resize is armed with a recorded grab offset
- **When**: The pointer moves left by 90 logical px
- **Then**: The panel width increases by 90 — computed as
  `viewport_w - (pointer_x - grab_offset)`, the mirror of the sidebar's expression

#### R9.7: The grid refits only on a column change
- **Given**: A panel drag moving the pointer within one cell width
- **When**: The pointer moves
- **Then**: The band follows the pointer but `settings::refit` is not called, so the selection
  is not cleared and no SIGWINCH is sent

## Tasks

Vertical-slice note: this feature's "user-visible end to end" axis is
**data → model → layout → pixels**, not backend↔frontend. A slice is therefore a *region of
the panel* wired from `TelemetrySnapshot` through `dashboard::model` to drawn shapes and
glyphs. T1–T3 are the horizontal infrastructure that has no single region to belong to
(the inset, the model/layout seam, the counters); everything after is a slice or a gesture.

### T1: Add `right_inset` to the renderer's grid arithmetic [infra]
- **Fulfills**: R4.1, R4.2, R4.7
- **Depends on**: (none)
- Extend `grid_dims` with a sixth parameter, add the `Renderer` field, a `sync_right_inset`
  derivation, and `FrameCtx.right_inset`. No shader or uniform change (the grid is laid out
  rightward from `left_inset`; a right band only removes columns). Update the three
  `grid_dims` test call sites and the eight `FrameCtx` construction sites.

### T2: Define the panel model and its pure layout [infra]
- **Fulfills**: R3.4
- **Depends on**: (none) — parallel with T1
- New `crates/renderer/src/monitor_model.rs` (plain data: sections, typed rows, colour roles,
  gauge values) and `monitor_layout.rs` (pure: model + rect + scale → shape instances and
  positioned text runs, using `char_cols`/`truncate_cols`/`layout_line` from `tab_bar`).
  Unit-tested without a GPU. No app-side or telemetry-side change here.

### T3: Add cumulative counters to `ai-monitor` [infra]
- **Fulfills**: R2.1, R2.2, R2.3, R2.4, R2.5
- **Depends on**: (none) — parallel with T1, T2
- Extend `SessionTelemetry`/`TelemetrySnapshot` with the named tally, the `other` bucket, the
  grand total and the failed-call counter, folded where `ok` is already derived. Purely
  additive — the existing suite must pass untouched.

### T4: Draw the docked panel — chrome, session row, stat grid [vertical]
- **Fulfills**: R5.1, R5.2, R5.3, R5.4, R5.5, R5.10
- **Depends on**: T1, T2, T3
- The first end-to-end slice: `dashboard::model` builds the pinned regions, the new
  `monitor_panel` layer draws them via `SidebarShapePipeline` (rounded boxes, capsules, rings)
  plus a text pass, and `overlay.rs` pushes the model and sets the inset. Retires
  `overlay_layer.rs`, porting its two GPU regression tests. Ships a visibly correct panel.

### T5: Replace `compose()` with the model and migrate the tests [vertical]
- **Fulfills**: R3.1, R3.2, R3.3, R3.5
- **Depends on**: T4
- Delete `RULE_COLS`/`fit_row`/`rule*`; convert the section functions to typed rows; add
  `utc_offset_secs` to `Env`. Re-pin the nine `insta` snapshots against a text projection of
  the model and rewrite the `dashboard.rs` tests that asserted string assembly. Serialized
  after T4 because both edit `dashboard.rs`.

### T6: Render the six content sections with the height allocator [vertical]
- **Fulfills**: R5.6, R5.7, R5.8, R5.9, R8.1, R8.2, R8.3, R8.4, R8.5, R8.6
- **Depends on**: T5
- TASKS, ACTIVE AGENTS, SKILLS, FILE ACTIVITY, RECENT CALLS, TOOL USAGE with their colour
  roles and bars, plus the two-pass allocator and heading-borne `+N more`. Serialized after
  T5 (same files) and needs T5's typed rows.

### T7: Clamp the inset and wire the reflow [vertical]
- **Fulfills**: R0.1, R0.2, R0.3, R4.3, R4.4, R4.5, R4.6
- **Depends on**: T4
- The 20-column floor, the force-close and auto-reopen, `settings::refit` on toggle, and
  `sync_right_inset` inside `apply_config`. Parallel with T5/T6: it edits `terminal_app.rs`,
  `settings.rs` and `lib.rs`, none of which T5/T6 touch.

### T8: Route pointer input around the panel [vertical]
- **Fulfills**: R9.1, R9.2, R9.3
- **Depends on**: T7
- Bound `pixel_to_cell` on a non-empty panel band, add the inert region to press resolution,
  and rewrite `pixel_to_cell_clamps_past_the_far_edge`. Needs T7's live inset to test against.

### T9: Drag the panel's left edge [vertical]
- **Fulfills**: R9.4, R9.5, R9.6, R9.7
- **Depends on**: T8
- Extend `ResizeDrag` with its target edge, order the two grab-band hit tests, add the
  mirrored width expression, and gate the refit on `needs_refit`. Serialized after T8: both
  edit the press-resolution path in `input.rs`/`terminal_app.rs`.

### T10: Persist width and open state [vertical]
- **Fulfills**: R6.1, R6.2, R6.3, R6.4, R6.5
- **Depends on**: T9
- Add `width_px` and `open` to `[ai_monitor]`, the runtime override, the drag-release write,
  and the echo-suppression/human-edit rules mirrored from tab-resize. Needs T9's release event.

### T11: Bind the panel to the active tab [vertical]
- **Fulfills**: R10.1, R10.2, R10.3, R10.4
- **Depends on**: T4, T3
- Verify and pin the existing per-window re-point behaviour against the new counters: a
  re-point re-folds from scratch, so tab A's tally must not leak into tab B, and a tab switch
  must not refit the grid. Parallel with T7–T10 (it edits `overlay.rs` and tests only).

### T12: Assert the cost guarantees [vertical]
- **Fulfills**: R1.1, R1.2, R1.3, R1.4, R7.1, R7.2, R7.3, R7.4, R0.5
- **Depends on**: T6, T9
- The keystroke contract (chord toggles; `Esc`/`q` pass through; no `q` keycap), the shaping
  cache on an unchanged model, the ported bind-group tests, the no-timer assertion, and the
  sidebar-untouched check. Last because it asserts properties of the finished surface.

### T13: Record the inherited reflow criterion [infra]
- **Fulfills**: R0.4
- **Depends on**: (none) — parallel with everything
- Cross-link `specs/grid-reflow/spec.md` from this spec's Known Gaps and confirm no
  `term-core` file appears in this feature's diff.

**DAG**
```
T1 ─┐
T2 ─┼─> T4 ─┬─> T5 ──> T6 ─┐
T3 ─┘       │              ├─> T12
            ├─> T7 ──> T8 ──> T9 ──> T10
            └─> T11
T13 (independent)
```
No cycles. Parallel fronts: {T1, T2, T3, T13}, then {T5→T6, T7→T8→T9→T10, T11}.

## External Dependencies

### Pre-work
- **`specs/grid-reflow/spec.md` needs its own `/specify` run** before its six open questions
  can be built. This feature does not block on it — R0.4 records the criterion as inherited —
  but shipping the panel without reflow means toggling truncates on-screen output.

### Post-work
- Update `docs/architecture.md`: G3 and the Runtime-model section describe an overlay, not a
  docked panel, and the G1 note should state that the panel adds a shaping cache rather than
  relying on the terminal's re-shape skip.
- Update `docs/domain.md`: the ai-monitor glossary entry still calls it "a toggle overlay …
  a future feature".
- Run `/run-term` to eyeball the panel against the design before merging.
