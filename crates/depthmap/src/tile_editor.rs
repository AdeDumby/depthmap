//! The tile editor window: draw a 32x32 greyscale tile (shade + transparency),
//! optionally with an "edge" picture that spills into the neighbouring cells,
//! and see it tinted and tiled the way it will look on the map.
//!
//! Tools: pencil and eraser with a brush size and mirroring, bucket fill,
//! line, rectangle, eyedropper, rectangle selection with copy / cut / paste
//! (and stamping), flips, rotation, wrap-around shifting (to check seams),
//! brightness, grain, copying another tile in, and undo / redo.

use crate::editor::Editor;
use depthmap_core::texture::{self, TILE};
use depthmap_core::tile::{self, Clip, CustomTile, TileArt};
use egui::{Color32, Rect, RichText, Sense, Stroke, TextureOptions, pos2, vec2};

const PREVIEW_CELLS: usize = 5;
const MAX_UNDO: usize = 80;
pub const MAX_BRUSH: usize = 8;
const SWATCHES: [u8; 9] = [0, 32, 64, 96, 128, 160, 192, 224, 255];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    Pencil,
    Erase,
    Fill,
    Line,
    Rect,
    /// Click a pixel to take its shade.
    Pick,
    /// Drag a rectangle to copy, cut, flip or clear.
    Select,
    /// Click to put the copied pixels down.
    Stamp,
}

impl Tool {
    const ALL: [(Tool, &'static str, &'static str); 7] = [
        (Tool::Pencil, "Pencil", "Draw with the shade (right click picks a shade)"),
        (Tool::Erase, "Erase", "Make pixels transparent"),
        (Tool::Fill, "Fill", "Fill the connected area of identical pixels"),
        (Tool::Line, "Line", "Drag a straight line"),
        (Tool::Rect, "Rect", "Drag a rectangle (outline, or filled if Filled is on)"),
        (Tool::Pick, "Pick", "Click a pixel to take its shade"),
        (Tool::Select, "Select", "Drag a rectangle to copy, cut, flip or clear it"),
    ];
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
    /// Brush width in pixels for the pencil and eraser.
    pub brush: usize,
    pub mirror_x: bool,
    pub mirror_y: bool,
    pub filled: bool,
    pub grid: bool,
    /// Texture id being edited, when this started from an existing tile.
    pub editing: Option<u8>,
    pub note: String,
    /// The selected rectangle (corners, inclusive).
    pub selection: Option<((usize, usize), (usize, usize))>,
    /// What was copied, ready to paste or stamp.
    pub clip: Option<Clip>,
    undo: Vec<(TileArt, TileArt)>,
    redo: Vec<(TileArt, TileArt)>,
    last: Option<(usize, usize)>,
    drag_start: Option<(usize, usize)>,
    grain_seed: u32,
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
            brush: 1,
            mirror_x: false,
            mirror_y: false,
            filled: false,
            grid: true,
            editing,
            note: String::new(),
            selection: None,
            clip: None,
            undo: Vec::new(),
            redo: Vec::new(),
            last: None,
            drag_start: None,
            grain_seed: 1,
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

    fn current_ref(&self) -> &TileArt {
        if self.edge { &self.overhang } else { &self.art }
    }

