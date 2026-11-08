//! The windows of the camera, rotary, cylinder, print-and-cut and test tools, the Camera
//! Control panel, the camera picture on the canvas and the mode chips in the status bar.
//! They call the functions in `vision_cmds.rs`; nothing here holds its own copy of the logic.
//!
//! A camera is opened only by the Start button in the panel, after the user allowed live
//! capture and picked the camera. Everything else works from picture files.

use darkburn_core::kurbo::Point;
use darkburn_ui::ui_kit::{self, BackingTooltipExt as _};
use egui::{Align2, Color32, Context, FontId, Id, Rect, RichText, Sense, Stroke, TextureHandle, Ui, pos2, vec2};
use serde_json::{Value, json};
use solidcraft_theme::Tokens;

use crate::render::Xf;
use crate::shell::DarkBurn;
use crate::vision_cmds as vc;
use crate::vision_state::SplitMode;
use darkburn_vision::cylinder::{CylinderAxis, CylinderKind};
use darkburn_vision::printcut::{PnCEvent, PnCMode, PnCStep};
use darkburn_vision::rotary::{ObjectSize, RotaryAxis, RotaryKind};

/// Which window is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dlg {
    Rotary,
    Cylinder,
    PrintCut,
    Focus,
    Interval,
    Centre,
    Feeder,
    Lens,
    Align,
    Head,
}

#[derive(Default, Clone)]
pub struct RotaryDraft {
    enabled: bool,
    chuck: bool,
    axis: usize,
    steps: f64,
    axis_steps: f64,
    roller: f64,
    by_circumference: bool,
    size: f64,
    mirror: bool,
    split: usize,
    split_max: f64,
    revs: f64,
    plan: String,
}

#[derive(Default, Clone)]
pub struct CylinderDraft {
    enabled: bool,
    concave: bool,
    along_y: bool,
    diameter: f64,
    centre: f64,
}

#[derive(Default, Clone)]
pub struct PncDraft {
    laser: [[f64; 2]; 4],
    design: [[f64; 2]; 4],
    mode: usize,
    wiz: [f64; 2],
    /// Waiting for a click on the canvas: (target, is the design position).
    pick: Option<(usize, bool)>,
    text: String,
}

#[derive(Default, Clone)]
pub struct TestDraft {
    x: f64,
    y: f64,
    first_layer: usize,
    label_layer: usize,
    text: String,
}

#[derive(Default, Clone)]
pub struct CentreDraft {
    pts: Vec<[f64; 2]>,
    mark: bool,
    text: String,
}

#[derive(Default, Clone)]
pub struct FeederDraft {
    width: f64,
    layer: usize,
    text: String,
}

#[derive(Default, Clone)]
pub struct LensDraft {
    checker: bool,
    cols: usize,
    rows: usize,
    spacing: f64,
    dark: bool,
    fisheye: bool,
    num: usize,
}

#[derive(Default, Clone)]
pub struct AlignDraft {
    count: usize,
    margin: f64,
    size: f64,
    layer: usize,
    flip_x: bool,
    flip_y: bool,
    rotate: usize,
    clicks: Vec<Point>,
    text: String,
}

#[derive(Default, Clone)]
pub struct HeadDraft {
    head: [f64; 2],
    bed: [f64; 2],
    pending: Option<Point>,
    text: String,
}

#[derive(Default)]
pub struct VisionDialogs {
    pub open: Option<Dlg>,
    rot: RotaryDraft,
    cyl: CylinderDraft,
    pnc: PncDraft,
    focus: TestDraft,
    interval: TestDraft,
    centre: CentreDraft,
    feeder: FeederDraft,
    lens: LensDraft,
    align: AlignDraft,
    head: HeadDraft,
    tex_raw: Option<(TextureHandle, u64)>,
    tex_overlay: Option<(TextureHandle, u64)>,
    tex_view: Option<(TextureHandle, u64)>,
    last_live: f64,
    /// The wizard picture: the lens-corrected frame and what it was made from.
    view_img: Option<(u64, image::RgbaImage)>,
}

fn upload(ctx: &Context, slot: &mut Option<(TextureHandle, u64)>, key: u64, name: &str, img: &image::RgbaImage) -> egui::TextureId {
    if slot.as_ref().is_none_or(|(_, k)| *k != key) {
        let ci = egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw());
        *slot = Some((ctx.load_texture(name, ci, egui::TextureOptions::LINEAR), key));
    }
    slot.as_ref().map(|(t, _)| t.id()).unwrap_or_default()
}

// ----- Commands from menus ------------------------------------------------------------------------

/// Menu and toolbar clicks for this area. Returns true when `id` was ours.
pub fn run(app: &mut DarkBurn, ctx: &Context, id: &str) -> bool {
    if !vc::handles(id) {
        return false;
    }
    vc::sync(app);
    let open = |app: &mut DarkBurn, d: Dlg| open_dialog(app, d);
    let say = |app: &mut DarkBurn, r: Result<Value, String>| {
        app.note = Some(match r {
            Ok(_) => String::new(),
            Err(e) => e,
        })
        .filter(|s| !s.is_empty());
    };
    match id {
        "laser.rotary" => open(app, Dlg::Rotary),
        "laser.cylinder" => open(app, Dlg::Cylinder),
        "laser.print_and_cut" | "laser.pnc_first" | "laser.pnc_second" | "laser.pnc_wizard" => open(app, Dlg::PrintCut),
        "laser.pnc_align" => {
            let r = vc::pnc_align(app, PnCMode::Scaled);
            say(app, r)
        }
        "laser.pnc_align_noscale" => {
            let r = vc::pnc_align(app, PnCMode::Unscaled);
            say(app, r)
        }
        "laser.pnc_reset" => {
            vc::pnc_reset(app);
            app.note = None;
        }
        "laser.focus_test" => open(app, Dlg::Focus),
        "laser.interval_test" => open(app, Dlg::Interval),
        "laser.centre_finder" => open(app, Dlg::Centre),
        "laser.feeder" => open(app, Dlg::Feeder),
        "laser.camera_lens" => open(app, Dlg::Lens),
        "laser.camera_alignment" => open(app, Dlg::Align),
        "laser.capture" => {
            let r = vc::update_overlay(app);
            say(app, r)
        }
        "file.save_capture" => {
            if let Some(p) = rfd::FileDialog::new().add_filter("Picture", &["png", "jpg", "bmp"]).set_file_name("camera.png").save_file() {
                let r = vc::save_capture(app, &p);
                say(app, r);
            }
        }
        _ => return false,
    }
    ctx.request_repaint();
    true
}

