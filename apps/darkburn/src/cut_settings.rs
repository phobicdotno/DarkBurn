//! The Cut Settings Editor window: every setting of a layer and its sub-layers, by mode, on a
//! Common and an Advanced tab. Layers picked together in the list on the left all take each
//! change. Edits show live; OK keeps them as one undo step, Cancel puts everything back.

use darkburn_core::layer::{AirMode, FillGrouping, ImageMode, LaserSelect, Layer, LeadStyle, Mode, is_tool_layer, layer_label};
use darkburn_core::{CutSetting, MAX_SUB_LAYERS};
use darkburn_ui::ui_kit;
use egui::{Align2, Color32, FontId, Id, Pos2, Rect, RichText, Sense, Stroke, Ui, pos2, vec2};
use solidcraft_theme::Tokens;

use crate::numeric::{self, Num};
use crate::palette::layer_colour;
use crate::shell::{APP_BAR_HEIGHT, DarkBurn};

/// The device has a Z axis (otherwise the Z settings are greyed out).
const DEVICE_HAS_Z: bool = false;

/// The editor window's key, where the user put it (docked, a window of its own...).
pub const WINDOW_KEY: &str = "cut_settings_window";

/// One open editing session.
#[derive(Clone, Debug, Default)]
struct Session {
    /// The layer `cut_settings_open` named last frame.
    opened_for: usize,
    /// Every layer as it was when the editor opened (Cancel puts these back).
    original: Vec<Layer>,
    /// The layer whose settings are shown.
    primary: usize,
    /// Layers that take each change (the primary one included).
    selected: Vec<usize>,
    /// The sub-layer shown.
    sub: usize,
    advanced: bool,
    /// The project had no unsaved changes when the editor opened: the number of undo steps then.
    clean_at: Option<usize>,
    /// The project was saved while the editor's live edits were in it.
    saved_since: bool,
    /// A live edit was made.
    edited: bool,
    /// The project the editor was opened in ([`darkburn_core::Editor::project`]).
    project: u64,
}

fn session_id() -> Id {
    Id::unique("cut_settings_session")
}

/// Copy the fields that differ between `before` and `after` onto `target` (nested settings
/// field by field), so several layers can take one change without losing their other values.
pub fn apply_change(before: &CutSetting, after: &CutSetting, target: &mut CutSetting) {
    use serde_json::Value;
    fn merge(before: &Value, after: &Value, target: &mut Value) {
        match (before, after, target) {
            (Value::Object(b), Value::Object(a), Value::Object(t)) => {
                for (k, av) in a {
                    let bv = b.get(k);
                    if bv == Some(av) {
                        continue;
                    }
                    match (bv, t.get_mut(k)) {
                        (Some(bv), Some(tv)) if bv.is_object() && av.is_object() && tv.is_object() => merge(bv, av, tv),
                        (_, Some(tv)) => *tv = av.clone(),
                        (_, None) => {
                            t.insert(k.clone(), av.clone());
                        }
                    }
                }
            }
            (_, a, t) => *t = a.clone(),
        }
    }
    let (Ok(b), Ok(a), Ok(mut t)) = (serde_json::to_value(before), serde_json::to_value(after), serde_json::to_value(&*target)) else {
        return;
    };
    merge(&b, &a, &mut t);
    if let Ok(s) = serde_json::from_value(t) {
        *target = s;
    }
}

/// Settings back to their defaults, keeping the name, mode and whether the sub-layer is on.
pub fn reset_setting(s: &CutSetting) -> CutSetting {
    // Gweike values hidden on other devices are kept: a reset there must not wipe them.
    CutSetting { name: s.name.clone(), mode: s.mode, enabled: s.enabled, gweike: s.gweike.clone(), ..CutSetting::default() }
}

/// A small sample of how `mode` turns a grey ramp (dark on the left) into dots: `w` by `h`
/// cells, each 0 (no burn) to 1 (full burn).
pub fn dither_sample(mode: ImageMode, w: usize, h: usize) -> Vec<f32> {
    dither_sample_engine(mode, w, h).unwrap_or_else(|| dither_sample_basic(mode, w, h))
}

/// The same sample made by the image engine itself, so the strip shows what will really burn.
fn dither_sample_engine(mode: ImageMode, w: usize, h: usize) -> Option<Vec<f32>> {
    use darkburn_raster::{Adjustments, GrayF, process_image};
    let pitch = 0.1;
    let grey = GrayF::from_fn(w as u32, h as u32, |x, _| x as f32 / (w.max(2) - 1) as f32);
    let c = CutSetting { image_mode: mode, interval: pitch, ..CutSetting::default() };
    let to_mm = darkburn_core::kurbo::Affine::new([pitch, 0.0, 0.0, -pitch, 0.0, h as f64 * pitch]);
    let p = process_image(&grey, to_mm, &Adjustments::default(), &crate::images::image_settings(&c, 0.0));
    if (p.map.width as usize, p.map.height as usize) != (w, h) {
        return None;
    }
    Some(p.map.data.iter().map(|&v| f32::from(v) / 255.0).collect())
}

/// A rough stand-in for [`dither_sample_engine`] (used if the grid does not line up).
fn dither_sample_basic(mode: ImageMode, w: usize, h: usize) -> Vec<f32> {
    let grey = |x: usize| 1.0 - x as f32 / (w.max(2) - 1) as f32; // burn amount
    let mut out = vec![0.0f32; w * h];
    match mode {
        ImageMode::Grayscale => {
            for y in 0..h {
                for x in 0..w {
                    out[y * w + x] = grey(x);
                }
            }
        }
        ImageMode::Threshold => {
            for y in 0..h {
                for x in 0..w {
                    out[y * w + x] = if grey(x) >= 0.5 { 1.0 } else { 0.0 };
                }
            }
        }
        ImageMode::Ordered | ImageMode::Halftone | ImageMode::Newsprint => {
            const BAYER: [[f32; 4]; 4] = [[0.0, 8.0, 2.0, 10.0], [12.0, 4.0, 14.0, 6.0], [3.0, 11.0, 1.0, 9.0], [15.0, 7.0, 13.0, 5.0]];
            for y in 0..h {
                for x in 0..w {
                    let t = (BAYER[y % 4][x % 4] + 0.5) / 16.0;
                    out[y * w + x] = if grey(x) > t { 1.0 } else { 0.0 };
                }
            }
        }
        _ => {
            // Error diffusion (Floyd-Steinberg weights; good enough to show the look).
            let mut v: Vec<f32> = (0..w * h).map(|i| grey(i % w)).collect();
            for y in 0..h {
                for x in 0..w {
                    let i = y * w + x;
                    let new = if v[i] >= 0.5 { 1.0 } else { 0.0 };
                    let err = v[i] - new;
                    out[i] = new;
                    let mut spread = |dx: isize, dy: usize, f: f32| {
                        let nx = x as isize + dx;
                        if nx >= 0 && (nx as usize) < w && y + dy < h {
                            v[(y + dy) * w + nx as usize] += err * f;
                        }
                    };
                    spread(1, 0, 7.0 / 16.0);
                    spread(-1, 1, 3.0 / 16.0);
                    spread(0, 1, 5.0 / 16.0);
                    spread(1, 1, 1.0 / 16.0);
                }
            }
        }
    }
    out
}

