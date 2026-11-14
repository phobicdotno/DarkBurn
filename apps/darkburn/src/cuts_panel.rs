//! The Cuts / Layers panel: the layers in use, in output order, with their mode, speed and
//! power and output / show / air switches; priority, reorder by drag, delete, copy and paste
//! of cut settings; and a quick settings box for the current layer.

use darkburn_core::layer::{LaserSelect, Mode, is_tool_layer, layer_label};
use darkburn_core::{CutSetting, Document, Shape, ShapeKind};
use darkburn_ui::ui_kit::{self, BackingTooltipExt as _, Menu, MenuItem};
use egui::{Align2, FontId, Id, Rect, RichText, Sense, Stroke, Ui, pos2, vec2};
use solidcraft_theme::Tokens;

use crate::numeric::{self, Num};
use crate::palette::layer_colour;
use crate::shell::DarkBurn;

const ROW_HEIGHT: f32 = 26.0;

/// The commands of the list header's context menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeaderCmd {
    ShowAll,
    HideAll,
    EnableAll,
    DisableAll,
    InvertOutput,
    AirOn,
    AirOff,
    AirInvert,
    SortCutsLast,
}

impl HeaderCmd {
    const MENU: [(HeaderCmd, &'static str); 9] = [
        (HeaderCmd::ShowAll, "Show all"),
        (HeaderCmd::HideAll, "Hide all"),
        (HeaderCmd::EnableAll, "Enable output for all"),
        (HeaderCmd::DisableAll, "Disable output for all"),
        (HeaderCmd::InvertOutput, "Invert output"),
        (HeaderCmd::AirOn, "Air on for all"),
        (HeaderCmd::AirOff, "Air off for all"),
        (HeaderCmd::AirInvert, "Invert air"),
        (HeaderCmd::SortCutsLast, "Sort cuts last"),
    ];
}

/// The switches in each row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Switch {
    Output,
    Show,
    Air,
}

/// Run a header menu command on the layers in use.
pub fn header_command(doc: &mut Document, cmd: HeaderCmd) -> bool {
    if cmd == HeaderCmd::SortCutsLast {
        let before = doc.used_layers();
        doc.sort_cuts_last();
        return doc.used_layers() != before;
    }
    let mut changed = false;
    for l in doc.used_layers() {
        let layer = doc.layer_mut(l);
        let before = (layer.output, layer.show, layer.air);
        match cmd {
            HeaderCmd::ShowAll => layer.show = true,
            HeaderCmd::HideAll => layer.show = false,
            HeaderCmd::EnableAll => layer.output = !layer.is_tool(),
            HeaderCmd::DisableAll => layer.output = false,
            HeaderCmd::InvertOutput => layer.output = !layer.output && !layer.is_tool(),
            HeaderCmd::AirOn => layer.air = !layer.is_tool(),
            HeaderCmd::AirOff => layer.air = false,
            HeaderCmd::AirInvert => layer.air = !layer.air && !layer.is_tool(),
            HeaderCmd::SortCutsLast => {}
        }
        changed |= before != (layer.output, layer.show, layer.air);
    }
    changed
}

/// Flip one switch of `layer` (tool layers never output).
pub fn toggle(doc: &mut Document, layer: usize, which: Switch) -> bool {
    let l = doc.layer_mut(layer);
    match which {
        Switch::Output => {
            if l.is_tool() {
                return false;
            }
            l.output = !l.output;
        }
        Switch::Show => l.show = !l.show,
        Switch::Air => l.air = !l.air,
    }
    true
}

fn walk(s: &Shape, f: &mut impl FnMut(&Shape)) {
    match &s.kind {
        ShapeKind::Group { children } => children.iter().for_each(|c| walk(c, f)),
        _ => f(s),
    }
}

/// The modes a layer can take: Image only for bitmaps, the vector modes for the rest.
pub fn modes_for(doc: &Document, layer: usize) -> Vec<Mode> {
    let (mut images, mut vectors) = (false, false);
    for s in doc.iter() {
        walk(s, &mut |leaf| {
            if leaf.layer == layer {
                if leaf.is_image() {
                    images = true;
                } else {
                    vectors = true;
                }
            }
        });
    }
    match (images, vectors) {
        (true, false) => vec![Mode::Image],
        (false, _) => Mode::ALL.into_iter().filter(|m| *m != Mode::Image).collect(),
        (true, true) => Mode::ALL.to_vec(),
    }
}

