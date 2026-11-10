//! The windows of the job area: the preview (Alt+P), Optimization Settings, G-code and simulation
//! settings, Material Test, the out-of-bounds question, and the Laser panel's job controls.
//! Every button calls a registered command (`job_cmds`); nothing here holds its own logic.

use darkburn_core::kurbo::Point;
use darkburn_job::{JobAnchor, OrderKey, PlannerSettings, ScanDirection};
use darkburn_ui::ui_kit;
use egui::{Color32, Id, Rect, Sense, Shape as EShape, Stroke, Ui, pos2, vec2};
use solidcraft_theme::Tokens;

use crate::job_build::{AdditionalSettings, DeviceGcodeSettings, StartMode, Transfer, format_time};
use crate::job_cmds::{self, ANCHORS, SPEEDS};
use crate::material_test::Axis;
use crate::render::Xf;
use crate::shell::DarkBurn;

/// Everything this area draws or polls each frame.
pub fn frame(ctx: &egui::Context, app: &mut DarkBurn) {
    let dir = app.settings_dir.clone();
    app.job.ensure_loaded(dir.as_deref());
    poll_build(ctx, app);
    build_progress_window(ctx, app);
    preview_window(ctx, app);
    optimization_dialog(ctx, app);
    gcode_dialog(ctx, app);
    material_dialog(ctx, app);
    bounds_prompt(ctx, app);
    message_window(ctx, app);
}

fn poll_build(ctx: &egui::Context, app: &mut DarkBurn) {
    let Some((task, _)) = app.job.pending.as_mut() else { return };
    ctx.request_repaint_after(std::time::Duration::from_millis(30));
    let Some(result) = task.poll().cloned() else { return };
    let Some((_, then)) = app.job.pending.take() else { return };
    app.job.pending_started = None;
    match result {
        Ok(job) => job_cmds::finish(app, job, then),
        Err(crate::job_build::BuildError::Cancelled) => {}
        Err(e) => {
            if matches!(then, job_cmds::After::Preview) {
                app.job.preview.error = Some(e.to_string());
                app.job.preview.data = None;
            } else {
                app.job.message = Some(job_cmds::Message { title: "No job".into(), text: e.to_string() });
            }
        }
    }
}

fn build_progress_window(ctx: &egui::Context, app: &mut DarkBurn) {
    let Some((task, _)) = app.job.pending.as_ref() else { return };
    // Quick jobs finish before this would flash up.
    if app.job.pending_started.is_none_or(|t| t.elapsed().as_millis() < 300) {
        return;
    }
    let fraction = task.progress();
    let (_, action) =
        ui_kit::Window::new(Id::unique("db_job_progress"), "db_app", "Preparing the job").actions("", "Cancel", None).show(ctx, true, |ui| {
            ui.label("Planning the cut order. Cancel stops it.");
            ui_kit::progress(ui, "job_build", fraction);
        });
    if action == ui_kit::Action::Cancel {
        task.cancel();
    }
}

// ----- Preview ------------------------------------------------------------------------------

fn clock(t: f64) -> String {
    format_time(t)
}

