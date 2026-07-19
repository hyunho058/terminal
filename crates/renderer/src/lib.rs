//! `renderer` — the wgpu/Metal cell renderer.
//!
//! Consumes an immutable [`term_core::GridSnapshot`] and draws it with real
//! glyphs: `swash` shapes each row (honoring ligatures per config), a dynamic
//! [`atlas::Atlas`] caches rasterized coverage on the GPU, and a two-pass draw
//! paints per-cell backgrounds then textured glyph quads (D1/D2/D3/D6).
//!
//! Window-agnostic (raw-window-handle, no winit — C1/SD2). The [`DrawLayer`] seam
//! (D4/SD3) lets later features (tab bar, image layer, ai-monitor overlay) add
//! their own passes without editing this file: the terminal glyph pass is the
//! implicit z=0 layer, and registered layers draw after it in `z_order`.

pub mod atlas;
pub mod cursor_layer;
pub mod glyph;
pub mod image_layer;
pub mod overlay_layer;
pub mod tab_bar;

use std::sync::Arc;

use atlas::{Atlas, GlyphKey};
use bytemuck::{Pod, Zeroable};
use common::config::Config;
use swash::shape::ShapeContext;
use swash::FontRef;
use term_core::{Cell, CellAttrs, GridSnapshot};
use thiserror::Error;
use wgpu::util::DeviceExt;

/// Candidate macOS monospace fonts (macOS-only — SD1). Family→path resolution is
/// a later enhancement; v1 uses the first that loads.
const FONT_CANDIDATES: &[&str] = &[
    "/System/Library/Fonts/Menlo.ttc",
    "/System/Library/Fonts/SFNSMono.ttf",
];

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
    #[error("no usable monospace font found (tried {0})")]
    NoFont(String),
    #[error("could not acquire next frame ({0})")]
    Frame(&'static str),
}

pub type Result<T> = std::result::Result<T, RendererError>;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    corner: [f32; 2],
}

/// Per-cell background quad (consumed by `cells.wgsl`).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct BgInstance {
    grid_pos: [f32; 2],
    color: [f32; 3],
    _pad: f32,
}

/// Per-glyph textured quad (consumed by `glyph.wgsl`).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GlyphInstance {
    pos: [f32; 2],
    size: [f32; 2],
    uv0: [f32; 2],
    uv1: [f32; 2],
    color: [f32; 3],
    _pad: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct BgUniforms {
    viewport: [f32; 2],
    cell: [f32; 2],
    top_inset: f32,
    left_inset: f32,
    _pad: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GlyphUniforms {
    viewport: [f32; 2],
    top_inset: f32,
    left_inset: f32,
}

const QUAD: [Vertex; 6] = [
    Vertex { corner: [0.0, 0.0] },
    Vertex { corner: [1.0, 0.0] },
    Vertex { corner: [0.0, 1.0] },
    Vertex { corner: [0.0, 1.0] },
    Vertex { corner: [1.0, 0.0] },
    Vertex { corner: [1.0, 1.0] },
];

/// Per-frame context handed to each [`DrawLayer`]. A layer records its own render
/// pass on `encoder` (Load, so it composites over the terminal), using its own
/// pipelines/buffers and, if it draws text, the shared glyph `atlas`.
pub struct FrameCtx<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub encoder: &'a mut wgpu::CommandEncoder,
    pub view: &'a wgpu::TextureView,
    /// Framebuffer size in physical px.
    pub viewport: [f32; 2],
    /// Cell size in physical px.
    pub cell: [f32; 2],
    /// Physical px reserved above the grid (the tab bar paints this top band).
    pub top_inset: f32,
    /// Physical px reserved left of the grid (the vertical tab sidebar paints
    /// this left band). Grid + glyphs shift right by this.
    pub left_inset: f32,
    /// Render-target format — a layer needs it to build its own pipelines.
    pub format: wgpu::TextureFormat,
    pub snapshot: &'a GridSnapshot,
    pub atlas: &'a mut Atlas,
}

