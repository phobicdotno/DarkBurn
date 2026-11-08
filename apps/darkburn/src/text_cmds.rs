//! Text commands: text shapes, text on a path, QR codes, barcodes and variable text. Every one is
//! a registered command that runs without a window; `text_ui.rs` (the text tool, the Text
//! Options toolbar, the dialogs and the Variable Text panel) calls these.

use darkburn_core::kurbo::{Affine, BezPath, Point};
use darkburn_core::shape::{AlignX, AlignY, VariableText};
use darkburn_core::{CodeShape, CodeSpec, Document, PathText, Shape, ShapeId, ShapeKind, TextShape};
use darkburn_text::{BarcodeKind, ContactCard, EccLevel, EvalContext, MergeSource, QrContent, WifiSecurity};
use serde_json::{Value, json};

use crate::commands::Cmd;
use crate::shell::DarkBurn;
use crate::text_render as render;
use crate::tools::Tool;

const fn cmd(id: &'static str, label: &'static str, args: &'static str) -> Cmd {
    Cmd { id, label, key: "", icon: "", args }
}

const TEXT_PROPS: &str = r#""text":{"type":"string"},"font":{"type":"string","description":"a font family, or shx:<path of an .shx file>"},"height":{"type":"number","description":"capital height, mm"},"bold":{"type":"boolean"},"italic":{"type":"boolean"},"upper_case":{"type":"boolean"},"welded":{"type":"boolean"},"distort":{"type":"boolean"},"letter_spacing":{"type":"number"},"line_spacing":{"type":"number"},"align_x":{"enum":["left","middle","right"]},"align_y":{"enum":["baseline","top","middle","bottom"]},"variable":{"enum":["normal","date_time","merge","serial","cut_setting"]},"offset":{"type":"integer"}"#;

const QR_PROPS: &str = r#""kind":{"enum":["text","url","wifi","contact"]},"text":{"type":"string"},"url":{"type":"string"},"ssid":{"type":"string"},"password":{"type":"string"},"security":{"enum":["wpa","wep","open"]},"hidden":{"type":"boolean"},"name":{"type":"string"},"phone":{"type":"string"},"email":{"type":"string"},"website":{"type":"string"},"address":{"type":"string"},"note":{"type":"string"},"ecc":{"enum":["L","M","Q","H"]},"size":{"type":"number","description":"side of the symbol, mm"},"module":{"type":"number","description":"side of one module, mm"},"variable":{"type":"boolean","description":"the data holds {serial}, {csv:1} and similar tokens"}"#;

const BARCODE_PROPS: &str = r#""barcode":{"enum":["code128","code39","ean13","upca","itf","datamatrix"]},"data":{"type":"string"},"module":{"type":"number","description":"narrow bar width, mm"},"height":{"type":"number","description":"bar height, mm"},"variable":{"type":"boolean"}"#;

