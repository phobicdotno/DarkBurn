//! The wire format of EzCad2 / BJJCZ "LMC" galvo boards.
//!
//! Every command is 12 bytes: six little-endian `u16` words, word 0 the opcode and words 1..=5
//! the arguments. Opcodes `>= 0x8000` are *list* commands. They are not sent one by one but
//! batched 256 to a 3072-byte packet and queued on the board; opcodes below `0x8000` are *single*
//! commands that are sent as a 12-byte write and answered with an 8-byte reply (four `u16`
//! words, word 3 is the status).
//!
//! Opcode names and numbers follow the MIT-licensed MeerK40t `balormk` driver (see
//! `ATTRIBUTION.md`).

use std::fmt;

/// Size of one command.
pub const COMMAND_LEN: usize = 12;
/// Commands per list packet.
pub const COMMANDS_PER_PACKET: usize = 256;
/// Size of one list packet.
pub const PACKET_LEN: usize = COMMAND_LEN * COMMANDS_PER_PACKET;
/// Size of a reply.
pub const REPLY_LEN: usize = 8;

/// Status bit: the board is executing a list.
pub const STATUS_BUSY: u16 = 0x04;
/// Status bit: the board can take another list packet.
pub const STATUS_READY: u16 = 0x20;
/// Status bit: an external axis is moving.
pub const STATUS_AXIS: u16 = 0x40;

/// Raw opcodes.
pub mod op {
    pub const LIST_JUMP_TO: u16 = 0x8001;
    pub const LIST_END_OF_LIST: u16 = 0x8002;
    pub const LIST_LASER_ON_POINT: u16 = 0x8003;
    pub const LIST_DELAY_TIME: u16 = 0x8004;
    pub const LIST_MARK_TO: u16 = 0x8005;
    pub const LIST_JUMP_SPEED: u16 = 0x8006;
    pub const LIST_LASER_ON_DELAY: u16 = 0x8007;
    pub const LIST_LASER_OFF_DELAY: u16 = 0x8008;
    pub const LIST_MARK_FREQ: u16 = 0x800A;
    pub const LIST_MARK_POWER_RATIO: u16 = 0x800B;
    pub const LIST_MARK_SPEED: u16 = 0x800C;
    pub const LIST_JUMP_DELAY: u16 = 0x800D;
    pub const LIST_POLYGON_DELAY: u16 = 0x800F;
    pub const LIST_WRITE_PORT: u16 = 0x8011;
    pub const LIST_MARK_CURRENT: u16 = 0x8012;
    pub const LIST_FLY_ENABLE: u16 = 0x801A;
    pub const LIST_QSWITCH_PERIOD: u16 = 0x801B;
    pub const LIST_SET_CO2_FPK: u16 = 0x801E;
    pub const LIST_FIBER_OPEN_MO: u16 = 0x8021;
    pub const LIST_WAIT_FOR_INPUT: u16 = 0x8022;
    pub const LIST_FIBER_YLPM_PULSE_WIDTH: u16 = 0x8026;
    pub const LIST_READY_MARK: u16 = 0x8051;

