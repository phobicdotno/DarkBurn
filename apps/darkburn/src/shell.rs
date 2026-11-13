//! The window: application bar with the menus, ribbon, tools column, dock panels, cut palette,
//! status bar and the canvas in the middle. Laid out after SolidCraft's shell.

use std::path::PathBuf;

use darkburn_ui::ui_kit::{self, BackingTooltipExt as _, Container, Edge, Menu, MenuItem, Surface, window_host};
use darkburn_ui::{fonts, icons, titlebar};
use egui::{Align2, FontId, Pos2, Rect, Sense, Stroke, Ui, pos2, vec2};
use solidcraft_theme::Tokens;

use crate::canvas::{self, ViewCmd};
use crate::commands::{self, Entry};
use crate::dock_layout::{self, Dock};
use crate::panels::{self, Area, PANELS};
use crate::prefs::{Prefs, TOOLBARS};
use crate::ribbon;
use crate::tools::Tool;
use darkburn_core::Editor;

pub const APP_BAR_HEIGHT: f32 = 34.0;

/// The main window less the application bar: where a floating window opens and stays, so it
/// never lies over the bar, its menus and the main window's own buttons.
pub fn room_under_the_bar(ctx: &egui::Context) -> Rect {
    let screen = ctx.content_rect();
    Rect::from_min_max(pos2(screen.left(), (screen.top() + APP_BAR_HEIGHT).min(screen.bottom())), screen.max)
}
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
    /// The wireless handheld pendant, when the user turned it on (pendant_ui.rs).
    pub pendant: crate::pendant_ui::PendantUi,
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
    /// The job under way, for the job log (settings_wiring.rs).
    pub job_run: crate::settings_wiring::JobRun,
    /// Per area, the panel that was in front when a docked window's tab came forward: back in
    /// front when that window closes or moves away.
    panel_before_window: std::collections::BTreeMap<Area, String>,
}

