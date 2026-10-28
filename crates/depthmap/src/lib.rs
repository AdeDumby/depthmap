pub mod editor;
pub mod renderer;

/// Fills a `n`×`n` cell area at depth 0 with a noisy pattern, plus a scatter
/// of detail at depth 1, for performance testing (`depthmap --stress 4000`).
pub fn stress_fill(world: &mut depthmap_core::World, n: i64) {
    use depthmap_core::{CHUNK_AREA, CHUNK_SIZE, ChunkPos};
    let hash = |x: i64, y: i64| -> u64 {
        let mut h = (x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
            ^ (y as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
        h ^= h >> 29;
        h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
        h ^ (h >> 32)
    };
    let chunks = n / CHUNK_SIZE;
    for cy in -chunks / 2..chunks / 2 {
        for cx in -chunks / 2..chunks / 2 {
            let mut cells = [0u8; CHUNK_AREA];
            for (i, c) in cells.iter_mut().enumerate() {
                let x = cx * CHUNK_SIZE + (i as i64 & (CHUNK_SIZE - 1));
                let y = cy * CHUNK_SIZE + (i as i64 >> 5);
                // Blobby regions of terrain with a bit of per-cell noise.
                let region = hash(x >> 3, y >> 3) % 7;
                *c = match region {
                    0 => 0,
                    r => (r as u8) + if hash(x, y) % 9 == 0 { 1 } else { 0 },
                };
            }
            world.insert_chunk(0, ChunkPos { x: cx, y: cy }, &cells);
            if (cx + cy) % 3 == 0 {
                let mut fine = [0u8; CHUNK_AREA];
                for (i, c) in fine.iter_mut().enumerate() {
                    if hash(i as i64, cx ^ cy) % 4 == 0 {
                        *c = 9;
                    }
                }
                world.insert_chunk(1, ChunkPos { x: cx, y: cy }, &fine);
            }
        }
    }
}
