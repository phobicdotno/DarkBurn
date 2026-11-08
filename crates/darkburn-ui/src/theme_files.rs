//! Colour and shape themes as files (#2255): a `solidcraft-theme/1` JSON file names a base palette (the
//! built-in Dark or Light) and sets any of its [`Tokens`] by name, so a theme can be a handful of
//! lines. Built-in themes (Gruvbox and others) are such files under `assets/styles/ui/themes/`;
//! imported ones are kept in the preferences (`Prefs::custom_themes`), so they also work in the
//! browser. Colour vision modes still apply on top of a theme (`Tokens::with_vision`).
//!
//! ```json
//! {"format": "solidcraft-theme/1", "name": "Mine", "base": "dark",
//!  "colors": {"panel": "#282828", "axis_x": "#fb4934", "axis_y": "#b8bb26", "axis_z": "#83a598"}}
//! ```

use std::collections::BTreeMap;
use std::sync::OnceLock;

use egui::Color32;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use solidcraft_theme::{IconColors, Tokens};

/// The `format` tag of a theme file.
pub const FORMAT: &str = "solidcraft-theme/1";
/// Hostile-input caps: file size, colours per file, name length, imported themes kept.
pub const MAX_BYTES: usize = 256 * 1024;
pub const MAX_COLORS: usize = 512;
const MAX_NAME: usize = 80;
pub const MAX_CUSTOM: usize = 64;

// Older exports contain colours for retired window bands and procedural icons.
// Discard them at import; they have no token, renderer, or exported setting now.
const RETIRED_COLORS: &[&str] = &[
    "icon_fill",
    "heading_bg",
    "subheading_bg",
    "app_bar_icon",
    "icons.sheet",
    "icons.sheet_2",
    "icons.sheet_light",
    "icons.sheet_line",
    "icons.glint",
    "icons.folder",
    "icons.plane",
    "icons.fold",
    "icons.stripe",
    "icons.sky",
    "icons.floor",
    "icons.paint",
    "icons.axes",
    "icons.access",
    "icons.draft",
    "icons.curvature",
    "icons.interference",
    "icons.warning",
];

/// The palette a theme starts from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Base {
    #[default]
    Dark,
    Light,
}

impl Base {
    pub fn is_dark(self) -> bool {
        self == Base::Dark
    }
}

/// A theme: identity, base value tables and optional declarations selecting literal overlays.
/// Colour overrides are `#rrggbb` / `#rrggbbaa` strings or lists, keyed by [`keys`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ThemeFile {
    pub id: String,
    pub name: String,
    pub base: Base,
    pub colors: BTreeMap<String, Value>,
    /// Optional corner radii in logical UI points. Missing entries keep the base values.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub radii: BTreeMap<String, u8>,
    /// Optional border roles; absent entries retain the existing surface-specific styling.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub borders: BTreeMap<String, Value>,
    /// SVG display roles and three-colour semantic ramps; omitted values keep compiled defaults.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub icon_palette: BTreeMap<String, Value>,
    /// Optional surface overrides for known ribbon tabs. Accents retain their existing tokens.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tabs: BTreeMap<String, BTreeMap<String, Value>>,
    /// Theme-owned declarations, in file order; standard ids are normalized on import.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<OptionDef>,
    /// Conditional value tables, applied in file order with later entries winning by key.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overlays: Vec<Overlay>,
    /// Levels newer than [`REQUIREMENTS_LEVEL`] disable options and overlays.
    #[serde(default = "default_requirements_level", skip_serializing_if = "is_default_requirements_level")]
    pub requires: u64,
}

/// Theme option/overlay format level understood by this app.
pub const REQUIREMENTS_LEVEL: u64 = 2;
pub const MAX_OPTIONS: usize = 16;
pub const MAX_OVERLAYS: usize = 32;
const STANDARD_OPTIONS: [&str; 4] = ["opacity", "blur", "tab_colours", "tab_fade_ms"];

fn default_requirements_level() -> u64 {
    1
}

fn is_default_requirements_level(level: &u64) -> bool {
    *level == 1
}

impl Default for ThemeFile {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            base: Base::default(),
            colors: BTreeMap::new(),
            radii: BTreeMap::new(),
            borders: BTreeMap::new(),
            icon_palette: BTreeMap::new(),
            tabs: BTreeMap::new(),
            options: Vec::new(),
            overlays: Vec::new(),
            requires: default_requirements_level(),
        }
    }
}

/// Saved option values use the same JSON primitives as defaults and comparisons.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OptionValue {
    Bool(bool),
    Choice(String),
    Num(f32),
}