    /// Remembers the pictures so the next change can be undone.
    pub fn push_undo(&mut self) {
        self.undo.push((self.art.clone(), self.overhang.clone()));
        if self.undo.len() > MAX_UNDO {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self) {
        if let Some((a, o)) = self.undo.pop() {
            self.redo.push((std::mem::replace(&mut self.art, a), std::mem::replace(&mut self.overhang, o)));
            self.revision += 1;
        }
    }

    pub fn redo(&mut self) {
        if let Some((a, o)) = self.redo.pop() {
            self.undo.push((std::mem::replace(&mut self.art, a), std::mem::replace(&mut self.overhang, o)));
            self.revision += 1;
        }
    }

    /// Replaces the picture being drawn on (one undo step).
    fn replace_current(&mut self, art: TileArt) {
        self.push_undo();
        *self.current() = art;
        self.revision += 1;
    }

    /// The pixel and its mirror images, per the symmetry switches.
    fn mirrored(&self, x: usize, y: usize) -> Vec<(usize, usize)> {
        let mut v = vec![(x, y)];
        if self.mirror_x {
            v.push((TILE - 1 - x, y));
        }
        if self.mirror_y {
            v.push((x, TILE - 1 - y));
        }
        if self.mirror_x && self.mirror_y {
            v.push((TILE - 1 - x, TILE - 1 - y));
        }
        v.dedup();
        v
    }

    /// One brush dab (a `brush` x `brush` square) at a pixel, mirrored.
    fn dab(&mut self, x: usize, y: usize, erase: bool) {
        let (shade, n) = (self.shade, self.brush.clamp(1, MAX_BRUSH));
        let lo = (n - 1) / 2;
        for (mx, my) in self.mirrored(x, y) {
            for dy in 0..n {
                for dx in 0..n {
                    let (px, py) = (mx as i32 + dx as i32 - lo as i32, my as i32 + dy as i32 - lo as i32);
                    if (0..TILE as i32).contains(&px) && (0..TILE as i32).contains(&py) {
                        if erase {
                            self.current().set(px as usize, py as usize, 0, 0);
                        } else {
                            self.current().set(px as usize, py as usize, shade, 255);
                        }
                    }
                }
            }
        }
        self.revision += 1;
    }

    fn fill_at(&mut self, x: usize, y: usize) {
        let shade = self.shade;
        for (mx, my) in self.mirrored(x, y) {
            flood(self.current(), mx, my, shade);
        }
        self.revision += 1;
    }

    /// Draws from the last pixel to this one with the brush.
    fn line_to(&mut self, (x1, y1): (usize, usize), erase: bool) {
        let from = self.last.unwrap_or((x1, y1));
        for (x, y) in line_points(from, (x1, y1)) {
            self.dab(x, y, erase);
        }
        self.last = Some((x1, y1));
    }

    /// A straight line or rectangle between two pixels, with the brush.
    fn shape(&mut self, a: (usize, usize), b: (usize, usize)) {
        let pts: Vec<(usize, usize)> = if self.tool == Tool::Line {
            line_points(a, b)
        } else {
            rect_points(a, b, self.filled)
        };
        for (x, y) in pts {
            self.dab(x, y, false);
        }
    }

    fn sel_rect(&self) -> ((usize, usize), (usize, usize)) {
        self.selection.unwrap_or(((0, 0), (TILE - 1, TILE - 1)))
    }

    pub fn copy(&mut self) {
        if let Some((a, b)) = self.selection {
            self.clip = Some(self.current_ref().copy_rect(a, b));
            self.note.clear();
        }
    }

    pub fn cut(&mut self) {
        if let Some((a, b)) = self.selection {
            self.copy();
            self.push_undo();
            self.current().clear_rect(a, b);
            self.revision += 1;
        }
    }

    pub fn clear_selection_pixels(&mut self) {
        if let Some((a, b)) = self.selection {
            self.push_undo();
            self.current().clear_rect(a, b);
            self.revision += 1;
        }
    }

    /// Puts the copied pixels down with their top left at `(x, y)`.
    pub fn stamp(&mut self, x: i32, y: i32) {
        if let Some(clip) = self.clip.clone() {
            self.push_undo();
            self.current().paste(&clip, x, y, true);
            self.revision += 1;
        }
    }

    /// Flips or turns what is selected (or the whole picture).
    fn transform(&mut self, f: impl Fn(&Clip) -> Clip) {
        let (a, b) = self.sel_rect();
        let clip = self.current_ref().copy_rect(a, b);
        let out = f(&clip);
        self.push_undo();
        let (x0, y0) = (a.0.min(b.0), a.1.min(b.1));
        self.current().clear_rect(a, b);
        self.current().paste(&out, x0 as i32, y0 as i32, false);
        // A turned rectangle changes shape; keep the selection on it.
        if self.selection.is_some() {
            self.selection = Some(((x0, y0), ((x0 + out.w - 1).min(TILE - 1), (y0 + out.h - 1).min(TILE - 1))));
        }
        self.revision += 1;
    }

    fn shift(&mut self, dx: i32, dy: i32) {
        let shifted = self.current_ref().shifted(dx, dy);
        self.replace_current(shifted);
    }

    fn brighten(&mut self, d: i32) {
        let mut art = self.current_ref().clone();
        art.brighten(d);
        self.replace_current(art);
    }

    fn grain(&mut self) {
        let mut art = self.current_ref().clone();
        self.grain_seed = self.grain_seed.wrapping_add(1);
        art.grain(14, self.grain_seed);
        self.replace_current(art);
    }

    pub fn to_tile(&self) -> CustomTile {
        let mut t = CustomTile::new(&self.name, self.art.clone());
        if self.use_overhang && !self.overhang.is_blank() {
            t.overhang = Some(self.overhang.clone());
        }
        t
    }
}

/// Pixels on the straight line between two pixels, ends included.
pub fn line_points((x0, y0): (usize, usize), (x1, y1): (usize, usize)) -> Vec<(usize, usize)> {
    let (mut x, mut y) = (x0 as i32, y0 as i32);
    let (dx, dy) = ((x1 as i32 - x).abs(), -(y1 as i32 - y).abs());
    let (sx, sy) = (if x < x1 as i32 { 1 } else { -1 }, if y < y1 as i32 { 1 } else { -1 });
    let mut err = dx + dy;
    let mut out = Vec::new();
    loop {
        out.push((x as usize, y as usize));
        if (x, y) == (x1 as i32, y1 as i32) {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
    }
    out
}

/// Pixels of a rectangle between two corners: its border, or all of it.
pub fn rect_points(a: (usize, usize), b: (usize, usize), filled: bool) -> Vec<(usize, usize)> {
    let (x0, x1) = (a.0.min(b.0), a.0.max(b.0));
    let (y0, y1) = (a.1.min(b.1), a.1.max(b.1));
    let mut out = Vec::new();
    for y in y0..=y1 {
        for x in x0..=x1 {
            if filled || x == x0 || x == x1 || y == y0 || y == y1 {
                out.push((x, y));
            }
        }
    }
    out
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

fn pixel_at(rect: Rect, cell: f32, p: egui::Pos2) -> (usize, usize) {
    let x = (((p.x - rect.left()) / cell).floor() as i32).clamp(0, TILE as i32 - 1) as usize;
    let y = (((p.y - rect.top()) / cell).floor() as i32).clamp(0, TILE as i32 - 1) as usize;
    (x, y)
}

fn pixel_rect(rect: Rect, cell: f32, (x, y): (usize, usize)) -> Rect {
    Rect::from_min_size(rect.min + vec2(x as f32 * cell, y as f32 * cell), vec2(cell, cell))
}

/// Draws the window.
pub fn window(ctx: &egui::Context, ed: &mut Editor, slot: &mut Option<TileEditor>, area: Rect) {
    let Some(te) = slot.as_mut() else { return };
    let mut close = false;
    let mut save = false;
    let rgb = ed.brush_rgb;
    // Tiles one could copy from: the built-in ones and those in this map.
    let mut sources: Vec<(String, TileArt)> = (1..=texture::TEXTURE_COUNT)
        .filter_map(|id| TileArt::from_builtin(id).map(|a| (texture::name(id).to_string(), a)))
        .collect();
    sources.extend(ed.world.tiles().map(|(_, t)| (t.name.clone(), t.art.clone())));
    te.canvas = None;
    te.save_button = None;
    let cell = 14.0_f32;
    egui::Window::new("Tile editor")
        .constrain_to(area)
        .collapsible(false)
        .resizable(false)
        .default_pos(pos2(300.0, 60.0))
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Name");
                ui.add(egui::TextEdit::singleline(&mut te.name).desired_width(160.0));
                ui.separator();
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
                    te.selection = None;
                    te.revision += 1;
                }
            });

