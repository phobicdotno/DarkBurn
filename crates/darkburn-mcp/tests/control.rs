//! The control channel: framing, authentication, loopback rules, concurrency, full stack.

mod common;

use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use common::*;
use darkburn_mcp::control::{Connect, ControlClient, ControlConfig, ControlServer, RemoteBackend, is_allowed_peer, read_frame, write_frame};
use darkburn_mcp::util::{base64_decode, ct_eq};
use darkburn_mcp::{Backend, ErrorCode, Gate, ListQuery, McpPolicy, MockBackend, PolicyHandle};
use serde_json::{Value, json};

const TOKEN: &str = "test-token-0123456789abcdef";

/// Stands in for the app: a thread that owns the gated backend and pumps the server, as the UI
/// thread does once per frame.
struct App {
    port: u16,
    stop: Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<(usize, Vec<String>)>>,
    policy: PolicyHandle,
}

impl App {
    fn start(policy: McpPolicy) -> App {
        Self::start_with(policy, ControlConfig { port: 0, token: Some(TOKEN.into()), ..Default::default() })
    }

    fn start_with(policy: McpPolicy, cfg: ControlConfig) -> App {
        let handle = PolicyHandle::new(policy);
        let stop = Arc::new(AtomicBool::new(false));
        let (ptx, prx) = mpsc::channel();
        let (h2, s2) = (handle.clone(), stop.clone());
        let join = std::thread::spawn(move || {
            let server = ControlServer::bind(cfg).expect("bind");
            ptx.send(server.port()).unwrap();
            let mut gate = Gate::new(MockBackend::new(), h2);
            while !s2.load(Ordering::SeqCst) {
                server.pump(&mut gate, 64);
                std::thread::sleep(Duration::from_millis(1));
            }
            (server.refused_connections(), gate.inner().executed.clone())
        });
        let port = prx.recv_timeout(Duration::from_secs(5)).unwrap();
        App { port, stop, join: Some(join), policy: handle }
    }

    fn remote(&self) -> RemoteBackend {
        RemoteBackend::new(Connect::Direct { port: self.port, token: TOKEN.into() })
    }

