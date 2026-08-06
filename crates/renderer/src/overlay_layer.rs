//! A generic translucent text-panel overlay as a registered [`DrawLayer`] (SD3).
//!
//! Used by ai-monitor (R2.2): the app pushes a [`OverlayModel`] (visible flag +
//! text lines) into a shared handle each time its data changes; this layer draws
//! a dimmed panel over the grid with the lines when visible. Renderer-owned and
//! app-state-free (SD2) — it only reads plain data the app hands down.
//!
//! Text layout is **not** duplicated here (D10/R5.4): [`truncate_cols`] and
//! [`layout_line`] are the same helpers the tab bar draws with, so font fallback
//! and the wide-char advance stay one rule instead of two that drift apart.

use std::cell::RefCell;
use std::rc::Rc;

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use crate::tab_bar::{layout_line, truncate_cols};
use crate::{DrawLayer, FrameCtx};

/// Plain data the overlay renders (D6-style one-way flow).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OverlayModel {
    pub visible: bool,
    /// Lines to show, oldest first (newest drawn at the bottom).
    pub lines: Vec<String>,
}

/// A cloneable write handle the app keeps to push new models.
#[derive(Clone, Default)]
pub struct OverlayHandle(Rc<RefCell<OverlayModel>>);

impl OverlayHandle {
    pub fn set(&self, model: OverlayModel) {
        *self.0.borrow_mut() = model;
    }
    fn snapshot(&self) -> OverlayModel {
        self.0.borrow().clone()
    }
}

const PANEL_BG: [f32; 4] = [0.04, 0.05, 0.07, 0.88]; // translucent dark
const TEXT_FG: u32 = 0xE6E1CF;
const PAD: f32 = 10.0;

/// The log pane's rect: the lower 55% of the window, never above `top_inset`. Pure, so
/// the sizing is unit-tested without a GPU.
fn panel_rect(viewport: [f32; 2], top_inset: f32) -> [f32; 4] {
    let (vw, vh) = (viewport[0], viewport[1]);
    let top = (vh * 0.45).max(top_inset);
    [0.0, top, vw, vh - top]
}

/// z-order: above the tab bar (10), a genuine overlay.
pub const OVERLAY_Z: i32 = 100;

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
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct RectInstance {
    rect: [f32; 4],
    color: [f32; 4],
}
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
struct VsIn { @location(0) corner: vec2<f32>, @location(1) rect: vec4<f32>, @location(2) color: vec4<f32> };
struct VsOut { @builtin(position) pos: vec4<f32>, @location(0) color: vec4<f32> };
@vertex fn vs_main(in: VsIn) -> VsOut {
    let px = in.rect.xy + in.corner * in.rect.zw;
    let clip = vec2<f32>(px.x / u.viewport.x * 2.0 - 1.0, 1.0 - px.y / u.viewport.y * 2.0);
    var o: VsOut; o.pos = vec4<f32>(clip, 0.0, 1.0); o.color = in.color; return o;
}
@fragment fn fs_main(in: VsOut) -> @location(0) vec4<f32> { return in.color; }
"#;

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

struct Gpu {
    format: wgpu::TextureFormat,
    quad: wgpu::Buffer,
    uniform: wgpu::Buffer,
    rect_pipeline: wgpu::RenderPipeline,
    rect_bind: wgpu::BindGroup,
    text_pipeline: wgpu::RenderPipeline,
    /// Kept (not dropped after pipeline creation) so [`OverlayLayer::ensure_text_bind`]
    /// can build a *replacement* bind group on a later frame — D3 rule 1.
    text_bgl: wgpu::BindGroupLayout,
    text_bind: wgpu::BindGroup,
    /// `(format, atlas epoch)` the live `text_bind` was built against (D2).
    ///
    /// It lives **inside `Gpu`** on purpose (D3 rule 2): `ensure_gpu` replaces the
    /// whole struct on a format change, so the key resets to `None` alongside the
    /// `uniform` buffer it binds — a cached bind group pointing at a replaced
    /// buffer is structurally impossible rather than merely unlikely.
    ///
    /// `generation` and the cell size are deliberately *not* in the key: eviction
    /// reuses the same texture (so a rebuild would be byte-identical) and the cell
    /// size never enters the bind group at all. Keying on either would churn bind
    /// groups at idle — the G1 regression.
    text_bind_key: Option<(wgpu::TextureFormat, u64)>,
    /// How many times `text_bind` has actually been (re)built — see the twin field on
    /// `SidebarTextPipeline`. Key equality alone cannot express "rebuilds = 0 while the
    /// atlas is unchanged": that assertion still passes with the short-circuit deleted
    /// and the bind group rebuilt every frame, the G1 idle-CPU regression this spec
    /// forbids. `#[cfg(test)]`, so the shipped binary is unaffected.
    #[cfg(test)]
    rebuilds: u32,
}

