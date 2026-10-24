//! Pure, GPU-free layout for the session graph (session-flow-graph D1, D9, D12, D13).
//!
//! `(FlowGraph, viewport, scale, card metrics, sidebar width) → slots, card rects, fold
//! chips, `+N` chips and sampled edges` ([`layout_graph`]), then `(layout, scroll, hover)
//! → shapes + card text runs + what was drawn` ([`compose`]). Nothing here holds state,
//! touches a device or reads a clock, so every decision the graph makes — which column a
//! node lands in, which slot, how wide a card is, whether an edge is drawn — is testable
//! without a GPU. The layer (`graph_panel`) only resolves the runs' glyphs and submits.
//!
//! **What this module is the contract for.** [`FlowGraph`] is the renderer's *input*,
//! like `SidebarRow`: plain data, tab ids as raw `u64`, so the renderer never names an app
//! type (dependency-direction, D15). Building it — grouping by repository, folding, which
//! session is selected — is the app's `build_graph`; this file draws what it is handed.
//!
//! **Three rules worth knowing before editing:**
//!
//! * **Slots, not pixels (D12/D13).** Every node occupies one fixed-height slot (a card
//!   plus a row gap), and the scroll offset is a slot index. The shape path has no scissor
//!   rect, so a half-drawn card has no representation; whole slots make that a non-issue.
//! * **Top-aligned subtrees.** A session takes `max(1, its column-2 nodes)` slots and a
//!   project the sum of its sessions', and each card sits in its subtree's *first* slot,
//!   so a node's position never depends on rows below it.
//! * **An edge is drawn only with both ends (D13).** Culling a card culls every edge that
//!   touches it, so no line runs off the top of the viewport without an end.

use crate::graph_panel::FlowHit;
use crate::sidebar_card::{build_card, card_state, truncate_cols_right, CardInput, CardStatus};
use crate::sidebar_card::{CardState, TextRun};
use crate::sidebar_shapes::{Fill, SidebarShape};
use crate::sidebar_text::{CardFace, CardMeasure};
use crate::sidebar_theme::{linear, AgentSkin, CardMetrics, SidebarTheme, CARD_META_ALPHA};
use crate::tab_bar::char_cols;

/// Horizontal gap between two columns, logical px (D12).
pub const COL_GAP: f32 = 48.0;
/// The margin left and right of the columns, logical px (D12).
pub const MARGIN: f32 = 24.0;
/// The fixed header band above the slots, logical px. It does not scroll (D12).
pub const HEADER_H: f32 = 40.0;
/// The narrowest a card is ever drawn, logical px. Below it a column is dropped (D12).
pub const MIN_CARD_W: f32 = 180.0;
/// Capsules per edge: the sampled cubic is a chain of this many strokes (D9).
pub const EDGE_SEGMENTS: usize = 16;
/// An edge's stroke width, logical px (D9).
pub const EDGE_STROKE: f32 = 1.5;
/// An edge's opacity over the backdrop (D9). Not the card border's `CARD_BORDER_ALPHA`
/// (0.10): that left an edge ~6 sRGB counts above `panel_bg`, barely visible, and the
/// user raised it (2026-10-21). A muted edge keeps the same proportion, × [`MUTED_ALPHA`].
pub const EDGE_ALPHA: f32 = 0.45;
/// The radius of the ring at an edge's child end, logical px (D9).
pub const EDGE_DOT_R: f32 = 3.0;
/// The `+N` chip's height, logical px (D12).
pub const CHIP_H: f32 = 22.0;
/// Horizontal padding inside a chip, logical px.
pub const CHIP_PAD_X: f32 = 8.0;
/// The `N finished` fold chip's height, logical px (D8). Shorter than a card: it is a
/// control, not a node with content, and it sits centred in its slot.
pub const FOLD_H: f32 = 32.0;
/// The opacity a finished subagent's card and edge are drawn at — the muted look (D8).
pub const MUTED_ALPHA: f32 = 0.45;

/// Float slack for "does this fit" comparisons — sums of scaled constants miss an exactly
/// sized viewport by an ulp otherwise.
const FIT_EPS: f32 = 1e-3;

/// What a node is in the graph (D1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum NodeKind {
    /// One git repository's card in column 0 (D2). Never counted, never pressable.
    Project,
    /// One agent session (any detected kind) in column 1 — the unit the header counts (D3).
    #[default]
    Session,
    /// One Agent-tool subagent in column 2 (D8).
    Subagent,
    /// A session's `N finished` control in column 2 (D8). Toggles; never switches tabs.
    Fold,
}

/// What a press on a node asks for. Raw ids only (D15): `Node` carries the tab id, `Fold`
/// the session's identity `(tab id, pane pid)` (D8).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum FlowTarget {
    /// Switch to this tab (D7).
    Node(u64),
    /// Toggle this session's finished subagents (D8).
    Fold(u64, u32),
    /// Inert — the project card.
    #[default]
    None,
}

/// One node of the graph: plain data, the card's strings already resolved by the app.
#[derive(Clone, Debug, PartialEq)]
pub struct FlowNode {
    pub kind: NodeKind,
    /// The node this one hangs from: a session's project, a subagent's or fold's session.
    /// `None` for a project and for a session outside any repository (D1).
    pub parent: Option<usize>,
    /// The card's line 1; a fold's label (`▸ 4 finished`).
    pub title: String,
    pub folder: String,
    pub branch: Option<String>,
    pub skin: AgentSkin,
    pub status: CardStatus,
    /// A finished subagent: drawn at [`MUTED_ALPHA`], its edge too (D8).
    pub muted: bool,
    /// The active tab's session: its card is drawn `Selected` (D3).
    pub selected: bool,
    pub target: FlowTarget,
    /// A session's subagent count — running plus finished — shown as a `+N` chip when the
    /// subagent column does not fit (D12). The app fills it; 0 draws no chip.
    pub extra: u32,
}

impl Default for FlowNode {
    fn default() -> FlowNode {
        FlowNode {
            kind: NodeKind::Session,
            parent: None,
            title: String::new(),
            folder: String::new(),
            branch: None,
            skin: AgentSkin::Generic,
            status: CardStatus::Hidden,
            muted: false,
            selected: false,
            target: FlowTarget::None,
            extra: 0,
        }
    }
}

/// The graph's whole input (D15), nodes in the builder's pre-order.
///
/// Derives `PartialEq` because it **is** the layout's cache key (D11). The scroll offset,
/// the hover and the visibility are deliberately not fields: none of them may re-lay-out.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FlowGraph {
    pub nodes: Vec<FlowNode>,
    /// The worker has not finished its first pass with the graph wanted (D16): the header
    /// reads `resolving…` and the builder sends no nodes.
    pub resolving: bool,
}

impl FlowGraph {
    /// The header: `{w} working · {q} waiting / {n}` (old D3), or `resolving…` (D16).
    ///
    /// `n` counts [`NodeKind::Session`] nodes only — projects and subagents never — `w`
    /// those `Working`, `q` those `WaitingForInput`. Zero sessions is `0 working · 0
    /// waiting / 0`, never an empty string.
    pub fn header(&self) -> String {
        if self.resolving {
            return "resolving…".to_string();
        }
        let (mut w, mut q, mut n) = (0usize, 0usize, 0usize);
        for node in self.nodes.iter().filter(|n| n.kind == NodeKind::Session) {
            n += 1;
            match node.status {
                CardStatus::Working => w += 1,
                CardStatus::WaitingForInput => q += 1,
                _ => {}
            }
        }
        format!("{w} working · {q} waiting / {n}")
    }

    /// The line under the header when there is nothing to draw (amendment 2026-10-23):
    /// resolved, and no [`NodeKind::Session`] node. `None` otherwise — while resolving the
    /// header already says why the graph is empty, and with a session there is a card.
    pub fn empty_hint(&self) -> Option<&'static str> {
        let empty = !self.resolving && !self.nodes.iter().any(|n| n.kind == NodeKind::Session);
        empty.then_some(EMPTY_HINT)
    }
}

/// [`FlowGraph::empty_hint`]'s text. The graph counts this window's tabs only, which is
/// what a blank graph most often means: the agents are running in another window.
pub const EMPTY_HINT: &str =
    "No agent sessions in this window. Start claude, codex, agy, gemini or aider in a tab.";

/// How many columns fit (D12).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutMode {
    /// Project, session and subagent columns.
    Full,
    /// No subagent column: each session takes one slot and shows a `+N` chip.
    TwoColumns,
    /// Sessions alone, in group order.
    OneColumn,
    /// Not even one card fits: the header only.
    HeaderOnly,
}

/// A laid-out card: a project, session or subagent node. `rect` is viewport-absolute at
/// scroll 0, physical px `[x, y, w, h]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlacedCard {
    /// Index into [`FlowGraph::nodes`].
    pub node: usize,
    pub slot: usize,
    pub rect: [f32; 4],
}

/// A laid-out `N finished` fold chip (D8) — pressable, unlike a `+N` chip.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlacedFold {
    pub node: usize,
    pub slot: usize,
    pub rect: [f32; 4],
}

