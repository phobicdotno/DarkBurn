//! Fast recipe switching for the Gweike M3 Ultra [MCC-04].
//!
//! The vendor software keeps its cutting, cleaning and quenching recipes as XML files in a
//! settings package (head folder, material folder, one file per recipe). This module reads a
//! copy of that package with `darkburn_mcc::library::scan` and puts a recipe on a craft slot with
//! one click, so switching material or thickness is a pick from a list instead of typing every
//! value into the craft editor.
//!
//! It only reads files. Nothing here writes to the package or talks to a machine: the craft lands
//! in DarkBurn's craft table, which goes out in the `.chf` and the craft table beside it for the
//! operator to check in the vendor software. The machine's height follower, piercing and gas
//! logic stay in charge [MCC-07]. A recipe the craft checks flag with an error (a value outside
//! the machine's limits) is listed but not used.

use std::path::{Path, PathBuf};

use darkburn_mcc::craft::{CraftIssue, Severity};
use darkburn_mcc::library::{HeadFolder, RecipeEntry};
use darkburn_mcc::{Craft, Library, RecipeKind};
use darkburn_ui::ui_kit::{self, BackingTooltipExt as _, Button, StatusTone};
use egui::{RichText, Ui};
use serde_json::{Value, json};
use solidcraft_theme::Tokens;

use crate::gweike_workflow::{GweikeSettings, Head};
use crate::shell::DarkBurn;

/// The recipe list of the Gweike window: the package folder read last and its contents.
#[derive(Default)]
pub struct RecipeShelf {
    /// The folder scanned.
    pub folder: Option<PathBuf>,
    /// What the scan found.
    pub library: Option<Library>,
    /// The list's text filter.
    pub filter: String,
    /// The quick picker is open for this layer (`recipe_picker.rs`).
    pub picker: Option<usize>,
    /// The quick picker's search text and highlighted line.
    pub pick_filter: String,
    pub pick_index: usize,
    /// The quick picker's material filter; empty means every material.
    pub pick_material: String,
    /// The last thing the quick picker did, shown under the layer list.
    pub pick_message: Option<String>,
    /// Device and its Gweike settings before each recipe put on a layer, for `gweike.recipe_undo`.
    pub undo: Vec<(u64, GweikeSettings)>,
}

/// One recipe as the list shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    /// The path below the package folder, with `/` between the parts: what `gweike.recipe_use`
    /// takes.
    pub key: String,
    /// The file stem.
    pub title: String,
    pub head: Head,
    /// Head folder, material folder and kind, as one line.
    pub detail: String,
    /// The craft check errors: such a recipe is not used.
    pub errors: Vec<String>,
    /// The craft check warnings.
    pub warnings: Vec<String>,
}

/// The head a craft is for.
pub fn head_of(c: &Craft) -> Head {
    match c {
        Craft::Fibre(_) => Head::Fibre,
        Craft::Co2(_) => Head::Co2,
    }
}

fn key_of(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

fn issues(list: &[CraftIssue], severity: Severity) -> Vec<String> {
    list.iter().filter(|i| i.severity == severity).map(|i| format!("{}: {}", i.path, i.message)).collect()
}

fn detail(e: &RecipeEntry) -> String {
    let mut parts = Vec::new();
    match &e.head {
        Some(HeadFolder::Fibre { watts, air: false }) => parts.push(format!("Fibre {watts} W")),
        Some(HeadFolder::Fibre { watts, air: true }) => parts.push(format!("Fibre {watts} W, air")),
        Some(HeadFolder::Co2) => parts.push("CO2".to_owned()),
        None => parts.push(head_of(e.craft()).label().to_owned()),
    }
    if let Some(m) = &e.material {
        parts.push(m.clone());
    }
    match e.kind {
        RecipeKind::Cutting => {}
        RecipeKind::Cleaning => parts.push("cleaning".to_owned()),
        RecipeKind::Quenching => parts.push("quenching".to_owned()),
    }
    parts.join(", ")
}

/// The recipe as a list row.
pub fn row(e: &RecipeEntry) -> Row {
    Row {
        key: key_of(&e.path),
        title: e.title(),
        head: head_of(e.craft()),
        detail: detail(e),
        errors: issues(&e.issues, Severity::Error),
        warnings: issues(&e.issues, Severity::Warning),
    }
}

/// The recipes for `head` (all heads when `None`) whose title, path or material holds every word
/// of `filter` (case is ignored), in the order the scan found them.
pub fn rows(lib: &Library, head: Option<Head>, filter: &str) -> Vec<Row> {
    let words: Vec<String> = filter.split_whitespace().map(str::to_lowercase).collect();
    lib.recipes
        .iter()
        .filter(|e| head.is_none_or(|h| head_of(e.craft()) == h))
        .map(row)
        .filter(|r| {
            let hay = format!("{} {} {}", r.title, r.key, r.detail).to_lowercase();
            words.iter().all(|w| hay.contains(w.as_str()))
        })
        .collect()
}

/// The recipe `key` names: its path below the package folder (either slash), else its title when
/// exactly one recipe has it (case is ignored).
pub fn find<'a>(lib: &'a Library, key: &str) -> Result<&'a RecipeEntry, String> {
    let k = key.trim().replace('\\', "/");
    if let Some(e) = lib.recipes.iter().find(|e| key_of(&e.path) == k) {
        return Ok(e);
    }
    let mut by_title = lib.recipes.iter().filter(|e| e.title().eq_ignore_ascii_case(&k));
    match (by_title.next(), by_title.next()) {
        (Some(e), None) => Ok(e),
        (Some(_), Some(_)) => Err(format!("More than one recipe is called {k}; give its path below the package folder.")),
        _ => Err(format!("No recipe {k} in the package.")),
    }
}

