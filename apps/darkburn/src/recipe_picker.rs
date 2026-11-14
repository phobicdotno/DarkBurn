//! The quick recipe picker [MCC-04]: one click on a layer's colour in the Cuts / Layers panel
//! opens a small list of the Gweike recipes, a few typed letters ("acr 24", "ply 3", "mild 2")
//! narrow it, and Enter or a double-click puts the recipe on that layer. No dialog tabs, no
//! folder tree: the package folder is chosen once and remembered with the machine.
//!
//! Applying sets the layer's head and material and puts the recipe's craft on a craft slot that
//! no other layer of that head uses. The step can be taken back with the picker's Undo
//! (`gweike.recipe_undo`). Like the Gweike window, this only reads files; nothing talks to a
//! machine, and a recipe the craft checks flag with an error is shown but cannot be picked.

use darkburn_core::Document;
use darkburn_mcc::library::RecipeEntry;
use darkburn_mcc::{Craft, GasType, Library};
use darkburn_ui::ui_kit::{self, BackingTooltipExt as _, Button, StatusTone};
use egui::{Context, RichText};
use serde_json::{Value, json};
use solidcraft_theme::Tokens;

use crate::gweike_recipes::{Row, find, head_of, row, use_recipe};
use crate::gweike_workflow::{GweikeSettings, Head};
use crate::shell::DarkBurn;

/// How many recipes the picker keeps as recently used.
pub const RECENTS: usize = 5;

/// One line of the picker.
#[derive(Clone, Debug, PartialEq)]
pub struct Pick {
    pub row: Row,
    /// "Acrylic · 24 mm · CO2 · air".
    pub label: String,
    /// The key values: "17 mm/s · 60 % · air 0.5 bar".
    pub values: String,
    /// Used lately: shown at the top.
    pub recent: bool,
    /// The material the material filter goes by ("Acrylic").
    pub material: String,
}

fn gas_label(g: GasType) -> &'static str {
    match g {
        GasType::Air => "air",
        GasType::O2 => "O2",
        GasType::N2 => "N2",
        GasType::HighAir => "air (high)",
        GasType::HighO2 => "O2 (high)",
        GasType::HighN2 => "N2 (high)",
    }
}

fn gas_of(c: &Craft) -> (GasType, f64) {
    match c {
        Craft::Fibre(f) => (f.cut.gas, f.cut.gas_pressure_bar),
        Craft::Co2(c) => (c.gas, c.gas_pressure_bar),
    }
}

fn num(v: f64) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// The recipe's material, else its title.
pub fn material_of(e: &RecipeEntry) -> String {
    e.material.as_deref().map(str::trim).filter(|m| !m.is_empty()).map_or_else(|| e.title(), str::to_owned)
}

/// The materials of the package, sorted, each once (case is ignored), for the material filter.
pub fn materials(lib: &Library) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for e in &lib.recipes {
        let m = material_of(e);
        if !out.iter().any(|o| o.eq_ignore_ascii_case(&m)) {
            out.push(m);
        }
    }
    out.sort_by_key(|m| m.to_lowercase());
    out
}

/// "Acrylic · 24 mm · CO2 · air": material, thickness, head, gas.
pub fn label(e: &RecipeEntry) -> String {
    let mut parts = vec![material_of(e)];
    if let Some(t) = e.name.thickness_mm {
        parts.push(format!("{} mm", num(t)));
    }
    parts.push(match head_of(e.craft()) {
        Head::Co2 => "CO2".to_owned(),
        _ => "Fibre".to_owned(),
    });
    parts.push(gas_label(gas_of(e.craft()).0).to_owned());
    parts.join(" · ")
}

/// The values the recipe puts on the layer: speed, power and gas.
pub fn values(e: &RecipeEntry) -> String {
    let (speed, power) = match e.craft() {
        Craft::Fibre(f) => (f.cut.speed_mm_s, f.cut.power_pct),
        Craft::Co2(c) => (c.speed_mm_s, c.duty_pct),
    };
    let (gas, bar) = gas_of(e.craft());
    // The Gweike software shows speeds in m/min; the recipe files hold mm/s.
    format!("{} mm/s ({} m/min) · {} % · {} {} bar", num(speed), num(speed * 0.06), num(power), gas_label(gas), num(bar))
}

