//! The link to the receiver, and the in-memory emulator every test uses.
//!
//! The poll thread only talks through [`PendantTransport`], and finds a receiver through a
//! [`Connector`]. Tests and demos use [`EmulatorConnector`]. The real receiver is reached only
//! through `HidConnector` (cargo feature `hid`), which the app creates when the user turns the
//! pendant on.

use std::collections::VecDeque;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use crate::protocol::{INPUT_REPORT_ID, Key};

/// Why the pendant cannot be used right now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PendantError {
    /// No receiver is plugged in.
    NotFound,
    /// The receiver is there but this user may not open it (Linux without the udev rule).
    /// `path` is the device node, for example `/dev/hidraw3`.
    PermissionDenied { path: String },
    /// This build has no HID support (the `hid` feature is off).
    Unsupported,
    /// The link failed: unplugged, or an error from the system.
    Io(String),
}

impl fmt::Display for PendantError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PendantError::NotFound => f.write_str("no pendant receiver is plugged in"),
            PendantError::PermissionDenied { path } => write!(f, "no permission to open the pendant receiver ({path})"),
            PendantError::Unsupported => f.write_str("this build of DarkBurn has no pendant support"),
            PendantError::Io(e) => write!(f, "pendant receiver error: {e}"),
        }
    }
}

impl std::error::Error for PendantError {}

/// A link to an open receiver. Moved into the poll thread, so it must be `Send`.
pub trait PendantTransport: Send {
    /// Reads one input report (report id first) into `buf`, waiting at most `timeout`.
    /// `Ok(0)` on timeout. An `Err` means the receiver is gone.
    fn read_report(&mut self, buf: &mut [u8], timeout: Duration) -> Result<usize, PendantError>;
    /// Sends a feature report (report id first).
    fn send_feature_report(&mut self, data: &[u8]) -> Result<(), PendantError>;
    /// A human name for messages, for example `XHC pendant 10ce:eba0`.
    fn name(&self) -> String;
}

/// Finds and opens a receiver. The poll thread calls it again after an unplug.
pub trait Connector: Send {
    fn connect(&mut self) -> Result<Box<dyn PendantTransport>, PendantError>;
}

// ----- Emulator -----------------------------------------------------------------------------------

#[derive(Default)]
struct Shared {
    plugged: bool,
    /// Set by `deny_permission`: connect fails as on Linux without the udev rule.
    denied: bool,
    /// Bumped on every unplug, so a transport from before it fails.
    generation: u64,
    reports: VecDeque<Vec<u8>>,
    feature_reports: Vec<Vec<u8>>,
    connects: usize,
}

/// The test side of an emulated receiver: press keys, unplug it, read back the LED reports.
#[derive(Clone, Default)]
pub struct EmulatorHandle(Arc<Mutex<Shared>>);

impl EmulatorHandle {
    /// A receiver that is plugged in.
    pub fn new() -> Self {
        let h = Self::default();
        h.lock().plugged = true;
        h
    }

    fn lock(&self) -> MutexGuard<'_, Shared> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The connector the poll thread uses to reach this receiver.
    pub fn connector(&self) -> EmulatorConnector {
        EmulatorConnector(self.clone())
    }

    /// Queues a raw input report (report id first).
    pub fn push_report(&self, report: &[u8]) {
        self.lock().reports.push_back(report.to_vec());
    }

    /// Queues the report for these keys held (at most three, as on the device).
    pub fn hold(&self, keys: &[Key]) {
        assert!(keys.len() <= 3, "the receiver reports at most three keys");
        let mut r = [INPUT_REPORT_ID, 0x10, 0, 0, 0, 0];
        for (i, k) in keys.iter().enumerate() {
            r[2 + i] = k.code();
        }
        r[5] = 0x10u8.wrapping_sub(r[2]);
        self.push_report(&r);
    }

    /// Queues the all-zero report: nothing held.
    pub fn release(&self) {
        self.hold(&[]);
    }

    /// Pulls the receiver out: open links fail from now on and connect finds nothing.
    pub fn unplug(&self) {
        let mut s = self.lock();
        s.plugged = false;
        s.generation += 1;
        s.reports.clear();
    }

