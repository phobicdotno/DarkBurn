//! The window: application bar with the menus, ribbon, tools column, dock panels, cut palette,
//! status bar and the canvas in the middle. Laid out after SolidCraft's shell.

use std::path::PathBuf;

use darkburn_ui::ui_kit::{self, BackingTooltipExt as _, Container, Edge, Menu, MenuItem, Surface};
use darkburn_ui::{fonts, icons, titlebar};
use egui::{Align2, FontId, Pos2, Rect, Sense, Stroke, Ui, pos2, vec2};
use solidcraft_theme::Tokens;

use crate::canvas::{self, ViewCmd};
use crate::commands::{self, Entry};
use crate::panels::{self, Area, PANELS};
use crate::prefs::{Prefs, TOOLBARS};
use crate::ribbon;
use crate::tools::Tool;
use darkburn_core::Editor;

pub const APP_BAR_HEIGHT: f32 = 34.0;
pub const STATUS_BAR_HEIGHT: f32 = 24.0;
pub const PALETTE_HEIGHT: f32 = 30.0;
pub const TOOLS_WIDTH: f32 = 40.0;
const PANEL_HEADER: f32 = 30.0;

/// Everything the window shows that is not a preference.
pub struct DarkBurn {
    pub prefs: Prefs,
    /// The pan tool is on: a left drag pans the canvas.
    pub pan_tool: bool,
    /// The pointer on the canvas, in machine millimetres.
    pub cursor: Option<Pos2>,
    pub view_cmd: Option<ViewCmd>,
    pub about_open: bool,
    pub settings_open: bool,
    /// The theme last applied: (preference, system is dark).
    theme_applied: Option<(String, bool)>,
    pub settings_dir: Option<PathBuf>,
    pub safe_mode: bool,
    /// A note for the status bar (settings moved aside, safe mode, …).
    pub note: Option<String>,
    /// No OS frame: the application bar is the title bar.
    pub custom_titlebar: bool,
    /// Windows answers the resize border itself (native_frame.rs); else egui edge zones.
    pub native_resize_border: bool,
    /// The open project.
    pub editor: Editor,
    /// Where the open project is saved (None: not saved yet).
    pub project_path: Option<PathBuf>,
    pub tool: Tool,
    /// Status bar locks: move, size, rotate, shear.
    pub locks: [bool; 4],
    /// Help text for the control under the pointer, set each frame by whoever draws it.
    pub hint: Option<String>,
    /// Auto-save state for the status bar.
    pub autosave_status: String,
    /// A copy was made: put it on the system clipboard too.
    pub clipboard_out: bool,
    /// Where the canvas context menu was opened (document mm), for paste at cursor.
    pub context_point: Option<darkburn_core::kurbo::Point>,
    /// Padding for distribute and docking, in mm.
    pub padding: f64,
    /// The layer whose cut settings are being edited, if the editor is open.
    pub cut_settings_open: Option<usize>,
    pub notes_open: bool,
    /// A layer whose shapes flash on the canvas, and when the flash began (egui time, seconds).
    pub flash_layer: Option<(usize, f64)>,
    /// Project files, auto-save and the system clipboard (files.rs).
    pub files: crate::files::FileState,
    /// Devices, the connected machine and its panels (machine_ui.rs).
    pub machine: crate::machine_ui::MachineUi,
    /// Libraries, hotkey editor, help (extras.rs).
    pub extras: crate::extras::Extras,
    /// Job, preview, G-code and material test state (job_cmds.rs).
    pub job: crate::job_cmds::JobState,
    /// Image windows and presets (image_ui.rs).
    pub images: crate::image_ui::ImageUi,
    /// Dialogs and ribbon fields of the shape-editing commands (shape_ui.rs).
    pub shape_ui: crate::shape_ui::ShapeUi,
    /// Opening, importing and exporting other file types (file_io.rs).
    pub fio: crate::file_io::FileIo,
    /// The text tool, text dialogs and the Variable Text panel (text_ui.rs).
    pub text: crate::text_ui::TextUi,
    /// Camera, rotary, cylinder, print and cut and the test tools (vision_cmds.rs).
    pub vision: crate::vision_cmds::Vision,
}

impl DarkBurn {
    pub fn new(prefs: Prefs) -> Self {
        DarkBurn {
            prefs,
            pan_tool: false,
            cursor: None,
            view_cmd: None,
            about_open: false,
            settings_open: false,
            theme_applied: None,
            settings_dir: None,
            safe_mode: false,
            note: None,
            custom_titlebar: true,
            native_resize_border: false,
            editor: Editor::new(),
            project_path: None,
            tool: Tool::Select,
            locks: [false; 4],
            hint: None,
            autosave_status: "Auto-save off".into(),
            clipboard_out: false,
            context_point: None,
            padding: 2.0,
            cut_settings_open: None,
            notes_open: false,
            flash_layer: None,
            files: Default::default(),
            machine: Default::default(),
            extras: Default::default(),
            job: Default::default(),
            images: Default::default(),
            shape_ui: Default::default(),
            fio: Default::default(),
            text: Default::default(),
            vision: Default::default(),
        }
    }

    /// Pick a drawing tool (the pan tool goes off).
    pub fn set_tool(&mut self, tool: Tool) {
        self.pan_tool = false;
        self.tool = tool;
    }