fn line_like(m: Mode) -> bool {
    matches!(m, Mode::Line | Mode::FillLine)
}

fn fill_like(m: Mode) -> bool {
    matches!(m, Mode::Fill | Mode::FillLine | Mode::OffsetFill)
}

/// Show the editor when it is open.
pub fn dialog(ctx: &egui::Context, app: &mut DarkBurn) {
    let project = app.editor.project();
    let Some(open) = app.cut_settings_open else {
        // Closed from elsewhere: the edits stay (as if OK).
        if let Some(s) = ctx.data_mut(|d| d.remove_temp::<Session>(session_id()))
            && s.project == project
        {
            finish(app, &s, true);
        }
        return;
    };
    if ctx.data(|d| d.get_temp::<Session>(session_id())).is_some_and(|s| s.project != project) {
        // Another project was opened, or a new one made, while the editor was open. What it would
        // put back (Cancel) or keep as an undo step (OK) are the layers of the project that is
        // gone: the editor closes and leaves this one as it came.
        close(ctx, app);
        return;
    }
    let mut s = ctx.data(|d| d.get_temp::<Session>(session_id())).unwrap_or_else(|| Session {
        opened_for: open,
        original: app.editor.doc.layers.clone(),
        primary: open,
        selected: vec![open],
        sub: 0,
        advanced: false,
        clean_at: (!app.editor.is_modified()).then(|| app.editor.history.len()),
        saved_since: false,
        edited: false,
        project,
    });
    if s.edited && !app.editor.is_modified() {
        s.saved_since = true;
    }
    if s.opened_for != open {
        // Another layer was double-clicked while the editor is open.
        s.opened_for = open;
        s.primary = open;
        s.selected = vec![open];
        s.sub = 0;
    }
    let title = "Cut Settings Editor";
    // The editor opens in the middle of the room under the application bar. Where the kit puts a
    // window that is given no place, two and a half rows above the middle of the main window,
    // the top of this tall one lay over the bar and its menus in most main windows.
    let middle = crate::shell::room_under_the_bar(ctx).center();
    // Closed from its tab or its window with layers changed, it asks whether to keep them.
    let unsaved = app.editor.doc.layers != s.original;
    let (closed, action) = ui_kit::Window::new(Id::unique("cut_settings_window"), "settings", title)
        .dockable(WINDOW_KEY)
        .cards(2.7)
        .fields()
        .placed(Align2::CENTER_CENTER, middle)
        .actions("OK", "Cancel", None)
        .unsaved(unsaved)
        .show(ctx, true, |ui| body(ui, app, &mut s));
    s.edited |= app.editor.doc.layers != s.original;
    match (closed, action) {
        (_, ui_kit::Action::Ok) => {
            finish(app, &s, true);
            close(ctx, app);
        }
        (true, _) | (_, ui_kit::Action::Cancel) => {
            finish(app, &s, false);
            close(ctx, app);
        }
        _ => {
            app.cut_settings_open = Some(s.primary);
            s.opened_for = s.primary;
            ctx.data_mut(|d| d.insert_temp(session_id(), s));
        }
    }
}

fn close(ctx: &egui::Context, app: &mut DarkBurn) {
    app.cut_settings_open = None;
    ctx.data_mut(|d| d.remove_temp::<Session>(session_id()));
}

/// End a session: keep the edits as one undo step, or put the layers back.
fn finish(app: &mut DarkBurn, s: &Session, keep: bool) {
    let e = &mut app.editor;
    let edited = e.doc.layers.clone();
    // With the layers as they were at the start, a project that had no unsaved changes then has
    // none now, unless it was saved or something else was done or undone meanwhile.
    let saved_again = s.clean_at == Some(e.history.len()) && !s.saved_since && e.is_modified();
    if edited == s.original {
        if s.edited && saved_again {
            e.mark_saved();
        }
        return;
    }
    e.doc.layers = s.original.clone();
    if keep {
        e.edit("Cut settings", |d, _| {
            d.layers = edited;
            true
        });
    } else {
        e.touch();
        if saved_again {
            e.mark_saved();
        }
    }
}

/// The layers listed on the left: those in use, and the one being edited.
fn listed_layers(app: &DarkBurn, s: &Session) -> Vec<usize> {
    let mut v = app.editor.doc.used_layers();
    if !v.contains(&s.primary) {
        v.push(s.primary);
    }
    v
}

/// The tallest the editor's two columns are, in points. A small main window gives them less.
const BODY_HEIGHT: f32 = 720.0;
/// The fields keep this much height however little room there is; the window scrolls then.
const FIELDS_MIN_HEIGHT: f32 = 160.0;

fn body(ui: &mut Ui, app: &mut DarkBurn, s: &mut Session) {
    let t = Tokens::get();
    let list_w = 130.0;
    // The columns have a height of their own. The rule between them takes all the height it is
    // offered: offered all a window may have, it made the editor as tall as the main window,
    // whatever was in it. In a small main window they leave the height of the application bar
    // unused, so the editor fits under the bar.
    let height = (ui.available_height() - APP_BAR_HEIGHT).clamp(0.0, BODY_HEIGHT);
    ui.allocate_ui_with_layout(vec2(ui.available_width(), height), egui::Layout::left_to_right(egui::Align::Min), |ui| {
        // Layer list.
        ui.allocate_ui_with_layout(vec2(list_w, ui.available_height().max(300.0)), egui::Layout::top_down(egui::Align::Min), |ui| {
            ui.set_width(list_w);
            ui.label(RichText::new("Layers").size(t.font_small).color(t.text_dim));
            for l in listed_layers(app, s) {
                let (r, resp) = ui.allocate_exact_size(vec2(list_w, 24.0), Sense::click());
                let on = s.selected.contains(&l);
                if l == s.primary {
                    ui.painter().rect_filled(r, t.radius_sm, t.accent_soft);
                } else if on {
                    ui.painter().rect_filled(r, t.radius_sm, t.selected_bg.gamma_multiply(0.6));
                } else if resp.hovered() {
                    ui.painter().rect_filled(r, t.radius_sm, t.hover);
                }
                let sw = Rect::from_center_size(pos2(r.left() + 10.0, r.center().y), vec2(12.0, 12.0));
                ui.painter().rect(
                    sw,
                    2.0,
                    if is_tool_layer(l) { t.control_fill } else { layer_colour(l) },
                    Stroke::new(1.0, t.border),
                    egui::StrokeKind::Inside,
                );
                let layer = app.editor.doc.layer(l);
                ui.painter().with_clip_rect(r).text(
                    pos2(sw.right() + 6.0, r.center().y),
                    Align2::LEFT_CENTER,
                    format!("{} {}", layer_label(l), layer.name),
                    FontId::proportional(t.font_label),
                    if l == s.primary { t.selected_ink } else { t.text },
                );
                if resp.clicked() {
                    let add = ui.input(|i| i.modifiers.command || i.modifiers.shift);
                    if add && l != s.primary {
                        if on {
                            s.selected.retain(|x| *x != l);
                        } else {
                            s.selected.push(l);
                        }
                    } else {
                        s.primary = l;
                        s.selected = vec![l];
                        s.sub = 0;
                    }
                }
            }
            ui.add_space(t.space_6);
            ui.add(egui::Label::new(RichText::new("Ctrl+click to change several layers at once.").size(t.font_micro).color(t.text_dim)).wrap());
        });
        ui.separator();
        ui.vertical(|ui| editor_side(ui, app, s));
    });
}

