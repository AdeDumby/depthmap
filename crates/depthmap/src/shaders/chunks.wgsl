// One instance per visible chunk, across every depth, in a single draw call.
// A chunk is a 32x32 block of cells living in a slot of the Rg8Uint atlas:
// r = texture id, g = index into the colour table. The fragment shader tints
// the texture's greyscale shade with that colour.

struct Globals {
    viewport: vec2<f32>,
    atlas_cols: u32,
    _pad: u32,
    colors: array<vec4<f32>, 256>,
};

@group(0) @binding(0) var<uniform> g: Globals;
@group(0) @binding(1) var atlas: texture_2d<u32>;
@group(0) @binding(2) var patterns: texture_2d_array<f32>;
@group(0) @binding(3) var pattern_sampler: sampler;

struct Inst {
    @location(0) origin: vec2<f32>, // top-left, in pixels
    @location(1) size: f32,         // chunk edge, in pixels
    @location(2) slot: u32,         // atlas slot
    @location(3) opacity: f32,
};

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) cell: vec2<f32>,
    @location(1) @interpolate(flat) slot: u32,
    @location(2) @interpolate(flat) opacity: f32,
    @location(3) @interpolate(flat) ppc: f32, // pixels per cell
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, inst: Inst) -> VOut {
    // Triangle strip: (0,0) (1,0) (0,1) (1,1)
    let corner = vec2<f32>(f32(vi & 1u), f32(vi >> 1u));
    let px = inst.origin + corner * inst.size;
    var o: VOut;
    o.pos = vec4<f32>(
        px.x / g.viewport.x * 2.0 - 1.0,
        1.0 - px.y / g.viewport.y * 2.0,
        0.0,
        1.0,
    );
    o.cell = corner * 32.0;
    o.slot = inst.slot;
    o.opacity = inst.opacity;
    o.ppc = inst.size / 32.0;
    return o;
}

@fragment
fn fs_main(v: VOut) -> @location(0) vec4<f32> {
    let ci = vec2<i32>(clamp(floor(v.cell), vec2<f32>(0.0), vec2<f32>(31.0)));
    let base = vec2<i32>(i32(v.slot % g.atlas_cols), i32(v.slot / g.atlas_cols)) * 32;
    let c = textureLoad(atlas, base + ci, 0).rg;
    if (c.x == 0u) {
        discard;
    }
    let rgb = g.colors[min(c.y, 255u)].rgb;
    // Texture tile (32 px) mapped onto one cell. The mip level is chosen
    // from the on-screen cell size so distant/small cells average out
    // instead of shimmering; Repeat addressing makes the tile seamless.
    let f = fract(v.cell);
    let lod = clamp(log2(32.0 / max(v.ppc, 0.001)), 0.0, 5.0);
    let shade = textureSampleLevel(patterns, pattern_sampler, f, min(c.x, 31u), lod).r;
    return vec4<f32>(rgb * shade, v.opacity);
}
