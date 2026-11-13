//! Bug hunt 10 (drivers): numbers that are not numbers in the device profile, the translator
//! and the settings of a line machine. Emulators only; no port is ever opened.

use std::time::{Duration, Instant};

use darkburn_gcodebots::emu::{EmuConfig, EmuFlavor, LineEmuHandle};
use darkburn_gcodebots::profile::{DeviceKind, DeviceProfile};
use darkburn_gcodebots::translate::{TranslateOptions, translate};
use darkburn_gcodebots::{GcodeMachine, MachineError, MachineEvent};

const T: Duration = Duration::from_secs(8);

fn connect(p: &DeviceProfile, emu: &LineEmuHandle) -> Box<dyn GcodeMachine> {
    let m = p.connect(Box::new(emu.transport())).expect("connect");
    let end = Instant::now() + T;
    loop {
        if m.poll_events().iter().any(|e| matches!(e, MachineEvent::Connected(_))) {
            return m;
        }
        assert!(Instant::now() < end, "no handshake");
        std::thread::sleep(Duration::from_millis(2));
    }
}

// ---- the device profile ---------------------------------------------------------------------

/// An endless S max passed "must be above 0"; every powered move was then written with the
/// largest number there is, which a controller reads as full power.
#[test]
fn an_s_max_that_is_not_a_number_is_a_problem_of_the_profile() {
    for kind in [DeviceKind::Marlin, DeviceKind::Smoothie, DeviceKind::Custom, DeviceKind::Grbl] {
        let mut p = DeviceProfile::for_kind(kind);
        p.name = "Test".into();
        assert!(p.validate().is_empty(), "{:?}", p.validate());
        for bad in [f64::INFINITY, f64::NAN, f64::NEG_INFINITY, 0.0, -1.0] {
            p.gcode.s_max = bad;
            assert!(p.validate().iter().any(|m| m.contains("S-value max")), "{kind:?} s_max {bad}: {:?}", p.validate());
        }
    }
}

#[test]
fn fire_limits_that_are_not_numbers_are_a_problem_of_the_profile_and_never_a_crash() {
    let mut p = DeviceProfile::for_kind(DeviceKind::Marlin);
    p.name = "Test".into();
    for bad in [f64::NAN, f64::INFINITY, -1.0] {
        p.gcode.max_fire_seconds = bad;
        assert!(!p.validate().is_empty(), "fire time {bad}");
    }
    // The time limit of the fire button: no limit that can be read means no time at all.
    p.gcode.max_fire_seconds = f64::NAN;
    assert_eq!(p.fire_duration(), Duration::ZERO);
    p.gcode.max_fire_seconds = f64::INFINITY;
    assert_eq!(p.fire_duration(), Duration::from_secs(600));
    p.gcode.max_fire_seconds = 10.0;
    p.gcode.max_fire_percent = f64::NAN;
    assert!(!p.validate().is_empty(), "fire power limit that is not a number");
}

// ---- the translator -------------------------------------------------------------------------

/// The same from the translator's side: whatever its options hold, it does not write a power
/// above the full-power value, and with no usable full-power value it writes none.
#[test]
fn the_translator_writes_no_power_for_an_s_max_that_is_not_a_number() {
    let src = ["G21 G90", "M4 S0", "G0 X0 Y0", "G1 X10 S500 F6000", "G1 X20 S1000", "M5"];
    for bad in [f64::INFINITY, f64::NAN, f64::NEG_INFINITY] {
        for style in [
            darkburn_gcodebots::translate::PowerStyle::Passthrough,
            darkburn_gcodebots::translate::PowerStyle::MarlinInline,
            darkburn_gcodebots::translate::PowerStyle::Command { set: "M3 S{value}".into(), off: "M5".into() },
        ] {
            let opts = TranslateOptions { out_s_max: bad, style: style.clone(), ..Default::default() };
            let out = translate(src, &opts);
            for l in &out {
                for w in l.split_whitespace().filter(|w| w.starts_with('S')) {
                    assert_eq!(w, "S0", "s_max {bad}, {style:?}: {out:?}");
                }
            }
        }
    }
    let out = translate(src, &TranslateOptions { out_s_max: 255.0, ..Default::default() });
    assert!(out.contains(&"G1 X10 S128 F6000".to_owned()) && out.contains(&"G1 X20 S255".to_owned()), "{out:?}");
}

