//! The galvo pieces of the window [GALVO-01..18, DS-18, LASER-21, LT-17, LT-18]: the Laser panel
//! section (arm, red light, framing), the red-light framing dialog with nudge and scale, the lens
//! test and scale calibration dialog, the repeat marking dialog and the galvo page of Device
//! Settings. Every button calls a registered command (`machine.galvo_*`, `galvo.*`) through
//! `machine_ui::run_with`; none of it reaches a real board (the only board is the simulator).

use darkburn_ui::ui_kit::{self, Action, BackingTooltipExt as _, Button, StatusTone};
use egui::{Context, Id, RichText, Ui};
use serde_json::json;
use solidcraft_theme::Tokens;

use crate::devices::Device;
use crate::galvo_settings::{GalvoDeviceSettings, Source};
use crate::i18n::tr;
use crate::machine_api::{Family, LaserState};
use crate::machine_ui;
use crate::shell::DarkBurn;

fn small(ui: &mut Ui, label: &str, tip: &str, enabled: bool) -> bool {
    ui.add_enabled_ui(enabled, |ui| ui_kit::button(ui, label, Button::Small).backing_on_hover_text(tip).clicked()).inner
}

fn note(ui: &mut Ui, text: &str) {
    let t = Tokens::get();
    ui.label(RichText::new(text).size(t.font_small).color(t.text_dim));
}

fn go(app: &mut DarkBurn, id: &str, args: serde_json::Value) {
    machine_ui::run_with(app, id, &args);
}

// ----- The Laser panel section [GALVO-09, GALVO-10, LASER-21] ----------------------------------------

/// Arm, red light and framing, under Start / Pause / Stop in the Laser panel.
pub fn panel(ui: &mut Ui, app: &mut DarkBurn) {
    let t = Tokens::get();
    let snap = app.machine.handle.snapshot().clone();
    let Some(g) = snap.galvo.clone() else {
        note(ui, &tr("galvo.connect_first", "Connect the galvo (or its simulator) to arm and frame."));
        return;
    };
    ui_kit::section_heading(ui, &tr("galvo.heading", "Galvo"));
    let connected = snap.connected();
    let idle = snap.state == LaserState::Ready;
    if g.armed {
        ui.label(RichText::new(tr("galvo.armed", "ARMED: the next job will fire the laser")).strong().size(t.font_label).color(t.error));
    } else {
        ui.label(RichText::new(tr("galvo.disarmed", "Not armed: the laser cannot fire")).size(t.font_small).color(t.text_dim));
    }
    ui.horizontal_wrapped(|ui| {
        if g.armed {
            if small(ui, &tr("galvo.disarm", "Disarm"), "Take the arm back. The laser stays off.", connected) {
                go(app, "machine.galvo_arm", json!({ "on": false }));
            }
        } else if small(
            ui,
            &tr("galvo.arm", "Arm the laser"),
            "Allow the next job to fire the laser. The arm is spent when the job ends.",
            connected && idle,
        ) {
            go(app, "machine.galvo_arm", json!({ "on": true }));
        }
        if small(ui, &tr("galvo.red_on", "Red light on"), "Show the red pointer at the current position", connected && idle) {
            go(app, "machine.galvo_light", json!({ "on": true }));
        }
        if small(ui, &tr("galvo.red_off", "Red light off"), "", connected && idle) {
            go(app, "machine.galvo_light", json!({ "on": false }));
        }
    });
    ui.horizontal_wrapped(|ui| {
        if small(ui, &tr("galvo.frame", "Frame..."), "Open the red-light framing window", connected) {
            app.machine.ext.galvo_frame_open = true;
        }
        if small(ui, &tr("galvo.lens", "Lens and test..."), "Lens correction file, test pattern and scale calibration", true) {
            app.machine.ext.galvo_lens_open = true;
        }
        if small(ui, &tr("galvo.repeat", "Repeat marking..."), "Mark the job several times", true) {
            app.machine.ext.galvo_repeat_open = true;
        }
    });
    let framed = g.framed_key.is_some();
    ui.label(
        RichText::new(format!(
            "{}{}{}",
            if g.require_framing {
                if framed { "Framing is required and done. " } else { "Framing is required before a job can start. " }
            } else {
                ""
            },
            if g.door_open { "Door open. " } else { "" },
            if g.foot_pressed { "Foot switch pressed. " } else { "" },
        ))
        .size(t.font_small)
        .color(t.text_dim),
    );
    if let Some(e) = &g.last_error {
        ui_kit::status_line(ui, e, StatusTone::Error);
    }
    if g.estimate_s > 0.0 {
        note(ui, &format!("Running: estimate {}", machine_ui::format_duration(std::time::Duration::from_secs_f64(g.estimate_s))));
    }
    if small(ui, &tr("galvo.estimate", "Estimate the time"), "Compile the job and show the library's time estimate", true) {
        go(app, "galvo.estimate", json!({}));
    }
    if let Some(e) = app.machine.ext.galvo_estimate.clone() {
        note(ui, &e);
    }
}

