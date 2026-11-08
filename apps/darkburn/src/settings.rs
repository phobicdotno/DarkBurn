//! The Settings window: theme, canvas and work area. More pages come with later phases.

use std::sync::OnceLock;

use darkburn_ui::ui_kit;
use darkburn_ui::{fonts, theme_files};
use egui::{Color32, Rect, Sense, Stroke, Ui, vec2};
use solidcraft_theme::Tokens;

use crate::canvas::Backdrop;
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
                &c.name,
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

/// The window's body.
pub fn page(ui: &mut Ui, prefs: &mut Prefs) {
    let t = Tokens::get();
    ui.spacing_mut().item_spacing.y = t.form_gap_y;
    ui_kit::section_heading(ui, "Theme");
    theme_picker(ui, prefs);
    ui.add_space(t.space_12);
    ui_kit::section_heading(ui, "Canvas");
    ui_kit::row(ui, "Background", |ui| {
        let mut i = Backdrop::ALL.iter().position(|b| *b == prefs.backdrop).unwrap_or(0);
        let labels: Vec<_> = Backdrop::ALL.iter().map(|b| b.label()).collect();
        ui_kit::dropdown(ui, "backdrop", &mut i, &labels);
        prefs.backdrop = Backdrop::ALL[i];
    });
    ui_kit::row(ui, "Grid contrast", |ui| {
        ui_kit::slider(ui, &mut prefs.grid_contrast, 0.0..=1.0, ui_kit::Slider { format: |v| format!("{:.0} %", v * 100.0), ..Default::default() });
    });
    ui_kit::checkbox(ui, "Show the grid", &mut prefs.show_grid);
    ui_kit::checkbox(ui, "Invert the mouse wheel zoom", &mut prefs.invert_wheel);
    ui.add_space(t.space_12);
    ui_kit::section_heading(ui, "Work area");
    ui_kit::row(ui, "Width", |ui| {
        ui_kit::number_field(ui, &mut prefs.page_width, 1.0..=10_000.0, 1.0, " mm");
    });
    ui_kit::row(ui, "Height", |ui| {
        ui_kit::number_field(ui, &mut prefs.page_height, 1.0..=10_000.0, 1.0, " mm");
    });
    ui_kit::row(ui, "Origin", |ui| {
        let mut i = Origin::ALL.iter().position(|o| *o == prefs.origin).unwrap_or(0);
        let labels: Vec<_> = Origin::ALL.iter().map(|o| o.label()).collect();
        ui_kit::dropdown(ui, "origin", &mut i, &labels);
        prefs.origin = Origin::ALL[i];
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