fn preview_window(ctx: &egui::Context, app: &mut DarkBurn) {
    if !app.job.preview.open {
        return;
    }
    let dt = ctx.input(|i| i.stable_dt).min(0.1) as f64;
    {
        let p = &mut app.job.preview;
        if p.playing
            && let Some(d) = &p.data
        {
            p.t += dt * SPEEDS[p.speed_index.min(SPEEDS.len() - 1)];
            if p.t >= d.stats.seconds {
                p.t = d.stats.seconds;
                p.playing = false;
            }
            ctx.request_repaint();
        }
    }
    let mut window = egui::Window::new("Preview").id(Id::unique("db_preview_window")).collapsible(false).resizable(true);
    let screen = ctx.content_rect();
    window = if app.job.preview.maximised {
        window.fixed_pos(screen.min + vec2(8.0, 8.0)).fixed_size(screen.size() - vec2(16.0, 16.0))
    } else {
        window.default_size([760.0, 600.0]).default_pos(pos2(screen.left() + 80.0, screen.top() + 60.0))
    };
    let mut open = true;
    let mut command: Option<(&'static str, serde_json::Value)> = None;
    window.open(&mut open).show(ctx, |ui| preview_body(ui, app, &mut command));
    if !open {
        app.job.preview.open = false;
        app.job.preview.playing = false;
    }
    if let Some((id, args)) = command {
        run_command(ctx, app, id, &args);
    }
}

/// Run a command from a button; errors show in a message window.
fn run_command(ctx: &egui::Context, app: &mut DarkBurn, id: &str, args: &serde_json::Value) {
    if crate::commands::hardware(id) {
        app.run(ctx, id);
        return;
    }
    if let Some(r) = job_cmds::execute(app, id, args)
        && let Err(e) = r
    {
        app.job.message = Some(job_cmds::Message { title: "Cannot do that".into(), text: e });
    }
}

/// What the Preview's "from here" buttons can do on a machine family [PREV-09]: (run, send).
/// A G-code machine runs a job as it streams it, a DSP controller can also store it, and a
/// galvo job is made whole from the shapes. Save from here always works.
pub fn from_here_actions(family: crate::machine_api::Family) -> (bool, bool) {
    use crate::machine_api::Family;
    match family {
        Family::Grbl => (true, false),
        Family::Ruida => (true, true),
        Family::Galvo | Family::DesignOnly | Family::FileOnly => (false, false),
    }
}
fn preview_body(ui: &mut Ui, app: &mut DarkBurn, command: &mut Option<(&'static str, serde_json::Value)>) {
    let t = Tokens::get();
    let pending = app.job.pending.as_ref().map(|(task, _)| task.progress());
    let revision = app.editor.revision();
    let bed = (f64::from(app.prefs.page_width), f64::from(app.prefs.page_height));
    let start_op = app.job.start_op;
    let mut refresh = false;
    let mut save_image = false;
    let mut close = false;
    {
        let p = &mut app.job.preview;
        ui.horizontal(|ui| {
            let total = p.data.as_ref().map_or(0.0, |d| d.stats.seconds);
            let label = if p.playing { "Pause" } else { "Play" };
            if ui_kit::button(ui, label, ui_kit::Button::Small).clicked() {
                if !p.playing && p.t >= total {
                    p.t = 0.0;
                }
                p.playing = !p.playing;
            }
            if ui_kit::button(ui, "Restart", ui_kit::Button::Small).clicked() {
                p.t = 0.0;
                p.playing = true;
            }
            ui.spacing_mut().slider_width = (ui.available_width() - 150.0).max(80.0);
            let mut tt = p.t;
            if ui.add(egui::Slider::new(&mut tt, 0.0..=total.max(0.001)).show_value(false)).changed() {
                p.t = tt;
                p.playing = false;
            }
            ui.label(format!("{} / {}", clock(p.t), clock(total)));
        });
        ui.horizontal(|ui| {
            let mut i = p.speed_index;
            let names: Vec<String> = SPEEDS.iter().map(|s| format!("{s}x")).collect();
            let refs: Vec<&str> = names.iter().map(String::as_str).collect();
            ui.label("Speed");
            ui_kit::dropdown(ui, "preview_speed", &mut i, &refs);
            p.speed_index = i;
            ui_kit::checkbox(ui, "Shade by power", &mut p.view.shade_by_power);
            ui_kit::checkbox(ui, "Show travel moves", &mut p.view.show_travel);
            ui_kit::checkbox(ui, "Invert", &mut p.view.invert);
        });
        if let Some(d) = p.data.clone() {
            let s = d.stats;
            ui.label(
                egui::RichText::new(format!(
                    "Cut {:.0} mm   Scan {:.0} mm   Travel {:.0} mm   Estimated time {}",
                    s.cut_mm,
                    s.scan_mm,
                    s.travel_mm,
                    format_time(s.seconds)
                ))
                .size(t.font_label),
            );
            let st = d.sim.state_at(p.t);
            ui.label(
                egui::RichText::new(format!("{}   Operation {}", d.readout(p.t), st.map_or(0, |s| s.op_index))).size(t.font_label).color(t.text_dim),
            );
            if d.revision != revision {
                ui_kit::status_line(ui, "The drawing changed since this preview. Refresh to see the new job.", ui_kit::StatusTone::Warning);
            }
            for w in &d.job.warnings {
                ui_kit::status_line(ui, w, ui_kit::StatusTone::Warning);
            }
        } else if let Some(frac) = pending {
            ui_kit::status_line(ui, "Planning the job...", ui_kit::StatusTone::Dim);
            ui_kit::progress(ui, "preview_build", frac);
        } else if let Some(e) = &p.error {
            ui_kit::status_line(ui, e, ui_kit::StatusTone::Error);
        }
    }

    // The picture.
    let avail = ui.available_size();
    let (rect, response) = ui.allocate_exact_size(vec2(avail.x.max(100.0), (avail.y - 40.0).max(120.0)), Sense::click_and_drag());
    draw_preview(ui, app, rect, &response, bed);

    ui.horizontal_wrapped(|ui| {
        let have = app.job.preview.data.is_some();
        let op = app.job.preview.data.as_ref().and_then(|d| d.sim.state_at(app.job.preview.t)).map_or(0, |s| s.op_index);
        let b = |ui: &mut Ui, label: &str, on: bool| ui.add_enabled_ui(on, |ui| ui_kit::button(ui, label, ui_kit::Button::Small)).inner.clicked();
        if b(ui, "Start here", have) {
            *command = Some(("job.preview_start_here", serde_json::json!({ "op_index": op })));
        }
        let family = app.machine.active().kind.family();
        let (can_run, can_send) = from_here_actions(family);
        if b(ui, "Run from here", have && can_run) {
            *command = Some(("job.preview_run_from_here", serde_json::json!({ "op_index": op })));
        }
        if b(ui, "Send from here", have && can_send) {
            *command = Some(("job.preview_send_from_here", serde_json::json!({ "op_index": op })));
        }
        if b(ui, "Save from here", have)
            && let Some(path) = rfd::FileDialog::new().add_filter("G-code", crate::job_build::GCODE_EXTENSIONS).set_file_name("job.gc").save_file()
        {
            *command = Some(("job.preview_save_from_here", serde_json::json!({ "path": path.to_string_lossy(), "op_index": op })));
        }
        if let Some(s) = start_op {
            ui.label(egui::RichText::new(format!("Starts at operation {s}")).color(t.text_dim));
        }
        if b(ui, "Save image", have) {
            save_image = true;
        }
        if b(ui, "Fit", have) {
            app.job.preview.fitted = false;
        }
        let max = if app.job.preview.maximised { "Restore" } else { "Maximise" };
        if b(ui, max, true) {
            app.job.preview.maximised = !app.job.preview.maximised;
        }
        if b(ui, "Refresh", pending.is_none()) {
            refresh = true;
        }
        if b(ui, "Close", true) {
            close = true;
        }
    });
    if save_image {
        let shift = ui.input(|i| i.modifiers.shift);
        if let Some(path) = rfd::FileDialog::new().add_filter("PNG image", &["png"]).set_file_name("preview.png").save_file() {
            let v = app.job.preview.view;
            *command = Some((
                "job.preview_save_image",
                serde_json::json!({
                    "path": path.to_string_lossy(), "transparent": shift, "shade_by_power": v.shade_by_power, "show_travel": v.show_travel, "invert": v.invert
                }),
            ));
        }
    }
    if refresh {
        job_cmds::open_preview(app);
    }
    if close {
        app.job.preview.open = false;
        app.job.preview.playing = false;
    }
}

fn draw_preview(ui: &mut Ui, app: &mut DarkBurn, rect: Rect, response: &egui::Response, bed: (f64, f64)) {
    let start_op = app.job.start_op;
    let p = &mut app.job.preview;
    let view = p.view;
    let t = Tokens::get();
    if !p.fitted {
        let z = (rect.width() as f64 / bed.0).min(rect.height() as f64 / bed.1) * 0.94;
        p.zoom = z as f32;
        p.pan = vec2((rect.width() - (bed.0 * z) as f32) / 2.0, (rect.height() - (bed.1 * z) as f32) / 2.0);
        p.fitted = true;
    }
    if response.dragged() {
        p.pan += response.drag_delta();
    }
    if let Some(at) = response.hover_pos() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            let k = (scroll * 0.002).exp();
            let new = (p.zoom * k).clamp(0.05, 500.0);
            // Keep the point under the pointer still.
            let local = at - rect.min - p.pan;
            p.pan = at - rect.min - local * (new / p.zoom);
            p.zoom = new;
        }
    }
    let (zoom, pan) = (p.zoom, p.pan);
    let origin = rect.min + pan;
    let to_screen = |q: Point| pos2(origin.x + q.x as f32 * zoom, origin.y + (bed.1 - q.y) as f32 * zoom);
    let painter = ui.painter().with_clip_rect(rect);
    let bg = view.background();
    let surround = if view.invert { Color32::from_rgb(8, 8, 10) } else { t.viewport_bottom };
    painter.rect_filled(rect, 0.0, surround);
    let bed_rect = Rect::from_min_max(to_screen(Point::new(0.0, bed.1)), to_screen(Point::new(bed.0, 0.0)));
    painter.rect_filled(bed_rect, 0.0, Color32::from_rgb(bg[0], bg[1], bg[2]));
    painter.rect_stroke(bed_rect, 0.0, Stroke::new(1.0, t.border), egui::StrokeKind::Outside);
    let Some(data) = p.data.clone() else { return };
    let n = data.started(p.t);
    let mut shapes: Vec<EShape> = Vec::with_capacity(n.min(200_000));
    let width = (0.18 * zoom).clamp(1.0, 4.0);
    for (i, s) in data.segs[..n].iter().enumerate() {
        let Some(c) = view.colour_of(s) else { continue };
        let (a, mut b) = (to_screen(s.a), to_screen(s.b));
        if i + 1 == n && s.dur > 0.0 {
            // The piece being drawn right now, partly.
            let f = ((p.t - s.t0) / s.dur).clamp(0.0, 1.0) as f32;
            b = a + (b - a) * f;
        }
        if !rect.intersects(Rect::from_two_pos(a, b).expand(2.0)) {
            continue;
        }
        shapes.push(EShape::line_segment([a, b], Stroke::new(width, Color32::from_rgb(c[0], c[1], c[2]))));
    }
    painter.extend(shapes);
    if let Some(h) = crate::preview::head_at(&data, p.t) {
        let c = to_screen(h);
        painter.circle_stroke(c, 5.0, Stroke::new(1.5, Color32::from_rgb(230, 60, 50)));
        painter.circle_filled(c, 2.0, Color32::from_rgb(230, 60, 50));
    }
    if let Some(op) = start_op
        && let Some(seg) = data.segs.iter().find(|s| s.op >= op)
    {
        let c = to_screen(seg.a);
        painter.circle_filled(c, 4.0, Color32::from_rgb(40, 170, 90));
    }
}

