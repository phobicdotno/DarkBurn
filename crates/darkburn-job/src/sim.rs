//! Time estimate and simulation with a trapezoidal motion model.
//!
//! Consecutive moves form a chain. Each junction between two moves gets a speed limit from the
//! angle between them (junction deviation model, as the controllers' documentation describes),
//! then a backward and a forward pass make every move reachable with the given acceleration.
//! Dwells, Z moves and the start and end of the job stop the head.

use std::collections::BTreeMap;

use kurbo::{Point, Vec2};
use serde::{Deserialize, Serialize};

use crate::geom::{self, EPS};
use crate::ir::{Op, Toolpath};

/// Motion limits from the device settings (or read from the controller).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct MotionSettings {
    /// Cutting acceleration, mm/s².
    pub accel: f64,
    /// Acceleration for fill and image layers, mm/s².
    pub engrave_accel: f64,
    /// Rapid (G0) speed, mm/s.
    pub rapid_speed: f64,
    /// Highest cut speed the machine can do, mm/s.
    pub max_speed: f64,
    /// Junction deviation, mm.
    pub junction_deviation: f64,
    /// Z speed, mm/s.
    pub z_speed: f64,
}

impl Default for MotionSettings {
    fn default() -> Self {
        Self { accel: 1000.0, engrave_accel: 2000.0, rapid_speed: 100.0, max_speed: 500.0, junction_deviation: 0.01, z_speed: 10.0 }
    }
}

/// Shape of one timed segment.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum SegmentPath {
    Line { from: Point, to: Point },
    Arc { from: Point, center: Point, sweep: f64 },
    Still { at: Point },
}

/// A timed piece of the job.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SimSegment {
    pub op_index: usize,
    pub t0: f64,
    pub duration: f64,
    pub path: SegmentPath,
    pub length: f64,
    pub v0: f64,
    pub v1: f64,
    pub v_peak: f64,
    pub accel: f64,
    /// Percent; 0 for travel.
    pub power: f64,
    pub rapid: bool,
    /// Index into `Toolpath::layers`, if inside a layer.
    pub layer: Option<usize>,
    pub pass: u32,
}

/// State of the machine at a moment.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SimState {
    pub t: f64,
    pub pos: Point,
    pub power: f64,
    pub layer: Option<usize>,
    pub pass: u32,
    pub op_index: usize,
    pub rapid: bool,
}

/// Time per layer and pass.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct LayerTime {
    pub layer: usize,
    pub pass: u32,
    pub time: f64,
}

/// Result of a simulation.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Simulation {
    /// Seconds.
    pub total_time: f64,
    /// mm with the laser on (or at cut speed with the laser off for tabs, gaps and overscan
    /// counted separately in `scan_distance`).
    pub cut_distance: f64,
    /// Laser-off moves at scan speed (overscan, gaps, tabs), mm.
    pub scan_distance: f64,
    pub rapid_distance: f64,
    pub per_layer: Vec<LayerTime>,
    pub segments: Vec<SimSegment>,
}

struct Pending {
    op_index: usize,
    path: SegmentPath,
    length: f64,
    dir_in: Vec2,
    dir_out: Vec2,
    v_max: f64,
    accel: f64,
    power: f64,
    rapid: bool,
    layer: Option<usize>,
    pass: u32,
}

