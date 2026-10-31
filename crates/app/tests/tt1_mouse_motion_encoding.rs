//! Acceptance tests for requirement R1 (D1, D2) of `specs/mouse-drag-reporting/spec.md`:
//! motion report encoding.
//!
//! Covers R1.1 to R1.5:
//! - R1.1: SGR left-drag motion
//! - R1.2: SGR middle and right drag codes
//! - R1.3: Legacy left-drag motion
//! - R1.4: Legacy coordinate past 223 drops the report
//! - R1.5: Buttons other than Left, Middle, Right are not encoded

use app::encode_mouse_motion_report;
use term_core::MouseEncoding;
use winit::event::MouseButton;

/// R1.1: SGR left-drag motion via `input::encode_mouse_motion_report`.
///
/// Given: `MouseEncoding::Sgr`.
/// When: `encode_mouse_motion_report(MouseEncoding::Sgr, MouseButton::Left, 10, 5)` is called.
/// Then: Returns `Some(b"\x1b[<32;11;6M".to_vec())`.
#[test]
fn r1_1_sgr_left_drag_motion() {
    assert_eq!(
        encode_mouse_motion_report(MouseEncoding::Sgr, MouseButton::Left, 10, 5),
        Some(b"\x1b[<32;11;6M".to_vec())
    );
}

/// R1.2: SGR middle and right drag codes via `input::encode_mouse_motion_report`.
///
/// Given: `MouseEncoding::Sgr`.
/// When: called with `MouseButton::Middle` and then `MouseButton::Right`, both at (0, 0).
/// Then: Returns `Some(b"\x1b[<33;1;1M".to_vec())` and `Some(b"\x1b[<34;1;1M".to_vec())`.
#[test]
fn r1_2_sgr_middle_and_right_drag_codes() {
    assert_eq!(
        encode_mouse_motion_report(MouseEncoding::Sgr, MouseButton::Middle, 0, 0),
        Some(b"\x1b[<33;1;1M".to_vec())
    );
    assert_eq!(
        encode_mouse_motion_report(MouseEncoding::Sgr, MouseButton::Right, 0, 0),
        Some(b"\x1b[<34;1;1M".to_vec())
    );
}

/// R1.3: Legacy left-drag motion via `input::encode_mouse_motion_report`.
///
/// Given: `MouseEncoding::Legacy`.
/// When: `encode_mouse_motion_report(MouseEncoding::Legacy, MouseButton::Left, 10, 5)` is called.
/// Then: Returns `Some(vec![0x1b, b'[', b'M', 64, 43, 38])`.
#[test]
fn r1_3_legacy_left_drag_motion() {
    assert_eq!(
        encode_mouse_motion_report(MouseEncoding::Legacy, MouseButton::Left, 10, 5),
        Some(vec![0x1b, b'[', b'M', 64, 43, 38])
    );
}

/// R1.4: Legacy coordinate past 223 drops the report via `input::encode_mouse_motion_report`.
///
/// Given: `MouseEncoding::Legacy`.
/// When: `encode_mouse_motion_report(MouseEncoding::Legacy, MouseButton::Left, 250, 5)` is called.
/// Then: Returns `None`.
#[test]
fn r1_4_legacy_coordinate_past_223_drops_report() {
    assert_eq!(
        encode_mouse_motion_report(MouseEncoding::Legacy, MouseButton::Left, 250, 5),
        None
    );
}

/// R1.5: Buttons other than Left, Middle, Right are not encoded via
/// `input::encode_mouse_motion_report`.
///
/// Given: `MouseEncoding::Sgr`.
/// When: `encode_mouse_motion_report(MouseEncoding::Sgr, MouseButton::Back, 1, 1)` is called.
/// Then: Returns `None`.
#[test]
fn r1_5_other_buttons_not_encoded() {
    assert_eq!(
        encode_mouse_motion_report(MouseEncoding::Sgr, MouseButton::Back, 1, 1),
        None
    );
}
