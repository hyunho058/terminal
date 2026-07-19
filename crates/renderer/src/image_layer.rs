//! Inline-image renderer layer (image-paste D3/R2, SD3).
//!
//! Draws each [`term_core::Placement`] carried on the snapshot as a full-color
//! quad at its cell rectangle, registered via the `DrawLayer` seam — no edit to
//! the central renderer. Each placement's RGBA is uploaded to a texture once and
//! cached by placement id (re-used across frames, dropped when the placement
//! scrolls off).

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use crate::{DrawLayer, FrameCtx};

/// z-order: above the tab bar (10), below overlays (100).
pub const IMAGE_Z: i32 = 20;

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
    rect: [f32; 4], // x, y, w, h px
}

const WGSL: &str = r#"
struct U { viewport: vec2<f32>, _pad: vec2<f32> };
@group(0) @binding(0) var<uniform> u: U;
@group(1) @binding(0) var img: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;
struct VsIn { @location(0) corner: vec2<f32>, @location(1) rect: vec4<f32> };
struct VsOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex fn vs_main(in: VsIn) -> VsOut {
    let px = in.rect.xy + in.corner * in.rect.zw;
    let clip = vec2<f32>(px.x / u.viewport.x * 2.0 - 1.0, 1.0 - px.y / u.viewport.y * 2.0);
    var o: VsOut; o.pos = vec4<f32>(clip, 0.0, 1.0); o.uv = in.corner; return o;
}
@fragment fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return textureSample(img, samp, in.uv);
}
"#;

struct Gpu {
    format: wgpu::TextureFormat,
    quad: wgpu::Buffer,
    uniform: wgpu::Buffer,
    pipeline: wgpu::RenderPipeline,
    uniform_bind: wgpu::BindGroup,
    tex_bgl: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
}

/// A cached uploaded image (texture + its group-1 bind group).
struct CachedImage {
    _texture: wgpu::Texture,
    bind: wgpu::BindGroup,
    instances: wgpu::Buffer,
}

/// The image draw layer. Register one via `Renderer::add_layer`.
#[derive(Default)]
pub struct ImageLayer {
    gpu: Option<Gpu>,
    cache: HashMap<u32, CachedImage>,
}

impl ImageLayer {
    pub fn new() -> Self {
        Self::default()
    }

    fn ensure_gpu(&mut self, ctx: &FrameCtx<'_>) {
        if self.gpu.as_ref().map(|g| g.format) == Some(ctx.format) {
            return;
        }
        // Format changed → old textures/bind groups are stale.
        self.cache.clear();
        let device = ctx.device;
        let quad = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("img-quad"),
            contents: bytemuck::cast_slice(&QUAD),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("img-uniform"),
            contents: bytemuck::bytes_of(&Uniforms {
                viewport: ctx.viewport,
                _pad: [0.0, 0.0],
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let uniform_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("img-u-bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let uniform_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("img-u-bg"),
            layout: &uniform_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });
        let tex_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("img-t-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("img-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("img"),
            source: wgpu::ShaderSource::Wgsl(WGSL.into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("img-layout"),
            bind_group_layouts: &[Some(&uniform_bgl), Some(&tex_bgl)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("img-pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[
                    Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<Vertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x2],
                    }),
                    Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<RectInstance>() as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &wgpu::vertex_attr_array![1 => Float32x4],
                    }),
                ],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: ctx.format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        self.gpu = Some(Gpu {
            format: ctx.format,
            quad,
            uniform,
            pipeline,
            uniform_bind,
            tex_bgl,
            sampler,
        });
    }
}

impl DrawLayer for ImageLayer {
    fn z_order(&self) -> i32 {
        IMAGE_Z
    }

