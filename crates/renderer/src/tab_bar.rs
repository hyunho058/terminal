//! The tab bar as a registered [`DrawLayer`] (D3/SD3, R2/R6).
//!
//! Draws a vertical **left sidebar** in the reserved left inset (see
//! [`crate::Renderer::set_left_inset`]): a full-height background band, tabs
//! stacked top→bottom in fixed-height slots, a highlight behind the active tab,
//! a right-edge border, and each tab's title text via the shared glyph
//! [`crate::atlas::Atlas`]. This lives entirely in its own module and registers
//! through `Renderer::add_layer` — the central renderer is never edited to add
//! it (SD3).
//!
//! The sidebar geometry ([`width_px`], [`tab_height_px`], [`slot_y`], [`tab_at`])
//! is exposed as pure free functions so the app's click hit-test consumes the
//! SAME math the layer draws with (SD2): `slot_y` is the forward slot placement
//! the draw path uses and `tab_at` is its exact inverse.
//!
//! Data flows one way (D6/SD2): the app writes a [`TabBarModel`] into a shared
//! handle each frame; this layer only *reads* it. The renderer never reaches into
//! `crates/app`.
//!
//! Text layout for *every* non-grid layer lives here (D10/R5.4): [`char_cols`],
//! [`advance_px`], [`pen_layout`], [`truncate_cols`] and [`layout_line`] are
//! `pub(crate)` and [`crate::overlay_layer`] consumes them, so the fallback +
//! width rules cannot drift into two copies.

use std::cell::RefCell;
use std::rc::Rc;

use bytemuck::{Pod, Zeroable};
use unicode_width::UnicodeWidthChar;
use wgpu::util::DeviceExt;

use crate::atlas::GlyphEntry;
use crate::{DrawLayer, FrameCtx};

/// The app-owned tab-bar state the layer renders (D6). Plain data — no GPU, no
/// app types.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TabBarModel {
    /// One title per tab, left to right.
    pub titles: Vec<String>,
    /// Index of the active (highlighted) tab.
    pub active: usize,
}

/// A cloneable write handle the app keeps to push new models each frame. The
/// layer holds the other end; single-threaded UI, so `Rc<RefCell<_>>` (the layer
/// is never `Send`).
#[derive(Clone, Default)]
pub struct TabModelHandle(Rc<RefCell<TabBarModel>>);

impl TabModelHandle {
    /// Replace the model the layer will draw next frame.
    pub fn set(&self, model: TabBarModel) {
        *self.0.borrow_mut() = model;
    }

    /// Snapshot the current model (used internally by the layer).
    fn snapshot(&self) -> TabBarModel {
        self.0.borrow().clone()
    }
}

// First-cut dark styling (Known Gap: visual design is refined later). Packed
// 0xRRGGBB; converted to linear at draw time to match the sRGB surface.
const STRIP_BG: u32 = 0x1A1F26;
const ACTIVE_BG: u32 = 0x2E3A4D;
const BORDER: u32 = 0x0A0D11;
const ACTIVE_FG: u32 = 0xE6E1CF;
const INACTIVE_FG: u32 = 0x8A93A0;

/// Horizontal padding inside each tab slot (before the title text), in px.
const PAD_X: f32 = 8.0;
/// Right-edge border thickness separating the sidebar from the grid, in px.
/// (The `BORDER` constant above is the border *color*, not a thickness.)
const BORDER_W: f32 = 1.0;

/// The z-order of the tab bar (above the terminal glyph pass = 0, below overlays).
pub const TAB_BAR_Z: i32 = 10;

/// Suggested sidebar **width** for a given cell width — the value the app feeds
/// to [`crate::Renderer::set_left_inset`]. ~14 columns wide so a title still has
/// room after [`PAD_X`] (D7/SD2 — one formula keeps the reserved band and the
/// drawn sidebar in lockstep).
pub fn width_px(cell_w: f32) -> f32 {
    (cell_w * 14.0).round().max(1.0)
}

/// Per-tab vertical **slot height** — the stride between stacked tabs.
pub fn tab_height_px(cell_h: f32) -> f32 {
    (cell_h * 1.5).round().max(1.0)
}

/// Legacy top-strip height helper. Retained (delegating to [`tab_height_px`])
/// only so the app keeps compiling until it migrates from the old top inset to
/// [`width_px`]/[`set_left_inset`] in the `tab-click-switch` feature.
///
/// [`set_left_inset`]: crate::Renderer::set_left_inset
pub fn height_px(cell_h: f32) -> f32 {
    tab_height_px(cell_h)
}

