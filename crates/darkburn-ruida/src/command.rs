//! Typed Ruida commands (logical, unswizzled bytes).
//!
//! The opcode table follows the public documentation listed in `ATTRIBUTION.md` (MeerK40t's
//! `rdjob.py` constants, MIT; EduTechWiki; Schuermans). Opcodes whose meaning is not known well
//! enough to type are kept as [`Command::Raw`], which round-trips byte for byte.

use crate::encoding::*;
use crate::error::{Result, RuidaError};

/// One of up to four laser sources.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Laser {
    L1,
    L2,
    L3,
    L4,
}

impl Laser {
    pub const ALL: [Laser; 4] = [Laser::L1, Laser::L2, Laser::L3, Laser::L4];

    /// 1-based laser number.
    pub fn number(self) -> u8 {
        match self {
            Laser::L1 => 1,
            Laser::L2 => 2,
            Laser::L3 => 3,
            Laser::L4 => 4,
        }
    }

    pub fn from_number(n: u8) -> Option<Laser> {
        match n {
            1 => Some(Laser::L1),
            2 => Some(Laser::L2),
            3 => Some(Laser::L3),
            4 => Some(Laser::L4),
            _ => None,
        }
    }

    fn imd_op(self) -> u8 {
        [0xC7, 0xC0, 0xC2, 0xC3][self.number() as usize - 1]
    }
    fn end_op(self) -> u8 {
        [0xC8, 0xC1, 0xC4, 0xC5][self.number() as usize - 1]
    }
    fn min_sub(self) -> u8 {
        [0x01, 0x21, 0x05, 0x07][self.number() as usize - 1]
    }
    fn max_sub(self) -> u8 {
        [0x02, 0x22, 0x06, 0x08][self.number() as usize - 1]
    }
    fn min_part_sub(self) -> u8 {
        [0x31, 0x41, 0x35, 0x37][self.number() as usize - 1]
    }
    fn max_part_sub(self) -> u8 {
        [0x32, 0x42, 0x36, 0x38][self.number() as usize - 1]
    }
    fn through_sub(self) -> u8 {
        [0x50, 0x51, 0x55, 0x56][self.number() as usize - 1]
    }
    fn find(f: impl Fn(Laser) -> u8, b: u8) -> Option<Laser> {
        Laser::ALL.into_iter().find(|&l| f(l) == b)
    }
}

/// Machine axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Axis {
    X,
    Y,
    Z,
    U,
}

impl Axis {
    pub const ALL: [Axis; 4] = [Axis::X, Axis::Y, Axis::Z, Axis::U];
    fn index(self) -> u8 {
        match self {
            Axis::X => 0,
            Axis::Y => 1,
            Axis::Z => 2,
            Axis::U => 3,
        }
    }
    fn from_index(i: u8) -> Option<Axis> {
        Axis::ALL.get(i as usize).copied()
    }
}

/// Keys for continuous jog on the data channel (`D8 2x` key down, `D8 3x` key up).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]
pub enum JogKey {
    XLeft,
    XRight,
    YTop,
    YBottom,
    ZUp,
    ZDown,
    UForward,
    UBackward,
}

impl JogKey {
    pub const ALL: [JogKey; 8] =
        [JogKey::XLeft, JogKey::XRight, JogKey::YTop, JogKey::YBottom, JogKey::ZUp, JogKey::ZDown, JogKey::UForward, JogKey::UBackward];

    fn offset(self) -> u8 {
        JogKey::ALL.iter().position(|&k| k == self).unwrap_or(0) as u8
    }

    /// Axis and direction (+1 or -1) in machine coordinates.
    ///
    /// UNCERTAIN: "left/top" are panel labels; which way they move depends on the machine's
    /// origin corner. This mapping assumes X left = -X and Y top = +Y.
    pub fn axis_dir(self) -> (Axis, i32) {
        match self {
            JogKey::XLeft => (Axis::X, -1),
            JogKey::XRight => (Axis::X, 1),
            JogKey::YTop => (Axis::Y, 1),
            JogKey::YBottom => (Axis::Y, -1),
            JogKey::ZUp => (Axis::Z, 1),
            JogKey::ZDown => (Axis::Z, -1),
            JogKey::UForward => (Axis::U, 1),
            JogKey::UBackward => (Axis::U, -1),
        }
    }

    pub fn for_axis(axis: Axis, positive: bool) -> JogKey {
        match (axis, positive) {
            (Axis::X, false) => JogKey::XLeft,
            (Axis::X, true) => JogKey::XRight,
            (Axis::Y, true) => JogKey::YTop,
            (Axis::Y, false) => JogKey::YBottom,
            (Axis::Z, true) => JogKey::ZUp,
            (Axis::Z, false) => JogKey::ZDown,
            (Axis::U, true) => JogKey::UForward,
            (Axis::U, false) => JogKey::UBackward,
        }
    }
}

/// Option byte for rapid (`D9`) moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum RapidOption {
    /// 0x00: coordinates relative to the job origin.
    Origin,
    /// 0x01: origin, red light on.
    LightOrigin,
    /// 0x02: plain move.
    None,
    /// 0x03: red light on.
    Light,
    Other(u8),
}

impl RapidOption {
    fn byte(self) -> u8 {
        match self {
            RapidOption::Origin => 0,
            RapidOption::LightOrigin => 1,
            RapidOption::None => 2,
            RapidOption::Light => 3,
            RapidOption::Other(b) => b,
        }
    }
    fn from_byte(b: u8) -> Self {
        match b {
            0 => RapidOption::Origin,
            1 => RapidOption::LightOrigin,
            2 => RapidOption::None,
            3 => RapidOption::Light,
            b => RapidOption::Other(b),
        }
    }
}

/// `C6 1x` timing values (all µs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum DelayKind {
    /// `C6 10`. Used here for dot-mode spacing; UNCERTAIN.
    LaserInterval,
    /// `C6 11`. Used here for dot-mode dot time; UNCERTAIN.
    AddDelay,
    LaserOn,
    LaserOff,
    LaserOn2,
    LaserOff2,
}

impl DelayKind {
    const TABLE: [(DelayKind, u8); 6] = [
        (DelayKind::LaserInterval, 0x10),
        (DelayKind::AddDelay, 0x11),
        (DelayKind::LaserOn, 0x12),
        (DelayKind::LaserOff, 0x13),
        (DelayKind::LaserOn2, 0x15),
        (DelayKind::LaserOff2, 0x16),
    ];
    fn sub(self) -> u8 {
        DelayKind::TABLE.iter().find(|(k, _)| *k == self).map(|(_, b)| *b).unwrap_or(0x10)
    }
    fn from_sub(b: u8) -> Option<DelayKind> {
        DelayKind::TABLE.iter().find(|(_, s)| *s == b).map(|(k, _)| *k)
    }
}

