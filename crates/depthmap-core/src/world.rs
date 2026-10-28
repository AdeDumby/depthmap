use rustc_hash::FxHashMap;

use crate::chunk::{Chunk, ChunkPos, EMPTY, Material, local_index};
use crate::{CHUNK_AREA, MAX_DEPTH};

/// All depths of the map. One sparse chunk map per depth.
pub struct World {
    layers: Vec<FxHashMap<ChunkPos, Chunk>>,
    /// Monotonic edit counter; see [`Chunk::version`].
    clock: u64,
}

impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}

impl World {
    pub fn new() -> Self {
        Self {
            layers: (0..=MAX_DEPTH).map(|_| FxHashMap::default()).collect(),
            clock: 0,
        }
    }

    #[inline]
    pub fn get(&self, depth: u8, x: i64, y: i64) -> Material {
        self.layers[depth as usize]
            .get(&ChunkPos::containing(x, y))
            .map_or(EMPTY, |c| c.cells()[local_index(x, y)])
    }

    /// Sets a cell and returns the previous value. Chunks are created on
    /// demand and dropped again when their last cell is erased.
    pub fn set(&mut self, depth: u8, x: i64, y: i64, m: Material) -> Material {
        let pos = ChunkPos::containing(x, y);
        let idx = local_index(x, y);
        let layer = &mut self.layers[depth as usize];
        match layer.get_mut(&pos) {
            Some(chunk) => {
                if chunk.cells()[idx] == m {
                    return m;
                }
                self.clock += 1;
                let old = chunk.set_index(idx, m, self.clock);
                if chunk.is_empty() {
                    layer.remove(&pos);
                }
                old
            }
            None => {
                if m == EMPTY {
                    return EMPTY;
                }
                self.clock += 1;
                let mut chunk = Chunk::empty();
                chunk.set_index(idx, m, self.clock);
                layer.insert(pos, chunk);
                EMPTY
            }
        }
    }

    /// Inserts a whole chunk at once (used by loading and generators).
    /// Replaces any chunk already there; an all-empty chunk removes it.
    pub fn insert_chunk(&mut self, depth: u8, pos: ChunkPos, cells: &[Material; CHUNK_AREA]) {
        let layer = &mut self.layers[depth as usize];
        if cells.iter().all(|&c| c == EMPTY) {
            layer.remove(&pos);
            return;
        }
        self.clock += 1;
        layer.insert(pos, Chunk::from_cells(cells, self.clock));
    }

    pub fn chunk(&self, depth: u8, pos: ChunkPos) -> Option<&Chunk> {
        self.layers[depth as usize].get(&pos)
    }

    #[inline]
    pub fn layer_is_empty(&self, depth: u8) -> bool {
        self.layers[depth as usize].is_empty()
    }

    pub fn chunk_count(&self, depth: u8) -> usize {
        self.layers[depth as usize].len()
    }

    pub fn total_chunks(&self) -> usize {
        self.layers.iter().map(|l| l.len()).sum()
    }

    pub fn iter_chunks(&self, depth: u8) -> impl Iterator<Item = (ChunkPos, &Chunk)> {
        self.layers[depth as usize].iter().map(|(p, c)| (*p, c))
    }

    /// Calls `f` for every existing chunk of `depth` inside the inclusive
    /// chunk rectangle `min..=max`.
    ///
    /// Picks the cheaper strategy automatically: probe every position of the
    /// rectangle when it is small, or scan the stored chunks when the
    /// rectangle is larger than the layer itself (very zoomed-out views).
    pub fn visit_chunks(
        &self,
        depth: u8,
        min: ChunkPos,
        max: ChunkPos,
        mut f: impl FnMut(ChunkPos, &Chunk),
    ) {
        let layer = &self.layers[depth as usize];
        if layer.is_empty() || max.x < min.x || max.y < min.y {
            return;
        }
        let w = (max.x - min.x) as u128 + 1;
        let h = (max.y - min.y) as u128 + 1;
        if w * h > layer.len() as u128 {
            for (p, c) in layer {
                if p.x >= min.x && p.x <= max.x && p.y >= min.y && p.y <= max.y {
                    f(*p, c);
                }
            }
        } else {
            for y in min.y..=max.y {
                for x in min.x..=max.x {
                    let p = ChunkPos { x, y };
                    if let Some(c) = layer.get(&p) {
                        f(p, c);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_get_roundtrip_including_negative_cells() {
        let mut w = World::new();
        for &(x, y) in &[(0, 0), (-1, -1), (31, 32), (-33, 100), (1 << 40, -(1 << 40))] {
            assert_eq!(w.get(2, x, y), EMPTY);
            assert_eq!(w.set(2, x, y, 7), EMPTY);
            assert_eq!(w.get(2, x, y), 7);
        }
        // Other depths are independent.
        assert_eq!(w.get(3, 0, 0), EMPTY);
        assert!(w.layer_is_empty(0));
    }

    #[test]
    fn erasing_the_last_cell_drops_the_chunk() {
        let mut w = World::new();
        w.set(0, 5, 5, 1);
        w.set(0, 6, 5, 1);
        assert_eq!(w.chunk_count(0), 1);
        w.set(0, 5, 5, EMPTY);
        assert_eq!(w.chunk_count(0), 1);
        w.set(0, 6, 5, EMPTY);
        assert_eq!(w.chunk_count(0), 0);
        assert_eq!(w.set(0, 6, 5, EMPTY), EMPTY); // erasing nothing is a no-op
    }

    #[test]
    fn recreated_chunk_gets_a_fresh_version() {
        let mut w = World::new();
        w.set(0, 0, 0, 1);
        let v1 = w.chunk(0, ChunkPos { x: 0, y: 0 }).unwrap().version();
        w.set(0, 0, 0, EMPTY);
        w.set(0, 0, 0, 1);
        let v2 = w.chunk(0, ChunkPos { x: 0, y: 0 }).unwrap().version();
        assert_ne!(v1, v2);
    }

    #[test]
    fn visit_chunks_agrees_for_both_strategies() {
        let mut w = World::new();
        for cx in -3..3 {
            for cy in -3..3 {
                w.set(1, cx * 32, cy * 32, 1);
            }
        }
        let collect = |min: ChunkPos, max: ChunkPos| {
            let mut v = Vec::new();
            w.visit_chunks(1, min, max, |p, _| v.push(p));
            v.sort();
            v
        };
        // Small rectangle: probes positions.
        let small = collect(ChunkPos { x: -1, y: -1 }, ChunkPos { x: 1, y: 1 });
        assert_eq!(small.len(), 9);
        // Huge rectangle: scans the layer.
        let huge = collect(
            ChunkPos { x: -1_000_000, y: -1_000_000 },
            ChunkPos { x: 1_000_000, y: 1_000_000 },
        );
        assert_eq!(huge.len(), 36);
        // Inverted rectangle visits nothing.
        assert!(collect(ChunkPos { x: 2, y: 2 }, ChunkPos { x: 1, y: 1 }).is_empty());
    }

    #[test]
    fn insert_chunk_roundtrip() {
        let mut w = World::new();
        let mut cells = [EMPTY; CHUNK_AREA];
        cells[0] = 4;
        cells[CHUNK_AREA - 1] = 9;
        w.insert_chunk(0, ChunkPos { x: -2, y: 3 }, &cells);
        assert_eq!(w.get(0, -64, 96), 4);
        assert_eq!(w.get(0, -33, 127), 9);
        w.insert_chunk(0, ChunkPos { x: -2, y: 3 }, &[EMPTY; CHUNK_AREA]);
        assert_eq!(w.total_chunks(), 0);
    }
}
