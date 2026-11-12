//! The wireless handheld pendant (XHC receiver, USB 10ce:eba0), owner approved 2026-11-12.
//!
//! Off by default: the receiver is only opened after the user ticks "Use wireless pendant" in
//! Settings, and never in safe mode. The pendant has no actions of its own. Each key runs the
//! command an on-screen button runs, through the same command path, and only when that button
//! would be enabled. So every gate (connection, state, the Ruida start switch, the Gweike rules)
//! applies to the pendant exactly as to the buttons.
//!
//! Key map:
//! - Start: `job.start`, only when the Start button is enabled.
//! - Pause: `job.pause` (pause or resume, like the button). Stop: `job.stop`.
//! - Frame: `job.frame`. Go to origin: `job.go_origin`. Both only when the head may be jogged.
//! - Arrows: one step of the Move panel's distance and speed through `machine.jog`, the path
//!   the keyboard arrows use. Held FastStep makes the step ten times longer (the panel's Ctrl).
//! - Z+ / Z-: one step of the Z axis, only where the Move panel shows its Z buttons.
//! - Laser: nothing, on purpose. Manual fire stays an on-screen, press-and-hold action.
//! - TODO: Blow (air assist has no button yet), Follow, Shutter, Red light, Z calibrate, and
//!   every Fn chord (Fn+Go to origin, Fn+Z calibrate, Fn+Z-, Fn+Z+) have no DarkBurn action yet.
//!
//! LEDs: Frame is lit while framing, Blow while the controller reports air assist on. The rest
//! stay dark until their keys do something.
//!
//! Jogging is step only. A held, continuous jog from a wireless key could keep moving if the
//! release report were lost, so the pendant never starts one.

use std::cell::RefCell;

use darkburn_pendant::{Connector, Key, KeySet, Leds, Pendant, PendantError, PendantEvent, PollOptions};
use serde_json::json;

use crate::laser_panel::{jog_free, start_allowed};
use crate::machine_api::{Family, LaserState};
use crate::machine_ui::{self, run_with};
use crate::shell::DarkBurn;

/// Makes the connector the poll thread uses. Tests swap in the emulator.
pub type MakeConnector = Box<dyn Fn() -> Box<dyn Connector>>;

/// What the pendant is doing, for the Settings page.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum PendantStatus {
    #[default]
    Off,
    /// On, no receiver open yet.
    Looking,
    Connected(String),
    Unavailable(PendantError),
}

impl PendantStatus {
    pub fn text(&self) -> String {
        match self {
            PendantStatus::Off => "Off".into(),
            PendantStatus::Looking => "Looking for the receiver".into(),
            PendantStatus::Connected(name) => format!("Connected: {name}"),
            PendantStatus::Unavailable(e) => format!("Not available: {e}"),
        }
    }
}

#[derive(Default)]
pub struct PendantUi {
    pendant: Option<Pendant>,
    pub status: PendantStatus,
    /// The keys held right now, to know whether FastStep is down when an arrow goes down.
    held: KeySet,
    /// None: the real receiver (`darkburn_pendant::system_connector`).
    pub make_connector: Option<MakeConnector>,
}

impl PendantUi {
    pub fn running(&self) -> bool {
        self.pendant.is_some()
    }
}

thread_local! {
    static STATUS: RefCell<PendantStatus> = RefCell::new(PendantStatus::Off);
}

/// The pendant's status as of the last frame, for the Settings page (which only sees the prefs).
pub fn status() -> PendantStatus {
    STATUS.with(|s| s.borrow().clone())
}

/// The message for a receiver this user may not open (Linux without the udev rule).
pub fn permission_message() -> String {
    format!(
        "The pendant receiver is plugged in, but this user may not open it. Install the udev rule once (it is also in packaging/linux), then unplug and replug the receiver:\n{}",
        darkburn_pendant::linux_install_command()
    )
}

/// Once per frame: start or stop the poll thread with the setting, run the keys, set the LEDs.
pub fn frame(ctx: &egui::Context, app: &mut DarkBurn) {
    let want = app.prefs.opts.pendant && !app.safe_mode;
    if want && app.pendant.pendant.is_none() {
        let connector = match &app.pendant.make_connector {
            Some(make) => make(),
            None => darkburn_pendant::system_connector(),
        };
        let ctx2 = ctx.clone();
        app.pendant.pendant = Some(Pendant::start(connector, PollOptions::default(), Some(Box::new(move || ctx2.request_repaint()))));
        app.pendant.status = PendantStatus::Looking;
    } else if !want && let Some(p) = app.pendant.pendant.take() {
        p.stop();
        app.pendant.status = PendantStatus::Off;
        app.pendant.held = KeySet::EMPTY;
    }
    let events = app.pendant.pendant.as_ref().map(Pendant::poll).unwrap_or_default();
    for ev in events {
        on_event(app, ev);
    }
    if let Some(p) = &app.pendant.pendant {
        p.set_leds(leds(app));
    }
    STATUS.with(|s| *s.borrow_mut() = app.pendant.status.clone());
}

