//! Blinking cursor overlay — a [`DrawLayer`](crate::DrawLayer).
//!
//! Draws a translucent block at the grid cursor cell, above the glyphs. Because it
//! is a layer (not part of the cached grid instance path), it is re-recorded every
//! frame and blinks off the continuous-redraw loop — it is NOT frozen by the
//! idle-CPU snapshot cache, and needs no `term-core` change. The cursor position
//! travels in [`GridSnapshot::cursor`](term_core::GridSnapshot); the blink phase is
//! wall-clock only (a UI concern, kept out of the headless engine).

use std::time::{Duration, Instant};

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use crate::{DrawLayer, FrameCtx};

/// Draw order: above glyphs (z=0), below the tab bar (z=10).
pub const CURSOR_Z: i32 = 5;

/// Blink half-period in ms: shown for this long, then hidden, repeating.
const BLINK_MS: u128 = 530;
/// Block opacity so the character under the cursor stays legible.
const CURSOR_ALPHA: f32 = 0.5;
/// Fallback cursor color (linear) when the cursor cell has no readable fg.
const FALLBACK_RGB: [f32; 3] = [0.8, 0.8, 0.8];

/// Whether the cursor is shown at `elapsed` since start — a `BLINK_MS` square wave
/// (on, off, on, …). Pure, so the blink cadence is unit-testable without a GPU.
fn cursor_visible(elapsed: Duration) -> bool {
    (elapsed.as_millis() / BLINK_MS).is_multiple_of(2)
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

/// GPU resources built once on first draw (needs a device + target format).
struct Gpu {
    format: wgpu::TextureFormat,
    quad: wgpu::Buffer,
    uniform: wgpu::Buffer,
    pipeline: wgpu::RenderPipeline,
    bind: wgpu::BindGroup,
}

/// The blinking-cursor draw layer. Register one via `Renderer::add_layer`.
pub struct CursorLayer {
    start: Instant,
    gpu: Option<Gpu>,
}

impl CursorLayer {
    pub fn new() -> Self {
        CursorLayer {
            start: Instant::now(),
            gpu: None,
        }
    }

    fn ensure_gpu(&mut self, ctx: &FrameCtx<'_>) {
        if self.gpu.as_ref().map(|g| g.format) == Some(ctx.format) {
            return;
        }
        let device = ctx.device;
        let quad = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("cursor-quad"),
            contents: bytemuck::cast_slice(&QUAD),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("cursor-uniform"),
            contents: bytemuck::bytes_of(&Uniforms {
                viewport: ctx.viewport,
                _pad: [0.0; 2],
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("cursor-bgl"),
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
            label: Some("cursor-bg"),
            layout: &bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("cursor-shader"),
            source: wgpu::ShaderSource::Wgsl(WGSL.into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("cursor-layout"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("cursor-pipeline"),
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

    /// The cursor block rect (px) + color for this frame, or `None` if it should
    /// not draw — blinked off, scrolled back into history, or out of bounds.
    fn cursor_rect(&self, ctx: &FrameCtx<'_>) -> Option<RectInstance> {
        let snap = ctx.snapshot;
        let (col, row) = (snap.cursor.col, snap.cursor.row);
        let rgb = snap
            .cell(col, row)
            .map(|c| c.fg.to_linear_f32())
            .unwrap_or(FALLBACK_RGB);
        block_rect(
            cursor_visible(self.start.elapsed()),
            snap.at_bottom,
            col,
            row,
            snap.cols,
            snap.rows,
            ctx.cell,
            ctx.top_inset,
            ctx.left_inset,
            rgb,
        )
    }
}

/// Pure geometry for the cursor block: the px rect + color, or `None` when it must
/// not draw (blinked off, scrolled back, or the cursor is out of bounds — e.g.
/// parked past the last column pending wrap). Split out so positioning is
/// unit-testable without a GPU `FrameCtx`.
#[allow(clippy::too_many_arguments)]
fn block_rect(
    visible: bool,
    at_bottom: bool,
    col: u16,
    row: u16,
    cols: u16,
    rows: u16,
    cell: [f32; 2],
    top_inset: f32,
    left_inset: f32,
    rgb: [f32; 3],
) -> Option<RectInstance> {
    if !visible || !at_bottom || col >= cols || row >= rows {
        return None;
    }
    // Shift right by left_inset (the sidebar band) and down by top_inset, matching
    // the bg/glyph shaders — else the cursor lands left of its glyph.
    let x = col as f32 * cell[0] + left_inset;
    let y = row as f32 * cell[1] + top_inset;
    Some(RectInstance {
        rect: [x, y, cell[0], cell[1]],
        color: [rgb[0], rgb[1], rgb[2], CURSOR_ALPHA],
    })
}

impl Default for CursorLayer {
    fn default() -> Self {
        Self::new()
    }
}

impl DrawLayer for CursorLayer {
    fn z_order(&self) -> i32 {
        CURSOR_Z
    }

    fn record(&mut self, ctx: &mut FrameCtx<'_>) {
        let Some(inst) = self.cursor_rect(ctx) else {
            return; // blinked off / hidden — draw nothing this frame
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
                label: Some("cursor-instance"),
                contents: bytemuck::bytes_of(&inst),
                usage: wgpu::BufferUsages::VERTEX,
            });
        let mut pass = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("cursor-pass"),
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

    #[test]
    fn blink_toggles_each_half_period() {
        // On for [0,530), off for [530,1060), on again, …
        assert!(cursor_visible(Duration::from_millis(0)));
        assert!(cursor_visible(Duration::from_millis(300)));
        assert!(cursor_visible(Duration::from_millis(529)));
        assert!(!cursor_visible(Duration::from_millis(530)));
        assert!(!cursor_visible(Duration::from_millis(900)));
        assert!(cursor_visible(Duration::from_millis(1060)));
    }

    #[test]
    fn block_rect_positions_at_the_cursor_cell() {
        // Visible + live + in-bounds → a quad at
        // (col·cw + left_inset, row·ch + top_inset).
        let r = block_rect(
            true,
            true,
            2,
            3,
            80,
            24,
            [10.0, 20.0],
            30.0, // top_inset
            40.0, // left_inset (the sidebar band)
            [1.0, 0.5, 0.0],
        )
        .expect("should draw");
        // x = 2*10 + 40 (left_inset), y = 3*20 + 30 (top_inset), w=cw, h=ch.
        assert_eq!(r.rect, [60.0, 90.0, 10.0, 20.0]);
        assert_eq!(r.color, [1.0, 0.5, 0.0, CURSOR_ALPHA]);
    }

    #[test]
    fn block_rect_shifts_with_left_inset() {
        // Regression: the sidebar shifts the grid right by left_inset, so the
        // cursor at column `col` must land at `col*cw + left_inset` — not at
        // `col*cw` (which put it in the middle of the prompt). With left_inset=0
        // it reduces to the bare column position.
        let cell = [10.0, 20.0];
        let rgb = [0.8, 0.8, 0.8];
        let at = |left_inset| {
            block_rect(true, true, 5, 0, 80, 24, cell, 0.0, left_inset, rgb)
                .expect("draws")
                .rect[0]
        };
        assert_eq!(at(0.0), 50.0, "no inset → bare column x");
        assert_eq!(at(140.0), 190.0, "x shifts right by exactly left_inset");
    }

    #[test]
    fn block_rect_none_when_hidden_or_off_screen() {
        let cell = [10.0, 20.0];
        let rgb = [0.8, 0.8, 0.8];
        // blinked off
        assert!(block_rect(false, true, 0, 0, 80, 24, cell, 0.0, 0.0, rgb).is_none());
        // scrolled back into history
        assert!(block_rect(true, false, 0, 0, 80, 24, cell, 0.0, 0.0, rgb).is_none());
        // cursor parked past the last column (col == cols) pending wrap
        assert!(block_rect(true, true, 80, 0, 80, 24, cell, 0.0, 0.0, rgb).is_none());
        // row out of bounds
        assert!(block_rect(true, true, 0, 24, 80, 24, cell, 0.0, 0.0, rgb).is_none());
    }
}