// ----- Dialogs ---------------------------------------------------------------------------------------

pub fn dialogs(ctx: &Context, app: &mut DarkBurn) {
    frame_dialog(ctx, app);
    lens_dialog(ctx, app);
    repeat_dialog(ctx, app);
}

fn frame_dialog(ctx: &Context, app: &mut DarkBurn) {
    if !app.machine.ext.galvo_frame_open {
        return;
    }
    let mut adj = app.machine.ext.frame;
    let framing = app.machine.handle.snapshot().state == LaserState::Framing;
    let mut resend = false;
    let (closed, action) = ui_kit::Window::new(Id::unique("db_galvo_frame"), "settings", &tr("galvo.frame_title", "Red-light frame"))
        .cards(3.0)
        .fields()
        .actions("", "Close", None)
        .show(ctx, true, |ui| {
            note(
                ui,
                &tr(
                    "galvo.frame_note",
                    "The red pointer traces the outline of the job with the laser off. Nudge and scale it to check the placement.",
                ),
            );
            ui_kit::section_heading(ui, "Nudge");
            let step = 0.5;
            ui.horizontal(|ui| {
                if small(ui, "Left", "", true) {
                    adj.nudge_mm.0 -= step;
                    resend = true;
                }
                if small(ui, "Right", "", true) {
                    adj.nudge_mm.0 += step;
                    resend = true;
                }
                if small(ui, "Up", "", true) {
                    adj.nudge_mm.1 += step;
                    resend = true;
                }
                if small(ui, "Down", "", true) {
                    adj.nudge_mm.1 -= step;
                    resend = true;
                }
                if small(ui, "Centre", "", true) {
                    adj.nudge_mm = (0.0, 0.0);
                    resend = true;
                }
            });
            ui_kit::row(ui, "Offset X", |ui| {
                ui_kit::number_field(ui, &mut adj.nudge_mm.0, -500.0..=500.0, 0.1, " mm");
            });
            ui_kit::row(ui, "Offset Y", |ui| {
                ui_kit::number_field(ui, &mut adj.nudge_mm.1, -500.0..=500.0, 0.1, " mm");
            });
            ui_kit::section_heading(ui, "Scale");
            ui.horizontal(|ui| {
                for (label, d) in [("-10%", -10.0), ("-1%", -1.0), ("+1%", 1.0), ("+10%", 10.0)] {
                    if small(ui, label, "", true) {
                        adj.scale_pct = (adj.scale_pct + d).clamp(1.0, 1000.0);
                        resend = true;
                    }
                }
                if small(ui, "100%", "", true) {
                    adj.scale_pct = 100.0;
                    resend = true;
                }
            });
            ui_kit::row(ui, "Scale", |ui| {
                ui_kit::number_field(ui, &mut adj.scale_pct, 1.0..=1000.0, 0.5, " %");
            });
            ui_kit::checkbox(ui, "Trace the outline instead of the rectangle", &mut adj.hull);
            ui_kit::checkbox(ui, "Keep going round until I stop it", &mut adj.repeat);
            ui.horizontal(|ui| {
                if small(ui, if framing { "Update the frame" } else { "Start the frame" }, "", true) {
                    resend = true;
                }
                if small(ui, "Stop", "Stop the red light", framing) {
                    go(app, "job.stop", json!({}));
                }
            });
            note(ui, &tr("galvo.frame_safe", "Framing never fires the laser and needs no arm."));
        });
    app.machine.ext.frame = adj;
    if resend {
        go(
            app,
            "machine.galvo_frame",
            json!({ "nudge_x": adj.nudge_mm.0, "nudge_y": adj.nudge_mm.1, "scale_pct": adj.scale_pct, "hull": adj.hull, "repeat": adj.repeat }),
        );
    }
    if closed || action != Action::None {
        app.machine.ext.galvo_frame_open = false;
    }
}