fn open_dialog(app: &mut DarkBurn, d: Dlg) {
    let s = &app.vision.state;
    let u = &mut app.vision.ui;
    match d {
        Dlg::Rotary => {
            let r = &s.rotary;
            u.rot = RotaryDraft {
                enabled: r.enabled,
                chuck: r.kind == RotaryKind::Chuck,
                axis: RotaryAxis::all().iter().position(|a| *a == r.axis).unwrap_or(0),
                steps: r.steps_per_rotation,
                axis_steps: r.axis_steps_per_mm,
                roller: r.roller_diameter,
                by_circumference: matches!(r.object, ObjectSize::Circumference(_)),
                size: match r.object {
                    ObjectSize::Diameter(v) | ObjectSize::Circumference(v) => v,
                },
                mirror: r.mirror,
                split: match s.rotary_split.mode {
                    SplitMode::Off => 0,
                    SplitMode::Slice => 1,
                    SplitMode::Shape => 2,
                },
                split_max: s.rotary_split.max_size,
                revs: 1.0,
                plan: String::new(),
            };
        }
        Dlg::Cylinder => {
            let c = &s.cylinder;
            u.cyl = CylinderDraft {
                enabled: c.enabled,
                concave: c.params.kind == CylinderKind::Concave,
                along_y: c.params.axis == CylinderAxis::AlongY,
                diameter: c.params.radius * 2.0,
                centre: c.params.centre,
            };
        }
        Dlg::PrintCut => {
            let p = &s.pnc;
            let mut d = PncDraft::default();
            for i in 0..4 {
                if let Some(Some(v)) = p.measured.get(i) {
                    d.laser[i] = [v.x, v.y];
                }
                if let Some(Some(v)) = p.design.get(i) {
                    d.design[i] = [v.x, v.y];
                }
            }
            d.mode = match p.mode {
                PnCMode::Scaled => 0,
                PnCMode::Unscaled => 1,
                PnCMode::FullAffine => 2,
            };
            u.pnc = d;
        }
        Dlg::Focus => {
            u.focus = TestDraft { x: 10.0, y: 10.0, first_layer: 0, label_layer: 29, text: String::new() };
        }
        Dlg::Interval => {
            u.interval = TestDraft { x: 10.0, y: 10.0, first_layer: 0, label_layer: 29, text: String::new() };
        }
        Dlg::Centre => {
            u.centre = CentreDraft { pts: vec![[0.0, 0.0]; 3], mark: true, text: String::new() };
        }
        Dlg::Feeder => {
            u.feeder = FeederDraft { width: 100.0, layer: 28, text: String::new() };
        }
        Dlg::Lens => {
            u.lens = LensDraft { checker: false, cols: 7, rows: 5, spacing: 25.0, dark: true, fisheye: false, num: 2 };
        }
        Dlg::Align => {
            let c = app.vision.align_count.max(4);
            let m = if app.vision.align_margin > 0.0 { app.vision.align_margin } else { 20.0 };
            app.vision.ui.align = AlignDraft { count: c, margin: m, size: 8.0, layer: app.editor.current_layer, ..Default::default() };
        }
        Dlg::Head => {
            u.head = HeadDraft::default();
        }
    }
    app.vision.ui.open = Some(d);
}

// ----- Per frame ------------------------------------------------------------------------------------