/// Localized text belongs to the theme, with English as the fallback.
pub type OptionText = BTreeMap<String, String>;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChoiceValue {
    pub id: String,
    pub label: OptionText,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum OptionKind {
    Bool {
        default: bool,
    },
    Choice {
        default: String,
        values: Vec<ChoiceValue>,
    },
    Range {
        default: f32,
        min: f32,
        max: f32,
        step: f32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unit: Option<String>,
    },
}

/// Explicit theme-local continuous effects; custom option ids and ordering remain authored.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OptionEffect {
    SurfaceBrightness,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OptionDef {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect: Option<OptionEffect>,
    #[serde(flatten)]
    pub kind: OptionKind,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub label: OptionText,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub help: OptionText,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub advanced: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Condition {
    Eq {
        opt: String,
        eq: OptionValue,
    },
    Range {
        opt: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        gte: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lte: Option<f32>,
    },
}

impl Condition {
    fn option(&self) -> &str {
        match self {
            Self::Eq { opt, .. } | Self::Range { opt, .. } => opt,
        }
    }
}

/// Overlay tables have exactly the same keys and bounds as the theme's base tables.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ThemeLayer {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub colors: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub radii: BTreeMap<String, u8>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub borders: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub icon_palette: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Overlay {
    pub when: Condition,
    #[serde(flatten)]
    pub values: ThemeLayer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OptionNoticeKind {
    UnknownOption,
    WrongType,
    UnknownChoice,
    NonFinite,
    Clamped,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OptionNotice {
    pub id: String,
    pub kind: OptionNoticeKind,
}

/// Maximum imported radius, in logical UI points; widgets clamp to their own bounds.
const MAX_RADIUS: u8 = 32;
const RADIUS_KEYS: [&str; 6] = ["radius_sm", "radius_md", "radius_lg", "radius_xl", "control_radius", "sketch_mode_radius"];

fn radius_slot<'a>(t: &'a mut Tokens, key: &str) -> Option<&'a mut u8> {
    match key {
        "radius_sm" => Some(&mut t.radius_sm),
        "radius_md" => Some(&mut t.radius_md),
        "radius_lg" => Some(&mut t.radius_lg),
        "radius_xl" => Some(&mut t.radius_xl),
        "control_radius" => Some(&mut t.control_radius),
        "sketch_mode_radius" => Some(&mut t.sketch_mode_radius),
        _ => None,
    }
}

impl ThemeFile {
    /// Shared by import and preference restoration; invalid saved maps are dropped as a unit.
    pub fn set_tabs(&mut self, source: &Value) -> Result<(), String> {
        self.tabs = parse_tabs(source.get("tabs"))?;
        Ok(())
    }

    /// A newer format level keeps the authored base tables but disables options and overlays.
    /// Computed from `requires` so it also works after restoring saved preferences.
    pub fn needs_newer(&self) -> bool {
        self.requires > REQUIREMENTS_LEVEL
    }

    /// Read option tables from a stored or directly supplied theme with the import parser.
    /// Base tables are deliberately left alone; only completely valid declarations replace these.
    pub fn set_option_tables(&mut self, source: &Value) -> Result<(), String> {
        let options = if self.needs_newer() { Vec::new() } else { parse_options(source.get("options"))? };
        if self.requires < 2 && options.iter().any(|option| option.effect.is_some()) {
            return Err("option effects require `requires` of at least 2".into());
        }
        let overlays = if self.needs_newer() { Vec::new() } else { parse_overlays(source.get("overlays"), &options)? };
        self.options = options;
        self.overlays = overlays;
        Ok(())
    }

    /// Validate direct control-channel preference updates as strictly as file imports.
    pub fn validate_radii(&self) -> Result<(), String> {
        for (key, value) in &self.radii {
            if !RADIUS_KEYS.contains(&key.as_str()) {
                return Err(format!("`radii.{key}` is not a theme radius"));
            }
            if *value > MAX_RADIUS {
                return Err(format!("`radii.{key}` must be an integer from 0 to {MAX_RADIUS}"));
            }
        }
        Ok(())
    }
}

/// Strictly bounded strokes shared by file imports and direct preference updates.
pub fn parse_borders(values: &BTreeMap<String, Value>) -> Result<solidcraft_theme::Borders, String> {
    use solidcraft_theme::{Bevel, Borders, InsetBorder};
    if values.len() > 7 {
        return Err("`borders` has more than seven roles".into());
    }
    let mut borders = Borders::default();
    for (key, value) in values {
        let obj = value.as_object().ok_or_else(|| format!("`borders.{key}` must be an object"))?;
        if key == "bevel" {
            if obj.keys().any(|k| !matches!(k.as_str(), "width" | "highlight" | "light" | "shadow" | "dark_shadow")) {
                return Err("`borders.bevel` has an unknown setting".into());
            }
            let width = obj
                .get("width")
                .and_then(Value::as_f64)
                .filter(|v| v.is_finite() && *v > 0.0 && *v <= 2.0)
                .ok_or("`borders.bevel.width` must be a finite number greater than 0 and at most 2")? as f32;
            let color = |name: &str| {
                obj.get(name)
                    .and_then(Value::as_str)
                    .and_then(parse_color)
                    .ok_or_else(|| format!("`borders.bevel.{name}` must be #rrggbb or #rrggbbaa"))
            };
            borders.bevel = Some(Bevel {
                width,
                highlight: color("highlight")?,
                light: color("light")?,
                shadow: color("shadow")?,
                dark_shadow: color("dark_shadow")?,
            });
            continue;
        }
        if obj.keys().any(|k| k != "color" && k != "width" && !(key == "primary_inset" && k == "gap") && !(key == "secondary" && k == "pattern")) {
            return Err(format!("`borders.{key}` has an unknown setting"));
        }
        let number = |name: &str, max: f64| -> Result<f32, String> {
            obj.get(name)
                .and_then(Value::as_f64)
                .filter(|v| v.is_finite() && *v >= 0.0 && *v <= max)
                .map(|v| v as f32)
                .ok_or_else(|| format!("`borders.{key}.{name}` must be a finite number from 0 to {max}"))
        };
        let width = number("width", 4.0)?;
        let color = obj
            .get("color")
            .and_then(Value::as_str)
            .and_then(parse_color)
            .ok_or_else(|| format!("`borders.{key}.color` must be #rrggbb or #rrggbbaa"))?;
        let stroke = egui::Stroke::new(width, color);
        match key.as_str() {
            "primary" => borders.primary = Some(stroke),
            "secondary" => {
                borders.secondary = Some(stroke);
                borders.secondary_dotted = match obj.get("pattern") {
                    None => false,
                    Some(Value::String(pattern)) if pattern == "solid" => false,
                    Some(Value::String(pattern)) if pattern == "dotted" => true,
                    _ => return Err("`borders.secondary.pattern` must be solid or dotted".into()),
                };
            }
            "tertiary" => borders.tertiary = Some(stroke),
            "container" => borders.container = Some(stroke),
            "control" => borders.control = Some(stroke),
            "primary_inset" => borders.primary_inset = Some(InsetBorder { stroke, gap: number("gap", 12.0)? }),
            _ => return Err(format!("`borders.{key}` is not a border role")),
        }
    }
    Ok(borders)
}

/// Import only known, opaque SVG roles and bounded semantic ramps. GAP remains an eraser.
pub fn parse_icon_palette(values: &BTreeMap<String, Value>) -> Result<Option<crate::icon_svg::Palette>, String> {
    use crate::icon_svg::{ACCENT_NAMES, Palette, ROLE_NAMES};
    if values.is_empty() {
        return Ok(None);
    }
    if values.len() > ROLE_NAMES.len() + ACCENT_NAMES.len() + 1 {
        return Err("too many icon palette entries".into());
    }
    let mut palette = Palette::default();
    for (name, value) in values {
        let ramp = if name == "solid" { Some(0) } else { ACCENT_NAMES.iter().position(|a| *a == name).map(|a| a + 1) };
        let role = ROLE_NAMES.iter().position(|r| *r == name && *r != "gap");
        if ramp.is_none() && role.is_none() {
            return Err(format!("`icon_palette.{name}` is not an SVG role or semantic ramp"));
        }
        let colors = colors_of(value, if ramp.is_some() { 3 } else { 1 }).map_err(|e| format!("`icon_palette.{name}` {e}"))?;
        if colors.iter().any(|c| c.a() != 255) {
            return Err(format!("`icon_palette.{name}` must use opaque colours"));
        }
        let rgb = |c: &Color32| [c.r(), c.g(), c.b()];
        if let Some(slot) = ramp.and_then(|r| palette.accents.get_mut(r)) {
            if let [a, b, c] = colors.as_slice() {
                *slot = Some([rgb(a), rgb(b), rgb(c)]);
            }
        } else if let Some((slot, color)) = role.and_then(|r| palette.roles.get_mut(r)).zip(colors.first()) {
            *slot = Some(rgb(color));
        }
    }
    Ok(Some(palette))
}

fn export_icon_palette(palette: Option<crate::icon_svg::Palette>) -> BTreeMap<String, Value> {
    let mut out = BTreeMap::new();
    let Some(palette) = palette else { return out };
    let hex = |[r, g, b]: [u8; 3]| format!("#{r:02x}{g:02x}{b:02x}");
    for (name, color) in crate::icon_svg::ROLE_NAMES.iter().zip(palette.roles) {
        if let Some(rgb) = color {
            out.insert((*name).into(), json!(hex(rgb)));
        }
    }
    for (name, ramp) in std::iter::once("solid").chain(crate::icon_svg::ACCENT_NAMES.iter().copied()).zip(palette.accents) {
        if let Some(colors) = ramp {
            out.insert(name.into(), json!(colors.map(hex)));
        }
    }
    out
}

/// The settable colours: a key, and where it lives in [`Tokens`]. `one` keys take a colour,
/// `many` keys a list of exactly as many colours as the token has; `icons.*` are the colours
/// viewport glyphs draw of their own. The destructuring below fails to build when a token is added
/// without a key here, so every colour stays themeable.
macro_rules! theme_keys {
    (one: [$($one:ident),* $(,)?], many: [$($many:ident),* $(,)?],
     icons: [$($ione:ident),* $(,)?] $(,)?) => {
        /// Every key a theme file may set, in the order an export writes them.
        pub fn keys() -> Vec<&'static str> {
            let mut k = vec![$(stringify!($one),)* $(stringify!($many),)*];
            k.extend([$(concat!("icons.", stringify!($ione)),)*]);
            k
        }

        /// Stable identity of resolved colours, including custom themes with the same base/id.
        /// Share the token inventory with import/export so added colour tokens cannot be missed.
        pub fn color_key(t: &Tokens) -> u64 {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            $(t.$one.hash(&mut h);)*
            $(t.$many.hash(&mut h);)*
            $(t.icons.$ione.hash(&mut h);)*
            h.finish()
        }

        fn slot<'a>(t: &'a mut Tokens, key: &str) -> Option<&'a mut [Color32]> {
            match key {
                $(stringify!($one) => Some(std::slice::from_mut(&mut t.$one)),)*
                $(stringify!($many) => Some(&mut t.$many[..]),)*
                $(concat!("icons.", stringify!($ione)) => Some(std::slice::from_mut(&mut t.icons.$ione)),)*
                _ => None,
            }
        }

        #[allow(dead_code)]
        fn every_colour_has_a_key(t: &Tokens) {
            // Not colours: the hover lift is a grey step, the analysis colours belong to shaders.
            let Tokens {
                $($one: _,)* $($many: _,)*
                hover_face_lift: _, analysis: _, icons: _, borders: _,
                file_action_height: _, file_back_height: _, file_recovery_icon: _, file_grid_gap: _,
                file_nav_width: _, file_nav_height: _, file_tile_height: _, file_tile_icon: _, file_recent_height: _,
                file_sample_image_height: _, file_content_inset_x: _, file_content_inset_y: _, file_section_gap: _, font_start_title: _,
                space_2: _, space_3: _, space_4: _, space_6: _, space_8: _, space_10: _,
                space_12: _, space_14: _, space_16: _, space_24: _, radius_sm: _, radius_md: _,
                radius_lg: _, radius_xl: _, font_badge: _, font_micro: _, font_panel: _, font_small: _,
                font_caption: _, font_body: _, font_label: _, font_button: _, font_heading: _, font_triad: _,
                font_compass: _, font_subtitle: _, font_intro: _, font_page_heading: _, font_article_heading: _, font_title: _, font_display: _,
                control_height: _,  control_radius: _, form_label_width: _, form_gap_y: _, form_footer_height: _, form_header_height: _,
                form_card_width: _, form_icon_size: _, form_close_size: _, form_inset: _, form_icon_inset: _, form_section_icon_size: _,
                form_checkbox_height: _, form_button_height: _, sketch_palette_width: _, sketch_mode_size: _, sketch_mode_radius: _, sketch_mode_border: _,
                sketch_palette_alpha: _, sketch_mode_tint: _, sketch_footer_icon_size: _, font_section_tracking: _,
                figure_ground: _, figure_line: _, figure_highlight: _, figure_shadow: _, figure_face_top: _, figure_face_left: _, figure_face_right: _, figure_card: _, figure_white: _, figure_black: _, figure_accent: _, figure_reference: _, figure_axis_x: _, figure_axis_y: _, figure_axis_z: _, figure_ok: _, figure_warn: _, figure_conflict: _, pref_well: _, pref_note: _, pref_note_fill: _, pref_on_accent: _, pref_ok: _, pref_ok_fill: _, pref_disabled: _,
            } = t;
            let IconColors { $($ione: _,)* } = &t.icons;
        }
    };
}

