//! The Agent Monitor overview as a registered [`DrawLayer`] (session-overview-ui D1, D18,
//! D21): a full-window layer at [`OVERVIEW_PANEL_Z`] drawn over the terminal, the sidebar
//! and an open ai-monitor, reserving no inset.
//!
//! **Two stages**, patterned on the retired session graph's panel. Stage one runs
//! [`layout_overview`](crate::overview_layout::layout_overview) /
//! [`layout_detail`](crate::overview_layout::layout_detail) when the model revision, the
//! viewport, the scale, the panel's atlases' identity or the scroll offset changed. Stage
//! two (compose) runs when stage one did or when the hovered hit changed: it adds the hover
//! highlight, publishes the hits and `max_scroll` to the handle, and resolves the glyphs.
//! An unchanged frame re-submits the resident buffers and writes none (G1, R10.5) — the
//! window redraws at 60 fps while visible (architecture C6).
//!
//! **Text** is drawn from the layer's own three atlases, one per [`OverviewFace`] (Title
//! and Meta at the card faces' ppems, Display at 30 logical px), built on the first visible
//! frame and never at startup, through the sidebar's text pipeline. A face whose atlas
//! could not be built is drawn from the grid atlas instead, as the sidebar's cards are.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use crate::atlas::Atlas;
use crate::font_set::FontSet;
use crate::overview_layout::{self, LaidText, OverviewFace, OverviewLayout};
use crate::overview_model::{OverviewHit, OverviewModel, View};
use crate::overview_theme as th;
use crate::sidebar_row::{SidebarTextPipeline, TextAtlas, TextInstance};
use crate::sidebar_shapes::{Fill, SidebarShape, SidebarShapePipeline};
use crate::{DrawLayer, FrameCtx};

/// Above the ai-monitor panel (100), as the session graph was (carried D4).
pub const OVERVIEW_PANEL_Z: i32 = 110;

#[derive(Default)]
struct Shared {
    model: OverviewModel,
    /// Bumped by [`OverviewHandle::set`] only when the model changed.
    rev: u64,
    visible: bool,
    /// The list's whole-line scroll offset (the table, or the detail's lanes).
    scroll: usize,
    hover: Option<[f32; 2]>,
    /// What the last composed frame drew: `(hit, [x, y, w, h])`, physical px.
    hits: Vec<(OverviewHit, [f32; 4])>,
    /// `max_scroll` of the last layout.
    max_scroll: usize,
}

/// What the app and the layer share. Cheap to clone.
#[derive(Clone, Default)]
pub struct OverviewHandle {
    inner: Rc<RefCell<Shared>>,
}

impl OverviewHandle {
    pub fn new() -> OverviewHandle {
        OverviewHandle::default()
    }

    /// Replace the model; the layer re-lays-out only if it differs.
    pub fn set(&self, model: OverviewModel) {
        let mut s = self.inner.borrow_mut();
        if s.model != model {
            s.model = model;
            s.rev += 1;
        }
    }

    pub fn model(&self) -> OverviewModel {
        self.inner.borrow().model.clone()
    }

    pub fn rev(&self) -> u64 {
        self.inner.borrow().rev
    }

    pub fn set_visible(&self, visible: bool) {
        self.inner.borrow_mut().visible = visible;
    }

    pub fn visible(&self) -> bool {
        self.inner.borrow().visible
    }

    /// Set the list scroll, clamped to the last layout's `max_scroll`.
    pub fn set_scroll(&self, lines: usize) {
        let mut s = self.inner.borrow_mut();
        s.scroll = lines.min(s.max_scroll);
    }

    pub fn scroll(&self) -> usize {
        self.inner.borrow().scroll
    }

    /// Set the list scroll **without** clamping it to the last layout's bound — for a view
    /// change, where the bound on hand is the other view's. The layer lays out with it and
    /// its next publish clamps it against the bound of the view actually shown.
    pub fn restore_scroll(&self, lines: usize) {
        self.inner.borrow_mut().scroll = lines;
    }

    pub fn set_hover(&self, point: Option<[f32; 2]>) {
        self.inner.borrow_mut().hover = point;
    }

    /// The pointer position the layer resolves its hover highlight from.
    pub fn hover(&self) -> Option<[f32; 2]> {
        self.inner.borrow().hover
    }

    /// The hits the last composed frame published.
    pub fn hits(&self) -> Vec<(OverviewHit, [f32; 4])> {
        self.inner.borrow().hits.clone()
    }

