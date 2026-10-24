//! TT12 — acceptance tests for session-flow-graph R12: the full-window graph layer
//! draws cards, edges and status colours over an opaque backdrop, and rebuilds only on
//! change (D4, D11, D14, D17).
//!
//! R12.1 goes through `Renderer::capture`, as `golden_frame.rs` does, because every
//! counter this layer exposes (`layout_count`, `compose_count`, `submit_count`) reports
//! a *decision* and stays correct while the frame it describes is blank. The GPU harness
//! is `renderer::testing::headless`: it skips only under `SKIP_GPU_TESTS=1` and fails
//! when the renderer is present but broken.
//!
//! The geometry the pixel test looks at is re-derived by calling the public
//! `graph_layout::layout_graph` with the full viewport (D4: the graph covers the whole
//! window), the renderer's scale and the default sidebar width — so a frame that draws
//! the right primitives in the wrong place, or nothing at all, fails. Every read of the
//! layout's output goes through [`geometry`], the one place that names its fields.

use std::cell::RefCell;
use std::rc::Rc;

use common::config::Config;
use renderer::graph_layout::{layout_graph, FlowGraph, FlowNode, FlowTarget, NodeKind};
use renderer::graph_panel::{FlowHandle, FlowPanel};
use renderer::monitor_model::MonitorModel;
use renderer::monitor_panel::MonitorPanel;
use renderer::sidebar_card::CardStatus;
use renderer::sidebar_theme::{linear, AgentSkin, CardMetrics, SidebarTheme};
use renderer::testing::is_ink;
use renderer::{CapturedFrame, DrawLayer, FrameCtx, Renderer};
use term_core::{Color, GridSnapshot, Terminal};

const FRAME_W: u32 = 1200;
const FRAME_H: u32 = 800;

/// Per-channel slack, in 8-bit sRGB, for "this pixel is that colour": the status icon
/// and the title's stems are opaque at full coverage, but the sRGB round trip moves a
/// channel by a count or two.
const COLOR_TOL: f32 = 12.0;

/// Per-channel slack for "this is the backdrop colour" (`panel_bg` through the sRGB
/// round trip).
const BACKDROP_TOL: f32 = 1.5;

fn headless(cfg: &Config) -> Option<Renderer> {
    renderer::testing::headless(FRAME_W, FRAME_H, cfg, "the session-flow graph frames")
}

// ---- The graph -------------------------------------------------------------------------

fn node(
    kind: NodeKind,
    parent: Option<usize>,
    title: &str,
    skin: AgentSkin,
    status: CardStatus,
    target: FlowTarget,
) -> FlowNode {
    FlowNode {
        kind,
        parent,
        title: title.into(),
        folder: "~/IdeaProjects/terminal".into(),
        branch: None,
        skin,
        status,
        muted: false,
        selected: false,
        target,
        extra: 0,
    }
}

fn project() -> FlowNode {
    node(
        NodeKind::Project,
        None,
        "terminal",
        AgentSkin::Generic,
        CardStatus::Hidden,
        FlowTarget::None,
    )
}

fn session(title: &str, status: CardStatus, tab: u64) -> FlowNode {
    FlowNode {
        branch: Some("main".into()),
        ..node(
            NodeKind::Session,
            Some(PROJECT),
            title,
            AgentSkin::Claude,
            status,
            FlowTarget::Node(tab),
        )
    }
}

/// Node indices of [`r12_1_graph`].
const PROJECT: usize = 0;
const WORKING: usize = 1;
const WAITING: usize = 2;
const READY: usize = 3;
const SUBAGENT: usize = 4;

/// R12.1's graph: a project, a Working, a WaitingForInput and a Ready session, and one
/// running subagent (under the Working session). Four parent→child edges.
fn r12_1_graph() -> FlowGraph {
    FlowGraph {
        nodes: vec![
            project(),
            session("claude working", CardStatus::Working, 1),
            session("claude waiting", CardStatus::WaitingForInput, 2),
            session("claude ready", CardStatus::Ready, 3),
            node(
                NodeKind::Subagent,
                Some(WORKING),
                "general-purpose",
                AgentSkin::Claude,
                CardStatus::Working,
                FlowTarget::Node(1),
            ),
        ],
        resolving: false,
    }
}

/// A project over `n` sessions — far more slots than an 800px frame holds, so a scroll
/// offset is not clamped back to 0. `first` is session 0's status.
fn tall_graph(n: usize, first: CardStatus) -> FlowGraph {
    let mut nodes = vec![project()];
    for i in 0..n {
        let status = if i == 0 { first } else { CardStatus::Working };
        nodes.push(session(&format!("claude {i}"), status, i as u64 + 1));
    }
    FlowGraph {
        nodes,
        resolving: false,
    }
}

