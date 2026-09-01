//! The docked ai-monitor panel as a registered [`DrawLayer`] (D14) — the layer that
//! retires `overlay_layer.rs`.
//!
//! The app pushes a [`MonitorModel`] into a shared handle whenever its data changes;
//! this layer paints the band [`FrameCtx::right_inset`] reserved on the **right** edge
//! of the window, below `top_inset`. Renderer-owned and app-state-free: it reads plain
//! data the app hands down and nothing else.
//!
//! **Three things moved here from the overlay it replaces, and they are the point:**
//!
//! * **The band comes from the inset, not from a fraction of the window.** The retiring
//!   overlay computed `[0, vh*0.45, vw, vh-top]` — the lower 55% at full width, painted
//!   *over* the grid. A docked panel must paint exactly the columns `grid_dims` already
//!   subtracted (R4.7), so the rect is derived from `ctx.right_inset` and a zero inset
//!   draws nothing at all. D4 can clamp that inset to zero on a narrow window, so
//!   "visible model, no band" is a normal state, not a bug.
//! * **Layout is not done here.** [`crate::monitor_layout`] owns every pixel; this file
//!   hands its shapes to [`SidebarShapePipeline`] and its runs to `tab_bar::layout_line`.
//!   Splitting them is what keeps the whole panel — including D18's height allocator —
//!   testable without a GPU.
//! * **The atlas bind-group discipline is carried over verbatim.** It encodes a bug that
//!   already shipped once: a live config reload swaps the whole `Atlas` without changing
//!   the surface format, so a format-gated bind group keeps sampling the *replaced*
//!   texture with UVs computed against the new one. See [`MonitorPanel::ensure_text_bind`]
//!   and the two regression tests at the bottom of this file.
//!
//! **And one thing that could not be carried over, because it did not exist:** the
//! shaping cache (R7.3). `OverlayLayer::record` re-laid-out its text on every single
//! frame, and its idle guarantee covered bind groups only — its own scope note said so.
//! A docked panel cannot inherit that: it is ten regions instead of a line list, and
//! `drive_monitor_resize` calls `redraw()` on every `CursorMoved`. See [`Shaped`].

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use crate::monitor_layout;
use crate::monitor_model::MonitorModel;
use crate::monitor_scroll;
use crate::sidebar_shapes::SidebarShapePipeline;
use crate::sidebar_theme::linear;
use crate::tab_bar::layout_line;
use crate::{DrawLayer, FrameCtx};

/// A cloneable write handle the app keeps to push new models (the shape
/// `OverlayHandle` had, so the `Overlay::layer` seam is unchanged).
///
/// `Rc<RefCell<..>>`, not a channel: the app writes and the layer reads on the one
/// winit thread, and the two never interleave — `set` runs from key/poll handling,
/// `record` from the redraw. [`MonitorPanel::record`] still reads it through
/// `try_borrow` rather than `borrow`, so if that ever stops being true the frame is
/// skipped instead of the process aborting.
#[derive(Clone, Default)]
pub struct MonitorHandle {
    model: Rc<RefCell<MonitorModel>>,
    /// The scroll position, held **beside** the model rather than in it (D26/C4).
    ///
    /// This separation is the single most load-bearing decision in the feature, and it is
    /// counter-intuitive: `MonitorModel` derives `PartialEq` and is stage one's cache key, so
    /// putting the offset in there would make cache invalidation free. It would also make
    /// every wheel notch invalidate the *whole document layout* — reviving the per-frame
    /// re-layout debt `architecture.md` G1 records as paid off, on a document D4 just made
    /// much longer. Out here, a scroll changes stage two's key and leaves stage one's alone.
    ///
    /// The mirror-image failure is just as quiet: leave it out of stage **two's** key as
    /// well and the panel hits a stale cache and never repaints. Both halves or neither.
    scroll: Rc<Cell<ScrollState>>,
    /// What the last laid-out frame implies about scrolling — published for the app, which
    /// owns the wheel and the jumps but has no business computing panel geometry (SD2).
    view: Rc<RefCell<ScrollView>>,
}

/// The scrolling facts of the last laid-out frame, published to the app.
///
/// The app owns the *policy* — which gestures the panel takes, where a jump goes — and the
/// renderer owns the *geometry*. Without this the app would have to rebuild the ladder from
/// its own copy of the model and guess a viewport height, which is two ways to disagree with
/// what is actually on screen.
#[derive(Clone, Debug)]
pub struct ScrollView {
    pub ladder: Vec<monitor_layout::Item>,
    pub heights: Vec<f32>,
    pub viewport_h: f32,
    /// The content band, **window-absolute**. Published so the app can resolve a press
    /// without doing geometry of its own — the same division `on_wheel` already keeps.
    pub viewport: crate::sidebar_layout::Rect,
    /// The sticky slot, window-absolute. A press here means the heading the slot is
    /// currently showing, which is a different rung from anything inside `viewport`.
    pub slot: crate::sidebar_layout::Rect,
}

impl Default for ScrollView {
    /// Before the panel has laid anything out. `Rect` is deliberately not `Default` — a zero
    /// rect is a real geometric claim in `sidebar_layout` — so "no band yet" is spelled with
    /// the name the layout already gives it.
    fn default() -> ScrollView {
        ScrollView {
            ladder: Vec::new(),
            heights: Vec::new(),
            viewport_h: 0.0,
            viewport: monitor_layout::NO_BAND,
            slot: monitor_layout::NO_BAND,
        }
    }
}

/// Where the view sits. Stage two's cache key.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScrollState {
    /// Ladder index of the first rung below the sticky slot (D17/D31).
    pub top: usize,
    /// Entries that arrived **above** the anchor and have not been seen (D11).
    ///
    /// Rides in the scroll state rather than in `MonitorModel` for the reason the offset
    /// does (D26/C4): it changes when the view changes, and stage one must not notice. The
    /// app counts it — only the app holds the previous document — and stage two renders it.
    pub badge: u32,
}

/// What the app needs to bound a scroll it is about to apply.
///
/// Published by the renderer because `max_index` needs the viewport height, which only the
/// layout knows — the app has the model and could rebuild the ladder, but not the band.
/// Same shape as the sidebar's `SidebarHandle::layout()` publish, for the same reason.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScrollMetrics {
    /// Rungs in the ladder the last frame was composed from.
    pub items: usize,
    /// The furthest `top` may go (D22).
    pub max_index: usize,
}

impl MonitorHandle {
    pub fn set(&self, model: MonitorModel) {
        *self.model.borrow_mut() = model;
    }

    /// Move the view. Does **not** touch the model, so stage one stays cached (C4/R10.1).
    pub fn set_scroll(&self, scroll: ScrollState) {
        self.scroll.set(scroll);
    }

    pub fn scroll(&self) -> ScrollState {
        self.scroll.get()
    }

    /// Publish the scrolling facts of a freshly laid-out frame. The panel's own call; named
    /// rather than done through a field so the direction of the flow — renderer writes, app
    /// reads — is visible at both ends.
    pub fn publish_view(&self, view: ScrollView) {
        *self.view.borrow_mut() = view;
    }

    /// What the last laid-out frame implies. `Default` until the panel has drawn once.
    pub fn metrics(&self) -> ScrollMetrics {
        let v = self.view.borrow();
        ScrollMetrics {
            items: v.ladder.len(),
            max_index: monitor_scroll::max_index(&v.ladder, &v.heights, v.viewport_h),
        }
    }

    /// Where section `n`'s heading sits in the current document (D16), clamped so a jump
    /// cannot land past the end. `None` before the panel has laid anything out.
    pub fn jump_target(&self, section: usize) -> Option<usize> {
        let v = self.view.borrow();
        let target = monitor_scroll::jump_target(&v.ladder, section)?;
        Some(monitor_scroll::clamp(
            &v.ladder,
            &v.heights,
            v.viewport_h,
            target,
        ))
    }

    /// The ladder rung at `i` in the current document, for a caller that needs to check what
    /// an index actually names rather than trusting the call that produced it.
    pub fn rung_at(&self, i: usize) -> Option<monitor_layout::Item> {
        self.view.borrow().ladder.get(i).copied()
    }

    /// A copy of the published view — ladder, heights and both bands.
    ///
    /// Clones, so it is not for the hot path: `metrics`, `press` and `wheel_px` all answer
    /// their own question without handing the vectors out. This exists for a caller that
    /// needs the whole picture, which in practice means republishing it with one field
    /// changed.
    pub fn view(&self) -> ScrollView {
        self.view.borrow().clone()
    }

