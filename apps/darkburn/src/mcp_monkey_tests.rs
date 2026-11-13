//! Bug hunt 7, part 2: the monkey on the agent (MCP) interface.
//!
//! From a fixed seed, a long sequence of JSON-RPC lines goes into the real MCP server, which sits
//! on the real safety gate, which sits on the app's real backend: tool calls with generated
//! arguments (of the right kind, of the wrong kind, missing, huge numbers, "NaN" as text, deep
//! nesting, very long texts), batches, unknown tools and lines that are not JSON-RPC at all. Every
//! line must get an answer (a result or an error), in time, without a panic.
//!
//! Hardware safety: between the gate and the app stands the [`Tripwire`]. A command that acts on
//! a machine is NEVER handed on to the app, whatever the gate decided: the tripwire only writes
//! down that it arrived. With "Allow AI agents to control machines" off nothing but the stop
//! commands may arrive there; with it on, nothing that starts a job or fires may arrive without
//! a click by the user (the monkey never clicks), and nothing for a Gweike machine while that
//! option is off. Commands the document monkey leaves out (file dialogs, files outside the
//! temporary folder, the network, quit) are held back in the same place.
//!
//! By design, and so no failure here: the stop commands (emergency stop, pause, stop) pass the
//! gate always; a `wait_ms` is honoured up to ten minutes (the monkey asks for milliseconds); with
//! eight confirmations waiting no further one is asked for.
//!
//! Explored beyond the seeds kept below, 800 lines each: machine control off, seeds 1000..1015 (all
//! passed; 3 to 11 stop commands arrived, nothing else); machine control on, seeds 2000..2015 (57 to
//! 83 machine commands arrived at the tripwire, 8 confirmations asked for and none given). One of
//! those runs is over the time limit and stays so: seed 2012 line 760, an Offset of five texts of
//! 3000 characters each, squashed to a fifteen-thousandth of their width (230 s; see
//! docs/BUGHUNT-NOTES.md, open suspicions).

use std::cell::RefCell;
use std::rc::Rc;

use darkburn_mcp::server::Server;
use darkburn_mcp::tools::{DarkburnTools, INFO};
use darkburn_mcp::{
    Backend, CommandError, CommandInfo, CommandPage, EventPage, Gate, HardwareClass, ListQuery, McpPolicy, PendingState, PolicyHandle, Region,
    RunningOp, tool_definitions,
};

use super::*;
use crate::mcp_backend::{McpBackend, SharedApp};
use crate::mcp_catalog;

/// What arrived behind the gate.
#[derive(Default)]
struct Seen {
    /// Machine commands the gate let through: (id, class, for a Gweike machine).
    reached: Vec<(String, Option<HardwareClass>, bool)>,
    /// Confirmation dialogs the gate asked the app for (command ids).
    asked: Vec<String>,
    ran: u32,
    held: u32,
}

/// Whether `id` acts on a machine or changes its set-up, by every list the app has.
fn machine_command(id: &str) -> bool {
    mcp_catalog::is_hardware(id) || mcp_catalog::acts_on_machine(id) || commands::hardware(id)
}

/// Whether the name of an argument says it is a file or a folder.
fn names_a_file(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    ["path", "file", "folder", "dir", "output", "dest"].iter().any(|w| k.contains(w)) && k != "svg_path" && !k.ends_with("_id") && k != "path_shape"
}

/// Whether `args` names a file outside `dir` anywhere.
fn leaves_the_folder(args: &Value, dir: &std::path::Path) -> bool {
    match args {
        Value::Object(o) => o.iter().any(|(k, v)| {
            let outside =
                names_a_file(k) && !v.is_null() && !v.as_str().is_some_and(|s| std::path::Path::new(s).starts_with(dir) && !s.contains(".."));
            outside || leaves_the_folder(v, dir)
        }),
        Value::Array(a) => a.iter().any(|v| leaves_the_folder(v, dir)),
        _ => false,
    }
}

/// Why the tripwire does not hand `id` on to the app; None when it does.
fn held(id: &str, args: &Value, dir: &std::path::Path) -> Option<&'static str> {
    if id == "settings.set" {
        // Only the keys the document monkey may change, and the agent settings themselves (which
        // no agent may change: that is checked after every call).
        let key = args.get("key").and_then(Value::as_str).unwrap_or("");
        return (!(SETTING_KEYS.contains(&key) || key == "mcp" || key.starts_with("mcp."))).then_some("a preference the monkey leaves alone");
    }
    if let Some(why) = excluded(id) {
        return Some(why);
    }
    leaves_the_folder(args, dir).then_some("a file outside the temporary folder")
}

