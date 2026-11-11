//! Bug hunt 7: a seeded monkey.
//!
//! From a fixed seed, a long sequence of registered commands with generated arguments (in range,
//! out of range, of the wrong kind, missing) runs on a document that holds every kind of object.
//! After every command the invariants in [`Monkey::check`] must hold. A failure names the seed,
//! the step and the command with its arguments, and prints the whole sequence so far, so it can
//! be cut down by hand into a named regression test (they are at the end of this file).
//!
//! Hardware safety: the monkey only runs the commands [`excluded`] lets through. No command that
//! connects to, moves, fires or streams to a machine is among them, nothing that opens a file
//! dialog, quits, starts another program or uses the network, and every path it passes lies in
//! its own temporary folder. The dialog seam is scripted and must stay unused, and no machine
//! may ever be connected.
//!
//! Explored beyond the seeds kept here: see [`EXPLORED`]; invariants that do not hold by design:
//! [`EXCEPTIONS`].

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use darkburn_core::kurbo::Affine;
use darkburn_core::{Document, ImageData, ImageShape, LAYER_COUNT, Shape, ShapeKind, T1, project};
use serde_json::{Map, Value, json};

use crate::commands;
use crate::job_build::{DeviceGcodeSettings, gcode_text};
use crate::prefs::Prefs;
use crate::shell::DarkBurn;

// ----- What the monkey may run -------------------------------------------------------------------

/// Why the monkey never runs `id`; None when it may.
fn excluded(id: &str) -> Option<&'static str> {
    if commands::hardware(id) {
        return Some("operates the machine");
    }
    const GROUPS: &[(&str, &str)] = &[
        ("machine.", "the machine connection"),
        ("device.", "devices: connections, port and network scans, device files"),
        ("laser.", "the Laser window: connection, jogging, machine dialogs"),
        ("gcode.", "machine profiles and their files"),
        ("galvo.", "galvo machine set-up (another hunter's area)"),
        ("gweike.", "Gweike machine set-up (another hunter's area)"),
        ("camera.", "the camera: a device"),
        ("pnc.", "print and cut: reads the laser position"),
        ("rotary.", "machine set-up"),
        ("cylinder.", "machine set-up"),
        ("test.", "machine calibration tests"),
        ("library.", "material library files"),
        ("art.", "art library files"),
        ("hotkeys.", "preferences, not the document"),
        ("support.", "writes a support file"),
        ("help.", "help windows, the browser and the update check (network)"),
        ("panel.", "window layout, not the document"),
        ("toolbar.", "window layout, not the document"),
        ("lang.", "language, not the document"),
        ("window.", "window layout, not the document"),
        ("file.recent.", "opens whatever the recent list names"),
    ];
    if let Some((_, why)) = GROUPS.iter().find(|(p, _)| id.starts_with(p)) {
        return Some(why);
    }
    const ONE: &[(&str, &str)] = &[
        ("file.open", "file dialog"),
        ("file.import", "file dialog"),
        ("file.save", "file dialog when the project has no file"),
        ("file.save_as", "file dialog"),
        ("file.save_lbrn2", "file dialog"),
        ("file.save_gcode", "file dialog"),
        ("file.export", "file dialog"),
        ("file.export_gweike_dxf", "file dialog"),
        ("file.export_gweike_nc", "file dialog"),
        ("file.print", "file dialog, printer"),
        ("file.print_colour", "file dialog, printer"),
        ("file.save_bitmap", "file dialog"),
        ("file.save_capture", "file dialog"),
        ("file.prefs_folder", "opens the file manager"),
        ("file.prefs_export", "file dialog"),
        ("file.prefs_import", "file dialog"),
        ("file.prefs_backup", "file dialog"),
        ("file.new_window", "starts another program"),
        ("file.exit", "quits"),
        ("edit.image_replace", "file dialog"),
        ("edit.image_replace_fit", "file dialog"),
        ("edit.image_refresh", "reads the picture's file from disk"),
        ("text.font_picker", "file dialog"),
        ("text.add_shx", "reads a font file"),
        ("image.replace", "reads a file"),
        ("image.save_bitmap", "writes a file"),
        ("image.trace_capture", "reads a file"),
        ("image.preset_import", "reads a file"),
        ("image.preset_export", "writes a file"),
        ("image.scan_offsets_import", "reads a file"),
        ("image.scan_offsets_export", "writes a file"),
        ("image.scan_offsets", "device tuning"),
        ("project.import", "reads files"),
        ("project.export", "writes files, among them machine files"),
        ("project.print_pdf", "writes a file"),
        ("project.prefs_export", "writes a file"),
        ("project.prefs_import", "replaces the preferences, the agent settings among them"),
        ("variable.csv_load", "reads a file"),
        ("job.save_gcode", "writes a machine file"),
        ("job.set_gcode_settings", "device set-up"),
        ("job.gcode_settings", "device set-up"),
        ("job.set_user_origin", "machine state"),
        ("job.clear_user_origin", "machine state"),
        ("job.frame_path", "builds a machine move"),
        ("job.preview_start_here", "the preview's machine buttons"),
        ("job.preview_save_from_here", "writes a machine file"),
        ("job.preview_save_image", "writes a file"),
        ("job.material_test_presets", "reads and writes files"),
        ("job.optimization_defaults", "preferences, not the document"),
        ("job.log_info", "the job log, not the document"),
        ("job.log_show", "opens the file manager"),
        ("job.sound_test", "plays a sound"),
        ("test", "not a real command"),
    ];
    ONE.iter().find(|(c, _)| *c == id).map(|(_, why)| *why)
}

/// `setting.set` keys the monkey may change (never a path, a URL or the agent settings).
const SETTING_KEYS: &[&str] = &[
    "filled",
    "smooth",
    "show_grid",
    "grid_spacing",
    "grid_contrast",
    "page_width",
    "page_height",
    "svg_dpi",
    "import_group",
    "import_select",
    "import_tool_layers",
];

/// The commands the monkey draws from, in registry order.
fn command_set() -> Vec<commands::Info> {
    commands::list().into_iter().filter(|i| i.implemented && excluded(&i.id).is_none()).collect()
}

// ----- A small deterministic generator -----------------------------------------------------------

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0x7A3C_91E5_02B6_44D7)
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn pick(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }

    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn f(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.unit()
    }

    fn chance(&mut self, p: f64) -> bool {
        self.unit() < p
    }

    fn of<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.pick(items.len())]
    }
}

// ----- Arguments ---------------------------------------------------------------------------------

/// How wild the generated arguments are.
#[derive(Clone, Copy, PartialEq)]
enum Wild {
    /// Sizes and counts a person would type; the odd value out of range or of the wrong kind.
    Mild,
    /// Also huge and tiny numbers, huge counts, long texts.
    Full,
}

