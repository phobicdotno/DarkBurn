//! The Shape Properties panel: a property grid for the selection. With several shapes selected
//! a field whose values differ shows blank, and an edit sets it on every shape that has it.

use darkburn_core::layer::{LAYER_COUNT, layer_label};
use darkburn_core::shape::{AlignX, AlignY};
use darkburn_core::{Document, Shape, ShapeId, ShapeKind, TextShape};
use darkburn_ui::ui_kit;
use egui::{Id, RichText, Ui};
use solidcraft_theme::Tokens;

use crate::numeric::{self, Num};
use crate::shell::DarkBurn;

/// The values the selected shapes have for one field: None when no shape has it.
pub fn common<T: PartialEq + Clone>(shapes: &[Shape], get: impl Fn(&Shape) -> Option<T>) -> Option<(T, bool)> {
    let mut values = shapes.iter().filter_map(&get);
    let first = values.next()?;
    let mixed = values.any(|v| v != first);
    Some((first, mixed))
}

/// Set a field on each of `ids` that has it; true when anything changed.
pub fn set_each<T: PartialEq>(doc: &mut Document, ids: &[ShapeId], get: impl Fn(&Shape) -> Option<T>, set: impl Fn(&mut Shape)) -> bool {
    let mut changed = false;
    for id in ids {
        let Some(before) = doc.get(*id).and_then(&get) else { continue };
        if let Some(s) = doc.get_mut(*id) {
            set(s);
            changed |= get(s).is_some_and(|after| after != before);
        }
    }
    changed
}

/// Scale of a shape's transform along its own X and Y axes.
pub fn transform_scale(s: &Shape) -> (f64, f64) {
    let c = s.transform.as_coeffs();
    (c[0].hypot(c[1]), c[2].hypot(c[3]))
}

/// Pixel size and resolution (dots per inch across and down) of an image shape.
pub fn image_resolution(doc: &Document, s: &Shape) -> Option<((u32, u32), (f64, f64))> {
    let ShapeKind::Image(img) = &s.kind else { return None };
    let data = doc.images.get(&img.image)?;
    let (sx, sy) = transform_scale(s);
    let (w, h) = (img.width * sx, img.height * sy);
    let dpi = |px: u32, mm: f64| if mm > 1e-9 { px as f64 / (mm / 25.4) } else { 0.0 };
    Some(((data.px_width, data.px_height), (dpi(data.px_width, w), dpi(data.px_height, h))))
}

struct Grid<'a> {
    app: &'a mut DarkBurn,
    ids: Vec<ShapeId>,
    shapes: Vec<Shape>,
}

impl Grid<'_> {
    fn number(&mut self, ui: &mut Ui, label: &str, num: Num, get: impl Fn(&Shape) -> Option<f64> + Copy, set: impl Fn(&mut Shape, f64) + Copy) {
        let Some((first, mixed)) = common(&self.shapes, get) else { return };
        let mut v = first;
        let num = num.mixed(mixed);
        let ed = ui.push_id(("sp", label), |ui| numeric::row(ui, label, &mut v, &num)).inner;
        if ed.changed {
            let merge = ed.continues();
            let ids = self.ids.clone();
            let ctx = ui.ctx().clone();
            numeric::commit(&mut self.app.editor, &ctx, Id::unique(("sp", label)), label, merge, |d, _| set_each(d, &ids, get, |s| set(s, v)));
        }
    }

    fn flag(&mut self, ui: &mut Ui, label: &str, get: impl Fn(&Shape) -> Option<bool> + Copy, set: impl Fn(&mut Shape, bool) + Copy) {
        let Some((first, mixed)) = common(&self.shapes, get) else { return };
        let mut v = first;
        if ui_kit::checkbox_indeterminate(ui, label, &mut v, mixed).changed() {
            let ids = self.ids.clone();
            self.app.editor.edit(label, |d, _| set_each(d, &ids, get, |s| set(s, v)));
        }
    }

    fn text(&mut self, ui: &mut Ui, label: &str, get: impl Fn(&Shape) -> Option<String> + Copy, set: impl Fn(&mut Shape, &str) + Copy) {
        let Some((first, mixed)) = common(&self.shapes, get) else { return };
        let mut v = if mixed { String::new() } else { first };
        let resp = ui_kit::row(ui, label, |ui| {
            ui.add_sized(
                egui::vec2(ui.available_width(), Tokens::get().control_height),
                egui::TextEdit::singleline(&mut v).id(Id::unique(("sp_text", label))).hint_text(if mixed { "Mixed" } else { "" }),
            )
        });
        if resp.changed() {
            // Typing goes on as one undo step while the field keeps focus.
            let merge = resp.has_focus() && !resp.gained_focus();
            let ids = self.ids.clone();
            let ctx = ui.ctx().clone();
            numeric::commit(&mut self.app.editor, &ctx, Id::unique(("sp_text", label)), label, merge, |d, _| set_each(d, &ids, get, |s| set(s, &v)));
        }
    }

    fn choice(
        &mut self,
        ui: &mut Ui,
        label: &str,
        options: &[&str],
        get: impl Fn(&Shape) -> Option<usize> + Copy,
        set: impl Fn(&mut Shape, usize) + Copy,
    ) {
        let Some((first, mixed)) = common(&self.shapes, get) else { return };
        let current = if mixed { "" } else { options.get(first).copied().unwrap_or("") };
        let mut picked = None;
        ui_kit::row(ui, label, |ui| {
            ui_kit::dropdown_menu(ui, ("sp_choice", label), current, |ui| {
                for (i, o) in options.iter().enumerate() {
                    if ui.selectable_label(!mixed && i == first, *o).clicked() {
                        picked = Some(i);
                    }
                }
            });
        });
        if let Some(i) = picked {
            let ids = self.ids.clone();
            self.app.editor.edit(label, |d, _| set_each(d, &ids, get, |s| set(s, i)));
        }
    }
}