fn lens_dialog(ctx: &Context, app: &mut DarkBurn) {
    if !app.machine.ext.galvo_lens_open {
        return;
    }
    let mut p = app.machine.ext.lens_test;
    let mut do_test = false;
    let mut load: Option<String> = None;
    let mut clear = false;
    let mut calib: Option<(f64, f64)> = None;
    let cal_id = Id::unique("galvo_cal_vals");
    let mut cal = ctx.data_mut(|d| d.get_temp::<(f64, f64)>(cal_id)).unwrap_or((100.0, 100.0));
    let cor = app.machine.active().settings.galvo.cor_path.clone();
    let (closed, action) = ui_kit::Window::new(Id::unique("db_galvo_lens"), "settings", &tr("galvo.lens_title", "Lens correction and test"))
        .cards(3.0)
        .fields()
        .actions("", "Close", None)
        .show(ctx, true, |ui| {
            ui_kit::section_heading(ui, "Lens correction file");
            note(ui, &if cor.is_empty() { "No lens correction file.".to_owned() } else { cor.clone() });
            ui.horizontal(|ui| {
                if small(ui, "Load .cor...", "Load a lens correction file", true)
                    && let Some(f) = rfd::FileDialog::new().add_filter("Lens correction", &["cor"]).pick_file()
                {
                    load = Some(f.display().to_string());
                }
                if small(ui, "Clear", "Go back to no correction", !cor.is_empty()) {
                    clear = true;
                }
            });
            ui_kit::section_heading(ui, "Test pattern");
            note(ui, "A border, grid and crosses. Mark it on card and measure it against the nominal size.");
            ui_kit::row(ui, "Size", |ui| {
                ui_kit::number_field(ui, &mut p.size_mm, 1.0..=1000.0, 1.0, " mm");
            });
            ui_kit::row(ui, "Cells per side", |ui| {
                ui_kit::number_field(ui, &mut p.divisions, 1..=50, 1.0, "");
            });
            ui_kit::row(ui, "Cross size", |ui| {
                ui_kit::number_field(ui, &mut p.cross_mm, 0.0..=20.0, 0.1, " mm");
            });
            ui_kit::checkbox(ui, "Grid lines", &mut p.grid_lines);
            ui_kit::row(ui, "On layer", |ui| {
                ui_kit::number_field(ui, &mut p.layer, 0..=29, 1.0, "");
            });
            if small(ui, "Add the pattern to the design", "", true) {
                do_test = true;
            }
            ui_kit::section_heading(ui, "Scale calibration");
            note(ui, "Mark a square, measure it, and enter both sizes. The scale of both galvos is corrected.");
            ui_kit::row(ui, "Nominal size", |ui| {
                ui_kit::number_field(ui, &mut cal.0, 1.0..=1000.0, 0.1, " mm");
            });
            ui_kit::row(ui, "Measured size", |ui| {
                ui_kit::number_field(ui, &mut cal.1, 1.0..=1000.0, 0.01, " mm");
            });
            if small(ui, "Apply the correction", "", true) {
                calib = Some(cal);
            }
        });
    ctx.data_mut(|d| d.insert_temp(cal_id, cal));
    app.machine.ext.lens_test = p;
    if let Some(path) = load {
        go(app, "machine.galvo_lens", json!({ "path": path }));
    }
    if clear {
        go(app, "machine.galvo_lens", json!({}));
    }
    if do_test {
        go(
            app,
            "galvo.lens_test",
            json!({ "size_mm": p.size_mm, "divisions": p.divisions, "cross_mm": p.cross_mm, "grid_lines": p.grid_lines, "layer": p.layer }),
        );
    }
    if let Some((nominal, measured)) = calib {
        let id = app.machine.active().id;
        if let Some(d) = app.machine.devices.get_mut(id)
            && d.family() == Family::Galvo
        {
            let g = &mut d.settings.galvo;
            if nominal > 0.0 && measured > 0.0 {
                g.cal1.scale *= nominal / measured;
                g.cal2.scale *= nominal / measured;
            }
        }
    }
    if closed || action != Action::None {
        app.machine.ext.galvo_lens_open = false;
    }
}