    pub const DISABLE_LASER: u16 = 0x0002;
    pub const ENABLE_LASER: u16 = 0x0004;
    pub const EXECUTE_LIST: u16 = 0x0005;
    pub const SET_PWM_PULSE_WIDTH: u16 = 0x0006;
    pub const GET_VERSION: u16 = 0x0007;
    pub const GET_SERIAL_NO: u16 = 0x0009;
    pub const GET_LIST_STATUS: u16 = 0x000A;
    pub const GET_POSITION_XY: u16 = 0x000C;
    pub const GOTO_XY: u16 = 0x000D;
    pub const LASER_SIGNAL_OFF: u16 = 0x000E;
    pub const LASER_SIGNAL_ON: u16 = 0x000F;
    pub const WRITE_COR_LINE: u16 = 0x0010;
    pub const RESET_LIST: u16 = 0x0012;
    pub const RESTART_LIST: u16 = 0x0013;
    pub const WRITE_COR_TABLE: u16 = 0x0015;
    pub const SET_CONTROL_MODE: u16 = 0x0016;
    pub const SET_DELAY_MODE: u16 = 0x0017;
    pub const SET_MAX_POLY_DELAY: u16 = 0x0018;
    pub const SET_END_OF_LIST: u16 = 0x0019;
    pub const SET_FIRST_PULSE_KILLER: u16 = 0x001A;
    pub const SET_LASER_MODE: u16 = 0x001B;
    pub const SET_TIMING: u16 = 0x001C;
    pub const SET_STANDBY: u16 = 0x001D;
    pub const SET_PWM_HALF_PERIOD: u16 = 0x001E;
    pub const STOP_EXECUTE: u16 = 0x001F;
    pub const STOP_LIST: u16 = 0x0020;
    pub const WRITE_PORT: u16 = 0x0021;
    pub const WRITE_ANALOG_PORT_1: u16 = 0x0022;
    pub const WRITE_ANALOG_PORT_2: u16 = 0x0023;
    pub const WRITE_ANALOG_PORT_X: u16 = 0x0024;
    pub const READ_PORT: u16 = 0x0025;
    pub const SET_AXIS_MOTION_PARAM: u16 = 0x0026;
    pub const SET_AXIS_ORIGIN_PARAM: u16 = 0x0027;
    pub const AXIS_GO_ORIGIN: u16 = 0x0028;
    pub const MOVE_AXIS_TO: u16 = 0x0029;
    pub const GET_AXIS_POS: u16 = 0x002A;
    pub const GET_MARK_COUNT: u16 = 0x002D;
    pub const SET_FPK_PARAM2: u16 = 0x002E;
    pub const FIBER_SET_MO: u16 = 0x0033;
    pub const DISABLE_Z: u16 = 0x0039;
    pub const ENABLE_Z: u16 = 0x003A;
    pub const SET_Z_DATA: u16 = 0x003B;
    pub const RESET: u16 = 0x0040;
    pub const GET_MARK_TIME: u16 = 0x0041;
    pub const INPUT_PORT: u16 = 0x0031;
    pub const SET_FLY_RES: u16 = 0x0032;
}

/// Decoding errors.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CodecError {
    /// A buffer had the wrong length.
    BadLength { expected: usize, got: usize },
}

impl fmt::Display for CodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadLength { expected, got } => write!(f, "expected {expected} bytes, got {got}"),
        }
    }
}
impl std::error::Error for CodecError {}

/// One 12-byte command: opcode and five argument words.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Command {
    pub op: u16,
    pub args: [u16; 5],
}

impl Command {
    pub const fn new(op: u16, args: [u16; 5]) -> Self {
        Self { op, args }
    }

    pub fn is_list(&self) -> bool {
        self.op >= 0x8000
    }

    pub fn to_bytes(&self) -> [u8; COMMAND_LEN] {
        let mut out = [0u8; COMMAND_LEN];
        out[0..2].copy_from_slice(&self.op.to_le_bytes());
        for (i, a) in self.args.iter().enumerate() {
            out[2 + i * 2..4 + i * 2].copy_from_slice(&a.to_le_bytes());
        }
        out
    }

    pub fn from_bytes(b: &[u8]) -> Result<Self, CodecError> {
        if b.len() != COMMAND_LEN {
            return Err(CodecError::BadLength { expected: COMMAND_LEN, got: b.len() });
        }
        let w = |i: usize| u16::from_le_bytes([b[i * 2], b[i * 2 + 1]]);
        Ok(Self { op: w(0), args: [w(1), w(2), w(3), w(4), w(5)] })
    }
}

/// A signed delay as the board takes it: magnitude in word 1, sign flag `0x8000` in word 2.
pub fn encode_signed(v: i32) -> (u16, u16) {
    (v.unsigned_abs().min(0xFFFF) as u16, if v < 0 { 0x8000 } else { 0 })
}

/// Inverse of [`encode_signed`].
pub fn decode_signed(mag: u16, sign: u16) -> i32 {
    if sign & 0x8000 != 0 { -(mag as i32) } else { mag as i32 }
}

