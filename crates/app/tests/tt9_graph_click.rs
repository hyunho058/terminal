//! Acceptance tests for session-flow-graph R9.2–R9.4: clicks on the graph (D7).
//! Written test-first from the spec's Thens; every app item is reached through the `app`
//! crate root, plus the renderer's `graph_panel::FlowHandle` the overlay is built over
//! (T3).
//!
//! R9.2 and R9.3 target T4's `FlowOverlay` (built over `graph_panel::FlowHandle`,
//! `on_press` per D7) and are red until T4 lands. The handle is made to resolve the
//! pressed point by publishing a view with one hit box under it — the same seam the panel
//! uses after every composed frame.

use std::sync::Arc;

use app::{tab_position, AppCtx, AppRequest, FlowOverlay, Handled, KeyInput, Overlay, TabId};
use renderer::graph_panel::{FlowHandle, FlowHit, FlowView};
use winit::keyboard::{Key, KeyCode, ModifiersState, PhysicalKey};

const POINT: [f32; 2] = [150.0, 130.0];

fn cmd_shift_f() -> KeyInput {
    KeyInput {
        key: Key::Character("F".into()),
        text: None,
        mods: ModifiersState::SUPER | ModifiersState::SHIFT,
        physical: PhysicalKey::Code(KeyCode::KeyF),
    }
}

/// An open graph overlay whose handle resolves [`POINT`] to `hit`.
fn open_graph_resolving(hit: FlowHit) -> (FlowOverlay, FlowHandle) {
    let handle = FlowHandle::default();
    let mut graph = FlowOverlay::new(Arc::new(|| {}), None, handle.clone());
    let mut pty = Vec::new();
    let mut ctx = AppCtx::new(&mut pty, 1, 0, None);
    graph.on_key(&cmd_shift_f(), &mut ctx);
    assert!(graph.visible(), "precondition: the chord opened the graph");
    handle.publish_view(FlowView {
        hits: vec![(hit, [100.0, 100.0, 200.0, 60.0])],
        visible_slots: 1,
        total_slots: 1,
    });
    assert_eq!(
        handle.press(POINT),
        Some(hit),
        "precondition: the handle resolves the point"
    );
    (graph, handle)
}

// R9.2
#[test]
fn r9_2_a_card_click_activates_its_tab_and_closes_the_graph() {
    let (mut graph, _handle) = open_graph_resolving(FlowHit::Node(7));
    let mut pty = Vec::new();
    let mut ctx = AppCtx::new(&mut pty, 1, 0, None);

    let handled = graph.on_press(POINT, &mut ctx);

    assert_eq!(handled, Handled::Consumed);
    assert_eq!(ctx.take_requests(), vec![AppRequest::ActivateTab(TabId(7))]);
    assert!(!graph.visible(), "the graph closed");
}

// R9.3
#[test]
fn r9_3_a_fold_click_toggles_the_fold_and_keeps_the_graph_open() {
    let (mut graph, _handle) = open_graph_resolving(FlowHit::Fold(7, 10));
    let key = (TabId(7), 10);

    let mut pty = Vec::new();
    let mut ctx = AppCtx::new(&mut pty, 1, 0, None);
    assert_eq!(graph.on_press(POINT, &mut ctx), Handled::Consumed);
    assert!(ctx.take_requests().is_empty(), "first click: no request");
    assert!(graph.folds().contains(&key), "first click expands");
    assert!(graph.visible(), "first click: the graph stays open");

    let mut pty = Vec::new();
    let mut ctx = AppCtx::new(&mut pty, 1, 0, None);
    assert_eq!(graph.on_press(POINT, &mut ctx), Handled::Consumed);
    assert!(ctx.take_requests().is_empty(), "second click: no request");
    assert!(!graph.folds().contains(&key), "second click collapses");
    assert!(graph.visible(), "second click: the graph stays open");
}

// R9.4
#[test]
fn r9_4_activate_tab_resolves_through_the_strip() {
    let ids = [TabId(1), TabId(7), TabId(9)];
    assert_eq!(tab_position(&ids, TabId(7)), Some(1));
    assert_eq!(tab_position(&ids, TabId(4)), None);
}
