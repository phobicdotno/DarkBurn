//! Bug hunt 10, the document and editor side: each test failed before its fix.
use darkburn_core::kurbo::{Point, Rect};
use darkburn_core::{ShapeId, ShapeKind};
use serde_json::{Value, json};

use crate::shape_ops::tests::{add_rect, app, run, select};
use crate::shell::DarkBurn;
use crate::ui_harness::Gui;
use egui::{Key, Modifiers};

fn id_of(v: &Value) -> ShapeId {
    ShapeId(v["id"].as_u64().expect("an id"))
}

fn bbox(a: &DarkBurn, id: ShapeId) -> Rect {
    a.editor.doc.get(id).expect("the shape").bbox()
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6
}

/// "Select Shapes Smaller than Selected" only changes the selection: a saved project stays saved.
#[test]
fn select_smaller_does_not_mark_the_project_as_changed() {
    let mut a = app();
    let big = add_rect(&mut a, 0.0, 0.0, 50.0, 50.0, 0);
    let small = add_rect(&mut a, 100.0, 0.0, 5.0, 5.0, 0);
    a.editor.mark_saved();
    select(&mut a, &[big]);
    run(&mut a, "tools.select_smaller", json!({})).unwrap();
    assert_eq!(a.editor.selected(), vec![small]);
    assert!(!a.editor.is_modified(), "picking shapes is not a change to the project");
}

/// Warp works in the document: a group that was moved after it was made is warped where it
/// lies, not where it was when it was grouped. Warping to the corners the selection already has
/// leaves it as it is.
#[test]
fn warp_of_a_moved_group_works_where_the_group_lies() {
    let mut a = app();
    let r1 = add_rect(&mut a, 0.0, 0.0, 10.0, 10.0, 0);
    let r2 = add_rect(&mut a, 20.0, 0.0, 10.0, 20.0, 0);
    select(&mut a, &[r1, r2]);
    run(&mut a, "arrange.group", json!({})).unwrap();
    let g = a.editor.selected()[0];
    run(&mut a, "shape.translate", json!({ "dx": 100.0, "dy": 50.0 })).unwrap();
    let before = bbox(&a, g);
    assert!(near(before.x0, 100.0) && near(before.y0, 50.0) && near(before.x1, 130.0) && near(before.y1, 70.0), "{before:?}");
    // The same corners: nothing moves.
    let same = json!({ "corners": [[100, 50], [130, 50], [130, 70], [100, 70]], "mode": "bilinear" });
    run(&mut a, "tools.warp", same).unwrap();
    let after = bbox(&a, g);
    assert!((after.x0 - 100.0).abs() < 0.05 && (after.y0 - 50.0).abs() < 0.05, "{after:?}");
    assert!((after.x1 - 130.0).abs() < 0.05 && (after.y1 - 70.0).abs() < 0.05, "{after:?}");
    // Twice as wide to the right: the left edge stays, the right edge goes to 160.
    let wide = json!({ "corners": [[100, 50], [160, 50], [160, 70], [100, 70]], "mode": "bilinear" });
    run(&mut a, "tools.warp", wide).unwrap();
    let after = bbox(&a, g);
    assert!((after.x0 - 100.0).abs() < 0.05 && (after.x1 - 160.0).abs() < 0.05, "{after:?}");
    assert!((after.y0 - 50.0).abs() < 0.05 && (after.y1 - 70.0).abs() < 0.05, "{after:?}");
}

/// Break Apart of a group that holds a picture frees the picture; it used to be thrown away
/// with the group.
#[test]
fn break_apart_keeps_the_picture_of_a_group() {
    use crate::images::testutil::{add_image, png};
    let mut a = app();
    let r = add_rect(&mut a, 0.0, 0.0, 10.0, 10.0, 0);
    let image = add_image(&mut a.editor.doc, png(4, 4, |_, _| 0), (4, 4), (10.0, 10.0), (30.0, 5.0), 3);
    select(&mut a, &[r, image]);
    run(&mut a, "arrange.group", json!({})).unwrap();
    run(&mut a, "arrange.break_apart", json!({})).unwrap();
    let pictures = a.editor.doc.iter().filter(|s| s.is_image()).count();
    assert_eq!(pictures, 1, "the picture is still in the project");
    assert_eq!(a.editor.doc.len(), 2);
    let pic = a.editor.doc.iter().find(|s| s.is_image()).unwrap().bbox();
    assert!(near(pic.x0, 25.0) && near(pic.x1, 35.0) && near(pic.y0, 0.0), "{pic:?}");
}

/// A start point set on a path that was moved since it was drawn stays where it was clicked.
#[test]
fn a_start_point_on_a_moved_path_is_where_it_was_clicked() {
    let mut a = app();
    let p = id_of(&run(&mut a, "shape.add_path", json!({ "svg_path": "M0 0 L20 0 L20 20 L0 20 Z" })).unwrap());
    select(&mut a, &[p]);
    run(&mut a, "shape.translate", json!({ "dx": 100.0, "dy": 0.0 })).unwrap();
    run(&mut a, "shape.set_start_point", json!({ "x": 120.0, "y": 10.0 })).unwrap();
    let s = a.editor.doc.get(p).unwrap();
    let start = s.start_point.expect("a start point");
    let at = s.transform * Point::new(start[0], start[1]);
    assert!(near(at.x, 120.0) && near(at.y, 10.0), "the start point is at {at:?}");
    let marker = crate::shape_nodes::marker_path(&a.editor.doc, p).unwrap();
    let first = darkburn_geom::path::start_point(&marker).unwrap();
    assert!(near(first.x, 120.0) && near(first.y, 10.0), "the cut starts at {first:?}");
}

/// The corner radius typed for a virtual array of rectangles is still there after the array is
/// edited again.
#[test]
fn a_corner_radius_given_to_an_array_survives_editing_the_array() {
    let mut a = app();
    let r = add_rect(&mut a, 0.0, 0.0, 10.0, 10.0, 0);
    select(&mut a, &[r]);
    run(&mut a, "arrange.grid_array", json!({ "rows": 1, "cols": 2, "dx": 5, "dy": 5, "virtual": true })).unwrap();
    let arr = a.editor.selected()[0];
    assert!(a.editor.doc.get(arr).unwrap().array.is_some(), "a virtual array");
    run(&mut a, "shape.set_corner_radius", json!({ "radius": 2.0 })).unwrap();
    let radii = |a: &DarkBurn| -> Vec<f64> {
        let arr = a.editor.selected()[0];
        a.editor
            .doc
            .get(arr)
            .unwrap()
            .leaves()
            .iter()
            .filter_map(|l| if let ShapeKind::Rect { radius, .. } = l.kind { Some(radius) } else { None })
            .collect()
    };
    assert_eq!(radii(&a), vec![2.0, 2.0]);
    run(&mut a, "arrange.array_edit", json!({ "cols": 3 })).unwrap();
    assert_eq!(radii(&a), vec![2.0, 2.0, 2.0], "the corners are still round");
}

