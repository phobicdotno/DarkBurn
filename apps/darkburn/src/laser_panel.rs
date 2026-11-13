//! The Laser, Move, Console and File List panels [LASER-01..05, 09, 10, 17..22, MOVE-01..18,
//! CON-01..04, FL-01..03]. Each button calls a registered `machine.*` command through
//! `machine_ui::run_with`; the panels keep only the fields they show.

use darkburn_ui::ui_kit::{self, BackingTooltipExt as _, Button, StatusTone};
use egui::{RichText, Ui};
use serde_json::{Value, json};
use solidcraft_theme::Tokens;

use crate::devices::AUTO_PORT;
use crate::machine_api::{ConsoleKind, Family, LaserState};
use crate::machine_ui::{self, format_duration};
use crate::shell::DarkBurn;

/// A small button that can be disabled, with a tooltip. True when clicked.
fn btn(ui: &mut Ui, label: &str, tip: &str, enabled: bool) -> bool {
    ui.add_enabled_ui(enabled, |ui| ui_kit::button(ui, label, Button::Small).backing_on_hover_text(tip).clicked()).inner
}

fn go(app: &mut DarkBurn, id: &str, args: Value) {
    machine_ui::run_with(app, id, &args);
}

/// The panels this module draws, by panel id. False for any other.
pub fn show_panel(ui: &mut Ui, id: &str, app: &mut DarkBurn) -> bool {
    match id {
        "laser" => laser(ui, app),
        "move" => move_panel(ui, app),
        "console" => console(ui, app),
        "file_list" => file_list(ui, app),
        _ => return false,
    }
    true
}

// ----- Laser ------------------------------------------------------------------------------------

/// The badge line: SIMULATOR or REAL MACHINE, the first thing in the Laser panel.
pub fn banner(ui: &mut Ui, app: &DarkBurn) {
    let t = Tokens::get();
    let m = &app.machine;
    if m.handle.is_real_machine() {
        ui.label(RichText::new("REAL MACHINE").strong().size(t.font_label).color(t.error));
        ui.label(
            RichText::new("Commands here move the laser and can fire it. Keep the lid closed and stay near the stop button.")
                .size(t.font_small)
                .color(t.error),
        );
    } else if m.active().is_simulator() || m.handle.is_simulator() {
        ui.label(RichText::new("SIMULATOR: no hardware attached").strong().size(t.font_label).color(t.accent));
    } else if m.active().is_design_only() {
        ui.label(RichText::new("DESIGN ONLY: no laser").size(t.font_label).color(t.text_dim));
    }
}

