//! The session graph as a registered [`DrawLayer`] (session-flow-graph D4, D11, D14).
//!
//! The app's `FlowOverlay` pushes a [`FlowGraph`] into a shared [`FlowHandle`] whenever
//! the sessions change; this layer paints the **whole viewport** at z [`GRAPH_PANEL_Z`],
//! above the ai-monitor panel, with an opaque backdrop, so nothing of the terminal, the
//! sidebar or an open monitor shows through (D4). It reserves no inset: opening the graph
//! never reflows the grid (R3.3). Renderer-owned and app-type-free: plain data, tab ids
//! as `u64`.
//!
//! **Two stages, as the ai-monitor and the sidebar have (D11).** Stage one,
//! [`graph_layout::layout_graph_with`], runs only when the graph, the viewport, the scale,
//! the sidebar width or the card atlases' identity changed. Stage two,
//! [`graph_layout::compose`], runs when stage one did or when the scroll offset or the
//! hovered card changed. An unchanged frame re-submits the resident buffers and writes
//! none (G1) — the window redraws at 60 fps while visible (architecture C6), so "unchanged"
//! is the common case.
//!
//! **Card text** is drawn from the layer's own [`CardAtlases`] (D14), built on the first
//! visible frame and never at startup, through the same pipelines and the same run
//! resolution (`sidebar_row::resolve_card_runs`) the sidebar uses — so a graph card's text
//! is the sidebar card's text, glyph for glyph.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::graph_layout::{self, FlowGraph, FlowTarget, GraphFrame, GraphLayout};
use crate::sidebar_row::TextInstance;
use crate::sidebar_row::{resolve_card_runs, CardGlyphs, SidebarTextPipeline, TextAtlas};
use crate::sidebar_shapes::SidebarShapePipeline;
use crate::sidebar_text::{CardAtlases, CardFace, CardMeasure};
use crate::sidebar_theme::CardMetrics;
use crate::{DrawLayer, FrameCtx};

/// The graph's z: above the ai-monitor panel (100), so the monitor's band never shows
/// through (D4).
pub const GRAPH_PANEL_Z: i32 = 110;

/// What a press on the graph asked for (D7).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FlowHit {
    /// A session or subagent card: switch to this tab id and close the graph.
    Node(u64),
    /// A session's `N finished` chip: toggle `(tab id, pane pid)` in the fold set.
    Fold(u64, u32),
}

impl FlowHit {
    /// The hit a node's target produces — `None` for an inert one.
    pub fn of(target: FlowTarget) -> Option<FlowHit> {
        match target {
            FlowTarget::Node(id) => Some(FlowHit::Node(id)),
            FlowTarget::Fold(id, pid) => Some(FlowHit::Fold(id, pid)),
            FlowTarget::None => None,
        }
    }
}

/// What the last composed frame drew, published for the app (D7, D13).
///
/// `hits` are `(hit, [x, y, w, h])` window-absolute physical px, half-open on the far
/// edges; only pressable nodes are listed. `visible_slots` / `total_slots` are what the
/// app hands [`scroll_offset`]. The app owns the wheel and click policy; the renderer owns
/// the geometry, and this is the whole of what crosses.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FlowView {
    pub hits: Vec<(FlowHit, [f32; 4])>,
    pub visible_slots: usize,
    pub total_slots: usize,
}

/// A cloneable handle the app keeps; the layer reads the same cells.
///
/// `Rc<RefCell<..>>`, not a channel: the app writes and the layer reads on the one winit
/// thread. [`FlowPanel::record`] reads through `try_borrow`, so an interleaving that ever
/// became possible skips a frame instead of aborting.
#[derive(Clone, Default)]
pub struct FlowHandle {
    graph: Rc<RefCell<FlowGraph>>,
    /// Bumped by [`set`](Self::set) only when the graph actually changed — stage one's
    /// graph key, so a frame compares one integer instead of every node's strings (D11).
    revision: Rc<Cell<u64>>,
    /// The scroll offset, in slots — beside the graph, never in it (D11): the graph is
    /// stage one's key, and a wheel notch must move stage two only.
    scroll: Rc<Cell<usize>>,
    /// Defaults to **hidden**: the graph starts closed and is never restored open (D4).
    visible: Rc<Cell<bool>>,
    /// The pointer, window-absolute physical px. A point rather than a node, so the layer
    /// resolves it against the frame it is about to draw — a scroll under a still pointer
    /// moves the hover with it — and keys stage two on the *node*, not on every pixel of
    /// pointer travel.
    hover: Rc<Cell<Option<[f32; 2]>>>,
    view: Rc<RefCell<FlowView>>,
}

