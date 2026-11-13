#![allow(dead_code, reason = "the MCP backend that calls this is wired in the next step")]
//! Running a registered command with JSON arguments, without a window: the entry point agents,
//! scripts and tests use. The commands and their argument schemas are listed in `commands.rs`;
//! this file does the work of the ones that take arguments and refuses the ones that cannot run.

use std::path::Path;

use darkburn_core::arrange::{self, Anchor};
use darkburn_core::kurbo::{BezPath, Point, Rect};
use darkburn_core::{Mode, Shape, ShapeId};
use serde_json::{Value, json};

use crate::commands;
use crate::shell::DarkBurn;

/// Run command `id` with `args` (a JSON object, or null when it takes none). Returns what the
/// command reports, or why it did nothing.
pub fn execute(app: &mut DarkBurn, ctx: &egui::Context, id: &str, args: &Value) -> Result<Value, String> {
    if commands::lookup(id).is_none() {
        return Err(format!("unknown command {id}"));
    }
    if let Some(result) = crate::laser_position::api(app, id, args) {
        return result;
    }
    if let Some(result) = crate::machine_ui::api(app, id, args) {
        return result;
    }
    if let Some(result) = crate::job_cmds::execute(app, id, args) {
        return result;
    }
    if let Some(result) = crate::vision_cmds::execute(app, id, args) {
        return result;
    }
    if let Some(result) = crate::layer_defaults::execute(app, id, args) {
        return result;
    }
    if let Some(result) = crate::misc_cmds::execute(app, id, args) {
        return result;
    }
    if let Some(result) = crate::nest_cmds::execute(app, id, args) {
        return result;
    }
    if let Some(result) = crate::tangent_tool::execute(app, id, args) {
        return result;
    }
    if let Some(result) = crate::taper_warp::execute(app, id, args) {
        return result;
    }
    if commands::hardware(id) {
        return Err(format!("{id} operates the machine and is not available"));
    }
    if !commands::works(id) {
        return Err(format!("{id} is not implemented in this build"));
    }
    // A command that names its shape does not need it selected.
    let names_shape = args.get("shape").and_then(Value::as_u64).is_some_and(|i| app.editor.doc.get(ShapeId(i)).is_some());
    if !names_shape && !app.can_run(id) {
        return Err(format!("{id} has nothing to act on right now"));
    }
    if let Some(result) = with_args(app, id, args) {
        return result;
    }
    if let Some(result) = crate::text_cmds::execute(app, id, args) {
        return result;
    }
    if let Some(result) = crate::shape_ops::execute(app, id, args) {
        return result;
    }
    let before = app.editor.revision();
    app.run(ctx, id);
    Ok(json!({ "changed": app.editor.revision() != before }))
}

/// [`execute`] with no window: viewport commands (exit) have no effect.
pub fn execute_headless(app: &mut DarkBurn, id: &str, args: &Value) -> Result<Value, String> {
    execute(app, &egui::Context::default(), id, args)
}

fn num(a: &Value, key: &str) -> Result<f64, String> {
    a.get(key).and_then(Value::as_f64).filter(|v| v.is_finite()).ok_or_else(|| format!("argument {key} must be a number"))
}

fn opt_num(a: &Value, key: &str) -> Option<f64> {
    a.get(key).and_then(Value::as_f64).filter(|v| v.is_finite())
}

fn text<'a>(a: &'a Value, key: &str) -> Result<&'a str, String> {
    a.get(key).and_then(Value::as_str).ok_or_else(|| format!("argument {key} must be text"))
}

fn layer_of(a: &Value) -> Result<usize, String> {
    let l = a.get("layer").and_then(Value::as_u64).ok_or("argument layer must be a whole number")?;
    if l < darkburn_core::LAYER_COUNT as u64 { Ok(l as usize) } else { Err(format!("layer {l} does not exist (0 to 31)")) }
}

fn opt_layer(a: &Value, default: usize) -> Result<usize, String> {
    if a.get("layer").is_some() { layer_of(a) } else { Ok(default) }
}