struct Tripwire {
    inner: McpBackend,
    seen: Rc<RefCell<Seen>>,
    dir: PathBuf,
}

impl Backend for Tripwire {
    fn describe(&self) -> String {
        self.inner.describe()
    }
    fn list_commands(&mut self, query: &ListQuery) -> Result<CommandPage, CommandError> {
        self.inner.list_commands(query)
    }
    fn describe_command(&mut self, id: &str) -> Result<Option<CommandInfo>, CommandError> {
        self.inner.describe_command(id)
    }
    fn execute(&mut self, id: &str, args: Value) -> Result<Value, CommandError> {
        if machine_command(id) {
            let info = self.inner.describe_command(id).ok().flatten();
            let gweike = info.as_ref().is_some_and(|i| i.gweike) || id.starts_with("gweike.") || id.starts_with("mcc.");
            self.seen.borrow_mut().reached.push((id.to_string(), mcp_catalog::hardware_class(id), gweike));
            return Err(CommandError::failed("monkey: a machine command is never run"));
        }
        if let Some(why) = held(id, &args, &self.dir) {
            self.seen.borrow_mut().held += 1;
            return Err(CommandError::failed(format!("monkey: not run ({why})")));
        }
        self.seen.borrow_mut().ran += 1;
        self.inner.execute(id, args)
    }
    fn checkpoint(&mut self) -> Result<Option<String>, CommandError> {
        self.inner.checkpoint()
    }
    fn restore(&mut self, checkpoint: &str) -> Result<(), CommandError> {
        self.inner.restore(checkpoint)
    }
    fn discard_checkpoint(&mut self, checkpoint: &str) {
        self.inner.discard_checkpoint(checkpoint);
    }
    fn inspect(&mut self, path: &str, depth: u32) -> Result<Value, CommandError> {
        self.inner.inspect(path, depth)
    }
    fn screenshot(&mut self, region: Option<Region>) -> Result<Vec<u8>, CommandError> {
        self.inner.screenshot(region)
    }
    fn measure(&mut self, request: Value) -> Result<Value, CommandError> {
        self.inner.measure(request)
    }
    fn preview_job(&mut self, request: Value) -> Result<Value, CommandError> {
        self.inner.preview_job(request)
    }
    fn pending(&mut self, id: &str) -> Result<PendingState, CommandError> {
        self.inner.pending(id)
    }
    fn running(&mut self) -> Result<Vec<RunningOp>, CommandError> {
        self.inner.running()
    }
    fn events(&mut self, since: u64) -> Result<EventPage, CommandError> {
        self.inner.events(since)
    }
    fn crash_reports(&mut self, limit: usize) -> Result<Value, CommandError> {
        self.inner.crash_reports(limit)
    }
    fn confirmation_scope(&mut self) -> String {
        self.inner.confirmation_scope()
    }
    fn request_confirmation(&mut self, _pending_id: &str, command: &str, _summary: &str) -> Result<(), CommandError> {
        // No dialog is opened and nobody ever clicks.
        self.seen.borrow_mut().asked.push(command.to_string());
        Ok(())
    }
}

// ----- Generated calls ---------------------------------------------------------------------------

/// A value no argument wants, also deep and long ones.
fn junk_json(rng: &mut Rng, depth: u32) -> Value {
    const KEYS: &[&str] = &["x", "y", "width", "height", "id", "shape", "layer", "path", "text", "params", "command", "steps", "value", "key"];
    match rng.pick(18) {
        0 => Value::Null,
        1 => json!(rng.chance(0.5)),
        2 => json!(1.7e308),
        3 => json!(-1.7e308),
        4 => json!(u64::MAX),
        5 => json!(i64::MIN),
        6 => json!("NaN"),
        7 => json!("-Infinity"),
        8 => json!("x".repeat(20_000)),
        9 => json!(""),
        10 => json!(5e-324),
        11 => json!("../../outside"),
        12 => (0..100).fold(json!(1), |v, _| json!([v])),
        13 => (0..100).fold(json!("deep"), |v, _| json!({ "a": v })),
        14 => json!(rng.f(-500.0, 500.0)),
        15 if depth < 3 => Value::Array((0..rng.pick(5)).map(|_| junk_json(rng, depth + 1)).collect()),
        16 | 17 if depth < 3 => {
            let mut o = Map::new();
            for _ in 0..rng.pick(5) {
                o.insert((*rng.of(KEYS)).to_string(), junk_json(rng, depth + 1));
            }
            Value::Object(o)
        }
        _ => json!("12"),
    }
}