    fn record(&mut self, ctx: &mut FrameCtx<'_>) {
        if ctx.snapshot.images.is_empty() && self.cache.is_empty() {
            return;
        }
        self.ensure_gpu(ctx);
        let (cw, ch) = (ctx.cell[0], ctx.cell[1]);
        let gpu = self.gpu.as_ref().expect("gpu built");
        ctx.queue.write_buffer(
            &gpu.uniform,
            0,
            bytemuck::bytes_of(&Uniforms {
                viewport: ctx.viewport,
                _pad: [0.0, 0.0],
            }),
        );

        // Upload any not-yet-cached placement; refresh its rect each frame (it may
        // have scrolled). Cell rect is offset right by the left tab-sidebar inset
        // and down by any top inset, matching the grid glyphs.
        let mut live = std::collections::HashSet::new();
        for p in &ctx.snapshot.images {
            live.insert(p.id);
            let rect = tile_rect(
                p.col,
                p.row,
                p.cols,
                p.rows,
                [cw, ch],
                ctx.top_inset,
                ctx.left_inset,
            );
            let instances = ctx
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("img-inst"),
                    contents: bytemuck::bytes_of(&RectInstance { rect }),
                    usage: wgpu::BufferUsages::VERTEX,
                });
            if let Some(cached) = self.cache.get_mut(&p.id) {
                cached.instances = instances;
                continue;
            }
            // Saturating usize math: never overflow on large dims.
            let need = (p.px_w as usize)
                .saturating_mul(p.px_h as usize)
                .saturating_mul(4);
            if p.px_w == 0 || p.px_h == 0 || p.rgba.len() < need {
                continue;
            }
            let texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("img-tex"),
                size: wgpu::Extent3d {
                    width: p.px_w,
                    height: p.px_h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            ctx.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &p.rgba[..need],
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(p.px_w * 4),
                    rows_per_image: Some(p.px_h),
                },
                wgpu::Extent3d {
                    width: p.px_w,
                    height: p.px_h,
                    depth_or_array_layers: 1,
                },
            );
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let bind = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("img-t-bg"),
                layout: &gpu.tex_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&gpu.sampler),
                    },
                ],
            });
            self.cache.insert(
                p.id,
                CachedImage {
                    _texture: texture,
                    bind,
                    instances,
                },
            );
        }
        // Drop textures for placements that scrolled off.
        self.cache.retain(|id, _| live.contains(id));
        if self.cache.is_empty() {
            return;
        }

        let mut pass = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("image-pass"),
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
        pass.set_pipeline(&gpu.pipeline);
        pass.set_bind_group(0, &gpu.uniform_bind, &[]);
        pass.set_vertex_buffer(0, gpu.quad.slice(..));
        for p in &ctx.snapshot.images {
            if let Some(cached) = self.cache.get(&p.id) {
                pass.set_bind_group(1, &cached.bind, &[]);
                pass.set_vertex_buffer(1, cached.instances.slice(..));
                pass.draw(0..QUAD.len() as u32, 0..1);
            }
        }
    }
}

/// Pure geometry for an image placement: the px rect of a tile anchored at cell
/// `(col, row)` spanning `cols × rows` cells. Shifted right by `left_inset` (the
/// left tab sidebar) and down by `top_inset`, matching the bg/glyph shaders — this
/// layer's own shader applies no inset, so it is baked in here. Split out so
/// positioning is unit-testable without a GPU `FrameCtx`.
fn tile_rect(
    col: u16,
    row: u16,
    cols: u16,
    rows: u16,
    cell: [f32; 2],
    top_inset: f32,
    left_inset: f32,
) -> [f32; 4] {
    [
        col as f32 * cell[0] + left_inset,
        row as f32 * cell[1] + top_inset,
        cols as f32 * cell[0],
        rows as f32 * cell[1],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn z_order_between_tab_bar_and_overlay() {
        let l = ImageLayer::new();
        assert!(l.z_order() > super::super::tab_bar::TAB_BAR_Z);
        assert!(l.z_order() < super::super::overlay_layer::OVERLAY_Z);
    }

    #[test]
    fn tile_rect_shifts_with_left_inset() {
        // An image at cell (5, 2) spanning 3x2 cells of 10x20 px. The sidebar
        // shifts it right by left_inset exactly as it shifts the grid glyphs;
        // size is unaffected by either inset.
        let r = tile_rect(5, 2, 3, 2, [10.0, 20.0], 30.0, 140.0);
        assert_eq!(r, [190.0, 70.0, 30.0, 40.0]); // x=5*10+140, y=2*20+30, w=3*10, h=2*20
                                                  // With no insets it reduces to the bare cell position (regression guard:
                                                  // pre-fix this layer omitted left_inset and drew images left of the grid).
        assert_eq!(
            tile_rect(5, 2, 3, 2, [10.0, 20.0], 0.0, 0.0),
            [50.0, 40.0, 30.0, 40.0]
        );
    }
}