/// Set the main mode of `layer`.
pub fn set_mode(doc: &mut Document, layer: usize, mode: Mode) -> bool {
    let s = doc.layer_mut(layer).main_mut();
    if s.mode == mode {
        return false;
    }
    s.mode = mode;
    true
}

/// Move `layer` so it is output at `index` in the list of layers in use (0 = first; the
/// number of layers = last).
pub fn reorder(doc: &mut Document, layer: usize, index: usize) -> bool {
    let used = doc.used_layers();
    let Some(from) = used.iter().position(|l| *l == layer) else { return false };
    if index == from || index == from + 1 {
        return false;
    }
    let before = used.get(index).copied();
    doc.place_layer_before(layer, before);
    doc.used_layers() != used
}

/// Replace the cut settings of `layer` with `settings` (sub-layers included).
pub fn paste_settings(doc: &mut Document, layer: usize, settings: &[CutSetting]) -> bool {
    if settings.is_empty() {
        return false;
    }
    let l = doc.layer_mut(layer);
    let mut new: Vec<CutSetting> = settings.iter().take(darkburn_core::MAX_SUB_LAYERS).cloned().collect();
    // The layer keeps its own name.
    new[0].name = l.settings[0].name.clone();
    if l.settings == new {
        return false;
    }
    l.settings = new;
    true
}

/// Rename `layer` (an empty name is refused).
pub fn rename(doc: &mut Document, layer: usize, name: &str) -> bool {
    let name = name.trim();
    let l = doc.layer_mut(layer);
    if name.is_empty() || l.name == name {
        return false;
    }
    l.name = name.to_string();
    true
}