            ui.horizontal_top(|ui| {
                // ---- canvas ------------------------------------------------
                let side = cell * TILE as f32;
                ui.vertical(|ui| {
                    let (rect, resp) = ui.allocate_exact_size(vec2(side, side), Sense::click_and_drag());
                    te.canvas = Some(rect);
                    let painter = ui.painter_at(rect);
                    checker(&painter, rect);
                    let art = te.current_ref();
                    for y in 0..TILE {
                        for x in 0..TILE {
                            let (s, a) = art.get(x, y);
                            if a > 0 {
                                let t = tinted(s, rgb);
                                painter.rect_filled(
                                    pixel_rect(rect, cell, (x, y)),
                                    0.0,
                                    Color32::from_rgba_unmultiplied(t[0], t[1], t[2], a),
                                );
                            }
                        }
                    }
                    if te.grid {
                        let line = Stroke::new(1.0, Color32::from_white_alpha(28));
                        for i in 1..TILE {
                            let o = i as f32 * cell;
                            painter.line_segment([pos2(rect.left() + o, rect.top()), pos2(rect.left() + o, rect.bottom())], line);
                            painter.line_segment([pos2(rect.left(), rect.top() + o), pos2(rect.right(), rect.top() + o)], line);
                        }
                    }
                    painter.rect_stroke(rect, 0.0, Stroke::new(1.0, Color32::BLACK), egui::StrokeKind::Inside);
                    // Mirror axes.
                    let axis = Stroke::new(1.5, Color32::from_rgba_unmultiplied(80, 160, 255, 200));
                    if te.mirror_x {
                        painter.line_segment([pos2(rect.center().x, rect.top()), pos2(rect.center().x, rect.bottom())], axis);
                    }
                    if te.mirror_y {
                        painter.line_segment([pos2(rect.left(), rect.center().y), pos2(rect.right(), rect.center().y)], axis);
                    }
                    if te.edge {
                        // Mark where the forest tile sits (below the picture).
                        let y = rect.bottom() - 1.0;
                        painter.line_segment(
                            [pos2(rect.left(), y), pos2(rect.right(), y)],
                            Stroke::new(3.0, Color32::from_rgb(220, 60, 60)),
                        );
                    }

                    // ---- mouse ---------------------------------------------
                    let hover = resp.hover_pos().map(|p| pixel_at(rect, cell, p));
                    let down_pos = ui.input(|i| i.pointer.interact_pos());
                    let pressed = resp.hovered() && ui.input(|i| i.pointer.primary_pressed());
                    let released = ui.input(|i| i.pointer.primary_released());
                    let cur = down_pos.map(|p| pixel_at(rect, cell, p));

                    if resp.secondary_clicked() || (resp.hovered() && ui.input(|i| i.pointer.secondary_pressed())) {
                        if let Some((x, y)) = hover {
                            let (s, a) = te.current_ref().get(x, y);
                            if a > 0 {
                                te.shade = s;
                            }
                        }
                    }
                    match te.tool {
                        Tool::Pencil | Tool::Erase => {
                            if pressed {
                                te.push_undo();
                            }
                            let erase = te.tool == Tool::Erase;
                            if let (true, Some(c)) = (resp.dragged() || resp.clicked(), cur) {
                                te.line_to(c, erase);
                            }
                            if !resp.dragged() && !pressed {
                                te.last = None;
                            }
                        }
                        Tool::Fill => {
                            if resp.clicked() {
                                if let Some(c) = cur {
                                    te.push_undo();
                                    te.fill_at(c.0, c.1);
                                }
                            }
                        }
                        Tool::Pick => {
                            if resp.clicked() {
                                if let Some((x, y)) = cur {
                                    let (s, a) = te.current_ref().get(x, y);
                                    if a > 0 {
                                        te.shade = s;
                                    }
                                    te.tool = Tool::Pencil;
                                }
                            }
                        }
                        Tool::Line | Tool::Rect | Tool::Select => {
                            if pressed {
                                te.drag_start = cur;
                            }
                            if let (Some(a), Some(b)) = (te.drag_start, cur) {
                                // Preview while the button is down.
                                if ui.input(|i| i.pointer.primary_down()) {
                                    let col = Color32::from_rgba_unmultiplied(255, 255, 255, 160);
                                    if te.tool == Tool::Select {
                                        let r = pixel_rect(rect, cell, (a.0.min(b.0), a.1.min(b.1)))
                                            .union(pixel_rect(rect, cell, (a.0.max(b.0), a.1.max(b.1))));
                                        painter.rect_stroke(r, 0.0, Stroke::new(2.0, col), egui::StrokeKind::Inside);
                                    } else {
                                        let pts = if te.tool == Tool::Line { line_points(a, b) } else { rect_points(a, b, te.filled) };
                                        for p in pts {
                                            painter.rect_filled(pixel_rect(rect, cell, p), 0.0, col);
                                        }
                                    }
                                }
                                if released {
                                    if te.tool == Tool::Select {
                                        te.selection = Some(((a.0.min(b.0), a.1.min(b.1)), (a.0.max(b.0), a.1.max(b.1))));
                                    } else {
                                        te.push_undo();
                                        te.shape(a, b);
                                    }
                                    te.drag_start = None;
                                }
                            }
                        }
                        Tool::Stamp => {
                            if let (Some(clip), Some((x, y))) = (te.clip.clone(), hover) {
                                // Ghost of what will be put down, centred on the mouse.
                                let (ox, oy) = (x as i32 - clip.w as i32 / 2, y as i32 - clip.h as i32 / 2);
                                for cy in 0..clip.h {
                                    for cx in 0..clip.w {
                                        let i = (cy * clip.w + cx) * 2;
                                        let (tx, ty) = (ox + cx as i32, oy + cy as i32);
                                        if clip.la[i + 1] > 0 && (0..TILE as i32).contains(&tx) && (0..TILE as i32).contains(&ty) {
                                            let t = tinted(clip.la[i], rgb);
                                            painter.rect_filled(
                                                pixel_rect(rect, cell, (tx as usize, ty as usize)),
                                                0.0,
                                                Color32::from_rgba_unmultiplied(t[0], t[1], t[2], 170),
                                            );
                                        }
                                    }
                                }
                                if resp.clicked() {
                                    te.stamp(ox, oy);
                                }
                            }
                        }
                    }
                    if let Some((a, b)) = te.selection {
                        let r = pixel_rect(rect, cell, a).union(pixel_rect(rect, cell, b));
                        painter.rect_stroke(r, 0.0, Stroke::new(3.0, Color32::BLACK), egui::StrokeKind::Outside);
                        painter.rect_stroke(r, 0.0, Stroke::new(1.5, Color32::WHITE), egui::StrokeKind::Outside);
                    }
                    let info = match hover {
                        Some((x, y)) => {
                            let (s, a) = te.current_ref().get(x, y);
                            if a > 0 { format!("x {x}, y {y}   shade {s}") } else { format!("x {x}, y {y}   empty") }
                        }
                        None => " ".to_string(),
                    };
                    ui.label(RichText::new(info).monospace().small());
                });

                // ---- tools and options --------------------------------------
                ui.vertical(|ui| {
                    ui.set_width(250.0);
                    ui.horizontal_wrapped(|ui| {
                        for (tool, label, tip) in Tool::ALL {
                            if ui.selectable_label(te.tool == tool, label).on_hover_text(tip).clicked() {
                                te.tool = tool;
                            }
                        }
                        if te.clip.is_some()
                            && ui.selectable_label(te.tool == Tool::Stamp, "Stamp").on_hover_text("Click to put the copied pixels down").clicked()
                        {
                            te.tool = Tool::Stamp;
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label("Brush");
                        ui.add(
                            egui::Slider::new(&mut te.brush, 1..=MAX_BRUSH)
                                .custom_formatter(|v, _| format!("{0}x{0}", v as i64)),
                        );
                    });
                    ui.horizontal_wrapped(|ui| {
                        ui.checkbox(&mut te.mirror_x, "Mirror L/R").on_hover_text("Draw the other half at the same time");
                        ui.checkbox(&mut te.mirror_y, "Mirror U/D");
                        ui.checkbox(&mut te.filled, "Filled");
                        ui.checkbox(&mut te.grid, "Grid");
                    });
                    ui.horizontal(|ui| {
                        ui.label("Shade");
                        ui.add(egui::Slider::new(&mut te.shade, 0..=255));
                    });
                    ui.horizontal(|ui| {
                        for s in SWATCHES {
                            let (r, resp) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::click());
                            let t = tinted(s, rgb);
                            ui.painter().rect_filled(r, 2.0, Color32::from_rgb(t[0], t[1], t[2]));
                            let stroke = if te.shade == s {
                                Stroke::new(2.0, ui.visuals().selection.stroke.color)
                            } else {
                                Stroke::new(1.0, Color32::from_black_alpha(120))
                            };
                            ui.painter().rect_stroke(r, 2.0, stroke, egui::StrokeKind::Inside);
                            if resp.on_hover_text(format!("Shade {s}")).clicked() {
                                te.shade = s;
                            }
                        }
                    });
                    ui.separator();

                    ui.horizontal_wrapped(|ui| {
                        if ui.add_enabled(te.can_undo(), egui::Button::new("Undo")).clicked() {
                            te.undo();
                        }
                        if ui.add_enabled(te.can_redo(), egui::Button::new("Redo")).clicked() {
                            te.redo();
                        }
                        ui.separator();
                        let has_sel = te.selection.is_some();
                        if ui.add_enabled(has_sel, egui::Button::new("Copy")).clicked() {
                            te.copy();
                        }
                        if ui.add_enabled(has_sel, egui::Button::new("Cut")).clicked() {
                            te.cut();
                        }
                        if ui.add_enabled(te.clip.is_some(), egui::Button::new("Paste")).on_hover_text("Then click the picture to stamp it").clicked() {
                            te.tool = Tool::Stamp;
                        }
                        if ui.add_enabled(has_sel, egui::Button::new("Delete")).clicked() {
                            te.clear_selection_pixels();
                        }
                        if ui.add_enabled(has_sel, egui::Button::new("Deselect")).clicked() {
                            te.selection = None;
                        }
                    });
                    ui.label(RichText::new(if te.selection.is_some() { "Acting on the selection" } else { "Acting on the whole picture" }).weak().small());
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("Flip L/R").clicked() {
                            te.transform(|c| c.flip_h());
                        }
                        if ui.button("Flip U/D").clicked() {
                            te.transform(|c| c.flip_v());
                        }
                        if ui.button("Rotate").on_hover_text("A quarter turn clockwise (a selection must be square)").clicked() {
                            let (a, b) = te.sel_rect();
                            if a.0.abs_diff(b.0) == a.1.abs_diff(b.1) {
                                te.transform(|c| c.rotate_cw());
                            } else {
                                te.note = "Select a square to rotate it".into();
                            }
                        }
                    });
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Slide");
                        for (label, dx, dy) in [("<", -1, 0), (">", 1, 0), ("^", 0, -1), ("v", 0, 1)] {
                            if ui.small_button(label).on_hover_text("Slide the whole picture, wrapping round the edges").clicked() {
                                te.shift(dx, dy);
                            }
                        }
                        if ui.small_button("Half").on_hover_text("Slide by half a tile: the seams move to the middle so you can see and fix them").clicked() {
                            te.shift(TILE as i32 / 2, TILE as i32 / 2);
                        }
                    });
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("Lighter").clicked() {
                            te.brighten(16);
                        }
                        if ui.button("Darker").clicked() {
                            te.brighten(-16);
                        }
                        if ui.button("Grain").on_hover_text("Random speckle on every pixel").clicked() {
                            te.grain();
                        }
                    });
                    ui.horizontal_wrapped(|ui| {
                        egui::ComboBox::from_id_salt("tile_copy_from")
                            .selected_text("Copy from tile...")
                            .width(150.0)
                            .show_ui(ui, |ui| {
                                for (name, art) in &sources {
                                    if ui.selectable_label(false, name).clicked() {
                                        te.replace_current(art.clone());
                                    }
                                }
                            });
                        if ui.button("Import PNG...").clicked() {
                            if let Some(path) = rfd::FileDialog::new().add_filter("PNG", &["png"]).pick_file() {
                                match std::fs::read(&path).map_err(|e| e.to_string()).and_then(|b| tile::art_from_png(&b)) {
                                    Ok(a) => te.replace_current(a),
                                    Err(e) => te.note = e,
                                }
                            }
                        }
                        if ui.button("Clear").clicked() {
                            te.replace_current(TileArt::blank());
                        }
                        if ui.button("Solid").clicked() {
                            let s = te.shade;
                            te.replace_current(TileArt::solid(s));
                        }
                    });
                    ui.separator();

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
                        ui.image((h.id(), vec2(160.0, 160.0)));
                    }
                    if !te.edge {
                        let e = te.art.seam_error();
                        if e == 0 {
                            ui.label(RichText::new("Edges match: it repeats seamlessly").small());
                        } else {
                            ui.label(RichText::new(format!("Edges differ by {e}: look for seams in the preview")).weak().small());
                        }
                    }
                    ui.label(RichText::new("Colour: the current brush colour").weak().small());
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