/// Called every frame: keeps the setup in step with the document, reads the live camera when the
/// user started one, takes canvas clicks for picking positions and shows the open window.
pub fn frame(ctx: &Context, app: &mut DarkBurn) {
    vc::sync(app);
    if app.vision.live {
        let now = ctx.input(|i| i.time);
        if now - app.vision.ui.last_live >= 0.1 {
            app.vision.ui.last_live = now;
            if let Some(src) = app.vision.source.as_mut() {
                match src.grab() {
                    Ok(img) => {
                        app.vision.raw = Some(img);
                        app.vision.frame_gen += 1;
                    }
                    Err(e) => {
                        app.vision.message = Some(e.to_string());
                        app.vision.live = false;
                        app.vision.source = None;
                    }
                }
            }
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
    take_pick(ctx, app);
    match app.vision.ui.open {
        Some(Dlg::Rotary) => rotary_dialog(ctx, app),
        Some(Dlg::Cylinder) => cylinder_dialog(ctx, app),
        Some(Dlg::PrintCut) => pnc_dialog(ctx, app),
        Some(Dlg::Focus) => focus_dialog(ctx, app),
        Some(Dlg::Interval) => interval_dialog(ctx, app),
        Some(Dlg::Centre) => centre_dialog(ctx, app),
        Some(Dlg::Feeder) => feeder_dialog(ctx, app),
        Some(Dlg::Lens) => lens_dialog(ctx, app),
        Some(Dlg::Align) => align_dialog(ctx, app),
        Some(Dlg::Head) => head_dialog(ctx, app),
        None => {}
    }
}

/// A click on the canvas while a position is being picked fills in that field.
fn take_pick(ctx: &Context, app: &mut DarkBurn) {
    let Some((i, design)) = app.vision.ui.pnc.pick else { return };
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.vision.ui.pnc.pick = None;
        return;
    }
    let clicked = ctx.input(|i| i.pointer.primary_clicked());
    if !(clicked && !ctx.is_pointer_over_egui()) {
        return;
    }
    let Some(m) = app.cursor else { return };
    let page = app.prefs.page();
    let p = app.prefs.origin.to_machine(m, page);
    let doc = [f64::from(p.x), f64::from(page.y - p.y)];
    let d = &mut app.vision.ui.pnc;
    if design {
        d.design[i] = doc;
    } else {
        d.laser[i] = [f64::from(m.x), f64::from(m.y)];
    }
    d.pick = None;
}

fn window(ctx: &Context, title: &str, width: f32, add: impl FnOnce(&mut Ui)) -> bool {
    let mut open = true;
    let screen = ctx.content_rect();
    egui::Window::new(title)
        .id(Id::unique(("vision_window", title)))
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_pos(pos2((screen.right() - width - 330.0).max(60.0), 100.0))
        .default_width(width)
        .show(ctx, add);
    open
}

fn close(app: &mut DarkBurn) {
    app.vision.ui.open = None;
    app.vision.ui.pnc.pick = None;
}

fn note(ui: &mut Ui, text: &str) {
    let t = Tokens::get();
    ui.label(RichText::new(text).size(t.font_small).color(t.text_dim));
}

fn field(ui: &mut Ui, label: &str, v: &mut f64, unit: &str) {
    ui_kit::row(ui, label, |ui| {
        ui.add(egui::DragValue::new(v).speed(0.1).suffix(unit));
    });
}

fn count_field(ui: &mut Ui, label: &str, v: &mut usize, range: std::ops::RangeInclusive<usize>) {
    ui_kit::row(ui, label, |ui| {
        ui.add(egui::DragValue::new(v).range(range));
    });
}

fn result_line(ui: &mut Ui, text: &str) {
    if !text.is_empty() {
        ui_kit::status_line(ui, text, ui_kit::StatusTone::Dim);
    }
}

fn msg(r: Result<Value, String>, ok: impl FnOnce(&Value) -> String) -> String {
    match r {
        Ok(v) => ok(&v),
        Err(e) => e,
    }
}

fn button(ui: &mut Ui, label: &str, tip: &str) -> bool {
    ui_kit::button(ui, label, ui_kit::Button::Small).backing_on_hover_text(tip.to_string()).clicked()
}

// ----- Rotary ---------------------------------------------------------------------------------------

const SPLITS: [&str; 3] = ["off", "slice", "shape"];

fn rotary_args(d: &RotaryDraft) -> Value {
    let mut v = json!({
        "enabled": d.enabled,
        "kind": if d.chuck { "chuck" } else { "roller" },
        "axis": RotaryAxis::all()[d.axis.min(5)].letter().to_string(),
        "steps_per_rotation": d.steps,
        "axis_steps_per_mm": d.axis_steps,
        "roller_diameter": d.roller,
        "mirror": d.mirror,
        "split": SPLITS[d.split.min(2)],
        "split_max_size": d.split_max,
    });
    v[if d.by_circumference { "circumference" } else { "diameter" }] = json!(d.size);
    v
}

fn rotary_dialog(ctx: &Context, app: &mut DarkBurn) {
    let mut d = std::mem::take(&mut app.vision.ui.rot);
    let mut closing = false;
    let open = window(ctx, "Rotary Setup", 340.0, |ui| {
        ui_kit::checkbox(ui, "Enable rotary", &mut d.enabled);
        ui_kit::section_heading(ui, "Type");
        let mut kind = usize::from(!d.chuck);
        ui_kit::row(ui, "Rotary type", |ui| ui_kit::dropdown(ui, "vision_rot_kind", &mut kind, &["Chuck", "Roller"]));
        d.chuck = kind == 0;
        let axes: Vec<String> = RotaryAxis::all().iter().map(|a| a.letter().to_string()).collect();
        let axes: Vec<&str> = axes.iter().map(String::as_str).collect();
        ui_kit::row(ui, "Rotary axis", |ui| ui_kit::dropdown(ui, "vision_rot_axis", &mut d.axis, &axes));
        field(ui, "Steps per rotation", &mut d.steps, "");
        field(ui, "Axis steps per mm", &mut d.axis_steps, "");
        if !d.chuck {
            field(ui, "Roller diameter", &mut d.roller, " mm");
        }
        let mut by = usize::from(d.by_circumference);
        ui_kit::row(ui, "Object size", |ui| ui_kit::dropdown(ui, "vision_rot_by", &mut by, &["Diameter", "Circumference"]));
        d.by_circumference = by == 1;
        field(ui, if d.by_circumference { "Circumference" } else { "Diameter" }, &mut d.size, " mm");
        ui_kit::checkbox(ui, "Mirror (turn the other way)", &mut d.mirror);
        ui_kit::section_heading(ui, "Galvo");
        ui_kit::row(ui, "Split the job", |ui| ui_kit::dropdown(ui, "vision_rot_split", &mut d.split, &["Off", "By slice", "By shape"]));
        if d.split > 0 {
            field(ui, "Largest piece", &mut d.split_max, " mm");
        }
        ui_kit::section_heading(ui, "Test rotation");
        field(ui, "Turns", &mut d.revs, "");
        ui.horizontal(|ui| {
            if button(ui, "Plan test", "Shows what the test would do; nothing is sent from here") {
                d.plan = msg(vc::rotary_set(app, &rotary_args(&d)).and_then(|_| execute_plan(app, d.revs)), plan_text);
            }
        });
        result_line(ui, &d.plan);
        note(ui, "A test rotation turns the object one way and back so you can check it returns to the start. The machine side sends it.");
        ui.separator();
        ui.horizontal(|ui| {
            if button(ui, "Apply", "Keep these settings") {
                d.plan = msg(vc::rotary_set(app, &rotary_args(&d)), |_| "Saved.".into());
            }
            if button(ui, "Close", "") {
                closing = true;
            }
        });
    });
    app.vision.ui.rot = d;
    if !open || closing {
        close(app);
    }
}

fn execute_plan(app: &mut DarkBurn, revs: f64) -> Result<Value, String> {
    crate::api::execute_headless(app, "rotary.test_plan", &json!({ "revolutions": revs }))
}

fn plan_text(v: &Value) -> String {
    let m = &v["plan"]["moves"][0];
    format!(
        "Turn axis {} by {:.2} units ({:.0} steps, {:.0} degrees) and back.",
        m["axis"].as_str().unwrap_or("?"),
        m["distance"].as_f64().unwrap_or(0.0),
        v["plan"]["steps"].as_f64().unwrap_or(0.0),
        v["plan"]["angle_deg"].as_f64().unwrap_or(0.0)
    )
}

// ----- Cylinder -------------------------------------------------------------------------------------

fn cylinder_dialog(ctx: &Context, app: &mut DarkBurn) {
    let mut d = std::mem::take(&mut app.vision.ui.cyl);
    let mut closing = false;
    let open = window(ctx, "Cylinder Correction", 320.0, |ui| {
        ui_kit::checkbox(ui, "Enable cylinder correction", &mut d.enabled);
        let mut kind = usize::from(d.concave);
        ui_kit::row(ui, "Surface", |ui| ui_kit::dropdown(ui, "vision_cyl_kind", &mut kind, &["Convex (outside)", "Concave (inside)"]));
        d.concave = kind == 1;
        let mut ax = usize::from(d.along_y);
        ui_kit::row(ui, "Cylinder runs along", |ui| ui_kit::dropdown(ui, "vision_cyl_axis", &mut ax, &["X", "Y"]));
        d.along_y = ax == 1;
        field(ui, "Diameter", &mut d.diameter, " mm");
        field(ui, if d.along_y { "Centre line X" } else { "Centre line Y" }, &mut d.centre, " mm");
        note(
            ui,
            "The design is stretched across the curve so it comes out true to size. Only the part within a quarter turn either side of the centre line can be reached from above.",
        );
        ui.horizontal(|ui| {
            if button(ui, "Apply", "") {
                let args = json!({ "enabled": d.enabled, "kind": if d.concave { "concave" } else { "convex" }, "axis": if d.along_y { "along_y" } else { "along_x" }, "diameter": d.diameter, "centre": d.centre });
                app.vision.message = vc::cylinder_set(app, &args).err();
            }
            if button(ui, "Close", "") {
                closing = true;
            }
        });
        if let Some(m) = &app.vision.message {
            ui_kit::status_line(ui, m, ui_kit::StatusTone::Error);
        }
    });
    app.vision.ui.cyl = d;
    if !open || closing {
        app.vision.message = None;
        close(app);
    }
}

// ----- Print and cut ------------------------------------------------------------------------------

fn pnc_dialog(ctx: &Context, app: &mut DarkBurn) {
    let mut d = std::mem::take(&mut app.vision.ui.pnc);
    let mut closing = false;
    let open = window(ctx, "Print and Cut", 420.0, |ui| {
        note(
            ui,
            "Mark two spots on the printed artwork. For each, put the laser on the spot, note the laser position and the matching position in the design, then set the target.",
        );
        let names = ["First target", "Second target", "Third target", "Fourth target"];
        #[allow(clippy::needless_range_loop, reason = "i indexes several parallel arrays")]
        for i in 0..2 {
            ui_kit::section_heading(ui, names[i]);
            ui.horizontal(|ui| {
                ui.label("Laser");
                ui.add(egui::DragValue::new(&mut d.laser[i][0]).speed(0.1).prefix("X ").suffix(" mm"));
                ui.add(egui::DragValue::new(&mut d.laser[i][1]).speed(0.1).prefix("Y ").suffix(" mm"));
                if button(ui, "Pick", "Click the canvas to take the position") {
                    d.pick = Some((i, false));
                }
            });
            ui.horizontal(|ui| {
                ui.label("Design");
                ui.add(egui::DragValue::new(&mut d.design[i][0]).speed(0.1).prefix("X ").suffix(" mm"));
                ui.add(egui::DragValue::new(&mut d.design[i][1]).speed(0.1).prefix("Y ").suffix(" mm"));
                if button(ui, "Pick", "Click the canvas to take the position") {
                    d.pick = Some((i, true));
                }
            });
            if button(ui, &format!("Set {}", names[i].to_lowercase()), "") {
                let args = json!({ "index": i, "x": d.laser[i][0], "y": d.laser[i][1], "design_x": d.design[i][0], "design_y": d.design[i][1] });
                d.text = msg(crate::api::execute_headless(app, "pnc.set_target", &args), |_| format!("{} set.", names[i]));
            }
        }
        if d.pick.is_some() {
            ui_kit::status_line(ui, "Click on the canvas to take the position (Esc cancels).", ui_kit::StatusTone::Warning);
        }
        ui.separator();
        ui.horizontal(|ui| {
            if button(ui, "Align (with scaling)", "Move, rotate and scale the output to meet the targets") {
                d.text = msg(vc::pnc_align(app, PnCMode::Scaled), align_text);
            }
            if button(ui, "Align (no scaling)", "Move and rotate only") {
                d.text = msg(vc::pnc_align(app, PnCMode::Unscaled), align_text);
            }
            if button(ui, "Reset", "Forget the targets and the alignment") {
                vc::pnc_reset(app);
                d = PncDraft::default();
            }
        });
        result_line(ui, &d.text);
        if let Some(s) = vc::pnc_status(&app.vision.state) {
            ui_kit::status_line(ui, &s, ui_kit::StatusTone::Dim);
        }
        ui_kit::section_heading(ui, "Wizard");
        wizard_section(ui, app, &mut d);
        ui.separator();
        if button(ui, "Close", "") {
            closing = true;
        }
    });
    let keep_pick = d.pick;
    app.vision.ui.pnc = d;
    app.vision.ui.pnc.pick = keep_pick;
    if !open || closing {
        close(app);
    }
}

fn align_text(v: &Value) -> String {
    format!(
        "Aligned: scale {:.4}, turn {:.2} degrees, error {:.3} mm.",
        v["scale"].as_f64().unwrap_or(1.0),
        v["rotation_deg"].as_f64().unwrap_or(0.0),
        v["rms_mm"].as_f64().unwrap_or(0.0)
    )
}

fn wizard_section(ui: &mut Ui, app: &mut DarkBurn, d: &mut PncDraft) {
    let Some(w) = app.vision.state.pnc.wizard.clone() else {
        ui.horizontal(|ui| {
            if button(ui, "Start wizard", "Uses the corners of the selection as the design targets, or the targets above") {
                d.text = msg(vc::pnc_wizard_start(app, None), |_| "Wizard started.".into());
            }
        });
        return;
    };
    let mut ev: Option<PnCEvent> = None;
    match w.step {
        PnCStep::ChooseMode => {
            note(ui, "Choose how the output may change to meet the targets.");
            ui.horizontal(|ui| {
                if button(ui, "With scaling", "") {
                    ev = Some(PnCEvent::SelectMode(PnCMode::Scaled));
                }
                if button(ui, "No scaling", "") {
                    ev = Some(PnCEvent::SelectMode(PnCMode::Unscaled));
                }
                if w.design_targets.len() >= 3 && button(ui, "Full fit", "Also allows shear and different X and Y scale") {
                    ev = Some(PnCEvent::SelectMode(PnCMode::FullAffine));
                }
            });
        }
        PnCStep::MeasureTarget(i) => {
            if let Some(t) = w.current_target() {
                note(
                    ui,
                    &format!(
                        "Target {} of {}: design position {:.2}, {:.2}. Put the laser on it and enter the laser position.",
                        i + 1,
                        w.design_targets.len(),
                        t.x,
                        t.y
                    ),
                );
            }
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut d.wiz[0]).speed(0.1).prefix("X ").suffix(" mm"));
                ui.add(egui::DragValue::new(&mut d.wiz[1]).speed(0.1).prefix("Y ").suffix(" mm"));
                if button(ui, "Set", "") {
                    ev = Some(PnCEvent::Measured(Point::new(d.wiz[0], d.wiz[1])));
                }
                if button(ui, "Back", "") {
                    ev = Some(PnCEvent::Back);
                }
            });
        }
        PnCStep::Review => {
            if let Some(f) = &w.fit {
                note(
                    ui,
                    &format!(
                        "Scale {:.4}, turn {:.2} degrees, error {:.3} mm (worst {:.3} mm).",
                        f.scale, f.rotation_deg, f.residuals.rms_mm, f.residuals.max_mm
                    ),
                );
            }
            ui.horizontal(|ui| {
                if button(ui, "Accept", "Use this alignment for the job") {
                    ev = Some(PnCEvent::Confirm);
                }
                if button(ui, "Back", "") {
                    ev = Some(PnCEvent::Back);
                }
            });
        }
        PnCStep::Done => note(ui, "The alignment is in use."),
        PnCStep::Cancelled => note(ui, "The wizard was cancelled."),
    }
    ui.horizontal(|ui| {
        if button(ui, "Restart", "") {
            ev = Some(PnCEvent::Reset);
        }
        if button(ui, "Cancel", "") {
            ev = Some(PnCEvent::Cancel);
        }
    });
    if let Some(e) = ev {
        d.text = match vc::pnc_wizard_event(app, e) {
            Ok(_) => String::new(),
            Err(e) => e,
        };
    }
}