theme_keys! {
    one: [
        nav_hover, nav_press, nav_dim, nav_viewport, nav_tip, nav_tip_ink, nav_tip_key_border, nav_field,
        file_dim, file_recovery_well, file_background, file_surface, file_border, file_image_tint, file_rail, file_rail_ink, file_rail_hover,
        file_primary, file_primary_ink, file_primary_border, file_recovery, file_recovery_ink, file_recovery_border,
        app_bar, toolbar, panel, panel_header, border, text, text_dim, accent, accent_soft, sketch_accent, error, warning,
        icon, timeline, hover, sel_face, sel_edge, sel_edge_rim, hover_edge, hover_edge_halo, sel_profile,
        sel_profile_edge, hover_profile, hover_profile_edge, sel_vertex, origin_plane, origin_plane_edge, origin_plane_hover,
        origin_point, axis_x, axis_y, axis_z, construction_plane, construction_geom, body, body_edge, manipulator, joint_manipulator, joint_axis,
        preview_add, preview_new, preview_sketch, preview_cut, cube_face, cube_border, cube_text, cube_hover, viewport_top,
        viewport_bottom, grid, grid_major, body_selected, sketch, sketch_fixed, sketch_halo, sketch_locked, sketch_conflict, dimension, dimension_driven,
        sketch_point, sketch_point_free, dialog_bg, dialog_border, chip, section_cap,
        profile_fill, glyph_bg, glyph_edge, glyph_text, overlay, rubber_band, app_bar_text, app_bar_hover, timeline_item,
        timeline_item_error, timeline_marker, field, box_window, box_crossing, sketch_projected, sketch_lost,
        sketch_construction, sketch_centerline, bend_up, bend_down, interference, comb_envelope, comb_teeth,
        sketch_ink, sketch_soft, surface_fill, surface_ink, surface_soft, mesh_fill, mesh_ink, mesh_soft, form_fill, form_tab_ink, form_soft, sheet_fill, sheet_ink, sheet_soft, render_fill, render_ink, render_soft, tab_on_fill_dark,
        app_bar_separator, surface_accent, mesh_accent, form_accent, render_accent, on_accent, agent_ok, agent_busy, app_bar_hover_subtle,
        app_bar_hover_button, app_bar_hover_strong, app_bar_tab_active,
        form_ink, form_dim, form_primary, form_primary_fill, form_error, form_error_fill, form_success, form_success_fill, dialog_body, dialog_footer, dialog_rule, control_fill, control_border, dialog_label, selected_ink, selected_bg,
    ],
    many: [continuity, logo, section_hatch],
    icons: [highlight, agent, marker_halo],
}

/// Keys of theme files from before every axis took one set of colours (#976): the view triad's
/// and the Move gizmo's X, Y and Z. Each still reads, as a list of three, and sets `axis_x`,
/// `axis_y` and `axis_z`, `triad` over `gizmo` and a file's own `axis_*` keys over both. An
/// export never writes them.
const LEGACY_AXES: [&str; 2] = ["gizmo", "triad"];

/// `#rrggbb` or `#rrggbbaa` (alpha not premultiplied).
fn parse_color(s: &str) -> Option<Color32> {
    let hex = s.trim().strip_prefix('#')?;
    if !(hex.len() == 6 || hex.len() == 8) || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |i: usize| hex.get(i..i + 2).and_then(|h| u8::from_str_radix(h, 16).ok());
    let a = if hex.len() == 8 { byte(6)? } else { 255 };
    Some(Color32::from_rgba_unmultiplied(byte(0)?, byte(2)?, byte(4)?, a))
}

fn format_color(c: Color32) -> String {
    let [r, g, b, a] = c.to_srgba_unmultiplied();
    if a == 255 { format!("#{r:02x}{g:02x}{b:02x}") } else { format!("#{r:02x}{g:02x}{b:02x}{a:02x}") }
}

/// The colours a value gives for a slot of `len` colours, or why not.
fn colors_of(v: &Value, len: usize) -> Result<Vec<Color32>, String> {
    let list: Vec<&Value> = match v {
        Value::String(_) if len == 1 => vec![v],
        Value::Array(a) if a.len() == len => a.iter().collect(),
        _ if len == 1 => return Err("must be a colour such as \"#282828\"".into()),
        _ => return Err(format!("must be a list of {len} colours")),
    };
    list.into_iter().map(|c| c.as_str().and_then(parse_color).ok_or_else(|| format!("`{c}` is not a colour such as \"#282828\""))).collect()
}

// The application mark now has three faces and no outline. Older theme exports and
// saved preferences carry a fourth, retired outline colour; keep their face colours.
fn colors_for_key(key: &str, value: &Value, len: usize) -> Result<Vec<Color32>, String> {
    if key == "logo"
        && len == 3
        && let Value::Array(colors) = value
        && colors.len() == 4
    {
        return colors_of(&Value::Array(colors.iter().take(len).cloned().collect()), len);
    }
    colors_of(value, len)
}

/// A theme file's text, checked: the format tag, a name, a base and only known keys with valid
/// colours, bounded shapes, option definitions and overlay comparisons. Nothing is taken from a
/// supported-level file with any error; messages name the offending key and overlay position.
pub fn parse(text: &str) -> Result<ThemeFile, String> {
    if text.len() > MAX_BYTES {
        return Err(format!("a theme file is at most {} KiB", MAX_BYTES / 1024));
    }
    let v: Value = serde_json::from_str(text).map_err(|e| format!("not a theme file: {e}"))?;
    from_value(&v)
}

/// [`parse`] for JSON already read (`ui.prefs` `theme_import`).
pub fn from_value(v: &Value) -> Result<ThemeFile, String> {
    if v.get("format").and_then(Value::as_str) != Some(FORMAT) {
        return Err(format!("not a theme file: `format` must be \"{FORMAT}\""));
    }
    let name = v.get("name").and_then(Value::as_str).map(str::trim).unwrap_or_default();
    if name.is_empty() || name.chars().count() > MAX_NAME {
        return Err(format!("`name` must be 1 to {MAX_NAME} characters"));
    }
    let base = match v.get("base").and_then(Value::as_str) {
        Some("dark") => Base::Dark,
        Some("light") => Base::Light,
        _ => return Err("`base` must be \"dark\" or \"light\"".into()),
    };
    let requires = match v.get("requires") {
        None => default_requirements_level(),
        Some(value) => value.as_u64().ok_or("`requires` must be a nonnegative integer")?,
    };
    let layer = parse_layer(v)?;
    let tabs = parse_tabs(v.get("tabs"))?;
    let options = if requires > REQUIREMENTS_LEVEL { Vec::new() } else { parse_options(v.get("options"))? };
    if requires < 2 && options.iter().any(|option| option.effect.is_some()) {
        return Err("option effects require `requires` of at least 2".into());
    }
    let overlays = if requires > REQUIREMENTS_LEVEL { Vec::new() } else { parse_overlays(v.get("overlays"), &options)? };
    Ok(ThemeFile {
        id: slug(name),
        name: name.to_string(),
        base,
        colors: layer.colors,
        radii: layer.radii,
        borders: layer.borders,
        icon_palette: layer.icon_palette,
        tabs,
        options,
        overlays,
        requires,
    })
}

