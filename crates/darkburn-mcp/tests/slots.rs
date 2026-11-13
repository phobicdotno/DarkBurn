//! The control channel's connection places: a program on this computer without the token must
//! not be able to keep the real agent out by holding connections open.

use std::io::Write;
use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use darkburn_mcp::control::{ControlClient, ControlConfig, ControlServer, read_frame, write_frame};
use darkburn_mcp::{Gate, McpPolicy, MockBackend, PolicyHandle};
use serde_json::{Value, json};

const TOKEN: &str = "slots-token-0123456789abcdef";

/// The app: a thread that pumps the server, as the UI thread does once per frame.
struct App {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl App {
    fn start(cfg: ControlConfig) -> App {
        let stop = Arc::new(AtomicBool::new(false));
        let (atx, arx) = mpsc::channel();
        let s2 = stop.clone();
        let join = std::thread::spawn(move || {
            let server = ControlServer::bind(cfg).expect("bind");
            atx.send(server.addr()).unwrap();
            let mut gate = Gate::new(MockBackend::new(), PolicyHandle::new(McpPolicy::default()));
            while !s2.load(Ordering::SeqCst) {
                server.pump(&mut gate, 64);
                std::thread::sleep(Duration::from_millis(1));
            }
        });
        let addr = arx.recv_timeout(Duration::from_secs(5)).unwrap();
        App { addr, stop, join: Some(join) }
    }
}

impl Drop for App {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

fn cfg() -> ControlConfig {
    ControlConfig { port: 0, token: Some(TOKEN.into()), ..Default::default() }
}

/// The real agent logs in and runs a command.
fn agent_gets_in(addr: SocketAddr) -> Result<(), String> {
    let mut c = ControlClient::connect(addr, TOKEN).map_err(|e| e.to_string())?;
    c.call("running", Value::Null).map(|_| ()).map_err(|e| e.to_string())
}

/// Opens a connection that sends the start of a hello one byte at a time, a byte every `gap`,
/// until `stop` is set or the server hangs up. Returns when the server hung up (or `stop`).
fn trickle(addr: SocketAddr, gap: Duration, stop: Arc<AtomicBool>) -> std::thread::JoinHandle<Duration> {
    std::thread::spawn(move || {
        let start = Instant::now();
        let Ok(mut s) = TcpStream::connect(addr) else { return start.elapsed() };
        // A length that promises a whole hello, then the hello's bytes one by one.
        let mut bytes = 4000u32.to_be_bytes().to_vec();
        bytes.extend(std::iter::repeat_n(b' ', 4000));
        for b in bytes {
            if stop.load(Ordering::SeqCst) || s.write_all(&[b]).is_err() {
                break;
            }
            std::thread::sleep(gap);
        }
        start.elapsed()
    })
}

#[test]
fn a_hello_sent_a_byte_at_a_time_runs_out_of_time_as_a_whole() {
    let app = App::start(ControlConfig { hello_timeout: Duration::from_millis(1500), ..cfg() });
    // One byte every 300 ms: each byte is well inside any per-read time limit.
    let mut s = TcpStream::connect(app.addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    let mut writer = s.try_clone().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let st = stop.clone();
    let feeder = std::thread::spawn(move || {
        let _ = writer.write_all(&4000u32.to_be_bytes());
        while !st.load(Ordering::SeqCst) && writer.write_all(b" ").is_ok() {
            std::thread::sleep(Duration::from_millis(300));
        }
    });
    let start = Instant::now();
    let reply = read_frame(&mut s, 1 << 20);
    let took = start.elapsed();
    stop.store(true, Ordering::SeqCst);
    feeder.join().unwrap();
    assert!(took < Duration::from_secs(4), "the slow hello kept its connection for {took:?}");
    if let Ok(Some(v)) = reply {
        assert_eq!(v["ok"], false, "{v}");
    }
}

#[test]
fn slow_hellos_on_every_place_do_not_keep_the_agent_out() {
    let app = App::start(cfg());
    let stop = Arc::new(AtomicBool::new(false));
    // More slow hellos than there are places of any kind, a byte every half second each.
    let hogs: Vec<_> = (0..12).map(|_| trickle(app.addr, Duration::from_millis(500), stop.clone())).collect();
    std::thread::sleep(Duration::from_millis(500));
    let result = agent_gets_in(app.addr);
    // And again while they are still at it.
    std::thread::sleep(Duration::from_millis(500));
    let again = agent_gets_in(app.addr);
    stop.store(true, Ordering::SeqCst);
    for h in hogs {
        h.join().unwrap();
    }
    result.expect("the agent was kept out by slow hellos");
    again.expect("the agent was kept out the second time");
}

#[test]
fn a_stream_of_new_silent_connections_does_not_keep_the_agent_out() {
    let app = App::start(cfg());
    let stop = Arc::new(AtomicBool::new(false));
    let st = stop.clone();
    let addr = app.addr;
    // A new connection every 100 ms that never says anything, each kept open.
    let hog = std::thread::spawn(move || {
        let mut held = Vec::new();
        while !st.load(Ordering::SeqCst) {
            if let Ok(s) = TcpStream::connect(addr) {
                held.push(s);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        held.len()
    });
    std::thread::sleep(Duration::from_millis(1500));
    let result = agent_gets_in(addr);
    std::thread::sleep(Duration::from_millis(700));
    let again = agent_gets_in(addr);
    stop.store(true, Ordering::SeqCst);
    let opened = hog.join().unwrap();
    assert!(opened > 10, "the hog opened only {opened} connections");
    result.expect("the agent was kept out by silent connections");
    again.expect("the agent was kept out the second time");
}

#[test]
fn logged_in_agents_are_never_dropped_for_newcomers() {
    let app = App::start(ControlConfig { max_connections: 2, max_unauthenticated: 1, ..cfg() });
    let mut a = ControlClient::connect(app.addr, TOKEN).unwrap();
    let mut b = ControlClient::connect(app.addr, TOKEN).unwrap();
    // A crowd of silent connections comes and goes.
    let crowd: Vec<_> = (0..10).filter_map(|_| TcpStream::connect(app.addr).ok()).collect();
    std::thread::sleep(Duration::from_millis(200));
    a.call("running", Value::Null).expect("the first agent was dropped");
    b.call("running", Value::Null).expect("the second agent was dropped");
    // A third agent with the token finds both places taken and is told so.
    let e = ControlClient::connect(app.addr, TOKEN).err().expect("a third agent got in");
    assert!(e.message.contains("already serves 2 agents"), "{e}");
    drop(crowd);
    // When one leaves, its place is free again.
    drop(b);
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        match ControlClient::connect(app.addr, TOKEN) {
            Ok(mut c) => {
                c.call("running", Value::Null).unwrap();
                break;
            }
            Err(e) => assert!(Instant::now() < end, "the freed place was not given back: {e}"),
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn a_dropped_handshake_gets_nothing_and_its_place_is_free() {
    let app = App::start(ControlConfig { max_unauthenticated: 1, ..cfg() });
    // The first connection sits in the handshake past the moment a hello takes; a second one
    // pushes it out.
    let mut first = TcpStream::connect(app.addr).unwrap();
    first.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    std::thread::sleep(Duration::from_millis(600));
    let _second = TcpStream::connect(app.addr).unwrap();
    std::thread::sleep(Duration::from_millis(200));
    // The first one is closed: a hello sent now gets no login.
    let _ = write_frame(&mut first, &json!({"hello": {"token": TOKEN, "version": 1}}));
    match read_frame(&mut first, 1 << 20) {
        Ok(Some(v)) => assert_ne!(v["ok"], true, "a dropped connection logged in: {v}"),
        Ok(None) | Err(_) => {}
    }
    agent_gets_in(app.addr).expect("the agent got in");
}

#[test]
fn agents_that_connect_at_the_same_moment_all_get_in() {
    let app = App::start(ControlConfig { max_unauthenticated: 1, ..cfg() });
    let addr = app.addr;
    let go = Arc::new(std::sync::Barrier::new(6));
    let all: Vec<_> = (0..6)
        .map(|_| {
            let go = go.clone();
            std::thread::spawn(move || {
                go.wait();
                agent_gets_in(addr)
            })
        })
        .collect();
    for h in all {
        h.join().unwrap().expect("an agent that came at the same moment as others was dropped");
    }
}