/// Where the monkey stands: the server on the gate on the tripwire on the app.
struct McpMonkey {
    server: Server<DarkburnTools>,
    app: SharedApp,
    seen: Rc<RefCell<Seen>>,
    policy: PolicyHandle,
    start_policy: McpPolicy,
    prefs_mcp: String,
    rng: Rng,
    seed: u64,
    dir: PathBuf,
    all: Vec<commands::Info>,
    machine: Vec<commands::Info>,
    tools: Vec<Value>,
    trail: Vec<String>,
}

fn short(s: &str, max: usize) -> String {
    if s.chars().nth(max).is_some() { format!("{}... ({} bytes)", s.chars().take(max).collect::<String>(), s.len()) } else { s.to_string() }
}

impl McpMonkey {
    fn new(seed: u64, policy: McpPolicy) -> McpMonkey {
        let dir = temp_dir("mcp");
        let mut h = headless(&dir);
        seed_document(&mut h.app);
        let app: SharedApp = Rc::new(RefCell::new(h.app));
        let seen: Rc<RefCell<Seen>> = Rc::default();
        let handle = PolicyHandle::new(policy);
        let trip = Tripwire { inner: McpBackend::new(app.clone(), egui::Context::default(), true), seen: seen.clone(), dir: dir.clone() };
        let tools = DarkburnTools::new(Box::new(Gate::new(trip, handle.clone()))).with_poll(Duration::from_millis(1));
        let all = commands::list();
        let machine = all.iter().filter(|i| machine_command(&i.id)).cloned().collect();
        let prefs_mcp = format!("{:?}", app.borrow().prefs.mcp);
        McpMonkey {
            server: Server::new(INFO, tools),
            app,
            seen,
            policy: handle,
            start_policy: policy,
            prefs_mcp,
            rng: Rng::new(seed),
            seed,
            dir,
            all,
            machine,
            tools: tool_definitions(),
            trail: Vec::new(),
        }
    }

    fn fail(&self, what: &str) -> ! {
        let from = self.trail.len().saturating_sub(25);
        let mut msg = format!("MCP MONKEY seed {} line {}: {what}\nThe last lines:\n", self.seed, self.trail.len());
        for (i, l) in self.trail.iter().enumerate().skip(from) {
            msg.push_str(&format!("  {i}: {}\n", short(l, 600)));
        }
        panic!("{msg}");
    }

    /// A command id: any registered one, one that acts on a machine, or one that does not exist.
    fn command(&mut self) -> (String, Option<commands::Info>) {
        let info = match self.rng.pick(20) {
            0..=11 => self.rng.of(&self.all).clone(),
            12..=17 => self.rng.of(&self.machine).clone(),
            18 => {
                let id = *self.rng.of(&["settings.set", "layer.set_cut_settings", "job.generate"]);
                return (id.to_string(), None);
            }
            _ => {
                let base = self.rng.of(&self.machine).id.clone();
                let id = match self.rng.pick(8) {
                    0 => String::new(),
                    1 => format!(" {base}"),
                    2 => format!("{base} "),
                    3 => base.to_uppercase(),
                    4 => format!("{base}\u{0}"),
                    5 => "x".repeat(20_000),
                    6 => format!("{base}.{base}"),
                    _ => "no.such_command".to_string(),
                };
                return (id, None);
            }
        };
        (info.id.clone(), Some(info))
    }

    /// Arguments for a command: what its schema asks for (in and out of range), or junk.
    fn params(&mut self, id: &str, info: Option<&commands::Info>) -> Value {
        if id == "settings.set" {
            let key = match self.rng.pick(6) {
                0 => "mcp.allow_hardware",
                1 => "mcp.allow_gweike",
                2 => "mcp.require_confirmation_for_job_start",
                3 => "mcp",
                _ => self.rng.of(SETTING_KEYS),
            };
            let value = match self.rng.pick(4) {
                0 => json!(true),
                1 => json!(false),
                2 => json!({ "allow_hardware": true, "allow_gweike": true, "require_confirmation_for_job_start": false }),
                _ => junk_json(&mut self.rng, 0),
            };
            return json!({ "key": key, "value": value });
        }
        match info {
            Some(info) if self.rng.chance(0.6) => {
                let ids: Vec<u64> = self.app.borrow().editor.doc.shapes.iter().map(|s| s.id.0).collect();
                let dir = self.dir.clone();
                Gen { rng: &mut self.rng, wild: Wild::Full, ids, dir: &dir, saved: &[] }.args(info)
            }
            _ => junk_json(&mut self.rng, 0),
        }
    }

