//! The Quick Nest and True Nest dialog [ARRAY-10]. It builds the same arguments as the
//! registered commands (`nest_cmds.rs`) and runs the packer on a worker thread, with a progress
//! bar. Cancel stops the run and uses the best layout found so far; closing the window while it
//! runs drops the run and moves nothing. The finished layout is one undo step.

use darkburn_ui::ui_kit;
use egui::{Color32, Context, Id, Ui, pos2};
use serde_json::{Value, json};

use crate::i18n::tr;
use crate::nest_cmds::{self, NestJob, QUICK, TRUE};
use crate::numeric::{self, Num};
use crate::shell::DarkBurn;

/// The dialog's settings, kept between runs.
#[derive(Clone, Debug, PartialEq)]
pub struct NestForm {
    /// A typed sheet instead of the work area.
    pub custom: bool,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub gap: f64,
    pub margin: f64,
    /// 0 no rotation, 1 quarter turns, 2 a typed step.
    pub rotation: usize,
    pub step: f64,
    pub holes: bool,
    /// Seconds.
    pub time: f64,
    pub sheets: f64,
    pub spacing: f64,
    pub keep_clear: bool,
}

impl Default for NestForm {
    fn default() -> Self {
        NestForm {
            custom: false,
            x: 0.0,
            y: 0.0,
            w: 300.0,
            h: 200.0,
            gap: 2.0,
            margin: 0.0,
            rotation: 1,
            step: 15.0,
            holes: true,
            time: 10.0,
            sheets: 1.0,
            spacing: 10.0,
            keep_clear: true,
        }
    }
}

impl NestForm {
    /// The arguments of the command `id` for these settings.
    pub fn args(&self, id: &str) -> Value {
        let step = match self.rotation {
            0 => 0.0,
            1 => 90.0,
            _ => self.step,
        };
        let mut a = json!({
            "gap": self.gap,
            "margin": self.margin,
            "rotation_step": step,
            "max_sheets": self.sheets.round(),
            "sheet_spacing": self.spacing,
            "keep_clear": self.keep_clear,
        });
        if id == TRUE {
            a["allow_holes"] = json!(self.holes);
            a["time_limit"] = json!(self.time);
        }
        if self.custom {
            a["x"] = json!(self.x);
            a["y"] = json!(self.y);
            a["width"] = json!(self.w);
            a["height"] = json!(self.h);
        }
        a
    }
}

/// State of the dialog.
#[derive(Default)]
pub struct NestUi {
    /// The command whose dialog is open.
    pub open: Option<&'static str>,
    pub form: NestForm,
    pub job: Option<NestJob>,
    pub error: Option<String>,
    /// What the last run did.
    pub note: Option<String>,
}

impl NestUi {
    fn close(&mut self) {
        self.open = None;
        self.error = None;
        if let Some(j) = self.job.take() {
            // The run ends soon on its own thread; its result is not used.
            j.cancel();
        }
    }
}

/// From a menu or button: open the dialog.
pub fn run(app: &mut DarkBurn, _ctx: &Context, id: &str) -> bool {
    let which = match id {
        QUICK => QUICK,
        TRUE => TRUE,
        _ => return false,
    };
    if app.nest_ui.job.is_none() {
        app.nest_ui.open = Some(which);
        app.nest_ui.error = None;
        app.nest_ui.note = None;
    }
    true
}

fn mm() -> Num {
    Num::new(-100_000.0..=100_000.0, 2, " mm").speed(0.1)
}

fn size() -> Num {
    Num::new(0.0..=100_000.0, 2, " mm").speed(0.1)
}

