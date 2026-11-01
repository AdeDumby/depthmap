# depthmap

Multi-depth TTRPG map editor. Draw on an orthogonal grid; zoom in and the layer
you drew becomes a faded backdrop for a finer 4×4-per-cell layer, recursively
(up to 16 depths).

```
cargo run --release                    # welcome screen (new / open / last map)
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
* **New map** asks for a **size** (cells across the whole map at depth 0,
  4 to 1024 per side, square presets or any width × height) and a **depth**
  (0 to 16: how many times you can zoom into a cell; each level is 4×4 finer).
  The size is the zoom-out limit: fully zoomed out, the whole map just fits the
  window. Zooming in stops at the chosen depth, you cannot pan off the map, and
  nothing can be drawn outside it (the outside is dimmed, the edge outlined).
  Both numbers are stored in the `.dmap` file. Older files without a size open
  as unbounded maps.
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
* **Checker overlay** (View menu or zoom panel): darkens every other cell of the
  depth you are drawing on, so single cells stand out. It fades away when cells
  get too small to tell apart, and is overlay-only like the grid.
* **Hover readout** (menu bar): the cell under the mouse at the current depth
  and the parent cell ("superpixel", one depth up) it belongs to, e.g.
  `d3 cell 2045,1280  in d2 cell 511,320`; greyed when the mouse is off the map.
* **Backdrop**: zooming past a layer's cells fades it to a light backdrop
  behind the layer you are drawing on (opacity set by the *Parent layer*
  slider), again smoothly.
* **Shapes** (left panel, below the colors): pick a shape and a fill, then drag
  with the **right mouse button**. The shape is drawn live with the real texture
  and color at the depth you are zoomed into, and releasing it is one undo step
  (Esc cancels). Fills: *Outline*, *Solid*, ordered dithering at 25 / 50 / 75 %,
  *Stripes* and *Crosshatch*; *Solid border* adds a full edge around a patterned
  fill. The brush size is the edge thickness. Patterns are tied to the grid, so
  neighbouring shapes tile seamlessly; a *Line* with a pattern becomes dashed.
* **Zoom panel** (floating, right): zoom readout, `−` / `1:1` / `+`, a zoom
  slider, jump one depth up/down, opacity of the parent-layer backdrop, grid
  toggle.

| Input | Action |
|---|---|
| LMB | paint with the brush |
| RMB drag | draw the chosen shape (line, rectangle, ellipse, triangle, diamond) |
| Shift + mouse, or `E` | erase (brush and shapes) |
| Esc | cancel the shape being dragged |
| MMB, or Space + LMB | pan |
| wheel | zoom around cursor (changes depth automatically) |
| WASD / arrows (physical keys) | pan |
| 1–9, 0 | pick texture |
| `[` `]` | brush size |
| Ctrl+Z, Ctrl+Y | undo, redo |
| Ctrl+N / O / S, Ctrl+Shift+S | new / open / save, save as |
| F1 | controls |

## Fill, selection and colours

**Tools** (top of the Tools panel): *Brush* (B), *Fill* (G), *Select* (M).

* **Fill** - click to replace the whole connected area of identical cells (same
  texture and colour) with the current texture and colour; Shift erases it. One undo
  step. Areas over 2 million cells (an open, unbounded canvas) are refused.
* **Select** - *Rectangle* (drag), *Connected* (click: the connected identical cells) or
  *Same tiles* (click: every identical cell on the layer). Every new selection is
  **added** to the current one. **Right click inverts** it, **Esc** (or Ctrl+D) clears it,
  Ctrl+A selects all, Ctrl+I inverts. While something is selected, the brush, eraser,
  shapes and fill cannot change cells outside it, which makes it easy to paint one area.
  Selections work across zoom levels and are not part of the saved map or the undo history.
* **Colors window** (View menu, or C): a hue/saturation wheel with a brightness bar, a hex
  field, the map's colours, and **palettes**. *Save current* stores the colour **and** the
  selected texture as an entry, so a palette works as a small tileset; click an entry to use
  both, right click to remove it. Palettes can be created, renamed and deleted, and are kept
  per user in the config folder (`palettes.txt`), not inside a map.

## Nodes

Nodes are points of interest. Press **N** (or *Add node* in the tools panel / Edit
menu) and click the map: a popup opens where you set the **name** (drawn under the
node), a **description**, an **icon** (15 built-in ones, plus your own) and the
**size** (0.25x-4x of one cell of the node's depth). *Done* keeps it, *Cancel*
discards the popup's changes (a new node is removed), *Delete* removes it.
Click a node to edit it again, drag it to move it, **Delete** deletes the open node.
Everything is undoable. *View > Show nodes* hides them.

**Custom icons:** *Import PNG icons...* (File menu, tools panel or the popup) copies
PNGs into `Documents/Depthmap/Custom Icons`. They scale to the node size. Icons a map
uses are also stored inside the `.dmap` file (format v4), so shared maps keep them.

## Layout

* `crates/depthmap-core`: no GPU, no window. Sparse chunked grids per depth
  (a cell is a texture id + an index into the map's color table, 2 bytes),
  integer-coordinate camera with origin rebasing, delta undo log, zstd save
  format (v3), procedural seamless texture tiles, Bresenham strokes.
* `crates/depthmap`: wgpu renderer (chunk atlas + mip-mapped texture array +
  one instanced draw for all depths + analytic grid overlay), egui interface,
  editor input logic, winit app.

## Known limits / next steps

* Atlas holds 65k chunks (67M cells) on-screen at once; beyond that the extra
  chunks are skipped (counted in `FrameStats::chunks_skipped`).
* A map holds up to 256 distinct colors (colors enter the table only when
  painted with); the nearest existing color is used once it is full.
* Map sides are limited to 1024 cells at depth 0 (use depth for finer detail).
  A shape larger than the map's visible part is only drawn where it is on screen.
* Save files from the first prototype (format v1) can't be opened.
* No background autosave yet.