    /// Which section a left press at `point` asks to fold or unfold, or `None`.
    ///
    /// Only two rungs answer: a section's **heading** and its **fold control**. A press on a
    /// row does nothing — rows are content, and giving them a meaning would mean every
    /// stray click in the panel changed the document.
    ///
    /// The sticky slot answers too, for the heading it is showing. It has to: once a section
    /// is scrolled into, its heading is *only* in the slot, and a control you can reach by
    /// scrolling away from it but not by looking at it is not a control.
    pub fn press(&self, point: [f32; 2]) -> Option<usize> {
        let v = self.view.borrow();
        let top = self.scroll().top;
        let [x, y] = point;
        let inside = |r: &crate::sidebar_layout::Rect| {
            r.h > 0.0 && x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h
        };
        let i = if inside(&v.slot) {
            monitor_scroll::heading_above(&v.ladder, top)?
        } else if inside(&v.viewport) {
            monitor_scroll::hit(&v.ladder, &v.heights, v.viewport_h, top, y - v.viewport.y)?
        } else {
            return None;
        };
        let item = v.ladder.get(i)?;
        match item.kind {
            monitor_layout::ItemKind::Heading | monitor_layout::ItemKind::Fold => {
                Some(item.section)
            }
            _ => None,
        }
    }

    /// Apply `px` of wheel travel, carrying the sub-rung remainder in `residue`.
    ///
    /// Returns whether the view moved — which is **two different facts**, and a caller that
    /// reads it as one gets D9 wrong:
    ///
    /// * the panel is at the edge in this direction, so the event belongs to the grid; or
    /// * the gesture is still **banking** below one rung, so the event is the panel's and
    ///   there is simply nothing to draw yet.
    ///
    /// `residue` tells them apart — `scroll_by_px` zeroes it at an edge and keeps it while
    /// banking — and `AiMonitorOverlay::on_wheel` is where that distinction is made. Reading
    /// `false` as "edge" alone sends every slow trackpad event to the terminal.
    ///
    /// Because the edge answer depends on the *direction* of travel and not merely on sitting
    /// at an edge, scrolling back the other way is still the panel's (C12).
    ///
    /// Positive `px` is toward the top of the document, matching winit and `wheel_lines`.
    pub fn wheel_px(&self, px: f32, residue: &mut f32) -> bool {
        let v = self.view.borrow();
        let before = self.scroll();
        let (top, left) = monitor_scroll::scroll_by_px(
            &v.ladder,
            &v.heights,
            v.viewport_h,
            before.top,
            px,
            *residue,
        );
        *residue = left;
        if top == before.top {
            return false;
        }
        drop(v);
        self.set_scroll(ScrollState { top, ..before });
        true
    }
}

/// z-order: above the tab bar (10) and the image layer (20) — the panel is chrome
/// drawn last, over everything the terminal owns.
pub const MONITOR_PANEL_Z: i32 = 100;

/// Baseline offset as a fraction of the cell height. The same constant the tab bar and
/// the retiring overlay draw with, so a glyph sits on the same baseline wherever it is
/// drawn; [`monitor_layout::TextRun::rect`] is the **line box**, not the ink.
const ASCENT_RATIO: f32 = 0.8;

/// The panel's rect in physical px, or `None` when there is no band to paint.
///
/// Pure, so the docking arithmetic is unit-tested without a GPU. `right_inset` is the
/// authority: a layer that painted wider than `grid_dims` reserved would cover columns
/// the terminal still believes it owns, and one that painted while the inset is zero
/// would cover a column of live output (D4's force-close is exactly that state).
fn panel_rect(viewport: [f32; 2], top_inset: f32, right_inset: f32) -> Option<[f32; 4]> {
    let (vw, vh) = (viewport[0], viewport[1]);
    if !(vw.is_finite() && vh.is_finite() && top_inset.is_finite() && right_inset.is_finite()) {
        return None;
    }
    let w = right_inset.clamp(0.0, vw.max(0.0));
    let top = top_inset.clamp(0.0, vh.max(0.0));
    let h = vh - top;
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    Some([vw - w, top, w, h])
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniforms {
    viewport: [f32; 2],
    _pad: [f32; 2],
}

#[repr(C)]
// `Debug`/`PartialEq` are for R7.3's test, which asserts the cached glyph quads are the
// same bytes the uncached path produced — a cache that skipped work and changed the answer
// would otherwise be indistinguishable from one that worked.
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
struct TextInstance {
    rect: [f32; 4],
    uv0: [f32; 2],
    uv1: [f32; 2],
    color: [f32; 4],
}

const TEXT_WGSL: &str = r#"
struct U { viewport: vec2<f32>, _pad: vec2<f32> };
@group(0) @binding(0) var<uniform> u: U;
@group(0) @binding(1) var atlas_tex: texture_2d<f32>;
@group(0) @binding(2) var atlas_samp: sampler;
struct VsIn { @location(0) corner: vec2<f32>, @location(1) rect: vec4<f32>, @location(2) uv0: vec2<f32>, @location(3) uv1: vec2<f32>, @location(4) color: vec4<f32> };
struct VsOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32>, @location(1) color: vec4<f32> };
@vertex fn vs_main(in: VsIn) -> VsOut {
    let px = in.rect.xy + in.corner * in.rect.zw;
    let clip = vec2<f32>(px.x / u.viewport.x * 2.0 - 1.0, 1.0 - px.y / u.viewport.y * 2.0);
    var o: VsOut; o.pos = vec4<f32>(clip, 0.0, 1.0); o.uv = mix(in.uv0, in.uv1, in.corner); o.color = in.color; return o;
}
@fragment fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let a = textureSample(atlas_tex, atlas_samp, in.uv).r;
    return vec4<f32>(in.color.rgb * a, a);
}
"#;

/// Premultiplied-alpha blend — the shader emits coverage premultiplied, matching the
/// glyph path and the shape pipeline.
const PREMULTIPLIED: wgpu::BlendState = wgpu::BlendState {
    color: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
        operation: wgpu::BlendOperation::Add,
    },
    alpha: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
        operation: wgpu::BlendOperation::Add,
    },
};

/// The panel's glyph pass. Shapes go through [`SidebarShapePipeline`] — every primitive
/// the design needs (rounded boxes, capsules, rings) already lives there, and a second
/// SDF shader would be a copy to keep in step with it.
struct Gpu {
    format: wgpu::TextureFormat,
    quad: wgpu::Buffer,
    uniform: wgpu::Buffer,
    text_pipeline: wgpu::RenderPipeline,
    /// Kept (not dropped after pipeline creation) so [`MonitorPanel::ensure_text_bind`]
    /// can build a *replacement* bind group on a later frame.
    text_bgl: wgpu::BindGroupLayout,
    text_bind: wgpu::BindGroup,
    /// `(format, atlas epoch)` the live `text_bind` was built against.
    ///
    /// It lives **inside `Gpu`** on purpose: `ensure_gpu` replaces the whole struct on a
    /// format change, so the key resets to `None` alongside the `uniform` buffer it
    /// binds — a cached bind group pointing at a replaced buffer is structurally
    /// impossible rather than merely unlikely.
    ///
    /// `generation` and the cell size are deliberately *not* in the key: eviction reuses
    /// the same texture (so a rebuild would be byte-identical) and the cell size never
    /// enters the bind group at all. Keying on either would churn bind groups at idle —
    /// the G1 regression.
    text_bind_key: Option<(wgpu::TextureFormat, u64)>,
    /// How many times `text_bind` has actually been (re)built. Key equality alone cannot
    /// express "rebuilds = 0 while the atlas is unchanged": that assertion still passes
    /// with the short-circuit deleted and the bind group rebuilt every frame, which is
    /// the G1 idle-CPU regression D2 forbids. `#[cfg(test)]`, so the shipped binary is
    /// unaffected.
    #[cfg(test)]
    rebuilds: u32,
    /// Grown across frames by `crate::upload_resident`, never reallocated per frame.
    instances: Option<wgpu::Buffer>,
}

