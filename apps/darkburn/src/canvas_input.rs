//! Pointer and keys on the canvas: selecting, the transform handles, drawing, snapping, the
//! context menu and auto-pan, plus the overlays they draw (handles, selection box, previews,
//! snap marker, readouts). The geometry itself lives in `tools.rs` and `darkburn_core::snap`.
//!
//! The state between frames lives in egui's temporary memory, so the window needs no extra
//! fields for it.

use darkburn_core::kurbo::{self, Affine, Point};
use darkburn_core::snap::{self, Snap, SnapKind, SnapOptions};
use darkburn_core::{Shape, ShapeId, select};
use darkburn_ui::ui_kit::{Menu, MenuItem};
use egui::{Align2, Color32, CursorIcon, FontId, Key, Modifiers, PointerButton, Pos2, Rect, Response, Stroke, Ui, pos2, vec2};

use crate::canvas::ViewCmd;
use crate::render::{self, Xf};
use crate::shell::DarkBurn;
use crate::tools::{self, DragKind, Handle, HandleMode, Mods, Pen, Tool};

/// How close (screen points) a click must be to an outline to pick it.
pub const CLICK_TOLERANCE_PX: f32 = 5.0;
/// How far (screen points) the pointer jumps to a shape feature; Alt multiplies it.
pub const SNAP_PX: f32 = 8.0;
pub const SNAP_WIDE: f32 = 3.0;
/// Handle size and how far outside the selection box they sit (screen points).
const HANDLE: f32 = 7.0;
const HANDLE_GAP: f32 = 7.0;
/// Auto-pan starts this close to the canvas edge (points) and pans up to this fast (points/s).
const EDGE: f32 = 24.0;
const PAN_SPEED: f32 = 900.0;
/// How long a layer flashes (seconds).
pub const FLASH_SECONDS: f64 = 1.5;

/// What the pointer is doing.
#[derive(Clone, Debug, Default)]
enum Gesture {
    #[default]
    None,
    /// Moving, scaling, rotating or shearing the selection.
    Transform { kind: DragKind, bbox: kurbo::Rect, start: Point, originals: Vec<(ShapeId, Affine)>, current: Affine },
    /// A selection box, from `start` (document) to the pointer.
    Box { start: Point, now: Point },
    /// A rectangle, ellipse or polygon being dragged out.
    Shape { start: Point, now: Point },
    /// Dragging the tangent handle of the line tool's last node.
    PenHandle,
}

/// The canvas tools' state between frames.
#[derive(Clone, Debug, Default)]
struct State {
    tool: Tool,
    mode: HandleMode,
    pen: Pen,
    gesture: Gesture,
    /// The pointer (document mm, snapped) and what it snapped to.
    pointer: Option<Point>,
    snapped: Option<Snap>,
    /// The hint for the status bar while the pointer is over the canvas.
    hint: Option<String>,
    /// A drag or nudge was refused by this lock (move, size, rotate, shear).
    locked: Option<usize>,
}

fn state_id() -> egui::Id {
    egui::Id::unique("darkburn_canvas_tools")
}

/// The status bar hint the canvas left last frame (the shell shows it while the pointer is on
/// the canvas).
pub fn hover_hint(ctx: &egui::Context) -> Option<String> {
    ctx.data(|d| d.get_temp::<State>(state_id())).and_then(|s| s.hint)
}

fn mods(m: Modifiers) -> Mods {
    Mods { shift: m.shift, ctrl: m.command, alt: m.alt }
}

/// The snap settings at this zoom: the finest grid on screen, and the snap distance.
fn snap_options(xf: &Xf, m: Mods) -> SnapOptions {
    let (major, minor) = crate::view::grid_steps(xf.zoom as f32);
    let o = crate::options::get();
    let px = if m.alt { o.object_snap_px * SNAP_WIDE } else { o.object_snap_px };
    let grid = if o.grid_snap_mm > 0.0 { o.grid_snap_mm } else { minor.unwrap_or(major) };
    SnapOptions { grid: o.snap_grid.then_some(grid as f64), objects: o.snap_objects, radius: xf.mm(px) }
}

/// `p` snapped (Ctrl shows the unsnapped position).
fn snap_at(app: &DarkBurn, xf: &Xf, p: Point, m: Mods, exclude: &[ShapeId]) -> (Point, Option<Snap>) {
    if m.ctrl {
        return (p, None);
    }
    match snap::snap(&app.editor.doc, p, &snap_options(xf, m), exclude) {
        Some(s) => (s.point, Some(s)),
        None => (p, None),
    }
}

/// The selection's bounding box (document mm).
fn selection_box(app: &DarkBurn) -> Option<kurbo::Rect> {
    app.editor.doc.bbox_of(&app.editor.selected())
}

