//! TT2 — acceptance test for `specs/session-overview-ui` R2.5 (D3, D17): until the worker has
//! published `graph_ready` for this open, the overlay's model is `resolving`, its header reads
//! `resolving…` and no row exists.
//!
//! Written against T8's constructor, `FlowOverlay::new(wake, tabmeta, OverviewHandle)`; this
//! file does not compile until T8 switches the overlay to an `OverviewHandle` (the brief allows
//! that for overlay-driving tests only, which is why it is split from
//! `tt2_overview_identity.rs`). There is no worker (`None`): the snapshot is stated through
//! the `sync_snapshot` seam, not read off the machine.

use std::io::Sink;
use std::sync::Arc;

use app::{
    Agent, AgentKind, AgentState, AppCtx, FlowOverlay, Handled, KeyInput, Overlay, TabId, TabMeta,
    TabMetaSnapshot,
};
use renderer::overview_layout::layout_overview;
use renderer::overview_panel::OverviewHandle;
use renderer::sidebar_row::{SidebarRow, TabBarModel, TabModelHandle};
use winit::keyboard::{Key, KeyCode, ModifiersState, PhysicalKey};

fn cmd_shift_f() -> KeyInput {
    KeyInput {
        key: Key::Character("f".into()),
        text: None,
        mods: ModifiersState::SUPER | ModifiersState::SHIFT,
        physical: PhysicalKey::Code(KeyCode::KeyF),
    }
}

fn row(id: u64) -> SidebarRow {
    SidebarRow {
        tab_id: id,
        title: format!("t{id}"),
        ..SidebarRow::default()
    }
}

fn claude(state: AgentState) -> TabMeta {
    let agent = Some(Agent {
        kind: AgentKind::Claude,
        state,
    });
    TabMeta {
        agent,
        graph_agent: agent,
        ..TabMeta::default()
    }
}

fn two_claude_tabs() -> Vec<(TabId, TabMeta)> {
    vec![
        (TabId(1), claude(AgentState::Busy)),
        (TabId(2), claude(AgentState::Waiting)),
    ]
}

/// An open overlay with no worker over two published sidebar rows.
fn open_overlay() -> (FlowOverlay, OverviewHandle) {
    TabModelHandle::shared().set(TabBarModel {
        titles: vec![row(1), row(2)],
        ..TabBarModel::default()
    });
    let handle = OverviewHandle::new();
    let mut overlay = FlowOverlay::new(Arc::new(|| {}), None, handle.clone());
    let mut sink: Sink = std::io::sink();
    let mut ctx = AppCtx::new(&mut sink, 2, 0, None);
    assert_eq!(
        overlay.on_key(&cmd_shift_f(), &mut ctx),
        Handled::Consumed,
        "fixture: the chord opens the overlay"
    );
    assert!(overlay.visible(), "fixture: the overlay is open");
    (overlay, handle)
}

/// R2.5: two Claude tabs are published, but `graph_ready` is false — the rebuilt model is
/// `resolving`, has no row, and its header reads `resolving…`.
#[test]
fn r2_5_resolving_draws_no_rows_and_the_header_reads_resolving() {
    let (mut overlay, handle) = open_overlay();
    overlay.sync_snapshot(TabMetaSnapshot::for_test(two_claude_tabs(), false));
    overlay.poll();

    let model = handle.model();
    assert!(model.resolving, "the model is resolving before graph_ready");
    assert!(
        model.rows.is_empty(),
        "no row before graph_ready: {:?}",
        model.rows
    );

    let laid = layout_overview(&model, [1400.0, 900.0], 1.0, 0);
    assert!(
        laid.texts.iter().any(|t| t.text == "resolving…"),
        "the header reads `resolving…`: {:?}",
        laid.texts.iter().map(|t| &t.text).collect::<Vec<_>>()
    );
}