fn text_of(s: &Shape) -> Option<&TextShape> {
    match &s.kind {
        ShapeKind::Text(t) => Some(t),
        _ => None,
    }
}

fn text_mut(s: &mut Shape) -> Option<&mut TextShape> {
    match &mut s.kind {
        ShapeKind::Text(t) => Some(t),
        _ => None,
    }
}

fn adjust_mut(s: &mut Shape) -> Option<&mut darkburn_core::ImageAdjust> {
    match &mut s.kind {
        ShapeKind::Image(i) => Some(&mut i.adjust),
        _ => None,
    }
}

fn adjust_of(s: &Shape) -> Option<&darkburn_core::ImageAdjust> {
    match &s.kind {
        ShapeKind::Image(i) => Some(&i.adjust),
        _ => None,
    }
}

const ALIGN_X: [AlignX; 3] = [AlignX::Left, AlignX::Middle, AlignX::Right];
const ALIGN_Y: [AlignY; 4] = [AlignY::Baseline, AlignY::Top, AlignY::Middle, AlignY::Bottom];

/// The panel.
pub fn show(ui: &mut Ui, app: &mut DarkBurn) {
    let t = Tokens::get();
    let ids = app.editor.selected();
    if ids.is_empty() {
        ui.label(RichText::new("Select a shape to see its properties.").size(t.font_small).color(t.text_dim));
        return;
    }
    let shapes: Vec<Shape> = ids.iter().filter_map(|id| app.editor.doc.get(*id).cloned()).collect();
    let title = if shapes.len() == 1 {
        shapes[0].kind_name().to_string()
    } else {
        let kinds = common(&shapes, |s| Some(s.kind_name()));
        match kinds {
            Some((k, false)) => format!("{} {}s", shapes.len(), k.to_lowercase()),
            _ => format!("{} shapes", shapes.len()),
        }
    };
    let image_info = if shapes.len() == 1 { image_resolution(&app.editor.doc, &shapes[0]) } else { None };
    let mut g = Grid { app, ids, shapes };
    ui_kit::section_heading(ui, &title);
    g.text(ui, "Name", |s| Some(s.name.clone()), |s, v| s.name = v.to_string());
    let layer_names: Vec<String> = (0..LAYER_COUNT).map(|l| format!("{}  {}", layer_label(l), g.app.editor.doc.layer(l).name)).collect();
    let layer_opts: Vec<&str> = layer_names.iter().map(|s| s.as_str()).collect();
    let layers_before = g.app.editor.revision();
    g.choice(ui, "Layer", &layer_opts, |s| Some(s.layer), |s, l| s.set_layer(l));
    if g.app.editor.revision() != layers_before {
        // The current layer follows the choice (groups move every child with Shape::set_layer).
        if let Some(l) = g.ids.first().and_then(|id| g.app.editor.doc.get(*id)).map(|s| s.layer) {
            g.app.editor.current_layer = l;
        }
    }
    g.flag(ui, "Locked", |s| Some(s.locked), |s, v| s.locked = v);
    g.number(
        ui,
        "Cut order priority",
        Num::new(-1000.0..=1000.0, 0, ""),
        |s| Some(s.priority as f64),
        |s, v| s.priority = numeric::whole(v).clamp(-1000, 1000) as i32,
    );
    g.number(ui, "Power scale", Num::new(0.0..=100.0, 1, " %").speed(0.5), |s| Some(s.power_scale), |s, v| s.power_scale = v.clamp(0.0, 100.0));

    let has = |g: &Grid, f: fn(&ShapeKind) -> bool| g.shapes.iter().any(|s| f(&s.kind));
    if has(&g, |k| matches!(k, ShapeKind::Rect { .. })) {
        ui_kit::section_heading(ui, "Rectangle");
        let get_w = |s: &Shape| if let ShapeKind::Rect { width, .. } = s.kind { Some(width) } else { None };
        let get_h = |s: &Shape| if let ShapeKind::Rect { height, .. } = s.kind { Some(height) } else { None };
        let get_r = |s: &Shape| if let ShapeKind::Rect { radius, .. } = s.kind { Some(radius) } else { None };
        g.number(ui, "Width", Num::new(0.001..=100_000.0, 2, " mm").speed(0.1), get_w, |s, v| {
            if let ShapeKind::Rect { width, .. } = &mut s.kind {
                *width = v.max(0.001);
            }
        });
        g.number(ui, "Height", Num::new(0.001..=100_000.0, 2, " mm").speed(0.1), get_h, |s, v| {
            if let ShapeKind::Rect { height, .. } = &mut s.kind {
                *height = v.max(0.001);
            }
        });
        g.number(ui, "Corner radius", Num::new(-10_000.0..=10_000.0, 2, " mm").speed(0.05), get_r, |s, v| {
            if let ShapeKind::Rect { radius, .. } = &mut s.kind {
                *radius = v;
            }
        });
        ui_kit::caption(ui, "Corner radius", "A negative radius cuts the corners inwards.");
    }
    if has(&g, |k| matches!(k, ShapeKind::Ellipse { .. })) {
        ui_kit::section_heading(ui, "Ellipse");
        g.number(
            ui,
            "Radius X",
            Num::new(0.001..=100_000.0, 2, " mm").speed(0.1),
            |s| if let ShapeKind::Ellipse { rx, .. } = s.kind { Some(rx) } else { None },
            |s, v| {
                if let ShapeKind::Ellipse { rx, .. } = &mut s.kind {
                    *rx = v.max(0.001);
                }
            },
        );
        g.number(
            ui,
            "Radius Y",
            Num::new(0.001..=100_000.0, 2, " mm").speed(0.1),
            |s| if let ShapeKind::Ellipse { ry, .. } = s.kind { Some(ry) } else { None },
            |s, v| {
                if let ShapeKind::Ellipse { ry, .. } = &mut s.kind {
                    *ry = v.max(0.001);
                }
            },
        );
    }
    if has(&g, |k| matches!(k, ShapeKind::Polygon { .. })) {
        ui_kit::section_heading(ui, "Polygon");
        g.number(
            ui,
            "Sides",
            Num::new(3.0..=1000.0, 0, ""),
            |s| if let ShapeKind::Polygon { sides, .. } = s.kind { Some(sides as f64) } else { None },
            |s, v| {
                if let ShapeKind::Polygon { sides, .. } = &mut s.kind {
                    *sides = numeric::whole(v).clamp(3, 1000) as u32;
                }
            },
        );
        g.number(
            ui,
            "Radius",
            Num::new(0.001..=100_000.0, 2, " mm").speed(0.1),
            |s| if let ShapeKind::Polygon { radius, .. } = s.kind { Some(radius) } else { None },
            |s, v| {
                if let ShapeKind::Polygon { radius, .. } = &mut s.kind {
                    *radius = v.max(0.001);
                }
            },
        );
    }
    if has(&g, |k| matches!(k, ShapeKind::Text(_))) {
        ui_kit::section_heading(ui, "Text");
        g.text(
            ui,
            "Text",
            |s| text_of(s).map(|t| t.text.clone()),
            |s, v| {
                if let Some(t) = text_mut(s) {
                    t.text = v.to_string();
                }
            },
        );
        g.number(
            ui,
            "Height",
            Num::new(0.1..=10_000.0, 2, " mm").speed(0.1),
            |s| text_of(s).map(|t| t.height),
            |s, v| {
                if let Some(t) = text_mut(s) {
                    t.height = v.max(0.1);
                }
            },
        );
        g.flag(
            ui,
            "Bold",
            |s| text_of(s).map(|t| t.bold),
            |s, v| {
                if let Some(t) = text_mut(s) {
                    t.bold = v;
                }
            },
        );
        g.flag(
            ui,
            "Italic",
            |s| text_of(s).map(|t| t.italic),
            |s, v| {
                if let Some(t) = text_mut(s) {
                    t.italic = v;
                }
            },
        );
        g.flag(
            ui,
            "Upper case",
            |s| text_of(s).map(|t| t.upper_case),
            |s, v| {
                if let Some(t) = text_mut(s) {
                    t.upper_case = v;
                }
            },
        );
        g.flag(
            ui,
            "Welded",
            |s| text_of(s).map(|t| t.welded),
            |s, v| {
                if let Some(t) = text_mut(s) {
                    t.welded = v;
                }
            },
        );
        g.number(
            ui,
            "Letter spacing",
            Num::new(-10.0..=100.0, 2, "").speed(0.01),
            |s| text_of(s).map(|t| t.letter_spacing),
            |s, v| {
                if let Some(t) = text_mut(s) {
                    t.letter_spacing = v;
                }
            },
        );
        g.number(
            ui,
            "Line spacing",
            Num::new(-10.0..=100.0, 2, "").speed(0.01),
            |s| text_of(s).map(|t| t.line_spacing),
            |s, v| {
                if let Some(t) = text_mut(s) {
                    t.line_spacing = v;
                }
            },
        );
        g.choice(
            ui,
            "Align X",
            &["Left", "Middle", "Right"],
            |s| text_of(s).and_then(|t| ALIGN_X.iter().position(|a| *a == t.align_x)),
            |s, i| {
                if let Some(t) = text_mut(s) {
                    t.align_x = ALIGN_X[i.min(2)];
                }
            },
        );
        g.choice(
            ui,
            "Align Y",
            &["Baseline", "Top", "Middle", "Bottom"],
            |s| text_of(s).and_then(|t| ALIGN_Y.iter().position(|a| *a == t.align_y)),
            |s, i| {
                if let Some(t) = text_mut(s) {
                    t.align_y = ALIGN_Y[i.min(3)];
                }
            },
        );
    }
    if has(&g, |k| matches!(k, ShapeKind::Text(_))) {
        crate::text_ui::properties_extra(ui, g.app);
    }
    if has(&g, |k| matches!(k, ShapeKind::Code(_))) {
        ui_kit::section_heading(ui, "Code");
        crate::text_ui::code_properties(ui, g.app);
    }
    if has(&g, |k| matches!(k, ShapeKind::Image(_))) {
        ui_kit::section_heading(ui, "Image");
        if let Some(((pw, ph), (dx, dy))) = image_info {
            ui_kit::row(ui, "Pixels", |ui| ui.label(format!("{pw} × {ph}")));
            ui_kit::row(ui, "Resolution", |ui| {
                ui.label(if (dx - dy).abs() < 0.5 { format!("{dx:.0} DPI") } else { format!("{dx:.0} × {dy:.0} DPI") })
            });
        }
        type A = darkburn_core::ImageAdjust;
        let fields: [(&str, Num, fn(&A) -> f64, fn(&mut A, f64)); 6] = [
            ("Brightness", Num::new(-100.0..=100.0, 0, " %").speed(0.5), |a| a.brightness, |a, v| a.brightness = v),
            ("Contrast", Num::new(-100.0..=100.0, 0, " %").speed(0.5), |a| a.contrast, |a, v| a.contrast = v),
            ("Gamma", Num::new(0.1..=10.0, 2, "").speed(0.01), |a| a.gamma, |a, v| a.gamma = v),
            ("Enhance amount", Num::new(0.0..=1000.0, 0, " %").speed(0.5), |a| a.enhance_amount, |a, v| a.enhance_amount = v),
            ("Enhance radius", Num::new(0.0..=100.0, 1, "").speed(0.1), |a| a.enhance_radius, |a, v| a.enhance_radius = v),
            ("Enhance denoise", Num::new(0.0..=100.0, 0, "").speed(0.5), |a| a.enhance_denoise, |a, v| a.enhance_denoise = v),
        ];
        for (label, num, get, set) in fields {
            g.number(
                ui,
                label,
                num,
                move |s| adjust_of(s).map(get),
                move |s, v| {
                    if let Some(a) = adjust_mut(s) {
                        set(a, v);
                    }
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use darkburn_core::kurbo::{Affine, Point, Rect};
    use darkburn_core::{ImageData, ImageShape};
    use std::sync::Arc;

    #[test]
    fn mixed_values_and_editing_several_shapes() {
        let mut d = Document::new();
        let a = d.add(Shape::rect(Rect::new(0.0, 0.0, 10.0, 10.0), 0.0, 0));
        let b = d.add(Shape::rect(Rect::new(0.0, 0.0, 20.0, 10.0), 0.0, 0));
        let c = d.add(Shape::polygon(Point::ZERO, 6, 5.0, 0));
        let shapes: Vec<Shape> = [a, b, c].iter().map(|id| d.get(*id).unwrap().clone()).collect();
        let width = |s: &Shape| if let ShapeKind::Rect { width, .. } = s.kind { Some(width) } else { None };
        assert_eq!(common(&shapes, width), Some((10.0, true)), "mixed widths");
        assert_eq!(common(&shapes, |s| Some(s.locked)), Some((false, false)));
        assert_eq!(common(&shapes, |s| text_of(s).map(|t| t.height)), None, "no text: no field");
        // Setting the width touches the rectangles only.
        assert!(set_each(&mut d, &[a, b, c], width, |s| {
            if let ShapeKind::Rect { width, .. } = &mut s.kind {
                *width = 30.0;
            }
        }));
        assert!((d.get(a).unwrap().bbox().width() - 30.0).abs() < 1e-9);
        assert!((d.get(b).unwrap().bbox().width() - 30.0).abs() < 1e-9);
        assert!(matches!(d.get(c).unwrap().kind, ShapeKind::Polygon { sides: 6, .. }));
        assert!(!set_each(&mut d, &[a, b], width, |_| {}), "no change, no step");
    }

    #[test]
    fn inward_corner_radius() {
        let mut d = Document::new();
        let a = d.add(Shape::rect(Rect::new(0.0, 0.0, 10.0, 10.0), 0.0, 0));
        set_each(&mut d, &[a], |s| Some(s.power_scale), |s| s.power_scale = 50.0);
        assert_eq!(d.get(a).unwrap().power_scale, 50.0);
        set_each(
            &mut d,
            &[a],
            |s| if let ShapeKind::Rect { radius, .. } = s.kind { Some(radius) } else { None },
            |s| {
                if let ShapeKind::Rect { radius, .. } = &mut s.kind {
                    *radius = -2.0;
                }
            },
        );
        let s = d.get(a).unwrap();
        // The bounding box stays; the area shrinks as the corners are cut inwards.
        assert!((s.bbox().width() - 10.0).abs() < 1e-6);
        assert!(!s.contains(Point::new(0.5, 0.5)));
        assert!(s.contains(Point::new(5.0, 5.0)));
    }

    #[test]
    fn image_dpi_readout() {
        let mut d = Document::new();
        let key = d.add_image(ImageData { format: "png".into(), bytes: Arc::from(&b"x"[..]), px_width: 254, px_height: 127 });
        let img = ImageShape { image: key, width: 25.4, height: 12.7, ..Default::default() };
        let mut s = Shape::new(ShapeKind::Image(img), Affine::IDENTITY, 0);
        let ((pw, ph), (dx, dy)) = image_resolution(&d, &s).unwrap();
        assert_eq!((pw, ph), (254, 127));
        assert!((dx - 254.0).abs() < 1e-9 && (dy - 254.0).abs() < 1e-9);
        s.transform = Affine::scale(2.0);
        let (_, (dx, _)) = image_resolution(&d, &s).unwrap();
        assert!((dx - 127.0).abs() < 1e-9, "twice as big, half the DPI");
    }
}