#[cfg(test)]
mod tests {
    use super::*;

    fn te() -> TileEditor {
        let mut t = TileEditor::from_tile(CustomTile::new("T", TileArt::blank()), None);
        t.shade = 100;
        t
    }

    #[test]
    fn brush_sizes_and_mirroring_paint_the_expected_pixels() {
        let mut t = te();
        t.brush = 3;
        t.dab(10, 10, false);
        let painted = |t: &TileEditor| (0..TILE).flat_map(|y| (0..TILE).map(move |x| (x, y))).filter(|&(x, y)| t.art.get(x, y).1 > 0).count();
        assert_eq!(painted(&t), 9);
        assert_eq!(t.art.get(9, 9), (100, 255));
        assert_eq!(t.art.get(11, 11), (100, 255));
        t.mirror_x = true;
        t.mirror_y = true;
        t.brush = 1;
        let before = painted(&t);
        t.dab(2, 3, false);
        assert_eq!(painted(&t), before + 4);
        assert_eq!(t.art.get(TILE - 1 - 2, 3).1, 255);
        assert_eq!(t.art.get(2, TILE - 1 - 3).1, 255);
        assert_eq!(t.art.get(TILE - 3, TILE - 4).1, 255);
        // A dab at the edge is clipped, not wrapped.
        let mut e = te();
        e.brush = 4;
        e.dab(0, 0, false);
        assert_eq!(e.art.get(TILE - 1, 0).1, 0);
        // Erasing.
        t.dab(2, 3, true);
        assert_eq!(t.art.get(2, 3).1, 0);
    }

