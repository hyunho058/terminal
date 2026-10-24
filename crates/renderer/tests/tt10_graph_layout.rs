//! Acceptance tests for session-flow-graph R10 (D9, D12): the pure graph layout —
//! slots and columns, card width, the narrow modes, and the sampled S-curve edges.
//!
//! Written test-first against `specs/session-flow-graph/spec.md` § T3. Every expected
//! number is derived from the spec's constants (`COL_GAP`, `MARGIN`, `MIN_CARD_W`,
//! `HEADER_H`, `EDGE_SEGMENTS`) and the real `CardMetrics::DEFAULT`, never typed in.
//!
//! Beyond what the spec names, these tests assume the layout result is a struct with
//! `cards` (`node`, `slot`, `rect = [x, y, w, h]`), `plus_chips` (`node`, `n`, `rect`),
//! `edges` (`parent`, `child`, `segments: Vec<SidebarShape>`, `ring: SidebarShape`),
//! `header: [f32; 4]` and a pure `hit(point) -> Option<FlowHit>`; that the viewport is
//! `[w, h]`; and that the constants are logical-px `f32`s.

use renderer::graph_layout::{
    layout_graph, FlowGraph, FlowNode, FlowTarget, NodeKind, COL_GAP, EDGE_SEGMENTS, HEADER_H,
    MARGIN, MIN_CARD_W,
};
use renderer::sidebar_card::CardStatus;
use renderer::sidebar_shapes::{SidebarShape, KIND_CAPSULE, KIND_RING};
use renderer::sidebar_theme::{AgentSkin, CardMetrics};

const SCALE: f64 = 1.0;
/// R10.2's `[sidebar].width_px`, logical px.
const SIDEBAR_W: f32 = 280.0;
/// Endpoint tolerance named by R10.4.
const TOL: f32 = 0.5;

const TAB_A: u64 = 1;
const TAB_B: u64 = 2;

// Node indices of the R10.1 graph, in builder (pre-order) order.
const PROJECT: usize = 0;
const SESSION_A: usize = 1;
const SUBAGENTS: [usize; 3] = [2, 3, 4];
const SESSION_B: usize = 5;

fn node(
    kind: NodeKind,
    parent: Option<usize>,
    title: &str,
    target: FlowTarget,
    status: CardStatus,
    extra: u32,
) -> FlowNode {
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
        status,
        muted: false,
        selected: false,
        target,
        extra,
    }
}

/// R10.1's Given: one project with two sessions, the first with three running
/// subagents (so its `+N` is 3: three running, none finished), the second with none.
fn graph() -> FlowGraph {
    let mut nodes = vec![
        node(
            NodeKind::Project,
            None,
            "terminal",
            FlowTarget::None,
            CardStatus::Hidden,
            0,
        ),
        node(
            NodeKind::Session,
            Some(PROJECT),
            "session a",
            FlowTarget::Node(TAB_A),
            CardStatus::Working,
            3,
        ),
    ];
    for title in ["Explore", "Plan", "fix"] {
        nodes.push(node(
            NodeKind::Subagent,
            Some(SESSION_A),
            title,
            FlowTarget::Node(TAB_A),
            CardStatus::Working,
            0,
        ));
    }
    nodes.push(node(
        NodeKind::Session,
        Some(PROJECT),
        "session b",
        FlowTarget::Node(TAB_B),
        CardStatus::Ready,
        0,
    ));
    assert_eq!(nodes[SESSION_B].kind, NodeKind::Session);
    FlowGraph {
        nodes,
        resolving: false,
    }
}

fn metrics() -> CardMetrics {
    CardMetrics::DEFAULT
}

fn card_h() -> f32 {
    CardMetrics::DEFAULT.scaled(SCALE).height
}

fn s() -> f32 {
    SCALE as f32
}

