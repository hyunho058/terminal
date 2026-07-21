//! Text-selection highlight — a [`DrawLayer`](crate::DrawLayer).
//!
//! Draws a translucent quad behind the selected cells, one per selected row-span,
//! above the glyphs but below the cursor block. Like [`cursor_layer`] it is a layer
//! (not part of the cached grid instance path), so it re-records every frame from a
//! shared [`SelectionHandle`] and is unaffected by the idle-CPU snapshot cache — the
//! app pushes a new [`Selection`] as the drag moves and the highlight follows.
//! Selection is display-only UI state, so it is NOT carried in `GridSnapshot`; the
//! geometry (`row_span`) lives in `term-core` and is shared with the copy path.
//!
//! [`cursor_layer`]: crate::cursor_layer

use std::cell::RefCell;
use std::rc::Rc;

use bytemuck::{Pod, Zeroable};
use term_core::Selection;
use wgpu::util::DeviceExt;

use crate::{DrawLayer, FrameCtx};

/// Draw order: above glyphs (z=0), below the cursor block (z=5) so the cursor stays
/// visible over a selection.
pub const SELECTION_Z: i32 = 4;

/// Highlight color (linear rgba). Translucent so the selected text stays legible.
const SELECTION_RGBA: [f32; 4] = [0.20, 0.40, 0.85, 0.35];

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

/// A shared handle the app writes the current selection into; the layer reads it
/// each frame. Single-threaded (UI thread only), mirroring `PreeditHandle`.
#[derive(Clone, Default)]
pub struct SelectionHandle(Rc<RefCell<Option<Selection>>>);

impl SelectionHandle {
    /// Set (or clear, with `None`) the selection drawn on the next frame.
    pub fn set(&self, sel: Option<Selection>) {
        *self.0.borrow_mut() = sel;
    }

    fn get(&self) -> Option<Selection> {
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

/// The selection-highlight draw layer. Register one via `Renderer::add_layer`.
pub struct SelectionLayer {
    handle: SelectionHandle,
    gpu: Option<Gpu>,
}

impl SelectionLayer {
    /// Build the layer and the handle the app pushes selection updates through.
    pub fn new() -> (SelectionLayer, SelectionHandle) {
        let handle = SelectionHandle::default();
        (
            SelectionLayer {
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
            label: Some("selection-quad"),
            contents: bytemuck::cast_slice(&QUAD),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("selection-uniform"),
            contents: bytemuck::bytes_of(&Uniforms {
                viewport: ctx.viewport,
                _pad: [0.0; 2],
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("selection-bgl"),
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
            label: Some("selection-bg"),
            layout: &bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("selection-shader"),
            source: wgpu::ShaderSource::Wgsl(WGSL.into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("selection-layout"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("selection-pipeline"),
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

/// Pure geometry: the highlight quads (px) for `sel` over a `cols`×`rows` grid, one
/// per selected row-span. Split out so positioning is unit-testable without a GPU.
fn selection_rects(
    sel: &Selection,
    cols: u16,
    rows: u16,
    cell: [f32; 2],
    top_inset: f32,
    left_inset: f32,
) -> Vec<RectInstance> {
    if cols == 0 || rows == 0 {
        return Vec::new();
    }
    let (start, end) = sel.ordered();
    let first = start.1;
    let last = end.1.min(rows - 1);
    if first > last {
        return Vec::new();
    }
    let mut rects = Vec::new();
    for row in first..=last {
        if let Some((c0, c1)) = sel.row_span(row, cols) {
            let x = c0 as f32 * cell[0] + left_inset;
            let y = row as f32 * cell[1] + top_inset;
            let w = (c1 - c0 + 1) as f32 * cell[0];
            rects.push(RectInstance {
                rect: [x, y, w, cell[1]],
                color: SELECTION_RGBA,
            });
        }
    }
    rects
}

impl DrawLayer for SelectionLayer {
    fn z_order(&self) -> i32 {
        SELECTION_Z
    }

    fn record(&mut self, ctx: &mut FrameCtx<'_>) {
        // No selection, or a live-view-only concern: draw nothing this frame.
        let Some(sel) = self.handle.get() else {
            return;
        };
        let rects = selection_rects(
            &sel,
            ctx.snapshot.cols,
            ctx.snapshot.rows,
            ctx.cell,
            ctx.top_inset,
            ctx.left_inset,
        );
        if rects.is_empty() {
            return;
        }
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
                label: Some("selection-instance"),
                contents: bytemuck::cast_slice(&rects),
                usage: wgpu::BufferUsages::VERTEX,
            });
        let mut pass = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("selection-pass"),
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
        pass.draw(0..QUAD.len() as u32, 0..rects.len() as u32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_row_selection_is_one_quad_spanning_the_columns() {
        // cols 2..=5 on row 1, cell 10×20, insets (left 40, top 30).
        let sel = Selection {
            anchor: (2, 1),
            head: (5, 1),
        };
        let rects = selection_rects(&sel, 80, 24, [10.0, 20.0], 30.0, 40.0);
        assert_eq!(rects.len(), 1);
        // x = 2*10 + 40 = 60, y = 1*20 + 30 = 50, w = (5-2+1)*10 = 40, h = 20.
        assert_eq!(rects[0].rect, [60.0, 50.0, 40.0, 20.0]);
        assert_eq!(rects[0].color, SELECTION_RGBA);
    }

    #[test]
    fn multi_row_selection_is_one_quad_per_row() {
        // (2,0)→(3,2) over an 8-col grid → 3 row-spans.
        let sel = Selection {
            anchor: (2, 0),
            head: (3, 2),
        };
        let rects = selection_rects(&sel, 8, 24, [10.0, 20.0], 0.0, 0.0);
        assert_eq!(rects.len(), 3);
        // Row 0: cols [2,7] → x=20, w=(7-2+1)*10=60.
        assert_eq!(rects[0].rect, [20.0, 0.0, 60.0, 20.0]);
        // Row 1: full width [0,7] → x=0, w=80.
        assert_eq!(rects[1].rect, [0.0, 20.0, 80.0, 20.0]);
        // Row 2: cols [0,3] → x=0, w=40.
        assert_eq!(rects[2].rect, [0.0, 40.0, 40.0, 20.0]);
    }

    #[test]
    fn selection_below_viewport_is_clamped_to_visible_rows() {
        // A head dragged to row 99 on a 3-row viewport only emits 3 quads.
        let sel = Selection {
            anchor: (0, 0),
            head: (0, 99),
        };
        let rects = selection_rects(&sel, 10, 3, [10.0, 20.0], 0.0, 0.0);
        assert_eq!(rects.len(), 3);
    }

    #[test]
    fn empty_grid_draws_nothing() {
        let sel = Selection::at(0, 0);
        assert!(selection_rects(&sel, 0, 0, [10.0, 20.0], 0.0, 0.0).is_empty());
    }
}