// ----- Optimization Settings ---------------------------------------------------------------------

const DIRECTIONS: [&str; 5] = ["Off", "Bottom to top", "Top to bottom", "Left to right", "Right to left"];

fn direction_index(d: Option<ScanDirection>) -> usize {
    match d {
        None => 0,
        Some(ScanDirection::BottomToTop) => 1,
        Some(ScanDirection::TopToBottom) => 2,
        Some(ScanDirection::LeftToRight) => 3,
        Some(ScanDirection::RightToLeft) => 4,
    }
}

fn direction_from(i: usize) -> Option<ScanDirection> {
    [None, Some(ScanDirection::BottomToTop), Some(ScanDirection::TopToBottom), Some(ScanDirection::LeftToRight), Some(ScanDirection::RightToLeft)]
        [i.min(4)]
}

fn key_label(k: OrderKey) -> &'static str {
    match k {
        OrderKey::Layer => "Layer",
        OrderKey::Group => "Group",
        OrderKey::Priority => "Priority",
    }
}

/// The optimization controls, shared by the dialog and any later place that shows them.
pub fn planner_editor(ui: &mut Ui, p: &mut PlannerSettings) {
    ui_kit::checkbox(ui, "Optimize cut path", &mut p.optimize);
    ui.add_enabled_ui(p.optimize, |ui| {
        ui_kit::status_line(ui, "Order by (checked keys sort the cuts, first one first)", ui_kit::StatusTone::Dim);
        let mut all: Vec<(OrderKey, bool)> = p
            .order_by
            .iter()
            .map(|k| (*k, true))
            .chain([OrderKey::Layer, OrderKey::Group, OrderKey::Priority].into_iter().filter(|k| !p.order_by.contains(k)).map(|k| (k, false)))
            .collect();
        let mut swap: Option<(usize, usize)> = None;
        for i in 0..all.len() {
            ui.horizontal(|ui| {
                ui_kit::checkbox(ui, key_label(all[i].0), &mut all[i].1);
                if i > 0 && ui_kit::button(ui, ui_kit::MOVE_UP, ui_kit::Button::Small).clicked() {
                    swap = Some((i, i - 1));
                }
                if i + 1 < all.len() && ui_kit::button(ui, ui_kit::MOVE_DOWN, ui_kit::Button::Small).clicked() {
                    swap = Some((i, i + 1));
                }
            });
        }
        if let Some((a, b)) = swap {
            all.swap(a, b);
        }
        p.order_by = all.iter().filter(|(_, on)| *on).map(|(k, _)| *k).collect();
        ui_kit::checkbox(ui, "Cut inner shapes first", &mut p.inner_first);
        let mut d = direction_index(p.direction_order);
        ui_kit::row(ui, "Cut in direction order", |ui| {
            ui_kit::dropdown(ui, "opt_direction", &mut d, &DIRECTIONS);
        });
        p.direction_order = direction_from(d);
        ui_kit::checkbox(ui, "Reduce travel moves", &mut p.reduce_travel);
        ui_kit::checkbox(ui, "Hide backlash", &mut p.hide_backlash);
        if p.hide_backlash {
            ui_kit::row(ui, "Approach distance", |ui| {
                ui_kit::number_field(ui, &mut p.backlash_approach, 0.0..=50.0, 0.1, " mm");
            });
        }
        ui_kit::checkbox(ui, "Reduce direction changes", &mut p.reduce_direction_changes);
        ui_kit::checkbox(ui, "Choose best starting point", &mut p.best_start);
        ui.add_enabled_ui(p.best_start, |ui| {
            ui_kit::checkbox(ui, "Choose corners if possible", &mut p.prefer_corners);
        });
        ui_kit::checkbox(ui, "Choose best direction", &mut p.best_direction);
        ui_kit::checkbox(ui, "Remove overlapping lines", &mut p.remove_overlaps);
        if p.remove_overlaps {
            ui_kit::row(ui, "Overlap distance", |ui| {
                ui_kit::number_field(ui, &mut p.overlap_distance, 0.0..=10.0, 0.01, " mm");
            });
        }
    });
}