fn anchor_of(a: &Value) -> Result<Anchor, String> {
    let Some(name) = a.get("anchor").and_then(Value::as_str) else { return Ok(Anchor::Centre) };
    Ok(match name {
        "top_left" => Anchor::TopLeft,
        "top" => Anchor::Top,
        "top_right" => Anchor::TopRight,
        "left" => Anchor::Left,
        "centre" | "center" => Anchor::Centre,
        "right" => Anchor::Right,
        "bottom_left" => Anchor::BottomLeft,
        "bottom" => Anchor::Bottom,
        "bottom_right" => Anchor::BottomRight,
        other => return Err(format!("unknown anchor {other}")),
    })
}

fn mode_of(name: &str) -> Result<Mode, String> {
    Ok(match name {
        "line" => Mode::Line,
        "fill" => Mode::Fill,
        "fill_line" => Mode::FillLine,
        "offset_fill" => Mode::OffsetFill,
        "image" => Mode::Image,
        other => return Err(format!("unknown mode {other}")),
    })
}

fn selected(app: &DarkBurn) -> Result<Vec<ShapeId>, String> {
    let ids = app.editor.selected();
    if ids.is_empty() { Err("nothing is selected".into()) } else { Ok(ids) }
}

fn changed(done: bool) -> Result<Value, String> {
    Ok(json!({ "changed": done }))
}

