// sidebar_shapes.wgsl — the shape primitive's shader.
//
// The SDF functions below are a line-for-line transcription of the Rust twins in
// `sidebar_shapes.rs` (`rounded_box_sdf` / `capsule_sdf` / `ring_sdf` /
// `sample_gradient`) — D5. Keep them in sync; the guarded headless GPU test
// cross-checks this shader's coverage against the Rust functions.
//
// Everything is in absolute px: the vertex stage positions the quad from `bbox`
// and hands the fragment the pixel's absolute px position; the fragment evaluates
// the SDF for its `kind`, antialiases with `fwidth`, fills with the gradient, and
// (for a rounded box) paints the outer `border_w` band in `border_color`.

struct U {
    viewport: vec2<f32>,
    _pad: vec2<f32>,
};
@group(0) @binding(0) var<uniform> u: U;

struct VsIn {
    @location(0) corner: vec2<f32>,
    @location(1) bbox: vec4<f32>,
    @location(2) seg: vec4<f32>,
    @location(3) geom: vec2<f32>,
    @location(4) kind_pad: vec2<u32>,
    @location(5) fill0: vec4<f32>,
    @location(6) fill1: vec4<f32>,
    @location(7) fill2: vec4<f32>,
    @location(8) border_color: vec4<f32>,
    @location(9) grad: vec4<f32>,
};

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) @interpolate(flat) bbox: vec4<f32>,
    @location(2) @interpolate(flat) seg: vec4<f32>,
    @location(3) @interpolate(flat) geom: vec2<f32>,
    @location(4) @interpolate(flat) kind: u32,
    @location(5) @interpolate(flat) fill0: vec4<f32>,
    @location(6) @interpolate(flat) fill1: vec4<f32>,
    @location(7) @interpolate(flat) fill2: vec4<f32>,
    @location(8) @interpolate(flat) border_color: vec4<f32>,
    @location(9) @interpolate(flat) grad: vec4<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    let px = in.bbox.xy + in.corner * in.bbox.zw;
    var o: VsOut;
    o.pos = vec4<f32>(
        px.x / u.viewport.x * 2.0 - 1.0,
        1.0 - px.y / u.viewport.y * 2.0,
        0.0,
        1.0,
    );
    o.local = px;
    o.bbox = in.bbox;
    o.seg = in.seg;
    o.geom = in.geom;
    o.kind = in.kind_pad.x;
    o.fill0 = in.fill0;
    o.fill1 = in.fill1;
    o.fill2 = in.fill2;
    o.border_color = in.border_color;
    o.grad = in.grad;
    return o;
}

// --- SDFs: transcribed from sidebar_shapes.rs (D5) ---

fn rounded_box_sdf(p: vec2<f32>, center: vec2<f32>, half: vec2<f32>, r: f32) -> f32 {
    let q = abs(p - center) - (half - vec2<f32>(r, r));
    return length(max(q, vec2<f32>(0.0, 0.0))) + min(max(q.x, q.y), 0.0) - r;
}

fn capsule_sdf(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>, r: f32) -> f32 {
    let pa = p - a;
    let ba = b - a;
    let denom = dot(ba, ba);
    var h = 0.0;
    if (denom > 0.0) {
        h = clamp(dot(pa, ba) / denom, 0.0, 1.0);
    }
    return length(pa - ba * h) - r;
}

fn ring_sdf(p: vec2<f32>, center: vec2<f32>, radius: f32, stroke: f32) -> f32 {
    return abs(length(p - center) - radius) - stroke * 0.5;
}

fn sample_gradient(f0: vec4<f32>, f1: vec4<f32>, f2: vec4<f32>, mid: f32, t: f32) -> vec4<f32> {
    if (t <= mid) {
        return mix(f0, f1, t / max(mid, 1e-6));
    }
    return mix(f1, f2, (t - mid) / max(1.0 - mid, 1e-6));
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let center = vec2<f32>(in.bbox.x + in.bbox.z * 0.5, in.bbox.y + in.bbox.w * 0.5);
    var d: f32;
    if (in.kind == 0u) {
        d = rounded_box_sdf(in.local, center, in.bbox.zw * 0.5, in.geom.x);
    } else if (in.kind == 1u) {
        d = capsule_sdf(in.local, in.seg.xy, in.seg.zw, in.geom.x);
    } else {
        d = ring_sdf(in.local, in.seg.xy, in.geom.x, in.geom.y);
    }
    let aa = max(fwidth(d), 1e-6);
    let cov = clamp(0.5 - d / aa, 0.0, 1.0);
    if (cov <= 0.0) {
        discard;
    }

    // Gradient fill: project the pixel onto the gradient direction over the bbox.
    let rel = in.local - in.bbox.xy;
    let ext = abs(in.grad.x) * in.bbox.z + abs(in.grad.y) * in.bbox.w;
    let t = clamp(dot(rel, in.grad.xy) / max(ext, 1e-6), 0.0, 1.0);
    let fill = sample_gradient(in.fill0, in.fill1, in.fill2, in.grad.z, t);

    // Rounded box with a border: outer `border_w` band is border_color, interior is fill.
    var col = fill;
    if (in.kind == 0u && in.geom.y > 0.0) {
        let interior = clamp(0.5 - (d + in.geom.y) / aa, 0.0, 1.0);
        col = mix(in.border_color, fill, interior);
    }

    let a = col.a * cov;
    return vec4<f32>(col.rgb * a, a); // premultiplied
}
