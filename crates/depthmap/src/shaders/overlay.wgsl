// Full-screen overlay drawn on top of the chunks: the cell grid of every depth
// around the one being drawn on, and the brush cursor. It never touches the
// map; everything is analytic, so it costs one cheap fragment pass regardless
// of map size.
//
// Each depth level contributes grid lines spaced one of its cells apart. How
// strong a line is depends ONLY on its on-screen spacing, never on which depth
// happens to be "active": lines fade in as cells grow past a few pixels and
// get heavier as they grow large. Because of that the picture is identical on
// both sides of a depth switch, so the grid never pops when you zoom.

struct Overlay {
    viewport: vec2<f32>,
    ppc: f32,            // pixels per cell at the active depth
    depth: f32,          // active depth (levels outside 0..16 do not exist)
    frac: vec2<f32>,     // position inside the view cell
    cell_mod: vec2<f32>, // view cell index mod 1024 (the coarsest level drawn)
    cursor_min: vec2<f32>, // brush top-left, in cells relative to the view cell
    cursor_size: f32,
    cursor_on: f32,
    grid: f32,           // 1.0 = draw grid lines
};

@group(0) @binding(0) var<uniform> o: Overlay;

const MAX_DEPTH: i32 = 16;
const MIN_LEVEL_PX: f32 = 2.5;   // finer than this is invisible
const MAX_LEVEL_PX: f32 = 8192.0; // coarser than this is off screen

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}

// Line strength of one depth level at this pixel. `spacing` is the level's
// cell size in active-depth cells (1/4 for the next finer level, 4 for the
// parent, ...); `pa` is the position in active-depth cells.
fn level_lines(pa: vec2<f32>, spacing: f32) -> f32 {
    let px = o.ppc * spacing; // on-screen cell size of this level
    // Distance in pixels to the nearest line of this level.
    let d = abs(fract(pa / spacing + 0.5) - 0.5) * px;
    let heavy = smoothstep(40.0, 90.0, px);
    let width = mix(1.5, 2.0, heavy);
    let line = 1.0 - smoothstep(mix(0.5, 0.75, heavy), width, min(d.x, d.y));
    let strength = mix(0.16, 0.38, heavy);
    let visible = smoothstep(5.0, 18.0, px);
    return line * strength * visible;
}

@fragment
fn fs_main(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    // Cells relative to the view cell; all values stay small, so f32 is exact enough.
    let p = o.frac + (frag.xy - o.viewport * 0.5) / o.ppc;
    let pa = p + o.cell_mod;

    var grid = 0.0;
    if (o.grid > 0.5) {
        let depth = i32(o.depth + 0.5);
        // o = +2 .. -5: two finer levels down to five coarser ones.
        for (var lvl = 2; lvl >= -5; lvl = lvl - 1) {
            let k = depth + lvl;
            if (k < 0 || k > MAX_DEPTH) {
                continue;
            }
            let spacing = exp2(f32(-2 * lvl));
            let px = o.ppc * spacing;
            if (px < MIN_LEVEL_PX || px > MAX_LEVEL_PX) {
                continue;
            }
            grid = max(grid, level_lines(pa, spacing));
        }
    }

    var cur = 0.0;
    if (o.cursor_on > 0.5) {
        let q = p - o.cursor_min;
        if (q.x >= 0.0 && q.y >= 0.0 && q.x < o.cursor_size && q.y < o.cursor_size) {
            let d = min(min(q.x, o.cursor_size - q.x), min(q.y, o.cursor_size - q.y)) * o.ppc;
            cur = 0.14 + 0.6 * (1.0 - smoothstep(1.0, 2.0, d));
        }
    }

    // Dark grid lines under a white cursor, straight alpha.
    let a = grid + cur * (1.0 - grid);
    let rgb = select(vec3<f32>(0.0), vec3<f32>(1.0), a > 0.0) * (cur * (1.0 - grid) / max(a, 1e-5));
    return vec4<f32>(rgb, a);
}
