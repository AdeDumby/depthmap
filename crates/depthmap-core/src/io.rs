//! Save files: a small header followed by one zstd-compressed payload.
//!
//! ```text
//! "DMAP" | version u32 | raw_len u64 | zstd( payload )
//! payload = width u32 | height u32 | max_depth u8          (version 3 only)
//!         | icons u16 | { name_len u16 | name | len u32 | png bytes }*   (version 4)
//!         | nodes u32 | { id u64 | depth u8 | x i64 | y i64 | scale f32
//!                       | snap u8 | edge u8                              (version 5)
//!                       | icon_kind u8 (0 built-in, 1 imported) | built-in id u8
//!                         or name_len u16 + name | name_len u16 + name
//!                       | desc_len u32 + desc }*                         (version 4)
//!         | tiles u8 | { id u8 | name_len u16 + name | flags u8 (1 = overhang)
//!                      | art[2048] | overhang art[2048] if flagged }*   (version 6)
//!         | zones u16 | { name_len u16 + name | rgb[3] | visible u8 | depth u8
//!                     | rows u32 | { y i64 | spans u32 | { x0 i64, x1 i64 }* }* }*   (version 7)
//!         | routes u16 | { name_len u16 + name | rgb[3] | width u8 | style u8 | visible u8 | depth u8
//!                      | from u64 | to u64 | via u16 | { x f64, y f64 }* }*   (version 8)
//!         | color_count u16 | { r g b }*color_count
//!         | { depth u8 | count u32 | { x i64 | y i64 | cells[1024] u16 }* }*
//! ```
//! All integers are little-endian. A cell is `texture | color << 8`.
//! `width` x `height` is the map size in depth-0 cells and `max_depth` how
//! many times it can be zoomed into (see `MapSpec`); `0 x 0` means unbounded.
//! Version 2 files have no size line and load as unbounded maps.

use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::Path;

use crate::chunk::{Cell, ChunkPos, EMPTY, cell_color, cell_texture, normalize};
use crate::colors::{ColorTable, MAX_COLORS};
use crate::icons;
use crate::node::{Icon, Node, Snap};
use crate::spec::MapSpec;
use crate::texture::{MAX_TEXTURES, TEXTURE_COUNT};
use crate::selection::{MAX_SELECTION_ROWS, Rows, Selection};
use crate::route::{MAX_ROUTES, MAX_VIA, MAX_WIDTH, MIN_WIDTH, Route, RouteStyle};
use crate::zone::Zone;
use crate::tile::{CustomTile, TILE_PIXELS, TileArt};
use crate::{CHUNK_AREA, MAX_DEPTH, World};

const MAGIC: &[u8; 4] = b"DMAP";
const VERSION: u32 = 8;
/// Oldest version that can still be read.
const OLDEST: u32 = 2;

