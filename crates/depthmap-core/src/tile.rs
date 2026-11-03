//! Custom tiles: 32x32 images of *shade* (how much of the cell's colour shows)
//! and *alpha*, just like the built-in textures but drawn by the user.
//!
//! A tile may also carry **overhang** art: the picture drawn into each
//! neighbouring cell that is not the same tile, with the tile on one side of
//! it. Forests use it for canopy bumps that spill past their edge. The art is
//! authored with the tile *below* the cell: the bottom of the image touches
//! the tile, the bumps stick up. It is rotated for the other three sides.

use std::fs;
use std::path::Path;

use crate::icons::{self, Image};
use crate::texture::TILE;

/// Pixels of one tile image.
pub const TILE_PIXELS: usize = TILE * TILE;
pub const MAX_TILE_NAME: usize = 40;

/// A 32x32 image with two bytes per pixel: shade, alpha (row-major, y down).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TileArt {
    pub la: Vec<u8>,
}

impl TileArt {
    /// Fully transparent.
    pub fn blank() -> Self {
        Self { la: vec![0; TILE_PIXELS * 2] }
    }

    /// Opaque, every pixel `shade`.
    pub fn solid(shade: u8) -> Self {
        let mut la = Vec::with_capacity(TILE_PIXELS * 2);
        for _ in 0..TILE_PIXELS {
            la.extend([shade, 255]);
        }
        Self { la }
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        (bytes.len() == TILE_PIXELS * 2).then(|| Self { la: bytes.to_vec() })
    }

    #[inline]
    pub fn get(&self, x: usize, y: usize) -> (u8, u8) {
        let i = (y * TILE + x) * 2;
        (self.la[i], self.la[i + 1])
    }

    #[inline]
    pub fn set(&mut self, x: usize, y: usize, shade: u8, alpha: u8) {
        let i = (y * TILE + x) * 2;
        self.la[i] = shade;
        self.la[i + 1] = alpha;
    }

    pub fn is_blank(&self) -> bool {
        self.la.chunks(2).all(|p| p[1] == 0)
    }

    /// Mip chain (32², 16², ... 1²) of two-byte pixels. Shades are averaged
    /// weighted by alpha, so transparent pixels do not darken the edges.
    pub fn mips(&self) -> Vec<Vec<u8>> {
        la_mips(&self.la)
    }

    /// RGBA bytes tinted with `rgb`, composited over `back` (for thumbnails
    /// and previews).
    pub fn tinted_over(&self, rgb: [u8; 3], back: [u8; 3]) -> Vec<u8> {
        let mut out = Vec::with_capacity(TILE_PIXELS * 4);
        for p in self.la.chunks(2) {
            let (s, a) = (p[0] as u32, p[1] as u32);
            for c in 0..3 {
                let fg = rgb[c] as u32 * s / 255;
                out.push(((fg * a + back[c] as u32 * (255 - a) + 127) / 255) as u8);
            }
            out.push(255);
        }
        out
    }
}

/// Mip chain of a two-byte-per-pixel square image whose side is a power of two.
pub fn la_mips(la: &[u8]) -> Vec<Vec<u8>> {
    let mut chain = vec![la.to_vec()];
    let mut size = TILE;
    while size > 1 {
        let prev = chain.last().unwrap();
        let half = size / 2;
        let mut next = Vec::with_capacity(half * half * 2);
        for y in 0..half {
            for x in 0..half {
                let px = |dx: usize, dy: usize| {
                    let i = ((2 * y + dy) * size + 2 * x + dx) * 2;
                    (prev[i] as u32, prev[i + 1] as u32)
                };
                let ps = [px(0, 0), px(1, 0), px(0, 1), px(1, 1)];
                let a_sum: u32 = ps.iter().map(|p| p.1).sum();
                let shade = if a_sum == 0 {
                    0
                } else {
                    (ps.iter().map(|p| p.0 * p.1).sum::<u32>() + a_sum / 2) / a_sum
                };
                next.extend([shade as u8, ((a_sum + 2) / 4) as u8]);
            }
        }
        chain.push(next);
        size = half;
    }
    chain
}

/// A tile in a map or in the library.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CustomTile {
    pub name: String,
    pub art: TileArt,
    /// Drawn into neighbouring cells that are not this tile (see the module docs).
    pub overhang: Option<TileArt>,
}

impl CustomTile {
    pub fn new(name: &str, art: TileArt) -> Self {
        Self { name: name.to_string(), art, overhang: None }
    }
}

// ---- names and files ------------------------------------------------------------------