/// The commands that open a dockable window, and its key. Run while the window is open, they
/// bring it into view.
pub(crate) const WINDOW_COMMANDS: [(&str, &str); 13] = [
    ("edit.settings", "settings"),
    ("file.hotkeys", "db_hotkeys"),
    ("file.import_options", "db_import_options"),
    ("file.prefs_backup", "db_prefs_backups"),
    ("laser.devices", "db_devices"),
    ("edit.device_settings", "db_device_settings"),
    ("edit.machine_settings", "db_machine_settings"),
    ("laser.controller_info", "db_controller_info"),
    ("tools.adjust_image", "db_adjust_image"),
    ("job.optimization_settings", "db_job_optimization"),
    ("job.gcode_settings", "db_job_gcode"),
    ("gweike.open", "db_gweike"),
    ("device.automation_open", "db_automation"),
];

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
            pendant: Default::default(),
            job: Default::default(),
            images: Default::default(),
            shape_ui: Default::default(),
            fio: Default::default(),
            text: Default::default(),
            vision: Default::default(),
            job_run: Default::default(),
            panel_before_window: Default::default(),
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
        if let Some((_, key)) = WINDOW_COMMANDS.iter().find(|(c, _)| *c == id) {
            // Open already, behind a tab or another window: setting its flag again shows nothing.
            window_host::raise(ctx, key);
        }
        if crate::actions::run(self, ctx, id)
            || crate::files::run(self, ctx, id)
            || crate::machine_ui::run(self, ctx, id)
            || crate::laser_position::run(self, ctx, id)
            || crate::job_cmds::run(self, ctx, id)
            || crate::key_cmds::run(self, ctx, id)
            || crate::misc_cmds::run(self, ctx, id)
            || crate::layer_defaults::run(self, ctx, id)
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

    /// The command whose shortcut was pressed this frame (the key press is used up).
    pub fn shortcut_command(&mut self, ctx: &egui::Context) -> Option<&'static str> {
        if ctx.memory(|m| m.focused().is_some()) || self.extras.capturing() {
            return None;
        }
        let over_window = crate::escape::window_open(ctx);
        for (shortcut, id) in crate::keymap::table(&self.prefs.hotkeys) {
            // A plain key pressed over a dialog or a menu is the dialog's: Delete or a letter must
            // not change the drawing behind it.
            if over_window && plain_key(&shortcut) {
                continue;
            }
            // In the node tool the plain letter keys are node commands, not align commands.
            if self.tool == Tool::Nodes && crate::node_tool::owns_shortcut(&shortcut) {
                continue;
            }
            if !ctx.input(|i| i.events.iter().any(|e| pressed_exactly(e, &shortcut))) {
                continue;
            }
            if commands::works(id) && ctx.input_mut(|i| i.consume_shortcut(&shortcut)) {
                return Some(id);
            }
        }
        None
    }

    /// The commands whose shortcuts were pressed while a window of its own (a detached window)
    /// had the keyboard, in the order pressed. The system gives the keys to that window, not to
    /// this one. They count as pressed over a window: a plain key was meant for the window.
    pub fn window_shortcut_commands(&self, ctx: &egui::Context) -> Vec<&'static str> {
        let keys = window_host::take_window_keys(ctx);
        if keys.is_empty() || self.extras.capturing() {
            return Vec::new();
        }
        let table = crate::keymap::table(&self.prefs.hotkeys);
        let nodes = self.tool == Tool::Nodes;
        keys.iter()
            .filter_map(|key| {
                table.iter().find_map(|(shortcut, id)| {
                    let mine = !plain_key(shortcut) && !(nodes && crate::node_tool::owns_shortcut(shortcut));
                    (mine && pressed_exactly(key, shortcut) && commands::works(id)).then_some(*id)
                })
            })
            .collect()
    }

    /// Run the shortcuts pressed in the app's windows of their own.
    pub fn window_shortcuts(&mut self, ctx: &egui::Context) {
        for id in self.window_shortcut_commands(ctx) {
            self.run(ctx, id);
        }
    }

    /// Keyboard shortcuts of the commands that work (from the keys their menu items show).
    pub fn shortcuts(&mut self, ctx: &egui::Context) {
        use egui::{Key, KeyboardShortcut, Modifiers};
        if ctx.memory(|m| m.focused().is_some()) || self.extras.capturing() {
            return;
        }
        let mut run = self.shortcut_command(ctx);
        let pressed = |m: Modifiers, k: Key| ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(m, k)));
        if run.is_none() && !crate::escape::window_open(ctx) {
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
        // Forget last frame's published widget rectangles (else the registry grows every frame).
        solidcraft_ui_registry::clear_handles();
        // Where the dockable windows are: the stored places in, the user's moves out.
        window_host::sync(ui.ctx(), &mut self.prefs.windows);
        keep_tab_for_the_canvas(ui.ctx());
        crate::tab_focus::keep_tab_in_the_top_window(ui.ctx());
        crate::extras::frame(ui.ctx(), self);
        // The areas and texts the window kit shows, in the language just set.
        let areas: Vec<(String, String)> = Area::ALL.into_iter().map(|a| (area_key(a), a.title())).collect();
        window_host::dock_areas(ui.ctx(), &areas.iter().map(|(k, l)| (k.as_str(), l.as_str())).collect::<Vec<_>>());
        window_host::set_texts(ui.ctx(), window_texts());
        // The canvas leaves its tool hint for the status bar, drawn before it next frame.
        self.hint = crate::canvas_input::hover_hint(ui.ctx());
        crate::files::frame(ui.ctx(), self);
        crate::machine_ui::frame(ui.ctx(), self);
        crate::pendant_ui::frame(self);
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
            for (icon, id) in [("save", "file.save"), ("undo", "edit.undo"), ("redo", "edit.redo")] {
                let tip = commands::find(id).map(command_tip).unwrap_or_default();
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
            for (icon, id) in [("help", "help.about"), ("settings", "edit.settings")] {
                let tip = commands::find(id).map(crate::i18n::cmd_label).unwrap_or_default();
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
                    let mut label = crate::i18n::cmd_label(c);
                    // Save machine file is named after the file the device takes [FILE-09].
                    if id == "file.save_gcode" && crate::machine_job::machine_file_kind(self.machine.active()).rd {
                        label = crate::machine_job::machine_file_kind(self.machine.active()).label.to_owned();
                    }
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
        // In machine coordinates, like the pointer position beside it, the rulers and the X and Y
        // fields: with the origin at the rear or the right they differ from the document's.
        let work_area = crate::numeric_edits::Frame {
            origin: self.prefs.origin,
            width: f64::from(self.prefs.page_width),
            height: f64::from(self.prefs.page_height),
        };
        let selection = self.editor.doc.bbox_of(&self.editor.selected()).map(|r| {
            let (a, b) = (
                work_area.to_machine(darkburn_core::kurbo::Point::new(r.x0, r.y0)),
                work_area.to_machine(darkburn_core::kurbo::Point::new(r.x1, r.y1)),
            );
            (darkburn_core::kurbo::Rect::from_points(a, b), self.editor.selection.len())
        });
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
            let right_edge = match crate::settings_wiring::status_text(self) {
                Some(text) => {
                    let g = ui.painter().layout_no_wrap(text, font.clone(), t.accent);
                    let left = right_edge - g.size().x - 12.0;
                    ui.painter().galley(pos2(left, r.center().y - g.size().y / 2.0), g, t.accent);
                    left - 8.0
                }
                None => right_edge,
            };
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
        if !self.area_used(ui.ctx(), area) {
            return;
        }
        let t = Tokens::get();
        let host = Container::new(ui.ctx(), Surface::Panel);
        let (edge, salt, dock) = match area {
            Area::Left => (Edge::Left, "db_dock_left", Dock::Left),
            Area::Bottom => (Edge::Bottom, "db_dock_bottom", Dock::Bottom),
            _ => (Edge::Right, "db_dock_right", Dock::Right),
        };
        let right_follows = dock == Dock::Left && (self.area_used(ui.ctx(), Area::RightTop) || self.area_used(ui.ctx(), Area::RightBottom));
        let (size, dragging) = self.dock_size(ui, dock, right_follows);
        let range = dragging.clone().unwrap_or(size..=size);
        let panel = host.edge(salt, edge).resizable(true).default_size(size).size_range(range);
        let frame = host.edge_frame(t.panel);
        let shown = host.show_edge(ui, panel, frame, |ui| {
            // The dock is as big as it was made, whatever the panel inside it needs.
            ui.expand_to_include_rect(ui.max_rect());
            self.tab_group(ui, area)
        });
        self.dock_shown(dock, size, dragging.is_some(), shown.response.rect);
    }

    /// The size dock `dock` gets this frame, and the sizes it may be dragged to while the pointer
    /// is down (egui's panel follows the pointer inside them). See `dock_layout`.
    fn dock_size(&self, ui: &Ui, dock: Dock, other_side: bool) -> (f32, Option<std::ops::RangeInclusive<f32>>) {
        let room = ui.available_rect_before_wrap();
        let available = if dock == Dock::Bottom { room.height() } else { room.width() };
        let limits = dock_layout::limits(dock, available, dock.reserve(other_side));
        let size = dock_layout::fit(dock_layout::wanted(&self.prefs.dock_sizes, dock), limits);
        let dragging = ui.input(|i| i.pointer.primary_down());
        (size, dragging.then_some(limits.0..=limits.1))
    }

    /// After a dock was drawn: a size that differs from the one asked for is the user dragging its
    /// edge, and becomes the size the dock keeps.
    fn dock_shown(&mut self, dock: Dock, asked: f32, dragging: bool, rect: Rect) {
        let shown = if dock == Dock::Bottom { rect.height() } else { rect.width() };
        if dragging && (shown - asked).abs() > 0.5 {
            self.prefs.dock_sizes.insert(dock.key().to_string(), shown);
        }
    }

    fn right_docks(&mut self, ui: &mut Ui) {
        let top = self.area_used(ui.ctx(), Area::RightTop);
        let bottom = self.area_used(ui.ctx(), Area::RightBottom);
        if !top && !bottom {
            return;
        }
        let t = Tokens::get();
        let host = Container::new(ui.ctx(), Surface::Panel);
        let (size, dragging) = self.dock_size(ui, Dock::Right, false);
        let range = dragging.clone().unwrap_or(size..=size);
        let panel = host.edge("db_dock_right", Edge::Right).resizable(true).default_size(size).size_range(range);
        let frame = host.edge_frame(t.panel);
        let shown = host.show_edge(ui, panel, frame, |ui| {
            ui.expand_to_include_rect(ui.max_rect());
            let r = ui.max_rect();
            match (top, bottom) {
                (true, true) => {
                    let split = r.top() + dock_layout::split(r.height());
                    let upper = Rect::from_min_max(r.min, pos2(r.right(), split));
                    let lower = Rect::from_min_max(pos2(r.left(), split + 1.0), r.max);
                    // Each group has its own id: both draw a close button with the same id source.
                    ui.scope_builder(egui::UiBuilder::new().id_salt("db_right_top").max_rect(upper), |ui| self.tab_group(ui, Area::RightTop));
                    ui.painter().hline(r.x_range(), split + 0.5, Stroke::new(1.0, t.border));
                    ui.scope_builder(egui::UiBuilder::new().id_salt("db_right_bottom").max_rect(lower), |ui| self.tab_group(ui, Area::RightBottom));
                }
                (true, false) => self.tab_group(ui, Area::RightTop),
                _ => self.tab_group(ui, Area::RightBottom),
            }
        });
        self.dock_shown(Dock::Right, size, dragging.is_some(), shown.response.rect);
    }

    /// The tab of `area` in front: the one brought forward; when that was a window's, now closed
    /// or moved away, the panel that was in front before it; else the first.
    fn front_tab(&mut self, area: Area, tabs: &[Tab]) -> String {
        let here = |id: &String| tabs.iter().any(|p| &p.id == id);
        let stored = self.prefs.active_tab.get(&area).cloned();
        match stored {
            Some(id) if here(&id) => id,
            Some(id) if window_host::window_of_tab(&id).is_some() => {
                let back = self.panel_before_window.remove(&area).filter(here).unwrap_or(tabs[0].id.clone());
                // Not left naming a window that is gone, in the stored settings either.
                self.prefs.active_tab.insert(area, back.clone());
                back
            }
            _ => tabs[0].id.clone(),
        }
    }

    /// Tab `id` of `area` comes to the front, in place of `front`. A window's tab remembers the
    /// panel it covers.
    fn bring_forward(&mut self, area: Area, front: &str, id: String) {
        if window_host::window_of_tab(&id).is_some() {
            if window_host::window_of_tab(front).is_none() {
                self.panel_before_window.insert(area, front.to_string());
            }
        } else {
            self.panel_before_window.remove(&area);
        }
        self.prefs.active_tab.insert(area, id);
    }

    /// Whether area `area` has anything to show: a visible panel or a window docked there.
    fn area_used(&self, ctx: &egui::Context, area: Area) -> bool {
        !self.visible_in(area).is_empty() || !window_host::docked_tabs(ctx, &area_key(area)).is_empty()
    }

    /// One area's tabs (its panels, then the windows docked there) and the active one's contents.
    fn tab_group(&mut self, ui: &mut Ui, area: Area) {
        let t = Tokens::get();
        let key = area_key(area);
        let mut tabs: Vec<Tab> = self
            .visible_in(area)
            .iter()
            .map(|p| Tab { id: p.id.to_string(), title: crate::i18n::panel_title(p.id, p.title), window: None })
            .collect();
        tabs.extend(window_host::docked_tabs(ui.ctx(), &key).into_iter().map(|w| Tab {
            id: window_host::tab_id(&w.key),
            title: w.title,
            window: Some(w.key),
        }));
        if tabs.is_empty() {
            return;
        }
        // A window just docked here, or opened while docked here, comes to the front.
        if let Some(w) = window_host::take_activation(ui.ctx(), &key) {
            let front = self.front_tab(area, &tabs);
            self.bring_forward(area, &front, window_host::tab_id(&w));
        }
        let active = self.front_tab(area, &tabs);
        let active_window = window_host::window_of_tab(&active).map(str::to_string);
        let r = ui.max_rect();
        solidcraft_ui_registry::publish_rect(&format!("panel:area:{area:?}"), r);
        window_host::dock_target(ui.ctx(), &key, r);
        let header = Rect::from_min_size(r.min, vec2(r.width(), PANEL_HEADER));
        ui.painter().rect_filled(header, 0.0, t.panel_header);
        ui.painter().hline(header.x_range(), header.bottom() - 0.5, Stroke::new(1.0, t.border));
        let mut x = header.left() + 4.0;
        let mut pick: Option<String> = None;
        let mut action: Option<(String, Option<Area>)> = None;
        let mut window_action: Option<(String, WindowMove)> = None;
        let close = Rect::from_center_size(pos2(header.right() - 15.0, header.center().y), vec2(22.0, 22.0));
        let mut tabs_clip = Rect::from_min_max(header.min, pos2(close.left() - 2.0, header.bottom()));
        if ui.rect_contains_pointer(r) && active_window.is_none() {
            self.extras.hover_topic = Some(crate::help::topic_id_for(&active));
        }
        // Tabs that do not fit go behind a chevron; the active one always stays visible.
        let widths: Vec<f32> =
            tabs.iter().map(|p| ui.painter().layout_no_wrap(p.title.clone(), fonts::semibold(t.font_label), t.text).size().x + 18.0).collect();
        let mut keep = vec![true; tabs.len()];
        let overflow_w = 26.0;
        if 4.0 + widths.iter().sum::<f32>() > tabs_clip.width() {
            let room = tabs_clip.width() - 4.0 - overflow_w;
            keep = vec![false; tabs.len()];
            let at = tabs.iter().position(|p| p.id == active).unwrap_or(0);
            // The active tab stays also when it alone is wider than the room: it is cut short.
            keep[at] = true;
            let mut used = widths[at];
            for i in (0..tabs.len()).filter(|i| *i != at) {
                if used + widths[i] <= room {
                    keep[i] = true;
                    used += widths[i];
                }
            }
            tabs_clip = Rect::from_min_max(tabs_clip.min, pos2(tabs_clip.right() - overflow_w, tabs_clip.bottom()));
            let ovf = Rect::from_center_size(pos2(tabs_clip.right() + overflow_w / 2.0, header.center().y), vec2(22.0, 22.0));
            let hidden: Vec<(String, String)> =
                tabs.iter().enumerate().filter(|(i, _)| !keep[*i]).map(|(_, p)| (p.id.clone(), p.title.clone())).collect();
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
                            pick = Some(pid.clone());
                        }
                    }
                },
            );
        }
        for (pi, p) in tabs.iter().enumerate() {
            if !keep[pi] {
                continue;
            }
            let on = p.id == active;
            let font = if on { fonts::semibold(t.font_label) } else { FontId::proportional(t.font_label) };
            let ink = if on { t.text } else { t.text_dim };
            let g = ui.painter().layout_no_wrap(p.title.clone(), font, ink);
            let tr = Rect::from_min_size(pos2(x, header.top() + 3.0), vec2(g.size().x + 16.0, PANEL_HEADER - 6.0));
            x = tr.right() + 2.0;
            // A window's tab can be dragged out: onto another area, or anywhere else to float.
            let sense = if p.window.is_some() { Sense::click_and_drag() } else { Sense::click() };
            let resp = ui.interact(tr.intersect(tabs_clip), ui.scope_id().with(("tab", p.id.as_str())), sense);
            let painter = ui.painter().with_clip_rect(tabs_clip);
            if on {
                painter.rect_filled(tr, t.radius_sm, t.accent_soft);
            } else if resp.hovered() {
                painter.rect_filled(tr, t.radius_sm, t.hover);
            }
            painter.galley(pos2(tr.left() + 8.0, tr.center().y - g.size().y / 2.0), g, if on { t.selected_ink } else { ink });
            solidcraft_ui_registry::publish_target(&format!("panel:tab:{}", p.id), tr.intersect(tabs_clip));
            if resp.clicked() {
                pick = Some(p.id.clone());
            }
            if let Some(w) = &p.window {
                if let Some(to) = tab_dragged_out(ui.ctx(), &resp, &key, r) {
                    window_action = Some((w.clone(), to));
                }
                Menu::context(&resp, |ui| {
                    ui.set_min_width(220.0);
                    for to in Area::ALL.into_iter().filter(|a| *a != area) {
                        let text = format!("{} {}", crate::i18n::tr("ui.move_to", "Move to"), to.title().to_lowercase());
                        if MenuItem::new("move", &text, "").show(ui).clicked() {
                            window_action = Some((w.clone(), WindowMove::Dock(area_key(to))));
                        }
                    }
                    Menu::separator(ui);
                    if MenuItem::new("float", &crate::i18n::tr("ui.window_float", "Float in the main window"), "").show(ui).clicked() {
                        window_action = Some((w.clone(), WindowMove::Float(None)));
                    }
                    if MenuItem::new("detach", &crate::i18n::tr("ui.window_detach", "Open in its own window"), "").show(ui).clicked() {
                        window_action = Some((w.clone(), WindowMove::Detach));
                    }
                    Menu::separator(ui);
                    if MenuItem::new("close", &crate::i18n::tr("ui.close", "Close"), "close").show(ui).clicked() {
                        window_action = Some((w.clone(), WindowMove::Close));
                    }
                });
            } else {
                Menu::context(&resp, |ui| {
                    ui.set_min_width(200.0);
                    for to in Area::ALL.into_iter().filter(|a| *a != area) {
                        let text = format!("{} {}", crate::i18n::tr("ui.move_to", "Move to"), to.title().to_lowercase());
                        if MenuItem::new("move", &text, "").show(ui).clicked() {
                            action = Some((p.id.clone(), Some(to)));
                        }
                    }
                    Menu::separator(ui);
                    if MenuItem::new("hide", &crate::i18n::tr("ui.hide", "Hide"), "close").show(ui).clicked() {
                        action = Some((p.id.clone(), None));
                    }
                });
            }
        }
        let close_label = if active_window.is_some() {
            crate::i18n::tr("ui.close_window", "Close this window")
        } else {
            crate::i18n::tr("ui.hide_panel", "Hide this panel")
        };
        if ui_kit::quiet_icon_button_at(ui, close, "close", &close_label).clicked() {
            match &active_window {
                Some(w) => window_action = Some((w.clone(), WindowMove::Close)),
                None => action = Some((active.clone(), None)),
            }
        }
        if let Some(id) = pick {
            self.bring_forward(area, &active, id);
        }
        let body = Rect::from_min_max(pos2(r.left(), header.bottom()), r.max);
        match &active_window {
            // The window draws itself into the body when its turn comes, after the docks.
            Some(w) => window_host::set_front(ui.ctx(), &key, Some(w), body, ui.layer_id()),
            None => {
                window_host::set_front(ui.ctx(), &key, None, body, ui.layer_id());
                let id = active.clone();
                ui.scope_builder(egui::UiBuilder::new().max_rect(body.shrink2(vec2(10.0, 8.0))), |ui| {
                    egui::ScrollArea::vertical().id_salt(("panel_body", area)).auto_shrink([false, false]).show(ui, |ui| {
                        panels::contents(ui, &id, self);
                    });
                });
            }
        }
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
        if let Some((w, to)) = window_action {
            match to {
                WindowMove::Dock(to) => window_host::update(ui.ctx(), &w, |p| p.dock(&to)),
                WindowMove::Float(at) => window_host::update(ui.ctx(), &w, |p| p.float(at)),
                WindowMove::Detach => window_host::update(ui.ctx(), &w, |p| p.detach()),
                WindowMove::Close => window_host::request_close(ui.ctx(), &w),
            }
        }
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        if self.about_open {
            let (closed, action) = ui_kit::Window::new(egui::Id::unique("about"), "db_app", &crate::i18n::tr("cmd.help.about", "About DarkBurn"))
                .actions("", &crate::i18n::tr("ui.close", "Close"), None)
                .show(ctx, true, |ui| {
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
            let work_area = (prefs.page_width, prefs.page_height, prefs.origin);
            let (closed, action) = ui_kit::Window::new(egui::Id::unique("settings"), "settings", &crate::i18n::tr("cmd.edit.settings", "Settings"))
                .dockable("settings")
                .cards(2.0)
                .fields()
                .actions("", &crate::i18n::tr("ui.close", "Close"), None)
                .show(ctx, true, |ui| {
                    crate::settings::page(ui, prefs);
                });
            if (self.prefs.page_width, self.prefs.page_height, self.prefs.origin) != work_area {
                crate::machine_ui::work_area_changed(self);
            }
            crate::misc_cmds::run_requests(self);
            if closed || action != ui_kit::Action::None {
                self.settings_open = false;
            }
        }
    }
}