/// The top y (px) of tab `index`'s slot — the shared **forward** placement the
/// draw path uses. [`tab_at`] is its exact inverse: slots have no top pad or
/// inter-slot gap, so `slot_y(i)` and `floor(y / tab_h)` round-trip (SD2).
pub fn slot_y(index: usize, tab_h: f32) -> f32 {
    index as f32 * tab_h
}

/// Hit-test: the tab index at physical point `(x, y)` in a sidebar of width
/// `sidebar_w` with per-tab slot height `tab_h`, or `None` if the point is
/// outside the band (`x` outside `[0, sidebar_w)` or `y < 0`) or below the last
/// of `count` tabs. The exact inverse of [`slot_y`] (SD2) — the app's
/// click-to-switch routes through this so drawing and hit-testing never diverge.
pub fn tab_at(x: f32, y: f32, sidebar_w: f32, tab_h: f32, count: usize) -> Option<usize> {
    if x < 0.0 || x >= sidebar_w || y < 0.0 || tab_h <= 0.0 {
        return None;
    }
    let i = (y / tab_h).floor() as usize;
    (i < count).then_some(i)
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    corner: [f32; 2],
}

const QUAD: [Vertex; 6] = [
    Vertex { corner: [0.0, 0.0] },
    Vertex { corner: [1.0, 0.0] },
    Vertex { corner: [0.0, 1.0] },
    Vertex { corner: [0.0, 1.0] },
    Vertex { corner: [1.0, 0.0] },
    Vertex { corner: [1.0, 1.0] },
];

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniforms {
    viewport: [f32; 2],
    _pad: [f32; 2],
}

/// A solid-color quad in px (strip background, active highlight, border).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct RectInstance {
    rect: [f32; 4], // x, y, w, h in px (top-left origin)
    color: [f32; 4],
}

/// A textured glyph quad in px sampling the atlas coverage.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct TextInstance {
    rect: [f32; 4],
    uv0: [f32; 2],
    uv1: [f32; 2],
    color: [f32; 4],
}

const RECT_WGSL: &str = r#"
struct U { viewport: vec2<f32>, _pad: vec2<f32> };
@group(0) @binding(0) var<uniform> u: U;
struct VsIn {
    @location(0) corner: vec2<f32>,
    @location(1) rect: vec4<f32>,
    @location(2) color: vec4<f32>,
};
struct VsOut { @builtin(position) pos: vec4<f32>, @location(0) color: vec4<f32> };
@vertex
fn vs_main(in: VsIn) -> VsOut {
    let px = in.rect.xy + in.corner * in.rect.zw;
    let clip = vec2<f32>(px.x / u.viewport.x * 2.0 - 1.0, 1.0 - px.y / u.viewport.y * 2.0);
    var o: VsOut;
    o.pos = vec4<f32>(clip, 0.0, 1.0);
    o.color = in.color;
    return o;
}
@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> { return in.color; }
"#;

const TEXT_WGSL: &str = r#"
struct U { viewport: vec2<f32>, _pad: vec2<f32> };
@group(0) @binding(0) var<uniform> u: U;
@group(0) @binding(1) var atlas_tex: texture_2d<f32>;
@group(0) @binding(2) var atlas_samp: sampler;
struct VsIn {
    @location(0) corner: vec2<f32>,
    @location(1) rect: vec4<f32>,
    @location(2) uv0: vec2<f32>,
    @location(3) uv1: vec2<f32>,
    @location(4) color: vec4<f32>,
};
struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};
@vertex
fn vs_main(in: VsIn) -> VsOut {
    let px = in.rect.xy + in.corner * in.rect.zw;
    let clip = vec2<f32>(px.x / u.viewport.x * 2.0 - 1.0, 1.0 - px.y / u.viewport.y * 2.0);
    var o: VsOut;
    o.pos = vec4<f32>(clip, 0.0, 1.0);
    o.uv = mix(in.uv0, in.uv1, in.corner);
    o.color = in.color;
    return o;
}
@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let a = textureSample(atlas_tex, atlas_samp, in.uv).r;
    return vec4<f32>(in.color.rgb * a, a); // premultiplied
}
"#;

/// GPU resources built once on first draw (needs a device + target format).
struct Gpu {
    format: wgpu::TextureFormat,
    quad: wgpu::Buffer,
    uniform: wgpu::Buffer,
    rect_pipeline: wgpu::RenderPipeline,
    rect_bind: wgpu::BindGroup,
    text_pipeline: wgpu::RenderPipeline,
    text_bind: wgpu::BindGroup,
}

