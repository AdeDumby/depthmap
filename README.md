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
* On launch a full-page **welcome screen** (sidebars hidden until you pick a map) offers *Continue with <last map>* (also bound
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
* **Tiles** (top of the Colors sidebar on the right; the left sidebar holds the options of the Select and Measure tools): 14 textures (flagstone, cobblestone,
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
* **Zoom controls** (header bar): zoom readout, `−` / `1:1` / `+`, a zoom
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
| Arrow keys | pan |
| 1–9, 0 | pick texture |
| `[` `]` | brush size |
| Ctrl+Z, Ctrl+Y | undo, redo |
| Ctrl+N / O / S, Ctrl+Shift+S | new / open / save, save as |
| F1 | controls |

## Fill, selection and colours

**Tools** (top of the Tools panel): *Brush* (B), *Bucket* (F), *Select* (S), *Pick* (C), *Region* (R), *Route* (T). Pick is an eyedropper: click a cell to copy its tile and colour, and the app returns to the brush. The Colors sidebar (right) is toggled with P. While the Region or Route tool is in use, its list covers the Colors sidebar; "Back to colors" or choosing another tool brings Colors back.

* **Fill** - click to replace the whole connected area of identical cells (same
  texture and colour) with the current texture and colour; Shift erases it. One undo
  step. Areas over 2 million cells (an open, unbounded canvas) are refused.
* **Select** - *Rectangle* (drag), *Connected* (click: the connected identical cells) or
  *Same tiles* (click: every identical cell on the layer). Every new selection is
  **added** to the current one. **Right click inverts** it, **Esc** (or Ctrl+D) clears it,
  Ctrl+A selects all, Ctrl+I inverts. While something is selected, the brush, eraser,
  shapes and fill cannot change cells outside it, which makes it easy to paint one area.
  Selections work across zoom levels and are not part of the saved map or the undo history.
* **Colors window** (View menu, or C): a saturation/brightness gradient with a hue bar, R/G/B number
  fields, a hex field, the map's colours, and **palettes**. *Save current* stores the colour **and** the
  selected texture as an entry, so a palette works as a small tileset; click an entry to use
  both, right click to remove it. Palettes can be created, renamed and deleted, and are kept
  per user in the config folder (`palettes.txt`), not inside a map.

## Map depth

*Edit > Map depth...* adds or removes zoom levels of the open map. Adding levels keeps
everything. Removing levels deletes the finest layers and their nodes (the dialog says
how much first); that cannot be undone, so the undo history is cleared.

## Nodes

Nodes are points of interest. Press **N** (or *Add node* in the tools panel / Edit
menu) and click the map: a popup opens where you set the **name** (drawn under the
node), a **description**, an **icon** (15 built-in ones, plus your own) and the
**size** (0.1x-10x of one cell of the node's depth) and **Sits on** (Auto / Centre / Edge: Auto puts odd sizes on a cell centre and even sizes on a grid corner, so a 2x icon covers exactly 2x2 cells). *Done* keeps it, *Cancel*
discards the popup's changes (a new node is removed), *Delete* removes it.
Click a node to edit it again, drag it to move it, **Delete** deletes the open node.
Everything is undoable. *View > Show nodes* hides them.

**Folders and tags:** with the Nodes tool and no node open, the right sidebar lists every
node in **folders**, like routes: click a folder to fold it (and make it the folder new
nodes go into), right click to rename, **X** deletes it (its nodes move to the first
folder, which cannot be deleted), *New folder* adds one. Click a node row to open it and
jump to it. A node has a **Folder** dropdown and a list of **tags** (up to 16 of 32
characters, no `#`, case-insensitive) in its panel; the *Filter* box above the list matches
names and tags. Stored in the `.dmap` file (format v12; older files open with all nodes in
the first folder).

**Custom icons:** *Import PNG icons...* (File menu, tools panel or the popup) copies
PNGs into `Documents/Depthmap/Custom Icons`. They scale to the node size. Icons a map
uses are also stored inside the `.dmap` file (format v8), so shared maps keep them.

## Custom tiles and forests

Tools panel → **+ New tile** opens the tile editor: a 32×32 canvas where each pixel has a
shade (0–255) and may be transparent. Tiles are greyscale and tinted by the brush colour when
painted, like the built-in textures. Pencil, Erase, Fill, Solid, Clear and PNG import
(luminance + alpha, resampled to 32×32) are available, with a live tinted preview.

**Edge art (trees):** tick "Edge art" and draw a second picture with the forest tile *below*
it (the bottom of the picture touches the tile, marked by a red line). On the map this picture
is drawn in every neighbouring cell outside the forest, automatically rotated for each side, so
the forest block is solid while its border gets light protrusions. Cells of the same tile never
get edge art between them.

Tiles are saved to `Documents/Depthmap/Custom Tiles` (`Name.png`, `Name.edge.png`) and listed under
"Tile library" to reuse them in other maps; the tiles a map uses are also stored in the `.dmap`
(format v8, up to 17 custom tiles per map). Palettes do not yet remember custom tiles.

## Regions

The **Region** tool (R) marks out named places. Each region has a name, a colour, a tinted fill and an
outline, and shows on every depth: a boundary drawn at one layer appears on all of them. Open the
**Regions window** (it opens with the tool) to create, rename, recolour, hide, delete and choose the
active region.

Ways to mark cells (on the active region; if there is none, a new one is created):
- **Lasso**: drag a loop; cells whose centre is inside join.
- **Smart: connected** / **same tiles**: click a tile to grab the connected (or every) tile of that kind;
  click empty floor inside walls to take the whole room.
- **Selection to region**: turns the current Select-tool selection into region cells.

*Add* / *Remove* chooses what marking does; hold Shift to flip it. Regions are stored as cells at the
depth they were drawn on (format v8). They are not part of undo/redo.

## Routes

The **Route** tool (T) draws a route between two nodes. Click the start node, then the end node; the route is
created on the depth you are viewing. Then click the map to add points: the route is a smooth curve that
passes through every point, in order (a new point goes into the stretch of the route nearest to the click).
Drag a point to move it, right click it to remove it. The Routes sidebar (it opens with the tool; right click a folder to rename it)
lists routes in **folders** and sets name, width, solid / dashed / dotted, and can swap the ends or clear the points.
Click a folder to open or close it. A folder has a colour (click its swatch) and every route in it is drawn in that
colour. New routes go into the selected folder; pick another folder for a route in its editor. Deleting a folder
moves its routes to a neighbouring folder. Older maps get one folder per route colour.

A route is shown on the depth it was drawn on and every deeper depth, never on shallower ones. It follows its
nodes when they move; if a node is deleted the route is hidden (it comes back if the node does). Routes are
saved in the map (format v8) and are not part of undo/redo.

Clicking a colour in **Colors > Map colors** now also flashes every cell drawn with that colour on the current
layer (while hovered, and for a few seconds after the click).

## Copy, paste and stamps

With the Select tool: **Ctrl+C** copies the selected cells (up to 256 x 256, the selection's
bounding box; unselected and empty cells stay transparent), **Ctrl+X** cuts, **Ctrl+V**
(or *Paste*) shows the copy under the mouse and each click pastes it, centred on the mouse,
at the layer you are on, as one undo step; Esc stops. While pasting, **X** / **Y** flip it,
**R** / **Shift+R** turn it a quarter, also as *Flip H / Flip V / rotate* buttons. Copies keep
tile and *colour* (not colour numbers), so they paste correctly into another map. *Save
stamp* keeps the copy under a name as a file in `Documents/Depthmap/Stamps`; click a stamp in
the list to place it, **X** deletes it.

## Exporting an image

**File > Export image...** (Ctrl+E) saves the map as a PNG, drawn by the same renderer as the screen, so it uses your tiles and forest edges.

- **Area**: the whole map, the current selection's bounding box, or what is on screen.
- **Layer (depth)** and **pixels per cell**: cells of that layer become that many pixels wide (32 is the tiles' own size, 64 is sharper). The window shows the final size and explains when it is too large (at most 16384 px a side and 64 million pixels).
- **One region**: exports the bounding box of a chosen region, and *Transparent outside the region* clears every pixel whose cell is not part of it (so an L-shaped region exports as an L).
- **Presets**: *Save preset* keeps all of these choices under a name (the region is remembered by name, so it works in any map with a region called that); click one to apply it. Stored in `export_presets.txt` in the config folder.
- **Draw on top**, each toggleable: regions (tint, outline, names), routes, nodes (icons and names), grid lines and the faded layer above.

## Header bar, measuring and 3D

The narrow bar on the left holds the tools: Brush (B), Bucket (F), Select (S), Shape (H), Pick (C), Measure (M), then Region (R), Route (T) and Nodes (N). Shape opens its shapes and fill styles in the right sidebar; Nodes opens the node editor there (name, a tall description, then the icon list); Brush shows its size and styles above the tiles. The **Mode** menu (next to View) switches between 2D, Isometric and Top-down; the bar under the menu only appears in the 3D modes, with the turn buttons and the height slider. Zoom, depth and "Whole map" are at the left of the bottom bar. The strip at the bottom shows the cell under the mouse, the last message and the file name.

- **Measure (M):** click points to lay out a path; the label shows its length in cells of the layer you started on (and in depth-0 cells), the last leg's dx/dy, and the number of board-game moves with and without diagonals. Right click removes the last point, Esc clears.
- **2D / Isometric / Top-down:** the 3D views draw the map as a heightfield. A cell's height is its tile's height plus its colour's (-255 to 255 each; the sum is kept within that, and below zero is a pit). **Right click a tile** in the Tools window, or a colour in *Map colors*, to set its height; water starts flat and brick walls tall. The *height* slider sets how many cells tall 255 is. Isometric can be turned with `<` `>` buttons or the `,` `.` keys; `V` cycles the views. The 3D views are for looking (pan and zoom as usual); empty cells show the parent layer through.

## Keyboard shortcuts

*Help > Keyboard shortcuts* (F1) lists every shortcut and mouse action by section, with a *Find* box.

## Undo and recovery

Undo (Ctrl+Z) and redo (Ctrl+Shift+Z or Ctrl+Y) cover painting, nodes, regions, routes, custom tiles and heights. A drag or a slider movement is one step. While the tile editor is open the keys undo the tile instead.

Unsaved work is copied to a recovery file every minute and when you quit (quitting never asks). The next start offers to restore it; your real map file is never touched until you save.

## Brush styles

Under the brush size: **Solid**, **Palette mix** (each cell takes a random colour and texture of the current palette), **Mottled** (the brush colour, a little lighter or darker per cell), **Spray** (a random third of the cells under the brush) and **Dither** (a checkerboard). The eraser ignores styles.

## Tile editor

Brush sizes 1 to 8, mirror left/right and up/down, pencil, eraser, fill, line, rectangle (outline or filled), eyedropper (also right click), rectangle select with copy, cut, paste and stamp, flips, rotation, wrap-around slide (and *Half*, which moves the seams to the middle so you can see them), lighter, darker, grain, copying any built-in or map tile into the one you are editing, PNG import, undo and redo. It tells you when the left/right and top/bottom edges match.

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