/// `CA 01 xx` layer flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum LayerFlag {
    /// `CA 01 00`.
    LayerEnd,
    /// `CA 01 01..05` and `CA 01 55`.
    WorkMode(u8),
    /// `CA 01 10` / `11`: laser device 0 or 1.
    LaserDevice(u8),
    /// `CA 01 12` off / `13` on.
    AirAssist(bool),
    /// `CA 01 14`.
    DbHead,
    /// `CA 01 30` / `31`: laser 2 offset off / on.
    Laser2Offset(bool),
    Other(u8),
}

impl LayerFlag {
    fn byte(self) -> u8 {
        match self {
            LayerFlag::LayerEnd => 0x00,
            LayerFlag::WorkMode(m) => m,
            LayerFlag::LaserDevice(d) => 0x10 + (d & 1),
            LayerFlag::AirAssist(on) => 0x12 + on as u8,
            LayerFlag::DbHead => 0x14,
            LayerFlag::Laser2Offset(on) => 0x30 + on as u8,
            LayerFlag::Other(b) => b,
        }
    }
    fn from_byte(b: u8) -> Self {
        match b {
            0x00 => LayerFlag::LayerEnd,
            0x01..=0x05 | 0x55 => LayerFlag::WorkMode(b),
            0x10 | 0x11 => LayerFlag::LaserDevice(b - 0x10),
            0x12 | 0x13 => LayerFlag::AirAssist(b == 0x13),
            0x14 => LayerFlag::DbHead,
            0x30 | 0x31 => LayerFlag::Laser2Offset(b == 0x31),
            b => LayerFlag::Other(b),
        }
    }
}

/// One Ruida command. Coordinates are µm, speeds µm/s, times µs, frequencies Hz,
/// colours 0x00BBGGRR, settings addresses are 14-bit.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Command {
    // ---- motion -------------------------------------------------------------------------
    /// `88 x y` travel to an absolute point (laser off).
    MoveAbs { x: i32, y: i32 },
    /// `89 dx dy`.
    MoveRel { dx: i32, dy: i32 },
    /// `8A dx`.
    MoveRelX(i32),
    /// `8B dy`.
    MoveRelY(i32),
    /// `A8 x y` cut to an absolute point (laser on).
    CutAbs { x: i32, y: i32 },
    /// `A9 dx dy`.
    CutRel { dx: i32, dy: i32 },
    /// `AA dx`.
    CutRelX(i32),
    /// `AB dy`.
    CutRelY(i32),
    /// Single-axis absolute move inside a job: `80 00` X, `80 08` Z, `A0 00` Y, `A0 08` U.
    /// UNCERTAIN: MeerK40t uses `80 01` for Z; EduTechWiki says `80 08`. `A0 00` may be the A axis.
    AxisMove { axis: Axis, pos: i32 },

    // ---- power and timing ---------------------------------------------------------------
    /// `C7/C0/C2/C3 p` immediate power.
    ImmediatePower { laser: Laser, power: Power },
    /// `C8/C1/C4/C5 p` end power.
    EndPower { laser: Laser, power: Power },
    /// `C6 01/21/05/07 p` minimum (corner) power.
    MinPower { laser: Laser, power: Power },
    /// `C6 02/22/06/08 p` maximum power.
    MaxPower { laser: Laser, power: Power },
    /// `C6 31/41/35/37 part p` per-layer minimum power (job header).
    MinPowerPart { laser: Laser, part: u8, power: Power },
    /// `C6 32/42/36/38 part p` per-layer maximum power (job header).
    MaxPowerPart { laser: Laser, part: u8, power: Power },
    /// `C6 50/51/55/56 p` through power.
    ThroughPower { laser: Laser, power: Power },
    /// `C6 10..16 t` timing.
    Delay { kind: DelayKind, us: u32 },
    /// `C6 60 laser part hz` PWM frequency for a layer. `laser` is the raw 0-based byte.
    FrequencyPart { laser: u8, part: u8, hz: u32 },

    // ---- speed --------------------------------------------------------------------------
    /// `C9 02 v` cut speed.
    Speed(u32),
    /// `C9 03 v` axis speed.
    SpeedAxis(u32),
    /// `C9 04 part v` per-layer speed (job header).
    SpeedPart { part: u8, speed: u32 },
    /// `C9 05 v` force engraving speed.
    ForceEngSpeed(u32),
    /// `C9 06 v` axis move speed.
    SpeedAxisMove(u32),

    // ---- layer --------------------------------------------------------------------------
    /// `CA 01 xx`.
    Flag(LayerFlag),
    /// `CA 02 part` start of a layer in the job body.
    LayerNumber(u8),
    /// `CA 03 v` enable laser tube start.
    LaserTubeStart(u8),
    /// `CA 04 v`.
    XSignMap(u8),
    /// `CA 05 colour`.
    LayerColor(u32),
    /// `CA 06 part colour`.
    LayerColorPart { part: u8, color: u32 },
    /// `CA 10 v`.
    ExIo(u8),
    /// `CA 22 part` highest layer number in the job.
    MaxLayerPart(u8),
    /// `CA 30 n`.
    UFileId(u16),
    /// `CA 40 v`.
    ZuMap(u8),
    /// `CA 41 part mode`.
    WorkModePart { part: u8, mode: u8 },

    // ---- handshake ----------------------------------------------------------------------
    /// `CC`.
    Ack,
    /// `CD`.
    Err,
    /// `CE` enquiry / keep-alive.
    Enq,
    /// `CF` checksum NAK.
    Nak,
    /// `D7` end of file.
    EndOfFile,

    // ---- process (D8) -------------------------------------------------------------------
    /// `D8 00`.
    Start,
    /// `D8 01`.
    Stop,
    /// `D8 02`.
    Pause,
    /// `D8 03`.
    Resume,
    /// `D8 10/11/12` reference-point (origin) mode.
    RefPointMode(u8),
    /// `D8 2A`.
    HomeXY,
    /// `D8 2C`.
    HomeZ,
    /// `D8 2D`.
    HomeU,
    /// `D8 2E`.
    FocusZ,
    /// `D8 20..27`.
    KeyDown(JogKey),
    /// `D8 30..37`.
    KeyUp(JogKey),

    // ---- rapid moves (D9) ---------------------------------------------------------------
    /// `D9 00..03 opt pos` rapid move of one axis.
    RapidAxis { axis: Axis, option: RapidOption, pos: i32 },
    /// `D9 0F opt`.
    RapidFeedAxis(RapidOption),
    /// `D9 10 opt x y`.
    RapidXY { option: RapidOption, x: i32, y: i32 },
    /// `D9 30 opt x y u`.
    RapidXYU { option: RapidOption, x: i32, y: i32, u: i32 },

    // ---- settings (DA) ------------------------------------------------------------------
    /// `DA 00 addr` read a setting.
    GetSetting { addr: u16 },
    /// `DA 01 addr v v` write a setting (value sent twice).
    SetSetting { addr: u16, value: u32, value2: u32 },
    /// `DA 01 addr v` controller's reply to a read.
    SettingReply { addr: u16, value: u32 },
    /// `DA 01 addr ...` reply carrying something other than one int35 (for example a version string).
    SettingReplyRaw { addr: u16, data: Vec<u8> },

    // ---- document (E5/E6/E7) ------------------------------------------------------------
    /// `E5 02`.
    DocumentEnd,
    /// `E5 05 sum` file checksum.
    FileSum(u32),
    /// `E6 01`.
    SetAbsolute,
    /// `E7 00`.
    BlockEnd,
    /// `E7 01 name 00`.
    SetFilename(String),
    /// `E7 03 x y`.
    ProcessTopLeft { x: i32, y: i32 },
    /// `E7 04` 7 x int14.
    ProcessRepeat([u16; 7]),
    /// `E7 05 dir`.
    ArrayDirection(u8),
    /// `E7 06 v v`.
    FeedRepeat(u32, u32),
    /// `E7 07 x y`.
    ProcessBottomRight { x: i32, y: i32 },
    /// `E7 08` 7 x int14.
    ArrayRepeat([u16; 7]),
    /// `E7 50 x y`.
    DocumentMin { x: i32, y: i32 },
    /// `E7 51 x y`.
    DocumentMax { x: i32, y: i32 },
    /// `E7 52 part x y`.
    PartMin { part: u8, x: i32, y: i32 },
    /// `E7 53 part x y`.
    PartMax { part: u8, x: i32, y: i32 },
    /// `E7 54 axis v`.
    PenOffset { axis: u8, value: i32 },
    /// `E7 55 axis v`.
    LayerOffset { axis: u8, value: i32 },
    /// `E7 60 i`.
    CurrentElementIndex(u8),
    /// `E7 61 part x y`.
    PartMinEx { part: u8, x: i32, y: i32 },
    /// `E7 62 part x y`.
    PartMaxEx { part: u8, x: i32, y: i32 },
    /// `EA i`.
    ArrayStart(u8),
    /// `EB`.
    ArrayEnd,
    /// `F0`.
    RefPointSet,
    /// `F1 00 i`.
    ElementMaxIndex(u8),
    /// `F1 01 i`.
    ElementNameMaxIndex(u8),
    /// `F1 02 v`.
    EnableBlockCutting(u8),
    /// `F1 03 x y`.
    DisplayOffset { x: i32, y: i32 },
    /// `F1 04 v`.
    FeedAutoCalc(u8),

    // ---- file system (E8) ---------------------------------------------------------------
    /// `E8 00 first last` delete files `first..=last` (1-based). `0, 0` deletes all.
    DeleteFiles { first: u16, last: u16 },
    /// `E8 01 n` ask for the name of file `n` (1-based).
    GetFileName(u16),
    /// `E8 01 n name 00` controller's reply.
    FileName { index: u16, name: String },
    /// `E8 02` the following `E7 01 name` and job bytes are stored, not run.
    BeginFileTransfer,
    /// `E8 03 n` run stored file `n`.
    RunFile(u16),
    /// `E8 04 n` calculate the run time of file `n`.
    CalcFileTime(u16),

    /// Anything else, kept byte for byte.
    Raw(Vec<u8>),
}