fn form_ui(ui: &mut Ui, f: &mut NestForm, quick: bool, page: (f64, f64)) {
    ui_kit::checkbox(ui, &tr("nest.custom_sheet", "Custom sheet (else the work area)"), &mut f.custom);
    if f.custom {
        numeric::row(ui, &tr("nest.x", "X"), &mut f.x, &mm());
        numeric::row(ui, &tr("nest.y", "Y"), &mut f.y, &mm());
        numeric::row(ui, &tr("nest.width", "Width"), &mut f.w, &size());
        numeric::row(ui, &tr("nest.height", "Height"), &mut f.h, &size());
    } else {
        ui.label(
            tr("nest.sheet_is_page", "Sheet: the work area, {w} x {h} mm")
                .replace("{w}", &format!("{:.0}", page.0))
                .replace("{h}", &format!("{:.0}", page.1)),
        );
    }
    numeric::row(ui, &tr("nest.gap", "Gap between parts"), &mut f.gap, &size());
    numeric::row(ui, &tr("nest.margin", "Edge margin"), &mut f.margin, &size());
    let options = [tr("nest.rot_none", "No rotation"), tr("nest.rot_quarter", "Quarter turns"), tr("nest.rot_step", "Rotation step")];
    let options: Vec<&str> = options.iter().map(String::as_str).collect();
    ui_kit::row(ui, &tr("nest.rotation", "Rotation"), |ui| ui_kit::dropdown(ui, "nest_rotation", &mut f.rotation, &options));
    if f.rotation == 2 {
        numeric::row(ui, &tr("nest.step", "Step"), &mut f.step, &Num::new(0.0..=360.0, 1, " deg").speed(1.0));
    }
    if !quick {
        ui_kit::checkbox(ui, &tr("nest.holes", "Put small parts inside holes"), &mut f.holes);
        numeric::row(ui, &tr("nest.time", "Time limit"), &mut f.time, &Num::new(0.5..=3600.0, 1, " s").speed(0.5));
    }
    numeric::row(ui, &tr("nest.sheets", "Most sheets"), &mut f.sheets, &Num::new(1.0..=64.0, 0, "").speed(0.2));
    if f.sheets > 1.5 {
        numeric::row(ui, &tr("nest.spacing", "Space between sheets"), &mut f.spacing, &size());
    }
    ui_kit::checkbox(ui, &tr("nest.keep_clear", "Keep clear of other shapes"), &mut f.keep_clear);
    ui.label(tr("nest.stays", "Locked shapes and shapes on layers with output off stay where they are."));
}

fn start(app: &mut DarkBurn, id: &str) -> Result<(), String> {
    let s = nest_cmds::settings(id, &app.nest_ui.form.args(id), nest_cmds::page(app))?;
    let job = nest_cmds::collect(&app.editor.doc, &app.editor.selected(), s, app.editor.revision())?;
    app.nest_ui.job = Some(NestJob::start(job));
    Ok(())
}

fn summary(v: &Value) -> String {
    let n = |k: &str| v[k].as_u64().unwrap_or(0).to_string();
    let mut s = tr("nest.done", "Placed {placed} of {parts} on {sheets} sheet(s), {used}% of the material used.")
        .replace("{placed}", &n("placed"))
        .replace("{parts}", &n("parts"))
        .replace("{sheets}", &n("sheets_used"))
        .replace("{used}", &format!("{:.0}", v["utilization"].as_f64().unwrap_or(0.0) * 100.0));
    if v["stopped"].as_bool() == Some(true) {
        s.push(' ');
        s.push_str(&tr("nest.stopped", "Stopped early: this is the best layout found."));
    }
    s
}

/// Take the finished run and move the shapes.
fn finish(app: &mut DarkBurn) {
    if !app.nest_ui.job.as_ref().is_some_and(NestJob::finished) {
        return;
    }
    let Some(worker) = app.nest_ui.job.take() else { return };
    let (job, result) = worker.take();
    match result.and_then(|r| nest_cmds::apply(&mut app.editor, &job, &r)) {
        Ok(v) => {
            let text = summary(&v);
            app.nest_ui.error = None;
            app.shape_ui.status = Some(text.clone());
            app.nest_ui.note = Some(text);
        }
        Err(e) => app.nest_ui.error = Some(e),
    }
}

