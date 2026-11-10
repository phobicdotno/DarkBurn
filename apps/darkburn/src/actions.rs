//! Edit, Arrange, Tools and View commands acting on the open project.

use darkburn_core::arrange::{self, Align, Anchor, Distribute, Order};
use darkburn_core::kurbo::{Rect, Vec2};
use darkburn_core::{PasteAt, select};

use crate::canvas::ViewCmd;
use crate::shell::DarkBurn;
use crate::tools::Tool;

/// Run command `id` if it is one of these; returns whether it was.
pub fn run(app: &mut DarkBurn, ctx: &egui::Context, id: &str) -> bool {
    let ids = app.editor.selected();
    let page = Rect::new(0.0, 0.0, app.prefs.page_width as f64, app.prefs.page_height as f64);
    let e = &mut app.editor;
    match id {
        // Tools
        "tools.select" => {
            app.pan_tool = false;
            app.tool = Tool::Select;
        }
        "tools.line" => app.set_tool(Tool::Line),
        "tools.rect" => app.set_tool(Tool::Rect),
        "tools.ellipse" => app.set_tool(Tool::Ellipse),
        "tools.polygon" => app.set_tool(Tool::Polygon),
        // Edit
        "edit.undo" => {
            e.undo();
        }
        "edit.redo" => {
            e.redo();
        }
        "edit.select_all" => {
            let all = select::all(&e.doc);
            e.selection.set(all);
        }
        "edit.invert_selection" => {
            let inv = select::inverted(&e.doc, &e.selection);
            e.selection.set(inv);
        }
        "edit.copy" => {
            e.clipboard.copy(&e.doc, &ids);
            app.clipboard_out = true;
        }
        "edit.cut" => {
            e.clipboard.copy(&e.doc, &ids);
            app.clipboard_out = true;
            e.edit("Cut", |d, _| arrange::delete(d, &ids));
        }
        "edit.duplicate" => {
            e.edit("Duplicate", |d, sel| {
                let new = arrange::duplicate(d, &ids, Vec2::ZERO);
                sel.set(new.iter().copied());
                !new.is_empty()
            });
        }
        "edit.paste" | "edit.paste_in_place" | "edit.paste_at_cursor" => {
            crate::files::pull_system_clipboard(app, ctx);
            let at = match id {
                "edit.paste_in_place" => PasteAt::InPlace,
                "edit.paste_at_cursor" => match app.context_point {
                    Some(p) => PasteAt::Centre(p),
                    None => PasteAt::Offset,
                },
                _ => PasteAt::Offset,
            };
            let e = &mut app.editor;
            let mut clip = std::mem::take(&mut e.clipboard);
            e.edit("Paste", |d, sel| {
                let new = clip.paste(d, at);
                sel.set(new.iter().copied());
                !new.is_empty()
            });
            e.clipboard = clip;
        }
        "edit.delete" => {
            e.edit("Delete", |d, _| arrange::delete(d, &ids));
        }
        "edit.select_open" => {
            let v = select::open_shapes(&e.doc);
            e.selection.set(v);
        }
        "edit.select_open_fill" => {
            let v = select::open_fill_shapes(&e.doc);
            e.selection.set(v);
        }
        "edit.select_layer" => {
            let layer = ids.last().and_then(|id| e.doc.get(*id)).map(|s| s.layer).unwrap_or(e.current_layer);
            let v = select::on_layer(&e.doc, layer);
            e.selection.set(v);
        }
        "edit.select_contained" => {
            let v = select::contained(&e.doc, &e.selection);
            e.selection.set(v);
        }
        // Arrange
        "arrange.group" => {
            e.edit("Group", |d, sel| match arrange::group(d, &ids) {
                Some(g) => {
                    sel.set([g]);
                    true
                }
                None => false,
            });
        }
        "arrange.ungroup" => {
            e.edit("Ungroup", |d, sel| {
                let had_group = ids.iter().any(|id| d.get(*id).is_some_and(|s| s.is_group()));
                let freed = arrange::ungroup(d, &ids);
                sel.set(freed);
                had_group
            });
        }
        "arrange.flip_h" => {
            e.edit("Flip", |d, _| arrange::flip_h(d, &ids));
        }
        "arrange.flip_v" => {
            e.edit("Flip", |d, _| arrange::flip_v(d, &ids));
        }
        "arrange.rotate_cw" => {
            e.edit("Rotate", |d, _| arrange::rotate(d, &ids, -90.0));
        }
        "arrange.rotate_ccw" => {
            e.edit("Rotate", |d, _| arrange::rotate(d, &ids, 90.0));
        }
        "arrange.same_width" => {
            e.edit("Same width", |d, _| arrange::same_size(d, &ids, true));
        }
        "arrange.same_height" => {
            e.edit("Same height", |d, _| arrange::same_size(d, &ids, false));
        }
        "arrange.forward" | "arrange.backward" | "arrange.front" | "arrange.back" => {
            let how = match id {
                "arrange.forward" => Order::Forward,
                "arrange.backward" => Order::Backward,
                "arrange.front" => Order::Front,
                _ => Order::Back,
            };
            e.edit("Draw order", |d, _| arrange::reorder(d, &ids, how));
        }
        "arrange.lock" => {
            e.edit("Lock", |d, sel| {
                let changed = arrange::set_locked(d, &ids, true);
                sel.clear();
                changed
            });
        }
        "arrange.unlock" => {
            // Locked shapes cannot be selected: unlock the selection, or everything if none.
            let targets: Vec<_> = if ids.is_empty() { e.doc.iter().filter(|s| s.locked).map(|s| s.id).collect() } else { ids.clone() };
            e.edit("Unlock", |d, sel| {
                let changed = arrange::set_locked(d, &targets, false);
                sel.set(targets.iter().copied());
                changed
            });
        }
        // View
        "view.wire_coarse" => (app.prefs.filled, app.prefs.smooth) = (false, false),
        "view.wire_smooth" => (app.prefs.filled, app.prefs.smooth) = (false, true),
        "view.filled_coarse" => (app.prefs.filled, app.prefs.smooth) = (true, false),
        "view.filled_smooth" => (app.prefs.filled, app.prefs.smooth) = (true, true),
        "view.toggle_filled" => app.prefs.filled = !app.prefs.filled,
        "view.frame_selection" => {
            let r = if ids.is_empty() { e.doc.bbox() } else { e.doc.bbox_of(&ids) };
            if let Some(r) = r {
                app.view_cmd = Some(ViewCmd::Frame(r));
            }
        }
        other => {
            if let Some(how) = align_of(other) {
                e.edit("Align", |d, _| arrange::align(d, &ids, how));
            } else if let Some(how) = distribute_of(other, app.padding) {
                e.edit("Distribute", |d, _| arrange::distribute(d, &ids, how));
            } else if let Some(anchor) = other.strip_prefix("arrange.page_").and_then(page_anchor) {
                e.edit("Move", |d, _| arrange::to_page(d, &ids, page, anchor));
            } else {
                return false;
            }
        }
    }
    true
}

