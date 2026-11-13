//! `darkburn_mcp::Backend` for the app: an AI agent runs any DarkBurn function through the
//! command registry.
//!
//! The backend lives on the UI thread and shares the app state (`Rc<RefCell<DarkBurn>>`) with the
//! window, which borrows it only inside `logic()` and `ui()`; the control server's `pump` runs in
//! `logic()` with no borrow outstanding. Every command goes through `api::execute` (the same code
//! the menus run) so there is no second implementation to drift. The safety gate wraps this
//! backend (see `mcp_ui.rs`): a denied command never reaches `execute` here.
//!
//! Long operations (job builds) run on worker threads and are reported through `pending` /
//! `running`; the screenshot of a window is taken by egui on a later frame and is reported the same
//! way. Headless (no window) the screenshot is an offscreen drawing of the design instead.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use darkburn_core::Editor;
use darkburn_mcp::{
    Backend, CommandError, CommandInfo, CommandPage, ErrorCode, Event, EventPage, ListQuery, PendingState, Region, RunningOp, now_ms,
    util::base64_encode,
};
use serde_json::{Value, json};

use crate::job_build::{self, BuildTask, BuiltJob};
use crate::mcp_catalog::{self, REDIRECTS};
use crate::mcp_inspect as insp;
use crate::shell::DarkBurn;

pub type SharedApp = Rc<RefCell<DarkBurn>>;

const MAX_CHECKPOINTS: usize = 8;
const MAX_EVENTS: usize = 2000;
const SHOT_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_SCOPES: usize = 16;
/// The largest number a command takes from an agent. Sizes and places are millimetres, so this
/// is a million kilometres; past it sums of two arguments stop being numbers at all (1e308 +
/// 1e308 is infinity) and a shape made from them has no size or place and cannot be saved.
const MAX_NUMBER: f64 = 1e12;

/// The first number in `v` that is beyond [`MAX_NUMBER`].
fn number_out_of_range(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64().filter(|x| !(x.abs() <= MAX_NUMBER)),
        Value::Array(items) => items.iter().find_map(number_out_of_range),
        Value::Object(fields) => fields.values().find_map(number_out_of_range),
        _ => None,
    }
}

enum Work {
    /// `job.generate`: the summary of the built job.
    Generate,
    /// `preview_job`: summary, layers, operation counts and optionally the G-code text.
    Preview { gcode: bool, max_lines: usize },
    /// `measure` with the job: the shape numbers plus the job's time.
    Measure { shapes: Value },
}

struct Build {
    id: String,
    what: String,
    task: BuildTask,
    work: Work,
    started: Instant,
}

struct Shot {
    id: String,
    region: Option<Region>,
    started: Instant,
    /// Filled by egui's callback on the frame that has the picture.
    slot: Arc<Mutex<Option<Arc<egui::ColorImage>>>>,
    result: Option<Result<Vec<u8>, String>>,
}

pub struct McpBackend {
    app: SharedApp,
    ctx: egui::Context,
    headless: bool,
    builds: Vec<Build>,
    /// `image.trace_files` runs: id, start, the run [IMG-21].
    traces: Vec<(String, Instant, crate::multi_trace::PendingTrace)>,
    /// `image.depth_map` runs: id, start, the run [IMG-25].
    depth_maps: Vec<(String, Instant, crate::model_import::PendingDepthMap)>,
    shots: Vec<Shot>,
    finished: BTreeMap<String, (String, Result<Value, CommandError>)>,
    checkpoints: Vec<(String, Editor, Option<PathBuf>)>,
    events: Vec<Event>,
    seq: u64,
    next_id: u64,
    dialogs: Vec<String>,
    /// What a job start depended on each time the user was asked to confirm one: a name, the
    /// design and the rest (see [`job_setup`]).
    scopes: Vec<(String, darkburn_core::Document, String)>,
}

