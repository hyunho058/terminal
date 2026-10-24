//! TT7 — acceptance tests for `specs/session-flow-graph` R7 (old D3, D16): the graph's
//! header — what it counts, and `resolving…` until the worker's first full pass.
//!
//! R7.1 reads `FlowGraph::header()` on a hand-built graph (the renderer's T3 model).
//! R7.2 drives a real `FlowOverlay` through its T4 test seam, `sync_snapshot`, with no
//! tabmeta worker (`None`): the snapshot is stated, not read off the machine.

use std::io::Sink;
use std::sync::Arc;

use app::{
    Agent, AgentKind, AgentState, AppCtx, FlowOverlay, Handled, KeyInput, Overlay, TabId, TabMeta,
    TabMetaSnapshot,
};
use renderer::graph_layout::{FlowGraph, FlowNode, FlowTarget, NodeKind};
use renderer::graph_panel::FlowHandle;
use renderer::sidebar_card::CardStatus;
use renderer::sidebar_row::{SidebarRow, TabBarModel, TabModelHandle};
use winit::keyboard::{Key, KeyCode, ModifiersState, PhysicalKey};

// ---------------------------------------------------------------------------------------
// R7.1 fixtures
// ---------------------------------------------------------------------------------------

fn project(title: &str) -> FlowNode {
    FlowNode {
        kind: NodeKind::Project,
        title: title.to_string(),
        status: CardStatus::Hidden,
        target: FlowTarget::None,
        ..FlowNode::default()
    }
}

fn session(parent: Option<usize>, tab: u64, status: CardStatus) -> FlowNode {
    FlowNode {
        kind: NodeKind::Session,
        parent,
        title: format!("s{tab}"),
        status,
        target: FlowTarget::Node(tab),
        ..FlowNode::default()
    }
}

fn subagent(parent: usize, tab: u64) -> FlowNode {
    FlowNode {
        kind: NodeKind::Subagent,
        parent: Some(parent),
        title: "Explore".to_string(),
        status: CardStatus::Working,
        target: FlowTarget::Node(tab),
        ..FlowNode::default()
    }
}

/// R7.1 — `2 working · 1 waiting / 5`: five sessions (two Working, one WaitingForInput, one
/// Ready, one Hidden), two project nodes and three running subagents. Projects and
/// subagents are never counted — the subagents are `Working` precisely so that counting
/// them would show.
#[test]
fn r7_1_header_counts_sessions_only_in_the_exact_format() {
    let nodes = vec![
        project("alpha"),                                 // 0
        session(Some(0), 1, CardStatus::Working),         // 1
        subagent(1, 1),                                   // 2
        subagent(1, 1),                                   // 3
        session(Some(0), 2, CardStatus::WaitingForInput), // 4
        subagent(4, 2),                                   // 5
        project("beta"),                                  // 6
        session(Some(6), 3, CardStatus::Working),         // 7
        session(Some(6), 4, CardStatus::Ready),           // 8
        session(None, 5, CardStatus::Hidden),             // 9 — the non-git tail
    ];
    let graph = FlowGraph {
        nodes,
        resolving: false,
    };
    assert_eq!(graph.header(), "2 working · 1 waiting / 5");
}

/// R7.1 — with no sessions the header is `0 working · 0 waiting / 0`, never empty; a
/// project card alone does not make it non-zero.
#[test]
fn r7_1_header_with_no_sessions_reads_all_zeros() {
    assert_eq!(FlowGraph::default().header(), "0 working · 0 waiting / 0");
    let only_project = FlowGraph {
        nodes: vec![project("alpha")],
        resolving: false,
    };
    assert_eq!(only_project.header(), "0 working · 0 waiting / 0");
}

// ---------------------------------------------------------------------------------------
// R7.2 fixtures
// ---------------------------------------------------------------------------------------

fn cmd_shift_f() -> KeyInput {
    KeyInput {
        key: Key::Character("f".into()),
        text: None,
        mods: ModifiersState::SUPER | ModifiersState::SHIFT,
        physical: PhysicalKey::Code(KeyCode::KeyF),
    }
}

fn claude(state: AgentState) -> TabMeta {
    TabMeta {
        agent: Some(Agent {
            kind: AgentKind::Claude,
            state,
        }),
        ..TabMeta::default()
    }
}

fn row(id: u64) -> SidebarRow {
    SidebarRow {
        tab_id: id,
        title: format!("t{id}"),
        ..SidebarRow::default()
    }
}

/// Two Claude tabs: tab 1 working, tab 2 waiting.
fn two_claude_tabs() -> Vec<(TabId, TabMeta)> {
    vec![
        (TabId(1), claude(AgentState::Busy)),
        (TabId(2), claude(AgentState::Waiting)),
    ]
}

/// An open `FlowOverlay` with no worker, and two Claude tabs published as sidebar rows on
/// this (test) thread's shared model.
fn open_overlay() -> (FlowOverlay, FlowHandle) {
    TabModelHandle::shared().set(TabBarModel {
        titles: vec![row(1), row(2)],
        ..TabBarModel::default()
    });
    let handle = FlowHandle::default();
    let mut overlay = FlowOverlay::new(Arc::new(|| {}), None, handle.clone());
    let mut sink: Sink = std::io::sink();
    let mut ctx = AppCtx::new(&mut sink, 2, 0, None);
    assert_eq!(
        overlay.on_key(&cmd_shift_f(), &mut ctx),
        Handled::Consumed,
        "fixture: the chord opens the graph"
    );
    assert!(overlay.visible(), "fixture: the graph is open");
    (overlay, handle)
}

fn session_count(graph: &FlowGraph) -> usize {
    graph
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::Session)
        .count()
}

/// R7.2 — before the first full pass (`graph_ready: false`) the built model has no nodes
/// and its header reads `resolving…`, though two Claude tabs are published.
#[test]
fn r7_2_until_graph_ready_the_model_is_empty_and_reads_resolving() {
    let (mut overlay, handle) = open_overlay();
    overlay.sync_snapshot(TabMetaSnapshot::for_test(two_claude_tabs(), false));
    overlay.poll();
    let graph = handle.graph();
    assert!(
        graph.nodes.is_empty(),
        "no nodes before graph_ready: {:?}",
        graph.nodes
    );
    assert_eq!(graph.header(), "resolving…");
}

/// R7.2 — once a `graph_ready: true` snapshot is synced, the next build has the nodes and
/// the count header.
#[test]
fn r7_2_once_graph_ready_is_synced_the_next_build_has_nodes_and_counts() {
    let (mut overlay, handle) = open_overlay();
    overlay.sync_snapshot(TabMetaSnapshot::for_test(two_claude_tabs(), false));
    overlay.poll();
    assert_eq!(handle.graph().header(), "resolving…", "precondition");

    overlay.sync_snapshot(TabMetaSnapshot::for_test(two_claude_tabs(), true));
    overlay.poll();
    let graph = handle.graph();
    assert!(!graph.resolving);
    assert_eq!(
        session_count(&graph),
        2,
        "both Claude tabs are session nodes"
    );
    assert_eq!(graph.header(), "1 working · 1 waiting / 2");
}