/// A `+N` chip on a session card in the narrow modes (D12). Inert.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlusChip {
    /// The session node it belongs to.
    pub node: usize,
    pub n: u32,
    pub rect: [f32; 4],
}

/// One parent→child connector (D9), at scroll 0.
#[derive(Clone, Debug, PartialEq)]
pub struct GraphEdge {
    pub parent: usize,
    pub child: usize,
    /// [`EDGE_SEGMENTS`] capsules along the sampled cubic, end to end.
    pub segments: Vec<SidebarShape>,
    /// The dot at the child end.
    pub ring: SidebarShape,
}

/// The layout's output — stage one of the panel's two-stage cache (D11).
#[derive(Clone, Debug, PartialEq)]
pub struct GraphLayout {
    /// The graph it was laid out from: [`compose`] reads the card strings from it, and the
    /// panel compares it to decide whether to lay out again.
    pub graph: FlowGraph,
    pub viewport: [f32; 2],
    pub scale: f64,
    pub mode: LayoutMode,
    /// Physical px.
    pub card_w: f32,
    pub card_h: f32,
    /// A card plus the row gap: the scroll unit.
    pub slot_h: f32,
    /// Where slot 0 starts — just below the header.
    pub list_top: f32,
    /// How many slots the graph takes.
    pub total_slots: usize,
    /// How many whole slots fit below the header.
    pub visible_slots: usize,
    /// The header band, `[x, y, w, h]`.
    pub header: [f32; 4],
    pub header_text: String,
    pub cards: Vec<PlacedCard>,
    pub folds: Vec<PlacedFold>,
    pub plus_chips: Vec<PlusChip>,
    pub edges: Vec<GraphEdge>,
    /// How card text measures — the panel's card atlases' answer (D14).
    pub text: CardMeasure,
    pub ellipsis: char,
}

impl GraphLayout {
    /// The largest scroll offset that still shows the last slot (D13).
    pub fn max_scroll(&self) -> usize {
        self.total_slots.saturating_sub(self.visible_slots)
    }

    /// Is `slot` on screen at `scroll`?
    fn shown(&self, slot: usize, scroll: usize) -> bool {
        slot >= scroll && slot - scroll < self.visible_slots
    }

    /// How far everything moves up at `scroll`, px.
    fn shift(&self, scroll: usize) -> f32 {
        scroll as f32 * self.slot_h
    }

    /// The session or subagent card under `point` at `scroll` — what the pointer hovers
    /// (D3). Projects never hover: they do nothing on a click (D7).
    pub fn node_at(&self, point: [f32; 2], scroll: usize) -> Option<usize> {
        let dy = self.shift(scroll);
        self.cards
            .iter()
            .filter(|c| self.shown(c.slot, scroll))
            .filter(|c| self.pressable_card(c.node))
            .find(|c| contains(moved(c.rect, dy), point))
            .map(|c| c.node)
    }

    /// What a press at `point` hits at `scroll`: a session or subagent card, or a fold
    /// chip (D7). The project card, edges, `+N` chips and empty space hit nothing.
    pub fn hit_at(&self, point: [f32; 2], scroll: usize) -> Option<FlowHit> {
        self.hits(scroll)
            .into_iter()
            .find(|(_, r)| contains(*r, point))
            .map(|(h, _)| h)
    }

    /// [`hit_at`](Self::hit_at) at scroll 0.
    pub fn hit(&self, point: [f32; 2]) -> Option<FlowHit> {
        self.hit_at(point, 0)
    }

    /// Every pressable rect on screen at `scroll`, as drawn.
    fn hits(&self, scroll: usize) -> Vec<(FlowHit, [f32; 4])> {
        let dy = self.shift(scroll);
        let node = |i: usize| &self.graph.nodes[i];
        let cards = self
            .cards
            .iter()
            .filter(|c| self.shown(c.slot, scroll) && self.pressable_card(c.node))
            .filter_map(|c| FlowHit::of(node(c.node).target).map(|h| (h, moved(c.rect, dy))));
        let folds = self
            .folds
            .iter()
            .filter(|f| self.shown(f.slot, scroll))
            .filter_map(|f| FlowHit::of(node(f.node).target).map(|h| (h, moved(f.rect, dy))));
        cards.chain(folds).collect()
    }

    fn pressable_card(&self, i: usize) -> bool {
        matches!(
            self.graph.nodes[i].kind,
            NodeKind::Session | NodeKind::Subagent
        )
    }
}

/// `rect` moved `dy` px up.
fn moved(rect: [f32; 4], dy: f32) -> [f32; 4] {
    [rect[0], rect[1] - dy, rect[2], rect[3]]
}

/// Half-open containment, so two adjacent rects never both claim their shared edge.
fn contains(r: [f32; 4], [x, y]: [f32; 2]) -> bool {
    x >= r[0] && x < r[0] + r[2] && y >= r[1] && y < r[1] + r[3]
}

/// Display width of `s` in columns.
fn cols(s: &str) -> usize {
    s.chars().map(char_cols).sum()
}

/// The mode and card width for a viewport `vw` px wide (D12): the most columns whose
/// cards fit at [`MIN_CARD_W`], each card the sidebar card's width shrunk to fit.
fn choose_mode(vw: f32, s: f32, sidebar_card_w: f32) -> (LayoutMode, f32) {
    for (mode, n) in [
        (LayoutMode::Full, 3.0),
        (LayoutMode::TwoColumns, 2.0),
        (LayoutMode::OneColumn, 1.0),
    ] {
        let fit = (vw - 2.0 * MARGIN * s - (n - 1.0) * COL_GAP * s) / n;
        if fit + FIT_EPS >= MIN_CARD_W * s {
            return (mode, sidebar_card_w.min(fit).max(MIN_CARD_W * s));
        }
    }
    (LayoutMode::HeaderOnly, 0.0)
}

/// The x of each graph column in `mode` (`None` = not drawn).
fn column_xs(mode: LayoutMode, s: f32, card_w: f32) -> [Option<f32>; 3] {
    let x = |k: f32| Some(MARGIN * s + k * (card_w + COL_GAP * s));
    match mode {
        LayoutMode::Full => [x(0.0), x(1.0), x(2.0)],
        LayoutMode::TwoColumns => [x(0.0), x(1.0), None],
        LayoutMode::OneColumn => [None, x(0.0), None],
        LayoutMode::HeaderOnly => [None, None, None],
    }
}

/// [`layout_graph_with`] measuring card text by the card faces' own sizes and drawing
/// `…` — the answer once the panel's card atlases are built.
pub fn layout_graph(
    graph: &FlowGraph,
    viewport: [f32; 2],
    scale: f64,
    metrics: CardMetrics,
    sidebar_w: f32,
) -> GraphLayout {
    layout_graph_with(
        graph,
        viewport,
        scale,
        metrics,
        sidebar_w,
        CardMeasure::of_faces(scale),
        '…',
    )
}

