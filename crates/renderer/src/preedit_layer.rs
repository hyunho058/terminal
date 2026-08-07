//! Inline IME composition (preedit) overlay — a [`DrawLayer`](crate::DrawLayer).
//!
//! While macOS composes a syllable (`ㅎ`+`ㅏ` → `하`) the text does not exist as far
//! as the shell is concerned: it has never been written to the PTY. So it is drawn
//! here, **over** the grid, and never enters `term-core` (D9). Writing it into the
//! grid would put characters the shell never received into the snapshot and the
//! scrollback, and would fight the idle-CPU snapshot cache. Being a layer instead
//! means the composition can update every keystroke while the grid snapshot sits
//! perfectly still — the same property that lets [`crate::cursor_layer`] blink.
//!
//! The app pushes the composition through a [`PreeditHandle`], the same shared-
//! model shape [`crate::tab_bar::TabModelHandle`] uses (D9). The handle also
//! carries the theme `bg`/`fg`: `Renderer::bg_default` is private and
//! [`FrameCtx`] carries no color, so the colors ride with the text rather than
//! widening the frame contract (D14).
//!
//! What is drawn, per D14: an **opaque** fill in the theme background over the
//! cells the composition covers (the underlying cell's own bg would not reliably
//! cover what the shell drew there), the glyphs on top, and an underline below —
//! the standard "not yet committed" IME affordance. The underline reuses the grid
//! path's constants verbatim (`lib.rs`: thickness `(ch*0.06).max(1.0)`, offset
//! `(ch*0.08).max(1.0)` below the baseline) so a composed syllable and a committed
//! one are underlined identically (R7.4).
//!
//! Text layout goes through the shared [`crate::tab_bar`] helpers (R5.4), so the
//! font-fallback and column-width rules exist in exactly one place — Hangul
//! resolves to the fallback face and spends two columns per char here as well.

use std::cell::RefCell;
use std::rc::Rc;

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use crate::tab_bar::{char_cols, layout_line, truncate_cols};
use crate::{DrawLayer, FrameCtx};

/// Draw order: **above** the cursor block (D18/R7.3).
///
/// The cursor draws a 0.5-alpha block at the very cell the composition starts in
/// ([`CURSOR_Z`](crate::cursor_layer::CURSOR_Z) `== 5`). Below it, the block would
/// wash out the first preedit cell and defeat the legibility rationale D14 paints
/// the opaque background for in the first place.
pub const PREEDIT_Z: i32 = 6;

/// Baseline as a fraction of cell height — the same approximation the grid pass
/// and the sibling layers use, so the composition sits on the shell's baseline.
const ASCENT_RATIO: f32 = 0.8;

/// The app-owned composition state the layer renders. Plain data — no GPU types,
/// no app types, no winit.
#[derive(Clone, Debug, PartialEq)]
pub struct PreeditModel {
    /// The in-flight composition (empty == not composing → nothing is drawn).
    pub text: String,
    /// Theme default background, packed `0xRRGGBB` — the opaque fill (D14).
    pub bg: u32,
    /// Theme default foreground, packed `0xRRGGBB` — glyphs and underline.
    pub fg: u32,
}

impl Default for PreeditModel {
    /// Not composing, on a black-on-white default. The app overwrites the colors
    /// from `cfg.theme` at startup and on every config reload (R7.5), so these
    /// only ever apply to a handle nobody has themed.
    fn default() -> Self {
        PreeditModel {
            text: String::new(),
            bg: 0x000000,
            fg: 0xFFFFFF,
        }
    }
}

/// A cloneable write handle the app keeps to push the composition (D9). The layer
/// holds the other end; single-threaded UI, so `Rc<RefCell<_>>`.
#[derive(Clone, Default)]
pub struct PreeditHandle(Rc<RefCell<PreeditModel>>);

impl PreeditHandle {
    /// Replace the composition text drawn next frame (R7.1). An empty string is
    /// "composition finished/cancelled" and draws nothing.
    pub fn set_text(&self, text: String) {
        self.0.borrow_mut().text = text;
    }

    /// Re-theme the fill and glyph colors, packed `0xRRGGBB` (D14/R7.5). Called at
    /// startup and again from the live config-reload path.
    pub fn set_theme(&self, bg: u32, fg: u32) {
        let mut m = self.0.borrow_mut();
        m.bg = bg;
        m.fg = fg;
    }

