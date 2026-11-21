//! Copied areas: a clipboard of cells that can be flipped, rotated, pasted
//! and saved as a named stamp. Colours are stored as RGB (not as indices into
//! one map's colour table) so a stamp works in any map.

use crate::chunk::{Cell, EMPTY, cell_color, cell_texture, make_cell};
use crate::history::CellEdit;
use crate::selection::Selection;
use crate::texture::MAX_TEXTURES;
use crate::World;

/// Largest side of a stamp, in cells.
pub const MAX_SIDE: u32 = 256;
pub const MAX_NAME: usize = 40;
const MAGIC: &[u8; 4] = b"DSTP";

/// One cell of a stamp. Texture `0` means "leave the map as it is".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct StampCell {
    pub texture: u8,
    pub rgb: [u8; 3],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stamp {
    pub name: String,
    w: u32,
    h: u32,
    cells: Vec<StampCell>,
}

impl Stamp {
    pub fn new(name: &str, w: u32, h: u32, cells: Vec<StampCell>) -> Option<Stamp> {
        if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE || cells.len() != (w * h) as usize {
            return None;
        }
        let name: String = name.chars().filter(|c| !c.is_control()).take(MAX_NAME).collect();
        Some(Stamp { name: name.trim().to_string(), w, h, cells })
    }

    pub fn width(&self) -> u32 {
        self.w
    }

    pub fn height(&self) -> u32 {
        self.h
    }

    pub fn at(&self, x: u32, y: u32) -> StampCell {
        self.cells[(y * self.w + x) as usize]
    }

    /// Number of cells that carry a tile.
    pub fn filled(&self) -> usize {
        self.cells.iter().filter(|c| c.texture != 0).count()
    }

    /// Copies the selected cells of `depth` (the whole bounding rectangle of
    /// the selection; unselected and empty cells stay transparent). `None`
    /// when the selection is empty, inverted or bigger than [`MAX_SIDE`].
    pub fn capture(world: &World, depth: u8, sel: &Selection) -> Result<Stamp, CaptureError> {
        let sel = sel.rebased(depth).unwrap_or_else(|| sel.clone());
        let ((x0, y0), (x1, y1)) = sel.bounds().ok_or(CaptureError::Unbounded)?;
        let (w, h) = ((x1 - x0 + 1) as u64, (y1 - y0 + 1) as u64);
        if w > MAX_SIDE as u64 || h > MAX_SIDE as u64 {
            return Err(CaptureError::TooBig);
        }
        let mut cells = Vec::with_capacity((w * h) as usize);
        for y in y0..=y1 {
            for x in x0..=x1 {
                let c = if sel.contains(depth, x, y) { world.get(depth, x, y) } else { EMPTY };
                cells.push(if cell_texture(c) == 0 {
                    StampCell::default()
                } else {
                    StampCell { texture: cell_texture(c), rgb: world.colors.get(cell_color(c)) }
                });
            }
        }
        let stamp = Stamp { name: String::new(), w: w as u32, h: h as u32, cells };
        if stamp.filled() == 0 { Err(CaptureError::Empty) } else { Ok(stamp) }
    }

    pub fn flip_horizontal(&mut self) {
        for y in 0..self.h {
            let row = (y * self.w) as usize;
            self.cells[row..row + self.w as usize].reverse();
        }
    }

    pub fn flip_vertical(&mut self) {
        let mut out = Vec::with_capacity(self.cells.len());
        for y in (0..self.h).rev() {
            let row = (y * self.w) as usize;
            out.extend_from_slice(&self.cells[row..row + self.w as usize]);
        }
        self.cells = out;
    }

    /// Quarter turn clockwise.
    pub fn rotate_clockwise(&mut self) {
        let (w, h) = (self.w, self.h);
        let mut out = vec![StampCell::default(); self.cells.len()];
        for y in 0..h {
            for x in 0..w {
                // (x, y) goes to (h - 1 - y, x) in an image h wide and w tall.
                out[(x * h + (h - 1 - y)) as usize] = self.at(x, y);
            }
        }
        self.cells = out;
        (self.w, self.h) = (h, w);
    }

    pub fn rotate_counter_clockwise(&mut self) {
        for _ in 0..3 {
            self.rotate_clockwise();
        }
    }

    /// Writes the stamp with its top-left cell at `origin` on `depth`.
    /// Cells outside the map are skipped. Returns the edits (already
    /// applied) so the caller can record them as one undo step.
    pub fn apply(&self, world: &mut World, depth: u8, origin: (i64, i64)) -> Vec<CellEdit> {
        let mut edits = Vec::new();
        for y in 0..self.h {
            for x in 0..self.w {
                let c = self.at(x, y);
                if c.texture == 0 {
                    continue;
                }
                let (mx, my) = (origin.0 + x as i64, origin.1 + y as i64);
                if !world.in_bounds(depth, mx, my) {
                    continue;
                }
                let index = world.colors.index_of(c.rgb);
                let new: Cell = make_cell(c.texture, index);
                let old = world.set(depth, mx, my, new);
                if old != new {
                    edits.push(CellEdit { depth, x: mx, y: my, old, new });
                }
            }
        }
        edits
    }

