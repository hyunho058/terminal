//! Acceptance tests for `specs/mouse-drag-reporting/spec.md` R2: when a motion
//! report is sent, where the latch goes, and who owns the pointer (D1, D2, D3, D6).
//!
//! One test per R2.1–R2.10, calling the pure decision functions
//! `reported_drag_motion`, `drag_owns_pointer`, `grid_press_action` and
//! `grid_release_action`, and asserting both the returned bytes and the latch
//! value after each call wherever the spec's Then states it.

use app::{
    drag_owns_pointer, grid_press_action, grid_release_action, reported_drag_motion,
    GridPressAction, GridReleaseAction,
};
use term_core::{MouseEncoding, MouseProtocol};
use winit::event::MouseButton;
use winit::keyboard::ModifiersState;

type Latch = Option<(MouseButton, u16, u16)>;

/// R2.1: A new cell under `ButtonEvent` reports and moves the latch.
#[test]
fn r2_1_new_cell_under_button_event_reports_and_moves_latch() {
    let mut latch: Latch = Some((MouseButton::Left, 2, 5));
    let out = reported_drag_motion(
        &mut latch,
        MouseProtocol::ButtonEvent,
        MouseEncoding::Sgr,
        (3, 5),
    );
    assert_eq!(out, Some(b"\x1b[<32;4;6M".to_vec()));
    assert_eq!(latch, Some((MouseButton::Left, 3, 5)));
}

/// R2.2: A new cell under `AnyEvent` reports.
#[test]
fn r2_2_new_cell_under_any_event_reports() {
    let mut latch: Latch = Some((MouseButton::Left, 2, 5));
    let out = reported_drag_motion(
        &mut latch,
        MouseProtocol::AnyEvent,
        MouseEncoding::Sgr,
        (2, 6),
    );
    assert_eq!(out, Some(b"\x1b[<32;3;7M".to_vec()));
    assert_eq!(latch, Some((MouseButton::Left, 2, 6)));
}

/// R2.3: The same cell sends nothing and leaves the latch unchanged.
#[test]
fn r2_3_same_cell_sends_nothing() {
    let mut latch: Latch = Some((MouseButton::Left, 2, 5));
    let out = reported_drag_motion(
        &mut latch,
        MouseProtocol::AnyEvent,
        MouseEncoding::Sgr,
        (2, 5),
    );
    assert_eq!(out, None);
    assert_eq!(latch, Some((MouseButton::Left, 2, 5)));
}

/// R2.4: Click-only and off protocols send no motion and keep the latch.
#[test]
fn r2_4_click_only_and_off_protocols_send_no_motion() {
    for p in [
        MouseProtocol::Normal,
        MouseProtocol::X10,
        MouseProtocol::Off,
    ] {
        let mut latch: Latch = Some((MouseButton::Left, 2, 5));
        let out = reported_drag_motion(&mut latch, p, MouseEncoding::Sgr, (3, 5));
        assert_eq!(out, None, "protocol {p:?}");
        assert_eq!(latch, Some((MouseButton::Left, 2, 5)), "protocol {p:?}");
    }
}

/// R2.5: No latched button sends nothing; the latch stays `None`.
#[test]
fn r2_5_no_latched_button_sends_nothing() {
    let mut latch: Latch = None;
    let out = reported_drag_motion(
        &mut latch,
        MouseProtocol::AnyEvent,
        MouseEncoding::Sgr,
        (3, 5),
    );
    assert_eq!(out, None);
    assert_eq!(latch, None);
}

/// R2.6: A legacy cell past 223 sends nothing and keeps the latch.
#[test]
fn r2_6_legacy_cell_past_223_sends_nothing_and_keeps_latch() {
    let mut latch: Latch = Some((MouseButton::Left, 2, 5));
    let out = reported_drag_motion(
        &mut latch,
        MouseProtocol::ButtonEvent,
        MouseEncoding::Legacy,
        (250, 5),
    );
    assert_eq!(out, None);
    assert_eq!(latch, Some((MouseButton::Left, 2, 5)));
}

