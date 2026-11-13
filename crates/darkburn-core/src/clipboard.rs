//! The clipboard: copied shapes with the images they use, and copied cut settings.
//! It can travel as text through the system clipboard to another running DarkBurn.

use std::sync::Arc;

use kurbo::{Affine, Point, Vec2};
use serde::{Deserialize, Serialize};

use crate::document::{Document, ImageData, ImageStore};
use crate::layer::CutSetting;
use crate::shape::{Shape, ShapeId};

/// First line of DarkBurn clipboard text, so pasted text is recognised.
pub const CLIPBOARD_MARK: &str = "darkburn-clipboard/1";

/// How far each repeated paste is shifted, in millimetres.
pub const PASTE_STEP: f64 = 5.0;

#[derive(Clone, Debug, Default)]
pub struct Clipboard {
    pub shapes: Vec<Shape>,
    pub images: ImageStore,
    /// Pastes since the last copy (each plain paste moves a step further).
    pub pastes: u32,
    /// Copied cut settings (Cuts / Layers copy and paste).
    pub cut_settings: Option<Vec<CutSetting>>,
}

/// Where pasted shapes go.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PasteAt {
    /// Shifted a step from the original, a step further each time.
    Offset,
    /// Exactly where they were copied from.
    InPlace,
    /// Centred on a point (paste at the cursor).
    Centre(Point),
}

impl Clipboard {
    pub fn is_empty(&self) -> bool {
        self.shapes.is_empty()
    }

    /// Copy `ids` from `doc` in draw order.
    pub fn copy(&mut self, doc: &Document, ids: &[ShapeId]) {
        self.shapes = doc.iter().filter(|s| ids.contains(&s.id)).cloned().collect();
        self.images = self.shapes.iter().flat_map(Shape::image_keys).filter_map(|k| doc.images.get(&k).map(|d| (k, d.clone()))).collect();
        self.pastes = 0;
    }

    /// Paste into `doc`; returns the new ids (to select).
    pub fn paste(&mut self, doc: &mut Document, at: PasteAt) -> Vec<ShapeId> {
        if self.shapes.is_empty() {
            return Vec::new();
        }
        let shift = match at {
            PasteAt::Offset => {
                self.pastes += 1;
                Vec2::new(PASTE_STEP, -PASTE_STEP) * self.pastes as f64
            }
            PasteAt::InPlace => Vec2::ZERO,
            PasteAt::Centre(p) => {
                let bb = self.shapes.iter().map(Shape::bbox).reduce(|a, b| a.union(b)).unwrap_or_default();
                p - bb.center()
            }
        };
        for (k, d) in &self.images {
            doc.images.entry(k.clone()).or_insert_with(|| d.clone());
        }
        self.shapes
            .iter()
            .map(|s| {
                let mut s = s.clone();
                s.transform = Affine::translate(shift) * s.transform;
                doc.add(s)
            })
            .collect()
    }

    /// The clipboard as text for the system clipboard.
    pub fn to_text(&self) -> String {
        let images = self
            .images
            .iter()
            .map(|(k, d)| ClipImage { key: k.clone(), format: d.format.clone(), px_width: d.px_width, px_height: d.px_height, data: hex(&d.bytes) })
            .collect();
        let body = ClipText { shapes: self.shapes.clone(), images };
        format!("{CLIPBOARD_MARK}\n{}", serde_json::to_string(&body).unwrap_or_default())
    }

    /// Read clipboard text written by [`Clipboard::to_text`]; None when it is something else.
    /// The text may come from anywhere: its shapes are put on layers that exist, as a
    /// project's are when it is opened.
    pub fn from_text(text: &str) -> Option<Clipboard> {
        let body = text.strip_prefix(CLIPBOARD_MARK)?.trim_start();
        let mut body: ClipText = serde_json::from_str(body).ok()?;
        body.shapes.iter_mut().for_each(crate::project::on_real_layers);
        let mut images = ImageStore::new();
        for i in body.images {
            let bytes = unhex(&i.data)?;
            images.insert(i.key, Arc::new(ImageData { format: i.format, bytes: Arc::from(bytes), px_width: i.px_width, px_height: i.px_height }));
        }
        Some(Clipboard { shapes: body.shapes, images, pastes: 0, cut_settings: None })
    }
}

#[derive(Serialize, Deserialize)]
struct ClipText {
    shapes: Vec<Shape>,
    images: Vec<ClipImage>,
}

#[derive(Serialize, Deserialize)]
struct ClipImage {
    key: String,
    format: String,
    px_width: u32,
    px_height: u32,
    data: String,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len()).step_by(2).map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shape::{ImageShape, ShapeKind};
    use kurbo::Rect;

    #[test]
    fn copy_and_paste_shift_each_time() {
        let mut d = Document::new();
        let a = d.add(Shape::rect(Rect::new(0.0, 0.0, 10.0, 10.0), 0.0, 2));
        let mut c = Clipboard::default();
        c.copy(&d, &[a]);
        let p1 = c.paste(&mut d, PasteAt::Offset);
        let p2 = c.paste(&mut d, PasteAt::Offset);
        assert_eq!(d.len(), 3);
        assert_ne!(p1[0], a);
        let x = |d: &Document, id| d.get(id).unwrap().bbox().x0;
        assert!((x(&d, p1[0]) - 5.0).abs() < 1e-9 && (x(&d, p2[0]) - 10.0).abs() < 1e-9);
        let p3 = c.paste(&mut d, PasteAt::InPlace);
        assert!(x(&d, p3[0]).abs() < 1e-9);
        let p4 = c.paste(&mut d, PasteAt::Centre(Point::new(100.0, 100.0)));
        assert!((d.get(p4[0]).unwrap().bbox().center() - Point::new(100.0, 100.0)).hypot() < 1e-9);
        assert_eq!(d.get(p4[0]).unwrap().layer, 2);
    }

    #[test]
    fn clipboard_text_round_trips_with_images() {
        let mut d = Document::new();
        let key = d.add_image(ImageData { format: "png".into(), bytes: Arc::from(&[1u8, 2, 255][..]), px_width: 3, px_height: 1 });
        let img = d.add(Shape::new(ShapeKind::Image(ImageShape { image: key.clone(), ..Default::default() }), Affine::IDENTITY, 0));
        let mut c = Clipboard::default();
        c.copy(&d, &[img]);
        let text = c.to_text();
        assert!(text.starts_with(CLIPBOARD_MARK));
        let mut back = Clipboard::from_text(&text).unwrap();
        let mut other = Document::new();
        back.paste(&mut other, PasteAt::InPlace);
        assert_eq!(other.len(), 1);
        assert_eq!(&other.images[&key].bytes[..], &[1, 2, 255]);
        assert!(Clipboard::from_text("plain words").is_none());
    }
}
