//! The tab bar as a registered [`DrawLayer`] (D3/SD3, R2/R6).
//!
//! Draws a strip in the reserved top inset (see [`crate::Renderer::set_top_inset`]):
//! a background band, a highlight behind the active tab, and each tab's title
//! text via the shared glyph [`crate::atlas::Atlas`]. This lives entirely in its
//! own module and registers through `Renderer::add_layer` — the central renderer
//! is never edited to add it (SD3).
//!
//! Data flows one way (D6/SD2): the app writes a [`TabBarModel`] into a shared
//! handle each frame; this layer only *reads* it. The renderer never reaches into
//! `crates/app`.

use std::cell::RefCell;
use std::rc::Rc;

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

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

/// Horizontal padding inside each tab segment, in px.
const PAD_X: f32 = 8.0;
/// Bottom border thickness separating the strip from the grid, in px.
const BORDER_H: f32 = 1.0;

/// The z-order of the tab bar (above the terminal glyph pass = 0, below overlays).
pub const TAB_BAR_Z: i32 = 10;

/// Suggested strip height for a given cell height (app sets the inset to this,
/// the layer draws to `ctx.top_inset` — one formula keeps them in lockstep).
pub fn height_px(cell_h: f32) -> f32 {
    (cell_h * 1.5).round().max(1.0)
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

    /// Build the strip/highlight/border rects for the current model + geometry.
    fn rects(&self, model: &TabBarModel, vw: f32, h: f32) -> Vec<RectInstance> {
        let mut out = vec![RectInstance {
            rect: [0.0, 0.0, vw, h],
            color: linear(STRIP_BG, 1.0),
        }];
        let n = model.titles.len().max(1);
        let tabw = vw / n as f32;
        if model.active < model.titles.len() {
            out.push(RectInstance {
                rect: [model.active as f32 * tabw, 0.0, tabw, h],
                color: linear(ACTIVE_BG, 1.0),
            });
        }
        // Bottom border separating the strip from the grid.
        out.push(RectInstance {
            rect: [0.0, h - BORDER_H, vw, BORDER_H],
            color: linear(BORDER, 1.0),
        });
        out
    }

    /// Lay out each tab's title as textured glyph quads (left-to-right, monospace
    /// advance, vertically centered, truncated to its segment).
    fn text(
        &self,
        ctx: &mut FrameCtx<'_>,
        model: &TabBarModel,
        vw: f32,
        h: f32,
    ) -> Vec<TextInstance> {
        let (cw, ch) = (ctx.cell[0], ctx.cell[1]);
        if cw <= 0.0 || model.titles.is_empty() {
            return Vec::new();
        }
        let tabw = vw / model.titles.len() as f32;
        let max_chars = (((tabw - 2.0 * PAD_X) / cw).floor() as i32).max(0) as usize;
        let ascent = ch * 0.8;
        let text_top = ((h - ch) * 0.5).max(0.0);
        let baseline = text_top + ascent;

        let mut out = Vec::new();
        for (i, title) in model.titles.iter().enumerate() {
            let fg = if i == model.active {
                ACTIVE_FG
            } else {
                INACTIVE_FG
            };
            let color = linear(fg, 1.0);
            let mut pen = i as f32 * tabw + PAD_X;
            for c in truncate(title, max_chars).chars() {
                let gid = ctx.atlas.glyph_id(c);
                if let Some(e) = ctx.atlas.get_or_insert(ctx.queue, gid) {
                    out.push(TextInstance {
                        rect: [pen + e.left, baseline - e.top, e.px_w, e.px_h],
                        uv0: e.uv0,
                        uv1: e.uv1,
                        color,
                    });
                }
                pen += cw;
            }
        }
        out
    }
}

impl DrawLayer for TabBar {
    fn z_order(&self) -> i32 {
        TAB_BAR_Z
    }

    fn record(&mut self, ctx: &mut FrameCtx<'_>) {
        let h = ctx.top_inset;
        if h <= 0.0 {
            return; // no reserved band → nothing to draw
        }
        self.ensure_gpu(ctx);
        let model = self.model.snapshot();
        let vw = ctx.viewport[0];

        let rects = self.rects(&model, vw, h);
        let texts = self.text(ctx, &model, vw, h);

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

/// First `max` chars of `s` (char-aware, never splits a codepoint).
fn truncate(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
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
    fn rects_include_strip_highlight_and_border() {
        let (bar, _h) = TabBar::new();
        let model = TabBarModel {
            titles: vec!["one".into(), "two".into()],
            active: 1,
        };
        let rects = bar.rects(&model, 800.0, 30.0);
        // strip + active highlight + border.
        assert_eq!(rects.len(), 3);
        // strip spans full width.
        assert_eq!(rects[0].rect, [0.0, 0.0, 800.0, 30.0]);
        // highlight sits over the 2nd tab (x = 400 for a 2-tab / 800px bar).
        assert_eq!(rects[1].rect[0], 400.0);
        assert_eq!(rects[1].rect[2], 400.0);
    }

    #[test]
    fn no_highlight_when_active_out_of_range() {
        let (bar, _h) = TabBar::new();
        let model = TabBarModel {
            titles: vec!["only".into()],
            active: 5, // stale index
        };
        let rects = bar.rects(&model, 400.0, 30.0);
        // strip + border only (no highlight for the out-of-range active tab).
        assert_eq!(rects.len(), 2);
    }

    #[test]
    fn height_scales_with_cell() {
        assert!(height_px(20.0) > height_px(10.0));
        assert!(height_px(20.0) >= 1.0);
    }

    #[test]
    fn truncate_is_char_safe() {
        assert_eq!(truncate("hello", 3), "hel");
        assert_eq!(truncate("hi", 9), "hi");
        assert_eq!(truncate("café", 4), "café");
    }
}