// ----- Test tools ------------------------------------------------------------------------------------

fn place_fields(ui: &mut Ui, d: &mut TestDraft) {
    ui.horizontal(|ui| {
        ui.label("Lower left at");
        ui.add(egui::DragValue::new(&mut d.x).speed(0.5).prefix("X ").suffix(" mm"));
        ui.add(egui::DragValue::new(&mut d.y).speed(0.5).prefix("Y ").suffix(" mm"));
    });
    count_field(ui, "First layer", &mut d.first_layer, 0..=29);
    count_field(ui, "Label layer", &mut d.label_layer, 0..=29);
}

fn focus_dialog(ctx: &Context, app: &mut DarkBurn) {
    let mut d = std::mem::take(&mut app.vision.ui.focus);
    let mut p = app.vision.state.focus;
    let mut closing = false;
    let open = window(ctx, "Focus Test", 340.0, |ui| {
        note(ui, "Draws a line for each height. Each line sits on its own layer with that Z offset. Burn it, then pick the sharpest line.");
        field(ui, "Z start", &mut p.z_start, " mm");
        field(ui, "Z end", &mut p.z_end, " mm");
        let mut steps = p.steps;
        count_field(ui, "Lines", &mut steps, 2..=29);
        p.steps = steps;
        field(ui, "Line length", &mut p.line_length, " mm");
        field(ui, "Row spacing", &mut p.row_pitch, " mm");
        field(ui, "Text height", &mut p.text_height, " mm");
        ui_kit::checkbox(ui, "Write the heights", &mut p.with_labels);
        place_fields(ui, &mut d);
        ui.horizontal(|ui| {
            if button(ui, "Create", "Adds the test to the design") {
                let args = json!({ "z_start": p.z_start, "z_end": p.z_end, "steps": p.steps, "line_length": p.line_length, "row_pitch": p.row_pitch, "text_height": p.text_height, "labels": p.with_labels, "x": d.x, "y": d.y, "first_layer": d.first_layer, "label_layer": d.label_layer });
                d.text = msg(crate::api::execute_headless(app, "test.focus", &args), |v| format!("Added {} shapes on {} layers.", v["created"].as_array().map_or(0, Vec::len), v["layers"].as_array().map_or(0, Vec::len)));
            }
            if button(ui, "Close", "") {
                closing = true;
            }
        });
        result_line(ui, &d.text);
    });
    app.vision.state.focus = p;
    app.vision.ui.focus = d;
    if !open || closing {
        close(app);
    }
}

fn interval_dialog(ctx: &Context, app: &mut DarkBurn) {
    let mut d = std::mem::take(&mut app.vision.ui.interval);
    let mut p = app.vision.state.interval;
    let mut closing = false;
    let open = window(ctx, "Interval Test", 340.0, |ui| {
        note(ui, "Draws a filled block for each line interval, each on its own layer. Burn it, then pick the interval that looks best.");
        field(ui, "Interval start", &mut p.interval_start, " mm");
        field(ui, "Interval end", &mut p.interval_end, " mm");
        let mut steps = p.steps;
        count_field(ui, "Blocks", &mut steps, 2..=29);
        p.steps = steps;
        field(ui, "Block width", &mut p.block_width, " mm");
        field(ui, "Block height", &mut p.block_height, " mm");
        field(ui, "Gap", &mut p.gap, " mm");
        ui_kit::checkbox(ui, "Write the intervals", &mut p.with_labels);
        place_fields(ui, &mut d);
        ui.horizontal(|ui| {
            if button(ui, "Create", "Adds the test to the design") {
                let args = json!({ "interval_start": p.interval_start, "interval_end": p.interval_end, "steps": p.steps, "block_width": p.block_width, "block_height": p.block_height, "gap": p.gap, "labels": p.with_labels, "x": d.x, "y": d.y, "first_layer": d.first_layer, "label_layer": d.label_layer });
                d.text = msg(crate::api::execute_headless(app, "test.interval", &args), |v| format!("Added {} shapes on {} layers.", v["created"].as_array().map_or(0, Vec::len), v["layers"].as_array().map_or(0, Vec::len)));
            }
            if button(ui, "Close", "") {
                closing = true;
            }
        });
        result_line(ui, &d.text);
    });
    app.vision.state.interval = p;
    app.vision.ui.interval = d;
    if !open || closing {
        close(app);
    }
}

