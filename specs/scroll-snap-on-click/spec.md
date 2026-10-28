# Spec: scroll-snap-on-click

## Meta
- **Created**: 2026-10-25
- **Type**: dev
- **Status**: approved
- **Mode**: unattended
- **Approved by**: unattended (request marker)
- **Approved at**: 2026-10-25T21:05:40-07:00

## Goal
Could you build Option B?

## Non-goals
- Rendering a custom floating "Jump to bottom" visual button or scrollbar overlay in `crates/renderer` (out of scope for app layer, requires renderer/shader changes)
- Alternate Scroll Mode under `?1049h` converting wheel events to Up/Down arrow key escape sequences (Option A, distinct feature)
- Snapping to bottom when the user performed an intentional text selection drag (`drag_moved == true`)
- Intercepting mouse clicks when mouse reporting is active (`MouseProtocol != Off`), which must continue forwarding clicks to the child TUI (such as Claude Code)

## Confirmed Goal
When the terminal viewport is scrolled back (`view_offset > 0`) in an active tab whose child application has not enabled mouse tracking (`MouseProtocol::Off`), a mouse left-click on the terminal grid (without dragging) snaps the viewport back to the live bottom (`view_offset = 0`), allowing users to easily escape scrollback without typing keyboard input (inferred). Text selection drags, ⌘-click hyperlink navigation, Shift-click selection bypass, and mouse-reporting TUIs remain unaffected (inferred). Done when:
1. Pure decision logic in `input.rs` determines whether a grid release qualifies as a click-to-bottom snap.
2. Clicking the grid when `view_offset > 0` and mouse tracking is off snaps `view_offset` to 0.
3. Dragging to select text while scrolled back retains the selection and does not snap to bottom.
4. ⌘-click opens hyperlinks and does not trigger snap-to-bottom.
5. All workspace unit, integration, clippy, and formatting checks pass cleanly.


## Research
- `crates/app/src/terminal_app.rs:2962-3015`: `WindowEvent::MouseInput { state: ElementState::Released, button: MouseButton::Left, .. }` handles `ReleaseTarget::Selection`. Resolves `url_to_open`, calls `drag::release`, replays `drag_release_snap(latch)`, and opens URLs. Currently does not snap when `view_offset > 0` and `is_click` is true.
- `crates/app/src/terminal_app.rs:2164-2182`: `snap_tab_to_live(id)` checks `!terminal.at_bottom()`, calls `terminal.scroll_to_bottom()`, redraws, and rescans agent card status at live bottom.
- `crates/app/src/drag.rs:165-167`: `drag::is_click(drag, active)` returns true if pointer never left the press cell (`!d.moved`), never crossed tabs (`!d.left_tab`), and landed on the active tab (`d.tab == active`).
- `crates/app/src/input.rs:458-480`: `ReleaseTarget` enum (`Resize`, `Selection`, `Neither`) and `release_target` pure resolution function.
- `crates/app/src/input.rs:3347-3353`: `mouse_button_route` bypasses mouse reporting when `!protocol.is_on()` or when `super_key()` or `shift_key()` is pressed.
- `crates/app/src/terminal_app.rs:2880-2892`: When mouse reporting is bypassed (`GridPressAction::Drag`), left press initializes `SelectionDrag::press` and sets `inner.dragging = true`.
- `crates/term-core/src/lib.rs:140-147`: `Terminal::at_bottom(&self) -> bool` reports whether `view_offset == 0`. `Terminal::scroll_to_bottom(&mut self)` sets `view_offset = 0`.
- `specs/terminal-scroll/spec.md:121-125, 139`: D12 documents `SNAP_FLOOR = 3` for auto-snapping on output, and Known Gaps notes that no visible affordance or button exists to jump to bottom, making keyboard input the only prior way to escape scrollback.

## Decisions

### D1: Pure click-to-bottom decision function `should_snap_on_click` in `input.rs`
- **Status**: assumed
- **Rationale**: Isolates the predicate that decides whether a mouse button release on the terminal grid should snap the viewport to live bottom into a pure, GPU-free, window-free function in `crates/app/src/input.rs`. Takes `is_click: bool`, `has_url: bool`, `mods: &ModifiersState`, and `at_bottom: bool`. Returns `true` if and only if:
  1. The gesture was a genuine click without movement (`is_click == true`),
  2. No hyperlink was opened (`!has_url`),
  3. No modifier keys were held (`mods.is_empty()`, ensuring Shift, Cmd, Alt, and Ctrl are all clear),
  4. The viewport is not already at live bottom (`!at_bottom`).
  Requiring `mods.is_empty()` ensures ⌘-click (hyperlinks), Shift-click (text selection bypass), Ctrl-click (macOS right-click emulation), and Option-click (block selection / chord bindings) are never hijacked by snap-to-bottom.
  Rejected alternative: inline condition checking directly within `terminal_app.rs` event loop, which prevents exhaustive unit testing of modifier and drag combinations without spinning up a winit event loop.

