//! Bug hunt 10: the picture an array's originals use is kept, and pasted content is put on
//! layers that exist, as a project is when it is opened.

use std::sync::Arc;

use darkburn_core::kurbo::{Affine, Rect};
use darkburn_core::{
    AlongDef, ArrayDef, ArrayKind, Clipboard, Document, GridDef, ImageData, ImageShape, LAYER_COUNT, PasteAt, Shape, ShapeKind, project,
};

fn picture(d: &mut Document) -> (String, Shape) {
    let key = d.add_image(ImageData { format: "png".into(), bytes: Arc::from(&b"\x89PNG not really"[..]), px_width: 4, px_height: 2 });
    let img = Shape::new(ShapeKind::Image(ImageShape { image: key.clone(), width: 40.0, height: 20.0, ..Default::default() }), Affine::IDENTITY, 0);
    (key, img)
}

/// An array whose path has no length yet has no copies, only its original. The original's
/// picture is still in use: it comes back as soon as the array is given a path.
#[test]
fn a_picture_used_only_by_an_arrays_original_is_not_thrown_away() {
    let mut d = Document::new();
    let (key, img) = picture(&mut d);
    let mut array = Shape::new(ShapeKind::Group { children: Vec::new() }, Affine::IDENTITY, 0);
    array.array = Some(Box::new(ArrayDef { source: vec![img.clone()], kind: ArrayKind::AlongPath(AlongDef::default()) }));
    let id = d.add(array);
    assert_eq!(d.get(id).unwrap().image_keys(), vec![key.clone()]);
    // An original that is a group, inside an array that is itself an original.
    let inner = Shape::new(ShapeKind::Group { children: vec![img] }, Affine::IDENTITY, 0);
    let mut nested = Shape::new(ShapeKind::Group { children: Vec::new() }, Affine::IDENTITY, 0);
    nested.array = Some(Box::new(ArrayDef { source: vec![inner], kind: ArrayKind::AlongPath(AlongDef::default()) }));
    let mut outer = Shape::new(ShapeKind::Group { children: Vec::new() }, Affine::IDENTITY, 0);
    outer.array = Some(Box::new(ArrayDef { source: vec![nested], kind: ArrayKind::AlongPath(AlongDef::default()) }));
    assert_eq!(outer.image_keys(), vec![key.clone()]);

    // Tidying the pictures up keeps it.
    d.prune_images();
    assert!(d.images.contains_key(&key), "the picture of the array's original is gone");
    // So does saving the project and opening it again.
    let back = project::load(&project::save(&d, "test", None).unwrap()).unwrap().doc;
    assert!(back.images.contains_key(&key), "the saved project lost the picture");
    // And copying the array to another document.
    let mut clip = Clipboard::default();
    clip.copy(&d, &[id]);
    let mut other = Document::new();
    Clipboard::from_text(&clip.to_text()).unwrap().paste(&mut other, PasteAt::InPlace);
    assert!(other.images.contains_key(&key), "the pasted array has no picture");
}

fn every_layer(s: &Shape, out: &mut Vec<usize>) {
    out.push(s.layer);
    if let ShapeKind::Group { children } = &s.kind {
        children.iter().for_each(|c| every_layer(c, out));
    }
    if let Some(a) = &s.array {
        a.source.iter().for_each(|c| every_layer(c, out));
    }
}

/// A group of two squares with an array in it, every part of it on a layer that does not exist.
fn on_layers_that_do_not_exist() -> Vec<Shape> {
    let far = LAYER_COUNT + 67;
    let square = Shape::rect(Rect::new(0.0, 0.0, 10.0, 10.0), 0.0, far);
    let inner = Shape::new(ShapeKind::Group { children: vec![square.clone()] }, Affine::IDENTITY, far);
    let mut array = Shape::new(ShapeKind::Group { children: vec![square.clone(), square.clone()] }, Affine::IDENTITY, far);
    array.array = Some(Box::new(ArrayDef { source: vec![inner], kind: ArrayKind::Grid(GridDef::default()) }));
    vec![square.clone(), Shape::new(ShapeKind::Group { children: vec![square, array] }, Affine::IDENTITY, far)]
}

/// Clipboard text can come from anywhere (another program version, the art library, a file
/// changed by hand). What is pasted lies on layers that exist, so the layer it is drawn in is
/// the layer that finds it.
#[test]
fn pasted_shapes_lie_on_layers_that_exist() {
    let clip = Clipboard { shapes: on_layers_that_do_not_exist(), ..Default::default() };
    let mut back = Clipboard::from_text(&clip.to_text()).unwrap();
    let mut d = Document::new();
    let ids = back.paste(&mut d, PasteAt::InPlace);
    let mut layers = Vec::new();
    d.iter().for_each(|s| every_layer(s, &mut layers));
    assert_eq!(layers.len(), 8);
    assert!(layers.iter().all(|l| *l == LAYER_COUNT - 1), "{layers:?}");
    // The last layer is the one in use, and it finds both shapes.
    assert_eq!(d.used_layers(), vec![LAYER_COUNT - 1]);
    assert_eq!(d.shapes_on_layer(LAYER_COUNT - 1), ids);
    d.delete_layer(LAYER_COUNT - 1, true);
    assert!(d.is_empty(), "deleting the layer with its shapes left {} of them", d.len());
}

/// The same for a project file: the originals of an array are tidied at every depth.
#[test]
fn an_opened_project_has_array_originals_on_layers_that_exist_at_every_depth() {
    let mut d = Document::new();
    for s in on_layers_that_do_not_exist() {
        d.add(s);
    }
    let back = project::load(&project::save(&d, "test", None).unwrap()).unwrap().doc;
    let mut layers = Vec::new();
    back.iter().for_each(|s| every_layer(s, &mut layers));
    assert_eq!(layers.len(), 8);
    assert!(layers.iter().all(|l| *l == LAYER_COUNT - 1), "{layers:?}");
}

/// Every number field takes a sum. One that nests brackets or signs deeper than anybody writes
/// (a pasted text) is refused as wrong text; it must not run the program out of stack, which
/// ends it on the spot with all unsaved work.
#[test]
fn a_sum_nested_absurdly_deep_is_refused_not_a_crash() {
    use darkburn_core::expr;
    let deep = |n: usize| -> Vec<String> {
        vec![format!("{}1{}", "(".repeat(n), ")".repeat(n)), format!("{}1", "-".repeat(n)), format!("{}1{}", "+-(".repeat(n), ")".repeat(n))]
    };
    let h = std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(move || {
            for text in deep(200_000) {
                assert!(expr::eval(&text).is_err(), "{} characters", text.len());
            }
            // Deep, but within what anyone writes, still works.
            for text in deep(100) {
                assert!(expr::eval(&text).is_ok(), "{text}");
            }
            assert_eq!(expr::eval_mm("((2in))-(-(1))").unwrap(), 51.8);
        })
        .unwrap();
    h.join().unwrap();
}