fn centre_dialog(ctx: &Context, app: &mut DarkBurn) {
    let mut d = std::mem::take(&mut app.vision.ui.centre);
    let mut closing = false;
    let open = window(ctx, "Centre Finder", 340.0, |ui| {
        note(
            ui,
            "Put the laser on the edge of the round stock at three or more places and enter each laser position. The centre and diameter come from those points.",
        );
        let mut remove = None;
        for (i, p) in d.pts.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.label(format!("Edge {}", i + 1));
                ui.add(egui::DragValue::new(&mut p[0]).speed(0.1).prefix("X ").suffix(" mm"));
                ui.add(egui::DragValue::new(&mut p[1]).speed(0.1).prefix("Y ").suffix(" mm"));
                if d_len_ok(i) && button(ui, "Remove", "") {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            d.pts.remove(i);
        }
        if button(ui, "Add a point", "") {
            d.pts.push([0.0, 0.0]);
        }
        ui_kit::checkbox(ui, "Mark the centre in the design", &mut d.mark);
        ui.horizontal(|ui| {
            if button(ui, "Find centre", "") {
                let pts: Vec<Value> = d.pts.iter().map(|p| json!({ "x": p[0], "y": p[1] })).collect();
                let layer = app.editor.current_layer;
                d.text = msg(crate::api::execute_headless(app, "test.centre", &json!({ "points": pts, "mark": d.mark, "layer": layer })), |v| {
                    format!(
                        "Centre {:.2}, {:.2} mm, diameter {:.2} mm.",
                        v["centre"]["x"].as_f64().unwrap_or(0.0),
                        v["centre"]["y"].as_f64().unwrap_or(0.0),
                        v["diameter"].as_f64().unwrap_or(0.0)
                    )
                });
            }
            if button(ui, "Close", "") {
                closing = true;
            }
        });
        result_line(ui, &d.text);
    });
    app.vision.ui.centre = d;
    if !open || closing {
        close(app);
    }
}

fn d_len_ok(i: usize) -> bool {
    i >= 3
}

fn feeder_dialog(ctx: &Context, app: &mut DarkBurn) {
    let mut d = std::mem::take(&mut app.vision.ui.feeder);
    let mut f = app.vision.state.feeder;
    let mut closing = false;
    let open = window(ctx, "Feeder Setup", 360.0, |ui| {
        ui_kit::section_heading(ui, "Long piece");
        field(ui, "Piece length", &mut f.total_length, " mm");
        field(ui, "Window height", &mut f.window_height, " mm");
        field(ui, "Overlap", &mut f.overlap, " mm");
        ui_kit::section_heading(ui, "Feed");
        field(ui, "Roller diameter", &mut f.roller_diameter, " mm");
        field(ui, "Steps per rotation", &mut f.steps_per_rotation, "");
        field(ui, "Feed speed", &mut f.speed_mm_s, " mm/s");
        field(ui, "Settle time", &mut f.settle_ms, " ms");
        ui_kit::section_heading(ui, "Calibrate distance");
        note(ui, "Feed a known distance, measure what the material really moved, and the corrected steps per mm come out below.");
        field(ui, "Steps per mm now", &mut f.current_steps_per_mm, "");
        field(ui, "Asked for", &mut f.commanded, " mm");
        field(ui, "Measured", &mut f.measured, " mm");
        ui_kit::section_heading(ui, "Windows");
        field(ui, "Drawn width", &mut d.width, " mm");
        count_field(ui, "Layer", &mut d.layer, 0..=29);
        ui.horizontal(|ui| {
            for (label, draw) in [("Calculate", false), ("Draw windows", true)] {
                if button(ui, label, if draw { "Adds an outline for each window to the design" } else { "" }) {
                    let args = json!({
                        "total_length": f.total_length, "window_height": f.window_height, "overlap": f.overlap, "roller_diameter": f.roller_diameter,
                        "steps_per_rotation": f.steps_per_rotation, "speed_mm_s": f.speed_mm_s, "settle_ms": f.settle_ms, "current_steps_per_mm": f.current_steps_per_mm,
                        "commanded": f.commanded, "measured": f.measured, "draw": draw, "width": d.width, "layer": d.layer,
                    });
                    d.text = msg(crate::api::execute_headless(app, "test.feeder", &args), |v| {
                        format!(
                            "{} windows, feed {:.2} mm each ({:.0} steps, wait {:.0} ms). Corrected steps per mm: {:.4}.",
                            v["windows"], v["advance_mm"].as_f64().unwrap_or(0.0), v["roller_feed_steps"].as_f64().unwrap_or(0.0),
                            v["feed_delay_ms"].as_f64().unwrap_or(0.0), v["corrected_steps_per_mm"].as_f64().unwrap_or(0.0)
                        )
                    });
                }
            }
            if button(ui, "Close", "") {
                closing = true;
            }
        });
        result_line(ui, &d.text);
    });
    app.vision.state.feeder = f;
    app.vision.ui.feeder = d;
    if !open || closing {
        close(app);
    }
}

// ----- Camera: lens, alignment and head wizards ------------------------------------------------------

/// The lens-corrected frame as a texture for clicking on, with its pixel size.
fn view_texture(ctx: &Context, app: &mut DarkBurn) -> Option<(egui::TextureId, (u32, u32))> {
    let key = app.vision.frame_gen * 4 + u64::from(app.vision.state.camera.lens.is_some());
    if app.vision.ui.view_img.as_ref().is_none_or(|(k, _)| *k != key) {
        let img = vc::undistorted(app)?;
        app.vision.ui.view_img = Some((key, img));
    }
    let (_, img) = app.vision.ui.view_img.as_ref()?;
    let size = (img.width(), img.height());
    let id = upload(ctx, &mut app.vision.ui.tex_view, key, "vision_view", img);
    Some((id, size))
}

/// Draws the picture at a width that fits, returns the pixel clicked.
fn clickable_picture(ui: &mut Ui, tex: egui::TextureId, size: (u32, u32), marks: &[Point]) -> Option<Point> {
    let w = ui.available_width().clamp(200.0, 520.0);
    let h = w * size.1 as f32 / size.0.max(1) as f32;
    let resp = ui.add(egui::Image::new(egui::load::SizedTexture::new(tex, vec2(w, h))).sense(Sense::click()));
    let r = resp.rect;
    for (i, m) in marks.iter().enumerate() {
        let c = pos2(r.left() + m.x as f32 / size.0 as f32 * r.width(), r.top() + m.y as f32 / size.1 as f32 * r.height());
        ui.painter().circle_stroke(c, 7.0, Stroke::new(2.0, Color32::from_rgb(255, 60, 60)));
        ui.painter().text(c + vec2(9.0, -9.0), Align2::LEFT_BOTTOM, (i + 1).to_string(), FontId::proportional(12.0), Color32::from_rgb(255, 60, 60));
    }
    if resp.clicked() {
        let p = resp.interact_pointer_pos()?;
        return Some(Point::new(
            f64::from((p.x - r.left()) / r.width()) * f64::from(size.0),
            f64::from((p.y - r.top()) / r.height()) * f64::from(size.1),
        ));
    }
    None
}

