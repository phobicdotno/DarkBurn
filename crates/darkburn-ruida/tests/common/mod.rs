#![allow(dead_code)]

use std::sync::{Arc, Mutex};

use darkburn_ruida::*;
use kurbo::Point;

pub type Emu = Arc<Mutex<RuidaEmulator>>;

pub fn emulator(cfg: EmulatorConfig) -> Emu {
    Arc::new(Mutex::new(RuidaEmulator::new(cfg)))
}

/// A UDP-framed machine wired to a fresh default emulator, connected.
pub fn udp_machine() -> (RuidaMachine<MemoryTransport>, Emu) {
    udp_machine_with(EmulatorConfig::default(), MachineConfig::default())
}

pub fn udp_machine_with(ecfg: EmulatorConfig, mcfg: MachineConfig) -> (RuidaMachine<MemoryTransport>, Emu) {
    let emu = emulator(ecfg);
    let mut m = RuidaMachine::new(MemoryTransport::udp(emu.clone()), mcfg);
    m.connect().expect("connect to emulator");
    (m, emu)
}

pub fn stream_machine() -> (RuidaMachine<MemoryTransport>, Emu) {
    let emu = emulator(EmulatorConfig::default());
    let mut m = RuidaMachine::new(MemoryTransport::stream(emu.clone()), MachineConfig::default());
    m.connect().expect("connect to emulator over stream");
    (m, emu)
}

pub fn lock(e: &Emu) -> std::sync::MutexGuard<'_, RuidaEmulator> {
    e.lock().unwrap()
}

pub fn never(_: &str) -> OverwriteChoice {
    panic!("overwrite prompt was not expected")
}

/// A square cut on one layer.
pub fn square_job(origin: (f64, f64), size: f64, speed: f64) -> RdJob {
    let (x, y) = origin;
    RdJob {
        layers: vec![RdLayer {
            settings: LayerSettings { index: 0, speed_mm_s: speed, color: 0xFF0000, ..Default::default() },
            ops: vec![
                RdOp::Move(Point::new(x, y)),
                RdOp::Cut { to: Point::new(x + size, y), power: None, speed: None },
                RdOp::Cut { to: Point::new(x + size, y + size), power: None, speed: None },
                RdOp::Cut { to: Point::new(x, y + size), power: None, speed: None },
                RdOp::Cut { to: Point::new(x, y), power: None, speed: None },
            ],
        }],
    }
}

/// A two-layer job using every op kind.
pub fn mixed_job() -> RdJob {
    RdJob {
        layers: vec![
            RdLayer {
                settings: LayerSettings {
                    index: 0,
                    speed_mm_s: 25.0,
                    min_power: [12.0, 10.0],
                    max_power: [65.0, 50.0],
                    color: 0x00FF00,
                    frequency_hz: Some(5_000),
                    air_assist: true,
                    dot_mode: None,
                },
                ops: vec![
                    RdOp::Move(Point::new(20.0, 20.0)),
                    RdOp::Cut { to: Point::new(22.5, 20.0), power: None, speed: None },
                    RdOp::Cut { to: Point::new(22.5, 25.75), power: Some(80.0), speed: Some(12.0) },
                    RdOp::Cut { to: Point::new(120.0, 180.0), power: None, speed: None },
                    RdOp::Move(Point::new(0.5, 0.25)),
                ],
            },
            RdLayer {
                settings: LayerSettings {
                    index: 3,
                    speed_mm_s: 150.0,
                    min_power: [8.0, 0.0],
                    max_power: [18.0, 0.0],
                    color: 0x123456,
                    frequency_hz: None,
                    air_assist: false,
                    dot_mode: Some(DotMode { dot_time_ms: 1.25, spacing_mm: 0.5 }),
                },
                ops: vec![
                    RdOp::Z(3.0),
                    RdOp::U(-7.5),
                    RdOp::Move(Point::new(50.0, 50.0)),
                    RdOp::Cut { to: Point::new(50.0, 58.0), power: None, speed: None },
                    RdOp::Cut { to: Point::new(42.0, 58.0), power: None, speed: None },
                ],
            },
        ],
    }
}