/// The overlay draw layer.
pub struct OverlayLayer {
    model: OverlayHandle,
    gpu: Option<Gpu>,
}

impl OverlayLayer {
    pub fn new() -> (OverlayLayer, OverlayHandle) {
        let model = OverlayHandle::default();
        (Self::from_handle(model.clone()), model)
    }

    /// Build a layer bound to an existing handle (so an owner — e.g. the
    /// ai-monitor overlay — keeps the write handle and hands the layer to the
    /// renderer via the SD4 `Overlay::layer` seam).
    pub fn from_handle(model: OverlayHandle) -> OverlayLayer {
        OverlayLayer { model, gpu: None }
    }

    fn ensure_gpu(&mut self, ctx: &FrameCtx<'_>) {
        if self.gpu.as_ref().map(|g| g.format) == Some(ctx.format) {
            return;
        }
        let device = ctx.device;
        let quad = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("ov-quad"),
            contents: bytemuck::cast_slice(&QUAD),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("ov-uniform"),
            contents: bytemuck::bytes_of(&Uniforms {
                viewport: ctx.viewport,
                _pad: [0.0, 0.0],
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let rect_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ov-rect-bgl"),
            entries: &[uniform_entry(0)],
        });
        let rect_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ov-rect-bg"),
            layout: &rect_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });
        let rect_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ov-rect"),
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
        let text_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ov-text-bgl"),
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
        let text_bind = build_text_bind(device, &text_bgl, &uniform, ctx.atlas);
        let text_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ov-text"),
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
            text_bgl,
            text_bind,
            text_bind_key: Some((ctx.format, ctx.atlas.epoch())),
            // The eager build in `ensure_gpu` counts as the first rebuild, so an idle
            // test asserting `rebuilds == 1` covers the whole record path.
            #[cfg(test)]
            rebuilds: 1,
        });
    }

    /// Rebuild the atlas bind group when — and *only* when — the texture it samples
    /// has been replaced (D3).
    ///
    /// `ensure_gpu` is gated on the surface format alone, which never changes at
    /// runtime; a live config reload swaps the whole `Atlas` (`Renderer::apply_config`)
    /// and leaves the format untouched, so without this the overlay would keep
    /// sampling the replaced texture with UVs computed against the new one — wrong
    /// glyphs for the rest of the session. Splitting only the bind group out keeps
    /// pipelines and shader modules (which a swap does *not* invalidate) format-gated.
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

    /// Lay out the last-N lines as glyph quads inside the panel rect.
    ///
    /// Clipping and advance are **column**-based (R5.2/R5.3/R5.4): an overlay line
    /// containing Hangul spends 2 columns per char, so the shared helpers keep it
    /// inside the panel instead of overlapping glyphs and running off the edge.
    fn text(&self, ctx: &mut FrameCtx<'_>, lines: &[String], panel: [f32; 4]) -> Vec<TextInstance> {
        let (cw, ch) = (ctx.cell[0], ctx.cell[1]);
        if cw <= 0.0 {
            return Vec::new();
        }
        let line_h = ch * 1.15;
        let max_lines = (((panel[3] - 2.0 * PAD) / line_h).floor() as i32).max(0) as usize;
        // A **column** budget, not a char count (R5.3) — see `truncate_cols`.
        let max_cols = (((panel[2] - 2.0 * PAD) / cw).floor() as i32).max(1) as usize;
        let ascent = ch * 0.8;
        let color = linear(TEXT_FG, 1.0);
        // Show the last `max_lines` (newest at the bottom).
        let start = lines.len().saturating_sub(max_lines);
        let mut out = Vec::new();
        for (row, line) in lines[start..].iter().enumerate() {
            let base_y = panel[1] + PAD + row as f32 * line_h + ascent;
            let fitted = truncate_cols(line, max_cols);
            layout_line(ctx, &fitted, panel[0] + PAD, cw, |pen, e| {
                out.push(TextInstance {
                    rect: [pen + e.left, base_y - e.top, e.px_w, e.px_h],
                    uv0: e.uv0,
                    uv1: e.uv1,
                    color,
                });
            });
        }
        out
    }
}

impl DrawLayer for OverlayLayer {
    fn z_order(&self) -> i32 {
        OVERLAY_Z
    }