fn lens_dialog(ctx: &Context, app: &mut DarkBurn) {
    let mut d = std::mem::take(&mut app.vision.ui.lens);
    let mut closing = false;
    let open = window(ctx, "Calibrate Camera Lens", 400.0, |ui| {
        note(
            ui,
            "Corrects the bending of straight lines near the picture edges. Print a grid of dots or a chessboard, flat on a board, then take 6 to 12 pictures with the board at different tilts and in different parts of the picture. Cover the corners too. Keep the whole pattern in view every time.",
        );
        let mut kind = usize::from(d.checker);
        ui_kit::row(ui, "Pattern", |ui| ui_kit::dropdown(ui, "vision_lens_kind", &mut kind, &["Grid of dots", "Chessboard"]));
        d.checker = kind == 1;
        count_field(ui, if d.checker { "Inner corners across" } else { "Dots across" }, &mut d.cols, 2..=40);
        count_field(ui, if d.checker { "Inner corners down" } else { "Dots down" }, &mut d.rows, 2..=40);
        field(ui, "Spacing", &mut d.spacing, " mm");
        if !d.checker {
            ui_kit::checkbox(ui, "Dark dots on light paper", &mut d.dark);
        }
        let mut lens_kind = usize::from(d.fisheye);
        ui_kit::row(ui, "Lens", |ui| ui_kit::dropdown(ui, "vision_lens_type", &mut lens_kind, &["Standard", "Fisheye"]));
        d.fisheye = lens_kind == 1;
        count_field(ui, "Curve terms", &mut d.num, 1..=4);
        let spec_args = json!({ "kind": if d.checker { "checkerboard" } else { "circle_grid" }, "cols": d.cols, "rows": d.rows, "spacing_mm": d.spacing, "dark_on_light": d.dark });
        ui.horizontal_wrapped(|ui| {
            if button(ui, "Add current picture", "Uses the latest camera picture") {
                app.vision.message = vc::pattern_from(&spec_args).and_then(|s| vc::calib_add_frame(app, &s)).err();
            }
            if button(ui, "Add picture files", "Choose saved pictures of the pattern")
                && let Some(files) = rfd::FileDialog::new().add_filter("Pictures", &["png", "jpg", "jpeg", "bmp"]).pick_files() {
                    let spec = vc::pattern_from(&spec_args);
                    app.vision.message = None;
                    for f in files {
                        let r = std::fs::read(&f).map_err(|e| e.to_string()).and_then(|b| crate::images::decode(&b)).map(|i| i.to_rgba8());
                        match (r, &spec) {
                            (Ok(img), Ok(s)) => {
                                app.vision.raw = Some(img);
                                app.vision.frame_gen += 1;
                                if let Err(e) = vc::calib_add_frame(app, s) {
                                    app.vision.message = Some(format!("{}: {e}", f.display()));
                                }
                            }
                            (Err(e), _) => app.vision.message = Some(format!("{}: {e}", f.display())),
                            (_, Err(e)) => app.vision.message = Some(e.clone()),
                        }
                    }
                }
            if button(ui, "Clear pictures", "") {
                app.vision.calib_frames.clear();
                app.vision.calib_result = None;
            }
        });
        ui_kit::status_line(ui, &format!("{} picture(s) with the pattern found.", app.vision.calib_frames.len()), ui_kit::StatusTone::Dim);
        ui.horizontal(|ui| {
            if ui.add_enabled(app.vision.calib_frames.len() >= 3, egui::Button::new("Calibrate")).clicked() {
                let r = vc::pattern_from(&spec_args).and_then(|s| vc::calibrate_lens(app, &s, d.fisheye, d.num));
                app.vision.message = r.err();
            }
            if button(ui, "Remove calibration", "Go back to the uncorrected picture") {
                vc::clear_lens(app);
            }
            if button(ui, "Close", "") {
                closing = true;
            }
        });
        if let Some(r) = &app.vision.calib_result {
            ui_kit::status_line(ui, r, ui_kit::StatusTone::Dim);
        }
        if let Some(m) = &app.vision.message {
            ui_kit::status_line(ui, m, ui_kit::StatusTone::Error);
        }
    });
    app.vision.ui.lens = d;
    if !open || closing {
        app.vision.message = None;
        close(app);
    }
}

fn align_dialog(ctx: &Context, app: &mut DarkBurn) {
    let mut d = std::mem::take(&mut app.vision.ui.align);
    let tex = view_texture(ctx, app);
    let mut closing = false;
    let open = window(ctx, "Calibrate Camera Alignment", 560.0, |ui| {
        note(
            ui,
            "Step 1: draw the targets, send them to the laser and mark them on the material in the bed. Step 2: take a camera picture, then click the centre of each target in the order they were drawn. Step 3: solve.",
        );
        ui_kit::section_heading(ui, "1. Targets");
        count_field(ui, "Targets", &mut d.count, 4..=9);
        field(ui, "Distance from the edge", &mut d.margin, " mm");
        field(ui, "Target size", &mut d.size, " mm");
        count_field(ui, "Layer", &mut d.layer, 0..=29);
        if button(ui, "Draw targets", "Adds the targets to the design; send them to the laser later") {
            let args = json!({ "count": d.count, "margin": d.margin, "size": d.size, "layer": d.layer });
            d.text = msg(crate::api::execute_headless(app, "camera.align_generate", &args), |v| {
                format!("Drew {} targets.", v["targets"].as_array().map_or(0, Vec::len))
            });
        }
        ui_kit::section_heading(ui, "2. Click the targets");
        ui.horizontal(|ui| {
            ui_kit::checkbox(ui, "Flip left-right", &mut d.flip_x);
            ui_kit::checkbox(ui, "Flip top-bottom", &mut d.flip_y);
            ui.label("Turns");
            ui.add(egui::DragValue::new(&mut d.rotate).range(0..=3));
        });
        match tex {
            Some((id, size)) => {
                if let Some(p) = clickable_picture(ui, id, size, &d.clicks)
                    && d.clicks.len() < d.count {
                        d.clicks.push(p);
                    }
                note(ui, &format!("Clicked {} of {}.", d.clicks.len(), d.count));
            }
            None => note(ui, "There is no camera picture yet. Pick a camera or a picture file in the Camera Control panel."),
        }
        ui.horizontal(|ui| {
            if button(ui, "Undo click", "") {
                d.clicks.pop();
            }
            if button(ui, "Clear clicks", "") {
                d.clicks.clear();
            }
            let enough = d.clicks.len() >= 3;
            if ui.add_enabled(enough, egui::Button::new("Solve")).clicked() {
                app.vision.align_margin = d.margin;
                let o = darkburn_vision::camera::ImageOrientation { flip_x: d.flip_x, flip_y: d.flip_y, rotate_quarters: d.rotate as u8 };
                let size = tex.map(|t| t.1).unwrap_or((0, 0));
                let pos = vc::align_positions(app, d.margin, d.count);
                d.text = msg(vc::align_solve(app, &d.clicks, Some(pos), o, size), |v| {
                    format!("Aligned, error {:.2} mm.", v["rms_mm"].as_f64().unwrap_or(0.0))
                });
            }
            if button(ui, "Remove alignment", "") {
                app.vision.state.camera.alignment = None;
                vc::commit(app, "Clear camera alignment", true);
                d.text = "Alignment removed.".into();
            }
            if button(ui, "Close", "") {
                closing = true;
            }
        });
        result_line(ui, &d.text);
    });
    app.vision.ui.align = d;
    if !open || closing {
        close(app);
    }
}

fn head_dialog(ctx: &Context, app: &mut DarkBurn) {
    let mut d = std::mem::take(&mut app.vision.ui.head);
    let tex = view_texture(ctx, app);
    let mut closing = false;
    let open = window(ctx, "Head Camera Alignment", 560.0, |ui| {
        note(
            ui,
            "For a camera on the laser head. Move the head so a known mark is in view, click the mark in the picture, enter where the head was and where the mark truly is, then add it. Use four or more marks spread across the picture.",
        );
        match tex {
            Some((id, size)) => {
                let shown: Vec<Point> = app.vision.marks.iter().map(|m| m.image).chain(d.pending).collect();
                if let Some(p) = clickable_picture(ui, id, size, &shown) {
                    d.pending = Some(p);
                }
            }
            None => note(ui, "There is no camera picture yet. Pick a camera or a picture file in the Camera Control panel."),
        }
        ui.horizontal(|ui| {
            ui.label("Head at");
            ui.add(egui::DragValue::new(&mut d.head[0]).speed(0.1).prefix("X ").suffix(" mm"));
            ui.add(egui::DragValue::new(&mut d.head[1]).speed(0.1).prefix("Y ").suffix(" mm"));
        });
        ui.horizontal(|ui| {
            ui.label("Mark at");
            ui.add(egui::DragValue::new(&mut d.bed[0]).speed(0.1).prefix("X ").suffix(" mm"));
            ui.add(egui::DragValue::new(&mut d.bed[1]).speed(0.1).prefix("Y ").suffix(" mm"));
        });
        ui.horizontal(|ui| {
            if ui.add_enabled(d.pending.is_some(), egui::Button::new("Add mark")).clicked()
                && let Some(p) = d.pending.take() {
                    app.vision.marks.push(darkburn_vision::camera::HeadMark {
                        image: p,
                        head_pos: Point::new(d.head[0], d.head[1]),
                        bed: Point::new(d.bed[0], d.bed[1]),
                    });
                }
            if button(ui, "Clear marks", "") {
                app.vision.marks.clear();
            }
            if ui.add_enabled(app.vision.marks.len() >= 4, egui::Button::new("Solve")).clicked() {
                let size = tex.map(|t| t.1).unwrap_or((0, 0));
                let marks = app.vision.marks.clone();
                d.text = msg(vc::head_solve(app, marks, size), |v| format!("Aligned, error {:.2} mm.", v["rms_mm"].as_f64().unwrap_or(0.0)));
            }
        });
        note(ui, &format!("{} mark(s).", app.vision.marks.len()));
        ui_kit::section_heading(ui, "Stitching");
        note(ui, "Move the head to different places over the bed, add a capture at each, then stitch them into one picture.");
        ui.horizontal(|ui| {
            if button(ui, "Add capture at head position", "Uses the latest camera picture")
                && let Some(img) = app.vision.raw.clone() {
                    let n = vc::head_add_capture(app, img, Point::new(d.head[0], d.head[1]));
                    d.text = format!("{n} capture(s).");
                }
            if button(ui, "Stitch", "") {
                d.text = msg(vc::head_stitch(app), |v| format!("Stitched {} capture(s).", v["captures"]));
            }
            if button(ui, "Clear captures", "") {
                app.vision.mosaic.clear();
            }
            if button(ui, "Close", "") {
                closing = true;
            }
        });
        result_line(ui, &d.text);
    });
    app.vision.ui.head = d;
    if !open || closing {
        close(app);
    }
}

