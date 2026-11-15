//! Preferences: everything DarkBurn remembers between runs. Stored as one JSON string under
//! [`STORAGE_KEY`] in eframe's `app.ron`, the way SolidCraft stores its own.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::canvas::Backdrop;
use crate::panels::{Area, PANELS, PanelDef, Place};
use crate::view::{Origin, View};

pub const STORAGE_KEY: &str = "darkburn.prefs";

/// The panel layout's version. Settings saved with an older one are moved on once when read
/// (see [`Prefs::migrate_layout`]). 1: the right dock is one tab bar (Laser, Library and Move
/// used to sit in a second group below it).
pub const LAYOUT_VERSION: u32 = 1;

/// The smallest and largest a floating tab group may be, in points.
pub const FLOAT_MIN: [f32; 2] = [200.0, 120.0];
pub const FLOAT_MAX: [f32; 2] = [8192.0, 8192.0];

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
    /// The docked area; also where the panel goes back to when its floating group is gone.
    pub area: Area,
    pub visible: bool,
    /// The floating tab group it is in (an id in [`Prefs::floating`]); none: docked in `area`.
    pub floating: Option<u32>,
}

/// A tab group floating in the main window: where it is, its size and the tab in front.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FloatGroup {
    /// Its top-left corner in the main window, in points.
    pub pos: [f32; 2],
    pub size: [f32; 2],
    /// The tab in front (a panel id or a window's tab id).
    pub active: String,
}

impl Default for FloatGroup {
    fn default() -> Self {
        FloatGroup { pos: [200.0, 160.0], size: [380.0, 420.0], active: String::new() }
    }
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
    /// The version of the panel layout these settings were saved with ([`LAYOUT_VERSION`]).
    pub layout_version: u32,
    /// The floating tab groups, by id.
    pub floating: BTreeMap<u32, FloatGroup>,
    /// The order of the tabs in each place ([`Place::key`]), as panel ids; panels not named
    /// follow in their usual order.
    pub tab_order: BTreeMap<String, Vec<String>>,
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
    /// Where each toolbar was moved to (an edge or floating); none: its usual place [TB-25].
    pub toolbar_places: crate::toolbar_dock::ToolbarPlaces,
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
        let panels = PANELS.iter().map(|p| (p.id.to_string(), PanelState { area: p.home, visible: p.shown_by_default, floating: None })).collect();
        let toolbars = TOOLBARS.iter().map(|(id, _)| (id.to_string(), true)).collect();
        let active_tab = [(Area::RightTop, "cuts")].into_iter().map(|(a, id)| (a, id.to_string())).collect();
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
            layout_version: LAYOUT_VERSION,
            floating: BTreeMap::new(),
            tab_order: BTreeMap::new(),
            active_tab,
            dock_sizes: BTreeMap::new(),
            windows: Default::default(),
            toolbars,
            toolbar_order: Vec::new(),
            toolbar_places: BTreeMap::new(),
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
        PanelState { area: Area::RightTop, visible: true, floating: None }
    }
}