/// Screen positions of the handles around the selection box.
fn handle_points(xf: &Xf, bbox: kurbo::Rect) -> Vec<(Handle, Pos2)> {
    let r = xf.rect(bbox);
    let r = r.expand(HANDLE_GAP);
    Handle::ALL
        .iter()
        .map(|h| {
            let x = match h.x {
                -1 => r.left(),
                1 => r.right(),
                _ => r.center().x,
            };
            // Handle y counts up; the screen counts down.
            let y = match h.y {
                -1 => r.bottom(),
                1 => r.top(),
                _ => r.center().y,
            };
            (*h, pos2(x, y))
        })
        .collect()
}

fn handle_at(xf: &Xf, bbox: kurbo::Rect, s: Pos2) -> Option<Handle> {
    handle_points(xf, bbox).into_iter().find(|(_, p)| (*p - s).length() <= HANDLE + 2.0).map(|(h, _)| h)
}

/// Handle the canvas input for one frame. `panning` is true while the view is being dragged.
pub fn interact(ui: &Ui, app: &mut DarkBurn, response: &Response, canvas: Rect, xf: &Xf, panning: bool) {
    let ctx = ui.ctx().clone();
    let mut st = ctx.data_mut(|d| d.get_temp::<State>(state_id())).unwrap_or_default();
    if st.tool != app.tool {
        cancel(app, &mut st);
        st.pen = Pen::default();
        st.tool = app.tool;
    }
    let m = mods(ui.input(|i| i.modifiers));
    let hover = response.hover_pos().filter(|p| canvas.contains(*p));
    let raw = hover.map(|p| xf.to_doc(p));
    let selected = app.editor.selected();
    if selected.is_empty() {
        st.mode = HandleMode::Scale;
    }
    st.snapped = None;
    st.pointer = None;
    let tol = xf.mm(crate::options::get().click_tolerance_px);
    let mut run: Option<&'static str> = None;

    if !panning && !app.pan_tool {
        match app.tool {
            Tool::Select => select_tool(ui, app, &mut st, response, xf, m, raw, tol),
            Tool::Line => line_tool(ui, app, &mut st, response, xf, m, raw, tol),
            Tool::Rect | Tool::Ellipse | Tool::Polygon => shape_tool(ui, app, &mut st, response, xf, m, raw),
            Tool::Text => crate::text_ui::interact(ui, app, response, xf, raw),
            _ => crate::node_tool::interact(ui, app, response, xf, m, raw),
        }
    }

    if app.tool == Tool::Select && !panning && response.double_clicked_by(PointerButton::Primary) {
        crate::text_ui::on_double_click(app, raw, tol);
    }

    // Keys, when no text field has the keyboard.
    if ctx.memory(|mem| mem.focused().is_none()) && !egui::Popup::is_any_open(&ctx) {
        keys(&ctx, app, &mut st, &mut run);
    }

    // Double middle click frames the design (or the page when there is none).
    if response.double_clicked_by(PointerButton::Middle) {
        app.view_cmd = Some(match app.editor.doc.bbox() {
            Some(r) => ViewCmd::Frame(r),
            None => ViewCmd::ZoomPage,
        });
    }

    // Auto-pan while dragging near the edge.
    if !matches!(st.gesture, Gesture::None)
        && let Some(p) = ui.input(|i| i.pointer.latest_pos())
    {
        let push = |pos: f32, lo: f32, hi: f32| {
            if pos < lo + EDGE {
                ((lo + EDGE - pos) / EDGE).min(2.0)
            } else if pos > hi - EDGE {
                -((pos - (hi - EDGE)) / EDGE).min(2.0)
            } else {
                0.0
            }
        };
        let v = vec2(push(p.x, canvas.left(), canvas.right()), push(p.y, canvas.top(), canvas.bottom()));
        if v != egui::Vec2::ZERO {
            let dt = ui.input(|i| i.stable_dt).min(0.05);
            app.prefs.view.pan(v * PAN_SPEED * dt);
            ctx.request_repaint();
        }
    }

    context_menu(app, response, raw, &mut run);

    // Cursor and hint.
    st.hint = None;
    if hover.is_some() && !panning && !app.pan_tool {
        if ui.input(|i| i.pointer.any_pressed()) && !matches!(st.gesture, Gesture::Transform { .. }) {
            st.locked = None;
        }
        st.hint = Some(match st.locked {
            Some(k) => format!("The {} lock is on (status bar)", ["move", "size", "rotate", "shear"][k]),
            None => app.tool.hint().to_string(),
        });
        let icon = match app.tool {
            Tool::Select => match (&st.gesture, hover) {
                (Gesture::Transform { kind: DragKind::Move, .. }, _) => CursorIcon::Grabbing,
                (Gesture::Transform { kind: DragKind::Rotate, .. }, _) => CursorIcon::Alias,
                (Gesture::Transform { kind, .. }, _) => resize_cursor(*kind),
                (_, Some(s)) => match selection_box(app).and_then(|b| handle_at(xf, b, s)) {
                    Some(h) => resize_cursor(DragKind::for_handle(h, st.mode)),
                    None if raw.and_then(|p| select::hit(&app.editor.doc, p, tol)).is_some_and(|id| app.editor.selection.contains(id)) => {
                        CursorIcon::Move
                    }
                    None => CursorIcon::Default,
                },
                _ => CursorIcon::Default,
            },
            Tool::Text => CursorIcon::Text,
            _ => CursorIcon::Crosshair,
        };
        ctx.set_cursor_icon(icon);
    }

    // The layer flash ends on its own.
    if let Some((_, t0)) = app.flash_layer {
        if ctx.input(|i| i.time) - t0 > FLASH_SECONDS {
            app.flash_layer = None;
        } else {
            ctx.request_repaint();
        }
    }

    ctx.data_mut(|d| d.insert_temp(state_id(), st));
    if let Some(id) = run {
        app.run(&ctx, id);
    }
}