/// The tab-bar draw layer. Register one via `Renderer::add_layer`.
pub struct TabBar {
    model: TabModelHandle,
    gpu: Option<Gpu>,
}

impl TabBar {
    /// Create the layer and a write handle for the app. Register the layer with
    /// `Renderer::add_layer(Box::new(tab_bar))`; keep the handle to push models.
    pub fn new() -> (TabBar, TabModelHandle) {
        let model = TabModelHandle::default();
        (
            TabBar {
                model: model.clone(),
                gpu: None,
            },
            model,
        )
    }

    fn ensure_gpu(&mut self, ctx: &FrameCtx<'_>) {
        if self.gpu.as_ref().map(|g| g.format) == Some(ctx.format) {
            return;
        }
        let device = ctx.device;

        let quad = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("tab-quad"),
            contents: bytemuck::cast_slice(&QUAD),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("tab-uniform"),
            contents: bytemuck::bytes_of(&Uniforms {
                viewport: ctx.viewport,
                _pad: [0.0, 0.0],
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        // Rect pipeline (uniform only).
        let rect_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("tab-rect-bgl"),
            entries: &[uniform_entry(0)],
        });
        let rect_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("tab-rect-bg"),
            layout: &rect_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });
        let rect_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("tab-rect"),
            source: wgpu::ShaderSource::Wgsl(RECT_WGSL.into()),
        });
        let rect_pipeline = build_pipeline(
            device,
            &rect_shader,
            &rect_bgl,
            ctx.format,
            std::mem::size_of::<RectInstance>() as u64,
            &wgpu::vertex_attr_array![1 => Float32x4, 2 => Float32x4],
            wgpu::BlendState::ALPHA_BLENDING,
        );

        // Text pipeline (uniform + atlas texture + sampler).
        let text_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("tab-text-bgl"),
            entries: &[
                uniform_entry(0),
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
        let text_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("tab-text-bg"),
            layout: &text_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(ctx.atlas.view()),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(ctx.atlas.sampler()),
                },
            ],
        });
        let text_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("tab-text"),
            source: wgpu::ShaderSource::Wgsl(TEXT_WGSL.into()),
        });
        let text_pipeline = build_pipeline(
            device,
            &text_shader,
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
            rect_pipeline,
            rect_bind,
            text_pipeline,
            text_bind,
        });
    }

    /// Build the sidebar strip / active-slot highlight / right-edge border rects
    /// for the current model + geometry. `w` = sidebar width, `vh` = viewport
    /// height, `tab_h` = per-slot height.
    fn rects(&self, model: &TabBarModel, w: f32, vh: f32, tab_h: f32) -> Vec<RectInstance> {
        let mut out = vec![RectInstance {
            rect: [0.0, 0.0, w, vh],
            color: linear(STRIP_BG, 1.0),
        }];
        if model.active < model.titles.len() {
            out.push(RectInstance {
                rect: [0.0, slot_y(model.active, tab_h), w, tab_h],
                color: linear(ACTIVE_BG, 1.0),
            });
        }
        // Right-edge border separating the sidebar from the grid.
        out.push(RectInstance {
            rect: [w - BORDER_W, 0.0, BORDER_W, vh],
            color: linear(BORDER, 1.0),
        });
        out
    }

    /// Lay out each tab's title as textured glyph quads — stacked top→bottom, one
    /// title per slot, drawn left-to-right indented by [`PAD_X`], vertically
    /// centered within its slot, and truncated to the sidebar width `w`.
    ///
    /// Layout is **column-based**, not char-based (D10/R5.2-R5.3): a Hangul cwd
    /// basename is 2 columns per char, so a fixed per-char advance overlapped the
    /// glyphs and a char-counting truncate overflowed the 14-column sidebar. Both
    /// go through the shared [`truncate_cols`]/[`layout_line`] helpers (R5.4).
    fn text(
        &self,
        ctx: &mut FrameCtx<'_>,
        model: &TabBarModel,
        w: f32,
        tab_h: f32,
    ) -> Vec<TextInstance> {
        let (cw, ch) = (ctx.cell[0], ctx.cell[1]);
        if cw <= 0.0 || model.titles.is_empty() {
            return Vec::new();
        }
        // A **column** budget (R5.3): how many cell-widths of title fit between
        // the two PAD_X gutters. Formerly named `max_chars`, which only meant the
        // same thing while every char was assumed 1 column wide.
        let max_cols = (((w - 2.0 * PAD_X) / cw).floor() as i32).max(0) as usize;
        let ascent = ch * 0.8;

        let mut out = Vec::new();
        for (i, title) in model.titles.iter().enumerate() {
            let fg = if i == model.active {
                ACTIVE_FG
            } else {
                INACTIVE_FG
            };
            let color = linear(fg, 1.0);
            // Vertically center the title within its own slot (not the whole strip).
            let text_top = slot_y(i, tab_h) + ((tab_h - ch) * 0.5).max(0.0);
            let baseline = text_top + ascent;
            let fitted = truncate_cols(title, max_cols);
            layout_line(ctx, &fitted, PAD_X, cw, |pen, e| {
                out.push(TextInstance {
                    rect: [pen + e.left, baseline - e.top, e.px_w, e.px_h],
                    uv0: e.uv0,
                    uv1: e.uv1,
                    color,
                });
            });
        }
        out
    }
}

