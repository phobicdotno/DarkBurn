//! Bug hunt 10b: the simulator with odd motion settings and odd operations.

use darkburn_job::sim::simulate;
use darkburn_job::{CoordSpace, MotionSettings, Op, Toolpath};
use kurbo::Point;

fn tp(ops: Vec<Op>) -> Toolpath {
    Toolpath { space: CoordSpace::Design, layers: Vec::new(), ops, start: Point::ZERO }
}

/// A dwell of negative or NaN length takes no time; it never turns the clock back.
#[test]
fn hunt10b_a_negative_dwell_does_not_turn_time_back() {
    let cut = Op::Cut { to: Point::new(100.0, 0.0), speed: 10.0, power: 50.0 };
    let m = MotionSettings { accel: 100_000.0, ..Default::default() };
    let plain = simulate(&tp(vec![cut.clone()]), &m).total_time;
    for ms in [-5000.0, f64::NAN] {
        let s = simulate(&tp(vec![Op::Dwell { ms, power: 0.0 }, cut.clone()]), &m);
        assert!((s.total_time - plain).abs() < 1e-9, "dwell {ms} ms: {} s, the cut alone {plain} s", s.total_time);
        let mut t0 = 0.0;
        for seg in &s.segments {
            assert!(seg.t0 >= t0 && seg.duration >= 0.0, "dwell {ms} ms: time runs back at {seg:?}");
            t0 = seg.t0;
        }
    }
}