    /// Whether command `id` has something to act on now (menus grey out the rest).
    pub fn can_run(&self, id: &str) -> bool {
        let sel = self.editor.selection.len();
        if let Some(can) = crate::shape_ops::can_run(self, id) {
            return can;
        }
        if let Some(can) = crate::text_cmds::can_run(self, id) {
            return can;
        }
        match id {
            "edit.undo" => self.editor.history.can_undo(),
            "edit.redo" => self.editor.history.can_redo(),
            "edit.cut"
            | "edit.copy"
            | "edit.duplicate"
            | "edit.delete"
            | "arrange.flip_h"
            | "arrange.flip_v"
            | "arrange.rotate_cw"
            | "arrange.rotate_ccw"
            | "arrange.lock"
            | "arrange.forward"
            | "arrange.backward"
            | "arrange.front"
            | "arrange.back"
            | "view.frame_selection"
            | "edit.select_contained"
            | "arrange.ungroup" => sel > 0,
            "arrange.group" | "arrange.same_width" | "arrange.same_height" => sel > 1,
            "edit.paste" | "edit.paste_in_place" => true,
            _ if crate::image_ui::handles(id) => crate::image_ui::can_run(self, id),
            _ if id.starts_with("arrange.align_") || id.starts_with("arrange.distribute_") || id.starts_with("arrange.move_") => sel > 1,
            _ if id.starts_with("arrange.page_") => sel > 0,
            _ => true,
        }
    }

    /// Set the theme from the preference when it (or the system's light/dark choice) changed.
    pub fn apply_theme(&mut self, ctx: &egui::Context) {
        let system_dark = ctx.system_theme().is_none_or(|t| t == egui::Theme::Dark);
        let key = (self.prefs.theme.clone(), system_dark);
        if self.theme_applied.as_ref() == Some(&key) {
            return;
        }
        let file = darkburn_ui::theme_files::builtin().iter().find(|f| f.id == self.prefs.theme);
        let dark = match (self.prefs.theme.as_str(), file) {
            ("dark", _) => true,
            ("light", _) => false,
            (_, Some(f)) => f.base.is_dark(),
            _ => system_dark,
        };
        let custom = file.map(|f| solidcraft_theme::Custom { dark, tokens: darkburn_ui::theme_files::resolve(f) });
        let icon_palette = file.and_then(|f| darkburn_ui::theme_files::resolve_icon_palette(f, &Default::default()));
        solidcraft_theme::set_custom(custom);
        solidcraft_theme::set_dark(dark);
        darkburn_ui::icon_svg::set_theme_palette(icon_palette.map(|p| (dark, p)));
        solidcraft_theme::apply(ctx);
        icons::clear_texture_cache(ctx);
        self.theme_applied = Some(key);
    }

    #[allow(dead_code, reason = "called by the MCP backend, wired in the next step")]
    /// Run registered command `id` with JSON arguments, as an agent or script does.
    pub fn execute(&mut self, ctx: &egui::Context, id: &str, args: &serde_json::Value) -> Result<serde_json::Value, String> {
        crate::api::execute(self, ctx, id, args)
    }

    /// Run command `id` (menus, ribbon, shortcuts). Commands of later phases do nothing.
    pub fn run(&mut self, ctx: &egui::Context, id: &str) {
        if crate::actions::run(self, ctx, id)
            || crate::files::run(self, ctx, id)
            || crate::machine_ui::run(self, ctx, id)
            || crate::laser_position::run(self, ctx, id)
            || crate::job_cmds::run(self, ctx, id)
            || crate::image_ui::run(self, ctx, id)
            || crate::shape_ops::run(self, ctx, id)
            || crate::file_io::run(self, ctx, id)
            || crate::text_cmds::run(self, ctx, id)
            || crate::vision_ui::run(self, ctx, id)
            || crate::extras::run(self, ctx, id)
        {
            return;
        }
        match id {
            "file.exit" => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            "file.prefs_folder" => {
                if let Some(dir) = &self.settings_dir {
                    let _ = std::fs::create_dir_all(dir);
                    open_folder(dir);
                }
            }
            "edit.settings" => self.settings_open = true,
            "help.about" => self.about_open = true,
            "view.pan" => self.pan_tool = !self.pan_tool,
            "view.zoom_in" => self.view_cmd = Some(ViewCmd::ZoomIn),
            "view.zoom_out" => self.view_cmd = Some(ViewCmd::ZoomOut),
            "view.zoom_page" => self.view_cmd = Some(ViewCmd::ZoomPage),
            "window.reset_layout" => {
                self.prefs.reset_layout();
                ctx.memory_mut(|m| m.reset_areas());
                self.view_cmd = Some(ViewCmd::ZoomPage);
            }
            other => {
                if let Some(panel) = other.strip_prefix("panel.") {
                    if let Some(state) = self.prefs.panels.get_mut(panel) {
                        state.visible = !state.visible;
                        if state.visible {
                            self.prefs.active_tab.insert(state.area, panel.to_string());
                        }
                    }
                } else if let Some(bar) = other.strip_prefix("toolbar.") {
                    if bar == "status" {
                        self.prefs.status_bar = !self.prefs.status_bar;
                    } else {
                        let on = self.prefs.toolbar(bar);
                        self.prefs.toolbars.insert(bar.to_string(), !on);
                    }
                } else if let Some(lang) = other.strip_prefix("lang.") {
                    self.prefs.language = lang.to_string();
                }
            }
        }
    }