    fn record(&mut self, ctx: &mut FrameCtx<'_>) {
        let model = self.model.snapshot();
        if !model.visible {
            return;
        }
        self.ensure_gpu(ctx);
        // Order matters (D3 rule 3): `ensure_text_bind` builds from the `text_bgl`
        // `ensure_gpu` stores.
        self.ensure_text_bind(ctx);
        let (vw, vh) = (ctx.viewport[0], ctx.viewport[1]);
        let panel = panel_rect([vw, vh], ctx.top_inset);

        let rects = [RectInstance {
            rect: panel,
            color: PANEL_BG,
        }];
        let texts = self.text(ctx, &model.lines, panel);

        let gpu = self.gpu.as_ref().expect("gpu built");
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
                label: Some("ov-rects"),
                contents: bytemuck::cast_slice(&rects),
                usage: wgpu::BufferUsages::VERTEX,
            });
        let text_buf = ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("ov-texts"),
                contents: bytemuck::cast_slice(&texts),
                usage: wgpu::BufferUsages::VERTEX,
            });

        let mut pass = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("overlay-pass"),
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
        pass.set_pipeline(&gpu.rect_pipeline);
        pass.set_bind_group(0, &gpu.rect_bind, &[]);
        pass.set_vertex_buffer(0, gpu.quad.slice(..));
        pass.set_vertex_buffer(1, rect_buf.slice(..));
        pass.draw(0..QUAD.len() as u32, 0..rects.len() as u32);
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
        label: Some("ov-text-bg"),
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
        label: Some("ov-layout"),
        bind_group_layouts: &[Some(bgl)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("ov-pipeline"),
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

fn linear(packed: u32, a: f32) -> [f32; 4] {
    let [r, g, b] = term_core::Color(packed).to_linear_f32();
    [r, g, b, a]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handle_roundtrips_visibility_and_lines() {
        let (_layer, h) = OverlayLayer::new();
        assert!(!h.snapshot().visible);
        h.set(OverlayModel {
            visible: true,
            lines: vec!["a".into(), "b".into()],
        });
        assert!(h.snapshot().visible);
        assert_eq!(h.snapshot().lines.len(), 2);
    }

    #[test]
    fn log_pane_is_the_lower_55_percent_below_the_top_inset() {
        // The ai-monitor's pane: full width, lower 55% of the window.
        assert_eq!(panel_rect([800.0, 600.0], 0.0), [0.0, 270.0, 800.0, 330.0]);
        // The top inset wins when it would otherwise overlap the tab bar.
        assert_eq!(panel_rect([800.0, 600.0], 400.0)[1], 400.0);
    }

    #[test]
    fn shares_the_tab_bar_text_rules() {
        // R5.4: the overlay clips and advances with the *same* helpers the tab bar
        // draws with — one rule, not two copies that can drift. Asserting them
        // through this module's own imports is what pins that sharing.
        assert_eq!(truncate_cols("한글abc", 5), "한글a");
        let cw = 8.0;
        let xs: Vec<f32> = crate::tab_bar::pen_layout("프a", PAD, cw)
            .map(|(_, x)| x)
            .collect();
        assert_eq!(xs, vec![PAD, PAD + 2.0 * cw]);
    }

    #[test]
    fn z_order_is_above_the_tab_bar() {
        let (layer, _h) = OverlayLayer::new();
        assert!(layer.z_order() > super::super::tab_bar::TAB_BAR_Z);
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

    /// A layer whose model clears the `!visible` early-out (R5.3): without a visible
    /// model and a non-empty line, `record` returns before touching any GPU state and
    /// every assertion below would pass vacuously.
    fn visible_layer() -> OverlayLayer {
        let (layer, handle) = OverlayLayer::new();
        handle.set(OverlayModel {
            visible: true,
            lines: vec!["ai-monitor log line".into()],
        });
        layer
    }

    fn bind_key(layer: &OverlayLayer) -> Option<(wgpu::TextureFormat, u64)> {
        layer
            .gpu
            .as_ref()
            .expect("recording must build gpu state")
            .text_bind_key
    }

    /// R0.2/R3.5: replacing the atlas — what `Renderer::apply_config` does on every
    /// live config reload — must invalidate the bind group that samples it. Before
    /// this fix `ensure_gpu`'s format-only gate meant the bind group was built once
    /// and never again, so the overlay sampled the *replaced* texture with UVs
    /// computed against the new one.
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

        let frame = |layer: &mut OverlayLayer,
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
        assert!(k1.is_some(), "a visible overlay must build its bind group");

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

    /// R4.2 / G1: the flip side — an unchanged atlas must *not* churn bind groups
    /// every frame. (Scope note: the per-frame instance-buffer `create_buffer_init`
    /// calls are existing behaviour and outside this contract; only the bind group
    /// is asserted stable.)
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
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    sidebar: &sidebar_cfg,
                    scale_factor: 1.0,
                    snapshot: &snap,
                    atlas: &mut atlas,
                };
                layer.record(&mut ctx);
            }
            let key = bind_key(&layer);
            assert!(key.is_some(), "a visible overlay must build its bind group");
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
}
