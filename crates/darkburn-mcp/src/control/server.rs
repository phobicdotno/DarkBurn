//! The server half: hosted by the app. Accepts loopback connections on background threads and
//! hands requests to the UI thread through a channel.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

use super::{DEFAULT_MAX_FRAME, DEFAULT_PORT, Discovery, HELLO_MAX, PROTOCOL, dispatch, is_allowed_peer, read_frame, write_discovery, write_frame};
use crate::backend::Backend;
use crate::types::{CommandError, ErrorCode, now_ms};
use crate::util::{ct_eq, random_hex};

/// Wake the UI thread (for egui: `ctx.request_repaint()`), so a request is served even when the
/// window is idle.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// Open connections by id, so the server can close them all and a finished one can leave.
type Streams = Arc<Mutex<Vec<(u64, TcpStream)>>>;

#[derive(Clone)]
pub struct ControlConfig {
    /// Address to bind. Must be loopback; anything else is refused.
    pub host: IpAddr,
    /// Port; 0 picks a free one (tests).
    pub port: u16,
    /// Session token; `None` makes a random one.
    pub token: Option<String>,
    pub max_frame: usize,
    pub max_connections: usize,
    /// How long a connection waits for the UI thread to answer one request.
    pub request_timeout: Duration,
    pub waker: Option<Waker>,
    /// Where to write `control.json`; `None` writes nothing (tests), `Some` writes it and removes
    /// it when the server is dropped.
    pub discovery_file: Option<PathBuf>,
}

impl Default for ControlConfig {
    fn default() -> Self {
        Self {
            host: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: DEFAULT_PORT,
            token: None,
            max_frame: DEFAULT_MAX_FRAME,
            max_connections: 8,
            request_timeout: Duration::from_secs(600),
            waker: None,
            discovery_file: None,
        }
    }
}

struct Job {
    method: String,
    params: Value,
    reply: SyncSender<Result<Value, CommandError>>,
}

pub struct ControlServer {
    addr: SocketAddr,
    token: String,
    rx: Receiver<Job>,
    shutdown: Arc<AtomicBool>,
    streams: Streams,
    discovery: Option<PathBuf>,
    refused: Arc<AtomicUsize>,
    active: Arc<AtomicUsize>,
}

impl ControlServer {
    /// Bind and start accepting. Fails if `cfg.host` is not loopback.
    pub fn bind(cfg: ControlConfig) -> std::io::Result<Self> {
        if !cfg.host.is_loopback() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!("the control channel only binds to loopback, not {}", cfg.host),
            ));
        }
        let listener = TcpListener::bind(SocketAddr::new(cfg.host, cfg.port))?;
        listener.set_nonblocking(true)?;
        let addr = listener.local_addr()?;
        let token = cfg.token.clone().unwrap_or_else(|| random_hex(32));
        let (tx, rx) = mpsc::channel::<Job>();
        let shutdown = Arc::new(AtomicBool::new(false));
        let streams: Streams = Arc::new(Mutex::new(Vec::new()));
        let active = Arc::new(AtomicUsize::new(0));
        let refused = Arc::new(AtomicUsize::new(0));
        {
            let (shutdown, streams, token, active, refused, cfg) =
                (shutdown.clone(), streams.clone(), token.clone(), active.clone(), refused.clone(), cfg.clone());
            std::thread::Builder::new()
                .name("darkburn-control-accept".into())
                .spawn(move || accept_loop(listener, tx, shutdown, streams, token, active, refused, cfg))?;
        }
        let discovery = cfg.discovery_file.clone();
        if let Some(p) = &discovery {
            write_discovery(
                p,
                &Discovery { host: addr.ip().to_string(), port: addr.port(), token: token.clone(), pid: std::process::id(), started_ms: now_ms() },
            )?;
        }
        Ok(Self { addr, token, rx, shutdown, streams, discovery, refused, active })
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }
    pub fn port(&self) -> u16 {
        self.addr.port()
    }
    pub fn token(&self) -> &str {
        &self.token
    }
    /// Clients connected now (authenticated or still in the handshake).
    pub fn connections(&self) -> usize {
        self.active.load(Ordering::SeqCst)
    }
    /// Connections dropped for a non-loopback peer, a wrong token or a bad hello.
    pub fn refused_connections(&self) -> usize {
        self.refused.load(Ordering::Relaxed)
    }

    /// Serve up to `max` queued requests on `backend`. Call it on the UI thread once per frame
    /// (`backend` is the [`crate::Gate`] around the app's backend). Returns how many it served.
    pub fn pump(&self, backend: &mut dyn Backend, max: usize) -> usize {
        let mut n = 0;
        while n < max {
            match self.rx.try_recv() {
                Ok(job) => {
                    let r = dispatch(backend, &job.method, job.params);
                    let _ = job.reply.send(r);
                    n += 1;
                }
                Err(_) => break,
            }
        }
        n
    }
}

