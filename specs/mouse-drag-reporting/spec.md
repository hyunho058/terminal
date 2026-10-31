# Spec: mouse-drag-reporting

## Meta
- **Created**: 2026-10-30
- **Type**: dev
- **Status**: drafting
- **Mode**: unattended
- **Approved by**: user (D3 approved; OSC 52 stays out of scope)
- **Approved at**: 2026-10-30

## Goal
When I drag some sentence on claude code and Antigravity CLI using mouse, It is not working (I wanted to  be drag and copy) mode : unattended

## Non-goals
- OSC 52 clipboard writes. The terminal does not start honouring `ESC ] 52` here. That would let any program in a tab set the system clipboard, which is a security-relevant policy change. Claude Code copies through `pbcopy` on macOS without it (see Research).
- Hover motion reports with no button held (`?1003` with no button down). mouse-button-reporting already left these out, and that stays.
- Terminal-side (our) selection under a plain drag while an app has mouse tracking on. The press has already been sent to the app and cannot be taken back. Shift-drag stays the way to get a terminal selection (mouse-button-reporting D3).
- Modifier bits (Shift/Alt/Ctrl) in the report byte. Press and release do not encode them today, and motion keeps the same shape.
- Changes to `crates/term-core` or `crates/renderer`.

## Confirmed Goal
In a tab running Claude Code or the Antigravity CLI (agy), a plain left-button drag across text highlights it in that app, and releasing the button puts the text on the macOS clipboard, the same as in iTerm2. Today the drag does nothing.

The cause (inferred, from the code and a measurement): since #57 (mouse-button-reporting), a press over the grid in a mouse-tracking app goes to the app as a report and arms no terminal selection. But the terminal sends **no motion reports while the button is held**, and it sends the release at the **press** cell. So the app sees a click in one spot and never a drag, and its own copy-on-select never fires.

Done when:
- with `?1002` (button-event) or `?1003` (any-event) tracking on, moving the pointer into a new cell while a reported button is held writes one motion report for that cell (SGR `ESC [ < 32+b ; col ; row M`, or the legacy encoding);
- the release is reported at the last reported cell, not the press cell;
- a drag that leaves the grid reports the cell clamped to the grid's edge;
- `?1000`-only and `?9` apps, and Shift-drag selection, behave exactly as today;
- `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings` stay green, with new tests for every rule above.

(inferred) Copying is the app's job. The terminal only has to deliver the drag.

## Research
- **Root cause.** `WindowEvent::CursorMoved` (`crates/app/src/terminal_app.rs:2683-2724`) only drives a *terminal* selection (`if inner.dragging`, `:2715`). When the press was reported (`inner.mouse_reported` is `Some`, set at `:2879` and `:2912`), `dragging` stays false, so a held-button move writes nothing to the PTY.
- The release arm (`terminal_app.rs:2918-2936`) reports at the cell latched at press time, `mouse_reported = Some((button, col, row))` (`:280`). The focus-loss release (`:3124-3136`) does the same.
- Pure routing and encoding live in `crates/app/src/input.rs`:
  - `mouse_button_route` (`:3347`): bypass on `Off`, ⌘ or Shift.
  - `encode_mouse_button_report` (`:3362`): SGR `M`/`m`. Legacy is `b+32`, release is code 3, and coordinates over 223 give `None`.
  - `mouse_button_bytes` (`:3407`): X10 never reports a release.
  - `grid_press_action` (`:3442`) and `grid_release_action` (`:3479`).
  - These are re-exported from `crates/app/src/lib.rs:92-94`, and `crates/app/tests/tt1..tt3_mouse_button_*.rs` cover them.
- **No motion encoder exists**: nothing produces the button-held motion code (`32 + button`).
- `pixel_to_cell` (`input.rs:1067-1092`) returns `None` for a pointer left of the grid or above it, and inside the monitor band. It clamps only to the right and below. A drag that strays over the sidebar therefore has no cell today.
- `Grid::mouse_protocol` (`crates/term-core/src/grid.rs:1051-1063`) reports the most capable mode enabled (AnyEvent > ButtonEvent > Normal > X10). The parser sets each mode on its own (`parser.rs:450-455`).
- **Measured 2026-10-30.** Claude Code 2.1.314 in a PTY, `TERM=xterm-256color`, enables `?1000h ?1002h ?1003h ?1004h ?1006h ?1049h ?2004h`, so its effective protocol is `AnyEvent` with SGR encoding.
- Claude Code's bundle carries an in-app selection with `isDragging` and `copyOnSelect` (default on, "Copy on select" in `/config`). On release it calls `copySelectionNoClear()` and then a clipboard write. Its clipboard path picks `"native"` on macOS (`TSe()`, which runs `pbcopy`) and also emits OSC 52. **The copy therefore works in this terminal, which ignores OSC 52, provided the drag arrives.**
- The Antigravity CLI binary (`~/.local/bin/agy`) contains `ESC[?1002h`, `ESC[?1003h` and `ESC[?1006h`. Its embedded changelog says: "Added copy-on-select in no-flickering mode so dragging highlights text and releasing the mouse copies…", plus a `copyOnSelect` setting. This is the same mechanism. Not measured live: agy stopped at sign-in and the folder-trust prompt in the probe.
- The term-core parser handles OSC 0/2 and 133 only (`crates/term-core/src/parser.rs:374-388`). OSC 52 is ignored.
- `specs/mouse-button-reporting/spec.md` lists "Mouse hover motion reporting (`?1003` AnyEvent without buttons held)" as a known gap. It never mentions button-held motion, which is the gap this spec closes.
- The test pattern is pure functions in `input.rs` with inline unit tests, re-exported in `lib.rs`, plus acceptance files `crates/app/tests/ttN_*.rs`.
- Build, test and lint: `cargo build`, `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings` (`CLAUDE.md`).