struct Gen<'a> {
    rng: &'a mut Rng,
    wild: Wild,
    ids: Vec<u64>,
    dir: &'a std::path::Path,
    saved: &'a [PathBuf],
}

impl Gen<'_> {
    fn number(&mut self) -> Value {
        let r = self.rng.unit();
        let edge = if self.wild == Wild::Full { 0.12 } else { 0.03 };
        if r < edge {
            let odd: &[f64] = if self.wild == Wild::Full {
                &[0.0, -0.0, 1e-9, -1e-9, 1e-300, 1e9, -1e9, 1e15, 1e150, -1e150, 1e300, -1e300, 1.7e308, -1.7e308, 0.5, -1.0]
            } else {
                &[0.0, -1.0, 1e-9, 1e6, -1e6, 0.001]
            };
            json!(*self.rng.of(odd))
        } else if r < edge + 0.02 {
            self.junk()
        } else if r < 0.6 {
            json!((self.rng.f(-20.0, 200.0) * 10.0).round() / 10.0)
        } else {
            json!(self.rng.f(0.1, 60.0))
        }
    }

    fn integer(&mut self) -> Value {
        let r = self.rng.unit();
        let edge = if self.wild == Wild::Full { 0.1 } else { 0.03 };
        if r < edge {
            let odd: &[i64] = if self.wild == Wild::Full { &[-1, 0, 1000, 100_000, i64::MAX, i64::MIN, 4_294_967_296, -7] } else { &[-1, 0, 40] };
            json!(*self.rng.of(odd))
        } else if r < edge + 0.02 {
            self.junk()
        } else {
            json!(self.rng.pick(6))
        }
    }

    /// A value of a kind no argument wants.
    fn junk(&mut self) -> Value {
        match self.rng.pick(7) {
            0 => Value::Null,
            1 => json!("NaN"),
            2 => json!("Infinity"),
            3 => json!(true),
            4 => json!({}),
            5 => json!([]),
            _ => json!("12"),
        }
    }

    fn text(&mut self) -> Value {
        const TEXTS: &[&str] = &["A", "Hej 12", "", " ", "Line one\nLine two", "ÆØÅ ж 日本", "%serial%", "0123456789", "a\tb", "🙂"];
        if self.wild == Wild::Full && self.rng.chance(0.03) {
            return json!("x".repeat(5000));
        }
        if self.rng.chance(0.02) {
            return self.junk();
        }
        json!(*self.rng.of(TEXTS))
    }

    fn shape_id(&mut self) -> Value {
        if self.ids.is_empty() || self.rng.chance(0.06) {
            return json!(*self.rng.of(&[0u64, 999_999, u64::MAX]));
        }
        json!(*self.rng.of(&self.ids))
    }

    fn id_list(&mut self) -> Value {
        let n = match self.rng.pick(10) {
            0 => 0,
            1..=4 => 1,
            5..=7 => 2,
            _ => 4,
        };
        Value::Array((0..n).map(|_| self.shape_id()).collect())
    }

    fn layer(&mut self) -> Value {
        match self.rng.pick(20) {
            0 => json!(LAYER_COUNT),
            1 => json!(-1),
            2 => self.junk(),
            3 | 4 => json!(T1 + self.rng.pick(2)),
            5..=8 => json!(self.rng.pick(LAYER_COUNT)),
            _ => json!(self.rng.pick(5)),
        }
    }

    fn point(&mut self) -> Value {
        Value::Array(vec![self.number(), self.number()])
    }

    fn points(&mut self, n: usize) -> Value {
        Value::Array((0..n).map(|_| self.point()).collect())
    }

    fn path(&mut self, id: &str) -> Value {
        if id == "project.open" {
            if self.saved.is_empty() || self.rng.chance(0.1) {
                return json!(self.dir.join("missing.dbrn").to_string_lossy());
            }
            return json!(self.rng.of(self.saved).to_string_lossy());
        }
        json!(self.dir.join(format!("monkey-{}.dbrn", self.rng.pick(3))).to_string_lossy())
    }

    fn svg_path(&mut self) -> Value {
        const PATHS: &[&str] = &[
            "M 10 10 L 60 10 L 60 40 Z",
            "M 0 0 L 30 0",
            "M 5 5 C 20 40 60 40 80 5",
            "M 10 10 L 40 10 L 40 40 L 10 40 Z M 20 20 L 30 20 L 30 30 L 20 30 Z",
            "M 0 0 L 50 50 L 0 50 L 50 0 Z",
            "M 5 5",
            "M 5 5 Z",
            "M 5 5 L 5 5",
            "",
            "not a path",
            "M 1e300 0 L 0 1e300",
            "M 0 0 Q 20 60 40 0 T 80 0",
            "M 0 0 A 20 20 0 0 1 40 0",
        ];
        json!(*self.rng.of(PATHS))
    }

    /// A value for the argument `name` of command `id` with schema `s`.
    fn value(&mut self, id: &str, name: &str, s: &Value) -> Value {
        if let Some(options) = s.get("enum").and_then(Value::as_array) {
            if self.rng.chance(0.04) {
                return json!("no_such_choice");
            }
            return self.rng.of(options).clone();
        }
        match name {
            "shape" | "id" | "path_id" | "text_id" | "path_shape" => return self.shape_id(),
            "ids" => return self.id_list(),
            "layer" | "first_layer" | "label_layer" => return self.layer(),
            "path" => return self.path(id),
            "svg_path" => return self.svg_path(),
            "points" => {
                let n = self.rng.pick(6);
                return self.points(n);
            }
            "corners" => return self.points(4),
            "grid" => {
                let n = if self.rng.chance(0.9) { 16 } else { self.rng.pick(20) };
                return self.points(n);
            }
            "at" | "c" | "from_a" | "from_b" | "to_a" | "to_b" => return self.point(),
            "a" | "b" => {
                return if matches!(id, "tools.boolean_assistant") { self.id_list() } else { self.point() };
            }
            "nodes" => {
                let n = self.rng.pick(4);
                return Value::Array((0..n).map(|_| json!([self.rng.pick(2), self.rng.pick(6)])).collect());
            }
            "clip" => return json!([self.rng.f(0.0, 0.4), self.rng.f(0.0, 0.4), self.rng.f(0.6, 1.0), self.rng.f(0.6, 1.0)]),
            "start_point" => return if self.rng.chance(0.3) { Value::Null } else { self.point() },
            "font" => return if self.rng.chance(0.9) { json!("IBM Plex Sans") } else { json!("No Such Font") },
            "distance" => return if self.rng.chance(0.1) { json!("2in/4") } else { self.number() },
            _ => {}
        }
        let kind = match s.get("type") {
            Some(Value::String(t)) => t.as_str(),
            Some(Value::Array(a)) => a.first().and_then(Value::as_str).unwrap_or(""),
            _ => "",
        };
        match kind {
            "number" => self.number(),
            "integer" => self.integer(),
            "boolean" => {
                if self.rng.chance(0.02) {
                    self.junk()
                } else {
                    json!(self.rng.chance(0.5))
                }
            }
            "string" => self.text(),
            "array" => self.point(),
            "object" => json!({}),
            _ => self.number(),
        }
    }

    /// Arguments for `info`.
    fn args(&mut self, info: &commands::Info) -> Value {
        if info.id == "setting.set" {
            let key = *self.rng.of(SETTING_KEYS);
            let value = match self.rng.pick(4) {
                0 => json!(self.rng.chance(0.5)),
                _ => self.number(),
            };
            return json!({ "key": key, "value": value });
        }
        if info.id == "layer.set_param" {
            let field = *self.rng.of(&["name", "speed", "max_power", "min_power", "interval", "passes", "output", "show", "air"]);
            let value = match field {
                "name" => self.text(),
                "output" | "show" | "air" => json!(self.rng.chance(0.5)),
                "passes" => self.integer(),
                _ => self.number(),
            };
            return json!({ "layer": self.layer(), "field": field, "value": value });
        }
        let Some(props) = info.args_schema.get("properties").and_then(Value::as_object) else {
            return if self.rng.chance(0.03) { self.junk() } else { Value::Null };
        };
        if self.rng.chance(0.01) {
            return self.junk();
        }
        let required: Vec<&str> =
            info.args_schema.get("required").and_then(Value::as_array).map(|r| r.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
        let mut out = Map::new();
        for (name, schema) in props {
            let p = if required.contains(&name.as_str()) { 0.96 } else { 0.35 };
            if self.rng.chance(p) {
                let v = self.value(&info.id, name, schema);
                out.insert(name.clone(), v);
            }
        }
        if self.rng.chance(0.02) {
            out.insert("no_such_argument".into(), json!(1));
        }
        Value::Object(out)
    }
}