    /// The hit under `point` in the last composed frame.
    pub fn hit_at(&self, point: [f32; 2]) -> Option<OverviewHit> {
        let [px, py] = point;
        self.inner
            .borrow()
            .hits
            .iter()
            .rev()
            .find_map(|(hit, [x, y, w, h])| {
                (px >= *x && px < x + w && py >= *y && py < y + h).then_some(*hit)
            })
    }

    /// Publish a layout's hits and scroll bound — the layer's call; public so a headless
    /// test can stand in for a frame.
    pub fn publish(&self, hits: Vec<(OverviewHit, [f32; 4])>, max_scroll: usize) {
        let mut s = self.inner.borrow_mut();
        s.hits = hits;
        s.max_scroll = max_scroll;
        s.scroll = s.scroll.min(max_scroll);
    }

    /// `max_scroll` of the last published layout.
    pub fn max_scroll(&self) -> usize {
        self.inner.borrow().max_scroll
    }
}

/// The identity of a font set: each family's regular face, by the address of its mapping
/// and its face index (as `sidebar_text` keys its card atlases).
fn font_identity(fonts: &FontSet) -> Vec<(usize, usize)> {
    fonts
        .families()
        .iter()
        .map(|family| {
            let face = family.regular();
            (Arc::as_ptr(&face.data) as usize, face.index)
        })
        .collect()
}

/// The panel's three atlases, one per [`OverviewFace`], and what they were built for. A
/// failed build is remembered as attempted and not retried until the scale or the font
/// set changes (the sidebar's rule, R2.6 there).
#[derive(Default)]
struct FaceAtlases {
    atlases: [Option<Atlas>; 3],
    built_scale: Option<f64>,
    built_fonts: Vec<(usize, usize)>,
    builds: u64,
}

impl FaceAtlases {
    fn ensure(&mut self, device: &wgpu::Device, fonts: &FontSet, scale: f64) {
        let ident = font_identity(fonts);
        if self.built_scale == Some(scale) && self.built_fonts == ident {
            return;
        }
        for face in OverviewFace::ALL {
            let ppem = face.ppem(scale);
            let atlas = Atlas::new(device, fonts, ppem);
            if atlas.is_none() {
                tracing::warn!(
                    ?face,
                    ppem,
                    "overview atlas could not be built; drawing that text from the grid atlas"
                );
            }
            self.atlases[face.index()] = atlas;
        }
        self.built_scale = Some(scale);
        self.built_fonts = ident;
        self.builds += 1;
    }

    fn get(&self, face: OverviewFace) -> Option<&Atlas> {
        self.atlases[face.index()].as_ref()
    }

    /// `(epoch, generation)` per face — what resolved UVs point into.
    fn state(&self) -> [Option<(u64, u64)>; 3] {
        OverviewFace::ALL.map(|f| self.get(f).map(|a| (a.epoch(), a.generation())))
    }

    fn epochs(&self) -> [Option<u64>; 3] {
        OverviewFace::ALL.map(|f| self.get(f).map(Atlas::epoch))
    }
}

/// Everything stage one depends on besides the model.
#[derive(Clone, Copy, Debug, PartialEq)]
struct LayoutKey {
    viewport: [f32; 2],
    scale: f64,
    epochs: [Option<u64>; 3],
    scroll: usize,
    rev: u64,
}

/// What stage two drew.
struct Composed {
    hover: Option<(OverviewHit, [f32; 4])>,
    shapes: Vec<SidebarShape>,
}

/// `[grid, title, meta, display]` atlas states the glyph buffers were resolved against.
type AtlasState = [Option<(u64, u64)>; 4];

/// The draw layer.
pub struct OverviewPanel {
    handle: OverviewHandle,
    composes: u64,
    layouts: u64,
    ppems: Vec<(OverviewFace, f32)>,
    atlases: FaceAtlases,
    shaped: Option<(LayoutKey, OverviewLayout)>,
    composed: Option<Composed>,
    shapes_pipe: Option<SidebarShapePipeline>,
    /// One text pipeline per face, then the grid-atlas fallback.
    text_pipes: [Option<SidebarTextPipeline>; 4],
    text_bufs: [Vec<TextInstance>; 4],
    text_atlas: Option<AtlasState>,
    uploaded: Option<wgpu::TextureFormat>,
}

