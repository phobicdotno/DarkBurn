//! Bug hunt 10 (drivers): wire format edges of the Ruida driver. Emulator only, no network, no USB.

mod common;

use common::*;
use darkburn_ruida::settings::addr;
use darkburn_ruida::*;
use kurbo::Point;

// ---- the firmware version is text of any length ---------------------------------------------

/// A version of four characters plus its NUL is five bytes long, as long as a number; one of
/// nine characters is as long as a setting write. Both are still text.
#[test]
fn a_firmware_version_of_any_length_reads_as_text() {
    for firmware in ["V8.1", "V8.01.70a", "RDC6442S-EMU", "V", ""] {
        let ecfg = EmulatorConfig { firmware: firmware.into(), ..Default::default() };
        let mcfg = MachineConfig { timeout: std::time::Duration::from_millis(50), ..Default::default() };
        let (m, _emu) = udp_machine_with(ecfg.clone(), mcfg.clone());
        assert_eq!(m.info().unwrap().firmware, firmware, "over UDP");

        let emu = emulator(ecfg);
        let mut m = RuidaMachine::new(MemoryTransport::stream(emu), mcfg);
        m.connect().unwrap();
        assert_eq!(m.info().unwrap().firmware, firmware, "over USB");
    }
}

#[test]
fn a_firmware_reply_decodes_as_text_whatever_its_length() {
    for text in ["V8.1", "V8.01.70a"] {
        let reply = Command::SettingReplyRaw { addr: addr::FIRMWARE, data: encoding::encode_cstring(text) };
        let bytes = reply.encode().unwrap();
        assert_eq!(Command::decode(&bytes), reply);
    }
    // Numbers stay numbers.
    let reply = Command::SettingReply { addr: addr::BED_X, value: 600_000 };
    assert_eq!(Command::decode(&reply.encode().unwrap()), reply);
}

// ---- numbers that do not fit their 14-bit field ----------------------------------------------

/// File 16384 is 0 in 14 bits, and "delete files 0 to 0" means all of them.
#[test]
fn a_file_number_beyond_14_bits_does_not_delete_every_file() {
    let (mut m, emu) = udp_machine();
    for name in ["A", "B"] {
        m.send_job(name, &square_job((5.0, 5.0), 10.0, 50.0), &SendOptions::default(), &mut never).unwrap();
    }
    assert!(matches!(m.delete_files(&[16384]), Err(RuidaError::Encode(_))));
    assert_eq!(m.list_files().unwrap().len(), 2, "the files are still there");
    // File 16385 is not file 1.
    let moves = lock(&emu).motion.len();
    assert!(matches!(m.run_file(16385), Err(RuidaError::Encode(_))));
    assert_eq!(lock(&emu).motion.len(), moves, "nothing ran");
    assert_eq!(lock(&emu).files().len(), 2);
}

#[test]
fn values_beyond_14_bits_are_refused_by_the_encoder() {
    for cmd in [
        Command::DeleteFiles { first: 0x4000, last: 0x4000 },
        Command::DeleteFiles { first: 1, last: 0x4001 },
        Command::RunFile(0x4001),
        Command::GetFileName(0x4001),
        Command::CalcFileTime(0xFFFF),
        Command::UFileId(0x4000),
        Command::GetSetting { addr: 0x4000 | addr::BED_X },
        Command::SetSetting { addr: 0x4000 | addr::BED_X, value: 1, value2: 1 },
        Command::ProcessRepeat([1, 0x4001, 0, 0, 0, 0, 0]),
        Command::ArrayRepeat([0x8000, 1, 0, 0, 0, 0, 0]),
    ] {
        assert!(matches!(cmd.encode(), Err(RuidaError::Encode(_))), "{cmd:?} was encoded as {:02X?}", cmd.encode());
    }
    // The largest value that fits still goes through.
    assert_eq!(Command::RunFile(0x3FFF).encode().unwrap(), [0xE8, 0x03, 0x7F, 0x7F]);
}

/// Address 0x4026 is not the bed width at 0x0026.
#[test]
fn a_setting_address_beyond_14_bits_is_not_another_setting() {
    let (mut m, _emu) = udp_machine();
    assert_eq!(m.read_setting(addr::BED_X).unwrap(), 600_000);
    // Refused at once, not asked for under another address and then waited for.
    assert!(matches!(m.read_setting(0x4000 | addr::BED_X), Err(RuidaError::Encode(_))));
    assert!(matches!(m.calc_time(u16::MAX - addr::FILE_TIME_BASE), Err(RuidaError::Encode(_))));
}