fn editor_side(ui: &mut Ui, app: &mut DarkBurn, s: &mut Session) {
    let t = Tokens::get();
    let l = s.primary;
    let layer = app.editor.doc.layer(l).clone();
    s.sub = s.sub.min(layer.settings.len() - 1);
    if s.selected.len() > 1 {
        let names: Vec<String> = s.selected.iter().map(|l| layer_label(*l)).collect();
        ui_kit::status_line(ui, &format!("Changes apply to layers {}.", names.join(", ")), ui_kit::StatusTone::Dim);
    }
    if let Some(link) = &layer.library_link {
        ui_kit::status_line(ui, &format!("Linked to the material library entry \"{link}\"."), ui_kit::StatusTone::Dim);
    }
    if layer.is_tool() {
        ui_kit::status_line(ui, "Tool layers are never sent to the laser.", ui_kit::StatusTone::Dim);
    }

    // Sub-layers.
    let mut sub_op: Option<SubOp> = None;
    ui.horizontal_wrapped(|ui| {
        for (i, c) in layer.settings.iter().enumerate() {
            let name = if c.name.is_empty() { if i == 0 { "Main".to_string() } else { format!("Sub {i}") } } else { c.name.clone() };
            let label = if c.enabled { name } else { format!("{name} (off)") };
            if ui_kit::choice_chip(ui, &format!("cse_sub_{i}"), &label, i == s.sub).clicked() {
                s.sub = i;
            }
        }
    });
    ui.horizontal_wrapped(|ui| {
        let n = layer.settings.len();
        let mut b = |ui: &mut Ui, label: &str, enabled: bool, op: SubOp| {
            if ui.add_enabled_ui(enabled, |ui| ui_kit::button(ui, label, ui_kit::Button::Small)).inner.clicked() {
                sub_op = Some(op);
            }
        };
        b(ui, "Add sub-layer", n < MAX_SUB_LAYERS, SubOp::Add);
        b(ui, "Duplicate", n < MAX_SUB_LAYERS, SubOp::Duplicate);
        b(ui, "Remove", n > 1, SubOp::Remove);
        b(ui, "Move left", s.sub > 0, SubOp::Move(-1));
        b(ui, "Move right", s.sub + 1 < n, SubOp::Move(1));
    });
    if let Some(op) = sub_op {
        let lay = app.editor.doc.layer_mut(l);
        let done = match op {
            SubOp::Add => lay.add_sub_layer(),
            SubOp::Duplicate => lay.duplicate_sub_layer(s.sub),
            SubOp::Remove => lay.remove_sub_layer(s.sub),
            SubOp::Move(d) => lay.move_sub_layer(s.sub, d),
        };
        if done {
            s.sub = match op {
                SubOp::Add => lay.settings.len() - 1,
                SubOp::Duplicate => s.sub + 1,
                SubOp::Remove => s.sub.min(lay.settings.len() - 1),
                SubOp::Move(d) => (s.sub as isize + d) as usize,
            };
            app.editor.touch();
        }
        return;
    }

    // Tabs and beginner mode.
    ui.horizontal(|ui| {
        if !app.prefs.cut_beginner {
            let mut tab = usize::from(s.advanced);
            ui.allocate_ui(vec2(220.0, t.control_height), |ui| {
                ui_kit::segmented(ui, &mut tab, &["Common", "Advanced"], ui_kit::Segments { small: true, ..Default::default() });
            });
            s.advanced = tab == 1;
        }
        ui_kit::checkbox(ui, "Beginner mode", &mut app.prefs.cut_beginner);
        if ui_kit::button(ui, "Reset to default", ui_kit::Button::Small).clicked() {
            let cur = app.editor.doc.layer(l).settings[s.sub].clone();
            let reset = reset_setting(&cur);
            apply_all(app, s, &cur, &reset);
        }
    });
    if app.prefs.cut_beginner {
        s.advanced = false;
    }

    let before = app.editor.doc.layer(l).settings[s.sub].clone();
    // The galvo-only fields show on the Advanced tab when the selected device is a galvo.
    let galvo_device = app.machine.active().family() == crate::machine_api::Family::Galvo;
    // Machine-specific sections (Gweike fibre cutting) follow the active device's caps.
    let caps = app.machine.active().kind.caps();
    let mut c = before.clone();
    let (mut output, mut air, mut name) = (layer.output, layer.air, layer.name.clone());
    let modes = crate::cuts_panel::modes_for(&app.editor.doc, l);
    // The fields go down to the bottom of the columns, less the row of the button under them.
    let under = if c.mode == Mode::Image && s.advanced { t.control_height + t.form_gap_y } else { 0.0 };
    let room = (ui.available_height() - under).max(FIELDS_MIN_HEIGHT);
    egui::ScrollArea::vertical().id_salt("cse_fields").max_height(room).auto_shrink([false, true]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = t.form_gap_y;
        if s.advanced {
            advanced(ui, &mut c);
        } else {
            common(ui, &mut c, &modes, &layer, &mut output, &mut air, &mut name, (l, s.sub));
        }
        if galvo_device && s.advanced {
            galvo(ui, &mut c);
        }
        if s.advanced {
            machine_sections(ui, &caps, &mut c);
        }
    });
    if c != before {
        apply_all(app, s, &before, &c);
    }
    if c.mode == Mode::Image && s.advanced && ui_kit::button(ui, "Scanning offset and fast skip...", ui_kit::Button::Small).clicked() {
        app.images.offsets_open = true;
    }
    let lay = app.editor.doc.layer(l);
    // An emptied name field is a name half typed, not a change.
    let renamed = !name.trim().is_empty() && name.trim() != lay.name;
    if (output, air) != (lay.output, lay.air) || renamed {
        for &other in &s.selected {
            let lay = app.editor.doc.layer_mut(other);
            if !lay.is_tool() {
                lay.output = output;
            }
            lay.air = air;
        }
        if renamed {
            app.editor.doc.layer_mut(l).name = name.trim().to_string();
        }
        app.editor.touch();
    }
}

