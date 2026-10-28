# depthmap

Multi-depth TTRPG map editor. Draw on an orthogonal grid; zoom in and the layer
you drew becomes a faded backdrop for a finer 4×4-per-cell layer, recursively
(up to 16 depths).

```
cargo run --release                 # edits ./map.dmap
cargo run --release -- --stress 4096   # 16M-cell test map
cargo test                          # core + editor logic + headless GPU render tests
```

| Input | Action |
|---|---|
| LMB / RMB | paint / erase |
| MMB, or Space + LMB | pan |
| wheel | zoom around cursor (changes depth automatically) |
| WASD / arrows (physical keys) | pan |
| 1–9, 0 | material |
| `[` `]` | brush size |
| Ctrl+Z, Ctrl+Y | undo, redo |
| Ctrl+S / Ctrl+O | save / load |

## Layout

* `crates/depthmap-core`: no GPU, no window. Sparse chunked grids per depth,
  integer-coordinate camera with origin rebasing, delta undo log, zstd save
  format, Bresenham strokes.
* `crates/depthmap`: wgpu renderer (chunk atlas + one instanced draw for all
  depths + analytic grid overlay), editor input logic, winit app.

## Known limits / next steps

* Atlas holds 65k chunks (67M cells) on-screen at once; beyond that the extra
  chunks are skipped (counted in `FrameStats::chunks_skipped`).
* Layers with cells under 0.5 px are not drawn; there is no mip chain yet.
* No UI panels (egui) and no background autosave yet.
