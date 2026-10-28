// Full-screen overlay drawn on top of the chunks: the cell grid, the heavier
// lines marking the parent cells' boundaries, and the brush cursor. Everything
// is analytic, so it costs one cheap fragment pass regardless of map size.

struct Overlay {
    viewport: vec2<f32>,
    ppc: f32,          // pixels per cell at the active depth
    major: f32,        // 1.0 when a parent depth exists
    frac: vec2<f32>,   // position inside the view cell
    cell_mod: vec2<f32>, // view cell index mod SUBDIV
    cursor_min: vec2<f32>, // brush top-left, in cells relative to the view cell
    cursor_size: f32,
    cursor_on: f32,
};

@group(0) @binding(0) var<uniform> o: Overlay;

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    // Cells relative to the view cell; all values stay small, so f32 is exact enough.
    let p = o.frac + (frag.xy - o.viewport * 0.5) / o.ppc;

    let dm = abs(fract(p + 0.5) - 0.5) * o.ppc;
    let minor = (1.0 - smoothstep(0.5, 1.5, min(dm.x, dm.y)))
        * smoothstep(3.0, 10.0, o.ppc) * 0.16;

    let pa = (p + o.cell_mod) / 4.0;
    let dM = abs(fract(pa + 0.5) - 0.5) * 4.0 * o.ppc;
    let major = (1.0 - smoothstep(0.75, 2.0, min(dM.x, dM.y)))
        * o.major * smoothstep(1.0, 4.0, o.ppc) * 0.38;

    let grid = max(minor, major);

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
