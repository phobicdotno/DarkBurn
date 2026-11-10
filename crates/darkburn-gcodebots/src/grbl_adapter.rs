//! [`GrblAdapter`]: makes `darkburn_grbl::GrblMachine` a [`GcodeMachine`].
//!
//! The adapter adds nothing to the GRBL driver; it renames types, maps events and keeps the
//! profile's air-assist commands.

use std::collections::BTreeMap;
use std::time::Duration;

use darkburn_grbl::{
    ConnectOptions, ConsoleLine, Event, FirmwareKind, FrameOptions, GrblMachine, Job, JogStep, MachineStatus, MoveTarget, Snapshot, StreamMode,
};
use kurbo::Point;

use crate::machine::{Capabilities, GcodeMachine, MachineError, MachineEvent, MachineFamily, MachineSnapshot};
use crate::profile::{DeviceProfile, TransferMode};

/// A GRBL, grblHAL or FluidNC controller behind the common interface.
pub struct GrblAdapter {
    machine: GrblMachine,
    air_on: String,
    air_off: String,
}

/// GRBL connect options for a profile (DS-11, DS-12, DS-13).
pub fn connect_options(p: &DeviceProfile) -> ConnectOptions {
    let g = &p.gcode;
    ConnectOptions {
        dtr: g.enable_dtr,
        toggle_dtr: g.enable_dtr,
        stream_mode: match g.transfer {
            TransferMode::Buffered => StreamMode::CharacterCounting,
            TransferMode::Synchronous => StreamMode::Synchronous,
        },
        s_max: Some(g.s_max),
        travel: Some(p.travel_limits()),
        max_fire_percent: g.max_fire_percent,
        max_fire_duration: p.fire_duration(),
        ..ConnectOptions::default()
    }
}

impl GrblAdapter {
    /// Connects with options derived from the profile.
    pub fn connect(transport: Box<dyn darkburn_grbl::Transport>, p: &DeviceProfile) -> Self {
        Self::with_options(transport, connect_options(p), &p.gcode.air_assist.on, &p.gcode.air_assist.off)
    }

    pub fn with_options(transport: Box<dyn darkburn_grbl::Transport>, opts: ConnectOptions, air_on: &str, air_off: &str) -> Self {
        Self { machine: GrblMachine::connect(transport, opts), air_on: air_on.to_owned(), air_off: air_off.to_owned() }
    }

    /// Wraps a machine that is already connected.
    pub fn from_machine(machine: GrblMachine) -> Self {
        Self { machine, air_on: "M8".into(), air_off: "M9".into() }
    }

    /// The wrapped driver, for GRBL-only features (settings backups, show-all, ...).
    pub fn inner(&self) -> &GrblMachine {
        &self.machine
    }
}

fn map_event(e: Event) -> MachineEvent {
    match e {
        Event::Connected(fw) => MachineEvent::Connected(fw.display_name()),
        Event::ConnectFailed(m) => MachineEvent::ConnectFailed(m),
        Event::Disconnected(r) => MachineEvent::Disconnected(r),
        Event::StateChanged(s) => MachineEvent::StateChanged(s),
        Event::Console(c) => MachineEvent::Console(ConsoleLine { kind: c.kind, text: c.text }),
        Event::Alarm { code, message } => MachineEvent::Alarm(format!("ALARM:{code} {message}")),
        Event::Error { code, message, line } => MachineEvent::Error { message: format!("error:{code} {message}"), line },
        Event::Settings(v) => MachineEvent::Settings(v.into_iter().map(|s| (format!("${}", s.number), s.value)).collect()),
        Event::SettingWritten { number, value, result } => MachineEvent::SettingWritten { key: format!("${number}"), value, result },
        Event::JobStarted(_) => MachineEvent::JobStarted,
        Event::JobProgress(p) => MachineEvent::JobProgress(p),
        Event::JobFinished(p) => MachineEvent::JobFinished(p),
        Event::JobLineError { line_number, line, code, message } => {
            MachineEvent::JobLineError { line_number, line, message: format!("error:{code} {message}") }
        }
        Event::JobAborted { progress, reason } => MachineEvent::JobAborted { progress, reason },
        Event::Stopped => MachineEvent::Stopped,
        Event::Homed => MachineEvent::Homed,
        Event::FireOff => MachineEvent::FireOff,
        Event::Rejected(m) => MachineEvent::Rejected(m),
    }
}

fn pos3(a: [f64; 4]) -> [f64; 3] {
    [a[0], a[1], a[2]]
}

fn map_snapshot(s: Snapshot) -> MachineSnapshot {
    let family = s.firmware.as_ref().map(|f| match f.kind {
        FirmwareKind::Grbl => MachineFamily::Grbl,
        FirmwareKind::GrblHal => MachineFamily::GrblHal,
        FirmwareKind::FluidNc => MachineFamily::FluidNc,
    });
    let MachineStatus { state, mpos, wpos, feed, spindle, accessories, updated, .. } = s.status.clone();
    let alarm = if s.alarm_locked || state.is_alarm() {
        Some(s.last_alarm.as_ref().map_or_else(|| "Alarm".to_owned(), |(c, m)| format!("ALARM:{c} {m}")))
    } else {
        None
    };
    MachineSnapshot {
        connection: s.connection,
        port: s.port,
        family,
        firmware: s.firmware.as_ref().map(|f| f.display_name()).unwrap_or_default(),
        state,
        mpos: pos3(mpos),
        wpos: pos3(wpos),
        feed,
        spindle,
        activity: s.activity,
        paused: s.paused,
        job: s.job,
        settings: s.settings.iter().map(|(k, v)| (format!("${k}"), v.clone())).collect::<BTreeMap<_, _>>(),
        fire_on: s.fire_on,
        air_on: accessories.contains('F') || accessories.contains('M'),
        alarm,
        updated,
    }
}