fn invalid(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

fn put_str16(out: &mut Vec<u8>, s: &str) {
    let bytes = &s.as_bytes()[..s.len().min(u16::MAX as usize)];
    out.extend((bytes.len() as u16).to_le_bytes());
    out.extend(bytes);
}

pub fn write_world<W: Write>(world: &World, mut out: W) -> io::Result<()> {
    let mut raw = Vec::new();
    let spec = world.spec();
    raw.extend(spec.width.to_le_bytes());
    raw.extend(spec.height.to_le_bytes());
    raw.push(spec.max_depth);
    let used = world.used_custom_icons();
    raw.extend((used.len() as u16).to_le_bytes());
    for (name, icon) in used {
        put_str16(&mut raw, name);
        raw.extend((icon.png.len() as u32).to_le_bytes());
        raw.extend(icon.png.iter());
    }
    raw.extend((world.nodes().len() as u32).to_le_bytes());
    for n in world.nodes() {
        raw.extend(n.id.to_le_bytes());
        raw.push(n.depth);
        raw.extend(n.x.to_le_bytes());
        raw.extend(n.y.to_le_bytes());
        raw.extend(n.scale.to_le_bytes());
        raw.push(n.snap.to_byte());
        raw.push(n.edge as u8);
        match &n.icon {
            Icon::Builtin(id) => {
                raw.push(0);
                raw.push(*id);
            }
            Icon::Custom(name) => {
                raw.push(1);
                put_str16(&mut raw, name);
            }
        }
        put_str16(&mut raw, &n.name);
        raw.extend((n.description.len() as u32).to_le_bytes());
        raw.extend(n.description.as_bytes());
    }
    let tiles: Vec<_> = world.tiles().collect();
    raw.push(tiles.len() as u8);
    for (id, t) in tiles {
        raw.push(id);
        put_str16(&mut raw, &t.name);
        raw.push(t.overhang.is_some() as u8);
        raw.extend(&t.art.la);
        if let Some(o) = &t.overhang {
            raw.extend(&o.la);
        }
    }
    raw.extend((world.zones().len() as u16).to_le_bytes());
    for z in world.zones() {
        put_str16(&mut raw, &z.name);
        raw.extend(z.color);
        raw.push(z.visible as u8);
        raw.push(z.depth());
        raw.extend((z.area().rows().len() as u32).to_le_bytes());
        for (&y, spans) in z.area().rows() {
            raw.extend(y.to_le_bytes());
            raw.extend((spans.len() as u32).to_le_bytes());
            for &(a, b) in spans {
                raw.extend(a.to_le_bytes());
                raw.extend(b.to_le_bytes());
            }
        }
    }
    raw.extend((world.routes().len() as u16).to_le_bytes());
    for r in world.routes() {
        put_str16(&mut raw, &r.name);
        raw.extend(r.color);
        raw.push(r.width);
        raw.push(r.style.to_byte());
        raw.push(r.visible as u8);
        raw.push(r.depth);
        raw.extend(r.from.to_le_bytes());
        raw.extend(r.to.to_le_bytes());
        raw.extend((r.via.len() as u16).to_le_bytes());
        for &(x, y) in &r.via {
            raw.extend(x.to_le_bytes());
            raw.extend(y.to_le_bytes());
        }
    }
    let colors = world.colors.as_slice();
    raw.extend((colors.len() as u16).to_le_bytes());
    for c in colors {
        raw.extend(c);
    }
    for depth in 0..=MAX_DEPTH {
        let n = world.chunk_count(depth);
        if n == 0 {
            continue;
        }
        raw.push(depth);
        raw.extend((n as u32).to_le_bytes());
        for (pos, chunk) in world.iter_chunks(depth) {
            raw.extend(pos.x.to_le_bytes());
            raw.extend(pos.y.to_le_bytes());
            for cell in chunk.cells() {
                raw.extend(cell.to_le_bytes());
            }
        }
    }
    let packed = zstd::encode_all(&raw[..], 3)?;
    out.write_all(MAGIC)?;
    out.write_all(&VERSION.to_le_bytes())?;
    out.write_all(&(raw.len() as u64).to_le_bytes())?;
    out.write_all(&packed)?;
    out.flush()
}

pub fn read_world<R: Read>(mut input: R) -> io::Result<World> {
    let mut header = [0u8; 16];
    input.read_exact(&mut header)?;
    if &header[0..4] != MAGIC {
        return Err(invalid("not a depthmap file"));
    }
    let version = u32::from_le_bytes(header[4..8].try_into().unwrap());
    if !(OLDEST..=VERSION).contains(&version) {
        return Err(invalid("unsupported depthmap file version"));
    }
    let raw_len = u64::from_le_bytes(header[8..16].try_into().unwrap()) as usize;

    let mut packed = Vec::new();
    input.read_to_end(&mut packed)?;
    let raw = zstd::decode_all(&packed[..])?;
    if raw.len() != raw_len {
        return Err(invalid("payload length mismatch"));
    }

    struct Cursor<'a> {
        data: &'a [u8],
        at: usize,
    }
    impl<'a> Cursor<'a> {
        fn take(&mut self, n: usize) -> io::Result<&'a [u8]> {
            let end = self.at.checked_add(n).filter(|&e| e <= self.data.len());
            let end = end.ok_or_else(|| invalid("truncated payload"))?;
            let s = &self.data[self.at..end];
            self.at = end;
            Ok(s)
        }

        fn string16(&mut self) -> io::Result<String> {
            let len = u16::from_le_bytes(self.take(2)?.try_into().unwrap()) as usize;
            String::from_utf8(self.take(len)?.to_vec()).map_err(|_| invalid("text is not valid UTF-8"))
        }
    }
    let mut cur = Cursor { data: &raw, at: 0 };

    let spec = if version >= 3 {
        let width = u32::from_le_bytes(cur.take(4)?.try_into().unwrap());
        let height = u32::from_le_bytes(cur.take(4)?.try_into().unwrap());
        let spec = MapSpec { width, height, max_depth: cur.take(1)?[0] };
        if !spec.is_valid() {
            return Err(invalid("bad map size"));
        }
        spec
    } else {
        MapSpec::UNBOUNDED
    };

    // Imported icons and nodes (version 4).
    let mut icon_blobs: Vec<(String, Vec<u8>)> = Vec::new();
    let mut node_list: Vec<Node> = Vec::new();
    if version >= 4 {
        let count = u16::from_le_bytes(cur.take(2)?.try_into().unwrap()) as usize;
        for _ in 0..count {
            let name = cur.string16()?;
            let len = u32::from_le_bytes(cur.take(4)?.try_into().unwrap()) as usize;
            let bytes = cur.take(len)?.to_vec();
            if !icons::valid_icon_name(&name) || bytes.len() > icons::MAX_PNG_BYTES {
                return Err(invalid("bad embedded icon"));
            }
            icon_blobs.push((name, bytes));
        }
        let count = u32::from_le_bytes(cur.take(4)?.try_into().unwrap()) as usize;
        for _ in 0..count {
            let id = u64::from_le_bytes(cur.take(8)?.try_into().unwrap());
            let depth = cur.take(1)?[0];
            let x = i64::from_le_bytes(cur.take(8)?.try_into().unwrap());
            let y = i64::from_le_bytes(cur.take(8)?.try_into().unwrap());
            let scale = f32::from_le_bytes(cur.take(4)?.try_into().unwrap());
            // Older files had every icon centred on its cell.
            let (snap, edge) = if version >= 5 {
                let snap = Snap::from_byte(cur.take(1)?[0]).ok_or_else(|| invalid("bad snap mode"))?;
                (snap, cur.take(1)?[0] != 0)
            } else {
                (Snap::Centre, false)
            };
            let icon = match cur.take(1)?[0] {
                0 => Icon::Builtin(cur.take(1)?[0]),
                1 => {
                    let name = cur.string16()?;
                    if !icons::valid_icon_name(&name) {
                        return Err(invalid("bad icon name"));
                    }
                    Icon::Custom(name)
                }
                _ => return Err(invalid("unknown icon kind")),
            };
            let name = cur.string16()?;
            let len = u32::from_le_bytes(cur.take(4)?.try_into().unwrap()) as usize;
            let description = String::from_utf8(cur.take(len)?.to_vec())
                .map_err(|_| invalid("text is not valid UTF-8"))?;
            node_list.push(Node { id, depth, x, y, name, description, icon, scale, snap, edge });
        }
    }

    // Custom tiles (version 6). Ids outside the free range or repeated are refused.
    let mut tile_list: Vec<(u8, CustomTile)> = Vec::new();
    if version >= 6 {
        let count = cur.take(1)?[0] as usize;
        for _ in 0..count {
            let id = cur.take(1)?[0];
            if !(TEXTURE_COUNT + 1..=MAX_TEXTURES).contains(&id) || tile_list.iter().any(|t| t.0 == id) {
                return Err(invalid("bad tile id"));
            }
            let name = cur.string16()?;
            if !crate::tile::valid_tile_name(&name) {
                return Err(invalid("bad tile name"));
            }
            let flags = cur.take(1)?[0];
            let art = TileArt::from_bytes(cur.take(TILE_PIXELS * 2)?).ok_or_else(|| invalid("bad tile"))?;
            let overhang = if flags & 1 != 0 {
                Some(TileArt::from_bytes(cur.take(TILE_PIXELS * 2)?).ok_or_else(|| invalid("bad tile"))?)
            } else {
                None
            };
            tile_list.push((id, CustomTile { name, art, overhang }));
        }
    }

    // Zones (version 7).
    let mut zone_list: Vec<Zone> = Vec::new();
    if version >= 7 {
        let count = u16::from_le_bytes(cur.take(2)?.try_into().unwrap()) as usize;
        if count > World::MAX_ZONES {
            return Err(invalid("too many zones"));
        }
        for _ in 0..count {
            let name = cur.string16()?;
            let rgb: [u8; 3] = cur.take(3)?.try_into().unwrap();
            let visible = cur.take(1)?[0] != 0;
            let depth = cur.take(1)?[0];
            if depth > spec.max_depth {
                return Err(invalid("bad zone depth"));
            }
            let row_count = u32::from_le_bytes(cur.take(4)?.try_into().unwrap()) as usize;
            if row_count > MAX_SELECTION_ROWS {
                return Err(invalid("zone too large"));
            }
            let mut rows = Rows::new();
            for _ in 0..row_count {
                let y = i64::from_le_bytes(cur.take(8)?.try_into().unwrap());
                let n = u32::from_le_bytes(cur.take(4)?.try_into().unwrap()) as usize;
                let mut spans: Vec<(i64, i64)> = Vec::new();
                for _ in 0..n {
                    let a = i64::from_le_bytes(cur.take(8)?.try_into().unwrap());
                    let b = i64::from_le_bytes(cur.take(8)?.try_into().unwrap());
                    if a > b || spans.last().is_some_and(|&(_, l)| l.saturating_add(1) >= a) {
                        return Err(invalid("bad zone spans"));
                    }
                    spans.push((a, b));
                }
                if rows.insert(y, spans).is_some() || n == 0 {
                    return Err(invalid("bad zone rows"));
                }
            }
            zone_list.push(Zone::from_area(&name, rgb, visible, Selection::from_rows(depth, rows)));
        }
    }

    // Routes (version 8).
    let mut route_list: Vec<Route> = Vec::new();
    if version >= 8 {
        let count = u16::from_le_bytes(cur.take(2)?.try_into().unwrap()) as usize;
        if count > MAX_ROUTES {
            return Err(invalid("too many routes"));
        }
        for _ in 0..count {
            let name = cur.string16()?;
            let color: [u8; 3] = cur.take(3)?.try_into().unwrap();
            let width = cur.take(1)?[0];
            let style = RouteStyle::from_byte(cur.take(1)?[0]).ok_or_else(|| invalid("bad route style"))?;
            let visible = cur.take(1)?[0] != 0;
            let depth = cur.take(1)?[0];
            if !(MIN_WIDTH..=MAX_WIDTH).contains(&width) || depth > spec.max_depth {
                return Err(invalid("bad route"));
            }
            let from = u64::from_le_bytes(cur.take(8)?.try_into().unwrap());
            let to = u64::from_le_bytes(cur.take(8)?.try_into().unwrap());
            let n = u16::from_le_bytes(cur.take(2)?.try_into().unwrap()) as usize;
            if n > MAX_VIA {
                return Err(invalid("too many route points"));
            }
            let mut via = Vec::with_capacity(n);
            for _ in 0..n {
                let x = f64::from_le_bytes(cur.take(8)?.try_into().unwrap());
                let y = f64::from_le_bytes(cur.take(8)?.try_into().unwrap());
                if !x.is_finite() || !y.is_finite() {
                    return Err(invalid("bad route point"));
                }
                via.push((x, y));
            }
            let mut r = Route::new(&name, color, depth, from, to);
            (r.width, r.style, r.visible, r.via) = (width, style, visible, via);
            route_list.push(r);
        }
    }

    let color_count = u16::from_le_bytes(cur.take(2)?.try_into().unwrap()) as usize;
    if color_count == 0 || color_count > MAX_COLORS {
        return Err(invalid("bad colour table size"));
    }
    let colors: Vec<[u8; 3]> = cur
        .take(color_count * 3)?
        .chunks_exact(3)
        .map(|c| [c[0], c[1], c[2]])
        .collect();

    let mut world = World::with_spec(spec);
    world.colors = ColorTable::from_vec(colors);
    let mut sections = 0;
    while cur.at < raw.len() {
        sections += 1;
        if sections > MAX_DEPTH as usize + 1 {
            return Err(invalid("too many depth sections"));
        }
        let depth = cur.take(1)?[0];
        if depth > spec.max_depth {
            return Err(invalid("depth out of range"));
        }
        let extent = spec.extent(depth);
        let n = u32::from_le_bytes(cur.take(4)?.try_into().unwrap());
        for _ in 0..n {
            let x = i64::from_le_bytes(cur.take(8)?.try_into().unwrap());
            let y = i64::from_le_bytes(cur.take(8)?.try_into().unwrap());
            let bytes = cur.take(CHUNK_AREA * 2)?;
            let mut cells = [EMPTY; CHUNK_AREA];
            // Chunks poking out of the map (hand-edited or corrupt files)
            // are trimmed to it.
            let inside = |cx: i64, cy: i64| match extent {
                Some((w, h)) => cx >= 0 && cy >= 0 && cx < w && cy < h,
                None => true,
            };
            let (ox, oy) = (x.wrapping_shl(crate::CHUNK_SHIFT), y.wrapping_shl(crate::CHUNK_SHIFT));
            let last = crate::CHUNK_SIZE - 1;
            let whole = inside(ox, oy)
                && inside(ox.saturating_add(last), oy.saturating_add(last));
            for (i, (cell, b)) in cells.iter_mut().zip(bytes.chunks_exact(2)).enumerate() {
                if !whole {
                    let lx = (i as i64) & (crate::CHUNK_SIZE - 1);
                    let ly = (i as i64) >> crate::CHUNK_SHIFT;
                    if !inside(ox.saturating_add(lx), oy.saturating_add(ly)) {
                        continue;
                    }
                }
                let c: Cell = normalize(u16::from_le_bytes([b[0], b[1]]));
                // A colour index past the table would render as garbage.
                *cell = if cell_color(c) as usize >= color_count && c != EMPTY {
                    crate::chunk::make_cell(cell_texture(c), 0)
                } else {
                    c
                };
            }
            world.insert_chunk(depth, ChunkPos { x, y }, &cells);
        }
    }
    for r in route_list {
        world.add_route(r).ok_or_else(|| invalid("too many routes"))?;
    }
    for z in zone_list {
        world.add_zone(z).ok_or_else(|| invalid("too many zones"))?;
    }
    for (_, tile) in tile_list {
        // Saved ids are kept by adding in id order into the same free slots.
        world.add_tile(tile).map_err(|e| invalid(&e))?;
    }
    for (name, bytes) in icon_blobs {
        world.add_custom_icon(&name, bytes).map_err(|e| invalid(&e))?;
    }
    // Nodes outside the map or with a repeated id are dropped.
    for node in node_list {
        if world.node(node.id).is_none() {
            world.put_node(node);
        }
    }
    Ok(world)
}

