//! The egui user interface: menu bar, floating texture/colour panel on the
//! left, zoom panel on the right, and the dialogs. `draw` only touches the
//! [`Editor`] and [`UiState`], so it works with any egui backend (the app
//! uses egui-winit; the tests render it offscreen).

use std::path::PathBuf;

use depthmap_core::camera::{MAX_ZOOM_LEVEL, MIN_ZOOM_LEVEL};
use depthmap_core::spec::{MAX_MAP_SIZE, MIN_MAP_SIZE};
use depthmap_core::icons::{self, BUILTIN_COUNT};
use depthmap_core::node::{MAX_DESCRIPTION_CHARS, MAX_NAME_CHARS, MAX_SCALE, MIN_SCALE};
use depthmap_core::{Fill, Icon, MAX_DEPTH, MapSpec, Shape, World};
use depthmap_core::texture::{self, TEXTURE_COUNT, TILE};
use egui::{Align, Align2, Color32, Layout, RichText, Sense, Stroke, TextureOptions, vec2};

use crate::editor::{Editor, SelectKind, ToolMode, format_zoom};
use crate::palette::{self, Entry, Palettes};
use crate::wheel;

/// Things the UI cannot do itself (file dialogs, quitting); the app runs them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// Asks for a new map; the UI shows the New Map dialog itself and later
    /// produces [`Command::Create`].
    New,
    /// Start an empty map of this size and depth.
    Create(MapSpec),
    /// Pick a file with a dialog, then open it.
    Open,
    /// Open this specific file (recent maps, the startup screen).
    OpenPath(PathBuf),
    SaveAs,
    /// Pick PNG files with a dialog and import them as node icons.
    ImportIcons,
    Quit,
}

/// A map in the recent list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecentEntry {
    pub path: PathBuf,
    /// Checked once when the list is set, not every frame.
    pub exists: bool,
}

impl RecentEntry {
    pub fn name(&self) -> String {
        self.path
            .file_stem()
            .map_or_else(|| self.path.display().to_string(), |n| n.to_string_lossy().into_owned())
    }

    pub fn folder(&self) -> String {
        self.path.parent().map_or_else(String::new, |p| p.display().to_string())
    }
}

/// Where interactive widgets were drawn last frame (screen points). Lets
/// tests and tooling click on real widgets instead of guessing coordinates.
#[derive(Default, Clone, Debug)]
pub struct UiLayout {
    pub textures: Vec<egui::Rect>,
    pub swatches: Vec<egui::Rect>,
    pub zoom_out: Option<egui::Rect>,
    pub zoom_in: Option<egui::Rect>,
    pub zoom_reset: Option<egui::Rect>,
    pub startup_continue: Option<egui::Rect>,
    pub startup_open: Option<egui::Rect>,
    pub startup_new: Option<egui::Rect>,
    pub startup_recent: Vec<egui::Rect>,
    pub shapes: Vec<egui::Rect>,
    pub fills: Vec<egui::Rect>,
    pub border_toggle: Option<egui::Rect>,
    pub eraser_toggle: Option<egui::Rect>,
    pub new_presets: Vec<egui::Rect>,
    pub new_create: Option<egui::Rect>,
    pub new_cancel: Option<egui::Rect>,
    pub place_node: Option<egui::Rect>,
    pub import_icons: Option<egui::Rect>,
    /// Icon choices in the node popup: the built-in ones, then imported.
    pub node_icons: Vec<egui::Rect>,
    pub node_done: Option<egui::Rect>,
    pub node_delete: Option<egui::Rect>,
    pub node_cancel: Option<egui::Rect>,
    pub node_import: Option<egui::Rect>,
    /// Brush, Fill, Select.
    pub tool_buttons: Vec<egui::Rect>,
    pub select_kinds: Vec<egui::Rect>,
    pub select_all: Option<egui::Rect>,
    pub select_invert: Option<egui::Rect>,
    pub select_clear: Option<egui::Rect>,
    pub wheel: Option<egui::Rect>,
    pub value_bar: Option<egui::Rect>,
    pub hex_field: Option<egui::Rect>,
    pub palette_new: Option<egui::Rect>,
    pub palette_rename: Option<egui::Rect>,
    pub palette_delete: Option<egui::Rect>,
    pub palette_add: Option<egui::Rect>,
    pub palette_entries: Vec<egui::Rect>,
    pub palette_name_field: Option<egui::Rect>,
    pub palette_rename_ok: Option<egui::Rect>,
}

#[derive(Clone, PartialEq, Eq, Hash)]
enum IconKey {
    Builtin(u8),
    Custom(String, u64),
}

/// An icon uploaded to egui in two sizes (the small one avoids shimmering
/// when the icon is drawn tiny).
#[derive(Clone)]
struct IconTex {
    big: egui::TextureHandle,
    small: egui::TextureHandle,
    /// Width and height of the picture, for drawing it with the right shape.
    aspect: (f32, f32),
}

impl IconTex {
    fn id(&self, size: f32) -> egui::TextureId {
        if size <= 40.0 { self.small.id() } else { self.big.id() }
    }
}

/// The New Map dialog's fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NewMapForm {
    pub width: u32,
    pub height: u32,
    /// How many times the map can be zoomed into.
    pub depths: u8,
}

impl Default for NewMapForm {
    fn default() -> Self {
        let s = MapSpec::DEFAULT_NEW;
        Self { width: s.width, height: s.height, depths: s.max_depth }
    }
}

impl NewMapForm {
    pub fn spec(&self) -> MapSpec {
        MapSpec::new(self.width, self.height, self.depths)
    }
}

pub struct UiState {
    pub layout: UiLayout,
    pub show_grid: bool,
    /// Darken every other cell so single cells are easier to see.
    pub show_checker: bool,
    /// Opacity of the parent layer shown behind the layer being drawn.
    pub ghost_opacity: f32,
    pub show_tools: bool,
    pub show_zoom: bool,
    /// The colour wheel and palettes window.
    pub show_colors: bool,
    show_help: bool,
    show_about: bool,
    /// A command waiting for the "unsaved changes" answer.
    confirm: Option<Command>,
    /// The welcome screen shown at launch.
    startup: bool,
    /// The New Map dialog, while it is open.
    new_map: Option<NewMapForm>,
    recent: Vec<RecentEntry>,
    ready: Vec<Command>,
    thumbs: Vec<egui::TextureHandle>,
    thumbs_rgb: Option<[u8; 3]>,
    icon_cache: std::collections::HashMap<IconKey, IconTex>,
    /// Icon files available in the Custom Icons folder or the map.
    icon_names: Vec<String>,
    seen_icons_rev: u64,
    /// Node whose name field should grab the keyboard when its popup opens.
    focus_node: Option<depthmap_core::NodeId>,
    palettes: Palettes,
    /// Hue, saturation, brightness of the brush colour (kept so that greys
    /// do not lose their hue while you drag the brightness around).
    hsv: [f32; 3],
    hsv_rgb: [u8; 3],
    hex_text: String,
    wheel_tex: Option<egui::TextureHandle>,
    tile_cache: std::collections::HashMap<(u8, [u8; 3]), egui::TextureHandle>,
    /// The palette name being edited.
    renaming: Option<String>,
    palette_note: String,
}

impl Default for UiState {
    fn default() -> Self {
        Self::new()
    }
}

impl UiState {
    pub fn new() -> Self {
        Self {
            layout: UiLayout::default(),
            show_grid: true,
            show_checker: false,
            ghost_opacity: 0.35,
            show_tools: true,
            show_zoom: true,
            show_colors: true,
            show_help: false,
            show_about: false,
            confirm: None,
            startup: false,
            new_map: None,
            recent: Vec::new(),
            ready: Vec::new(),
            thumbs: Vec::new(),
            thumbs_rgb: None,
            icon_cache: Default::default(),
            icon_names: Vec::new(),
            seen_icons_rev: u64::MAX,
            focus_node: None,
            palettes: Palettes::in_memory(),
            hsv: [0.0, 0.0, 1.0],
            hsv_rgb: [255, 255, 255],
            hex_text: String::new(),
            wheel_tex: None,
            tile_cache: Default::default(),
            renaming: None,
            palette_note: String::new(),
        }
    }

    pub fn palettes(&self) -> &Palettes {
        &self.palettes
    }

    pub fn palettes_mut(&mut self) -> &mut Palettes {
        &mut self.palettes
    }

    /// Uses these palettes (the app passes the ones stored on disk).
    pub fn set_palettes(&mut self, palettes: Palettes) {
        self.palettes = palettes;
        self.renaming = None;
    }

    /// The text in the hex colour field.
    pub fn hex_text(&self) -> &str {
        &self.hex_text
    }

    /// Hue, saturation and brightness shown on the wheel.
    pub fn hsv(&self) -> [f32; 3] {
        self.hsv
    }

    /// Follows the brush colour when something else changed it (a swatch, a
    /// palette entry), keeping the hue of greys.
    fn follow_brush(&mut self, rgb: [u8; 3]) {
        if self.hsv_rgb == rgb {
            return;
        }
        self.hsv_rgb = rgb;
        let [h, s, v] = wheel::rgb_to_hsv(rgb);
        self.hsv = [if s < 0.001 || v < 0.001 { self.hsv[0] } else { h }, s, v];
    }