/// Simulate a toolpath.
pub fn simulate(tp: &Toolpath, m: &MotionSettings) -> Simulation {
    let mut sim = Simulation::default();
    let mut chain: Vec<Pending> = Vec::new();
    let mut pos = tp.start;
    let mut layer: Option<usize> = None;
    let mut pass = 0u32;
    let mut z: Option<f64> = None;
    let mut t = 0.0;
    let mut per: BTreeMap<(usize, u32), f64> = BTreeMap::new();
    let accel_for = |layer: Option<usize>, rapid: bool| {
        let raster = layer.and_then(|l| tp.layers.get(l)).is_some_and(|l| l.raster);
        if raster && !rapid { m.engrave_accel } else { m.accel }
    };
    for (i, op) in tp.ops.iter().enumerate() {
        match op {
            Op::LayerStart { layer: l } => {
                layer = Some(*l);
                pass = 1;
            }
            Op::PassStart { pass: p } => pass = *p,
            Op::Rapid { to } | Op::Cut { to, .. } | Op::Overscan { to, .. } => {
                let (v, power, rapid) = match op {
                    Op::Rapid { .. } => (m.rapid_speed, 0.0, true),
                    Op::Cut { speed, power, .. } => (speed.min(m.max_speed), *power, false),
                    Op::Overscan { speed, .. } => (speed.min(m.max_speed), 0.0, false),
                    _ => unreachable!(),
                };
                let len = pos.distance(*to);
                if len > EPS {
                    let d = geom::unit(*to - pos);
                    chain.push(Pending {
                        op_index: i,
                        path: SegmentPath::Line { from: pos, to: *to },
                        length: len,
                        dir_in: d,
                        dir_out: d,
                        v_max: v.max(EPS),
                        accel: accel_for(layer, rapid),
                        power,
                        rapid,
                        layer,
                        pass,
                    });
                    if rapid {
                        sim.rapid_distance += len;
                    } else if power > 0.0 {
                        sim.cut_distance += len;
                    } else {
                        sim.scan_distance += len;
                    }
                }
                pos = *to;
            }
            Op::Arc { to, center, clockwise, speed, power } => {
                let r = pos.distance(*center);
                let a0 = (pos - *center).atan2();
                let a1 = (*to - *center).atan2();
                let mut sweep = a1 - a0;
                if *clockwise {
                    if sweep >= -EPS {
                        sweep -= std::f64::consts::TAU;
                    }
                } else if sweep <= EPS {
                    sweep += std::f64::consts::TAU;
                }
                if pos.distance(*to) < EPS && r < EPS {
                    continue;
                }
                let len = r * sweep.abs();
                let tangent = |p: Point| {
                    let v = p - *center;
                    let tn = if *clockwise { Vec2::new(v.y, -v.x) } else { Vec2::new(-v.y, v.x) };
                    geom::unit(tn)
                };
                chain.push(Pending {
                    op_index: i,
                    path: SegmentPath::Arc { from: pos, center: *center, sweep },
                    length: len,
                    dir_in: tangent(pos),
                    dir_out: tangent(*to),
                    v_max: speed.min(m.max_speed).max(EPS),
                    accel: accel_for(layer, false),
                    power: *power,
                    rapid: false,
                    layer,
                    pass,
                });
                if *power > 0.0 {
                    sim.cut_distance += len;
                } else {
                    sim.scan_distance += len;
                }
                pos = *to;
            }
            Op::Dwell { ms, power } => {
                t = flush(&mut chain, m, t, &mut sim, &mut per);
                let d = (ms / 1000.0).max(0.0);
                sim.segments.push(still(i, t, d, pos, *power, layer, pass));
                if let Some(l) = layer {
                    *per.entry((l, pass)).or_default() += d;
                }
                t += d;
            }
            Op::ZMove { z: nz } => {
                t = flush(&mut chain, m, t, &mut sim, &mut per);
                let d = z.map_or(0.0, |z| (nz - z).abs() / m.z_speed.max(EPS));
                z = Some(*nz);
                if d > 0.0 {
                    sim.segments.push(still(i, t, d, pos, 0.0, layer, pass));
                    if let Some(l) = layer {
                        *per.entry((l, pass)).or_default() += d;
                    }
                    t += d;
                }
            }
            _ => {}
        }
    }
    t = flush(&mut chain, m, t, &mut sim, &mut per);
    sim.total_time = t;
    sim.per_layer = per.into_iter().map(|((layer, pass), time)| LayerTime { layer, pass, time }).collect();
    sim
}

fn still(op_index: usize, t0: f64, duration: f64, at: Point, power: f64, layer: Option<usize>, pass: u32) -> SimSegment {
    SimSegment {
        op_index,
        t0,
        duration,
        path: SegmentPath::Still { at },
        length: 0.0,
        v0: 0.0,
        v1: 0.0,
        v_peak: 0.0,
        accel: 0.0,
        power,
        rapid: false,
        layer,
        pass,
    }
}

