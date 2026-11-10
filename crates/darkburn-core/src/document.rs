//! The document: shapes in draw order, the 32 layers with their output priority, embedded
//! images and notes. Shapes are shared (`Arc`) so a copy of the document for undo is cheap.

use std::collections::BTreeMap;
use std::sync::Arc;

use kurbo::Rect;
use serde::{Deserialize, Serialize};

use crate::layer::{LAYER_COUNT, Layer, default_layers};
use crate::shape::{Shape, ShapeId, ShapeKind};

/// The pixels of an embedded image: the file as it was imported (PNG, JPEG, …).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageData {
    /// File extension of the encoded bytes: "png", "jpg", "bmp", "gif", "tif", "webp".
    pub format: String,
    pub bytes: Arc<[u8]>,
    pub px_width: u32,
    pub px_height: u32,
}

impl ImageData {
    /// A stable key for these bytes (FNV-1a, hex).
    pub fn key_for(bytes: &[u8]) -> String {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in bytes {
            h ^= *b as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        format!("{h:016x}")
    }
}

/// Embedded images by key.
pub type ImageStore = BTreeMap<String, Arc<ImageData>>;

/// A design.
#[derive(Clone, Debug, PartialEq)]
pub struct Document {
    /// Top-level shapes, bottom (drawn first) to top.
    pub shapes: Vec<Arc<Shape>>,
    /// The 32 layers, by index.
    pub layers: Vec<Layer>,
    /// Layer output order, first burned first. Holds every layer index once.
    pub layer_order: Vec<usize>,
    /// Embedded images, by key.
    pub images: ImageStore,
    pub notes: String,
    /// Open the notes when the project is opened.
    pub show_notes_on_load: bool,
    /// Job settings saved with the project (optimization, job origin): an opaque, versioned JSON
    /// value owned by the app. None until the user changes them.
    pub job_settings: Option<serde_json::Value>,
    /// Variable text state (serial number, merge row, auto-advance), saved with the project.
    pub variables: darkburn_text::VariableState,
    /// Camera, rotary, cylinder and print-and-cut setup saved with the project (opaque JSON owned
    /// by the app's vision code). None until the user sets any of it up.
    pub vision_settings: Option<serde_json::Value>,
    next_id: u64,
}

impl Default for Document {
    fn default() -> Self {
        Document::new()
    }
}

/// Shapes or layers a change touched (for status messages and tests).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    pub shapes: usize,
}

impl Document {
    pub fn new() -> Document {
        Document {
            shapes: Vec::new(),
            layers: default_layers(),
            layer_order: (0..LAYER_COUNT).collect(),
            images: ImageStore::new(),
            notes: String::new(),
            show_notes_on_load: false,
            job_settings: None,
            variables: Default::default(),
            vision_settings: None,
            next_id: 1,
        }
    }

    /// A fresh id.
    pub fn alloc_id(&mut self) -> ShapeId {
        let id = ShapeId(self.next_id);
        self.next_id += 1;
        id
    }

    /// The next id that would be given out (saved with the project).
    pub fn next_id(&self) -> u64 {
        self.next_id
    }

    pub(crate) fn set_next_id(&mut self, next: u64) {
        let max = self.shapes.iter().map(|s| max_id(s)).max().unwrap_or(0);
        self.next_id = next.max(max + 1);
    }

    /// Give `shape` (and its children) fresh ids.
    pub fn renumber(&mut self, shape: &mut Shape) {
        shape.id = self.alloc_id();
        if let ShapeKind::Group { children } = &mut shape.kind {
            for c in children {
                self.renumber(c);
            }
        }
    }

    /// Add `shape` on top with fresh ids; returns its id.
    pub fn add(&mut self, mut shape: Shape) -> ShapeId {
        self.renumber(&mut shape);
        let id = shape.id;
        self.shapes.push(Arc::new(shape));
        id
    }

    /// Insert `shape` at draw position `index` with fresh ids.
    pub fn insert(&mut self, index: usize, mut shape: Shape) -> ShapeId {
        self.renumber(&mut shape);
        let id = shape.id;
        self.shapes.insert(index.min(self.shapes.len()), Arc::new(shape));
        id
    }