    fn wheel_texture(&mut self, ctx: &egui::Context) -> egui::TextureId {
        self.wheel_tex
            .get_or_insert_with(|| {
                const N: usize = 192;
                ctx.load_texture(
                    "colour-wheel",
                    egui::ColorImage::from_rgba_unmultiplied([N, N], &wheel::wheel_rgba(N)),
                    TextureOptions::LINEAR,
                )
            })
            .id()
    }

    /// A texture tile in a colour, for palette entries.
    fn tile_texture(&mut self, ctx: &egui::Context, texture_id: u8, rgb: [u8; 3]) -> egui::TextureId {
        self.tile_cache
            .entry((texture_id, rgb))
            .or_insert_with(|| {
                ctx.load_texture(
                    format!("tile-{texture_id}-{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2]),
                    egui::ColorImage::from_rgba_unmultiplied([TILE, TILE], &texture::tinted_rgba(texture_id, rgb)),
                    TextureOptions::LINEAR,
                )
            })
            .id()
    }

    /// The picture for an icon, uploading it on first use. Imported icons
    /// that cannot be decoded (or are missing) show the "missing" icon.
    fn icon_tex(&mut self, ctx: &egui::Context, world: &World, icon: &Icon) -> IconTex {
        let (key, image) = match icon {
            Icon::Builtin(id) => (IconKey::Builtin(*id), None),
            Icon::Custom(name) => match world.custom_icon(name) {
                Some(c) => (IconKey::Custom(name.clone(), c.hash), Some(c)),
                None => (IconKey::Builtin(0), None),
            },
        };
        if let Some(t) = self.icon_cache.get(&key) {
            return t.clone();
        }
        let img = match (&key, image) {
            (IconKey::Custom(..), Some(c)) => {
                icons::decode_png(&c.png).unwrap_or_else(|_| icons::builtin_image(0))
            }
            (IconKey::Builtin(id), _) => icons::builtin_image(*id),
            _ => icons::builtin_image(0),
        };
        let upload = |name: String, img: &icons::Image| {
            ctx.load_texture(
                name,
                egui::ColorImage::from_rgba_unmultiplied([img.w, img.h], &img.rgba),
                TextureOptions::LINEAR,
            )
        };
        let tag = match &key {
            IconKey::Builtin(id) => format!("b{id}"),
            IconKey::Custom(n, h) => format!("{n}-{h}"),
        };
        let tex = IconTex {
            big: upload(format!("icon-{tag}"), &icons::downscale(&img, 128)),
            small: upload(format!("icon-{tag}-s"), &icons::downscale(&img, 32)),
            aspect: (img.w as f32, img.h as f32),
        };
        self.icon_cache.insert(key, tex.clone());
        tex
    }

    /// The picture for an icon file that is in the Custom Icons folder but
    /// not (yet) in the map, for the picker.
    fn icon_tex_from_file(&mut self, ctx: &egui::Context, dir: &std::path::Path, name: &str) -> IconTex {
        let key = IconKey::Custom(format!("file:{name}"), 0);
        if let Some(t) = self.icon_cache.get(&key) {
            return t.clone();
        }
        let img = std::fs::read(dir.join(name))
            .ok()
            .and_then(|b| icons::decode_png(&b).ok())
            .unwrap_or_else(|| icons::builtin_image(0));
        let upload = |tag: &str, img: &icons::Image| {
            ctx.load_texture(
                format!("icon-file-{name}{tag}"),
                egui::ColorImage::from_rgba_unmultiplied([img.w, img.h], &img.rgba),
                TextureOptions::LINEAR,
            )
        };
        let tex = IconTex {
            big: upload("", &icons::downscale(&img, 128)),
            small: upload("-s", &icons::downscale(&img, 32)),
            aspect: (img.w as f32, img.h as f32),
        };
        self.icon_cache.insert(key, tex.clone());
        tex
    }

    /// Re-reads which icon files exist when something may have changed.
    fn refresh_icon_names(&mut self, ed: &Editor) {
        if self.seen_icons_rev == ed.icons_revision {
            return;
        }
        self.seen_icons_rev = ed.icons_revision;
        let mut names = icons::list_custom(&ed.icons_dir);
        for (name, _) in ed.world.custom_icons() {
            if !names.contains(name) {
                names.push(name.clone());
            }
        }
        names.sort_by_key(|n| n.to_lowercase());
        self.icon_names = names;
    }

    /// Asks for a command to run. Commands that would throw away unsaved
    /// work first show a confirmation dialog.
    pub fn request(&mut self, cmd: Command, modified: bool) {
        let destructive = matches!(
            cmd,
            Command::New | Command::Open | Command::OpenPath(_) | Command::Quit
        );
        if destructive && modified {
            self.confirm = Some(cmd);
        } else {
            self.dispatch(cmd);
        }
    }

    /// Runs a command that needs no confirmation (any more). `New` only
    /// opens the dialog; the app never sees it.
    fn dispatch(&mut self, cmd: Command) {
        match cmd {
            Command::New => self.open_new_map(),
            cmd => self.ready.push(cmd),
        }
    }

    /// Opens the New Map dialog with the default size.
    pub fn open_new_map(&mut self) {
        self.new_map = Some(NewMapForm::default());
    }

    pub fn new_map_open(&self) -> bool {
        self.new_map.is_some()
    }

    pub fn new_map_form(&self) -> Option<NewMapForm> {
        self.new_map
    }

    pub fn set_new_map_form(&mut self, form: NewMapForm) {
        if self.new_map.is_some() {
            self.new_map = Some(form);
        }
    }

    /// The app calls this once a map was created.
    pub fn close_new_map(&mut self) {
        self.new_map = None;
    }

    /// Commands the app should execute now.
    pub fn take_commands(&mut self) -> Vec<Command> {
        std::mem::take(&mut self.ready)
    }

    pub fn confirm_open(&self) -> bool {
        self.confirm.is_some()
    }

    /// Shows the welcome screen (continue last map / open / new).
    pub fn open_startup(&mut self) {
        self.startup = true;
    }

    pub fn close_startup(&mut self) {
        self.startup = false;
    }

    pub fn startup_open(&self) -> bool {
        self.startup
    }

    /// Replaces the recent-maps list (most recent first).
    pub fn set_recent(&mut self, paths: &[PathBuf]) {
        self.recent = paths
            .iter()
            .map(|p| RecentEntry { path: p.clone(), exists: p.is_file() })
            .collect();
    }

    pub fn recent(&self) -> &[RecentEntry] {
        &self.recent
    }

    pub fn toggle_help(&mut self) {
        self.show_help = !self.show_help;
    }

    fn refresh_thumbs(&mut self, ctx: &egui::Context, rgb: [u8; 3]) {
        if self.thumbs_rgb == Some(rgb) && self.thumbs.len() == TEXTURE_COUNT as usize {
            return;
        }
        self.thumbs_rgb = Some(rgb);
        for id in 1..=TEXTURE_COUNT {
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [TILE, TILE],
                &texture::tinted_rgba(id, rgb),
            );
            match self.thumbs.get_mut(id as usize - 1) {
                Some(handle) => handle.set(image, TextureOptions::LINEAR),
                None => self.thumbs.push(ctx.load_texture(
                    format!("texture-{id}"),
                    image,
                    TextureOptions::LINEAR,
                )),
            }
        }
    }
}

pub fn setup_style(ctx: &egui::Context) {
    let mut style = (*ctx.global_style()).clone();
    let fill = style.visuals.window_fill;
    style.visuals.window_fill = Color32::from_rgba_unmultiplied(fill.r(), fill.g(), fill.b(), 238);
    style.visuals.window_shadow = egui::Shadow {
        offset: [0, 2],
        blur: 12,
        spread: 0,
        color: Color32::from_black_alpha(90),
    };
    style.spacing.item_spacing = vec2(6.0, 6.0);
    ctx.set_global_style(style);
}

/// Draws the whole interface for one frame.
pub fn draw(ui: &mut egui::Ui, ed: &mut Editor, st: &mut UiState) {
    let ctx = &ui.ctx().clone();
    menu_bar(ui, ed, st);
    // Dialogs that ask for something block the Enter shortcut of the welcome screen.
    let modal = st.new_map.is_some() || st.confirm.is_some();
    if st.show_tools {
        tools_panel(ctx, ed, st);
    }
    if st.show_zoom {
        zoom_panel(ctx, ed, st);
    }
    if st.show_colors {
        colors_window(ctx, ed, st);
    }
    selection_layer(ctx, ed);
    nodes_layer(ctx, ed, st);
    dialogs(ctx, ed, st);
    node_popup(ctx, ed, st);
    startup_window(ctx, st, modal);
    new_map_window(ctx, st);
}