// ---- jobs with numbers that cannot be sent ----------------------------------------------------

fn job_with(ops: Vec<RdOp>) -> RdJob {
    RdJob { reference: RefPoint::MachineZero, layers: vec![RdLayer { settings: LayerSettings::default(), ops }] }
}

/// "Not a number" became 0 µm, infinity became 2147 m, and both were written into the job.
#[test]
fn a_job_with_a_coordinate_that_is_not_a_number_is_refused() {
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 3.0e6, -3.0e6] {
        for ops in [
            vec![RdOp::Move(Point::new(bad, 10.0)), RdOp::Cut { to: Point::new(20.0, 10.0), power: None, speed: None }],
            vec![RdOp::Move(Point::new(10.0, 10.0)), RdOp::Cut { to: Point::new(20.0, bad), power: None, speed: None }],
            vec![RdOp::Z(bad), RdOp::Move(Point::new(10.0, 10.0))],
            vec![RdOp::U(bad), RdOp::Move(Point::new(10.0, 10.0))],
        ] {
            let job = job_with(ops);
            assert!(matches!(job.to_commands(), Err(RuidaError::Encode(_))), "{bad}: {:?}", job.layers[0].ops);
            assert!(job.to_rd_file(Magic::DEFAULT).is_err());
        }
    }
    // Nothing reaches the controller either.
    let (mut m, emu) = udp_machine();
    let job = job_with(vec![RdOp::Move(Point::new(10.0, 10.0)), RdOp::Cut { to: Point::new(f64::NAN, 10.0), power: None, speed: None }]);
    assert!(m.send_job("BAD", &job, &SendOptions::default(), &mut never).is_err());
    assert!(m.run_job_now(&job).is_err());
    assert!(lock(&emu).files().is_empty());
    assert!(lock(&emu).motion.is_empty());
}

#[test]
fn a_job_with_a_speed_that_is_not_a_number_is_refused() {
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let mut job = square_job((5.0, 5.0), 10.0, bad);
        assert!(matches!(job.to_commands(), Err(RuidaError::Encode(_))), "layer speed {bad}");
        job = square_job((5.0, 5.0), 10.0, 50.0);
        job.layers[0].ops.push(RdOp::Cut { to: Point::new(1.0, 1.0), power: None, speed: Some(bad) });
        assert!(matches!(job.to_commands(), Err(RuidaError::Encode(_))), "cut speed {bad}");
        job = square_job((5.0, 5.0), 10.0, 50.0);
        job.layers[0].settings.dot_mode = Some(DotMode { dot_time_ms: bad, spacing_mm: 0.5 });
        assert!(matches!(job.to_commands(), Err(RuidaError::Encode(_))), "dot time {bad}");
        job.layers[0].settings.dot_mode = Some(DotMode { dot_time_ms: 1.0, spacing_mm: bad });
        assert!(matches!(job.to_commands(), Err(RuidaError::Encode(_))), "dot spacing {bad}");
    }
}

/// Two points at opposite ends of the number range are 4295 km apart, not 1 µm.
#[test]
fn a_cut_across_the_whole_number_range_is_not_a_short_relative_cut() {
    let far = 2_147_483.0;
    let job = job_with(vec![RdOp::Move(Point::new(far, 0.0)), RdOp::Cut { to: Point::new(-far, 0.0), power: None, speed: None }]);
    let cmds = job.to_commands().unwrap();
    assert!(cmds.contains(&Command::CutAbs { x: -2_147_483_000, y: 0 }), "{cmds:?}");
}

