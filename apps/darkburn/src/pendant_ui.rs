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
//!
//! Stop and Pause are carried out in [`logic`], which runs on every frame of the app, also
//! while the window is minimised or covered and nothing is drawn. Every other key waits for
//! [`frame`], which runs only when the window is drawn: with the window out of sight the
//! pendant stops and pauses, and does nothing else.
//!
//! A key press is only carried out while it is fresh. The poll thread stamps each press when
//! it reads it. If the app gets to a press more than [`MAX_AGE`] later (the window was hidden,
//! a file dialog was open, a long import ran), the press is dropped: nothing moves or starts
//! long after its key was pressed, perhaps with nobody at the machine any more. Stop is
//! carried out however old it is. A late Pause only ever pauses, it never sets a paused job
//! going again, and of several Pause presses that arrive together only the first counts.
//!
//! One window at a time: every window that opened the receiver would get every key press, so
//! one Start would start a job in each window and one arrow would move each machine. Before it
//! opens the receiver a window takes a lock on [`LOCK_FILE`] in the settings folder, and holds
//! it while the pendant is on. Another window with the pendant on shows that the pendant is in
//! use and tries again every [`LOCK_RETRY`], so it takes over when the first window closes or
//! turns the pendant off. The system lets go of the lock when a window ends, also in a crash.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::fs::{File, TryLockError};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use darkburn_pendant::{Connector, Key, KeySet, Leds, Pendant, PendantError, PendantEvent, PollOptions, Stamped};
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
    /// On, but another DarkBurn window has the receiver.
    InUse,
    /// On, but the lock that keeps other windows off the receiver could not be taken.
    NoLock(String),
}

impl PendantStatus {
    pub fn text(&self) -> String {
        match self {
            PendantStatus::Off => "Off".into(),
            PendantStatus::Looking => "Looking for the receiver".into(),
            PendantStatus::Connected(name) => format!("Connected: {name}"),
            PendantStatus::Unavailable(e) => format!("Not available: {e}"),
            PendantStatus::InUse => "In use by another DarkBurn window".into(),
            PendantStatus::NoLock(e) => format!("Not opened: no other DarkBurn window could be kept off the receiver ({e})"),
        }
    }
}

/// How old a key press may be when the app carries it out. Older presses are dropped, except
/// Stop (always carried out) and Pause (pauses, never resumes).
pub const MAX_AGE: Duration = Duration::from_millis(500);

/// A key press that waits for the next drawn frame.
struct Waiting {
    /// When the poll thread read the press.
    at: Instant,
    key: Key,
    /// Whether FastStep was held when the key went down.
    fast: bool,
}

/// The most key presses that wait for a drawn frame; the oldest is dropped beyond that.
const MAX_WAITING: usize = 16;

/// The lock file, in the settings folder, held by the one window that has the receiver open.
pub const LOCK_FILE: &str = "pendant.lock";

/// How often a window that wants the pendant tries again to take it from another window.
pub const LOCK_RETRY: Duration = Duration::from_secs(1);

pub struct PendantUi {
    pendant: Option<Pendant>,
    pub status: PendantStatus,
    /// The keys held right now, to know whether FastStep is down when an arrow goes down.
    held: KeySet,
    /// The key presses [`logic`] took and [`frame`] carries out: everything but Stop and Pause.
    waiting: VecDeque<Waiting>,
    /// None: the real receiver (`darkburn_pendant::system_connector`); in tests, no receiver.
    pub make_connector: Option<MakeConnector>,
    /// [`MAX_AGE`]; a field so that tests on a loaded computer can allow more.
    pub max_age: Duration,
    /// Held while this window has the receiver open: no other DarkBurn window opens it then.
    /// After `pendant`, so a window that goes away closes the receiver before it lets go.
    lock: Option<File>,
    /// When to try again for a lock another window holds.
    next_try: Option<Instant>,
    /// Where the lock is. None: [`LOCK_FILE`] in the settings folder, or in the system's
    /// temporary folder when there is no settings folder.
    pub lock_path: Option<PathBuf>,
}

impl Default for PendantUi {
    fn default() -> Self {
        PendantUi {
            pendant: None,
            status: PendantStatus::Off,
            held: KeySet::EMPTY,
            waiting: VecDeque::new(),
            make_connector: None,
            max_age: MAX_AGE,
            lock: None,
            next_try: None,
            lock_path: None,
        }
    }
}

