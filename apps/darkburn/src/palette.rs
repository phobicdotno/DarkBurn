//! The cut palette: 30 colour layers and 2 tool layers along the bottom of the window.
//! Click puts the selected shapes on a layer (and makes it current), Shift+click selects the
//! layer's shapes, right-click flashes them on the canvas.

use darkburn_core::arrange;
use darkburn_core::layer::{LAYER_COUNT, is_tool_layer, layer_label};
use darkburn_ui::ui_kit::{BackingTooltipExt as _, Container, Edge, Surface};
use egui::{Align2, Color32, FontId, Rect, Sense, Stroke, Ui, pos2, vec2};
use solidcraft_theme::Tokens;

use crate::shell::{DarkBurn, PALETTE_HEIGHT};

/// A layer's colour for painting.
pub fn layer_colour(i: usize) -> Color32 {
    let [r, g, b] = darkburn_core::layer::layer_colour(i);
    Color32::from_rgb(r, g, b)
}

/// Black or white, whichever reads better on `fill`.
pub fn ink_on(fill: Color32) -> Color32 {
    let l = 0.299 * fill.r() as f32 + 0.587 * fill.g() as f32 + 0.114 * fill.b() as f32;
    if l > 140.0 { Color32::BLACK } else { Color32::WHITE }
}

/// What a click on a palette button does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaletteClick {
    /// Put the selection on the layer and make it current.
    Assign,
    /// Select the layer's shapes.
    SelectShapes,
    /// Flash the layer's shapes.
    Flash,
}

/// Do `click` on layer `layer`.
pub fn apply(app: &mut DarkBurn, layer: usize, click: PaletteClick, now: f64) {
    let e = &mut app.editor;
    match click {
        PaletteClick::Assign => {
            let ids = e.selected();
            if !ids.is_empty() {
                e.edit("Change layer", |d, _| arrange::set_layer(d, &ids, layer));
            }
            e.current_layer = layer;
        }
        PaletteClick::SelectShapes => {
            let ids: Vec<_> = e.doc.shapes_on_layer(layer).into_iter().filter(|id| e.doc.get(*id).is_some_and(|s| e.doc.is_selectable(s))).collect();
            e.selection.set(ids);
            e.current_layer = layer;
        }
        PaletteClick::Flash => app.flash_layer = Some((layer, now)),
    }
}

pub fn show(ui: &mut Ui, app: &mut DarkBurn) {
    let t = Tokens::get();
    let host = Container::new(ui.ctx(), Surface::Chrome);
    let panel = host.edge("db_palette", Edge::Bottom).exact_size(PALETTE_HEIGHT).show_separator_line(false);
    let frame = host.edge_frame(t.toolbar);
    let mut action = None;
    host.show_edge(ui, panel, frame, |ui| {
        let r = ui.max_rect();
        // Dragging the strip moves the palette [TB-25].
        crate::toolbar_dock::grip(ui, "palette", "strip", r, crate::toolbar_dock::Dock::Home);
        let gap = 3.0;
        let size = ((r.width() - 16.0 - gap * (LAYER_COUNT as f32 - 1.0)) / LAYER_COUNT as f32).clamp(14.0, 22.0);
        let mut x = r.left() + 8.0;
        let used = app.editor.doc.used_layers();
        for i in 0..LAYER_COUNT {
            let br = Rect::from_center_size(pos2(x + size / 2.0, r.center().y), vec2(size, size));
            x += size + gap;
            if let Some(click) = swatch(ui, app, i, br, &used) {
                action = Some((i, click));
            }
        }
    });
    if let Some((layer, click)) = action {
        let now = ui.input(|i| i.time);
        apply(app, layer, click, now);
    }
}