    /// Snapshot the current model (used internally by the layer).
    fn snapshot(&self) -> PreeditModel {
        self.0.borrow().clone()
    }
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

/// The opaque background fill, in px (top-left origin).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct RectInstance {
    rect: [f32; 4], // x, y, w, h
    color: [f32; 4],
}

/// A textured quad sampling the atlas — a glyph, or the underline rule (which
/// samples the atlas's opaque `solid_entry`, exactly as the grid path does).
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

/// GPU resources built lazily on first draw (needs a device + target format).
struct Gpu {
    format: wgpu::TextureFormat,
    quad: wgpu::Buffer,
    uniform: wgpu::Buffer,
    rect_pipeline: wgpu::RenderPipeline,
    rect_bind: wgpu::BindGroup,
    text_pipeline: wgpu::RenderPipeline,
    /// Kept so [`PreeditLayer::ensure_text_bind`] can rebuild `text_bind` on a
    /// later frame (atlas-swap-bind-invalidation D3 rule 1). Without the layout
    /// stored here, building a bind group after `ensure_gpu` is impossible.
    text_bgl: wgpu::BindGroupLayout,
    text_bind: wgpu::BindGroup,
    /// What `text_bind` was built against: `(surface format, atlas epoch)` (D2).
    ///
    /// It lives **inside** `Gpu`, not on the layer: `ensure_gpu` replaces this
    /// whole struct on a format change, so the key resets to `None` with it and a
    /// cached bind group pointing at a replaced `uniform` buffer is structurally
    /// impossible (D3 rule 2). `generation` and cell size are deliberately absent
    /// — eviction reuses the same texture (a rebuild would be byte-identical) and
    /// the cell size is not in the bind group at all, so keying on either would
    /// churn bind groups at idle (the G1 regression).
    text_bind_key: Option<(wgpu::TextureFormat, u64)>,
    /// How many times `text_bind` has actually been (re)built — see the twin field on
    /// `SidebarTextPipeline`. Key equality alone cannot express "rebuilds = 0 while the
    /// atlas is unchanged": that assertion still passes with the short-circuit deleted
    /// and the bind group rebuilt every frame, the G1 idle-CPU regression this spec
    /// forbids. `#[cfg(test)]`, so the shipped binary is unaffected.
    #[cfg(test)]
    rebuilds: u32,
}

/// The inline-preedit draw layer. Register one via `Renderer::add_layer`.
pub struct PreeditLayer {
    model: PreeditHandle,
    gpu: Option<Gpu>,
}

impl PreeditLayer {
    /// Create the layer and a write handle for the app (D9). Register the layer
    /// with `Renderer::add_layer(Box::new(layer))`; keep the handle and push the
    /// composition into it from the `WindowEvent::Ime` arm.
    pub fn new() -> (PreeditLayer, PreeditHandle) {
        let model = PreeditHandle::default();
        (
            PreeditLayer {
                model: model.clone(),
                gpu: None,
            },
            model,
        )
    }

