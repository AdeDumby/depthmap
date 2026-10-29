//! Editor state and input logic. No GPU or window types in here, so it is
//! unit-testable.

use std::path::{Path, PathBuf};

use depthmap_core::camera::BASE_PPC;
use depthmap_core::colors::DEFAULT_COLORS;
use depthmap_core::raster::{line_cells, square_cells};
use depthmap_core::{Camera, Cell, EMPTY, History, MAX_DEPTH, World, io, make_cell};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tool {
    Paint,
    Erase,
}

pub struct Editor {
    pub world: World,
    pub history: History,
    pub camera: Camera,
    /// Texture id painted by the left button (`1..=TEXTURE_COUNT`).
    pub texture: u8,
    /// Colour painted by the left button. It enters the world's colour table
    /// only when a stroke actually starts, so picking colours is free.
    pub brush_rgb: [u8; 3],
    /// Brush radius in cells; the brush is a `(2r+1)²` square.
    pub brush_half: i64,
    pub viewport: (f64, f64),
    pub cursor_px: Option<(f64, f64)>,
    /// File the map is saved to; `None` for an untitled map.
    pub path: Option<PathBuf>,
    pub status: String,
    saved_revision: u64,
    stroke: Option<Tool>,
    stroke_cell: Cell,
    last_cell: Option<(i64, i64)>,
    panning: bool,
}

impl Default for Editor {
    fn default() -> Self {
        Self::new()
    }
}

impl Editor {
    pub fn new() -> Self {
        let world = World::new();
        Self {
            saved_revision: world.revision(),
            world,
            history: History::new(),
            camera: Camera::new(),
            texture: 2,
            brush_rgb: DEFAULT_COLORS[2],
            brush_half: 0,
            viewport: (1280.0, 720.0),
            cursor_px: None,
            path: None,
            status: String::new(),
            stroke: None,
            stroke_cell: EMPTY,
            last_cell: None,
            panning: false,
        }
    }

    pub fn resize(&mut self, w: f64, h: f64) {
        self.viewport = (w, h);
    }

    fn centre(&self) -> (f64, f64) {
        (self.viewport.0 * 0.5, self.viewport.1 * 0.5)
    }

    // ---- painting ---------------------------------------------------------

    /// Cell at the active depth under the mouse, if it is over the window.
    pub fn cursor_cell(&self) -> Option<(i64, i64)> {
        let (x, y) = self.cursor_px?;
        Some(self.camera.screen_to_cell(x, y, self.viewport.0, self.viewport.1))
    }

    pub fn mouse_moved(&mut self, x: f64, y: f64) {
        if let (true, Some((px, py))) = (self.panning, self.cursor_px) {
            self.camera.pan_pixels(x - px, y - py);
            self.last_cell = None;
        }
        self.cursor_px = Some((x, y));
        if self.stroke.is_some() {
            self.stroke_to_cursor();
        }
    }

    pub fn mouse_left_window(&mut self) {
        self.cursor_px = None;
        self.end_stroke();
        self.panning = false;
    }

    pub fn begin_paint(&mut self) {
        let color = self.world.colors.index_of(self.brush_rgb);
        self.stroke_cell = make_cell(self.texture, color);
        self.begin(Tool::Paint);
    }

    pub fn begin_erase(&mut self) {
        self.stroke_cell = EMPTY;
        self.begin(Tool::Erase);
    }

    fn begin(&mut self, tool: Tool) {
        if self.cursor_px.is_none() {
            return;
        }
        self.stroke = Some(tool);
        self.last_cell = None;
        self.stroke_to_cursor();
    }

    pub fn end_stroke(&mut self) {
        self.stroke = None;
        self.last_cell = None;
        self.history.end_stroke();
    }

    pub fn set_panning(&mut self, on: bool) {
        self.panning = on;
    }

