//! Head calibration run for Gweike M lasers (capture 2026-11-14, fiber head).
//!
//! Mlaser writes `0x65 <- [107]`, polls status while the card works, and when the run is over
//! writes `0x67 <- [3]` then `0x67 <- [1]`. This module is the pure state machine behind the
//! progress window: it decides what to send next from the elapsed time and the polled status.
//! Cancel, the 120 s timeout and a follower alarm all use the normal stop-all path.
//!
//! Progress comes from the follower status block at 10000 (capture of the real machine): word 3
//! (run command) reads 107 while the follower calibrates and drops to 0 when it is done (about
//! 22 s); a non-zero word 1 (follower alarm) means the run failed. Mlaser grades nothing from a
//! decoded register, so a finished run is only "done". A card without a follower block falls
//! back to the MCC run status word (busy, then idle).
//!
//! **Superblock (owner, 2026-11-16):** a calibration started while the FTC reports an error can
//! crack the ceramic ring. [`HeadCalibration::start`] therefore needs a fresh follower status read
//! taken just before the start and refuses outright, with no override, when any follower alarm
//! bit is set ([`crate::registers::zf_alarm`], FTC alarm included) or when no follower status
//! could be read at all. [`command::head_calibrate`] is private to this crate, so nothing else
//! can send `0x65 <- [107]`.
//!
//! Open: 9.6 s into the captured run Mlaser also sent `0x65 <- [1, 0x1F, 2, 0x176F, 0x30D40]`
//! then `0x65 <- [0x76, 5, 0]`. What triggers that pair is not decoded, so it is not part of
//! this flow.

use crate::command::{self, MachineParams, SafeWrite};
use crate::registers::Forbidden;

/// Why a head calibration was not started. There is no override for any of these.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StartBlocked {
    /// The follower/FTC reports an error (alarm bits, `word 1 & 0xFFFF` of block 10000).
    FtcError {
        /// The alarm bits that were set.
        alarm: u32,
    },
    /// No follower status was read before the start, so an FTC error cannot be ruled out.
    NoFollowerStatus,
    /// The write itself was refused by the register rules.
    Forbidden(Forbidden),
}

impl StartBlocked {
    /// Plain text for the user.
    pub fn text(&self) -> String {
        match self {
            StartBlocked::FtcError { alarm } => format!(
                "Head calibration blocked: the FTC reports an error ({}). Calibrating with an FTC error standing can crack the ceramic. Clear the error on the machine first.",
                crate::registers::zf_alarm::names(*alarm).join(", ")
            ),
            StartBlocked::NoFollowerStatus => {
                "Head calibration blocked: the FTC status could not be read, so an FTC error cannot be ruled out.".into()
            }
            StartBlocked::Forbidden(f) => format!("Head calibration blocked: {f:?}"),
        }
    }
}

/// The run stops itself and reports a failure after this long.
pub const TIMEOUT_MS: u64 = 120_000;

/// Where a calibration run is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CalibState {
    /// Started; waiting for the card to report busy and then idle again.
    Running,
    /// The card went idle after working: done.
    Finished,
    /// The follower raised an alarm (bits of its alarm word, see
    /// [`crate::registers::zf_alarm`]); stop-all was sent.
    Failed {
        /// Follower alarm bits (`word 1 & 0xFFFF`).
        alarm: u32,
    },
    /// The user cancelled; stop-all was sent.
    Cancelled,
    /// No result inside [`TIMEOUT_MS`]; stop-all was sent.
    TimedOut,
}

/// What one status poll says about the run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CalibInput {
    /// The card has a follower block: its run command (word 3) and alarm word (word 1).
    Follower {
        /// Word 3 of block 10000: 107 while calibrating.
        run_command: u32,
        /// Word 1 of block 10000 (low 16 bits are the alarm bits).
        alarm: u32,
    },
    /// No follower block: the MCC run status word is non-zero.
    RunStatus {
        /// Run status non-zero.
        busy: bool,
    },
}