/// Everything a started job depends on besides the design itself, as text: the selection, the
/// active device with all its settings, the G-code and motion settings, the origin and where
/// the job starts.
fn job_setup(app: &DarkBurn) -> String {
    let json = |v: Value| v.to_string();
    let device = app.machine.devices.active.and_then(|id| app.machine.devices.export_profile(id)).unwrap_or_default();
    format!(
        "{:?}|{:?}|{device}|{}|{}|{}|{:?}|{:?}|{:?}|{}|{}",
        app.machine.devices.active,
        app.editor.selected(),
        json(serde_json::to_value(&app.job.gcode).unwrap_or(Value::Null)),
        json(serde_json::to_value(app.job.additional).unwrap_or(Value::Null)),
        json(serde_json::to_value(&app.job.default_planner).unwrap_or(Value::Null)),
        app.job.user_origin,
        app.job.start_op,
        app.job.start_op_revision,
        app.prefs.opts.output_tolerance,
        app.prefs.opts.ignore_out_of_bounds,
    )
}

/// What a running `image.depth_map` call is doing, for the agent.
const DEPTH_MAP_WHAT: &str = "making a height picture from a 3D model";

fn err_of(m: String) -> CommandError {
    if m.contains("is not implemented in this build") {
        CommandError::new(ErrorCode::NotSupported, m)
    } else if m.contains("has nothing to act on") {
        CommandError::new(ErrorCode::Disabled, m)
    } else if m.starts_with("argument ") || m.contains(" must be ") {
        CommandError::invalid(m)
    } else {
        CommandError::failed(m)
    }
}

fn flag(a: &Value, k: &str) -> bool {
    a.get(k).and_then(Value::as_bool).unwrap_or(false)
}

/// Run command `id` the way an agent means it (the agent forms of the dialog commands, then the
/// registry). Pure app logic: no backend state.
pub fn run_command(app: &mut DarkBurn, ctx: &egui::Context, id: &str, args: &Value) -> Result<Value, String> {
    let args = if args.is_null() { json!({}) } else { args.clone() };
    let go = |app: &mut DarkBurn, id: &str, a: &Value| crate::api::execute(app, ctx, id, a);
    match id {
        "settings.set" => {
            let key = args.get("key").and_then(Value::as_str).ok_or("argument key must be text")?;
            let value = args.get("value").ok_or("argument value is missing")?;
            insp::set_setting(app, key, value)
        }
        "layer.set_cut_settings" => set_cut_settings(app, &args),
        "file.new" => {
            if app.editor.is_modified() && !flag(&args, "discard_changes") {
                return Err(
                    "the design has unsaved changes: save it first (file.save or file.save_as with a path) or pass discard_changes: true".into()
                );
            }
            app.editor.mark_saved();
            go(app, "file.new", &Value::Null)
        }
        "file.open" => {
            if app.editor.is_modified() && !flag(&args, "discard_changes") {
                return Err("the design has unsaved changes: save it first or pass discard_changes: true".into());
            }
            go(app, "project.open", &json!({ "path": args.get("path").cloned().unwrap_or(Value::Null) }))
        }
        "file.save" => match app.project_path.clone() {
            Some(p) => go(app, "project.save_as", &json!({ "path": p.display().to_string() })),
            None => Err("this design has no file yet: use file.save_as with a path".into()),
        },
        "file.import" => {
            let mut a = args.as_object().cloned().unwrap_or_default();
            if let Some(p) = a.remove("path") {
                a.insert("paths".into(), json!([p]));
            }
            go(app, "project.import", &Value::Object(a))
        }
        "edit.undo" | "edit.redo" => {
            let n = args.get("count").and_then(Value::as_u64).unwrap_or(1).clamp(1, 10_000);
            let mut done = 0u64;
            for _ in 0..n {
                match go(app, id, &Value::Null) {
                    Ok(_) => done += 1,
                    Err(e) if done == 0 => return Err(e),
                    Err(_) => break,
                }
            }
            Ok(json!({ if id == "edit.undo" { "undone" } else { "redone" }: done }))
        }
        _ => {
            if let Some(r) = REDIRECTS.iter().find(|r| r.id == id) {
                let mut a = args.as_object().cloned().unwrap_or_default();
                if let Some((k, v)) = r.fixed {
                    a.insert(
                        k.into(),
                        match v {
                            "true" => json!(true),
                            "false" => json!(false),
                            other => json!(other),
                        },
                    );
                }
                return go(app, r.target, &Value::Object(a));
            }
            go(app, id, &args)
        }
    }
}

fn mode_variant(name: &str) -> Option<&'static str> {
    Some(match name {
        "line" | "Line" => "Line",
        "fill" | "Fill" => "Fill",
        "fill_line" | "FillLine" => "FillLine",
        "offset_fill" | "OffsetFill" => "OffsetFill",
        "image" | "Image" => "Image",
        _ => return None,
    })
}

