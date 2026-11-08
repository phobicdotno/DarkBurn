//! `.rd` job files: a neutral job description, a writer and a reader.
//!
//! A `.rd` file has no header or magic: it is the swizzled command stream itself. The same
//! bytes are what gets uploaded over UDP or USB. Coordinates in [`RdJob`] are millimetres in
//! the controller's own frame (the caller does any origin or mirroring transform).
//!
//! Layout written here (following the public descriptions of RDWorks/MeerK40t output):
//! header (`D8 12`, `E6 01`, `F0`, `F1 02`, `D8 00`, bounds, repeat), a per-layer table
//! (`C9 04`, `C6 60`, `C6 31/32/41/42`, `CA 06`, `CA 41`, `E7 52/53/61/62`), `CA 22`, offsets,
//! `EA 00`, then for each layer a settings block (`CA 02`, `CA 01 10`, `CA 01 12/13`, `C9 02`,
//! `C6 01/02/21/22`, dot settings, `CA 03`) followed by motion, and `E7 00 CA 01 00 CA 01 30`.
//! The trailer is `EB E7 00 E5 05 <sum> D7`.

use std::collections::BTreeMap;

use kurbo::Point;

use crate::command::{Axis, Command, DelayKind, Laser, LayerFlag, RapidOption, parse_commands};
use crate::encoding::{Power, REL_MAX, REL_MIN, bgr_to_rgb, mm_s_to_um_s, mm_to_um, ms_to_us, rgb_to_bgr, um_s_to_mm_s, um_to_mm, us_to_ms};
use crate::error::{Result, RuidaError};
use crate::swizzle::{Magic, Swizzler, detect_magic_histogram};

/// Dot mode for a layer. UNCERTAIN encoding: dot time is written as `C6 11` (add delay, µs)
/// and dot spacing as `C6 10` (laser interval, here in µm). Needs checking on a controller.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DotMode {
    pub dot_time_ms: f64,
    pub spacing_mm: f64,
}

/// Cut settings for one layer.
#[derive(Debug, Clone, PartialEq)]
pub struct LayerSettings {
    /// Layer number in the file (0-based, < 128).
    pub index: u8,
    pub speed_mm_s: f64,
    /// Minimum (corner) power in percent for laser 1 and laser 2.
    pub min_power: [f64; 2],
    /// Maximum power in percent for laser 1 and laser 2.
    pub max_power: [f64; 2],
    /// Display colour 0xRRGGBB.
    pub color: u32,
    /// PWM frequency override.
    pub frequency_hz: Option<u32>,
    pub air_assist: bool,
    pub dot_mode: Option<DotMode>,
}

impl Default for LayerSettings {
    fn default() -> Self {
        LayerSettings {
            index: 0,
            speed_mm_s: 100.0,
            min_power: [10.0, 10.0],
            max_power: [30.0, 30.0],
            color: 0x000000,
            frequency_hz: None,
            air_assist: true,
            dot_mode: None,
        }
    }
}

/// One operation in a layer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RdOp {
    /// Travel with the laser off.
    Move(Point),
    /// Cut to a point. `power` (percent, laser 1) and `speed` (mm/s) override the layer and
    /// stay in effect for later cuts until changed again, as on the controller.
    Cut { to: Point, power: Option<f64>, speed: Option<f64> },
    /// Move Z to an absolute height (mm).
    Z(f64),
    /// Move the U axis to an absolute position (mm), for example a rotary or a U offset.
    U(f64),
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct RdLayer {
    pub settings: LayerSettings,
    pub ops: Vec<RdOp>,
}

/// A whole job.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RdJob {
    pub layers: Vec<RdLayer>,
}

fn q_mm(v: f64) -> f64 {
    um_to_mm(mm_to_um(v))
}
fn q_pct(v: f64) -> f64 {
    Power::from_percent(v).percent()
}
fn q_speed(v: f64) -> f64 {
    um_s_to_mm_s(mm_s_to_um_s(v))
}
fn q_point(p: Point) -> Point {
    Point::new(q_mm(p.x), q_mm(p.y))
}

