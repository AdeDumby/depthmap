//! UI behaviour (clicks on real widgets, no GPU needed) plus an offscreen
//! render of the whole interface. Set `DEPTHMAP_PNG_DIR` to dump screenshots.

use depthmap::editor::Editor;
use depthmap::gui::{Command, Gui, UiState, draw, setup_style};
use depthmap_core::colors::DEFAULT_COLORS;
use depthmap_core::texture::TEXTURE_COUNT;
use depthmap_core::{EMPTY, World, make_cell};
use egui::{Event, PointerButton, Pos2, RawInput, Rect, vec2};
use std::path::PathBuf;

const W: f32 = 1280.0;
const H: f32 = 800.0;

struct Harness {
    ctx: egui::Context,
    ed: Editor,
    st: UiState,
    time: f64,
}

impl Harness {
    fn new() -> Self {
        let ctx = egui::Context::default();
        setup_style(&ctx);
        let mut ed = Editor::new();
        ed.resize(W as f64, H as f64);
        let mut h = Self { ctx, ed, st: UiState::new(), time: 0.0 };
        h.frame(vec![]);
        h.frame(vec![]); // first frames size the windows
        h
    }

    fn frame(&mut self, events: Vec<Event>) -> egui::FullOutput {
        self.time += 0.1;
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(W, H))),
            time: Some(self.time),
            events,
            ..Default::default()
        };
        let (ed, st) = (&mut self.ed, &mut self.st);
        let mut out = self.ctx.run_ui(input, |ui| draw(ui, ed, st));
        // No GPU here: egui insists texture updates are consumed, so drop them explicitly.
        out.textures_delta.clear();
        out
    }

    fn click(&mut self, pos: Pos2) {
        let button = |pressed| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        self.frame(vec![Event::PointerMoved(pos)]);
        self.frame(vec![button(true)]);
        self.frame(vec![button(false)]);
        self.frame(vec![]);
    }

    fn hover(&mut self, pos: Pos2) {
        self.frame(vec![Event::PointerMoved(pos)]);
        self.frame(vec![]);
    }
}

#[test]
fn clicking_a_texture_selects_it() {
    let mut h = Harness::new();
    assert_eq!(h.st.layout.textures.len(), TEXTURE_COUNT as usize);
    for id in [1u8, 4, 9, TEXTURE_COUNT] {
        let rect = h.st.layout.textures[id as usize - 1];
        h.click(rect.center());
        assert_eq!(h.ed.texture, id);
    }
}

#[test]
fn clicking_a_swatch_sets_the_brush_colour() {
    let mut h = Harness::new();
    let rect = h.st.layout.swatches[5];
    h.click(rect.center());
    assert_eq!(h.ed.brush_rgb, DEFAULT_COLORS[5]);
    let rect = h.st.layout.swatches[10];
    h.click(rect.center());
    assert_eq!(h.ed.brush_rgb, DEFAULT_COLORS[10]);
    // Choosing colours alone never grows the colour table.
    assert_eq!(h.ed.world.colors.len(), DEFAULT_COLORS.len());
}

#[test]
fn zoom_buttons_change_the_zoom() {
    let mut h = Harness::new();
    let l0 = h.ed.zoom_level();
    h.click(h.st.layout.zoom_in.unwrap().center());
    assert!((h.ed.zoom_level() - (l0 + 1.0)).abs() < 1e-9);
    h.click(h.st.layout.zoom_in.unwrap().center());
    h.click(h.st.layout.zoom_in.unwrap().center());
    assert!(h.ed.camera.depth >= 1, "three doublings cross into depth 1");
    h.click(h.st.layout.zoom_out.unwrap().center());
    assert!((h.ed.zoom_level() - (l0 + 2.0)).abs() < 1e-9);
    h.click(h.st.layout.zoom_reset.unwrap().center());
    assert!(h.ed.zoom_level().abs() < 1e-9);
    assert_eq!(h.ed.camera.depth, 0);
}