fn menu_bar(root: &mut egui::Ui, ed: &mut Editor, st: &mut UiState) {
    egui::Panel::top("menu_bar").show(root, |ui| {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if item(ui, "New", "Ctrl+N") {
                    st.request(Command::New, ed.modified());
                }
                if item(ui, "Open...", "Ctrl+O") {
                    st.request(Command::Open, ed.modified());
                }
                ui.menu_button("Open recent", |ui| {
                    if st.recent.is_empty() {
                        ui.add_enabled(false, egui::Button::new("No recent maps"));
                    }
                    for entry in st.recent.clone().iter().take(8) {
                        let button = ui
                            .add_enabled(entry.exists, egui::Button::new(entry.name()))
                            .on_hover_text(entry.path.display().to_string());
                        if button.clicked() {
                            st.request(Command::OpenPath(entry.path.clone()), ed.modified());
                            ui.close();
                        }
                    }
                });
                ui.separator();
                if item(ui, "Save", "Ctrl+S") {
                    if !ed.save() {
                        st.request(Command::SaveAs, ed.modified());
                    }
                }
                if item(ui, "Save As...", "Ctrl+Shift+S") {
                    st.request(Command::SaveAs, ed.modified());
                }
                ui.separator();
                if item(ui, "Import PNG icons...", "") {
                    st.request(Command::ImportIcons, false);
                }
                ui.separator();
                if item(ui, "Quit", "") {
                    st.request(Command::Quit, ed.modified());
                }
            });
            ui.menu_button("Edit", |ui| {
                ui.add_enabled_ui(ed.history.can_undo(), |ui| {
                    if item(ui, "Undo", "Ctrl+Z") {
                        ed.undo();
                    }
                });
                ui.add_enabled_ui(ed.history.can_redo(), |ui| {
                    if item(ui, "Redo", "Ctrl+Y") {
                        ed.redo();
                    }
                });
                ui.separator();
                if item(ui, "Select all", "Ctrl+A") {
                    ed.select_all();
                }
                if item(ui, "Invert selection", "Ctrl+I") {
                    ed.invert_selection();
                }
                ui.add_enabled_ui(ed.has_selection(), |ui| {
                    if item(ui, "Deselect", "Ctrl+D / Esc") {
                        ed.clear_selection();
                    }
                });
                ui.separator();
                if item(ui, "Add node", "N") {
                    ed.start_placing_node();
                }
            });
            ui.menu_button("View", |ui| {
                ui.checkbox(&mut st.show_grid, "Show grid");
                ui.checkbox(&mut st.show_checker, "Checker overlay (darken every other cell)");
                ui.checkbox(&mut ed.show_nodes, "Show nodes");
                ui.checkbox(&mut st.show_tools, "Textures & tools panel");
                ui.checkbox(&mut st.show_colors, "Colors window");
                ui.checkbox(&mut st.show_zoom, "Zoom panel");
                ui.separator();
                if item(ui, "Zoom in", "") {
                    ed.zoom_centre(2.0);
                }
                if item(ui, "Zoom out", "") {
                    ed.zoom_centre(0.5);
                }
                if item(ui, "Reset zoom", "") {
                    ed.reset_zoom();
                }
                if item(ui, "Reset view", "") {
                    ed.reset_view();
                }
            });
            ui.menu_button("Help", |ui| {
                if item(ui, "Controls", "F1") {
                    st.show_help = true;
                }
                if item(ui, "About", "") {
                    st.show_about = true;
                }
            });
            if let Some(info) = ed.hover_info() {
                ui.separator();
                let mut text = RichText::new(info.text()).monospace();
                if !info.inside {
                    text = text.weak();
                }
                ui.label(text).on_hover_text(
                    "Cell under the mouse at the depth you are drawing on,\nand the parent cell (one depth up) it belongs to",
                );
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let name = format!("{}{}", ed.file_name(), if ed.modified() { " *" } else { "" });
                ui.label(RichText::new(name).strong());
                if !ed.status.is_empty() {
                    ui.separator();
                    ui.label(RichText::new(&ed.status).weak());
                }
            });
        });
    });
}

/// A menu entry with an optional shortcut hint; closes the menu when clicked.
fn item(ui: &mut egui::Ui, label: &str, shortcut: &str) -> bool {
    let mut button = egui::Button::new(label);
    if !shortcut.is_empty() {
        button = button.shortcut_text(shortcut);
    }
    let clicked = ui.add(button).clicked();
    if clicked {
        ui.close();
    }
    clicked
}

fn tools_panel(ctx: &egui::Context, ed: &mut Editor, st: &mut UiState) {
    st.refresh_thumbs(ctx, ed.brush_rgb);
    st.layout.textures.clear();
    st.layout.shapes.clear();
    st.layout.tool_buttons.clear();
    st.layout.select_kinds.clear();
    st.layout.select_all = None;
    st.layout.select_invert = None;
    st.layout.select_clear = None;
    st.layout.fills.clear();
    st.layout.border_toggle = None;
    st.layout.eraser_toggle = None;
    st.layout.place_node = None;
    st.layout.import_icons = None;
    let max_height = (ctx.content_rect().height() - 70.0).max(240.0);
    egui::Window::new("Tools")
        .anchor(Align2::LEFT_TOP, [10.0, 40.0])
        .max_width(260.0)
        .resizable(false)
        .collapsible(true)
        .show(ctx, |ui| {
          egui::ScrollArea::vertical()
              .min_scrolled_height(max_height)
              .max_height(max_height)
              .auto_shrink([false, true])
              .show(ui, |ui| {
            const COLUMNS: usize = 4;
            const THUMB: f32 = 46.0;
            ui.set_width(COLUMNS as f32 * (THUMB + 6.0));
            ui.set_max_width(COLUMNS as f32 * (THUMB + 6.0));

            tool_section(ui, ed, st);
            ui.separator();
            ui.label(RichText::new("Texture").strong());
            egui::Grid::new("texture_grid")
                .spacing([6.0, 6.0])
                .show(ui, |ui| {
                    for id in 1..=TEXTURE_COUNT {
                        let (rect, resp) =
                            ui.allocate_exact_size(vec2(THUMB, THUMB), Sense::click());
                        let selected = ed.texture == id;
                        st.layout.textures.push(rect);
                        ui.painter().image(
                            st.thumbs[id as usize - 1].id(),
                            rect,
                            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                            Color32::WHITE,
                        );
                        let stroke = if selected {
                            Stroke::new(2.5, ui.visuals().selection.stroke.color)
                        } else if resp.hovered() {
                            Stroke::new(1.5, ui.visuals().widgets.hovered.fg_stroke.color)
                        } else {
                            Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color)
                        };
                        ui.painter()
                            .rect_stroke(rect, 3.0, stroke, egui::StrokeKind::Inside);
                        if resp.on_hover_text(texture::name(id)).clicked() {
                            ed.set_texture(id);
                        }
                        if id as usize % COLUMNS == 0 {
                            ui.end_row();
                        }
                    }
                });
            ui.label(RichText::new(texture::name(ed.texture)).strong());

            ui.separator();
            let mut half = ed.brush_half;
            ui.horizontal(|ui| {
                ui.label("Brush");
                ui.add(
                    egui::Slider::new(&mut half, 0..=16)
                        .custom_formatter(|v, _| format!("{0}x{0}", 2 * v as i64 + 1)),
                );
            });
            ed.brush_half = half;
            let eraser = ui
                .checkbox(&mut ed.eraser, "Eraser  (E, or hold Shift)")
                .on_hover_text("Brush and shapes remove cells instead of painting");
            st.layout.eraser_toggle = Some(eraser.rect);

            ui.separator();
            ui.label(RichText::new("Shape  (right mouse drag)").strong());
            egui::Grid::new("shape_grid").spacing([4.0, 4.0]).show(ui, |ui| {
                for (i, shape) in Shape::ALL.into_iter().enumerate() {
                    let r = ui.selectable_value(&mut ed.shape, shape, shape.name());
                    st.layout.shapes.push(r.rect);
                    if i % 3 == 2 {
                        ui.end_row();
                    }
                }
            });
            ui.label(RichText::new("Fill").strong());
            egui::Grid::new("fill_grid").spacing([4.0, 4.0]).show(ui, |ui| {
                for (i, fill) in Fill::ALL.into_iter().enumerate() {
                    let r = ui.selectable_value(&mut ed.fill, fill, fill.name());
                    st.layout.fills.push(r.rect);
                    if i % 3 == 2 {
                        ui.end_row();
                    }
                }
            });
            let border = ui
                .add_enabled(
                    ed.fill != Fill::Outline && ed.shape != Shape::Line,
                    egui::Checkbox::new(&mut ed.border, "Solid border around the fill"),
                )
                .on_hover_text("Brush size sets the border / outline thickness");
            st.layout.border_toggle = Some(border.rect);

            ui.separator();
            ui.label(RichText::new("Nodes").strong());
            ui.horizontal(|ui| {
                let tex = st.icon_tex(ctx, &ed.world, &ed.default_icon);
                let (rect, _) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::hover());
                ui.painter().image(
                    tex.id(24.0),
                    fit_rect(rect, tex.aspect),
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
                let add = ui
                    .selectable_label(ed.placing_node, "Add node  (N)")
                    .on_hover_text("Then click the map. Click a node later to edit it, drag to move it.");
                st.layout.place_node = Some(add.rect);
                if add.clicked() {
                    if ed.placing_node {
                        ed.cancel_placing_node();
                    } else {
                        ed.start_placing_node();
                    }
                }
            });
            let import = ui
                .button("Import PNG icons...")
                .on_hover_text(format!("Copied to {}", ed.icons_dir.display()));
            st.layout.import_icons = Some(import.rect);
            if import.clicked() {
                st.request(Command::ImportIcons, false);
            }
          });
        });
}