/// A damaged file: a relative move from the edge of the number range.
#[test]
fn a_file_whose_relative_move_leaves_the_number_range_is_refused() {
    for step in [Command::MoveRelX(8191), Command::CutRelX(8191), Command::MoveRel { dx: 5, dy: 5 }, Command::CutRel { dx: 1, dy: 1 }] {
        let cmds = [Command::LayerNumber(0), Command::LaserTubeStart(1), Command::MoveAbs { x: i32::MAX, y: i32::MAX }, step, Command::EndOfFile];
        assert!(matches!(RdJob::from_commands(&cmds), Err(RuidaError::Decode(_))));
        let bytes = command::encode_commands(&cmds).unwrap();
        assert!(RdJob::from_logical(&bytes).is_err());
    }
    for step in [Command::MoveRelY(-8192), Command::CutRelY(-1)] {
        let cmds = [Command::LayerNumber(0), Command::MoveAbs { x: i32::MIN, y: i32::MIN }, step, Command::EndOfFile];
        assert!(matches!(RdJob::from_commands(&cmds), Err(RuidaError::Decode(_))));
    }
}

// ---- file names the controller already holds --------------------------------------------------

/// A file that some other program named with the largest 32-bit number.
#[test]
fn a_stored_file_with_a_huge_number_for_a_name_does_not_break_numbering() {
    assert_eq!(machine::next_name("JOB", &["4294967295"], 8), "JOB1");
    assert_eq!(machine::next_name("JOB", &["JOB4294967295", "JOB7"], 16), "JOB8");
    let (mut m, emu) = udp_machine();
    let job = square_job((5.0, 5.0), 10.0, 50.0);
    lock(&emu).add_file("4294967295", job.to_logical().unwrap());
    let out = m.send_job("JOB", &job, &SendOptions { auto_increment: true, ..Default::default() }, &mut never).unwrap();
    assert_eq!(out.name, "JOB1");
}

// ---- a reply that comes twice -----------------------------------------------------------------

/// What happens to the answers of one outgoing datagram.
#[derive(Clone, Copy, PartialEq)]
enum Fate {
    Pass,
    /// The controller's answers arrive only after the next datagram has been sent.
    Late,
}

/// A link to the emulator that treats the n-th outgoing datagram as scripted.
struct Scripted {
    inner: MemoryTransport,
    script: Vec<Fate>,
    sent: usize,
    held: Vec<Vec<u8>>,
    ready: std::collections::VecDeque<Vec<u8>>,
}

impl Transport for Scripted {
    fn framing(&self) -> Framing {
        Framing::Udp
    }
    fn send(&mut self, channel: Channel, data: &[u8]) -> Result<()> {
        let fate = self.script.get(self.sent).copied().unwrap_or(Fate::Pass);
        self.sent += 1;
        self.ready.extend(self.held.drain(..));
        self.inner.send(channel, data)?;
        while let Some(reply) = self.inner.recv(channel, std::time::Duration::ZERO)? {
            if fate == Fate::Late { self.held.push(reply) } else { self.ready.push_back(reply) }
        }
        Ok(())
    }
    fn recv(&mut self, _channel: Channel, _timeout: std::time::Duration) -> Result<Option<Vec<u8>>> {
        Ok(self.ready.pop_front())
    }
    fn describe(&self) -> String {
        "scripted".into()
    }
}

/// The question is asked twice because its answer was late; both copies get answered. The
/// second answer must not pass for the answer to the next question about the same setting.
#[test]
fn a_reply_that_comes_twice_does_not_put_every_later_read_one_behind() {
    let emu = emulator(EmulatorConfig::default());
    let link = Scripted { inner: MemoryTransport::udp(emu.clone()), script: Vec::new(), sent: 0, held: Vec::new(), ready: Default::default() };
    let mut m = RuidaMachine::new(link, MachineConfig::default());
    m.connect().unwrap();
    m.write_setting(addr::G0_VELOCITY, 100_000).unwrap();
    let n = m.transport().sent;
    m.transport_mut().script = [vec![Fate::Pass; n], vec![Fate::Late]].concat();
    assert_eq!(m.read_setting(addr::G0_VELOCITY).unwrap(), 100_000);
    assert_eq!(m.resends, 1, "the question was asked twice");
    for speed in [200_000, 300_000, 400_000] {
        m.write_setting(addr::G0_VELOCITY, speed).unwrap();
        assert_eq!(m.read_setting(addr::G0_VELOCITY).unwrap(), speed, "the read after the write shows the value before it");
    }
}

// ---- second pass: odd input into the readers, and encode-then-decode -------------------------

/// Small deterministic generator, so a failure can be repeated.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