/// Viewport width at which exactly `cols` columns of `MIN_CARD_W` cards, their gaps and
/// both margins fit with nothing to spare.
fn fit_width(cols: f32) -> f32 {
    (cols * MIN_CARD_W + (cols - 1.0) * COL_GAP + 2.0 * MARGIN) * s()
}

fn column_of(kind: NodeKind) -> usize {
    match kind {
        NodeKind::Project => 0,
        NodeKind::Session => 1,
        NodeKind::Subagent | NodeKind::Fold => 2,
    }
}

fn overlap(a: [f32; 4], b: [f32; 4]) -> bool {
    let ix = (a[0] + a[2]).min(b[0] + b[2]) - a[0].max(b[0]);
    let iy = (a[1] + a[3]).min(b[1] + b[3]) - a[1].max(b[1]);
    ix > 0.0 && iy > 0.0
}

fn near(a: [f32; 2], b: [f32; 2]) -> bool {
    (a[0] - b[0]).abs() <= TOL && (a[1] - b[1]).abs() <= TOL
}

fn p0(shape: &SidebarShape) -> [f32; 2] {
    [shape.seg[0], shape.seg[1]]
}

fn p1(shape: &SidebarShape) -> [f32; 2] {
    [shape.seg[2], shape.seg[3]]
}

// ---------------------------------------------------------------------------------
// R10.1: Slots and columns
// ---------------------------------------------------------------------------------

#[test]
fn r10_1_slots_follow_the_subtree() {
    let g = graph();
    let layout = layout_graph(&g, [1600.0, 1000.0], SCALE, metrics(), SIDEBAR_W);
    let slot = |n: usize| {
        layout
            .cards
            .iter()
            .find(|c| c.node == n)
            .unwrap_or_else(|| panic!("node {n} has no card"))
            .slot
    };
    assert_eq!(slot(PROJECT), 0, "the project starts in slot 0");
    assert_eq!(slot(SESSION_A), 0, "the first session starts in slot 0");
    for (i, &sub) in SUBAGENTS.iter().enumerate() {
        assert_eq!(slot(sub), i, "subagent {i} occupies slot {i}");
    }
    assert_eq!(
        slot(SESSION_B),
        SUBAGENTS.len(),
        "the second session follows the first's three slots"
    );
    assert_eq!(layout.cards.len(), g.nodes.len(), "every node is a card");
}

#[test]
fn r10_1_every_card_is_card_height() {
    let layout = layout_graph(&graph(), [1600.0, 1000.0], SCALE, metrics(), SIDEBAR_W);
    for c in &layout.cards {
        assert!(
            (c.rect[3] - card_h()).abs() < 1e-3,
            "node {} is {} tall, want card_h {}",
            c.node,
            c.rect[3],
            card_h()
        );
    }
}

#[test]
fn r10_1_columns_share_x_and_increase() {
    let g = graph();
    let layout = layout_graph(&g, [1600.0, 1000.0], SCALE, metrics(), SIDEBAR_W);
    let mut col_x: [Option<f32>; 3] = [None; 3];
    for c in &layout.cards {
        let k = column_of(g.nodes[c.node].kind);
        match col_x[k] {
            None => col_x[k] = Some(c.rect[0]),
            Some(x) => assert!(
                (x - c.rect[0]).abs() < 1e-3,
                "column {k}: node {} at x {} but column x is {x}",
                c.node,
                c.rect[0]
            ),
        }
    }
    let [x0, x1, x2] = col_x.map(|x| x.expect("all three columns laid out"));
    assert!(x0 < x1 && x1 < x2, "column x must increase: {x0} {x1} {x2}");
}

#[test]
fn r10_1_slots_stack_downward() {
    let layout = layout_graph(&graph(), [1600.0, 1000.0], SCALE, metrics(), SIDEBAR_W);
    for a in &layout.cards {
        for b in &layout.cards {
            if a.slot == b.slot {
                assert!((a.rect[1] - b.rect[1]).abs() < 1e-3, "same slot, same y");
            } else if a.slot < b.slot {
                assert!(a.rect[1] < b.rect[1], "a later slot sits lower");
            }
        }
    }
}

