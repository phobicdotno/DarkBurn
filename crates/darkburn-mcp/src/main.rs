//! `darkburn-mcp`: the stdio MCP server. Connects to a running DarkBurn app over the loopback
//! control channel, or with `--headless` serves an in-memory backend without a window.

use std::io::{BufReader, stdin, stdout};
use std::path::PathBuf;
use std::process::ExitCode;

use darkburn_mcp::control::{Connect, DEFAULT_PORT, control_file_path};
use darkburn_mcp::{Backend, Gate, MockBackend, PolicyHandle, RemoteBackend, mcp_server};

const HELP: &str = "darkburn-mcp: MCP server for DarkBurn (JSON-RPC over stdio)

USAGE: darkburn-mcp [--headless] [--control-file PATH] [--port N --token-file PATH]

  (default)             connect to the running DarkBurn app; the port and session token are read
                        from %APPDATA%\\DarkBurn\\control.json each time a connection is made
  --control-file PATH   read the port and token from this file instead
  --port N              port of the app's control channel (default 27879); needs --token-file
  --token-file PATH     file whose first line is the session token (with --port)
  --headless            serve an in-memory backend without a window (CI, tests); hardware is denied
  --version, --help
";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mut headless = false;
    let mut control_file: Option<PathBuf> = None;
    let mut port: Option<u16> = None;
    let mut token_file: Option<PathBuf> = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--headless" => headless = true,
            "--control-file" => control_file = args.next().map(PathBuf::from),
            "--port" => port = args.next().and_then(|p| p.parse().ok()),
            "--token-file" => token_file = args.next().map(PathBuf::from),
            "--version" | "-V" => {
                println!("darkburn-mcp {}", env!("CARGO_PKG_VERSION"));
                return ExitCode::SUCCESS;
            }
            "--help" | "-h" => {
                print!("{HELP}");
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("darkburn-mcp: unknown argument `{other}`\n\n{HELP}");
                return ExitCode::from(2);
            }
        }
    }
    let backend: Box<dyn Backend> = if headless {
        Box::new(Gate::new(MockBackend::new(), PolicyHandle::default()))
    } else if let Some(tf) = token_file {
        let token = match std::fs::read_to_string(&tf) {
            Ok(t) => t.lines().next().unwrap_or("").trim().to_string(),
            Err(e) => {
                eprintln!("darkburn-mcp: cannot read {}: {e}", tf.display());
                return ExitCode::from(2);
            }
        };
        Box::new(RemoteBackend::new(Connect::Direct { port: port.unwrap_or(DEFAULT_PORT), token }))
    } else {
        let file = control_file.unwrap_or_else(control_file_path);
        eprintln!("darkburn-mcp: control file {}", file.display());
        Box::new(RemoteBackend::new(Connect::File(file)))
    };
    let mut server = mcp_server(backend);
    match server.serve(BufReader::new(stdin().lock()), stdout().lock()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("darkburn-mcp: {e}");
            ExitCode::FAILURE
        }
    }
}
