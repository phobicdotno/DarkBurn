//! Network test for the Gweike M3 Ultra: pings the card and the fiber source, times one status
//! read of the card, the poll DarkBurn already does, and reads the FTC follower on the card.
//!
//! Everything here only reads. The ping is a 1000-byte ICMP echo, like the vendor's test button.
//! The card probe sends one read of the status block (1000, 36 words) and one of the follower
//! status block (10000, 18 words), and checks each answer's CRC and sequence number; no register
//! is ever written. The FTC follower sits on the MCC100 card itself, so it has no address of its own. Nothing runs until the user presses
//! Start, and tests only use the card simulator and fake pingers.
//!
//! The ping uses the system `ping` program on every platform: `IcmpSendEcho` and raw ICMP sockets
//! need `unsafe`, which the workspace forbids. On Windows the console window is hidden.

use std::net::Ipv4Addr;
use std::time::{Duration, Instant};

use darkburn_mcc::registers::{ZfStatus, ZfWord};
use darkburn_mcc::{Reply, Request};
use serde_json::{Value, json};

use crate::commands::Cmd;

/// The MCP command: start, stop or read the test.
pub const COMMAND: Cmd = Cmd {
    id: "net.test",
    label: "Test the Gweike laser network",
    key: "",
    icon: "",
    args: r#"{"type":"object","properties":{"action":{"enum":["status","start","stop"],"description":"status (the default) reads the results without starting anything"}}}"#,
};

/// Run `net.test`: `{"action": "start" | "stop" | "status"}`.
pub fn command(n: &mut NetTest, a: &Value) -> Result<Value, String> {
    match a.get("action").and_then(Value::as_str).unwrap_or("status") {
        "start" => n.start(),
        "stop" => n.stop(),
        "status" => n.pump(),
        other => return Err(format!("unknown action {other}: use start, stop or status")),
    }
    Ok(n.to_json())
}

/// Samples kept per device for the sparkline and the numbers.
pub const HISTORY: usize = 60;
/// An average round trip above this is "slow" on a direct cable.
pub const SLOW_MS: f64 = 20.0;
/// Loss above this share (percent) is "lost" rather than "slow".
pub const LOST_PCT: f64 = 50.0;
/// The PC side of the laser network.
#[cfg(test)]
pub const PC_IP: Ipv4Addr = Ipv4Addr::new(10, 1, 1, 10);

/// What is being tested.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// MCC100 card, ICMP.
    Card,
    /// MCC100 card, one status read over UDP 502.
    CardProtocol,
    /// FTC follower on the card: one read of its status block (10000) over UDP 502.
    Ftc,
    /// Fiber source.
    Fiber,
}

impl Target {
    pub const ALL: [Target; 4] = [Target::Card, Target::CardProtocol, Target::Ftc, Target::Fiber];

    /// The address the device answers on. The FTC is read through the card.
    pub fn ip(self) -> Ipv4Addr {
        match self {
            Target::Card | Target::CardProtocol | Target::Ftc => Ipv4Addr::new(10, 1, 1, 168),
            Target::Fiber => Ipv4Addr::new(10, 1, 1, 170),
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Target::Card => "card",
            Target::CardProtocol => "card_protocol",
            Target::Ftc => "ftc",
            Target::Fiber => "fiber",
        }
    }
}

/// Signal height when the follower sees nothing (out of range, far from the sheet).
pub const FTC_FAR_UM: u32 = 25000;

/// What one read of the follower status block said.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FtcReading {
    /// Alarm bits (`word 1 & 0xFFFF`); any set is an error.
    pub alarm: u32,
    /// Running status (word 2).
    pub running: u32,
    /// Signal strength (word 4).
    pub signal: u32,
    /// Signal height in µm (word 5): 25000 = out of range, 0 = touching.
    pub height_um: u32,
    /// Z position in µm (word 6, signed).
    pub z_um: i32,
    /// Live signal (word 11).
    pub live_signal: u32,
}

impl FtcReading {
    pub fn of(z: &ZfStatus) -> FtcReading {
        FtcReading {
            alarm: z.alarm(),
            running: z.word(ZfWord::RunStatus),
            signal: z.word(ZfWord::SignalStrength),
            height_um: z.word(ZfWord::SignalHeight),
            z_um: z.z_um(),
            live_signal: z.word(ZfWord::LiveSignalStrength),
        }
    }

