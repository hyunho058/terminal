//! Acceptance tests for session-flow-graph R11.1–R11.2 (D13): the graph scrolls by whole
//! slots, clamped so the last slot stays reachable, and the composed frame draws whole
//! nodes only and an edge only when both of its ends are drawn.
//!
//! Written from `specs/session-flow-graph/spec.md` § R11 against T3's
//! `renderer::graph_layout` / `renderer::graph_panel`. Every geometric number is derived
//! from `HEADER_H` and the real `CardMetrics::DEFAULT`, never typed in. R11.3 (the app's
//! wheel routing) lives in `crates/app/tests/tt11_graph_wheel.rs`.

use renderer::graph_layout::{
    compose, layout_graph, FlowGraph, FlowNode, FlowTarget, NodeKind, HEADER_H,
};
use renderer::graph_panel::scroll_offset;
use renderer::sidebar_card::CardStatus;
use renderer::sidebar_theme::{AgentSkin, CardMetrics};

const SCALE: f64 = 1.0;
const SIDEBAR_W: f32 = 280.0;
/// Wide enough for all three columns at the sidebar card's own width (R10.2's 1600).
const VIEWPORT_W: f32 = 1600.0;

const TAB_A: u64 = 1;
const TAB_B: u64 = 2;

// Node indices of the R11.2 graph, in builder (pre-order) order: R10.1's graph plus one
// running subagent under the second session.
const PROJECT: usize = 0;
const SESSION_A: usize = 1;
const SUBAGENTS_A: [usize; 3] = [2, 3, 4];
const SESSION_B: usize = 5;
const SUBAGENT_B: usize = 6;

fn node(kind: NodeKind, parent: Option<usize>, title: &str, target: FlowTarget) -> FlowNode {
    FlowNode {
        kind,
        parent,
        title: title.to_string(),
        folder: String::new(),
        branch: None,
        skin: if kind == NodeKind::Project {
            AgentSkin::Generic
        } else {
            AgentSkin::Claude
        },
        status: if kind == NodeKind::Project {
            CardStatus::Hidden
        } else {
            CardStatus::Working
        },
        muted: false,
        selected: false,
        target,
        extra: 0,
    }
}

/// R11.2's Given: one project; the first session with three running subagents, the
/// second with one, so the second session and its subagent share slot 3.
fn graph() -> FlowGraph {
    let mut nodes = vec![
        node(NodeKind::Project, None, "terminal", FlowTarget::None),
        node(
            NodeKind::Session,
            Some(PROJECT),
            "session a",
            FlowTarget::Node(TAB_A),
        ),
    ];
    for title in ["Explore", "Plan", "fix"] {
        nodes.push(node(
            NodeKind::Subagent,
            Some(SESSION_A),
            title,
            FlowTarget::Node(TAB_A),
        ));
    }
    nodes.push(node(
        NodeKind::Session,
        Some(PROJECT),
        "session b",
        FlowTarget::Node(TAB_B),
    ));
    nodes.push(node(
        NodeKind::Subagent,
        Some(SESSION_B),
        "review",
        FlowTarget::Node(TAB_B),
    ));
    assert_eq!(nodes[SUBAGENT_B].parent, Some(SESSION_B));
    FlowGraph {
        nodes,
        resolving: false,
    }
}

/// A viewport height whose area below the header holds exactly `slots` cards and the
/// gaps between them — `slots` whole slots, the last without its trailing gap.
fn height_for(slots: usize) -> f32 {
    let m = CardMetrics::DEFAULT.scaled(SCALE);
    HEADER_H * SCALE as f32 + slots as f32 * m.height + (slots as f32 - 1.0) * m.gap
}

// R11.1
#[test]
fn r11_1_the_offset_clamps_by_slot() {
    let (total, visible) = (60, 20);
    let down = scroll_offset(0, 100, total, visible);
    assert_eq!(
        down, 40,
        "+100 from 0 stops where the last slot is still shown"
    );
    assert_eq!(
        scroll_offset(down, -100, total, visible),
        0,
        "-100 from the bottom stops at the top"
    );
    assert_eq!(
        scroll_offset(0, 100, 10, 20),
        0,
        "nothing scrolls when everything fits"
    );
}

// R11.2
#[test]
fn r11_2_culling_draws_whole_nodes_and_edges_only_with_both_ends() {
    let layout = layout_graph(
        &graph(),
        [VIEWPORT_W, height_for(3)],
        SCALE,
        CardMetrics::DEFAULT,
        SIDEBAR_W,
    );
    // Preconditions of the Given: three columns, three slots fit, the second session and
    // its subagent share slot 3, and that edge exists in the layout.
    assert_eq!(
        layout.visible_slots, 3,
        "precondition: the viewport fits 3 slots"
    );
    let slot_of = |n: usize| {
        layout
            .cards
            .iter()
            .find(|c| c.node == n)
            .map(|c| c.slot)
            .unwrap_or_else(|| panic!("precondition: node {n} is laid out"))
    };
    assert_eq!(slot_of(PROJECT), 0, "precondition");
    assert_eq!(slot_of(SESSION_A), 0, "precondition");
    assert_eq!(slot_of(SESSION_B), 3, "precondition");
    assert_eq!(slot_of(SUBAGENT_B), 3, "precondition");
    assert!(
        layout
            .edges
            .iter()
            .any(|e| (e.parent, e.child) == (SESSION_B, SUBAGENT_B)),
        "precondition: the second session -> its subagent edge is laid out"
    );

    let frame = compose(&layout, 1, None);

    for gone in [PROJECT, SESSION_A] {
        assert!(
            !frame.drawn.contains(&gone),
            "node {gone} (slot 0) is not drawn at scroll 1"
        );
        assert!(
            !frame.edges.iter().any(|&(p, c)| p == gone || c == gone),
            "no edge from or to node {gone} is drawn: {:?}",
            frame.edges
        );
    }
    for shown in [SUBAGENTS_A[1], SUBAGENTS_A[2], SESSION_B, SUBAGENT_B] {
        assert!(
            frame.drawn.contains(&shown),
            "node {shown} is drawn: {:?}",
            frame.drawn
        );
    }
    assert!(
        frame.edges.contains(&(SESSION_B, SUBAGENT_B)),
        "the second session -> its subagent edge is drawn: {:?}",
        frame.edges
    );
    for &(p, c) in &frame.edges {
        assert!(
            frame.drawn.contains(&p) && frame.drawn.contains(&c),
            "edge ({p}, {c}) is drawn with both ends: {:?}",
            frame.drawn
        );
    }
}
