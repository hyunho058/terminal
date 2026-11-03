//! TT4 — acceptance test for `specs/session-overview-ui` R4.5 (D6, D10): pressing a filter
//! tab selects it and resets the table scroll; the filter survives a close and reopen within
//! one overlay's life, and a fresh `FlowOverlay` starts at `All`.
//!
//! Written from the spec's Then by the checker side, against T8's overlay: `FlowOverlay::new`
//! over a `renderer::overview_panel::OverviewHandle`. **This file does not compile until T8**
//! switches the constructor from the old graph's `FlowHandle`; that is expected, and is why
//! it is kept apart from the other TT4 files.
//!
//! The overlay is driven only through the `Overlay` trait and `sync_snapshot`. A frame is
//! stood in by `OverviewHandle::publish` — one `Filter(Working)` hit under [`POINT`] and a
//! scroll bound — exactly as the panel publishes after it composes. The filter is read back
//! from the model the overlay sets on the handle, after a `poll` gives it the chance to
//! rebuild.

use std::sync::Arc;

use app::{AppCtx, FlowOverlay, Handled, KeyInput, Overlay, TabMetaSnapshot};
use renderer::overview_model::{Filter, OverviewHit};
use renderer::overview_panel::OverviewHandle;
use winit::keyboard::{Key, KeyCode, ModifiersState, PhysicalKey};

const POINT: [f32; 2] = [150.0, 130.0];
const WORKING_TAB: [f32; 4] = [100.0, 100.0, 120.0, 60.0];

fn cmd_shift_f() -> KeyInput {
    KeyInput {
        key: Key::Character("F".into()),
        text: None,
        mods: ModifiersState::SUPER | ModifiersState::SHIFT,
        physical: PhysicalKey::Code(KeyCode::KeyF),
    }
}

/// Press the chord through `Overlay::on_key`.
fn chord(overlay: &mut FlowOverlay) {
    let mut pty = Vec::new();
    let mut ctx = AppCtx::new(&mut pty, 1, 0, None);
    assert_eq!(overlay.on_key(&cmd_shift_f(), &mut ctx), Handled::Consumed);
}

/// Stand in the worker's first full pass for this open (no tabs), and let the overlay
/// rebuild its model from it.
fn ready_and_poll(overlay: &mut FlowOverlay) {
    overlay.sync_snapshot(TabMetaSnapshot::for_test(Vec::new(), true));
    overlay.poll();
}

// R4.5
#[test]
fn r4_5_filter_press_persists_and_resets_scroll() {
    let handle = OverviewHandle::new();
    let mut overlay = FlowOverlay::new(Arc::new(|| {}), None, handle.clone());

    // Given: the overview is open, scrolled down 3 lines, filter `All`.
    chord(&mut overlay);
    assert!(
        overlay.visible(),
        "precondition: the chord opened the overlay"
    );
    ready_and_poll(&mut overlay);
    handle.publish(
        vec![(OverviewHit::Filter(Filter::Working), WORKING_TAB)],
        10,
    );
    handle.set_scroll(3);
    assert_eq!(handle.scroll(), 3, "precondition: scrolled down 3 lines");
    assert_eq!(
        handle.model().filter,
        Filter::All,
        "precondition: filter All"
    );
    assert_eq!(
        handle.hit_at(POINT),
        Some(OverviewHit::Filter(Filter::Working)),
        "precondition: the point lands on the Working tab"
    );

    // When: a press lands on the `Working` filter tab.
    let mut pty = Vec::new();
    let mut ctx = AppCtx::new(&mut pty, 1, 0, None);
    assert_eq!(overlay.on_press(POINT, &mut ctx), Handled::Consumed);
    overlay.poll();

    // Then: the filter is `Working` with table scroll 0.
    assert!(overlay.visible(), "a filter press keeps the overlay open");
    assert_eq!(
        handle.model().filter,
        Filter::Working,
        "filter after the press"
    );
    assert_eq!(handle.scroll(), 0, "selecting a filter resets the scroll");

    // When: closed and reopened via the chord.
    chord(&mut overlay);
    assert!(
        !overlay.visible(),
        "precondition: the chord closed the overlay"
    );
    chord(&mut overlay);
    assert!(
        overlay.visible(),
        "precondition: the chord reopened the overlay"
    );
    ready_and_poll(&mut overlay);

    // Then: still `Working`.
    assert_eq!(
        handle.model().filter,
        Filter::Working,
        "the filter is kept across close and reopen"
    );
}

// R4.5
#[test]
fn r4_5_a_fresh_overlay_starts_at_all() {
    let handle = OverviewHandle::new();
    let mut overlay = FlowOverlay::new(Arc::new(|| {}), None, handle.clone());

    chord(&mut overlay);
    assert!(
        overlay.visible(),
        "precondition: the chord opened the overlay"
    );
    ready_and_poll(&mut overlay);

    assert_eq!(handle.model().filter, Filter::All);
}