/// Keep the accepted vocabulary at the same boundary as the existing accent mapping.
fn parse_tabs(v: Option<&Value>) -> Result<BTreeMap<String, BTreeMap<String, Value>>, String> {
    let Some(v) = v else {
        return Ok(BTreeMap::new());
    };
    let tabs = v.as_object().ok_or("`tabs` must be an object")?;
    if tabs.len() > solidcraft_theme::TAB_NAMES.len() {
        return Err("`tabs` has too many entries".into());
    }
    let mut parsed = BTreeMap::new();
    for (tab, surfaces) in tabs {
        if !solidcraft_theme::TAB_NAMES.contains(&tab.as_str()) {
            return Err(format!("`tabs.{tab}` is not a ribbon tab"));
        }
        let surfaces = surfaces.as_object().ok_or_else(|| format!("`tabs.{tab}` must be an object"))?;
        let mut values = BTreeMap::new();
        for (key, value) in surfaces {
            if !matches!(key.as_str(), "panel" | "viewport_top" | "viewport_bottom") {
                return Err(format!("`tabs.{tab}.{key}` is not a tab surface"));
            }
            if value.as_str().and_then(parse_color).is_none() {
                return Err(format!("`tabs.{tab}.{key}` must be #rrggbb or #rrggbbaa"));
            }
            values.insert(key.clone(), value.clone());
        }
        parsed.insert(tab.clone(), values);
    }
    Ok(parsed)
}

fn parse_layer(v: &Value) -> Result<ThemeLayer, String> {
    let mut src = match v.get("colors") {
        None => serde_json::Map::new(),
        Some(Value::Object(o)) => o.clone(),
        Some(_) => return Err("`colors` must be an object of colour names and values".into()),
    };
    if src.len() > MAX_COLORS {
        return Err(format!("`colors` has more than {MAX_COLORS} entries"));
    }
    src.retain(|key, _| !RETIRED_COLORS.contains(&key.as_str()));
    let mut probe = Tokens::light();
    let mut bad = Vec::new();
    for (k, c) in &src {
        if LEGACY_AXES.contains(&k.as_str()) {
            if let Err(e) = colors_of(c, 3) {
                bad.push(format!("`{k}` {e}"));
            }
            continue;
        }
        match slot(&mut probe, k) {
            None => bad.push(format!("`{k}` is not a theme colour")),
            Some(s) => {
                if let Err(e) = colors_for_key(k, c, s.len()) {
                    bad.push(format!("`{k}` {e}"));
                }
            }
        }
    }
    let radii = match v.get("radii") {
        None => BTreeMap::new(),
        Some(Value::Object(values)) => {
            if values.len() > RADIUS_KEYS.len() {
                return Err(format!("`radii` has more than {} entries", RADIUS_KEYS.len()));
            }
            let mut radii = BTreeMap::new();
            for (key, value) in values {
                if !RADIUS_KEYS.contains(&key.as_str()) {
                    bad.push(format!("`radii.{key}` is not a theme radius"));
                } else if let Some(n) = value.as_u64().filter(|n| *n <= u64::from(MAX_RADIUS)) {
                    radii.insert(key.clone(), n as u8);
                } else {
                    bad.push(format!("`radii.{key}` must be an integer from 0 to {MAX_RADIUS}"));
                }
            }
            radii
        }
        Some(_) => return Err("`radii` must be an object of radius names and values".into()),
    };
    if !bad.is_empty() {
        bad.truncate(8);
        return Err(bad.join("; "));
    }
    let borders = match v.get("borders") {
        None => BTreeMap::new(),
        Some(Value::Object(values)) => values.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        Some(_) => return Err("`borders` must be an object of border roles".into()),
    };
    parse_borders(&borders)?;
    let icon_palette = match v.get("icon_palette") {
        None => BTreeMap::new(),
        Some(Value::Object(values)) => values.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        Some(_) => return Err("`icon_palette` must be an object of SVG roles and ramps".into()),
    };
    parse_icon_palette(&icon_palette)?;
    Ok(ThemeLayer { colors: src.into_iter().collect(), radii, borders, icon_palette })
}

fn valid_option_id(id: &str) -> bool {
    (1..=32).contains(&id.len())
        && id.bytes().next().is_some_and(|b| b.is_ascii_lowercase())
        && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

fn option_number(v: &Value, key: &str) -> Result<f32, String> {
    v.get(key)
        .and_then(Value::as_f64)
        .map(|n| n as f32)
        .filter(|n| n.is_finite())
        .ok_or_else(|| format!("`{key}` must be a finite number representable as f32"))
}

fn option_text(v: Option<&Value>, key: &str, max: usize) -> Result<OptionText, String> {
    let Some(v) = v else {
        return Ok(BTreeMap::new());
    };
    let obj = v.as_object().ok_or_else(|| format!("`{key}` must be a map of language codes to strings"))?;
    if obj.len() > 64 {
        return Err(format!("`{key}` has more than 64 languages"));
    }
    obj.iter()
        .map(|(language, value)| {
            if language.is_empty() || language.len() > 32 || !language.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
                return Err(format!("`{key}` has an invalid language code"));
            }
            let text = value
                .as_str()
                .filter(|s| s.chars().count() <= max)
                .ok_or_else(|| format!("`{key}.{language}` must be a string of at most {max} characters"))?;
            Ok((language.clone(), text.into()))
        })
        .collect()
}

fn parse_option(v: &Value) -> Result<OptionDef, String> {
    let id = v.get("id").and_then(Value::as_str).filter(|id| valid_option_id(id)).ok_or("`id` must match [a-z][a-z0-9_]{0,31}")?;
    let kind = v.get("type").and_then(Value::as_str).ok_or("`type` must be bool, choice or range")?;
    let reserved = STANDARD_OPTIONS.contains(&id);
    let effect = match v.get("effect") {
        None => None,
        Some(Value::String(effect)) if effect == "surface_brightness" && !reserved => Some(OptionEffect::SurfaceBrightness),
        Some(_) => {
            return Err("`effect` must be surface_brightness on a custom range option".into());
        }
    };
    let kind = if id == "tab_colours" || (!reserved && kind == "bool") {
        if kind != "bool" {
            return Err(format!("`{id}.type` must be bool"));
        }
        OptionKind::Bool { default: v.get("default").and_then(Value::as_bool).ok_or("`default` must be bool")? }
    } else if reserved || kind == "range" {
        if kind != "range" {
            return Err(format!("`{id}.type` must be range"));
        }
        let (floor, ceiling, standard_unit) = match id {
            "opacity" => (40.0, 100.0, Some("%")),
            "blur" => (0.0, 60.0, Some("px")),
            "tab_fade_ms" => (0.0, 600.0, Some("ms")),
            _ => (0.0, 0.0, None),
        };
        let min = if reserved && v.get("min").is_none() { floor } else { option_number(v, "min")? };
        let max = if reserved && v.get("max").is_none() { ceiling } else { option_number(v, "max")? };
        if min > max {
            return Err("`min` must not exceed `max`".into());
        }
        if reserved && (min < floor || max > ceiling) {
            return Err(format!("`min` / `max` for {id} must stay inside {floor} to {ceiling}"));
        }
        let step = if id == "tab_fade_ms" {
            20.0
        } else if reserved {
            1.0
        } else {
            option_number(v, "step")?
        };
        if step <= 0.0 {
            return Err("`step` must be greater than zero".into());
        }
        let default = if id == "opacity" && v.get("default").is_none() { 100.0 } else { option_number(v, "default")? };
        let default = if reserved {
            default.clamp(min, max)
        } else {
            if default < min || default > max {
                return Err("`default` must be inside `min` / `max`".into());
            }
            default
        };
        let unit = if reserved {
            standard_unit.map(str::to_string)
        } else {
            v.get("unit")
                .map(|v| v.as_str().filter(|s| s.chars().count() <= 4).map(str::to_string).ok_or("`unit` must be a string of at most 4 characters"))
                .transpose()?
        };
        OptionKind::Range { default, min, max, step, unit }
    } else if kind == "choice" {
        let values = v.get("values").and_then(Value::as_array).filter(|v| (2..=12).contains(&v.len())).ok_or("`values` must have 2 to 12 choices")?;
        let mut ids = std::collections::BTreeSet::new();
        let mut choices = Vec::new();
        for value in values {
            let id = value.get("id").and_then(Value::as_str).filter(|id| valid_option_id(id)).ok_or("`values.id` must match [a-z][a-z0-9_]{0,31}")?;
            if !ids.insert(id) {
                return Err(format!("`values.id` duplicates {id}"));
            }
            let label = option_text(Some(value.get("label").ok_or("`values.label` is required")?), "values.label", 40)?;
            choices.push(ChoiceValue { id: id.into(), label });
        }
        let default = v.get("default").and_then(Value::as_str).filter(|id| ids.contains(id)).ok_or("`default` must name an id in `values`")?;
        OptionKind::Choice { default: default.into(), values: choices }
    } else {
        return Err("`type` must be bool, choice or range".into());
    };
    // Standard definitions are app-owned: even malformed presentation fields are ignored.
    if effect.is_some()
        && !matches!(&kind, OptionKind::Range { default, min, max, step, unit }
        if *default == 100.0 && *min >= 60.0 && *max <= 120.0 && *min <= 100.0 && *max >= 100.0 && *step == 1.0 && unit.as_deref() == Some("%"))
    {
        return Err("surface_brightness requires a percent range inside 60 to 120, default 100 and step 1".into());
    }
    if reserved {
        return Ok(OptionDef {
            id: id.into(),
            effect,
            kind,
            label: BTreeMap::new(),
            help: BTreeMap::new(),
            parent: (id == "tab_fade_ms").then(|| "tab_colours".into()),
            advanced: false,
            section: None,
        });
    }
    let parent = v
        .get("parent")
        .map(|v| v.as_str().filter(|id| valid_option_id(id)).map(str::to_string).ok_or("`parent` must be an option id"))
        .transpose()?;
    let advanced = v.get("advanced").map(|v| v.as_bool().ok_or("`advanced` must be bool")).transpose()?.unwrap_or(false);
    let section = v
        .get("section")
        .map(|v| v.as_str().filter(|s| s.chars().count() <= 24).map(str::to_string).ok_or("`section` must be a string of at most 24 characters"))
        .transpose()?;
    Ok(OptionDef {
        id: id.into(),
        effect,
        kind,
        label: option_text(v.get("label"), "label", 40)?,
        help: option_text(v.get("help"), "help", 160)?,
        parent,
        advanced,
        section,
    })
}