// ----- The monkey --------------------------------------------------------------------------------

/// Runs a command on the app; the window variant also draws a frame afterwards.
trait Host {
    fn app<R>(&mut self, f: impl FnOnce(&mut DarkBurn, &egui::Context) -> R) -> R;
    fn frame(&mut self) {}
}

struct Headless {
    app: DarkBurn,
    ctx: egui::Context,
}

impl Host for Headless {
    fn app<R>(&mut self, f: impl FnOnce(&mut DarkBurn, &egui::Context) -> R) -> R {
        f(&mut self.app, &self.ctx)
    }
}

#[allow(dead_code)]
impl Host for crate::ui_harness::Gui {
    fn app<R>(&mut self, f: impl FnOnce(&mut DarkBurn, &egui::Context) -> R) -> R {
        let ctx = self.ctx();
        f(&mut crate::ui_harness::Gui::app(self), &ctx)
    }

    fn frame(&mut self) {
        self.frames(1);
    }
}

fn temp_dir(tag: &str) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!("darkburn-monkey-{}-{tag}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn headless(dir: &std::path::Path) -> Headless {
    ::rfd::script_begin();
    let mut app = DarkBurn::new(Prefs::default());
    app.settings_dir = Some(dir.join("settings"));
    std::fs::create_dir_all(dir.join("settings")).expect("temp dir");
    app.safe_mode = true;
    Headless { app, ctx: egui::Context::default() }
}

/// A tiny real PNG.
fn png() -> Vec<u8> {
    let px: Vec<u8> = (0..16 * 8).map(|i| if i % 16 < 6 && i / 16 < 5 { 0 } else { 255 }).collect();
    let img = image::GrayImage::from_raw(16, 8, px).unwrap();
    let mut png = Vec::new();
    image::DynamicImage::ImageLuma8(img).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
    png
}

/// Every kind of object: rectangle, ellipse, polygon, open and closed paths, text, text on a
/// path, a picture, a QR code, a group, a group in a group, a virtual and a real array, tabs,
/// a start point, a locked shape, a shape on a hidden layer, a tool layer and a layer with a
/// sub-layer.
fn seed_document(app: &mut DarkBurn) {
    let run = |app: &mut DarkBurn, id: &str, args: Value| -> Value {
        crate::api::execute_headless(app, id, &args).unwrap_or_else(|e| panic!("seed document: {id} {args}: {e}"))
    };
    let id = |v: &Value| v["id"].as_u64().expect("an id");
    let rect = id(&run(app, "shape.add_rect", json!({ "x": 10, "y": 10, "width": 40, "height": 25, "radius": 3, "layer": 0 })));
    let ellipse = id(&run(app, "shape.add_ellipse", json!({ "x": 60, "y": 10, "width": 30, "height": 20, "layer": 1 })));
    let polygon = id(&run(app, "shape.add_polygon", json!({ "cx": 120, "cy": 25, "sides": 6, "radius": 14, "layer": 1 })));
    let open = id(&run(app, "shape.add_path", json!({ "svg_path": "M 10 60 C 30 90 60 90 90 60", "layer": 2 })));
    let closed = id(&run(app, "shape.add_path", json!({ "svg_path": "M 100 50 L 140 50 L 140 80 L 100 80 Z", "layer": 0 })));
    let text = id(&run(app, "text.add", json!({ "text": "Monkey", "x": 20, "y": 110, "height": 8, "font": "IBM Plex Sans", "layer": 2 })));
    let curved = id(&run(app, "text.add", json!({ "text": "Round", "x": 20, "y": 130, "height": 6, "font": "IBM Plex Sans", "layer": 0 })));
    let guide = id(&run(app, "shape.add_path", json!({ "svg_path": "M 100 110 C 120 140 150 140 170 110", "layer": 0 })));
    run(app, "text.on_path", json!({ "text_id": curved, "path_id": guide }));
    run(app, "qr.add", json!({ "text": "darkburn", "x": 150, "y": 10, "size": 20, "layer": 3 }));

    let key = app.editor.doc.add_image(ImageData { format: "png".into(), bytes: Arc::from(png()), px_width: 16, px_height: 8 });
    let picture = app
        .editor
        .doc
        .add(Shape::new(
            ShapeKind::Image(ImageShape { image: key, width: 32.0, height: 16.0, ..Default::default() }),
            Affine::translate((200.0, 30.0)),
            4,
        ))
        .0;

    // A group, and a group inside a group.
    let a = id(&run(app, "shape.add_rect", json!({ "x": 200, "y": 60, "width": 10, "height": 10, "layer": 0 })));
    let b = id(&run(app, "shape.add_ellipse", json!({ "x": 215, "y": 60, "width": 10, "height": 10, "layer": 1 })));
    run(app, "selection.set", json!({ "ids": [a, b] }));
    run(app, "arrange.group", Value::Null);
    let inner = app.editor.selected()[0].0;
    let c = id(&run(app, "shape.add_polygon", json!({ "cx": 240, "cy": 65, "sides": 3, "radius": 6, "layer": 2 })));
    run(app, "selection.set", json!({ "ids": [inner, c] }));
    run(app, "arrange.group", Value::Null);

    // Arrays: one virtual, one real.
    let cell = id(&run(app, "shape.add_rect", json!({ "x": 10, "y": 160, "width": 8, "height": 6, "layer": 0 })));
    run(app, "selection.set", json!({ "ids": [cell] }));
    run(app, "arrange.grid_array", json!({ "rows": 2, "cols": 3, "dx": 2, "dy": 2, "virtual": true }));
    let cell = id(&run(app, "shape.add_ellipse", json!({ "x": 80, "y": 160, "width": 6, "height": 6, "layer": 1 })));
    run(app, "selection.set", json!({ "ids": [cell] }));
    run(app, "arrange.grid_array", json!({ "rows": 2, "cols": 2, "dx": 3, "dy": 3 }));

    // Tabs, a start point, a locked shape, layers of every sort.
    run(app, "tabs.add", json!({ "shape": closed, "x": 120, "y": 50 }));
    run(app, "shape.set_start_point", json!({ "shape": rect, "x": 50, "y": 20 }));
    run(app, "selection.set", json!({ "ids": [polygon] }));
    run(app, "shape.set_props", json!({ "locked": true, "name": "locked one" }));
    let tool = id(&run(app, "shape.add_rect", json!({ "x": 0, "y": 0, "width": 280, "height": 190, "layer": T1 })));
    let hidden = id(&run(app, "shape.add_rect", json!({ "x": 150, "y": 160, "width": 12, "height": 12, "layer": 5 })));
    run(app, "layer.set_param", json!({ "layer": 5, "field": "show", "value": false }));
    run(app, "layer.set_mode", json!({ "layer": 1, "mode": "fill" }));
    run(app, "layer.set_mode", json!({ "layer": 4, "mode": "image" }));
    assert!(app.editor.edit("Sub-layer", |d, _| d.layer_mut(2).add_sub_layer()));
    run(app, "selection.set", json!({ "ids": [rect, ellipse, open, text] }));
    let _ = (picture, tool, hidden);
}

