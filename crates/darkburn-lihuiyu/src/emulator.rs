//! An EGV reader and an in-memory board, used by every test.
//!
//! [`EgvReader`] reads EGV text back into moves with the laser on or off, following the same
//! state machine the original EGV parser of MeerK40t uses (MIT; see `ATTRIBUTION.md`):
//!
//! * `I` starts a command and leaves the finished state;
//! * `B`/`T`/`R`/`L` set the direction (right, left, down, up) and the axis distances apply to;
//!   `M` makes distances apply to both axes (45 degree moves);
//! * `U`/`D` turn the laser off and on;
//! * `S1E` enters "compact" (program) mode; in it every command first runs the distance
//!   collected so far. Outside it the distance runs on `N`, `S` and `P`;
//! * in a horizontal raster (compact mode with a `G` step), reversing between `B` and `T`
//!   moves one raster step down or up (in the declared vertical direction);
//! * `F` then `NSE` finishes (everything up to the next `I` is ignored); `@NSE` resets;
//! * `PP` in one command homes to 0,0.
//!
//! [`Board`] wraps a reader with packet checks and status replies, like the real board.

use crate::packet::{PACKET_LEN, PAYLOAD_LEN, Status, crc8};
use crate::speed::{self, Board as BoardModel, RasterStep};

/// One move read back from EGV, in mils from where the reader started.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Move {
    pub from: (i64, i64),
    pub to: (i64, i64),
    /// Laser on during the move.
    pub laser: bool,
    /// The move ran in program (compact) mode. Moves outside it are travel moves.
    pub program: bool,
}

impl Move {
    /// Laser on in program mode: the move burns.
    pub fn burns(&self) -> bool {
        self.laser && self.program
    }
}

/// A speed code met in the text.
#[derive(Clone, Debug, PartialEq)]
pub struct SpeedSeen {
    pub code: String,
    /// Speed in mm/s for the reader's board, if the code could be read.
    pub speed: Option<f64>,
    pub raster_step: Option<RasterStep>,
    /// Index into [`EgvReader::moves`] of the first move after the code.
    pub at_move: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Number {
    Distance,
    SpeedHigh,
    SpeedLow,
    Accel,
    Step,
    DiagHigh,
    DiagLow,
    RasterStep,
    Mode,
    Power,
}

impl Number {
    fn width(self) -> usize {
        match self {
            Number::Accel | Number::Mode => 1,
            _ => 3,
        }
    }
}

/// Reads EGV text into moves.
#[derive(Clone, Debug)]
pub struct EgvReader {
    board: BoardModel,
    fix_speeds: bool,
    pub x: i64,
    pub y: i64,
    pub laser: bool,
    left: bool,
    top: bool,
    x_on: bool,
    y_on: bool,
    small_jump: bool,
    returning_compact: bool,
    returning_finished: bool,
    mode: Option<u8>,
    raster_step: u32,
    paused: bool,
    compact: bool,
    finished: bool,
    horizontal_major: bool,
    number: Number,
    digits: String,
    distance_x: i64,
    distance_y: i64,
    speed_text: Option<String>,
    last_char: char,
    /// Every move, in order.
    pub moves: Vec<Move>,
    /// Every speed code, in order.
    pub speeds: Vec<SpeedSeen>,
    /// `W` power values (M3 boards with newer firmware only).
    pub powers: Vec<u32>,
    /// Times the reader homed (`PP`).
    pub homes: usize,
}

impl Default for EgvReader {
    fn default() -> Self {
        EgvReader::new(BoardModel::M2)
    }
}

impl EgvReader {
    pub fn new(board: BoardModel) -> Self {
        EgvReader {
            board,
            fix_speeds: false,
            x: 0,
            y: 0,
            laser: false,
            left: false,
            top: false,
            x_on: false,
            y_on: false,
            small_jump: false,
            returning_compact: true,
            returning_finished: false,
            mode: None,
            raster_step: 0,
            paused: false,
            compact: false,
            finished: false,
            horizontal_major: false,
            number: Number::Distance,
            digits: String::new(),
            distance_x: 0,
            distance_y: 0,
            speed_text: None,
            last_char: '\0',
            moves: Vec::new(),
            speeds: Vec::new(),
            powers: Vec::new(),
            homes: 0,
        }
    }

    /// Reads speed codes with the measured-speed equation.
    pub fn with_fix_speeds(mut self, on: bool) -> Self {
        self.fix_speeds = on;
        self
    }

    /// True while the reader is in program (compact) mode.
    pub fn in_program(&self) -> bool {
        self.compact
    }

    /// True after a finish (`F...NSE`) until the next `I`.
    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// Reads a whole text.
    pub fn read(board: BoardModel, text: &str) -> EgvReader {
        let mut r = EgvReader::new(board);
        r.write(text.as_bytes());
        r.end();
        r
    }