fn zoom_panel(ctx: &egui::Context, ed: &mut Editor, st: &mut UiState) {
    egui::Window::new("Zoom")
        .anchor(Align2::RIGHT_TOP, [-10.0, 40.0])
        .max_width(210.0)
        .resizable(false)
        .collapsible(true)
        .show(ctx, |ui| {
            ui.set_width(190.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(ed.zoom_label()).size(26.0).strong());
                ui.label(
                    RichText::new(format!(
                        "depth {} of {}  ·  {:.1} px/cell",
                        ed.camera.depth,
                        ed.spec().max_depth,
                        ed.pixels_per_cell()
                    ))
                    .weak(),
                );
                if let Some((w, h)) = ed.spec().extent(0) {
                    ui.label(RichText::new(format!("map {w} × {h} cells")).weak());
                }
                ui.label(RichText::new(format!("snap: {}", snap_label(ed.camera.depth))).weak())
                    .on_hover_text(
                        "Strokes snap to the cells of the depth you are drawing on.\n\
                         The grid is only an overlay; it never changes the map.",
                    );
            });

            ui.horizontal(|ui| {
                let w = (ui.available_width() - 12.0) / 3.0;
                let out = ui.add_sized([w, 28.0], egui::Button::new("−")).on_hover_text("Zoom out");
                st.layout.zoom_out = Some(out.rect);
                if out.clicked() {
                    ed.zoom_centre(0.5);
                }
                let reset = ui.add_sized([w, 28.0], egui::Button::new("1:1")).on_hover_text("Reset zoom");
                st.layout.zoom_reset = Some(reset.rect);
                if reset.clicked() {
                    ed.reset_zoom();
                }
                let zin = ui.add_sized([w, 28.0], egui::Button::new("+")).on_hover_text("Zoom in");
                st.layout.zoom_in = Some(zin.rect);
                if zin.clicked() {
                    ed.zoom_centre(2.0);
                }
            });

            let mut level = ed.zoom_level();
            ui.spacing_mut().slider_width = 170.0;
            let (lo, hi) = ed.zoom_range();
            let slider = egui::Slider::new(&mut level, lo..=hi).show_value(false);
            if ui.add(slider).changed() {
                ed.set_zoom_level(level);
            }

            ui.separator();
            ui.label(RichText::new("Depth").strong());
            ui.horizontal(|ui| {
                let w = (ui.available_width() - 6.0) / 2.0;
                if ui
                    .add_sized([w, 26.0], egui::Button::new("Up one"))
                    .on_hover_text("Back to the parent layer")
                    .clicked()
                {
                    ed.jump_depth(-1);
                }
                if ui
                    .add_enabled(
                        ed.camera.depth < ed.spec().max_depth,
                        egui::Button::new("Down one").min_size(vec2(w, 26.0)),
                    )
                    .on_hover_text("Into the next, finer layer")
                    .clicked()
                {
                    ed.jump_depth(1);
                }
            });

            ui.separator();
            ui.label(RichText::new("Parent layer").strong());
            ui.add(
                egui::Slider::new(&mut st.ghost_opacity, 0.0..=1.0)
                    .custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
            )
            .on_hover_text("Opacity of the layer behind the one you are drawing on");
            ui.checkbox(&mut st.show_grid, "Show grid");
            ui.checkbox(&mut st.show_checker, "Checker overlay")
                .on_hover_text("Darkens every other cell of the depth you draw on, so single cells are easier to see");
            if ui.button("Show whole map").clicked() {
                ed.reset_view();
            }
        });
}

/// Cell size of a depth relative to a depth-0 cell, e.g. `1/16 cell`.
pub fn snap_label(depth: u8) -> String {
    let n = (depth_subdiv() as u128).pow(depth as u32);
    if depth == 0 { "1 cell".to_string() } else { format!("1/{n} cell") }
}

fn depth_subdiv() -> i64 {
    depthmap_core::SUBDIV
}

fn startup_window(ctx: &egui::Context, st: &mut UiState, modal: bool) {
    st.layout.startup_continue = None;
    st.layout.startup_open = None;
    st.layout.startup_new = None;
    st.layout.startup_recent.clear();
    // The New Map dialog takes over; the welcome screen returns if it is cancelled.
    if !st.startup || st.new_map.is_some() {
        return;
    }
    let recent = st.recent.clone();
    let last = recent.iter().find(|e| e.exists).cloned();
    let enter = !modal && ctx.input(|i| i.key_pressed(egui::Key::Enter));
    let mut chosen: Option<Command> = None;

    egui::Window::new("Welcome to depthmap")
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.set_width(430.0);
            ui.label("Pick up where you left off, open a map, or start a new one.");
            ui.add_space(6.0);

            if let Some(entry) = &last {
                let fill = ui.visuals().selection.bg_fill;
                let button = egui::Button::new(
                    RichText::new(format!("Continue with  {}", entry.name())).size(17.0).strong(),
                )
                .fill(fill);
                let resp = ui.add_sized([ui.available_width(), 44.0], button);
                st.layout.startup_continue = Some(resp.rect);
                ui.label(RichText::new(entry.folder()).weak().small());
                if resp.clicked() || enter {
                    chosen = Some(Command::OpenPath(entry.path.clone()));
                }
                ui.add_space(4.0);
            }

            ui.horizontal(|ui| {
                let w = (ui.available_width() - 6.0) / 2.0;
                let open = ui.add_sized([w, 32.0], egui::Button::new("Open map..."));
                st.layout.startup_open = Some(open.rect);
                if open.clicked() {
                    chosen = Some(Command::Open);
                }
                let new = ui.add_sized([w, 32.0], egui::Button::new("New map"));
                st.layout.startup_new = Some(new.rect);
                if new.clicked() {
                    chosen = Some(Command::New);
                }
            });

            if !recent.is_empty() {
                ui.separator();
                ui.label(RichText::new("Recent maps").strong());
                for entry in recent.iter().take(8) {
                    let text = if entry.exists {
                        format!("{}    {}", entry.name(), entry.folder())
                    } else {
                        format!("{}    (file not found)", entry.name())
                    };
                    let resp = ui.add_enabled(
                        entry.exists,
                        egui::Button::new(text).frame(false).min_size(vec2(ui.available_width(), 24.0)),
                    );
                    st.layout.startup_recent.push(resp.rect);
                    if resp.clicked() {
                        chosen = Some(Command::OpenPath(entry.path.clone()));
                    }
                }
            }

            ui.separator();
            ui.label(
                RichText::new(
                    "A map is one .dmap file. Save writes to the open map; \
                     a new map asks where to save the first time.",
                )
                .weak()
                .small(),
            );
        });

    if let Some(cmd) = chosen {
        st.dispatch(cmd);
    }
}

/// Brush / Fill / Select buttons and the selection controls.
fn tool_section(ui: &mut egui::Ui, ed: &mut Editor, st: &mut UiState) {
    ui.label(RichText::new("Tool").strong());
    ui.horizontal_wrapped(|ui| {
        for (mode, label, tip) in [
            (ToolMode::Paint, "Brush", "Paint with the brush (B). Right mouse drags shapes."),
            (ToolMode::Fill, "Fill", "Bucket (G): click to fill the connected area of identical cells. Shift erases it."),
            (ToolMode::Select, "Select", "Selection (M): painting is limited to the selected cells"),
        ] {
            let r = ui.selectable_label(ed.tool == mode, label).on_hover_text(tip);
            st.layout.tool_buttons.push(r.rect);
            if r.clicked() {
                ed.tool = mode;
            }
        }
    });
    match ed.tool {
        ToolMode::Fill => {
            ui.label(RichText::new("Click: fill the connected area of the same tile (Shift: erase)").weak().small());
        }
        ToolMode::Select => {
            ui.horizontal_wrapped(|ui| {
                for kind in SelectKind::ALL {
                    let r = ui.selectable_label(ed.select_kind == kind, kind.name()).on_hover_text(kind.hint());
                    st.layout.select_kinds.push(r.rect);
                    if r.clicked() {
                        ed.select_kind = kind;
                    }
                }
            });
            ui.label(RichText::new(format!("{}. Right click inverts; new selections add.", ed.select_kind.hint())).weak().small());
            ui.horizontal(|ui| {
                let all = ui.small_button("All").on_hover_text("Select everything (Ctrl+A)");
                st.layout.select_all = Some(all.rect);
                if all.clicked() {
                    ed.select_all();
                }
                let inv = ui.small_button("Invert").on_hover_text("Select what is not selected (right click, Ctrl+I)");
                st.layout.select_invert = Some(inv.rect);
                if inv.clicked() {
                    ed.invert_selection();
                }
                let clear = ui.add_enabled(ed.has_selection(), egui::Button::new("Clear").small());
                st.layout.select_clear = Some(clear.rect);
                if clear.on_hover_text("Drop the selection (Esc, Ctrl+D)").clicked() {
                    ed.clear_selection();
                }
            });
        }
        ToolMode::Paint => {}
    }
    if let Some(summary) = ed.selection_summary() {
        ui.label(RichText::new(summary).color(Color32::from_rgb(90, 190, 255)));
        if ed.tool != ToolMode::Select {
            ui.label(RichText::new("Painting is limited to it.").weak().small());
            let clear = ui.small_button("Clear selection  (Esc)");
            st.layout.select_clear = Some(clear.rect);
            if clear.clicked() {
                ed.clear_selection();
            }
        }
    }
}

