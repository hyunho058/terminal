//! Acceptance tests for session-flow-graph R2: pointer routing while the graph is open (D6).
//! Written from the spec's Thens; every app item is reached through the `app` crate root.
//!
//! R2.1 and R2.2 pin the pure router T1 built. R2.3 targets T4's `FlowOverlay`, built on
//! `renderer::graph_panel::FlowHandle`, and is red until T4 lands.

use std::sync::Arc;

use app::{pointer_owner, AppCtx, FlowOverlay, Handled, KeyInput, Overlay, PointerOwner};
use renderer::graph_panel::{FlowHandle, FlowHit, FlowView};
use winit::keyboard::{Key, KeyCode, ModifiersState, PhysicalKey};

// R2.1
#[test]
fn r2_1_an_open_graph_owns_the_pointer_wherever_it_lands() {
    for in_monitor_band in [false, true] {
        for in_sidebar in [false, true] {
            assert_eq!(
                pointer_owner(true, in_monitor_band, in_sidebar),
                PointerOwner::Graph,
                "in_monitor_band {in_monitor_band}, in_sidebar {in_sidebar}"
            );
        }
    }
}

// R2.2
#[test]
fn r2_2_a_closed_graph_routes_monitor_then_sidebar_then_grid() {
    let cases = [
        (true, true, PointerOwner::Monitor),
        (true, false, PointerOwner::Monitor),
        (false, true, PointerOwner::Sidebar),
        (false, false, PointerOwner::Grid),
    ];
    for (in_monitor_band, in_sidebar, owner) in cases {
        assert_eq!(
            pointer_owner(false, in_monitor_band, in_sidebar),
            owner,
            "in_monitor_band {in_monitor_band}, in_sidebar {in_sidebar}"
        );
    }
}

fn toggle_chord() -> KeyInput {
    KeyInput {
        key: Key::Character("F".into()),
        text: Some("F".into()),
        mods: ModifiersState::SUPER | ModifiersState::SHIFT,
        physical: PhysicalKey::Code(KeyCode::KeyF),
    }
}

// R2.3
#[test]
fn r2_3_a_press_outside_every_hit_box_is_consumed_and_keeps_the_graph_open() {
    let handle = FlowHandle::default();
    let mut graph = FlowOverlay::new(Arc::new(|| {}), None, handle.clone());
    let mut pty = Vec::new();
    let mut ctx = AppCtx::new(&mut pty, 1, 0, None);

    // Given: an open graph whose handle published hit boxes.
    graph.on_key(&toggle_chord(), &mut ctx);
    assert!(graph.visible(), "precondition: the chord opened the graph");
    ctx.take_requests();
    handle.publish_view(FlowView {
        hits: vec![(FlowHit::Node(1), [100.0, 100.0, 200.0, 60.0])],
        visible_slots: 1,
        total_slots: 1,
    });
    let outside = [20.0, 20.0];
    assert_eq!(
        handle.press(outside),
        None,
        "precondition: the point hits nothing"
    );

    // When / Then.
    assert_eq!(graph.on_press(outside, &mut ctx), Handled::Consumed);
    assert!(
        ctx.take_requests().is_empty(),
        "an unresolved press queues nothing"
    );
    assert!(graph.visible(), "the graph stays open");
}
