//! State queries for MCP: what `inspect`, `measure` and the headless screenshot report. Pure
//! functions of the app state (`&DarkBurn`), so they run with or without a window.

use darkburn_core::kurbo::{Affine, PathEl, Shape as _};
use darkburn_core::layer::layer_colour;
use darkburn_core::{Document, LAYER_COUNT, Mode, Shape, ShapeId, ShapeKind};
use darkburn_mcp::{CommandError, ErrorCode, Region};
use serde_json::{Map, Value, json};

use crate::shell::DarkBurn;

pub fn mode_name(m: Mode) -> &'static str {
    match m {
        Mode::Line => "line",
        Mode::Fill => "fill",
        Mode::FillLine => "fill_line",
        Mode::OffsetFill => "offset_fill",
        Mode::Image => "image",
    }
}

fn r4(v: f64) -> f64 {
    if v.is_finite() { (v * 10_000.0).round() / 10_000.0 } else { 0.0 }
}

fn bbox_json(b: darkburn_core::kurbo::Rect) -> Value {
    json!([r4(b.x0), r4(b.y0), r4(b.x1), r4(b.y1)])
}

/// `s12` for shape 12: the form agents see and pass back.
pub fn shape_key(id: ShapeId) -> String {
    format!("s{}", id.0)
}

/// Parse `s12` or `12`.
pub fn parse_shape_key(text: &str) -> Option<ShapeId> {
    text.strip_prefix('s').unwrap_or(text).parse().ok().map(ShapeId)
}

pub fn shape_json(s: &Shape, detail: bool) -> Value {
    let b = s.bbox();
    let c = s.transform.as_coeffs();
    let mut o = Map::new();
    o.insert("id".into(), json!(shape_key(s.id)));
    o.insert("numeric_id".into(), json!(s.id.0));
    o.insert("name".into(), json!(s.name));
    o.insert("kind".into(), json!(s.kind_name()));
    o.insert("layer".into(), json!(s.layer));
    o.insert("locked".into(), json!(s.locked));
    o.insert("transform".into(), json!([r4(c[0]), r4(c[1]), r4(c[2]), r4(c[3]), r4(c[4]), r4(c[5])]));
    o.insert("bbox".into(), bbox_json(b));
    o.insert("width".into(), json!(r4(b.width())));
    o.insert("height".into(), json!(r4(b.height())));
    if detail {
        o.insert("priority".into(), json!(s.priority));
        o.insert("power_scale".into(), json!(s.power_scale));
        o.insert("closed".into(), json!(s.is_closed()));
        o.insert("tabs".into(), json!(s.tabs.len()));
        o.insert("virtual_array".into(), json!(s.array.is_some()));
    }
    match &s.kind {
        ShapeKind::Rect { width, height, radius } => {
            o.insert("geometry".into(), json!({"width": r4(*width), "height": r4(*height), "corner_radius": r4(*radius)}));
        }
        ShapeKind::Ellipse { rx, ry } => {
            o.insert("geometry".into(), json!({"rx": r4(*rx), "ry": r4(*ry)}));
        }
        ShapeKind::Polygon { sides, radius } => {
            o.insert("geometry".into(), json!({"sides": sides, "radius": r4(*radius)}));
        }
        ShapeKind::Path { path } => {
            let mut d = path.to_svg();
            if d.len() > 4000 && !detail {
                d.truncate(4000);
                d.push_str(" ...");
            }
            o.insert("geometry".into(), json!({"svg_path": d, "nodes": path.elements().len()}));
        }
        ShapeKind::Text(t) => {
            o.insert(
                "geometry".into(),
                json!({"text": t.text, "font": t.font, "height": t.height, "variable": format!("{:?}", t.variable).to_lowercase(), "offset": t.offset}),
            );
        }
        ShapeKind::Image(i) => {
            o.insert("geometry".into(), json!({"width": r4(i.width), "height": r4(i.height), "source": i.source, "cropped": i.crop.is_some()}));
        }
        ShapeKind::Code(c) => {
            o.insert(
                "geometry".into(),
                json!({"spec": serde_json::to_value(&c.spec).unwrap_or(Value::Null), "variable": c.variable, "offset": c.offset, "error": c.error}),
            );
        }
        ShapeKind::Group { children } => {
            o.insert("children".into(), Value::Array(children.iter().map(|c| shape_json(c, false)).collect()));
        }
    }
    Value::Object(o)
}