/// One frame's shaped output, plus **every input it was shaped from** (R7.3).
///
/// The panel is not redrawn on a cadence, but it is redrawn a lot: PTY output, a cursor
/// move, a focus change and — the one this exists for — `drive_monitor_resize`, which
/// calls `redraw()` on every `CursorMoved` of a drag. Without a cache each of those
/// re-runs `monitor_layout::layout` over all ten regions, rebuilds the shape instance
/// list, and re-walks every glyph through the atlas. D2 made the panel static precisely
/// so it would cost nothing when nothing changed; re-shaping an identical model would
/// spend that saving anyway, just on a different line.
///
/// Every field is part of the key, and each one is here because it changes what
/// `layout` emits:
///
/// * `model` — the data. [`MonitorModel`] derives `PartialEq` for exactly this
///   comparison; keeping the clone is what makes the comparison possible at all.
/// * `panel` — the rect. **It changes on every pointer move of a drag**, so a drag is
///   *not* free and this cache does not claim it is. What it does claim is that the
///   redraws which are not geometry changes — which is nearly all of them — re-present
///   instead of re-shaping.
/// * `scale` / `cell` — a DPI change or a font resize moves every pixel in the panel
///   without touching the model. Omitting either would freeze the panel at the old
///   metrics until the next data arrived.
struct Shaped {
    model: MonitorModel,
    panel: [f32; 4],
    scale: f64,
    cell: [f32; 2],
    /// The position-independent document. Stage two reads it; it survives every scroll.
    draw: monitor_layout::PanelDraw,
}

/// **Stage two's** cache: the frame that actually reaches the GPU, plus the scroll state it
/// was composed for.
///
/// Separate from [`Shaped`] because the two go stale for different reasons, and collapsing
/// them re-creates exactly the defect the split exists to prevent (C4). A scroll must
/// invalidate this and *not* `Shaped`; a model change must invalidate both.
struct Composed {
    scroll: ScrollState,
    frame: monitor_layout::Primitives,
}

/// The ai-monitor panel draw layer.
pub struct MonitorPanel {
    model: MonitorHandle,
    gpu: Option<Gpu>,
    shapes: Option<SidebarShapePipeline>,
    /// Reused across frames so a steady panel allocates nothing per frame.
    text_buf: Vec<TextInstance>,
    /// Last frame's layout and the inputs that produced it (R7.3).
    shaped: Option<Shaped>,
    /// Last frame's composed primitives and the scroll they were composed for.
    composed: Option<Composed>,
    /// The atlas identity `text_buf`'s baked UVs were computed against — `(epoch,
    /// generation)`.
    ///
    /// `generation` is in the key and `epoch` alone is not enough: an eviction repacks
    /// the *same* texture and invalidates every UV already handed out
    /// (`atlas.rs`), so a buffer cached across one would sample the wrong glyphs.
    /// `None` means "rebuild unconditionally".
    text_atlas: Option<(u64, u64)>,
    /// How many times `monitor_layout::layout` actually ran. A test that only compared
    /// *output* would pass with the cache deleted — the layout is pure, so re-running it
    /// yields an equal `PanelDraw`. Only a count can tell "cached" from "recomputed to
    /// the same answer", and that distinction is the whole requirement.
    #[cfg(test)]
    layouts: u32,
    /// How many times **stage two** ran. Distinct from `layouts` on purpose: the whole
    /// claim of D24 is that a scroll moves this counter and leaves `layouts` alone, and only
    /// two counters can tell that apart from "both re-ran and produced the same answer".
    #[cfg(test)]
    composes: u32,
    /// How many times the glyph quads were re-walked through the atlas. Counted
    /// separately from `layouts` because they have different invalidation rules: an
    /// atlas repack invalidates the glyphs and not the layout.
    #[cfg(test)]
    text_builds: u32,
}

impl MonitorPanel {
    pub fn new() -> (MonitorPanel, MonitorHandle) {
        let model = MonitorHandle::default();
        (Self::from_handle(model.clone()), model)
    }

    /// Build a layer bound to an existing handle, so the owner (the ai-monitor overlay)
    /// keeps the write handle and hands the layer to the renderer through the SD4
    /// `Overlay::layer` seam.
    pub fn from_handle(model: MonitorHandle) -> MonitorPanel {
        MonitorPanel {
            model,
            gpu: None,
            shapes: None,
            text_buf: Vec::new(),
            shaped: None,
            composed: None,
            text_atlas: None,
            #[cfg(test)]
            layouts: 0,
            #[cfg(test)]
            composes: 0,
            #[cfg(test)]
            text_builds: 0,
        }
    }

    fn ensure_gpu(&mut self, ctx: &FrameCtx<'_>) {
        if self.gpu.as_ref().map(|g| g.format) == Some(ctx.format) {
            return;
        }
        let device = ctx.device;
        let quad = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("monitor-quad"),
            contents: bytemuck::cast_slice(&crate::QUAD),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("monitor-uniform"),
            contents: bytemuck::bytes_of(&Uniforms {
                viewport: ctx.viewport,
                _pad: [0.0, 0.0],
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let text_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("monitor-text-bgl"),
            entries: &[
                crate::uniform_entry(0),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let text_bind = build_text_bind(device, &text_bgl, &uniform, ctx.atlas);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("monitor-text"),
            source: wgpu::ShaderSource::Wgsl(TEXT_WGSL.into()),
        });
        let text_pipeline = crate::build_pipeline(
            device,
            &shader,
            &text_bgl,
            ctx.format,
            std::mem::size_of::<TextInstance>() as u64,
            &wgpu::vertex_attr_array![1 => Float32x4, 2 => Float32x2, 3 => Float32x2, 4 => Float32x4],
            PREMULTIPLIED,
        );
        self.gpu = Some(Gpu {
            format: ctx.format,
            quad,
            uniform,
            text_pipeline,
            text_bgl,
            text_bind,
            text_bind_key: Some((ctx.format, ctx.atlas.epoch())),
            // The eager build here counts as the first rebuild, so an idle test asserting
            // `rebuilds == 1` covers the whole record path.
            #[cfg(test)]
            rebuilds: 1,
            instances: None,
        });
    }

    /// Rebuild the atlas bind group when — and *only* when — the texture it samples has
    /// been replaced.
    ///
    /// `ensure_gpu` is gated on the surface format alone, which never changes at runtime;
    /// a live config reload swaps the whole `Atlas` (`Renderer::apply_config`) and leaves
    /// the format untouched, so without this the panel would keep sampling the replaced
    /// texture with UVs computed against the new one — wrong glyphs for the rest of the
    /// session. Splitting only the bind group out keeps pipelines and shader modules
    /// (which a swap does *not* invalidate) format-gated.
    ///
    /// Must run **after** `ensure_gpu`: it needs the `text_bgl` that `ensure_gpu` stores.
    fn ensure_text_bind(&mut self, ctx: &FrameCtx<'_>) {
        let Some(gpu) = self.gpu.as_mut() else {
            return;
        };
        let key = (ctx.format, ctx.atlas.epoch());
        if gpu.text_bind_key == Some(key) {
            return;
        }
        gpu.text_bind = build_text_bind(ctx.device, &gpu.text_bgl, &gpu.uniform, ctx.atlas);
        gpu.text_bind_key = Some(key);
        #[cfg(test)]
        {
            gpu.rebuilds += 1;
        }
    }

    /// Draw the accumulated glyph quads.
    fn draw_text(&mut self, ctx: &mut FrameCtx<'_>) {
        let Some(gpu) = self.gpu.as_mut() else {
            return;
        };
        ctx.queue.write_buffer(
            &gpu.uniform,
            0,
            bytemuck::bytes_of(&Uniforms {
                viewport: ctx.viewport,
                _pad: [0.0, 0.0],
            }),
        );
        let count = crate::upload_resident(
            ctx.device,
            ctx.queue,
            &mut gpu.instances,
            &self.text_buf,
            "monitor-text",
        );
        if count == 0 {
            return;
        }
        let instances = gpu.instances.as_ref().expect("resident buffer present");
        let mut pass = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("monitor-text"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: ctx.view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&gpu.text_pipeline);
        pass.set_bind_group(0, &gpu.text_bind, &[]);
        pass.set_vertex_buffer(0, gpu.quad.slice(..));
        pass.set_vertex_buffer(1, instances.slice(..));
        pass.draw(0..crate::QUAD.len() as u32, 0..count);
    }
}

impl DrawLayer for MonitorPanel {
    fn z_order(&self) -> i32 {
        MONITOR_PANEL_Z
    }

