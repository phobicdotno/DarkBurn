//! The Move window commands through the interface, on the simulator: the Move panel's rotary
//! row, Rotary Setup's Run test and print and cut targets taken where the head stands.
//! See docs/UI-TESTING.md.
use std::time::{Duration, Instant};

use serde_json::json;

use crate::machine_api::LaserState;
use crate::ui_harness::Gui;

fn until(g: &mut Gui, what: &str, f: impl Fn(&Gui) -> bool) {
    let end = Instant::now() + Duration::from_secs(20);
    while !f(g) {
        assert!(Instant::now() < end, "timed out waiting for {what}: {:?}", g.app().machine.handle.snapshot().machine_pos);
        std::thread::sleep(Duration::from_millis(10));
        g.frames(2);
    }
}

fn at(g: &Gui, x: f64, y: f64) -> bool {
    let app = g.app();
    let s = app.machine.handle.snapshot();
    s.state == LaserState::Ready && (s.machine_pos[0] - x).abs() < 1e-3 && (s.machine_pos[1] - y).abs() < 1e-3
}

fn connected_at(g: &mut Gui, x: f64, y: f64) {
    g.run_command("machine.connect");
    until(g, "the simulator to connect", |g| g.app().machine.handle.snapshot().state == LaserState::Ready);
    g.exec("machine.move_to", json!({ "x": x, "y": y }));
    until(g, "the move", |g| at(g, x, y));
}

fn show_move_panel(g: &mut Gui) {
    if !g.app().prefs.panel("move").visible {
        g.run_command("panel.move");
    }
    let area = g.app().prefs.panel("move").area;
    g.app().prefs.active_tab.insert(area, "move".to_string());
    g.frames(3);
}

/// With the project's rotary on, the Move panel turns the object by the jog distance measured
/// along its surface [MOVE-03]; Rotary Setup's Run test turns it and back [ROT-06].
#[test]
fn the_move_panel_turns_the_rotary_and_rotary_setup_runs_a_test() {
    let mut g = Gui::new(1600.0, 1200.0, 1.0);
    connected_at(&mut g, 50.0, 60.0);
    show_move_panel(&mut g);
    assert!(!g.has_text("Turn +"), "no rotary row while the rotary is off");
    g.exec(
        "rotary.set",
        json!({ "enabled": true, "kind": "chuck", "axis": "Y", "steps_per_rotation": 1600, "axis_steps_per_mm": 80, "circumference": 200 }),
    );
    g.app().machine.jog.distance = 10.0;
    g.frames(3);
    assert!(g.has_text("Rotary (Y)"), "the rotary row is shown: {:?}", g.visible_texts());
    g.screenshot("move-panel-rotary");
    g.click_label("Turn +");
    // 10 mm along a 200 mm surface is 1/20 of a turn: one axis unit here.
    until(&mut g, "one unit on Y", |g| at(g, 50.0, 61.0));
    g.click_label("Turn \u{2212}");
    until(&mut g, "back", |g| at(g, 50.0, 60.0));

    g.run_command("laser.rotary");
    g.settle();
    assert!(g.dialog_open());
    g.click_label("Run test");
    g.frames(3);
    assert!(g.visible_texts().iter().any(|t| t.starts_with("Test rotation sent")), "Run test reports: {:?}", g.visible_texts());
    g.screenshot("rotary-setup-run-test");
    until(&mut g, "the turn there and back", |g| at(g, 50.0, 60.0));
    assert!(g.app().machine.handle.console().iter().any(|c| c.text.contains("Y20")), "the test turn was sent");
}

/// With the laser connected, Set First Target takes the head's bed position, as the controller
/// reports it from machine zero, even with an origin set [LT-01, PNC-01]. The Print and Cut
/// window has the red dot box for heads placed with the pointer.
#[test]
fn set_first_target_takes_where_the_head_stands() {
    let mut g = Gui::new(1600.0, 1200.0, 1.0);
    g.sample_design();
    connected_at(&mut g, 50.0, 60.0);
    g.exec("machine.set_origin", json!({}));
    until(&mut g, "the origin", |g| g.app().machine.handle.snapshot().work_pos[0].abs() < 1e-6);
    g.exec("machine.move_to", json!({ "x": 10, "y": 15 }));
    until(&mut g, "the move", |g| at(g, 60.0, 75.0));
    g.run_command("laser.pnc_first");
    let note = g.app().note.clone().unwrap_or_default();
    assert!(note.starts_with("First target set where the laser is: X 60.00, Y 75.00"), "{note}");
    let p = g.app().vision.state.pnc.measured[0];
    assert_eq!(p, Some(darkburn_core::kurbo::Point::new(60.0, 75.0)));
    g.run_command("laser.print_and_cut");
    g.settle();
    assert!(g.has_text("Placed with the red dot (adds the pointer offset)"), "{:?}", g.visible_texts());
    g.screenshot("print-and-cut-red-dot");
}
