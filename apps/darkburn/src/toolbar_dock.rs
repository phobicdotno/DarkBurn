//! Toolbars that move [TB-25]. Each toolbar has its usual place (its groups in the ribbon, the
//! tools column at the left, the cut palette at the bottom) and can instead sit at any edge of
//! the main window or float over it. It moves when its background (or, away from its usual
//! place, its grip) is dragged, or from its right-click menu. Where each one is goes into the
//! preferences with the rest of the layout, and into each device's layout.
//!
//! The ribbon itself stays at the top: it is the row of tabs that holds the toolbars, so the
//! toolbars are what move out of it and back into it.

use std::collections::BTreeMap;

use darkburn_core::layer::LAYER_COUNT;
use darkburn_ui::icons;
use darkburn_ui::ui_kit::{BackingTooltipExt as _, Container, Edge, Menu, MenuItem, Surface};
use egui::{Id, LayerId, Order, Pos2, Rect, Sense, Stroke, Ui, Vec2, pos2, vec2};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use solidcraft_theme::Tokens;

use crate::commands;
use crate::prefs::{Prefs, TOOLBARS};
use crate::ribbon::BarItem;
use crate::shell::DarkBurn;

/// Space around a toolbar's contents.
const PAD: f32 = 4.0;
/// Thickness of the grip at the start of a moved toolbar.
const GRIP: f32 = 12.0;
/// Space between buttons.
const GAP: f32 = 3.0;
/// A command button.
const BUTTON: f32 = 32.0;
/// A cut palette button.
const SWATCH: f32 = 22.0;
/// The space a separator between two ribbon groups takes.
const SEP: f32 = 9.0;
/// How near an edge a toolbar has to be let go to dock there.
pub const BAND: f32 = 56.0;
/// How far the pointer has to move before a press on a toolbar moves it.
const DRAG_START: f32 = 6.0;

/// Where a toolbar is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Dock {
    /// Its usual place: the ribbon, or its own edge for the tools and the cut palette.
    #[default]
    Home,
    Top,
    Bottom,
    Left,
    Right,
    /// Over the main window, where it was let go.
    Floating,
}

impl Dock {
    pub const ALL: [Dock; 6] = [Dock::Home, Dock::Top, Dock::Bottom, Dock::Left, Dock::Right, Dock::Floating];

    pub fn id(self) -> &'static str {
        match self {
            Dock::Home => "home",
            Dock::Top => "top",
            Dock::Bottom => "bottom",
            Dock::Left => "left",
            Dock::Right => "right",
            Dock::Floating => "floating",
        }
    }

    pub fn parse(s: &str) -> Option<Dock> {
        Dock::ALL.into_iter().find(|d| d.id() == s)
    }

    /// At a side: the buttons stand in a column.
    pub fn vertical(self) -> bool {
        matches!(self, Dock::Left | Dock::Right)
    }

    fn edge(self) -> Option<Edge> {
        match self {
            Dock::Top => Some(Edge::Top),
            Dock::Bottom => Some(Edge::Bottom),
            Dock::Left => Some(Edge::Left),
            Dock::Right => Some(Edge::Right),
            Dock::Home | Dock::Floating => None,
        }
    }
}

/// Where one toolbar is, and where it floated last.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolbarPlace {
    pub dock: Dock,
    /// The top-left corner it floats at (points); none: a place of its own.
    pub pos: Option<[f32; 2]>,
}

/// Toolbar id to place; a toolbar that is not in it is in its usual place.
pub type ToolbarPlaces = BTreeMap<String, ToolbarPlace>;

/// The edge toolbar `bar` has as its usual place; none for the toolbars of the ribbon.
pub fn home_edge(bar: &str) -> Option<Dock> {
    match bar {
        "tools" => Some(Dock::Left),
        "palette" => Some(Dock::Bottom),
        _ => None,
    }
}

/// `dock` for toolbar `bar`, with its own edge counted as its usual place.
fn normalise(bar: &str, dock: Dock) -> Dock {
    if Some(dock) == home_edge(bar) { Dock::Home } else { dock }
}

/// Where toolbar `bar` is.
pub fn dock_of(prefs: &Prefs, bar: &str) -> Dock {
    normalise(bar, prefs.toolbar_places.get(bar).map_or(Dock::Home, |p| p.dock))
}

/// Toolbar `bar` is in its usual place.
pub fn at_home(prefs: &Prefs, bar: &str) -> bool {
    dock_of(prefs, bar) == Dock::Home
}

