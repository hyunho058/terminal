// Instanced cell-quad shader.
//
// One instance per non-blank cell. A unit quad (corner in 0..1) is placed at the
// cell's grid position and scaled by the cell pixel size, then mapped from
// top-left pixel space into clip space. Colors are pre-converted to linear light
// on the CPU, matching an sRGB surface format.

struct Uniforms {
    viewport: vec2<f32>, // framebuffer size in physical pixels
    cell: vec2<f32>,     // cell size in physical pixels
};

@group(0) @binding(0) var<uniform> u: Uniforms;

struct VsIn {
    @location(0) corner: vec2<f32>,   // unit-quad corner, per-vertex
    @location(1) grid_pos: vec2<f32>, // (col, row), per-instance
    @location(2) color: vec3<f32>,    // linear rgb, per-instance
};

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec3<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    let px = (in.grid_pos + in.corner) * u.cell;
    let clip = vec2<f32>(
        px.x / u.viewport.x * 2.0 - 1.0,
        1.0 - px.y / u.viewport.y * 2.0,
    );
    var out: VsOut;
    out.pos = vec4<f32>(clip, 0.0, 1.0);
    out.color = in.color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(in.color, 1.0);
}