/// Lay the graph out (D1, D9, D12). `viewport` is `[w, h]` physical px; `metrics` and
/// `sidebar_w` (`[sidebar].width_px`) are **logical** and scaled here, as are this
/// module's constants. `text` and `ellipsis` are what the card text will be drawn with
/// (the `+N` chips are sized by them).
pub fn layout_graph_with(
    graph: &FlowGraph,
    viewport: [f32; 2],
    scale: f64,
    metrics: CardMetrics,
    sidebar_w: f32,
    text: CardMeasure,
    ellipsis: char,
) -> GraphLayout {
    let s = scale as f32;
    let m = metrics.scaled(scale);
    let [vw, vh] = viewport;
    let sidebar_card_w = ((sidebar_w - 2.0 * metrics.inset_x) * s).max(0.0);
    let (mode, card_w) = choose_mode(vw, s, sidebar_card_w);
    let xs = column_xs(mode, s, card_w);
    let card_h = m.height;
    let slot_h = card_h + m.gap;
    let list_top = HEADER_H * s;
    let slot_y = |slot: usize| list_top + slot as f32 * slot_h;

    let nodes = &graph.nodes;
    let n = nodes.len();
    let kind_at = |i: usize| nodes.get(i).map(|node| node.kind);
    // A session's project, when its parent link names one.
    let project_of = |i: usize| {
        nodes[i]
            .parent
            .filter(|&p| kind_at(p) == Some(NodeKind::Project))
    };
    let is_column2 = |i: usize| matches!(nodes[i].kind, NodeKind::Subagent | NodeKind::Fold);
    // Column-2 nodes per session, in node order.
    let mut kids: Vec<Vec<usize>> = vec![Vec::new(); n];
    for i in (0..n).filter(|&i| is_column2(i)) {
        if let Some(p) = nodes[i]
            .parent
            .filter(|&p| kind_at(p) == Some(NodeKind::Session))
        {
            kids[p].push(i);
        }
    }
    let mut sessions: Vec<Vec<usize>> = vec![Vec::new(); n];
    for i in (0..n).filter(|&i| nodes[i].kind == NodeKind::Session) {
        if let Some(p) = project_of(i) {
            sessions[p].push(i);
        }
    }

    let mut out = GraphLayout {
        graph: graph.clone(),
        viewport,
        scale,
        mode,
        card_w,
        card_h,
        slot_h,
        list_top,
        total_slots: 0,
        visible_slots: 0,
        header: [0.0, 0.0, vw.max(0.0), list_top],
        header_text: graph.header(),
        cards: Vec::new(),
        folds: Vec::new(),
        plus_chips: Vec::new(),
        edges: Vec::new(),
        text,
        ellipsis,
    };
    let mut rects: Vec<Option<[f32; 4]>> = vec![None; n];

    if mode != LayoutMode::HeaderOnly {
        let mut cursor = 0usize;
        let place_session = |i: usize,
                             cursor: &mut usize,
                             out: &mut GraphLayout,
                             rects: &mut Vec<Option<[f32; 4]>>| {
            let Some(x1) = xs[1] else { return };
            let rect = [x1, slot_y(*cursor), card_w, card_h];
            out.cards.push(PlacedCard {
                node: i,
                slot: *cursor,
                rect,
            });
            rects[i] = Some(rect);
            match xs[2] {
                Some(x2) => {
                    for (j, &k) in kids[i].iter().enumerate() {
                        let slot = *cursor + j;
                        if nodes[k].kind == NodeKind::Fold {
                            let h = FOLD_H * s;
                            let r = [x2, slot_y(slot) + (card_h - h) * 0.5, card_w, h];
                            out.folds.push(PlacedFold {
                                node: k,
                                slot,
                                rect: r,
                            });
                            rects[k] = Some(r);
                        } else {
                            let r = [x2, slot_y(slot), card_w, card_h];
                            out.cards.push(PlacedCard {
                                node: k,
                                slot,
                                rect: r,
                            });
                            rects[k] = Some(r);
                        }
                    }
                    *cursor += kids[i].len().max(1);
                }
                None => {
                    let extra = nodes[i].extra;
                    if extra > 0 {
                        let label = format!("+{extra}");
                        let h = CHIP_H * s;
                        let w = (cols(&label) as f32 * text.meta_col + 2.0 * CHIP_PAD_X * s).max(h);
                        out.plus_chips.push(PlusChip {
                            node: i,
                            n: extra,
                            rect: [
                                rect[0] + rect[2] - w * 0.5,
                                rect[1] + (card_h - h) * 0.5,
                                w,
                                h,
                            ],
                        });
                    }
                    *cursor += 1;
                }
            }
        };
        for i in 0..n {
            match nodes[i].kind {
                NodeKind::Project => {
                    let start = cursor;
                    for &sess in &sessions[i] {
                        place_session(sess, &mut cursor, &mut out, &mut rects);
                    }
                    if let Some(x0) = xs[0] {
                        // An empty group still takes its card's slot, so nothing overlaps it.
                        cursor = cursor.max(start + 1);
                        let rect = [x0, slot_y(start), card_w, card_h];
                        out.cards.push(PlacedCard {
                            node: i,
                            slot: start,
                            rect,
                        });
                        rects[i] = Some(rect);
                    }
                }
                NodeKind::Session if project_of(i).is_none() => {
                    place_session(i, &mut cursor, &mut out, &mut rects);
                }
                _ => {}
            }
        }
        out.total_slots = cursor;
        out.cards.sort_by_key(|c| c.node);
    }

    // Whole slots below the header; the last needs its card, not its trailing gap.
    let room = vh - list_top + m.gap;
    // A non-positive room floors to 0 by the saturating cast; a zero slot (scale 0) is
    // not finite and fits nothing.
    let fit = (room + FIT_EPS) / slot_h;
    out.visible_slots = if fit.is_finite() {
        fit.floor() as usize
    } else {
        0
    };

    for (child, node) in nodes.iter().enumerate() {
        let Some(parent) = node.parent else { continue };
        let (Some(Some(p)), Some(c)) = (rects.get(parent), rects[child]) else {
            continue;
        };
        let from = [p[0] + p[2], p[1] + p[3] * 0.5];
        let to = [c[0], c[1] + c[3] * 0.5];
        if let Some((segments, ring)) = edge_shapes(from, to, s, node.muted) {
            out.edges.push(GraphEdge {
                parent,
                child,
                segments,
                ring,
            });
        }
    }
    out
}

/// The point at `t` on the cubic Bézier `p0 → p3` with control points `p1`, `p2`.
fn bezier(p: [[f32; 2]; 4], t: f32) -> [f32; 2] {
    let u = 1.0 - t;
    let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
    [
        a * p[0][0] + b * p[1][0] + c * p[2][0] + d * p[3][0],
        a * p[0][1] + b * p[1][1] + c * p[2][1] + d * p[3][1],
    ]
}

/// The colour an edge (or a muted edge) is stroked in (D9).
pub fn edge_color(muted: bool) -> [f32; 4] {
    let a = if muted {
        EDGE_ALPHA * MUTED_ALPHA
    } else {
        EDGE_ALPHA
    };
    linear(SidebarTheme::DEFAULT.card_border_active, a)
}

/// D9's S-curve from `from` (a parent's right-edge midpoint) to `to` (a child's left-edge
/// midpoint): control points half a column gap in from each end, horizontally, sampled
/// into [`EDGE_SEGMENTS`] capsules plus a ring at `to`. `None` on non-finite geometry.
fn edge_shapes(
    from: [f32; 2],
    to: [f32; 2],
    s: f32,
    muted: bool,
) -> Option<(Vec<SidebarShape>, SidebarShape)> {
    let off = COL_GAP * s * 0.5;
    let ctrl = [from, [from[0] + off, from[1]], [to[0] - off, to[1]], to];
    let fill = Fill::solid(edge_color(muted));
    let stroke = EDGE_STROKE * s;
    let at = |i: usize| {
        if i == EDGE_SEGMENTS {
            to // exact, so the chain ends on the child whatever the float error
        } else {
            bezier(ctrl, i as f32 / EDGE_SEGMENTS as f32)
        }
    };
    let segments = (0..EDGE_SEGMENTS)
        .map(|i| {
            let (a, b) = (at(i), at(i + 1));
            SidebarShape::capsule(a[0], a[1], b[0], b[1], stroke, fill)
        })
        .collect::<Option<Vec<_>>>()?;
    let ring = SidebarShape::ring(to[0], to[1], EDGE_DOT_R * s, stroke, fill)?;
    Some((segments, ring))
}

/// What one composed frame draws — stage two of the panel's cache (D11, D13).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GraphFrame {
    /// In submit order: backdrop, edges, cards, chips.
    pub shapes: Vec<SidebarShape>,
    /// Card text, chip labels and the header.
    pub runs: Vec<TextRun>,
    /// The nodes drawn (cards and fold chips), in node order.
    pub drawn: Vec<usize>,
    /// The edges drawn, `(parent, child)`.
    pub edges: Vec<(usize, usize)>,
    /// What a press resolves against, window-absolute `[x, y, w, h]`.
    pub hits: Vec<(FlowHit, [f32; 4])>,
}

/// `shape` at `alpha` × its opacity.
fn faded(mut shape: SidebarShape, alpha: f32) -> SidebarShape {
    shape.fill0[3] *= alpha;
    shape.fill1[3] *= alpha;
    shape.fill2[3] *= alpha;
    shape.border_color[3] *= alpha;
    shape
}

/// A run at `alpha`. The glyph shader ignores colour alpha (coverage is the alpha), so
/// the colour itself is darkened too — toward the dark backdrop, which reads as faded.
fn faded_run(mut run: TextRun, alpha: f32) -> TextRun {
    for c in &mut run.color {
        *c *= alpha;
    }
    run
}

/// A capsule chip filling `rect`, with `label` left-padded inside it.
fn chip(out: &mut GraphFrame, rect: [f32; 4], label: &str, layout: &GraphLayout, alpha: f32) {
    let [x, y, w, h] = rect;
    let r = h * 0.5;
    let theme = SidebarTheme::DEFAULT;
    let fill = Fill::solid(linear(theme.card_border_active, alpha));
    if let Some(shape) = SidebarShape::capsule(x + r, y + r, x + w - r, y + r, h, fill) {
        out.shapes.push(shape);
    }
    let pad = CHIP_PAD_X * layout.scale as f32;
    let max_cols = ((w - 2.0 * pad) / layout.text.meta_col).floor().max(0.0) as usize;
    out.runs.push(faded_run(
        TextRun {
            face: CardFace::Meta,
            text: truncate_cols_right(label, max_cols, layout.ellipsis),
            x: x + pad,
            top: y + (h - layout.text.meta_line) * 0.5,
            color: linear(theme.card_meta_fg, CARD_META_ALPHA),
            bold: false,
        },
        alpha,
    ));
}