impl Prefs {
    /// Preferences from their stored JSON; anything missing or unreadable takes its default.
    pub fn from_json(text: &str) -> Prefs {
        let mut prefs: Prefs = serde_json::from_str(text).unwrap_or_else(|_| Prefs::salvage(text));
        // Settings without a layout version are from before there was one (the defaults filled it in).
        let saved = serde_json::from_str::<serde_json::Value>(text)
            .ok()
            .and_then(|v| v.get("layout_version").and_then(serde_json::Value::as_u64))
            .map_or(0, |v| v.min(u32::MAX as u64) as u32);
        prefs.layout_version = saved;
        prefs.migrate_layout();
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

    /// Move a layout saved by an older version on to this one, once.
    pub fn migrate_layout(&mut self) {
        if self.layout_version < 1 {
            // One tab bar on the right: the panels of the lower group join the upper one.
            for state in self.panels.values_mut() {
                if state.area == Area::RightBottom {
                    state.area = Area::RightTop;
                }
            }
            self.active_tab.remove(&Area::RightBottom);
            self.tab_order.remove(&Place::Docked(Area::RightBottom).key());
        }
        self.layout_version = self.layout_version.max(LAYOUT_VERSION);
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
        self.hotkeys.retain(|id, key| crate::keymap::slot(id).is_some() && (key.is_empty() || crate::keymap::parse_binding(key).is_some()));
        self.recent.truncate(crate::files::RECENT_MAX);
        self.files.sanitise();
        for (id, state) in d.panels {
            self.panels.entry(id).or_insert(state);
        }
        self.panels.retain(|id, _| PANELS.iter().any(|p| p.id == id));
        for g in self.floating.values_mut() {
            for (i, v) in g.pos.iter_mut().enumerate() {
                if !v.is_finite() {
                    *v = FloatGroup::default().pos[i];
                }
                *v = v.clamp(-FLOAT_MAX[i], FLOAT_MAX[i]);
            }
            for (i, v) in g.size.iter_mut().enumerate() {
                if !v.is_finite() {
                    *v = FloatGroup::default().size[i];
                }
                *v = v.clamp(FLOAT_MIN[i], FLOAT_MAX[i]);
            }
        }
        let groups: Vec<u32> = self.floating.keys().copied().collect();
        for state in self.panels.values_mut() {
            if state.floating.is_some_and(|g| !groups.contains(&g)) {
                state.floating = None;
            }
        }
        self.prune_floating();
        self.tab_order.retain(|key, ids| {
            let mut seen = std::collections::BTreeSet::new();
            ids.retain(|id| PANELS.iter().any(|p| p.id == id) && seen.insert(id.clone()));
            Place::from_key(key).is_some() && !ids.is_empty()
        });
        self.dock_sizes.retain(|_, size| size.is_finite() && *size > 0.0);
        darkburn_ui::ui_kit::window_host::sanitise(&mut self.windows);
        for (id, on) in d.toolbars {
            self.toolbars.entry(id).or_insert(on);
        }
        if !self.toolbar_order.is_empty() {
            self.toolbar_order = self.full_toolbar_order();
        }
        crate::toolbar_dock::sanitise(&mut self.toolbar_places);
        if !(self.view.zoom.is_finite() && self.view.zoom > 0.0 && self.view.offset.is_finite()) {
            self.view = View::default();
        }
    }

    /// Back to the default panel and toolbar arrangement (Window > Reset to default layout).
    pub fn reset_layout(&mut self) {
        let d = Prefs::default();
        self.panels = d.panels;
        self.floating = d.floating;
        self.tab_order = d.tab_order;
        self.active_tab = d.active_tab;
        self.dock_sizes = d.dock_sizes;
        self.windows = d.windows;
        self.toolbars = d.toolbars;
        self.toolbar_order = Vec::new();
        self.toolbar_places = d.toolbar_places;
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

    /// Where panel `id` is: its floating group, else its docked area.
    pub fn place(&self, id: &str) -> Place {
        let state = self.panel(id);
        match state.floating {
            Some(g) if self.floating.contains_key(&g) => Place::Floating(g),
            _ => Place::Docked(state.area),
        }
    }

    /// Every panel in `place`, shown or not, in its tab order.
    pub fn panels_in(&self, place: Place) -> Vec<&'static PanelDef> {
        let here: Vec<&'static PanelDef> = PANELS.iter().filter(|p| self.place(p.id) == place).collect();
        let mut out: Vec<&'static PanelDef> = Vec::new();
        for id in self.tab_order.get(&place.key()).into_iter().flatten() {
            if let Some(p) = here.iter().find(|p| p.id == id)
                && !out.iter().any(|o| o.id == p.id)
            {
                out.push(p);
            }
        }
        for p in here {
            if !out.iter().any(|o| o.id == p.id) {
                out.push(p);
            }
        }
        out
    }

    /// The shown panels in `place`, in tab order.
    pub fn visible_in(&self, place: Place) -> Vec<&'static PanelDef> {
        self.panels_in(place).into_iter().filter(|p| self.panel(p.id).visible).collect()
    }

    /// The tab stored as in front in `place`, if any.
    pub fn front(&self, place: Place) -> Option<String> {
        match place {
            Place::Docked(area) => self.active_tab.get(&area).cloned(),
            Place::Floating(g) => self.floating.get(&g).map(|g| g.active.clone()).filter(|a| !a.is_empty()),
        }
    }

    /// Tab `id` (a panel's or a window's) is in front in `place`.
    pub fn set_front(&mut self, place: Place, id: &str) {
        match place {
            Place::Docked(area) => {
                self.active_tab.insert(area, id.to_string());
            }
            Place::Floating(g) => {
                if let Some(g) = self.floating.get_mut(&g) {
                    g.active = id.to_string();
                }
            }
        }
    }

    /// Panel `id` comes to the front wherever it is.
    pub fn bring_panel_forward(&mut self, id: &str) {
        self.set_front(self.place(id), id);
    }

