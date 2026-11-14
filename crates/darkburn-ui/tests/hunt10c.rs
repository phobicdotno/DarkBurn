//! Bug hunt 10c (UI): kit layout fed a viewport that is not a number.

use darkburn_ui::frame::Side;
use darkburn_ui::ui_kit::Panel;
use egui::{Pos2, Rect, pos2};

/// A panel placed in a viewport whose bounds are NaN (not laid out yet) keeps its anchor instead of panicking.
#[test]
fn panel_anchor_in_a_nan_viewport_does_not_panic() {
    let ctx = egui::Context::default();
    let anchor = pos2(120.0, 80.0);
    let nan = Rect::from_min_max(pos2(f32::NAN, f32::NAN), pos2(f32::NAN, f32::NAN));
    let panel = Panel::new("Hunt", egui::Id::unique("hunt10c_panel"), anchor, nan, Side::Right, egui::Frame::NONE);
    assert_eq!(panel.bounded_anchor(&ctx), anchor);
    // a normal viewport still holds the anchor inside it
    let view = Rect::from_min_max(Pos2::ZERO, pos2(800.0, 600.0));
    let panel = Panel::new("Hunt", egui::Id::unique("hunt10c_panel"), pos2(5000.0, -50.0), view, Side::Right, egui::Frame::NONE);
    let a = panel.bounded_anchor(&ctx);
    assert!(view.contains(a), "{a:?}");
}
