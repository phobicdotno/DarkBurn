//! The dock panels and the areas that hold them. Each area (left, right top, right bottom,
//! bottom) shows its visible panels as tabs. Panels fill in as their phases arrive; for now each
//! shows its controls, disabled, so the layout can be judged.

use darkburn_ui::ui_kit;
use egui::{RichText, Ui};
use serde::{Deserialize, Serialize};
use solidcraft_theme::Tokens;

/// Where a panel is docked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Area {
    Left,
    RightTop,
    RightBottom,
    Bottom,
}

impl Area {
    pub const ALL: [Area; 4] = [Area::Left, Area::RightTop, Area::RightBottom, Area::Bottom];

    pub fn label(self) -> &'static str {
        match self {
            Area::Left => "Left",
            Area::RightTop => "Right, top",
            Area::RightBottom => "Right, bottom",
            Area::Bottom => "Bottom",
        }
    }
}

pub struct PanelDef {
    pub id: &'static str,
    pub title: &'static str,
    pub home: Area,
    pub shown_by_default: bool,
}

const fn p(id: &'static str, title: &'static str, home: Area, shown_by_default: bool) -> PanelDef {
    PanelDef { id, title, home, shown_by_default }
}

/// In tab order within each area.
pub const PANELS: &[PanelDef] = &[
    p("cuts", "Cuts / Layers", Area::RightTop, true),
    p("console", "Console", Area::RightTop, true),
    p("camera", "Camera Control", Area::RightTop, true),
    p("variable_text", "Variable Text", Area::RightTop, true),
    p("shape_properties", "Shape Properties", Area::RightTop, true),
    p("laser", "Laser", Area::RightBottom, true),
    p("library", "Library", Area::RightBottom, true),
    p("move", "Move", Area::RightBottom, false),
    p("art_library", "Art Library", Area::Left, false),
    p("file_list", "File List", Area::Bottom, false),
];

#[cfg(test)]
pub fn def(id: &str) -> Option<&'static PanelDef> {
    PANELS.iter().find(|p| p.id == id)
}

/// A note at the top of a panel that is not working yet.
fn coming(ui: &mut Ui, text: &str) {
    let t = Tokens::get();
    ui.label(RichText::new(text).size(t.font_small).color(t.text_dim));
    ui.add_space(t.space_6);
}

fn field_row(ui: &mut Ui, label: &str, value: &str) {
    ui_kit::row(ui, label, |ui| {
        let mut v = value.to_string();
        ui_kit::field(ui, &mut v);
    });
}

fn buttons(ui: &mut Ui, labels: &[&str]) {
    ui.horizontal_wrapped(|ui| {
        for l in labels {
            ui_kit::button(ui, *l, ui_kit::Button::Small);
        }
    });
}

fn checks(ui: &mut Ui, labels: &[&str]) {
    for l in labels {
        let mut on = false;
        ui_kit::checkbox(ui, l, &mut on);
    }
}

fn choice(ui: &mut Ui, label: &str, id: &str, options: &[&str]) {
    ui_kit::row(ui, label, |ui| {
        let mut i = 0;
        ui_kit::dropdown(ui, id, &mut i, options);
    });
}

