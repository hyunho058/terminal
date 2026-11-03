//! Acceptance tests for requirement R4 (D5) of `specs/mouse-drag-reporting/spec.md`:
//! bypasses and click-only apps are unchanged.
//!
//! Covers R4.1 and R4.2 (task TT4):
//! - R4.1: Shift-press arms terminal selection, so no motion can follow
//! - R4.2: ⌘-press never latches a report
//!
//! R4.3 (a `?1000`-only drag) is a manual check (Known Gaps manual check 4).

use app::{grid_press_action, reported_drag_motion, GridPressAction};
use term_core::{MouseEncoding, MouseProtocol};
use winit::event::MouseButton;
use winit::keyboard::ModifiersState;

/// R4.1: Shift-press arms terminal selection, so no motion can follow, via
/// `input::grid_press_action`.
///
/// Given: `MouseProtocol::AnyEvent`, Shift held.
/// When: `grid_press_action(MouseProtocol::AnyEvent, MouseEncoding::Sgr, MouseButton::Left,
/// (2, 5), &ModifiersState::SHIFT)` is called.
/// Then: Returns `GridPressAction::Drag`. No `mouse_reported` latch is set, and
/// `reported_drag_motion(&mut None, …)` returns `None` (R2.5).
#[test]
fn r4_1_shift_press_arms_selection_and_no_motion_follows() {
    let action = grid_press_action(
        MouseProtocol::AnyEvent,
        MouseEncoding::Sgr,
        MouseButton::Left,
        (2, 5),
        &ModifiersState::SHIFT,
    );
    assert_eq!(action, GridPressAction::Drag);

    // A `Drag` press sets no latch, so the latch the motion path sees is `None`.
    let mut latch: Option<(MouseButton, u16, u16)> = None;
    assert_eq!(
        reported_drag_motion(
            &mut latch,
            MouseProtocol::AnyEvent,
            MouseEncoding::Sgr,
            (6, 5)
        ),
        None
    );
    assert_eq!(latch, None);
}

/// R4.2: ⌘-press never latches a report via `input::grid_press_action`.
///
/// Given: `MouseProtocol::AnyEvent`, ⌘ held.
/// When: `grid_press_action(MouseProtocol::AnyEvent, MouseEncoding::Sgr, MouseButton::Left,
/// (2, 5), &ModifiersState::SUPER)` is called.
/// Then: Returns `GridPressAction::Drag`, never `Report`.
#[test]
fn r4_2_cmd_press_never_reports() {
    let action = grid_press_action(
        MouseProtocol::AnyEvent,
        MouseEncoding::Sgr,
        MouseButton::Left,
        (2, 5),
        &ModifiersState::SUPER,
    );
    assert!(
        !matches!(action, GridPressAction::Report(_)),
        "⌘-press must never report, got {action:?}"
    );
    assert_eq!(action, GridPressAction::Drag);
}