    /// Move panel `id` to `to`, at tab position `at` there (none: last), shown and in front.
    /// Moving within its own place reorders. A floating group left empty is gone.
    /// Returns whether the panel exists.
    pub fn move_panel(&mut self, id: &str, to: Place, at: Option<usize>) -> bool {
        if !self.panels.contains_key(id) || matches!(to, Place::Floating(g) if !self.floating.contains_key(&g)) {
            return false;
        }
        let mut order: Vec<String> = self.panels_in(to).iter().map(|p| p.id.to_string()).filter(|p| p != id).collect();
        let at = at.unwrap_or(order.len()).min(order.len());
        order.insert(at, id.to_string());
        for ids in self.tab_order.values_mut() {
            ids.retain(|p| p != id);
        }
        self.tab_order.retain(|_, ids| !ids.is_empty());
        self.tab_order.insert(to.key(), order);
        if let Some(state) = self.panels.get_mut(id) {
            state.visible = true;
            match to {
                Place::Docked(area) => (state.area, state.floating) = (area, None),
                Place::Floating(g) => state.floating = Some(g),
            }
        }
        self.set_front(to, id);
        self.prune_floating();
        true
    }

    /// Panel `id` floats in a new tab group of its own at `pos` with `size` (clamped). Returns
    /// the new group's id, or none for an unknown panel.
    #[cfg_attr(not(test), allow(dead_code))] // the tab drag uses it next
    pub fn float_panel(&mut self, id: &str, pos: [f32; 2], size: [f32; 2]) -> Option<u32> {
        if !self.panels.contains_key(id) {
            return None;
        }
        let g = self.floating.keys().next_back().map_or(0, |last| last + 1);
        let size = [size[0].clamp(FLOAT_MIN[0], FLOAT_MAX[0]), size[1].clamp(FLOAT_MIN[1], FLOAT_MAX[1])];
        self.floating.insert(g, FloatGroup { pos, size, active: id.to_string() });
        self.move_panel(id, Place::Floating(g), None);
        Some(g)
    }

    /// Close floating group `g`: its panels are hidden and go back to their docked area.
    pub fn close_floating(&mut self, g: u32) {
        for state in self.panels.values_mut() {
            if state.floating == Some(g) {
                (state.floating, state.visible) = (None, false);
            }
        }
        self.floating.remove(&g);
        self.tab_order.remove(&Place::Floating(g).key());
    }

    /// Floating groups without a shown panel are gone; their hidden panels go back to their
    /// docked area.
    pub fn prune_floating(&mut self) {
        let empty: Vec<u32> = self.floating.keys().copied().filter(|g| !self.panels.values().any(|s| s.floating == Some(*g) && s.visible)).collect();
        for g in empty {
            self.close_floating(g);
        }
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
        assert_eq!(right_top, ["cuts", "console", "camera", "variable_text", "shape_properties", "laser", "library"]);
        assert!(right_bottom.is_empty(), "one tab bar on the right: {right_bottom:?}");
        assert_eq!(p.layout_version, LAYOUT_VERSION);
        assert!(p.floating.is_empty() && p.tab_order.is_empty());
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

    /// Settings saved before the right dock became one tab bar move their lower group up, once;
    /// a panel the user puts in the lower area after that stays there.
    #[test]
    fn old_layouts_move_the_lower_right_group_up_once() {
        let old = r#"{"theme":"nord","panels":{"laser":{"area":"RightBottom","visible":true},"library":{"area":"RightBottom","visible":false},
            "move":{"area":"Left","visible":true}},"active_tab":{"RightTop":"console","RightBottom":"laser"}}"#;
        let p = Prefs::from_json(old);
        assert_eq!(p.theme, "nord");
        assert_eq!(p.layout_version, LAYOUT_VERSION);
        assert_eq!((p.panel("laser").area, p.panel("laser").visible), (Area::RightTop, true));
        assert_eq!((p.panel("library").area, p.panel("library").visible), (Area::RightTop, false));
        assert_eq!(p.panel("move").area, Area::Left, "panels elsewhere stay where they were");
        assert_eq!(p.active_tab.get(&Area::RightTop).map(String::as_str), Some("console"));
        assert!(!p.active_tab.contains_key(&Area::RightBottom));
        // Saved again with the new version: a panel moved down by hand stays down.
        let mut q = p.clone();
        q.move_panel("laser", Place::Docked(Area::RightBottom), None);
        let back = Prefs::from_json(&q.to_json());
        assert_eq!(back.panel("laser").area, Area::RightBottom);
        assert_eq!(back, q);
        // Damaged or salvaged settings without a version are moved on too.
        let salvaged = Prefs::from_json(r#"{"anchor": 7, "panels":{"laser":{"area":"RightBottom","visible":true}}}"#);
        assert_eq!(salvaged.panel("laser").area, Area::RightTop);
    }