/// The commands that take arguments; None when `id` is not one of them.
fn with_args(app: &mut DarkBurn, id: &str, a: &Value) -> Option<Result<Value, String>> {
    Some(match id {
        "window.toolbar_place" => crate::toolbar_dock::place_command(app, a),
        "window.toolbar_move" => (|| {
            let tb = text(a, "toolbar")?;
            if !crate::prefs::TOOLBARS.iter().any(|(t, _)| *t == tb) {
                return Err(format!("unknown toolbar {tb}"));
            }
            let delta = opt_num(a, "delta").unwrap_or(-1.0) as i32;
            let moved = app.prefs.move_toolbar(tb, delta);
            Ok(json!({ "changed": moved, "order": app.prefs.full_toolbar_order() }))
        })(),
        "selection.set" => (|| {
            let ids: Vec<ShapeId> =
                a.get("ids").and_then(Value::as_array).ok_or("argument ids must be a list")?.iter().filter_map(Value::as_u64).map(ShapeId).collect();
            let known: Vec<ShapeId> = ids.into_iter().filter(|i| app.editor.doc.get(*i).is_some()).collect();
            app.editor.selection.set(known.iter().copied());
            Ok(json!({ "selected": known.iter().map(|i| i.0).collect::<Vec<_>>() }))
        })(),
        "selection.clear" => {
            app.editor.selection.clear();
            Ok(json!({ "selected": [] }))
        }
        "shape.add_rect" => (|| {
            let (x, y, w, h) = (num(a, "x")?, num(a, "y")?, num(a, "width")?, num(a, "height")?);
            let layer = opt_layer(a, app.editor.current_layer)?;
            let radius = opt_num(a, "radius").unwrap_or(0.0);
            sized(w, h)?;
            add(app, "Add rectangle", Shape::rect(Rect::new(x, y, x + w, y + h), radius, layer))
        })(),
        "shape.add_ellipse" => (|| {
            let (x, y, w, h) = (num(a, "x")?, num(a, "y")?, num(a, "width")?, num(a, "height")?);
            let layer = opt_layer(a, app.editor.current_layer)?;
            sized(w, h)?;
            add(app, "Add ellipse", Shape::ellipse(Rect::new(x, y, x + w, y + h), layer))
        })(),
        "shape.add_polygon" => (|| {
            let (cx, cy, r) = (num(a, "cx")?, num(a, "cy")?, num(a, "radius")?);
            let sides = a.get("sides").and_then(Value::as_u64).ok_or("argument sides must be a whole number")? as u32;
            let layer = opt_layer(a, app.editor.current_layer)?;
            if r < MIN_DRAWN {
                return Err("radius must be above zero".into());
            }
            add(app, "Add polygon", Shape::polygon(Point::new(cx, cy), sides, r, layer))
        })(),
        "shape.add_path" => (|| {
            let path = BezPath::from_svg(text(a, "svg_path")?).map_err(|e| format!("path data: {e}"))?;
            let layer = opt_layer(a, app.editor.current_layer)?;
            add(app, "Add path", Shape::path(path, layer))
        })(),
        "shape.place" => (|| {
            let ids = selected(app)?;
            let (anchor, x, y) = (anchor_of(a)?, num(a, "x")?, num(a, "y")?);
            changed(app.editor.edit("Move", |d, _| arrange::place(d, &ids, anchor, Point::new(x, y))))
        })(),
        "shape.translate" => (|| {
            let ids = selected(app)?;
            let d2 = darkburn_core::kurbo::Vec2::new(num(a, "dx")?, num(a, "dy")?);
            changed(app.editor.edit("Move", |d, _| arrange::translate(d, &ids, d2)))
        })(),
        "shape.resize" => (|| {
            let ids = selected(app)?;
            let (anchor, w, h) = (anchor_of(a)?, num(a, "width")?, num(a, "height")?);
            if w <= 0.0 || h <= 0.0 {
                return Err("width and height must be above zero".into());
            }
            changed(app.editor.edit("Resize", |d, _| arrange::resize(d, &ids, anchor, w, h)))
        })(),
        "shape.rotate" => (|| {
            let ids = selected(app)?;
            let deg = num(a, "degrees")?;
            changed(app.editor.edit("Rotate", |d, _| arrange::rotate(d, &ids, deg)))
        })(),
        "shape.set_layer" => (|| {
            let ids = selected(app)?;
            let layer = layer_of(a)?;
            changed(app.editor.edit("Change layer", |d, _| arrange::set_layer(d, &ids, layer)))
        })(),
        "shape.set_props" => (|| {
            let ids = selected(app)?;
            let (name, locked) = (a.get("name").and_then(Value::as_str).map(str::to_string), a.get("locked").and_then(Value::as_bool));
            let (priority, power) = (a.get("priority").and_then(Value::as_i64), opt_num(a, "power_scale"));
            // Forced cut direction and start point (a document point, or null to clear it).
            let direction = match a.get("direction").and_then(Value::as_str) {
                None => None,
                Some("auto") => Some(darkburn_core::CutDirection::Auto),
                Some("clockwise") => Some(darkburn_core::CutDirection::Clockwise),
                Some("counter_clockwise") => Some(darkburn_core::CutDirection::CounterClockwise),
                Some("reversed") => Some(darkburn_core::CutDirection::Reversed),
                Some(other) => return Err(format!("unknown direction {other}")),
            };
            let start: Option<Option<Point>> = match a.get("start_point") {
                None => None,
                Some(Value::Null) => Some(None),
                Some(v) => {
                    let xy = v.as_array().filter(|a| a.len() == 2).and_then(|a| Some((a[0].as_f64()?, a[1].as_f64()?)));
                    Some(Some(xy.map(|(x, y)| Point::new(x, y)).ok_or("start_point must be [x, y] or null")?))
                }
            };
            changed(app.editor.edit("Shape properties", |d, _| {
                let mut any = false;
                for id in &ids {
                    let Some(s) = d.get_mut(*id) else { continue };
                    let before = s.clone();
                    if let Some(n) = &name {
                        s.name = n.clone();
                    }
                    if let Some(l) = locked {
                        s.locked = l;
                    }
                    if let Some(p) = priority {
                        s.priority = p.clamp(-1000, 1000) as i32;
                    }
                    if let Some(p) = power {
                        s.power_scale = p.clamp(0.0, 1000.0);
                    }
                    if let Some(dir) = direction {
                        s.direction = dir;
                    }
                    if let Some(sp) = start {
                        let local = sp.map(|p| {
                            let l = s.transform.inverse() * p;
                            [l.x, l.y]
                        });
                        s.set_start_points(local.as_slice());
                    }
                    // Nothing asked for, or what the shape already had, is not a change.
                    any |= *s != before;
                }
                any
            }))
        })(),
        "layer.select" => (|| {
            app.editor.current_layer = layer_of(a)?;
            Ok(json!({ "current_layer": app.editor.current_layer }))
        })(),
        "layer.assign" => (|| {
            let layer = layer_of(a)?;
            crate::palette::apply(app, layer, crate::palette::PaletteClick::Assign, 0.0);
            Ok(json!({ "current_layer": layer }))
        })(),
        "layer.select_shapes" => (|| {
            let layer = layer_of(a)?;
            crate::palette::apply(app, layer, crate::palette::PaletteClick::SelectShapes, 0.0);
            Ok(json!({ "selected": app.editor.selected().iter().map(|i| i.0).collect::<Vec<_>>() }))
        })(),
        "layer.set_mode" => (|| {
            let (layer, mode) = (layer_of(a)?, mode_of(text(a, "mode")?)?);
            if !crate::cuts_panel::modes_for(&app.editor.doc, layer).contains(&mode) {
                return Err(format!("layer {layer} cannot use {}", mode.label()));
            }
            changed(app.editor.edit("Layer mode", |d, _| crate::cuts_panel::set_mode(d, layer, mode)))
        })(),
        "layer.set_param" => set_param(app, a),
        "layer.move" => (|| {
            let (layer, delta) = (layer_of(a)?, a.get("delta").and_then(Value::as_i64).ok_or("argument delta must be a whole number")?);
            changed(app.editor.edit("Move layer", |d, _| d.move_layer(layer, delta.clamp(-32, 32) as isize)))
        })(),
        "layer.delete" => (|| {
            let layer = layer_of(a)?;
            let with = a.get("with_shapes").and_then(Value::as_bool).unwrap_or(false);
            // A layer nobody used or set up has nothing to delete.
            changed(app.editor.edit("Delete layer", |d, _| {
                let before = d.clone();
                d.delete_layer(layer, with);
                *d != before
            }))
        })(),
        "project.info" => Ok(info(app)),
        "project.open" => text(a, "path").and_then(|p| crate::files::open_path(app, Path::new(p))).map(|_| info(app)),
        "project.save_as" => text(a, "path").and_then(|p| crate::files::save_to(app, Path::new(p))).map(|_| info(app)),
        "setting.set" => setting(app, a),
        _ if crate::image_ui::handles(id) => crate::image_ui::execute(app, id, a),
        _ => return crate::file_io::with_args(app, id, a).or_else(|| crate::extras::with_args(app, id, a)),
    })
}