/// A list command, queued on the board inside a [`ListPacket`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListCmd {
    /// Move with the laser off. `distance` is the move length in galvo units, capped at 0xFFFF.
    JumpTo {
        x: u16,
        y: u16,
        angle: u16,
        distance: u16,
    },
    /// Move with the laser on.
    MarkTo {
        x: u16,
        y: u16,
        angle: u16,
        distance: u16,
    },
    /// Fire in place for `dwell` units.
    LaserOnPoint {
        dwell: u16,
    },
    /// Wait, in 10 microsecond units.
    DelayTime {
        units: u16,
    },
    /// Jump speed in galvo units per millisecond.
    JumpSpeed(u16),
    /// Mark speed in galvo units per millisecond.
    MarkSpeed(u16),
    LaserOnDelay(i32),
    LaserOffDelay(i32),
    JumpDelay(i32),
    PolygonDelay(u16),
    /// CO2 / UV frequency word.
    MarkFreq(u16),
    /// CO2 / UV power ratio word.
    MarkPowerRatio(u16),
    /// Fibre current word (0..=0xFFF).
    MarkCurrent(u16),
    /// Fibre Q-switch period word (20000 / kHz).
    QSwitchPeriod(u16),
    /// Write the 16 output port bits.
    WritePort(u16),
    FiberOpenMo(u16),
    /// MOPA pulse width.
    FiberYlpmPulseWidth(u16),
    SetCo2Fpk {
        first: u16,
        second: u16,
    },
    FlyEnable(u16),
    WaitForInput {
        mask: u16,
        level: u16,
    },
    ReadyMark,
    EndOfList,
    /// An opcode this crate has no name for.
    Raw(Command),
}

impl ListCmd {
    pub fn to_command(&self) -> Command {
        use ListCmd::*;
        use op::*;
        let c = Command::new;
        match *self {
            JumpTo { x, y, angle, distance } => c(LIST_JUMP_TO, [x, y, angle, distance, 0]),
            MarkTo { x, y, angle, distance } => c(LIST_MARK_TO, [x, y, angle, distance, 0]),
            LaserOnPoint { dwell } => c(LIST_LASER_ON_POINT, [dwell, 0, 0, 0, 0]),
            DelayTime { units } => c(LIST_DELAY_TIME, [units, 0, 0, 0, 0]),
            JumpSpeed(v) => c(LIST_JUMP_SPEED, [v, 0, 0, 0, 0]),
            MarkSpeed(v) => c(LIST_MARK_SPEED, [v, 0, 0, 0, 0]),
            LaserOnDelay(v) => {
                let (m, s) = encode_signed(v);
                c(LIST_LASER_ON_DELAY, [m, s, 0, 0, 0])
            }
            LaserOffDelay(v) => {
                let (m, s) = encode_signed(v);
                c(LIST_LASER_OFF_DELAY, [m, s, 0, 0, 0])
            }
            JumpDelay(v) => {
                let (m, s) = encode_signed(v);
                c(LIST_JUMP_DELAY, [m, s, 0, 0, 0])
            }
            PolygonDelay(v) => c(LIST_POLYGON_DELAY, [v, 0, 0, 0, 0]),
            MarkFreq(v) => c(LIST_MARK_FREQ, [v, 0, 0, 0, 0]),
            MarkPowerRatio(v) => c(LIST_MARK_POWER_RATIO, [v, 0, 0, 0, 0]),
            MarkCurrent(v) => c(LIST_MARK_CURRENT, [v, 0, 0, 0, 0]),
            QSwitchPeriod(v) => c(LIST_QSWITCH_PERIOD, [v, 0, 0, 0, 0]),
            WritePort(v) => c(LIST_WRITE_PORT, [v, 0, 0, 0, 0]),
            FiberOpenMo(v) => c(LIST_FIBER_OPEN_MO, [v, 0, 0, 0, 0]),
            FiberYlpmPulseWidth(v) => c(LIST_FIBER_YLPM_PULSE_WIDTH, [v, 0, 0, 0, 0]),
            SetCo2Fpk { first, second } => c(LIST_SET_CO2_FPK, [first, second, 0, 0, 0]),
            FlyEnable(v) => c(LIST_FLY_ENABLE, [v, 0, 0, 0, 0]),
            WaitForInput { mask, level } => c(LIST_WAIT_FOR_INPUT, [mask, level, 0, 0, 0]),
            ReadyMark => c(LIST_READY_MARK, [0; 5]),
            EndOfList => c(LIST_END_OF_LIST, [0; 5]),
            Raw(cmd) => cmd,
        }
    }

