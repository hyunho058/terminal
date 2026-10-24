# Spec: mouse-button-reporting

## Meta
- **Created**: 2026-10-24
- **Type**: dev
- **Status**: drafting
- **Mode**: unattended
- **Approved by**: unattended (request marker)
- **Approved at**: 2026-10-24


## Goal
jump to bottom button is not working(/Users/hyunhokim/.cache/mac-terminal/pasted/paste-1792866370-0.png ). So You should check and fix it.  mode: unattended

## Non-goals
- Continuous mouse hover motion reporting without buttons pressed (`?1003` hover stream without button down)
- Custom terminal-side scrollbar or native scroll-to-bottom overlay rendering (Claude Code already renders its own button inside the terminal stream)
- Modifying `crates/renderer` or altering how fonts, cards, or tabs are rendered
- Altering ⌘-click hyperlink opening behavior (which must remain functional and never send mouse bytes to the PTY)

## Confirmed Goal
Forward mouse button press and release events over the terminal grid to the PTY when the child application has enabled mouse reporting (`MouseProtocol != Off`), so that interactive TUI buttons like Claude Code's "Jump to bottom (click) ↓" receive click events and function as expected (inferred). Scope includes: encoding mouse press and release for SGR (`?1006`) and legacy formats (inferred), routing grid clicks to PTY writes instead of selection drag when tracking is active (inferred), preserving ⌘-click for hyperlinks without PTY interference per hyperlink D17/R4.11 (inferred), and retaining text selection behavior when tracking is disabled (inferred). Done when clicking on grid coordinates in a mouse-reporting application writes valid mouse press and release escape sequences to the PTY, the existing test suite and clippy remain clean, and unit/acceptance tests cover mouse button press/release routing and encoding (inferred).

## Research
- `crates/term-core/src/grid.rs:329-350`: `MouseProtocol` enum defines `X10`, `Normal`, `ButtonEvent`, `AnyEvent`, `Off` with `is_on()` method.
- `crates/term-core/src/grid.rs:352-360`: `MouseEncoding` enum defines `Legacy` (X10 byte offset) and `Sgr` (`ESC [ < ... M/m`).
- `crates/term-core/src/parser.rs:450-459`: VT parser sets `MouseProtocol` via `?9`, `?1000`, `?1002`, `?1003` and `MouseEncoding` via `?1006`.
- `crates/app/src/input.rs:884-909`: `encode_wheel_report` implements SGR and Legacy encoding for wheel events only, but lacks button press/release encoding.
- `crates/app/src/input.rs:3212-3282`: `wheel_route` and `wheel_bytes` decide routing and PTY bytes for wheel events; no equivalent `mouse_button_bytes` exists for clicks.
- `crates/app/src/terminal_app.rs:2746-2879`: `WindowEvent::MouseInput` pressed handler routes `PressRoute::Grid` unconditionally to `SelectionDrag::press`, never sending mouse press reports to PTY.
- `crates/app/src/terminal_app.rs:2905-2950`: `WindowEvent::MouseInput` released handler clears selection / opens links, never reporting button release to PTY.
- `crates/app/src/hyperlink.rs:858-875`: Test `the_decision_cannot_depend_on_the_mouse_protocol` explicitly documents that button reports are not yet encoded and ⌘-click must never be suppressed by or written to running applications (D17/R4.11).
- `specs/alt-screen-scroll/loop.md:24-39`: Prior session diagnosed Claude Code sending `?1000`, `?1002`, `?1003`, `?1006` and implemented wheel reporting, explicitly deferring button click forwarding.
- `CLAUDE.md:44-46`: Build/test/lint commands: `cargo build`, `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`.

## Decisions

### D1: Pure mouse button routing and encoding in `input.rs`
- **Status**: assumed
- **Rationale**: Follows the established architectural pattern of `wheel_route` and `wheel_bytes` (`input.rs:3200-3282`). By isolating mouse routing decisions and byte encodings into pure functions (`mouse_button_route`, `mouse_button_bytes`, `grid_press_action`, `grid_release_action`), all edge cases (protocol on/off, SGR vs Legacy, modifier bypasses, coordinate ceilings) can be tested thoroughly without needing a window, GPU surface, or live PTY. Re-exported in `crates/app/src/lib.rs` for integration tests.
  - `GridPressAction` has variants `Report(Vec<u8>)`, `Drag` (reserved strictly for Left button selection), and `Ignore` (Right/Middle button when reporting is bypassed).
  - `GridReleaseAction` has variants `Report(Vec<u8>)` and `Ignore` (when release reports are suppressed, such as `MouseProtocol::X10` or coordinate drops).
  Rejected alternative: inlining escape sequence generation directly inside `terminal_app.rs` event loop match arms, which makes unit testing difficult and duplicates logic.