    /// The stamp as a small binary file (see [`from_bytes`](Self::from_bytes)).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(16 + self.cells.len() * 4);
        out.extend_from_slice(MAGIC);
        out.push(1);
        let name = self.name.as_bytes();
        out.push(name.len() as u8);
        out.extend_from_slice(name);
        out.extend_from_slice(&(self.w as u16).to_le_bytes());
        out.extend_from_slice(&(self.h as u16).to_le_bytes());
        for c in &self.cells {
            out.push(c.texture);
            out.extend_from_slice(&c.rgb);
        }
        out
    }

    pub fn from_bytes(b: &[u8]) -> Option<Stamp> {
        if b.get(..4)? != MAGIC || *b.get(4)? != 1 {
            return None;
        }
        let n = *b.get(5)? as usize;
        let name = std::str::from_utf8(b.get(6..6 + n)?).ok()?.to_string();
        let mut p = 6 + n;
        let w = u16::from_le_bytes([*b.get(p)?, *b.get(p + 1)?]) as u32;
        let h = u16::from_le_bytes([*b.get(p + 2)?, *b.get(p + 3)?]) as u32;
        p += 4;
        if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE {
            return None;
        }
        let body = b.get(p..p + (w * h) as usize * 4)?;
        let cells = body
            .chunks_exact(4)
            .map(|c| StampCell {
                texture: if c[0] > MAX_TEXTURES { 0 } else { c[0] },
                rgb: [c[1], c[2], c[3]],
            })
            .collect();
        Stamp::new(&name, w, h, cells)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureError {
    /// Nothing selected, or "everything but..." (no bounds).
    Unbounded,
    TooBig,
    /// The selected cells hold no tiles.
    Empty,
}

impl CaptureError {
    pub fn message(self) -> String {
        match self {
            CaptureError::Unbounded => "Select an area first (not \"everything but...\")".into(),
            CaptureError::TooBig => format!("Copy at most {MAX_SIDE} x {MAX_SIDE} cells at a time"),
            CaptureError::Empty => "The selected cells are empty".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world_with_l() -> World {
        let mut w = World::new();
        // An L shape, 2 wide 3 tall:   X.
        //                              X.
        //                              XX
        for (x, y) in [(10, 20), (10, 21), (10, 22), (11, 22)] {
            let i = w.colors.index_of([200, 10, 10]);
            w.set(0, x, y, make_cell(3, i));
        }
        w
    }

    fn captured() -> Stamp {
        let w = world_with_l();
        Stamp::capture(&w, 0, &Selection::rect(0, (10, 20), (11, 22))).unwrap()
    }

    #[test]
    fn capture_keeps_tiles_colours_and_gaps() {
        let s = captured();
        assert_eq!((s.width(), s.height(), s.filled()), (2, 3, 4));
        assert_eq!(s.at(0, 0).texture, 3);
        assert_eq!(s.at(0, 0).rgb, [200, 10, 10]);
        assert_eq!(s.at(1, 0).texture, 0);
        assert_eq!(s.at(1, 2).texture, 3);
    }

    #[test]
    fn capture_only_takes_selected_cells_and_rejects_bad_selections() {
        let w = world_with_l();
        let s = Stamp::capture(&w, 0, &Selection::rect(0, (10, 20), (10, 22))).unwrap();
        assert_eq!((s.width(), s.filled()), (1, 3));
        assert_eq!(
            Stamp::capture(&w, 0, &Selection::rect(0, (50, 50), (52, 52))),
            Err(CaptureError::Empty)
        );
        assert_eq!(Stamp::capture(&w, 0, &Selection::all(0)), Err(CaptureError::Unbounded));
        assert_eq!(
            Stamp::capture(&w, 0, &Selection::rect(0, (0, 0), (300, 3))),
            Err(CaptureError::TooBig)
        );
    }

    #[test]
    fn flips_and_rotations_move_the_cells() {
        let mut s = captured();
        s.flip_horizontal();
        assert_eq!((s.at(1, 0).texture, s.at(0, 0).texture), (3, 0));
        s.flip_horizontal();
        s.flip_vertical();
        assert_eq!((s.at(0, 0).texture, s.at(1, 0).texture, s.at(1, 2).texture), (3, 3, 0));
        s.flip_vertical();
        assert_eq!(s, captured());

        // Clockwise: the L (3 tall) lies down, 3 wide and 2 tall.
        s.rotate_clockwise();
        assert_eq!((s.width(), s.height()), (3, 2));
        // Original column x=0 becomes the top row (read right to left).
        assert_eq!([s.at(0, 0).texture, s.at(1, 0).texture, s.at(2, 0).texture], [3, 3, 3]);
        assert_eq!([s.at(0, 1).texture, s.at(1, 1).texture, s.at(2, 1).texture], [3, 0, 0]);
        s.rotate_counter_clockwise();
        assert_eq!(s, captured());
        for _ in 0..4 {
            s.rotate_clockwise();
        }
        assert_eq!(s, captured());
    }

    #[test]
    fn apply_writes_with_map_colours_and_skips_gaps_and_outside() {
        let s = captured();
        let mut other = World::new();
        other.set(0, 6, 5, make_cell(9, 0)); // under a gap: must stay
        let edits = s.apply(&mut other, 0, (5, 5));
        assert_eq!(edits.len(), 4);
        assert_eq!(cell_texture(other.get(0, 5, 5)), 3);
        assert_eq!(other.colors.get(cell_color(other.get(0, 5, 5))), [200, 10, 10]);
        assert_eq!(cell_texture(other.get(0, 6, 5)), 9, "gaps do not erase");
        assert_eq!(edits[0].old, EMPTY);
    }

    #[test]
    fn bytes_round_trip_and_garbage_is_rejected() {
        let mut s = captured();
        s.name = "Corner".into();
        assert_eq!(Stamp::from_bytes(&s.to_bytes()), Some(s.clone()));
        let b = s.to_bytes();
        assert_eq!(Stamp::from_bytes(&b[..b.len() - 1]), None);
        assert_eq!(Stamp::from_bytes(b"nope"), None);
        assert_eq!(Stamp::from_bytes(&[]), None);
    }
}
