//! `MockBackend`: a tiny in-memory command registry so the whole stack (tools, gate, control
//! channel, coverage helpers) can be exercised without the app. It is also what
//! `darkburn-mcp --headless` serves until the app crate provides its own headless backend.
//!
//! It models a design of rectangles with layers, an undo stack, settings, a simulated machine
//! and one long-running command. No port, USB or network is ever opened.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::backend::Backend;
use crate::types::*;
use crate::util::base64_decode;

#[derive(Clone, Debug, PartialEq)]
struct Shape {
    id: String,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    layer: usize,
}

#[derive(Clone, Debug, Default)]
struct Doc {
    shapes: Vec<Shape>,
    next_id: u32,
    layers: Vec<String>,
    path: Option<String>,
}

/// 1x1 PNG.
const PNG_1X1: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

#[derive(Default)]
pub struct MockBackend {
    doc: Doc,
    undo: Vec<Doc>,
    redo: Vec<Doc>,
    settings: BTreeMap<String, Value>,
    events: Vec<Event>,
    seq: u64,
    checkpoints: BTreeMap<String, (Doc, Vec<Doc>, Vec<Doc>)>,
    next_cp: u32,
    running: BTreeMap<String, (String, u32)>,
    next_op: u32,
    /// Every command id that reached `execute`, in order (tests read this).
    pub executed: Vec<String>,
    /// Confirmation dialogs the gate asked for: (pending id, command, summary).
    pub confirmations_requested: Vec<(String, String, String)>,
    /// Make `checkpoint` return `None` (a backend that cannot roll back).
    pub no_rollback: bool,
}

impl MockBackend {
    pub fn new() -> Self {
        let mut m = Self::default();
        m.doc.layers = vec!["Cut".into(), "Engrave".into()];
        m.settings.insert("units".into(), json!("mm"));
        m.settings.insert("grid.spacing".into(), json!(10.0));
        m.settings.insert("mcp.allow_hardware".into(), json!(false));
        m
    }

    /// Ids of every action the mock's GUI would render (for the coverage test).
    pub fn gui_actions(&self) -> Vec<String> {
        self.catalog().into_iter().map(|c| c.id).collect()
    }

    fn event(&mut self, kind: &str, command: &str, outcome: &str, message: &str) {
        self.seq += 1;
        self.events.push(Event {
            seq: self.seq,
            ts_ms: now_ms(),
            kind: kind.into(),
            tool: String::new(),
            command: command.into(),
            args_hash: String::new(),
            outcome: outcome.into(),
            message: message.into(),
            data: Value::Null,
        });
    }

