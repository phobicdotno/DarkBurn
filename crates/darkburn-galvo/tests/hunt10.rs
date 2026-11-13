//! Bug hunt 10 (drivers): numbers the galvo board cannot hold. No test opens a real device.

use darkburn_galvo::cor::{CorFormat, decode_offset, encode_offset};
use darkburn_galvo::*;
use kurbo::BezPath;

fn rect(x: f64, y: f64, w: f64, h: f64) -> BezPath {
    let mut p = BezPath::new();
    p.move_to((x, y));
    p.line_to((x + w, y));
    p.line_to((x + w, y + h));
    p.line_to((x, y + h));
    p.close_path();
    p
}

fn job_with(mode: LayerMode, params: MarkParams) -> Job {
    let mut job = Job::new();
    job.layers.push(Layer::new("L", mode, params, vec![rect(-1.0, -1.0, 2.0, 2.0)]));
    job
}

// ---- lens correction files ------------------------------------------------------------------

/// The board takes an offset as a sign bit and 15 bits. 40000 does not fit; it went out as
/// 0x9C40, which the board reads as -7232.
#[test]
fn a_lens_offset_too_large_for_the_board_keeps_its_sign() {
    assert_eq!(decode_offset(encode_offset(40_000)), 0x7FFF);
    assert_eq!(decode_offset(encode_offset(-40_000)), -0x7FFF);
    assert_eq!(decode_offset(encode_offset(i32::MAX)), 0x7FFF);
    assert_eq!(decode_offset(encode_offset(i32::MIN)), -0x7FFF);
    assert_eq!(decode_offset(encode_offset(0x8000)), 0x7FFF, "0x8000 alone is the sign bit, read as minus zero");
    for v in [-0x7FFF, -300, -1, 0, 1, 300, 0x7FFF] {
        assert_eq!(decode_offset(encode_offset(v)), v);
    }
}

#[test]
fn a_lens_file_with_an_offset_the_board_cannot_hold_is_refused() {
    for format in [CorFormat::Int, CorFormat::Float] {
        for bad in [40_000, -40_000, i32::MAX, i32::MIN, 0x8000] {
            let mut t = CorTable::from_fn(|r, c| (r as i32 - 32, c as i32 - 32));
            t.format = format;
            assert_eq!(CorTable::parse(&t.to_bytes()).unwrap().node(10, 20), (-22, -12), "a good table reads back");
            t.set_node(10, 20, (5, bad));
            let err = CorTable::parse(&t.to_bytes()).expect_err("an offset that does not fit");
            assert!(!matches!(err, CorError::Truncated { .. }), "{err}");
            assert!(err.to_string().contains("row 11") && err.to_string().contains("column 21"), "{err}");
        }
    }
}

/// A float file can also hold "not a number" and infinity. They were read as 0 and as the
/// largest number.
#[test]
fn a_lens_file_with_a_node_that_is_not_a_number_is_refused() {
    let good = CorTable::from_fn(|r, c| (r as i32, c as i32)).to_bytes();
    let node0 = good.len() - 65 * 65 * 16;
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 1.0e12, -1.0e300] {
        for at in [node0, node0 + 8, good.len() - 8] {
            let mut bytes = good.clone();
            bytes[at..at + 8].copy_from_slice(&bad.to_le_bytes());
            assert!(CorTable::parse(&bytes).is_err(), "{bad} at byte {at}");
        }
    }
    assert!(CorTable::parse(&good).is_ok());
}

// ---- jobs with numbers that are not numbers -------------------------------------------------

#[test]
fn a_layer_with_a_speed_or_frequency_that_is_not_a_number_is_refused() {
    for source in [LaserSource::Fiber, LaserSource::Mopa, LaserSource::Co2, LaserSource::Uv] {
        let dev = DeviceSettings { source, ..Default::default() };
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            for params in [
                MarkParams { speed_mm_s: bad, ..Default::default() },
                MarkParams { frequency_khz: bad, ..Default::default() },
                MarkParams { power_pct: bad, ..Default::default() },
                MarkParams { timings: Some(Timings { jump_speed_mm_s: bad, ..Default::default() }), ..Default::default() },
            ] {
                let r = compile(&job_with(LayerMode::Line, params.clone()), &dev, &CompileOptions::default());
                assert!(matches!(r, Err(CompileError::Invalid(_))), "{source:?} {params:?}");
            }
        }
        assert!(compile(&job_with(LayerMode::Line, MarkParams::default()), &dev, &CompileOptions::default()).is_ok());
    }
}