/// Put the recipe's craft on `slot` of its head. A recipe with a craft check error is refused and
/// the table keeps what it had. The warnings come back.
pub fn use_recipe(s: &mut GweikeSettings, e: &RecipeEntry, slot: usize) -> Result<(Head, Vec<String>), String> {
    let n = darkburn_mcc::craft::LAYERS_PER_HEAD;
    if slot >= n {
        return Err(format!("Craft slots are 0 to {}.", n - 1));
    }
    if let Some(err) = issues(&e.issues, Severity::Error).first() {
        return Err(format!("The recipe {} was not used, it has a value the machine does not allow ({err}).", e.title()));
    }
    let head = head_of(e.craft());
    match e.craft() {
        Craft::Fibre(c) => s.crafts.fibre[slot] = c.clone(),
        Craft::Co2(c) => s.crafts.co2[slot] = c.clone(),
    }
    Ok((head, issues(&e.issues, Severity::Warning)))
}

/// What a scan found, in numbers.
pub fn summary(lib: &Library) -> Value {
    json!({
        "recipes": lib.recipes.len(),
        "cutting": lib.recipes_of(RecipeKind::Cutting).count(),
        "cleaning": lib.recipes_of(RecipeKind::Cleaning).count(),
        "quenching": lib.recipes_of(RecipeKind::Quenching).count(),
        "fibre": lib.recipes.iter().filter(|e| head_of(e.craft()) == Head::Fibre).count(),
        "co2": lib.recipes.iter().filter(|e| head_of(e.craft()) == Head::Co2).count(),
        "refused": refused(lib),
        "with_warnings": lib.flagged().len() - refused(lib),
        "machine_settings": lib.profiles.len(),
        "unreadable": lib.unreadable.len(),
    })
}

/// How many recipes have a craft check error, so are not used.
fn refused(lib: &Library) -> usize {
    lib.flagged().iter().filter(|(_, i)| i.iter().any(|x| x.severity == Severity::Error)).count()
}

fn summary_line(lib: &Library) -> String {
    let flagged = refused(lib);
    let mut s = format!("{} recipes found", lib.recipes.len());
    if flagged > 0 {
        s.push_str(&format!(", {flagged} with a value the machine does not allow"));
    }
    if !lib.unreadable.is_empty() {
        s.push_str(&format!(", {} files could not be read", lib.unreadable.len()));
    }
    s.push('.');
    s
}

// ----- Commands -----------------------------------------------------------------------------------

fn head_arg(a: &Value) -> Result<Option<Head>, String> {
    match a.get("head").and_then(Value::as_str).map(str::to_ascii_lowercase).as_deref() {
        None | Some("") | Some("all") => Ok(None),
        Some("fibre") | Some("fiber") => Ok(Some(Head::Fibre)),
        Some("co2") => Ok(Some(Head::Co2)),
        Some(other) => Err(format!("unknown head {other}: use fibre, co2 or all")),
    }
}

/// `gweike.recipes_scan {path}`: read a settings package folder.
pub fn scan_command(app: &mut DarkBurn, a: &Value) -> Result<Value, String> {
    let path = a.get("path").and_then(Value::as_str).ok_or("argument path must be text")?;
    let dir = PathBuf::from(path);
    if !dir.is_dir() {
        return Err(format!("{} is not a folder.", dir.display()));
    }
    let lib = darkburn_mcc::library::scan(&dir);
    let mut out = summary(&lib);
    out["ok"] = json!(true);
    out["path"] = json!(dir.display().to_string());
    let id = app.machine.active().id;
    if let Some(d) = app.machine.devices.get_mut(id) {
        d.settings.gweike.recipe_folder = dir.display().to_string();
    }
    let shelf = &mut app.machine.ext.gweike_recipes;
    shelf.folder = Some(dir);
    shelf.library = Some(lib);
    Ok(out)
}