/// The smallest size (mm) the rectangle, ellipse and polygon tools draw; a drag shorter than
/// this makes no shape, and neither do the commands.
const MIN_DRAWN: f64 = 1e-3;

/// A width and height a shape can be drawn with (either may be negative: the box then lies on
/// the other side of its corner).
fn sized(w: f64, h: f64) -> Result<(), String> {
    if w.abs() < MIN_DRAWN || h.abs() < MIN_DRAWN { Err("width and height must not be zero".into()) } else { Ok(()) }
}

fn add(app: &mut DarkBurn, label: &str, shape: Shape) -> Result<Value, String> {
    let mut new = ShapeId(0);
    app.editor.edit(label, |d, sel| {
        new = d.add(shape);
        sel.set([new]);
        true
    });
    Ok(json!({ "id": new.0 }))
}

fn set_param(app: &mut DarkBurn, a: &Value) -> Result<Value, String> {
    let layer = layer_of(a)?;
    let field = text(a, "field")?;
    let value = a.get("value").ok_or("argument value is missing")?;
    let number = || value.as_f64().filter(|v| v.is_finite()).ok_or_else(|| format!("{field} needs a number"));
    let flag = || value.as_bool().ok_or_else(|| format!("{field} needs true or false"));
    enum V {
        Name(String),
        Speed(f64),
        Max(f64),
        Min(f64),
        Interval(f64),
        Passes(u32),
        Output(bool),
        Show(bool),
        Air(bool),
    }
    let v = match field {
        "name" => V::Name(value.as_str().ok_or("name needs text")?.to_string()),
        "speed" => V::Speed(number()?),
        "max_power" => V::Max(number()?),
        "min_power" => V::Min(number()?),
        "interval" => V::Interval(number()?),
        "passes" => V::Passes(number()?.round().clamp(1.0, 10_000.0) as u32),
        "output" => V::Output(flag()?),
        "show" => V::Show(flag()?),
        "air" => V::Air(flag()?),
        other => return Err(format!("unknown layer field {other}")),
    };
    if matches!(v, V::Output(true)) && darkburn_core::layer::is_tool_layer(layer) {
        return Err("tool layers never output".into());
    }
    let done = app.editor.edit("Layer setting", |d, sel| {
        let before = d.layer(layer).clone();
        let l = d.layer_mut(layer);
        match v {
            V::Name(n) => l.name = n,
            V::Speed(x) => l.main_mut().speed = x,
            V::Max(x) => l.main_mut().max_power = x,
            V::Min(x) => l.main_mut().min_power = x,
            V::Interval(x) => l.main_mut().interval = x,
            V::Passes(x) => l.main_mut().passes = x,
            V::Output(x) => l.output = x,
            V::Show(x) => l.show = x,
            V::Air(x) => l.air = x,
        }
        l.main_mut().sanitise();
        let changed = *d.layer(layer) != before;
        // Shapes of a layer that was just hidden are no longer selected.
        if changed {
            sel.retain_selectable(d);
        }
        changed
    });
    changed(done)
}