    /// Feeds bytes.
    pub fn write(&mut self, data: &[u8]) {
        for &b in data {
            self.process(b as char);
        }
    }

    /// Ends the input: a speed code still being read is recorded.
    pub fn end(&mut self) {
        self.flush_speed(None);
    }

    /// Where the head is.
    pub fn position(&self) -> (i64, i64) {
        (self.x, self.y)
    }

    /// Total burning length in mils (diagonals counted as their true length).
    pub fn burn_length(&self) -> f64 {
        self.moves.iter().filter(|m| m.burns()).map(|m| (((m.to.0 - m.from.0) as f64).powi(2) + ((m.to.1 - m.from.1) as f64).powi(2)).sqrt()).sum()
    }

    fn append_distance(&mut self, amount: i64) {
        if self.x_on {
            self.distance_x += amount;
        }
        if self.y_on {
            self.distance_y += amount;
        }
    }

    fn execute_distance(&mut self) {
        if self.distance_x == 0 && self.distance_y == 0 {
            return;
        }
        let dx = if self.left { -self.distance_x } else { self.distance_x };
        let dy = if self.top { -self.distance_y } else { self.distance_y };
        self.distance_x = 0;
        self.distance_y = 0;
        let from = (self.x, self.y);
        self.x += dx;
        self.y += dy;
        self.moves.push(Move { from, to: (self.x, self.y), laser: self.laser, program: self.compact });
    }

    fn flush_speed(&mut self, next: Option<char>) -> bool {
        let Some(mut code) = self.speed_text.take() else {
            return false;
        };
        let consumed = next == Some('C');
        if consumed {
            code.push('C');
        }
        let parsed = speed::parse_speed_code(&code).ok();
        let speed = speed::speed_from_code(&code, self.board, self.fix_speeds).ok();
        self.speeds.push(SpeedSeen { code, speed, raster_step: parsed.and_then(|p| p.raster_step), at_move: self.moves.len() });
        consumed
    }

    fn number_digit(&mut self, c: char) {
        self.digits.push(c);
        if self.digits.len() < self.number.width() {
            return;
        }
        let value: i64 = self.digits.parse().unwrap_or(0);
        self.digits.clear();
        self.number = match self.number {
            Number::Distance => {
                self.append_distance(value);
                Number::Distance
            }
            Number::SpeedHigh => Number::SpeedLow,
            Number::SpeedLow => Number::Accel,
            Number::Accel => Number::Step,
            Number::Step => Number::DiagHigh,
            Number::DiagHigh => Number::DiagLow,
            Number::DiagLow => Number::Distance,
            Number::RasterStep => {
                self.raster_step = value as u32;
                Number::Distance
            }
            Number::Mode => {
                self.mode = Some(value as u8);
                Number::Step
            }
            Number::Power => {
                self.powers.push(if value == 0 { 1000 } else { value as u32 });
                Number::Distance
            }
        };
    }

    fn process(&mut self, c: char) {
        // Speed codes are recorded whole, as text, next to the state machine.
        if self.speed_text.is_some() {
            if c.is_ascii_digit() || c == 'G' {
                if let Some(t) = self.speed_text.as_mut() {
                    t.push(c);
                }
            } else {
                // A closing C becomes part of the code; the state machine still sees it.
                self.flush_speed(Some(c));
            }
        }
        self.state_machine(c);
        self.last_char = c;
    }