/// `layer.set_cut_settings`: merge named fields into one cut setting, validated by serde.
fn set_cut_settings(app: &mut DarkBurn, a: &Value) -> Result<Value, String> {
    let layer =
        a.get("layer").and_then(Value::as_u64).filter(|l| *l < darkburn_core::LAYER_COUNT as u64).ok_or("argument layer must be 0 to 31")? as usize;
    let sub = a.get("sublayer").and_then(Value::as_u64).unwrap_or(0) as usize;
    let values = a.get("settings").and_then(Value::as_object).ok_or("argument settings must be an object")?;
    let current = app.editor.doc.layer(layer).settings.get(sub).ok_or_else(|| format!("layer {layer} has no sub-layer {sub}"))?;
    let mut cur = serde_json::to_value(current).map_err(|e| e.to_string())?;
    let mut new_mode = None;
    for (k, v) in values {
        let field = cur.get_mut(k).ok_or_else(|| format!("unknown cut setting `{k}`: inspect_design layers.{layer} lists them"))?;
        let v = if k == "mode" {
            let name = v.as_str().and_then(mode_variant).ok_or("mode is line, fill, fill_line, offset_fill or image")?;
            new_mode = serde_json::from_value::<darkburn_core::Mode>(json!(name)).ok();
            json!(name)
        } else {
            v.clone()
        };
        if std::mem::discriminant(&*field) != std::mem::discriminant(&v) && !(field.is_number() && v.is_number()) {
            return Err(format!("cut setting `{k}` needs a different kind of value"));
        }
        *field = v;
    }
    if let Some(m) = new_mode
        && sub == 0
        && !crate::cuts_panel::modes_for(&app.editor.doc, layer).contains(&m)
    {
        return Err(format!("layer {layer} cannot use {}", m.label()));
    }
    let mut new: darkburn_core::layer::CutSetting = serde_json::from_value(cur).map_err(|e| format!("cut settings: {e}"))?;
    new.sanitise();
    let changed = app.editor.edit("Cut settings", |d, _| {
        let before = d.layer(layer).clone();
        d.layer_mut(layer).settings[sub] = new;
        *d.layer(layer) != before
    });
    Ok(json!({ "changed": changed }))
}

impl McpBackend {
    pub fn new(app: SharedApp, ctx: egui::Context, headless: bool) -> Self {
        let dialogs = insp::open_dialogs(&app.borrow());
        Self {
            app,
            ctx,
            headless,
            builds: Vec::new(),
            traces: Vec::new(),
            depth_maps: Vec::new(),
            shots: Vec::new(),
            finished: BTreeMap::new(),
            checkpoints: Vec::new(),
            events: Vec::new(),
            seq: 0,
            next_id: 1,
            dialogs,
            scopes: Vec::new(),
        }
    }

    #[cfg(test)]
    pub fn app_for_test(&self) -> SharedApp {
        self.app.clone()
    }

    /// A build or screenshot is in progress: the window should keep drawing frames.
    pub fn busy(&self) -> bool {
        !self.builds.is_empty() || !self.traces.is_empty() || !self.depth_maps.is_empty() || self.shots.iter().any(|s| s.result.is_none())
    }

    fn id(&mut self, prefix: &str) -> String {
        self.next_id += 1;
        format!("{prefix}{}", self.next_id)
    }

    /// Record an app event (dialogs, errors, warnings) for `events`.
    pub fn note(&mut self, kind: &str, command: &str, outcome: &str, message: &str, data: Value) {
        self.seq += 1;
        if self.events.len() >= MAX_EVENTS {
            self.events.remove(0);
        }
        self.events.push(Event {
            seq: self.seq,
            ts_ms: now_ms(),
            kind: kind.into(),
            tool: String::new(),
            command: command.into(),
            args_hash: String::new(),
            outcome: outcome.into(),
            message: message.chars().take(300).collect(),
            data,
        });
    }

    fn note_dialogs(&mut self) {
        let Ok(app) = self.app.try_borrow() else { return };
        let now = insp::open_dialogs(&app);
        drop(app);
        let opened: Vec<String> = now.iter().filter(|d| !self.dialogs.contains(d)).cloned().collect();
        let closed: Vec<String> = self.dialogs.iter().filter(|d| !now.contains(d)).cloned().collect();
        for d in opened {
            self.note("dialog", "", "opened", &d, Value::Null);
        }
        for d in closed {
            self.note("dialog", "", "closed", &d, Value::Null);
        }
        self.dialogs = now;
    }