### D2: Integration point in `terminal_app.rs` under `ReleaseTarget::Selection`
- **Status**: assumed
- **Rationale**: In `WindowEvent::MouseInput { state: ElementState::Released, button: MouseButton::Left, .. }` when `inner.release_target() == ReleaseTarget::Selection`:
  Bind `let is_click = drag::is_click(inner.drag, active);` BEFORE `inner.drag.take()` is called.
  Evaluate `let should_snap = should_snap_on_click(is_click, url.is_some(), &inner.mods, inner.tabs.active().terminal.at_bottom());`.
  After clearing the selection via `drag::release(inner.drag.take(), active, ...)`, replay latched snaps and click snaps independently:
  ```rust
  let latch = std::mem::take(&mut inner.drag_latch);
  if let Some(id) = drag_release_snap(latch) {
      inner.snap_tab_to_live(id);
  }
  if should_snap {
      inner.snap_tab_to_live(active);
  }
  ```
  Executing both independently guarantees that background tab output that arrived during the drag snaps that background tab (`id`), while the user's click snap on the active tab (`active`) is never dropped. Since `snap_tab_to_live` is guarded by `!terminal.at_bottom()`, this is completely idempotent when `id == active`.
  Ordering invariant: `is_click` must be evaluated before `inner.drag.take()`, because once consumed, `drag::is_click(None, active)` returns true and corrupts drag-release semantics.
  Rejected alternative: chaining via `else if should_snap`, which would drop the active tab's click snap whenever background output arrived during the click gesture.
  Rejected alternative: snapping in `ElementState::Pressed`, which would prematurely snap the viewport before knowing whether the user intended a drag-selection or an in-place click.
  Rejected alternative: snapping inside `drag::release`, which violates module boundary separation because `drag.rs` only manages selection state, not viewport offsets or tab switching.

### D3: Strict preservation of text selection drags, modifier bypasses, and UX trade-offs
- **Status**: assumed
- **Rationale**: If the user holds the left button and moves the cursor across cells (`!is_click`, i.e., `drag_moved == true`), the selection must stay intact for copying via ⌘-C; the viewport must remain at its scrolled offset and never snap to bottom.
  If any modifier key is held (`!mods.is_empty()`), snap-to-bottom is inhibited. Specifically, ⌘ is reserved for hyperlinks (`specs/hyperlinks/spec.md` D17/R4.11), Shift is reserved for forced selection, and Alt/Option is reserved for future box/rectangular selection or shortcuts.
  **Steelman UX trade-off acknowledgment**: By design of Option B, an unmodified left-click anywhere on the grid while scrolled back snaps to the live bottom. Consequently:
  1. A click used to clear an existing text selection while scrolled back will also snap to bottom (the user can deselect while preserving scrollback by pressing Shift-click or Esc).
  2. A click-through into the terminal window from another application on macOS will snap to bottom if the window was scrolled back. This trade-off is accepted as the fundamental definition of the "click anywhere to jump to bottom" escape hatch.
  Rejected alternative: requiring a multi-pixel deadzone or complex double-click gesture to jump to bottom, which defeats the immediacy of Option B.

### D4: Zero interaction with mouse-reporting TUIs
- **Status**: assumed
- **Rationale**: When child applications enable mouse tracking (`MouseProtocol != Off`), `grid_press_action` routes left clicks to `GridPressAction::Report(bytes)` and arms `inner.mouse_reported`.
  Their release is handled by the `mouse_reported` release arm (`terminal_app.rs:2918-2936`), writing release bytes to the PTY. Such events never set `ReleaseTarget::Selection` and therefore never trigger `should_snap_on_click`. Claude Code and other mouse-aware TUIs continue receiving clicks without any interference.
  Rejected alternative: intercepting clicks over mouse-reporting TUIs when `view_offset > 0`, which would break in-TUI buttons like Claude Code's native "Jump to bottom (click) ↓".

## Constraints
- C1: `crates/term-core` and `crates/renderer` public APIs are consumed as shipped; zero changes outside `crates/app` (`common ← term-core ← renderer ← app`).
- C2: `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings` must stay green with zero warnings.
- C3: ⌘-click hyperlink opening and Shift-click selection behavior must remain 100% intact.
- C4: No new external crate dependencies.

## Known Gaps
- **Deselection snaps scrollback**: Clicking to clear an active selection while `view_offset > 0` snaps to live bottom. Users wishing to deselect without snapping can use Shift-click or press an arrow key (accepted trade-off of Option B).


