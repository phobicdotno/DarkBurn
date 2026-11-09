//! Position Laser (TOOLS-09, DRAW-20) and the two arrange commands that tie the selection to the
//! head (ARR-28, ARR-29): click the canvas to move the head there, move the head to one of the nine
//! points of the selection, or move the selection to where the head is.
//!
//! Moves go through `machine.move_to`, so they reach whatever the Laser window is connected to
//! (the simulator unless the person connected a real machine) and are refused when nothing is
//! connected. Design coordinates are bed coordinates when the job starts from absolute
//! coordinates; the machine's origin corner decides which way X and Y run.

use darkburn_core::arrange::Anchor;
use darkburn_core::kurbo::{Point, Vec2};
use serde_json::{Value, json};

use crate::job_build::StartMode;
use crate::shell::DarkBurn;
use crate::tools::Tool;

/// Whether `id` is one of ours.
pub fn handles(id: &str) -> bool {
    id == "tools.position_laser" || id == "arrange.to_laser" || anchor_of(id).is_some()
}

fn anchor_of(id: &str) -> Option<Anchor> {
    Some(match id.strip_prefix("arrange.laser_")? {
        "centre" => Anchor::Centre,
        "upper_left" => Anchor::TopLeft,
        "top" => Anchor::Top,
        "upper_right" => Anchor::TopRight,
        "left" => Anchor::Left,
        "right" => Anchor::Right,
        "lower_left" => Anchor::BottomLeft,
        "bottom" => Anchor::Bottom,
        "lower_right" => Anchor::BottomRight,
        _ => return None,
    })
}

fn anchor_arg(a: &Value) -> Result<Anchor, String> {
    Ok(match a.get("anchor").and_then(Value::as_str).unwrap_or("centre") {
        "centre" | "center" => Anchor::Centre,
        "top_left" => Anchor::TopLeft,
        "top" => Anchor::Top,
        "top_right" => Anchor::TopRight,
        "left" => Anchor::Left,
        "right" => Anchor::Right,
        "bottom_left" => Anchor::BottomLeft,
        "bottom" => Anchor::Bottom,
        "bottom_right" => Anchor::BottomRight,
        other => return Err(format!("unknown anchor {other}")),
    })
}

/// Machine coordinates of design point `p` (the origin corner decides the axis directions).
pub fn to_machine(app: &DarkBurn, p: Point) -> Point {
    let o = app.prefs.origin;
    Point::new(
        if o.flips_x() { f64::from(app.prefs.page_width) - p.x } else { p.x },
        if o.flips_y() { p.y } else { f64::from(app.prefs.page_height) - p.y },
    )
}

/// The design point of machine coordinates `m` (the mapping is its own inverse).
pub fn from_machine(app: &DarkBurn, m: Point) -> Point {
    to_machine(app, m)
}

fn absolute(app: &DarkBurn) -> bool {
    crate::job_cmds::current_settings(app).start_from == StartMode::Absolute
}

/// Move the head to design point `p`.
pub fn move_head_to(app: &mut DarkBurn, p: Point) -> Result<Value, String> {
    let m = to_machine(app, p);
    let args = json!({ "x": m.x, "y": m.y, "machine_coords": absolute(app) });
    crate::machine_ui::api(app, "machine.move_to", &args).unwrap_or_else(|| Err("moving the laser is not available".into()))
}

/// The head's position as a design point: the last position the machine reported.
fn head_point(app: &DarkBurn) -> Result<Point, String> {
    let snap = app.machine.handle.snapshot();
    if !snap.has_position {
        return Err("the laser has not reported its position yet; connect it and use Get position".into());
    }
    let pos = if absolute(app) { snap.machine_pos } else { snap.work_pos };
    Ok(from_machine(app, Point::new(pos[0], pos[1])))
}

fn selection_anchor(app: &DarkBurn, anchor: Anchor) -> Result<Point, String> {
    let ids = app.editor.selected();
    let bb = app.editor.doc.bbox_of(&ids).ok_or("nothing is selected")?;
    Ok(anchor.of(bb))
}

/// The agent path: the commands here move or read the machine, so they sit before the usual
/// hardware refusal; `machine.move_to` still refuses when nothing is connected.
pub fn api(app: &mut DarkBurn, id: &str, args: &Value) -> Option<Result<Value, String>> {
    if !handles(id) {
        return None;
    }
    Some(match id {
        "tools.position_laser" => {
            app.set_tool(Tool::PositionLaser);
            Ok(json!({ "changed": false, "tool": "position_laser" }))
        }
        "arrange.to_laser" => (|| {
            let anchor = anchor_arg(args)?;
            let head = head_point(app)?;
            let at = selection_anchor(app, anchor)?;
            let ids = app.editor.selected();
            let d: Vec2 = head - at;
            let done = app.editor.edit("Move to laser position", |doc, _| darkburn_core::arrange::translate(doc, &ids, d));
            Ok(json!({ "changed": done, "dx": d.x, "dy": d.y }))
        })(),
        _ => (|| {
            let anchor = anchor_of(id).ok_or("unknown anchor")?;
            let p = selection_anchor(app, anchor)?;
            move_head_to(app, p)
        })(),
    })
}

