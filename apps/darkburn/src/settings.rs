//! The Settings window: pages of settings (checklist section 22), each saved as it changes.

use std::sync::OnceLock;

use darkburn_ui::ui_kit;
use darkburn_ui::{fonts, theme_files};
use egui::{Color32, Rect, Sense, Stroke, Ui, vec2};
use solidcraft_theme::Tokens;

use crate::canvas::Backdrop;
use crate::i18n::tr;
use crate::options::{CurveQuality, FileDialogs, IconSize};
use crate::prefs::Prefs;
use crate::view::Origin;

/// A theme in the picker: id, name, and a few colours to preview it by.
struct Choice {
    id: String,
    name: String,
    swatches: [Color32; 4],
}

fn swatches(t: &Tokens) -> [Color32; 4] {
    [t.app_bar, t.panel, t.viewport_top, t.accent]
}

/// System, Dark and Light, then SolidCraft's bundled themes.
fn choices() -> &'static [Choice] {
    static CHOICES: OnceLock<Vec<Choice>> = OnceLock::new();
    CHOICES.get_or_init(|| {
        let dark = swatches(&Tokens::dark());
        let light = swatches(&Tokens::light());
        let mut all = vec![
            Choice { id: "system".into(), name: "Follow the system".into(), swatches: [dark[0], dark[1], light[1], dark[3]] },
            Choice { id: "dark".into(), name: "Dark".into(), swatches: dark },
            Choice { id: "light".into(), name: "Light".into(), swatches: light },
        ];
        for f in theme_files::builtin() {
            if matches!(f.id.as_str(), "dark" | "light") {
                continue;
            }
            all.push(Choice { id: f.id.clone(), name: f.name.clone(), swatches: swatches(&theme_files::resolve(f)) });
        }
        all
    })
}

#[cfg(test)]
/// The theme ids the picker offers.
pub fn theme_ids() -> Vec<String> {
    choices().iter().map(|c| c.id.clone()).collect()
}

fn theme_picker(ui: &mut Ui, prefs: &mut Prefs) {
    let t = Tokens::get();
    let columns = 3;
    let width = ((ui.available_width() - (columns - 1) as f32 * t.space_6) / columns as f32).floor();
    egui::Grid::new("themes").spacing(vec2(t.space_6, t.space_6)).show(ui, |ui| {
        for (i, c) in choices().iter().enumerate() {
            let selected = prefs.theme == c.id;
            let (rect, resp) = ui.allocate_exact_size(vec2(width, 44.0), Sense::click());
            let fill = if selected {
                t.selected_bg
            } else if resp.hovered() {
                t.hover
            } else {
                t.control_fill
            };
            let border = if selected { Stroke::new(2.0, t.form_primary) } else { Stroke::new(1.0, t.dialog_border) };
            ui.painter().rect(rect, t.radius_md, fill, border, egui::StrokeKind::Inside);
            let strip = Rect::from_min_size(rect.min + vec2(8.0, 8.0), vec2(rect.width() - 16.0, 10.0));
            let w = strip.width() / 4.0;
            for (k, colour) in c.swatches.iter().enumerate() {
                let s = Rect::from_min_size(strip.min + vec2(k as f32 * w, 0.0), vec2(w, strip.height()));
                ui.painter().rect_filled(s, 0.0, *colour);
            }
            ui.painter().rect_stroke(strip, 0.0, Stroke::new(1.0, t.border), egui::StrokeKind::Outside);
            let font = if selected { fonts::semibold(t.font_small) } else { egui::FontId::proportional(t.font_small) };
            ui.painter().text(
                rect.left_bottom() + vec2(8.0, -8.0),
                egui::Align2::LEFT_BOTTOM,
                choice(&c.name),
                font,
                if selected { t.selected_ink } else { t.text },
            );
            solidcraft_ui_registry::publish_target(&format!("settings:theme:{}", c.id), rect);
            if resp.clicked() {
                prefs.theme = c.id.clone();
            }
            if (i + 1) % columns == 0 {
                ui.end_row();
            }
        }
    });
}

