use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use depthmap::editor::Editor;
use depthmap::gui::{Command, Gui};
use depthmap::renderer::{FrameStats, RenderOptions, Renderer};
use depthmap::settings::Recent;
use depthmap_core::texture::TEXTURE_COUNT;
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
use winit::window::{Window, WindowId};

const HELP: &str = "\
depthmap [FILE] [--stress N]

  LMB paint            RMB drag shape       MMB / Space+LMB pan
  Shift+mouse erase    E eraser toggle      Esc cancel shape / node
  N add node, click map   click node edit, drag moves   Del delete open node
  wheel zoom           WASD / arrows pan    1-9, 0 texture
  [ ] brush size       Ctrl+Z / Ctrl+Y undo / redo
  Ctrl+N/O/S new / open / save   Ctrl+Shift+S save as   F1 help
";

struct Gfx {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: Renderer,
    gui: Gui,
    egui_state: egui_winit::State,
}

impl Gfx {
    fn new(window: Arc<Window>) -> Self {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle_from_env(
            Box::new(window.clone()),
        ));
        let surface = instance.create_surface(window.clone()).expect("create surface");
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .expect("no suitable GPU adapter");
        log::info!("adapter: {:?}", adapter.get_info());
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("depthmap"),
            required_limits: wgpu::Limits::default().using_resolution(adapter.limits()),
            ..Default::default()
        }))
        .expect("request device");

        let caps = surface.get_capabilities(&adapter);
        // A non-sRGB format keeps colours exactly as authored.
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
            .unwrap_or(caps.formats[0]);
        let size = window.inner_size();
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::AutoVsync,
            desired_maximum_frame_latency: 2,
            alpha_mode: caps.alpha_modes[0],
            color_space: wgpu::SurfaceColorSpace::Auto,
            view_formats: vec![],
        };
        surface.configure(&device, &config);

        let renderer = Renderer::new(device.clone(), queue.clone(), format);
        let gui = Gui::new(&device, format);
        let egui_state = egui_winit::State::new(
            gui.ctx.clone(),
            egui::ViewportId::ROOT,
            &*window,
            Some(window.scale_factor() as f32),
            window.theme(),
            Some(device.limits().max_texture_dimension_2d as usize),
        );
        Self { window, surface, config, device, queue, renderer, gui, egui_state }
    }

    fn resize(&mut self, size: PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 {
            return;
        }
        self.config.width = size.width;
        self.config.height = size.height;
        self.surface.configure(&self.device, &self.config);
    }
}

#[derive(Default)]
struct Held {
    left: bool,
    right: bool,
    up: bool,
    down: bool,
}

impl Held {
    fn any(&self) -> bool {
        self.left || self.right || self.up || self.down
    }
}

struct App {
    gfx: Option<Gfx>,
    editor: Editor,
    mods: ModifiersState,
    held: Held,
    space: bool,
    last_tick: Instant,
    last_title: String,
    repaint_at: Option<Instant>,
    stats: FrameStats,
    /// Maps opened or saved recently (stored in the user's config folder).
    recent: Recent,
    /// Show the welcome screen once the window exists.
    show_startup: bool,
}

impl App {
    fn new(editor: Editor, recent: Recent, show_startup: bool) -> Self {
        Self {
            gfx: None,
            editor,
            mods: ModifiersState::empty(),
            held: Held::default(),
            space: false,
            last_tick: Instant::now(),
            last_title: String::new(),
            repaint_at: None,
            stats: FrameStats::default(),
            recent,
            show_startup,
        }
    }

    /// Folder the file dialogs should start in: next to the open map,
    /// otherwise next to the last map used.
    fn dialog_dir(&self) -> Option<PathBuf> {
        self.editor
            .path
            .as_deref()
            .and_then(|p| p.parent())
            .filter(|d| d.is_dir())
            .map(PathBuf::from)
            .or_else(|| self.recent.dialog_dir())
    }

    /// Records the map that was just opened or saved in the recent list.
    fn sync_recent(&mut self) {
        let Some(path) = self.editor.take_touched_path() else { return };
        self.recent.push(&path);
        if let Some(g) = self.gfx.as_mut() {
            g.gui.state.set_recent(self.recent.paths());
        }
    }

    fn close_startup(&mut self) {
        if let Some(g) = self.gfx.as_mut() {
            g.gui.state.close_startup();
        }
    }

    fn request_redraw(&self) {
        if let Some(g) = &self.gfx {
            g.window.request_redraw();
        }
    }

    fn request(&mut self, cmd: Command) {
        let modified = self.editor.modified();
        if let Some(g) = self.gfx.as_mut() {
            g.gui.state.request(cmd, modified);
        }
    }

