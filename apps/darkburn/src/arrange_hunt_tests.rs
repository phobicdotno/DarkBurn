//! Arrange, clipboard and layer commands checked against numbers worked out by hand.
use serde_json::json;

use crate::shape_ops::tests::{add_rect, app, run};

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