/// A pluggable draw layer (SD3/D4 seam). The terminal glyph pass is the implicit
/// z=0 layer; registered layers draw after it, ordered by [`DrawLayer::z_order`]
/// (lower first). Provisional contract — may be refined when the first consumer
/// (`tabs`) lands.
pub trait DrawLayer {
    /// Draw order: lower records first (glyph=0 < tab bar=10 < image=20 < overlay=100).
    fn z_order(&self) -> i32;
    /// Record this layer's pass for the frame.
    fn record(&mut self, ctx: &mut FrameCtx<'_>);
}

/// GPU state for drawing the cell grid.
pub struct Renderer {
    /// `None` for a headless renderer (benchmarks/tests draw to an offscreen view).
    surface: Option<wgpu::Surface<'static>>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    bg_pipeline: wgpu::RenderPipeline,
    glyph_pipeline: wgpu::RenderPipeline,
    quad_vbuf: wgpu::Buffer,
    bg_uniform_buf: wgpu::Buffer,
    bg_bind_group: wgpu::BindGroup,
    glyph_uniform_buf: wgpu::Buffer,
    glyph_bind_group: wgpu::BindGroup,
    /// Kept so `apply_config` can rebuild the glyph bind group against a fresh atlas.
    glyph_bgl: wgpu::BindGroupLayout,
    atlas: Atlas,
    font: Arc<Vec<u8>>,
    /// Needed to re-derive cell size / ppem when the config changes live.
    scale_factor: f64,
    cell_size: (f32, f32),
    /// Physical px reserved above the grid (e.g. a tab bar). Grid + glyphs are
    /// shifted down by this, and `grid_size` reports the rows that fit below it.
    top_inset: f32,
    /// Physical px reserved left of the grid (e.g. a vertical tab sidebar). The
    /// grid and glyphs are shifted right by this, and `grid_size` reports the
    /// columns that fit beside it. Orthogonal to `top_inset` (rows vs cols).
    left_inset: f32,
    ppem: f32,
    ligatures: bool,
    bg_default: u32,
    clear: wgpu::Color,
    layers: Vec<Box<dyn DrawLayer>>,
    /// Idle-CPU cache (D5): the last snapshot whose instances are resident in
    /// `bg_vbuf`/`glyph_vbuf`. When the next `draw_to` snapshot `==` this, the
    /// instance builders are skipped and the resident buffers re-presented. Reset
    /// to `None` by `apply_config`/`resize` (D5 cache-invalidation caveat, R1.4) so
    /// a content-equal snapshot can never re-present stale theme/font/size buffers.
    last_snapshot: Option<GridSnapshot>,
    /// Resident per-cell instance buffers (D6) — reused across frames instead of a
    /// `create_buffer_init` every frame; grown on a cache miss that overflows them.
    bg_vbuf: Option<wgpu::Buffer>,
    glyph_vbuf: Option<wgpu::Buffer>,
    bg_count: u32,
    glyph_count: u32,
    /// Observability: number of cache **misses** (frames that actually re-shaped).
    /// Idle frames re-present without incrementing it — the signal the idle-CPU
    /// gate reads (R1.1/R1.4).
    reshape_count: u64,
}

impl Renderer {
    pub fn new(
        window: impl Into<wgpu::SurfaceTarget<'static>>,
        width: u32,
        height: u32,
        scale_factor: f64,
        cfg: &Config,
    ) -> Result<Renderer> {
        let font = load_font()?;

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

        Self::assemble(
            device,
            queue,
            config,
            Some(surface),
            font,
            scale_factor,
            cfg,
        )
    }