/// One layer: summary line, or with `full` every cut setting (main and sub-layers).
pub fn layer_json(doc: &Document, i: usize, full: bool) -> Value {
    let l = doc.layer(i);
    let m = l.main();
    let used = doc.shapes_on_layer(i).len();
    let order = doc.layer_order.iter().position(|x| *x == i);
    let mut o = json!({
        "index": i,
        "name": l.name,
        "colour": layer_colour(i),
        "mode": mode_name(m.mode),
        "speed": m.speed,
        "max_power": m.max_power,
        "min_power": m.min_power,
        "passes": m.passes,
        "interval": m.interval,
        "output": l.output,
        "show": l.show,
        "air": l.air,
        "tool_layer": darkburn_core::layer::is_tool_layer(i),
        "shapes": used,
        "output_order": order,
        "library_link": l.library_link,
    });
    if full {
        o["frame"] = json!(l.frame);
        o["settings"] = Value::Array(
            l.settings
                .iter()
                .map(|s| {
                    let mut v = serde_json::to_value(s).unwrap_or(Value::Null);
                    v["mode"] = json!(mode_name(s.mode));
                    v
                })
                .collect(),
        );
    }
    o
}

pub fn find_layer(doc: &Document, key: &str) -> Option<usize> {
    if let Ok(n) = key.parse::<usize>() {
        return (n < LAYER_COUNT).then_some(n);
    }
    (0..LAYER_COUNT).find(|i| doc.layer(*i).name.eq_ignore_ascii_case(key))
}

fn undo_json(app: &DarkBurn) -> Value {
    let h = &app.editor.history;
    json!({
        "can_undo": h.can_undo(),
        "can_redo": h.can_redo(),
        "undo_depth": h.len(),
        "undo": h.undo_labels().into_iter().take(50).collect::<Vec<_>>(),
        "redo": h.redo_labels().into_iter().take(50).collect::<Vec<_>>(),
        "revision": app.editor.revision(),
        "modified": app.editor.is_modified(),
    })
}

fn selection_json(app: &DarkBurn) -> Value {
    let ids = app.editor.selected();
    json!({
        "ids": ids.iter().map(|i| shape_key(*i)).collect::<Vec<_>>(),
        "count": ids.len(),
        "bbox": app.editor.doc.bbox_of(&ids).map(bbox_json),
        "current_layer": app.editor.current_layer,
    })
}

fn variables_json(doc: &Document) -> Value {
    let mut out = Vec::new();
    for s in doc.iter() {
        for leaf in s.leaves() {
            if let ShapeKind::Text(t) = &leaf.kind
                && t.variable != darkburn_core::shape::VariableText::Normal
            {
                out.push(
                    json!({"shape": shape_key(leaf.id), "source": format!("{:?}", t.variable).to_lowercase(), "text": t.text, "offset": t.offset}),
                );
            }
        }
    }
    json!(out)
}

