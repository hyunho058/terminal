//! Acceptance tests for scroll-snap-on-click R4 (D4): isolation of mouse-reporting TUIs.
//!
//! Covers:
//! - R4.1: When child applications enable mouse reporting (`MouseProtocol != Off`),
//!   left clicks are reported as escape sequences to the PTY and never trigger `ReleaseTarget::Selection`.

use app::{
    grid_press_action, grid_release_action, mouse_button_route, GridPressAction, GridReleaseAction,
    MouseButtonRoute,
};
use term_core::{MouseEncoding, MouseProtocol};
use winit::event::MouseButton;
use winit::keyboard::ModifiersState;

/// R4.1: Grid clicks in mouse-reporting applications route to PTY reports,
/// bypassing terminal selection and snap-to-bottom.
#[test]
fn r4_1_mouse_reporting_tui_bypasses_selection_release() {
    let mods = ModifiersState::empty();
    let protocol = MouseProtocol::AnyEvent;
    let encoding = MouseEncoding::Sgr;
    let cell = (10, 10);

    // 1. Route check: mouse_button_route decides Report
    assert_eq!(
        mouse_button_route(protocol, &mods),
        MouseButtonRoute::Report
    );

    // 2. Press resolves to GridPressAction::Report (not GridPressAction::Drag)
    let press_action = grid_press_action(protocol, encoding, MouseButton::Left, cell, &mods);
    match press_action {
        GridPressAction::Report(bytes) => {
            // Sgr: ESC[<0;11;11M
            assert_eq!(bytes, b"\x1b[<0;11;11M");
        }
        _ => panic!("Expected GridPressAction::Report, got {:?}", press_action),
    }

    // 3. Release resolves to GridReleaseAction::Report
    let release_action = grid_release_action(
        Some((MouseButton::Left, cell.0, cell.1)),
        protocol,
        encoding,
        MouseButton::Left,
        &mods,
    );
    match release_action {
        GridReleaseAction::Report(bytes) => {
            // Sgr release: ESC[<0;11;11m
            assert_eq!(bytes, b"\x1b[<0;11;11m");
        }
        _ => panic!(
            "Expected GridReleaseAction::Report, got {:?}",
            release_action
        ),
    }

    // When mouse_reported is armed by GridPressAction::Report, the release event is
    // handled by the `mouse_reported` release arm, and `ReleaseTarget::Selection` is never reached.
}