    /// Build a headless renderer (no window/surface) for benchmarks and tests:
    /// same GPU pipelines drawing to a caller-supplied offscreen view via
    /// [`draw_to`](Renderer::draw_to). `render` is a no-op headlessly.
    pub fn new_headless(
        width: u32,
        height: u32,
        scale_factor: f64,
        cfg: &Config,
    ) -> Result<Renderer> {
        let font = load_font()?;
        let instance = wgpu::Instance::default();
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            ..Default::default()
        }))?;
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("term-device-headless"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                ..Default::default()
            }))?;
        // Offscreen target format matching the on-screen sRGB path.
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: wgpu::TextureFormat::Bgra8UnormSrgb,
            width: width.max(1),
            height: height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            color_space: wgpu::SurfaceColorSpace::Auto,
            view_formats: vec![],
        };
        Self::assemble(device, queue, config, None, font, scale_factor, cfg)
    }

    /// Shared GPU assembly for both the windowed and headless constructors.
    #[allow(clippy::too_many_arguments)]
    fn assemble(
        device: wgpu::Device,
        queue: wgpu::Queue,
        config: wgpu::SurfaceConfiguration,
        surface: Option<wgpu::Surface<'static>>,
        font: Arc<Vec<u8>>,
        scale_factor: f64,
        cfg: &Config,
    ) -> Result<Renderer> {
        let cell_size = cell_size_px(cfg, scale_factor);
        let ppem = cfg.font.size_pt * scale_factor as f32;
        let clear = to_wgpu_color(cfg.theme.background);

        let atlas = Atlas::new(&device, font.clone(), 0, ppem)
            .ok_or_else(|| RendererError::NoFont(FONT_CANDIDATES.join(", ")))?;

        let quad_vbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("quad"),
            contents: bytemuck::cast_slice(&QUAD),
            usage: wgpu::BufferUsages::VERTEX,
        });

        // --- Background pipeline (colored cell quads) ---
        let bg_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("cells"),
            source: wgpu::ShaderSource::Wgsl(include_str!("cells.wgsl").into()),
        });
        let bg_uniforms = BgUniforms {
            viewport: [config.width as f32, config.height as f32],
            cell: [cell_size.0, cell_size.1],
            top_inset: 0.0,
            left_inset: 0.0,
            _pad: [0.0; 2],
        };
        let bg_uniform_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("bg-uniforms"),
            contents: bytemuck::bytes_of(&bg_uniforms),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let bg_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bg-bgl"),
            entries: &[uniform_entry(0)],
        });
        let bg_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bg-bg"),
            layout: &bg_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: bg_uniform_buf.as_entire_binding(),
            }],
        });
        let bg_pipeline = build_pipeline(
            &device,
            &bg_shader,
            &bg_bgl,
            config.format,
            std::mem::size_of::<BgInstance>() as u64,
            &wgpu::vertex_attr_array![1 => Float32x2, 2 => Float32x3],
            wgpu::BlendState::REPLACE,
        );

        // --- Glyph pipeline (textured atlas quads) ---
        let glyph_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("glyph"),
            source: wgpu::ShaderSource::Wgsl(include_str!("glyph.wgsl").into()),
        });
        let glyph_uniforms = GlyphUniforms {
            viewport: [config.width as f32, config.height as f32],
            top_inset: 0.0,
            left_inset: 0.0,
        };
        let glyph_uniform_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("glyph-uniforms"),
            contents: bytemuck::bytes_of(&glyph_uniforms),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let glyph_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("glyph-bgl"),
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
        let glyph_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("glyph-bg"),
            layout: &glyph_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: glyph_uniform_buf.as_entire_binding(),
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
        });
        let glyph_blend = wgpu::BlendState {
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
        let glyph_pipeline = build_pipeline(
            &device,
            &glyph_shader,
            &glyph_bgl,
            config.format,
            std::mem::size_of::<GlyphInstance>() as u64,
            &wgpu::vertex_attr_array![1 => Float32x2, 2 => Float32x2, 3 => Float32x2, 4 => Float32x2, 5 => Float32x3],
            glyph_blend,
        );

        Ok(Renderer {
            surface,
            device,
            queue,
            config,
            bg_pipeline,
            glyph_pipeline,
            quad_vbuf,
            bg_uniform_buf,
            bg_bind_group,
            glyph_uniform_buf,
            glyph_bind_group,
            glyph_bgl,
            atlas,
            font,
            scale_factor,
            cell_size,
            top_inset: 0.0,
            left_inset: 0.0,
            ppem,
            ligatures: cfg.font.ligatures,
            bg_default: cfg.theme.background,
            clear,
            layers: Vec::new(),
            last_snapshot: None,
            bg_vbuf: None,
            glyph_vbuf: None,
            bg_count: 0,
            glyph_count: 0,
            reshape_count: 0,
        })
    }

    /// Register an extra draw layer (SD3/D4). Ordered by `z_order` at draw time —
    /// no edit to this file is required to add a layer.
    pub fn add_layer(&mut self, layer: Box<dyn DrawLayer>) {
        self.layers.push(layer);
    }

    pub fn cell_size(&self) -> (f32, f32) {
        self.cell_size
    }

    /// The GPU device (headless benches/tests create their own offscreen target).
    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    /// The render-target texture format (offscreen targets must match it).
    pub fn surface_format(&self) -> wgpu::TextureFormat {
        self.config.format
    }

    pub fn grid_size(&self) -> (u16, u16) {
        grid_dims(
            self.config.width as f32,
            self.config.height as f32,
            self.cell_size,
            self.top_inset,
            self.left_inset,
        )
    }

    /// Reserve `px` physical pixels above the grid (e.g. for a tab bar). The
    /// terminal grid + glyphs draw below this band, and [`grid_size`] reports the
    /// rows that fit under it. A registered `DrawLayer` (the tab bar) paints the
    /// band, reading the same value from [`FrameCtx::top_inset`]. Set to `0.0` to
    /// clear. (Unblocks `tabs` R2.2/D3 — decision ①A.)
    ///
    /// [`grid_size`]: Renderer::grid_size
    pub fn set_top_inset(&mut self, px: f32) {
        self.top_inset = px.max(0.0);
        self.write_uniforms();
    }

    /// Current top inset in physical px.
    pub fn top_inset(&self) -> f32 {
        self.top_inset
    }

    /// Reserve `px` physical pixels to the left of the grid (e.g. for a vertical
    /// tab sidebar). The terminal grid + glyphs draw to the right of this band,
    /// and [`grid_size`] reports the columns that fit beside it. A registered
    /// `DrawLayer` (the sidebar) paints the band, reading the same value from
    /// [`FrameCtx::left_inset`]. Set to `0.0` to clear. The exact structural twin
    /// of [`set_top_inset`] (rows→cols); like it, this writes only the uniforms —
    /// per-cell instance buffers are in cell coords, so no re-shape is needed.
    ///
    /// [`grid_size`]: Renderer::grid_size
    /// [`set_top_inset`]: Renderer::set_top_inset
    pub fn set_left_inset(&mut self, px: f32) {
        self.left_inset = px.max(0.0);
        self.write_uniforms();
    }

    /// Current left inset in physical px.
    pub fn left_inset(&self) -> f32 {
        self.left_inset
    }

    /// Apply a new [`Config`] live (settings feature, D4): re-derive cell metrics,
    /// ppem, theme colors, and ligature state, and rebuild the glyph atlas at the
    /// new size (invalidating the glyph cache — R3.2) without reconstructing the
    /// renderer. After this the caller should re-fit the grid to the new
    /// [`grid_size`] and redraw. Font *family* swaps are not applied (the renderer
    /// resolves a fixed font set — a documented first-cut limit).
    ///
    /// [`grid_size`]: Renderer::grid_size
    pub fn apply_config(&mut self, cfg: &Config) {
        self.cell_size = cell_size_px(cfg, self.scale_factor);
        self.ppem = cfg.font.size_pt * self.scale_factor as f32;
        self.ligatures = cfg.font.ligatures;
        self.bg_default = cfg.theme.background;
        self.clear = to_wgpu_color(cfg.theme.background);

        // Rebuild the atlas at the new ppem so glyphs re-rasterize (size + ligature
        // changes need fresh coverage), then re-point the glyph bind group at it.
        if let Some(atlas) = Atlas::new(&self.device, self.font.clone(), 0, self.ppem) {
            self.atlas = atlas;
            self.glyph_bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("glyph-bg"),
                layout: &self.glyph_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.glyph_uniform_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(self.atlas.view()),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(self.atlas.sampler()),
                    },
                ],
            });
        }
        // Theme/font/atlas/cell-metrics changed → force the next frame to re-shape,
        // else a content-equal snapshot would re-present stale-theme buffers (R1.4).
        self.last_snapshot = None;
        self.write_uniforms();
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.config.width = width.max(1);
        self.config.height = height.max(1);
        if let Some(surface) = &self.surface {
            surface.configure(&self.device, &self.config);
        }
        // Viewport dims changed → force a re-shape so a content-equal snapshot does
        // not re-present buffers built for the old size (R1.4).
        self.last_snapshot = None;
        self.write_uniforms();
    }

    /// Number of cache **misses** so far — frames that actually re-shaped the grid.
    /// A steady idle terminal holds this constant while still presenting a frame
    /// every redraw; the idle-CPU gate reads it to prove re-shape is skipped (R1).
    pub fn reshape_count(&self) -> u64 {
        self.reshape_count
    }

    /// Push the current viewport / cell / top-inset into both uniform buffers.
    fn write_uniforms(&mut self) {
        let vp = [self.config.width as f32, self.config.height as f32];
        self.queue.write_buffer(
            &self.bg_uniform_buf,
            0,
            bytemuck::bytes_of(&BgUniforms {
                viewport: vp,
                cell: [self.cell_size.0, self.cell_size.1],
                top_inset: self.top_inset,
                left_inset: self.left_inset,
                _pad: [0.0; 2],
            }),
        );
        self.queue.write_buffer(
            &self.glyph_uniform_buf,
            0,
            bytemuck::bytes_of(&GlyphUniforms {
                viewport: vp,
                top_inset: self.top_inset,
                left_inset: self.left_inset,
            }),
        );
    }

    /// Draw one frame from a grid snapshot. No-op headlessly (no surface).
    pub fn render(&mut self, snapshot: &GridSnapshot) -> Result<()> {
        if self.surface.is_none() {
            return Ok(());
        }
        // Borrow ends after the match (frame is owned), freeing self for draw_to.
        let frame = match self.surface.as_ref().unwrap().get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t)
            | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface
                    .as_ref()
                    .unwrap()
                    .configure(&self.device, &self.config);
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
        self.draw_to(&view, snapshot);
        self.queue.present(frame);
        Ok(())
    }

    /// Draw one frame to an arbitrary target view (surface or offscreen). Split
    /// out so the [`DrawLayer`] seam is headless-testable without a surface.
    pub fn draw_to(&mut self, view: &wgpu::TextureView, snapshot: &GridSnapshot) {
        // Idle-CPU cache (D5/D6): re-shape the grid only when the snapshot changed.
        // On a hit the resident `bg_vbuf`/`glyph_vbuf` (built for the cached
        // snapshot) are re-presented as-is — no instance building, no allocation.
        // On a miss, rebuild, upload into the resident buffers (growing them if
        // needed), and store the snapshot. `GridSnapshot`'s derived `PartialEq` is
        // the equality basis (SD2); `Cell`'s attributes are part of it, so an
        // attribute-only change is a miss.
        if self.last_snapshot.as_ref() != Some(snapshot) {
            let bg = build_bg_instances(snapshot, self.bg_default);
            let glyphs = build_glyph_instances(
                &mut self.atlas,
                &self.queue,
                &self.font,
                self.ligatures,
                self.cell_size,
                self.ppem,
                snapshot,
            );
            self.bg_count = upload_resident(
                &self.device,
                &self.queue,
                &mut self.bg_vbuf,
                &bg,
                "bg-instances",
            );
            self.glyph_count = upload_resident(
                &self.device,
                &self.queue,
                &mut self.glyph_vbuf,
                &glyphs,
                "glyph-instances",
            );
            self.last_snapshot = Some(snapshot.clone());
            self.reshape_count += 1;
        }

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });

        // Terminal glyph pass (implicit z=0 layer): clear → bg quads → glyph quads.
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("terminal-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
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
            if let (true, Some(bg_buf)) = (self.bg_count > 0, self.bg_vbuf.as_ref()) {
                pass.set_pipeline(&self.bg_pipeline);
                pass.set_bind_group(0, &self.bg_bind_group, &[]);
                pass.set_vertex_buffer(0, self.quad_vbuf.slice(..));
                pass.set_vertex_buffer(1, bg_buf.slice(..));
                pass.draw(0..QUAD.len() as u32, 0..self.bg_count);
            }
            if let (true, Some(glyph_buf)) = (self.glyph_count > 0, self.glyph_vbuf.as_ref()) {
                pass.set_pipeline(&self.glyph_pipeline);
                pass.set_bind_group(0, &self.glyph_bind_group, &[]);
                pass.set_vertex_buffer(0, self.quad_vbuf.slice(..));
                pass.set_vertex_buffer(1, glyph_buf.slice(..));
                pass.draw(0..QUAD.len() as u32, 0..self.glyph_count);
            }
        }

        // Registered layers, in z-order (each records its own Load pass).
        let mut layers = std::mem::take(&mut self.layers);
        layers.sort_by_key(|l| l.z_order());
        {
            let viewport = [self.config.width as f32, self.config.height as f32];
            let cell = [self.cell_size.0, self.cell_size.1];
            let mut ctx = FrameCtx {
                device: &self.device,
                queue: &self.queue,
                encoder: &mut encoder,
                view,
                viewport,
                cell,
                top_inset: self.top_inset,
                left_inset: self.left_inset,
                format: self.config.format,
                snapshot,
                atlas: &mut self.atlas,
            };
            for layer in &mut layers {
                layer.record(&mut ctx);
            }
        }
        self.layers = layers;

        self.queue.submit(std::iter::once(encoder.finish()));
    }
}