#[test]
fn panels_capture_the_pointer_but_the_canvas_does_not() {
    let mut h = Harness::new();
    let thumb = h.st.layout.textures[0].center();
    h.hover(thumb);
    assert!(h.ctx.is_pointer_over_egui(), "over the texture panel");
    h.hover(h.st.layout.zoom_in.unwrap().center());
    assert!(h.ctx.is_pointer_over_egui(), "over the zoom panel");
    h.hover(Pos2::new(5.0, 5.0));
    assert!(h.ctx.is_pointer_over_egui(), "over the menu bar");
    h.hover(Pos2::new(W * 0.5, H * 0.6));
    assert!(!h.ctx.is_pointer_over_egui(), "canvas must stay paintable");
}

#[test]
fn panels_can_be_hidden() {
    let mut h = Harness::new();
    h.st.show_tools = false;
    h.st.show_zoom = false;
    h.frame(vec![]);
    h.hover(Pos2::new(40.0, 100.0));
    assert!(!h.ctx.is_pointer_over_egui());
    h.hover(Pos2::new(W - 40.0, 100.0));
    assert!(!h.ctx.is_pointer_over_egui());
}

#[test]
fn file_menu_new_asks_before_discarding_work() {
    let mut h = Harness::new();
    // Draw something so the map has unsaved changes.
    h.ed.mouse_moved(600.0, 500.0);
    h.ed.begin_paint();
    h.ed.end_stroke();
    assert!(h.ed.modified());

    h.st.request(Command::New, h.ed.modified());
    assert!(h.st.confirm_open());
    assert!(h.st.take_commands().is_empty());
    h.frame(vec![]); // the dialog is drawn and blocks painting
    h.hover(Pos2::new(W * 0.5, H * 0.5));
    assert!(h.ctx.is_pointer_over_egui(), "dialog is under the pointer");
}

#[test]
fn undo_menu_entries_follow_the_history() {
    let mut h = Harness::new();
    assert!(!h.ed.history.can_undo());
    h.ed.mouse_moved(600.0, 500.0);
    h.ed.begin_paint();
    h.ed.end_stroke();
    assert!(h.ed.history.can_undo());
    h.ed.undo();
    assert!(h.ed.world.get(0, 0, 0) == EMPTY && h.ed.history.can_redo());
    let _ = World::new();
    let _ = make_cell(1, 0);
}

