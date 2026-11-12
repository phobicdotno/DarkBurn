//! The poll thread: opens the receiver, reads key reports, sends LED changes, and opens the
//! receiver again after it is unplugged.
//!
//! LED messages are queued by the receiver and the pendant takes one per wake (a key press, or
//! its own wake every few seconds). So the thread sends only when the wanted LED state differs
//! from what it last sent, and never more often than [`PollOptions::led_min_gap`]: a burst of
//! changes ends up as one message with the last state.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::events::{KeyTracker, PendantEvent};
use crate::protocol::{Leds, decode_input, encode_leds};
use crate::transport::{Connector, PendantError, PendantTransport};

/// Timing of the poll thread.
#[derive(Clone, Copy, Debug)]
pub struct PollOptions {
    /// How long one read waits for a report. Also how quickly a stop is noticed.
    pub read_timeout: Duration,
    /// How long to wait before looking for the receiver again.
    pub retry_every: Duration,
    /// The shortest time between two LED messages.
    pub led_min_gap: Duration,
}

impl Default for PollOptions {
    fn default() -> Self {
        PollOptions { read_timeout: Duration::from_millis(50), retry_every: Duration::from_secs(2), led_min_gap: Duration::from_millis(250) }
    }
}

type Wake = Box<dyn Fn() + Send>;