/// The contents of panel `id`. Everything is disabled until the panel's phase.
pub fn contents(ui: &mut Ui, id: &str) {
    let t = Tokens::get();
    ui.spacing_mut().item_spacing.y = t.form_gap_y;
    ui.add_enabled_ui(false, |ui| match id {
        "cuts" => {
            let columns = [
                ui_kit::Column::new("#", 0.5),
                ui_kit::Column::new("Layer", 1.0).flex(),
                ui_kit::Column::new("Mode", 0.8),
                ui_kit::Column::new("Spd/Pwr", 0.9),
                ui_kit::Column::new("Output", 0.7),
                ui_kit::Column::new("Show", 0.6),
                ui_kit::Column::new("Air", 0.5),
            ];
            ui_kit::table_custom(ui, "cuts", &columns, "No layers yet. Shapes you draw add their layer here.", |_, _| {});
            ui.add_space(t.space_8);
            ui_kit::section_heading(ui, "Cut settings");
            field_row(ui, "Speed", "100 mm/s");
            field_row(ui, "Max power", "20 %");
            field_row(ui, "Min power", "10 %");
            field_row(ui, "Interval", "0.1 mm");
            field_row(ui, "Passes", "1");
        }
        "console" => {
            coming(ui, "The machine's replies appear here once a laser is connected.");
            let mut log = String::new();
            ui_kit::text_area(ui, "Log", egui::Id::unique("console_log"), &mut log, 6, 10_000);
            ui_kit::row(ui, "Command", |ui| {
                let mut v = String::new();
                ui_kit::hint_field(ui, &mut v, "Type a command");
            });
            checks(ui, &["Show all"]);
            buttons(ui, &["Macro 1", "Macro 2", "Macro 3", "Macro 4", "Macro 5", "Macro 6"]);
        }
        "camera" => {
            coming(ui, "Camera overlay arrives with camera support.");
            choice(ui, "Camera", "camera_pick", &["No camera"]);
            buttons(ui, &["Update overlay", "Trace", "Save settings"]);
            checks(ui, &["Fade", "Show"]);
            field_row(ui, "Width", "100 %");
            field_row(ui, "Height", "100 %");
            field_row(ui, "X shift", "0 mm");
            field_row(ui, "Y shift", "0 mm");
        }
        "variable_text" => {
            field_row(ui, "Start", "0");
            field_row(ui, "End", "100");
            field_row(ui, "Current", "0");
            field_row(ui, "Advance by", "1");
            checks(ui, &["Auto-advance"]);
            buttons(ui, &["Previous", "Next", "Test", "Reset", "Bake"]);
            ui_kit::row(ui, "CSV file", |ui| {
                let mut v = String::new();
                ui_kit::hint_field(ui, &mut v, "No file");
            });
            buttons(ui, &["Browse", "Clear"]);
        }
        "shape_properties" => {
            coming(ui, "Select a shape to see its properties.");
        }
        "laser" => {
            ui_kit::status_line(ui, "Disconnected", ui_kit::StatusTone::Dim);
            ui_kit::progress(ui, "laser_job", 0.0);
            buttons(ui, &["Start", "Pause", "Stop"]);
            buttons(ui, &["Frame", "Save G-code", "Run G-code", "Send"]);
            buttons(ui, &["Home", "Go to origin"]);
            choice(ui, "Start from", "start_from", &["Absolute coords", "Current position", "User origin"]);
            checks(ui, &["Cut selected graphics", "Use selection origin", "Show last position", "Optimize cut path"]);
            buttons(ui, &["Devices"]);
            choice(ui, "Port", "port", &["(choose)"]);
            choice(ui, "Device", "device", &["No device"]);
        }
        "library" => {
            coming(ui, "Material library entries will be listed here.");
            buttons(ui, &["New", "Load", "Save", "Save as"]);
            buttons(ui, &["Assign to layer", "Create from layer", "Edit", "Delete"]);
        }
        "move" => {
            coming(ui, "Jog the laser once one is connected.");
            buttons(ui, &["↖", "↑", "↗"]);
            buttons(ui, &["←", "Home", "→"]);
            buttons(ui, &["↙", "↓", "↘"]);
            field_row(ui, "Distance", "10 mm");
            field_row(ui, "Speed", "100 mm/s");
            checks(ui, &["Continuous jog"]);
            buttons(ui, &["Get position", "Move to", "Set origin", "Clear origin", "Fire"]);
        }
        "art_library" => {
            coming(ui, "Art libraries will open here.");
            ui_kit::row(ui, "Search", |ui| {
                let mut v = String::new();
                ui_kit::hint_field(ui, &mut v, "Search art");
            });
            buttons(ui, &["Load library", "Add selection"]);
        }
        "file_list" => {
            coming(ui, "Files stored on a DSP controller are listed here.");
            buttons(ui, &["Refresh", "Upload", "Start", "Delete", "Delete all", "Calc time"]);
        }
        _ => {}
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_panel_is_unique_and_has_a_home() {
        let mut ids = std::collections::HashSet::new();
        for p in PANELS {
            assert!(ids.insert(p.id));
            assert!(def(p.id).is_some());
        }
        assert_eq!(PANELS.len(), 10);
    }
}