impl RdJob {
    /// The job as it reads back after a write: every value rounded to wire precision.
    pub fn quantized(&self) -> RdJob {
        RdJob {
            layers: self
                .layers
                .iter()
                .map(|l| {
                    let s = &l.settings;
                    RdLayer {
                        settings: LayerSettings {
                            index: s.index,
                            speed_mm_s: q_speed(s.speed_mm_s),
                            min_power: s.min_power.map(q_pct),
                            max_power: s.max_power.map(q_pct),
                            color: s.color & 0xFF_FFFF,
                            frequency_hz: s.frequency_hz,
                            air_assist: s.air_assist,
                            dot_mode: s.dot_mode.map(|d| DotMode { dot_time_ms: q_mm(d.dot_time_ms), spacing_mm: q_mm(d.spacing_mm) }),
                        },
                        ops: l
                            .ops
                            .iter()
                            .map(|op| match *op {
                                RdOp::Move(p) => RdOp::Move(q_point(p)),
                                RdOp::Cut { to, power, speed } => RdOp::Cut { to: q_point(to), power: power.map(q_pct), speed: speed.map(q_speed) },
                                RdOp::Z(z) => RdOp::Z(q_mm(z)),
                                RdOp::U(u) => RdOp::U(q_mm(u)),
                            })
                            .collect(),
                    }
                })
                .collect(),
        }
    }

    /// Bounds of all Move and Cut points in µm: (min_x, min_y, max_x, max_y).
    fn bounds_um(ops: impl Iterator<Item = RdOp>) -> (i32, i32, i32, i32) {
        let mut b: Option<(i32, i32, i32, i32)> = None;
        for op in ops {
            let p = match op {
                RdOp::Move(p) | RdOp::Cut { to: p, .. } => p,
                _ => continue,
            };
            let (x, y) = (mm_to_um(p.x), mm_to_um(p.y));
            b = Some(match b {
                None => (x, y, x, y),
                Some((a, c, d, e)) => (a.min(x), c.min(y), d.max(x), e.max(y)),
            });
        }
        b.unwrap_or((0, 0, 0, 0))
    }

    /// Bounds of the whole job in mm.
    pub fn bounds_mm(&self) -> kurbo::Rect {
        let (a, b, c, d) = Self::bounds_um(self.layers.iter().flat_map(|l| l.ops.iter().copied()));
        kurbo::Rect::new(um_to_mm(a), um_to_mm(b), um_to_mm(c), um_to_mm(d))
    }