fn align_of(id: &str) -> Option<Align> {
    Some(match id {
        "arrange.align_left" => Align::Left,
        "arrange.align_right" => Align::Right,
        "arrange.align_top" => Align::Top,
        "arrange.align_bottom" => Align::Bottom,
        "arrange.align_h_centres" => Align::VCentres,
        "arrange.align_v_centres" => Align::HCentres,
        "arrange.align_centres" => Align::Centres,
        _ => return None,
    })
}

fn distribute_of(id: &str, padding: f64) -> Option<Distribute> {
    Some(match id {
        "arrange.distribute_h" => Distribute::Horizontally,
        "arrange.distribute_v" => Distribute::Vertically,
        "arrange.distribute_h_centres" => Distribute::HCentres,
        "arrange.distribute_v_centres" => Distribute::VCentres,
        "arrange.distribute_h_spacing" => Distribute::HSpacing(padding),
        "arrange.distribute_v_spacing" => Distribute::VSpacing(padding),
        "arrange.move_h_together" => Distribute::HTogether,
        "arrange.move_v_together" => Distribute::VTogether,
        _ => return None,
    })
}

fn page_anchor(name: &str) -> Option<Anchor> {
    Some(match name {
        "centre" => Anchor::Centre,
        "upper_left" => Anchor::TopLeft,
        "top" => Anchor::Top,
        "upper_right" => Anchor::TopRight,
        "left" => Anchor::Left,
        "right" => Anchor::Right,
        "lower_left" => Anchor::BottomLeft,
        "bottom" => Anchor::Bottom,
        "lower_right" => Anchor::BottomRight,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prefs::Prefs;
    use darkburn_core::Shape;

    fn app_with_two() -> DarkBurn {
        let mut app = DarkBurn::new(Prefs::default());
        app.editor.edit("Add", |d, sel| {
            let a = d.add(Shape::rect(Rect::new(0.0, 0.0, 10.0, 10.0), 0.0, 0));
            let b = d.add(Shape::rect(Rect::new(50.0, 20.0, 60.0, 40.0), 0.0, 1));
            sel.set([a, b]);
            true
        });
        app
    }

    #[test]
    fn edit_commands_change_the_document_and_undo() {
        let ctx = egui::Context::default();
        let mut app = app_with_two();
        app.run(&ctx, "edit.copy");
        app.run(&ctx, "edit.paste");
        assert_eq!(app.editor.doc.len(), 4);
        assert_eq!(app.editor.selected().len(), 2, "pasted shapes are selected");
        app.run(&ctx, "edit.undo");
        assert_eq!(app.editor.doc.len(), 2);
        app.run(&ctx, "edit.redo");
        assert_eq!(app.editor.doc.len(), 4);
        app.run(&ctx, "edit.delete");
        assert_eq!(app.editor.doc.len(), 2);
        app.run(&ctx, "edit.select_all");
        app.run(&ctx, "arrange.group");
        assert_eq!(app.editor.doc.len(), 1);
        app.run(&ctx, "arrange.ungroup");
        assert_eq!(app.editor.doc.len(), 2);
        app.run(&ctx, "edit.invert_selection");
        assert!(app.editor.selected().is_empty());
    }

    #[test]
    fn arrange_commands_move_shapes() {
        let ctx = egui::Context::default();
        let mut app = app_with_two();
        app.run(&ctx, "arrange.align_left");
        let xs: Vec<f64> = app.editor.doc.iter().map(|s| s.bbox().x0).collect();
        assert!((xs[0] - xs[1]).abs() < 1e-9);
        app.run(&ctx, "arrange.page_upper_right");
        let bb = app.editor.doc.bbox().unwrap();
        assert!((bb.x1 - 400.0).abs() < 1e-9 && (bb.y1 - 400.0).abs() < 1e-9);
        app.run(&ctx, "arrange.rotate_cw");
        app.run(&ctx, "arrange.lock");
        assert!(app.editor.selected().is_empty());
        app.run(&ctx, "arrange.unlock");
        assert_eq!(app.editor.selected().len(), 2);
        app.run(&ctx, "view.toggle_filled");
        assert!(!app.prefs.filled, "filled is the default; the toggle turns it off");
        app.run(&ctx, "tools.rect");
        assert_eq!(app.tool, Tool::Rect);
    }
}

/// Every document command, run on a document with every kind of shape: when it changes the
/// document that is exactly one undo step, Undo gives back the document and selection as they
/// were, and Redo the changed document again.
#[cfg(test)]
mod undo_sweep {
    use std::sync::Arc;

    use darkburn_core::kurbo::{Affine, BezPath, Rect};
    use darkburn_core::{CutDirection, ImageData, ImageShape, Shape, ShapeId, ShapeKind, TextShape};
    use serde_json::json;

    use crate::prefs::Prefs;
    use crate::shell::DarkBurn;

    const COMMANDS: &[&str] = &[
        "edit.cut",
        "edit.duplicate",
        "edit.paste",
        "edit.paste_in_place",
        "edit.delete",
        "edit.convert_path",
        "edit.close_path",
        "edit.auto_join",
        "edit.optimize",
        "edit.delete_duplicates",
        "edit.delete_overlapping",
        "arrange.group",
        "arrange.ungroup",
        "arrange.auto_group",
        "arrange.flip_h",
        "arrange.flip_v",
        "arrange.rotate_cw",
        "arrange.rotate_ccw",
        "arrange.align_left",
        "arrange.align_right",
        "arrange.align_top",
        "arrange.align_bottom",
        "arrange.align_h_centres",
        "arrange.align_v_centres",
        "arrange.align_centres",
        "arrange.distribute_h",
        "arrange.distribute_v",
        "arrange.distribute_h_centres",
        "arrange.distribute_v_centres",
        "arrange.distribute_h_spacing",
        "arrange.distribute_v_spacing",
        "arrange.move_h_together",
        "arrange.move_v_together",
        "arrange.same_width",
        "arrange.same_height",
        "arrange.dock_left",
        "arrange.dock_right",
        "arrange.dock_up",
        "arrange.dock_down",
        "arrange.page_centre",
        "arrange.page_upper_left",
        "arrange.page_lower_right",
        "arrange.break_apart",
        "arrange.forward",
        "arrange.backward",
        "arrange.front",
        "arrange.back",
        "arrange.lock",
        "arrange.unlock",
        "arrange.array_flatten",
        "arrange.lock_inner",
        "shape.remove_rounded_corners",
    ];

    fn sample() -> (DarkBurn, Vec<ShapeId>) {
        let mut app = DarkBurn::new(Prefs::default());
        let mut ids = Vec::new();
        app.editor.edit("Build", |d, _| {
            let mut r = Shape::rect(Rect::new(10.0, 10.0, 40.0, 30.0), 3.0, 1);
            r.tabs = vec![[0.0, -10.0]];
            r.start_point = Some([15.0, 0.0]);
            r.direction = CutDirection::Clockwise;
            r.name = "plate".into();
            r.power_scale = 60.0;
            r.priority = 2;
            ids.push(d.add(r));
            ids.push(d.add(Shape::ellipse(Rect::new(60.0, 15.0, 90.0, 45.0), 2)));
            let mut open = BezPath::new();
            open.move_to((100.0, 10.0));
            open.line_to((130.0, 10.0));
            open.line_to((130.0, 40.0));
            ids.push(d.add(Shape::path(open, 3)));
            let mut two = BezPath::from_svg("M0 0 L10 0 L10 10 Z M20 0 L30 0 L30 10 Z").unwrap();
            two.apply_affine(Affine::translate((150.0, 60.0)));
            ids.push(d.add(Shape::path(two, 3)));
            ids.push(d.add(Shape::polygon((40.0, 80.0).into(), 6, 12.0, 4)));
            let key = d.add_image(ImageData { format: "png".into(), bytes: Arc::from(&b"\x89PNG not really"[..]), px_width: 4, px_height: 2 });
            let img = ImageShape { image: key, width: 30.0, height: 15.0, ..Default::default() };
            ids.push(d.add(Shape::new(ShapeKind::Image(img), Affine::translate((90.0, 90.0)), 5)));
            let text = TextShape { text: "Hi".into(), backup: Some(BezPath::from_svg("M0 0 L8 0 L8 6 L0 6 Z").unwrap()), ..Default::default() };
            ids.push(d.add(Shape::new(ShapeKind::Text(text), Affine::translate((20.0, 120.0)), 6)));
            let kids = vec![Shape::rect(Rect::new(0.0, 0.0, 8.0, 8.0), 0.0, 1), Shape::ellipse(Rect::new(12.0, 0.0, 20.0, 8.0), 7)];
            ids.push(d.add(Shape::new(ShapeKind::Group { children: kids }, Affine::translate((150.0, 100.0)), 1)));
            d.notes = "notes".into();
            true
        });
        app.editor.clipboard.copy(&app.editor.doc, &ids[..2]);
        app.editor.mark_saved();
        (app, ids)
    }

    #[test]
    fn every_document_command_is_one_undo_step_that_undo_and_redo_restore() {
        let mut problems = Vec::new();
        for id in COMMANDS {
            assert!(crate::commands::lookup(id).is_some(), "{id} is not a command");
            assert!(!crate::commands::hardware(id), "{id} is a hardware command");
            for pick in ["all", "two", "one", "none"] {
                let (mut app, ids) = sample();
                let picked: Vec<ShapeId> = match pick {
                    "all" => ids.clone(),
                    "two" => vec![ids[3], ids[0]],
                    "one" => vec![ids[7]],
                    _ => Vec::new(),
                };
                app.editor.selection.set(picked);
                let (before, selection, steps) = (app.editor.doc.clone(), app.editor.selection.clone(), app.editor.history.len());
                let _ = crate::api::execute_headless(&mut app, id, &json!({}));
                let after = app.editor.doc.clone();
                let made = app.editor.history.len() - steps;
                if after == before {
                    if made != 0 {
                        problems.push(format!("{id} ({pick}): {made} undo step(s) for no change"));
                    }
                    continue;
                }
                if made != 1 {
                    problems.push(format!("{id} ({pick}): changed the document with {made} undo steps"));
                    continue;
                }
                app.editor.undo();
                if app.editor.doc != before {
                    problems.push(format!("{id} ({pick}): undo does not give the document back"));
                }
                if app.editor.selection != selection {
                    problems.push(format!("{id} ({pick}): undo does not give the selection back"));
                }
                app.editor.redo();
                if app.editor.doc != after {
                    problems.push(format!("{id} ({pick}): redo does not give the changed document back"));
                }
            }
        }
        assert!(problems.is_empty(), "{}", problems.join("\n"));
    }
}