fn push_i35(v: &mut Vec<u8>, x: i32) {
    v.extend_from_slice(&encode_i35(x));
}
fn push_u35(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&encode_u35(x));
}
fn push_i14(v: &mut Vec<u8>, x: i32) -> Result<()> {
    v.extend_from_slice(&encode_i14(x)?);
    Ok(())
}
fn push_u14(v: &mut Vec<u8>, x: u16) {
    v.extend_from_slice(&encode_u14(x));
}
fn param(b: u8) -> Result<u8> {
    if b >= 0x80 { Err(RuidaError::Encode(format!("parameter byte 0x{b:02X} must be < 0x80"))) } else { Ok(b) }
}

impl Command {
    /// Encode to logical bytes.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut v = Vec::with_capacity(12);
        self.encode_into(&mut v)?;
        Ok(v)
    }

    /// Append logical bytes to `v`.
    pub fn encode_into(&self, v: &mut Vec<u8>) -> Result<()> {
        use Command::*;
        match self {
            MoveAbs { x, y } => {
                v.push(0x88);
                push_i35(v, *x);
                push_i35(v, *y);
            }
            MoveRel { dx, dy } => {
                v.push(0x89);
                push_i14(v, *dx)?;
                push_i14(v, *dy)?;
            }
            MoveRelX(d) => {
                v.push(0x8A);
                push_i14(v, *d)?;
            }
            MoveRelY(d) => {
                v.push(0x8B);
                push_i14(v, *d)?;
            }
            CutAbs { x, y } => {
                v.push(0xA8);
                push_i35(v, *x);
                push_i35(v, *y);
            }
            CutRel { dx, dy } => {
                v.push(0xA9);
                push_i14(v, *dx)?;
                push_i14(v, *dy)?;
            }
            CutRelX(d) => {
                v.push(0xAA);
                push_i14(v, *d)?;
            }
            CutRelY(d) => {
                v.push(0xAB);
                push_i14(v, *d)?;
            }
            AxisMove { axis, pos } => {
                let (op, sub) = match axis {
                    Axis::X => (0x80, 0x00),
                    Axis::Z => (0x80, 0x08),
                    Axis::Y => (0xA0, 0x00),
                    Axis::U => (0xA0, 0x08),
                };
                v.extend_from_slice(&[op, sub]);
                push_i35(v, *pos);
            }
            ImmediatePower { laser, power } => {
                v.push(laser.imd_op());
                v.extend_from_slice(&power.encode());
            }
            EndPower { laser, power } => {
                v.push(laser.end_op());
                v.extend_from_slice(&power.encode());
            }
            MinPower { laser, power } => {
                v.extend_from_slice(&[0xC6, laser.min_sub()]);
                v.extend_from_slice(&power.encode());
            }
            MaxPower { laser, power } => {
                v.extend_from_slice(&[0xC6, laser.max_sub()]);
                v.extend_from_slice(&power.encode());
            }
            MinPowerPart { laser, part, power } => {
                v.extend_from_slice(&[0xC6, laser.min_part_sub(), param(*part)?]);
                v.extend_from_slice(&power.encode());
            }
            MaxPowerPart { laser, part, power } => {
                v.extend_from_slice(&[0xC6, laser.max_part_sub(), param(*part)?]);
                v.extend_from_slice(&power.encode());
            }
            ThroughPower { laser, power } => {
                v.extend_from_slice(&[0xC6, laser.through_sub()]);
                v.extend_from_slice(&power.encode());
            }
            Delay { kind, us } => {
                v.extend_from_slice(&[0xC6, kind.sub()]);
                push_u35(v, *us);
            }
            FrequencyPart { laser, part, hz } => {
                v.extend_from_slice(&[0xC6, 0x60, param(*laser)?, param(*part)?]);
                push_u35(v, *hz);
            }
            Speed(s) => {
                v.extend_from_slice(&[0xC9, 0x02]);
                push_u35(v, *s);
            }
            SpeedAxis(s) => {
                v.extend_from_slice(&[0xC9, 0x03]);
                push_u35(v, *s);
            }
            SpeedPart { part, speed } => {
                v.extend_from_slice(&[0xC9, 0x04, param(*part)?]);
                push_u35(v, *speed);
            }
            ForceEngSpeed(s) => {
                v.extend_from_slice(&[0xC9, 0x05]);
                push_u35(v, *s);
            }
            SpeedAxisMove(s) => {
                v.extend_from_slice(&[0xC9, 0x06]);
                push_u35(v, *s);
            }
            Flag(f) => v.extend_from_slice(&[0xCA, 0x01, param(f.byte())?]),
            LayerNumber(p) => v.extend_from_slice(&[0xCA, 0x02, param(*p)?]),
            LaserTubeStart(p) => v.extend_from_slice(&[0xCA, 0x03, param(*p)?]),
            XSignMap(p) => v.extend_from_slice(&[0xCA, 0x04, param(*p)?]),
            LayerColor(c) => {
                v.extend_from_slice(&[0xCA, 0x05]);
                push_u35(v, *c);
            }
            LayerColorPart { part, color } => {
                v.extend_from_slice(&[0xCA, 0x06, param(*part)?]);
                push_u35(v, *color);
            }
            ExIo(p) => v.extend_from_slice(&[0xCA, 0x10, param(*p)?]),
            MaxLayerPart(p) => v.extend_from_slice(&[0xCA, 0x22, param(*p)?]),
            UFileId(n) => {
                v.extend_from_slice(&[0xCA, 0x30]);
                push_u14(v, *n);
            }
            ZuMap(p) => v.extend_from_slice(&[0xCA, 0x40, param(*p)?]),
            WorkModePart { part, mode } => v.extend_from_slice(&[0xCA, 0x41, param(*part)?, param(*mode)?]),
            Ack => v.push(0xCC),
            Err => v.push(0xCD),
            Enq => v.push(0xCE),
            Nak => v.push(0xCF),
            EndOfFile => v.push(0xD7),
            Start => v.extend_from_slice(&[0xD8, 0x00]),
            Stop => v.extend_from_slice(&[0xD8, 0x01]),
            Pause => v.extend_from_slice(&[0xD8, 0x02]),
            Resume => v.extend_from_slice(&[0xD8, 0x03]),
            RefPointMode(m) => v.extend_from_slice(&[0xD8, param(*m)?]),
            HomeXY => v.extend_from_slice(&[0xD8, 0x2A]),
            HomeZ => v.extend_from_slice(&[0xD8, 0x2C]),
            HomeU => v.extend_from_slice(&[0xD8, 0x2D]),
            FocusZ => v.extend_from_slice(&[0xD8, 0x2E]),
            KeyDown(k) => v.extend_from_slice(&[0xD8, 0x20 + k.offset()]),
            KeyUp(k) => v.extend_from_slice(&[0xD8, 0x30 + k.offset()]),
            RapidAxis { axis, option, pos } => {
                v.extend_from_slice(&[0xD9, axis.index(), param(option.byte())?]);
                push_i35(v, *pos);
            }
            RapidFeedAxis(o) => v.extend_from_slice(&[0xD9, 0x0F, param(o.byte())?]),
            RapidXY { option, x, y } => {
                v.extend_from_slice(&[0xD9, 0x10, param(option.byte())?]);
                push_i35(v, *x);
                push_i35(v, *y);
            }
            RapidXYU { option, x, y, u } => {
                v.extend_from_slice(&[0xD9, 0x30, param(option.byte())?]);
                push_i35(v, *x);
                push_i35(v, *y);
                push_i35(v, *u);
            }
            GetSetting { addr } => {
                v.extend_from_slice(&[0xDA, 0x00]);
                push_u14(v, *addr);
            }
            SetSetting { addr, value, value2 } => {
                v.extend_from_slice(&[0xDA, 0x01]);
                push_u14(v, *addr);
                push_u35(v, *value);
                push_u35(v, *value2);
            }
            SettingReply { addr, value } => {
                v.extend_from_slice(&[0xDA, 0x01]);
                push_u14(v, *addr);
                push_u35(v, *value);
            }
            SettingReplyRaw { addr, data } => {
                v.extend_from_slice(&[0xDA, 0x01]);
                push_u14(v, *addr);
                for &b in data {
                    v.push(param(b)?);
                }
            }
            DocumentEnd => v.extend_from_slice(&[0xE5, 0x02]),
            FileSum(s) => {
                v.extend_from_slice(&[0xE5, 0x05]);
                push_u35(v, *s);
            }
            SetAbsolute => v.extend_from_slice(&[0xE6, 0x01]),
            BlockEnd => v.extend_from_slice(&[0xE7, 0x00]),
            SetFilename(name) => {
                v.extend_from_slice(&[0xE7, 0x01]);
                v.extend_from_slice(&encode_cstring(name));
            }
            ProcessTopLeft { x, y } => xy(v, &[0xE7, 0x03], *x, *y),
            ProcessRepeat(a) => {
                v.extend_from_slice(&[0xE7, 0x04]);
                a.iter().for_each(|&n| push_u14(v, n));
            }
            ArrayDirection(d) => v.extend_from_slice(&[0xE7, 0x05, param(*d)?]),
            FeedRepeat(a, b) => {
                v.extend_from_slice(&[0xE7, 0x06]);
                push_u35(v, *a);
                push_u35(v, *b);
            }
            ProcessBottomRight { x, y } => xy(v, &[0xE7, 0x07], *x, *y),
            ArrayRepeat(a) => {
                v.extend_from_slice(&[0xE7, 0x08]);
                a.iter().for_each(|&n| push_u14(v, n));
            }
            DocumentMin { x, y } => xy(v, &[0xE7, 0x50], *x, *y),
            DocumentMax { x, y } => xy(v, &[0xE7, 0x51], *x, *y),
            PartMin { part, x, y } => xy(v, &[0xE7, 0x52, param(*part)?], *x, *y),
            PartMax { part, x, y } => xy(v, &[0xE7, 0x53, param(*part)?], *x, *y),
            PenOffset { axis, value } => {
                v.extend_from_slice(&[0xE7, 0x54, param(*axis)?]);
                push_i35(v, *value);
            }
            LayerOffset { axis, value } => {
                v.extend_from_slice(&[0xE7, 0x55, param(*axis)?]);
                push_i35(v, *value);
            }
            CurrentElementIndex(i) => v.extend_from_slice(&[0xE7, 0x60, param(*i)?]),
            PartMinEx { part, x, y } => xy(v, &[0xE7, 0x61, param(*part)?], *x, *y),
            PartMaxEx { part, x, y } => xy(v, &[0xE7, 0x62, param(*part)?], *x, *y),
            ArrayStart(i) => v.extend_from_slice(&[0xEA, param(*i)?]),
            ArrayEnd => v.push(0xEB),
            RefPointSet => v.push(0xF0),
            ElementMaxIndex(i) => v.extend_from_slice(&[0xF1, 0x00, param(*i)?]),
            ElementNameMaxIndex(i) => v.extend_from_slice(&[0xF1, 0x01, param(*i)?]),
            EnableBlockCutting(i) => v.extend_from_slice(&[0xF1, 0x02, param(*i)?]),
            DisplayOffset { x, y } => xy(v, &[0xF1, 0x03], *x, *y),
            FeedAutoCalc(i) => v.extend_from_slice(&[0xF1, 0x04, param(*i)?]),
            DeleteFiles { first, last } => {
                v.extend_from_slice(&[0xE8, 0x00]);
                push_u14(v, *first);
                push_u14(v, *last);
            }
            GetFileName(n) => {
                v.extend_from_slice(&[0xE8, 0x01]);
                push_u14(v, *n);
            }
            FileName { index, name } => {
                v.extend_from_slice(&[0xE8, 0x01]);
                push_u14(v, *index);
                v.extend_from_slice(&encode_cstring(name));
            }
            BeginFileTransfer => v.extend_from_slice(&[0xE8, 0x02]),
            RunFile(n) => {
                v.extend_from_slice(&[0xE8, 0x03]);
                push_u14(v, *n);
            }
            CalcFileTime(n) => {
                v.extend_from_slice(&[0xE8, 0x04]);
                push_u14(v, *n);
            }
            Raw(bytes) => v.extend_from_slice(bytes),
        }
        Ok(())
    }

    /// Decode exactly one command. Unknown or malformed input becomes [`Command::Raw`].
    pub fn decode(b: &[u8]) -> Command {
        decode_typed(b).unwrap_or_else(|| Command::Raw(b.to_vec()))
    }

    /// True for commands that move the head.
    pub fn is_motion(&self) -> bool {
        use Command::*;
        matches!(
            self,
            MoveAbs { .. }
                | MoveRel { .. }
                | MoveRelX(_)
                | MoveRelY(_)
                | CutAbs { .. }
                | CutRel { .. }
                | CutRelX(_)
                | CutRelY(_)
                | AxisMove { .. }
                | RapidAxis { .. }
                | RapidXY { .. }
                | RapidXYU { .. }
        )
    }
}