// ---- The layout, re-derived ------------------------------------------------------------

/// What the pixel test reads of a layout, in plain types, physical px.
struct Geo {
    /// `(node index, [x, y, w, h])` for every laid-out card.
    cards: Vec<(usize, [f32; 4])>,
    /// Per edge, its capsule chain as `[x0, y0, x1, y1]` segments plus each capsule's
    /// quad bounding box.
    edges: Vec<Vec<([f32; 4], [f32; 4])>>,
    /// The header line's rect.
    header: [f32; 4],
}

impl Geo {
    fn card(&self, node: usize) -> [f32; 4] {
        self.cards
            .iter()
            .find(|c| c.0 == node)
            .map(|c| c.1)
            .unwrap_or_else(|| panic!("node {node} was not laid out as a card"))
    }
}

/// The layout the panel must have drawn: the whole viewport, the renderer's scale, the
/// card metrics and `[sidebar].width_px` — D12's inputs. The only function that names
/// `layout_graph`'s output fields.
fn geometry(r: &Renderer, cfg: &Config, graph: &FlowGraph) -> Geo {
    let lay = layout_graph(
        graph,
        [FRAME_W as f32, FRAME_H as f32],
        r.scale_factor(),
        CardMetrics::DEFAULT,
        cfg.sidebar.width_px,
    );
    Geo {
        cards: lay.cards.iter().map(|c| (c.node, c.rect)).collect(),
        edges: lay
            .edges
            .iter()
            .map(|e| e.segments.iter().map(|s| (s.seg, s.bbox)).collect())
            .collect(),
        header: lay.header,
    }
}

// ---- The renderer ----------------------------------------------------------------------

/// A terminal sized to the renderer's grid, every row full of text — so a backdrop that
/// fails to cover the grid leaves glyph ink behind.
fn full_snapshot(r: &Renderer) -> GridSnapshot {
    let cfg = Config::default();
    let (cols, rows) = r.grid_size();
    let mut t = Terminal::new(
        cols,
        rows,
        Color(cfg.theme.foreground),
        Color(cfg.theme.background),
    );
    let line = "M".repeat(usize::from(cols.saturating_sub(1)));
    let mut feed = String::new();
    for row in 0..rows {
        feed.push_str(&line);
        if row + 1 < rows {
            feed.push_str("\r\n");
        }
    }
    t.advance(feed.as_bytes());
    t.snapshot()
}

/// Forwards to a shared `FlowPanel`, so the test can read its counters after handing the
/// layer to the renderer (which takes it by `Box`).
struct Shared(Rc<RefCell<FlowPanel>>);

impl DrawLayer for Shared {
    fn z_order(&self) -> i32 {
        self.0.borrow().z_order()
    }
    fn record(&mut self, ctx: &mut FrameCtx<'_>) {
        self.0.borrow_mut().record(ctx);
    }
}

fn with_shared(cfg: &Config, handle: &FlowHandle) -> Option<(Renderer, Rc<RefCell<FlowPanel>>)> {
    let mut r = headless(cfg)?;
    let panel = Rc::new(RefCell::new(FlowPanel::new(handle.clone())));
    r.add_layer(Box::new(Shared(panel.clone())));
    Some((r, panel))
}

// ---- Pixels ----------------------------------------------------------------------------

/// Linear → 8-bit sRGB, as the `*UnormSrgb` target stores it.
fn srgb8(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    let s = if c <= 0.003_130_8 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    s * 255.0
}

fn srgb_of(packed: u32) -> [f32; 3] {
    let c = linear(packed, 1.0);
    [srgb8(c[0]), srgb8(c[1]), srgb8(c[2])]
}

fn near(px: [u8; 4], want: [f32; 3], tol: f32) -> bool {
    (0..3).all(|k| (f32::from(px[k]) - want[k]).abs() <= tol)
}

fn for_px(frame: &CapturedFrame, rect: [f32; 4], mut f: impl FnMut([u8; 4], u32, u32)) {
    let [x, y, w, h] = rect;
    let x0 = x.max(0.0).floor() as u32;
    let y0 = y.max(0.0).floor() as u32;
    let x1 = ((x + w).max(0.0).ceil() as u32).min(frame.width());
    let y1 = ((y + h).max(0.0).ceil() as u32).min(frame.height());
    for py in y0..y1 {
        for px in x0..x1 {
            f(frame.pixel(px, py), px, py);
        }
    }
}