/// A real job file cut off at every byte, and with every single byte changed in turn. The
/// reader may refuse it or read something, but it must not stop the program; and what it reads
/// can be written again or is refused, never a crash.
#[test]
fn a_job_file_cut_off_or_damaged_anywhere_never_stops_the_program() {
    let logical = mixed_job().to_logical().unwrap();
    let mut seen_ok = 0;
    let mut check = |bytes: &[u8]| {
        if let Ok(job) = RdJob::from_logical(bytes) {
            seen_ok += 1;
            let _ = job.to_logical();
            let _ = job.bounds_mm();
            let _ = job.quantized();
        }
        let _ = command::parse_commands(bytes);
        let _ = command::split_commands(bytes);
    };
    for cut in 0..=logical.len() {
        check(&logical[..cut]);
        check(&logical[cut..]);
    }
    let mut r = Rng(0x1234_5678_9ABC_DEF1);
    for at in 0..logical.len() {
        for v in [0x00, 0x7F, 0x80, 0xFF, 0x88, 0x89, 0xA8, 0xA9, 0xCA, 0xD7, 0xDA, 0xE7, 0xE8, r.next() as u8] {
            let mut b = logical.clone();
            b[at] = v;
            check(&b);
        }
    }
    // The same through the swizzled file, with every magic.
    let file = mixed_job().to_rd_file(Magic::DEFAULT).unwrap();
    for m in 0..=255u8 {
        let _ = RdJob::from_rd_file(&file, Some(Magic(m)));
    }
    for cut in (0..file.len()).step_by(7) {
        let _ = RdJob::from_rd_file(&file[..cut], None);
    }
    assert!(seen_ok > 0);
}

/// Every command with the smallest and the largest value of each field: what is encoded reads
/// back as the same command (or is refused by the encoder).
#[test]
fn commands_with_extreme_values_read_back_as_written() {
    use Command::*;
    let ints = [i32::MIN, i32::MIN + 1, -8192, -1, 0, 1, 8191, i32::MAX];
    let uints = [0u32, 1, 0x7F, 0x80, 0x3FFF, 0x4000, 0x0FFF_FFFF, 0x1000_0000, 0x7FFF_FFFF, 0x8000_0000, u32::MAX];
    let mut cmds: Vec<Command> = Vec::new();
    for &a in &ints {
        for &b in &[i32::MIN, 0, i32::MAX] {
            cmds.extend([
                MoveAbs { x: a, y: b },
                CutAbs { x: a, y: b },
                ProcessTopLeft { x: a, y: b },
                ProcessBottomRight { x: a, y: b },
                DocumentMin { x: a, y: b },
                DocumentMax { x: a, y: b },
                PartMin { part: 0x7F, x: a, y: b },
                PartMaxEx { part: 0, x: a, y: b },
                DisplayOffset { x: a, y: b },
                RapidXY { option: RapidOption::Light, x: a, y: b },
            ]);
        }
        cmds.extend([
            AxisMove { axis: Axis::Z, pos: a },
            AxisMove { axis: Axis::U, pos: a },
            PenOffset { axis: 1, value: a },
            LayerOffset { axis: 0, value: a },
        ]);
        if (-8192..=8191).contains(&a) {
            cmds.extend([MoveRel { dx: a, dy: -a.max(-8191) }, CutRel { dx: a, dy: a }, MoveRelX(a), MoveRelY(a), CutRelX(a), CutRelY(a)]);
        } else {
            for c in [MoveRel { dx: a, dy: 0 }, CutRel { dx: 0, dy: a }, MoveRelX(a), MoveRelY(a), CutRelX(a), CutRelY(a)] {
                assert!(c.encode().is_err(), "{c:?}");
            }
        }
    }
    for &u in &uints {
        cmds.extend([
            Speed(u),
            SpeedPart { part: 0x7F, speed: u },
            FileSum(u),
            FeedRepeat(u, u),
            LayerColorPart { part: 3, color: u },
            SettingReply { addr: 0x3FFF, value: u },
            SetSetting { addr: 0, value: u, value2: !u },
            FrequencyPart { laser: 0, part: 1, hz: u },
        ]);
    }
    for n in [0u16, 1, 0x7F, 0x80, 0x3FFF] {
        cmds.extend([RunFile(n), GetFileName(n), CalcFileTime(n), UFileId(n), DeleteFiles { first: n, last: 0x3FFF - n }, GetSetting { addr: n }]);
        cmds.push(FileName { index: n, name: "A_b-1.rd".into() });
        cmds.push(ProcessRepeat([n, 1, 0, 0x3FFF, 0, 0, n]));
        cmds.push(ArrayRepeat([n; 7]));
        cmds.push(MinPower { laser: Laser::L1, power: Power(n) });
        cmds.push(MaxPowerPart { laser: Laser::L2, part: 0x7F, power: Power(n) });
    }
    for c in &cmds {
        let bytes = c.encode().unwrap_or_else(|e| panic!("{c:?}: {e}"));
        assert!(bytes[1..].iter().all(|b| *b < 0x80), "{c:?} has a high bit in a parameter: {bytes:02X?}");
        assert_eq!(&Command::decode(&bytes), c, "{bytes:02X?}");
    }
    // And as one stream: the splitter finds every command again.
    let stream = command::encode_commands(&cmds).unwrap();
    assert_eq!(command::parse_commands(&stream), cmds);
}

