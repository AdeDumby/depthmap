// Overhang sprites: one instance per cell that receives a protrusion from a
// neighbouring tile. Shares the chunk bind group (colours, pattern array).
// The art lives in pattern layer 32 + tile id; `side` says how it is rotated.

struct Globals {
    viewport: vec2<f32>,
    atlas_cols: u32,
    _pad: u32,
    colors: array<vec4<f32>, 256>,
};

@group(0) @binding(0) var<uniform> g: Globals;
@group(0) @binding(2) var patterns: texture_2d_array<f32>;
@group(0) @binding(3) var pattern_sampler: sampler;

struct Inst {
    @location(0) origin: vec2<f32>,
    @location(1) size: f32,
    @location(2) layer: u32,
    @location(3) side: u32,
    @location(4) color: u32,
    @location(5) opacity: f32,
};

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) layer: u32,
    @location(2) @interpolate(flat) side: u32,
    @location(3) @interpolate(flat) color: u32,
    @location(4) @interpolate(flat) opacity: f32,
    @location(5) @interpolate(flat) ppc: f32,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, inst: Inst) -> VOut {
    let corner = vec2<f32>(f32(vi & 1u), f32(vi >> 1u));
    let px = inst.origin + corner * inst.size;
    var o: VOut;
    o.pos = vec4<f32>(px.x / g.viewport.x * 2.0 - 1.0, 1.0 - px.y / g.viewport.y * 2.0, 0.0, 1.0);
    o.uv = corner;
    o.layer = inst.layer;
    o.side = inst.side;
    o.color = inst.color;
    o.opacity = inst.opacity;
    o.ppc = inst.size;
    return o;
}

// Mirrors `tile::Side::art_uv`: 0 south, 1 north, 2 east, 3 west.
fn art_uv(side: u32, c: vec2<f32>) -> vec2<f32> {
    if (side == 0u) { return c; }
    if (side == 1u) { return vec2<f32>(1.0 - c.x, 1.0 - c.y); }
    if (side == 2u) { return vec2<f32>(1.0 - c.y, c.x); }
    return vec2<f32>(c.y, 1.0 - c.x);
}

@fragment
fn fs_main(v: VOut) -> @location(0) vec4<f32> {
    let uv = clamp(art_uv(v.side, v.uv), vec2<f32>(0.0), vec2<f32>(1.0));
    let lod = clamp(log2(32.0 / max(v.ppc, 0.001)), 0.0, 5.0);
    let t = textureSampleLevel(patterns, pattern_sampler, uv, v.layer, lod).rg;
    let rgb = g.colors[min(v.color, 255u)].rgb;
    return vec4<f32>(rgb * t.x, v.opacity * t.y);
}
