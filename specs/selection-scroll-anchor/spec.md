# Spec: selection-scroll-anchor

## Meta
- **Created**: 2026-10-03
- **Type**: dev
- **Status**: approved (not yet executed)
- **Approved by**: user (spec only — implementation is a separate decision)
- **Approved at**: 2026-10-03
- **L2-reviewer**: two rounds, both NEEDS_FIX, all findings verified and applied except one rejected with reasoning (D5 steelman: idle debounce starves under continuous repaint)

## Goal
Make a drag selection follow its text. Reported in Claude Code: drag-select some
text, then wheel — the highlight stays on the same screen rows while different text
moves under it, and Cmd+C copies whatever is now under the highlight instead of what
was selected.

(1) Terminal-owned moves (shell output scrolling the screen, wheel into scrollback):
anchor selection rows to content (an absolute line index through history) instead of
viewport rows, so the highlight moves with its text, stays visible in scrollback, and
can be made while scrolled back. Reverses `selection-model` D9 and its Known Gap
("a selection that survives scrolling is out of scope").

(2) App-owned moves (Claude Code scrolling its own transcript from a mouse-reported
wheel, tmux, vim): the terminal cannot follow, so clear the selection when the text
under it changes — it never shows or copies stale text.

## Non-goals
- Forwarding click/drag to mouse-tracking apps, or a Shift-drag "force local
  selection" convention (`alt-screen-scroll` kept click/drag reporting out of scope;
  this spec does too).
- Following an application's own scroll. An app that repaints rows in place gives the
  terminal no "scrolled by N" signal; the selection is cleared, not moved.
- Drag auto-scroll past the top/bottom edge (`selection-input` Known Gap — unchanged).
- Word/line (double/triple-click) selection, rectangular selection, copy-on-select.
- Preserving a selection across a window resize — `Grid::resize` keeps clearing it.

## Confirmed Goal
A drag selection either tracks the text it was made on or disappears; it never
highlights or copies text the user did not select.

- **Terminal-owned moves** — output retiring rows into scrollback, and the wheel moving
  the scrollback viewport: the selection is anchored to content lines, so the highlight
  moves with its text, is visible while scrolled back, and a drag can begin while
  scrolled back.
- **App-owned moves** — any program repainting rows in place (Claude Code scrolling its
  transcript, tmux, vim): once the selected text changes, the selection is cleared.
- **Copy reads the selection, not the screen** — Cmd+C returns the selected text even
  when part of it has scrolled off-screen, which means extraction reads scrollback, not
  only the visible snapshot.
- **Eviction is honest** — if selected lines age out of the scrollback cap, the
  selection is cleared rather than re-pointed at different lines.

Done when: new shell output and the wheel move the highlight with its text and Cmd+C
returns that text; and in Claude Code the highlight disappears when the user scrolls
instead of staying on the wrong rows.

## Research

### The defect, reproduced headlessly (2026-10-01, throwaway test, deleted)
On a 10×3 `Terminal`, with `bravo` selected on row 1:
- one line of shell output (`\r\ndelta`) → the selection still reads `(0,1)-(4,1)`, and
  `selection_text()` returns `"charl"`, the text that moved under it;
- an alt-screen app repainting rows in place → `"3-thr"` instead of `"2-two"`;
- `scroll_up(1)` into history → `snapshot().selection == None`, and copy returns nothing.

So the highlight is wrong, and **Cmd+C copies text the user never selected**.

### Why: the selection is stored in viewport coordinates
- `Selection { anchor, focus }` are "**live-viewport** cells (`(col, row)` within the
  visible `cols × rows` grid)" (`crates/term-core/src/selection.rs:7`, `:13-17`). Nothing
  rewrites them when content moves; `Grid::set_selection`/`clear_selection` are bare
  stores (`grid.rs:987-998`), and the only invalidation is `Grid::resize` (`grid.rs:1320-1321`).