struct Step {
    id: String,
    args: Value,
}

struct Monkey<H: Host> {
    host: H,
    seed: u64,
    rng: Rng,
    wild: Wild,
    set: Vec<commands::Info>,
    dir: PathBuf,
    /// Project files the monkey has saved.
    saved_files: Vec<PathBuf>,
    /// The document as of the last save, open or new project.
    saved_doc: Document,
    trail: Vec<Step>,
    /// How often each command ran, and how often it was accepted.
    tally: std::collections::BTreeMap<String, (u32, u32)>,
    /// Build the job every this many steps (it is the slow check).
    job_every: usize,
    /// Exploring: a finding that leaves the document sound is noted here and the run goes on.
    soft: bool,
    /// Print every command before it runs (to find the one that takes the process down).
    trace: bool,
    findings: std::collections::BTreeMap<String, String>,
    /// How often the planner's time budget made two builds of one document differ.
    budget_hits: u32,
}

/// A command may take this long at most.
const LIMIT: Duration = Duration::from_secs(20);
/// The time limit for building the job twice. Longer than [`LIMIT`]: see [`EXCEPTIONS`].
const JOB_LIMIT: Duration = Duration::from_secs(120);

/// Commands that replace the project: no undo step, the history starts again.
const NEW_PROJECT: &[&str] = &["file.new", "project.open"];

impl<H: Host> Monkey<H> {
    fn new(mut host: H, seed: u64, wild: Wild, dir: PathBuf) -> Monkey<H> {
        host.app(|app, _| {
            seed_document(app);
            app.editor.history.clear();
            app.editor.mark_saved();
        });
        let saved_doc = host.app(|app, _| app.editor.doc.clone());
        let mut m = Monkey {
            host,
            seed,
            rng: Rng::new(seed),
            wild,
            set: command_set(),
            dir,
            saved_files: Vec::new(),
            saved_doc,
            trail: Vec::new(),
            tally: Default::default(),
            job_every: 10,
            soft: false,
            trace: std::env::var("MONKEY_TRACE").is_ok(),
            findings: Default::default(),
            budget_hits: 0,
        };
        m.check_state("the seed document");
        m.check_job("the seed document");
        m
    }

    fn fail(&self, what: &str) -> ! {
        let mut s = format!("\nMONKEY seed {} step {}: {what}\nThe sequence:\n", self.seed, self.trail.len());
        let from = self.trail.len().saturating_sub(400);
        for (i, t) in self.trail.iter().enumerate().skip(from) {
            s.push_str(&format!("  {i}: {} {}\n", t.id, t.args));
        }
        panic!("{s}");
    }

    /// A finding that leaves the document sound: fatal in the suite, noted when exploring.
    fn flag(&mut self, class: &str, id: &str, what: &str) {
        if !self.soft {
            self.fail(what);
        }
        let (seed, step) = (self.seed, self.trail.len() - 1);
        self.findings.entry(format!("{class} | {id}")).or_insert_with(|| format!("seed {seed} step {step}: {what}"));
    }

    /// One random command and every check.
    fn step(&mut self) {
        let info = self.set[self.rng.pick(self.set.len())].clone();
        let ids: Vec<u64> = self.host.app(|app, _| app.editor.doc.iter().map(|s| s.id.0).collect());
        let args = Gen { rng: &mut self.rng, wild: self.wild, ids, dir: &self.dir, saved: &self.saved_files }.args(&info);
        let _ = self.run(&info.id, args);
    }