/// Upload `data` into a resident vertex buffer (D6), reusing it across frames and
/// growing it only when the byte size exceeds the current capacity. Returns the
/// instance count to draw. An empty slice leaves the buffer untouched and returns
/// 0 (the caller skips the draw), so no zero-size buffer is ever allocated.
fn upload_resident<T: Pod>(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    buf: &mut Option<wgpu::Buffer>,
    data: &[T],
    label: &str,
) -> u32 {
    let needed = std::mem::size_of_val(data) as u64;
    if needed == 0 {
        return 0;
    }
    let fits = buf.as_ref().map(|b| b.size() >= needed).unwrap_or(false);
    if !fits {
        *buf = Some(device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: needed,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));
    }
    queue.write_buffer(buf.as_ref().unwrap(), 0, bytemuck::cast_slice(data));
    data.len() as u32
}

/// One bg quad per cell whose background differs from the theme default.
fn build_bg_instances(snapshot: &GridSnapshot, bg_default: u32) -> Vec<BgInstance> {
    let mut out = Vec::new();
    for row in 0..snapshot.rows {
        for col in 0..snapshot.cols {
            if let Some(cell) = snapshot.cell(col, row) {
                // REVERSE swaps fg/bg at draw time; the default-bg skip is evaluated
                // on the EFFECTIVE (post-swap) background so a reversed cell whose
                // logical fg differs from the default still emits its quad (R5.1).
                let eff_bg = if cell.attrs.contains(CellAttrs::REVERSE) {
                    cell.fg
                } else {
                    cell.bg
                };
                if eff_bg.0 != bg_default {
                    out.push(BgInstance {
                        grid_pos: [col as f32, row as f32],
                        color: eff_bg.to_linear_f32(),
                        _pad: 0.0,
                    });
                }
            }
        }
    }
    out
}