    fn stroke_to_cursor(&mut self) {
        let (Some(_), Some(cell)) = (self.stroke, self.cursor_cell()) else {
            return;
        };
        let m = self.stroke_cell;
        let depth = self.camera.depth;
        let half = self.brush_half;
        let from = self.last_cell.unwrap_or(cell);
        let (world, history) = (&mut self.world, &mut self.history);
        line_cells(from, cell, |x, y| {
            square_cells((x, y), half, |a, b| history.paint(world, depth, a, b, m));
        });
        self.last_cell = Some(cell);
    }

    pub fn set_texture(&mut self, id: u8) {
        self.texture = id;
    }

    pub fn set_brush_rgb(&mut self, rgb: [u8; 3]) {
        self.brush_rgb = rgb;
    }

    pub fn change_brush(&mut self, delta: i64) {
        self.brush_half = (self.brush_half + delta).clamp(0, 32);
    }

    // ---- view -------------------------------------------------------------

    /// Zoom around the mouse (or the window centre if it is outside).
    pub fn zoom(&mut self, factor: f64) {
        let (vw, vh) = self.viewport;
        let (sx, sy) = self.cursor_px.unwrap_or_else(|| self.centre());
        self.camera.zoom_at(factor, sx, sy, vw, vh);
        self.view_changed();
    }

    /// Zoom around the window centre (used by the zoom panel).
    pub fn zoom_centre(&mut self, factor: f64) {
        let (vw, vh) = self.viewport;
        let (sx, sy) = self.centre();
        self.camera.zoom_at(factor, sx, sy, vw, vh);
        self.view_changed();
    }

    /// Total zoom as a base-2 logarithm; see [`Camera::zoom_level`].
    pub fn zoom_level(&self) -> f64 {
        self.camera.zoom_level()
    }

    pub fn set_zoom_level(&mut self, level: f64) {
        let (vw, vh) = self.viewport;
        let (sx, sy) = self.centre();
        self.camera.set_zoom_level(level, sx, sy, vw, vh);
        self.view_changed();
    }

    /// Jumps `delta` depths deeper (or shallower), landing on the default
    /// cell size of that depth.
    pub fn jump_depth(&mut self, delta: i32) {
        let target = (self.camera.depth as i32 + delta).clamp(0, MAX_DEPTH as i32);
        self.set_zoom_level(2.0 * target as f64);
    }

    /// Back to the default zoom (depth 0, 32 px/cell), keeping the position.
    pub fn reset_zoom(&mut self) {
        self.set_zoom_level(0.0);
    }

    /// Back to depth 0 at the default zoom, centred on the origin.
    pub fn reset_view(&mut self) {
        self.camera = Camera::new();
        self.view_changed();
    }

    pub fn pan_keys(&mut self, dx: f64, dy: f64) {
        self.camera.pan_pixels(dx, dy);
        self.last_cell = None;
    }

    fn view_changed(&mut self) {
        // Cells changed meaning (maybe depth too); restart the line segment.
        self.last_cell = None;
        if self.stroke.is_some() {
            self.stroke_to_cursor();
        }
    }

    // ---- history and files ------------------------------------------------

    pub fn undo(&mut self) {
        self.end_stroke();
        self.history.undo(&mut self.world);
    }

    pub fn redo(&mut self) {
        self.end_stroke();
        self.history.redo(&mut self.world);
    }

    /// True if the map changed since it was last saved, loaded or created.
    pub fn modified(&self) -> bool {
        self.world.revision() != self.saved_revision
    }

    pub fn file_name(&self) -> String {
        self.path
            .as_deref()
            .and_then(|p| p.file_name())
            .map_or_else(|| "Untitled".to_string(), |n| n.to_string_lossy().into_owned())
    }

    /// Starts an empty map.
    pub fn new_map(&mut self) {
        self.end_stroke();
        self.world = World::new();
        self.history.clear();
        self.camera = Camera::new();
        self.path = None;
        self.saved_revision = self.world.revision();
        self.status.clear();
    }

    /// Saves to the current path. Returns false if there is no path yet.
    pub fn save(&mut self) -> bool {
        match self.path.clone() {
            Some(p) => {
                self.save_to(&p);
                true
            }
            None => false,
        }
    }