fn laser(ui: &mut Ui, app: &mut DarkBurn) {
    let t = Tokens::get();
    banner(ui, app);
    let snap = app.machine.handle.snapshot().clone();
    let dev = app.machine.active().clone();
    let family = machine_ui::family_of(&app.machine);
    let design_only = dev.is_design_only();

    // Status [LASER-01]
    let (text, tone) = match snap.state {
        LaserState::Disconnected if design_only => ("No laser (design only)".to_string(), StatusTone::Dim),
        LaserState::Disconnected => ("Disconnected".to_string(), StatusTone::Dim),
        LaserState::Alarm => (format!("Alarm{}", snap.alarm.as_deref().map(|a| format!(": {a}")).unwrap_or_default()), StatusTone::Error),
        s => (format!("{}{}", s.label(), if snap.firmware.is_empty() { String::new() } else { format!("  ({})", snap.firmware) }), StatusTone::Dim),
    };
    ui_kit::status_line(ui, &text, tone);
    if let Some(m) = app.machine.message.clone() {
        ui_kit::status_line(ui, &m.text, if m.error { StatusTone::Error } else { StatusTone::Dim });
    }

    // Progress [LASER-02]
    let (fraction, line) = match &snap.job {
        Some(j) => {
            let rem = j.remaining.map(|r| format!("  remaining {}", format_duration(r))).unwrap_or_default();
            (j.fraction.unwrap_or(0.0) as f32, format!("elapsed {}{rem}", format_duration(j.elapsed)))
        }
        None => (0.0, "No job running".to_string()),
    };
    ui_kit::progress(ui, "laser_job", fraction);
    ui.label(RichText::new(line).size(t.font_small).color(t.text_dim));
    if let Some((sent, total)) = snap.ruida.upload {
        ui.label(RichText::new(format!("Sending to the controller: {sent} of {total} bytes")).size(t.font_small).color(t.text_dim));
    }

    // Start, pause, stop [LASER-03]; stop and pause are never disabled.
    let connected = snap.connected();
    let ready = snap.state.can_start();
    let start_ok = ready && !(family == Family::Ruida && dev.settings.dsp.disable_start) && !design_only;
    ui.horizontal_wrapped(|ui| {
        if btn(ui, "Start", "Run the job on the laser", start_ok) {
            go(app, "job.start", json!({}));
        }
        let paused = snap.state == LaserState::Paused;
        if ui_kit::button(ui, if paused { "Resume" } else { "Pause" }, Button::Small).backing_on_hover_text("Pause or resume the job").clicked() {
            go(app, "job.pause", json!({}));
        }
        if ui_kit::button(ui, "Stop", Button::Small).backing_on_hover_text("Stop the job and turn the laser off").clicked() {
            go(app, "job.stop", json!({}));
        }
    });
    if family == Family::Ruida && dev.settings.dsp.disable_start {
        ui.label(RichText::new("Start is off for this device. Send the job and start it from the controller.").size(t.font_small).color(t.text_dim));
    }
    if snap.state == LaserState::Alarm && family == Family::Grbl {
        ui.horizontal_wrapped(|ui| {
            if btn(ui, "Home", "Run the homing cycle", true) {
                go(app, "job.home", json!({}));
            }
            if btn(ui, "Unlock", "Clear the alarm lock without homing", true) {
                go(app, "machine.unlock", json!({}));
            }
        });
    }
    if family == Family::Galvo {
        crate::galvo_ui::panel(ui, app);
    }
    // Fire [MOVE-15]: also here for convenience when the device allows it.
    if fire_allowed(app) {
        fire_button(ui, app, connected && ready);
    }

    // Connection [LASER-17..20]
    ui.add_space(t.space_6);
    ui_kit::section_heading(ui, "Connection");
    ui.horizontal_wrapped(|ui| {
        if connected || snap.state == LaserState::Connecting {
            if btn(ui, "Disconnect", "Close the connection", true) {
                go(app, "machine.disconnect", json!({}));
            }
        } else if btn(ui, "Connect", "Connect to the selected device", !design_only) {
            go(app, "machine.connect", json!({}));
        }
        let devices =
            ui_kit::button(ui, "Devices", Button::Small).backing_on_hover_text("Open the device list. Right-click to reset the connection.");
        if devices.clicked() {
            go(app, "laser.devices", json!({}));
        }
        if devices.secondary_clicked() {
            go(app, "machine.reset_connection", json!({}));
        }
    });
    // Device profile [LASER-19]
    let names: Vec<String> = app.machine.devices.devices.iter().map(|d| d.name.clone()).collect();
    let mut idx = app.machine.devices.devices.iter().position(|d| Some(d.id) == app.machine.devices.active).unwrap_or(0);
    let before = idx;
    ui_kit::row(ui, "Device", |ui| {
        let opts: Vec<&str> = names.iter().map(String::as_str).collect();
        ui_kit::dropdown(ui, "laser_device", &mut idx, &opts);
    });
    if idx != before {
        let id = app.machine.devices.devices[idx].id;
        go(app, "device.select", json!({ "id": id }));
    }
    // Port [LASER-18, LASER-20]
    if matches!(dev.connection, crate::devices::ConnectionKind::Serial | crate::devices::ConnectionKind::Bluetooth) && !dev.is_simulator() {
        port_picker(ui, app);
    } else if !dev.is_simulator() && !design_only {
        ui.label(RichText::new(format!("Connection: {}", dev.connection.label())).size(t.font_small).color(t.text_dim));
    }
    if ui_kit::button(ui, "Rescan", Button::Small)
        .backing_on_hover_text("Look for serial ports again. This only lists them; nothing is opened.")
        .clicked()
    {
        app.machine.rescan_ports();
    }
    crate::connections::laser_panel_rows(ui, app);

    // Mode status [LASER-22]
    ui.add_space(t.space_6);
    // The boxes are the project's rotary and cylinder setup itself (what the job is built
    // with), not a note beside it.
    let setup = crate::vision_state::VisionState::from_doc(&app.editor.doc);
    let (mut rot, mut cyl) = (setup.rotary.enabled, setup.cylinder.enabled);
    if ui_kit::checkbox(ui, "Rotary", &mut rot).changed()
        && let Err(e) = crate::vision_cmds::rotary_set(app, &json!({ "enabled": rot }))
    {
        app.machine.fail(e);
    }
    if ui_kit::checkbox(ui, "Cylinder correction", &mut cyl).changed()
        && let Err(e) = crate::vision_cmds::cylinder_set(app, &json!({ "enabled": cyl }))
    {
        app.machine.fail(e);
    }
    let setup = crate::vision_state::VisionState::from_doc(&app.editor.doc);
    let (rot, cyl) = (setup.rotary.enabled, setup.cylinder.enabled);
    ui.label(RichText::new(mode_status(rot, cyl, dev.settings.dsp.u_axis || family == Family::Grbl)).size(t.font_small).color(t.text_dim));

    ui.add_space(t.space_6);
    crate::job_ui::laser_panel(ui, app);
}