    pub fn index_of(&self, id: ShapeId) -> Option<usize> {
        self.shapes.iter().position(|s| s.id == id)
    }

    pub fn get(&self, id: ShapeId) -> Option<&Shape> {
        self.shapes.iter().find(|s| s.id == id).map(|s| s.as_ref())
    }

    /// The shape to change (copied first if an undo step still shares it).
    pub fn get_mut(&mut self, id: ShapeId) -> Option<&mut Shape> {
        self.shapes.iter_mut().find(|s| s.id == id).map(Arc::make_mut)
    }

    /// Remove the shapes `ids`; returns how many were removed.
    pub fn remove(&mut self, ids: &[ShapeId]) -> usize {
        let before = self.shapes.len();
        self.shapes.retain(|s| !ids.contains(&s.id));
        before - self.shapes.len()
    }

    /// Shapes in draw order.
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &Shape> {
        self.shapes.iter().map(|s| s.as_ref())
    }

    pub fn len(&self) -> usize {
        self.shapes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.shapes.is_empty()
    }

    pub fn layer(&self, i: usize) -> &Layer {
        &self.layers[i.min(LAYER_COUNT - 1)]
    }

    pub fn layer_mut(&mut self, i: usize) -> &mut Layer {
        &mut self.layers[i.min(LAYER_COUNT - 1)]
    }

    /// Layers that hold at least one shape, in output order (the Cuts / Layers list).
    pub fn used_layers(&self) -> Vec<usize> {
        let mut used = [false; LAYER_COUNT];
        for s in self.iter() {
            for l in s.layers() {
                used[l.min(LAYER_COUNT - 1)] = true;
            }
        }
        self.layer_order.iter().copied().filter(|l| used[*l]).collect()
    }

    /// Move layer `layer` earlier (`delta` below zero) or later by that many places among the used
    /// layers, as far as the list goes. False when it is already at that end.
    pub fn move_layer(&mut self, layer: usize, delta: isize) -> bool {
        let mut moved = false;
        for _ in 0..delta.unsigned_abs().min(LAYER_COUNT) {
            let used = self.used_layers();
            let Some(pos) = used.iter().position(|l| *l == layer) else { return false };
            let to = pos as isize + delta.signum();
            if to < 0 || to as usize >= used.len() {
                break;
            }
            self.swap_order(layer, used[to as usize]);
            moved = true;
        }
        moved
    }

    /// Put `layer` just before `before` in the output order (drag and drop in the list).
    pub fn place_layer_before(&mut self, layer: usize, before: Option<usize>) {
        self.layer_order.retain(|l| *l != layer);
        let at = before.and_then(|b| self.layer_order.iter().position(|l| *l == b)).unwrap_or(self.layer_order.len());
        self.layer_order.insert(at, layer);
    }

    fn swap_order(&mut self, a: usize, b: usize) {
        let ia = self.layer_order.iter().position(|l| *l == a);
        let ib = self.layer_order.iter().position(|l| *l == b);
        if let (Some(ia), Some(ib)) = (ia, ib) {
            self.layer_order.swap(ia, ib);
        }
    }

    /// Move `layers` to the end of the output order, keeping their relative order ("Sort cuts
    /// last": layers in Line mode go after fills and images).
    pub fn sort_cuts_last(&mut self) {
        let (mut first, last): (Vec<usize>, Vec<usize>) =
            self.layer_order.iter().partition(|l| self.layers[**l].fills() || self.layers[**l].main().mode == crate::layer::Mode::Image);
        first.extend(last);
        self.layer_order = first;
    }

    /// Ids of the top-level shapes on layer `layer` (a group counts when any child is on it).
    pub fn shapes_on_layer(&self, layer: usize) -> Vec<ShapeId> {
        self.iter().filter(|s| s.layers().contains(&layer)).map(|s| s.id).collect()
    }

