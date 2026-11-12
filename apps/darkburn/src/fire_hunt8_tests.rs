//! Pass 8 of the bug hunt: the Fire (hold) button of the Laser panel, pressed for real in a
//! headless window against the simulators. No real connection.
//!
//! The button is a click-only widget: egui stops calling it held once the press can no longer be
//! a click (0.8 s, or the pointer moved a few points). So one press fires for less than the
//! shortest fire time, and these tests guard that a press never fires twice.

use std::time::{Duration, Instant};

use egui::Pos2;
use serde_json::json;

use crate::devices::DeviceType;
use crate::machine_api::LaserState;
use crate::ui_harness::Gui;

const T: Duration = Duration::from_secs(20);
const KINDS: [DeviceType; 2] = [DeviceType::SimulatorGrbl, DeviceType::SimulatorRuida];

fn fire_on(g: &Gui) -> bool {
    g.app().machine.handle.snapshot().fire_on
}

fn ready(g: &Gui) -> bool {
    g.app().machine.handle.snapshot().state == LaserState::Ready
}

/// Draw frames until `f` holds.
#[track_caller]
fn until(g: &mut Gui, what: &str, mut f: impl FnMut(&Gui) -> bool) {
    let end = Instant::now() + T;
    while !f(g) {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        g.frames(1);
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Draw frames for `time`; true when the laser was on in any of them.
fn fired_within(g: &mut Gui, time: Duration) -> bool {
    let end = Instant::now() + time;
    let mut fired = false;
    while Instant::now() < end {
        g.frames(1);
        fired |= fire_on(g);
        std::thread::sleep(Duration::from_millis(5));
    }
    fired
}

fn connect(g: &mut Gui) {
    g.exec("machine.connect", json!({}));
    until(g, "connected and ready", ready);
}

/// A window with the simulator `kind` connected and the Laser panel open; the middle of its
/// Fire (hold) button.
fn rig(kind: DeviceType) -> (Gui, Pos2) {
    let mut g = Gui::new(1600.0, 1400.0, 1.0);
    let id = g.app().machine.devices.devices.iter().find(|d| d.kind == kind).unwrap().id;
    g.exec("device.select", json!({ "id": id }));
    connect(&mut g);
    let area = g.app().prefs.panel("laser").area;
    if g.rect_of("panel:tab:laser").is_some() {
        g.click_id("panel:tab:laser");
    } else {
        g.click_id(&format!("panel:overflow:{area:?}"));
        g.click_label("Laser");
    }
    g.frames(2);
    for _ in 0..25 {
        let r = g.find_texts("Fire (hold)").into_iter().next_back().unwrap_or_else(|| panic!("no Fire button: {:?}", g.visible_texts()));
        if r.bottom() < g.screen().bottom() - 40.0 && r.top() > 40.0 {
            return (g, r.center());
        }
        let over = g.rect_of("dialog:button::Connect").map_or(r.center(), |c| c.center());
        g.scroll(over, egui::vec2(0.0, -120.0));
    }
    panic!("the Fire button never came into view");
}

/// Press the button and see the laser come on.
#[track_caller]
fn press(g: &mut Gui, at: Pos2, kind: DeviceType) {
    assert!(!fire_on(g), "{kind:?}: on before the press");
    g.press_pos(at);
    until(g, "the laser on under the held button", fire_on);
}

#[test]
fn the_fire_button_fires_while_held_and_stops_on_release() {
    for kind in KINDS {
        let (mut g, at) = rig(kind);
        press(&mut g, at, kind);
        g.release_pos(at);
        until(&mut g, "the laser off after the release", |g| !fire_on(g));
        assert!(!fired_within(&mut g, Duration::from_millis(300)), "{kind:?}: on again after the release");
        // a second press fires again
        press(&mut g, at, kind);
        // and a release with the pointer somewhere else ends it too
        let away = at + egui::vec2(0.0, -200.0);
        g.hover_pos(away);
        g.release_pos(away);
        until(&mut g, "the laser off after a release away from the button", |g| !fire_on(g));
        assert!(!fired_within(&mut g, Duration::from_millis(300)), "{kind:?}: on again after the release");
    }
}

#[test]
fn a_button_that_stays_down_does_not_fire_again_once_the_laser_went_off() {
    for kind in KINDS {
        let (mut g, at) = rig(kind);
        press(&mut g, at, kind);
        // The button stays down: the laser goes off by itself and stays off.
        until(&mut g, "the laser off under the button", |g| !fire_on(g));
        assert!(!fired_within(&mut g, Duration::from_millis(1500)), "{kind:?}: the held button fired a second time");
        g.release_pos(at);
        assert!(!fired_within(&mut g, Duration::from_millis(200)), "{kind:?}");
        // a new press is a new hold
        press(&mut g, at, kind);
        g.release_pos(at);
        until(&mut g, "the laser off after the release", |g| !fire_on(g));
    }
}

#[test]
fn a_button_still_held_over_a_reconnect_does_not_fire_on_the_new_link() {
    for kind in KINDS {
        let (mut g, at) = rig(kind);
        press(&mut g, at, kind);
        g.exec("machine.disconnect", json!({}));
        until(&mut g, "the link down", |g| !g.app().machine.handle.is_connected());
        assert!(!fire_on(&g), "{kind:?}: the fire flag outlived the link");
        connect(&mut g);
        assert!(!fired_within(&mut g, Duration::from_millis(600)), "{kind:?}: the held button fired on the new link");
        g.release_pos(at);
        assert!(!fired_within(&mut g, Duration::from_millis(200)), "{kind:?}");
    }
}