    fn save(&mut self) {
        if !self.editor.save() {
            self.request(Command::SaveAs);
        }
    }

    fn run_command(&mut self, cmd: Command, event_loop: &ActiveEventLoop) {
        match cmd {
            // The UI handles `New` itself (it opens the New Map dialog).
            Command::New => {}
            Command::Create(spec) => {
                self.editor.new_map(spec);
                self.close_startup();
                if let Some(g) = self.gfx.as_mut() {
                    g.gui.state.close_new_map();
                }
            }
            Command::Open => {
                let mut dialog = rfd::FileDialog::new()
                    .set_title("Open map")
                    .add_filter("depthmap", &["dmap"]);
                if let Some(dir) = self.dialog_dir() {
                    dialog = dialog.set_directory(dir);
                }
                if let Some(path) = dialog.pick_file() {
                    self.editor.load_from(&path);
                    self.close_startup();
                }
            }
            Command::OpenPath(path) => {
                if path.is_file() {
                    self.editor.load_from(&path);
                    self.close_startup();
                } else {
                    self.editor.status = format!("{} no longer exists", path.display());
                    self.recent.remove(&path);
                    if let Some(g) = self.gfx.as_mut() {
                        g.gui.state.set_recent(self.recent.paths());
                    }
                }
            }
            Command::SaveAs => {
                let mut dialog = rfd::FileDialog::new()
                    .set_title("Save map as")
                    .add_filter("depthmap", &["dmap"])
                    .set_file_name(self.editor.file_name_for_dialog());
                if let Some(dir) = self.dialog_dir() {
                    dialog = dialog.set_directory(dir);
                }
                if let Some(mut path) = dialog.save_file() {
                    if path.extension().is_none() {
                        path.set_extension("dmap");
                    }
                    self.editor.save_to(&path);
                }
            }
            Command::ImportIcons => {
                let mut dialog = rfd::FileDialog::new()
                    .set_title("Import PNG icons")
                    .add_filter("PNG image", &["png"]);
                if let Some(dir) = depthmap::settings::icon_import_start_dir() {
                    dialog = dialog.set_directory(dir);
                }
                if let Some(paths) = dialog.pick_files() {
                    self.editor.import_icons(&paths);
                }
            }
            Command::Quit => event_loop.exit(),
        }
    }

    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        let Some(gfx) = self.gfx.as_mut() else { return };

        // Keyboard panning, frame-rate independent.
        let now = Instant::now();
        let dt = now.duration_since(self.last_tick).as_secs_f64().min(0.1);
        self.last_tick = now;
        if self.held.any() {
            let speed = 700.0 * dt;
            let dx = (self.held.right as i32 - self.held.left as i32) as f64 * speed;
            let dy = (self.held.down as i32 - self.held.up as i32) as f64 * speed;
            // Panning moves the view, so the content slides the other way.
            self.editor.pan_keys(-dx, -dy);
        }