    /// Build the pipelines, shaders and buffers — gated on the **surface format**
    /// alone, deliberately (atlas-swap-bind-invalidation D3). An atlas swap does
    /// not invalidate any of this; only the atlas bind group, which
    /// [`Self::ensure_text_bind`] owns from the second frame on.
    fn ensure_gpu(&mut self, ctx: &FrameCtx<'_>) {
        if self.gpu.as_ref().map(|g| g.format) == Some(ctx.format) {
            return;
        }
        let device = ctx.device;
        let quad = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("preedit-quad"),
            contents: bytemuck::cast_slice(&QUAD),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("preedit-uniform"),
            contents: bytemuck::bytes_of(&Uniforms {
                viewport: ctx.viewport,
                _pad: [0.0, 0.0],
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        // Fill pipeline (uniform only).
        let rect_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("preedit-rect-bgl"),
            entries: &[uniform_entry(0)],
        });
        let rect_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("preedit-rect-bg"),
            layout: &rect_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });
        let rect_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("preedit-rect"),
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

        // Glyph/underline pipeline (uniform + atlas texture + sampler).
        let text_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("preedit-text-bgl"),
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
        let text_bind = build_text_bind(device, &text_bgl, &uniform, ctx.atlas);
        let text_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("preedit-text"),
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
            text_bgl,
            text_bind,
            text_bind_key: Some((ctx.format, ctx.atlas.epoch())),
            // The eager build just above counts as the first rebuild, so an idle test
            // asserting `rebuilds == 1` covers the whole record path.
            #[cfg(test)]
            rebuilds: 1,
        });
    }

    /// Re-point the atlas bind group at the current atlas when the texture behind
    /// it was replaced (atlas-swap-bind-invalidation R0.3/R3.1–R3.5).
    ///
    /// `Renderer::apply_config` swaps `Atlas` wholesale on every live config
    /// reload, but [`Self::ensure_gpu`] is format-gated and the surface format
    /// never changes at runtime — so without this the preedit sampled the
    /// *replaced* texture with UVs computed against the new one, silently, for the
    /// rest of the session. Keyed on `(format, epoch)` so it fires exactly on a
    /// texture swap and never at idle.
    ///
    /// `text_bind` stays non-`Option` and is simply reassigned: the draw site
    /// consumes it unconditionally, and an `Option` there would force an
    /// `unwrap()`/`expect()` (D3 rule 4, CLAUDE.md's no-`unwrap()` rule).
    fn ensure_text_bind(&mut self, ctx: &FrameCtx<'_>) {
        let key = (ctx.format, ctx.atlas.epoch());
        let Some(gpu) = self.gpu.as_mut() else {
            return; // no pipelines yet — `ensure_gpu` builds the bind group itself
        };
        if gpu.text_bind_key == Some(key) {
            return;
        }
        gpu.text_bind = build_text_bind(ctx.device, &gpu.text_bgl, &gpu.uniform, ctx.atlas);
        gpu.text_bind_key = Some(key);
        #[cfg(test)]
        {
            gpu.rebuilds += 1;
        }
    }

    /// The glyph quads plus the trailing underline rule, in draw order.
    ///
    /// Glyph lookup rides [`layout_line`], i.e. `Atlas::resolve` +
    /// `get_or_insert_styled`, so the composition picks up the fallback chain
    /// (a composing 한글 syllable resolves to the Hangul face, not `.notdef`) and
    /// advances two columns per wide char (R5.4).
    fn text(&self, ctx: &mut FrameCtx<'_>, geom: &PreeditGeom, fg: [f32; 4]) -> Vec<TextInstance> {
        let mut out = Vec::new();
        layout_line(ctx, &geom.text, geom.pen_x, ctx.cell[0], |pen, e| {
            out.push(TextInstance {
                rect: [pen + e.left, geom.baseline - e.top, e.px_w, e.px_h],
                uv0: e.uv0,
                uv1: e.uv1,
                color: fg,
            });
        });
        // Underline: the atlas's opaque 2×2 block sampled at its center, so linear
        // filtering can never pick up a neighboring texel — same trick, and the
        // same geometry constants, as the grid's UNDERLINE pass (R7.4).
        let e = ctx.atlas.solid_entry(ctx.queue);
        let uv = [(e.uv0[0] + e.uv1[0]) * 0.5, (e.uv0[1] + e.uv1[1]) * 0.5];
        out.push(TextInstance {
            rect: geom.underline,
            uv0: uv,
            uv1: uv,
            color: fg,
        });
        out
    }
}

/// Where the composition lands this frame — pure px geometry plus the (possibly
/// clipped) text to draw. Split out of the layer so every boundary rule in D18 is
/// unit-testable without a GPU `FrameCtx`, exactly as `cursor_layer::block_rect` is.
#[derive(Clone, Debug, PartialEq)]
struct PreeditGeom {
    /// The text that actually fits on the row — the clip of D18 ③ already applied.
    text: String,
    /// Opaque background fill: x, y, w, h (D14).
    fill: [f32; 4],
    /// Pen origin for the first glyph (== `fill[0]`; named for the draw path).
    pen_x: f32,
    /// Baseline y the glyphs sit on.
    baseline: f32,
    /// Underline rule: x, y, w, thickness (D14/R7.4).
    underline: [f32; 4],
}

