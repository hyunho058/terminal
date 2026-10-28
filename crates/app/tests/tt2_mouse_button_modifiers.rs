//! Acceptance tests for mouse-button-reporting R2 (D3): modifier bypass for
//! hyperlinks and selection.
//!
//! Verifies that ⌘-click (hyperlinks) and Shift-click (terminal selection) bypass
//! mouse reporting even when a child application has requested mouse events, and
//! that when mouse tracking is disabled (MouseProtocol::Off), clicks route to bypass.
//! Clean clicks with active mouse tracking route to Report.

use app::{mouse_button_route, MouseButtonRoute};
use term_core::MouseProtocol;
use winit::keyboard::ModifiersState;

/// R2.1: ⌘-click bypasses mouse reporting via `input::mouse_button_route`
///
/// Given: Mouse protocol is `MouseProtocol::AnyEvent` and `mods.super_key()` is true
/// When: `mouse_button_route(MouseProtocol::AnyEvent, mods)` is called
/// Then: Returns `MouseButtonRoute::Bypass` so ⌘-click is handled by the terminal
///       hyperlink detector and nothing is written to the PTY
#[test]
fn r2_1_cmd_click_bypasses() {
    let mods = ModifiersState::SUPER;
    assert!(mods.super_key(), "precondition: mods.super_key() is true");
    assert_eq!(
        mouse_button_route(MouseProtocol::AnyEvent, &mods),
        MouseButtonRoute::Bypass,
        "⌘-click must bypass mouse reporting for hyperlink handling"
    );

    // Also verify with other protocols and additional non-shift modifiers
    let cmd_alt = ModifiersState::SUPER | ModifiersState::ALT;
    assert_eq!(
        mouse_button_route(MouseProtocol::Normal, &cmd_alt),
        MouseButtonRoute::Bypass,
        "⌘+Alt click must bypass mouse reporting"
    );
}

/// R2.2: Shift-click bypasses mouse reporting via `input::mouse_button_route`
///
/// Given: Mouse protocol is `MouseProtocol::AnyEvent` and `mods.shift_key()` is true
/// When: `mouse_button_route(MouseProtocol::AnyEvent, mods)` is called
/// Then: Returns `MouseButtonRoute::Bypass` so Shift-click is handled by the terminal
///       selection drag
#[test]
fn r2_2_shift_click_bypasses() {
    let mods = ModifiersState::SHIFT;
    assert!(mods.shift_key(), "precondition: mods.shift_key() is true");
    assert_eq!(
        mouse_button_route(MouseProtocol::AnyEvent, &mods),
        MouseButtonRoute::Bypass,
        "Shift-click must bypass mouse reporting for terminal selection"
    );

    // Also verify with other tracking protocols and modifier combinations
    let shift_ctrl = ModifiersState::SHIFT | ModifiersState::CONTROL;
    assert_eq!(
        mouse_button_route(MouseProtocol::ButtonEvent, &shift_ctrl),
        MouseButtonRoute::Bypass,
        "Shift+Ctrl click must bypass mouse reporting"
    );
}

/// R2.3: Tracking off routes to viewport selection via `input::mouse_button_route`
///
/// Given: Mouse protocol is `MouseProtocol::Off` and no modifiers are held
/// When: `mouse_button_route(MouseProtocol::Off, mods)` is called
/// Then: Returns `MouseButtonRoute::Bypass` so normal grid selection handles the click
#[test]
fn r2_3_tracking_off_bypasses() {
    let mods = ModifiersState::empty();
    assert_eq!(
        mouse_button_route(MouseProtocol::Off, &mods),
        MouseButtonRoute::Bypass,
        "MouseProtocol::Off with no modifiers must route to Bypass"
    );
}

/// R2.4: Active mouse tracking with no modifiers routes to report via `input::mouse_button_route`
///
/// Given: Mouse protocol is `MouseProtocol::AnyEvent` and neither `mods.super_key()` nor `mods.shift_key()` is held
/// When: `mouse_button_route(MouseProtocol::AnyEvent, mods)` is called
/// Then: Returns `MouseButtonRoute::Report`
#[test]
fn r2_4_active_tracking_reports() {
    let mods = ModifiersState::empty();
    assert!(
        !mods.super_key() && !mods.shift_key(),
        "precondition: neither super nor shift held"
    );
    assert_eq!(
        mouse_button_route(MouseProtocol::AnyEvent, &mods),
        MouseButtonRoute::Report,
        "Active tracking with no modifiers must route to Report"
    );

    // Also verify active tracking with non-bypass modifiers like Alt or Ctrl
    let alt_mods = ModifiersState::ALT;
    assert_eq!(
        mouse_button_route(MouseProtocol::AnyEvent, &alt_mods),
        MouseButtonRoute::Report,
        "Active tracking with Alt (neither Super nor Shift) must route to Report"
    );

    let ctrl_mods = ModifiersState::CONTROL;
    assert_eq!(
        mouse_button_route(MouseProtocol::AnyEvent, &ctrl_mods),
        MouseButtonRoute::Report,
        "Active tracking with Control (neither Super nor Shift) must route to Report"
    );

    // Verify other active protocols (Normal, ButtonEvent, X10) route to Report
    assert_eq!(
        mouse_button_route(MouseProtocol::Normal, &mods),
        MouseButtonRoute::Report,
        "Normal tracking must route to Report"
    );
    assert_eq!(
        mouse_button_route(MouseProtocol::ButtonEvent, &mods),
        MouseButtonRoute::Report,
        "ButtonEvent tracking must route to Report"
    );
    assert_eq!(
        mouse_button_route(MouseProtocol::X10, &mods),
        MouseButtonRoute::Report,
        "X10 tracking must route to Report"
    );
}
