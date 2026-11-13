//! The Gweike MCC100 card commands (`mcc.*`) [MCC-05].
//!
//! Every one of them works on the in-memory card simulator (`darkburn_mcc::SimLink`) that the app
//! holds in [`CardState`]. There is no transport here or in anything this file calls: the card is
//! a struct in this process. Connect only reads (the prologue is never sent), status only reads,
//! and the stops send the driver's own stop sequences, which never write licence registers 150 or
//! 151, never write register 0x65 from 0x65 or 0x66, and leave the height follower, piercing and
//! gas logic to the machine. Nothing here moves an axis, fires, opens gas or writes settings, and
//! there is no remote-monitor or cloud connection.
//!
//! `mcc.dissect` only reads the bytes it is given and needs no card.

use darkburn_mcc as mcc;
use mcc::registers::{self, Status};
use mcc::sim::{ConnectOptions, ConnectReport, SimLink};
use serde_json::{Value, json};

use crate::commands::Cmd;
use crate::shell::DarkBurn;

const fn a(id: &'static str, label: &'static str, args: &'static str) -> Cmd {
    Cmd { id, label, key: "", icon: "", args }
}

pub const CONNECT: Cmd = a(
    "mcc.connect",
    "Connect to the Gweike card simulator (read only)",
    r#"{"type":"object","properties":{"min_version":{"type":"integer","description":"lowest card program version to accept"}}}"#,
);
pub const DISCONNECT: Cmd = a("mcc.disconnect", "Disconnect from the Gweike card simulator", "");
pub const STATUS: Cmd = a("mcc.status", "Read the Gweike card status", "");
pub const DISSECT: Cmd = a(
    "mcc.dissect",
    "Explain a Gweike controller frame",
    r#"{"type":"object","properties":{"hex":{"type":"string","description":"the frame bytes in hex"},"from":{"enum":["pc","card"]}},"required":["hex"]}"#,
);
pub const STOP: Cmd = a("mcc.stop", "Stop the Gweike card simulator", "");
pub const EMERGENCY_STOP: Cmd = a("mcc.emergency_stop", "Emergency stop the Gweike card simulator", "");

/// The `mcc.*` commands, in menu order.
pub const COMMANDS: [Cmd; 6] = [CONNECT, DISCONNECT, STATUS, DISSECT, STOP, EMERGENCY_STOP];

/// Every `mcc.*` command but the dissector acts on the (simulated) card.
pub fn is_hardware(id: &str) -> bool {
    id.starts_with("mcc.") && id != "mcc.dissect"
}

/// The simulated card the app holds while connected.
#[derive(Default)]
pub struct CardState {
    link: Option<SimLink>,
    report: Option<ConnectReport>,
}

impl CardState {
    pub fn connected(&self) -> bool {
        self.link.is_some()
    }
}

const NOT_CONNECTED: &str = "the Gweike card simulator is not connected; run mcc.connect first";

/// Run one of the `mcc.*` commands.
pub fn run(app: &mut DarkBurn, id: &str, a: &Value) -> Result<Value, String> {
    let card = &mut app.machine.ext.mcc;
    match id {
        "mcc.connect" => connect(card, a),
        "mcc.dissect" => dissect(a),
        "mcc.disconnect" => {
            let mut link = card.link.take().ok_or(NOT_CONNECTED)?;
            card.report = None;
            let steps = link.disconnect();
            Ok(json!({ "ok": true, "stop_steps": steps }))
        }
        "mcc.status" => status(card.link.as_mut().ok_or(NOT_CONNECTED)?),
        "mcc.stop" => {
            let link = card.link.as_mut().ok_or(NOT_CONNECTED)?;
            let steps = link.stop_sequence(false);
            Ok(json!({ "ok": true, "stop_steps": steps, "arm_state": format!("{:?}", link.gate.arming.state()) }))
        }
        "mcc.emergency_stop" => {
            let link = card.link.as_mut().ok_or(NOT_CONNECTED)?;
            // No laser-gate output is set up for the simulator, so port 0.
            let steps = link.emergency_stop(0);
            Ok(json!({ "ok": true, "stop_steps": steps, "arm_state": format!("{:?}", link.gate.arming.state()) }))
        }
        other => Err(format!("{other} is not a Gweike card command")),
    }
}

