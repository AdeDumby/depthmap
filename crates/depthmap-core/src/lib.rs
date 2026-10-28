//! Renderer-independent data model for the multi-depth map editor.
//!
//! # Model
//!
//! The map is a stack of sparse orthogonal grids, one per *depth*. Every cell
//! at depth `d` is subdivided into `SUBDIV x SUBDIV` cells at depth `d + 1`, so
//! the cell `(x, y)` at depth `d` covers the same area as the block
//! `(x*SUBDIV .. (x+1)*SUBDIV, y*SUBDIV .. (y+1)*SUBDIV)` at depth `d + 1`.
//!
//! * Coordinates are **integers per depth** (`i64`), never floats, so zooming
//!   deeper never loses precision.
//! * Each depth is a hash map of fixed-size **chunks** (`CHUNK_SIZE`²
//!   cells). Empty space costs nothing, and a chunk is the unit of GPU upload.
//! * The [`Camera`] is `(depth, integer cell, fractional offset, px-per-cell)`.
//!   Crossing a zoom threshold re-bases it onto the next depth ("origin
//!   rebasing"), which is what makes zooming effectively unbounded.

pub mod camera;
pub mod chunk;
pub mod history;
pub mod io;
pub mod palette;
pub mod raster;
pub mod world;

pub use camera::{Camera, LayerView};
pub use chunk::{Chunk, ChunkPos, EMPTY, Material};
pub use history::History;
pub use world::World;

/// Children per axis: one cell at depth `d` becomes `SUBDIV²` cells at `d + 1`.
pub const SUBDIV: i64 = 4;
/// `log2(CHUNK_SIZE)`; chunk lookup is a shift, not a division.
pub const CHUNK_SHIFT: u32 = 5;
/// Cells per chunk axis.
pub const CHUNK_SIZE: i64 = 1 << CHUNK_SHIFT;
/// Cells per chunk.
pub const CHUNK_AREA: usize = (CHUNK_SIZE * CHUNK_SIZE) as usize;
/// Deepest subdivision level (`SUBDIV^MAX_DEPTH` still fits comfortably in `i64`).
pub const MAX_DEPTH: u8 = 16;