#[derive(Clone, Copy)]
enum SubOp {
    Add,
    Duplicate,
    Remove,
    Move(isize),
}

/// Apply the change from `before` to `after` to the shown sub-layer of every selected layer.
fn apply_all(app: &mut DarkBurn, s: &Session, before: &CutSetting, after: &CutSetting) {
    for &l in &s.selected {
        let lay = app.editor.doc.layer_mut(l);
        let Some(target) = lay.settings.get_mut(s.sub) else { continue };
        if l == s.primary {
            *target = after.clone();
        } else {
            apply_change(before, after, target);
        }
        target.sanitise();
    }
    app.editor.touch();
}

fn num(ui: &mut Ui, label: &str, v: &mut f64, n: Num) -> bool {
    ui.push_id(label, |ui| numeric::row(ui, label, v, &n)).inner.changed
}

fn count(ui: &mut Ui, label: &str, v: &mut u32, lo: u32, hi: u32) {
    let mut f = *v as f64;
    if num(ui, label, &mut f, Num::new(lo as f64..=hi as f64, 0, "")) {
        *v = numeric::whole(f).clamp(lo as i64, hi as i64) as u32;
    }
}

fn pick<T: PartialEq + Copy>(ui: &mut Ui, label: &str, v: &mut T, options: &[(T, &str)]) {
    let mut i = options.iter().position(|(o, _)| o == v).unwrap_or(0);
    let labels: Vec<&str> = options.iter().map(|(_, l)| *l).collect();
    ui_kit::choice_row(ui, label, &format!("cse_{label}"), &mut i, &labels);
    if let Some((o, _)) = options.get(i) {
        *v = *o;
    }
}

fn pct() -> Num {
    Num::new(0.0..=100.0, 1, " %").speed(0.2)
}

fn mm(lo: f64, hi: f64) -> Num {
    Num::new(lo..=hi, 2, " mm").speed(0.01)
}

fn interval_rows(ui: &mut Ui, c: &mut CutSetting, dpi_label: &str) {
    num(ui, "Interval", &mut c.interval, Num::new(0.001..=100.0, 4, " mm").speed(0.001));
    let mut dpi = c.dpi();
    if num(ui, dpi_label, &mut dpi, Num::new(0.254..=25_400.0, 1, "").speed(1.0)) {
        c.set_dpi(dpi);
    }
    if c.very_high_dpi() {
        ui_kit::status_line(ui, "Over 1000 lines per inch: the burn will be very slow and dark. Check this is meant.", ui_kit::StatusTone::Warning);
    }
}

fn overscan_rows(ui: &mut Ui, c: &mut CutSetting) {
    let suffix = if c.overscan_percent { " %" } else { " mm" };
    num(ui, "Overscan", &mut c.overscan, Num::new(0.0..=1000.0, 2, suffix).speed(0.05));
    ui_kit::caption(ui, "Overscan", &format!("{:.2} mm past each end at this speed", c.overscan_mm()));
}

fn scan_angle_rows(ui: &mut Ui, c: &mut CutSetting) {
    num(ui, "Scan angle", &mut c.scan_angle, Num::new(-360.0..=360.0, 1, " deg").speed(0.5));
}

/// The Common tab.
#[allow(clippy::too_many_arguments)]
fn common(
    ui: &mut Ui,
    c: &mut CutSetting,
    modes: &[Mode],
    layer: &Layer,
    output: &mut bool,
    air: &mut bool,
    name: &mut String,
    (l, sub): (usize, usize),
) {
    let t = Tokens::get();
    ui_kit::section_heading(ui, "Layer");
    ui_kit::row(ui, "Layer name", |ui| {
        // The layer takes the name without the spaces round it. While the field has the keyboard
        // it shows what was typed, not the layer's name read back: else a space would vanish as
        // soon as it is typed, and the field could never be emptied to type another name.
        let typing = Id::unique("cse_layer_name_typing");
        let typed = ui.data(|d| d.get_temp::<(usize, String)>(typing)).filter(|(of, _)| *of == l);
        let mut text = typed.map_or_else(|| name.clone(), |(_, text)| text);
        let r = ui.add_sized(vec2(ui.available_width(), t.control_height), egui::TextEdit::singleline(&mut text));
        crate::a11y::text(ui, &r, "Layer name");
        if r.has_focus() {
            ui.data_mut(|d| d.insert_temp(typing, (l, text.clone())));
        } else {
            ui.data_mut(|d| d.remove_temp::<(usize, String)>(typing));
        }
        if r.changed() {
            *name = text;
        }
    });
    if sub > 0 || layer.is_multi() {
        ui_kit::row(ui, "Sub-layer name", |ui| {
            let r = ui.add_sized(vec2(ui.available_width(), t.control_height), egui::TextEdit::singleline(&mut c.name));
            crate::a11y::text(ui, &r, "Sub-layer name");
        });
        ui_kit::checkbox(ui, "Sub-layer on", &mut c.enabled);
    }
    let mode_opts: Vec<(Mode, &str)> = modes.iter().map(|m| (*m, m.label())).collect();
    pick(ui, "Mode", &mut c.mode, &mode_opts);
    ui.horizontal(|ui| {
        ui.add_enabled_ui(!layer.is_tool(), |ui| ui_kit::checkbox(ui, "Output", output));
        ui_kit::checkbox(ui, "Air assist", air);
    });

    ui_kit::section_heading(ui, "Speed and power");
    let o = crate::options::get();
    let mut shown = o.speed_value(c.speed);
    let (k, suffix) = if o.speed_per_min { (60.0, " mm/min") } else { (1.0, " mm/s") };
    if num(ui, "Speed", &mut shown, Num::new(0.01..=100_000.0 * k, 1, suffix).speed(1.0)) {
        c.speed = o.speed_to_mm_s(shown);
    }
    pick(ui, "Laser", &mut c.laser, &[(LaserSelect::Laser1, "Laser 1"), (LaserSelect::Laser2, "Laser 2"), (LaserSelect::Both, "Both")]);
    if c.laser != LaserSelect::Laser2 {
        num(ui, "Max power", &mut c.max_power, pct());
        num(ui, "Min power", &mut c.min_power, pct());
    }
    if c.laser != LaserSelect::Laser1 {
        num(ui, "Laser 2 max power", &mut c.max_power_2, pct());
        num(ui, "Laser 2 min power", &mut c.min_power_2, pct());
    }
    count(ui, "Passes", &mut c.passes, 1, 10_000);

    if line_like(c.mode) {
        ui_kit::section_heading(ui, "Line");
        num(ui, "Kerf offset", &mut c.kerf, mm(-10.0, 10.0));
    }
    if fill_like(c.mode) {
        ui_kit::section_heading(ui, if c.mode == Mode::OffsetFill { "Offset fill" } else { "Fill" });
        interval_rows(ui, c, "Lines per inch");
        if c.mode != Mode::OffsetFill {
            scan_angle_rows(ui, c);
            ui_kit::checkbox(ui, "Bi-directional", &mut c.bidirectional);
            ui_kit::checkbox(ui, "Cross-hatch", &mut c.cross_hatch);
            overscan_rows(ui, c);
        } else {
            ui_kit::caption(ui, "Interval", "Offset fill spirals in from the outline; shapes' power scale is respected.");
        }
    }
    if c.mode == Mode::Image {
        ui_kit::section_heading(ui, "Image");
        let opts: Vec<(ImageMode, &str)> = ImageMode::ALL.iter().map(|m| (*m, m.label())).collect();
        pick(ui, "Image mode", &mut c.image_mode, &opts);
        dither_preview(ui, c.image_mode);
        interval_rows(ui, c, "DPI");
        scan_angle_rows(ui, c);
        ui_kit::checkbox(ui, "Bi-directional", &mut c.bidirectional);
        overscan_rows(ui, c);
        ui_kit::checkbox(ui, "Negative image", &mut c.negative);
        ui_kit::checkbox(ui, "Pass-through (already dithered)", &mut c.pass_through);
        if matches!(c.image_mode, ImageMode::Halftone | ImageMode::Newsprint) {
            num(ui, "Halftone cells per inch", &mut c.halftone_cells, Num::new(1.0..=1000.0, 1, "").speed(0.5));
            num(ui, "Halftone angle", &mut c.halftone_angle, Num::new(-360.0..=360.0, 1, " deg").speed(0.5));
        }
        if c.image_mode == ImageMode::Threshold {
            num(ui, "Ramp length", &mut c.threshold_ramp, mm(0.0, 100.0));
        }
        if c.image_mode == ImageMode::Grayscale {
            ui_kit::caption(ui, "Image mode", "Min power burns the lightest grey, max power the darkest.");
        }
    }
    z_rows(ui, c);
    ui.add_space(t.space_6);
    scan_picture(ui, c);
}

