//! Sending EGV to a board: the [`Transport`] trait, an in-memory link and the send loop.
//!
//! There is no real USB transport in this crate. The stock boards sit behind a CH341 chip
//! (USB `1a86:5512`) in EPP mode: 34-byte writes on bulk endpoint `0x02` (the 32-byte packet
//! with an `0xA6` command byte in front of each 31 bytes: `A6 00 <30 bytes> A6 <crc>`), status
//! read with a vendor control request. Adding one needs the owner's
//! approval and a test on a real machine first; until then every sender here talks to the
//! in-memory board in [`crate::emulator`].
//!
//! The send loop follows MeerK40t's controller (MIT; see `ATTRIBUTION.md`): send a packet,
//! read the status until the board answers OK, resend on a CRC error, and after a packet
//! that ends a job (`-`), wait until the board reports it has finished.

use std::fmt;
use std::sync::{Arc, Mutex};

use crate::emulator::Board;
use crate::packet::{After, Status, WIRE_LEN, frames};
use crate::speed::Board as BoardModel;

/// Why sending failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransportError {
    /// The link failed.
    Io(String),
    /// The board kept rejecting a packet.
    TooManyErrors { packet: usize },
    /// The board reported a power fault (status 239).
    Power,
    /// The board did not answer in time.
    Timeout(String),
}

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TransportError::Io(e) => write!(f, "link error: {e}"),
            TransportError::TooManyErrors { packet } => write!(f, "the board kept rejecting packet {packet}"),
            TransportError::Power => f.write_str("the board reports a power problem"),
            TransportError::Timeout(what) => write!(f, "the board did not answer in time ({what})"),
        }
    }
}

impl std::error::Error for TransportError {}

/// A link to a board.
pub trait Transport: Send {
    /// A short name for logs.
    fn name(&self) -> String;
    /// Writes one packet in its wire form (`0xA6`, `0x00`, 30 bytes, `0xA6`, CRC).
    fn write_packet(&mut self, wire: &[u8; WIRE_LEN]) -> Result<(), TransportError>;
    /// Reads the 6-byte status; byte 1 is the status.
    fn read_status(&mut self) -> Result<[u8; 6], TransportError>;
}

/// A link to an in-memory [`Board`]; the board is shared so tests can look at it.
#[derive(Clone, Debug)]
pub struct MemoryTransport {
    pub board: Arc<Mutex<Board>>,
}

impl MemoryTransport {
    pub fn new(model: BoardModel) -> Self {
        MemoryTransport { board: Arc::new(Mutex::new(Board::new(model))) }
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Board>, TransportError> {
        self.board.lock().map_err(|_| TransportError::Io("in-memory board lock poisoned".into()))
    }
}

impl Transport for MemoryTransport {
    fn name(&self) -> String {
        "in-memory board".into()
    }

    fn write_packet(&mut self, wire: &[u8; WIRE_LEN]) -> Result<(), TransportError> {
        self.lock()?.receive(wire);
        Ok(())
    }

    fn read_status(&mut self) -> Result<[u8; 6], TransportError> {
        Ok(self.lock()?.status())
    }
}

/// Limits of the send loop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SendOptions {
    /// Resends of one packet before giving up.
    pub max_resends: u32,
    /// Status reads while waiting for one answer before giving up.
    pub max_polls: u32,
}

impl Default for SendOptions {
    fn default() -> Self {
        SendOptions { max_resends: 5, max_polls: 100_000 }
    }
}

/// What a send did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SendReport {
    /// Packets the board accepted.
    pub packets: usize,
    /// Packets sent again after a CRC error.
    pub resends: usize,
    /// Waits for the board to finish.
    pub finish_waits: usize,
    /// Bytes at the end that were not sent (no line end).
    pub unsent: usize,
    /// Sending stopped at an abort or quit mark.
    pub stopped: bool,
}

fn status_byte(t: &mut dyn Transport) -> Result<Status, TransportError> {
    Ok(Status::from_byte(t.read_status()?[1]))
}

