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

/// A rectangle of pixels cut or copied from a tile (shade, alpha pairs).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Clip {
    pub w: usize,
    pub h: usize,
    pub la: Vec<u8>,
}

impl Clip {
    pub fn flip_h(&self) -> Clip {
        let mut la = Vec::with_capacity(self.la.len());
        for y in 0..self.h {
            for x in (0..self.w).rev() {
                let i = (y * self.w + x) * 2;
                la.extend([self.la[i], self.la[i + 1]]);
            }
        }
        Clip { w: self.w, h: self.h, la }
    }

    pub fn flip_v(&self) -> Clip {
        let mut la = Vec::with_capacity(self.la.len());
        for y in (0..self.h).rev() {
            la.extend_from_slice(&self.la[y * self.w * 2..(y + 1) * self.w * 2]);
        }
        Clip { w: self.w, h: self.h, la }
    }

    /// Turned a quarter clockwise.
    pub fn rotate_cw(&self) -> Clip {
        let (w, h) = (self.h, self.w);
        let mut la = vec![0; self.la.len()];
        for y in 0..self.h {
            for x in 0..self.w {
                let (nx, ny) = (self.h - 1 - y, x);
                let (s, d) = ((y * self.w + x) * 2, (ny * w + nx) * 2);
                la[d] = self.la[s];
                la[d + 1] = self.la[s + 1];
            }
        }
        Clip { w, h, la }
    }
}

impl TileArt {
    /// The art of a built-in texture.
    pub fn from_builtin(id: u8) -> Option<Self> {
        (1..=crate::texture::TEXTURE_COUNT).contains(&id).then(|| Self { la: crate::texture::tile(id).into_iter().flat_map(|s| [s, 255]).collect() })
    }

    /// Copies the rectangle between two corners (inclusive, any order).
    pub fn copy_rect(&self, a: (usize, usize), b: (usize, usize)) -> Clip {
        let (x0, x1) = (a.0.min(b.0).min(TILE - 1), a.0.max(b.0).min(TILE - 1));
        let (y0, y1) = (a.1.min(b.1).min(TILE - 1), a.1.max(b.1).min(TILE - 1));
        let (w, h) = (x1 - x0 + 1, y1 - y0 + 1);
        let mut la = Vec::with_capacity(w * h * 2);
        for y in y0..=y1 {
            for x in x0..=x1 {
                let (s, al) = self.get(x, y);
                la.extend([s, al]);
            }
        }
        Clip { w, h, la }
    }

    /// Makes the rectangle transparent.
    pub fn clear_rect(&mut self, a: (usize, usize), b: (usize, usize)) {
        for y in a.1.min(b.1)..=a.1.max(b.1).min(TILE - 1) {
            for x in a.0.min(b.0)..=a.0.max(b.0).min(TILE - 1) {
                self.set(x, y, 0, 0);
            }
        }
    }

    /// Puts a clip with its top left at `(x, y)`. Transparent pixels of the
    /// clip leave the tile alone when `skip_transparent` is set. Anything
    /// outside the tile is cut off.
    pub fn paste(&mut self, clip: &Clip, x: i32, y: i32, skip_transparent: bool) {
        for cy in 0..clip.h {
            for cx in 0..clip.w {
                let (tx, ty) = (x + cx as i32, y + cy as i32);
                if !(0..TILE as i32).contains(&tx) || !(0..TILE as i32).contains(&ty) {
                    continue;
                }
                let i = (cy * clip.w + cx) * 2;
                if skip_transparent && clip.la[i + 1] == 0 {
                    continue;
                }
                self.set(tx as usize, ty as usize, clip.la[i], clip.la[i + 1]);
            }
        }
    }

    /// Whole tile as a clip.
    pub fn as_clip(&self) -> Clip {
        Clip { w: TILE, h: TILE, la: self.la.clone() }
    }

    pub fn flip_h(&self) -> Self {
        Self { la: self.as_clip().flip_h().la }
    }

    pub fn flip_v(&self) -> Self {
        Self { la: self.as_clip().flip_v().la }
    }

    pub fn rotate_cw(&self) -> Self {
        Self { la: self.as_clip().rotate_cw().la }
    }

    /// Slides the picture, wrapping round the edges: the way to check or fix
    /// the seams of a tile that has to repeat.
    pub fn shifted(&self, dx: i32, dy: i32) -> Self {
        let mut out = Self::blank();
        for y in 0..TILE {
            for x in 0..TILE {
                let (s, a) = self.get(x, y);
                let nx = (x as i32 + dx).rem_euclid(TILE as i32) as usize;
                let ny = (y as i32 + dy).rem_euclid(TILE as i32) as usize;
                out.set(nx, ny, s, a);
            }
        }
        out
    }