impl GcodeMachine for GrblAdapter {
    fn family(&self) -> MachineFamily {
        self.machine.snapshot().firmware.map_or(MachineFamily::Grbl, |f| match f.kind {
            FirmwareKind::Grbl => MachineFamily::Grbl,
            FirmwareKind::GrblHal => MachineFamily::GrblHal,
            FirmwareKind::FluidNc => MachineFamily::FluidNc,
        })
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            hardware_pause: true,
            jog_cancel: true,
            overrides: true,
            reports_state: true,
            read_settings: true,
            write_settings: true,
            air_assist: !self.air_on.trim().is_empty(),
            fire: true,
            home: true,
            user_origin: true,
        }
    }

    fn poll_events(&self) -> Vec<MachineEvent> {
        self.machine.poll_events().into_iter().map(map_event).collect()
    }

    fn next_event(&self, timeout: Duration) -> Option<MachineEvent> {
        self.machine.next_event(timeout).map(map_event)
    }

    fn snapshot(&self) -> MachineSnapshot {
        map_snapshot(self.machine.snapshot())
    }

    fn start_job(&self, job: Job) -> Result<(), MachineError> {
        Ok(self.machine.start_job(job)?)
    }
    fn pause(&self) -> Result<(), MachineError> {
        Ok(self.machine.pause()?)
    }
    fn resume(&self) -> Result<(), MachineError> {
        Ok(self.machine.resume()?)
    }
    fn stop(&self) -> Result<(), MachineError> {
        Ok(self.machine.stop()?)
    }
    fn emergency_stop(&self) -> Result<(), MachineError> {
        Ok(self.machine.soft_reset()?)
    }
    fn frame_outline(&self, points: &[Point], opts: FrameOptions) -> Result<(), MachineError> {
        Ok(self.machine.frame_outline(points, opts)?)
    }
    fn jog(&self, step: JogStep) -> Result<(), MachineError> {
        Ok(self.machine.jog(step)?)
    }
    fn jog_cancel(&self) -> Result<(), MachineError> {
        Ok(self.machine.jog_cancel()?)
    }
    fn move_to(&self, target: MoveTarget) -> Result<(), MachineError> {
        Ok(self.machine.move_to(target)?)
    }
    fn go_to_origin(&self) -> Result<(), MachineError> {
        Ok(self.machine.go_to_origin()?)
    }
    fn home(&self) -> Result<(), MachineError> {
        Ok(self.machine.home()?)
    }
    fn unlock(&self) -> Result<(), MachineError> {
        Ok(self.machine.unlock()?)
    }
    fn set_user_origin(&self) -> Result<(), MachineError> {
        Ok(self.machine.set_user_origin()?)
    }
    fn clear_user_origin(&self) -> Result<(), MachineError> {
        Ok(self.machine.clear_user_origin()?)
    }
    fn request_status(&self) -> Result<(), MachineError> {
        Ok(self.machine.request_status()?)
    }
    fn fire(&self, percent: f64, duration: Duration) -> Result<(), MachineError> {
        Ok(self.machine.fire(percent, duration)?)
    }
    fn fire_off(&self) -> Result<(), MachineError> {
        Ok(self.machine.fire_off()?)
    }
    fn set_air(&self, on: bool) -> Result<(), MachineError> {
        let cmd = if on { &self.air_on } else { &self.air_off };
        if cmd.trim().is_empty() {
            return Err(MachineError::Unsupported("air assist"));
        }
        Ok(self.machine.run_accessory(cmd)?)
    }
    fn set_feed_override(&self, percent: u16) -> Result<(), MachineError> {
        Ok(self.machine.set_feed_override(percent)?)
    }
    fn set_power_override(&self, percent: u16) -> Result<(), MachineError> {
        Ok(self.machine.set_power_override(percent)?)
    }
    fn send_console(&self, line: &str) -> Result<(), MachineError> {
        Ok(self.machine.send_console(line)?)
    }
    fn run_macro(&self, text: &str) -> Result<(), MachineError> {
        Ok(self.machine.run_macro(text)?)
    }
    fn read_settings(&self) -> Result<(), MachineError> {
        Ok(self.machine.read_settings()?)
    }
    fn write_setting(&self, key: &str, value: &str) -> Result<(), MachineError> {
        let n: u16 = key
            .trim()
            .trim_start_matches('$')
            .parse()
            .map_err(|_| MachineError::InvalidArgument(format!("'{key}' is not a GRBL setting like $30.")))?;
        Ok(self.machine.write_setting(n, value)?)
    }
    fn disconnect(self: Box<Self>) {
        self.machine.disconnect();
    }
}
