//! Arrange, clipboard and layer commands checked against numbers worked out by hand.
use darkburn_core::{ShapeId, ShapeKind};
use serde_json::json;

use crate::shape_ops::tests::{add_rect, app, run, select};

/// Layers 1, 2, 3 in output order; layer 1 moved two places later is 2, 3, 1 (not a swap with
/// the layer two places on, which gave 3, 2, 1).
#[test]
fn moving_a_layer_two_places_keeps_the_order_of_the_others() {
    let mut a = app();
    for l in [1, 2, 3] {
        add_rect(&mut a, 0.0, 0.0, 10.0, 10.0, l);
    }
    run(&mut a, "layer.move", json!({ "layer": 1, "delta": 2 })).unwrap();
    assert_eq!(a.editor.doc.used_layers(), vec![2, 3, 1]);
    run(&mut a, "edit.undo", json!({})).unwrap();
    assert_eq!(a.editor.doc.used_layers(), vec![1, 2, 3]);
}

/// Copy or Cut with nothing selected leaves the clipboard as it was (it used to empty it and put
/// an empty DarkBurn clipboard over whatever the system clipboard held).
#[test]
fn copy_and_cut_with_nothing_selected_keep_what_was_copied() {
    let ctx = egui::Context::default();
    let mut a = app();
    let r = add_rect(&mut a, 0.0, 0.0, 10.0, 10.0, 2);
    select(&mut a, &[r]);
    a.run(&ctx, "edit.copy");
    assert!(a.clipboard_out);
    a.clipboard_out = false;
    select(&mut a, &[]);
    for id in ["edit.copy", "edit.cut"] {
        a.run(&ctx, id);
        assert_eq!(a.editor.clipboard.shapes.len(), 1, "{id}");
        assert!(!a.clipboard_out, "{id}: nothing goes to the system clipboard");
    }
    a.run(&ctx, "edit.paste");
    assert_eq!(a.editor.doc.len(), 2);
}

/// Copy Along Path follows the shape selected last, also when it was drawn first: three copies of
/// a 4 mm square along a line from 0 to 100 sit centred on 0, 50 and 100.
#[test]
fn copy_along_path_follows_the_last_selected_shape_not_the_topmost() {
    let mut a = app();
    let line = run(&mut a, "shape.add_path", json!({ "svg_path": "M0 0 L100 0", "layer": 0 })).unwrap()["id"].as_u64().unwrap();
    let r = add_rect(&mut a, 0.0, 50.0, 4.0, 4.0, 0);
    select(&mut a, &[r, ShapeId(line)]);
    run(&mut a, "arrange.copy_along_path", json!({ "count": 3 })).unwrap();
    let mut centres: Vec<(f64, f64)> = a
        .editor
        .doc
        .iter()
        .filter(|s| matches!(s.kind, ShapeKind::Rect { .. }))
        .map(|s| ((s.bbox().center().x * 1e6).round() / 1e6, (s.bbox().center().y * 1e6).round() / 1e6))
        .collect();
    centres.sort_by(|a, b| a.0.total_cmp(&b.0));
    assert_eq!(centres, vec![(0.0, 0.0), (50.0, 0.0), (100.0, 0.0)]);
    assert!(a.editor.doc.get(ShapeId(line)).is_some(), "the line stays");
}

/// A virtual grid array of a 4 mm square at x 300..304, moved 50 mm right and then changed to a
/// circular array of two: the page is 400 mm wide, so about its centre (200) the square at
/// 350..354 gets its second copy at 46..50. The centre used to be taken 50 mm off.
#[test]
fn a_moved_array_changed_to_circular_turns_about_the_page_centre() {
    let mut a = app();
    let r = add_rect(&mut a, 300.0, 198.0, 4.0, 4.0, 2);
    select(&mut a, &[r]);
    let v = run(&mut a, "arrange.grid_array", json!({ "rows": 1, "cols": 2, "virtual": true })).unwrap();
    let id = v["ids"][0].as_u64().unwrap();
    run(&mut a, "shape.translate", json!({ "dx": 50.0, "dy": 0.0 })).unwrap();
    run(&mut a, "arrange.array_edit", json!({ "id": id, "kind": "circular", "count": 2 })).unwrap();
    let b = a.editor.doc.get(ShapeId(id)).unwrap().bbox();
    assert!((b.x0 - 46.0).abs() < 1e-6 && (b.x1 - 354.0).abs() < 1e-6 && (b.y0 - 198.0).abs() < 1e-6, "{b:?}");
}

