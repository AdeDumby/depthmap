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

## Starting up and where maps are stored

* A map is **one `.dmap` file** (a small header + zstd-compressed color table
  and chunks; the layout is documented at the top of
  `crates/depthmap-core/src/io.rs`). You choose where it lives; the app never
  copies it anywhere else.
* On launch a **welcome screen** offers *Continue with <last map>* (also bound
  to Enter), *Open map...*, *New map* and the recent list. Launching with a file
  (`depthmap my.dmap`) skips it.
* **Save** (Ctrl+S) writes to the map that is open; an untitled map asks for a
  file name the first time. *Save As* picks a new file.
* The list of recent maps is a plain text file, one path per line, newest
  first (at most 10): `%APPDATA%\depthmap\recent.txt` on Windows,
  `~/Library/Application Support/depthmap/recent.txt` on macOS,
  `~/.config/depthmap/recent.txt` on Linux. File dialogs open in the folder of
  the last map.

## Interface

* **Menu bar** (top): File (New, Open, Save, Save As, Quit), Edit (Undo, Redo),
  View (grid, panels, zoom actions), Help (controls, about). File also has
  *Open recent*. Destructive actions ask before discarding unsaved changes.
* **Textures panel** (floating, left): 14 textures (flagstone, cobblestone,
  brick, planks, dirt, grass, sand, water, hatch, ...). Every texture can be
  painted in **any color**: pick a swatch or use the color picker. Brush size
  slider below.
* **Grid overlay**: the cell grid of the depth you are drawing on, plus the
  parent depths (heavier lines) and, as you approach the next depth, the finer
  grid fading in. It is purely an overlay: it never changes the map, and since
  every line's strength depends only on its on-screen spacing, nothing jumps
  when the camera switches depth. The zoom panel shows the cell size strokes
  snap to ("snap: 1/16 cell").
* **Backdrop**: zooming past a layer's cells fades it to a light backdrop
  behind the layer you are drawing on (opacity set by the *Parent layer*
  slider), again smoothly.
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