## Decisions

Complexity: Simple (1 signal: a press, motion, release state machine). Brownfield, so the implementation checkpoints come from the mouse-button-reporting pattern. No checkpoint is fenced.

| Dimension | Checkpoints | Resolved |
|-----------|-------------|----------|
| Core Behavior | motion reported while a button is held (D1); one report per cell (D2); where the release lands (D3) | 3/3 |
| Scope | which protocols get motion (D1); modifier bypasses unchanged (D5); copy stays the app's job (D7) | 3/3 |
| Error/Edge | pointer leaves the grid (D4); app turns tracking off mid-drag (D1/D3); legacy coordinates over 223 (D2) | 3/3 |
| Data Model | the `mouse_reported` latch moves with motion (D2/D3); a second button pressed while one is latched (D3); hover and link state while latched and at release (D6) | 3/3 |
| Implementation | pure functions in `input.rs` (D2/D4); `CursorMoved` wiring and hover during the drag (D6) | 2/2 |

Composite: 1.00. Inversion probe: "every requirement passes, but the drag still does nothing in Claude Code". That happens if Claude Code ignores motion when the effective protocol is `AnyEvent`, or needs a modifier bit. The measurement rules out the first: Claude enables `?1002`/`?1003` and has an `isDragging` path. The second is excluded by its SGR parser accepting code 32 for a left-button drag (standard xterm). The manual check in Known Gaps confirms it end to end.

### D1: Report button-held motion only under `?1002` and `?1003`
- **Status**: assumed
- **Rationale**: A pointer move while a *reported* button is held writes one motion report, but only when the effective protocol is `MouseProtocol::ButtonEvent` or `MouseProtocol::AnyEvent`.
  - The button code is `32 + b`, with b = 0, 1, 2 for Left, Middle, Right.
  - SGR gives `ESC [ < {32+b} ; {col+1} ; {row+1} M`. Motion is always `M`, never `m`.
  - Legacy gives `ESC [ M {32+b+32} {col+33} {row+33}`, and `None` past 223, the same as press and release.
  - `Normal` (`?1000`), `X10` (`?9`) and `Off` get no motion. That is xterm's definition of those modes, and it keeps every click-only app exactly as today.
  - The protocol is read fresh on each move, so an app that switches tracking off mid-drag stops getting motion at once.
  - Rejected: sending motion under `?1000` as well. Click-only apps (an older `mc` or `htop`) would get bytes they never asked for.
  - Rejected: per-protocol behaviour beyond xterm's. There is nothing to gain from it.

### D2: One report per cell change, decided by one pure function that also moves the latch
- **Status**: assumed (revised after L2 review: the latch move is now inside the tested unit)
- **Rationale**: Add `input::reported_drag_motion(latch: &mut Option<(MouseButton, u16, u16)>, protocol, encoding, cell: (u16, u16)) -> Option<Vec<u8>>`.
  - It returns `None`, and leaves `latch` untouched, when the latch is `None`, when `cell` equals the latched cell, when D1's protocol rule says no, or when the encoding returns `None` (legacy past 223).
  - Otherwise it returns the bytes and sets `*latch = Some((button, cell.0, cell.1))`.
  - Because the latch moves inside the function, "press, move, release at the new cell" is testable end to end without a window: call `grid_press_action`, then `reported_drag_motion`, then `grid_release_action` on the same latch value. The event loop only writes the bytes.
  - The encoder is `input::encode_mouse_motion_report(encoding, button, col, row) -> Option<Vec<u8>>`.
  - Both are re-exported from `lib.rs`, like the mouse-button-reporting functions.
  - Rejected: returning the new cell and letting the event loop move the latch. The pure pieces would all pass while the latch move, the line this feature hinges on, went untested. Today's bug is exactly a missing event-loop line.
  - Rejected: one report per `CursorMoved`. winit fires on every pixel, xterm reports per cell, and a pixel stream would flood the PTY with duplicates.
  - Rejected: putting the logic inline in `terminal_app.rs`. It could not be tested without a window (mouse-button-reporting D1).

### D3: The latch follows the drag, so the release lands on the last reported cell
- **Status**: assumed
- **Rationale**: `reported_drag_motion` moves `inner.mouse_reported` to each newly reported cell (D2), and the event loop writes the bytes.
  - The focus-loss release (`terminal_app.rs:3124-3136`) stays unchanged and now reports at that cell.
  - **A latched report always gets its release, whatever modifiers are held at release** (revised after Spec Review 3). `grid_release_action` today routes the release through `mouse_button_route`, which bypasses on ⌘ or Shift (`input.rs:3347-3352`). So a drag that ends with ⌘ held (R0.4) would send press and motion but never the `m`, stranding Claude Code's `isDragging` selection, and its copy-on-release would never fire. The press already committed the gesture to the app (D5's own argument), so `grid_release_action` passes `&ModifiersState::empty()` to `mouse_button_bytes`. Protocol checks are kept: `Off` still gives no release, and X10 still gives none. Its signature does not change. No existing test asserts a modifier-held release (checked: `crates/app/tests/*` and `input.rs` tests), so C4 holds.
    - Rejected: accepting the dropped release. That breaks R0.3's copy for any drag ending with ⌘ or Shift.
  - That is the drag's end, which is the point the app uses to close its selection.
  - Rejected: passing the live cursor cell to `grid_release_action`. It changes a shipped signature and its tests. It also gives the focus-loss path a different answer from the release path. And it can name a cell the app never saw a motion for (a legacy cell past 223).
  - A press-and-release with no move still reports both at the press cell, so mouse-button-reporting R0.1 and R3.2 hold.
  - If an app drops `?1002`/`?1003` mid-drag and leaves `?1000` on, motion stops, and the release reports at the last *moved* cell. That is the last point the app was told about.
  - A second button pressed while one is latched overwrites the latch (the press arms at `terminal_app.rs:2879` and `:2912`, unchanged). Motion then reports the newer button, and the first button's release no longer matches and goes unreported. This is the shipped behaviour for chorded presses, and this spec leaves it alone.