fn resize_cursor(kind: DragKind) -> CursorIcon {
    match kind {
        DragKind::Scale(h) | DragKind::Shear(h) => match (h.x, h.y) {
            (0, _) => CursorIcon::ResizeVertical,
            (_, 0) => CursorIcon::ResizeHorizontal,
            (x, y) if x == y => CursorIcon::ResizeNeSw,
            _ => CursorIcon::ResizeNwSe,
        },
        DragKind::Rotate => CursorIcon::Alias,
        DragKind::Move => CursorIcon::Move,
    }
}

/// Undo a transform drag in progress and forget the gesture.
fn cancel(app: &mut DarkBurn, st: &mut State) {
    if let Gesture::Transform { .. } = st.gesture {
        app.editor.drop_checkpoint();
    }
    st.gesture = Gesture::None;
}

fn press_origin(ui: &Ui, xf: &Xf) -> Option<Point> {
    ui.input(|i| i.pointer.press_origin()).map(|p| xf.to_doc(p))
}

fn select_tool(ui: &Ui, app: &mut DarkBurn, st: &mut State, response: &Response, xf: &Xf, m: Mods, raw: Option<Point>, tol: f64) {
    let selected = app.editor.selected();
    let bbox = selection_box(app);
    if response.drag_started_by(PointerButton::Primary)
        && let Some(origin) = press_origin(ui, xf)
    {
        let origin_screen = xf.to_screen(origin);
        let handle = bbox.and_then(|b| handle_at(xf, b, origin_screen));
        let hit = select::hit(&app.editor.doc, origin, tol);
        let kind = if let Some(h) = handle {
            Some(DragKind::for_handle(h, st.mode))
        } else if let Some(id) = hit {
            if m.ctrl && m.shift {
                app.editor.selection.remove(id);
                None
            } else {
                if !app.editor.selection.contains(id) {
                    if m.shift {
                        app.editor.selection.add(id);
                    } else {
                        app.editor.selection.set([id]);
                        st.mode = HandleMode::Scale;
                    }
                }
                Some(DragKind::Move)
            }
        } else {
            st.gesture = Gesture::Box { start: origin, now: origin };
            None
        };
        if let Some(kind) = kind {
            if app.locks[kind.lock()] {
                st.locked = Some(kind.lock());
            } else {
                let ids = app.editor.selected();
                let originals: Vec<(ShapeId, Affine)> = ids.iter().filter_map(|id| app.editor.doc.get(*id).map(|s| (*id, s.transform))).collect();
                let bbox = app.editor.doc.bbox_of(&ids).unwrap_or_default();
                let start = match kind {
                    DragKind::Scale(h) => h.on(bbox),
                    DragKind::Move => snap_at(app, xf, origin, m, &[]).0,
                    _ => origin,
                };
                let label = match kind {
                    DragKind::Move => "Move",
                    DragKind::Scale(_) => "Resize",
                    DragKind::Rotate => "Rotate",
                    DragKind::Shear(_) => "Shear",
                };
                app.editor.checkpoint(label);
                st.gesture = Gesture::Transform { kind, bbox, start, originals, current: Affine::IDENTITY };
            }
        }
    }

    match &mut st.gesture {
        Gesture::Transform { kind, bbox, start, originals, current } => {
            if let Some(p) = raw {
                let exclude: Vec<ShapeId> = originals.iter().map(|(id, _)| *id).collect();
                let (now, snapped) = match kind {
                    DragKind::Rotate => (p, None),
                    _ => snap_at(app, xf, p, m, &exclude),
                };
                st.snapped = snapped;
                st.pointer = Some(now);
                let a = tools::drag_transform(*kind, *bbox, *start, now, m);
                if a != *current {
                    *current = a;
                    for (id, t) in originals.iter() {
                        if let Some(s) = app.editor.doc.get_mut(*id) {
                            s.transform = a * *t;
                        }
                    }
                    // What follows the document by its revision (a dialog's preview, the
                    // auto-save) must see each step of the drag, not only its start.
                    app.editor.touch();
                }
            }
            if !ui.input(|i| i.pointer.primary_down()) {
                let unchanged = tools::is_identity(*current);
                st.gesture = Gesture::None;
                if unchanged {
                    app.editor.drop_checkpoint();
                }
            }
        }
        Gesture::Box { start, now } => {
            if let Some(p) = raw {
                *now = p;
            }
            if !ui.input(|i| i.pointer.primary_down()) {
                let (a, b) = (*start, *now);
                st.gesture = Gesture::None;
                let crossing = b.x < a.x;
                let ids = select::in_box(&app.editor.doc, kurbo::Rect::from_points(a, b), crossing);
                let sel = &mut app.editor.selection;
                if m.ctrl && m.shift {
                    ids.iter().for_each(|id| sel.remove(*id));
                } else if m.shift {
                    sel.extend(ids);
                } else {
                    sel.set(ids);
                    st.mode = HandleMode::Scale;
                }
            }
        }
        _ => {}
    }

    if response.clicked_by(PointerButton::Primary)
        && let Some(p) = raw
    {
        let on_handle = bbox.is_some_and(|b| handle_at(xf, b, xf.to_screen(p)).is_some());
        let hit = select::hit(&app.editor.doc, p, tol);
        let sel = &mut app.editor.selection;
        match hit {
            Some(id) if m.ctrl && m.shift => sel.remove(id),
            Some(id) if m.shift => sel.toggle(id),
            Some(id) if selected.contains(&id) => {
                st.mode = if st.mode == HandleMode::Scale { HandleMode::Rotate } else { HandleMode::Scale };
            }
            Some(id) => {
                sel.set([id]);
                st.mode = HandleMode::Scale;
            }
            None if on_handle => {}
            None if m.shift => {}
            None => sel.clear(),
        }
    }
}