    fn catalog(&self) -> Vec<CommandInfo> {
        let obj = |props: Value, req: &[&str]| json!({"type": "object", "properties": props, "required": req, "additionalProperties": false});
        let path = json!({"path": {"type": "string", "description": "File path"}});
        let mut v = Vec::new();
        let mut add = |mut c: CommandInfo, f: &dyn Fn(&mut CommandInfo)| {
            f(&mut c);
            v.push(c);
        };
        add(CommandInfo::new("file.new", "New", "File"), &|c| {
            c.menu_path = vec!["File".into(), "New".into()];
            c.shortcut = Some("Ctrl+N".into());
            c.checklist_ids = vec!["FILE-01".into()];
        });
        add(CommandInfo::new("file.open", "Open", "File"), &|c| {
            c.params_schema = obj(path.clone(), &["path"]);
            c.checklist_ids = vec!["FILE-04".into()];
        });
        add(CommandInfo::new("file.save", "Save", "File"), &|c| {
            c.writes_files = true;
            c.checklist_ids = vec!["FILE-07".into()];
        });
        add(CommandInfo::new("file.save_as", "Save As", "File"), &|c| {
            c.writes_files = true;
            c.params_schema = obj(path.clone(), &["path"]);
            c.checklist_ids = vec!["FILE-08".into()];
        });
        add(CommandInfo::new("file.import", "Import", "File"), &|c| c.params_schema = obj(path.clone(), &["path"]));
        add(CommandInfo::new("file.export", "Export", "File"), &|c| {
            c.writes_files = true;
            c.params_schema = obj(json!({"path": {"type": "string"}, "format": {"type": "string", "enum": ["svg", "dxf", "gcode"]}}), &["path"]);
        });
        add(CommandInfo::new("edit.undo", "Undo", "Edit"), &|c| c.params_schema = obj(json!({"count": {"type": "integer", "minimum": 1}}), &[]));
        add(CommandInfo::new("edit.redo", "Redo", "Edit"), &|c| c.params_schema = obj(json!({"count": {"type": "integer", "minimum": 1}}), &[]));
        add(CommandInfo::new("settings.set", "Change setting", "Settings"), &|c| {
            c.params_schema = obj(json!({"key": {"type": "string"}, "value": {}}), &["key", "value"])
        });
        add(CommandInfo::new("shape.add_rect", "Add rectangle", "Create"), &|c| {
            c.params_schema = obj(
                json!({"x": {"type": "number"}, "y": {"type": "number"}, "width": {"type": "number", "exclusiveMinimum": 0}, "height": {"type": "number", "exclusiveMinimum": 0}, "layer": {"type": ["integer", "string"]}}),
                &["x", "y", "width", "height"],
            );
            c.result_schema = obj(json!({"id": {"type": "string"}}), &["id"]);
            c.doc = "Adds a rectangle, millimetres, Y up. Returns its stable id.".into();
        });
        add(CommandInfo::new("shape.move", "Move shape", "Edit"), &|c| {
            c.params_schema = obj(json!({"id": {"type": "string"}, "dx": {"type": "number"}, "dy": {"type": "number"}}), &["id", "dx", "dy"])
        });
        add(CommandInfo::new("shape.delete", "Delete shape", "Edit"), &|c| c.params_schema = obj(json!({"id": {"type": "string"}}), &["id"]));
        add(CommandInfo::new("job.long_task", "Trace image (long)", "Tools"), &|c| c.doc = "Starts a long operation: returns a pending id.".into());
        add(CommandInfo::new("machine.connect", "Connect", "Laser"), &|c| {
            c.hardware = true;
            c.hardware_class = Some(HardwareClass::Connect);
        });
        add(CommandInfo::new("machine.jog", "Jog", "Laser"), &|c| {
            c.hardware = true;
            c.hardware_class = Some(HardwareClass::Motion);
            c.params_schema = obj(json!({"dx": {"type": "number"}, "dy": {"type": "number"}}), &["dx", "dy"]);
        });
        add(CommandInfo::new("job.start", "Start job", "Laser"), &|c| {
            c.hardware = true;
            c.hardware_class = Some(HardwareClass::FireOrJob);
            c.checklist_ids = vec!["LASER-01".into()];
        });
        add(CommandInfo::new("laser.test_fire", "Test fire", "Laser"), &|c| {
            c.hardware = true;
            c.hardware_class = Some(HardwareClass::FireOrJob);
            c.params_schema = obj(json!({"power": {"type": "number", "minimum": 0, "maximum": 100}}), &["power"]);
        });
        add(CommandInfo::new("machine.emergency_stop", "Emergency stop", "Laser"), &|c| {
            c.hardware = true;
            c.hardware_class = Some(HardwareClass::Stop);
        });
        add(CommandInfo::new("machine.pause", "Pause", "Laser"), &|c| {
            c.hardware = true;
            c.hardware_class = Some(HardwareClass::Stop);
        });
        add(CommandInfo::new("gweike.read_card", "Read Gweike card", "Laser"), &|c| {
            c.hardware = true;
            c.hardware_class = Some(HardwareClass::Connect);
            c.gweike = true;
        });
        add(CommandInfo::new("gweike.job_start", "Start Gweike job", "Laser"), &|c| {
            c.hardware = true;
            c.hardware_class = Some(HardwareClass::FireOrJob);
            c.gweike = true;
        });
        add(CommandInfo::new("machine.unflagged", "Unflagged motion", "Laser"), &|c| c.hardware = true);
        add(CommandInfo::new("edit.disabled_example", "Disabled example", "Edit"), &|c| c.enabled = false);
        v
    }

    fn find(&self, id: &str) -> Option<CommandInfo> {
        self.catalog().into_iter().find(|c| c.id == id)
    }