/// What the Laser panel says about rotary and cylinder correction.
pub fn mode_status(rotary: bool, cylinder: bool, has_axis: bool) -> String {
    match (rotary, cylinder) {
        (false, false) => "Flat bed".into(),
        (true, false) if has_axis => "Rotary on: jobs wrap around the roller".into(),
        (true, false) => "Rotary on, but this device has no rotary axis set up".into(),
        (false, true) => "Cylinder correction on: the design is stretched to suit a cylinder".into(),
        (true, true) => "Rotary and cylinder correction both on".into(),
    }
}

fn fire_allowed(app: &DarkBurn) -> bool {
    let d = app.machine.active();
    match d.family() {
        Family::Grbl => d.settings.gcode.fire_button,
        Family::Ruida => true,
        Family::DesignOnly | Family::Galvo | Family::FileOnly => false,
    }
}

/// Press and hold: the laser is on while the button is held, and goes off by itself after the
/// time limit or when the pointer lets go.
fn fire_button(ui: &mut Ui, app: &mut DarkBurn, enabled: bool) {
    let r = ui
        .add_enabled_ui(enabled, |ui| {
            ui_kit::button(ui, "Fire (hold)", Button::Small)
                .backing_on_hover_text("Hold to fire the laser at the power below. It goes off when you let go.")
        })
        .inner;
    let down = r.is_pointer_button_down_on();
    let on = app.machine.handle.snapshot().fire_on;
    if down && !on && enabled {
        let (p, s) = (app.machine.fire_percent, app.machine.fire_secs.clamp(1.0, 10.0));
        go(app, "machine.fire", json!({ "percent": p, "secs": s }));
    } else if !down && on {
        go(app, "machine.fire_off", json!({}));
    }
    ui_kit::row(ui, "Fire power", |ui| {
        let max = app.machine.active().settings.fire_max_percent.max(0.1);
        ui_kit::number_field(ui, &mut app.machine.fire_percent, 0.1..=max, 0.1, " %");
    });
}

fn port_picker(ui: &mut Ui, app: &mut DarkBurn) {
    if !app.machine.ports_scanned {
        app.machine.rescan_ports();
    }
    let ports = app.machine.ports.clone();
    let remembered = app.machine.active().state.port.clone();
    let mut labels = vec!["Automatic".to_string()];
    labels.extend(ports.iter().map(|p| p.label()));
    let mut idx = if remembered == AUTO_PORT { 0 } else { ports.iter().position(|p| p.name == remembered).map_or(0, |i| i + 1) };
    let before = idx;
    ui_kit::row(ui, "Port", |ui| {
        let opts: Vec<&str> = labels.iter().map(String::as_str).collect();
        ui_kit::dropdown(ui, "laser_port", &mut idx, &opts);
    });
    if idx != before {
        let port = if idx == 0 { AUTO_PORT.to_string() } else { ports[idx - 1].name.clone() };
        if let Some(d) = app.machine.devices.active_mut() {
            d.state.port = port;
        }
    }
    // Exclusion list: ports DarkBurn never picks on its own.
    if !ports.is_empty() {
        ui.collapsing("Ignore these ports", |ui| {
            for p in &ports {
                let excluded = app.machine.active().state.excluded_ports.iter().any(|e| e == &p.name);
                let mut on = excluded;
                ui_kit::checkbox(ui, &p.label(), &mut on);
                if on != excluded
                    && let Some(d) = app.machine.devices.active_mut()
                {
                    if on {
                        d.state.excluded_ports.push(p.name.clone());
                    } else {
                        d.state.excluded_ports.retain(|e| e != &p.name);
                    }
                }
            }
        });
    }
    if ports.is_empty() {
        ui.label(RichText::new("No serial ports found.").size(Tokens::get().font_small).color(Tokens::get().text_dim));
    }
}

// ----- Move -----------------------------------------------------------------------------------------

