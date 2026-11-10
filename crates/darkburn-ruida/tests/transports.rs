//! The real transports against the emulator: UDP over the loopback address (sockets the test
//! opens itself on 127.0.0.1, ports picked by the system) and the USB byte stream over an
//! in-memory pipe. Nothing here reaches a network card or a USB device.

mod common;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::*;
use darkburn_ruida::emulator::MotionKind;
use darkburn_ruida::settings::addr;
use darkburn_ruida::*;

fn loopback(ecfg: EmulatorConfig, mcfg: MachineConfig) -> (RuidaMachine<UdpTransport>, UdpEmulatorServer) {
    let server = UdpEmulatorServer::start(emulator(ecfg)).expect("loopback sockets");
    let m = RuidaMachine::new(warm(&server), mcfg);
    (m, server)
}

/// A transport to `server` whose first datagram has already made the trip. On Windows the first
/// datagram between two new loopback sockets can take from 20 ms to more than a second when
/// many sockets open at once (measured here; later datagrams take microseconds). Tests that use
/// short timeouts to keep the suite quick start from a link that is past that. The driver's own
/// answer to late datagrams is tested without sockets in `tests/driver.rs`.
fn warm(server: &UdpEmulatorServer) -> UdpTransport {
    let mut t = server.transport().expect("loopback transport");
    // A lone ENQ, scrambled with the server's own magic: it is acknowledged and changes nothing.
    let magic = server.emulator().lock().unwrap().magic();
    t.send(Channel::Data, &emulator::udp_datagram(&[0xCE], magic)).unwrap();
    assert!(t.recv(Channel::Data, Duration::from_secs(20)).unwrap().is_some(), "the loopback server answers");
    t
}

#[test]
fn udp_ports_are_the_documented_ones_and_loopback_ports_are_not() {
    assert_eq!((packet::DATA_PORT, packet::HOST_DATA_PORT, packet::JOG_PORT, packet::HOST_JOG_PORT), (50200, 40200, 50207, 40207));
    let server = UdpEmulatorServer::start(emulator(EmulatorConfig::default())).unwrap();
    let p = server.ports();
    assert!(p.data != 0 && p.jog != 0 && p.data != p.jog);
    assert_eq!((p.host_data, p.host_jog), (0, 0), "the test side takes any free port");
    assert!(server.transport().unwrap().describe().contains("127.0.0.1"));
}

#[test]
fn udp_connect_job_files_and_status_over_loopback_sockets() {
    let (mut m, server) = loopback(EmulatorConfig::default(), MachineConfig::default());
    let info = m.connect().unwrap();
    assert_eq!(info.card_id, 0x6510_6510);
    assert_eq!(m.bed_size_mm(), Some((600.0, 400.0)));

    let job = square_job((10.0, 20.0), 30.0, 50.0);
    let out = m.send_job("NET", &job, &SendOptions { start_immediately: true, ..Default::default() }, &mut never).unwrap();
    assert!(out.started);
    assert_eq!(m.list_files().unwrap(), vec![ControllerFile { index: 1, name: "NET".into() }]);
    {
        let emu = server.emulator();
        let e = lock(&emu);
        let cuts: Vec<_> = e.motion.iter().filter(|r| r.kind == MotionKind::Cut).collect();
        assert_eq!(cuts.len(), 4);
        assert_eq!(cuts[0].to[..2], [40_000, 20_000]);
        assert_eq!(e.checksum_failures, 0);
        assert_eq!(e.file_sum_mismatches, 0);
    }
    let s = m.status().unwrap();
    assert_eq!((s.state, s.x, s.y), (MachineState::Idle, 10_000, 20_000));
    m.delete_files(&[1]).unwrap();
    assert!(m.list_files().unwrap().is_empty());
    assert!(server.datagrams() > 10);
}