    fn state_machine(&mut self, c: char) {
        if c == 'I' {
            self.finished = false;
            self.compact = false;
            self.paused = false;
            self.distance_x = 0;
            self.distance_y = 0;
        }
        if self.finished {
            return;
        }
        if c.is_ascii_digit() {
            self.number_digit(c);
            return;
        }
        self.number = Number::Distance;
        self.digits.clear();
        if self.compact {
            self.execute_distance();
        }
        match c {
            '|' => {
                self.append_distance(25);
                self.small_jump = true;
            }
            'a'..='y' => {
                self.append_distance(c as i64 - 'a' as i64 + 1);
                self.small_jump = false;
            }
            'z' => {
                self.append_distance(if self.small_jump { 26 } else { 255 });
                self.small_jump = false;
            }
            'B' => {
                if self.left && self.horizontal_major && self.raster_step != 0 && self.compact {
                    self.distance_y += self.raster_step as i64;
                }
                self.left = false;
                self.x_on = true;
                self.y_on = false;
            }
            'T' => {
                if !self.left && self.horizontal_major && self.raster_step != 0 && self.compact {
                    self.distance_y += self.raster_step as i64;
                }
                self.left = true;
                self.x_on = true;
                self.y_on = false;
            }
            'R' => {
                if self.top && !self.horizontal_major && self.raster_step != 0 && self.compact {
                    self.distance_x += self.raster_step as i64;
                }
                self.top = false;
                self.x_on = false;
                self.y_on = true;
            }
            'L' => {
                if !self.top && !self.horizontal_major && self.raster_step != 0 && self.compact {
                    self.distance_x += self.raster_step as i64;
                }
                self.top = true;
                self.x_on = false;
                self.y_on = true;
            }
            'U' => self.laser = false,
            'D' => self.laser = true,
            'F' => {
                self.returning_compact = false;
                self.returning_finished = true;
            }
            '@' => {
                self.returning_finished = false;
                self.returning_compact = false;
            }
            'V' => {
                self.raster_step = 0;
                self.number = Number::SpeedHigh;
                self.speed_text = Some(if self.last_char == 'C' { "CV".into() } else { "V".into() });
            }
            'G' => self.number = Number::RasterStep,
            'S' => {
                self.laser = false;
                self.execute_distance();
                self.mode = None;
                self.number = Number::Mode;
            }
            'E' => {
                match self.mode {
                    None => {
                        if self.returning_compact {
                            self.compact = true;
                        }
                        if self.returning_finished {
                            self.finished = true;
                        }
                        if self.horizontal_major {
                            self.left = !self.left;
                            self.x_on = true;
                            self.y_on = false;
                        } else {
                            self.top = !self.top;
                            self.x_on = false;
                            self.y_on = true;
                        }
                    }
                    Some(1) => {
                        self.compact = true;
                        self.horizontal_major = self.x_on;
                    }
                    Some(2) => self.compact = true,
                    Some(_) => {}
                }
                self.returning_finished = false;
                self.returning_compact = true;
                self.laser = false;
            }
            'P' => {
                self.laser = false;
                if self.paused {
                    if (self.x, self.y) != (0, 0) {
                        self.moves.push(Move { from: (self.x, self.y), to: (0, 0), laser: false, program: false });
                    }
                    self.x = 0;
                    self.y = 0;
                    self.distance_x = 0;
                    self.distance_y = 0;
                    self.finished = true;
                    self.paused = false;
                    self.homes += 1;
                } else {
                    self.execute_distance();
                    self.paused = true;
                }
            }
            'N' => {
                self.execute_distance();
                self.laser = false;
                self.compact = false;
            }
            'M' => {
                self.x_on = true;
                self.y_on = true;
            }
            'W' => self.number = Number::Power,
            _ => {}
        }
    }
}

/// An in-memory Lihuiyu board: checks each packet's framing and CRC like the real board,
/// answers with a status byte and runs accepted payloads through an [`EgvReader`].
#[derive(Clone, Debug)]
pub struct Board {
    pub reader: EgvReader,
    /// Packets accepted.
    pub accepted: usize,
    /// Packets rejected (bad CRC or framing).
    pub rejected: usize,
    /// Make the next N packets fail their CRC check, as if damaged on the wire.
    pub corrupt_next: usize,
    /// Accepted payloads, in order.
    pub payloads: Vec<[u8; PAYLOAD_LEN]>,
    last: u8,
}

impl Board {
    pub fn new(model: BoardModel) -> Self {
        Board { reader: EgvReader::new(model), accepted: 0, rejected: 0, corrupt_next: 0, payloads: Vec::new(), last: Status::OK }
    }

    /// Takes one 32-byte packet (`0x00`, payload, CRC) or its 33-byte wire form (`0xA6` first).
    /// Returns the status the board would report after it.
    pub fn receive(&mut self, bytes: &[u8]) -> Status {
        let bytes = match bytes.len() {
            n if n == PACKET_LEN + 1 && bytes[0] == crate::packet::CH341_EPP_WRITE => &bytes[1..],
            _ => bytes,
        };
        let ok = bytes.len() == PACKET_LEN && bytes[0] == 0 && crc8(&bytes[1..31]) == bytes[31];
        if !ok || self.corrupt_next > 0 {
            self.corrupt_next = self.corrupt_next.saturating_sub(1);
            self.rejected += 1;
            self.last = Status::ERROR;
            return Status::Error;
        }
        let mut payload = [0u8; PAYLOAD_LEN];
        payload.copy_from_slice(&bytes[1..31]);
        self.reader.write(&payload);
        self.payloads.push(payload);
        self.accepted += 1;
        self.last = Status::OK;
        Status::Ok
    }

