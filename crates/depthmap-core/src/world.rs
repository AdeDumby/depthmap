use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use rustc_hash::FxHashMap;

use crate::chunk::{Cell, Chunk, ChunkPos, EMPTY, local_index, normalize};
use crate::colors::ColorTable;
use crate::icons;
use crate::node::{Icon, Node, NodeId};
use crate::spec::MapSpec;
use crate::{CHUNK_AREA, MAX_DEPTH};

/// Source of chunk version stamps. Global rather than per-world on purpose:
/// a renderer caches GPU data per chunk position and version, and a fresh
/// `World` (File > New / Open) must never reuse a version an older world
/// already handed out, or the cache would show stale content.
static STAMP: AtomicU64 = AtomicU64::new(1);

fn next_stamp() -> u64 {
    STAMP.fetch_add(1, Ordering::Relaxed)
}

/// An imported PNG kept inside the map, so a map file still shows its icons
/// on a computer that does not have the original files.
#[derive(Clone, Debug)]
pub struct CustomIcon {
    pub png: Arc<Vec<u8>>,
    /// Identifies the contents (for caches), not a security hash.
    pub hash: u64,
}

impl CustomIcon {
    pub fn new(png: Vec<u8>) -> Self {
        let mut h = rustc_hash::FxHasher::default();
        png.hash(&mut h);
        Self { hash: h.finish(), png: Arc::new(png) }
    }
}

/// All depths of the map. One sparse chunk map per depth.
pub struct World {
    /// The colour table cells index into.
    pub colors: ColorTable,
    layers: Vec<FxHashMap<ChunkPos, Chunk>>,
    /// Number of edits so far (see [`World::revision`]).
    clock: u64,
    spec: MapSpec,
    nodes: Vec<Node>,
    node_seq: NodeId,
    custom_icons: BTreeMap<String, CustomIcon>,
}

impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}

impl World {
    /// An empty, unbounded map (every depth, no size limit).
    pub fn new() -> Self {
        Self::with_spec(MapSpec::UNBOUNDED)
    }

    /// An empty map of the given size and depth.
    pub fn with_spec(spec: MapSpec) -> Self {
        Self {
            colors: ColorTable::new(),
            layers: (0..=MAX_DEPTH).map(|_| FxHashMap::default()).collect(),
            clock: 0,
            spec,
            nodes: Vec::new(),
            node_seq: 1,
            custom_icons: BTreeMap::new(),
        }
    }

