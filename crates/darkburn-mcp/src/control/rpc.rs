//! Method names and parameters of the control channel, shared by the server (dispatch to a
//! backend) and the client ([`super::RemoteBackend`]).

use serde::Deserialize;
use serde_json::{Value, json};

use crate::backend::Backend;
use crate::types::*;
use solidcraft_mcp_server::base64;

fn parse<T: for<'de> Deserialize<'de>>(method: &str, params: Value) -> Result<T, CommandError> {
    serde_json::from_value(params).map_err(|e| CommandError::invalid(format!("control method `{method}`: bad params: {e}")))
}

#[derive(Deserialize)]
struct ExecuteP {
    id: String,
    #[serde(default)]
    args: Value,
    #[serde(default)]
    tool: String,
    #[serde(default)]
    confirmation_token: Option<String>,
}
#[derive(Deserialize)]
struct BatchP {
    steps: Vec<BatchStep>,
    #[serde(default = "yes")]
    all_or_nothing: bool,
}
fn yes() -> bool {
    true
}
#[derive(Deserialize)]
struct InspectP {
    path: String,
    #[serde(default)]
    depth: u32,
}
#[derive(Deserialize)]
struct IdP {
    id: String,
}
#[derive(Deserialize, Default)]
struct RegionP {
    #[serde(default)]
    region: Option<Region>,
}
#[derive(Deserialize)]
struct SinceP {
    #[serde(default)]
    since: u64,
}
#[derive(Deserialize)]
struct LimitP {
    #[serde(default)]
    limit: Option<usize>,
}

fn to_value<T: serde::Serialize>(v: &T) -> Result<Value, CommandError> {
    serde_json::to_value(v).map_err(|e| CommandError::failed(format!("cannot encode reply: {e}")))
}

/// Run one control method on a backend. This is the only code that turns wire requests into
/// backend calls; it runs on the UI thread inside [`super::ControlServer::pump`].
pub fn dispatch(b: &mut dyn Backend, method: &str, params: Value) -> Result<Value, CommandError> {
    match method {
        "ping" => Ok(json!({"pong": true})),
        "describe" => Ok(json!({"describe": b.describe()})),
        "list_commands" => to_value(&b.list_commands(&parse(method, params)?)?),
        "describe_command" => {
            let p: IdP = parse(method, params)?;
            to_value(&b.describe_command(&p.id)?)
        }
        "execute" => {
            let p: ExecuteP = parse(method, params)?;
            let ctx = CallContext { tool: p.tool, confirmation_token: p.confirmation_token };
            b.execute_ctx(&p.id, p.args, &ctx)
        }
        "batch" => {
            let p: BatchP = parse(method, params)?;
            to_value(&b.batch(&p.steps, p.all_or_nothing)?)
        }
        "inspect" => {
            let p: InspectP = parse(method, params)?;
            b.inspect(&p.path, p.depth)
        }
        "screenshot" => {
            let p: RegionP = if params.is_null() { RegionP::default() } else { parse(method, params)? };
            let png = b.screenshot(p.region)?;
            Ok(json!({"png_base64": base64(&png)}))
        }
        "measure" => b.measure(params),
        "preview_job" => b.preview_job(params),
        "pending" => {
            let p: IdP = parse(method, params)?;
            to_value(&b.pending(&p.id)?)
        }
        "running" => to_value(&b.running()?),
        "events" => {
            let p: SinceP = parse(method, params)?;
            to_value(&b.events(p.since)?)
        }
        "crash_reports" => {
            let p: LimitP = parse(method, params)?;
            b.crash_reports(p.limit.unwrap_or(20))
        }
        _ => Err(CommandError::new(ErrorCode::NotSupported, format!("unknown control method `{method}`"))),
    }
}