### D2: SGR (`?1006`) and Legacy wire format encoding for button press and release
- **Status**: assumed
- **Rationale**: Modern terminal apps like Claude Code request SGR encoding (`?1006h`), which encodes press as `ESC [ < {button} ; {col} ; {row} M` and release as `ESC [ < {button} ; {col} ; {row} m` with 1-based coordinates and no upper bound on columns/rows. For older or legacy applications (`?1006l`), X10/VT200 encoding is used (`ESC [ M {b+32} {col+32} {row+32}`), with release encoded as button code 3 (unless `MouseProtocol::X10`, which emits no release reports), dropping reports where coordinate + 32 > 255. Rejected alternative: supporting only SGR encoding, which would break older TUIs (e.g. htop/mc without ?1006); rejected alternative: wrapping coordinates past 223 in legacy mode, which would corrupt click positions.

### D3: Modifier bypass precedence for ⌘ (hyperlinks) and Shift (manual selection)
- **Status**: assumed
- **Rationale**: Mandated by `specs/hyperlinks/spec.md` D17/R4.11 ("Mouse-reporting apps do not suppress ⌘-click. Links stay live inside a mouse-reporting TUI"). If `mods.super_key()` is pressed, mouse reporting is bypassed so the user can open hyperlinks. Additionally, standard terminal convention (xterm, iTerm2, Alacritty) specifies that holding Shift allows selecting and copying terminal text even when a child TUI has mouse reporting enabled. If `mods.shift_key()` is pressed, mouse reporting is bypassed in favor of terminal text selection. Rejected alternative: letting mouse reporting swallow ⌘-click, violating hyperlink R4.11; rejected alternative: omitting Shift bypass, making text selection impossible inside mouse-enabled TUIs.

### D4: Explicit mouse button reporting state `mouse_reported` in `terminal_app.rs`
- **Status**: assumed
- **Rationale**: In `terminal_app.rs`, `WindowEvent::MouseInput` manages the press and release lifecycle. `inner.mouse_reported: Option<(MouseButton, u16, u16)>` tracks the exact button and `(col, row)` cell latched at press time. On `WindowEvent::MouseInput { state: ElementState::Pressed, .. }`, if `grid_press_action` determines `GridPressAction::Report(bytes)`, it writes the bytes to the active tab's PTY, sets `inner.mouse_reported = Some((button, col, row))`, and leaves `inner.drag = None` and `inner.dragging = false`. On `WindowEvent::MouseInput { state: ElementState::Released, .. }`, if `mouse_reported` is `Some((btn, col, row))`, `grid_release_action` produces release bytes (omitted if `MouseProtocol::X10`), writes them to the PTY, and clears `mouse_reported = None` without touching terminal text selection. Rejected alternative: checking `mouse_protocol().is_on()` independently at release without latched coordinates, which would fail to send the release to the cell where press originated or would misbehave if the child process toggled mouse mode mid-click.


### D5: Button coverage: Left, Middle, and Right
- **Status**: assumed
- **Rationale**: Map `winit::event::MouseButton::Left` to button index 0, `Middle` to 1, and `Right` to 2. While the primary user symptom involves Left click on the "Jump to bottom (click) ↓" button, supporting all three standard mouse buttons provides full TUI mouse compliance with minimal incremental code. Rejected alternative: supporting only Left click, leaving right-click context menus in TUIs broken.

## Constraints
- C1: `crates/term-core` and `crates/renderer` public APIs are consumed as shipped; no architectural layer violations (`common ← term-core ← renderer ← app`).
- C2: ⌘-click on hyperlinks must not be broken or suppressed, and must never write mouse report bytes to the PTY (`specs/hyperlinks/spec.md` D17, R4.11).
- C3: When mouse reporting is off (`MouseProtocol::Off`), text selection and link clicking behavior must remain 100% unchanged.
- C4: No external GUI or widget crate dependencies; pure Rust/winit/Metal (`CLAUDE.md`).
- C5: `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings` must stay green with zero warnings.