// ---- settings of a Marlin board -------------------------------------------------------------

/// "nan" and "inf" are numbers to the program's own reader and to the one of the board.
#[test]
fn a_marlin_setting_that_is_not_a_number_is_not_sent() {
    let mut p = DeviceProfile::for_kind(DeviceKind::Marlin);
    p.width_mm = 300.0;
    p.height_mm = 200.0;
    let emu = LineEmuHandle::with_config(EmuConfig::new(EmuFlavor::Marlin));
    let m = connect(&p, &emu);
    for bad in ["nan", "NaN", "inf", "-inf", "infinity", "1e999"] {
        assert!(matches!(m.write_setting("M92.X", bad), Err(MachineError::InvalidArgument(_))), "{bad}");
    }
    m.write_setting("M92.X", "100.5").unwrap();
    let end = Instant::now() + T;
    while !emu.with(|e| e.stats().arrived.iter().any(|l| l == "M92 X100.5")) {
        assert!(Instant::now() < end, "the good value never arrived");
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(!emu.with(|e| e.stats().arrived.iter().any(|l| l.to_ascii_lowercase().contains("nan") || l.to_ascii_lowercase().contains("inf"))));
    m.disconnect();
}

// ---- second pass: controller text cut off anywhere, with other bytes mixed in ----------------

/// Small deterministic generator, so a failure can be repeated.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

const CONTROLLER_LINES: [&str; 22] = [
    "<Idle|MPos:10.0000,-5.2500,0.0000|WPos:1.0000,2.0000,0.0000|F:4000.0,100.0>",
    "<Run,MPos:5.5290,0.5600,7.0000,WPos:1.5290,-5.4400,-0.0000>",
    "<Alarm|MPos:0.0000,0.0000,0.0000|F:0.0>",
    "X:10.00 Y:-5.25 Z:0.00 E:0.00 Count X:800 Y:-420 Z:0",
    "ok C: X:1.0000 Y:2.0000 Z:0.0000",
    "ok",
    "ok N12 P15 B3",
    "ok T:25.0 /0.0 B:25.0 /0.0 @:0 B@:0",
    "error:Unsupported command - M999",
    "Error:Printer halted. kill() called!",
    "echo:Unknown command: \"G5\"",
    "echo:busy: processing",
    "echo:  M92 X80.00 Y80.00 Z400.00 E93.00",
    "FIRMWARE_NAME:Marlin 2.1.2 (Jun  1 2023 10:00:00) SOURCE_CODE_URL:github.com/MarlinFirmware/Marlin PROTOCOL_VERSION:1.0 MACHINE_TYPE:Laser",
    "FIRMWARE_NAME:Smoothieware, FIRMWARE_URL:http%3A//smoothieware.org, X-SOURCE_CODE_URL:https://github.com/Smoothieware/Smoothieware",
    "Build version: edge-94de3d9, Build date: Mar 15 2018 22:14:36, MCU: LPC1769, System Clock: 120MHz",
    "Smoothie",
    "Snapmaker 2.0 A350 FIRMWARE_NAME:Marlin SM2-4.5.0 SOURCE_CODE_URL:https://github.com/Snapmaker",
    "start",
    "!!",
    "ALARM: Abort during cycle",
    "laser_module_max_power 0.8 # the maximum power",
];

#[test]
fn controller_lines_cut_off_or_garbled_anywhere_are_read_or_ignored() {
    use darkburn_gcodebots::line::{parse_marlin_settings, parse_smoothie_config, parse_status};
    use darkburn_gcodebots::{identify, identify_lines};
    let odd = ['\u{e9}', '\u{20ac}', '\u{1f525}', '\u{0}', '\u{7f}', '\t', '\u{fffd}', '{', '}', '%'];
    let mut all: Vec<String> = Vec::new();
    for line in CONTROLLER_LINES {
        let chars: Vec<char> = line.chars().collect();
        for cut in 0..=chars.len() {
            let head: String = chars[..cut].iter().collect();
            let tail: String = chars[cut..].iter().collect();
            for o in odd {
                let mut garbled = chars.clone();
                if cut < garbled.len() {
                    garbled[cut] = o;
                }
                let garbled: String = garbled.into_iter().collect();
                for text in [head.clone(), tail.clone(), garbled, format!("{head}{o}{tail}"), format!("{head}\r"), format!("{o}{head}")] {
                    if let Some((_, mpos, wpos, feed)) = parse_status(&text) {
                        for v in mpos.iter().chain(wpos.iter()).flatten().chain(feed.iter()) {
                            assert!(v.is_finite(), "{text:?} gave {v}");
                        }
                    }
                    let _ = identify(&text);
                    let _ = darkburn_gcodebots::template::render(&text, &[("s", "5".into()), ("", "x".into())]);
                    let _ = darkburn_gcodebots::template::render_lines(&text, &[]);
                    all.push(text);
                }
            }
        }
    }
    let _ = identify_lines(&all);
    let _ = parse_marlin_settings(&all);
    let _ = parse_smoothie_config(&all);
    let out = translate(
        &all,
        &TranslateOptions { cluster: true, s_with_every_g1: true, g0_for_overscan: true, fast_whitespace: Some(9000.0), ..Default::default() },
    );
    assert!(out.len() <= all.len());
}

/// "nan" and "inf" are no positions.
#[test]
fn a_status_report_with_a_position_that_is_not_a_number_is_not_a_position() {
    use darkburn_gcodebots::line::parse_status;
    for bad in ["nan", "inf", "-inf", "infinity", "1e999"] {
        for line in [
            format!("<Idle|MPos:{bad},0.0,0.0|WPos:0.0,0.0,0.0>"),
            format!("<Idle|MPos:0.0,{bad},0.0|WPos:0.0,0.0,0.0>"),
            format!("<Idle|MPos:0.0,0.0,0.0|WPos:0.0,0.0,{bad}>"),
            format!("<Idle,MPos:0.0,0.0,0.0,WPos:{bad},0.0,0.0>"),
            format!("<Idle|MPos:0.0,0.0,0.0|F:{bad}>"),
        ] {
            if let Some((_, mpos, wpos, feed)) = parse_status(&line) {
                for v in mpos.iter().chain(wpos.iter()).flatten().chain(feed.iter()) {
                    assert!(v.is_finite(), "{line:?} gave {v}");
                }
            }
        }
    }
}

/// G-code in odd shapes through the translator in every power style: it must come out as
/// lines, with no power above the full-power value.
#[test]
fn the_translator_takes_any_lines() {
    use darkburn_gcodebots::translate::{PowerStyle, cluster};
    let pieces = [
        "G",
        "0",
        "1",
        "2",
        "3",
        "4",
        "20",
        "21",
        "28",
        "90",
        "91",
        "92",
        "M",
        "5",
        "7",
        "8",
        "9",
        "X",
        "Y",
        "Z",
        "I",
        "J",
        "F",
        "S",
        "P",
        " ",
        ".",
        "-",
        "+",
        "1e9",
        "999999999999999999999",
        "nan",
        "inf",
        ";",
        "(",
        ")",
        "$H",
        "\u{e9}",
        "\t",
        "\r",
    ];
    let mut r = Rng(0x0123_4567_89AB_CDEF);
    let styles = [PowerStyle::Passthrough, PowerStyle::MarlinInline, PowerStyle::Command { set: "M106 P0 S{value}".into(), off: "M107 P0".into() }];
    for round in 0..400 {
        let lines: Vec<String> = (0..r.below(30)).map(|_| (0..r.below(12)).map(|_| pieces[r.below(pieces.len())]).collect()).collect();
        for style in &styles {
            let opts = TranslateOptions {
                style: style.clone(),
                out_s_max: [255.0, 1.0, 1000.0][round % 3],
                swap_xy: round % 2 == 0,
                s_with_every_g1: round % 3 == 0,
                g0_for_overscan: round % 5 == 0,
                fast_whitespace: (round % 4 == 0).then_some(6000.0),
                cluster: round % 2 == 1,
                inches: round % 7 == 0,
                travel_s0: round % 3 == 1,
                dwell_p_ms: round % 2 == 0,
                ..Default::default()
            };
            let out = translate(&lines, &opts);
            let _ = cluster(&out);
            for l in &out {
                assert!(!l.contains('\n'), "{l:?}");
                // Power words the translator wrote itself (the command style) stay in range.
                if let Some(v) = l.strip_prefix("M106 P0 S").and_then(|v| v.parse::<f64>().ok()) {
                    assert!((0.0..=opts.out_s_max).contains(&v), "{l:?} with S max {}", opts.out_s_max);
                }
            }
        }
    }
}

/// A controller made of a function: every line it is sent is answered with what the function
/// gives for it.
struct Scripted<F: FnMut(&str) -> String + Send> {
    answer: F,
    line: Vec<u8>,
    pending: std::collections::VecDeque<u8>,
}

impl<F: FnMut(&str) -> String + Send> Scripted<F> {
    fn new(answer: F) -> Self {
        Self { answer, line: Vec::new(), pending: Default::default() }
    }
}

impl<F: FnMut(&str) -> String + Send> darkburn_gcodebots::Transport for Scripted<F> {
    fn write_all(&mut self, data: &[u8]) -> std::io::Result<()> {
        for &b in data {
            if b == b'\n' {
                let line = String::from_utf8_lossy(&self.line).into_owned();
                self.line.clear();
                self.pending.extend((self.answer)(&line).into_bytes());
            } else if b >= 0x20 {
                self.line.push(b);
            }
        }
        Ok(())
    }
    fn read(&mut self, buf: &mut [u8], timeout: Duration) -> std::io::Result<usize> {
        if self.pending.is_empty() {
            std::thread::sleep(timeout.min(Duration::from_millis(1)));
        }
        let n = buf.len().min(self.pending.len());
        for slot in &mut buf[..n] {
            *slot = self.pending.pop_front().unwrap_or(b'\n');
        }
        Ok(n)
    }
    fn name(&self) -> String {
        "scripted".into()
    }
}

/// A Marlin board whose position report holds "nan" for an axis.
#[test]
fn a_marlin_position_that_is_not_a_number_is_not_shown_as_the_position() {
    let board = Scripted::new(|line: &str| match line {
        "M115" => "FIRMWARE_NAME:Marlin 2.1.2 (test) SOURCE_CODE_URL:x PROTOCOL_VERSION:1.0\nok\n".to_owned(),
        "M114" => "X:nan Y:5.00 Z:inf E:0.00 Count X:0 Y:400 Z:0\nok\n".to_owned(),
        _ => "ok\n".to_owned(),
    });
    let mut p = DeviceProfile::for_kind(DeviceKind::Marlin);
    p.width_mm = 300.0;
    p.height_mm = 200.0;
    let m = p.connect(Box::new(board)).unwrap();
    let end = Instant::now() + T;
    while m.snapshot().mpos[1] != 5.0 {
        assert!(Instant::now() < end, "no position report was read: {:?}", m.snapshot().mpos);
        std::thread::sleep(Duration::from_millis(2));
    }
    let s = m.snapshot();
    assert!(s.mpos.iter().chain(s.wpos.iter()).all(|v| v.is_finite()), "machine {:?}, work {:?}", s.mpos, s.wpos);
    m.disconnect();
}

/// A controller that answers every line with `ok` and with pieces of real and garbled
/// controller text. The driver may show nonsense on the console; its worker must keep running
/// and must let go on disconnect.
#[test]
fn the_driver_survives_a_controller_that_talks_nonsense() {
    use darkburn_grbl::{FrameOptions, Job, JogStep, MoveTarget};
    use std::sync::{Arc, Mutex};

    let panics = Arc::new(Mutex::new(Vec::<String>::new()));
    let seen = panics.clone();
    let before = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // Only what happens on a driver's worker thread; other tests have their own threads
        // and are reported as usual.
        if std::thread::current().name() == Some("darkburn-gcode") {
            seen.lock().unwrap().push(info.to_string());
        } else {
            before(info);
        }
    }));
    for (round, kind) in
        [DeviceKind::Marlin, DeviceKind::Smoothie, DeviceKind::Custom, DeviceKind::Snapmaker, DeviceKind::Marlin, DeviceKind::Smoothie]
            .into_iter()
            .enumerate()
    {
        let mut r = Rng(0x9E37_79B9_7F4A_7C15 ^ ((round as u64 + 1) << 40));
        let board = Scripted::new(move |line: &str| {
            let mut out = String::new();
            if line == "M115" || line == "version" {
                out.push_str("FIRMWARE_NAME:Marlin 2.1.2 Smoothie Build version: edge-1, MCU: LPC1769\n");
            }
            for _ in 0..r.below(4) {
                let mut text: Vec<char> = CONTROLLER_LINES[r.below(CONTROLLER_LINES.len())].chars().collect();
                match r.below(4) {
                    0 => text.truncate(r.below(text.len() + 1)),
                    1 => {
                        let at = r.below(text.len());
                        text[at] = ['\u{e9}', '\u{0}', '\u{fffd}', '<', '>', ':', ','][r.below(7)];
                    }
                    2 => text.extend("x".repeat(r.below(6000)).chars()),
                    _ => {}
                }
                out.extend(text);
                out.push_str(["\n", "\r\n", "\r", ""][r.below(4)]);
            }
            if r.below(5) != 0 {
                out.push_str("ok\n");
            }
            out
        });
        let mut p = DeviceProfile::for_kind(kind);
        p.width_mm = 300.0;
        p.height_mm = 200.0;
        if kind == DeviceKind::Custom {
            p.gcode.custom.status = "?".into();
        }
        let m = p.connect(Box::new(board)).unwrap();
        let end = Instant::now() + T;
        while m.snapshot().connection == darkburn_grbl::ConnectionState::Connecting && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(2));
        }
        for i in 0..8 {
            let _ = m.start_job(Job::from_text("G21 G90\nM4 S0\nG0 X1 Y1\nG1 X5 S300 F3000\nG1 Y5\nM5"));
            std::thread::sleep(Duration::from_millis(8));
            let _ = m.pause();
            let _ = m.resume();
            let _ = m.jog(JogStep { dx: 1.0, feed: 600.0, ..Default::default() });
            let _ = m.move_to(MoveTarget { x: Some(3.0), y: Some(4.0), feed: 600.0, ..Default::default() });
            let _ = m.frame_outline(&[kurbo::Point::new(1.0, 1.0), kurbo::Point::new(9.0, 9.0)], FrameOptions::default());
            let _ = m.fire(1.0, Duration::from_millis(5));
            let _ = m.read_settings();
            let _ = m.write_setting("M92.X", "80");
            let _ = m.send_console("M114");
            let _ = m.home();
            let _ = m.unlock();
            let _ = m.set_user_origin();
            let _ = m.go_to_origin();
            let _ = m.request_status();
            std::thread::sleep(Duration::from_millis(5));
            let _ = if i % 3 == 2 { m.emergency_stop() } else { m.stop() };
            let _ = m.fire_off();
            let _ = m.poll_events();
            let _ = m.snapshot();
        }
        let start = Instant::now();
        m.disconnect();
        assert!(start.elapsed() < Duration::from_secs(20), "{kind:?}: the driver did not let go");
    }
    drop(std::panic::take_hook());
    let seen = panics.lock().unwrap();
    assert!(seen.is_empty(), "the worker stopped with: {seen:?}");
}