fn move_panel(ui: &mut Ui, app: &mut DarkBurn) {
    let t = Tokens::get();
    let snap = app.machine.handle.snapshot().clone();
    let family = machine_ui::family_of(&app.machine);
    let dev = app.machine.active().clone();
    let free = snap.connected() && snap.state == LaserState::Ready;
    if !snap.connected() {
        ui.label(RichText::new("Connect a laser to jog it. The simulator needs no hardware.").size(t.font_small).color(t.text_dim));
    }
    banner(ui, app);
    let (ctrl, shift) = ui.input(|i| (i.modifiers.command, i.modifiers.shift));
    let mult = machine_ui::jog_multiplier(ctrl, shift);

    // Arrows [MOVE-01, MOVE-04, MOVE-06, MOVE-07]
    const ARROWS: [[(&str, i8, i8); 3]; 3] = [
        [("\u{2196}", -1, 1), ("\u{2191}", 0, 1), ("\u{2197}", 1, 1)],
        [("\u{2190}", -1, 0), ("Home", 0, 0), ("\u{2192}", 1, 0)],
        [("\u{2199}", -1, -1), ("\u{2193}", 0, -1), ("\u{2198}", 1, -1)],
    ];
    let mut pressed: Option<(i8, i8)> = None;
    let mut homed = false;
    for row in ARROWS {
        ui.horizontal(|ui| {
            for (label, dx, dy) in row {
                let r = ui
                    .add_enabled_ui(free || (app.machine.jog.holding && label != "Home"), |ui| {
                        ui_kit::button(ui, label, Button::Small).backing_on_hover_text("Ctrl x10, Shift x0.1, both x0.01")
                    })
                    .inner;
                if label == "Home" {
                    homed |= r.clicked();
                } else if app.machine.jog.continuous {
                    if r.is_pointer_button_down_on() && !app.machine.jog.holding {
                        pressed = Some((dx, dy));
                    }
                } else if r.clicked() {
                    pressed = Some((dx, dy));
                }
            }
        });
    }
    if homed {
        go(app, "machine.home", json!({}));
    }
    if let Some((dx, dy)) = pressed {
        jog_arrow(app, dx, dy, mult);
    }
    // Numpad keys jog while the pointer is over the panel [MOVE-01].
    if free && ui.rect_contains_pointer(ui.max_rect()) && !app.machine.jog.continuous && ui.memory(|m| m.focused().is_none()) {
        use egui::Key::*;
        let keys = [(Num7, -1, 1), (Num8, 0, 1), (Num9, 1, 1), (Num4, -1, 0), (Num6, 1, 0), (Num1, -1, -1), (Num2, 0, -1), (Num3, 1, -1)];
        if let Some((_, dx, dy)) = keys.iter().find(|(k, _, _)| ui.input(|i| i.key_pressed(*k))) {
            jog_arrow(app, *dx, *dy, mult);
        }
    }

    // Z and rotary [MOVE-02, MOVE-03]
    if dev.settings.z_enabled || family == Family::Ruida {
        ui.horizontal(|ui| {
            ui.label("Z");
            let rev = if dev.settings.z_reverse { -1.0 } else { 1.0 };
            for (label, dir) in [("Z up", 1.0), ("Z down", -1.0)] {
                if btn(ui, label, "Jog the Z axis by the distance", free) {
                    let feed = app.machine.jog.z_speed * 60.0;
                    go(app, "machine.jog", json!({ "dz": dir * rev * app.machine.jog.distance * mult, "feed": feed }));
                }
            }
        });
        ui_kit::row(ui, "Z speed", |ui| {
            ui_kit::number_field(ui, &mut app.machine.jog.z_speed, 0.1..=200.0, 0.5, " mm/s");
        });
    }
    if family == Family::Ruida && dev.settings.dsp.u_axis {
        ui.horizontal(|ui| {
            ui.label("Rotary");
            for (label, dir) in [("U \u{2212}", -1.0), ("U +", 1.0)] {
                if btn(ui, label, "Turn the rotary axis by the distance", free) {
                    let feed = app.machine.jog.speed * 60.0;
                    go(app, "machine.jog", json!({ "du": dir * app.machine.jog.distance * mult, "feed": feed }));
                }
            }
        });
    }

    // Distance, speed, options [MOVE-05, MOVE-06, MOVE-16]
    let inches = app.prefs.numeric_inches;
    let (unit, scale) = if inches { (" in", 1.0 / 25.4) } else { (" mm", 1.0) };
    ui_kit::row(ui, "Distance", |ui| {
        let mut v = app.machine.jog.distance * scale;
        if ui_kit::number_field(ui, &mut v, 0.001..=2000.0, 0.1, unit).changed() {
            app.machine.jog.distance = (v / scale).max(0.001);
        }
    });
    ui_kit::row(ui, "Speed", |ui| {
        ui_kit::number_field(ui, &mut app.machine.jog.speed, 0.1..=2000.0, 1.0, " mm/s");
    });
    ui_kit::checkbox(ui, "Continuous jog", &mut app.machine.jog.continuous);
    ui_kit::checkbox(ui, "Invert keypad direction", &mut app.machine.jog.invert_keypad);

    // Position [MOVE-08]
    ui_kit::section_heading(ui, "Position");
    let pos = position_text(snap.has_position.then_some(snap.work_pos), inches);
    ui.label(RichText::new(pos).monospace().size(t.font_small));
    if btn(ui, "Get position", "Ask the laser where it is", snap.connected()) {
        go(app, "machine.get_position", json!({}));
    }

    // Move to [MOVE-09, MOVE-11, MOVE-12]
    ui_kit::section_heading(ui, "Move to");
    if inches {
        ui.add(egui::Label::new(RichText::new("In inches; a unit can be typed (25 mm).").size(t.font_small).color(t.text_dim)).wrap());
    }
    ui_kit::row(ui, "X", |ui| {
        ui_kit::field(ui, &mut app.machine.jog.goto_x);
    });
    ui_kit::row(ui, "Y", |ui| {
        ui_kit::field(ui, &mut app.machine.jog.goto_y);
    });
    ui_kit::row(ui, "Z", |ui| {
        ui_kit::hint_field(ui, &mut app.machine.jog.goto_z, "unchanged");
    });
    ui_kit::checkbox(ui, "From machine zero", &mut app.machine.jog.from_machine_zero);
    ui.horizontal_wrapped(|ui| {
        if btn(ui, "Move to", "Move to the position above", free) {
            let j = &app.machine.jog;
            let mut a = json!({ "machine_coords": j.from_machine_zero, "feed": j.speed * 60.0 });
            for (k, v) in [("x", &j.goto_x), ("y", &j.goto_y), ("z", &j.goto_z)] {
                if let Some(n) = typed_mm(v, inches) {
                    a[k] = json!(n);
                }
            }
            go(app, "machine.move_to", a);
        }
        if btn(ui, "Focus Z", "Move only the Z axis to the Z value above", free && typed_mm(&app.machine.jog.goto_z, inches).is_some()) {
            let z = typed_mm(&app.machine.jog.goto_z, inches).unwrap_or(0.0);
            go(
                app,
                "machine.move_to",
                json!({ "z": z, "feed": app.machine.jog.z_speed * 60.0, "machine_coords": app.machine.jog.from_machine_zero }),
            );
        }
    });

    // Saved positions [MOVE-10]
    ui_kit::section_heading(ui, "Saved positions");
    if btn(ui, "Save position", "Remember the position above", true) {
        let j = &app.machine.jog;
        let (x, y) = (typed_mm(&j.goto_x, inches).unwrap_or(0.0), typed_mm(&j.goto_y, inches).unwrap_or(0.0));
        let z = typed_mm(&j.goto_z, inches);
        let n = j.saved.len() + 1;
        app.machine.jog.saved.push(machine_ui::SavedPos { name: format!("Position {n}"), x, y, z: z.unwrap_or(0.0), use_z: z.is_some() });
    }
    let mut action: Option<(usize, &str)> = None;
    for (i, sp) in app.machine.jog.saved.iter_mut().enumerate() {
        // The name takes the panel's width, so the buttons go on a line of their own (in one
        // row with it they pushed the panel wider than its dock).
        ui.push_id(("saved_position", i), |ui| ui_kit::field(ui, &mut sp.name));
        ui.horizontal_wrapped(|ui| {
            ui_kit::checkbox(ui, "Z", &mut sp.use_z);
            if btn(ui, "Go", "Move there", free) {
                action = Some((i, "go"));
            }
            if btn(ui, ui_kit::MOVE_UP, "Move up the list", i > 0) {
                action = Some((i, "up"));
            }
            if btn(ui, ui_kit::MOVE_DOWN, "Move down the list", true) {
                action = Some((i, "down"));
            }
            if btn(ui, "Delete", "Forget it", true) {
                action = Some((i, "del"));
            }
        });
    }
    if let Some((i, what)) = action {
        let n = app.machine.jog.saved.len();
        match what {
            "go" => {
                let sp = app.machine.jog.saved[i].clone();
                let mut a =
                    json!({ "x": sp.x, "y": sp.y, "feed": app.machine.jog.speed * 60.0, "machine_coords": app.machine.jog.from_machine_zero });
                if sp.use_z {
                    a["z"] = json!(sp.z);
                }
                go(app, "machine.move_to", a);
            }
            "up" if i > 0 => app.machine.jog.saved.swap(i, i - 1),
            "down" if i + 1 < n => app.machine.jog.saved.swap(i, i + 1),
            "del" => {
                app.machine.jog.saved.remove(i);
            }
            _ => {}
        }
    }
    if app.machine.jog.saved.len() > 1 && btn(ui, "Sort by name", "Sort the list", true) {
        app.machine.jog.saved.sort_by_key(|s| s.name.to_lowercase());
    }

    // Origin and finish [MOVE-13, MOVE-14]
    ui_kit::section_heading(ui, "Origin");
    ui.horizontal_wrapped(|ui| {
        if btn(ui, "Set origin", "Use the laser's position as the user origin", free && family == Family::Grbl) {
            go(app, "machine.set_origin", json!({}));
        }
        if btn(ui, "Clear origin", "Go back to the machine's own zero", free && family == Family::Grbl) {
            go(app, "machine.clear_origin", json!({}));
        }
        if btn(ui, "Set finish position", "Return here when a job ends", snap.has_position) {
            set_finish_here(app);
        }
        if app.machine.jog.finish.is_some() && btn(ui, "Clear finish", "Leave the head where the job ends", true) {
            app.machine.jog.finish = None;
        }
    });
    if family == Family::Ruida {
        ui.label(RichText::new("A DSP controller keeps its origin on its own panel.").size(t.font_small).color(t.text_dim));
    }
    if let Some((x, y)) = app.machine.jog.finish {
        let (unit, k) = if inches { ("in", 1.0 / 25.4) } else { ("mm", 1.0) };
        let text = format!("Finish position: X {:.2}, Y {:.2} {unit} from machine zero", x * k, y * k);
        ui.label(RichText::new(text).size(t.font_small).color(t.text_dim));
    }

    // Fire [MOVE-15]
    ui_kit::section_heading(ui, "Fire");
    if fire_allowed(app) {
        fire_button(ui, app, free);
    } else {
        ui.label(RichText::new("The Fire button is off for this device (Device Settings, G-code).").size(t.font_small).color(t.text_dim));
    }

    // Overrides [MOVE-17]
    if family == Family::Grbl {
        ui_kit::section_heading(ui, "While running");
        let o = snap.overrides;
        let (mut feed, mut power) = (if o.0 == 0 { 100 } else { o.0 }, if o.2 == 0 { 100 } else { o.2 });
        let (f0, p0) = (feed, power);
        ui_kit::row(ui, "Speed", |ui| {
            ui_kit::slider(
                ui,
                &mut feed,
                10..=200,
                ui_kit::Slider { step: Some(10.0), integer: true, format: |v| format!("{v:.0} %"), ..Default::default() },
            );
        });
        ui_kit::row(ui, "Power", |ui| {
            ui_kit::slider(
                ui,
                &mut power,
                10..=200,
                ui_kit::Slider { step: Some(10.0), integer: true, format: |v| format!("{v:.0} %"), ..Default::default() },
            );
        });
        if feed != f0 {
            go(app, "machine.overrides", json!({ "feed": feed }));
        }
        if power != p0 {
            go(app, "machine.overrides", json!({ "power": power }));
        }
        ui.horizontal(|ui| {
            ui.label("Rapid");
            for r in [100u16, 50, 25] {
                if btn(ui, &format!("{r} %"), "Rapid move override", snap.connected()) {
                    go(app, "machine.overrides", json!({ "rapid": r }));
                }
            }
        });
    }

    // Laser 2 [MOVE-18, CUTS-09]
    if family == Family::Ruida && dev.settings.dsp.dual_tube {
        laser2_controls(ui, app);
    }
}