    /// The alarm bits in words, empty when there is none.
    pub fn alarm_text(&self) -> String {
        if self.alarm == 0 { String::new() } else { crate::gweike_ui::follower_alarm_text(self.alarm) }
    }

    /// The signal height in a few words: "far", "touching" or millimetres.
    pub fn height_text(&self) -> String {
        use crate::i18n::tr;
        match self.height_um {
            h if h >= FTC_FAR_UM => tr("net.ftc_far", "far"),
            0 => tr("net.ftc_touching", "touching"),
            h => format!("{:.2} mm", f64::from(h) / 1000.0),
        }
    }
}

/// Numbers over a run of samples (`None` = no answer).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Stats {
    pub sent: usize,
    pub received: usize,
    pub min_ms: Option<f64>,
    pub avg_ms: Option<f64>,
    pub max_ms: Option<f64>,
    /// Mean absolute difference between consecutive answered samples (RFC 3550 style, unsmoothed).
    pub jitter_ms: Option<f64>,
}

impl Stats {
    pub fn of(samples: &[Option<f64>]) -> Stats {
        let got: Vec<f64> = samples.iter().flatten().copied().collect();
        let mut s = Stats { sent: samples.len(), received: got.len(), ..Stats::default() };
        if got.is_empty() {
            return s;
        }
        s.min_ms = got.iter().copied().reduce(f64::min);
        s.max_ms = got.iter().copied().reduce(f64::max);
        s.avg_ms = Some(got.iter().sum::<f64>() / got.len() as f64);
        if got.len() >= 2 {
            let d: f64 = got.windows(2).map(|w| (w[1] - w[0]).abs()).sum();
            s.jitter_ms = Some(d / (got.len() - 1) as f64);
        }
        s
    }

    /// Lost share in percent; 0 when nothing was sent.
    pub fn loss_pct(&self) -> f64 {
        if self.sent == 0 { 0.0 } else { 100.0 * (self.sent - self.received) as f64 / self.sent as f64 }
    }
}

/// The pill shown for a device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Health {
    Waiting,
    Ok,
    Slow,
    Lost,
    /// The FTC's last status read got no (valid) answer.
    NoAnswer,
    /// The FTC reports alarm bits.
    Alarm,
}

impl Health {
    /// Health of a pinged or read device from its numbers.
    pub fn of(s: &Stats) -> Health {
        if s.sent == 0 {
            Health::Waiting
        } else if s.received == 0 || s.loss_pct() > LOST_PCT {
            Health::Lost
        } else if s.loss_pct() > 0.0 || s.avg_ms.is_some_and(|a| a > SLOW_MS) {
            Health::Slow
        } else {
            Health::Ok
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Health::Waiting => "waiting",
            Health::Ok => "ok",
            Health::Slow => "slow",
            Health::Lost => "lost",
            Health::NoAnswer => "no_answer",
            Health::Alarm => "alarm",
        }
    }

    pub fn text(self) -> String {
        use crate::i18n::tr;
        match self {
            Health::Waiting => tr("net.health_waiting", "Waiting"),
            Health::Ok => tr("net.health_ok", "OK"),
            Health::Slow => tr("net.health_slow", "Slow"),
            Health::Lost => tr("net.health_lost", "Lost"),
            Health::NoAnswer => tr("net.health_no_answer", "No answer"),
            Health::Alarm => tr("net.health_alarm", "Alarm"),
        }
    }
}

/// History of one device.
#[derive(Clone, Debug, Default)]
pub struct Track {
    pub samples: std::collections::VecDeque<Option<f64>>,
    /// Card protocol only: answers whose CRC or sequence number did not check out.
    pub bad_frames: usize,
}

impl Track {
    pub fn push(&mut self, sample: Option<f64>) {
        if self.samples.len() == HISTORY {
            self.samples.pop_front();
        }
        self.samples.push_back(sample);
    }

    pub fn stats(&self) -> Stats {
        let v: Vec<Option<f64>> = self.samples.iter().copied().collect();
        Stats::of(&v)
    }
}