fn count_near(frame: &CapturedFrame, rect: [f32; 4], want: [f32; 3], tol: f32) -> usize {
    let mut n = 0;
    for_px(frame, rect, |px, _, _| {
        n += usize::from(near(px, want, tol))
    });
    n
}

fn ink_in(frame: &CapturedFrame, rect: [f32; 4], bg: [u8; 4]) -> usize {
    let mut n = 0;
    for_px(frame, rect, |px, _, _| n += usize::from(is_ink(px, bg)));
    n
}

/// The most common pixel inside `rect` — that region's own background.
fn modal_in(frame: &CapturedFrame, rect: [f32; 4]) -> [u8; 4] {
    let mut counts = std::collections::HashMap::<[u8; 4], u32>::new();
    for_px(frame, rect, |px, _, _| *counts.entry(px).or_default() += 1);
    counts
        .into_iter()
        .max_by_key(|&(_, n)| n)
        .map(|(px, _)| px)
        .expect("the region holds at least one pixel")
}

fn inflate(r: [f32; 4], by: f32) -> [f32; 4] {
    [r[0] - by, r[1] - by, r[2] + 2.0 * by, r[3] + 2.0 * by]
}

fn contains(r: [f32; 4], x: f32, y: f32) -> bool {
    x >= r[0] && x < r[0] + r[2] && y >= r[1] && y < r[1] + r[3]
}

/// A card's title zone: its top-left quarter, clear of the agent icon at the top right
/// and of the status / branch row at the bottom.
fn title_zone(card: [f32; 4]) -> [f32; 4] {
    [card[0], card[1], card[2] * 0.5, card[3] * 0.5]
}

/// A card's status zone: its bottom-left half-width, where `build_card` draws the status
/// icon and label.
fn status_zone(card: [f32; 4]) -> [f32; 4] {
    [
        card[0],
        card[1] + card[3] * 0.5,
        card[2] * 0.6,
        card[3] * 0.5,
    ]
}

// ---- R12.1 -----------------------------------------------------------------------------

