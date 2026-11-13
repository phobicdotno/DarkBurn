//! The tangent circle tool driven through the whole UI (DRAW-22).

use darkburn_core::ShapeKind;
use darkburn_core::kurbo::Point;
use egui::Pos2;
use serde_json::json;

use crate::render::Xf;
use crate::tangent_tool::{self, TOOL};
use crate::tools::Tool;
use crate::ui_harness::Gui;

/// Where design point `p` (mm, Y up) is on the screen, checked to be on the canvas and clear of
/// the tool's window.
fn at(g: &Gui, p: Point) -> Pos2 {
    let canvas = g.canvas_rect();
    let s = {
        let app = g.app();
        Xf::new(canvas, &app.prefs.view, app.prefs.page().y).to_screen(p)
    };
    assert!(canvas.shrink(10.0).contains(s), "{p:?} is at {s:?}, off the canvas {canvas:?}");
    for w in g.dialog_windows() {
        assert!(!w.contains(s), "{p:?} at {s:?} is under the window {w:?}");
    }
    s
}

fn circle_count(g: &Gui) -> usize {
    g.app().editor.doc.iter().filter(|s| matches!(s.kind, ShapeKind::Ellipse { .. })).count()
}

#[test]
fn picking_two_edges_shows_the_candidates_and_adds_the_chosen_circle() {
    let mut g = Gui::standard();
    g.frames(2);
    let page = g.app().prefs.page();
    // A rectangle in the right half of the page, away from the window at the top left.
    let (x0, y0) = (page.x as f64 * 0.5, page.y as f64 * 0.3);
    g.exec("shape.add_rect", json!({ "x": x0, "y": y0, "width": 100, "height": 80, "layer": 0 }));
    g.exec("selection.set", json!({ "ids": [] }));
    g.run_command(TOOL);
    assert_eq!(g.app().tool, Tool::TangentCircle);
    assert!(g.window_titles().iter().any(|t| t == "Tangent Circle"), "{:?}", g.window_titles());

    // The left edge, then the bottom edge.
    let left = at(&g, Point::new(x0, y0 + 30.0));
    g.click_pos(left).frames(2);
    let bottom = at(&g, Point::new(x0 + 40.0, y0));
    g.click_pos(bottom).frames(2);
    let st = tangent_tool::state(&g.ctx());
    assert_eq!(st.picks.len(), 2, "{:?}", st.message);
    assert_eq!(st.candidates().unwrap().len(), 1, "only the circle in the corner touches both edges themselves");

    // Show the circles on the extensions too: four of them, dashed, the corner one chosen.
    g.click_label("Only on the edges").frames(2);
    let cands = tangent_tool::state(&g.ctx()).candidates().unwrap();
    assert_eq!(cands.len(), 4);
    let corner = Point::new(x0 + 10.0, y0 + 10.0);
    assert!(cands[0].circle.center.distance(corner) < 1e-6, "{:?}", cands[0]);
    assert!(g.has_text("Add All"));
    let shot = g.screenshot("tangent_circle");
    assert!(shot.exists());
    assert!(g.layout_problems().is_empty(), "{:?}", g.layout_problems());

    // Add the chosen one: one circle, one undo step, the picks cleared, the tool still on.
    let undo = g.app().editor.history.len();
    g.click_label("Add Circle").frames(2);
    assert_eq!(circle_count(&g), 1);
    assert_eq!(g.app().editor.history.len(), undo + 1);
    assert!(tangent_tool::state(&g.ctx()).picks.is_empty());
    assert_eq!(g.app().tool, Tool::TangentCircle);
    {
        let app = g.app();
        let id = app.editor.selected()[0];
        let b = app.editor.doc.get(id).unwrap().bbox();
        assert!(b.center().distance(corner) < 1e-6 && (b.width() - 20.0).abs() < 1e-6, "{b:?}");
    }

    // Esc closes the window and goes back to the select tool.
    g.escape().frames(2);
    assert_eq!(g.app().tool, Tool::Select);
    assert!(!g.window_titles().iter().any(|t| t == "Tangent Circle"));
}

#[test]
fn three_edges_and_add_all_from_the_window() {
    let mut g = Gui::standard();
    g.frames(2);
    let page = g.app().prefs.page();
    let (x0, y0) = (page.x as f64 * 0.5, page.y as f64 * 0.3);
    g.exec("shape.add_rect", json!({ "x": x0, "y": y0, "width": 100, "height": 80, "layer": 0 }));
    g.run_command(TOOL);
    for p in [Point::new(x0, y0 + 30.0), Point::new(x0 + 40.0, y0), Point::new(x0 + 100.0, y0 + 30.0)] {
        let s = at(&g, p);
        g.click_pos(s).frames(2);
    }
    let cands = tangent_tool::state(&g.ctx()).candidates().unwrap();
    assert_eq!(cands.len(), 1, "{cands:?}");
    assert!((cands[0].circle.radius - 50.0).abs() < 1e-6);
    // A click on nothing says so and keeps the picks.
    let empty = at(&g, Point::new(x0 + 50.0, y0 + 40.0));
    g.click_pos(empty).frames(2);
    let st = tangent_tool::state(&g.ctx());
    assert_eq!(st.picks.len(), 3);
    assert!(st.message.is_some());
    g.click_label("Add Circle").frames(2);
    assert_eq!(circle_count(&g), 1);
}
