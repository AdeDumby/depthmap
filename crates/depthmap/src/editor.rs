//! Editor state and input logic. No GPU or window types in here, so it is
//! unit-testable.

use std::path::{Path, PathBuf};

use depthmap_core::raster::{line_cells, square_cells};
use depthmap_core::{Camera, EMPTY, History, Material, World, io, palette};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tool {
    Paint,
    Erase,
}

pub struct Editor {
    pub world: World,
    pub history: History,
    pub camera: Camera,
    pub material: Material,
    /// Brush radius in cells; the brush is a `(2r+1)²` square.
    pub brush_half: i64,
    pub viewport: (f64, f64),
    pub cursor_px: Option<(f64, f64)>,
    pub path: PathBuf,
    pub status: String,
    stroke: Option<Tool>,
    last_cell: Option<(i64, i64)>,
    panning: bool,
}

impl Editor {
    pub fn new(path: PathBuf) -> Self {
        Self {
            world: World::new(),
            history: History::new(),
            camera: Camera::new(),
            material: 1,
            brush_half: 0,
            viewport: (1280.0, 720.0),
            cursor_px: None,
            path,
            status: String::new(),
            stroke: None,
            last_cell: None,
            panning: false,
        }
    }

    pub fn resize(&mut self, w: f64, h: f64) {
        self.viewport = (w, h);
    }

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
        self.begin(Tool::Paint);
    }

    pub fn begin_erase(&mut self) {
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
        let (Some(tool), Some(cell)) = (self.stroke, self.cursor_cell()) else {
            return;
        };
        let m = if tool == Tool::Paint { self.material } else { EMPTY };
        let depth = self.camera.depth;
        let half = self.brush_half;
        let from = self.last_cell.unwrap_or(cell);
        let (world, history) = (&mut self.world, &mut self.history);
        line_cells(from, cell, |x, y| {
            square_cells((x, y), half, |a, b| history.paint(world, depth, a, b, m));
        });
        self.last_cell = Some(cell);
    }

    /// Zoom around the mouse (or the window centre if it is outside).
    pub fn zoom(&mut self, factor: f64) {
        let (vw, vh) = self.viewport;
        let (sx, sy) = self.cursor_px.unwrap_or((vw * 0.5, vh * 0.5));
        self.camera.zoom_at(factor, sx, sy, vw, vh);
        // Cells changed meaning (maybe depth too); restart the line segment.
        self.last_cell = None;
        if self.stroke.is_some() {
            self.stroke_to_cursor();
        }
    }

    pub fn pan_keys(&mut self, dx: f64, dy: f64) {
        self.camera.pan_pixels(dx, dy);
        self.last_cell = None;
    }

    pub fn set_material(&mut self, m: Material) {
        self.material = m;
    }

    pub fn change_brush(&mut self, delta: i64) {
        self.brush_half = (self.brush_half + delta).clamp(0, 32);
    }

    pub fn undo(&mut self) {
        self.end_stroke();
        self.history.undo(&mut self.world);
    }

    pub fn redo(&mut self) {
        self.end_stroke();
        self.history.redo(&mut self.world);
    }

    pub fn save(&mut self) {
        self.status = match io::save(&self.world, &self.path) {
            Ok(()) => format!("saved {}", self.path.display()),
            Err(e) => format!("save failed: {e}"),
        };
    }

    pub fn load(&mut self) {
        self.status = match io::load(&self.path) {
            Ok(w) => {
                self.world = w;
                self.history.clear();
                format!("loaded {}", self.path.display())
            }
            Err(e) => format!("load failed: {e}"),
        };
    }

    pub fn open_if_exists(&mut self, path: &Path) {
        if path.exists() {
            self.load();
        }
    }

    pub fn title(&self, fps: f32, chunks: usize) -> String {
        format!(
            "depthmap | depth {} | {:.1} px/cell | {} | brush {}x{} | {} chunks | {:.0} fps{}{}",
            self.camera.depth,
            self.camera.ppc,
            palette::material_name(self.material),
            self.brush_half * 2 + 1,
            self.brush_half * 2 + 1,
            chunks,
            fps,
            if self.status.is_empty() { "" } else { " | " },
            self.status,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor() -> Editor {
        let mut e = Editor::new(PathBuf::from("test.dmap"));
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
            if e.world.get(0, x, y) != 1 {
                missing += 1;
            }
        });
        assert_eq!(missing, 0);

        e.undo();
        assert_eq!(e.world.total_chunks(), 0);
        e.redo();
        assert_eq!(e.world.get(0, a.0, a.1), 1);
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
        assert_eq!(e.world.get(d, cell.0, cell.1), 1);
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
                assert_eq!(e.world.get(0, c.0 + dx, c.1 + dy), 1);
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
}