fn line_tool(ui: &Ui, app: &mut DarkBurn, st: &mut State, response: &Response, xf: &Xf, m: Mods, raw: Option<Point>, tol: f64) {
    let place = |app: &DarkBurn, pen: &Pen, p: Point| -> (Point, Option<Snap>) {
        let (q, s) = snap_at(app, xf, p, m, &[]);
        match (m.shift, pen.last()) {
            (true, Some(last)) => (snap::snap_angle(last, p, 45.0), None),
            _ => (q, s),
        }
    };
    if let Some(p) = raw {
        let (q, s) = place(app, &st.pen, p);
        st.pointer = Some(q);
        st.snapped = s;
    }
    if response.double_clicked_by(PointerButton::Primary) {
        finish_pen(app, st, false);
        return;
    }
    if response.clicked_by(PointerButton::Primary)
        && let Some(p) = raw
    {
        let (q, _) = place(app, &st.pen, p);
        if st.pen.is_on_start(q, tol.max(xf.mm(SNAP_PX))) {
            finish_pen(app, st, true);
        } else {
            st.pen.add(q);
        }
    }
    if response.drag_started_by(PointerButton::Primary)
        && let Some(origin) = press_origin(ui, xf)
    {
        let (q, _) = place(app, &st.pen, origin);
        st.pen.add(q);
        st.gesture = Gesture::PenHandle;
    }
    if let Gesture::PenHandle = st.gesture {
        if let Some(p) = raw {
            st.pen.drag_handle(p);
        }
        if !ui.input(|i| i.pointer.primary_down()) {
            st.gesture = Gesture::None;
        }
    }
}

fn finish_pen(app: &mut DarkBurn, st: &mut State, closed: bool) {
    st.gesture = Gesture::None;
    if let Some(path) = st.pen.finish(closed) {
        let layer = app.editor.current_layer;
        // A line that starts or ends on the end of an open line on the same layer extends it.
        let closed = darkburn_core::shape::path_is_closed(&path);
        app.editor.edit("Draw line", |d, sel| {
            let (id, _) = crate::shape_nodes::add_line_in(d, path, layer, !closed, 0.05);
            sel.set([id]);
            true
        });
    }
}

/// The shape a rectangle, ellipse or polygon drag makes.
fn dragged_shape(tool: Tool, a: Point, b: Point, m: Mods, layer: usize) -> Option<Shape> {
    match tool {
        Tool::Rect | Tool::Ellipse => {
            let r = tools::rect_from_drag(a, b, m.ctrl, m.shift);
            if r.width() < 1e-3 || r.height() < 1e-3 {
                return None;
            }
            Some(if tool == Tool::Rect { Shape::rect(r, 0.0, layer) } else { Shape::ellipse(r, layer) })
        }
        Tool::Polygon => (a.distance(b) > 1e-3).then(|| tools::polygon_from_drag(a, b, tools::POLYGON_SIDES, m.shift, layer)),
        _ => None,
    }
}

