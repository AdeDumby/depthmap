//! Procedurally generated, seamlessly tiling texture patterns.
//!
//! A texture is a `TILE`×`TILE` greyscale "shade" image: 255 leaves the cell's
//! colour untouched, darker values draw mortar, grain, speckles and so on.
//! The renderer multiplies the shade by the cell's colour, which is how every
//! texture can be painted in any colour. The same bytes feed the UI
//! thumbnails, so there is exactly one definition of each pattern.

pub const TILE: usize = 32;
/// Mip levels of a tile: 32, 16, 8, 4, 2, 1.
pub const MIP_LEVELS: usize = 6;
/// Highest usable texture id (id `0` is "empty"). The renderer reserves
/// `MAX_TEXTURES + 1` array layers.
pub const MAX_TEXTURES: u8 = 31;

pub const NAMES: [&str; 14] = [
    "Solid",
    "Flagstone",
    "Cobblestone",
    "Brick",
    "Wood planks",
    "Dirt",
    "Grass",
    "Sand",
    "Water",
    "Hatch",
    "Crosshatch",
    "Checker",
    "Tiles",
    "Dots",
];

/// Number of defined textures; valid ids are `1..=TEXTURE_COUNT`.
pub const TEXTURE_COUNT: u8 = NAMES.len() as u8;

pub fn name(id: u8) -> &'static str {
    if id == 0 {
        return "Empty";
    }
    NAMES.get(id as usize - 1).copied().unwrap_or("?")
}

fn hash(x: i32, y: i32, seed: u32) -> u32 {
    let mut h = (x as u32).wrapping_mul(0x9E37_79B1)
        ^ (y as u32).wrapping_mul(0x85EB_CA77)
        ^ seed.wrapping_mul(0xC2B2_AE3D);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^ (h >> 15)
}

/// Pixel noise in `-range..=range`.
fn noise(x: i32, y: i32, seed: u32, range: i32) -> i32 {
    (hash(x, y, seed) % (2 * range as u32 + 1)) as i32 - range
}

fn wrap(v: i32) -> i32 {
    v.rem_euclid(TILE as i32)
}

fn clamp_u8(v: f32) -> u8 {
    v.clamp(0.0, 255.0).round() as u8
}

fn shade(id: u8, x: i32, y: i32) -> u8 {
    let seed = id as u32;
    match id {
        // Solid
        1 => 255,
        // Flagstone: two staggered rows of slabs with mortar joints.
        2 => {
            let (a, b) = if y / 16 == 0 { (0, 20) } else { (10, 26) };
            if y % 16 == 0 || x == a || x == b {
                150
            } else {
                (232 + noise(x, y, seed, 7)) as u8
            }
        }
        // Cobblestone: rounded stones, every other row shifted by half.
        3 => {
            let off = if (y / 8) % 2 == 1 { 4 } else { 0 };
            let bx = ((x + off) % 8) as f32 - 3.5;
            let by = (y % 8) as f32 - 3.5;
            let d = (bx * bx + by * by).sqrt();
            if d < 3.7 {
                clamp_u8(246.0 - d * 9.0 + noise(x / 2, y / 2, seed, 6) as f32)
            } else {
                135
            }
        }
        // Brick: running bond.
        4 => {
            let off = if (y / 8) % 2 == 1 { 8 } else { 0 };
            let (bx, ly) = ((x + off) % 16, y % 8);
            if ly == 0 || bx == 0 {
                135
            } else {
                let base = 215 + noise(x / 3, y / 3, seed, 10) + noise(x, y, seed + 1, 4);
                (base + if ly == 1 { 14 } else { 0 }).clamp(0, 255) as u8
            }
        }
        // Wood planks: horizontal boards with wavy grain and end joints.
        5 => {
            let (plank, ly) = (y / 8, y % 8);
            if ly == 0 || x == (plank * 13 + 5) % TILE as i32 {
                140
            } else {
                let phase = plank as f32 * 1.7 + ly as f32 * 0.5;
                let wave = (std::f32::consts::TAU * 2.0 * x as f32 / TILE as f32 + phase).sin();
                clamp_u8(228.0 + 14.0 * wave + noise(x, y, seed, 3) as f32)
            }
        }
        // Dirt: blotches and speckles.
        6 => {
            let mut v = 222 + noise(x / 4, y / 4, seed, 12);
            match hash(x, y, seed + 7) % 40 {
                0..=2 => v = 165,
                3..=4 => v = 250,
                _ => {}
            }
            v.clamp(0, 255) as u8
        }
        // Grass: short darker blades on a mottled base.
        7 => {
            let blade = |bx: i32, by: i32| hash(wrap(bx), wrap(by), seed) % 37 == 0;
            if blade(x, y) {
                150
            } else if blade(x, y - 1) {
                175
            } else if blade(x - 1, y + 1) {
                200
            } else {
                (232 + noise(x / 3, y / 3, seed + 3, 8)).clamp(0, 255) as u8
            }
        }
        // Sand: fine grains.
        8 => {
            let mut v = 244 + noise(x / 4, y / 4, seed, 5);
            let h = hash(x, y, seed + 5);
            if h % 17 == 0 {
                v = 205;
            } else if h % 23 == 0 {
                v = 255;
            }
            v.clamp(0, 255) as u8
        }
        // Water: wavy horizontal ripples.
        9 => {
            let ripple = (0..4).any(|k| {
                let wave = (std::f32::consts::TAU * x as f32 / 16.0 + k as f32 * 1.3).sin();
                let row = 4 + 8 * k + (1.6 * wave).round() as i32;
                wrap(y - row) == 0
            });
            if ripple {
                178
            } else {
                (238 + noise(x / 4, y / 4, seed, 6)).clamp(0, 255) as u8
            }
        }
        // Hatch
        10 => {
            if (x + y) % 8 < 2 { 165 } else { 250 }
        }
        // Crosshatch
        11 => {
            if (x + y) % 8 < 2 || (x - y).rem_euclid(8) < 2 {
                165
            } else {
                250
            }
        }
        // Checker
        12 => {
            if (x / 16 + y / 16) % 2 == 0 { 255 } else { 214 }
        }
        // Square tiles with grout and a lit top-left edge.
        13 => {
            let (tx, ty) = (x % 16, y % 16);
            if tx == 0 || ty == 0 {
                150
            } else if tx == 1 || ty == 1 {
                252
            } else {
                238
            }
        }
        // Dots
        14 => {
            let near = |cx: f32, cy: f32| {
                let (dx, dy) = (x as f32 - cx, y as f32 - cy);
                dx * dx + dy * dy < 3.3 * 3.3
            };
            if near(8.0, 8.0) || near(24.0, 8.0) || near(8.0, 24.0) || near(24.0, 24.0) {
                170
            } else {
                250
            }
        }
        _ => 255,
    }
}