/// Plan the speeds of a chain that starts and ends at rest; returns the new time.
fn flush(chain: &mut Vec<Pending>, m: &MotionSettings, mut t: f64, sim: &mut Simulation, per: &mut BTreeMap<(usize, u32), f64>) -> f64 {
    let n = chain.len();
    if n == 0 {
        return t;
    }
    // Junction limits (entry speed of segment i).
    let mut entry = vec![0.0f64; n + 1];
    for i in 1..n {
        let a = chain[i - 1].dir_out;
        let b = chain[i].dir_in;
        let cos = a.dot(b).clamp(-1.0, 1.0);
        let acc = chain[i - 1].accel.min(chain[i].accel);
        let lim = if cos > 1.0 - 1e-9 {
            f64::INFINITY
        } else if cos < -1.0 + 1e-9 {
            0.0
        } else {
            let sin_half = ((1.0 + cos) / 2.0).sqrt();
            (acc * m.junction_deviation * sin_half / (1.0 - sin_half)).sqrt()
        };
        entry[i] = lim.min(chain[i - 1].v_max).min(chain[i].v_max);
    }
    // Backward pass.
    for i in (0..n).rev() {
        let reach = (entry[i + 1].powi(2) + 2.0 * chain[i].accel * chain[i].length).sqrt();
        entry[i] = entry[i].min(reach);
    }
    // Forward pass.
    for i in 0..n {
        let reach = (entry[i].powi(2) + 2.0 * chain[i].accel * chain[i].length).sqrt();
        entry[i + 1] = entry[i + 1].min(reach);
    }
    for (i, p) in chain.drain(..).enumerate() {
        let (v0, v1, a, d, vmax) = (entry[i], entry[i + 1], p.accel, p.length, p.v_max);
        let d_acc = (vmax * vmax - v0 * v0) / (2.0 * a);
        let d_dec = (vmax * vmax - v1 * v1) / (2.0 * a);
        let (dur, peak) = if d_acc + d_dec <= d {
            ((vmax - v0) / a + (vmax - v1) / a + (d - d_acc - d_dec) / vmax, vmax)
        } else {
            let vp = ((2.0 * a * d + v0 * v0 + v1 * v1) / 2.0).sqrt().max(v0.max(v1));
            ((vp - v0) / a + (vp - v1) / a, vp)
        };
        sim.segments.push(SimSegment {
            op_index: p.op_index,
            t0: t,
            duration: dur,
            path: p.path,
            length: d,
            v0,
            v1,
            v_peak: peak,
            accel: a,
            power: p.power,
            rapid: p.rapid,
            layer: p.layer,
            pass: p.pass,
        });
        if let Some(l) = p.layer {
            *per.entry((l, p.pass)).or_default() += dur;
        }
        t += dur;
    }
    t
}

impl SimSegment {
    /// Distance travelled `dt` seconds into the segment.
    fn distance_at(&self, dt: f64) -> f64 {
        let a = self.accel;
        if self.length <= 0.0 || a <= 0.0 {
            return 0.0;
        }
        let t_acc = (self.v_peak - self.v0) / a;
        let t_dec = (self.v_peak - self.v1) / a;
        let t_cruise = (self.duration - t_acc - t_dec).max(0.0);
        let dt = dt.clamp(0.0, self.duration);
        let s = if dt <= t_acc {
            self.v0 * dt + 0.5 * a * dt * dt
        } else if dt <= t_acc + t_cruise {
            let s_acc = self.v0 * t_acc + 0.5 * a * t_acc * t_acc;
            s_acc + self.v_peak * (dt - t_acc)
        } else {
            let s_acc = self.v0 * t_acc + 0.5 * a * t_acc * t_acc;
            let s_cr = self.v_peak * t_cruise;
            let u = dt - t_acc - t_cruise;
            s_acc + s_cr + self.v_peak * u - 0.5 * a * u * u
        };
        s.clamp(0.0, self.length)
    }

    /// Position `dt` seconds into the segment.
    pub fn position_at(&self, dt: f64) -> Point {
        let s = self.distance_at(dt);
        match self.path {
            SegmentPath::Still { at } => at,
            SegmentPath::Line { from, to } => {
                if self.length <= 0.0 {
                    to
                } else {
                    from.lerp(to, s / self.length)
                }
            }
            SegmentPath::Arc { from, center, sweep } => {
                let f = if self.length <= 0.0 { 1.0 } else { s / self.length };
                center + geom::rotate(from - center, sweep * f)
            }
        }
    }
}

impl Simulation {
    /// Machine state at time `t` seconds (clamped to the job).
    pub fn state_at(&self, t: f64) -> Option<SimState> {
        if self.segments.is_empty() {
            return None;
        }
        let t = t.clamp(0.0, self.total_time);
        let i = self.segments.partition_point(|s| s.t0 <= t).saturating_sub(1);
        let s = &self.segments[i];
        Some(SimState { t, pos: s.position_at(t - s.t0), power: s.power, layer: s.layer, pass: s.pass, op_index: s.op_index, rapid: s.rapid })
    }