/// A drag on a whole-number field ("Passes") whose first frames round to the old value: the
/// frame that changes the value makes an undo step.
#[test]
fn a_drag_that_first_changes_nothing_still_gets_its_undo_step() {
    let ctx = egui::Context::default();
    let mut e = darkburn_core::Editor::new();
    let key = egui::Id::unique("passes");
    let before = e.doc.layer(0).main().passes;
    for passes in [before, before, before + 1, before + 2] {
        crate::numeric::commit(&mut e, &ctx, key, "Passes", true, |d, _| {
            let s = d.layer_mut(0).main_mut();
            let changed = s.passes != passes;
            s.passes = passes;
            changed
        });
    }
    assert_eq!(e.doc.layer(0).main().passes, before + 2);
    assert_eq!(e.history.len(), 1, "the drag is one undo step");
    e.undo();
    assert_eq!(e.doc.layer(0).main().passes, before, "undo takes the drag back");
}

/// A corner handle of the box around a straight line (a box with no height) makes the line
/// shorter as well as longer.
#[test]
fn a_corner_handle_shortens_a_straight_line() {
    use crate::tools::{DragKind, Handle, Mods, drag_transform};
    let line = Rect::new(0.0, 0.0, 50.0, 0.0);
    let grip = Handle { x: 1, y: 1 };
    let t = drag_transform(DragKind::Scale(grip), line, Point::new(50.0, 0.0), Point::new(25.0, 3.0), Mods::default());
    let end = t * Point::new(50.0, 0.0);
    assert!(near(end.x, 25.0) && near(end.y, 0.0), "the end of the line went to {end:?}");
    let upright = Rect::new(0.0, 0.0, 0.0, 50.0);
    let t = drag_transform(DragKind::Scale(grip), upright, Point::new(0.0, 50.0), Point::new(2.0, 10.0), Mods::default());
    let end = t * Point::new(0.0, 50.0);
    assert!(near(end.x, 0.0) && near(end.y, 10.0), "the end of the line went to {end:?}");
}

/// Two rectangles in a group that was then moved by (100, 50) and made twice as large.
fn moved_scaled_group(a: &mut DarkBurn) -> ShapeId {
    let r1 = add_rect(a, 0.0, 0.0, 10.0, 10.0, 0);
    let r2 = add_rect(a, 40.0, 0.0, 10.0, 10.0, 0);
    select(a, &[r1, r2]);
    run(a, "arrange.group", json!({})).unwrap();
    let g = a.editor.selected()[0];
    run(a, "shape.translate", json!({ "dx": 100.0, "dy": 50.0 })).unwrap();
    run(a, "shape.resize", json!({ "anchor": "bottom_left", "width": 100.0, "height": 20.0 })).unwrap();
    let b = bbox(a, g);
    assert!(near(b.x0, 100.0) && near(b.y0, 50.0) && near(b.x1, 200.0) && near(b.y1, 70.0), "{b:?}");
    g
}

/// Deform to the grid the selection already has leaves a moved group where it is.
#[test]
fn deform_of_a_moved_group_works_where_the_group_lies() {
    let mut a = app();
    let g = moved_scaled_group(&mut a);
    let mut grid = Vec::new();
    for i in 0..4 {
        for j in 0..4 {
            // The box of the group, its top edge pulled up by 10.
            grid.push(json!([100.0 + 100.0 * j as f64 / 3.0, 50.0 + 30.0 * i as f64 / 3.0]));
        }
    }
    run(&mut a, "tools.deform", json!({ "grid": grid })).unwrap();
    let b = bbox(&a, g);
    assert!((b.x0 - 100.0).abs() < 0.05 && (b.x1 - 200.0).abs() < 0.05, "{b:?}");
    assert!((b.y0 - 50.0).abs() < 0.05 && (b.y1 - 80.0).abs() < 0.05, "{b:?}");
}

/// Round Corners with 2 mm rounds by 2 mm, also in a group that was made larger.
#[test]
fn round_corners_in_a_scaled_group_uses_the_radius_asked_for() {
    let mut a = app();
    let g = moved_scaled_group(&mut a);
    run(&mut a, "tools.round_corners", json!({ "radius": 2.0 })).unwrap();
    // The first rectangle lies at (100, 50) and is 20 x 20: its bottom edge now starts 2 mm in.
    let first = a.editor.doc.get(g).unwrap().leaves()[0].outline();
    let on_bottom =
        first.elements().iter().filter_map(|el| el.end_point()).filter(|p| (p.y - 50.0).abs() < 1e-6).map(|p| p.x).fold(f64::INFINITY, f64::min);
    assert!((on_bottom - 102.0).abs() < 1e-3, "the bottom edge starts at x = {on_bottom}");
}

/// Resize Slots makes a slot as wide as asked, also in a group that was made larger.
#[test]
fn resize_slots_in_a_scaled_group_gives_the_width_asked_for() {
    let mut a = app();
    let plate = id_of(&run(&mut a, "shape.add_path", json!({ "svg_path": "M0 0 L40 0 L40 20 L22 20 L22 12 L18 12 L18 20 L0 20 Z" })).unwrap());
    let other = add_rect(&mut a, 60.0, 0.0, 10.0, 10.0, 0);
    select(&mut a, &[plate, other]);
    run(&mut a, "arrange.group", json!({})).unwrap();
    run(&mut a, "shape.resize", json!({ "anchor": "bottom_left", "width": 140.0, "height": 40.0 })).unwrap();
    let found = run(&mut a, "tools.resize_slots", json!({})).unwrap();
    assert!((found["slots"][0]["width"].as_f64().unwrap() - 8.0).abs() < 0.01, "{found}");
    run(&mut a, "tools.resize_slots", json!({ "new_width": 6 })).unwrap();
    let now = run(&mut a, "tools.resize_slots", json!({})).unwrap();
    assert!((now["slots"][0]["width"].as_f64().unwrap() - 6.0).abs() < 0.01, "{now}");
}

/// Weld of a group that holds a picture with another shape keeps the picture.
#[test]
fn weld_keeps_the_picture_of_a_group() {
    use crate::images::testutil::{add_image, png};
    let mut a = app();
    let r = add_rect(&mut a, 0.0, 0.0, 10.0, 10.0, 0);
    let image = add_image(&mut a.editor.doc, png(4, 4, |_, _| 0), (4, 4), (10.0, 10.0), (30.0, 5.0), 3);
    select(&mut a, &[r, image]);
    run(&mut a, "arrange.group", json!({})).unwrap();
    let g = a.editor.selected()[0];
    let other = add_rect(&mut a, 5.0, 5.0, 10.0, 10.0, 0);
    select(&mut a, &[g, other]);
    run(&mut a, "tools.weld", json!({})).unwrap();
    assert_eq!(a.editor.doc.iter().filter(|s| s.is_image()).count(), 1, "the picture is still in the project");
    assert_eq!(a.editor.doc.len(), 2, "the welded outline and the picture");
    assert!(a.editor.selected().iter().all(|i| !a.editor.doc.get(*i).unwrap().is_image()), "the result is selected, not the picture");
    run(&mut a, "edit.undo", json!({})).unwrap();
    assert_eq!(a.editor.doc.len(), 2, "the group and the other rectangle");
    assert!(a.editor.doc.get(g).is_some());
}