fn xy(v: &mut Vec<u8>, head: &[u8], x: i32, y: i32) {
    v.extend_from_slice(head);
    push_i35(v, x);
    push_i35(v, y);
}

fn power_laser(f: impl Fn(Laser) -> u8, sub: u8) -> Option<Laser> {
    Laser::find(f, sub)
}

fn decode_typed(b: &[u8]) -> Option<Command> {
    use Command::*;
    let n = b.len();
    let op = *b.first()?;
    let len_is = |l: usize| if n == l { Some(()) } else { None };
    let i35 = |at: usize| decode_i35(b.get(at..at + 5)?).ok();
    let u35 = |at: usize| decode_u35(b.get(at..at + 5)?).ok();
    let i14 = |at: usize| decode_i14(b.get(at..at + 2)?).ok();
    let u14 = |at: usize| decode_u14(b.get(at..at + 2)?).ok();
    let pw = |at: usize| Power::decode(b.get(at..at + 2)?).ok();
    let sub = b.get(1).copied();
    Some(match op {
        0x88 => {
            len_is(11)?;
            MoveAbs { x: i35(1)?, y: i35(6)? }
        }
        0x89 => {
            len_is(5)?;
            MoveRel { dx: i14(1)?, dy: i14(3)? }
        }
        0x8A => {
            len_is(3)?;
            MoveRelX(i14(1)?)
        }
        0x8B => {
            len_is(3)?;
            MoveRelY(i14(1)?)
        }
        0xA8 => {
            len_is(11)?;
            CutAbs { x: i35(1)?, y: i35(6)? }
        }
        0xA9 => {
            len_is(5)?;
            CutRel { dx: i14(1)?, dy: i14(3)? }
        }
        0xAA => {
            len_is(3)?;
            CutRelX(i14(1)?)
        }
        0xAB => {
            len_is(3)?;
            CutRelY(i14(1)?)
        }
        0x80 | 0xA0 => {
            len_is(7)?;
            let axis = match (op, sub?) {
                (0x80, 0x00) => Axis::X,
                (0x80, 0x08) => Axis::Z,
                (0xA0, 0x00) => Axis::Y,
                (0xA0, 0x08) => Axis::U,
                _ => return None,
            };
            AxisMove { axis, pos: i35(2)? }
        }
        0xC7 | 0xC0 | 0xC2 | 0xC3 => {
            len_is(3)?;
            ImmediatePower { laser: power_laser(Laser::imd_op, op)?, power: pw(1)? }
        }
        0xC8 | 0xC1 | 0xC4 | 0xC5 => {
            len_is(3)?;
            EndPower { laser: power_laser(Laser::end_op, op)?, power: pw(1)? }
        }
        0xC6 => {
            let s = sub?;
            if let Some(l) = power_laser(Laser::min_sub, s) {
                len_is(4)?;
                MinPower { laser: l, power: pw(2)? }
            } else if let Some(l) = power_laser(Laser::max_sub, s) {
                len_is(4)?;
                MaxPower { laser: l, power: pw(2)? }
            } else if let Some(l) = power_laser(Laser::min_part_sub, s) {
                len_is(5)?;
                MinPowerPart { laser: l, part: b[2], power: pw(3)? }
            } else if let Some(l) = power_laser(Laser::max_part_sub, s) {
                len_is(5)?;
                MaxPowerPart { laser: l, part: b[2], power: pw(3)? }
            } else if let Some(l) = power_laser(Laser::through_sub, s) {
                len_is(4)?;
                ThroughPower { laser: l, power: pw(2)? }
            } else if let Some(kind) = DelayKind::from_sub(s) {
                len_is(7)?;
                Delay { kind, us: u35(2)? }
            } else if s == 0x60 {
                len_is(9)?;
                FrequencyPart { laser: b[2], part: b[3], hz: u35(4)? }
            } else {
                return None;
            }
        }
        0xC9 => match sub? {
            0x02 => {
                len_is(7)?;
                Speed(u35(2)?)
            }
            0x03 => {
                len_is(7)?;
                SpeedAxis(u35(2)?)
            }
            0x04 => {
                len_is(8)?;
                SpeedPart { part: b[2], speed: u35(3)? }
            }
            0x05 => {
                len_is(7)?;
                ForceEngSpeed(u35(2)?)
            }
            0x06 => {
                len_is(7)?;
                SpeedAxisMove(u35(2)?)
            }
            _ => return None,
        },
        0xCA => {
            let s = sub?;
            match s {
                0x01 | 0x02 | 0x03 | 0x04 | 0x10 | 0x22 | 0x40 => {
                    len_is(3)?;
                    let p = b[2];
                    match s {
                        0x01 => Flag(LayerFlag::from_byte(p)),
                        0x02 => LayerNumber(p),
                        0x03 => LaserTubeStart(p),
                        0x04 => XSignMap(p),
                        0x10 => ExIo(p),
                        0x22 => MaxLayerPart(p),
                        _ => ZuMap(p),
                    }
                }
                0x05 => {
                    len_is(7)?;
                    LayerColor(u35(2)?)
                }
                0x06 => {
                    len_is(8)?;
                    LayerColorPart { part: b[2], color: u35(3)? }
                }
                0x30 => {
                    len_is(4)?;
                    UFileId(u14(2)?)
                }
                0x41 => {
                    len_is(4)?;
                    WorkModePart { part: b[2], mode: b[3] }
                }
                _ => return None,
            }
        }
        0xCC => {
            len_is(1)?;
            Ack
        }
        0xCD => {
            len_is(1)?;
            Err
        }
        0xCE => {
            len_is(1)?;
            Enq
        }
        0xCF => {
            len_is(1)?;
            Nak
        }
        0xD7 => {
            len_is(1)?;
            EndOfFile
        }
        0xD8 => {
            len_is(2)?;
            match sub? {
                0x00 => Start,
                0x01 => Stop,
                0x02 => Pause,
                0x03 => Resume,
                m @ 0x10..=0x12 => RefPointMode(m),
                0x2A => HomeXY,
                0x2C => HomeZ,
                0x2D => HomeU,
                0x2E => FocusZ,
                k @ 0x20..=0x27 => KeyDown(JogKey::ALL[(k - 0x20) as usize]),
                k @ 0x30..=0x37 => KeyUp(JogKey::ALL[(k - 0x30) as usize]),
                _ => return None,
            }
        }
        0xD9 => {
            let s = sub?;
            let option = RapidOption::from_byte(*b.get(2)?);
            match s {
                0x00..=0x03 => {
                    len_is(8)?;
                    RapidAxis { axis: Axis::from_index(s)?, option, pos: i35(3)? }
                }
                0x0F => {
                    len_is(3)?;
                    RapidFeedAxis(option)
                }
                0x10 => {
                    len_is(13)?;
                    RapidXY { option, x: i35(3)?, y: i35(8)? }
                }
                0x30 => {
                    len_is(18)?;
                    RapidXYU { option, x: i35(3)?, y: i35(8)?, u: i35(13)? }
                }
                _ => return None,
            }
        }
        0xDA => match sub? {
            0x00 => {
                len_is(4)?;
                GetSetting { addr: u14(2)? }
            }
            0x01 => {
                let addr = u14(2)?;
                match n {
                    14 => SetSetting { addr, value: u35(4)?, value2: u35(9)? },
                    9 => SettingReply { addr, value: u35(4)? },
                    _ => SettingReplyRaw { addr, data: b[4..].to_vec() },
                }
            }
            _ => return None,
        },
        0xE5 => match sub? {
            0x02 => {
                len_is(2)?;
                DocumentEnd
            }
            0x05 => {
                len_is(7)?;
                FileSum(u35(2)?)
            }
            _ => return None,
        },
        0xE6 => {
            len_is(2)?;
            if sub? != 0x01 {
                return None;
            }
            SetAbsolute
        }
        0xE7 => {
            let s = sub?;
            match s {
                0x00 => {
                    len_is(2)?;
                    BlockEnd
                }
                0x01 => {
                    let (name, used) = decode_cstring(&b[2..]);
                    if used + 2 != n || b.last() != Some(&0) {
                        return None;
                    }
                    SetFilename(name)
                }
                0x03 | 0x07 | 0x50 | 0x51 => {
                    len_is(12)?;
                    let (x, y) = (i35(2)?, i35(7)?);
                    match s {
                        0x03 => ProcessTopLeft { x, y },
                        0x07 => ProcessBottomRight { x, y },
                        0x50 => DocumentMin { x, y },
                        _ => DocumentMax { x, y },
                    }
                }
                0x04 | 0x08 => {
                    len_is(16)?;
                    let mut a = [0u16; 7];
                    for (i, slot) in a.iter_mut().enumerate() {
                        *slot = u14(2 + 2 * i)?;
                    }
                    if s == 0x04 { ProcessRepeat(a) } else { ArrayRepeat(a) }
                }
                0x05 => {
                    len_is(3)?;
                    ArrayDirection(b[2])
                }
                0x06 => {
                    len_is(12)?;
                    FeedRepeat(u35(2)?, u35(7)?)
                }
                0x52 | 0x53 | 0x61 | 0x62 => {
                    len_is(13)?;
                    let (part, x, y) = (b[2], i35(3)?, i35(8)?);
                    match s {
                        0x52 => PartMin { part, x, y },
                        0x53 => PartMax { part, x, y },
                        0x61 => PartMinEx { part, x, y },
                        _ => PartMaxEx { part, x, y },
                    }
                }
                0x54 | 0x55 => {
                    len_is(8)?;
                    let (axis, value) = (b[2], i35(3)?);
                    if s == 0x54 { PenOffset { axis, value } } else { LayerOffset { axis, value } }
                }
                0x60 => {
                    len_is(3)?;
                    CurrentElementIndex(b[2])
                }
                _ => return None,
            }
        }
        0xEA => {
            len_is(2)?;
            ArrayStart(b[1])
        }
        0xEB => {
            len_is(1)?;
            ArrayEnd
        }
        0xF0 => {
            len_is(1)?;
            RefPointSet
        }
        0xF1 => match sub? {
            0x03 => {
                len_is(12)?;
                DisplayOffset { x: i35(2)?, y: i35(7)? }
            }
            s @ (0x00 | 0x01 | 0x02 | 0x04) => {
                len_is(3)?;
                let p = b[2];
                match s {
                    0x00 => ElementMaxIndex(p),
                    0x01 => ElementNameMaxIndex(p),
                    0x02 => EnableBlockCutting(p),
                    _ => FeedAutoCalc(p),
                }
            }
            _ => return None,
        },
        0xE8 => match sub? {
            0x00 => {
                len_is(6)?;
                DeleteFiles { first: u14(2)?, last: u14(4)? }
            }
            0x01 => {
                let index = u14(2)?;
                if n == 4 {
                    GetFileName(index)
                } else {
                    let (name, _) = decode_cstring(&b[4..]);
                    if b.last() != Some(&0) || name.len() + 5 != n {
                        return None;
                    }
                    FileName { index, name }
                }
            }
            0x02 => {
                len_is(2)?;
                BeginFileTransfer
            }
            0x03 => {
                len_is(4)?;
                RunFile(u14(2)?)
            }
            0x04 => {
                len_is(4)?;
                CalcFileTime(u14(2)?)
            }
            _ => return None,
        },
        _ => return None,
    })
}