/// A tile name usable as a file name: letters, digits, spaces and `-_()`.
pub fn valid_tile_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= MAX_TILE_NAME
        && name == name.trim()
        && name.chars().all(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '(' | ')'))
}

/// Turns anything typed or imported into a valid name (`None` if nothing is left).
pub fn clean_tile_name(raw: &str) -> Option<String> {
    let s: String = raw
        .chars()
        .map(|c| if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '(' | ')') { c } else { ' ' })
        .collect();
    let s: String = s.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(MAX_TILE_NAME).collect();
    let s = s.trim().to_string();
    (!s.is_empty()).then_some(s)
}

fn file_for(name: &str, edge: bool) -> String {
    if edge { format!("{name}.edge.png") } else { format!("{name}.png") }
}

/// PNG (grey + alpha, 32x32) of tile art.
pub fn encode_png(art: &TileArt) -> Vec<u8> {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, TILE as u32, TILE as u32);
    enc.set_color(png::ColorType::GrayscaleAlpha);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(&art.la).unwrap();
    out
}

/// Any PNG becomes tile art: resampled to 32x32, colour reduced to its
/// brightness, transparency kept.
pub fn art_from_png(bytes: &[u8]) -> Result<TileArt, String> {
    let img = icons::decode_png(bytes)?;
    Ok(art_from_image(&img))
}

pub fn art_from_image(img: &Image) -> TileArt {
    let mut art = TileArt::blank();
    for ty in 0..TILE {
        let (y0, y1) = span(ty, img.h);
        for tx in 0..TILE {
            let (x0, x1) = span(tx, img.w);
            let (mut l, mut a, mut n) = (0.0f32, 0.0f32, 0.0f32);
            for y in y0..y1 {
                for x in x0..x1 {
                    let p = &img.rgba[(y * img.w + x) * 4..][..4];
                    let alpha = p[3] as f32 / 255.0;
                    let lum = 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32;
                    l += lum * alpha;
                    a += alpha;
                    n += 1.0;
                }
            }
            if a > 0.0 {
                art.set(tx, ty, (l / a).round().clamp(0.0, 255.0) as u8, (a / n * 255.0).round() as u8);
            }
        }
    }
    art
}

/// Source pixel range covering tile pixel `t` (at least one pixel).
fn span(t: usize, src: usize) -> (usize, usize) {
    let a = t * src / TILE;
    (a, ((t + 1) * src).div_ceil(TILE).min(src).max(a + 1))
}

/// Tile names in the library folder (every `Name.png` that is not an edge file).
pub fn list_library(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter_map(|n| n.strip_suffix(".png").map(str::to_string))
        .filter(|n| !n.ends_with(".edge") && valid_tile_name(n))
        .collect();
    names.sort_by_key(|n| n.to_lowercase());
    names
}

/// Reads a tile (and its `.edge.png`, if any) from the library.
pub fn load_library(dir: &Path, name: &str) -> Result<CustomTile, String> {
    if !valid_tile_name(name) {
        return Err(format!("\"{name}\" is not a usable tile name"));
    }
    let read = |edge: bool| fs::read(dir.join(file_for(name, edge)));
    let art = art_from_png(&read(false).map_err(|e| format!("cannot read tile \"{name}\": {e}"))?)?;
    let overhang = match read(true) {
        Ok(b) => Some(art_from_png(&b)?),
        Err(_) => None,
    };
    Ok(CustomTile { name: name.to_string(), art, overhang })
}

/// Writes a tile to the library (replacing a tile of that name).
pub fn save_library(dir: &Path, tile: &CustomTile) -> Result<(), String> {
    if !valid_tile_name(&tile.name) {
        return Err(format!("\"{}\" is not a usable tile name", tile.name));
    }
    fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let write = |edge: bool, art: &TileArt| {
        fs::write(dir.join(file_for(&tile.name, edge)), encode_png(art))
            .map_err(|e| format!("cannot save the tile: {e}"))
    };
    write(false, &tile.art)?;
    match &tile.overhang {
        Some(o) => write(true, o)?,
        None => {
            let _ = fs::remove_file(dir.join(file_for(&tile.name, true)));
        }
    }
    Ok(())
}

// ---- overhang geometry -----------------------------------------------------------------

/// Which side of the cell the overhanging tile is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// The tile is below the cell (the orientation the art is drawn in).
    South = 0,
    North = 1,
    East = 2,
    West = 3,
}

impl Side {
    pub const ALL: [Side; 4] = [Side::South, Side::North, Side::East, Side::West];