fn connect(card: &mut CardState, a: &Value) -> Result<Value, String> {
    if card.connected() {
        return Err("the Gweike card simulator is already connected; disconnect first".into());
    }
    let min_version = match a.get("min_version") {
        None | Some(Value::Null) => registers::MIN_PROGRAM_VERSION,
        Some(v) => v.as_u64().and_then(|v| u32::try_from(v).ok()).ok_or("argument min_version must be a whole number")?,
    };
    let mut link = SimLink::new(mcc::CardSimulator::new(mcc::SimConfig::default()));
    let report = link.connect(ConnectOptions { min_version, send_prologue: false }).map_err(|e| format!("could not connect: {e}"))?;
    let out = json!({
        "ok": true,
        "simulator": true,
        "program_id": report.version.program_id,
        "program_version": report.version.program_version,
        "bus_cycle_us": report.system.bus_cycle_us(),
        "has_follower": report.system.has_follower(),
        "arm_state": format!("{:?}", link.gate.arming.state()),
        "alarms": registers::alarm_labels(report.status.alarm1(), report.status.alarm2()),
    });
    card.link = Some(link);
    card.report = Some(report);
    Ok(out)
}

fn status(link: &mut SimLink) -> Result<Value, String> {
    let lerr = |e: mcc::sim::LinkError| format!("could not read the card: {e}");
    let st = link.poll_status().map_err(lerr)?;
    let axis_ro = link.read_axis_ro().map_err(lerr)?;
    let (a1, a2) = (st.alarm1(), st.alarm2());
    let alarms = registers::alarm_labels(a1, a2);
    let codes: Vec<Value> =
        registers::alarm_codes(a1, a2, Some(&axis_ro.words)).into_iter().map(|c| json!({ "code": c.code, "label": c.label })).collect();
    let axis_faults: Vec<Value> = (0..registers::AXIS_SLOTS)
        .filter_map(|slot| {
            let names = registers::axis::fault_names(axis_ro.status(slot));
            (!names.is_empty()).then(|| json!({ "slot": slot, "faults": names }))
        })
        .collect();
    let state = registers::derive_machine_state(&st, &axis_ro.words);
    let follower = match link.read_system() {
        Ok(sys) if sys.has_follower() => link.read_follower().ok().map(|z| json!({ "state": format!("{:?}", z.state()), "alarms": z.alarm_names() })),
        _ => None,
    };
    let fault = link.gate.machine_fault.clone();
    let held = link.gate.queue_held();
    let text = if alarms.is_empty() && fault.is_none() {
        format!("{state:?}, no alarms")
    } else {
        let mut parts = alarms.clone();
        if let Some(f) = &fault {
            parts.push(f.clone());
        }
        format!("{state:?}, alarms: {}", parts.join("; "))
    };
    Ok(json!({
        "text": text,
        "machine_state": format!("{state:?}"),
        "program_version": st.word(Status::ProgramVersion),
        "fifo_running": st.fifo_running(),
        "fifo_frame_id": st.fifo_frame_id(),
        "fifo_space_margin": st.fifo_space_margin(),
        "restart_required": st.restart_required(),
        "inputs": st.word(Status::Di),
        "outputs": st.word(Status::Do),
        "alarm1": a1,
        "alarm2": a2,
        "alarms": alarms,
        "alarm_codes": codes,
        "axis_faults": axis_faults,
        "follower": follower,
        "arm_state": format!("{:?}", link.gate.arming.state()),
        "machine_fault": fault,
        "queue_held": held,
    }))
}

/// Bytes from hex text; spaces, colons, dashes and a leading `0x` are allowed.
fn parse_hex(s: &str) -> Result<Vec<u8>, String> {
    let s = s.trim();
    let s = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")).unwrap_or(s);
    let digits: Vec<u8> = s.bytes().filter(|b| !matches!(b, b' ' | b'\t' | b'\n' | b'\r' | b':' | b'-' | b',')).collect();
    if digits.is_empty() {
        return Err("argument hex is empty".into());
    }
    if digits.len() % 2 != 0 {
        return Err("argument hex has an odd number of digits".into());
    }
    digits
        .chunks(2)
        .map(|p| {
            let t = std::str::from_utf8(p).map_err(|_| "argument hex is not hex".to_string())?;
            u8::from_str_radix(t, 16).map_err(|_| format!("argument hex is not hex: {t}"))
        })
        .collect()
}