    pub fn save_to(&mut self, path: &Path) {
        self.end_stroke();
        match io::save(&self.world, path) {
            Ok(()) => {
                self.path = Some(path.to_path_buf());
                self.saved_revision = self.world.revision();
                self.status = format!("Saved {}", path.display());
            }
            Err(e) => self.status = format!("Save failed: {e}"),
        }
    }

    pub fn load_from(&mut self, path: &Path) {
        self.end_stroke();
        match io::load(path) {
            Ok(w) => {
                self.world = w;
                self.history.clear();
                self.camera = Camera::new();
                self.path = Some(path.to_path_buf());
                self.saved_revision = self.world.revision();
                self.status = format!("Opened {}", path.display());
            }
            Err(e) => self.status = format!("Open failed: {e}"),
        }
    }

    /// Suggested file name for the "Save as" dialog.
    pub fn file_name_for_dialog(&self) -> String {
        match &self.path {
            Some(_) => self.file_name(),
            None => "map.dmap".to_string(),
        }
    }

    pub fn title(&self) -> String {
        format!(
            "{}{} - depthmap",
            self.file_name(),
            if self.modified() { " *" } else { "" }
        )
    }

    /// Human-readable zoom factor relative to the default view, e.g. `x16`.
    pub fn zoom_label(&self) -> String {
        format_zoom(self.zoom_level())
    }

    pub fn pixels_per_cell(&self) -> f64 {
        self.camera.ppc
    }

    pub const BASE_PPC: f64 = BASE_PPC;
}

