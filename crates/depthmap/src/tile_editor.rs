//! The tile editor window: draw a 32x32 greyscale tile (shade + transparency),
//! optionally with an "edge" picture that spills into the neighbouring cells,
//! and see it tinted and tiled the way it will look on the map.

use crate::editor::Editor;
use depthmap_core::texture::TILE;
use depthmap_core::tile::{self, CustomTile, TileArt};
use egui::{Color32, Rect, RichText, Sense, Stroke, TextureOptions, pos2, vec2};

const CELL: f32 = 12.0;
const PREVIEW_CELLS: usize = 5;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Pencil,
    Erase,
    Fill,
}

pub struct TileEditor {
    pub name: String,
    pub art: TileArt,
    pub overhang: TileArt,
    pub use_overhang: bool,
    /// Drawing the edge picture instead of the tile itself.
    pub edge: bool,
    pub shade: u8,
    pub tool: Tool,
    /// Texture id being edited, when this started from an existing tile.
    pub editing: Option<u8>,
    pub note: String,
    last: Option<(usize, usize)>,
    preview: Option<egui::TextureHandle>,
    preview_key: Option<(u64, [u8; 3])>,
    revision: u64,
    pub canvas: Option<Rect>,
    pub save_button: Option<Rect>,
}

impl TileEditor {
    pub fn new_tile(ed: &Editor) -> Self {
        let mut n = 1;
        let name = loop {
            let c = format!("Tile {n}");
            if ed.world.tile_id(&c).is_none() {
                break c;
            }
            n += 1;
        };
        Self::from_tile(CustomTile::new(&name, TileArt::solid(200)), None)
    }

    pub fn from_tile(t: CustomTile, editing: Option<u8>) -> Self {
        Self {
            name: t.name,
            art: t.art,
            use_overhang: t.overhang.is_some(),
            overhang: t.overhang.unwrap_or_else(TileArt::blank),
            edge: false,
            shade: 200,
            tool: Tool::Pencil,
            editing,
            note: String::new(),
            last: None,
            preview: None,
            preview_key: None,
            revision: 1,
            canvas: None,
            save_button: None,
        }
    }

    fn current(&mut self) -> &mut TileArt {
        if self.edge { &mut self.overhang } else { &mut self.art }
    }

    fn paint_at(&mut self, x: usize, y: usize) {
        let (shade, tool) = (self.shade, self.tool);
        match tool {
            Tool::Pencil => self.current().set(x, y, shade, 255),
            Tool::Erase => self.current().set(x, y, 0, 0),
            Tool::Fill => {
                flood(self.current(), x, y, shade);
            }
        }
        self.revision += 1;
    }

    fn line_to(&mut self, (x1, y1): (usize, usize)) {
        let (mut x, mut y) = self.last.unwrap_or((x1, y1));
        let (dx, dy) = ((x1 as i32 - x as i32).abs(), -(y1 as i32 - y as i32).abs());
        let (sx, sy) = (if x < x1 { 1i32 } else { -1 }, if y < y1 { 1i32 } else { -1 });
        let mut err = dx + dy;
        loop {
            self.paint_at(x, y);
            if (x, y) == (x1, y1) {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x = (x as i32 + sx) as usize;
            }
            if e2 <= dx {
                err += dx;
                y = (y as i32 + sy) as usize;
            }
        }
        self.last = Some((x1, y1));
    }

    pub fn to_tile(&self) -> CustomTile {
        let mut t = CustomTile::new(&self.name, self.art.clone());
        if self.use_overhang && !self.overhang.is_blank() {
            t.overhang = Some(self.overhang.clone());
        }
        t
    }
}

/// Replaces the connected area of pixels equal to the clicked one.
fn flood(art: &mut TileArt, x: usize, y: usize, shade: u8) {
    let target = art.get(x, y);
    let new = (shade, 255);
    if target == new {
        return;
    }
    let mut stack = vec![(x, y)];
    while let Some((cx, cy)) = stack.pop() {
        if art.get(cx, cy) != target {
            continue;
        }
        art.set(cx, cy, new.0, new.1);
        if cx > 0 { stack.push((cx - 1, cy)); }
        if cy > 0 { stack.push((cx, cy - 1)); }
        if cx + 1 < TILE { stack.push((cx + 1, cy)); }
        if cy + 1 < TILE { stack.push((cx, cy + 1)); }
    }
}

fn checker(painter: &egui::Painter, rect: Rect) {
    let n = TILE * 2;
    let s = rect.width() / n as f32;
    for y in 0..n {
        for x in 0..n {
            let c = if (x + y) % 2 == 0 { Color32::from_gray(200) } else { Color32::from_gray(150) };
            painter.rect_filled(
                Rect::from_min_size(rect.min + vec2(x as f32 * s, y as f32 * s), vec2(s, s)),
                0.0,
                c,
            );
        }
    }
}