fn temp_maps(tag: &str, names: &[&str], create: &[bool]) -> (PathBuf, Vec<PathBuf>) {
    let dir = std::env::temp_dir().join(format!("depthmap-ui-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let paths: Vec<PathBuf> = names.iter().map(|n| dir.join(n)).collect();
    for (p, &make) in paths.iter().zip(create) {
        if make {
            std::fs::write(p, b"map").unwrap();
        }
    }
    (dir, paths)
}

fn enter_key() -> Event {
    Event::Key {
        key: egui::Key::Enter,
        physical_key: Some(egui::Key::Enter),
        pressed: true,
        repeat: false,
        modifiers: Default::default(),
    }
}

#[test]
fn startup_offers_the_last_map_and_clicking_it_opens_it() {
    let (dir, maps) = temp_maps("last", &["tavern.dmap", "keep.dmap"], &[true, true]);
    let mut h = Harness::new();
    h.st.set_recent(&maps);
    h.st.open_startup();
    h.frame(vec![]);
    h.frame(vec![]);
    assert!(h.st.layout.startup_continue.is_some());
    assert!(h.st.layout.startup_open.is_some() && h.st.layout.startup_new.is_some());
    assert_eq!(h.st.layout.startup_recent.len(), 2);

    h.click(h.st.layout.startup_continue.unwrap().center());
    assert_eq!(h.st.take_commands(), vec![Command::OpenPath(maps[0].clone())]);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn enter_continues_with_the_last_map() {
    let (dir, maps) = temp_maps("enter", &["a.dmap"], &[true]);
    let mut h = Harness::new();
    h.st.set_recent(&maps);
    h.st.open_startup();
    h.frame(vec![]);
    h.frame(vec![enter_key()]);
    assert_eq!(h.st.take_commands(), vec![Command::OpenPath(maps[0].clone())]);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn startup_skips_deleted_maps_and_never_opens_them() {
    // Most recent map was deleted: continue with the newest one that still exists.
    let (dir, maps) = temp_maps("gone", &["deleted.dmap", "real.dmap"], &[false, true]);
    let mut h = Harness::new();
    h.st.set_recent(&maps);
    h.st.open_startup();
    h.frame(vec![]);
    h.frame(vec![]);
    h.click(h.st.layout.startup_continue.unwrap().center());
    assert_eq!(h.st.take_commands(), vec![Command::OpenPath(maps[1].clone())]);

    // The deleted entry is listed but cannot be clicked.
    h.st.open_startup();
    h.frame(vec![]);
    h.frame(vec![]);
    h.click(h.st.layout.startup_recent[0].center());
    assert!(h.st.take_commands().is_empty());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn startup_without_history_offers_open_and_new_only() {
    let mut h = Harness::new();
    h.st.open_startup();
    h.frame(vec![]);
    h.frame(vec![]);
    assert!(h.st.layout.startup_continue.is_none());
    assert!(h.st.layout.startup_recent.is_empty());

    h.click(h.st.layout.startup_new.unwrap().center());
    assert!(h.st.take_commands().is_empty(), "New opens the size dialog first");
    assert!(h.st.new_map_open());
    h.st.close_new_map();
    h.frame(vec![]);
    h.frame(vec![]);
    h.click(h.st.layout.startup_open.unwrap().center());
    assert_eq!(h.st.take_commands(), vec![Command::Open]);
    // Enter does nothing when there is no map to continue with.
    h.frame(vec![enter_key()]);
    assert!(h.st.take_commands().is_empty());
}

#[test]
fn startup_stays_until_the_app_closes_it_and_blocks_the_canvas() {
    let mut h = Harness::new();
    h.st.open_startup();
    h.frame(vec![]);
    assert!(h.st.startup_open());
    // Choosing "Open..." and cancelling the dialog must leave the screen up.
    h.click(h.st.layout.startup_open.unwrap().center());
    assert!(h.st.startup_open());
    h.st.close_startup();
    h.frame(vec![]);
    assert!(!h.st.startup_open());
    assert!(h.st.layout.startup_new.is_none());
}

// ---------------------------------------------------------------------------
// Offscreen render of the whole interface.

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    target: wgpu::Texture,
}

const PW: u32 = 1280;
const PH: u32 = 800;

fn gpu() -> Option<Gpu> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default()))
        .or_else(|_| {
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                force_fallback_adapter: true,
                ..Default::default()
            }))
        })
        .ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_limits: wgpu::Limits::default().using_resolution(adapter.limits()),
        ..Default::default()
    }))
    .ok()?;
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("target"),
        size: wgpu::Extent3d { width: PW, height: PH, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    Some(Gpu { device, queue, target })
}

fn read_back(gpu: &Gpu) -> Vec<u8> {
    let bpr = PW * 4;
    let buf = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: (bpr * PH) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = gpu.device.create_command_encoder(&Default::default());
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &gpu.target,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bpr),
                rows_per_image: Some(PH),
            },
        },
        wgpu::Extent3d { width: PW, height: PH, depth_or_array_layers: 1 },
    );
    gpu.queue.submit([enc.finish()]);
    let slice = buf.slice(..);
    slice.map_async(wgpu::MapMode::Read, |r| r.unwrap());
    gpu.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    slice.get_mapped_range().unwrap().to_vec()
}