/// Key press `e` is `shortcut`. egui matches a plain L when Alt+L is pressed; here the keys must
/// match exactly (a Shift that only types the symbol, like the one in "+", is allowed).
fn pressed_exactly(e: &egui::Event, shortcut: &egui::KeyboardShortcut) -> bool {
    let m = shortcut.modifiers;
    matches!(e, egui::Event::Key { key, pressed: true, modifiers, .. }
        if *key == shortcut.logical_key
            && modifiers.alt == m.alt
            && modifiers.command == m.command
            && (modifiers.shift == m.shift || *key == egui::Key::Plus))
}

/// A shortcut without Ctrl or Alt that is not a function key or Pause: a key a dialog may want
/// for itself. The others (Ctrl+Z, F1, Pause to hold a job) work whatever is open.
fn plain_key(shortcut: &egui::KeyboardShortcut) -> bool {
    let name = shortcut.logical_key.name();
    let function = name.len() > 1 && name.starts_with('F') && name[1..].chars().all(|c| c.is_ascii_digit());
    !shortcut.modifiers.command && !shortcut.modifiers.alt && !function && name != "Pause"
}

/// Tab steps through the shapes on the canvas. With no field holding the keyboard and no window
/// open, egui would also take the key to start its own walk through the controls: the focus
/// lands on the first button of the application bar, the canvas never sees the key, and every
/// shortcut is dead until the next click (shortcuts are off while a control has the keyboard).
/// Called before anything is drawn, so no control takes the focus this frame.
fn keep_tab_for_the_canvas(ctx: &egui::Context) {
    let tab = ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Key { key: egui::Key::Tab, pressed: true, .. })));
    if tab && ctx.memory(|m| m.focused().is_none()) && !crate::escape::window_open(ctx) {
        ctx.memory_mut(|m| m.move_focus(egui::FocusDirection::None));
    }
}