- `GridSnapshot::from_grid` copies the selection **only at `view_offset == 0`**
  (`snapshot.rs:69-71`), the deliberate `selection-model` D9 ("a live-viewport concept …
  same policy as images"). Its Known Gap reads: "a selection that survives scrolling is
  out of scope" (`specs/selection-model/spec.md:107`). This spec reverses both.
- `selection-model` also left "when to clear on new output/scroll" to the app
  (`selection-model/spec.md:96`, `:108`), and `selection-input` never wired it:
  "clearing on fresh output is a possible follow-up" (`selection-input/spec.md:108`).
  The only app-side clear is a plain click (`terminal_app.rs:2634-2641`).

### Scrollback: the coordinate system a content anchor needs
- `History` is a `VecDeque<Row>` capped at `DEFAULT_HISTORY_LIMIT = 100_000`, evicting with
  `pop_front` (`scrollback.rs:15-38`). It keeps **no count of evicted rows**, so today
  there is no stable index for "this line" once eviction starts. Index 0 is the oldest
  *retained* row, so its meaning shifts on every eviction.
- The only history push is `Grid::scroll_region_up`, and only for a full-screen region
  on the primary screen (`grid.rs:664-686`). It also pins a scrolled-back viewport with
  `view_offset + 1` and shifts image placements (`images.scroll_up_one()`, `:682`). This
  is the existing precedent for "content moved up one, so move the things anchored to it".
- Snapshot composition maps viewport row `v` to global row `hist - view_offset + v` over
  `[history .. history + rows)` (`snapshot.rs:44-58`). A history row's width is the
  column count **when it was captured**; composition reads `r.get(col)` and pads with
  blanks (`:48-51`).
- Partial-region scrolls (`SU`/`SD`/`IL`/`DL`/`RI` inside a DECSTBM band) and every
  alt-screen scroll move rows **without** touching history (`grid.rs:657-700`, guard at
  `:672`). Content moves on screen but nothing marks it as "the same line".
- The alt screen is a **switch, not a second buffer**: `set_alt_screen` flips a flag and
  resets the region (`grid.rs:914-917`). Entering it with `?1049h` + `CSI 2J` blanks the
  live cells; leaving restores nothing. `CSI 2J`/`3J` both route to `Grid::clear`, which
  blanks the live screen only and never touches history (`parser.rs:508-511`,
  `grid.rs:1115-1124`).
- `Grid::resize` does not rewrap (architecture G4) and clears the selection.

### Measured: what Claude Code actually emits (2026-10-03, real `claude` in a 120×40 PTY, 8 s)
Captured with a Python `pty.fork()` harness from the repo cwd. The raw bytes were
scanned for mode sequences and mapped against the read boundaries.
- Enters the **alt screen** (`?1049h` at byte 164, then `CSI 2J`) and turns on mouse
  tracking `?1000h ?1002h ?1003h ?1006h`. That matches `alt-screen-scroll/loop.md:26`, so
  its wheel scroll is a mouse report the app answers by repainting, never a terminal scroll.
- **Every frame is wrapped in synchronized output**: 4 × `?2026h`, 4 × `?2026l`, each
  pair bracketing exactly one repaint (`?2026h` at 233 … `?2026l` at 2207, then three
  ~110-byte frames).
- **macOS PTY reads cap at 1024 bytes.** Read sizes were `[13, 30, 16, 12, 33, 40, 20,
  1024, 1024, 239, 111, 111, 105]` against a 65 536-byte buffer. The first frame
  (~2 KB) arrived **split across two reads**. The app reads with an 8 KiB buffer
  (`session.rs:121`), and the same cap applies. A check run per chunk would therefore
  routinely see a half-painted frame.
- The four captured frames contain no `SU`/`SD`/`RI` and no `CSI 2K`/`CSI A`. These were
  **startup frames only**: no wheel scroll was driven, because that needs a long
  transcript, which means resuming a live session. The sequence of a *scroll* frame is
  therefore **unmeasured**. The design does not depend on it. Claude Code runs on the
  alt screen, which never retains history (`grid.rs:672`), so whether it scrolls by
  repaint or by `SU` inside the alt screen, the moved rows keep their positional line
  numbers and D4 clears the selection either way.
- This repo parses **no** `?2026` today: `grep` finds no `2026` handling in `parser.rs`.
  Unknown DECSET modes fall through the `?`-mode loop (`parser.rs:410-420`).

### The paths a content-anchored selection touches
- **Drag** (`terminal_app.rs:2574-2589` press, `:2435-2451` move): the press stores
  `drag_anchor = cursor_cell()` in viewport coords, and each `CursorMoved` builds
  `Selection { anchor: drag_anchor, focus: cursor_cell() }`. `cursor_cell` is
  `pixel_to_cell` over the active grid's `dimensions()` (`:1767-1779`).
- **Drag freeze** (`terminal-scroll` D6, `terminal_app.rs:2894-2930`): wheel and snaps
  are suppressed mid-drag *because* the focus is in viewport coords while the store is
  in live-grid coords. A content anchor removes that reason, but the latch/replay logic
  stands on its own and is not re-litigated here.
- **Copy**: `on_key` builds `AppCtx.selection_text` from
  `terminal.snapshot().selection_text()` whenever a selection exists
  (`terminal_app.rs:2009-2016`). It reads only the **visible** snapshot, so it cannot
  return a selection that has scrolled partly off-screen.
- **Extraction** lives once in `GridSnapshot::selection_text` (`snapshot.rs:124-169`),
  with the D8 soft-wrap rule `row_soft_wraps` shared, deliberately, with hyperlinks
  (`snapshot.rs:171-187`, `links.rs:144-153`: "one heuristic, not two that could drift").
- **Drain**: `Session::drain` calls `terminal.advance(chunk)` once per queued chunk, then
  reads one-shot terminal state once at the end (`session.rs:155-185`). The
  `PtyOutput` arm redraws the active tab when anything advanced (`terminal_app.rs:2168-2214`).
  `crates/app/tests/vertical_slice.rs:116` drives `session.drain()` without a window.
- **Render**: `SelectionLayer` tints `snapshot.selection` cells via `Selection::contains`
  in **viewport** coords (`renderer/src/selection_layer.rs:78-101`). It never sees a grid.
  Its one test sets a selection directly (`selection_layer.rs:286`). **No golden-frame
  test covers the selection tint** (`grep` finds no `selection` in
  `renderer/tests/golden_frame.rs`).

### Existing tests this changes
- `term-core/tests/selection.rs:70-80` (`scrolled_snapshot_carries_no_selection`, R3.2)
  asserts the behavior being reversed.
- `term-core/tests/selection.rs` and `selection_layer.rs:286` call `set_selection` with
  viewport `Selection` values; they are the API's only callers besides `terminal_app.rs:2448`.

## Decisions

Complexity: **Complex** (4 signals). Brownfield weights apply.
- 3+ subsystems: `term-core` scrollback/grid/parser, the snapshot projection the renderer
  reads, and the app's drag/copy/drain paths.
- State machine: drag lifecycle × synchronized-update open/closed × when the check runs.
- Data schema: a new content coordinate and a stored reference for the change check.
- Async arrival: PTY chunk timing decides what a check can observe (measured, see Research).

The user delegated the design ("write the spec with your recommendation"), so the
checkpoints below were resolved by recommendation and are marked `assumed`.

### D1: A content line is an absolute index counted through scrollback, kept stable by an eviction counter
- **Status**: assumed
- **Decision**: `History` gains a monotonic `evicted: u64`, incremented once per
  `pop_front`. A content position is `Point { line: u64, col: u16 }`, where history row `i`
  is line `evicted + i` and live row `r` is line `evicted + history.len() + r`. A
  full-screen primary scroll moves live row 0 into history at the **same** line number,
  and an eviction raises `evicted` exactly as much as it lowers every index. So a line
  number names the same text for as long as that text is retained, with no
  per-push bookkeeping.
- **Rationale**: stability becomes a property of the coordinate rather than of every
  call site remembering to shift it. **Rejected**: storing a history-relative index and
  shifting it on each push/evict the way `images.scroll_up_one()` does (`grid.rs:682`).
  That makes a second write path that every future push site must remember, and the
  eviction half has no precedent at all. **Rejected**: a unique id per stored row
  (changes the `Row = Vec<Cell>` type used across the crate for no gain over a counter).
  Rows that move *without* entering history (partial DECSTBM regions, the alt screen)
  keep their positional line number on purpose: that motion is app-owned and D4
  handles it.
  **Rejected (steelman)**: also *following* region scrolls (`SU`/`SD`/`RI`/`IL`/`DL`
  inside a DECSTBM band) by shifting the selection's lines, as some terminals rotate a
  selection on a region scroll. It would keep a selection on its text when `less` or a
  pager scrolls by `SU`. It is rejected for three reasons:
  - A linear span that crosses the band edge would have to move partly and stay partly,
    which two endpoints cannot represent.
  - TUIs use `IL`/`DL` to *insert* and *delete* content, not only to scroll, so
    "the band moved" is not reliably "this text moved".
  - The case that prompted this spec (Claude Code) is a repaint either way (Research).

  D4 turns every such motion into an honest clear. Following region scrolls can be
  layered on later without changing D1, because it would only add a line shift inside
  the band.

### D2: The grid stores a content-anchored selection; the snapshot keeps today's viewport type, produced by projection
- **Status**: assumed
- **Decision**: `Grid` stores the selection as two `Point`s plus the D4 reference.
  `GridSnapshot.selection: Option<Selection>` **keeps its type** (viewport `(col, row)`),
  and `from_grid` fills it by projecting the stored span into the current viewport:
  - the two `Point`s are **normalised first**, to `start ≤ end` ordered by
    `(line, col)`. A backward drag projects exactly like a forward one;
  - with `top = evicted + history.len() − view_offset` (the line on viewport row 0),
    line `L` sits on viewport row `L − top`. This is computed as a **signed** difference
    (`i128`, or a compare before subtracting), so a line above or below the viewport
    can never underflow;
  - a start line above `top` clips to `(0, 0)`, and an end line below the last viewport
    row (`top + rows − 1`) clips to `(cols−1, rows−1)`;
  - the projection is `None` **iff** `end.line < top` or `start.line > top + rows − 1`.
    Otherwise at least one cell is visible, because every stored `col` came from a
    viewport cell and is `< cols`.

  This runs at **every** `view_offset`. It reverses `selection-model` D9 and its test
  R3.2 (`scrolled_snapshot_carries_no_selection`). Clipping preserves the linear-span
  rule: everything from the clipped start to the clipped end is exactly the part of
  the stream that is visible.
- **Rationale**: the renderer's contract and `SelectionLayer` stay byte-identical, so
  all scrollback arithmetic stays inside `term-core`. **Rejected**: putting content
  coordinates on the snapshot and teaching the renderer the offset arithmetic. That
  would widen the versioned contract with `evicted`/`history_len` for no gain, and push
  term-core knowledge into the GPU crate. **Rejected**: keeping the selection hidden
  while scrolled back, which is today's behavior and the third row of the reported defect.

### D3: Viewport cells become content points at the moment the app learns them
- **Status**: assumed
- **Decision**: `Terminal::point_at(col, row) -> Point` converts a viewport cell at the
  current `view_offset`, and `Terminal::set_selection(anchor: Point, focus: Point)`
  replaces the viewport-coordinate setter. The press converts and stores the anchor as
  a `Point` **at press time**. Each `CursorMoved` converts the focus **at move time**.
  **`drag_moved` stays keyed on the viewport cell** (the press cell vs. the pointer's
  current cell), exactly as `selection-input` D8 defines it. It answers "did the
  *pointer* move", not "did the content move", so the press keeps both: the press
  cell for `drag_moved`, and the content `Point` for the selection.
- **Rationale**: the anchor must name the text under the finger *when the button went
  down*. **Rejected**: keeping a viewport setter that converts inside on each call. At
  the live bottom, output that lands between the press and the first move shifts the
  content. Converting the press cell at move time would then anchor on the wrong line:
  the reported defect, reproduced inside a single drag. **Rejected**: comparing
  content `Point`s for `drag_moved`. During streaming output (`tail -f`) the content
  under a motionless pointer moves, so one jitter `CursorMoved` in the same cell would
  read as a drag. A plain click would then create a multi-line selection instead of
  clearing, breaking `selection-input` R3.3/R3.4 and hyperlinks' click-vs-drag test
  (`url_to_open` reads the same flag).
- **A change during a drag ends that drag's selection.** If D4 clears the selection
  while the button is still held, later `CursorMoved` events in the **same** drag do
  not re-create it. The text the press anchored on is gone, and re-creating from that
  anchor would highlight whatever now occupies its line.

  The detection uses explicit drag state, never `selection().is_none()` alone, which
  is also `None` *before* the first move has set anything:
  - the press records **`drag_tab`**, the id of the tab it landed in, and resets
    **`selection_started = false`**;
  - a `CursorMoved` acts only when the active tab is `drag_tab`. A move after a tab
    switch does nothing, so the drag can no longer write its anchor into another
    tab's grid. That closes a pre-existing defect, precedented by `DragLatch`
    carrying tab ids (`terminal_app.rs:2896-2930`);
  - on each acting move, in this order: **(1)** if `selection_started` and the drag
    tab's `selection()` is `None`, the drag is spent, so return; **(2)** if the cell
    differs from the press cell, set `drag_moved`; **(3)** if `drag_moved`, call
    `set_selection` and set `selection_started = true`.

  Anything that clears a started selection mid-drag spends the drag the same way:
  a D4 change or a `resize` (D7). Release behaves as it does today (nothing left to
  clear), and the next press starts fresh. `term-core` needs no new event.
  **Rejected**: re-creating on the next move. A selection made over repainting
  content would flicker off and back on, re-anchored to text the user never pressed on.

### D4: A selection whose selected characters change is cleared
- **Status**: assumed
- **Decision**: when a selection is set, `Grid` records a **reference**: the characters
  (`Cell::ch`, continuation spacers included) of the selected cells on every selected
  line that is **on the live screen** at that moment, keyed by line number. It is
  **always captured at the moment of the set**, and re-taken on **every**
  `set_selection`, so each drag move re-baselines it. There is no deferred or
  "pending" state.

  A selection lying **entirely in history** when set has an **empty** reference. (b)
  below can never fire for it, only (a) can clear it, and copy returns its text with
  no cell check. That is correct, because history rows are immutable.

  **What a re-baseline can adopt.** A change is adopted only if no check has run
  since it happened. Every drain ends in a check unless a synchronized update is open
  (D5), so adoption is confined to one case: a drag move that lands between the two
  halves of a frame split across drains. That move baselines the half-painted cells,
  and the frame's `?2026l` then compares only the second half's changes. The user is
  mid-drag over what is on screen at that instant, and the renderer draws it. The
  adopted state is therefore what the user was pointing at, not a stale selection.
  **Rejected**: deferring the capture to the frame's end (a "pending" reference). The
  second review found that it silently absorbs the frame's own change into the
  baseline. It also needs a pending exception in copy's re-verify, which breaks D6's
  guarantee, and it adds a state for a case that, on the measured traffic, is almost
  always a real change. Claude Code's split frames are its large repaints; its spinner
  frames fit in one 1024-byte read.

  A check (timed by D5) clears the selection when either:
  - **(a)** the selection's **start** line (the earlier of the two points, ordered by
    `(line, col)`, never necessarily the anchor) is older than the oldest retained
    line (evicted). Eviction removes the oldest lines first, so any evicted selected
    line implies the start is evicted. Or
  - **(b)** any referenced cell, re-read at its line (now in history or still live),
    holds a different character.

  History rows are never rewritten, so a line that has moved into history compares
  equal by construction. That is exactly why terminal-owned scrolling preserves a
  selection while app-owned repaints clear it. Selected cells outside the referenced
  column range do not count: typing to the right of a selected word on the same line
  does not clear it.
