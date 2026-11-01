//! Editor state and input logic. No GPU or window types in here, so it is
//! unit-testable.

use std::path::{Path, PathBuf};

use depthmap_core::camera::{BASE_PPC, MIN_PPC};
use depthmap_core::colors::DEFAULT_COLORS;
use depthmap_core::raster::{line_cells, square_cells};
use depthmap_core::shape::{Clip, shape_cells};
use depthmap_core::{
    Camera, Cell, CellEdit, EMPTY, Fill, History, MapSpec, Shape, ShapeStyle, World, io, make_cell,
};

/// Fraction of the window the whole map fills when fully zoomed out.
const FIT_MARGIN: f64 = 0.92;
/// Never zoom out past this, however large the map.
const MIN_FIT_PPC: f64 = 0.02;

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
    /// Brush radius in cells; the brush is a `(2r+1)²` square. Shape edges
    /// use it as their thickness.
    pub brush_half: i64,
    /// What the right mouse button draws (its thickness is `brush_half`).
    pub shape: Shape,
    pub fill: Fill,
    /// Also draw a solid edge around a patterned or solid shape.
    pub border: bool,
    /// Draw with the eraser instead of the texture (brush and shapes).
    pub eraser: bool,
    pub viewport: (f64, f64),
    pub cursor_px: Option<(f64, f64)>,
    /// File the map is saved to; `None` for an untitled map.
    pub path: Option<PathBuf>,
    pub status: String,
    saved_revision: u64,
    /// Last file successfully read or written, until taken (see
    /// [`Editor::take_touched_path`]).
    touched: Option<PathBuf>,
    stroke: Option<Tool>,
    stroke_cell: Cell,
    last_cell: Option<(i64, i64)>,
    panning: bool,
    shape_drag: Option<ShapeDrag>,
    shape_buf: Vec<(i64, i64)>,
}

/// The cell under the mouse, see [`Editor::hover_info`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HoverInfo {
    pub depth: u8,
    pub cell: (i64, i64),
    /// The cell one depth up that contains it (none at depth 0).
    pub parent: Option<(i64, i64)>,
    /// False when the mouse is outside the map.
    pub inside: bool,
}

impl HoverInfo {
    /// e.g. `d2 cell 13,7 in d1 3,1`
    pub fn text(&self) -> String {
        let mut t = format!("d{} cell {},{}", self.depth, self.cell.0, self.cell.1);
        if let Some((px, py)) = self.parent {
            t.push_str(&format!("  in d{} cell {},{}", self.depth - 1, px, py));
        }
        t
    }
}

/// A shape being dragged out with the right mouse button. It is applied to
/// the map live (so you see the real texture and pattern) and rewound and
/// redrawn whenever the mouse moves; releasing keeps it as one undo step.
struct ShapeDrag {
    depth: u8,
    anchor: (i64, i64),
    cell: Cell,
    /// Cells the current preview changed, with what they held before.
    edits: Vec<CellEdit>,
    /// World revision before the preview touched anything.
    revision: u64,
    /// Inputs of the last draw, to skip redrawing when nothing changed.
    key: Option<(u8, (i64, i64), (i64, i64), Clip, ShapeStyle)>,
}

impl Default for Editor {
    fn default() -> Self {
        Self::new()
    }
}

impl Editor {
    /// An editor on an empty, unbounded map.
    pub fn new() -> Self {
        Self::with_world(World::new())
    }

    /// An editor on an empty map of the given size and depth.
    pub fn with_spec(spec: MapSpec) -> Self {
        let mut e = Self::with_world(World::with_spec(spec));
        e.fit_view();
        e
    }

    fn with_world(world: World) -> Self {
        Self {
            saved_revision: world.revision(),
            world,
            history: History::new(),
            camera: Camera::new(),
            texture: 2,
            brush_rgb: DEFAULT_COLORS[2],
            brush_half: 0,
            shape: Shape::Rectangle,
            fill: Fill::Outline,
            border: false,
            eraser: false,
            viewport: (1280.0, 720.0),
            cursor_px: None,
            path: None,
            status: String::new(),
            touched: None,
            stroke: None,
            stroke_cell: EMPTY,
            last_cell: None,
            panning: false,
            shape_drag: None,
            shape_buf: Vec::new(),
        }
    }