/// The edge `bar` is shown at (its own edge when it is home); `Home` for a ribbon toolbar at home.
fn shown_at(bar: &str, dock: Dock) -> Dock {
    match (dock, home_edge(bar)) {
        (Dock::Home, Some(edge)) => edge,
        (d, _) => d,
    }
}

/// Drop places of toolbars that do not exist, and positions that are not numbers.
pub fn sanitise(places: &mut ToolbarPlaces) {
    places.retain(|id, _| TOOLBARS.iter().any(|(t, _)| *t == id.as_str()));
    for (id, p) in places.iter_mut() {
        p.dock = normalise(id, p.dock);
        if p.pos.is_some_and(|xy| !xy.iter().all(|v| v.is_finite() && v.abs() < 100_000.0)) {
            p.pos = None;
        }
    }
    places.retain(|_, p| p.dock != Dock::Home);
}

/// Put toolbar `bar` at `dock`; a floating one at `pos` (none: where it floated last).
pub fn set_place(prefs: &mut Prefs, bar: &str, dock: Dock, pos: Option<Pos2>) {
    let dock = normalise(bar, dock);
    if dock == Dock::Home {
        prefs.toolbar_places.remove(bar);
        return;
    }
    let old = prefs.toolbar_places.get(bar).and_then(|p| p.pos);
    let pos = pos.filter(|p| p.x.is_finite() && p.y.is_finite()).map(|p| [p.x, p.y]).or(old);
    prefs.toolbar_places.insert(bar.to_string(), ToolbarPlace { dock, pos });
}

/// What the `window.toolbar_place` command reports about toolbar `bar`.
fn describe(prefs: &Prefs, bar: &str) -> Value {
    let dock = dock_of(prefs, bar);
    let pos = prefs.toolbar_places.get(bar).and_then(|p| p.pos);
    json!({
        "toolbar": bar,
        "place": dock.id(),
        "edge": shown_at(bar, dock).id(),
        "vertical": shown_at(bar, dock).vertical(),
        "pos": pos,
        "shown": prefs.toolbar(bar),
    })
}

/// `window.toolbar_place` {toolbar, place, x?, y?}: move a toolbar (and show it).
pub fn place_command(app: &mut DarkBurn, a: &Value) -> Result<Value, String> {
    let bar = a.get("toolbar").and_then(Value::as_str).ok_or("argument toolbar must be a toolbar id")?;
    let Some(&(bar, _)) = TOOLBARS.iter().find(|(t, _)| *t == bar) else {
        return Err(format!("unknown toolbar {bar}"));
    };
    let place = a.get("place").and_then(Value::as_str).ok_or("argument place must be home, top, bottom, left, right or floating")?;
    let dock = Dock::parse(place).ok_or_else(|| format!("unknown place {place}: home, top, bottom, left, right or floating"))?;
    let num = |k: &str| a.get(k).and_then(Value::as_f64).map(|v| v as f32).filter(|v| v.is_finite());
    let pos = match (num("x"), num("y")) {
        (Some(x), Some(y)) => Some(pos2(x, y)),
        (None, None) => None,
        _ => return Err("give both x and y, or neither".into()),
    };
    app.prefs.toolbars.insert(bar.to_string(), true);
    set_place(&mut app.prefs, bar, dock, pos);
    Ok(describe(&app.prefs, bar))
}

// ----- moves asked for while drawing ---------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
enum Move {
    Place(Dock, Option<Pos2>),
    Hide,
}

fn moves_id() -> Id {
    Id::unique("db_toolbar_moves")
}

fn ribbon_id() -> Id {
    Id::unique("db_toolbar_ribbon_rect")
}

fn request(ctx: &egui::Context, bar: &str, m: Move) {
    ctx.data_mut(|d| d.get_temp_mut_or_default::<Vec<(String, Move)>>(moves_id()).push((bar.to_string(), m)));
}

/// Make the moves the toolbars asked for this frame (the shell calls it once the frame is drawn).
pub fn apply_requests(ctx: &egui::Context, app: &mut DarkBurn) {
    let todo = ctx.data_mut(|d| {
        d.remove::<Rect>(ribbon_id());
        d.remove_temp::<Vec<(String, Move)>>(moves_id())
    });
    for (bar, m) in todo.unwrap_or_default() {
        match m {
            Move::Place(dock, pos) => set_place(&mut app.prefs, &bar, dock, pos),
            Move::Hide => {
                app.prefs.toolbars.insert(bar, false);
            }
        }
    }
}

/// The ribbon tells where it is this frame (a ribbon toolbar let go on it goes back into it).
pub fn note_ribbon(ctx: &egui::Context, rect: Rect) {
    ctx.data_mut(|d| d.insert_temp(ribbon_id(), rect));
}