fn on_event(app: &mut DarkBurn, ev: PendantEvent) {
    match ev {
        PendantEvent::Connected(name) => {
            log::info!("pendant: {name}");
            app.pendant.status = PendantStatus::Connected(name);
        }
        PendantEvent::Disconnected(name) => {
            log::info!("pendant gone: {name}");
            app.pendant.status = PendantStatus::Looking;
            app.pendant.held = KeySet::EMPTY;
        }
        PendantEvent::Unavailable(e) => {
            log::info!("pendant: {e}");
            if matches!(e, PendantError::PermissionDenied { .. }) {
                app.machine.fail(permission_message());
            }
            app.pendant.status = PendantStatus::Unavailable(e);
        }
        PendantEvent::KeyDown(k) => {
            app.pendant.held.insert(k);
            key_down(app, k);
        }
        PendantEvent::Chord(k) => {
            app.pendant.held.insert(k);
            // TODO(owner): the printed Fn functions (Fn+Go to origin, Fn+Z calibrate, Fn+Z-,
            // Fn+Z+) have no DarkBurn action yet.
            log::debug!("pendant: Fn+{k} has no action");
        }
        PendantEvent::KeyUp(k) => {
            app.pendant.held.remove(k);
        }
    }
}

/// One key went down on its own. Everything goes through the buttons' command path and gates.
pub fn key_down(app: &mut DarkBurn, key: Key) {
    match key {
        Key::Start => {
            if start_allowed(app) {
                run_with(app, "job.start", &json!({}));
            }
        }
        // Pause and Stop, like their buttons, are never held back.
        Key::Pause => {
            run_with(app, "job.pause", &json!({}));
        }
        Key::Stop => {
            run_with(app, "job.stop", &json!({}));
        }
        Key::Frame => {
            if jog_free(app) {
                run_with(app, "job.frame", &json!({}));
            }
        }
        Key::GoToOrigin => {
            if jog_free(app) {
                run_with(app, "job.go_origin", &json!({}));
            }
        }
        Key::Left => jog(app, -1, 0),
        Key::Right => jog(app, 1, 0),
        Key::Up => jog(app, 0, 1),
        Key::Down => jog(app, 0, -1),
        Key::ZPlusW => jog_z(app, 1.0),
        Key::ZMinusW => jog_z(app, -1.0),
        // Manual fire: never from the pendant.
        Key::Laser => {}
        // FastStep is read as held by the arrows; Fn only makes chords.
        Key::FastStep | Key::Fn => {}
        // TODO(owner): no DarkBurn action yet. Blow could switch air assist (machine.air), but the
        // app has no air button, so the pendant does not get one first.
        Key::Blow | Key::Follow | Key::Shutter | Key::RedLight | Key::ZCalibrate => {}
    }
}

fn multiplier(app: &DarkBurn) -> f64 {
    machine_ui::jog_multiplier(app.pendant.held.contains(Key::FastStep), false)
}

/// One step, as the Move panel's arrow (step mode) and the keyboard arrows do.
fn jog(app: &mut DarkBurn, dx: i8, dy: i8) {
    if !jog_free(app) {
        return;
    }
    let (ax, ay) = machine_ui::arrow_to_machine(dx, dy, app.machine.active(), app.machine.jog.invert_keypad);
    let d = app.machine.jog.distance * multiplier(app);
    let feed = app.machine.jog.speed * 60.0;
    run_with(app, "machine.jog", &json!({ "dx": f64::from(ax) * d, "dy": f64::from(ay) * d, "feed": feed }));
}

/// One Z step, only where the Move panel shows its Z buttons.
fn jog_z(app: &mut DarkBurn, dir: f64) {
    let dev = app.machine.active();
    let z_shown = dev.settings.z_enabled || machine_ui::family_of(&app.machine) == Family::Ruida;
    if !z_shown || !jog_free(app) {
        return;
    }
    let rev = if dev.settings.z_reverse { -1.0 } else { 1.0 };
    let dz = dir * rev * app.machine.jog.distance * multiplier(app);
    let feed = app.machine.jog.z_speed * 60.0;
    run_with(app, "machine.jog", &json!({ "dz": dz, "feed": feed }));
}

/// The LEDs for what the app shows now.
pub fn leds(app: &DarkBurn) -> Leds {
    let snap = app.machine.handle.snapshot();
    let mut l = Leds::NONE;
    if snap.state == LaserState::Framing {
        l = l | Leds::FRAME;
    }
    if snap.connected() && snap.air_on {
        l = l | Leds::BLOW;
    }
    l
}