impl Drop for ControlServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        for (_, s) in self.streams.lock().unwrap_or_else(|e| e.into_inner()).drain(..) {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }
        if let Some(p) = &self.discovery
            && let Ok(d) = super::read_discovery(p)
            && d.token == self.token
        {
            let _ = std::fs::remove_file(p);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn accept_loop(
    listener: TcpListener,
    tx: Sender<Job>,
    shutdown: Arc<AtomicBool>,
    streams: Streams,
    token: String,
    active: Arc<AtomicUsize>,
    refused: Arc<AtomicUsize>,
    cfg: ControlConfig,
) {
    let mut conn_id = 0u64;
    while !shutdown.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, peer)) => {
                if !is_allowed_peer(peer.ip()) || active.load(Ordering::SeqCst) >= cfg.max_connections {
                    refused.fetch_add(1, Ordering::Relaxed);
                    let _ = stream.shutdown(std::net::Shutdown::Both);
                    continue;
                }
                let _ = stream.set_nonblocking(false);
                let _ = stream.set_nodelay(true);
                conn_id += 1;
                if let Ok(c) = stream.try_clone() {
                    streams.lock().unwrap_or_else(|e| e.into_inner()).push((conn_id, c));
                }
                let (my_id, my_streams) = (conn_id, streams.clone());
                active.fetch_add(1, Ordering::SeqCst);
                let (tx, token, refused, cfg) = (tx.clone(), token.clone(), refused.clone(), cfg.clone());
                let active = active.clone();
                let a2 = active.clone();
                let spawned = std::thread::Builder::new().name("darkburn-control-conn".into()).spawn(move || {
                    connection(stream, tx, &token, &refused, &cfg);
                    // Close the socket for real: the server's list holds a second handle to it.
                    let mut list = my_streams.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(i) = list.iter().position(|(id, _)| *id == my_id) {
                        let (_, s) = list.swap_remove(i);
                        let _ = s.shutdown(std::net::Shutdown::Both);
                    }
                    active.fetch_sub(1, Ordering::SeqCst);
                });
                if spawned.is_err() {
                    a2.fetch_sub(1, Ordering::SeqCst);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(15)),
            Err(_) => std::thread::sleep(Duration::from_millis(100)),
        }
    }
}

fn connection(mut stream: TcpStream, tx: Sender<Job>, token: &str, refused: &AtomicUsize, cfg: &ControlConfig) {
    // Handshake: the first frame is a small hello with the token, within five seconds.
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let hello = read_frame(&mut stream, HELLO_MAX);
    let ok = match hello {
        Ok(Some(v)) => v["hello"]["token"].as_str().is_some_and(|t| ct_eq(t.as_bytes(), token.as_bytes())),
        _ => false,
    };
    if !ok {
        refused.fetch_add(1, Ordering::Relaxed);
        // A pause makes guessing slow; the reply says nothing about which part was wrong.
        std::thread::sleep(Duration::from_millis(100));
        let _ = write_frame(
            &mut stream,
            &json!({"ok": false, "error": "authentication failed: missing or wrong token (read it from DarkBurn's control.json)"}),
        );
        let _ = stream.shutdown(std::net::Shutdown::Both);
        return;
    }
    if write_frame(&mut stream, &json!({"ok": true, "server": "DarkBurn", "version": PROTOCOL})).is_err() {
        return;
    }
    let _ = stream.set_read_timeout(None);
    loop {
        let req = match read_frame(&mut stream, cfg.max_frame) {
            Ok(Some(v)) => v,
            Ok(None) => return,
            Err(e) => {
                if e.kind() == std::io::ErrorKind::InvalidData {
                    let _ =
                        write_frame(&mut stream, &json!({"id": null, "ok": false, "error": {"code": "invalid_params", "message": e.to_string()}}));
                }
                return;
            }
        };
        let id = req["id"].clone();
        let reply = match req["method"].as_str() {
            None => Err(CommandError::invalid("request has no `method`")),
            Some(m) => {
                let (rtx, rrx) = mpsc::sync_channel(1);
                if tx.send(Job { method: m.to_string(), params: req.get("params").cloned().unwrap_or(Value::Null), reply: rtx }).is_err() {
                    return;
                }
                if let Some(w) = &cfg.waker {
                    w();
                }
                match rrx.recv_timeout(cfg.request_timeout) {
                    Ok(r) => r,
                    Err(_) => Err(CommandError::new(ErrorCode::Timeout, "DarkBurn did not answer in time (the app may be frozen or closed)")),
                }
            }
        };
        let frame = match reply {
            Ok(v) => json!({"id": id, "ok": true, "result": v}),
            Err(e) => json!({"id": id, "ok": false, "error": e}),
        };
        if write_frame(&mut stream, &frame).is_err() {
            return;
        }
    }
}