#[test]
fn r10_1_no_two_cards_overlap() {
    let layout = layout_graph(&graph(), [1600.0, 1000.0], SCALE, metrics(), SIDEBAR_W);
    for (i, a) in layout.cards.iter().enumerate() {
        for b in &layout.cards[i + 1..] {
            assert!(
                !overlap(a.rect, b.rect),
                "nodes {} {:?} and {} {:?} overlap",
                a.node,
                a.rect,
                b.node,
                b.rect
            );
        }
    }
}

// ---------------------------------------------------------------------------------
// R10.2: Card width shrinks to fit three columns, with a floor
// ---------------------------------------------------------------------------------

fn card_widths(viewport_w: f32) -> Vec<f32> {
    let layout = layout_graph(&graph(), [viewport_w, 1000.0], SCALE, metrics(), SIDEBAR_W);
    layout.cards.iter().map(|c| c.rect[2]).collect()
}

#[test]
fn r10_2_wide_viewport_uses_the_sidebar_card_width() {
    let sidebar_card_w = (SIDEBAR_W - 2.0 * CardMetrics::DEFAULT.inset_x) * s();
    let widths = card_widths(1600.0);
    assert!(!widths.is_empty());
    for w in widths {
        assert!(
            (w - sidebar_card_w).abs() < 1e-3,
            "at 1600 the card is {w}, want the sidebar card's {sidebar_card_w}"
        );
    }
}

#[test]
fn r10_2_mid_viewport_shrinks_to_fit_three_columns() {
    let vw = 700.0;
    let fitted = (vw - 2.0 * COL_GAP * s() - 2.0 * MARGIN * s()) / 3.0;
    assert!(
        fitted >= MIN_CARD_W * s(),
        "R10.2's 700 px is meant to fit three columns above the floor"
    );
    assert!(
        fitted < (SIDEBAR_W - 2.0 * CardMetrics::DEFAULT.inset_x) * s(),
        "and to be narrower than the sidebar card"
    );
    let layout = layout_graph(&graph(), [vw, 1000.0], SCALE, metrics(), SIDEBAR_W);
    assert!(
        layout
            .cards
            .iter()
            .any(|c| graph().nodes[c.node].kind == NodeKind::Subagent),
        "three columns still fit at 700"
    );
    for c in &layout.cards {
        let w = c.rect[2];
        assert!(w >= MIN_CARD_W * s(), "card {w} below MIN_CARD_W");
        assert!(
            3.0 * w + 2.0 * COL_GAP * s() + 2.0 * MARGIN * s() <= vw + 1e-3,
            "card {w} does not fit three columns in {vw}"
        );
        // Largest that fits: allow a whole-pixel snap, no more.
        assert!(
            w > fitted - 1.0,
            "card {w} is not the largest fitting width {fitted}"
        );
    }
}

#[test]
fn r10_2_narrow_viewport_drops_column_two() {
    let vw = 500.0;
    assert!(
        vw < fit_width(3.0),
        "500 px is below three columns at the floor"
    );
    let g = graph();
    let layout = layout_graph(&g, [vw, 1000.0], SCALE, metrics(), SIDEBAR_W);
    assert!(
        layout
            .cards
            .iter()
            .all(|c| column_of(g.nodes[c.node].kind) != 2),
        "column 2 must not be laid out at 500"
    );
    assert!(!layout.cards.is_empty(), "columns 0 and 1 still are at 500");
}

// ---------------------------------------------------------------------------------
// R10.3: Narrow modes
// ---------------------------------------------------------------------------------