pub fn design_json(app: &DarkBurn, rest: &[&str]) -> Result<Value, CommandError> {
    let doc = &app.editor.doc;
    match rest {
        [] => {
            let used: Vec<Value> = (0..LAYER_COUNT)
                .filter(|i| !doc.shapes_on_layer(*i).is_empty() || *i == app.editor.current_layer)
                .map(|i| layer_json(doc, i, true))
                .collect();
            Ok(json!({
                "path": app.project_path.as_ref().map(|p| p.display().to_string()),
                "modified": app.editor.is_modified(),
                "revision": app.editor.revision(),
                "page": {"width": app.prefs.page_width, "height": app.prefs.page_height, "origin": app.prefs.origin.label(), "unit": "mm"},
                "bbox": doc.bbox().map(bbox_json),
                "shape_count": doc.len(),
                "shapes": doc.iter().map(|s| shape_json(s, true)).collect::<Vec<_>>(),
                "layers_in_use": used,
                "selection": selection_json(app),
                "notes": doc.notes,
                "variables": variables_json(doc),
                "undo": undo_json(app),
            }))
        }
        ["shapes"] => Ok(json!(doc.iter().map(|s| shape_json(s, true)).collect::<Vec<_>>())),
        ["shapes", key] => {
            let id =
                parse_shape_key(key).ok_or_else(|| CommandError::invalid(format!("`{key}` is not a shape id (use `s12`, from design.shapes)")))?;
            doc.get(id).map(|s| shape_json(s, true)).ok_or_else(|| CommandError::failed(format!("no shape `{key}`: design.shapes lists the ids")))
        }
        ["layers"] => Ok(json!((0..LAYER_COUNT).map(|i| layer_json(doc, i, false)).collect::<Vec<_>>())),
        ["layers", key] => {
            let i = find_layer(doc, key).ok_or_else(|| CommandError::failed(format!("no layer `{key}`: use an index 0-31 or the layer's name")))?;
            Ok(layer_json(doc, i, true))
        }
        ["selection"] => Ok(selection_json(app)),
        ["notes"] => Ok(json!(doc.notes)),
        ["variables"] => Ok(variables_json(doc)),
        ["bbox"] => Ok(json!(doc.bbox().map(bbox_json))),
        ["page"] => Ok(json!({"width": app.prefs.page_width, "height": app.prefs.page_height, "origin": app.prefs.origin.label()})),
        ["job"] => Ok(job_json(app)),
        other => Err(CommandError::failed(format!(
            "unknown design path `design.{}`: use shapes, shapes.s1, layers, layers.0, layers.<name>, selection, notes, variables, bbox, page",
            other.join(".")
        ))),
    }
}

/// Flatten a JSON value to `(dotted key, leaf)`; arrays and empty objects are leaves.
fn flatten(prefix: &str, v: &Value, out: &mut Vec<(String, Value)>) {
    match v {
        Value::Object(m) if !m.is_empty() => {
            for (k, x) in m {
                flatten(&if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") }, x, out);
            }
        }
        _ => out.push((prefix.to_string(), v.clone())),
    }
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
        Value::Null => "null",
    }
}

/// The preferences as JSON without the AI-agent section (agents cannot read or change it).
pub fn prefs_value(app: &DarkBurn) -> Value {
    let mut v: Value = serde_json::from_str(&app.prefs.to_json()).unwrap_or(Value::Null);
    if let Some(o) = v.as_object_mut() {
        o.remove("mcp");
    }
    v
}

pub fn settings_list(app: &DarkBurn) -> Vec<Value> {
    let mut leaves = Vec::new();
    flatten("", &prefs_value(app), &mut leaves);
    leaves.sort_by(|a, b| a.0.cmp(&b.0));
    leaves.into_iter().map(|(k, v)| json!({"key": k, "value": v, "type": type_name(&v)})).collect()
}

pub fn settings_json(app: &DarkBurn, rest: &[&str]) -> Result<Value, CommandError> {
    let list = settings_list(app);
    if rest.is_empty() {
        return Ok(Value::Array(list));
    }
    let key = rest.join(".");
    list.iter()
        .find(|s| s["key"] == key.as_str() || s["key"] == format!("opts.{key}").as_str())
        .cloned()
        .ok_or_else(|| CommandError::failed(format!("no setting `{key}`: list_settings shows the keys")))
}

/// Change one preference by dotted key. The value's type must match the old one.
pub fn set_setting(app: &mut DarkBurn, key: &str, value: &Value) -> Result<Value, String> {
    if key == "mcp" || key.starts_with("mcp.") {
        return Err("the AI-agent settings can only be changed by the user in DarkBurn's Settings window".into());
    }
    let mut all: Value = serde_json::from_str(&app.prefs.to_json()).map_err(|e| e.to_string())?;
    let candidates = [key.to_string(), format!("opts.{key}")];
    let mut done = None;
    for k in &candidates {
        let pointer = format!("/{}", k.replace('.', "/"));
        let Some(cur) = all.pointer_mut(&pointer) else { continue };
        if cur.is_object() {
            return Err(format!("`{k}` is a group of settings, not one setting: list_settings shows the leaves"));
        }
        if std::mem::discriminant(&*cur) != std::mem::discriminant(value) && !(cur.is_number() && value.is_number()) {
            return Err(format!("`{k}` is a {} setting, not a {}", type_name(cur), type_name(value)));
        }
        *cur = value.clone();
        done = Some(k.clone());
        break;
    }
    let Some(k) = done else { return Err(format!("no setting `{key}`: list_settings shows the keys")) };
    let mut new: crate::prefs::Prefs = serde_json::from_value(all).map_err(|e| format!("`{k}`: {e}"))?;
    new.sanitise();
    app.prefs = new;
    Ok(json!({"key": k, "value": value}))
}

