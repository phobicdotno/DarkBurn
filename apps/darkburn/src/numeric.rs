//! Number fields that take equations and units (`10in/4`, `5' + 2"`), and undo steps that
//! coalesce a drag on a field into one.

use std::ops::RangeInclusive;

use darkburn_core::{Document, Editor, Selection, expr};
use darkburn_ui::ui_kit;
use egui::{Response, Ui, vec2};
use solidcraft_theme::Tokens;

/// How a number field shows and reads its value.
#[derive(Clone)]
pub struct Num {
    pub range: RangeInclusive<f64>,
    /// Change per point dragged.
    pub speed: f64,
    pub decimals: usize,
    /// Shown after the number (" mm", " %").
    pub suffix: String,
    /// Millimetres in the field's unit, for typed lengths (1 for mm, 25.4 for inches). Fields that
    /// are not lengths use 1: a typed length then counts in millimetres.
    pub unit_mm: f64,
    /// Several values differ: the field shows blank until something is typed.
    pub mixed: bool,
}

impl Num {
    pub fn new(range: RangeInclusive<f64>, decimals: usize, suffix: &str) -> Num {
        let span = (range.end() - range.start()).abs();
        let speed = if decimals == 0 { 0.1 } else { (span / 2000.0).clamp(0.001, 1.0) };
        Num { range, speed, decimals, suffix: suffix.to_string(), unit_mm: 1.0, mixed: false }
    }

    pub fn speed(mut self, speed: f64) -> Num {
        self.speed = speed;
        self
    }

    pub fn unit(mut self, unit_mm: f64) -> Num {
        self.unit_mm = unit_mm;
        self
    }

    pub fn mixed(mut self, mixed: bool) -> Num {
        self.mixed = mixed;
        self
    }
}

/// What a number field did this frame.
pub struct Edited {
    pub response: Response,
    /// The value was set (typed and accepted, or dragged).
    pub changed: bool,
}

impl Edited {
    /// Whether this change continues a drag already under way (so it joins the last undo step).
    pub fn continues(&self) -> bool {
        self.response.dragged() && !self.response.drag_started()
    }
}

/// Parse `text` typed into a field showing `suffix`: the suffix may be typed or left off.
pub fn parse(text: &str, suffix: &str, unit_mm: f64) -> Option<f64> {
    let trimmed = text.trim();
    let suffix = suffix.trim();
    let body = if !suffix.is_empty() && trimmed.len() > suffix.len() && trimmed.to_ascii_lowercase().ends_with(&suffix.to_ascii_lowercase()) {
        // Only drop the suffix when what is left still reads (so "2 mm" in a mm field is 2,
        // and "1in" in a mm field keeps its unit).
        let cut = &trimmed[..trimmed.len() - suffix.len()];
        if expr::eval_in(cut, unit_mm).is_some() { cut } else { trimmed }
    } else {
        trimmed
    };
    expr::eval_in(body, unit_mm)
}

/// A number field `width` points wide (the available width when None). Typed text is worked out
/// with [`expr`] when the field loses focus; dragging changes the value as it goes.
pub fn field_sized(ui: &mut Ui, value: &mut f64, num: &Num, width: Option<f32>) -> Edited {
    let t = Tokens::get();
    let before = *value;
    let parsed = std::cell::Cell::new(false);
    let suffix = num.suffix.clone();
    let unit_mm = num.unit_mm;
    let decimals = num.decimals;
    let mixed = num.mixed;
    let drag = egui::DragValue::new(value)
        .range(num.range.clone())
        .clamp_existing_to_range(false)
        .speed(num.speed)
        .update_while_editing(false)
        .custom_parser(|text| {
            let v = parse(text, &suffix, unit_mm);
            parsed.set(v.is_some());
            v
        })
        .custom_formatter(move |v, _| if mixed { String::new() } else { format!("{v:.decimals$}") })
        .suffix(if mixed { String::new() } else { num.suffix.clone() });
    let w = width.unwrap_or_else(|| ui.available_width().max(0.0));
    let response = ui.add_sized(vec2(w, t.control_height), drag);
    t.input_bevel(ui, &response);
    // The field is named after its row ("Speed"), not after its unit ("mm/s").
    crate::a11y::name_from_row(ui, &response, egui::Role::SpinButton);
    let changed = (*value != before) || (num.mixed && parsed.get());
    Edited { response, changed }
}