// ----- The Camera Control panel ----------------------------------------------------------------

#[cfg(feature = "live-camera")]
fn find_cameras(app: &mut DarkBurn) {
    match darkburn_vision::capture::live::list_cameras() {
        Ok(v) => {
            app.vision.message = if v.is_empty() { Some("No camera found.".into()) } else { None };
            app.vision.cameras = v;
        }
        Err(e) => app.vision.message = Some(e.to_string()),
    }
}

#[cfg(not(feature = "live-camera"))]
fn find_cameras(app: &mut DarkBurn) {
    app.vision.message = Some("This build was made without live camera support. You can still use picture files.".into());
}

#[cfg(feature = "live-camera")]
fn start_live(app: &mut DarkBurn, index: u32) {
    match darkburn_vision::capture::live::LiveCamera::open(index) {
        Ok(cam) => {
            app.vision.source = Some(Box::new(cam));
            app.vision.live = true;
            app.vision.message = None;
        }
        Err(e) => app.vision.message = Some(e.to_string()),
    }
}

#[cfg(not(feature = "live-camera"))]
fn start_live(app: &mut DarkBurn, _index: u32) {
    app.vision.message = Some("This build was made without live camera support.".into());
}

fn stop_live(app: &mut DarkBurn) {
    app.vision.live = false;
    app.vision.source = None;
}

/// The Camera Control panel [CAMP-01 to CAMP-07, CAM-01 to CAM-09].
pub fn camera_panel(ui: &mut Ui, app: &mut DarkBurn) {
    vc::sync(app);
    let t = Tokens::get();
    let ctx = ui.ctx().clone();
    if let Some(m) = &app.vision.message {
        ui_kit::status_line(ui, m, ui_kit::StatusTone::Warning);
    }
    // Source
    ui_kit::section_heading(ui, "Camera");
    let mut allowed = app.vision.prefs.live_enabled;
    if vc::live_built()
        && ui_kit::checkbox(ui, "Allow live camera", &mut allowed).changed() {
            app.vision.prefs.live_enabled = allowed;
            if !allowed {
                stop_live(app);
            }
            let _ = vc::save_prefs(app);
        }
    if allowed && vc::live_built() {
        let names: Vec<String> = app.vision.cameras.iter().map(|(_, n)| n.clone()).collect();
        let mut names_ref: Vec<&str> = vec!["No camera"];
        names_ref.extend(names.iter().map(String::as_str));
        let mut pick = app.vision.cameras.iter().position(|(i, _)| Some(*i) == app.vision.state.camera.device_index).map_or(0, |p| p + 1);
        let before = pick;
        ui_kit::row(ui, "Camera", |ui| ui_kit::dropdown(ui, "vision_camera_pick", &mut pick, &names_ref));
        if pick != before {
            let sel = pick.checked_sub(1).and_then(|p| app.vision.cameras.get(p).cloned());
            app.vision.state.camera.device_index = sel.as_ref().map(|s| s.0);
            app.vision.state.camera.camera_name = sel.map(|s| s.1).unwrap_or_default();
            vc::commit(app, "Camera choice", false);
        }
        ui.horizontal_wrapped(|ui| {
            if button(ui, "Find cameras", "List the cameras the system reports") {
                find_cameras(app);
            }
            if app.vision.live {
                if button(ui, "Stop", "") {
                    stop_live(app);
                }
            } else if button(ui, "Start", "Opens the chosen camera") {
                match app.vision.state.camera.device_index {
                    Some(i) => start_live(app, i),
                    None => app.vision.message = Some("Pick a camera first.".into()),
                }
            }
        });
    } else if !vc::live_built() {
        ui.label(RichText::new("No live camera in this build. Use a picture file.").size(t.font_small).color(t.text_dim));
    }
    ui.horizontal_wrapped(|ui| {
        if button(ui, "Open picture file", "Use a saved picture instead of a camera")
            && let Some(p) = rfd::FileDialog::new().add_filter("Pictures", &["png", "jpg", "jpeg", "bmp", "webp"]).pick_file() {
                stop_live(app);
                app.vision.message = vc::load_frame_file(app, &p).err();
            }
    });
    let mut kind = usize::from(app.vision.state.camera.kind == darkburn_vision::camera::CameraKind::HeadMounted);
    let before = kind;
    ui_kit::row(ui, "Type", |ui| ui_kit::dropdown(ui, "vision_camera_kind", &mut kind, &["Overhead", "On the laser head"]));
    if kind != before {
        let _ = crate::api::execute_headless(app, "camera.set", &json!({ "kind": if kind == 1 { "head_mounted" } else { "overhead" } }));
    }
    // Viewfinder
    if let Some(raw) = app.vision.raw.as_ref() {
        let key = app.vision.frame_gen;
        let id = upload(&ctx, &mut app.vision.ui.tex_raw, key, "vision_raw", raw);
        let w = ui.available_width().min(300.0);
        let h = w * raw.height() as f32 / raw.width().max(1) as f32;
        ui.add(egui::Image::new(egui::load::SizedTexture::new(id, vec2(w, h))));
    } else {
        ui.label(RichText::new("No picture yet.").size(t.font_small).color(t.text_dim));
    }
    // Actions
    ui.horizontal_wrapped(|ui| {
        if button(ui, "Update overlay", "Take the latest picture and lay it over the bed") {
            app.vision.message = vc::update_overlay(app).err();
        }
        if button(ui, "Trace", "Open the Trace Image window on the camera picture") {
            match vc::overlay_as_image(app) {
                Ok(_) => {
                    let ctx2 = ctx.clone();
                    app.run(&ctx2, "tools.trace_image");
                }
                Err(e) => app.vision.message = Some(e),
            }
        }
        if button(ui, "Save settings", "Remember these camera settings for new projects") {
            app.vision.message = vc::save_settings(app).err();
        }
        if button(ui, "Save capture", "Save the camera picture to a file") {
            app.run(&ctx, "file.save_capture");
        }
    });
    ui.horizontal_wrapped(|ui| {
        if button(ui, "Import settings", "")
            && let Some(p) = rfd::FileDialog::new().add_filter("Camera settings", &["json"]).pick_file() {
                app.vision.message = crate::api::execute_headless(app, "camera.import_settings", &json!({ "path": p.display().to_string() })).err();
            }
        if button(ui, "Export settings", "")
            && let Some(p) = rfd::FileDialog::new().add_filter("Camera settings", &["json"]).set_file_name("camera-settings.json").save_file() {
                app.vision.message = crate::api::execute_headless(app, "camera.export_settings", &json!({ "path": p.display().to_string() })).err();
            }
    });
    // Overlay controls
    ui_kit::section_heading(ui, "Overlay");
    let mut c = app.vision.state.camera.clone();
    let covers = app.vision.overlay.as_ref().map_or_else(|| vc::bed(app), |o| o.covers);
    ui_kit::checkbox(ui, "Show", &mut c.state.show);
    ui_kit::checkbox(ui, "Colour", &mut c.state.color);
    ui_kit::checkbox(ui, "Crop to page", &mut c.crop_to_page);
    ui_kit::row(ui, "Fade", |ui| {
        ui.add(egui::Slider::new(&mut c.state.fade, 0.0..=0.95).show_value(false));
    });
    let mut wp = c.overlay.width_mm.map_or(100.0, |w| w / covers.width * 100.0);
    let mut hp = c.overlay.height_mm.map_or(100.0, |h| h / covers.height * 100.0);
    field(ui, "Width", &mut wp, " %");
    field(ui, "Height", &mut hp, " %");
    field(ui, "X shift", &mut c.overlay.x_shift_mm, " mm");
    field(ui, "Y shift", &mut c.overlay.y_shift_mm, " mm");
    c.overlay.width_mm = ((wp - 100.0).abs() > 1e-9 && wp > 0.0).then(|| wp / 100.0 * covers.width);
    c.overlay.height_mm = ((hp - 100.0).abs() > 1e-9 && hp > 0.0).then(|| hp / 100.0 * covers.height);
    // Camera controls
    let mut ex = c.exposure.unwrap_or(0.0);
    let mut br = c.brightness.unwrap_or(0.0);
    let (ex0, br0) = (ex, br);
    field(ui, "Exposure", &mut ex, "");
    field(ui, "Brightness", &mut br, "");
    if ex != ex0 {
        c.exposure = Some(ex);
        if let Some(s) = app.vision.source.as_mut() {
            let _ = s.set_exposure(ex);
        }
    }
    if br != br0 {
        c.brightness = Some(br);
        if let Some(s) = app.vision.source.as_mut() {
            let _ = s.set_brightness(br);
        }
    }
    if c != app.vision.state.camera {
        let crop_changed = c.crop_to_page != app.vision.state.camera.crop_to_page;
        app.vision.state.camera = c;
        vc::commit(app, "Camera settings", false);
        if crop_changed && app.vision.raw.is_some() {
            let _ = vc::rebuild_overlay(app);
        }
    }
    // Wizards
    ui_kit::section_heading(ui, "Calibration");
    ui.horizontal_wrapped(|ui| {
        if button(ui, "Lens...", "Calibrate the camera lens") {
            open_dialog(app, Dlg::Lens);
        }
        if button(ui, "Alignment...", "Align the camera picture to the bed") {
            open_dialog(app, Dlg::Align);
        }
        if button(ui, "Head camera...", "Align a camera on the laser head and stitch pictures") {
            open_dialog(app, Dlg::Head);
        }
    });
    let cam = &app.vision.state.camera;
    ui.label(
        RichText::new(format!(
            "Lens {}   Alignment {}",
            if cam.lens.is_some() { "calibrated" } else { "not calibrated" },
            if cam.alignment.is_some() || cam.head.is_some() { "done" } else { "not done" }
        ))
        .size(t.font_small)
        .color(t.text_dim),
    );
}