fn parse_options(v: Option<&Value>) -> Result<Vec<OptionDef>, String> {
    let Some(v) = v else { return Ok(Vec::new()) };
    let values = v.as_array().filter(|v| v.len() <= MAX_OPTIONS).ok_or("`options` must be an array of at most 16 entries")?;
    let mut options = Vec::new();
    let mut ids = std::collections::BTreeSet::new();
    for (position, value) in values.iter().enumerate() {
        let option = parse_option(value).map_err(|e| format!("`options[{}]`: {e}", position + 1))?;
        if !ids.insert(option.id.clone()) {
            return Err(format!("`options.id` duplicates {}", option.id));
        }
        if option.effect.is_some() && options.iter().any(|previous: &OptionDef| previous.effect == option.effect) {
            return Err("`options.effect` may declare surface_brightness only once".into());
        }
        options.push(option);
    }
    let mut parents = std::collections::BTreeSet::new();
    for option in &options {
        if let Some(parent) = &option.parent {
            if !options.iter().any(|o| &o.id == parent && matches!(o.kind, OptionKind::Bool { .. })) {
                return Err(format!("`options.{}.parent` must name a bool option", option.id));
            }
            if !parents.insert(parent) {
                return Err(format!("`options.parent` {parent} has more than one child"));
            }
            let mut ancestor = Some(parent.as_str());
            for _ in 0..=MAX_OPTIONS {
                let Some(id) = ancestor else { break };
                if id == option.id {
                    return Err(format!("`options.{}.parent` forms a cycle", option.id));
                }
                ancestor = options.iter().find(|o| o.id == id).and_then(|o| o.parent.as_deref());
            }
        }
    }
    Ok(options)
}

fn parse_condition(v: &Value, options: &[OptionDef]) -> Result<Condition, String> {
    let obj = v.as_object().ok_or("`when` must be an object")?;
    let opt = v.get("opt").and_then(Value::as_str).ok_or("`when.opt` must be an option id")?;
    let option = options.iter().find(|o| o.id == opt).ok_or_else(|| format!("`when.opt` names unknown option {opt}"))?;
    if let Some(eq) = v.get("eq") {
        if obj.len() != 2 {
            return Err("`when.eq` cannot be combined with other comparisons".into());
        }
        let eq = match &option.kind {
            OptionKind::Bool { .. } => OptionValue::Bool(eq.as_bool().ok_or("`when.eq` must be bool")?),
            OptionKind::Choice { values, .. } => OptionValue::Choice(
                eq.as_str().filter(|id| values.iter().any(|v| v.id == *id)).ok_or("`when.eq` must name a choice in `values`")?.into(),
            ),
            OptionKind::Range { .. } => OptionValue::Num(option_number(v, "eq").map_err(|e| format!("`when`: {e}"))?),
        };
        return Ok(Condition::Eq { opt: opt.into(), eq });
    }
    if !matches!(option.kind, OptionKind::Range { .. }) {
        return Err("`when.gte` / `when.lte` require a range option".into());
    }
    if obj.keys().any(|k| !matches!(k.as_str(), "opt" | "gte" | "lte")) {
        return Err("`when` has an unknown comparison".into());
    }
    let bound = |key| v.get(key).map(|_| option_number(v, key).map_err(|e| format!("`when`: {e}"))).transpose();
    let (gte, lte) = (bound("gte")?, bound("lte")?);
    if gte.is_none() && lte.is_none() {
        return Err("`when` needs eq, gte or lte".into());
    }
    if matches!((gte, lte), (Some(a), Some(b)) if a > b) {
        return Err("`when.gte` must not exceed `when.lte`".into());
    }
    Ok(Condition::Range { opt: opt.into(), gte, lte })
}

fn parse_overlays(v: Option<&Value>, options: &[OptionDef]) -> Result<Vec<Overlay>, String> {
    let Some(v) = v else { return Ok(Vec::new()) };
    let values = v.as_array().filter(|v| v.len() <= MAX_OVERLAYS).ok_or("`overlays` must be an array of at most 32 entries")?;
    values
        .iter()
        .enumerate()
        .map(|(position, value)| {
            let parse = || {
                let when = parse_condition(value.get("when").ok_or("`when` is required")?, options)?;
                Ok(Overlay { when, values: parse_layer(value)? })
            };
            parse().map_err(|e: String| format!("`overlays[{}]`: {e}", position + 1))
        })
        .collect()
}

/// An id from a name: lower-case letters, digits and dashes.
fn slug(name: &str) -> String {
    let mut s = String::new();
    for c in name.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            s.push(c);
        } else if !s.ends_with('-') && !s.is_empty() {
            s.push('-');
        }
    }
    let s = s.trim_end_matches('-').to_string();
    if s.is_empty() { "theme".into() } else { s }
}

/// Definitions from hand-edited preferences get the same bounds and reserved-id normalization
/// as imports. An invalid definition list cannot drive an overlay.
fn usable_options(theme: &ThemeFile) -> Vec<OptionDef> {
    if theme.needs_newer() || theme.options.len() > MAX_OPTIONS {
        return Vec::new();
    }
    let options = serde_json::to_value(&theme.options).ok().and_then(|v| parse_options(Some(&v)).ok()).unwrap_or_default();
    if theme.requires < 2 && options.iter().any(|option| option.effect.is_some()) { Vec::new() } else { options }
}

impl OptionDef {
    pub fn default_value(&self) -> OptionValue {
        match &self.kind {
            OptionKind::Bool { default } => OptionValue::Bool(*default),
            OptionKind::Choice { default, .. } => OptionValue::Choice(default.clone()),
            OptionKind::Range { default, .. } => OptionValue::Num(*default),
        }
    }
}