/// The plain-language line at the top of the panel. `ftc_alarm` is the standing FTC alarm in
/// words, if any.
pub fn verdict(nic_ok: Option<bool>, card: &Stats, protocol: &Stats, ftc_alarm: Option<&str>) -> String {
    use crate::i18n::tr;
    if nic_ok == Some(false) {
        return tr("net.verdict_no_nic", "No network adapter is on 10.1.1.x: set the PC adapter to 10.1.1.10 / 255.255.255.0");
    }
    let best = if protocol.received > 0 { protocol } else { card };
    if card.sent == 0 && protocol.sent == 0 {
        return tr("net.verdict_idle", "Press Start to test the laser network");
    }
    if best.received == 0 {
        return tr("net.verdict_card_down", "Card not answering: check the cable and the PC IP address");
    }
    let loss = format!("{:.0}", best.loss_pct());
    let ms = format!("{:.1}", best.avg_ms.unwrap_or(0.0));
    if protocol.sent > 0 && protocol.received == 0 {
        return tr("net.verdict_ping_only", "Card answers ping but not the controller protocol: is Mlaser still connected?");
    }
    let ok = tr("net.verdict_card_ok", "Card OK, {loss}% loss, {ms} ms").replace("{loss}", &loss).replace("{ms}", &ms);
    match ftc_alarm {
        Some(a) if !a.is_empty() => tr("net.verdict_ftc_alarm", "{verdict} — FTC alarm: {alarm}").replace("{verdict}", &ok).replace("{alarm}", a),
        _ => ok,
    }
}

/// Whether any local adapter has an address in 10.1.1.0/24. `None` when it cannot be told.
#[cfg_attr(test, allow(dead_code))]
pub fn nic_on_laser_net() -> Option<bool> {
    // Asking the OS which source address it would use for the card needs no packet: a UDP
    // "connect" only picks a route.
    let s = std::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    if s.connect((Target::Card.ip(), 502)).is_err() {
        return Some(false);
    }
    match s.local_addr().ok()?.ip() {
        std::net::IpAddr::V4(ip) => Some(on_laser_net(ip)),
        std::net::IpAddr::V6(_) => Some(false),
    }
}

pub fn on_laser_net(ip: Ipv4Addr) -> bool {
    ip.octets()[..3] == [10, 1, 1]
}

/// One ping. Returns the round trip in milliseconds, or `None` for no answer.
pub trait Pinger: Send {
    fn ping(&mut self, ip: Ipv4Addr, timeout: Duration) -> Option<f64>;
}

/// One exchange of a datagram with the card.
pub trait CardWire: Send {
    fn exchange(&mut self, datagram: &[u8], timeout: Duration) -> Option<Vec<u8>>;
}

/// The result of one status read.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ProbeResult {
    Ok(f64),
    BadFrame,
    NoAnswer,
}

/// Send one status read (block 1000, 36 words) and check the answer.
pub fn probe_card(wire: &mut dyn CardWire, seq: u16, timeout: Duration) -> ProbeResult {
    let Ok(frame) = Request::read(1000, 36).map_err(|_| ()).and_then(|r| r.encode(seq).map_err(|_| ())) else {
        return ProbeResult::BadFrame;
    };
    let t = Instant::now();
    let Some(bytes) = wire.exchange(&frame, timeout) else {
        return ProbeResult::NoAnswer;
    };
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    match Reply::decode(&bytes, Some(seq)) {
        Ok(Reply::Read { addr: 1000, words }) if words.len() == 36 => ProbeResult::Ok(ms),
        _ => ProbeResult::BadFrame,
    }
}

/// The card over UDP 502. Opened only when the user presses Start.
#[cfg_attr(test, allow(dead_code))]
pub struct UdpCard {
    socket: std::net::UdpSocket,
}

#[cfg_attr(test, allow(dead_code))]
impl UdpCard {
    pub fn open() -> std::io::Result<UdpCard> {
        let socket = std::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
        socket.connect((Target::CardProtocol.ip(), 502))?;
        Ok(UdpCard { socket })
    }
}

impl CardWire for UdpCard {
    fn exchange(&mut self, datagram: &[u8], timeout: Duration) -> Option<Vec<u8>> {
        self.socket.set_read_timeout(Some(timeout)).ok()?;
        self.socket.send(datagram).ok()?;
        let mut buf = [0u8; 1500];
        let n = self.socket.recv(&mut buf).ok()?;
        Some(buf[..n].to_vec())
    }
}