/// The fire button with an S max that is no number: with an endless one the fire line asked
/// for the largest number there is, which a board takes as full power.
#[test]
fn the_fire_button_is_refused_when_the_s_max_is_not_a_number() {
    for bad in [f64::INFINITY, f64::NAN, 0.0, -255.0] {
        let mut p = DeviceProfile::for_kind(DeviceKind::Marlin);
        p.width_mm = 300.0;
        p.height_mm = 200.0;
        p.gcode.s_max = bad;
        let emu = LineEmuHandle::with_config(EmuConfig::new(EmuFlavor::Marlin));
        let m = connect(&p, &emu);
        assert!(matches!(m.fire(5.0, Duration::from_millis(50)), Err(MachineError::InvalidArgument(_))), "s_max {bad}");
        let frame = darkburn_grbl::FrameOptions { laser_percent: Some(5.0), ..Default::default() };
        assert!(
            m.frame_outline(&[kurbo::Point::new(1.0, 1.0), kurbo::Point::new(9.0, 9.0)], frame).is_err(),
            "s_max {bad}: a frame with the laser on"
        );
        std::thread::sleep(Duration::from_millis(60));
        let sent = emu.with(|e| e.stats().arrived.clone());
        assert!(!sent.iter().any(|l| l.starts_with("M3") || l.starts_with("M4") || l.starts_with("M106")), "s_max {bad}: {sent:?}");
        m.disconnect();
    }
}