/// Deleting a layer with its shapes also takes them out of a virtual array's originals: editing
/// the array afterwards must not bring them back.
#[test]
fn shapes_of_a_deleted_layer_do_not_come_back_when_a_virtual_array_is_edited() {
    let mut a = app();
    let r1 = add_rect(&mut a, 0.0, 0.0, 10.0, 10.0, 2);
    let r2 = add_rect(&mut a, 20.0, 0.0, 10.0, 10.0, 5);
    select(&mut a, &[r1, r2]);
    let v = run(&mut a, "arrange.grid_array", json!({ "rows": 1, "cols": 2, "virtual": true })).unwrap();
    let id = v["ids"][0].as_u64().unwrap();
    run(&mut a, "layer.delete", json!({ "layer": 2, "with_shapes": true })).unwrap();
    assert_eq!(a.editor.doc.used_layers(), vec![5]);
    run(&mut a, "arrange.array_edit", json!({ "id": id, "cols": 3 })).unwrap();
    assert_eq!(a.editor.doc.used_layers(), vec![5]);
    assert_eq!(a.editor.doc.get(ShapeId(id)).unwrap().leaves().len(), 3, "three copies of the one square left");
}

/// Hiding a layer drops its shapes from the selection: they used to stay selected, unseen, and
/// the next Delete removed them. A group with a child on a shown layer stays.
#[test]
fn hiding_a_layer_deselects_its_shapes() {
    let ctx = egui::Context::default();
    let mut a = app();
    let hidden = add_rect(&mut a, 0.0, 0.0, 10.0, 10.0, 2);
    let shown = add_rect(&mut a, 20.0, 0.0, 10.0, 10.0, 3);
    select(&mut a, &[hidden, shown]);
    run(&mut a, "layer.set_param", json!({ "layer": 2, "field": "show", "value": false })).unwrap();
    assert_eq!(a.editor.selected(), vec![shown]);
    a.run(&ctx, "edit.delete");
    assert!(a.editor.doc.get(hidden).is_some(), "the hidden shape is not deleted");
    assert!(a.editor.doc.get(shown).is_none());
    // Undo of the hiding gives the selection back with the layer.
    run(&mut a, "edit.undo", json!({})).unwrap();
    run(&mut a, "edit.undo", json!({})).unwrap();
    assert!(a.editor.doc.layer(2).show);
    assert_eq!(a.editor.selected(), vec![hidden, shown]);
}

/// Unlock with nothing selected unlocks everything, and selects what it unlocked: but not the
/// shapes on a hidden layer, which nobody can see.
#[test]
fn unlocking_everything_does_not_select_shapes_on_hidden_layers() {
    let ctx = egui::Context::default();
    let mut a = app();
    let r = add_rect(&mut a, 0.0, 0.0, 10.0, 10.0, 2);
    let s = add_rect(&mut a, 20.0, 0.0, 10.0, 10.0, 3);
    select(&mut a, &[r, s]);
    a.run(&ctx, "arrange.lock");
    run(&mut a, "layer.set_param", json!({ "layer": 2, "field": "show", "value": false })).unwrap();
    a.run(&ctx, "arrange.unlock");
    assert!(!a.editor.doc.get(r).unwrap().locked && !a.editor.doc.get(s).unwrap().locked);
    assert_eq!(a.editor.selected(), vec![s]);
}

