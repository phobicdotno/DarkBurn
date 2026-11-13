//! Wheel input for the kit's scrolled strips. Camera input never reads the wheel here: it goes
//! through the app's navigation handler.

use egui::Ui;

/// Take a predominantly horizontal wheel gesture for a hovered scroll viewport.
/// Leave vertical input for its enclosing page, and leave horizontal input
/// unconsumed when this viewport is already at the requested end.
pub fn take_horizontal_wheel(ui: &Ui, offset: f32, max_offset: f32) -> f32 {
    ui.input_mut(|input| {
        let delta = input.smooth_scroll_delta;
        if delta.x == 0.0 || delta.x.abs() <= delta.y.abs() {
            return 0.0;
        }
        if (offset - delta.x).clamp(0.0, max_offset.max(0.0)) != offset {
            input.smooth_scroll_delta.x = 0.0;
        }
        delta.x
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A strip whose content is narrower than its viewport (negative max offset) or not measured yet (NaN)
    /// must not panic on a sideways wheel.
    #[test]
    fn sideways_wheel_with_a_negative_or_nan_max_offset_does_not_panic() {
        let ctx = egui::Context::default();
        for max_offset in [-10.0_f32, f32::NAN] {
            let input =
                egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0))), ..Default::default() };
            let mut out = ctx.run_ui(input, |ui| {
                ui.input_mut(|i| i.smooth_scroll_delta = egui::vec2(5.0, 0.0));
                assert_eq!(take_horizontal_wheel(ui, 0.0, max_offset), 5.0);
                // already at the start of a strip that cannot scroll: the wheel is left for others
                assert_eq!(ui.input(|i| i.smooth_scroll_delta.x), 5.0);
            });
            out.textures_delta.clear();
        }
    }
}
