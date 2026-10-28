// One instance per visible chunk, across every depth, in a single draw call.
// A chunk is a 32x32 block of material ids living in a slot of the R8Uint
// atlas; the fragment shader looks the id up and maps it through the palette.

struct Globals {
    viewport: vec2<f32>,
    atlas_cols: u32,
    _pad: u32,
    palette: array<vec4<f32>, 32>,
};

@group(0) @binding(0) var<uniform> g: Globals;
@group(0) @binding(1) var atlas: texture_2d<u32>;

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
    return o;
}

@fragment
fn fs_main(v: VOut) -> @location(0) vec4<f32> {
    let ci = vec2<i32>(clamp(floor(v.cell), vec2<f32>(0.0), vec2<f32>(31.0)));
    let base = vec2<i32>(i32(v.slot % g.atlas_cols), i32(v.slot / g.atlas_cols)) * 32;
    let id = textureLoad(atlas, base + ci, 0).r;
    if (id == 0u) {
        discard;
    }
    let c = g.palette[min(id, 31u)];
    return vec4<f32>(c.rgb, c.a * v.opacity);
}
