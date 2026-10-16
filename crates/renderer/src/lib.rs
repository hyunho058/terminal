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
pub mod box_draw;
pub mod cursor_layer;
pub mod font_set;
pub mod glyph;
pub mod image_layer;
pub mod link_layer;
pub mod monitor_layout;
pub mod monitor_model;
pub mod monitor_panel;
pub mod monitor_scroll;
pub mod preedit_layer;
pub mod selection_layer;
pub mod sidebar_card;
pub mod sidebar_header;
pub mod sidebar_icons;
pub mod sidebar_layout;
pub mod sidebar_row;
pub mod sidebar_shapes;
pub mod sidebar_text;
pub mod sidebar_theme;
pub mod tab_bar;
pub mod testing;

use atlas::{Atlas, GlyphKey};
use bytemuck::{Pod, Zeroable};
use common::config::{
    AiMonitorConfig, Config, SidebarConfig, FONT_SIZE_DEFAULT, FONT_SIZE_MAX, FONT_SIZE_MIN,
    MONITOR_WIDTH_MAX, MONITOR_WIDTH_MIN, SIDEBAR_WIDTH_MAX, SIDEBAR_WIDTH_MIN,
};
use font_set::FONT_CANDIDATES;
use swash::shape::ShapeContext;
use swash::FontRef;
use term_core::{Cell, CellAttrs, GridSnapshot};
use thiserror::Error;
use wgpu::util::DeviceExt;

pub use font_set::{FontData, FontFace, FontFamily, FontSet, StyleResolution, StyleSlot};

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
    /// A GPU readback in [`Renderer::capture`] failed. Carries a `String` rather
    /// than a `&'static str` because every cause here — a device poll timeout, a
    /// failed buffer map, an unavailable mapped range — reports its own detail,
    /// and folding that into a fixed message would leave the caller with nothing
    /// to act on.
    #[error("could not read back the rendered frame: {0}")]
    Capture(String),
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
    /// Physical px reserved right of the grid (the docked ai-monitor panel paints
    /// this right band). Nothing shifts by it — the grid still starts at
    /// `left_inset` and merely ends sooner — so unlike `left_inset` it reaches no
    /// uniform (D21; see [`grid_dims`]). Always equal to [`Renderer::right_inset`]
    /// (R4.7): a layer that painted a wider band than `grid_size` reserved would
    /// cover columns the terminal still believes it owns.
    pub right_inset: f32,
    /// Render-target format — a layer needs it to build its own pipelines.
    pub format: wgpu::TextureFormat,
    /// The live sidebar config (D10). A sidebar layer reads its toggles and width
    /// from here; it is the pre-override config, so `left_inset` (not
    /// `sidebar.width_px`) is the authoritative drawn band width.
    pub sidebar: &'a SidebarConfig,
    /// Window backing scale factor — multiply logical-px sidebar metrics by this to
    /// get physical px (D1/D10, `SidebarMetrics::scaled`).
    pub scale_factor: f64,
    pub snapshot: &'a GridSnapshot,
    pub atlas: &'a mut Atlas,
    /// The font fallback chain `atlas` was built from. A layer that rasterizes text at
    /// a size of its own — the sidebar's card atlases (sidebar-weather-cards D2) —
    /// builds from the same chain, so it gets the same fonts and real bold faces.
    pub fonts: &'a FontSet,
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
/// Runtime sidebar overrides that outrank the config for the rest of the session (D4):
/// a hide/restore button or a drag-resize writes here, and `apply_config` leaves it
/// untouched — so saving `config.toml` for an unrelated reason cannot undo a click the
/// user just made. `None` fields defer to the config.
#[derive(Default)]
struct SidebarOverride {
    collapsed: Option<bool>,
    width_px: Option<f32>,
}

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
    /// Live sidebar config (D10). Refreshed by `apply_config`; handed to each layer
    /// via `FrameCtx.sidebar`. Foundations stores it but registers no layer.
    sidebar: SidebarConfig,
    cell_size: (f32, f32),
    /// Physical px reserved above the grid (e.g. a tab bar). Grid + glyphs are
    /// shifted down by this, and `grid_size` reports the rows that fit below it.
    top_inset: f32,
    /// Physical px reserved left of the grid (e.g. a vertical tab sidebar). The
    /// grid and glyphs are shifted right by this, and `grid_size` reports the
    /// columns that fit beside it. Orthogonal to `top_inset` (rows vs cols). The
    /// renderer owns this now (D4): it is derived from the sidebar config + override by
    /// `sync_left_inset`, and `set_left_inset`'s argument is ignored.
    left_inset: f32,
    /// Physical px reserved right of the grid (the docked ai-monitor panel). Costs
    /// the grid columns exactly as `left_inset` does, but shifts nothing — see
    /// `grid_dims` for why that difference keeps it out of the uniforms. Derived by
    /// `sync_right_inset`, never assigned directly (D21).
    right_inset: f32,
    /// Runtime sidebar overrides (D4), consulted before the config by `effective_sidebar`.
    sidebar_override: SidebarOverride,
    /// Live ai-monitor config (D6/D21). Refreshed by `apply_config`; the pre-override half
    /// of `effective_monitor_width`.
    ai_monitor: AiMonitorConfig,
    /// Is the panel open? A **runtime** fact, not a config-derived one (D21).
    ///
    /// `[ai_monitor].open` is deliberately not read here: the band this reserves is painted
    /// by `AiMonitorOverlay`'s layer, whose visibility the app owns, so a renderer that
    /// adopted the file's flag would reserve an empty strip the instant the two disagreed —
    /// at startup, or on any live reload. The file's flag records the user's intent for a
    /// restore that belongs to the overlay; the inset follows what is actually painted.
    monitor_open: bool,
    /// The panel's runtime width in **logical** px (D6/D21) — a released drag writes here.
    /// `None` defers to `[ai_monitor].width_px`, exactly as `SidebarOverride::width_px`
    /// defers to `[sidebar].width_px`.
    monitor_width_override: Option<f32>,
    /// Baseline terminal font size in logical points from `Config` (terminal-font-size D9).
    font_size_pt: f32,
    /// Runtime font size override in logical points (terminal-font-size D9). `None` defers
    /// to `font_size_pt`.
    font_size_override: Option<f32>,
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

        let mut renderer = Renderer {
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
            sidebar: cfg.sidebar.clone(),
            cell_size,
            top_inset: 0.0,
            left_inset: 0.0,
            right_inset: 0.0,
            sidebar_override: SidebarOverride::default(),
            ai_monitor: cfg.ai_monitor.clone(),
            monitor_open: false,
            monitor_width_override: None,
            font_size_pt: cfg.font.size_pt,
            font_size_override: None,
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
        };
        // The startup insets must be right before the first frame (D4/D21).
        renderer.sync_left_inset();
        renderer.sync_right_inset();
        Ok(renderer)
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
            self.right_inset,
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
    pub fn set_left_inset(&mut self, _requested_px: f32) {
        // D4: the sidebar owns its width now — the argument is ignored, retained only so
        // the two out-of-surface call sites (`terminal_app.rs`, `settings.rs`) keep
        // compiling. `set_sidebar_width` / `set_sidebar_collapsed` are the real setters.
        self.sync_left_inset();
    }

    /// The sidebar config with the runtime override's `Some` fields applied (D4). The
    /// override outranks the file for the session, so a click survives a `config.toml`
    /// save.
    fn effective_sidebar(&self) -> SidebarConfig {
        let mut s = self.sidebar.clone();
        if let Some(c) = self.sidebar_override.collapsed {
            s.collapsed = c;
        }
        if let Some(w) = self.sidebar_override.width_px {
            s.width_px = w;
        }
        s
    }

    /// Re-derive `left_inset` from the effective sidebar (config + override) and push the
    /// uniforms (D4). The single site that writes the inset; called from `assemble`,
    /// `apply_config`, and the three runtime setters.
    fn sync_left_inset(&mut self) {
        self.left_inset =
            sidebar_row::sidebar_width_px(&self.effective_sidebar(), self.scale_factor);
        self.write_uniforms();
        // The sidebar's band is one of the four inputs to D4's cap on the panel's band, so
        // moving it re-derives the panel's too (R4.3). One tail call rather than four
        // copies at the sidebar setters: the coupling is real and one-directional
        // (`sync_right_inset` reads `left_inset`, never the reverse), so stating it once
        // here is what keeps a future fifth sidebar setter from forgetting it.
        self.sync_right_inset();
    }

    /// Collapse or restore the sidebar at runtime (D4). Returns the reclaimed width to
    /// the grid; a subsequent `apply_config` will not undo it.
    pub fn set_sidebar_collapsed(&mut self, collapsed: bool) {
        self.sidebar_override.collapsed = Some(collapsed);
        self.sync_left_inset();
    }

    /// Set the sidebar's logical width at runtime (D4), clamped to foundations' 260–440
    /// band (R5.7) — the public backing for a drag-resize, which does not go through
    /// `Config::load_from`'s clamp.
    pub fn set_sidebar_width(&mut self, logical_px: f32) {
        self.sidebar_override.width_px =
            Some(logical_px.clamp(SIDEBAR_WIDTH_MIN, SIDEBAR_WIDTH_MAX));
        self.sync_left_inset();
    }

    /// The effective sidebar width in **logical** px — config with the runtime override
    /// applied (D4). Unlike [`left_inset`](Renderer::left_inset) this is non-zero while
    /// collapsed: it is the width a restore brings back, and the value tab-resize persists
    /// (D7), so collapsing must not erase it.
    pub fn sidebar_width_logical(&self) -> f32 {
        self.effective_sidebar().width_px
    }

    /// The effective collapsed state — config with the runtime override applied (D4).
    pub fn sidebar_collapsed(&self) -> bool {
        self.effective_sidebar().collapsed
    }

    /// Drop the runtime sidebar override so the config file is authoritative again
    /// (tab-resize D15a).
    ///
    /// The override is otherwise sticky for the session, which would make `width_px` and
    /// `collapsed` the only fields in `config.toml` that stop live-reloading once the user
    /// has dragged. Called when a delivered config carries a value that is *not* our own
    /// echo — i.e. a real human edit, which should win.
    pub fn clear_sidebar_override(&mut self) {
        self.sidebar_override = SidebarOverride::default();
        self.sync_left_inset();
    }

    /// Current left inset in physical px.
    pub fn left_inset(&self) -> f32 {
        self.left_inset
    }

    /// Current right inset in physical px — the band the docked ai-monitor panel
    /// occupies (D21). `0.0` while the panel is closed, which is the pre-panel
    /// behavior exactly (R4.2). [`FrameCtx::right_inset`] carries the same value to
    /// the layers (R4.7).
    pub fn right_inset(&self) -> f32 {
        self.right_inset
    }

    /// The width the panel is **asking** for, in **logical** px — config with the runtime
    /// override applied, or `0.0` while it is closed (D6/D21). The twin of
    /// [`effective_sidebar`](Renderer::effective_sidebar).
    ///
    /// This is intent, not the band that gets drawn. D4's clamps live one level down, in
    /// [`monitor_band_px`], because D8 turns on exactly that split: the persisted value is
    /// what the user dragged to, and a window too narrow to honour it applies a *transient*
    /// squeeze on top — so re-widening restores the intended width with no user action
    /// (R4.5), and nothing ever writes the squeezed number back (R6.2).
    fn effective_monitor_width(&self) -> f32 {
        if !self.monitor_open {
            return 0.0;
        }
        self.monitor_width_override
            .unwrap_or(self.ai_monitor.width_px)
            .max(0.0)
    }

    /// Re-derive `right_inset` from the effective panel width (config + override + D4's
    /// clamps) and invalidate the instance cache (D21). The single site that writes the
    /// inset; called from `assemble`, `apply_config`, `resize`, `sync_left_inset` and the
    /// three runtime setters.
    ///
    /// `sync_left_inset` and `resize` are in that list because the clamp reads the window
    /// width, the cell width and the sidebar's band: a value derived from four inputs but
    /// recomputed on only one of them is a stale value waiting to happen — the sidebar
    /// widening past the point where the panel still fits would otherwise leave the band
    /// overlapping columns `grid_size` still believes the terminal owns.
    ///
    /// Writes **no uniforms**, unlike [`sync_left_inset`](Renderer::sync_left_inset).
    /// `BgUniforms`/`GlyphUniforms` carry `left_inset` because the grid is *shifted*
    /// right by it; a right band shifts nothing and only removes columns, which
    /// `grid_dims` already accounts for. Handing it to the shader as well would make
    /// the GPU a second author of a fact `grid_size` owns, and the two would disagree
    /// the first time one of them is updated alone.
    fn sync_right_inset(&mut self) {
        let scale = self.scale_factor as f32;
        let px = monitor_band_px(
            self.effective_monitor_width() * scale,
            self.config.width as f32,
            self.cell_size.0,
            self.left_inset,
            MONITOR_WIDTH_MIN * scale,
        );
        if px == self.right_inset {
            // A drag calls this on every `CursorMoved`, so an unchanged width must not
            // cost a full re-shape per pointer move.
            return;
        }
        self.right_inset = px;
        // The column count just moved, so the resident instance buffers were built for
        // a grid that no longer exists — a content-equal snapshot must not re-present
        // them (R1.4, the same reason `resize` clears the cache).
        self.last_snapshot = None;
    }

    /// Open or close the ai-monitor panel's band (D21/R0.1/R0.3). Two callers, both in the
    /// app's `terminal_app`: the `Cmd+Shift+M` chord by way of `AppRequest::SetMonitorOpen`
    /// — the one gesture D8 lets *persist* the open state — and the launch restore that
    /// replays the persisted flag with no keystroke (R6.6).
    ///
    /// Neither passes a width, and that is why this takes none: the band is derived from
    /// `[ai_monitor].width_px` plus the runtime override, so a width dragged last week comes
    /// back on the next open (R4.5).
    ///
    /// Separate from [`set_monitor_width`](Renderer::set_monitor_width) on purpose: a
    /// force-close (R4.4) must not look like a width of zero, or the next re-widening
    /// would have nothing to restore.
    pub fn set_monitor_open(&mut self, open: bool) {
        self.monitor_open = open;
        self.sync_right_inset();
    }

    /// Whether the panel is open — the **intent**, which a width-forced close leaves alone
    /// (D8/R6.3). `right_inset() == 0.0` while this is `true` is exactly R4.4's state.
    pub fn monitor_open(&self) -> bool {
        self.monitor_open
    }

    /// Set the panel's logical width at runtime (D6/D21), clamped to the design's
    /// 300–760 drag band — the public backing for the edge drag, which does not go through
    /// `Config::load_from`'s clamp.
    ///
    /// Unlike [`set_left_inset`](Renderer::set_left_inset) this is a real setter, but it
    /// still routes through `sync_right_inset` rather than assigning the inset, so the next
    /// `apply_config` re-derives the same band instead of silently dropping it while the
    /// panel keeps painting (R4.6).
    pub fn set_monitor_width(&mut self, logical_px: f32) {
        self.monitor_width_override = Some(clamp_monitor_width(logical_px));
        self.sync_right_inset();
    }

    /// The panel's effective width in **logical** px — config with the runtime override
    /// applied (D6). This is the value a drag release persists: the *intent*, readable even
    /// while a narrow window is squeezing the drawn band (D8/R6.2), and non-zero even while
    /// the panel is closed, exactly as `sidebar_width_logical` is.
    pub fn monitor_width_logical(&self) -> f32 {
        self.monitor_width_override
            .unwrap_or(self.ai_monitor.width_px)
    }

    /// Drop the runtime panel-width override so the config file is authoritative again
    /// (D6, mirroring [`clear_sidebar_override`](Renderer::clear_sidebar_override)).
    ///
    /// Called when a delivered config carries a `width_px` that is not our own echo — i.e.
    /// a real human edit, which should win (R6.5). The open state is **not** cleared with
    /// it. `[ai_monitor].open` is read from the file exactly once, at launch, and the app
    /// replays it by *opening the overlay and the band together* (R6.6); adopting the file's
    /// flag again mid-session would reserve a band with no overlay painting into it — an
    /// empty strip appearing on an unrelated config edit.
    pub fn clear_monitor_override(&mut self) {
        self.monitor_width_override = None;
        self.sync_right_inset();
    }

    /// Effective terminal font size in logical points — runtime override if set, else the
    /// baseline `font_size_pt` from `Config` (terminal-font-size D9).
    fn effective_font_size(&self) -> f32 {
        self.font_size_override.unwrap_or(self.font_size_pt)
    }

    /// The terminal's effective font size in **logical** points — config baseline with the
    /// runtime override applied (terminal-font-size D9, R2.1).
    pub fn font_size_logical(&self) -> f32 {
        self.effective_font_size()
    }

    /// Drop the runtime font-size override so the config baseline is authoritative again
    /// (terminal-font-size D9, R2.4).
    pub fn clear_font_override(&mut self) {
        self.font_size_override = None;
    }

    /// Set the terminal font size in logical points at runtime (terminal-font-size D1, D4,
    /// D6, D9, D10; R2.1, R2.2, R2.5).
    ///
    /// Clamps `size_pt` to `[FONT_SIZE_MIN, FONT_SIZE_MAX]` (falling back to
    /// `FONT_SIZE_DEFAULT` if non-finite). Returns `false` without rebuilding `Atlas` or
    /// clearing `last_snapshot` when the clamped size equals `effective_font_size()` (D10 /
    /// R2.2). Otherwise updates `font_size_override`, `cell_size`, and `ppem`, rebuilds the
    /// glyph `Atlas` and `glyph_bind_group`, invalidates `last_snapshot`, re-syncs both side
    /// insets (`sync_left_inset` → `sync_right_inset`), and returns `true`.
    pub fn set_font_size(&mut self, size_pt: f32) -> bool {
        let clamped = clamp_font_size(size_pt);
        if (clamped - self.effective_font_size()).abs() == 0.0 {
            return false;
        }
        self.font_size_override = Some(clamped);
        self.cell_size = cell_size_for_pt(clamped, self.scale_factor);
        self.ppem = clamped * self.scale_factor as f32;
        self.swap_atlas();
        self.last_snapshot = None;
        self.sync_left_inset();
        true
    }

    /// Rebuild the glyph atlas at `self.ppem` and re-point the grid's `glyph_bind_group`
    /// at it — the **only** atlas-swap site. `apply_config` and `set_font_size` both
    /// come through here.
    ///
    /// Rebuilding shares the same mapped bytes (D7/R3.2): a live ppem change must not
    /// re-read (or re-copy) the 55 MB fallback face on every tick of the config watcher.
    ///
    /// The `None` arm — allocation failed, the old atlas is retained — is a *defined*
    /// outcome for bind groups, not an unhandled hole: `epoch` does not change, so the
    /// three text layers correctly keep their cached bind groups. That is right rather
    /// than lucky, because the old texture is still alive and its view still valid
    /// (D8/R4.3). What that arm leaves unaddressed: both callers update `cell_size` and
    /// `ppem` *before* this fallible call, so a failure leaves the atlas rasterized at the
    /// OLD ppem while the caller's `last_snapshot = None` forces a re-shape at the NEW
    /// cell geometry — glyphs drawn at the wrong size. A pre-existing defect, recorded as
    /// Known Gap G-d in specs/atlas-swap-bind-invalidation/spec.md.
    fn swap_atlas(&mut self) {
        let Some(atlas) = Atlas::new(&self.device, &self.fonts, self.ppem) else {
            tracing::warn!(ppem = self.ppem, "failed to rebuild the glyph atlas");
            return;
        };
        self.atlas = atlas;
        // The three text layers (sidebar/overlay/preedit) self-guard against this swap:
        // each caches its atlas bind group keyed on `(format, atlas.epoch())` and rebuilds
        // when the texture identity changes. The grid does **not** — it is correct only
        // because this is the single swap site and rebuilds `glyph_bind_group` by hand
        // right here (D7; Known Gap G-a in specs/atlas-swap-bind-invalidation/spec.md).
        // A new caller must therefore route through this function, not copy it, or give
        // the grid the same epoch-keyed guard the layers have. Otherwise the grid samples
        // the replaced texture with UVs computed against the new one: no crash, no
        // validation error, just wrong glyphs for the rest of the session.
        // `golden_frame.rs`'s `a_zoomed_frame_matches_one_rendered_at_that_size_from_the_start`
        // is the pixel check that fails if this line goes missing.
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

    /// The window backing scale factor. A sidebar `DrawLayer` multiplies logical-px
    /// [`SidebarConfig::width_px`] / `SidebarMetrics` by this to get physical px
    /// (D1/D10); `FrameCtx.scale_factor` carries the same value to layers.
    pub fn scale_factor(&self) -> f64 {
        self.scale_factor
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
        self.font_size_pt = cfg.font.size_pt;
        let eff_pt = self.effective_font_size();
        self.cell_size = cell_size_for_pt(eff_pt, self.scale_factor);
        self.ppem = eff_pt * self.scale_factor as f32;
        self.ligatures = cfg.font.ligatures;
        self.bg_default = cfg.theme.background;
        self.clear = to_wgpu_color(cfg.theme.background);
        // Sidebar toggles/width/accent reach layers via `FrameCtx.sidebar` (D10).
        self.sidebar = cfg.sidebar.clone();
        // The panel's persisted width is the pre-override half of `effective_monitor_width`
        // (D6), so a live reload has to refresh it before `sync_right_inset` runs below.
        self.ai_monitor = cfg.ai_monitor.clone();

        // Rebuild the atlas at the new ppem so glyphs re-rasterize (size + ligature
        // changes need fresh coverage) and re-point the glyph bind group at it.
        self.swap_atlas();
        // Theme/font/atlas/cell-metrics changed → force the next frame to re-shape,
        // else a content-equal snapshot would re-present stale-theme buffers (R1.4).
        self.last_snapshot = None;
        // Re-fit the grid to the (possibly changed) sidebar width, WITHOUT clearing the
        // runtime override — a saved config must not undo a runtime collapse (D4). This
        // also calls write_uniforms.
        self.sync_left_inset();
        // Same rule on the right edge (D21): a live reload for an unrelated key must
        // re-derive the panel's band, not drop it while the panel keeps painting (R4.6).
        self.sync_right_inset();
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
        // The window width is D4's other cap input: narrowing squeezes (and eventually
        // force-closes) the panel's band, and widening restores it at the intended width
        // with no keystroke (R4.4/R4.5). Both directions are *this* call — the caller
        // re-fits the grid immediately after, so a stale inset here would resize every tab
        // to a column count the panel then paints over.
        self.sync_right_inset();
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
                right_inset: self.right_inset,
                format: self.config.format,
                sidebar: &self.sidebar,
                scale_factor: self.scale_factor,
                snapshot,
                atlas: &mut self.atlas,
                fonts: &self.fonts,
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

    /// Draw `snapshot` into a fresh offscreen target and read the pixels back.
    ///
    /// This is the only way to observe what a frame **actually contains**. Every
    /// other observable the renderer exposes reports a *decision* — `reshape_count`
    /// says the instance builders ran, `Atlas::generation` says a glyph was packed —
    /// and each of those stays perfectly correct while the frame it describes comes
    /// out blank. A regression that zeroes `glyph_count`, mis-derives the insets, or
    /// swaps a pipeline is invisible to all of them and visible here.
    ///
    /// Costs a texture, a buffer and a device stall per call, so it belongs in tests
    /// and benches, not on the frame path — `draw_to` is that path, and this wraps it
    /// rather than reimplementing it, so the two can never drift.
    ///
    /// Returns [`RendererError::Capture`] rather than panicking. Its callers are all
    /// tests today, which makes an `unwrap` tempting, but this is `pub` library code
    /// on a path with real runtime failures — a lost device, a rejected map — and the
    /// crate's rule is a `thiserror` enum, with `unwrap` confined to tests. A test
    /// that wants a panic writes `.expect(..)` at the call site and gets the cause in
    /// the message; the reverse cannot be recovered.
    pub fn capture(&mut self, snapshot: &GridSnapshot) -> Result<CapturedFrame> {
        // Refused here, before a texture, a buffer and a device stall are spent on
        // a frame that could not be decoded anyway.
        let Some(bgra) = capture_channel_order(self.config.format) else {
            return Err(RendererError::Capture(format!(
                "target format {:?} is not an 8-bit, four-channel layout, so the \
                 readback cannot be decoded. Widening this means teaching \
                 `capture_channel_order` and the normalisation loop the new \
                 layout, not relaxing the check.",
                self.config.format
            )));
        };
        let (width, height) = (self.config.width, self.config.height);
        // Also refused before any GPU work. `new_headless` cannot produce a
        // zero-sized config, but `capture` is `pub` and reads `self.config`, which
        // on the windowed path follows the surface: a minimised or zero-sized
        // window configures 0×0, and `create_texture` answers that with a wgpu
        // validation *panic*, not an error this method could report. The same
        // argument that made this return `Result` covers it.
        if width == 0 || height == 0 {
            return Err(RendererError::Capture(format!(
                "the target is {width}x{height}; there is no frame to read back. \
                 The surface is configured to a zero-sized window, so this is a \
                 caller's timing question rather than a GPU failure."
            )));
        }
        let target = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("capture-target"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.config.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        self.draw_to(&view, snapshot);

        // `copy_texture_to_buffer` requires a 256-byte-aligned row pitch, so the
        // readback rows are padded and the padding is stripped below.
        let unpadded = width * 4;
        let padded = unpadded.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
            * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("capture-readback"),
            size: padded as u64 * height as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("capture-copy"),
            });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &target,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        // Submitted after `draw_to`'s encoder on the same queue, so the copy sees
        // the finished frame.
        self.queue.submit(std::iter::once(encoder.finish()));

        // Three results, all of which have to be read. `poll` returning `Ok` says
        // the queue drained, not that the mapping succeeded, and the map's own
        // outcome is reported *only* through the callback — so discarding either
        // (the `|_| {}` and `let _ =` this replaces) turned a device loss into an
        // unexplained panic on the `get_mapped_range` line below.
        let slice = readback.slice(..);
        let (map_tx, map_rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |res| {
            let _ = map_tx.send(res);
        });
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| RendererError::Capture(format!("device poll failed: {e}")))?;
        map_rx
            .recv()
            .map_err(|_| RendererError::Capture("map callback never ran (device lost?)".into()))?
            .map_err(|e| RendererError::Capture(format!("buffer map failed: {e}")))?;
        let mapped = slice
            .get_mapped_range()
            .map_err(|e| RendererError::Capture(format!("mapped range unavailable: {e}")))?;

        // Normalise to RGBA so callers never branch on the target's channel order.
        //
        // Every product here widens to `usize` before multiplying. In `u32` a
        // 4K-by-4K target already puts `row * padded` within a factor of four of
        // wrapping, and the failure is silent in release: the offset wraps to a
        // small number and the frame is assembled from the wrong rows.
        let mut pixels = Vec::with_capacity(unpadded as usize * height as usize);
        for row in 0..height as usize {
            let start = row * padded as usize;
            let bytes = &mapped[start..start + unpadded as usize];
            for px in bytes.as_chunks::<4>().0 {
                if bgra {
                    pixels.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
                } else {
                    pixels.extend_from_slice(px);
                }
            }
        }
        drop(mapped);
        readback.unmap();

        Ok(CapturedFrame {
            width,
            height,
            pixels,
        })
    }
}