fn dissect(a: &Value) -> Result<Value, String> {
    let hex = a.get("hex").and_then(Value::as_str).ok_or("argument hex must be text")?;
    let bytes = parse_hex(hex)?;
    let from = a.get("from").and_then(Value::as_str).unwrap_or("pc");
    let d = match from {
        "pc" => mcc::dissect::dissect_request(&bytes),
        "card" => mcc::dissect::dissect_reply(&bytes),
        _ => return Err("argument from must be pc or card".into()),
    };
    Ok(json!({
        "text": d.text,
        "from": from,
        "seq": d.seq,
        "func": d.func,
        "crc_ok": d.crc_ok,
        "length_ok": d.length_ok,
        "vector": d.vector,
        "verdict": format!("{:?}", d.verdict),
    }))
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use crate::api::execute_headless;
    use crate::prefs::Prefs;
    use crate::shell::DarkBurn;

    fn app() -> DarkBurn {
        DarkBurn::new(Prefs::default())
    }

    fn run(app: &mut DarkBurn, id: &str, args: Value) -> Result<Value, String> {
        execute_headless(app, id, &args)
    }

    #[test]
    fn gweike_card_commands_are_registered_with_the_right_hardware_flags() {
        for id in ["mcc.connect", "mcc.disconnect", "mcc.status", "mcc.stop", "mcc.emergency_stop"] {
            assert!(crate::commands::find(id).is_some(), "{id} is not registered");
            assert!(crate::commands::hardware(id), "{id} must be a hardware command");
        }
        assert!(crate::commands::find("mcc.dissect").is_some());
        assert!(!crate::commands::hardware("mcc.dissect"), "the dissector only reads bytes it is given");
    }

    #[test]
    fn gweike_card_commands_refuse_when_not_connected() {
        let mut a = app();
        for id in ["mcc.disconnect", "mcc.status", "mcc.stop", "mcc.emergency_stop"] {
            let e = run(&mut a, id, json!({})).expect_err(id);
            assert!(e.contains("not connected"), "{id}: {e}");
        }
    }

    #[test]
    fn gweike_card_connect_status_and_disconnect_on_the_simulator() {
        let mut a = app();
        let c = run(&mut a, "mcc.connect", json!({})).unwrap();
        assert_eq!(c["simulator"], true);
        assert!(c["program_version"].as_u64().unwrap() >= u64::from(darkburn_mcc::registers::MIN_PROGRAM_VERSION));
        assert_eq!(c["arm_state"], "Disarmed", "connect leaves the gate disarmed");
        assert!(run(&mut a, "mcc.connect", json!({})).unwrap_err().contains("already connected"));
        let s = run(&mut a, "mcc.status", json!({})).unwrap();
        assert_eq!(s["alarms"], json!([]), "{s}");
        assert_eq!(s["fifo_running"], false);
        assert_eq!(s["outputs"], 0, "connect wrote no outputs");
        assert!(s["text"].as_str().unwrap().contains("no alarms"), "{s}");
        // Connect and status only read: the card saw no writes at all.
        let card = a.machine.ext.mcc.link.as_ref().unwrap();
        assert!(card.sim.commands().is_empty(), "connect or status wrote to the card: {:?}", card.sim.commands());
        let d = run(&mut a, "mcc.disconnect", json!({})).unwrap();
        assert_eq!(d["ok"], true);
        assert!(!a.machine.ext.mcc.connected());
        assert!(run(&mut a, "mcc.status", json!({})).unwrap_err().contains("not connected"));
    }

    #[test]
    fn gweike_card_connect_refuses_an_old_card_and_bad_arguments() {
        let mut a = app();
        let e = run(&mut a, "mcc.connect", json!({ "min_version": 999_999 })).unwrap_err();
        assert!(e.contains("older than 999999"), "{e}");
        assert!(!a.machine.ext.mcc.connected(), "a refused connect left a link behind");
        assert!(run(&mut a, "mcc.connect", json!({ "min_version": "new" })).is_err());
        assert!(run(&mut a, "mcc.connect", json!({ "min_version": -1 })).is_err());
        run(&mut a, "mcc.connect", json!({ "min_version": 1 })).unwrap();
    }

    #[test]
    fn gweike_card_status_names_the_alarms_in_words() {
        let mut a = app();
        run(&mut a, "mcc.connect", json!({})).unwrap();
        let link = a.machine.ext.mcc.link.as_mut().unwrap();
        link.sim.inject_alarm(Some(darkburn_mcc::registers::alarm1::ESTOP), None);
        let s = run(&mut a, "mcc.status", json!({})).unwrap();
        let labels = s["alarms"].as_array().unwrap();
        assert!(!labels.is_empty(), "{s}");
        let want = darkburn_mcc::registers::alarm_labels(darkburn_mcc::registers::alarm1::ESTOP, 0);
        assert_eq!(labels, &want.iter().map(|l| json!(l)).collect::<Vec<_>>());
        assert!(s["text"].as_str().unwrap().contains(&want[0]), "{s}");
        assert!(s["machine_fault"].is_string(), "the gate did not see the alarm: {s}");
    }

    #[test]
    fn gweike_card_stop_and_emergency_stop_reach_the_simulator() {
        let mut a = app();
        run(&mut a, "mcc.connect", json!({})).unwrap();
        let s = run(&mut a, "mcc.stop", json!({})).unwrap();
        assert!(s["stop_steps"].as_u64().unwrap() > 0, "{s}");
        let e = run(&mut a, "mcc.emergency_stop", json!({})).unwrap();
        assert!(e["stop_steps"].as_u64().unwrap() > 0, "{e}");
        let link = a.machine.ext.mcc.link.as_ref().unwrap();
        assert_eq!(link.sim.outputs(), 0);
        assert!(link.sim.forbidden_attempts().is_empty());
        // No stop ever writes 0x65 <- [0x65] or [0x66], or the licence registers.
        for v in link.sim.commands() {
            assert!(v.as_slice() != [0x65] && v.as_slice() != [0x66], "{v:?}");
        }
        for r in link.sim.requests() {
            if let Some(v) = &r.vector {
                assert!(!matches!(v.as_slice(), [0x40, 150 | 151, ..]), "a stop wrote a licence register: {v:?}");
            }
        }
        assert_ne!(e["arm_state"], "MotionArmed");
    }

    #[test]
    fn gweike_card_dissect_explains_a_frame_without_a_card() {
        let mut a = app();
        assert!(!a.machine.ext.mcc.connected());
        let req = darkburn_mcc::Request::read(1000, 2).unwrap().encode(7).unwrap();
        let hex: String = req.iter().map(|b| format!("{b:02x} ")).collect();
        let d = run(&mut a, "mcc.dissect", json!({ "hex": hex })).unwrap();
        assert_eq!(d["seq"], 7, "{d}");
        assert_eq!(d["crc_ok"], true, "{d}");
        assert!(!d["text"].as_str().unwrap().is_empty());
        let r = run(&mut a, "mcc.dissect", json!({ "hex": hex, "from": "card" })).unwrap();
        assert_eq!(r["from"], "card");
        assert!(run(&mut a, "mcc.dissect", json!({ "hex": "abc" })).is_err());
        assert!(run(&mut a, "mcc.dissect", json!({ "hex": "zz" })).is_err());
        assert!(run(&mut a, "mcc.dissect", json!({ "hex": "" })).is_err());
        assert!(run(&mut a, "mcc.dissect", json!({ "hex": hex, "from": "cloud" })).is_err());
        assert!(!a.machine.ext.mcc.connected(), "the dissector connected");
    }

    #[test]
    fn gweike_card_code_reaches_no_real_transport() {
        let forbidden = ["std::net", "UdpSocket", "TcpStream", "TcpListener", "serialport", "nusb", "rusb", "std::process", "reqwest", "open_real"];
        let src = include_str!("gweike_card.rs");
        let code = src.split("#[cfg(test)]").next().unwrap();
        for (i, line) in code.lines().enumerate() {
            let line = line.split("//").next().unwrap_or("");
            for f in forbidden {
                assert!(!line.contains(f), "gweike_card.rs:{} mentions {f}", i + 1);
            }
        }
    }
}