/// Formats a zoom level (log2 scale) as a multiplier: `x0.25`, `x16`, `x1.0e6`.
pub fn format_zoom(level: f64) -> String {
    let z = level.exp2();
    if z >= 100_000.0 {
        format!("x{z:.1e}")
    } else if z >= 100.0 {
        format!("x{z:.0}")
    } else if z >= 10.0 {
        format!("x{z:.1}")
    } else {
        format!("x{z:.2}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use depthmap_core::{cell_color, cell_texture};

    fn editor() -> Editor {
        let mut e = Editor::new();
        e.resize(1000.0, 800.0);
        e
    }

    /// Screen position of the centre of a cell at the active depth.
    fn centre_of(e: &Editor, cell: (i64, i64)) -> (f64, f64) {
        let v = e.camera.view();
        let (x, y) = v.cell_origin_px(cell.0, cell.1, e.viewport.0, e.viewport.1);
        (x + v.ppc * 0.5, y + v.ppc * 0.5)
    }

    #[test]
    fn fast_drag_leaves_no_gaps_and_undoes_as_one_stroke() {
        let mut e = editor();
        e.mouse_moved(100.0, 100.0);
        e.begin_paint();
        e.mouse_moved(900.0, 500.0); // jump of many cells in one event
        e.end_stroke();

        let (a, b) = (
            e.camera.screen_to_cell(100.0, 100.0, 1000.0, 800.0),
            e.camera.screen_to_cell(900.0, 500.0, 1000.0, 800.0),
        );
        let mut missing = 0;
        line_cells(a, b, |x, y| {
            if cell_texture(e.world.get(0, x, y)) != e.texture {
                missing += 1;
            }
        });
        assert_eq!(missing, 0);

        e.undo();
        assert_eq!(e.world.total_chunks(), 0);
        e.redo();
        assert_ne!(e.world.get(0, a.0, a.1), EMPTY);
    }

    #[test]
    fn strokes_use_the_selected_texture_and_any_colour() {
        let mut e = editor();
        e.set_texture(5);
        e.set_brush_rgb([12, 200, 99]); // not in the default table
        let before = e.world.colors.len();
        e.mouse_moved(500.0, 400.0);
        e.begin_paint();
        e.end_stroke();
        let c = e.cursor_cell().unwrap();
        let cell = e.world.get(0, c.0, c.1);
        assert_eq!(cell_texture(cell), 5);
        assert_eq!(e.world.colors.get(cell_color(cell)), [12, 200, 99]);
        assert_eq!(e.world.colors.len(), before + 1);

        // Painting again with the same colour reuses the table entry.
        e.mouse_moved(560.0, 400.0);
        e.begin_paint();
        e.end_stroke();
        assert_eq!(e.world.colors.len(), before + 1);
    }

    #[test]
    fn previewing_colours_does_not_touch_the_table() {
        let mut e = editor();
        let before = e.world.colors.len();
        for i in 0..50u8 {
            e.set_brush_rgb([i, i, i]);
        }
        assert_eq!(e.world.colors.len(), before);
    }

    #[test]
    fn painting_after_zooming_in_goes_to_the_deeper_layer() {
        let mut e = editor();
        e.mouse_moved(500.0, 400.0);
        for _ in 0..8 {
            e.zoom(1.5);
        }
        assert!(e.camera.depth >= 1);
        let d = e.camera.depth;
        let cell = e.cursor_cell().unwrap();
        let (x, y) = centre_of(&e, cell);
        e.mouse_moved(x, y);
        e.begin_paint();
        e.end_stroke();
        assert_ne!(e.world.get(d, cell.0, cell.1), EMPTY);
        assert!(e.world.layer_is_empty(0));
    }

    #[test]
    fn right_button_erases_and_brush_is_square() {
        let mut e = editor();
        e.brush_half = 1;
        e.mouse_moved(500.0, 400.0);
        e.begin_paint();
        e.end_stroke();
        let c = e.cursor_cell().unwrap();
        for dy in -1..=1 {
            for dx in -1..=1 {
                assert_ne!(e.world.get(0, c.0 + dx, c.1 + dy), EMPTY);
            }
        }
        e.begin_erase();
        e.end_stroke();
        assert_eq!(e.world.total_chunks(), 0);
    }

    #[test]
    fn panning_moves_the_view_not_the_map() {
        let mut e = editor();
        e.mouse_moved(500.0, 400.0);
        e.set_panning(true);
        let before = e.camera.world_pos();
        e.mouse_moved(564.0, 400.0); // drag right by 2 cells (32 px/cell)
        let after = e.camera.world_pos();
        assert!((before.0 - after.0 - 2.0).abs() < 1e-9);
        assert_eq!(e.world.total_chunks(), 0);
    }

    #[test]
    fn zoom_panel_actions() {
        let mut e = editor();
        e.zoom_centre(2.0);
        assert!((e.zoom_level() - 1.0).abs() < 1e-9);
        // x2 crossed the depth threshold, so we are already at depth 1.
        assert_eq!(e.camera.depth, 1);
        e.jump_depth(2);
        assert_eq!(e.camera.depth, 3);
        assert!((e.zoom_level() - 6.0).abs() < 1e-9);
        e.jump_depth(-5);
        assert_eq!(e.camera.depth, 0);
        e.set_zoom_level(3.0);
        e.reset_zoom();
        assert!(e.zoom_level().abs() < 1e-9);
        assert_eq!(format_zoom(0.0), "x1.00");
        assert_eq!(format_zoom(4.0), "x16.0");
        assert_eq!(format_zoom(-2.0), "x0.25");
        assert_eq!(format_zoom(40.0), "x1.1e12");
    }

    #[test]
    fn modified_flag_and_file_round_trip() {
        let dir = std::env::temp_dir().join(format!("depthmap-editor-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.dmap");

        let mut e = editor();
        assert!(!e.modified());
        assert_eq!(e.title(), "Untitled - depthmap");
        assert!(!e.save(), "untitled map has no path to save to");
        e.mouse_moved(500.0, 400.0);
        e.begin_paint();
        e.end_stroke();
        assert!(e.modified());
        assert!(e.title().contains('*'));

        e.save_to(&path);
        assert!(!e.modified());
        assert_eq!(e.file_name(), "a.dmap");
        assert!(e.save());

        let mut e2 = editor();
        e2.load_from(&path);
        assert!(!e2.modified());
        assert_eq!(e2.world.total_chunks(), 1);
        e2.new_map();
        assert_eq!(e2.world.total_chunks(), 0);
        assert!(e2.path.is_none());

        e2.load_from(&dir.join("missing.dmap"));
        assert!(e2.status.starts_with("Open failed"));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