fn z_rows(ui: &mut Ui, c: &mut CutSetting) {
    ui.add_enabled_ui(DEVICE_HAS_Z, |ui| {
        num(ui, "Z offset", &mut c.z_offset, mm(-100.0, 100.0));
        num(ui, "Z step per pass", &mut c.z_step, mm(-100.0, 100.0));
    });
    if !DEVICE_HAS_Z {
        ui_kit::caption(ui, "Z offset", "This device has no Z axis.");
    }
}

/// The Advanced tab.
fn advanced(ui: &mut Ui, c: &mut CutSetting) {
    ui_kit::section_heading(ui, "Shared");
    pick(ui, "Air assist mode", &mut c.air_mode, &[(AirMode::ByLayer, "By layer"), (AirMode::WithLaser, "With the laser")]);
    ui_kit::checkbox(ui, "Constant power mode (M3)", &mut c.constant_power);
    if line_like(c.mode) {
        ui_kit::section_heading(ui, "Perforation");
        ui_kit::checkbox(ui, "Perforation mode", &mut c.perforation);
        if c.perforation {
            num(ui, "Cut length", &mut c.perf_cut, mm(0.01, 1000.0));
            num(ui, "Skip length", &mut c.perf_skip, mm(0.01, 1000.0));
        }
        ui_kit::section_heading(ui, "Tabs");
        ui_kit::checkbox(ui, "Use tabs", &mut c.tabs.enabled);
        if c.tabs.enabled {
            ui_kit::checkbox(ui, "Space tabs evenly", &mut c.tabs.auto_spacing);
            if c.tabs.auto_spacing {
                num(ui, "Tab spacing", &mut c.tabs.spacing, mm(0.1, 10_000.0));
                count(ui, "Min tabs", &mut c.tabs.min_count, 0, 1000);
                count(ui, "Max tabs", &mut c.tabs.max_count, 0, 1000);
            }
            num(ui, "Tab size", &mut c.tabs.size, mm(0.0, 100.0));
            ui_kit::checkbox(ui, "Cut tabs partly", &mut c.tabs.partial);
            if c.tabs.partial {
                num(ui, "Tab power", &mut c.tabs.tab_power, pct());
            }
            ui_kit::checkbox(ui, "Skip inner shapes", &mut c.tabs.skip_inner);
        }
        ui_kit::section_heading(ui, "Start and end");
        num(ui, "Start pause", &mut c.start_pause_ms, Num::new(0.0..=60_000.0, 0, " ms").speed(1.0));
        num(ui, "End pause", &mut c.end_pause_ms, Num::new(0.0..=60_000.0, 0, " ms").speed(1.0));
        ui_kit::checkbox(ui, "Cut through at start", &mut c.cut_through_start);
        ui_kit::checkbox(ui, "Cut through at end", &mut c.cut_through_end);
        if c.cut_through_start || c.cut_through_end {
            num(ui, "Cut-through power", &mut c.cut_through_power, pct());
        }
        num(ui, "Overcut", &mut c.overcut, mm(0.0, 100.0));
        lead(ui, "Lead in", &mut c.lead_in);
        lead(ui, "Lead out", &mut c.lead_out);
        ui_kit::section_heading(ui, "Controller");
        ui_kit::checkbox(ui, "PWM frequency override", &mut c.pwm_override);
        if c.pwm_override {
            num(ui, "PWM frequency", &mut c.pwm_khz, Num::new(0.1..=1000.0, 1, " kHz").speed(0.1));
        }
        ui_kit::checkbox(ui, "PPI mode", &mut c.ppi_enabled);
        if c.ppi_enabled {
            num(ui, "Pulses per inch", &mut c.ppi, Num::new(1.0..=10_000.0, 0, "").speed(1.0));
        }
        ui_kit::checkbox(ui, "Dot mode", &mut c.dot_mode);
        if c.dot_mode {
            num(ui, "Dot time", &mut c.dot_time_ms, Num::new(0.001..=10_000.0, 2, " ms").speed(0.01));
            num(ui, "Dot spacing", &mut c.dot_spacing, mm(0.001, 1000.0));
        }
        num(ui, "U axis offset", &mut c.u_offset, mm(-1000.0, 1000.0));
    }
    if fill_like(c.mode) {
        ui_kit::section_heading(ui, "Fill");
        num(ui, "Angle increase per pass", &mut c.angle_increment, Num::new(-360.0..=360.0, 1, " deg").speed(0.5));
        pick(
            ui,
            "Fill grouping",
            &mut c.fill_grouping,
            &[
                (FillGrouping::AllAtOnce, "All shapes at once"),
                (FillGrouping::GroupsTogether, "Groups together"),
                (FillGrouping::Individually, "Shapes one by one"),
            ],
        );
        num(ui, "Ramp length", &mut c.ramp_length, mm(0.0, 100.0));
        ui_kit::checkbox(ui, "Ramp outer edge only", &mut c.ramp_outer);
        ui_kit::checkbox(ui, "Flood fill", &mut c.flood_fill);
        ui_kit::checkbox(ui, "Overscan as a percent of speed", &mut c.overscan_percent);
    }
    if c.mode == Mode::Image {
        ui_kit::section_heading(ui, "Image");
        num(ui, "Dot width correction", &mut c.dot_width_correction, mm(0.0, 10.0));
        num(ui, "Angle increase per pass", &mut c.angle_increment, Num::new(-360.0..=360.0, 1, " deg").speed(0.5));
        ui_kit::checkbox(ui, "Merge images on this layer into one scan", &mut c.merge_images);
        ui_kit::checkbox(ui, "Overscan as a percent of speed", &mut c.overscan_percent);
    }
    z_rows(ui, c);
}