fn shape_tool(ui: &Ui, app: &mut DarkBurn, st: &mut State, response: &Response, xf: &Xf, m: Mods, raw: Option<Point>) {
    if let Some(p) = raw {
        let (q, s) = snap_at(app, xf, p, m, &[]);
        st.pointer = Some(q);
        st.snapped = s;
    }
    if response.drag_started_by(PointerButton::Primary)
        && let Some(origin) = press_origin(ui, xf)
    {
        let start = snap_at(app, xf, origin, m, &[]).0;
        st.gesture = Gesture::Shape { start, now: start };
    }
    if let Gesture::Shape { start, now } = &mut st.gesture {
        if let Some(p) = st.pointer {
            *now = p;
        }
        if !ui.input(|i| i.pointer.primary_down()) {
            let (a, b) = (*start, *now);
            st.gesture = Gesture::None;
            let layer = app.editor.current_layer;
            if let Some(shape) = dragged_shape(app.tool, a, b, m, layer) {
                let label = match app.tool {
                    Tool::Rect => "Draw rectangle",
                    Tool::Ellipse => "Draw ellipse",
                    _ => "Draw polygon",
                };
                app.editor.edit(label, |d, sel| {
                    let id = d.add(shape);
                    sel.set([id]);
                    true
                });
            }
        }
    }
}

fn keys(ctx: &egui::Context, app: &mut DarkBurn, st: &mut State, run: &mut Option<&'static str>) {
    let pressed = |m: Modifiers, k: Key| ctx.input_mut(|i| i.consume_key(m, k));
    // A dialog or window is open: Escape is its (it closes the dialog), not the canvas's.
    let window_open = crate::escape::window_open(ctx);
    if !window_open && pressed(Modifiers::NONE, Key::Escape) {
        if !matches!(st.gesture, Gesture::None) {
            cancel(app, st);
        } else if !st.pen.is_empty() {
            finish_pen(app, st, false);
        } else {
            app.editor.selection.clear();
            st.mode = HandleMode::Scale;
            app.set_tool(Tool::Select);
            st.tool = Tool::Select;
        }
        return;
    }
    // So are the other keys: an arrow, Tab or a letter pressed over a dialog must not move, pick
    // or flip the shapes behind it.
    if window_open {
        return;
    }
    if !st.pen.is_empty() && pressed(Modifiers::NONE, Key::Enter) {
        finish_pen(app, st, false);
        return;
    }
    if !matches!(st.gesture, Gesture::None) {
        return;
    }
    // Shift+Tab first: a plain pattern also matches with Shift held.
    let back = pressed(Modifiers::SHIFT, Key::Tab);
    let forward = !back && pressed(Modifiers::NONE, Key::Tab);
    if (back || forward)
        && let Some(id) = select::cycle(&app.editor.doc, &app.editor.selection, forward)
    {
        app.editor.selection.set([id]);
        st.mode = HandleMode::Scale;
    }
    if app.tool == Tool::Nodes {
        // The node tool uses the plain letter keys itself.
    } else if pressed(Modifiers::NONE, Key::H) {
        *run = Some("arrange.flip_h");
    } else if pressed(Modifiers::NONE, Key::V) {
        *run = Some("arrange.flip_v");
    }
    // `<` and `>` turn the selection 90 degrees (the shell runs those); `,` and `.` turn it by a
    // small angle: 1 degree, Ctrl 5 degrees, Alt 10 degrees.
    if !app.editor.selected().is_empty() {
        for (m, deg) in [(Modifiers::ALT, 10.0), (Modifiers::COMMAND, 5.0), (Modifiers::NONE, 1.0)] {
            if pressed(m, Key::Comma) {
                let _ = crate::api::execute_headless(app, "arrange.rotate_by", &serde_json::json!({ "degrees": deg }));
            }
            if pressed(m, Key::Period) {
                let _ = crate::api::execute_headless(app, "arrange.rotate_by", &serde_json::json!({ "degrees": -deg }));
            }
        }
    }
    // Arrow keys nudge, with key repeat.
    let nudges: Vec<(Key, Modifiers)> = ctx.input(|i| {
        i.events
            .iter()
            .filter_map(|e| match e {
                egui::Event::Key { key, pressed: true, modifiers, .. }
                    if matches!(key, Key::ArrowLeft | Key::ArrowRight | Key::ArrowUp | Key::ArrowDown) =>
                {
                    Some((*key, *modifiers))
                }
                _ => None,
            })
            .collect()
    });
    let ids = app.editor.selected();
    for (key, m) in nudges {
        if ids.is_empty() {
            break;
        }
        if app.locks[0] {
            st.locked = Some(0);
            break;
        }
        let step = tools::nudge_step(m.shift, m.command);
        let d = match key {
            Key::ArrowLeft => kurbo::Vec2::new(-step, 0.0),
            Key::ArrowRight => kurbo::Vec2::new(step, 0.0),
            Key::ArrowUp => kurbo::Vec2::new(0.0, step),
            _ => kurbo::Vec2::new(0.0, -step),
        };
        app.editor.edit("Nudge", |doc, _| darkburn_core::arrange::translate(doc, &ids, d));
    }
}