#[cfg(test)]
impl PendantUi {
    pub fn running(&self) -> bool {
        self.pendant.is_some()
    }

    /// How many key presses wait for a drawn frame.
    pub fn waiting_len(&self) -> usize {
        self.waiting.len()
    }
}

thread_local! {
    static STATUS: RefCell<PendantStatus> = const { RefCell::new(PendantStatus::Off) };
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

/// Once per frame of the app, drawn or not: start or stop the poll thread with the setting, take
/// the keys, carry out Stop and Pause, set the LEDs.
///
/// A window that is minimised or covered is not drawn: only the app's logic runs then, and the
/// poll thread's wake-up reaches nothing else. So this is called from the app's logic, and Stop
/// and Pause are carried out here. The other keys wait for [`frame`].
pub fn logic(ctx: &egui::Context, app: &mut DarkBurn) {
    let want = app.prefs.opts.pendant && !app.safe_mode;
    if want && app.pendant.pendant.is_none() && owner(ctx, app) {
        let connector = match &app.pendant.make_connector {
            Some(make) => make(),
            // Tests never reach the real receiver, even with every setting switched on.
            None if cfg!(test) => Box::new(darkburn_pendant::NoConnector),
            None => darkburn_pendant::system_connector(),
        };
        let ctx2 = ctx.clone();
        app.pendant.pendant = Some(Pendant::start(connector, PollOptions::default(), Some(Box::new(move || ctx2.request_repaint()))));
        app.pendant.status = PendantStatus::Looking;
    } else if !want {
        if let Some(p) = app.pendant.pendant.take() {
            p.stop();
            app.pendant.held = KeySet::EMPTY;
            app.pendant.waiting.clear();
        }
        // After the receiver is closed: now another window may open it.
        app.pendant.lock = None;
        app.pendant.next_try = None;
        app.pendant.status = PendantStatus::Off;
    }
    let events = app.pendant.pendant.as_ref().map(Pendant::poll_stamped).unwrap_or_default();
    let now = Instant::now();
    // Of the Pause presses that arrive together only the first counts: two in one go are a
    // replay or a radio drop-out, and the second would set the job going again.
    let mut paused = false;
    for Stamped { at, event } in events {
        let late = now.saturating_duration_since(at) > app.pendant.max_age;
        on_event(app, event, at, late, &mut paused);
    }
    if let Some(p) = &app.pendant.pendant {
        p.set_leds(leds(app));
    }
    STATUS.with(|s| *s.borrow_mut() = app.pendant.status.clone());
}

/// Whether this window may open the receiver: it holds the lock, or has just taken it. Tries
/// at most once per [`LOCK_RETRY`] and wakes the window for the next try.
fn owner(ctx: &egui::Context, app: &mut DarkBurn) -> bool {
    if app.pendant.lock.is_some() {
        return true;
    }
    let now = Instant::now();
    if app.pendant.next_try.is_some_and(|t| now < t) {
        return false;
    }
    let path = app.pendant.lock_path.clone().unwrap_or_else(|| app.settings_dir.clone().unwrap_or_else(std::env::temp_dir).join(LOCK_FILE));
    match take_lock(&path) {
        Ok(Some(f)) => {
            app.pendant.lock = Some(f);
            app.pendant.next_try = None;
            return true;
        }
        Ok(None) => app.pendant.status = PendantStatus::InUse,
        Err(e) => {
            log::warn!("pendant: no lock: {e}");
            app.pendant.status = PendantStatus::NoLock(e.to_string());
        }
    }
    app.pendant.next_try = Some(now + LOCK_RETRY);
    ctx.request_repaint_after(LOCK_RETRY);
    false
}

/// The lock on `path`, or None while another window (another process) holds it.
fn take_lock(path: &Path) -> std::io::Result<Option<File>> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let f = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path)?;
    match f.try_lock() {
        Ok(()) => Ok(Some(f)),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(e)) => Err(e),
    }
}