    /// Once per frame, before the control channel is served: collect pictures the window took,
    /// finish job builds and note dialogs the user opened or closed.
    pub fn poll(&mut self) {
        let ppp = self.ctx.pixels_per_point();
        for shot in self.shots.iter_mut().filter(|s| s.result.is_none()) {
            let image = shot.slot.lock().ok().and_then(|mut s| s.take());
            if let Some(image) = image {
                shot.result = Some(png_of(&image, shot.region, ppp));
            }
        }
        self.poll_builds();
        self.note_dialogs();
    }

    fn poll_builds(&mut self) {
        self.poll_traces();
        let mut i = 0;
        while i < self.builds.len() {
            let ready = self.builds[i].task.poll().is_some();
            if !ready {
                i += 1;
                continue;
            }
            let b = self.builds.remove(i);
            let id = b.id.clone();
            let result = match b.task.wait() {
                Ok(job) => Ok(self.report(&job, b.work)),
                Err(e) => Err(CommandError::failed(e.to_string())),
            };
            self.finished.insert(id, (b.what, result));
        }
    }

    /// Puts finished `image.trace_files` and `image.depth_map` runs into the document.
    fn poll_traces(&mut self) {
        while let Some(i) = self.traces.iter().position(|(.., t)| t.finished()) {
            let Ok(mut app) = self.app.try_borrow_mut() else { return };
            let (id, _, t) = self.traces.remove(i);
            let result = t.finish(&mut app).map_err(err_of);
            drop(app);
            self.finished.insert(id, ("tracing picture files".into(), result));
        }
        while let Some(i) = self.depth_maps.iter().position(|(.., d)| d.finished()) {
            let Ok(mut app) = self.app.try_borrow_mut() else { return };
            let (id, _, d) = self.depth_maps.remove(i);
            let result = d.finish(&mut app).map_err(err_of);
            drop(app);
            self.finished.insert(id, (DEPTH_MAP_WHAT.into(), result));
        }
    }

    fn report(&mut self, job: &std::sync::Arc<BuiltJob>, work: Work) -> Value {
        let Ok(mut app) = self.app.try_borrow_mut() else { return json!({"error": "the app was busy"}) };
        app.job.last_job = Some(job.clone());
        let mut v = crate::job_cmds::summary(&app, job);
        match work {
            Work::Generate => {}
            Work::Measure { shapes } => {
                let mut s = shapes;
                s["job"] = v;
                return s;
            }
            Work::Preview { gcode, max_lines } => {
                v["layers_detail"] = Value::Array(
                    job.toolpath
                        .layers
                        .iter()
                        .map(|l| {
                            json!({"layer": l.layer_index, "sublayer": l.sublayer, "name": l.name, "mode": format!("{:?}", l.mode).to_lowercase(),
                               "speed": l.speed, "max_power": l.max_power, "min_power": l.min_power, "passes": l.passes})
                        })
                        .collect(),
                );
                let mut counts: BTreeMap<String, usize> = BTreeMap::new();
                for op in &job.toolpath.ops {
                    let name = match serde_json::to_value(op) {
                        Ok(Value::Object(m)) => m.keys().next().cloned().unwrap_or_default(),
                        Ok(Value::String(s)) => s,
                        _ => "other".into(),
                    };
                    *counts.entry(name.to_lowercase()).or_default() += 1;
                }
                v["op_counts"] = json!(counts);
                let sim = job.simulate(&app.job.additional);
                v["time_per_layer"] =
                    json!(sim.per_layer.iter().map(|l| json!({"layer": l.layer, "pass": l.pass, "seconds": l.time})).collect::<Vec<_>>());
                v["sent_to_machine"] = json!(false);
                if gcode {
                    // What the selected device would be sent (its dialect, its scripts), as Save
                    // G-code writes it.
                    let text = crate::gcode_machines::job_text(app.machine.active(), job, None, &app.job.gcode);
                    let total = text.lines().count();
                    v["gcode_lines"] = json!(total);
                    if total > max_lines {
                        v["gcode_truncated"] = json!(true);
                        v["gcode"] = json!(text.lines().take(max_lines).collect::<Vec<_>>().join("\n"));
                    } else {
                        v["gcode"] = json!(text);
                    }
                }
            }
        }
        v
    }

