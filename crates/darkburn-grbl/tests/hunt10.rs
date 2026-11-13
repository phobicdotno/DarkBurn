//! Bug hunt 10 (drivers): odd numbers and odd controller text. Everything runs against the
//! in-memory emulator; no port is ever opened.

use std::time::{Duration, Instant};

use darkburn_grbl::*;
use kurbo::Rect;

const T: Duration = Duration::from_secs(10);

fn emulator(cfg: EmulatorConfig) -> EmulatorHandle {
    EmulatorHandle::new(GrblEmulator::new(cfg))
}

fn connect(emu: &EmulatorHandle, opts: ConnectOptions) -> GrblMachine {
    let m = GrblMachine::connect(Box::new(emu.transport()), opts);
    let end = Instant::now() + T;
    while Instant::now() < end {
        match m.next_event(Duration::from_millis(20)) {
            Some(Event::Connected(_)) => return m,
            Some(Event::ConnectFailed(r)) => panic!("connect failed: {r}"),
            _ => {}
        }
    }
    panic!("timed out waiting for the connection");
}

// ---- the bounds check and numbers that are not numbers --------------------------------------

/// Every comparison with "not a number" is false, so a job whose extent could not be worked out
/// passed as "inside the machine".
#[test]
fn a_job_whose_extent_is_not_a_number_does_not_pass_the_bounds_check() {
    let machine = Rect::new(0.0, 0.0, 400.0, 400.0);
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        for job in [Rect::new(bad, 0.0, 10.0, 10.0), Rect::new(0.0, bad, 10.0, 10.0), Rect::new(0.0, 0.0, bad, 10.0), Rect::new(0.0, 0.0, 10.0, bad)]
        {
            assert!(matches!(BoundsCheck { job, machine }.check(), Err(GrblError::OutOfBounds(_))), "job {job:?}");
        }
    }
    // A machine whose size is not known as a number cannot vouch for a job either.
    for m in [Rect::new(f64::NAN, 0.0, 400.0, 400.0), Rect::new(0.0, 0.0, f64::NAN, 400.0), Rect::new(0.0, f64::NAN, 400.0, f64::NAN)] {
        assert!(BoundsCheck { job: Rect::new(0.0, 0.0, 10.0, 10.0), machine: m }.check().is_err(), "machine {m:?}");
    }
    assert!(BoundsCheck { job: Rect::new(0.0, 0.0, 400.0, 400.0), machine }.check().is_ok());

    let emu = emulator(EmulatorConfig::atomstack());
    let m = connect(&emu, ConnectOptions::default());
    let job = Job::from_text("G0 X450").with_bounds(BoundsCheck { job: Rect::new(0.0, 0.0, f64::NAN, 10.0), machine });
    assert!(matches!(m.start_job(job), Err(GrblError::OutOfBounds(_))));
    std::thread::sleep(Duration::from_millis(50));
    assert!(!emu.with(|e| e.stats().lines.iter().any(|l| l.contains("X450"))));
}

// ---- second pass: controller text cut off anywhere, with other bytes mixed in ----------------