    /// Run `id` and check everything.
    fn run(&mut self, id: &str, args: Value) -> Result<Value, String> {
        self.trail.push(Step { id: id.to_string(), args: args.clone() });
        if self.trace {
            eprintln!("TRACE seed {} step {}: {id} {args}", self.seed, self.trail.len() - 1);
        }
        let (before, undo_before, modified_before) =
            self.host.app(|app, _| (app.editor.doc.clone(), app.editor.history.len(), app.editor.is_modified()));
        let started = Instant::now();
        let done = Arc::new(AtomicBool::new(false));
        let watchdog = {
            let done = done.clone();
            let what = format!("MONKEY seed {} step {}: {id} {args} hangs (over 120 s)", self.seed, self.trail.len() - 1);
            std::thread::spawn(move || {
                let t = Instant::now();
                while !done.load(Ordering::Relaxed) {
                    if t.elapsed() > Duration::from_secs(120) {
                        eprintln!("{what}");
                        std::process::abort();
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
            })
        };
        let result = catch_unwind(AssertUnwindSafe(|| {
            let r = self.host.app(|app, ctx| crate::api::execute(app, ctx, id, &args));
            self.host.frame();
            r
        }));
        done.store(true, Ordering::Relaxed);
        let _ = watchdog.join();
        let result = match result {
            Ok(r) => r,
            Err(p) => {
                let text = p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default();
                self.fail(&format!("{id} {args} panicked: {text}"));
            }
        };
        if started.elapsed() > LIMIT {
            self.fail(&format!("{id} {args} took {:?}", started.elapsed()));
        }
        let t = self.tally.entry(id.to_string()).or_default();
        t.0 += 1;
        t.1 += result.is_ok() as u32;

        let (after, undo_after) = self.host.app(|app, _| (app.editor.doc.clone(), app.editor.history.len()));
        let what = format!(
            "{id} {args} -> {}",
            match &result {
                Ok(_) => "ok".to_string(),
                Err(e) => format!("refused: {e}"),
            }
        );
        match &result {
            Err(_) => {
                if after != before {
                    self.flag("refused but changed", id, &format!("{what}, but it changed the document: {}", difference(&before, &after)));
                }
                if undo_after != undo_before {
                    self.flag("refused but undo step", id, &format!("{what}, but the undo history went from {undo_before} to {undo_after} steps"));
                }
                if self.host.app(|app, _| app.editor.is_modified()) != modified_before {
                    self.flag("refused but modified", id, &format!("{what}, but the modified flag changed"));
                }
            }
            Ok(_) if NEW_PROJECT.contains(&id) => {
                self.saved_doc = after.clone();
            }
            Ok(_) if id == "edit.undo" || id == "edit.redo" => {}
            Ok(_) => {
                if id == "project.save_as" {
                    self.saved_doc = after.clone();
                    if let Some(p) = args.get("path").and_then(Value::as_str) {
                        let p = PathBuf::from(p);
                        if !self.saved_files.contains(&p) {
                            self.saved_files.push(p);
                        }
                    }
                }
                if after != before {
                    if undo_after != undo_before + 1 {
                        self.flag(
                            "changed without one undo step",
                            id,
                            &format!(
                                "{what}: the document changed but the undo history went from {undo_before} to {undo_after} steps: {}",
                                difference(&before, &after)
                            ),
                        );
                        self.check_state(&what);
                        return result;
                    }
                    // Undo gives exactly the state before, redo exactly the state after.
                    let (undone, redone, sel_ok) = self.host.app(|app, _| {
                        let sel = app.editor.selection.clone();
                        let modified = app.editor.is_modified();
                        assert!(app.editor.undo());
                        let undone = app.editor.doc.clone();
                        assert!(app.editor.redo());
                        let redone = app.editor.doc.clone();
                        let same = app.editor.selection.ids() == sel.ids() && app.editor.is_modified() == modified;
                        app.editor.selection = sel;
                        (undone, redone, same)
                    });
                    if undone != before {
                        self.fail(&format!("{what}: undo does not give the state before: {}", difference(&before, &undone)));
                    }
                    if redone != after {
                        self.fail(&format!("{what}: redo does not give the state after: {}", difference(&after, &redone)));
                    }
                    if !sel_ok {
                        self.flag("undo redo selection", id, &format!("{what}: undo then redo changed the selection or the modified flag"));
                    }
                } else if undo_after != undo_before {
                    self.flag(
                        "empty undo step",
                        id,
                        &format!("{what}: nothing changed but the undo history went from {undo_before} to {undo_after} steps"),
                    );
                }
            }
        }
        self.check_state(&what);
        if self.trail.len() % self.job_every == 0 {
            self.check_job(&what);
        }
        result
    }

    /// The invariants of the document as it stands.
    fn check_state(&mut self, what: &str) {
        let (doc, selection, current, modified, real, dialogs) = self.host.app(|app, _| {
            (
                app.editor.doc.clone(),
                app.editor.selected(),
                app.editor.current_layer,
                app.editor.is_modified(),
                app.machine.handle.is_real_machine(),
                ::rfd::script_take_log().len(),
            )
        });
        if real {
            self.fail(&format!("{what}: a real machine is connected"));
        }
        if dialogs != 0 {
            self.fail(&format!("{what}: asked for a file dialog"));
        }
        if current >= LAYER_COUNT {
            self.fail(&format!("{what}: the current layer is {current}"));
        }
        // The selection names shapes that exist, once each.
        let mut seen = std::collections::BTreeSet::new();
        for id in &selection {
            if doc.get(*id).is_none() {
                self.fail(&format!("{what}: the selection holds shape {} which does not exist", id.0));
            }
            if !seen.insert(id.0) {
                self.fail(&format!("{what}: the selection holds shape {} twice", id.0));
            }
        }
        // Layers.
        if doc.layers.len() != LAYER_COUNT {
            self.fail(&format!("{what}: {} layers", doc.layers.len()));
        }
        let mut order = doc.layer_order.clone();
        order.sort_unstable();
        if order != (0..LAYER_COUNT).collect::<Vec<_>>() {
            self.fail(&format!("{what}: the layer order is {:?}", doc.layer_order));
        }
        for (i, l) in doc.layers.iter().enumerate() {
            if l.index != i || l.settings.is_empty() || l.settings.len() > darkburn_core::MAX_SUB_LAYERS + 1 {
                self.fail(&format!("{what}: layer {i} has index {} and {} settings", l.index, l.settings.len()));
            }
            let text = without_strings(&format!("{l:?}"));
            if text.contains("NaN") || text.contains("inf") {
                self.fail(&format!("{what}: layer {i} holds a number that is not finite: {text}"));
            }
        }
        // Shapes: ids, links, numbers.
        let mut ids = std::collections::BTreeSet::new();
        let mut problem = None;
        for s in doc.iter() {
            if !ids.insert(s.id.0) {
                problem = Some(format!("two shapes have id {}", s.id.0));
            }
            if s.id.0 == 0 || s.id.0 >= doc.next_id() {
                problem = Some(format!("shape id {} with next id {}", s.id.0, doc.next_id()));
            }
            check_shape(&doc, s, &mut problem);
        }
        if let Some(p) = problem {
            self.fail(&format!("{what}: {p}"));
        }
        // Saved and opened again it is the same document.
        let bytes = match project::save(&doc, "monkey", None) {
            Ok(b) => b,
            Err(e) => self.fail(&format!("{what}: the project does not save: {e}")),
        };
        let loaded = match project::load(&bytes) {
            Ok(l) => l,
            Err(e) => self.fail(&format!("{what}: the saved project does not open: {e}")),
        };
        let mut pruned = doc.clone();
        pruned.prune_images();
        if loaded.doc != pruned {
            self.fail(&format!("{what}: saved and opened again the document differs: {}", difference(&pruned, &loaded.doc)));
        }
        // "Modified" is off exactly when the document is the saved one.
        let same = doc == self.saved_doc;
        if !modified && !same {
            let d = difference(&self.saved_doc, &doc);
            let id = self.trail.last().map(|t| t.id.clone()).unwrap_or_default();
            self.flag("not modified", &id, &format!("{what}: not marked modified, but the document differs from the saved one: {d}"));
        }
    }

    /// The job builds, twice to the same output.
    /// The job built twice with no time limit on the planner gives the same G-code.
    fn same_without_time_budget(&mut self) -> bool {
        self.host.app(|app, _| {
            let mut input = crate::job_cmds::build_input(app);
            input.settings.planner.time_budget_ms = 3_600_000;
            let dev = DeviceGcodeSettings::default();
            let a = crate::job_build::build_job_simple(&input).map(|j| gcode_text(&j, None, &dev)).map_err(|e| e.to_string());
            let b = crate::job_build::build_job_simple(&input).map(|j| gcode_text(&j, None, &dev)).map_err(|e| e.to_string());
            a == b
        })
    }

    fn check_job(&mut self, what: &str) {
        if self.trace {
            // The document the job is built from, for a closer look when the build takes the
            // whole process down.
            let file = self.dir.join("before-job.dbrn");
            let bytes = self.host.app(|app, _| project::save(&app.editor.doc, "monkey", None)).expect("save");
            std::fs::write(&file, bytes).expect("write");
            println!("TRACE seed {} job check ({what}), document in {}", self.seed, file.display());
        }
        let started = Instant::now();
        let built = catch_unwind(AssertUnwindSafe(|| {
            self.host.app(|app, _| {
                let a = crate::job_cmds::build_now(app);
                let b = crate::job_cmds::build_now(app);
                (a, b)
            })
        }));
        let (a, b) = match built {
            Ok(x) => x,
            Err(p) => {
                let text = p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default();
                let shapes = self.host.app(|app, _| {
                    fn leaves(s: &Shape, out: &mut Vec<String>, doc: &Document) {
                        if let ShapeKind::Group { children } = &s.kind {
                            children.iter().for_each(|c| leaves(c, out, doc));
                        }
                        if out.len() < 60 {
                            let l = doc.layer(s.layer).main();
                            out.push(format!(
                                "\n  shape {} {} layer {} ({:?}, interval {}) transform {:?} {}",
                                s.id.0,
                                kind_name(s),
                                s.layer,
                                l.mode,
                                l.interval,
                                s.transform.as_coeffs(),
                                if let ShapeKind::Image(i) = &s.kind { format!("{} x {}", i.width, i.height) } else { String::new() }
                            ));
                        }
                    }
                    let mut out = Vec::new();
                    app.editor.doc.iter().for_each(|s| leaves(s, &mut out, &app.editor.doc));
                    out.concat()
                });
                self.fail(&format!("{what}: building the job panicked: {text}{shapes}"));
            }
        };
        if started.elapsed() > JOB_LIMIT {
            self.fail(&format!("{what}: building the job twice took {:?}", started.elapsed()));
        }
        match (a, b) {
            (Ok(a), Ok(b)) => {
                let dev = DeviceGcodeSettings::default();
                let (ga, gb) = (gcode_text(&a, None, &dev), gcode_text(&b, None, &dev));
                // The planner improves the cut order until its time budget (half a second) runs
                // out, so a large job can come out in two orders: by design (see EXCEPTIONS).
                // With the budget out of the way the same document has to give the same job.
                if ga != gb && self.same_without_time_budget() {
                    self.budget_hits += 1;
                } else if ga != gb {
                    let at = ga.lines().zip(gb.lines()).position(|(x, y)| x != y).unwrap_or(0);
                    let (x, y) = (ga.lines().nth(at).unwrap_or(""), gb.lines().nth(at).unwrap_or(""));
                    self.fail(&format!(
                        "{what}: the job built twice gives two different outputs ({} and {} lines), line {at}: {x} / {y}",
                        ga.lines().count(),
                        gb.lines().count()
                    ));
                }
                if ga.contains("NaN") || ga.contains("inf") {
                    let line = ga.lines().find(|l| l.contains("NaN") || l.contains("inf")).unwrap_or_default();
                    self.fail(&format!("{what}: the job holds a coordinate that is not a number: {line}"));
                }
            }
            (Err(a), Err(b)) => {
                if a != b {
                    self.fail(&format!("{what}: the job built twice fails in two ways: {a} / {b}"));
                }
            }
            (a, b) => self.fail(&format!("{what}: the job built twice: {:?} then {:?}", a.err(), b.err())),
        }
    }
}

/// Links and numbers of one shape and everything in it.
fn check_shape(doc: &Document, s: &Shape, problem: &mut Option<String>) {
    let mut found: Vec<String> = Vec::new();
    let mut bad = |text: String| found.push(format!("shape {} ({}): {text}", s.id.0, kind_name(s)));
    if s.layer >= LAYER_COUNT {
        bad(format!("layer {}", s.layer));
    }
    if s.transform.as_coeffs().iter().any(|c| !c.is_finite()) {
        bad(format!("transform {:?}", s.transform.as_coeffs()));
    }
    let bb = s.bbox();
    if ![bb.x0, bb.y0, bb.x1, bb.y1].iter().all(|c| c.is_finite()) {
        bad(format!("bounding box {bb:?}"));
    }
    if !s.power_scale.is_finite() {
        bad(format!("power scale {}", s.power_scale));
    }
    for k in s.image_keys() {
        if doc.images.get(&k).is_none() {
            bad(format!("picture {k} is not in the project"));
        }
    }
    match &s.kind {
        ShapeKind::Group { children } => {
            if children.is_empty() && s.array.is_none() {
                bad("an empty group".into());
            }
            for c in children {
                check_shape(doc, c, problem);
            }
        }
        ShapeKind::Rect { width, height, radius } => {
            if !(width.is_finite() && height.is_finite() && radius.is_finite()) {
                bad(format!("rectangle {width} x {height} radius {radius}"));
            }
        }
        ShapeKind::Ellipse { rx, ry } => {
            if !(rx.is_finite() && ry.is_finite()) {
                bad(format!("ellipse {rx} x {ry}"));
            }
        }
        ShapeKind::Polygon { sides, radius } => {
            if *sides < 3 || !radius.is_finite() {
                bad(format!("polygon of {sides} sides, radius {radius}"));
            }
        }
        _ => {}
    }
    if let Some(a) = &s.array {
        if a.source.is_empty() {
            bad("an array of nothing".into());
        }
        for c in &a.source {
            for k in c.image_keys() {
                if doc.images.get(&k).is_none() {
                    bad(format!("picture {k} of an array item is not in the project"));
                }
            }
            check_shape(doc, c, problem);
        }
    }
    let text = without_strings(&format!("{:?} {:?} {:?} {:?}", s.tabs, s.start_point, s.extra_start_points, s.kind));
    if text.contains("NaN") || text.contains("inf,") || text.contains("inf)") || text.contains("inf ") {
        let at = text.find("NaN").or_else(|| text.find("inf")).unwrap_or(0);
        let from = at.saturating_sub(80);
        let to = (at + 40).min(text.len());
        bad(format!("holds a number that is not finite: ...{}...", text.get(from..to).unwrap_or("")));
    }
    if problem.is_none() {
        *problem = found.into_iter().next();
    }
}

/// Debug text with every quoted string taken out, so only the numbers are searched for NaN.
fn without_strings(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '"' {
            out.push(c);
            continue;
        }
        out.push_str("\"\"");
        while let Some(c) = chars.next() {
            match c {
                '\\' => {
                    chars.next();
                }
                '"' => break,
                _ => {}
            }
        }
    }
    out
}

fn kind_name(s: &Shape) -> &'static str {
    match &s.kind {
        ShapeKind::Path { .. } => "path",
        ShapeKind::Rect { .. } => "rectangle",
        ShapeKind::Ellipse { .. } => "ellipse",
        ShapeKind::Polygon { .. } => "polygon",
        ShapeKind::Text(_) => "text",
        ShapeKind::Code(_) => "code",
        ShapeKind::Image(_) => "picture",
        ShapeKind::Group { .. } => {
            if s.array.is_some() {
                "virtual array"
            } else {
                "group"
            }
        }
    }
}