/// Once per drawn frame, after [`logic`]: carry out the key presses that waited for the window
/// and are still fresh.
pub fn frame(app: &mut DarkBurn) {
    let now = Instant::now();
    while let Some(w) = app.pendant.waiting.pop_front() {
        let age = now.saturating_duration_since(w.at);
        if age > app.pendant.max_age {
            log::info!("pendant: {} was pressed {} ms ago and is not carried out", w.key, age.as_millis());
            continue;
        }
        key_down(app, w.key, w.fast);
    }
}

/// `at` is when the poll thread read the event, `late` whether that is more than the allowed
/// age ago, `paused` whether a Pause was already carried out in this go.
fn on_event(app: &mut DarkBurn, ev: PendantEvent, at: Instant, late: bool, paused: &mut bool) {
    match ev {
        PendantEvent::Connected(name) => {
            log::info!("pendant: {name}");
            app.pendant.status = PendantStatus::Connected(name);
        }
        PendantEvent::Disconnected(name) => {
            log::info!("pendant gone: {name}");
            app.pendant.status = PendantStatus::Looking;
            app.pendant.held = KeySet::EMPTY;
            app.pendant.waiting.clear();
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
            if matches!(k, Key::Stop | Key::Pause) {
                // Nothing pressed before a Stop or a Pause is carried out after it.
                app.pendant.waiting.clear();
                if k == Key::Stop {
                    // However old: a Stop is always carried out.
                    key_down(app, k, false);
                } else if !*paused {
                    *paused = true;
                    pause(app, late);
                }
            } else {
                if app.pendant.waiting.len() >= MAX_WAITING {
                    app.pendant.waiting.pop_front();
                }
                let fast = app.pendant.held.contains(Key::FastStep);
                app.pendant.waiting.push_back(Waiting { at, key: k, fast });
            }
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

/// The Pause key. A press the app got to late pauses a running job and nothing else: on a job
/// that is already paused the button's command would set it going again.
fn pause(app: &mut DarkBurn, late: bool) {
    if late {
        app.machine.tick();
        if app.machine.handle.snapshot().state == LaserState::Paused {
            log::info!("pendant: a late Pause is not carried out on a paused job");
            return;
        }
    }
    key_down(app, Key::Pause, false);
}

/// One key went down on its own; `fast` is whether FastStep was held then. Everything goes
/// through the buttons' command path and gates.
fn key_down(app: &mut DarkBurn, key: Key, fast: bool) {
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
        Key::Left => jog(app, -1, 0, fast),
        Key::Right => jog(app, 1, 0, fast),
        Key::Up => jog(app, 0, 1, fast),
        Key::Down => jog(app, 0, -1, fast),
        Key::ZPlusW => jog_z(app, 1.0, fast),
        Key::ZMinusW => jog_z(app, -1.0, fast),
        // Manual fire: never from the pendant.
        Key::Laser => {}
        // FastStep is read as held by the arrows; Fn only makes chords.
        Key::FastStep | Key::Fn => {}
        // TODO(owner): no DarkBurn action yet. Blow could switch air assist (machine.air), but the
        // app has no air button, so the pendant does not get one first.
        Key::Blow | Key::Follow | Key::Shutter | Key::RedLight | Key::ZCalibrate => {}
    }
}

/// One step, as the Move panel's arrow (step mode) and the keyboard arrows do.
fn jog(app: &mut DarkBurn, dx: i8, dy: i8, fast: bool) {
    if !jog_free(app) {
        return;
    }
    let (ax, ay) = machine_ui::arrow_to_machine(dx, dy, app.machine.active(), app.machine.jog.invert_keypad);
    let d = app.machine.jog.distance * machine_ui::jog_multiplier(fast, false);
    let feed = app.machine.jog.speed * 60.0;
    run_with(app, "machine.jog", &json!({ "dx": f64::from(ax) * d, "dy": f64::from(ay) * d, "feed": feed }));
}

/// One Z step, only where the Move panel shows its Z buttons.
fn jog_z(app: &mut DarkBurn, dir: f64, fast: bool) {
    let dev = app.machine.active();
    let z_shown = dev.settings.z_enabled || machine_ui::family_of(&app.machine) == Family::Ruida;
    if !z_shown || !jog_free(app) {
        return;
    }
    let rev = if dev.settings.z_reverse { -1.0 } else { 1.0 };
    let dz = dir * rev * app.machine.jog.distance * machine_ui::jog_multiplier(fast, false);
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
