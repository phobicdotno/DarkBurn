//! The thread + channel API, as the egui app will use it.

mod common;

use std::time::{Duration, Instant};

use common::*;
use darkburn_ruida::settings::addr;
use darkburn_ruida::*;

const T: Duration = Duration::from_secs(10);

fn handle(status: Option<Duration>) -> (RuidaHandle, Emu) {
    let emu = emulator(EmulatorConfig::default());
    let m: RuidaMachine<Box<dyn Transport>> = RuidaMachine::new(Box::new(MemoryTransport::udp(emu.clone())), MachineConfig::default());
    (RuidaHandle::spawn(m, WorkerConfig { status_interval: status }), emu)
}

#[test]
fn requests_need_a_connection() {
    let (mut h, _emu) = handle(None);
    let id = h.request(Request::Status);
    assert_eq!(h.wait(id, T, &mut never), Err(RuidaError::NotConnected));
}

#[test]
fn connect_send_list_and_run() {
    let (mut h, emu) = handle(None);
    let id = h.request(Request::Connect);
    let Response::Connected(info) = h.wait(id, T, &mut never).unwrap() else { panic!() };
    assert_eq!(info.card_id, 0x6510_6510);

    let id = h.request(Request::SendJob { name: "W".into(), job: Box::new(square_job((1.0, 1.0), 5.0, 10.0)), options: SendOptions::default() });
    let Response::Sent(out) = h.wait(id, T, &mut never).unwrap() else { panic!() };
    assert_eq!(out.index, 1);

    let id = h.request(Request::ListFiles);
    assert_eq!(h.wait(id, T, &mut never).unwrap(), Response::Files(vec![ControllerFile { index: 1, name: "W".into() }]));

    let id = h.request(Request::RunFile(1));
    assert_eq!(h.wait(id, T, &mut never).unwrap(), Response::Done);
    assert_eq!(lock(&emu).motion.iter().filter(|r| r.kind == MotionKind::Cut).count(), 4);

    let id = h.request(Request::ReadSetting(addr::BED_X));
    assert_eq!(h.wait(id, T, &mut never).unwrap(), Response::Setting(600_000));

    // Progress events were sent during the upload.
    assert!(h.poll().iter().any(|e| matches!(e, Event::Progress { .. })));
}

#[test]
fn overwrite_prompt_via_poll() {
    let (mut h, emu) = handle(None);
    let id = h.request(Request::Connect);
    h.wait(id, T, &mut never).unwrap();
    let job = square_job((1.0, 1.0), 5.0, 10.0);
    let id = h.request(Request::SendJob { name: "DUP".into(), job: Box::new(job.clone()), options: SendOptions::default() });
    h.wait(id, T, &mut never).unwrap();

    // Second send with the same name: the UI sees a prompt in poll() and answers it.
    let id = h.request(Request::SendJob { name: "DUP".into(), job: Box::new(job), options: SendOptions::default() });
    // A request sent while the prompt is open waits its turn.
    let queued = h.request(Request::ListFiles);
    let deadline = Instant::now() + T;
    let mut answered = false;
    let mut reply = None;
    let mut files = None;
    while Instant::now() < deadline && (reply.is_none() || files.is_none()) {
        for e in h.poll() {
            match e {
                Event::OverwritePrompt { id: pid, name } => {
                    assert_eq!((pid, name.as_str()), (id, "DUP"));
                    h.answer_overwrite(pid, OverwriteChoice::Rename);
                    answered = true;
                }
                Event::Reply { id: rid, result } if rid == id => reply = Some(result),
                Event::Reply { id: rid, result } if rid == queued => files = Some(result),
                _ => {}
            }
        }
        std::thread::yield_now();
    }
    assert!(answered);
    let Response::Sent(out) = reply.unwrap().unwrap() else { panic!() };
    assert_eq!(out.name, "DUP1");
    assert!(files.unwrap().is_ok());
    assert_eq!(lock(&emu).files().len(), 2);
}

#[test]
fn status_is_polled_while_connected() {
    let (mut h, _emu) = handle(Some(Duration::from_millis(5)));
    let id = h.request(Request::Connect);
    h.wait(id, T, &mut never).unwrap();
    let deadline = Instant::now() + T;
    let mut got = false;
    while Instant::now() < deadline && !got {
        got = h.poll().iter().any(|e| matches!(e, Event::Status(s) if s.state == MachineState::Idle));
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(got);
}

#[test]
fn many_requests_in_order() {
    let (mut h, emu) = handle(None);
    let ids = [
        h.request(Request::Connect),
        h.request(Request::Home(HomeAxes::XY)),
        h.request(Request::MoveTo { to: kurbo::Point::new(10.0, 10.0), light: false }),
        h.request(Request::JogStep { axis: Axis::X, distance_mm: 1.0 }),
        h.request(Request::JogStart { axis: Axis::Y, positive: true }),
        h.request(Request::JogStop { axis: Axis::Y, positive: true }),
        h.request(Request::WriteSetting { addr: addr::G0_VELOCITY, value: 123 }),
        h.request(Request::BackupSettings),
        h.request(Request::ControllerInfo),
        h.request(Request::ReadUAxis),
        h.request(Request::ReadLaser2),
        h.request(Request::CalcTime(1)),
        h.request(Request::Raw(vec![Command::Stop])),
        h.request(Request::Status),
    ];
    let results: Vec<_> = ids.iter().map(|&id| h.wait(id, T, &mut never)).collect();
    let Ok(Response::Settings(backup)) = &results[7] else { panic!("{:?}", results[7]) };
    assert_eq!(backup.get(addr::G0_VELOCITY), Some(123));
    let Ok(Response::Status(s)) = &results[13] else { panic!() };
    assert_eq!((s.x, s.y), (11_000, 20_000));
    assert_eq!(lock(&emu).position()[..2], [11_000, 20_000]);
    h.shutdown();
    assert!(h.poll().contains(&Event::Stopped));
}