    fn start_build(&mut self, what: &str, work: Work, only_layers: Option<Vec<usize>>) -> Result<Value, CommandError> {
        let input = {
            let app = self.app.try_borrow().map_err(|_| CommandError::failed("the app is busy: try again"))?;
            let mut input = crate::job_cmds::build_input(&app);
            if let Some(layers) = &only_layers {
                let drop: Vec<_> = input.doc.iter().filter(|s| !layers.contains(&s.layer)).map(|s| s.id).collect();
                input.doc.remove(&drop);
                input.selection.retain(|id| input.doc.get(*id).is_some());
            }
            input
        };
        let id = self.id("b");
        self.builds.push(Build { id: id.clone(), what: what.into(), task: job_build::spawn_build(input), work, started: Instant::now() });
        Err(CommandError::still_running(id, what))
    }
}

pub(crate) fn png_of(image: &egui::ColorImage, region: Option<Region>, ppp: f32) -> Result<Vec<u8>, String> {
    let [w, h] = image.size;
    if w == 0 || h == 0 {
        return Err("the window has no picture right now (is it minimized?)".into());
    }
    let (x0, y0, cw, ch) = match region {
        Some(r) if r.width > 0.0 && r.height > 0.0 => {
            let x0 = ((r.x * ppp).max(0.0) as usize).min(w.saturating_sub(1));
            let y0 = ((r.y * ppp).max(0.0) as usize).min(h.saturating_sub(1));
            let cw = ((r.width * ppp) as usize).clamp(1, w - x0);
            let ch = ((r.height * ppp) as usize).clamp(1, h - y0);
            (x0, y0, cw, ch)
        }
        _ => (0, 0, w, h),
    };
    let mut rgba = Vec::with_capacity(cw * ch * 4);
    for y in y0..y0 + ch {
        for x in x0..x0 + cw {
            rgba.extend_from_slice(&image.pixels[y * w + x].to_array());
        }
    }
    crate::import::rgba_to_png(cw as u32, ch as u32, rgba).ok_or_else(|| "could not encode the screenshot".to_string())
}

impl Backend for McpBackend {
    fn describe(&self) -> String {
        format!("DarkBurn {} {}", env!("CARGO_PKG_VERSION"), if self.headless { "headless (no window)" } else { "window" })
    }

    fn list_commands(&mut self, query: &ListQuery) -> Result<CommandPage, CommandError> {
        let app = self.app.try_borrow().map_err(|_| CommandError::failed("the app is busy: try again"))?;
        let g = mcp_catalog::gweike_active(&app);
        let mut all: Vec<CommandInfo> = crate::commands::list().iter().map(|i| mcp_catalog::command_info(&app, i, g)).collect();
        all.extend(mcp_catalog::extra_commands());
        Ok(query.apply(all))
    }

    fn describe_command(&mut self, id: &str) -> Result<Option<CommandInfo>, CommandError> {
        if let Some(c) = mcp_catalog::extra_commands().into_iter().find(|c| c.id == id) {
            return Ok(Some(c));
        }
        let app = self.app.try_borrow().map_err(|_| CommandError::failed("the app is busy: try again"))?;
        let g = mcp_catalog::gweike_active(&app);
        Ok(crate::commands::describe(id).map(|i| mcp_catalog::command_info(&app, &i, g)))
    }

    fn execute(&mut self, id: &str, args: Value) -> Result<Value, CommandError> {
        let r = self.execute_inner(id, args);
        match &r {
            Err(e) if e.code != ErrorCode::StillRunning => {
                let code = format!("{:?}", e.code).to_lowercase();
                self.note("error", id, &code, &e.message, Value::Null);
            }
            _ => {}
        }
        self.note_dialogs();
        r
    }

    fn checkpoint(&mut self) -> Result<Option<String>, CommandError> {
        let app = self.app.try_borrow().map_err(|_| CommandError::failed("the app is busy: try again"))?;
        let id = format!("cp{}", self.next_id);
        self.next_id += 1;
        self.checkpoints.push((id.clone(), app.editor.clone(), app.project_path.clone()));
        if self.checkpoints.len() > MAX_CHECKPOINTS {
            self.checkpoints.remove(0);
        }
        Ok(Some(id))
    }