/// Where two documents first differ, in words short enough to read.
fn difference(a: &Document, b: &Document) -> String {
    if a.shapes.len() != b.shapes.len() {
        return format!("{} shapes against {}", a.shapes.len(), b.shapes.len());
    }
    for (x, y) in a.shapes.iter().zip(&b.shapes) {
        if x != y {
            return format!("shape {}: {}", x.id.0, text_difference(&format!("{x:?}"), &format!("{y:?}")));
        }
    }
    for (x, y) in a.layers.iter().zip(&b.layers) {
        if x != y {
            return format!("layer {}: {}", x.index, text_difference(&format!("{x:?}"), &format!("{y:?}")));
        }
    }
    if a.layer_order != b.layer_order {
        return format!("layer order {:?} against {:?}", a.layer_order, b.layer_order);
    }
    if a.images != b.images {
        return "the pictures kept in the project".into();
    }
    if a.next_id() != b.next_id() {
        return format!("next id {} against {}", a.next_id(), b.next_id());
    }
    text_difference(&format!("{a:?}"), &format!("{b:?}"))
}

fn text_difference(a: &str, b: &str) -> String {
    let (x, y): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let at = x.iter().zip(&y).position(|(p, q)| p != q).unwrap_or(x.len().min(y.len()));
    let cut = |v: &[char]| v[at.saturating_sub(60)..(at + 60).min(v.len())].iter().collect::<String>();
    format!("...{}... against ...{}...", cut(&x), cut(&y))
}