/// R2.7: Press, two moves and release write the full drag sequence; the release
/// lands on the last moved cell.
#[test]
fn r2_7_press_two_moves_release_full_sequence() {
    let p = MouseProtocol::AnyEvent;
    let e = MouseEncoding::Sgr;
    let mods = ModifiersState::empty();

    let mut out: Vec<u8> = Vec::new();
    match grid_press_action(p, e, MouseButton::Left, (2, 5), &mods) {
        GridPressAction::Report(b) => out.extend(b),
        other => panic!("press: expected Report, got {other:?}"),
    }
    // The event loop records the latch on a reported press.
    let mut latch: Latch = Some((MouseButton::Left, 2, 5));

    out.extend(reported_drag_motion(&mut latch, p, e, (3, 5)).expect("move 1"));
    assert_eq!(latch, Some((MouseButton::Left, 3, 5)));
    out.extend(reported_drag_motion(&mut latch, p, e, (6, 5)).expect("move 2"));
    assert_eq!(latch, Some((MouseButton::Left, 6, 5)));

    match grid_release_action(latch, p, e, MouseButton::Left, &mods) {
        GridReleaseAction::Report(b) => out.extend(b),
        other => panic!("release: expected Report, got {other:?}"),
    }

    let expected: Vec<u8> = [
        &b"\x1b[<0;3;6M"[..],
        &b"\x1b[<32;4;6M"[..],
        &b"\x1b[<32;7;6M"[..],
        &b"\x1b[<0;7;6m"[..],
    ]
    .concat();
    assert_eq!(out, expected);
}

/// R2.8: Tracking narrowed to `?1000` mid-drag releases at the last moved cell.
#[test]
fn r2_8_narrowed_to_normal_mid_drag_releases_at_last_moved_cell() {
    let e = MouseEncoding::Sgr;
    let mut latch: Latch = Some((MouseButton::Left, 2, 5));

    // Given: a move to (4, 5) under AnyEvent has moved the latch.
    assert!(reported_drag_motion(&mut latch, MouseProtocol::AnyEvent, e, (4, 5)).is_some());
    assert_eq!(latch, Some((MouseButton::Left, 4, 5)));

    // When: the protocol becomes Normal and the pointer moves to (7, 5).
    let motion = reported_drag_motion(&mut latch, MouseProtocol::Normal, e, (7, 5));
    assert_eq!(motion, None);
    assert_eq!(latch, Some((MouseButton::Left, 4, 5)));

    let release = grid_release_action(
        latch,
        MouseProtocol::Normal,
        e,
        MouseButton::Left,
        &ModifiersState::empty(),
    );
    assert_eq!(release, GridReleaseAction::Report(b"\x1b[<0;5;6m".to_vec()));
}

/// R2.9: A latched release reports even with Cmd or Shift held; under `Off` it
/// is ignored.
#[test]
fn r2_9_latched_release_reports_with_cmd_or_shift_held() {
    let latch: Latch = Some((MouseButton::Left, 6, 5));
    for mods in [ModifiersState::SUPER, ModifiersState::SHIFT] {
        let action = grid_release_action(
            latch,
            MouseProtocol::AnyEvent,
            MouseEncoding::Sgr,
            MouseButton::Left,
            &mods,
        );
        assert_eq!(
            action,
            GridReleaseAction::Report(b"\x1b[<0;7;6m".to_vec()),
            "mods {mods:?}"
        );
    }
    for mods in [ModifiersState::SUPER, ModifiersState::SHIFT] {
        let action = grid_release_action(
            latch,
            MouseProtocol::Off,
            MouseEncoding::Sgr,
            MouseButton::Left,
            &mods,
        );
        assert_eq!(action, GridReleaseAction::Ignore, "Off, mods {mods:?}");
    }
}

/// R2.10: Only a latched press under `?1002`/`?1003` owns the pointer.
#[test]
fn r2_10_only_latched_press_under_drag_protocols_owns_pointer() {
    let latch: Latch = Some((MouseButton::Left, 2, 5));
    assert!(drag_owns_pointer(latch, MouseProtocol::AnyEvent));
    assert!(drag_owns_pointer(latch, MouseProtocol::ButtonEvent));
    assert!(!drag_owns_pointer(latch, MouseProtocol::Normal));
    assert!(!drag_owns_pointer(latch, MouseProtocol::X10));
    assert!(!drag_owns_pointer(latch, MouseProtocol::Off));
    assert!(!drag_owns_pointer(None, MouseProtocol::AnyEvent));
}