## Requirements

### R0: Snap to live bottom on grid click when scrolled back (goal)

#### R0.1: Grid click when scrolled back returns to live bottom
- **Given**: An active tab terminal with `terminal.at_bottom() == false` and `terminal.view_offset() == 15`
- **When**: Left click occurs with `is_click == true`, `has_url == false`, and `mods.is_empty() == true`
- **Then**: `should_snap_on_click` returns `true`, and applying bottom snap resets `terminal.view_offset()` to 0 and `terminal.at_bottom()` to `true`

#### R0.2: Grid click when already at live bottom does not re-snap
- **Given**: An active tab terminal with `terminal.at_bottom() == true` and `terminal.view_offset() == 0`
- **When**: Left click occurs with `is_click == true`, `has_url == false`, and `mods.is_empty() == true`
- **Then**: `should_snap_on_click` returns `false`, so no snap call is triggered

### R1: Pure decision predicate `should_snap_on_click` — D1

#### R1.1: Unmodified click while scrolled back returns true
- **Given**: `is_click = true`, `has_url = false`, `mods = ModifiersState::empty()`, and `at_bottom = false`
- **When**: `should_snap_on_click(is_click, has_url, &mods, at_bottom)` is called
- **Then**: Returns `true`

#### R1.2: Moving drag returns false
- **Given**: `is_click = false`, `has_url = false`, `mods = ModifiersState::empty()`, and `at_bottom = false`
- **When**: `should_snap_on_click(is_click, has_url, &mods, at_bottom)` is called
- **Then**: Returns `false`

#### R1.3: Click over lit hyperlink returns false
- **Given**: `is_click = true`, `has_url = true`, `mods = ModifiersState::SUPER`, and `at_bottom = false`
- **When**: `should_snap_on_click(is_click, has_url, &mods, at_bottom)` is called
- **Then**: Returns `false`

#### R1.4: Modifiers (Shift, Super, Alt, Ctrl) return false
- **Given**: `is_click = true`, `has_url = false`, `at_bottom = false`, and `mods` is non-empty (any of `SHIFT`, `SUPER`, `ALT`, `CONTROL`)
- **When**: `should_snap_on_click(is_click, has_url, &mods, at_bottom)` is called
- **Then**: Returns `false`

#### R1.5: Already at bottom returns false
- **Given**: `is_click = true`, `has_url = false`, `mods = ModifiersState::empty()`, and `at_bottom = true`
- **When**: `should_snap_on_click(is_click, has_url, &mods, at_bottom)` is called
- **Then**: Returns `false`

### R2: Selection drag release integration — D2

#### R2.1: Selection release triggers active snap when `should_snap` is true
- **Given**: Active tab terminal with `terminal.at_bottom() == false` and `ReleaseTarget::Selection`
- **When**: `should_snap_on_click(true, false, &ModifiersState::empty(), false)` returns `true`
- **Then**: Viewport snaps to bottom, resulting in `terminal.at_bottom() == true` and `terminal.view_offset() == 0`

#### R2.2: Replay of background tab drag-latch does not drop active click snap
- **Given**: Background tab latched in `drag_latch` and active tab click with `should_snap == true`
- **When**: The release arm in `terminal_app.rs` runs
- **Then**: The release block independently checks both `drag_release_snap(latch)` and `if should_snap`, ensuring both the background tab and active tab are snapped without short-circuiting

### R3: Preservation of selection drag and modifier bypasses — D3

#### R3.1: Text selection drag leaves selection standing and does not snap
- **Given**: `Terminal` with `view_offset == 10` and `SelectionDrag` where pointer moved (`is_click == false`)
- **When**: `should_snap_on_click(is_click, false, &mods, false)` is evaluated
- **Then**: Returns `false`, and `terminal.view_offset()` remains 10

#### R3.2: ⌘-click opens link without snapping
- **Given**: Pointer over hyperlink with `mods.super_key() == true`
- **When**: `url_to_open` evaluates to `Some(...)` and `should_snap_on_click(is_click, true, &mods, at_bottom)` is evaluated
- **Then**: `should_snap_on_click` returns `false`, and `terminal.view_offset()` remains unchanged

#### R3.3: Shift-click manual selection bypass does not snap
- **Given**: `mods.shift_key() == true`
- **When**: `should_snap_on_click(is_click, false, &mods, at_bottom)` is evaluated
- **Then**: Returns `false`

### R4: Isolation of mouse-reporting applications — D4