    fn wait_ms(&mut self) -> Value {
        match self.rng.pick(10) {
            0 => json!(u64::MAX),
            1 => junk_json(&mut self.rng, 2),
            _ => json!(self.rng.pick(40)),
        }
    }

    fn execute_call(&mut self) -> Value {
        let (id, info) = self.command();
        let mut a = Map::new();
        if self.rng.chance(0.97) {
            a.insert("command".into(), json!(id));
        }
        if self.rng.chance(0.9) {
            let p = self.params(&id, info.as_ref());
            a.insert("params".into(), p);
        }
        if self.rng.chance(0.25) {
            let t = match self.rng.pick(5) {
                0 => json!(""),
                1 => json!("made-up"),
                2 => json!("0".repeat(64)),
                3 => json!("f".repeat(5000)),
                _ => junk_json(&mut self.rng, 2),
            };
            a.insert("confirmation_token".into(), t);
        }
        if self.rng.chance(0.5) {
            let w = self.wait_ms();
            a.insert("wait_ms".into(), w);
        }
        json!({ "name": "execute", "arguments": a })
    }

    fn batch_call(&mut self) -> Value {
        let mut steps = Vec::new();
        for _ in 0..1 + self.rng.pick(6) {
            let (id, info) = self.command();
            let mut s = Map::new();
            s.insert("command".into(), json!(id));
            if self.rng.chance(0.8) {
                let p = self.params(&id, info.as_ref());
                s.insert("params".into(), p);
            }
            if self.rng.chance(0.15) {
                s.insert("confirmation_token".into(), json!("made-up"));
            }
            steps.push(if self.rng.chance(0.04) { junk_json(&mut self.rng, 1) } else { Value::Object(s) });
        }
        let mut a = Map::new();
        a.insert("steps".into(), if self.rng.chance(0.04) { junk_json(&mut self.rng, 0) } else { Value::Array(steps) });
        if self.rng.chance(0.5) {
            a.insert("all_or_nothing".into(), if self.rng.chance(0.1) { junk_json(&mut self.rng, 2) } else { json!(self.rng.chance(0.5)) });
        }
        json!({ "name": "batch", "arguments": a })
    }

    /// Any tool, with arguments made from its own description.
    fn tool_call(&mut self) -> Value {
        const WORDS: &[&str] = &[
            "",
            "design",
            "shapes",
            "layers",
            "settings",
            "ui",
            "machine",
            "mcp.allow_hardware",
            "page_width",
            "c1234",
            "b1",
            "shape",
            "time",
            "gcode",
            "svg",
            "machine.jog",
            "job.start",
            "shapes.0",
            "layers.999999",
            "a.b.c.d.e.f.g.h",
        ];
        let tool = self.rng.of(&self.tools).clone();
        let name = tool["name"].as_str().unwrap_or("").to_string();
        let mut a = Map::new();
        if let Some(props) = tool["inputSchema"]["properties"].as_object() {
            for (k, schema) in props {
                if !self.rng.chance(0.7) {
                    continue;
                }
                let v = if k == "wait_ms" {
                    self.wait_ms()
                } else if self.rng.chance(0.3) {
                    junk_json(&mut self.rng, 0)
                } else {
                    match schema["type"].as_str() {
                        Some("string") => json!(*self.rng.of(WORDS)),
                        Some("integer") | Some("number") => json!(*self.rng.of(&[0u64, 1, 2, 50, 100_000, u64::MAX])),
                        Some("boolean") => json!(self.rng.chance(0.5)),
                        Some("array") => json!([*self.rng.of(WORDS), self.rng.pick(40)]),
                        _ => junk_json(&mut self.rng, 1),
                    }
                };
                a.insert(k.clone(), v);
            }
        }
        if self.rng.chance(0.05) {
            a.insert("no_such_argument".into(), junk_json(&mut self.rng, 1));
        }
        let name = if self.rng.chance(0.03) { *self.rng.of(&["", "no_such_tool", "EXECUTE", "execute ", "machine.jog"]) } else { name.as_str() };
        json!({ "name": name, "arguments": if self.rng.chance(0.03) { junk_json(&mut self.rng, 0) } else { Value::Object(a) } })
    }