/// How well `words` fit a recipe; `None` when a word is missing. A word at the start of the
/// material or title counts more than one inside, and a number equal to the thickness most.
fn score(e: &RecipeEntry, lab: &str, hay: &str, words: &[String]) -> Option<u32> {
    let lab = lab.to_lowercase();
    let starts = |w: &str| lab.split(|c: char| !c.is_alphanumeric() && c != '.').any(|p| p.starts_with(w));
    let mut total = 0;
    for w in words {
        let thick = w.trim_end_matches("mm").parse::<f64>().ok().zip(e.name.thickness_mm).is_some_and(|(a, b)| (a - b).abs() < 1e-6);
        total += if thick {
            8
        } else if starts(w) {
            4
        } else if lab.contains(w.as_str()) {
            2
        } else if hay.contains(w.as_str()) {
            1
        } else {
            return None;
        };
    }
    Some(total)
}

/// The picker's list for `filter`: the recently used recipes first (most recent on top), then
/// the best matches, ties in the order the scan found them. Every word of the filter must match.
#[cfg(test)]
pub fn picks(lib: &Library, filter: &str, recents: &[String]) -> Vec<Pick> {
    picks_of(lib, filter, "", recents)
}

/// [`picks`] for one material only (case is ignored); an empty `material` means every material.
pub fn picks_of(lib: &Library, filter: &str, material: &str, recents: &[String]) -> Vec<Pick> {
    let material = material.trim();
    let words: Vec<String> = filter.split_whitespace().map(str::to_lowercase).collect();
    let mut out: Vec<(usize, u32, usize, Pick)> = lib
        .recipes
        .iter()
        .enumerate()
        .filter(|(_, e)| material.is_empty() || material_of(e).eq_ignore_ascii_case(material))
        .filter_map(|(i, e)| {
            let r = row(e);
            let lab = label(e);
            let hay = format!("{} {} {} {}", lab, r.title, r.key, r.detail).to_lowercase();
            let s = score(e, &lab, &hay, &words)?;
            let recent_at = recents.iter().position(|k| *k == r.key);
            let pick = Pick { label: lab, values: values(e), recent: recent_at.is_some(), material: material_of(e), row: r };
            Some((recent_at.unwrap_or(usize::MAX), s, i, pick))
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)).then(a.2.cmp(&b.2)));
    out.into_iter().map(|(_, _, _, p)| p).collect()
}

/// The craft slot a recipe for `head` goes on for `layer`: the layer's own slot when it already
/// uses that head and no other used layer shares it, else the first slot no other used layer of
/// that head uses (counting from the layer's number), else the layer's default slot.
pub fn slot_for(s: &GweikeSettings, doc: &Document, layer: usize, head: Head) -> usize {
    let n = darkburn_mcc::craft::LAYERS_PER_HEAD;
    let taken: Vec<usize> =
        doc.used_layers().into_iter().filter(|&l| l != layer).map(|l| s.map_of(l, doc)).filter(|m| m.head == head).map(|m| m.slot as usize).collect();
    let cur = s.map_of(layer, doc);
    if cur.head == head && !taken.contains(&(cur.slot as usize)) {
        return cur.slot as usize;
    }
    (0..n).map(|i| (layer + i) % n).find(|k| !taken.contains(k)).unwrap_or(layer % n)
}

/// Put the recipe on `layer`: its head, its material and its craft on a free slot. Returns the
/// slot and the warnings; a recipe with a craft check error is refused and nothing changes.
pub fn apply_to_layer(s: &mut GweikeSettings, doc: &Document, layer: usize, e: &RecipeEntry) -> Result<(usize, Vec<String>), String> {
    let head = head_of(e.craft());
    let slot = slot_for(s, doc, layer, head);
    let mut next = s.clone();
    let (_, mut warnings) = use_recipe(&mut next, e, slot)?;
    let shared: Vec<usize> = doc
        .used_layers()
        .into_iter()
        .filter(|&l| l != layer)
        .filter(|&l| s.map_of(l, doc).head == head && s.map_of(l, doc).slot as usize == slot)
        .collect();
    if !shared.is_empty() {
        let names: Vec<String> = shared.iter().map(|&l| darkburn_core::layer::layer_label(l)).collect();
        warnings.push(format!(
            "Every {} craft slot is in use: slot {slot} is shared with layer {}, which now cuts with this recipe too.",
            head.label(),
            names.join(", ")
        ));
    }
    let mut m = next.map_of(layer, doc);
    m.head = head;
    m.slot = slot as u8;
    if let Some(mat) = e.material.as_ref().filter(|m| !m.trim().is_empty()) {
        m.material = mat.trim().to_owned();
    }
    next.layers.insert(layer, m);
    let key = e.path.to_string_lossy().replace('\\', "/");
    next.recipe_recents.retain(|k| *k != key);
    next.recipe_recents.insert(0, key);
    next.recipe_recents.truncate(RECENTS);
    *s = next;
    Ok((slot, warnings))
}

