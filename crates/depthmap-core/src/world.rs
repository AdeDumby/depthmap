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
use crate::texture::{MAX_TEXTURES, TEXTURE_COUNT};
use crate::tile::CustomTile;
use crate::route::Route;
use crate::zone::Zone;
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
    /// User-drawn tiles, indexed by texture id (`TEXTURE_COUNT + 1..=MAX_TEXTURES`).
    tiles: Vec<Option<CustomTile>>,
    tiles_version: u64,
    zones: Vec<Zone>,
    routes: Vec<Route>,
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
            tiles: vec![None; MAX_TEXTURES as usize + 1],
            tiles_version: crate::colors::next_version(),
            zones: Vec::new(),
            routes: Vec::new(),
        }
    }

    // ---- routes ----------------------------------------------------------------

    pub fn routes(&self) -> &[Route] {
        &self.routes
    }

    pub fn add_route(&mut self, route: Route) -> Option<usize> {
        if self.routes.len() >= crate::route::MAX_ROUTES {
            return None;
        }
        self.routes.push(route);
        self.clock += 1;
        Some(self.routes.len() - 1)
    }

    pub fn edit_route<R>(&mut self, index: usize, f: impl FnOnce(&mut Route) -> R) -> Option<R> {
        let r = self.routes.get_mut(index)?;
        let out = f(r);
        self.clock += 1;
        Some(out)
    }

    pub fn remove_route(&mut self, index: usize) -> bool {
        if index < self.routes.len() {
            self.routes.remove(index);
            self.clock += 1;
            true
        } else {
            false
        }
    }

    // ---- zones (named regions) -------------------------------------------------

    pub const MAX_ZONES: usize = 64;

    pub fn zones(&self) -> &[Zone] {
        &self.zones
    }

    /// Adds a zone; returns its index, or `None` when there are too many.
    pub fn add_zone(&mut self, zone: Zone) -> Option<usize> {
        if self.zones.len() >= Self::MAX_ZONES {
            return None;
        }
        self.zones.push(zone);
        self.clock += 1;
        Some(self.zones.len() - 1)
    }

    /// Edits a zone in place (marks the map as changed).
    pub fn edit_zone<R>(&mut self, index: usize, f: impl FnOnce(&mut Zone) -> R) -> Option<R> {
        let z = self.zones.get_mut(index)?;
        let r = f(z);
        self.clock += 1;
        Some(r)
    }

    pub fn remove_zone(&mut self, index: usize) -> bool {
        if index < self.zones.len() {
            self.zones.remove(index);
            self.clock += 1;
            true
        } else {
            false
        }
    }

    // ---- custom tiles -------------------------------------------------------------

    /// Free texture ids for custom tiles.
    pub const TILE_SLOTS: usize = (MAX_TEXTURES - TEXTURE_COUNT) as usize;

    pub fn tile(&self, id: u8) -> Option<&CustomTile> {
        self.tiles.get(id as usize).and_then(|t| t.as_ref())
    }

    /// Custom tiles with their texture ids.
    pub fn tiles(&self) -> impl Iterator<Item = (u8, &CustomTile)> {
        self.tiles.iter().enumerate().filter_map(|(i, t)| t.as_ref().map(|t| (i as u8, t)))
    }

    pub fn tile_id(&self, name: &str) -> Option<u8> {
        self.tiles().find(|(_, t)| t.name == name).map(|(i, _)| i)
    }

    /// Changes whenever a tile is added, changed or removed (never repeats
    /// across maps), so a renderer knows when to re-upload them.
    pub fn tiles_version(&self) -> u64 {
        self.tiles_version
    }

    /// Whether texture `id` draws overhang art next to it.
    pub fn has_overhang(&self, id: u8) -> bool {
        self.tile(id).is_some_and(|t| t.overhang.is_some())
    }

    pub fn any_overhang(&self) -> bool {
        self.tiles().any(|(_, t)| t.overhang.is_some())
    }

    /// Adds a tile, or replaces the tile of the same name. Returns its
    /// texture id, or an error when all slots are taken.
    pub fn add_tile(&mut self, tile: CustomTile) -> Result<u8, String> {
        if !crate::tile::valid_tile_name(&tile.name) {
            return Err(format!("\"{}\" is not a usable tile name", tile.name));
        }
        let id = match self.tile_id(&tile.name) {
            Some(id) => id,
            None => (TEXTURE_COUNT + 1..=MAX_TEXTURES)
                .find(|&i| self.tiles[i as usize].is_none())
                .ok_or_else(|| format!("This map already has {} custom tiles", Self::TILE_SLOTS))?,
        };
        if self.tiles[id as usize].as_ref() != Some(&tile) {
            self.tiles[id as usize] = Some(tile);
            self.tiles_version = crate::colors::next_version();
            self.clock += 1;
        }
        Ok(id)
    }

    /// How many cells (over every depth) use texture `id`.
    pub fn cells_using(&self, id: u8) -> usize {
        self.layers
            .iter()
            .flat_map(|l| l.values())
            .map(|c| c.cells().iter().filter(|&&m| crate::chunk::cell_texture(m) == id).count())
            .sum()
    }

    /// Colour indices that at least one cell (of any depth) is drawn with,
    /// in table order.
    pub fn used_colors(&self) -> Vec<u8> {
        let mut used = [false; 256];
        for chunk in self.layers.iter().flat_map(|l| l.values()) {
            for &c in chunk.cells() {
                if crate::chunk::cell_texture(c) != 0 {
                    used[crate::chunk::cell_color(c) as usize] = true;
                }
            }
        }
        (0..=255u8).filter(|&i| used[i as usize]).collect()
    }

    /// Horizontal runs `(y, x0, x1)` of cells in the cell rectangle `min..=max`
    /// of `depth` that are drawn with colour index `color`. Stops after
    /// `max_runs` runs.
    pub fn color_runs(
        &self,
        depth: u8,
        color: u8,
        min: (i64, i64),
        max: (i64, i64),
        max_runs: usize,
    ) -> Vec<(i64, i64, i64)> {
        use crate::{CHUNK_SHIFT, CHUNK_SIZE};
        let mut runs: Vec<(i64, i64, i64)> = Vec::new();
        if max.0 < min.0 || max.1 < min.1 {
            return runs;
        }
        let lo = ChunkPos { x: min.0 >> CHUNK_SHIFT, y: min.1 >> CHUNK_SHIFT };
        let hi = ChunkPos { x: max.0 >> CHUNK_SHIFT, y: max.1 >> CHUNK_SHIFT };
        self.visit_chunks(depth, lo, hi, |pos, chunk| {
            let (ox, oy) = (pos.x << CHUNK_SHIFT, pos.y << CHUNK_SHIFT);
            let cells = chunk.cells();
            for ly in 0..CHUNK_SIZE {
                let y = oy + ly;
                if y < min.1 || y > max.1 {
                    continue;
                }
                let mut start: Option<i64> = None;
                for lx in 0..=CHUNK_SIZE {
                    let x = ox + lx;
                    let hit = lx < CHUNK_SIZE && x >= min.0 && x <= max.0 && {
                        let c = cells[(ly * CHUNK_SIZE + lx) as usize];
                        crate::chunk::cell_texture(c) != 0 && crate::chunk::cell_color(c) == color
                    };
                    match (hit, start) {
                        (true, None) => start = Some(x),
                        (false, Some(s)) => {
                            if runs.len() < max_runs {
                                runs.push((y, s, x - 1));
                            }
                            start = None;
                        }
                        _ => {}
                    }
                }
            }
        });
        runs
    }

    /// Removes a custom tile. The cells that used it stay, drawn as nothing.
    pub fn remove_tile(&mut self, id: u8) -> bool {
        match self.tiles.get_mut(id as usize) {
            Some(slot) if id > TEXTURE_COUNT && slot.is_some() => {
                *slot = None;
                self.tiles_version = crate::colors::next_version();
                self.clock += 1;
                true
            }
            _ => false,
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

    /// What lowering the map's depth to `max_depth` would throw away:
    /// `(chunks of detail, nodes)`.
    pub fn depth_loss(&self, max_depth: u8) -> (usize, usize) {
        let chunks = self
            .layers
            .iter()
            .enumerate()
            .filter(|(d, _)| *d > max_depth as usize)
            .map(|(_, l)| l.len())
            .sum();
        let nodes = self.nodes.iter().filter(|n| n.depth > max_depth).count();
        (chunks, nodes)
    }

    /// Changes how many times the map can be zoomed into. Making it deeper
    /// adds empty layers; making it shallower deletes the layers (and the
    /// nodes) below the new depth. Returns what was deleted, like
    /// [`World::depth_loss`].
    pub fn set_max_depth(&mut self, max_depth: u8) -> (usize, usize) {
        let max_depth = max_depth.min(MAX_DEPTH);
        if max_depth == self.spec.max_depth {
            return (0, 0);
        }
        let lost = self.depth_loss(max_depth);
        for layer in self.layers.iter_mut().skip(max_depth as usize + 1) {
            layer.clear();
        }
        self.nodes.retain(|n| n.depth <= max_depth);
        self.spec.max_depth = max_depth;
        self.clock += 1;
        lost
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
    fn color_runs_merge_neighbours_and_respect_the_window() {
        let mut w = World::with_spec(MapSpec::new(200, 200, 1));
        for x in 3..=6 {
            w.set(0, x, 2, crate::make_cell(3, 5));
        }
        w.set(0, 9, 2, crate::make_cell(3, 5));
        w.set(0, 4, 3, crate::make_cell(3, 6));
        w.set(0, 100, 2, crate::make_cell(3, 5)); // another chunk
        let all = w.color_runs(0, 5, (0, 0), (150, 10), 100);
        assert_eq!(all, vec![(2, 3, 6), (2, 9, 9), (2, 100, 100)]);
        // The window clips runs.
        assert_eq!(w.color_runs(0, 5, (5, 0), (9, 10), 100), vec![(2, 5, 6), (2, 9, 9)]);
        assert_eq!(w.color_runs(0, 6, (0, 0), (10, 10), 100), vec![(3, 4, 4)]);
        assert_eq!(w.color_runs(0, 5, (0, 0), (150, 10), 1).len(), 1, "capped");
    }

    #[test]
    fn used_colors_lists_only_colours_cells_use() {
        let mut w = World::with_spec(MapSpec::new(64, 64, 2));
        assert!(w.used_colors().is_empty());
        w.set(0, 1, 1, crate::make_cell(3, 5));
        w.set(1, 9, 9, crate::make_cell(3, 2));
        w.set(0, 2, 2, crate::make_cell(3, 5));
        assert_eq!(w.used_colors(), vec![2, 5]);
        w.set(0, 1, 1, 0);
        w.set(0, 2, 2, 0);
        assert_eq!(w.used_colors(), vec![2], "erased cells stop counting");
        w.colors.index_of([7, 7, 7]);
        assert_eq!(w.used_colors(), vec![2], "table colours nothing uses are not listed");
    }

    #[test]
    fn changing_the_depth_adds_layers_or_deletes_the_deeper_ones() {
        let mut w = World::with_spec(MapSpec::new(8, 8, 2));
        w.set(0, 1, 1, 0x0102);
        w.set(2, 5, 5, 0x0102);
        w.set(2, 6, 5, 0x0102);
        assert!(w.put_node(Node::new(1, 2, 3, 3, Icon::default())));
        assert!(w.put_node(Node::new(2, 0, 3, 3, Icon::default())));
        let rev = w.revision();

        assert_eq!(w.set_max_depth(2), (0, 0), "same depth: nothing happens");
        assert_eq!(w.revision(), rev);

        assert_eq!(w.set_max_depth(4), (0, 0), "deeper loses nothing");
        assert_eq!(w.spec().max_depth, 4);
        w.set(4, 100, 100, 0x0102);
        assert_ne!(w.get(4, 100, 100), 0);
        assert!(w.revision() > rev);

        assert_eq!(w.depth_loss(1), (2, 1));
        assert_eq!(w.set_max_depth(1), (2, 1));
        assert_eq!(w.get(2, 5, 5), 0);
        assert!(w.node(1).is_none() && w.node(2).is_some());
        assert_ne!(w.get(0, 1, 1), 0, "shallower layers stay");
        assert!(!w.in_bounds(2, 0, 0));
        w.set(2, 5, 5, 0x0102);
        assert_eq!(w.get(2, 5, 5), 0, "the deleted depth no longer exists");
    }

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