fn optimization_dialog(ctx: &egui::Context, app: &mut DarkBurn) {
    if !app.job.optimization_open {
        return;
    }
    let mut draft = app.job.optimization_draft.take().unwrap_or_else(|| job_cmds::current_settings(app).planner);
    let mut set_default = false;
    let mut reset = false;
    let (closed, action) = ui_kit::Window::new(Id::unique("db_job_optimization"), "settings", "Optimization Settings")
        .fields()
        .actions("OK", "Cancel", None)
        .show(ctx, true, |ui| {
            planner_editor(ui, &mut draft);
            ui.add_space(Tokens::get().space_6);
            ui.horizontal(|ui| {
                set_default = ui_kit::button(ui, "Set as default", ui_kit::Button::Small).clicked();
                reset = ui_kit::button(ui, "Reset", ui_kit::Button::Small).clicked();
            });
            ui_kit::status_line(
                ui,
                "The settings are saved in the project. Set as default makes new projects start with them; Reset goes back to the default.",
                ui_kit::StatusTone::Dim,
            );
        });
    if set_default {
        app.job.default_planner = draft.clone();
        app.job.save(app.settings_dir.as_deref());
    }
    if reset {
        draft = app.job.default_planner.clone();
    }
    match action {
        ui_kit::Action::Ok => {
            job_cmds::update_settings(app, "Optimization settings", |s| s.planner = draft);
            app.job.optimization_open = false;
        }
        ui_kit::Action::Cancel => app.job.optimization_open = false,
        _ if closed => app.job.optimization_open = false,
        _ => app.job.optimization_draft = Some(draft),
    }
}

// ----- G-code and simulation settings (reusable editors for the Devices dialog) -------------------

pub const FLAVORS: [&str; 6] = ["GRBL", "grblHAL", "FluidNC", "Marlin", "Smoothieware", "LinuxCNC"];

