//! Exporting the map as a PNG: the same renderer and overlay drawing as the
//! screen, pointed at an offscreen texture sized to the chosen area.

use crate::editor::{Editor, ToolMode};
use crate::gui::{Gui, draw_map_overlays};
use crate::renderer::{RenderOptions, Renderer};
use depthmap_core::SUBDIV;
use depthmap_core::camera::BASE_PPC;
use std::path::Path;

/// Longest side of an exported image.
pub const MAX_SIDE: u32 = 16_384;
/// Most pixels in an exported image (4 bytes each, plus a copy to read back).
pub const MAX_PIXELS: u64 = 64_000_000;
pub const MIN_PX_PER_CELL: u32 = 1;
pub const MAX_PX_PER_CELL: u32 = 256;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportArea {
    WholeMap,
    Selection,
    /// What is on screen now.
    View,
}

impl ExportArea {
    pub const ALL: [ExportArea; 3] = [ExportArea::WholeMap, ExportArea::Selection, ExportArea::View];

    pub fn name(self) -> &'static str {
        match self {
            ExportArea::WholeMap => "Whole map",
            ExportArea::Selection => "Selection",
            ExportArea::View => "Visible view",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExportOptions {
    pub area: ExportArea,
    /// The layer to draw (cells of this depth are `px_per_cell` pixels wide).
    pub depth: u8,
    pub px_per_cell: u32,
    pub grid: bool,
    pub regions: bool,
    pub routes: bool,
    pub nodes: bool,
    /// Show the faded layer above, as on screen.
    pub parent_layer: bool,
}

impl ExportOptions {
    pub fn new(depth: u8) -> Self {
        Self {
            area: ExportArea::WholeMap,
            depth,
            px_per_cell: 32,
            grid: false,
            regions: true,
            routes: true,
            nodes: true,
            parent_layer: false,
        }
    }
}

/// What an export would produce.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plan {
    /// Top-left cell and size in cells, all at the export depth.
    pub origin: (i64, i64),
    pub cells: (i64, i64),
    pub size_px: (u32, u32),
}

impl Plan {
    pub fn megabytes(&self) -> f64 {
        self.size_px.0 as f64 * self.size_px.1 as f64 * 4.0 / 1_048_576.0
    }
}

/// Converts an inclusive cell box between depths (growing or shrinking).
fn rebase(((x0, y0), (x1, y1)): ((i64, i64), (i64, i64)), from: u8, to: u8) -> ((i64, i64), (i64, i64)) {
    if to >= from {
        let s = SUBDIV.pow((to - from) as u32);
        ((x0 * s, y0 * s), ((x1 + 1) * s - 1, (y1 + 1) * s - 1))
    } else {
        let s = SUBDIV.pow((from - to) as u32);
        ((x0.div_euclid(s), y0.div_euclid(s)), (x1.div_euclid(s), y1.div_euclid(s)))
    }
}

/// Works out the exported rectangle and image size, or says why not.
pub fn plan(ed: &Editor, opts: &ExportOptions) -> Result<Plan, String> {
    let spec = ed.world.spec();
    let d = opts.depth;
    if d > spec.max_depth {
        return Err(format!("This map only goes down to depth {}", spec.max_depth));
    }
    if !(MIN_PX_PER_CELL..=MAX_PX_PER_CELL).contains(&opts.px_per_cell) {
        return Err(format!("Pixels per cell must be {MIN_PX_PER_CELL} to {MAX_PX_PER_CELL}"));
    }
    let extent = spec.extent(d);
    let ((x0, y0), (x1, y1)) = match opts.area {
        ExportArea::WholeMap => {
            let (w, h) = extent.ok_or("This map has no fixed size: export the selection or the visible view")?;
            ((0, 0), (w - 1, h - 1))
        }
        ExportArea::Selection => {
            let sel = ed.selection().ok_or("Select something first (Select tool)")?;
            let b = sel.bounds().ok_or("An \"everything but...\" selection cannot be exported")?;
            rebase(b, sel.depth(), d)
        }
        ExportArea::View => {
            let (min, max) = ed.camera.at_depth(d).visible_cells(ed.viewport.0, ed.viewport.1);
            (min, max)
        }
    };
    // Keep to the map.
    let (x0, y0, x1, y1) = match extent {
        Some((w, h)) => (x0.max(0), y0.max(0), x1.min(w - 1), y1.min(h - 1)),
        None => (x0, y0, x1, y1),
    };
    if x1 < x0 || y1 < y0 {
        return Err("That area is outside the map".into());
    }
    let (cw, ch) = (x1 - x0 + 1, y1 - y0 + 1);
    let (pw, ph) = (cw.saturating_mul(opts.px_per_cell as i64), ch.saturating_mul(opts.px_per_cell as i64));
    if pw > MAX_SIDE as i64 || ph > MAX_SIDE as i64 || (pw as u64) * (ph as u64) > MAX_PIXELS {
        return Err(format!(
            "{pw} \u{d7} {ph} px is too large (at most {MAX_SIDE} px a side and {} million pixels): use fewer pixels per cell, a shallower depth or a smaller area",
            MAX_PIXELS / 1_000_000
        ));
    }
    Ok(Plan { origin: (x0, y0), cells: (cw, ch), size_px: (pw as u32, ph as u32) })
}

/// Renders the export to RGBA pixels (`width * height * 4` bytes).
pub fn render(
    ed: &mut Editor,
    opts: &ExportOptions,
    ghost_opacity: f32,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> Result<(u32, u32, Vec<u8>), String> {
    let plan = plan(ed, opts)?;
    let (w, h) = plan.size_px;
    let max_dim = device.limits().max_texture_dimension_2d;
    if w > max_dim || h > max_dim {
        return Err(format!(
            "This graphics card handles images up to {max_dim} px a side; {w} \u{d7} {h} is too large"
        ));
    }

    // Point the editor's camera at the area, draw, then put everything back.
    let saved = (ed.camera, ed.viewport, ed.cursor_px, ed.tool, ed.show_nodes, ed.placing_node);
    let mut cam = ed.camera;
    cam.depth = opts.depth;
    cam.ppc = opts.px_per_cell as f64;
    cam.cell = (plan.origin.0 + plan.cells.0 / 2, plan.origin.1 + plan.cells.1 / 2);
    cam.frac = (0.5 * (plan.cells.0 % 2) as f64, 0.5 * (plan.cells.1 % 2) as f64);
    ed.camera = cam;
    ed.viewport = (w as f64, h as f64);
    ed.cursor_px = None;
    ed.tool = ToolMode::Paint;
    ed.show_nodes = opts.nodes;
    ed.placing_node = false;
    let result = render_inner(ed, opts, ghost_opacity, device, queue, (w, h));
    (ed.camera, ed.viewport, ed.cursor_px, ed.tool, ed.show_nodes, ed.placing_node) = saved;
    result.map(|data| (w, h, data))
}

fn render_inner(
    ed: &mut Editor,
    opts: &ExportOptions,
    ghost_opacity: f32,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    (w, h): (u32, u32),
) -> Result<Vec<u8>, String> {
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("export"),
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let mut renderer = Renderer::new(device.clone(), queue.clone(), FORMAT);
    renderer.render(
        &view,
        (w, h),
        &ed.world,
        &ed.camera,
        &RenderOptions {
            cursor: None,
            brush_half: 0,
            show_grid: opts.grid,
            checker: false,
            ghost_opacity: if opts.parent_layer { ghost_opacity } else { 0.0 },
        },
    );

    // Regions, routes and nodes are egui drawings. They scale with the cell
    // size, so a 64 px cell looks like the screen zoomed in twice.
    if opts.regions || opts.routes || opts.nodes {
        let mut gui = Gui::new(device, FORMAT);
        gui.state.show_zones = opts.regions;
        gui.state.show_route_lines = opts.routes;
        let ppp = (opts.px_per_cell as f32 / BASE_PPC as f32).clamp(0.5, 4.0);
        gui.ctx.set_pixels_per_point(ppp);
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(w as f32 / ppp, h as f32 / ppp),
            )),
            ..Default::default()
        };
        // The first frame only prepares fonts and icons; its shapes are dropped.
        let mut first = gui.ctx.run_ui(input(), |ui| draw_map_overlays(&ui.ctx().clone(), ed, &mut gui.state));
        first.shapes.clear();
        gui.paint(device, queue, &view, [w, h], first);
        let second = gui.ctx.run_ui(input(), |ui| draw_map_overlays(&ui.ctx().clone(), ed, &mut gui.state));
        gui.paint(device, queue, &view, [w, h], second);
    }

    // Read the pixels back (rows are padded to 256 bytes).
    let row = w * 4;
    let padded = row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("export readback"),
        size: padded as u64 * h as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&Default::default());
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &target,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    queue.submit([enc.finish()]);
    let slice = buf.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| format!("graphics error: {e}"))?;
    rx.recv().map_err(|e| e.to_string())?.map_err(|e| format!("could not read the image back: {e}"))?;
    let mapped = slice.get_mapped_range().map_err(|e| e.to_string())?;
    let mut out = Vec::with_capacity(row as usize * h as usize);
    for y in 0..h as usize {
        let start = y * padded as usize;
        out.extend_from_slice(&mapped[start..start + row as usize]);
    }
    Ok(out)
}