/// Group, rotate, ungroup, flip, same width, align and page corner, each against hand numbers.
#[test]
fn group_rotate_flip_same_width_align_and_page_corner_give_the_hand_computed_boxes() {
    let ctx = egui::Context::default();
    let mut a = app();
    let r = add_rect(&mut a, 10.0, 10.0, 20.0, 10.0, 2);
    let s = add_rect(&mut a, 50.0, 30.0, 10.0, 30.0, 3);
    select(&mut a, &[r, s]);
    a.run(&ctx, "arrange.group");
    // The group spans 10..60 by 10..60: centre 35, 35.
    a.run(&ctx, "arrange.rotate_cw");
    let g = a.editor.selected()[0];
    a.run(&ctx, "arrange.ungroup");
    let _ = g;
    // Clockwise about (35, 35): (x, y) -> (35 + (y - 35), 35 - (x - 35)). r 10..30 x 10..20 -> x 10..20, y 40..60.
    let b = a.editor.doc.get(r).unwrap().bbox();
    assert!((b.x0 - 10.0).abs() < 1e-6 && (b.x1 - 20.0).abs() < 1e-6 && (b.y0 - 40.0).abs() < 1e-6 && (b.y1 - 60.0).abs() < 1e-6, "{b:?}");
    // s 50..60 x 30..60 -> x 30..60, y 10..20.
    let b = a.editor.doc.get(s).unwrap().bbox();
    assert!((b.x0 - 30.0).abs() < 1e-6 && (b.x1 - 60.0).abs() < 1e-6 && (b.y0 - 10.0).abs() < 1e-6 && (b.y1 - 20.0).abs() < 1e-6, "{b:?}");
    select(&mut a, &[r, s]);
    a.run(&ctx, "arrange.flip_h");
    // About x = 35: r 10..20 -> 50..60; s 30..60 -> 10..40.
    let b = a.editor.doc.get(r).unwrap().bbox();
    assert!((b.x0 - 50.0).abs() < 1e-6 && (b.x1 - 60.0).abs() < 1e-6, "{b:?}");
    a.run(&ctx, "arrange.same_width");
    // r becomes as wide as s (30) about its centre 55: 40..70.
    let b = a.editor.doc.get(r).unwrap().bbox();
    assert!((b.x0 - 40.0).abs() < 1e-6 && (b.x1 - 70.0).abs() < 1e-6, "{b:?}");
    a.run(&ctx, "arrange.align_h_centres");
    a.run(&ctx, "arrange.align_right");
    let b = a.editor.doc.get(r).unwrap().bbox();
    // s is x 10..40, y 10..20 (centre y 15); r 30 wide, 20 tall (y 40..60).
    assert!((b.x1 - 40.0).abs() < 1e-6 && (b.center().y - 15.0).abs() < 1e-6, "{b:?}");
    a.run(&ctx, "arrange.page_lower_right");
    let u = a.editor.doc.bbox().unwrap();
    assert!((u.x1 - 400.0).abs() < 1e-6 && u.y0.abs() < 1e-6, "{u:?}");
}

fn span_x(a: &crate::shell::DarkBurn, id: ShapeId) -> (f64, f64) {
    let b = a.editor.doc.get(id).unwrap().bbox();
    ((b.x0 * 1e6).round() / 1e6, (b.x1 * 1e6).round() / 1e6)
}

fn span_y(a: &crate::shell::DarkBurn, id: ShapeId) -> (f64, f64) {
    let b = a.editor.doc.get(id).unwrap().bbox();
    ((b.y0 * 1e6).round() / 1e6, (b.y1 * 1e6).round() / 1e6)
}