/// G-code device settings, DS-09 to DS-15.
pub fn gcode_settings_editor(ui: &mut Ui, g: &mut DeviceGcodeSettings) {
    let mut f = g.flavor_index();
    ui_kit::row(ui, "G-code flavor", |ui| {
        ui_kit::dropdown(ui, "gcode_flavor", &mut f, &FLAVORS);
    });
    if f != g.flavor_index() {
        g.set_flavor_index(f);
    }
    ui_kit::row(ui, "S-value max", |ui| {
        ui_kit::number_field(ui, &mut g.s_max, 1.0..=1_000_000.0, 1.0, "");
    });
    let mut air = usize::from(matches!(g.air, darkburn_job::gcode::AirCommand::M7));
    ui_kit::row(ui, "Air assist command", |ui| {
        ui_kit::dropdown(ui, "gcode_air", &mut air, &["M8 (flood)", "M7 (mist)"]);
    });
    g.air = if air == 1 { darkburn_job::gcode::AirCommand::M7 } else { darkburn_job::gcode::AirCommand::M8 };
    ui_kit::row(ui, "Baud rate", |ui| {
        ui_kit::number_field(ui, &mut g.baud_rate, 300..=2_000_000, 100.0, "");
    });
    let mut t = usize::from(g.transfer == Transfer::Synchronous);
    ui_kit::row(ui, "Transfer mode", |ui| {
        ui_kit::dropdown(ui, "gcode_transfer", &mut t, &["Buffered", "Synchronous"]);
    });
    g.transfer = if t == 1 { Transfer::Synchronous } else { Transfer::Buffered };
    ui_kit::checkbox(ui, "Auto-home on start", &mut g.auto_home);
    ui_kit::checkbox(ui, "Swap X and Y", &mut g.swap_xy);
    ui_kit::checkbox(ui, "Fast whitespace scan", &mut g.fast_whitespace_scan);
    if g.fast_whitespace_scan {
        ui_kit::row(ui, "Whitespace scan speed", |ui| {
            ui_kit::number_field(ui, &mut g.fast_whitespace_speed, 1.0..=1000.0, 1.0, " mm/s");
        });
    }
    ui_kit::checkbox(ui, "Use $J jogging", &mut g.jog_with_dollar_j);
    ui_kit::checkbox(ui, "Enable DTR", &mut g.dtr);
    ui_kit::checkbox(ui, "Enable laser fire button", &mut g.fire_button);
    ui_kit::checkbox(ui, "Laser on when framing", &mut g.laser_on_when_framing);
    ui_kit::checkbox(ui, "Use G0 for overscan", &mut g.overscan_as_g0);
    ui_kit::checkbox(ui, "Write S with every G1", &mut g.s_every_g1);
    ui_kit::checkbox(ui, "Return to the start when the job ends", &mut g.return_to_finish);
    ui_kit::text_area(ui, "Start G-code", Id::unique("gcode_start_script"), &mut g.start_script, 3, 10_000);
    ui_kit::text_area(ui, "End G-code", Id::unique("gcode_end_script"), &mut g.end_script, 3, 10_000);
}

/// Acceleration and speed numbers for the time estimate, DS-19.
pub fn additional_settings_editor(ui: &mut Ui, a: &mut AdditionalSettings) {
    ui_kit::status_line(ui, "These numbers make the preview's time estimate match the machine.", ui_kit::StatusTone::Dim);
    let row = |ui: &mut Ui, label: &str, v: &mut f64, suffix: &str| {
        ui_kit::row(ui, label, |ui| {
            ui_kit::number_field(ui, v, 1.0..=1_000_000.0, 1.0, suffix);
        });
    };
    row(ui, "Cutting acceleration", &mut a.accel, " mm/s²");
    row(ui, "Engraving acceleration", &mut a.engrave_accel, " mm/s²");
    row(ui, "Rapid speed", &mut a.rapid_speed, " mm/s");
    row(ui, "Maximum speed", &mut a.max_speed, " mm/s");
    row(ui, "Z speed", &mut a.z_speed, " mm/s");
    ui_kit::row(ui, "Junction deviation", |ui| {
        ui_kit::number_field(ui, &mut a.junction_deviation, 0.0001..=10.0, 0.001, " mm");
    });
}

fn gcode_dialog(ctx: &egui::Context, app: &mut DarkBurn) {
    if !app.job.gcode_open {
        return;
    }
    let (mut g, mut a) = app.job.gcode_draft.take().unwrap_or_else(|| (app.job.gcode.clone(), app.job.additional));
    let (closed, action) = ui_kit::Window::new(Id::unique("db_job_gcode"), "settings", "G-code and Simulation Settings")
        .fields()
        .cards(3.2)
        .actions("OK", "Cancel", None)
        .show(ctx, true, |ui| {
            ui_kit::status_line(ui, "G-code device settings", ui_kit::StatusTone::Dim);
            gcode_settings_editor(ui, &mut g);
            ui.separator();
            ui_kit::status_line(ui, "Simulation (Additional Settings)", ui_kit::StatusTone::Dim);
            additional_settings_editor(ui, &mut a);
        });
    match action {
        ui_kit::Action::Ok => {
            app.job.gcode = g;
            app.job.additional = a;
            app.job.save(app.settings_dir.as_deref());
            app.job.gcode_open = false;
        }
        ui_kit::Action::Cancel => app.job.gcode_open = false,
        _ if closed => app.job.gcode_open = false,
        _ => app.job.gcode_draft = Some((g, a)),
    }
}

// ----- Material test ---------------------------------------------------------------------------

fn axis_index(a: Axis) -> usize {
    Axis::ALL.iter().position(|x| *x == a).unwrap_or(0)
}