    /// Puts the receiver back.
    pub fn plug_in(&self) {
        self.lock().plugged = true;
    }

    /// Makes connect fail with [`PendantError::PermissionDenied`], or work again.
    pub fn deny_permission(&self, denied: bool) {
        self.lock().denied = denied;
    }

    /// Every feature report sent so far, in order.
    pub fn feature_reports(&self) -> Vec<Vec<u8>> {
        self.lock().feature_reports.clone()
    }

    /// How many times a link was opened.
    pub fn connects(&self) -> usize {
        self.lock().connects
    }
}

/// Opens [`EmulatorTransport`]s on an [`EmulatorHandle`].
pub struct EmulatorConnector(EmulatorHandle);

impl Connector for EmulatorConnector {
    fn connect(&mut self) -> Result<Box<dyn PendantTransport>, PendantError> {
        let mut s = self.0.lock();
        if !s.plugged {
            return Err(PendantError::NotFound);
        }
        if s.denied {
            return Err(PendantError::PermissionDenied { path: "/dev/hidraw-emulator".into() });
        }
        s.connects += 1;
        let generation = s.generation;
        drop(s);
        Ok(Box::new(EmulatorTransport { handle: self.0.clone(), generation }))
    }
}

/// An open link to the emulated receiver.
pub struct EmulatorTransport {
    handle: EmulatorHandle,
    generation: u64,
}

impl EmulatorTransport {
    fn alive(&self, s: &Shared) -> Result<(), PendantError> {
        if s.plugged && s.generation == self.generation { Ok(()) } else { Err(PendantError::Io("the receiver was unplugged".into())) }
    }
}

impl PendantTransport for EmulatorTransport {
    fn read_report(&mut self, buf: &mut [u8], timeout: Duration) -> Result<usize, PendantError> {
        {
            let mut s = self.handle.lock();
            self.alive(&s)?;
            if let Some(r) = s.reports.pop_front() {
                let n = r.len().min(buf.len());
                buf[..n].copy_from_slice(&r[..n]);
                return Ok(n);
            }
        }
        std::thread::sleep(timeout.min(Duration::from_millis(5)));
        Ok(0)
    }

    fn send_feature_report(&mut self, data: &[u8]) -> Result<(), PendantError> {
        let mut s = self.handle.lock();
        self.alive(&s)?;
        s.feature_reports.push(data.to_vec());
        Ok(())
    }

    fn name(&self) -> String {
        "pendant emulator".into()
    }
}

/// A connector that never finds anything: what a build without the `hid` feature has.
pub struct NoConnector;

impl Connector for NoConnector {
    fn connect(&mut self) -> Result<Box<dyn PendantTransport>, PendantError> {
        Err(PendantError::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emulator_round_trip_and_unplug() {
        let h = EmulatorHandle::new();
        let mut c = h.connector();
        let mut t = c.connect().unwrap();
        let mut buf = [0u8; 16];
        assert_eq!(t.read_report(&mut buf, Duration::ZERO).unwrap(), 0);
        h.hold(&[Key::Fn, Key::Up]);
        assert_eq!(t.read_report(&mut buf, Duration::ZERO).unwrap(), 6);
        assert_eq!(&buf[..6], &[0x04, 0x10, 0x0b, 0x0f, 0x00, 0x05]);
        t.send_feature_report(&[6, 0xfe, 0xfd, 0, 1, 0, 0, 0]).unwrap();
        assert_eq!(h.feature_reports().len(), 1);
        h.unplug();
        assert!(t.read_report(&mut buf, Duration::ZERO).is_err());
        assert!(t.send_feature_report(&[6]).is_err());
        assert_eq!(c.connect().err(), Some(PendantError::NotFound));
        h.plug_in();
        // The old link stays dead; a new one works.
        assert!(t.read_report(&mut buf, Duration::ZERO).is_err());
        assert!(c.connect().is_ok());
        assert_eq!(h.connects(), 2);
    }

    #[test]
    fn permission_denied_is_its_own_error() {
        let h = EmulatorHandle::new();
        h.deny_permission(true);
        assert!(matches!(h.connector().connect().err(), Some(PendantError::PermissionDenied { .. })));
        assert_eq!(NoConnector.connect().err(), Some(PendantError::Unsupported));
    }
}