#[test]
fn r12_1_cards_edges_and_status_colours_reach_the_screen_over_an_opaque_backdrop() {
    let cfg = Config::default();
    let theme = SidebarTheme::DEFAULT;
    let Some(mut r) = headless(&cfg) else {
        return;
    };
    r.set_monitor_open(true);
    assert!(
        r.right_inset() > 0.0,
        "set_monitor_open(true) reserved no band"
    );

    // The ai-monitor, open beneath the graph, with something to draw.
    let (monitor, monitor_handle) = MonitorPanel::new();
    monitor_handle.set(MonitorModel {
        visible: true,
        title: "AI MONITOR".into(),
        footer: "updated 12:00:00".into(),
        ..MonitorModel::default()
    });
    r.add_layer(Box::new(monitor));

    let handle = FlowHandle::default();
    let graph = r12_1_graph();
    handle.set(graph.clone());
    r.add_layer(Box::new(FlowPanel::new(handle.clone())));

    let snap = full_snapshot(&r);
    let (w, h) = (FRAME_W as f32, FRAME_H as f32);
    let grid = [
        r.left_inset(),
        r.top_inset(),
        w - r.left_inset() - r.right_inset(),
        h - r.top_inset(),
    ];
    let band = [
        w - r.right_inset(),
        r.top_inset(),
        r.right_inset(),
        h - r.top_inset(),
    ];

    // Control: with the graph not yet visible, the grid holds text and the monitor band
    // holds ink — so the "nothing shows through" assertion below is not vacuous.
    let before = r.capture(&snap).expect("read the control frame back");
    let grid_ink = ink_in(&before, grid, before.modal_pixel());
    assert!(
        grid_ink > 1000,
        "control frame: the full grid carries only {grid_ink} ink pixels"
    );
    let band_ink = ink_in(&before, band, modal_in(&before, band));
    assert!(
        band_ink > 20,
        "control frame: the open ai-monitor band carries only {band_ink} ink pixels"
    );

    handle.set_visible(true);
    let frame = r.capture(&snap).expect("read the rendered frame back");
    let geo = geometry(&r, &cfg, &graph);
    let scale = r.scale_factor() as f32;

    // The backdrop is the sidebar's panel colour, and it is what most of the frame is.
    let backdrop = frame.modal_pixel();
    let want_bg = srgb_of(theme.panel_bg);
    assert!(
        near(backdrop, want_bg, BACKDROP_TOL),
        "the frame's dominant colour {backdrop:?} is not the backdrop panel_bg {want_bg:?}"
    );

    // Every node is laid out as a card (three columns fit 1200px), and every card rect
    // carries glyph ink: title-coloured pixels in its title zone.
    assert_eq!(
        geo.cards.len(),
        graph.nodes.len(),
        "every node of R12.1's graph is a card"
    );
    let title_fg = srgb_of(theme.card_title_fg);
    for &(i, rect) in &geo.cards {
        let ink = count_near(&frame, title_zone(rect), title_fg, COLOR_TOL);
        assert!(
            ink >= 10,
            "card {i} ({:?} {:?}) at {rect:?}: its title zone carries {ink} pixels of \
             the title colour — its text did not reach the screen there",
            graph.nodes[i].kind,
            graph.nodes[i].title
        );
    }

    // Each session card's status zone contains its sidebar status colour.
    for (i, status, packed) in [
        (WORKING, CardStatus::Working, theme.status_working_fg),
        (
            WAITING,
            CardStatus::WaitingForInput,
            theme.status_waiting_fg,
        ),
        (READY, CardStatus::Ready, theme.status_ready_fg),
    ] {
        let want = srgb_of(packed);
        let zone = status_zone(geo.card(i));
        let hits = count_near(&frame, zone, want, COLOR_TOL);
        assert!(
            hits >= 4,
            "{status:?} session (node {i}): its status zone {zone:?} holds {hits} pixels \
             of its status colour {want:?}"
        );
    }
    // The three columns, left to right: project, session, subagent.
    let col_x = |i: usize| geo.card(i)[0];
    assert!(
        col_x(PROJECT) < col_x(WORKING) && col_x(WORKING) < col_x(SUBAGENT),
        "columns out of order: project x {}, session x {}, subagent x {}",
        col_x(PROJECT),
        col_x(WORKING),
        col_x(SUBAGENT)
    );

    // One edge per parent→child pair, and each edge's sampled midpoint is drawn.
    assert_eq!(
        geo.edges.len(),
        4,
        "project→3 sessions plus Working→its subagent"
    );
    for (e, chain) in geo.edges.iter().enumerate() {
        assert!(!chain.is_empty(), "edge {e} has no capsules");
        let mid = chain[chain.len() / 2].0;
        let (mx, my) = (mid[0], mid[1]);
        let mut differs = 0;
        for_px(&frame, [mx - 1.5, my - 1.5, 3.0, 3.0], |px, _, _| {
            differs += usize::from(px != backdrop);
        });
        assert!(
            differs > 0,
            "edge {e}: every pixel around its sampled midpoint ({mx}, {my}) is the \
             backdrop {backdrop:?} — the edge did not reach the screen"
        );
    }

    // Nothing of the terminal grid or the ai-monitor band shows through: outside the
    // cards (plus the selected glow and antialiasing), the edges (capsules and the end
    // ring) and the header, every pixel there is the backdrop.
    let card_pad = (CardMetrics::DEFAULT.glow + 2.0) * scale;
    let cards: Vec<[f32; 4]> = geo.cards.iter().map(|c| inflate(c.1, card_pad)).collect();
    let mut edge_boxes: Vec<[f32; 4]> = Vec::new();
    for chain in &geo.edges {
        for &(_, bbox) in chain {
            edge_boxes.push(inflate(bbox, 2.0 * scale));
        }
        if let Some(&(seg, _)) = chain.last() {
            // The end ring at the child: radius 3px plus its stroke and antialiasing.
            let r = 7.0 * scale;
            edge_boxes.push([seg[2] - r, seg[3] - r, 2.0 * r, 2.0 * r]);
        }
    }
    let header = inflate(geo.header, 2.0 * scale);
    let excused = |x: f32, y: f32| {
        contains(header, x, y)
            || cards.iter().any(|&c| contains(c, x, y))
            || edge_boxes.iter().any(|&b| contains(b, x, y))
    };
    let mut leaks = Vec::new();
    let mut checked = 0usize;
    for region in [grid, band] {
        for_px(&frame, region, |px, x, y| {
            let (cx, cy) = (x as f32 + 0.5, y as f32 + 0.5);
            if excused(cx, cy) {
                return;
            }
            checked += 1;
            if px != backdrop {
                leaks.push((x, y, px));
            }
        });
    }
    assert!(
        checked > 10_000,
        "only {checked} pixels lay outside the graph's cards, edges and header"
    );
    assert!(
        leaks.is_empty(),
        "{} pixels of the grid / ai-monitor band differ from the backdrop {backdrop:?} \
         outside the graph's cards, edges and header; first: {:?}",
        leaks.len(),
        &leaks[..leaks.len().min(8)]
    );
}