impl DrawLayer for TabBar {
    fn z_order(&self) -> i32 {
        TAB_BAR_Z
    }

    fn record(&mut self, ctx: &mut FrameCtx<'_>) {
        let w = ctx.left_inset;
        if w <= 0.0 {
            return; // no reserved left band → nothing to draw
        }
        self.ensure_gpu(ctx);
        let model = self.model.snapshot();
        let vh = ctx.viewport[1];
        let tab_h = tab_height_px(ctx.cell[1]);

        let rects = self.rects(&model, w, vh, tab_h);
        let texts = self.text(ctx, &model, w, tab_h);

        let gpu = self.gpu.as_ref().expect("gpu built in ensure_gpu");
        ctx.queue.write_buffer(
            &gpu.uniform,
            0,
            bytemuck::bytes_of(&Uniforms {
                viewport: ctx.viewport,
                _pad: [0.0, 0.0],
            }),
        );
        let rect_buf = ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("tab-rects"),
                contents: bytemuck::cast_slice(&rects),
                usage: wgpu::BufferUsages::VERTEX,
            });
        let text_buf = ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("tab-texts"),
                contents: bytemuck::cast_slice(&texts),
                usage: wgpu::BufferUsages::VERTEX,
            });

        let mut pass = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("tab-bar-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: ctx.view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load, // composite over the terminal
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        if !rects.is_empty() {
            pass.set_pipeline(&gpu.rect_pipeline);
            pass.set_bind_group(0, &gpu.rect_bind, &[]);
            pass.set_vertex_buffer(0, gpu.quad.slice(..));
            pass.set_vertex_buffer(1, rect_buf.slice(..));
            pass.draw(0..QUAD.len() as u32, 0..rects.len() as u32);
        }
        if !texts.is_empty() {
            pass.set_pipeline(&gpu.text_pipeline);
            pass.set_bind_group(0, &gpu.text_bind, &[]);
            pass.set_vertex_buffer(0, gpu.quad.slice(..));
            pass.set_vertex_buffer(1, text_buf.slice(..));
            pass.draw(0..QUAD.len() as u32, 0..texts.len() as u32);
        }
    }
}

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

fn uniform_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn build_pipeline(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    bgl: &wgpu::BindGroupLayout,
    format: wgpu::TextureFormat,
    instance_stride: u64,
    instance_attrs: &[wgpu::VertexAttribute],
    blend: wgpu::BlendState,
) -> wgpu::RenderPipeline {
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("tab-layout"),
        bind_group_layouts: &[Some(bgl)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("tab-pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[
                Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2],
                }),
                Some(wgpu::VertexBufferLayout {
                    array_stride: instance_stride,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: instance_attrs,
                }),
            ],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(blend),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

/// Packed 0xRRGGBB → linear-light RGBA (matches the sRGB surface, like the cells).
fn linear(packed: u32, a: f32) -> [f32; 4] {
    let [r, g, b] = term_core::Color(packed).to_linear_f32();
    [r, g, b, a]
}

// ---------------------------------------------------------------------------
// Shared layer-text layout (D10/R5) — used by this layer AND `overlay_layer`.
//
// Every non-grid text path routes through these four functions so the fallback
// rule and the width rule exist exactly once (R5.4). Duplicating them per layer
// is what let "Hangul tab title draws `.notdef`" survive as its own bug.
// ---------------------------------------------------------------------------

