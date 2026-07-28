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
    text_bind: wgpu::BindGroup,
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
        let text_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ov-text-bg"),
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
            text_bind,
        });
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
}