#### R4.1: Grid click in mouse-reporting application routes to PTY and does not snap
- **Given**: Active tab whose child application has enabled mouse reporting (`MouseProtocol::AnyEvent`, `MouseEncoding::Sgr`) and `view_offset == 5`
- **When**: User clicks with left mouse button at `(10, 10)` with no modifiers
- **Then**: `grid_press_action` yields `GridPressAction::Report(_)`, `inner.mouse_reported` is armed, `ReleaseTarget::Selection` is never entered, and `should_snap_on_click` is never evaluated

## Tasks

### TT1: Acceptance tests for should_snap_on_click predicate [test]
- **Fulfills**: R1 (tests)
- **Depends on**: (none)
- Test cases for `should_snap_on_click`:
  - `is_click=true`, `has_url=false`, `mods.is_empty()=true`, `at_bottom=false` -> `true`
  - `is_click=false` (drag moved) -> `false`
  - `has_url=true` (hyperlink lit) -> `false`
  - `mods` has `SHIFT`, `SUPER`, `ALT`, or `CONTROL` -> `false`
  - `at_bottom=true` -> `false`
  Place in `crates/app/tests/tt1_scroll_snap_predicate.rs`.

### TT2: Acceptance tests for selection drag release snap integration [test]
- **Fulfills**: R0, R2 (tests)
- **Depends on**: (none)
- Test cases for snap mechanics and event loop ordering:
  - Verify `term_core::Terminal` scrollback and snap semantics (`scroll_to_bottom` resets `view_offset` to 0 and `at_bottom` to `true`).
  - Source-scan structural test (following `drag.rs:483-528`): assert that in `terminal_app.rs` under `ReleaseTarget::Selection`, `drag::is_click(inner.drag, active)` is bound to `let is_click` BEFORE `inner.drag.take()` is called, and `should_snap` is evaluated before `drag::release`.
  - Source-scan test asserting that `inner.snap_tab_to_live(active)` is invoked under `if should_snap` independently after `drag_release_snap(latch)`.
  Place in `crates/app/tests/tt2_scroll_snap_integration.rs`.

### TT3: Acceptance tests for drag preservation and modifier bypass [test]
- **Fulfills**: R3 (tests)
- **Depends on**: (none)
- Test cases:
  - Dragging over cells (`is_click = false`) produces `should_snap_on_click == false` and preserves `terminal.view_offset() > 0`.
  - ⌘-click (`mods.super_key() == true` or `has_url == true`) produces `should_snap_on_click == false` and preserves `terminal.view_offset() > 0`.
  - Shift-click (`mods.shift_key() == true`) produces `should_snap_on_click == false` and preserves `terminal.view_offset() > 0`.
  Place in `crates/app/tests/tt3_scroll_snap_drag_bypass.rs`.

### TT4: Acceptance tests for mouse-reporting TUI isolation [test]
- **Fulfills**: R4 (tests)
- **Depends on**: (none)
- Test cases:
  - When `term.mouse_protocol().is_on() == true`, `grid_press_action` produces `GridPressAction::Report` and `grid_release_action` produces `GridReleaseAction::Report`, routing events to the PTY and bypassing `ReleaseTarget::Selection`.
  Place in `crates/app/tests/tt4_scroll_snap_tui_isolation.rs`.

### T1: Implement should_snap_on_click in input.rs [infra]
- **Fulfills**: R1
- **Depends on**: TT1
- Add `pub fn should_snap_on_click(is_click: bool, has_url: bool, mods: &ModifiersState, at_bottom: bool) -> bool` to `crates/app/src/input.rs` and re-export in `crates/app/src/lib.rs`.
- Wire `is_click && !has_url && mods.is_empty() && !at_bottom`.

### T2: Integrate click-to-bottom snap in terminal_app.rs [vertical]
- **Fulfills**: R0, R2, R3, R4
- **Depends on**: T1, TT2, TT3, TT4
- In `crates/app/src/terminal_app.rs` under `ReleaseTarget::Selection`:
  - Explicitly bind `let is_click = drag::is_click(inner.drag, active);` before `inner.drag.take()`.
  - Update `ClickInput` to use `drag_moved: !is_click`.
  - Evaluate `let should_snap = should_snap_on_click(is_click, url.is_some(), &inner.mods, inner.tabs.active().terminal.at_bottom());` prior to `drag::release(inner.drag.take(), ...)`.
  - After `drag::release` and `drag_release_snap(latch)`, add:
    ```rust
    if should_snap {
        inner.snap_tab_to_live(active);
    }
    ```
  - Verify `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings`.

## External Dependencies
(none)

## Spec Review
- **Date**: 2026-10-25
- **Reviewer**: Subagent `03cafa25-3286-48bb-919f-cd6ce9ea8b4b`
- **Verdict**: PASS
- **Notes**: ISSUES: none. All 6 verification checks (Coverage, Testability, Grounding, Task order, Consistency, Readiness) passed.