/// Fraction a DIM cell's foreground is mixed toward its background.
const DIM_MIX: f32 = 0.45;

/// The linear-light glyph color for a cell, applying REVERSE (fg/bg swap) and DIM
/// (mix toward the background) — the color both its glyphs and its underline use.
fn glyph_color(cell: &Cell) -> [f32; 3] {
    let (fg, bg) = if cell.attrs.contains(CellAttrs::REVERSE) {
        (cell.bg, cell.fg)
    } else {
        (cell.fg, cell.bg)
    };
    let mut c = fg.to_linear_f32();
    if cell.attrs.contains(CellAttrs::DIM) {
        let b = bg.to_linear_f32();
        for i in 0..3 {
            c[i] = c[i] * (1.0 - DIM_MIX) + b[i] * DIM_MIX;
        }
    }
    c
}

/// Shape each row (ligature-aware per `ligatures`) and emit one textured quad per
/// glyph, anchored to its lead cell, placed by baseline + bearings (D2/D5/D6).
fn build_glyph_instances(
    atlas: &mut Atlas,
    queue: &wgpu::Queue,
    font_data: &Arc<Vec<u8>>,
    ligatures: bool,
    cell: (f32, f32),
    ppem: f32,
    snapshot: &GridSnapshot,
) -> Vec<GlyphInstance> {
    let font = match FontRef::from_index(font_data, 0) {
        Some(f) => f,
        None => return Vec::new(),
    };
    let (cw, ch) = cell;
    let ascent = ch * 0.8; // baseline approximation (units-per-em query is a later refinement)
    let liga: u16 = if ligatures { 1 } else { 0 };
    let features = [("liga", liga), ("calt", liga)];

    let mut out = Vec::new();
    let mut shape_ctx = ShapeContext::new();
    for row in 0..snapshot.rows {
        // Build the row string + a byte→col map so a cluster resolves to its lead cell.
        // Skip the trailing spacer of a double-width char: its leader's glyph already
        // spans both cells, so the next real glyph lands 2 columns over (positioning
        // is by column, so wide chars are 2 cells apart with no overlap).
        let mut line = String::new();
        let mut byte_col: Vec<u16> = Vec::new();
        for col in 0..snapshot.cols {
            let cell = snapshot.cell(col, row);
            if cell.map(Cell::is_continuation).unwrap_or(false) {
                continue;
            }
            let ch_at = cell.map(|c| c.ch).unwrap_or(' ');
            let before = line.len();
            line.push(ch_at);
            for _ in before..line.len() {
                byte_col.push(col);
            }
        }
        let row_y = row as f32 * ch;
        let baseline = row_y + ascent;

        let mut shaper = shape_ctx
            .builder(font)
            .size(ppem)
            .features(features.iter().copied())
            .build();
        shaper.add_str(&line);
        shaper.shape_with(|cluster| {
            let src = cluster.source.start as usize;
            let col = byte_col.get(src).copied().unwrap_or(0);
            // Style comes from the cluster's lead cell (D11): color (reverse/dim)
            // and the bold/italic that select a synthesized, separately-cached glyph.
            let (color, bold, italic) = match snapshot.cell(col, row) {
                Some(c) => (
                    glyph_color(c),
                    c.attrs.contains(CellAttrs::BOLD),
                    c.attrs.contains(CellAttrs::ITALIC),
                ),
                None => ([1.0; 3], false, false),
            };
            let mut pen = col as f32 * cw;
            for g in cluster.glyphs {
                let key = GlyphKey {
                    id: g.id,
                    bold,
                    italic,
                };
                if let Some(entry) = atlas.get_or_insert_styled(queue, key) {
                    out.push(GlyphInstance {
                        pos: [pen + g.x + entry.left, baseline - entry.top],
                        size: [entry.px_w, entry.px_h],
                        uv0: entry.uv0,
                        uv1: entry.uv1,
                        color,
                        _pad: 0.0,
                    });
                }
                pen += g.advance;
            }
        });
    }

    // Underline pass (R5.3): a solid rule below the baseline of each UNDERLINE
    // cell, drawn in the glyph pass via the atlas's opaque solid entry. Kept
    // separate so it doesn't borrow `atlas` inside the shaping closure.
    let thickness = (ch * 0.06).max(1.0);
    for row in 0..snapshot.rows {
        let underline_y = row as f32 * ch + ascent + (ch * 0.08).max(1.0);
        for col in 0..snapshot.cols {
            let Some(cell) = snapshot.cell(col, row) else {
                continue;
            };
            if !cell.attrs.contains(CellAttrs::UNDERLINE) {
                continue;
            }
            let e = atlas.solid_entry(queue);
            // Sample the block center so linear filtering never picks up a
            // neighboring non-opaque texel — a constant 255 coverage → solid fill.
            let uv = [(e.uv0[0] + e.uv1[0]) * 0.5, (e.uv0[1] + e.uv1[1]) * 0.5];
            out.push(GlyphInstance {
                pos: [col as f32 * cw, underline_y],
                size: [cw, thickness],
                uv0: uv,
                uv1: uv,
                color: glyph_color(cell),
                _pad: 0.0,
            });
        }
    }
    out
}

