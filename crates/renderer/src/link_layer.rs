//! ⌘-hover link underline — a [`DrawLayer`](crate::DrawLayer) (spec `hyperlinks`,
//! D7, D15, D25).
//!
//! The affordance is transient app state, not grid state: a link is lit only while
//! ⌘ is held over it, and the grid content underneath does not change. So it cannot
//! ride on [`GridSnapshot`] (D11) — the renderer's idle-CPU cache re-presents a
//! content-equal snapshot without re-shaping, and a hover field would both join that
//! equality and be recomputed on every PTY burst.
//!
//! That rules out the [`crate::selection_layer`] shape, which works only because its
//! span *is* a snapshot field. This layer uses the [`crate::preedit_layer`] shape
//! instead: the app writes a [`LinkHandle`], the layer reads it, and the two share an
//! `Rc<RefCell<_>>` (single-threaded UI). The handle is constructed in the app
//! alongside the cursor and preedit layers.
//!
//! What is drawn (D7/R3.1): an **underline only** — no tint, no cursor change —
//! reusing the grid `UNDERLINE` pass's constants verbatim (thickness
//! `(ch*0.06).max(1.0)`, offset `(ch*0.08).max(1.0)` below a `ch*0.8` baseline) and
//! each covered cell's **own** glyph color via [`crate::glyph_color`], so a link
//! inside reversed or dim text underlines in the color its text is actually drawn
//! in. Nothing new is themed, and a link and a shell-emitted `SGR 4` underline are
//! indistinguishable in geometry — which is the point.

use std::cell::RefCell;
use std::rc::Rc;

use bytemuck::{Pod, Zeroable};
use term_core::{GridSnapshot, LinkSpan};
use wgpu::util::DeviceExt;

use crate::{DrawLayer, FrameCtx};

/// z-order: above the glyph pass (0), below the selection tint (4) and the cursor.
///
/// Under the tint rather than over it: a selection is the stronger, more transient
/// statement about the same cells, and the tint is translucent, so the rule still
/// reads through.
pub const LINK_Z: i32 = 3;
const _: () = assert!(LINK_Z > 0 && LINK_Z < crate::selection_layer::SELECTION_Z);

/// Baseline as a fraction of cell height — the same approximation the grid pass and
/// [`crate::preedit_layer`] use, so every underline in the app sits on one baseline.
const ASCENT_RATIO: f32 = 0.8;

/// The app-owned hover state the layer renders. Plain data — no GPU types, no winit.
///
/// Exactly **one** span (D25): `link_at` resolves the cell under the pointer to at
/// most one link, and only that link is ever lit.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LinkModel {
    /// The lit link, or `None` when nothing is hovered with ⌘ held.
    pub span: Option<LinkSpan>,
}

/// A cloneable write handle the app keeps to push the lit span (D15). The layer
/// holds the other end.
#[derive(Clone, Default)]
pub struct LinkHandle(Rc<RefCell<LinkModel>>);

impl LinkHandle {
    /// Light `span` from the next frame on; `None` clears the affordance.
    pub fn set_span(&self, span: Option<LinkSpan>) {
        self.0.borrow_mut().span = span;
    }

    /// The lit span, if any. The app reads this at button-release to decide what to
    /// open — "open what is lit" (D19) is one handle read, not a second resolve.
    pub fn span(&self) -> Option<LinkSpan> {
        self.0.borrow().span.clone()
    }

