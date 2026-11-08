//! Background blur stub. Adapted from SolidCraft crates/ui-egui/src/blur.rs (MIT OR Apache-2.0):
//! DarkBurn has no compositor blur, so containers draw plain fills.
use egui::{Color32, Frame, Painter, Rect, Shape};

pub fn enabled() -> bool {
    false
}

pub fn panel_fill(fill: Color32) -> Color32 {
    fill
}

pub fn decorate(_: &Painter, _: Frame, _: Rect, shape: Shape) -> Shape {
    shape
}