fn material_dialog(ctx: &egui::Context, app: &mut DarkBurn) {
    if !app.job.material_open {
        return;
    }
    let mut p = app.job.material.clone();
    let problem = crate::material_test::validate(&p).err();
    let mut preset_action: Option<&'static str> = None;
    let mut preset_pick: Option<usize> = None;
    let names: Vec<String> = app.job.presets.iter().map(|q| q.name.clone()).collect();
    let mut name = app.job.preset_name.clone();
    let (closed, action) = ui_kit::Window::new(Id::unique("db_job_material"), "db_material_test", "Material Test")
        .fields()
        .cards(3.4)
        .actions("Generate", "Close", None)
        .show(ctx, problem.is_none(), |ui| {
            let labels: Vec<&str> = Axis::ALL.iter().map(|a| a.label()).collect();
            for (title, axis, min, max, count, id) in [
                ("Columns", &mut p.x_axis, &mut p.x_min, &mut p.x_max, &mut p.columns, "mt_x"),
                ("Rows", &mut p.y_axis, &mut p.y_min, &mut p.y_max, &mut p.rows, "mt_y"),
            ] {
                ui_kit::status_line(ui, title, ui_kit::StatusTone::Dim);
                let mut i = axis_index(*axis);
                ui_kit::row(ui, "Varies", |ui| {
                    ui_kit::dropdown(ui, id, &mut i, &labels);
                });
                *axis = Axis::ALL[i];
                ui_kit::row(ui, "From", |ui| {
                    ui_kit::number_field(ui, min, -1_000_000.0..=1_000_000.0, 0.1, "");
                });
                ui_kit::row(ui, "To", |ui| {
                    ui_kit::number_field(ui, max, -1_000_000.0..=1_000_000.0, 0.1, "");
                });
                ui_kit::row(ui, "Count", |ui| {
                    ui_kit::number_field(ui, count, 1..=30, 1.0, "");
                });
            }
            ui.separator();
            ui_kit::row(ui, "Cell width", |ui| {
                ui_kit::number_field(ui, &mut p.cell_w, 0.5..=500.0, 0.1, " mm");
            });
            ui_kit::row(ui, "Cell height", |ui| {
                ui_kit::number_field(ui, &mut p.cell_h, 0.5..=500.0, 0.1, " mm");
            });
            ui_kit::row(ui, "Gap", |ui| {
                ui_kit::number_field(ui, &mut p.gap, 0.0..=100.0, 0.1, " mm");
            });
            ui_kit::row(ui, "Left", |ui| {
                ui_kit::number_field(ui, &mut p.x, -10_000.0..=10_000.0, 0.5, " mm");
            });
            ui_kit::row(ui, "Bottom", |ui| {
                ui_kit::number_field(ui, &mut p.y, -10_000.0..=10_000.0, 0.5, " mm");
            });
            ui_kit::checkbox(ui, "Fill the cells (otherwise outlines)", &mut p.fill);
            ui_kit::row(ui, "Passes", |ui| {
                ui_kit::number_field(ui, &mut p.passes, 1..=100, 1.0, "");
            });
            ui_kit::row(ui, "Base speed", |ui| {
                ui_kit::number_field(ui, &mut p.base_speed, 0.1..=100_000.0, 1.0, " mm/s");
            });
            ui_kit::row(ui, "Base power", |ui| {
                ui_kit::number_field(ui, &mut p.base_power, 0.0..=100.0, 0.5, " %");
            });
            ui_kit::row(ui, "Base interval", |ui| {
                ui_kit::number_field(ui, &mut p.base_interval, 0.001..=10.0, 0.005, " mm");
            });
            ui.separator();
            ui_kit::checkbox(ui, "Label the rows and columns", &mut p.labels);
            if p.labels {
                ui_kit::row(ui, "Label height", |ui| {
                    ui_kit::number_field(ui, &mut p.label_height, 0.5..=50.0, 0.1, " mm");
                });
                ui_kit::row(ui, "Label layer", |ui| {
                    ui_kit::number_field(ui, &mut p.label_layer, 0..=29, 1.0, "");
                });
                ui_kit::row(ui, "Label speed", |ui| {
                    ui_kit::number_field(ui, &mut p.label_speed, 0.1..=100_000.0, 1.0, " mm/s");
                });
                ui_kit::row(ui, "Label power", |ui| {
                    ui_kit::number_field(ui, &mut p.label_power, 0.0..=100.0, 0.5, " %");
                });
            }
            if let Some(e) = &problem {
                ui_kit::status_line(ui, e, ui_kit::StatusTone::Error);
            } else {
                ui_kit::status_line(ui, &format!("{} cells, each on its own layer.", p.rows * p.columns), ui_kit::StatusTone::Dim);
            }
            ui.separator();
            ui_kit::status_line(ui, "Presets", ui_kit::StatusTone::Dim);
            ui_kit::row(ui, "Name", |ui| {
                ui_kit::field(ui, &mut name);
            });
            ui.horizontal_wrapped(|ui| {
                for (label, key) in [("Save preset", "save"), ("Import", "import"), ("Export", "export"), ("Frame", "frame")] {
                    if ui_kit::button(ui, label, ui_kit::Button::Small).clicked() {
                        preset_action = Some(key);
                    }
                }
            });
            for (i, n) in names.iter().enumerate() {
                if ui_kit::choice_chip(ui, &format!("mt_preset_{i}"), n, false).clicked() {
                    preset_pick = Some(i);
                }
            }
        });
    app.job.material = p;
    app.job.preset_name = name.clone();
    if let Some(i) = preset_pick {
        let n = app.job.presets[i].name.clone();
        app.job.preset_name = n.clone();
        run_command(ctx, app, "job.material_test_presets", &serde_json::json!({ "action": "load", "name": n }));
    }
    match preset_action {
        Some("save") => run_command(ctx, app, "job.material_test_presets", &serde_json::json!({ "action": "save", "name": name })),
        Some("import") => {
            if let Some(path) = rfd::FileDialog::new().add_filter("Material test presets", &["json"]).pick_file() {
                run_command(ctx, app, "job.material_test_presets", &serde_json::json!({ "action": "import", "path": path.to_string_lossy() }));
            }
        }
        Some("export") => {
            if let Some(path) =
                rfd::FileDialog::new().add_filter("Material test presets", &["json"]).set_file_name("material-test-presets.json").save_file()
            {
                run_command(ctx, app, "job.material_test_presets", &serde_json::json!({ "action": "export", "path": path.to_string_lossy() }));
            }
        }
        Some("frame") => {
            if let Err(e) = crate::material_test::frame_test(app) {
                app.job.message = Some(job_cmds::Message { title: "Material test".into(), text: e });
            }
        }
        _ => {}
    }
    match action {
        ui_kit::Action::Ok => {
            let p = app.job.material.clone();
            match job_cmds::add_material_test(app, &p) {
                Ok(r) => {
                    app.view_cmd = Some(crate::canvas::ViewCmd::Frame(r.bounds));
                    app.job.material_open = false;
                }
                Err(e) => app.job.message = Some(job_cmds::Message { title: "Material test".into(), text: e }),
            }
        }
        ui_kit::Action::Cancel => app.job.material_open = false,
        _ if closed => app.job.material_open = false,
        _ => {}
    }
}