    /// The 6-byte status reply; byte 1 is the status. Once the reader has finished a job the
    /// board reports FINISH until the next packet.
    pub fn status(&self) -> [u8; 6] {
        let s = if self.last == Status::OK && self.reader.is_finished() { Status::FINISH } else { self.last };
        [255, s, 0, 0, 0, 0]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::frames;

    const RECT: &str = "IBzzzvRzzzvS1P\nICV2490731016000027CNLBS1EDz139Rz139Tz139Lz139FNSE-\n";

    #[test]
    fn reads_the_reference_rectangle() {
        let r = EgvReader::read(BoardModel::M2, RECT);
        let burns: Vec<_> = r.moves.iter().filter(|m| m.burns()).map(|m| (m.from, m.to)).collect();
        assert_eq!(burns, vec![((787, 787), (1181, 787)), ((1181, 787), (1181, 1181)), ((1181, 1181), (787, 1181)), ((787, 1181), (787, 787))]);
        assert_eq!(r.moves[0], Move { from: (0, 0), to: (787, 787), laser: false, program: false });
        assert_eq!(r.speeds.len(), 1);
        assert_eq!(r.speeds[0].code, "CV2490731016000027C");
        assert!((r.speeds[0].speed.unwrap() - 15.0).abs() < 0.01);
        assert!(r.is_finished());
    }

    #[test]
    fn reads_raster_steps_on_reversal() {
        // MeerK40t's raster test output (start).
        let text = "IB055R091S1P\nIV1552121G002NLBS1EDnUxTtD|aUvBtD|iUtTtD|aDlFNSE-\n";
        let r = EgvReader::read(BoardModel::M2, text);
        assert_eq!(r.speeds[0].raster_step, Some(RasterStep::Bidirectional(2)));
        let burns: Vec<_> = r.moves.iter().filter(|m| m.burns()).map(|m| (m.from, m.to)).collect();
        // Row 1 at y=91 left to right, then up by 2 each reversal (declared upward).
        assert_eq!(burns[0], ((55, 91), (69, 91)));
        assert!(r.moves.iter().any(|m| m.from.1 == 91 && m.to.1 == 89 && !m.laser));
        assert!(burns.iter().all(|(a, b)| a.1 == b.1));
    }

    #[test]
    fn diagonal_moves_use_both_axes() {
        let r = EgvReader::read(BoardModel::M2, "ICV2490731016000027CNRBS1EDMjFNSE-\n");
        let b: Vec<_> = r.moves.iter().filter(|m| m.burns()).collect();
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].to, (10, 10));
    }

    #[test]
    fn double_p_homes() {
        let r = EgvReader::read(BoardModel::M2, "IBzS1P\nIPP\n");
        assert_eq!(r.homes, 1);
        assert_eq!(r.position(), (0, 0));
    }

    #[test]
    fn finished_state_ignores_until_i() {
        let r = EgvReader::read(BoardModel::M2, "ICV2490731016000027CNLBS1EDzFNSE-BzzzzS1P\n");
        assert_eq!(r.position(), (255, 0));
    }

    #[test]
    fn power_codes_are_recorded() {
        let r = EgvReader::read(BoardModel::M3, "ICV2490731016000027W500CNLBS1EDzFNSE-\n");
        assert_eq!(r.powers, vec![500]);
    }

    #[test]
    fn board_accepts_good_packets_and_rejects_bad_crc() {
        let mut board = Board::new(BoardModel::M2);
        let framed = frames(RECT.as_bytes());
        for f in &framed.frames {
            let p = f.packet.unwrap();
            assert_eq!(board.receive(&p.to_bytes()), Status::Ok);
        }
        assert_eq!(board.accepted, 3);
        assert_eq!(board.reader.moves.iter().filter(|m| m.burns()).count(), 4);
        assert_eq!(board.status()[1], Status::FINISH);

        let mut bad = framed.frames[0].packet.unwrap().to_bytes();
        bad[31] ^= 1;
        assert_eq!(board.receive(&bad), Status::Error);
        assert_eq!(board.status()[1], Status::ERROR);
        assert_eq!(board.receive(&[0u8; 5]), Status::Error);
        assert_eq!(board.rejected, 2);
        assert_eq!(board.accepted, 3);
    }

    #[test]
    fn board_takes_the_wire_form() {
        let mut board = Board::new(BoardModel::M2);
        let p = frames(b"IBzS1P\n").frames[0].packet.unwrap();
        assert_eq!(board.receive(&p.to_wire()), Status::Ok);
        assert_eq!(board.reader.position(), (255, 0));
    }

    #[test]
    fn padding_does_not_move_the_head() {
        let direct = EgvReader::read(BoardModel::M2, RECT);
        let mut board = Board::new(BoardModel::M2);
        for f in frames(RECT.as_bytes()).frames {
            board.receive(&f.packet.unwrap().to_bytes());
        }
        let a: Vec<_> = direct.moves.iter().map(|m| (m.from, m.to, m.burns())).collect();
        let b: Vec<_> = board.reader.moves.iter().map(|m| (m.from, m.to, m.burns())).collect();
        assert_eq!(a, b);
    }
}