    fn record(&mut self, ctx: &mut FrameCtx<'_>) {
        // Clone the `Rc`, not the model: the borrow then comes off a local and leaves
        // `self` free for the GPU state below, without deep-copying every section each
        // frame.
        let handle = self.model.model.clone();
        let Ok(model) = handle.try_borrow() else {
            return;
        };
        // Both early-outs come before any GPU state is touched: an invisible panel and a
        // panel whose band D4 clamped to nothing must cost a frame nothing at all.
        if !model.visible {
            return;
        }
        let Some(panel) = panel_rect(ctx.viewport, ctx.top_inset, ctx.right_inset) else {
            return;
        };

        // ---- stage one (R7.3/R10.1) --------------------------------------------------
        //
        // Re-lay-out only when an input to the *layout* moved. See [`Shaped`] for why each
        // of the four is in the key — and note what is deliberately NOT: the scroll state.
        // That omission is C4's first half.
        let layout_stale = match self.shaped.as_ref() {
            Some(s) => {
                s.panel != panel
                    || s.scale != ctx.scale_factor
                    || s.cell != ctx.cell
                    || s.model != *model
            }
            None => true,
        };
        if layout_stale {
            let draw = monitor_layout::layout(&model, panel, ctx.scale_factor, ctx.cell);
            // The model is cloned, not referenced: the handle is the app's to overwrite at
            // any time, so the only way to answer "is this the model I shaped?" next frame
            // is to still be holding it. That clone is the cache's entire cost, and it is
            // paid on a miss — never on the hit path this exists for.
            self.shaped = Some(Shaped {
                model: model.clone(),
                panel,
                scale: ctx.scale_factor,
                cell: ctx.cell,
                draw,
            });
            #[cfg(test)]
            {
                self.layouts += 1;
            }
        }

        // ---- stage two (R10.2/R10.3) -------------------------------------------------
        //
        // The scroll IS in this key. C4's second half: leave it out here as well as in
        // stage one and the panel hits a stale frame and never repaints — the failure the
        // interview flagged as the top technical risk, and the one that is invisible in
        // review because nothing errors.
        let shaped = self
            .shaped
            .as_ref()
            .expect("stage one just ran or was cached");
        let want = {
            let s = self.model.scroll.get();
            let max = monitor_scroll::max_index(
                &shaped.draw.ladder,
                &shaped.draw.heights,
                shaped.draw.viewport.h,
            );
            // Clamp defensively: the app can legitimately hold an index from before the
            // last model swap, and one frame of a stale index must degrade, not panic.
            let top = monitor_scroll::clamp(
                &shaped.draw.ladder,
                &shaped.draw.heights,
                shaped.draw.viewport.h,
                s.top.min(max),
            );
            ScrollState {
                badge: if top == 0 { 0 } else { s.badge },
                top,
            }
        };
        // Reconcile the handle with what will actually be composed.
        //
        // Without this the clamp above is a *local* correction: the frame draws at the
        // clamped index while the app goes on believing the old one. That is reachable —
        // grow the window until the whole document fits and `max_index` collapses to 0 —
        // and it makes two of the spec's own invariants lie. `rearm_anchor` reads this
        // `top`, so the mode stays `Held` at a frame that is showing the document's top
        // (D37/R6.3); and `count_new_above` keeps counting arrivals above a rung nobody is
        // looking at, so the footer says "N new above" while all of it is on screen
        // (D33/R7.8). It self-heals on the next wheel event, which is exactly what would
        // make it present as intermittent.
        //
        // Reaching index 0 clears the badge here for the same reason it does on the wheel
        // path: at the top there is nothing above left unseen. This is the **fourth** site
        // D33 is enforced at (`count_new_above`, `on_wheel` and `jump_to` are the others, and
        // D26/D27 put the rule in the app); it is here as the clamp path's backstop, because
        // the clamp is the one way the index moves without the app asking.
        //
        // Two bounds this reconcile deliberately does NOT reach, so the next reader does not
        // mistake them for oversights:
        //
        // * A clamp to a NON-zero index leaves `AiMonitorOverlay::anchor` naming the old
        //   rung. The app re-publishes it and this re-clamps it every frame — stable, but the
        //   badge can still count arrivals that are on screen. Strictly narrower than the bug
        //   this fixes, which had the badge lying at the very top — D33's own case.
        // * It does not clear the panel's wheel residue, so the first gesture after a resize
        //   spends a bank accumulated against the old position. Bounded by one rung.
        if want != self.model.scroll.get() {
            self.model.set_scroll(want);
        }
        let compose_stale = layout_stale || self.composed.as_ref().map(|c| c.scroll) != Some(want);
        if compose_stale {
            let frame = monitor_layout::compose(&shaped.draw, &model, want.top, want.badge);
            self.composed = Some(Composed {
                scroll: want,
                frame,
            });
            #[cfg(test)]
            {
                self.composes += 1;
            }
        }
        // Publish what the app needs to route a wheel or a jump. Only on a stage-one miss:
        // the ladder and the band are exactly what stage one produced, so a scroll — which
        // changes neither — must not pay for a clone of them.
        if layout_stale {
            self.model.publish_view(ScrollView {
                ladder: shaped.draw.ladder.clone(),
                heights: shaped.draw.heights.clone(),
                viewport_h: shaped.draw.viewport.h,
                viewport: shaped.draw.viewport,
                slot: shaped.draw.slot,
            });
        }
        drop(model);

        self.ensure_gpu(ctx);
        // Order matters: `ensure_text_bind` builds from the `text_bgl` `ensure_gpu` stores.
        self.ensure_text_bind(ctx);

        let Some(composed) = self.composed.as_ref() else {
            return;
        };
        // Shapes first, glyphs over them — both are `LoadOp::Load` passes, so the order
        // of the two calls *is* the compositing order. (Disjoint fields: `self.shapes` is
        // borrowed mutably while `self.shaped` is read.)
        {
            let sp = SidebarShapePipeline::ensure(&mut self.shapes, ctx);
            sp.draw(ctx, &composed.frame.shapes);
        }

        // The glyph quads carry baked atlas UVs, so they survive a frame only while the
        // atlas is byte-for-byte the one they were baked against — a *repack* is enough to
        // invalidate them, which is why `generation` rides with `epoch`.
        let atlas_id = (ctx.atlas.epoch(), ctx.atlas.generation());
        // Glyph quads follow stage TWO — they are built from the composed frame, so a
        // scroll must rebuild them even though it re-shaped nothing. Cheap: `compose` moved
        // already-shaped runs, and `build_text` walks the atlas for glyphs it has seen.
        if compose_stale || self.text_atlas != Some(atlas_id) {
            build_text(ctx, &composed.frame.runs, &mut self.text_buf);
            // Re-read afterwards: `layout_line` packs glyphs it has not seen, and a pack
            // that evicts bumps the generation *mid-build*, leaving the quads pushed
            // before it holding pre-repack UVs. Storing `None` in that case costs one
            // extra rebuild next frame and is the only way this cache cannot outlive its
            // own UVs.
            let after = (ctx.atlas.epoch(), ctx.atlas.generation());
            self.text_atlas = (after == atlas_id).then_some(after);
            #[cfg(test)]
            {
                self.text_builds += 1;
            }
        }
        self.draw_text(ctx);
    }
}

/// Turn the laid-out runs into glyph quads.
///
/// `run.rect.x` is the pen origin and `run.rect.y` the top of the **line box**, so the
/// baseline is `y + ascent`. Advance and fallback go through `layout_line`, the same
/// helper the tab bar draws with, so a Hangul task title spends 2 columns per char here
/// exactly as `monitor_layout` reserved for it.
///
/// Free-standing rather than a method so [`MonitorPanel::record`] can hold `&self.shaped`
/// (the runs) and `&mut self.text_buf` (the output) at once — two disjoint fields, which
/// `&mut self` would collapse into one borrow.
fn build_text(
    ctx: &mut FrameCtx<'_>,
    runs: &[monitor_layout::TextRun],
    out: &mut Vec<TextInstance>,
) {
    out.clear();
    let cw = ctx.cell[0];
    let ascent = ctx.cell[1] * ASCENT_RATIO;
    for run in runs {
        let base_y = run.rect.y + ascent;
        let color = linear(run.color, 1.0);
        layout_line(ctx, &run.text, run.rect.x, cw, |pen, e| {
            out.push(TextInstance {
                rect: [pen + e.left, base_y - e.top, e.px_w, e.px_h],
                uv0: e.uv0,
                uv1: e.uv1,
                color,
            });
        });
    }
}

/// The one place the atlas bind group is assembled — `ensure_gpu` builds it eagerly,
/// `ensure_text_bind` reassigns it after a texture swap, and both must bind the same
/// three resources.
fn build_text_bind(
    device: &wgpu::Device,
    bgl: &wgpu::BindGroupLayout,
    uniform: &wgpu::Buffer,
    atlas: &crate::atlas::Atlas,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("monitor-text-bg"),
        layout: bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(atlas.view()),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(atlas.sampler()),
            },
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::monitor_model::{ColorRole, PanelStatus, SessionRow, StatTile};

