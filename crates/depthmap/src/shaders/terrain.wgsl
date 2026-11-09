// The 3D view: every visible face of the heightfield is one instance (a quad
// built from the vertex index). Cells are columns; their tops carry the tile
// pattern tinted with the cell's colour, their sides a darker shade of it.

struct Globals {
    viewport: vec2<f32>,
    ppc: f32,         // pixels per cell on the ground
    depth_range: f32, // cells covered by the depth buffer
    frac: vec2<f32>,  // camera position inside its cell
    yaw: vec2<f32>,   // cos, sin
    elev: vec2<f32>,  // sin, cos of the camera elevation
    lod_ppc: f32,
    _pad: f32,
    background: vec4<f32>,
    colors: array<vec4<f32>, 256>,
};

@group(0) @binding(0) var<uniform> g: Globals;
@group(0) @binding(1) var patterns: texture_2d_array<f32>;
@group(0) @binding(2) var pattern_sampler: sampler;

struct Face {
    @location(0) pos: vec2<f32>,  // cell, relative to the camera's cell
    @location(1) z: vec2<f32>,    // bottom and top, in cells
    @location(2) kind: u32,       // 0 top, 1 south, 2 east, 3 north, 4 west
    @location(3) tex: u32,
    @location(4) color: u32,
    @location(5) shade: f32,
};

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) tex: u32,
    @location(2) @interpolate(flat) color: u32,
    @location(3) @interpolate(flat) shade: f32,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, f: Face) -> VOut {
    let a = f32(vi & 1u);
    let b = f32(vi >> 1u);
    var p = vec3<f32>(f.pos, f.z.y);
    var n = vec3<f32>(0.0, 0.0, 1.0);
    let z = mix(f.z.y, f.z.x, b);
    switch f.kind {
        case 0u: { p = vec3<f32>(f.pos.x + a, f.pos.y + b, f.z.y); }
        case 1u: { p = vec3<f32>(f.pos.x + a, f.pos.y + 1.0, z); n = vec3<f32>(0.0, 1.0, 0.0); }
        case 2u: { p = vec3<f32>(f.pos.x + 1.0, f.pos.y + a, z); n = vec3<f32>(1.0, 0.0, 0.0); }
        case 3u: { p = vec3<f32>(f.pos.x + a, f.pos.y, z); n = vec3<f32>(0.0, -1.0, 0.0); }
        default: { p = vec3<f32>(f.pos.x, f.pos.y + a, z); n = vec3<f32>(-1.0, 0.0, 0.0); }
    }
    // Camera position inside its cell, then turn about the camera.
    let rel = p.xy - g.frac;
    let u = rel.x * g.yaw.x - rel.y * g.yaw.y;
    let v = rel.x * g.yaw.y + rel.y * g.yaw.x;
    let sx = u * g.ppc;
    let sy = (v * g.elev.x - p.z * g.elev.y) * g.ppc;
    let d = v * g.elev.y + p.z * g.elev.x; // larger = nearer the viewer
    var o: VOut;
    o.pos = vec4<f32>(sx / (g.viewport.x * 0.5), -sy / (g.viewport.y * 0.5), clamp(0.5 - d / (2.0 * g.depth_range), 0.0, 1.0), 1.0);
    o.uv = vec2<f32>(a, b * select(f.z.y - f.z.x, 1.0, f.kind == 0u));
    o.tex = f.tex;
    o.color = f.color;
    // Sides are lit by the turned normal; the top keeps its own shade.
    var shade = f.shade;
    if (f.kind != 0u) {
        let nu = n.x * g.yaw.x - n.y * g.yaw.y;
        let nv = n.x * g.yaw.y + n.y * g.yaw.x;
        shade = 0.5 + 0.4 * clamp(dot(vec2<f32>(nu, nv), normalize(vec2<f32>(-0.6, 0.8))) * 0.5 + 0.5, 0.0, 1.0);
    }
    o.shade = shade;
    return o;
}

@fragment
fn fs_main(v: VOut) -> @location(0) vec4<f32> {
    let lod = clamp(log2(32.0 / max(g.lod_ppc, 0.001)), 0.0, 5.0);
    let t = textureSampleLevel(patterns, pattern_sampler, fract(v.uv), min(v.tex, 31u), lod).rg;
    let rgb = g.colors[min(v.color, 255u)].rgb * t.x * v.shade;
    return vec4<f32>(mix(g.background.rgb, clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)), t.y), 1.0);
}