// ----- On the canvas ---------------------------------------------------------------------------------

/// The camera picture under the design, and the print-and-cut target marks over it.
pub fn paint_overlay(ctx: &Context, app: &mut DarkBurn, xf: &Xf, painter: &egui::Painter) {
    vc::sync(app);
    let v = &mut app.vision;
    if let Some(o) = &v.overlay {
        let cam = &v.state.camera;
        let opacity = cam.state.opacity();
        if opacity > 0.0 {
            let key = (v.overlay_gen << 1) | u64::from(cam.state.color);
            let colour = cam.state.color;
            let id = if v.ui.tex_overlay.as_ref().is_none_or(|(_, k)| *k != key) || colour {
                let shown = if colour { None } else { darkburn_vision::camera::OverlayState { show: true, fade: 0.0, color: false }.apply(&o.img) };
                upload(ctx, &mut v.ui.tex_overlay, key, "vision_overlay", shown.as_ref().unwrap_or(&o.img))
            } else {
                v.ui.tex_overlay.as_ref().map(|(t, _)| t.id()).unwrap_or_default()
            };
            let rect = xf.rect(cam.overlay.placement(o.covers));
            painter.image(id, rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE.gamma_multiply(opacity));
        }
    }
    // Print and cut marks.
    let accent = Tokens::get().accent;
    let pnc = &v.state.pnc;
    for (i, p) in pnc.design.iter().enumerate() {
        if let Some(p) = p {
            cross(painter, xf.to_screen(*p), accent, 7.0);
            painter.text(xf.to_screen(*p) + vec2(8.0, -8.0), Align2::LEFT_BOTTOM, (i + 1).to_string(), FontId::proportional(11.0), accent);
        }
    }
}

fn cross(painter: &egui::Painter, c: egui::Pos2, ink: Color32, arm: f32) {
    let s = Stroke::new(1.5, ink);
    painter.line_segment([c - vec2(arm, 0.0), c + vec2(arm, 0.0)], s);
    painter.line_segment([c - vec2(0.0, arm), c + vec2(0.0, arm)], s);
    painter.circle_stroke(c, arm * 0.7, s);
}

// ----- Status bar ------------------------------------------------------------------------------------

/// Chips for Rotary, Cylinder and Print and Cut at the right of the status bar [ROT-08, PNC-06,
/// LASER-22]. Clicking a chip turns the mode on or off (print and cut: opens its window).
/// Returns the left edge of the chips so the status text can stop before them.
pub fn status_chips(ui: &mut Ui, bar: Rect, app: &mut DarkBurn) -> f32 {
    let t = Tokens::get();
    let font = FontId::proportional(t.font_small);
    let dim = t.app_bar_text.gamma_multiply(0.45);
    let s = app.vision.state.clone();
    let chips = [("Rotary", s.rotary.enabled, "rotary"), ("Cylinder", s.cylinder.enabled, "cylinder"), ("Print and cut", s.pnc.fit.is_some(), "pnc")];
    let mut x = bar.right() - 190.0;
    for (label, on, key) in chips.into_iter().rev() {
        let g = ui.painter().layout_no_wrap(label.to_string(), font.clone(), if on { t.app_bar_text } else { dim });
        let w = g.size().x + 12.0;
        let br = Rect::from_min_size(pos2(x - w, bar.top() + 3.0), vec2(w, bar.height() - 6.0));
        let resp = ui.interact(br, ui.scope_id().with(("vision_chip", key)), Sense::click());
        if on {
            ui.painter().rect(br, t.radius_sm, t.accent_soft, Stroke::new(1.0, t.accent), egui::StrokeKind::Inside);
        } else {
            ui.painter().rect_stroke(br, t.radius_sm, Stroke::new(1.0, dim), egui::StrokeKind::Inside);
        }
        ui.painter().galley(pos2(br.left() + 6.0, br.center().y - g.size().y / 2.0), g, if on { t.selected_ink } else { dim });
        let tip = match key {
            "rotary" => format!("Rotary is {}. Click to turn it {}.", if on { "on" } else { "off" }, if on { "off" } else { "on" }),
            "cylinder" => format!("Cylinder correction is {}. Click to turn it {}.", if on { "on" } else { "off" }, if on { "off" } else { "on" }),
            _ => vc::pnc_status(&s).unwrap_or_else(|| "Print and cut is off. Click to set it up.".into()),
        };
        if resp.hovered() {
            app.hint = Some(tip.clone());
        }
        if resp.backing_on_hover_text(tip).clicked() {
            let r = match key {
                "rotary" => vc::rotary_set(app, &json!({ "enabled": !on })),
                "cylinder" => vc::cylinder_set(app, &json!({ "enabled": !on })),
                _ => {
                    open_dialog(app, Dlg::PrintCut);
                    Ok(Value::Null)
                }
            };
            match r {
                Err(e) => app.note = Some(e),
                Ok(_) => {
                    if key == "rotary" {
                        app.note = None;
                    }
                }
            }
        }
        x = br.left() - 4.0;
    }
    x - 8.0
}
