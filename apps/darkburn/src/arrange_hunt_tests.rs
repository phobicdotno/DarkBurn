//! Arrange, clipboard and layer commands checked against numbers worked out by hand.
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