fn save_png(name: &str, data: &[u8]) {
    if let Ok(dir) = std::env::var("DEPTHMAP_PNG_DIR") {
        let path = std::path::Path::new(&dir).join(format!("{name}.png"));
        let mut enc = png::Encoder::new(std::fs::File::create(path).unwrap(), PW, PH);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().unwrap().write_image_data(data).unwrap();
    }
}

#[test]
fn full_window_renders_map_and_interface() {
    let Some(gpu) = gpu() else {
        eprintln!("SKIPPED: no GPU adapter available");
        return;
    };
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut renderer = depthmap::renderer::Renderer::new(gpu.device.clone(), gpu.queue.clone(), format);
    let mut gui = Gui::new(&gpu.device, format);

    // A small map to look at: a stone room with a wooden floor, some water and
    // grass, and detail drawn one depth deeper.
    let mut ed = Editor::new();
    ed.resize(PW as f64, PH as f64);
    let colors = [2usize, 3, 7, 5, 9];
    for y in -6..6 {
        for x in -9..9 {
            let wall = x == -9 || x == 8 || y == -6 || y == 5;
            let (tex, col) = if wall {
                (4, 1)
            } else if x > 2 && y > 0 {
                (9, 7)
            } else if x < -4 && y < -1 {
                (7, 5)
            } else {
                (5, 3)
            };
            let col = ed.world.colors.index_of(DEFAULT_COLORS[col]);
            ed.world.set(0, x, y, make_cell(tex, col));
        }
    }
    let _ = colors;
    let tint = ed.world.colors.index_of([200, 90, 160]);
    for t in 1..=TEXTURE_COUNT {
        ed.world.set(0, -8 + t as i64, -4, make_cell(t, tint));
    }
    ed.set_texture(5);
    ed.set_brush_rgb(DEFAULT_COLORS[3]);
    ed.brush_half = 1;

    let screen = Rect::from_min_size(Pos2::ZERO, vec2(PW as f32, PH as f32));
    let view = gpu.target.create_view(&Default::default());
    // The first frames only size the windows; every frame's texture updates
    // must still reach the GPU, so each one is painted over a fresh map.
    for i in 0..3 {
        let input = RawInput {
            screen_rect: Some(screen),
            time: Some(i as f64 * 0.1),
            ..Default::default()
        };
        let output = gui.run(input, &mut ed);
        renderer.render(
            &view,
            (PW, PH),
            &ed.world,
            &ed.camera,
            &depthmap::renderer::RenderOptions {
                cursor: Some((3, 2)),
                brush_half: 1,
                ..Default::default()
            },
        );
        gui.paint(&gpu.device, &gpu.queue, &view, [PW, PH], output);
    }
    let img = read_back(&gpu);
    save_png("ui_full", &img);

    let px = |x: u32, y: u32| {
        let i = ((y * PW + x) * 4) as usize;
        [img[i], img[i + 1], img[i + 2]]
    };
    let bg = [230u8, 222, 204];
    // Menu bar: a solid UI-coloured strip across the top, not parchment.
    assert_ne!(px(600, 6), bg);
    // The texture panel and zoom panel sit over the map's empty corners.
    let tex_panel = gui.state.layout.textures[0].center();
    assert_ne!(px(tex_panel.x as u32, tex_panel.y as u32), bg);
    let zoom_btn = gui.state.layout.zoom_in.unwrap().center();
    assert_ne!(px(zoom_btn.x as u32, zoom_btn.y as u32), bg);
    // The map itself is visible in the middle.
    assert_ne!(px(PW / 2 + 20, PH / 2 + 60), bg);
}