/// Whether [`Renderer::capture`] can decode `format`, and if so whether its
/// channels arrive as BGRA rather than RGBA.
///
/// `Some(true)` = BGRA, `Some(false)` = RGBA, `None` = refuse.
///
/// The readback is not format-agnostic and never was. Its row pitch is
/// `width * 4` and its normalisation walks `as_chunks::<4>()`, so every step
/// assumes four bytes per pixel with one byte per channel. What used to stand in
/// for this check was a bare `matches!` on the two BGRA variants, whose `false`
/// arm meant "RGBA" rather than "unknown" — so any other format was decoded as
/// RGBA and returned as `Ok`.
///
/// That is reachable. `new_headless` pins `Bgra8UnormSrgb`, but `capture` is
/// `pub` and reads `self.config.format`, which on the windowed path comes from
/// `surface.get_default_config`, and on macOS that can be `Rgba16Float` for an
/// EDR display or `Rgb10a2Unorm`. At eight bytes per pixel every `pixel(x, y)`
/// would be half of one channel pair: `modal_pixel`, `is_ink` and every golden
/// assertion would go on measuring noise, with nothing to act on. A method whose
/// whole purpose is to report what a frame contains must not answer confidently
/// when it cannot read it.
fn capture_channel_order(format: wgpu::TextureFormat) -> Option<bool> {
    match format {
        wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb => Some(true),
        wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Rgba8UnormSrgb => Some(false),
        _ => None,
    }
}