fn ribbon_rect(ctx: &egui::Context) -> Option<Rect> {
    ctx.data(|d| d.get_temp(ribbon_id()))
}

/// The bottom of what is fixed at the top of the window: the ribbon, else the application bar.
fn top_of(screen: Rect, ribbon: Option<Rect>) -> f32 {
    ribbon.map_or(screen.top() + crate::shell::APP_BAR_HEIGHT, |r| r.bottom())
}

/// Where toolbar `bar` goes when let go at `pos`, in a window `screen` with the ribbon at
/// `ribbon` (none: no ribbon shown). Above the top of the canvas a ribbon toolbar goes back into
/// the ribbon; near an edge a toolbar docks there; anywhere else it floats.
pub fn target(bar: &str, pos: Pos2, screen: Rect, ribbon: Option<Rect>) -> Dock {
    let top = top_of(screen, ribbon);
    let to = if pos.y < top {
        if home_edge(bar).is_none() { Dock::Home } else { Dock::Top }
    } else if pos.y < top + BAND {
        Dock::Top
    } else if pos.x < screen.left() + BAND {
        Dock::Left
    } else if pos.x > screen.right() - BAND {
        Dock::Right
    } else if pos.y > screen.bottom() - BAND {
        Dock::Bottom
    } else {
        Dock::Floating
    };
    normalise(bar, to)
}

/// The top-left corner of a toolbar let go at `pos` to float (the grip under the pointer).
fn float_at(pos: Pos2) -> Pos2 {
    pos - vec2(PAD + GRIP / 2.0, PAD + BUTTON / 2.0)
}

/// The rectangle to light up while toolbar `bar` is dragged towards `to`.
fn zone(ctx: &egui::Context, bar: &str, to: Dock, pos: Pos2) -> Rect {
    let screen = ctx.content_rect();
    let ribbon = ribbon_rect(ctx);
    let top = top_of(screen, ribbon);
    let thick = 40.0;
    match shown_at(bar, to) {
        Dock::Home => ribbon.unwrap_or_else(|| Rect::from_min_max(screen.min, pos2(screen.right(), top))),
        Dock::Top => Rect::from_min_max(pos2(screen.left(), top), pos2(screen.right(), top + thick)),
        Dock::Bottom => Rect::from_min_max(pos2(screen.left(), screen.bottom() - thick), screen.max),
        Dock::Left => Rect::from_min_max(pos2(screen.left(), top), pos2(screen.left() + thick, screen.bottom())),
        Dock::Right => Rect::from_min_max(pos2(screen.right() - thick, top), screen.max),
        Dock::Floating => Rect::from_min_size(float_at(pos), vec2(160.0, BUTTON + 2.0 * PAD)),
    }
}

fn paint_hint(ctx: &egui::Context, rect: Rect) {
    let t = Tokens::get();
    let p = ctx.layer_painter(LayerId::new(Order::Foreground, Id::unique("db_toolbar_hint")));
    p.rect(rect, t.radius_sm, t.accent.gamma_multiply(0.18), Stroke::new(2.0, t.accent), egui::StrokeKind::Inside);
    solidcraft_ui_registry::publish_target("bar:hint", rect);
}

/// The handle of toolbar `bar`, now at `now`: drag it to move the toolbar, right-click it for
/// its menu. Call it before the toolbar's buttons are drawn so they stay on top of it. `salt`
/// tells apart the handles of one toolbar (a ribbon toolbar has one in each of its groups).
/// It draws nothing.
pub fn grip(ui: &mut Ui, bar: &'static str, salt: &str, rect: Rect, now: Dock) {
    let ctx = ui.ctx().clone();
    let id = Id::unique(("db_toolbar_grip", bar, salt));
    let resp = ui.interact(rect, id, Sense::click_and_drag());
    let origin_id = id.with("origin");
    if resp.is_pointer_button_down_on()
        && let Some(p) = ctx.input(|i| i.pointer.press_origin())
    {
        ctx.data_mut(|d| d.insert_temp(origin_id, p));
    }
    let origin: Option<Pos2> = ctx.data(|d| d.get_temp(origin_id));
    let pos = ctx.input(|i| i.pointer.latest_pos());
    let moved = matches!((origin, pos), (Some(o), Some(p)) if o.distance(p) >= DRAG_START);
    if resp.dragged_by(egui::PointerButton::Primary)
        && moved
        && let Some(p) = pos
    {
        ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
        let to = target(bar, p, ctx.content_rect(), ribbon_rect(&ctx));
        paint_hint(&ctx, zone(&ctx, bar, to, p));
    }
    if resp.drag_stopped_by(egui::PointerButton::Primary)
        && moved
        && let Some(p) = pos
    {
        let to = target(bar, p, ctx.content_rect(), ribbon_rect(&ctx));
        if to != now || to == Dock::Floating {
            request(&ctx, bar, Move::Place(to, (to == Dock::Floating).then(|| float_at(p))));
        }
    }
    if !resp.is_pointer_button_down_on() && !resp.dragged() {
        ctx.data_mut(|d| d.remove::<Pos2>(origin_id));
    }
    Menu::context(&resp, |ui| menu(ui, bar, now));
}

