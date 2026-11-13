//! The server half: hosted by the app. Accepts loopback connections on background threads and
//! hands requests to the UI thread through a channel.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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
    /// [`QUEUED`], then either [`RUNNING`] (the UI thread took it) or [`ABANDONED`] (its
    /// connection gave up first). Whoever changes it from `QUEUED` wins, so a request the client
    /// was told had failed can never run afterwards.
    state: Arc<AtomicU8>,
}

const QUEUED: u8 = 0;
const RUNNING: u8 = 1;
const ABANDONED: u8 = 2;
/// How often a connection that waits for the UI thread looks whether its client is still there.
const WATCH: Duration = Duration::from_millis(100);

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
                    if job.state.compare_exchange(QUEUED, RUNNING, Ordering::SeqCst, Ordering::SeqCst).is_err() {
                        // Its client timed out or went away while it waited: it is not run.
                        continue;
                    }
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
                    // The connection has ended and closed its side in an orderly way; drop the
                    // server's second handle to the socket so it is really closed.
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

/// How long a closing connection may stay quiet before the server stops waiting for the client.
const LINGER_IDLE: Duration = Duration::from_millis(250);
/// The longest a closing connection is kept, however busy the client still is.
const LINGER_MAX: Duration = Duration::from_secs(2);

fn connection(mut stream: TcpStream, tx: Sender<Job>, token: &str, refused: &AtomicUsize, cfg: &ControlConfig) {
    serve(&mut stream, tx, token, refused, cfg);
    linger_close(stream);
}

/// Closes a connection so that the last reply arrives.
///
/// Closing a socket that still holds unread input makes the system send a reset instead of a
/// normal end of stream, and a reset can throw away a reply the client has not read yet (it
/// does on Windows). So: end our side of the stream, which sends everything written so far,
/// then read and discard what the client has sent until it closes too or goes quiet, and only
/// then let go of the socket.
fn linger_close(mut stream: TcpStream) {
    use std::io::Read;
    if stream.shutdown(std::net::Shutdown::Write).is_err() {
        return;
    }
    let _ = stream.set_read_timeout(Some(LINGER_IDLE));
    let end = Instant::now() + LINGER_MAX;
    let mut sink = [0u8; 16 * 1024];
    // Ends on end of stream, on a quiet client (the read times out), on any error and at the
    // overall limit, so a client that never stops sending cannot hold the connection open.
    while Instant::now() < end && matches!(stream.read(&mut sink), Ok(n) if n > 0) {}
}

/// Whether the client closed or lost its connection (nothing is read from it).
fn peer_gone(stream: &TcpStream) -> bool {
    use std::io::ErrorKind::{Interrupted, TimedOut, WouldBlock};
    if stream.set_nonblocking(true).is_err() {
        return false;
    }
    let mut byte = [0u8; 1];
    let gone = match stream.peek(&mut byte) {
        Ok(0) => true,
        Ok(_) => false,
        Err(e) => !matches!(e.kind(), WouldBlock | TimedOut | Interrupted),
    };
    let _ = stream.set_nonblocking(false);
    gone
}

/// How long a new connection has to send its whole hello.
const HELLO_TIME: Duration = Duration::from_secs(5);

/// Reads from a stream until a fixed moment; a read after it fails as timed out.
struct ByDeadline<'a> {
    stream: &'a TcpStream,
    end: Instant,
}

impl std::io::Read for ByDeadline<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let left = self.end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(std::io::ErrorKind::TimedOut.into());
        }
        self.stream.set_read_timeout(Some(left))?;
        std::io::Read::read(&mut self.stream, buf)
    }
}

fn serve(stream: &mut TcpStream, tx: Sender<Job>, token: &str, refused: &AtomicUsize, cfg: &ControlConfig) {
    // Handshake: the first frame is a small hello with the token, within five seconds in all
    // (not per read, or a client sending a byte every few seconds keeps its slot for hours).
    let hello = read_frame(&mut ByDeadline { stream: &*stream, end: Instant::now() + HELLO_TIME }, HELLO_MAX);
    let ok = match hello {
        Ok(Some(v)) => v["hello"]["token"].as_str().is_some_and(|t| ct_eq(t.as_bytes(), token.as_bytes())),
        _ => false,
    };
    if !ok {
        refused.fetch_add(1, Ordering::Relaxed);
        // A pause makes guessing slow; the reply says nothing about which part was wrong.
        std::thread::sleep(Duration::from_millis(100));
        let _ = write_frame(
            stream,
            &json!({"ok": false, "error": "authentication failed: missing or wrong token (read it from DarkBurn's control.json)"}),
        );
        return;
    }
    if write_frame(stream, &json!({"ok": true, "server": "DarkBurn", "version": PROTOCOL})).is_err() {
        return;
    }
    let _ = stream.set_read_timeout(None);
    loop {
        let req = match read_frame(stream, cfg.max_frame) {
            Ok(Some(v)) => v,
            Ok(None) => return,
            Err(e) => {
                if e.kind() == std::io::ErrorKind::InvalidData {
                    let _ = write_frame(stream, &json!({"id": null, "ok": false, "error": {"code": "invalid_params", "message": e.to_string()}}));
                }
                return;
            }
        };
        let id = req["id"].clone();
        let reply = match req["method"].as_str() {
            None => Err(CommandError::invalid("request has no `method`")),
            Some(m) => {
                let (rtx, rrx) = mpsc::sync_channel(1);
                let state = Arc::new(AtomicU8::new(QUEUED));
                let job = Job { method: m.to_string(), params: req.get("params").cloned().unwrap_or(Value::Null), reply: rtx, state: state.clone() };
                if tx.send(job).is_err() {
                    return;
                }
                if let Some(w) = &cfg.waker {
                    w();
                }
                let deadline = Instant::now() + cfg.request_timeout;
                loop {
                    let left = deadline.saturating_duration_since(Instant::now());
                    match rrx.recv_timeout(WATCH.min(left)) {
                        Ok(r) => break r,
                        Err(mpsc::RecvTimeoutError::Disconnected) => return,
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                    let gone = peer_gone(stream);
                    if !gone && Instant::now() < deadline {
                        continue;
                    }
                    if state.compare_exchange(QUEUED, ABANDONED, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
                        if gone {
                            return;
                        }
                        break Err(CommandError::new(
                            ErrorCode::Timeout,
                            "DarkBurn did not answer in time (the app may be frozen or closed). The request was dropped: it did not run and will not run later.",
                        ));
                    }
                    // The UI thread is running it at this moment: what it answers is what happened.
                    match rrx.recv() {
                        Ok(r) if !gone => break r,
                        _ => return,
                    }
                }
            }
        };
        let frame = match reply {
            Ok(v) => json!({"id": id, "ok": true, "result": v}),
            Err(e) => json!({"id": id, "ok": false, "error": e}),
        };
        if write_frame(stream, &frame).is_err() {
            return;
        }
    }
}