    /// Delete layer `layer`: its settings go back to the defaults; with `shapes`, its shapes go too.
    pub fn delete_layer(&mut self, layer: usize, shapes: bool) {
        if shapes {
            // A group loses only its children on the layer; it goes when none are left.
            let old = std::mem::take(&mut self.shapes);
            for s in old {
                if !s.layers().contains(&layer) {
                    self.shapes.push(s);
                } else if let Some(kept) = without_layer(&s, layer) {
                    self.shapes.push(Arc::new(kept));
                }
            }
        }
        self.layers[layer] = Layer::new(layer);
    }

    /// Bounding box of the shapes `ids`.
    pub fn bbox_of(&self, ids: &[ShapeId]) -> Option<Rect> {
        ids.iter().filter_map(|id| self.get(*id)).map(Shape::bbox).reduce(|a, b| a.union(b))
    }

    /// Bounding box of everything.
    pub fn bbox(&self) -> Option<Rect> {
        self.iter().map(Shape::bbox).reduce(|a, b| a.union(b))
    }

    /// Whether shape `s` would be drawn filled.
    pub fn is_filled(&self, s: &Shape) -> bool {
        s.is_closed() && s.layers().iter().any(|l| self.layer(*l).fills())
    }

    /// Whether the shape can be clicked: on a shown layer and not locked.
    pub fn is_selectable(&self, s: &Shape) -> bool {
        !s.locked && s.layers().iter().any(|l| self.layer(*l).show)
    }

    /// Add `data` to the image store; returns its key.
    pub fn add_image(&mut self, data: ImageData) -> String {
        let key = ImageData::key_for(&data.bytes);
        self.images.entry(key.clone()).or_insert_with(|| Arc::new(data));
        key
    }

    /// Drop images no shape uses.
    pub fn prune_images(&mut self) {
        let used: std::collections::BTreeSet<String> = self.iter().flat_map(Shape::image_keys).collect();
        self.images.retain(|k, _| used.contains(k));
    }
}

fn max_id(s: &Shape) -> u64 {
    match &s.kind {
        ShapeKind::Group { children } => children.iter().map(max_id).max().unwrap_or(0).max(s.id.0),
        _ => s.id.0,
    }
}