/// Draws the window. Returns true while it is open.
pub fn window(ctx: &egui::Context, ed: &mut Editor, slot: &mut Option<TileEditor>) {
    let Some(te) = slot.as_mut() else { return };
    let mut close = false;
    let mut save = false;
    let rgb = ed.brush_rgb;
    te.canvas = None;
    te.save_button = None;
    egui::Window::new("Tile editor")
        .collapsible(false)
        .resizable(false)
        .default_pos(pos2(300.0, 60.0))
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Name");
                ui.add(egui::TextEdit::singleline(&mut te.name).desired_width(160.0));
            });
            ui.horizontal(|ui| {
                ui.selectable_value(&mut te.edge, false, "Tile");
                ui.add_enabled_ui(te.use_overhang, |ui| {
                    ui.selectable_value(&mut te.edge, true, "Edge");
                });
                if ui
                    .checkbox(&mut te.use_overhang, "Edge art (trees)")
                    .on_hover_text(
                        "A picture drawn on the neighbouring cells around this tile. Draw it with the \
                         tile BELOW the picture: the bottom of the image touches the tile. It is \
                         rotated automatically for the other sides.",
                    )
                    .changed()
                {
                    te.edge = te.edge && te.use_overhang;
                    te.revision += 1;
                }
            });
            ui.horizontal(|ui| {
                ui.selectable_value(&mut te.tool, Tool::Pencil, "Pencil");
                ui.selectable_value(&mut te.tool, Tool::Erase, "Erase");
                ui.selectable_value(&mut te.tool, Tool::Fill, "Fill");
                ui.add(egui::Slider::new(&mut te.shade, 0..=255).text("Shade"));
            });

            ui.horizontal_top(|ui| {
                // Canvas
                let side = CELL * TILE as f32;
                let (rect, resp) = ui.allocate_exact_size(vec2(side, side), Sense::click_and_drag());
                te.canvas = Some(rect);
                let painter = ui.painter_at(rect);
                checker(&painter, rect);
                let art = if te.edge { &te.overhang } else { &te.art };
                for y in 0..TILE {
                    for x in 0..TILE {
                        let (s, a) = art.get(x, y);
                        if a > 0 {
                            let t = tinted(s, rgb);
                            let c = Color32::from_rgba_unmultiplied(t[0], t[1], t[2], a);
                            painter.rect_filled(
                                Rect::from_min_size(
                                    rect.min + vec2(x as f32 * CELL, y as f32 * CELL),
                                    vec2(CELL, CELL),
                                ),
                                0.0,
                                c,
                            );
                        }
                    }
                }
                painter.rect_stroke(rect, 0.0, Stroke::new(1.0, Color32::BLACK), egui::StrokeKind::Inside);
                if te.edge {
                    // Mark where the forest tile sits (below the picture).
                    let y = rect.bottom() - 1.0;
                    painter.line_segment(
                        [pos2(rect.left(), y), pos2(rect.right(), y)],
                        Stroke::new(3.0, Color32::from_rgb(220, 60, 60)),
                    );
                }
                let pos = resp.interact_pointer_pos().filter(|_| resp.dragged() || resp.clicked());
                if let Some(p) = pos {
                    let x = (((p.x - rect.left()) / CELL) as i32).clamp(0, TILE as i32 - 1) as usize;
                    let y = (((p.y - rect.top()) / CELL) as i32).clamp(0, TILE as i32 - 1) as usize;
                    if te.tool == Tool::Fill {
                        if resp.clicked() {
                            te.paint_at(x, y);
                        }
                    } else {
                        te.line_to((x, y));
                    }
                }
                if !resp.dragged() {
                    te.last = None;
                }

                // Preview
                ui.vertical(|ui| {
                    ui.label(RichText::new("Preview (tinted)").strong());
                    let key = (te.revision + te.use_overhang as u64 * 1_000_000, rgb);
                    if te.preview_key != Some(key) || te.preview.is_none() {
                        te.preview_key = Some(key);
                        let t = te.to_tile();
                        let px = tile::forest_preview(&t.art, t.overhang.as_ref(), rgb, [230, 222, 204], PREVIEW_CELLS);
                        let n = PREVIEW_CELLS * TILE;
                        let img = egui::ColorImage::from_rgba_unmultiplied([n, n], &px);
                        match &mut te.preview {
                            Some(h) => h.set(img, TextureOptions::NEAREST),
                            None => te.preview = Some(ctx.load_texture("tile-preview", img, TextureOptions::NEAREST)),
                        }
                    }
                    if let Some(h) = &te.preview {
                        ui.image((h.id(), vec2(150.0, 150.0)));
                    }
                    ui.label(RichText::new("Colour: the current brush colour").weak().small());
                    ui.separator();
                    if ui.button("Import PNG…").clicked() {
                        if let Some(path) = rfd::FileDialog::new().add_filter("PNG", &["png"]).pick_file() {
                            match std::fs::read(&path).map_err(|e| e.to_string()).and_then(|b| tile::art_from_png(&b)) {
                                Ok(a) => {
                                    *te.current() = a;
                                    te.revision += 1;
                                }
                                Err(e) => te.note = e,
                            }
                        }
                    }
                    if ui.button("Clear").clicked() {
                        *te.current() = TileArt::blank();
                        te.revision += 1;
                    }
                    if ui.button("Solid").clicked() {
                        let s = te.shade;
                        *te.current() = TileArt::solid(s);
                        te.revision += 1;
                    }
                });
            });
            if !te.note.is_empty() {
                ui.colored_label(Color32::from_rgb(200, 60, 60), te.note.clone());
            }
            ui.horizontal(|ui| {
                let ok = tile::valid_tile_name(&te.name);
                let b = ui.add_enabled(ok, egui::Button::new("Save tile"));
                te.save_button = Some(b.rect);
                if b.clicked() {
                    save = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
    if save {
        let tile = te.to_tile();
        match ed.save_tile(tile) {
            Ok(_) => close = true,
            Err(e) => te.note = e,
        }
    }
    if close {
        *slot = None;
    }
}

fn tinted(shade: u8, rgb: [u8; 3]) -> [u8; 3] {
    let f = |c: u8| (c as u32 * shade as u32 / 255) as u8;
    [f(rgb[0]), f(rgb[1]), f(rgb[2])]
}