/// The right-click menu of toolbar `bar`, now at `now`.
fn menu(ui: &mut Ui, bar: &'static str, now: Dock) {
    ui.set_min_width(210.0);
    let ctx = ui.ctx().clone();
    let shown = shown_at(bar, now);
    let mut places = vec![
        ("bar_top", crate::i18n::tr("bar.dock_top", "Dock at the Top"), Dock::Top),
        ("bar_bottom", crate::i18n::tr("bar.dock_bottom", "Dock at the Bottom"), Dock::Bottom),
        ("bar_left", crate::i18n::tr("bar.dock_left", "Dock at the Left"), Dock::Left),
        ("bar_right", crate::i18n::tr("bar.dock_right", "Dock at the Right"), Dock::Right),
        ("bar_float", crate::i18n::tr("bar.float", "Float"), Dock::Floating),
    ];
    if home_edge(bar).is_none() {
        places.push(("bar_home", crate::i18n::tr("bar.home", "Back in the Ribbon"), Dock::Home));
    }
    for (id, text, dock) in places {
        let mut item = MenuItem::new(id, &text, "");
        item.checked = Some(shown == dock);
        if item.show(ui).clicked() && shown != dock {
            request(&ctx, bar, Move::Place(dock, None));
        }
    }
    Menu::separator(ui);
    if MenuItem::new("bar_hide", &crate::i18n::tr("ui.hide", "Hide"), "close").show(ui).clicked() {
        request(&ctx, bar, Move::Hide);
    }
}

// ----- drawing -------------------------------------------------------------------------------

/// One thing on a moved toolbar.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Piece {
    Cmd(&'static str),
    /// A wide ribbon control (the numeric edits, the text options, the docking options).
    Wide(&'static str),
    /// A cut palette button.
    Swatch(usize),
    /// Between two groups of a ribbon toolbar.
    Sep,
}

fn pieces(bar: &str) -> Vec<Piece> {
    match bar {
        "tools" => crate::shell::tool_ids().iter().copied().map(Piece::Cmd).collect(),
        "palette" => (0..LAYER_COUNT).map(Piece::Swatch).collect(),
        _ => {
            let mut out = Vec::new();
            for group in crate::ribbon::bar_groups(bar) {
                if !out.is_empty() {
                    out.push(Piece::Sep);
                }
                out.extend(group.into_iter().map(|i| match i {
                    BarItem::Cmd(id) => Piece::Cmd(id),
                    BarItem::Wide(kind) => Piece::Wide(kind),
                }));
            }
            out
        }
    }
}

fn wide_width_id(kind: &str) -> Id {
    Id::unique(("db_toolbar_wide", kind))
}

/// The size of `piece` (a separator has no thickness across the toolbar: it spans its line).
fn piece_size(ctx: &egui::Context, piece: Piece, vertical: bool) -> Vec2 {
    match piece {
        Piece::Cmd(_) => vec2(BUTTON, BUTTON),
        Piece::Swatch(_) => vec2(SWATCH, SWATCH),
        Piece::Sep if vertical => vec2(0.0, SEP),
        Piece::Sep => vec2(SEP, 0.0),
        Piece::Wide(kind) => {
            let w = ctx.data(|d| d.get_temp::<f32>(wide_width_id(kind))).unwrap_or_else(|| crate::ribbon::wide_width(kind));
            vec2(w, crate::ribbon::WIDE_HEIGHT)
        }
    }
}

/// Lay out items of `sizes` one after another along x (along y when `vertical`), starting a new
/// line when the next would reach past `room`. Returns each item's rectangle from (0, 0) and the
/// size of them all. Items are centred across their line; one with no thickness across (a
/// separator) spans its line.
fn flow(sizes: &[Vec2], vertical: bool, room: f32) -> (Vec<Rect>, Vec2) {
    let split = |v: Vec2| if vertical { (v.y, v.x) } else { (v.x, v.y) };
    let join = |main: f32, cross: f32| if vertical { vec2(cross, main) } else { vec2(main, cross) };
    let mut lines: Vec<(usize, usize, f32)> = Vec::new();
    let (mut start, mut at, mut thick) = (0, 0.0_f32, 0.0_f32);
    for (i, s) in sizes.iter().enumerate() {
        let (m, c) = split(*s);
        if i > start && at + m > room {
            lines.push((start, i, thick));
            (start, at, thick) = (i, 0.0, 0.0);
        }
        at += m + GAP;
        thick = thick.max(c);
    }
    if start < sizes.len() {
        lines.push((start, sizes.len(), thick));
    }
    let mut rects = vec![Rect::NOTHING; sizes.len()];
    let (mut across, mut longest) = (0.0_f32, 0.0_f32);
    for (s, e, thick) in lines {
        let mut along = 0.0;
        for (rect, size) in rects[s..e].iter_mut().zip(&sizes[s..e]) {
            let (m, c) = split(*size);
            let c = if c <= 0.0 { thick } else { c };
            let min = join(along, across + (thick - c) / 2.0);
            *rect = Rect::from_min_size(min.to_pos2(), join(m, c));
            along += m + GAP;
        }
        longest = longest.max(along - GAP);
        across += thick + GAP;
    }
    (rects, join(longest, (across - GAP).max(0.0)))
}

/// The toolbars shown at `dock`, in the user's toolbar order.
fn placed(prefs: &Prefs, dock: Dock) -> Vec<&'static str> {
    prefs
        .full_toolbar_order()
        .iter()
        .filter_map(|id| TOOLBARS.iter().find(|(t, _)| t == id).map(|(t, _)| *t))
        .filter(|bar| prefs.toolbar(bar) && dock_of(prefs, bar) == dock)
        .collect()
}

