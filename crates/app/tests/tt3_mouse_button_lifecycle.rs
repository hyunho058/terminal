//! Acceptance tests for mouse-button-reporting R3: terminal event loop lifecycle
//! and PTY dispatch (D4, D5).
//!
//! Verifies pure lifecycle functions `grid_press_action` and `grid_release_action`
//! governing press/release routing, selection drag suppression, multi-button support,
//! and protocol release omission.

use app::{grid_press_action, grid_release_action, GridPressAction, GridReleaseAction};
use term_core::{MouseEncoding, MouseProtocol};
use winit::event::MouseButton;
use winit::keyboard::ModifiersState;

/// R3.1: Mouse press over grid resolves to report via `input::grid_press_action`.
///
/// Given: Active tab has `MouseProtocol::AnyEvent` and pointer is over grid cell `(12, 4)`.
/// When: `grid_press_action(MouseProtocol::AnyEvent, MouseEncoding::Sgr, MouseButton::Left, (12, 4), ModifiersState::empty())` is called.
/// Then: Returns `GridPressAction::Report(b"\x1b[<0;13;5M".to_vec())`.
#[test]
fn r3_1_press_resolves_to_report() {
    let action = grid_press_action(
        MouseProtocol::AnyEvent,
        MouseEncoding::Sgr,
        MouseButton::Left,
        (12, 4),
        &ModifiersState::empty(),
    );
    assert_eq!(action, GridPressAction::Report(b"\x1b[<0;13;5M".to_vec()));
}

/// R3.2: Mouse release over grid resolves to release report via `input::grid_release_action`.
///
/// Given: A mouse press report for `(MouseButton::Left, 12, 4)` was recorded in `mouse_reported`.
/// When: `grid_release_action(Some((MouseButton::Left, 12, 4)), MouseProtocol::AnyEvent, MouseEncoding::Sgr, MouseButton::Left, ModifiersState::empty())` is called.
/// Then: Returns `GridReleaseAction::Report(b"\x1b[<0;13;5m".to_vec())`.
#[test]
fn r3_2_release_resolves_to_report() {
    let action = grid_release_action(
        Some((MouseButton::Left, 12, 4)),
        MouseProtocol::AnyEvent,
        MouseEncoding::Sgr,
        MouseButton::Left,
        &ModifiersState::empty(),
    );
    assert_eq!(action, GridReleaseAction::Report(b"\x1b[<0;13;5m".to_vec()));
}

/// R3.3: Selection drag suppression via `input::grid_press_action`.
///
/// Given: Active tab has `MouseProtocol::AnyEvent` and pointer is over grid cell `(12, 4)`.
/// When: `grid_press_action(MouseProtocol::AnyEvent, MouseEncoding::Sgr, MouseButton::Left, (12, 4), ModifiersState::empty())` is called with no modifiers.
/// Then: Returns `GridPressAction::Report(_)`, distinguishing it from `GridPressAction::Drag` so the event loop does not arm `inner.drag`.
#[test]
fn r3_3_selection_drag_suppressed() {
    let action = grid_press_action(
        MouseProtocol::AnyEvent,
        MouseEncoding::Sgr,
        MouseButton::Left,
        (12, 4),
        &ModifiersState::empty(),
    );

    // Must be Report, not Drag
    assert!(
        matches!(action, GridPressAction::Report(_)),
        "expected GridPressAction::Report, got {:?}",
        action
    );
    assert_ne!(
        action,
        GridPressAction::Drag,
        "grid press with active mouse tracking must suppress selection drag"
    );
}

/// R3.4: Right and Middle mouse buttons encode to PTY in `input::grid_press_action`.
///
/// Given: Active tab has `MouseProtocol::AnyEvent` and pointer is over grid cell `(12, 4)`.
/// When: `grid_press_action(MouseProtocol::AnyEvent, MouseEncoding::Sgr, MouseButton::Right, (12, 4), ModifiersState::empty())` is called.
/// Then: Returns `GridPressAction::Report(b"\x1b[<2;13;5M".to_vec())`.
#[test]
fn r3_4_right_middle_button_press() {
    // Right mouse button (index 2)
    let right_action = grid_press_action(
        MouseProtocol::AnyEvent,
        MouseEncoding::Sgr,
        MouseButton::Right,
        (12, 4),
        &ModifiersState::empty(),
    );
    assert_eq!(
        right_action,
        GridPressAction::Report(b"\x1b[<2;13;5M".to_vec())
    );

    // Middle mouse button (index 1)
    let middle_action = grid_press_action(
        MouseProtocol::AnyEvent,
        MouseEncoding::Sgr,
        MouseButton::Middle,
        (12, 4),
        &ModifiersState::empty(),
    );
    assert_eq!(
        middle_action,
        GridPressAction::Report(b"\x1b[<1;13;5M".to_vec())
    );
}

/// R3.5: Release omission resolves to ignore via `input::grid_release_action`.
///
/// Given: A mouse press report for `(MouseButton::Left, 12, 4)` was recorded under `MouseProtocol::X10`.
/// When: `grid_release_action(Some((MouseButton::Left, 12, 4)), MouseProtocol::X10, MouseEncoding::Sgr, MouseButton::Left, ModifiersState::empty())` is called.
/// Then: Returns `GridReleaseAction::Ignore`.
#[test]
fn r3_5_release_omission_resolves_to_ignore() {
    let action = grid_release_action(
        Some((MouseButton::Left, 12, 4)),
        MouseProtocol::X10,
        MouseEncoding::Sgr,
        MouseButton::Left,
        &ModifiersState::empty(),
    );
    assert_eq!(action, GridReleaseAction::Ignore);
}