impl OverviewPanel {
    pub fn new(handle: OverviewHandle) -> OverviewPanel {
        OverviewPanel {
            handle,
            composes: 0,
            layouts: 0,
            ppems: Vec::new(),
            atlases: FaceAtlases::default(),
            shaped: None,
            composed: None,
            shapes_pipe: None,
            text_pipes: [None, None, None, None],
            text_bufs: [Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            text_atlas: None,
            uploaded: None,
        }
    }

    /// How many times stage two (compose) ran — an unchanged frame does not advance it
    /// (R10.5).
    pub fn compose_count(&self) -> u64 {
        self.composes
    }

    /// How many times stage one (layout) ran.
    pub fn layout_count(&self) -> u64 {
        self.layouts
    }

    /// How many times the panel's own atlases were (re)built.
    pub fn atlas_builds(&self) -> u64 {
        self.atlases.builds
    }

    /// The faces and ppems of the atlases currently built — empty until the first visible
    /// frame (R9.6).
    pub fn atlas_ppems(&self) -> Vec<(OverviewFace, f32)> {
        self.ppems.clone()
    }

    pub fn handle(&self) -> &OverviewHandle {
        &self.handle
    }

    /// Drop the published hits when nothing is painted, so a press cannot land on rows no
    /// longer on screen. Checked first: an idle hidden panel writes nothing.
    fn clear_hits(&self) {
        let Ok(mut s) = self.handle.inner.try_borrow_mut() else {
            return;
        };
        if !s.hits.is_empty() {
            s.hits.clear();
        }
    }

    fn atlas_state(&self, ctx: &FrameCtx<'_>) -> AtlasState {
        let [a, b, c] = self.atlases.state();
        [Some((ctx.atlas.epoch(), ctx.atlas.generation())), a, b, c]
    }

    /// Resolve every run into its face's buffer (or the fallback's), twice at most: an
    /// eviction mid-pass repacks a texture under UVs already resolved this pass.
    fn resolve_text(&mut self, ctx: &mut FrameCtx<'_>, texts: &[LaidText], scale: f64) {
        for _ in 0..2 {
            for b in &mut self.text_bufs {
                b.clear();
            }
            let before = self.atlases.state();
            for t in texts {
                let col = t.face.column_px(scale);
                let line = t.face.line_px(scale);
                let baseline = t.rect[1] + line * 0.8;
                let color = th::to_linear(t.color);
                let (atlas, buf) = match self.atlases.atlases[t.face.index()].as_mut() {
                    Some(a) => (a, &mut self.text_bufs[t.face.index()]),
                    None => (&mut *ctx.atlas, &mut self.text_bufs[3]),
                };
                crate::tab_bar::layout_line_in(
                    atlas,
                    ctx.queue,
                    &t.text,
                    t.rect[0],
                    col,
                    t.bold,
                    |pen, e| {
                        buf.push(TextInstance {
                            rect: [pen + e.left, baseline - e.top, e.px_w, e.px_h],
                            uv0: e.uv0,
                            uv1: e.uv1,
                            color,
                        });
                    },
                );
            }
            if self.atlases.state() == before {
                break;
            }
        }
    }
}

impl DrawLayer for OverviewPanel {
    fn z_order(&self) -> i32 {
        OVERVIEW_PANEL_Z
    }