// ----- Commands -----------------------------------------------------------------------------------

/// Read the remembered package folder when nothing has been read yet this session.
pub fn ensure_scanned(app: &mut DarkBurn) {
    if app.machine.ext.gweike_recipes.library.is_some() {
        return;
    }
    let folder = app.machine.active().settings.gweike.recipe_folder.clone();
    if !folder.trim().is_empty() && std::path::Path::new(&folder).is_dir() {
        let _ = crate::gweike_recipes::scan_command(app, &json!({ "path": folder }));
    }
}

/// `gweike.recipe_layer {recipe, layer?}`: put a recipe on a layer (the current layer when none
/// is given). Undo with `gweike.recipe_undo`.
pub fn layer_command(app: &mut DarkBurn, a: &Value) -> Result<Value, String> {
    let key = a.get("recipe").and_then(Value::as_str).ok_or("argument recipe must be text")?;
    let layer = match a.get("layer") {
        None | Some(Value::Null) => app.editor.current_layer,
        Some(v) => v.as_u64().filter(|l| *l < darkburn_core::layer::COLOUR_LAYERS as u64).ok_or("argument layer must be 0 to 29")? as usize,
    };
    ensure_scanned(app);
    let lib = app.machine.ext.gweike_recipes.library.as_ref().ok_or("No settings package read yet: choose the package folder first.")?;
    let e = find(lib, key)?;
    let title = label(e);
    let id = app.machine.active().id;
    let before = app.machine.active().settings.gweike.clone();
    let mut s = before.clone();
    let (slot, warnings) = apply_to_layer(&mut s, &app.editor.doc, layer, e)?;
    let head = head_of(e.craft());
    if let Some(d) = app.machine.devices.get_mut(id) {
        d.settings.gweike = s;
    }
    app.machine.ext.gweike_recipes.undo.push((id, before));
    let msg = format!("{title} is on layer {} ({} slot {slot}).", darkburn_core::layer::layer_label(layer), head.label());
    app.machine.ext.gweike_message = Some(msg.clone());
    app.machine.ext.gweike_recipes.pick_message = Some(msg.clone());
    Ok(
        json!({ "ok": true, "layer": layer, "slot": slot, "head": match head { Head::Co2 => "co2", _ => "fibre" }, "message": msg, "warnings": warnings }),
    )
}

/// `gweike.recipe_undo`: take back the last recipe put on a layer.
pub fn undo_command(app: &mut DarkBurn, _a: &Value) -> Result<Value, String> {
    let (id, before) = app.machine.ext.gweike_recipes.undo.pop().ok_or("No recipe to take back.")?;
    let d = app.machine.devices.get_mut(id).ok_or("The machine of that recipe is gone.")?;
    d.settings.gweike = before;
    let msg = "Recipe taken back.".to_owned();
    app.machine.ext.gweike_recipes.pick_message = Some(msg.clone());
    Ok(json!({ "ok": true, "message": msg }))
}

// ----- The popup ----------------------------------------------------------------------------------

/// Open the picker for `layer`, reading the remembered package folder if needed.
pub fn open(app: &mut DarkBurn, layer: usize) {
    ensure_scanned(app);
    let shelf = &mut app.machine.ext.gweike_recipes;
    shelf.picker = Some(layer);
    shelf.pick_filter.clear();
    shelf.pick_index = 0;
}