/// Compose the frame at `scroll` (slots) with `hover` (a node index) — D13's culling: a
/// node is drawn only when its slot is on screen, an edge only when both ends are.
pub fn compose(layout: &GraphLayout, scroll: usize, hover: Option<usize>) -> GraphFrame {
    let theme = SidebarTheme::DEFAULT;
    let s = layout.scale as f32;
    let dy = layout.shift(scroll);
    let nodes = &layout.graph.nodes;
    let mut out = GraphFrame::default();
    let [vw, vh] = layout.viewport;
    if let Some(bg) = SidebarShape::rounded_box(
        [0.0, 0.0, vw, vh],
        0.0,
        0.0,
        Fill::solid(linear(theme.panel_bg, 1.0)),
        [0.0; 4],
    ) {
        out.shapes.push(bg);
    }

    let mut on = vec![false; nodes.len()];
    for c in layout.cards.iter().filter(|c| layout.shown(c.slot, scroll)) {
        on[c.node] = true;
    }
    for f in layout.folds.iter().filter(|f| layout.shown(f.slot, scroll)) {
        on[f.node] = true;
    }
    out.drawn = (0..nodes.len()).filter(|&i| on[i]).collect();

    // Edges first, so the cards cover their ends (D9).
    for e in layout.edges.iter().filter(|e| on[e.parent] && on[e.child]) {
        out.shapes
            .extend(e.segments.iter().map(|seg| seg.translated_y(-dy)));
        out.shapes.push(e.ring.translated_y(-dy));
        out.edges.push((e.parent, e.child));
    }

    let t = layout.text;
    for c in layout.cards.iter().filter(|c| on[c.node]) {
        let node = &nodes[c.node];
        let state = if node.kind == NodeKind::Project {
            CardState::Default
        } else {
            card_state(node.selected, hover == Some(c.node))
        };
        let card = build_card(&CardInput {
            rect: moved(c.rect, dy),
            skin: node.skin,
            state,
            status: node.status,
            title: &node.title,
            folder: &node.folder,
            branch: node.branch.as_deref(),
            show_branch: true,
            show_status: true,
            show_menu: false,
            editing: false,
            scale: layout.scale,
            title_col: t.title_col,
            title_line: t.title_line,
            meta_col: t.meta_col,
            meta_line: t.meta_line,
            ellipsis: layout.ellipsis,
        });
        let alpha = if node.muted { MUTED_ALPHA } else { 1.0 };
        out.shapes
            .extend(card.shapes.into_iter().map(|sh| faded(sh, alpha)));
        out.runs
            .extend(card.runs.into_iter().map(|r| faded_run(r, alpha)));
    }
    for f in layout.folds.iter().filter(|f| on[f.node]) {
        let node = &nodes[f.node];
        chip(&mut out, moved(f.rect, dy), &node.title, layout, 1.0);
    }
    for p in layout
        .plus_chips
        .iter()
        .filter(|p| on.get(p.node).copied().unwrap_or(false))
    {
        chip(
            &mut out,
            moved(p.rect, dy),
            &format!("+{}", p.n),
            layout,
            1.0,
        );
    }

    let header_cols = ((vw - 2.0 * MARGIN * s) / t.title_col).floor().max(0.0) as usize;
    out.runs.push(TextRun {
        face: CardFace::Title,
        text: truncate_cols_right(&layout.header_text, header_cols, layout.ellipsis),
        x: MARGIN * s,
        top: ((layout.header[3] - t.title_line) * 0.5).max(0.0),
        color: linear(theme.card_title_fg, 1.0),
        bold: true,
    });
    // The empty-state hint sits where slot 0 would, in the cards' muted meta style, and
    // only where a whole line of it fits.
    if let Some(hint) = layout.graph.empty_hint() {
        let top = layout.list_top;
        if top + t.meta_line <= vh {
            let cols = ((vw - 2.0 * MARGIN * s) / t.meta_col).floor().max(0.0) as usize;
            out.runs.push(TextRun {
                face: CardFace::Meta,
                text: truncate_cols_right(hint, cols, layout.ellipsis),
                x: MARGIN * s,
                top,
                color: linear(theme.card_meta_fg, CARD_META_ALPHA),
                bold: false,
            });
        }
    }
    out.hits = layout.hits(scroll);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sidebar_shapes::{KIND_CAPSULE, KIND_RING, KIND_ROUNDED_BOX};

    fn node(kind: NodeKind, parent: Option<usize>, target: FlowTarget) -> FlowNode {
        FlowNode {
            kind,
            parent,
            title: format!("{kind:?}"),
            skin: AgentSkin::Claude,
            status: CardStatus::Working,
            target,
            ..FlowNode::default()
        }
    }

    fn session(parent: Option<usize>, tab: u64) -> FlowNode {
        node(NodeKind::Session, parent, FlowTarget::Node(tab))
    }

    fn sub(parent: usize, tab: u64) -> FlowNode {
        node(NodeKind::Subagent, Some(parent), FlowTarget::Node(tab))
    }

    fn project() -> FlowNode {
        FlowNode {
            status: CardStatus::Hidden,
            ..node(NodeKind::Project, None, FlowTarget::None)
        }
    }

    /// R10.1's graph: project 0; session 1 (tab 7) with subagents 2, 3, 4; session 5.
    fn r10_graph() -> FlowGraph {
        FlowGraph {
            nodes: vec![
                project(),
                FlowNode {
                    extra: 3,
                    ..session(Some(0), 7)
                },
                sub(1, 7),
                sub(1, 7),
                sub(1, 7),
                session(Some(0), 8),
            ],
            resolving: false,
        }
    }

    const M: CardMetrics = CardMetrics::DEFAULT;

    fn lay(g: &FlowGraph, vw: f32, vh: f32, scale: f64) -> GraphLayout {
        layout_graph(g, [vw, vh], scale, M, 280.0)
    }

    fn card(l: &GraphLayout, n: usize) -> Option<PlacedCard> {
        l.cards.iter().copied().find(|c| c.node == n)
    }

    fn centre(r: [f32; 4]) -> [f32; 2] {
        [r[0] + r[2] * 0.5, r[1] + r[3] * 0.5]
    }

    // ---- header ----------------------------------------------------------------------

    #[test]
    fn r7_1_header_counts_sessions_only() {
        let st = |status| FlowNode {
            status,
            ..session(None, 1)
        };
        let mut nodes = vec![
            st(CardStatus::Working),
            st(CardStatus::Working),
            st(CardStatus::WaitingForInput),
            st(CardStatus::Ready),
            st(CardStatus::Hidden),
            project(),
            project(),
        ];
        nodes.extend((0..3).map(|_| sub(0, 1)));
        let g = FlowGraph {
            nodes,
            resolving: false,
        };
        assert_eq!(g.header(), "2 working · 1 waiting / 5");
        assert_eq!(FlowGraph::default().header(), "0 working · 0 waiting / 0");
        let r = FlowGraph {
            resolving: true,
            ..g
        };
        assert_eq!(r.header(), "resolving…");
    }

    #[test]
    fn the_empty_hint_shows_only_when_resolved_with_no_session() {
        assert_eq!(FlowGraph::default().empty_hint(), Some(EMPTY_HINT));
        let projects_only = FlowGraph {
            nodes: vec![project()],
            resolving: false,
        };
        assert_eq!(
            projects_only.empty_hint(),
            Some(EMPTY_HINT),
            "a project is no session"
        );
        let resolving = FlowGraph {
            nodes: Vec::new(),
            resolving: true,
        };
        assert_eq!(resolving.empty_hint(), None, "the header already says why");
        let one = FlowGraph {
            nodes: vec![session(None, 1)],
            resolving: false,
        };
        assert_eq!(one.empty_hint(), None);
    }

    #[test]
    fn an_empty_graph_draws_the_hint_under_the_header() {
        let hint_runs = |f: &GraphFrame| {
            f.runs
                .iter()
                .filter(|r| r.text.starts_with("No agent"))
                .cloned()
                .collect::<Vec<_>>()
        };
        let l = lay(&FlowGraph::default(), 3000.0, 1000.0, 1.0);
        let runs = hint_runs(&compose(&l, 0, None));
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].text, EMPTY_HINT, "a wide window fits it whole");
        assert_eq!(runs[0].top, l.list_top);
        assert_eq!(runs[0].face, CardFace::Meta);

        let narrow = lay(&FlowGraph::default(), 200.0, 1000.0, 1.0);
        let runs = hint_runs(&compose(&narrow, 0, None));
        assert_eq!(runs.len(), 1);
        assert!(
            runs[0].text.ends_with(narrow.ellipsis),
            "truncated, not clipped"
        );

        let short = lay(&FlowGraph::default(), 3000.0, l.list_top + 1.0, 1.0);
        assert!(
            hint_runs(&compose(&short, 0, None)).is_empty(),
            "no half line"
        );

        let with_session = lay(&r10_graph(), 3000.0, 1000.0, 1.0);
        assert!(hint_runs(&compose(&with_session, 0, None)).is_empty());
    }

    #[test]
    fn the_empty_hint_is_placed_and_cut_exactly() {
        // Scale 2, so a margin multiplied and a margin divided by the scale differ.
        let probe = lay(&FlowGraph::default(), 400.0, 1000.0, 2.0);
        let (line, col) = (probe.text.meta_line, probe.text.meta_col);
        let fits = lay(&FlowGraph::default(), 400.0, probe.list_top + line, 2.0);
        let runs: Vec<_> = compose(&fits, 0, None)
            .runs
            .into_iter()
            .filter(|r| r.face == CardFace::Meta)
            .collect();
        assert_eq!(runs.len(), 1, "a line that exactly fits is drawn");
        assert_eq!(runs[0].x, MARGIN * 2.0);
        let cols = ((400.0 - 2.0 * MARGIN * 2.0) / col).floor() as usize;
        assert!(cols < EMPTY_HINT.chars().count(), "the probe must truncate");
        assert_eq!(runs[0].text.chars().count(), cols);

        let short = lay(
            &FlowGraph::default(),
            400.0,
            probe.list_top + line - 0.5,
            2.0,
        );
        assert!(
            compose(&short, 0, None)
                .runs
                .iter()
                .all(|r| r.face != CardFace::Meta),
            "half a pixel short is not a whole line"
        );
    }

    #[test]
    fn defaults_are_an_inert_hidden_session() {
        let n = FlowNode::default();
        assert_eq!(n.kind, NodeKind::Session);
        assert_eq!(NodeKind::default(), NodeKind::Session);
        assert_eq!(n.parent, None);
        assert_eq!((n.title.as_str(), n.folder.as_str()), ("", ""));
        assert_eq!(n.branch, None);
        assert_eq!(n.skin, AgentSkin::Generic);
        assert_eq!(n.status, CardStatus::Hidden);
        assert!(!n.muted && !n.selected);
        assert_eq!(n.target, FlowTarget::None);
        assert_eq!(FlowTarget::default(), FlowTarget::None);
        assert_eq!(n.extra, 0);
    }

    // ---- modes and widths ------------------------------------------------------------

    #[test]
    fn mode_thresholds_sit_exactly_at_min_card_w() {
        // 3 × 180 + 2 × 48 + 2 × 24 = 684; 2 → 456; 1 → 228.
        let w = 252.0;
        assert_eq!(choose_mode(684.0, 1.0, w), (LayoutMode::Full, 180.0));
        assert_eq!(choose_mode(683.0, 1.0, w).0, LayoutMode::TwoColumns);
        assert_eq!(choose_mode(456.0, 1.0, w), (LayoutMode::TwoColumns, 180.0));
        assert_eq!(choose_mode(455.0, 1.0, w).0, LayoutMode::OneColumn);
        assert_eq!(choose_mode(228.0, 1.0, w), (LayoutMode::OneColumn, 180.0));
        assert_eq!(choose_mode(227.0, 1.0, w), (LayoutMode::HeaderOnly, 0.0));
        // Scaled: every constant doubles.
        assert_eq!(choose_mode(1368.0, 2.0, 504.0), (LayoutMode::Full, 360.0));
        assert_eq!(choose_mode(1367.0, 2.0, 504.0).0, LayoutMode::TwoColumns);
    }

    #[test]
    fn card_width_is_the_sidebar_cards_shrunk_to_fit_and_floored() {
        assert_eq!(choose_mode(1600.0, 1.0, 252.0).1, 252.0);
        let mid = choose_mode(700.0, 1.0, 252.0).1;
        assert!((mid - (700.0 - 144.0) / 3.0).abs() < 1e-4, "{mid}");
        // Two columns: (600 − 48 − 48) / 2 = 252 → 240 sidebar card caps it.
        assert_eq!(
            choose_mode(600.0, 1.0, 240.0),
            (LayoutMode::TwoColumns, 240.0)
        );
        assert_eq!(choose_mode(520.0, 1.0, 240.0).1, (520.0 - 96.0) / 2.0);
        // A sidebar card narrower than the floor is widened to it.
        assert_eq!(choose_mode(1600.0, 1.0, 100.0), (LayoutMode::Full, 180.0));
        assert_eq!(choose_mode(3000.0, 2.0, 100.0), (LayoutMode::Full, 360.0));
        // One column: 300 − 48 = 252.
        assert_eq!(
            choose_mode(300.0, 1.0, 400.0),
            (LayoutMode::OneColumn, 252.0)
        );
    }

    #[test]
    fn layout_scales_the_sidebar_card_width() {
        let l = lay(&r10_graph(), 3000.0, 2000.0, 2.0);
        assert_eq!(l.card_w, (280.0 - 28.0) * 2.0);
        assert_eq!(l.card_h, 208.0);
        assert_eq!(l.slot_h, 228.0);
        assert_eq!(l.list_top, 80.0);
        assert_eq!(l.header, [0.0, 0.0, 3000.0, 80.0]);
        let xs: Vec<f32> = [0, 1, 2]
            .iter()
            .map(|&n| card(&l, n).expect("card").rect[0])
            .collect();
        assert_eq!(
            xs,
            vec![48.0, 48.0 + 504.0 + 96.0, 48.0 + 2.0 * (504.0 + 96.0)]
        );
        assert_eq!(l.mode, LayoutMode::Full);
    }

    #[test]
    fn column_xs_per_mode() {
        assert_eq!(
            column_xs(LayoutMode::Full, 1.0, 100.0),
            [Some(24.0), Some(172.0), Some(320.0)]
        );
        assert_eq!(
            column_xs(LayoutMode::TwoColumns, 1.0, 100.0),
            [Some(24.0), Some(172.0), None]
        );
        assert_eq!(
            column_xs(LayoutMode::OneColumn, 2.0, 100.0),
            [None, Some(48.0), None]
        );
        assert_eq!(column_xs(LayoutMode::HeaderOnly, 1.0, 100.0), [None; 3]);
    }

    // ---- slots -----------------------------------------------------------------------

    #[test]
    fn full_mode_slots_and_rects() {
        let l = lay(&r10_graph(), 1600.0, 1000.0, 1.0);
        let slots: Vec<(usize, usize)> = l.cards.iter().map(|c| (c.node, c.slot)).collect();
        assert_eq!(slots, vec![(0, 0), (1, 0), (2, 0), (3, 1), (4, 2), (5, 3)]);
        assert_eq!(l.total_slots, 4);
        let r = card(&l, 4).expect("subagent").rect;
        assert_eq!(r, [24.0 + 2.0 * 300.0, 40.0 + 2.0 * 114.0, 252.0, 104.0]);
        assert!(l.folds.is_empty() && l.plus_chips.is_empty());
        assert_eq!(l.header_text, "2 working · 0 waiting / 2");
    }

    #[test]
    fn groups_follow_node_order_and_ungrouped_sessions_are_roots() {
        // Session 0 has no project; project 1 with session 2; session 3 has no parent.
        let g = FlowGraph {
            nodes: vec![
                session(None, 1),
                project(),
                session(Some(1), 2),
                session(None, 3),
            ],
            resolving: false,
        };
        let l = lay(&g, 1600.0, 1000.0, 1.0);
        let slots: Vec<(usize, usize)> = l.cards.iter().map(|c| (c.node, c.slot)).collect();
        assert_eq!(slots, vec![(0, 0), (1, 1), (2, 1), (3, 2)]);
        assert_eq!(l.edges.len(), 1);
        assert_eq!((l.edges[0].parent, l.edges[0].child), (1, 2));
    }

    #[test]
    fn a_parent_that_is_not_a_project_or_session_is_ignored() {
        // Session 1 claims session 0 as parent (not a project): it is its own root.
        // Subagent 2 claims project 3 (not a session) and subagent 4 an index out of
        // range: neither is placed.
        let g = FlowGraph {
            nodes: vec![
                session(None, 1),
                session(Some(0), 2),
                sub(3, 9),
                project(),
                sub(99, 9),
            ],
            resolving: false,
        };
        let l = lay(&g, 1600.0, 1000.0, 1.0);
        let slots: Vec<(usize, usize)> = l.cards.iter().map(|c| (c.node, c.slot)).collect();
        assert_eq!(slots, vec![(0, 0), (1, 1), (3, 2)]);
        // An edge only for a placed child under a placed parent: 0 → 1 (session under
        // session is still a parent link, both placed).
        let pairs: Vec<(usize, usize)> = l.edges.iter().map(|e| (e.parent, e.child)).collect();
        assert_eq!(pairs, vec![(0, 1)]);
        assert_eq!(l.total_slots, 3);
    }

    #[test]
    fn an_empty_project_takes_one_slot_but_not_in_one_column() {
        let g = FlowGraph {
            nodes: vec![project(), session(None, 1)],
            resolving: false,
        };
        let l = lay(&g, 1600.0, 1000.0, 1.0);
        assert_eq!(card(&l, 1).expect("s").slot, 1);
        assert_eq!(l.total_slots, 2);
        let one = lay(&g, 300.0, 1000.0, 1.0);
        assert_eq!(one.mode, LayoutMode::OneColumn);
        assert_eq!(card(&one, 1).expect("s").slot, 0);
        assert_eq!(one.total_slots, 1);
    }

    #[test]
    fn fold_nodes_are_chips_centred_in_their_slot() {
        let mut g = r10_graph();
        g.nodes
            .insert(5, node(NodeKind::Fold, Some(1), FlowTarget::Fold(7, 10)));
        let l = lay(&g, 1600.0, 1000.0, 1.0);
        assert_eq!(l.folds.len(), 1);
        let f = l.folds[0];
        assert_eq!((f.node, f.slot), (5, 3));
        let y = 40.0 + 3.0 * 114.0 + (104.0 - 32.0) / 2.0;
        assert_eq!(f.rect, [624.0, y, 252.0, 32.0]);
        assert_eq!(card(&l, 6).expect("session b").slot, 4);
        assert_eq!(l.total_slots, 5);
        let e = l.edges.iter().find(|e| e.child == 5).expect("fold edge");
        assert_eq!(e.parent, 1);
        let last = e.segments[EDGE_SEGMENTS - 1].seg;
        assert_eq!([last[2], last[3]], [624.0, y + 16.0]);
    }

    #[test]
    fn two_columns_chip_from_extra_alone() {
        // `extra` is the app's count; a session with subagents but extra 0 shows none.
        let mut g = r10_graph();
        g.nodes[5].extra = 12;
        let l = lay(&g, 683.0, 1000.0, 1.0);
        assert_eq!(l.mode, LayoutMode::TwoColumns);
        let chips: Vec<(usize, u32)> = l.plus_chips.iter().map(|c| (c.node, c.n)).collect();
        assert_eq!(chips, vec![(1, 3), (5, 12)]);
        let a = card(&l, 1).expect("a").rect;
        let c = l.plus_chips[0].rect;
        let meta_col = CardFace::Meta.column_px(1.0);
        let w = 2.0 * meta_col + 16.0;
        let w = w.max(22.0);
        assert_eq!(c, [a[0] + a[2] - w / 2.0, a[1] + 41.0, w, 22.0]);
        assert_eq!(l.total_slots, 2);
        assert!(l.edges.iter().all(|e| e.parent == 0), "project edges only");
        assert_eq!(l.edges.len(), 2);
        g.nodes[1].extra = 0;
        assert_eq!(lay(&g, 683.0, 1000.0, 1.0).plus_chips.len(), 1);
    }

    #[test]
    fn a_long_plus_label_widens_its_chip() {
        let mut g = r10_graph();
        g.nodes[1].extra = 123_456;
        let l = layout_graph_with(
            &g,
            [683.0, 1000.0],
            1.0,
            M,
            280.0,
            CardMeasure {
                meta_col: 10.0,
                ..CardMeasure::of_faces(1.0)
            },
            '…',
        );
        assert_eq!(l.plus_chips[0].rect[2], 7.0 * 10.0 + 16.0);
        assert_eq!(l.text.meta_col, 10.0);
    }

    #[test]
    fn chips_folds_and_edges_scale() {
        // Every logical constant is multiplied by the scale, never divided by it.
        let mut g = r10_graph();
        g.nodes
            .insert(5, node(NodeKind::Fold, Some(1), FlowTarget::Fold(7, 10)));
        g.nodes[5].title = "x".repeat(200);
        let l = lay(&g, 3000.0, 3000.0, 2.0);
        let f = l.folds[0].rect;
        assert_eq!(f[3], 64.0, "FOLD_H × 2");
        assert_eq!(f[1], 80.0 + 3.0 * 228.0 + (208.0 - 64.0) / 2.0);
        let frame = compose(&l, 0, None);
        let label = frame
            .runs
            .iter()
            .find(|r| r.text.starts_with('x'))
            .expect("fold label");
        assert_eq!(label.x, f[0] + 16.0, "CHIP_PAD_X × 2");
        let max = ((f[2] - 32.0) / l.text.meta_col).floor() as usize;
        assert_eq!(cols(&label.text), max);
        // Edges: the first capsule leaves the parent's right-edge midpoint, and the
        // control offset is half a scaled column gap (48 px at 2×).
        let e = l.edges.iter().find(|e| e.child == 2).expect("edge");
        let p = card(&l, 1).expect("parent").rect;
        let c = card(&l, 2).expect("child").rect;
        let from = [p[0] + p[2], p[1] + p[3] * 0.5];
        let to = [c[0], c[1] + c[3] * 0.5];
        assert_eq!([e.segments[0].seg[0], e.segments[0].seg[1]], from);
        let t: f32 = 1.0 / 16.0;
        let u = 1.0 - t;
        let x = u * u * u * from[0]
            + 3.0 * u * u * t * (from[0] + 48.0)
            + 3.0 * u * t * t * (to[0] - 48.0)
            + t * t * t * to[0];
        assert!(
            (e.segments[0].seg[2] - x).abs() < 1e-3,
            "{}",
            e.segments[0].seg[2]
        );
        // The +N chip at 2×: 44 tall, padded 16 each side.
        let mut g2 = r10_graph();
        g2.nodes[1].extra = 3;
        let two = lay(&g2, 1300.0, 3000.0, 2.0);
        assert_eq!(two.mode, LayoutMode::TwoColumns);
        let chip = two.plus_chips[0].rect;
        assert_eq!(chip[3], 44.0);
        assert_eq!(chip[2], (2.0 * two.text.meta_col + 32.0).max(44.0));
        let a = card(&two, 1).expect("a").rect;
        assert_eq!(chip[1], a[1] + (208.0 - 44.0) / 2.0);
    }

    #[test]
    fn a_zero_scale_fits_no_slot() {
        assert_eq!(lay(&r10_graph(), 1600.0, 1000.0, 0.0).visible_slots, 0);
    }

    #[test]
    fn header_only_mode_lays_out_nothing_else() {
        let l = lay(&r10_graph(), 200.0, 1000.0, 1.0);
        assert_eq!(l.mode, LayoutMode::HeaderOnly);
        assert!(l.cards.is_empty() && l.edges.is_empty());
        assert_eq!(l.total_slots, 0);
        assert_eq!(l.header, [0.0, 0.0, 200.0, 40.0]);
    }

    #[test]
    fn visible_slots_count_whole_cards_below_the_header() {
        // 3 cards + 2 gaps = 332 below a 40px header.
        let g = r10_graph();
        assert_eq!(lay(&g, 1600.0, 372.0, 1.0).visible_slots, 3);
        assert_eq!(lay(&g, 1600.0, 371.0, 1.0).visible_slots, 2);
        assert_eq!(lay(&g, 1600.0, 144.0, 1.0).visible_slots, 1);
        assert_eq!(lay(&g, 1600.0, 143.0, 1.0).visible_slots, 0);
        assert_eq!(lay(&g, 1600.0, 0.0, 1.0).visible_slots, 0);
        assert_eq!(lay(&g, 1600.0, -40.0, 1.0).visible_slots, 0);
        let l = lay(&g, 1600.0, 372.0, 1.0);
        assert_eq!(l.max_scroll(), 1);
        assert_eq!(lay(&g, 1600.0, 2000.0, 1.0).max_scroll(), 0);
    }

    // ---- edges -----------------------------------------------------------------------

    #[test]
    fn edges_sample_the_cubic_and_carry_the_colour() {
        let (segs, ring) = edge_shapes([0.0, 0.0], [48.0, 100.0], 1.0, false).expect("finite edge");
        assert_eq!(segs.len(), EDGE_SEGMENTS);
        assert!(segs.iter().all(|s| s.kind == KIND_CAPSULE));
        // The midpoint of a symmetric S-curve is the chord's midpoint.
        let mid = segs[EDGE_SEGMENTS / 2].seg;
        assert!((mid[0] - 24.0).abs() < 1e-4 && (mid[1] - 50.0).abs() < 1e-4);
        // t = 1/16 by hand: control points (24, 0) and (24, 100).
        let t: f32 = 1.0 / 16.0;
        let u = 1.0 - t;
        let x = 3.0 * u * u * t * 24.0 + 3.0 * u * t * t * 24.0 + t * t * t * 48.0;
        let y = 3.0 * u * t * t * 100.0 + t * t * t * 100.0;
        assert!((segs[0].seg[2] - x).abs() < 1e-4 && (segs[0].seg[3] - y).abs() < 1e-4);
        assert_eq!(segs[0].geom[0], 0.75, "1.5px stroke");
        assert_eq!(segs[0].fill0, edge_color(false));
        assert_eq!(ring.kind, KIND_RING);
        assert_eq!(ring.geom, [3.0, 1.5]);
        assert_eq!([ring.seg[0], ring.seg[1]], [48.0, 100.0]);
        let (msegs, mring) = edge_shapes([0.0, 0.0], [48.0, 0.0], 2.0, true).expect("edge");
        assert_eq!(msegs[0].fill0, edge_color(true));
        assert_eq!(mring.geom, [6.0, 3.0]);
        assert!(edge_shapes([f32::NAN, 0.0], [1.0, 1.0], 1.0, false).is_none());
    }

    #[test]
    fn edge_colour_is_the_active_border_at_edge_alpha() {
        let c = linear(SidebarTheme::DEFAULT.card_border_active, EDGE_ALPHA);
        assert_eq!(edge_color(false), c);
        assert_eq!(edge_color(true)[3], EDGE_ALPHA * MUTED_ALPHA);
        assert_eq!(edge_color(true)[..3], c[..3]);
    }

    /// Linear → sRGB, 0..=255 — what the surface stores after blending in linear space.
    fn srgb_byte(l: f32) -> f32 {
        let s = if l <= 0.003_130_8 {
            l * 12.92
        } else {
            1.055 * l.powf(1.0 / 2.4) - 0.055
        };
        s * 255.0
    }

    /// The colour a fully covered edge pixel lands on screen as, over `panel_bg`, in
    /// sRGB counts: source-over in linear space, then encoded.
    fn edge_over_backdrop(muted: bool) -> [f32; 3] {
        let e = edge_color(muted);
        let bg = linear(SidebarTheme::DEFAULT.panel_bg, 1.0);
        std::array::from_fn(|i| srgb_byte(e[i] * e[3] + bg[i] * (1.0 - e[3])))
    }

    #[test]
    fn d9_an_edge_stands_clearly_above_the_backdrop() {
        // D9 (2026-10-21): at the border alpha an edge sat ~6 counts above `panel_bg`
        // and read as nothing. Pin the contrast, not just "differs", so a later dimming
        // back toward the border alpha goes red.
        let bg = linear(SidebarTheme::DEFAULT.panel_bg, 1.0).map(srgb_byte);
        let lift = |muted| {
            let px = edge_over_backdrop(muted);
            (0..3).map(|i| px[i] - bg[i]).fold(f32::MIN, f32::max)
        };
        assert!(lift(false) >= 18.0, "edge lift {} counts", lift(false));
        // A muted edge stays visible but reads dimmer than a live one.
        assert!(lift(true) >= 8.0, "muted edge lift {} counts", lift(true));
        assert!(lift(true) < lift(false));
    }

    #[test]
    fn bezier_hits_its_ends_and_weights() {
        let p = [[8.0, 16.0], [1.0, 2.0], [3.0, 4.0], [5.0, 6.0]];
        assert_eq!(bezier(p, 0.0), [8.0, 16.0]);
        assert_eq!(bezier(p, 1.0), [5.0, 6.0]);
        // t = .5: (p0 + 3p1 + 3p2 + p3) / 8.
        assert_eq!(bezier(p, 0.5), [25.0 / 8.0, 40.0 / 8.0]);
        // t = .25: weights 27, 27, 9, 1 over 64.
        let w = bezier(p, 0.25);
        let want = (27.0 * 8.0 + 27.0 * 1.0 + 9.0 * 3.0 + 5.0) / 64.0;
        assert!((w[0] - want).abs() < 1e-5, "{w:?}");
    }

    #[test]
    fn a_muted_child_gets_a_muted_edge() {
        let mut g = r10_graph();
        g.nodes[3].muted = true;
        let l = lay(&g, 1600.0, 1000.0, 1.0);
        let e = |c: usize| l.edges.iter().find(|e| e.child == c).expect("edge");
        assert_eq!(e(3).segments[0].fill0, edge_color(true));
        assert_eq!(e(2).segments[0].fill0, edge_color(false));
    }

    // ---- hit-testing -----------------------------------------------------------------

    #[test]
    fn r9_1_hits_cards_and_folds_only() {
        let mut g = r10_graph();
        g.nodes
            .insert(5, node(NodeKind::Fold, Some(1), FlowTarget::Fold(7, 10)));
        let l = lay(&g, 1600.0, 1000.0, 1.0);
        let at = |n: usize| centre(card(&l, n).expect("card").rect);
        assert_eq!(l.hit(at(0)), None, "the project is inert");
        assert_eq!(l.hit(at(1)), Some(FlowHit::Node(7)));
        assert_eq!(l.hit(at(2)), Some(FlowHit::Node(7)));
        assert_eq!(l.hit(centre(l.folds[0].rect)), Some(FlowHit::Fold(7, 10)));
        let e = &l.edges[0].segments[EDGE_SEGMENTS / 2].seg;
        assert_eq!(l.hit([e[0], e[1]]), None, "edges are inert");
        assert_eq!(l.hit([2.0, 2.0]), None, "the header and margins are empty");
        // Half-open: a card's far edge is outside it, its near edge inside.
        let r = card(&l, 1).expect("s").rect;
        assert_eq!(l.hit([r[0], r[1]]), Some(FlowHit::Node(7)));
        assert_eq!(l.hit([r[0] + r[2], r[1] + 1.0]), None);
        assert_eq!(l.hit([r[0] + 1.0, r[1] + r[3]]), None);
        assert_eq!(l.hit([r[0] - 0.01, r[1] + 1.0]), None);
        assert_eq!(l.hit([r[0] + 1.0, r[1] - 0.01]), None);
    }

    #[test]
    fn a_session_with_no_target_is_not_pressable() {
        let g = FlowGraph {
            nodes: vec![node(NodeKind::Session, None, FlowTarget::None)],
            resolving: false,
        };
        let l = lay(&g, 1600.0, 1000.0, 1.0);
        assert_eq!(l.hit(centre(l.cards[0].rect)), None);
        assert_eq!(
            l.node_at(centre(l.cards[0].rect), 0),
            Some(0),
            "but it hovers"
        );
    }

    #[test]
    fn a_project_with_a_target_is_still_inert() {
        let g = FlowGraph {
            nodes: vec![FlowNode {
                target: FlowTarget::Node(3),
                ..project()
            }],
            resolving: false,
        };
        let l = lay(&g, 1600.0, 1000.0, 1.0);
        let c = centre(l.cards[0].rect);
        assert_eq!(l.hit(c), None);
        assert_eq!(l.node_at(c, 0), None);
    }

    #[test]
    fn hits_and_hover_follow_the_scroll() {
        let l = lay(&r10_graph(), 1600.0, 40.0 + 2.0 * 114.0, 1.0);
        assert_eq!(l.visible_slots, 2);
        let sub3 = card(&l, 4).expect("slot 2").rect;
        let at_scroll1 = [sub3[0] + 5.0, sub3[1] - 114.0 + 5.0];
        assert_eq!(l.hit_at(at_scroll1, 1), Some(FlowHit::Node(7)));
        assert_eq!(l.node_at(at_scroll1, 1), Some(4));
        // At scroll 0 that point is slot 1's subagent.
        assert_eq!(l.node_at(at_scroll1, 0), Some(3));
        // Slot 3 is past the window at scroll 1: no hit where it would be drawn.
        let b = card(&l, 5).expect("slot 3").rect;
        let p = [b[0] + 5.0, b[1] - 114.0 + 5.0];
        assert_eq!(l.hit_at(p, 1), None);
        assert_eq!(l.node_at(p, 1), None);
        // Slot 0 is above the window at scroll 1.
        let s0 = card(&l, 1).expect("slot 0").rect;
        assert_eq!(l.hit_at([s0[0] + 5.0, s0[1] - 114.0 + 5.0], 1), None);
        // A culled fold is not pressable either.
        let mut g = r10_graph();
        g.nodes
            .insert(2, node(NodeKind::Fold, Some(1), FlowTarget::Fold(7, 1)));
        let l = lay(&g, 1600.0, 40.0 + 114.0, 1.0);
        let f = l.folds[0];
        assert_eq!(f.slot, 0);
        assert_eq!(l.hit_at(centre(f.rect), 0), Some(FlowHit::Fold(7, 1)));
        let below = [f.rect[0] + 5.0, f.rect[1] + 114.0];
        assert_eq!(l.hit_at(below, 0), None, "slot 1 is culled");
        assert_eq!(l.hit_at(centre(moved(f.rect, 114.0)), 1), None);
    }

    #[test]
    fn shown_is_the_half_open_window() {
        let l = lay(&r10_graph(), 1600.0, 40.0 + 2.0 * 114.0, 1.0);
        assert!(!l.shown(0, 1));
        assert!(l.shown(1, 1) && l.shown(2, 1));
        assert!(!l.shown(3, 1));
        assert_eq!(l.shift(3), 342.0);
    }

    // ---- compose ---------------------------------------------------------------------

    #[test]
    fn r11_2_culling_draws_whole_nodes_and_edges_with_both_ends() {
        let mut g = r10_graph();
        g.nodes.push(sub(5, 8)); // the second session's subagent, slot 3
        let l = lay(&g, 1600.0, 40.0 + 3.0 * 114.0, 1.0);
        assert_eq!(l.visible_slots, 3);
        let f = compose(&l, 1, None);
        assert_eq!(f.drawn, vec![3, 4, 5, 6]);
        assert_eq!(f.edges, vec![(5, 6)]);
        // Drawn cards moved up one slot: subagent 3 sits where slot 0 was.
        let at_top = centre(card(&l, 2).expect("slot 0").rect);
        assert!(f
            .hits
            .iter()
            .any(|(h, r)| *h == FlowHit::Node(7) && contains(*r, at_top)));
        assert_eq!(f.hits.len(), 4);
        // Unscrolled, everything but slot 3 is drawn, with four edges.
        let f0 = compose(&l, 0, None);
        assert_eq!(f0.drawn, vec![0, 1, 2, 3, 4]);
        assert_eq!(f0.edges, vec![(0, 1), (1, 2), (1, 3), (1, 4)]);
    }

    #[test]
    fn compose_starts_with_an_opaque_full_viewport_backdrop() {
        let l = lay(&r10_graph(), 1600.0, 1000.0, 1.0);
        let f = compose(&l, 0, None);
        let bg = f.shapes[0];
        assert_eq!(bg.kind, KIND_ROUNDED_BOX);
        assert_eq!(bg.bbox, [0.0, 0.0, 1600.0, 1000.0]);
        assert_eq!(bg.fill0, linear(SidebarTheme::DEFAULT.panel_bg, 1.0));
        // Then the edges' capsules, before any card body.
        let first_card = f
            .shapes
            .iter()
            .position(|s| s.kind == KIND_ROUNDED_BOX && s.bbox[2] == 252.0)
            .expect("a card");
        let edge_shapes = 5 * (EDGE_SEGMENTS + 1);
        assert_eq!(first_card, 1 + edge_shapes);
        assert!(f.shapes[1..first_card]
            .iter()
            .all(|s| s.kind == KIND_CAPSULE || s.kind == KIND_RING));
    }

    #[test]
    fn compose_translates_edges_by_the_scroll() {
        let mut g = r10_graph();
        g.nodes.push(sub(5, 8));
        let l = lay(&g, 1600.0, 40.0 + 3.0 * 114.0, 1.0);
        let f = compose(&l, 1, None);
        let e = l.edges.iter().find(|e| e.child == 6).expect("edge");
        let want = e.segments[0].translated_y(-114.0);
        assert!(f.shapes.contains(&want));
        assert!(f.shapes.contains(&e.ring.translated_y(-114.0)));
    }

    fn body_of(f: &GraphFrame, rect: [f32; 4]) -> SidebarShape {
        *f.shapes
            .iter()
            .find(|s| s.kind == KIND_ROUNDED_BOX && s.bbox == rect)
            .expect("card body")
    }

    #[test]
    fn card_states_follow_selection_and_hover_but_projects_stay_default() {
        let mut g = r10_graph();
        g.nodes[5].selected = true;
        g.nodes[0].selected = true;
        let l = lay(&g, 1600.0, 1000.0, 1.0);
        let rect = |n| card(&l, n).expect("c").rect;
        let want = |n: usize, state| {
            let node = &g.nodes[n];
            let d = build_card(&CardInput {
                rect: rect(n),
                skin: node.skin,
                state,
                status: node.status,
                title: &node.title,
                folder: "",
                branch: None,
                show_branch: true,
                show_status: true,
                show_menu: false,
                editing: false,
                scale: 1.0,
                title_col: l.text.title_col,
                title_line: l.text.title_line,
                meta_col: l.text.meta_col,
                meta_line: l.text.meta_line,
                ellipsis: '…',
            });
            *d.shapes.iter().find(|s| s.bbox == rect(n)).expect("body")
        };
        let f = compose(&l, 0, Some(2));
        assert_eq!(body_of(&f, rect(2)), want(2, CardState::Hover));
        assert_eq!(body_of(&f, rect(3)), want(3, CardState::Default));
        assert_eq!(body_of(&f, rect(5)), want(5, CardState::Selected));
        assert_eq!(body_of(&f, rect(0)), want(0, CardState::Default));
        let hovered_project = compose(&l, 0, Some(0));
        assert_eq!(
            body_of(&hovered_project, rect(0)),
            want(0, CardState::Default)
        );
        assert_eq!(f.runs.iter().filter(|r| r.text == "Working").count(), 5);
    }

    #[test]
    fn muted_cards_fade_shapes_and_text() {
        let mut g = r10_graph();
        g.nodes[3].muted = true;
        g.nodes[3].title = "finished one".into();
        let l = lay(&g, 1600.0, 1000.0, 1.0);
        let f = compose(&l, 0, None);
        let r = card(&l, 3).expect("c").rect;
        let plain = compose(&lay(&r10_graph(), 1600.0, 1000.0, 1.0), 0, None);
        let (a, b) = (body_of(&f, r), body_of(&plain, r));
        assert_eq!(a.fill0[3], b.fill0[3] * MUTED_ALPHA);
        assert_eq!(a.fill1[3], b.fill1[3] * MUTED_ALPHA);
        assert_eq!(a.fill2[3], b.fill2[3] * MUTED_ALPHA);
        assert_eq!(a.border_color[3], b.border_color[3] * MUTED_ALPHA);
        assert_eq!(a.fill0[..3], b.fill0[..3]);
        let run = f
            .runs
            .iter()
            .find(|r| r.text == "finished one")
            .expect("title");
        let full = linear(SidebarTheme::DEFAULT.card_title_fg, 1.0);
        assert_eq!(run.color, full.map(|c| c * MUTED_ALPHA));
    }

    #[test]
    fn chips_draw_a_capsule_and_their_label() {
        let mut g = r10_graph();
        g.nodes
            .insert(5, node(NodeKind::Fold, Some(1), FlowTarget::Fold(7, 10)));
        g.nodes[5].title = "▸ 4 finished".into();
        let l = lay(&g, 1600.0, 1000.0, 1.0);
        let f = compose(&l, 0, None);
        let fr = l.folds[0].rect;
        let cap = f
            .shapes
            .iter()
            .find(|s| s.kind == KIND_CAPSULE && s.geom[0] == 16.0)
            .expect("fold capsule");
        assert_eq!(
            cap.seg,
            [
                fr[0] + 16.0,
                fr[1] + 16.0,
                fr[0] + fr[2] - 16.0,
                fr[1] + 16.0
            ]
        );
        assert_eq!(
            cap.fill0,
            linear(SidebarTheme::DEFAULT.card_border_active, 1.0)
        );
        let label = f
            .runs
            .iter()
            .find(|r| r.text == "▸ 4 finished")
            .expect("label");
        assert_eq!(label.x, fr[0] + 8.0);
        assert_eq!(label.top, fr[1] + (32.0 - l.text.meta_line) / 2.0);
        assert_eq!(label.face, CardFace::Meta);
        assert_eq!(
            label.color,
            linear(SidebarTheme::DEFAULT.card_meta_fg, CARD_META_ALPHA)
        );

        let two = lay(&r10_graph(), 683.0, 1000.0, 1.0);
        let f2 = compose(&two, 0, None);
        let plus = f2.runs.iter().find(|r| r.text == "+3").expect("+3");
        assert_eq!(plus.x, two.plus_chips[0].rect[0] + 8.0);
        // The +N chip is drawn but not pressable.
        assert_eq!(f2.hits.len(), 2);
    }

    #[test]
    fn a_chip_label_is_truncated_to_the_chip() {
        let mut g = r10_graph();
        g.nodes
            .insert(5, node(NodeKind::Fold, Some(1), FlowTarget::Fold(7, 10)));
        g.nodes[5].title = "x".repeat(200);
        let l = lay(&g, 1600.0, 1000.0, 1.0);
        let f = compose(&l, 0, None);
        let max = ((252.0 - 16.0) / l.text.meta_col).floor() as usize;
        assert!(f
            .runs
            .iter()
            .any(|r| r.text.ends_with('…') && cols(&r.text) == max));
    }

    #[test]
    fn a_culled_session_draws_no_plus_chip() {
        let l = lay(&r10_graph(), 683.0, 40.0 + 114.0, 1.0);
        assert_eq!(l.visible_slots, 1);
        let f = compose(&l, 1, None);
        assert!(f.runs.iter().all(|r| r.text != "+3"));
        assert!(compose(&l, 0, None).runs.iter().any(|r| r.text == "+3"));
    }

    #[test]
    fn the_header_is_a_title_run_centred_in_its_band() {
        let l = lay(&r10_graph(), 1600.0, 1000.0, 2.0);
        let f = compose(&l, 3, None);
        let h = f.runs.last().expect("header");
        assert_eq!(h.text, "2 working · 0 waiting / 2");
        assert_eq!(h.face, CardFace::Title);
        assert_eq!(h.x, 48.0);
        assert_eq!(h.top, (80.0 - l.text.title_line) / 2.0);
        assert!(h.bold);
        // A narrow window truncates it rather than drawing past the margin.
        let n = lay(&r10_graph(), 200.0, 1000.0, 1.0);
        let hn = compose(&n, 0, None);
        let text = &hn.runs.last().expect("header").text;
        let max = ((200.0 - 48.0) / n.text.title_col).floor() as usize;
        assert!(cols(text) <= max && text.ends_with('…'), "{text}");
        assert_eq!(hn.drawn, Vec::<usize>::new());
        // At 2×: (296 − 2 × 24 × 2) / 20 = exactly 10 title columns.
        let n2 = lay(&r10_graph(), 296.0, 1000.0, 2.0);
        assert_eq!(n2.text.title_col, 20.0);
        let h2 = compose(&n2, 0, None);
        assert_eq!(cols(&h2.runs.last().expect("header").text), 10);
    }

    #[test]
    fn faded_helpers_scale_alpha() {
        let s = SidebarShape::rounded_box(
            [0.0, 0.0, 10.0, 10.0],
            1.0,
            1.0,
            Fill::solid([0.5, 0.5, 0.5, 0.8]),
            [0.2, 0.2, 0.2, 0.6],
        )
        .expect("box");
        let f = faded(s, 0.5);
        assert_eq!(f.fill0, [0.5, 0.5, 0.5, 0.4]);
        assert_eq!(f.border_color, [0.2, 0.2, 0.2, 0.3]);
        assert_eq!(faded(s, 1.0), s);
    }
}