    #[test]
    fn lines_and_rectangles() {
        let l = line_points((1, 1), (5, 3));
        assert_eq!((l.first(), l.last()), (Some(&(1, 1)), Some(&(5, 3))));
        assert_eq!(l.len(), 5);
        assert_eq!(line_points((4, 4), (4, 4)), vec![(4, 4)]);
        assert_eq!(rect_points((2, 2), (4, 4), true).len(), 9);
        assert_eq!(rect_points((4, 4), (2, 2), false).len(), 8, "corners in any order, border only");
        let mut t = te();
        t.tool = Tool::Rect;
        t.shape((0, 0), (3, 3));
        assert_eq!(t.art.get(0, 2).1, 255);
        assert_eq!(t.art.get(1, 1).1, 0, "the inside of an outline stays empty");
        t.filled = true;
        t.shape((0, 0), (3, 3));
        assert_eq!(t.art.get(1, 1).1, 255);
    }

    #[test]
    fn undo_redo_copy_cut_stamp_and_transform() {
        let mut t = te();
        t.push_undo();
        t.dab(5, 5, false);
        assert!(t.can_undo());
        t.undo();
        assert_eq!(t.art.get(5, 5).1, 0);
        assert!(t.can_redo());
        t.redo();
        assert_eq!(t.art.get(5, 5).1, 255);

        // Copy a 2x1 piece and stamp it elsewhere.
        t.art.set(6, 5, 200, 255);
        t.selection = Some(((5, 5), (6, 5)));
        t.copy();
        t.stamp(20, 20);
        assert_eq!((t.art.get(20, 20), t.art.get(21, 20)), ((100, 255), (200, 255)));
        t.undo();
        assert_eq!(t.art.get(20, 20).1, 0, "a stamp is one undo step");

        // Flipping a selection swaps its pixels.
        t.transform(|c| c.flip_h());
        assert_eq!((t.art.get(5, 5), t.art.get(6, 5)), ((200, 255), (100, 255)));

        // Cutting removes and keeps the pixels.
        t.cut();
        assert_eq!(t.art.get(5, 5).1, 0);
        assert!(t.clip.is_some());
        t.undo();
        assert_eq!(t.art.get(5, 5).1, 255);

        // Without a selection transforms act on the whole picture.
        t.selection = None;
        let before = t.art.clone();
        t.transform(|c| c.flip_v());
        assert_eq!(t.art, before.flip_v());
        t.shift(3, 0);
        assert_eq!(t.art, before.flip_v().shifted(3, 0));
    }

    #[test]
    fn undo_covers_the_edge_picture_and_new_work_clears_redo() {
        let mut t = te();
        t.use_overhang = true;
        t.edge = true;
        t.push_undo();
        t.dab(1, 1, false);
        assert_eq!(t.overhang.get(1, 1).1, 255);
        assert_eq!(t.art.get(1, 1).1, 0, "the tile itself is untouched");
        t.undo();
        assert_eq!(t.overhang.get(1, 1).1, 0);
        t.push_undo();
        assert!(!t.can_redo());
    }
}