/// The toolbars docked at edge `dock` of the window, each a strip of its own.
pub fn edge(ui: &mut Ui, app: &mut DarkBurn, dock: Dock, clicked: &mut Option<String>) {
    let Some(edge) = dock.edge() else { return };
    for bar in placed(&app.prefs, dock) {
        let t = Tokens::get();
        let vertical = dock.vertical();
        let pieces = pieces(bar);
        let sizes: Vec<Vec2> = pieces.iter().map(|p| piece_size(ui.ctx(), *p, vertical)).collect();
        let avail = ui.available_rect_before_wrap();
        let length = if vertical { avail.height() } else { avail.width() };
        let room = (length - GRIP - 3.0 * PAD).max(BUTTON);
        let (rects, total) = flow(&sizes, vertical, room);
        let across = if vertical { total.x } else { total.y };
        let thick = across + 2.0 * PAD;
        let host = Container::new(ui.ctx(), Surface::Chrome);
        let panel = host.edge(("db_bar", bar), edge).exact_size(thick).resizable(false);
        let frame = host.edge_frame(t.toolbar);
        host.show_edge(ui, panel, frame, |ui| {
            let r = ui.max_rect();
            draw(ui, app, bar, dock, r, &pieces, &rects, clicked);
        });
    }
}

/// The floating toolbars, each a small card over the window.
pub fn floating(ui: &mut Ui, app: &mut DarkBurn, clicked: &mut Option<String>) {
    let ctx = ui.ctx().clone();
    let screen = ctx.content_rect();
    let t = Tokens::get();
    for (n, bar) in placed(&app.prefs, Dock::Floating).into_iter().enumerate() {
        let pieces = pieces(bar);
        let sizes: Vec<Vec2> = pieces.iter().map(|p| piece_size(&ctx, *p, false)).collect();
        let (rects, total) = flow(&sizes, false, (screen.width() * 0.8).max(BUTTON));
        let size = vec2(total.x + GRIP + 3.0 * PAD, total.y + 2.0 * PAD);
        let first = pos2(screen.left() + 120.0 + 30.0 * n as f32, screen.top() + 200.0 + 56.0 * n as f32);
        let pos = app.prefs.toolbar_places.get(bar).and_then(|p| p.pos).map_or(first, |[x, y]| pos2(x, y));
        // Keep at least the grip inside the window.
        let pos = pos2(
            pos.x.clamp(screen.left(), (screen.right() - 3.0 * GRIP).max(screen.left())),
            pos.y.clamp(screen.top(), (screen.bottom() - size.y).max(screen.top())),
        );
        egui::Area::new(Id::unique(("db_bar_float", bar))).order(Order::Middle).fixed_pos(pos).constrain(false).show(&ctx, |ui| {
            let (r, _) = ui.allocate_exact_size(size, Sense::hover());
            ui.painter().rect(r, t.radius_md, t.toolbar, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
            draw(ui, app, bar, Dock::Floating, r, &pieces, &rects, clicked);
        });
    }
}

/// A moved toolbar in `r`: its grip, then its pieces at `rects` (from the corner after the grip).
#[allow(clippy::too_many_arguments, reason = "one drawing pass over a toolbar's parts")]
fn draw(ui: &mut Ui, app: &mut DarkBurn, bar: &'static str, dock: Dock, r: Rect, pieces: &[Piece], rects: &[Rect], clicked: &mut Option<String>) {
    let t = Tokens::get();
    let vertical = dock.vertical();
    grip(ui, bar, "bar", r, dock);
    let handle = if vertical {
        Rect::from_min_size(r.min + vec2(PAD, PAD), vec2(r.width() - 2.0 * PAD, GRIP))
    } else {
        Rect::from_min_size(r.min + vec2(PAD, PAD), vec2(GRIP, r.height() - 2.0 * PAD))
    };
    paint_grip(ui, handle, vertical);
    solidcraft_ui_registry::publish_target(&format!("bar:{bar}"), r);
    solidcraft_ui_registry::publish_target(&format!("bar:{bar}:grip"), handle);
    let origin = if vertical { pos2(r.left() + PAD, handle.bottom() + PAD) } else { pos2(handle.right() + PAD, r.top() + PAD) };
    // Centre the pieces across the strip (it may be thicker than they are).
    let across = rects.iter().fold(Rect::NOTHING, |a, b| a.union(*b));
    let shift = if vertical {
        vec2(((r.width() - 2.0 * PAD) - across.width()).max(0.0) / 2.0, 0.0)
    } else {
        vec2(0.0, ((r.height() - 2.0 * PAD) - across.height()).max(0.0) / 2.0)
    };
    let mut swatch = None;
    let used = if bar == "palette" { app.editor.doc.used_layers() } else { Vec::new() };
    for (piece, rect) in pieces.iter().zip(rects) {
        let rect = rect.translate(origin.to_vec2() + shift);
        match *piece {
            Piece::Cmd(id) => button(ui, app, bar, id, rect, clicked),
            Piece::Wide(kind) => {
                let right = crate::ribbon::draw_wide(ui, kind, rect.min, app);
                let w = (right - rect.left()).max(BUTTON);
                if (w - rect.width()).abs() > 0.5 {
                    ui.ctx().data_mut(|d| d.insert_temp(wide_width_id(kind), w));
                    ui.ctx().request_repaint();
                }
            }
            Piece::Swatch(i) => {
                if let Some(click) = crate::palette::swatch(ui, app, i, rect, &used) {
                    swatch = Some((i, click));
                }
            }
            Piece::Sep => {
                let stroke = Stroke::new(1.0, t.border);
                let c = rect.center();
                if vertical {
                    ui.painter().hline(rect.x_range().shrink(2.0), c.y, stroke);
                } else {
                    ui.painter().vline(c.x, rect.y_range().shrink(2.0), stroke);
                }
            }
        }
    }
    if let Some((layer, click)) = swatch {
        let now = ui.input(|i| i.time);
        crate::palette::apply(app, layer, click, now);
    }
}

/// Two rows of dots along the grip.
fn paint_grip(ui: &Ui, r: Rect, vertical: bool) {
    let t = Tokens::get();
    let ink = t.text_dim.gamma_multiply(0.8);
    let (len, step) = if vertical { (r.width(), 5.0) } else { (r.height(), 5.0) };
    let n = ((len - 8.0) / step).floor().clamp(0.0, 12.0) as usize;
    let start = (len - (n.saturating_sub(1)) as f32 * step) / 2.0;
    for i in 0..n {
        let along = start + i as f32 * step;
        for side in [-2.0, 2.0] {
            let p = if vertical { pos2(r.left() + along, r.center().y + side) } else { pos2(r.center().x + side, r.top() + along) };
            ui.painter().circle_filled(p, 1.0, ink);
        }
    }
}

/// A command button of a moved toolbar: its icon, the command's tip.
fn button(ui: &mut Ui, app: &mut DarkBurn, bar: &str, id: &'static str, rect: Rect, clicked: &mut Option<String>) {
    let t = Tokens::get();
    let Some(c) = commands::find(id) else { return };
    let enabled = commands::works(id);
    let resp = ui.interact(rect, Id::unique(("db_bar_button", bar, id)), if enabled { Sense::click() } else { Sense::hover() });
    let active = (id == "view.pan" && app.pan_tool) || (bar == "tools" && id == app.tool.command() && !app.pan_tool);
    if active || (enabled && resp.is_pointer_button_down_on()) {
        ui.painter().rect(rect, t.radius_sm, t.accent_soft, Stroke::new(1.0, t.accent), egui::StrokeKind::Inside);
    } else if enabled && resp.hovered() {
        ui.painter().rect_filled(rect, t.radius_sm, t.hover);
    }
    icons::paint_icon(ui.painter(), rect.shrink(6.0), c.icon, if enabled { 1.0 } else { 0.4 });
    solidcraft_ui_registry::publish_target(&format!("bar:{bar}:{id}"), rect);
    if bar == "tools" {
        solidcraft_ui_registry::publish_target(&format!("tool:{id}"), rect);
    }
    let tip = if enabled {
        crate::shell::command_tip(c)
    } else {
        format!("{} - {}", crate::i18n::cmd_label(c), crate::i18n::tr("ui.later_version", "arrives in a later version"))
    };
    if resp.hovered() {
        app.hint = Some(tip.clone());
    }
    if resp.backing_on_hover_text(tip).clicked() && enabled {
        *clicked = Some(id.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::execute_headless;

    #[test]
    fn a_toolbar_at_its_own_edge_is_in_its_usual_place() {
        let mut p = Prefs::default();
        for (bar, _) in TOOLBARS {
            assert_eq!(dock_of(&p, bar), Dock::Home, "{bar}");
        }
        set_place(&mut p, "tools", Dock::Left, None);
        set_place(&mut p, "palette", Dock::Bottom, None);
        assert!(p.toolbar_places.is_empty(), "the tools at the left and the palette at the bottom are where they always are");
        set_place(&mut p, "tools", Dock::Right, None);
        set_place(&mut p, "main", Dock::Left, None);
        assert_eq!((dock_of(&p, "tools"), dock_of(&p, "main")), (Dock::Right, Dock::Left));
        set_place(&mut p, "main", Dock::Home, None);
        assert!(at_home(&p, "main") && !p.toolbar_places.contains_key("main"));
    }

    #[test]
    fn a_floating_toolbar_remembers_where_it_floated() {
        let mut p = Prefs::default();
        set_place(&mut p, "modes", Dock::Floating, Some(pos2(300.0, 240.0)));
        assert_eq!(p.toolbar_places["modes"], ToolbarPlace { dock: Dock::Floating, pos: Some([300.0, 240.0]) });
        set_place(&mut p, "modes", Dock::Top, None);
        set_place(&mut p, "modes", Dock::Floating, None);
        assert_eq!(p.toolbar_places["modes"].pos, Some([300.0, 240.0]), "floated again: back where it was");
    }

    #[test]
    fn stored_places_are_cleaned() {
        let json =
            r#"{"toolbar_places":{"main":{"dock":"left"},"gone":{"dock":"top"},"tools":{"dock":"left"},"modes":{"dock":"floating","pos":[1e30,5]}}}"#;
        let p = Prefs::from_json(json);
        assert_eq!(p.toolbar_places.keys().collect::<Vec<_>>(), ["main", "modes"]);
        assert_eq!(p.toolbar_places["modes"].pos, None);
        // A place this version does not know costs only the toolbar places.
        let p = Prefs::from_json(r#"{"toolbar_places":{"main":{"dock":"diagonal"}},"show_grid":false}"#);
        assert!(p.toolbar_places.is_empty() && !p.show_grid);
        // Round trip.
        let mut p = Prefs::default();
        set_place(&mut p, "numeric", Dock::Right, None);
        set_place(&mut p, "palette", Dock::Floating, Some(pos2(10.0, 20.0)));
        assert_eq!(Prefs::from_json(&p.to_json()).toolbar_places, p.toolbar_places);
        p.reset_layout();
        assert!(p.toolbar_places.is_empty(), "reset puts every toolbar back");
    }

    #[test]
    fn letting_go_near_an_edge_docks_there() {
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(1600.0, 1000.0));
        let ribbon = Some(Rect::from_min_max(pos2(0.0, 34.0), pos2(1600.0, 151.0)));
        assert_eq!(target("main", pos2(800.0, 100.0), screen, ribbon), Dock::Home, "back into the ribbon");
        assert_eq!(target("tools", pos2(800.0, 100.0), screen, ribbon), Dock::Top);
        assert_eq!(target("main", pos2(800.0, 170.0), screen, ribbon), Dock::Top);
        assert_eq!(target("main", pos2(10.0, 500.0), screen, ribbon), Dock::Left);
        assert_eq!(target("tools", pos2(10.0, 500.0), screen, ribbon), Dock::Home, "the tools' own edge");
        assert_eq!(target("main", pos2(1590.0, 500.0), screen, ribbon), Dock::Right);
        assert_eq!(target("main", pos2(800.0, 990.0), screen, ribbon), Dock::Bottom);
        assert_eq!(target("palette", pos2(800.0, 990.0), screen, ribbon), Dock::Home);
        assert_eq!(target("main", pos2(800.0, 500.0), screen, ribbon), Dock::Floating);
        // No ribbon (every ribbon toolbar moved out): the application bar is the top.
        assert_eq!(target("main", pos2(800.0, 20.0), screen, None), Dock::Home);
        assert_eq!(target("main", pos2(800.0, 60.0), screen, None), Dock::Top);
    }

    #[test]
    fn flow_lays_out_in_rows_or_columns() {
        let sizes = [vec2(32.0, 32.0), vec2(9.0, 0.0), vec2(100.0, 60.0)];
        let (rects, total) = flow(&sizes, false, 1000.0);
        assert_eq!(total, vec2(32.0 + 9.0 + 100.0 + 2.0 * GAP, 60.0));
        assert_eq!(rects[0], Rect::from_min_size(pos2(0.0, 14.0), vec2(32.0, 32.0)), "centred across the row");
        assert_eq!(rects[1].height(), 60.0, "a separator spans its row");
        // Vertical: a column as wide as the widest item.
        let sizes = [vec2(32.0, 32.0), vec2(0.0, 9.0), vec2(100.0, 60.0)];
        let (rects, total) = flow(&sizes, true, 1000.0);
        assert_eq!(total, vec2(100.0, 32.0 + 9.0 + 60.0 + 2.0 * GAP));
        assert_eq!(rects[0].min, pos2(34.0, 0.0));
        assert_eq!(rects[2].min, pos2(0.0, 32.0 + 9.0 + 2.0 * GAP));
        // Too long for the room: a second column.
        let sizes = vec![vec2(22.0, 22.0); 10];
        let (rects, total) = flow(&sizes, true, 125.0);
        assert_eq!(rects[4].min, pos2(0.0, 4.0 * 25.0));
        assert_eq!(rects[5].min, pos2(25.0, 0.0));
        assert_eq!(total, vec2(47.0, 4.0 * 25.0 + 22.0));
        assert_eq!(flow(&[], false, 10.0).1, Vec2::ZERO);
    }

    #[test]
    fn every_toolbar_has_something_to_show_away_from_home() {
        for (bar, _) in TOOLBARS {
            let p = pieces(bar);
            assert!(p.iter().any(|p| !matches!(p, Piece::Sep)), "{bar}");
            assert!(p.first() != Some(&Piece::Sep) && p.last() != Some(&Piece::Sep), "{bar}");
        }
        assert_eq!(pieces("palette").len(), LAYER_COUNT);
        assert!(pieces("numeric").contains(&Piece::Wide("numeric")));
        assert!(pieces("main").contains(&Piece::Cmd("file.open")) && pieces("main").contains(&Piece::Sep));
    }

    #[test]
    fn the_place_command_moves_a_toolbar_and_shows_it() {
        let mut a = DarkBurn::new(Prefs::default());
        a.prefs.toolbars.insert("modes".into(), false);
        let v = execute_headless(&mut a, "window.toolbar_place", &json!({ "toolbar": "modes", "place": "left" })).unwrap();
        assert_eq!((v["place"].as_str(), v["vertical"].as_bool(), v["shown"].as_bool()), (Some("left"), Some(true), Some(true)));
        let v = execute_headless(&mut a, "window.toolbar_place", &json!({ "toolbar": "tools", "place": "left" })).unwrap();
        assert_eq!((v["place"].as_str(), v["edge"].as_str()), (Some("home"), Some("left")));
        let v = execute_headless(&mut a, "window.toolbar_place", &json!({ "toolbar": "palette", "place": "floating", "x": 50, "y": 60 })).unwrap();
        assert_eq!(v["pos"], json!([50.0, 60.0]));
        for bad in [
            json!({ "toolbar": "nope", "place": "left" }),
            json!({ "toolbar": "main", "place": "middle" }),
            json!({ "toolbar": "main" }),
            json!({ "toolbar": "main", "place": "floating", "x": 5 }),
        ] {
            assert!(execute_headless(&mut a, "window.toolbar_place", &bad).is_err(), "{bad}");
        }
        let info = crate::commands::describe("window.toolbar_place").unwrap();
        assert!(info.implemented && !info.hardware && info.checklist.iter().any(|c| c == "TB-25"));
    }
}