    /// A model with something in every pinned region, so a test that draws it produces
    /// real shapes and real runs rather than passing on an empty frame.
    fn filled() -> MonitorModel {
        MonitorModel {
            visible: true,
            title: "ai-monitor".into(),
            status: PanelStatus::Connected,
            session: SessionRow {
                name: "session".into(),
                short_id: "5e59b629".into(),
                provider: "[Claude]".into(),
                model: "claude-opus-5".into(),
                note: None,
            },
            tiles: vec![
                StatTile::new("UPTIME", "45m12s", ColorRole::TextPrimary),
                StatTile::new("CONTEXT", "59%", ColorRole::Teal).with_gauge(0.59),
            ],
            sections: Vec::new(),
            footer: "Last update 16:21:18".into(),
        }
    }

    /// `filled()` plus a document long enough that the panel must scroll to reach its end.
    fn scrollable() -> MonitorModel {
        use crate::monitor_model::{Row, Section, SectionBody};
        let section = |label: &str, n: usize| Section {
            collapsed: false,
            label: label.into(),
            total: n,
            lost: 0,
            body: SectionBody::Rows(
                (0..n)
                    .map(|i| Row {
                        text: format!("{label}/{i}"),
                        ..Row::default()
                    })
                    .collect(),
            ),
        };
        MonitorModel {
            sections: vec![section("TASKS", 12), section("RECENT CALLS", 20)],
            ..filled()
        }
    }

    // ---- ai-monitor-panel-scroll T4 / R10.1-R10.4 / R2.1: the two-stage split ----

    /// The claim D24 exists to make, and the one that is expensive to be wrong about.
    ///
    /// A scroll must move stage two and leave stage one alone. Both halves are asserted
    /// because each fails in its own silent way: keep the scroll out of stage two's key
    /// (C4's second half) and the panel hits a stale frame and **never repaints** — no
    /// error, no warning, just a wheel that does nothing. Put it into stage one's key
    /// instead (C4's first half) and every wheel notch re-lays-out a document D4 just made
    /// much longer, reviving the per-frame re-layout debt `architecture.md` G1 records as
    /// paid off.
    ///
    /// The pixel assertion is not decoration: counters alone would pass a `compose` that
    /// ran and returned the previous frame.
    #[test]
    fn r10_1_to_r10_3_a_scroll_recomposes_without_re_laying_out_and_the_pixels_move() {
        let (Some((device, queue)), Some(fonts)) = (crate::headless(), crate::FontSet::load().ok())
        else {
            eprintln!("skipping: no GPU/font");
            return;
        };
        let mut atlas = crate::atlas::Atlas::new(&device, &fonts, 24.0).expect("atlas");
        let view = offscreen(&device);
        let t = term_core::Terminal::new(10, 2, term_core::Color(0xFFFFFF), term_core::Color(0));
        let snap = t.snapshot();
        let sidebar_cfg = common::config::SidebarConfig::default();
        let (mut layer, handle) = MonitorPanel::new();
        handle.set(scrollable());
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        let mut frame = |layer: &mut MonitorPanel, atlas: &mut crate::atlas::Atlas| {
            let mut ctx = FrameCtx {
                device: &device,
                queue: &queue,
                encoder: &mut encoder,
                view: &view,
                viewport: [256.0, 400.0],
                cell: [12.0, 24.0],
                top_inset: 0.0,
                left_inset: 0.0,
                right_inset: 200.0,
                format: wgpu::TextureFormat::Rgba8Unorm,
                sidebar: &sidebar_cfg,
                scale_factor: 1.0,
                snapshot: &snap,
                atlas,
            };
            layer.record(&mut ctx);
        };

        frame(&mut layer, &mut atlas);
        assert_eq!(layer.layouts, 1);
        assert_eq!(layer.composes, 1);
        let at_top = layer.text_buf.clone();
        assert!(!at_top.is_empty(), "premise: the panel drew glyphs");

        // The panel must actually have somewhere to scroll, or every assertion below is
        // vacuously true.
        let max = handle.metrics().max_index;
        assert!(
            max > 0,
            "premise: the document must overflow the viewport (max_index {max})"
        );

        // Idle frames at an unchanged scroll: neither stage re-runs.
        for _ in 0..20 {
            frame(&mut layer, &mut atlas);
        }
        assert_eq!(layer.layouts, 1, "idle frames must not re-lay-out");
        assert_eq!(layer.composes, 1, "nor recompose");

        // Now scroll. Stage two only.
        handle.set_scroll(ScrollState { top: max, badge: 0 });
        frame(&mut layer, &mut atlas);
        assert_eq!(
            layer.layouts, 1,
            "a scroll must NOT re-lay-out the document (C4 first half / R10.1)"
        );
        assert_eq!(
            layer.composes, 2,
            "but it must recompose (C4 second half / R10.3)"
        );
        assert_ne!(
            layer.text_buf, at_top,
            "and the recomposition must reach the pixels — a counter that moved while the \
             frame did not is exactly the stale-cache failure C4 names"
        );

        // Scrolling back reproduces the first frame: stage two skips work, it does not
        // change the answer.
        handle.set_scroll(ScrollState { top: 0, badge: 0 });
        frame(&mut layer, &mut atlas);
        assert_eq!(layer.layouts, 1);
        assert_eq!(layer.composes, 3);
        assert_eq!(layer.text_buf, at_top);
    }

    /// R6.3/R7.8 — the clamp is written back, so the app cannot go on believing an index the
    /// frame is not drawing.
    ///
    /// Found by review, not by the maker's own tests, and worth the words: computing a
    /// clamped index, composing from it and discarding it is *locally* correct — every pixel
    /// is right — while leaving the app holding the old value. It is reachable by growing the
    /// window until the document fits and `max_index` collapses to 0, and it makes two of the
    /// spec's invariants lie at once: the anchor stays `Held` at a frame showing the top
    /// (D37), and the footer keeps claiming "N new above" with all of it on screen (D33).
    /// It self-heals on the next wheel event, which is what would have made it intermittent.
    #[test]
    fn r6_3_and_r7_8_a_shrunken_document_reconciles_the_index_instead_of_drifting() {
        let (Some((device, queue)), Some(fonts)) = (crate::headless(), crate::FontSet::load().ok())
        else {
            eprintln!("skipping: no GPU/font");
            return;
        };
        let mut atlas = crate::atlas::Atlas::new(&device, &fonts, 24.0).expect("atlas");
        let view = offscreen(&device);
        let t = term_core::Terminal::new(10, 2, term_core::Color(0xFFFFFF), term_core::Color(0));
        let snap = t.snapshot();
        let sidebar_cfg = common::config::SidebarConfig::default();
        let (mut layer, handle) = MonitorPanel::new();
        handle.set(scrollable());
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        let mut frame = |layer: &mut MonitorPanel, atlas: &mut crate::atlas::Atlas, h: f32| {
            let mut ctx = FrameCtx {
                device: &device,
                queue: &queue,
                encoder: &mut encoder,
                view: &view,
                viewport: [256.0, h],
                cell: [12.0, 24.0],
                top_inset: 0.0,
                left_inset: 0.0,
                right_inset: 200.0,
                format: wgpu::TextureFormat::Rgba8Unorm,
                sidebar: &sidebar_cfg,
                scale_factor: 1.0,
                snapshot: &snap,
                atlas,
            };
            layer.record(&mut ctx);
        };

        // A short panel: the document overflows, so scroll away from the top and take a badge.
        frame(&mut layer, &mut atlas, 400.0);
        let max = handle.metrics().max_index;
        assert!(max > 0, "premise: the document overflows a 400px panel");
        handle.set_scroll(ScrollState { top: max, badge: 5 });
        frame(&mut layer, &mut atlas, 400.0);
        assert_eq!(handle.scroll().top, max);

        // Now grow the window until everything fits. `max_index` collapses.
        frame(&mut layer, &mut atlas, 4000.0);
        assert_eq!(
            handle.metrics().max_index,
            0,
            "premise: the whole document now fits"
        );
        assert_eq!(
            handle.scroll().top,
            0,
            "the app's index must follow the frame, not lag it"
        );
        assert_eq!(
            handle.scroll().badge,
            0,
            "and a badge claiming items above the top is a lie once the top is all there is"
        );
    }