### D4: A reported drag that leaves the grid is clamped to the grid's edge
- **Status**: assumed
- **Rationale**: Add `input::pixel_to_cell_clamped(cursor, cell, left_inset, top_inset, viewport_w, right_inset, cols, rows) -> Option<(u16, u16)>`, carrying the same `#[allow(clippy::too_many_arguments)]` as `pixel_to_cell` (`input.rs:1066`).
  - It takes `pixel_to_cell`'s arguments.
  - A pointer left of the grid (over the sidebar), above it (over the top inset), or outside the window with a negative x or y maps to column 0 or row 0. macOS keeps sending `CursorMoved` outside the window while a button is held, and those coordinates clamp exactly like in-window ones.
  - A pointer in the monitor band or right of the grid maps to the last column. A pointer below the grid maps to the last row.
  - It returns `None` only for degenerate geometry (zero cell size, `cols == 0` or `rows == 0`), the same guards as `pixel_to_cell`.
  - Only the reported-drag motion path uses it. A press still uses `pixel_to_cell`, because a press outside the grid is not a grid press.
  - Rejected: dropping motion while the pointer is outside the grid. The app's selection would freeze wherever the pointer crossed the edge, and iTerm2 and xterm clamp.

### D5: Bypasses and click-only behaviour do not change
- **Status**: resolved (L1: mouse-button-reporting D3, hyperlinks D17/R4.11)
- **Rationale**: Shift-drag and ⌘ still bypass reporting at press time. They arm the terminal's own `SelectionDrag`, never `mouse_reported`, so no motion report can follow them.
  - Shift-drag is the terminal-selection fallback for apps that track the mouse but have no selection of their own.
  - Rejected: also adding Option as a bypass (iTerm2's key). Nobody asked for it, and Shift already works.
  - Rejected: letting a plain drag become a terminal selection once the pointer moves. The press already reached the app, so the app and the terminal would both own the gesture.

### D6: Wiring in `CursorMoved`
- **Status**: assumed
- **Rationale**: In `WindowEvent::CursorMoved`, after the resize-drag and graph early returns and **before** `inner.refresh_hover()` / `inner.refresh_link_hover(..)` (`terminal_app.rs:2709-2710`, above the `if inner.dragging` block at `:2715`), add a branch for when `input::drag_owns_pointer(inner.mouse_reported, protocol)` is true. `drag_owns_pointer` is a new pure helper: `latch.is_some()` and the protocol is `ButtonEvent` or `AnyEvent`. Gating on the protocol keeps hover for a held `?1000`/`?9` press, or a held Right/Middle press under those modes, exactly as today, as D1 promises for click-only apps. The branch:
  1. Compute `pixel_to_cell_clamped`.
  2. Call `reported_drag_motion(&mut inner.mouse_reported, …)` with the active tab's `mouse_protocol()` and `mouse_encoding()`.
  3. On `Some(bytes)`, write the bytes to `tabs.active_mut()`. A write error is logged with `tracing::warn!`, as at `:2876`. The latch has already moved (D3).
  - The two branches are exclusive by construction, because a press sets either `mouse_reported` or `dragging`, never both.
  - While `drag_owns_pointer` is true, the arm sends the motion report and returns **before** `refresh_hover()` and `refresh_link_hover()`, after setting `last_cursor` and `pointer_inside` as today. A reported drag owns the pointer, just as a resize drag does (`terminal_app.rs:2695-2698`), so no sidebar card or ⌘-link lights up under it. Link hover has two more triggers, `LinkTrigger::Modifier` (⌘ pressed mid-drag, `terminal_app.rs:2676-2681`) and `LinkTrigger::Snapshot`. So `refresh_link_hover` (`:2041`) builds its `HoverInput` with `cmd_held: self.mods.super_key() && !input::drag_owns_pointer(self.mouse_reported, <active tab's mouse_protocol()>)`. That covers all three triggers in one place.
    - It is a mask, not an early return, because `HoverState::on_modifier` is the only thing that re-arms after a focus-loss disarm (`hyperlink.rs:122-147`). An early return would throw away a real ⌘ edge pressed mid-drag, and the link would stay dead after the release until ⌘ was pressed again. With the mask, the ⌘ edge still re-arms and nothing lights.
  - The reported-release arm (`terminal_app.rs:2918-2936`) ends with `inner.refresh_hover()` and `inner.refresh_link_hover(LinkTrigger::Snapshot)`. Both go at the end of the arm, **outside** the `if let GridReleaseAction::Report(..)`, so they run whatever the release action was. A ⌘ held across the release therefore lights the link under a stationary pointer, so a ⌘-click works without nudging the pointer, and the sidebar card under the pointer re-highlights. `Snapshot` forces a resolve past `on_pointer`'s same-cell early return, and it never re-arms on its own (`hyperlink.rs:150-165`), so the focus-loss guard is untouched.
  - Rejected: putting motion in a new event arm. `CursorMoved` already has a single arm, and two arms would split one event's handling.

### D7: Copying stays the app's job, and the terminal does not add OSC 52
- **Status**: assumed
- **Rationale**: Claude Code copies on release through `pbcopy` on macOS (measured from its bundle, `TSe()` returns `"native"`), so delivering the drag is enough for "drag and copy".
  - **tmux is the case where this falls short.** With `set -g mouse on`, tmux enables `?1002`/`?1006`. After this change a plain drag in a tmux pane enters tmux copy-mode and highlights, which it never did before. On release, tmux's default `MouseDragEnd1Pane` copies into tmux's own buffer and reaches the system clipboard only through OSC 52, which this terminal ignores. So in tmux the highlight appears, but `pbpaste` does not change. This is what iTerm2 does with "Applications may access clipboard" off. Shift-drag stays the working path there, because it bypasses reporting and makes a terminal selection, which ⌘C copies. Claude Code run *inside* tmux still copies, because its `TSe()` picks `"native"` (`pbcopy`) on macOS before it considers a multiplexer.
  - Honouring OSC 52 would let any program in any tab overwrite the clipboard. That is a security-relevant policy, and an unattended run does not decide it (see Known Gaps).
  - Rejected: copying the app's highlighted text from the grid ourselves. The terminal cannot know the app's selection bounds, and it would race the app's own copy.

## Constraints
- C1: `crates/term-core` and `crates/renderer` are not modified, and the dependency direction holds (`common ← term-core ← renderer ← app`).
- C2: ⌘-click never writes mouse bytes to the PTY (hyperlinks D17/R4.11). Shift-drag still produces a terminal selection.
- C3: For any press, move and release sequence that starts and stays under `MouseProtocol::Off`, `Normal` or `X10`, the PTY receives byte-for-byte what it receives today. A sequence that starts under `?1002`/`?1003` and drops it mid-drag follows D3.
- C4: The existing mouse-button-reporting and scroll-snap tests pass unmodified (`crates/app/tests/tt1..tt4_*`).
- C5: `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings` stay green, and nothing calls `unwrap()` outside tests.

## Known Gaps
- **Manual check, not automatable here.** It covers the `CursorMoved` wiring (D6), which no headless test reaches. Run the terminal and check each case:
  1. Claude Code: plain-drag across a sentence, release, then `pbpaste`. The text appears.
  2. agy in its no-flicker mode: the same. agy was not measured live (the probe stopped at sign-in and folder trust), so its copy path, `pbcopy` or OSC 52, is unconfirmed.
  3. A `mouse on` tmux pane: a plain drag highlights in tmux copy-mode, but `pbpaste` does **not** change (D7). Shift-drag then ⌘C copies.
  4. R4.3, a `?1000`-only app: in a tab, run `printf '\e[?1000h\e[?1006h'; cat -v`. Drag from one cell to another and release. `cat -v` echoes exactly one `^[[<0;c;rM` and one `^[[<0;c;rm`, with the same c;r (the press cell) and no `<32;` motion lines. Ctrl-C, then `printf '\e[?1000l\e[?1006l'`.
- L2 fenced: security — whether to honour OSC 52 clipboard writes. Without it, a plain drag in tmux (`mouse on`), and in any app that copies only through OSC 52 (possibly agy), highlights but never reaches the system clipboard. It is a person's decision because it lets any program in a tab set the system clipboard.
- Opening the session graph (`Cmd+Shift+F`) mid-drag: its early return in `CursorMoved` (`terminal_app.rs:2702-2706`) runs first, so motion stops while the release still reaches the PTY. Out of scope.
- A tab switch in the middle of a reported drag (by keyboard chord) sends the rest of that drag's motion and its release to the new active tab. Press and release already share this, and it is out of scope here.
- Mutants in `terminal_app.rs` that no headless test reaches: deleting the `CursorMoved` branch, the `cmd_held` mask, or either release-arm refresh call. Their only guards are the manual checks R0.1, R0.2 and R0.4. The decisions inside them (`drag_owns_pointer`, `reported_drag_motion`) are pure and mutation-tested in T1.
- L2 provisional: none. L2 unresolved: none.

## Requirements

### R0: A plain drag in a mouse-tracking app reaches the app as a drag — D1, D3, D4, D6, D7
#### R0.1: Drag across three cells in Claude Code's mode writes press, motions, release (manual)
- **Given**: The active tab's terminal has `?1000h ?1002h ?1003h ?1006h` applied (effective `MouseProtocol::AnyEvent`, `MouseEncoding::Sgr`), as Claude Code 2.1.314 sends them.
- **When**: In `WindowEvent::MouseInput`, the left button is pressed over cell (2, 5) with no modifiers. `WindowEvent::CursorMoved` then moves the pointer over cells (3, 5) and (6, 5), and the button is released.
- **Then** (headless proxy: R2.7): The PTY receives exactly `\x1b[<0;3;6M`, `\x1b[<32;4;6M`, `\x1b[<32;7;6M`, `\x1b[<0;7;6m`, in that order, and no terminal selection is set on the tab.

#### R0.2: Sidebar and link hover do not change during a reported drag (manual)
- **Given**: A reported left-button drag is in progress (`mouse_reported` is `Some`) in a tab with the sidebar shown
- **When**: The pointer crosses a sidebar card, or ⌘ is held over a URL, before the button is released
- **Then**: No sidebar card hover and no link highlight appear until after the release. The drag's motion reports continue, clamped to column 0 (D4).

#### R0.4: ⌘ pressed mid-drag lights the link on release without motion, even after a focus loss (manual)
- **Given**: The window has lost and regained focus, which disarms the link hover. Claude Code is running in the active tab, and a URL is on screen.
- **When**: The user plain-drags in Claude Code, presses ⌘ during the drag, releases the button with the pointer resting on the URL, and ⌘-clicks without moving
- **Then**: No link lights during the drag. The release writes `ESC [ < 0 ; c ; r m` to the PTY even though ⌘ is held (R2.9). The URL lights at the release, and the ⌘-click opens it, writing no mouse bytes to the PTY (C2).

#### R0.3: Releasing a drag in Claude Code puts the text on the clipboard, with no OSC 52 from us (manual)
- **Given**: A build of this branch, with Claude Code (copy-on-select on, its default) running in the active tab, and an answer containing the word `selection` on screen
- **When**: The user plain-drags across that word, releases, and runs `pbpaste` in another tab
- **Then**: `pbpaste` prints the dragged text, and `crates/term-core/src/parser.rs` still has no OSC 52 handler (D7)

### R1: Motion report encoding — D1, D2
#### R1.1: SGR left-drag motion via `input::encode_mouse_motion_report`
- **Given**: `MouseEncoding::Sgr`
- **When**: `encode_mouse_motion_report(MouseEncoding::Sgr, MouseButton::Left, 10, 5)` is called
- **Then**: Returns `Some(b"\x1b[<32;11;6M".to_vec())`

#### R1.2: SGR middle and right drag codes via `input::encode_mouse_motion_report`
- **Given**: `MouseEncoding::Sgr`
- **When**: `encode_mouse_motion_report` is called with `MouseButton::Middle` and then with `MouseButton::Right`, both at (0, 0)
- **Then**: Returns `Some(b"\x1b[<33;1;1M".to_vec())` and `Some(b"\x1b[<34;1;1M".to_vec())`

#### R1.3: Legacy left-drag motion via `input::encode_mouse_motion_report`
- **Given**: `MouseEncoding::Legacy`
- **When**: `encode_mouse_motion_report(MouseEncoding::Legacy, MouseButton::Left, 10, 5)` is called
- **Then**: Returns `Some(vec![0x1b, b'[', b'M', 64, 43, 38])`

#### R1.4: Legacy coordinate past 223 drops the report via `input::encode_mouse_motion_report`
- **Given**: `MouseEncoding::Legacy`
- **When**: `encode_mouse_motion_report(MouseEncoding::Legacy, MouseButton::Left, 250, 5)` is called
- **Then**: Returns `None`

#### R1.5: Buttons other than Left, Middle, Right are not encoded via `input::encode_mouse_motion_report`
- **Given**: `MouseEncoding::Sgr`
- **When**: `encode_mouse_motion_report(MouseEncoding::Sgr, MouseButton::Back, 1, 1)` is called
- **Then**: Returns `None`

### R2: When a motion report is sent, where the latch goes, and who owns the pointer — D1, D2, D3, D6
#### R2.1: A new cell under `ButtonEvent` reports and moves the latch via `input::reported_drag_motion`
- **Given**: `latch = Some((MouseButton::Left, 2, 5))`, `MouseProtocol::ButtonEvent`, `MouseEncoding::Sgr`
- **When**: `reported_drag_motion(&mut latch, MouseProtocol::ButtonEvent, MouseEncoding::Sgr, (3, 5))` is called
- **Then**: Returns `Some(b"\x1b[<32;4;6M".to_vec())`, and `latch == Some((MouseButton::Left, 3, 5))`

#### R2.2: A new cell under `AnyEvent` reports via `input::reported_drag_motion`
- **Given**: `latch = Some((MouseButton::Left, 2, 5))`, `MouseProtocol::AnyEvent`, `MouseEncoding::Sgr`
- **When**: `reported_drag_motion(&mut latch, MouseProtocol::AnyEvent, MouseEncoding::Sgr, (2, 6))` is called
- **Then**: Returns `Some(b"\x1b[<32;3;7M".to_vec())`, and `latch == Some((MouseButton::Left, 2, 6))`

#### R2.3: The same cell sends nothing via `input::reported_drag_motion`
- **Given**: `latch = Some((MouseButton::Left, 2, 5))`, `MouseProtocol::AnyEvent`
- **When**: `reported_drag_motion(&mut latch, MouseProtocol::AnyEvent, MouseEncoding::Sgr, (2, 5))` is called
- **Then**: Returns `None`, and `latch` is unchanged

#### R2.4: Click-only and off protocols send no motion via `input::reported_drag_motion`
- **Given**: `latch = Some((MouseButton::Left, 2, 5))`
- **When**: `reported_drag_motion` is called with `(3, 5)` under each of `MouseProtocol::Normal`, `MouseProtocol::X10` and `MouseProtocol::Off`
- **Then**: Returns `None` for all three, and `latch` stays `Some((MouseButton::Left, 2, 5))`

#### R2.5: No latched button sends nothing via `input::reported_drag_motion`
- **Given**: `latch = None`, `MouseProtocol::AnyEvent`
- **When**: `reported_drag_motion(&mut latch, MouseProtocol::AnyEvent, MouseEncoding::Sgr, (3, 5))` is called
- **Then**: Returns `None`, and `latch` stays `None`. Hover motion with no button held stays unreported (Non-goals).

#### R2.6: A legacy cell past 223 sends nothing and keeps the latch via `input::reported_drag_motion`
- **Given**: `latch = Some((MouseButton::Left, 2, 5))`, `MouseProtocol::ButtonEvent`, `MouseEncoding::Legacy`
- **When**: `reported_drag_motion(&mut latch, MouseProtocol::ButtonEvent, MouseEncoding::Legacy, (250, 5))` is called
- **Then**: Returns `None`, and `latch` stays `Some((MouseButton::Left, 2, 5))`

#### R2.7: Press, two moves and release write the full drag sequence via `input::grid_press_action`, `input::reported_drag_motion`, `input::grid_release_action`
- **Given**: `MouseProtocol::AnyEvent`, `MouseEncoding::Sgr`, no modifiers. `grid_press_action(…, MouseButton::Left, (2, 5), …)` returns `Report(b)`, and the test sets `latch = Some((MouseButton::Left, 2, 5))`, as the event loop does.
- **When**: `reported_drag_motion(&mut latch, …, (3, 5))`, then `reported_drag_motion(&mut latch, …, (6, 5))`, then `grid_release_action(latch, MouseProtocol::AnyEvent, MouseEncoding::Sgr, MouseButton::Left, &ModifiersState::empty())` are called
- **Then**: The concatenated bytes are `\x1b[<0;3;6M`, `\x1b[<32;4;6M`, `\x1b[<32;7;6M`, `\x1b[<0;7;6m`, so the release lands on the last moved cell

#### R2.8: Tracking narrowed to `?1000` mid-drag releases at the last moved cell via `input::reported_drag_motion` and `input::grid_release_action`
- **Given**: `latch = Some((MouseButton::Left, 2, 5))` and `MouseEncoding::Sgr`. A move to (4, 5) under `AnyEvent` has moved the latch.
- **When**: The protocol becomes `MouseProtocol::Normal`, `reported_drag_motion(&mut latch, MouseProtocol::Normal, MouseEncoding::Sgr, (7, 5))` is called, and then `grid_release_action(latch, MouseProtocol::Normal, …, MouseButton::Left, …)`
- **Then**: The motion call returns `None`, and the release returns `Report(b"\x1b[<0;5;6m".to_vec())`

#### R2.9: A latched release reports even with ⌘ or Shift held via `input::grid_release_action`
- **Given**: `latch = Some((MouseButton::Left, 6, 5))`, `MouseProtocol::AnyEvent`, `MouseEncoding::Sgr`
- **When**: `grid_release_action(latch, MouseProtocol::AnyEvent, MouseEncoding::Sgr, MouseButton::Left, &ModifiersState::SUPER)` is called, and again with `&ModifiersState::SHIFT`
- **Then**: Both return `GridReleaseAction::Report(b"\x1b[<0;7;6m".to_vec())`. With `MouseProtocol::Off`, the same call returns `GridReleaseAction::Ignore`.

#### R2.10: Only a latched press under `?1002`/`?1003` owns the pointer via `input::drag_owns_pointer`
- **Given**: `latch = Some((MouseButton::Left, 2, 5))`
- **When**: `drag_owns_pointer(latch, p)` is called for each `p` in `AnyEvent`, `ButtonEvent`, `Normal`, `X10` and `Off`, and `drag_owns_pointer(None, MouseProtocol::AnyEvent)` is called
- **Then**: Returns `true` for `AnyEvent` and `ButtonEvent`, and `false` for `Normal`, `X10`, `Off`, and for the `None` latch

### R3: An off-grid pointer during a reported drag clamps to the edge — D4
#### R3.1: Left of and above the grid maps to column 0 and row 0 via `input::pixel_to_cell_clamped`
- **Given**: Cell size (10.0, 20.0), `left_inset` 200.0, `top_inset` 30.0, `viewport_w` 1000.0, `right_inset` 0.0, 80 cols, 24 rows
- **When**: `pixel_to_cell_clamped((50.0, 10.0), …)` is called
- **Then**: Returns `Some((0, 0))`, where `pixel_to_cell` returns `None` for the same input

#### R3.2: The monitor band and beyond the bottom map to the last column and last row via `input::pixel_to_cell_clamped`
- **Given**: Cell size (10.0, 20.0), `left_inset` 0.0, `top_inset` 0.0, `viewport_w` 1000.0, `right_inset` 200.0, 80 cols, 24 rows
- **When**: `pixel_to_cell_clamped((950.0, 5000.0), …)` is called
- **Then**: Returns `Some((79, 23))`

#### R3.3: An in-grid pointer matches `pixel_to_cell` via `input::pixel_to_cell_clamped`
- **Given**: Cell size (10.0, 20.0), insets 0.0, `viewport_w` 1000.0, `right_inset` 0.0, 80 cols, 24 rows
- **When**: `pixel_to_cell_clamped((125.0, 45.0), …)` and `pixel_to_cell((125.0, 45.0), …)` are called
- **Then**: Both return `Some((12, 2))`

#### R3.4: Degenerate geometry gives no cell via `input::pixel_to_cell_clamped`
- **Given**: `cols == 0`, or a cell size of (0.0, 20.0)
- **When**: `pixel_to_cell_clamped` is called with any cursor
- **Then**: Returns `None`

#### R3.5: Negative coordinates and the top inset clamp to row 0 and column 0 via `input::pixel_to_cell_clamped`
- **Given**: Cell size (10.0, 20.0), `left_inset` 200.0, `top_inset` 30.0, `viewport_w` 1000.0, `right_inset` 0.0, 80 cols, 24 rows
- **When**: `pixel_to_cell_clamped((-40.0, -15.0), …)` and `pixel_to_cell_clamped((215.0, 29.0), …)` are called
- **Then**: Returns `Some((0, 0))` and `Some((1, 0))`

### R4: Bypasses and click-only apps are unchanged — D5
#### R4.1: Shift-press arms terminal selection, so no motion can follow via `input::grid_press_action`
- **Given**: `MouseProtocol::AnyEvent`, Shift held
- **When**: `grid_press_action(MouseProtocol::AnyEvent, MouseEncoding::Sgr, MouseButton::Left, (2, 5), &ModifiersState::SHIFT)` is called
- **Then**: Returns `GridPressAction::Drag`. No `mouse_reported` latch is set, and `reported_drag_motion(&mut None, …)` returns `None` (R2.5).

#### R4.2: ⌘-press never latches a report via `input::grid_press_action`
- **Given**: `MouseProtocol::AnyEvent`, ⌘ held
- **When**: `grid_press_action(MouseProtocol::AnyEvent, MouseEncoding::Sgr, MouseButton::Left, (2, 5), &ModifiersState::SUPER)` is called
- **Then**: Returns `GridPressAction::Drag`, never `Report`

#### R4.3: A `?1000`-only drag writes only press and release at the press cell (manual)
- **Given**: The app has enabled only `?1000h ?1006h`
- **When**: The user presses at (2, 5), drags to (6, 5) and releases (recipe: Known Gaps manual check 4)
- **Then**: The PTY receives `\x1b[<0;3;6M` and `\x1b[<0;3;6m` only, the same as before this change


## Tasks

### T1: Motion encoding, latch-moving motion step, and clamped cell lookup in `input.rs` [infra]
- **Fulfills**: R1, R2, R3
- **Depends on**: (none)
- **Target**: `crates/app/src/input.rs`, `crates/app/src/lib.rs`
- **Details**:
  - Add these functions next to `grid_release_action` (`input.rs:3479`), each with a doc comment citing this spec's decision ids, in the style of the surrounding mouse-button-reporting functions:
    - `pub fn encode_mouse_motion_report(encoding: MouseEncoding, button: MouseButton, col: u16, row: u16) -> Option<Vec<u8>>`, per D1;
    - `pub fn reported_drag_motion(latch: &mut Option<(MouseButton, u16, u16)>, protocol: MouseProtocol, encoding: MouseEncoding, cell: (u16, u16)) -> Option<Vec<u8>>`, per D1, D2 and D3.
  - Add `pub fn pixel_to_cell_clamped(...)`, with `pixel_to_cell`'s parameter list and its `#[allow(clippy::too_many_arguments)]` attribute (`input.rs:1066`), next to `pixel_to_cell`, per D4. Compute in `f32`, clamp a negative offset to 0, then apply `.min(cols - 1)` and `.min(rows - 1)`.
  - Re-export all three from `crates/app/src/lib.rs` beside the existing `encode_mouse_button_report` group (`lib.rs:92-94`). Also re-export the existing `pixel_to_cell`, which is not re-exported today, so TT3 can compare the two.
  - Add inline `#[cfg(test)]` unit tests covering R1.1–R1.5, R2.1–R2.10 and R3.1–R3.5.
  - Add `pub fn drag_owns_pointer(latch: Option<(MouseButton, u16, u16)>, protocol: MouseProtocol) -> bool`, per D6, and re-export it.
  - In `grid_release_action`, pass `&ModifiersState::empty()` instead of `mods` to `mouse_button_bytes` (D3). Keep the `mods` parameter (now unused, renamed `_mods`) so the signature and its callers do not change.
  - Do not change `grid_press_action`, `mouse_button_bytes` or `pixel_to_cell` (C4).
  - Verify:
    - `cargo test -p app`;
    - `cargo clippy --workspace --all-targets -- -D warnings`;
    - `cargo mutants -f crates/app/src/input.rs -F 'encode_mouse_motion_report|reported_drag_motion|pixel_to_cell_clamped|drag_owns_pointer|grid_release_action' -j 4`. Record the survivor count. Any survivor is a missing test unless it is provably equivalent (CLAUDE.md).

### TT1: acceptance tests for R1's GWTs
- **Fulfills**: R1 (tests)
- **Depends on**: T1
- **Target**: `crates/app/tests/tt1_mouse_motion_encoding.rs` (new)
- **Details**: One test per R1.1–R1.5, calling `app::encode_mouse_motion_report`. Imports follow `crates/app/tests/tt1_mouse_button_encoding.rs:11-14`.

### TT2: acceptance tests for R2's GWTs
- **Fulfills**: R2 (tests)
- **Depends on**: T1
- **Target**: `crates/app/tests/tt2_reported_drag_motion.rs` (new)
- **Details**: One test per R2.1–R2.10, calling `app::drag_owns_pointer`, `app::reported_drag_motion`, `app::grid_press_action` and `app::grid_release_action`. Assert the returned bytes and the latch value after each call.

### TT3: acceptance tests for R3's GWTs
- **Fulfills**: R3 (tests)
- **Depends on**: T1
- **Target**: `crates/app/tests/tt3_drag_cell_clamp.rs` (new)
- **Details**: One test per R3.1–R3.5, calling `app::pixel_to_cell_clamped`. R3.1 and R3.3 also call `app::pixel_to_cell`, which T1 re-exports.

### TT4: acceptance tests for R4's GWTs
- **Fulfills**: R4 (tests)
- **Depends on**: T1
- **Target**: `crates/app/tests/tt4_drag_bypass.rs` (new)
- **Details**: Tests for R4.1 and R4.2. These call `app::grid_press_action` with `ModifiersState::SHIFT` and with `ModifiersState::SUPER`, assert `GridPressAction::Drag`, and for R4.1 assert that `reported_drag_motion(&mut None, …)` returns `None`. R4.3 is manual, and the requirement check covers it by reading `reported_drag_motion`'s `Normal` arm (R2.4).

### T2: Wire reported-drag motion into `WindowEvent::CursorMoved` [vertical]
- **Fulfills**: R0, R4
- **Depends on**: T1, TT4
- **Target**: `crates/app/src/terminal_app.rs`
- **Details**: Implements D6 in the `WindowEvent::CursorMoved` arm (`terminal_app.rs:2683-2724`):
  - After the resize-drag and graph early returns, and **before** `inner.refresh_hover()` / `inner.refresh_link_hover(..)` (`:2709-2710`): if `input::drag_owns_pointer(inner.mouse_reported, active.terminal.mouse_protocol())`, then:
    1. compute `input::pixel_to_cell_clamped` with the same arguments `cursor_cell()` passes to `pixel_to_cell` (`terminal_app.rs:2014-2026`);
    2. read the active tab's `terminal.mouse_protocol()` and `mouse_encoding()`;
    3. call `reported_drag_motion(&mut inner.mouse_reported, protocol, encoding, cell)`;
    4. on `Some(bytes)`, call `inner.tabs.active_mut().write(&bytes)`, logging an `Err` with `tracing::warn!("pty write failed: {e}")`;
    5. `return`.
  - Write a comment above the branch, matching the resize-drag comment's style, that names this spec and says a reported drag owns the pointer.
  - In `fn refresh_link_hover` (`terminal_app.rs:2041`), change `cmd_held: self.mods.super_key()` to `cmd_held: self.mods.super_key() && !input::drag_owns_pointer(self.mouse_reported, self.tabs.active().terminal.mouse_protocol())`, with a comment citing D6. It is a mask, not an early return, so `on_modifier` still re-arms (R0.2, R0.4).
  - At the end of the reported-release arm (`terminal_app.rs:2918-2936`), **outside** the `if let GridReleaseAction::Report(..)`, call `inner.refresh_hover()` and then `inner.refresh_link_hover(LinkTrigger::Snapshot)` (R0.4).
  - Leave the latch handling in the press arms, the release arm and the focus-loss release as it is. Those arms already read the latch, which now moves.
  - Verify with `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings`. Then run the manual checks R0.1, R0.2, R0.3, R0.4 and R4.3 through `/run-term` if a display is available, and otherwise record them as pending in the report.

## External Dependencies

### Pre-work
- (none)

### Post-work
- Manual check (Known Gaps): drag-and-`pbpaste` in Claude Code, agy no-flicker mode, and a `mouse on` tmux pane, in a build of this branch.
- A person decides the `L2 fenced: security` OSC 52 question if the agy or tmux results call for it.

## Spec Review
- **Verdict**: NEEDS_FIX (final, review 3 of 3). The run stopped before execution, and `Approved by` is not written.
- **Reviews**: 3. L2-reviewer: 4 runs (NEEDS_FIX, PASS, NEEDS_FIX, NEEDS_FIX). Its last NEEDS_FIX covered the same release issue.
- **Fixed** (reviews 1–2):
  - [Consistency] R0.2/D6: ⌘ pressed mid-drag lit links through the `Modifier`/`Snapshot` triggers. Fixed by gating `cmd_held`, as a mask rather than an early return, so `on_modifier` still re-arms.
  - [Coverage] R4.3: the manual check had no runner. Added Known Gaps manual check 4 (a `printf … ; cat -v` recipe) and listed it in T2's verify step.
  - [Readiness] T1: named the `#[allow(clippy::too_many_arguments)]` that `pixel_to_cell_clamped` must carry.
- **Applied after review 3, not re-reviewed**:
  - [Consistency] D3/R0.4: a ⌘- or Shift-held release dropped the latched release (`mouse_button_route` bypass), stranding the app's drag. D3 now says a latched report always gets its release (`grid_release_action` ignores modifiers). R2.9 and a T1 bullet were added.
  - [Consistency] D6/T2 vs D1: the hover freeze and the `cmd_held` mask were gated only on the latch, which also froze hover for `?1000`/`?9` and Right/Middle holds. They now gate on the new pure `input::drag_owns_pointer` (latch and `?1002`/`?1003`). Added R2.10.
  - [Readiness] T2/D6: the release-arm refresh calls go at the end of the arm, outside the `if let`. D6's placement now says "before `refresh_hover`" (`:2709-2710`), and T2's "leave the arms as they are" wording was fixed.
  - [L2] Known Gaps: recorded the `terminal_app.rs` mutants that only manual checks guard.
- **Open**:
  - (none). The user approved D3 on 2026-10-30 and kept OSC 52 out of scope (the `L2 fenced` gap stays closed as a non-goal).