/// The driver through a stream link that hands every reply over one byte at a time, and through
/// one that repeats every reply.
#[test]
fn replies_in_single_bytes_and_replies_that_come_twice_read_the_same() {
    struct Dribble {
        inner: MemoryTransport,
        twice: bool,
        q: std::collections::VecDeque<Vec<u8>>,
    }
    impl Transport for Dribble {
        fn framing(&self) -> Framing {
            self.inner.framing()
        }
        fn send(&mut self, channel: Channel, data: &[u8]) -> Result<()> {
            self.inner.send(channel, data)?;
            while let Some(r) = self.inner.recv(channel, std::time::Duration::ZERO)? {
                if self.inner.framing() == Framing::Stream {
                    self.q.extend(r.iter().map(|b| vec![*b]));
                } else {
                    self.q.push_back(r.clone());
                    if self.twice && r.len() > 1 {
                        self.q.push_back(r);
                    }
                }
            }
            Ok(())
        }
        fn recv(&mut self, _channel: Channel, _timeout: std::time::Duration) -> Result<Option<Vec<u8>>> {
            Ok(self.q.pop_front())
        }
        fn describe(&self) -> String {
            "dribble".into()
        }
    }
    for (framing, twice) in [(Framing::Stream, false), (Framing::Udp, true)] {
        let emu = emulator(EmulatorConfig { firmware: "V8.01.70a".into(), ..Default::default() });
        let link = Dribble { inner: MemoryTransport::new(emu.clone(), framing), twice, q: Default::default() };
        let mut m = RuidaMachine::new(link, MachineConfig::default());
        let info = m.connect().unwrap();
        assert_eq!((info.card_id, info.firmware.as_str()), (0x6510_6510, "V8.01.70a"), "{framing:?}");
        for name in ["ONE", "TWO", "THREE"] {
            m.send_job(name, &square_job((5.0, 5.0), 10.0, 50.0), &SendOptions::default(), &mut never).unwrap();
        }
        let names: Vec<String> = m.list_files().unwrap().into_iter().map(|f| f.name).collect();
        assert_eq!(names, ["ONE", "TWO", "THREE"], "{framing:?}");
        for v in [111_000, 222_000, 333_000] {
            m.write_setting(addr::G0_VELOCITY, v).unwrap();
            assert_eq!(m.read_setting(addr::G0_VELOCITY).unwrap(), v, "{framing:?}");
            assert_eq!(m.read_setting(addr::BED_X).unwrap(), 600_000, "{framing:?}");
        }
    }
}

/// A jog too long for the 32-bit position was cut to the end of the range (2147 m) and sent.
#[test]
fn a_jog_longer_than_the_position_range_is_refused_not_cut_short() {
    let (mut m, emu) = udp_machine();
    let before = lock(&emu).position();
    for (axis, d) in [(Axis::Z, 3.0e6), (Axis::U, -3.0e6), (Axis::Z, 1.0e12), (Axis::X, 3.0e6)] {
        assert!(matches!(m.jog_step(axis, d), Err(RuidaError::Encode(_))), "{axis:?} by {d} mm");
    }
    assert_eq!(lock(&emu).position(), before, "nothing moved");
    m.jog_step(Axis::Z, 2.5).unwrap();
    assert_eq!(lock(&emu).position()[2], before[2] + 2_500);
}