    /// One line for the server.
    fn line(&mut self) -> String {
        let call = match self.rng.pick(20) {
            0..=8 => self.execute_call(),
            9..=11 => self.batch_call(),
            12..=17 => self.tool_call(),
            18 => {
                // A request of another kind, or a broken one.
                let method = *self.rng.of(&["initialize", "tools/list", "resources/list", "resources/read", "ping", "no/such", "tools/call", ""]);
                let params = junk_json(&mut self.rng, 0);
                return match self.rng.pick(4) {
                    0 => json!({ "jsonrpc": "2.0", "method": method, "params": params }).to_string(),
                    1 => json!({ "jsonrpc": "1.0", "id": junk_json(&mut self.rng, 1), "method": method, "params": params }).to_string(),
                    2 => json!([{ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }, 7, null]).to_string(),
                    _ => json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }).to_string(),
                };
            }
            _ => {
                // Not JSON-RPC at all.
                return match self.rng.pick(9) {
                    0 => "{".to_string(),
                    1 => "[".repeat(100_000),
                    2 => r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"execute","arguments":{"command":"shape.add_rect","params":{"x":NaN,"y":1e999,"width":-0,"height":1e308}}}}"#.to_string(),
                    3 => "null".to_string(),
                    4 => "\u{feff}{}".to_string(),
                    5 => r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"execute","arguments":{"command":"machine.jog","command":"view.zoom_in","params":{"dx":1}}}}"#.to_string(),
                    6 => format!(r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"execute","arguments":{{"command":"text.add","params":{{"text":"{}"}}}}}}}}"#, "\\u0000".repeat(3000)),
                    7 => r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"execute","arguments":{"command":"machine.jog","params":{"dx":1,"dy":1},"confirmation_token":"made-up"}}}"#.to_string(),
                    _ => "   ".to_string(),
                };
            }
        };
        json!({ "jsonrpc": "2.0", "id": self.trail.len(), "method": "tools/call", "params": call }).to_string()
    }

    fn step(&mut self) {
        let line = self.line();
        self.trail.push(line.clone());
        let started = Instant::now();
        let reply = match catch_unwind(AssertUnwindSafe(|| self.server.handle_line(&line))) {
            Ok(r) => r,
            Err(e) => {
                let what = e.downcast_ref::<String>().cloned().or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default();
                self.fail(&format!("panicked: {what}"));
            }
        };
        if started.elapsed() > LIMIT {
            self.fail(&format!("took {:?}", started.elapsed()));
        }
        if let Some(r) = &reply
            && serde_json::from_str::<Value>(r).is_err()
        {
            self.fail(&format!("the reply is not JSON: {}", short(r, 300)));
        }
        self.check();
    }

    /// What must hold after every line.
    fn check(&mut self) {
        let p = self.policy.get();
        if p != self.start_policy {
            self.fail(&format!("the policy changed: {p:?}"));
        }
        let (real, dialogs, mcp, finite) = {
            let Ok(app) = self.app.try_borrow() else { self.fail("the app is still borrowed") };
            let finite = app.editor.doc.shapes.iter().all(|s| {
                let b = s.bbox();
                [b.x0, b.y0, b.x1, b.y1].iter().all(|v| v.is_finite()) && s.transform.as_coeffs().iter().all(|v| v.is_finite())
            });
            (app.machine.handle.is_real_machine(), ::rfd::script_take_log().len(), format!("{:?}", app.prefs.mcp), finite)
        };
        if real {
            self.fail("a real machine is connected");
        }
        if dialogs != 0 {
            self.fail("asked for a file dialog");
        }
        if mcp != self.prefs_mcp {
            self.fail(&format!("the agent settings changed: {mcp}"));
        }
        if !finite {
            self.fail("a shape has a coordinate that is not a number");
        }
        let seen = self.seen.borrow();
        for (id, class, gweike) in &seen.reached {
            let stop = *class == Some(HardwareClass::Stop);
            if !p.allow_hardware && !stop {
                self.fail(&format!("{id} got past the gate with machine control off"));
            }
            if *gweike && !p.allow_gweike && !stop {
                self.fail(&format!("{id} (Gweike) got past the gate with the Gweike option off"));
            }
            if *class == Some(HardwareClass::FireOrJob) && p.require_confirmation_for_job_start {
                self.fail(&format!("{id} got past the gate without the user's click"));
            }
        }
        if !p.allow_hardware && !seen.asked.is_empty() {
            self.fail(&format!("a confirmation dialog was asked for with machine control off: {:?}", seen.asked));
        }
    }
}