/// Draws the selection (and the rectangle being dragged) over the map.
fn selection_layer(ctx: &egui::Context, ed: &Editor) {
    if ed.tool != ToolMode::Paint && !ctx.is_pointer_over_egui() {
        ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
    }
    let preview = ed.select_preview();
    if ed.selection().is_none() && preview.is_none() {
        return;
    }
    let ppp = ctx.pixels_per_point();
    let painter = ctx
        .layer_painter(egui::LayerId::new(egui::Order::Background, egui::Id::unique("selection")))
        .with_clip_rect(ctx.content_rect());
    if let Some(s) = ed.selection() {
        draw_selection(&painter, ed, s, ppp, Color32::from_rgb(90, 190, 255));
    }
    if let Some(s) = &preview {
        draw_selection(&painter, ed, s, ppp, Color32::from_rgb(255, 214, 90));
    }
}

fn draw_selection(
    painter: &egui::Painter,
    ed: &Editor,
    sel: &depthmap_core::Selection,
    ppp: f32,
    colour: Color32,
) {
    use depthmap_core::selection::spans_subtract;
    /// Enough for any sane selection; keeps a pathological one from stalling the frame.
    const MAX_SHAPES: usize = 20_000;
    let v = ed.camera.at_depth(sel.depth());
    let (vw, vh) = ed.viewport;
    let ((x0, y0), (x1, y1)) = v.visible_cells(vw, vh);
    let (mut x0, mut y0, mut x1, mut y1) = (x0 - 1, y0 - 1, x1 + 1, y1 + 1);
    if let Some((w, h)) = ed.spec().extent(sel.depth()) {
        (x0, y0, x1, y1) = (x0.max(0), y0.max(0), x1.min(w - 1), y1.min(h - 1));
    }
    let blocks = sel.blocks(x0, x1, y0, y1);
    let at = |cx: i64, cy: i64| {
        let (x, y) = v.cell_origin_px(cx, cy, vw, vh);
        egui::pos2(x as f32 / ppp, y as f32 / ppp)
    };
    let fill = Color32::from_rgba_unmultiplied(colour.r(), colour.g(), colour.b(), 56);
    let mut lines: Vec<[egui::Pos2; 2]> = Vec::new();
    for (i, b) in blocks.iter().enumerate() {
        if lines.len() > MAX_SHAPES {
            break;
        }
        for &(a, c) in &b.spans {
            painter.rect_filled(egui::Rect::from_two_pos(at(a, b.y0), at(c + 1, b.y1 + 1)), 0.0, fill);
            lines.push([at(a, b.y0), at(a, b.y1 + 1)]);
            lines.push([at(c + 1, b.y0), at(c + 1, b.y1 + 1)]);
        }
        // Top and bottom edges: only where the neighbouring rows are not selected.
        let above = i.checked_sub(1).map(|j| &blocks[j]).filter(|p| p.y1 + 1 == b.y0);
        let below = blocks.get(i + 1).filter(|n| n.y0 == b.y1 + 1);
        for (row, edge_y) in [(above, b.y0), (below, b.y1 + 1)] {
            let open = match row {
                Some(r) => spans_subtract(&b.spans, &r.spans),
                None => b.spans.clone(),
            };
            for (a, c) in open {
                lines.push([at(a, edge_y), at(c + 1, edge_y)]);
            }
        }
    }
    for l in &lines {
        painter.line_segment(*l, Stroke::new(3.0, Color32::from_black_alpha(150)));
    }
    for l in &lines {
        painter.line_segment(*l, Stroke::new(1.5, colour));
    }
}

/// The colour wheel, hex field, map colours and palettes.
fn colors_window(ctx: &egui::Context, ed: &mut Editor, st: &mut UiState) {
    let l = &mut st.layout;
    l.swatches.clear();
    l.palette_entries.clear();
    (l.wheel, l.value_bar, l.hex_field) = (None, None, None);
    (l.palette_new, l.palette_rename, l.palette_delete, l.palette_add) = (None, None, None, None);
    (l.palette_name_field, l.palette_rename_ok) = (None, None);
    st.follow_brush(ed.brush_rgb);

    let screen = ctx.content_rect();
    // A Window does not grow around a ScrollArea by itself, so give it the
    // height it needs (or what the screen has).
    let max_height = (screen.height() - 90.0).clamp(260.0, 620.0);
    let mut open = st.show_colors;
    let mut acts: Vec<PaletteAct> = Vec::new();
    egui::Window::new("Colors")
        // Left of the zoom panel, so the two do not cover each other.
        .default_pos(egui::pos2((screen.right() - 500.0).max(290.0), 40.0))
        .resizable(false)
        .collapsible(true)
        .open(&mut open)
        .show(ctx, |ui| {
            egui::ScrollArea::vertical()
                .min_scrolled_height(max_height)
                .max_height(max_height)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.set_width(250.0);
                    wheel_section(ui, ctx, ed, st);
                    ui.separator();
                    ui.label(RichText::new("Map colors").strong());
                    let colors: Vec<[u8; 3]> = ed.world.colors.as_slice().to_vec();
                    egui::Grid::new("swatches").spacing([4.0, 4.0]).min_col_width(0.0).show(ui, |ui| {
                        for (i, rgb) in colors.iter().enumerate() {
                            let (rect, resp) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::click());
                            ui.painter().rect_filled(rect, 3.0, Color32::from_rgb(rgb[0], rgb[1], rgb[2]));
                            st.layout.swatches.push(rect);
                            let stroke = if ed.brush_rgb == *rgb {
                                Stroke::new(2.5, ui.visuals().selection.stroke.color)
                            } else {
                                Stroke::new(1.0, Color32::from_black_alpha(120))
                            };
                            ui.painter().rect_stroke(rect, 3.0, stroke, egui::StrokeKind::Inside);
                            let tip = format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2]);
                            if resp.on_hover_text(tip).clicked() {
                                ed.set_brush_rgb(*rgb);
                            }
                            if (i + 1) % 9 == 0 {
                                ui.end_row();
                            }
                        }
                    });
                    ui.separator();
                    palette_section(ui, ctx, ed, st, &mut acts);
                });
        });
    st.show_colors = open;

    let mut changed = false;
    for act in acts {
        let cur = st.palettes.current_index();
        match act {
            PaletteAct::Select(i) => {
                st.palettes.select(i);
                st.renaming = None;
            }
            PaletteAct::New => {
                changed |= st.palettes.create().is_some();
                st.renaming = None;
                if !changed {
                    st.palette_note = "Too many palettes".into();
                }
            }
            PaletteAct::StartRename => st.renaming = Some(st.palettes.current().name.clone()),
            PaletteAct::Rename(name) => {
                if st.palettes.rename(cur, &name) {
                    st.renaming = None;
                    st.palette_note.clear();
                    changed = true;
                } else {
                    st.palette_note = "A palette needs a name".into();
                }
            }
            PaletteAct::CancelRename => st.renaming = None,
            PaletteAct::Delete => {
                st.palettes.delete(cur);
                st.renaming = None;
                changed = true;
            }
            PaletteAct::Add => {
                let entry = Entry { rgb: ed.brush_rgb, texture: ed.texture };
                if st.palettes.add_entry(cur, entry) {
                    st.palette_note.clear();
                    changed = true;
                } else {
                    st.palette_note = "Already in this palette (or it is full)".into();
                }
            }
            PaletteAct::Remove(i) => {
                st.palettes.remove_entry(cur, i);
                changed = true;
            }
        }
    }
    if changed {
        if let Err(e) = st.palettes.save() {
            ed.status = e;
        }
    }
}

enum PaletteAct {
    Select(usize),
    New,
    StartRename,
    Rename(String),
    CancelRename,
    Delete,
    Add,
    Remove(usize),
}