    pub fn resize(&mut self, w: f64, h: f64) {
        // A view showing the whole map keeps showing the whole map.
        let was_fit = self.camera.depth == 0 && self.camera.ppc <= self.camera.min_ppc * 1.0001;
        self.viewport = (w, h);
        if was_fit && self.spec().is_bounded() {
            self.camera.ppc = self.min_ppc();
        }
        self.constrain();
    }

    pub fn spec(&self) -> MapSpec {
        self.world.spec()
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

    /// What is under the mouse: the cell at the active depth and the parent
    /// cell (the "superpixel", one depth up) it belongs to.
    pub fn hover_info(&self) -> Option<HoverInfo> {
        let cell = self.cursor_cell()?;
        let depth = self.camera.depth;
        let s = depthmap_core::SUBDIV;
        Some(HoverInfo {
            depth,
            cell,
            parent: (depth > 0).then(|| (cell.0.div_euclid(s), cell.1.div_euclid(s))),
            inside: self.world.in_bounds(depth, cell.0, cell.1),
        })
    }

    pub fn mouse_moved(&mut self, x: f64, y: f64) {
        if let (true, Some((px, py))) = (self.panning, self.cursor_px) {
            self.camera.pan_pixels(x - px, y - py);
            self.constrain();
            self.last_cell = None;
        }
        self.cursor_px = Some((x, y));
        if self.stroke.is_some() {
            self.stroke_to_cursor();
        }
        self.update_shape();
    }

    pub fn mouse_left_window(&mut self) {
        self.cursor_px = None;
        self.end_stroke();
        self.panning = false;
    }

    /// Starts a brush stroke with the texture (or the eraser, if selected).
    pub fn begin_paint(&mut self) {
        if self.eraser {
            return self.begin_erase();
        }
        let color = self.world.colors.index_of(self.brush_rgb);
        self.stroke_cell = make_cell(self.texture, color);
        self.begin(Tool::Paint);
    }

    pub fn begin_erase(&mut self) {
        self.stroke_cell = EMPTY;
        self.begin(Tool::Erase);
    }

    fn begin(&mut self, tool: Tool) {
        if self.cursor_px.is_none() || self.shape_drag.is_some() {
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

    /// Lowest and highest zoom level this map allows: the lowest shows the
    /// whole map, the highest is the deepest depth at full zoom.
    pub fn zoom_range(&self) -> (f64, f64) {
        self.camera.zoom_range()
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
        let target = (self.camera.depth as i32 + delta).clamp(0, self.spec().max_depth as i32);
        self.set_zoom_level(2.0 * target as f64);
    }

    /// Back to the default zoom (depth 0, 32 px/cell), keeping the position.
    pub fn reset_zoom(&mut self) {
        self.set_zoom_level(0.0);
    }

    /// Shows the whole map: fully zoomed out and centred (an unbounded map
    /// returns to the origin at the default zoom).
    pub fn reset_view(&mut self) {
        self.fit_view();
        self.view_changed();
    }

    fn fit_view(&mut self) {
        let mut cam = Camera::new();
        cam.min_ppc = self.min_ppc();
        cam.max_depth = self.spec().max_depth;
        if let Some((w, h)) = self.spec().extent(0) {
            cam.cell = (w / 2, h / 2);
            cam.frac = ((w % 2) as f64 * 0.5, (h % 2) as f64 * 0.5);
            cam.ppc = cam.min_ppc;
        }
        self.camera = cam;
        self.constrain();
    }

    /// Smallest pixels-per-cell at depth 0: the whole map just fits.
    fn min_ppc(&self) -> f64 {
        match self.spec().extent(0) {
            Some((w, h)) => {
                let fit = (self.viewport.0 / w as f64).min(self.viewport.1 / h as f64);
                (fit * FIT_MARGIN).clamp(MIN_FIT_PPC, BASE_PPC)
            }
            None => MIN_PPC,
        }
    }

    /// Re-applies the map's limits to the camera: zoom range, depth, and
    /// keeping the view over the map.
    fn constrain(&mut self) {
        self.camera.min_ppc = self.min_ppc();
        self.camera.max_depth = self.spec().max_depth;
        self.camera.normalize();
        let Some((w, h)) = self.spec().extent(self.camera.depth) else { return };
        let (vw, vh) = self.viewport;
        let ppc = self.camera.ppc;
        let cam = &mut self.camera;
        for (extent, view_px, cell, frac) in [
            (w, vw, &mut cam.cell.0, &mut cam.frac.0),
            (h, vh, &mut cam.cell.1, &mut cam.frac.1),
        ] {
            if view_px / ppc >= extent as f64 {
                // The whole axis is on screen: keep the map centred.
                *cell = extent / 2;
                *frac = (extent % 2) as f64 * 0.5;
            } else if *cell < 0 {
                (*cell, *frac) = (0, 0.0);
            } else if *cell >= extent {
                (*cell, *frac) = (extent - 1, 0.999_999);
            }
        }
    }

    pub fn pan_keys(&mut self, dx: f64, dy: f64) {
        self.camera.pan_pixels(dx, dy);
        self.constrain();
        self.last_cell = None;
        self.update_shape();
    }

    fn view_changed(&mut self) {
        self.constrain();
        // Cells changed meaning (maybe depth too); restart the line segment.
        self.last_cell = None;
        if self.stroke.is_some() {
            self.stroke_to_cursor();
        }
        self.update_shape();
    }

    // ---- shapes -----------------------------------------------------------

    /// Starts dragging a shape from the cell under the mouse. With `erase`
    /// (or the eraser selected) the shape removes cells instead.
    pub fn begin_shape(&mut self, erase: bool) {
        let Some(anchor) = self.cursor_cell() else { return };
        if self.stroke.is_some() {
            return;
        }
        self.end_shape(); // a previous drag whose release was lost
        let cell = if erase || self.eraser {
            EMPTY
        } else {
            make_cell(self.texture, self.world.colors.index_of(self.brush_rgb))
        };
        self.shape_drag = Some(ShapeDrag {
            depth: self.camera.depth,
            anchor,
            cell,
            edits: Vec::new(),
            revision: self.world.revision(),
            key: None,
        });
        self.update_shape();
    }

    pub fn shape_active(&self) -> bool {
        self.shape_drag.is_some()
    }

    /// Keeps the shape being dragged; it becomes one undo step.
    pub fn end_shape(&mut self) {
        if let Some(drag) = self.shape_drag.take() {
            self.history.record_applied(drag.edits);
        }
    }

    /// Drops the shape being dragged, restoring the map exactly.
    pub fn cancel_shape(&mut self) {
        if let Some(mut drag) = self.shape_drag.take() {
            Self::rewind(&mut self.world, &mut drag);
            self.world.rewind_revision(drag.revision);
        }
    }

    fn rewind(world: &mut World, drag: &mut ShapeDrag) {
        for e in drag.edits.iter().rev() {
            world.set(e.depth, e.x, e.y, e.old);
        }
        drag.edits.clear();
        drag.key = None;
    }

    /// Cells of the active depth that are on screen and part of the map.
    fn shape_clip(&self) -> Clip {
        let (vw, vh) = self.viewport;
        let ((x0, y0), (x1, y1)) = self.camera.view().visible_cells(vw, vh);
        let mut clip = ((x0 - 1, y0 - 1), (x1 + 1, y1 + 1));
        if let Some((w, h)) = self.spec().extent(self.camera.depth) {
            clip = (
                (clip.0.0.max(0), clip.0.1.max(0)),
                (clip.1.0.min(w - 1), clip.1.1.min(h - 1)),
            );
        }
        clip
    }

    /// Redraws the dragged shape for the current mouse position and view.
    fn update_shape(&mut self) {
        let Some(mut drag) = self.shape_drag.take() else { return };
        if let Some(cursor) = self.cursor_cell() {
            let depth = self.camera.depth;
            // The view moved to another depth mid-drag: keep the anchor at
            // the same place (the middle of its cell, if it got finer).
            if drag.depth != depth {
                drag.anchor = convert_cell(drag.anchor, drag.depth, depth);
                drag.depth = depth;
            }
            let style = ShapeStyle {
                shape: self.shape,
                fill: self.fill,
                border: self.border,
                half: self.brush_half,
            };
            let clip = self.shape_clip();
            let key = (depth, drag.anchor, cursor, clip, style);
            if drag.key != Some(key) {
                Self::rewind(&mut self.world, &mut drag);
                shape_cells(&style, drag.anchor, cursor, clip, &mut self.shape_buf);
                for &(x, y) in &self.shape_buf {
                    let old = self.world.set(depth, x, y, drag.cell);
                    if old != drag.cell {
                        drag.edits.push(CellEdit { depth, x, y, old, new: drag.cell });
                    }
                }
                drag.key = Some(key);
            }
        }
        self.shape_drag = Some(drag);
    }

    // ---- history and files ------------------------------------------------

    pub fn undo(&mut self) {
        self.end_stroke();
        self.end_shape();
        self.history.undo(&mut self.world);
    }

    pub fn redo(&mut self) {
        self.end_stroke();
        self.end_shape();
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

    /// Starts an empty map of the given size and depth.
    pub fn new_map(&mut self, spec: MapSpec) {
        self.end_stroke();
        self.shape_drag = None;
        self.world = World::with_spec(spec);
        self.history.clear();
        self.path = None;
        self.saved_revision = self.world.revision();
        self.status = match spec.extent(0) {
            Some((w, h)) => format!("New map {w}x{h}, {} zoom levels", spec.max_depth),
            None => String::new(),
        };
        self.fit_view();
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
        self.end_shape();
        match io::save(&self.world, path) {
            Ok(()) => {
                self.path = Some(path.to_path_buf());
                self.saved_revision = self.world.revision();
                self.touched = Some(path.to_path_buf());
                self.status = format!("Saved {}", path.display());
            }
            Err(e) => self.status = format!("Save failed: {e}"),
        }
    }

    pub fn load_from(&mut self, path: &Path) {
        self.end_stroke();
        match io::load(path) {
            Ok(w) => {
                self.shape_drag = None;
                self.world = w;
                self.history.clear();
                self.path = Some(path.to_path_buf());
                self.saved_revision = self.world.revision();
                self.touched = Some(path.to_path_buf());
                self.status = format!("Opened {}", path.display());
                self.fit_view();
            }
            Err(e) => self.status = format!("Open failed: {e}"),
        }
    }

    /// The file that was last opened or saved successfully, once. The app uses
    /// this to keep its recent-maps list up to date.
    pub fn take_touched_path(&mut self) -> Option<PathBuf> {
        self.touched.take()
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

/// The cell at depth `to` that stands for `cell` at depth `from`: the
/// containing cell when going coarser, the middle sub-cell when going finer.
fn convert_cell(cell: (i64, i64), from: u8, to: u8) -> (i64, i64) {
    if to >= from {
        let s = depthmap_core::SUBDIV.pow((to - from) as u32);
        (cell.0 * s + s / 2, cell.1 * s + s / 2)
    } else {
        let s = depthmap_core::SUBDIV.pow((from - to) as u32);
        (cell.0.div_euclid(s), cell.1.div_euclid(s))
    }
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
        e2.new_map(MapSpec::UNBOUNDED);
        assert_eq!(e2.world.total_chunks(), 0);
        assert!(e2.path.is_none());

        e2.load_from(&dir.join("missing.dmap"));
        assert!(e2.status.starts_with("Open failed"));
        assert_eq!(e2.take_touched_path(), Some(path.clone()), "failed open must not count");
        assert_eq!(e2.take_touched_path(), None, "reported only once");
        std::fs::remove_dir_all(dir).unwrap();
    }

    // ---- map size / depth ---------------------------------------------------

    fn bounded(w: u32, h: u32, d: u8) -> Editor {
        let mut e = Editor::with_spec(MapSpec::new(w, h, d));
        e.resize(1000.0, 800.0);
        e
    }

    #[test]
    fn a_new_map_starts_fully_zoomed_out_and_centred() {
        let e = bounded(64, 32, 3);
        // 64 cells across 1000 px, with a margin: 14.4 px/cell.
        assert!((e.camera.ppc - 1000.0 / 64.0 * FIT_MARGIN).abs() < 1e-9);
        assert_eq!(e.camera.depth, 0);
        let (lo, _) = e.zoom_range();
        assert!((e.zoom_level() - lo).abs() < 1e-9);
        assert_eq!(e.camera.cell, (32, 16));
        // Both edges of the map are on screen.
        let v = e.camera.view();
        let (x0, _) = v.cell_origin_px(0, 0, 1000.0, 800.0);
        let (x1, _) = v.cell_origin_px(64, 0, 1000.0, 800.0);
        assert!(x0 > 0.0 && x1 < 1000.0);
    }

    #[test]
    fn cannot_zoom_out_past_the_map_or_in_past_its_depth() {
        let mut e = bounded(128, 128, 2);
        for _ in 0..100 {
            e.zoom(0.5);
        }
        let (lo, hi) = e.zoom_range();
        assert!((e.zoom_level() - lo).abs() < 1e-9);
        for _ in 0..200 {
            e.zoom(2.0);
        }
        assert_eq!(e.camera.depth, 2);
        assert!((e.zoom_level() - hi).abs() < 1e-9);
        e.jump_depth(5);
        assert_eq!(e.camera.depth, 2);
        e.jump_depth(-9);
        assert_eq!(e.camera.depth, 0);
        e.set_zoom_level(-50.0);
        assert!((e.zoom_level() - lo).abs() < 1e-9);
    }

    #[test]
    fn small_maps_do_not_zoom_out_below_normal_size() {
        let e = bounded(8, 8, 1);
        assert_eq!(e.camera.ppc, BASE_PPC);
        let (lo, _) = e.zoom_range();
        assert!(lo.abs() < 1e-12);
    }

    #[test]
    fn painting_outside_the_map_does_nothing() {
        let mut e = bounded(16, 16, 1);
        e.set_zoom_level(0.0); // 32 px/cell: the 16-cell map is 512 px wide
        e.mouse_moved(5.0, 5.0); // far above-left of the map
        e.begin_paint();
        e.mouse_moved(995.0, 795.0);
        e.end_stroke();
        for (x, y) in [(-1, -1), (16, 16), (-3, 7)] {
            assert_eq!(e.world.get(0, x, y), EMPTY);
        }
        assert!(e.world.get(0, 8, 8) != EMPTY, "the stroke crosses the map");
        for y in -30..60 {
            for x in -30..60 {
                if !e.spec().contains(0, x, y) {
                    assert_eq!(e.world.get(0, x, y), EMPTY, "({x},{y})");
                }
            }
        }
    }

    #[test]
    fn panning_keeps_the_view_over_the_map() {
        let mut e = bounded(256, 256, 2);
        e.set_zoom_level(0.0);
        for _ in 0..200 {
            e.pan_keys(-2000.0, -2000.0);
        }
        let (w, h) = e.spec().extent(e.camera.depth).unwrap();
        assert!(e.camera.cell.0 >= 0 && e.camera.cell.0 < w);
        assert!(e.camera.cell.1 >= 0 && e.camera.cell.1 < h);
        for _ in 0..200 {
            e.pan_keys(2000.0, 2000.0);
        }
        assert!(e.camera.cell.0 < w && e.camera.cell.1 < h);
        // When the whole map is visible the view stays centred.
        e.set_zoom_level(e.zoom_range().0);
        e.pan_keys(500.0, 500.0);
        let (w0, h0) = e.spec().extent(0).unwrap();
        assert_eq!(e.camera.cell, (w0 / 2, h0 / 2));
    }

    #[test]
    fn new_map_replaces_size_depth_and_view_and_fit_view_resets() {
        let mut e = bounded(64, 64, 1);
        e.mouse_moved(500.0, 400.0);
        e.begin_paint();
        e.end_stroke();
        e.new_map(MapSpec::new(200, 100, 5));
        assert_eq!(e.spec(), MapSpec::new(200, 100, 5));
        assert_eq!(e.world.total_chunks(), 0);
        assert!(!e.modified() && e.status.contains("200x100"));
        e.set_zoom_level(6.0);
        e.reset_view();
        assert_eq!(e.camera.depth, 0);
        assert!((e.zoom_level() - e.zoom_range().0).abs() < 1e-9);
    }

    #[test]
    fn size_and_depth_survive_save_and_open() {
        let dir = std::env::temp_dir().join(format!("depthmap-size-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s.dmap");
        let mut e = bounded(96, 48, 4);
        e.save_to(&path);
        let mut e2 = editor();
        e2.load_from(&path);
        assert_eq!(e2.spec(), MapSpec::new(96, 48, 4));
        assert!((e2.zoom_level() - e2.zoom_range().0).abs() < 1e-9);
        std::fs::remove_dir_all(dir).unwrap();
    }

    // ---- shapes ----------------------------------------------------------------

    /// Mouse position (pixels) at the centre of a cell of the active depth.
    fn at(e: &mut Editor, cell: (i64, i64)) {
        let (x, y) = centre_of(e, cell);
        e.mouse_moved(x, y);
    }

    fn count(e: &Editor, depth: u8, x: std::ops::RangeInclusive<i64>, y: std::ops::RangeInclusive<i64>) -> usize {
        let mut n = 0;
        for yy in y {
            for xx in x.clone() {
                if e.world.get(depth, xx, yy) != EMPTY {
                    n += 1;
                }
            }
        }
        n
    }

    #[test]
    fn dragging_a_rectangle_previews_live_and_is_one_undo_step() {
        let mut e = editor();
        e.shape = Shape::Rectangle;
        e.fill = Fill::Solid;
        at(&mut e, (0, 0));
        e.begin_shape(false);
        assert!(e.shape_active());
        at(&mut e, (4, 2));
        assert_eq!(count(&e, 0, -2..=8, -2..=8), 15, "5 x 3 live preview");
        at(&mut e, (1, 1)); // shrink: the old preview is rewound
        assert_eq!(count(&e, 0, -2..=8, -2..=8), 4);
        at(&mut e, (6, 3));
        e.end_shape();
        assert!(!e.shape_active());
        assert_eq!(count(&e, 0, -2..=9, -2..=9), 28);
        assert!(e.modified());

        e.undo();
        assert_eq!(e.world.total_chunks(), 0, "one undo removes the whole shape");
        e.redo();
        assert_eq!(count(&e, 0, -2..=9, -2..=9), 28);
    }

    #[test]
    fn shape_uses_texture_colour_and_current_depth() {
        let mut e = editor();
        e.set_texture(7);
        e.set_brush_rgb([1, 2, 3]);
        for _ in 0..8 {
            e.zoom_centre(1.5);
        }
        let d = e.camera.depth;
        assert!(d >= 1);
        let cell = e.camera.screen_to_cell(500.0, 400.0, 1000.0, 800.0);
        e.fill = Fill::Solid;
        at(&mut e, cell);
        e.begin_shape(false);
        at(&mut e, (cell.0 + 2, cell.1 + 1));
        e.end_shape();
        let m = e.world.get(d, cell.0 + 1, cell.1);
        assert_eq!(cell_texture(m), 7);
        assert_eq!(e.world.colors.get(cell_color(m)), [1, 2, 3]);
        assert!(e.world.layer_is_empty(0));
    }

    #[test]
    fn cancelling_a_shape_restores_the_map_and_the_saved_flag() {
        let mut e = editor();
        e.mouse_moved(500.0, 400.0);
        e.begin_paint();
        e.end_stroke();
        e.save_to(&std::env::temp_dir().join(format!("depthmap-cancel-{}.dmap", std::process::id())));
        assert!(!e.modified());
        let before: Vec<_> = (-5..10).map(|x| e.world.get(0, x, 0)).collect();
        e.fill = Fill::Solid;
        e.shape = Shape::Ellipse;
        at(&mut e, (-3, -3));
        e.begin_shape(false);
        at(&mut e, (8, 6));
        assert!(count(&e, 0, -5..=10, -5..=10) > 5);
        e.cancel_shape();
        assert!(!e.shape_active() && !e.modified());
        let after: Vec<_> = (-5..10).map(|x| e.world.get(0, x, 0)).collect();
        assert_eq!(before, after);
        assert!(!e.history.can_redo());
        let _ = std::fs::remove_file(std::env::temp_dir().join(format!("depthmap-cancel-{}.dmap", std::process::id())));
    }

    #[test]
    fn shapes_overwrite_and_erase_shapes_restore_on_undo() {
        let mut e = editor();
        e.fill = Fill::Solid;
        at(&mut e, (0, 0));
        e.begin_shape(false);
        at(&mut e, (5, 5));
        e.end_shape();
        assert_eq!(count(&e, 0, -1..=6, -1..=6), 36);
        // Shift (erase=true) removes the inside of a smaller rectangle.
        at(&mut e, (1, 1));
        e.begin_shape(true);
        at(&mut e, (4, 4));
        e.end_shape();
        assert_eq!(count(&e, 0, -1..=6, -1..=6), 36 - 16);
        e.undo();
        assert_eq!(count(&e, 0, -1..=6, -1..=6), 36);
        // The eraser toggle does the same without Shift, for brush and shape.
        e.eraser = true;
        at(&mut e, (0, 0));
        e.begin_shape(false);
        at(&mut e, (5, 5));
        e.end_shape();
        assert_eq!(e.world.total_chunks(), 0);
        e.undo();
        at(&mut e, (2, 2));
        e.begin_paint();
        e.end_stroke();
        assert_eq!(e.world.get(0, 2, 2), EMPTY);
    }

    #[test]
    fn lines_and_patterns_and_borders_through_the_editor() {
        let mut e = editor();
        e.shape = Shape::Line;
        e.fill = Fill::Outline;
        at(&mut e, (0, 0));
        e.begin_shape(false);
        at(&mut e, (9, 0));
        e.end_shape();
        assert_eq!(count(&e, 0, -1..=10, -1..=1), 10);

        e.shape = Shape::Rectangle;
        e.fill = Fill::Dither50;
        e.border = true;
        at(&mut e, (0, -11));
        e.begin_shape(false);
        at(&mut e, (9, -2));
        e.end_shape();
        let n = count(&e, 0, 0..=9, -11..=-2);
        // Solid border (36 cells) plus half of the 8 x 8 interior.
        assert_eq!(n, 36 + 32);
    }

    #[test]
    fn zooming_during_a_drag_moves_the_shape_to_the_new_depth() {
        let mut e = editor();
        e.fill = Fill::Solid;
        at(&mut e, (0, 0));
        e.begin_shape(false);
        at(&mut e, (3, 3));
        assert!(!e.world.layer_is_empty(0));
        for _ in 0..8 {
            e.zoom(1.5); // wheel zoom around the mouse, crossing into depth 1
        }
        assert!(e.camera.depth >= 1);
        e.end_shape();
        // The preview at depth 0 was rewound; the shape now lives at the new depth.
        assert!(e.world.layer_is_empty(0));
        assert!(!e.world.layer_is_empty(e.camera.depth));
        e.undo();
        assert_eq!(e.world.total_chunks(), 0);
    }

    #[test]
    fn a_shape_cannot_start_during_a_stroke_and_strokes_wait_for_shapes() {
        let mut e = editor();
        e.mouse_moved(400.0, 400.0);
        e.begin_paint();
        e.begin_shape(false);
        assert!(!e.shape_active());
        e.end_stroke();
        e.begin_shape(false);
        assert!(e.shape_active());
        e.begin_paint(); // ignored while the shape is dragged
        e.mouse_moved(600.0, 500.0);
        e.end_stroke();
        e.end_shape();
        e.undo();
        e.undo();
        assert_eq!(e.world.total_chunks(), 0);
    }

    #[test]
    fn convert_cell_round_trips_through_a_finer_depth() {
        assert_eq!(convert_cell((3, -2), 1, 2), (3 * 4 + 2, -2 * 4 + 2));
        let fine = convert_cell((3, -2), 1, 3);
        assert_eq!(convert_cell(fine, 3, 1), (3, -2));
        assert_eq!(convert_cell((-1, 5), 2, 2), (-1 * 1 + 0, 5));
    }

    #[test]
    fn hover_info_names_the_cell_and_its_parent() {
        let mut e = bounded(16, 16, 3);
        assert_eq!(e.hover_info(), None);
        e.set_zoom_level(0.0);
        e.mouse_moved(500.0, 400.0);
        let h = e.hover_info().unwrap();
        assert_eq!((h.depth, h.parent, h.inside), (0, None, true));
        assert_eq!(h.text(), format!("d0 cell {},{}", h.cell.0, h.cell.1));

        e.set_zoom_level(4.0); // depth 2
        e.mouse_moved(500.0, 400.0);
        let h = e.hover_info().unwrap();
        assert_eq!(h.depth, 2);
        let p = h.parent.unwrap();
        assert_eq!(p, (h.cell.0.div_euclid(4), h.cell.1.div_euclid(4)));
        assert!(h.text().contains("in d1 cell"));
        // The parent really contains the cell: it is the cell under the mouse one depth up.
        let up = e.camera.at_depth(1);
        let (vw, vh) = e.viewport;
        let px = up.frac.0 + (500.0 - vw * 0.5) / up.ppc;
        let py = up.frac.1 + (400.0 - vh * 0.5) / up.ppc;
        assert_eq!(p, (up.cell.0 + px.floor() as i64, up.cell.1 + py.floor() as i64));

        e.mouse_moved(1.0, 1.0);
        e.set_zoom_level(0.0);
        e.mouse_moved(0.0, 0.0);
        assert!(!e.hover_info().unwrap().inside, "off the map");
    }
}