#[test]
fn r10_3a_two_column_mode_one_slot_per_session_and_inert_plus_chip() {
    let vw = fit_width(3.0) - 1.0;
    assert!(
        vw >= fit_width(2.0),
        "the (a) viewport still fits two columns"
    );
    let g = graph();
    let layout = layout_graph(&g, [vw, 1000.0], SCALE, metrics(), SIDEBAR_W);

    let card = |n: usize| layout.cards.iter().find(|c| c.node == n);
    for &sub in &SUBAGENTS {
        assert!(card(sub).is_none(), "subagent {sub} must not be laid out");
    }
    assert_eq!(card(PROJECT).expect("project card").slot, 0);
    assert_eq!(card(SESSION_A).expect("session a card").slot, 0);
    assert_eq!(
        card(SESSION_B).expect("session b card").slot,
        1,
        "a session with subagents takes exactly one slot"
    );

    assert_eq!(
        layout.plus_chips.len(),
        1,
        "only the first session has a +N"
    );
    let chip = &layout.plus_chips[0];
    assert_eq!(chip.node, SESSION_A);
    assert_eq!(chip.n, 3, "+N counts the three running subagents");

    let a = card(SESSION_A).unwrap().rect;
    let cx = chip.rect[0] + chip.rect[2] / 2.0;
    let cy = chip.rect[1] + chip.rect[3] / 2.0;
    assert!(
        cy >= a[1] && cy <= a[1] + a[3],
        "the chip sits on its card's row"
    );
    assert!(
        cx > a[0] + a[2] / 2.0,
        "the chip sits at its card's right edge"
    );
    assert_eq!(layout.hit([cx, cy]), None, "the +N chip is inert");
}

#[test]
fn r10_3b_one_column_mode_sessions_only_in_group_order() {
    let vw = fit_width(2.0) - 1.0;
    assert!(vw >= fit_width(1.0), "the (b) viewport still fits one card");
    let g = graph();
    let layout = layout_graph(&g, [vw, 1000.0], SCALE, metrics(), SIDEBAR_W);

    assert!(
        layout
            .cards
            .iter()
            .all(|c| g.nodes[c.node].kind == NodeKind::Session),
        "only session cards are laid out"
    );
    let mut cards: Vec<_> = layout.cards.iter().collect();
    cards.sort_by(|a, b| a.rect[1].total_cmp(&b.rect[1]));
    let order: Vec<usize> = cards.iter().map(|c| c.node).collect();
    assert_eq!(order, vec![SESSION_A, SESSION_B], "group order, top down");
    assert!(
        (cards[0].rect[0] - cards[1].rect[0]).abs() < 1e-3,
        "one column: one x"
    );
    assert!(layout.plus_chips.iter().all(|c| c.node != PROJECT));
}

#[test]
fn r10_3c_too_narrow_for_one_card_lays_out_only_the_header() {
    let vw = MIN_CARD_W * s() - 1.0;
    let layout = layout_graph(&graph(), [vw, 1000.0], SCALE, metrics(), SIDEBAR_W);
    assert!(layout.cards.is_empty(), "no cards");
    assert!(layout.plus_chips.is_empty(), "no chips");
    assert!(layout.edges.is_empty(), "no edges");
    assert!(
        (layout.header[3] - HEADER_H * s()).abs() < 1e-3,
        "the header is still laid out, HEADER_H tall"
    );
}

// ---------------------------------------------------------------------------------
// R10.4: Edges are sampled S-curves between card midpoints
// ---------------------------------------------------------------------------------

#[test]
fn r10_4_one_edge_per_parent_child_pair() {
    let g = graph();
    let layout = layout_graph(&g, [1600.0, 1000.0], SCALE, metrics(), SIDEBAR_W);
    let mut pairs: Vec<(usize, usize)> = layout.edges.iter().map(|e| (e.parent, e.child)).collect();
    pairs.sort_unstable();
    let mut want: Vec<(usize, usize)> = g
        .nodes
        .iter()
        .enumerate()
        .filter_map(|(i, n)| n.parent.map(|p| (p, i)))
        .collect();
    want.sort_unstable();
    assert_eq!(want.len(), 5, "the R10.1 graph has five parent→child pairs");
    assert_eq!(pairs, want);
}