    fn snapshot(&mut self) {
        self.undo.push(self.doc.clone());
        self.redo.clear();
    }

    fn layer_index(&self, v: &Value) -> Result<usize, CommandError> {
        match v {
            Value::Null => Ok(0),
            Value::Number(n) => {
                let i = n.as_u64().unwrap_or(u64::MAX) as usize;
                if i < self.doc.layers.len() {
                    Ok(i)
                } else {
                    Err(CommandError::failed(format!(
                        "layer index {i} does not exist; there are {} layers (0..{})",
                        self.doc.layers.len(),
                        self.doc.layers.len().saturating_sub(1)
                    )))
                }
            }
            Value::String(s) => self
                .doc
                .layers
                .iter()
                .position(|l| l == s)
                .ok_or_else(|| CommandError::failed(format!("no layer named `{s}`; layers are {:?}", self.doc.layers))),
            _ => Err(CommandError::invalid("layer must be an index or a name")),
        }
    }

    fn shape_json(&self, s: &Shape) -> Value {
        json!({"id": s.id, "kind": "rect", "x": s.x, "y": s.y, "width": s.w, "height": s.h, "layer": s.layer, "layer_name": self.doc.layers.get(s.layer)})
    }

    fn do_execute(&mut self, id: &str, a: Value) -> Result<Value, CommandError> {
        let num = |k: &str| a[k].as_f64().unwrap_or(0.0);
        match id {
            "file.new" => {
                self.snapshot();
                self.doc = Doc { layers: vec!["Cut".into(), "Engrave".into()], ..Default::default() };
                Ok(json!({}))
            }
            "file.open" | "file.import" | "file.save_as" | "file.export" => {
                let p = a["path"].as_str().unwrap_or("").to_string();
                if id == "file.open" {
                    self.doc.path = Some(p.clone());
                }
                if id == "file.save_as" {
                    self.doc.path = Some(p.clone());
                }
                Ok(json!({"path": p}))
            }
            "file.save" => match &self.doc.path {
                Some(p) => Ok(json!({"path": p})),
                None => Err(CommandError::failed("the design has no file yet: use file.save_as with a path")),
            },
            "edit.undo" | "edit.redo" => {
                let n = a["count"].as_u64().unwrap_or(1);
                let mut done = 0;
                for _ in 0..n {
                    let (from, to) = if id == "edit.undo" { (&mut self.undo, &mut self.redo) } else { (&mut self.redo, &mut self.undo) };
                    match from.pop() {
                        Some(d) => {
                            to.push(std::mem::replace(&mut self.doc, d));
                            done += 1;
                        }
                        None => break,
                    }
                }
                Ok(json!({"done": done}))
            }
            "settings.set" => {
                let k = a["key"].as_str().unwrap_or("").to_string();
                if !self.settings.contains_key(&k) {
                    return Err(CommandError::failed(format!("unknown setting `{k}`; list_settings shows the keys")));
                }
                self.settings.insert(k, a["value"].clone());
                Ok(json!({}))
            }
            "shape.add_rect" => {
                let layer = self.layer_index(&a["layer"])?;
                self.snapshot();
                self.doc.next_id += 1;
                let sid = format!("s{}", self.doc.next_id);
                self.doc.shapes.push(Shape { id: sid.clone(), x: num("x"), y: num("y"), w: num("width"), h: num("height"), layer });
                Ok(json!({"id": sid}))
            }
            "shape.move" => {
                let sid = a["id"].as_str().unwrap_or("");
                if !self.doc.shapes.iter().any(|s| s.id == sid) {
                    return Err(CommandError::failed(format!("no shape with id `{sid}`; inspect_design path `design.shapes` lists ids")));
                }
                self.snapshot();
                if let Some(s) = self.doc.shapes.iter_mut().find(|s| s.id == sid) {
                    s.x += num("dx");
                    s.y += num("dy");
                }
                Ok(json!({}))
            }
            "shape.delete" => {
                let sid = a["id"].as_str().unwrap_or("").to_string();
                if !self.doc.shapes.iter().any(|s| s.id == sid) {
                    return Err(CommandError::failed(format!("no shape with id `{sid}`")));
                }
                self.snapshot();
                self.doc.shapes.retain(|s| s.id != sid);
                Ok(json!({}))
            }
            "job.long_task" => {
                self.next_op += 1;
                let pid = format!("p{}", self.next_op);
                self.running.insert(pid.clone(), ("trace image".into(), 2));
                Err(CommandError::still_running(pid, "trace image"))
            }
            "machine.jog"
            | "machine.connect"
            | "job.start"
            | "laser.test_fire"
            | "machine.emergency_stop"
            | "machine.pause"
            | "gweike.read_card"
            | "gweike.job_start"
            | "machine.unflagged" => {
                self.event("machine", id, "simulated", "no device is opened by the mock");
                Ok(json!({"simulated": true}))
            }
            _ => Err(CommandError::failed("not implemented in the mock")),
        }
    }
}