/// Real lines of a controller, cut off after every character and with a character changed for
/// one that is not ASCII. The readers may make nothing of such a line; they must not stop.
#[test]
fn controller_lines_cut_off_or_garbled_anywhere_are_read_or_ignored() {
    use darkburn_grbl::protocol::*;
    let lines = [
        "<Idle|MPos:10.000,-5.250,0.000|FS:1500,300|Bf:15,127|WCO:1.000,2.000,0.000|Ov:100,100,100|Pn:XYD|A:SFM|Ln:12>",
        "<Run,MPos:5.529,0.560,7.000,WPos:1.529,-5.440,-0.000,Buf:3,RX:12,Ln:5,F:500.>",
        "<Hold:1|WPos:0.000,0.000,0.000|FS:0,0>",
        "<Door:3|MPos:0.000,0.000,0.000,0.000,0.000|F:0>",
        "Grbl 1.1h ['$' for help]",
        "GrblHAL 1.1f ['$' or '$HELP' for help]",
        "Grbl 3.7 [FluidNC v3.7.8 (wifi) '$' for help]",
        "[VER:1.1h.20190825:My laser]",
        "[OPT:VZL,15,128,3]",
        "[MSG:Reset to continue]",
        "[GC:G0 G54 G17 G21 G90 G94 M5 M9 T0 F0 S0]",
        "$130=400.000",
        "$32=1 (laser mode, bool)",
        "error:22",
        "ALARM:9",
        "ok",
    ];
    let odd = ['\u{e9}', '\u{20ac}', '\u{1f525}', '\u{0}', '\u{7f}', '\t', '\u{fffd}'];
    for line in lines {
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
                for text in
                    [head.as_str(), tail.as_str(), garbled.as_str(), &format!("{head}{o}{tail}"), &format!("{head}\r\n"), &format!("{o}{head}")]
                {
                    let _ = parse_response(text);
                    let _ = parse_banner(text);
                    let _ = clean_line(text);
                    let _ = is_motion_line(text);
                    let _ = words(text);
                    let _ = darkburn_grbl::identify_laser(text);
                    if let Some(st) = parse_status(text) {
                        // A position that is read is a number.
                        for v in st.mpos.iter().chain(st.wpos.iter()).chain(st.wco.iter()).flatten() {
                            assert!(v.is_finite(), "{text:?} gave {v}");
                        }
                    }
                }
            }
        }
    }
    // Very long lines.
    let long = format!("<Idle|MPos:{}>", "1,".repeat(50_000));
    let _ = parse_status(&long);
    let _ = parse_response(&"9".repeat(100_000));
    let _ = parse_response(&format!("error:{}", "9".repeat(1000)));
    let _ = parse_response(&format!("${}={}", "1".repeat(1000), "2".repeat(1000)));
}

/// One character of a GRBL 0.9 report garbled on the way: the other values moved up one place,
/// so X showed what Y is and Y what Z is.
#[test]
fn a_garbled_value_in_an_old_style_report_does_not_shift_the_other_axes() {
    use darkburn_grbl::protocol::parse_status;
    let good = parse_status("<Idle,MPos:5.529,0.560,7.000,WPos:1.529,-5.440,-0.000>").unwrap();
    assert_eq!(good.mpos.unwrap()[..3], [5.529, 0.560, 7.000]);
    for line in [
        "<Idle,MPos:5.5x9,0.560,7.000,WPos:1.529,-5.440,-0.000>",
        "<Idle,MPos:5.529,0.5?0,7.000,WPos:1.529,-5.440,-0.000>",
        "<Idle,MPos:5.529,0.560,7.000,WPos:1.5\u{fffd}9,-5.440,-0.000>",
        "<Idle,MPos:,0.560,7.000,WPos:1.529,-5.440,-0.000>",
    ] {
        if let Some(st) = parse_status(line) {
            assert!(st.mpos.is_none_or(|m| m[..3] == [5.529, 0.560, 7.000]), "{line:?} gave machine position {:?}", st.mpos);
            assert!(st.wpos.is_none_or(|w| w[..3] == [1.529, -5.440, 0.0]), "{line:?} gave work position {:?}", st.wpos);
        }
    }
}

/// Positions a controller cannot be at. Rust reads "nan", "inf" and "1e999" as numbers.
#[test]
fn a_status_report_with_a_position_that_is_not_a_number_is_not_a_position() {
    use darkburn_grbl::protocol::parse_status;
    for bad in ["nan", "NaN", "inf", "-inf", "infinity", "1e999", "-1e999"] {
        for line in [
            format!("<Idle|MPos:{bad},0.000,0.000|FS:0,0>"),
            format!("<Idle|MPos:0.000,{bad},0.000|FS:0,0>"),
            format!("<Idle|WPos:0.000,0.000,{bad}|FS:0,0>"),
            format!("<Idle|MPos:1.000,2.000,3.000|WCO:{bad},0.000,0.000>"),
            format!("<Idle,MPos:{bad},0.000,0.000,WPos:0.000,0.000,0.000>"),
            format!("<Idle,MPos:0.000,0.000,0.000,WPos:0.000,{bad},0.000>"),
        ] {
            if let Some(st) = parse_status(&line) {
                for v in st.mpos.iter().chain(st.wpos.iter()).chain(st.wco.iter()).flatten() {
                    assert!(v.is_finite(), "{line:?} gave {v}");
                }
            }
        }
    }
}