    /// The command list for this job.
    pub fn to_commands(&self) -> Result<Vec<Command>> {
        use Command::*;
        for l in &self.layers {
            if l.settings.index >= 0x80 {
                return std::result::Result::Err(RuidaError::Encode(format!("layer index {} must be < 128", l.settings.index)));
            }
        }
        let mut c = Vec::new();
        let (min_x, min_y, max_x, max_y) = Self::bounds_um(self.layers.iter().flat_map(|l| l.ops.iter().copied()));
        c.extend([
            RefPointMode(0x12),
            SetAbsolute,
            RefPointSet,
            EnableBlockCutting(0),
            Start,
            FeedRepeat(0, 0),
            ProcessTopLeft { x: min_x, y: min_y },
            ProcessBottomRight { x: max_x, y: max_y },
            DocumentMin { x: min_x, y: min_y },
            DocumentMax { x: max_x, y: max_y },
            ProcessRepeat([1, 1, 0, 0, 0, 0, 0]),
            ArrayDirection(0),
        ]);
        // Layer table.
        for l in &self.layers {
            let s = &l.settings;
            let part = s.index;
            let (a, b, cc, d) = Self::bounds_um(l.ops.iter().copied());
            c.push(SpeedPart { part, speed: mm_s_to_um_s(s.speed_mm_s) });
            if let Some(hz) = s.frequency_hz {
                c.push(FrequencyPart { laser: 0, part, hz });
            }
            c.push(MinPowerPart { laser: Laser::L1, part, power: Power::from_percent(s.min_power[0]) });
            c.push(MaxPowerPart { laser: Laser::L1, part, power: Power::from_percent(s.max_power[0]) });
            c.push(MinPowerPart { laser: Laser::L2, part, power: Power::from_percent(s.min_power[1]) });
            c.push(MaxPowerPart { laser: Laser::L2, part, power: Power::from_percent(s.max_power[1]) });
            c.push(LayerColorPart { part, color: rgb_to_bgr(s.color & 0xFF_FFFF) });
            c.push(WorkModePart { part, mode: 0 });
            c.push(PartMin { part, x: a, y: b });
            c.push(PartMax { part, x: cc, y: d });
            c.push(PartMinEx { part, x: a, y: b });
            c.push(PartMaxEx { part, x: cc, y: d });
        }
        c.push(MaxLayerPart(self.layers.iter().map(|l| l.settings.index).max().unwrap_or(0)));
        c.extend([
            PenOffset { axis: 0, value: 0 },
            PenOffset { axis: 1, value: 0 },
            LayerOffset { axis: 0, value: 0 },
            LayerOffset { axis: 1, value: 0 },
            DisplayOffset { x: 0, y: 0 },
            ArrayStart(0),
            CurrentElementIndex(0),
        ]);
        // Body.
        let mut cur: Option<(i32, i32)> = None;
        for l in &self.layers {
            let s = &l.settings;
            c.push(LayerNumber(s.index));
            c.push(Flag(LayerFlag::LaserDevice(0)));
            c.push(Flag(LayerFlag::AirAssist(s.air_assist)));
            c.push(Speed(mm_s_to_um_s(s.speed_mm_s)));
            for (i, laser) in [Laser::L1, Laser::L2].into_iter().enumerate() {
                c.push(MinPower { laser, power: Power::from_percent(s.min_power[i]) });
                c.push(MaxPower { laser, power: Power::from_percent(s.max_power[i]) });
            }
            if let Some(d) = s.dot_mode {
                c.push(Delay { kind: DelayKind::AddDelay, us: ms_to_us(d.dot_time_ms) });
                c.push(Delay { kind: DelayKind::LaserInterval, us: mm_to_um(d.spacing_mm).max(0) as u32 });
            }
            c.push(LaserTubeStart(1));
            for op in &l.ops {
                match *op {
                    RdOp::Move(p) => {
                        let t = (mm_to_um(p.x), mm_to_um(p.y));
                        c.push(motion(cur, t, false));
                        cur = Some(t);
                    }
                    RdOp::Cut { to, power, speed } => {
                        if let Some(sp) = speed {
                            c.push(Speed(mm_s_to_um_s(sp)));
                        }
                        if let Some(pw) = power {
                            c.push(MinPower { laser: Laser::L1, power: Power::from_percent(pw) });
                            c.push(MaxPower { laser: Laser::L1, power: Power::from_percent(pw) });
                        }
                        let t = (mm_to_um(to.x), mm_to_um(to.y));
                        c.push(motion(cur, t, true));
                        cur = Some(t);
                    }
                    RdOp::Z(z) => c.push(AxisMove { axis: Axis::Z, pos: mm_to_um(z) }),
                    RdOp::U(u) => c.push(AxisMove { axis: Axis::U, pos: mm_to_um(u) }),
                }
            }
            c.extend([BlockEnd, Flag(LayerFlag::LayerEnd), Flag(LayerFlag::Laser2Offset(false))]);
        }
        c.extend([ArrayEnd, BlockEnd]);
        let sum = file_sum(&c)?;
        c.push(FileSum(sum.wrapping_add(0xD7)));
        c.push(EndOfFile);
        Ok(c)
    }

    /// Logical (unswizzled) bytes.
    pub fn to_logical(&self) -> Result<Vec<u8>> {
        crate::command::encode_commands(&self.to_commands()?)
    }

    /// A ready-to-save `.rd` file.
    pub fn to_rd_file(&self, magic: Magic) -> Result<Vec<u8>> {
        Ok(Swizzler::new(magic).swizzle(&self.to_logical()?))
    }

    /// Read a job back from commands.
    pub fn from_commands(cmds: &[Command]) -> Result<RdJob> {
        read_job(cmds)
    }

