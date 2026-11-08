//! The window: application bar with the menus, ribbon, tools column, dock panels, cut palette,
//! status bar and the canvas in the middle. Laid out after SolidCraft's shell.

use std::path::PathBuf;

use darkburn_ui::ui_kit::{self, BackingTooltipExt as _, Container, Edge, Menu, MenuItem, Surface};
use darkburn_ui::{fonts, icons, titlebar};
use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Ui, pos2, vec2};
use solidcraft_theme::Tokens;

use crate::canvas::{self, ViewCmd};
use crate::commands::{self, Entry};
use crate::panels::{self, Area, PANELS};
use crate::prefs::{Prefs, TOOLBARS};
use crate::ribbon;

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

    /// Run command `id` (menus, ribbon, shortcuts). Commands of later phases do nothing.
    pub fn run(&mut self, ctx: &egui::Context, id: &str) {
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
            "tools.select" => self.pan_tool = false,
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

    /// Keyboard shortcuts of the commands that work, and the canvas keys.
    pub fn shortcuts(&mut self, ctx: &egui::Context) {
        use egui::{Key, KeyboardShortcut, Modifiers};
        if ctx.memory(|m| m.focused().is_some()) {
            return;
        }
        let pressed = |m: Modifiers, k: Key| ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(m, k)));
        let mut run = None;
        if pressed(Modifiers::COMMAND, Key::Q) {
            run = Some("file.exit");
        } else if pressed(Modifiers::COMMAND, Key::Num0) {
            run = Some("view.zoom_page");
        } else if pressed(Modifiers::COMMAND, Key::Equals) || pressed(Modifiers::COMMAND, Key::Plus) || pressed(Modifiers::NONE, Key::Plus) {
            run = Some("view.zoom_in");
        } else if pressed(Modifiers::COMMAND, Key::Minus) || pressed(Modifiers::NONE, Key::Minus) {
            run = Some("view.zoom_out");
        } else if pressed(Modifiers::NONE, Key::Escape) && self.pan_tool {
            run = Some("tools.select");
        }
        if let Some(id) = run {
            self.run(ctx, id);
        }
    }

    /// The whole window for one frame.
    pub fn ui(&mut self, ui: &mut Ui) {
        let mut clicked: Option<String> = None;
        self.app_bar(ui, &mut clicked);
        ribbon::show(ui, &mut self.prefs, self.pan_tool, &mut clicked);
        if self.prefs.status_bar {
            self.status_bar(ui);
        }
        if self.prefs.toolbar("palette") {
            cut_palette(ui);
        }
        self.dock(ui, Area::Bottom);
        if self.prefs.toolbar("tools") {
            tools_column(ui, self.pan_tool, &mut clicked);
        }
        self.dock(ui, Area::Left);
        self.right_docks(ui);
        let cmd = self.view_cmd.take();
        let frame = egui::Frame::NONE.fill(Tokens::get().viewport_bottom);
        let pan_tool = self.pan_tool;
        let prefs = &mut self.prefs;
        let mut cursor = None;
        Container::new(ui.ctx(), Surface::Viewport).central().frame(frame).show(ui, |ui| {
            cursor = canvas::show(ui, prefs, pan_tool, cmd).cursor;
        });
        self.cursor = cursor;
        self.dialogs(ui.ctx());
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
                let galley = ui.painter().layout_no_wrap(name.to_string(), FontId::proportional(t.font_button), t.app_bar_text);
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
            ui.painter().text(
                pos2(rx + 4.0, r.center().y),
                Align2::RIGHT_CENTER,
                version,
                FontId::proportional(t.font_small),
                t.app_bar_text.gamma_multiply(0.7),
            );
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
                    let mut item = MenuItem::new(c.id, c.label, c.icon);
                    item.shortcut = c.key;
                    item.enabled = commands::works(id);
                    if id == "view.pan" {
                        item.checked = Some(self.pan_tool);
                    }
                    if item.show(ui).clicked() && item.enabled {
                        *clicked = Some(id.to_string());
                    }
                }
                Entry::Sub(label, inner) => {
                    let mut row = MenuItem::new(label, label, "");
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
                    let mut row = MenuItem::new("file.recent", "Recent Projects", "");
                    row.submenu = true;
                    row.enabled = false;
                    row.show(ui);
                }
                Entry::Dynamic("panels") => {
                    Menu::heading(ui, "Panels");
                    for p in PANELS {
                        let id = format!("panel.{}", p.id);
                        let mut item = MenuItem::new(&id, p.title, "");
                        item.checked = Some(self.prefs.panel(p.id).visible);
                        if item.show(ui).clicked() {
                            *clicked = Some(id.clone());
                        }
                    }
                }
                Entry::Dynamic("toolbars") => {
                    Menu::heading(ui, "Toolbars");
                    for (bar, label) in TOOLBARS.iter().copied().chain([("status", "Status Bar")]) {
                        let id = format!("toolbar.{bar}");
                        let mut item = MenuItem::new(&id, label, "");
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
                        item.enabled = *code == "en";
                        if item.show(ui).clicked() && item.enabled {
                            *clicked = Some(id.clone());
                        }
                    }
                }
                Entry::Dynamic(_) => {}
            }
        }
    }

    fn status_bar(&self, ui: &mut Ui) {
        let t = Tokens::get();
        let host = Container::new(ui.ctx(), Surface::ApplicationBar);
        let panel = host.edge("db_status", Edge::Bottom).exact_size(STATUS_BAR_HEIGHT).show_separator_line(false);
        let frame = host.edge_frame(t.app_bar);
        host.show_edge(ui, panel, frame, |ui| {
            let r = ui.max_rect();
            let font = FontId::proportional(t.font_small);
            let ink = t.app_bar_text;
            let dim = t.app_bar_text.gamma_multiply(0.45);
            let mut x = r.left() + 10.0;
            for lock in ["Move", "Size", "Rotate", "Shear"] {
                let g = ui.painter().layout_no_wrap(lock.to_string(), font.clone(), dim);
                let br = Rect::from_min_size(pos2(x, r.top() + 3.0), vec2(g.size().x + 12.0, r.height() - 6.0));
                ui.painter().rect_stroke(br, t.radius_sm, Stroke::new(1.0, dim), egui::StrokeKind::Inside);
                ui.painter().galley(pos2(br.left() + 6.0, br.center().y - g.size().y / 2.0), g, dim);
                ui.interact(br, ui.scope_id().with(("lock", lock)), Sense::hover())
                    .backing_on_hover_text(format!("{lock} lock (arrives with editing)"));
                x = br.right() + 4.0;
            }
            x += 12.0;
            let position = match self.cursor {
                Some(p) => format!("x: {:.2}  y: {:.2} mm", p.x, p.y),
                None => "x: –  y: –".to_string(),
            };
            let g = ui.painter().layout_no_wrap(position, FontId::monospace(t.font_small), ink);
            ui.painter().galley(pos2(x, r.center().y - g.size().y / 2.0), g, ink);
            solidcraft_ui_registry::publish_rect("status:cursor", Rect::from_min_size(pos2(x, r.top()), vec2(220.0, r.height())));
            x += 230.0;
            ui.painter().text(pos2(x, r.center().y), Align2::LEFT_CENTER, "No selection", font.clone(), dim);
            x += 110.0;
            let tip = self.note.clone().unwrap_or_else(|| if self.pan_tool { "Drag to pan the view".into() } else { String::new() });
            ui.painter().text(pos2(x, r.center().y), Align2::LEFT_CENTER, tip, font.clone(), ink);
            let right = format!("{:.0} %", self.prefs.view.percent());
            ui.painter().text(pos2(r.right() - 10.0, r.center().y), Align2::RIGHT_CENTER, "Auto-save off", font.clone(), dim);
            ui.painter().text(pos2(r.right() - 110.0, r.center().y), Align2::RIGHT_CENTER, right, font, ink);
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
            _ => (Edge::Right, "db_dock_right", 300.0, 220.0..=560.0),
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
        let panel = host.edge("db_dock_right", Edge::Right).resizable(true).default_size(300.0).size_range(220.0..=560.0);
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
        let tabs_clip = Rect::from_min_max(header.min, pos2(close.left() - 2.0, header.bottom()));
        for p in &shown {
            let on = p.id == active;
            let font = if on { fonts::semibold(t.font_label) } else { FontId::proportional(t.font_label) };
            let ink = if on { t.text } else { t.text_dim };
            let g = ui.painter().layout_no_wrap(p.title.to_string(), font, ink);
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
                    if MenuItem::new("move", &format!("Move to {}", to.label().to_lowercase()), "").show(ui).clicked() {
                        action = Some((p.id.to_string(), Some(to)));
                    }
                }
                Menu::separator(ui);
                if MenuItem::new("hide", "Hide", "close").show(ui).clicked() {
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
                panels::contents(ui, &id);
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
                ui.label("A laser cutter and engraver program.");
                ui.add_space(t.space_8);
                ui.label(egui::RichText::new("Licensed under the MIT License or the Apache License 2.0.").color(t.text_dim));
                ui.label(
                    egui::RichText::new("Look and feel, widgets, icons and themes from SolidCraft (MIT OR Apache-2.0). Interface font IBM Plex (SIL OFL 1.1). See ATTRIBUTION.md.")
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

fn tools_column(ui: &mut Ui, pan_tool: bool, clicked: &mut Option<String>) {
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
            let active = *id == "tools.select" && !pan_tool;
            if active {
                ui.painter().rect(br, t.radius_sm, t.accent_soft, Stroke::new(1.0, t.accent), egui::StrokeKind::Inside);
            } else if enabled && resp.hovered() {
                ui.painter().rect_filled(br, t.radius_sm, t.hover);
            }
            icons::paint_icon(ui.painter(), br.shrink(6.0), c.icon, if enabled { 1.0 } else { 0.5 });
            solidcraft_ui_registry::publish_target(&format!("tool:{id}"), br);
            let tip = if c.key.is_empty() { c.label.to_string() } else { format!("{} ({})", c.label, c.key) };
            if resp.backing_on_hover_text(tip).clicked() && enabled {
                *clicked = Some(id.to_string());
            }
        }
    });
}

/// The 30 colour layers and 2 tool layers along the bottom. Layers arrive with drawing.
fn cut_palette(ui: &mut Ui) {
    let t = Tokens::get();
    let host = Container::new(ui.ctx(), Surface::Chrome);
    let panel = host.edge("db_palette", Edge::Bottom).exact_size(PALETTE_HEIGHT).show_separator_line(false);
    let frame = host.edge_frame(t.toolbar);
    host.show_edge(ui, panel, frame, |ui| {
        let r = ui.max_rect();
        let size = 20.0;
        let mut x = r.left() + 8.0;
        for i in 0..32 {
            let br = Rect::from_center_size(pos2(x + size / 2.0, r.center().y), vec2(size, size));
            x += size + 3.0;
            let (fill, label) = if i < 30 { (layer_colour(i), format!("{i:02}")) } else { (t.control_fill, format!("T{}", i - 29)) };
            let p = ui.painter();
            p.rect(br, t.radius_sm, fill.gamma_multiply(0.55), Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
            if i >= 30 {
                p.text(br.center(), Align2::CENTER_CENTER, label.clone(), FontId::proportional(t.font_micro), t.text_dim);
            }
            ui.interact(br, ui.scope_id().with(("layer", i)), Sense::hover()).backing_on_hover_text(format!("Layer {label} (arrives with drawing)"));
        }
    });
}

/// DarkBurn's own layer colours: 30 well-separated hues and greys, picked for contrast.
pub fn layer_colour(i: usize) -> Color32 {
    match i {
        0 => Color32::from_rgb(20, 20, 20),
        27 => Color32::from_rgb(110, 110, 110),
        28 => Color32::from_rgb(170, 170, 170),
        29 => Color32::from_rgb(230, 230, 230),
        _ => {
            let h = ((i - 1) as f32 * 137.508) % 360.0;
            let l = if i.is_multiple_of(2) { 0.45 } else { 0.6 };
            hsl(h, 0.75, l)
        }
    }
}

fn hsl(h: f32, s: f32, l: f32) -> Color32 {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = match (h / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let f = |v: f32| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgb(f(r), f(g), f(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layer_colours_are_distinct() {
        let colours: Vec<_> = (0..30).map(layer_colour).collect();
        for (i, a) in colours.iter().enumerate() {
            for b in &colours[i + 1..] {
                assert_ne!(a, b);
            }
        }
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
