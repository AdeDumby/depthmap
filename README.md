# depthmap

Multi-depth TTRPG map editor. Draw on an orthogonal grid; zoom in and the layer
you drew becomes a faded backdrop for a finer 4×4-per-cell layer, recursively
(up to 16 depths).

```
cargo run --release                    # starts an untitled map
cargo run --release -- my.dmap         # open (or create) a file
cargo run --release -- --stress 4096   # 16M-cell test map
cargo test                             # core + editor + UI + headless GPU render tests
```

## Interface

* **Menu bar** (top): File (New, Open, Save, Save As, Quit), Edit (Undo, Redo),
  View (grid, panels, zoom actions), Help (controls, about). Destructive
  actions ask before discarding unsaved changes.
* **Textures panel** (floating, left): 14 textures (flagstone, cobblestone,
  brick, planks, dirt, grass, sand, water, hatch, ...). Every texture can be
  painted in **any color**: pick a swatch or use the color picker. Brush size
  slider below.
* **Zoom panel** (floating, right): zoom readout, `−` / `1:1` / `+`, a zoom
  slider, jump one depth up/down, opacity of the parent-layer backdrop, grid
  toggle.

| Input | Action |
|---|---|
| LMB / RMB | paint / erase |
| MMB, or Space + LMB | pan |
| wheel | zoom around cursor (changes depth automatically) |
| WASD / arrows (physical keys) | pan |
| 1–9, 0 | pick texture |
| `[` `]` | brush size |
| Ctrl+Z, Ctrl+Y | undo, redo |
| Ctrl+N / O / S, Ctrl+Shift+S | new / open / save, save as |
| F1 | controls |

## Layout

* `crates/depthmap-core`: no GPU, no window. Sparse chunked grids per depth
  (a cell is a texture id + an index into the map's color table, 2 bytes),
  integer-coordinate camera with origin rebasing, delta undo log, zstd save
  format (v2), procedural seamless texture tiles, Bresenham strokes.
* `crates/depthmap`: wgpu renderer (chunk atlas + mip-mapped texture array +
  one instanced draw for all depths + analytic grid overlay), egui interface,
  editor input logic, winit app.

## Known limits / next steps

* Atlas holds 65k chunks (67M cells) on-screen at once; beyond that the extra
  chunks are skipped (counted in `FrameStats::chunks_skipped`).
* A map holds up to 256 distinct colors (colors enter the table only when
  painted with); the nearest existing color is used once it is full.
* Save files from the first prototype (format v1) can't be opened.
* No background autosave yet.
