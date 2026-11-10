//! The material / cut library: named collections of cut settings, grouped by material, then
//! thickness, then description, saved as `.dbclb` files. An entry can be assigned to a layer,
//! linked to it (the layer then says which entry it follows), made from a layer, edited,
//! duplicated and deleted. Libraries are chosen per device (the hook is [`LibraryState::device`]).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use darkburn_core::layer::{CutSetting, is_tool_layer};
use darkburn_core::{Document, Mode};
use darkburn_ui::ui_kit::{self, BackingTooltipExt as _};
use egui::{RichText, Ui};
use serde::{Deserialize, Serialize};
use solidcraft_theme::Tokens;

use crate::i18n::tr;
use crate::shell::DarkBurn;

pub const FILE_FORMAT: &str = "darkburn-cut-library";
pub const FILE_EXTENSION: &str = "dbclb";
/// A library with changes is saved to its file (or the auto-save file) this long after the change.
pub const AUTOSAVE_AFTER: Duration = Duration::from_secs(20);

/// What an entry is for, shown as a small icon before it in the list [LIB-04].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    Line,
    Fill,
    Image,
}

/// The side of the type icon in points.
const KIND_ICON: f32 = 14.0;

impl EntryKind {
    /// Line, image, or one of the fills (fill, fill and line, offset fill).
    pub fn of(mode: Mode) -> EntryKind {
        match mode {
            Mode::Line => EntryKind::Line,
            Mode::Image => EntryKind::Image,
            Mode::Fill | Mode::FillLine | Mode::OffsetFill => EntryKind::Fill,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            EntryKind::Line => "line",
            EntryKind::Fill => "fill",
            EntryKind::Image => "image",
        }
    }

    pub fn label(self) -> String {
        match self {
            EntryKind::Line => tr("library.kind_line", "Line"),
            EntryKind::Fill => tr("library.kind_fill", "Fill"),
            EntryKind::Image => tr("library.kind_image", "Image"),
        }
    }
}