    /// R10.4 — the scroll state is not part of the model, stated as an executable fact.
    ///
    /// `MonitorModel`'s `PartialEq` **is** stage one's cache key, so "the scroll is not a
    /// field" and "a scroll does not invalidate stage one" are the same sentence. A future
    /// change that quietly moved `top` into the model would keep every other test in this
    /// file green — the panel would still be correct, just slow again, which is precisely
    /// the kind of regression nobody notices.
    #[test]
    fn r10_4_and_r7_7_moving_the_scroll_or_badge_leaves_the_model_comparing_equal() {
        let (_layer, handle) = MonitorPanel::new();
        handle.set(scrollable());
        let before = handle.model.borrow().clone();
        handle.set_scroll(ScrollState { top: 7, badge: 0 });
        assert_eq!(handle.scroll(), ScrollState { top: 7, badge: 0 });
        assert_eq!(
            *handle.model.borrow(),
            before,
            "the model — stage one's key — must not notice a scroll"
        );
        // R7.7: the badge rides here for the same reason and must not leak into the model
        // either. `model.footer` is the natural place to put it — the app composes both —
        // and it is wrong twice over: stage one would re-lay-out the whole document on every
        // count change, and `dashboard`'s own assertion that the footer begins with
        // "Last update" would start failing for a reason unrelated to time.
        handle.set_scroll(ScrollState { top: 7, badge: 12 });
        assert_eq!(
            *handle.model.borrow(),
            before,
            "nor a badge — stage two renders it, stage one never sees it"
        );
    }

    /// R10.9 — stage two's own shaping stays under the atlas-invalidation guard.
    ///
    /// D36 lets stage two shape exactly one thing: the badge line. That one line can still
    /// pack a glyph the atlas has not seen, and a pack that evicts bumps `generation`
    /// **mid-build** — which invalidates the UVs of everything already copied out of stage
    /// one in the same frame. The existing guard (`text_atlas = (after == atlas_id)
    /// .then_some(after)`) covers it only because the badge goes through the *same*
    /// `build_text` call as the rest of the frame.
    ///
    /// So what this pins is the routing: a badge change must move `text_builds`. A future
    /// change that drew the badge from a side buffer to "save a rebuild" would keep every
    /// other test green and start sampling the wrong glyphs after an eviction — silently.
    #[test]
    fn r10_9_a_badge_change_rebuilds_the_glyphs_under_the_atlas_guard() {
        let (Some((device, queue)), Some(fonts)) = (crate::headless(), crate::FontSet::load().ok())
        else {
            eprintln!("skipping: no GPU/font");
            return;
        };
        let mut atlas = crate::atlas::Atlas::new(&device, &fonts, 24.0).expect("atlas");
        let view = offscreen(&device);
        let t = term_core::Terminal::new(10, 2, term_core::Color(0xFFFFFF), term_core::Color(0));
        let snap = t.snapshot();
        let sidebar_cfg = common::config::SidebarConfig::default();
        let (mut layer, handle) = MonitorPanel::new();
        handle.set(scrollable());
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        let mut frame = |layer: &mut MonitorPanel, atlas: &mut crate::atlas::Atlas| {
            let mut ctx = FrameCtx {
                device: &device,
                queue: &queue,
                encoder: &mut encoder,
                view: &view,
                viewport: [256.0, 400.0],
                cell: [12.0, 24.0],
                top_inset: 0.0,
                left_inset: 0.0,
                right_inset: 200.0,
                format: wgpu::TextureFormat::Rgba8Unorm,
                sidebar: &sidebar_cfg,
                scale_factor: 1.0,
                snapshot: &snap,
                atlas,
            };
            layer.record(&mut ctx);
        };

        handle.set_scroll(ScrollState { top: 2, badge: 0 });
        frame(&mut layer, &mut atlas);
        let builds = layer.text_builds;
        let no_badge = layer.text_buf.clone();
        assert!(!no_badge.is_empty(), "premise: the panel drew glyphs");

        handle.set_scroll(ScrollState { top: 2, badge: 7 });
        frame(&mut layer, &mut atlas);
        assert_eq!(
            layer.layouts, 1,
            "a badge is scroll state, so stage one must not re-run (C4)"
        );
        assert!(
            layer.text_builds > builds,
            "but the glyphs must be rebuilt — that is what keeps the badge inside the \
             atlas-generation guard rather than beside it"
        );
        assert_ne!(
            layer.text_buf, no_badge,
            "and the rebuild must reach the pixels"
        );
    }

    /// R2.1 — the pinned chrome does not move when the document does.
    ///
    /// Asserted on stage one's output rather than on pixels because that is where the
    /// property lives: `compose` copies `chrome` through and translates `content`, so a
    /// region in the wrong bag would scroll away no matter how careful stage two was.
    #[test]
    fn r2_1_and_r10_6_the_chrome_is_identical_at_every_scroll_position() {
        let m = scrollable();
        let panel = [0.0, 0.0, 200.0, 400.0];
        let draw = monitor_layout::layout(&m, panel, 1.0, [12.0, 24.0]);
        let ladder = monitor_layout::item_ladder(&m);
        let heights = monitor_layout::item_heights(&m, 1.0, [12.0, 24.0]);
        let max = monitor_scroll::max_index(&ladder, &heights, draw.viewport.h);
        assert!(max > 0, "premise: this document scrolls");

        let chrome_runs: Vec<_> = draw.chrome.runs.iter().map(|r| r.rect.y).collect();
        let mut seen_bodies = std::collections::HashSet::new();
        for top in 0..=max {
            let frame = monitor_layout::compose(&draw, &m, top, 0);
            // Every chrome run is present at its original y, untranslated.
            for (run, y) in draw.chrome.runs.iter().zip(&chrome_runs) {
                assert!(
                    frame
                        .runs
                        .iter()
                        .any(|r| r.text == run.text && r.rect.y == *y),
                    "top {top}: chrome run {:?} moved or vanished",
                    run.text
                );
            }
            seen_bodies.insert(
                frame
                    .runs
                    .iter()
                    .filter(|r| r.text.contains('/'))
                    .map(|r| r.text.clone())
                    .collect::<Vec<_>>(),
            );
        }
        assert!(
            seen_bodies.len() > 1,
            "premise: the document really did move under the chrome"
        );
    }

    #[test]
    fn handle_roundtrips_the_model() {
        let (_layer, h) = MonitorPanel::new();
        assert!(!h.model.borrow().visible);
        h.set(filled());
        assert!(h.model.borrow().visible);
        assert_eq!(h.model.borrow().footer, "Last update 16:21:18");
    }

    #[test]
    fn the_band_is_the_right_edge_below_the_top_inset() {
        // The dock, in one assertion: a 96px inset on an 800x600 window is the rightmost
        // 96px, full height below the tab bar. (The retiring overlay's rect was the lower
        // 55% at FULL width — the change this layer exists for.)
        assert_eq!(
            panel_rect([800.0, 600.0], 0.0, 96.0),
            Some([704.0, 0.0, 96.0, 600.0])
        );
        assert_eq!(
            panel_rect([800.0, 600.0], 40.0, 96.0),
            Some([704.0, 40.0, 96.0, 560.0])
        );
    }

    #[test]
    fn a_zero_inset_paints_nothing() {
        // R4.4 / D4: the panel force-closes on a window too narrow to hold it, and the
        // inset going to zero IS that close. Painting a sliver — or worse, painting the
        // old full-width overlay — would cover a column the terminal still owns.
        assert_eq!(panel_rect([800.0, 600.0], 0.0, 0.0), None);
        assert_eq!(panel_rect([800.0, 600.0], 0.0, -10.0), None);
        // Degenerate geometry answers `None` rather than emitting a NaN rect.
        assert_eq!(panel_rect([800.0, 600.0], 700.0, 96.0), None);
        assert_eq!(panel_rect([800.0, 600.0], 0.0, f32::NAN), None);
        assert_eq!(panel_rect([f32::NAN, 600.0], 0.0, 96.0), None);
        // An inset wider than the window clamps rather than starting off-screen left.
        assert_eq!(
            panel_rect([800.0, 600.0], 0.0, 1200.0),
            Some([0.0, 0.0, 800.0, 600.0])
        );
    }

