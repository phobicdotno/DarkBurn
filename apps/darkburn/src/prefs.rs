//! Preferences: everything DarkBurn remembers between runs. Stored as one JSON string under
//! [`STORAGE_KEY`] in eframe's `app.ron`, the way SolidCraft stores its own.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::canvas::Backdrop;
use crate::panels::{Area, PANELS};
use crate::view::{Origin, View};

pub const STORAGE_KEY: &str = "darkburn.prefs";

/// Toolbars that can be shown or hidden (Window menu). In the ribbon each is a group or a tab.
pub const TOOLBARS: &[(&str, &str)] = &[
    ("main", "Main"),
    ("arrange", "Arrange"),
    ("modifiers", "Modifiers"),
    ("numeric", "Numeric Edits"),
    ("text", "Text Options"),
    ("modes", "Modes"),
    ("docking", "Docking"),
    ("tools", "Tools"),
    ("palette", "Cut Palette"),
];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PanelState {
    pub area: Area,
    pub visible: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    /// `system`, `dark`, `light` or the id of a bundled theme. The default is DarkBurn's own Warning Label.
    pub theme: String,
    /// The canvas colours: the theme's, or always light or dark.
    pub backdrop: Backdrop,
    /// How strong the grid lines are, 0 to 1.
    pub grid_contrast: f32,
    pub show_grid: bool,
    /// Scrolling towards the user zooms in instead of out.
    pub invert_wheel: bool,
    /// Work area size in millimetres.
    pub page_width: f32,
    pub page_height: f32,
    pub origin: Origin,
    pub panels: BTreeMap<String, PanelState>,
    /// The tab in front in each panel area.
    pub active_tab: BTreeMap<Area, String>,
    pub toolbars: BTreeMap<String, bool>,
    pub ribbon_tab: String,
    pub status_bar: bool,
    pub view: View,
    pub language: String,
    /// Shapes on fill layers drawn filled.
    pub filled: bool,
    /// Curves drawn finely (smooth) or coarsely (fast).
    pub smooth: bool,
    /// Recent projects, newest first (at most 24).
    pub recent: Vec<String>,
    /// Auto-save interval in minutes (0 = off).
    pub autosave_minutes: f32,
    /// Layer numbers on the cut palette buttons.
    pub palette_labels: bool,
    /// Numeric edits show inches instead of millimetres.
    pub numeric_inches: bool,
    /// The 9-point anchor numeric edits work about.
    pub anchor: darkburn_core::arrange::Anchor,
    /// Numeric edits keep the width-to-height ratio.
    pub lock_aspect: bool,
    /// The Cut Settings Editor hides its Advanced tab.
    pub cut_beginner: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        let panels = PANELS.iter().map(|p| (p.id.to_string(), PanelState { area: p.home, visible: p.shown_by_default })).collect();
        let toolbars = TOOLBARS.iter().map(|(id, _)| (id.to_string(), true)).collect();
        let active_tab = [(Area::RightTop, "cuts"), (Area::RightBottom, "laser")].into_iter().map(|(a, id)| (a, id.to_string())).collect();
        Prefs {
            theme: "warning-label".into(),
            backdrop: Backdrop::Theme,
            grid_contrast: 0.5,
            show_grid: true,
            invert_wheel: false,
            page_width: 400.0,
            page_height: 400.0,
            origin: Origin::FrontLeft,
            panels,
            active_tab,
            toolbars,
            ribbon_tab: "home".into(),
            status_bar: true,
            view: View::default(),
            language: "en".into(),
            filled: true,
            smooth: true,
            recent: Vec::new(),
            autosave_minutes: 5.0,
            palette_labels: false,
            numeric_inches: false,
            anchor: darkburn_core::arrange::Anchor::Centre,
            lock_aspect: true,
            cut_beginner: false,
        }
    }
}

impl Default for PanelState {
    fn default() -> Self {
        PanelState { area: Area::RightTop, visible: true }
    }
}