    fn restore(&mut self, checkpoint: &str) -> Result<(), CommandError> {
        let Some(i) = self.checkpoints.iter().position(|(id, ..)| id == checkpoint) else {
            return Err(CommandError::failed("that checkpoint is gone"));
        };
        let (_, saved, path) = self.checkpoints.remove(i);
        let mut app = self.app.try_borrow_mut().map_err(|_| CommandError::failed("the app is busy: try again"))?;
        // Put the saved editor back with a revision newer than anything the renderers have seen.
        let current = app.editor.revision();
        let mut editor = saved;
        while editor.revision() <= current {
            editor.touch();
        }
        app.editor = editor;
        app.project_path = path;
        Ok(())
    }

    fn discard_checkpoint(&mut self, checkpoint: &str) {
        self.checkpoints.retain(|(id, ..)| id != checkpoint);
    }

    fn inspect(&mut self, path: &str, depth: u32) -> Result<Value, CommandError> {
        let app = self.app.try_borrow().map_err(|_| CommandError::failed("the app is busy: try again"))?;
        let parts: Vec<&str> = path.split('.').filter(|p| !p.is_empty()).collect();
        let (root, rest) = parts.split_first().map_or(("design", &[][..]), |(r, rest)| (*r, rest));
        let v = match root {
            "design" => insp::design_json(&app, rest)?,
            "settings" => insp::settings_json(&app, rest)?,
            "ui" => descend(insp::ui_json(&app, (!self.headless).then_some(&self.ctx)), rest)?,
            "machine" | "devices" => insp::machine_json(&app, rest)?,
            "undo" => descend(insp::design_json(&app, &[])?["undo"].clone(), rest)?,
            "job" => descend(insp::job_json(&app), rest)?,
            "library" | "art" => descend(insp::library_json(&app), rest)?,
            "vision" | "camera" | "rotary" | "print_and_cut" => {
                let which = if root == "vision" { rest.first().copied() } else { Some(root) };
                insp::vision_json(&app, which)?
            }
            other => {
                return Err(CommandError::failed(format!(
                    "unknown inspect root `{other}`: use design, settings, ui, machine, undo, job, library, camera, rotary, print_and_cut"
                )));
            }
        };
        Ok(insp::limit_depth(v, depth))
    }

