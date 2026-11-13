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
    /// The distance between grid lines in mm; 0 follows the zoom.
    pub grid_spacing: f32,
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
    /// The size the user dragged each dock to ("left", "right", "bottom"); none: the default.
    pub dock_sizes: BTreeMap<String, f32>,
    /// Where each dockable window was left (floating, its own window or a tab) and its size.
    #[serde(deserialize_with = "darkburn_ui::ui_kit::window_host::lenient")]
    pub windows: darkburn_ui::ui_kit::window_host::WindowPlaces,
    pub toolbars: BTreeMap<String, bool>,
    /// The order the toolbars appear in within a ribbon tab (ids; empty: the default order).
    pub toolbar_order: Vec<String>,
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
    /// Pixels per inch of SVG files without units: 72 or 96.
    pub svg_dpi: f32,
    /// Imported shapes are grouped, one group per file.
    pub import_group: bool,
    /// Imported shapes are selected.
    pub import_select: bool,
    /// Imported colours may land on the tool layers T1 and T2.
    pub import_tool_layers: bool,
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
    /// Everything in the Settings window that is not layout (options.rs).
    pub opts: crate::options::Options,
    /// Changes to the default keys: command id to key text, empty for no key (keymap.rs).
    pub hotkeys: crate::keymap::Overrides,
    /// The cut library file each device last used (empty key: no device).
    pub library_paths: BTreeMap<String, String>,
    /// The art libraries that were open, to open again at the next start (art_library.rs).
    pub art_libraries: Vec<String>,
    /// Import, export and file preferences (formats.rs).
    pub files: crate::formats::FilePrefs,
    /// AI agents (MCP): off until the user turns it on (mcp_ui.rs).
    pub mcp: crate::mcp_ui::McpPrefs,
    /// Recent and favourite fonts, default font and size for new text (text_ui.rs).
    pub fonts: darkburn_text::FontPrefs,
    /// SHX single-line fonts the user added: "shx:" and the file path.
    pub shx_fonts: Vec<String>,
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
            grid_spacing: 10.0,
            show_grid: true,
            invert_wheel: false,
            page_width: 400.0,
            page_height: 400.0,
            origin: Origin::FrontLeft,
            panels,
            active_tab,
            dock_sizes: BTreeMap::new(),
            windows: Default::default(),
            toolbars,
            toolbar_order: Vec::new(),
            ribbon_tab: "home".into(),
            status_bar: true,
            view: View::default(),
            language: "en".into(),
            filled: true,
            smooth: true,
            recent: Vec::new(),
            autosave_minutes: 5.0,
            svg_dpi: 96.0,
            import_group: true,
            import_select: true,
            import_tool_layers: false,
            palette_labels: false,
            numeric_inches: false,
            anchor: darkburn_core::arrange::Anchor::Centre,
            lock_aspect: true,
            cut_beginner: false,
            opts: crate::options::Options::default(),
            hotkeys: Default::default(),
            library_paths: BTreeMap::new(),
            art_libraries: Vec::new(),
            files: Default::default(),
            mcp: Default::default(),
            fonts: Default::default(),
            shx_fonts: Vec::new(),
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
        let mut prefs: Prefs = serde_json::from_str(text).unwrap_or_else(|_| Prefs::salvage(text));
        prefs.sanitise();
        prefs
    }

    /// Preferences from JSON that does not read as a whole: one value of a kind this version
    /// does not know (a setting that changed since the file was written, a file edited by
    /// hand) must not cost every other setting. Each value that can be read is kept; the ones
    /// that cannot take their defaults. Text that is not a JSON object gives the defaults.
    fn salvage(text: &str) -> Prefs {
        use serde_json::Value;
        fn at<'a>(v: &'a mut Value, path: &[&str]) -> Option<&'a mut Value> {
            path.iter().try_fold(v, |v, key| v.get_mut(*key))
        }
        /// Take the values of `file` at `path` over into `good`, as long as `good` still reads.
        fn take(good: &mut Value, file: &Value, path: &mut Vec<String>) {
            let Value::Object(fields) = file else { return };
            for (key, value) in fields {
                let keys: Vec<&str> = path.iter().map(String::as_str).collect();
                let Some(Value::Object(parent)) = at(good, &keys) else { return };
                let before = parent.insert(key.clone(), value.clone());
                if serde_json::from_value::<Prefs>(good.clone()).is_ok() {
                    continue;
                }
                // Not readable with this value: put the old one back, and look inside it for
                // the parts that are (a settings group with one bad value keeps the rest).
                let Some(Value::Object(parent)) = at(good, &keys) else { return };
                match before {
                    Some(old) => {
                        let nested = old.is_object() && value.is_object() && path.len() < 3;
                        parent.insert(key.clone(), old);
                        if nested {
                            path.push(key.clone());
                            take(good, value, path);
                            path.pop();
                        }
                    }
                    None => {
                        parent.remove(key);
                    }
                }
            }
        }
        let Ok(file @ Value::Object(_)) = serde_json::from_str::<Value>(text) else { return Prefs::default() };
        let Ok(mut good) = serde_json::to_value(Prefs::default()) else { return Prefs::default() };
        take(&mut good, &file, &mut Vec::new());
        serde_json::from_value(good).unwrap_or_default()
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
        if !(self.grid_spacing.is_finite()) {
            self.grid_spacing = d.grid_spacing;
        }
        self.grid_spacing = self.grid_spacing.clamp(0.0, 1000.0);
        for size in [&mut self.page_width, &mut self.page_height] {
            if !(size.is_finite() && *size >= 1.0 && *size <= 10_000.0) {
                *size = 400.0;
            }
        }
        if self.svg_dpi != 72.0 {
            self.svg_dpi = 96.0;
        }
        if !self.autosave_minutes.is_finite() {
            self.autosave_minutes = d.autosave_minutes;
        }
        self.autosave_minutes = self.autosave_minutes.clamp(0.0, 240.0);
        self.opts.sanitise();
        self.hotkeys.retain(|id, key| {
            crate::keymap::bindable().iter().any(|(i, _, _)| i == id) && (key.is_empty() || crate::commands::parse_key(key).is_some())
        });
        self.recent.truncate(crate::files::RECENT_MAX);
        self.files.sanitise();
        for (id, state) in d.panels {
            self.panels.entry(id).or_insert(state);
        }
        self.panels.retain(|id, _| PANELS.iter().any(|p| p.id == id));
        self.dock_sizes.retain(|_, size| size.is_finite() && *size > 0.0);
        darkburn_ui::ui_kit::window_host::sanitise(&mut self.windows);
        for (id, on) in d.toolbars {
            self.toolbars.entry(id).or_insert(on);
        }
        if !self.toolbar_order.is_empty() {
            self.toolbar_order = self.full_toolbar_order();
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
        self.dock_sizes = d.dock_sizes;
        self.windows = d.windows;
        self.toolbars = d.toolbars;
        self.toolbar_order = Vec::new();
        self.ribbon_tab = d.ribbon_tab;
        self.status_bar = true;
    }

    /// Every toolbar id once, in the user's order (unknown ids dropped, missing ones last).
    pub fn full_toolbar_order(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for id in &self.toolbar_order {
            if TOOLBARS.iter().any(|(t, _)| t == id) && !out.contains(id) {
                out.push(id.clone());
            }
        }
        for (id, _) in TOOLBARS {
            if !out.iter().any(|o| o == id) {
                out.push((*id).to_string());
            }
        }
        out
    }

    /// Where toolbar `id` sits in the order (for sorting ribbon groups).
    pub fn toolbar_rank(&self, id: &str) -> usize {
        self.full_toolbar_order().iter().position(|t| t == id).unwrap_or(usize::MAX)
    }

    /// Move toolbar `id` earlier (`delta` < 0) or later (`delta` > 0) by that many places.
    /// Returns whether it moved.
    pub fn move_toolbar(&mut self, id: &str, delta: i32) -> bool {
        let mut order = self.full_toolbar_order();
        let Some(at) = order.iter().position(|t| t == id) else { return false };
        let to = (at as i32 + delta).clamp(0, order.len() as i32 - 1) as usize;
        if to == at {
            return false;
        }
        let item = order.remove(at);
        order.insert(to, item);
        self.toolbar_order = order;
        true
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
        let mut docked = Prefs::default();
        docked.windows.entry("settings".into()).or_default().dock("RightTop");
        docked.windows.entry("db_hotkeys".into()).or_default().resize(egui::vec2(640.0, 480.0));
        assert_eq!(Prefs::from_json(&docked.to_json()), docked, "window places survive a restart");
        // One entry this version cannot read costs that window its place, not the others theirs.
        let odd = Prefs::from_json(
            r#"{"windows": {"settings": {"placement": "docked"}, "x": 5, "y": {"size": [1e9, -1]},
                "db_devices": {"placement": {"docked": "Left"}, "size": [500, 400]}}}"#,
        );
        let windows: Vec<&str> = odd.windows.keys().map(String::as_str).collect();
        assert_eq!(windows, ["db_devices", "settings", "x", "y"]);
        assert_eq!(odd.windows["settings"], Default::default(), "a placement it cannot read: a floating card");
        assert_eq!(odd.windows["x"], Default::default());
        assert_eq!(odd.windows["y"].size, Some([8192.0, 180.0]), "a size is held to the limits");
        assert_eq!((odd.windows["db_devices"].area(), odd.windows["db_devices"].size), (Some("Left"), Some([500.0, 400.0])));
        assert_eq!(Prefs::from_json("not json"), Prefs::default());
        let odd = Prefs::from_json(r#"{"grid_contrast": 7, "page_width": -3, "panels": {"gone": {"area": "Left", "visible": true}}}"#);
        assert_eq!(odd.grid_contrast, 1.0);
        assert_eq!(odd.page_width, 400.0);
        assert!(!odd.panels.contains_key("gone"));
        assert_eq!(odd.panels.len(), PANELS.len());
        let odd = Prefs::from_json(r#"{"svg_dpi": 80, "autosave_minutes": -2}"#);
        assert_eq!((odd.svg_dpi, odd.autosave_minutes), (96.0, 0.0));
        assert_eq!(Prefs::from_json(r#"{"svg_dpi": 72}"#).svg_dpi, 72.0);
    }

    /// A preferences file with one value this version cannot read keeps everything else: it
    /// used to give the defaults for every setting, without a word, also when it was imported.
    #[test]
    fn one_unreadable_value_does_not_cost_the_other_settings() {
        let mut p = Prefs { theme: "nord".into(), invert_wheel: true, autosave_minutes: 9.0, ..Default::default() };
        p.opts.nudge = 2.5;
        p.opts.sound_done = true;
        p.files.dxf_arcs = false;
        p.panels.get_mut("move").unwrap().visible = true;
        p.recent = vec!["a.dbrn".into()];
        let mut v: serde_json::Value = serde_json::from_str(&p.to_json()).unwrap();
        // A setting that was a number in another version, an option that is no longer a choice,
        // and a whole group of the wrong kind.
        v["anchor"] = serde_json::json!(7);
        v["opts"]["icon_size"] = serde_json::json!("Enormous");
        v["opts"]["locks"] = serde_json::json!("none");
        v["fonts"] = serde_json::json!([1, 2, 3]);
        v["something_new"] = serde_json::json!({ "from": "a later version" });
        let got = Prefs::from_json(&v.to_string());
        assert_eq!(got, p, "the readable settings were lost with the unreadable ones");
        // The same through a preferences file (import).
        let file = serde_json::json!({ "format": crate::prefs_files::FORMAT, "version": 1, "prefs": v });
        assert_eq!(crate::prefs_files::from_file_text(&file.to_string()).unwrap(), p);
        // A file of an older version: settings it does not have take their defaults.
        let old = Prefs::from_json(r#"{"theme":"light","opts":{"nudge":3},"panels":{"cuts":{"visible":false}}}"#);
        let want = {
            let mut d = Prefs { theme: "light".into(), ..Default::default() };
            d.opts.nudge = 3.0;
            d.panels.get_mut("cuts").unwrap().visible = false;
            d
        };
        assert_eq!(old.theme, "light");
        assert_eq!((old.opts.clone(), old.files.clone(), old.hotkeys.clone()), (want.opts.clone(), want.files.clone(), want.hotkeys.clone()));
        assert!(!old.panel("cuts").visible && old.panels.len() == PANELS.len());
        // Damaged: cut off half way, or not preferences at all.
        let text = p.to_json();
        assert_eq!(Prefs::from_json(&text[..text.len() / 2]), Prefs::default());
        assert_eq!(Prefs::from_json("[1, 2]"), Prefs::default());
        assert_eq!(Prefs::from_json(""), Prefs::default());
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
