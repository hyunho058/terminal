//! The sidebar shape primitive — one instanced pipeline drawing antialiased
//! rounded boxes, capsules and rings with per-instance corner radius, border and
//! up-to-3-stop gradient (D3, D4, D5, D9).
//!
//! **SDFs work in absolute px.** Each instance carries an axis-aligned bounding box
//! (`bbox`, for quad positioning) plus the shape's defining geometry in the same px
//! space: a rounded box is its bbox + `radius`; a **capsule is a segment `p0`→`p1` +
//! half-thickness**, so it can be diagonal (the `✳`/`◇` marks and the git-branch
//! elbow need diagonal strokes — a rect-only layout can't express them); a ring is a
//! centre + radius + stroke. The three SDF functions below are the *specification*;
//! `sidebar_shapes.wgsl` (T4b) transcribes them line-for-line and is cross-checked
//! against them by the guarded headless GPU test (D5).
//!
//! Degenerate geometry is normalised in the constructors, never in the shader (D9):
//! they are the only way to build an instance, they clamp radius/border/mid and the
//! gradient direction, and drop a non-finite / non-positive extent to `None`.

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use crate::FrameCtx;

/// `kind` discriminant: an axis-aligned rounded box (bbox + radius + border).
pub const KIND_ROUNDED_BOX: u32 = 0;
/// `kind` discriminant: a capsule — segment `p0`→`p1` with round caps, radius = half-thickness.
pub const KIND_CAPSULE: u32 = 1;
/// `kind` discriminant: a stroked ring — centre + radius + stroke width.
pub const KIND_RING: u32 = 2;

/// Down-right (~135°) unit vector in top-left-origin px space — the mock's gradient
/// direction and the fallback for a degenerate one (D9).
pub const DIR_135: [f32; 2] = [
    std::f32::consts::FRAC_1_SQRT_2,
    std::f32::consts::FRAC_1_SQRT_2,
];

/// A per-instance fill: up to three linear-RGBA stops, a mid position and a
/// direction, packed for the shader. Build via [`Fill::solid`] / [`Fill::gradient`]
/// so the mid/direction are always normalised (D4/D9).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fill {
    pub stops: [[f32; 4]; 3],
    /// `[dir_x, dir_y, mid, _]`.
    pub grad: [f32; 4],
}

impl Fill {
    /// A flat fill of one colour (all three stops equal).
    pub fn solid(c: [f32; 4]) -> Fill {
        Fill {
            stops: [c, c, c],
            grad: [DIR_135[0], DIR_135[1], 0.5, 0.0],
        }
    }

    /// A gradient fill. `mid` is clamped to `[0.05, 0.95]` (so neither interpolation
    /// segment has zero width) and `dir` is normalised, falling back to [`DIR_135`]
    /// when it is zero-length or non-finite (D9 — a divide-by-zero here would NaN the
    /// whole frame).
    pub fn gradient(stops: [[f32; 4]; 3], mid: f32, dir: [f32; 2]) -> Fill {
        let mid = if mid.is_finite() {
            mid.clamp(0.05, 0.95)
        } else {
            0.5
        };
        let dir = normalize_or_default(dir);
        Fill {
            stops,
            grad: [dir[0], dir[1], mid, 0.0],
        }
    }
}

/// One GPU instance. `#[repr(C)]` + `Pod`/`Zeroable`; the attribute layout is asserted
/// by the `const _` offset checks below and consumed by the T4b pipeline.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct SidebarShape {
    /// Quad bounding box `x, y, w, h` in px (top-left origin) — positions the quad.
    pub bbox: [f32; 4],
    /// Capsule endpoints `p0.xy, p1.xy` in px. For box/ring, `.xy` is the centre.
    pub seg: [f32; 4],
    /// `radius` (box corner / ring radius / capsule half-thickness) and `border_w`
    /// (box border / ring stroke). In px.
    pub geom: [f32; 2],
    pub kind: u32,
    // Private on purpose (R8.6/D9): one non-`pub` field makes external struct-literal
    // construction fail to compile (E0451), so the D9 clamps in the constructors are
    // the ONLY way to build an instance. Never read outside this module.
    _pad: u32,
    pub fill0: [f32; 4],
    pub fill1: [f32; 4],
    pub fill2: [f32; 4],
    pub border_color: [f32; 4],
    /// `[dir_x, dir_y, mid, _]`.
    pub grad: [f32; 4],
}