/// Read the follower status block (10000, 18 words) once. Returns the round trip and, when the
/// answer checked out, what it said.
pub fn probe_ftc(wire: &mut dyn CardWire, seq: u16, timeout: Duration) -> (ProbeResult, Option<FtcReading>) {
    let Ok(frame) = Request::read(10000, 18).map_err(|_| ()).and_then(|r| r.encode(seq).map_err(|_| ())) else {
        return (ProbeResult::BadFrame, None);
    };
    let t = Instant::now();
    let Some(bytes) = wire.exchange(&frame, timeout) else {
        return (ProbeResult::NoAnswer, None);
    };
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    match Reply::decode(&bytes, Some(seq)) {
        Ok(Reply::Read { addr: 10000, words }) => match ZfStatus::from_words(&words) {
            Some(z) => (ProbeResult::Ok(ms), Some(FtcReading::of(&z))),
            None => (ProbeResult::BadFrame, None),
        },
        _ => (ProbeResult::BadFrame, None),
    }
}

/// The simulated card, for tests and the simulator device.
impl CardWire for darkburn_mcc::CardSimulator {
    fn exchange(&mut self, datagram: &[u8], _timeout: Duration) -> Option<Vec<u8>> {
        darkburn_mcc::CardSimulator::exchange(self, datagram).ok()
    }
}

/// The system `ping` program with a 1000-byte payload.
#[cfg_attr(test, allow(dead_code))]
pub struct SystemPing;

impl Pinger for SystemPing {
    fn ping(&mut self, ip: Ipv4Addr, timeout: Duration) -> Option<f64> {
        let ms = timeout.as_millis().max(1);
        let mut cmd = std::process::Command::new("ping");
        if cfg!(windows) {
            cmd.args(["-n", "1", "-l", "1000", "-w", &ms.to_string(), &ip.to_string()]);
        } else if cfg!(target_os = "macos") {
            cmd.args(["-c", "1", "-s", "1000", "-W", &ms.to_string(), &ip.to_string()]);
        } else {
            let secs = timeout.as_secs().max(1);
            cmd.args(["-n", "-c", "1", "-s", "1000", "-W", &secs.to_string(), &ip.to_string()]);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt as _;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let out = cmd.stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).output().ok()?;
        if !out.status.success() {
            return None;
        }
        parse_ping_ms(&String::from_utf8_lossy(&out.stdout))
    }
}

/// The round trip in a `ping` reply line: "time=0.42 ms", "time<1ms", "tid=3ms", "Zeit=1ms".
/// Works on localised output by looking for `=` or `<` followed by a number and `ms`.
pub fn parse_ping_ms(text: &str) -> Option<f64> {
    for line in text.lines() {
        let b = line.as_bytes();
        for (i, c) in b.iter().enumerate() {
            if *c != b'=' && *c != b'<' {
                continue;
            }
            let rest = &line[i + 1..];
            let num: String = rest.chars().take_while(|c| c.is_ascii_digit() || *c == '.' || *c == ',').collect();
            if num.is_empty() {
                continue;
            }
            let after = rest[num.len()..].trim_start();
            if after.starts_with("ms") {
                let v: f64 = num.replace(',', ".").parse().ok()?;
                // "time<1ms" means under one millisecond; show it as half of one.
                return Some(if *c == b'<' { v / 2.0 } else { v });
            }
        }
    }
    None
}

/// One sample from the background thread.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    pub target: Target,
    pub result: ProbeResult,
    /// FTC only: what the follower said.
    pub ftc: Option<FtcReading>,
}

/// A running test: a thread that probes every device once per round until stopped.
pub struct Runner {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    rx: std::sync::mpsc::Receiver<Sample>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Runner {
    /// Start probing. `wire` is `None` when the card protocol is not to be probed.
    pub fn start(mut pinger: Box<dyn Pinger>, mut wire: Option<Box<dyn CardWire>>, period: Duration) -> Runner {
        use std::sync::atomic::Ordering;
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let flag = stop.clone();
        let handle = std::thread::Builder::new()
            .name("net-test".into())
            .spawn(move || {
                let timeout = Duration::from_millis(1000);
                let mut seq: u16 = 0;
                while !flag.load(Ordering::Relaxed) {
                    let round = Instant::now();
                    for t in [Target::Card, Target::Fiber] {
                        let r = match pinger.ping(t.ip(), timeout) {
                            Some(ms) => ProbeResult::Ok(ms),
                            None => ProbeResult::NoAnswer,
                        };
                        if tx.send(Sample { target: t, result: r, ftc: None }).is_err() || flag.load(Ordering::Relaxed) {
                            return;
                        }
                    }
                    if let Some(w) = wire.as_mut() {
                        let r = probe_card(w.as_mut(), seq, timeout);
                        seq = seq.wrapping_add(1);
                        if tx.send(Sample { target: Target::CardProtocol, result: r, ftc: None }).is_err() {
                            return;
                        }
                        let (r, ftc) = probe_ftc(w.as_mut(), seq, timeout);
                        seq = seq.wrapping_add(1);
                        if tx.send(Sample { target: Target::Ftc, result: r, ftc }).is_err() {
                            return;
                        }
                    }
                    while round.elapsed() < period && !flag.load(Ordering::Relaxed) {
                        std::thread::sleep(Duration::from_millis(20));
                    }
                }
            })
            .ok();
        Runner { stop, rx, handle }
    }