    #[test]
    fn z_order_is_above_the_tab_bar() {
        let (layer, _h) = MonitorPanel::new();
        assert!(layer.z_order() > crate::tab_bar::TAB_BAR_Z);
    }

    /// An offscreen render target for the GPU tests below.
    fn offscreen(device: &wgpu::Device) -> wgpu::TextureView {
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen"),
            size: wgpu::Extent3d {
                width: 256,
                height: 128,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        target.create_view(&wgpu::TextureViewDescriptor::default())
    }

    /// The band the GPU tests record against. **Non-zero is load-bearing**: with a zero
    /// `right_inset` `record` returns before touching any GPU state (there is no band to
    /// paint), and every assertion below would pass vacuously — the same trap the
    /// `visible` flag sets, one field over.
    const BAND: f32 = 96.0;

    /// A layer whose model clears the `!visible` early-out: without a visible model
    /// `record` returns before touching any GPU state.
    fn visible_layer() -> MonitorPanel {
        let (layer, handle) = MonitorPanel::new();
        handle.set(filled());
        layer
    }

    fn bind_key(layer: &MonitorPanel) -> Option<(wgpu::TextureFormat, u64)> {
        layer
            .gpu
            .as_ref()
            .expect("recording must build gpu state")
            .text_bind_key
    }

    /// R7.2: replacing the atlas — what `Renderer::apply_config` does on every live
    /// config reload — must invalidate the bind group that samples it. Before this fix
    /// `ensure_gpu`'s format-only gate meant the bind group was built once and never
    /// again, so the panel sampled the *replaced* texture with UVs computed against the
    /// new one.
    #[test]
    fn swapping_the_atlas_rebuilds_the_text_bind_group() {
        let (Some((device, queue)), Some(fonts)) = (crate::headless(), crate::FontSet::load().ok())
        else {
            eprintln!("skipping: no GPU/font");
            return;
        };
        let mut atlas_a = crate::atlas::Atlas::new(&device, &fonts, 24.0).expect("atlas");
        let mut atlas_b = crate::atlas::Atlas::new(&device, &fonts, 24.0).expect("atlas");
        let view = offscreen(&device);
        let t = term_core::Terminal::new(10, 2, term_core::Color(0xFFFFFF), term_core::Color(0));
        let snap = t.snapshot();
        let sidebar_cfg = common::config::SidebarConfig::default();
        let mut layer = visible_layer();
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });

        let frame = |layer: &mut MonitorPanel,
                     atlas: &mut crate::atlas::Atlas,
                     encoder: &mut wgpu::CommandEncoder| {
            let mut ctx = FrameCtx {
                device: &device,
                queue: &queue,
                encoder,
                view: &view,
                viewport: [256.0, 128.0],
                cell: [12.0, 24.0],
                top_inset: 0.0,
                left_inset: 0.0,
                right_inset: BAND,
                format: wgpu::TextureFormat::Rgba8Unorm,
                sidebar: &sidebar_cfg,
                scale_factor: 1.0,
                snapshot: &snap,
                atlas,
            };
            layer.record(&mut ctx);
        };

        frame(&mut layer, &mut atlas_a, &mut encoder);
        let k1 = bind_key(&layer);
        assert!(k1.is_some(), "a visible panel must build its bind group");
        assert!(
            !layer.text_buf.is_empty(),
            "premise: the panel actually drew glyphs, so the bind group matters"
        );