fn lead(ui: &mut Ui, label: &str, l: &mut darkburn_core::layer::Lead) {
    ui_kit::checkbox(ui, label, &mut l.enabled);
    if l.enabled {
        ui.push_id(label, |ui| {
            num(ui, "  Length", &mut l.length, mm(0.0, 100.0));
            num(ui, "  Angle", &mut l.angle, Num::new(0.0..=180.0, 1, " deg").speed(0.5));
            pick(ui, "  Style", &mut l.style, &[(LeadStyle::Line, "Line"), (LeadStyle::Arc, "Arc")]);
        });
    }
}

/// The sections only some machines have (Gweike M-series fibre cutting). What the device lacks
/// is not shown, but stays in the layer untouched. The values are stored and written to the
/// Gweike files only; nothing here drives a machine.
fn machine_sections(ui: &mut Ui, caps: &darkburn_core::MachineCaps, c: &mut CutSetting) {
    use darkburn_core::layer::{GasKind, GweikeLayer, GweikeSource, PierceStep};
    let f = caps.layer_fields(c);
    if !f.any() {
        return;
    }
    let mut g = c.gweike.clone().unwrap_or_default();
    if f.source_select {
        ui_kit::section_heading(ui, "Source");
        pick(ui, "Laser source", &mut g.source, &[(GweikeSource::Fibre, "Fibre"), (GweikeSource::Co2, "CO2")]);
    }
    // The pick above may have changed the source: recompute what shows.
    let f = caps.layer_fields(&CutSetting { gweike: Some(g.clone()), ..CutSetting::default() });
    if f.source_freq {
        ui_kit::section_heading(ui, "Fibre source");
        num(ui, "Frequency", &mut g.freq_hz, Num::new(1.0..=100_000.0, 0, " Hz").speed(10.0));
        num(ui, "Peak current", &mut g.peak_current_pct, pct());
        num(ui, "Duty", &mut g.duty_pct, pct());
    }
    if f.follower {
        ui_kit::section_heading(ui, "Height follower");
        ui_kit::checkbox(ui, "Follow the sheet", &mut g.follow);
        if g.follow {
            num(ui, "Cut height", &mut g.cut_height_mm, mm(0.0, 30.0));
        } else {
            num(ui, "Cut position", &mut g.fixed_height_mm, mm(0.0, 60.0));
        }
        ui_kit::checkbox(ui, "Frog-jump lift", &mut g.frog_jump);
        num(ui, "Lift height", &mut g.lift_height_mm, mm(0.0, 60.0));
        ui_kit::checkbox(ui, "No lift on short moves", &mut g.short_distance_no_lift);
    }
    if f.gas_control {
        ui_kit::section_heading(ui, "Assist gas");
        let gases: Vec<(GasKind, &str)> = GasKind::ALL.iter().map(|k| (*k, k.label())).collect();
        pick(ui, "Gas", &mut g.gas, &gases);
        num(ui, "Gas pressure", &mut g.gas_pressure_bar, Num::new(0.0..=30.0, 1, " bar").speed(0.1));
        let ms = || Num::new(0.0..=60_000.0, 0, " ms").speed(1.0);
        num(ui, "Laser-on delay", &mut g.laser_on_delay_ms, ms());
        num(ui, "Delay before laser off", &mut g.laser_off_before_delay_ms, ms());
        num(ui, "Gas after laser off", &mut g.laser_off_after_delay_ms, ms());
        ui_kit::checkbox(ui, "Gas on during short moves", &mut g.gas_on_short_moves);
        ui_kit::checkbox(ui, "Gas on for the whole job", &mut g.gas_on_whole_job);
    }
    if f.piercing {
        ui_kit::section_heading(ui, "Piercing");
        ui_kit::checkbox(ui, "Pre-drill all start points", &mut g.pre_drill);
        ui_kit::checkbox(ui, "Pierce again before the cut", &mut g.pierce_again_before_cut);
        let mut stages = g.pierce.len() as u32;
        count(ui, "Pierce stages", &mut stages, 0, 5);
        g.pierce.resize_with(stages as usize, PierceStep::default);
        for (i, st) in g.pierce.iter_mut().enumerate() {
            let n = i + 1;
            num(ui, &format!("Stage {n} height"), &mut st.height_mm, mm(0.0, 30.0));
            num(ui, &format!("Stage {n} time"), &mut st.time_ms, Num::new(0.0..=60_000.0, 0, " ms").speed(1.0));
            num(ui, &format!("Stage {n} duty"), &mut st.duty_pct, pct());
            num(ui, &format!("Stage {n} frequency"), &mut st.freq_hz, Num::new(1.0..=100_000.0, 0, " Hz").speed(10.0));
            num(ui, &format!("Stage {n} peak current"), &mut st.peak_current_pct, pct());
            num(ui, &format!("Stage {n} gas pressure"), &mut st.gas_pressure_bar, Num::new(0.0..=30.0, 1, " bar").speed(0.1));
        }
        ui_kit::checkbox(ui, "Slow start", &mut g.slow_start);
        if g.slow_start {
            num(ui, "Slow start length", &mut g.slow_start_length_mm, mm(0.0, 100.0));
            num(ui, "Slow start speed", &mut g.slow_start_speed_mm_s, Num::new(0.1..=1000.0, 1, " mm/s").speed(0.5));
        }
    }
    if f.micro_joints || f.film_removal {
        ui_kit::section_heading(ui, "Contours");
    }
    if f.micro_joints {
        ui_kit::checkbox(ui, "Micro-joints", &mut g.micro_joints);
        if g.micro_joints {
            num(ui, "Joint length", &mut g.micro_joint_length_mm, mm(0.0, 10.0));
            num(ui, "Joint spacing", &mut g.micro_joint_spacing_mm, mm(1.0, 1000.0));
        }
    }
    if f.film_removal {
        ui_kit::checkbox(ui, "Film-removal pass first", &mut g.film_removal);
    }
    // Only a layer the user actually touched gets Gweike values.
    if c.gweike.is_some() || g != GweikeLayer::default() {
        c.gweike = Some(g);
    }
}

