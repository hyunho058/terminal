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
pub mod font_set;
pub mod glyph;
pub mod image_layer;
pub mod overlay_layer;
pub mod preedit_layer;
pub mod tab_bar;

use atlas::{Atlas, GlyphKey};
use bytemuck::{Pod, Zeroable};
use common::config::Config;
use font_set::FONT_CANDIDATES;
use swash::shape::ShapeContext;
use swash::FontRef;
use term_core::{Cell, CellAttrs, GridSnapshot};
use thiserror::Error;
use wgpu::util::DeviceExt;

pub use font_set::{FontData, FontFace, FontSet};

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
    /// The ordered fallback chain (D1): `faces()[0]` is the primary Latin face,
    /// later faces cover what it does not (Hangul today). Owned here; the atlas
    /// and the shaper share the same mapped bytes through `FontData` clones.
    fonts: FontSet,
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
        let fonts = FontSet::load()?;

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
            fonts,
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
        let fonts = FontSet::load()?;
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
        Self::assemble(device, queue, config, None, fonts, scale_factor, cfg)
    }

    /// Shared GPU assembly for both the windowed and headless constructors.
    #[allow(clippy::too_many_arguments)]
    fn assemble(
        device: wgpu::Device,
        queue: wgpu::Queue,
        config: wgpu::SurfaceConfiguration,
        surface: Option<wgpu::Surface<'static>>,
        fonts: FontSet,
        scale_factor: f64,
        cfg: &Config,
    ) -> Result<Renderer> {
        let cell_size = cell_size_px(cfg, scale_factor);
        let ppem = cfg.font.size_pt * scale_factor as f32;
        let clear = to_wgpu_color(cfg.theme.background);

        // One rasterizer per face of the chain, all sharing the mapped bytes (D7):
        // no face's font file is ever copied into the heap here (R3.2).
        let atlas = Atlas::new(&device, &fonts, ppem)
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
            fonts,
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
        // Rebuilding shares the same mapped bytes (D7/R3.2): a live ppem change
        // must not re-read (or re-copy) the 55 MB fallback face on every tick of
        // the config watcher.
        if let Some(atlas) = Atlas::new(&self.device, &self.fonts, self.ppem) {
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
        // D17/R3.4: the atlas generation this frame starts from. Anything that packs
        // a glyph — the grid shaping below, or a `DrawLayer` further down — can
        // overflow the atlas and evict, which invalidates every UV already handed
        // out. Compared again once the layers have recorded.
        let generation_at_entry = self.atlas.generation();
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
                &self.fonts,
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

        // D17/R3.4: a layer (tab bar / overlay / preedit) packing new glyphs may
        // have evicted the atlas. The grid's resident `glyph_vbuf` then holds UVs
        // into a texture that has been repacked, and because the snapshot did not
        // change the cache hit above would re-present those stale UVs *forever* —
        // permanent on-screen corruption, not a transient glitch. Dropping the
        // cached snapshot forces the next frame to re-shape against the new
        // generation. (Reachable by construction: while an IME composes, keystrokes
        // are suppressed, so the grid snapshot is static while the preedit layer
        // rasterizes a brand-new syllable per keystroke.)
        //
        // DEPENDENCY: this only reaches the screen because the app's
        // `RedrawRequested` arm (`app/src/terminal_app.rs`, search
        // `request_redraw` in that arm) drives a *continuous* redraw (paused
        // while occluded) — the renderer never
        // requests a frame itself. If that ever reverts to on-demand redraws, this
        // corruption path silently reopens and this invalidation must be paired with
        // an explicit redraw request. The current frame still draws the stale UVs;
        // intra-frame invalidation is an accepted Known Gap.
        if self.atlas.generation() != generation_at_entry {
            self.last_snapshot = None;
        }

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

/// A maximal span of consecutive characters that resolved to the *same* face — the
/// unit of shaping (D16/R4.1).
///
/// Bounds index the row's **continuation-skipped char sequence** (the order
/// [`row_chars`] produces), never raw column numbers: a wide char occupies two
/// columns but one char, so column-based bounds would desync every run boundary
/// after the first wide char (R4.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Run {
    /// The face every char in the run resolved to. `None` = no face covers them,
    /// so the run draws nothing at all (D5/R4.2).
    face: Option<usize>,
    start_char: usize,
    /// Exclusive.
    end_char: usize,
}

/// Group a row's per-char face resolutions into maximal same-face runs (D16/R4.1).
///
/// The Confirmed Goal's "pick a font per cluster" is circular — a cluster is an
/// *output* of shaping, so the shaping font cannot be chosen from it. The real
/// order is: resolve each **char** to a face, group consecutive equal faces, then
/// shape each group with that face's `FontRef`. Pure, so the grouping rule is
/// table-testable with no GPU, font, or snapshot.
fn split_runs(faces: &[Option<usize>]) -> Vec<Run> {
    let mut runs: Vec<Run> = Vec::new();
    for (i, &face) in faces.iter().enumerate() {
        match runs.last_mut() {
            Some(run) if run.face == face => run.end_char = i + 1,
            _ => runs.push(Run {
                face,
                start_char: i,
                end_char: i + 1,
            }),
        }
    }
    runs
}

/// One row flattened for shaping (D16/R4.3).
struct RowChars {
    /// The row's characters in column order, with the trailing spacer of every
    /// double-width char dropped: its leader's glyph already spans both cells.
    chars: Vec<char>,
    /// `char_col[i]` is the absolute column of `chars[i]` — the anchor its cluster
    /// is placed at (R4.5). Wide chars leave a gap: `한a` → `[0, 2]`.
    char_col: Vec<u16>,
}

/// Whether `(col, row)` holds the trailing spacer of a double-width char.
///
/// The single predicate both the row walk (R4.3) and the span measurement (D6)
/// read, so the two can never disagree about what a wide cell is.
fn is_continuation(snapshot: &GridSnapshot, col: u16, row: u16) -> bool {
    snapshot
        .cell(col, row)
        .map(Cell::is_continuation)
        .unwrap_or(false)
}

/// Walk a row into its shaping input: chars with spacers skipped, plus the
/// absolute column of each (D16/R4.3). Split out of [`build_glyph_instances`] so
/// the indexing basis — the thing wide chars desync — is testable on its own.
fn row_chars(snapshot: &GridSnapshot, row: u16) -> RowChars {
    let mut chars = Vec::new();
    let mut char_col = Vec::new();
    for col in 0..snapshot.cols {
        if is_continuation(snapshot, col, row) {
            continue;
        }
        chars.push(snapshot.cell(col, row).map(|c| c.ch).unwrap_or(' '));
        char_col.push(col);
    }
    RowChars { chars, char_col }
}

/// Cell span of the char led by `col`: 2 when the next column holds its
/// continuation spacer, else 1 (D6).
fn cell_span(snapshot: &GridSnapshot, col: u16, row: u16) -> u16 {
    if is_continuation(snapshot, col.saturating_add(1), row) {
        2
    } else {
        1
    }
}

/// Horizontal offset that centers a cluster inside the cell span it occupies (D6).
///
/// Fallback faces are proportional: a Hangul glyph is roughly `1.0 × ppem` where a
/// two-cell span is `1.2 × ppem`, so anchoring it flush left leaves a visible
/// right-hand gutter. `adv` must be the cluster's **total** advance, not its first
/// glyph's — the placement loop resets `pen` per cluster and then advances by
/// `g.advance` per glyph, so a first-glyph measure mis-centers multi-glyph
/// clusters.
///
/// Two rules are load-bearing:
///
/// * face 0 is always `0.0` (R4.8). The primary's placement arithmetic is left
///   exactly as it was, which is what makes ASCII-only output byte-identical to
///   before the chain existed (R0.3, a hard constraint). Latin placement is not
///   "improved" here.
/// * `max(0.0)` (R4.7): a glyph wider than its span overflows to the **right**
///   only and never bleeds back into the previous cell.
fn center_offset(face: usize, span: u16, cw: f32, adv: f32) -> f32 {
    if face == 0 {
        return 0.0;
    }
    ((span as f32 * cw - adv) * 0.5).max(0.0)
}

/// Shape each row (ligature-aware per `ligatures`) and emit one textured quad per
/// glyph, anchored to its lead cell, placed by baseline + bearings (D2/D5/D6).
///
/// A row is not shaped as one string: it is split into per-face runs (D16) and
/// each run is shaped with its own face's `FontRef`, so Hangul reaches the shaper
/// through the fallback face instead of collapsing to `.notdef` (R0.2/R4.4).
/// Ligature features apply within a run only — a ligature spanning a
/// primary↔fallback boundary is not a thing, so breaking there is correct.
///
/// Takes the whole [`FontSet`] (D16): the set is owned by [`Renderer`] and the
/// atlas holds cloned `FontData` handles to the same mapped bytes, so shaping and
/// rasterization share bytes with a single owner.
fn build_glyph_instances(
    atlas: &mut Atlas,
    queue: &wgpu::Queue,
    fonts: &FontSet,
    ligatures: bool,
    cell: (f32, f32),
    ppem: f32,
    snapshot: &GridSnapshot,
) -> Vec<GlyphInstance> {
    let (cw, ch) = cell;
    let ascent = ch * 0.8; // baseline approximation (units-per-em query is a later refinement)
    let liga: u16 = if ligatures { 1 } else { 0 };
    let features = [("liga", liga), ("calt", liga)];

    let mut out = Vec::new();
    let mut shape_ctx = ShapeContext::new();
    for row in 0..snapshot.rows {
        let RowChars { chars, char_col } = row_chars(snapshot, row);
        // Resolve every char to a face first (D2/D16): `resolve` borrows the atlas
        // immutably while the shaping closure below borrows it mutably, and the
        // face is an *input* to shaping, so it has to be decided up front.
        let faces: Vec<Option<usize>> = chars
            .iter()
            .map(|&c| atlas.resolve(c).map(|key| key.face as usize))
            .collect();
        let baseline = row as f32 * ch + ascent;

        for run in split_runs(&faces) {
            // D5/R4.2: no face covers these chars — emit nothing rather than a wall
            // of tofu. (In degraded mode `resolve` answers face 0 instead, so the
            // primary's `.notdef` still draws and the missing font stays visible.)
            let Some(face_index) = run.face else {
                continue;
            };
            let Some(font) = fonts
                .faces()
                .get(face_index)
                .and_then(|face| FontRef::from_index(&face.data, face.index))
            else {
                continue;
            };

            // The run's string + a **run-relative** byte→column map (R4.5).
            // `cluster.source.start` is an offset into the string handed to *this*
            // shaper, so a row-wide map would read the wrong column for every run
            // after the first. Absolute columns come from `char_col`.
            let mut text = String::new();
            let mut byte_col: Vec<u16> = Vec::new();
            for i in run.start_char..run.end_char {
                let before = text.len();
                text.push(chars[i]);
                for _ in before..text.len() {
                    byte_col.push(char_col[i]);
                }
            }

            let mut shaper = shape_ctx
                .builder(font)
                .size(ppem)
                .features(features.iter().copied())
                .build();
            shaper.add_str(&text);
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
                // D6: a fallback cluster is centered in the span its lead cell
                // covers; face 0 keeps `pen == col * cw` exactly as before (R4.8).
                let adv: f32 = cluster.glyphs.iter().map(|g| g.advance).sum();
                let offset = center_offset(face_index, cell_span(snapshot, col, row), cw, adv);
                let mut pen = col as f32 * cw + offset;
                for g in cluster.glyphs {
                    // The face is the run's, not a guess: glyph ids are per-font
                    // namespaces, so a key without it would draw the primary's
                    // gid N for the fallback's gid N (D7/R3.1).
                    let key = GlyphKey {
                        face: face_index as u16,
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

    /// The primary face of the loaded chain (T2). `None` when no system font is
    /// available, which the GPU-dependent tests below treat as "skip".
    fn primary_face() -> Option<FontFace> {
        FontSet::load().ok()?.faces().first().cloned()
    }

    /// The loaded fallback chain (T2/T3) — what `Atlas::new` now takes. `None`
    /// when no system font is available, which the tests below treat as "skip".
    fn fonts() -> Option<FontSet> {
        FontSet::load().ok()
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
        let (Some((device, queue)), Some(fonts)) = (headless(), fonts()) else {
            eprintln!("skipping: no GPU/font");
            return;
        };
        let mut atlas = Atlas::new(&device, &fonts, 32.0).expect("atlas");
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
        // R2.6: with a fallback face loaded, .notdef is refused outright — those
        // tofu boxes are exactly the defect this feature removes.
        if !atlas.is_degraded() {
            let cached = atlas.cached_len();
            assert!(
                atlas.get_or_insert(&queue, 0).is_none(),
                ".notdef must not be packed while a fallback face exists (R2.6)"
            );
            assert_eq!(
                atlas.cached_len(),
                cached,
                "a refused .notdef must not even occupy a cache slot"
            );
        }
        // R2.5: degraded (no fallback face) keeps the guard off, so .notdef still
        // draws — a missing font must stay *visible*, not silently invisible (D5).
        atlas.degrade_for_test();
        assert!(atlas.is_degraded());
        assert!(
            atlas.get_or_insert(&queue, 0).is_some(),
            ".notdef should stay a visible box in degraded mode (R2.5)"
        );
    }

    #[test]
    fn atlas_resolve_prefers_the_primary_face() {
        // R2.1/R2.2: Latin resolves to face 0 (so its render is untouched — R0.3);
        // Hangul falls through to a later face with a real glyph, not .notdef.
        let (Some((device, _queue)), Some(fonts)) = (headless(), fonts()) else {
            eprintln!("skipping: no GPU/font");
            return;
        };
        let atlas = Atlas::new(&device, &fonts, 32.0).expect("atlas");
        let latin = atlas.resolve('A').expect("the primary covers 'A'");
        assert_eq!(latin.face, 0, "Latin must keep using the primary face");
        assert_ne!(latin.id, 0);
        if atlas.is_degraded() {
            eprintln!("skipping the fallback half: no Hangul face on this machine");
            return;
        }
        let hangul = atlas.resolve('한').expect("some face covers U+D55C");
        assert_ne!(hangul.face, 0, "Hangul must come from a fallback face");
        assert_ne!(hangul.id, 0, "and from a real glyph, not .notdef (R0.2)");
    }

    #[test]
    fn atlas_resolve_reports_uncovered_by_mode() {
        // R2.4 vs R2.5: normal mode declines to draw what no face covers; degraded
        // mode answers with the primary's .notdef so the breakage stays visible.
        let (Some((device, _queue)), Some(fonts)) = (headless(), fonts()) else {
            eprintln!("skipping: no GPU/font");
            return;
        };
        let mut atlas = Atlas::new(&device, &fonts, 32.0).expect("atlas");
        // A private-use codepoint no system font maps.
        let uncovered = '\u{F8FF0}';
        if !atlas.is_degraded() {
            assert_eq!(atlas.resolve(uncovered), None, "R2.4: nothing to draw");
        }
        atlas.degrade_for_test();
        assert_eq!(
            atlas.resolve(uncovered),
            Some(GlyphKey::plain(0)),
            "R2.5: degraded mode keeps the primary .notdef"
        );
    }

    #[test]
    fn atlas_caches_the_same_glyph_id_per_face() {
        // R3.1: glyph ids are per-font namespaces — face 0's gid N and face 1's
        // gid N are different glyphs and must not share a cache slot.
        let (Some((device, queue)), Some(fonts)) = (headless(), fonts()) else {
            eprintln!("skipping: no GPU/font");
            return;
        };
        let mut atlas = Atlas::new(&device, &fonts, 32.0).expect("atlas");
        if atlas.is_degraded() {
            eprintln!("skipping: no fallback face on this machine");
            return;
        }
        let primary = atlas.resolve('A').expect("the primary covers 'A'");
        let same_id_elsewhere = GlyphKey { face: 1, ..primary };
        let before = atlas.cached_len();
        let a = atlas.get_or_insert_styled(&queue, primary);
        let b = atlas.get_or_insert_styled(&queue, same_id_elsewhere);
        assert_eq!(
            atlas.cached_len(),
            before + 2,
            "one cache entry per (face, id)"
        );
        if let (Some(a), Some(b)) = (a, b) {
            assert_ne!(a.uv0, b.uv0, "each face's glyph packs to its own UV rect");
        }
    }

    #[test]
    fn shaping_ligature_combines_glyphs() {
        let Some(face) = primary_face() else {
            eprintln!("skipping: no font");
            return;
        };
        let count = |ligatures: bool| {
            let mut ctx = ShapeContext::new();
            let f = FontRef::from_index(&face.data, face.index).unwrap();
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
        let (Some((device, queue)), Some(fonts)) = (headless(), fonts()) else {
            eprintln!("skipping: no GPU/font");
            return;
        };
        let mut atlas = Atlas::new(&device, &fonts, 32.0).expect("atlas");
        let mut t = term_core::Terminal::new(4, 1, term_core::Color(0xFFFFFF), term_core::Color(0));
        t.advance(b"Ag");
        let snap = t.snapshot();
        let inst =
            build_glyph_instances(&mut atlas, &queue, &fonts, true, (20.0, 40.0), 32.0, &snap);
        assert!(inst.len() >= 2, "A and g should each emit a glyph quad");
        // 'g' has a descender: its quad top is below the baseline-top of 'A'.
        assert!(inst.iter().all(|g| g.size[0] > 0.0 && g.size[1] > 0.0));
    }

    #[test]
    fn split_runs_groups_consecutive_same_face() {
        // R4.1: maximal runs — the same face returning later starts a NEW run, it
        // does not rejoin the earlier one (shaping is per contiguous span).
        let faces = [Some(0), Some(0), Some(1), Some(1), Some(0)];
        assert_eq!(
            split_runs(&faces),
            vec![
                Run {
                    face: Some(0),
                    start_char: 0,
                    end_char: 2
                },
                Run {
                    face: Some(1),
                    start_char: 2,
                    end_char: 4
                },
                Run {
                    face: Some(0),
                    start_char: 4,
                    end_char: 5
                },
            ]
        );
        // The R0.3 shape: an all-primary row is ONE run over the whole row, so it
        // is shaped exactly as the single-font path shaped it.
        assert_eq!(
            split_runs(&[Some(0); 5]),
            vec![Run {
                face: Some(0),
                start_char: 0,
                end_char: 5
            }]
        );
        assert!(split_runs(&[]).is_empty());
    }

    #[test]
    fn split_runs_isolates_uncovered_chars() {
        // R4.2: `None` (no face covers it) is a face value like any other — such
        // chars group together and the caller emits nothing for them (D5).
        let runs = split_runs(&[Some(0), None, None, Some(1)]);
        assert_eq!(runs.len(), 3);
        assert_eq!(runs[1].face, None);
        assert_eq!((runs[1].start_char, runs[1].end_char), (1, 3));
        // Two different covered faces never merge just because neither is `None`.
        assert_eq!(split_runs(&[Some(1), Some(2)]).len(), 2);
    }

    #[test]
    fn row_chars_skips_wide_char_spacers() {
        // R4.3: the indexing basis is the continuation-skipped char sequence, NOT
        // raw columns. `한a` is cols 0(한)/1(spacer)/2(a) → 2 chars at cols [0, 2].
        // Getting this wrong desyncs every run boundary after the first wide char.
        let mut t = term_core::Terminal::new(4, 1, term_core::Color(0xFFFFFF), term_core::Color(0));
        t.advance("한a".as_bytes());
        let snap = t.snapshot();
        let row = row_chars(&snap, 0);
        assert_eq!(row.chars.len(), 2 + 1, "한, a, and the trailing blank col");
        assert_eq!(&row.chars[..2], &['한', 'a']);
        assert_eq!(&row.char_col[..2], &[0, 2]);
        assert_eq!(row.chars.len(), row.char_col.len());
        // The span predicate agrees with the walk: the lead cell covers 2 columns.
        assert_eq!(cell_span(&snap, 0, 0), 2, "한 spans its spacer");
        assert_eq!(cell_span(&snap, 2, 0), 1, "a is single-width");
    }

    #[test]
    fn fallback_cluster_is_centered_in_its_span() {
        // R4.6: col 4, cw 10, 2-cell span, total advance 16 → offset 2, pen 42.
        let cw = 10.0;
        let offset = center_offset(1, 2, cw, 16.0);
        assert_eq!(offset, 2.0);
        assert_eq!(4.0 * cw + offset, 42.0);
        // R4.7: an oversized glyph overflows right only — never into col 3.
        assert_eq!(center_offset(1, 2, cw, 24.0), 0.0);
        assert_eq!(
            center_offset(1, 2, cw, 20.0),
            0.0,
            "exact fit is flush left"
        );
        // A single-cell fallback char centers in one cell.
        assert_eq!(center_offset(1, 1, cw, 6.0), 2.0);
    }

    #[test]
    fn primary_face_placement_is_untouched() {
        // R4.8/R0.3, a hard constraint: face 0 offsets to exactly 0.0 whatever its
        // advance is, so Latin placement is byte-identical to the pre-chain path.
        // Menlo's advance does not equal `cw` exactly, so "improving" this would
        // silently move every ASCII glyph.
        for adv in [0.0, 6.0, 11.7, 40.0] {
            for span in [1, 2] {
                assert_eq!(center_offset(0, span, 10.0, adv), 0.0);
            }
        }
    }

    #[test]
    fn ascii_placement_matches_single_font_shaping() {
        // R0.3: an ASCII-only snapshot resolves entirely to face 0, so run
        // splitting yields one whole-row run and every offset is 0 — the emitted
        // instances must match the pre-fallback path (one `FontRef`, one row-wide
        // byte→col map) field for field, not merely "look right".
        let (Some((device, queue)), Some(fonts)) = (headless(), fonts()) else {
            eprintln!("skipping: no GPU/font");
            return;
        };
        let face = fonts.faces().first().cloned().expect("a primary face");
        let mut atlas = Atlas::new(&device, &fonts, 32.0).expect("atlas");
        let mut t =
            term_core::Terminal::new(12, 2, term_core::Color(0xFFFFFF), term_core::Color(0));
        t.advance(b"hi != ok\r\nzsh %");
        let snap = t.snapshot();
        let (cw, ch) = (20.0f32, 40.0f32);
        let got = build_glyph_instances(&mut atlas, &queue, &fonts, true, (cw, ch), 32.0, &snap);
        assert!(!got.is_empty(), "an ASCII row must emit glyphs");

        // The pre-change path, reproduced verbatim.
        let font = FontRef::from_index(&face.data, face.index).expect("a valid primary face");
        let mut want: Vec<GlyphInstance> = Vec::new();
        let mut shape_ctx = ShapeContext::new();
        for row in 0..snap.rows {
            let mut line = String::new();
            let mut byte_col: Vec<u16> = Vec::new();
            for col in 0..snap.cols {
                let cell = snap.cell(col, row);
                if cell.map(Cell::is_continuation).unwrap_or(false) {
                    continue;
                }
                let before = line.len();
                line.push(cell.map(|c| c.ch).unwrap_or(' '));
                for _ in before..line.len() {
                    byte_col.push(col);
                }
            }
            let baseline = row as f32 * ch + ch * 0.8;
            let mut shaper = shape_ctx
                .builder(font)
                .size(32.0)
                .features([("liga", 1u16), ("calt", 1u16)].iter().copied())
                .build();
            shaper.add_str(&line);
            shaper.shape_with(|cluster| {
                let col = byte_col
                    .get(cluster.source.start as usize)
                    .copied()
                    .unwrap_or(0);
                let (color, bold, italic) = match snap.cell(col, row) {
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
                        face: 0,
                        id: g.id,
                        bold,
                        italic,
                    };
                    if let Some(entry) = atlas.get_or_insert_styled(&queue, key) {
                        want.push(GlyphInstance {
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

        assert_eq!(got.len(), want.len(), "same number of glyph quads");
        for (a, b) in got.iter().zip(&want) {
            assert_eq!(a.pos, b.pos, "ASCII glyph positions must not move (R0.3)");
            assert_eq!(a.size, b.size);
            assert_eq!((a.uv0, a.uv1), (b.uv0, b.uv1));
            assert_eq!(a.color, b.color);
        }
    }

    #[test]
    fn hangul_shapes_through_the_fallback_run() {
        // R0.2/R4.4/R4.5: the Hangul run is shaped with the fallback face's own
        // `FontRef`, so it emits a real glyph instead of the `.notdef` the atlas
        // now refuses — and it anchors to its own lead column, not the run's start.
        let (Some((device, queue)), Some(fonts)) = (headless(), fonts()) else {
            eprintln!("skipping: no GPU/font");
            return;
        };
        let mut atlas = Atlas::new(&device, &fonts, 32.0).expect("atlas");
        if atlas.is_degraded() {
            eprintln!("skipping: no fallback face on this machine");
            return;
        }
        let mut t = term_core::Terminal::new(6, 1, term_core::Color(0xFFFFFF), term_core::Color(0));
        t.advance("A한".as_bytes()); // cols: 0 = 'A', 1 = '한', 2 = its spacer
        let snap = t.snapshot();
        let cw = 20.0f32;
        let inst = build_glyph_instances(&mut atlas, &queue, &fonts, true, (cw, 40.0), 32.0, &snap);
        assert_eq!(inst.len(), 2, "'A' from face 0 and '한' from the fallback");
        assert!(inst[0].pos[0] < cw, "'A' stays anchored at column 0");
        let hangul_x = inst[1].pos[0];
        assert!(
            (cw..3.0 * cw).contains(&hangul_x),
            "'한' anchors to column 1 and stays inside its 2-cell span, got {hangul_x}"
        );
    }

    #[test]
    fn layer_eviction_forces_a_reshape() {
        // D17/R3.4: a DrawLayer that fills the atlas evicts it, invalidating the
        // UVs baked into the grid's resident `glyph_vbuf`. Since the snapshot is
        // unchanged, without this the stale UVs would be re-presented forever.
        let cfg = Config::default();
        let Ok(mut r) = Renderer::new_headless(256, 128, 4.0, &cfg) else {
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

        r.draw_to(&view, &snap);
        let settled = r.reshape_count();
        r.draw_to(&view, &snap);
        assert_eq!(
            r.reshape_count(),
            settled,
            "baseline: unchanged → cache hit"
        );

        /// Packs distinct glyphs until the atlas evicts — the tab-bar/overlay/
        /// preedit behavior that D17 exists for, reduced to its essential effect.
        struct Evictor(std::rc::Rc<std::cell::Cell<bool>>);
        impl DrawLayer for Evictor {
            fn z_order(&self) -> i32 {
                50
            }
            fn record(&mut self, ctx: &mut FrameCtx<'_>) {
                if self.0.get() {
                    return; // evict once; later frames must settle
                }
                let start = ctx.atlas.generation();
                for gid in 1u16..900 {
                    for (bold, italic) in
                        [(false, false), (true, false), (false, true), (true, true)]
                    {
                        let _ = ctx.atlas.get_or_insert_styled(
                            ctx.queue,
                            GlyphKey {
                                face: 0,
                                id: gid,
                                bold,
                                italic,
                            },
                        );
                    }
                    if ctx.atlas.generation() != start {
                        self.0.set(true);
                        return;
                    }
                }
            }
        }

        let evicted = std::rc::Rc::new(std::cell::Cell::new(false));
        r.add_layer(Box::new(Evictor(evicted.clone())));
        r.draw_to(&view, &snap); // the layer evicts during this frame
        if !evicted.get() {
            eprintln!("skipping: this font could not fill the atlas");
            return;
        }
        let after = r.reshape_count();
        r.draw_to(&view, &snap); // same snapshot — must re-shape anyway
        assert_eq!(
            r.reshape_count(),
            after + 1,
            "an eviction must force the next frame to re-shape (R3.4)"
        );
        r.draw_to(&view, &snap); // and then settle back to cache hits
        assert_eq!(
            r.reshape_count(),
            after + 1,
            "the forced re-shape must settle, not loop every frame"
        );
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
        let (Some((device, queue)), Some(fonts)) = (headless(), fonts()) else {
            eprintln!("skipping: no GPU/font");
            return;
        };
        let mut atlas = Atlas::new(&device, &fonts, 24.0).expect("atlas");
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

    /// R5.1/R5.4/D10: the layer text path — the single `tab_bar::layout_line`
    /// chokepoint that the tab bar, the ai-monitor overlay AND the preedit layer all
    /// funnel through — resolves Hangul through the *fallback* chain and emits real
    /// glyph quads, advancing two columns per syllable.
    ///
    /// The sibling tests assert this by inspecting shared code; this one observes the
    /// emitted instances, so the "all layer paths get fallback" property is
    /// regression-proof rather than inferred.
    #[test]
    fn layer_text_path_resolves_hangul_through_the_fallback() {
        let (Some((device, queue)), Some(fonts)) = (headless(), fonts()) else {
            eprintln!("skipping: no GPU/font");
            return;
        };
        if fonts.is_degraded() {
            eprintln!("skipping: no Hangul fallback face on this machine");
            return;
        }
        let mut atlas = Atlas::new(&device, &fonts, 24.0).expect("atlas");
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
        let t = term_core::Terminal::new(2, 1, term_core::Color(0xFFFFFF), term_core::Color(0));
        let snap = t.snapshot();

        // The face a Hangul syllable resolves to must not be the primary — that is
        // the whole point of the fallback chain (a tab title like `~/문서/프로젝트`
        // used to render as `.notdef` boxes).
        let key = atlas.resolve('한').expect("한 resolves to some face");
        assert_ne!(key.face, 0, "Hangul must come from a fallback face");
        assert_ne!(key.id, 0, "and must not be .notdef");

        let cw = 12.0;
        let mut ctx = FrameCtx {
            device: &device,
            queue: &queue,
            encoder: &mut encoder,
            view: &view,
            viewport: [64.0, 64.0],
            cell: [cw, 24.0],
            top_inset: 0.0,
            left_inset: 0.0,
            format: wgpu::TextureFormat::Rgba8Unorm,
            snapshot: &snap,
            atlas: &mut atlas,
        };
        let mut pens = Vec::new();
        crate::tab_bar::layout_line(&mut ctx, "한a", 0.0, cw, |pen, _entry| pens.push(pen));

        // Both chars emit a quad, and the Latin one starts two columns over — the
        // width-aware advance (R5.2) observed through the real layer path.
        assert_eq!(pens.len(), 2, "both glyphs emitted");
        assert_eq!(pens[0], 0.0);
        assert_eq!(pens[1], 2.0 * cw, "한 advances 2 columns, not 1");
    }

    #[test]
    fn atlas_eviction_is_bounded() {
        // R1.3: overflowing the atlas evicts (clear + repack) instead of growing
        // unboundedly or panicking; the generation bumps.
        let (Some((device, queue)), Some(fonts)) = (headless(), fonts()) else {
            eprintln!("skipping: no GPU/font");
            return;
        };
        let mut atlas = Atlas::new(&device, &fonts, 64.0).expect("atlas");
        // Many distinct glyphs at a large size force a shelf overflow. D13 took the
        // atlas 1024²→2048², i.e. 4× the area, so this needs 4× the glyph area the
        // 1024² version did: the same glyph-id range, now in all four synthesized
        // styles — each a distinct `GlyphKey`, hence its own packed bitmap.
        for gid in 1u16..900 {
            for (bold, italic) in [(false, false), (true, false), (false, true), (true, true)] {
                let key = GlyphKey {
                    face: 0,
                    id: gid,
                    bold,
                    italic,
                };
                let _ = atlas.get_or_insert_styled(&queue, key); // must never panic
            }
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