#[cfg(test)]
/// Choice labels the pages show; each is translated under `choice.<label>`.
pub const CHOICES: &[&str] = &[
    "Follow the system",
    "Dark",
    "Light",
    "Follow the theme",
    "Front left",
    "Front right",
    "Rear left",
    "Rear right",
    "Coarse",
    "Normal",
    "Fine",
    "Small",
    "Medium",
    "Large",
    "System dialogs",
    "Built-in dialogs",
    "72 DPI",
    "96 DPI",
    "Millimetres",
    "Inches",
    "mm/s",
    "mm/min",
    "Automatic",
    "Default camera",
];

fn choice(label: &str) -> String {
    tr(&format!("choice.{label}"), label)
}

/// A drop-down over `labels` (translated), `selected` is an index.
fn pick(ui: &mut Ui, id: &str, selected: &mut usize, labels: &[&str]) {
    let shown: Vec<String> = labels.iter().map(|l| choice(l)).collect();
    let refs: Vec<&str> = shown.iter().map(String::as_str).collect();
    ui_kit::dropdown(ui, id, selected, &refs);
}

fn pick_enum<T: Copy + PartialEq>(ui: &mut Ui, id: &str, value: &mut T, all: &[T], label_of: impl Fn(T) -> &'static str) {
    let labels: Vec<&str> = all.iter().map(|v| label_of(*v)).collect();
    let mut i = all.iter().position(|v| v == value).unwrap_or(0);
    pick(ui, id, &mut i, &labels);
    *value = all[i];
}