impl FlowHandle {
    /// Replace the graph. Stage one re-lays-out on the next frame only if it differs.
    ///
    /// The comparison runs here, once per call — the app calls this only when its own key
    /// moved — rather than on every visible frame.
    pub fn set(&self, graph: FlowGraph) {
        let mut cur = self.graph.borrow_mut();
        if *cur != graph {
            *cur = graph;
            self.revision.set(self.revision.get() + 1);
        }
    }

    /// How many times [`set`](Self::set) has changed the graph.
    pub fn revision(&self) -> u64 {
        self.revision.get()
    }

    /// A copy of the current graph.
    pub fn graph(&self) -> FlowGraph {
        self.graph.borrow().clone()
    }

    /// Show or hide the layer. Hidden, it costs a frame nothing (Constraints).
    pub fn set_visible(&self, visible: bool) {
        self.visible.set(visible);
    }

    pub fn visible(&self) -> bool {
        self.visible.get()
    }

    /// Move the view to slot `top`. The next frame clamps it and writes the clamp back.
    pub fn set_scroll(&self, top: usize) {
        self.scroll.set(top);
    }

    pub fn scroll(&self) -> usize {
        self.scroll.get()
    }

    /// Scroll by `notches` slots (positive = down the graph), clamped against the last
    /// published view (D13). Returns the new offset.
    pub fn wheel(&self, notches: i32) -> usize {
        let (total, visible) = {
            let v = self.view.borrow();
            (v.total_slots, v.visible_slots)
        };
        let top = scroll_offset(self.scroll(), notches, total, visible);
        self.set_scroll(top);
        top
    }

    /// Where the pointer is, or `None` when it left the window (D3's `Hover`).
    pub fn hover(&self, point: Option<[f32; 2]>) {
        self.hover.set(point);
    }

    pub fn hover_point(&self) -> Option<[f32; 2]> {
        self.hover.get()
    }

    /// Publish what a frame drew. The panel's own call each time stage two runs; public so
    /// a test can drive [`press`](Self::press) with hit boxes it chose.
    pub fn publish_view(&self, view: FlowView) {
        *self.view.borrow_mut() = view;
    }

    /// The last published view. `Default` until something was published.
    pub fn view(&self) -> FlowView {
        self.view.borrow().clone()
    }

    /// What a left press at `point` (window-absolute physical px) hit, or `None` — a card
    /// or a fold chip only; edges, the project card, `+N` chips and empty space are `None`
    /// (D7). Resolved against the published view alone, so the click and the pixels on
    /// screen cannot disagree.
    pub fn press(&self, point: [f32; 2]) -> Option<FlowHit> {
        let [x, y] = point;
        self.view
            .borrow()
            .hits
            .iter()
            .find(|(_, [rx, ry, w, h])| x >= *rx && x < rx + w && y >= *ry && y < ry + h)
            .map(|(hit, _)| *hit)
    }
}

/// The scroll offset after `notches` slots of wheel travel (D13/R11.1).
///
/// Clamped to `[0, total - visible]`, so the last slot stays reachable and the graph never
/// scrolls into blank space; `0` whenever everything fits. Pure.
pub fn scroll_offset(current: usize, notches: i32, total: usize, visible: usize) -> usize {
    // i128 so no `usize` (current or max) can wrap negative on the cast.
    let max = total.saturating_sub(visible) as i128;
    let moved = current as i128 + i128::from(notches);
    moved.clamp(0, max) as usize
}

/// Everything stage one depends on besides the graph (D11).
#[derive(Clone, Copy, Debug, PartialEq)]
struct LayoutKey {
    viewport: [f32; 2],
    scale: f64,
    /// `[sidebar].width_px`, logical — the card width's source (D12).
    sidebar_w: f32,
    measure: CardMeasure,
    ellipsis: char,
    /// The card atlases' texture identities: a rebuilt atlas is a new face.
    epochs: [Option<u64>; 2],
}

struct Composed {
    scroll: usize,
    hover: Option<usize>,
    frame: GraphFrame,
}

/// `(epoch, generation)` of `[grid, title, meta]` — what resolved glyph UVs point into.
type AtlasState = [Option<(u64, u64)>; 3];