    pub fn from_command(cmd: Command) -> Self {
        use ListCmd::*;
        use op::*;
        let a = cmd.args;
        match cmd.op {
            LIST_JUMP_TO => JumpTo { x: a[0], y: a[1], angle: a[2], distance: a[3] },
            LIST_MARK_TO => MarkTo { x: a[0], y: a[1], angle: a[2], distance: a[3] },
            LIST_LASER_ON_POINT => LaserOnPoint { dwell: a[0] },
            LIST_DELAY_TIME => DelayTime { units: a[0] },
            LIST_JUMP_SPEED => JumpSpeed(a[0]),
            LIST_MARK_SPEED => MarkSpeed(a[0]),
            LIST_LASER_ON_DELAY => LaserOnDelay(decode_signed(a[0], a[1])),
            LIST_LASER_OFF_DELAY => LaserOffDelay(decode_signed(a[0], a[1])),
            LIST_JUMP_DELAY => JumpDelay(decode_signed(a[0], a[1])),
            LIST_POLYGON_DELAY => PolygonDelay(a[0]),
            LIST_MARK_FREQ => MarkFreq(a[0]),
            LIST_MARK_POWER_RATIO => MarkPowerRatio(a[0]),
            LIST_MARK_CURRENT => MarkCurrent(a[0]),
            LIST_QSWITCH_PERIOD => QSwitchPeriod(a[0]),
            LIST_WRITE_PORT => WritePort(a[0]),
            LIST_FIBER_OPEN_MO => FiberOpenMo(a[0]),
            LIST_FIBER_YLPM_PULSE_WIDTH => FiberYlpmPulseWidth(a[0]),
            LIST_SET_CO2_FPK => SetCo2Fpk { first: a[0], second: a[1] },
            LIST_FLY_ENABLE => FlyEnable(a[0]),
            LIST_WAIT_FOR_INPUT => WaitForInput { mask: a[0], level: a[1] },
            LIST_READY_MARK => ReadyMark,
            LIST_END_OF_LIST => EndOfList,
            _ => Raw(cmd),
        }
    }

    /// True when the command can make the laser emit (mark or fire).
    pub fn emits(&self) -> bool {
        matches!(self, ListCmd::MarkTo { .. } | ListCmd::LaserOnPoint { .. })
    }
}

/// A single (immediate) command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SingleCmd {
    DisableLaser,
    EnableLaser,
    ExecuteList,
    SetPwmPulseWidth(u16),
    GetVersion,
    GetSerialNo,
    GetListStatus,
    GetPositionXy,
    GotoXy {
        x: u16,
        y: u16,
        angle: u16,
        distance: u16,
    },
    LaserSignalOff,
    LaserSignalOn,
    /// One row of the 65x65 lens table. `non_first` is 0 for the first row, 1 for the rest.
    WriteCorLine {
        dx: u16,
        dy: u16,
        non_first: u16,
    },
    ResetList,
    RestartList,
    /// `true` starts a table upload, `false` selects the blank table.
    WriteCorTable(bool),
    SetControlMode(u16),
    SetDelayMode(u16),
    SetMaxPolyDelay(i32),
    SetEndOfList(u16),
    SetFirstPulseKiller(u16),
    SetLaserMode(u16),
    SetTiming(u16),
    SetStandby(u16, u16),
    SetPwmHalfPeriod(u16),
    StopExecute,
    StopList,
    WritePort(u16),
    WriteAnalogPort1(u16),
    WriteAnalogPort2(u16),
    WriteAnalogPortX(u16),
    ReadPort,
    SetAxisMotionParam([u16; 5]),
    SetAxisOriginParam([u16; 5]),
    AxisGoOrigin,
    MoveAxisTo {
        position: u16,
        invert: u16,
    },
    GetAxisPos(u16),
    GetMarkCount,
    SetFpkParam2([u16; 4]),
    FiberSetMo(bool),
    EnableZ,
    DisableZ,
    SetZData(u16),
    SetFlyRes([u16; 4]),
    InputPort(u16),
    Reset,
    GetMarkTime,
    Raw(Command),
}

fn p4(v: [u16; 4]) -> [u16; 5] {
    [v[0], v[1], v[2], v[3], 0]
}