/// Values for every declared option. Missing values use defaults without a notice; invalid saved
/// values use defaults and report why. Unknown ids are omitted (there is no definition/default).
/// Range values are clamped, without rounding to `step`. Inactive children retain their values.
pub fn effective_values(theme: &ThemeFile, saved: &BTreeMap<String, OptionValue>) -> (BTreeMap<String, OptionValue>, Vec<OptionNotice>) {
    let options = usable_options(theme);
    let mut values: BTreeMap<_, _> = options.iter().map(|o| (o.id.clone(), o.default_value())).collect();
    let mut notices = Vec::new();
    for (id, saved) in saved {
        let Some(option) = options.iter().find(|o| &o.id == id) else {
            notices.push(OptionNotice { id: id.clone(), kind: OptionNoticeKind::UnknownOption });
            continue;
        };
        let mut notice = None;
        let value = match (&option.kind, saved) {
            (OptionKind::Bool { .. }, OptionValue::Bool(_)) => saved.clone(),
            (OptionKind::Choice { values, .. }, OptionValue::Choice(id)) if values.iter().any(|v| &v.id == id) => saved.clone(),
            (OptionKind::Choice { .. }, OptionValue::Choice(_)) => {
                notice = Some(OptionNoticeKind::UnknownChoice);
                option.default_value()
            }
            (OptionKind::Range { min, max, .. }, OptionValue::Num(n)) if n.is_finite() => {
                let clamped = n.clamp(*min, *max);
                if clamped != *n {
                    notice = Some(OptionNoticeKind::Clamped);
                }
                OptionValue::Num(clamped)
            }
            (OptionKind::Range { .. }, OptionValue::Num(_)) => {
                notice = Some(OptionNoticeKind::NonFinite);
                option.default_value()
            }
            _ => {
                notice = Some(OptionNoticeKind::WrongType);
                option.default_value()
            }
        };
        values.insert(id.clone(), value);
        if let Some(kind) = notice {
            notices.push(OptionNotice { id: id.clone(), kind });
        }
    }
    (values, notices)
}

/// Standard options in the app's fixed order, each followed by its declared child, then custom
/// options in file order. Each definition appears once. No undeclared standard option is added.
pub fn ordered_options(theme: &ThemeFile) -> Vec<&OptionDef> {
    if theme.needs_newer() {
        return Vec::new();
    }
    let options = theme.options.get(..theme.options.len().min(MAX_OPTIONS)).unwrap_or_default();
    let mut ordered: Vec<&OptionDef> = Vec::new();
    for id in ["opacity", "blur", "tab_colours"] {
        let mut next = options.iter().find(|o| o.id == id);
        for _ in 0..MAX_OPTIONS {
            let Some(option) = next else { break };
            if ordered.iter().any(|o| o.id == option.id) {
                break;
            }
            ordered.push(option);
            next = options.iter().find(|o| o.parent.as_deref() == Some(option.id.as_str()));
        }
    }
    for option in options {
        if !ordered.iter().any(|o| o.id == option.id) {
            ordered.push(option);
        }
    }
    ordered
}

fn option_active(id: &str, options: &[OptionDef], values: &BTreeMap<String, OptionValue>) -> bool {
    let mut current = id;
    for _ in 0..=MAX_OPTIONS {
        let Some(option) = options.iter().find(|o| o.id == current) else {
            return false;
        };
        let Some(parent) = option.parent.as_deref() else {
            return true;
        };
        if values.get(parent) != Some(&OptionValue::Bool(true)) {
            return false;
        }
        current = parent;
    }
    false
}

fn condition_matches(condition: &Condition, values: &BTreeMap<String, OptionValue>) -> bool {
    match condition {
        Condition::Eq { opt, eq } => values.get(opt) == Some(eq),
        Condition::Range { opt, gte, lte } => matches!(values.get(opt), Some(OptionValue::Num(n))
            if n.is_finite() && gte.is_none_or(|a| *n >= a) && lte.is_none_or(|b| *n <= b)),
    }
}

/// Resolve at declared defaults. Files without options retain the original palette exactly.
pub fn resolve(theme: &ThemeFile) -> Tokens {
    resolve_with(theme, &BTreeMap::new())
}

/// Resolve base tables, then matching overlays in file order. Later tables replace earlier
/// entries by key. Saved values are normalized here too, so callers cannot bypass finite checks.
pub fn resolve_with(theme: &ThemeFile, saved: &BTreeMap<String, OptionValue>) -> Tokens {
    resolve_for_tab(theme, saved, None)
}

/// Resolve a tab and its base/overlays in one pass. Providers run last so opacity also protects
/// tab-authored panels. Restored/direct Rust values are revalidated before they affect tokens.
pub fn resolve_for_tab(theme: &ThemeFile, saved: &BTreeMap<String, OptionValue>, tab: Option<&str>) -> Tokens {
    let Some(Merged { file: merged, options, values }) = merge(theme, saved) else {
        return resolve_base(theme);
    };
    let mut tokens = resolve_base(&merged);
    let mut tab_panel = false;
    if crate::theme_options::tab_settings(theme, &values).0
        && let Some(tab) = tab
        && let Ok(tabs) = serde_json::to_value(&theme.tabs).map_err(|e| e.to_string()).and_then(|v| parse_tabs(Some(&v)))
        && let Some(surfaces) = tabs.get(tab)
    {
        for (key, value) in surfaces {
            if let Some(slot) = slot(&mut tokens, key)
                && let Ok(colors) = colors_for_key(key, value, 1)
            {
                slot.copy_from_slice(&colors);
            }
        }
        tab_panel = surfaces.contains_key("panel");
    }
    for option in &options {
        if option.effect == Some(OptionEffect::SurfaceBrightness)
            && option_active(&option.id, &options, &values)
            && let Some(OptionValue::Num(percent)) = values.get(&option.id)
        {
            tokens.apply_surface_brightness(*percent);
        }
    }
    crate::theme_options::apply(theme, &values, &mut tokens);
    if tab_panel {
        tokens.protect_tab_panel();
    }
    tokens
}

/// A theme's base tables with its matching overlays merged in, and the options and values that chose them.
struct Merged {
    file: ThemeFile,
    options: Vec<OptionDef>,
    values: BTreeMap<String, OptionValue>,
}

/// Merge the overlays that match the saved values into the base tables, in file order; later tables replace earlier
/// entries by key. `None`: the theme has no usable options or overlays, so its base tables are the whole theme.
fn merge(theme: &ThemeFile, saved: &BTreeMap<String, OptionValue>) -> Option<Merged> {
    if theme.needs_newer() || (theme.options.is_empty() && theme.overlays.is_empty()) || theme.overlays.len() > MAX_OVERLAYS {
        return None;
    }
    let options = usable_options(theme);
    let overlays = serde_json::to_value(&theme.overlays).ok().and_then(|v| parse_overlays(Some(&v), &options).ok()).unwrap_or_default();
    let (values, _) = effective_values(theme, saved);
    let mut merged = theme.clone();
    for overlay in overlays {
        if !option_active(overlay.when.option(), &options, &values) || !condition_matches(&overlay.when, &values) {
            continue;
        }
        // Legacy axis lists are local to a layer: an overlay's triad must override axes in
        // the base, while explicit axis keys in that same overlay still win over its triad.
        for key in LEGACY_AXES {
            if let Some(Value::Array(colors)) = overlay.values.colors.get(key) {
                for (axis, color) in ["axis_x", "axis_y", "axis_z"].into_iter().zip(colors) {
                    merged.colors.insert(axis.into(), color.clone());
                }
            }
        }
        merged.colors.extend(overlay.values.colors);
        merged.radii.extend(overlay.values.radii);
        merged.borders.extend(overlay.values.borders);
        merged.icon_palette.extend(overlay.values.icon_palette);
    }
    Some(Merged { file: merged, options, values })
}

/// The icon palette a theme sets for the icon renderer, after its matching overlays (as [`resolve_with`] resolves its
/// tokens). `None`: the theme keeps the built-in artwork colours, as does an icon palette saved preferences hold that
/// this version cannot read.
pub fn resolve_icon_palette(theme: &ThemeFile, saved: &BTreeMap<String, OptionValue>) -> Option<crate::icon_svg::Palette> {
    let merged = merge(theme, saved);
    parse_icon_palette(&merged.as_ref().map_or(theme, |m| &m.file).icon_palette).unwrap_or_default()
}