/// The dialog, when it is open.
pub fn show(ctx: &Context, app: &mut DarkBurn) {
    let Some(id) = app.nest_ui.open else { return };
    let quick = id == QUICK;
    let title = if quick { tr("nest.title_quick", "Quick Nest") } else { tr("nest.title_true", "True Nest") };
    let page = (app.prefs.page_width as f64, app.prefs.page_height as f64);
    let can = !app.editor.selected().is_empty();
    let running = app.nest_ui.job.as_ref().map(NestJob::progress);
    let stopping = app.nest_ui.job.as_ref().is_some_and(NestJob::cancelled);
    let mut f = app.nest_ui.form.clone();
    let (mut go, mut stop, mut close) = (false, false, false);
    let mut open = true;
    let screen = ctx.content_rect();
    let win = Id::unique("nest_dialog");
    egui::Window::new(title)
        .id(win)
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .default_pos(pos2((screen.right() - 360.0).max(0.0), 120.0))
        .default_width(320.0)
        .show(ctx, |ui| {
            ui.label(if quick {
                tr("nest.about_quick", "Packs the selected shapes by their bounding boxes. Fast, and good for rectangular parts.")
            } else {
                tr("nest.about_true", "Packs the selected shapes by their real outlines, so parts interlock and small parts can go into holes.")
            });
            ui.add_space(4.0);
            ui.add_enabled_ui(running.is_none(), |ui| form_ui(ui, &mut f, quick, page));
            ui.add_space(4.0);
            if let Some((done, total)) = running {
                let frac = if total > 0 { done as f32 / total as f32 } else { 0.0 };
                let text =
                    tr("nest.progress", "Placing {done} of {total}").replace("{done}", &done.to_string()).replace("{total}", &total.to_string());
                ui.add(egui::ProgressBar::new(frac).text(text));
                if stopping {
                    ui.label(tr("nest.stopping", "Stopping..."));
                } else {
                    ui.label(tr("nest.cancel_keeps", "Cancel stops the run and uses the best layout found so far."));
                    if ui.button(tr("nest.cancel", "Cancel")).clicked() {
                        stop = true;
                    }
                }
            } else {
                ui.horizontal(|ui| {
                    if ui.add_enabled(can, egui::Button::new(tr("nest.go", "Nest"))).clicked() {
                        go = true;
                    }
                    if ui.button(tr("nest.close", "Close")).clicked() {
                        close = true;
                    }
                });
            }
            if let Some(e) = &app.nest_ui.error {
                ui.colored_label(Color32::from_rgb(225, 80, 70), e);
            } else if let Some(n) = &app.nest_ui.note {
                ui.label(n);
            } else if !can {
                ui.label(tr("nest.select_first", "Select the shapes to nest first."));
            }
        });
    app.nest_ui.form = f;
    if crate::escape::closes(ctx, win) {
        open = false;
    }
    if !open || close {
        app.nest_ui.close();
        return;
    }
    if stop && let Some(j) = &app.nest_ui.job {
        j.cancel();
    }
    if go {
        app.nest_ui.note = None;
        app.nest_ui.error = start(app, id).err();
    }
    finish(app);
    if app.nest_ui.job.is_some() {
        ctx.request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_harness::Gui;
    use darkburn_core::ShapeId;
    use darkburn_core::kurbo::Rect;

    fn click_button(g: &mut Gui, label: &str) {
        let b = g.find_a11y(Some("Button"), label).last().cloned().unwrap_or_else(|| panic!("no {label} button: {:?}", g.visible_texts()));
        g.click_pos(b.rect.center());
    }

    fn wait(g: &mut Gui) {
        let end = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while g.app().nest_ui.job.is_some() {
            assert!(std::time::Instant::now() < end, "the nest run never ended");
            std::thread::sleep(std::time::Duration::from_millis(5));
            g.frames(1);
        }
        g.frames(2);
    }

    fn add(g: &mut Gui, x: f64, y: f64, w: f64, h: f64) -> ShapeId {
        let v = g.exec("shape.add_rect", json!({ "x": x, "y": y, "width": w, "height": h, "layer": 0 }));
        ShapeId(v["id"].as_u64().unwrap())
    }

    #[test]
    fn dialog_nests_on_a_worker_thread_in_one_undo_step() {
        let mut g = Gui::standard();
        let mut ids = vec![add(&mut g, 250.0, 150.0, 60.0, 40.0)];
        // A frame with a hole, then small parts scattered over the work area.
        let f = g.exec("shape.add_path", json!({ "svg_path": "M20 200 L100 200 L100 280 L20 280 Z M35 215 L35 265 L85 265 L85 215 Z" }));
        ids.push(ShapeId(f["id"].as_u64().unwrap()));
        for i in 0..8 {
            ids.push(add(&mut g, 150.0 + 25.0 * i as f64, 20.0 + 9.0 * i as f64, 18.0, 12.0));
        }
        let tri = g.exec("shape.add_path", json!({ "svg_path": "M300 250 L360 250 L300 290 Z" }));
        ids.push(ShapeId(tri["id"].as_u64().unwrap()));
        g.exec("edit.select_all", json!({}));
        let before: Vec<Rect> = ids.iter().map(|i| g.app().editor.doc.get(*i).unwrap().bbox()).collect();
        g.run_command(TRUE);
        assert_eq!(g.app().nest_ui.open, Some(TRUE));
        assert!(g.window_titles().iter().any(|t| t == "True Nest"), "{:?}", g.window_titles());
        {
            let mut app = g.app();
            app.nest_ui.form.custom = true;
            app.nest_ui.form.w = 150.0;
            app.nest_ui.form.h = 120.0;
            app.nest_ui.form.time = 3.0;
        }
        g.frames(2);
        click_button(&mut g, "Nest");
        wait(&mut g);
        let note = g.app().nest_ui.note.clone();
        assert!(note.as_deref().is_some_and(|n| n.starts_with("Placed 11 of 11")), "{note:?} {:?}", g.app().nest_ui.error);
        let sheet = Rect::new(0.0, 0.0, 150.0, 120.0);
        for i in &ids {
            let b = g.app().editor.doc.get(*i).unwrap().bbox();
            assert!(b.x0 >= -1e-6 && b.y0 >= -1e-6 && b.x1 <= sheet.x1 + 1e-6 && b.y1 <= sheet.y1 + 1e-6, "{b:?}");
        }
        g.screenshot("nest-dialog-true");
        g.exec("edit.undo", json!({}));
        let undone: Vec<Rect> = ids.iter().map(|i| g.app().editor.doc.get(*i).unwrap().bbox()).collect();
        assert_eq!(undone, before, "one undo step");
        g.close_dialogs();
        assert_eq!(g.app().nest_ui.open, None);
    }

    #[test]
    fn cancel_keeps_the_best_so_far_and_closing_drops_the_run() {
        let mut g = Gui::standard();
        for i in 0..150 {
            add(&mut g, (i % 15) as f64 * 22.0, 300.0 + (i / 15) as f64 * 15.0, 9.0 + (i % 4) as f64 * 3.0, 6.0 + (i % 3) as f64 * 2.0);
        }
        g.exec("edit.select_all", json!({}));
        g.run_command(TRUE);
        {
            let mut app = g.app();
            app.nest_ui.form.time = 600.0;
            app.nest_ui.form.rotation = 2;
            app.nest_ui.form.step = 10.0;
        }
        g.frames(1);
        click_button(&mut g, "Nest");
        assert!(g.app().nest_ui.job.is_some(), "the run is under way");
        assert!(g.has_text("Cancel stops the run and uses the best layout found so far."));
        g.screenshot("nest-dialog-running");
        click_button(&mut g, "Cancel");
        wait(&mut g);
        let note = g.app().nest_ui.note.clone().unwrap_or_default();
        assert!(note.contains("Stopped early"), "{note} {:?}", g.app().nest_ui.error);
        assert_eq!(g.app().nest_ui.open, Some(TRUE), "the dialog stays open after a cancelled run");
        // A run whose window is closed moves nothing.
        let rev = g.app().editor.revision();
        g.frames(1);
        click_button(&mut g, "Nest");
        g.escape();
        g.frames(3);
        assert_eq!(g.app().nest_ui.open, None);
        assert!(g.app().nest_ui.job.is_none());
        std::thread::sleep(std::time::Duration::from_millis(50));
        g.frames(2);
        assert_eq!(g.app().editor.revision(), rev);
    }

    #[test]
    fn quick_nest_dialog_has_no_time_limit_and_needs_a_selection() {
        let mut g = Gui::standard();
        g.run_command(QUICK);
        assert!(g.window_titles().iter().any(|t| t == "Quick Nest"), "{:?}", g.window_titles());
        assert!(!g.has_text("Time limit"));
        assert!(g.has_text("Select the shapes to nest first."));
        let a = NestForm::default().args(QUICK);
        assert!(a.get("time_limit").is_none() && a.get("allow_holes").is_none());
        let a = NestForm { rotation: 2, step: 30.0, custom: true, ..NestForm::default() }.args(TRUE);
        assert_eq!(a["rotation_step"], 30.0);
        assert_eq!(a["width"], 300.0);
        g.close_dialogs();
    }
}
