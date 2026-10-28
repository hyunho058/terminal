//! Acceptance tests for requirement R1 (D1, D2): pure routing and encoding functions.
//!
//! Covers R1.1 to R1.6:
//! - R1.1: SGR press encoding
//! - R1.2: SGR release encoding
//! - R1.3: Legacy X10 press encoding
//! - R1.4: Legacy X10 release encoding
//! - R1.5: X10 protocol omits release
//! - R1.6: Legacy coordinate ceiling clamp

use app::{encode_mouse_button_report, mouse_button_bytes};
use term_core::{MouseEncoding, MouseProtocol};
use winit::event::{ElementState, MouseButton};
use winit::keyboard::ModifiersState;

/// R1.1: SGR press encoding via `input::mouse_button_bytes`.
///
/// Given: Mouse protocol is `MouseProtocol::Normal` and encoding is `MouseEncoding::Sgr`.
/// When: `mouse_button_bytes(MouseProtocol::Normal, MouseEncoding::Sgr, MouseButton::Left, ElementState::Pressed, (10, 5), &ModifiersState::empty())` is called.
/// Then: Returns `Some(b"\x1b[<0;11;6M")`.
#[test]
fn r1_1_sgr_press_encoding() {
    let mods = ModifiersState::empty();
    let bytes = mouse_button_bytes(
        MouseProtocol::Normal,
        MouseEncoding::Sgr,
        MouseButton::Left,
        ElementState::Pressed,
        (10, 5),
        &mods,
    );
    assert_eq!(bytes, Some(b"\x1b[<0;11;6M".to_vec()));

    let direct = encode_mouse_button_report(
        MouseEncoding::Sgr,
        MouseButton::Left,
        ElementState::Pressed,
        10,
        5,
        &mods,
    );
    assert_eq!(direct, Some(b"\x1b[<0;11;6M".to_vec()));
}

/// R1.2: SGR release encoding via `input::mouse_button_bytes`.
///
/// Given: Mouse protocol is `MouseProtocol::Normal` and encoding is `MouseEncoding::Sgr`.
/// When: `mouse_button_bytes(MouseProtocol::Normal, MouseEncoding::Sgr, MouseButton::Left, ElementState::Released, (10, 5), &ModifiersState::empty())` is called.
/// Then: Returns `Some(b"\x1b[<0;11;6m")`.
#[test]
fn r1_2_sgr_release_encoding() {
    let mods = ModifiersState::empty();
    let bytes = mouse_button_bytes(
        MouseProtocol::Normal,
        MouseEncoding::Sgr,
        MouseButton::Left,
        ElementState::Released,
        (10, 5),
        &mods,
    );
    assert_eq!(bytes, Some(b"\x1b[<0;11;6m".to_vec()));

    let direct = encode_mouse_button_report(
        MouseEncoding::Sgr,
        MouseButton::Left,
        ElementState::Released,
        10,
        5,
        &mods,
    );
    assert_eq!(direct, Some(b"\x1b[<0;11;6m".to_vec()));
}

/// R1.3: Legacy X10 press encoding via `input::mouse_button_bytes`.
///
/// Given: Mouse protocol is `MouseProtocol::Normal` and encoding is `MouseEncoding::Legacy`.
/// When: `mouse_button_bytes(MouseProtocol::Normal, MouseEncoding::Legacy, MouseButton::Left, ElementState::Pressed, (10, 5), &ModifiersState::empty())` is called.
/// Then: Returns `Some(vec![0x1b, b'[', b'M', 32, 10 + 1 + 32, 5 + 1 + 32])`.
#[test]
fn r1_3_legacy_press_encoding() {
    let mods = ModifiersState::empty();
    let bytes = mouse_button_bytes(
        MouseProtocol::Normal,
        MouseEncoding::Legacy,
        MouseButton::Left,
        ElementState::Pressed,
        (10, 5),
        &mods,
    );
    assert_eq!(
        bytes,
        Some(vec![0x1b, b'[', b'M', 32, 10 + 1 + 32, 5 + 1 + 32])
    );

    let direct = encode_mouse_button_report(
        MouseEncoding::Legacy,
        MouseButton::Left,
        ElementState::Pressed,
        10,
        5,
        &mods,
    );
    assert_eq!(
        direct,
        Some(vec![0x1b, b'[', b'M', 32, 10 + 1 + 32, 5 + 1 + 32])
    );
}

/// R1.4: Legacy X10 release encoding via `input::mouse_button_bytes`.
///
/// Given: Mouse protocol is `MouseProtocol::Normal` and encoding is `MouseEncoding::Legacy`.
/// When: `mouse_button_bytes(MouseProtocol::Normal, MouseEncoding::Legacy, MouseButton::Left, ElementState::Released, (10, 5), &ModifiersState::empty())` is called.
/// Then: Returns `Some(vec![0x1b, b'[', b'M', 32 + 3, 10 + 1 + 32, 5 + 1 + 32])`.
#[test]
fn r1_4_legacy_release_encoding() {
    let mods = ModifiersState::empty();
    let bytes = mouse_button_bytes(
        MouseProtocol::Normal,
        MouseEncoding::Legacy,
        MouseButton::Left,
        ElementState::Released,
        (10, 5),
        &mods,
    );
    assert_eq!(
        bytes,
        Some(vec![0x1b, b'[', b'M', 32 + 3, 10 + 1 + 32, 5 + 1 + 32])
    );

    let direct = encode_mouse_button_report(
        MouseEncoding::Legacy,
        MouseButton::Left,
        ElementState::Released,
        10,
        5,
        &mods,
    );
    assert_eq!(
        direct,
        Some(vec![0x1b, b'[', b'M', 32 + 3, 10 + 1 + 32, 5 + 1 + 32])
    );
}

/// R1.5: X10 protocol omits release via `input::mouse_button_bytes`.
///
/// Given: Mouse protocol is `MouseProtocol::X10`.
/// When: `mouse_button_bytes(MouseProtocol::X10, MouseEncoding::Sgr, MouseButton::Left, ElementState::Released, (10, 5), &ModifiersState::empty())` is called.
/// Then: Returns `None` (X10 protocol reports press only).
#[test]
fn r1_5_x10_omits_release() {
    let mods = ModifiersState::empty();
    let bytes = mouse_button_bytes(
        MouseProtocol::X10,
        MouseEncoding::Sgr,
        MouseButton::Left,
        ElementState::Released,
        (10, 5),
        &mods,
    );
    assert_eq!(bytes, None);
}

/// R1.6: Legacy coordinate ceiling clamp via `input::mouse_button_bytes`.
///
/// Given: Mouse protocol is `MouseProtocol::Normal` and encoding is `MouseEncoding::Legacy`.
/// When: `mouse_button_bytes(MouseProtocol::Normal, MouseEncoding::Legacy, MouseButton::Left, ElementState::Pressed, (250, 5), &ModifiersState::empty())` is called with column exceeding 223.
/// Then: Returns `None` (prevents byte overflow/corruption in legacy wire protocol).
#[test]
fn r1_6_legacy_coordinate_clamp() {
    let mods = ModifiersState::empty();
    let bytes = mouse_button_bytes(
        MouseProtocol::Normal,
        MouseEncoding::Legacy,
        MouseButton::Left,
        ElementState::Pressed,
        (250, 5),
        &mods,
    );
    assert_eq!(bytes, None);

    let direct = encode_mouse_button_report(
        MouseEncoding::Legacy,
        MouseButton::Left,
        ElementState::Pressed,
        250,
        5,
        &mods,
    );
    assert_eq!(direct, None);
}