    /// Samples that arrived since the last call.
    pub fn drain(&self) -> Vec<Sample> {
        self.rx.try_iter().collect()
    }

    pub fn stop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        // The thread ends after its current probe (at most one timeout); it is not joined so the
        // UI never waits on it.
        self.handle.take();
    }
}

impl Drop for Runner {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The panel's state.
#[derive(Default)]
pub struct NetTest {
    pub open: bool,
    pub runner: Option<Runner>,
    pub tracks: [Track; 4],
    pub nic_ok: Option<bool>,
    /// The FTC's last answer; `None` before the first one and after a read that failed.
    pub ftc: Option<FtcReading>,
}

impl NetTest {
    fn index(t: Target) -> usize {
        Target::ALL.iter().position(|x| *x == t).unwrap_or(0)
    }

    pub fn track(&self, t: Target) -> &Track {
        &self.tracks[Self::index(t)]
    }

    pub fn running(&self) -> bool {
        self.runner.is_some()
    }

    /// The pill for a device. The FTC is judged by its alarm bits and its last read.
    pub fn health(&self, t: Target) -> Health {
        let track = self.track(t);
        let h = Health::of(&track.stats());
        if t != Target::Ftc || h == Health::Waiting {
            return h;
        }
        match (track.samples.back().copied().flatten(), self.ftc) {
            (Some(_), Some(f)) if f.alarm != 0 => Health::Alarm,
            (Some(_), Some(_)) => Health::Ok,
            _ => Health::NoAnswer,
        }
    }

    /// The standing FTC alarm in words, if there is one.
    pub fn ftc_alarm_text(&self) -> Option<String> {
        self.ftc.filter(|f| f.alarm != 0).map(|f| f.alarm_text())
    }

    pub fn record(&mut self, s: Sample) {
        if s.target == Target::Ftc {
            self.ftc = match s.result {
                ProbeResult::Ok(_) => s.ftc,
                _ => None,
            };
        }
        let tr = &mut self.tracks[Self::index(s.target)];
        match s.result {
            ProbeResult::Ok(ms) => tr.push(Some(ms)),
            ProbeResult::NoAnswer => tr.push(None),
            ProbeResult::BadFrame => {
                tr.bad_frames += 1;
                tr.push(None);
            }
        }
    }

    /// Take what the thread measured.
    pub fn pump(&mut self) {
        let samples = self.runner.as_ref().map(|r| r.drain()).unwrap_or_default();
        for s in samples {
            self.record(s);
        }
    }

    pub fn start_with(&mut self, pinger: Box<dyn Pinger>, wire: Option<Box<dyn CardWire>>, nic_ok: Option<bool>) {
        self.stop();
        self.tracks = Default::default();
        self.ftc = None;
        self.nic_ok = nic_ok;
        self.runner = Some(Runner::start(pinger, wire, Duration::from_millis(1000)));
    }

    /// Start on the real laser network. Under `cargo test` a fake pinger and the card
    /// simulator stand in, so no test touches the network.
    pub fn start(&mut self) {
        #[cfg(not(test))]
        {
            let wire = UdpCard::open().ok().map(|w| Box::new(w) as Box<dyn CardWire>);
            self.start_with(Box::new(SystemPing), wire, nic_on_laser_net());
        }
        #[cfg(test)]
        {
            let sim = darkburn_mcc::CardSimulator::new(darkburn_mcc::SimConfig::default());
            self.start_with(Box::new(SimPing), Some(Box::new(sim)), Some(true));
        }
    }