// ---- R12.2 -----------------------------------------------------------------------------

#[test]
fn r12_2_ten_unchanged_frames_neither_relayout_nor_recompose() {
    let cfg = Config::default();
    let handle = FlowHandle::default();
    handle.set(r12_1_graph());
    handle.set_visible(true);
    let Some((mut r, panel)) = with_shared(&cfg, &handle) else {
        return;
    };
    let snap = full_snapshot(&r);
    r.capture(&snap).expect("frame 1");
    assert_eq!(panel.borrow().layout_count(), 1, "the first frame lays out");
    assert_eq!(
        panel.borrow().compose_count(),
        1,
        "the first frame composes"
    );
    for n in 0..10 {
        r.capture(&snap).expect("a repeat frame");
        assert_eq!(
            panel.borrow().layout_count(),
            1,
            "repeat frame {n}: same graph, scroll and hover re-laid the graph out"
        );
        assert_eq!(
            panel.borrow().compose_count(),
            1,
            "repeat frame {n}: same graph, scroll and hover re-composed"
        );
    }
}

// ---- R12.3 -----------------------------------------------------------------------------

#[test]
fn r12_3_status_change_relayouts_scroll_and_hover_only_recompose() {
    let cfg = Config::default();
    let handle = FlowHandle::default();
    // Twenty slots: far taller than 800px, so scroll offset 2 is not clamped to 0.
    let graph = tall_graph(20, CardStatus::Working);
    handle.set(graph.clone());
    handle.set_visible(true);
    let Some((mut r, panel)) = with_shared(&cfg, &handle) else {
        return;
    };
    let snap = full_snapshot(&r);
    r.capture(&snap).expect("frame 1");
    assert_eq!(panel.borrow().layout_count(), 1, "the first frame lays out");
    let c0 = panel.borrow().compose_count();

    // One session's status changes: re-laid out on the very next frame.
    handle.set(tall_graph(20, CardStatus::WaitingForInput));
    r.capture(&snap).expect("frame after the status change");
    assert_eq!(
        panel.borrow().layout_count(),
        2,
        "a status change must re-lay out on the next frame"
    );
    let c1 = panel.borrow().compose_count();
    assert!(
        c1 > c0,
        "a status change must re-compose (compose count {c0} -> {c1})"
    );

    // Then the scroll offset changes: re-composed, not re-laid out.
    handle.set_scroll(2);
    r.capture(&snap).expect("frame after the scroll");
    assert_eq!(
        panel.borrow().layout_count(),
        2,
        "a scroll must not re-lay out"
    );
    let c2 = panel.borrow().compose_count();
    assert!(
        c2 > c1,
        "a scroll must re-compose (compose count {c1} -> {c2})"
    );

    // Then the hovered node changes: the pointer moves onto a card (slot 2's position,
    // below the header, which holds a session card at any scroll offset).
    let geo = geometry(&r, &cfg, &graph);
    let target = geo.card(3);
    let point = [target[0] + target[2] * 0.5, target[1] + target[3] * 0.5];
    handle.hover(Some(point));
    r.capture(&snap).expect("frame after the hover");
    assert_eq!(
        panel.borrow().layout_count(),
        2,
        "a hover change must not re-lay out"
    );
    let c3 = panel.borrow().compose_count();
    assert!(
        c3 > c2,
        "a hover change must re-compose (compose count {c2} -> {c3})"
    );
}

// ---- R12.4 -----------------------------------------------------------------------------

#[test]
fn r12_4_a_never_visible_panel_builds_nothing() {
    let cfg = Config::default();
    let handle = FlowHandle::default();
    handle.set(r12_1_graph());
    let Some((mut r, panel)) = with_shared(&cfg, &handle) else {
        return;
    };
    let snap = full_snapshot(&r);
    for n in 0..10 {
        r.capture(&snap).expect("a frame");
        let p = panel.borrow();
        assert_eq!(
            p.atlas_builds(),
            0,
            "frame {n}: a never-visible graph built its card atlases"
        );
        assert_eq!(
            p.layout_count(),
            0,
            "frame {n}: a never-visible graph laid out"
        );
        assert_eq!(
            p.submit_count(),
            0,
            "frame {n}: a never-visible graph submitted a draw"
        );
    }
}