    /// Keyboard shortcuts of the commands that work (from the keys their menu items show).
    pub fn shortcuts(&mut self, ctx: &egui::Context) {
        use egui::{Key, KeyboardShortcut, Modifiers};
        if ctx.memory(|m| m.focused().is_some()) || self.extras.capturing() {
            return;
        }
        let mut run = None;
        for (shortcut, id) in crate::keymap::table(&self.prefs.hotkeys) {
            // In the node tool the plain letter keys are node commands, not align commands.
            if self.tool == Tool::Nodes && crate::node_tool::owns_shortcut(&shortcut) {
                continue;
            }
            if commands::works(id) && ctx.input_mut(|i| i.consume_shortcut(&shortcut)) {
                run = Some(id);
                break;
            }
        }
        let pressed = |m: Modifiers, k: Key| ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(m, k)));
        if run.is_none() {
            if pressed(Modifiers::COMMAND, Key::Plus) || pressed(Modifiers::NONE, Key::Plus) || pressed(Modifiers::NONE, Key::Equals) {
                run = Some("view.zoom_in");
            } else if pressed(Modifiers::NONE, Key::Minus) {
                run = Some("view.zoom_out");
            } else if self.tool != Tool::Nodes && pressed(Modifiers::NONE, Key::Backspace) {
                run = Some("edit.delete");
            }
        }
        if let Some(id) = run {
            self.run(ctx, id);
        }
    }

    /// The whole window for one frame.
    pub fn ui(&mut self, ui: &mut Ui) {
        let mut clicked: Option<String> = None;
        crate::extras::frame(ui.ctx(), self);
        // The canvas leaves its tool hint for the status bar, drawn before it next frame.
        self.hint = crate::canvas_input::hover_hint(ui.ctx());
        crate::files::frame(ui.ctx(), self);
        crate::machine_ui::frame(ui.ctx(), self);
        crate::file_io::frame(ui.ctx(), self);
        self.app_bar(ui, &mut clicked);
        ribbon::show(ui, self, &mut clicked);
        if self.prefs.status_bar {
            self.status_bar(ui);
        }
        if self.prefs.toolbar("palette") {
            crate::palette::show(ui, self);
        }
        self.dock(ui, Area::Bottom);
        if self.prefs.toolbar("tools") {
            tools_column(ui, self.pan_tool, self.tool, &mut self.hint, &mut clicked);
        }
        self.dock(ui, Area::Left);
        self.right_docks(ui);
        let cmd = self.view_cmd.take();
        let frame = egui::Frame::NONE.fill(Tokens::get().viewport_bottom);
        let mut cursor = None;
        Container::new(ui.ctx(), Surface::Viewport).central().frame(frame).show(ui, |ui| {
            cursor = canvas::show(ui, self, cmd).cursor;
        });
        self.cursor = cursor;
        self.dialogs(ui.ctx());
        crate::extras::dialogs(ui.ctx(), self);
        crate::cut_settings::dialog(ui.ctx(), self);
        crate::files::dialogs(ui.ctx(), self);
        crate::devices_ui::dialogs(ui.ctx(), self);
        crate::job_ui::frame(ui.ctx(), self);
        crate::image_ui::dialogs(ui.ctx(), self);
        crate::shape_ui::show(ui.ctx(), self);
        crate::text_ui::show(ui.ctx(), self);
        crate::file_io::dialogs(ui.ctx(), self);
        crate::vision_ui::frame(ui.ctx(), self);
        if self.custom_titlebar && !self.native_resize_border {
            titlebar::resize_zones(ui);
        }
        if let Some(id) = clicked {
            self.run(&ui.ctx().clone(), &id);
        }
    }

    fn app_bar(&mut self, ui: &mut Ui, clicked: &mut Option<String>) {
        let t = Tokens::get();
        let host = Container::new(ui.ctx(), Surface::ApplicationBar);
        let panel = host.edge("db_appbar", Edge::Top).exact_size(APP_BAR_HEIGHT).show_separator_line(false);
        let frame = host.edge_frame(t.app_bar);
        let custom = self.custom_titlebar;
        host.show_edge(ui, panel, frame, |ui| {
            let r = ui.max_rect();
            if custom {
                titlebar::drag_area(ui, r);
            }
            let left = r.left();
            icons::paint_on(ui.painter(), Rect::from_center_size(pos2(left + 20.0, r.center().y), vec2(22.0, 22.0)), "db_app", Some(true), 1.0);
            ui.painter().text(pos2(left + 36.0, r.center().y), Align2::LEFT_CENTER, "DarkBurn", fonts::semibold(t.font_heading), t.app_bar_text);
            let mut x = left + 118.0;
            separator(ui, r, x - 10.0);
            for (name, entries) in commands::MENUS {
                let galley =
                    ui.painter().layout_no_wrap(crate::i18n::tr(&format!("menu.{name}"), name), FontId::proportional(t.font_button), t.app_bar_text);
                let br = Rect::from_min_size(pos2(x, r.center().y - 13.0), vec2(galley.size().x + 16.0, 26.0));
                x = br.right() + 2.0;
                let id = ui.scope_id().with(("menu", *name));
                let open_id = id.with("popup");
                Menu::button_with(
                    ui,
                    |ui, _| {
                        let resp = ui.interact(br, id, Sense::click());
                        let open = egui::Popup::is_id_open(ui.ctx(), open_id);
                        if resp.hovered() || open {
                            ui.painter().rect_filled(br, t.radius_sm, t.app_bar_hover);
                        }
                        ui.painter().galley(pos2(br.left() + 8.0, br.center().y - galley.size().y / 2.0), galley.clone(), t.app_bar_text);
                        solidcraft_ui_registry::publish_target(&format!("appbar:menu:{name}"), br);
                        resp
                    },
                    |ui| {
                        ui.set_min_width(ui_kit::LIST_W);
                        self.menu_entries(ui, entries, clicked);
                    },
                );
            }
            separator(ui, r, x + 8.0);
            x += 26.0;
            for (icon, tip) in [("save", "Save (Ctrl+S)"), ("undo", "Undo (Ctrl+Z)"), ("redo", "Redo (Ctrl+Shift+Z)")] {
                // Too narrow: these give way before they run into the settings and help buttons.
                let right_cluster = r.right() - if custom { titlebar::WIDTH } else { 0.0 } - 22.0 - 2.0 * 34.0 - 8.0;
                if x + 18.0 > right_cluster {
                    break;
                }
                let br = Rect::from_center_size(pos2(x, r.center().y), vec2(32.0, 32.0));
                let resp = ui.interact(br, ui.scope_id().with(("ab", icon)), Sense::hover());
                icons::paint_app_bar(ui.painter(), Rect::from_center_size(br.center(), vec2(18.0, 18.0)), icon, 0.35);
                resp.backing_on_hover_text(tip);
                x += 34.0;
            }
            // Right-hand cluster: version, settings, help, then the caption buttons.
            let captions = if custom { titlebar::WIDTH } else { 0.0 };
            let mut rx = r.right() - captions - 22.0;
            for (icon, tip, id) in [("help", "About DarkBurn", "help.about"), ("settings", "Settings", "edit.settings")] {
                let br = Rect::from_center_size(pos2(rx, r.center().y), vec2(32.0, 32.0));
                let resp = ui.interact(br, ui.scope_id().with(("ab", icon)), Sense::click());
                if resp.hovered() {
                    ui.painter().rect_filled(br.shrink(3.0), t.radius_sm, t.app_bar_hover_button);
                }
                icons::paint_app_bar(ui.painter(), Rect::from_center_size(br.center(), vec2(18.0, 18.0)), icon, 1.0);
                solidcraft_ui_registry::publish_target(&format!("appbar:{icon}"), br);
                if resp.backing_on_hover_text(tip).clicked() {
                    *clicked = Some(id.to_string());
                }
                rx -= 34.0;
            }
            let version = format!("v{}", env!("CARGO_PKG_VERSION"));
            // The version gives way when the window is too narrow for it beside the menus.
            let version_room = (rx - x) > 120.0;
            if version_room {
                ui.painter().text(
                    pos2(rx + 4.0, r.center().y),
                    Align2::RIGHT_CENTER,
                    version,
                    FontId::proportional(t.font_small),
                    t.app_bar_text.gamma_multiply(0.7),
                );
            }
            if custom {
                titlebar::caption_buttons(ui, r);
            }
        });
    }

    fn menu_entries(&self, ui: &mut Ui, entries: &[Entry], clicked: &mut Option<String>) {
        for entry in entries {
            match *entry {
                Entry::Sep => Menu::separator(ui),
                Entry::Cmd(id) => {
                    let Some(c) = commands::find(id) else {
                        continue;
                    };
                    let label = crate::i18n::cmd_label(c);
                    let key = crate::keymap::key_for(c);
                    let mut item = MenuItem::new(c.id, &label, c.icon);
                    item.shortcut = &key;
                    item.enabled = commands::works(id) && self.can_run(id);
                    if id == "view.pan" {
                        item.checked = Some(self.pan_tool);
                    }
                    if item.show(ui).clicked() && item.enabled {
                        *clicked = Some(id.to_string());
                    }
                }
                Entry::Sub(label, inner) => {
                    let shown = crate::i18n::tr(&format!("menu.{label}"), label);
                    let mut row = MenuItem::new(label, &shown, "");
                    row.submenu = true;
                    Menu::button_with(
                        ui,
                        |ui, _| row.show(ui),
                        |ui| {
                            ui.set_min_width(ui_kit::LIST_W);
                            self.menu_entries(ui, inner, clicked);
                        },
                    );
                }
                Entry::Dynamic("recent") => {
                    let recent = self.prefs.recent.clone();
                    let recent_text = crate::i18n::tr("menu.recent", "Recent Projects");
                    let mut row = MenuItem::new("file.recent", &recent_text, "");
                    row.submenu = true;
                    row.enabled = !recent.is_empty();
                    if recent.is_empty() {
                        row.show(ui);
                    } else {
                        Menu::button_with(
                            ui,
                            |ui, _| row.show(ui),
                            |ui| {
                                ui.set_min_width(ui_kit::LIST_W * 1.6);
                                for (i, path) in recent.iter().enumerate() {
                                    let id = format!("file.recent.{i}");
                                    if MenuItem::new(&id, path, "").show(ui).clicked() {
                                        *clicked = Some(id);
                                    }
                                }
                            },
                        );
                    }
                }
                Entry::Dynamic("panels") => {
                    Menu::heading(ui, &crate::i18n::tr("menu.panels", "Panels"));
                    for p in PANELS {
                        let id = format!("panel.{}", p.id);
                        let title = crate::i18n::panel_title(p.id, p.title);
                        let mut item = MenuItem::new(&id, &title, "");
                        item.checked = Some(self.prefs.panel(p.id).visible);
                        if item.show(ui).clicked() {
                            *clicked = Some(id.clone());
                        }
                    }
                }
                Entry::Dynamic("toolbars") => {
                    Menu::heading(ui, &crate::i18n::tr("menu.toolbars", "Toolbars"));
                    for (bar, label) in TOOLBARS.iter().copied().chain([("status", "Status Bar")]) {
                        let id = format!("toolbar.{bar}");
                        let label = crate::i18n::tr(&format!("toolbar.{bar}"), label);
                        let mut item = MenuItem::new(&id, &label, "");
                        item.checked = Some(if bar == "status" { self.prefs.status_bar } else { self.prefs.toolbar(bar) });
                        if item.show(ui).clicked() {
                            *clicked = Some(id.clone());
                        }
                    }
                }
                Entry::Dynamic("languages") => {
                    for (code, name) in commands::LANGUAGES {
                        let id = format!("lang.{code}");
                        let mut item = MenuItem::new(&id, name, "");
                        item.checked = Some(self.prefs.language == *code);
                        item.enabled = crate::i18n::available(code);
                        if item.show(ui).clicked() && item.enabled {
                            *clicked = Some(id.clone());
                        }
                    }
                }
                Entry::Dynamic(_) => {}
            }
        }
    }

    fn status_bar(&mut self, ui: &mut Ui) {
        let t = Tokens::get();
        let host = Container::new(ui.ctx(), Surface::ApplicationBar);
        let panel = host.edge("db_status", Edge::Bottom).exact_size(STATUS_BAR_HEIGHT).show_separator_line(false);
        let frame = host.edge_frame(t.app_bar);
        let selection = self.editor.doc.bbox_of(&self.editor.selected()).map(|r| (r, self.editor.selection.len()));
        host.show_edge(ui, panel, frame, |ui| {
            let r = ui.max_rect();
            let font = FontId::proportional(t.font_small);
            let ink = t.app_bar_text;
            let dim = t.app_bar_text.gamma_multiply(0.45);
            let mut x = r.left() + 10.0;
            for (i, lock) in ["Move", "Size", "Rotate", "Shear"].into_iter().enumerate() {
                let on = self.locks[i];
                let g = ui.painter().layout_no_wrap(lock.to_string(), font.clone(), if on { t.app_bar_text } else { dim });
                let br = Rect::from_min_size(pos2(x, r.top() + 3.0), vec2(g.size().x + 12.0, r.height() - 6.0));
                let resp = ui.interact(br, ui.scope_id().with(("lock", lock)), Sense::click());
                if on {
                    ui.painter().rect(br, t.radius_sm, t.accent_soft, Stroke::new(1.0, t.accent), egui::StrokeKind::Inside);
                } else {
                    ui.painter().rect_stroke(br, t.radius_sm, Stroke::new(1.0, dim), egui::StrokeKind::Inside);
                }
                ui.painter().galley(pos2(br.left() + 6.0, br.center().y - g.size().y / 2.0), g, if on { t.selected_ink } else { dim });
                solidcraft_ui_registry::publish_target(&format!("status:lock:{}", lock.to_lowercase()), br);
                let tip = format!("{lock} lock: when on, the selection cannot be {}", ["moved", "resized", "rotated", "sheared"][i]);
                if resp.hovered() {
                    self.hint = Some(tip.clone());
                }
                if resp.backing_on_hover_text(tip).clicked() {
                    self.locks[i] = !on;
                }
                x = br.right() + 4.0;
            }
            x += 12.0;
            let inches = self.prefs.numeric_inches;
            let position = match self.cursor {
                Some(p) if inches => format!("x: {:.3}  y: {:.3} in", p.x / 25.4, p.y / 25.4),
                Some(p) => format!("x: {:.2}  y: {:.2} mm", p.x, p.y),
                None => "x: –  y: –".to_string(),
            };
            let g = ui.painter().layout_no_wrap(position, FontId::monospace(t.font_small), ink);
            ui.painter().galley(pos2(x, r.center().y - g.size().y / 2.0), g, ink);
            solidcraft_ui_registry::publish_rect("status:cursor", Rect::from_min_size(pos2(x, r.top()), vec2(220.0, r.height())));
            x += 230.0;
            let u = if inches { 25.4 } else { 1.0 };
            let (dec, unit) = if inches { (3, "in") } else { (2, "mm") };
            let sel_text = match selection {
                Some((b, n)) => format!(
                    "{} {:.dec$}, {:.dec$}  {} {:.dec$}, {:.dec$}  ({:.dec$} × {:.dec$} {unit})  {n} {}",
                    crate::i18n::tr("status.min", "Min"),
                    b.x0 / u,
                    b.y0 / u,
                    crate::i18n::tr("status.max", "Max"),
                    b.x1 / u,
                    b.y1 / u,
                    b.width() / u,
                    b.height() / u,
                    if n == 1 { crate::i18n::tr("status.object", "object") } else { crate::i18n::tr("status.objects", "objects") },
                    dec = dec
                ),
                None => crate::i18n::tr("status.no_selection", "No selection"),
            };
            let g = ui.painter().layout_no_wrap(sel_text, FontId::monospace(t.font_small), if selection.is_some() { ink } else { dim });
            let w = g.size().x;
            ui.painter().galley(pos2(x, r.center().y - g.size().y / 2.0), g, ink);
            solidcraft_ui_registry::publish_rect("status:selection", Rect::from_min_size(pos2(x, r.top()), vec2(w, r.height())));
            x += w.max(100.0) + 16.0;
            let tip = self
                .note
                .clone()
                .or_else(|| self.hint.clone())
                .unwrap_or_else(|| if self.pan_tool { "Drag to pan the view".into() } else { String::new() });
            let right_edge = crate::vision_ui::status_chips(ui, r, self);
            let right_edge = crate::mcp_ui::status_chip(ui, r, right_edge, self);
            let clip = ui.painter().with_clip_rect(Rect::from_min_max(pos2(x, r.top()), pos2(right_edge, r.bottom())));
            clip.text(pos2(x, r.center().y), Align2::LEFT_CENTER, tip, font.clone(), ink);
            let right = format!("{:.0} %", self.prefs.view.percent());
            ui.painter().text(pos2(r.right() - 10.0, r.center().y), Align2::RIGHT_CENTER, &self.autosave_status, font.clone(), dim);
            ui.painter().text(pos2(r.right() - 130.0, r.center().y), Align2::RIGHT_CENTER, right, font, ink);
        });
    }

    fn visible_in(&self, area: Area) -> Vec<&'static panels::PanelDef> {
        PANELS.iter().filter(|p| self.prefs.panel(p.id).visible && self.prefs.panel(p.id).area == area).collect()
    }

    fn dock(&mut self, ui: &mut Ui, area: Area) {
        if self.visible_in(area).is_empty() {
            return;
        }
        let t = Tokens::get();
        let host = Container::new(ui.ctx(), Surface::Panel);
        let (edge, salt, size, range) = match area {
            Area::Left => (Edge::Left, "db_dock_left", 260.0, 180.0..=520.0),
            Area::Bottom => (Edge::Bottom, "db_dock_bottom", 180.0, 100.0..=480.0),
            _ => (Edge::Right, "db_dock_right", 380.0, 300.0..=640.0),
        };
        let panel = host.edge(salt, edge).resizable(true).default_size(size).size_range(range);
        let frame = host.edge_frame(t.panel);
        host.show_edge(ui, panel, frame, |ui| self.tab_group(ui, area));
    }

    fn right_docks(&mut self, ui: &mut Ui) {
        let top = !self.visible_in(Area::RightTop).is_empty();
        let bottom = !self.visible_in(Area::RightBottom).is_empty();
        if !top && !bottom {
            return;
        }
        let t = Tokens::get();
        let host = Container::new(ui.ctx(), Surface::Panel);
        let panel = host.edge("db_dock_right", Edge::Right).resizable(true).default_size(380.0).size_range(300.0..=640.0);
        let frame = host.edge_frame(t.panel);
        host.show_edge(ui, panel, frame, |ui| {
            let r = ui.max_rect();
            match (top, bottom) {
                (true, true) => {
                    let split = r.top() + (r.height() * 0.58).round();
                    let upper = Rect::from_min_max(r.min, pos2(r.right(), split));
                    let lower = Rect::from_min_max(pos2(r.left(), split + 1.0), r.max);
                    ui.scope_builder(egui::UiBuilder::new().max_rect(upper), |ui| self.tab_group(ui, Area::RightTop));
                    ui.painter().hline(r.x_range(), split + 0.5, Stroke::new(1.0, t.border));
                    ui.scope_builder(egui::UiBuilder::new().max_rect(lower), |ui| self.tab_group(ui, Area::RightBottom));
                }
                (true, false) => self.tab_group(ui, Area::RightTop),
                _ => self.tab_group(ui, Area::RightBottom),
            }
        });
    }

    /// One area's tabs and the active panel's contents.
    fn tab_group(&mut self, ui: &mut Ui, area: Area) {
        let t = Tokens::get();
        let shown = self.visible_in(area);
        let Some(first) = shown.first() else { return };
        let active = self.prefs.active_tab.get(&area).filter(|id| shown.iter().any(|p| p.id == id.as_str())).cloned().unwrap_or(first.id.to_string());
        let r = ui.max_rect();
        let header = Rect::from_min_size(r.min, vec2(r.width(), PANEL_HEADER));
        ui.painter().rect_filled(header, 0.0, t.panel_header);
        ui.painter().hline(header.x_range(), header.bottom() - 0.5, Stroke::new(1.0, t.border));
        let mut x = header.left() + 4.0;
        let mut pick = None;
        let mut action: Option<(String, Option<Area>)> = None;
        let close = Rect::from_center_size(pos2(header.right() - 15.0, header.center().y), vec2(22.0, 22.0));
        let mut tabs_clip = Rect::from_min_max(header.min, pos2(close.left() - 2.0, header.bottom()));
        if ui.rect_contains_pointer(r) {
            self.extras.hover_topic = Some(crate::help::topic_id_for(&active));
        }
        // Tabs that do not fit go behind a chevron; the active one always stays visible.
        let titles: Vec<String> = shown.iter().map(|p| crate::i18n::panel_title(p.id, p.title)).collect();
        let widths: Vec<f32> =
            titles.iter().map(|s| ui.painter().layout_no_wrap(s.clone(), fonts::semibold(t.font_label), t.text).size().x + 18.0).collect();
        let mut keep = vec![true; shown.len()];
        let overflow_w = 26.0;
        if 4.0 + widths.iter().sum::<f32>() > tabs_clip.width() {
            let room = tabs_clip.width() - 4.0 - overflow_w;
            keep = vec![false; shown.len()];
            let at = shown.iter().position(|p| p.id == active).unwrap_or(0);
            let mut used = 0.0;
            for i in std::iter::once(at).chain((0..shown.len()).filter(|i| *i != at)) {
                if used + widths[i] <= room {
                    keep[i] = true;
                    used += widths[i];
                }
            }
            tabs_clip = Rect::from_min_max(tabs_clip.min, pos2(tabs_clip.right() - overflow_w, tabs_clip.bottom()));
            let ovf = Rect::from_center_size(pos2(tabs_clip.right() + overflow_w / 2.0, header.center().y), vec2(22.0, 22.0));
            let hidden: Vec<(&'static str, String)> =
                shown.iter().zip(&titles).enumerate().filter(|(i, _)| !keep[*i]).map(|(_, (p, s))| (p.id, s.clone())).collect();
            let id = ui.scope_id().with(("tab_overflow", area));
            Menu::button_with(
                ui,
                |ui, _| {
                    let resp = ui.interact(ovf, id, Sense::click());
                    if resp.hovered() {
                        ui.painter().rect_filled(ovf, t.radius_sm, t.hover);
                    }
                    ui.painter().text(ovf.center(), Align2::CENTER_CENTER, "\u{bb}", FontId::proportional(t.font_heading), t.text);
                    solidcraft_ui_registry::publish_target(&format!("panel:overflow:{area:?}"), ovf);
                    resp.backing_on_hover_text(crate::i18n::tr("ui.more_tabs", "More tabs"))
                },
                |ui| {
                    ui.set_min_width(180.0);
                    for (pid, title) in &hidden {
                        if MenuItem::new(pid, title, "").show(ui).clicked() {
                            pick = Some(*pid);
                        }
                    }
                },
            );
        }
        for (pi, p) in shown.iter().enumerate() {
            if !keep[pi] {
                continue;
            }
            let on = p.id == active;
            let font = if on { fonts::semibold(t.font_label) } else { FontId::proportional(t.font_label) };
            let ink = if on { t.text } else { t.text_dim };
            let g = ui.painter().layout_no_wrap(titles[pi].clone(), font, ink);
            let tr = Rect::from_min_size(pos2(x, header.top() + 3.0), vec2(g.size().x + 16.0, PANEL_HEADER - 6.0));
            x = tr.right() + 2.0;
            let resp = ui.interact(tr.intersect(tabs_clip), ui.scope_id().with(("tab", p.id)), Sense::click());
            let painter = ui.painter().with_clip_rect(tabs_clip);
            if on {
                painter.rect_filled(tr, t.radius_sm, t.accent_soft);
            } else if resp.hovered() {
                painter.rect_filled(tr, t.radius_sm, t.hover);
            }
            painter.galley(pos2(tr.left() + 8.0, tr.center().y - g.size().y / 2.0), g, if on { t.selected_ink } else { ink });
            solidcraft_ui_registry::publish_target(&format!("panel:tab:{}", p.id), tr);
            if resp.clicked() {
                pick = Some(p.id);
            }
            Menu::context(&resp, |ui| {
                ui.set_min_width(200.0);
                for to in Area::ALL.into_iter().filter(|a| *a != area) {
                    let text = format!("{} {}", crate::i18n::tr("ui.move_to", "Move to"), to.label().to_lowercase());
                    if MenuItem::new("move", &text, "").show(ui).clicked() {
                        action = Some((p.id.to_string(), Some(to)));
                    }
                }
                Menu::separator(ui);
                if MenuItem::new("hide", &crate::i18n::tr("ui.hide", "Hide"), "close").show(ui).clicked() {
                    action = Some((p.id.to_string(), None));
                }
            });
        }
        if ui_kit::quiet_icon_button_at(ui, close, "close", "Hide this panel").clicked() {
            action = Some((active.clone(), None));
        }
        if let Some(id) = pick {
            self.prefs.active_tab.insert(area, id.to_string());
        }
        let body = Rect::from_min_max(pos2(r.left(), header.bottom()), r.max);
        let id = active.clone();
        ui.scope_builder(egui::UiBuilder::new().max_rect(body.shrink2(vec2(10.0, 8.0))), |ui| {
            egui::ScrollArea::vertical().id_salt(("panel_body", area)).auto_shrink([false, false]).show(ui, |ui| {
                panels::contents(ui, &id, self);
            });
        });
        if let Some((id, to)) = action
            && let Some(state) = self.prefs.panels.get_mut(&id)
        {
            match to {
                Some(to) => {
                    state.area = to;
                    self.prefs.active_tab.insert(to, id.clone());
                }
                None => state.visible = false,
            }
        }
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        if self.about_open {
            let (closed, action) = ui_kit::Window::new(egui::Id::unique("about"), "db_app", "About DarkBurn").actions("", "Close", None).show(ctx, true, |ui| {
                let t = Tokens::get();
                ui.label(egui::RichText::new(format!("DarkBurn {}", env!("CARGO_PKG_VERSION"))).font(fonts::semibold(t.font_subtitle)));
                ui.label(crate::i18n::tr("about.tagline", "A laser cutter and engraver program."));
                if ui.link(crate::i18n::tr("about.docs", "Online documentation")).clicked() {
                    crate::help::open_url(crate::help::DOCS_URL);
                }
                ui.add_space(t.space_8);
                ui.label(egui::RichText::new("Licensed under the MIT License or the Apache License 2.0.").color(t.text_dim));
                ui.label(
                    egui::RichText::new("Widgets, icons and themes from SolidCraft (MIT OR Apache-2.0). Interface font IBM Plex (SIL OFL 1.1). See ATTRIBUTION.md.")
                        .color(t.text_dim),
                );
            });
            if closed || action != ui_kit::Action::None {
                self.about_open = false;
            }
        }
        if self.settings_open {
            let prefs = &mut self.prefs;
            let (closed, action) = ui_kit::Window::new(egui::Id::unique("settings"), "settings", "Settings")
                .cards(2.0)
                .fields()
                .actions("", "Close", None)
                .show(ctx, true, |ui| {
                    crate::settings::page(ui, prefs);
                });
            if closed || action != ui_kit::Action::None {
                self.settings_open = false;
            }
        }
    }
}