/// With a CO2 or UV source the pulse width is power divided by frequency. A frequency that is
/// not a number was taken as the lowest one, which gave the widest pulse there is.
#[test]
fn a_frequency_that_is_not_a_number_never_becomes_full_power() {
    let dev = DeviceSettings { source: LaserSource::Co2, ..Default::default() };
    let params = MarkParams { frequency_khz: f64::NAN, power_pct: 10.0, ..Default::default() };
    match compile(&job_with(LayerMode::Line, params), &dev, &CompileOptions::default()) {
        Err(CompileError::Invalid(_)) => {}
        Ok(j) => panic!("compiled with {:?}", j.list_commands().iter().find(|c| matches!(c, ListCmd::MarkPowerRatio(_)))),
        Err(e) => panic!("{e}"),
    }
}

/// A fill whose line interval is not a number was hatched at the smallest interval there is.
#[test]
fn a_fill_with_an_interval_that_is_not_a_number_is_refused() {
    for bad in [f64::NAN, f64::INFINITY] {
        let fill = FillSettings { interval_mm: bad, ..Default::default() };
        for mode in [LayerMode::Fill(fill), LayerMode::Flood(fill)] {
            let r = compile(&job_with(mode, MarkParams::default()), &DeviceSettings::default(), &CompileOptions::default());
            assert!(matches!(r, Err(CompileError::Invalid(_))), "interval {bad}: {:?}", r.map(|j| j.list_commands().len()));
        }
    }
}

#[test]
fn device_speeds_that_are_not_numbers_are_refused() {
    let job = job_with(LayerMode::Line, MarkParams::default());
    let frame = FrameSpec::from_rect(kurbo::Rect::new(-5.0, -5.0, 5.0, 5.0));
    for bad in [f64::NAN, f64::INFINITY] {
        let dev = DeviceSettings { timings: Timings { jump_speed_mm_s: bad, ..Default::default() }, ..Default::default() };
        assert!(matches!(compile(&job, &dev, &CompileOptions::default()), Err(CompileError::Invalid(_))), "jump speed {bad}");
        let dev = DeviceSettings { light: LightSettings { speed_mm_s: bad, ..Default::default() }, ..Default::default() };
        assert!(matches!(compile_frame(&frame, &dev), Err(CompileError::Invalid(_))), "frame speed {bad}");
        let spec = FrameSpec { speed_mm_s: Some(bad), ..frame.clone() };
        assert!(matches!(compile_frame(&spec, &DeviceSettings::default()), Err(CompileError::Invalid(_))), "frame speed {bad}");
    }
    assert!(compile_frame(&frame, &DeviceSettings::default()).is_ok());
}

// ---- port bits the port does not have -------------------------------------------------------

/// The driver refuses such settings when it connects, but a job is compiled without it.
#[test]
fn a_job_for_a_port_bit_the_port_does_not_have_is_refused() {
    let job = job_with(LayerMode::Line, MarkParams::default());
    let frame = FrameSpec::from_rect(kurbo::Rect::new(-5.0, -5.0, 5.0, 5.0));
    for ports in [
        PortConfig { mark_bit: 16, ..Default::default() },
        PortConfig { mark_bit: 99, ..Default::default() },
        PortConfig { light_bit: 16, ..Default::default() },
        PortConfig { light_bit: 255, ..Default::default() },
    ] {
        let dev = DeviceSettings { ports, ..Default::default() };
        let r = compile(&job, &dev, &CompileOptions::default());
        assert!(matches!(&r, Err(CompileError::Invalid(why)) if why.contains("0 to 15")), "{ports:?}");
        let r = compile_frame(&frame, &dev);
        assert!(matches!(&r, Err(CompileError::Invalid(why)) if why.contains("0 to 15")), "{ports:?}");
    }
}