impl Backend for MockBackend {
    fn describe(&self) -> String {
        "DarkBurn mock backend (in-memory, no machine)".into()
    }

    fn list_commands(&mut self, query: &ListQuery) -> Result<CommandPage, CommandError> {
        Ok(query.apply(self.catalog()))
    }

    fn describe_command(&mut self, id: &str) -> Result<Option<CommandInfo>, CommandError> {
        Ok(self.find(id))
    }

    fn execute(&mut self, id: &str, args: Value) -> Result<Value, CommandError> {
        let Some(info) = self.find(id) else {
            return Err(CommandError::new(ErrorCode::UnknownCommand, format!("unknown command `{id}`")));
        };
        if !info.enabled {
            return Err(CommandError::new(ErrorCode::Disabled, format!("`{id}` is disabled")));
        }
        self.executed.push(id.to_string());
        let r = self.do_execute(id, args);
        match &r {
            Ok(_) => self.event("command", id, "ok", ""),
            Err(e) => self.event("command", id, "error", &e.message),
        }
        r
    }

    fn checkpoint(&mut self) -> Result<Option<String>, CommandError> {
        if self.no_rollback {
            return Ok(None);
        }
        self.next_cp += 1;
        let id = format!("cp{}", self.next_cp);
        self.checkpoints.insert(id.clone(), (self.doc.clone(), self.undo.clone(), self.redo.clone()));
        Ok(Some(id))
    }

    fn restore(&mut self, checkpoint: &str) -> Result<(), CommandError> {
        let (d, u, r) = self.checkpoints.remove(checkpoint).ok_or_else(|| CommandError::failed("unknown checkpoint"))?;
        self.doc = d;
        self.undo = u;
        self.redo = r;
        Ok(())
    }

    fn discard_checkpoint(&mut self, checkpoint: &str) {
        self.checkpoints.remove(checkpoint);
    }

    fn inspect(&mut self, path: &str, _depth: u32) -> Result<Value, CommandError> {
        let parts: Vec<&str> = path.split('.').collect();
        match parts.as_slice() {
            ["design"] => Ok(
                json!({"path": self.doc.path, "layers": self.doc.layers, "shapes": self.doc.shapes.iter().map(|s| self.shape_json(s)).collect::<Vec<_>>(), "selection": []}),
            ),
            ["design", "shapes"] => Ok(Value::Array(self.doc.shapes.iter().map(|s| self.shape_json(s)).collect())),
            ["design", "shapes", sid] => self
                .doc
                .shapes
                .iter()
                .find(|s| s.id == *sid)
                .map(|s| self.shape_json(s))
                .ok_or_else(|| CommandError::failed(format!("no shape with id `{sid}`"))),
            ["design", "layers"] => Ok(json!(self.doc.layers.iter().enumerate().map(|(i, n)| json!({"index": i, "name": n})).collect::<Vec<_>>())),
            ["design", "layers", l] => {
                let idx = match l.parse::<usize>() {
                    Ok(i) => self.layer_index(&json!(i))?,
                    Err(_) => self.layer_index(&json!(l))?,
                };
                Ok(
                    json!({"index": idx, "name": self.doc.layers[idx], "shapes": self.doc.shapes.iter().filter(|s| s.layer == idx).map(|s| s.id.clone()).collect::<Vec<_>>()}),
                )
            }
            ["design", "selection"] => Ok(json!([])),
            ["settings"] => Ok(json!(self.settings.iter().map(|(k, v)| json!({"key": k, "value": v})).collect::<Vec<_>>())),
            ["settings", rest @ ..] => {
                let k = rest.join(".");
                self.settings.get(&k).map(|v| json!({"key": k, "value": v})).ok_or_else(|| CommandError::failed(format!("unknown setting `{k}`")))
            }
            ["machine", "status"] | ["machine"] => Ok(json!({"connected": false, "state": "disconnected", "simulated": true})),
            ["machine", "devices"] => Ok(json!([{"id": "mock-grbl", "name": "Mock GRBL (simulated)", "kind": "grbl", "gweike": false}])),
            ["ui"] => Ok(json!({"panels": ["Cuts / Layers", "Laser"], "dialogs": [], "focus": null})),
            ["undo"] => Ok(json!({"undo": self.undo.len(), "redo": self.redo.len()})),
            ["job"] => Ok(json!({"shapes": self.doc.shapes.len()})),
            _ => Err(CommandError::failed(format!("unknown path `{path}`; roots are design, settings, machine, ui, undo, job"))),
        }
    }