    fn screenshot(&mut self, region: Option<Region>) -> Result<Vec<u8>, CommandError> {
        if self.headless {
            let app = self.app.try_borrow().map_err(|_| CommandError::failed("the app is busy: try again"))?;
            return insp::design_png(&app.editor.doc, (f64::from(app.prefs.page_width), f64::from(app.prefs.page_height)), region, 1200);
        }
        let id = self.id("shot");
        let slot: Arc<Mutex<Option<Arc<egui::ColorImage>>>> = Arc::default();
        let (put, wake) = (slot.clone(), self.ctx.clone());
        self.ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::ScreenshotCallback::new(move |image| {
            if let Ok(mut s) = put.lock() {
                *s = Some(image);
            }
            wake.request_repaint();
        })));
        self.ctx.request_repaint();
        self.shots.push(Shot { id: id.clone(), region, started: Instant::now(), slot, result: None });
        Err(CommandError::still_running(id, "taking a screenshot of the window"))
    }

    fn measure(&mut self, request: Value) -> Result<Value, CommandError> {
        let target = request.get("target").cloned().unwrap_or(Value::Null);
        let what: Vec<&str> = request.get("what").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
        let job = what.contains(&"time");
        let shapes = {
            let app = self.app.try_borrow().map_err(|_| CommandError::failed("the app is busy: try again"))?;
            insp::measure_shapes(&app, &target)?
        };
        if job { self.start_build("measuring the job time", Work::Measure { shapes }, None) } else { Ok(shapes) }
    }

    fn preview_job(&mut self, request: Value) -> Result<Value, CommandError> {
        let gcode = request.get("format").and_then(Value::as_str) == Some("gcode");
        let max_lines = request.get("max_lines").and_then(Value::as_u64).map_or(2000, |n| n.clamp(1, 1_000_000) as usize);
        let layers = match request.get("layers").and_then(Value::as_array) {
            None => None,
            Some(list) => {
                let app = self.app.try_borrow().map_err(|_| CommandError::failed("the app is busy: try again"))?;
                let mut out = Vec::new();
                for l in list {
                    let key = l.as_u64().map(|n| n.to_string()).or_else(|| l.as_str().map(str::to_string)).unwrap_or_default();
                    out.push(insp::find_layer(&app.editor.doc, &key).ok_or_else(|| CommandError::invalid(format!("no layer `{key}`")))?);
                }
                Some(out)
            }
        };
        self.start_build("building the job preview (nothing is sent to a machine)", Work::Preview { gcode, max_lines }, layers)
    }

    fn pending(&mut self, id: &str) -> Result<PendingState, CommandError> {
        self.poll_builds();
        if let Some(i) = self.shots.iter().position(|s| s.id == id) {
            if self.shots[i].result.is_none() && self.shots[i].started.elapsed() > SHOT_TIMEOUT {
                self.shots[i].result = Some(Err("the window did not deliver a picture in time (is it minimized or hidden?)".into()));
            }
            return Ok(match self.shots[i].result.take() {
                None => PendingState::Running { what: "taking a screenshot".into(), running_s: self.shots[i].started.elapsed().as_secs() },
                Some(r) => {
                    self.shots.remove(i);
                    PendingState::Done {
                        what: "screenshot".into(),
                        result: r
                            .map(|png| json!({ "png_base64": base64_encode(&png), "bytes": png.len() }))
                            .map_err(|m| CommandError::new(ErrorCode::Timeout, m)),
                    }
                }
            });
        }
        if let Some(b) = self.builds.iter().find(|b| b.id == id) {
            return Ok(PendingState::Running { what: b.what.clone(), running_s: b.started.elapsed().as_secs() });
        }
        if let Some((_, started, t)) = self.traces.iter().find(|(t, ..)| t == id) {
            let (done, total) = t.progress();
            return Ok(PendingState::Running { what: format!("tracing picture files ({done} of {total})"), running_s: started.elapsed().as_secs() });
        }
        if let Some((_, started, _)) = self.depth_maps.iter().find(|(d, ..)| d == id) {
            return Ok(PendingState::Running { what: DEPTH_MAP_WHAT.into(), running_s: started.elapsed().as_secs() });
        }
        Ok(match self.finished.remove(id) {
            Some((what, result)) => PendingState::Done { what, result },
            None => PendingState::Unknown,
        })
    }

    fn running(&mut self) -> Result<Vec<RunningOp>, CommandError> {
        self.poll_builds();
        let mut v: Vec<RunningOp> =
            self.builds.iter().map(|b| RunningOp { id: b.id.clone(), what: b.what.clone(), running_s: b.started.elapsed().as_secs() }).collect();
        v.extend(self.traces.iter().map(|(id, started, _)| RunningOp {
            id: id.clone(),
            what: "tracing picture files".into(),
            running_s: started.elapsed().as_secs(),
        }));
        v.extend(self.depth_maps.iter().map(|(id, started, _)| RunningOp {
            id: id.clone(),
            what: DEPTH_MAP_WHAT.into(),
            running_s: started.elapsed().as_secs(),
        }));
        v.extend(self.shots.iter().filter(|s| s.result.is_none()).map(|s| RunningOp {
            id: s.id.clone(),
            what: "taking a screenshot".into(),
            running_s: s.started.elapsed().as_secs(),
        }));
        Ok(v)
    }

    fn events(&mut self, since: u64) -> Result<EventPage, CommandError> {
        self.note_dialogs();
        Ok(EventPage { events: self.events.iter().filter(|e| e.seq > since).cloned().collect(), next: self.seq })
    }

    fn crash_reports(&mut self, limit: usize) -> Result<Value, CommandError> {
        let app = self.app.try_borrow().map_err(|_| CommandError::failed("the app is busy: try again"))?;
        Ok(crate::mcp_ui::crash_reports(app.settings_dir.as_deref(), limit))
    }

    fn confirmation_scope(&mut self) -> String {
        let Ok(app) = self.app.try_borrow() else {
            // The state cannot be read: a name nothing else has, so no confirmation covers it.
            return self.id("busy");
        };
        let setup = job_setup(&app);
        if let Some((name, ..)) = self.scopes.iter().find(|(_, doc, s)| *s == setup && *doc == app.editor.doc) {
            return name.clone();
        }
        let doc = app.editor.doc.clone();
        drop(app);
        let name = self.id("state");
        self.scopes.push((name.clone(), doc, setup));
        if self.scopes.len() > MAX_SCOPES {
            self.scopes.remove(0);
        }
        name
    }

    fn request_confirmation(&mut self, pending_id: &str, command: &str, summary: &str) -> Result<(), CommandError> {
        // The dialog is drawn by the window from the gate's open confirmations; here it is only
        // noted and the window woken.
        self.note("dialog", command, "confirmation_opened", summary, json!({ "pending_id": pending_id }));
        self.ctx.request_repaint();
        Ok(())
    }
}