// ---- the S value of full power ----------------------------------------------------------------

/// The device settings can hand over an S max that is no number (their own check lets it
/// through). The driver then took 1000 for full power whatever the controller's own `$30`
/// said: on a controller with `$30=255` a fire at 10 % went out as S100, which is 39 %.
#[test]
fn an_s_max_that_is_not_a_number_falls_back_to_the_controllers_own() {
    for bad in [f64::NAN, f64::INFINITY, 0.0, -5.0] {
        let emu = emulator(EmulatorConfig::atomstack().with_setting(30, 255.0));
        let m = connect(&emu, ConnectOptions { s_max: Some(bad), max_fire_percent: 10.0, ..Default::default() });
        assert!(m.wait_until(T, |s| s.settings.contains_key(&30) && s.status.state == MachineState::Idle), "no settings read");
        m.fire(10.0, Duration::from_millis(200)).unwrap();
        let end = Instant::now() + T;
        let fired = loop {
            if let Some(l) = emu.with(|e| e.stats().lines.iter().find(|l| l.contains("M3 S")).cloned()) {
                break l;
            }
            assert!(Instant::now() < end, "s_max {bad}: the fire line never arrived: {:?}", emu.with(|e| e.stats().lines.clone()));
            std::thread::sleep(Duration::from_millis(2));
        };
        assert!(fired.ends_with("M3 S26"), "s_max {bad}: 10 % of 255 is S26, sent {fired:?}");
        m.fire_off().unwrap();
    }
}

// ---- settings that are not numbers ------------------------------------------------------------

/// Rust reads "nan", "inf" and "1e999" as numbers, so they passed the driver's "is it a number"
/// check on their way to the controller, and a settings backup holding them was read as good.
#[test]
fn a_setting_that_is_not_a_number_is_not_sent() {
    let emu = emulator(EmulatorConfig::atomstack());
    let m = connect(&emu, ConnectOptions::default());
    assert!(m.wait_until(T, |s| s.status.state == MachineState::Idle));
    for bad in ["nan", "NaN", "inf", "-inf", "infinity", "1e999"] {
        assert!(matches!(m.write_setting(30, bad), Err(GrblError::InvalidArgument(_))), "{bad}");
        assert!(SettingsBackup::parse(&format!("$30={bad}\n$31=0\n")).is_err(), "backup with {bad}");
    }
    assert!(SettingsBackup::parse("$30=1000\n$31=0.5\n").is_ok());
    std::thread::sleep(Duration::from_millis(50));
    emu.with(|e| assert!(!e.stats().lines.iter().any(|l| l.starts_with("$30=")), "{:?}", e.stats().lines));
}

/// A GRBL controller is not ready before its first status report: until then its state (a hold,
/// the door, an alarm it reports only in the status) and the place of its head are unknown, and a
/// job placed then used a work offset of zero.
#[test]
fn a_controller_is_not_ready_before_its_first_status_report() {
    let mut s = darkburn_grbl::machine::Snapshot { connection: darkburn_grbl::machine::ConnectionState::Connected, ..Default::default() };
    assert_ne!(s.laser_status(), darkburn_grbl::machine::LaserStatus::Ready, "ready without a status report");
    s.status.updated = Some(std::time::Instant::now());
    assert_eq!(s.laser_status(), darkburn_grbl::machine::LaserStatus::Ready);
}