/// Encodes RGBA pixels as a PNG file.
pub fn write_png(path: &Path, w: u32, h: u32, rgba: &[u8]) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()
        .and_then(|mut wr| wr.write_image_data(rgba))
        .map_err(|e| e.to_string())
}

/// Renders and saves in one go; the message says what was written.
pub fn export_png(
    ed: &mut Editor,
    opts: &ExportOptions,
    ghost_opacity: f32,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    path: &Path,
) -> Result<String, String> {
    let (w, h, rgba) = render(ed, opts, ghost_opacity, device, queue)?;
    write_png(path, w, h, &rgba)?;
    Ok(format!("Exported {w} \u{d7} {h} px to {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use depthmap_core::selection::Selection;
    use depthmap_core::MapSpec;

    fn editor(w: u32, h: u32, depth: u8) -> Editor {
        let mut e = Editor::with_spec(MapSpec::new(w, h, depth));
        e.resize(800.0, 600.0);
        e
    }

    #[test]
    fn whole_map_size_follows_depth_and_pixels_per_cell() {
        let e = editor(64, 32, 2);
        let mut o = ExportOptions::new(0);
        o.px_per_cell = 16;
        let p = plan(&e, &o).unwrap();
        assert_eq!((p.cells, p.size_px, p.origin), ((64, 32), (1024, 512), (0, 0)));
        o.depth = 1;
        assert_eq!(plan(&e, &o).unwrap().size_px, (4096, 2048), "depth 1 has 4x the cells per side");
        o.depth = 3;
        assert!(plan(&e, &o).unwrap_err().contains("depth 2"));
    }

    #[test]
    fn too_large_exports_are_refused_with_advice() {
        let e = editor(100, 100, 2);
        let mut o = ExportOptions::new(2);
        o.px_per_cell = 16;
        let err = plan(&e, &o).unwrap_err();
        assert!(err.contains("too large"), "{err}");
        o.depth = 0;
        o.px_per_cell = 64;
        assert!(plan(&e, &o).is_ok());
        o.px_per_cell = 0;
        assert!(plan(&e, &o).is_err());
    }

    #[test]
    fn selection_area_is_its_bounding_box_at_the_export_depth() {
        let mut e = editor(64, 64, 2);
        let mut o = ExportOptions::new(0);
        o.area = ExportArea::Selection;
        assert!(plan(&e, &o).unwrap_err().contains("Select"));
        e.set_selection(Some(Selection::rect(1, (8, 12), (23, 15))));
        o.depth = 1;
        let p = plan(&e, &o).unwrap();
        assert_eq!((p.origin, p.cells), ((8, 12), (16, 4)));
        o.depth = 0; // shrinks to the parent cells that contain it
        let p = plan(&e, &o).unwrap();
        assert_eq!((p.origin, p.cells), ((2, 3), (4, 1)));
        o.depth = 2; // grows to the child cells
        let p = plan(&e, &o).unwrap();
        assert_eq!((p.origin, p.cells), ((32, 48), (64, 16)));
    }

    #[test]
    fn view_area_is_clipped_to_the_map() {
        let mut e = editor(8, 8, 1);
        let mut o = ExportOptions::new(0);
        o.area = ExportArea::View;
        e.camera.ppc = 32.0; // 800x600 px = 25 x 19 cells, more than the 8x8 map
        let p = plan(&e, &o).unwrap();
        assert_eq!(p.cells, (8, 8), "never bigger than the map");
    }

    #[test]
    fn rebase_between_depths() {
        assert_eq!(rebase(((1, 1), (2, 2)), 0, 1), ((4, 4), (11, 11)));
        assert_eq!(rebase(((5, 6), (9, 9)), 1, 0), ((1, 1), (2, 2)));
        assert_eq!(rebase(((-1, 0), (0, 0)), 1, 0), ((-1, 0), (0, 0)));
    }
}
