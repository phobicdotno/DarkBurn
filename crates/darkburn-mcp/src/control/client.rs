//! The client half, used by the `darkburn-mcp` binary: one authenticated connection, and a
//! [`Backend`] that forwards every call to the app over it.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Value, json};

use super::{DEFAULT_MAX_FRAME, PROTOCOL, control_file_path, read_discovery, read_frame, write_frame};
use crate::backend::Backend;
use crate::types::*;
use crate::util::base64_decode;

pub struct ControlClient {
    stream: TcpStream,
    next_id: u64,
    max_frame: usize,
}

impl ControlClient {
    /// Connect to `addr` (must be loopback) and authenticate with `token`.
    pub fn connect(addr: SocketAddr, token: &str) -> Result<Self, CommandError> {
        if !super::is_allowed_peer(addr.ip()) {
            return Err(CommandError::transport(format!("refusing to connect to {addr}: the control channel is loopback only")));
        }
        let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(3))
            .map_err(|e| CommandError::transport(format!("cannot reach DarkBurn at {addr}: {e}. Is the app running with the control channel on?")))?;
        let _ = stream.set_nodelay(true);
        let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
        write_frame(
            &mut stream,
            &json!({"hello": {"token": token, "client": concat!("darkburn-mcp ", env!("CARGO_PKG_VERSION")), "version": PROTOCOL}}),
        )
        .map_err(|e| CommandError::transport(format!("handshake failed: {e}")))?;
        let reply = read_frame(&mut stream, super::HELLO_MAX).map_err(|e| CommandError::transport(format!("handshake failed: {e}")))?;
        match reply {
            Some(v) if v["ok"] == true => {}
            Some(v) => return Err(CommandError::transport(v["error"].as_str().unwrap_or("handshake refused").to_string())),
            None => return Err(CommandError::transport("DarkBurn closed the connection during the handshake (wrong token?)")),
        }
        let _ = stream.set_read_timeout(None);
        Ok(Self { stream, next_id: 1, max_frame: DEFAULT_MAX_FRAME })
    }

    /// Set how long a call waits for the reply (None: forever).
    pub fn set_timeout(&mut self, t: Option<Duration>) {
        let _ = self.stream.set_read_timeout(t);
    }

    /// One request, one reply.
    pub fn call(&mut self, method: &str, params: Value) -> Result<Value, CommandError> {
        let id = self.next_id;
        self.next_id += 1;
        write_frame(&mut self.stream, &json!({"id": id, "method": method, "params": params}))
            .map_err(|e| CommandError::transport(format!("lost the connection to DarkBurn: {e}")))?;
        let reply = read_frame(&mut self.stream, self.max_frame)
            .map_err(|e| CommandError::transport(format!("lost the connection to DarkBurn: {e}")))?
            .ok_or_else(|| CommandError::transport("DarkBurn closed the connection"))?;
        if reply["id"] != json!(id) && !reply["id"].is_null() {
            return Err(CommandError::transport("out-of-order reply from DarkBurn"));
        }
        if reply["ok"] == true {
            Ok(reply.get("result").cloned().unwrap_or(Value::Null))
        } else {
            Err(serde_json::from_value(reply["error"].clone()).unwrap_or_else(|_| CommandError::transport("malformed error reply from DarkBurn")))
        }
    }
}

/// How a [`RemoteBackend`] finds the app.
#[derive(Clone, Debug)]
pub enum Connect {
    /// Read the port and token from `control.json` on every (re)connect: the app may start after
    /// the MCP server, or restart with a new token.
    File(PathBuf),
    Direct {
        port: u16,
        token: String,
    },
}

impl Default for Connect {
    fn default() -> Self {
        Connect::File(control_file_path())
    }
}

/// A [`Backend`] over the control channel. Connects lazily and reconnects after a failure.
pub struct RemoteBackend {
    connect: Connect,
    client: Option<ControlClient>,
}

impl RemoteBackend {
    pub fn new(connect: Connect) -> Self {
        Self { connect, client: None }
    }