// Layout is load-bearing (the shader binds to these offsets) — assert it (D3 style,
// mirroring lib.rs's uniform offset checks).
const _: () = {
    assert!(core::mem::size_of::<SidebarShape>() == 128);
    assert!(core::mem::offset_of!(SidebarShape, bbox) == 0);
    assert!(core::mem::offset_of!(SidebarShape, seg) == 16);
    assert!(core::mem::offset_of!(SidebarShape, geom) == 32);
    assert!(core::mem::offset_of!(SidebarShape, kind) == 40);
    assert!(core::mem::offset_of!(SidebarShape, fill0) == 48);
    assert!(core::mem::offset_of!(SidebarShape, fill1) == 64);
    assert!(core::mem::offset_of!(SidebarShape, fill2) == 80);
    assert!(core::mem::offset_of!(SidebarShape, border_color) == 96);
    assert!(core::mem::offset_of!(SidebarShape, grad) == 112);
};

impl SidebarShape {
    /// An axis-aligned rounded box. `None` if `w`/`h` is non-positive or non-finite;
    /// `radius` and `border_w` are each clamped to `min(w,h)/2` so the SDF can't
    /// invert (D9).
    pub fn rounded_box(
        rect: [f32; 4],
        radius: f32,
        border_w: f32,
        fill: Fill,
        border_color: [f32; 4],
    ) -> Option<SidebarShape> {
        let [x, y, w, h] = rect;
        if !(w.is_finite() && h.is_finite()) || w <= 0.0 || h <= 0.0 {
            return None;
        }
        let m = w.min(h) * 0.5;
        let radius = clamp_finite(radius, 0.0, m);
        let border_w = clamp_finite(border_w, 0.0, m);
        let cx = x + w * 0.5;
        let cy = y + h * 0.5;
        Some(SidebarShape {
            bbox: [x, y, w, h],
            seg: [cx, cy, cx, cy],
            geom: [radius, border_w],
            kind: KIND_ROUNDED_BOX,
            _pad: 0,
            fill0: fill.stops[0],
            fill1: fill.stops[1],
            fill2: fill.stops[2],
            border_color,
            grad: fill.grad,
        })
    }

    /// A capsule: a `thickness`-wide stroke from `(x1,y1)` to `(x2,y2)` with round
    /// caps, at any orientation. `None` if any coordinate or `thickness` is
    /// non-finite, or `thickness <= 0`. A zero-length segment is a filled dot (valid).
    pub fn capsule(
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        thickness: f32,
        fill: Fill,
    ) -> Option<SidebarShape> {
        if !(x1.is_finite()
            && y1.is_finite()
            && x2.is_finite()
            && y2.is_finite()
            && thickness.is_finite())
            || thickness <= 0.0
        {
            return None;
        }
        let r = thickness * 0.5;
        let minx = x1.min(x2) - r;
        let miny = y1.min(y2) - r;
        let maxx = x1.max(x2) + r;
        let maxy = y1.max(y2) + r;
        Some(SidebarShape {
            bbox: [minx, miny, maxx - minx, maxy - miny],
            seg: [x1, y1, x2, y2],
            geom: [r, 0.0],
            kind: KIND_CAPSULE,
            _pad: 0,
            fill0: fill.stops[0],
            fill1: fill.stops[1],
            fill2: fill.stops[2],
            border_color: [0.0; 4],
            grad: fill.grad,
        })
    }

