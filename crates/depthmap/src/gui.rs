//! The egui user interface: menu bar, floating texture/colour panel on the
//! left, zoom panel on the right, and the dialogs. `draw` only touches the
//! [`Editor`] and [`UiState`], so it works with any egui backend (the app
//! uses egui-winit; the tests render it offscreen).

use std::path::PathBuf;

use depthmap_core::camera::{MAX_ZOOM_LEVEL, MIN_ZOOM_LEVEL};
use depthmap_core::spec::{MAX_MAP_SIZE, MIN_MAP_SIZE};
use depthmap_core::{Fill, MAX_DEPTH, MapSpec, Shape};
use depthmap_core::texture::{self, TEXTURE_COUNT, TILE};
use egui::{Align, Align2, Color32, Layout, RichText, Sense, Stroke, TextureOptions, vec2};

use crate::editor::{Editor, format_zoom};

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
            show_help: false,
            show_about: false,
            confirm: None,
            startup: false,
            new_map: None,
            recent: Vec::new(),
            ready: Vec::new(),
            thumbs: Vec::new(),
            thumbs_rgb: None,
        }
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
    dialogs(ctx, ed, st);
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
            });
            ui.menu_button("View", |ui| {
                ui.checkbox(&mut st.show_grid, "Show grid");
                ui.checkbox(&mut st.show_checker, "Checker overlay (darken every other cell)");
                ui.checkbox(&mut st.show_tools, "Textures & colors panel");
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
    st.layout.swatches.clear();
    st.layout.shapes.clear();
    st.layout.fills.clear();
    st.layout.border_toggle = None;
    st.layout.eraser_toggle = None;
    let max_height = (ctx.content_rect().height() - 70.0).max(240.0);
    egui::Window::new("Textures")
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
            ui.label(RichText::new("Color").strong());
            let swatch_columns = 8;
            let colors: Vec<[u8; 3]> = ed.world.colors.as_slice().to_vec();
            egui::ScrollArea::vertical()
                .max_height(116.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    egui::Grid::new("swatches").spacing([4.0, 4.0]).min_col_width(0.0).show(ui, |ui| {
                        for (i, rgb) in colors.iter().enumerate() {
                            let (rect, resp) =
                                ui.allocate_exact_size(vec2(22.0, 22.0), Sense::click());
                            ui.painter()
                                .rect_filled(rect, 3.0, Color32::from_rgb(rgb[0], rgb[1], rgb[2]));
                            let selected = ed.brush_rgb == *rgb;
                            st.layout.swatches.push(rect);
                            let stroke = if selected {
                                Stroke::new(2.5, ui.visuals().selection.stroke.color)
                            } else {
                                Stroke::new(1.0, Color32::from_black_alpha(120))
                            };
                            ui.painter()
                                .rect_stroke(rect, 3.0, stroke, egui::StrokeKind::Inside);
                            if resp
                                .on_hover_text(format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2]))
                                .clicked()
                            {
                                ed.set_brush_rgb(*rgb);
                            }
                            if (i + 1) % swatch_columns == 0 {
                                ui.end_row();
                            }
                        }
                    });
                });
            ui.horizontal(|ui| {
                let mut rgb = ed.brush_rgb;
                if ui.color_edit_button_srgb(&mut rgb).changed() {
                    ed.set_brush_rgb(rgb);
                }
                ui.label(format!(
                    "Any color  #{:02x}{:02x}{:02x}",
                    ed.brush_rgb[0], ed.brush_rgb[1], ed.brush_rgb[2]
                ));
            });

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

/// `1234567` -> `1,234,567`.
fn grouped(n: i64) -> String {
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
                        ("Esc", "Cancel the shape being dragged"),
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