    /// Adds up to `amount` of random grain to every visible pixel's shade.
    pub fn grain(&mut self, amount: u8, seed: u32) {
        let mut state = seed.wrapping_mul(2_654_435_761).wrapping_add(0x9E37_79B9) | 1;
        for p in self.la.chunks_mut(2) {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            if p[1] > 0 {
                let d = (state % (2 * amount as u32 + 1)) as i32 - amount as i32;
                p[0] = (p[0] as i32 + d).clamp(0, 255) as u8;
            }
        }
    }

    /// Lightens (positive) or darkens (negative) every visible pixel.
    pub fn brighten(&mut self, delta: i32) {
        for p in self.la.chunks_mut(2) {
            if p[1] > 0 {
                p[0] = (p[0] as i32 + delta).clamp(0, 255) as u8;
            }
        }
    }

    /// Is the left edge a continuation of the right one, and the top of the
    /// bottom one? A rough count of the visible jump across each seam, 0 for a
    /// perfectly seamless tile.
    pub fn seam_error(&self) -> u32 {
        let jump = |a: (u8, u8), b: (u8, u8)| (a.0 as i32 - b.0 as i32).unsigned_abs() + (a.1 as i32 - b.1 as i32).unsigned_abs();
        let mut e = 0;
        for i in 0..TILE {
            e += jump(self.get(TILE - 1, i), self.get(0, i));
            e += jump(self.get(i, TILE - 1), self.get(i, 0));
        }
        e
    }
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

#[cfg(test)]
mod ops_tests {
    use super::*;

    fn gradient() -> TileArt {
        let mut a = TileArt::blank();
        for y in 0..TILE {
            for x in 0..TILE {
                a.set(x, y, (x * 8) as u8, 255);
            }
        }
        a
    }

    #[test]
    fn flips_rotations_and_shifts_are_exact() {
        let a = gradient();
        assert_eq!(a.flip_h().get(0, 5), a.get(TILE - 1, 5));
        assert_eq!(a.flip_h().flip_h(), a);
        assert_eq!(a.flip_v().get(3, 0), a.get(3, TILE - 1));
        let r = a.rotate_cw();
        assert_eq!(r.get(TILE - 1, 0), a.get(0, 0), "top left goes to top right");
        assert_eq!(r.get(TILE - 1, TILE - 1), a.get(TILE - 1, 0));
        assert_eq!(a.rotate_cw().rotate_cw().rotate_cw().rotate_cw(), a);
        assert_eq!(a.shifted(1, 0).get(1, 0), a.get(0, 0));
        assert_eq!(a.shifted(1, 0).get(0, 0), a.get(TILE - 1, 0), "wraps round");
        assert_eq!(a.shifted(7, 3).shifted(-7, -3), a);
    }

    #[test]
    fn copy_paste_and_clear_rectangles() {
        let a = gradient();
        let clip = a.copy_rect((10, 4), (3, 2)); // corners in any order
        assert_eq!((clip.w, clip.h), (8, 3));
        let mut b = TileArt::blank();
        b.paste(&clip, 20, 29, false);
        assert_eq!(b.get(20, 29), a.get(3, 2));
        assert_eq!(b.get(27, 31), a.get(10, 4));
        assert_eq!(b.get(28, 31), (0, 0), "nothing beyond the clip");
        // Cut off at the tile's edge instead of panicking.
        b.paste(&clip, 30, 30, false);
        assert_eq!(b.get(31, 31), a.get(4, 3));
        let mut c = TileArt::solid(9);
        c.clear_rect((2, 2), (4, 3));
        assert_eq!(c.get(3, 3), (0, 0));
        assert_eq!(c.get(5, 3), (9, 255));
        // Transparent pixels can be left out of a paste.
        let mut hole = TileArt::solid(50);
        hole.clear_rect((0, 0), (31, 31));
        let mut d = TileArt::solid(7);
        d.paste(&hole.as_clip(), 0, 0, true);
        assert_eq!(d, TileArt::solid(7));
    }

    #[test]
    fn grain_brighten_builtin_and_seams() {
        let mut a = TileArt::solid(128);
        a.grain(10, 3);
        assert!(a.la.chunks(2).all(|p| (118..=138).contains(&p[0]) && p[1] == 255));
        assert!(a.la.chunks(2).any(|p| p[0] != 128));
        let mut t = TileArt::blank();
        t.grain(50, 1);
        assert!(t.is_blank(), "grain leaves transparent pixels alone");
        let mut b = TileArt::solid(250);
        b.brighten(20);
        assert_eq!(b.get(0, 0).0, 255);
        b.brighten(-300);
        assert_eq!(b.get(0, 0).0, 0);
        assert_eq!(TileArt::from_builtin(4).unwrap().la.len(), TILE_PIXELS * 2);
        assert!(TileArt::from_builtin(1).is_some() && TileArt::from_builtin(0).is_none() && TileArt::from_builtin(99).is_none());
        assert_eq!(TileArt::solid(40).seam_error(), 0);
        assert!(gradient().seam_error() > 0, "a ramp has a jump at the seam");
        assert!(gradient().shifted(16, 0).seam_error() > 0);
    }
}
