use crate::{CHUNK_AREA, CHUNK_SHIFT, CHUNK_SIZE};

/// A material id. `0` means "nothing here"; the palette maps ids to colours.
pub type Material = u8;
pub const EMPTY: Material = 0;

/// Chunk coordinates within one depth.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct ChunkPos {
    pub x: i64,
    pub y: i64,
}

impl ChunkPos {
    /// The chunk containing a cell. Arithmetic shift == floor division, so
    /// negative coordinates work.
    #[inline]
    pub fn containing(cell_x: i64, cell_y: i64) -> Self {
        Self {
            x: cell_x >> CHUNK_SHIFT,
            y: cell_y >> CHUNK_SHIFT,
        }
    }

    /// Cell coordinates of this chunk's top-left cell.
    #[inline]
    pub fn origin(self) -> (i64, i64) {
        (self.x << CHUNK_SHIFT, self.y << CHUNK_SHIFT)
    }
}

/// Index of a cell inside its chunk's flat array (row-major).
#[inline]
pub fn local_index(cell_x: i64, cell_y: i64) -> usize {
    let m = CHUNK_SIZE - 1;
    (((cell_y & m) << CHUNK_SHIFT) | (cell_x & m)) as usize
}

/// A `CHUNK_SIZE`² block of cells.
///
/// `version` changes whenever the content changes. It comes from a world-wide
/// monotonic clock, so a renderer can cache GPU data per chunk and re-upload
/// only when the version it holds differs, even if a chunk was deleted and
/// later re-created at the same position.
pub struct Chunk {
    cells: Box<[Material; CHUNK_AREA]>,
    filled: u32,
    version: u64,
}

impl Chunk {
    pub(crate) fn empty() -> Self {
        Self {
            cells: Box::new([EMPTY; CHUNK_AREA]),
            filled: 0,
            version: 0,
        }
    }

    pub(crate) fn from_cells(cells: &[Material; CHUNK_AREA], version: u64) -> Self {
        let filled = cells.iter().filter(|&&c| c != EMPTY).count() as u32;
        Self {
            cells: Box::new(*cells),
            filled,
            version,
        }
    }

    #[inline]
    pub fn cells(&self) -> &[Material; CHUNK_AREA] {
        &self.cells
    }

    #[inline]
    pub fn version(&self) -> u64 {
        self.version
    }

    /// Number of non-empty cells.
    #[inline]
    pub fn filled(&self) -> u32 {
        self.filled
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.filled == 0
    }

    /// Sets a cell and returns the previous value. Bumps `version` only if
    /// the value actually changed.
    #[inline]
    pub(crate) fn set_index(&mut self, idx: usize, m: Material, version: u64) -> Material {
        let old = std::mem::replace(&mut self.cells[idx], m);
        if old != m {
            match (old == EMPTY, m == EMPTY) {
                (true, false) => self.filled += 1,
                (false, true) => self.filled -= 1,
                _ => {}
            }
            self.version = version;
        }
        old
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_pos_handles_negative_cells() {
        assert_eq!(ChunkPos::containing(0, 0), ChunkPos { x: 0, y: 0 });
        assert_eq!(ChunkPos::containing(31, 31), ChunkPos { x: 0, y: 0 });
        assert_eq!(ChunkPos::containing(32, -1), ChunkPos { x: 1, y: -1 });
        assert_eq!(ChunkPos::containing(-32, -33), ChunkPos { x: -1, y: -2 });
        assert_eq!(ChunkPos { x: -1, y: -2 }.origin(), (-32, -64));
    }

    #[test]
    fn local_index_is_unique_within_a_chunk() {
        let mut seen = vec![false; CHUNK_AREA];
        for y in -64..-32 {
            for x in 96..128 {
                let i = local_index(x, y);
                assert!(!seen[i]);
                seen[i] = true;
            }
        }
        assert!(seen.iter().all(|&s| s));
    }

    #[test]
    fn filled_counter_tracks_changes() {
        let mut c = Chunk::empty();
        assert!(c.is_empty());
        assert_eq!(c.set_index(5, 3, 1), EMPTY);
        assert_eq!(c.set_index(5, 4, 2), 3); // overwrite: still one cell
        assert_eq!(c.filled(), 1);
        assert_eq!(c.version(), 2);
        c.set_index(5, 4, 3); // no-op must not bump the version
        assert_eq!(c.version(), 2);
        c.set_index(5, EMPTY, 4);
        assert!(c.is_empty());
    }
}