fn wheel_section(ui: &mut egui::Ui, ctx: &egui::Context, ed: &mut Editor, st: &mut UiState) {
    const SIZE: f32 = 150.0;
    let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    let mut picked = false;
    ui.vertical_centered(|ui| {
        let (rect, resp) = ui.allocate_exact_size(vec2(SIZE, SIZE), Sense::click_and_drag());
        st.layout.wheel = Some(rect);
        let tex = st.wheel_texture(ctx);
        let dim = (st.hsv[2] * 255.0).round() as u8;
        ui.painter().image(tex, rect, uv, Color32::from_gray(dim));
        let radius = SIZE * 0.5;
        if resp.is_pointer_button_down_on() || resp.dragged() || resp.clicked() {
            if let Some(p) = resp.interact_pointer_pos() {
                let c = rect.center();
                let (h, s) = wheel::wheel_to_hs((p.x - c.x) / radius, (p.y - c.y) / radius);
                st.hsv[0] = h;
                st.hsv[1] = s;
                picked = true;
            }
        }
        let (mx, my) = wheel::hs_to_wheel(st.hsv[0], st.hsv[1]);
        let at = rect.center() + vec2(mx, my) * radius;
        ui.painter().circle_stroke(at, 6.0, Stroke::new(2.5, Color32::BLACK));
        ui.painter().circle_stroke(at, 5.0, Stroke::new(1.5, Color32::WHITE));

        // Brightness: a bar from black to the full colour.
        let (bar, bresp) = ui.allocate_exact_size(vec2(SIZE, 16.0), Sense::click_and_drag());
        st.layout.value_bar = Some(bar);
        let full = wheel::hsv_to_rgb(st.hsv[0], st.hsv[1], 1.0);
        let right = Color32::from_rgb(full[0], full[1], full[2]);
        let mut mesh = egui::Mesh::default();
        mesh.colored_vertex(bar.left_top(), Color32::BLACK);
        mesh.colored_vertex(bar.right_top(), right);
        mesh.colored_vertex(bar.right_bottom(), right);
        mesh.colored_vertex(bar.left_bottom(), Color32::BLACK);
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(0, 2, 3);
        ui.painter().add(egui::Shape::mesh(mesh));
        ui.painter().rect_stroke(bar, 2.0, Stroke::new(1.0, Color32::from_black_alpha(140)), egui::StrokeKind::Inside);
        if bresp.is_pointer_button_down_on() || bresp.dragged() || bresp.clicked() {
            if let Some(p) = bresp.interact_pointer_pos() {
                st.hsv[2] = ((p.x - bar.left()) / bar.width()).clamp(0.0, 1.0);
                picked = true;
            }
        }
        let x = bar.left() + bar.width() * st.hsv[2];
        ui.painter().line_segment([egui::pos2(x, bar.top() - 1.0), egui::pos2(x, bar.bottom() + 1.0)], Stroke::new(3.0, Color32::BLACK));
        ui.painter().line_segment([egui::pos2(x, bar.top() - 1.0), egui::pos2(x, bar.bottom() + 1.0)], Stroke::new(1.5, Color32::WHITE));
    });
    if picked {
        let rgb = wheel::hsv_to_rgb(st.hsv[0], st.hsv[1], st.hsv[2]);
        st.hsv_rgb = rgb;
        ed.set_brush_rgb(rgb);
    }

    // Hex code.
    let hex_id = egui::Id::unique("hex-field");
    let typing = ui.memory(|m| m.has_focus(hex_id));
    let canonical = format!("#{:02x}{:02x}{:02x}", ed.brush_rgb[0], ed.brush_rgb[1], ed.brush_rgb[2]);
    if !typing && st.hex_text != canonical {
        st.hex_text = canonical;
    }
    ui.horizontal(|ui| {
        let (sw, _) = ui.allocate_exact_size(vec2(30.0, 22.0), Sense::hover());
        let c = ed.brush_rgb;
        ui.painter().rect_filled(sw, 3.0, Color32::from_rgb(c[0], c[1], c[2]));
        ui.painter().rect_stroke(sw, 3.0, Stroke::new(1.0, Color32::from_black_alpha(140)), egui::StrokeKind::Inside);
        let edit = ui.add(
            egui::TextEdit::singleline(&mut st.hex_text)
                .id(hex_id)
                .desired_width(74.0)
                .char_limit(7)
                .font(egui::TextStyle::Monospace),
        );
        st.layout.hex_field = Some(edit.rect);
        if edit.changed() {
            if let Some(rgb) = palette::parse_hex(&st.hex_text) {
                st.follow_brush(rgb);
                st.hsv_rgb = rgb;
                ed.set_brush_rgb(rgb);
            }
        }
        ui.label(RichText::new(format!("rgb {} {} {}", c[0], c[1], c[2])).weak().small());
    });
}

fn palette_section(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    ed: &mut Editor,
    st: &mut UiState,
    acts: &mut Vec<PaletteAct>,
) {
    ui.label(RichText::new("Palettes").strong())
        .on_hover_text("A palette entry remembers a colour and the texture that was selected with it");
    let names: Vec<String> = st.palettes.list().iter().map(|p| p.name.clone()).collect();
    let cur = st.palettes.current_index();
    egui::ComboBox::from_id_salt("palette_select")
        .selected_text(names[cur].clone())
        .width(200.0)
        .show_ui(ui, |ui| {
            for (i, n) in names.iter().enumerate() {
                if ui.selectable_label(i == cur, n).clicked() {
                    acts.push(PaletteAct::Select(i));
                }
            }
        });

    if let Some(buf) = st.renaming.as_mut() {
        let mut name = std::mem::take(buf);
        ui.horizontal(|ui| {
            let edit = ui.add(egui::TextEdit::singleline(&mut name).desired_width(130.0).char_limit(palette::MAX_NAME_CHARS));
            st.layout.palette_name_field = Some(edit.rect);
            if !edit.has_focus() && !edit.lost_focus() {
                edit.request_focus();
            }
            let ok = ui.button("OK");
            st.layout.palette_rename_ok = Some(ok.rect);
            let cancel = ui.button("Cancel");
            let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
            if ok.clicked() || enter {
                acts.push(PaletteAct::Rename(name.clone()));
            } else if cancel.clicked() || escape {
                acts.push(PaletteAct::CancelRename);
            }
        });
        if let Some(buf) = st.renaming.as_mut() {
            *buf = name;
        }
    } else {
        ui.horizontal(|ui| {
            let new = ui.small_button("New").on_hover_text("Start an empty palette");
            st.layout.palette_new = Some(new.rect);
            if new.clicked() {
                acts.push(PaletteAct::New);
            }
            let rename = ui.small_button("Rename");
            st.layout.palette_rename = Some(rename.rect);
            if rename.clicked() {
                acts.push(PaletteAct::StartRename);
            }
            let delete = ui.small_button("Delete").on_hover_text("Delete this palette (the last one is only emptied)");
            st.layout.palette_delete = Some(delete.rect);
            if delete.clicked() {
                acts.push(PaletteAct::Delete);
            }
        });
    }

    // Save the current colour and texture.
    ui.horizontal(|ui| {
        let tex = st.tile_texture(ctx, ed.texture, ed.brush_rgb);
        let (rect, _) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::hover());
        ui.painter().image(tex, rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
        let add = ui
            .button("Save current")
            .on_hover_text("Adds the current colour together with the selected texture to this palette");
        st.layout.palette_add = Some(add.rect);
        if add.clicked() {
            acts.push(PaletteAct::Add);
        }
    });
    if !st.palette_note.is_empty() {
        ui.label(RichText::new(st.palette_note.clone()).weak().small());
    }

    let entries = st.palettes.current().entries.clone();
    if entries.is_empty() {
        ui.label(RichText::new("Empty. Pick a colour and a texture, then Save current.").weak().small());
        return;
    }
    egui::ScrollArea::vertical().id_salt("palette_entries").max_height(150.0).auto_shrink([false, true]).show(ui, |ui| {
        egui::Grid::new("palette_grid").spacing([4.0, 4.0]).min_col_width(0.0).show(ui, |ui| {
            for (i, e) in entries.iter().enumerate() {
                let (rect, resp) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::click());
                st.layout.palette_entries.push(rect);
                let tex = st.tile_texture(ctx, e.texture, e.rgb);
                ui.painter().image(tex, rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
                let stroke = if ed.brush_rgb == e.rgb && ed.texture == e.texture {
                    Stroke::new(2.5, ui.visuals().selection.stroke.color)
                } else {
                    Stroke::new(1.0, Color32::from_black_alpha(120))
                };
                ui.painter().rect_stroke(rect, 3.0, stroke, egui::StrokeKind::Inside);
                let tip = format!("{}  {}\nClick to use, right click to remove", e.hex(), texture::name(e.texture));
                let resp = resp.on_hover_text(tip);
                if resp.clicked() {
                    ed.set_brush_rgb(e.rgb);
                    ed.set_texture(e.texture);
                }
                resp.context_menu(|ui| {
                    if ui.button("Remove from palette").clicked() {
                        acts.push(PaletteAct::Remove(i));
                        ui.close();
                    }
                });
                if (i + 1) % 8 == 0 {
                    ui.end_row();
                }
            }
        });
    });
}

/// `rect` shrunk to the shape of a picture of `aspect` (w, h), centred.
fn fit_rect(rect: egui::Rect, aspect: (f32, f32)) -> egui::Rect {
    let big = aspect.0.max(aspect.1).max(1.0);
    let size = vec2(rect.width() * aspect.0 / big, rect.height() * aspect.1 / big);
    egui::Rect::from_center_size(rect.center(), size)
}