fn separator(ui: &Ui, bar: Rect, x: f32) {
    let t = Tokens::get();
    ui.painter().vline(x, (bar.top() + 9.0)..=(bar.bottom() - 9.0), Stroke::new(1.0, t.app_bar_separator));
}

fn open_folder(dir: &std::path::Path) {
    if cfg!(test) {
        return; // tests never open a file manager
    }
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

/// A command's tooltip: its name in the language in use, and the key it has now (which the
/// user may have changed or taken away).
pub fn command_tip(c: &commands::Cmd) -> String {
    let (label, key) = (crate::i18n::cmd_label(c), crate::keymap::key_for(c));
    if key.is_empty() { label } else { format!("{label} ({key})") }
}

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
            let tip = command_tip(c);
            if resp.hovered() {
                *hint = Some(tip.clone());
            }
            if resp.backing_on_hover_text(tip).clicked() && enabled {
                *clicked = Some(id.to_string());
            }
        }
    });
}

/// The kit's key for panel area `area` (its name).
fn area_key(area: Area) -> String {
    format!("{area:?}")
}

/// The window kit's texts for moving windows, in the interface language.
fn window_texts() -> window_host::WindowTexts {
    use crate::i18n::tr;
    window_host::WindowTexts {
        move_window: tr("ui.window_move", "Move this window"),
        float: tr("ui.window_float", "Float in the main window"),
        detach: tr("ui.window_detach", "Open in its own window"),
        dock_as_tab: tr("ui.window_dock", "Dock as tab"),
        close_question: tr("ui.window_close_question", "This window has changes you have not applied. Apply them before it closes?"),
        close_apply: tr("ui.window_close_apply", "Apply"),
        close_discard: tr("ui.window_close_discard", "Discard"),
        close_keep_open: tr("ui.window_close_keep_open", "Keep open"),
    }
}