fn repeat_dialog(ctx: &Context, app: &mut DarkBurn) {
    if !app.machine.ext.galvo_repeat_open {
        return;
    }
    let id = app.machine.active().id;
    let mut r = app.machine.active().settings.galvo.repeat;
    let (closed, action) = ui_kit::Window::new(Id::unique("db_galvo_repeat"), "settings", &tr("galvo.repeat_title", "Repeat marking"))
        .cards(2.0)
        .fields()
        .actions("", "Close", None)
        .show(ctx, true, |ui| {
            note(ui, "Mark the whole job again and again, optionally moving an external axis between marks.");
            ui_kit::row(ui, "Times", |ui| {
                ui_kit::number_field(ui, &mut r.count, 1..=100_000, 1.0, "");
            });
            ui_kit::checkbox(ui, "Move an external axis between marks", &mut r.axis_on);
            if r.axis_on {
                ui_kit::row(ui, "Axis", |ui| {
                    ui_kit::number_field(ui, &mut r.axis, 0..=1, 1.0, "");
                });
                ui_kit::row(ui, "Step", |ui| {
                    ui_kit::number_field(ui, &mut r.axis_step_mm, -10_000.0..=10_000.0, 0.1, " mm");
                });
            }
            ui_kit::checkbox(ui, "Wait for the foot switch before each repeat", &mut r.wait_foot_each);
        });
    if let Some(d) = app.machine.devices.get_mut(id)
        && d.family() == Family::Galvo
    {
        d.settings.galvo.repeat = r;
    }
    if closed || action != Action::None {
        app.machine.ext.galvo_repeat_open = false;
    }
}

// ----- Device Settings: the galvo page [DS-18] -----------------------------------------------------

fn opt_bit(ui: &mut Ui, label: &str, v: &mut Option<u8>) {
    ui.horizontal(|ui| {
        let mut on = v.is_some();
        ui_kit::checkbox(ui, label, &mut on);
        if on {
            let mut b = v.unwrap_or(0);
            ui_kit::number_field(ui, &mut b, 0..=15, 1.0, "");
            *v = Some(b);
        } else {
            *v = None;
        }
    });
}

