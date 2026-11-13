//! Bug hunt 10 (UI, second pass): tables and other kit layout fed odd input.

use darkburn_ui::theme::Tokens;
use darkburn_ui::ui_kit::{Column, table_custom};
use egui::{Event, PointerButton, Pos2, Rect, vec2};

/// Two tables share one key for their dragged column widths. Dragging a divider in the
/// narrower table keeps the widths the wider one stored for its later columns.
#[test]
fn dragging_a_column_in_a_narrow_table_keeps_a_wider_tables_widths() {
    let ctx = egui::Context::default();
    let key = egui::Id::unique("hunt10b_shared_columns");
    ctx.data_mut(|d| d.insert_persisted(key, vec![None, None, None, Some(120.0_f32)]));
    let divider = std::cell::Cell::new(Pos2::ZERO);
    let frame = |events: Vec<Event>| {
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0))), events, ..Default::default() };
        let mut out = ctx.run_ui(input, |ui| {
            let columns = [Column::new("a", 1.0), Column::new("b", 1.0)];
            table_custom(ui, "hunt10b", &columns, "", |ui, table| {
                table.headers_resizable(ui, key, |ui, index| {
                    if index == 0 {
                        let cell = ui.max_rect();
                        divider.set(Pos2::new(cell.right() + Tokens::get().space_8 * 0.5, cell.center().y));
                    }
                    false
                });
            });
        });
        out.textures_delta.clear();
    };
    let button = |pos, pressed| Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
    frame(Vec::new());
    let start = divider.get();
    frame(vec![Event::PointerMoved(start)]);
    frame(vec![button(start, true)]);
    for step in 1..=6 {
        frame(vec![Event::PointerMoved(start + vec2(5.0 * step as f32, 0.0))]);
    }
    let end = start + vec2(30.0, 0.0);
    frame(vec![button(end, false)]);
    frame(Vec::new());
    let stored: Vec<Option<f32>> = ctx.data_mut(|d| d.get_persisted(key)).unwrap_or_default();
    assert!(stored.first().copied().flatten().is_some(), "the test drags the first divider: {stored:?}");
    assert_eq!(stored.get(3).copied().flatten(), Some(120.0), "the wider table's fourth column: {stored:?}");
}

/// Huge column weights (a hostile design) still lay the table out at finite widths.
#[test]
fn huge_flexible_column_weights_keep_finite_widths() {
    let ctx = egui::Context::default();
    let cells = std::cell::RefCell::new(Vec::new());
    let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0))), ..Default::default() };
    let mut out = ctx.run_ui(input, |ui| {
        let columns = [Column::new("a", f32::MAX).flex(), Column::new("b", f32::MAX).flex()];
        table_custom(ui, "hunt10b_huge", &columns, "", |ui, table| {
            table.headers(ui, |ui, _| {
                cells.borrow_mut().push(ui.max_rect());
                false
            });
        });
    });
    out.textures_delta.clear();
    let cells = cells.into_inner();
    assert_eq!(cells.len(), 2);
    assert!(cells.iter().all(|cell| cell.is_finite()), "{cells:?}");
}
