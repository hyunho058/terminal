// Textured glyph shader (D3 pass 2). One instance per shaped glyph cluster;
// samples the R8 coverage atlas as alpha and multiplies by the cell foreground.
// Premultiplied-alpha output (blend: One, OneMinusSrcAlpha).

struct Uniforms {
    viewport: vec2<f32>, // framebuffer size in physical px
    _pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var atlas_tex: texture_2d<f32>;
@group(0) @binding(2) var atlas_samp: sampler;

struct VsIn {
    @location(0) corner: vec2<f32>, // unit-quad corner, per-vertex
    @location(1) pos: vec2<f32>,    // glyph top-left in px (top-left origin), per-instance
    @location(2) size: vec2<f32>,   // glyph px size, per-instance
    @location(3) uv0: vec2<f32>,
    @location(4) uv1: vec2<f32>,
    @location(5) color: vec3<f32>,  // linear rgb
};

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec3<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    let px = in.pos + in.corner * in.size;
    let clip = vec2<f32>(
        px.x / u.viewport.x * 2.0 - 1.0,
        1.0 - px.y / u.viewport.y * 2.0,
    );
    var out: VsOut;
    out.pos = vec4<f32>(clip, 0.0, 1.0);
    out.uv = mix(in.uv0, in.uv1, in.corner);
    out.color = in.color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let a = textureSample(atlas_tex, atlas_samp, in.uv).r;
    return vec4<f32>(in.color * a, a); // premultiplied
}