impl SingleCmd {
    pub fn to_command(&self) -> Command {
        use SingleCmd::*;
        use op::*;
        let c = |o, a: [u16; 5]| Command::new(o, a);
        let a1 = |o, v| Command::new(o, [v, 0, 0, 0, 0]);
        let n = |o| Command::new(o, [0; 5]);
        match *self {
            DisableLaser => n(DISABLE_LASER),
            EnableLaser => n(ENABLE_LASER),
            ExecuteList => n(EXECUTE_LIST),
            SetPwmPulseWidth(v) => a1(SET_PWM_PULSE_WIDTH, v),
            GetVersion => n(GET_VERSION),
            GetSerialNo => n(GET_SERIAL_NO),
            GetListStatus => n(GET_LIST_STATUS),
            GetPositionXy => n(GET_POSITION_XY),
            GotoXy { x, y, angle, distance } => c(GOTO_XY, [x, y, angle, distance, 0]),
            LaserSignalOff => n(LASER_SIGNAL_OFF),
            LaserSignalOn => n(LASER_SIGNAL_ON),
            WriteCorLine { dx, dy, non_first } => c(WRITE_COR_LINE, [dx, dy, non_first, 0, 0]),
            ResetList => n(RESET_LIST),
            RestartList => n(RESTART_LIST),
            WriteCorTable(v) => a1(WRITE_COR_TABLE, v as u16),
            SetControlMode(v) => a1(SET_CONTROL_MODE, v),
            SetDelayMode(v) => a1(SET_DELAY_MODE, v),
            SetMaxPolyDelay(v) => {
                let (m, s) = encode_signed(v);
                c(SET_MAX_POLY_DELAY, [m, s, 0, 0, 0])
            }
            SetEndOfList(v) => a1(SET_END_OF_LIST, v),
            SetFirstPulseKiller(v) => a1(SET_FIRST_PULSE_KILLER, v),
            SetLaserMode(v) => a1(SET_LASER_MODE, v),
            SetTiming(v) => a1(SET_TIMING, v),
            SetStandby(a, b) => c(SET_STANDBY, [a, b, 0, 0, 0]),
            SetPwmHalfPeriod(v) => a1(SET_PWM_HALF_PERIOD, v),
            StopExecute => n(STOP_EXECUTE),
            StopList => n(STOP_LIST),
            WritePort(v) => a1(WRITE_PORT, v),
            WriteAnalogPort1(v) => a1(WRITE_ANALOG_PORT_1, v),
            WriteAnalogPort2(v) => a1(WRITE_ANALOG_PORT_2, v),
            WriteAnalogPortX(v) => a1(WRITE_ANALOG_PORT_X, v),
            ReadPort => n(READ_PORT),
            SetAxisMotionParam(p) => c(SET_AXIS_MOTION_PARAM, p),
            SetAxisOriginParam(p) => c(SET_AXIS_ORIGIN_PARAM, p),
            AxisGoOrigin => n(AXIS_GO_ORIGIN),
            MoveAxisTo { position, invert } => c(MOVE_AXIS_TO, [position, invert, 0, 0, 0]),
            GetAxisPos(i) => a1(GET_AXIS_POS, i),
            GetMarkCount => n(GET_MARK_COUNT),
            SetFpkParam2(p) => c(SET_FPK_PARAM2, p4(p)),
            FiberSetMo(on) => a1(FIBER_SET_MO, on as u16),
            EnableZ => n(ENABLE_Z),
            DisableZ => n(DISABLE_Z),
            SetZData(v) => a1(SET_Z_DATA, v),
            SetFlyRes(p) => c(SET_FLY_RES, p4(p)),
            InputPort(v) => a1(INPUT_PORT, v),
            Reset => n(RESET),
            GetMarkTime => a1(GET_MARK_TIME, 3),
            Raw(cmd) => cmd,
        }
    }