/// `shape` without everything on layer `layer`: `None` when nothing is left of it.
fn without_layer(shape: &Shape, layer: usize) -> Option<Shape> {
    match &shape.kind {
        ShapeKind::Group { children } => {
            let kept: Vec<Shape> = children.iter().filter_map(|c| without_layer(c, layer)).collect();
            let first = kept.first()?.layer;
            let mut group = shape.clone();
            if group.layer == layer {
                group.layer = first;
            }
            group.kind = ShapeKind::Group { children: kept };
            // A virtual array loses its originals on the layer too, or they would be back the next
            // time the array is edited.
            if let Some(a) = &mut group.array {
                a.source = a.source.iter().filter_map(|s| without_layer(s, layer)).collect();
            }
            Some(group)
        }
        _ => (shape.layer != layer).then(|| shape.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layer::Mode;
    use kurbo::Rect;

    fn rect(x: f64, layer: usize) -> Shape {
        Shape::rect(Rect::new(x, 0.0, x + 10.0, 10.0), 0.0, layer)
    }

    #[test]
    fn add_get_remove() {
        let mut d = Document::new();
        let a = d.add(rect(0.0, 0));
        let b = d.add(rect(20.0, 3));
        assert_ne!(a, b);
        assert_eq!(d.index_of(b), Some(1));
        d.get_mut(a).unwrap().name = "first".into();
        assert_eq!(d.get(a).unwrap().name, "first");
        assert_eq!(d.used_layers(), vec![0, 3]);
        assert_eq!(d.remove(&[a]), 1);
        assert!(d.get(a).is_none());
        assert_eq!(d.used_layers(), vec![3]);
        let bb = d.bbox().unwrap();
        assert!((bb.x0 - 20.0).abs() < 1e-9);
    }

    #[test]
    fn copies_share_shapes_until_changed() {
        let mut d = Document::new();
        let a = d.add(rect(0.0, 0));
        let snapshot = d.clone();
        assert!(Arc::ptr_eq(&d.shapes[0], &snapshot.shapes[0]));
        d.get_mut(a).unwrap().layer = 5;
        assert!(!Arc::ptr_eq(&d.shapes[0], &snapshot.shapes[0]));
        assert_eq!(snapshot.get(a).unwrap().layer, 0, "the copy is untouched");
    }

    #[test]
    fn layer_priority_moves() {
        let mut d = Document::new();
        d.add(rect(0.0, 4));
        d.add(rect(0.0, 1));
        d.add(rect(0.0, 9));
        assert_eq!(d.used_layers(), vec![1, 4, 9]);
        assert!(d.move_layer(9, -1));
        assert_eq!(d.used_layers(), vec![1, 9, 4]);
        assert!(!d.move_layer(1, -1));
        // Two places is two steps, not a swap with the layer two places on: 1 9 4 -> 9 4 1.
        assert!(d.move_layer(1, 2));
        assert_eq!(d.used_layers(), vec![9, 4, 1]);
        assert!(d.move_layer(1, -5), "further than the list goes: to the front");
        assert_eq!(d.used_layers(), vec![1, 9, 4]);
        d.place_layer_before(4, Some(1));
        assert_eq!(d.used_layers(), vec![4, 1, 9]);
        d.layer_mut(9).main_mut().mode = Mode::Fill;
        d.sort_cuts_last();
        assert_eq!(d.used_layers(), vec![9, 4, 1]);
        assert_eq!(d.layer_order.len(), LAYER_COUNT);
    }

    #[test]
    fn delete_layer_with_and_without_shapes() {
        let mut d = Document::new();
        d.add(rect(0.0, 2));
        d.layer_mut(2).main_mut().speed = 5.0;
        d.delete_layer(2, false);
        assert_eq!(d.len(), 1);
        assert_eq!(d.layer(2).main().speed, 100.0);
        d.delete_layer(2, true);
        assert!(d.is_empty());
    }

    #[test]
    fn deleting_a_layer_with_its_shapes_keeps_the_rest_of_a_group() {
        let mut d = Document::new();
        let inner = Shape::new(ShapeKind::Group { children: vec![rect(0.0, 2), rect(10.0, 2)] }, kurbo::Affine::IDENTITY, 2);
        let group = Shape::new(ShapeKind::Group { children: vec![rect(0.0, 2), rect(20.0, 5), inner, rect(40.0, 7)] }, kurbo::Affine::IDENTITY, 2);
        let g = d.add(group);
        let only = d.add(Shape::new(ShapeKind::Group { children: vec![rect(0.0, 2)] }, kurbo::Affine::IDENTITY, 2));
        d.add(rect(0.0, 2));
        d.add(rect(0.0, 3));
        d.delete_layer(2, true);
        assert_eq!(d.len(), 2, "the mixed group and the shape on layer 3 stay");
        assert!(d.get(only).is_none(), "a group with nothing left goes");
        let kept = d.get(g).expect("the group stays");
        match &kept.kind {
            ShapeKind::Group { children } => assert_eq!(children.iter().map(|c| c.layer).collect::<Vec<_>>(), vec![5, 7]),
            other => panic!("{other:?}"),
        }
        assert_eq!(d.used_layers(), vec![3, 5, 7]);
    }

    #[test]
    fn images_are_stored_once_and_pruned() {
        let mut d = Document::new();
        let data = ImageData { format: "png".into(), bytes: Arc::from(&b"abc"[..]), px_width: 1, px_height: 1 };
        let k1 = d.add_image(data.clone());
        let k2 = d.add_image(data);
        assert_eq!(k1, k2);
        assert_eq!(d.images.len(), 1);
        d.prune_images();
        assert!(d.images.is_empty());
    }

    #[test]
    fn hidden_and_locked_shapes_are_not_selectable() {
        let mut d = Document::new();
        let a = d.add(rect(0.0, 1));
        assert!(d.is_selectable(d.get(a).unwrap()));
        d.layer_mut(1).show = false;
        assert!(!d.is_selectable(d.get(a).unwrap()));
        d.layer_mut(1).show = true;
        d.get_mut(a).unwrap().locked = true;
        assert!(!d.is_selectable(d.get(a).unwrap()));
    }
}