    /// Read a job from logical bytes.
    pub fn from_logical(data: &[u8]) -> Result<RdJob> {
        read_job(&parse_commands(data))
    }

    /// Read a `.rd` file. With `magic = None` the magic is guessed from the byte histogram.
    pub fn from_rd_file(data: &[u8], magic: Option<Magic>) -> Result<RdJob> {
        let magic = match magic {
            Some(m) => m,
            None => detect_magic_histogram(data).ok_or_else(|| RuidaError::Decode("empty .rd file".into()))?,
        };
        Self::from_logical(&Swizzler::new(magic).unswizzle(data))
    }
}

/// Byte sum of the logical encoding of `cmds` (what `E5 05` carries, before the EOF byte).
pub fn file_sum(cmds: &[Command]) -> Result<u32> {
    let mut sum = 0u32;
    let mut buf = Vec::new();
    for c in cmds {
        buf.clear();
        c.encode_into(&mut buf)?;
        sum = buf.iter().fold(sum, |a, &b| a.wrapping_add(b as u32));
    }
    Ok(sum)
}

fn motion(cur: Option<(i32, i32)>, to: (i32, i32), cut: bool) -> Command {
    use Command::*;
    let fits = |v: i32| (REL_MIN..=REL_MAX).contains(&v);
    match cur {
        Some((cx, cy)) if fits(to.0 - cx) && fits(to.1 - cy) => {
            let (dx, dy) = (to.0 - cx, to.1 - cy);
            match (dx, dy, cut) {
                (0, 0, false) => MoveRel { dx, dy },
                (0, 0, true) => CutRel { dx, dy },
                (0, _, false) => MoveRelY(dy),
                (0, _, true) => CutRelY(dy),
                (_, 0, false) => MoveRelX(dx),
                (_, 0, true) => CutRelX(dx),
                (_, _, false) => MoveRel { dx, dy },
                (_, _, true) => CutRel { dx, dy },
            }
        }
        _ => {
            if cut {
                CutAbs { x: to.0, y: to.1 }
            } else {
                MoveAbs { x: to.0, y: to.1 }
            }
        }
    }
}

#[derive(PartialEq)]
enum Phase {
    Header,
    Settings,
    Body,
}

