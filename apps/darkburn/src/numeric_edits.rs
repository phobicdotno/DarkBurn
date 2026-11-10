//! The numeric edits in the ribbon: X, Y, width, height, scale and rotation of the selection
//! about a 9-point anchor, in machine coordinates, in millimetres or inches. Fields take
//! equations and units (`10in/4`); every edit is one undo step (a drag on a field is one).

use darkburn_core::Document;
use darkburn_core::ShapeId;
use darkburn_core::arrange::{self, Anchor};
use darkburn_core::kurbo::{Affine, Point};
use darkburn_ui::ui_kit::{self, BackingTooltipExt as _};
use egui::{Id, Pos2, Rect, Sense, Stroke, Ui, pos2, vec2};
use solidcraft_theme::Tokens;

use crate::numeric::{self, Num};
use crate::shell::DarkBurn;
use crate::view::Origin;

/// The work area, for converting between document and machine coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub origin: Origin,
    pub width: f64,
    pub height: f64,
}

impl Frame {
    /// Machine coordinates of document point `p` (document: Y up from the front-left corner).
    pub fn to_machine(self, p: Point) -> Point {
        // The page point (Y down from the top-left), then the origin's corner.
        let page = Point::new(p.x, self.height - p.y);
        Point::new(
            if self.origin.flips_x() { self.width - page.x } else { page.x },
            if self.origin.flips_y() { self.height - page.y } else { page.y },
        )
    }

    /// Document point of machine coordinates `m`.
    pub fn to_doc(self, m: Point) -> Point {
        let page =
            Point::new(if self.origin.flips_x() { self.width - m.x } else { m.x }, if self.origin.flips_y() { self.height - m.y } else { m.y });
        Point::new(page.x, self.height - page.y)
    }
}

/// One numeric edit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NumEdit {
    /// Machine X or Y of the anchor point, in millimetres.
    X(f64),
    Y(f64),
    /// Width or height in millimetres.
    W(f64),
    H(f64),
    /// Scale in percent across or down.
    ScaleX(f64),
    ScaleY(f64),
    /// Rotate by this many degrees (counter-clockwise).
    RotateBy(f64),
}

/// Apply `edit` to the shapes `ids` about `anchor`; `lock` keeps the aspect ratio.
pub fn apply(doc: &mut Document, ids: &[ShapeId], frame: Frame, anchor: Anchor, lock: bool, edit: NumEdit) -> bool {
    let Some(bb) = doc.bbox_of(ids) else { return false };
    let at = anchor.of(bb);
    let m = frame.to_machine(at);
    match edit {
        NumEdit::X(x) => arrange::place(doc, ids, anchor, frame.to_doc(Point::new(x, m.y))),
        NumEdit::Y(y) => arrange::place(doc, ids, anchor, frame.to_doc(Point::new(m.x, y))),
        NumEdit::W(w) => {
            let h = if lock && bb.width() > 1e-9 { bb.height() * w / bb.width() } else { bb.height() };
            arrange::resize(doc, ids, anchor, w, if h > 1e-9 { h } else { 1.0 })
        }
        NumEdit::H(h) => {
            let w = if lock && bb.height() > 1e-9 { bb.width() * h / bb.height() } else { bb.width() };
            arrange::resize(doc, ids, anchor, if w > 1e-9 { w } else { 1.0 }, h)
        }
        NumEdit::ScaleX(p) | NumEdit::ScaleY(p) => {
            if !(p.is_finite() && p > 0.0) {
                return false;
            }
            let f = p / 100.0;
            let (sx, sy) = match (edit, lock) {
                (_, true) => (f, f),
                (NumEdit::ScaleX(_), false) => (f, 1.0),
                _ => (1.0, f),
            };
            arrange::scale(doc, ids, anchor, sx, sy)
        }
        NumEdit::RotateBy(deg) => {
            if !deg.is_finite() || deg.abs() < 1e-12 {
                return false;
            }
            arrange::transform(doc, ids, arrange::about(at, Affine::rotate(deg.to_radians())))
        }
    }
}