fn context_menu(app: &mut DarkBurn, response: &Response, raw: Option<Point>, run: &mut Option<&'static str>) {
    if response.secondary_clicked() {
        app.context_point = raw;
    }
    let items: [(&str, &str, &str); 10] = [
        ("edit.cut", "Cut", "Ctrl+X"),
        ("edit.copy", "Copy", "Ctrl+C"),
        ("edit.paste", "Paste", "Ctrl+V"),
        ("edit.paste_at_cursor", "Paste at Cursor", ""),
        ("edit.duplicate", "Duplicate", "Ctrl+D"),
        ("edit.select_all", "Select All", "Ctrl+A"),
        ("arrange.group", "Group", "Ctrl+G"),
        ("arrange.ungroup", "Ungroup", "Ctrl+U"),
        ("window.preview", "Preview", "Alt+P"),
        ("canvas.properties", "Properties", ""),
    ];
    let enabled: Vec<bool> = items
        .iter()
        .map(|(id, _, _)| match *id {
            "canvas.properties" => true,
            "edit.paste_at_cursor" => app.context_point.is_some(),
            id => app.can_run(id),
        })
        .collect();
    let mut picked: Option<&'static str> = None;
    Menu::context(response, |ui| {
        ui.set_min_width(220.0);
        for (i, (id, label, key)) in items.iter().enumerate() {
            if matches!(i, 3 | 5 | 8) {
                Menu::separator(ui);
            }
            let cmd = crate::commands::find(id);
            let icon = cmd.map(|c| c.icon).unwrap_or("");
            // The key the command has now (the user may have changed or removed it).
            let shortcut = cmd.map_or_else(|| key.to_string(), |c| crate::keymap::key_in(&app.prefs.hotkeys, c.id, crate::keymap::default_key(c)));
            let mut item = MenuItem::new(id, label, icon);
            item.shortcut = &shortcut;
            item.enabled = enabled[i];
            if item.show(ui).clicked() && item.enabled {
                picked = Some(id);
                ui.close();
            }
        }
    });
    match picked {
        Some("canvas.properties") => {
            let panel = app.prefs.panel("shape_properties");
            if panel.visible {
                app.prefs.active_tab.insert(panel.area, "shape_properties".into());
            } else {
                *run = Some("panel.shape_properties");
            }
        }
        Some(id) => *run = Some(id),
        None => {}
    }
}