    pub fn stop(&mut self) {
        if let Some(mut r) = self.runner.take() {
            r.stop();
        }
    }

    pub fn verdict(&self) -> String {
        let alarm = self.ftc_alarm_text();
        verdict(self.nic_ok, &self.track(Target::Card).stats(), &self.track(Target::CardProtocol).stats(), alarm.as_deref())
    }

    /// The results as JSON, for `net.test`.
    pub fn to_json(&self) -> Value {
        let devices: Vec<Value> = Target::ALL
            .iter()
            .map(|t| {
                let tr = self.track(*t);
                let s = tr.stats();
                let ip = if *t == Target::Ftc { "on card".to_owned() } else { t.ip().to_string() };
                json!({
                    "device": t.id(),
                    "ip": ip,
                    "health": self.health(*t).id(),
                    "sent": s.sent,
                    "received": s.received,
                    "loss_pct": s.loss_pct(),
                    "min_ms": s.min_ms,
                    "avg_ms": s.avg_ms,
                    "max_ms": s.max_ms,
                    "jitter_ms": s.jitter_ms,
                    "bad_frames": tr.bad_frames,
                })
            })
            .collect();
        let s = self.track(Target::Ftc).stats();
        let ftc = json!({
            "health": self.health(Target::Ftc).id(),
            "alarm": self.ftc.map(|f| f.alarm),
            "alarm_text": self.ftc.map(|f| f.alarm_text()),
            "running": self.ftc.map(|f| f.running),
            "signal": self.ftc.map(|f| f.signal),
            "live_signal": self.ftc.map(|f| f.live_signal),
            "height_um": self.ftc.map(|f| f.height_um),
            "z_um": self.ftc.map(|f| f.z_um),
            "sent": s.sent,
            "received": s.received,
            "min_ms": s.min_ms,
            "avg_ms": s.avg_ms,
            "max_ms": s.max_ms,
            "jitter_ms": s.jitter_ms,
            "bad_frames": self.track(Target::Ftc).bad_frames,
        });
        json!({ "running": self.running(), "nic_on_laser_net": self.nic_ok, "verdict": self.verdict(), "devices": devices, "ftc": ftc })
    }
}

/// Answers like a healthy direct cable (tests only).
#[cfg(test)]
struct SimPing;

#[cfg(test)]
impl Pinger for SimPing {
    fn ping(&mut self, _ip: Ipv4Addr, _timeout: Duration) -> Option<f64> {
        Some(0.4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_of_a_clean_run() {
        let s = Stats::of(&[Some(1.0), Some(3.0), Some(2.0)]);
        assert_eq!((s.sent, s.received), (3, 3));
        assert_eq!(s.min_ms, Some(1.0));
        assert_eq!(s.max_ms, Some(3.0));
        assert_eq!(s.avg_ms, Some(2.0));
        assert_eq!(s.jitter_ms, Some(1.5));
        assert_eq!(s.loss_pct(), 0.0);
    }

    #[test]
    fn stats_count_loss_and_skip_lost_samples_in_jitter() {
        let s = Stats::of(&[Some(1.0), None, Some(2.0), None]);
        assert_eq!(s.loss_pct(), 50.0);
        assert_eq!(s.jitter_ms, Some(1.0));
        let none = Stats::of(&[None, None]);
        assert_eq!(none.loss_pct(), 100.0);
        assert_eq!(none.avg_ms, None);
        assert_eq!(Stats::of(&[]).loss_pct(), 0.0);
        assert_eq!(Stats::of(&[Some(4.0)]).jitter_ms, None);
    }

    #[test]
    fn history_keeps_the_last_sixty() {
        let mut t = Track::default();
        for i in 0..100 {
            t.push(Some(i as f64));
        }
        assert_eq!(t.samples.len(), HISTORY);
        assert_eq!(t.stats().min_ms, Some(40.0));
    }

    #[test]
    fn the_ftc_pill_follows_its_alarm_bits_and_last_read() {
        use darkburn_mcc::registers::zf_alarm;
        let mut n = NetTest::default();
        assert_eq!(n.health(Target::Ftc), Health::Waiting);
        let clean = FtcReading { height_um: FTC_FAR_UM, ..FtcReading::default() };
        n.record(Sample { target: Target::Ftc, result: ProbeResult::Ok(0.3), ftc: Some(clean) });
        assert_eq!(n.health(Target::Ftc), Health::Ok);
        assert_eq!(clean.height_text(), "far");
        let alarm = FtcReading { alarm: zf_alarm::SIGNAL_SMALL, ..clean };
        n.record(Sample { target: Target::Ftc, result: ProbeResult::Ok(0.3), ftc: Some(alarm) });
        assert_eq!(n.health(Target::Ftc), Health::Alarm);
        assert_eq!(n.ftc_alarm_text().as_deref(), Some("signal too small"));
        n.record(Sample { target: Target::Ftc, result: ProbeResult::NoAnswer, ftc: None });
        assert_eq!(n.health(Target::Ftc), Health::NoAnswer);
        assert_eq!(n.ftc_alarm_text(), None);
    }

    #[test]
    fn health_pills() {
        assert_eq!(Health::of(&Stats::default()), Health::Waiting);
        assert_eq!(Health::of(&Stats::of(&[Some(0.4); 10])), Health::Ok);
        assert_eq!(Health::of(&Stats::of(&[Some(45.0); 10])), Health::Slow);
        assert_eq!(Health::of(&Stats::of(&[Some(1.0), None, None])), Health::Lost);
        assert_eq!(Health::of(&Stats::of(&[None; 5])), Health::Lost);
    }

    #[test]
    fn verdict_texts() {
        let ok = Stats::of(&[Some(0.4); 10]);
        let down = Stats::of(&[None; 10]);
        assert_eq!(verdict(Some(true), &ok, &ok, None), "Card OK, 0% loss, 0.4 ms");
        assert_eq!(verdict(Some(true), &down, &down, None), "Card not answering: check the cable and the PC IP address");
        assert!(verdict(Some(false), &ok, &ok, None).contains("10.1.1.10 / 255.255.255.0"));
        assert_eq!(verdict(None, &Stats::default(), &Stats::default(), None), "Press Start to test the laser network");
        assert!(verdict(Some(true), &ok, &down, None).contains("not the controller protocol"));
        // Without a protocol probe the ping decides.
        assert_eq!(verdict(Some(true), &ok, &Stats::default(), None), "Card OK, 0% loss, 0.4 ms");
        assert_eq!(verdict(Some(true), &ok, &ok, Some("signal zero")), "Card OK, 0% loss, 0.4 ms — FTC alarm: signal zero");
    }

    #[test]
    fn laser_net_check() {
        assert!(on_laser_net(PC_IP));
        assert!(!on_laser_net(Ipv4Addr::new(192, 168, 1, 10)));
        assert!(!on_laser_net(Ipv4Addr::new(10, 1, 2, 10)));
    }

    #[test]
    fn ping_output_is_read_in_any_language() {
        assert_eq!(parse_ping_ms("64 bytes from 10.1.1.168: icmp_seq=1 ttl=64 time=0.412 ms"), Some(0.412));
        assert_eq!(parse_ping_ms("Reply from 10.1.1.168: bytes=1000 time=3ms TTL=64"), Some(3.0));
        assert_eq!(parse_ping_ms("Svar fra 10.1.1.168: byte=1000 tid<1ms TTL=64"), Some(0.5));
        assert_eq!(parse_ping_ms("Antwort von 10.1.1.168: Bytes=1000 Zeit=12ms TTL=64"), Some(12.0));
        assert_eq!(parse_ping_ms("Request timed out."), None);
    }

    #[test]
    fn the_card_probe_reads_status_from_the_simulator() {
        let mut sim = darkburn_mcc::CardSimulator::new(darkburn_mcc::SimConfig::default());
        let r = probe_card(&mut sim, 7, Duration::from_millis(10));
        assert!(matches!(r, ProbeResult::Ok(_)), "{r:?}");
        let (r, ftc) = probe_ftc(&mut sim, 8, Duration::from_millis(10));
        assert!(matches!(r, ProbeResult::Ok(_)), "{r:?}");
        assert_eq!(ftc.map(|f| f.alarm), Some(0));
        // Read only: the card saw no command.
        assert!(sim.commands().is_empty());
        assert!(sim.forbidden_attempts().is_empty());
    }

    #[test]
    fn a_silent_or_garbled_card_is_counted() {
        let mut sim = darkburn_mcc::CardSimulator::new(darkburn_mcc::SimConfig::default());
        sim.drop_replies(1);
        assert_eq!(probe_card(&mut sim, 1, Duration::from_millis(10)), ProbeResult::NoAnswer);
        struct Garble;
        impl CardWire for Garble {
            fn exchange(&mut self, d: &[u8], _: Duration) -> Option<Vec<u8>> {
                let mut v = d.to_vec();
                if let Some(b) = v.last_mut() {
                    *b ^= 0xff;
                }
                Some(v)
            }
        }
        assert_eq!(probe_card(&mut Garble, 1, Duration::from_millis(10)), ProbeResult::BadFrame);
    }

    struct FakePing;
    impl Pinger for FakePing {
        fn ping(&mut self, ip: Ipv4Addr, _: Duration) -> Option<f64> {
            // Nothing lives at .169: the FTC is on the card.
            assert_ne!(ip, Ipv4Addr::new(10, 1, 1, 169));
            Some(0.4)
        }
    }

    /// Run one round against `sim` and return the results.
    fn one_round(sim: darkburn_mcc::CardSimulator) -> (NetTest, Value) {
        let mut n = NetTest::default();
        n.start_with(Box::new(FakePing), Some(Box::new(sim)), Some(true));
        let t = Instant::now();
        while n.track(Target::Ftc).samples.is_empty() && t.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(5));
            n.pump();
        }
        n.stop();
        assert!(!n.running());
        let j = n.to_json();
        (n, j)
    }

