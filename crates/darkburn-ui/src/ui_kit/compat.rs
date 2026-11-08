//! Small helpers the copied kit modules use, lifted from SolidCraft modules DarkBurn does not
//! copy whole (assistant, navigation, navigation_menu, preferences, history, keymap).
//! SolidCraft, MIT OR Apache-2.0.
use crate::ui_kit::BackingTooltipExt as _;
use egui::{FontId, Pos2, Rect, Response, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use solidcraft_theme::Tokens;

use super::Container;
use super::metrics::dimension;

pub const NAV_BORDER: f32 = 1.0;
pub const NAV_ROW_GAP: f32 = 9.0;
pub const NAV_POPUP_PADDING: i8 = 5;
pub const TREE_ROW_HEIGHT: f32 = 26.0;

pub fn navigation_popup_frame() -> egui::Frame {
    Container::menu_frame().inner_margin(egui::Margin::same(NAV_POPUP_PADDING))
}

/// A key chord as shown to the user (Ctrl becomes Cmd on macOS).
pub fn key_display(key: &str) -> String {
    let mac = cfg!(target_os = "macos");
    let both = key.starts_with("Ctrl+Cmd+");
    key.split('+')
        .map(|part| match (mac, part) {
            (true, "Ctrl") if !both => "Cmd",
            (true, "Alt") => "Option",
            _ => part,
        })
        .collect::<Vec<_>>()
        .join("+")
}

pub fn workspace_chip(ui: &mut Ui, label: &str) -> Response {
    let accent = solidcraft_theme::TabAccent::current(ui.ctx());
    badge(ui, label, accent.ink, accent.soft)
}

fn badge(ui: &mut Ui, label: &str, ink: egui::Color32, fill: egui::Color32) -> Response {
    let t = Tokens::get();
    let font = egui::FontId::proportional(t.font_caption);
    let width = ui.painter().layout_no_wrap(label.to_owned(), font.clone(), ink).size().x + t.space_12;
    let (rect, response) = ui.allocate_exact_size(vec2(width, t.form_close_size - t.space_4), egui::Sense::hover());
    ui.painter().rect_filled(rect, t.radius_xl, fill);
    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, label, font, ink);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, ui.is_enabled(), label));
    response
}

pub fn settings_scroll<R>(ui: &mut Ui, id: impl egui::AsIdSalt, add: impl FnOnce(&mut Ui) -> R) -> egui::scroll_area::ScrollAreaOutput<R> {
    let t = Tokens::get();
    let gutter = ui.stack().iter().find_map(|node| node.tags().get_downcast::<f32>(PAGE_SCROLL_GUTTER)).copied().unwrap_or(0.0);
    let width = dimension(ui.available_width());
    let height = if ui.available_height().is_finite() { dimension(ui.available_height()) } else { t.control_height * 16.0 };
    let bar = ui.spacing().scroll.floating_width;
    let extension = if gutter > 0.0 { gutter / 2.0 + bar / 2.0 } else { 0.0 };
    let host = Rect::from_min_size(ui.next_widget_position(), vec2(width + extension, height));
    ui.scope_builder(egui::UiBuilder::new().max_rect(host).layout(egui::Layout::top_down(egui::Align::Min)), |ui| {
        if gutter > 0.0 {
            let scroll = &mut ui.style_mut().spacing.scroll;
            scroll.floating = true;
            scroll.floating_allocated_width = 0.0;
            scroll.bar_outer_margin = 0.0;
        }
        let mut scroll = egui::ScrollArea::vertical().id_salt(id).max_height(height).auto_shrink([false, false]).show(ui, |ui| {
            if gutter > 0.0 {
                // The scrollbar host extends into the gutter. Its content must neither use
                // that extension nor grow the page/window when a child requests extra width.
                let bounds = Rect::from_min_size(ui.next_widget_position(), vec2(width, ui.available_height()));
                let mut content = ui.new_child(egui::UiBuilder::new().max_rect(bounds));
                let clip = content.clip_rect();
                content.set_clip_rect(clip.intersect(Rect::from_min_max(pos2(bounds.left(), clip.top()), pos2(bounds.right(), clip.bottom()))));
                let result = add(&mut content);
                ui.allocate_exact_size(vec2(width, content.min_size().y), Sense::hover());
                result
            } else {
                add(ui)
            }
        });
        // Consumers publish the page's content well, not the extended scrollbar host.
        if gutter > 0.0 {
            scroll.inner_rect.max.x = scroll.inner_rect.left() + width;
        }
        scroll
    })
    .inner
}

pub mod preferences {
    pub const PAGE_SCROLL_GUTTER: &str = "sc_page_scroll_gutter";
    pub use super::rounded_perimeter;
}
use preferences::PAGE_SCROLL_GUTTER;