/// The full-window session graph layer (D4, D15).
pub struct FlowPanel {
    handle: FlowHandle,
    shapes: Option<SidebarShapePipeline>,
    title_pipe: Option<SidebarTextPipeline>,
    meta_pipe: Option<SidebarTextPipeline>,
    fallback_pipe: Option<SidebarTextPipeline>,
    cards: CardAtlases,
    /// Stage one: the layout, keyed on the frame inputs and the handle's graph revision.
    shaped: Option<(LayoutKey, u64, GraphLayout)>,
    composed: Option<Composed>,
    title_buf: Vec<TextInstance>,
    meta_buf: Vec<TextInstance>,
    fallback_buf: Vec<TextInstance>,
    /// What the glyph instances were resolved against; `None` = resolve again.
    text_atlas: Option<AtlasState>,
    /// The target format the resident buffers were uploaded for.
    uploaded: Option<wgpu::TextureFormat>,
    /// Counters. Not `#[cfg(test)]`: R12 is asserted from `crates/renderer/tests/`, which
    /// compiles this crate without `cfg(test)`, and a count is the only observable that
    /// tells "cached" from "recomputed to the same answer".
    layouts: u64,
    composes: u64,
    submits: u64,
}

impl FlowPanel {
    /// A layer bound to the overlay's handle; the overlay keeps its clone to write.
    pub fn new(handle: FlowHandle) -> FlowPanel {
        FlowPanel {
            handle,
            shapes: None,
            title_pipe: None,
            meta_pipe: None,
            fallback_pipe: None,
            cards: CardAtlases::new(),
            shaped: None,
            composed: None,
            title_buf: Vec::new(),
            meta_buf: Vec::new(),
            fallback_buf: Vec::new(),
            text_atlas: None,
            uploaded: None,
            layouts: 0,
            composes: 0,
            submits: 0,
        }
    }

    /// How many times stage one ran (R12.2–R12.4).
    pub fn layout_count(&self) -> u64 {
        self.layouts
    }

    /// How many times stage two ran (R12.2, R12.3).
    pub fn compose_count(&self) -> u64 {
        self.composes
    }

    /// How many times the layer's own card atlases were (re)built (R12.4).
    pub fn atlas_builds(&self) -> u64 {
        self.cards.builds()
    }

    /// Frames in which the layer submitted any draw (R12.4).
    pub fn submit_count(&self) -> u64 {
        self.submits
    }

    /// Drop the published hit boxes when nothing is painted, so a press cannot hit cards
    /// that are no longer on screen. Borrow-checked first: an idle hidden panel writes
    /// nothing.
    fn clear_view(&self) {
        if self.handle.view.borrow().hits != Vec::new() {
            self.handle.publish_view(FlowView::default());
        }
    }

    fn atlas_state(&self, ctx: &FrameCtx<'_>) -> AtlasState {
        let card = |face| {
            self.cards
                .get(face)
                .map(|a: &crate::atlas::Atlas| (a.epoch(), a.generation()))
        };
        [
            Some((ctx.atlas.epoch(), ctx.atlas.generation())),
            card(CardFace::Title),
            card(CardFace::Meta),
        ]
    }
}

impl DrawLayer for FlowPanel {
    fn z_order(&self) -> i32 {
        GRAPH_PANEL_Z
    }