    fn healths(j: &Value) -> Vec<String> {
        j["devices"].as_array().unwrap().iter().map(|d| d["health"].as_str().unwrap().to_owned()).collect()
    }

    #[test]
    fn the_runner_fills_every_row_and_stops() {
        let sim = darkburn_mcc::CardSimulator::new(darkburn_mcc::SimConfig::default());
        let (n, j) = one_round(sim);
        assert_eq!(healths(&j), ["ok", "ok", "ok", "ok"]);
        assert_eq!(j["devices"][2]["device"], "ftc");
        assert_eq!(j["devices"][2]["ip"], "on card");
        // The protocol probe wins over ping; the simulator answers at once.
        assert!(n.verdict().starts_with("Card OK, 0% loss"), "{}", n.verdict());
    }

    #[test]
    fn a_clean_ftc_reads_ok_without_an_alarm() {
        let mut sim = darkburn_mcc::CardSimulator::new(darkburn_mcc::SimConfig::default());
        assert!(sim.set_card_word(10005, FTC_FAR_UM));
        assert!(sim.set_card_word(10004, 1234));
        let (n, j) = one_round(sim);
        assert_eq!(j["ftc"]["health"], "ok");
        assert_eq!(j["ftc"]["alarm"], 0);
        assert_eq!(j["ftc"]["alarm_text"], "");
        assert_eq!(j["ftc"]["signal"], 1234);
        assert_eq!(j["ftc"]["height_um"], FTC_FAR_UM);
        assert_eq!(j["ftc"]["received"], 1);
        assert!(!n.verdict().contains("FTC alarm"), "{}", n.verdict());
    }