/// The commands of this area. `commands::registry` adds them to the registry.
pub const COMMANDS: &[Cmd] = &[
    cmd("text.font_picker", "Choose a Font", ""),
    cmd("text.add_shx", "Add an SHX Font", r#"{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}"#),
    cmd(
        "text.add",
        "Add Text",
        r#"{"type":"object","properties":{"x":{"type":"number"},"y":{"type":"number"},"layer":{"type":"integer"},TEXT},"required":["text"]}"#,
    ),
    cmd("text.set", "Change Text", r#"{"type":"object","properties":{"ids":{"type":"array","items":{"type":"integer"}},TEXT}}"#),
    cmd(
        "text.on_path",
        "Put Text on a Path",
        r#"{"type":"object","properties":{"text_id":{"type":"integer"},"path_id":{"type":"integer"},"start_offset":{"type":"number"},"offset":{"type":"number"},"center":{"type":"boolean"},"flip":{"type":"boolean"},"distort":{"type":"boolean"}}}"#,
    ),
    cmd("text.detach_path", "Take Text off its Path", ""),
    cmd("text.fonts", "List Fonts", r#"{"type":"object","properties":{"query":{"type":"string"}}}"#),
    cmd(
        "text.font_defaults",
        "Default Font and Size",
        r#"{"type":"object","properties":{"font":{"type":"string"},"size":{"type":"number"},"favourite":{"type":"string","description":"toggle a family in the favourites"}}}"#,
    ),
    cmd("qr.add", "Add a QR Code", r#"{"type":"object","properties":{"x":{"type":"number"},"y":{"type":"number"},"layer":{"type":"integer"},QR}}"#),
    cmd("qr.set", "Change a QR Code", r#"{"type":"object","properties":{"ids":{"type":"array","items":{"type":"integer"}},QR}}"#),
    cmd(
        "barcode.add",
        "Add a Barcode",
        r#"{"type":"object","properties":{"x":{"type":"number"},"y":{"type":"number"},"layer":{"type":"integer"},BARCODE},"required":["data"]}"#,
    ),
    cmd("barcode.set", "Change a Barcode", r#"{"type":"object","properties":{"ids":{"type":"array","items":{"type":"integer"}},BARCODE}}"#),
    cmd("variable.get", "Variable Text State", ""),
    cmd(
        "variable.set",
        "Change Variable Text Settings",
        r#"{"type":"object","properties":{"start":{"type":"integer"},"end":{"type":"integer"},"current":{"type":"integer"},"advance_by":{"type":"integer"},"padding":{"type":"integer"},"auto_advance":{"type":"boolean"}}}"#,
    ),
    cmd("variable.next", "Next Variable Text", ""),
    cmd("variable.previous", "Previous Variable Text", ""),
    cmd("variable.reset", "Reset Variable Text", ""),
    cmd("variable.test", "Test Variable Text", r#"{"type":"object","properties":{"template":{"type":"string"},"count":{"type":"integer"}}}"#),
    cmd(
        "variable.bake",
        "Bake Variable Text",
        r#"{"type":"object","properties":{"all":{"type":"boolean","description":"every variable shape, not just the selection"}}}"#,
    ),
    cmd(
        "variable.csv_load",
        "Load a CSV File",
        r#"{"type":"object","properties":{"path":{"type":"string"},"header":{"type":"boolean"}},"required":["path"]}"#,
    ),
    cmd("variable.csv_clear", "Clear the CSV File", ""),
    cmd("variable.after_job", "Variable Text After a Job", ""),
];

/// The commands of this area with their argument schemas filled in.
pub fn commands() -> &'static [Cmd] {
    static ALL: std::sync::OnceLock<Vec<Cmd>> = std::sync::OnceLock::new();
    ALL.get_or_init(|| {
        COMMANDS
            .iter()
            .map(|c| {
                let args: &'static str = if c.args.is_empty() {
                    ""
                } else {
                    Box::leak(c.args.replace("TEXT", TEXT_PROPS).replace("QR", QR_PROPS).replace("BARCODE", BARCODE_PROPS).into_boxed_str())
                };
                Cmd { args, ..*c }
            })
            .collect()
    })
}

/// The commands that run in this build.
pub fn works(id: &str) -> bool {
    id.starts_with("text.")
        || id.starts_with("qr.")
        || id.starts_with("barcode.")
        || id.starts_with("variable.")
        || matches!(id, "tools.text" | "tools.qr" | "tools.path_to_text")
}

/// The checklist items a command implements.
pub fn checklist(id: &str) -> &'static [&'static str] {
    match id {
        "tools.text" => &["TOOLS-08", "DRAW-08", "DRAW-09", "DRAW-12", "DRAW-13"],
        "text.add" | "text.set" => &["TOOLS-08", "DRAW-08", "DRAW-12", "DRAW-13", "TB-17", "TB-18", "SP-04"],
        "text.edit_dialog" => &["DRAW-09", "DRAW-10"],
        "text.font_picker" | "text.fonts" | "text.font_defaults" | "text.add_shx" => &["TB-16", "DRAW-13", "DRAW-14"],
        "tools.path_to_text" | "text.on_path" | "text.detach_path" => &["TOOLS-21", "SHAPE-28", "DRAW-11"],
        "tools.qr" | "qr.add" | "qr.set" => &["TOOLS-11", "DRAW-16", "DRAW-17"],
        "barcode.add" | "barcode.set" => &["DRAW-21"],
        "variable.get" | "variable.set" | "variable.next" | "variable.previous" | "variable.reset" | "variable.test" | "variable.bake" => {
            &["VT-01", "VT-02", "VT-04", "VTP-01", "VTP-02", "VTP-04", "TB-19"]
        }
        "variable.csv_load" | "variable.csv_clear" => &["VT-03", "VTP-05", "AUX-10"],
        "variable.after_job" => &["VT-05", "VTP-03"],
        _ => &[],
    }
}

// ---------------------------------------------------------------- argument helpers

fn num(a: &Value, k: &str) -> Option<f64> {
    a.get(k).and_then(Value::as_f64).filter(|v| v.is_finite())
}

fn int(a: &Value, k: &str) -> Option<i64> {
    a.get(k).and_then(|v| v.as_i64().or_else(|| v.as_f64().filter(|f| f.is_finite()).map(|f| f.round() as i64)))
}

fn flag(a: &Value, k: &str) -> Option<bool> {
    a.get(k).and_then(Value::as_bool)
}

fn word<'a>(a: &'a Value, k: &str) -> Option<&'a str> {
    a.get(k).and_then(Value::as_str)
}

fn layer_arg(app: &DarkBurn, a: &Value) -> Result<usize, String> {
    match a.get("layer").and_then(Value::as_u64) {
        Some(l) if l < darkburn_core::LAYER_COUNT as u64 => Ok(l as usize),
        Some(l) => Err(format!("layer {l} does not exist (0 to 31)")),
        None => Ok(app.editor.current_layer),
    }
}

fn ids_arg(app: &DarkBurn, a: &Value) -> Vec<ShapeId> {
    match a.get("ids").and_then(Value::as_array) {
        Some(v) => v.iter().filter_map(Value::as_u64).map(ShapeId).collect(),
        None => app.editor.selected(),
    }
}

fn ok_changed(changed: bool) -> Result<Value, String> {
    Ok(json!({ "changed": changed }))
}

/// Apply the text settings found in `a` to `t`.
pub fn apply_text_args(t: &mut TextShape, a: &Value) -> Result<(), String> {
    if let Some(s) = word(a, "text") {
        t.text = s.to_string();
    }
    if let Some(s) = word(a, "font") {
        t.font = s.to_string();
    }
    if let Some(h) = num(a, "height") {
        if h <= 0.0 {
            return Err("height must be above zero".into());
        }
        t.height = h;
    }
    if let Some(v) = flag(a, "bold") {
        t.bold = v;
    }
    if let Some(v) = flag(a, "italic") {
        t.italic = v;
    }
    if let Some(v) = flag(a, "upper_case") {
        t.upper_case = v;
    }
    if let Some(v) = flag(a, "welded") {
        t.welded = v;
    }
    if let Some(v) = flag(a, "distort") {
        t.distort = v;
    }
    if let Some(v) = num(a, "letter_spacing") {
        t.letter_spacing = v;
    }
    if let Some(v) = num(a, "line_spacing") {
        t.line_spacing = v;
    }
    if let Some(s) = word(a, "align_x") {
        t.align_x = match s {
            "left" => AlignX::Left,
            "middle" | "centre" | "center" => AlignX::Middle,
            "right" => AlignX::Right,
            o => return Err(format!("unknown align_x {o}")),
        };
    }
    if let Some(s) = word(a, "align_y") {
        t.align_y = match s {
            "baseline" => AlignY::Baseline,
            "top" => AlignY::Top,
            "middle" | "centre" | "center" => AlignY::Middle,
            "bottom" => AlignY::Bottom,
            o => return Err(format!("unknown align_y {o}")),
        };
    }
    if let Some(s) = word(a, "variable") {
        t.variable = variable_kind(s)?;
    }
    if let Some(v) = int(a, "offset") {
        t.offset = v;
    }
    Ok(())
}

pub fn variable_kind(s: &str) -> Result<VariableText, String> {
    Ok(match s {
        "normal" => VariableText::Normal,
        "date_time" | "datetime" => VariableText::DateTime,
        "merge" => VariableText::Merge,
        "serial" => VariableText::Serial,
        "cut_setting" => VariableText::CutSetting,
        o => return Err(format!("unknown variable text type {o}")),
    })
}

fn qr_content(a: &Value, current: Option<&QrContent>) -> Result<Option<QrContent>, String> {
    let kind = word(a, "kind");
    let any = ["text", "url", "ssid", "password", "security", "hidden", "name", "phone", "email", "website", "address", "note"]
        .iter()
        .any(|k| a.get(k).is_some());
    if kind.is_none() && !any {
        return Ok(None);
    }
    let s = |k: &str| word(a, k).map(str::to_string);
    let kind = kind.unwrap_or(match current {
        Some(QrContent::Url(_)) => "url",
        Some(QrContent::Wifi { .. }) => "wifi",
        Some(QrContent::Contact(_)) => "contact",
        _ => "text",
    });
    Ok(Some(match kind {
        "text" => {
            QrContent::Text(s("text").or_else(|| if let Some(QrContent::Text(t)) = current { Some(t.clone()) } else { None }).unwrap_or_default())
        }
        "url" => QrContent::Url(s("url").or_else(|| if let Some(QrContent::Url(t)) = current { Some(t.clone()) } else { None }).unwrap_or_default()),
        "wifi" => {
            let (mut ssid, mut password, mut security, mut hidden) = match current {
                Some(QrContent::Wifi { ssid, password, security, hidden }) => (ssid.clone(), password.clone(), *security, *hidden),
                _ => (String::new(), String::new(), WifiSecurity::Wpa, false),
            };
            if let Some(v) = s("ssid") {
                ssid = v;
            }
            if let Some(v) = s("password") {
                password = v;
            }
            if let Some(v) = word(a, "security") {
                security = match v {
                    "wpa" => WifiSecurity::Wpa,
                    "wep" => WifiSecurity::Wep,
                    "open" | "none" => WifiSecurity::Open,
                    o => return Err(format!("unknown security {o}")),
                };
            }
            if let Some(v) = flag(a, "hidden") {
                hidden = v;
            }
            QrContent::Wifi { ssid, password, security, hidden }
        }
        "contact" => {
            let mut c = match current {
                Some(QrContent::Contact(c)) => c.clone(),
                _ => ContactCard::default(),
            };
            for (k, f) in [
                ("name", &mut c.name),
                ("phone", &mut c.phone),
                ("email", &mut c.email),
                ("website", &mut c.url),
                ("address", &mut c.address),
                ("note", &mut c.note),
            ] {
                if let Some(v) = word(a, k) {
                    *f = v.to_string();
                }
            }
            QrContent::Contact(c)
        }
        o => return Err(format!("unknown QR kind {o}")),
    }))
}

fn apply_qr_args(doc: &Document, layer: usize, c: &mut CodeShape, a: &Value) -> Result<(), String> {
    let CodeSpec::Qr(q) = &mut c.spec else { return Err("not a QR code".into()) };
    if let Some(content) = qr_content(a, Some(&q.content))? {
        q.content = content;
    }
    if let Some(e) = word(a, "ecc") {
        q.ecc = match e.to_ascii_uppercase().as_str() {
            "L" => EccLevel::L,
            "M" => EccLevel::M,
            "Q" => EccLevel::Q,
            "H" => EccLevel::H,
            o => return Err(format!("unknown error correction level {o}")),
        };
    }
    if let Some(m) = num(a, "module") {
        if m <= 0.0 {
            return Err("module must be above zero".into());
        }
        q.module_mm = m;
    }
    if let Some(v) = flag(a, "variable") {
        c.variable = v;
    }
    if let Some(size) = num(a, "size") {
        if size <= 0.0 {
            return Err("size must be above zero".into());
        }
        render::set_code_size(doc, layer, c, size);
    }
    Ok(())
}

fn barcode_kind(s: &str) -> Result<BarcodeKind, String> {
    Ok(match s.to_ascii_lowercase().replace(['-', '_', ' '], "").as_str() {
        "code128" => BarcodeKind::Code128,
        "code39" => BarcodeKind::Code39,
        "ean13" => BarcodeKind::Ean13,
        "upca" | "upc" => BarcodeKind::UpcA,
        "itf" => BarcodeKind::Itf,
        "datamatrix" => BarcodeKind::DataMatrix,
        o => return Err(format!("unknown barcode {o}")),
    })
}

fn apply_barcode_args(c: &mut CodeShape, a: &Value) -> Result<(), String> {
    let CodeSpec::Barcode(b) = &mut c.spec else { return Err("not a barcode".into()) };
    if let Some(k) = word(a, "barcode") {
        b.kind = barcode_kind(k)?;
    }
    if let Some(d) = word(a, "data") {
        b.data = d.to_string();
    }
    if let Some(m) = num(a, "module") {
        if m <= 0.0 {
            return Err("module must be above zero".into());
        }
        b.module_mm = m;
    }
    if let Some(h) = num(a, "height") {
        b.height_mm = h.max(0.1);
    }
    if let Some(v) = flag(a, "variable") {
        c.variable = v;
    }
    Ok(())
}

// ---------------------------------------------------------------- shape access

/// Run `f` on the text shape `id` (also inside groups and arrays' sources).
fn with_shape(doc: &mut Document, id: ShapeId, f: &mut dyn FnMut(&mut Shape) -> bool) -> bool {
    fn walk(s: &mut Shape, id: ShapeId, f: &mut dyn FnMut(&mut Shape) -> bool) -> Option<bool> {
        if s.id == id {
            return Some(f(s));
        }
        if let ShapeKind::Group { children } = &mut s.kind {
            for c in children {
                if let Some(r) = walk(c, id, f) {
                    return Some(r);
                }
            }
        }
        None
    }
    let Some(top) = doc.iter().find(|s| s.id == id || contains_id(s, id)).map(|s| s.id) else { return false };
    doc.get_mut(top).and_then(|s| walk(s, id, f)).unwrap_or(false)
}

fn contains_id(s: &Shape, id: ShapeId) -> bool {
    match &s.kind {
        ShapeKind::Group { children } => children.iter().any(|c| c.id == id || contains_id(c, id)),
        _ => false,
    }
}

/// The text shape `id`, also inside groups.
pub fn find_text(doc: &Document, id: ShapeId) -> Option<&TextShape> {
    fn walk(s: &Shape, id: ShapeId) -> Option<&TextShape> {
        match &s.kind {
            ShapeKind::Text(t) if s.id == id => Some(t),
            ShapeKind::Group { children } => children.iter().find_map(|c| walk(c, id)),
            _ => None,
        }
    }
    doc.iter().find_map(|s| walk(s, id))
}

fn is_text(s: &Shape) -> bool {
    matches!(s.kind, ShapeKind::Text(_))
}

/// The ids of the selected text shapes (top level and inside groups).
pub fn selected_text(app: &DarkBurn, ids: &[ShapeId]) -> Vec<ShapeId> {
    fn collect(s: &Shape, out: &mut Vec<ShapeId>) {
        match &s.kind {
            ShapeKind::Text(_) => out.push(s.id),
            ShapeKind::Group { children } if s.array.is_none() => children.iter().for_each(|c| collect(c, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for id in ids {
        if let Some(s) = app.editor.doc.get(*id) {
            collect(s, &mut out);
        }
    }
    out
}

/// Add a text shape; the new shape is selected.
pub fn add_text(app: &mut DarkBurn, at: Point, layer: usize, a: &Value) -> Result<ShapeId, String> {
    let mut t = TextShape { font: default_font(app), height: app.prefs.fonts.default_size.max(0.1), ..Default::default() };
    apply_text_args(&mut t, a)?;
    app.prefs.fonts.touch(&t.font);
    let shape = Shape::new(ShapeKind::Text(t), Affine::translate(at.to_vec2()), layer);
    let mut id = ShapeId(0);
    app.editor.edit("Add text", |d, sel| {
        id = d.add(shape);
        sel.set([id]);
        render::refresh(d);
        true
    });
    Ok(id)
}

/// The font a new text object gets.
pub fn default_font(app: &DarkBurn) -> String {
    app.prefs.fonts.family_for_new_text(&render::lib())
}

fn text_summary(app: &DarkBurn, id: ShapeId) -> Value {
    let bb = app.editor.doc.get(id).map(|s| s.bbox());
    json!({ "id": id.0, "bbox": bb.map(|b| [b.x0, b.y0, b.x1, b.y1]) })
}

// ---------------------------------------------------------------- the path text

fn path_for_text(text: &Shape, path: &Shape) -> BezPath {
    // The path in the text's own coordinates.
    let to_text = text.transform.inverse() * path.transform;
    let mut p = path.local_outline();
    p.apply_affine(to_text);
    p
}

fn on_path(app: &mut DarkBurn, a: &Value) -> Result<Value, String> {
    let sel = app.editor.selected();
    let mut text_id = int(a, "text_id").map(|i| ShapeId(i as u64));
    let mut path_id = int(a, "path_id").map(|i| ShapeId(i as u64));
    for id in &sel {
        let Some(s) = app.editor.doc.get(*id) else { continue };
        if is_text(s) && text_id.is_none() {
            text_id = Some(*id);
        } else if !is_text(s) && !s.is_image() && path_id.is_none() {
            path_id = Some(*id);
        }
    }
    let (tid, pid) = (text_id.ok_or("select a text shape and a path")?, path_id.ok_or("select a text shape and a path")?);
    let (ts, ps) = (app.editor.doc.get(tid).cloned().ok_or("no such text")?, app.editor.doc.get(pid).cloned().ok_or("no such path")?);
    if !is_text(&ts) {
        return Err("text_id is not a text shape".into());
    }
    if ps.local_outline().elements().is_empty() {
        return Err("the path is empty".into());
    }
    let path = path_for_text(&ts, &ps);
    let pt = PathText {
        path,
        start_offset: num(a, "start_offset").unwrap_or(0.0),
        offset: num(a, "offset").unwrap_or(0.0),
        center_on_path: flag(a, "center").unwrap_or(false),
        flip: flag(a, "flip").unwrap_or(false),
    };
    let distort = flag(a, "distort");
    let changed = app.editor.edit("Text on path", |d, _| {
        d.get_mut(tid).is_some_and(|s| {
            if let ShapeKind::Text(t) = &mut s.kind {
                t.on_path = Some(pt);
                if let Some(v) = distort {
                    t.distort = v;
                }
            }
            true
        }) && {
            render::refresh(d);
            true
        }
    });
    Ok(json!({ "changed": changed, "text": tid.0, "path": pid.0 }))
}

// ---------------------------------------------------------------- variable text

fn state_json(doc: &Document) -> Value {
    let v = &doc.variables;
    json!({
        "start": v.serial.start, "end": v.serial.end, "current": v.serial.current, "advance_by": v.serial.advance_by,
        "padding": v.serial.padding, "auto_advance": v.auto_advance,
        "csv": { "path": v.merge.path, "header": v.merge.header, "rows": v.merge.row_count(), "current_row": v.merge.current_row, "columns": v.merge.column_names() },
    })
}

fn edit_variables(app: &mut DarkBurn, label: &str, f: impl FnOnce(&mut Document) -> Result<(), String>) -> Result<Value, String> {
    let mut err = None;
    let changed = app.editor.edit(label, |d, _| {
        let before = d.variables.clone();
        if let Err(e) = f(d) {
            err = Some(e);
            return false;
        }
        render::refresh(d);
        d.variables != before || true
    });
    match err {
        Some(e) => Err(e),
        None => {
            let mut v = state_json(&app.editor.doc);
            v["changed"] = json!(changed);
            Ok(v)
        }
    }
}

/// After a job is sent: advance to the next serial number or CSV row when auto-advance is on and
/// the job contains variable text. The machine side calls this once the job went out.
pub fn after_job(app: &mut DarkBurn) -> bool {
    let doc = &app.editor.doc;
    if !doc.variables.auto_advance || !render::has_variable_text(doc) {
        return false;
    }
    app.editor.edit("Advance variable text", |d, _| {
        d.variables = d.variables.after_job(true);
        render::refresh(d);
        true
    })
}

fn bake(app: &mut DarkBurn, a: &Value) -> Result<Value, String> {
    let all = flag(a, "all").unwrap_or(false);
    let ids: Vec<ShapeId> = if all { app.editor.doc.iter().map(|s| s.id).collect() } else { ids_arg(app, a) };
    if ids.is_empty() {
        return Err("nothing is selected".into());
    }
    let now = EvalContext::local_now();
    let mut n = 0usize;
    let changed = app.editor.edit("Bake variable text", |d, _| {
        let reference = d.clone();
        fn walk(reference: &Document, s: &mut Shape, now: &EvalContext, n: &mut usize) {
            let before = s.kind.clone();
            render::bake_text(reference, s, now);
            if s.kind != before {
                *n += 1;
            }
            if let ShapeKind::Group { children } = &mut s.kind {
                for c in children {
                    walk(reference, c, now, n);
                }
            }
            if let Some(a) = &mut s.array {
                for c in &mut a.source {
                    walk(reference, c, now, n);
                }
            }
        }
        for id in &ids {
            if let Some(s) = d.get_mut(*id) {
                walk(&reference, s, &now, &mut n);
            }
        }
        if n > 0 {
            render::refresh(d);
        }
        n > 0
    });
    Ok(json!({ "changed": changed, "baked": n }))
}

// ---------------------------------------------------------------- the commands

/// Run a command with arguments; None when `id` is not one of this area's.
pub fn execute(app: &mut DarkBurn, id: &str, a: &Value) -> Option<Result<Value, String>> {
    let a = if a.is_null() { &json!({}) } else { a };
    Some(match id {
        "text.add" => (|| {
            if word(a, "text").is_none() {
                return Err("argument text must be text".into());
            }
            let at = Point::new(num(a, "x").unwrap_or(0.0), num(a, "y").unwrap_or(0.0));
            let layer = layer_arg(app, a)?;
            let id = add_text(app, at, layer, a)?;
            Ok(text_summary(app, id))
        })(),
        "text.set" => (|| {
            let ids = selected_text(app, &ids_arg(app, a));
            if ids.is_empty() {
                return Err("select a text shape first".into());
            }
            let mut err = None;
            let font = word(a, "font").map(str::to_string);
            let changed = app.editor.edit("Change text", |d, _| {
                let mut any = false;
                for id in &ids {
                    any |= with_shape(d, *id, &mut |s| match &mut s.kind {
                        ShapeKind::Text(t) => match apply_text_args(t, a) {
                            Ok(()) => true,
                            Err(e) => {
                                err = Some(e);
                                false
                            }
                        },
                        _ => false,
                    });
                }
                if err.is_none() {
                    render::refresh(d);
                }
                any && err.is_none()
            });
            if let Some(e) = err {
                return Err(e);
            }
            if let Some(f) = font {
                app.prefs.fonts.touch(&f);
            }
            Ok(json!({ "changed": changed, "ids": ids.iter().map(|i| i.0).collect::<Vec<_>>() }))
        })(),
        "text.on_path" | "tools.path_to_text" => on_path(app, a),
        "text.detach_path" => {
            let ids = selected_text(app, &ids_arg(app, a));
            let changed = app.editor.edit("Take text off its path", |d, _| {
                let mut any = false;
                for id in &ids {
                    any |= with_shape(d, *id, &mut |s| match &mut s.kind {
                        ShapeKind::Text(t) if t.on_path.is_some() => {
                            t.on_path = None;
                            true
                        }
                        _ => false,
                    });
                }
                render::refresh(d);
                any
            });
            ok_changed(changed)
        }
        "text.fonts" => {
            let lib = render::lib();
            let installed = darkburn_text::list_families(&lib);
            let l = app.prefs.fonts.listing(&installed, word(a, "query").unwrap_or(""), true);
            Ok(json!({ "favourites": l.favourites, "recent": l.recent, "all": l.all, "default": app.prefs.fonts.family_for_new_text(&lib) }))
        }
        "text.font_defaults" => {
            if let Some(f) = word(a, "font") {
                app.prefs.fonts.default_family = f.to_string();
            }
            if let Some(s) = num(a, "size") {
                app.prefs.fonts.default_size = s.clamp(0.1, 10_000.0);
            }
            if let Some(f) = word(a, "favourite") {
                app.prefs.fonts.toggle_favourite(f);
            }
            let f = &app.prefs.fonts;
            Ok(json!({ "font": f.default_family, "size": f.default_size, "favourites": f.favourites, "recent": f.recent }))
        }
        "text.add_shx" => (|| {
            let path = word(a, "path").ok_or("argument path must be text")?;
            render::forget_shx(path);
            let f = render::shx_font(path).ok_or_else(|| format!("{path} is not a readable SHX font"))?;
            let name = format!("{}{path}", render::SHX_PREFIX);
            if !app.prefs.shx_fonts.contains(&name) {
                app.prefs.shx_fonts.push(name.clone());
            }
            Ok(json!({ "font": name, "name": f.name, "glyphs": f.glyph_count() }))
        })(),
        "qr.add" => (|| {
            let at = Point::new(num(a, "x").unwrap_or(0.0), num(a, "y").unwrap_or(0.0));
            let layer = layer_arg(app, a)?;
            let mut c = CodeShape { spec: CodeSpec::Qr(Default::default()), ..Default::default() };
            apply_qr_args(&app.editor.doc, layer, &mut c, a)?;
            add_code(app, "Add QR code", c, at, layer)
        })(),
        "barcode.add" => (|| {
            let at = Point::new(num(a, "x").unwrap_or(0.0), num(a, "y").unwrap_or(0.0));
            let layer = layer_arg(app, a)?;
            let mut c = CodeShape { spec: CodeSpec::Barcode(Default::default()), ..Default::default() };
            apply_barcode_args(&mut c, a)?;
            add_code(app, "Add barcode", c, at, layer)
        })(),
        "qr.set" | "barcode.set" => (|| {
            let want_qr = id == "qr.set";
            let ids: Vec<ShapeId> = ids_arg(app, a)
                .into_iter()
                .filter(|i| {
                    app.editor.doc.get(*i).is_some_and(|s| matches!(&s.kind, ShapeKind::Code(c) if matches!(c.spec, CodeSpec::Qr(_)) == want_qr))
                })
                .collect();
            if ids.is_empty() {
                return Err(if want_qr { "select a QR code first" } else { "select a barcode first" }.into());
            }
            let mut err = None;
            let reference = app.editor.doc.clone();
            let changed = app.editor.edit("Change code", |d, _| {
                for id in &ids {
                    with_shape(d, *id, &mut |s| {
                        let layer = s.layer;
                        if let ShapeKind::Code(c) = &mut s.kind {
                            let r = if want_qr { apply_qr_args(&reference, layer, c, a) } else { apply_barcode_args(c, a) };
                            if let Err(e) = r {
                                err = Some(e);
                            }
                        }
                        true
                    });
                }
                render::refresh(d);
                err.is_none()
            });
            match err {
                Some(e) => Err(e),
                None => Ok(json!({ "changed": changed, "errors": code_errors(app, &ids) })),
            }
        })(),
        "variable.get" => Ok(state_json(&app.editor.doc)),
        "variable.set" => edit_variables(app, "Variable text settings", |d| {
            let s = &mut d.variables.serial;
            if let Some(v) = int(a, "start") {
                s.start = v;
            }
            if let Some(v) = int(a, "end") {
                s.end = v;
            }
            if let Some(v) = int(a, "current") {
                s.current = v;
            }
            if let Some(v) = int(a, "advance_by") {
                s.advance_by = v;
            }
            if let Some(v) = int(a, "padding") {
                s.padding = v.clamp(0, 64) as usize;
            }
            if let Some(v) = flag(a, "auto_advance") {
                d.variables.auto_advance = v;
            }
            Ok(())
        }),
        "variable.next" => edit_variables(app, "Next variable text", |d| {
            d.variables = d.variables.next();
            Ok(())
        }),
        "variable.previous" => edit_variables(app, "Previous variable text", |d| {
            d.variables = d.variables.previous();
            Ok(())
        }),
        "variable.reset" => edit_variables(app, "Reset variable text", |d| {
            d.variables = d.variables.reset();
            Ok(())
        }),
        "variable.test" => (|| {
            let count = int(a, "count").unwrap_or(5).clamp(1, 1000) as usize;
            let template = match word(a, "template") {
                Some(t) => t.to_string(),
                None => {
                    let ids = selected_text(app, &app.editor.selected());
                    ids.first()
                        .and_then(|i| find_text(&app.editor.doc, *i))
                        .map(|t| t.text.clone())
                        .ok_or("give a template, or select variable text")?
                }
            };
            let ctx = EvalContext::local_now();
            Ok(json!({ "values": app.editor.doc.variables.test(&template, &ctx, count) }))
        })(),
        "variable.bake" => bake(app, a),
        "variable.csv_load" => (|| {
            let path = word(a, "path").ok_or("argument path must be text")?.to_string();
            let header = flag(a, "header").unwrap_or(app.editor.doc.variables.merge.header);
            let src = MergeSource::load(&path, header).map_err(|e| e.to_string())?;
            edit_variables(app, "Load CSV file", |d| {
                d.variables.merge = src;
                Ok(())
            })
        })(),
        "variable.csv_clear" => edit_variables(app, "Clear CSV file", |d| {
            d.variables.merge = d.variables.merge.cleared();
            Ok(())
        }),
        "variable.after_job" => Ok(json!({ "advanced": after_job(app) })),
        _ => return None,
    })
}

fn code_errors(app: &DarkBurn, ids: &[ShapeId]) -> Vec<String> {
    ids.iter().filter_map(|i| app.editor.doc.get(*i)).filter_map(|s| if let ShapeKind::Code(c) = &s.kind { c.error.clone() } else { None }).collect()
}

fn add_code(app: &mut DarkBurn, label: &str, c: CodeShape, at: Point, layer: usize) -> Result<Value, String> {
    let shape = Shape::new(ShapeKind::Code(c), Affine::translate(at.to_vec2()), layer);
    let mut id = ShapeId(0);
    app.editor.edit(label, |d, sel| {
        id = d.add(shape);
        sel.set([id]);
        render::refresh(d);
        true
    });
    let mut v = text_summary(app, id);
    v["errors"] = json!(code_errors(app, &[id]));
    Ok(v)
}

/// Copy the settings of a text draft (the Edit Text Shapes dialog) into the shape.
fn copy_settings(t: &mut TextShape, d: &TextShape) {
    t.text = d.text.clone();
    t.font = d.font.clone();
    t.height = d.height.max(0.1);
    t.bold = d.bold;
    t.italic = d.italic;
    t.upper_case = d.upper_case;
    t.welded = d.welded;
    t.distort = d.distort;
    t.letter_spacing = d.letter_spacing;
    t.line_spacing = d.line_spacing;
    t.align_x = d.align_x;
    t.align_y = d.align_y;
    t.variable = d.variable;
    t.offset = d.offset;
}

/// Apply the edited drafts of the Edit Text Shapes dialog as one undo step.
pub fn apply_drafts(app: &mut DarkBurn, drafts: &[(ShapeId, TextShape)]) -> bool {
    let changed = app.editor.edit("Edit text shapes", |d, _| {
        let mut any = false;
        for (id, draft) in drafts {
            any |= with_shape(d, *id, &mut |s| match &mut s.kind {
                ShapeKind::Text(t) => {
                    let before = t.clone();
                    copy_settings(t, draft);
                    *t != before
                }
                _ => false,
            });
        }
        if any {
            render::refresh(d);
        }
        any
    });
    if changed {
        for (_, d) in drafts {
            app.prefs.fonts.touch(&d.font);
        }
    }
    changed
}

/// The tool and dialog commands (menus, ribbon, shortcuts).
pub fn run(app: &mut DarkBurn, _ctx: &egui::Context, id: &str) -> bool {
    match id {
        "tools.text" => app.set_tool(Tool::Text),
        "tools.qr" => app.text.qr.open_new(),
        "text.edit_dialog" => app.text.open_edit_dialog(&app.editor),
        "text.font_picker" => app.text.font_picker_open = true,
        "tools.path_to_text" => {
            if let Err(e) = on_path(app, &Value::Null) {
                app.note = Some(e);
            }
        }
        "variable.next" | "variable.previous" | "variable.reset" | "variable.bake" | "text.detach_path" | "variable.csv_clear" => {
            if let Some(Err(e)) = execute(app, id, &Value::Null) {
                app.note = Some(e);
            }
        }
        _ => return false,
    }
    true
}

/// Whether a text command has something to act on (None: not one of this area's).
pub fn can_run(app: &DarkBurn, id: &str) -> Option<bool> {
    let sel = app.editor.selected();
    Some(match id {
        "tools.path_to_text" => {
            let texts = sel.iter().filter(|i| app.editor.doc.get(**i).is_some_and(is_text)).count();
            sel.len() >= 2 && texts >= 1 && sel.len() > texts
        }
        "text.edit_dialog" | "text.detach_path" => !selected_text(app, &sel).is_empty(),
        "variable.bake" => !sel.is_empty(),
        _ => return None,
    })
}