fn read_job(cmds: &[Command]) -> Result<RdJob> {
    use Command::*;
    let mut table: BTreeMap<u8, LayerSettings> = BTreeMap::new();
    let mut layers: Vec<RdLayer> = Vec::new();
    let mut current: Option<RdLayer> = None;
    let mut phase = Phase::Header;
    let mut pos = (0i32, 0i32);
    let mut pending_speed: Option<f64> = None;
    let mut pending_power: Option<f64> = None;
    let mut dot_time: Option<u32> = None;
    let mut dot_spacing: Option<u32> = None;
    let mut saw_eof = false;

    fn entry(t: &mut BTreeMap<u8, LayerSettings>, part: u8) -> &mut LayerSettings {
        t.entry(part).or_insert_with(|| LayerSettings { index: part, ..Default::default() })
    }

    for cmd in cmds {
        match cmd {
            SpeedPart { part, speed } => entry(&mut table, *part).speed_mm_s = um_s_to_mm_s(*speed),
            FrequencyPart { part, hz, .. } => entry(&mut table, *part).frequency_hz = Some(*hz),
            MinPowerPart { laser, part, power } if matches!(laser, Laser::L1 | Laser::L2) => {
                entry(&mut table, *part).min_power[laser.number() as usize - 1] = power.percent()
            }
            MaxPowerPart { laser, part, power } if matches!(laser, Laser::L1 | Laser::L2) => {
                entry(&mut table, *part).max_power[laser.number() as usize - 1] = power.percent()
            }
            LayerColorPart { part, color } => entry(&mut table, *part).color = bgr_to_rgb(*color),
            LayerNumber(part) => {
                if let Some(l) = current.take() {
                    layers.push(l);
                }
                let settings = table.get(part).cloned().unwrap_or(LayerSettings { index: *part, ..Default::default() });
                current = Some(RdLayer { settings, ops: Vec::new() });
                phase = Phase::Settings;
                pending_speed = None;
                pending_power = None;
                dot_time = None;
                dot_spacing = None;
            }
            _ if phase == Phase::Settings => {
                let l = current.as_mut().expect("settings phase has a layer");
                let s = &mut l.settings;
                match cmd {
                    Speed(v) => s.speed_mm_s = um_s_to_mm_s(*v),
                    MinPower { laser: laser @ (Laser::L1 | Laser::L2), power } => s.min_power[laser.number() as usize - 1] = power.percent(),
                    MaxPower { laser: laser @ (Laser::L1 | Laser::L2), power } => s.max_power[laser.number() as usize - 1] = power.percent(),
                    Flag(LayerFlag::AirAssist(on)) => s.air_assist = *on,
                    Delay { kind: DelayKind::AddDelay, us } => dot_time = Some(*us),
                    Delay { kind: DelayKind::LaserInterval, us } => dot_spacing = Some(*us),
                    LaserTubeStart(_) => {
                        if let (Some(t), Some(sp)) = (dot_time, dot_spacing) {
                            s.dot_mode = Some(DotMode { dot_time_ms: us_to_ms(t), spacing_mm: um_to_mm(sp as i32) });
                        }
                        phase = Phase::Body;
                    }
                    c if c.is_motion() => {
                        // A file without CA 03: treat the first motion as the start of the body.
                        phase = Phase::Body;
                        body_motion(c, l, &mut pos, &mut pending_speed, &mut pending_power);
                    }
                    _ => {}
                }
            }
            _ if phase == Phase::Body => {
                let l = current.as_mut().expect("body phase has a layer");
                match cmd {
                    Speed(v) => pending_speed = Some(um_s_to_mm_s(*v)),
                    MaxPower { laser: Laser::L1, power } => pending_power = Some(power.percent()),
                    Flag(LayerFlag::LayerEnd) => {
                        layers.push(current.take().expect("layer"));
                        phase = Phase::Header;
                    }
                    c => body_motion(c, l, &mut pos, &mut pending_speed, &mut pending_power),
                }
            }
            EndOfFile => saw_eof = true,
            _ => {}
        }
    }
    if let Some(l) = current.take() {
        layers.push(l);
    }
    if !saw_eof && layers.is_empty() {
        return std::result::Result::Err(RuidaError::Decode("no layers and no end-of-file marker".into()));
    }
    Ok(RdJob { layers })
}