/// The type icon: an outline for a line entry, a solid block for a fill, a framed picture
/// (sun and hill) for an image.
fn paint_kind_icon(p: &egui::Painter, r: egui::Rect, kind: EntryKind, ink: egui::Color32) {
    use egui::{Stroke, StrokeKind, pos2};
    let b = r.shrink(2.0);
    let stroke = Stroke::new(1.5, ink);
    match kind {
        EntryKind::Line => {
            p.rect_stroke(b, 1.0, stroke, StrokeKind::Inside);
        }
        EntryKind::Fill => {
            p.rect_filled(b, 1.0, ink);
        }
        EntryKind::Image => {
            p.rect_stroke(b, 1.0, Stroke::new(1.0, ink), StrokeKind::Inside);
            p.circle_filled(pos2(b.left() + b.width() * 0.3, b.top() + b.height() * 0.32), 1.4, ink);
            let hill = vec![
                pos2(b.left() + 1.0, b.bottom() - 1.0),
                pos2(b.left() + b.width() * 0.62, b.top() + b.height() * 0.42),
                pos2(b.right() - 1.0, b.bottom() - 1.0),
            ];
            p.add(egui::Shape::convex_polygon(hill, ink, Stroke::NONE));
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Entry {
    pub material: String,
    /// Material thickness in mm; none for entries that do not depend on it.
    pub thickness_mm: Option<f64>,
    pub description: String,
    /// The layer's settings: main first, then sub-layers.
    pub settings: Vec<CutSetting>,
}

impl Default for Entry {
    fn default() -> Self {
        Entry { material: String::new(), thickness_mm: None, description: String::new(), settings: vec![CutSetting::default()] }
    }
}

impl Entry {
    /// The name layers link to: unique within a library, independent of the display units.
    pub fn link_name(&self) -> String {
        let thickness = self.thickness_mm.map(|t| format!("{} mm", trim_number(t))).unwrap_or_default();
        [self.material.trim(), thickness.as_str(), self.description.trim()].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join(" - ")
    }

    pub fn mode(&self) -> Mode {
        self.settings.first().map(|s| s.mode).unwrap_or(Mode::Line)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Library {
    pub format: String,
    pub version: u32,
    pub name: String,
    /// The device this library is meant for (empty: any).
    pub device: String,
    pub entries: Vec<Entry>,
}

impl Default for Library {
    fn default() -> Self {
        Library { format: FILE_FORMAT.into(), version: 1, name: "Untitled library".into(), device: String::new(), entries: Vec::new() }
    }
}

/// One material in the grouped view: its thickness groups, each holding entry indexes.
pub type Group = (String, Vec<(Option<f64>, Vec<usize>)>);

impl Library {
    /// Entries grouped by material, then thickness (none first, then thin to thick), then
    /// description, filtered by `filter` (a word in the material, thickness or description).
    pub fn groups(&self, filter: &str) -> Vec<Group> {
        let words: Vec<String> = filter.split_whitespace().map(str::to_lowercase).collect();
        let mut idx: Vec<usize> = (0..self.entries.len())
            .filter(|&i| {
                let e = &self.entries[i];
                let hay = format!("{} {} {}", e.material, e.thickness_mm.map(trim_number).unwrap_or_default(), e.description).to_lowercase();
                words.iter().all(|w| hay.contains(w))
            })
            .collect();
        let key = |i: &usize| {
            let e = &self.entries[*i];
            (e.material.to_lowercase(), e.thickness_mm.map(|t| (t * 1000.0).round() as i64), e.description.to_lowercase())
        };
        idx.sort_by_key(key);
        let mut out: Vec<Group> = Vec::new();
        for i in idx {
            let e = &self.entries[i];
            match out.last_mut() {
                Some((m, _)) if m.eq_ignore_ascii_case(&e.material) => {}
                _ => out.push((e.material.clone(), Vec::new())),
            }
            let thicknesses = &mut out.last_mut().expect("a group was just pushed").1;
            match thicknesses.last_mut() {
                Some((t, v)) if *t == e.thickness_mm => v.push(i),
                _ => thicknesses.push((e.thickness_mm, vec![i])),
            }
        }
        out
    }

    /// Add `entry`, renaming its description if another entry has the same link name.
    pub fn add(&mut self, mut entry: Entry) -> usize {
        let base = entry.description.clone();
        let mut n = 1;
        while self.entries.iter().any(|e| e.link_name() == entry.link_name()) {
            n += 1;
            entry.description = if base.is_empty() { format!("{n}") } else { format!("{base} {n}") };
        }
        self.entries.push(entry);
        self.entries.len() - 1
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    pub fn from_json(text: &str) -> Result<Library, String> {
        let bad = |e: String| format!("{}: {e}", tr("library.bad_file", "This is not a cut library"));
        let value: serde_json::Value = serde_json::from_str(text).map_err(|e| bad(e.to_string()))?;
        if value.get("format").and_then(|f| f.as_str()) != Some(FILE_FORMAT) {
            return Err(tr("library.bad_file", "This is not a cut library"));
        }
        let mut lib: Library = serde_json::from_value(value).map_err(|e| bad(e.to_string()))?;
        // As for a project file: only what the Cut Settings editor could have made.
        for e in &mut lib.entries {
            e.settings.truncate(darkburn_core::MAX_SUB_LAYERS);
            if e.settings.is_empty() {
                e.settings.push(CutSetting::default());
            }
            e.settings.iter_mut().for_each(CutSetting::sanitise);
        }
        Ok(lib)
    }

    /// Save to `path`, keeping the previous file as `<name>.dbclb.bak`.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        if path.is_file() {
            let mut bak = path.as_os_str().to_owned();
            bak.push(".bak");
            let _ = std::fs::copy(path, PathBuf::from(bak));
        }
        crate::files::write_atomic(path, self.to_json().as_bytes()).map_err(|e| e.to_string())
    }

    pub fn load(path: &Path) -> Result<Library, String> {
        Library::from_json(&std::fs::read_to_string(path).map_err(|e| e.to_string())?)
    }
}

/// `3`, `3.2`, `0.125`: a number without trailing zeros.
pub fn trim_number(v: f64) -> String {
    let s = format!("{v:.3}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// A thickness as text: mm, or inches with a fraction (`1/4"`, `1 1/2"`) when one fits.
pub fn thickness_text(mm: f64, inches: bool) -> String {
    if !inches {
        return format!("{} mm", trim_number(mm));
    }
    let inch = mm / 25.4;
    let whole = inch.floor();
    let rest = inch - whole;
    for den in [2u32, 4, 8, 16, 32, 64] {
        let num = (rest * den as f64).round();
        if (rest - num / den as f64).abs() < 0.002 && num < den as f64 {
            return match (whole as u32, num as u32) {
                (w, 0) => format!("{w}\""),
                (0, n) => format!("{n}/{den}\""),
                (w, n) => format!("{w} {n}/{den}\""),
            };
        }
        if (rest - num / den as f64).abs() < 0.002 && num == den as f64 {
            return format!("{}\"", whole as u32 + 1);
        }
    }
    format!("{}\"", trim_number(inch))
}

/// The thickness in mm that `text` means: `3`, `3mm`, `1/4`, `1 1/2"`, `0.25in`. A bare number is
/// in the display unit `inches`.
pub fn parse_thickness(text: &str, inches: bool) -> Option<f64> {
    let t = text.trim().to_lowercase();
    let (body, unit_in) = if let Some(b) = t.strip_suffix("mm") {
        (b.trim(), false)
    } else if let Some(b) = t.strip_suffix("in").or_else(|| t.strip_suffix('"')) {
        (b.trim(), true)
    } else {
        (t.as_str(), inches)
    };
    let mut total = 0.0;
    let mut any = false;
    for part in body.split_whitespace() {
        total += match part.split_once('/') {
            Some((a, b)) => {
                let (a, b): (f64, f64) = (a.parse().ok()?, b.parse().ok()?);
                if b == 0.0 {
                    return None;
                }
                a / b
            }
            None => part.parse().ok()?,
        };
        any = true;
    }
    let mm = if unit_in { total * 25.4 } else { total };
    (any && mm.is_finite() && mm >= 0.0).then(|| (mm * 1e6).round() / 1e6)
}

/// Put `entry`'s settings on `layer` and link the layer to it.
pub fn assign(doc: &mut Document, layer: usize, entry: &Entry) -> Result<(), String> {
    if is_tool_layer(layer) {
        return Err(tr("library.tool_layer", "Tool layers have no cut settings"));
    }
    let l = doc.layer_mut(layer);
    l.settings = entry.settings.clone();
    if l.settings.is_empty() {
        l.settings.push(CutSetting::default());
    }
    l.library_link = Some(entry.link_name());
    Ok(())
}

/// A new entry from the settings of `layer`.
pub fn entry_from_layer(doc: &Document, layer: usize, material: &str, thickness_mm: Option<f64>, description: &str) -> Entry {
    let l = doc.layer(layer);
    let description = if description.trim().is_empty() { l.name.clone() } else { description.to_string() };
    Entry { material: material.to_string(), thickness_mm, description, settings: l.settings.clone() }
}

/// Links to `name` are removed from every layer; returns how many layers were unlinked.
pub fn unlink_all(doc: &mut Document, name: &str) -> usize {
    let mut n = 0;
    for i in 0..darkburn_core::LAYER_COUNT {
        if doc.layer(i).library_link.as_deref() == Some(name) {
            doc.layer_mut(i).library_link = None;
            n += 1;
        }
    }
    n
}

/// What the Library panel remembers while the program runs.
#[derive(Default)]
pub struct LibraryState {
    pub lib: Library,
    pub path: Option<PathBuf>,
    pub dirty_since: Option<Instant>,
    pub selected: Option<usize>,
    pub filter: String,
    pub status: String,
    /// The entry being edited, as a copy, with the thickness as typed.
    pub editing: Option<(usize, Entry, String)>,
    /// Text fields of "create from layer".
    pub new_material: String,
    pub new_thickness: String,
    pub new_description: String,
    /// The device whose library is shown; each device remembers its own file.
    pub device: String,
    pub loaded: bool,
    /// The settings folder: where a library without a file of its own is kept.
    pub settings_dir: Option<PathBuf>,
}

/// The file a library without a file of its own is kept in, one per device.
pub fn autosave_file(dir: &Path, device: &str) -> PathBuf {
    let slug: String = device.trim().chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).collect();
    let name = if slug.is_empty() { format!("library-autosave.{FILE_EXTENSION}") } else { format!("library-autosave-{slug}.{FILE_EXTENSION}") };
    dir.join(name)
}

/// Load the library of the active device once, before the panel or a command first uses it.
pub fn ensure_loaded(app: &mut DarkBurn) {
    let st = &mut app.extras.library;
    if st.loaded {
        return;
    }
    st.loaded = true;
    st.settings_dir = app.settings_dir.clone();
    if !st.is_dirty() {
        let dev = st.device.clone();
        st.switch_device(&dev, &app.prefs);
    }
}

impl LibraryState {
    pub fn touch(&mut self) {
        self.dirty_since.get_or_insert_with(Instant::now);
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty_since.is_some()
    }

    /// Switch to the library of `device`: the file remembered for it, else the library kept for
    /// it in the settings folder, else an empty one. Changes to the library shown are written
    /// first, so switching never drops them.
    pub fn switch_device(&mut self, device: &str, prefs: &crate::prefs::Prefs) {
        self.flush();
        self.device = device.to_string();
        self.editing = None;
        if let Some(p) = prefs.library_paths.get(device).map(PathBuf::from).filter(|p| p.is_file())
            && self.open(&p).is_ok()
        {
            return;
        }
        let kept = self.settings_dir.as_deref().map(|d| autosave_file(d, device)).filter(|p| p.is_file()).and_then(|p| Library::load(&p).ok());
        self.lib = kept.filter(|l| l.device == device).unwrap_or_else(|| Library { device: device.to_string(), ..Default::default() });
        self.path = None;
        self.dirty_since = None;
        self.selected = None;
    }

    /// Write unsaved changes now (to the library's file, or the one kept in the settings folder).
    pub fn flush(&mut self) {
        if let Some(since) = self.dirty_since {
            let dir = self.settings_dir.clone();
            self.autosave(dir.as_deref(), since + AUTOSAVE_AFTER);
        }
    }

    pub fn open(&mut self, path: &Path) -> Result<(), String> {
        self.lib = Library::load(path)?;
        self.path = Some(path.to_path_buf());
        self.dirty_since = None;
        self.selected = None;
        self.editing = None;
        Ok(())
    }

    pub fn save(&mut self) -> Result<(), String> {
        let path = self.path.clone().ok_or_else(|| tr("library.no_file", "The library has no file yet: use Save as"))?;
        self.lib.save(&path)?;
        self.dirty_since = None;
        Ok(())
    }

    pub fn save_as(&mut self, path: &Path) -> Result<(), String> {
        let path = crate::files::with_ending(path, FILE_EXTENSION);
        self.lib.save(&path)?;
        self.path = Some(path);
        self.dirty_since = None;
        Ok(())
    }

    /// Auto-save: a changed library is written some seconds after the change, to its file or, if
    /// it has none, to the device's `library-autosave` file in the settings folder (which is read
    /// back the next time the device's library is loaded).
    pub fn autosave(&mut self, settings_dir: Option<&Path>, now: Instant) {
        let Some(since) = self.dirty_since else { return };
        if now.duration_since(since) < AUTOSAVE_AFTER {
            return;
        }
        let ok = match (&self.path, settings_dir) {
            (Some(p), _) => self.lib.save(p).is_ok(),
            (None, Some(dir)) => {
                let _ = std::fs::create_dir_all(dir);
                self.lib.save(&autosave_file(dir, &self.device)).is_ok()
            }
            _ => false,
        };
        if ok {
            self.dirty_since = None;
        }
    }
}

// ----- Actions, shared by the panel and the commands -----

/// Add an entry made from `layer`.
pub fn create_from_layer(app: &mut DarkBurn, layer: usize, material: &str, thickness: Option<f64>, description: &str) -> usize {
    let entry = entry_from_layer(&app.editor.doc, layer, material, thickness, description);
    let i = app.extras.library.lib.add(entry);
    app.extras.library.selected = Some(i);
    app.extras.library.touch();
    i
}

/// Assign entry `index` to `layer` (undoable).
pub fn assign_to_layer(app: &mut DarkBurn, index: usize, layer: usize) -> Result<(), String> {
    let entry = app.extras.library.lib.entries.get(index).cloned().ok_or_else(|| tr("library.no_entry", "No such entry"))?;
    let mut result = Ok(());
    app.editor.edit("Assign library entry", |d, _| match assign(d, layer, &entry) {
        Ok(()) => true,
        Err(e) => {
            result = Err(e);
            false
        }
    });
    result
}

/// Link `layer` to entry `index` without touching the layer's settings.
pub fn link_layer(app: &mut DarkBurn, index: usize, layer: usize) -> Result<(), String> {
    let name = app.extras.library.lib.entries.get(index).map(Entry::link_name).ok_or_else(|| tr("library.no_entry", "No such entry"))?;
    app.editor.edit("Link layer to library", |d, _| {
        d.layer_mut(layer).library_link = Some(name);
        true
    });
    Ok(())
}

pub fn unlink_layer(app: &mut DarkBurn, layer: usize) {
    app.editor.edit("Unlink layer from library", |d, _| {
        let had = d.layer(layer).library_link.is_some();
        d.layer_mut(layer).library_link = None;
        had
    });
}

/// Delete entry `index`; layers linked to it are unlinked.
pub fn delete_entry(app: &mut DarkBurn, index: usize) -> Result<(), String> {
    let name = app.extras.library.lib.entries.get(index).map(Entry::link_name).ok_or_else(|| tr("library.no_entry", "No such entry"))?;
    app.extras.library.lib.entries.remove(index);
    app.extras.library.selected = None;
    app.extras.library.editing = None;
    app.extras.library.touch();
    app.editor.edit("Delete library entry", |d, _| unlink_all(d, &name) > 0);
    Ok(())
}

pub fn duplicate_entry(app: &mut DarkBurn, index: usize) -> Result<usize, String> {
    let mut e = app.extras.library.lib.entries.get(index).cloned().ok_or_else(|| tr("library.no_entry", "No such entry"))?;
    e.description = if e.description.is_empty() { tr("library.copy", "copy") } else { format!("{} ({})", e.description, tr("library.copy", "copy")) };
    let i = app.extras.library.lib.add(e);
    app.extras.library.selected = Some(i);
    app.extras.library.touch();
    Ok(i)
}

/// Replace entry `index` by `entry`; layers linked to the old name follow the new one.
pub fn replace_entry(app: &mut DarkBurn, index: usize, entry: Entry) -> Result<(), String> {
    let old = app.extras.library.lib.entries.get(index).map(Entry::link_name).ok_or_else(|| tr("library.no_entry", "No such entry"))?;
    let new = entry.link_name();
    app.extras.library.lib.entries[index] = entry;
    app.extras.library.touch();
    if old != new {
        app.editor.edit("Rename library entry", |d, _| {
            let mut any = false;
            for i in 0..darkburn_core::LAYER_COUNT {
                if d.layer(i).library_link.as_deref() == Some(old.as_str()) {
                    d.layer_mut(i).library_link = Some(new.clone());
                    any = true;
                }
            }
            any
        });
    }
    Ok(())
}

// ----- The panel -----

fn pick_save_path(name: &str) -> Option<PathBuf> {
    rfd::FileDialog::new().add_filter("Cut library", &[FILE_EXTENSION]).set_file_name(format!("{name}.{FILE_EXTENSION}")).save_file()
}

fn btn(ui: &mut Ui, text: &str, tip: &str, enabled: bool) -> bool {
    ui.add_enabled_ui(enabled, |ui| ui_kit::button(ui, text, ui_kit::Button::Small).backing_on_hover_text(tip).clicked()).inner
}

/// The Library panel.
pub fn show(ui: &mut Ui, app: &mut DarkBurn) {
    let t = Tokens::get();
    let inches = app.prefs.numeric_inches;
    ensure_loaded(app);
    let mut st = std::mem::take(&mut app.extras.library);
    let mut status = None;

    ui.horizontal(|ui| {
        let mut name = st.lib.name.clone();
        ui.label(RichText::new(tr("library.name", "Library")).size(t.font_small).color(t.text_dim));
        if ui_kit::named_control(ui, "Library name", |ui| ui_kit::field(ui, &mut name)) {
            st.lib.name = name;
            st.touch();
        }
    });
    let file = st
        .path
        .as_ref()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| tr("library.unsaved", "not saved yet"));
    let dirty = if st.is_dirty() { " *" } else { "" };
    let device = if st.device.is_empty() { tr("library.any_device", "any device") } else { st.device.clone() };
    ui.label(RichText::new(format!("{file}{dirty}  ({device})")).size(t.font_small).color(t.text_dim));
    ui.horizontal_wrapped(|ui| {
        if btn(ui, &tr("library.new", "New"), &tr("library.new_tip", "Start an empty library"), true) {
            st.lib = Library { device: st.device.clone(), ..Default::default() };
            st.path = None;
            st.selected = None;
            st.editing = None;
            // Changed: the empty library replaces the one kept for this device.
            st.dirty_since = None;
            st.touch();
        }
        if btn(ui, &tr("library.load", "Load"), &tr("library.load_tip", "Open a library file"), true)
            && let Some(p) = rfd::FileDialog::new().add_filter("Cut library", &[FILE_EXTENSION]).pick_file()
        {
            match st.open(&p) {
                Ok(()) => {
                    app.prefs.library_paths.insert(st.device.clone(), p.to_string_lossy().into_owned());
                }
                Err(e) => status = Some(e),
            }
        }
        if btn(ui, &tr("library.save", "Save"), &tr("library.save_tip", "Save the library; the old file is kept as a backup"), true) {
            if st.path.is_none() {
                if let Some(p) = pick_save_path(&st.lib.name) {
                    status = st.save_as(&p).err();
                }
            } else {
                status = st.save().err();
            }
            if let Some(p) = &st.path {
                app.prefs.library_paths.insert(st.device.clone(), p.to_string_lossy().into_owned());
            }
        }
        if btn(ui, &tr("library.save_as", "Save as"), "", true)
            && let Some(p) = pick_save_path(&st.lib.name)
        {
            status = st.save_as(&p).err();
            if let Some(p) = &st.path {
                app.prefs.library_paths.insert(st.device.clone(), p.to_string_lossy().into_owned());
            }
        }
    });
    ui.add_space(t.space_6);
    let search = tr("library.search", "Search the library");
    ui_kit::named_control(ui, &search, |ui| ui_kit::hint_field(ui, &mut st.filter, &search));
    ui.add_space(t.space_6);

    // The grouped list.
    let groups = st.lib.groups(&st.filter);
    if groups.is_empty() {
        ui.label(
            RichText::new(tr("library.empty", "No entries. Use Create from layer to add the current layer's settings."))
                .size(t.font_small)
                .color(t.text_dim),
        );
    }
    let linked: std::collections::HashSet<String> =
        (0..darkburn_core::LAYER_COUNT).filter_map(|i| app.editor.doc.layer(i).library_link.clone()).collect();
    for (material, thicknesses) in &groups {
        let title = if material.is_empty() { tr("library.no_material", "(no material)") } else { material.clone() };
        ui.label(RichText::new(title).strong().color(t.text));
        for (thickness, entries) in thicknesses {
            let heading = match thickness {
                Some(mm) => thickness_text(*mm, inches),
                None => tr("library.no_thickness", "any thickness"),
            };
            ui.label(RichText::new(format!("  {heading}")).size(t.font_small).color(t.text_dim));
            for &i in entries {
                let e = &st.lib.entries[i];
                let kind = EntryKind::of(e.mode());
                let mut text = if e.description.is_empty() { tr("library.no_description", "(no description)") } else { e.description.clone() };
                if linked.contains(&e.link_name()) {
                    text += &format!("  - {}", tr("library.linked", "linked"));
                }
                let selected = st.selected == Some(i);
                let mut clicked = false;
                ui.horizontal(|ui| {
                    ui.add_space(t.space_12);
                    let (icon, r) = ui.allocate_exact_size(egui::vec2(KIND_ICON, KIND_ICON), egui::Sense::click());
                    paint_kind_icon(ui.painter(), icon, kind, if selected { t.text } else { t.text_dim });
                    solidcraft_ui_registry::publish_target(&format!("library:kind:{}:{i}", kind.id()), icon);
                    clicked |= r.on_hover_text(kind.label()).clicked();
                    clicked |= ui.selectable_label(selected, text).on_hover_text(format!("{} ({})", e.link_name(), kind.label())).clicked();
                });
                if clicked {
                    st.selected = Some(i);
                }
            }
        }
    }
    ui.add_space(t.space_8);

    // Actions on the selection and the current layer.
    let layer = app.editor.current_layer;
    let have = st.selected.filter(|i| *i < st.lib.entries.len());
    let can_layer = !is_tool_layer(layer);
    ui.horizontal_wrapped(|ui| {
        if btn(
            ui,
            &tr("library.assign", "Assign to layer"),
            &tr("library.assign_tip", "Put this entry's settings on the current layer and link it"),
            have.is_some() && can_layer,
        ) && let Some(i) = have
        {
            status = assign_to_layer(app, i, layer).err();
        }
        if btn(
            ui,
            &tr("library.link", "Link"),
            &tr("library.link_tip", "Link the current layer to this entry without changing its settings"),
            have.is_some() && can_layer,
        ) && let Some(i) = have
        {
            status = link_layer(app, i, layer).err();
        }
        let is_linked = app.editor.doc.layer(layer).library_link.is_some();
        if btn(ui, &tr("library.unlink", "Unlink"), &tr("library.unlink_tip", "The current layer stops following its library entry"), is_linked) {
            unlink_layer(app, layer);
        }
    });
    ui.horizontal_wrapped(|ui| {
        if btn(ui, &tr("library.edit", "Edit"), "", have.is_some())
            && let Some(i) = have
        {
            let e = st.lib.entries[i].clone();
            let th = e.thickness_mm.map(|t| thickness_text(t, inches)).unwrap_or_default();
            st.editing = Some((i, e, th));
        }
        if btn(ui, &tr("library.duplicate", "Duplicate"), "", have.is_some())
            && let Some(i) = have
        {
            let _ = {
                app.extras.library = std::mem::take(&mut st);
                let r = duplicate_entry(app, i);
                st = std::mem::take(&mut app.extras.library);
                r
            };
        }
        if btn(ui, &tr("library.delete", "Delete"), &tr("library.delete_tip", "Delete the entry; layers linked to it are unlinked"), have.is_some())
            && let Some(i) = have
        {
            app.extras.library = std::mem::take(&mut st);
            let _ = delete_entry(app, i);
            st = std::mem::take(&mut app.extras.library);
        }
    });

    // Edit form.
    let mut finish: Option<Option<(usize, Entry)>> = None;
    if let Some((i, e, th)) = &mut st.editing {
        ui.add_space(t.space_8);
        ui_kit::section_heading(ui, &tr("library.edit_heading", "Edit entry"));
        ui_kit::row(ui, &tr("library.material", "Material"), |ui| {
            ui_kit::field(ui, &mut e.material);
        });
        ui_kit::row(ui, &tr("library.thickness", "Thickness"), |ui| {
            ui_kit::hint_field(ui, th, &tr("library.thickness_hint", "3 mm or 1/8\""));
        });
        ui_kit::row(ui, &tr("library.description", "Description"), |ui| {
            ui_kit::field(ui, &mut e.description);
        });
        let unit = app.prefs.opts.speed_unit();
        if let Some(s) = e.settings.first_mut() {
            let mut speed = app.prefs.opts.speed_value(s.speed);
            ui_kit::row(ui, &tr("library.speed", "Speed"), |ui| {
                ui_kit::number_field(ui, &mut speed, 0.01..=1_000_000.0, 1.0, &format!(" {unit}"));
            });
            s.speed = app.prefs.opts.speed_to_mm_s(speed);
            ui_kit::row(ui, &tr("library.max_power", "Max power"), |ui| {
                ui_kit::number_field(ui, &mut s.max_power, 0.0..=100.0, 0.2, " %");
            });
            ui_kit::row(ui, &tr("library.min_power", "Min power"), |ui| {
                ui_kit::number_field(ui, &mut s.min_power, 0.0..=100.0, 0.2, " %");
            });
            ui_kit::row(ui, &tr("library.interval", "Interval"), |ui| {
                ui_kit::number_field(ui, &mut s.interval, 0.001..=100.0, 0.001, " mm");
            });
            ui_kit::row(ui, &tr("library.passes", "Passes"), |ui| {
                ui_kit::number_field(ui, &mut s.passes, 1..=10_000, 1.0, "");
            });
        }
        ui.horizontal(|ui| {
            if btn(ui, &tr("library.apply", "Apply"), "", true) {
                let mut e = e.clone();
                e.thickness_mm = if th.trim().is_empty() { None } else { parse_thickness(th, inches).or(e.thickness_mm) };
                finish = Some(Some((*i, e)));
            }
            if btn(ui, &tr("library.cancel", "Cancel"), "", true) {
                finish = Some(None);
            }
        });
    }
    if let Some(done) = finish {
        st.editing = None;
        if let Some((i, e)) = done {
            app.extras.library = std::mem::take(&mut st);
            let _ = replace_entry(app, i, e);
            st = std::mem::take(&mut app.extras.library);
        }
    }

    // Create from layer.
    ui.add_space(t.space_8);
    ui_kit::section_heading(ui, &tr("library.create_heading", "Create from layer"));
    ui_kit::row(ui, &tr("library.material", "Material"), |ui| {
        ui_kit::field(ui, &mut st.new_material);
    });
    ui_kit::row(ui, &tr("library.thickness", "Thickness"), |ui| {
        ui_kit::hint_field(ui, &mut st.new_thickness, &tr("library.thickness_hint", "3 mm or 1/8\""));
    });
    ui_kit::row(ui, &tr("library.description", "Description"), |ui| {
        ui_kit::field(ui, &mut st.new_description);
    });
    if btn(
        ui,
        &tr("library.create", "Create from layer"),
        &tr("library.create_tip", "Add the current layer's cut settings as a new entry"),
        can_layer,
    ) {
        let (m, th, d) = (st.new_material.clone(), parse_thickness(&st.new_thickness, inches), st.new_description.clone());
        app.extras.library = std::mem::take(&mut st);
        create_from_layer(app, layer, &m, th, &d);
        st = std::mem::take(&mut app.extras.library);
        st.new_description.clear();
    }
    if let Some(s) = status {
        st.status = s;
    }
    if !st.status.is_empty() {
        ui_kit::status_line(ui, &st.status, ui_kit::StatusTone::Dim);
    }
    app.extras.library = st;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prefs::Prefs;

    fn entry(material: &str, thickness: Option<f64>, description: &str, speed: f64) -> Entry {
        let mut e = Entry { material: material.into(), thickness_mm: thickness, description: description.into(), ..Default::default() };
        e.settings[0].speed = speed;
        e
    }

    #[test]
    fn groups_are_material_then_thickness_then_description() {
        let mut lib = Library::default();
        lib.add(entry("Plywood", Some(6.0), "Cut", 10.0));
        lib.add(entry("Acrylic", Some(3.0), "Engrave", 200.0));
        lib.add(entry("Plywood", Some(3.0), "Score", 100.0));
        lib.add(entry("Plywood", Some(3.0), "Cut", 20.0));
        lib.add(entry("Plywood", None, "Any", 50.0));
        let g = lib.groups("");
        assert_eq!(g.iter().map(|(m, _)| m.as_str()).collect::<Vec<_>>(), ["Acrylic", "Plywood"]);
        let ply = &g[1].1;
        assert_eq!(ply.iter().map(|(t, _)| *t).collect::<Vec<_>>(), [None, Some(3.0), Some(6.0)]);
        let three: Vec<_> = ply[1].1.iter().map(|&i| lib.entries[i].description.as_str()).collect();
        assert_eq!(three, ["Cut", "Score"]);
        assert_eq!(lib.groups("acr 3").len(), 1);
        assert!(lib.groups("steel").is_empty());
    }

    #[test]
    fn duplicate_names_get_a_suffix_so_links_stay_unique() {
        let mut lib = Library::default();
        let a = lib.add(entry("Ply", Some(3.0), "Cut", 1.0));
        let b = lib.add(entry("Ply", Some(3.0), "Cut", 2.0));
        assert_ne!(lib.entries[a].link_name(), lib.entries[b].link_name());
    }

    #[test]
    fn thickness_text_uses_fractions_in_inches() {
        assert_eq!(thickness_text(6.35, true), "1/4\"");
        assert_eq!(thickness_text(38.1, true), "1 1/2\"");
        assert_eq!(thickness_text(25.4, true), "1\"");
        assert_eq!(thickness_text(3.0, false), "3 mm");
        assert_eq!(thickness_text(3.175, true), "1/8\"");
        assert_eq!(thickness_text(3.0, true), "0.118\"");
        assert_eq!(thickness_text(3.2, false), "3.2 mm");
    }

    #[test]
    fn thickness_text_parses_fractions_and_units() {
        assert_eq!(parse_thickness("1/4", true), Some(6.35));
        assert_eq!(parse_thickness("1/8 in", false), Some(3.175));
        assert_eq!(parse_thickness("1 1/2\"", false), Some(38.1));
        assert_eq!(parse_thickness("3", false), Some(3.0));
        assert_eq!(parse_thickness("3mm", true), Some(3.0));
        assert_eq!(parse_thickness("0.25in", false), Some(6.35));
        assert_eq!(parse_thickness("abc", false), None);
        assert_eq!(parse_thickness("1/0", false), None);
        assert_eq!(parse_thickness("", false), None);
    }

    #[test]
    fn library_files_round_trip_and_keep_a_backup() {
        let dir = std::env::temp_dir().join(format!("darkburn-lib-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("wood.dbclb");
        let mut st = LibraryState::default();
        st.lib.name = "Wood".into();
        st.lib.add(entry("Ply", Some(3.0), "Cut", 12.0));
        st.save_as(&path).unwrap();
        let back = Library::load(&path).unwrap();
        assert_eq!(back, st.lib);
        st.lib.add(entry("Ply", Some(6.0), "Cut", 6.0));
        st.touch();
        st.save().unwrap();
        assert!(dir.join("wood.dbclb.bak").is_file(), "the previous file is kept");
        assert_eq!(Library::load(&dir.join("wood.dbclb.bak")).unwrap().entries.len(), 1);
        assert_eq!(Library::load(&path).unwrap().entries.len(), 2);
        std::fs::write(dir.join("bad.dbclb"), "{}").unwrap();
        assert!(Library::load(&dir.join("bad.dbclb")).is_err());
        std::fs::write(dir.join("junk.dbclb"), "not json").unwrap();
        assert!(Library::load(&dir.join("junk.dbclb")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn autosave_writes_after_the_delay() {
        let dir = std::env::temp_dir().join(format!("darkburn-lib-auto-{}", std::process::id()));
        let mut st = LibraryState::default();
        st.lib.add(entry("Ply", None, "x", 1.0));
        st.touch();
        let now = Instant::now();
        st.autosave(Some(&dir), now);
        assert!(st.is_dirty(), "too soon");
        st.autosave(Some(&dir), now + AUTOSAVE_AFTER + Duration::from_secs(1));
        assert!(!st.is_dirty());
        assert!(dir.join("library-autosave.dbclb").is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn assigning_links_and_deleting_unlinks() {
        let mut app = DarkBurn::new(Prefs::default());
        app.editor.doc.layer_mut(3).main_mut().speed = 77.0;
        let i = create_from_layer(&mut app, 3, "Ply", Some(3.0), "Cut");
        let name = app.extras.library.lib.entries[i].link_name();
        assert_eq!(app.extras.library.lib.entries[i].settings[0].speed, 77.0);
        assign_to_layer(&mut app, i, 5).unwrap();
        assert_eq!(app.editor.doc.layer(5).main().speed, 77.0);
        assert_eq!(app.editor.doc.layer(5).library_link.as_deref(), Some(name.as_str()));
        link_layer(&mut app, i, 6).unwrap();
        assert_eq!(app.editor.doc.layer(6).library_link.as_deref(), Some(name.as_str()));
        unlink_layer(&mut app, 6);
        assert!(app.editor.doc.layer(6).library_link.is_none());
        assert!(assign_to_layer(&mut app, i, 30).is_err(), "tool layers take no settings");
        // Editing the name moves the links along.
        let mut e = app.extras.library.lib.entries[i].clone();
        e.description = "Cut fast".into();
        replace_entry(&mut app, i, e).unwrap();
        assert_eq!(app.editor.doc.layer(5).library_link.as_deref(), Some("Ply - 3 mm - Cut fast"));
        let d = duplicate_entry(&mut app, i).unwrap();
        assert_eq!(app.extras.library.lib.entries.len(), 2);
        assert_ne!(d, i);
        delete_entry(&mut app, i).unwrap();
        assert!(app.editor.doc.layer(5).library_link.is_none(), "deleting the entry unlinks layers");
        assert_eq!(app.extras.library.lib.entries.len(), 1);
    }

    #[test]
    fn each_device_remembers_its_library() {
        let dir = std::env::temp_dir().join(format!("darkburn-lib-dev-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut prefs = Prefs::default();
        let mut st = LibraryState::default();
        st.switch_device("diode", &prefs);
        st.lib.add(entry("Card", None, "Engrave", 300.0));
        let p = dir.join("diode.dbclb");
        st.save_as(&p).unwrap();
        prefs.library_paths.insert("diode".into(), p.to_string_lossy().into_owned());
        st.switch_device("co2", &prefs);
        assert!(st.lib.entries.is_empty());
        st.switch_device("diode", &prefs);
        assert_eq!(st.lib.entries.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A library that was never given a file is kept in the settings folder, per device, and is
    /// there again after a restart or after a look at another device.
    #[test]
    fn a_library_without_a_file_comes_back_after_a_restart() {
        let dir = std::env::temp_dir().join(format!("darkburn-lib-kept-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let prefs = Prefs::default();
        let mut st = LibraryState { settings_dir: Some(dir.clone()), ..Default::default() };
        st.switch_device("diode", &prefs);
        st.lib.add(entry("Card", None, "Engrave", 300.0));
        st.touch();
        st.autosave(Some(&dir), Instant::now() + AUTOSAVE_AFTER + Duration::from_secs(1));
        assert!(!st.is_dirty(), "it was written");
        // The next start.
        let mut again = LibraryState { settings_dir: Some(dir.clone()), ..Default::default() };
        again.switch_device("diode", &prefs);
        assert_eq!(again.lib.entries.len(), 1, "the auto-saved library is read back");
        // Another device starts empty; its entries do not replace the first device's.
        again.switch_device("co2", &prefs);
        assert!(again.lib.entries.is_empty());
        again.lib.add(entry("Acrylic", Some(3.0), "Cut", 8.0));
        again.lib.add(entry("Acrylic", Some(5.0), "Cut", 4.0));
        again.touch();
        again.switch_device("diode", &prefs);
        assert_eq!(again.lib.entries.len(), 1, "the diode's library");
        again.switch_device("co2", &prefs);
        assert_eq!(again.lib.entries.len(), 2, "changes made just before switching are not dropped");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An entry made by a command before the Library panel was ever shown is still there when
    /// the panel first draws.
    #[test]
    fn entries_made_before_the_panel_was_shown_stay() {
        use crate::ui_harness::Gui;
        let mut g = Gui::standard();
        g.exec("library.create_from_layer", serde_json::json!({ "layer": 2, "material": "Birch", "thickness": 3.0, "description": "Cut" }));
        g.click_id("panel:tab:library");
        g.frames(2);
        assert_eq!(g.app().extras.library.lib.entries.len(), 1);
        assert!(g.visible_texts().join(" ").contains("Birch"), "{:?}", g.visible_texts());
    }
}
