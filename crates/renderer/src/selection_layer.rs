//! Selection highlight layer (selection-highlight D6–D9, SD2).
//!
//! A hand-rolled per-cell [`DrawLayer`] that tints the cells in
//! [`GridSnapshot::selection`] with a translucent color, reusing the cell-bg quad
//! technique (SD2) — registered through the existing `DrawLayer` seam, with no edit
//! to the central `Renderer::draw_to`. The selected-cell→instance mapping is a pure
//! function ([`selection_instances`]) so it is unit-testable without a GPU.
//!
//! The layer composites *after* the glyph pass (Load), so an opaque fill would hide
//! the text; it alpha-blends a translucent tint instead, like the preedit block.
//! [`SELECTION_Z`] places it above the glyph pass and below the cursor.

use bytemuck::{Pod, Zeroable};
use term_core::GridSnapshot;
use wgpu::util::DeviceExt;

use crate::{DrawLayer, FrameCtx};

/// z-order: above the glyph pass (0), below the cursor (`CURSOR_Z = 5`) — so the
/// cursor block and IME preedit draw *over* the tint, and all chrome (tab bar,
/// images, sidebar, overlays) painted later is never tinted.
pub const SELECTION_Z: i32 = 4;
const _: () = assert!(SELECTION_Z > 0 && SELECTION_Z < crate::cursor_layer::CURSOR_Z);

/// The translucent selection tint (baseline fixed color; a theme/config-driven
/// color is a deferred Known Gap). Alpha-blended so glyphs read through.
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
    color: [f32; 4],
}

/// One selected cell's pixel rect (`x, y, w, h`).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug, PartialEq)]
struct RectInstance {
    rect: [f32; 4],
}

const WGSL: &str = r#"
struct U { viewport: vec2<f32>, _pad: vec2<f32>, color: vec4<f32> };
@group(0) @binding(0) var<uniform> u: U;
struct VsIn { @location(0) corner: vec2<f32>, @location(1) rect: vec4<f32> };
struct VsOut { @builtin(position) pos: vec4<f32> };
@vertex fn vs_main(in: VsIn) -> VsOut {
    let px = in.rect.xy + in.corner * in.rect.zw;
    let clip = vec2<f32>(px.x / u.viewport.x * 2.0 - 1.0, 1.0 - px.y / u.viewport.y * 2.0);
    var o: VsOut; o.pos = vec4<f32>(clip, 0.0, 1.0); return o;
}
@fragment fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return u.color;
}
"#;

/// Pure geometry: one pixel rect per selected cell, in grid space. Reuses the
/// cell-bg mapping `px = (col,row) * cell + (left_inset, top_inset)` (SD2). Every
/// selected coordinate is emitted — **including double-width continuation cells**
/// (so a selected 한글 tints its full width), deliberately unlike text extraction.
/// Empty when there is no selection (D9). Split out so it is testable without a GPU.
fn selection_instances(
    snapshot: &GridSnapshot,
    cell: [f32; 2],
    left_inset: f32,
    top_inset: f32,
) -> Vec<RectInstance> {
    let Some(sel) = snapshot.selection else {
        return Vec::new();
    };
    let cols = snapshot.cols;
    let mut out = Vec::new();
    for row in 0..snapshot.rows {
        for col in 0..cols {
            if sel.contains(col, row, cols) {
                out.push(RectInstance {
                    rect: [
                        col as f32 * cell[0] + left_inset,
                        row as f32 * cell[1] + top_inset,
                        cell[0],
                        cell[1],
                    ],
                });
            }
        }
    }
    out
}

struct Gpu {
    format: wgpu::TextureFormat,
    quad: wgpu::Buffer,
    uniform: wgpu::Buffer,
    uniform_bind: wgpu::BindGroup,
    pipeline: wgpu::RenderPipeline,
}

/// The selection highlight draw layer. Register one via an `Overlay` (see the
/// `copy-clipboard` feature's `SelectionPresenter`), exactly as `ImageLayer` is.
#[derive(Default)]
pub struct SelectionLayer {
    gpu: Option<Gpu>,
}

impl SelectionLayer {
    pub fn new() -> Self {
        Self::default()
    }