/// Offset with "delete original" of a group that holds a picture keeps the picture.
#[test]
fn offset_that_deletes_the_original_keeps_the_picture_of_a_group() {
    use crate::images::testutil::{add_image, png};
    let mut a = app();
    let r = add_rect(&mut a, 0.0, 0.0, 10.0, 10.0, 0);
    let image = add_image(&mut a.editor.doc, png(4, 4, |_, _| 0), (4, 4), (10.0, 10.0), (30.0, 5.0), 3);
    select(&mut a, &[r, image]);
    run(&mut a, "arrange.group", json!({})).unwrap();
    run(&mut a, "tools.offset", json!({ "distance": 1, "delete_original": true })).unwrap();
    assert_eq!(a.editor.doc.iter().filter(|s| s.is_image()).count(), 1, "the picture is still in the project");
    assert_eq!(a.editor.doc.len(), 2, "the offset outline and the picture");
}

/// A path and a square in a group made ten times as large (the path then starts at 0, 0).
fn path_in_a_group_ten_times_as_large(a: &mut DarkBurn, svg_path: &str) -> ShapeId {
    let path = id_of(&run(a, "shape.add_path", json!({ "svg_path": svg_path })).unwrap());
    let other = add_rect(a, 20.0, 0.0, 10.0, 10.0, 0);
    select(a, &[path, other]);
    run(a, "arrange.group", json!({})).unwrap();
    let g = a.editor.selected()[0];
    run(a, "shape.resize", json!({ "anchor": "bottom_left", "width": 300.0, "height": 100.0 })).unwrap();
    g
}

/// Optimize takes away only what lies within its tolerance in the document, also in a group
/// that was made larger: a corner that stands out by 0.05 mm stays with a tolerance of 0.01 mm.
#[test]
fn optimize_in_a_scaled_group_keeps_what_is_outside_the_tolerance() {
    let mut a = app();
    path_in_a_group_ten_times_as_large(&mut a, "M0 0 L5 0.005 L10 0 L10 10 L0 10 Z");
    let out = run(&mut a, "edit.optimize", json!({ "tolerance": 0.01 })).unwrap();
    assert_eq!(out["nodes_after"], out["nodes_before"], "{out}");
    assert_eq!(out["changed"], false, "{out}");
}

/// Close Selected Paths With Tolerance closes a gap of up to the tolerance in the document, also
/// in a group that was made larger: a gap of 0.5 mm stays open with a tolerance of 0.1 mm.
#[test]
fn close_with_tolerance_in_a_scaled_group_measures_the_gap_in_the_document() {
    let mut a = app();
    let g = path_in_a_group_ten_times_as_large(&mut a, "M0 0 L10 0 L10 10 L0 10 L0 0.05");
    let out = run(&mut a, "edit.close_tolerance", json!({ "tolerance": 0.1 })).unwrap();
    assert_eq!(out["closed"], 0, "{out}");
    assert!(!a.editor.doc.get(g).unwrap().leaves()[0].is_closed(), "the path is still open");
    // A gap inside the tolerance is closed.
    let out = run(&mut a, "edit.close_tolerance", json!({ "tolerance": 1.0 })).unwrap();
    assert_eq!(out["closed"], 1, "{out}");
}

fn show_panel(g: &mut Gui, panel: &str) {
    if !g.app().prefs.panel(panel).visible {
        g.run_command(&format!("panel.{panel}"));
    }
    let area = g.app().prefs.panel(panel).area;
    g.app().prefs.active_tab.insert(area, panel.to_string());
    g.frames(3);
}

/// A new project made with Ctrl+N while the Cut Settings Editor is open keeps its own layers
/// when the editor is closed: Cancel used to give it the layers of the project before.
#[test]
fn a_new_project_under_the_cut_settings_editor_keeps_its_own_layers() {
    let mut g = Gui::standard();
    g.sample_design();
    g.exec("layer.set_param", json!({ "layer": 1, "field": "speed", "value": 55 }));
    g.app().editor.mark_saved();
    g.app().cut_settings_open = Some(1);
    g.settle();
    assert_eq!(g.window_titles(), ["Cut Settings Editor"]);
    g.key(Key::N, Modifiers::COMMAND);
    g.frames(3);
    assert!(g.app().editor.doc.is_empty(), "Ctrl+N made a new project");
    assert_eq!(g.app().editor.doc.layer(1).main().speed, 100.0);
    if g.app().cut_settings_open.is_some() {
        g.click_id("dialog:cancel");
        g.frames(3);
    }
    assert!(g.app().cut_settings_open.is_none());
    assert_eq!(g.app().editor.doc.layer(1).main().speed, 100.0, "the new project has the speed of the old one");
    assert_eq!(g.app().editor.history.len(), 0, "a new project has nothing to undo");
    assert!(!g.app().editor.is_modified(), "a new project has no changes to save");
}

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in data.chunks(3) {
        let n = u32::from(c[0]) << 16 | u32::from(*c.get(1).unwrap_or(&0)) << 8 | u32::from(*c.get(2).unwrap_or(&0));
        for k in 0..4 {
            out.push(if k <= c.len() { T[(n >> (18 - 6 * k) & 63) as usize] as char } else { '=' });
        }
    }
    out
}

