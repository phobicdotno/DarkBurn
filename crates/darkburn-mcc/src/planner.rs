//! Planner dry run: a neutral toolpath in, 12-byte FIFO ticks and a time estimate out. Nothing more.
//!
//! What this is: a conversion of straight moves and cuts at a given speed into opcode-3000 ticks
//! (motor pulses per tick, see [`crate::fifo`]), packed into frames, plus a duration estimate.
//!
//! What this is not: it emits **no** laser data (every tick has duty 0), no digital-output,
//! gas, PWM or analogue records, no follower or pierce records and no dwell records. A cut and a
//! rapid produce identical ticks; the difference is only in the statistics. The vendor's own
//! height follower, pierce sequence and gas logic stay in charge of everything beyond motion
//! (`docs/BUILD-RULES.md`), and nothing here asks the card to bypass them. Real-card use needs the
//! read-only capture described in `99-gaps.md` first (tick period, register 1016 units, stop
//! vectors); the defaults below are the owner's static-analysis values.
//!
//! The motion profile is a simple trapezoid from rest to rest per segment. The vendor planner has
//! look-ahead and corner blending that this estimate does not model, so the estimate is an upper
//! bound for chains of short segments.

use crate::fifo::{FifoFeeder, FifoItem, FramePacker, PackedFrame};
use kurbo::Point;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Kind of a path segment.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SegmentKind {
    /// Travel without cutting.
    Rapid,
    /// Cutting feed. Only the statistics differ from a rapid.
    Cut,
}

/// One straight segment ending at `to` (millimetres, Y up, machine origin).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PathSegment {
    /// End point.
    pub to: Point,
    /// Rapid or cut.
    pub kind: SegmentKind,
    /// Speed in mm/s. `None` uses the rapid speed of the config for rapids and is an error for cuts.
    pub speed_mm_s: Option<f64>,
}

impl PathSegment {
    /// A rapid move at the configured rapid speed.
    pub fn rapid(x: f64, y: f64) -> PathSegment {
        PathSegment { to: Point::new(x, y), kind: SegmentKind::Rapid, speed_mm_s: None }
    }

    /// A cut at `speed_mm_s`.
    pub fn cut(x: f64, y: f64, speed_mm_s: f64) -> PathSegment {
        PathSegment { to: Point::new(x, y), kind: SegmentKind::Cut, speed_mm_s: Some(speed_mm_s) }
    }
}

/// Planner configuration. Defaults are the owner's CF1390 values from static analysis.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlanConfig {
    /// Where the head is when the path starts.
    pub start: Point,
    /// Motor pulses per mm, X: `8000 / 31.003`.
    pub pulses_per_mm_x: f64,
    /// Motor pulses per mm, Y: `8000 / 31.009`.
    pub pulses_per_mm_y: f64,
    /// Tick period in microseconds (register 50005, default 250, unconfirmed).
    pub tick_us: f64,
    /// Acceleration in mm/s^2 for the trapezoid.
    pub accel_mm_s2: f64,
    /// Rapid speed in mm/s.
    pub rapid_speed_mm_s: f64,
    /// Speed ceiling in mm/s (`FCP.MaxSpeed`).
    pub max_speed_mm_s: f64,
    /// Work area `(width, height)` in mm; a segment ending outside `0..=w`, `0..=h` is an error.
    pub work_area_mm: (f64, f64),
    /// Refuse plans longer than this many ticks.
    pub max_ticks: u64,
}

impl Default for PlanConfig {
    fn default() -> PlanConfig {
        PlanConfig {
            start: Point::new(0.0, 0.0),
            pulses_per_mm_x: 8000.0 / 31.003,
            pulses_per_mm_y: 8000.0 / 31.009,
            tick_us: 250.0,
            accel_mm_s2: 5999.99,
            rapid_speed_mm_s: 550.0,
            max_speed_mm_s: 3000.0,
            work_area_mm: (1371.0, 950.0),
            max_ticks: 40_000_000,
        }
    }
}