/// A colour the saved preferences hold that this version cannot read is skipped safely.
fn resolve_base(theme: &ThemeFile) -> Tokens {
    let mut t = if theme.base.is_dark() { Tokens::dark() } else { Tokens::light() };
    for k in LEGACY_AXES {
        if let Some(Ok(c)) = theme.colors.get(k).map(|v| colors_of(v, 3))
            && let [x, y, z] = c[..]
        {
            (t.axis_x, t.axis_y, t.axis_z) = (x, y, z);
        }
    }
    for (k, v) in &theme.colors {
        if let Some(s) = slot(&mut t, k)
            && let Ok(c) = colors_for_key(k, v, s.len())
        {
            s.copy_from_slice(&c);
        }
    }
    if !theme.colors.contains_key("form_primary_fill") {
        t.form_primary_fill = t.form_primary;
    }
    for (key, value) in &theme.radii {
        if *value <= MAX_RADIUS
            && let Some(slot) = radius_slot(&mut t, key)
        {
            *slot = *value;
        }
    }
    // Malformed hand-edited saved settings cannot introduce non-finite drawing geometry.
    t.borders = parse_borders(&theme.borders).unwrap_or_default();
    // Conflicting sketch geometry is drawn in the theme's own error colour unless it sets one of
    // its own (#7244).
    if !theme.colors.contains_key("sketch_conflict") {
        t.sketch_conflict = t.error;
    }
    // A file that sets no colours is its base palette, exactly.
    if !theme.colors.is_empty() {
        file_view_from_theme(&mut t, theme.base.is_dark(), |k| theme.colors.contains_key(k));
    }
    t
}

/// The File view's colours a theme leaves unset follow the theme's own panel, border, accent and
/// text, not its base's fixed blue rail and grey cards (#4868). Each ink is whichever candidate
/// reads best on the fills it sits on.
fn file_view_from_theme(t: &mut Tokens, dark: bool, set: impl Fn(&str) -> bool) {
    use solidcraft_theme::contrast_ratio;
    let fallback = t.text;
    let best = |inks: &[Color32], fills: &[Color32]| {
        let worst = |ink: Color32| fills.iter().map(|f| contrast_ratio(ink, *f)).fold(f64::INFINITY, f64::min);
        inks.iter().copied().max_by(|a, b| worst(*a).total_cmp(&worst(*b))).unwrap_or(fallback)
    };
    if !set("file_background") {
        t.file_background = t.panel;
    }
    if !set("file_surface") {
        t.file_surface = t.dialog_bg;
    }
    if !set("file_border") {
        t.file_border = t.border;
    }
    let inks = [t.on_accent, t.text, t.panel, t.app_bar, t.form_ink];
    let (rail_set, hover_set) = (set("file_rail"), set("file_rail_hover"));
    if !rail_set {
        // The accent deepened as the built-in palettes deepen theirs (Dark's rail is about 0.55 of
        // its accent, Light's about 0.85), toward the theme's own darkest colour.
        let (toward, k) = if dark { (t.app_bar, 0.45) } else { (t.text, 0.15) };
        t.file_rail = t.accent.lerp_to_gamma(toward, k);
    }
    if !hover_set {
        t.file_rail_hover = t.file_rail.lerp_to_gamma(t.accent, 0.25);
    }
    if !set("file_rail_ink") {
        t.file_rail_ink = best(&inks, &[t.file_rail, t.file_rail_hover]);
    }
    // A rail still too mid-toned for any ink goes further from the ink until the ink reads.
    let away = best(&inks, &[t.file_rail_ink]);
    for _ in 0..10 {
        let reads = |fill: Color32| contrast_ratio(t.file_rail_ink, fill) >= 4.5;
        if (rail_set || reads(t.file_rail)) && (hover_set || reads(t.file_rail_hover)) {
            break;
        }
        if !rail_set {
            t.file_rail = t.file_rail.lerp_to_gamma(away, 0.1);
        }
        if !hover_set {
            t.file_rail_hover = t.file_rail_hover.lerp_to_gamma(away, 0.1);
        }
    }
    if !set("file_primary") {
        t.file_primary = t.accent_soft;
    }
    if !set("file_primary_border") {
        t.file_primary_border = t.accent;
    }
    if !set("file_primary_ink") {
        t.file_primary_ink = best(&[t.accent, t.text, t.on_accent], &[t.file_primary, t.file_background]);
    }
    if !set("file_dim") {
        // Dim where dim text still reads on the page and the cards, else the plain text colour.
        let on = [t.file_surface, t.file_background];
        t.file_dim = if on.iter().all(|f| contrast_ratio(t.text_dim, *f) >= 4.5) { t.text_dim } else { best(&[t.text, t.form_ink], &on) };
    }
}

/// A complete theme file of a palette and its icon palette: every key, so it is a starting point for a new theme.
pub fn export(name: &str, base: Base, t: &Tokens, icons: Option<crate::icon_svg::Palette>) -> Value {
    let mut t = *t;
    let mut colors = serde_json::Map::new();
    for k in keys() {
        if let Some(s) = slot(&mut t, k) {
            let v = if let [c] = s { json!(format_color(*c)) } else { json!(s.iter().map(|c| format_color(*c)).collect::<Vec<_>>()) };
            colors.insert(k.to_string(), v);
        }
    }
    let radii: BTreeMap<_, _> = RADIUS_KEYS.into_iter().filter_map(|key| radius_slot(&mut t, key).map(|value| (key, *value))).collect();
    let mut borders = serde_json::Map::new();
    for (key, stroke) in [
        ("primary", t.borders.primary),
        ("secondary", t.borders.secondary),
        ("tertiary", t.borders.tertiary),
        ("container", t.borders.container),
        ("control", t.borders.control),
    ] {
        if let Some(stroke) = stroke {
            borders.insert(key.into(), json!({"width": stroke.width, "color": format_color(stroke.color)}));
        }
    }
    if t.borders.secondary_dotted
        && let Some(secondary) = borders.get_mut("secondary").and_then(Value::as_object_mut)
    {
        secondary.insert("pattern".into(), json!("dotted"));
    }
    if let Some(inset) = t.borders.primary_inset {
        borders.insert("primary_inset".into(), json!({"width": inset.stroke.width, "color": format_color(inset.stroke.color), "gap": inset.gap}));
    }
    if let Some(bevel) = t.borders.bevel {
        let shadow = format_color(bevel.shadow);
        borders.insert(
            "bevel".into(),
            json!({"width": bevel.width, "highlight": format_color(bevel.highlight),
            "light": format_color(bevel.light), "shadow": shadow, "dark_shadow": format_color(bevel.dark_shadow)}),
        );
    }
    let icon_palette = export_icon_palette(icons);
    json!({"format": FORMAT, "name": name, "base": base, "colors": colors, "radii": radii, "borders": borders, "icon_palette": icon_palette})
}

/// Export a theme definition, preserving its options and overlays rather than flattening them
/// to the current tokens. The existing [`export`] remains a complete token snapshot.
pub fn export_file(theme: &ThemeFile) -> Value {
    let mut value = json!({"format": FORMAT, "name": theme.name, "base": theme.base, "colors": theme.colors});
    if let Some(obj) = value.as_object_mut() {
        if !theme.radii.is_empty() {
            obj.insert("radii".into(), json!(theme.radii));
        }
        if !theme.borders.is_empty() {
            obj.insert("borders".into(), json!(theme.borders));
        }
        if !theme.icon_palette.is_empty() {
            obj.insert("icon_palette".into(), json!(theme.icon_palette));
        }
        if !theme.tabs.is_empty() {
            obj.insert("tabs".into(), json!(theme.tabs));
        }
        if !theme.options.is_empty() {
            obj.insert("options".into(), json!(theme.options));
        }
        if !theme.overlays.is_empty() {
            obj.insert("overlays".into(), json!(theme.overlays));
        }
        if !is_default_requirements_level(&theme.requires) {
            obj.insert("requires".into(), json!(theme.requires));
        }
    }
    value
}

// Theme sources are collected at build time; picker order is validated below.
include!(concat!(env!("OUT_DIR"), "/builtin_themes.rs"));

/// The two-colour built-in themes (1-bit and 1-bit Dark): pure black and white, so a few looks
/// other themes tell apart by colour are told apart by text or shape instead.