fn monkey_with(seed: u64, steps: usize, wild: Wild, soft: bool) -> Monkey<Headless> {
    let dir = temp_dir("h");
    let mut m = Monkey::new(headless(&dir), seed, wild, dir.clone());
    m.soft = soft;
    for _ in 0..steps {
        m.step();
    }
    m.check_job("the end");
    let _ = std::fs::remove_dir_all(&dir);
    m
}

// ----- The tests ---------------------------------------------------------------------------------

/// Every registered command is either run by the monkey or left out for a stated reason, and
/// nothing that operates the machine is in the set.
#[test]
fn the_command_set_leaves_out_everything_that_reaches_a_machine() {
    let set = command_set();
    assert!(set.len() > 200, "the monkey runs {} commands", set.len());
    for i in &set {
        assert!(!i.hardware && !commands::hardware(&i.id), "{} operates the machine", i.id);
        for p in ["machine.", "device.", "laser.", "gcode.", "galvo.", "gweike.", "camera.", "help.", "job.start", "job.send", "job.frame"] {
            assert!(!i.id.starts_with(p), "{} is in the monkey's set", i.id);
        }
    }
    for i in commands::list() {
        if i.hardware {
            assert_eq!(excluded(&i.id), Some("operates the machine"), "{}", i.id);
        }
    }
}

/// Wider runs, by hand: `MONKEY_SEEDS=1000..1100 MONKEY_STEPS=2000 cargo test -p darkburn
/// monkey_tests::monkey_explore -- --nocapture`. Does nothing without `MONKEY_SEEDS`.
#[test]
fn monkey_explore() {
    let Ok(range) = std::env::var("MONKEY_SEEDS") else { return };
    let seeds: Vec<u64> = match range.split_once("..") {
        Some((a, b)) => (a.parse::<u64>().unwrap()..b.parse::<u64>().unwrap()).collect(),
        None => range.split(',').map(|s| s.trim().parse::<u64>().expect("MONKEY_SEEDS=from..to or a,b,c")).collect(),
    };
    let steps: usize = std::env::var("MONKEY_STEPS").ok().and_then(|s| s.parse().ok()).unwrap_or(1000);
    let wild = if std::env::var("MONKEY_MILD").is_ok() { Wild::Mild } else { Wild::Full };
    let mut tally: std::collections::BTreeMap<String, (u32, u32)> = Default::default();
    let soft = std::env::var("MONKEY_SOFT").is_ok();
    let mut findings: std::collections::BTreeMap<String, String> = Default::default();
    let mut failed = 0;
    for seed in seeds {
        let t = Instant::now();
        match catch_unwind(|| monkey_with(seed, steps, wild, soft)) {
            Ok(m) => {
                for (k, v) in &m.tally {
                    let e = tally.entry(k.clone()).or_default();
                    e.0 += v.0;
                    e.1 += v.1;
                }
                for (k, v) in m.findings {
                    findings.entry(k).or_insert(v);
                }
                println!("seed {seed}: {steps} steps in {:?}, planner time budget hit {} times", t.elapsed(), m.budget_hits);
            }
            Err(_) => failed += 1,
        }
    }
    for (k, v) in &findings {
        println!("FINDING {k} :: {v}");
    }
    assert_eq!(failed, 0, "seeds failed");
    if std::env::var("MONKEY_TALLY").is_ok() {
        for (k, (ran, ok)) in &tally {
            println!("TALLY {k} ran {ran} accepted {ok}");
        }
    }
}

