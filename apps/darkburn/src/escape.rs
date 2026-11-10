//! Who gets the Escape key.
//!
//! One press does one thing. An open window or dialog comes first: Escape closes it, and the
//! canvas and its tools leave the key alone. A window that is not built with `ui_kit::Window`
//! (which handles Escape itself) asks [`closes`] each frame.

use egui::{Context, Id, Key, LayerId, Modifiers, Order};

/// The status toast is drawn above the windows but is not one: it never takes the key.
pub const TOAST: &str = "shape_status_toast";

fn window_layers(ctx: &Context) -> Vec<LayerId> {
    let toast = Id::unique(TOAST);
    ctx.memory(|m| {
        m.areas().visible_layer_ids().into_iter().filter(|l| matches!(l.order, Order::Middle | Order::Foreground) && l.id != toast).collect()
    })
}

/// A dialog, floating window, menu or drop-down list is open.
pub fn window_open(ctx: &Context) -> bool {
    !window_layers(ctx).is_empty()
}

/// Escape was pressed and belongs to the window with this id: nothing else (another dialog, a
/// menu, a drop-down list) is open above or beside it. The key is used up, so nothing later in
/// the frame acts on it too.
pub fn closes(ctx: &Context, window: Id) -> bool {
    window_layers(ctx).iter().all(|l| l.id == window) && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(ctx: &Context, events: Vec<egui::Event>, mut add: impl FnMut(&Context)) {
        let input =
            egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))), events, ..Default::default() };
        let mut out = ctx.run_ui(input, |ui| add(ui.ctx()));
        out.textures_delta.clear();
    }

    fn escape() -> egui::Event {
        egui::Event::Key { key: Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }
    }

    #[test]
    fn escape_goes_to_the_only_open_window_and_is_used_up() {
        let ctx = Context::default();
        let a = Id::unique("escape_test_a");
        let show = |ctx: &Context, id: Id, title: &str| {
            egui::Window::new(title).id(id).show(ctx, |ui| {
                ui.label("x");
            });
        };
        assert!(!window_open(&ctx));
        frame(&ctx, vec![], |ctx| show(ctx, a, "A"));
        assert!(window_open(&ctx));
        // No key: nothing closes.
        let mut closed = false;
        frame(&ctx, vec![], |ctx| {
            show(ctx, a, "A");
            closed = closes(ctx, a);
        });
        assert!(!closed);
        // One press closes it once: a second asker in the same frame gets nothing.
        let mut got = (false, false);
        frame(&ctx, vec![escape()], |ctx| {
            show(ctx, a, "A");
            got = (closes(ctx, a), closes(ctx, a));
        });
        assert_eq!(got, (true, false));
    }

    #[test]
    fn a_window_under_another_one_keeps_out_of_it() {
        let ctx = Context::default();
        let (a, b) = (Id::unique("escape_test_under"), Id::unique("escape_test_over"));
        let show = |ctx: &Context| {
            for (id, title) in [(a, "Under"), (b, "Over")] {
                egui::Window::new(title).id(id).show(ctx, |ui| {
                    ui.label("x");
                });
            }
        };
        frame(&ctx, vec![], show);
        let mut closed = true;
        frame(&ctx, vec![escape()], |ctx| {
            show(ctx);
            closed = closes(ctx, a);
        });
        assert!(!closed, "another window is open: this one does not take Escape");
    }

    #[test]
    fn the_status_toast_is_not_a_window() {
        let ctx = Context::default();
        let toast = |ctx: &Context| {
            egui::Area::new(Id::unique(TOAST)).order(Order::Foreground).show(ctx, |ui| {
                ui.label("Done");
            });
        };
        frame(&ctx, vec![], toast);
        frame(&ctx, vec![], toast);
        assert!(!window_open(&ctx));
    }
}