pub fn job_json(app: &DarkBurn) -> Value {
    let settings = crate::job_cmds::current_settings(app);
    json!({
        "settings": serde_json::to_value(&settings).unwrap_or(Value::Null),
        "user_origin": app.job.user_origin.map(|p| json!([r4(p.x), r4(p.y)])),
        "gcode_flavor": format!("{:?}", app.job.gcode.flavor).to_lowercase(),
        "last_job": app.job.last_job.as_ref().map(|j| json!({
            "ops": j.toolpath.ops.len(), "bounds": bbox_json(j.bounds), "warnings": j.warnings, "out_of_bounds": j.out_of_bounds(),
        })),
        "building": app.job.pending.is_some(),
        "preview": {"open": app.job.preview.open, "playing": app.job.preview.playing, "error": app.job.preview.error},
    })
}

pub fn machine_json(app: &DarkBurn, rest: &[&str]) -> Result<Value, CommandError> {
    match rest {
        [] | ["status"] => Ok(crate::machine_ui::status_json(&app.machine)),
        ["devices"] => {
            let active = app.machine.devices.active;
            Ok(json!(
                app.machine
                    .devices
                    .devices
                    .iter()
                    .map(|d| json!({
                        "id": d.id, "name": d.name, "kind": format!("{:?}", d.kind), "connection": format!("{:?}", d.connection),
                        "host": d.host, "width_mm": d.width_mm, "height_mm": d.height_mm, "origin": format!("{:?}", d.origin),
                        "active": Some(d.id) == active,
                        "simulator": format!("{:?}", d.kind).to_lowercase().contains("simulator"),
                    }))
                    .collect::<Vec<_>>()
            ))
        }
        other => Err(CommandError::failed(format!("unknown machine path `machine.{}`: use status or devices", other.join(".")))),
    }
}

pub fn library_json(app: &DarkBurn) -> Value {
    let lib = &app.extras.library;
    json!({
        "cut_library": {
            "name": lib.lib.name,
            "path": lib.path.as_ref().map(|p| p.display().to_string()),
            "entries": lib.lib.entries.iter().map(|e| json!({"name": e.link_name(), "mode": e.mode().label()})).collect::<Vec<_>>(),
            "selected": lib.selected,
            "status": lib.status,
        },
        "art": {
            "libraries": app.extras.art.libs.len(),
            "status": app.extras.art.status,
        },
        "linked_layers": (0..LAYER_COUNT).filter_map(|i| app.editor.doc.layer(i).library_link.as_ref().map(|n| json!({"layer": i, "entry": n}))).collect::<Vec<_>>(),
    })
}

pub fn vision_json(app: &DarkBurn, which: Option<&str>) -> Result<Value, CommandError> {
    let all = serde_json::to_value(&app.vision.state).unwrap_or(Value::Null);
    match which {
        None => Ok(all),
        Some("camera") => Ok(all["camera"].clone()),
        Some("rotary") => {
            Ok(json!({"rotary": all["rotary"], "split": all["rotary_split"], "cylinder": all["cylinder"], "on": app.machine.rotary_on}))
        }
        Some("print_and_cut" | "pnc") => Ok(all["pnc"].clone()),
        Some(other) => Err(CommandError::failed(format!("unknown vision path `{other}`: use camera, rotary or print_and_cut"))),
    }
}