/// Run from a menu or key.
pub fn run(app: &mut DarkBurn, _ctx: &egui::Context, id: &str) -> bool {
    if !handles(id) {
        return false;
    }
    if let Some(Err(m)) = api(app, id, &json!({})) {
        app.shape_ui.status = Some(m);
    }
    true
}

/// The tool's click: move the head to the clicked point.
pub fn interact(app: &mut DarkBurn, response: &egui::Response, raw: Option<Point>) {
    let Some(p) = raw else { return };
    if response.clicked_by(egui::PointerButton::Primary)
        && let Err(m) = move_head_to(app, p)
    {
        app.shape_ui.status = Some(m);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::execute_headless;
    use crate::devices::DeviceType;
    use crate::machine_api::LaserState;
    use crate::prefs::Prefs;
    use std::time::{Duration, Instant};

    fn run(app: &mut DarkBurn, id: &str, args: Value) -> Result<Value, String> {
        execute_headless(app, id, &args)
    }

    fn wait(app: &mut DarkBurn, what: &str, mut f: impl FnMut(&DarkBurn) -> bool) {
        let end = Instant::now() + Duration::from_secs(20);
        loop {
            app.machine.tick();
            if f(app) {
                return;
            }
            assert!(Instant::now() < end, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn connected() -> DarkBurn {
        let mut a = DarkBurn::new(Prefs::default());
        let id = a.machine.devices.devices.iter().find(|d| d.kind == DeviceType::SimulatorGrbl).unwrap().id;
        run(&mut a, "device.select", json!({ "id": id })).unwrap();
        run(&mut a, "machine.connect", json!({})).unwrap();
        wait(&mut a, "ready", |a| a.machine.handle.snapshot().state == LaserState::Ready);
        a
    }

    #[test]
    fn nothing_connected_means_a_clear_refusal() {
        let mut a = DarkBurn::new(Prefs::default());
        run(&mut a, "shape.add_rect", json!({ "x": 10, "y": 10, "width": 20, "height": 20, "layer": 0 })).unwrap();
        run(&mut a, "selection.set", json!({ "ids": [1] })).unwrap();
        assert!(run(&mut a, "arrange.laser_centre", json!({})).is_err());
        assert!(run(&mut a, "arrange.to_laser", json!({})).unwrap_err().contains("position"));
    }

    #[test]
    fn the_head_goes_to_the_nine_points_of_the_selection_on_the_simulator() {
        let mut a = connected();
        let r = run(&mut a, "shape.add_rect", json!({ "x": 40, "y": 30, "width": 20, "height": 10, "layer": 0 })).unwrap();
        run(&mut a, "selection.set", json!({ "ids": [r["id"]] })).unwrap();
        for (id, x, y) in [
            ("arrange.laser_centre", 50.0, 35.0),
            ("arrange.laser_upper_left", 40.0, 40.0),
            ("arrange.laser_lower_right", 60.0, 30.0),
            ("arrange.laser_top", 50.0, 40.0),
        ] {
            run(&mut a, id, json!({})).unwrap();
            wait(&mut a, id, |a| {
                let p = a.machine.handle.snapshot().machine_pos;
                (p[0] - x).abs() < 0.05 && (p[1] - y).abs() < 0.05
            });
            wait(&mut a, "idle", |a| a.machine.handle.snapshot().state == LaserState::Ready);
        }
    }

    #[test]
    fn the_selection_moves_to_where_the_head_is() {
        let mut a = connected();
        run(&mut a, "machine.move_to", json!({ "x": 100.0, "y": 80.0, "machine_coords": true, "feed": 6000 })).unwrap();
        wait(&mut a, "moved", |a| (a.machine.handle.snapshot().machine_pos[0] - 100.0).abs() < 0.05);
        wait(&mut a, "idle", |a| a.machine.handle.snapshot().state == LaserState::Ready);
        run(&mut a, "machine.get_position", json!({})).unwrap();
        wait(&mut a, "position known", |a| a.machine.handle.snapshot().has_position);
        let r = run(&mut a, "shape.add_rect", json!({ "x": 10, "y": 10, "width": 20, "height": 10, "layer": 0 })).unwrap();
        let id = darkburn_core::ShapeId(r["id"].as_u64().unwrap());
        run(&mut a, "selection.set", json!({ "ids": [id.0] })).unwrap();
        run(&mut a, "arrange.to_laser", json!({ "anchor": "bottom_left" })).unwrap();
        let bb = a.editor.doc.get(id).unwrap().bbox();
        assert!((bb.x0 - 100.0).abs() < 0.05 && (bb.y0 - 80.0).abs() < 0.05, "{bb:?}");
        // And the origin corner flips the axes.
        a.prefs.origin = crate::view::Origin::FrontRight;
        let m = to_machine(&a, Point::new(10.0, 20.0));
        assert_eq!(m, Point::new(f64::from(a.prefs.page_width) - 10.0, 20.0));
    }

    #[test]
    fn the_position_laser_tool_moves_the_head_on_a_click() {
        let mut a = connected();
        run(&mut a, "tools.position_laser", json!({})).unwrap();
        assert_eq!(a.tool, Tool::PositionLaser);
        move_head_to(&mut a, Point::new(55.0, 45.0)).unwrap();
        wait(&mut a, "head there", |a| {
            let p = a.machine.handle.snapshot().machine_pos;
            (p[0] - 55.0).abs() < 0.05 && (p[1] - 45.0).abs() < 0.05
        });
    }
}
