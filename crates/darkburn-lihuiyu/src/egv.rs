//! The EGV writer: turns moves in mils into EGV text.
//!
//! EGV ("LHYMICRO-GL") is the text the Lihuiyu boards run. Every move is relative. Native units
//! are mils (1/1000 inch); `+x` is right and `+y` is towards the front of the machine (the
//! K40 homes in the back left corner).
//!
//! The writer is a state machine with four modes, as in the reference driver (MeerK40t's
//! `lihuiyu/driver.py`, MIT; see `ATTRIBUTION.md`):
//!
//! * **rapid**: travel. Each move is one command, `I` + directions and distances + `S1P`.
//! * **finish**: between commands; reached from rapid with `I`.
//! * **program**: vector cutting. Entered with a speed code, `N`, the two declared directions
//!   and `S1E`; then directions (`B` right, `T` left, `R` down, `L` up, `M` 45 degrees), laser
//!   switches (`D` on, `U` off) and distances. Left with `FNSE-` (back to rapid; `-` makes the
//!   sender wait until the board has finished).
//! * **raster**: like program, with a raster step in the speed code (`G`). Reversing the
//!   horizontal direction moves the head one step down or up by itself.
//!
//! The writer only uses the conservative transitions: program and raster are always left
//! through rapid (never the "on the fly" `@NSE` switch), and each program block starts from a
//! standstill.

use std::fmt;

use crate::distance::push_distance;
use crate::speed::{Board, RasterStep, SpeedOptions, speed_code};

/// Right.
pub const CODE_RIGHT: char = 'B';
/// Left.
pub const CODE_LEFT: char = 'T';
/// Up (towards the back, `-y`).
pub const CODE_TOP: char = 'L';
/// Down (towards the front, `+y`).
pub const CODE_BOTTOM: char = 'R';
/// 45 degree move on both axes.
pub const CODE_ANGLE: char = 'M';
/// Laser on.
pub const CODE_LASER_ON: char = 'D';
/// Laser off.
pub const CODE_LASER_OFF: char = 'U';

/// Largest step difference a raster reversal can correct (larger ones need a separate move).
pub const MAX_RASTER_CORRECTION: i64 = 15;

/// Writer settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WriterOptions {
    pub board: Board,
    /// Keep the rail locked (motors on) after a travel move. Off sends `IS2P` after each one.
    pub autolock: bool,
    /// Use the measured M2 slope so real speeds match the requested ones.
    pub fix_speeds: bool,
}

impl Default for WriterOptions {
    fn default() -> Self {
        WriterOptions { board: Board::M2, autolock: true, fix_speeds: false }
    }
}

/// The writer's mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Rapid,
    Finish,
    Program,
    Raster,
}

/// A writer call that does not fit the mode it was made in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WriterError(pub String);

impl fmt::Display for WriterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for WriterError {}

/// Writes EGV text.
#[derive(Clone, Debug)]
pub struct EgvWriter {
    opts: WriterOptions,
    out: String,
    mode: Mode,
    x: i64,
    y: i64,
    leftward: bool,
    topward: bool,
    x_engaged: bool,
    y_engaged: bool,
    horizontal_major: bool,
    laser: bool,
    raster_step: u32,
}

impl EgvWriter {
    /// A writer with the head at 0,0.
    pub fn new(opts: WriterOptions) -> Self {
        Self::starting_at(opts, (0, 0))
    }

    /// A writer with the head at `pos` (mils). Only used to report positions; all moves are
    /// relative.
    pub fn starting_at(opts: WriterOptions, pos: (i64, i64)) -> Self {
        EgvWriter {
            opts,
            out: String::new(),
            mode: Mode::Rapid,
            x: pos.0,
            y: pos.1,
            leftward: false,
            topward: false,
            x_engaged: false,
            y_engaged: false,
            horizontal_major: false,
            laser: false,
            raster_step: 0,
        }
    }

    pub fn options(&self) -> &WriterOptions {
        &self.opts
    }