/// Galvo settings (only shown for galvo devices).
fn galvo(ui: &mut Ui, c: &mut CutSetting) {
    let g = &mut c.galvo;
    ui_kit::section_heading(ui, "Galvo");
    if c.mode == Mode::Line {
        ui_kit::checkbox(ui, "Drill mode (fire at points along the outline)", &mut g.drill);
        if g.drill {
            num(ui, "Drill time", &mut g.drill_dwell_ms, Num::new(0.01..=60_000.0, 2, " ms"));
            num(ui, "Point spacing", &mut g.drill_spacing_mm, mm(0.0, 1000.0));
            ui_kit::caption(ui, "Point spacing", "0 drills only the corners and ends.");
        }
    }
    num(ui, "Frequency", &mut g.frequency_khz, Num::new(1.0..=4000.0, 1, " kHz"));
    num(ui, "Q-pulse width", &mut g.q_pulse_ns, Num::new(1.0..=10_000.0, 0, " ns"));
    num(ui, "Pause before", &mut g.pause_before_ms, Num::new(0.0..=60_000.0, 0, " ms"));
    num(ui, "Pause after", &mut g.pause_after_ms, Num::new(0.0..=60_000.0, 0, " ms"));
    count(ui, "Global passes", &mut g.global_passes, 1, 10_000);
    ui_kit::checkbox(ui, "Auto-rotate", &mut g.auto_rotate);
    ui_kit::checkbox(ui, "Wobble", &mut g.wobble);
    if g.wobble {
        num(ui, "Wobble size", &mut g.wobble_size, mm(0.0, 10.0));
        num(ui, "Wobble step", &mut g.wobble_step, mm(0.0, 10.0));
    }
    ui_kit::checkbox(ui, "Timing override", &mut g.timing_override);
    if g.timing_override {
        num(ui, "Jump speed", &mut g.jump_speed, Num::new(1.0..=100_000.0, 0, " mm/s"));
        num(ui, "Min jump delay", &mut g.min_jump_delay_us, Num::new(0.0..=100_000.0, 0, " us"));
        num(ui, "Max jump delay", &mut g.max_jump_delay_us, Num::new(0.0..=100_000.0, 0, " us"));
        num(ui, "Jump distance limit", &mut g.jump_distance_limit, mm(0.0, 1000.0));
        num(ui, "Laser on delay", &mut g.laser_on_tc_us, Num::new(-10_000.0..=10_000.0, 0, " us"));
        num(ui, "Laser off delay", &mut g.laser_off_tc_us, Num::new(-10_000.0..=10_000.0, 0, " us"));
        num(ui, "End delay", &mut g.end_tc_us, Num::new(0.0..=100_000.0, 0, " us"));
        num(ui, "Polygon delay", &mut g.polygon_tc_us, Num::new(0.0..=100_000.0, 0, " us"));
    }
}

/// A strip showing how the image mode turns a grey ramp into dots.
fn dither_preview(ui: &mut Ui, mode: ImageMode) {
    let t = Tokens::get();
    let (w, h, cell) = (64usize, 8usize, 3.0f32);
    ui_kit::row(ui, "Sample", |ui| {
        let (r, _) = ui.allocate_exact_size(vec2(w as f32 * cell, h as f32 * cell), Sense::hover());
        let p = ui.painter();
        p.rect_filled(r, 0.0, Color32::from_rgb(235, 225, 205));
        for (i, v) in dither_sample(mode, w, h).into_iter().enumerate() {
            if v > 0.0 {
                let (x, y) = ((i % w) as f32, (i / w) as f32);
                let c = Rect::from_min_size(pos2(r.left() + x * cell, r.top() + y * cell), vec2(cell, cell));
                p.rect_filled(c, 0.0, Color32::from_black_alpha((v * 230.0) as u8));
            }
        }
        p.rect_stroke(r, 0.0, Stroke::new(1.0, t.border), egui::StrokeKind::Outside);
    });
}

/// A picture of the scan pattern: lines at the scan angle across a sample shape, overscan
/// past its edges, arrows for the direction of each line, cross-hatch, and offset fill rings.
fn scan_picture(ui: &mut Ui, c: &CutSetting) {
    let t = Tokens::get();
    let size = vec2(ui.available_width().min(320.0), 130.0);
    let (r, _) = ui.allocate_exact_size(size, Sense::hover());
    let p = ui.painter().with_clip_rect(r);
    p.rect_filled(r, t.radius_sm, t.panel_header);
    let shape = Rect::from_center_size(r.center(), vec2(r.width() * 0.5, r.height() * 0.6));
    let ink = t.accent;
    let dim = t.text_dim.gamma_multiply(0.6);
    if c.mode == Mode::OffsetFill {
        let mut s = shape;
        while s.width() > 6.0 && s.height() > 6.0 {
            p.rect_stroke(s, 3.0, Stroke::new(1.2, ink), egui::StrokeKind::Middle);
            s = s.shrink(7.0);
        }
    } else if fill_like(c.mode) || c.mode == Mode::Image {
        let over = if c.overscan_mm() > 0.0 { 14.0 } else { 0.0 };
        let mut angles = vec![c.scan_angle];
        if c.cross_hatch && c.mode != Mode::Image {
            angles.push(c.scan_angle + 90.0);
        }
        for (k, a) in angles.into_iter().enumerate() {
            let a = (a as f32).to_radians();
            let dir = vec2(a.cos(), -a.sin());
            let normal = vec2(-dir.y, dir.x);
            let spacing = 9.0;
            let reach = shape.size().length() / 2.0;
            let steps = (reach / spacing) as i32;
            for i in -steps..=steps {
                let centre = shape.center() + normal * (i as f32 * spacing);
                let Some((a0, a1)) = clip_line(centre, dir, shape) else { continue };
                let (from, to) = if c.bidirectional && i.rem_euclid(2) == 1 { (a1, a0) } else { (a0, a1) };
                let d = (to - from).normalized();
                p.line_segment([from, to], Stroke::new(1.2, if k == 0 { ink } else { ink.gamma_multiply(0.6) }));
                if over > 0.0 {
                    p.line_segment([from - d * over, from], Stroke::new(1.0, dim));
                    p.line_segment([to, to + d * over], Stroke::new(1.0, dim));
                }
                if k == 0 && i.rem_euclid(3) == 0 {
                    let tip = from + (to - from) * 0.55;
                    let back = tip - d * 5.0;
                    let side = vec2(-d.y, d.x) * 3.0;
                    p.line_segment([tip, back + side], Stroke::new(1.2, ink));
                    p.line_segment([tip, back - side], Stroke::new(1.2, ink));
                }
            }
        }
    }
    if line_like(c.mode) || c.mode == Mode::OffsetFill {
        p.rect_stroke(shape, 3.0, Stroke::new(1.6, t.text), egui::StrokeKind::Middle);
    } else {
        p.rect_stroke(shape, 3.0, Stroke::new(1.0, dim), egui::StrokeKind::Middle);
    }
    let note = match c.mode {
        Mode::Line => "Cuts along the outline",
        Mode::OffsetFill => "Rings that follow the outline inwards",
        Mode::Image => "Image scanned line by line",
        _ if c.bidirectional => "Scans both ways",
        _ => "Scans one way",
    };
    p.text(r.left_bottom() + vec2(6.0, -4.0), Align2::LEFT_BOTTOM, note, FontId::proportional(t.font_micro), t.text_dim);
}