/// Walk `rest` (object keys, array indexes) into `v`.
fn descend(mut v: Value, rest: &[&str]) -> Result<Value, CommandError> {
    for part in rest {
        v = match &v {
            Value::Object(m) => m.get(*part).cloned(),
            Value::Array(a) => part.parse::<usize>().ok().and_then(|i| a.get(i).cloned()),
            _ => None,
        }
        .ok_or_else(|| CommandError::failed(format!("nothing at `{part}`: inspect the parent path to see what is there")))?;
    }
    Ok(v)
}

impl McpBackend {
    fn execute_inner(&mut self, id: &str, args: Value) -> Result<Value, CommandError> {
        if let Some(why) = mcp_catalog::refused(id) {
            return Err(CommandError::new(ErrorCode::NotSupported, format!("`{id}`: {why}")));
        }
        if let Some(n) = number_out_of_range(&args) {
            return Err(CommandError::invalid(format!("`{id}`: {n:e} is out of range: numbers go from -{MAX_NUMBER:e} to {MAX_NUMBER:e}")));
        }
        if id == "job.generate" {
            return self.start_build("generating the job", Work::Generate, None);
        }
        if matches!(id, "image.trace_files" | "tools.trace_files") {
            // Off the UI thread: a long list of large pictures would freeze the window.
            let app = self.app.try_borrow().map_err(|_| CommandError::failed("the app is busy: try again"))?;
            let run = crate::multi_trace::PendingTrace::start(&app, &args).map_err(err_of)?;
            drop(app);
            let pid = self.id("t");
            self.traces.push((pid.clone(), Instant::now(), run));
            return Err(CommandError::still_running(pid, "tracing picture files"));
        }
        if id == "image.depth_map" {
            // Off the UI thread too: a large model at a high resolution takes a while.
            let app = self.app.try_borrow().map_err(|_| CommandError::failed("the app is busy: try again"))?;
            let run = crate::model_import::PendingDepthMap::start(&app, &args).map_err(err_of)?;
            drop(app);
            let pid = self.id("d");
            self.depth_maps.push((pid.clone(), Instant::now(), run));
            return Err(CommandError::still_running(pid, DEPTH_MAP_WHAT));
        }
        let app_rc = self.app.clone();
        let mut app = app_rc.try_borrow_mut().map_err(|_| CommandError::failed("the app is busy: try again"))?;
        if self.headless
            && mcp_catalog::acts_on_machine(id)
            && mcp_catalog::hardware_class(id) != Some(darkburn_mcp::HardwareClass::Stop)
            && !simulator_only(&app, &args)
        {
            return Err(CommandError::denied(format!(
                "`{id}`: without a window DarkBurn only drives simulators; the active device is not one. Start DarkBurn with its window for real machines."
            )));
        }
        // The AI-agent settings are the user's: whatever a command did to them is undone.
        let policy = app.prefs.mcp.clone();
        let r = run_command(&mut app, &self.ctx, id, &args);
        app.prefs.mcp = policy;
        r.map_err(err_of)
    }
}

/// Headless mode only drives simulated machines: the active device (and the one named in `args`)
/// must be a simulator.
fn simulator_only(app: &DarkBurn, args: &Value) -> bool {
    let is_sim = |kind: String| kind.to_lowercase().contains("simulator");
    if !is_sim(format!("{:?}", app.machine.active().kind)) {
        return false;
    }
    match args.get("device").or_else(|| args.get("id")).and_then(Value::as_u64) {
        Some(d) => app.machine.devices.devices.iter().find(|x| x.id == d).is_some_and(|x| is_sim(format!("{:?}", x.kind))),
        None => true,
    }
}