    pub fn position(&self) -> (i64, i64) {
        (self.x, self.y)
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// The text written so far.
    pub fn text(&self) -> &str {
        &self.out
    }

    /// Returns to rapid mode and hands back the text.
    pub fn finish(mut self) -> String {
        self.rapid_mode();
        self.out
    }

    fn is_left(&self) -> bool {
        self.x_engaged && !self.y_engaged && self.leftward
    }

    fn is_right(&self) -> bool {
        self.x_engaged && !self.y_engaged && !self.leftward
    }

    fn is_top(&self) -> bool {
        !self.x_engaged && self.y_engaged && self.topward
    }

    fn is_bottom(&self) -> bool {
        !self.x_engaged && self.y_engaged && !self.topward
    }

    /// Goes to rapid mode (`FNSE-` from program or raster, `S1P` from finish).
    pub fn rapid_mode(&mut self) {
        match self.mode {
            Mode::Rapid => return,
            Mode::Finish => {
                self.out.push_str("S1P\n");
                if !self.opts.autolock {
                    self.out.push_str("IS2P\n");
                }
            }
            Mode::Program | Mode::Raster => {
                self.out.push_str("FNSE-\n");
                self.laser = false;
            }
        }
        self.mode = Mode::Rapid;
    }

    /// Goes to finish mode (`I` from rapid). From program or raster it goes through rapid.
    pub fn finished_mode(&mut self) {
        match self.mode {
            Mode::Finish => return,
            Mode::Program | Mode::Raster => {
                self.rapid_mode();
                self.out.push('I');
            }
            Mode::Rapid => self.out.push('I'),
        }
        self.mode = Mode::Finish;
    }

    /// Travels to `(x, y)` mils with the laser off.
    pub fn move_to(&mut self, x: i64, y: i64) {
        self.rapid_mode();
        let (dx, dy) = (x - self.x, y - self.y);
        if dx == 0 && dy == 0 {
            return;
        }
        self.out.push('I');
        self.goto_xy(dx, dy, None);
        self.out.push_str("S1P\n");
        if !self.opts.autolock {
            self.out.push_str("IS2P\n");
        }
    }

    /// Homes the head (`IPP`).
    pub fn home(&mut self) {
        self.rapid_mode();
        self.out.push_str("IPP\n");
        self.x = 0;
        self.y = 0;
    }

    fn declare_directions(&mut self) {
        let x_dir = if self.leftward { CODE_LEFT } else { CODE_RIGHT };
        let y_dir = if self.topward { CODE_TOP } else { CODE_BOTTOM };
        if self.horizontal_major {
            self.x_engaged = true;
            self.y_engaged = false;
            self.out.push(y_dir);
            self.out.push(x_dir);
        } else {
            self.x_engaged = false;
            self.y_engaged = true;
            self.out.push(x_dir);
            self.out.push(y_dir);
        }
    }

    /// Enters program (vector) mode at `speed` mm/s. `first` is the first move that will follow
    /// (relative, mils); it sets the declared directions.
    pub fn program_mode(&mut self, speed: f64, first: (i64, i64)) {
        self.finished_mode();
        let (dx, dy) = first;
        self.horizontal_major = dx.abs() > dy.abs();
        self.leftward = dx <= 0;
        self.topward = dy <= 0;
        // The "suffix C" form (no accel twitches), as the reference driver uses by default. The
        // form always writes acceleration digit 1, so the value is worked out for 1 as well
        // (the reference uses the speed's own acceleration, which only matters for boards
        // B2, A, B and B1 above 25.4 mm/s, where it reads back as a different speed).
        let o = SpeedOptions {
            board: self.opts.board,
            acceleration: Some(1),
            suffix_c: Some(true),
            fix_limit: true,
            fix_lows: true,
            fix_speeds: self.opts.fix_speeds,
            raster_horizontal: self.horizontal_major,
            ..Default::default()
        };
        self.out.push_str(&speed_code(speed, &o));
        self.out.push('N');
        self.declare_directions();
        self.out.push_str("S1E");
        self.raster_step = 0;
        self.laser = false;
        self.mode = Mode::Program;
    }

    /// Enters horizontal raster mode at `speed` mm/s with a raster step of `step` mils. The
    /// first row runs left when `leftward`; rows follow upwards when `topward`.
    pub fn raster_mode(&mut self, speed: f64, step: u32, leftward: bool, topward: bool) -> Result<(), WriterError> {
        if step == 0 || step > 999 {
            return Err(WriterError(format!("raster step {step} is outside 1..=999 mils")));
        }
        self.finished_mode();
        self.horizontal_major = true;
        self.leftward = leftward;
        self.topward = topward;
        let o = SpeedOptions {
            board: self.opts.board,
            raster_step: Some(RasterStep::Bidirectional(step)),
            suffix_c: Some(false),
            fix_limit: true,
            fix_lows: true,
            fix_speeds: self.opts.fix_speeds,
            raster_horizontal: true,
            ..Default::default()
        };
        self.out.push_str(&speed_code(speed, &o));
        self.out.push('N');
        self.declare_directions();
        self.out.push_str("S1E");
        self.raster_step = step;
        self.laser = false;
        self.mode = Mode::Raster;
        Ok(())
    }

    fn set_laser(&mut self, on: bool) {
        if on != self.laser {
            self.out.push(if on { CODE_LASER_ON } else { CODE_LASER_OFF });
            self.laser = on;
        }
    }

    fn goto_xy(&mut self, dx: i64, dy: i64, on: Option<bool>) {
        let rapid = !matches!(self.mode, Mode::Program | Mode::Raster);
        if dx != 0 {
            self.x += dx;
            if dx > 0 {
                if !self.is_right() || rapid {
                    self.out.push(CODE_RIGHT);
                    self.leftward = false;
                }
            } else if !self.is_left() || rapid {
                self.out.push(CODE_LEFT);
                self.leftward = true;
            }
            self.x_engaged = true;
            self.y_engaged = false;
            if let Some(on) = on {
                self.set_laser(on);
            }
            push_distance(&mut self.out, dx.unsigned_abs() as u32);
        }
        if dy != 0 {
            self.y += dy;
            if dy > 0 {
                if !self.is_bottom() || rapid {
                    self.out.push(CODE_BOTTOM);
                    self.topward = false;
                }
            } else if !self.is_top() || rapid {
                self.out.push(CODE_TOP);
                self.topward = true;
            }
            self.x_engaged = false;
            self.y_engaged = true;
            if let Some(on) = on {
                self.set_laser(on);
            }
            push_distance(&mut self.out, dy.unsigned_abs() as u32);
        }
    }

    /// One orthogonal or 45 degree move.
    fn goto_octent(&mut self, dx: i64, dy: i64, on: bool) {
        if dx == 0 && dy == 0 {
            return;
        }
        if dx.abs() != dy.abs() {
            self.goto_xy(dx, dy, Some(on));
            return;
        }
        self.x_engaged = true;
        self.y_engaged = true;
        if dx > 0 {
            if self.leftward {
                self.out.push(CODE_RIGHT);
                self.leftward = false;
            }
        } else if !self.leftward {
            self.out.push(CODE_LEFT);
            self.leftward = true;
        }
        if dy > 0 {
            if self.topward {
                self.out.push(CODE_BOTTOM);
                self.topward = false;
            }
        } else if !self.topward {
            self.out.push(CODE_TOP);
            self.topward = true;
        }
        self.x += dx;
        self.y += dy;
        self.out.push(CODE_ANGLE);
        self.set_laser(on);
        push_distance(&mut self.out, dy.unsigned_abs() as u32);
    }

    /// A straight move to `(x, y)` mils in program mode, laser on or off. Lines that are not
    /// orthogonal or 45 degrees are stepped (Bresenham) into runs of such moves.
    pub fn line_to(&mut self, x: i64, y: i64, on: bool) -> Result<(), WriterError> {
        if self.mode != Mode::Program {
            return Err(WriterError("a line needs program mode".into()));
        }
        let (dx, dy) = (x - self.x, y - self.y);
        if dx == 0 && dy == 0 {
            return Ok(());
        }
        let mut last = (0, 0);
        for p in octant_runs(dx, dy) {
            self.goto_octent(p.0 - last.0, p.1 - last.1, on);
            last = p;
        }
        Ok(())
    }

    /// Runs along the current raster row to `x` mils, laser on or off. The run must go in the
    /// row's direction (a reversal would step to the next row).
    pub fn raster_run_to(&mut self, x: i64, on: bool) -> Result<(), WriterError> {
        if self.mode != Mode::Raster {
            return Err(WriterError("a raster run needs raster mode".into()));
        }
        let dx = x - self.x;
        if dx == 0 {
            return Ok(());
        }
        if (dx < 0) != self.leftward {
            return Err(WriterError(format!("raster run of {dx} mils goes against the row direction")));
        }
        self.goto_xy(dx, 0, Some(on));
        Ok(())
    }

    /// Ends the row with the laser off, reverses and steps to the row at `y` mils. The step is
    /// the raster step; up to [`MAX_RASTER_CORRECTION`] extra mils (in the same direction) are
    /// added with a 45 degree move before the reversal.
    pub fn raster_next_row(&mut self, y: i64) -> Result<(), WriterError> {
        if self.mode != Mode::Raster {
            return Err(WriterError("a raster row change needs raster mode".into()));
        }
        let step = self.raster_step as i64;
        let step_amount = if self.topward { -step } else { step };
        let remaining = (y - self.y) - step_amount;
        if (remaining > 0 && self.topward) || (remaining < 0 && !self.topward) || remaining.abs() > MAX_RASTER_CORRECTION {
            return Err(WriterError(format!("row change of {} mils does not fit a raster step of {step}", y - self.y)));
        }
        self.set_laser(false);
        if remaining != 0 {
            let dx = if self.leftward { -remaining.abs() } else { remaining.abs() };
            self.goto_octent(dx, remaining, false);
        }
        self.x_engaged = true;
        self.y_engaged = false;
        if self.leftward {
            self.out.push(CODE_RIGHT);
            self.leftward = false;
        } else {
            self.out.push(CODE_LEFT);
            self.leftward = true;
        }
        self.y += step_amount;
        self.laser = false;
        Ok(())
    }

    /// The current raster row direction (true = left).
    pub fn raster_leftward(&self) -> bool {
        self.leftward
    }
}

/// End points of the orthogonal / 45 degree runs that step a line from 0,0 to `(dx, dy)`
/// (Zingl's Bresenham line, grouped by step direction). The last point is `(dx, dy)`.
pub fn octant_runs(dx: i64, dy: i64) -> Vec<(i64, i64)> {
    let mut out = Vec::new();
    let (x1, y1) = (dx, dy);
    let ddx = x1.abs();
    let ddy = -y1.abs();
    let sx = if 0 < x1 { 1 } else { -1 };
    let sy = if 0 < y1 { 1 } else { -1 };
    let mut err = ddx + ddy;
    let (mut x, mut y) = (0i64, 0i64);
    let mut dir = (0i64, 0i64);
    loop {
        if x == x1 && y == y1 {
            break;
        }
        let (px, py) = (x, y);
        let e2 = 2 * err;
        if e2 >= ddy {
            err += ddy;
            x += sx;
        }
        if e2 <= ddx {
            err += ddx;
            y += sy;
        }
        let d = (x - px, y - py);
        if d != dir && dir != (0, 0) {
            out.push((px, py));
        }
        dir = d;
    }
    out.push((x1, y1));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// MeerK40t's driver test: a 1 cm square at 2 cm, 2 cm, M2, 15 mm/s.
    const RECT: &str = "IBzzzvRzzzvS1P\nICV2490731016000027CNLBS1EDz139Rz139Tz139Lz139FNSE-\n";

    #[test]
    fn square_matches_the_reference_driver() {
        let mut w = EgvWriter::new(WriterOptions::default());
        w.move_to(787, 787);
        w.program_mode(15.0, (394, 0));
        w.line_to(1181, 787, true).unwrap();
        w.line_to(1181, 1181, true).unwrap();
        w.line_to(787, 1181, true).unwrap();
        w.line_to(787, 787, true).unwrap();
        assert_eq!(w.finish(), RECT);
    }

    #[test]
    fn unlocked_rail_after_travel() {
        let mut w = EgvWriter::new(WriterOptions { autolock: false, ..Default::default() });
        w.move_to(10, 0);
        assert_eq!(w.finish(), "IBjS1P\nIS2P\n");
    }

    #[test]
    fn travel_always_names_both_directions() {
        let mut w = EgvWriter::new(WriterOptions::default());
        w.move_to(-5, 0);
        w.move_to(-10, 0);
        w.move_to(-10, -3);
        assert_eq!(w.finish(), "ITeS1P\nITeS1P\nILcS1P\n");
    }

    #[test]
    fn zero_moves_write_nothing() {
        let mut w = EgvWriter::new(WriterOptions::default());
        w.move_to(0, 0);
        assert_eq!(w.text(), "");
        w.program_mode(10.0, (1, 0));
        let before = w.text().len();
        w.line_to(0, 0, true).unwrap();
        assert_eq!(w.text().len(), before);
    }

    #[test]
    fn diagonal_lines_use_m() {
        let mut w = EgvWriter::new(WriterOptions::default());
        w.program_mode(10.0, (5, 5));
        w.line_to(5, 5, true).unwrap();
        assert!(w.text().ends_with("MDe"), "{}", w.text());
    }

    #[test]
    fn laser_switches_only_when_it_changes() {
        let mut w = EgvWriter::new(WriterOptions::default());
        w.program_mode(10.0, (5, 0));
        w.line_to(5, 0, true).unwrap();
        w.line_to(10, 0, true).unwrap();
        w.line_to(15, 0, false).unwrap();
        let t = w.finish();
        assert!(t.contains("S1EDeeUe"), "{t}");
    }

    #[test]
    fn lines_need_program_mode() {
        let mut w = EgvWriter::new(WriterOptions::default());
        assert!(w.line_to(5, 0, true).is_err());
        assert!(w.raster_run_to(5, true).is_err());
        assert!(w.raster_next_row(5).is_err());
        assert!(w.raster_mode(10.0, 0, false, false).is_err());
    }

    #[test]
    fn octant_runs_end_at_the_target() {
        for (dx, dy) in [(5, 2), (-7, 3), (3, -11), (-4, -4), (10, 0), (0, -6), (1, 1000), (1000, 1), (-123, 77)] {
            let runs = octant_runs(dx, dy);
            assert_eq!(*runs.last().unwrap(), (dx, dy));
            let mut last = (0, 0);
            for p in runs {
                let (a, b) = (p.0 - last.0, p.1 - last.1);
                assert!(a == 0 || b == 0 || a.abs() == b.abs(), "{dx},{dy}: step {a},{b}");
                last = p;
            }
        }
        assert_eq!(octant_runs(10, 0), vec![(10, 0)]);
        assert_eq!(octant_runs(4, 4), vec![(4, 4)]);
    }

    #[test]
    fn program_mode_from_program_goes_through_rapid() {
        let mut w = EgvWriter::new(WriterOptions::default());
        w.program_mode(10.0, (5, 0));
        w.line_to(5, 0, true).unwrap();
        w.program_mode(20.0, (5, 0));
        assert!(w.text().contains("FNSE-\nICV"), "{}", w.text());
        assert!(!w.text().contains('@'));
    }

    #[test]
    fn raster_rows_reverse_and_step() {
        let mut w = EgvWriter::new(WriterOptions::default());
        w.raster_mode(15.0, 2, false, true).unwrap();
        assert!(w.text().starts_with("IV1552121G002NLBS1E"), "{}", w.text());
        w.raster_run_to(10, true).unwrap();
        assert!(w.raster_run_to(5, true).is_err());
        w.raster_next_row(-2).unwrap();
        assert!(w.raster_leftward());
        w.raster_run_to(0, true).unwrap();
        assert_eq!(w.position(), (0, -2));
        let t = w.finish();
        assert!(t.ends_with("DjUTDjFNSE-\n"), "{t}");
    }

    #[test]
    fn raster_row_change_corrects_small_differences() {
        let mut w = EgvWriter::new(WriterOptions::default());
        w.raster_mode(15.0, 3, false, false).unwrap();
        w.raster_run_to(10, true).unwrap();
        w.raster_next_row(4).unwrap();
        assert_eq!(w.position(), (11, 4));
        assert!(w.text().ends_with("DjUMaT"), "{}", w.text());
        assert!(w.raster_next_row(30).is_err());
        assert!(w.raster_next_row(1).is_err());
    }

    #[test]
    fn home_resets_position() {
        let mut w = EgvWriter::starting_at(WriterOptions::default(), (100, 100));
        w.home();
        assert_eq!(w.position(), (0, 0));
        assert_eq!(w.finish(), "IPP\n");
    }
}