#[test]
fn startup_screen_renders() {
    let Some(gpu) = gpu() else {
        eprintln!("SKIPPED: no GPU adapter available");
        return;
    };
    let (dir, maps) = temp_maps(
        "shot",
        &["Tavern of the Wandering Bear.dmap", "Dragon keep, level 2.dmap", "old-test.dmap"],
        &[true, true, false],
    );
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut renderer = depthmap::renderer::Renderer::new(gpu.device.clone(), gpu.queue.clone(), format);
    let mut gui = Gui::new(&gpu.device, format);
    let mut ed = Editor::new();
    ed.resize(PW as f64, PH as f64);
    gui.state.set_recent(&maps);
    gui.state.open_startup();
    let view = gpu.target.create_view(&Default::default());
    for i in 0..3 {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(PW as f32, PH as f32))),
            time: Some(i as f64 * 0.1),
            ..Default::default()
        };
        let out = gui.run(input, &mut ed);
        renderer.render(&view, (PW, PH), &ed.world, &ed.camera, &Default::default());
        gui.paint(&gpu.device, &gpu.queue, &view, [PW, PH], out);
    }
    let img = read_back(&gpu);
    save_png("ui_startup", &img);
    let r = gui.state.layout.startup_continue.expect("continue button");
    let px = |x: f32, y: f32| {
        let i = (((y as u32) * PW + x as u32) * 4) as usize;
        [img[i], img[i + 1], img[i + 2]]
    };
    assert_ne!(px(r.min.x + 6.0, r.center().y), [230, 222, 204], "welcome window not drawn");
    std::fs::remove_dir_all(dir).unwrap();
}

// ---- New Map dialog ---------------------------------------------------------

#[test]
fn new_map_dialog_asks_for_size_and_depth_then_creates() {
    let mut h = Harness::new();
    h.st.open_startup();
    h.frame(vec![]);
    h.frame(vec![]);
    h.click(h.st.layout.startup_new.unwrap().center());
    assert!(h.st.new_map_open());
    h.frame(vec![]);
    h.frame(vec![]);
    assert_eq!(h.st.layout.new_presets.len(), 6);
    assert_eq!(h.st.new_map_form().unwrap().width, 128, "defaults offered");

    // Pick the 256 preset by clicking it.
    h.click(h.st.layout.new_presets[3].center());
    let form = h.st.new_map_form().unwrap();
    assert_eq!((form.width, form.height), (256, 256));

    // A custom, non-square size and depth (as typed or dragged in the fields).
    h.st.set_new_map_form(depthmap::gui::NewMapForm { width: 90, height: 40, depths: 5 });
    h.frame(vec![]);
    h.click(h.st.layout.new_create.unwrap().center());
    assert!(!h.st.new_map_open());
    assert_eq!(h.st.take_commands(), vec![Command::Create(depthmap_core::MapSpec::new(90, 40, 5))]);
}

#[test]
fn new_map_dialog_can_be_cancelled_by_button_or_escape() {
    let mut h = Harness::new();
    h.st.open_new_map();
    h.frame(vec![]);
    h.frame(vec![]);
    h.click(h.st.layout.new_cancel.unwrap().center());
    assert!(!h.st.new_map_open());
    assert!(h.st.take_commands().is_empty());

    h.st.open_new_map();
    h.frame(vec![]);
    h.frame(vec![Event::Key {
        key: egui::Key::Escape,
        physical_key: Some(egui::Key::Escape),
        pressed: true,
        repeat: false,
        modifiers: Default::default(),
    }]);
    assert!(!h.st.new_map_open());
    assert!(h.st.take_commands().is_empty());
}