/// A running pendant: events come out, the wanted LED state goes in. Dropping it stops the
/// thread and closes the receiver.
pub struct Pendant {
    events: Receiver<PendantEvent>,
    leds: Arc<AtomicU8>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Pendant {
    /// Starts the poll thread. `wake` is called after events were queued (the app uses it to
    /// redraw); it must be quick.
    pub fn start(connector: Box<dyn Connector>, opts: PollOptions, wake: Option<Wake>) -> Pendant {
        let (tx, rx) = mpsc::channel();
        let leds = Arc::new(AtomicU8::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let worker = Worker {
            connector,
            opts,
            tx,
            wake: Mutex::new(wake),
            leds: leds.clone(),
            stop: stop.clone(),
            tracker: KeyTracker::new(),
            last_error: None,
            sent: None,
            last_send: None,
            seq: 0,
        };
        let thread = std::thread::Builder::new().name("pendant".into()).spawn(move || worker.run()).ok();
        Pendant { events: rx, leds, stop, thread }
    }

    /// The LEDs the pendant should show. Sent when it changes; repeated calls with the same
    /// state send nothing.
    pub fn set_leds(&self, leds: Leds) {
        self.leds.store(leds.0 & Leds::MASK, Ordering::Relaxed);
    }

    /// Every event that has arrived, oldest first. Never waits.
    pub fn poll(&self) -> Vec<PendantEvent> {
        self.events.try_iter().collect()
    }

    /// Waits up to `timeout` for the next event.
    pub fn next_event(&self, timeout: Duration) -> Option<PendantEvent> {
        self.events.recv_timeout(timeout).ok()
    }

    /// Stops the thread and closes the receiver.
    pub fn stop(mut self) {
        self.shut_down();
    }

    fn shut_down(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Pendant {
    fn drop(&mut self) {
        self.shut_down();
    }
}

struct Worker {
    connector: Box<dyn Connector>,
    opts: PollOptions,
    tx: Sender<PendantEvent>,
    wake: Mutex<Option<Wake>>,
    leds: Arc<AtomicU8>,
    stop: Arc<AtomicBool>,
    tracker: KeyTracker,
    last_error: Option<PendantError>,
    /// The LED state the receiver last took; `None` after (re)connecting.
    sent: Option<Leds>,
    last_send: Option<Instant>,
    seq: u8,
}

impl Worker {
    fn emit(&self, events: impl IntoIterator<Item = PendantEvent>) {
        let mut any = false;
        for e in events {
            any = true;
            let _ = self.tx.send(e);
        }
        if any && let Some(w) = self.wake.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            w();
        }
    }

    fn run(mut self) {
        let mut link: Option<Box<dyn PendantTransport>> = None;
        let mut next_try = Instant::now();
        let mut buf = [0u8; 64];
        while !self.stop.load(Ordering::Relaxed) {
            let Some(t) = link.as_mut() else {
                let now = Instant::now();
                if now < next_try {
                    std::thread::sleep((next_try - now).min(self.opts.read_timeout));
                    continue;
                }
                match self.connector.connect() {
                    Ok(t) => {
                        self.last_error = None;
                        self.sent = None;
                        self.last_send = None;
                        self.emit([PendantEvent::Connected(t.name())]);
                        link = Some(t);
                    }
                    Err(e) => {
                        if self.last_error.as_ref() != Some(&e) {
                            self.last_error = Some(e.clone());
                            self.emit([PendantEvent::Unavailable(e)]);
                        }
                        next_try = Instant::now() + self.opts.retry_every;
                    }
                }
                continue;
            };
            let r = self.send_leds(t.as_mut()).and_then(|_| t.read_report(&mut buf, self.opts.read_timeout));
            match r {
                Ok(0) => {}
                Ok(n) => match decode_input(&buf[..n]) {
                    Ok(report) => {
                        if !report.unknown.is_empty() {
                            log::debug!("pendant: unknown key codes {:02x?}", report.unknown);
                        }
                        let ev = self.tracker.update(report.held);
                        self.emit(ev);
                    }
                    Err(e) => log::debug!("pendant: {e}"),
                },
                Err(e) => {
                    link = None;
                    let mut ev = self.tracker.release_all();
                    ev.push(PendantEvent::Disconnected(e.to_string()));
                    self.emit(ev);
                    next_try = Instant::now() + self.opts.retry_every;
                }
            }
        }
        // Closing: let go of anything still held, so nothing waits for a release.
        let ev = self.tracker.release_all();
        self.emit(ev);
    }

    /// Sends the wanted LED state if it changed and the last message is old enough.
    fn send_leds(&mut self, t: &mut dyn PendantTransport) -> Result<(), PendantError> {
        let want = Leds(self.leds.load(Ordering::Relaxed));
        if self.sent == Some(want) || self.last_send.is_some_and(|l| l.elapsed() < self.opts.led_min_gap) {
            return Ok(());
        }
        t.send_feature_report(&encode_leds(self.seq, want))?;
        self.seq = self.seq.wrapping_add(1);
        self.sent = Some(want);
        self.last_send = Some(Instant::now());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::PendantEvent::*;
    use super::*;
    use crate::protocol::Key;
    use crate::transport::{EmulatorHandle, NoConnector};

    fn fast() -> PollOptions {
        PollOptions { read_timeout: Duration::from_millis(5), retry_every: Duration::from_millis(20), led_min_gap: Duration::from_millis(30) }
    }

    /// Waits for the next event that matches `want`, skipping others; fails after 2 s.
    fn wait_for(p: &Pendant, want: impl Fn(&PendantEvent) -> bool) -> PendantEvent {
        let end = Instant::now() + Duration::from_secs(2);
        while Instant::now() < end {
            if let Some(e) = p.next_event(Duration::from_millis(20))
                && want(&e)
            {
                return e;
            }
        }
        panic!("event did not arrive");
    }

    fn wait_until(f: impl Fn() -> bool) {
        let end = Instant::now() + Duration::from_secs(2);
        while !f() {
            assert!(Instant::now() < end, "condition not met in time");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn keys_come_out_as_events() {
        let h = EmulatorHandle::new();
        let p = Pendant::start(Box::new(h.connector()), fast(), None);
        assert_eq!(wait_for(&p, |_| true), Connected("pendant emulator".into()));
        h.hold(&[Key::Start]);
        h.release();
        h.hold(&[Key::Fn]);
        h.hold(&[Key::Fn, Key::GoToOrigin]);
        h.release();
        let mut got = Vec::new();
        while got.len() < 6 {
            got.push(wait_for(&p, |_| true));
        }
        assert_eq!(
            got,
            vec![KeyDown(Key::Start), KeyUp(Key::Start), KeyDown(Key::Fn), Chord(Key::GoToOrigin), KeyUp(Key::GoToOrigin), KeyUp(Key::Fn)]
        );
    }

    #[test]
    fn leds_are_sent_once_per_change() {
        let h = EmulatorHandle::new();
        let p = Pendant::start(Box::new(h.connector()), fast(), None);
        wait_for(&p, |e| matches!(e, Connected(_)));
        // On connect the current state goes out once.
        wait_until(|| h.feature_reports().len() == 1);
        assert_eq!(h.feature_reports()[0], vec![0x06, 0xfe, 0xfd, 0x00, 0x00, 0x00, 0x00, 0x00]);
        // The same state again and again sends nothing.
        for _ in 0..20 {
            p.set_leds(Leds::NONE);
        }
        std::thread::sleep(Duration::from_millis(80));
        assert_eq!(h.feature_reports().len(), 1);
        p.set_leds(Leds::BLOW);
        wait_until(|| h.feature_reports().len() == 2);
        assert_eq!(h.feature_reports()[1], vec![0x06, 0xfe, 0xfd, 0x01, 0x02, 0x00, 0x00, 0x00]);
        std::thread::sleep(Duration::from_millis(80));
        assert_eq!(h.feature_reports().len(), 2, "nothing more without a change");
    }

    #[test]
    fn a_burst_of_led_changes_is_coalesced() {
        let h = EmulatorHandle::new();
        let opts = PollOptions { led_min_gap: Duration::from_millis(300), ..fast() };
        let p = Pendant::start(Box::new(h.connector()), opts, None);
        wait_for(&p, |e| matches!(e, Connected(_)));
        wait_until(|| h.feature_reports().len() == 1);
        // Many changes inside one gap: only the last state goes out after the gap.
        for l in [Leds::BLOW, Leds::FRAME, Leds::LASER, Leds::FRAME | Leds::BLOW] {
            p.set_leds(l);
            std::thread::sleep(Duration::from_millis(10));
        }
        wait_until(|| h.feature_reports().len() >= 2);
        std::thread::sleep(Duration::from_millis(350));
        let sent = h.feature_reports();
        assert_eq!(sent.len(), 2, "{sent:02x?}");
        assert_eq!(sent[1][4], 0x06);
        assert_eq!(sent[1][3], 0x01, "the sequence byte goes up by one per message");
    }

    #[test]
    fn unplug_releases_keys_and_reconnects() {
        let h = EmulatorHandle::new();
        let p = Pendant::start(Box::new(h.connector()), fast(), None);
        wait_for(&p, |e| matches!(e, Connected(_)));
        p.set_leds(Leds::FRAME);
        h.hold(&[Key::Up]);
        assert_eq!(wait_for(&p, |_| true), KeyDown(Key::Up));
        h.unplug();
        assert_eq!(wait_for(&p, |_| true), KeyUp(Key::Up), "a held key is let go when the receiver goes");
        assert!(matches!(wait_for(&p, |_| true), Disconnected(_)));
        assert_eq!(wait_for(&p, |_| true), Unavailable(PendantError::NotFound));
        let before = h.feature_reports().len();
        h.plug_in();
        assert!(matches!(wait_for(&p, |_| true), Connected(_)));
        // The LED state is sent again to the new link.
        wait_until(|| h.feature_reports().len() == before + 1);
        assert_eq!(h.feature_reports().last().unwrap()[4], 0x04);
        assert_eq!(h.connects(), 2);
    }

    #[test]
    fn permission_denied_is_reported_once() {
        let h = EmulatorHandle::new();
        h.deny_permission(true);
        let p = Pendant::start(Box::new(h.connector()), fast(), None);
        assert!(matches!(wait_for(&p, |_| true), Unavailable(PendantError::PermissionDenied { .. })));
        std::thread::sleep(Duration::from_millis(100));
        assert!(p.poll().is_empty(), "the same reason is not repeated");
        h.deny_permission(false);
        assert!(matches!(wait_for(&p, |_| true), Connected(_)));
    }

    #[test]
    fn stopping_closes_and_wake_is_called() {
        let woke = Arc::new(AtomicBool::new(false));
        let w = woke.clone();
        let p = Pendant::start(Box::new(NoConnector), fast(), Some(Box::new(move || w.store(true, Ordering::Relaxed))));
        assert_eq!(wait_for(&p, |_| true), Unavailable(PendantError::Unsupported));
        assert!(woke.load(Ordering::Relaxed));
        p.stop();
    }

    #[test]
    fn bad_reports_are_skipped() {
        let h = EmulatorHandle::new();
        let p = Pendant::start(Box::new(h.connector()), fast(), None);
        wait_for(&p, |e| matches!(e, Connected(_)));
        h.push_report(&[0x05, 0x01, 0x02]);
        h.push_report(&[0x04]);
        h.hold(&[Key::Stop]);
        assert_eq!(wait_for(&p, |_| true), KeyDown(Key::Stop));
    }
}