/// With an endless field every point is the centre of the field (a drill job fired every shot
/// into one spot); with a negative one the job came out turned round.
#[test]
fn a_field_size_that_is_not_a_size_is_refused() {
    let job = job_with(LayerMode::Line, MarkParams::default());
    let frame = FrameSpec::from_rect(kurbo::Rect::new(-5.0, -5.0, 5.0, 5.0));
    for bad in [f64::INFINITY, f64::NAN, 0.0, -110.0] {
        let dev = DeviceSettings { field: FieldConfig { size_mm: bad, ..Default::default() }, ..Default::default() };
        let r = compile(&job, &dev, &CompileOptions::default());
        assert!(r.is_err(), "field size {bad}: {:?}", r.map(|j| j.list_commands().len()));
        assert!(compile_frame(&frame, &dev).is_err(), "frame, field size {bad}");
        let drill = job_with(LayerMode::Drill(DrillSettings::default()), MarkParams::default());
        assert!(compile(&drill, &dev, &CompileOptions::default()).is_err(), "drill, field size {bad}");
    }
}

/// A band size that is not a number passed the "between 0 and the field size" check and the
/// whole height was then marked as one band.
#[test]
fn a_rotary_band_size_that_is_not_a_number_is_refused() {
    let mut job = Job::new();
    job.layers.push(Layer::new("L", LayerMode::Line, MarkParams::default(), vec![rect(-10.0, -40.0, 20.0, 80.0)]));
    for mode in [RotaryMode::BySlice, RotaryMode::ByShape] {
        job.rotary = Some(RotaryConfig { mode, max_size_mm: 10.0, axis: 0 });
        assert!(compile(&job, &DeviceSettings::default(), &CompileOptions::default()).is_ok());
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            job.rotary = Some(RotaryConfig { mode, max_size_mm: bad, axis: 0 });
            let r = compile(&job, &DeviceSettings::default(), &CompileOptions::default());
            assert!(matches!(r, Err(CompileError::Invalid(_))), "{mode:?} band size {bad}");
        }
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

/// Any 12 bytes read as a command and written again are the same 12 bytes, as a list command
/// and as a single command, and a second reading gives the same command.
#[test]
fn every_command_reads_back_as_written() {
    let mut r = Rng(0xA5A5_5A5A_1234_4321);
    let mut codes: Vec<u16> = (0..0x60).chain(0x8000..0x8060).collect();
    codes.extend([0x7FFF, 0xFFFF, 0x0100, 0x8100]);
    for code in codes {
        for round in 0..40 {
            let mut b = [0u8; 12];
            b[..2].copy_from_slice(&code.to_le_bytes());
            for (i, slot) in b[2..].iter_mut().enumerate() {
                *slot = match round {
                    0 => 0x00,
                    1 => 0xFF,
                    2 => 0x80,
                    3 => [0xFF, 0x7F][i % 2],
                    _ => r.next() as u8,
                };
            }
            let cmd = Command::from_bytes(&b).unwrap();
            assert_eq!(cmd.to_bytes(), b);
            let list = ListCmd::from_command(cmd);
            assert_eq!(ListCmd::from_command(list.to_command()), list, "{b:02X?}");
            let single = SingleCmd::from_command(cmd);
            assert_eq!(SingleCmd::from_command(single.to_command()), single, "{b:02X?}");
        }
    }
}

#[test]
fn a_compiled_job_reads_back_from_its_packets() {
    let mut job = Job::new();
    let fill = LayerMode::Fill(FillSettings { interval_mm: 0.05, ..Default::default() });
    job.layers.push(Layer::new("F", fill, MarkParams::default(), vec![rect(-10.0, -10.0, 20.0, 20.0)]));
    job.layers.push(Layer::new("D", LayerMode::Drill(DrillSettings::default()), MarkParams::default(), vec![rect(-3.0, -3.0, 6.0, 6.0)]));
    let compiled = compile(&job, &DeviceSettings::default(), &CompileOptions::default()).unwrap();
    let cmds = compiled.list_commands();
    assert!(cmds.len() > 300, "more than one packet: {}", cmds.len());
    let packets = packetize(&cmds);
    let bytes: Vec<Vec<u8>> = packets.iter().map(|p| p.as_bytes().to_vec()).collect();
    let back: Vec<ListPacket> = bytes.iter().map(|b| ListPacket::from_bytes(b).unwrap()).collect();
    assert_eq!(unpacketize(&back), cmds);
    // Cut off at any length, a packet is refused, not read short.
    for cut in [0, 1, 11, 12, 13, 3060, 3071, 3073] {
        let mut b = bytes[0].clone();
        b.resize(cut, 0);
        assert!(ListPacket::from_bytes(&b).is_err(), "{cut} bytes");
    }
}

/// A lens file cut off anywhere is refused as too short; with any one byte changed it is read
/// or refused, and what is read can always be sent to the board.
#[test]
fn a_lens_file_cut_off_or_damaged_anywhere_is_refused_or_read() {
    let mut r = Rng(0x0DDB_A11C_0FFE_E000);
    for format in [CorFormat::Float, CorFormat::Int] {
        let mut t = CorTable::from_fn(|row, col| (row as i32 * 3 - 90, 70 - col as i32 * 2));
        t.format = format;
        t.scale = Some(1.25);
        let good = t.to_bytes();
        let back = CorTable::parse(&good).unwrap();
        assert_eq!((back.node(64, 64), back.node(0, 0), back.scale), (t.node(64, 64), t.node(0, 0), Some(1.25)), "{format:?}");
        for cut in (0..good.len()).step_by(97).chain(good.len() - 20..good.len()) {
            assert!(matches!(CorTable::parse(&good[..cut]), Err(CorError::Truncated { .. })), "{format:?} cut at {cut}");
        }
        for _ in 0..3000 {
            let mut b = good.clone();
            let at = (r.next() % b.len() as u64) as usize;
            b[at] = r.next() as u8;
            if let Ok(t) = CorTable::parse(&b) {
                let cmds = t.upload_commands();
                assert_eq!(cmds.len(), 1 + 65 * 65);
                let _ = t.to_bytes();
                let _ = t.offset_at(r.next() as f64 / 1e15, 32768.0);
            }
        }
    }
}

/// EZCad settings text in odd shapes: other line ends, other encodings, cut off in the middle
/// of a character, very long lines, numbers that are none.
#[test]
fn settings_text_in_odd_shapes_is_read_or_refused() {
    let text = "[Lens]\r\nFieldSize=110,5\r\nOffsetX = -1.5\r\nGalvo1Scale=101.5\r\nSwapXY=1\r\nLaserType=co2\r\nJumpSpeed=4000\r\n";
    let good = ezcad::import(text.as_bytes()).unwrap();
    assert_eq!((good.settings.field.size_mm, good.settings.field.offset_mm.0, good.settings.source), (110.5, -1.5, LaserSource::Co2));
    // UTF-16 with a byte order mark, both ways, reads the same.
    for le in [true, false] {
        let mut b: Vec<u8> = if le { vec![0xFF, 0xFE] } else { vec![0xFE, 0xFF] };
        for u in text.encode_utf16() {
            b.extend(if le { u.to_le_bytes() } else { u.to_be_bytes() });
        }
        assert_eq!(ezcad::import(&b).unwrap().settings, good.settings);
        for cut in 0..b.len() {
            let _ = ezcad::import(&b[..cut]);
        }
    }
    for cut in 0..text.len() {
        let _ = ezcad::import(&text.as_bytes()[..cut]);
    }
    for odd in [
        "FieldSize=\u{fffd}\u{20ac}110",
        "Fi\u{00e9}ldSize=110\nOffsetX=\u{00b5}",
        "FieldSize=1e999\nJumpSpeed=1e999\nGalvo1Scale=1e999",
        "FieldSize=-0\nFieldSize=--5\nFieldSize=+.e\nRotate=1e300\nRotate=-1e300\nMinJumpDelay=1e300\nEndTC=-1e300",
        "=\n==\n=5\nFieldSize\nFieldSize=\n",
    ] {
        if let Ok(i) = ezcad::import(odd.as_bytes()) {
            let s = &i.settings;
            assert!(s.field.size_mm.is_finite() && s.field.size_mm > 0.0, "{odd:?} gave field size {}", s.field.size_mm);
            assert!(
                s.timings.jump_speed_mm_s.is_finite() && s.timings.jump_speed_mm_s > 0.0,
                "{odd:?} gave jump speed {}",
                s.timings.jump_speed_mm_s
            );
            assert!(s.field.galvo1.scale.is_finite(), "{odd:?} gave scale {}", s.field.galvo1.scale);
            assert!(s.field.rotate_quarters < 4);
        }
    }
    let long = format!("FieldSize={}\nOffsetY={}\n", "9".repeat(100_000), "1".repeat(400));
    let _ = ezcad::import(long.as_bytes());
}

/// A board that answers every command with eight bytes of noise, or with nothing. The driver
/// may fail the connection or the job, but its worker must keep running and must let go.
#[test]
fn the_driver_survives_a_board_that_talks_nonsense() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    struct Noise {
        rng: Rng,
        mode: u8,
        writes: Arc<AtomicUsize>,
        pending: usize,
    }
    impl Transport for Noise {
        fn write(&mut self, data: &[u8]) -> std::io::Result<()> {
            assert!(data.len() == 12 || data.len() == 3072, "a write of {} bytes", data.len());
            self.writes.fetch_add(1, Ordering::SeqCst);
            // Sometimes two answers, sometimes none.
            self.pending += [1, 1, 1, 2, 0][(self.rng.next() % 5) as usize];
            Ok(())
        }
        fn read(&mut self, _timeout: Duration) -> std::io::Result<Option<[u8; 8]>> {
            if self.pending == 0 {
                return Ok(None);
            }
            self.pending -= 1;
            let mut b = self.rng.next().to_le_bytes();
            match self.mode {
                // Always "ready, not busy".
                0 => b[6..].copy_from_slice(&0x0020u16.to_le_bytes()),
                // Always "busy".
                1 => b[6..].copy_from_slice(&0x0004u16.to_le_bytes()),
                // All bits set, all bits clear.
                2 => b = [0xFF; 8],
                3 => b = [0; 8],
                _ => {}
            }
            Ok(Some(b))
        }
        fn name(&self) -> String {
            "noise".into()
        }
    }

    let panics = Arc::new(Mutex::new(Vec::<String>::new()));
    let seen = panics.clone();
    let before = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // Only what happens on a driver's worker thread; other tests have their own threads
        // and are reported as usual.
        if std::thread::current().name() == Some("galvo-worker") {
            seen.lock().unwrap().push(info.to_string());
        } else {
            before(info);
        }
    }));
    let dev =
        DeviceSettings { ports: PortConfig { door_bit: Some(3), foot_bit: Some(4), done_bit: Some(5), ..Default::default() }, ..Default::default() };
    let mut job = Job::new();
    job.layers.push(Layer::new("L", LayerMode::Line, MarkParams::default(), vec![rect(-10.0, -10.0, 20.0, 20.0)]));
    job.repeat.count = 2;
    let compiled = compile(&job, &dev, &CompileOptions::default()).unwrap();
    let frame = compile_frame(&FrameSpec::from_rect(kurbo::Rect::new(-5.0, -5.0, 5.0, 5.0)), &dev).unwrap();
    for mode in 0..6u8 {
        let writes = Arc::new(AtomicUsize::new(0));
        let t = Noise { rng: Rng(0x9E37_79B9_7F4A_7C15 ^ ((mode as u64 + 1) << 32)), mode, writes: writes.clone(), pending: 0 };
        let opts = ConnectOptions { port_poll_interval: Duration::from_millis(1), read_timeout: Duration::from_millis(5), ..Default::default() };
        let mut m = GalvoMachine::connect(Box::new(t), dev.clone(), opts);
        let end = Instant::now() + Duration::from_secs(5);
        while m.snapshot().connection == ConnectionState::Connecting && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_ne!(m.snapshot().connection, ConnectionState::Connecting, "mode {mode}: still connecting");
        for round in 0..6 {
            let _ = m.arm();
            let _ = m.start_job(compiled.clone(), StartOptions { wait_for_foot: round % 2 == 1, skip_framing_check: true });
            std::thread::sleep(Duration::from_millis(15));
            let _ = m.pause();
            let _ = m.resume();
            let _ = m.move_axis(0, 5.0);
            let _ = m.set_light(round % 2 == 0);
            std::thread::sleep(Duration::from_millis(10));
            let _ = m.stop();
            let _ = m.start_frame(frame.clone(), true);
            std::thread::sleep(Duration::from_millis(10));
            let _ = m.set_lens(Some(CorTable::identity()));
            let _ = m.stop();
            let _ = m.disarm();
            let _ = m.poll_events();
        }
        let start = Instant::now();
        m.disconnect();
        assert!(start.elapsed() < Duration::from_secs(20), "mode {mode}: the driver did not let go");
    }
    drop(std::panic::take_hook());
    let seen = panics.lock().unwrap();
    assert!(seen.is_empty(), "the worker stopped with: {seen:?}");
}