/// A position typed into the Move to fields, in millimetres. A bare number is in the unit the
/// panel shows (inches with the inch setting on); a typed unit or a sum is worked out
/// (`10in/4`, `25 mm`). None for an empty field or text that is not a length.
pub fn typed_mm(text: &str, inches: bool) -> Option<f64> {
    let unit_mm = if inches { 25.4 } else { 1.0 };
    let text = text.replace('\u{2212}', "-");
    let v = darkburn_core::expr::eval_in(&text, unit_mm).or_else(|| ui_kit::parse_number(&text))?;
    v.is_finite().then_some(v * unit_mm)
}

/// The Position line: the laser's place in the unit the panel works in, with the unit named.
pub fn position_text(pos: Option<[f64; 4]>, inches: bool) -> String {
    let (unit, scale) = if inches { ("in", 1.0 / 25.4) } else { ("mm", 1.0) };
    match pos {
        Some(p) => format!("X {:.3}   Y {:.3}   Z {:.3} {unit}   U {:.3}", p[0] * scale, p[1] * scale, p[2] * scale, p[3]),
        None => format!("X ?   Y ?   Z ? {unit}   U ?"),
    }
}

/// "Set finish position": jobs end where the head is now. Kept as a place on the bed (machine
/// coordinates), so setting or clearing the origin later does not move it.
pub fn set_finish_here(app: &mut DarkBurn) {
    let snap = app.machine.handle.snapshot();
    if snap.has_position {
        app.machine.jog.finish = Some((snap.machine_pos[0], snap.machine_pos[1]));
    }
}