        // Get the image to draw into first: egui hands out texture uploads
        // exactly once, so a frame must never be skipped after the UI ran.
        let frame = match gfx.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                gfx.surface.configure(&gfx.device, &gfx.config);
                gfx.window.request_redraw();
                return;
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => return,
            wgpu::CurrentSurfaceTexture::Validation => {
                log::error!("surface validation error");
                event_loop.exit();
                return;
            }
        };
        // The UI runs before the map is drawn: it may change the editor
        // (zoom buttons, texture choice...).
        let raw = gfx.egui_state.take_egui_input(&gfx.window);
        let mut output = gfx.gui.run(raw, &mut self.editor);
        gfx.egui_state
            .handle_platform_output(&gfx.window, std::mem::take(&mut output.platform_output));
        let delay = output
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .map_or(Duration::MAX, |v| v.repaint_delay);
        self.repaint_at = (delay < Duration::from_secs(5)).then(|| Instant::now() + delay);

        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let size = (gfx.config.width, gfx.config.height);
        let over_ui = gfx.gui.pointer_over_ui();
        self.stats = gfx.renderer.render(
            &view,
            size,
            &self.editor.world,
            &self.editor.camera,
            &RenderOptions {
                cursor: if over_ui { None } else { self.editor.cursor_cell() },
                brush_half: self.editor.brush_half,
                show_grid: gfx.gui.state.show_grid,
                checker: gfx.gui.state.show_checker,
                ghost_opacity: gfx.gui.state.ghost_opacity,
            },
        );
        gfx.gui.paint(&gfx.device, &gfx.queue, &view, [size.0, size.1], output);
        gfx.window.pre_present_notify();
        gfx.queue.present(frame);

        let title = self.editor.title();
        if title != self.last_title {
            gfx.window.set_title(&title);
            self.last_title = title;
        }

        // Keep animating while keys pan the view or egui asks for it.
        if self.held.any() || delay.is_zero() {
            gfx.window.request_redraw();
        }

        for cmd in gfx.gui.state.take_commands() {
            self.run_command(cmd, event_loop);
            self.request_redraw();
        }
        self.sync_recent();
    }

    fn key(&mut self, event: &winit::event::KeyEvent) {
        let pressed = event.state == ElementState::Pressed;
        let ctrl = self.mods.control_key() || self.mods.super_key();
        if let PhysicalKey::Code(code) = event.physical_key {
            // Physical positions, so WASD means the same place on AZERTY (ZQSD).
            match code {
                KeyCode::KeyW | KeyCode::ArrowUp => self.held.up = pressed,
                KeyCode::KeyS | KeyCode::ArrowDown if !ctrl => self.held.down = pressed,
                KeyCode::ArrowDown => self.held.down = pressed,
                KeyCode::KeyA | KeyCode::ArrowLeft => self.held.left = pressed,
                KeyCode::KeyD | KeyCode::ArrowRight => self.held.right = pressed,
                KeyCode::Space => self.space = pressed,
                _ => {}
            }
            if pressed && !event.repeat && !ctrl {
                let digit = match code {
                    KeyCode::Digit1 => Some(1),
                    KeyCode::Digit2 => Some(2),
                    KeyCode::Digit3 => Some(3),
                    KeyCode::Digit4 => Some(4),
                    KeyCode::Digit5 => Some(5),
                    KeyCode::Digit6 => Some(6),
                    KeyCode::Digit7 => Some(7),
                    KeyCode::Digit8 => Some(8),
                    KeyCode::Digit9 => Some(9),
                    KeyCode::Digit0 => Some(10),
                    _ => None,
                };
                if let Some(id) = digit.filter(|&d| d <= TEXTURE_COUNT) {
                    self.editor.set_texture(id);
                }
            }
        }
        if !pressed {
            return;
        }
        if let Key::Named(NamedKey::Escape) = event.logical_key {
            if self.editor.placing_node {
                self.editor.cancel_placing_node();
            } else if self.editor.open_node_id().is_some() {
                self.editor.close_node(true);
            } else {
                self.editor.cancel_shape();
            }
            return;
        }
        if let Key::Named(NamedKey::Delete) = event.logical_key {
            self.editor.delete_open_node();
            return;
        }
        if let Key::Named(NamedKey::F1) = event.logical_key {
            if let Some(g) = self.gfx.as_mut() {
                g.gui.state.toggle_help();
            }
            return;
        }
        // Shortcuts use the key's label, not its position.
        if ctrl {
            if let Key::Character(c) = event.key_without_modifiers() {
                match c.as_str() {
                    "z" if self.mods.shift_key() => self.editor.redo(),
                    "z" => self.editor.undo(),
                    "y" => self.editor.redo(),
                    "s" if self.mods.shift_key() => self.request(Command::SaveAs),
                    "s" => self.save(),
                    "o" => self.request(Command::Open),
                    "n" => self.request(Command::New),
                    _ => {}
                }
            }
            return;
        }
        if let Key::Character(c) = &event.logical_key {
            match c.as_str() {
                "e" | "E" => self.editor.eraser = !self.editor.eraser,
                "n" | "N" => {
                    if self.editor.placing_node {
                        self.editor.cancel_placing_node();
                    } else {
                        self.editor.start_placing_node();
                    }
                }
                "[" => self.editor.change_brush(-1),
                "]" => self.editor.change_brush(1),
                _ => {}
            }
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gfx.is_some() {
            return;
        }
        let window = Arc::new(
            event_loop
                .create_window(
                    Window::default_attributes()
                        .with_title("depthmap")
                        .with_inner_size(PhysicalSize::new(1280, 800)),
                )
                .expect("create window"),
        );
        let size = window.inner_size();
        self.editor.resize(size.width as f64, size.height as f64);
        let mut gfx = Gfx::new(window);
        gfx.gui.state.set_recent(self.recent.paths());
        if self.show_startup {
            gfx.gui.state.open_startup();
        }
        self.gfx = Some(gfx);
        self.request_redraw();
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        match self.repaint_at {
            Some(t) if t <= Instant::now() => {
                self.repaint_at = None;
                self.request_redraw();
                event_loop.set_control_flow(ControlFlow::Wait);
            }
            Some(t) => event_loop.set_control_flow(ControlFlow::WaitUntil(t)),
            None => event_loop.set_control_flow(ControlFlow::Wait),
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        // egui sees every event first; it tells us whether it used it.
        let consumed = match self.gfx.as_mut() {
            Some(g) => {
                let r = g.egui_state.on_window_event(&g.window, &event);
                if r.repaint {
                    g.window.request_redraw();
                }
                r.consumed
            }
            None => false,
        };
        let over_ui = self.gfx.as_ref().is_some_and(|g| g.gui.pointer_over_ui());

        match event {
            WindowEvent::CloseRequested => self.request(Command::Quit),
            WindowEvent::Resized(size) => {
                if let Some(g) = self.gfx.as_mut() {
                    g.resize(size);
                }
                self.editor.resize(size.width as f64, size.height as f64);
                self.request_redraw();
            }
            WindowEvent::ModifiersChanged(m) => self.mods = m.state(),
            WindowEvent::KeyboardInput { event, .. } => {
                // Typing into a text field must not trigger shortcuts, but
                // key releases always count so movement never gets stuck.
                if !consumed || event.state == ElementState::Released {
                    self.key(&event);
                }
                self.request_redraw();
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.editor.mouse_moved(position.x, position.y);
                self.request_redraw();
            }
            WindowEvent::CursorLeft { .. } => {
                self.editor.mouse_left_window();
                self.request_redraw();
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let down = state == ElementState::Pressed;
                match (button, down) {
                    // Clicks that land on a panel or dialog belong to egui.
                    (_, true) if over_ui || consumed => {}
                    (MouseButton::Left, true) if self.space => self.editor.set_panning(true),
                    (MouseButton::Left, true) if self.mods.shift_key() => self.editor.begin_erase(),
                    (MouseButton::Left, true) if self.editor.placing_node => {
                        self.editor.place_node_at_cursor();
                    }
                    // A node under the mouse is grabbed instead of painted over.
                    (MouseButton::Left, true) => {
                        if !self.editor.press_node() {
                            self.editor.begin_paint();
                        }
                    }
                    // Right button: drag out a shape (Shift erases with it).
                    (MouseButton::Right, true) => self.editor.begin_shape(self.mods.shift_key()),
                    (MouseButton::Middle, true) => self.editor.set_panning(true),
                    (MouseButton::Right, false) => self.editor.end_shape(),
                    (MouseButton::Left, false) => {
                        self.editor.end_stroke();
                        self.editor.end_node_drag();
                        self.editor.set_panning(false);
                    }
                    // Other releases end the brush stroke and panning.
                    (_, false) => {
                        self.editor.end_stroke();
                        self.editor.set_panning(false);
                    }
                    _ => {}
                }
                self.request_redraw();
            }
            WindowEvent::MouseWheel { delta, .. } => {
                if !over_ui && !consumed {
                    let steps = match delta {
                        MouseScrollDelta::LineDelta(_, y) => y as f64,
                        MouseScrollDelta::PixelDelta(p) => p.y / 50.0,
                    };
                    self.editor.zoom(1.2f64.powf(steps));
                }
                self.request_redraw();
            }
            WindowEvent::RedrawRequested => self.redraw(event_loop),
            _ => {}
        }
    }
}

fn main() {
    env_logger::init();
    let mut file: Option<PathBuf> = None;
    let mut stress = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "-h" | "--help" => {
                print!("{HELP}");
                return;
            }
            "--stress" => stress = args.next().and_then(|n| n.parse::<i64>().ok()),
            _ => file = Some(PathBuf::from(a)),
        }
    }

    // A stress test is an unbounded sandbox; otherwise start from a default-sized map.
    let mut editor = if stress.is_some() {
        Editor::new()
    } else {
        Editor::with_spec(depthmap_core::MapSpec::DEFAULT_NEW)
    };
    // Launched with a file (or a stress test): go straight to it. Otherwise
    // the welcome screen offers the last map, opening another, or a new one.
    let show_startup = file.is_none() && stress.is_none();
    if let Some(path) = file {
        if path.exists() {
            editor.load_from(&path);
        } else {
            editor.path = Some(path);
        }
    }
    if let Some(n) = stress {
        depthmap::stress_fill(&mut editor.world, n);
    }
    let recent = Recent::load();
    print!("{HELP}");

    let event_loop = EventLoop::new().expect("event loop");
    event_loop.set_control_flow(ControlFlow::Wait);
    event_loop.run_app(&mut App::new(editor, recent, show_startup)).expect("run");
}