// ---- what a disconnect reports ----------------------------------------------------------------

/// A disconnect while the red light traces a frame ended it as a stopped job, and the program
/// then said that a job had been aborted.
#[test]
fn a_disconnect_while_framing_reports_a_stopped_frame_not_a_stopped_job() {
    use std::time::{Duration, Instant};
    let dev = DeviceSettings::default();
    let frame = compile_frame(&FrameSpec::from_rect(kurbo::Rect::new(-5.0, -5.0, 5.0, 5.0)), &dev).unwrap();
    let (t, _emu) = MemoryTransport::new(GalvoEmulator::new(EmulatorConfig { cmds_per_poll: 1, ..Default::default() }));
    let mut m = GalvoMachine::connect(Box::new(t), dev, ConnectOptions { port_poll_interval: Duration::ZERO, ..Default::default() });
    let mut events = Vec::new();
    let end = Instant::now() + Duration::from_secs(10);
    let wait = |m: &GalvoMachine, events: &mut Vec<Event>, what: &dyn Fn(&[Event], &Snapshot) -> bool| {
        loop {
            events.extend(m.poll_events());
            if what(events, &m.snapshot()) {
                return;
            }
            assert!(Instant::now() < end, "timed out: {events:?}");
            std::thread::sleep(Duration::from_millis(2));
        }
    };
    wait(&m, &mut events, &|_, s| s.connection == ConnectionState::Connected);
    m.start_frame(frame, true).unwrap();
    wait(&m, &mut events, &|e, _| e.contains(&Event::FrameStarted));
    m.disconnect();
    events.extend(m.poll_events());
    assert!(events.contains(&Event::FrameStopped), "{events:?}");
    assert!(!events.contains(&Event::JobStopped), "{events:?}");
    assert!(matches!(events.last(), Some(Event::Disconnected(None))), "{events:?}");
}