/// Level-0 shade image of a texture, `TILE * TILE` bytes, row-major, y down.
/// Unknown ids give a solid tile.
pub fn tile(id: u8) -> Vec<u8> {
    let mut out = Vec::with_capacity(TILE * TILE);
    for y in 0..TILE as i32 {
        for x in 0..TILE as i32 {
            out.push(shade(id, x, y));
        }
    }
    out
}

/// Full mip chain (`MIP_LEVELS` images, 32², 16², … 1²) built by box filtering.
pub fn mips(id: u8) -> Vec<Vec<u8>> {
    let mut chain = vec![tile(id)];
    let mut size = TILE;
    while size > 1 {
        let prev = chain.last().unwrap();
        let half = size / 2;
        let mut next = Vec::with_capacity(half * half);
        for y in 0..half {
            for x in 0..half {
                let s = |dx: usize, dy: usize| prev[(2 * y + dy) * size + 2 * x + dx] as u32;
                next.push(((s(0, 0) + s(1, 0) + s(0, 1) + s(1, 1) + 2) / 4) as u8);
            }
        }
        chain.push(next);
        size = half;
    }
    chain
}

/// A tile tinted with a colour, as RGBA bytes (for thumbnails).
pub fn tinted_rgba(id: u8, rgb: [u8; 3]) -> Vec<u8> {
    let mut out = Vec::with_capacity(TILE * TILE * 4);
    for s in tile(id) {
        for c in rgb {
            out.push(((c as u32 * s as u32 + 127) / 255) as u8);
        }
        out.push(255);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_texture_is_deterministic_full_size_and_named() {
        for id in 1..=TEXTURE_COUNT {
            assert_eq!(tile(id), tile(id));
            assert_eq!(tile(id).len(), TILE * TILE);
            assert!(!name(id).is_empty() && name(id) != "?");
        }
        assert_eq!(name(0), "Empty");
        assert_eq!(name(TEXTURE_COUNT + 1), "?");
        assert!(TEXTURE_COUNT <= MAX_TEXTURES);
    }

    #[test]
    fn solid_is_flat_and_others_have_visible_structure() {
        assert!(tile(1).iter().all(|&s| s == 255));
        for id in 2..=TEXTURE_COUNT {
            let t = tile(id);
            let (lo, hi) = (*t.iter().min().unwrap(), *t.iter().max().unwrap());
            assert!(hi - lo >= 30, "{} is too flat ({lo}..{hi})", name(id));
            assert!(lo >= 100, "{} is too dark to tint well ({lo})", name(id));
        }
    }

    #[test]
    fn textures_are_distinct() {
        for a in 1..=TEXTURE_COUNT {
            for b in a + 1..=TEXTURE_COUNT {
                assert_ne!(tile(a), tile(b), "{} == {}", name(a), name(b));
            }
        }
    }

    #[test]
    fn mip_chain_has_the_right_shape_and_preserves_brightness() {
        let m = mips(4);
        assert_eq!(m.len(), MIP_LEVELS);
        for (i, level) in m.iter().enumerate() {
            let s = TILE >> i;
            assert_eq!(level.len(), s * s);
        }
        let avg = |v: &[u8]| v.iter().map(|&b| b as f64).sum::<f64>() / v.len() as f64;
        assert!((avg(&m[0]) - avg(&m[5])).abs() < 3.0);
    }

    #[test]
    fn tinting_scales_the_colour_by_the_shade() {
        let px = tinted_rgba(1, [10, 100, 200]);
        assert_eq!(&px[0..4], &[10, 100, 200, 255]);
        let hatch = tinted_rgba(10, [255, 255, 255]);
        let t = tile(10);
        assert_eq!(hatch[0], t[0]);
    }
}