impl Prefs {
    /// Preferences from their stored JSON; anything missing or unreadable takes its default.
    pub fn from_json(text: &str) -> Prefs {
        let mut prefs: Prefs = serde_json::from_str(text).unwrap_or_default();
        prefs.sanitise();
        prefs
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// Keep values in range and fill in panels or toolbars added since they were saved.
    pub fn sanitise(&mut self) {
        let d = Prefs::default();
        if !(self.grid_contrast.is_finite()) {
            self.grid_contrast = d.grid_contrast;
        }
        self.grid_contrast = self.grid_contrast.clamp(0.0, 1.0);
        for size in [&mut self.page_width, &mut self.page_height] {
            if !(size.is_finite() && *size >= 1.0 && *size <= 10_000.0) {
                *size = 400.0;
            }
        }
        for (id, state) in d.panels {
            self.panels.entry(id).or_insert(state);
        }
        self.panels.retain(|id, _| PANELS.iter().any(|p| p.id == id));
        for (id, on) in d.toolbars {
            self.toolbars.entry(id).or_insert(on);
        }
        if !(self.view.zoom.is_finite() && self.view.zoom > 0.0 && self.view.offset.is_finite()) {
            self.view = View::default();
        }
    }

    /// Back to the default panel and toolbar arrangement (Window > Reset to default layout).
    pub fn reset_layout(&mut self) {
        let d = Prefs::default();
        self.panels = d.panels;
        self.active_tab = d.active_tab;
        self.toolbars = d.toolbars;
        self.ribbon_tab = d.ribbon_tab;
        self.status_bar = true;
    }

    pub fn toolbar(&self, id: &str) -> bool {
        self.toolbars.get(id).copied().unwrap_or(true)
    }

    pub fn panel(&self, id: &str) -> PanelState {
        self.panels.get(id).cloned().unwrap_or_default()
    }

    pub fn page(&self) -> egui::Vec2 {
        egui::vec2(self.page_width, self.page_height)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_layout_matches_the_checklist() {
        let p = Prefs::default();
        let right_top: Vec<_> = PANELS.iter().filter(|d| p.panel(d.id).visible && p.panel(d.id).area == Area::RightTop).map(|d| d.id).collect();
        let right_bottom: Vec<_> = PANELS.iter().filter(|d| p.panel(d.id).visible && p.panel(d.id).area == Area::RightBottom).map(|d| d.id).collect();
        assert_eq!(right_top, ["cuts", "console", "camera", "variable_text", "shape_properties"]);
        assert_eq!(right_bottom, ["laser", "library"]);
        for hidden in ["move", "art_library", "file_list"] {
            assert!(!p.panel(hidden).visible, "{hidden} is hidden by default");
        }
        assert_eq!((p.page_width, p.page_height, p.origin), (400.0, 400.0, Origin::FrontLeft));
    }

    #[test]
    fn prefs_round_trip_and_survive_bad_input() {
        let mut p = Prefs { theme: "nord".into(), invert_wheel: true, ..Default::default() };
        p.panels.get_mut("move").unwrap().visible = true;
        assert_eq!(Prefs::from_json(&p.to_json()), p);
        assert_eq!(Prefs::from_json("not json"), Prefs::default());
        let odd = Prefs::from_json(r#"{"grid_contrast": 7, "page_width": -3, "panels": {"gone": {"area": "Left", "visible": true}}}"#);
        assert_eq!(odd.grid_contrast, 1.0);
        assert_eq!(odd.page_width, 400.0);
        assert!(!odd.panels.contains_key("gone"));
        assert_eq!(odd.panels.len(), PANELS.len());
    }

    #[test]
    fn reset_layout_restores_panels_but_keeps_settings() {
        let mut p = Prefs { theme: "light".into(), ..Default::default() };
        p.panels.get_mut("cuts").unwrap().visible = false;
        p.toolbars.insert("arrange".into(), false);
        p.reset_layout();
        assert!(p.panel("cuts").visible);
        assert!(p.toolbar("arrange"));
        assert_eq!(p.theme, "light");
    }
}
