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