/// Why a plan could not be made.
#[derive(Clone, Debug, PartialEq)]
pub enum PlanError {
    /// A coordinate or speed is not finite.
    NotFinite,
    /// A cut has no speed, or a speed is not positive.
    BadSpeed(f64),
    /// Speed above the ceiling.
    SpeedAboveLimit(f64),
    /// A point lies outside the work area.
    OutOfBounds(Point),
    /// More pulses in one tick than a signed 16-bit field holds.
    TickOverflow(f64),
    /// The plan would exceed [`PlanConfig::max_ticks`].
    TooLong,
    /// The configuration itself is unusable.
    BadConfig(&'static str),
}

impl fmt::Display for PlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlanError::NotFinite => write!(f, "coordinate or speed is not finite"),
            PlanError::BadSpeed(v) => write!(f, "speed {v} mm/s is missing or not positive"),
            PlanError::SpeedAboveLimit(v) => write!(f, "speed {v} mm/s is above the machine limit"),
            PlanError::OutOfBounds(p) => write!(f, "point ({}, {}) is outside the work area", p.x, p.y),
            PlanError::TickOverflow(v) => write!(f, "{v} pulses in one tick do not fit 16 bits"),
            PlanError::TooLong => write!(f, "plan exceeds the tick limit"),
            PlanError::BadConfig(s) => write!(f, "bad planner configuration: {s}"),
        }
    }
}

impl std::error::Error for PlanError {}

/// Result of a dry run.
#[derive(Clone, Debug, PartialEq)]
pub struct DryRunPlan {
    /// FIFO frames, ready for [`FifoFeeder`].
    pub frames: Vec<PackedFrame>,
    /// Number of 12-byte ticks.
    pub tick_count: u64,
    /// Sum of X pulses over all ticks.
    pub total_pulses_x: i64,
    /// Sum of Y pulses over all ticks.
    pub total_pulses_y: i64,
    /// Estimated duration: ticks times the tick period.
    pub estimated_seconds: f64,
    /// Estimated time spent in cut segments.
    pub cut_seconds: f64,
    /// Estimated time spent in rapids.
    pub rapid_seconds: f64,
    /// Cut length in mm.
    pub cut_length_mm: f64,
    /// Rapid length in mm.
    pub rapid_length_mm: f64,
    /// Where the head ends.
    pub end: Point,
}

impl DryRunPlan {
    /// Hand the frames to a flow-control feeder.
    pub fn feeder(&self) -> FifoFeeder {
        let mut f = FifoFeeder::new(self.frames.clone());
        f.push(Vec::new(), true);
        f
    }

    /// Bytes of ticks (12 per tick).
    pub fn tick_bytes(&self) -> u64 {
        self.tick_count * 12
    }
}

/// Trapezoid from rest to rest over `len` mm with peak speed `v` and acceleration `a`.
/// Returns `(total time, position at time)`.
struct Profile {
    len: f64,
    a: f64,
    vp: f64,
    t_acc: f64,
    t_cruise: f64,
    total: f64,
}

impl Profile {
    fn new(len: f64, v: f64, a: f64) -> Profile {
        let d_acc = v * v / a; // distance for accelerate + decelerate
        if len >= d_acc {
            let t_acc = v / a;
            let t_cruise = (len - d_acc) / v;
            Profile { len, a, vp: v, t_acc, t_cruise, total: 2.0 * t_acc + t_cruise }
        } else {
            let t_acc = (len / a).sqrt();
            Profile { len, a, vp: a * t_acc, t_acc, t_cruise: 0.0, total: 2.0 * t_acc }
        }
    }

    fn pos(&self, t: f64) -> f64 {
        let t = t.clamp(0.0, self.total);
        let s = if t <= self.t_acc {
            0.5 * self.a * t * t
        } else if t <= self.t_acc + self.t_cruise {
            0.5 * self.a * self.t_acc * self.t_acc + self.vp * (t - self.t_acc)
        } else {
            let r = self.total - t;
            self.len - 0.5 * self.a * r * r
        };
        s.clamp(0.0, self.len)
    }
}