/// The rotation the field shows: the shape's own for one shape, else 0.
pub fn shown_rotation(doc: &Document, ids: &[ShapeId]) -> f64 {
    match ids {
        [one] => doc.get(*one).map(|s| arrange::rotation_of(s.transform)).unwrap_or(0.0),
        _ => 0.0,
    }
}

/// Width of the numeric edits as drawn by [`show`].
pub const WIDTH: f32 = 497.0;

/// Draw the numeric edits from `at`, `height` points tall; returns the right edge.
pub fn show(ui: &mut Ui, at: Pos2, height: f32, app: &mut DarkBurn) -> f32 {
    let t = Tokens::get();
    let ids = app.editor.selected();
    let frame = Frame { origin: app.prefs.origin, width: app.prefs.page_width as f64, height: app.prefs.page_height as f64 };
    let anchor = app.prefs.anchor;
    let bb = app.editor.doc.bbox_of(&ids);
    let unit = if app.prefs.numeric_inches { 25.4 } else { 1.0 };
    let suffix = if app.prefs.numeric_inches { " in" } else { " mm" };
    let decimals = if app.prefs.numeric_inches { 3 } else { 2 };
    let (m, size) = match bb {
        Some(bb) => (frame.to_machine(anchor.of(bb)), (bb.width(), bb.height())),
        None => (Point::ZERO, (0.0, 0.0)),
    };
    let enabled = bb.is_some();
    let row_h = t.control_height;
    let gap = 4.0;
    let mut edits: Vec<(&'static str, NumEdit, bool)> = Vec::new();
    let mut x = at.x;

    // 9-point anchor.
    let cell = 13.0;
    let pad = 3.0;
    let grid =
        Rect::from_min_size(pos2(x, at.y + (height - (3.0 * cell + 2.0 * pad)) / 2.0 - 6.0), vec2(3.0 * cell + 2.0 * pad, 3.0 * cell + 2.0 * pad));
    for (i, a) in Anchor::ALL.iter().enumerate() {
        let (cx, cy) = ((i % 3) as f32, (i / 3) as f32);
        let r = Rect::from_min_size(pos2(grid.left() + cx * (cell + pad), grid.top() + cy * (cell + pad)), vec2(cell, cell));
        let resp = ui.interact(r, Id::unique(("numeric_anchor", i)), Sense::click());
        let on = *a == anchor;
        let fill = if on {
            t.accent
        } else if resp.hovered() {
            t.hover
        } else {
            t.control_fill
        };
        ui.painter().rect(r, 2.0, fill, Stroke::new(1.0, if on { t.accent } else { t.border }), egui::StrokeKind::Inside);
        if resp.backing_on_hover_text("Anchor: the point position, size, scale and rotation work about").clicked() {
            app.prefs.anchor = *a;
        }
    }
    x = grid.right() + 8.0;

    // A column of two labelled fields.
    let mut column = |ui: &mut Ui, x: f32, label_w: f32, field_w: f32, rows: Vec<(&'static str, f64, Num, &dyn Fn(f64) -> NumEdit)>| -> f32 {
        for (k, (label, value, num, make)) in rows.into_iter().enumerate() {
            let y = at.y + k as f32 * (row_h + gap);
            let lr = Rect::from_min_size(pos2(x, y), vec2(label_w, row_h));
            ui.painter().text(lr.left_center(), egui::Align2::LEFT_CENTER, label, egui::FontId::proportional(t.font_small), t.text_dim);
            let fr = Rect::from_min_size(pos2(x + label_w, y), vec2(field_w, row_h));
            let mut v = value;
            let ed = ui
                .scope_builder(egui::UiBuilder::new().max_rect(fr).id_salt(("numeric_edit", label, k)), |ui| {
                    ui.add_enabled_ui(enabled, |ui| {
                        let ed = numeric::field_sized(ui, &mut v, &num, Some(field_w));
                        // Named after the label painted beside it, not after its unit ("mm").
                        crate::a11y::number(ui, &ed.response, label);
                        ed
                    })
                    .inner
                })
                .inner;
            if ed.changed && enabled {
                edits.push((label, make(v), ed.continues()));
            }
        }
        x + label_w + field_w
    };
    let len = |v: f64| v / unit;
    let lnum = Num::new(-100_000.0..=100_000.0, decimals, suffix).unit(unit).speed(0.1 / unit);
    let snum = Num::new(0.0001..=100_000.0, decimals, suffix).unit(unit).speed(0.1 / unit);
    x = column(ui, x, 14.0, 92.0, vec![("X", len(m.x), lnum.clone(), &|v| NumEdit::X(v * unit)), ("Y", len(m.y), lnum, &|v| NumEdit::Y(v * unit))])
        + 8.0;
    x = column(
        ui,
        x,
        16.0,
        92.0,
        vec![("W", len(size.0), snum.clone(), &|v| NumEdit::W(v * unit)), ("H", len(size.1), snum, &|v| NumEdit::H(v * unit))],
    ) + 8.0;
    let pnum = Num::new(0.01..=100_000.0, 1, " %").speed(0.5);
    x = column(ui, x, 24.0, 66.0, vec![("W %", 100.0, pnum.clone(), &NumEdit::ScaleX), ("H %", 100.0, pnum, &NumEdit::ScaleY)]) + 8.0;
    let rot = shown_rotation(&app.editor.doc, &ids);
    let rnum = Num::new(-36_000.0..=36_000.0, 2, " deg").speed(0.5);
    let rx = x;
    x = column(ui, x, 40.0, 76.0, vec![("Rotate", rot, rnum, &move |v| NumEdit::RotateBy(v - rot))]);
    // Under the rotate field: the units toggle and lock aspect.
    let y2 = at.y + row_h + gap;
    let units_r = Rect::from_min_size(pos2(rx, y2), vec2(40.0, row_h));
    let label = if app.prefs.numeric_inches { "in" } else { "mm" };
    if ui_kit::button_at(ui, "numeric_units", units_r, label, ui_kit::Button::Small).backing_on_hover_text("Show millimetres or inches").clicked() {
        app.prefs.numeric_inches = !app.prefs.numeric_inches;
    }
    let lock_r = Rect::from_min_size(pos2(units_r.right() + 6.0, y2), vec2(x - units_r.right() - 6.0, row_h));
    ui.scope_builder(egui::UiBuilder::new().max_rect(lock_r).layout(egui::Layout::left_to_right(egui::Align::Center)), |ui| {
        ui_kit::checkbox(ui, "Lock", &mut app.prefs.lock_aspect).backing_on_hover_text("Keep the width-to-height ratio");
    });

    // Apply.
    let lock = app.prefs.lock_aspect;
    let ctx = ui.ctx().clone();
    for (label, edit, merge) in edits {
        let label_text = match edit {
            NumEdit::X(_) | NumEdit::Y(_) => "Move",
            NumEdit::W(_) | NumEdit::H(_) => "Resize",
            NumEdit::ScaleX(_) | NumEdit::ScaleY(_) => "Scale",
            NumEdit::RotateBy(_) => "Rotate",
        };
        let ids = ids.clone();
        numeric::commit(&mut app.editor, &ctx, Id::unique(("numeric_edit", label)), label_text, merge, |d, _| {
            apply(d, &ids, frame, anchor, lock, edit)
        });
    }
    x
}

#[cfg(test)]
mod tests {
    use super::*;
    use darkburn_core::Shape;
    use darkburn_core::kurbo::Rect as KRect;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn machine_coordinates_follow_the_origin() {
        let f = Frame { origin: Origin::FrontLeft, width: 400.0, height: 300.0 };
        assert_eq!(f.to_machine(Point::new(10.0, 20.0)), Point::new(10.0, 20.0), "front-left: same as the document");
        let f = Frame { origin: Origin::RearRight, ..f };
        assert_eq!(f.to_machine(Point::new(10.0, 20.0)), Point::new(390.0, 280.0));
        for o in Origin::ALL {
            let f = Frame { origin: o, ..f };
            let p = Point::new(12.5, 33.0);
            assert_eq!(f.to_doc(f.to_machine(p)), p);
        }
    }

    fn doc() -> (Document, Vec<ShapeId>) {
        let mut d = Document::new();
        let a = d.add(Shape::rect(KRect::new(10.0, 10.0, 30.0, 20.0), 0.0, 0));
        (d, vec![a])
    }

    const FRAME: Frame = Frame { origin: Origin::FrontLeft, width: 400.0, height: 400.0 };

    #[test]
    fn position_about_the_anchor() {
        let (mut d, ids) = doc();
        assert!(apply(&mut d, &ids, FRAME, Anchor::BottomLeft, true, NumEdit::X(100.0)));
        let bb = d.bbox_of(&ids).unwrap();
        assert!(close(bb.x0, 100.0) && close(bb.y0, 10.0));
        assert!(apply(&mut d, &ids, FRAME, Anchor::Centre, true, NumEdit::Y(50.0)));
        assert!(close(d.bbox_of(&ids).unwrap().center().y, 50.0));
        let rear = Frame { origin: Origin::RearLeft, ..FRAME };
        assert!(apply(&mut d, &ids, rear, Anchor::TopLeft, true, NumEdit::Y(0.0)));
        assert!(close(d.bbox_of(&ids).unwrap().y1, 400.0), "rear origin: Y 0 is the back edge");
    }

    #[test]
    fn size_with_and_without_lock() {
        let (mut d, ids) = doc();
        assert!(apply(&mut d, &ids, FRAME, Anchor::BottomLeft, true, NumEdit::W(40.0)));
        let bb = d.bbox_of(&ids).unwrap();
        assert!(close(bb.width(), 40.0) && close(bb.height(), 20.0) && close(bb.x0, 10.0) && close(bb.y0, 10.0));
        assert!(apply(&mut d, &ids, FRAME, Anchor::TopRight, false, NumEdit::H(5.0)));
        let bb2 = d.bbox_of(&ids).unwrap();
        assert!(close(bb2.height(), 5.0) && close(bb2.width(), 40.0) && close(bb2.y1, bb.y1) && close(bb2.x1, bb.x1));
        assert!(!apply(&mut d, &ids, FRAME, Anchor::Centre, false, NumEdit::W(-3.0)), "a negative size is refused");
    }

    #[test]
    fn scale_and_rotate() {
        let (mut d, ids) = doc();
        assert!(apply(&mut d, &ids, FRAME, Anchor::Centre, true, NumEdit::ScaleX(200.0)));
        let bb = d.bbox_of(&ids).unwrap();
        assert!(close(bb.width(), 40.0) && close(bb.height(), 20.0) && close(bb.center().x, 20.0));
        assert!(apply(&mut d, &ids, FRAME, Anchor::Centre, false, NumEdit::ScaleY(50.0)));
        assert!(close(d.bbox_of(&ids).unwrap().height(), 10.0));
        assert!(!apply(&mut d, &ids, FRAME, Anchor::Centre, false, NumEdit::ScaleY(0.0)));
        assert!(apply(&mut d, &ids, FRAME, Anchor::BottomLeft, false, NumEdit::RotateBy(90.0)));
        assert!(close(shown_rotation(&d, &ids), 90.0));
        let bb = d.bbox_of(&ids).unwrap();
        assert!(close(bb.width(), 10.0) && close(bb.height(), 40.0));
        assert!(close(bb.x1, 0.0), "turned about the bottom-left corner");
    }
}