/// Writes to a temporary file and renames it over `path`, so a crash while
/// saving can never leave a half-written map behind.
pub fn save(world: &World, path: &Path) -> io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = std::path::PathBuf::from(tmp);
    write_world(world, BufWriter::new(File::create(&tmp)?))?;
    fs::rename(tmp, path)
}

pub fn load(path: &Path) -> io::Result<World> {
    read_world(BufReader::new(File::open(path)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::make_cell;
    use crate::chunk::ChunkPos;

    fn sample() -> World {
        sample_in(World::new())
    }

    fn sample_in(mut w: World) -> World {
        for i in 0..2000i64 {
            w.set(0, i % 97 - 40, i / 13 - 70, make_cell((i % 14 + 1) as u8, (i % 16) as u8));
        }
        w.set(5, -1_000_000, 2_000_000, make_cell(3, 0));
        w.set(16, 1, 1, make_cell(9, 4));
        let custom = w.colors.index_of([12, 34, 56]);
        w.set(2, 3, 3, make_cell(2, custom));
        w
    }

    #[test]
    fn roundtrip_preserves_every_cell_and_the_colour_table() {
        let w = sample();
        let mut bytes = Vec::new();
        write_world(&w, &mut bytes).unwrap();
        let r = read_world(&bytes[..]).unwrap();
        assert_eq!(w.total_chunks(), r.total_chunks());
        assert_eq!(w.colors.as_slice(), r.colors.as_slice());
        assert_eq!(r.colors.get(cell_color(r.get(2, 3, 3))), [12, 34, 56]);
        for depth in 0..=MAX_DEPTH {
            assert_eq!(w.chunk_count(depth), r.chunk_count(depth));
            for (pos, chunk) in w.iter_chunks(depth) {
                assert_eq!(r.chunk(depth, pos).unwrap().cells(), chunk.cells());
            }
        }
    }

    #[test]
    fn map_size_and_depth_are_saved() {
        let spec = MapSpec::new(96, 40, 5);
        let mut w = World::with_spec(spec);
        w.set(0, 95, 39, make_cell(2, 0));
        w.set(5, 96 * 1024 - 1, 3, make_cell(4, 1));
        let mut bytes = Vec::new();
        write_world(&w, &mut bytes).unwrap();
        let r = read_world(&bytes[..]).unwrap();
        assert_eq!(r.spec(), spec);
        assert_eq!(r.get(0, 95, 39), make_cell(2, 0));
        assert_eq!(r.get(5, 96 * 1024 - 1, 3), make_cell(4, 1));
        // And an unbounded map stays unbounded.
        let mut bytes = Vec::new();
        write_world(&sample(), &mut bytes).unwrap();
        assert_eq!(read_world(&bytes[..]).unwrap().spec(), MapSpec::UNBOUNDED);
    }

    /// Builds a file by hand, with or without the version-3 size line.
    fn hand_built(version: u32, spec: Option<(u32, u32, u8)>, chunks: &[(u8, i64, i64, Vec<u16>)]) -> Vec<u8> {
        let mut raw = Vec::new();
        if let Some((w, h, d)) = spec {
            raw.extend(w.to_le_bytes());
            raw.extend(h.to_le_bytes());
            raw.push(d);
        }
        raw.extend(1u16.to_le_bytes());
        raw.extend([9, 9, 9]);
        for d in 0..=MAX_DEPTH {
            let here: Vec<_> = chunks.iter().filter(|c| c.0 == d).collect();
            if here.is_empty() {
                continue;
            }
            raw.push(d);
            raw.extend((here.len() as u32).to_le_bytes());
            for (_, x, y, cells) in here {
                raw.extend(x.to_le_bytes());
                raw.extend(y.to_le_bytes());
                for c in cells {
                    raw.extend(c.to_le_bytes());
                }
            }
        }
        let packed = zstd::encode_all(&raw[..], 1).unwrap();
        let mut file = Vec::new();
        file.extend(MAGIC);
        file.extend(version.to_le_bytes());
        file.extend((raw.len() as u64).to_le_bytes());
        file.extend(packed);
        file
    }

    #[test]
    fn nodes_and_their_imported_icons_round_trip() {
        let mut w = World::with_spec(MapSpec::new(64, 64, 3));
        w.add_custom_icon("dragon.png", vec![7, 7, 7, 7]).unwrap();
        w.add_custom_icon("unused.png", vec![1]).unwrap();
        let a = w.alloc_node_id();
        let mut n = Node::new(a, 2, 700, 33, Icon::Custom("dragon.png".into()));
        n.name = "Wyrm's lair \u{1F409}".into();
        n.description = "Line one\nLine two with ünïcode".into();
        n.scale = 2.5;
        n.snap = Snap::Edge;
        n.edge = true;
        w.put_node(n.clone());
        let b = w.alloc_node_id();
        w.put_node(Node::new(b, 0, 3, 4, Icon::Builtin(9)));

        let mut bytes = Vec::new();
        write_world(&w, &mut bytes).unwrap();
        let r = read_world(&bytes[..]).unwrap();
        assert_eq!(r.nodes(), w.nodes());
        assert_eq!(r.node(a).unwrap(), &n);
        assert_eq!(&**r.custom_icon("dragon.png").unwrap().png, &[7, 7, 7, 7]);
        assert!(r.custom_icon("unused.png").is_none(), "only used icons are embedded");
        let mut r = r;
        assert!(r.alloc_node_id() > b);
    }

    #[test]
    fn hostile_node_data_is_sanitised_or_rejected() {
        // Out-of-map nodes are dropped, scales clamped, long text cut.
        let mut w = World::with_spec(MapSpec::new(8, 8, 1));
        let mut ok = Node::new(1, 0, 2, 2, Icon::Builtin(1));
        ok.scale = 3.0;
        w.put_node(ok);
        let mut bytes = Vec::new();
        write_world(&w, &mut bytes).unwrap();
        let r = read_world(&bytes[..]).unwrap();
        assert_eq!(r.nodes().len(), 1);

        // Hand-built payload with an out-of-range node and a NaN scale.
        let mut raw = Vec::new();
        raw.extend(8u32.to_le_bytes());
        raw.extend(8u32.to_le_bytes());
        raw.push(1);
        raw.extend(0u16.to_le_bytes()); // icons
        raw.extend(3u32.to_le_bytes()); // nodes
        for (id, depth, x, scale) in [(1u64, 0u8, 99i64, 1.0f32), (2, 0, 1, f32::NAN), (2, 0, 3, 1.0)] {
            raw.extend(id.to_le_bytes());
            raw.push(depth);
            raw.extend(x.to_le_bytes());
            raw.extend(1i64.to_le_bytes());
            raw.extend(scale.to_le_bytes());
            raw.extend([0, 5]); // built-in 5
            raw.extend(0u16.to_le_bytes()); // name
            raw.extend(0u32.to_le_bytes()); // description
        }
        raw.extend(1u16.to_le_bytes());
        raw.extend([9, 9, 9]);
        let packed = zstd::encode_all(&raw[..], 1).unwrap();
        let mut file = Vec::new();
        file.extend(MAGIC);
        file.extend(4u32.to_le_bytes());
        file.extend((raw.len() as u64).to_le_bytes());
        file.extend(packed);
        let r = read_world(&file[..]).unwrap();
        assert_eq!(r.nodes().len(), 1, "x=99 is off the map, the repeated id is dropped");
        assert_eq!(r.nodes()[0].x, 1);
        assert_eq!(r.nodes()[0].scale, 1.0, "NaN scale replaced");

        // Truncating anywhere in the node block is an error, not a panic.
        let mut bytes = Vec::new();
        write_world(&sample_with_nodes(), &mut bytes).unwrap();
        for cut in [bytes.len() - 5, bytes.len() / 2, 20] {
            assert!(read_world(&bytes[..cut]).is_err());
        }
    }

    fn sample_with_nodes() -> World {
        let mut w = sample();
        for i in 0..5 {
            let id = w.alloc_node_id();
            let mut n = Node::new(id, 0, i, i, Icon::Builtin(2));
            n.name = format!("node {i}");
            w.put_node(n);
        }
        w
    }

    #[test]
    fn version_3_files_open_without_nodes() {
        let file = hand_built(3, Some((8, 8, 1)), &[]);
        let w = read_world(&file[..]).unwrap();
        assert!(w.nodes().is_empty());
    }

    #[test]
    fn version_2_files_still_open_as_unbounded_maps() {
        let mut cells = vec![0u16; CHUNK_AREA];
        cells[5] = 3;
        let file = hand_built(2, None, &[(7, -4, 9, cells)]);
        let w = read_world(&file[..]).unwrap();
        assert_eq!(w.spec(), MapSpec::UNBOUNDED);
        assert_eq!(w.get(7, -4 * 32 + 5, 9 * 32), 3);
    }

    #[test]
    fn cells_outside_the_saved_map_are_dropped_on_load() {
        let spec = Some((40, 40, 1));
        let mut cells = vec![0u16; CHUNK_AREA];
        for c in cells.iter_mut() {
            *c = 2;
        }
        // Chunk (1,0) covers x 32..64 but the map is only 40 wide.
        let file = hand_built(3, spec, &[(0, 1, 0, cells.clone()), (0, 5, 5, cells.clone())]);
        let w = read_world(&file[..]).unwrap();
        assert_eq!(w.get(0, 39, 0), 2);
        assert_eq!(w.get(0, 40, 0), EMPTY);
        assert_eq!(w.get(0, 39, 32), EMPTY, "below the map");
        assert!(w.chunk(0, ChunkPos { x: 5, y: 5 }).is_none(), "wholly outside");
        // A section deeper than the map allows is an error.
        let file = hand_built(3, spec, &[(2, 0, 0, cells)]);
        assert!(read_world(&file[..]).is_err());
    }

    #[test]
    fn nonsense_map_sizes_are_rejected() {
        for bad in [(0, 8, 1), (8, 0, 1), (5000, 8, 1), (8, 8, 40), (2, 8, 1)] {
            let file = hand_built(3, Some(bad), &[]);
            assert!(read_world(&file[..]).is_err(), "{bad:?}");
        }
        let file = hand_built(VERSION + 1, Some((8, 8, 1)), &[]);
        assert!(read_world(&file[..]).is_err(), "future versions are refused");
    }

    #[test]
    fn empty_world_roundtrips() {
        let mut bytes = Vec::new();
        write_world(&World::new(), &mut bytes).unwrap();
        let r = read_world(&bytes[..]).unwrap();
        assert_eq!(r.total_chunks(), 0);
        assert_eq!(r.colors.len(), crate::colors::DEFAULT_COLORS.len());
    }

    #[test]
    fn corrupt_input_is_an_error_not_a_panic() {
        let mut bytes = Vec::new();
        write_world(&sample(), &mut bytes).unwrap();

        assert!(read_world(&b"nope"[..]).is_err());
        let mut bad_magic = bytes.clone();
        bad_magic[0] = b'X';
        assert!(read_world(&bad_magic[..]).is_err());
        let mut bad_version = bytes.clone();
        bad_version[4] = 1; // v1 files (u8 materials) are not readable
        assert!(read_world(&bad_version[..]).is_err());
        let mut bad_len = bytes.clone();
        bad_len[8] ^= 0xFF;
        assert!(read_world(&bad_len[..]).is_err());
        assert!(read_world(&bytes[..bytes.len() / 2]).is_err());
    }

    #[test]
    fn out_of_range_textures_and_colours_are_sanitised() {
        // Hand-build a (version 2) payload: 1 colour, one chunk with hostile cells.
        let mut raw = Vec::new();
        raw.extend(1u16.to_le_bytes());
        raw.extend([9, 9, 9]);
        raw.push(0);
        raw.extend(1u32.to_le_bytes());
        raw.extend(0i64.to_le_bytes());
        raw.extend(0i64.to_le_bytes());
        for i in 0..CHUNK_AREA {
            let cell: u16 = match i {
                0 => 2 | (50 << 8),  // colour index beyond the table
                1 => 250,            // texture beyond the known range
                _ => 0,
            };
            raw.extend(cell.to_le_bytes());
        }
        let packed = zstd::encode_all(&raw[..], 1).unwrap();
        let mut file = Vec::new();
        file.extend(MAGIC);
        file.extend(2u32.to_le_bytes());
        file.extend((raw.len() as u64).to_le_bytes());
        file.extend(packed);

        let w = read_world(&file[..]).unwrap();
        assert_eq!(w.get(0, 0, 0), make_cell(2, 0));
        assert_eq!(w.get(0, 1, 0), EMPTY);
    }

    #[test]
    fn save_and_load_through_a_file() {
        let dir = std::env::temp_dir().join(format!("depthmap-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("m.dmap");
        let w = sample();
        save(&w, &path).unwrap();
        assert!(!dir.join("m.dmap.tmp").exists());
        let r = load(&path).unwrap();
        assert_eq!(r.get(0, 0, 0), w.get(0, 0, 0));
        assert_eq!(r.get(16, 1, 1), make_cell(9, 4));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn custom_tiles_round_trip_with_their_ids() {
        use crate::tile::{CustomTile, TileArt};
        let mut w = World::new();
        let mut art = TileArt::solid(120);
        art.set(3, 4, 200, 17);
        let mut t = CustomTile::new("Forest", art);
        t.overhang = Some(TileArt::solid(60));
        let a = w.add_tile(t.clone()).unwrap();
        let b = w.add_tile(CustomTile::new("Rug", TileArt::solid(90))).unwrap();
        assert_eq!((a, b), (15, 16));
        let mut bytes = Vec::new();
        write_world(&w, &mut bytes).unwrap();
        let r = read_world(&bytes[..]).unwrap();
        assert_eq!(r.tile(a), Some(&t));
        assert_eq!(r.tile_id("Rug"), Some(b));
        assert!(r.has_overhang(a) && !r.has_overhang(b));
    }

    #[test]
    fn tiles_replace_by_name_and_run_out_of_slots() {
        use crate::tile::{CustomTile, TileArt};
        let mut w = World::new();
        let v = w.tiles_version();
        let id = w.add_tile(CustomTile::new("A", TileArt::solid(1))).unwrap();
        assert_ne!(w.tiles_version(), v);
        assert_eq!(w.add_tile(CustomTile::new("A", TileArt::solid(2))).unwrap(), id);
        for i in 1..World::TILE_SLOTS {
            w.add_tile(CustomTile::new(&format!("T{i}"), TileArt::solid(5))).unwrap();
        }
        assert!(w.add_tile(CustomTile::new("Extra", TileArt::solid(5))).is_err());
        assert!(w.remove_tile(id));
        assert!(w.add_tile(CustomTile::new("Extra", TileArt::solid(5))).is_ok());
    }

    #[test]
    fn zones_round_trip() {
        use crate::selection::Selection;
        use crate::zone::Zone;
        let mut w = World::new();
        let mut z = Zone::new("Old Keep", [10, 20, 30], 1);
        z.add(&Selection::rect(1, (2, 3), (9, 5)));
        z.subtract(&Selection::rect(1, (4, 4), (5, 4)));
        z.visible = false;
        w.add_zone(z.clone());
        w.add_zone(Zone::new("Empty", [1, 2, 3], 0));
        let mut bytes = Vec::new();
        write_world(&w, &mut bytes).unwrap();
        let r = read_world(&bytes[..]).unwrap();
        assert_eq!(r.zones().len(), 2);
        assert_eq!(r.zones()[0], z);
        assert_eq!(r.zones()[1].name, "Empty");
    }

    #[test]
    fn routes_round_trip() {
        use crate::route::{Route, RouteStyle};
        let mut w = World::new();
        let mut r = Route::new("King's Road", [1, 2, 3], 1, 4, 9);
        r.via = vec![(10.5, 20.25), (-3.0, 7.0)];
        r.width = 7;
        r.style = RouteStyle::Dashed;
        r.visible = false;
        w.add_route(r.clone());
        let mut bytes = Vec::new();
        write_world(&w, &mut bytes).unwrap();
        let back = read_world(&bytes[..]).unwrap();
        assert_eq!(back.routes(), &[r]);
    }
}
