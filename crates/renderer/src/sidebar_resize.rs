//! Sidebar resize-handle rule — a [`DrawLayer`](crate::DrawLayer).
//!
//! Draws a thin vertical rule at the sidebar's right edge (`x = left_inset`), derived
//! from the SINGLE geometry source [`sidebar_layout::resize_handle_rect`] — so it draws
//! nothing when that returns `None` (collapsed), keeping draw and hit-test single-sourced
//! (sidebar-resize SD3/D8/D10). It brightens while a drag is active; the app pushes that
//! state through [`SidebarResizeHandle`]. Like `cursor_layer`/`selection_layer` it is
//! re-recorded every frame from a shared handle, so it follows the drag off the
//! continuous-redraw loop and is unaffected by the idle-CPU snapshot cache.

use std::cell::RefCell;
use std::rc::Rc;

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use crate::sidebar_layout::{resize_handle_rect, SidebarLayoutInput};
use crate::{DrawLayer, FrameCtx};

/// Draw order: above the glyphs (z=0), below the cursor block (z=5). The rule sits at the
/// sidebar's right edge and never overlaps the cards (z=10) or chrome (z=30).
pub const SIDEBAR_RESIZE_Z: i32 = 3;

/// Visible rule width, logical px (× `scale` at use) — thinner than the grab band.
const RULE_W_PX: f32 = 1.5;
/// Rule color when idle (linear rgba), translucent so it reads as a hairline.
const RULE_IDLE: [f32; 4] = [0.50, 0.50, 0.50, 0.30];
/// Rule color while a resize drag is active — brighter/bluer for feedback.
const RULE_ACTIVE: [f32; 4] = [0.60, 0.72, 1.0, 0.75];

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

/// A solid-color quad in px (top-left origin).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct RectInstance {
    rect: [f32; 4], // x, y, w, h
    color: [f32; 4],
}

const WGSL: &str = r#"
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

/// A shared handle the app sets to `true` while a resize drag is in flight; the layer
/// reads it each frame to brighten the rule. Single-threaded (UI thread only), mirroring
/// `SelectionHandle`.
#[derive(Clone, Default)]
pub struct SidebarResizeHandle(Rc<RefCell<bool>>);

impl SidebarResizeHandle {
    /// Mark the resize drag active (`true`) or idle (`false`) for the next frame.
    pub fn set_active(&self, active: bool) {
        *self.0.borrow_mut() = active;
    }

    fn active(&self) -> bool {
        *self.0.borrow()
    }
}

/// GPU resources built once on first draw (needs a device + target format).
struct Gpu {
    format: wgpu::TextureFormat,
    quad: wgpu::Buffer,
    uniform: wgpu::Buffer,
    pipeline: wgpu::RenderPipeline,
    bind: wgpu::BindGroup,
}

/// The sidebar resize-rule draw layer. Register one via `Renderer::add_layer`.
pub struct SidebarResizeLayer {
    handle: SidebarResizeHandle,
    gpu: Option<Gpu>,
}

impl SidebarResizeLayer {
    /// Build the layer and the handle the app pushes drag-active state through.
    pub fn new() -> (SidebarResizeLayer, SidebarResizeHandle) {
        let handle = SidebarResizeHandle::default();
        (
            SidebarResizeLayer {
                handle: handle.clone(),
                gpu: None,
            },
            handle,
        )
    }

    fn ensure_gpu(&mut self, ctx: &FrameCtx<'_>) {
        if self.gpu.as_ref().map(|g| g.format) == Some(ctx.format) {
            return;
        }
        let device = ctx.device;
        let quad = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("sidebar-resize-quad"),
            contents: bytemuck::cast_slice(&QUAD),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("sidebar-resize-uniform"),
            contents: bytemuck::bytes_of(&Uniforms {
                viewport: ctx.viewport,
                _pad: [0.0; 2],
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sidebar-resize-bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sidebar-resize-bg"),
            layout: &bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sidebar-resize-shader"),
            source: wgpu::ShaderSource::Wgsl(WGSL.into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sidebar-resize-layout"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sidebar-resize-pipeline"),
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
                        attributes: &wgpu::vertex_attr_array![1 => Float32x4, 2 => Float32x4],
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
            bind,
        });
    }
}

/// Pure geometry: the rule quad (px) for the current sidebar, or `None` when there is no
/// handle (collapsed). Split out so positioning is unit-testable without a GPU.
fn rule_rect(inp: &SidebarLayoutInput, active: bool) -> Option<RectInstance> {
    let band = resize_handle_rect(inp)?;
    Some(RectInstance {
        rect: [band.x, band.y, RULE_W_PX * inp.scale as f32, band.h],
        color: if active { RULE_ACTIVE } else { RULE_IDLE },
    })
}

impl DrawLayer for SidebarResizeLayer {
    fn z_order(&self) -> i32 {
        SIDEBAR_RESIZE_Z
    }

    fn record(&mut self, ctx: &mut FrameCtx<'_>) {
        // `resize_handle_rect` reads only sidebar_w / viewport / scale, so row_h/row_count
        // are irrelevant here — pass 0.
        let inp = SidebarLayoutInput::from_frame(ctx, 0.0, 0);
        let Some(inst) = rule_rect(&inp, self.handle.active()) else {
            return; // collapsed — nothing to draw
        };
        self.ensure_gpu(ctx);
        let gpu = self.gpu.as_ref().expect("gpu built in ensure_gpu");
        ctx.queue.write_buffer(
            &gpu.uniform,
            0,
            bytemuck::bytes_of(&Uniforms {
                viewport: ctx.viewport,
                _pad: [0.0; 2],
            }),
        );
        let inst_buf = ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("sidebar-resize-instance"),
                contents: bytemuck::bytes_of(&inst),
                usage: wgpu::BufferUsages::VERTEX,
            });
        let mut pass = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("sidebar-resize-pass"),
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
        pass.set_pipeline(&gpu.pipeline);
        pass.set_bind_group(0, &gpu.bind, &[]);
        pass.set_vertex_buffer(0, gpu.quad.slice(..));
        pass.set_vertex_buffer(1, inst_buf.slice(..));
        pass.draw(0..QUAD.len() as u32, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sidebar_theme::SidebarMetrics;

    fn input(sidebar_w: f32, scale: f64) -> SidebarLayoutInput {
        SidebarLayoutInput {
            sidebar_w,
            viewport: [sidebar_w + 100.0, 900.0],
            scale,
            row_h: 60.0,
            row_count: 3,
            metrics: SidebarMetrics::DEFAULT.scaled(scale),
        }
    }

    #[test]
    fn rule_sits_at_the_edge_and_brightens_when_active() {
        let r = rule_rect(&input(300.0, 1.0), false).expect("expanded → a rule");
        assert_eq!(r.rect[0], 300.0, "rule at the right edge");
        assert_eq!(r.rect[2], RULE_W_PX, "thin rule at scale 1.0");
        assert_eq!(r.rect[3], 900.0, "full height");
        assert_eq!(r.color, RULE_IDLE);
        // Active drag → brighter color, same geometry.
        assert_eq!(
            rule_rect(&input(300.0, 1.0), true).unwrap().color,
            RULE_ACTIVE
        );
    }

    #[test]
    fn no_rule_when_collapsed() {
        assert!(rule_rect(&input(0.0, 1.0), true).is_none());
    }
}