/// Display width of `c` in terminal **columns** (R5.2).
///
/// `unicode-width` (same crate + version `term-core` uses, so the engine and the
/// renderer can never disagree) reports 2 for Hangul/CJK. `None` means a control
/// char, which we lay out as one column rather than collapsing it.
pub(crate) fn char_cols(c: char) -> usize {
    c.width().unwrap_or(1)
}

/// Pen advance in px for `c` at cell width `cell_w` (R5.2) — the replacement for
/// the old fixed `pen += cell_w`, which overlapped every wide glyph.
pub(crate) fn advance_px(c: char, cell_w: f32) -> f32 {
    char_cols(c) as f32 * cell_w
}

/// `(char, pen_x)` for each char of `s`, starting at `start_x` and advancing by
/// [`advance_px`] (R5.2). Pure and allocation-free so the advance rule is unit
/// testable without a GPU.
pub(crate) fn pen_layout(
    s: &str,
    start_x: f32,
    cell_w: f32,
) -> impl Iterator<Item = (char, f32)> + '_ {
    let mut pen = start_x;
    s.chars().map(move |c| {
        let at = pen;
        pen += advance_px(c, cell_w);
        (c, at)
    })
}

/// The prefix of `s` that fits in a **column** budget of `max_cols` (R5.3).
///
/// Counts display width, not chars: `truncate_cols("한글abc", 5) == "한글a"`
/// (2+2+1). A char is taken only when it fits whole, so a wide char is never
/// half-drawn past the budget and a codepoint is never split.
pub(crate) fn truncate_cols(s: &str, max_cols: usize) -> String {
    let mut used = 0usize;
    let mut out = String::new();
    for c in s.chars() {
        let w = char_cols(c);
        if used + w > max_cols {
            break;
        }
        used += w;
        out.push(c);
    }
    out
}