    #[test]
    fn an_ftc_alarm_turns_the_row_red_and_reaches_the_verdict() {
        use darkburn_mcc::registers::zf_alarm;
        let mut sim = darkburn_mcc::CardSimulator::new(darkburn_mcc::SimConfig::default());
        assert!(sim.set_card_word(10001, zf_alarm::SIGNAL_ZERO | zf_alarm::FTC_ALARM));
        let (n, j) = one_round(sim);
        assert_eq!(healths(&j), ["ok", "ok", "alarm", "ok"]);
        assert_eq!(j["ftc"]["alarm"], zf_alarm::SIGNAL_ZERO | zf_alarm::FTC_ALARM);
        assert_eq!(j["ftc"]["alarm_text"], "signal zero, follower alarm");
        assert!(n.verdict().ends_with("— FTC alarm: signal zero, follower alarm"), "{}", n.verdict());
    }

    #[test]
    fn the_command_reads_starts_and_stops() {
        let mut n = NetTest::default();
        let v = command(&mut n, &json!({})).unwrap();
        assert_eq!(v["running"], false);
        assert_eq!(command(&mut n, &json!({ "action": "start" })).unwrap()["running"], true);
        assert_eq!(command(&mut n, &json!({ "action": "stop" })).unwrap()["running"], false);
        assert!(command(&mut n, &json!({ "action": "jump" })).is_err());
    }
}