    /// Offset from the tile to the cell that receives the overhang.
    pub fn towards(self) -> (i64, i64) {
        match self {
            // The cell the overhang is drawn in lies *opposite* to the side it hangs from.
            Side::South => (0, -1),
            Side::North => (0, 1),
            Side::East => (-1, 0),
            Side::West => (1, 0),
        }
    }

    /// Where in the art a point `(cx, cy)` of the receiving cell (0..1, y down)
    /// lands. The same formulas run in the shader.
    pub fn art_uv(self, cx: f32, cy: f32) -> (f32, f32) {
        match self {
            Side::South => (cx, cy),
            Side::North => (1.0 - cx, 1.0 - cy),
            Side::East => (1.0 - cy, cx),
            Side::West => (cy, 1.0 - cx),
        }
    }
}

/// A small picture of a forest: a block of `interior` with its overhang on
/// every side, tinted with `rgb` over `back`. Used by the tile editor.
/// `cells` is the side of the picture in cells; the forest fills the middle.
pub fn forest_preview(
    interior: &TileArt,
    overhang: Option<&TileArt>,
    rgb: [u8; 3],
    back: [u8; 3],
    cells: usize,
) -> Vec<u8> {
    let side = cells * TILE;
    let mut img = vec![0u8; side * side * 4];
    let tint = |art: &TileArt, x: usize, y: usize| -> (u32, u32, u32, u32) {
        let (s, a) = art.get(x, y);
        let c = |i: usize| rgb[i] as u32 * s as u32 / 255;
        (c(0), c(1), c(2), a as u32)
    };
    let inside = |cx: i64, cy: i64| (1..cells as i64 - 1).contains(&cx) && (1..cells as i64 - 1).contains(&cy);
    for py in 0..side {
        for px in 0..side {
            let (cx, cy) = ((px / TILE) as i64, (py / TILE) as i64);
            let (lx, ly) = (px % TILE, py % TILE);
            let mut out = [back[0] as u32, back[1] as u32, back[2] as u32];
            let mut blend = |(r, g, b, a): (u32, u32, u32, u32)| {
                for (o, v) in out.iter_mut().zip([r, g, b]) {
                    *o = (v * a + *o * (255 - a) + 127) / 255;
                }
            };
            if inside(cx, cy) {
                blend(tint(interior, lx, ly));
            }
            if let Some(over) = overhang {
                for side_of_tile in Side::ALL {
                    let (dx, dy) = side_of_tile.towards();
                    // The tile that hangs over this cell is at the opposite offset.
                    if inside(cx - dx, cy - dy) && !inside(cx, cy) {
                        let (u, v) = side_of_tile.art_uv((lx as f32 + 0.5) / TILE as f32, (ly as f32 + 0.5) / TILE as f32);
                        let (ax, ay) = ((u * TILE as f32) as usize % TILE, (v * TILE as f32) as usize % TILE);
                        blend(tint(over, ax.min(TILE - 1), ay.min(TILE - 1)));
                    }
                }
            }
            let i = (py * side + px) * 4;
            img[i..i + 4].copy_from_slice(&[out[0] as u8, out[1] as u8, out[2] as u8, 255]);
        }
    }
    img
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_validated_and_cleaned() {
        assert!(valid_tile_name("Pine forest (2)"));
        for bad in ["", " x", "a/b", "..", "a.b", "tab\t", &"x".repeat(41)] {
            assert!(!valid_tile_name(bad), "{bad:?}");
        }
        assert_eq!(clean_tile_name("  my/tile..png "), Some("my tile png".into()));
        assert_eq!(clean_tile_name("///"), None);
        assert!(valid_tile_name(&clean_tile_name("Ünïcode ok").unwrap()));
    }

    #[test]
    fn png_round_trip_keeps_shade_and_alpha() {
        let mut art = TileArt::blank();
        art.set(3, 4, 200, 255);
        art.set(31, 31, 10, 128);
        let back = art_from_png(&encode_png(&art)).unwrap();
        assert_eq!(back.get(3, 4), (200, 255));
        assert_eq!(back.get(31, 31).1, 128);
        assert_eq!(back.get(0, 0).1, 0);
    }

    #[test]
    fn importing_a_colour_png_keeps_brightness_and_transparency() {
        // 64x64: left half white, right half black, bottom row transparent.
        let mut rgba = Vec::new();
        for y in 0..64 {
            for x in 0..64 {
                let v = if x < 32 { 255 } else { 0 };
                rgba.extend([v, v, v, if y == 63 { 0 } else { 255 }]);
            }
        }
        let art = art_from_image(&Image { w: 64, h: 64, rgba });
        assert_eq!(art.get(5, 5), (255, 255));
        assert_eq!(art.get(20, 5), (0, 255));
        assert_eq!(art.get(5, 31).1, 128, "half of the source pixels were transparent");
        // A tiny image is stretched to fill the tile.
        let tiny = art_from_image(&Image { w: 2, h: 1, rgba: vec![255, 255, 255, 255, 0, 0, 0, 255] });
        assert_eq!(tiny.get(0, 0), (255, 255));
        assert_eq!(tiny.get(31, 31), (0, 255));
    }

    #[test]
    fn mips_average_by_alpha() {
        let mut art = TileArt::blank();
        art.set(0, 0, 200, 255); // one opaque pixel in a 2x2 block
        let m = art.mips();
        assert_eq!(m.len(), 6);
        assert_eq!(m[1][0], 200, "transparent neighbours do not darken it");
        assert_eq!(m[1][1], 64, "alpha 255/4");
        assert_eq!(m[5].len(), 2);
        assert!(TileArt::solid(9).mips().iter().all(|l| l.chunks(2).all(|p| p == [9, 255])));
    }

    #[test]
    fn library_files_round_trip_with_and_without_overhang() {
        let dir = std::env::temp_dir().join(format!("depthmap-tiles-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let mut t = CustomTile::new("Pine", TileArt::solid(120));
        save_library(&dir, &t).unwrap();
        t.name = "Oak".into();
        t.overhang = Some(TileArt::solid(220));
        save_library(&dir, &t).unwrap();
        assert_eq!(list_library(&dir), vec!["Oak".to_string(), "Pine".to_string()], "edge files are not tiles");
        assert_eq!(load_library(&dir, "Oak").unwrap(), t);
        assert_eq!(load_library(&dir, "Pine").unwrap().overhang, None);
        // Removing the overhang removes its file.
        t.overhang = None;
        save_library(&dir, &t).unwrap();
        assert_eq!(load_library(&dir, "Oak").unwrap().overhang, None);
        assert!(load_library(&dir, "../evil").is_err());
        assert!(load_library(&dir, "Missing").is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn overhang_rotations_are_proper_rotations_with_the_base_on_the_tile_side() {
        // The base of the art is its bottom edge (v = 1). It must land on the cell side that touches the tile.
        // South: tile below the cell -> base at the cell's bottom (cy = 1).
        let base_at = |side: Side, cx: f32, cy: f32| side.art_uv(cx, cy).1 > 0.99;
        assert!(base_at(Side::South, 0.5, 1.0) && !base_at(Side::South, 0.5, 0.0));
        assert!(base_at(Side::North, 0.5, 0.0), "tile above -> base at the top");
        assert!(base_at(Side::East, 1.0, 0.5), "tile to the right -> base at the right");
        assert!(base_at(Side::West, 0.0, 0.5), "tile to the left -> base at the left");
        // The receiving cell for South is the one above the tile.
        assert_eq!(Side::South.towards(), (0, -1));
        // Rotations preserve handedness: u and v of the four corners go round consistently.
        for side in Side::ALL {
            let (u0, v0) = side.art_uv(0.0, 0.0);
            let (u1, v1) = side.art_uv(1.0, 0.0);
            let (u2, v2) = side.art_uv(0.0, 1.0);
            let det = (u1 - u0) * (v2 - v0) - (v1 - v0) * (u2 - u0);
            assert!(det > 0.99, "{side:?} is mirrored (det {det})");
        }
    }

    #[test]
    fn forest_preview_draws_bumps_outside_the_block_only() {
        let mut over = TileArt::blank();
        // Opaque only in the top-left pixel of the art, far from its base.
        over.set(0, 0, 255, 255);
        let px = forest_preview(&TileArt::solid(255), Some(&over), [255, 0, 0], [0, 0, 0], 5);
        let at = |x: usize, y: usize| {
            let i = (y * 5 * TILE + x) * 4;
            [px[i], px[i + 1], px[i + 2]]
        };
        assert_eq!(at(2 * TILE, 2 * TILE), [255, 0, 0], "interior is tinted");
        assert_eq!(at(0, 0), [0, 0, 0], "corners get nothing");
        assert_eq!(at(2 * TILE + 5, 10), [0, 0, 0], "a transparent part of the overhang shows the background");
        // The art's single pixel (top-left of the art) lands at the top-left of the cell above the block.
        assert_eq!(at(2 * TILE, 0), [255, 0, 0]);
    }
}