fn monkey(seed: u64, steps: usize, wild: Wild) -> Monkey<Headless> {
    monkey_with(seed, steps, wild, false)
}

/// Where an invariant of the monkey does not hold by design, and why.
#[allow(dead_code)]
const EXCEPTIONS: &str = "\
1. Building the job twice may give two different orders when the planner runs out of its time \
   budget (500 ms by default): its last improvement step stops at a deadline. The monkey then \
   builds twice more with the budget removed and only fails when those differ. \
2. Building the job has a longer time limit (120 s for two builds) than a command (20 s): after \
   a few arrays of arrays a document holds tens of thousands of shapes and its job honestly \
   takes that long. \
3. file.new, project.open and the other commands that replace the document start a new undo \
   history: undo and redo are not checked across them. \
4. A command that reports success without changing the document adds no undo step (that was \
   bug 4; the monkey accepts both a step that restores exactly and no step at all).";

/// What was run beyond the seeds kept in the suite (all on the headless host).
#[allow(dead_code)]
const EXPLORED: &str = "\
Mild (arguments mostly in range): seeds 1000..1029 x 500 steps, 2006..2029 x 300 steps. \
Full (huge numbers, wrong kinds, missing fields): seeds 3000..3045 x 200..300 steps, 4000..4029 x 300 steps. \
Run more with MONKEY_SEEDS=a..b or a,b,c, MONKEY_STEPS=n, MONKEY_MILD=1 and the test \
monkey_explore; MONKEY_TRACE=1 prints every command.";

#[test]
fn monkey_mild_seeds() {
    for seed in [1, 2, 3] {
        monkey(seed, MILD_STEPS, Wild::Mild);
    }
}

#[test]
fn monkey_wild_seeds() {
    for seed in [101, 102, 103] {
        monkey(seed, WILD_STEPS, Wild::Full);
    }
}

/// The same with a window: every command is followed by a drawn frame, so a state the canvas or
/// a panel cannot draw shows up as a panic.
#[test]
fn monkey_in_the_window() {
    let g = crate::ui_harness::Gui::standard();
    let dir = g.temp("monkey");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let mut m = Monkey::new(g, 201, Wild::Mild, dir);
    m.job_every = 50;
    for _ in 0..WINDOW_STEPS {
        m.step();
    }
}

const MILD_STEPS: usize = 400;
const WILD_STEPS: usize = 400;
const WINDOW_STEPS: usize = 150;

// ----- Regression tests for what the monkey found --------------------------------------------------

/// Monkey seed 3013, step 128: `qr.add` with a module of 1.7e308 mm panicked inside the outline
/// code. A QR code or barcode of an absurd size is refused.
#[test]
fn a_code_of_an_absurd_size_is_refused() {
    let dir = temp_dir("code");
    let mut h = headless(&dir);
    let app = &mut h.app;
    for (id, args) in [
        ("qr.add", json!({ "text": "a", "module": 1.7e308 })),
        ("qr.add", json!({ "text": "a", "size": 1.7e308 })),
        ("qr.add", json!({ "text": "a", "module": 1e9 })),
        ("barcode.add", json!({ "data": "12345", "module": 1.7e308 })),
        ("barcode.add", json!({ "data": "12345", "height": 1.7e308 })),
    ] {
        let before = app.editor.doc.shapes.len();
        let r = crate::api::execute_headless(app, id, &args);
        assert!(r.is_err(), "{id} {args} was accepted: {r:?}");
        assert_eq!(app.editor.doc.shapes.len(), before, "{id} {args} was refused but left a shape");
    }
    // Sizes a person would use still work.
    assert!(crate::api::execute_headless(app, "qr.add", &json!({ "text": "a", "module": 2.0 })).is_ok());
    assert!(crate::api::execute_headless(app, "barcode.add", &json!({ "data": "12345", "module": 0.5, "height": 20 })).is_ok());
    let _ = std::fs::remove_dir_all(&dir);
}

/// Monkey seed 102, step 31: a tab put on a shape that had been squashed to a point had "not a
/// number" as its place, so the document was no longer equal to itself (redo "gave another
/// state"). Such a shape takes no tab.
#[test]
fn a_tab_never_lands_on_not_a_number() {
    let dir = temp_dir("tab");
    let mut h = headless(&dir);
    let app = &mut h.app;
    let r = crate::api::execute_headless(app, "shape.add_rect", &json!({ "x": 10, "y": 10, "width": 40, "height": 25 })).unwrap();
    let id = darkburn_core::ShapeId(r["id"].as_u64().unwrap());
    assert!(app.editor.edit("Squash", |d, _| {
        d.get_mut(id).unwrap().transform = Affine::new([0.0, 0.0, 0.0, 0.0, 0.0, 400.0]);
        true
    }));
    let before = app.editor.history.len();
    let r = crate::api::execute_headless(app, "tabs.add", &json!({ "shape": id.0, "x": 117.9, "y": 138.5 }));
    let tabs = app.editor.doc.get(id).unwrap().tabs.clone();
    assert!(tabs.iter().all(|t| t[0].is_finite() && t[1].is_finite()), "tabs {tabs:?} after {r:?}");
    assert!(r.is_err(), "{r:?}");
    assert_eq!(app.editor.history.len(), before);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Monkey seed 4004, step 73: `tools.offset` with a tolerance of -3.8 took 50 s (and 735 s on
/// the document of all kinds of objects, against 5 ms with the usual tolerance): a tolerance
/// below zero was read as the finest one there is, a millionth of a millimetre.
#[test]
fn an_offset_with_a_tolerance_below_zero_does_not_take_minutes() {
    let dir = temp_dir("off");
    let mut h = headless(&dir);
    let app = &mut h.app;
    seed_document(app);
    crate::api::execute_headless(app, "edit.select_all", &json!({})).unwrap();
    let t = Instant::now();
    let r = crate::api::execute_headless(app, "tools.offset", &json!({ "distance": 5.0, "tolerance": -3.8, "side": "outward" }));
    assert!(r.is_ok(), "{r:?}");
    assert!(t.elapsed() < Duration::from_secs(5), "the offset took {:?}", t.elapsed());
    // And the finest tolerance that can be asked for is still quick.
    crate::api::execute_headless(app, "edit.undo", &json!({})).unwrap();
    let t = Instant::now();
    let r = crate::api::execute_headless(app, "tools.offset", &json!({ "distance": 5.0, "tolerance": 1e-9, "side": "outward" }));
    assert!(r.is_ok(), "{r:?}");
    assert!(t.elapsed() < Duration::from_secs(5), "the finest offset took {:?}", t.elapsed());
    let _ = std::fs::remove_dir_all(&dir);
}