/// Split a logical byte stream into command slices: every byte >= 0x80 starts a new command.
pub fn split_commands(data: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut mark = 0;
    for (i, &b) in data.iter().enumerate() {
        if b >= 0x80 && i != mark {
            out.push(&data[mark..i]);
            mark = i;
        }
    }
    if mark < data.len() {
        out.push(&data[mark..]);
    }
    out
}

/// Decode a logical byte stream into commands.
pub fn parse_commands(data: &[u8]) -> Vec<Command> {
    split_commands(data).into_iter().map(Command::decode).collect()
}

/// Encode many commands into one logical byte stream.
pub fn encode_commands(cmds: &[Command]) -> Result<Vec<u8>> {
    let mut v = Vec::new();
    for c in cmds {
        c.encode_into(&mut v)?;
    }
    Ok(v)
}

/// Commands on the jog / "wireless panel" port (50207). These are NOT swizzled and carry no checksum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum PanelCommand {
    /// `CC`.
    Connect,
    /// `CE`.
    KeepAlive,
    /// `CD`.
    Disconnect,
    /// `A5 50 key`.
    KeyDown(PanelKey),
    /// `A5 51 key`.
    KeyUp(PanelKey),
    /// `A5 53 00` frame the current job.
    Frame,
}

/// Keys of the wireless panel protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum PanelKey {
    MinusX,
    PlusX,
    PlusY,
    MinusY,
    Pulse,
    StartPause,
    Escape,
    Origin,
    Stop,
    PlusZ,
    MinusZ,
    PlusU,
    MinusU,
    Trace,
    Speed,
    LaserGate,
    Reset,
    Other(u8),
}