    fn screenshot(&mut self, _region: Option<Region>) -> Result<Vec<u8>, CommandError> {
        base64_decode(PNG_1X1).ok_or_else(|| CommandError::failed("bad png"))
    }

    fn measure(&mut self, _request: Value) -> Result<Value, CommandError> {
        let s = &self.doc.shapes;
        if s.is_empty() {
            return Ok(json!({"shapes": 0}));
        }
        let minx = s.iter().map(|s| s.x).fold(f64::MAX, f64::min);
        let miny = s.iter().map(|s| s.y).fold(f64::MAX, f64::min);
        let maxx = s.iter().map(|s| s.x + s.w).fold(f64::MIN, f64::max);
        let maxy = s.iter().map(|s| s.y + s.h).fold(f64::MIN, f64::max);
        let length: f64 = s.iter().map(|s| 2.0 * (s.w + s.h)).sum();
        let area: f64 = s.iter().map(|s| s.w * s.h).sum();
        Ok(
            json!({"shapes": s.len(), "bbox": {"min": [minx, miny], "max": [maxx, maxy]}, "length_mm": length, "area_mm2": area, "estimated_time_s": length / 10.0}),
        )
    }

    fn preview_job(&mut self, request: Value) -> Result<Value, CommandError> {
        let mut g = String::from("G21\nG90\n");
        for s in &self.doc.shapes {
            g.push_str(&format!("G0 X{} Y{}\nM3\nG1 X{} Y{}\nM5\n", s.x, s.y, s.x + s.w, s.y));
        }
        if request["format"] == "gcode" {
            Ok(json!({"gcode": g, "sent": false}))
        } else {
            Ok(json!({"moves": self.doc.shapes.len() * 2, "sent": false}))
        }
    }

    fn pending(&mut self, id: &str) -> Result<PendingState, CommandError> {
        match self.running.get_mut(id) {
            None => Ok(PendingState::Unknown),
            Some((what, left)) if *left > 0 => {
                *left -= 1;
                Ok(PendingState::Running { what: what.clone(), running_s: 0 })
            }
            Some(_) => {
                let (what, _) = self.running.remove(id).unwrap_or_default();
                Ok(PendingState::Done { what, result: Ok(json!({"done": true})) })
            }
        }
    }

    fn running(&mut self) -> Result<Vec<RunningOp>, CommandError> {
        Ok(self.running.iter().map(|(id, (what, _))| RunningOp { id: id.clone(), what: what.clone(), running_s: 0 }).collect())
    }

    fn events(&mut self, since: u64) -> Result<EventPage, CommandError> {
        Ok(EventPage { events: self.events.iter().filter(|e| e.seq > since).cloned().collect(), next: self.seq })
    }

    fn request_confirmation(&mut self, pending_id: &str, command: &str, summary: &str) -> Result<(), CommandError> {
        self.confirmations_requested.push((pending_id.into(), command.into(), summary.into()));
        self.event("dialog", command, "opened", summary);
        Ok(())
    }
}