fn cal_block(ui: &mut Ui, title: &str, c: &mut crate::galvo_settings::Cal) {
    ui_kit::section_heading(ui, title);
    ui_kit::row(ui, "Scale", |ui| {
        ui_kit::number_field(ui, &mut c.scale, -10.0..=10.0, 0.001, "");
    });
    ui_kit::row(ui, "Skew", |ui| {
        ui_kit::number_field(ui, &mut c.skew, -1.0..=1.0, 0.0005, "");
    });
    ui_kit::row(ui, "Bulge", |ui| {
        ui_kit::number_field(ui, &mut c.bulge, -1.0..=1.0, 0.0005, "");
    });
    ui_kit::row(ui, "Trapezoid", |ui| {
        ui_kit::number_field(ui, &mut c.trapezoid, -1.0..=1.0, 0.0005, "");
    });
    ui_kit::checkbox(ui, &format!("Negate ({title})"), &mut c.negate);
    ui_kit::checkbox(ui, &format!("This galvo is X ({title})"), &mut c.is_x);
}

/// The galvo page of the Device Settings dialog. `message` is the last import result.
pub fn settings_page(ui: &mut Ui, d: &mut Device, message: &mut Option<String>) {
    let g: &mut GalvoDeviceSettings = &mut d.settings.galvo;
    ui_kit::section_heading(ui, &tr("galvo.set.laser", "Laser source"));
    let mut si = Source::ALL.iter().position(|s| *s == g.source).unwrap_or(0);
    ui_kit::row(ui, "Type", |ui| {
        let labels: Vec<&str> = Source::ALL.iter().map(|s| s.label()).collect();
        ui_kit::dropdown(ui, "galvo_source", &mut si, &labels);
    });
    g.source = Source::ALL[si];
    ui_kit::row(ui, "Frequency", |ui| {
        ui_kit::number_field(ui, &mut g.default_frequency_khz, 1.0..=4000.0, 1.0, " kHz");
    });
    if g.source.has_q_pulse() {
        ui_kit::row(ui, "Q-pulse width", |ui| {
            ui_kit::number_field(ui, &mut g.default_q_pulse_ns, 1.0..=10_000.0, 1.0, " ns");
        });
    } else {
        note(ui, "The Q-pulse width is only used by a MOPA source.");
    }
    ui_kit::checkbox(ui, "Invert power (UV boards)", &mut g.power_invert);
    ui_kit::row(ui, "PWM half period", |ui| {
        ui_kit::number_field(ui, &mut g.pwm_half_period, 1..=65_535, 1.0, "");
    });
    ui_kit::row(ui, "PWM pulse width", |ui| {
        ui_kit::number_field(ui, &mut g.pwm_pulse_width, 0..=65_535, 1.0, "");
    });
    ui_kit::row(ui, "Wobble points per turn", |ui| {
        ui_kit::number_field(ui, &mut g.wobble_points_per_rev, 3..=64, 1.0, "");
    });
    note(ui, "Wobble size and step are set per layer, in the Advanced tab of the cut settings.");

    ui_kit::section_heading(ui, &tr("galvo.set.field", "Field"));
    ui_kit::row(ui, "Field size", |ui| {
        ui_kit::number_field(ui, &mut g.field_size_mm, 5.0..=1000.0, 1.0, " mm");
    });
    ui_kit::row(ui, "Offset X", |ui| {
        ui_kit::number_field(ui, &mut g.offset_x_mm, -500.0..=500.0, 0.05, " mm");
    });
    ui_kit::row(ui, "Offset Y", |ui| {
        ui_kit::number_field(ui, &mut g.offset_y_mm, -500.0..=500.0, 0.05, " mm");
    });
    ui_kit::checkbox(ui, "Zero is the lower-left corner of the field", &mut g.origin_lower_left);
    ui_kit::checkbox(ui, "Flip X", &mut g.flip_x);
    ui_kit::checkbox(ui, "Flip Y", &mut g.flip_y);
    ui_kit::checkbox(ui, "Swap X and Y", &mut g.swap_xy);
    let mut rq = usize::from(g.rotate_quarters % 4);
    ui_kit::row(ui, "Rotate", |ui| {
        ui_kit::dropdown(ui, "galvo_rotate", &mut rq, &["0 degrees", "90 degrees", "180 degrees", "270 degrees"]);
    });
    g.rotate_quarters = rq as u8;

    ui_kit::section_heading(ui, &tr("galvo.set.cal", "Calibration"));
    cal_block(ui, "Galvo 1", &mut g.cal1);
    cal_block(ui, "Galvo 2", &mut g.cal2);

    ui_kit::section_heading(ui, &tr("galvo.set.lens", "Lens correction"));
    ui_kit::row(ui, "Lens file", |ui| {
        ui_kit::hint_field(ui, &mut g.cor_path, "none");
    });
    ui.horizontal(|ui| {
        if small(ui, "Browse...", "", true)
            && let Some(f) = rfd::FileDialog::new().add_filter("Lens correction", &["cor"]).pick_file()
        {
            g.cor_path = f.display().to_string();
        }
        if small(ui, "Clear", "", !g.cor_path.is_empty()) {
            g.cor_path.clear();
        }
    });
    if let Err(e) = g.load_lens() {
        ui_kit::status_line(ui, &e, StatusTone::Error);
    }

    ui_kit::section_heading(ui, &tr("galvo.set.timing", "Timing"));
    let t = &mut g.timings;
    ui_kit::row(ui, "Jump speed", |ui| {
        ui_kit::number_field(ui, &mut t.jump_speed_mm_s, 1.0..=100_000.0, 10.0, " mm/s");
    });
    ui_kit::row(ui, "Min jump delay", |ui| {
        ui_kit::number_field(ui, &mut t.min_jump_delay, 0..=100_000, 1.0, " us");
    });
    ui_kit::row(ui, "Max jump delay", |ui| {
        ui_kit::number_field(ui, &mut t.max_jump_delay, 0..=100_000, 1.0, " us");
    });
    ui_kit::row(ui, "Jump distance limit", |ui| {
        ui_kit::number_field(ui, &mut t.jump_distance_limit_mm, 0.0..=1000.0, 0.5, " mm");
    });
    ui_kit::row(ui, "Laser on delay", |ui| {
        ui_kit::number_field(ui, &mut t.laser_on_tc, -10_000..=10_000, 1.0, " us");
    });
    ui_kit::row(ui, "Laser off delay", |ui| {
        ui_kit::number_field(ui, &mut t.laser_off_tc, -10_000..=10_000, 1.0, " us");
    });
    ui_kit::row(ui, "End delay", |ui| {
        ui_kit::number_field(ui, &mut t.end_tc, 0..=100_000, 1.0, " us");
    });
    ui_kit::row(ui, "Polygon delay", |ui| {
        ui_kit::number_field(ui, &mut t.polygon_tc, 0..=100_000, 1.0, " us");
    });

    ui_kit::section_heading(ui, &tr("galvo.set.light", "Red light"));
    ui_kit::row(ui, "Frame speed", |ui| {
        ui_kit::number_field(ui, &mut g.light_speed_mm_s, 1.0..=100_000.0, 10.0, " mm/s");
    });
    ui_kit::row(ui, "Corner delay (dark)", |ui| {
        ui_kit::number_field(ui, &mut g.light_delay_dark, 0..=100_000, 1.0, " us");
    });
    ui_kit::row(ui, "Corner delay (light)", |ui| {
        ui_kit::number_field(ui, &mut g.light_delay_light, 0..=100_000, 1.0, " us");
    });
    ui_kit::checkbox(ui, "Require framing before a job starts", &mut g.require_framing);

    ui_kit::section_heading(ui, &tr("galvo.set.ports", "Ports"));
    ui_kit::row(ui, "Red light output bit", |ui| {
        ui_kit::number_field(ui, &mut g.ports.light_bit, 0..=15, 1.0, "");
    });
    ui_kit::row(ui, "Marking output bit", |ui| {
        ui_kit::number_field(ui, &mut g.ports.mark_bit, 0..=15, 1.0, "");
    });
    opt_bit(ui, "Foot switch input", &mut g.ports.foot_bit);
    ui_kit::checkbox(ui, "Foot switch reads low when pressed", &mut g.ports.foot_active_low);
    opt_bit(ui, "Door switch input", &mut g.ports.door_bit);
    ui_kit::checkbox(ui, "Door switch reads low when the door is open", &mut g.ports.door_open_is_low);
    opt_bit(ui, "Done-marking output", &mut g.ports.done_bit);
    if g.ports.done_bit.is_some() {
        ui_kit::row(ui, "Done pulse length", |ui| {
            ui_kit::number_field(ui, &mut g.ports.done_ms, 1..=60_000, 10.0, " ms");
        });
    }
    ui_kit::checkbox(ui, "Wait for the door to close before each layer pass", &mut g.door_protect);

    ui_kit::section_heading(ui, &tr("galvo.set.axes", "External axes"));
    ui_kit::row(ui, "Axis 0 steps per mm", |ui| {
        ui_kit::number_field(ui, &mut g.axis_steps_per_mm[0], 0.01..=100_000.0, 1.0, "");
    });
    ui_kit::row(ui, "Axis 1 steps per mm", |ui| {
        ui_kit::number_field(ui, &mut g.axis_steps_per_mm[1], 0.01..=100_000.0, 1.0, "");
    });
    ui_kit::row(ui, "Z steps per mm", |ui| {
        ui_kit::number_field(ui, &mut g.z_steps_per_mm, 0.01..=100_000.0, 1.0, "");
    });

    ui_kit::section_heading(ui, &tr("galvo.set.job", "Job"));
    ui_kit::checkbox(ui, "Order paths by nearest neighbour", &mut g.optimize);
    ui_kit::row(ui, "Repeat marking", |ui| {
        ui_kit::number_field(ui, &mut g.repeat.count, 1..=100_000, 1.0, " times");
    });
    ui_kit::checkbox(ui, "Rotary runner", &mut g.rotary.on);
    if g.rotary.on {
        ui_kit::checkbox(ui, "Keep shapes whole (by shape)", &mut g.rotary.by_shape);
        ui_kit::row(ui, "Longest band", |ui| {
            ui_kit::number_field(ui, &mut g.rotary.max_size_mm, 1.0..=1000.0, 1.0, " mm");
        });
        ui_kit::row(ui, "Rotary axis", |ui| {
            ui_kit::number_field(ui, &mut g.rotary.axis, 0..=1, 1.0, "");
        });
    }

    ui_kit::section_heading(ui, &tr("galvo.set.import", "EZCad"));
    if small(
        ui,
        "Import an EZCad configuration...",
        "Read field, calibration, lens file name, laser type and timings from a text configuration file",
        true,
    ) && let Some(f) = rfd::FileDialog::new().add_filter("EZCad configuration", &["cfg", "ini", "txt", "markcfg"]).pick_file()
    {
        *message =
            Some(match std::fs::read(&f).map_err(|e| e.to_string()).and_then(|b| darkburn_galvo::ezcad::import(&b).map_err(|e| e.to_string())) {
                Ok(imp) => {
                    g.apply_library(&imp.settings);
                    if let Some(c) = &imp.cor_file {
                        g.cor_path = c.clone();
                    }
                    format!("Imported {} settings. {} not recognised.", imp.applied.len(), imp.ignored.len())
                }
                Err(e) => e,
            });
    }
    if let Some(m) = message.as_deref() {
        note(ui, m);
    }
    note(ui, "Laser enable is never sent when the board connects. It is sent only when a job starts after you press Arm.");
    d.width_mm = d.settings.galvo.field_size_mm;
    d.height_mm = d.settings.galvo.field_size_mm;
}