impl CalibInput {
    /// Read the input from a follower status block.
    pub fn from_follower(z: &crate::registers::ZfStatus) -> CalibInput {
        use crate::registers::ZfWord;
        CalibInput::Follower { run_command: z.word(ZfWord::RunCommand), alarm: z.word(ZfWord::Alarm) }
    }
}

/// Follower run command while the follower calibrates.
pub const RUN_COMMAND_CALIBRATE: u32 = 107;

/// One head calibration run.
#[derive(Clone, Debug)]
pub struct HeadCalibration {
    started_ms: u64,
    seen_busy: bool,
    state: CalibState,
}

impl HeadCalibration {
    /// Start a run at `now_ms`; returns the run and the write to send. `before` is the follower
    /// status read just before the start: any alarm bit, or no follower status at all, blocks the
    /// start (superblock, no override).
    pub fn start(now_ms: u64, before: CalibInput) -> Result<(HeadCalibration, SafeWrite), StartBlocked> {
        match before {
            CalibInput::Follower { alarm, .. } if alarm & 0xFFFF != 0 => return Err(StartBlocked::FtcError { alarm: alarm & 0xFFFF }),
            CalibInput::Follower { .. } => {}
            CalibInput::RunStatus { .. } => return Err(StartBlocked::NoFollowerStatus),
        }
        let w = command::head_calibrate().map_err(StartBlocked::Forbidden)?;
        Ok((HeadCalibration { started_ms: now_ms, seen_busy: false, state: CalibState::Running }, w))
    }

    /// Current state.
    pub fn state(&self) -> &CalibState {
        &self.state
    }

    /// Milliseconds since the start.
    pub fn elapsed_ms(&self, now_ms: u64) -> u64 {
        now_ms.saturating_sub(self.started_ms)
    }

