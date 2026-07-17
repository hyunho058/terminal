//! `renderer` — the wgpu/Metal cell renderer.
//!
//! Consumes an immutable [`term_core::GridSnapshot`] and draws it. Deliberately
//! window-agnostic: it takes any handle that implements the raw-window-handle
//! traits (constraint C4 — no winit dependency), so `app` owns the window.
//!
//! **Scaffold scope**: each non-blank cell is drawn as a foreground-colored quad
//! (a "block font"), proving the full `pty → grid → GPU` path end to end. The
//! textured-glyph atlas ([`glyph`]) is the first render feature via `/specify`;
//! this renderer is where it plugs in.

pub mod glyph;

use bytemuck::{Pod, Zeroable};
use common::config::Config;
use term_core::GridSnapshot;
use thiserror::Error;
use wgpu::util::DeviceExt;

#[derive(Debug, Error)]
pub enum RendererError {
    #[error("failed to create surface: {0}")]
    Surface(#[from] wgpu::CreateSurfaceError),
    #[error("no compatible GPU adapter: {0}")]
    Adapter(#[from] wgpu::RequestAdapterError),
    #[error("failed to acquire device: {0}")]
    Device(#[from] wgpu::RequestDeviceError),
    #[error("surface has no default configuration for this adapter")]
    NoDefaultConfig,
    #[error("could not acquire next frame ({0})")]
    Frame(&'static str),
}

pub type Result<T> = std::result::Result<T, RendererError>;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    corner: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Instance {
    grid_pos: [f32; 2],
    color: [f32; 3],
    _pad: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniforms {
    viewport: [f32; 2],
    cell: [f32; 2],
}

/// Unit quad as two triangles (top-left origin).
const QUAD: [Vertex; 6] = [
    Vertex { corner: [0.0, 0.0] },
    Vertex { corner: [1.0, 0.0] },
    Vertex { corner: [0.0, 1.0] },
    Vertex { corner: [0.0, 1.0] },
    Vertex { corner: [1.0, 0.0] },
    Vertex { corner: [1.0, 1.0] },
];

/// GPU state for drawing the cell grid.
pub struct Renderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    quad_vbuf: wgpu::Buffer,
    uniform_buf: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    cell_size: (f32, f32),
    clear: wgpu::Color,
}

impl Renderer {
    /// Create a renderer targeting `window`, sized `width * height` physical px.
    ///
    /// `window` is any owned handle implementing the raw-window-handle traits
    /// (e.g. `Arc<winit::window::Window>`).
    pub fn new(
        window: impl Into<wgpu::SurfaceTarget<'static>>,
        width: u32,
        height: u32,
        scale_factor: f64,
        cfg: &Config,
    ) -> Result<Renderer> {
        let instance = wgpu::Instance::default();
        let surface = instance.create_surface(window)?;

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            ..Default::default()
        }))?;

        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("term-device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                ..Default::default()
            }))?;

        let info = adapter.get_info();
        tracing::info!(backend = ?info.backend, adapter = %info.name, "gpu adapter selected");

        let config = surface
            .get_default_config(&adapter, width.max(1), height.max(1))
            .ok_or(RendererError::NoDefaultConfig)?;
        surface.configure(&device, &config);
        tracing::info!(format = ?config.format, width = config.width, height = config.height, "surface configured");