## Known Gaps
- Mouse hover motion reporting (`?1003` AnyEvent without buttons held) is omitted to prevent high-frequency PTY write traffic during ordinary pointer movement.
- L2 provisional: none. L2 unresolved: none.

## Requirements

### R0: Mouse button reporting over terminal grid — D1, D4
#### R0.1: Grid click in mouse-aware application transmits click to PTY
- **Given**: An active terminal tab whose child application has enabled mouse reporting (`MouseProtocol::AnyEvent`, `MouseEncoding::Sgr`)
- **When**: The user clicks with the left mouse button at grid cell `(15, 30)` with no modifier keys held
- **Then**: An SGR press sequence `\x1b[<0;16;31M` followed by release sequence `\x1b[<0;16;31m` is written to the active tab's PTY, and no text selection drag is armed

### R1: Pure routing and encoding functions — D1, D2
#### R1.1: SGR press encoding via `input::mouse_button_bytes`
- **Given**: Mouse protocol is `MouseProtocol::Normal` and encoding is `MouseEncoding::Sgr`
- **When**: `mouse_button_bytes(MouseProtocol::Normal, MouseEncoding::Sgr, MouseButton::Left, ElementState::Pressed, (10, 5), ModifiersState::empty())` is called
- **Then**: Returns `Some(b"\x1b[<0;11;6M")`

#### R1.2: SGR release encoding via `input::mouse_button_bytes`
- **Given**: Mouse protocol is `MouseProtocol::Normal` and encoding is `MouseEncoding::Sgr`
- **When**: `mouse_button_bytes(MouseProtocol::Normal, MouseEncoding::Sgr, MouseButton::Left, ElementState::Released, (10, 5), ModifiersState::empty())` is called
- **Then**: Returns `Some(b"\x1b[<0;11;6m")`

#### R1.3: Legacy X10 press encoding via `input::mouse_button_bytes`
- **Given**: Mouse protocol is `MouseProtocol::Normal` and encoding is `MouseEncoding::Legacy`
- **When**: `mouse_button_bytes(MouseProtocol::Normal, MouseEncoding::Legacy, MouseButton::Left, ElementState::Pressed, (10, 5), ModifiersState::empty())` is called
- **Then**: Returns `Some(vec![0x1b, b'[', b'M', 32, 10 + 1 + 32, 5 + 1 + 32])`

#### R1.4: Legacy X10 release encoding via `input::mouse_button_bytes`
- **Given**: Mouse protocol is `MouseProtocol::Normal` and encoding is `MouseEncoding::Legacy`
- **When**: `mouse_button_bytes(MouseProtocol::Normal, MouseEncoding::Legacy, MouseButton::Left, ElementState::Released, (10, 5), ModifiersState::empty())` is called
- **Then**: Returns `Some(vec![0x1b, b'[', b'M', 32 + 3, 10 + 1 + 32, 5 + 1 + 32])`

#### R1.5: X10 protocol omits release via `input::mouse_button_bytes`
- **Given**: Mouse protocol is `MouseProtocol::X10`
- **When**: `mouse_button_bytes(MouseProtocol::X10, MouseEncoding::Sgr, MouseButton::Left, ElementState::Released, (10, 5), ModifiersState::empty())` is called
- **Then**: Returns `None` (X10 protocol reports press only)

#### R1.6: Legacy coordinate ceiling clamp via `input::mouse_button_bytes`
- **Given**: Mouse protocol is `MouseProtocol::Normal` and encoding is `MouseEncoding::Legacy`
- **When**: `mouse_button_bytes(MouseProtocol::Normal, MouseEncoding::Legacy, MouseButton::Left, ElementState::Pressed, (250, 5), ModifiersState::empty())` is called with column exceeding 223
- **Then**: Returns `None` (prevents byte overflow/corruption in legacy wire protocol)

### R2: Modifier bypass for hyperlinks and selection — D3
#### R2.1: ⌘-click bypasses mouse reporting via `input::mouse_button_route`
- **Given**: Mouse protocol is `MouseProtocol::AnyEvent` and `mods.super_key()` is true
- **When**: `mouse_button_route(MouseProtocol::AnyEvent, mods)` is called
- **Then**: Returns `MouseButtonRoute::Bypass` so ⌘-click is handled by the terminal hyperlink detector and nothing is written to the PTY

