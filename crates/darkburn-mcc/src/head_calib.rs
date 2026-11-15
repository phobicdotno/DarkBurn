//! Head calibration run for Gweike M lasers (capture 2026-11-14, fiber head).
//!
//! Mlaser writes `0x65 <- [107]`, polls status while the card works, and when the run is over
//! writes `0x67 <- [3]` then `0x67 <- [1]`. This module is the pure state machine behind the
//! progress window: it decides what to send next from the elapsed time and the polled run
//! status. Cancel and the 120 s timeout both use the normal stop-all path.
//!
//! Open: 9.6 s into the captured run Mlaser also sent `0x65 <- [1, 0x1F, 2, 0x176F, 0x30D40]`
//! then `0x65 <- [0x76, 5, 0]`. What triggers that pair is not decoded, so it is not part of
//! this flow.

use crate::command::{self, MachineParams, SafeWrite};
use crate::registers::Forbidden;

/// The run stops itself and reports a failure after this long.
pub const TIMEOUT_MS: u64 = 120_000;

/// Where a calibration run is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CalibState {
    /// Started; waiting for the card to report busy and then idle again.
    Running,
    /// The card went idle after working: done.
    Finished,
    /// The user cancelled; stop-all was sent.
    Cancelled,
    /// No result inside [`TIMEOUT_MS`]; stop-all was sent.
    TimedOut,
}

/// One head calibration run.
#[derive(Clone, Debug)]
pub struct HeadCalibration {
    started_ms: u64,
    seen_busy: bool,
    state: CalibState,
}

impl HeadCalibration {
    /// Start a run at `now_ms`; returns the run and the write to send.
    pub fn start(now_ms: u64) -> Result<(HeadCalibration, SafeWrite), Forbidden> {
        Ok((HeadCalibration { started_ms: now_ms, seen_busy: false, state: CalibState::Running }, command::head_calibrate()?))
    }

    /// Current state.
    pub fn state(&self) -> &CalibState {
        &self.state
    }

    /// Milliseconds since the start.
    pub fn elapsed_ms(&self, now_ms: u64) -> u64 {
        now_ms.saturating_sub(self.started_ms)
    }

    /// Feed one status poll (`busy` = run status non-zero). Returns the writes to send now.
    pub fn on_status(&mut self, now_ms: u64, busy: bool, params: &MachineParams) -> Result<Vec<SafeWrite>, Forbidden> {
        if self.state != CalibState::Running {
            return Ok(Vec::new());
        }
        if self.elapsed_ms(now_ms) >= TIMEOUT_MS {
            self.state = CalibState::TimedOut;
            return Ok(vec![command::stop_all(0, params)?]);
        }
        if busy {
            self.seen_busy = true;
        } else if self.seen_busy {
            self.state = CalibState::Finished;
            return Ok(vec![command::fifo_stop()?, command::fifo_clear()?]);
        }
        Ok(Vec::new())
    }

    /// The user pressed Cancel. Returns the stop-all write, or nothing if the run is over.
    pub fn cancel(&mut self, params: &MachineParams) -> Result<Option<SafeWrite>, Forbidden> {
        if self.state != CalibState::Running {
            return Ok(None);
        }
        self.state = CalibState::Cancelled;
        Ok(Some(command::stop_all(0, params)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p() -> MachineParams {
        MachineParams::default()
    }

    #[test]
    fn start_sends_107() {
        let (c, w) = HeadCalibration::start(0).unwrap();
        assert_eq!(w.words(), &[107]);
        assert_eq!(c.state(), &CalibState::Running);
    }

    #[test]
    fn finishes_after_busy_then_idle() {
        let (mut c, _) = HeadCalibration::start(0).unwrap();
        assert!(c.on_status(100, false, &p()).unwrap().is_empty(), "idle before the card picked it up");
        assert!(c.on_status(500, true, &p()).unwrap().is_empty());
        let w = c.on_status(22_000, false, &p()).unwrap();
        assert_eq!(c.state(), &CalibState::Finished);
        assert_eq!(w.iter().map(|w| w.words().to_vec()).collect::<Vec<_>>(), vec![vec![3], vec![1]]);
    }

    #[test]
    fn timeout_stops() {
        let (mut c, _) = HeadCalibration::start(1000).unwrap();
        c.on_status(2000, true, &p()).unwrap();
        let w = c.on_status(1000 + TIMEOUT_MS, true, &p()).unwrap();
        assert_eq!(c.state(), &CalibState::TimedOut);
        assert_eq!(w[0].words()[..2], [1, 0x1F]);
    }

    #[test]
    fn cancel_stops_once() {
        let (mut c, _) = HeadCalibration::start(0).unwrap();
        let w = c.cancel(&p()).unwrap().unwrap();
        assert_eq!(w.words()[..2], [1, 0x1F]);
        assert_eq!(c.state(), &CalibState::Cancelled);
        assert!(c.cancel(&p()).unwrap().is_none());
        assert!(c.on_status(10, false, &p()).unwrap().is_empty());
    }
}