    fn open(&self) -> Result<ControlClient, CommandError> {
        let (port, token, host) = match &self.connect {
            Connect::Direct { port, token } => (*port, token.clone(), IpAddr::V4(Ipv4Addr::LOCALHOST)),
            Connect::File(p) => {
                let d = read_discovery(p).map_err(|e| {
                    CommandError::transport(format!("DarkBurn is not running (or its control channel is off): {e}. Start DarkBurn, then call again."))
                })?;
                let host: IpAddr = d.host.parse().map_err(|_| CommandError::transport("control.json holds a bad host address"))?;
                (d.port, d.token, host)
            }
        };
        ControlClient::connect(SocketAddr::new(host, port), &token)
    }

    pub fn call(&mut self, method: &str, params: Value) -> Result<Value, CommandError> {
        for attempt in 0..2 {
            if self.client.is_none() {
                self.client = Some(self.open()?);
            }
            let Some(c) = self.client.as_mut() else { break };
            match c.call(method, params.clone()) {
                Err(e) if e.code == ErrorCode::Transport => {
                    self.client = None;
                    // Retry once, but only for requests that cannot have changed anything.
                    let safe = matches!(
                        method,
                        "ping"
                            | "describe"
                            | "list_commands"
                            | "describe_command"
                            | "inspect"
                            | "pending"
                            | "running"
                            | "events"
                            | "screenshot"
                            | "crash_reports"
                    );
                    if attempt == 0 && safe {
                        continue;
                    }
                    return Err(e);
                }
                r => return r,
            }
        }
        Err(CommandError::transport("DarkBurn is not reachable"))
    }

    fn typed<T: for<'de> serde::Deserialize<'de>>(&mut self, method: &str, params: Value) -> Result<T, CommandError> {
        let v = self.call(method, params)?;
        serde_json::from_value(v).map_err(|e| CommandError::transport(format!("unexpected reply to `{method}`: {e}")))
    }
}

impl Backend for RemoteBackend {
    fn describe(&self) -> String {
        match &self.connect {
            Connect::Direct { port, .. } => format!("DarkBurn app at 127.0.0.1:{port}"),
            Connect::File(p) => format!("DarkBurn app (control file {})", p.display()),
        }
    }
    fn list_commands(&mut self, q: &ListQuery) -> Result<CommandPage, CommandError> {
        self.typed("list_commands", json!(q))
    }
    fn describe_command(&mut self, id: &str) -> Result<Option<CommandInfo>, CommandError> {
        self.typed("describe_command", json!({"id": id}))
    }
    fn execute(&mut self, id: &str, args: Value) -> Result<Value, CommandError> {
        self.execute_ctx(id, args, &CallContext::default())
    }
    fn execute_ctx(&mut self, id: &str, args: Value, ctx: &CallContext) -> Result<Value, CommandError> {
        self.call("execute", json!({"id": id, "args": args, "tool": ctx.tool, "confirmation_token": ctx.confirmation_token}))
    }
    fn batch(&mut self, steps: &[BatchStep], all_or_nothing: bool) -> Result<BatchResult, CommandError> {
        self.typed("batch", json!({"steps": steps, "all_or_nothing": all_or_nothing}))
    }
    fn inspect(&mut self, path: &str, depth: u32) -> Result<Value, CommandError> {
        self.call("inspect", json!({"path": path, "depth": depth}))
    }
    fn screenshot(&mut self, region: Option<Region>) -> Result<Vec<u8>, CommandError> {
        let v = self.call("screenshot", json!({"region": region}))?;
        v["png_base64"].as_str().and_then(base64_decode).ok_or_else(|| CommandError::transport("the screenshot reply carried no image"))
    }
    fn measure(&mut self, request: Value) -> Result<Value, CommandError> {
        self.call("measure", request)
    }
    fn preview_job(&mut self, request: Value) -> Result<Value, CommandError> {
        self.call("preview_job", request)
    }
    fn pending(&mut self, id: &str) -> Result<PendingState, CommandError> {
        self.typed("pending", json!({"id": id}))
    }
    fn running(&mut self) -> Result<Vec<RunningOp>, CommandError> {
        self.typed("running", Value::Null)
    }
    fn events(&mut self, since: u64) -> Result<EventPage, CommandError> {
        self.typed("events", json!({"since": since}))
    }
    fn crash_reports(&mut self, limit: usize) -> Result<Value, CommandError> {
        self.call("crash_reports", json!({"limit": limit}))
    }
}