/// Click on an arrow: a step, or the start of a continuous jog.
fn jog_arrow(app: &mut DarkBurn, dx: i8, dy: i8, mult: f64) {
    let (ax, ay) = machine_ui::arrow_to_machine(dx, dy, app.machine.active(), app.machine.jog.invert_keypad);
    let feed = app.machine.jog.speed * 60.0;
    if app.machine.jog.continuous {
        go(app, "machine.jog_start", json!({ "x": ax, "y": ay, "feed": feed }));
    } else {
        let d = app.machine.jog.distance * mult;
        go(app, "machine.jog", json!({ "dx": f64::from(ax) * d, "dy": f64::from(ay) * d, "feed": feed }));
    }
}

/// Laser 2 power limits for dual-tube machines.
fn laser2_controls(ui: &mut Ui, app: &mut DarkBurn) {
    ui_kit::section_heading(ui, "Laser 2");
    let cached = app.machine.handle.snapshot().ruida.laser2;
    let connected = app.machine.handle.is_connected();
    // Nothing connected, or nothing read from this controller: no draft Write could send.
    if !connected || cached.is_none() {
        app.machine.dlg.laser2 = None;
    }
    if app.machine.dlg.laser2.is_none() {
        app.machine.dlg.laser2 = cached;
    }
    if let Some(l2) = app.machine.dlg.laser2.as_mut() {
        let (mut lo, mut hi) = (f64::from(l2.min_power) / 10.0, f64::from(l2.max_power) / 10.0);
        ui_kit::row(ui, "Min power", |ui| {
            ui_kit::number_field(ui, &mut lo, 0.0..=100.0, 0.5, " %");
        });
        ui_kit::row(ui, "Max power", |ui| {
            ui_kit::number_field(ui, &mut hi, 0.0..=100.0, 0.5, " %");
        });
        l2.min_power = (lo * 10.0).round() as u32;
        l2.max_power = (hi * 10.0).round().max(lo * 10.0) as u32;
    }
    ui.horizontal(|ui| {
        if btn(ui, "Read", "Read laser 2 settings from the controller", connected) {
            go(app, "machine.dsp_read", json!({ "what": "laser2" }));
            app.machine.dlg.laser2 = None;
        }
        if btn(ui, "Write", "Write laser 2 settings to the controller", connected && app.machine.dlg.laser2.is_some())
            && let Some(v) = app.machine.dlg.laser2.and_then(|l| serde_json::to_value(l).ok())
        {
            go(app, "machine.dsp_write", json!({ "what": "laser2", "values": v }));
        }
    });
}

