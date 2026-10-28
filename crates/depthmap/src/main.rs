use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use depthmap::editor::Editor;
use depthmap::renderer::{FrameStats, Renderer};
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
use winit::window::{Window, WindowId};

const HELP: &str = "\
depthmap [FILE] [--stress N]

  LMB paint            RMB erase            MMB / Space+LMB pan
  wheel zoom           WASD / arrows pan    1-9, 0 material
  [ ] brush size       Ctrl+Z / Ctrl+Y undo / redo
  Ctrl+S save          Ctrl+O load          Esc quit
";

struct Gfx {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: Renderer,
}

impl Gfx {
    fn new(window: Arc<Window>) -> Self {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
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
        // A non-sRGB format keeps the palette's colours exactly as authored.
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
        Self { window, surface, config, device, queue, renderer }
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

struct App {
    gfx: Option<Gfx>,
    editor: Editor,
    mods: ModifiersState,
    held: Held,
    space: bool,
    last_tick: Instant,
    fps_frames: u32,
    fps_since: Instant,
    fps: f32,
    stats: FrameStats,
    dirty: bool,
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

impl App {
    fn new(editor: Editor) -> Self {
        Self {
            gfx: None,
            editor,
            mods: ModifiersState::empty(),
            held: Held::default(),
            space: false,
            last_tick: Instant::now(),
            fps_frames: 0,
            fps_since: Instant::now(),
            fps: 0.0,
            stats: FrameStats::default(),
            dirty: true,
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
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.stats = gfx.renderer.render(
            &view,
            (gfx.config.width, gfx.config.height),
            &self.editor.world,
            &self.editor.camera,
            self.editor.cursor_cell(),
            self.editor.brush_half,
        );
        gfx.window.pre_present_notify();
        gfx.queue.present(frame);

        self.fps_frames += 1;
        let elapsed = self.fps_since.elapsed();
        if elapsed >= Duration::from_millis(500) {
            self.fps = self.fps_frames as f32 / elapsed.as_secs_f32();
            self.fps_frames = 0;
            self.fps_since = Instant::now();
            let chunks = self.editor.world.total_chunks();
            gfx.window.set_title(&format!(
                "{} | drawn {} up {}",
                self.editor.title(self.fps, chunks),
                self.stats.chunks_drawn,
                self.stats.chunks_uploaded,
            ));
        }

        // Only spin while something is animating; otherwise sleep until input.
        self.dirty = self.held.any();
        if self.dirty {
            gfx.window.request_redraw();
        }
    }

    fn request_redraw(&mut self) {
        self.dirty = true;
        if let Some(g) = &self.gfx {
            g.window.request_redraw();
        }
    }

    fn key(&mut self, event: &winit::event::KeyEvent, event_loop: &ActiveEventLoop) {
        let pressed = event.state == ElementState::Pressed;
        if let PhysicalKey::Code(code) = event.physical_key {
            // Physical positions, so WASD means the same place on AZERTY (ZQSD).
            match code {
                KeyCode::KeyW | KeyCode::ArrowUp => self.held.up = pressed,
                KeyCode::KeyS | KeyCode::ArrowDown if !self.mods.control_key() => self.held.down = pressed,
                KeyCode::KeyA | KeyCode::ArrowLeft => self.held.left = pressed,
                KeyCode::KeyD | KeyCode::ArrowRight => self.held.right = pressed,
                KeyCode::Space => self.space = pressed,
                _ => {}
            }
            if pressed && !event.repeat {
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
                if let Some(m) = digit {
                    self.editor.set_material(m);
                }
            }
        }
        if !pressed {
            return;
        }
        if let Key::Named(NamedKey::Escape) = event.logical_key {
            event_loop.exit();
            return;
        }
        // Ctrl shortcuts use the key's label, not its position.
        if self.mods.control_key() {
            if let Key::Character(c) = event.key_without_modifiers() {
                match c.as_str() {
                    "z" if self.mods.shift_key() => self.editor.redo(),
                    "z" => self.editor.undo(),
                    "y" => self.editor.redo(),
                    "s" => self.editor.save(),
                    "o" => self.editor.load(),
                    _ => {}
                }
            }
            return;
        }
        if let Key::Character(c) = &event.logical_key {
            match c.as_str() {
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
        self.gfx = Some(Gfx::new(window));
        self.request_redraw();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(g) = self.gfx.as_mut() {
                    g.resize(size);
                }
                self.editor.resize(size.width as f64, size.height as f64);
                self.request_redraw();
            }
            WindowEvent::ModifiersChanged(m) => self.mods = m.state(),
            WindowEvent::KeyboardInput { event, .. } => {
                self.key(&event, event_loop);
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
                    (MouseButton::Left, true) if self.space => self.editor.set_panning(true),
                    (MouseButton::Left, true) => self.editor.begin_paint(),
                    (MouseButton::Right, true) => self.editor.begin_erase(),
                    (MouseButton::Middle, true) => self.editor.set_panning(true),
                    (_, false) => {
                        self.editor.end_stroke();
                        self.editor.set_panning(false);
                    }
                    _ => {}
                }
                self.request_redraw();
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let steps = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y as f64,
                    MouseScrollDelta::PixelDelta(p) => p.y / 50.0,
                };
                self.editor.zoom(1.2f64.powf(steps));
                self.request_redraw();
            }
            WindowEvent::RedrawRequested => self.redraw(event_loop),
            _ => {}
        }
    }
}

fn main() {
    env_logger::init();
    let mut path = PathBuf::from("map.dmap");
    let mut stress = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "-h" | "--help" => {
                print!("{HELP}");
                return;
            }
            "--stress" => stress = args.next().and_then(|n| n.parse::<i64>().ok()),
            _ => path = PathBuf::from(a),
        }
    }

    let mut editor = Editor::new(path.clone());
    editor.open_if_exists(&path);
    if let Some(n) = stress {
        depthmap::stress_fill(&mut editor.world, n);
    }
    print!("{HELP}");

    let event_loop = EventLoop::new().expect("event loop");
    event_loop.set_control_flow(ControlFlow::Wait);
    event_loop.run_app(&mut App::new(editor)).expect("run");
}