/// The picker popup, once per frame.
pub fn dialog(ctx: &Context, app: &mut DarkBurn) {
    let Some(layer) = app.machine.ext.gweike_recipes.picker else { return };
    let t = Tokens::get();
    let recents = app.machine.active().settings.gweike.recipe_recents.clone();
    let mut open = true;
    let mut cmd: Option<(&'static str, Value)> = None;
    let mut close = false;
    let title = format!("Recipe for layer {}", darkburn_core::layer::layer_label(layer));
    let shelf = &mut app.machine.ext.gweike_recipes;
    let (closed, _) =
        ui_kit::Window::new(egui::Id::unique("db_recipe_picker"), "db_material_test", &title).actions("", "", None).show(ctx, true, |ui| {
            ui.set_min_width(420.0);
            let Some(lib) = &shelf.library else {
                ui.label(
                    RichText::new("Pick the Gweike software's settings folder (or a copy of it) once; it is remembered.")
                        .size(t.font_small)
                        .color(t.text_dim),
                );
                if ui_kit::button(ui, "Choose package folder", Button::Small).clicked()
                    && let Some(dir) = rfd::FileDialog::new().pick_folder()
                {
                    cmd = Some(("gweike.recipes_scan", json!({ "path": dir.display().to_string() })));
                }
                return;
            };
            let edit = ui
                .add(egui::TextEdit::singleline(&mut shelf.pick_filter).hint_text("Search: acr 24, ply 3, mild 2 ...").desired_width(f32::INFINITY));
            crate::a11y::text(ui, &edit, "Search recipes");
            if !edit.has_focus() && !edit.lost_focus() {
                edit.request_focus();
            }
            if edit.changed() {
                shelf.pick_index = 0;
            }
            let before = shelf.pick_material.clone();
            let all = "All materials";
            egui::ComboBox::from_id_salt("db_recipe_pick_material")
                .selected_text(if shelf.pick_material.is_empty() { all } else { shelf.pick_material.as_str() })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut shelf.pick_material, String::new(), all);
                    for m in materials(lib) {
                        ui.selectable_value(&mut shelf.pick_material, m.clone(), m);
                    }
                });
            if shelf.pick_material != before {
                shelf.pick_index = 0;
            }
            let list = picks_of(lib, &shelf.pick_filter, &shelf.pick_material, &recents);
            let (down, up, enter, esc) = ui.input(|i| {
                (
                    i.key_pressed(egui::Key::ArrowDown),
                    i.key_pressed(egui::Key::ArrowUp),
                    i.key_pressed(egui::Key::Enter),
                    i.key_pressed(egui::Key::Escape),
                )
            });
            if down {
                shelf.pick_index = (shelf.pick_index + 1).min(list.len().saturating_sub(1));
            }
            if up {
                shelf.pick_index = shelf.pick_index.saturating_sub(1);
            }
            if esc {
                close = true;
            }
            if list.is_empty() {
                ui.label(RichText::new("No recipe matches.").size(t.font_small).color(t.text_dim));
            }
            let sel = shelf.pick_index.min(list.len().saturating_sub(1));
            if enter && let Some(p) = list.get(sel).filter(|p| p.row.errors.is_empty()) {
                cmd = Some(("gweike.recipe_layer", json!({ "recipe": p.row.key, "layer": layer })));
            }
            egui::ScrollArea::vertical().id_salt("db_recipe_pick_list").max_height(320.0).show(ui, |ui| {
                let mut last_recent = true;
                for (i, p) in list.iter().enumerate() {
                    if last_recent && !p.recent && i > 0 {
                        ui.separator();
                    }
                    last_recent = p.recent;
                    let usable = p.row.errors.is_empty();
                    let text = format!("{}{}\n{}", if p.recent { "\u{21ba} " } else { "" }, p.label, p.values);
                    let r = ui.add_enabled(usable, egui::Button::selectable(i == sel, text));
                    let r = if usable {
                        r.backing_on_hover_text(p.row.title.as_str())
                    } else {
                        r.backing_on_disabled_hover_text(p.row.errors.join("\n").as_str())
                    };
                    if i == sel && (down || up) {
                        r.scroll_to_me(None);
                    }
                    if r.clicked() {
                        shelf.pick_index = i;
                    }
                    if r.double_clicked() {
                        cmd = Some(("gweike.recipe_layer", json!({ "recipe": p.row.key, "layer": layer })));
                    }
                }
            });
            ui.horizontal(|ui| {
                if !shelf.undo.is_empty()
                    && ui_kit::button(ui, "Undo", Button::Small).backing_on_hover_text("Take back the last recipe put on a layer").clicked()
                {
                    cmd = Some(("gweike.recipe_undo", json!({})));
                }
                if ui_kit::button(ui, "Other folder...", Button::Small).clicked()
                    && let Some(dir) = rfd::FileDialog::new().pick_folder()
                {
                    cmd = Some(("gweike.recipes_scan", json!({ "path": dir.display().to_string() })));
                }
            });
            if let Some(m) = &shelf.pick_message {
                ui_kit::status_line(ui, m, StatusTone::Dim);
            }
        });
    if closed {
        open = false;
    }
    if !open || close {
        app.machine.ext.gweike_recipes.picker = None;
    }
    if let Some((c, args)) = cmd {
        let out = crate::api::execute_headless(app, c, &args);
        let shelf = &mut app.machine.ext.gweike_recipes;
        match out {
            Ok(v) if c == "gweike.recipe_layer" => {
                shelf.pick_message = v.get("message").and_then(Value::as_str).map(str::to_owned);
                shelf.picker = None;
            }
            Ok(v) => shelf.pick_message = v.get("message").and_then(Value::as_str).map(str::to_owned),
            Err(e) => shelf.pick_message = Some(e),
        }
    }
}