    /// A stroked ring of `stroke` width centred at `(cx,cy)` with radius `radius`.
    /// `None` if any input is non-finite or `radius`/`stroke` is non-positive.
    pub fn ring(cx: f32, cy: f32, radius: f32, stroke: f32, fill: Fill) -> Option<SidebarShape> {
        if !(cx.is_finite() && cy.is_finite() && radius.is_finite() && stroke.is_finite())
            || radius <= 0.0
            || stroke <= 0.0
        {
            return None;
        }
        let ext = radius + stroke * 0.5;
        Some(SidebarShape {
            bbox: [cx - ext, cy - ext, 2.0 * ext, 2.0 * ext],
            seg: [cx, cy, cx, cy],
            geom: [radius, stroke],
            kind: KIND_RING,
            _pad: 0,
            fill0: fill.stops[0],
            fill1: fill.stops[1],
            fill2: fill.stops[2],
            border_color: [0.0; 4],
            grad: fill.grad,
        })
    }
}

// ---- SDFs (the specification; sidebar_shapes.wgsl transcribes these — D5) ----

/// Signed distance from `p` to an axis-aligned rounded box centred at `center` with
/// half-extents `half` and corner radius `r`. Negative inside, positive outside.
pub fn rounded_box_sdf(p: [f32; 2], center: [f32; 2], half: [f32; 2], r: f32) -> f32 {
    let qx = (p[0] - center[0]).abs() - (half[0] - r);
    let qy = (p[1] - center[1]).abs() - (half[1] - r);
    let ox = qx.max(0.0);
    let oy = qy.max(0.0);
    (ox * ox + oy * oy).sqrt() + qx.max(qy).min(0.0) - r
}

/// Signed distance from `p` to a capsule: the segment `a`→`b` inflated by radius `r`.
pub fn capsule_sdf(p: [f32; 2], a: [f32; 2], b: [f32; 2], r: f32) -> f32 {
    let pa = [p[0] - a[0], p[1] - a[1]];
    let ba = [b[0] - a[0], b[1] - a[1]];
    let denom = ba[0] * ba[0] + ba[1] * ba[1];
    let h = if denom > 0.0 {
        ((pa[0] * ba[0] + pa[1] * ba[1]) / denom).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let dx = pa[0] - ba[0] * h;
    let dy = pa[1] - ba[1] * h;
    (dx * dx + dy * dy).sqrt() - r
}

/// Signed distance from `p` to a stroked ring: circle of `radius` at `center`,
/// stroke `stroke` centred on the circle. Negative on the stroke, positive off it.
pub fn ring_sdf(p: [f32; 2], center: [f32; 2], radius: f32, stroke: f32) -> f32 {
    let dx = p[0] - center[0];
    let dy = p[1] - center[1];
    ((dx * dx + dy * dy).sqrt() - radius).abs() - stroke * 0.5
}

/// Evaluate the 3-stop gradient at parameter `t ∈ [0,1]` (the projection of a pixel
/// onto the gradient direction). `mid` splits `[fill0..fill1]` from `[fill1..fill2]`.
/// The shader computes the same thing (D4).
pub fn sample_gradient(stops: [[f32; 4]; 3], mid: f32, t: f32) -> [f32; 4] {
    let t = t.clamp(0.0, 1.0);
    if t <= mid {
        lerp4(stops[0], stops[1], if mid > 0.0 { t / mid } else { 0.0 })
    } else {
        lerp4(stops[1], stops[2], (t - mid) / (1.0 - mid))
    }
}

// ---- helpers ----

fn clamp_finite(v: f32, lo: f32, hi: f32) -> f32 {
    if v.is_finite() {
        v.clamp(lo, hi)
    } else {
        lo
    }
}

fn normalize_or_default(d: [f32; 2]) -> [f32; 2] {
    let len = (d[0] * d[0] + d[1] * d[1]).sqrt();
    if len.is_finite() && len > 1e-6 {
        [d[0] / len, d[1] / len]
    } else {
        DIR_135
    }
}

fn lerp4(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
        a[3] + (b[3] - a[3]) * t,
    ]
}