/// Draws every visible node (icon and name) over the map.
fn nodes_layer(ctx: &egui::Context, ed: &mut Editor, st: &mut UiState) {
    let ppp = ctx.pixels_per_point();
    let over_ui = ctx.is_pointer_over_egui();
    if ed.placing_node && !over_ui {
        ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
    } else if !over_ui && ed.cursor_px.is_some_and(|(x, y)| ed.node_at(x, y).is_some()) {
        ctx.set_cursor_icon(if ed.node_dragging() {
            egui::CursorIcon::Grabbing
        } else {
            egui::CursorIcon::PointingHand
        });
    }
    if !ed.show_nodes && !ed.placing_node {
        return;
    }
    let avail = ctx.content_rect();
    let painter = ctx
        .layer_painter(egui::LayerId::new(egui::Order::Background, egui::Id::unique("nodes")))
        .with_clip_rect(avail);
    let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));

    if ed.show_nodes {
        let open = ed.open_node_id();
        let nodes: Vec<_> = ed.world.nodes().to_vec();
        for n in &nodes {
            let Some(s) = ed.node_screen(n) else { continue };
            let size = s.size as f32 / ppp;
            let centre = egui::pos2(s.centre.0 as f32 / ppp, s.centre.1 as f32 / ppp);
            // Skip nodes whose icon and label are entirely off screen.
            let reach = egui::Rect::from_center_size(centre, vec2(size + 400.0, size + 60.0));
            if !reach.intersects(avail) {
                continue;
            }
            let tex = st.icon_tex(ctx, &ed.world, &n.icon);
            let rect = fit_rect(egui::Rect::from_center_size(centre, vec2(size, size)), tex.aspect);
            painter.image(tex.id(size), rect, uv, Color32::WHITE);
            if open == Some(n.id) {
                painter.rect_stroke(
                    rect.expand(3.0),
                    4.0,
                    Stroke::new(2.0, Color32::from_rgb(120, 200, 255)),
                    egui::StrokeKind::Outside,
                );
            }
            if !n.name.is_empty() {
                let at = egui::pos2(centre.x, centre.y + size * 0.5 + 2.0);
                let font = egui::FontId::proportional(13.0);
                for (dx, dy) in [(-1.0, 0.0), (1.0, 0.0), (0.0, -1.0), (0.0, 1.0), (-1.0, -1.0), (1.0, 1.0)] {
                    painter.text(
                        at + vec2(dx, dy),
                        Align2::CENTER_TOP,
                        &n.name,
                        font.clone(),
                        Color32::from_black_alpha(230),
                    );
                }
                painter.text(at, Align2::CENTER_TOP, &n.name, font, Color32::WHITE);
            }
        }
    }

    // A faint copy of the icon follows the mouse while placing.
    if ed.placing_node && !over_ui {
        if let Some((x, y)) = ed.cursor_px {
            let tex = st.icon_tex(ctx, &ed.world, &ed.default_icon);
            let size = 32.0;
            let rect = fit_rect(
                egui::Rect::from_center_size(egui::pos2(x as f32 / ppp, y as f32 / ppp), vec2(size, size)),
                tex.aspect,
            );
            painter.image(tex.id(size), rect, uv, Color32::from_white_alpha(150));
        }
    }
}

fn node_popup(ctx: &egui::Context, ed: &mut Editor, st: &mut UiState) {
    st.layout.node_icons.clear();
    st.layout.node_done = None;
    st.layout.node_delete = None;
    st.layout.node_cancel = None;
    st.layout.node_import = None;
    let Some(id) = ed.open_node_id() else {
        st.focus_node = None;
        return;
    };
    let Some(mut node) = ed.world.node(id).cloned() else {
        ed.close_node(false);
        return;
    };
    if st.focus_node.is_none() && st.seen_icons_rev != ed.icons_revision {
        // Opening a node: a new one gets the keyboard in its name field.
        st.focus_node = (node.name.is_empty()).then_some(id);
    }
    st.refresh_icon_names(ed);

    let ppp = ctx.pixels_per_point();
    let screen = ctx.content_rect();
    let near = ed.node_screen(&node).map_or(screen.center(), |s| {
        egui::pos2(s.centre.0 as f32 / ppp + 40.0, s.centre.1 as f32 / ppp - 20.0)
    });
    let pos = egui::pos2(
        near.x.clamp(screen.left() + 10.0, (screen.right() - 330.0).max(10.0)),
        near.y.clamp(screen.top() + 40.0, (screen.bottom() - 420.0).max(40.0)),
    );

    let mut open = true;
    let (mut done, mut delete, mut cancel) = (false, false, false);
    let mut chosen: Option<Icon> = None;
    let mut import = false;
    let names = st.icon_names.clone();
    let mut thumbs: Vec<(Icon, IconTex)> = Vec::new();
    for i in 1..=BUILTIN_COUNT {
        let icon = Icon::Builtin(i);
        let t = st.icon_tex(ctx, &ed.world, &icon);
        thumbs.push((icon, t));
    }
    for name in &names {
        let icon = Icon::Custom(name.clone());
        // Icons from the folder that this map has not loaded yet are drawn
        // from the file; only chosen ones are stored in the map.
        let t = if ed.world.custom_icon(name).is_some() {
            st.icon_tex(ctx, &ed.world, &icon)
        } else {
            st.icon_tex_from_file(ctx, &ed.icons_dir, name)
        };
        thumbs.push((icon, t));
    }
    let focus = st.focus_node == Some(id);

    egui::Window::new("Node")
        .id(egui::Id::unique(("node-popup", id)))
        .default_pos(pos)
        .resizable(false)
        .collapsible(false)
        .open(&mut open)
        .show(ctx, |ui| {
            ui.set_width(300.0);
            ui.label(RichText::new("Name").strong());
            let name = ui.add(
                egui::TextEdit::singleline(&mut node.name)
                    .hint_text("Shown under the node")
                    .char_limit(MAX_NAME_CHARS)
                    .desired_width(f32::INFINITY),
            );
            if focus {
                name.request_focus();
            }
            ui.label(RichText::new("Description").strong());
            ui.add(
                egui::TextEdit::multiline(&mut node.description)
                    .hint_text("Notes about this place")
                    .char_limit(MAX_DESCRIPTION_CHARS)
                    .desired_rows(4)
                    .desired_width(f32::INFINITY),
            );

            ui.separator();
            ui.horizontal(|ui| {
                ui.label(RichText::new("Icon").strong());
                let label = match &node.icon {
                    Icon::Builtin(i) => icons::builtin_name(*i).to_string(),
                    Icon::Custom(n) => n.clone(),
                };
                ui.label(RichText::new(label).weak());
            });
            egui::ScrollArea::vertical().max_height(150.0).show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
                    for (icon, tex) in &thumbs {
                        let (rect, resp) = ui.allocate_exact_size(vec2(40.0, 40.0), Sense::click());
                        st.layout.node_icons.push(rect);
                        let selected = node.icon == *icon;
                        if selected {
                            ui.painter().rect_filled(rect, 4.0, ui.visuals().selection.bg_fill);
                        } else if resp.hovered() {
                            ui.painter().rect_filled(rect, 4.0, Color32::from_white_alpha(25));
                        }
                        ui.painter().image(
                            tex.id(32.0),
                            fit_rect(rect.shrink(4.0), tex.aspect),
                            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                            Color32::WHITE,
                        );
                        let tip = match icon {
                            Icon::Builtin(i) => icons::builtin_name(*i).to_string(),
                            Icon::Custom(n) => n.clone(),
                        };
                        if resp.on_hover_text(tip).clicked() {
                            chosen = Some(icon.clone());
                        }
                    }
                });
            });
            let imp = ui
                .button("Import PNG...")
                .on_hover_text(format!("Adds PNG files to {}", ed.icons_dir.display()));
            st.layout.node_import = Some(imp.rect);
            import = imp.clicked();

            ui.separator();
            ui.horizontal(|ui| {
                ui.label(RichText::new("Size").strong());
                ui.add(
                    egui::Slider::new(&mut node.scale, MIN_SCALE..=MAX_SCALE)
                        .logarithmic(true)
                        .custom_formatter(|v, _| format!("{v:.2}×")),
                )
                .on_hover_text("Icon size relative to one cell of this node's depth");
                if ui.small_button("1×").clicked() {
                    node.scale = 1.0;
                }
            });
            ui.label(
                RichText::new(format!("depth {}  ·  cell {}, {}", node.depth, node.x, node.y)).weak(),
            );

            ui.separator();
            ui.horizontal(|ui| {
                let d = ui.add_sized(
                    [90.0, 28.0],
                    egui::Button::new(RichText::new("Done").strong()).fill(ui.visuals().selection.bg_fill),
                );
                st.layout.node_done = Some(d.rect);
                done = d.clicked();
                let x = ui.add_sized([90.0, 28.0], egui::Button::new("Delete"));
                st.layout.node_delete = Some(x.rect);
                delete = x.clicked();
                let c = ui
                    .add_sized([90.0, 28.0], egui::Button::new("Cancel"))
                    .on_hover_text("Throw away what you changed in this popup");
                st.layout.node_cancel = Some(c.rect);
                cancel = c.clicked();
            });
        });

    if focus {
        st.focus_node = None;
    }
    if let Some(icon) = chosen {
        ed.choose_icon(icon);
        if let Some(n) = ed.world.node(id) {
            node.icon = n.icon.clone(); // a failed load keeps the old icon
        }
    }
    ed.update_open_node(node);
    if import {
        st.request(Command::ImportIcons, false);
    }
    if delete {
        ed.delete_open_node();
    } else if cancel {
        ed.close_node(false);
    } else if done || !open {
        ed.close_node(true);
    }
}