impl PanelKey {
    const TABLE: [(PanelKey, u8); 17] = [
        (PanelKey::MinusX, 0x01),
        (PanelKey::PlusX, 0x02),
        (PanelKey::PlusY, 0x03),
        (PanelKey::MinusY, 0x04),
        (PanelKey::Pulse, 0x05),
        (PanelKey::StartPause, 0x06),
        (PanelKey::Escape, 0x07),
        (PanelKey::Origin, 0x08),
        (PanelKey::Stop, 0x09),
        (PanelKey::PlusZ, 0x0A),
        (PanelKey::MinusZ, 0x0B),
        (PanelKey::PlusU, 0x0C),
        (PanelKey::MinusU, 0x0D),
        (PanelKey::Trace, 0x0F),
        (PanelKey::Speed, 0x11),
        (PanelKey::LaserGate, 0x12),
        (PanelKey::Reset, 0x5A),
    ];

    pub fn byte(self) -> u8 {
        match self {
            PanelKey::Other(b) => b,
            k => PanelKey::TABLE.iter().find(|(x, _)| *x == k).map(|(_, b)| *b).unwrap_or(0),
        }
    }

    pub fn from_byte(b: u8) -> PanelKey {
        PanelKey::TABLE.iter().find(|(_, x)| *x == b).map(|(k, _)| *k).unwrap_or(PanelKey::Other(b))
    }
}