// ---- The pipeline (D3, D11): built on the crate-root helpers, never a private copy ----

/// Viewport uniform for the vertex px→clip transform. VERTEX-only (matches
/// `crate::uniform_entry`); the fragment works in interpolated px, needing no uniform.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniforms {
    viewport: [f32; 2],
    _pad: [f32; 2],
}

/// Premultiplied-alpha blend (the shader outputs premultiplied), matching the glyph path.
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

/// One instanced pipeline drawing every sidebar shape kind (D3). A consumer
/// `DrawLayer` owns one `Option<SidebarShapePipeline>` and builds it lazily via
/// [`ensure`] (layers are constructed device-free); instances go through the
/// crate's grow-only resident buffer, never a per-frame allocation.
///
/// [`ensure`]: SidebarShapePipeline::ensure
pub struct SidebarShapePipeline {
    format: wgpu::TextureFormat,
    pipeline: wgpu::RenderPipeline,
    quad: wgpu::Buffer,
    uniform: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    /// Grown across frames by `crate::upload_resident`, never reallocated per frame.
    instances: Option<wgpu::Buffer>,
}

impl SidebarShapePipeline {
    /// Build the pipeline for `format` from a bare device. Public so a compile-smoke
    /// test can construct it without assembling a `FrameCtx` (D5).
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> SidebarShapePipeline {
        let quad = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("sidebar-shape-quad"),
            contents: bytemuck::cast_slice(&crate::QUAD),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sidebar-shape-uniform"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sidebar-shape-bgl"),
            entries: &[crate::uniform_entry(0)],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sidebar-shape-bg"),
            layout: &bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sidebar-shape"),
            source: wgpu::ShaderSource::Wgsl(include_str!("sidebar_shapes.wgsl").into()),
        });
        let attrs = wgpu::vertex_attr_array![
            1 => Float32x4, // bbox
            2 => Float32x4, // seg
            3 => Float32x2, // geom
            4 => Uint32x2,  // kind, _pad
            5 => Float32x4, // fill0
            6 => Float32x4, // fill1
            7 => Float32x4, // fill2
            8 => Float32x4, // border_color
            9 => Float32x4, // grad
        ];
        let pipeline = crate::build_pipeline(
            device,
            &shader,
            &bgl,
            format,
            std::mem::size_of::<SidebarShape>() as u64,
            &attrs,
            PREMULTIPLIED,
        );
        SidebarShapePipeline {
            format,
            pipeline,
            quad,
            uniform,
            bind_group,
            instances: None,
        }
    }

    /// Lazily build (or rebuild on a format change) the pipeline a layer holds in an
    /// `Option` slot, returning it. Mirrors the `ensure_gpu` idiom (D3).
    pub fn ensure<'a>(
        slot: &'a mut Option<SidebarShapePipeline>,
        ctx: &FrameCtx<'_>,
    ) -> &'a mut SidebarShapePipeline {
        let stale = slot
            .as_ref()
            .map(|p| p.format != ctx.format)
            .unwrap_or(true);
        if stale {
            *slot = Some(SidebarShapePipeline::new(ctx.device, ctx.format));
        }
        slot.as_mut().expect("just built")
    }

    /// Draw `shapes` over the current frame (its own `LoadOp::Load` pass). Returns
    /// immediately on an empty slice — no zero-size buffer, no pass (D3/R3.7).
    pub fn draw(&mut self, ctx: &mut FrameCtx<'_>, shapes: &[SidebarShape]) {
        self.draw_raw(
            ctx.device,
            ctx.queue,
            ctx.encoder,
            ctx.view,
            ctx.viewport,
            shapes,
        );
    }

    /// The device-level draw, decoupled from `FrameCtx` so a headless test can drive
    /// it against a bare offscreen view (D5).
    fn draw_raw(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        viewport: [f32; 2],
        shapes: &[SidebarShape],
    ) {
        if shapes.is_empty() {
            return;
        }
        queue.write_buffer(
            &self.uniform,
            0,
            bytemuck::bytes_of(&Uniforms {
                viewport,
                _pad: [0.0, 0.0],
            }),
        );
        let count =
            crate::upload_resident(device, queue, &mut self.instances, shapes, "sidebar-shapes");
        if count == 0 {
            return;
        }
        let instances = self.instances.as_ref().expect("resident buffer present");
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("sidebar-shapes"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
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
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.quad.slice(..));
        pass.set_vertex_buffer(1, instances.slice(..));
        pass.draw(0..6, 0..count);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

    // ---- R5.1: each SDF has the right sign inside / outside / on the edge ----

    #[test]
    fn rounded_box_sdf_sign_and_corner() {
        let center = [50.0, 50.0];
        let half = [40.0, 30.0];
        let r = 10.0;
        // Well inside → negative; well outside → positive.
        assert!(rounded_box_sdf(center, center, half, r) < 0.0);
        assert!(rounded_box_sdf([200.0, 50.0], center, half, r) > 0.0);
        // On the flat right edge (x = center+half): ~0.
        assert!(rounded_box_sdf([90.0, 50.0], center, half, r).abs() < 1e-3);
        // On the rounded corner arc: distance to the arc centre is exactly r → 0.
        // Corner-arc centre = center + (half - r) = (80, 70); a point r out along the
        // diagonal sits on the edge.
        let arc_c = [center[0] + (half[0] - r), center[1] + (half[1] - r)];
        let on = [
            arc_c[0] + r * std::f32::consts::FRAC_1_SQRT_2,
            arc_c[1] + r * std::f32::consts::FRAC_1_SQRT_2,
        ];
        assert!(rounded_box_sdf(on, center, half, r).abs() < 1e-3);
    }

    #[test]
    fn capsule_sdf_sign_diagonal() {
        let a = [10.0, 10.0];
        let b = [40.0, 40.0]; // diagonal segment
        let r = 5.0;
        // On the segment → negative (inside the stroke).
        assert!(capsule_sdf([25.0, 25.0], a, b, r) < 0.0);
        // Perpendicular, > r away → positive.
        assert!(capsule_sdf([25.0, 40.0], a, b, r) > 0.0);
        // Exactly r perpendicular to the midpoint → ~0. Perp dir of (1,1) is (1,-1)/√2.
        let mid = [25.0, 25.0];
        let perp = [
            mid[0] + r * std::f32::consts::FRAC_1_SQRT_2,
            mid[1] - r * std::f32::consts::FRAC_1_SQRT_2,
        ];
        assert!(capsule_sdf(perp, a, b, r).abs() < 1e-3);
        // Beyond the cap end, within r → still inside (round cap).
        assert!(capsule_sdf([8.0, 8.0], a, b, r) < 0.0);
    }

    #[test]
    fn ring_sdf_is_zero_on_stroke_centreline() {
        let c = [50.0, 50.0];
        let radius = 20.0;
        let stroke = 4.0;
        // On the circle centreline → -stroke/2 (deepest inside the stroke).
        assert!((ring_sdf([70.0, 50.0], c, radius, stroke) + stroke * 0.5).abs() < 1e-3);
        // Ring centre is a hole → positive (off the stroke).
        assert!(ring_sdf(c, c, radius, stroke) > 0.0);
        // Far outside → positive.
        assert!(ring_sdf([100.0, 50.0], c, radius, stroke) > 0.0);
        // On the outer edge of the stroke → ~0.
        assert!(ring_sdf([70.0 + stroke * 0.5, 50.0], c, radius, stroke).abs() < 1e-3);
    }

    // ---- R8: degenerate geometry normalised in the constructors ----

    #[test]
    fn rounded_box_clamps_radius_and_border() {
        // R8.1 / R8.2 — radius and border each clamp to min(w,h)/2.
        let s = SidebarShape::rounded_box(
            [0.0, 0.0, 20.0, 10.0],
            50.0,
            40.0,
            Fill::solid(WHITE),
            WHITE,
        )
        .unwrap();
        assert_eq!(s.geom[0], 5.0, "radius clamped to min(w,h)/2");
        assert_eq!(s.geom[1], 5.0, "border clamped to min(w,h)/2");
    }

    #[test]
    fn degenerate_extents_are_dropped() {
        // R8.3 — zero / negative / NaN extent → None.
        assert!(SidebarShape::rounded_box(
            [0.0, 0.0, 0.0, 10.0],
            1.0,
            1.0,
            Fill::solid(WHITE),
            WHITE
        )
        .is_none());
        assert!(SidebarShape::rounded_box(
            [0.0, 0.0, 10.0, -5.0],
            1.0,
            1.0,
            Fill::solid(WHITE),
            WHITE
        )
        .is_none());
        assert!(SidebarShape::rounded_box(
            [0.0, 0.0, f32::NAN, 10.0],
            1.0,
            1.0,
            Fill::solid(WHITE),
            WHITE
        )
        .is_none());
        assert!(SidebarShape::capsule(0.0, 0.0, 10.0, 10.0, 0.0, Fill::solid(WHITE)).is_none());
        assert!(
            SidebarShape::capsule(f32::NAN, 0.0, 10.0, 10.0, 2.0, Fill::solid(WHITE)).is_none()
        );
        assert!(SidebarShape::ring(0.0, 0.0, 0.0, 2.0, Fill::solid(WHITE)).is_none());
        assert!(SidebarShape::ring(0.0, 0.0, 10.0, 0.0, Fill::solid(WHITE)).is_none());
    }

    #[test]
    fn degenerate_gradient_direction_is_replaced() {
        // R8.4 — a zero-length or non-finite direction becomes DIR_135, never NaN.
        let f = Fill::gradient([WHITE; 3], 0.5, [0.0, 0.0]);
        assert_eq!([f.grad[0], f.grad[1]], DIR_135);
        let f2 = Fill::gradient([WHITE; 3], 0.5, [f32::NAN, 1.0]);
        assert!(f2.grad[0].is_finite() && f2.grad[1].is_finite());
    }

    #[test]
    fn degenerate_mid_is_clamped() {
        // R8.5 — mid clamped into [0.05, 0.95].
        assert_eq!(Fill::gradient([WHITE; 3], 0.0, DIR_135).grad[2], 0.05);
        assert_eq!(Fill::gradient([WHITE; 3], 1.0, DIR_135).grad[2], 0.95);
        assert_eq!(Fill::gradient([WHITE; 3], -3.0, DIR_135).grad[2], 0.05);
    }

    // ---- R4: gradient stop interpolation ----

    #[test]
    fn gradient_hits_each_stop() {
        // R4.1 — three distinct stops reached at t = 0, mid, 1.
        let s0 = [1.0, 0.0, 0.0, 1.0];
        let s1 = [0.0, 1.0, 0.0, 1.0];
        let s2 = [0.0, 0.0, 1.0, 1.0];
        let stops = [s0, s1, s2];
        assert_eq!(sample_gradient(stops, 0.55, 0.0), s0);
        assert_eq!(sample_gradient(stops, 0.55, 0.55), s1);
        assert_eq!(sample_gradient(stops, 0.55, 1.0), s2);
    }

    #[test]
    fn gradient_two_and_flat_degenerate() {
        // R4.2 — fill2 == fill1 → a plain 2-stop ramp (no seam at mid).
        let s0 = [1.0, 0.0, 0.0, 1.0];
        let s1 = [0.0, 0.0, 1.0, 1.0];
        let two = [s0, s1, s1];
        assert_eq!(sample_gradient(two, 0.5, 1.0), s1);
        let just_after = sample_gradient(two, 0.5, 0.75);
        assert_eq!(just_after, s1, "past mid with equal upper stops stays flat");
        // R4.3 — all three equal → flat everywhere.
        let flat = [s0, s0, s0];
        assert_eq!(sample_gradient(flat, 0.3, 0.1), s0);
        assert_eq!(sample_gradient(flat, 0.3, 0.9), s0);
    }

    // ---- R3.2: instance layout ----

    #[test]
    fn instance_is_pod_and_kinds_distinct() {
        let b =
            SidebarShape::rounded_box([0.0, 0.0, 10.0, 10.0], 2.0, 1.0, Fill::solid(WHITE), WHITE)
                .unwrap();
        let c = SidebarShape::capsule(0.0, 0.0, 5.0, 5.0, 2.0, Fill::solid(WHITE)).unwrap();
        let r = SidebarShape::ring(5.0, 5.0, 3.0, 1.0, Fill::solid(WHITE)).unwrap();
        assert_eq!(b.kind, KIND_ROUNDED_BOX);
        assert_eq!(c.kind, KIND_CAPSULE);
        assert_eq!(r.kind, KIND_RING);
        // Pod round-trips through bytes.
        let bytes = bytemuck::bytes_of(&b);
        assert_eq!(bytes.len(), 128);
    }

    // ---- R5.2 / R5.3: guarded GPU tests (skip cleanly with no adapter) ----

    #[test]
    fn pipeline_builds_and_wgsl_compiles() {
        // R5.2 — building the pipeline runs sidebar_shapes.wgsl through naga, so a
        // syntax/type error fails here rather than two features later.
        let Some((device, _queue)) = crate::headless() else {
            return; // no adapter → skip
        };
        let _p = SidebarShapePipeline::new(&device, wgpu::TextureFormat::Rgba8Unorm);
    }

    /// Render one opaque-white shape to a `size`×`size` texture (cleared to
    /// transparent) and read back the RGBA bytes. `size*4` must be a multiple of
    /// `COPY_BYTES_PER_ROW_ALIGNMENT` (64*4 = 256 is).
    fn render_readback(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pipe: &mut SidebarShapePipeline,
        shape: SidebarShape,
        size: u32,
    ) -> Vec<u8> {
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("sdf-readback"),
            size: wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let _clear = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        }
        pipe.draw_raw(
            device,
            queue,
            &mut enc,
            &view,
            [size as f32, size as f32],
            &[shape],
        );
        let row = size * 4;
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback-buf"),
            size: (row * size) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(size),
                },
            },
            wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
        );
        queue.submit(std::iter::once(enc.finish()));
        let slice = buf.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        let data = slice.get_mapped_range().expect("buffer mapped");
        data.to_vec()
    }

    /// Expected coverage from a signed distance, assuming `fwidth(d) ≈ 1` (true for a
    /// unit-gradient SDF sampled 1:1 in px) — the same `clamp(0.5 - d, 0, 1)` the
    /// shader computes.
    fn expected_cov(d: f32) -> f32 {
        (0.5 - d).clamp(0.0, 1.0)
    }

    #[test]
    fn coverage_readback_matches_rust_twin() {
        // R5.3 / D5 — the shipped WGSL must AGREE WITH the Rust SDF specification, not
        // merely be monotone. For each kind, render one instance and compare the
        // read-back alpha at chosen pixels (incl. a large-radius corner arc) against
        // the Rust twin evaluated at the same pixel centre, within ±0.15. A
        // transcription bug (halved radius, dropped term, swapped capsule axis) moves
        // the boundary far more than 0.15 and fails this.
        let Some((device, queue)) = crate::headless() else {
            return; // no adapter → skip
        };
        let mut pipe = SidebarShapePipeline::new(&device, wgpu::TextureFormat::Rgba8Unorm);
        let size = 64u32;
        let row = (size * 4) as usize;
        let white = Fill::solid([1.0, 1.0, 1.0, 1.0]);
        let tol = 0.15;

        // (pixel_x, pixel_y, expected coverage from the Rust twin). The fragment
        // samples at the pixel centre, so evaluate the SDF at (x+0.5, y+0.5).
        let check = |data: &[u8], samples: &[(u32, u32, f32)], label: &str| {
            for &(x, y, exp) in samples {
                let got = data[(y as usize) * row + (x as usize) * 4 + 3] as f32 / 255.0;
                assert!(
                    (got - exp).abs() <= tol,
                    "{label}: pixel ({x},{y}) readback cov {got:.3} vs Rust-twin {exp:.3} (tol {tol})"
                );
            }
        };

        // --- rounded box, radius 16 (big corner arc) ---
        let rb =
            SidebarShape::rounded_box([8.0, 8.0, 48.0, 48.0], 16.0, 0.0, white, [0.0; 4]).unwrap();
        let d_rb = |x: u32, y: u32| {
            rounded_box_sdf(
                [x as f32 + 0.5, y as f32 + 0.5],
                [32.0, 32.0],
                [24.0, 24.0],
                16.0,
            )
        };
        let rb_data = render_readback(&device, &queue, &mut pipe, rb, size);
        check(
            &rb_data,
            &[
                (32, 32, expected_cov(d_rb(32, 32))), // deep inside → 1
                (2, 2, expected_cov(d_rb(2, 2))),     // deep outside → 0
                // Corner-arc discriminators: with radius 16 these are OUTSIDE the cut
                // corner; a halved radius would wrongly fill them.
                (10, 10, expected_cov(d_rb(10, 10))),
                (14, 14, expected_cov(d_rb(14, 14))),
            ],
            "rounded_box",
        );
        // Guard the discriminator is meaningful: (10,10) must be ~empty, (14,14) ~full.
        assert!(expected_cov(d_rb(10, 10)) < 0.2 && expected_cov(d_rb(14, 14)) > 0.8);

        // --- ring, radius 16, stroke 6 ---
        let ring = SidebarShape::ring(32.0, 32.0, 16.0, 6.0, white).unwrap();
        let d_ring =
            |x: u32, y: u32| ring_sdf([x as f32 + 0.5, y as f32 + 0.5], [32.0, 32.0], 16.0, 6.0);
        let ring_data = render_readback(&device, &queue, &mut pipe, ring, size);
        check(
            &ring_data,
            &[
                (48, 32, expected_cov(d_ring(48, 32))), // on the stroke → 1
                (32, 32, expected_cov(d_ring(32, 32))), // the hole → 0
                (2, 2, expected_cov(d_ring(2, 2))),     // outside → 0
            ],
            "ring",
        );
        assert!(expected_cov(d_ring(48, 32)) > 0.8 && expected_cov(d_ring(32, 32)) < 0.2);

        // --- capsule, diagonal (16,16)→(48,48), thickness 8 (r=4) ---
        let cap = SidebarShape::capsule(16.0, 16.0, 48.0, 48.0, 8.0, white).unwrap();
        let d_cap = |x: u32, y: u32| {
            capsule_sdf(
                [x as f32 + 0.5, y as f32 + 0.5],
                [16.0, 16.0],
                [48.0, 48.0],
                4.0,
            )
        };
        let cap_data = render_readback(&device, &queue, &mut pipe, cap, size);
        check(
            &cap_data,
            &[
                (32, 32, expected_cov(d_cap(32, 32))), // on the segment → 1
                (48, 16, expected_cov(d_cap(48, 16))), // off perpendicular → 0
                (16, 48, expected_cov(d_cap(16, 48))), // off perpendicular → 0
            ],
            "capsule",
        );
        assert!(expected_cov(d_cap(32, 32)) > 0.8 && expected_cov(d_cap(48, 16)) < 0.2);
    }
}