    pub fn from_command(cmd: Command) -> Self {
        use SingleCmd::*;
        use op::*;
        let a = cmd.args;
        match cmd.op {
            DISABLE_LASER => DisableLaser,
            ENABLE_LASER => EnableLaser,
            EXECUTE_LIST => ExecuteList,
            SET_PWM_PULSE_WIDTH => SetPwmPulseWidth(a[0]),
            GET_VERSION => GetVersion,
            GET_SERIAL_NO => GetSerialNo,
            GET_LIST_STATUS => GetListStatus,
            GET_POSITION_XY => GetPositionXy,
            GOTO_XY => GotoXy { x: a[0], y: a[1], angle: a[2], distance: a[3] },
            LASER_SIGNAL_OFF => LaserSignalOff,
            LASER_SIGNAL_ON => LaserSignalOn,
            WRITE_COR_LINE => WriteCorLine { dx: a[0], dy: a[1], non_first: a[2] },
            RESET_LIST => ResetList,
            RESTART_LIST => RestartList,
            WRITE_COR_TABLE => WriteCorTable(a[0] != 0),
            SET_CONTROL_MODE => SetControlMode(a[0]),
            SET_DELAY_MODE => SetDelayMode(a[0]),
            SET_MAX_POLY_DELAY => SetMaxPolyDelay(decode_signed(a[0], a[1])),
            SET_END_OF_LIST => SetEndOfList(a[0]),
            SET_FIRST_PULSE_KILLER => SetFirstPulseKiller(a[0]),
            SET_LASER_MODE => SetLaserMode(a[0]),
            SET_TIMING => SetTiming(a[0]),
            SET_STANDBY => SetStandby(a[0], a[1]),
            SET_PWM_HALF_PERIOD => SetPwmHalfPeriod(a[0]),
            STOP_EXECUTE => StopExecute,
            STOP_LIST => StopList,
            WRITE_PORT => WritePort(a[0]),
            WRITE_ANALOG_PORT_1 => WriteAnalogPort1(a[0]),
            WRITE_ANALOG_PORT_2 => WriteAnalogPort2(a[0]),
            WRITE_ANALOG_PORT_X => WriteAnalogPortX(a[0]),
            READ_PORT => ReadPort,
            SET_AXIS_MOTION_PARAM => SetAxisMotionParam(a),
            SET_AXIS_ORIGIN_PARAM => SetAxisOriginParam(a),
            AXIS_GO_ORIGIN => AxisGoOrigin,
            MOVE_AXIS_TO => MoveAxisTo { position: a[0], invert: a[1] },
            GET_AXIS_POS => GetAxisPos(a[0]),
            GET_MARK_COUNT => GetMarkCount,
            SET_FPK_PARAM2 => SetFpkParam2([a[0], a[1], a[2], a[3]]),
            FIBER_SET_MO => FiberSetMo(a[0] != 0),
            ENABLE_Z => EnableZ,
            DISABLE_Z => DisableZ,
            SET_Z_DATA => SetZData(a[0]),
            SET_FLY_RES => SetFlyRes([a[0], a[1], a[2], a[3]]),
            INPUT_PORT => InputPort(a[0]),
            RESET => Reset,
            GET_MARK_TIME => GetMarkTime,
            _ => Raw(cmd),
        }
    }
}

/// The 8-byte answer to a single command.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Reply {
    pub words: [u16; 4],
}

impl Reply {
    pub fn from_bytes(b: &[u8]) -> Result<Self, CodecError> {
        if b.len() != REPLY_LEN {
            return Err(CodecError::BadLength { expected: REPLY_LEN, got: b.len() });
        }
        let w = |i: usize| u16::from_le_bytes([b[i * 2], b[i * 2 + 1]]);
        Ok(Self { words: [w(0), w(1), w(2), w(3)] })
    }

    pub fn to_bytes(&self) -> [u8; REPLY_LEN] {
        let mut out = [0u8; REPLY_LEN];
        for (i, w) in self.words.iter().enumerate() {
            out[i * 2..i * 2 + 2].copy_from_slice(&w.to_le_bytes());
        }
        out
    }

    /// Word 3.
    pub fn status(&self) -> u16 {
        self.words[3]
    }
    pub fn is_busy(&self) -> bool {
        self.status() & STATUS_BUSY != 0
    }
    pub fn is_ready(&self) -> bool {
        self.status() & STATUS_READY != 0
    }
    pub fn axis_moving(&self) -> bool {
        self.status() & STATUS_AXIS != 0
    }
}

/// One 3072-byte list packet.
#[derive(Clone, PartialEq, Eq)]
pub struct ListPacket(Vec<u8>);

impl fmt::Debug for ListPacket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ListPacket({} commands)", self.payload().len())
    }
}

impl ListPacket {
    /// Builds a packet from up to 256 commands; the unused tail is filled with `EndOfList`
    /// no-ops, as the vendor software does.
    pub fn new(cmds: &[ListCmd]) -> Self {
        assert!(cmds.len() <= COMMANDS_PER_PACKET);
        let mut buf = Vec::with_capacity(PACKET_LEN);
        for c in cmds {
            buf.extend_from_slice(&c.to_command().to_bytes());
        }
        let pad = ListCmd::EndOfList.to_command().to_bytes();
        while buf.len() < PACKET_LEN {
            buf.extend_from_slice(&pad);
        }
        Self(buf)
    }