    fn addr(&self) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), self.port)
    }

    fn finish(mut self) -> (usize, Vec<String>) {
        self.stop.store(true, Ordering::SeqCst);
        self.join.take().unwrap().join().unwrap()
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

#[test]
fn only_loopback_peers_are_allowed() {
    for ok in [
        IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V4(Ipv4Addr::new(127, 5, 5, 5)),
        IpAddr::V6(Ipv6Addr::LOCALHOST),
        "::ffff:127.0.0.1".parse().unwrap(),
    ] {
        assert!(is_allowed_peer(ok), "{ok}");
    }
    for bad in ["192.168.1.20", "10.1.1.5", "172.16.0.1", "8.8.8.8", "0.0.0.0", "169.254.1.1", "::ffff:192.168.1.1", "fe80::1", "2001:db8::1", "::"] {
        assert!(!is_allowed_peer(bad.parse().unwrap()), "{bad}");
    }
}

#[test]
fn the_server_refuses_to_bind_a_non_loopback_address() {
    for host in ["0.0.0.0", "192.168.1.5", "::", "10.1.1.10"] {
        let cfg = ControlConfig { host: host.parse().unwrap(), port: 0, ..Default::default() };
        let e = ControlServer::bind(cfg).err().unwrap_or_else(|| panic!("{host} must be refused"));
        assert_eq!(e.kind(), std::io::ErrorKind::PermissionDenied, "{host}");
    }
}

#[test]
fn the_client_refuses_a_non_loopback_address_without_connecting() {
    let e = ControlClient::connect("192.0.2.7:27879".parse().unwrap(), TOKEN).err().unwrap();
    assert_eq!(e.code, ErrorCode::Transport);
    assert!(e.message.contains("loopback"), "{e}");
}

#[test]
fn constant_time_compare_is_correct() {
    assert!(ct_eq(b"abc", b"abc"));
    assert!(ct_eq(b"", b""));
    assert!(!ct_eq(b"abc", b"abd"));
    assert!(!ct_eq(b"abc", b"abcd"));
    assert!(!ct_eq(b"abcd", b"abc"));
    assert!(!ct_eq(b"", b"a"));
}

#[test]
fn frames_round_trip_and_enforce_limits() {
    let mut buf = Vec::new();
    write_frame(&mut buf, &json!({"a": [1, 2, 3], "s": "é"})).unwrap();
    assert_eq!(&buf[..4], &(buf.len() as u32 - 4).to_be_bytes());
    let v = read_frame(&mut &buf[..], 1 << 20).unwrap().unwrap();
    assert_eq!(v["a"], json!([1, 2, 3]));
    // Clean end of stream.
    assert!(read_frame(&mut &b""[..], 100).unwrap().is_none());
    // Too long: refused from the header alone.
    let e = read_frame(&mut &buf[..], 5).unwrap_err();
    assert_eq!(e.kind(), std::io::ErrorKind::InvalidData);
    let huge = u32::MAX.to_be_bytes();
    assert!(read_frame(&mut &huge[..], 1 << 20).is_err());
    // Truncated body.
    assert!(read_frame(&mut &buf[..buf.len() - 2], 1 << 20).is_err());
    // Bad JSON.
    let mut bad = 3u32.to_be_bytes().to_vec();
    bad.extend_from_slice(b"{{{");
    assert_eq!(read_frame(&mut &bad[..], 100).unwrap_err().kind(), std::io::ErrorKind::InvalidData);
}

#[test]
fn base64_decode_round_trips() {
    for data in [&b""[..], b"f", b"fo", b"foo", b"foob", b"\x00\xff\x10binary\xfe"] {
        let enc = solidcraft_mcp_server::base64(data);
        assert_eq!(base64_decode(&enc).unwrap(), data);
    }
    assert!(base64_decode("not base64!").is_none());
}

#[test]
fn authenticated_client_runs_commands_over_tcp() {
    let app = App::start(McpPolicy::default());
    let mut r = app.remote();
    assert!(r.describe().contains("127.0.0.1"));
    let page = r.list_commands(&ListQuery { filter: Some("shape".into()), ..Default::default() }).unwrap();
    assert_eq!(page.total, 3);
    let info = r.describe_command("shape.add_rect").unwrap().unwrap();
    assert_eq!(info.id, "shape.add_rect");
    assert!(r.describe_command("zzz.none").unwrap().is_none());
    let out = r.execute("shape.add_rect", json!({"x": 0, "y": 0, "width": 2, "height": 2})).unwrap();
    assert_eq!(out["id"], "s1");
    assert_eq!(r.inspect("design.shapes.s1", 0).unwrap()["width"], 2.0);
    let e = r.execute("shape.move", json!({"id": "s9", "dx": 1, "dy": 1})).unwrap_err();
    assert_eq!(e.code, ErrorCode::Failed, "typed errors survive the wire");
    assert!(e.message.contains("s9"));
    let png = r.screenshot(None).unwrap();
    assert_eq!(&png[..4], b"\x89PNG");
    assert!(r.events(0).unwrap().events.iter().any(|e| e.command == "shape.add_rect"));
    let b = r.batch(&[darkburn_mcp::BatchStep { command: "file.new".into(), params: Value::Null, confirmation_token: None }], true).unwrap();
    assert!(b.ok);
}

#[test]
fn the_gate_applies_over_the_wire() {
    let app = App::start(McpPolicy::default());
    let mut r = app.remote();
    let e = r.execute("machine.jog", json!({"dx": 1, "dy": 1})).unwrap_err();
    assert_eq!(e.code, ErrorCode::Denied);
    r.execute("machine.emergency_stop", json!({})).unwrap();
    // The human flips the setting in the app: the next call sees it.
    app.policy.update(|p| p.allow_hardware = true);
    r.execute("machine.jog", json!({"dx": 1, "dy": 1})).unwrap();
    let e = r.execute("job.start", json!({})).unwrap_err();
    assert_eq!(e.code, ErrorCode::ConfirmationRequired);
    let (_, executed) = app.finish();
    assert_eq!(executed, ["machine.emergency_stop", "machine.jog"]);
}

#[test]
fn a_wrong_token_is_refused_and_counted() {
    let app = App::start(McpPolicy::default());
    let e = ControlClient::connect(app.addr(), "wrong").err().unwrap();
    assert_eq!(e.code, ErrorCode::Transport);
    assert!(e.message.contains("authentication failed"), "{e}");
    let e = ControlClient::connect(app.addr(), "").err().unwrap();
    assert!(e.message.contains("authentication failed"), "{e}");
    // A token that is a prefix of the real one.
    assert!(ControlClient::connect(app.addr(), &TOKEN[..10]).is_err());
    // The right one still works.
    assert!(ControlClient::connect(app.addr(), TOKEN).is_ok());
    let (refused, _) = app.finish();
    assert_eq!(refused, 3);
}

#[test]
fn garbage_and_unauthenticated_frames_get_nowhere() {
    let app = App::start(McpPolicy::default());
    // A request without hello.
    let mut s = TcpStream::connect(app.addr()).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write_frame(&mut s, &json!({"id": 1, "method": "execute", "params": {"id": "file.new"}})).unwrap();
    let reply = read_frame(&mut s, 1 << 20).unwrap().unwrap();
    assert_eq!(reply["ok"], false);
    assert!(read_frame(&mut s, 1 << 20).map(|v| v.is_none()).unwrap_or(true), "connection closed");
    // Raw bytes that are not a frame.
    let mut s = TcpStream::connect(app.addr()).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    s.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
    let mut sink = Vec::new();
    let _ = s.read_to_end(&mut sink);
    // A hello that claims a gigabyte: refused from the header, no allocation, server unharmed.
    let mut s = TcpStream::connect(app.addr()).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    s.write_all(&(1u32 << 30).to_be_bytes()).unwrap();
    let mut sink = Vec::new();
    let _ = s.read_to_end(&mut sink);
    // Nothing from the failed attempts ran, and the server still serves a good client.
    let mut r = app.remote();
    r.execute("file.new", json!({})).unwrap();
    let (refused, executed) = app.finish();
    assert!(refused >= 3);
    assert_eq!(executed, ["file.new"]);
}

#[test]
fn an_oversized_request_after_login_closes_only_that_connection() {
    let cfg = ControlConfig { port: 0, token: Some(TOKEN.into()), max_frame: 4096, ..Default::default() };
    let app = App::start_with(McpPolicy::default(), cfg);
    let mut c = ControlClient::connect(app.addr(), TOKEN).unwrap();
    let e = c.call("measure", json!({"pad": "x".repeat(10_000)})).unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidParams, "the server says why before it hangs up");
    assert!(e.message.contains("exceeds"), "{e}");
    assert_eq!(c.call("running", Value::Null).unwrap_err().code, ErrorCode::Transport, "then the connection is closed");
    let mut other = app.remote();
    assert!(other.inspect("undo", 0).is_ok());
}