/// A tab in a panel area: a panel, or a window docked there.
struct Tab {
    id: String,
    title: String,
    window: Option<String>,
}

/// What the user did with a docked window's tab.
enum WindowMove {
    Dock(String),
    Float(Option<egui::Pos2>),
    Detach,
    Close,
}

/// A window's tab being dragged: show where it will go; dropped on another area it docks there,
/// dropped outside its own area it floats where it was let go.
fn tab_dragged_out(ctx: &egui::Context, resp: &egui::Response, area: &str, own: Rect) -> Option<WindowMove> {
    if !(resp.dragged() || resp.drag_stopped()) {
        return None;
    }
    let pos = ctx.input(|i| i.pointer.latest_pos())?;
    let target = window_host::target_at(ctx, pos).filter(|a| a != area);
    if resp.dragged() {
        ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
        if let Some(rect) = target.as_deref().and_then(|a| window_host::target_rect(ctx, a)) {
            window_host::paint_drop_hint(ctx, rect);
        }
        return None;
    }
    match target {
        Some(to) => Some(WindowMove::Dock(to)),
        None if !own.contains(pos) => Some(WindowMove::Float(Some(pos - egui::vec2(40.0, 12.0)))),
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_command_that_raises_a_window_names_a_command_and_a_dockable_window() {
        let sources = [
            include_str!("shell.rs"),
            include_str!("extras.rs"),
            include_str!("file_io.rs"),
            include_str!("devices_ui.rs"),
            include_str!("image_ui.rs"),
            include_str!("job_ui.rs"),
            include_str!("gweike_ui.rs"),
            include_str!("connections.rs"),
        ]
        .concat();
        for (id, key) in WINDOW_COMMANDS {
            assert!(commands::lookup(id).is_some(), "{id} is a command");
            assert!(sources.contains(&format!(".dockable(\"{key}\")")), "{key} is a dockable window");
        }
    }

    #[test]
    fn every_button_of_the_tools_column_works_and_picks_its_tool() {
        let ctx = egui::Context::default();
        let mut app = DarkBurn::new(Prefs::default());
        for id in TOOLS {
            assert!(commands::works(id), "{id}");
            app.run(&ctx, id);
            if let Some(cmd) = [
                Tool::Select,
                Tool::Line,
                Tool::Rect,
                Tool::Ellipse,
                Tool::Polygon,
                Tool::Nodes,
                Tool::Tabs,
                Tool::Text,
                Tool::PositionLaser,
                Tool::Measure,
            ]
            .into_iter()
            .find(|t| t.command() == *id)
            {
                assert_eq!(app.tool, cmd, "{id} picks its tool");
            }
        }
        assert!(app.text.qr.open, "the QR button opens the QR code dialog");
    }

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
        let mut out = ctx.run_ui(
            egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0))), ..Default::default() },
            |ui| app.ui(ui),
        );
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
