//! Headless tests of the machine commands: everything a panel button does, driven through the
//! command registry against both emulators. No window and no real connection.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::api::execute_headless;
use crate::devices::{ConnectionKind, Device, DeviceType};
use crate::machine_api::{Family, LaserState, MachineCommand};
use crate::machine_ui::{self, Source, StartOutcome};
use crate::prefs::Prefs;
use crate::shell::DarkBurn;

const T: Duration = Duration::from_secs(20);

fn app() -> DarkBurn {
    DarkBurn::new(Prefs::default())
}

fn run(app: &mut DarkBurn, id: &str, args: Value) -> Result<Value, String> {
    execute_headless(app, id, &args)
}

fn wait(app: &mut DarkBurn, what: &str, f: impl Fn(&DarkBurn) -> bool) {
    let end = Instant::now() + T;
    loop {
        app.machine.tick();
        if f(app) {
            return;
        }
        assert!(Instant::now() < end, "timed out waiting for {what}; state {:?}", app.machine.handle.snapshot().state);
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn state(app: &DarkBurn) -> LaserState {
    app.machine.handle.snapshot().state
}

fn connect_sim(app: &mut DarkBurn, kind: DeviceType) {
    let id = app.machine.devices.devices.iter().find(|d| d.kind == kind).unwrap().id;
    run(app, "device.select", json!({ "id": id })).unwrap();
    run(app, "machine.connect", json!({})).unwrap();
    wait(app, "connected and ready", |a| state(a) == LaserState::Ready);
}

#[test]
fn nothing_connects_by_itself() {
    let mut a = app();
    let ctx = egui::Context::default();
    machine_ui::frame(&ctx, &mut a);
    machine_ui::frame(&ctx, &mut a);
    assert!(!a.machine.handle.is_connected());
    assert_eq!(state(&a), LaserState::Disconnected);
    assert!(a.machine.active().is_simulator(), "the simulator is the selected device");
    assert!(a.machine.active().name.contains("Simulator"));
}

#[test]
fn machine_commands_are_registered_and_flagged_hardware() {
    for id in [
        "machine.connect",
        "machine.start",
        "machine.pause",
        "machine.resume",
        "machine.stop",
        "machine.frame",
        "machine.home",
        "machine.go_to_origin",
        "machine.jog",
        "machine.move_to",
        "machine.set_origin",
        "machine.fire",
        "machine.console",
        "machine.write_setting",
        "machine.file_run",
        "laser.devices",
        "edit.device_settings",
        "edit.machine_settings",
        "laser.controller_info",
    ] {
        let info = crate::commands::describe(id).unwrap_or_else(|| panic!("{id} is not registered"));
        assert!(info.hardware, "{id} must be flagged hardware");
        assert!(info.implemented, "{id} works");
        assert!(!info.checklist.is_empty() || id.starts_with("laser.devices"), "{id} names checklist ids");
    }
    for id in ["device.list", "device.select", "device.import"] {
        assert!(!crate::commands::describe(id).unwrap().hardware, "{id} only edits the device list");
    }
}

#[test]
fn grbl_simulator_through_the_commands() {
    let mut a = app();
    assert_eq!(run(&mut a, "machine.status", Value::Null).unwrap()["state"], "Disconnected");
    assert!(run(&mut a, "machine.start", Value::Null).is_err(), "no job without a connection");
    assert!(run(&mut a, "machine.stop", Value::Null).is_err());
    connect_sim(&mut a, DeviceType::SimulatorGrbl);
    let s = run(&mut a, "machine.status", Value::Null).unwrap();
    assert_eq!(s["simulator"], true);
    assert_eq!(s["real_machine"], false);

    // Job: start, see progress, finish.
    run(&mut a, "machine.start", Value::Null).unwrap();
    wait(&mut a, "job finish", |a| a.machine.job_log.iter().any(|l| l.starts_with("Job finished")));
    wait(&mut a, "ready again", |a| state(a) == LaserState::Ready);

    // Jog, move, origin.
    let x0 = a.machine.handle.snapshot().work_pos[0];
    run(&mut a, "machine.jog", json!({ "dx": 12.0, "dy": 3.0, "feed": 6000 })).unwrap();
    wait(&mut a, "jog done", |a| (a.machine.handle.snapshot().work_pos[0] - x0 - 12.0).abs() < 0.01);
    wait(&mut a, "idle", |a| state(a) == LaserState::Ready);
    run(&mut a, "machine.set_origin", Value::Null).unwrap();
    run(&mut a, "machine.get_position", Value::Null).unwrap();
    wait(&mut a, "origin set", |a| a.machine.handle.snapshot().work_pos[0].abs() < 0.01);
    run(&mut a, "machine.move_to", json!({ "x": 5.0, "y": 5.0, "feed": 6000 })).unwrap();
    wait(&mut a, "moved", |a| (a.machine.handle.snapshot().work_pos[0] - 5.0).abs() < 0.01);
    wait(&mut a, "idle", |a| state(a) == LaserState::Ready);
    assert!(run(&mut a, "machine.move_to", json!({ "x": 99999.0, "feed": 100 })).unwrap_err().contains("Out of bounds"));
    run(&mut a, "machine.go_to_origin", Value::Null).unwrap();
    wait(&mut a, "at origin", |a| a.machine.handle.snapshot().work_pos[0].abs() < 0.01);
    wait(&mut a, "idle", |a| state(a) == LaserState::Ready);
    run(&mut a, "machine.clear_origin", Value::Null).unwrap();

    // Console and settings.
    run(&mut a, "machine.console", json!({ "line": "$I" })).unwrap();
    run(&mut a, "machine.macro", json!({ "text": "G21\nG90" })).unwrap();
    run(&mut a, "machine.read_settings", Value::Null).unwrap();
    wait(&mut a, "settings", |a| a.machine.handle.snapshot().grbl_settings.contains_key(&110));
    assert!(run(&mut a, "machine.write_setting", json!({ "id": 110, "value": "abc" })).is_err());
    assert!(run(&mut a, "machine.file_list", Value::Null).unwrap_err().contains("Not supported"));
    run(&mut a, "machine.disconnect", Value::Null).unwrap();
    assert_eq!(state(&a), LaserState::Disconnected);
}

#[test]
fn pause_resume_stop_through_the_commands() {
    let mut a = app();
    connect_sim(&mut a, DeviceType::SimulatorGrbl);
    let mut j = crate::machine_api::JobRequest::gcode(
        std::iter::once("G21 G90".to_owned())
            .chain(std::iter::once("M4 S100".to_owned()))
            .chain((0..60).map(|i| format!("G1 X{} Y10 F300", 10 + (i % 2) * 30)))
            .collect(),
    );
    j.name = "slow".into();
    a.machine.begin_start(j).unwrap();
    wait(&mut a, "busy", |a| state(a) == LaserState::Busy);
    assert!(a.machine.guard_close(), "closing during a job asks first");
    assert!(a.machine.dlg.confirm_close);
    run(&mut a, "machine.pause", Value::Null).unwrap();
    wait(&mut a, "paused", |a| state(a) == LaserState::Paused);
    run(&mut a, "machine.resume", Value::Null).unwrap();
    wait(&mut a, "busy again", |a| state(a) == LaserState::Busy);
    std::thread::sleep(Duration::from_millis(400));
    a.machine.stop_and_close();
    assert!(a.machine.close_ok && !a.machine.dlg.confirm_close);
    wait(&mut a, "stopped", |a| matches!(state(a), LaserState::Ready | LaserState::Alarm));
    assert!(!a.machine.guard_close(), "no job, nothing to ask");
}

#[test]
fn ruida_simulator_through_the_commands() {
    let mut a = app();
    connect_sim(&mut a, DeviceType::SimulatorRuida);
    let s = run(&mut a, "machine.status", Value::Null).unwrap();
    assert!(s["firmware"].as_str().unwrap().contains("RDC6442S"));
    run(&mut a, "machine.start", Value::Null).unwrap();
    wait(&mut a, "job finish", |a| a.machine.job_log.iter().any(|l| l.starts_with("Job finished")));
    run(&mut a, "machine.file_list", Value::Null).unwrap();
    wait(&mut a, "file list", |a| a.machine.handle.snapshot().ruida.files.is_some());
    run(&mut a, "machine.controller_info", Value::Null).unwrap();
    wait(&mut a, "info", |a| a.machine.handle.snapshot().ruida.info.is_some());
    run(&mut a, "machine.read_settings", Value::Null).unwrap();
    wait(&mut a, "settings", |a| a.machine.handle.snapshot().ruida.settings.is_some());
    run(&mut a, "machine.jog", json!({ "dx": 10.0, "dy": 5.0 })).unwrap();
    wait(&mut a, "jogged", |a| (a.machine.handle.snapshot().machine_pos[0] - 10.0).abs() < 0.01);
    run(&mut a, "machine.home", Value::Null).unwrap();
    run(&mut a, "machine.go_to_origin", Value::Null).unwrap();
    assert!(run(&mut a, "machine.console", json!({ "line": "?" })).is_err());
    assert!(run(&mut a, "machine.overrides", json!({ "feed": 120 })).is_err());
    assert!(run(&mut a, "machine.file_calc_time", json!({ "index": 1 })).is_ok());
    assert!(run(&mut a, "machine.file_run", json!({ "index": "x" })).is_err());
    run(&mut a, "machine.disconnect", Value::Null).unwrap();
}

#[test]
fn a_real_machine_needs_the_users_confirmation_and_is_refused_to_agents() {
    let mut a = app();
    let id = a.machine.devices.add(Device {
        name: "Real".into(),
        connection: ConnectionKind::Tcp,
        host: "192.168.1.77".into(),
        ..Device::new(DeviceType::Grbl)
    });
    a.machine.devices.select(id);
    // An agent may not connect it at all.
    let e = run(&mut a, "machine.connect", json!({})).unwrap_err();
    assert!(e.contains("simulator"), "{e}");
    assert!(a.machine.dlg.confirm_connect.is_none());
    // The UI asks first.
    assert!(machine_ui::run_with(&mut a, "machine.connect", &json!({})));
    assert!(a.machine.dlg.confirm_connect.is_some(), "the Connect confirmation is shown");
    assert!(!a.machine.handle.is_real_machine(), "nothing connected before the click");
    // Test builds never open a real link, even after the click.
    a.machine.confirm_connect();
    assert!(!a.machine.handle.is_real_machine());
    assert!(a.machine.message.as_ref().is_some_and(|m| m.error && m.text.contains("test")), "{:?}", a.machine.message);
    // And with no job builder a real machine gets no job.
    assert!(machine_ui::make_job(&a).is_err());
}

#[test]
fn design_only_devices_have_nothing_to_connect() {
    let mut a = app();
    let id = a.machine.devices.devices.iter().find(|d| d.is_design_only()).unwrap().id;
    run(&mut a, "device.select", json!({ "id": id })).unwrap();
    let e = run(&mut a, "machine.connect", json!({})).unwrap_err();
    assert!(e.contains("design only"), "{e}");
}

#[test]
fn the_pre_job_checklist_blocks_start_until_ticked() {
    let mut a = app();
    connect_sim(&mut a, DeviceType::SimulatorGrbl);
    let id = a.machine.devices.active.unwrap();
    {
        let d = a.machine.devices.get_mut(id).unwrap();
        d.settings.checklist_on = true;
        d.settings.checklist = vec!["one".into(), "two".into()];
    }
    let r = run(&mut a, "machine.start", Value::Null).unwrap();
    assert_eq!(r["waiting_for_checklist"], true);
    assert_eq!(state(&a), LaserState::Ready, "not started yet");
    assert!(a.machine.finish_checklist().is_err(), "unticked items block the start");
    a.machine.dlg.checklist.as_mut().unwrap().1 = vec![true, true];
    a.machine.finish_checklist().unwrap();
    wait(&mut a, "job under way", |a| matches!(state(a), LaserState::Busy) || a.machine.job_log.iter().any(|l| l.starts_with("Job finished")));
    let _ = StartOutcome::Started;
}

#[test]
fn device_commands_edit_the_list() {
    let mut a = app();
    let list = run(&mut a, "device.list", Value::Null).unwrap();
    let n = list["devices"].as_array().unwrap().len();
    let first = list["devices"][0]["id"].as_u64().unwrap();
    let dup = run(&mut a, "device.duplicate", json!({ "id": first })).unwrap()["id"].as_u64().unwrap();
    assert_eq!(run(&mut a, "device.list", Value::Null).unwrap()["devices"].as_array().unwrap().len(), n + 1);
    let text = run(&mut a, "device.export", json!({ "id": dup })).unwrap()["text"].as_str().unwrap().to_owned();
    let imported = run(&mut a, "device.import", json!({ "text": text })).unwrap()["id"].as_u64().unwrap();
    assert_ne!(imported, dup);
    assert!(run(&mut a, "device.import", json!({ "text": "{}" })).is_err());
    run(&mut a, "device.remove", json!({ "id": imported })).unwrap();
    assert!(run(&mut a, "device.remove", json!({ "id": 9999 })).is_err());
    assert!(run(&mut a, "device.select", json!({ "id": 9999 })).is_err());
}

#[test]
fn switching_device_adopts_its_remembered_state() {
    let mut a = app();
    machine_ui::adopt_device_state(&mut a);
    let mut d = Device::new(DeviceType::Grbl);
    d.name = "Inch machine".into();
    d.width_mm = 300.0;
    d.height_mm = 200.0;
    d.origin = darkburn_job::Corner::TopRight;
    d.state.inches = true;
    d.state.show_grid = false;
    d.state.panel_layout = serde_json::to_string(&a.prefs.panels).ok();
    let id = a.machine.devices.add(d);
    a.machine.select_device(id);
    machine_ui::adopt_device_state(&mut a);
    assert!(a.prefs.numeric_inches && !a.prefs.show_grid);
    assert_eq!((a.prefs.page_width, a.prefs.page_height), (300.0, 200.0));
    assert_eq!(a.prefs.origin, crate::view::Origin::RearRight);
    // Changing a pref while on it is remembered when leaving.
    a.prefs.numeric_inches = false;
    a.prefs.panels.get_mut("move").unwrap().visible = true;
    let first = a.machine.devices.devices[0].id;
    a.machine.select_device(first);
    machine_ui::adopt_device_state(&mut a);
    assert!(!a.machine.devices.get(id).unwrap().state.inches);
    assert!(a.machine.devices.get(id).unwrap().state.panel_layout.as_deref().unwrap().contains("move"));
    a.machine.select_device(id);
    machine_ui::adopt_device_state(&mut a);
    assert!(a.prefs.panel("move").visible);
}

#[test]
fn a_new_device_takes_over_the_current_units_and_panels() {
    let mut a = app();
    a.prefs.numeric_inches = true;
    a.prefs.panels.get_mut("move").unwrap().visible = true;
    machine_ui::adopt_device_state(&mut a);
    assert!(a.prefs.numeric_inches, "first use does not reset the user's units");
    assert!(a.prefs.panel("move").visible);
    let first = a.machine.active().clone();
    assert!(first.state.inches && first.state.panel_layout.is_some(), "and the device remembers them from then on");
}

#[test]
fn default_layers_save_and_apply() {
    let mut a = app();
    a.editor.doc.layers[3].name = "Keep me".into();
    assert!(machine_ui::save_default_layers(&mut a));
    a.editor.doc.layers[3].name = "Changed".into();
    assert!(machine_ui::apply_default_layers(&mut a));
    assert_eq!(a.editor.doc.layers[3].name, "Keep me");
}

#[test]
fn the_device_list_persists() {
    let dir = std::env::temp_dir().join(format!("darkburn-mui-{}", std::process::id()));
    let mut a = app();
    a.settings_dir = Some(dir.clone());
    let ctx = egui::Context::default();
    machine_ui::frame(&ctx, &mut a);
    let id = a.machine.devices.add(Device { name: "Saved one".into(), ..Device::new(DeviceType::Ruida) });
    machine_ui::frame(&ctx, &mut a);
    let mut b = app();
    b.settings_dir = Some(dir.clone());
    machine_ui::frame(&ctx, &mut b);
    assert!(b.machine.devices.get(id).is_some_and(|d| d.name == "Saved one"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn jog_arrows_follow_the_origin_corner() {
    use darkburn_job::Corner::*;
    assert_eq!(machine_ui::arrow_to_axes(1, 1, BottomLeft, false), (1, 1));
    assert_eq!(machine_ui::arrow_to_axes(1, 1, TopLeft, false), (1, -1));
    assert_eq!(machine_ui::arrow_to_axes(1, 1, BottomRight, false), (-1, 1));
    assert_eq!(machine_ui::arrow_to_axes(-1, 1, TopRight, false), (1, -1));
    assert_eq!(machine_ui::arrow_to_axes(0, 1, BottomLeft, true), (0, -1), "inverted keypad flips the vertical arrows");
    assert_eq!(machine_ui::jog_multiplier(false, false), 1.0);
    assert_eq!(machine_ui::jog_multiplier(true, false), 10.0);
    assert_eq!(machine_ui::jog_multiplier(false, true), 0.1);
    assert_eq!(machine_ui::jog_multiplier(true, true), 0.01);
}

#[test]
fn bad_arguments_say_why() {
    let mut a = app();
    connect_sim(&mut a, DeviceType::SimulatorGrbl);
    assert!(run(&mut a, "machine.frame", json!({ "x": 1 })).unwrap_err().contains("y"));
    assert!(run(&mut a, "machine.console", json!({})).unwrap_err().contains("line"));
    assert!(run(&mut a, "machine.write_setting", json!({ "id": 1 })).is_err());
    assert!(run(&mut a, "machine.fire", json!({ "percent": 99.0, "secs": 1.0 })).is_err(), "above the fire limit");
    run(&mut a, "machine.fire", json!({ "percent": 2.0, "secs": 0.3 })).unwrap();
    wait(&mut a, "fire on", |a| a.machine.handle.snapshot().fire_on);
    run(&mut a, "machine.fire_off", Value::Null).unwrap();
    wait(&mut a, "fire off", |a| !a.machine.handle.snapshot().fire_on);
    run(&mut a, "machine.frame", json!({ "x": 10, "y": 10, "width": 30, "height": 20 })).unwrap();
    wait(&mut a, "framing done", |a| state(a) == LaserState::Ready && a.machine.handle.snapshot().job.is_none());
    let _ = (Family::Grbl, MachineCommand::Stop, Source::Api);
}

/// Draw one frame of everything the machine side puts on screen.
fn draw(ctx: &egui::Context, app: &mut DarkBurn, events: Vec<egui::Event>) -> usize {
    let input =
        egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 1000.0))), events, ..Default::default() };
    let mut out = ctx.run_ui(input, |ui| {
        machine_ui::frame(ui.ctx(), app);
        for id in ["laser", "move", "console", "file_list"] {
            egui::ScrollArea::vertical().id_salt(id).show(ui, |ui| {
                crate::panels::contents(ui, id, app);
            });
        }
        crate::devices_ui::dialogs(ui.ctx(), app);
    });
    out.textures_delta.clear();
    out.shapes.len()
}

#[test]
fn every_panel_and_dialog_draws_in_every_state() {
    let ctx = egui::Context::default();
    let mut a = app();
    // Disconnected, simulator selected.
    assert!(draw(&ctx, &mut a, vec![]) > 0);
    // Connected GRBL, then every dialog open at once.
    connect_sim(&mut a, DeviceType::SimulatorGrbl);
    run(&mut a, "machine.read_settings", Value::Null).unwrap();
    wait(&mut a, "settings", |a| a.machine.handle.snapshot().grbl_settings.contains_key(&110));
    a.machine.dlg.devices = true;
    a.machine.dlg.wizard = Some(crate::devices::Wizard::new());
    a.machine.dlg.find = Some(crate::devices::find_my_laser(&[]));
    a.machine.dlg.settings = Some((a.machine.active().clone(), 0));
    a.machine.dlg.machine_settings = true;
    a.machine.dlg.controller_info = true;
    a.machine.dlg.macro_edit = Some((0, "G0 X1".into()));
    a.machine.dlg.confirm_close = true;
    a.machine.dlg.setting_edits.insert(32, "0".into());
    for _ in 0..3 {
        assert!(draw(&ctx, &mut a, vec![]) > 0);
    }
    for page in 0..4 {
        a.machine.dlg.settings = Some((a.machine.active().clone(), page));
        draw(&ctx, &mut a, vec![]);
    }
    for step in crate::devices::WizardStep::ALL {
        let mut w = crate::devices::Wizard::new();
        w.step = step;
        a.machine.dlg.wizard = Some(w);
        draw(&ctx, &mut a, vec![]);
    }
    // A job running.
    run(&mut a, "machine.start", Value::Null).unwrap();
    wait(&mut a, "busy", |a| a.machine.handle.snapshot().job.is_some() || a.machine.job_log.iter().any(|l| l.starts_with("Job finished")));
    draw(&ctx, &mut a, vec![]);
    // Connected Ruida with its file list, settings, dual tube and U axis dialogs.
    let id = a.machine.devices.devices.iter().find(|d| d.kind == DeviceType::SimulatorRuida).unwrap().id;
    a.machine.devices.get_mut(id).unwrap().settings.dsp.dual_tube = true;
    a.machine.devices.get_mut(id).unwrap().settings.dsp.u_axis = true;
    run(&mut a, "device.select", json!({ "id": id })).unwrap();
    run(&mut a, "machine.connect", json!({})).unwrap();
    wait(&mut a, "ruida ready", |a| state(a) == LaserState::Ready);
    for what in ["machine.file_list", "machine.read_settings", "machine.controller_info"] {
        run(&mut a, what, Value::Null).unwrap();
    }
    run(&mut a, "machine.dsp_read", json!({ "what": "laser2" })).unwrap();
    run(&mut a, "machine.dsp_read", json!({ "what": "u_axis" })).unwrap();
    wait(&mut a, "ruida data", |a| {
        let r = &a.machine.handle.snapshot().ruida;
        r.files.is_some() && r.settings.is_some() && r.info.is_some() && r.laser2.is_some() && r.u_axis.is_some()
    });
    a.machine.dlg.laser2 = None;
    a.machine.dlg.u_axis = None;
    for _ in 0..3 {
        assert!(draw(&ctx, &mut a, vec![]) > 0);
    }
    assert!(a.machine.dlg.laser2.is_some() && a.machine.dlg.u_axis.is_some(), "the dialogs picked up what the controller sent");
}

#[test]
fn the_banner_says_what_is_connected() {
    let mut a = app();
    assert!(!a.machine.handle.is_real_machine());
    connect_sim(&mut a, DeviceType::SimulatorGrbl);
    assert!(a.machine.handle.is_simulator() && !a.machine.handle.is_real_machine());
    assert_eq!(crate::laser_panel::mode_status(false, false, true), "Flat bed");
    assert!(crate::laser_panel::mode_status(true, false, false).contains("no rotary axis"));
    assert_eq!(crate::laser_panel::macro_label(2, ""), "Macro 3");
    assert_eq!(
        crate::laser_panel::macro_label(
            0,
            "
  G0 X10
G1"
        ),
        "G0 X10"
    );
    assert!(crate::laser_panel::macro_label(0, "a very long first line").ends_with("..."));
}

#[test]
fn the_frame_button_frames_the_selection_in_machine_coordinates() {
    use darkburn_core::kurbo::Rect;
    let mut a = app();
    assert!(crate::laser_panel::frame_args(&a).is_none(), "nothing drawn, nothing to frame");
    run(&mut a, "shape.add_rect", json!({ "x": 10, "y": 20, "width": 30, "height": 40 })).unwrap();
    let f = crate::laser_panel::frame_args(&a).unwrap();
    assert_eq!((f["x"].as_f64(), f["y"].as_f64(), f["width"].as_f64(), f["height"].as_f64()), (Some(10.0), Some(20.0), Some(30.0), Some(40.0)));
    // A machine with its zero at the top flips Y: the rectangle sits at 400 - 60.
    let id = a.machine.devices.devices.iter().find(|d| d.kind == DeviceType::SimulatorRuida).unwrap().id;
    a.machine.devices.select(id);
    let f = crate::laser_panel::frame_args(&a).unwrap();
    assert_eq!(f["y"].as_f64(), Some(400.0 - 60.0));
    let _ = Rect::ZERO;
}