enum Act {
    Select(usize),
    Open(usize),
    StartRename(usize),
    Rename(usize, String),
    Mode(usize, Mode),
    Switch(usize, Switch),
    Reorder(usize, usize),
    Move(isize),
    Delete(bool),
    Copy,
    Paste,
    Header(HeaderCmd),
    Run(&'static str),
    /// Open the quick recipe picker for a layer (Gweike machines).
    Recipe(usize),
}

fn rename_id() -> Id {
    Id::unique("cuts_rename")
}

/// The panel.
pub fn show(ui: &mut Ui, app: &mut DarkBurn) {
    let t = Tokens::get();
    let mut acts: Vec<Act> = Vec::new();
    let used = app.editor.doc.used_layers();
    let current = app.editor.current_layer;
    let renaming: Option<(usize, String)> = ui.data(|d| d.get_temp(rename_id()));

    // Buttons.
    let on_list = used.contains(&current);
    let b = |ui: &mut Ui, label: &str, tip: &str, enabled: bool| -> bool {
        ui.add_enabled_ui(enabled, |ui| ui_kit::button(ui, label, ui_kit::Button::Small).backing_on_hover_text(tip).clicked()).inner
    };
    ui.horizontal(|ui| {
        if b(ui, "Up", "Output this layer earlier", on_list && used.first() != Some(&current)) {
            acts.push(Act::Move(-1));
        }
        if b(ui, "Down", "Output this layer later", on_list && used.last() != Some(&current)) {
            acts.push(Act::Move(1));
        }
        if b(ui, "Delete", "Reset this layer's settings. Shift+click: delete the layer with its shapes", true) {
            acts.push(Act::Delete(ui.input(|i| i.modifiers.shift)));
        }
    });
    ui.horizontal(|ui| {
        if b(ui, "Delete with shapes", "Delete this layer and every shape on it", on_list) {
            acts.push(Act::Delete(true));
        }
        if b(ui, "Copy", "Copy this layer's cut settings", true) {
            acts.push(Act::Copy);
        }
        if b(ui, "Paste", "Paste cut settings onto this layer", app.editor.clipboard.cut_settings.is_some()) {
            acts.push(Act::Paste);
        }
    });
    ui.add_space(t.space_6);

    // Column layout.
    let width = (ui.available_width() - 14.0).max(200.0);
    // Narrow panels shrink the fixed columns before the name column runs out.
    // The fixed columns keep their width; the panel is at least 300 points wide, so the name
    // column always has room. The three switches are narrow (their headers are short); a
    // column whose translated header is longer grows to fit it, so headers never overlap.
    let headers = [
        "#".to_string(),
        crate::i18n::tr("cuts.col.layer", "Layer"),
        crate::i18n::tr("cuts.col.mode", "Mode"),
        crate::i18n::tr("cuts.col.speed_power", "Spd/Pwr"),
        crate::i18n::tr("cuts.col.out", "Out"),
        crate::i18n::tr("cuts.col.show", "Vis"),
        crate::i18n::tr("cuts.col.air", "Air"),
    ];
    let mut fixed = [20.0f32, 0.0, 62.0, 54.0, 24.0, 24.0, 24.0];
    for (i, (w, h)) in fixed.iter_mut().zip(&headers).enumerate() {
        if i != 1 {
            let text_w = ui.painter().layout_no_wrap(h.clone(), FontId::proportional(t.font_small), t.text_dim).size().x;
            *w = w.max((text_w + 8.0).ceil());
        }
    }
    let name_w = (width - fixed.iter().sum::<f32>()).max(40.0);
    let mut widths = fixed;
    widths[1] = name_w;
    let cols = |r: Rect| -> [Rect; 7] {
        let mut x = r.left();
        let mut out = [Rect::NOTHING; 7];
        for (i, w) in widths.iter().enumerate() {
            out[i] = Rect::from_min_size(pos2(x, r.top()), vec2(*w, r.height()));
            x += w;
        }
        out
    };

    // Header.
    let (header, header_resp) = ui.allocate_exact_size(vec2(width, 22.0), Sense::click());
    let p = ui.painter();
    p.rect_filled(header, t.radius_sm, t.panel_header);
    for (c, label) in cols(header).iter().zip(headers) {
        p.text(pos2(c.left() + 4.0, c.center().y), Align2::LEFT_CENTER, label, FontId::proportional(t.font_small), t.text_dim);
    }
    let header_resp = header_resp.backing_on_hover_text("Right-click for show, output and air for every layer");
    Menu::context(&header_resp, |ui| {
        ui.set_min_width(200.0);
        for (i, (cmd, label)) in HeaderCmd::MENU.iter().enumerate() {
            if i == 4 || i == 8 {
                Menu::separator(ui);
            }
            if MenuItem::new(label, label, "").show(ui).clicked() {
                acts.push(Act::Header(*cmd));
                ui.close();
            }
        }
        Menu::separator(ui);
        for (id, label) in [
            ("layer.default_make", "Make these the default layers"),
            ("layer.default_apply", "Load the default layers"),
            ("layer.default_reset", "Reset the default layers"),
        ] {
            if MenuItem::new(label, label, "").show(ui).clicked() {
                acts.push(Act::Run(id));
                ui.close();
            }
        }
    });

    if used.is_empty() {
        ui.add_space(t.space_6);
        ui.label(RichText::new("No layers yet. Shapes you draw add their layer here.").size(t.font_small).color(t.text_dim));
    }

    // Rows.
    let list_top = ui.cursor().top();
    let mut drop_line: Option<f32> = None;
    let gweike = app.machine.active().kind.is_gweike();
    for (row_i, &l) in used.iter().enumerate() {
        let (r, resp) = ui.allocate_exact_size(vec2(width, ROW_HEIGHT), Sense::click_and_drag());
        let layer = app.editor.doc.layer(l).clone();
        let c = cols(r);
        let selected = l == current;
        let p = ui.painter().clone();
        if selected {
            p.rect_filled(r, t.radius_sm, t.accent_soft);
        } else if resp.hovered() {
            p.rect_filled(r, t.radius_sm, t.hover);
        }
        p.hline(r.x_range(), r.bottom() - 0.5, Stroke::new(1.0, t.border.gamma_multiply(0.5)));
        let ink = if selected { t.selected_ink } else { t.text };
        let font = FontId::proportional(t.font_label);
        p.text(pos2(c[0].left() + 4.0, r.center().y), Align2::LEFT_CENTER, layer_label(l), FontId::monospace(t.font_small), t.text_dim);
        let sw = Rect::from_center_size(pos2(c[1].left() + 9.0, r.center().y), vec2(13.0, 13.0));
        p.rect(sw, 2.0, if is_tool_layer(l) { t.control_fill } else { layer_colour(l) }, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
        let name_rect = Rect::from_min_max(pos2(sw.right() + 5.0, r.top() + 2.0), pos2(c[1].right() - 2.0, r.bottom() - 2.0));
        match &renaming {
            Some((rl, text)) if *rl == l => {
                let mut buf = text.clone();
                let edit = ui.put(name_rect, egui::TextEdit::singleline(&mut buf).id(Id::unique(("cuts_rename_edit", l))).font(font.clone()));
                crate::a11y::text(ui, &edit, "Layer name");
                if !edit.has_focus() && !edit.lost_focus() {
                    edit.request_focus();
                }
                if edit.lost_focus() {
                    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        ui.data_mut(|d| d.remove_temp::<(usize, String)>(rename_id()));
                    } else {
                        acts.push(Act::Rename(l, buf));
                    }
                } else {
                    ui.data_mut(|d| d.insert_temp(rename_id(), (l, buf)));
                }
            }
            _ => {
                p.with_clip_rect(name_rect).text(pos2(name_rect.left(), r.center().y), Align2::LEFT_CENTER, &layer.name, font.clone(), ink);
            }
        }
        // Mode: a drop-down (it ignores the mouse wheel), or a label for sub-layers and tools.
        let modes = modes_for(&app.editor.doc, l);
        if layer.is_tool() || layer.is_multi() || modes.len() < 2 {
            p.text(pos2(c[2].left() + 4.0, r.center().y), Align2::LEFT_CENTER, layer.mode_label(), font.clone(), ink);
        } else {
            let labels: Vec<&str> = modes.iter().map(|m| m.label()).collect();
            let mut idx = modes.iter().position(|m| *m == layer.main().mode).unwrap_or(0);
            let before = idx;
            let cell = c[2].shrink2(vec2(1.0, 1.0));
            ui.scope_builder(egui::UiBuilder::new().max_rect(cell).id_salt(("cuts_mode", l)), |ui| {
                ui.set_width(cell.width());
                ui_kit::dropdown(ui, &format!("cuts_mode_{l}"), &mut idx, &labels);
            });
            if idx != before
                && let Some(m) = modes.get(idx)
            {
                acts.push(Act::Mode(l, *m));
            }
        }
        if !layer.is_tool() {
            let opts = crate::options::get();
            let sp = format!("{:.0}/{:.0}", opts.speed_value(layer.main().speed), layer.main().max_power);
            p.text(pos2(c[3].left() + 4.0, r.center().y), Align2::LEFT_CENTER, sp, FontId::monospace(t.font_small), ink);
        }
        for (col, which, on, tip) in
            [(4, Switch::Output, layer.outputs(), "Output"), (5, Switch::Show, layer.show, "Show"), (6, Switch::Air, layer.air, "Air assist")]
        {
            if which == Switch::Output && layer.is_tool() {
                continue;
            }
            let bx = ui_kit::box_at(ui, pos2(c[col].left() + 10.0, r.center().y));
            let mut v = on;
            if ui_kit::checkbox_at(ui, bx, &format!("{tip} for layer {}", layer_label(l)), &mut v).changed() {
                acts.push(Act::Switch(l, which));
            }
        }
        // Row clicks.
        if resp.double_clicked() {
            let on_name = resp.interact_pointer_pos().is_some_and(|p| name_rect.contains(p));
            acts.push(if on_name { Act::StartRename(l) } else { Act::Open(l) });
        } else if resp.clicked() {
            let on_swatch = resp.interact_pointer_pos().is_some_and(|p| sw.expand(3.0).contains(p));
            acts.push(if gweike && on_swatch && !layer.is_tool() { Act::Recipe(l) } else { Act::Select(l) });
        }
        if gweike && resp.hover_pos().is_some_and(|p| sw.expand(3.0).contains(p)) {
            app.hint = Some("Click the colour: pick a Gweike recipe for this layer.".into());
        } else if resp.hovered() {
            app.hint = Some("Click: make current. Double-click: cut settings (on the name: rename). Drag: change the output order.".into());
        }
        // Drag to reorder.
        if resp.dragged() || resp.drag_stopped() {
            if let Some(pos) = ui.ctx().pointer_interact_pos() {
                let target = (((pos.y - list_top) / ROW_HEIGHT).round().max(0.0) as usize).min(used.len());
                drop_line = Some(list_top + target as f32 * ROW_HEIGHT);
                if resp.drag_stopped() {
                    acts.push(Act::Reorder(l, target));
                }
            }
            let _ = row_i;
        }
    }
    if let Some(y) = drop_line {
        ui.painter().hline((ui.max_rect().left())..=(ui.max_rect().left() + width), y, Stroke::new(2.0, t.accent));
    }

    if gweike && let Some(m) = &app.machine.ext.gweike_recipes.pick_message {
        ui_kit::status_line(ui, m, ui_kit::StatusTone::Dim);
    }
    ui.add_space(t.space_12);
    quick_settings(ui, app);

    // Apply what was done.
    let ctx = ui.ctx().clone();
    let mut run_later: Vec<&'static str> = Vec::new();
    for act in acts {
        let e = &mut app.editor;
        match act {
            Act::Select(l) => e.current_layer = l,
            Act::Open(l) => {
                e.current_layer = l;
                app.cut_settings_open = Some(l);
                ui_kit::window_host::raise(&ctx, crate::cut_settings::WINDOW_KEY);
            }
            Act::Recipe(l) => {
                e.current_layer = l;
                crate::recipe_picker::open(app, l);
            }
            Act::StartRename(l) => {
                e.current_layer = l;
                let name = e.doc.layer(l).name.clone();
                ctx.data_mut(|d| d.insert_temp(rename_id(), (l, name)));
            }
            Act::Rename(l, name) => {
                ctx.data_mut(|d| d.remove_temp::<(usize, String)>(rename_id()));
                e.edit("Rename layer", |d, _| rename(d, l, &name));
            }
            Act::Mode(l, m) => {
                e.current_layer = l;
                e.edit("Layer mode", |d, _| set_mode(d, l, m));
            }
            Act::Switch(l, which) => {
                let label = match which {
                    Switch::Output => "Layer output",
                    Switch::Show => "Show layer",
                    Switch::Air => "Air assist",
                };
                e.edit(label, |d, sel| {
                    let changed = toggle(d, l, which);
                    sel.retain_selectable(d);
                    changed
                });
            }
            Act::Reorder(l, to) => {
                e.edit("Layer order", |d, _| reorder(d, l, to));
            }
            Act::Move(delta) => {
                let l = e.current_layer;
                e.edit("Layer order", |d, _| d.move_layer(l, delta));
            }
            Act::Delete(with_shapes) => {
                let l = e.current_layer;
                e.edit(if with_shapes { "Delete layer and shapes" } else { "Reset layer" }, |d, _| {
                    let before = (d.len(), d.layer(l).clone());
                    d.delete_layer(l, with_shapes);
                    if with_shapes {
                        d.prune_images();
                    }
                    (d.len(), d.layer(l).clone()) != before
                });
            }
            Act::Copy => {
                let l = e.current_layer;
                e.clipboard.cut_settings = Some(e.doc.layer(l).settings.clone());
            }
            Act::Paste => {
                let l = e.current_layer;
                if let Some(s) = e.clipboard.cut_settings.clone() {
                    e.edit("Paste cut settings", |d, _| paste_settings(d, l, &s));
                }
            }
            Act::Header(cmd) => {
                e.edit("Layers", |d, sel| {
                    let changed = header_command(d, cmd);
                    sel.retain_selectable(d);
                    changed
                });
            }
            Act::Run(id) => run_later.push(id),
        }
    }
    for id in run_later {
        app.run(&ctx, id);
    }
}

/// The quick settings box: the current layer's main settings.
fn quick_settings(ui: &mut Ui, app: &mut DarkBurn) {
    let t = Tokens::get();
    let l = app.editor.current_layer;
    let layer = app.editor.doc.layer(l).clone();
    ui_kit::section_heading(ui, &format!("Layer {} settings", layer_label(l)));
    if layer.is_tool() {
        ui.label(RichText::new("Tool layers are never sent to the laser.").size(t.font_small).color(t.text_dim));
        return;
    }
    if layer.is_multi() {
        ui.label(RichText::new("This layer has sub-layers; these are the first one's settings.").size(t.font_small).color(t.text_dim));
    }
    let s = layer.main();
    let dual = layer.settings.iter().any(|s| s.laser != LaserSelect::Laser1);
    type Get = fn(&CutSetting) -> f64;
    type Set = fn(&mut CutSetting, f64);
    let (speed_k, speed_suffix) = if crate::options::get().speed_per_min { (60.0, " mm/min") } else { (1.0, " mm/s") };
    let mut fields: Vec<(&str, &str, Num, Get, Set)> = vec![
        (
            "Speed",
            "speed",
            Num::new(0.01..=100_000.0 * speed_k, 1, speed_suffix).speed(1.0),
            |s| crate::options::get().speed_value(s.speed),
            |s, v| s.speed = crate::options::get().speed_to_mm_s(v),
        ),
        ("Max power", "max", Num::new(0.0..=100.0, 1, " %").speed(0.2), |s| s.max_power, |s, v| s.max_power = v),
        ("Min power", "min", Num::new(0.0..=100.0, 1, " %").speed(0.2), |s| s.min_power, |s, v| s.min_power = v),
    ];
    if dual {
        fields.push(("Laser 2 max", "max2", Num::new(0.0..=100.0, 1, " %").speed(0.2), |s| s.max_power_2, |s, v| s.max_power_2 = v));
        fields.push(("Laser 2 min", "min2", Num::new(0.0..=100.0, 1, " %").speed(0.2), |s| s.min_power_2, |s, v| s.min_power_2 = v));
    }
    fields.push(("Interval", "interval", Num::new(0.001..=100.0, 3, " mm").speed(0.001), |s| s.interval, |s, v| s.interval = v));
    fields.push((
        "Passes",
        "passes",
        Num::new(1.0..=10_000.0, 0, ""),
        |s| s.passes as f64,
        |s, v| s.passes = numeric::whole(v).clamp(1, 10_000) as u32,
    ));
    let ctx = ui.ctx().clone();
    for (label, key, num, get, set) in fields {
        let mut v = get(s);
        let ed = ui.push_id(("quick", key), |ui| numeric::row(ui, label, &mut v, &num)).inner;
        if ed.changed {
            let merge = ed.continues();
            numeric::commit(&mut app.editor, &ctx, Id::unique(("quick", key, l)), label, merge, |d, _| {
                let s = d.layer_mut(l).main_mut();
                let before = get(s);
                set(s, v);
                s.sanitise();
                get(s) != before
            });
        }
    }
    if s.very_high_dpi() {
        ui_kit::status_line(ui, "The interval is finer than 1000 lines per inch. Check it is not a mistake.", ui_kit::StatusTone::Warning);
    }
    ui.horizontal(|ui| {
        if ui_kit::button(ui, "Edit cut settings", ui_kit::Button::Small).clicked() {
            app.cut_settings_open = Some(l);
            ui_kit::window_host::raise(ui.ctx(), crate::cut_settings::WINDOW_KEY);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use darkburn_core::kurbo::Rect as KRect;
    use darkburn_core::{ImageShape, Shape};

    fn doc() -> Document {
        let mut d = Document::new();
        for l in [4, 1, 9] {
            d.add(Shape::rect(KRect::new(0.0, 0.0, 10.0, 10.0), 0.0, l));
        }
        d
    }

    #[test]
    fn header_menu_commands() {
        let mut d = doc();
        assert!(header_command(&mut d, HeaderCmd::HideAll));
        assert!(d.used_layers().iter().all(|l| !d.layer(*l).show));
        assert!(!header_command(&mut d, HeaderCmd::HideAll), "nothing left to change");
        assert!(header_command(&mut d, HeaderCmd::ShowAll));
        assert!(header_command(&mut d, HeaderCmd::DisableAll));
        assert!(!d.layer(4).output);
        assert!(header_command(&mut d, HeaderCmd::InvertOutput));
        assert!(d.layer(4).output);
        assert!(header_command(&mut d, HeaderCmd::AirOff));
        assert!(!d.layer(9).air);
        assert!(header_command(&mut d, HeaderCmd::AirInvert));
        assert!(d.layer(9).air);
        assert!(!header_command(&mut d, HeaderCmd::AirOn));
        assert!(!d.layer(20).output || d.layer(20).output, "unused layers are left alone");
        d.layer_mut(9).main_mut().mode = Mode::Fill;
        assert!(header_command(&mut d, HeaderCmd::SortCutsLast));
        assert_eq!(d.used_layers(), vec![9, 1, 4]);
    }

    #[test]
    fn tool_layers_never_output() {
        let mut d = doc();
        d.add(Shape::rect(KRect::new(0.0, 0.0, 1.0, 1.0), 0.0, darkburn_core::T1));
        assert!(!toggle(&mut d, darkburn_core::T1, Switch::Output));
        header_command(&mut d, HeaderCmd::EnableAll);
        assert!(!d.layer(darkburn_core::T1).outputs());
        assert!(toggle(&mut d, 1, Switch::Output));
        assert!(!d.layer(1).output);
        assert!(toggle(&mut d, 1, Switch::Show));
        assert!(!d.layer(1).show);
    }

    #[test]
    fn modes_follow_what_is_on_the_layer() {
        let mut d = doc();
        assert_eq!(modes_for(&d, 1), vec![Mode::Line, Mode::Fill, Mode::FillLine, Mode::OffsetFill]);
        let img = Shape::new(ShapeKind::Image(ImageShape::default()), darkburn_core::kurbo::Affine::IDENTITY, 7);
        d.add(img.clone());
        assert_eq!(modes_for(&d, 7), vec![Mode::Image]);
        let mut both = img;
        both.layer = 1;
        d.add(both);
        assert_eq!(modes_for(&d, 1).len(), 5);
        assert!(set_mode(&mut d, 4, Mode::Fill));
        assert!(!set_mode(&mut d, 4, Mode::Fill));
    }

    #[test]
    fn drag_reorders_the_output() {
        let mut d = doc();
        assert_eq!(d.used_layers(), vec![1, 4, 9]);
        assert!(reorder(&mut d, 9, 0));
        assert_eq!(d.used_layers(), vec![9, 1, 4]);
        assert!(reorder(&mut d, 9, 3));
        assert_eq!(d.used_layers(), vec![1, 4, 9]);
        assert!(!reorder(&mut d, 4, 1), "dropping a row on itself does nothing");
        assert!(!reorder(&mut d, 4, 2));
        assert!(reorder(&mut d, 1, 2));
        assert_eq!(d.used_layers(), vec![4, 1, 9]);
    }

    #[test]
    fn paste_and_rename() {
        let mut d = doc();
        d.layer_mut(1).main_mut().speed = 7.0;
        d.layer_mut(1).add_sub_layer();
        let copied = d.layer(1).settings.clone();
        assert!(paste_settings(&mut d, 4, &copied));
        assert_eq!(d.layer(4).settings.len(), 2);
        assert_eq!(d.layer(4).main().speed, 7.0);
        assert!(!paste_settings(&mut d, 4, &copied));
        assert!(!paste_settings(&mut d, 4, &[]));
        assert!(rename(&mut d, 4, " Engrave "));
        assert_eq!(d.layer(4).name, "Engrave");
        assert!(!rename(&mut d, 4, "  "));
    }

    #[test]
    fn panel_edits_are_undoable() {
        let mut app = DarkBurn::new(crate::prefs::Prefs::default());
        app.editor = darkburn_core::Editor::with_document(doc());
        app.editor.edit("Mode", |d, _| set_mode(d, 9, Mode::Image));
        app.editor.edit("Layers", |d, _| header_command(d, HeaderCmd::HideAll));
        assert!(app.editor.undo());
        assert!(app.editor.doc.layer(1).show);
        assert!(app.editor.undo());
        assert_eq!(app.editor.doc.layer(9).main().mode, Mode::Line);
    }
}