/// The button of layer `i` at `br` (`used`: the layers with shapes on them), and what a click on
/// it asks for.
pub fn swatch(ui: &mut Ui, app: &mut DarkBurn, i: usize, br: Rect, used: &[usize]) -> Option<PaletteClick> {
    let t = Tokens::get();
    let current = app.editor.current_layer;
    let tool = is_tool_layer(i);
    let label = layer_label(i);
    let fill = if tool { t.control_fill } else { layer_colour(i) };
    let resp = ui.interact(br, ui.scope_id().with(("layer", i)), Sense::click());
    let p = ui.painter();
    let on = i == current;
    let border = if on {
        Stroke::new(2.0, t.accent)
    } else if resp.hovered() {
        Stroke::new(1.5, t.text)
    } else {
        Stroke::new(1.0, t.border)
    };
    p.rect(br, t.radius_sm, fill, border, egui::StrokeKind::Inside);
    if on {
        // A notch under the current layer.
        p.hline(br.x_range(), br.bottom() + 2.5, Stroke::new(2.0, t.accent));
    }
    if used.contains(&i) {
        p.circle_filled(pos2(br.right() - 3.5, br.top() + 3.5), 1.8, ink_on(fill));
    }
    if tool || app.prefs.palette_labels {
        let ink = if tool { t.text_dim } else { ink_on(fill) };
        p.text(br.center(), Align2::CENTER_CENTER, label.clone(), FontId::proportional(t.font_micro), ink);
    }
    solidcraft_ui_registry::publish_target(&format!("palette:{label}"), br);
    let name = &app.editor.doc.layer(i).name;
    let tip = format!("Layer {label} ({name}). Click: put the selection on this layer. Shift+click: select its shapes. Right-click: flash them.");
    if resp.hovered() {
        app.hint = Some(tip.clone());
    }
    let resp = resp.backing_on_hover_text(tip);
    if resp.secondary_clicked() {
        Some(PaletteClick::Flash)
    } else if resp.clicked() {
        let shift = ui.input(|inp| inp.modifiers.shift);
        Some(if shift { PaletteClick::SelectShapes } else { PaletteClick::Assign })
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use darkburn_core::Shape;
    use darkburn_core::kurbo::Rect as KRect;

    fn app_with_shapes() -> DarkBurn {
        let mut app = DarkBurn::new(crate::prefs::Prefs::default());
        app.editor.edit("Add", |d, sel| {
            let a = d.add(Shape::rect(KRect::new(0.0, 0.0, 10.0, 10.0), 0.0, 0));
            d.add(Shape::rect(KRect::new(20.0, 0.0, 30.0, 10.0), 0.0, 3));
            sel.set([a]);
            true
        });
        app
    }

    #[test]
    fn click_assigns_the_selection_and_makes_the_layer_current() {
        let mut app = app_with_shapes();
        apply(&mut app, 5, PaletteClick::Assign, 0.0);
        assert_eq!(app.editor.current_layer, 5);
        assert_eq!(app.editor.doc.used_layers(), vec![3, 5]);
        assert!(app.editor.undo());
        assert_eq!(app.editor.doc.used_layers(), vec![0, 3], "one undo step");
        app.editor.selection.clear();
        let steps = app.editor.history.len();
        apply(&mut app, 7, PaletteClick::Assign, 0.0);
        assert_eq!(app.editor.current_layer, 7);
        assert_eq!(app.editor.history.len(), steps, "nothing selected: no undo step");
    }

    #[test]
    fn shift_click_selects_and_right_click_flashes() {
        let mut app = app_with_shapes();
        apply(&mut app, 3, PaletteClick::SelectShapes, 0.0);
        assert_eq!(app.editor.selection.len(), 1);
        assert_eq!(app.editor.doc.get(app.editor.selection.ids()[0]).unwrap().layer, 3);
        apply(&mut app, 3, PaletteClick::Flash, 2.5);
        assert_eq!(app.flash_layer, Some((3, 2.5)));
        assert_eq!(ink_on(Color32::WHITE), Color32::BLACK);
        assert_eq!(ink_on(Color32::BLACK), Color32::WHITE);
    }
}