// ----- Console ----------------------------------------------------------------------------------------

fn console(ui: &mut Ui, app: &mut DarkBurn) {
    let t = Tokens::get();
    let family = machine_ui::family_of(&app.machine);
    let h = (ui.available_height() - 120.0).clamp(80.0, 600.0);
    egui::ScrollArea::vertical().max_height(h).stick_to_bottom(true).auto_shrink([false, true]).show(ui, |ui| {
        for c in app.machine.handle.console() {
            let ink = match c.kind {
                ConsoleKind::Sent => t.text_dim,
                ConsoleKind::Received => t.form_ink,
                ConsoleKind::Note => t.accent,
            };
            let prefix = match c.kind {
                ConsoleKind::Sent => "> ",
                ConsoleKind::Received => "",
                ConsoleKind::Note => "# ",
            };
            ui.label(RichText::new(format!("{prefix}{}", c.text)).monospace().size(t.font_small).color(ink));
        }
    });
    let can_send = app.machine.handle.is_connected() && family == Family::Grbl;
    let mut send = false;
    ui.horizontal(|ui| {
        let r = ui.add_enabled(
            can_send,
            egui::TextEdit::singleline(&mut app.machine.console_input)
                .hint_text(if can_send { "Type a command" } else { "Console is for G-code machines" })
                .desired_width(ui.available_width() - 60.0),
        );
        crate::a11y::text(ui, &r, "Console command");
        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            send = true;
            r.request_focus();
        }
        if btn(ui, "Send", "Send the line", can_send && !app.machine.console_input.trim().is_empty()) {
            send = true;
        }
    });
    if send && !app.machine.console_input.trim().is_empty() {
        let line = std::mem::take(&mut app.machine.console_input);
        go(app, "machine.console", json!({ "line": line }));
    }
    ui.horizontal(|ui| {
        let mut show = app.machine.show_all;
        if ui_kit::checkbox(ui, "Show all", &mut show).changed() {
            go(app, "machine.show_all", json!({ "on": show }));
        }
        if btn(ui, "Clear", "Clear the log", true) {
            app.machine.handle.clear_console();
        }
    });
    // Six macros [CON-03, CON-04]
    ui.horizontal_wrapped(|ui| {
        for i in 0..6 {
            let text = app.machine.active().state.macros[i].clone();
            let label = macro_label(i, &text);
            // A button that does not fit moves to the next row; squeezed into what is left of this
            // one it would break its label in the middle of a word.
            let t = Tokens::get();
            let need = ui.painter().layout_no_wrap(label.clone(), egui::FontId::proportional(t.font_label), t.text).size().x + 2.0 * t.space_12;
            if ui.available_size_before_wrap().x < need {
                ui.end_row();
            }
            let r = ui.add_enabled_ui(can_send && !text.trim().is_empty(), |ui| {
                ui_kit::button(ui, label, Button::Small).backing_on_hover_text(if text.trim().is_empty() {
                    "Empty. Right-click to edit.".to_string()
                } else {
                    format!("{text}\n\nRight-click to edit.")
                })
            });
            if r.inner.clicked() {
                go(app, "machine.macro", json!({ "text": text }));
            }
            // Right-click edits, even when the button itself is greyed out.
            if r.response.secondary_clicked() || r.inner.secondary_clicked() {
                app.machine.dlg.macro_edit = Some((i, text));
            }
        }
    });
}

