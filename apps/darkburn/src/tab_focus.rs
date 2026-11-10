//! Where Tab goes while a window is open.
//!
//! With no window open Tab belongs to the canvas (`shell::keep_tab_for_the_canvas`). With one
//! open, egui walks the controls in the order they are drawn: the application bar, the ribbon
//! and the panels first, the window last. A dialog's fields were then dozens of presses away,
//! and Enter or Space on the way pressed a button behind the dialog.
//!
//! The walk is kept inside the window on top instead. egui has a means for that, made for its
//! modal windows: controls in the layers below a "modal layer" are left out of the walk. Held
//! all the time it would also take the keyboard from a panel field that is clicked while a
//! window is open, so it is held for one frame only: a Tab press is put off by a frame, and
//! that frame walks with the layers below left out.

use egui::{Context, Event, FocusDirection, Id, Key, LayerId, Order};

/// The window, dialog or floating panel drawn on top, if any is open, and the layers below it.
fn top_window(ctx: &Context) -> Option<(LayerId, Vec<LayerId>)> {
    ctx.memory(top_window_in)
}

fn top_window_in(m: &egui::Memory) -> Option<(LayerId, Vec<LayerId>)> {
    let toast = Id::unique(crate::escape::TOAST);
    let visible = m.areas().visible_layer_ids();
    // Back to front: the last one is on top.
    let layers: Vec<LayerId> = m.layer_ids().collect();
    let at = layers.iter().rposition(|l| l.order == Order::Middle && l.id != toast && visible.contains(l))?;
    Some((layers[at], layers[..at].to_vec()))
}

/// Tab (true) or Shift+Tab (false) was pressed this frame.
fn tab_pressed(ctx: &Context) -> Option<bool> {
    ctx.input(|i| {
        i.events.iter().find_map(|e| match e {
            Event::Key { key: Key::Tab, pressed: true, modifiers, .. } if !modifiers.any() => Some(true),
            Event::Key { key: Key::Tab, pressed: true, modifiers, .. } if modifiers.shift_only() => Some(false),
            _ => None,
        })
    })
}

/// Once a frame, before anything is drawn. A field in a panel behind the window that was
/// clicked into keeps its own Tab order: the walk is only held in the window while the keyboard
/// is in it or nowhere.
pub fn keep_tab_in_the_top_window(ctx: &Context) {
    let pending = Id::unique("db_tab_walk_put_off");
    if let Some(forward) = ctx.data_mut(|d| d.remove_temp::<bool>(pending)) {
        // The press of the frame before: the layers below the window are left out now. They
        // stay out for one frame more, in which a walk past the last control of the window
        // comes round to its first.
        ctx.memory_mut(|m| {
            m.move_focus(if forward { FocusDirection::Next } else { FocusDirection::Previous });
            if let Some((top, _)) = top_window_in(m) {
                m.set_modal_layer(top);
            }
        });
        ctx.request_repaint();
        return;
    }
    let Some(forward) = tab_pressed(ctx) else { return };
    let Some((top, below)) = top_window(ctx) else { return };
    let focused = ctx.memory(|m| m.focused()).and_then(|id| ctx.read_response(id)).map(|r| r.layer_id);
    if focused.is_some_and(|layer| below.contains(&layer) || layer.order == Order::Background) {
        return;
    }
    ctx.memory_mut(|m| {
        m.move_focus(FocusDirection::None);
        m.set_modal_layer(top);
    });
    ctx.data_mut(|d| d.insert_temp(pending, forward));
    ctx.request_repaint();
}