/// `1234567` -> `1,234,567`.
pub fn grouped(n: i64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn new_map_window(ctx: &egui::Context, st: &mut UiState) {
    st.layout.new_presets.clear();
    st.layout.new_create = None;
    st.layout.new_cancel = None;
    let Some(mut form) = st.new_map else { return };
    let mut create = false;
    let mut cancel = ctx.input(|i| i.key_pressed(egui::Key::Escape));
    let mut rects = Vec::new();
    let mut create_rect = None;
    let mut cancel_rect = None;

    egui::Window::new("New map")
        .order(egui::Order::Foreground)
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.set_width(360.0);
            ui.label(RichText::new("Map size").strong());
            ui.label(
                RichText::new("Cells across the whole map, as seen fully zoomed out.")
                    .weak()
                    .small(),
            );
            ui.horizontal(|ui| {
                for n in [32u32, 64, 128, 256, 512, 1024] {
                    let selected = form.width == n && form.height == n;
                    let r = ui.selectable_label(selected, format!("{n}"));
                    rects.push(r.rect);
                    if r.clicked() {
                        form.width = n;
                        form.height = n;
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.label("Width");
                ui.add(
                    egui::DragValue::new(&mut form.width)
                        .range(MIN_MAP_SIZE..=MAX_MAP_SIZE)
                        .speed(1.0),
                );
                ui.label("Height");
                ui.add(
                    egui::DragValue::new(&mut form.height)
                        .range(MIN_MAP_SIZE..=MAX_MAP_SIZE)
                        .speed(1.0),
                );
                ui.label("cells");
            });

            ui.separator();
            ui.label(RichText::new("Depth").strong());
            ui.label(
                RichText::new("How many times you can zoom into a cell to draw finer detail.")
                    .weak()
                    .small(),
            );
            let mut depths = form.depths;
            ui.spacing_mut().slider_width = 250.0;
            ui.add(egui::Slider::new(&mut depths, 0..=MAX_DEPTH));
            form.depths = depths;

            let spec = form.spec();
            if let Some((fw, fh)) = spec.finest_extent() {
                let per = depthmap_core::SUBDIV.pow(spec.max_depth as u32);
                ui.add_space(4.0);
                ui.label(
                    RichText::new(format!(
                        "Finest layer: {} × {} cells\n1 map cell = {} × {} cells at the deepest zoom",
                        grouped(fw),
                        grouped(fh),
                        grouped(per),
                        grouped(per)
                    ))
                    .weak(),
                );
            }

            ui.separator();
            ui.horizontal(|ui| {
                let c = ui.add_sized(
                    [140.0, 30.0],
                    egui::Button::new(RichText::new("Create").strong())
                        .fill(ui.visuals().selection.bg_fill),
                );
                create_rect = Some(c.rect);
                create = c.clicked();
                let x = ui.add_sized([100.0, 30.0], egui::Button::new("Cancel"));
                cancel_rect = Some(x.rect);
                cancel |= x.clicked();
            });
        });

    st.layout.new_presets = rects;
    st.layout.new_create = create_rect;
    st.layout.new_cancel = cancel_rect;
    if create {
        st.new_map = None;
        st.ready.push(Command::Create(form.spec()));
    } else if cancel {
        st.new_map = None;
    } else {
        st.new_map = Some(form);
    }
}

fn dialogs(ctx: &egui::Context, ed: &mut Editor, st: &mut UiState) {
    if let Some(cmd) = st.confirm.clone() {
        egui::Window::new("Unsaved changes")
            .order(egui::Order::Foreground)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(format!("\"{}\" has changes that are not saved.", ed.file_name()));
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui.button("Save").clicked() {
                        st.confirm = None;
                        if ed.save() {
                            st.dispatch(cmd.clone());
                        } else {
                            // Untitled: pick a file first; the user then retries.
                            st.ready.push(Command::SaveAs);
                        }
                    }
                    if ui.button("Discard").clicked() {
                        st.confirm = None;
                        st.dispatch(cmd);
                    }
                    if ui.button("Cancel").clicked() {
                        st.confirm = None;
                    }
                });
            });
    }

    if st.show_help {
        egui::Window::new("Controls")
            .open(&mut st.show_help)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                egui::Grid::new("help").striped(true).show(ui, |ui| {
                    for (k, v) in [
                        ("Left mouse", "Paint with the selected texture and color"),
                        ("Right mouse (drag)", "Draw the chosen shape and fill at the current depth"),
                        ("Shift + mouse", "Erase instead (also E to toggle the eraser)"),
                        ("B / G / M", "Brush / Fill / Select tool"),
                        ("Fill tool (click)", "Fill the connected area of identical cells (Shift erases it)"),
                        ("Select tool", "Drag a rectangle, or click to pick connected / same tiles; each new selection adds"),
                        ("Right mouse (Select)", "Invert the selection"),
                        ("Ctrl+A / Ctrl+D / Ctrl+I", "Select all / deselect / invert selection"),
                        ("C", "Show or hide the Colors window"),
                        ("Esc", "Cancel the shape / node placing / node popup, then clear the selection"),
                        ("N", "Add a node: then click the map"),
                        ("Click / drag a node", "Edit it / move it"),
                        ("Delete", "Delete the node whose popup is open"),
                        ("Middle mouse / Space + drag", "Pan"),
                        ("Mouse wheel", "Zoom (deeper zoom = finer layer)"),
                        ("WASD / arrow keys", "Pan (physical key positions)"),
                        ("1 - 9, 0", "Pick texture"),
                        ("[  ]", "Brush size"),
                        ("Ctrl+Z / Ctrl+Y", "Undo / redo"),
                        ("Ctrl+N / O / S", "New / open / save"),
                        ("Ctrl+Shift+S", "Save as"),
                    ] {
                        ui.label(RichText::new(k).strong());
                        ui.label(v);
                        ui.end_row();
                    }
                });
            });
    }

    if st.show_about {
        egui::Window::new("About depthmap")
            .open(&mut st.show_about)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label("Multi-depth map editor.");
                ui.label(format!(
                    "Zoom ×{} into any cell to draw finer detail on top of the layer you started with.",
                    depthmap_core::SUBDIV
                ));
                ui.label(RichText::new(format!("Zoom range: {} to {}", format_zoom(MIN_ZOOM_LEVEL), format_zoom(MAX_ZOOM_LEVEL))).weak());
            });
    }
}

/// egui rendering on top of wgpu.
pub struct Gui {
    pub ctx: egui::Context,
    pub state: UiState,
    renderer: egui_wgpu::Renderer,
}

impl Gui {
    pub fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let ctx = egui::Context::default();
        setup_style(&ctx);
        Self {
            ctx,
            state: UiState::new(),
            renderer: egui_wgpu::Renderer::new(device, target_format, Default::default()),
        }
    }

    /// Runs one UI frame.
    pub fn run(&mut self, input: egui::RawInput, ed: &mut Editor) -> egui::FullOutput {
        let Self { ctx, state, .. } = self;
        ctx.run_ui(input, |ui| draw(ui, ed, state))
    }

    /// True while the pointer is over any panel or window (so clicks there
    /// must not paint).
    pub fn pointer_over_ui(&self) -> bool {
        self.ctx.is_pointer_over_egui()
            || self.state.confirm_open()
            || self.state.startup_open()
            || self.state.new_map_open()
    }

    /// Draws the UI into `target` on top of what is already there.
    pub fn paint(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target: &wgpu::TextureView,
        size_in_pixels: [u32; 2],
        mut output: egui::FullOutput,
    ) {
        let pixels_per_point = output.pixels_per_point;
        let primitives = self.ctx.tessellate(std::mem::take(&mut output.shapes), pixels_per_point);
        // egui hands out texture updates exactly once; they must be consumed.
        let mut textures = std::mem::take(&mut output.textures_delta);
        for (id, deltas) in std::mem::take(&mut textures.set) {
            for delta in &deltas {
                self.renderer.update_texture(device, queue, id, delta);
            }
        }
        let screen = egui_wgpu::ScreenDescriptor { size_in_pixels, pixels_per_point };
        let mut encoder = device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("egui") });
        let extra = self
            .renderer
            .update_buffers(device, queue, &mut encoder, &primitives, &screen);
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("egui"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            self.renderer.render(&mut pass.forget_lifetime(), &primitives, &screen);
        }
        queue.submit(extra.into_iter().chain([encoder.finish()]));
        for id in &textures.free {
            self.renderer.free_texture(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opening_a_specific_map_also_asks_before_discarding_work() {
        let mut st = UiState::new();
        st.request(Command::OpenPath(PathBuf::from("/maps/a.dmap")), true);
        assert!(st.confirm_open());
        assert!(st.take_commands().is_empty());
    }

    #[test]
    fn new_map_with_unsaved_work_asks_first_then_opens_the_dialog() {
        let mut st = UiState::new();
        st.request(Command::New, true);
        assert!(st.confirm_open() && !st.new_map_open());
    }

    #[test]
    fn grouped_numbers() {
        assert_eq!(grouped(7), "7");
        assert_eq!(grouped(1234), "1,234");
        assert_eq!(grouped(4_398_046_511_104), "4,398,046,511,104");
    }

    #[test]
    fn snap_label_shows_the_cell_size_of_each_depth() {
        assert_eq!(snap_label(0), "1 cell");
        assert_eq!(snap_label(1), "1/4 cell");
        assert_eq!(snap_label(3), "1/64 cell");
    }

    #[test]
    fn destructive_commands_ask_first_only_when_there_are_unsaved_changes() {
        let mut st = UiState::new();
        st.request(Command::New, false);
        assert!(st.take_commands().is_empty(), "New opens the dialog instead");
        assert!(st.new_map_open() && !st.confirm_open());

        st.request(Command::Open, true);
        assert!(st.confirm_open());
        assert!(st.take_commands().is_empty(), "must wait for the answer");

        // Saving never needs confirmation.
        let mut st = UiState::new();
        st.request(Command::SaveAs, true);
        assert_eq!(st.take_commands(), vec![Command::SaveAs]);
    }
}