/// Names of the windows and dialogs open now, in a stable order.
pub fn open_dialogs(app: &DarkBurn) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    let mut add = |on: bool, name: &str| {
        if on {
            v.push(name.to_string());
        }
    };
    add(app.settings_open, "settings");
    add(app.about_open, "about");
    add(app.notes_open, "notes");
    add(app.cut_settings_open.is_some(), "cut_settings");
    add(app.extras.help_open, "help");
    add(app.job.preview.open, "preview");
    add(app.job.optimization_open, "optimization_settings");
    add(app.job.gcode_open, "gcode_settings");
    add(app.job.material_open, "material_test");
    add(app.job.bounds_prompt.is_some(), "out_of_bounds_prompt");
    add(app.job.message.is_some(), "job_message");
    for name in app.files.open_dialogs() {
        add(true, name);
    }
    add(app.images.offsets_open, "scanning_offsets");
    let d = &app.machine.dlg;
    add(d.devices, "devices");
    add(d.wizard.is_some(), "device_wizard");
    add(d.find.is_some(), "find_device");
    add(d.settings.is_some(), "device_settings");
    add(d.machine_settings, "machine_settings");
    add(d.controller_info, "controller_info");
    add(d.confirm_connect.is_some(), "confirm_connect");
    add(d.confirm_close, "confirm_close");
    add(d.checklist.is_some(), "job_checklist");
    add(d.macro_edit.is_some(), "macro_editor");
    add(d.message.is_some(), "machine_message");
    add(d.confirm_restore.is_some(), "confirm_restore");
    add(app.machine.message.is_some(), "machine_notice");
    if let Some(dlg) = &app.vision.ui.open {
        v.push(format!("vision:{dlg:?}").to_lowercase());
    }
    if let Some(name) = app.shape_ui.open {
        v.push(format!("shape:{name}"));
    }
    v
}

pub fn ui_json(app: &DarkBurn, ctx: Option<&egui::Context>) -> Value {
    let panels: Vec<Value> = crate::panels::PANELS
        .iter()
        .map(|p| {
            let st = app.prefs.panel(p.id);
            let front = app.prefs.active_tab.get(&st.area).is_some_and(|t| t == p.id);
            json!({"id": p.id, "title": p.title, "area": st.area.label(), "visible": st.visible, "active_tab": st.visible && front})
        })
        .collect();
    let shortcuts: Vec<Value> = crate::keymap::effective(&app.prefs.hotkeys)
        .into_iter()
        .filter(|(_, k)| !k.is_empty())
        .map(|(id, k)| json!({"command": id, "key": k}))
        .collect();
    json!({
        "window": ctx.map(|c| {
            let (size, ppp) = c.input(|i| (i.viewport().inner_rect.map(|r| [r.width(), r.height()]), i.pixels_per_point()));
            json!({"inner_size": size, "pixels_per_point": ppp})
        }),
        "headless": ctx.is_none(),
        "dialogs": open_dialogs(app),
        "tool": format!("{:?}", app.tool).to_lowercase(),
        "pan_tool": app.pan_tool,
        "locks": {"move": app.locks[0], "size": app.locks[1], "rotate": app.locks[2], "shear": app.locks[3]},
        "panels": panels,
        "toolbars": app.prefs.toolbars,
        "ribbon_tab": app.prefs.ribbon_tab,
        "status_bar": app.prefs.status_bar,
        "language": app.prefs.language,
        "theme": app.prefs.theme,
        "zoom_percent": app.prefs.view.percent(),
        "hint": app.hint,
        "note": app.note,
        "autosave": app.autosave_status,
        "shortcuts": shortcuts,
        "mcp_chip": "shown in the status bar while MCP control is on",
    })
}

/// Cut a JSON value to `depth` levels (0 = no limit); deeper values become `"..."` summaries.
pub fn limit_depth(v: Value, depth: u32) -> Value {
    if depth == 0 {
        return v;
    }
    fn go(v: Value, left: u32) -> Value {
        match v {
            Value::Object(m) => {
                if left == 0 {
                    return json!(format!("{{... {} fields}}", m.len()));
                }
                Value::Object(m.into_iter().map(|(k, x)| (k, go(x, left - 1))).collect())
            }
            Value::Array(a) => {
                if left == 0 {
                    return json!(format!("[... {} items]", a.len()));
                }
                Value::Array(a.into_iter().map(|x| go(x, left - 1)).collect())
            }
            other => other,
        }
    }
    go(v, depth)
}