#[test]
fn new_map_dialog_is_modal_and_does_not_let_enter_open_the_last_map() {
    let (dir, maps) = temp_maps("modal", &["a.dmap"], &[true]);
    let mut h = Harness::new();
    h.st.set_recent(&maps);
    h.st.open_startup();
    h.st.open_new_map();
    h.frame(vec![]);
    h.frame(vec![enter_key()]);
    assert!(h.st.take_commands().is_empty());
    assert!(h.st.new_map_open());
    h.hover(Pos2::new(W * 0.5, H * 0.5));
    assert!(h.ctx.is_pointer_over_egui());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn creating_applies_to_the_editor() {
    let mut h = Harness::new();
    h.ed.new_map(depthmap_core::MapSpec::new(50, 50, 2));
    assert_eq!(h.ed.spec().max_depth, 2);
    h.frame(vec![]);
    // The zoom panel reflects the map.
    assert!(h.ed.zoom_range().1 <= 5.0 + 1e-9);
}

// ---- shape controls -----------------------------------------------------------

#[test]
fn clicking_shape_and_fill_buttons_selects_them() {
    use depthmap_core::{Fill, Shape};
    let mut h = Harness::new();
    assert_eq!(h.st.layout.shapes.len(), Shape::ALL.len());
    assert_eq!(h.st.layout.fills.len(), Fill::ALL.len());
    for (i, shape) in Shape::ALL.iter().enumerate() {
        h.click(h.st.layout.shapes[i].center());
        assert_eq!(h.ed.shape, *shape);
    }
    for (i, fill) in Fill::ALL.iter().enumerate() {
        h.click(h.st.layout.fills[i].center());
        assert_eq!(h.ed.fill, *fill);
    }
}

#[test]
fn border_toggle_only_applies_to_filled_shapes_and_eraser_toggles() {
    use depthmap_core::{Fill, Shape};
    let mut h = Harness::new();
    h.ed.shape = Shape::Rectangle;
    h.ed.fill = Fill::Outline;
    h.frame(vec![]);
    h.click(h.st.layout.border_toggle.unwrap().center());
    assert!(!h.ed.border, "disabled for an outline");

    h.ed.fill = Fill::Dither50;
    h.frame(vec![]);
    h.click(h.st.layout.border_toggle.unwrap().center());
    assert!(h.ed.border);

    h.ed.shape = Shape::Line;
    h.ed.border = false;
    h.frame(vec![]);
    h.click(h.st.layout.border_toggle.unwrap().center());
    assert!(!h.ed.border, "lines have no fill");

    assert!(!h.ed.eraser);
    h.click(h.st.layout.eraser_toggle.unwrap().center());
    assert!(h.ed.eraser);
}

#[test]
fn every_tool_control_is_reachable_in_a_short_window() {
    // 1280 x 720 is a common laptop size: the panel must scroll, not run off screen.
    let mut h = Harness::new();
    h.ed.resize(1280.0, 720.0);
    for _ in 0..3 {
        h.frame(vec![]);
    }
    for r in [h.st.layout.eraser_toggle.unwrap(), h.st.layout.shapes[0]] {
        h.hover(r.center());
        assert!(h.ctx.is_pointer_over_egui());
    }
}

#[test]
fn new_map_dialog_renders_over_the_welcome_screen() {
    let Some(gpu) = gpu() else {
        eprintln!("SKIPPED: no GPU adapter available");
        return;
    };
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut renderer = depthmap::renderer::Renderer::new(gpu.device.clone(), gpu.queue.clone(), format);
    let mut gui = Gui::new(&gpu.device, format);
    let mut ed = Editor::with_spec(depthmap_core::MapSpec::new(64, 40, 3));
    ed.resize(PW as f64, PH as f64);
    gui.state.open_startup();
    gui.state.open_new_map();
    gui.state.set_new_map_form(depthmap::gui::NewMapForm { width: 256, height: 256, depths: 6 });
    let view = gpu.target.create_view(&Default::default());
    for i in 0..3 {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(PW as f32, PH as f32))),
            time: Some(i as f64 * 0.1),
            ..Default::default()
        };
        let out = gui.run(input, &mut ed);
        renderer.render(&view, (PW, PH), &ed.world, &ed.camera, &Default::default());
        gui.paint(&gpu.device, &gpu.queue, &view, [PW, PH], out);
    }
    let img = read_back(&gpu);
    save_png("ui_new_map", &img);
    let r = gui.state.layout.new_create.expect("create button");
    let i = (((r.center().y as u32) * PW + (r.min.x as u32 + 6)) * 4) as usize;
    assert_ne!([img[i], img[i + 1], img[i + 2]], [230, 222, 204]);
}
