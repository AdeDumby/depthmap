//! UI behaviour (clicks on real widgets, no GPU needed) plus an offscreen
//! render of the whole interface. Set `DEPTHMAP_PNG_DIR` to dump screenshots.

use depthmap::editor::Editor;
use depthmap::gui::{Command, Gui, UiState, draw, setup_style};
use depthmap_core::colors::DEFAULT_COLORS;
use depthmap_core::texture::TEXTURE_COUNT;
use depthmap_core::{EMPTY, World, make_cell};
use egui::{Event, PointerButton, Pos2, RawInput, Rect, vec2};

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