/// One rendered frame read back from the GPU, in tightly packed RGBA8.
///
/// Produced by [`Renderer::capture`]. Row-major, `width * height * 4` bytes, with
/// the readback row padding already stripped and the channel order normalised, so
/// a caller indexes it without knowing anything about the target's format.
#[derive(Clone, PartialEq, Eq)]
pub struct CapturedFrame {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl CapturedFrame {
    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// The raw RGBA8 bytes, row-major and tightly packed.
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// RGBA at `(x, y)`. Panics out of bounds — a test that reads outside the
    /// frame has a bug in its geometry, which is exactly what these frames exist
    /// to catch, so it must not read as a silent black pixel.
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        assert!(
            x < self.width && y < self.height,
            "pixel({x}, {y}) is outside the {}x{} frame",
            self.width,
            self.height
        );
        // Widened before the multiply: `(y * self.width + x) * 4` in `u32`
        // overflows on a large frame, which debug-panics inside the accessor and
        // in release wraps into a wrong — but in-bounds — pixel.
        let i = (y as usize * self.width as usize + x as usize) * 4;
        [
            self.pixels[i],
            self.pixels[i + 1],
            self.pixels[i + 2],
            self.pixels[i + 3],
        ]
    }

    /// The most common pixel value in the frame — the painted background, derived
    /// from the frame itself rather than from the config.
    ///
    /// Deriving it here is deliberate: the clear color goes through
    /// `to_wgpu_color` and an sRGB target before it becomes a byte, so a test that
    /// compared against `cfg.theme.background` would be asserting that conversion
    /// chain, not the thing it came to check. Every terminal frame is
    /// overwhelmingly background, so the mode is that background by construction.
    pub fn modal_pixel(&self) -> [u8; 4] {
        let mut counts: std::collections::HashMap<[u8; 4], u32> = std::collections::HashMap::new();
        for px in self.pixels.as_chunks::<4>().0 {
            *counts.entry(*px).or_default() += 1;
        }
        counts
            .into_iter()
            .max_by_key(|&(_, n)| n)
            .map(|(px, _)| px)
            .unwrap_or([0, 0, 0, 0])
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

/// The center UV of an atlas entry — the sample point for a solid-fill quad.
///
/// Sampling the *center* of the 2×2 block matters: at an edge texel, linear
/// filtering would blend in a neighbouring glyph's coverage and the fill would come
/// out patchy along its border. Used by the underline rule and every procedural
/// box-drawing rect, which are the two paths that draw a flat color rather than a
/// glyph shape.
fn center_uv(entry: &atlas::GlyphEntry) -> [f32; 2] {
    [
        (entry.uv0[0] + entry.uv1[0]) * 0.5,
        (entry.uv0[1] + entry.uv1[1]) * 0.5,
    ]
}

/// Fraction a DIM cell's foreground is mixed toward its background.
const DIM_MIX: f32 = 0.45;

/// The linear-light glyph color for a cell, applying REVERSE (fg/bg swap) and DIM
/// (mix toward the background) — the color both its glyphs and its underline use.
pub(crate) fn glyph_color(cell: &Cell) -> [f32; 3] {
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
/// Everything about a character that decides **which font file face shapes it**,
/// plus the synthesis owed afterwards (D4/D15).
///
/// The face alone is not enough any more. Two cells can resolve to the same chain
/// entry and still need different shapers — a bold cell shapes from Menlo Bold
/// while its neighbour shapes from Menlo Regular, and their glyph ids are
/// different namespaces. Splitting on the entry alone would shape both with one
/// face and hand the other's ids to the atlas.
///
/// `synth_*` ride along even though they do not affect shaping: keeping them in
/// the key makes each run uniform in style, so the [`GlyphKey`] is constant for
/// the whole run instead of being re-derived from each cluster's lead cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RunStyle {
    /// Chain entry — index into `FontSet::families()`.
    face: usize,
    /// Which face *within* that entry shapes and owns the resulting glyph ids.
    slot: StyleSlot,
    synth_bold: bool,
    synth_italic: bool,
}

/// A maximal span of chars that shape together.
///
/// Bounds index the row's **continuation-skipped char sequence** (the order
/// [`row_chars`] produces), never raw column numbers: a wide char occupies two
/// columns but one char, so column-based bounds would desync every run boundary
/// after the first wide char (H-R4.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Run {
    /// The resolved style every char in the run shares. `None` = no face covers
    /// them, so the run draws nothing at all (H-D5/H-R4.2).
    style: Option<RunStyle>,
    start_char: usize,
    /// Exclusive.
    end_char: usize,
}

/// Group a row's per-char resolutions into maximal same-`RunStyle` runs
/// (H-D16/H-R4.1, narrowed by D4/D15).
///
/// The Confirmed Goal's "pick a font per cluster" is circular — a cluster is an
/// *output* of shaping, so the shaping font cannot be chosen from it. The real
/// order is: resolve each **char** to a face *and slot*, group consecutive equal
/// resolutions, then shape each group with that slot's `FontRef`. Pure, so the
/// grouping rule is table-testable with no GPU, font, or snapshot.
///
/// A run breaks wherever the *resolved* style changes — which is not the same as
/// wherever the *requested* style changes. Italic text containing a character the
/// italic face lacks splits at that character, because it alone shapes from the
/// regular face (D7/R4.1).
fn split_runs(styles: &[Option<RunStyle>]) -> Vec<Run> {
    let mut runs: Vec<Run> = Vec::new();
    for (i, &style) in styles.iter().enumerate() {
        match runs.last_mut() {
            Some(run) if run.style == style => run.end_char = i + 1,
            _ => runs.push(Run {
                style,
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

/// The `(bold, italic)` a cell asks for — what the *cell* wants, before any
/// question of whether a real face can supply it (D7/D9).
///
/// Read once per char during the resolve pass rather than per cluster after
/// shaping, because it selects the face the shaper is built from (D4).
fn cell_style(snapshot: &GridSnapshot, col: u16, row: u16) -> (bool, bool) {
    snapshot.cell(col, row).map_or((false, false), |c| {
        (
            c.attrs.contains(CellAttrs::BOLD),
            c.attrs.contains(CellAttrs::ITALIC),
        )
    })
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
    // Box-drawing cells collected during the row walk and drawn after it (D2/D8).
    // Deferred for the same reason the underline pass is: the shaping closure below
    // holds a mutable borrow of the atlas, and these need one too.
    let mut procedural: Vec<(box_draw::Glyph, u16, u16)> = Vec::new();
    for row in 0..snapshot.rows {
        let RowChars { chars, char_col } = row_chars(snapshot, row);
        // Resolve every char to a face *and slot* first (H-D2/H-D16, D4): `resolve`
        // borrows the atlas immutably while the shaping closure below borrows it
        // mutably, and the resolved face is an *input* to shaping, so it has to be
        // decided up front.
        //
        // The style comes from the cell here, not from the cluster later. It has to:
        // it selects the face the shaper is built from, and a shaper cannot be
        // chosen from its own output (D4/D15).
        let mut styles: Vec<Option<RunStyle>> = chars
            .iter()
            .enumerate()
            .map(|(i, &c)| {
                let (bold, italic) = cell_style(snapshot, char_col[i], row);
                atlas.resolve(c, bold, italic).map(|key| RunStyle {
                    face: key.face as usize,
                    slot: key.slot,
                    synth_bold: key.synth_bold,
                    synth_italic: key.synth_italic,
                })
            })
            .collect();
        // H-D8: take box-drawing characters out of the shaping input by marking
        // their style `None` — `split_runs` then yields a run the loop below already
        // skips. Marking rather than *removing* is what keeps `chars`, `char_col`
        // and the run-relative byte map in sync; deleting an element would desync
        // every column after it (the hazard documented at the byte_col build below).
        //
        // Splitting a face-0 run here moves nothing: `pen` is reset per cluster to
        // `col * cw + offset`, and `center_offset(0, ..)` is always 0.0.
        for (i, &c) in chars.iter().enumerate() {
            if let Some(glyph) = box_draw::classify(c) {
                styles[i] = None;
                procedural.push((glyph, char_col[i], row));
            }
        }
        let baseline = row as f32 * ch + ascent;

        for run in split_runs(&styles) {
            // H-D5/H-R4.2: no face covers these chars — emit nothing rather than a
            // wall of tofu. (In degraded mode `resolve` answers face 0 instead, so
            // the primary's `.notdef` still draws and the missing font stays
            // visible.)
            let Some(style) = run.style else {
                continue;
            };
            let face_index = style.face;
            // D4: the shaper is built from the **slot's** face, not the family's
            // regular one. Shaping Menlo Regular and then rasterizing the result
            // from Menlo Italic would draw the wrong glyphs outright — 1,712 of
            // 2,116 codepoints have different ids between those two faces.
            let Some(font) = fonts
                .families()
                .get(face_index)
                .and_then(|family| family.slot(style.slot))
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
                // Colour still comes from the cluster's lead cell (H-D11) — it is a
                // per-cell property that does not affect shaping.
                //
                // The *style* no longer does, and that is the T4 change: it was
                // read here, after the shaper had already been built from the
                // regular face. It now lives on the run, decided before shaping,
                // because it chooses which face shapes at all (D4). Every cell in
                // this run shares it by construction of `split_runs`.
                let color = snapshot.cell(col, row).map_or([1.0; 3], glyph_color);
                // D6: a fallback cluster is centered in the span its lead cell
                // covers; face 0 keeps `pen == col * cw` exactly as before (R4.8).
                let adv: f32 = cluster.glyphs.iter().map(|g| g.advance).sum();
                let offset = center_offset(face_index, cell_span(snapshot, col, row), cw, adv);
                let mut pen = col as f32 * cw + offset;
                for g in cluster.glyphs {
                    // The whole key is the run's, not a guess. Glyph ids are a
                    // per-*face* namespace, and the face is `(entry, slot)` — so
                    // `slot` has to travel with `id` exactly as `face` does, or the
                    // atlas rasterizes this id from the wrong face (H-D7/H-R3.1,
                    // D15). `g.id` came from the shaper built on `style.slot`'s
                    // face just above, so the two agree by construction.
                    let key = GlyphKey {
                        face: face_index as u16,
                        slot: style.slot,
                        id: g.id,
                        synth_bold: style.synth_bold,
                        synth_italic: style.synth_italic,
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

    // Procedural box-drawing pass (D2/D4/D7): the cells taken out of shaping above,
    // drawn as quads snapped to the cell rect. Because every rect comes from
    // `box_draw`'s `band`/`snap` — pure functions of the cell size — two adjacent
    // cells compute a shared edge from identical inputs, which is what removes the
    // seams that per-glyph rasterization leaves.
    for (glyph, col, row) in procedural {
        // Style comes from the cell, exactly as it does for a shaped glyph (D11), so
        // box art honours reverse-video and dim instead of ignoring them.
        let Some(cell) = snapshot.cell(col, row) else {
            continue;
        };
        let color = glyph_color(cell);
        let (x, y) = (col as f32 * cw, row as f32 * ch);

        match glyph {
            // `░▒▓`: one whole-cell quad on a partial-coverage entry. The shader
            // multiplies that coverage into rgb and alpha alike, so this is a
            // correct premultiplied 25/50/75% fill — and it composites over an
            // inline image the way a blend toward the cell background would not.
            box_draw::Glyph::Shade(level) => {
                let Some(entry) = atlas.shade_entry(queue, level) else {
                    continue;
                };
                out.push(GlyphInstance {
                    pos: [x, y],
                    size: [cw, ch],
                    uv0: center_uv(&entry),
                    uv1: center_uv(&entry),
                    color,
                    _pad: 0.0,
                });
            }
            _ => {
                let bold = cell.attrs.contains(CellAttrs::BOLD);
                let entry = atlas.solid_entry(queue);
                let uv = center_uv(&entry);
                for r in box_draw::rects(cell.ch, x, y, cw, ch, bold) {
                    out.push(GlyphInstance {
                        pos: [r.x0, r.y0],
                        size: [r.width(), r.height()],
                        uv0: uv,
                        uv1: uv,
                        color,
                        _pad: 0.0,
                    });
                }
            }
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
            let uv = center_uv(&e);
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

fn clamp_font_size(size_pt: f32) -> f32 {
    if !size_pt.is_finite() {
        return FONT_SIZE_DEFAULT;
    }
    size_pt.clamp(FONT_SIZE_MIN, FONT_SIZE_MAX)
}

fn cell_size_for_pt(size_pt: f32, scale_factor: f64) -> (f32, f32) {
    let px = size_pt * scale_factor as f32;
    ((px * 0.6).round().max(1.0), (px * 1.2).round().max(1.0))
}

pub fn cell_size_px(cfg: &Config, scale_factor: f64) -> (f32, f32) {
    cell_size_for_pt(cfg.font.size_pt, scale_factor)
}

/// Cols × rows that fit a framebuffer, reserving `top_inset` px above the grid
/// (e.g. a tab bar), `left_inset` px to its left (e.g. a vertical tab sidebar)
/// and `right_inset` px to its right (the docked ai-monitor panel). Only the
/// height below the top inset holds rows; only the width *between* the two side
/// insets holds cols. Kept as a free function so the inset arithmetic is
/// unit-testable without a GPU surface. Both floors at ≥1 so a pathological inset
/// larger than the window still yields a drawable grid.
///
/// `right_inset` is the reason no shader or uniform changed for the panel (D21).
/// `BgUniforms`/`GlyphUniforms` carry `left_inset` because the grid is *shifted*
/// right by it — every cell's origin moves. A right band shifts nothing: the grid
/// still starts at `left_inset` and simply ends sooner, which is entirely
/// expressible as a smaller column count. Feeding it to the shader as well would
/// make the GPU a second author of the same fact, and one of the two authors would
/// eventually be updated alone.
fn grid_dims(
    width: f32,
    height: f32,
    cell: (f32, f32),
    top_inset: f32,
    left_inset: f32,
    right_inset: f32,
) -> (u16, u16) {
    let usable_w = (width - left_inset - right_inset).max(0.0);
    let cols = (usable_w / cell.0).floor().max(1.0) as u16;
    let usable_h = (height - top_inset).max(0.0);
    let rows = (usable_h / cell.1).floor().max(1.0) as u16;
    (cols, rows)
}

/// Columns the terminal keeps no matter how narrow the window gets (D4). The smallest
/// width at which a shell prompt plus a short command is still readable; below it the
/// terminal stops being usable at all, and the terminal is the application — the panel is
/// the accessory, so the panel is what yields.
pub const MIN_TERMINAL_COLS: u16 = 20;

/// The panel's requested width clamped to the design's drag band (D6). Non-finite input
/// falls back to the minimum rather than propagating a `NaN` into the inset arithmetic,
/// where every later comparison would silently answer `false`.
fn clamp_monitor_width(raw: f32) -> f32 {
    if !raw.is_finite() {
        return MONITOR_WIDTH_MIN;
    }
    raw.clamp(MONITOR_WIDTH_MIN, MONITOR_WIDTH_MAX)
}

/// The band the ai-monitor panel may actually occupy, in **physical** px — D4's clamp,
/// applied to the panel's asking width at layout time (R4.3/R4.4).
///
/// Two rules, in order:
/// 1. The terminal keeps [`MIN_TERMINAL_COLS`] columns, so the band is capped at whatever
///    is left of the window once the sidebar's band and those columns are paid for. The
///    check is arithmetically identical to `grid_dims(..) .0 >= MIN_TERMINAL_COLS` —
///    `floor(x) >= 20 ⟺ x >= 20` — which is why the sidebar's width is accounted for
///    automatically rather than by a second rule here.
/// 2. When even `min_px` does not fit inside that cap the band is **zero** and the panel is
///    not drawn: a 40px sliver of a 430px design is not a smaller panel, it is a broken one.
///
/// It never touches the panel's *intent* (D8). A window narrow enough to trip rule 2
/// force-closes the band and leaves `[ai_monitor].open` and the persisted width exactly as
/// they were, so widening the window restores the panel with no keystroke (R4.5/R6.2/R6.3).
fn monitor_band_px(intent_px: f32, width: f32, cell_w: f32, left_inset: f32, min_px: f32) -> f32 {
    if !(intent_px.is_finite() && intent_px > 0.0) {
        return 0.0;
    }
    let cap = width - left_inset - MIN_TERMINAL_COLS as f32 * cell_w;
    if cap < min_px {
        return 0.0;
    }
    intent_px.min(cap)
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

/// A guarded headless `(Device, Queue)` for GPU-touching tests, or `None` when no
/// adapter is available (adapterless CI → the test skips). Hoisted to the crate root
/// (D10) so sibling modules' tests — e.g. `sidebar_shapes::tests` — can reach it;
/// a private item inside `mod tests` is invisible to sibling modules. `#[cfg(test)]`,
/// so it costs the shipped binary nothing.
#[cfg(test)]
pub(crate) fn headless() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        ..Default::default()
    }))
    .ok()?;
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()
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

    /// `capture` decodes the two 8-bit layouts and refuses everything else.
    ///
    /// A pure decision, so it is asserted without a GPU — which matters, because
    /// the formats that used to be mis-decoded are exactly the ones this machine
    /// cannot easily produce. The old code was a `matches!` on the BGRA pair whose
    /// `false` arm meant "RGBA" rather than "unknown", so the two macOS surface
    /// formats below were read as BGRA at half their real stride and returned as
    /// `Ok`.
    #[test]
    fn capture_decodes_8_bit_layouts_and_refuses_the_rest() {
        use wgpu::TextureFormat as F;
        assert_eq!(capture_channel_order(F::Bgra8UnormSrgb), Some(true));
        assert_eq!(capture_channel_order(F::Bgra8Unorm), Some(true));
        assert_eq!(capture_channel_order(F::Rgba8UnormSrgb), Some(false));
        assert_eq!(capture_channel_order(F::Rgba8Unorm), Some(false));

        // What `surface.get_default_config` can actually hand us on macOS: an EDR
        // display's float surface, and a 10-bit one. Both are wider than four
        // bytes per pixel or not byte-aligned per channel.
        assert_eq!(
            capture_channel_order(F::Rgba16Float),
            None,
            "an EDR surface is 8 bytes per pixel; decoding it as RGBA8 reads half \
             of one channel pair per 'pixel'"
        );
        assert_eq!(capture_channel_order(F::Rgb10a2Unorm), None);
        assert_eq!(capture_channel_order(F::Rgba32Float), None);
    }

    /// The primary face of the loaded chain (T2). `None` when no system font is
    /// available, which the GPU-dependent tests below treat as "skip".
    fn primary_face() -> Option<FontFace> {
        FontSet::load()
            .ok()?
            .families()
            .first()
            .map(|f| f.regular().clone())
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
        let (_, rows0) = grid_dims(1200.0, 800.0, (10.0, 20.0), 0.0, 0.0, 0.0);
        assert_eq!(rows0, 40);
        // A 40px tab bar (2 rows) reserves the top band → 38 rows below it.
        let (cols, rows1) = grid_dims(1200.0, 800.0, (10.0, 20.0), 40.0, 0.0, 0.0);
        assert_eq!(rows1, 38, "grid rows shrink by the inset, not the cols");
        assert_eq!(cols, 120, "top inset never affects column count");
        // A pathological inset taller than the window still yields ≥1 row.
        let (_, rows2) = grid_dims(1200.0, 800.0, (10.0, 20.0), 10_000.0, 0.0, 0.0);
        assert_eq!(rows2, 1);
    }

    #[test]
    fn left_inset_reserves_cols_for_the_sidebar() {
        // 1200px wide, 10px cells → 120 cols with no inset.
        let (cols0, _) = grid_dims(1200.0, 800.0, (10.0, 20.0), 0.0, 0.0, 0.0);
        assert_eq!(cols0, 120);
        // A 200px sidebar (20 cols) reserves the left band → 100 cols beside it,
        // rows unchanged (R1.1).
        let (cols1, rows1) = grid_dims(1200.0, 800.0, (10.0, 20.0), 0.0, 200.0, 0.0);
        assert_eq!(cols1, 100, "grid cols shrink by the left inset");
        assert_eq!(rows1, 40, "left inset never affects row count");
        // A pathological inset wider than the window still yields ≥1 col (R1.2).
        let (cols2, _) = grid_dims(1200.0, 800.0, (10.0, 20.0), 0.0, 10_000.0, 0.0);
        assert_eq!(cols2, 1);
        // Zero-inset invariance (R4.1): left_inset == 0.0 reproduces the no-inset
        // column count exactly.
        let (cols_z, rows_z) = grid_dims(1200.0, 800.0, (10.0, 20.0), 0.0, 0.0, 0.0);
        assert_eq!((cols_z, rows_z), (120, 40));
        // Both insets compose independently (cols from left, rows from top).
        let (cols_b, rows_b) = grid_dims(1200.0, 800.0, (10.0, 20.0), 40.0, 200.0, 0.0);
        assert_eq!((cols_b, rows_b), (100, 38));
    }

    #[test]
    fn right_inset_reserves_cols_for_the_ai_monitor_panel() {
        // R4.1: the two side bands come off the *same* width — 1200 − 200 − 300 = 700px
        // of grid, i.e. 70 cols at 10px cells — and neither touches the rows.
        let (cols, rows) = grid_dims(1200.0, 800.0, (10.0, 20.0), 0.0, 200.0, 300.0);
        assert_eq!(cols, 70, "cols come off both bands, not just the left one");
        assert_eq!(rows, 40, "no side inset ever affects the row count");
        // Rows still answer to the top inset alone, with both side bands present.
        let both = grid_dims(1200.0, 800.0, (10.0, 20.0), 40.0, 200.0, 300.0);
        assert_eq!(both, (70, 38));

        // R4.2 (zero-inset invariance): with the panel closed the result is exactly the
        // pre-panel five-argument form's, for every combination of the other two insets.
        // Pinned as literals rather than compared against a helper — a helper sharing
        // this function's body would agree with it even if both were wrong.
        for (top, left, want) in [
            (0.0, 0.0, (120, 40)),
            (0.0, 200.0, (100, 40)),
            (40.0, 0.0, (120, 38)),
            (40.0, 200.0, (100, 38)),
        ] {
            assert_eq!(
                grid_dims(1200.0, 800.0, (10.0, 20.0), top, left, 0.0),
                want,
                "right_inset 0.0 must be a no-op at top={top} left={left}"
            );
        }

        // A panel wider than the window still leaves a drawable column, like the left
        // band does (R1.2) — the floor is on the total, not per inset.
        let (cols_p, _) = grid_dims(1200.0, 800.0, (10.0, 20.0), 0.0, 0.0, 10_000.0);
        assert_eq!(cols_p, 1);
        let (cols_q, _) = grid_dims(1200.0, 800.0, (10.0, 20.0), 0.0, 700.0, 700.0);
        assert_eq!(cols_q, 1, "two bands that together exceed the window");
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
    fn sidebar_owns_width_and_set_left_inset_is_a_no_op() {
        // D4: the renderer owns the sidebar width — the config drives it at startup, a
        // stale set_left_inset cannot move it, and the runtime setters do. Needs a GPU.
        if headless().is_none() {
            eprintln!("skipping: no GPU");
            return;
        }
        let cfg = Config::default();
        let Ok(mut r) = Renderer::new_headless(1200, 800, 1.0, &cfg) else {
            eprintln!("skipping: headless renderer unavailable");
            return;
        };
        // The config's 340px logical width (× 1.0 scale) reserves the band at startup —
        // NOT 0 as before D4.
        assert_eq!(
            r.left_inset(),
            340.0,
            "startup inset comes from the config width"
        );
        let (cols_open, rows_open) = r.grid_size();

        // A stale set_left_inset argument is ignored — the band does not move (D4).
        r.set_left_inset(0.0);
        assert_eq!(r.left_inset(), 340.0, "set_left_inset(0) is a no-op");
        r.set_left_inset(99_999.0);
        assert_eq!(r.left_inset(), 340.0, "set_left_inset(huge) is a no-op");
        assert_eq!(r.grid_size(), (cols_open, rows_open), "grid unchanged");

        // Collapsing returns the reclaimed width to the grid; restoring brings it back.
        r.set_sidebar_collapsed(true);
        assert_eq!(r.left_inset(), 0.0, "collapsed → no band");
        assert!(r.grid_size().0 > cols_open, "collapse gives columns back");
        r.set_sidebar_collapsed(false);
        assert_eq!(r.left_inset(), 340.0, "restored to the config width");

        // A runtime width applies and is clamped to foundations' 260–440 band (R5.7).
        r.set_sidebar_width(260.0);
        assert_eq!(r.left_inset(), 260.0, "runtime width applied");
        r.set_sidebar_width(100_000.0);
        assert_eq!(r.left_inset(), 440.0, "clamped to SIDEBAR_WIDTH_MAX");
        assert_eq!(r.grid_size().1, rows_open, "left inset never changes rows");

        // tab-resize R9.3/D15a — a real human edit clears BOTH override fields, so the
        // file is authoritative again. Without this the two persisted fields would be the
        // only ones in config.toml that stop live-reloading once the user has dragged.
        r.set_sidebar_collapsed(true);
        r.set_sidebar_width(400.0);
        assert_eq!(r.left_inset(), 0.0, "override says collapsed");
        r.clear_sidebar_override();
        assert_eq!(
            r.left_inset(),
            340.0,
            "both override fields dropped — back to the config's width and open state"
        );
        assert_eq!(r.sidebar_width_logical(), 340.0);
        assert!(!r.sidebar_collapsed());

        // …and `set_sidebar_collapsed` does not zero `width_px`, so a restore brings back
        // whatever width the override last held. NOTE this proves a property of `Renderer`
        // alone: it does NOT mean a drag-collapse restores the pre-drag width. The drag
        // writes the clamped width on every frame down to the collapse threshold, so it
        // restores at 260 — see `drive_resize` and the spec's Known Gaps.
        r.set_sidebar_width(400.0);
        r.set_sidebar_collapsed(true);
        assert_eq!(r.left_inset(), 0.0);
        assert_eq!(
            r.sidebar_width_logical(),
            400.0,
            "collapsing must not erase the remembered width"
        );
        r.set_sidebar_collapsed(false);
        assert_eq!(r.left_inset(), 400.0, "restored at the chosen width");
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
        let latin = atlas
            .resolve('A', false, false)
            .expect("the primary covers 'A'");
        assert_eq!(latin.face, 0, "Latin must keep using the primary face");
        assert_ne!(latin.id, 0);
        if atlas.is_degraded() {
            eprintln!("skipping the fallback half: no Hangul face on this machine");
            return;
        }
        let hangul = atlas
            .resolve('한', false, false)
            .expect("some face covers U+D55C");
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
            assert_eq!(
                atlas.resolve(uncovered, false, false),
                None,
                "R2.4: nothing to draw"
            );
        }
        atlas.degrade_for_test();
        assert_eq!(
            atlas.resolve(uncovered, false, false),
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
        let primary = atlas
            .resolve('A', false, false)
            .expect("the primary covers 'A'");
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

    /// A `RunStyle` naming only a chain entry — the shape every pre-style test
    /// meant when it wrote `Some(0)`.
    fn plain_run(face: usize) -> Option<RunStyle> {
        Some(RunStyle {
            face,
            slot: StyleSlot::Regular,
            synth_bold: false,
            synth_italic: false,
        })
    }

    /// Glyph instances for one line of text, optionally bolded via SGR (T4).
    fn styled_instances(
        atlas: &mut Atlas,
        queue: &wgpu::Queue,
        fonts: &FontSet,
        bold: bool,
        text: &str,
    ) -> Vec<GlyphInstance> {
        let mut t = term_core::Terminal::new(8, 1, term_core::Color(0xFFFFFF), term_core::Color(0));
        if bold {
            t.advance(b"\x1b[1m");
        }
        t.advance(text.as_bytes());
        let snap = t.snapshot();
        build_glyph_instances(atlas, queue, fonts, true, (20.0, 40.0), 32.0, &snap)
    }

    #[test]
    fn bold_hangul_is_shaped_and_rasterized_from_the_bold_face() {
        // R0.1 end to end, through the production path rather than the resolver.
        // Before T4 this test could not distinguish anything: the run was shaped
        // from the regular face either way and only the synthesis flag differed.
        let (Some((device, queue)), Some(fonts)) = (headless(), fonts()) else {
            eprintln!("skipping: no GPU/font");
            return;
        };
        let mut atlas = Atlas::new(&device, &fonts, 32.0).expect("atlas");
        if atlas.is_degraded() {
            eprintln!("skipping: no Hangul fallback on this machine");
            return;
        }
        assert_eq!(
            atlas.resolve('한', true, false).map(|k| k.slot),
            Some(StyleSlot::Bold),
            "precondition: bold 한 must resolve to a real bold face, or this test \
             proves nothing"
        );

        let plain = styled_instances(&mut atlas, &queue, &fonts, false, "한");
        let bold = styled_instances(&mut atlas, &queue, &fonts, true, "한");
        assert!(!plain.is_empty() && !bold.is_empty(), "both must draw");

        // Different faces ⇒ different rasterized bitmaps ⇒ different atlas rects.
        assert_ne!(
            (plain[0].uv0, plain[0].uv1),
            (bold[0].uv0, bold[0].uv1),
            "bold must come from a different face's outline, not the same glyph"
        );
        // R4.3: and it must not move. Cell width is wcwidth's, not the font's, and
        // Hangul advances are identical between the two faces (D16).
        // R4.3: bolding must not *move* the character, only thicken it. Asserting
        // equal `pos[0]` would be wrong — a heavier glyph has a wider bitmap and a
        // smaller left bearing, so its origin legitimately shifts. What must hold
        // is that it grows **symmetrically** about the same centre: measured here,
        // plain is 26px wide at x=7.16 and bold is 28px at x=6.16, so both centre
        // on 20.16. A bold glyph drifting inside its cell would show up as a
        // centre that moved.
        let centre = |g: &GlyphInstance| g.pos[0] + g.size[0] / 2.0;
        assert!(
            (centre(&plain[0]) - centre(&bold[0])).abs() < 0.01,
            "bold must grow about the same centre, not drift: plain centre {}, \
             bold centre {}",
            centre(&plain[0]),
            centre(&bold[0])
        );
        assert!(
            bold[0].size[0] > plain[0].size[0],
            "and the real bold face must actually be wider"
        );
    }

    #[test]
    fn a_row_mixing_styles_still_places_every_glyph_on_its_own_column() {
        // R4.1: the run now breaks at the style boundary, so this row shapes as
        // several runs instead of one. Placement is anchored per cluster at
        // `col * cw`, so splitting must not move anything.
        let (Some((device, queue)), Some(fonts)) = (headless(), fonts()) else {
            eprintln!("skipping: no GPU/font");
            return;
        };
        let mut atlas = Atlas::new(&device, &fonts, 32.0).expect("atlas");
        let mut t = term_core::Terminal::new(8, 1, term_core::Color(0xFFFFFF), term_core::Color(0));
        t.advance(b"a\x1b[1mb\x1b[0mc");
        let snap = t.snapshot();
        let inst =
            build_glyph_instances(&mut atlas, &queue, &fonts, true, (20.0, 40.0), 32.0, &snap);
        assert_eq!(inst.len(), 3, "three visible cells, three quads");
        let xs: Vec<f32> = inst.iter().map(|g| g.pos[0]).collect();
        assert!(
            xs.windows(2).all(|w| w[1] > w[0]),
            "glyphs must stay in column order across run boundaries: {xs:?}"
        );
    }

    #[test]
    fn split_runs_groups_consecutive_same_face() {
        // H-R4.1: maximal runs — the same face returning later starts a NEW run, it
        // does not rejoin the earlier one (shaping is per contiguous span).
        let styles = [
            plain_run(0),
            plain_run(0),
            plain_run(1),
            plain_run(1),
            plain_run(0),
        ];
        assert_eq!(
            split_runs(&styles),
            vec![
                Run {
                    style: plain_run(0),
                    start_char: 0,
                    end_char: 2
                },
                Run {
                    style: plain_run(1),
                    start_char: 2,
                    end_char: 4
                },
                Run {
                    style: plain_run(0),
                    start_char: 4,
                    end_char: 5
                },
            ]
        );
        // The H-R0.3 shape: an all-primary unstyled row is ONE run over the whole
        // row, so it is shaped exactly as the single-font path shaped it.
        assert_eq!(
            split_runs(&[plain_run(0); 5]),
            vec![Run {
                style: plain_run(0),
                start_char: 0,
                end_char: 5
            }]
        );
        assert!(split_runs(&[]).is_empty());
    }

    #[test]
    fn split_runs_isolates_uncovered_chars() {
        // H-R4.2: `None` (no face covers it) is a value like any other — such chars
        // group together and the caller emits nothing for them (H-D5).
        let runs = split_runs(&[plain_run(0), None, None, plain_run(1)]);
        assert_eq!(runs.len(), 3);
        assert_eq!(runs[1].style, None);
        assert_eq!((runs[1].start_char, runs[1].end_char), (1, 3));
        // Two different covered faces never merge just because neither is `None`.
        assert_eq!(split_runs(&[plain_run(1), plain_run(2)]).len(), 2);
    }

    #[test]
    fn ligatures_shape_identically_from_a_styled_face() {
        // R4.4/D17. Splitting runs by slot means a bold run is shaped by a
        // *different font file face* than its regular neighbour. If that face
        // carried a different ligature set, `!=` would collapse to one glyph in
        // one weight and stay two in the other, and the two lines would no longer
        // align column-for-column. Measured here rather than assumed.
        let Some(fonts) = fonts() else {
            eprintln!("skipping: no font");
            return;
        };
        let mut ctx = ShapeContext::new();
        let features = [("liga", 1u16), ("calt", 1u16)];
        // Counted, because a test that compares nothing must not report success.
        // Skipping when the *font* is absent is legitimate; skipping when the font
        // loaded and its bold slot is empty is the bug this suite exists to catch,
        // so that case asserts instead of continuing (same rule as
        // `bold_faces.rs::pair`, which this test originally failed to follow).
        let mut compared = 0;
        for (label, family_index, text) in [
            ("Menlo", 0usize, "!= => -> <= fi fl ff"),
            ("Gothic", 1usize, "한글 볼드 테스트"),
        ] {
            let Some(family) = fonts.families().get(family_index) else {
                continue; // no such chain entry on this machine — a real skip
            };
            let bold_face = family.slot(StyleSlot::Bold).unwrap_or_else(|| {
                panic!(
                    "{label} loaded but has no real bold face — the style probe has \
                     regressed to synthesis (real slots: {:?})",
                    family.real_slots()
                )
            });
            compared += 1;
            let mut shape = |face: &crate::FontFace| -> (usize, usize, f32) {
                let font = FontRef::from_index(&face.data, face.index).expect("face");
                let mut s = ctx
                    .builder(font)
                    .size(32.0)
                    .features(features.iter().copied())
                    .build();
                s.add_str(text);
                let (mut clusters, mut glyphs, mut adv) = (0, 0, 0.0f32);
                s.shape_with(|c| {
                    clusters += 1;
                    glyphs += c.glyphs.len();
                    adv += c.glyphs.iter().map(|g| g.advance).sum::<f32>();
                });
                (clusters, glyphs, adv)
            };
            let regular = shape(family.regular());
            let bold = shape(bold_face);
            assert_eq!(
                (regular.0, regular.1),
                (bold.0, bold.1),
                "{label}: regular and bold must produce the same cluster/glyph \
                 structure for {text:?}"
            );
            assert!(
                (regular.2 - bold.2).abs() < 0.01,
                "{label}: total advance must match ({} vs {})",
                regular.2,
                bold.2
            );
        }
        assert!(
            compared > 0,
            "no family was actually compared — this test asserted nothing"
        );
    }

    #[test]
    fn split_runs_breaks_where_the_slot_changes_not_just_the_face() {
        // D4/R4.1 — the T4 change. These three chars are all on chain entry 0, so
        // the old face-only key made them ONE run and shaped them with one face.
        // They need three shapers: bold, regular, bold-italic are three different
        // font files' worth of glyph ids.
        let styled = |slot, sb, si| {
            Some(RunStyle {
                face: 0,
                slot,
                synth_bold: sb,
                synth_italic: si,
            })
        };
        let runs = split_runs(&[
            styled(StyleSlot::Bold, false, false),
            styled(StyleSlot::Regular, false, false),
            styled(StyleSlot::BoldItalic, false, false),
        ]);
        assert_eq!(runs.len(), 3, "same face, three slots, three runs");

        // And the subtle one: same *requested* style, different *resolved* slot.
        // Italic text containing a char the italic face lacks splits there, because
        // that char alone shapes from the regular face with a shear (D7/R3.2).
        let runs = split_runs(&[
            styled(StyleSlot::Italic, false, false),
            styled(StyleSlot::Regular, false, true), // the hole
            styled(StyleSlot::Italic, false, false),
        ]);
        assert_eq!(runs.len(), 3, "a coverage hole breaks the run");

        // Synthesis differences split too, so a run's GlyphKey is constant.
        let runs = split_runs(&[
            styled(StyleSlot::Regular, false, false),
            styled(StyleSlot::Regular, true, false),
        ]);
        assert_eq!(runs.len(), 2);
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
        let face = fonts
            .families()
            .first()
            .map(|f| f.regular().clone())
            .expect("a primary face");
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
                        slot: StyleSlot::Regular,
                        synth_bold: bold,
                        synth_italic: italic,
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

    /// Build the grid's instances for `input` on a `cols`×`rows` snapshot, at the
    /// 14pt @2x reference cell. `None` when this machine has no GPU or font.
    fn instances_for(input: &str, cols: u16, rows: u16) -> Option<(Vec<GlyphInstance>, f32, f32)> {
        let (device, queue) = headless()?;
        let fonts = fonts()?;
        let mut atlas = Atlas::new(&device, &fonts, 32.0)?;
        let mut t =
            term_core::Terminal::new(cols, rows, term_core::Color(0xFFFFFF), term_core::Color(0));
        t.advance(input.as_bytes());
        let snap = t.snapshot();
        let (cw, ch) = (17.0f32, 34.0f32);
        Some((
            build_glyph_instances(&mut atlas, &queue, &fonts, false, (cw, ch), 32.0, &snap),
            cw,
            ch,
        ))
    }

    #[test]
    fn adjacent_box_stems_share_an_exact_edge() {
        // R0.1: the goal, end to end. Three `─` cells must tile with no gap and no
        // overlap — the seam the font path leaves is exactly what this removes.
        let Some((got, cw, _)) = instances_for("───", 6, 1) else {
            eprintln!("skipping: no GPU/font");
            return;
        };
        let mut spans: Vec<(f32, f32)> = got
            .iter()
            .map(|i| (i.pos[0], i.pos[0] + i.size[0]))
            .collect();
        spans.sort_by(|a, b| a.0.total_cmp(&b.0));
        assert_eq!(
            spans.len(),
            3,
            "one quad per `─` cell — opposing arms merge"
        );
        // Coalesce touching spans. Three separate cells collapsing to a single run
        // is the seamlessness claim: any gap at a cell join leaves two runs here.
        let merged: Vec<(f32, f32)> = spans.iter().fold(Vec::new(), |mut acc, &(s, e)| {
            match acc.last_mut() {
                Some(last) if s <= last.1 => last.1 = last.1.max(e),
                _ => acc.push((s, e)),
            }
            acc
        });
        assert_eq!(
            merged,
            vec![(0.0, 3.0 * cw)],
            "three `─` cells must merge into one unbroken run"
        );
        // Every quad sits on the identical y band.
        let ys: Vec<(u32, u32)> = got
            .iter()
            .map(|i| (i.pos[1].to_bits(), i.size[1].to_bits()))
            .collect();
        assert!(ys.windows(2).all(|w| w[0] == w[1]), "one shared band");
    }

    #[test]
    fn vertically_adjacent_box_stems_share_an_exact_edge() {
        // R0.3: the same claim on the other axis — `│` over `│`.
        let Some((got, _, ch)) = instances_for("│\r\n│", 4, 2) else {
            eprintln!("skipping: no GPU/font");
            return;
        };
        assert_eq!(got.len(), 2, "one quad per stem");
        let mut spans: Vec<(f32, f32)> = got
            .iter()
            .map(|i| (i.pos[1], i.pos[1] + i.size[1]))
            .collect();
        spans.sort_by(|a, b| a.0.total_cmp(&b.0));
        assert_eq!(spans[0], (0.0, ch));
        assert_eq!(spans[1], (ch, 2.0 * ch), "abuts exactly, no gap or overlap");
        assert_eq!(got[0].pos[0], got[1].pos[0], "same x band");
        assert_eq!(got[0].size[0], got[1].size[0]);
    }

    #[test]
    fn a_box_char_is_not_shaped() {
        // R1.2: the interception actually bypasses swash. A `─` cell emits quads
        // whose UVs are the solid entry's center point (uv0 == uv1) — a shaped
        // glyph always spans a real UV rect.
        let Some((got, _, _)) = instances_for("─", 4, 1) else {
            eprintln!("skipping: no GPU/font");
            return;
        };
        assert!(!got.is_empty());
        for i in &got {
            assert_eq!(i.uv0, i.uv1, "a solid fill samples one point, not a rect");
        }

        // R8.3: and that point must be the entry's CENTER, not just any single
        // texel — a corner sample would also satisfy `uv0 == uv1` while picking up
        // a neighbouring glyph's coverage under linear filtering.
        let (device, queue) = headless().expect("checked above");
        let fonts = fonts().expect("checked above");
        let mut atlas = Atlas::new(&device, &fonts, 32.0).expect("atlas");
        let entry = atlas.solid_entry(&queue);
        assert_eq!(
            got[0].uv0,
            center_uv(&entry),
            "procedural quads must sample the solid block's center"
        );
    }

    #[test]
    fn sibling_layers_still_shape_box_chars_through_the_font() {
        // R8.5 / D9: interception is the GRID's behaviour only. A sidebar row or
        // overlay containing `─` draws it at its own non-cell-snapped position, so
        // cell-snapped rects would be meaningless there — those layers must keep
        // resolving the character through the atlas like any other glyph.
        let Some((device, queue)) = headless() else {
            eprintln!("skipping: no GPU");
            return;
        };
        let Some(fonts) = fonts() else {
            eprintln!("skipping: no font");
            return;
        };
        let mut atlas = Atlas::new(&device, &fonts, 32.0).expect("atlas");

        // The sibling text path resolves box characters to a real rasterized glyph
        // — a UV *rect*, where a procedural quad collapses to one sampled point.
        for ch in ['─', '│', '█'] {
            let key = atlas
                .resolve(ch, false, false)
                .expect("the primary covers the range");
            let entry = atlas
                .get_or_insert_styled(&queue, key)
                .unwrap_or_else(|| panic!("{ch:?} must rasterize for the text layers"));
            assert_ne!(
                (entry.uv0[0], entry.uv0[1]),
                (entry.uv1[0], entry.uv1[1]),
                "{ch:?} must still rasterize as a shaped glyph for the text layers"
            );
        }

        // The above is a property of `Atlas`, not proof that the sibling layers
        // decline to intercept. THIS is that proof, and it is what actually fails if
        // one of them starts: interception exists only where `box_draw` is consulted,
        // and it is consulted from exactly one module. `sidebar_row` calling
        // `box_draw::classify` would break this immediately.
        //
        // (`push_glyphs` in `sidebar_row` is private and needs a live `FrameCtx`, so
        // driving it directly would cost a GPU harness for a weaker guarantee than
        // this structural one.)
        // Derived from the source directory rather than a hardcoded list, so a
        // module added later is covered automatically instead of escaping silently.
        let src_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src");
        let mut checked = 0usize;
        for entry in std::fs::read_dir(src_dir).expect("the crate's own src/ dir") {
            let path = entry.expect("a readable dir entry").path();
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_string();
            // The grid path and the module itself are the two places allowed to
            // know about box_draw.
            if name == "lib.rs" || name == "box_draw.rs" {
                continue;
            }
            let src = std::fs::read_to_string(&path).expect("a readable source file");
            assert!(
                !src.contains("box_draw"),
                "{name} references box_draw — interception must stay in the grid \
                 path (D9); sibling layers draw at their own unsnapped positions \
                 where cell-snapped rects are meaningless"
            );
            checked += 1;
        }
        assert!(
            checked >= 8,
            "expected to scan the sibling modules, only saw {checked} — the walk \
             is not finding the source tree"
        );
    }

    // R8.6 (cursor/selection compositing over a procedural quad) has **no test of
    // its own**, deliberately. Its two halves are each already covered elsewhere and
    // a test here would only restate them:
    //   * z-order — `selection_layer::SELECTION_Z` / `cursor_layer::CURSOR_Z` are
    //     compile-time constants, already asserted by
    //     `selection_layer::tests::z_order_above_glyph_and_below_cursor`.
    //   * "neither layer re-renders the cell's character" — read from the layers
    //     themselves (`cursor_layer.rs:3`, `selection_layer.rs:9`): both draw a
    //     translucent quad over the cell rect and never touch the atlas.
    // Restating either here would add a passing test and no failure mode. The
    // residual — that the composite *looks* right on screen — is a visual check,
    // recorded as post-work in the spec rather than faked as a unit test.

    #[test]
    fn a_diagonal_still_shapes_through_the_font() {
        // R1.3: `╱` is the sole fallthrough. It must still come out as a real glyph
        // — a UV *rect*, not the solid entry's collapsed sample point.
        let Some((got, _, _)) = instances_for("╱", 4, 1) else {
            eprintln!("skipping: no GPU/font");
            return;
        };
        if got.is_empty() {
            eprintln!("skipping: the primary face does not cover U+2571");
            return;
        }
        assert!(
            got.iter().any(|i| i.uv0 != i.uv1),
            "the diagonal must be a shaped glyph, not a procedural fill"
        );
    }

    #[test]
    fn procedural_quads_honour_reverse_and_dim() {
        // R8.1/R8.2: box art must respond to the same attributes a glyph does —
        // reverse-video swaps fg/bg, dim mixes toward the background.
        let Some((device, queue)) = headless() else {
            eprintln!("skipping: no GPU");
            return;
        };
        let Some(fonts) = fonts() else {
            eprintln!("skipping: no font");
            return;
        };
        let mut atlas = Atlas::new(&device, &fonts, 32.0).expect("atlas");
        let mut t = term_core::Terminal::new(4, 1, term_core::Color(0xFFFFFF), term_core::Color(0));
        // SGR 7 = reverse, then a box char.
        t.advance("\x1b[7m│".as_bytes());
        let snap = t.snapshot();
        let got =
            build_glyph_instances(&mut atlas, &queue, &fonts, false, (17.0, 34.0), 32.0, &snap);
        assert!(!got.is_empty(), "the stem must draw");
        let cell = snap.cell(0, 0).expect("a cell");
        assert!(cell.attrs.contains(CellAttrs::REVERSE), "reverse is set");

        // A plain `│` for comparison — without it, `color == glyph_color(cell)`
        // would hold even if REVERSE were ignored everywhere, since both sides
        // would then be the raw foreground.
        let mut plain =
            term_core::Terminal::new(4, 1, term_core::Color(0xFFFFFF), term_core::Color(0));
        plain.advance("│".as_bytes());
        let plain_snap = plain.snapshot();
        let plain_cell = plain_snap.cell(0, 0).expect("a cell");
        for i in &got {
            assert_eq!(
                i.color,
                glyph_color(cell),
                "the quad uses the cell's resolved color, not a raw fg"
            );
            assert_ne!(
                i.color,
                glyph_color(plain_cell),
                "a REVERSE quad must not render in the un-swapped foreground"
            );
        }

        // R8.2 — DIM, which the reverse case above does NOT exercise. Mixing toward
        // the background must reach a procedural quad exactly as it reaches a glyph,
        // so a dim frame reads as dim rather than full-strength.
        let mut t = term_core::Terminal::new(4, 1, term_core::Color(0xFFFFFF), term_core::Color(0));
        t.advance("\x1b[2m│".as_bytes()); // SGR 2 = dim
        let dim_snap = t.snapshot();
        let dim_cell = dim_snap.cell(0, 0).expect("a cell");
        assert!(
            dim_cell.attrs.contains(CellAttrs::DIM),
            "the DIM attribute must actually be set, or this proves nothing"
        );
        let dim_got = build_glyph_instances(
            &mut atlas,
            &queue,
            &fonts,
            false,
            (17.0, 34.0),
            32.0,
            &dim_snap,
        );
        assert!(!dim_got.is_empty(), "the dim stem must draw");

        let mut plain =
            term_core::Terminal::new(4, 1, term_core::Color(0xFFFFFF), term_core::Color(0));
        plain.advance("│".as_bytes());
        let plain_snap = plain.snapshot();
        let plain_cell = plain_snap.cell(0, 0).expect("a cell");
        for i in &dim_got {
            assert_eq!(i.color, glyph_color(dim_cell), "dim colour must be applied");
            assert_ne!(
                i.color,
                glyph_color(plain_cell),
                "a DIM quad must not render at full foreground strength"
            );
        }
    }

    #[test]
    fn shading_blocks_use_a_partial_coverage_entry() {
        // R6.5 end to end: `▒` must land on a DIFFERENT atlas entry than a solid
        // block — that difference is the only thing making it 50% rather than opaque.
        // Both must be drawn into the SAME atlas: two fresh atlases would each pack
        // their first block at the origin, so the UVs would collide for a reason
        // that has nothing to do with coverage.
        let Some((got, cw, ch)) = instances_for("▒█", 4, 1) else {
            eprintln!("skipping: no GPU/font");
            return;
        };
        assert_eq!(got.len(), 2, "one quad each");
        let shade = got.iter().find(|i| i.pos[0] == 0.0).expect("`▒` at col 0");
        let solid = got.iter().find(|i| i.pos[0] == cw).expect("`█` at col 1");
        assert_ne!(
            (shade.uv0[0].to_bits(), shade.uv0[1].to_bits()),
            (solid.uv0[0].to_bits(), solid.uv0[1].to_bits()),
            "`▒` must sample a partial-coverage block, not the opaque one"
        );
        // Both still cover the whole cell.
        assert_eq!(shade.size, [cw, ch]);
        assert_eq!(solid.size, [cw, ch]);
    }

    /// Reference implementation of the **pre-interception** placement path.
    ///
    /// This resolves *every* character through `atlas.resolve`, exactly as the code
    /// did before interception existed — so `abc─def` is shaped as ONE unbroken
    /// face-0 run, not as `abc` / `def`. The box-drawing clusters it produces are
    /// then dropped **by column**, leaving only the glyphs that must not have moved.
    ///
    /// That distinction is the entire point. An earlier version of this helper
    /// marked box characters `None` here too, which reproduced the *post*-change run
    /// split — so `want` and `got` came out of identical logic and the comparison
    /// could not fail for the reason R9.1 states. It only caught column desync.
    /// Shaping the unsplit run is what actually exercises D8's claim: that splitting
    /// a face-0 run at a box character moves no surviving glyph, because `pen` is
    /// reset per cluster to `col * cw + offset` and `center_offset(0, ..) == 0.0`.
    fn reference_instances(
        atlas: &mut Atlas,
        queue: &wgpu::Queue,
        fonts: &FontSet,
        ligatures: bool,
        cell: (f32, f32),
        ppem: f32,
        snapshot: &GridSnapshot,
    ) -> Vec<GlyphInstance> {
        let (cw, ch) = cell;
        let ascent = ch * 0.8;
        let liga: u16 = if ligatures { 1 } else { 0 };
        let features = [("liga", liga), ("calt", liga)];
        let mut out = Vec::new();
        let mut shape_ctx = ShapeContext::new();
        for row in 0..snapshot.rows {
            let RowChars { chars, char_col } = row_chars(snapshot, row);
            // Pre-change behaviour: no interception, so a box char the primary
            // covers joins the surrounding face-0 run instead of splitting it.
            let styles: Vec<Option<RunStyle>> = chars
                .iter()
                .enumerate()
                .map(|(i, &c)| {
                    let (bold, italic) = cell_style(snapshot, char_col[i], row);
                    atlas.resolve(c, bold, italic).map(|key| RunStyle {
                        face: key.face as usize,
                        slot: key.slot,
                        synth_bold: key.synth_bold,
                        synth_italic: key.synth_italic,
                    })
                })
                .collect();
            let baseline = row as f32 * ch + ascent;
            for run in split_runs(&styles) {
                let Some(style) = run.style else { continue };
                let face_index = style.face;
                let Some(font) = fonts
                    .families()
                    .get(face_index)
                    .and_then(|family| family.slot(style.slot))
                    .and_then(|face| FontRef::from_index(&face.data, face.index))
                else {
                    continue;
                };
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
                    // Drop the clusters that are now drawn procedurally — by COLUMN,
                    // not by face, so the run above stays unsplit and the surviving
                    // glyphs are placed exactly as they were before interception.
                    let procedural = snapshot
                        .cell(col, row)
                        .map(|c| box_draw::classify(c.ch).is_some())
                        .unwrap_or(false);
                    if procedural {
                        return;
                    }
                    let color = snapshot.cell(col, row).map_or([1.0; 3], glyph_color);
                    let adv: f32 = cluster.glyphs.iter().map(|g| g.advance).sum();
                    let offset = center_offset(face_index, cell_span(snapshot, col, row), cw, adv);
                    let mut pen = col as f32 * cw + offset;
                    for g in cluster.glyphs {
                        let key = GlyphKey {
                            face: face_index as u16,
                            id: g.id,
                            slot: style.slot,
                            synth_bold: style.synth_bold,
                            synth_italic: style.synth_italic,
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
        out
    }

    /// Assert every *shaped* instance in `input`'s row is placed identically to the
    /// reference path — i.e. that interception moved nothing that still shapes.
    ///
    /// Procedural quads are identified by their collapsed UV (`uv0 == uv1`) and
    /// excluded; everything else must match field for field.
    fn assert_shaped_output_unchanged(input: &str, cols: u16, ligatures: bool) {
        let (Some((device, queue)), Some(fonts)) = (headless(), fonts()) else {
            eprintln!("skipping: no GPU/font");
            return;
        };
        let mut atlas = Atlas::new(&device, &fonts, 32.0).expect("atlas");
        let mut t =
            term_core::Terminal::new(cols, 1, term_core::Color(0xFFFFFF), term_core::Color(0));
        t.advance(input.as_bytes());
        let snap = t.snapshot();
        let cell = (17.0f32, 34.0f32);

        // The whole test rests on the reference shaping an UNSPLIT run: if the
        // primary face didn't cover the box characters, the pre-change path would
        // have split there too and this would degenerate into comparing identical
        // logic. Assert the premise instead of trusting it.
        let box_chars: Vec<char> = input
            .chars()
            .filter(|&c| box_draw::classify(c).is_some())
            .collect();
        assert!(
            !box_chars.is_empty(),
            "{input:?}: no box character — nothing is being intercepted"
        );
        for &c in &box_chars {
            assert!(
                atlas.resolve(c, false, false).is_some(),
                "{input:?}: the primary face does not cover {c:?}, so the reference \
                 run would split anyway and this comparison proves nothing"
            );
        }

        let got = build_glyph_instances(&mut atlas, &queue, &fonts, ligatures, cell, 32.0, &snap);
        let want = reference_instances(&mut atlas, &queue, &fonts, ligatures, cell, 32.0, &snap);

        let shaped: Vec<&GlyphInstance> = got.iter().filter(|i| i.uv0 != i.uv1).collect();
        // Anti-vacuity: with no shaped glyphs on either side the comparison below
        // is an empty loop and would "pass" while proving nothing.
        assert!(
            !want.is_empty(),
            "{input:?}: the reference path produced no glyphs — test is vacuous"
        );
        assert!(
            got.iter().any(|i| i.uv0 == i.uv1),
            "{input:?}: no procedural quad was emitted — nothing was intercepted"
        );
        assert_eq!(
            shaped.len(),
            want.len(),
            "{input:?}: shaped glyph count changed"
        );
        for (a, b) in shaped.iter().zip(&want) {
            assert_eq!(a.pos, b.pos, "{input:?}: a shaped glyph moved");
            assert_eq!(a.size, b.size, "{input:?}");
            assert_eq!((a.uv0, a.uv1), (b.uv0, b.uv1), "{input:?}");
            assert_eq!(a.color, b.color, "{input:?}");
        }
    }

    #[test]
    fn ascii_is_unmoved_by_an_intercepted_neighbour() {
        // R9.1: the byte-identical constraint. Splitting a face-0 run at `─` must
        // move nothing — `pen` resets per cluster and `center_offset(0, ..)` is 0.0.
        // Run with ligatures ON, because that is the case where a run split could
        // in principle change shaping context.
        assert_shaped_output_unchanged("abc─def", 12, true);
        assert_shaped_output_unchanged("ab!=cd─ef", 16, true);
        assert_shaped_output_unchanged("─abc", 8, true);
        assert_shaped_output_unchanged("abc─", 8, true);
        assert_shaped_output_unchanged("a─b─c─d", 12, true);
    }

    #[test]
    fn ascii_is_unmoved_with_ligatures_off() {
        // The default configuration (`font.ligatures = false`).
        assert_shaped_output_unchanged("abc─def", 12, false);
    }

    #[test]
    fn hangul_is_unmoved_by_an_intercepted_neighbour() {
        // R9.2: the guarantee has to cover the fallback chain, not just face 0.
        // `center_offset` for face > 0 reads the cluster's own advance and the lead
        // cell's span — both per-cluster — so a new run boundary cannot move it.
        assert_shaped_output_unchanged("한글─한글", 16, false);
        assert_shaped_output_unchanged("한─a─글", 16, false);
    }

    #[test]
    fn a_diagonal_is_unmoved() {
        // R9.3: `╱` is the one character in the range still shaped; it must be
        // placed exactly as before even with procedural neighbours.
        assert_shaped_output_unchanged("─╱─", 8, false);
    }

    #[test]
    fn wide_chars_keep_their_columns_beside_a_box_char() {
        // R9.4: a double-width char occupies two columns but one `chars` entry, so
        // a column/char index mix-up in the interception loop would desync every
        // column after it. Marking `faces[i] = None` (rather than removing the
        // element) is what prevents that — this pins it.
        assert_shaped_output_unchanged("한─한", 12, false);

        let Some((got, cw, _)) = instances_for("한─한", 12, 1) else {
            eprintln!("skipping: no GPU/font");
            return;
        };
        // The box quad must land on column 2 — after the wide char's two columns —
        // not on column 1 (its continuation spacer).
        let procedural: Vec<&GlyphInstance> = got.iter().filter(|i| i.uv0 == i.uv1).collect();
        assert!(!procedural.is_empty(), "the stem must draw");
        for i in &procedural {
            assert_eq!(
                i.pos[0],
                2.0 * cw,
                "the box quad anchors to its own column, not the spacer's"
            );
        }
    }

    #[test]
    fn cell_size_is_integral() {
        // R10.1 / D10: every procedural rect is derived from `(col * cw, row * ch)`
        // and rounded against `cw`/`ch`. Integral cell dimensions are what make a
        // stem land on whole pixels and keep its width uniform across the screen.
        //
        // This invariant currently holds because `cell_size_px` rounds. The deferred
        // `font.metrics()`-based cell sizing is exactly the change that could break
        // it — if this test fails there, box art gets uneven stems, and THAT is the
        // reason this assertion exists rather than a comment.
        let mut cfg = Config::default();
        for pt in [8.0f32, 10.0, 14.0, 18.0, 24.0, 32.0] {
            for sf in [1.0f64, 2.0] {
                cfg.font.size_pt = pt;
                let (cw, ch) = cell_size_px(&cfg, sf);
                assert_eq!(cw, cw.floor(), "cw not integral at {pt}pt @{sf}x");
                assert_eq!(ch, ch.floor(), "ch not integral at {pt}pt @{sf}x");
                assert!(cw >= 1.0 && ch >= 1.0, "degenerate cell at {pt}pt @{sf}x");
            }
        }
    }

    #[test]
    fn no_box_drawing_config_key_exists() {
        // R10.2 / D5: the procedural path is unconditional — there is no toggle.
        //
        // Expressed as an exhaustive struct literal rather than a parse check: a
        // toggle would have to be a new `FontConfig` field, and adding one makes
        // this fail to COMPILE with "missing field". That is a stronger guard than
        // asserting a config string is rejected at runtime, and it costs the crate
        // no new dependency.
        let _no_toggle = common::config::FontConfig {
            family: String::new(),
            size_pt: 14.0,
            ligatures: false,
        };
        // And the draw path takes no configuration beyond ligatures, so nothing
        // could switch interception off even if a key existed.
        assert!(box_draw::classify('─').is_some());
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
                                slot: StyleSlot::Regular,
                                synth_bold: bold,
                                synth_italic: italic,
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
    fn draw_to_hands_layers_the_renderers_own_font_set() {
        // sidebar-weather-cards R2.2 — through the real frame path, not a hand-built
        // FrameCtx: the set a layer sees is the one the renderer's atlas came from, so a
        // layer building atlases of its own (the sidebar's card atlases) gets the same
        // fonts and bold faces as the grid.
        let Some(mut r) =
            crate::testing::headless(64, 64, &common::config::Config::default(), "R2.2")
        else {
            return;
        };
        let identity = |fonts: &FontSet| {
            fonts
                .families()
                .iter()
                .map(|f| std::sync::Arc::as_ptr(&f.regular().data) as usize)
                .collect::<Vec<_>>()
        };
        type Seen = std::rc::Rc<std::cell::RefCell<Vec<usize>>>;
        struct Spy(Seen, fn(&FontSet) -> Vec<usize>);
        impl DrawLayer for Spy {
            fn z_order(&self) -> i32 {
                50
            }
            fn record(&mut self, ctx: &mut FrameCtx<'_>) {
                *self.0.borrow_mut() = (self.1)(ctx.fonts);
            }
        }
        let seen: Seen = Default::default();
        r.add_layer(Box::new(Spy(seen.clone(), identity)));
        let t = term_core::Terminal::new(4, 1, term_core::Color(0xFFFFFF), term_core::Color(0));
        r.capture(&t.snapshot()).expect("capture");
        assert!(!seen.borrow().is_empty(), "the layer recorded");
        assert_eq!(*seen.borrow(), identity(&r.fonts));
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
            right_inset: 0.0,
            format: wgpu::TextureFormat::Rgba8Unorm,
            sidebar: &SidebarConfig::default(),
            scale_factor: 1.0,
            snapshot: &snap,
            atlas: &mut atlas,
            fonts: &fonts,
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
        let key = atlas
            .resolve('한', false, false)
            .expect("한 resolves to some face");
        assert_ne!(key.face, 0, "Hangul must come from a fallback face");
        assert_ne!(key.id, 0, "and must not be .notdef");

        let cw = 12.0;
        let sidebar_cfg = SidebarConfig::default();
        let mut ctx = FrameCtx {
            device: &device,
            queue: &queue,
            encoder: &mut encoder,
            view: &view,
            viewport: [64.0, 64.0],
            cell: [cw, 24.0],
            top_inset: 0.0,
            left_inset: 0.0,
            right_inset: 0.0,
            format: wgpu::TextureFormat::Rgba8Unorm,
            sidebar: &sidebar_cfg,
            scale_factor: 1.0,
            snapshot: &snap,
            atlas: &mut atlas,
            fonts: &fonts,
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
                    slot: StyleSlot::Regular,
                    synth_bold: bold,
                    synth_italic: italic,
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

    #[test]
    fn frame_ctx_carries_the_panels_right_inset() {
        // R4.7: what a layer is told the panel occupies must be what `grid_size` took
        // away from the terminal. If the two ever disagreed the panel would paint over
        // columns the terminal still believed it owned — the failure this pins is
        // silent, so it is asserted through a real frame rather than by inspection.
        // Needs a GPU.
        let cfg = Config::default();
        let Ok(mut r) = Renderer::new_headless(1200, 800, 1.0, &cfg) else {
            eprintln!("skipping: no GPU");
            return;
        };
        let target = r.device().create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen"),
            size: wgpu::Extent3d {
                width: 1200,
                height: 800,
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

        /// Records what the frame actually handed the layer.
        struct Probe(std::rc::Rc<std::cell::Cell<f32>>);
        impl DrawLayer for Probe {
            fn z_order(&self) -> i32 {
                100
            }
            fn record(&mut self, ctx: &mut FrameCtx<'_>) {
                self.0.set(ctx.right_inset);
            }
        }
        let seen = std::rc::Rc::new(std::cell::Cell::new(f32::NAN));
        r.add_layer(Box::new(Probe(seen.clone())));

        // Closed panel: no band, and the grid is whatever it always was (R4.2).
        r.draw_to(&view, &snap);
        assert_eq!(r.right_inset(), 0.0, "closed panel reserves nothing");
        assert_eq!(seen.get(), r.right_inset());
        let (cols_closed, rows_closed) = r.grid_size();
        r.draw_to(&view, &snap); // settle the instance cache before measuring it
        let settled = r.reshape_count();

        // Opening the panel: the layer sees the live inset, the grid pays for it in
        // columns only, and the resident buffers — built for the wider grid — are
        // dropped even though the snapshot did not change. The width comes from
        // `[ai_monitor].width_px` (default 430) — the chord carries no width of its own.
        r.set_monitor_open(true);
        r.draw_to(&view, &snap);
        assert_eq!(
            r.right_inset(),
            430.0,
            "1.0 scale → logical px are physical"
        );
        assert_eq!(
            seen.get(),
            r.right_inset(),
            "FrameCtx must carry the renderer's live inset"
        );
        let (cols_open, rows_open) = r.grid_size();
        assert!(cols_open < cols_closed, "the band costs the grid columns");
        assert_eq!(rows_open, rows_closed, "and never costs it rows");
        assert_eq!(
            r.reshape_count(),
            settled + 1,
            "a moved inset must invalidate the instance cache (R1.4)"
        );

        // An unchanged width is not a change: a drag calls the setter on every pointer
        // move, which must not re-shape the grid per move.
        let after_open = r.reshape_count();
        r.set_monitor_width(430.0);
        r.draw_to(&view, &snap);
        assert_eq!(
            r.reshape_count(),
            after_open,
            "re-setting the same width must not re-shape"
        );

        // Closing returns exactly the columns the panel took (R4.2/R0.3).
        r.set_monitor_open(false);
        r.draw_to(&view, &snap);
        assert_eq!(seen.get(), 0.0);
        assert_eq!(r.grid_size(), (cols_closed, rows_closed));
    }

    /// R4.3/R4.4/R4.5/R6.2: the window squeezes the band, force-closes it, and gives it
    /// back — all without touching the width the user asked for. Needs a GPU.
    #[test]
    fn a_narrow_window_squeezes_then_force_closes_the_panel_without_losing_its_width() {
        let cfg = Config::default();
        let Ok(mut r) = Renderer::new_headless(1200, 800, 1.0, &cfg) else {
            eprintln!("skipping: no GPU");
            return;
        };
        // Default sidebar 340px, 8px cells → the cap is 1200 − 340 − 20·8 = 700px.
        assert_eq!(r.left_inset(), 340.0);
        let cell_w = r.cell_size().0;
        assert_eq!(
            cell_w, 8.0,
            "the arithmetic below is pinned to this cell width"
        );

        r.set_monitor_open(true);
        assert_eq!(r.right_inset(), 430.0, "the asking width fits the cap");
        assert!(r.grid_size().0 >= MIN_TERMINAL_COLS);

        // Narrow until the cap bites: 900 − 340 − 160 = 400px of allowance.
        r.resize(900, 800);
        assert_eq!(r.right_inset(), 400.0, "R4.3: squeezed to keep 20 columns");
        assert_eq!(
            r.grid_size().0,
            MIN_TERMINAL_COLS,
            "the terminal is left exactly its floor, never less"
        );
        assert_eq!(
            r.monitor_width_logical(),
            430.0,
            "R6.2: the squeeze is a display clamp — the intent is untouched"
        );

        // Narrower still: 780 − 340 − 160 = 280px, under the panel's 300px minimum.
        r.resize(780, 800);
        assert_eq!(r.right_inset(), 0.0, "R4.4: no sliver — the band closes");
        assert!(
            r.monitor_open(),
            "R6.3: a width-forced close is not a user action; the intent stays open"
        );

        // Widening restores it with no keystroke, at the width the user asked for (R4.5).
        r.resize(1200, 800);
        assert_eq!(r.right_inset(), 430.0);

        // The sidebar is the cap's fourth input: widening it re-derives the band too.
        r.set_sidebar_width(440.0);
        assert_eq!(r.right_inset(), 430.0, "1200 − 440 − 160 = 600 still fits");
        r.resize(940, 800);
        assert_eq!(
            r.right_inset(),
            340.0,
            "940 − 440 − 160 leaves 340px, so the wider sidebar squeezes the panel"
        );
        r.set_sidebar_collapsed(true);
        assert_eq!(
            r.right_inset(),
            430.0,
            "collapsing the sidebar hands the allowance back to the panel"
        );
    }

    /// R6.6 — the renderer's half of the launch restore: a persisted `open = true` comes
    /// back at the persisted `width_px`, and comes back only when the app asks. Needs a GPU.
    #[test]
    fn a_persisted_open_panel_restores_at_its_persisted_width() {
        let mut cfg = Config::default();
        cfg.ai_monitor.width_px = 520.0;
        cfg.ai_monitor.open = true; // as the last session left it
        let Ok(mut r) = Renderer::new_headless(1800, 900, 1.0, &cfg) else {
            eprintln!("skipping: no GPU");
            return;
        };

        // The band is NOT derived from the flag. Deriving it here would reserve a strip at
        // startup — and again on every config reload, since `apply_config` re-runs the same
        // derivation — before the overlay has resolved a session to paint into it, which is
        // an empty column of window with live terminal output missing beside it. The flag
        // belongs to the app, which opens the overlay and the band in one act.
        assert_eq!(r.right_inset(), 0.0, "a flag alone reserves nothing");
        assert!(!r.monitor_open());
        let (cols_closed, rows_closed) = r.grid_size();

        // What the app's launch restore does, and all it does: no width of its own, because
        // the persisted one is already the renderer's to find.
        r.set_monitor_open(true);
        assert_eq!(
            r.right_inset(),
            520.0,
            "the panel comes back at the width the user dragged to, not the 430 default"
        );
        assert_eq!(r.monitor_width_logical(), 520.0);
        assert!(r.grid_size().0 < cols_closed, "and the terminal reflows");
        assert_eq!(r.grid_size().1, rows_closed, "columns only, never rows");

        // A restore sets no override — the file is still authoritative, so a hand edit to
        // `width_px` takes effect on the next reload rather than being outranked for the
        // session by a width nobody dragged (R6.5).
        let mut edited = cfg.clone();
        edited.ai_monitor.width_px = 610.0;
        r.apply_config(&edited);
        assert_eq!(r.right_inset(), 610.0);
        assert!(
            r.monitor_open(),
            "and the reload leaves the restored panel open"
        );
    }

    /// R4.6 + D6's override precedence: a live reload for an unrelated key re-derives the
    /// panel's band instead of dropping it, a dragged width outranks the file for the
    /// session, and clearing the override hands authority back. Needs a GPU.
    #[test]
    fn a_config_reload_re_derives_the_panel_band_and_the_override_outranks_the_file() {
        let cfg = Config::default();
        let Ok(mut r) = Renderer::new_headless(1200, 800, 1.0, &cfg) else {
            eprintln!("skipping: no GPU");
            return;
        };
        r.set_monitor_open(true);
        r.set_monitor_width(520.0);
        assert_eq!(r.right_inset(), 520.0);

        // A reload for a field the panel knows nothing about. `set_left_inset`'s trap is
        // that a plain setter's value would be silently dropped here while the panel kept
        // painting; `sync_right_inset` inside `apply_config` is what prevents it.
        let mut other = Config::default();
        other.theme.background = 0x00_11_22;
        r.apply_config(&other);
        assert_eq!(
            r.right_inset(),
            520.0,
            "R4.6: the inset survives the reload"
        );

        // The file disagreeing with the drag does not win — the override is the session's
        // authority (D6), or dragging would be undone by any unrelated save.
        let mut narrower = Config::default();
        narrower.ai_monitor.width_px = 320.0;
        r.apply_config(&narrower);
        assert_eq!(r.right_inset(), 520.0, "the drag outranks the file");

        // …until a genuine human edit clears it, which is what keeps `width_px`
        // hand-editable after the first drag (R6.5).
        r.clear_monitor_override();
        assert_eq!(r.right_inset(), 320.0, "the file is authoritative again");
        assert_eq!(r.monitor_width_logical(), 320.0);
    }

    #[test]
    fn monitor_band_is_capped_by_the_terminals_column_floor() {
        // R4.3: 1200px window, a 340px sidebar and 8px cells leave 1200 − 340 − 160 = 700px
        // for the panel. Asking for less is granted verbatim; asking for more is capped.
        assert_eq!(monitor_band_px(430.0, 1200.0, 8.0, 340.0, 300.0), 430.0);
        assert_eq!(monitor_band_px(760.0, 1200.0, 8.0, 340.0, 300.0), 700.0);
        // The cap is exactly `grid_dims`'s 20-column boundary, not one column off.
        let capped = monitor_band_px(760.0, 1200.0, 8.0, 340.0, 300.0);
        assert_eq!(
            grid_dims(1200.0, 800.0, (8.0, 16.0), 0.0, 340.0, capped).0,
            20
        );
        assert_eq!(
            grid_dims(1200.0, 800.0, (8.0, 16.0), 0.0, 340.0, capped + 1.0).0,
            19,
            "one px past the cap really does cost the twentieth column"
        );

        // R4.4: once the allowance falls under the panel's own minimum the band is zero —
        // never a sliver, and never a negative number handed to `grid_dims`.
        assert_eq!(monitor_band_px(430.0, 780.0, 8.0, 340.0, 300.0), 0.0);
        assert_eq!(monitor_band_px(430.0, 100.0, 8.0, 340.0, 300.0), 0.0);
        // Exactly at the minimum the panel still draws — the rule is `<`, not `<=`.
        assert_eq!(monitor_band_px(430.0, 800.0, 8.0, 340.0, 300.0), 300.0);

        // A closed panel (or a garbage width) reserves nothing, which R4.2 needs to be
        // bit-identical to the pre-panel behaviour.
        assert_eq!(monitor_band_px(0.0, 1200.0, 8.0, 340.0, 300.0), 0.0);
        assert_eq!(monitor_band_px(f32::NAN, 1200.0, 8.0, 340.0, 300.0), 0.0);
    }

    #[test]
    fn monitor_width_clamps_to_the_designs_drag_band() {
        assert_eq!(clamp_monitor_width(430.0), 430.0);
        assert_eq!(clamp_monitor_width(10.0), common::config::MONITOR_WIDTH_MIN);
        assert_eq!(
            clamp_monitor_width(10_000.0),
            common::config::MONITOR_WIDTH_MAX
        );
        // NaN must not reach the inset arithmetic, where `px == self.right_inset` would
        // answer false forever and re-shape the grid on every pointer move.
        assert_eq!(
            clamp_monitor_width(f32::NAN),
            common::config::MONITOR_WIDTH_MIN
        );
    }

    #[test]
    fn font_size_clamps_to_bounds_and_falls_back_on_non_finite() {
        assert_eq!(clamp_font_size(14.0), 14.0);
        assert_eq!(clamp_font_size(8.0), FONT_SIZE_MIN);
        assert_eq!(clamp_font_size(32.0), FONT_SIZE_MAX);
        assert_eq!(clamp_font_size(4.0), FONT_SIZE_MIN);
        assert_eq!(clamp_font_size(60.0), FONT_SIZE_MAX);
        assert_eq!(clamp_font_size(f32::NAN), FONT_SIZE_DEFAULT);
        assert_eq!(clamp_font_size(f32::INFINITY), FONT_SIZE_DEFAULT);
        assert_eq!(clamp_font_size(f32::NEG_INFINITY), FONT_SIZE_DEFAULT);

        let mut cfg = Config::default();
        for pt in [8.0_f32, 13.0, 14.0, 15.0, 20.0, 32.0] {
            for sf in [1.0_f64, 2.0] {
                cfg.font.size_pt = pt;
                assert_eq!(cell_size_px(&cfg, sf), cell_size_for_pt(pt, sf));
            }
        }
    }

    #[test]
    fn set_font_size_updates_metrics_and_atlas_epoch_and_skips_boundary_no_op() {
        // R2.1 & R2.2: set_font_size updates runtime override, cell_size, ppem, and rebuilds
        // Atlas when clamped size changes; returns false without rebuilding Atlas or clearing
        // last_snapshot when clamped size equals effective_font_size.
        let cfg = Config::default();
        let Ok(mut r) = Renderer::new_headless(1200, 800, 2.0, &cfg) else {
            eprintln!("skipping: no GPU");
            return;
        };
        let target = r.device().create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen"),
            size: wgpu::Extent3d {
                width: 1200,
                height: 800,
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
        let mut t =
            term_core::Terminal::new(10, 2, term_core::Color(0xFFFFFF), term_core::Color(0));
        t.advance(b"hello");
        let snap = t.snapshot();

        assert_eq!(r.font_size_logical(), 14.0);
        assert_eq!(r.font_size_override, None);
        assert_eq!(r.cell_size(), cell_size_for_pt(14.0, 2.0));
        assert_eq!(r.ppem, 28.0);

        r.draw_to(&view, &snap);
        let epoch_14 = r.atlas.epoch();
        let reshape_14 = r.reshape_count();
        assert!(r.last_snapshot.is_some());

        // R2.2: setting 14.0 (or NaN, which falls back to 14.0) is a boundary no-op.
        assert!(!r.set_font_size(14.0));
        assert!(!r.set_font_size(f32::NAN));
        assert_eq!(r.atlas.epoch(), epoch_14, "no-op must not rebuild Atlas");
        assert!(
            r.last_snapshot.is_some(),
            "no-op must not clear last_snapshot"
        );
        r.draw_to(&view, &snap);
        assert_eq!(
            r.reshape_count(),
            reshape_14,
            "idle frame after no-op must not re-shape"
        );

        // R2.1: changing to 15.0 updates override, cell_size, ppem, rebuilds Atlas, and clears cache.
        assert!(r.set_font_size(15.0));
        assert_eq!(r.font_size_logical(), 15.0);
        assert_eq!(r.font_size_override, Some(15.0));
        assert_eq!(r.cell_size(), cell_size_for_pt(15.0, 2.0));
        assert_eq!(r.ppem, 30.0);
        let epoch_15 = r.atlas.epoch();
        assert!(
            epoch_15 > epoch_14,
            "set_font_size must rebuild Atlas with incremented epoch"
        );
        assert!(
            r.last_snapshot.is_none(),
            "set_font_size must invalidate last_snapshot"
        );

        // Next frame re-shapes once and then settles (D6).
        r.draw_to(&view, &snap);
        assert_eq!(r.reshape_count(), reshape_14 + 1);
        r.draw_to(&view, &snap);
        assert_eq!(r.reshape_count(), reshape_14 + 1);

        // R2.2 at FONT_SIZE_MIN (8.0): clamping 7.0 -> 8.0 when already at 8.0 is a no-op.
        assert!(r.set_font_size(8.0));
        assert_eq!(r.font_size_logical(), 8.0);
        r.draw_to(&view, &snap);
        let epoch_min = r.atlas.epoch();
        assert!(!r.set_font_size(7.0));
        assert!(!r.set_font_size(8.0));
        assert_eq!(r.atlas.epoch(), epoch_min);
        assert!(r.last_snapshot.is_some());

        // R2.2 at FONT_SIZE_MAX (32.0): clamping 33.0 -> 32.0 when already at 32.0 is a no-op.
        assert!(r.set_font_size(32.0));
        assert_eq!(r.font_size_logical(), 32.0);
        r.draw_to(&view, &snap);
        let epoch_max = r.atlas.epoch();
        assert!(!r.set_font_size(33.0));
        assert!(!r.set_font_size(32.0));
        assert_eq!(r.atlas.epoch(), epoch_max);
        assert!(r.last_snapshot.is_some());
    }

    #[test]
    fn apply_config_preserves_font_override_until_clear_font_override() {
        // R2.3 & R2.4: apply_config updates baseline font_size_pt while preserving an
        // active font_size_override; clear_font_override restores effective size to baseline.
        let cfg = Config::default();
        let Ok(mut r) = Renderer::new_headless(1200, 800, 2.0, &cfg) else {
            eprintln!("skipping: no GPU");
            return;
        };
        assert!(r.set_font_size(18.0));
        assert_eq!(r.font_size_logical(), 18.0);

        // R2.3: reload with cfg.font.size_pt == 14.0 keeps override 18.0 authoritative.
        r.apply_config(&cfg);
        assert_eq!(r.font_size_pt, 14.0);
        assert_eq!(r.font_size_logical(), 18.0);
        assert_eq!(r.cell_size(), cell_size_for_pt(18.0, 2.0));
        assert_eq!(r.ppem, 36.0);

        // R2.4: clear_font_override followed by apply_config restores baseline (16.0).
        let mut cfg16 = Config::default();
        cfg16.font.size_pt = 16.0;
        r.apply_config(&cfg16);
        assert_eq!(r.font_size_pt, 16.0);
        assert_eq!(
            r.font_size_logical(),
            18.0,
            "override still wins before clear"
        );

        r.clear_font_override();
        r.apply_config(&cfg16);
        assert_eq!(r.font_size_override, None);
        assert_eq!(r.font_size_logical(), 16.0);
        assert_eq!(r.cell_size(), cell_size_for_pt(16.0, 2.0));
        assert_eq!(r.ppem, 32.0);
    }

    #[test]
    fn font_size_increase_re_evaluates_right_inset_against_min_terminal_cols() {
        // R2.5: increasing font_size_pt widens cell_size.0 and re-runs sync_right_inset(),
        // squeezing or force-closing monitor_band_px to preserve MIN_TERMINAL_COLS (20)
        // without mutating monitor_open().
        let cfg = Config::default();
        let Ok(mut r) = Renderer::new_headless(950, 800, 1.0, &cfg) else {
            eprintln!("skipping: no GPU");
            return;
        };
        r.set_monitor_open(true);
        // At 14pt @1x: cell_w = 8.0, left_inset = 340.0 -> cap = 950 - 340 - 160 = 450 >= 430.
        assert_eq!(r.right_inset(), 430.0);
        assert!(r.grid_size().0 >= MIN_TERMINAL_COLS);

        // At 20pt @1x: cell_w = 12.0 -> cap = 950 - 340 - 240 = 370 (squeezed, >= 300 min).
        assert!(r.set_font_size(20.0));
        assert_eq!(r.right_inset(), 370.0);
        assert!(r.grid_size().0 >= MIN_TERMINAL_COLS);
        assert!(r.monitor_open());
        assert_eq!(r.monitor_width_logical(), 430.0);

        // At 32pt @1x: cell_w = 19.0 -> cap = 950 - 340 - 380 = 230 < 300 -> collapses to 0.0.
        assert!(r.set_font_size(32.0));
        assert_eq!(r.right_inset(), 0.0);
        assert!(r.grid_size().0 >= MIN_TERMINAL_COLS);
        assert!(
            r.monitor_open(),
            "R2.5: monitor_open intent remains true when squeezed closed by font size"
        );
        assert_eq!(r.monitor_width_logical(), 430.0);

        // Resetting font size back to 14pt restores the 430px band automatically.
        assert!(r.set_font_size(14.0));
        assert_eq!(r.right_inset(), 430.0);
    }

    #[test]
    fn card_atlases_stay_fixed_while_preedit_and_monitor_panel_follow_font_size_epoch() {
        // R2.6: Sidebar CardAtlases (17px title / 12.5px meta) do not rebuild when
        // terminal font size changes, while PreeditLayer and MonitorPanel detect the new
        // ctx.atlas.epoch() and ctx.cell on the next frame and rebuild once.
        let cfg = Config::default();
        let Ok(mut r) = Renderer::new_headless(1200, 800, 1.0, &cfg) else {
            eprintln!("skipping: no GPU");
            return;
        };
        let mut cards = sidebar_text::CardAtlases::new();
        assert!(cards.ensure(r.device(), &r.fonts, r.scale_factor()));
        assert_eq!(cards.builds(), 1);
        let title_epoch = cards.epoch(sidebar_text::CardFace::Title);
        let meta_epoch = cards.epoch(sidebar_text::CardFace::Meta);

        let target = r.device().create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen"),
            size: wgpu::Extent3d {
                width: 1200,
                height: 800,
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
        let mut t =
            term_core::Terminal::new(40, 10, term_core::Color(0xFFFFFF), term_core::Color(0));
        t.advance(b"prompt % ");
        let snap = t.snapshot();

        r.set_monitor_open(true);
        let (preedit_layer, preedit_handle) = preedit_layer::PreeditLayer::new();
        preedit_handle.set_text("한".into());
        let (monitor_layer, monitor_handle) = monitor_panel::MonitorPanel::new();
        monitor_handle.set(monitor_model::MonitorModel {
            visible: true,
            ..monitor_model::MonitorModel::default()
        });

        type SeenCtx = std::rc::Rc<std::cell::Cell<(u64, [f32; 2])>>;
        struct EpochCellProbe(SeenCtx);
        impl DrawLayer for EpochCellProbe {
            fn z_order(&self) -> i32 {
                120
            }
            fn record(&mut self, ctx: &mut FrameCtx<'_>) {
                self.0.set((ctx.atlas.epoch(), ctx.cell));
            }
        }
        let seen: SeenCtx = Default::default();
        r.add_layer(Box::new(preedit_layer));
        r.add_layer(Box::new(monitor_layer));
        r.add_layer(Box::new(EpochCellProbe(seen.clone())));

        r.draw_to(&view, &snap);
        let (epoch_before, cell_before) = seen.get();
        assert_eq!(cell_before, [8.0, 17.0]);

        // Increase terminal font size to 18.0 pt.
        assert!(r.set_font_size(18.0));

        // Sidebar CardAtlases depend only on (FontSet, scale_factor) and do not rebuild.
        assert!(!cards.ensure(r.device(), &r.fonts, r.scale_factor()));
        assert_eq!(cards.builds(), 1);
        assert_eq!(cards.epoch(sidebar_text::CardFace::Title), title_epoch);
        assert_eq!(cards.epoch(sidebar_text::CardFace::Meta), meta_epoch);

        // Next frame hands the new atlas epoch and 18pt cell metrics to PreeditLayer and MonitorPanel.
        r.draw_to(&view, &snap);
        let (epoch_after, cell_after) = seen.get();
        assert!(epoch_after > epoch_before);
        assert_eq!(cell_after, [11.0, 22.0]);
    }
}