    fn ensure_gpu(&mut self, ctx: &FrameCtx<'_>) {
        if self.gpu.as_ref().map(|g| g.format) == Some(ctx.format) {
            return;
        }
        let device = ctx.device;
        let quad = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("sel-quad"),
            contents: bytemuck::cast_slice(&QUAD),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("sel-uniform"),
            contents: bytemuck::bytes_of(&Uniforms {
                viewport: ctx.viewport,
                _pad: [0.0, 0.0],
                color: SELECTION_RGBA,
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let uniform_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sel-u-bgl"),
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
            label: Some("sel-u-bg"),
            layout: &uniform_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sel"),
            source: wgpu::ShaderSource::Wgsl(WGSL.into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sel-layout"),
            bind_group_layouts: &[Some(&uniform_bgl)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sel-pipeline"),
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
                    // Translucent tint over the glyphs (R3.1) — not an opaque fill.
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
            uniform_bind,
            pipeline,
        });
    }
}

impl DrawLayer for SelectionLayer {
    fn z_order(&self) -> i32 {
        SELECTION_Z
    }

    fn record(&mut self, ctx: &mut FrameCtx<'_>) {
        let instances = selection_instances(ctx.snapshot, ctx.cell, ctx.left_inset, ctx.top_inset);
        // No selection (or nothing visible) → record no pass at all (D9/R4).
        if instances.is_empty() {
            return;
        }
        self.ensure_gpu(ctx);
        let gpu = self.gpu.as_ref().expect("gpu built");
        // Refresh the uniform each frame (viewport may have resized).
        ctx.queue.write_buffer(
            &gpu.uniform,
            0,
            bytemuck::bytes_of(&Uniforms {
                viewport: ctx.viewport,
                _pad: [0.0, 0.0],
                color: SELECTION_RGBA,
            }),
        );
        let inst = ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("sel-inst"),
                contents: bytemuck::cast_slice(&instances),
                usage: wgpu::BufferUsages::VERTEX,
            });
        let mut pass = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("selection-pass"),
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
        pass.set_vertex_buffer(1, inst.slice(..));
        pass.draw(0..QUAD.len() as u32, 0..instances.len() as u32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use term_core::{Color, Selection, Terminal};

    fn snap_with_selection(cols: u16, rows: u16, feed: &[u8], sel: Selection) -> GridSnapshot {
        let mut t = Terminal::new(cols, rows, Color(0xE0E0E0), Color(0x101010));
        if !feed.is_empty() {
            t.advance(feed);
        }
        let anchor = t.point_at(sel.anchor.0, sel.anchor.1);
        let focus = t.point_at(sel.focus.0, sel.focus.1);
        t.set_selection(anchor, focus);
        t.snapshot()
    }

    #[test]
    fn z_order_above_glyph_and_below_cursor() {
        // R1.1 — the compile-time `const _` guard pins `0 < SELECTION_Z < CURSOR_Z`;
        // assert the layer reports it via the (runtime) method.
        let z = SelectionLayer::new().z_order();
        assert_eq!(z, SELECTION_Z);
        assert!(z > 0, "above the glyph pass");
        assert!(z < crate::cursor_layer::CURSOR_Z, "below the cursor");
    }

    #[test]
    fn each_selected_cell_maps_to_its_pixel_rect() {
        // R2.1 — single cell (3,1); cell (8,16), insets (40,24) → px (64,40).
        let snap = snap_with_selection(10, 3, b"", Selection::new((3, 1)));
        let inst = selection_instances(&snap, [8.0, 16.0], 40.0, 24.0);
        assert_eq!(inst.len(), 1);
        assert_eq!(inst[0].rect, [64.0, 40.0, 8.0, 16.0]);
    }

    #[test]
    fn double_width_cell_is_highlighted_across_both_cells() {
        // R2.2 — a 한 leader (col 0) + its continuation (col 1) both tint.
        let snap = snap_with_selection(
            10,
            1,
            "한".as_bytes(),
            Selection {
                anchor: (0, 0),
                focus: (1, 0),
            },
        );
        let inst = selection_instances(&snap, [8.0, 16.0], 0.0, 0.0);
        assert_eq!(inst.len(), 2, "leader + continuation both highlighted");
        // The two rects land on the leader (col 0) and its continuation (col 1),
        // and (1,0) really is a continuation cell — so this exercises D7, not just
        // an arbitrary 2-cell span.
        assert_eq!(inst[0].rect[0], 0.0);
        assert_eq!(inst[1].rect[0], 8.0);
        assert!(snap.cell(1, 0).unwrap().is_continuation());
    }

    #[test]
    fn instance_count_equals_number_of_selected_cells() {
        // R2.3 — span (8,0)→(1,1) on 10 cols = 2 + 2 = 4 cells.
        let snap = snap_with_selection(
            10,
            2,
            b"",
            Selection {
                anchor: (8, 0),
                focus: (1, 1),
            },
        );
        let inst = selection_instances(&snap, [8.0, 16.0], 0.0, 0.0);
        assert_eq!(inst.len(), 4);
    }

    #[test]
    fn no_selection_yields_no_instances() {
        // R4.1 — a snapshot with no selection records nothing.
        let mut t = Terminal::new(10, 2, Color(0xE0E0E0), Color(0x101010));
        t.advance(b"hi");
        let inst = selection_instances(&t.snapshot(), [8.0, 16.0], 0.0, 0.0);
        assert!(inst.is_empty());
    }

    #[test]
    fn off_grid_selection_yields_no_instances() {
        // R4.2 — a selection whose cells are all beyond the grid → empty membership.
        let snap = snap_with_selection(
            10,
            2,
            b"",
            Selection {
                anchor: (0, 5),
                focus: (9, 7),
            },
        );
        let inst = selection_instances(&snap, [8.0, 16.0], 0.0, 0.0);
        assert!(inst.is_empty());
    }
}