/// [`field_sized`] filling the available width.
pub fn field(ui: &mut Ui, value: &mut f64, num: &Num) -> Edited {
    field_sized(ui, value, num, None)
}

/// A labelled row holding a number field.
pub fn row(ui: &mut Ui, label: &str, value: &mut f64, num: &Num) -> Edited {
    ui_kit::row(ui, label, |ui| field(ui, value, num))
}

/// Change the document as one undo step labelled `label`, or, when `merge` (a drag on the same
/// field going on), as part of the step this field made last frame.
pub fn commit(
    editor: &mut Editor,
    ctx: &egui::Context,
    key: egui::Id,
    label: &str,
    merge: bool,
    f: impl FnOnce(&mut Document, &mut Selection) -> bool,
) -> bool {
    let stamp = egui::Id::unique("darkburn_coalesce");
    let last: Option<(egui::Id, u64)> = ctx.data(|d| d.get_temp(stamp));
    let changed = if merge && last == Some((key, editor.revision())) {
        let changed = f(&mut editor.doc, &mut editor.selection);
        if changed {
            editor.touch();
        }
        changed
    } else {
        editor.edit(label, f)
    };
    ctx.data_mut(|d| d.insert_temp(stamp, (key, editor.revision())));
    changed
}

/// A whole number from a field's value.
pub fn whole(v: f64) -> i64 {
    if v.is_finite() { v.round() as i64 } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use darkburn_core::Shape;
    use darkburn_core::kurbo::Rect;

    #[test]
    fn typed_text_with_or_without_the_suffix() {
        assert_eq!(parse("12", " mm", 1.0), Some(12.0));
        assert_eq!(parse("12 mm", " mm", 1.0), Some(12.0));
        assert_eq!(parse("1in", " mm", 1.0), Some(25.4));
        assert_eq!(parse("10in/4", " mm", 1.0), Some(63.5));
        assert_eq!(parse("50 %", " %", 1.0), Some(50.0));
        assert_eq!(parse("100 mm/s", " mm/s", 1.0), Some(100.0));
        assert_eq!(parse("25.4mm", " in", 25.4), Some(1.0));
        assert_eq!(parse("2,5", "", 1.0), Some(2.5));
        assert_eq!(parse("nonsense", " mm", 1.0), None);
        assert_eq!(whole(2.6), 3);
        assert_eq!(whole(f64::NAN), 0);
    }

    #[test]
    fn a_drag_is_one_undo_step() {
        let ctx = egui::Context::default();
        let mut e = Editor::new();
        let key = egui::Id::unique("speed");
        for (i, merge) in [false, true, true, true].into_iter().enumerate() {
            commit(&mut e, &ctx, key, "Speed", merge, |d, _| {
                d.layer_mut(0).main_mut().speed = 10.0 + i as f64;
                true
            });
        }
        assert_eq!(e.history.len(), 1);
        assert_eq!(e.doc.layer(0).main().speed, 13.0);
        // Another field, or anything in between, starts a new step.
        commit(&mut e, &ctx, egui::Id::unique("power"), "Power", true, |d, _| {
            d.layer_mut(0).main_mut().max_power = 50.0;
            true
        });
        assert_eq!(e.history.len(), 2);
        e.edit("Add", |d, _| {
            d.add(Shape::rect(Rect::new(0.0, 0.0, 1.0, 1.0), 0.0, 0));
            true
        });
        commit(&mut e, &ctx, egui::Id::unique("power"), "Power", true, |d, _| {
            d.layer_mut(0).main_mut().max_power = 60.0;
            true
        });
        assert_eq!(e.history.len(), 4);
        e.undo();
        assert_eq!(e.doc.layer(0).main().max_power, 50.0);
        e.undo();
        e.undo();
        e.undo();
        assert_eq!(e.doc.layer(0).main().speed, 100.0, "the whole drag undoes at once");
    }
}