/// Where the line through `p` along `dir` crosses `r`, as entry and exit points.
fn clip_line(p: Pos2, dir: egui::Vec2, r: Rect) -> Option<(Pos2, Pos2)> {
    let (mut t0, mut t1) = (f32::NEG_INFINITY, f32::INFINITY);
    for (o, d, lo, hi) in [(p.x, dir.x, r.left(), r.right()), (p.y, dir.y, r.top(), r.bottom())] {
        if d.abs() < 1e-6 {
            if o < lo || o > hi {
                return None;
            }
        } else {
            let (a, b) = ((lo - o) / d, (hi - o) / d);
            t0 = t0.max(a.min(b));
            t1 = t1.min(a.max(b));
        }
    }
    (t1 - t0 > 1.0).then(|| (p + dir * t0, p + dir * t1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use darkburn_core::Shape;
    use darkburn_core::kurbo::Rect as KRect;

    #[test]
    fn a_change_reaches_other_layers_without_their_other_values() {
        let before = CutSetting::default();
        let mut after = before.clone();
        after.speed = 250.0;
        after.tabs.size = 2.0;
        let mut other = CutSetting { max_power: 80.0, tabs: darkburn_core::layer::Tabs { spacing: 9.0, ..Default::default() }, ..Default::default() };
        apply_change(&before, &after, &mut other);
        assert_eq!(other.speed, 250.0);
        assert_eq!(other.tabs.size, 2.0);
        assert_eq!(other.max_power, 80.0, "untouched fields stay");
        assert_eq!(other.tabs.spacing, 9.0, "nested untouched fields stay");
    }

    #[test]
    fn caps_follow_the_device_kind() {
        use crate::devices::DeviceType as D;
        for kind in D::ALL {
            let caps = kind.caps();
            let gweike = kind.is_gweike();
            assert_eq!(caps.gweike.is_some(), gweike, "{kind:?}");
            assert_eq!(caps.layer_fields(&CutSetting::default()).any(), gweike, "{kind:?}: fibre sections only on Gweike");
        }
        assert!(D::Ruida.caps().dual_laser);
        assert!(D::SimulatorGalvo.caps().sources.galvo);
    }

    #[test]
    fn hidden_gweike_values_survive_a_change_on_another_device() {
        let g = darkburn_core::layer::GweikeLayer { gas_pressure_bar: 12.0, ..Default::default() };
        let before = CutSetting { gweike: Some(g.clone()), ..Default::default() };
        let after = CutSetting { speed: 40.0, ..before.clone() };
        let mut target = before.clone();
        apply_change(&before, &after, &mut target);
        assert_eq!(target.speed, 40.0);
        assert_eq!(target.gweike, Some(g));
    }

    #[test]
    fn reset_keeps_name_and_mode() {
        let s = CutSetting { name: "Score".into(), mode: Mode::Fill, speed: 3.0, ..Default::default() };
        let r = reset_setting(&s);
        assert_eq!((r.name.as_str(), r.mode, r.speed), ("Score", Mode::Fill, 100.0));
    }

    #[test]
    fn dither_samples_burn_more_where_darker() {
        for mode in ImageMode::ALL {
            let v = dither_sample(mode, 32, 8);
            assert_eq!(v.len(), 256);
            let col = |x: usize| (0..8).map(|y| v[y * 32 + x]).sum::<f32>();
            let left: f32 = (0..8).map(col).sum();
            let right: f32 = (24..32).map(col).sum();
            assert!(left > right, "{mode:?}: the dark end burns more");
        }
        let threshold = dither_sample(ImageMode::Threshold, 4, 1);
        assert_eq!(threshold, vec![1.0, 1.0, 0.0, 0.0]);
    }

    #[test]
    fn scan_lines_clip_to_the_shape() {
        let r = Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 50.0));
        let (a, b) = clip_line(pos2(50.0, 25.0), vec2(1.0, 0.0), r).unwrap();
        assert_eq!((a.x, b.x), (0.0, 100.0));
        assert!(clip_line(pos2(50.0, 80.0), vec2(1.0, 0.0), r).is_none());
    }

    #[test]
    fn ok_is_one_undo_step_and_cancel_restores() {
        let mut app = DarkBurn::new(crate::prefs::Prefs::default());
        app.editor.edit("Add", |d, _| {
            d.add(Shape::rect(KRect::new(0.0, 0.0, 1.0, 1.0), 0.0, 2));
            true
        });
        let session = Session { opened_for: 2, original: app.editor.doc.layers.clone(), primary: 2, selected: vec![2, 5], ..Default::default() };
        // Two live edits, applied to both selected layers.
        for speed in [50.0, 60.0] {
            let before = app.editor.doc.layer(2).main().clone();
            let after = CutSetting { speed, ..before.clone() };
            apply_all(&mut app, &session, &before, &after);
        }
        assert_eq!(app.editor.doc.layer(5).main().speed, 60.0);
        finish(&mut app, &session, true);
        assert_eq!(app.editor.history.len(), 2, "the session is one step");
        assert_eq!(app.editor.history.undo_label(), Some("Cut settings"));
        app.editor.undo();
        assert_eq!(app.editor.doc.layer(2).main().speed, 100.0);
        // Cancel.
        let session = Session { original: app.editor.doc.layers.clone(), ..session };
        let before = app.editor.doc.layer(2).main().clone();
        apply_all(&mut app, &session, &before, &CutSetting { max_power: 99.0, ..before.clone() });
        finish(&mut app, &session, false);
        assert_eq!(app.editor.doc.layer(2).main().max_power, 20.0);
        assert_eq!(app.editor.history.len(), 1);
    }
}
