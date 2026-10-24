//! TT6 — acceptance tests for `specs/session-flow-graph` R6 (old D2, old D13): a session
//! card's status in the graph is the sidebar card's, computed by the card's own join.
//!
//! * An unsplit session and a split tab's **active** pane take `row_meta(..).status`, the
//!   blocking-dialog override included (R6.1).
//! * A **non-active** pane is log-only — `resolve_status(Some(&agent), false, false, None)` —
//!   and an `Unknown` Claude state is still a session node, drawn undotted (R6.2).
//!
//! Every fixture is hand-built data: no tmux, no logs, no process table. Every app item is
//! reached through the `app` crate root; the graph's types through `renderer::graph_layout`.
//! Panes are told apart by D1's "inside a split tab, pane order": both pane sessions here
//! sit outside any repository, so they form the non-git tail in pane order.

use std::collections::HashMap;
use std::path::PathBuf;

use ai_monitor::telemetry::Subagent;
use app::{
    build_graph, resolve_status, row_meta, Agent, AgentKind, AgentState, FoldSet, PaneMeta, TabId,
    TabMeta,
};
use renderer::graph_layout::{FlowGraph, FlowNode, FlowTarget, NodeKind};
use renderer::sidebar_card::CardStatus;
use renderer::sidebar_row::SidebarRow;

const TAB: u64 = 7;

const ALL_STATES: [AgentState; 4] = [
    AgentState::Busy,
    AgentState::Waiting,
    AgentState::Idle,
    AgentState::Unknown,
];

fn claude(state: AgentState) -> Agent {
    Agent {
        kind: AgentKind::Claude,
        state,
    }
}

fn row(blocking_dialog: bool) -> SidebarRow {
    SidebarRow {
        tab_id: TAB,
        title: "work".to_string(),
        blocking_dialog,
        ..Default::default()
    }
}

fn unsplit_meta(state: AgentState) -> TabMeta {
    TabMeta {
        agent: Some(claude(state)),
        ..Default::default()
    }
}

fn pane(pid: u32, active: bool, agent: Option<Agent>) -> PaneMeta {
    PaneMeta {
        pane_pid: pid,
        active,
        agent,
        log: None,
        cwd: None,
        branch: None,
        repo: None,
    }
}

/// `build_graph` over one row and its meta, with no subagents and no folds.
fn graph_of(row: &SidebarRow, meta: TabMeta) -> FlowGraph {
    let mut metas = HashMap::new();
    metas.insert(TabId(row.tab_id), meta);
    let subagents: HashMap<PathBuf, Vec<Subagent>> = HashMap::new();
    let folds = FoldSet::new();
    // The active strip index only decides `selected`, which R6 does not read.
    let (graph, _) = build_graph(
        std::slice::from_ref(row),
        &metas,
        None,
        &subagents,
        &folds,
        0,
    );
    graph
}

/// The session nodes of tab `TAB`, in node order.
fn sessions(graph: &FlowGraph) -> Vec<&FlowNode> {
    graph
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::Session && n.target == FlowTarget::Node(TAB))
        .collect()
}

// R6.1
#[test]
fn r6_1_unsplit_session_status_equals_the_cards_in_every_combination() {
    for state in ALL_STATES {
        for dialog in [false, true] {
            let r = row(dialog);
            let meta = unsplit_meta(state);
            let card = row_meta(&r, Some(&meta), None).status;
            let graph = graph_of(&r, meta);
            let got = sessions(&graph);
            assert_eq!(
                got.len(),
                1,
                "one session node expected (state={state:?}, dialog={dialog})"
            );
            assert_eq!(
                got[0].status, card,
                "session status must equal row_meta(..).status (state={state:?}, dialog={dialog})"
            );
        }
    }
}

// R6.1
#[test]
fn r6_1_active_pane_takes_the_cards_status_and_the_other_pane_does_not() {
    let r = row(true);
    let meta = TabMeta {
        agent: Some(claude(AgentState::Busy)),
        panes: vec![
            pane(10, true, Some(claude(AgentState::Busy))),
            pane(11, false, Some(claude(AgentState::Busy))),
        ],
        ..Default::default()
    };
    let graph = graph_of(&r, meta);
    let got = sessions(&graph);
    assert_eq!(got.len(), 2, "one session node per Claude pane");
    // Pane order (D1): pane 10 first, pane 11 second.
    assert_eq!(
        got[0].status,
        CardStatus::WaitingForInput,
        "active pane 10 takes the card's status, dialog override included"
    );
    assert_eq!(
        got[1].status,
        CardStatus::Working,
        "non-active pane 11 ignores the tab's blocking dialog"
    );
}

// R6.2
#[test]
fn r6_2_non_active_pane_is_log_only() {
    let waiting = claude(AgentState::Waiting);
    let r = row(false);
    let meta = TabMeta {
        // The card reads Working for the tab; the non-active pane must not follow it.
        agent: Some(claude(AgentState::Busy)),
        panes: vec![
            pane(10, true, Some(claude(AgentState::Busy))),
            pane(11, false, Some(waiting)),
        ],
        ..Default::default()
    };
    let graph = graph_of(&r, meta);
    let got = sessions(&graph);
    assert_eq!(got.len(), 2, "one session node per Claude pane");
    assert_eq!(
        got[1].status,
        resolve_status(Some(&waiting), false, false, None),
        "non-active pane 11's status is resolve_status(Some(&agent), false, false, None)"
    );
}

// R6.2
#[test]
fn r6_2_unknown_state_is_a_session_node_with_hidden_status() {
    let graph = graph_of(&row(false), unsplit_meta(AgentState::Unknown));
    let got = sessions(&graph);
    assert_eq!(
        got.len(),
        1,
        "an Unknown Claude state is still a session node"
    );
    assert_eq!(got[0].status, CardStatus::Hidden);
}
