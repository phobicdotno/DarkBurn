//! Headless tests of the Move window commands in `motion_cmds.rs` (Focus Z, the finish position,
//! the rotary jog and test rotation) and of taking a print and cut target from the head, driven
//! through the command registry against the simulators. No window and no real connection.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::api::execute_headless;
use crate::devices::DeviceType;
use crate::machine_api::{ConsoleKind, LaserState};
use crate::prefs::Prefs;
use crate::shell::DarkBurn;

const T: Duration = Duration::from_secs(20);
const IDS: [&str; 5] = ["machine.focus_z", "machine.set_finish", "machine.clear_finish", "machine.jog_rotary", "machine.rotary_test"];

fn app() -> DarkBurn {
    DarkBurn::new(Prefs::default())
}

fn run(app: &mut DarkBurn, id: &str, args: Value) -> Result<Value, String> {
    execute_headless(app, id, &args)
}

fn wait(app: &mut DarkBurn, what: &str, mut f: impl FnMut(&DarkBurn) -> bool) {
    let end = Instant::now() + T;
    loop {
        app.machine.tick();
        if f(app) {
            return;
        }
        assert!(Instant::now() < end, "timed out waiting for {what}; state {:?}, at {:?}", state(app), app.machine.handle.snapshot().machine_pos);
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

/// Ready, with the head at `pos` (machine coordinates; `None` skips an axis).
fn at(app: &DarkBurn, pos: [Option<f64>; 4]) -> bool {
    let s = app.machine.handle.snapshot();
    state(app) == LaserState::Ready && pos.iter().zip(s.machine_pos).all(|(want, got)| want.is_none_or(|w| (w - got).abs() < 1e-3))
}

fn xy(x: f64, y: f64) -> [Option<f64>; 4] {
    [Some(x), Some(y), None, None]
}

fn sent(app: &DarkBurn) -> Vec<String> {
    app.machine.handle.console().iter().filter(|c| c.kind == ConsoleKind::Sent).map(|c| c.text.clone()).collect()
}

/// A rotary on `axis` that turns 20 axis units per turn of a 200 mm round object: 10 mm along
/// the surface is one unit.
fn rotary_on(app: &mut DarkBurn, axis: &str) {
    run(
        app,
        "rotary.set",
        json!({ "enabled": true, "kind": "chuck", "axis": axis, "steps_per_rotation": 1600, "axis_steps_per_mm": 80, "circumference": 200 }),
    )
    .unwrap();
}

#[test]
fn the_move_commands_are_registered_hardware_with_their_checklist_items() {
    for id in IDS {
        let c = crate::commands::find(id).unwrap_or_else(|| panic!("{id} is in the registry"));
        assert!(!c.label.is_empty());
        assert!(crate::commands::registry().iter().any(|r| r.id == id), "{id} is listed");
        assert!(crate::commands::hardware(id), "{id} counts as hardware");
        assert!(crate::machine_ui::handles(id), "{id} is run by the machine dispatcher");
        assert!(crate::mcp_catalog::hardware_class(id).is_some(), "{id} has a hardware class");
    }
    assert_eq!(crate::commands::checklist("machine.focus_z"), &["MOVE-12"]);
    assert_eq!(crate::commands::checklist("machine.set_finish"), &["MOVE-14"]);
    assert_eq!(crate::commands::checklist("machine.clear_finish"), &["MOVE-14"]);
    assert_eq!(crate::commands::checklist("machine.jog_rotary"), &["MOVE-03"]);
    assert_eq!(crate::commands::checklist("machine.rotary_test"), &["ROT-06"]);
    use darkburn_mcp::HardwareClass;
    assert_eq!(crate::mcp_catalog::hardware_class("machine.focus_z"), Some(HardwareClass::Motion));
    assert_eq!(crate::mcp_catalog::hardware_class("machine.jog_rotary"), Some(HardwareClass::Motion));
    assert_eq!(crate::mcp_catalog::hardware_class("machine.rotary_test"), Some(HardwareClass::Motion));
    assert_eq!(crate::mcp_catalog::hardware_class("machine.set_finish"), Some(HardwareClass::Config));
}

#[test]
fn focus_z_runs_the_ruida_focus_routine_to_the_focus_distance() {
    let mut a = app();
    connect_sim(&mut a, DeviceType::SimulatorRuida);
    let v = run(&mut a, "machine.focus_z", json!({})).unwrap();
    assert_eq!(v["routine"], "controller");
    // The simulator's Z settings keep a 5 mm focus distance.
    wait(&mut a, "Z at the focus distance", |a| at(a, [None, None, Some(5.0), None]));
}

#[test]
fn focus_z_on_grbl_moves_z_to_the_height_given() {
    let mut a = app();
    connect_sim(&mut a, DeviceType::SimulatorGrbl);
    let e = run(&mut a, "machine.focus_z", json!({})).unwrap_err();
    assert!(e.contains("no focus routine"), "{e}");
    run(&mut a, "machine.move_to", json!({ "x": 30, "y": 40 })).unwrap();
    wait(&mut a, "the move", |a| at(a, xy(30.0, 40.0)));
    run(&mut a, "machine.focus_z", json!({ "z": 3.5, "feed": 600 })).unwrap();
    wait(&mut a, "Z at 3.5 mm", |a| at(a, [Some(30.0), Some(40.0), Some(3.5), None]));
}

#[test]
fn the_finish_position_is_set_from_the_head_or_given_and_cleared() {
    let mut a = app();
    let e = run(&mut a, "machine.set_finish", json!({})).unwrap_err();
    assert!(e.contains("not reported where it is"), "{e}");
    run(&mut a, "machine.set_finish", json!({ "x": 12.5, "y": 30 })).unwrap();
    assert_eq!(a.machine.jog.finish, Some((12.5, 30.0)));
    let e = run(&mut a, "machine.set_finish", json!({ "x": 5 })).unwrap_err();
    assert!(e.contains("both x and y"), "{e}");
    assert_eq!(a.machine.jog.finish, Some((12.5, 30.0)), "a refused change keeps the old one");
    run(&mut a, "machine.clear_finish", json!({})).unwrap();
    assert_eq!(a.machine.jog.finish, None);

    // From the head: a place on the bed, not in work coordinates.
    connect_sim(&mut a, DeviceType::SimulatorGrbl);
    run(&mut a, "machine.move_to", json!({ "x": 50, "y": 60 })).unwrap();
    wait(&mut a, "the move", |a| at(a, xy(50.0, 60.0)));
    run(&mut a, "machine.set_origin", json!({})).unwrap();
    wait(&mut a, "the origin", |a| state(a) == LaserState::Ready && a.machine.handle.snapshot().work_pos[0].abs() < 1e-6);
    run(&mut a, "machine.move_to", json!({ "x": 5, "y": 6 })).unwrap();
    wait(&mut a, "the move", |a| at(a, xy(55.0, 66.0)));
    let v = run(&mut a, "machine.set_finish", json!({})).unwrap();
    assert_eq!(v["finish"]["x"], 55.0);
    assert_eq!(a.machine.jog.finish, Some((55.0, 66.0)));
}

#[test]
fn the_rotary_jog_turns_the_object_by_the_distance_along_its_surface() {
    let mut a = app();
    connect_sim(&mut a, DeviceType::SimulatorGrbl);
    run(&mut a, "machine.move_to", json!({ "x": 50, "y": 60 })).unwrap();
    wait(&mut a, "the move", |a| at(a, xy(50.0, 60.0)));
    let e = run(&mut a, "machine.jog_rotary", json!({ "distance": 10 })).unwrap_err();
    assert!(e.contains("switch the rotary on"), "{e}");
    rotary_on(&mut a, "Y");
    let v = run(&mut a, "machine.jog_rotary", json!({ "distance": 10, "feed": 1200 })).unwrap();
    assert_eq!(v["axis"], "Y");
    assert!((v["units"].as_f64().unwrap() - 1.0).abs() < 1e-9, "{v}");
    wait(&mut a, "one unit on Y", |a| at(a, xy(50.0, 61.0)));
    run(&mut a, "machine.jog_rotary", json!({ "distance": -30 })).unwrap();
    wait(&mut a, "three units back", |a| at(a, xy(50.0, 58.0)));
    // Mirrored, the object turns the other way.
    run(&mut a, "rotary.set", json!({ "mirror": true })).unwrap();
    run(&mut a, "machine.jog_rotary", json!({ "distance": 20 })).unwrap();
    wait(&mut a, "mirrored", |a| at(a, xy(50.0, 56.0)));
    assert!(run(&mut a, "machine.jog_rotary", json!({ "distance": 0 })).is_err());
    // Jobs turn the rotary on Y (or U), so another axis letter is refused, and nothing moves.
    rotary_on(&mut a, "A");
    let e = run(&mut a, "machine.jog_rotary", json!({ "distance": 10 })).unwrap_err();
    assert!(e.contains("axis A"), "{e}");
    // U is a DSP controller's axis: GRBL says it has none.
    rotary_on(&mut a, "U");
    assert!(run(&mut a, "machine.jog_rotary", json!({ "distance": 10 })).is_err());
    assert!(at(&a, xy(50.0, 56.0)));
}

#[test]
fn the_rotary_jog_on_a_ruida_turns_the_u_axis() {
    let mut a = app();
    connect_sim(&mut a, DeviceType::SimulatorRuida);
    rotary_on(&mut a, "U");
    let u0 = a.machine.handle.snapshot().machine_pos[3];
    run(&mut a, "machine.jog_rotary", json!({ "distance": 50 })).unwrap();
    wait(&mut a, "five units on U", |a| at(a, [None, None, None, Some(u0 + 5.0)]));
}

#[test]
fn a_test_rotation_turns_the_object_and_brings_it_back() {
    let mut a = app();
    connect_sim(&mut a, DeviceType::SimulatorGrbl);
    run(&mut a, "machine.show_all", json!({ "on": true })).unwrap();
    run(&mut a, "machine.move_to", json!({ "x": 50, "y": 60 })).unwrap();
    wait(&mut a, "the move", |a| at(a, xy(50.0, 60.0)));
    rotary_on(&mut a, "Y");
    let before = a.vision.state.rotary;
    // The settings in the arguments are the ones tried (the Rotary Setup window sends what it
    // shows), and the project's setup is left as it was.
    let v = run(&mut a, "machine.rotary_test", json!({ "revolutions": 0.5, "feed": 3000, "axis_steps_per_mm": 40 })).unwrap();
    assert_eq!(v["sent"], true);
    assert_eq!(v["plan"]["moves"][0]["distance"], 20.0, "{v}");
    assert_eq!(v["plan"]["moves"][1]["distance"], -20.0, "{v}");
    assert!(a.vision.state.rotary == before);
    wait(&mut a, "both turns", |a| sent(a).iter().filter(|l| l.starts_with("$J=")).count() >= 2);
    let jogs: Vec<String> = sent(&a).into_iter().filter(|l| l.starts_with("$J=")).collect();
    let words = |l: &str| l.split_whitespace().map(str::to_string).collect::<Vec<_>>();
    assert!(words(&jogs[jogs.len() - 2]).contains(&"Y20".to_string()), "{jogs:?}");
    assert!(words(&jogs[jogs.len() - 1]).contains(&"Y-20".to_string()), "{jogs:?}");
    wait(&mut a, "back at the start", |a| at(a, xy(50.0, 60.0)));
    assert!(run(&mut a, "machine.rotary_test", json!({ "revolutions": 0 })).is_err());
}

#[test]
fn a_target_taken_from_the_head_is_its_bed_position() {
    let mut a = app();
    // Not connected: there is no head to take it from.
    let e = run(&mut a, "laser.pnc_first", json!({ "design_x": 0, "design_y": 0 })).unwrap_err();
    assert!(e.contains("connect the laser"), "{e}");
    connect_sim(&mut a, DeviceType::SimulatorGrbl);
    run(&mut a, "machine.move_to", json!({ "x": 50, "y": 60 })).unwrap();
    wait(&mut a, "the move", |a| at(a, xy(50.0, 60.0)));
    run(&mut a, "machine.set_origin", json!({})).unwrap();
    wait(&mut a, "the origin", |a| state(a) == LaserState::Ready && a.machine.handle.snapshot().work_pos[0].abs() < 1e-6);
    run(&mut a, "machine.move_to", json!({ "x": 20, "y": 20 })).unwrap();
    wait(&mut a, "the move", |a| at(a, xy(70.0, 80.0)));
    // GRBL reports the work position as well; the target is kept from machine zero.
    let v = run(&mut a, "laser.pnc_first", json!({ "design_x": 0, "design_y": 0 })).unwrap();
    assert_eq!((v["laser"]["x"].as_f64(), v["laser"]["y"].as_f64()), (Some(70.0), Some(80.0)), "{v}");
    assert_eq!(a.vision.state.pnc.measured[0], Some(darkburn_core::kurbo::Point::new(70.0, 80.0)));
    // Placed with the red dot: the beam is the pointer offset away from it.
    let id = a.machine.devices.active.unwrap();
    let d = a.machine.devices.devices.iter_mut().find(|d| d.id == id).unwrap();
    d.settings.red_dot_x = 1.5;
    d.settings.red_dot_y = -2.0;
    let v = run(&mut a, "laser.pnc_second", json!({ "pointer": true, "design_x": 100, "design_y": 0 })).unwrap();
    assert_eq!((v["laser"]["x"].as_f64(), v["laser"]["y"].as_f64()), (Some(71.5), Some(78.0)), "{v}");
    // Given coordinates are kept as given.
    let v = run(&mut a, "laser.pnc_first", json!({ "x": 1, "y": 2 })).unwrap();
    assert_eq!((v["laser"]["x"].as_f64(), v["laser"]["y"].as_f64()), (Some(1.0), Some(2.0)));
}