// ----- Questions and messages ---------------------------------------------------------------------

fn bounds_prompt(ctx: &egui::Context, app: &mut DarkBurn) {
    let Some(prompt) = &app.job.bounds_prompt else { return };
    let violations = prompt.job.bounds_report.violations.len();
    let burning = prompt.job.bounds_report.violations.iter().filter(|v| v.burning).count();
    let (_, action) = ui_kit::Window::new(Id::unique("db_job_bounds"), "db_app", "The job might be out of bounds")
        .actions("Continue anyway", "Cancel", Some("Discard out-of-bounds shapes"))
        .show(ctx, true, |ui| {
            ui.label(format!("{violations} moves leave the work area, {burning} of them with the laser on."));
            ui.label("Discard out-of-bounds shapes leaves out every shape that is not fully inside the work area.");
        });
    match action {
        ui_kit::Action::Ok => job_cmds::answer_bounds_prompt(app, true, false),
        ui_kit::Action::Apply => job_cmds::answer_bounds_prompt(app, false, true),
        ui_kit::Action::Cancel => job_cmds::answer_bounds_prompt(app, false, false),
        ui_kit::Action::None => {}
    }
}

fn message_window(ctx: &egui::Context, app: &mut DarkBurn) {
    let Some(m) = app.job.message.clone() else { return };
    let (closed, action) = ui_kit::Window::new(Id::unique("db_job_message"), "db_app", &m.title).actions("", "Close", None).show(ctx, true, |ui| {
        ui.label(&m.text);
    });
    if closed || action != ui_kit::Action::None {
        app.job.message = None;
    }
}

// ----- The Laser panel ---------------------------------------------------------------------------

/// The 3 by 3 job origin picker. Returns the anchor clicked.
pub fn origin_picker(ui: &mut Ui, current: JobAnchor) -> Option<JobAnchor> {
    let t = Tokens::get();
    let size = 18.0;
    let gap = 6.0;
    let total = vec2(3.0 * size + 2.0 * gap, 3.0 * size + 2.0 * gap);
    let (rect, _) = ui.allocate_exact_size(total, Sense::hover());
    let mut picked = None;
    for (i, a) in ANCHORS.iter().enumerate() {
        let (col, row) = ((i % 3) as f32, (i / 3) as f32);
        let r = Rect::from_min_size(rect.min + vec2(col * (size + gap), row * (size + gap)), vec2(size, size));
        let resp = ui.interact(r, ui.scope_id().with(("anchor", i)), Sense::click());
        let on = *a == current;
        let fill = if on {
            t.accent
        } else if resp.hovered() {
            t.hover
        } else {
            t.control_fill
        };
        ui.painter().circle(r.center(), size / 2.0 - 2.0, fill, Stroke::new(1.0, t.border));
        if resp.clicked() {
            picked = Some(*a);
        }
    }
    picked
}

fn anchor_name(a: JobAnchor) -> &'static str {
    match a {
        JobAnchor::TopLeft => "top_left",
        JobAnchor::Top => "top",
        JobAnchor::TopRight => "top_right",
        JobAnchor::Left => "left",
        JobAnchor::Center => "centre",
        JobAnchor::Right => "right",
        JobAnchor::BottomLeft => "bottom_left",
        JobAnchor::Bottom => "bottom",
        JobAnchor::BottomRight => "bottom_right",
    }
}