    fn record(&mut self, ctx: &mut FrameCtx<'_>) {
        let (visible, rev, scroll, hover_point) = match self.handle.inner.try_borrow() {
            Ok(s) => (s.visible, s.rev, s.scroll, s.hover),
            Err(_) => return,
        };
        // Both early-outs precede any GPU work, the atlas build included.
        if !visible {
            self.clear_hits();
            return;
        }
        let [vw, vh] = ctx.viewport;
        if !(vw > 0.0 && vh > 0.0) {
            self.clear_hits();
            return;
        }
        let scale = ctx.scale_factor;
        self.atlases.ensure(ctx.device, ctx.fonts, scale);
        self.ppems = OverviewFace::ALL
            .into_iter()
            .filter(|f| self.atlases.get(*f).is_some())
            .map(|f| (f, f.ppem(scale)))
            .collect();

        // ---- stage one: layout ---------------------------------------------------------
        let key = LayoutKey {
            viewport: ctx.viewport,
            scale,
            epochs: self.atlases.epochs(),
            scroll,
            rev,
        };
        let layout_stale = self.shaped.as_ref().map(|(k, _)| *k) != Some(key);
        if layout_stale {
            let lay = {
                let Ok(s) = self.handle.inner.try_borrow() else {
                    return;
                };
                match &s.model.view {
                    View::Overview => {
                        overview_layout::layout_overview(&s.model, ctx.viewport, scale, scroll)
                    }
                    View::Detail(d) => {
                        overview_layout::layout_detail(d, ctx.viewport, scale, scroll)
                    }
                }
            };
            self.layouts += 1;
            self.shaped = Some((key, lay));
        }
        let Some((_, lay)) = self.shaped.as_ref() else {
            return;
        };

        // ---- stage two: compose --------------------------------------------------------
        let hover = hover_point.and_then(|p| lay.hit_rect_at(p));
        let compose_stale = layout_stale || self.composed.as_ref().map(|c| c.hover) != Some(hover);
        if compose_stale {
            let mut shapes = lay.shapes.clone();
            if let Some((_, r)) = hover {
                if let Some(sh) = SidebarShape::rounded_box(
                    r,
                    6.0 * scale as f32,
                    0.0,
                    Fill::solid([1.0, 1.0, 1.0, 0.04]),
                    [0.0; 4],
                ) {
                    shapes.push(sh);
                }
            }
            // Published on every compose: a scroll moves every hit box.
            self.handle.publish(lay.hits.clone(), lay.max_scroll());
            // The publish may have clamped the scroll; key on what it left, so the clamp
            // does not cost a second layout of the same lines.
            let clamped = self.handle.scroll();
            if let Some((k, _)) = self.shaped.as_mut() {
                k.scroll = clamped;
            }
            self.composed = Some(Composed { hover, shapes });
            self.composes += 1;
        }

        // ---- glyphs: again when composed, or when an atlas they point into moved --------
        let before = self.atlas_state(ctx);
        let text_stale = compose_stale || self.text_atlas != Some(before);
        if text_stale {
            let texts = match self.shaped.as_ref() {
                Some((_, lay)) => lay.texts.clone(),
                None => return,
            };
            self.resolve_text(ctx, &texts, scale);
            let after = self.atlas_state(ctx);
            self.text_atlas = (after == before).then_some(after);
        }

        // ---- submit: shapes, then each face's text over them -----------------------------
        let Some(composed) = self.composed.as_ref() else {
            return;
        };
        let fresh = self.uploaded != Some(ctx.format);
        let shapes_full = compose_stale || fresh;
        let text_full = text_stale || fresh;
        {
            let sp = SidebarShapePipeline::ensure(&mut self.shapes_pipe, ctx);
            if shapes_full {
                sp.draw(ctx, &composed.shapes);
            } else {
                sp.redraw(ctx, composed.shapes.len() as u32);
            }
        }
        for i in 0..4 {
            let buf = &self.text_bufs[i];
            if buf.is_empty() && self.text_pipes[i].is_none() {
                continue;
            }
            let source = if i < 3 {
                match self.atlases.atlases[i].as_ref() {
                    Some(a) => TextAtlas::Card(a),
                    None => continue,
                }
            } else {
                TextAtlas::Grid
            };
            let p = SidebarTextPipeline::ensure(&mut self.text_pipes[i], ctx);
            if text_full {
                p.draw(ctx, source, buf);
            } else {
                p.redraw(ctx, buf.len() as u32);
            }
        }
        self.uploaded = Some(ctx.format);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handle_clamps_scroll_to_the_published_bound() {
        let h = OverviewHandle::new();
        h.publish(vec![(OverviewHit::Back, [0.0, 0.0, 10.0, 10.0])], 4);
        h.set_scroll(9);
        assert_eq!(h.scroll(), 4);
        h.publish(Vec::new(), 2);
        assert_eq!(h.scroll(), 2);
        assert_eq!(h.max_scroll(), 2);
        assert_eq!(h.hit_at([1.0, 1.0]), None);
    }

    #[test]
    fn set_bumps_rev_only_on_change() {
        let h = OverviewHandle::new();
        h.set(OverviewModel::default());
        assert_eq!(h.rev(), 0);
        h.set(OverviewModel {
            resolving: true,
            ..OverviewModel::default()
        });
        assert_eq!(h.rev(), 1);
    }

    #[test]
    fn the_overview_draws_above_the_monitor() {
        let layer = OverviewPanel::new(OverviewHandle::new());
        assert_eq!(layer.z_order(), 110);
        const { assert!(OVERVIEW_PANEL_Z > crate::monitor_panel::MONITOR_PANEL_Z) };
    }
}
