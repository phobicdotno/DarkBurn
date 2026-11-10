//! Accessible names (what a screen reader says) for controls built from plain egui widgets.
//!
//! The `ui_kit` fields name themselves after the row they sit in. A bare `egui::TextEdit`,
//! `DragValue` or `Slider` has no name, or only its unit ("mm"), so the place that adds it calls
//! one of these with what the visible label says.

use egui::{Response, Role, Ui, WidgetInfo};

/// Name `response` `label` for assistive technology. The widget keeps its role's value and actions.
pub fn name(ui: &Ui, response: &Response, role: Role, label: &str) {
    let label = label.trim();
    if !label.is_empty() {
        response.widget_info(|| WidgetInfo::labeled(role, ui.is_enabled(), label));
    }
}

/// Name a control after the `ui_kit::row` it is in (nothing happens outside a labelled row).
pub fn name_from_row(ui: &Ui, response: &Response, role: Role) {
    name(ui, response, role, darkburn_ui::ui_kit::row_label(ui));
}

/// A single-line text box with an accessible name.
pub fn text(ui: &Ui, response: &Response, label: &str) {
    name(ui, response, Role::TextInput, label);
}

/// A number field (`egui::DragValue`) with an accessible name.
pub fn number(ui: &Ui, response: &Response, label: &str) {
    name(ui, response, Role::SpinButton, label);
}

/// A slider with an accessible name.
pub fn slider(ui: &Ui, response: &Response, label: &str) {
    name(ui, response, Role::Slider, label);
}