impl PanelCommand {
    pub fn encode(self) -> Vec<u8> {
        match self {
            PanelCommand::Connect => vec![0xCC],
            PanelCommand::KeepAlive => vec![0xCE],
            PanelCommand::Disconnect => vec![0xCD],
            PanelCommand::KeyDown(k) => vec![0xA5, 0x50, k.byte()],
            PanelCommand::KeyUp(k) => vec![0xA5, 0x51, k.byte()],
            PanelCommand::Frame => vec![0xA5, 0x53, 0x00],
        }
    }

    pub fn decode(b: &[u8]) -> Option<PanelCommand> {
        match b {
            [0xCC] => Some(PanelCommand::Connect),
            [0xCE] => Some(PanelCommand::KeepAlive),
            [0xCD] => Some(PanelCommand::Disconnect),
            [0xA5, 0x50, k] => Some(PanelCommand::KeyDown(PanelKey::from_byte(*k))),
            [0xA5, 0x51, k] => Some(PanelCommand::KeyUp(PanelKey::from_byte(*k))),
            [0xA5, 0x53, 0x00] => Some(PanelCommand::Frame),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One example of every typed command.
    pub(crate) fn all_examples() -> Vec<Command> {
        use Command::*;
        let mut v = vec![
            MoveAbs { x: 123_456, y: -7_890 },
            MoveRel { dx: -8192, dy: 8191 },
            MoveRelX(-5),
            MoveRelY(77),
            CutAbs { x: 0, y: 600_000 },
            CutRel { dx: 1, dy: -1 },
            CutRelX(4000),
            CutRelY(-4000),
            Delay { kind: DelayKind::LaserOn, us: 1500 },
            FrequencyPart { laser: 0, part: 3, hz: 20_000 },
            Speed(100_000),
            SpeedAxis(200_000),
            SpeedPart { part: 2, speed: 50_000 },
            ForceEngSpeed(300_000),
            SpeedAxisMove(150_000),
            LayerNumber(5),
            LaserTubeStart(1),
            XSignMap(0),
            LayerColor(0x0000FF),
            LayerColorPart { part: 1, color: 0x00FF00 },
            ExIo(0),
            MaxLayerPart(3),
            UFileId(1000),
            ZuMap(1),
            WorkModePart { part: 0, mode: 0 },
            Ack,
            Err,
            Enq,
            Nak,
            EndOfFile,
            Start,
            Stop,
            Pause,
            Resume,
            RefPointMode(0x10),
            RefPointMode(0x11),
            RefPointMode(0x12),
            HomeXY,
            HomeZ,
            HomeU,
            FocusZ,
            RapidFeedAxis(RapidOption::None),
            RapidXY { option: RapidOption::Light, x: 1000, y: 2000 },
            RapidXYU { option: RapidOption::Origin, x: -1, y: 2, u: 3 },
            GetSetting { addr: 0x2FE },
            SetSetting { addr: 0x26, value: 600_000, value2: 600_000 },
            SettingReply { addr: 0x221, value: 12345 },
            SettingReplyRaw { addr: 0x2FF, data: b"RDC6442S\0".to_vec() },
            DocumentEnd,
            FileSum(0x1234567),
            SetAbsolute,
            BlockEnd,
            SetFilename("JOB01".into()),
            SetFilename(String::new()),
            ProcessTopLeft { x: 1, y: 2 },
            ProcessRepeat([1, 1, 0, 0, 0, 0, 0]),
            ArrayDirection(0),
            FeedRepeat(0, 0),
            ProcessBottomRight { x: 3, y: 4 },
            ArrayRepeat([1, 1, 0, 1123, 0x3FFF, 4, 3480]),
            DocumentMin { x: -5, y: -6 },
            DocumentMax { x: 5, y: 6 },
            PartMin { part: 1, x: 7, y: 8 },
            PartMax { part: 1, x: 9, y: 10 },
            PenOffset { axis: 0, value: -100 },
            LayerOffset { axis: 1, value: 100 },
            CurrentElementIndex(0),
            PartMinEx { part: 2, x: 11, y: 12 },
            PartMaxEx { part: 2, x: 13, y: 14 },
            ArrayStart(0),
            ArrayEnd,
            RefPointSet,
            ElementMaxIndex(0),
            ElementNameMaxIndex(0),
            EnableBlockCutting(0),
            DisplayOffset { x: 0, y: 0 },
            FeedAutoCalc(0),
            DeleteFiles { first: 0, last: 0 },
            DeleteFiles { first: 3, last: 5 },
            GetFileName(1),
            FileName { index: 2, name: "CUT2".into() },
            FileName { index: 3, name: String::new() },
            BeginFileTransfer,
            RunFile(4),
            CalcFileTime(4),
            Raw(vec![0xFE, 0x01, 0x02]),
            Raw(vec![0xE7, 0x38, 0x00]),
        ];
        for axis in Axis::ALL {
            v.push(AxisMove { axis, pos: -42_000 });
            v.push(RapidAxis { axis, option: RapidOption::None, pos: 9_000 });
        }
        for laser in Laser::ALL {
            let power = Power(laser.number() as u16 * 1000);
            v.push(ImmediatePower { laser, power });
            v.push(EndPower { laser, power });
            v.push(MinPower { laser, power });
            v.push(MaxPower { laser, power });
            v.push(MinPowerPart { laser, part: 7, power });
            v.push(MaxPowerPart { laser, part: 7, power });
            v.push(ThroughPower { laser, power });
        }
        for kind in DelayKind::TABLE.map(|(k, _)| k) {
            v.push(Delay { kind, us: 250 });
        }
        for k in JogKey::ALL {
            v.push(KeyDown(k));
            v.push(KeyUp(k));
        }
        for f in [
            LayerFlag::LayerEnd,
            LayerFlag::WorkMode(1),
            LayerFlag::WorkMode(3),
            LayerFlag::WorkMode(0x55),
            LayerFlag::LaserDevice(0),
            LayerFlag::LaserDevice(1),
            LayerFlag::AirAssist(true),
            LayerFlag::AirAssist(false),
            LayerFlag::DbHead,
            LayerFlag::Laser2Offset(false),
            LayerFlag::Laser2Offset(true),
            LayerFlag::Other(0x7E),
        ] {
            v.push(Flag(f));
        }
        v
    }

    #[test]
    fn every_command_round_trips() {
        for cmd in all_examples() {
            let bytes = cmd.encode().unwrap();
            assert!(bytes[0] >= 0x80, "{cmd:?} must start with a command byte");
            assert!(bytes[1..].iter().all(|&b| b < 0x80), "{cmd:?} params must be 7-bit: {bytes:02X?}");
            assert_eq!(Command::decode(&bytes), cmd, "bytes {bytes:02X?}");
        }
    }

    #[test]
    fn stream_round_trips() {
        let cmds = all_examples();
        let bytes = encode_commands(&cmds).unwrap();
        assert_eq!(parse_commands(&bytes), cmds);
    }

    #[test]
    fn known_byte_layouts() {
        assert_eq!(Command::GetSetting { addr: 0x2FE }.encode().unwrap(), [0xDA, 0x00, 0x05, 0x7E]);
        assert_eq!(Command::GetSetting { addr: 0x26 }.encode().unwrap(), [0xDA, 0x00, 0x00, 0x26]);
        assert_eq!(Command::Flag(LayerFlag::AirAssist(true)).encode().unwrap(), [0xCA, 0x01, 0x13]);
        assert_eq!(Command::MinPower { laser: Laser::L2, power: Power::FULL }.encode().unwrap(), [0xC6, 0x21, 0x7F, 0x7F]);
        assert_eq!(Command::KeyDown(JogKey::XRight).encode().unwrap(), [0xD8, 0x21]);
        assert_eq!(Command::KeyUp(JogKey::UBackward).encode().unwrap(), [0xD8, 0x37]);
        assert_eq!(Command::RunFile(1).encode().unwrap(), [0xE8, 0x03, 0x00, 0x01]);
    }

    #[test]
    fn out_of_range_relative_is_error() {
        assert!(Command::MoveRelX(9000).encode().is_err());
        assert!(Command::CutRel { dx: 0, dy: -9000 }.encode().is_err());
        assert!(Command::LayerNumber(0x80).encode().is_err());
    }

    #[test]
    fn malformed_becomes_raw() {
        assert_eq!(Command::decode(&[0x88, 0x01]), Command::Raw(vec![0x88, 0x01]));
        assert_eq!(Command::decode(&[0xD8, 0x7F]), Command::Raw(vec![0xD8, 0x7F]));
    }

    #[test]
    fn panel_commands_round_trip() {
        let mut all = vec![PanelCommand::Connect, PanelCommand::KeepAlive, PanelCommand::Disconnect, PanelCommand::Frame];
        for (k, _) in PanelKey::TABLE {
            all.push(PanelCommand::KeyDown(k));
            all.push(PanelCommand::KeyUp(k));
        }
        all.push(PanelCommand::KeyDown(PanelKey::Other(0x40)));
        for c in all {
            assert_eq!(PanelCommand::decode(&c.encode()), Some(c));
        }
        assert_eq!(PanelCommand::KeyDown(PanelKey::PlusX).encode(), [0xA5, 0x50, 0x02]);
        assert_eq!(PanelCommand::decode(&[0x00]), None);
    }
}