    // ---- nodes ----------------------------------------------------------------

    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }

    pub fn node(&self, id: NodeId) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }

    /// A fresh node id, never used before in this map.
    pub fn alloc_node_id(&mut self) -> NodeId {
        let id = self.node_seq;
        self.node_seq += 1;
        id
    }

    /// Adds a node, or replaces the one with the same id (keeping its place
    /// in the drawing order). Returns false, changing nothing, when the node
    /// is outside the map. Only real changes count as edits.
    pub fn put_node(&mut self, node: Node) -> bool {
        let node = node.sanitized();
        if !self.spec.contains(node.depth, node.x, node.y) {
            return false;
        }
        self.node_seq = self.node_seq.max(node.id.saturating_add(1));
        match self.nodes.iter_mut().find(|n| n.id == node.id) {
            Some(slot) => {
                if *slot != node {
                    *slot = node;
                    self.clock += 1;
                }
            }
            None => {
                self.nodes.push(node);
                self.clock += 1;
            }
        }
        true
    }

    pub fn remove_node(&mut self, id: NodeId) -> Option<Node> {
        let at = self.nodes.iter().position(|n| n.id == id)?;
        self.clock += 1;
        Some(self.nodes.remove(at))
    }

    /// Makes node `id` exactly `state` (used by undo and redo).
    pub fn set_node_state(&mut self, id: NodeId, state: Option<&Node>) {
        match state {
            Some(n) => {
                self.put_node(n.clone());
            }
            None => {
                self.remove_node(id);
            }
        }
    }

    // ---- imported icons ----------------------------------------------------------

    /// Stores an imported PNG under `name` (replacing any with that name).
    pub fn add_custom_icon(&mut self, name: &str, png: Vec<u8>) -> Result<(), String> {
        if !icons::valid_icon_name(name) {
            return Err(format!("\"{name}\" is not a usable icon file name"));
        }
        if png.len() > icons::MAX_PNG_BYTES {
            return Err("the icon is larger than 8 MB".into());
        }
        self.custom_icons.insert(name.to_string(), CustomIcon::new(png));
        Ok(())
    }

    pub fn custom_icon(&self, name: &str) -> Option<&CustomIcon> {
        self.custom_icons.get(name)
    }

    pub fn custom_icons(&self) -> impl Iterator<Item = (&String, &CustomIcon)> {
        self.custom_icons.iter()
    }

    /// Imported icons that some node actually uses (what gets saved).
    pub fn used_custom_icons(&self) -> Vec<(&String, &CustomIcon)> {
        self.custom_icons
            .iter()
            .filter(|(name, _)| {
                self.nodes.iter().any(|n| matches!(&n.icon, Icon::Custom(c) if c == *name))
            })
            .collect()
    }

    pub fn spec(&self) -> MapSpec {
        self.spec
    }

    /// True if the cell lies inside the map.
    #[inline]
    pub fn in_bounds(&self, depth: u8, x: i64, y: i64) -> bool {
        self.spec.contains(depth, x, y)
    }

    #[inline]
    pub fn get(&self, depth: u8, x: i64, y: i64) -> Cell {
        self.layers[depth as usize]
            .get(&ChunkPos::containing(x, y))
            .map_or(EMPTY, |c| c.cells()[local_index(x, y)])
    }

    /// Sets a cell and returns the previous value. Chunks are created on
    /// demand and dropped again when their last cell is erased.
    ///
    /// Cells outside the map (see [`MapSpec`]) are ignored; like any other
    /// no-op, that returns `m` itself.
    pub fn set(&mut self, depth: u8, x: i64, y: i64, m: Cell) -> Cell {
        let m = normalize(m);
        if !self.spec.contains(depth, x, y) {
            return m;
        }
        let pos = ChunkPos::containing(x, y);
        let idx = local_index(x, y);
        let layer = &mut self.layers[depth as usize];
        match layer.get_mut(&pos) {
            Some(chunk) => {
                if chunk.cells()[idx] == m {
                    return m;
                }
                self.clock += 1;
                let old = chunk.set_index(idx, m, next_stamp());
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
                chunk.set_index(idx, m, next_stamp());
                layer.insert(pos, chunk);
                EMPTY
            }
        }
    }

    /// Inserts a whole chunk at once (used by loading and generators).
    /// Replaces any chunk already there; an all-empty chunk removes it.
    pub fn insert_chunk(&mut self, depth: u8, pos: ChunkPos, cells: &[Cell; CHUNK_AREA]) {
        let layer = &mut self.layers[depth as usize];
        if cells.iter().all(|&c| c == EMPTY) {
            layer.remove(&pos);
            return;
        }
        self.clock += 1;
        layer.insert(pos, Chunk::from_cells(cells, next_stamp()));
    }

    pub fn chunk(&self, depth: u8, pos: ChunkPos) -> Option<&Chunk> {
        self.layers[depth as usize].get(&pos)
    }

    /// Number of edits so far. Compare two values to learn whether anything
    /// changed in between (used for the "unsaved changes" flag).
    pub fn revision(&self) -> u64 {
        self.clock
    }

    /// Puts the edit counter back to an earlier value. Only valid when the
    /// map has been restored to exactly the state it had at that revision
    /// (cancelling a shape preview), so the "unsaved changes" flag stays honest.
    pub fn rewind_revision(&mut self, to: u64) {
        self.clock = self.clock.min(to);
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
    fn different_worlds_never_share_chunk_versions() {
        let mut a = World::new();
        let mut b = World::new();
        a.set(0, 0, 0, 1);
        b.set(0, 0, 0, 2);
        let pos = ChunkPos { x: 0, y: 0 };
        assert_ne!(
            a.chunk(0, pos).unwrap().version(),
            b.chunk(0, pos).unwrap().version(),
            "a renderer cache keyed by (position, version) would show stale data"
        );
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
    fn invalid_cells_are_normalised_on_set() {
        let mut w = World::new();
        w.set(0, 1, 1, 7 << 8); // colour but no texture
        assert_eq!(w.total_chunks(), 0);
        w.set(0, 1, 1, 200); // texture id out of range
        assert_eq!(w.total_chunks(), 0);
    }

    #[test]
    fn revision_advances_only_on_real_changes() {
        let mut w = World::new();
        let r = w.revision();
        w.set(0, 0, 0, 1);
        assert!(w.revision() > r);
        let r = w.revision();
        w.set(0, 0, 0, 1);
        assert_eq!(w.revision(), r);
    }

    #[test]
    fn nodes_can_be_added_replaced_and_removed() {
        let mut w = World::with_spec(MapSpec::new(8, 8, 1));
        let id = w.alloc_node_id();
        assert!(w.put_node(Node::new(id, 1, 5, 5, Icon::Builtin(3))));
        assert_eq!(w.revision(), 1);
        let mut n = w.node(id).unwrap().clone();
        assert!(w.put_node(n.clone()));
        assert_eq!(w.revision(), 1, "an identical node is not an edit");
        n.name = "Tavern".into();
        assert!(w.put_node(n));
        assert_eq!(w.revision(), 2);
        assert_eq!(w.node(id).unwrap().name, "Tavern");
        assert_eq!(w.nodes().len(), 1);
        assert!(w.alloc_node_id() > id);
        assert_eq!(w.remove_node(id).unwrap().name, "Tavern");
        assert!(w.remove_node(id).is_none());
        assert!(w.nodes().is_empty());
    }

    #[test]
    fn nodes_outside_the_map_are_refused_and_ids_never_repeat() {
        let mut w = World::with_spec(MapSpec::new(8, 8, 1));
        assert!(!w.put_node(Node::new(1, 0, 8, 0, Icon::default())));
        assert!(!w.put_node(Node::new(2, 2, 0, 0, Icon::default())), "depth 2 does not exist");
        assert!(w.nodes().is_empty() && w.revision() == 0);
        assert!(w.put_node(Node::new(40, 0, 1, 1, Icon::default())));
        assert!(w.alloc_node_id() > 40, "ids stay ahead of loaded nodes");
    }

    #[test]
    fn imported_icons_are_kept_and_only_used_ones_are_listed() {
        let mut w = World::new();
        w.add_custom_icon("a.png", vec![1, 2, 3]).unwrap();
        w.add_custom_icon("b.png", vec![4]).unwrap();
        assert!(w.add_custom_icon("../evil.png", vec![1]).is_err());
        assert!(w.add_custom_icon("c.png", vec![0; crate::icons::MAX_PNG_BYTES + 1]).is_err());
        assert_eq!(w.custom_icons().count(), 2);
        assert!(w.used_custom_icons().is_empty());
        w.put_node(Node::new(1, 0, 0, 0, Icon::Custom("b.png".into())));
        let used: Vec<_> = w.used_custom_icons().iter().map(|(n, _)| n.to_string()).collect();
        assert_eq!(used, vec!["b.png"]);
        let h = w.custom_icon("a.png").unwrap().hash;
        w.add_custom_icon("a.png", vec![9, 9]).unwrap();
        assert_ne!(w.custom_icon("a.png").unwrap().hash, h, "changed contents, new hash");
    }

    #[test]
    fn cells_outside_the_map_are_ignored() {
        let mut w = World::with_spec(MapSpec::new(8, 8, 1));
        assert_eq!(w.set(0, 8, 0, 3), 3, "rejected like a no-op");
        assert_eq!(w.set(0, -1, 0, 3), 3);
        assert_eq!(w.set(2, 0, 0, 3), 3, "depth 2 does not exist in this map");
        assert_eq!(w.total_chunks(), 0);
        assert_eq!(w.revision(), 0);
        assert_eq!(w.set(1, 31, 31, 3), EMPTY);
        assert_eq!(w.get(1, 31, 31), 3);
        assert_eq!(w.set(1, 32, 31, 3), 3);
        assert_eq!(w.total_chunks(), 1);
    }

    #[test]
    fn rewinding_the_revision_never_moves_it_forward() {
        let mut w = World::new();
        w.set(0, 0, 0, 1);
        let r = w.revision();
        w.set(0, 1, 0, 1);
        w.rewind_revision(r);
        assert_eq!(w.revision(), r);
        w.rewind_revision(r + 100);
        assert_eq!(w.revision(), r);
    }

    #[test]
    fn insert_chunk_roundtrip() {
        let mut w = World::new();
        let mut cells = [EMPTY; CHUNK_AREA];
        cells[0] = 4;
        cells[CHUNK_AREA - 1] = 9 | (3 << 8);
        w.insert_chunk(0, ChunkPos { x: -2, y: 3 }, &cells);
        assert_eq!(w.get(0, -64, 96), 4);
        assert_eq!(w.get(0, -33, 127), 9 | (3 << 8));
        w.insert_chunk(0, ChunkPos { x: -2, y: 3 }, &[EMPTY; CHUNK_AREA]);
        assert_eq!(w.total_chunks(), 0);
    }
}