/// Convert a toolpath into FIFO ticks and a time estimate.
pub fn dry_run(path: &[PathSegment], cfg: &PlanConfig) -> Result<DryRunPlan, PlanError> {
    let positive = |v: f64| v.is_finite() && v > 0.0;
    if !(positive(cfg.pulses_per_mm_x) && positive(cfg.pulses_per_mm_y)) {
        return Err(PlanError::BadConfig("pulses per mm must be positive"));
    }
    if !(positive(cfg.tick_us) && positive(cfg.accel_mm_s2) && positive(cfg.rapid_speed_mm_s) && positive(cfg.max_speed_mm_s)) {
        return Err(PlanError::BadConfig("tick period, acceleration and speeds must be positive"));
    }
    let (w, h) = cfg.work_area_mm;
    let inside = |p: Point| p.x.is_finite() && p.y.is_finite() && (0.0..=w).contains(&p.x) && (0.0..=h).contains(&p.y);
    if !inside(cfg.start) {
        return Err(PlanError::OutOfBounds(cfg.start));
    }
    let tick_s = cfg.tick_us * 1e-6;
    let mut packer = FramePacker::new();
    let mut plan = DryRunPlan {
        frames: Vec::new(),
        tick_count: 0,
        total_pulses_x: 0,
        total_pulses_y: 0,
        estimated_seconds: 0.0,
        cut_seconds: 0.0,
        rapid_seconds: 0.0,
        cut_length_mm: 0.0,
        rapid_length_mm: 0.0,
        end: cfg.start,
    };
    let (mut carry_x, mut carry_y) = (0.0f64, 0.0f64);
    let mut here = cfg.start;
    for seg in path {
        if !(seg.to.x.is_finite() && seg.to.y.is_finite()) {
            return Err(PlanError::NotFinite);
        }
        if !inside(seg.to) {
            return Err(PlanError::OutOfBounds(seg.to));
        }
        let speed = match (seg.speed_mm_s, seg.kind) {
            (Some(v), _) => v,
            (None, SegmentKind::Rapid) => cfg.rapid_speed_mm_s,
            (None, SegmentKind::Cut) => return Err(PlanError::BadSpeed(f64::NAN)),
        };
        if !speed.is_finite() {
            return Err(PlanError::NotFinite);
        }
        if speed <= 0.0 {
            return Err(PlanError::BadSpeed(speed));
        }
        if speed > cfg.max_speed_mm_s {
            return Err(PlanError::SpeedAboveLimit(speed));
        }
        let (dx, dy) = (seg.to.x - here.x, seg.to.y - here.y);
        let len = dx.hypot(dy);
        if len < 1e-9 {
            here = seg.to;
            continue;
        }
        let (ux, uy) = (dx / len, dy / len);
        let prof = Profile::new(len, speed, cfg.accel_mm_s2);
        let n = (prof.total / tick_s).ceil().max(1.0) as u64;
        if plan.tick_count + n > cfg.max_ticks {
            return Err(PlanError::TooLong);
        }
        let mut prev_s = 0.0;
        for k in 1..=n {
            let s = if k == n { len } else { prof.pos(k as f64 * tick_s) };
            let ds = s - prev_s;
            prev_s = s;
            let ex = ds * ux * cfg.pulses_per_mm_x + carry_x;
            let ey = ds * uy * cfg.pulses_per_mm_y + carry_y;
            for v in [ex, ey] {
                if v.abs() > i16::MAX as f64 {
                    return Err(PlanError::TickOverflow(v));
                }
            }
            let (px, py) = (ex.trunc(), ey.trunc());
            carry_x = ex - px;
            carry_y = ey - py;
            plan.total_pulses_x += px as i64;
            plan.total_pulses_y += py as i64;
            packer.add_record(&[FifoItem::tick(px as i16, py as i16)]);
        }
        plan.tick_count += n;
        let secs = n as f64 * tick_s;
        match seg.kind {
            SegmentKind::Cut => {
                plan.cut_length_mm += len;
                plan.cut_seconds += secs;
            }
            SegmentKind::Rapid => {
                plan.rapid_length_mm += len;
                plan.rapid_seconds += secs;
            }
        }
        here = seg.to;
    }
    plan.frames = packer.finish();
    plan.estimated_seconds = plan.tick_count as f64 * tick_s;
    plan.end = here;
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command;
    use crate::fifo::{StopReason, parse_fifo_words};

    fn cfg() -> PlanConfig {
        PlanConfig { start: Point::new(10.0, 10.0), accel_mm_s2: 1000.0, ..PlanConfig::default() }
    }

    #[test]
    fn pure_x_cut_matches_the_hand_calculation() {
        // 100 mm at 50 mm/s, a = 1000: cruise 100 - 2.5 = 97.5 mm -> 1.95 s + 2 * 0.05 s = 2.05 s
        let plan = dry_run(&[PathSegment::cut(110.0, 10.0, 50.0)], &cfg()).unwrap();
        assert_eq!(plan.tick_count, 8200);
        assert!((plan.estimated_seconds - 2.05).abs() < 1e-9);
        assert!((plan.cut_seconds - 2.05).abs() < 1e-9);
        assert_eq!(plan.rapid_seconds, 0.0);
        let expect = 100.0 * 8000.0 / 31.003;
        assert!((plan.total_pulses_x as f64 - expect).abs() < 1.0, "{} vs {expect}", plan.total_pulses_x);
        assert_eq!(plan.total_pulses_y, 0);
        assert!((plan.cut_length_mm - 100.0).abs() < 1e-9);
        assert_eq!(plan.end, Point::new(110.0, 10.0));
        assert_eq!(plan.tick_bytes(), 8200 * 12);
    }

    #[test]
    fn short_segment_never_reaches_cruise() {
        // 1 mm at 500 mm/s, a = 1000: triangular, total = 2 * sqrt(1/1000)
        let plan = dry_run(&[PathSegment::cut(11.0, 10.0, 500.0)], &cfg()).unwrap();
        let t = 2.0 * (1.0f64 / 1000.0).sqrt();
        assert!((plan.estimated_seconds - t).abs() < 0.0003, "{} vs {t}", plan.estimated_seconds);
    }

    #[test]
    fn diagonal_pulses_follow_the_axis_scales_and_carry() {
        let plan = dry_run(&[PathSegment::rapid(40.0, 50.0)], &cfg()).unwrap();
        let ex = 30.0 * 8000.0 / 31.003;
        let ey = 40.0 * 8000.0 / 31.009;
        assert!((plan.total_pulses_x as f64 - ex).abs() < 1.0);
        assert!((plan.total_pulses_y as f64 - ey).abs() < 1.0);
        assert!(plan.rapid_seconds > 0.0 && plan.cut_seconds == 0.0);
        // many short segments must not drift: carry is kept across them
        let mut path = Vec::new();
        for i in 1..=100 {
            path.push(PathSegment::cut(10.0 + i as f64 * 0.1, 10.0, 20.0));
        }
        let plan = dry_run(&path, &cfg()).unwrap();
        let expect = 10.0 * 8000.0 / 31.003;
        assert!((plan.total_pulses_x as f64 - expect).abs() < 1.0, "{} vs {expect}", plan.total_pulses_x);
    }

    #[test]
    fn ticks_are_dry_and_frames_are_valid_writes() {
        let plan = dry_run(&[PathSegment::cut(60.0, 10.0, 100.0), PathSegment::cut(60.0, 60.0, 100.0)], &cfg()).unwrap();
        let mut ticks = 0u64;
        let (mut sx, mut sy) = (0i64, 0i64);
        for (i, f) in plan.frames.iter().enumerate() {
            // every frame is acceptable to the safe-write gate at motion level
            let w = command::fifo_packed_frame(i as u32 + 1, f).unwrap();
            assert_eq!(w.policy(), command::Policy::Motion);
            let parsed = parse_fifo_words(w.words()).unwrap();
            for item in parsed.items {
                let t = item.as_tick().expect("only ticks");
                assert_eq!(t.duty, 0, "dry run never turns the laser on");
                assert!(!item.is_laser());
                sx += t.dx as i64;
                sy += t.dy as i64;
                ticks += 1;
            }
        }
        assert_eq!(ticks, plan.tick_count);
        assert_eq!((sx, sy), (plan.total_pulses_x, plan.total_pulses_y));
        // all frames except the last are the 99-tick 297-word frames
        assert!(plan.frames[..plan.frames.len() - 1].iter().all(|f| f.data.len() == 297));
    }

    #[test]
    fn feeder_accepts_the_plan() {
        let plan = dry_run(&[PathSegment::cut(40.0, 10.0, 100.0)], &cfg()).unwrap();
        let mut feeder = plan.feeder();
        let r = feeder.fill(0, 60_000, 50);
        assert_eq!(r.frames.len(), plan.frames.len());
        assert_eq!(r.stopped_by, StopReason::Empty);
        assert!(feeder.job_finished(60_000, 1));
    }

    #[test]
    fn rejects_bad_input() {
        let c = cfg();
        assert!(matches!(dry_run(&[PathSegment::cut(2000.0, 10.0, 10.0)], &c), Err(PlanError::OutOfBounds(_))));
        assert!(matches!(dry_run(&[PathSegment::cut(-1.0, 10.0, 10.0)], &c), Err(PlanError::OutOfBounds(_))));
        assert!(matches!(dry_run(&[PathSegment::cut(20.0, 10.0, 0.0)], &c), Err(PlanError::BadSpeed(_))));
        assert!(matches!(dry_run(&[PathSegment::cut(20.0, 10.0, -5.0)], &c), Err(PlanError::BadSpeed(_))));
        assert!(matches!(dry_run(&[PathSegment::cut(20.0, 10.0, 5000.0)], &c), Err(PlanError::SpeedAboveLimit(_))));
        assert!(matches!(dry_run(&[PathSegment::cut(f64::NAN, 10.0, 10.0)], &c), Err(PlanError::NotFinite)));
        assert!(matches!(dry_run(&[PathSegment::cut(20.0, 10.0, f64::NAN)], &c), Err(PlanError::NotFinite)));
        let no_speed = PathSegment { to: Point::new(20.0, 10.0), kind: SegmentKind::Cut, speed_mm_s: None };
        assert!(matches!(dry_run(&[no_speed], &c), Err(PlanError::BadSpeed(_))));
        let tiny = PlanConfig { max_ticks: 10, ..cfg() };
        assert_eq!(dry_run(&[PathSegment::cut(100.0, 10.0, 50.0)], &tiny), Err(PlanError::TooLong));
        let bad = PlanConfig { tick_us: 0.0, ..cfg() };
        assert!(matches!(dry_run(&[], &bad), Err(PlanError::BadConfig(_))));
        let bad = PlanConfig { start: Point::new(-5.0, 0.0), ..cfg() };
        assert!(matches!(dry_run(&[], &bad), Err(PlanError::OutOfBounds(_))));
    }

    #[test]
    fn overflow_is_reported_not_wrapped() {
        // 3000 mm/s with a huge pulse scale would need more than 32767 pulses per 250 us tick
        let c = PlanConfig { pulses_per_mm_x: 50_000.0, accel_mm_s2: 1.0e9, ..cfg() };
        assert!(matches!(dry_run(&[PathSegment::cut(1000.0, 10.0, 3000.0)], &c), Err(PlanError::TickOverflow(_))));
    }

    #[test]
    fn empty_and_degenerate_paths() {
        let plan = dry_run(&[], &cfg()).unwrap();
        assert_eq!(plan.tick_count, 0);
        assert!(plan.frames.is_empty());
        let plan = dry_run(&[PathSegment::cut(10.0, 10.0, 10.0)], &cfg()).unwrap();
        assert_eq!(plan.tick_count, 0);
        assert_eq!(plan.estimated_seconds, 0.0);
    }

    #[test]
    fn config_and_path_serialise() {
        let c = PlanConfig::default();
        let json = serde_json::to_string(&c).unwrap();
        assert_eq!(serde_json::from_str::<PlanConfig>(&json).unwrap(), c);
        let p = PathSegment::cut(1.0, 2.0, 3.0);
        assert_eq!(serde_json::from_str::<PathSegment>(&serde_json::to_string(&p).unwrap()).unwrap(), p);
    }
}