/// The built-in themes, validated and ordered once; a test checks every shipped file.
pub fn builtin() -> &'static [ThemeFile] {
    static THEMES: OnceLock<Vec<ThemeFile>> = OnceLock::new();
    THEMES.get_or_init(|| match read_builtin(BUILTIN) {
        Ok(themes) => themes,
        Err(e) => {
            log::error!("built-in themes: {e}");
            Vec::new()
        }
    })
}

fn read_builtin(sources: &[(&str, &str)]) -> Result<Vec<ThemeFile>, String> {
    let mut orders = std::collections::BTreeSet::new();
    let mut ids = std::collections::BTreeSet::new();
    let mut themes = Vec::new();
    for &(id, text) in sources {
        if !ids.insert(id) || id.is_empty() {
            return Err(format!("empty or duplicate theme id: {id}"));
        }
        if text.len() > MAX_BYTES {
            return Err(format!("theme {id} exceeds the file size limit"));
        }
        let value: Value = serde_json::from_str(text).map_err(|e| format!("theme {id}: {e}"))?;
        let order = value.get("order").and_then(Value::as_u64).ok_or_else(|| format!("theme {id}: order must be a nonnegative integer"))?;
        if !orders.insert(order) {
            return Err(format!("theme {id}: duplicate order {order}"));
        }
        if value.get("provenance").and_then(Value::as_str).is_none_or(|s| s.trim().is_empty()) {
            return Err(format!("theme {id}: missing provenance"));
        }
        let theme = from_value(&value).map_err(|e| format!("theme {id}: {e}"))?;
        themes.push((order, ThemeFile { id: id.into(), ..theme }));
    }
    if themes.is_empty() {
        return Err("no built-in themes".into());
    }
    themes.sort_by_key(|(order, _)| *order);
    Ok(themes.into_iter().map(|(_, theme)| theme).collect())
}

/// The theme a `theme` preference names: built in, or imported. `None` for dark, light, system
/// or an unknown id.
pub fn find<'a>(id: &str, custom: &'a [ThemeFile]) -> Option<&'a ThemeFile> {
    builtin().iter().find(|t| t.id == id).or_else(|| custom.iter().find(|t| t.id == id))
}

/// Retired built-in cards now select a contrast value on their light/dark family.
/// Keep these ids reserved so importing a theme cannot take a saved built-in identity.
pub const GRUVBOX_ALIASES: [(&str, &str, &str); 4] = [
    ("gruvbox-dark-hard", "gruvbox-dark", "hard"),
    ("gruvbox-dark-soft", "gruvbox-dark", "soft"),
    ("gruvbox-light-hard", "gruvbox-light", "hard"),
    ("gruvbox-light-soft", "gruvbox-light", "soft"),
];

pub fn legacy_gruvbox(id: &str) -> Option<(&'static str, &'static str)> {
    GRUVBOX_ALIASES.iter().find(|(old, _, _)| *old == id).map(|(_, family, contrast)| (*family, *contrast))
}

/// Add an imported theme to `custom`, replacing one with the same id (importing a file again
/// updates it). Its id never takes a built-in one's or the modes' (dark, light, system).
/// Returns the id it is kept under.
pub fn add(custom: &mut Vec<ThemeFile>, mut theme: ThemeFile) -> Result<String, String> {
    if ["dark", "light", "system"].contains(&theme.id.as_str()) || builtin().iter().any(|t| t.id == theme.id) || legacy_gruvbox(&theme.id).is_some() {
        theme.id = format!("custom-{}", theme.id);
    }
    let id = theme.id.clone();
    match custom.iter().position(|t| t.id == id) {
        Some(i) => {
            if let Some(old) = custom.get_mut(i) {
                *old = theme;
            }
        }
        None if custom.len() >= MAX_CUSTOM => {
            return Err(format!("at most {MAX_CUSTOM} imported themes; remove one first"));
        }
        None => custom.push(theme),
    }
    Ok(id)
}

/// Seconds between two looks at a followed file's modified time and size.
pub const FOLLOW_INTERVAL: f64 = 1.0;
/// Longest followed path accepted, in bytes (hostile input from the control channel).
pub const MAX_FOLLOW_PATH: usize = 4096;

/// A theme file the desktop app follows (#7573): another tool writes it, and the app reloads it
/// when its modified time or size changes. Only a read that parses completely is applied; while
/// the file is being written, broken, missing or too big, the last good theme stays on and
/// [`Follow::error`] says why.
#[derive(Clone, Debug, Default)]
pub struct Follow {
    /// The `--theme-file` launch flag's path: followed for this run in place of the preference,
    /// never saved.
    pub launch: Option<String>,
    /// The path the state below belongs to.
    path: String,
    /// The file's modified time and size at the last read.
    stamp: Option<(Option<std::time::SystemTime>, u64)>,
    /// When (`egui` input time, seconds) the file was last looked at.
    checked: Option<f64>,
    /// The last theme that read completely.
    theme: Option<ThemeFile>,
    /// Why the last read was not applied.
    error: Option<String>,
    /// Whether the window had the focus last frame (gaining it re-reads the file).
    pub focused: bool,
}

/// What a [`Follow::poll`] changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FollowChange {
    /// Nothing new.
    None,
    /// The file read completely; its theme is on now.
    Loaded(String),
    /// A read failed for a new reason (reported once; the last good theme stays on).
    Failed(String),
}

/// A followed path as given: trimmed, at most [`MAX_FOLLOW_PATH`] bytes, no control characters.
pub fn follow_path(path: &str) -> Result<String, String> {
    let path = path.trim();
    if path.len() > MAX_FOLLOW_PATH || path.chars().any(char::is_control) {
        return Err(format!("a followed theme file's path is at most {MAX_FOLLOW_PATH} bytes without control characters"));
    }
    Ok(path.to_string())
}

impl Follow {
    /// The path followed now: the launch flag's, else the preference's (empty: none).
    pub fn path<'a>(&'a self, preference: &'a str) -> &'a str {
        match self.launch.as_deref().map(str::trim) {
            Some(path) if !path.is_empty() => path,
            _ => preference.trim(),
        }
    }

    /// The theme the followed file gave last, if any read completely.
    pub fn theme(&self) -> Option<&ThemeFile> {
        self.theme.as_ref()
    }

    /// Why the followed file's last read was not applied.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Look at `path` (`now` in seconds): at most every [`FOLLOW_INTERVAL`] unless `force`, and
    /// read it only when its modified time or size changed. An empty path stops following.
    pub fn poll(&mut self, path: &str, now: f64, force: bool) -> FollowChange {
        if path != self.path {
            *self = Follow { launch: self.launch.take(), focused: self.focused, path: path.to_string(), ..Default::default() };
        }
        if path.is_empty() {
            return FollowChange::None;
        }
        let due = self.checked.is_none_or(|t| now - t >= FOLLOW_INTERVAL || now < t);
        if !force && !due {
            return FollowChange::None;
        }
        self.checked = Some(now);
        match self.read(force) {
            Ok(None) => FollowChange::None,
            Ok(Some(theme)) => {
                let name = theme.name.clone();
                self.theme = Some(theme);
                self.error = None;
                FollowChange::Loaded(name)
            }
            Err(e) if self.error.as_deref() == Some(e.as_str()) => FollowChange::None,
            Err(e) => {
                self.error = Some(e.clone());
                FollowChange::Failed(e)
            }
        }
    }

    /// The file's theme when it changed since the last read (`Ok(None)`: unchanged).
    fn read(&mut self, force: bool) -> Result<Option<ThemeFile>, String> {
        use std::io::Read;
        let path = std::path::Path::new(&self.path);
        let meta = std::fs::metadata(path).map_err(|e| format!("can't read {}: {e}", self.path))?;
        if !meta.is_file() {
            return Err(format!("{} is not a file", self.path));
        }
        if meta.len() > MAX_BYTES as u64 {
            return Err(format!("a theme file is at most {} KiB", MAX_BYTES / 1024));
        }
        let stamp = (meta.modified().ok(), meta.len());
        if !force && self.stamp == Some(stamp) {
            return Ok(None);
        }
        self.stamp = Some(stamp);
        // Never more than the cap, whatever the size said a moment ago.
        let mut text = String::new();
        std::fs::File::open(path)
            .and_then(|f| f.take(MAX_BYTES as u64 + 1).read_to_string(&mut text))
            .map_err(|e| format!("can't read {}: {e}", self.path))?;
        let theme = parse(&text)?;
        Ok(Some(ThemeFile { id: format!("file-{}", theme.id), ..theme }))
    }
}