/// Draw what the tools show over the document: handles, the selection box, previews and the
/// snap marker.
pub fn paint_overlays(ui: &Ui, app: &DarkBurn, xf: &Xf, painter: &egui::Painter, accent: Color32, dark: bool) {
    let Some(st) = ui.ctx().data(|d| d.get_temp::<State>(state_id())) else { return };
    let ink = render::ink(app.editor.current_layer, dark);
    let text = if dark { Color32::from_gray(230) } else { Color32::from_gray(25) };
    let font = FontId::proportional(11.0);
    let draw_path = |path: &kurbo::BezPath, colour: Color32| {
        for (mut pts, closed) in render::screen_polylines(path, xf, true) {
            if closed && !pts.is_empty() {
                pts.push(pts[0]);
            }
            painter.add(egui::Shape::line(pts, Stroke::new(1.5, colour)));
        }
    };

    // Selection box and handles.
    if app.tool == Tool::Select
        && !matches!(st.gesture, Gesture::Box { .. })
        && let Some(b) = selection_box(app)
    {
        let r = xf.rect(b);
        let outline = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
        let mut dashes = Vec::new();
        crate::render::dashed_line_in(&outline, painter.clip_rect(), Stroke::new(1.0, accent.gamma_multiply(0.7)), 4.0, 4.0, &mut dashes);
        painter.extend(dashes);
        let mode = st.mode;
        for (h, p) in handle_points(xf, b) {
            let locked = app.locks[DragKind::for_handle(h, mode).lock()];
            let fill = if locked { Color32::GRAY } else { accent };
            match DragKind::for_handle(h, mode) {
                DragKind::Scale(_) => {
                    painter.rect(Rect::from_center_size(p, vec2(HANDLE, HANDLE)), 1.0, fill, Stroke::new(1.0, text), egui::StrokeKind::Middle);
                }
                DragKind::Rotate => {
                    painter.circle(p, HANDLE * 0.6, fill, Stroke::new(1.0, text));
                }
                _ => {
                    // Shear: a small diamond.
                    let d = HANDLE * 0.65;
                    let pts = vec![p + vec2(0.0, -d), p + vec2(d, 0.0), p + vec2(0.0, d), p + vec2(-d, 0.0)];
                    painter.add(egui::Shape::convex_polygon(pts, fill, Stroke::new(1.0, text)));
                }
            }
        }
        if let Gesture::Transform { kind: DragKind::Rotate, current, .. } = &st.gesture {
            let at = r.center() + vec2(12.0, -12.0);
            painter.text(at, Align2::LEFT_BOTTOM, format!("{:.1}°", tools::rotation_degrees(*current)), font.clone(), text);
        }
    }

    match &st.gesture {
        Gesture::Box { start, now } => {
            let r = Rect::from_two_pos(xf.to_screen(*start), xf.to_screen(*now));
            if now.x >= start.x {
                let blue = Color32::from_rgb(70, 130, 240);
                painter.rect(r, 0.0, blue.gamma_multiply(0.12), Stroke::new(1.0, blue), egui::StrokeKind::Middle);
            } else {
                let green = Color32::from_rgb(60, 190, 90);
                painter.rect_filled(r, 0.0, green.gamma_multiply(0.10));
                let pts = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
                painter.extend(egui::Shape::dashed_line(&pts, Stroke::new(1.0, green), 5.0, 4.0));
            }
        }
        Gesture::Shape { start, now } => {
            let m = mods(ui.input(|i| i.modifiers));
            if let Some(s) = dragged_shape(app.tool, *start, *now, m, app.editor.current_layer) {
                draw_path(&s.outline(), ink);
                let b = s.bbox();
                let at = xf.to_screen(*now) + vec2(14.0, 14.0);
                painter.text(at, Align2::LEFT_TOP, format!("{:.2} × {:.2} mm", b.width(), b.height()), font.clone(), text);
            }
        }
        _ => {}
    }

    // The line tool's path and rubber band.
    if app.tool == Tool::Line && !st.pen.is_empty() {
        let to = st.pointer.unwrap_or_else(|| st.pen.last().unwrap_or_default());
        let path = if matches!(st.gesture, Gesture::PenHandle) { st.pen.path(false) } else { st.pen.preview(to) };
        draw_path(&path, ink);
        for (i, n) in st.pen.nodes.iter().enumerate() {
            let p = xf.to_screen(n.at);
            let closing = i == 0 && st.pen.is_on_start(to, xf.mm(SNAP_PX));
            let node = crate::options::get().node_size;
            let size = if closing { node * 2.5 } else { node * 1.5 };
            painter.rect_stroke(
                Rect::from_center_size(p, vec2(size, size)),
                0.0,
                Stroke::new(1.0, if closing { accent } else { text }),
                egui::StrokeKind::Middle,
            );
            if let Some(h) = n.handle {
                for q in [n.at + h, n.at - h] {
                    let s = xf.to_screen(q);
                    painter.line_segment([p, s], Stroke::new(1.0, accent.gamma_multiply(0.7)));
                    painter.circle_filled(s, 3.0, accent);
                }
            }
        }
        if let (Some(last), false) = (st.pen.last(), matches!(st.gesture, Gesture::PenHandle)) {
            let (len, ang) = tools::length_angle(last, to);
            painter.text(xf.to_screen(to) + vec2(14.0, 14.0), Align2::LEFT_TOP, format!("{len:.2} mm  {ang:.1}°"), font.clone(), text);
        }
    }

    crate::node_tool::paint(ui, app, xf, painter, accent, dark);
    crate::shape_ui::paint_preview(app, xf, painter, accent);

    // Snap marker.
    if let Some(s) = st.snapped {
        let p = xf.to_screen(s.point);
        let c = Color32::from_rgb(255, 170, 40);
        let k = 5.0;
        match s.kind {
            SnapKind::Grid => {
                painter.line_segment([p - vec2(k, 0.0), p + vec2(k, 0.0)], Stroke::new(1.0, c));
                painter.line_segment([p - vec2(0.0, k), p + vec2(0.0, k)], Stroke::new(1.0, c));
            }
            SnapKind::Node => {
                painter.rect_stroke(Rect::from_center_size(p, vec2(2.0 * k, 2.0 * k)), 0.0, Stroke::new(1.5, c), egui::StrokeKind::Middle);
            }
            SnapKind::Centre => {
                painter.circle_stroke(p, k, Stroke::new(1.5, c));
            }
            SnapKind::Midpoint => {
                let pts = vec![p + vec2(0.0, -k), p + vec2(k, k), p + vec2(-k, k)];
                painter.add(egui::Shape::closed_line(pts, Stroke::new(1.5, c)));
            }
            SnapKind::Intersection => {
                painter.line_segment([p - vec2(k, k), p + vec2(k, k)], Stroke::new(1.5, c));
                painter.line_segment([p - vec2(k, -k), p + vec2(k, -k)], Stroke::new(1.5, c));
            }
            SnapKind::OnPath => {
                let pts = vec![p + vec2(0.0, -k), p + vec2(k, 0.0), p + vec2(0.0, k), p + vec2(-k, 0.0)];
                painter.add(egui::Shape::closed_line(pts, Stroke::new(1.5, c)));
            }
        }
        if s.kind != SnapKind::Grid {
            painter.text(p + vec2(k + 4.0, -k - 2.0), Align2::LEFT_BOTTOM, s.kind.label(), font, c);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use darkburn_core::kurbo::Rect as KRect;

    #[test]
    fn dragged_shapes() {
        let m = Mods::default();
        let r = dragged_shape(Tool::Rect, Point::new(0.0, 0.0), Point::new(10.0, 5.0), m, 3).unwrap();
        assert_eq!(r.layer, 3);
        assert!((r.bbox().width() - 10.0).abs() < 1e-9);
        let c = dragged_shape(Tool::Ellipse, Point::new(0.0, 0.0), Point::new(10.0, 5.0), Mods { ctrl: true, ..m }, 0).unwrap();
        assert!((c.bbox().width() - c.bbox().height()).abs() < 0.05, "Ctrl draws a circle");
        assert!(dragged_shape(Tool::Rect, Point::new(1.0, 1.0), Point::new(1.0, 1.0), m, 0).is_none());
        let p = dragged_shape(Tool::Polygon, Point::new(0.0, 0.0), Point::new(0.0, 10.0), m, 0).unwrap();
        assert_eq!(p.nodes().len(), tools::POLYGON_SIDES as usize);
        assert!(dragged_shape(Tool::Select, Point::ZERO, Point::new(1.0, 1.0), m, 0).is_none());
    }

    #[test]
    fn handles_sit_outside_the_box() {
        let canvas = Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0));
        let view = crate::view::View { zoom: 2.0, offset: vec2(0.0, 0.0), placed: true };
        let xf = Xf::new(canvas, &view, 300.0);
        let b = KRect::new(10.0, 10.0, 50.0, 30.0);
        let pts = handle_points(&xf, b);
        assert_eq!(pts.len(), 8);
        let top_right = pts.iter().find(|(h, _)| *h == Handle { x: 1, y: 1 }).unwrap().1;
        let corner = xf.to_screen(Point::new(50.0, 30.0));
        assert!(top_right.x > corner.x && top_right.y < corner.y, "outside the top-right corner on screen");
        assert_eq!(handle_at(&xf, b, top_right), Some(Handle { x: 1, y: 1 }));
        assert_eq!(handle_at(&xf, b, xf.to_screen(b.center())), None);
    }

    #[test]
    fn snapping_follows_zoom_and_alt() {
        let canvas = Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0));
        let near = Xf::new(canvas, &crate::view::View { zoom: 20.0, offset: vec2(0.0, 0.0), placed: true }, 300.0);
        let far = Xf::new(canvas, &crate::view::View { zoom: 1.0, offset: vec2(0.0, 0.0), placed: true }, 300.0);
        let (a, b) = (snap_options(&near, Mods::default()), snap_options(&far, Mods::default()));
        assert!(a.grid.unwrap() < b.grid.unwrap(), "a finer grid when zoomed in");
        assert!(a.radius < b.radius);
        let wide = snap_options(&far, Mods { alt: true, ..Mods::default() });
        assert!((wide.radius - b.radius * SNAP_WIDE as f64).abs() < 1e-9);
    }

    /// One headless frame with every kind of shape: rendering and the tools run without panics,
    /// and Escape clears the selection and returns to the select tool.
    #[test]
    fn a_frame_draws_and_escape_clears() {
        use darkburn_core::layer::Mode;
        let ctx = egui::Context::default();
        let mut app = DarkBurn::new(crate::prefs::Prefs::default());
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(4, 4, image::Rgba([200, 10, 10, 255]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        app.editor.edit("Add", |d, sel| {
            let a = d.add(Shape::rect(KRect::new(10.0, 10.0, 60.0, 40.0), 0.0, 1));
            let mut e = Shape::ellipse(KRect::new(100.0, 100.0, 160.0, 130.0), 2);
            e.transform = Affine::rotate(0.4) * e.transform;
            d.add(e);
            d.add(Shape::rect(KRect::new(380.0, 380.0, 450.0, 450.0), 0.0, 3));
            let key = d.add_image(darkburn_core::ImageData { format: "png".into(), bytes: png.clone().into(), px_width: 4, px_height: 4 });
            let img = darkburn_core::ImageShape { image: key, width: 20.0, height: 20.0, ..Default::default() };
            d.add(Shape::new(darkburn_core::ShapeKind::Image(img), Affine::translate((200.0, 200.0)), 4));
            sel.set([a]);
            true
        });
        app.editor.doc.layer_mut(1).main_mut().mode = Mode::Fill;
        app.editor.doc.layer_mut(4).show = false;
        app.flash_layer = Some((2, 0.0));
        let frame = |app: &mut DarkBurn, events: Vec<egui::Event>| {
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1200.0, 900.0))), events, ..Default::default() };
            let mut out = ctx.run_ui(input, |ui| {
                crate::canvas::show(ui, app, None);
            });
            out.textures_delta.clear();
            out.shapes.len()
        };
        assert!(frame(&mut app, vec![]) > 0);
        app.tool = Tool::Rect;
        let esc = egui::Event::Key { key: Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE };
        frame(&mut app, vec![esc]);
        assert!(app.editor.selected().is_empty());
        assert_eq!(app.tool, Tool::Select);
    }
}