#[test]
fn udp_jog_port_works_over_loopback_sockets() {
    let (mut m, server) = loopback(EmulatorConfig::default(), MachineConfig::default());
    m.connect().unwrap();
    m.panel_key(PanelKey::PlusX, true).unwrap();
    m.panel_key(PanelKey::PlusX, false).unwrap();
    m.jog_start(Axis::Y, true).unwrap();
    m.jog_stop(Axis::Y, true).unwrap();
    assert_eq!(server.emulator().lock().unwrap().position()[..2], [10_000, 10_000]);
}

#[test]
fn udp_big_job_is_chunked_and_acknowledged_datagram_by_datagram() {
    let (mut m, server) = loopback(EmulatorConfig::default(), MachineConfig::default());
    m.connect().unwrap();
    let mut job = square_job((1.0, 1.0), 5.0, 10.0);
    for i in 0..2000 {
        job.layers[0].ops.push(RdOp::Cut { to: kurbo::Point::new(1.0 + (i % 50) as f64, 1.0 + (i % 7) as f64), power: None, speed: None });
    }
    let before = server.datagrams();
    let out = m.send_job("BIG", &job, &SendOptions::default(), &mut never).unwrap();
    assert!(out.bytes > 5 * packet::DEFAULT_CHUNK);
    assert!(server.datagrams() - before >= out.bytes / packet::DEFAULT_CHUNK);
    let emu = server.emulator();
    let stored = lock(&emu).files()[0].data.clone();
    assert_eq!(RdJob::from_logical(&stored).unwrap(), job.quantized());
}

#[test]
fn udp_lost_replies_are_asked_for_again() {
    let mcfg = MachineConfig { timeout: Duration::from_millis(150), retries: 3, ..Default::default() };
    let (mut m, server) = loopback(EmulatorConfig::default(), mcfg);
    m.connect().unwrap();
    server.drop_replies(1);
    assert_eq!(m.read_setting(addr::BED_Y).unwrap(), 400_000);
    assert!(m.resends >= 1);
}

#[test]
fn udp_wrong_magic_times_out_and_probing_finds_it() {
    let ecfg = EmulatorConfig { magic: Magic(0x11), ..Default::default() };
    let mcfg = MachineConfig { timeout: Duration::from_millis(60), retries: 0, ..Default::default() };
    let (mut m, _server) = loopback(ecfg.clone(), mcfg);
    assert!(matches!(m.connect(), Err(RuidaError::Timeout(_))));
    // Probing, on sockets that have not carried a datagram yet and with a short wait per value:
    // the first answers may come long after their probe, and the magic is still found.
    let mcfg = MachineConfig { magic: None, probe_timeout: Duration::from_millis(60), ..Default::default() };
    let server = UdpEmulatorServer::start(emulator(ecfg)).unwrap();
    let mut m = RuidaMachine::new(server.transport().unwrap(), mcfg);
    m.connect().unwrap();
    assert_eq!(m.magic(), Magic(0x11));
    assert_eq!(m.read_setting(addr::BED_X).unwrap(), 600_000);
}

#[test]
fn the_loopback_server_answers_every_datagram_whenever_it_arrives() {
    // Datagrams at uneven gaps around 20 ms: none is lost and none waits for a poll.
    let server = UdpEmulatorServer::start(emulator(EmulatorConfig::default())).unwrap();
    let mut t = warm(&server);
    let enq = emulator::udp_datagram(&[0xCE], Magic::DEFAULT);
    let mut answered = 0;
    for i in 0..120u64 {
        std::thread::sleep(Duration::from_millis(15 + i % 11));
        t.send(Channel::Data, &enq).unwrap();
        if t.recv(Channel::Data, Duration::from_millis(500)).unwrap().is_some() {
            answered += 1;
        }
    }
    assert_eq!((answered, server.datagrams()), (120, 121), "120 and the warm-up");
}

#[test]
fn udp_silence_is_a_timeout_not_a_hang() {
    // A server that has stopped: the port is closed, nothing answers.
    let server = UdpEmulatorServer::start(emulator(EmulatorConfig::default())).unwrap();
    let t = warm(&server);
    drop(server);
    let mut m = RuidaMachine::new(t, MachineConfig { timeout: Duration::from_millis(50), retries: 1, ..Default::default() });
    let started = std::time::Instant::now();
    assert!(m.connect().is_err());
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(!m.is_connected());
}