/// Lay `s` out left-to-right from `start_x`, calling `emit(pen_x, entry)` once
/// per drawable char (R5.1/R5.4).
///
/// Glyph lookup goes through [`Atlas::resolve`](crate::atlas::Atlas::resolve) +
/// `get_or_insert_styled`, so layers pick up the font-fallback chain (Hangul
/// resolves to the fallback face instead of the primary's `.notdef`). A char no
/// face covers emits nothing **but still advances the pen**, keeping the rest of
/// the line in its columns.
pub(crate) fn layout_line(
    ctx: &mut FrameCtx<'_>,
    s: &str,
    start_x: f32,
    cell_w: f32,
    mut emit: impl FnMut(f32, GlyphEntry),
) {
    for (c, pen) in pen_layout(s, start_x, cell_w) {
        if let Some(key) = ctx.atlas.resolve(c) {
            if let Some(e) = ctx.atlas.get_or_insert_styled(ctx.queue, key) {
                emit(pen, e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handle_roundtrips_model() {
        let (_bar, handle) = TabBar::new();
        handle.set(TabBarModel {
            titles: vec!["a".into(), "b".into()],
            active: 1,
        });
        assert_eq!(handle.snapshot().active, 1);
        assert_eq!(handle.snapshot().titles.len(), 2);
    }

    #[test]
    fn rects_are_vertical_strip_highlight_and_right_border() {
        let (bar, _h) = TabBar::new();
        let model = TabBarModel {
            titles: vec!["one".into(), "two".into(), "three".into()],
            active: 2,
        };
        // sidebar width 160, viewport height 540, slot height 30.
        let rects = bar.rects(&model, 160.0, 540.0, 30.0);
        // strip + active highlight + right border.
        assert_eq!(rects.len(), 3);
        // strip spans the full sidebar height.
        assert_eq!(rects[0].rect, [0.0, 0.0, 160.0, 540.0]);
        // highlight sits over slot 2: y = slot_y(2, 30) = 60, full width, one slot tall.
        assert_eq!(rects[1].rect, [0.0, 60.0, 160.0, 30.0]);
        // border is a vertical line on the right edge (thickness BORDER_W, full height).
        assert_eq!(rects[2].rect, [160.0 - BORDER_W, 0.0, BORDER_W, 540.0]);
    }

    #[test]
    fn no_highlight_when_active_out_of_range() {
        let (bar, _h) = TabBar::new();
        let model = TabBarModel {
            titles: vec!["only".into()],
            active: 5, // stale index
        };
        let rects = bar.rects(&model, 160.0, 540.0, 30.0);
        // strip + right border only (no highlight for the out-of-range active tab).
        assert_eq!(rects.len(), 2);
    }

    #[test]
    fn width_and_height_scale_with_cell_and_leave_title_room() {
        assert!(width_px(20.0) > width_px(10.0));
        assert!(width_px(8.0) >= 1.0);
        assert!(tab_height_px(20.0) > tab_height_px(10.0));
        assert!(tab_height_px(20.0) >= 1.0);
        // height_px is retained and equals tab_height_px (legacy delegate).
        assert_eq!(height_px(20.0), tab_height_px(20.0));
        // The sidebar leaves room for >= 8 columns after PAD_X for a typical cell
        // (R3.1). Columns, not chars — a Hangul title spends 2 per char (R5.2).
        let cw = 8.0;
        let max_cols = ((width_px(cw) - 2.0 * PAD_X) / cw).floor() as i32;
        assert!(
            max_cols >= 8,
            "sidebar must fit >= 8 columns, got {max_cols}"
        );
    }

    #[test]
    fn tab_at_hits_slots_and_rejects_outside() {
        // inside slot 2 (y in [60, 90)), x within the band → Some(2).
        assert_eq!(tab_at(40.0, 65.0, 160.0, 30.0, 3), Some(2));
        // x past the sidebar width.
        assert_eq!(tab_at(200.0, 65.0, 160.0, 30.0, 3), None);
        // y below the last slot (only 3 tabs → slots 0..90).
        assert_eq!(tab_at(40.0, 500.0, 160.0, 30.0, 3), None);
        // y above the strip (negative) must not cast to slot 0.
        assert_eq!(tab_at(40.0, -5.0, 160.0, 30.0, 3), None);
        // x left of the band (negative).
        assert_eq!(tab_at(-1.0, 10.0, 160.0, 30.0, 3), None);
        // no tabs → never a hit.
        assert_eq!(tab_at(40.0, 10.0, 160.0, 30.0, 0), None);
    }

    #[test]
    fn slot_y_and_tab_at_are_exact_inverses() {
        // R3.4: a click at the vertical center of each drawn slot returns that slot.
        let (w, tab_h, n) = (160.0, 30.0, 3usize);
        for i in 0..n {
            let y = slot_y(i, tab_h) + tab_h * 0.5;
            assert_eq!(tab_at(80.0, y, w, tab_h, n), Some(i), "slot {i} round-trip");
        }
    }

    #[test]
    fn truncate_cols_spends_a_column_budget_not_a_char_count() {
        // R5.3: the spec's exact case — 2 + 2 + 1 = 5 columns.
        assert_eq!(truncate_cols("한글abc", 5), "한글a");
        // ASCII: one column each, so it still behaves like the old char count.
        assert_eq!(truncate_cols("hello", 3), "hel");
        assert_eq!(truncate_cols("hi", 9), "hi");
        // Never splits a codepoint (multi-byte but 1 column).
        assert_eq!(truncate_cols("café", 4), "café");
        // A budget that would split a wide char drops it whole rather than
        // letting it bleed one column past the sidebar.
        assert_eq!(truncate_cols("한글", 3), "한");
        assert_eq!(truncate_cols("한", 1), "");
        // Edges: empty input, zero budget.
        assert_eq!(truncate_cols("", 5), "");
        assert_eq!(truncate_cols("한글abc", 0), "");
    }

    #[test]
    fn pen_advances_by_display_width() {
        // R5.2: `프` is 2 columns, `a` is 1 — the pen positions are 0 then 2*cw,
        // not the old fixed 1*cw stride that overlapped the two glyphs.
        let cw = 8.0;
        let xs: Vec<f32> = pen_layout("프a", 0.0, cw).map(|(_, x)| x).collect();
        assert_eq!(xs, vec![0.0, 2.0 * cw]);
        // The advance past the whole string is 3 columns.
        let end = pen_layout("프a", 0.0, cw)
            .map(|(c, _)| advance_px(c, cw))
            .sum::<f32>();
        assert_eq!(end, 3.0 * cw);
        // A start offset (PAD_X in the draw path) shifts every pen.
        let xs: Vec<f32> = pen_layout("프a", PAD_X, cw).map(|(_, x)| x).collect();
        assert_eq!(xs, vec![PAD_X, PAD_X + 2.0 * cw]);
    }

    #[test]
    fn char_cols_matches_the_grid_width_rule() {
        assert_eq!(char_cols('a'), 1);
        assert_eq!(char_cols('프'), 2);
        // Control chars have no width; lay them out as one column, not zero.
        assert_eq!(char_cols('\u{7}'), 1);
    }
}