/// Place the composition at the grid cursor, or `None` when it must not draw.
///
/// The boundary rules are D18's, and all three exist because the composition is a
/// transient overlay the shell knows nothing about:
///
/// * **scrolled back** (`at_bottom == false`) — draw nothing. The cursor cell is
///   not on screen; the same rule `cursor_layer` applies (R7.7).
/// * **cursor parked past the last column** (`col == cols`, pending wrap) — clamp
///   the start to `cols - 1` rather than dropping the composition off the row (R7.8).
/// * **longer than the columns left** — clip at the row end. No wrap, no scroll:
///   wrapping would obscure more of the shell's own output, and the shell has no
///   idea these cells are occupied (R7.8).
///
/// Empty text draws nothing at all — not even the fill.
#[allow(clippy::too_many_arguments)]
fn preedit_geom(
    text: &str,
    at_bottom: bool,
    col: u16,
    row: u16,
    cols: u16,
    rows: u16,
    cell: [f32; 2],
    top_inset: f32,
    left_inset: f32,
) -> Option<PreeditGeom> {
    if text.is_empty() || !at_bottom || cols == 0 || row >= rows {
        return None;
    }
    // D18 ②: `col == cols` is the "parked pending wrap" state, not an error.
    let start_col = col.min(cols - 1);
    // D18 ③: a column budget, so the clip counts display width like the grid does.
    let fitted = truncate_cols(text, (cols - start_col) as usize);
    if fitted.is_empty() {
        return None; // one column left and a wide char: nothing fits whole
    }
    let used_cols: usize = fitted.chars().map(char_cols).sum();

    let (cw, ch) = (cell[0], cell[1]);
    // Both insets are physical px and shift the whole grid; a layer draws in
    // absolute px (the grid's own shaders fold them in via uniforms instead), so
    // missing them here lands the composition on top of the tab sidebar (R7.6).
    let x = start_col as f32 * cw + left_inset;
    let y = row as f32 * ch + top_inset;
    let w = used_cols as f32 * cw;
    let baseline = y + ch * ASCENT_RATIO;
    Some(PreeditGeom {
        text: fitted,
        fill: [x, y, w, ch],
        pen_x: x,
        baseline,
        // Identical to the grid's UNDERLINE pass (`lib.rs`), so a composing
        // syllable and a committed one are ruled the same way (R7.4).
        underline: [x, baseline + (ch * 0.08).max(1.0), w, (ch * 0.06).max(1.0)],
    })
}

impl DrawLayer for PreeditLayer {
    fn z_order(&self) -> i32 {
        PREEDIT_Z
    }

    fn record(&mut self, ctx: &mut FrameCtx<'_>) {
        let model = self.model.snapshot();
        let snap = ctx.snapshot;
        let Some(geom) = preedit_geom(
            &model.text,
            snap.at_bottom,
            snap.cursor.col,
            snap.cursor.row,
            snap.cols,
            snap.rows,
            ctx.cell,
            ctx.top_inset,
            ctx.left_inset,
        ) else {
            return; // not composing / scrolled back / nothing fits
        };
        self.ensure_gpu(ctx);
        // Order matters: `ensure_text_bind` needs the `text_bgl` the call above
        // stores, and the draw below needs the re-pointed bind group (D3 rule 3).
        self.ensure_text_bind(ctx);

        // D14: the theme default bg, opaque — NOT the underlying cell's bg. The
        // point is to reliably cover whatever the shell drew in these cells, so the
        // composition stays legible instead of interleaving strokes with it.
        let fill = RectInstance {
            rect: geom.fill,
            color: linear(model.bg, 1.0),
        };
        let texts = self.text(ctx, &geom, linear(model.fg, 1.0));

        let gpu = self.gpu.as_ref().expect("gpu built in ensure_gpu");
        ctx.queue.write_buffer(
            &gpu.uniform,
            0,
            bytemuck::bytes_of(&Uniforms {
                viewport: ctx.viewport,
                _pad: [0.0, 0.0],
            }),
        );
        let fill_buf = ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("preedit-fill"),
                contents: bytemuck::bytes_of(&fill),
                usage: wgpu::BufferUsages::VERTEX,
            });
        let text_buf = ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("preedit-texts"),
                contents: bytemuck::cast_slice(&texts),
                usage: wgpu::BufferUsages::VERTEX,
            });

        let mut pass = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("preedit-pass"),
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
        // Fill first, glyphs + underline over it.
        pass.set_pipeline(&gpu.rect_pipeline);
        pass.set_bind_group(0, &gpu.rect_bind, &[]);
        pass.set_vertex_buffer(0, gpu.quad.slice(..));
        pass.set_vertex_buffer(1, fill_buf.slice(..));
        pass.draw(0..QUAD.len() as u32, 0..1);
        if !texts.is_empty() {
            pass.set_pipeline(&gpu.text_pipeline);
            pass.set_bind_group(0, &gpu.text_bind, &[]);
            pass.set_vertex_buffer(0, gpu.quad.slice(..));
            pass.set_vertex_buffer(1, text_buf.slice(..));
            pass.draw(0..QUAD.len() as u32, 0..texts.len() as u32);
        }
    }
}