/// The Laser panel: job actions, Start From, job origin and the toggles.
pub fn laser_panel(ui: &mut Ui, app: &mut DarkBurn) {
    let ctx = ui.ctx().clone();
    let t = Tokens::get();
    let s = job_cmds::current_settings(app);
    let mut clicked: Option<&'static str> = None;
    // The state, progress bar and Start, Pause and Stop are the machine side's (laser_panel.rs).
    let progress = app.job.pending.as_ref().map(|(task, _)| task.progress());
    if let Some(p) = progress {
        ui_kit::progress(ui, "laser_plan", p);
        ui_kit::status_line(ui, "Planning the job...", ui_kit::StatusTone::Dim);
    }
    let buttons = |ui: &mut Ui, items: &[(&str, &'static str)], clicked: &mut Option<&'static str>| {
        ui.horizontal_wrapped(|ui| {
            for (label, id) in items {
                if ui_kit::button(ui, *label, ui_kit::Button::Small).clicked() {
                    *clicked = Some(*id);
                }
            }
        });
    };
    buttons(
        ui,
        &[("Frame", "job.frame"), ("Frame outline", "job.frame_rubber"), ("Home", "job.home"), ("Go to origin", "job.go_origin")],
        &mut clicked,
    );
    buttons(
        ui,
        &[("Save G-code", "file.save_gcode"), ("Run G-code", "job.run_gcode"), ("Send", "job.send"), ("Preview", "window.preview")],
        &mut clicked,
    );
    if let Some(op) = app.job.start_op {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(format!("Starting at operation {op}")).size(t.font_small).color(t.text_dim));
            if ui_kit::button(ui, "Clear", ui_kit::Button::Small).clicked() {
                app.job.start_op = None;
            }
        });
    }
    let mut mode = StartMode::ALL.iter().position(|m| *m == s.start_from).unwrap_or(0);
    ui_kit::row(ui, "Start from", |ui| {
        ui_kit::dropdown(ui, "start_from", &mut mode, &["Absolute coords", "Current position", "User origin"]);
    });
    if StartMode::ALL[mode] != s.start_from {
        let v = match StartMode::ALL[mode] {
            StartMode::Absolute => "absolute",
            StartMode::CurrentPosition => "current_position",
            StartMode::UserOrigin => "user_origin",
        };
        run_command(&ctx, app, "job.set_origin", &serde_json::json!({ "start_from": v }));
    }
    ui.horizontal(|ui| {
        ui.label("Job origin");
        if let Some(a) = origin_picker(ui, s.anchor) {
            run_command(&ctx, app, "job.set_origin", &serde_json::json!({ "anchor": anchor_name(a) }));
        }
    });
    if s.start_from == StartMode::UserOrigin {
        let text = match app.job.user_origin {
            Some(p) => format!("User origin at X {:.2}  Y {:.2}", p.x, p.y),
            None => "No user origin set. Use Set origin in the Move panel.".to_string(),
        };
        ui_kit::status_line(ui, &text, ui_kit::StatusTone::Dim);
        if app.job.user_origin.is_some() && ui_kit::button(ui, "Clear origin", ui_kit::Button::Small).clicked() {
            run_command(&ctx, app, "job.clear_user_origin", &serde_json::Value::Null);
        }
    }
    let mut toggles = [
        ("Cut selected graphics", "cut_selected", s.cut_selected),
        ("Use selection origin", "use_selection_origin", s.use_selection_origin),
        ("Show last position", "show_last_position", s.show_last_position),
        ("Return to start when done", "return_to_start", s.finish == darkburn_job::FinishPosition::StartPosition),
        ("Optimize cut path", "optimize", s.planner.optimize),
    ];
    for (label, key, on) in toggles.iter_mut() {
        let before = *on;
        ui_kit::checkbox(ui, label, on);
        if *on != before {
            if *key == "optimize" {
                run_command(&ctx, app, "job.set_optimization", &serde_json::json!({ "optimize": *on }));
            } else {
                run_command(&ctx, app, "job.set_origin", &serde_json::json!({ *key: *on }));
            }
        }
    }
    ui.horizontal_wrapped(|ui| {
        if ui_kit::button(ui, "Optimization settings", ui_kit::Button::Small).clicked() {
            clicked = Some("job.optimization_settings");
        }
        if ui_kit::button(ui, "G-code settings", ui_kit::Button::Small).clicked() {
            clicked = Some("job.gcode_settings");
        }
    });
    if let Some(id) = clicked {
        app.run(&ctx, id);
    }
}

// ----- Canvas overlay ----------------------------------------------------------------------------

/// The last job's position and the user origin, on the design canvas.
pub fn paint_overlay(painter: &egui::Painter, xf: &Xf, app: &DarkBurn, ink: Color32) {
    let s = job_cmds::current_settings(app);
    let device = {
        let mut d = app.job.device;
        d.bed_w = f64::from(app.prefs.page_width);
        d.bed_h = f64::from(app.prefs.page_height);
        d.origin = match app.prefs.origin {
            crate::view::Origin::FrontLeft => darkburn_job::Corner::BottomLeft,
            crate::view::Origin::FrontRight => darkburn_job::Corner::BottomRight,
            crate::view::Origin::RearLeft => darkburn_job::Corner::TopLeft,
            crate::view::Origin::RearRight => darkburn_job::Corner::TopRight,
        };
        d
    };
    if s.show_last_position
        && let Some(last) = app.job.last_position
    {
        let r = xf.rect(last.bounds);
        let stroke = Stroke::new(1.5, ink.gamma_multiply(0.8));
        let pts = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
        painter.extend(EShape::dashed_line(&pts, stroke, 6.0, 4.0));
        painter.text(r.left_top() + vec2(3.0, -2.0), egui::Align2::LEFT_BOTTOM, "Last position", egui::FontId::proportional(11.0), ink);
    }
    if let Some(o) = app.job.user_origin_design(&device) {
        let c = xf.to_screen(o);
        let st = Stroke::new(1.5, Color32::from_rgb(40, 170, 90));
        painter.line_segment([c - vec2(8.0, 0.0), c + vec2(8.0, 0.0)], st);
        painter.line_segment([c - vec2(0.0, 8.0), c + vec2(0.0, 8.0)], st);
        painter.circle_stroke(c, 4.0, st);
    }
}