/// Docking one by one, as a group, against a shape that is not selected and with inner shapes
/// locked, then spacing and move-together with the padding field: all against hand numbers.
#[test]
fn docking_spacing_and_move_together_give_the_hand_computed_places() {
    let mut a = app();
    let p = add_rect(&mut a, 50.0, 0.0, 10.0, 10.0, 0);
    let q = add_rect(&mut a, 80.0, 0.0, 10.0, 10.0, 0);
    select(&mut a, &[q, p]);
    run(&mut a, "arrange.dock_left", json!({ "padding": 2.0, "as_group": false })).unwrap();
    assert_eq!((span_x(&a, p), span_x(&a, q)), ((0.0, 10.0), (12.0, 22.0)));
    run(&mut a, "arrange.dock_right", json!({ "padding": 2.0, "as_group": true })).unwrap();
    assert_eq!((span_x(&a, p), span_x(&a, q)), ((378.0, 388.0), (390.0, 400.0)));
    run(&mut a, "arrange.dock_up", json!({ "padding": 3.0, "as_group": false })).unwrap();
    assert_eq!((span_y(&a, p), span_y(&a, q)), ((390.0, 400.0), (390.0, 400.0)));
    // An obstacle that is not selected: a wall at x 200..210 over the whole height.
    let wall = add_rect(&mut a, 200.0, 0.0, 10.0, 400.0, 0);
    select(&mut a, &[p, q]);
    run(&mut a, "arrange.dock_left", json!({ "padding": 5.0, "as_group": false })).unwrap();
    assert_eq!((span_x(&a, p), span_x(&a, q)), ((215.0, 225.0), (230.0, 240.0)));
    // Lock inner: a small square inside a big one follows it.
    let big = add_rect(&mut a, 300.0, 100.0, 50.0, 50.0, 0);
    let small = add_rect(&mut a, 310.0, 110.0, 5.0, 5.0, 0);
    select(&mut a, &[big, small]);
    run(&mut a, "arrange.dock_down", json!({ "padding": 0.0, "as_group": false, "lock_inner": true })).unwrap();
    assert_eq!((span_y(&a, big), span_y(&a, small)), ((0.0, 50.0), (10.0, 15.0)));
    let _ = wall;
    // Spacing and together through the padding field.
    let mut a = app();
    let p = add_rect(&mut a, 50.0, 0.0, 10.0, 10.0, 0);
    let q = add_rect(&mut a, 80.0, 0.0, 20.0, 10.0, 0);
    let r = add_rect(&mut a, 5.0, 0.0, 10.0, 10.0, 0);
    select(&mut a, &[p, q, r]);
    a.padding = 4.0;
    let ctx = egui::Context::default();
    a.run(&ctx, "arrange.distribute_h_spacing");
    assert_eq!((span_x(&a, r), span_x(&a, p), span_x(&a, q)), ((5.0, 15.0), (19.0, 29.0), (33.0, 53.0)));
    a.run(&ctx, "arrange.move_h_together");
    // 40 wide together about the middle of 5..53 (29): 9..49.
    assert_eq!((span_x(&a, r), span_x(&a, p), span_x(&a, q)), ((9.0, 19.0), (19.0, 29.0), (29.0, 49.0)));
}

/// Deleting a layer with its shapes also drops the pixels of the pictures that went with it, as
/// the Cuts / Layers button does: they used to stay in the project file for good.
#[test]
fn deleting_a_layer_with_its_shapes_drops_the_pixels_of_its_pictures() {
    use darkburn_core::kurbo::Affine;
    use darkburn_core::{ImageData, ImageShape, Shape};
    let mut a = app();
    a.editor.edit("Add", |d, _| {
        let key = d.add_image(ImageData { format: "png".into(), bytes: std::sync::Arc::from(&b"pixels"[..]), px_width: 2, px_height: 1 });
        d.add(Shape::new(ShapeKind::Image(ImageShape { image: key, ..Default::default() }), Affine::IDENTITY, 4));
        true
    });
    assert_eq!(a.editor.doc.images.len(), 1);
    run(&mut a, "layer.delete", json!({ "layer": 4, "with_shapes": true })).unwrap();
    assert!(a.editor.doc.is_empty());
    assert!(a.editor.doc.images.is_empty(), "the pixels went with the picture");
    run(&mut a, "edit.undo", json!({})).unwrap();
    assert_eq!((a.editor.doc.len(), a.editor.doc.images.len()), (1, 1), "undo brings both back");
}