    pub fn from_bytes(b: &[u8]) -> Result<Self, CodecError> {
        if b.len() != PACKET_LEN {
            return Err(CodecError::BadLength { expected: PACKET_LEN, got: b.len() });
        }
        Ok(Self(b.to_vec()))
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// All 256 slots, padding included.
    pub fn commands(&self) -> Vec<ListCmd> {
        self.0.chunks_exact(COMMAND_LEN).map(|c| ListCmd::from_command(Command::from_bytes(c).expect("12 bytes"))).collect()
    }

    /// The commands without the trailing `EndOfList` padding.
    pub fn payload(&self) -> Vec<ListCmd> {
        let mut v = self.commands();
        while v.last() == Some(&ListCmd::EndOfList) {
            v.pop();
        }
        v
    }
}

/// Splits a command stream into packets (256 commands each). An `EndOfList` is always appended
/// so the board knows the list is over.
pub fn packetize(cmds: &[ListCmd]) -> Vec<ListPacket> {
    let mut all: Vec<ListCmd> = cmds.to_vec();
    all.push(ListCmd::EndOfList);
    all.chunks(COMMANDS_PER_PACKET).map(ListPacket::new).collect()
}

/// Flattens packets back into commands, dropping all trailing `EndOfList` padding.
pub fn unpacketize(packets: &[ListPacket]) -> Vec<ListCmd> {
    let mut out = Vec::new();
    for p in packets {
        out.extend(p.commands());
    }
    while out.last() == Some(&ListCmd::EndOfList) {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_bytes_are_little_endian() {
        let c = Command::new(0x8005, [0x1234, 0x5678, 0, 0xFFFF, 1]);
        let b = c.to_bytes();
        assert_eq!(b, [0x05, 0x80, 0x34, 0x12, 0x78, 0x56, 0, 0, 0xFF, 0xFF, 1, 0]);
        assert_eq!(Command::from_bytes(&b).unwrap(), c);
        assert!(Command::from_bytes(&b[..11]).is_err());
    }

    #[test]
    fn list_commands_round_trip() {
        let all = [
            ListCmd::JumpTo { x: 1, y: 2, angle: 3, distance: 4 },
            ListCmd::MarkTo { x: 65535, y: 0, angle: 0, distance: 65535 },
            ListCmd::LaserOnPoint { dwell: 9 },
            ListCmd::DelayTime { units: 30 },
            ListCmd::JumpSpeed(200),
            ListCmd::MarkSpeed(300),
            ListCmd::LaserOnDelay(100),
            ListCmd::LaserOffDelay(-50),
            ListCmd::JumpDelay(8),
            ListCmd::PolygonDelay(10),
            ListCmd::MarkFreq(333),
            ListCmd::MarkPowerRatio(60),
            ListCmd::MarkCurrent(0x800),
            ListCmd::QSwitchPeriod(667),
            ListCmd::WritePort(0x0101),
            ListCmd::FiberOpenMo(1),
            ListCmd::FiberYlpmPulseWidth(4),
            ListCmd::SetCo2Fpk { first: 5, second: 6 },
            ListCmd::FlyEnable(1),
            ListCmd::WaitForInput { mask: 0x8000, level: 0 },
            ListCmd::ReadyMark,
            ListCmd::EndOfList,
        ];
        for c in all {
            let cmd = c.to_command();
            assert!(cmd.is_list(), "{c:?}");
            assert_eq!(ListCmd::from_command(Command::from_bytes(&cmd.to_bytes()).unwrap()), c);
        }
        let raw = ListCmd::from_command(Command::new(0x8999, [1, 2, 3, 4, 5]));
        assert!(matches!(raw, ListCmd::Raw(_)));
    }

    #[test]
    fn single_commands_round_trip() {
        let all = [
            SingleCmd::DisableLaser,
            SingleCmd::EnableLaser,
            SingleCmd::ExecuteList,
            SingleCmd::SetPwmPulseWidth(125),
            SingleCmd::GetVersion,
            SingleCmd::GetSerialNo,
            SingleCmd::GetListStatus,
            SingleCmd::GetPositionXy,
            SingleCmd::GotoXy { x: 0x8000, y: 0x8000, angle: 0, distance: 7 },
            SingleCmd::LaserSignalOff,
            SingleCmd::LaserSignalOn,
            SingleCmd::WriteCorLine { dx: 1, dy: 0x8002, non_first: 1 },
            SingleCmd::ResetList,
            SingleCmd::RestartList,
            SingleCmd::WriteCorTable(true),
            SingleCmd::WriteCorTable(false),
            SingleCmd::SetControlMode(0),
            SingleCmd::SetDelayMode(1),
            SingleCmd::SetMaxPolyDelay(-3),
            SingleCmd::SetEndOfList(0),
            SingleCmd::SetFirstPulseKiller(200),
            SingleCmd::SetLaserMode(1),
            SingleCmd::SetTiming(1),
            SingleCmd::SetStandby(2000, 20),
            SingleCmd::SetPwmHalfPeriod(125),
            SingleCmd::StopExecute,
            SingleCmd::StopList,
            SingleCmd::WritePort(0x100),
            SingleCmd::WriteAnalogPort1(0x7FF),
            SingleCmd::WriteAnalogPort2(1),
            SingleCmd::WriteAnalogPortX(2),
            SingleCmd::ReadPort,
            SingleCmd::SetAxisMotionParam([1, 2, 3, 4, 5]),
            SingleCmd::SetAxisOriginParam([5, 4, 3, 2, 1]),
            SingleCmd::AxisGoOrigin,
            SingleCmd::MoveAxisTo { position: 1000, invert: 1 },
            SingleCmd::GetAxisPos(0),
            SingleCmd::GetMarkCount,
            SingleCmd::SetFpkParam2([0xFFB, 1, 409, 100]),
            SingleCmd::FiberSetMo(true),
            SingleCmd::FiberSetMo(false),
            SingleCmd::EnableZ,
            SingleCmd::DisableZ,
            SingleCmd::SetZData(5),
            SingleCmd::SetFlyRes([0, 99, 1000, 25]),
            SingleCmd::InputPort(4),
            SingleCmd::Reset,
            SingleCmd::GetMarkTime,
        ];
        for c in all {
            let cmd = c.to_command();
            assert!(!cmd.is_list(), "{c:?}");
            assert_eq!(SingleCmd::from_command(Command::from_bytes(&cmd.to_bytes()).unwrap()), c);
        }
        assert!(matches!(SingleCmd::from_command(Command::new(0x0999, [0; 5])), SingleCmd::Raw(_)));
    }

    #[test]
    fn signed_encoding() {
        assert_eq!(encode_signed(-5), (5, 0x8000));
        assert_eq!(encode_signed(70000), (0xFFFF, 0));
        assert_eq!(decode_signed(5, 0x8000), -5);
    }

    #[test]
    fn reply_status_bits() {
        let r = Reply::from_bytes(&[7, 0, 0, 0, 0, 0, 0x24, 0]).unwrap();
        assert_eq!(r.words, [7, 0, 0, 0x24]);
        assert!(r.is_busy() && r.is_ready() && !r.axis_moving());
        assert_eq!(Reply::from_bytes(&r.to_bytes()).unwrap(), r);
        assert!(Reply::from_bytes(&[0; 7]).is_err());
    }

    #[test]
    fn packets_are_3072_bytes_and_round_trip() {
        let mut cmds = Vec::new();
        for i in 0..600u16 {
            cmds.push(ListCmd::MarkTo { x: i, y: i * 2, angle: 0, distance: 1 });
        }
        let packets = packetize(&cmds);
        assert_eq!(packets.len(), 3);
        for p in &packets {
            assert_eq!(p.as_bytes().len(), 3072);
            assert_eq!(ListPacket::from_bytes(p.as_bytes()).unwrap(), *p);
        }
        assert_eq!(unpacketize(&packets), cmds);
        assert!(ListPacket::from_bytes(&[0; 100]).is_err());
    }

    #[test]
    fn exact_multiple_still_ends_the_list() {
        let cmds = vec![ListCmd::ReadyMark; 256];
        let packets = packetize(&cmds);
        assert_eq!(packets.len(), 2);
        assert_eq!(packets[1].commands()[0], ListCmd::EndOfList);
    }
}