fn setting(app: &mut DarkBurn, a: &Value) -> Result<Value, String> {
    let key = text(a, "key")?;
    let value = a.get("value").ok_or("argument value is missing")?;
    let flag = || value.as_bool().ok_or_else(|| format!("{key} needs true or false"));
    let number = || value.as_f64().filter(|v| v.is_finite()).ok_or_else(|| format!("{key} needs a number"));
    let p = &mut app.prefs;
    match key {
        "filled" => p.filled = flag()?,
        "smooth" => p.smooth = flag()?,
        "show_grid" => p.show_grid = flag()?,
        "grid_spacing" => p.grid_spacing = number()?.clamp(0.0, 1000.0) as f32,
        "grid_contrast" => p.grid_contrast = number()?.clamp(0.0, 1.0) as f32,
        "backdrop" => {
            p.backdrop = match value.as_str().map(str::to_ascii_lowercase).as_deref() {
                Some("theme") => crate::canvas::Backdrop::Theme,
                Some("light") => crate::canvas::Backdrop::Light,
                Some("dark") => crate::canvas::Backdrop::Dark,
                _ => return Err("backdrop needs \"theme\", \"light\" or \"dark\"".into()),
            }
        }
        "import_group" => p.import_group = flag()?,
        "import_select" => p.import_select = flag()?,
        "import_tool_layers" => p.import_tool_layers = flag()?,
        "theme" => p.theme = value.as_str().ok_or("theme needs text")?.to_string(),
        "page_width" => p.page_width = number()?.clamp(1.0, 10_000.0) as f32,
        "page_height" => p.page_height = number()?.clamp(1.0, 10_000.0) as f32,
        "autosave_minutes" => p.autosave_minutes = number()?.clamp(0.0, 240.0) as f32,
        "svg_dpi" => p.svg_dpi = if number()? <= 72.0 { 72.0 } else { 96.0 },
        other => return crate::extras::set_option(app, other, value).map(|_| json!({ "ok": true })),
    }
    if matches!(key, "page_width" | "page_height") {
        crate::machine_ui::work_area_changed(app);
    }
    Ok(json!({ "ok": true }))
}

