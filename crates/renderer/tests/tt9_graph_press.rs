//! TT9 — acceptance test for session-flow-graph R9.1 (D7): the renderer resolves a press
//! on the graph by node — session and subagent cards to their tab, the fold chip to its
//! `(tab, pane)`, and the project card, edges and empty space to nothing.
//!
//! `FlowHandle::press` resolves against the view the panel last *published*, so the
//! handle is given exactly what `FlowPanel` publishes after a frame: the pure
//! `layout_graph` of the graph over the whole viewport (D4), composed at scroll 0 with no
//! hover, its `hits` and slot counts. No GPU: the point under test is the handle's
//! resolution, and every point pressed is derived from the same layout, never typed in.

use renderer::graph_layout::{
    compose, layout_graph, FlowGraph, FlowNode, FlowTarget, GraphLayout, NodeKind, EDGE_SEGMENTS,
};
use renderer::graph_panel::{FlowHandle, FlowHit, FlowView};
use renderer::sidebar_card::CardStatus;
use renderer::sidebar_theme::{AgentSkin, CardMetrics};

/// Wide and tall enough for all three columns (D12's full mode) and every slot.
const VIEWPORT: [f32; 2] = [1600.0, 1000.0];
const SCALE: f64 = 1.0;
/// `[sidebar].width_px`, logical px.
const SIDEBAR_W: f32 = 280.0;

const TAB: u64 = 7;
const PANE: u32 = 10;

// Node indices, in builder pre-order.
const PROJECT: usize = 0;
const SESSION: usize = 1;
const SUBAGENT: usize = 2;
const FOLD: usize = 3;

fn node(kind: NodeKind, parent: Option<usize>, target: FlowTarget) -> FlowNode {
    FlowNode {
        kind,
        parent,
        title: format!("{kind:?}"),
        folder: "~/IdeaProjects/terminal".into(),
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

/// R9.1's Given: a project, a session (tab 7), a subagent (tab 7) and a fold (tab 7,
/// pane 10).
fn graph() -> FlowGraph {
    FlowGraph {
        nodes: vec![
            node(NodeKind::Project, None, FlowTarget::None),
            node(NodeKind::Session, Some(PROJECT), FlowTarget::Node(TAB)),
            node(NodeKind::Subagent, Some(SESSION), FlowTarget::Node(TAB)),
            node(NodeKind::Fold, Some(SESSION), FlowTarget::Fold(TAB, PANE)),
        ],
        resolving: false,
    }
}

/// A handle holding the graph and the view the panel would publish for it.
fn laid_out_handle() -> (FlowHandle, GraphLayout) {
    let g = graph();
    let layout = layout_graph(&g, VIEWPORT, SCALE, CardMetrics::DEFAULT, SIDEBAR_W);
    let frame = compose(&layout, 0, None);
    let handle = FlowHandle::default();
    handle.set(g);
    handle.set_visible(true);
    handle.publish_view(FlowView {
        hits: frame.hits.clone(),
        visible_slots: layout.visible_slots,
        total_slots: layout.total_slots,
    });
    (handle, layout)
}

fn centre(r: [f32; 4]) -> [f32; 2] {
    [r[0] + r[2] * 0.5, r[1] + r[3] * 0.5]
}

fn card_centre(layout: &GraphLayout, n: usize) -> [f32; 2] {
    let card = layout
        .cards
        .iter()
        .find(|c| c.node == n)
        .unwrap_or_else(|| panic!("node {n} has a card"));
    centre(card.rect)
}

fn fold_centre(layout: &GraphLayout, n: usize) -> [f32; 2] {
    let chip = layout
        .folds
        .iter()
        .find(|f| f.node == n)
        .unwrap_or_else(|| panic!("node {n} has a fold chip"));
    centre(chip.rect)
}

/// The midpoint of the `parent → child` edge: the start of its middle capsule, which is
/// the cubic at t = 0.5.
fn edge_midpoint(layout: &GraphLayout, parent: usize, child: usize) -> [f32; 2] {
    let edge = layout
        .edges
        .iter()
        .find(|e| e.parent == parent && e.child == child)
        .unwrap_or_else(|| panic!("an edge {parent} → {child}"));
    let seg = edge.segments[EDGE_SEGMENTS / 2].seg;
    [seg[0], seg[1]]
}

/// A point inside the viewport and outside every card, chip and the header band.
fn empty_space(layout: &GraphLayout) -> [f32; 2] {
    let p = [VIEWPORT[0] - 4.0, VIEWPORT[1] - 4.0];
    let inside =
        |r: [f32; 4]| p[0] >= r[0] && p[0] < r[0] + r[2] && p[1] >= r[1] && p[1] < r[1] + r[3];
    assert!(
        !layout.cards.iter().any(|c| inside(c.rect))
            && !layout.folds.iter().any(|f| inside(f.rect))
            && !layout.plus_chips.iter().any(|c| inside(c.rect))
            && !inside(layout.header),
        "precondition: {p:?} is empty space"
    );
    p
}

// R9.1
#[test]
fn r9_1_press_resolves_cards_and_the_fold_chip_by_node_and_nothing_else() {
    let (handle, layout) = laid_out_handle();

    let points = [
        card_centre(&layout, PROJECT),
        card_centre(&layout, SESSION),
        card_centre(&layout, SUBAGENT),
        fold_centre(&layout, FOLD),
        edge_midpoint(&layout, PROJECT, SESSION),
        empty_space(&layout),
    ];
    let got: Vec<Option<FlowHit>> = points.iter().map(|&p| handle.press(p)).collect();

    assert_eq!(
        got,
        vec![
            None,
            Some(FlowHit::Node(TAB)),
            Some(FlowHit::Node(TAB)),
            Some(FlowHit::Fold(TAB, PANE)),
            None,
            None,
        ],
        "project, session, subagent, fold, edge midpoint, empty space at {points:?}"
    );
}
