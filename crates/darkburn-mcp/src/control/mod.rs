//! The control channel between the stdio MCP server (`darkburn-mcp`) and a RUNNING DarkBurn app.
//!
//! # Wire protocol (version 1)
//!
//! TCP on `127.0.0.1` only. Every message is a frame: a 4-byte big-endian length, then that many
//! bytes of UTF-8 JSON. Frames before authentication are limited to [`HELLO_MAX`] bytes, after it
//! to [`ControlConfig::max_frame`] (default 64 MiB; screenshots travel as base64).
//!
//! ```text
//! client -> {"hello": {"token": "<hex>", "client": "darkburn-mcp 0.1", "version": 1}}
//! server -> {"ok": true, "server": "DarkBurn", "version": 1}          (or {"ok": false, "error": ".."} and close)
//! client -> {"id": 7, "method": "execute", "params": {"id": "file.new", "args": {}, "tool": "new_design"}}
//! server -> {"id": 7, "ok": true, "result": {...}}
//!        or {"id": 7, "ok": false, "error": {"code": "denied", "message": "..."}}
//! ```
//!
//! # Security
//!
//! * the server binds to loopback only and drops any peer whose address is not loopback;
//! * a random 256-bit token is created per app session and written to `control.json` in the
//!   user's profile (`%APPDATA%\DarkBurn\control.json`; mode 0600 on Unix); the first frame must
//!   carry it, compared in constant time; a wrong token closes the connection;
//! * the safety gate ([`crate::Gate`]) runs on the app side, so a client cannot skip it.
//!
//! # Threading
//!
//! See [`crate::backend`]: connection threads only queue requests; the UI thread runs
//! [`ControlServer::pump`] and is the only one to touch the backend.

mod client;
mod rpc;
mod server;

use std::io::{self, Read, Write};
use std::net::IpAddr;
use std::path::PathBuf;

use serde_json::Value;

pub use client::{Connect, ControlClient, RemoteBackend};
pub use rpc::dispatch;
pub use server::{ControlConfig, ControlServer};

/// The default port (SolidCraft uses 27878).
pub const DEFAULT_PORT: u16 = 27879;
/// Protocol version spoken.
pub const PROTOCOL: u32 = 1;
/// Largest frame accepted before the client has authenticated.
pub const HELLO_MAX: usize = 4096;
/// Default largest frame after authentication.
pub const DEFAULT_MAX_FRAME: usize = 64 << 20;

/// Write one frame.
pub fn write_frame(w: &mut impl Write, v: &Value) -> io::Result<()> {
    let bytes = serde_json::to_vec(v).map_err(io::Error::other)?;
    let len = u32::try_from(bytes.len()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "frame too large"))?;
    // One write so a frame is never interleaved or half-sent by a concurrent writer.
    let mut buf = Vec::with_capacity(bytes.len() + 4);
    buf.extend_from_slice(&len.to_be_bytes());
    buf.extend_from_slice(&bytes);
    w.write_all(&buf)?;
    w.flush()
}

/// Read one frame. `Ok(None)` on a clean end of stream between frames. A frame longer than `max`
/// is refused before any of it is read into memory.
pub fn read_frame(r: &mut impl Read, max: usize) -> io::Result<Option<Value>> {
    let mut len = [0u8; 4];
    match r.read(&mut len[..1])? {
        0 => return Ok(None),
        _ => r.read_exact(&mut len[1..])?,
    }
    let n = u32::from_be_bytes(len) as usize;
    if n > max {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("frame of {n} bytes exceeds the limit of {max}")));
    }
    // Read in chunks so a lying length cannot make us allocate `n` up front.
    let mut buf = Vec::with_capacity(n.min(1 << 20));
    let mut remaining = n;
    let mut chunk = [0u8; 16 * 1024];
    while remaining > 0 {
        let want = remaining.min(chunk.len());
        r.read_exact(&mut chunk[..want])?;
        buf.extend_from_slice(&chunk[..want]);
        remaining -= want;
    }
    serde_json::from_slice(&buf).map(Some).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("bad JSON frame: {e}")))
}

/// True for the peers the server accepts: loopback addresses only.
pub fn is_allowed_peer(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback(),
        IpAddr::V6(v6) => v6.is_loopback() || v6.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback()),
    }
}

/// What `control.json` holds.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Discovery {
    pub host: String,
    pub port: u16,
    pub token: String,
    pub pid: u32,
    pub started_ms: u64,
}

/// Where the discovery file lives: `$DARKBURN_CONTROL_FILE`, else `%APPDATA%\DarkBurn\control.json`,
/// else `~/.config/DarkBurn/control.json`.
pub fn control_file_path() -> PathBuf {
    if let Some(p) = std::env::var_os("DARKBURN_CONTROL_FILE") {
        return PathBuf::from(p);
    }
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("DarkBurn").join("control.json")
}

/// Write the discovery file readable by the user only.
pub fn write_discovery(path: &std::path::Path, d: &Discovery) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text = serde_json::to_string_pretty(d).map_err(io::Error::other)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)?;
        f.write_all(text.as_bytes())
    }
    #[cfg(not(unix))]
    {
        // %APPDATA% is only readable by the user and SYSTEM/administrators by its inherited ACL.
        std::fs::write(path, text)
    }
}

/// Read the discovery file.
pub fn read_discovery(path: &std::path::Path) -> Result<Discovery, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{} is not a valid control file: {e}", path.display()))
}