    #[test]
    fn tab_order_follows_moves_and_survives_a_restart() {
        let mut p = Prefs::default();
        let top = Place::Docked(Area::RightTop);
        let ids = |p: &Prefs, at: Place| p.visible_in(at).iter().map(|d| d.id).collect::<Vec<_>>();
        assert_eq!(ids(&p, top), ["cuts", "console", "camera", "variable_text", "shape_properties", "laser", "library"]);
        // Reorder within the bar: Laser to the front.
        assert!(p.move_panel("laser", top, Some(0)));
        assert_eq!(ids(&p, top)[..3], ["laser", "cuts", "console"]);
        assert_eq!(p.front(top).as_deref(), Some("laser"));
        // To another area at a position; a hidden panel moved is shown.
        let left = Place::Docked(Area::Left);
        assert!(p.move_panel("move", left, None));
        assert!(p.move_panel("console", left, Some(0)));
        assert_eq!(ids(&p, left), ["console", "move"]);
        assert!(p.panel("move").visible);
        assert!(!ids(&p, top).contains(&"console"));
        let back = Prefs::from_json(&p.to_json());
        assert_eq!(back, p);
        assert_eq!(ids(&back, top)[..2], ["laser", "cuts"]);
        // Unknown panels and groups are refused.
        assert!(!p.move_panel("nope", top, None));
        assert!(!p.move_panel("cuts", Place::Floating(9), None));
        // Unknown ids, doubles and unknown places in the stored order are dropped.
        let odd = Prefs::from_json(r#"{"layout_version":1,"tab_order":{"RightTop":["library","gone","library","cuts"],"Nowhere":["cuts"]}}"#);
        assert_eq!(odd.tab_order.len(), 1);
        assert_eq!(odd.tab_order["RightTop"], ["library", "cuts"]);
        assert_eq!(ids(&odd, top)[..3], ["library", "cuts", "console"]);
    }

    #[test]
    fn floating_groups_come_and_go_with_their_tabs() {
        let mut p = Prefs::default();
        let g = p.float_panel("laser", [300.0, 200.0], [10.0, 500.0]).unwrap();
        let float = Place::Floating(g);
        assert_eq!(p.place("laser"), float);
        assert_eq!(p.panel("laser").area, Area::RightTop, "its docked area is kept to return to");
        assert_eq!(p.floating[&g].size, [FLOAT_MIN[0], 500.0], "held to the smallest size");
        assert_eq!(p.front(float).as_deref(), Some("laser"));
        // Another panel joins it in front of the first; a second group gets its own id.
        assert!(p.move_panel("library", float, Some(0)));
        assert_eq!(p.visible_in(float).iter().map(|d| d.id).collect::<Vec<_>>(), ["library", "laser"]);
        let h = p.float_panel("cuts", [0.0, 0.0], [400.0, 400.0]).unwrap();
        assert_ne!(g, h);
        // Saved and read back.
        let back = Prefs::from_json(&p.to_json());
        assert_eq!(back, p);
        // The last tab leaving takes the group with it.
        assert!(p.move_panel("cuts", Place::Docked(Area::Left), None));
        assert!(!p.floating.contains_key(&h));
        // Closing a group hides its panels and sends them home.
        p.close_floating(g);
        assert!(p.floating.is_empty());
        assert_eq!(p.place("laser"), Place::Docked(Area::RightTop));
        assert!(!p.panel("laser").visible && !p.panel("library").visible);
        // Hiding the last shown panel of a group (as the Window menu does) removes it too.
        let g = p.float_panel("console", [0.0, 0.0], [400.0, 400.0]).unwrap();
        p.panels.get_mut("console").unwrap().visible = false;
        p.prune_floating();
        assert!(!p.floating.contains_key(&g));
        // Stored nonsense: a panel in a group that does not exist docks; a group with nothing in it goes.
        let odd = Prefs::from_json(
            r#"{"layout_version":1,"panels":{"laser":{"area":"Left","visible":true,"floating":5}},
                "floating":{"2":{"pos":[1e30,5],"size":[-1,99999]}}}"#,
        );
        assert_eq!(odd.place("laser"), Place::Docked(Area::Left));
        assert!(odd.floating.is_empty());
        let kept = Prefs::from_json(
            r#"{"layout_version":1,"panels":{"laser":{"area":"Left","visible":true,"floating":2}},"floating":{"2":{"pos":[1e30,5],"size":[-1,99999]}}}"#,
        );
        assert_eq!(kept.place("laser"), Place::Floating(2));
        assert_eq!(kept.floating[&2].pos, [FLOAT_MAX[0], 5.0]);
        assert_eq!(kept.floating[&2].size, [FLOAT_MIN[0], FLOAT_MAX[1]]);
        // A layout reset puts everything back in the docks.
        let mut r = back;
        r.reset_layout();
        assert!(r.floating.is_empty() && r.tab_order.is_empty());
        assert_eq!(r.place("laser"), Place::Docked(Area::RightTop));
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