    /// Feed one status poll. Returns the writes to send now.
    pub fn on_status(&mut self, now_ms: u64, input: CalibInput, params: &MachineParams) -> Result<Vec<SafeWrite>, Forbidden> {
        if self.state != CalibState::Running {
            return Ok(Vec::new());
        }
        if let CalibInput::Follower { alarm, .. } = input {
            let alarm = alarm & 0xFFFF;
            if alarm != 0 {
                self.state = CalibState::Failed { alarm };
                return Ok(vec![command::stop_all(0, params)?]);
            }
        }
        if self.elapsed_ms(now_ms) >= TIMEOUT_MS {
            self.state = CalibState::TimedOut;
            return Ok(vec![command::stop_all(0, params)?]);
        }
        let busy = match input {
            CalibInput::Follower { run_command, .. } => run_command == RUN_COMMAND_CALIBRATE,
            CalibInput::RunStatus { busy } => busy,
        };
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

    fn zf(run_command: u32) -> CalibInput {
        CalibInput::Follower { run_command, alarm: 0 }
    }

    fn rs(busy: bool) -> CalibInput {
        CalibInput::RunStatus { busy }
    }

    #[test]
    fn start_sends_107() {
        let (c, w) = HeadCalibration::start(0, zf(0)).unwrap();
        assert_eq!(w.words(), &[107]);
        assert_eq!(c.state(), &CalibState::Running);
    }

    #[test]
    fn superblock_any_ftc_error_refuses_the_start() {
        use crate::registers::zf_alarm;
        for alarm in [zf_alarm::FTC_ALARM, zf_alarm::SIGNAL_SMALL, zf_alarm::TOUCH_PLATE, zf_alarm::FPGA_NOT_LOADED, 1 << 14] {
            let e = HeadCalibration::start(0, CalibInput::Follower { run_command: 0, alarm }).unwrap_err();
            assert_eq!(e, StartBlocked::FtcError { alarm });
            assert!(e.text().contains("FTC reports an error"), "{}", e.text());
        }
        // high bits are not alarm bits
        assert!(HeadCalibration::start(0, CalibInput::Follower { run_command: 0, alarm: 0x1_0000 }).is_ok());
    }

    #[test]
    fn superblock_no_follower_status_refuses_the_start() {
        for busy in [false, true] {
            assert_eq!(HeadCalibration::start(0, rs(busy)).unwrap_err(), StartBlocked::NoFollowerStatus);
        }
    }

    #[test]
    fn finishes_on_the_follower_run_command_edge() {
        let (mut c, _) = HeadCalibration::start(0, zf(0)).unwrap();
        assert!(c.on_status(100, zf(105), &p()).unwrap().is_empty(), "before the follower picked it up");
        assert!(c.on_status(500, zf(107), &p()).unwrap().is_empty());
        assert!(c.on_status(11_000, zf(107), &p()).unwrap().is_empty());
        assert_eq!(c.state(), &CalibState::Running);
        let w = c.on_status(22_000, zf(0), &p()).unwrap();
        assert_eq!(c.state(), &CalibState::Finished);
        assert_eq!(w.iter().map(|w| w.words().to_vec()).collect::<Vec<_>>(), vec![vec![3], vec![1]]);
        assert!(c.on_status(22_100, zf(0), &p()).unwrap().is_empty(), "finished once");
    }

    #[test]
    fn a_zero_run_command_before_107_is_not_done() {
        let (mut c, _) = HeadCalibration::start(0, zf(0)).unwrap();
        assert!(c.on_status(100, zf(0), &p()).unwrap().is_empty());
        assert_eq!(c.state(), &CalibState::Running);
    }

    #[test]
    fn follower_alarm_fails_the_run_with_stop_all() {
        use crate::registers::zf_alarm;
        let (mut c, _) = HeadCalibration::start(0, zf(0)).unwrap();
        c.on_status(500, zf(107), &p()).unwrap();
        let alarm = zf_alarm::SIGNAL_SMALL | zf_alarm::FTC_ALARM;
        let w = c.on_status(5_000, CalibInput::Follower { run_command: 107, alarm }, &p()).unwrap();
        assert_eq!(c.state(), &CalibState::Failed { alarm });
        assert_eq!(w[0].words()[..2], [1, 0x1F]);
        assert_eq!(zf_alarm::names(alarm), vec!["signal too small", "follower alarm"]);
        assert!(c.on_status(5_100, zf(0), &p()).unwrap().is_empty());
        assert!(c.cancel(&p()).unwrap().is_none());
    }

    #[test]
    fn run_status_fallback_without_a_follower() {
        let (mut c, _) = HeadCalibration::start(0, zf(0)).unwrap();
        assert!(c.on_status(100, rs(false), &p()).unwrap().is_empty(), "idle before the card picked it up");
        assert!(c.on_status(500, rs(true), &p()).unwrap().is_empty());
        let w = c.on_status(22_000, rs(false), &p()).unwrap();
        assert_eq!(c.state(), &CalibState::Finished);
        assert_eq!(w.len(), 2);
    }

    #[test]
    fn timeout_stops() {
        let (mut c, _) = HeadCalibration::start(1000, zf(0)).unwrap();
        c.on_status(2000, zf(107), &p()).unwrap();
        let w = c.on_status(1000 + TIMEOUT_MS, zf(107), &p()).unwrap();
        assert_eq!(c.state(), &CalibState::TimedOut);
        assert_eq!(w[0].words()[..2], [1, 0x1F]);
    }

    #[test]
    fn cancel_stops_once() {
        let (mut c, _) = HeadCalibration::start(0, zf(0)).unwrap();
        let w = c.cancel(&p()).unwrap().unwrap();
        assert_eq!(w.words()[..2], [1, 0x1F]);
        assert_eq!(c.state(), &CalibState::Cancelled);
        assert!(c.cancel(&p()).unwrap().is_none());
        assert!(c.on_status(10, zf(0), &p()).unwrap().is_empty());
    }
}