/// The label of macro button `i`: its first line, or "Macro n".
pub fn macro_label(i: usize, text: &str) -> String {
    let first = text.lines().map(str::trim).find(|l| !l.is_empty());
    match first {
        Some(l) if l.chars().count() > 12 => format!("{}...", l.chars().take(10).collect::<String>()),
        Some(l) => l.to_string(),
        None => format!("Macro {}", i + 1),
    }
}

// ----- File list ------------------------------------------------------------------------------------

fn file_list(ui: &mut Ui, app: &mut DarkBurn) {
    let t = Tokens::get();
    let snap = app.machine.handle.snapshot().clone();
    let dsp = machine_ui::family_of(&app.machine) == Family::Ruida;
    let ok = dsp && snap.connected();
    if !dsp {
        ui.label(
            RichText::new("Files stored on a DSP controller are listed here. The selected device is not one.").size(t.font_small).color(t.text_dim),
        );
    } else if !snap.connected() {
        ui.label(RichText::new("Connect to the controller to see its files.").size(t.font_small).color(t.text_dim));
    }
    let free = ok && snap.state == LaserState::Ready;
    // A tick is a file of the list shown: none carries over to a list not read yet (another
    // controller, a new connection) or one that no longer has that file.
    let listed = snap.ruida.files.as_deref().unwrap_or_default();
    app.machine.file_sel.retain(|i| listed.iter().any(|f| f.index == *i));
    ui.horizontal_wrapped(|ui| {
        if btn(ui, "Refresh", "Read the file list", ok) {
            go(app, "machine.file_list", json!({}));
        }
        if btn(ui, "Upload", "Send a .rd file to the controller", ok)
            && let Some(p) = rfd::FileDialog::new().add_filter("Ruida job", &["rd"]).pick_file()
        {
            go(app, "machine.file_upload", json!({ "path": p.to_string_lossy(), "start": false }));
        }
        let first = app.machine.file_sel.iter().next().copied();
        if btn(ui, "Start", "Run the selected file", free && first.is_some()) {
            go(app, "machine.file_run", json!({ "index": first.unwrap_or(1) }));
        }
        if btn(ui, "Delete", "Delete the selected files", ok && !app.machine.file_sel.is_empty()) {
            let v: Vec<u16> = app.machine.file_sel.iter().copied().collect();
            go(app, "machine.file_delete", json!({ "indices": v }));
            app.machine.file_sel.clear();
        }
        if btn(ui, "Delete all", "Delete every file on the controller", ok) {
            go(app, "machine.file_delete_all", json!({}));
            app.machine.file_sel.clear();
        }
        if btn(ui, "Calc time", "Ask the controller how long the selected file takes", ok && !app.machine.file_sel.is_empty()) {
            let v: Vec<u16> = app.machine.file_sel.iter().copied().collect();
            for i in v {
                go(app, "machine.file_calc_time", json!({ "index": i }));
            }
        }
    });
    if let Some((sent, total)) = snap.ruida.upload {
        ui_kit::progress(ui, "file_upload", sent as f32 / total.max(1) as f32);
    }
    let files = snap.ruida.files.clone().unwrap_or_default();
    if files.is_empty() && ok {
        ui.label(
            RichText::new(if snap.ruida.files.is_some() { "The controller holds no files." } else { "Press Refresh to read the list." })
                .size(t.font_small)
                .color(t.text_dim),
        );
    }
    egui::ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| {
        for f in &files {
            ui.horizontal(|ui| {
                let mut sel = app.machine.file_sel.contains(&f.index);
                if ui_kit::checkbox(ui, &format!("{}  {}", f.index, f.name), &mut sel).changed() {
                    if sel {
                        app.machine.file_sel.insert(f.index);
                    } else {
                        app.machine.file_sel.remove(&f.index);
                    }
                }
                if let Some(d) = snap.ruida.times.get(&f.index) {
                    ui.label(RichText::new(format_duration(*d)).size(t.font_small).color(t.text_dim));
                }
            });
        }
    });
}