    /// Whether anything is currently lit (cheaper than cloning the span).
    pub fn is_lit(&self) -> bool {
        self.0.borrow().span.is_some()
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

/// One underlined cell's pixel rect plus the color to draw it in.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug, PartialEq)]
struct RectInstance {
    rect: [f32; 4],
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
@vertex fn vs_main(in: VsIn) -> VsOut {
    let px = in.rect.xy + in.corner * in.rect.zw;
    let clip = vec2<f32>(px.x / u.viewport.x * 2.0 - 1.0, 1.0 - px.y / u.viewport.y * 2.0);
    var o: VsOut;
    o.pos = vec4<f32>(clip, 0.0, 1.0);
    o.color = in.color;
    return o;
}
@fragment fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return in.color;
}
"#;

/// Pure geometry: one underline rect per cell the lit span covers (R3.4).
///
/// Empty when nothing is lit (R3.2). A span reaching into a second row underlines
/// both halves (R3.3) — membership is the span's own linear-range test, so no cell
/// between the two ends is skipped. Split out so it is testable with no GPU.
fn link_instances(
    span: Option<&LinkSpan>,
    snapshot: &GridSnapshot,
    cell: [f32; 2],
    left_inset: f32,
    top_inset: f32,
) -> Vec<RectInstance> {
    let Some(span) = span else {
        return Vec::new();
    };
    let (cw, ch) = (cell[0], cell[1]);
    // The grid's UNDERLINE pass, verbatim — see the module docs.
    let thickness = (ch * 0.06).max(1.0);
    let drop = ch * ASCENT_RATIO + (ch * 0.08).max(1.0);

    let mut out = Vec::new();
    for row in 0..snapshot.rows {
        for col in 0..snapshot.cols {
            if !span.contains(col, row, snapshot.cols) {
                continue;
            }
            let Some(cell_ref) = snapshot.cell(col, row) else {
                continue;
            };
            let [r, g, b] = crate::glyph_color(cell_ref);
            out.push(RectInstance {
                rect: [
                    col as f32 * cw + left_inset,
                    row as f32 * ch + top_inset + drop,
                    cw,
                    thickness,
                ],
                color: [r, g, b, 1.0],
            });
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

/// The ⌘-hover underline draw layer. Build it with [`LinkLayer::new`], which hands
/// back the [`LinkHandle`] the app writes.
#[derive(Default)]
pub struct LinkLayer {
    handle: LinkHandle,
    gpu: Option<Gpu>,
}

impl LinkLayer {
    /// The layer plus the handle the app keeps — the `PreeditLayer::new` shape.
    pub fn new() -> (Self, LinkHandle) {
        let layer = LinkLayer::default();
        let handle = layer.handle.clone();
        (layer, handle)
    }

    fn ensure_gpu(&mut self, ctx: &FrameCtx<'_>) {
        if self.gpu.as_ref().map(|g| g.format) == Some(ctx.format) {
            return;
        }
        let device = ctx.device;
        let quad = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("link-quad"),
            contents: bytemuck::cast_slice(&QUAD),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("link-uniform"),
            contents: bytemuck::bytes_of(&Uniforms {
                viewport: ctx.viewport,
                _pad: [0.0, 0.0],
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let uniform_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("link-u-bgl"),
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
            label: Some("link-u-bg"),
            layout: &uniform_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("link"),
            source: wgpu::ShaderSource::Wgsl(WGSL.into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("link-layout"),
            bind_group_layouts: &[Some(&uniform_bgl)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("link-pipeline"),
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
                    // Opaque rule, like the grid's own underline — it replaces the
                    // pixels it covers rather than tinting them.
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

impl DrawLayer for LinkLayer {
    fn z_order(&self) -> i32 {
        LINK_Z
    }

    fn record(&mut self, ctx: &mut FrameCtx<'_>) {
        let span = self.handle.span();
        let instances = link_instances(
            span.as_ref(),
            ctx.snapshot,
            ctx.cell,
            ctx.left_inset,
            ctx.top_inset,
        );
        // Nothing lit → record no pass at all (R3.2). This is the common case: the
        // handle is empty except while ⌘ is held.
        if instances.is_empty() {
            return;
        }
        self.ensure_gpu(ctx);
        let Some(gpu) = self.gpu.as_ref() else {
            return;
        };
        // Refresh the uniform each frame (the viewport may have resized).
        ctx.queue.write_buffer(
            &gpu.uniform,
            0,
            bytemuck::bytes_of(&Uniforms {
                viewport: ctx.viewport,
                _pad: [0.0, 0.0],
            }),
        );
        let inst = ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("link-inst"),
                contents: bytemuck::cast_slice(&instances),
                usage: wgpu::BufferUsages::VERTEX,
            });
        let mut pass = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("link-pass"),
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
    use term_core::{link_at, Color, Terminal};

    fn snap(cols: u16, rows: u16, text: &str) -> GridSnapshot {
        let mut t = Terminal::new(cols, rows, Color(0xE0E0E0), Color(0x101010));
        t.advance(text.as_bytes());
        t.snapshot()
    }

    #[test]
    fn z_order_is_above_the_glyph_pass_and_below_the_cursor() {
        let (layer, _) = LinkLayer::new();
        let z = layer.z_order();
        assert_eq!(z, LINK_Z);
        assert!(z > 0, "above the glyph pass");
        assert!(z < crate::cursor_layer::CURSOR_Z, "below the cursor");
    }

    #[test]
    fn a_lit_span_renders_one_underline_per_covered_cell() {
        // R3.1: `see ` then a 21-char URL → 21 rects, at the grid's own underline
        // geometry, offset by the insets.
        let s = snap(40, 3, "see https://example.com/a now");
        let span = link_at(&s, 8, 0).expect("resolves");
        let inst = link_instances(Some(&span), &s, [8.0, 16.0], 40.0, 24.0);
        assert_eq!(inst.len(), 21);

        let ch = 16.0_f32;
        let expect_y = 24.0 + ch * ASCENT_RATIO + (ch * 0.08_f32).max(1.0);
        let expect_h = (ch * 0.06_f32).max(1.0);
        let r = inst[0].rect;
        assert_eq!([r[0], r[2], r[3]], [4.0 * 8.0 + 40.0, 8.0, expect_h]);
        assert!((r[1] - expect_y).abs() < 1e-3, "y {} vs {expect_y}", r[1]);
        assert_eq!(inst[20].rect[0], 24.0 * 8.0 + 40.0, "last covered column");
    }

    #[test]
    fn the_underline_uses_the_grid_underline_constants() {
        // R3.1: identical geometry to the grid's SGR-4 pass and to preedit — a link
        // and a shell-emitted underline must be indistinguishable.
        let s = snap(20, 2, "https://a.com/x");
        let span = link_at(&s, 3, 0).expect("resolves");
        for ch in [8.0_f32, 16.0, 30.0] {
            let inst = link_instances(Some(&span), &s, [7.0, ch], 0.0, 0.0);
            assert_eq!(
                inst[0].rect[3],
                (ch * 0.06).max(1.0),
                "thickness at ch={ch}"
            );
            assert!(
                (inst[0].rect[1] - (ch * ASCENT_RATIO + (ch * 0.08).max(1.0))).abs() < 1e-3,
                "offset below the baseline at ch={ch}"
            );
        }
        // A tiny cell still gets a visible 1px rule, never a sub-pixel one.
        let tiny = link_instances(Some(&span), &s, [2.0, 2.0], 0.0, 0.0);
        assert_eq!(tiny[0].rect[3], 1.0);
    }

    #[test]
    fn an_empty_handle_draws_nothing() {
        // R3.2.
        let s = snap(40, 3, "see https://example.com/a now");
        assert!(link_instances(None, &s, [8.0, 16.0], 0.0, 0.0).is_empty());
    }

    #[test]
    fn a_wrapped_span_underlines_both_rows_with_no_gap() {
        // R3.3: the 18-col wrap from the resolver's own tests.
        let s = snap(18, 4, "https://example.com/pull/9 done");
        let span = link_at(&s, 3, 0).expect("resolves");
        assert_eq!(span.start, (0, 0));
        assert_eq!(span.end, (7, 1));
        let inst = link_instances(Some(&span), &s, [8.0, 16.0], 0.0, 0.0);
        // 18 cells on row 0 + 8 on row 1 = the URL's 26 characters.
        assert_eq!(inst.len(), 26);
        let row0 = 0.0 + 16.0 * ASCENT_RATIO + (16.0_f32 * 0.08).max(1.0);
        let row1 = 16.0 + 16.0 * ASCENT_RATIO + (16.0_f32 * 0.08).max(1.0);
        assert_eq!(inst.iter().filter(|i| i.rect[1] == row0).count(), 18);
        assert_eq!(inst.iter().filter(|i| i.rect[1] == row1).count(), 8);
        // No column is skipped on either row.
        let cols_row1: Vec<f32> = inst
            .iter()
            .filter(|i| i.rect[1] == row1)
            .map(|i| i.rect[0])
            .collect();
        assert_eq!(
            cols_row1,
            (0..8).map(|c| c as f32 * 8.0).collect::<Vec<_>>()
        );
    }

    #[test]
    fn each_cell_underlines_in_its_own_glyph_color() {
        // R3.1: the color comes from `glyph_color`, so reverse/dim text underlines
        // in the color it is actually drawn in rather than a fixed theme value.
        let mut t = Terminal::new(40, 2, Color(0xE0E0E0), Color(0x101010));
        // Red foreground for the URL.
        t.advance(b"\x1b[31mhttps://a.com/x\x1b[0m");
        let s = t.snapshot();
        let span = link_at(&s, 3, 0).expect("resolves");
        let inst = link_instances(Some(&span), &s, [8.0, 16.0], 0.0, 0.0);
        let cell = s.cell(0, 0).expect("cell");
        let [r, g, b] = crate::glyph_color(cell);
        assert_eq!(inst[0].color, [r, g, b, 1.0]);
        assert!(r > g && r > b, "the red SGR color reached the underline");
    }

    #[test]
    fn the_handle_carries_at_most_one_span() {
        // R3.5: the model is `Option<LinkSpan>`, and a later write replaces the
        // earlier one rather than accumulating.
        let s = snap(40, 3, "https://a.com/x and https://b.com/y");
        let (_, handle) = LinkLayer::new();
        assert!(!handle.is_lit());
        assert_eq!(handle.span(), None);

        let first = link_at(&s, 3, 0).expect("first resolves");
        handle.set_span(Some(first.clone()));
        assert!(handle.is_lit());
        assert_eq!(handle.span(), Some(first));

        let second = link_at(&s, 22, 0).expect("second resolves");
        handle.set_span(Some(second.clone()));
        assert_eq!(handle.span(), Some(second), "replaced, not accumulated");

        handle.set_span(None);
        assert!(!handle.is_lit(), "cleared");
    }

    #[test]
    fn the_handle_is_shared_between_app_and_layer() {
        // D15: the app writes and the layer reads through one Rc.
        let s = snap(40, 3, "https://a.com/x");
        let (layer, handle) = LinkLayer::new();
        let span = link_at(&s, 3, 0).expect("resolves");
        handle.set_span(Some(span.clone()));
        assert_eq!(
            layer.handle.span(),
            Some(span),
            "the layer sees the app's write"
        );
    }

    #[test]
    fn a_span_off_the_visible_grid_yields_no_instances() {
        // Defensive: a span held from a taller grid must not index outside the
        // snapshot after a resize.
        let s = snap(20, 2, "hi");
        let stale = LinkSpan {
            start: (0, 9),
            end: (5, 9),
            url: "https://a.com/x".into(),
        };
        assert!(link_instances(Some(&stale), &s, [8.0, 16.0], 0.0, 0.0).is_empty());
    }
}
