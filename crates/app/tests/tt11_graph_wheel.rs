//! Acceptance test for session-flow-graph R11.3 (D13): while the graph is open the wheel
//! scrolls it by whole slots. Written from the spec's Then; every app item is reached
//! through the `app` crate root.
//!
//! Targets T4's `FlowOverlay::on_wheel` over `renderer::graph_panel::FlowHandle`, and is
//! red (or does not compile) until T4 lands. "3 notches down" is `WheelDelta::Lines(-3.0)`
//! — winit's `LineDelta` counts notches, positive toward the top — so one notch is one
//! slot. "A graph taller than the viewport" is what the renderer publishes as the
//! handle's `FlowView`: 60 slots of which 20 fit (R11.1's numbers).

use std::sync::Arc;

use app::{AppCtx, FlowOverlay, Handled, KeyInput, Overlay, WheelDelta};
use renderer::graph_panel::{FlowHandle, FlowView};
use winit::keyboard::{Key, KeyCode, ModifiersState, PhysicalKey};

/// Any sane cell height: a notch is a slot whatever the terminal font's line height.
const CELL_H: f32 = 17.0;

fn toggle_chord() -> KeyInput {
    KeyInput {
        key: Key::Character("F".into()),
        text: Some("F".into()),
        mods: ModifiersState::SUPER | ModifiersState::SHIFT,
        physical: PhysicalKey::Code(KeyCode::KeyF),
    }
}

// R11.3
#[test]
fn r11_3_the_wheel_scrolls_the_graph_by_slots() {
    let handle = FlowHandle::default();
    let mut graph = FlowOverlay::new(Arc::new(|| {}), None, handle.clone());
    let mut pty = Vec::new();
    let mut ctx = AppCtx::new(&mut pty, 1, 0, None);

    // Given: an open graph taller than the viewport.
    graph.on_key(&toggle_chord(), &mut ctx);
    assert!(graph.visible(), "precondition: the chord opened the graph");
    handle.publish_view(FlowView {
        hits: Vec::new(),
        visible_slots: 20,
        total_slots: 60,
    });
    assert_eq!(handle.scroll(), 0, "precondition: scrolled to the top");

    // When: 3 notches down.
    let handled = graph.on_wheel(WheelDelta::Lines(-3.0), CELL_H);

    // Then.
    assert_eq!(handled, Handled::Consumed);
    assert_eq!(handle.scroll(), 3, "3 notches move the graph 3 slots");
}
