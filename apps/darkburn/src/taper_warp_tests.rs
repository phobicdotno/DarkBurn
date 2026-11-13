//! The taper warp window driven through the whole UI (OTHER-10).

use darkburn_core::ShapeKind;
use egui::{Key, Modifiers};
use serde_json::json;

use crate::taper_warp::{self, ID};
use crate::ui_harness::Gui;

fn is_rect(g: &Gui, id: u64) -> bool {
    matches!(g.app().editor.doc.get(darkburn_core::ShapeId(id)).unwrap().kind, ShapeKind::Rect { .. })
}

#[test]
fn the_window_previews_the_warp_and_ok_replaces_the_shapes_in_one_undo_step() {
    let mut g = Gui::standard();
    g.frames(2);
    let page = g.app().prefs.page();
    // Artwork in the right half of the page, away from the window at the top left.
    let (x0, y0) = (page.x as f64 * 0.55, page.y as f64 * 0.3);
    let id = g.exec("shape.add_rect", json!({ "x": x0, "y": y0, "width": 80, "height": 50, "layer": 0 }))["id"].as_u64().unwrap();
    let ellipse =
        g.exec("shape.add_ellipse", json!({ "x": x0 + 20.0, "y": y0 + 10.0, "width": 40, "height": 30, "layer": 0 }))["id"].as_u64().unwrap();
    g.exec("selection.set", json!({ "ids": [id, ellipse] }));
    g.run_command(ID);
    g.frames(2);
    assert!(g.window_titles().iter().any(|t| t == "Taper Warp"), "{:?}", g.window_titles());

    // The diameters of a cup, then the cone mode: the preview follows the fields.
    g.click_id("dialog:control:Top diameter");
    g.key(Key::A, Modifiers::COMMAND);
    g.type_text("90");
    g.enter();
    g.frames(2);
    assert_eq!(taper_warp::form(&g.ctx()).top_d, 90.0);
    g.click_label("Cone").frames(2);
    assert!(taper_warp::form(&g.ctx()).cone);
    let p = taper_warp::current_preview(&g.ctx()).expect("a preview");
    let p = p.as_ref().as_ref().expect("the sizes fit");
    assert_eq!(p.paths.len(), 2);
    assert!((p.taper.top - 80.0).abs() < 1e-9 && p.taper.bottom < 80.0, "{:?}", p.taper);
    assert!(g.has_text("Top edge 80.00 mm"), "{:?}", g.visible_texts());
    assert!(is_rect(&g, id), "the preview changes nothing");
    let shot = g.screenshot("taper_warp");
    assert!(shot.exists());
    assert!(g.layout_problems().is_empty(), "{:?}", g.layout_problems());

    // OK: both shapes become paths in one undo step, and the window closes.
    let steps = g.app().editor.history.len();
    g.click_id("dialog:ok").frames(2);
    assert_eq!(g.app().editor.history.len(), steps + 1);
    assert!(!is_rect(&g, id));
    assert!(!g.window_titles().iter().any(|t| t == "Taper Warp"));
    assert!(taper_warp::current_preview(&g.ctx()).is_none());
    g.exec("edit.undo", json!({}));
    assert!(is_rect(&g, id), "one undo brings the artwork back");
}

#[test]
fn without_a_selection_the_window_says_so_and_cancel_closes_it() {
    let mut g = Gui::standard();
    g.frames(2);
    g.exec("selection.set", json!({ "ids": [] }));
    g.run_command(ID);
    g.frames(2);
    assert!(g.has_text("Select the shapes to warp."), "{:?}", g.visible_texts());
    let steps = g.app().editor.history.len();
    g.click_id("dialog:ok").frames(2);
    assert!(g.window_titles().iter().any(|t| t == "Taper Warp"), "OK does nothing without shapes");
    g.click_id("dialog:cancel").frames(2);
    assert!(!g.window_titles().iter().any(|t| t == "Taper Warp"));
    assert_eq!(g.app().editor.history.len(), steps);
}

#[test]
fn widths_open_at_the_artwork_size_and_esc_closes_without_change() {
    let mut g = Gui::standard();
    g.frames(2);
    let page = g.app().prefs.page();
    let (x0, y0) = (page.x as f64 * 0.55, page.y as f64 * 0.3);
    let id = g.exec("shape.add_rect", json!({ "x": x0, "y": y0, "width": 60, "height": 40, "layer": 0 }))["id"].as_u64().unwrap();
    g.exec("selection.set", json!({ "ids": [id] }));
    g.run_command(ID);
    g.frames(2);
    g.click_label("Artwork widths").frames(2);
    let f = taper_warp::form(&g.ctx());
    assert!(f.widths);
    assert_eq!((f.top_w, f.bottom_w, f.art_h), (60.0, 60.0, 40.0));
    assert!(g.has_text("These sizes leave the artwork as it is."), "{:?}", g.visible_texts());
    g.escape().frames(2);
    assert!(!g.window_titles().iter().any(|t| t == "Taper Warp"));
    assert!(is_rect(&g, id));
}

#[test]
fn untouched_widths_follow_the_selection_and_typed_ones_stay() {
    let mut g = Gui::standard();
    g.frames(2);
    let page = g.app().prefs.page();
    let (x0, y0) = (page.x as f64 * 0.55, page.y as f64 * 0.3);
    let first = g.exec("shape.add_rect", json!({ "x": x0, "y": y0, "width": 60, "height": 40, "layer": 0 }))["id"].as_u64().unwrap();
    let second = g.exec("shape.add_rect", json!({ "x": x0, "y": y0 + 60.0, "width": 100, "height": 20, "layer": 0 }))["id"].as_u64().unwrap();
    g.exec("selection.set", json!({ "ids": [first] }));
    g.run_command(ID);
    g.frames(2);
    g.click_label("Artwork widths").frames(2);
    assert_eq!(taper_warp::form(&g.ctx()).top_w, 60.0);

    // Another shape is picked while the window is open: the untouched widths follow it, so OK
    // leaves it as it is.
    g.exec("selection.set", json!({ "ids": [second] }));
    g.frames(2);
    let f = taper_warp::form(&g.ctx());
    assert_eq!((f.top_w, f.bottom_w, f.art_h), (100.0, 100.0, 20.0));
    assert!(g.has_text("These sizes leave the artwork as it is."), "{:?}", g.visible_texts());
    let steps = g.app().editor.history.len();
    g.click_id("dialog:ok").frames(2);
    assert_eq!(g.app().editor.history.len(), steps, "nothing to change");
    assert!(is_rect(&g, second));

    // A width the person typed is kept when the selection changes.
    g.run_command(ID);
    g.frames(2);
    g.click_id("dialog:control:Bottom width");
    g.key(Key::A, Modifiers::COMMAND);
    g.type_text("50");
    g.enter();
    g.frames(2);
    assert_eq!(taper_warp::form(&g.ctx()).bottom_w, 50.0);
    g.exec("selection.set", json!({ "ids": [first] }));
    g.frames(2);
    let f = taper_warp::form(&g.ctx());
    assert_eq!((f.top_w, f.bottom_w, f.art_h), (100.0, 50.0, 20.0));
    g.close_dialogs();
}