/// Waits until the server has let go of every connection.
fn wait_all_closed(server: &ControlServer) {
    let end = std::time::Instant::now() + Duration::from_secs(10);
    while server.connections() > 0 {
        assert!(std::time::Instant::now() < end, "the server kept the connection open");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn a_refusal_still_reaches_a_client_that_reads_it_after_the_server_hung_up() {
    // In each case the client has sent bytes the server never reads as a request. The reply
    // must survive the server closing its end, however late the client gets round to reading.
    let server = ControlServer::bind(ControlConfig { port: 0, token: Some(TOKEN.into()), max_frame: 4096, ..Default::default() }).unwrap();
    let hello = json!({"hello": {"token": TOKEN, "client": "test", "version": 1}});

    // An oversized request after login.
    let mut s = TcpStream::connect(server.addr()).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write_frame(&mut s, &hello).unwrap();
    assert_eq!(read_frame(&mut s, 1 << 20).unwrap().unwrap()["ok"], true);
    write_frame(&mut s, &json!({"id": 1, "method": "measure", "params": {"pad": "x".repeat(10_000)}})).unwrap();
    wait_all_closed(&server);
    let reply = read_frame(&mut s, 1 << 20).expect("the reply is not lost").expect("a reply before the end");
    assert_eq!(reply["error"]["code"], "invalid_params");
    assert!(read_frame(&mut s, 1 << 20).unwrap().is_none(), "then a clean end of stream");

    // Bytes that are not a frame, and a hello that claims a gigabyte and keeps sending.
    let mut junk = (1u32 << 30).to_be_bytes().to_vec();
    junk.extend_from_slice(&[b'z'; 20_000]);
    for bytes in [b"GET / HTTP/1.1\r\nHost: x\r\n\r\n".to_vec(), junk] {
        let mut s = TcpStream::connect(server.addr()).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        s.write_all(&bytes).unwrap();
        wait_all_closed(&server);
        let reply = read_frame(&mut s, 1 << 20).expect("the reply is not lost").expect("a reply before the end");
        assert_eq!(reply["ok"], false);
        assert!(reply["error"].as_str().unwrap().contains("authentication failed"));
        assert!(read_frame(&mut s, 1 << 20).unwrap().is_none(), "then a clean end of stream");
    }
    assert_eq!(server.refused_connections(), 2);
}

#[test]
fn large_payloads_cross_the_channel() {
    let app = App::start(McpPolicy::default());
    let mut r = app.remote();
    let big = "y".repeat(8 << 20);
    r.execute("settings.set", json!({"key": "units", "value": big})).unwrap();
    let back = r.inspect("settings.units", 0).unwrap();
    assert_eq!(back["value"].as_str().unwrap().len(), 8 << 20);
}

#[test]
fn concurrent_clients_are_served_one_at_a_time_without_loss() {
    let app = App::start(McpPolicy::default());
    let port = app.port;
    let handles: Vec<_> = (0..8)
        .map(|t| {
            std::thread::spawn(move || {
                let mut r = RemoteBackend::new(Connect::Direct { port, token: TOKEN.into() });
                let mut ids = Vec::new();
                for k in 0..25 {
                    let out = r.execute("shape.add_rect", json!({"x": t, "y": k, "width": 1, "height": 1})).unwrap();
                    ids.push(out["id"].as_str().unwrap().to_string());
                }
                ids
            })
        })
        .collect();
    let mut all: Vec<String> = handles.into_iter().flat_map(|h| h.join().unwrap()).collect();
    assert_eq!(all.len(), 200);
    all.sort();
    all.dedup();
    assert_eq!(all.len(), 200, "every call got its own shape id");
    let mut r = app.remote();
    assert_eq!(r.inspect("design.shapes", 0).unwrap().as_array().unwrap().len(), 200);
}

#[test]
fn a_request_times_out_when_the_ui_thread_does_not_pump() {
    let cfg = ControlConfig { port: 0, token: Some(TOKEN.into()), request_timeout: Duration::from_millis(200), ..Default::default() };
    // No pumping thread at all.
    let server = ControlServer::bind(cfg).unwrap();
    let mut c = ControlClient::connect(server.addr(), TOKEN).unwrap();
    let e = c.call("running", Value::Null).unwrap_err();
    assert_eq!(e.code, ErrorCode::Timeout, "{e}");
}

#[test]
fn the_waker_is_called_so_an_idle_ui_wakes_up() {
    let woke = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let w = woke.clone();
    let cfg = ControlConfig {
        port: 0,
        token: Some(TOKEN.into()),
        waker: Some(Arc::new(move || {
            w.fetch_add(1, Ordering::SeqCst);
        })),
        ..Default::default()
    };
    let app = App::start_with(McpPolicy::default(), cfg);
    app.remote().running().unwrap();
    assert!(woke.load(Ordering::SeqCst) >= 1);
}

#[test]
fn discovery_file_lets_the_client_find_the_app_and_is_removed_on_exit() {
    let dir = std::env::temp_dir().join(format!("darkburn-mcp-test-{}", std::process::id()));
    let file = dir.join("control.json");
    let _ = std::fs::remove_dir_all(&dir);
    let cfg = ControlConfig { port: 0, discovery_file: Some(file.clone()), ..Default::default() };
    let app = App::start_with(McpPolicy::default(), cfg);
    let d = darkburn_mcp::control::read_discovery(&file).unwrap();
    assert_eq!(d.port, app.port);
    assert_eq!(d.host, "127.0.0.1");
    assert_eq!(d.token.len(), 64, "256-bit random token");
    assert_eq!(d.pid, std::process::id());
    // Not the fixed test token: a fresh random one per session.
    assert_ne!(d.token, TOKEN);
    let mut r = RemoteBackend::new(Connect::File(file.clone()));
    assert_eq!(r.inspect("undo", 0).unwrap()["undo"], 0);
    app.finish();
    assert!(!file.exists(), "control.json is removed when the server stops");
    // With the app gone, the message says so.
    let mut r = RemoteBackend::new(Connect::File(file));
    let e = r.inspect("undo", 0).unwrap_err();
    assert_eq!(e.code, ErrorCode::Transport);
    assert!(e.message.contains("not running"), "{e}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn two_sessions_get_different_tokens() {
    let a = ControlServer::bind(ControlConfig { port: 0, ..Default::default() }).unwrap();
    let b = ControlServer::bind(ControlConfig { port: 0, ..Default::default() }).unwrap();
    assert_ne!(a.token(), b.token());
    assert_ne!(a.port(), b.port());
}

#[test]
fn the_client_reconnects_after_the_app_restarts() {
    let dir = std::env::temp_dir().join(format!("darkburn-mcp-restart-{}", std::process::id()));
    let file = dir.join("control.json");
    let _ = std::fs::remove_dir_all(&dir);
    let app = App::start_with(McpPolicy::default(), ControlConfig { port: 0, discovery_file: Some(file.clone()), ..Default::default() });
    let mut r = RemoteBackend::new(Connect::File(file.clone()));
    r.inspect("undo", 0).unwrap();
    app.finish();
    assert!(r.inspect("undo", 0).is_err());
    // A new session with a new port and token.
    let app2 = App::start_with(McpPolicy::default(), ControlConfig { port: 0, discovery_file: Some(file), ..Default::default() });
    assert!(r.inspect("undo", 0).is_ok(), "read-only calls reconnect on their own");
    drop(app2);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn full_stack_mcp_over_tcp_over_the_gate() {
    let app = App::start(McpPolicy::default());
    let mut s = mcp(Box::new(app.remote()));
    let init = rpc(&mut s, "initialize", json!({}));
    assert!(init["result"]["instructions"].as_str().unwrap().contains("127.0.0.1"));
    let r = call(&mut s, "execute", json!({"command": "shape.add_rect", "params": {"x": 0, "y": 0, "width": 3, "height": 3}}));
    assert!(!is_error(&r), "{}", text(&r));
    let r = call(&mut s, "execute", json!({"command": "machine.jog", "params": {"dx": 1, "dy": 1}}));
    assert!(is_error(&r) && text(&r).contains("[denied]"));
    let shot = call(&mut s, "screenshot", json!({}));
    assert_eq!(shot["content"][0]["type"], "image");
    let long = call(&mut s, "execute", json!({"command": "job.long_task"}));
    assert!(!is_error(&long), "{}", text(&long));
    let ev = json_of(&call(&mut s, "events", json!({})));
    assert!(ev["events"].as_array().unwrap().iter().any(|e| e["kind"] == "audit" && e["command"] == "machine.jog" && e["outcome"] == "denied"));
}

#[test]
fn mcp_server_says_so_when_the_app_is_not_running() {
    let mut s = mcp(Box::new(RemoteBackend::new(Connect::File(std::env::temp_dir().join("darkburn-no-such-dir").join("control.json")))));
    let r = call(&mut s, "list_commands", json!({}));
    assert!(is_error(&r));
    assert!(text(&r).contains("DarkBurn is not running"), "{}", text(&r));
    // The static answers still work.
    assert!(!text(&call(&mut s, "skill_reference", json!({}))).is_empty());
}