fn mcp_monkey(seed: u64, lines: usize, policy: McpPolicy) -> McpMonkey {
    let mut m = McpMonkey::new(seed, policy);
    for _ in 0..lines {
        m.step();
    }
    let _ = std::fs::remove_dir_all(&m.dir);
    m
}

/// The policy of a seed: machine control off for `hardware == false`, the rest drawn from the seed.
fn policy_for(seed: u64, hardware: bool) -> McpPolicy {
    McpPolicy {
        allow_hardware: hardware,
        allow_file_write: seed.is_multiple_of(2),
        // With machine control on, a job start always needs the click: that is the rule under test.
        require_confirmation_for_job_start: hardware || !seed.is_multiple_of(3),
        allow_gweike: !hardware && seed.is_multiple_of(5),
    }
}

/// With machine control off (the default), no generated call reaches a machine command.
#[test]
fn mcp_monkey_with_machine_control_off() {
    for seed in [1, 3] {
        // Twice the lines: most calls here are refused, and how many run moves with every command
        // added to the registry (the seed then draws other ones).
        let m = mcp_monkey(seed, MCP_LINES * 2, policy_for(seed, false));
        let seen = m.seen.borrow();
        assert!(seen.ran > 20, "seed {seed}: only {} commands ran", seen.ran);
        assert!(seen.reached.iter().all(|r| r.1 == Some(HardwareClass::Stop)), "{:?}", seen.reached);
    }
}

/// With machine control on, a job start or a shot still needs the user's click, and a Gweike
/// machine its own switch. (No machine command is run here either: see [`Tripwire`].)
#[test]
#[allow(clippy::single_element_loop)] // a list of seeds, with one entry for now
fn mcp_monkey_with_machine_control_on_never_starts_a_job_by_itself() {
    for seed in [11] {
        let m = mcp_monkey(seed, MCP_LINES, policy_for(seed, true));
        let seen = m.seen.borrow();
        assert!(!seen.reached.is_empty(), "seed {seed}: no machine command arrived, so nothing was tested");
        assert!(!seen.asked.is_empty(), "seed {seed}: no job start was tried");
    }
}

const MCP_LINES: usize = 300;

/// Wider runs, by hand: `MCP_MONKEY_SEEDS=1000..1100 MCP_MONKEY_LINES=2000 cargo test -p darkburn
/// mcp_monkey_explore -- --nocapture`; `MCP_MONKEY_HARDWARE=1` turns machine control on (still
/// behind the tripwire). Does nothing without `MCP_MONKEY_SEEDS`.
#[test]
fn mcp_monkey_explore() {
    let Ok(range) = std::env::var("MCP_MONKEY_SEEDS") else { return };
    let seeds: Vec<u64> = match range.split_once("..") {
        Some((a, b)) => (a.parse::<u64>().unwrap()..b.parse::<u64>().unwrap()).collect(),
        None => range.split(',').map(|s| s.trim().parse::<u64>().expect("MCP_MONKEY_SEEDS=from..to or a,b,c")).collect(),
    };
    let lines: usize = std::env::var("MCP_MONKEY_LINES").ok().and_then(|s| s.parse().ok()).unwrap_or(1000);
    let hardware = std::env::var("MCP_MONKEY_HARDWARE").is_ok();
    let mut failed = 0;
    for seed in seeds {
        let t = Instant::now();
        match catch_unwind(|| {
            let m = mcp_monkey(seed, lines, policy_for(seed, hardware));
            let seen = m.seen.borrow();
            (seen.ran, seen.held, seen.reached.len(), seen.asked.len())
        }) {
            Ok((ran, held, reached, asked)) => println!(
                "mcp seed {seed}: {lines} lines in {:?}; {ran} commands ran, {held} held back, {reached} machine commands arrived behind the gate, {asked} confirmations asked",
                t.elapsed()
            ),
            Err(_) => failed += 1,
        }
    }
    assert_eq!(failed, 0, "seeds failed");
}