#### R2.2: Shift-click bypasses mouse reporting via `input::mouse_button_route`
- **Given**: Mouse protocol is `MouseProtocol::AnyEvent` and `mods.shift_key()` is true
- **When**: `mouse_button_route(MouseProtocol::AnyEvent, mods)` is called
- **Then**: Returns `MouseButtonRoute::Bypass` so Shift-click is handled by the terminal selection drag

#### R2.3: Tracking off routes to viewport selection via `input::mouse_button_route`
- **Given**: Mouse protocol is `MouseProtocol::Off` and no modifiers are held
- **When**: `mouse_button_route(MouseProtocol::Off, mods)` is called
- **Then**: Returns `MouseButtonRoute::Bypass` so normal grid selection handles the click

#### R2.4: Active mouse tracking with no modifiers routes to report via `input::mouse_button_route`
- **Given**: Mouse protocol is `MouseProtocol::AnyEvent` and neither `mods.super_key()` nor `mods.shift_key()` is held
- **When**: `mouse_button_route(MouseProtocol::AnyEvent, mods)` is called
- **Then**: Returns `MouseButtonRoute::Report`

### R3: Terminal event loop lifecycle and PTY dispatch — D4, D5
#### R3.1: Mouse press over grid resolves to report via `input::grid_press_action`
- **Given**: Active tab has `MouseProtocol::AnyEvent` and pointer is over grid cell `(12, 4)`
- **When**: `grid_press_action(MouseProtocol::AnyEvent, MouseEncoding::Sgr, MouseButton::Left, (12, 4), ModifiersState::empty())` is called
- **Then**: Returns `GridPressAction::Report(b"\x1b[<0;13;5M".to_vec())`

#### R3.2: Mouse release over grid resolves to release report via `input::grid_release_action`
- **Given**: A mouse press report for `(MouseButton::Left, 12, 4)` was recorded in `mouse_reported`
- **When**: `grid_release_action(Some((MouseButton::Left, 12, 4)), MouseProtocol::AnyEvent, MouseEncoding::Sgr, MouseButton::Left, ModifiersState::empty())` is called
- **Then**: Returns `GridReleaseAction::Report(b"\x1b[<0;13;5m".to_vec())`

#### R3.3: Selection drag suppression via `input::grid_press_action`
- **Given**: Active tab has `MouseProtocol::AnyEvent` and pointer is over grid cell `(12, 4)`
- **When**: `grid_press_action(MouseProtocol::AnyEvent, MouseEncoding::Sgr, MouseButton::Left, (12, 4), ModifiersState::empty())` is called with no modifiers
- **Then**: Returns `GridPressAction::Report(_)`, distinguishing it from `GridPressAction::Drag` so the event loop does not arm `inner.drag`

#### R3.4: Right and Middle mouse buttons encode to PTY in `input::grid_press_action`
- **Given**: Active tab has `MouseProtocol::AnyEvent` and pointer is over grid cell `(12, 4)`
- **When**: `grid_press_action(MouseProtocol::AnyEvent, MouseEncoding::Sgr, MouseButton::Right, (12, 4), ModifiersState::empty())` is called
- **Then**: Returns `GridPressAction::Report(b"\x1b[<2;13;5M".to_vec())`

#### R3.5: Release omission resolves to ignore via `input::grid_release_action`
- **Given**: A mouse press report for `(MouseButton::Left, 12, 4)` was recorded under `MouseProtocol::X10`
- **When**: `grid_release_action(Some((MouseButton::Left, 12, 4)), MouseProtocol::X10, MouseEncoding::Sgr, MouseButton::Left, ModifiersState::empty())` is called
- **Then**: Returns `GridReleaseAction::Ignore`

## Tasks

### T1: Mouse button wire encoding and routing pure functions in `crates/app/src/input.rs` [infra]
- **Fulfills**: R1, R2, R3
- **Depends on**: (none)
- **Target**: `crates/app/src/input.rs`, `crates/app/src/lib.rs`
- **Details**: Implement in `input.rs` and re-export in `lib.rs`:
  - Enums `MouseButtonRoute` (`Report`, `Bypass`), `GridPressAction` (`Report(Vec<u8>)`, `Drag`, `Ignore`), and `GridReleaseAction` (`Report(Vec<u8>)`, `Ignore`).
  - Functions `mouse_button_route(protocol, mods)`, `encode_mouse_button_report(encoding, button, state, col, row, mods)`, `mouse_button_bytes(protocol, encoding, button, state, pos, mods)`, `grid_press_action(protocol, encoding, button, pos, mods)`, and `grid_release_action(reported, protocol, encoding, button, mods)`.
  - Supports SGR (`?1006`) and Legacy formats, Left/Middle/Right buttons, modifier bypasses (Cmd, Shift), protocol checks (X10 no-release, Off bypass), and coordinate bounds clamping.
  - Ensures `GridPressAction::Drag` is only emitted for `MouseButton::Left` when reporting is bypassed, and `GridPressAction::Ignore` for Right/Middle when reporting is bypassed.
  - Includes inline unit tests covering R1, R2, and R3.