/// A byte stream wired to an emulator that hands replies back a few bytes at a time, the way a
/// USB serial port does.
struct Pipe {
    emu: Emu,
    rx: Arc<Mutex<VecDeque<u8>>>,
    piece: usize,
}

impl ByteStream for Pipe {
    fn write_all(&mut self, data: &[u8]) -> std::io::Result<()> {
        // The host may also split what it writes: feed the emulator in pieces too.
        for part in data.chunks(7) {
            let reply = lock(&self.emu).handle_stream_bytes(part);
            self.rx.lock().unwrap().extend(reply);
        }
        Ok(())
    }
    fn read_some(&mut self, buf: &mut [u8], _timeout: Duration) -> std::io::Result<usize> {
        let mut rx = self.rx.lock().unwrap();
        let n = rx.len().min(self.piece).min(buf.len());
        for slot in buf.iter_mut().take(n) {
            *slot = rx.pop_front().unwrap();
        }
        Ok(n)
    }
    fn name(&self) -> String {
        "pipe".into()
    }
}

fn pipe_machine(piece: usize) -> (RuidaMachine<StreamTransport<Pipe>>, Emu) {
    let emu = emulator(EmulatorConfig::default());
    let pipe = Pipe { emu: emu.clone(), rx: Default::default(), piece };
    (RuidaMachine::new(StreamTransport::new(pipe), MachineConfig::default()), emu)
}

#[test]
fn usb_stream_replies_arriving_in_pieces_are_put_together() {
    for piece in [1, 3, 64] {
        let (mut m, emu) = pipe_machine(piece);
        let info = m.connect().unwrap();
        assert_eq!(info.firmware, "RDC6442S-EMU", "a string reply split over many reads");
        assert_eq!(m.transport().framing(), Framing::Stream);
        assert_eq!(m.transport().describe(), "USB pipe");
        let out = m
            .send_job("USB", &square_job((5.0, 5.0), 10.0, 50.0), &SendOptions { start_immediately: true, ..Default::default() }, &mut never)
            .unwrap();
        assert_eq!(out.index, 1);
        assert_eq!(m.list_files().unwrap()[0].name, "USB");
        assert_eq!(lock(&emu).motion.iter().filter(|r| r.kind == MotionKind::Cut).count(), 4);
        assert_eq!(m.status().unwrap().state, MachineState::Idle);
        // Continuous jog uses the data channel, so it works over USB.
        m.jog_start(Axis::X, true).unwrap();
        m.jog_stop(Axis::X, true).unwrap();
        assert_eq!(lock(&emu).position()[0], 15_000);
    }
}

#[test]
fn usb_stream_has_no_jog_port() {
    let (mut m, _emu) = pipe_machine(64);
    m.connect().unwrap();
    assert!(matches!(m.panel_key(PanelKey::Pulse, true), Err(RuidaError::Unsupported(_))));
    assert!(matches!(m.frame_on_controller(), Err(RuidaError::Unsupported(_))));
    assert_eq!((transport::FTDI_VID, transport::FTDI_PID), (0x0403, 0x6001));
}

#[test]
fn hardware_overscan_mode_reaches_the_controller_and_ends_with_the_layer() {
    let (mut m, emu) = udp_machine();
    let mut job = mixed_job();
    job.layers[0].settings.overscan = OverscanMode::HorizontalBidirectional;
    job.layers[1].settings.overscan = OverscanMode::VerticalUnidirectional;
    m.run_job_now(&job).unwrap();
    let e = lock(&emu);
    assert_eq!(e.overscan_modes, vec![(0, OverscanMode::HorizontalBidirectional), (3, OverscanMode::VerticalUnidirectional)]);
    assert_eq!(e.overscan(), OverscanMode::Off, "CA 01 00 at the end of the layer turns it off");
}