        let cell_size = cell_size_px(cfg, scale_factor);
        let clear = to_wgpu_color(cfg.theme.background);

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("cells"),
            source: wgpu::ShaderSource::Wgsl(include_str!("cells.wgsl").into()),
        });

        let uniforms = Uniforms {
            viewport: [config.width as f32, config.height as f32],
            cell: [cell_size.0, cell_size.1],
        };
        let uniform_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("uniforms"),
            contents: bytemuck::bytes_of(&uniforms),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("uniform-bgl"),
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
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("uniform-bg"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buf.as_entire_binding(),
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("cells-layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let quad_vbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("quad"),
            contents: bytemuck::cast_slice(&QUAD),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("cells-pipeline"),
            layout: Some(&pipeline_layout),
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
                        array_stride: std::mem::size_of::<Instance>() as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &wgpu::vertex_attr_array![1 => Float32x2, 2 => Float32x3],
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
                    format: config.format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        Ok(Renderer {
            surface,
            device,
            queue,
            config,
            pipeline,
            quad_vbuf,
            uniform_buf,
            bind_group,
            cell_size,
            clear,
        })
    }

    /// Cell size in physical pixels `(width, height)`.
    pub fn cell_size(&self) -> (f32, f32) {
        self.cell_size
    }

    /// Grid dimensions `(cols, rows)` that fit the current surface.
    pub fn grid_size(&self) -> (u16, u16) {
        let cols = (self.config.width as f32 / self.cell_size.0)
            .floor()
            .max(1.0) as u16;
        let rows = (self.config.height as f32 / self.cell_size.1)
            .floor()
            .max(1.0) as u16;
        (cols, rows)
    }

    /// Reconfigure the surface after a window resize.
    pub fn resize(&mut self, width: u32, height: u32) {
        self.config.width = width.max(1);
        self.config.height = height.max(1);
        self.surface.configure(&self.device, &self.config);
        let uniforms = Uniforms {
            viewport: [self.config.width as f32, self.config.height as f32],
            cell: [self.cell_size.0, self.cell_size.1],
        };
        self.queue
            .write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(&uniforms));
    }

    /// Draw one frame from a grid snapshot.
    pub fn render(&mut self, snapshot: &GridSnapshot) -> Result<()> {
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t)
            | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                // Surface changed; reconfigure and skip this frame.
                self.surface.configure(&self.device, &self.config);
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Timeout => return Err(RendererError::Frame("timeout")),
            wgpu::CurrentSurfaceTexture::Occluded => return Ok(()),
            wgpu::CurrentSurfaceTexture::Validation => {
                return Err(RendererError::Frame("validation"))
            }
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let instances = build_instances(snapshot);
        let instance_buf = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("cell-instances"),
                contents: bytemuck::cast_slice(&instances),
                usage: wgpu::BufferUsages::VERTEX,
            });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("cells-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(self.clear),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            if !instances.is_empty() {
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &self.bind_group, &[]);
                pass.set_vertex_buffer(0, self.quad_vbuf.slice(..));
                pass.set_vertex_buffer(1, instance_buf.slice(..));
                pass.draw(0..QUAD.len() as u32, 0..instances.len() as u32);
            }
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        self.queue.present(frame);
        Ok(())
    }
}

/// One instance per non-blank cell, colored by its foreground (block-font scaffold).
fn build_instances(snapshot: &GridSnapshot) -> Vec<Instance> {
    let mut out = Vec::new();
    for row in 0..snapshot.rows {
        for col in 0..snapshot.cols {
            if let Some(cell) = snapshot.cell(col, row) {
                if !cell.is_blank() {
                    out.push(Instance {
                        grid_pos: [col as f32, row as f32],
                        color: cell.fg.to_linear_f32(),
                        _pad: 0.0,
                    });
                }
            }
        }
    }
    out
}

/// Cell metrics from the font size. Monospace approximation: 0.6 × advance,
/// 1.2 × line height, scaled to physical pixels.
fn cell_size_px(cfg: &Config, scale_factor: f64) -> (f32, f32) {
    let px = cfg.font.size_pt * scale_factor as f32;
    ((px * 0.6).round().max(1.0), (px * 1.2).round().max(1.0))
}

fn to_wgpu_color(packed: u32) -> wgpu::Color {
    let [r, g, b] = term_core::Color(packed).to_linear_f32();
    wgpu::Color {
        r: r as f64,
        g: g as f64,
        b: b as f64,
        a: 1.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::config::Config;

    #[test]
    fn cell_size_scales_with_dpi() {
        let cfg = Config::default();
        let (w1, _) = cell_size_px(&cfg, 1.0);
        let (w2, _) = cell_size_px(&cfg, 2.0);
        assert!(w2 > w1);
    }

    #[test]
    fn instances_skip_blank_cells() {
        let mut t =
            term_core::Terminal::new(4, 2, term_core::Color(0xFFFFFF), term_core::Color(0x000000));
        t.advance(b"hi");
        let instances = build_instances(&t.snapshot());
        assert_eq!(instances.len(), 2); // 'h' and 'i' only
    }
}