### TT1: acceptance tests for R1's GWTs
- **Fulfills**: R1 (tests)
- **Depends on**: T1
- **Target**: `crates/app/tests/tt1_mouse_button_encoding.rs`
- **Details**: Verify SGR and Legacy press and release encodings, X10 protocol release omission, and legacy coordinate clamp against R1.1–R1.6.

### TT2: acceptance tests for R2's GWTs
- **Fulfills**: R2 (tests)
- **Depends on**: T1
- **Target**: `crates/app/tests/tt2_mouse_button_modifiers.rs`
- **Details**: Verify modifier bypass rules (Super key, Shift key, tracking Off, and clean Report routing) against R2.1–R2.4.

### TT3: acceptance tests for R3's GWTs
- **Fulfills**: R3 (tests)
- **Depends on**: T1
- **Target**: `crates/app/tests/tt3_mouse_button_lifecycle.rs`
- **Details**: Verify `grid_press_action` and `grid_release_action` lifecycle, press/release matching, drag suppression, release omission, and multi-button encoding against R3.1–R3.5.

### T2: Terminal event loop mouse button integration and PTY dispatch [vertical]
- **Fulfills**: R0, R3
- **Depends on**: T1, TT1, TT2, TT3
- **Target**: `crates/app/src/terminal_app.rs`
- **Details**: Integrate `grid_press_action` and `grid_release_action` into `WindowEvent::MouseInput`:
  - Add `mouse_reported: Option<(MouseButton, u16, u16)>` to `Running` state in `terminal_app.rs`.
  - In `WindowEvent::MouseInput { state: ElementState::Pressed, button, .. }`:
    - Sidebar and overlay handlers strictly retain their check for `MouseButton::Left` only, ignoring Right/Middle clicks.
    - On `PressRoute::Grid`: invoke `grid_press_action`.
      - If `GridPressAction::Report(bytes)`: write to `active_mut().write(&bytes)` and set `inner.mouse_reported = Some((button, cell.0, cell.1))` without arming `inner.drag`.
      - If `GridPressAction::Drag`: (only happens for `MouseButton::Left`), proceed with `SelectionDrag::press` to start text selection.
      - If `GridPressAction::Ignore`: do nothing (preserves C3; Right/Middle clicks when reporting is Off do not start selection or switch tabs).
  - In `WindowEvent::MouseInput { state: ElementState::Released, button, .. }`:
    - If `inner.mouse_reported` is active for `button`: invoke `grid_release_action`, write any returned release bytes to active tab PTY, and reset `inner.mouse_reported = None` (preventing selection clear or link open).
    - Else if `button == MouseButton::Left`: proceed with existing `ReleaseTarget::Resize` and `ReleaseTarget::Selection` handlers.
  - Verify with `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings`.



## External Dependencies

### Pre-work
- (none)

### Post-work
- (none)

## Spec Review
- **Verdict**: PASS
- **Reviews**: 3
- **Fixed**:
  - [Coverage] T1 / T2 — Updated T1 to fulfill R1, R2, R3 and T2 to fulfill R0, R3.
  - [Testability] R2 — Added R2.4 verifying mouse_button_route returns Report when tracking is active and no modifiers are pressed.
  - [Testability] R3 — Specified pure functions grid_press_action and grid_release_action in input.rs for headless testability.
  - [Grounding] T1 — Added crates/app/src/lib.rs re-exports for all new types and functions.
  - [Consistency] D4 — Resolved tracking state representation concretely to mouse_reported: Option<(MouseButton, u16, u16)>.
  - [Readiness] T1, T2 — Documented concrete targets, function signatures, enum variants, and qualified button matching in terminal_app.rs.
- **Open**:
  (none)