/// A bounded settings row/card using the same selection treatment as picture cards.
pub fn settings_row_card<R>(ui: &mut Ui, selected: bool, height: f32, add: impl FnOnce(&mut Ui) -> R) -> egui::InnerResponse<R> {
    let t = Tokens::get();
    let (rect, response) = ui.allocate_exact_size(vec2(dimension(ui.available_width()), height), Sense::hover());
    let border = if selected { 2.0 } else { 1.0 };
    ui.painter().rect(
        rect,
        t.radius_lg,
        if selected { t.selected_bg } else { t.control_fill },
        Stroke::new(border, if selected { t.form_primary } else { t.dialog_border }),
        StrokeKind::Inside,
    );
    let inner = rect.shrink2(vec2(t.space_10, 0.0));
    let result = ui
        .scope_builder(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::left_to_right(egui::Align::Center)), |ui| {
            ui.set_clip_rect(ui.clip_rect().intersect(inner));
            ui.set_min_size(inner.size());
            add(ui)
        })
        .inner;
    egui::InnerResponse { inner: result, response }
}

/// The (i) icon after a label: shows `text` as a tooltip and tells assistive technology the same.
pub fn help_icon(ui: &mut Ui, text: &str) -> Response {
    let t = Tokens::get();
    let size = t.space_12 + 1.0;
    let (rect, response) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    let ink = if response.hovered() { t.form_ink } else { t.form_dim };
    let p = ui.painter();
    p.circle_stroke(rect.center(), size * 0.5 - 0.5, Stroke::new(1.0, ink));
    p.circle_filled(pos2(rect.center().x, rect.top() + size * 0.3), 0.9, ink);
    p.line_segment([pos2(rect.center().x, rect.top() + size * 0.46), pos2(rect.center().x, rect.bottom() - size * 0.24)], Stroke::new(1.2, ink));
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, ui.is_enabled(), text));
    solidcraft_ui_registry::publish_target("prefs:help_icon", rect);
    response.backing_on_hover_text(text)
}

/// Natural-width dim text, for inline suffixes and metadata without a full-width note row.
pub fn settings_inline_note(ui: &mut Ui, text: &str) {
    if super::settings_search::suppress_chrome(ui) {
        return;
    }
    let t = Tokens::get();
    let galley = if super::settings_page_active(ui) && ui.layout().main_wrap() {
        ui.painter().layout(text.to_owned(), FontId::proportional(t.font_body), t.form_dim, dimension(ui.max_rect().width()))
    } else {
        ui.painter().layout_no_wrap(text.to_owned(), FontId::proportional(t.font_body), t.form_dim)
    };
    let (r, response) = ui.allocate_exact_size(galley.size(), Sense::hover());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, ui.is_enabled(), text));
    ui.painter().galley(r.min, galley, t.form_dim);
}

/// List border around a caller-owned virtualised viewport. Tokens: dialog_rule, control_radius, space_10.
pub fn list_frame<R>(ui: &mut Ui, key: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    list_frame_filled(ui, key, egui::Color32::TRANSPARENT, 0.0, add)
}

pub fn list_frame_filled<R>(ui: &mut Ui, key: &str, fill: egui::Color32, vertical: f32, add: impl FnOnce(&mut Ui) -> R) -> R {
    let t = Tokens::get();
    let response = crate::ui_kit::Container::show_backing(
        ui,
        egui::Frame::NONE
            .fill(fill)
            .stroke(t.container_border(t.dialog_rule))
            .corner_radius(t.control_radius)
            .inner_margin(egui::Margin::symmetric(t.space_10 as i8, vertical as i8)),
        |ui| {
            // Frame already subtracts its border and padding from the content width.
            ui.set_min_width(ui.available_width().max(0.0));
            add(ui)
        },
    );
    solidcraft_ui_registry::publish_rect(&format!("dialog:list:{key}"), response.response.rect);
    response.inner
}

pub fn rounded_perimeter(r: Rect, radius: f32) -> Vec<Pos2> {
    let radius = radius.min(r.width() / 2.0).min(r.height() / 2.0).max(0.0);
    let corners = [
        (pos2(r.right() - radius, r.top() + radius), -std::f32::consts::FRAC_PI_2),
        (pos2(r.right() - radius, r.bottom() - radius), 0.0),
        (pos2(r.left() + radius, r.bottom() - radius), std::f32::consts::FRAC_PI_2),
        (pos2(r.left() + radius, r.top() + radius), std::f32::consts::PI),
    ];
    corners
        .into_iter()
        .flat_map(|(centre, start)| {
            (0..=8).map(move |step| {
                let angle = start + std::f32::consts::FRAC_PI_2 * step as f32 / 8.0;
                centre + vec2(angle.cos(), angle.sin()) * radius
            })
        })
        .collect()
}