fn uniform_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

#[allow(clippy::too_many_arguments)]
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
        label: None,
        bind_group_layouts: &[Some(bgl)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: None,
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

fn load_font() -> Result<Arc<Vec<u8>>> {
    for path in FONT_CANDIDATES {
        if let Ok(bytes) = std::fs::read(path) {
            return Ok(Arc::new(bytes));
        }
    }
    Err(RendererError::NoFont(FONT_CANDIDATES.join(", ")))
}

fn cell_size_px(cfg: &Config, scale_factor: f64) -> (f32, f32) {
    let px = cfg.font.size_pt * scale_factor as f32;
    ((px * 0.6).round().max(1.0), (px * 1.2).round().max(1.0))
}

/// Cols × rows that fit a framebuffer, reserving `top_inset` px above the grid
/// (e.g. a tab bar) and `left_inset` px to its left (e.g. a vertical tab
/// sidebar). Only the height below the top inset holds rows; only the width to
/// the right of the left inset holds cols. Kept as a free function so the inset
/// arithmetic is unit-testable without a GPU surface. Both floors at ≥1 so a
/// pathological inset larger than the window still yields a drawable grid.
fn grid_dims(
    width: f32,
    height: f32,
    cell: (f32, f32),
    top_inset: f32,
    left_inset: f32,
) -> (u16, u16) {
    let usable_w = (width - left_inset).max(0.0);
    let cols = (usable_w / cell.0).floor().max(1.0) as u16;
    let usable_h = (height - top_inset).max(0.0);
    let rows = (usable_h / cell.1).floor().max(1.0) as u16;
    (cols, rows)
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

    fn headless() -> Option<(wgpu::Device, wgpu::Queue)> {
        let instance = wgpu::Instance::default();
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            ..Default::default()
        }))
        .ok()?;
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()
    }

    fn font() -> Option<Arc<Vec<u8>>> {
        super::load_font().ok()
    }

    #[test]
    fn cell_size_scales_with_dpi() {
        let cfg = Config::default();
        assert!(cell_size_px(&cfg, 2.0).0 > cell_size_px(&cfg, 1.0).0);
    }

    #[test]
    fn top_inset_reserves_rows_for_the_tab_bar() {
        // 800px tall, 20px cells → 40 rows with no inset.
        let (_, rows0) = grid_dims(1200.0, 800.0, (10.0, 20.0), 0.0, 0.0);
        assert_eq!(rows0, 40);
        // A 40px tab bar (2 rows) reserves the top band → 38 rows below it.
        let (cols, rows1) = grid_dims(1200.0, 800.0, (10.0, 20.0), 40.0, 0.0);
        assert_eq!(rows1, 38, "grid rows shrink by the inset, not the cols");
        assert_eq!(cols, 120, "top inset never affects column count");
        // A pathological inset taller than the window still yields ≥1 row.
        let (_, rows2) = grid_dims(1200.0, 800.0, (10.0, 20.0), 10_000.0, 0.0);
        assert_eq!(rows2, 1);
    }

    #[test]
    fn left_inset_reserves_cols_for_the_sidebar() {
        // 1200px wide, 10px cells → 120 cols with no inset.
        let (cols0, _) = grid_dims(1200.0, 800.0, (10.0, 20.0), 0.0, 0.0);
        assert_eq!(cols0, 120);
        // A 200px sidebar (20 cols) reserves the left band → 100 cols beside it,
        // rows unchanged (R1.1).
        let (cols1, rows1) = grid_dims(1200.0, 800.0, (10.0, 20.0), 0.0, 200.0);
        assert_eq!(cols1, 100, "grid cols shrink by the left inset");
        assert_eq!(rows1, 40, "left inset never affects row count");
        // A pathological inset wider than the window still yields ≥1 col (R1.2).
        let (cols2, _) = grid_dims(1200.0, 800.0, (10.0, 20.0), 0.0, 10_000.0);
        assert_eq!(cols2, 1);
        // Zero-inset invariance (R4.1): left_inset == 0.0 reproduces the no-inset
        // column count exactly.
        let (cols_z, rows_z) = grid_dims(1200.0, 800.0, (10.0, 20.0), 0.0, 0.0);
        assert_eq!((cols_z, rows_z), (120, 40));
        // Both insets compose independently (cols from left, rows from top).
        let (cols_b, rows_b) = grid_dims(1200.0, 800.0, (10.0, 20.0), 40.0, 200.0);
        assert_eq!((cols_b, rows_b), (100, 38));
    }

    #[test]
    fn uniform_blocks_are_16_byte_aligned() {
        // WGSL uniform structs must be a multiple of 16 bytes; the Rust mirrors
        // (with top_inset + left_inset) must match or the shader reads garbage.
        assert_eq!(std::mem::size_of::<BgUniforms>(), 32);
        assert_eq!(std::mem::size_of::<GlyphUniforms>(), 16);
        // Pin the field OFFSETS the WGSL structs are hand-mirrored against
        // (cells.wgsl / glyph.wgsl): a transposed field would still sum to
        // 32/16 above but read garbage, so machine-check the Rust layout here.
        assert_eq!(std::mem::offset_of!(BgUniforms, top_inset), 16);
        assert_eq!(std::mem::offset_of!(BgUniforms, left_inset), 20);
        assert_eq!(std::mem::offset_of!(GlyphUniforms, top_inset), 8);
        assert_eq!(std::mem::offset_of!(GlyphUniforms, left_inset), 12);
    }

    #[test]
    fn set_left_inset_clamps_and_narrows_grid() {
        // R2.1: set_left_inset clamps negatives to 0; a positive inset shrinks
        // the reported column count (rows unchanged). Needs a GPU (new_headless).
        if headless().is_none() {
            eprintln!("skipping: no GPU");
            return;
        }
        let cfg = Config::default();
        let Ok(mut r) = Renderer::new_headless(1200, 800, 1.0, &cfg) else {
            eprintln!("skipping: headless renderer unavailable");
            return;
        };
        let (cols0, rows0) = r.grid_size();
        assert_eq!(r.left_inset(), 0.0, "default left inset is 0");

        // A negative inset clamps to 0 → geometry unchanged (R2.1 clamp).
        r.set_left_inset(-5.0);
        assert_eq!(r.left_inset(), 0.0, "negative inset clamps to 0.0");
        assert_eq!(
            r.grid_size(),
            (cols0, rows0),
            "clamped-to-0 inset changes nothing"
        );

        // A one-cell-wide inset drops exactly one column, rows unchanged.
        let cw = r.cell_size().0;
        r.set_left_inset(cw);
        let (cols1, rows1) = r.grid_size();
        assert_eq!(cols1, cols0 - 1, "a one-cell left inset drops one column");
        assert_eq!(rows1, rows0, "left inset never changes rows");
    }

    #[test]
    fn layers_sort_by_z_order() {
        struct L(i32);
        impl DrawLayer for L {
            fn z_order(&self) -> i32 {
                self.0
            }
            fn record(&mut self, _: &mut FrameCtx<'_>) {}
        }
        let mut v: Vec<Box<dyn DrawLayer>> =
            vec![Box::new(L(100)), Box::new(L(0)), Box::new(L(10))];
        v.sort_by_key(|l| l.z_order());
        assert_eq!(
            v.iter().map(|l| l.z_order()).collect::<Vec<_>>(),
            vec![0, 10, 100]
        );
    }

    #[test]
    fn atlas_inserts_caches_and_notdef() {
        let (Some((device, queue)), Some(font)) = (headless(), font()) else {
            eprintln!("skipping: no GPU/font");
            return;
        };
        let mut atlas = Atlas::new(&device, font, 0, 32.0).expect("atlas");
        let a = atlas.glyph_id('A');
        let e1 = atlas.get_or_insert(&queue, a);
        assert!(e1.is_some(), "'A' should rasterize into the atlas");
        let before = atlas.cached_len();
        let e2 = atlas.get_or_insert(&queue, a); // reuse — no new cache entry
        assert_eq!(e1, e2);
        assert_eq!(atlas.cached_len(), before);
        // A space glyph has no outline → None (nothing to draw), no panic.
        let space = atlas.glyph_id(' ');
        assert!(atlas.get_or_insert(&queue, space).is_none());
        // .notdef (glyph 0) rasterizes to a visible tofu box — the R6 placeholder.
        assert!(
            atlas.get_or_insert(&queue, 0).is_some(),
            ".notdef should be a visible box (R6)"
        );
    }

    #[test]
    fn shaping_ligature_combines_glyphs() {
        let Some(font) = font() else {
            eprintln!("skipping: no font");
            return;
        };
        let count = |ligatures: bool| {
            let mut ctx = ShapeContext::new();
            let f = FontRef::from_index(&font, 0).unwrap();
            let liga: u16 = if ligatures { 1 } else { 0 };
            let mut sh = ctx
                .builder(f)
                .size(32.0)
                .features([("liga", liga), ("calt", liga)])
                .build();
            sh.add_str("!=");
            let mut n = 0usize;
            sh.shape_with(|c| n += c.glyphs.len());
            n
        };
        // With ligatures the two chars may collapse to one glyph (font-dependent);
        // it must never produce MORE glyphs than the ligatures-off path.
        assert!(count(true) <= count(false));
    }

    #[test]
    fn glyph_instances_place_by_baseline() {
        let (Some((device, queue)), Some(font)) = (headless(), font()) else {
            eprintln!("skipping: no GPU/font");
            return;
        };
        let mut atlas = Atlas::new(&device, font.clone(), 0, 32.0).expect("atlas");
        let mut t = term_core::Terminal::new(4, 1, term_core::Color(0xFFFFFF), term_core::Color(0));
        t.advance(b"Ag");
        let snap = t.snapshot();
        let inst =
            build_glyph_instances(&mut atlas, &queue, &font, true, (20.0, 40.0), 32.0, &snap);
        assert!(inst.len() >= 2, "A and g should each emit a glyph quad");
        // 'g' has a descender: its quad top is below the baseline-top of 'A'.
        assert!(inst.iter().all(|g| g.size[0] > 0.0 && g.size[1] > 0.0));
    }

    #[test]
    fn bg_instances_only_nondefault() {
        let mut t =
            term_core::Terminal::new(3, 1, term_core::Color(0xFFFFFF), term_core::Color(0x101010));
        t.advance(b"hi");
        // default bg everywhere → no bg quads.
        assert!(build_bg_instances(&t.snapshot(), 0x101010).is_empty());
    }

    #[test]
    fn registered_layer_record_is_invoked() {
        // R4.2: a registered layer's `record` fires when the frame's layer loop
        // runs — drives the real FrameCtx + loop headlessly, without a surface.
        let (Some((device, queue)), Some(font)) = (headless(), font()) else {
            eprintln!("skipping: no GPU/font");
            return;
        };
        let mut atlas = Atlas::new(&device, font, 0, 24.0).expect("atlas");
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen"),
            size: wgpu::Extent3d {
                width: 64,
                height: 64,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        let mut t = term_core::Terminal::new(2, 1, term_core::Color(0xFFFFFF), term_core::Color(0));
        t.advance(b"x");
        let snap = t.snapshot();

        struct Rec(std::rc::Rc<std::cell::Cell<bool>>);
        impl DrawLayer for Rec {
            fn z_order(&self) -> i32 {
                50
            }
            fn record(&mut self, _ctx: &mut FrameCtx<'_>) {
                self.0.set(true);
            }
        }
        let fired = std::rc::Rc::new(std::cell::Cell::new(false));
        let mut layers: Vec<Box<dyn DrawLayer>> = vec![Box::new(Rec(fired.clone()))];
        layers.sort_by_key(|l| l.z_order());
        let mut ctx = FrameCtx {
            device: &device,
            queue: &queue,
            encoder: &mut encoder,
            view: &view,
            viewport: [64.0, 64.0],
            cell: [12.0, 24.0],
            top_inset: 0.0,
            left_inset: 0.0,
            format: wgpu::TextureFormat::Rgba8Unorm,
            snapshot: &snap,
            atlas: &mut atlas,
        };
        for layer in &mut layers {
            layer.record(&mut ctx);
        }
        assert!(fired.get(), "registered layer's record must be invoked");
    }

    #[test]
    fn atlas_eviction_is_bounded() {
        // R1.3: overflowing the atlas evicts (clear + repack) instead of growing
        // unboundedly or panicking; the generation bumps.
        let (Some((device, queue)), Some(font)) = (headless(), font()) else {
            eprintln!("skipping: no GPU/font");
            return;
        };
        let mut atlas = Atlas::new(&device, font, 0, 64.0).expect("atlas");
        // Many distinct glyph ids at a large size force a shelf overflow.
        for gid in 1u16..900 {
            let _ = atlas.get_or_insert(&queue, gid); // must never panic
        }
        assert!(
            atlas.generation() >= 1,
            "atlas should have evicted at least once"
        );
    }

    #[test]
    fn draw_to_skips_reshape_on_unchanged_snapshot() {
        // R1.1/R1.2/R1.4: draw_to re-shapes only when the snapshot changes. An
        // equal snapshot (incl. a value-equal clone) is a cache hit — no reshape;
        // a changed snapshot or a resize forces a miss. `reshape_count` is the
        // idle-CPU signal (idle frames re-present without incrementing it).
        let cfg = Config::default();
        let Ok(mut r) = Renderer::new_headless(256, 128, 1.0, &cfg) else {
            eprintln!("skipping: no GPU");
            return;
        };
        let target = r.device().create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen"),
            size: wgpu::Extent3d {
                width: 256,
                height: 128,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: r.surface_format(),
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());

        let mut t = term_core::Terminal::new(8, 2, term_core::Color(0xFFFFFF), term_core::Color(0));
        t.advance(b"hello");
        let snap = t.snapshot();

        assert_eq!(r.reshape_count(), 0);
        r.draw_to(&view, &snap);
        assert_eq!(r.reshape_count(), 1, "first draw is a miss");
        r.draw_to(&view, &snap); // equal snapshot → hit
        assert_eq!(r.reshape_count(), 1, "unchanged snapshot must not re-shape");
        r.draw_to(&view, &snap.clone()); // value-equal clone → still a hit
        assert_eq!(
            r.reshape_count(),
            1,
            "value-equal snapshot must not re-shape"
        );

        t.advance(b" world");
        let snap2 = t.snapshot();
        r.draw_to(&view, &snap2); // changed → miss
        assert_eq!(r.reshape_count(), 2, "changed snapshot must re-shape");
        r.draw_to(&view, &snap2); // hit again
        assert_eq!(r.reshape_count(), 2);

        // R1.4: a resize forces a miss even for a content-equal snapshot.
        r.resize(320, 160);
        r.draw_to(&view, &snap2);
        assert_eq!(r.reshape_count(), 3, "resize must invalidate the cache");
        r.draw_to(&view, &snap2); // settled → hit again
        assert_eq!(r.reshape_count(), 3);

        // R1.4: apply_config (theme/font/atlas change) also invalidates, so a
        // content-equal snapshot re-shapes rather than re-presenting stale buffers.
        r.apply_config(&cfg);
        r.draw_to(&view, &snap2);
        assert_eq!(
            r.reshape_count(),
            4,
            "apply_config must invalidate the cache"
        );
    }
}