// ----- measure -----------------------------------------------------------------------------------

/// Which shapes a measure request names: `selection`, `all`, `layer:N`, `s12`.
pub fn measure_targets(app: &DarkBurn, target: &Value) -> Result<(String, Vec<ShapeId>), CommandError> {
    let doc = &app.editor.doc;
    let sel = app.editor.selected();
    let one = |t: &str| -> Result<(String, Vec<ShapeId>), CommandError> {
        match t {
            "selection" => Ok(("selection".into(), sel.clone())),
            "all" | "design" => Ok(("design".into(), doc.iter().map(|s| s.id).collect())),
            other => {
                let layer = other.strip_prefix("layer:").or_else(|| other.strip_prefix("layer.")).unwrap_or(other);
                if let Some(id) = parse_shape_key(other).filter(|id| doc.get(*id).is_some()) {
                    return Ok((shape_key(id), vec![id]));
                }
                match find_layer(doc, layer) {
                    Some(i) => Ok((format!("layer {i}"), doc.shapes_on_layer(i))),
                    None => Err(CommandError::invalid(format!(
                        "unknown measure target `{other}`: use design, selection, a layer name or index, or shape ids such as s12"
                    ))),
                }
            }
        }
    };
    match target {
        Value::Null => one(if sel.is_empty() { "design" } else { "selection" }),
        Value::String(t) => one(t),
        Value::Array(list) => {
            let mut ids = Vec::new();
            for t in list {
                let key = t.as_str().ok_or_else(|| CommandError::invalid("a target list holds shape ids such as s12"))?;
                let id = parse_shape_key(key).filter(|id| doc.get(*id).is_some()).ok_or_else(|| CommandError::failed(format!("no shape `{key}`")))?;
                ids.push(id);
            }
            Ok((format!("{} shapes", ids.len()), ids))
        }
        _ => Err(CommandError::invalid("target is a name or a list of shape ids")),
    }
}

fn outline_length_area(s: &Shape) -> (f64, f64) {
    let mut length = 0.0;
    let mut area = 0.0;
    for leaf in s.leaves() {
        let p = leaf.outline();
        length += p.perimeter(0.01);
        if leaf.is_closed() {
            area += p.area().abs();
        }
    }
    (length, area)
}

pub fn measure_shapes(app: &DarkBurn, target: &Value) -> Result<Value, CommandError> {
    let (name, ids) = measure_targets(app, target)?;
    let doc = &app.editor.doc;
    let mut per = Vec::new();
    let (mut total_len, mut total_area) = (0.0, 0.0);
    for id in &ids {
        let Some(s) = doc.get(*id) else { continue };
        let b = s.bbox();
        let (len, area) = outline_length_area(s);
        total_len += len;
        total_area += area;
        per.push(json!({"id": shape_key(*id), "kind": s.kind_name(), "layer": s.layer, "bbox": bbox_json(b), "width": r4(b.width()), "height": r4(b.height()), "length_mm": r4(len), "area_mm2": r4(area)}));
    }
    Ok(json!({
        "target": name,
        "shapes": per.len(),
        "bbox": doc.bbox_of(&ids).map(bbox_json),
        "width": doc.bbox_of(&ids).map(|b| r4(b.width())),
        "height": doc.bbox_of(&ids).map(|b| r4(b.height())),
        "total_length_mm": r4(total_len),
        "total_area_mm2": r4(total_area),
        "per_shape": per,
        "unit": "mm",
    }))
}

// ----- offscreen picture ---------------------------------------------------------------------------

