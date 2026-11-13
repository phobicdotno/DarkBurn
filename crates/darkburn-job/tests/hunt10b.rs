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

/// An acceleration of zero, below zero or NaN falls back to the default one, so the job time is a real number.
#[test]
fn hunt10b_a_bad_acceleration_uses_the_default() {
    let ops =
        vec![Op::Cut { to: Point::new(100.0, 0.0), speed: 10.0, power: 50.0 }, Op::Cut { to: Point::new(100.0, 50.0), speed: 10.0, power: 50.0 }];
    let good = simulate(&tp(ops.clone()), &MotionSettings::default()).total_time;
    assert!(good.is_finite() && good > 0.0);
    for a in [0.0, -500.0, f64::NAN, f64::INFINITY] {
        let m = MotionSettings { accel: a, engrave_accel: a, ..Default::default() };
        let s = simulate(&tp(ops.clone()), &m);
        assert!((s.total_time - good).abs() < 1e-9, "accel {a}: {} s, with the default accel {good} s", s.total_time);
        for seg in &s.segments {
            assert!(seg.duration.is_finite() && seg.duration >= 0.0, "accel {a}: {seg:?}");
        }
        let end = s.state_at(s.total_time).unwrap().pos;
        assert!(end.distance(Point::new(100.0, 50.0)) < 1e-6, "accel {a}: ends at {end:?}");
    }
}

/// A NaN cut speed is simulated like a speed of 0, not at the machine's top speed.
#[test]
fn hunt10b_a_nan_speed_is_not_the_top_speed() {
    let m = MotionSettings::default();
    let line = |speed| simulate(&tp(vec![Op::Cut { to: Point::new(10.0, 0.0), speed, power: 50.0 }]), &m).total_time;
    let arc = |speed| {
        simulate(&tp(vec![Op::Arc { to: Point::new(10.0, 0.0), center: Point::new(5.0, 0.0), clockwise: true, speed, power: 50.0 }]), &m).total_time
    };
    for (name, f) in [("line", &line as &dyn Fn(f64) -> f64), ("arc", &arc)] {
        let zero = f(0.0);
        let nan = f(f64::NAN);
        assert_eq!(nan, zero, "{name}: NaN speed takes {nan} s, speed 0 takes {zero} s");
        assert!(nan > f(m.max_speed), "{name}: NaN speed is as fast as the top speed");
    }
}

/// A very slow job does not make a timeline with hundreds of millions of states.
#[test]
fn hunt10b_a_slow_job_timeline_stays_small() {
    let s = simulate(&tp(vec![Op::Cut { to: Point::new(10.0, 0.0), speed: 0.0, power: 50.0 }]), &MotionSettings::default());
    assert!(s.total_time > 1e6, "speed 0 should be very slow, took {} s", s.total_time);
    let started = std::time::Instant::now();
    let tl = s.timeline(0.05);
    assert!(tl.len() <= 100_001, "{} states", tl.len());
    assert!(tl.len() >= 2);
    assert!((tl.last().unwrap().t - s.total_time).abs() < 1e-6);
    assert!(tl.windows(2).all(|w| w[1].t >= w[0].t));
    assert!(started.elapsed().as_secs() < 5, "took {:?}", started.elapsed());
}

/// An empty open or closed shape has nothing to cut: plan() leaves it out instead of panicking,
/// and the other shapes come out in the same order as without it.
#[test]
fn hunt10b_plan_skips_empty_items() {
    use darkburn_job::PlannerSettings;
    use darkburn_job::Progress;
    use darkburn_job::planner::{ItemGeom, PlanItem, plan};
    let square = |x: f64| ItemGeom::Closed(vec![Point::new(x, 0.0), Point::new(x + 10.0, 0.0), Point::new(x + 10.0, 10.0), Point::new(x, 10.0)]);
    let line = |x: f64| ItemGeom::Open(vec![Point::new(x, 0.0), Point::new(x + 5.0, 5.0)]);
    let mut forced = PlanItem::new(ItemGeom::Closed(Vec::new()));
    forced.forced_start = Some(3);
    let full = vec![
        PlanItem::new(square(50.0)),
        PlanItem::new(ItemGeom::Open(Vec::new())),
        PlanItem::new(line(0.0)),
        forced,
        PlanItem::new(ItemGeom::Closed(Vec::new())),
        PlanItem::new(square(20.0)),
    ];
    // Index in `full` of each item of `kept`.
    let map = [0usize, 2, 5];
    let kept: Vec<PlanItem> = map.iter().map(|&i| full[i].clone()).collect();
    let fast = PlannerSettings { time_budget_ms: 50, ..Default::default() };
    let all_off = PlannerSettings {
        optimize: true,
        best_start: false,
        best_direction: false,
        reduce_travel: false,
        inner_first: false,
        time_budget_ms: 50,
        ..Default::default()
    };
    for (name, s) in [("default", fast), ("not optimised", PlannerSettings { optimize: false, ..Default::default() }), ("toggles off", all_off)] {
        let got = plan(&full, &s, Point::ZERO, &mut Progress::none()).unwrap();
        let want = plan(&kept, &s, Point::ZERO, &mut Progress::none()).unwrap();
        let got: Vec<(usize, usize, bool)> = got.iter().map(|p| (p.item, p.start, p.reversed)).collect();
        let want: Vec<(usize, usize, bool)> = want.iter().map(|p| (map[p.item], p.start, p.reversed)).collect();
        assert_eq!(got, want, "{name}");
    }
}