/// Packed 0xRRGGBB → linear-light RGBA (matches the sRGB surface, like the cells).
fn linear(packed: u32, a: f32) -> [f32; 4] {
    let [r, g, b] = term_core::Color(packed).to_linear_f32();
    [r, g, b, a]
}

/// The atlas bind group: `{uniform, atlas view, atlas sampler}`.
///
/// One assembly point for both callers — `ensure_gpu`'s eager first build and
/// `ensure_text_bind`'s post-swap rebuild. They must bind the *same* three resources
/// or a swap would silently re-point only some of them, so writing the descriptor
/// twice is a standing invitation for the two copies to drift apart.
fn build_text_bind(
    device: &wgpu::Device,
    bgl: &wgpu::BindGroupLayout,
    uniform: &wgpu::Buffer,
    atlas: &crate::atlas::Atlas,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("preedit-text-bg"),
        layout: bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
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
    })
}

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
        label: Some("preedit-layout"),
        bind_group_layouts: &[Some(bgl)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("preedit-pipeline"),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cursor_layer::CURSOR_Z;

    /// 80×24 grid, live viewport, 10×20 px cells, no insets unless overridden.
    fn geom(text: &str, col: u16, left_inset: f32) -> Option<PreeditGeom> {
        preedit_geom(text, true, col, 3, 80, 24, [10.0, 20.0], 0.0, left_inset)
    }

    /// A GPU device, or `None` on a machine/CI runner without one (the crate's
    /// existing convention: skip rather than fail).
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

    #[test]
    fn handle_roundtrips_text_and_theme() {
        // R7.1/R7.5: the handle is the only channel — text from the Ime arm, colors
        // from the config (FrameCtx carries neither).
        let (_layer, handle) = PreeditLayer::new();
        handle.set_text("한".into());
        assert_eq!(handle.snapshot().text, "한");
        handle.set_theme(0x101010, 0xE6E1CF);
        let m = handle.snapshot();
        assert_eq!((m.bg, m.fg), (0x101010, 0xE6E1CF));
        assert_eq!(m.text, "한", "re-theming must not clear the composition");
    }

    #[test]
    fn preedit_draws_above_the_cursor_block() {
        // R7.3/D18: the cursor's 0.5-alpha block would wash out the first preedit
        // cell if it drew after us.
        // Sorted the way `Renderer::draw_to` orders its layers, so this asserts the
        // actual record order rather than a bare constant comparison.
        let mut zs = [PREEDIT_Z, CURSOR_Z];
        zs.sort_unstable();
        assert_eq!(
            zs,
            [CURSOR_Z, PREEDIT_Z],
            "preedit must record after the cursor block"
        );
    }

    #[test]
    fn geom_places_the_composition_at_the_cursor_cell() {
        // R7.2: fill covers the columns the text spends — `한` is 2 columns wide.
        let g = geom("한", 5, 0.0).expect("draws");
        assert_eq!(g.fill, [50.0, 60.0, 20.0, 20.0]);
        assert_eq!(g.pen_x, 50.0);
        assert_eq!(g.baseline, 60.0 + 20.0 * ASCENT_RATIO);
        assert_eq!(g.text, "한");
    }

    #[test]
    fn geom_shifts_with_left_inset() {
        // R7.6 regression, the shape of `cursor_layer::block_rect_shifts_with_left_inset`:
        // the sidebar shifts the grid right, so every px x must move with it — this
        // exact inset was already gotten wrong once for the cursor.
        let at = |left_inset| geom("하", 5, left_inset).expect("draws");
        assert_eq!(at(0.0).fill[0], 50.0, "no inset → bare column x");
        assert_eq!(
            at(112.0).fill[0],
            162.0,
            "fill shifts by exactly left_inset"
        );
        assert_eq!(at(112.0).pen_x, 162.0, "pen shifts with the fill");
        assert_eq!(at(112.0).underline[0], 162.0, "underline shifts too");
    }

    #[test]
    fn geom_shifts_with_top_inset() {
        let g = preedit_geom("하", true, 0, 2, 80, 24, [10.0, 20.0], 30.0, 0.0).expect("draws");
        assert_eq!(g.fill[1], 2.0 * 20.0 + 30.0);
        assert_eq!(g.baseline, 2.0 * 20.0 + 30.0 + 20.0 * ASCENT_RATIO);
    }

    #[test]
    fn underline_uses_the_grid_constants() {
        // R7.4: thickness (ch*0.06).max(1.0), offset (ch*0.08).max(1.0) below the
        // baseline — the same values `build_glyph_instances` rules committed text
        // with, so composing and committed text look identical.
        // (f32 subtraction of two large-ish px values loses low bits, so the
        // baseline offset is compared with a sub-pixel tolerance, not bit-exactly.)
        let ch = 20.0f32;
        let g = geom("하", 0, 0.0).expect("draws");
        assert_eq!(g.underline[3], (ch * 0.06).max(1.0));
        assert!((g.underline[1] - g.baseline - (ch * 0.08).max(1.0)).abs() < 1e-3);
        // A tiny cell floors both at 1px rather than a sub-pixel rule.
        let tiny = preedit_geom("하", true, 0, 0, 80, 24, [4.0, 5.0], 0.0, 0.0).expect("draws");
        assert_eq!(tiny.underline[3], 1.0);
        assert!((tiny.underline[1] - tiny.baseline - 1.0).abs() < 1e-3);
    }

    #[test]
    fn nothing_is_drawn_while_scrolled_back() {
        // R7.7: same rule as the cursor — the cursor cell isn't on screen.
        assert!(preedit_geom("하", false, 5, 3, 80, 24, [10.0, 20.0], 0.0, 0.0).is_none());
    }

    #[test]
    fn empty_composition_draws_nothing() {
        // R7.8: not even the background fill — an empty preedit is "not composing".
        assert!(geom("", 5, 0.0).is_none());
    }

    #[test]
    fn cursor_parked_pending_wrap_clamps_to_the_last_column() {
        // R7.8/D18 ②: `col == cols` is a legal grid state (parked awaiting wrap);
        // clamp to `cols - 1` instead of running off the row.
        let g = geom("a", 80, 0.0).expect("clamped, still draws");
        assert_eq!(g.fill[0], 79.0 * 10.0, "start clamped to cols-1");
        assert_eq!(g.fill[2], 10.0, "the one remaining column");
        assert_eq!(g.text, "a");
        // Only one column is left there, so a 2-column syllable cannot fit whole —
        // the clamp plus the row-end clip leaves nothing to draw.
        let none = geom("한", 80, 0.0);
        assert!(none.is_none(), "a wide char in one column draws nothing");
    }

    #[test]
    fn overflow_clips_at_the_row_end() {
        // R7.8/D18 ③: no wrap, no scroll — the shell knows nothing about these
        // cells, and wrapping would obscure more of its own output.
        let g = preedit_geom("abcde", true, 77, 0, 80, 24, [10.0, 20.0], 0.0, 0.0).expect("draws");
        assert_eq!(g.text, "abc", "3 columns left → 3 chars");
        assert_eq!(g.fill[2], 30.0, "fill covers only the drawn columns");
        assert_eq!(g.underline[2], 30.0, "underline matches the fill width");
        // Wide chars spend the budget two at a time.
        let g = preedit_geom("한글", true, 77, 0, 80, 24, [10.0, 20.0], 0.0, 0.0).expect("draws");
        assert_eq!(
            g.text, "한",
            "2 of the 3 columns; the second syllable is cut"
        );
        assert_eq!(g.fill[2], 20.0);
    }

    #[test]
    fn recording_a_composition_builds_its_passes() {
        // End-to-end smoke over a real `FrameCtx` (R7.2): the WGSL compiles, the two
        // pipelines build, and a Hangul composition records without panicking —
        // none of which the pure-geometry tests above can see. Recording again with
        // an empty composition must take the early-out and leave the GPU state
        // built (R7.8).
        let (Some((device, queue)), Some(fonts)) = (headless(), crate::FontSet::load().ok()) else {
            eprintln!("skipping: no GPU/font");
            return;
        };
        let mut atlas = crate::atlas::Atlas::new(&device, &fonts, 24.0).expect("atlas");
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen"),
            size: wgpu::Extent3d {
                width: 128,
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
        let t = term_core::Terminal::new(10, 2, term_core::Color(0xFFFFFF), term_core::Color(0));
        let snap = t.snapshot();

        let (mut layer, handle) = PreeditLayer::new();
        handle.set_theme(0x101010, 0xE6E1CF);
        handle.set_text("한글".into());
        let sidebar_cfg = common::config::SidebarConfig::default();
        let mut ctx = FrameCtx {
            device: &device,
            queue: &queue,
            encoder: &mut encoder,
            view: &view,
            viewport: [128.0, 64.0],
            cell: [12.0, 24.0],
            top_inset: 0.0,
            left_inset: 0.0,
            right_inset: 0.0,
            format: wgpu::TextureFormat::Rgba8Unorm,
            sidebar: &sidebar_cfg,
            scale_factor: 1.0,
            snapshot: &snap,
            atlas: &mut atlas,
        };
        layer.record(&mut ctx);
        assert!(layer.gpu.is_some(), "recording must build the pipelines");
        handle.set_text(String::new());
        layer.record(&mut ctx); // empty composition: early-out, no pass
    }

    /// An offscreen render target to record into.
    fn offscreen(device: &wgpu::Device) -> wgpu::TextureView {
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen"),
            size: wgpu::Extent3d {
                width: 128,
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
        target.create_view(&wgpu::TextureViewDescriptor::default())
    }

    /// Record one frame of a composing preedit against `atlas`.
    ///
    /// The 10×2 grid and 12×24 cells are copied from
    /// `recording_a_composition_builds_its_passes` on purpose: `preedit_geom`
    /// returns `None` — and `record` never reaches the bind-group code — on a
    /// smaller grid or a cursor off the viewport (spec R5.3).
    #[allow(clippy::too_many_arguments)]
    fn record_frame(
        layer: &mut PreeditLayer,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        atlas: &mut crate::atlas::Atlas,
        snap: &term_core::GridSnapshot,
        sidebar: &common::config::SidebarConfig,
    ) {
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        let mut ctx = FrameCtx {
            device,
            queue,
            encoder: &mut encoder,
            view,
            viewport: [128.0, 64.0],
            cell: [12.0, 24.0],
            top_inset: 0.0,
            left_inset: 0.0,
            right_inset: 0.0,
            format: wgpu::TextureFormat::Rgba8Unorm,
            sidebar,
            scale_factor: 1.0,
            snapshot: snap,
            atlas,
        };
        layer.record(&mut ctx);
    }

    /// The `text_bind_key` after at least one record, or `None` if the layer never
    /// built its GPU state (which would mean the test took an early-out).
    fn bind_key(layer: &PreeditLayer) -> Option<(wgpu::TextureFormat, u64)> {
        layer.gpu.as_ref().and_then(|g| g.text_bind_key)
    }

    #[test]
    fn atlas_swap_rebuilds_the_text_bind() {
        // R0.3/R3.5: `Renderer::apply_config` replaces the whole `Atlas`, but
        // `ensure_gpu` is format-gated and the surface format never changes at
        // runtime — so before this fix the preedit kept sampling the replaced
        // texture with UVs computed against the new one, for the rest of the
        // session. The key must move with the texture.
        let (Some((device, queue)), Some(fonts)) = (headless(), crate::FontSet::load().ok()) else {
            eprintln!("skipping: no GPU/font");
            return;
        };
        let view = offscreen(&device);
        let t = term_core::Terminal::new(10, 2, term_core::Color(0xFFFFFF), term_core::Color(0));
        let snap = t.snapshot();
        let sidebar_cfg = common::config::SidebarConfig::default();

        let (mut layer, handle) = PreeditLayer::new();
        handle.set_theme(0x101010, 0xE6E1CF);
        handle.set_text("한글".into());

        let mut atlas = crate::atlas::Atlas::new(&device, &fonts, 24.0).expect("atlas");
        record_frame(
            &mut layer,
            &device,
            &queue,
            &view,
            &mut atlas,
            &snap,
            &sidebar_cfg,
        );
        let k1 = bind_key(&layer);
        // Guard against a vacuous pass: if `preedit_geom` had returned `None` the
        // layer would never have built any GPU state and every assertion below
        // would be about nothing.
        assert!(
            k1.is_some(),
            "the first record must reach the bind-group code (early-out taken?)"
        );

        // The swap `apply_config` performs: a brand-new Atlas, hence a new texture.
        let mut swapped = crate::atlas::Atlas::new(&device, &fonts, 24.0).expect("atlas");
        record_frame(
            &mut layer,
            &device,
            &queue,
            &view,
            &mut swapped,
            &snap,
            &sidebar_cfg,
        );
        let k2 = bind_key(&layer);
        assert!(k2.is_some(), "the layer must still hold a bind group");
        assert_ne!(
            k1, k2,
            "an atlas swap must re-point the preedit's atlas bind group"
        );
        // A moved key does NOT imply a moved bind group. Without this, a mutant that
        // advances the key while leaving `text_bind` pointing at the replaced texture
        // passes — and that mutant *is* the production bug.
        assert_eq!(
            layer.gpu.as_ref().map(|g| g.rebuilds),
            Some(2),
            "the swap must actually re-point the bind group, not just re-stamp the key"
        );
    }

    #[test]
    fn idle_frames_do_not_rebuild_the_text_bind() {
        // The other half of the contract (R2.3's shape, G1 idle-CPU constraint):
        // rebuilding every frame would be a regression, so with the same atlas the
        // key must not budge. Scope note (R4.2): this is about *bind groups* only —
        // the per-frame instance `create_buffer_init` calls are existing behaviour.
        let (Some((device, queue)), Some(fonts)) = (headless(), crate::FontSet::load().ok()) else {
            eprintln!("skipping: no GPU/font");
            return;
        };
        let view = offscreen(&device);
        let t = term_core::Terminal::new(10, 2, term_core::Color(0xFFFFFF), term_core::Color(0));
        let snap = t.snapshot();
        let sidebar_cfg = common::config::SidebarConfig::default();

        let (mut layer, handle) = PreeditLayer::new();
        handle.set_theme(0x101010, 0xE6E1CF);
        handle.set_text("한글".into());

        let mut atlas = crate::atlas::Atlas::new(&device, &fonts, 24.0).expect("atlas");
        record_frame(
            &mut layer,
            &device,
            &queue,
            &view,
            &mut atlas,
            &snap,
            &sidebar_cfg,
        );
        let k1 = bind_key(&layer);
        assert!(
            k1.is_some(),
            "the first record must reach the bind-group code (early-out taken?)"
        );

        for frame in 1..=4 {
            record_frame(
                &mut layer,
                &device,
                &queue,
                &view,
                &mut atlas,
                &snap,
                &sidebar_cfg,
            );
            assert_eq!(
                bind_key(&layer),
                k1,
                "idle frame {frame} rebuilt the atlas bind group"
            );
        }
        // Key equality alone does NOT prove this: it still holds when the bind group is
        // rebuilt every frame with the same key. Count the actual rebuilds, or the G1
        // guarantee is asserted in name only.
        assert_eq!(
            layer.gpu.as_ref().map(|g| g.rebuilds),
            Some(1),
            "5 frames on one atlas must build the bind group exactly once"
        );
    }

    #[test]
    fn row_out_of_bounds_draws_nothing() {
        assert!(preedit_geom("하", true, 0, 24, 80, 24, [10.0, 20.0], 0.0, 0.0).is_none());
        // A zero-column grid (mid-resize) must not underflow `cols - 1`.
        assert!(preedit_geom("하", true, 0, 0, 0, 24, [10.0, 20.0], 0.0, 0.0).is_none());
    }
}