        frame(&mut layer, &mut atlas_b, &mut encoder);
        let k2 = bind_key(&layer);
        // Only inequality is asserted: the epoch counter is process-global and shared
        // with tests running in parallel, so no specific value is deterministic.
        assert_ne!(
            k1, k2,
            "a replaced atlas texture must invalidate the bind group keyed on it"
        );
        // A moved key does NOT imply a moved bind group. Without this, a mutant that
        // advances the key while leaving `text_bind` pointing at the replaced texture
        // passes — and that mutant *is* the production bug.
        assert_eq!(
            layer.gpu.as_ref().map(|g| g.rebuilds),
            Some(2),
            "the swap must actually re-point the bind group, not just re-stamp the key"
        );
    }

    /// R7.1 / G1: the flip side — an unchanged atlas must *not* churn bind groups every
    /// frame. (Scope note: **only the bind group** is asserted here, because the panel's
    /// other per-frame costs are guaranteed elsewhere and asserting them twice would let
    /// one copy rot: R7.3's `Shaped` cache is what keeps `layout` and the glyph walk off
    /// an unchanged frame — see
    /// `an_unchanged_model_is_shaped_once_no_matter_how_many_frames_are_drawn` — and
    /// `crate::upload_resident` grows one resident buffer instead of creating one per
    /// frame. A test that folded all three together would fail for three unrelated
    /// reasons and name none of them.)
    #[test]
    fn an_unchanged_atlas_never_rebuilds_the_text_bind_group() {
        let (Some((device, queue)), Some(fonts)) = (crate::headless(), crate::FontSet::load().ok())
        else {
            eprintln!("skipping: no GPU/font");
            return;
        };
        let mut atlas = crate::atlas::Atlas::new(&device, &fonts, 24.0).expect("atlas");
        let view = offscreen(&device);
        let t = term_core::Terminal::new(10, 2, term_core::Color(0xFFFFFF), term_core::Color(0));
        let snap = t.snapshot();
        let sidebar_cfg = common::config::SidebarConfig::default();
        let mut layer = visible_layer();
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });

        let mut first = None;
        for frame in 0..4 {
            {
                let mut ctx = FrameCtx {
                    device: &device,
                    queue: &queue,
                    encoder: &mut encoder,
                    view: &view,
                    viewport: [256.0, 128.0],
                    cell: [12.0, 24.0],
                    top_inset: 0.0,
                    left_inset: 0.0,
                    right_inset: BAND,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    sidebar: &sidebar_cfg,
                    scale_factor: 1.0,
                    snapshot: &snap,
                    atlas: &mut atlas,
                };
                layer.record(&mut ctx);
            }
            let key = bind_key(&layer);
            assert!(key.is_some(), "a visible panel must build its bind group");
            match first {
                None => first = key,
                Some(k) => assert_eq!(key, Some(k), "frame {frame} rebuilt at idle"),
            }
        }
        // Key equality alone does NOT prove this: it still holds when the bind group is
        // rebuilt every frame with the same key. Count the actual rebuilds, or the G1
        // guarantee is asserted in name only.
        assert_eq!(
            layer.gpu.as_ref().map(|g| g.rebuilds),
            Some(1),
            "4 frames on one atlas must build the bind group exactly once"
        );
    }

    /// R7.3: the guarantee the retiring overlay could not hand over.
    ///
    /// **What is guaranteed:** a frame whose model, band, scale and cell metrics all match
    /// the previous frame's re-presents the cached shapes and glyph quads — it does not
    /// re-run `monitor_layout::layout` over the ten regions and does not re-walk every run
    /// through the atlas. That covers nearly every redraw the panel sees: PTY output, a
    /// cursor move, a focus change, a config reload that changed something else.
    ///
    /// **What is NOT guaranteed, and the test says so by asserting it:** a drag is not
    /// free. `drive_monitor_resize` calls `redraw()` on every `CursorMoved` *and* the band
    /// it is dragging is in the cache key, so a pointer move that actually changes the
    /// panel's width re-shapes — as it must, or the panel would paint at last frame's
    /// width. The cost bound is "one shaping per distinct appearance", not "one shaping
    /// per drag".
    ///
    /// **Why a counter and not an output comparison.** `layout` is pure, so re-running it
    /// returns an equal `PanelDraw`. A test asserting the drawn output is unchanged passes
    /// identically with the cache deleted — it would be asserting that the layout is
    /// deterministic, which was never in doubt. Only the count separates "cached" from
    /// "recomputed to the same answer". (Verified by mutation: replacing the `stale` test
    /// with `true` leaves every other assertion in this file green and fails the two
    /// counter assertions below.)
    #[test]
    fn an_unchanged_model_is_shaped_once_no_matter_how_many_frames_are_drawn() {
        let (Some((device, queue)), Some(fonts)) = (crate::headless(), crate::FontSet::load().ok())
        else {
            eprintln!("skipping: no GPU/font");
            return;
        };
        let mut atlas = crate::atlas::Atlas::new(&device, &fonts, 24.0).expect("atlas");
        let view = offscreen(&device);
        let t = term_core::Terminal::new(10, 2, term_core::Color(0xFFFFFF), term_core::Color(0));
        let snap = t.snapshot();
        let sidebar_cfg = common::config::SidebarConfig::default();
        let (mut layer, handle) = MonitorPanel::new();
        handle.set(filled());
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });

        let mut frame = |layer: &mut MonitorPanel, band: f32, atlas: &mut crate::atlas::Atlas| {
            let mut ctx = FrameCtx {
                device: &device,
                queue: &queue,
                encoder: &mut encoder,
                view: &view,
                viewport: [256.0, 128.0],
                cell: [12.0, 24.0],
                top_inset: 0.0,
                left_inset: 0.0,
                right_inset: band,
                format: wgpu::TextureFormat::Rgba8Unorm,
                sidebar: &sidebar_cfg,
                scale_factor: 1.0,
                snapshot: &snap,
                atlas,
            };
            layer.record(&mut ctx);
        };

        // 50 frames — the pointer-move count R7.3 names — on one unchanged model.
        for _ in 0..50 {
            frame(&mut layer, BAND, &mut atlas);
        }
        assert_eq!(
            layer.layouts, 1,
            "50 identical frames must lay the ten regions out once, not 50 times"
        );
        assert_eq!(
            layer.text_builds, 1,
            "and must walk the runs through the atlas once — the layout cache alone \
             would still re-shape every glyph per frame"
        );
        assert!(
            !layer.text_buf.is_empty(),
            "premise: there were glyphs to cache, so the counters above mean something"
        );
        let cached = layer.text_buf.clone();

        // A drag step. The band is in the key *because* it changes what is drawn, so this
        // MUST miss — the cache is a skip on sameness, never a stale-frame optimisation.
        frame(&mut layer, BAND + 8.0, &mut atlas);
        assert_eq!(
            layer.layouts, 2,
            "a band that actually changed width must re-shape — a drag is not free"
        );
        assert_ne!(
            layer.text_buf, cached,
            "and the re-shape must reach the glyphs, not just the counter"
        );

        // Back to the original band: a miss again (the key moved twice), and the output
        // must equal what was cached the first time. This is the correctness half — a
        // cache that returned the right count and the wrong pixels would pass everything
        // above.
        frame(&mut layer, BAND, &mut atlas);
        assert_eq!(layer.layouts, 3);
        assert_eq!(
            layer.text_buf, cached,
            "the same inputs must still produce the same glyphs — the cache skips work, \
             it does not change the answer"
        );

        // New data with the band held still: the model is in the key too, so a snapshot
        // arriving while nothing moved still repaints. Without this the panel would freeze
        // on the first model it ever drew.
        handle.set(MonitorModel {
            footer: "Last update 16:22:00".into(),
            ..filled()
        });
        frame(&mut layer, BAND, &mut atlas);
        assert_eq!(
            layer.layouts, 4,
            "a changed model must re-shape even though the geometry held still"
        );
        assert_eq!(
            layer.text_builds, 4,
            "and its glyphs must be rebuilt with it"
        );
    }

    /// R5.1, at the only level that can actually be wrong: **pixels**.
    ///
    /// Every other test in this file asserts about structs. This one records a frame to
    /// an offscreen target and reads it back, so it fails if the layer paints the wrong
    /// band, paints nothing, or paints over the terminal — three things a rect returned
    /// by a pure function cannot tell you apart. Alpha is the discriminator: the target
    /// is cleared transparent and the panel's background is an opaque fill, so an
    /// untouched pixel reads 0 and a painted one reads 255 whatever the theme is.
    #[test]
    fn the_band_is_painted_at_the_right_edge_and_nowhere_else() {
        let (Some((device, queue)), Some(fonts)) = (crate::headless(), crate::FontSet::load().ok())
        else {
            eprintln!("skipping: no GPU/font");
            return;
        };
        const W: u32 = 256;
        const H: u32 = 128;
        let mut atlas = crate::atlas::Atlas::new(&device, &fonts, 24.0).expect("atlas");
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("monitor-readback"),
            size: wgpu::Extent3d {
                width: W,
                height: H,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
        let t = term_core::Terminal::new(10, 2, term_core::Color(0xFFFFFF), term_core::Color(0));
        let snap = t.snapshot();
        let sidebar_cfg = common::config::SidebarConfig::default();
        let mut layer = visible_layer();
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let _clear = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        }
        {
            let mut ctx = FrameCtx {
                device: &device,
                queue: &queue,
                encoder: &mut enc,
                view: &view,
                viewport: [W as f32, H as f32],
                cell: [12.0, 24.0],
                top_inset: 0.0,
                left_inset: 0.0,
                right_inset: BAND,
                format: wgpu::TextureFormat::Rgba8Unorm,
                sidebar: &sidebar_cfg,
                scale_factor: 1.0,
                snapshot: &snap,
                atlas: &mut atlas,
            };
            layer.record(&mut ctx);
        }
        let row = W * 4;
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("monitor-readback-buf"),
            size: (row * H) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(H),
                },
            },
            wgpu::Extent3d {
                width: W,
                height: H,
                depth_or_array_layers: 1,
            },
        );
        queue.submit(std::iter::once(enc.finish()));
        let slice = buf.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        let data = slice.get_mapped_range().expect("buffer mapped").to_vec();
        let px = |x: u32, y: u32| {
            let i = (y * row + x * 4) as usize;
            [data[i], data[i + 1], data[i + 2], data[i + 3]]
        };

        let left_edge = W - BAND as u32; // 160
                                         // Inside the band: opaque, top to bottom.
        for y in [1, H / 2, H - 2] {
            assert_eq!(
                px(left_edge + 8, y)[3],
                255,
                "the band must be filled at y={y}"
            );
        }
        // Outside it: never touched. The terminal owns every pixel left of the band, and
        // there is no scissor rect to save us — this is the assertion that would catch a
        // rect derived from the window instead of from the inset.
        for x in [0, 40, left_edge - 2] {
            assert_eq!(
                px(x, H / 2)[3],
                0,
                "the terminal's column at x={x} was painted"
            );
        }
        // Glyphs actually landed *in* the band: some pixel there is brighter than the
        // panel background, which a shapes-only frame could not produce.
        let bg = px(left_edge + 8, H / 2);
        let lit = (left_edge..W).any(|x| (0..H).any(|y| px(x, y)[1] > bg[1] + 24));
        assert!(lit, "no glyph coverage inside the band (background {bg:?})");
    }

    #[test]
    fn an_invisible_model_never_touches_gpu_state() {
        // The early-out the two tests above depend on being *conditional*.
        let (Some((device, queue)), Some(fonts)) = (crate::headless(), crate::FontSet::load().ok())
        else {
            eprintln!("skipping: no GPU/font");
            return;
        };
        let mut atlas = crate::atlas::Atlas::new(&device, &fonts, 24.0).expect("atlas");
        let view = offscreen(&device);
        let t = term_core::Terminal::new(10, 2, term_core::Color(0xFFFFFF), term_core::Color(0));
        let snap = t.snapshot();
        let sidebar_cfg = common::config::SidebarConfig::default();
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        let (mut layer, handle) = MonitorPanel::new();
        // Visible content, but hidden — and separately, visible with no band (R4.4).
        for (visible, band) in [(false, BAND), (true, 0.0)] {
            handle.set(MonitorModel {
                visible,
                ..filled()
            });
            let mut ctx = FrameCtx {
                device: &device,
                queue: &queue,
                encoder: &mut encoder,
                view: &view,
                viewport: [256.0, 128.0],
                cell: [12.0, 24.0],
                top_inset: 0.0,
                left_inset: 0.0,
                right_inset: band,
                format: wgpu::TextureFormat::Rgba8Unorm,
                sidebar: &sidebar_cfg,
                scale_factor: 1.0,
                snapshot: &snap,
                atlas: &mut atlas,
            };
            layer.record(&mut ctx);
            assert!(
                layer.gpu.is_none(),
                "visible={visible} band={band}: no GPU state may be built"
            );
        }
    }
}