/// A picture that came in inside an SVG file has no file of its own: Refresh Image says so
/// instead of trying to read the SVG as a picture.
#[test]
fn a_picture_from_an_svg_file_does_not_take_the_svg_as_its_file() {
    let png = crate::images::testutil::png(4, 2, |_, _| 0);
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="96" height="96"><image x="0" y="0" width="96" height="48" xlink:href="data:image/png;base64,{}"/></svg>"#,
        base64(&png)
    );
    let dir = std::env::temp_dir().join(format!("darkburn-hunt10-svg-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("art.svg");
    std::fs::write(&file, svg).unwrap();
    let mut a = app();
    let out = run(&mut a, "project.import", json!({ "paths": [file.to_string_lossy()] })).unwrap();
    let id = ShapeId(out["imported"][0].as_u64().expect("one shape came in"));
    let ShapeKind::Image(img) = &a.editor.doc.get(id).unwrap().kind else { panic!("not a picture") };
    assert_eq!(img.source, None, "the SVG file is not the picture's file");
    select(&mut a, &[id]);
    let err = run(&mut a, "edit.image_refresh", json!({})).unwrap_err();
    assert!(err.contains("does not remember a file"), "{err}");
    // A picture file is still remembered.
    let picture = dir.join("p.png");
    std::fs::write(&picture, &png).unwrap();
    let out = run(&mut a, "project.import", json!({ "paths": [picture.to_string_lossy()] })).unwrap();
    let id = ShapeId(out["imported"][0].as_u64().unwrap());
    let ShapeKind::Image(img) = &a.editor.doc.get(id).unwrap().kind else { panic!("not a picture") };
    assert_eq!(img.source.as_deref(), Some(picture.to_string_lossy().as_ref()));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A name typed for one shape and then a name typed for another are two undo steps: one undo
/// takes back the second name only.
#[test]
fn names_typed_for_two_shapes_are_two_undo_steps() {
    let mut g = Gui::standard();
    g.sample_design();
    show_panel(&mut g, "shape_properties");
    let ids = g.app().editor.selected();
    let name = |g: &mut Gui, id: ShapeId, text: &str| {
        g.app().editor.selection.set([id]);
        g.frames(2);
        let field = g.find_a11y(Some("TextInput"), "Name").into_iter().next().unwrap_or_else(|| panic!("no Name field: {:?}", g.a11y()));
        g.click_pos(field.rect.center());
        g.type_text(text);
        g.frames(2);
        // Leave the field.
        g.key(Key::Tab, Modifiers::NONE);
        g.frames(2);
    };
    name(&mut g, ids[0], "one");
    name(&mut g, ids[1], "two");
    let names = |g: &Gui| -> Vec<String> { ids.iter().map(|i| g.app().editor.doc.get(*i).unwrap().name.clone()).collect() };
    assert_eq!(names(&g), ["one", "two"]);
    g.app().editor.undo();
    assert_eq!(names(&g), ["one", ""], "one undo took back both names");
}

/// Two drags on the same number field, one for each of two shapes, are two undo steps: one
/// undo takes back the second drag only.
#[test]
fn drags_on_one_field_for_two_shapes_are_two_undo_steps() {
    let mut g = Gui::standard();
    g.sample_design();
    show_panel(&mut g, "shape_properties");
    let ids = g.app().editor.selected();
    let drag = |g: &mut Gui, id: ShapeId| {
        g.app().editor.selection.set([id]);
        g.frames(2);
        let field =
            g.find_a11y(Some("SpinButton"), "Cut order priority").into_iter().next().unwrap_or_else(|| panic!("no priority field: {:?}", g.a11y()));
        let c = field.rect.center();
        // Slowly, as a drag begins: the number first changes some frames into the drag.
        g.drag_steps(c, c + egui::vec2(60.0, 0.0), 60);
    };
    drag(&mut g, ids[0]);
    drag(&mut g, ids[1]);
    let priorities = |g: &Gui| -> Vec<i32> { ids.iter().map(|i| g.app().editor.doc.get(*i).unwrap().priority).collect() };
    let both = priorities(&g);
    assert!(both[0] != 0 && both[1] != 0, "each drag changed its shape: {both:?}");
    g.app().editor.undo();
    assert_eq!(priorities(&g), [both[0], 0], "one undo took back both drags");
}

/// A cut library saved to a file, or opened from one, by command is the library of its device
/// from then on, as with the panel's buttons: the next start opens that file.
#[test]
fn a_cut_library_saved_or_opened_by_command_is_opened_at_the_next_start() {
    let dir = std::env::temp_dir().join(format!("darkburn-hunt10-lib-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("mine.dbclb");
    let mut a = app();
    run(&mut a, "library.create_from_layer", json!({ "layer": 1, "material": "Birch" })).unwrap();
    run(&mut a, "library.save_as", json!({ "path": file.to_string_lossy() })).unwrap();
    let device = a.extras.library.device.clone();
    assert_eq!(a.prefs.library_paths.get(&device).cloned(), Some(file.to_string_lossy().into_owned()), "Save as remembers the file");
    let mut next = DarkBurn::new(a.prefs.clone());
    let list = run(&mut next, "library.list", json!({})).unwrap();
    assert_eq!(list["entries"].as_array().map(Vec::len), Some(1), "{list}");

    let mut b = app();
    run(&mut b, "library.open", json!({ "path": file.to_string_lossy() })).unwrap();
    assert_eq!(b.prefs.library_paths.get(&device).cloned(), Some(file.to_string_lossy().into_owned()), "Open remembers the file");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A new project made with Ctrl+N while Adjust Image is open keeps its own layers when the
/// window is closed: Cancel used to give it the image layer of the project before, and mark it
/// as changed.
#[test]
fn a_new_project_under_adjust_image_keeps_its_own_layers() {
    let mut g = Gui::standard();
    let bytes = crate::images::testutil::png(8, 8, |x, _| (x * 30) as u8);
    let id = crate::images::testutil::add_image(&mut g.app().editor.doc, bytes, (8, 8), (20.0, 20.0), (50.0, 50.0), 3);
    g.app().editor.selection.set([id]);
    g.exec("layer.set_param", json!({ "layer": 3, "field": "speed", "value": 55 }));
    g.exec("tools.adjust_image", json!({}));
    assert_eq!(g.window_titles(), ["Adjust Image"]);
    g.app().images.adjust.as_mut().unwrap().args.brightness = Some(25.0);
    g.frames(3);
    g.app().editor.mark_saved();
    g.frames(2);
    g.key(Key::N, Modifiers::COMMAND);
    g.frames(3);
    assert!(g.app().editor.doc.is_empty(), "Ctrl+N made a new project");
    if g.app().images.adjust.is_some() {
        g.click_id("dialog:cancel");
        g.frames(3);
    }
    assert!(g.app().images.adjust.is_none());
    assert_eq!(g.app().editor.doc.layer(3).main().speed, 100.0, "the new project has the speed of the old one");
    assert!(!g.app().editor.is_modified(), "a new project has no changes to save");
}

/// A project opened while Edit Text Shapes is open keeps its own text: OK used to write the
/// text of the project before into the text with the same number.
#[test]
fn a_project_opened_under_edit_text_keeps_its_own_text() {
    let mut g = Gui::standard();
    let file = g.temp("hunt10-other.dbrn");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    g.exec("text.add", json!({ "text": "Other", "x": 10, "y": 10, "font": "IBM Plex Sans" }));
    g.exec("project.save_as", json!({ "path": file.to_string_lossy() }));
    crate::files::new_project(&mut g.app());
    g.exec("text.add", json!({ "text": "Mine", "x": 10, "y": 10, "font": "IBM Plex Sans" }));
    g.exec("text.edit_dialog", json!({}));
    assert_eq!(g.window_titles(), ["Edit Text Shapes"]);
    g.exec("project.open", json!({ "path": file.to_string_lossy() }));
    if g.app().text.edit.open {
        g.click_id("dialog:ok");
        g.frames(3);
    }
    assert!(!g.app().text.edit.open);
    let texts: Vec<String> = g
        .app()
        .editor
        .doc
        .iter()
        .filter_map(|s| match &s.kind {
            ShapeKind::Text(t) => Some(t.text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(texts, ["Other"], "the opened project has the text of the project before");
    assert!(!g.app().editor.is_modified(), "a project just opened has no changes to save");
    let _ = std::fs::remove_file(&file);
}

/// A cut library started with the panel's New button is the device's library from then on: the
/// next start used to open the file of the library before, without the entries made since.
#[test]
fn a_new_cut_library_is_still_there_at_the_next_start() {
    let mut g = Gui::standard();
    let settings = g.app().settings_dir.clone().expect("the harness has a settings folder");
    let file = g.temp("hunt10-old.dbclb");
    g.exec("library.create_from_layer", json!({ "layer": 1, "material": "Birch" }));
    g.exec("library.save_as", json!({ "path": file.to_string_lossy() }));
    // The file is remembered for the device, as after the panel's Save as.
    let device = g.app().extras.library.device.clone();
    g.app().prefs.library_paths.insert(device.clone(), file.to_string_lossy().into_owned());
    show_panel(&mut g, "library");
    let load = g.find_a11y(Some("Button"), "Load").into_iter().next().unwrap_or_else(|| panic!("no Load button: {:?}", g.a11y())).rect;
    let new = g
        .find_a11y(Some("Button"), "New")
        .into_iter()
        .find(|b| (b.rect.center().y - load.center().y).abs() < 2.0)
        .unwrap_or_else(|| panic!("no New button beside Load: {:?}", g.a11y()));
    g.click_pos(new.rect.center());
    g.frames(2);
    assert_eq!(g.exec("library.list", json!({}))["entries"].as_array().map(Vec::len), Some(0), "New gave an empty library");
    g.exec("library.create_from_layer", json!({ "layer": 1, "material": "Oak" }));
    g.app().extras.library.flush();
    let mut next = DarkBurn::new(g.app().prefs.clone());
    next.settings_dir = Some(settings);
    next.extras.library.device = device;
    let list = run(&mut next, "library.list", json!({})).unwrap();
    let names: Vec<&str> = list["entries"].as_array().unwrap().iter().filter_map(|e| e["name"].as_str()).collect();
    assert!(names.len() == 1 && names[0].starts_with("Oak"), "the next start shows {names:?}");
    let _ = std::fs::remove_file(&file);
}

// ----- The last pass: the commands the monkey leaves out, with odd arguments -----------------------

/// A document with the things commands trip over: a shape of no size, a path of one point,
/// groups in a group, a hidden layer, a locked shape, a picture, text that is not ASCII.
/// Everything is made the way a user can make it; what a command refuses is left out.
fn odd_document(a: &mut DarkBurn) {
    let made = |a: &mut DarkBurn, id: &str, args: Value| run(a, id, args).ok().and_then(|v| v["id"].as_u64()).map(ShapeId);
    let flat = made(a, "shape.add_rect", json!({ "x": 5, "y": 5, "width": 0, "height": 0, "layer": 0 }));
    let dot = made(a, "shape.add_path", json!({ "svg_path": "M 1 1", "layer": 1 }));
    let bytes = crate::images::testutil::png(4, 4, |x, y| ((x + y) * 30) as u8);
    let picture = crate::images::testutil::add_image(&mut a.editor.doc, bytes, (4, 4), (10.0, 10.0), (40.0, 40.0), 4);
    let r1 = add_rect(a, 0.0, 0.0, 10.0, 10.0, 2);
    let r2 = add_rect(a, 20.0, 0.0, 10.0, 10.0, 3);
    select(a, &[r1, r2]);
    run(a, "arrange.group", json!({})).unwrap();
    let inner = a.editor.selected()[0];
    let r3 = add_rect(a, 40.0, 0.0, 10.0, 10.0, 5);
    select(a, &[inner, r3]);
    run(a, "arrange.group", json!({})).unwrap();
    let outer = a.editor.selected()[0];
    let text = made(a, "text.add", json!({ "text": "Blåbærsyltetøy 日本語", "x": 10, "y": 60, "font": "IBM Plex Sans" }));
    let locked = add_rect(a, 60.0, 60.0, 5.0, 5.0, 6);
    select(a, &[locked]);
    run(a, "shape.set_props", json!({ "locked": true })).unwrap();
    run(a, "layer.set_param", json!({ "layer": 3, "field": "show", "value": false })).unwrap();
    let ids: Vec<ShapeId> = [flat, dot, Some(picture), Some(outer), text, Some(locked)].into_iter().flatten().collect();
    a.editor.selection.set(ids);
}

/// What is wrong with the document, if anything: a shape whose place or size is not a number.
fn unsound(d: &darkburn_core::Document) -> Option<String> {
    fn walk(s: &darkburn_core::Shape) -> Option<String> {
        if !s.transform.is_finite() {
            return Some(format!("shape {} has a transform that is not finite", s.id.0));
        }
        let b = s.bbox();
        if !(b.x0.is_finite() && b.y0.is_finite() && b.x1.is_finite() && b.y1.is_finite()) {
            return Some(format!("shape {} has bounds that are not finite", s.id.0));
        }
        match &s.kind {
            ShapeKind::Group { children } => children.iter().find_map(walk),
            _ => None,
        }
    }
    d.iter().find_map(walk)
}

/// Every command of the document side that the monkey leaves out (the cut and art libraries,
/// the hotkeys, the camera, rotary, cylinder, print and cut, the test tools, and the commands
/// that read a file) is run with odd arguments, on an empty document and on an odd one, with
/// files that are missing, empty or cut off in the middle. None may panic or leave a shape
/// whose place is not a number, and undo after a command that made a step puts the document
/// back, redo brings the change again.
#[test]
fn commands_the_monkey_leaves_out_take_odd_arguments() {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::path::PathBuf;
    const GROUPS: &[&str] = &["library.", "art.", "hotkeys.", "camera.", "pnc.", "rotary.", "cylinder.", "test."];
    const READERS: &[&str] =
        &["project.import", "project.open", "image.replace", "image.trace_capture", "image.preset_import", "variable.csv_load", "text.add_shx"];
    let dir = std::env::temp_dir().join(format!("darkburn-hunt10-odd-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("settings")).unwrap();
    let put = |name: &str, bytes: &[u8]| -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, bytes).unwrap();
        p
    };
    let png = crate::images::testutil::png(16, 16, |x, y| ((x * y) % 255) as u8);
    let svg: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><path d="M0 0 L50 50 L100 0 Z"/><rect x="1" y="1" width="5" height="5"/></svg>"#;
    let folder = dir.join("a-folder");
    std::fs::create_dir_all(&folder).unwrap();
    let mut files: Vec<PathBuf> = vec![dir.join("not-there.png"), folder.clone(), put("empty.png", b"")];
    files.push(put("half.png", &png[..png.len() / 2]));
    files.push(put("half.svg", &svg[..svg.len() / 2]));
    {
        let mut a = app();
        odd_document(&mut a);
        let whole = dir.join("whole.dbrn");
        run(&mut a, "project.save_as", json!({ "path": whole.to_string_lossy() })).unwrap();
        let bytes = std::fs::read(&whole).unwrap();
        files.push(put("half.dbrn", &bytes[..bytes.len() / 2]));
        run(&mut a, "library.create_from_layer", json!({ "layer": 1, "material": "Ply" })).unwrap();
        let whole = dir.join("whole.dbclb");
        run(&mut a, "library.save_as", json!({ "path": whole.to_string_lossy() })).unwrap();
        let bytes = std::fs::read(&whole).unwrap();
        files.push(put("half.dbclb", &bytes[..bytes.len() / 2]));
    }
    for ext in ["jpg", "bmp", "gif", "tif", "webp", "dxf", "pdf", "ai", "plt", "lbrn2", "lbrn", "csv", "shx", "json"] {
        files.push(put(&format!("picture-bytes.{ext}"), &png[..png.len() / 2]));
        files.push(put(&format!("text-bytes.{ext}"), &svg[..svg.len() / 2]));
    }
    let text_of = |p: &PathBuf| Value::String(p.to_string_lossy().into_owned());
    let outputs = [text_of(&dir.join("out.tmp")), text_of(&dir.join("no-such-folder").join("out.tmp")), text_of(&folder)];
    let numbers = [
        json!(0),
        json!(-1),
        json!(1),
        json!(0.5),
        json!(1e308),
        json!(-1e308),
        json!(1e-300),
        json!(4_294_967_296u64),
        json!(i64::MIN),
        json!(u64::MAX),
    ];
    let others = [
        Value::Null,
        json!(""),
        json!("æøå 日本語 \u{202e}x"),
        json!(true),
        json!(false),
        json!([]),
        json!({}),
        json!([0, 1e308]),
        json!([[0, 0], [1e308, -1e308], [5, 5]]),
        json!([{ "x": 0, "y": 0 }, { "x": 1e308, "y": -1e308 }, { "x": 3, "y": 4 }, { "x": 5, "y": 1 }]),
    ];
    let values: Vec<Value> = numbers.iter().chain(&others).cloned().collect();
    let commands: Vec<crate::commands::Info> = crate::commands::list()
        .into_iter()
        .filter(|i| i.implemented && !i.hardware && !crate::commands::hardware(&i.id))
        .filter(|i| GROUPS.iter().any(|g| i.id.starts_with(g)) || READERS.contains(&i.id.as_str()))
        .collect();
    assert!(commands.len() >= 40, "only {} commands were found", commands.len());

    // The argument sets of one command.
    let arg_sets = |info: &crate::commands::Info| -> Vec<Value> {
        let writes = info.id.contains("save") || info.id.contains("export");
        let names: Vec<String> = info.args_schema["properties"].as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default();
        let is_file = |n: &str| ["path", "file", "folder", "dir"].iter().any(|k| n.contains(k)) || n == "images";
        let is_list = |n: &str| n == "paths" || n == "images";
        let file_values = |n: &str| -> Vec<Value> {
            let one: Vec<Value> = if writes { outputs.to_vec() } else { files.iter().map(&text_of).collect() };
            if is_list(n) { one.iter().map(|v| json!([v])).chain([Value::Array(one.clone()), json!([])]).collect() } else { one }
        };
        let mut sets = vec![json!({}), Value::Null];
        for n in &names {
            let vs = if is_file(n) { file_values(n) } else { values.clone() };
            sets.extend(vs.into_iter().map(|v| json!({ n.as_str(): v })));
        }
        // Every argument at once, so the checks of one do not hide the others.
        for (k, v) in values.iter().enumerate() {
            let all: serde_json::Map<String, Value> = names
                .iter()
                .map(|n| {
                    let v = if is_file(n) {
                        let fv = file_values(n);
                        fv[k % fv.len()].clone()
                    } else {
                        v.clone()
                    };
                    (n.clone(), v)
                })
                .collect();
            sets.push(Value::Object(all));
        }
        sets
    };

    let fresh = |odd: bool| -> DarkBurn {
        let mut a = app();
        a.settings_dir = Some(dir.join("settings"));
        if odd {
            odd_document(&mut a);
        }
        a
    };
    let mut bad: Vec<String> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    let mut ran = 0usize;
    for odd in [false, true] {
        let mut a = fresh(odd);
        for info in &commands {
            for args in arg_sets(info) {
                ran += 1;
                let _ = std::fs::write(dir.join("last-command"), format!("{} {args}", info.id));
                let before = a.editor.doc.clone();
                let steps = a.editor.history.len();
                let Ok(r) = catch_unwind(AssertUnwindSafe(|| run(&mut a, &info.id, args.clone()))) else {
                    bad.push(format!("panic: {} {args}", info.id));
                    a = fresh(odd);
                    continue;
                };
                if let Some(why) = unsound(&a.editor.doc) {
                    bad.push(format!("{} {args}: {why}", info.id));
                    a = fresh(odd);
                    continue;
                }
                if info.id == "project.open" {
                    if r.is_ok() {
                        a = fresh(odd);
                    }
                    continue;
                }
                if a.editor.history.len() > steps {
                    let after = a.editor.doc.clone();
                    a.editor.undo();
                    if a.editor.doc != before {
                        bad.push(format!("undo does not put the document back: {} {args}", info.id));
                    }
                    a.editor.redo();
                    if a.editor.doc != after {
                        bad.push(format!("redo does not bring the change again: {} {args}", info.id));
                    }
                    // The document stays as it was, so it does not grow through the pass.
                    a.editor.undo();
                } else if let Err(e) = &r
                    && a.editor.doc != before
                {
                    notes.push(format!("refused but the document changed: {} {args}: {e}", info.id));
                }
            }
        }
    }
    let _ = std::fs::write(
        std::env::temp_dir().join("darkburn-hunt10-odd-notes.txt"),
        format!("{ran} runs of {} commands\n{}\n", commands.len(), notes.join("\n")),
    );
    let _ = std::fs::remove_dir_all(&dir);
    assert!(bad.is_empty(), "{} of {ran} runs went wrong:\n{}", bad.len(), bad.join("\n"));
}

/// An alignment target, a centre mark or a test pattern that cannot be drawn is refused. It used
/// to be reported as drawn (with the numbers of shapes that were not there), and a target of an
/// absurd size took all the memory there was.
#[test]
fn a_mark_that_cannot_be_drawn_is_refused() {
    let mut a = app();
    let far = json!([{ "x": 1e30, "y": 0 }, { "x": 0, "y": 1e30 }, { "x": -1e30, "y": 0 }]);
    for (id, args) in [
        ("camera.align_generate", json!({ "size": 1e30 })),
        ("camera.align_generate", json!({ "margin": 1e30 })),
        ("test.centre", json!({ "points": far, "mark": true })),
        ("test.focus", json!({ "x": 1e30 })),
        ("test.interval", json!({ "y": -1e30 })),
    ] {
        let r = run(&mut a, id, args.clone());
        assert!(r.is_err(), "{id} {args} was reported as done: {r:?}");
        assert!(a.editor.doc.is_empty() && a.editor.history.is_empty(), "{id} {args} left something behind");
    }
    // What a person asks for is still drawn.
    assert!(run(&mut a, "camera.align_generate", json!({ "size": 8 })).is_ok());
    assert!(run(&mut a, "test.centre", json!({ "points": [{ "x": 10, "y": 0 }, { "x": 0, "y": 10 }, { "x": -10, "y": 0 }], "mark": true })).is_ok());
}

/// The cut library commands that name a layer refuse a layer that does not exist. `library.link`
/// with layer 99 used to link tool layer T2.
#[test]
fn library_commands_refuse_a_layer_that_does_not_exist() {
    let mut a = app();
    run(&mut a, "library.create_from_layer", json!({ "layer": 1, "material": "Ply" })).unwrap();
    for id in ["library.link", "library.assign", "library.unlink"] {
        let r = run(&mut a, id, json!({ "entry": 0, "layer": 99 }));
        assert!(r.as_ref().is_err_and(|e| e.contains("does not exist")), "{id}: {r:?}");
    }
    assert!(a.editor.doc.layers.iter().all(|l| l.library_link.is_none()), "a layer was linked");
    assert_eq!(a.editor.history.len(), 0);
    run(&mut a, "library.link", json!({ "entry": 0, "layer": 2 })).unwrap();
    assert!(a.editor.doc.layer(2).library_link.is_some());
}

/// With the Offset window open, a shape moved with the mouse takes the preview with it: the
/// preview used to stay where the shape was when the drag began.
#[test]
fn a_dialog_preview_follows_a_shape_moved_with_the_mouse() {
    use darkburn_core::kurbo::Shape as _;
    let mut g = Gui::standard();
    let id = id_of(&g.exec("shape.add_rect", json!({ "x": 20, "y": 20, "width": 30, "height": 20, "layer": 1 })));
    g.app().editor.selection.set([id]);
    g.run_command("tools.offset");
    g.frames(3);
    assert_eq!(g.app().shape_ui.open, Some("tools.offset"));
    let preview = |g: &Gui| g.app().shape_ui.preview.iter().map(|p| p.bounding_box()).reduce(|a, b| a.union(b));
    let before = preview(&g).expect("the window shows a preview");
    let at = bbox(&g.app(), id);
    // Taken by its bottom edge, away from the handles (an outline is picked by its line). The
    // page has y down from its top edge, the design y up from its bottom edge.
    let page_y = g.app().prefs.page_height - at.y0 as f32;
    let x = (at.x0 + at.width() / 4.0) as f32;
    g.drag_mm((x, page_y), (x + 40.0, page_y));
    g.frames(3);
    let moved = bbox(&g.app(), id).x0 - at.x0;
    assert!(moved > 20.0, "the drag moved the shape {moved} mm");
    let after = preview(&g).expect("the window still shows a preview");
    assert!(near(after.x0 - before.x0, moved), "the shape moved {moved} mm and its preview {} mm", after.x0 - before.x0);
}

/// A command that adds shapes names only shapes that are there. One that would lie further out
/// than a design can reach is refused, and its answer used to name shapes that do not exist.
#[test]
fn shapes_that_cannot_be_made_are_not_reported_as_made() {
    let mut a = app();
    let r = add_rect(&mut a, 0.0, 0.0, 10.0, 10.0, 0);
    for (id, args) in [
        ("shape.add_rect", json!({ "x": 1e12, "y": 0, "width": 5, "height": 5 })),
        ("shape.add_ellipse", json!({ "x": 1e12, "y": 0, "width": 5, "height": 5 })),
        ("shape.add_polygon", json!({ "cx": 1e12, "cy": 0, "radius": 5, "sides": 6 })),
        ("shape.add_path", json!({ "svg_path": "M0 0 L1e12 0" })),
        ("shape.add_line", json!({ "points": [[0, 0], [1e12, 0]] })),
        ("text.add", json!({ "text": "Hi", "x": 1e12, "y": 0, "font": "IBM Plex Sans" })),
        ("arrange.grid_array", json!({ "rows": 1, "cols": 3, "dx": 1e12 })),
        ("arrange.grid_array", json!({ "rows": 1, "cols": 3, "dx": 1e12, "virtual": true })),
        ("arrange.circular_array", json!({ "cx": 1e12, "cy": 0, "count": 4 })),
    ] {
        select(&mut a, &[r]);
        let out = run(&mut a, id, args.clone());
        assert_eq!((a.editor.doc.len(), a.editor.history.len()), (1, 1), "{id} {args} changed the design");
        let mut named: Vec<u64> = Vec::new();
        if let Ok(v) = &out {
            named.extend(v["id"].as_u64());
            named.extend(v["ids"].as_array().into_iter().flatten().filter_map(Value::as_u64));
        }
        assert!(named.iter().all(|i| a.editor.doc.get(ShapeId(*i)).is_some()), "{id} {args} answered {out:?}");
    }
}

/// Help > Check for Updates while the check made at the start is still on its way shows the
/// answer of that check. It used to show nothing, and the status line said "Checking for
/// updates..." for good.
#[test]
fn a_check_asked_for_while_the_start_check_runs_shows_its_answer() {
    use std::io::{Read, Write};
    use std::time::Duration;
    // A server on this computer that keeps its answer back until it is told to give it.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://127.0.0.1:{}/releases/latest", listener.local_addr().unwrap().port());
    let (go, wait) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || {
        let Ok((mut s, _)) = listener.accept() else { return };
        let mut buf = [0u8; 4096];
        let _ = s.read(&mut buf);
        let _ = wait.recv_timeout(Duration::from_secs(8));
        let body = r#"{"tag_name":"v0.0.1"}"#;
        let reply = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
        let _ = s.write_all(reply.as_bytes());
    });
    let mut g = Gui::standard();
    g.app().prefs.opts.update_url = url.clone();
    // The check the setting makes when the program starts: by itself it shows only a newer version.
    g.app().extras.updates.start(url, false);
    assert!(g.app().extras.updates.running());
    g.run_command("help.check_updates");
    g.frames(2);
    go.send(()).unwrap();
    for _ in 0..500 {
        if !g.app().extras.updates.running() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
        g.frames(1);
    }
    assert!(!g.app().extras.updates.running(), "the check came to an end");
    g.frames(2);
    let note = g.app().note.clone();
    assert!(g.app().extras.updates.open, "the answer is shown; the status line says {note:?}");
    assert!(note.as_deref().is_some_and(|n| n.contains("up to date")), "the status line says {note:?}");
}

/// A drag on a number field of the Text toolbar is one undo step, as on every other number
/// field. It used to be a step for every frame of the drag in which the number changed.
#[test]
fn a_drag_on_a_number_field_of_the_text_toolbar_is_one_undo_step() {
    for (label, read) in [
        ("Text height", (|t: &darkburn_core::TextShape| t.height) as fn(&darkburn_core::TextShape) -> f64),
        ("Letters spacing", |t| t.letter_spacing),
        ("Lines spacing", |t| t.line_spacing),
        ("Index offset", |t| t.offset as f64),
    ] {
        let mut g = Gui::standard();
        let text = id_of(&g.exec("text.add", json!({ "text": "Hi", "x": 80, "y": 80, "height": 10, "font": "IBM Plex Sans" })));
        g.app().editor.selection.set([text]);
        g.app().prefs.ribbon_tab = "text".into();
        g.frames(3);
        let value = |g: &Gui| match &g.app().editor.doc.get(text).unwrap().kind {
            ShapeKind::Text(t) => read(t),
            _ => panic!("not a text"),
        };
        // The toolbar is at the top of the window (Shape Properties may show a field of the same name).
        let field = g
            .find_a11y(Some("SpinButton"), label)
            .into_iter()
            .min_by(|a, b| a.rect.top().total_cmp(&b.rect.top()))
            .unwrap_or_else(|| panic!("no field {label} in the toolbar: {:?}", g.a11y()));
        let steps = g.app().editor.history.len();
        let before = value(&g);
        let c = field.rect.center();
        g.drag_steps(c, c + egui::vec2(60.0, 0.0), 60);
        assert!(value(&g) != before, "{label}: the drag changed nothing");
        assert_eq!(g.app().editor.history.len(), steps + 1, "{label}: undo steps of one drag");
        g.app().editor.undo();
        assert_eq!(value(&g), before, "{label}: one undo takes the drag back");
    }
}

/// The same for the numbers of the Variable Text panel and the index offset in Shape Properties.
#[test]
fn a_drag_on_a_variable_text_number_is_one_undo_step() {
    use darkburn_core::Document;
    let fields: [(&str, &str, fn(&Document) -> i64); 5] = [
        ("variable_text", "Start", |d| d.variables.serial.start),
        ("variable_text", "End", |d| d.variables.serial.end),
        ("variable_text", "Current", |d| d.variables.serial.current),
        ("variable_text", "Advance by", |d| d.variables.serial.advance_by),
        ("variable_text", "Zero pad to", |d| d.variables.serial.padding as i64),
    ];
    for (panel, label, read) in fields {
        let mut g = Gui::standard();
        show_panel(&mut g, panel);
        let field = g.find_a11y(Some("SpinButton"), label).into_iter().next().unwrap_or_else(|| panic!("no field {label}: {:?}", g.a11y()));
        let steps = g.app().editor.history.len();
        let before = read(&g.app().editor.doc);
        let c = field.rect.center();
        g.drag_steps(c, c + egui::vec2(60.0, 0.0), 60);
        let after = read(&g.app().editor.doc);
        assert!(after != before, "{label}: the drag changed nothing");
        assert_eq!(g.app().editor.history.len(), steps + 1, "{label}: undo steps of one drag");
        g.app().editor.undo();
        assert_eq!(read(&g.app().editor.doc), before, "{label}: one undo takes the drag back");
    }
    // Index offset of a text, in Shape Properties: a tall window, so the panel shows it all.
    let mut g = Gui::new(1600.0, 2400.0, 1.0);
    let text = id_of(&g.exec("text.add", json!({ "text": "Hi", "x": 80, "y": 80, "height": 10, "font": "IBM Plex Sans" })));
    g.app().editor.selection.set([text]);
    show_panel(&mut g, "shape_properties");
    let offset = |g: &Gui| match &g.app().editor.doc.get(text).unwrap().kind {
        ShapeKind::Text(t) => t.offset,
        _ => panic!("not a text"),
    };
    let field = g.find_a11y(Some("SpinButton"), "Index offset").into_iter().next().unwrap_or_else(|| panic!("no Index offset field: {:?}", g.a11y()));
    let steps = g.app().editor.history.len();
    let before = offset(&g);
    let c = field.rect.center();
    g.drag_steps(c, c + egui::vec2(60.0, 0.0), 60);
    assert!(offset(&g) != before, "Index offset: the drag changed nothing");
    assert_eq!(g.app().editor.history.len(), steps + 1, "Index offset: undo steps of one drag");
    g.app().editor.undo();
    assert_eq!(offset(&g), before);
}

/// In the Warp and Deform tools the handles follow a shape that is moved with the arrow keys,
/// so Enter with the handles untouched leaves the shape where it is. The handles used to stay
/// where the shape had been, and Enter pulled the shape back there.
#[test]
fn the_warp_handles_follow_a_shape_moved_with_the_arrow_keys() {
    for tool in ["tools.warp", "tools.deform"] {
        let mut g = Gui::standard();
        let id = id_of(&g.exec("shape.add_rect", json!({ "x": 100, "y": 100, "width": 40, "height": 20, "layer": 1 })));
        g.app().editor.selection.set([id]);
        g.run_command(tool);
        g.frames(2);
        for _ in 0..3 {
            g.key(Key::ArrowRight, Modifiers::SHIFT);
        }
        let moved = bbox(&g.app(), id);
        assert!(near(moved.x0, 130.0), "{tool}: the arrow keys move the shape: {moved:?}");
        g.key(Key::Enter, Modifiers::NONE);
        g.frames(2);
        let after = bbox(&g.app(), id);
        assert!(near(after.x0, moved.x0) && near(after.x1, moved.x1), "{tool}: Enter moved the shape from {moved:?} to {after:?}");
    }
}

/// Two overlapping rectangles in an otherwise empty project; their ids.
fn two_rects(g: &mut Gui) -> (ShapeId, ShapeId) {
    let a = id_of(&g.exec("shape.add_rect", json!({ "x": 10, "y": 10, "width": 30, "height": 30, "layer": 1 })));
    let b = id_of(&g.exec("shape.add_rect", json!({ "x": 25, "y": 25, "width": 30, "height": 30, "layer": 1 })));
    (a, b)
}

/// A new project made with Ctrl+N while the Boolean Assistant holds its two sets, or while
/// Convert to Bitmap is open, is left alone by that window: OK used to work on the shapes of the
/// new project that have the numbers the old shapes had.
#[test]
fn a_window_that_holds_shapes_lets_go_of_them_when_a_new_project_is_made() {
    for window in ["tools.boolean_assistant", "edit.convert_bitmap"] {
        let mut g = Gui::standard();
        let (a, b) = two_rects(&mut g);
        g.app().editor.mark_saved();
        g.app().editor.selection.set([a, b]);
        g.run_command(window);
        g.frames(2);
        if window == "tools.boolean_assistant" {
            g.app().editor.selection.set([a]);
            g.frames(2);
            g.click_label("Set A from selection");
            g.app().editor.selection.set([b]);
            g.frames(2);
            g.click_label("Set B from selection");
            g.frames(2);
            let sets = g.app().shape_ui.boolean.clone();
            assert_eq!((sets.a, sets.b), (vec![a], vec![b]));
        } else {
            assert!(g.app().images.convert.is_some(), "Convert to Bitmap is open");
        }
        g.key(Key::N, Modifiers::COMMAND);
        g.frames(3);
        assert!(g.app().editor.doc.is_empty(), "{window}: Ctrl+N made a new project");
        // The new project gets shapes of its own; they have the numbers the old ones had.
        assert_eq!(two_rects(&mut g), (a, b));
        g.app().editor.selection.clear();
        g.frames(2);
        if window == "tools.boolean_assistant" {
            if g.app().shape_ui.open.is_some() && g.find_text("OK").is_some() {
                g.click_label("OK");
            }
        } else if g.app().images.convert.is_some() {
            g.click_id("dialog:ok");
        }
        g.frames(3);
        let rects = g.app().editor.doc.iter().filter(|s| matches!(s.kind, ShapeKind::Rect { .. })).count();
        assert_eq!((g.app().editor.doc.len(), rects), (2, 2), "{window}: the window changed shapes of the new project");
    }
}

/// A shape far larger than the work area can be selected: its dashed outline used to be cut
/// into dashes along its whole length, far more than memory holds, and the program died.
/// Run it under a memory limit (`ulimit -v`) when it fails, or it takes the whole machine's.
#[test]
fn a_huge_shape_can_be_selected() {
    let mut g = Gui::standard();
    let id = id_of(&g.exec("shape.add_rect", json!({ "x": 0, "y": 0, "width": 5e8, "height": 5e8, "layer": 1 })));
    g.app().editor.selection.set([id]);
    g.frames(3);
    g.run_command("view.zoom_in");
    g.frames(3);
    assert_eq!(g.app().editor.selected(), [id]);
}

/// Convert to Bitmap on a very large shape says the picture would be too large. Working out the
/// number of pixels used to overflow and take the program down.
#[test]
fn convert_to_bitmap_of_a_huge_shape_says_it_is_too_large() {
    let mut g = Gui::standard();
    let id = id_of(&g.exec("shape.add_rect", json!({ "x": 0, "y": 0, "width": 5e8, "height": 5e8, "layer": 1 })));
    g.app().editor.selection.set([id]);
    g.run_command("edit.convert_bitmap");
    g.frames(3);
    let problem = g.app().images.convert.as_ref().expect("Convert to Bitmap is open").problem();
    assert!(problem.as_deref().is_some_and(|p| p.contains("too large")), "{problem:?}");
}