#[test]
fn r10_4_edges_chain_from_right_mid_to_left_mid() {
    let layout = layout_graph(&graph(), [1600.0, 1000.0], SCALE, metrics(), SIDEBAR_W);
    let rect = |n: usize| layout.cards.iter().find(|c| c.node == n).unwrap().rect;
    assert_eq!(layout.edges.len(), 5);
    for e in &layout.edges {
        let (p, c) = (rect(e.parent), rect(e.child));
        let start = [p[0] + p[2], p[1] + p[3] / 2.0];
        let end = [c[0], c[1] + c[3] / 2.0];
        let segs = &e.segments;
        assert_eq!(segs.len(), EDGE_SEGMENTS, "edge {}→{}", e.parent, e.child);
        assert_eq!(EDGE_SEGMENTS, 16);
        assert!(segs.iter().all(|s| s.kind == KIND_CAPSULE), "capsules only");
        assert!(
            near(p0(&segs[0]), start),
            "edge {}→{} starts at {:?}, want parent right-mid {start:?}",
            e.parent,
            e.child,
            p0(&segs[0])
        );
        assert!(
            near(p1(&segs[segs.len() - 1]), end),
            "edge {}→{} ends at {:?}, want child left-mid {end:?}",
            e.parent,
            e.child,
            p1(&segs[segs.len() - 1])
        );
        for w in segs.windows(2) {
            assert!(
                near(p1(&w[0]), p0(&w[1])),
                "consecutive capsules must share endpoints: {:?} vs {:?}",
                p1(&w[0]),
                p0(&w[1])
            );
        }
        assert_eq!(e.ring.kind, KIND_RING, "one end ring");
        assert!(
            near(p0(&e.ring), end),
            "the ring sits at the child end: {:?} vs {end:?}",
            p0(&e.ring)
        );
    }
}

/// D9's S-curve: control points are offset horizontally from each end, so the curve
/// leaves the parent and meets the child horizontally and stays between its ends. A
/// straight chord would pass the endpoint checks above; this is what tells them apart.
#[test]
fn r10_4_edges_are_s_curves_not_chords() {
    let layout = layout_graph(&graph(), [1600.0, 1000.0], SCALE, metrics(), SIDEBAR_W);
    let mut bent = 0;
    for e in &layout.edges {
        let segs = &e.segments;
        let start = p0(&segs[0]);
        let end = p1(&segs[segs.len() - 1]);
        let (dx, dy) = (end[0] - start[0], end[1] - start[1]);
        // Monotone in both axes: no overshoot past either end.
        let (ylo, yhi) = (start[1].min(end[1]), start[1].max(end[1]));
        for s in segs {
            let a = p0(s);
            let b = p1(s);
            assert!(b[0] >= a[0] - TOL, "x runs parent → child");
            assert!(
                a[1] >= ylo - TOL && a[1] <= yhi + TOL,
                "y stays between ends"
            );
        }
        if dy.abs() > 1.0 {
            bent += 1;
            let first = &segs[0];
            let fdx = first.seg[2] - first.seg[0];
            let fdy = first.seg[3] - first.seg[1];
            assert!(fdx > 0.0, "the first capsule heads right");
            assert!(
                (fdy / fdx).abs() < (dy / dx).abs(),
                "edge {}→{} leaves its parent no flatter than a straight chord",
                e.parent,
                e.child
            );
            let last = &segs[segs.len() - 1];
            let ldx = last.seg[2] - last.seg[0];
            let ldy = last.seg[3] - last.seg[1];
            assert!(
                (ldy / ldx).abs() < (dy / dx).abs(),
                "edge {}→{} meets its child no flatter than a straight chord",
                e.parent,
                e.child
            );
        }
    }
    assert!(bent >= 3, "the R10.1 graph has edges spanning slots");
}