/// A pause and resume while the job waits for the foot switch showed a running job; the job
/// still waited for the foot switch.
#[test]
fn a_resume_while_waiting_for_the_foot_switch_still_shows_the_wait() {
    use std::time::{Duration, Instant};
    let dev = DeviceSettings::default();
    let (t, emu) = MemoryTransport::new(GalvoEmulator::new(EmulatorConfig { cmds_per_poll: 16, ..Default::default() }));
    let m = GalvoMachine::connect(Box::new(t), dev.clone(), ConnectOptions { port_poll_interval: Duration::ZERO, ..Default::default() });
    let wait = |what: &dyn Fn(&Snapshot) -> bool| {
        let end = Instant::now() + Duration::from_secs(10);
        while !what(&m.snapshot()) {
            assert!(Instant::now() < end, "timed out: {:?}", m.snapshot());
            std::thread::sleep(Duration::from_millis(2));
        }
    };
    wait(&|s| s.connection == ConnectionState::Connected);
    let mut job = Job::new();
    job.layers.push(Layer::new("L", LayerMode::Line, MarkParams::default(), vec![rect(-5.0, -5.0, 10.0, 10.0)]));
    let compiled = compile(&job, &dev, &CompileOptions::default()).unwrap();
    m.arm().unwrap();
    m.start_job(compiled, StartOptions { wait_for_foot: true, ..Default::default() }).unwrap();
    wait(&|s| s.activity == Activity::WaitingFoot);
    m.pause().unwrap();
    wait(&|s| s.activity == Activity::Paused);
    m.resume().unwrap();
    wait(&|s| s.activity != Activity::Paused);
    std::thread::sleep(Duration::from_millis(30));
    assert_eq!(m.snapshot().activity, Activity::WaitingFoot);
    assert_eq!(emu.lock().unwrap().emitting_marks(), 0, "nothing marked without the foot switch");
    emu.lock().unwrap().set_input_bit(15, false);
    wait(&|s| s.activity == Activity::Idle);
    assert_eq!(emu.lock().unwrap().emitting_marks(), 4);
}