- **Rationale**: "the text under it changed" is decided by the text, which is also what
  Cmd+C would return. The reference is bounded by one screen of cells (`rows × cols`),
  however far a selection reaches into history. **Rejected**: comparing colours and
  attributes as well. Apps restyle text without changing it (hover, dim, focus), copy
  is unaffected, and clearing would be a false positive. **Rejected**: instrumenting
  every write path with dirty marks (`print`, six erase forms, `ICH`/`DCH`/`ECH`,
  `IL`/`DL`, `SU`/`SD`/`RI`, `clear`, the alt switch). A missed path is a silent stale
  selection, and the bounded compare makes the optimization unnecessary. **Rejected**:
  clearing on any output, which would kill a shell selection whenever anything else on
  screen updates (a prompt clock, Claude Code's spinner). **Rejected**: clearing only
  when the wheel is forwarded to an app. It misses streaming, typing, and `clear`, and
  it puts a terminal policy in the app, where `term-core` tests cannot reach it.
  **Rejected**: keeping the stored text after a change and hiding only the highlight.
  Cmd+C would then paste something no longer on screen, which is invisible state.

### D5: The check runs at frame boundaries — `?2026l` and the end of each drain — never mid-frame
- **Status**: assumed
- **Decision**:
  - The parser tracks DECSET `?2026` (synchronized update) as a `Grid` flag.
  - The D4 check runs **(a)** when `?2026l` is processed, at the exact end of the
    app's frame, and **(b)** in `Terminal::settle_selection(now)`, which
    `Session::drain` calls **once after** its chunk loop. (b) is skipped while a synchronized update is
    open.
  - The flag is cleared by `?2026l`, RIS (`ESC c`), and leaving the alt screen
    (`?1049l`/`?1047l`/`?47l`).
  - **An open block stops deferring after one second, measured at drains.**
    `settle_selection(now: Instant)` takes the time from its caller (the app passes
    `Instant::now()`, and tests pass fixed instants), so `term-core` owns the rule but
    no clock. A settle that finds the flag set with no stamp stamps
    `sync_seen_at = now`. Because the drain that carries `?2026h` always ends in a
    settle, the stamp is taken in that same drain, within one drain of the block
    opening. A later settle with `now − sync_seen_at ≥ SYNC_STALE` (1 s) drops the
    flag and runs the check. Any close (`?2026l`, RIS, alt leave) clears the stamp.
  - **The bound, stated exactly:** a check runs at the **first drain at least one
    second after the block opened**. Settles happen only when output arrives. So if an
    app dies mid-frame and **nothing more is printed**, a changed selection stays
    highlighted until the next output, typically the shell's echo of the user's next
    keystroke. It **cannot be copied** in that window (D6 re-verify).
    **Rejected**: also settling on every redraw to close that window. Redraws run at
    60 fps while the window is visible (architecture C6), so this would add a per-frame
    compare of up to one screen at idle whenever a selection exists. That is a cost on
    the G1 idle budget, spent on a corner that copy is already safe in.
  - Rendering does **not** change: `?2026` gates only the selection check.
- **Rationale**: measured, not assumed (see Research). Claude Code wraps every frame in
  `?2026h … ?2026l`, and macOS hands out at most 1024 bytes per read, so its first frame
  arrived in two pieces. A check at either the frame's end or the drain's end sees only
  finished frames, whereas a check per chunk would see half-painted ones and clear
  selections whose text never changed. **Rejected**: checking at the end of every
  `advance` chunk, for exactly that reason. **Rejected**: checking at render time
  (the same split problem, plus clearing would depend on frame timing, which cannot be
  tested headlessly). **Rejected**: leaving an unclosed block unbounded and relying on
  RIS (`reset`). An app that crashes mid-frame while its shell survives would leave
  every later check skipped. That is today's bug, in a corner, and it contradicts the
  Confirmed Goal. The caller-supplied instant gives the bound without giving
  `term-core` a clock.
  **Rejected (steelman)**: an app-side idle debounce, checking only after N ms with no
  output, to cut false clears for TUIs that do not use `?2026`. A TUI that repaints
  continuously (htop, a tmux status clock, a spinner) never goes idle, so the check
  would **starve** and a changed selection would stay highlighted indefinitely: the
  stale direction, which this spec ranks worse than a false clear. It would also move
  the rule out of `term-core`, where the headless tests are.

### D6: One extractor, on the grid, reading history + live by line; copy uses it
- **Status**: assumed
- **Decision**:
  - `Terminal::selection_text()` extracts the **whole** stored selection from history
    + live by line number. It keeps the D6 linear span, the continuation-skip and
    `trim_end` from `selection-model`, and the D8 soft-wrap join, which becomes a free
    function over a row slice. `GridSnapshot::row_soft_wraps` delegates to that function,
    so hyperlinks keep the **same** rule, with no second heuristic.
  - A history row captured at a different width is read through the current `cols`
    with blank padding, matching what the snapshot shows (`snapshot.rs:48-51`).
  - The app's copy path calls `terminal.selection_text()` instead of
    `snapshot().selection_text()`, and `GridSnapshot::selection_text` is **removed**.
  - **Copy re-verifies.** `selection_text()` runs the D4 test itself, read-only and
    regardless of D5's timing. It returns `None` when the start is evicted or a
    referenced cell differs. So **Cmd+C can never return text that differs from the
    reference**, even in the window before the next check clears the highlight. D4
    has no pending state, so this holds without exception.
- **Rationale**: copy must return a selection that has scrolled partly off-screen
  (Confirmed Goal), and the visible snapshot cannot. **Rejected**: keeping
  `GridSnapshot::selection_text` alongside. It would silently return only the visible
  slice: the trap this spec exists to remove. **Rejected**: carrying the selected
  text on every snapshot, a per-frame extraction cost while any selection exists.
  This also drops the per-keystroke snapshot clone `selection-input` listed as a
  Known Gap, since extraction no longer clones the grid.

### D7: Every existing lifetime rule stays; nothing else clears or moves a selection
- **Status**: assumed
- **Decision**:
  - `Grid::resize` still clears the selection.
  - A plain click still clears it (`selection-input` R3.4).
  - Each tab's `Terminal` owns its own selection.
  - RIS, the alt-screen switch, and `CSI 2J` get **no** dedicated clear. They change
    the cells, and D4 decides. One consequence is accepted: an alt-screen switch made
    **inside** an open synchronized update (`?2026h ?1049h CSI 2J …`) is cleared at
    that block's `?2026l`, or at the first drain one second after it opened (D5), not
    earlier. Until then the highlight trails the switch. Copy is already guarded by
    D6's re-verify.
  - The `terminal-scroll` D6 drag freeze and its latch are **kept unchanged**. Their
    first rationale (viewport focus vs. live-grid store) stops being true, but the
    latch/replay semantics stand on their own, and lifting the freeze is a separate
    feature (wheel-to-extend while dragging).
- **Rationale**: one rule, change-based, instead of a list of events that each must
  remember to clear. **Rejected**: an explicit clear on RIS/alt switch/`2J`. Every one of
  those is already covered by D4, and a dedicated clear would also fire when the text
  survives (for example, a `2J` that changes no selected cell because the selection is
  entirely in history).

### D8: The pixel half is asserted, not assumed
- **Status**: assumed
- **Decision**: a `golden_frame.rs` test registers `SelectionLayer` and renders through
  `Renderer::capture`. On a grid of `rows ≥ 4`:
  - select a word on viewport row 2, feed **one** line of output, and assert that the
    tint is on row **1** (where the text moved) and absent from row 2 (where it was);
  - scroll back by 1, and assert that the tint is on row **2** again.

  Each assertion compares the tinted cell's pixels against an untinted cell of the same
  row, so it measures the tint and not the glyph.
- **Rationale**: CLAUDE.md, verbatim: "A change to the render path is not covered until
  something asserts what reached the screen". The renderer's code does not change, but
  what the snapshot hands it does. Today no test asserts the selection tint in pixels
  at all. **Rejected**: relying on the `selection_instances` unit test, which takes a
  hand-built snapshot and so cannot see a projection bug.

## Constraints
- **C1 — `term-core` stays headless.** All policy (D1, D2, D4, D5, D6) lives in
  `term-core`. No wgpu/winit, and no new dependency anywhere.
- **C2 — The renderer contract keeps its shape.** `GridSnapshot.selection` stays
  `Option<Selection>` in viewport coordinates, and `renderer/src/selection_layer.rs`
  is not edited. Removing `GridSnapshot::selection_text` (D6) is the one deliberate
  contract change, and the renderer never called it.
- **C3 — Hyperlinks unchanged.** The soft-wrap rule is refactored, not altered.
  `links.rs` tests pass unmodified.
- **C4 — `Row` is still `Vec<Cell>`.** Scrollback gains a counter, not a row type.
- **C5 — The check is bounded.** At most `rows × cols` cell comparisons per check, and
  none when no selection exists.
- **C6 — Project gates.** `cargo build`, `cargo test --workspace`, `cargo clippy
  --workspace --all-targets -- -D warnings`, no `unwrap()` outside tests. Run `cargo
  mutants -f` on each touched file (`selection.rs`, `scrollback.rs`, `snapshot.rs`, the
  grid selection functions) with **0 survivors** in the new functions, or each
  survivor shown to be equivalent. Record the survivor counts.

## Known Gaps
- **The Confirmed Goal holds absolutely for copy, and with a stated bound for the
  highlight.** Copy can never return changed text (D6 re-verify, no exceptions). The
  highlight clears at the end of the drain or frame that changed the text. While a
  synchronized update is open, it clears at the first drain one second after the
  block opened (D5). An app that dies mid-frame with nothing printed afterwards
  leaves a changed selection highlighted until the next output, typically the echo of
  the user's next keystroke. In that window it is drawn but cannot be copied.
- **Apps that do not bracket frames with `?2026`** (vim, tmux, others): a repaint split
  across two drain batches can clear a selection whose text ends up unchanged. That is
  a false *clear*, never a stale highlight, and erring in that direction is the choice D5
  makes on purpose (an idle debounce was weighed and rejected there). **How often this
  happens is unmeasured.** To measure it, clears are logged at `debug` with their
  trigger (`sync-end`, `drain`, `sync-timeout`), so a session in vim or tmux shows
  whether `drain` clears outnumber real changes. Whether tmux emits `?2026` toward
  this terminal is also unmeasured.
- **A drag move between the halves of a split frame adopts the half-painted state**
  as its baseline (D4, "What a re-baseline can adopt"). The user is pointing at what
  is drawn at that instant, so this is accepted rather than guarded.
- **An app's own scroll is not followed.** A repaint carries no "scrolled by N"
  signal, and region scrolls are deliberately not followed (D1, steelman). In Claude
  Code a selection is cleared as soon as the view shifts, including the app's
  auto-follow while a response streams. Claude Code's actual scroll-frame sequence is
  unmeasured (Research), but D4's outcome does not depend on it.
- **Rendering still ignores `?2026`**, so there is no tear-free frame presentation. That
  is a separate feature: D5 uses the flag only to time the check.
- **A drag held while output streams at the live bottom** carries its anchor up into
  history, so the selection can grow toward the scrollback cap (D1/D3). The D4 check
  stays bounded (C5), but `AppCtx.selection_text` is still built on **every keystroke**
  while a selection exists (`selection-input` D9). In this corner that is an
  O(selection) extraction per key. A lazy copy path remains that spec's open refinement.

## Requirements

Fixtures below use the throwaway repro's shape: a `Terminal` of the stated size fed
`alpha\r\nbravo\r\ncharlie` (rows 0–2), with `bravo` selected as `(0,1)–(4,1)` unless
stated. "Settle" means `settle_selection(now)` with a fixed test `Instant`.

### R0: The reported defect is fixed end to end, headlessly

#### R0.1: Shell output moves the selection with its text, and copy returns that text
- **Given**: a 10×3 terminal with `bravo` selected at the live bottom
- **When**: `\r\ndelta` is advanced and the terminal settles
- **Then**: `snapshot().selection` projects to `(0,0)–(4,0)` (one row up), and
  `selection_text()` returns `"bravo"`, not `"charl"` as it does today

#### R0.2: A Claude-Code-shaped repaint clears the selection instead of leaving it on new text
- **Given**: a 20×5 terminal after `?1049h ?1000h ?1002h ?1003h ?1006h`, rows painted
  `line-1`…`line-5`, and `line-2` selected on row 1
- **When**: one frame `?2026h` + a repaint of all rows shifted by one (row 1 now
  `line-3`) + `?2026l` is advanced
- **Then**: `selection()` is `None` and `selection_text()` is `None`

#### R0.3: `term-core` stays headless and the workspace adds no dependency
- **Given**: the workspace after this feature
- **When**: `cargo tree -p term-core` is inspected and every `Cargo.toml` is diffed
- **Then**: neither `wgpu` nor `winit` appears under `term-core`, no `[dependencies]`
  line is added in any crate, and `Cargo.lock` is unchanged

### R1: A content line is stable through terminal-owned scrolling (D1)

#### R1.1: Scrollback counts evictions
- **Given**: a `History` with limit 3
- **When**: 5 rows are pushed
- **Then**: `len() == 3` and `evicted() == 2`, and a fresh history reports `evicted() == 0`

#### R1.2: A live cell maps to `evicted + history_len + row`
- **Given**: a 10×3 terminal at the live bottom with `history_len() == h` and `evicted() == e`
- **When**: `point_at(4, 2)` is called
- **Then**: it returns `Point { line: e + h + 2, col: 4 }`

#### R1.3: A full-screen scroll keeps a line's number on its text
- **Given**: a 10×3 terminal with `p = point_at(0, 1)` naming the row that reads `bravo`
- **When**: `\r\ndelta` is advanced
- **Then**: `point_at(0, 0) == p`, and the row at line `p.line` still reads `bravo`

#### R1.4: Eviction keeps line numbers pointing at the same text
- **Given**: a terminal whose history limit is 2 (set through a test constructor) and a
  line number `L` recorded for a row that is still retained
- **When**: enough output is advanced to evict one older row (and not `L`)
- **Then**: line `L` still reads the same text, and `evicted()` has grown by exactly
  the number of rows evicted

#### R1.5: A scrolled-back cell maps through `view_offset`
- **Given**: a 10×3 terminal with history and `scroll_up(1)` applied
- **When**: `point_at(0, 1)` is called
- **Then**: it returns line `evicted + history_len`. At offset 1, viewport row 0 shows
  line `evicted + history_len − 1` (the newest history row), so row 1 is the next one

#### R1.6: An app-owned scroll does not renumber lines
- **Given**: a 10×3 terminal on the alt screen with text on all rows, and
  `p = point_at(0, 1)`
- **When**: `CSI S` (scroll up 1) is advanced
- **Then**: `point_at(0, 1) == p` and `history_len()` is unchanged, while the
  characters at `p` now differ (R4 decides what follows)

### R2: The snapshot projects the stored selection into the viewport at every offset (D2)

#### R2.1: At the live bottom with no output, projection equals the cells the drag covered
- **Given**: a 10×3 terminal; `set_selection(point_at(0,1), point_at(4,1))`
- **When**: `snapshot()` is taken
- **Then**: `snapshot().selection == Some(Selection { anchor: (0,1), focus: (4,1) })`

#### R2.2: Output moves the projection up with the text
- **Given**: R2.1's state
- **When**: `\r\ndelta` is advanced
- **Then**: `snapshot().selection == Some(Selection { anchor: (0,0), focus: (4,0) })`

#### R2.3: A scrolled-back viewport shows the selection (reverses `selection-model` R3.2)
- **Given**: R2.2's state
- **When**: `scroll_up(1)` is applied
- **Then**: `snapshot().selection == Some(Selection { anchor: (0,1), focus: (4,1) })`,
  and the old test `scrolled_snapshot_carries_no_selection` is replaced by this one

#### R2.4: A start above the viewport clips to the top-left cell
- **Given**: a selection from line `top − 2`, col 5, to `top + 1`, col 3, where `top` is
  the line on viewport row 0
- **When**: `snapshot()` is taken
- **Then**: the projection is `(0,0)–(3,1)`

#### R2.5: An end below the viewport clips to the bottom-right cell
- **Given**: a 10×3 viewport scrolled back so that the selection's end line is below
  viewport row 2, and its start is on row 1, col 4
- **When**: `snapshot()` is taken
- **Then**: the projection is `(4,1)–(9,2)`

#### R2.6: A span wholly outside the viewport projects to `None`, and one touching its edge does not
- **Given**: three selections on a 10×3 viewport whose row 0 is line `top`: one ending
  at line `top − 1`, one starting at line `top + 3`, and one ending at line `top`, col 0
- **When**: `snapshot()` is taken for each
- **Then**: the first two project to `None`, and the third projects to a span ending at `(0,0)`

#### R2.7: A backward drag projects like a forward one
- **Given**: `set_selection(anchor = point_at(4,2), focus = point_at(1,0))`
- **When**: `snapshot()` is taken
- **Then**: `selection.normalized() == ((1,0), (4,2))`, the same span as the forward drag

#### R2.8: The renderer side is untouched
- **Given**: the feature branch
- **When**: `git diff main -- crates/renderer/src/selection_layer.rs` is run, and the
  type of `GridSnapshot.selection` is read
- **Then**: the diff touches **only** the `#[cfg(test)]` module, and there only the
  `snap_with_selection` helper (`selection_layer.rs:280-287`), which is adapted to the
  new `point_at` + `set_selection(Point, Point)` setter. No assertion changes, and the
  field is still `Option<Selection>` of viewport `(u16, u16)` points

### R3: The app's drag anchors on content and ends cleanly (D3)

The drag state machine is extracted into a pure type in `crates/app`, testable without a
window, in the idiom of `release_target`/`drag_release_snap`. The event-loop arms only
route to it.

#### R3.1: The press anchors on the text under the pointer at press time
- **Given**: a press on viewport cell `(0,1)` over `bravo` at the live bottom, then
  `\r\ndelta` advanced before any move
- **When**: the first move lands on cell `(4,0)`
- **Then**: the selection set is anchor = the line that reads `bravo` (now viewport row 0),
  col 0, and focus = `point_at(4,0)`. It is **not** the line now on row 1 (`charlie`)

#### R3.2: A motionless pointer over moving content is still a click
- **Given**: a press on cell `(2,2)` while output streams, and a jitter move reported in
  the same cell `(2,2)` after the content has scrolled
- **When**: the move, then the release, are processed
- **Then**: `drag_moved` stays false, no selection is set, and the release clears any
  selection (`selection-input` R3.4 unchanged)

#### R3.3: A started selection that gets cleared spends the drag
- **Given**: a drag that has moved and set a selection (`selection_started == true`)
- **When**: the terminal clears the selection (a D4 change), and a further move to a
  new cell arrives in the same drag
- **Then**: no selection is set, and `selection()` stays `None` through release

#### R3.4: The first move is never mistaken for a spent drag
- **Given**: a fresh press (`selection_started == false`, `selection() == None`)
- **When**: the first move leaves the press cell
- **Then**: the selection is set and `selection_started` becomes true

#### R3.5: A move after a tab switch does nothing
- **Given**: a drag pressed in tab A, then tab B made active with the button still held
- **When**: a move arrives
- **Then**: neither tab A's nor tab B's selection is written

#### R3.6: A drag while scrolled back makes a visible selection
- **Given**: a 10×3 terminal fed `alpha\r\nbravo\r\ncharlie\r\ndelta` (so `alpha` is in
  history), then scrolled back by 1 so that viewport row 0 shows `alpha`
- **When**: a press on `(0,0)` and a move to `(3,0)` are processed
- **Then**: `snapshot().selection == Some((0,0)–(3,0))` and `selection_text()` returns
  `"alph"`

### R4: Changed selected text clears the selection; terminal-owned motion does not (D4)

#### R4.1: A full-screen scroll keeps the selection
- **Given**: R0.1's state before output
- **When**: three lines of output are advanced and the terminal settles
- **Then**: `selection()` is `Some`, and `selection_text()` returns `"bravo"`

#### R4.2: An in-place repaint of a selected cell clears it
- **Given**: `bravo` selected
- **When**: `CSI 2;1H` + `x` is advanced and the terminal settles
- **Then**: `selection()` is `None`

#### R4.3: A change outside the selected columns does not clear
- **Given**: `bra` selected as `(0,1)–(2,1)`
- **When**: `CSI 2;8H` + `zzz` is advanced and the terminal settles
- **Then**: `selection()` is `Some`, and `selection_text()` returns `"bra"`

#### R4.4: A colour-only change does not clear
- **Given**: `bravo` selected
- **When**: `CSI 2;1H` + `CSI 31m` + `bravo` is advanced (same characters, red) and the
  terminal settles
- **Then**: `selection()` is `Some`

#### R4.5: Evicting the selection's start clears it, even for a backward drag
- **Given**: a terminal with history limit 2 and a selection set as anchor = a later
  line, focus = an earlier line (backward), both in history
- **When**: output evicts the earlier line and the terminal settles
- **Then**: `selection()` is `None`

#### R4.6: A selection lying wholly in history ignores live repaints
- **Given**: a selection made while scrolled back, covering only history lines
  (empty reference)
- **When**: `CSI 2J` + new text is advanced and the terminal settles
- **Then**: `selection()` is `Some` and `selection_text()` is unchanged

#### R4.7: A selection spanning history and live is cleared by a change in its live part
- **Given**: a selection from a history line to a live line
- **When**: a selected live cell is overwritten with a different character and the
  terminal settles
- **Then**: `selection()` is `None`

#### R4.8: Every set re-takes the reference
- **Given**: `bravo` selected, then `CSI 2;1H` + `x` advanced (row 1 now `xravo`)
  **without** a settle
- **When**: the same span is set again and the terminal settles
- **Then**: `selection()` is `Some` and `selection_text()` returns `"xravo"`

#### R4.9: The reference never exceeds one screen
- **Given**: a 10×3 terminal and a selection from a line 50 rows back in history to the
  bottom live row
- **When**: the reference is inspected (crate-internal test)
- **Then**: it holds at most `10 × 3` cells

### R5: The check runs at frame boundaries, never mid-frame (D5)

#### R5.1: `?2026` is tracked, including inside a multi-parameter DECSET
- **Given**: a fresh terminal
- **When**: `?2026h`, then `?2026l`, then `?1049;2026h` are advanced in turn
- **Then**: the sync flag reads true, false, and true respectively

#### R5.2: `?2026l` checks at the exact frame end, without a settle
- **Given**: `bravo` selected
- **When**: one chunk `?2026h CSI 2;1H xravo ?2026l` is advanced, with no settle
- **Then**: `selection()` is `None` immediately after the `advance`

#### R5.3: A frame split across drains that restores the text keeps the selection
- **Given**: `bravo` selected
- **When**: chunk 1 `?2026h CSI 2;1H CSI 2K` is advanced and the terminal settles,
  then chunk 2 `bravo ?2026l` is advanced
- **Then**: `selection()` is `Some` after the settle (skipped, block open), and is still
  `Some` after chunk 2 (the text at frame end is unchanged)

#### R5.4: A split frame that really changes the text clears at its end
- **Given**: R5.3's chunk 1 advanced and settled
- **When**: chunk 2 `BRAVO ?2026l` is advanced
- **Then**: `selection()` is `None`

#### R5.5: Without `?2026`, the check waits for the drain's end, not each chunk
- **Given**: `bravo` selected and no synchronized update open
- **When**: `CSI 2;1H x` is advanced, then the terminal settles
- **Then**: `selection()` is `Some` after the `advance`, and `None` after the settle

#### R5.6: An unclosed block stops deferring one second after it was first seen
- **Given**: `bravo` selected, and `?2026h CSI 2;1H x` advanced
- **When**: the terminal settles at `t0`, at `t0 + 999 ms`, and at `t0 + 1000 ms`
- **Then**: `selection()` is `Some` after the first two settles and `None` after the
  third, and the sync flag is false afterwards

#### R5.7: RIS and leaving the alt screen close an open block
- **Given**: `?2026h` advanced, in two separate terminals
- **When**: `ESC c` is advanced in one, and `?1049h` then `?1049l` in the other
- **Then**: the sync flag is false in both, and a settle runs the check without waiting

#### R5.8: `Session::drain` settles once after its chunk loop
- **Given**: a spawned `Session` whose terminal has a selection over a row that the next
  shell output will overwrite
- **When**: that output arrives and `session.drain()` is called, and the test calls no
  settle itself
- **Then**: `terminal.selection()` is `None` after `drain` returns

#### R5.9: Each clear is logged with its trigger
- **Given**: `RUST_LOG=term_core=debug`
- **When**: a selection is cleared by a `?2026l` check, a drain settle, and a sync timeout
- **Then**: each emits one `debug` line naming its trigger (`sync-end`, `drain`,
  `sync-timeout`). This is verified once by hand during the `/run-term` eyeball (T-final),
  since the crate has no log-capture test harness.

### R6: One extractor reads the whole selection, and copy re-verifies it (D6)

#### R6.1: Copy returns a selection whose start has scrolled off-screen
- **Given**: a 10×3 terminal with `bravo`–`charlie` selected as `(0,1)–(6,2)`
- **When**: two lines of output are advanced (moving `bravo` into history)
- **Then**: `selection_text()` returns `"bravo\ncharlie"`

#### R6.2: Existing extraction behavior is preserved
- **Given**: the `selection-model` extraction tests (double-width `한글`, soft-wrapped
  `helloworld`, hard-broken `hi\nyo`, trailing-blank trim)
- **When**: they run against `Terminal::selection_text()`
- **Then**: they return the same strings as before

#### R6.3: A history row captured at another width is read at the current width
- **Given**: a terminal at 20 columns that fills a row with 20 characters, scrolls it into
  history, and is then resized to 10 columns (clearing any selection)
- **When**: that history row is selected across its full current width while scrolled back
- **Then**: `selection_text()` returns its first 10 characters, the same as that row in
  `snapshot().to_text()`

#### R6.4: Copy refuses changed text before any settle
- **Given**: `bravo` selected
- **When**: `CSI 2;1H x` is advanced with no settle
- **Then**: `selection()` is still `Some`, and `selection_text()` returns `None`

#### R6.5: Copy refuses an evicted selection before any settle
- **Given**: a selection whose start line has just been evicted, with no settle since
- **When**: `selection_text()` is called
- **Then**: it returns `None`

#### R6.6: Hyperlinks keep the same soft-wrap rule
- **Given**: the shared soft-wrap function
- **When**: `cargo test -p term-core links` runs
- **Then**: every `links.rs` test passes with no edit to that file's tests

#### R6.7: The visible-only extractor is gone, and copy uses the grid one
- **Given**: the feature branch
- **When**: `grep -n "fn selection_text" crates/term-core/src/snapshot.rs` and
  `grep -n "snapshot().selection_text" crates/app/src` are run
- **Then**: both find nothing, and `terminal_app.rs`'s `AppCtx` population calls
  `terminal.selection_text()`

### R7: Existing lifetime rules are unchanged (D7)

#### R7.1: Resize still clears
- **Given**: a terminal with a selection
- **When**: `resize(6, 2)` is called
- **Then**: `selection()` is `None` (the existing R2.3 test stays green)

#### R7.2: `CSI 2J` does not clear a selection whose text it did not touch
- **Given**: R4.6's history-only selection
- **When**: `CSI 2J` is advanced and the terminal settles
- **Then**: `selection()` is `Some`

#### R7.3: An alt switch inside an open block clears at the block's end, not at the switch
- **Given**: `bravo` selected
- **When**: `?2026h ?1049h CSI 2J` is advanced and the terminal settles, then `?2026l`
  is advanced
- **Then**: `selection()` is `Some` after the settle and `None` after `?2026l`

#### R7.4: The drag freeze and its latch are untouched
- **Given**: the `terminal-scroll` D6 tests (`drag_release_snap` and the wheel-suppression
  tests)
- **When**: `cargo test -p app` runs
- **Then**: they pass with no edit to them

### R8: The tint reaches the screen where the text is (D8)

#### R8.1: Output moves the tint to the text's new row
- **Given**: a headless `Renderer` with `SelectionLayer` added, and a 10×4 terminal with
  all four rows filled and a word selected on row 2
- **When**: one line of output is advanced and the frame is captured
- **Then**: a selected cell on row 1 differs from an unselected cell of row 1 by the tint,
  and no cell on row 2 does. The test skips only if no GPU adapter exists
  (project rule: skip when absent, fail when present but wrong)

#### R8.2: Scrolling back moves the tint back
- **Given**: R8.1's state
- **When**: `scroll_up(1)` is applied and the frame is captured
- **Then**: the tint is on row 2 and absent from row 1

### R9: Project gates (C6)

#### R9.1: Build, test and lint are green
- **Given**: the finished branch
- **When**: `cargo build`, `cargo test --workspace`, and `cargo clippy --workspace
  --all-targets -- -D warnings` run
- **Then**: all three succeed, and no `unwrap()` is added outside tests

#### R9.2: Mutation testing leaves no unexplained survivor
- **Given**: the touched files (`scrollback.rs`, `selection.rs`, `snapshot.rs`, `grid.rs`,
  `parser.rs`, the app's new drag type)
- **When**: `cargo mutants -f <file> -j 4` runs per file, scoped to the new and
  changed functions
- **Then**: the survivor count in those functions is 0, or each survivor is shown
  equivalent and rewritten (`n < size` → `n != size`) rather than excluded, and the
  counts are recorded

## Tasks

> Slices are **compilable units**. A task that changes a public `term-core` signature
> owns the **mechanical** fix of every caller, so the workspace builds when it ends.
> Behavior changes at those call sites belong to the task named for them. Every task
> also owns R9: build, test, and `clippy -D warnings` green, plus `cargo mutants -f`
> on the files it touched, with the survivor count recorded in the task's report.

### T1: `term-core` — content coordinate, anchored storage, projection, one extractor [infra]
- **Fulfills**: R0.3, R1.1–R1.6, R2.1–R2.8, R6.1, R6.2, R6.3, R6.6, R6.7, R7.1
- **Depends on**: (none)
- **Changes**:
  - `scrollback.rs`: an `evicted` counter.
  - `selection.rs`: `Point` and its `(line, col)` ordering.
  - `grid.rs`:
    - stores two `Point`s;
    - `point_at`;
    - a test-only history-limit constructor for R1.4/R4.5;
    - the extractor reading history + live through the current `cols`.
  - `snapshot.rs`:
    - projection with normalisation, signed arithmetic and clipping (D2);
    - `selection_text` removed;
    - `row_soft_wraps` delegating to the shared free function (D6).
  - `lib.rs`: the `Terminal` surface (`point_at`, `set_selection(Point, Point)`,
    `selection_text`).
- **Tests**: `term-core/tests/selection.rs` is updated: R3.2's old test is replaced by
  R2.3, and the extraction tests move to `Terminal::selection_text` (R6.2).
- **Mechanical callers**: the renderer's `snap_with_selection` helper (R2.8), and
  `terminal_app.rs`'s drag arms. Those convert the cell with `point_at` at the
  existing call sites, which is a stop-gap that T3 replaces. The `AppCtx` copy source
  becomes `terminal.selection_text()` (R6.7).

### T2: `term-core` — change check, `?2026` timing, copy re-verify [infra]
- **Fulfills**: R0.1, R0.2, R4.1–R4.9, R5.1–R5.7, R5.9, R6.4, R6.5, R7.2, R7.3
- **Depends on**: T1 (same files: `grid.rs`, `lib.rs`)
- **Changes**:
  - the D4 reference, captured on every set, bounded to live lines;
  - the check, with (a) evicted start and (b) changed referenced cell;
  - `parser.rs`: tracks DECSET `?2026` inside the existing multi-parameter loop; runs
    the check at `?2026l`; and RIS clears the flag;
  - `set_alt_screen(false)` clears it;
  - `Terminal::settle_selection(now)` with the `sync_seen_at` stamp and
    `SYNC_STALE = 1 s`;
  - `selection_text()` re-verifies read-only (D6);
  - each clear emits one `tracing::debug!` naming its trigger (R5.9).
- **Headless only**: no app file is touched.

### T3: `app` — drag state machine, routed from the event loop [vertical]
- **Fulfills**: R3.1–R3.6, R7.4
- **Depends on**: T1 ← **parallel with T2** (T2 touches only `term-core`; T3 touches only
  `crates/app/src/terminal_app.rs`, `crates/app/src/input.rs` or a new
  `crates/app/src/drag.rs`, and `crates/app/src/lib.rs`'s module list)
- **Changes**:
  - Extract the drag into a pure type: press records `drag_tab`, press cell, anchor
    `Point`, and `selection_started = false`. A move returns *ignore / spent / set*
    in D3's order (spent-check, then `drag_moved`, then set).
  - Its unit tests drive R3.1–R3.5 with a `Terminal` and plain cells, and no window.
    R3.3 simulates the clear with `clear_selection()`, so it does not wait on T2.
  - The `CursorMoved`/`MouseInput` arms keep their routing and only delegate.
  - `drag_moved` keeps its meaning for hyperlinks' `url_to_open`.
  - The `terminal-scroll` D6 latch code is not edited (R7.4).

### T4: `app` — `Session::drain` settles once per drain [vertical]
- **Fulfills**: R5.8
- **Depends on**: T2 (the `settle_selection` API) ← **parallel with T3** (touches only
  `crates/app/src/session.rs`, plus a test in `crates/app/tests/vertical_slice.rs`)
- **Changes**: one call, `self.terminal.settle_selection(Instant::now())`, after the
  chunk loop and before the one-shot reads. R5.8 is tested with a real zsh session
  that prints over the selected row, in the idiom of `vertical_slice.rs:116`.

### T5: `renderer` — golden-frame assertion of the moved tint [vertical]
- **Fulfills**: R8.1, R8.2
- **Depends on**: T1 ← **parallel with T2, T3, T4** (touches only
  `crates/renderer/tests/golden_frame.rs`)
- **Changes**:
  - registers `SelectionLayer` with `add_layer` and captures through
    `Renderer::capture`;
  - compares a selected cell's pixels with an unselected cell of the same row, so it
    measures tint rather than glyph;
  - skips only when no GPU adapter exists, and fails when one does but the tint is
    on the wrong row (memory: skip vs. fail).

## External Dependencies

### Pre-work
- (none)

### Post-work
- **Eyeball with `/run-term`** (by the user, once T1–T5 are merged):
  1. In zsh: select a line, run `seq 5`, and confirm the highlight rides up with its
     text and Cmd+C pastes the original line.
  2. Wheel back, drag over history, and confirm it is visible and copies.
  3. In Claude Code: select, wheel, and confirm the highlight disappears.
  4. With `RUST_LOG=term_core=debug`, confirm each clear logs its trigger (R5.9).
- **Optional measurement for the Known Gap**: run a short vim and tmux session with a
  selection on screen and read the `drain` vs. `sync-end` clear counts. Whether
  `drain` clears outnumber real changes decides whether the false-clear gap needs
  more than this spec.
- **Spec bookkeeping**: add a one-line pointer under `specs/selection-model/spec.md`
  Known Gaps and `specs/selection-highlight/spec.md` D9, saying that
  `selection-scroll-anchor` reverses the "hidden while scrolled" policy, so the next
  reader does not follow a stale decision.