/// A picture of the design drawn without a window: the page, the shapes in their layer colours
/// (filled layers lightly filled). `region` is in design millimetres (x, y of the lower left).
/// `width_px` is the picture's longer side.
pub fn design_png(doc: &Document, page: (f64, f64), region: Option<Region>, width_px: u32) -> Result<Vec<u8>, CommandError> {
    let (x0, y0, w, h) = match region {
        Some(r) if r.width > 0.0 && r.height > 0.0 => (f64::from(r.x), f64::from(r.y), f64::from(r.width), f64::from(r.height)),
        _ => {
            let mut b = darkburn_core::kurbo::Rect::new(0.0, 0.0, page.0, page.1);
            if let Some(d) = doc.bbox() {
                b = b.union(d);
            }
            let m = (b.width().max(b.height()) * 0.02).max(1.0);
            (b.x0 - m, b.y0 - m, b.width() + 2.0 * m, b.height() + 2.0 * m)
        }
    };
    let longer = width_px.clamp(64, 4000);
    let k = f64::from(longer) / w.max(h).max(1e-6);
    let (pw, ph) = (((w * k).round() as u32).clamp(1, 4000), ((h * k).round() as u32).clamp(1, 4000));
    let mut pixmap = tiny_skia::Pixmap::new(pw, ph).ok_or_else(|| CommandError::failed("the picture size is not valid"))?;
    pixmap.fill(tiny_skia::Color::from_rgba8(32, 32, 36, 255));
    let to_px = Affine::scale_non_uniform(k, -k) * Affine::translate((-x0, -(y0 + h)));
    let pt = |q: darkburn_core::kurbo::Point| {
        let q = to_px * q;
        (q.x as f32, q.y as f32)
    };
    let build = |path: &darkburn_core::kurbo::BezPath| {
        let mut pb = tiny_skia::PathBuilder::new();
        for el in path.elements() {
            match *el {
                PathEl::MoveTo(a) => {
                    let (x, y) = pt(a);
                    pb.move_to(x, y);
                }
                PathEl::LineTo(a) => {
                    let (x, y) = pt(a);
                    pb.line_to(x, y);
                }
                PathEl::QuadTo(a, b) => {
                    let ((x1, y1), (x, y)) = (pt(a), pt(b));
                    pb.quad_to(x1, y1, x, y);
                }
                PathEl::CurveTo(a, b, c) => {
                    let ((x1, y1), (x2, y2), (x, y)) = (pt(a), pt(b), pt(c));
                    pb.cubic_to(x1, y1, x2, y2, x, y);
                }
                PathEl::ClosePath => pb.close(),
            }
        }
        pb.finish()
    };
    // The page.
    let page_path = darkburn_core::kurbo::Rect::new(0.0, 0.0, page.0, page.1).to_path(0.1);
    if let Some(p) = build(&page_path) {
        let mut paint = tiny_skia::Paint::default();
        paint.set_color_rgba8(52, 52, 58, 255);
        pixmap.fill_path(&p, &paint, tiny_skia::FillRule::Winding, tiny_skia::Transform::identity(), None);
        paint.set_color_rgba8(110, 110, 120, 255);
        let stroke = tiny_skia::Stroke { width: 1.0, ..Default::default() };
        pixmap.stroke_path(&p, &paint, &stroke, tiny_skia::Transform::identity(), None);
    }
    for shape in doc.iter() {
        for leaf in shape.leaves() {
            if !doc.layer(leaf.layer).show {
                continue;
            }
            let [r, g, b] = layer_colour(leaf.layer);
            let Some(p) = build(&leaf.outline()) else { continue };
            let mut paint = tiny_skia::Paint::default();
            paint.anti_alias = true;
            if doc.is_filled(&leaf) && leaf.is_closed() {
                paint.set_color_rgba8(r, g, b, 90);
                pixmap.fill_path(&p, &paint, tiny_skia::FillRule::EvenOdd, tiny_skia::Transform::identity(), None);
            }
            paint.set_color_rgba8(r, g, b, 255);
            let stroke = tiny_skia::Stroke { width: 1.5, ..Default::default() };
            pixmap.stroke_path(&p, &paint, &stroke, tiny_skia::Transform::identity(), None);
        }
    }
    // tiny-skia holds premultiplied pixels; everything drawn over the opaque background is opaque.
    crate::import::rgba_to_png(pw, ph, pixmap.data().to_vec()).ok_or_else(|| CommandError::new(ErrorCode::Failed, "could not encode the picture"))
}