/// `gweike.recipes {head?, filter?}`: the recipes of the package read last.
pub fn list_command(app: &DarkBurn, a: &Value) -> Result<Value, String> {
    let lib = app.machine.ext.gweike_recipes.library.as_ref().ok_or("No settings package read yet: run gweike.recipes_scan first.")?;
    let head = head_arg(a)?;
    let filter = a.get("filter").and_then(Value::as_str).unwrap_or("");
    let list: Vec<Value> = rows(lib, head, filter)
        .into_iter()
        .map(|r| {
            json!({
                "recipe": r.key, "title": r.title, "head": match r.head { Head::Co2 => "co2", _ => "fibre" },
                "detail": r.detail, "usable": r.errors.is_empty(), "errors": r.errors, "warnings": r.warnings,
            })
        })
        .collect();
    Ok(json!({ "recipes": list }))
}

/// `gweike.recipe_use {recipe, slot?}`: put a recipe on a craft slot of its head (the slot open
/// in the craft editor when none is given) and show it in the editor.
pub fn use_command(app: &mut DarkBurn, a: &Value) -> Result<Value, String> {
    let key = a.get("recipe").and_then(Value::as_str).ok_or("argument recipe must be text")?;
    let slot = match a.get("slot") {
        None | Some(Value::Null) => app.machine.ext.gweike_craft_slot,
        Some(v) => v.as_u64().ok_or("argument slot must be a whole number")? as usize,
    };
    let lib = app.machine.ext.gweike_recipes.library.as_ref().ok_or("No settings package read yet: run gweike.recipes_scan first.")?;
    let e = find(lib, key)?;
    let title = e.title();
    let id = app.machine.active().id;
    let mut s = app.machine.active().settings.gweike.clone();
    let (head, warnings) = use_recipe(&mut s, e, slot)?;
    if let Some(d) = app.machine.devices.get_mut(id) {
        d.settings.gweike = s;
    }
    app.machine.ext.gweike_craft_head = head;
    app.machine.ext.gweike_craft_slot = slot;
    app.machine.ext.gweike_message = Some(format!("{title} is on {} slot {slot}.", head.label()));
    Ok(json!({
        "ok": true, "title": title, "head": match head { Head::Co2 => "co2", _ => "fibre" }, "slot": slot, "warnings": warnings,
    }))
}

// ----- The window section -------------------------------------------------------------------------

fn note(ui: &mut Ui, text: &str) {
    let t = Tokens::get();
    ui.label(RichText::new(text).size(t.font_small).color(t.text_dim));
}

/// The recipe list in the Gweike window, for the head and slot open in the craft editor. Returns
/// the command a click asks for.
pub fn section(ui: &mut Ui, shelf: &mut RecipeShelf, head: Head, slot: usize) -> Option<(&'static str, Value)> {
    let mut cmd = None;
    ui.horizontal_wrapped(|ui| {
        if ui_kit::button(ui, "Open package folder...", Button::Small)
            .backing_on_hover_text("Read the recipes of a copy of the Gweike software's settings folder")
            .clicked()
            && let Some(dir) = rfd::FileDialog::new().pick_folder()
        {
            cmd = Some(("gweike.recipes_scan", json!({ "path": dir.display().to_string() })));
        }
        if let Some(dir) = shelf.folder.clone()
            && ui_kit::button(ui, "Read again", Button::Small).backing_on_hover_text("Read the same folder again").clicked()
        {
            cmd = Some(("gweike.recipes_scan", json!({ "path": dir.display().to_string() })));
        }
    });
    let Some(lib) = &shelf.library else {
        note(ui, "Open the Gweike software's settings folder (or a copy of it) to pick its recipes with one click.");
        return cmd;
    };
    note(ui, &summary_line(lib));
    ui_kit::hint_field(ui, &mut shelf.filter, "Filter: material, thickness, gas");
    let list = rows(lib, Some(head), &shelf.filter);
    if list.is_empty() {
        note(ui, &format!("No {} recipes match.", head.label()));
        return cmd;
    }
    egui::ScrollArea::vertical().id_salt("gw_recipe_list").max_height(240.0).show(ui, |ui| {
        for (i, r) in list.iter().enumerate() {
            ui.push_id(("gw_recipe", i), |ui| {
                ui.horizontal(|ui| {
                    let usable = r.errors.is_empty();
                    let tip = if usable { format!("Put this recipe on slot {slot}") } else { r.errors.join("\n") };
                    if ui
                        .add_enabled_ui(usable, |ui| {
                            ui_kit::button(ui, "Use", Button::Small)
                                .backing_on_hover_text(tip.as_str())
                                .backing_on_disabled_hover_text(tip.as_str())
                                .clicked()
                        })
                        .inner
                    {
                        cmd = Some(("gweike.recipe_use", json!({ "recipe": r.key, "slot": slot })));
                    }
                    ui.label(r.title.as_str());
                    note(ui, &r.detail);
                });
                for e in &r.errors {
                    ui_kit::status_line(ui, e, StatusTone::Warning);
                }
            });
        }
    });
    cmd
}