/// A summary of the open project for `project.info`.
fn info(app: &DarkBurn) -> Value {
    let e = &app.editor;
    let doc = &e.doc;
    let bbox = doc.bbox().map(|b| json!([b.x0, b.y0, b.x1, b.y1]));
    json!({
        "path": app.project_path.as_ref().map(|p| p.display().to_string()),
        "modified": e.is_modified(),
        "shapes": doc.iter().map(|s| json!({
            "id": s.id.0, "kind": s.kind_name(), "layer": s.layer, "name": s.name, "locked": s.locked,
            "bbox": [s.bbox().x0, s.bbox().y0, s.bbox().x1, s.bbox().y1],
        })).collect::<Vec<_>>(),
        "selected": e.selected().iter().map(|i| i.0).collect::<Vec<_>>(),
        "used_layers": doc.used_layers(),
        "current_layer": e.current_layer,
        "bbox": bbox,
        "can_undo": e.history.can_undo(),
        "can_redo": e.history.can_redo(),
        "page": [app.prefs.page_width, app.prefs.page_height],
        "tool": format!("{:?}", app.tool),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prefs::Prefs;

    fn app() -> DarkBurn {
        DarkBurn::new(Prefs::default())
    }

    fn run(app: &mut DarkBurn, id: &str, args: Value) -> Result<Value, String> {
        execute_headless(app, id, &args)
    }

    #[test]
    fn an_agent_can_build_and_edit_a_design_without_a_window() {
        let mut app = app();
        let r = run(&mut app, "shape.add_rect", json!({ "x": 10, "y": 10, "width": 40, "height": 20, "layer": 2 })).unwrap();
        let id = r["id"].as_u64().unwrap();
        run(&mut app, "shape.add_ellipse", json!({ "x": 100, "y": 10, "width": 30, "height": 30 })).unwrap();
        run(&mut app, "selection.set", json!({ "ids": [id] })).unwrap();
        run(&mut app, "shape.place", json!({ "anchor": "bottom_left", "x": 0, "y": 0 })).unwrap();
        run(&mut app, "shape.rotate", json!({ "degrees": 90 })).unwrap();
        let info = run(&mut app, "project.info", Value::Null).unwrap();
        assert_eq!(info["shapes"].as_array().unwrap().len(), 2);
        let bb = &info["shapes"][0]["bbox"];
        assert!(((bb[2].as_f64().unwrap() - bb[0].as_f64().unwrap()) - 20.0).abs() < 1e-6, "rotated 40 x 20 rectangle is 20 wide: {bb}");
        run(&mut app, "layer.set_mode", json!({ "layer": 2, "mode": "fill" })).unwrap();
        run(&mut app, "layer.set_param", json!({ "layer": 2, "field": "speed", "value": 250 })).unwrap();
        assert_eq!(app.editor.doc.layer(2).main().speed, 250.0);
        assert_eq!(app.editor.doc.layer(2).main().mode, Mode::Fill);
        run(&mut app, "edit.undo", Value::Null).unwrap();
        assert_eq!(app.editor.doc.layer(2).main().speed, 100.0);
        run(&mut app, "edit.select_all", Value::Null).unwrap();
        run(&mut app, "arrange.group", Value::Null).unwrap();
        assert_eq!(app.editor.doc.len(), 1);
        run(&mut app, "setting.set", json!({ "key": "filled", "value": false })).unwrap();
        assert!(!app.prefs.filled);
    }

    #[test]
    fn bad_requests_say_why() {
        let mut app = app();
        assert!(run(&mut app, "no.such", Value::Null).unwrap_err().contains("unknown"));
        assert!(run(&mut app, "arrange.to_laser", Value::Null).unwrap_err().contains("laser"));
        assert!(run(&mut app, "tools.resize_slots", Value::Null).is_err());
        assert!(run(&mut app, "shape.add_rect", json!({ "x": 0 })).unwrap_err().contains("y"));
        assert!(run(&mut app, "shape.rotate", json!({ "degrees": 5 })).unwrap_err().contains("selected"));
        assert!(run(&mut app, "layer.set_param", json!({ "layer": 99, "field": "speed", "value": 1 })).is_err());
        assert!(run(&mut app, "layer.set_param", json!({ "layer": 30, "field": "output", "value": true })).is_err());
        assert!(run(&mut app, "edit.undo", Value::Null).is_err(), "nothing to undo is refused");
        assert!(run(&mut app, "shape.add_path", json!({ "svg_path": "not a path" })).is_err());
    }

    #[test]
    fn projects_save_open_and_import_by_path() {
        let dir = std::env::temp_dir().join(format!("darkburn-api-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.dbrn");
        let mut app = app();
        run(&mut app, "shape.add_polygon", json!({ "cx": 50, "cy": 50, "sides": 5, "radius": 10 })).unwrap();
        run(&mut app, "project.save_as", json!({ "path": file.to_string_lossy() })).unwrap();
        let mut other = DarkBurn::new(Prefs::default());
        let info = run(&mut other, "project.open", json!({ "path": file.to_string_lossy() })).unwrap();
        assert_eq!(info["shapes"][0]["kind"], "Polygon");
        let svg = dir.join("b.svg");
        std::fs::write(
            &svg,
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="20mm" height="10mm"><rect width="20" height="10" fill="none" stroke="red"/></svg>"#,
        )
        .unwrap();
        let r = run(&mut other, "project.import", json!({ "paths": [svg.to_string_lossy()] })).unwrap();
        assert!(!r["imported"].as_array().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