fn separator(ui: &Ui, bar: Rect, x: f32) {
    let t = Tokens::get();
    ui.painter().vline(x, (bar.top() + 9.0)..=(bar.bottom() - 9.0), Stroke::new(1.0, t.app_bar_separator));
}

fn open_folder(dir: &std::path::Path) {
    #[cfg(target_os = "windows")]
    let program = "explorer";
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let program = "xdg-open";
    if let Err(e) = std::process::Command::new(program).arg(dir).spawn() {
        log::warn!("could not open {}: {e}", dir.display());
    }
}

/// The creation tools, top to bottom along the window's left edge.
/// The tool column's command ids.
pub fn tool_ids() -> &'static [&'static str] {
    TOOLS
}

const TOOLS: &[&str] = &[
    "tools.select",
    "tools.line",
    "tools.rect",
    "tools.ellipse",
    "tools.polygon",
    "tools.nodes",
    "tools.tabs",
    "tools.text",
    "tools.position_laser",
    "tools.measure",
    "tools.qr",
];

fn tools_column(ui: &mut Ui, pan_tool: bool, tool: Tool, hint: &mut Option<String>, clicked: &mut Option<String>) {
    let t = Tokens::get();
    let host = Container::new(ui.ctx(), Surface::Chrome);
    let panel = host.edge("db_tools", Edge::Left).exact_size(TOOLS_WIDTH).resizable(false);
    let frame = host.edge_frame(t.toolbar);
    host.show_edge(ui, panel, frame, |ui| {
        let r = ui.max_rect();
        let mut y = r.top() + 6.0;
        for id in TOOLS {
            let Some(c) = commands::find(id) else {
                continue;
            };
            let br = Rect::from_center_size(pos2(r.center().x, y + 16.0), vec2(32.0, 32.0));
            y += 36.0;
            let enabled = commands::works(id);
            let resp = ui.interact(br, ui.scope_id().with(("tool", *id)), if enabled { Sense::click() } else { Sense::hover() });
            let active = *id == tool.command() && !pan_tool;
            if active {
                ui.painter().rect(br, t.radius_sm, t.accent_soft, Stroke::new(1.0, t.accent), egui::StrokeKind::Inside);
            } else if enabled && resp.hovered() {
                ui.painter().rect_filled(br, t.radius_sm, t.hover);
            }
            icons::paint_icon(ui.painter(), br.shrink(6.0), c.icon, if enabled { 1.0 } else { 0.5 });
            solidcraft_ui_registry::publish_target(&format!("tool:{id}"), br);
            let tip = if c.key.is_empty() { c.label.to_string() } else { format!("{} ({})", c.label, c.key) };
            if resp.hovered() {
                *hint = Some(tip.clone());
            }
            if resp.backing_on_hover_text(tip).clicked() && enabled {
                *clicked = Some(id.to_string());
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toolbars_can_be_hidden_and_reordered_and_the_order_is_saved() {
        let mut app = DarkBurn::new(Prefs::default());
        let before = app.prefs.full_toolbar_order();
        assert_eq!(before[0], "main");
        let out = crate::api::execute_headless(&mut app, "window.toolbar_move", &serde_json::json!({ "toolbar": "arrange", "delta": -1 })).unwrap();
        assert_eq!(out["changed"], true);
        assert_eq!(app.prefs.full_toolbar_order()[0], "arrange");
        assert!(app.prefs.toolbar_rank("arrange") < app.prefs.toolbar_rank("main"));
        // At the start already: nothing moves. Unknown toolbars are refused.
        let out = crate::api::execute_headless(&mut app, "window.toolbar_move", &serde_json::json!({ "toolbar": "arrange", "delta": -1 })).unwrap();
        assert_eq!(out["changed"], false);
        assert!(crate::api::execute_headless(&mut app, "window.toolbar_move", &serde_json::json!({ "toolbar": "nope" })).is_err());
        // The order survives a save and load, and a layout reset restores the default.
        let mut back: Prefs = serde_json::from_str(&app.prefs.to_json()).unwrap();
        back.sanitise();
        assert_eq!(back.full_toolbar_order()[0], "arrange");
        app.prefs.reset_layout();
        assert_eq!(app.prefs.full_toolbar_order(), before);
        // A frame with the reordered ribbon draws.
        let ctx = egui::Context::default();
        app.prefs.move_toolbar("modes", -5);
        let mut out = ctx.run_ui(egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0))), ..Default::default() }, |ui| app.ui(ui));
        out.textures_delta.clear();
    }

    #[test]
    fn commands_toggle_panels_and_toolbars() {
        let ctx = egui::Context::default();
        let mut app = DarkBurn::new(Prefs::default());
        app.run(&ctx, "panel.move");
        assert!(app.prefs.panel("move").visible);
        assert_eq!(app.prefs.active_tab.get(&Area::RightBottom).map(String::as_str), Some("move"));
        app.run(&ctx, "toolbar.arrange");
        assert!(!app.prefs.toolbar("arrange"));
        app.run(&ctx, "window.reset_layout");
        assert!(!app.prefs.panel("move").visible);
        assert!(app.prefs.toolbar("arrange"));
        app.run(&ctx, "view.pan");
        assert!(app.pan_tool);
        app.run(&ctx, "tools.select");
        assert!(!app.pan_tool);
        app.run(&ctx, "view.zoom_in");
        assert_eq!(app.view_cmd, Some(ViewCmd::ZoomIn));
    }
}