fn length_field(ui: &mut Ui, v: &mut f64, lo: f64, hi: f64, speed: f64, suffix: &str) {
    ui_kit::number_field(ui, v, lo..=hi, speed, suffix);
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Page {
    General,
    Display,
    Editing,
    WorkArea,
    Files,
    Text,
    Devices,
    Defaults,
    Agents,
}

impl Page {
    const ALL: [Page; 9] =
        [Page::General, Page::Display, Page::Editing, Page::WorkArea, Page::Files, Page::Text, Page::Devices, Page::Defaults, Page::Agents];

    fn title(self) -> String {
        match self {
            Page::General => tr("settings.page.general", "General"),
            Page::Display => tr("settings.page.display", "Display"),
            Page::Editing => tr("settings.page.editing", "Editing"),
            Page::WorkArea => tr("settings.page.work_area", "Work area"),
            Page::Files => tr("settings.page.files", "Files"),
            Page::Text => tr("settings.page.text", "Text"),
            Page::Devices => tr("settings.page.devices", "Devices"),
            Page::Defaults => tr("settings.page.defaults", "Shared defaults"),
            Page::Agents => "AI agents (MCP)".to_string(),
        }
    }
}

/// The window's body: a row of pages, and the settings of the chosen one.
pub fn page(ui: &mut Ui, prefs: &mut Prefs) {
    let t = Tokens::get();
    let tab_id = egui::Id::unique("db_settings_page");
    let mut current: usize = ui.data(|d| d.get_temp(tab_id)).unwrap_or(0);
    if crate::mcp_ui::take_goto_page() {
        current = Page::ALL.len() - 1;
    }
    ui.horizontal_wrapped(|ui| {
        for (i, p) in Page::ALL.iter().enumerate() {
            if ui.selectable_label(current == i, p.title()).clicked() {
                current = i;
            }
        }
    });
    ui.data_mut(|d| d.insert_temp(tab_id, current));
    ui.add_space(t.space_8);
    ui.spacing_mut().item_spacing.y = t.form_gap_y;
    match Page::ALL[current.min(Page::ALL.len() - 1)] {
        Page::General => general(ui, prefs),
        Page::Display => display(ui, prefs),
        Page::Editing => editing(ui, prefs),
        Page::WorkArea => work_area(ui, prefs),
        Page::Files => files(ui, prefs),
        Page::Text => text(ui, prefs),
        Page::Devices => devices(ui, prefs),
        Page::Defaults => defaults(ui, prefs),
        Page::Agents => crate::mcp_ui::settings_page(ui, prefs),
    }
}

fn general(ui: &mut Ui, prefs: &mut Prefs) {
    let t = Tokens::get();
    ui_kit::section_heading(ui, &tr("settings.units", "Units"));
    ui_kit::row(ui, &tr("settings.length_units", "Lengths"), |ui| {
        let mut i = usize::from(prefs.numeric_inches);
        pick(ui, "length_units", &mut i, &["Millimetres", "Inches"]);
        prefs.numeric_inches = i == 1;
    });
    ui_kit::row(ui, &tr("settings.speed_units", "Speeds"), |ui| {
        let mut i = usize::from(prefs.opts.speed_per_min);
        pick(ui, "speed_units", &mut i, &["mm/s", "mm/min"]);
        prefs.opts.speed_per_min = i == 1;
    });
    ui.add_space(t.space_12);
    crate::text_ui::settings_section(ui, prefs);
    ui.add_space(t.space_12);
    ui_kit::section_heading(ui, &tr("settings.behaviour", "Behaviour"));
    ui_kit::checkbox(ui, &tr("settings.beginner", "Beginner mode (hides advanced cut settings)"), &mut prefs.cut_beginner);
    ui_kit::checkbox(ui, &tr("settings.check_updates", "Check for updates"), &mut prefs.opts.check_updates);
    ui_kit::checkbox(ui, &tr("settings.running_copy", "Open files in the running copy"), &mut prefs.files.single_instance);
    ui_kit::checkbox(ui, &tr("settings.sound_error", "Play a sound on errors"), &mut prefs.opts.sound_error);
    ui_kit::checkbox(ui, &tr("settings.sound_done", "Play a sound when a job finishes"), &mut prefs.opts.sound_done);
    ui_kit::checkbox(ui, &tr("settings.job_log", "Save a log of each job"), &mut prefs.opts.job_log);
    ui_kit::checkbox(ui, &tr("settings.system_clipboard", "Use the system clipboard"), &mut prefs.opts.system_clipboard);
    ui_kit::row(ui, &tr("settings.file_dialogs", "File dialogs"), |ui| {
        pick_enum(ui, "file_dialogs", &mut prefs.opts.file_dialogs, &[FileDialogs::System, FileDialogs::BuiltIn], |d| match d {
            FileDialogs::System => "System dialogs",
            FileDialogs::BuiltIn => "Built-in dialogs",
        });
    });
    ui.add_space(t.space_12);
    ui_kit::section_heading(ui, &tr("settings.autosave", "Auto-save"));
    ui_kit::row(ui, &tr("settings.every", "Every"), |ui| {
        ui_kit::number_field(ui, &mut prefs.autosave_minutes, 0.0..=240.0, 0.5, " min");
    });
    ui_kit::caption(ui, &tr("settings.every", "Every"), &tr("settings.autosave_off", "0 turns auto-save off."));
}

fn display(ui: &mut Ui, prefs: &mut Prefs) {
    let t = Tokens::get();
    ui_kit::section_heading(ui, &tr("settings.theme", "Theme"));
    theme_picker(ui, prefs);
    ui.add_space(t.space_12);
    ui_kit::section_heading(ui, &tr("settings.canvas", "Canvas"));
    ui_kit::row(ui, &tr("settings.background", "Background"), |ui| {
        pick_enum(ui, "backdrop", &mut prefs.backdrop, &Backdrop::ALL, Backdrop::label);
    });
    ui_kit::row(ui, &tr("settings.grid_contrast", "Grid contrast"), |ui| {
        ui_kit::slider(ui, &mut prefs.grid_contrast, 0.0..=1.0, ui_kit::Slider { format: |v| format!("{:.0} %", v * 100.0), ..Default::default() });
    });
    ui_kit::checkbox(ui, &tr("settings.show_grid", "Show the grid"), &mut prefs.show_grid);
    ui_kit::checkbox(ui, &tr("settings.invert_wheel", "Invert the mouse wheel zoom"), &mut prefs.invert_wheel);
    ui_kit::checkbox(ui, &tr("settings.palette_labels", "Numbers on the cut palette"), &mut prefs.palette_labels);
    ui_kit::checkbox(ui, &tr("settings.fullscreen_cursor", "Full-screen cursor lines"), &mut prefs.opts.fullscreen_cursor);
    ui_kit::checkbox(ui, &tr("settings.centre_cross", "Cross at the centre of the page"), &mut prefs.opts.centre_cross);
    ui_kit::checkbox(ui, &tr("settings.rotary_toggles", "Show the rotary and cylinder toggles"), &mut prefs.opts.show_rotary_toggles);
    prefs.toolbars.insert("modes".into(), prefs.opts.show_rotary_toggles);
    ui.add_space(t.space_12);
    ui_kit::section_heading(ui, &tr("settings.quality", "Quality"));
    ui_kit::row(ui, &tr("settings.curve_quality", "Curve quality"), |ui| {
        pick_enum(ui, "curve_quality", &mut prefs.opts.curve_quality, &CurveQuality::ALL, |q| match q {
            CurveQuality::Coarse => "Coarse",
            CurveQuality::Normal => "Normal",
            CurveQuality::Fine => "Fine",
        });
    });
    ui_kit::checkbox(ui, &tr("settings.antialias", "Smooth edges (antialiasing)"), &mut prefs.opts.antialias);
    ui_kit::checkbox(ui, &tr("settings.filled", "Draw filled shapes filled"), &mut prefs.filled);
    ui_kit::checkbox(ui, &tr("settings.reduce_motion", "Reduce motion"), &mut prefs.opts.reduce_motion);
    ui.add_space(t.space_12);
    ui_kit::section_heading(ui, &tr("settings.sizes", "Sizes"));
    ui_kit::row(ui, &tr("settings.ui_scale", "Interface size"), |ui| {
        ui_kit::number_field(ui, &mut prefs.opts.ui_scale, 70.0..=250.0, 5.0, " %");
    });
    ui_kit::row(ui, &tr("settings.icon_size", "Toolbar icons"), |ui| {
        pick_enum(ui, "icon_size", &mut prefs.opts.icon_size, &IconSize::ALL, |s| match s {
            IconSize::Small => "Small",
            IconSize::Medium => "Medium",
            IconSize::Large => "Large",
        });
    });
    ui_kit::row(ui, &tr("settings.cursor_size", "Cursor size"), |ui| {
        ui_kit::number_field(ui, &mut prefs.opts.cursor_size, 2.0..=60.0, 0.5, " px");
    });
    ui_kit::row(ui, &tr("settings.node_size", "Node size"), |ui| {
        ui_kit::number_field(ui, &mut prefs.opts.node_size, 2.0..=16.0, 0.5, " px");
    });
}

fn editing(ui: &mut Ui, prefs: &mut Prefs) {
    let t = Tokens::get();
    ui_kit::section_heading(ui, &tr("settings.snapping", "Snapping"));
    ui_kit::checkbox(ui, &tr("settings.snap_grid", "Snap to the grid"), &mut prefs.opts.snap_grid);
    ui_kit::checkbox(ui, &tr("settings.snap_objects", "Snap to shapes"), &mut prefs.opts.snap_objects);
    ui_kit::row(ui, &tr("settings.grid_snap", "Grid snap distance"), |ui| {
        ui_kit::number_field(ui, &mut prefs.opts.grid_snap_mm, 0.0..=1000.0, 0.1, " mm");
    });
    ui_kit::caption(ui, &tr("settings.grid_snap", "Grid snap distance"), &tr("settings.grid_snap_auto", "0 follows the zoom."));
    ui_kit::row(ui, &tr("settings.object_snap", "Shape snap distance"), |ui| {
        ui_kit::number_field(ui, &mut prefs.opts.object_snap_px, 0.0..=40.0, 0.5, " px");
    });
    ui_kit::row(ui, &tr("settings.click_tolerance", "Click tolerance"), |ui| {
        ui_kit::number_field(ui, &mut prefs.opts.click_tolerance_px, 1.0..=20.0, 0.5, " px");
    });
    ui.add_space(t.space_12);
    ui_kit::section_heading(ui, &tr("settings.nudge", "Arrow key nudge"));
    ui_kit::row(ui, &tr("settings.nudge_plain", "Plain"), |ui| length_field(ui, &mut prefs.opts.nudge, 0.001, 1000.0, 0.1, " mm"));
    ui_kit::row(ui, &tr("settings.nudge_ctrl", "With Ctrl"), |ui| length_field(ui, &mut prefs.opts.nudge_ctrl, 0.001, 1000.0, 0.01, " mm"));
    ui_kit::row(ui, &tr("settings.nudge_shift", "With Shift"), |ui| length_field(ui, &mut prefs.opts.nudge_shift, 0.001, 1000.0, 1.0, " mm"));
    ui.add_space(t.space_12);
    ui_kit::section_heading(ui, &tr("settings.jobs", "Jobs"));
    ui_kit::checkbox(ui, &tr("settings.cut_selected", "Cut selected graphics by default"), &mut prefs.opts.default_cut_selected);
    ui_kit::checkbox(ui, &tr("settings.selection_origin", "Use selection origin by default"), &mut prefs.opts.default_use_selection_origin);
    ui_kit::checkbox(ui, &tr("settings.save_job_origin", "Save the job origin and cut selected in the project"), &mut prefs.opts.save_job_origin);
    ui_kit::checkbox(ui, &tr("settings.remember_locks", "Remember the move, size, rotate and shear locks"), &mut prefs.opts.remember_locks);
    ui_kit::checkbox(ui, &tr("settings.ignore_oob", "Ignore shapes outside the work area"), &mut prefs.opts.ignore_out_of_bounds);
    ui_kit::checkbox(ui, &tr("settings.ignore_start", "Ignore Start while the monitor is off"), &mut prefs.opts.ignore_start_when_monitor_off);
    ui_kit::checkbox(ui, &tr("settings.default_layers", "Load the default layers on a new project"), &mut prefs.opts.default_layers_on_new);
}

fn work_area(ui: &mut Ui, prefs: &mut Prefs) {
    ui_kit::section_heading(ui, &tr("settings.work_area", "Work area"));
    ui_kit::row(ui, &tr("settings.width", "Width"), |ui| {
        ui_kit::number_field(ui, &mut prefs.page_width, 1.0..=10_000.0, 1.0, " mm");
    });
    ui_kit::row(ui, &tr("settings.height", "Height"), |ui| {
        ui_kit::number_field(ui, &mut prefs.page_height, 1.0..=10_000.0, 1.0, " mm");
    });
    ui_kit::row(ui, &tr("settings.origin", "Origin"), |ui| {
        pick_enum(ui, "origin", &mut prefs.origin, &Origin::ALL, Origin::label);
    });
}

fn files(ui: &mut Ui, prefs: &mut Prefs) {
    let t = Tokens::get();
    ui_kit::section_heading(ui, &tr("settings.files", "Files"));
    crate::file_io::import_options_body(ui, prefs);
    ui.add_space(t.space_12);
    ui_kit::row(ui, &tr("settings.output_tolerance", "Output curve tolerance"), |ui| {
        length_field(ui, &mut prefs.opts.output_tolerance, 0.0001, 10.0, 0.001, " mm")
    });
}

fn text(ui: &mut Ui, prefs: &mut Prefs) {
    ui_kit::section_heading(ui, &tr("settings.default_text", "Default text"));
    ui_kit::row(ui, &tr("settings.font", "Font"), |ui| {
        let mut v = prefs.opts.font_name.clone();
        if ui_kit::field(ui, &mut v) {
            prefs.opts.font_name = v;
        }
    });
    ui_kit::row(ui, &tr("settings.font_height", "Height"), |ui| length_field(ui, &mut prefs.opts.font_height, 0.1, 1000.0, 0.5, " mm"));
    ui_kit::checkbox(ui, &tr("settings.auto_weld", "Weld text automatically"), &mut prefs.opts.auto_weld);
}

fn devices(ui: &mut Ui, prefs: &mut Prefs) {
    let t = Tokens::get();
    ui_kit::section_heading(ui, &tr("settings.ports", "Serial ports"));
    let mut text = prefs.opts.excluded_ports.join(", ");
    ui_kit::row(ui, &tr("settings.excluded_ports", "Never list"), |ui| {
        if ui_kit::hint_field(ui, &mut text, &tr("settings.excluded_hint", "COM1, COM2")) {
            prefs.opts.excluded_ports = text.split(',').map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect();
        }
    });
    ui.add_space(t.space_12);
    ui_kit::section_heading(ui, &tr("settings.capture", "Camera capture"));
    ui_kit::row(ui, &tr("settings.capture_system", "Capture system"), |ui| {
        let mut v = prefs.opts.capture_system.clone();
        if ui_kit::field(ui, &mut v) {
            prefs.opts.capture_system = v;
        }
    });
    ui_kit::row(ui, &tr("settings.capture_width", "Width"), |ui| {
        ui_kit::number_field(ui, &mut prefs.opts.capture_width, 160..=7680, 10.0, " px");
    });
    ui_kit::row(ui, &tr("settings.capture_height", "Height"), |ui| {
        ui_kit::number_field(ui, &mut prefs.opts.capture_height, 120..=4320, 10.0, " px");
    });
    ui_kit::checkbox(ui, &tr("settings.capture_colour", "Show the camera view in colour"), &mut prefs.opts.capture_colour);
}

fn defaults(ui: &mut Ui, prefs: &mut Prefs) {
    let t = Tokens::get();
    ui_kit::section_heading(ui, &tr("settings.shared", "Shared defaults"));
    ui.label(egui::RichText::new(tr("settings.shared_text", "A workshop can give every computer the same starting settings: put a file named defaults.dbprefs next to the program or in the settings folder. It is read when DarkBurn starts with no settings of its own.")).size(t.font_small).color(t.text_dim));
    ui.add_space(t.space_6);
    ui.horizontal_wrapped(|ui| {
        if ui_kit::button(ui, tr("settings.save_defaults", "Save these settings as shared defaults"), ui_kit::Button::Small).clicked()
            && let Some(path) = rfd::FileDialog::new()
                .add_filter("DarkBurn defaults", &[crate::extras::DEFAULTS_EXTENSION])
                .set_file_name(crate::extras::DEFAULTS_FILE)
                .save_file()
        {
            let _ = std::fs::write(path, prefs.to_json());
        }
        if ui_kit::button(ui, tr("settings.reset_all", "Reset all settings"), ui_kit::Button::Small).clicked() {
            let recent = prefs.recent.clone();
            *prefs = Prefs { recent, ..Prefs::default() };
        }
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn picker_offers_the_bundled_themes() {
        let ids = super::theme_ids();
        assert_eq!(&ids[..3], ["system", "dark", "light"]);
        assert!(ids.len() > 20, "SolidCraft's bundled themes are offered: {ids:?}");
        let mut unique = ids.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), ids.len());
    }
}
