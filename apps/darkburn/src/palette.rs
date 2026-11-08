//! The cut palette: 30 colour layers and 2 tool layers along the bottom of the window.

use darkburn_ui::ui_kit::{BackingTooltipExt as _, Container, Edge, Surface};
use egui::{Align2, Color32, FontId, Rect, Sense, Stroke, Ui, pos2, vec2};
use solidcraft_theme::Tokens;

use crate::shell::{DarkBurn, PALETTE_HEIGHT};

/// A layer's colour for painting.
pub fn layer_colour(i: usize) -> Color32 {
    let [r, g, b] = darkburn_core::layer::layer_colour(i);
    Color32::from_rgb(r, g, b)
}

pub fn show(ui: &mut Ui, _app: &mut DarkBurn) {
    cut_palette(ui);
}

/// The 30 colour layers and 2 tool layers along the bottom. Layers arrive with drawing.
fn cut_palette(ui: &mut Ui) {
    let t = Tokens::get();
    let host = Container::new(ui.ctx(), Surface::Chrome);
    let panel = host.edge("db_palette", Edge::Bottom).exact_size(PALETTE_HEIGHT).show_separator_line(false);
    let frame = host.edge_frame(t.toolbar);
    host.show_edge(ui, panel, frame, |ui| {
        let r = ui.max_rect();
        let size = 20.0;
        let mut x = r.left() + 8.0;
        for i in 0..32 {
            let br = Rect::from_center_size(pos2(x + size / 2.0, r.center().y), vec2(size, size));
            x += size + 3.0;
            let (fill, label) = if i < 30 { (layer_colour(i), format!("{i:02}")) } else { (t.control_fill, format!("T{}", i - 29)) };
            let p = ui.painter();
            p.rect(br, t.radius_sm, fill.gamma_multiply(0.55), Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
            if i >= 30 {
                p.text(br.center(), Align2::CENTER_CENTER, label.clone(), FontId::proportional(t.font_micro), t.text_dim);
            }
            ui.interact(br, ui.scope_id().with(("layer", i)), Sense::hover()).backing_on_hover_text(format!("Layer {label} (arrives with drawing)"));
        }
    });
}