fn body_motion(c: &Command, l: &mut RdLayer, pos: &mut (i32, i32), speed: &mut Option<f64>, power: &mut Option<f64>) {
    use Command::*;
    let pt = |p: (i32, i32)| Point::new(um_to_mm(p.0), um_to_mm(p.1));
    let (target, cut) = match *c {
        MoveAbs { x, y } => ((x, y), false),
        MoveRel { dx, dy } => ((pos.0 + dx, pos.1 + dy), false),
        MoveRelX(dx) => ((pos.0 + dx, pos.1), false),
        MoveRelY(dy) => ((pos.0, pos.1 + dy), false),
        CutAbs { x, y } => ((x, y), true),
        CutRel { dx, dy } => ((pos.0 + dx, pos.1 + dy), true),
        CutRelX(dx) => ((pos.0 + dx, pos.1), true),
        CutRelY(dy) => ((pos.0, pos.1 + dy), true),
        RapidXY { option: RapidOption::None | RapidOption::Light, x, y } => ((x, y), false),
        AxisMove { axis: Axis::Z, pos: z } => {
            l.ops.push(RdOp::Z(um_to_mm(z)));
            return;
        }
        AxisMove { axis: Axis::U, pos: u } => {
            l.ops.push(RdOp::U(um_to_mm(u)));
            return;
        }
        _ => return,
    };
    *pos = target;
    if cut {
        l.ops.push(RdOp::Cut { to: pt(target), power: power.take(), speed: speed.take() });
    } else {
        l.ops.push(RdOp::Move(pt(target)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn sample_job() -> RdJob {
        RdJob {
            layers: vec![
                RdLayer {
                    settings: LayerSettings {
                        index: 0,
                        speed_mm_s: 20.0,
                        min_power: [15.0, 12.5],
                        max_power: [60.0, 55.0],
                        color: 0xFF0000,
                        frequency_hz: Some(20_000),
                        air_assist: true,
                        dot_mode: None,
                    },
                    ops: vec![
                        RdOp::Move(Point::new(10.0, 10.0)),
                        RdOp::Cut { to: Point::new(50.0, 10.0), power: None, speed: None },
                        RdOp::Cut { to: Point::new(50.0, 40.0), power: Some(40.0), speed: None },
                        RdOp::Cut { to: Point::new(10.0, 40.0), power: None, speed: Some(15.5) },
                        RdOp::Cut { to: Point::new(10.0, 10.0), power: None, speed: None },
                        RdOp::Cut { to: Point::new(300.0, 200.123), power: None, speed: None },
                    ],
                },
                RdLayer {
                    settings: LayerSettings {
                        index: 1,
                        speed_mm_s: 300.0,
                        min_power: [5.0, 0.0],
                        max_power: [20.0, 0.0],
                        color: 0x0000FF,
                        frequency_hz: None,
                        air_assist: false,
                        dot_mode: Some(DotMode { dot_time_ms: 0.5, spacing_mm: 0.2 }),
                    },
                    ops: vec![
                        RdOp::Z(-2.5),
                        RdOp::U(12.0),
                        RdOp::Move(Point::new(100.0, 100.0)),
                        RdOp::Cut { to: Point::new(101.234, 99.001), power: None, speed: None },
                        RdOp::Move(Point::new(5.0, 5.0)),
                    ],
                },
            ],
        }
    }

    #[test]
    fn round_trip_through_commands_bytes_and_file() {
        let job = sample_job();
        let want = job.quantized();
        assert_eq!(RdJob::from_commands(&job.to_commands().unwrap()).unwrap(), want);
        assert_eq!(RdJob::from_logical(&job.to_logical().unwrap()).unwrap(), want);
        for magic in [Magic(0x88), Magic(0x11), Magic(0x38)] {
            let file = job.to_rd_file(magic).unwrap();
            assert_eq!(RdJob::from_rd_file(&file, Some(magic)).unwrap(), want);
            assert_eq!(RdJob::from_rd_file(&file, None).unwrap(), want, "magic auto-detect {magic:?}");
        }
    }

    #[test]
    fn relative_and_absolute_moves_chosen() {
        let cmds = sample_job().to_commands().unwrap();
        assert!(cmds.contains(&Command::MoveAbs { x: 10_000, y: 10_000 }));
        // 40 mm in X does not fit a relative move.
        assert!(cmds.contains(&Command::CutAbs { x: 50_000, y: 10_000 }));
        assert!(cmds.contains(&Command::CutRelX(1_234)) || cmds.iter().any(|c| matches!(c, Command::CutRel { dx: 1234, dy: -999 })));
        assert!(cmds.contains(&Command::AxisMove { axis: Axis::Z, pos: -2500 }));
    }

    #[test]
    fn file_sum_matches_bytes() {
        let cmds = sample_job().to_commands().unwrap();
        let n = cmds.len();
        let Command::FileSum(sum) = cmds[n - 2] else { panic!("file sum before EOF") };
        let bytes = crate::command::encode_commands(&cmds[..n - 2]).unwrap();
        let s: u32 = bytes.iter().map(|&b| b as u32).sum();
        assert_eq!(sum, s + 0xD7);
    }

    #[test]
    fn bounds() {
        let r = sample_job().bounds_mm();
        assert_eq!((r.x0, r.y0, r.x1, r.y1), (5.0, 5.0, 300.0, 200.123));
    }

    #[test]
    fn bad_layer_index_rejected() {
        let mut job = sample_job();
        job.layers[0].settings.index = 200;
        assert!(job.to_commands().is_err());
    }

    #[test]
    fn garbage_is_rejected() {
        assert!(RdJob::from_logical(&[]).is_err());
        assert!(RdJob::from_rd_file(&[], None).is_err());
    }
}