    fn record(&mut self, ctx: &mut FrameCtx<'_>) {
        // Both early-outs precede any GPU work, the atlas build included (R12.4).
        if !self.handle.visible() {
            self.clear_view();
            return;
        }
        let [vw, vh] = ctx.viewport;
        if !(vw > 0.0 && vh > 0.0) {
            self.clear_view();
            return;
        }
        self.cards.ensure(ctx.device, ctx.fonts, ctx.scale_factor);
        let measure = self.cards.measure(ctx.scale_factor, ctx.cell);
        let ellipsis = if ctx.atlas.glyph_id('…') != 0 {
            '…'
        } else {
            '.'
        };
        let key = LayoutKey {
            viewport: ctx.viewport,
            scale: ctx.scale_factor,
            sidebar_w: ctx.sidebar.width_px,
            measure,
            ellipsis,
            epochs: CardFace::ALL.map(|f| self.cards.epoch(f)),
        };

        // ---- stage one (D11) ---------------------------------------------------------
        let cell = self.handle.graph.clone();
        let Ok(graph) = cell.try_borrow() else {
            return;
        };
        let revision = self.handle.revision();
        let layout_stale = match self.shaped.as_ref() {
            Some((k, rev, _)) => *k != key || *rev != revision,
            None => true,
        };
        if layout_stale {
            let lay = graph_layout::layout_graph_with(
                &graph,
                key.viewport,
                key.scale,
                CardMetrics::DEFAULT,
                key.sidebar_w,
                measure,
                ellipsis,
            );
            self.shaped = Some((key, revision, lay));
            self.layouts += 1;
        }
        drop(graph);
        let Some((_, _, lay)) = self.shaped.as_ref() else {
            return;
        };

        // ---- stage two (D13) ---------------------------------------------------------
        //
        // The clamp is written back so the app's offset follows the frame (a graph that
        // shrank collapses `max_scroll`).
        let scroll = self.handle.scroll().min(lay.max_scroll());
        if scroll != self.handle.scroll() {
            self.handle.set_scroll(scroll);
        }
        let hover = self
            .handle
            .hover_point()
            .and_then(|p| lay.node_at(p, scroll));
        let compose_stale = layout_stale
            || self.composed.as_ref().map(|c| (c.scroll, c.hover)) != Some((scroll, hover));
        if compose_stale {
            let frame = graph_layout::compose(lay, scroll, hover);
            // Published on every stage-two run: a scroll moves every hit box, and a press
            // against last frame's boxes would switch to the wrong tab.
            self.handle.publish_view(FlowView {
                hits: frame.hits.clone(),
                visible_slots: lay.visible_slots,
                total_slots: lay.total_slots,
            });
            self.composed = Some(Composed {
                scroll,
                hover,
                frame,
            });
            self.composes += 1;
        }
        let Some(composed) = self.composed.as_ref() else {
            return;
        };

        // ---- glyphs: again when composed, or when an atlas they point into moved ------
        let before = self.atlas_state(ctx);
        let text_stale = compose_stale || self.text_atlas != Some(before);
        if text_stale {
            resolve_card_runs(
                &mut self.cards,
                ctx.atlas,
                ctx.queue,
                &composed.frame.runs,
                measure,
                CardGlyphs {
                    title: &mut self.title_buf,
                    meta: &mut self.meta_buf,
                    fallback: &mut self.fallback_buf,
                },
                |_| {},
            );
            // An eviction mid-resolve (card atlases resolve again inside, the grid's does
            // not) leaves this frame's state unequal to `before`: resolve once more next
            // frame rather than re-submit stale UVs.
            let after = self.atlas_state(ctx);
            self.text_atlas = (after == before).then_some(after);
        }

        // ---- submit: shapes, then title, meta and fallback text over them -------------
        let fresh = self.uploaded != Some(ctx.format);
        let shapes_full = compose_stale || fresh;
        let text_full = text_stale || fresh;
        {
            let sp = SidebarShapePipeline::ensure(&mut self.shapes, ctx);
            if shapes_full {
                sp.draw(ctx, &composed.frame.shapes);
            } else {
                sp.redraw(ctx, composed.frame.shapes.len() as u32);
            }
        }
        for (face, pipe, buf) in [
            (CardFace::Title, &mut self.title_pipe, &self.title_buf),
            (CardFace::Meta, &mut self.meta_pipe, &self.meta_buf),
        ] {
            if let Some(atlas) = self.cards.get(face) {
                let p = SidebarTextPipeline::ensure(pipe, ctx);
                if text_full {
                    p.draw(ctx, TextAtlas::Card(atlas), buf);
                } else {
                    p.redraw(ctx, buf.len() as u32);
                }
            }
        }
        if !self.fallback_buf.is_empty() {
            let p = SidebarTextPipeline::ensure(&mut self.fallback_pipe, ctx);
            if text_full {
                p.draw(ctx, TextAtlas::Grid, &self.fallback_buf);
            } else {
                p.redraw(ctx, self.fallback_buf.len() as u32);
            }
        }
        self.uploaded = Some(ctx.format);
        self.submits += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph_layout::{FlowNode, NodeKind};
    use crate::sidebar_card::CardStatus;
    use crate::sidebar_theme::AgentSkin;

    // ---- R11.1: scroll_offset ----------------------------------------------------------

    #[test]
    fn r11_1_scroll_offset_clamps_to_the_last_page_and_to_zero() {
        let down = scroll_offset(0, 100, 60, 20);
        assert_eq!(down, 40);
        assert_eq!(scroll_offset(down, -100, 60, 20), 0);
        assert_eq!(scroll_offset(0, 100, 10, 20), 0, "everything fits");
        assert_eq!(scroll_offset(3, 1, 60, 20), 4, "one notch is one slot");
        assert_eq!(scroll_offset(50, 0, 60, 20), 40, "a stale offset clamps");
        assert_eq!(
            scroll_offset(usize::MAX, i32::MAX, usize::MAX, 0),
            usize::MAX
        );
    }

    // ---- press / handle ------------------------------------------------------------------

    #[test]
    fn r9_1_press_resolves_against_the_published_hits() {
        let h = FlowHandle::default();
        h.publish_view(FlowView {
            hits: vec![
                (FlowHit::Node(7), [0.0, 0.0, 100.0, 20.0]),
                (FlowHit::Fold(7, 10), [0.0, 20.0, 100.0, 20.0]),
            ],
            visible_slots: 5,
            total_slots: 9,
        });
        assert_eq!(h.press([5.0, 5.0]), Some(FlowHit::Node(7)));
        assert_eq!(h.press([5.0, 25.0]), Some(FlowHit::Fold(7, 10)));
        assert_eq!(
            h.press([5.0, 20.0]),
            Some(FlowHit::Fold(7, 10)),
            "half-open"
        );
        assert_eq!(h.press([5.0, 40.0]), None);
        assert_eq!(h.press([100.0, 5.0]), None, "far edge is exclusive");
        assert_eq!(h.press([-0.5, 5.0]), None);
        assert_eq!(h.press([5.0, -0.5]), None);
        assert_eq!(h.view().total_slots, 9);
    }

    #[test]
    fn hit_of_target() {
        assert_eq!(FlowHit::of(FlowTarget::Node(3)), Some(FlowHit::Node(3)));
        assert_eq!(
            FlowHit::of(FlowTarget::Fold(3, 4)),
            Some(FlowHit::Fold(3, 4))
        );
        assert_eq!(FlowHit::of(FlowTarget::None), None);
    }

    #[test]
    fn handles_share_state_and_default_to_hidden() {
        let a = FlowHandle::default();
        let b = a.clone();
        assert!(!a.visible(), "the graph starts closed");
        b.set_visible(true);
        assert!(a.visible());
        b.set_scroll(9);
        assert_eq!(a.scroll(), 9);
        b.hover(Some([1.0, 2.0]));
        assert_eq!(a.hover_point(), Some([1.0, 2.0]));
        b.hover(None);
        assert_eq!(a.hover_point(), None);
        b.set(FlowGraph {
            nodes: vec![FlowNode::default()],
            resolving: true,
        });
        assert_eq!(a.graph().nodes.len(), 1);
        assert!(a.graph().resolving);
    }

    #[test]
    fn set_bumps_the_revision_only_when_the_graph_changes() {
        let h = FlowHandle::default();
        assert_eq!(h.revision(), 0);
        h.set(FlowGraph::default());
        assert_eq!(h.revision(), 0, "an equal graph is not a change");
        let one = FlowGraph {
            nodes: vec![FlowNode::default()],
            resolving: false,
        };
        h.set(one.clone());
        assert_eq!(h.revision(), 1);
        h.set(one);
        assert_eq!(
            h.revision(),
            1,
            "re-sending the same graph re-lays-out nothing"
        );
        h.set(FlowGraph::default());
        assert_eq!(h.revision(), 2);
    }

    #[test]
    fn wheel_clamps_against_the_published_view() {
        let h = FlowHandle::default();
        h.publish_view(FlowView {
            hits: vec![],
            visible_slots: 20,
            total_slots: 60,
        });
        assert_eq!(h.wheel(3), 3);
        assert_eq!(h.scroll(), 3);
        assert_eq!(h.wheel(100), 40);
        assert_eq!(h.wheel(-1), 39);
        assert_eq!(h.wheel(-100), 0);
        assert_eq!(h.scroll(), 0);
    }

    #[test]
    fn the_graph_draws_above_the_monitor() {
        let layer = FlowPanel::new(FlowHandle::default());
        assert_eq!(layer.z_order(), 110);
        const { assert!(GRAPH_PANEL_Z > crate::monitor_panel::MONITOR_PANEL_Z) };
    }

    // ---- the layer, on a real device -----------------------------------------------------

    const W: u32 = 1200;
    const H: u32 = 800;

    fn offscreen(device: &wgpu::Device) -> wgpu::TextureView {
        device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("offscreen"),
                size: wgpu::Extent3d {
                    width: W,
                    height: H,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor::default())
    }

    #[derive(Clone, Copy)]
    struct Inputs {
        viewport: [f32; 2],
        scale: f64,
        sidebar_w: f32,
    }

    const BASE: Inputs = Inputs {
        viewport: [W as f32, H as f32],
        scale: 1.0,
        sidebar_w: 280.0,
    };

    struct Rig {
        device: wgpu::Device,
        queue: wgpu::Queue,
        fonts: crate::FontSet,
        atlas: crate::atlas::Atlas,
        view: wgpu::TextureView,
        snap: term_core::GridSnapshot,
    }

    impl Rig {
        fn new() -> Option<Rig> {
            let (Some((device, queue)), Some(fonts)) =
                (crate::headless(), crate::FontSet::load().ok())
            else {
                eprintln!("skipping: no GPU/font");
                return None;
            };
            let atlas = crate::atlas::Atlas::new(&device, &fonts, 24.0).expect("atlas");
            let view = offscreen(&device);
            let t =
                term_core::Terminal::new(10, 2, term_core::Color(0xFFFFFF), term_core::Color(0));
            Some(Rig {
                snap: t.snapshot(),
                device,
                queue,
                fonts,
                atlas,
                view,
            })
        }

        fn record(&mut self, layer: &mut FlowPanel, i: Inputs) {
            self.record_with(layer, i, None);
        }

        fn record_with(
            &mut self,
            layer: &mut FlowPanel,
            i: Inputs,
            fonts: Option<&crate::FontSet>,
        ) {
            let cfg = common::config::SidebarConfig {
                width_px: i.sidebar_w,
                ..common::config::SidebarConfig::default()
            };
            let mut encoder = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
            let mut ctx = FrameCtx {
                device: &self.device,
                queue: &self.queue,
                encoder: &mut encoder,
                view: &self.view,
                viewport: i.viewport,
                cell: [12.0, 24.0],
                top_inset: 0.0,
                left_inset: 0.0,
                right_inset: 0.0,
                format: wgpu::TextureFormat::Rgba8Unorm,
                sidebar: &cfg,
                scale_factor: i.scale,
                snapshot: &self.snap,
                atlas: &mut self.atlas,
                fonts: fonts.unwrap_or(&self.fonts),
            };
            layer.record(&mut ctx);
            self.queue.submit([encoder.finish()]);
        }
    }

    fn graph(n: usize, status: CardStatus) -> FlowGraph {
        let mut nodes = vec![FlowNode {
            kind: NodeKind::Project,
            title: "terminal".into(),
            ..FlowNode::default()
        }];
        for i in 0..n {
            nodes.push(FlowNode {
                kind: NodeKind::Session,
                parent: Some(0),
                title: format!("claude {i}"),
                skin: AgentSkin::Claude,
                status: if i == 0 { status } else { CardStatus::Ready },
                target: FlowTarget::Node(i as u64 + 1),
                ..FlowNode::default()
            });
        }
        FlowGraph {
            nodes,
            resolving: false,
        }
    }

    fn shown(g: FlowGraph) -> (FlowHandle, FlowPanel) {
        let h = FlowHandle::default();
        h.set(g);
        h.set_visible(true);
        let layer = FlowPanel::new(h.clone());
        (h, layer)
    }

    #[test]
    fn hidden_builds_nothing_and_clears_the_view() {
        let Some(mut rig) = Rig::new() else { return };
        let (h, mut layer) = shown(graph(3, CardStatus::Working));
        rig.record(&mut layer, BASE);
        assert_eq!(
            (
                layer.layout_count(),
                layer.compose_count(),
                layer.submit_count()
            ),
            (1, 1, 1)
        );
        assert_eq!(layer.atlas_builds(), 1, "built on the first visible frame");
        assert!(!h.view().hits.is_empty());
        h.set_visible(false);
        rig.record(&mut layer, BASE);
        assert_eq!(layer.submit_count(), 1, "hidden: no submit");
        assert_eq!(h.view(), FlowView::default(), "and no hits left to press");
        // A zero viewport is the same as hidden.
        h.set_visible(true);
        rig.record(&mut layer, BASE);
        assert_eq!(layer.submit_count(), 2);
        for viewport in [[0.0, 800.0], [1200.0, 0.0]] {
            rig.record(&mut layer, Inputs { viewport, ..BASE });
            assert_eq!(layer.submit_count(), 2, "{viewport:?}");
            assert!(h.view().hits.is_empty());
        }
    }

    #[test]
    fn idle_frames_resubmit_without_rebuilding() {
        let Some(mut rig) = Rig::new() else { return };
        let (h, mut layer) = shown(graph(3, CardStatus::Working));
        rig.record(&mut layer, BASE);
        let title = layer.title_buf.clone();
        assert!(
            !title.is_empty() && !layer.meta_buf.is_empty(),
            "text resolved"
        );
        assert!(
            layer.fallback_buf.is_empty(),
            "card atlases built: no fallback"
        );
        for _ in 0..5 {
            rig.record(&mut layer, BASE);
        }
        assert_eq!((layer.layout_count(), layer.compose_count()), (1, 1));
        assert_eq!(layer.submit_count(), 6, "every visible frame submits");
        assert_eq!(layer.title_buf, title);
        let v = h.view();
        assert_eq!((v.total_slots, v.visible_slots), (3, 6));
        assert_eq!(v.hits.len(), 3);
    }

    #[test]
    fn each_layout_input_alone_re_lays_out() {
        let Some(mut rig) = Rig::new() else { return };
        let (h, mut layer) = shown(graph(3, CardStatus::Working));
        rig.record(&mut layer, BASE);
        let mut want = 1;
        for i in [
            Inputs {
                viewport: [1000.0, 800.0],
                ..BASE
            },
            Inputs {
                viewport: [1000.0, 700.0],
                ..BASE
            },
            Inputs {
                viewport: [1000.0, 700.0],
                scale: 2.0,
                ..BASE
            },
            Inputs {
                viewport: [1000.0, 700.0],
                scale: 2.0,
                sidebar_w: 300.0,
            },
        ] {
            rig.record(&mut layer, i);
            want += 1;
            assert_eq!(layer.layout_count(), want);
            assert_eq!(layer.compose_count(), want, "a layout recomposes");
            rig.record(&mut layer, i);
            assert_eq!(layer.layout_count(), want, "and is cached after");
        }
        // The graph itself.
        h.set(graph(3, CardStatus::WaitingForInput));
        rig.record(&mut layer, BASE);
        assert_eq!(layer.layout_count(), want + 1);
        // A new font set rebuilds the card atlases at the same scale: a new identity.
        let fonts = crate::FontSet::load().expect("fonts");
        rig.record_with(&mut layer, BASE, Some(&fonts));
        assert_eq!(
            layer.layout_count(),
            want + 2,
            "atlas identity is in the key"
        );
        assert_eq!(layer.atlas_builds(), 4);
    }

    #[test]
    fn scroll_and_hover_recompose_only_and_the_clamp_is_written_back() {
        let Some(mut rig) = Rig::new() else { return };
        let (h, mut layer) = shown(graph(20, CardStatus::Working));
        rig.record(&mut layer, BASE);
        let before = h.view();
        h.set_scroll(2);
        rig.record(&mut layer, BASE);
        assert_eq!((layer.layout_count(), layer.compose_count()), (1, 2));
        assert_ne!(
            h.view().hits,
            before.hits,
            "the hit boxes move with the scroll"
        );
        // A hover onto a session card recomposes; moving within it does not.
        let r = h.view().hits[0].1;
        h.hover(Some([r[0] + 10.0, r[1] + 10.0]));
        rig.record(&mut layer, BASE);
        assert_eq!(layer.compose_count(), 3);
        h.hover(Some([r[0] + 20.0, r[1] + 20.0]));
        rig.record(&mut layer, BASE);
        assert_eq!(layer.compose_count(), 3, "same card, same frame");
        h.hover(Some([1.0, 1.0]));
        rig.record(&mut layer, BASE);
        assert_eq!(layer.compose_count(), 4, "off the card");
        h.hover(None);
        rig.record(&mut layer, BASE);
        assert_eq!(
            layer.compose_count(),
            4,
            "None and empty space are both no hover"
        );
        // Past the end: clamped and written back.
        h.set_scroll(10_000);
        rig.record(&mut layer, BASE);
        let max = 20 - h.view().visible_slots;
        assert_eq!(h.scroll(), max);
        assert_eq!(layer.layout_count(), 1);
    }

    #[test]
    fn an_atlas_change_re_resolves_glyphs_without_recomposing() {
        let Some(mut rig) = Rig::new() else { return };
        let (_h, mut layer) = shown(graph(2, CardStatus::Working));
        rig.record(&mut layer, BASE);
        let state = layer.text_atlas;
        assert!(state.is_some(), "a clean resolve records its atlases");
        rig.record(&mut layer, BASE);
        assert_eq!(layer.text_atlas, state);
        // A grid-atlas swap moves state[0] only.
        rig.atlas = crate::atlas::Atlas::new(&rig.device, &rig.fonts, 24.0).expect("atlas");
        rig.record(&mut layer, BASE);
        assert_eq!(
            layer.compose_count(),
            1,
            "the atlas is not in stage two's key"
        );
        assert_ne!(layer.text_atlas, state, "the glyphs were resolved again");
        // A card-atlas eviction too.
        let s2 = layer.text_atlas;
        layer
            .cards
            .get_mut(CardFace::Title)
            .expect("title atlas")
            .evict_for_test();
        rig.record(&mut layer, BASE);
        assert_ne!(layer.text_atlas, s2);
        assert_eq!(layer.compose_count(), 1);
    }

    /// `[shapes, title, meta, fallback]` buffer writes so far.
    fn uploads(layer: &FlowPanel) -> [u32; 4] {
        let text = |p: &Option<SidebarTextPipeline>| p.as_ref().map_or(0, |p| p.uploads);
        [
            layer.shapes.as_ref().map_or(0, |p| p.uploads),
            text(&layer.title_pipe),
            text(&layer.meta_pipe),
            text(&layer.fallback_pipe),
        ]
    }

    #[test]
    fn a_steady_frame_writes_no_buffer_and_each_change_writes_only_its_own() {
        let Some(mut rig) = Rig::new() else { return };
        let (h, mut layer) = shown(graph(20, CardStatus::Working));
        rig.record(&mut layer, BASE);
        assert_eq!(
            uploads(&layer),
            [1, 1, 1, 0],
            "no fallback pipe while atlases build"
        );
        assert!(layer.fallback_pipe.is_none());
        for _ in 0..3 {
            rig.record(&mut layer, BASE);
        }
        assert_eq!(
            uploads(&layer),
            [1, 1, 1, 0],
            "steady: re-submitted, not rewritten"
        );
        // A scroll recomposes: shapes and text are written again.
        h.set_scroll(2);
        rig.record(&mut layer, BASE);
        assert_eq!(uploads(&layer), [2, 2, 2, 0]);
        // An atlas move alone re-resolves the text, not the shapes.
        layer
            .cards
            .get_mut(CardFace::Meta)
            .expect("meta atlas")
            .evict_for_test();
        rig.record(&mut layer, BASE);
        assert_eq!(uploads(&layer), [2, 3, 3, 0]);
        // A new target format rebuilds the pipelines, so everything is written once.
        layer.uploaded = Some(wgpu::TextureFormat::Bgra8Unorm);
        rig.record(&mut layer, BASE);
        assert_eq!(uploads(&layer), [3, 4, 4, 0]);
    }

    #[test]
    fn the_ellipsis_is_the_fonts_when_it_has_one() {
        let Some(mut rig) = Rig::new() else { return };
        assert_ne!(
            rig.atlas.glyph_id('…'),
            0,
            "premise: the font has an ellipsis"
        );
        let (_h, mut layer) = shown(graph(1, CardStatus::Working));
        rig.record(&mut layer, BASE);
        let (key, _, lay) = layer.shaped.as_ref().expect("laid out");
        assert_eq!((key.ellipsis, lay.ellipsis), ('…', '…'));
    }

    #[test]
    fn a_failed_card_atlas_falls_back_to_the_grid() {
        let Some(mut rig) = Rig::new() else { return };
        let (_h, mut layer) = shown(graph(2, CardStatus::Working));
        let broken = crate::sidebar_text::broken_fonts();
        rig.record_with(&mut layer, BASE, Some(&broken));
        assert!(layer.title_buf.is_empty() && layer.meta_buf.is_empty());
        assert!(!layer.fallback_buf.is_empty(), "drawn from the grid atlas");
        assert_eq!(uploads(&layer), [1, 0, 0, 1], "through the fallback pipe");
        let (key, _, _) = layer.shaped.as_ref().expect("laid out");
        assert_eq!(key.measure.title_col, 12.0, "measured by the grid cell");
        rig.record_with(&mut layer, BASE, Some(&broken));
        assert_eq!(layer.submit_count(), 2);
    }
}