    /// Evenly sampled timeline for the preview slider, `dt` seconds apart (end included).
    pub fn timeline(&self, dt: f64) -> Vec<SimState> {
        let dt = dt.max(1e-3);
        let n = (self.total_time / dt).ceil() as usize;
        (0..=n).filter_map(|k| self.state_at((k as f64 * dt).min(self.total_time))).collect()
    }

    /// Time at which `op_index` starts (for "start here" and the preview readout).
    pub fn time_of_op(&self, op_index: usize) -> Option<f64> {
        self.segments.iter().find(|s| s.op_index >= op_index).map(|s| s.t0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::CoordSpace;

    fn tp(ops: Vec<Op>) -> Toolpath {
        Toolpath { space: CoordSpace::Design, layers: Vec::new(), ops, start: Point::ZERO }
    }

    #[test]
    fn hundred_mm_at_ten_mm_s() {
        let t = tp(vec![Op::Cut { to: Point::new(100.0, 0.0), speed: 10.0, power: 50.0 }]);
        let m = MotionSettings { accel: 100_000.0, ..Default::default() };
        let s = simulate(&t, &m);
        assert!((s.total_time - 10.0).abs() < 0.01, "{}", s.total_time);
        assert!((s.cut_distance - 100.0).abs() < 1e-9);
        // Middle of the line at 5 s.
        let st = s.state_at(5.0).unwrap();
        assert!((st.pos.x - 50.0).abs() < 0.01, "{:?}", st.pos);
        assert_eq!(st.power, 50.0);
    }

    #[test]
    fn low_accel_triangle_profile() {
        // 10 mm at accel 10 never reaches 1000 mm/s: t = 2*sqrt(d/a) = 2 s.
        let t = tp(vec![Op::Cut { to: Point::new(10.0, 0.0), speed: 1000.0, power: 50.0 }]);
        let m = MotionSettings { accel: 10.0, max_speed: 1000.0, ..Default::default() };
        let s = simulate(&t, &m);
        assert!((s.total_time - 2.0).abs() < 1e-6, "{}", s.total_time);
    }

    #[test]
    fn collinear_moves_do_not_stop_but_corners_do() {
        let m = MotionSettings { accel: 100.0, ..Default::default() };
        let straight =
            tp(vec![Op::Cut { to: Point::new(50.0, 0.0), speed: 50.0, power: 1.0 }, Op::Cut { to: Point::new(100.0, 0.0), speed: 50.0, power: 1.0 }]);
        let single = tp(vec![Op::Cut { to: Point::new(100.0, 0.0), speed: 50.0, power: 1.0 }]);
        let a = simulate(&straight, &m).total_time;
        let b = simulate(&single, &m).total_time;
        assert!((a - b).abs() < 1e-9);
        let corner =
            tp(vec![Op::Cut { to: Point::new(50.0, 0.0), speed: 50.0, power: 1.0 }, Op::Cut { to: Point::new(50.0, 50.0), speed: 50.0, power: 1.0 }]);
        assert!(simulate(&corner, &m).total_time > b);
    }

    #[test]
    fn dwell_rapid_and_timeline() {
        let t = tp(vec![
            Op::Rapid { to: Point::new(10.0, 0.0) },
            Op::Dwell { ms: 500.0, power: 10.0 },
            Op::Arc { to: Point::new(10.0, 0.0), center: Point::new(15.0, 0.0), clockwise: false, speed: 10.0, power: 5.0 },
        ]);
        let m = MotionSettings { accel: 1e7, rapid_speed: 100.0, ..Default::default() };
        let s = simulate(&t, &m);
        let arc = std::f64::consts::TAU * 5.0;
        assert!((s.total_time - (0.1 + 0.5 + arc / 10.0)).abs() < 0.01, "{}", s.total_time);
        assert!((s.rapid_distance - 10.0).abs() < 1e-9);
        assert!((s.cut_distance - arc).abs() < 1e-9);
        let tl = s.timeline(0.05);
        assert!(tl.iter().any(|st| st.op_index == 1 && st.power == 10.0));
        let half = s.state_at(0.6 + arc / 20.0).unwrap();
        assert!(half.pos.distance(Point::new(20.0, 0.0)) < 0.05, "{:?}", half.pos);
        assert_eq!(s.time_of_op(2).map(|t| (t * 100.0).round()), Some(60.0));
    }
}
