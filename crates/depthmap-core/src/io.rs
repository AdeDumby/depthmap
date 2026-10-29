//! Save files: a small header followed by one zstd-compressed payload.
//!
//! ```text
//! "DMAP" | version u32 | raw_len u64 | zstd( payload )
//! payload = color_count u16 | { r g b }*color_count
//!         | { depth u8 | count u32 | { x i64 | y i64 | cells[1024] u16 }* }*
//! ```
//! All integers are little-endian. A cell is `texture | color << 8`.

use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::Path;

use crate::chunk::{Cell, ChunkPos, EMPTY, cell_color, cell_texture, normalize};
use crate::colors::{ColorTable, MAX_COLORS};
use crate::{CHUNK_AREA, MAX_DEPTH, World};

const MAGIC: &[u8; 4] = b"DMAP";
const VERSION: u32 = 2;

fn invalid(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

pub fn write_world<W: Write>(world: &World, mut out: W) -> io::Result<()> {
    let mut raw = Vec::new();
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
    if version != VERSION {
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
    }
    let mut cur = Cursor { data: &raw, at: 0 };

    let color_count = u16::from_le_bytes(cur.take(2)?.try_into().unwrap()) as usize;
    if color_count == 0 || color_count > MAX_COLORS {
        return Err(invalid("bad colour table size"));
    }
    let colors: Vec<[u8; 3]> = cur
        .take(color_count * 3)?
        .chunks_exact(3)
        .map(|c| [c[0], c[1], c[2]])
        .collect();

    let mut world = World::new();
    world.colors = ColorTable::from_vec(colors);
    let mut sections = 0;
    while cur.at < raw.len() {
        sections += 1;
        if sections > MAX_DEPTH as usize + 1 {
            return Err(invalid("too many depth sections"));
        }
        let depth = cur.take(1)?[0];
        if depth > MAX_DEPTH {
            return Err(invalid("depth out of range"));
        }
        let n = u32::from_le_bytes(cur.take(4)?.try_into().unwrap());
        for _ in 0..n {
            let x = i64::from_le_bytes(cur.take(8)?.try_into().unwrap());
            let y = i64::from_le_bytes(cur.take(8)?.try_into().unwrap());
            let bytes = cur.take(CHUNK_AREA * 2)?;
            let mut cells = [EMPTY; CHUNK_AREA];
            for (cell, b) in cells.iter_mut().zip(bytes.chunks_exact(2)) {
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

    fn sample() -> World {
        let mut w = World::new();
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
        // Hand-build a payload: 1 colour, one chunk with hostile cells.
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
        file.extend(VERSION.to_le_bytes());
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
}