/// Sends EGV bytes over `t`.
pub fn send(t: &mut dyn Transport, data: &[u8], opts: &SendOptions) -> Result<SendReport, TransportError> {
    let framed = frames(data);
    let mut report = SendReport { unsent: framed.unsent, ..Default::default() };
    for frame in &framed.frames {
        if let Some(packet) = frame.packet {
            let wire = packet.to_wire();
            let mut resends = 0;
            'packet: loop {
                t.write_packet(&wire)?;
                for _ in 0..opts.max_polls {
                    match status_byte(t)? {
                        Status::Ok | Status::Finish | Status::SerialOkOrM3Finish => break 'packet,
                        Status::Error => {
                            resends += 1;
                            if resends > opts.max_resends || packet.bad_crc {
                                return Err(TransportError::TooManyErrors { packet: report.packets });
                            }
                            report.resends += 1;
                            continue 'packet;
                        }
                        Status::Power => return Err(TransportError::Power),
                        Status::Busy | Status::NoReply | Status::Unknown(_) => {}
                    }
                }
                return Err(TransportError::Timeout(format!("packet {}", report.packets)));
            }
            report.packets += 1;
        }
        match frame.after {
            Some(After::WaitFinish) => {
                report.finish_waits += 1;
                let mut done = false;
                for _ in 0..opts.max_polls {
                    match status_byte(t)? {
                        Status::Finish | Status::SerialOkOrM3Finish => {
                            done = true;
                            break;
                        }
                        Status::Power => return Err(TransportError::Power),
                        _ => {}
                    }
                }
                if !done {
                    return Err(TransportError::Timeout("waiting for the job to finish".into()));
                }
            }
            Some(After::Abort) | Some(After::Quit) => {
                report.stopped = true;
                return Ok(report);
            }
            // Pausing, resuming and serial number checks are the application's business.
            Some(After::Pause) | Some(After::Resume) | Some(After::ConfirmSerial) | None => {}
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emulator::EgvReader;

    const RECT: &str = "IBzzzvRzzzvS1P\nICV2490731016000027CNLBS1EDz139Rz139Tz139Lz139FNSE-\n";

    #[test]
    fn the_board_gets_what_was_sent() {
        let mut t = MemoryTransport::new(BoardModel::M2);
        let report = send(&mut t, RECT.as_bytes(), &SendOptions::default()).unwrap();
        assert_eq!(report.packets, 3);
        assert_eq!(report.finish_waits, 1);
        assert_eq!(report.resends, 0);
        let board = t.board.lock().unwrap();
        let direct = EgvReader::read(BoardModel::M2, RECT);
        assert_eq!(board.reader.moves, direct.moves);
        assert!(board.reader.is_finished());
    }

    #[test]
    fn damaged_packets_are_sent_again() {
        let mut t = MemoryTransport::new(BoardModel::M2);
        t.board.lock().unwrap().corrupt_next = 2;
        let report = send(&mut t, RECT.as_bytes(), &SendOptions::default()).unwrap();
        assert_eq!(report.resends, 2);
        let board = t.board.lock().unwrap();
        assert_eq!(board.rejected, 2);
        assert_eq!(board.reader.moves, EgvReader::read(BoardModel::M2, RECT).moves);
    }

    #[test]
    fn a_board_that_keeps_failing_stops_the_send() {
        let mut t = MemoryTransport::new(BoardModel::M2);
        t.board.lock().unwrap().corrupt_next = 100;
        let err = send(&mut t, RECT.as_bytes(), &SendOptions { max_resends: 3, ..Default::default() }).unwrap_err();
        assert_eq!(err, TransportError::TooManyErrors { packet: 0 });
        assert_eq!(t.board.lock().unwrap().rejected, 4);
    }

    struct Stuck(u8);

    impl Transport for Stuck {
        fn name(&self) -> String {
            "stuck".into()
        }
        fn write_packet(&mut self, _: &[u8; WIRE_LEN]) -> Result<(), TransportError> {
            Ok(())
        }
        fn read_status(&mut self) -> Result<[u8; 6], TransportError> {
            Ok([255, self.0, 0, 0, 0, 0])
        }
    }

    #[test]
    fn a_busy_board_times_out() {
        let opts = SendOptions { max_polls: 10, ..Default::default() };
        assert!(matches!(send(&mut Stuck(Status::BUSY), b"IPP\n", &opts), Err(TransportError::Timeout(_))));
        assert_eq!(send(&mut Stuck(Status::POWER), b"IPP\n", &opts), Err(TransportError::Power));
        // OK but never finished: the wait after the job end times out.
        assert!(matches!(send(&mut Stuck(Status::OK), RECT.as_bytes(), &opts), Err(TransportError::Timeout(_))));
    }

    #[test]
    fn abort_marks_stop_the_send_and_text_without_a_line_end_waits() {
        let mut t = MemoryTransport::new(BoardModel::M2);
        let report = send(&mut t, b"IPP\nI*\nIBjS1P\n", &SendOptions::default()).unwrap();
        assert!(report.stopped);
        assert_eq!(report.packets, 2);
        let report = send(&mut t, b"IPP\nIBj", &SendOptions::default()).unwrap();
        assert_eq!(report.unsent, 3);
        assert_eq!(t.name(), "in-memory board");
    }
}
