//! Where a settings window lives: a floating card in the main window, its own native window, or a
//! tab in one of the app's panel areas.
//!
//! The kit does not know the app's panel areas. The app names them (by a short key such as
//! `"RightTop"`), says each frame where they are on screen, and draws the tabs of the windows
//! docked in them. A docked window draws its contents into the body rectangle its tab group
//! gave it ([`set_slot`]). What the user chose for each window, and the size and position it was
//! given, is a [`WindowPlaces`] map the app stores with its other settings ([`sync`]).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use egui::{Context, Id, LayerId, Pos2, Rect, Vec2, vec2};
use serde::{Deserialize, Serialize};

/// The smallest size a window can be dragged to, outer size in points.
pub const MIN_SIZE: Vec2 = vec2(300.0, 180.0);
/// The largest size remembered, so a stored size from a huge screen cannot ask for the moon.
pub const MAX_SIZE: Vec2 = vec2(8192.0, 8192.0);
/// How many windows' places are remembered.
pub const MAX_WINDOWS: usize = 256;
/// A tab id in the app's active-tab memory that stands for a docked window.
pub const TAB_PREFIX: &str = "window:";
/// How many key presses from native windows are kept until the app takes them.
const MAX_KEYS: usize = 32;

/// Where a window is shown.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Placement {
    /// A card floating over the main window (how every window started).
    #[default]
    Floating,
    /// A native window of its own.
    Detached,
    /// A tab in the app's panel area with this key.
    Docked(String),
}

/// What is remembered about one window.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowPlace {
    pub placement: Placement,
    /// Top-left corner of the floating card in the main window, points.
    pub pos: Option<[f32; 2]>,
    /// Outer size of the floating card, once the user resized it; none: its natural size.
    pub size: Option<[f32; 2]>,
    /// Outer position of the native window, where the system tells it.
    pub native_pos: Option<[f32; 2]>,
    /// Inner size of the native window.
    pub native_size: Option<[f32; 2]>,
    /// Where its tab is among the windows docked in the same area, in the order they were
    /// docked there: lower first.
    pub tab: Option<u32>,
}

/// Every window's place, by the window's key.
pub type WindowPlaces = BTreeMap<String, WindowPlace>;

fn finite(v: [f32; 2]) -> bool {
    v[0].is_finite() && v[1].is_finite()
}

fn clamp_size(v: [f32; 2]) -> [f32; 2] {
    [v[0].clamp(MIN_SIZE.x, MAX_SIZE.x), v[1].clamp(MIN_SIZE.y, MAX_SIZE.y)]
}

impl WindowPlace {
    /// Back to a floating card; at `pos` (its top-left corner) when given, else where it was.
    pub fn float(&mut self, pos: Option<Pos2>) {
        self.placement = Placement::Floating;
        if let Some(p) = pos.filter(|p| p.x.is_finite() && p.y.is_finite()) {
            self.pos = Some([p.x, p.y]);
        }
    }

    /// A tab in area `area`. The floating size and position are kept for when it floats again.
    pub fn dock(&mut self, area: &str) {
        if !area.is_empty() {
            self.placement = Placement::Docked(area.to_string());
        }
    }

    /// Its own native window.
    pub fn detach(&mut self) {
        self.placement = Placement::Detached;
    }

    /// The user dragged the floating card to outer size `size`.
    pub fn resize(&mut self, size: Vec2) {
        if size.x.is_finite() && size.y.is_finite() {
            self.size = Some(clamp_size([size.x, size.y]));
        }
    }

    /// The docked area, if docked.
    pub fn area(&self) -> Option<&str> {
        match &self.placement {
            Placement::Docked(a) => Some(a),
            _ => None,
        }
    }

    /// Drop what cannot be right: values that are not numbers, sizes out of range.
    pub fn sanitise(&mut self) {
        for v in [&mut self.pos, &mut self.native_pos] {
            if v.is_some_and(|p| !finite(p) || p[0].abs() > 1.0e6 || p[1].abs() > 1.0e6) {
                *v = None;
            }
        }
        for v in [&mut self.size, &mut self.native_size] {
            *v = v.filter(|s| finite(*s)).map(clamp_size);
        }
        if self.placement == Placement::Docked(String::new()) {
            self.placement = Placement::Floating;
        }
    }
}

/// Clean a stored map: bad values dropped, at most [`MAX_WINDOWS`] entries, empty keys gone.
pub fn sanitise(places: &mut WindowPlaces) {
    places.retain(|k, _| !k.is_empty() && k.len() <= 128);
    while places.len() > MAX_WINDOWS {
        let last = places.keys().next_back().cloned().unwrap_or_default();
        places.remove(&last);
    }
    for p in places.values_mut() {
        p.sanitise();
    }
}

/// Read stored places one window at a time: an entry this version cannot read (a placement
/// added later, a file edited by hand) falls back to a floating card instead of costing the
/// others. For `#[serde(deserialize_with = "...")]`.
pub fn lenient<'de, D: serde::Deserializer<'de>>(d: D) -> Result<WindowPlaces, D::Error> {
    let raw = BTreeMap::<String, serde_json::Value>::deserialize(d).unwrap_or_default();
    let mut places: WindowPlaces = raw.into_iter().map(|(k, v)| (k, serde_json::from_value(v).unwrap_or_default())).collect();
    sanitise(&mut places);
    Ok(places)
}

/// The app's active-tab id for the window `key`.
pub fn tab_id(key: &str) -> String {
    format!("{TAB_PREFIX}{key}")
}

/// The window key of an active-tab id, if it stands for a window.
pub fn window_of_tab(id: &str) -> Option<&str> {
    id.strip_prefix(TAB_PREFIX)
}

/// A docked window's tab, for the app to draw.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DockedTab {
    pub key: String,
    pub title: String,
}

#[derive(Clone, Debug)]
struct Shown {
    title: String,
    frame: u64,
}

#[derive(Clone, Copy, Debug)]
struct Slot {
    rect: Rect,
    layer: LayerId,
    frame: u64,
}

#[derive(Clone, Debug)]
struct Target {
    rect: Rect,
    frame: u64,
}

/// The kit's state for one context, shared through egui's temporary data.
#[derive(Default)]
struct State {
    places: WindowPlaces,
    /// The places changed in the UI since the app last took them.
    dirty: bool,
    shown: BTreeMap<String, Shown>,
    slots: BTreeMap<String, Slot>,
    targets: BTreeMap<String, Target>,
    /// The app's panel areas, key and label, in its order.
    areas: Vec<(String, String)>,
    close: BTreeSet<String>,
    activate: BTreeMap<String, String>,
    /// A native window's size and position as it was opened, kept while it stays open.
    opened: BTreeMap<String, ([f32; 2], Option<[f32; 2]>)>,
    /// Key presses made in native windows that nothing in them used, oldest first.
    keys: Vec<egui::Event>,
    /// Windows a command asked to bring into view, with the frame it asked in.
    raise: BTreeMap<String, u64>,
    /// The native windows that had the system focus, with the frame they last had it in.
    native_focus: BTreeMap<String, u64>,
    /// A native window had input that can change a setting, or was moved, since the app last
    /// asked ([`take_native_input`]).
    native_input: bool,
    /// The texts of the header menu and the drop hint, in the app's language.
    texts: Option<WindowTexts>,
    /// A window being dragged by its title bar or card: where the press began, and whether the
    /// pointer has since gone further from there than a click may.
    presses: BTreeMap<String, (Pos2, bool)>,
}

/// The kit's own texts for moving windows, which the app gives in its language
/// ([`set_texts`]). English until it does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowTexts {
    /// The tooltip of the header button that moves the window.
    pub move_window: String,
    /// The menu item that floats the window in the main window.
    pub float: String,
    /// The menu item that opens the window in a window of its own.
    pub detach: String,
    /// The menu heading over the panel areas, and the hint on the area a window will dock in.
    pub dock_as_tab: String,
}

impl Default for WindowTexts {
    fn default() -> Self {
        WindowTexts {
            move_window: "Move this window".into(),
            float: "Float in the main window".into(),
            detach: "Open in its own window".into(),
            dock_as_tab: "Dock as tab".into(),
        }
    }
}

type Shared = Arc<Mutex<State>>;

fn state_id() -> Id {
    Id::unique("db_window_host")
}

fn with<R>(ctx: &Context, f: impl FnOnce(&mut State) -> R) -> R {
    let shared: Shared = ctx.data_mut(|d| d.get_temp_mut_or_default::<Shared>(state_id()).clone());
    let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
    f(&mut state)
}

fn now(ctx: &Context) -> u64 {
    ctx.cumulative_frame_nr_for(egui::ViewportId::ROOT)
}

/// Once a frame, before any window is drawn: the app's stored places and the kit's agree
/// afterwards. A change made in the UI goes to `places`; otherwise `places` (just loaded, or
/// changed by the app, such as a layout reset) is what the kit uses.
pub fn sync(ctx: &Context, places: &mut WindowPlaces) {
    with(ctx, |s| {
        if s.dirty {
            *places = s.places.clone();
            s.dirty = false;
        } else if s.places != *places {
            s.places = places.clone();
            // A native window no longer meant to be one opens afresh when it is one again.
            let detached: BTreeSet<String> = s.places.iter().filter(|(_, p)| p.placement == Placement::Detached).map(|(k, _)| k.clone()).collect();
            s.opened.retain(|k, _| detached.contains(k));
        }
    });
}

/// The remembered place of window `key`.
pub fn place(ctx: &Context, key: &str) -> WindowPlace {
    with(ctx, |s| s.places.get(key).cloned().unwrap_or_default())
}

/// Change the place of window `key`. A window that is docked by this comes to the front of its
/// tab group.
pub fn update(ctx: &Context, key: &str, change: impl FnOnce(&mut WindowPlace)) {
    with(ctx, |s| {
        let before = s.places.get(key).cloned().unwrap_or_default();
        let mut after = before.clone();
        change(&mut after);
        if after != before {
            if let Some(area) = after.area().filter(|a| before.area() != Some(*a)) {
                s.activate.insert(area.to_string(), key.to_string());
                // Its tab goes after those already there.
                let last = s.places.iter().filter(|(k, p)| k.as_str() != key && p.area() == Some(area)).filter_map(|(_, p)| p.tab).max();
                after.tab = Some(last.map_or(0, |t| t.saturating_add(1)));
            }
            if after.placement != Placement::Detached {
                s.opened.remove(key);
            }
            s.places.insert(key.to_string(), after);
            s.dirty = true;
        }
    });
    ctx.request_repaint();
}

/// The app's panel areas (key, label) a window can be docked in, in menu order. Once a frame.
pub fn dock_areas(ctx: &Context, areas: &[(&str, &str)]) {
    with(ctx, |s| {
        s.areas = areas.iter().map(|(k, l)| (k.to_string(), l.to_string())).collect();
    });
}

/// The texts of the header menu and the drop hint, in the app's language. Once a frame.
pub fn set_texts(ctx: &Context, texts: WindowTexts) {
    with(ctx, |s| s.texts = Some(texts));
}

/// The texts of the header menu and the drop hint: the app's, or English.
pub fn texts(ctx: &Context) -> WindowTexts {
    with(ctx, |s| s.texts.clone()).unwrap_or_default()
}

/// The known dock areas, key and label.
pub fn areas(ctx: &Context) -> Vec<(String, String)> {
    with(ctx, |s| s.areas.clone())
}

/// Area `area` is on screen at `rect` this frame: a window dragged onto it is docked there.
pub fn dock_target(ctx: &Context, area: &str, rect: Rect) {
    let frame = now(ctx);
    with(ctx, |s| {
        s.targets.insert(area.to_string(), Target { rect, frame });
    });
}

/// The dock area under `pos`, of those on screen this frame or the last.
pub fn target_at(ctx: &Context, pos: Pos2) -> Option<String> {
    let frame = now(ctx);
    with(ctx, |s| s.targets.iter().find(|(_, t)| t.frame + 1 >= frame && t.rect.contains(pos)).map(|(k, _)| k.clone()))
}

/// Where area `area` is on screen, if it was drawn this frame or the last.
pub fn target_rect(ctx: &Context, area: &str) -> Option<Rect> {
    let frame = now(ctx);
    with(ctx, |s| s.targets.get(area).filter(|t| t.frame + 1 >= frame).map(|t| t.rect))
}

/// The windows docked in `area` that are open (shown this frame or the last), in the order
/// they were docked there (the stored order, so it is the same after a restart).
pub fn docked_tabs(ctx: &Context, area: &str) -> Vec<DockedTab> {
    let frame = now(ctx);
    with(ctx, |s| {
        let mut tabs: Vec<(u32, DockedTab)> = s
            .shown
            .iter()
            .filter(|(k, shown)| shown.frame + 1 >= frame && s.places.get(*k).and_then(WindowPlace::area) == Some(area))
            .map(|(k, shown)| (s.places.get(k).and_then(|p| p.tab).unwrap_or(0), DockedTab { key: k.clone(), title: shown.title.clone() }))
            .collect();
        // Stable: windows with the same place (stored before tabs had one) keep key order.
        tabs.sort_by_key(|(tab, _)| *tab);
        tabs.into_iter().map(|(_, t)| t).collect()
    })
}

/// Every window drawn through the kit as a dockable window this frame or the last, wherever it
/// is: (key, title), in key order.
pub fn open_windows(ctx: &Context) -> Vec<(String, String)> {
    let frame = now(ctx);
    with(ctx, |s| s.shown.iter().filter(|(_, shown)| shown.frame + 1 >= frame).map(|(k, shown)| (k.clone(), shown.title.clone())).collect())
}

/// The tab group of `area` has window `key` in front (none: a panel), with this body rectangle
/// on `layer`. The other windows docked there draw nothing.
pub fn set_front(ctx: &Context, area: &str, key: Option<&str>, rect: Rect, layer: LayerId) {
    let frame = now(ctx);
    with(ctx, |s| {
        let places = &s.places;
        s.slots.retain(|k, _| places.get(k).and_then(WindowPlace::area) != Some(area));
        if let Some(key) = key {
            s.slots.insert(key.to_string(), Slot { rect, layer, frame });
        }
    });
}

/// The user closed window `key` from its tab: it reports itself closed the next time it is drawn.
pub fn request_close(ctx: &Context, key: &str) {
    with(ctx, |s| {
        s.close.insert(key.to_string());
    });
    ctx.request_repaint();
}

/// Bring window `key` into view, for a command that opens it: when it is open already, behind
/// another tab, under the main window or minimised, setting its open flag again shows nothing.
/// A docked window's tab comes to the front, a floating card comes on top, a native window is
/// restored. The system focus only moves while the app has it, so a command an agent runs does
/// not take the focus from another program.
pub fn raise(ctx: &Context, key: &str) {
    let frame = now(ctx);
    with(ctx, |s| {
        if let Some(area) = s.places.get(key).and_then(WindowPlace::area) {
            s.activate.insert(area.to_string(), key.to_string());
        }
        s.raise.insert(key.to_string(), frame);
    });
    ctx.request_repaint();
}

/// Whether window `key` was just asked to come into view ([`raise`]); once.
pub(crate) fn take_raise(ctx: &Context, key: &str) -> bool {
    let frame = now(ctx);
    with(ctx, |s| s.raise.remove(key).is_some_and(|asked| asked + 2 >= frame))
}

/// Whether the native window of `key` has the system focus, as it reports when drawn.
pub(crate) fn native_focus(ctx: &Context, key: &str, focused: bool) {
    let frame = now(ctx);
    with(ctx, |s| {
        if focused {
            s.native_focus.insert(key.to_string(), frame);
        } else {
            s.native_focus.remove(key);
        }
    });
}

fn native_window_focused(ctx: &Context) -> bool {
    let frame = now(ctx);
    with(ctx, |s| s.native_focus.values().any(|f| f + 1 >= frame))
}

fn main_window_focused(ctx: &Context) -> bool {
    ctx.input_for(egui::ViewportId::ROOT, |i| i.viewport().focused == Some(true))
}

/// Whether the app has the system focus: its main window or one of its own windows.
pub(crate) fn app_focused(ctx: &Context) -> bool {
    main_window_focused(ctx) || native_window_focused(ctx)
}

/// A window raised in the main window ([`raise`]) while one of the app's own windows has the
/// focus: the main window comes up too, or the window stays behind the one the command came from.
pub(crate) fn focus_main_window(ctx: &Context) {
    if !main_window_focused(ctx) && native_window_focused(ctx) {
        ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Focus);
    }
}

/// A window that was just docked in `area`, or opened there, which should come to the front.
pub fn take_activation(ctx: &Context, area: &str) -> Option<String> {
    with(ctx, |s| s.activate.remove(area))
}

/// Called by the window each time it is drawn, whatever its placement.
pub(crate) fn shown(ctx: &Context, key: &str, title: &str) {
    let frame = now(ctx);
    with(ctx, |s| {
        let was_open = s.shown.get(key).is_some_and(|x| x.frame + 1 >= frame);
        if !was_open {
            s.close.remove(key);
            s.opened.remove(key);
            if let Some(area) = s.places.get(key).and_then(WindowPlace::area) {
                s.activate.insert(area.to_string(), key.to_string());
            }
        }
        s.shown.insert(key.to_string(), Shown { title: title.to_string(), frame });
    });
}

pub(crate) fn take_close(ctx: &Context, key: &str) -> bool {
    with(ctx, |s| s.close.remove(key))
}

/// The body a docked window draws into: this frame's or the last's.
pub(crate) fn slot(ctx: &Context, key: &str) -> Option<(Rect, LayerId)> {
    let frame = now(ctx);
    with(ctx, |s| s.slots.get(key).filter(|x| x.frame + 1 >= frame).map(|x| (x.rect, x.layer)))
}

/// Whether area `area` is known and on screen, so a window docked there can show.
pub(crate) fn area_on_screen(ctx: &Context, area: &str) -> bool {
    // An area the app offers but does not show now (all its panels hidden) opens for the tab.
    target_rect(ctx, area).is_some() || with(ctx, |s| s.areas.iter().any(|(a, _)| a == area))
}

/// The size and position a native window opens with: what was stored when it opened (no size
/// stored yet: `first_size`), kept the same while it is open so the system's own moves are not
/// fought. A stored position that is no longer on a screen near the main window (a monitor since
/// unplugged) is left to the system, which opens the window where it can be seen; a stored size
/// is kept to the monitor's.
pub(crate) fn native_opening(ctx: &Context, key: &str, first_size: Vec2) -> ([f32; 2], Option<[f32; 2]>) {
    let (main, monitor) = ctx.input(|i| (i.viewport().outer_rect, i.viewport().monitor_size));
    with(ctx, |s| {
        let stored = s.places.get(key).map(|p| (p.native_size, p.native_pos)).unwrap_or_default();
        let size = stored.0.map(|v| match monitor.filter(|m| m.x > 0.0 && m.y > 0.0) {
            Some(m) => clamp_size([v[0].min(m.x), v[1].min(m.y)]),
            None => v,
        });
        let pos = stored.1.filter(|p| native_pos_reachable(main, monitor, *p));
        *s.opened.entry(key.to_string()).or_insert((size.unwrap_or([first_size.x, first_size.y]), pos))
    })
}

/// Whether a native window's stored outer position `pos` is still where it can be seen: on the
/// main window's monitor or on one beside it. The system does not say where its other monitors
/// are, so "beside" is one monitor's width or height away from the main window. With the main
/// window or its monitor unknown, the position is trusted.
pub fn native_pos_reachable(main: Option<Rect>, monitor: Option<Vec2>, pos: [f32; 2]) -> bool {
    if !finite(pos) {
        return false;
    }
    match (main, monitor) {
        (Some(main), Some(m)) if main.is_finite() && m.x > 0.0 && m.y > 0.0 => {
            // The title bar must be reachable: the corner at least a little inside that room.
            let room = Rect::from_min_max(main.min - m, main.max + m).shrink2(vec2(MIN_SIZE.x * 0.25, 8.0));
            room.contains(Pos2::new(pos[0] + MIN_SIZE.x * 0.25, pos[1]))
        }
        _ => true,
    }
}

/// The native window of `key` is now `size` (inner) at `pos` (outer, if known).
pub(crate) fn native_moved(ctx: &Context, key: &str, size: Option<Vec2>, pos: Option<Pos2>) {
    let changed = with(ctx, |s| {
        let place = s.places.entry(key.to_string()).or_default();
        let size = size.filter(|v| v.x.is_finite() && v.y.is_finite() && v.x > 0.0 && v.y > 0.0).map(|v| clamp_size([v.x, v.y]));
        let pos = pos.filter(|p| p.x.is_finite() && p.y.is_finite()).map(|p| [p.x, p.y]);
        let changed = (size.is_some() && size != place.native_size) || (pos.is_some() && pos != place.native_pos);
        if changed {
            place.native_size = size.or(place.native_size);
            place.native_pos = pos.or(place.native_pos);
            s.dirty = true;
        }
        changed
    });
    if changed {
        note_native_input(ctx);
    }
}

/// Input that can change a setting: a click, a key, typed text, a wheel or a pinch.
pub fn changes_settings(event: &egui::Event) -> bool {
    matches!(
        event,
        egui::Event::PointerButton { .. }
            | egui::Event::Key { .. }
            | egui::Event::Text(_)
            | egui::Event::Paste(_)
            | egui::Event::MouseWheel { .. }
            | egui::Event::Zoom(_)
    )
}

/// In a native window's pass: notes input there that can change a setting. The main window's
/// input never shows it, so the app asks [`take_native_input`] as well.
pub(crate) fn native_input(ctx: &Context) {
    if ctx.input(|i| i.events.iter().any(changes_settings)) {
        note_native_input(ctx);
    }
}

fn note_native_input(ctx: &Context) {
    with(ctx, |s| s.native_input = true);
    // The main window runs next, to hear of it.
    ctx.request_repaint_of(egui::ViewportId::ROOT);
}

/// Whether a native window had input that can change a setting, or was moved or resized, since
/// the last call: what was changed there is the app's to store, as for input in the main window.
pub fn take_native_input(ctx: &Context) -> bool {
    with(ctx, |s| std::mem::take(&mut s.native_input))
}

/// At the end of a native window's pass: the key presses nothing in the window used are kept for
/// the app ([`take_window_keys`]). None while a control there has the keyboard: a field takes the
/// letters typed into it without using them up.
pub(crate) fn pass_on_keys(ctx: &Context) {
    if ctx.memory(|m| m.focused().is_some()) {
        return;
    }
    let keys: Vec<egui::Event> =
        ctx.input(|i| i.events.iter().filter(|e| matches!(e, egui::Event::Key { pressed: true, repeat: false, .. })).cloned().collect());
    if keys.is_empty() {
        return;
    }
    with(ctx, |s| {
        s.keys.extend(keys);
        let over = s.keys.len().saturating_sub(MAX_KEYS);
        s.keys.drain(..over);
    });
    ctx.request_repaint_of(egui::ViewportId::ROOT);
}

/// The key presses made in the app's native windows since the last call that nothing in them
/// used, oldest first. The system gives the keyboard to the window that has the focus, so the
/// main window never sees these: the app runs its shortcuts on them too, or Pause and Stop
/// would be dead while a window of its own has the focus. A held key counts once.
pub fn take_window_keys(ctx: &Context) -> Vec<egui::Event> {
    with(ctx, |s| std::mem::take(&mut s.keys))
}

/// The floating card of `key` was dragged to `pos` (top-left).
pub(crate) fn moved(ctx: &Context, key: &str, pos: Pos2) {
    with(ctx, |s| {
        let place = s.places.entry(key.to_string()).or_default();
        if place.pos != Some([pos.x, pos.y]) && pos.x.is_finite() && pos.y.is_finite() {
            place.pos = Some([pos.x, pos.y]);
            s.dirty = true;
        }
    });
}

/// Mark `rect`, a panel area, as where the window being dragged will dock.
pub fn paint_drop_hint(ctx: &Context, rect: Rect) {
    let t = solidcraft_theme::Tokens::get();
    let text = texts(ctx).dock_as_tab;
    let painter = ctx.layer_painter(LayerId::new(egui::Order::Foreground, Id::unique("db_dock_drop_hint")));
    painter.rect(rect.shrink(2.0), t.radius_sm, t.accent.gamma_multiply(0.18), egui::Stroke::new(2.0, t.accent), egui::StrokeKind::Inside);
    painter.text(rect.center(), egui::Align2::CENTER_CENTER, text, crate::fonts::semibold(t.font_heading), t.text);
    solidcraft_ui_registry::publish_rect("dialog:dock_hint", rect);
}

/// The header button that moves the window: back into the main window, into a window of its
/// own, or into a panel area as a tab.
pub(crate) fn place_button(ui: &mut egui::Ui, rect: Rect, key: &str) {
    let texts = texts(ui.ctx());
    let mut pick: Option<Placement> = None;
    super::Menu::button_with(
        ui,
        |ui, _| {
            let response = super::icon_button_at(ui, rect, "prefs_layout", &texts.move_window);
            solidcraft_ui_registry::publish_target("dialog:place", response.rect);
            response
        },
        |ui| pick = place_items(ui, key),
    );
    apply_pick(ui.ctx(), key, pick);
}

/// The same choices as the header's place button, for a window with a title bar of its own
/// (the caller shows them as the title bar's context menu).
pub fn place_menu(ui: &mut egui::Ui, key: &str) {
    let pick = place_items(ui, key);
    apply_pick(ui.ctx(), key, pick);
}

fn place_items(ui: &mut egui::Ui, key: &str) -> Option<Placement> {
    let place = place(ui.ctx(), key);
    let areas = areas(ui.ctx());
    let texts = texts(ui.ctx());
    let mut pick: Option<Placement> = None;
    ui.set_min_width(super::LIST_W);
    if place.placement != Placement::Floating && super::MenuItem::new("window.float", &texts.float, "").show(ui).clicked() {
        pick = Some(Placement::Floating);
    }
    if place.placement != Placement::Detached && super::MenuItem::new("window.detach", &texts.detach, "").show(ui).clicked() {
        pick = Some(Placement::Detached);
    }
    if !areas.is_empty() {
        super::Menu::separator(ui);
        super::Menu::heading(ui, &texts.dock_as_tab);
    }
    for (area, label) in &areas {
        if place.area() == Some(area.as_str()) {
            continue;
        }
        let id = format!("window.dock.{area}");
        if super::MenuItem::new(&id, label, "").show(ui).clicked() {
            pick = Some(Placement::Docked(area.clone()));
        }
    }
    pick
}

fn apply_pick(ctx: &Context, key: &str, pick: Option<Placement>) {
    if let Some(to) = pick {
        update(ctx, key, |p| match to {
            Placement::Floating => p.float(None),
            Placement::Detached => p.detach(),
            Placement::Docked(area) => p.dock(&area),
        });
        egui::Popup::close_all(ctx);
    }
}

/// A window with a title bar of its own is being dragged (`response` is its move response):
/// over a panel area, show that it will dock there; dropped there, it becomes a tab.
pub fn dock_on_drop(ctx: &Context, key: &str, response: &egui::Response) {
    if !moved_since_press(ctx, key, response) {
        return;
    }
    let Some(area) = ctx.input(|i| i.pointer.latest_pos()).and_then(|p| target_at(ctx, p)) else { return };
    if response.drag_stopped() {
        update(ctx, key, |p| p.dock(&area));
        return;
    }
    if let Some(rect) = target_rect(ctx, &area) {
        paint_drop_hint(ctx, rect);
    }
}

/// The window is being dragged and the pointer has gone further than a click's wobble from where
/// its button went down: a press and release on a button in a window (its Cancel, say) is not a
/// drop, however long the button was held. (egui counts a press held still for long as a drag,
/// and forgets where it began on the frame the button is let go, so the start is kept here.)
fn moved_since_press(ctx: &Context, key: &str, response: &egui::Response) -> bool {
    let dragging = response.dragged() || response.drag_stopped();
    let (origin, latest) = ctx.input(|i| (i.pointer.press_origin(), i.pointer.latest_pos()));
    let wobble = ctx.options(|o| o.input_options.max_click_dist);
    with(ctx, |s| {
        if !dragging {
            s.presses.remove(key);
            return false;
        }
        // A new press starts afresh, also when the end of the last one was not seen.
        let stale = s.presses.get(key).is_none_or(|(from, _)| origin.is_some_and(|o| o != *from));
        if (response.drag_started() || stale)
            && let Some(from) = origin.or(latest)
        {
            s.presses.insert(key.to_string(), (from, false));
        }
        let moved = match s.presses.get_mut(key) {
            Some((from, moved)) => {
                *moved |= latest.is_some_and(|p| p.distance(*from) > wobble);
                *moved
            }
            None => false,
        };
        if response.drag_stopped() {
            s.presses.remove(key);
        }
        moved
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(ctx: &Context, add: impl FnMut(&mut egui::Ui)) {
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1200.0, 800.0))), ..Default::default() };
        let mut out = ctx.run_ui(input, add);
        out.textures_delta.clear();
    }

    #[test]
    fn floating_docked_detached_and_back() {
        let mut p = WindowPlace::default();
        assert_eq!(p.placement, Placement::Floating);
        p.resize(vec2(500.0, 400.0));
        p.float(Some(Pos2::new(40.0, 50.0)));
        p.dock("RightTop");
        assert_eq!(p.area(), Some("RightTop"));
        // Docking keeps the floating size and position for later.
        assert_eq!((p.size, p.pos), (Some([500.0, 400.0]), Some([40.0, 50.0])));
        p.detach();
        assert_eq!((p.placement.clone(), p.area()), (Placement::Detached, None));
        p.dock("Left");
        p.float(None);
        assert_eq!(p.placement, Placement::Floating);
        assert_eq!(p.pos, Some([40.0, 50.0]), "floating again without a drop point goes back where it was");
        p.dock("");
        assert_eq!(p.placement, Placement::Floating, "an empty area is not a dock");
    }

    #[test]
    fn sizes_are_held_to_the_limits() {
        let mut p = WindowPlace::default();
        p.resize(vec2(10.0, 10.0));
        assert_eq!(p.size, Some([MIN_SIZE.x, MIN_SIZE.y]));
        p.resize(vec2(f32::NAN, 300.0));
        assert_eq!(p.size, Some([MIN_SIZE.x, MIN_SIZE.y]), "a size that is not a number is ignored");
        p.resize(vec2(1.0e9, 400.0));
        assert_eq!(p.size, Some([MAX_SIZE.x, 400.0]));
    }

    #[test]
    fn stored_places_round_trip_and_are_cleaned() {
        let mut places = WindowPlaces::new();
        let mut a = WindowPlace::default();
        a.dock("Bottom");
        a.resize(vec2(640.0, 480.0));
        a.native_size = Some([800.0, 600.0]);
        a.native_pos = Some([100.0, 120.0]);
        places.insert("db_devices".into(), a);
        let mut b = WindowPlace::default();
        b.detach();
        places.insert("settings".into(), b);
        let json = serde_json::to_string(&places).expect("serialises");
        let back: WindowPlaces = serde_json::from_str(&json).expect("reads back");
        assert_eq!(back, places);

        let odd = r#"{"":{"placement":"floating"},"x":{"placement":{"docked":""},"pos":[1e30,0],"size":[1,2],"native_size":[5000,1e12]},
            "y":{"placement":"something_new","size":[400,300]}}"#;
        let read = lenient(&mut serde_json::Deserializer::from_str(odd)).expect("reads leniently");
        assert!(!read.contains_key(""));
        let x = &read["x"];
        assert_eq!(x.placement, Placement::Floating);
        assert_eq!(x.pos, None);
        assert_eq!(x.size, Some([MIN_SIZE.x, MIN_SIZE.y]));
        assert_eq!(x.native_size, Some([5000.0, MAX_SIZE.y]));
        assert_eq!(read["y"], WindowPlace::default(), "an entry this version cannot read falls back to a floating card");
    }

    #[test]
    fn too_many_windows_are_trimmed() {
        let mut places: WindowPlaces = (0..MAX_WINDOWS + 10).map(|i| (format!("w{i:04}"), WindowPlace::default())).collect();
        sanitise(&mut places);
        assert_eq!(places.len(), MAX_WINDOWS);
    }

    #[test]
    fn tab_ids_name_their_window() {
        assert_eq!(window_of_tab(&tab_id("db_devices")), Some("db_devices"));
        assert_eq!(window_of_tab("cuts"), None);
    }

    #[test]
    fn sync_takes_ui_changes_and_otherwise_follows_the_app() {
        let ctx = Context::default();
        let mut stored = WindowPlaces::new();
        let mut loaded = WindowPlace::default();
        loaded.dock("Left");
        stored.insert("a".into(), loaded.clone());
        sync(&ctx, &mut stored);
        assert_eq!(place(&ctx, "a"), loaded, "the app's stored places are what the kit uses");
        update(&ctx, "a", |p| p.detach());
        sync(&ctx, &mut stored);
        assert_eq!(stored["a"].placement, Placement::Detached, "a change in the UI goes to the app");
        stored.clear();
        sync(&ctx, &mut stored);
        assert_eq!(place(&ctx, "a"), WindowPlace::default(), "the app reset its layout");
    }

    #[test]
    fn docked_windows_show_as_tabs_while_open_and_come_to_the_front() {
        let ctx = Context::default();
        update(&ctx, "a", |p| p.dock("RightTop"));
        assert_eq!(take_activation(&ctx, "RightTop").as_deref(), Some("a"), "docking brings the tab to the front");
        frame(&ctx, |ui| shown(ui.ctx(), "a", "Window A"));
        assert_eq!(take_activation(&ctx, "RightTop").as_deref(), Some("a"), "opening a docked window brings it to the front");
        frame(&ctx, |ui| {
            assert_eq!(docked_tabs(ui.ctx(), "RightTop"), vec![DockedTab { key: "a".into(), title: "Window A".into() }]);
            assert!(docked_tabs(ui.ctx(), "Left").is_empty());
            shown(ui.ctx(), "a", "Window A");
        });
        assert_eq!(take_activation(&ctx, "RightTop"), None, "a window already open stays where it is in the order");
        // Not drawn for two frames: closed, its tab goes.
        frame(&ctx, |_| {});
        frame(&ctx, |ui| assert!(docked_tabs(ui.ctx(), "RightTop").is_empty()));
    }

    #[test]
    fn the_drop_hint_and_header_menu_use_the_texts_the_app_gives() {
        let ctx = Context::default();
        assert_eq!(texts(&ctx), WindowTexts::default(), "English until the app gives its texts");
        assert_eq!(texts(&ctx).dock_as_tab, "Dock as tab");
        let given =
            WindowTexts { move_window: "Flytt".into(), float: "Flyt".into(), detach: "Eget vindu".into(), dock_as_tab: "Fest som fane".into() };
        set_texts(&ctx, given.clone());
        assert_eq!(texts(&ctx), given);
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1200.0, 800.0))), ..Default::default() };
        let mut out = ctx.run_ui(input, |ui| paint_drop_hint(ui.ctx(), Rect::from_min_size(Pos2::new(20.0, 20.0), vec2(400.0, 300.0))));
        out.textures_delta.clear();
        let painted: Vec<String> = out
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Text(t) => Some(t.galley.text().to_string()),
                _ => None,
            })
            .collect();
        assert!(painted.iter().any(|t| t == "Fest som fane"), "{painted:?}");
        assert!(!painted.iter().any(|t| t == "Dock as tab"), "{painted:?}");
    }

    #[test]
    fn close_from_the_tab_reaches_the_window_once() {
        let ctx = Context::default();
        frame(&ctx, |ui| shown(ui.ctx(), "a", "A"));
        request_close(&ctx, "a");
        assert!(take_close(&ctx, "a"));
        assert!(!take_close(&ctx, "a"));
    }

    #[test]
    fn targets_and_slots_last_one_frame() {
        let ctx = Context::default();
        let r = Rect::from_min_size(Pos2::new(900.0, 40.0), vec2(300.0, 400.0));
        frame(&ctx, |ui| {
            dock_target(ui.ctx(), "RightTop", r);
            update(ui.ctx(), "a", |p| p.dock("RightTop"));
            set_front(ui.ctx(), "RightTop", Some("a"), r, ui.layer_id());
        });
        frame(&ctx, |ui| {
            assert_eq!(target_at(ui.ctx(), Pos2::new(950.0, 100.0)).as_deref(), Some("RightTop"));
            assert_eq!(target_at(ui.ctx(), Pos2::new(10.0, 100.0)), None);
            assert!(slot(ui.ctx(), "a").is_some() && area_on_screen(ui.ctx(), "RightTop"));
        });
        frame(&ctx, |_| {});
        frame(&ctx, |ui| {
            assert_eq!(target_at(ui.ctx(), Pos2::new(950.0, 100.0)), None, "an area no longer drawn takes no drops");
            assert!(slot(ui.ctx(), "a").is_none());
            set_front(ui.ctx(), "RightTop", Some("a"), r, ui.layer_id());
            assert!(slot(ui.ctx(), "a").is_some());
            set_front(ui.ctx(), "RightTop", None, r, ui.layer_id());
            assert!(slot(ui.ctx(), "a").is_none(), "a panel in front: the window draws nothing");
        });
    }

    #[test]
    fn a_click_inside_a_floating_window_over_a_panel_area_does_not_dock_it() {
        use egui::{Event, Modifiers, PointerButton};
        let ctx = Context::default();
        crate::fonts::setup_bundled_fonts(&ctx);
        let button = std::cell::Cell::new(Rect::NOTHING);
        let clicks = std::cell::Cell::new(0);
        let draw = |events: Vec<Event>| {
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1200.0, 800.0))), events, ..Default::default() };
            let mut out = ctx.run_ui(input, |ui| {
                // A panel area under the whole window, as when a card is opened over the side panel.
                dock_target(ui.ctx(), "RightTop", ui.ctx().content_rect());
                super::super::Window::new(Id::unique("w"), "", "W").dockable("w").show(ui.ctx(), true, |ui| {
                    let r = ui.button("Press");
                    button.set(r.rect);
                    if r.clicked() {
                        clicks.set(clicks.get() + 1);
                    }
                });
            });
            out.textures_delta.clear();
        };
        let press = |pos: Pos2, pressed: bool| Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
        for _ in 0..3 {
            draw(vec![]);
        }
        let at = button.get().center();
        assert!(button.get().is_positive(), "the window is drawn");
        draw(vec![Event::PointerMoved(at), press(at, true)]);
        draw(vec![press(at, false)]);
        draw(vec![]);
        assert_eq!(clicks.get(), 1);
        assert_eq!(place(&ctx, "w").placement, Placement::Floating, "a click is not a drop");
        assert_eq!(place(&ctx, "w").pos, None, "nor a move");

        // Dragged a little way and dropped over the same area: now it docks there.
        draw(vec![Event::PointerMoved(at), press(at, true)]);
        for step in 1..=4 {
            draw(vec![Event::PointerMoved(at + vec2(25.0 * step as f32, 0.0))]);
        }
        draw(vec![press(at + vec2(100.0, 0.0), false)]);
        assert_eq!(clicks.get(), 1, "a drag is not a click");
        assert_eq!(place(&ctx, "w").area(), Some("RightTop"));
    }

    #[test]
    fn a_native_window_keeps_its_opening_size_while_open() {
        let ctx = Context::default();
        update(&ctx, "a", |p| {
            p.detach();
            p.native_size = Some([640.0, 480.0]);
        });
        let first = vec2(400.0, 300.0);
        assert_eq!(native_opening(&ctx, "a", first), ([640.0, 480.0], None));
        native_moved(&ctx, "a", Some(vec2(700.0, 500.0)), Some(Pos2::new(10.0, 20.0)));
        assert_eq!(native_opening(&ctx, "a", first), ([640.0, 480.0], None), "the system's own resize is not sent back");
        assert_eq!(place(&ctx, "a").native_size, Some([700.0, 500.0]), "but it is remembered");
        // Closed (not drawn for two frames) and opened again: the next opening uses it.
        frame(&ctx, |ui| shown(ui.ctx(), "a", "A"));
        frame(&ctx, |_| {});
        frame(&ctx, |_| {});
        frame(&ctx, |ui| shown(ui.ctx(), "a", "A"));
        assert_eq!(native_opening(&ctx, "a", first), ([700.0, 500.0], Some([10.0, 20.0])));
        // Nothing stored: the first size, the same while it is open.
        update(&ctx, "b", |p| p.detach());
        assert_eq!(native_opening(&ctx, "b", first), ([400.0, 300.0], None));
        assert_eq!(native_opening(&ctx, "b", vec2(900.0, 900.0)), ([400.0, 300.0], None));
    }

    type Keys = std::rc::Rc<std::cell::RefCell<Vec<egui::Event>>>;
    type Info = std::rc::Rc<std::cell::RefCell<egui::ViewportInfo>>;
    /// The size each native window was asked to have, in the order they were drawn.
    type Opened = std::rc::Rc<std::cell::RefCell<Vec<(egui::ViewportId, Option<Vec2>)>>>;

    /// Native windows run the way a desktop backend runs them: each in a pass of its own, with
    /// input of its own, which here is what `keys` holds when the window is drawn.
    fn native_windows(keys: &Keys) {
        native_windows_with(keys, &Info::default(), &Opened::default());
    }

    /// As [`native_windows`], each window saying of itself what `info` holds (focus, minimised)
    /// and `opened` recording the size it asks for.
    fn native_windows_with(keys: &Keys, info: &Info, opened: &Opened) {
        let keys = keys.clone();
        let info = info.clone();
        let opened = opened.clone();
        Context::set_immediate_viewport_renderer(move |ctx, mut viewport| {
            opened.borrow_mut().push((viewport.ids.this, viewport.builder.inner_size));
            let events = std::mem::take(&mut *keys.borrow_mut());
            let screen_rect = Some(Rect::from_min_size(Pos2::ZERO, vec2(600.0, 500.0)));
            let info = egui::ViewportInfo { parent: Some(viewport.ids.parent), ..info.borrow().clone() };
            let viewports = std::iter::once((viewport.ids.this, info)).collect();
            let input = egui::RawInput { viewport_id: viewport.ids.this, viewports, screen_rect, events, ..Default::default() };
            let mut out = ctx.run_ui(input, |ui| (viewport.viewport_ui_cb)(ui));
            out.textures_delta.clear();
        });
    }

    fn key(key: egui::Key, modifiers: egui::Modifiers, pressed: bool) -> egui::Event {
        egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers }
    }

    /// Detached window "w", with a text field that holds the keyboard when `field` is set.
    fn detached_window(ctx: &Context, field: bool) {
        frame(ctx, |ui| {
            super::super::Window::new(Id::unique("w"), "", "W").dockable("w").show(ui.ctx(), true, |ui| {
                if field {
                    let mut text = String::new();
                    ui.text_edit_singleline(&mut text).request_focus();
                }
            });
        });
    }

    #[test]
    fn keys_nothing_in_a_native_window_used_go_to_the_app() {
        use egui::{Key, Modifiers};
        let keys = Keys::default();
        native_windows(&keys);
        let ctx = Context::default();
        crate::fonts::setup_bundled_fonts(&ctx);
        ctx.set_embed_viewports(false);
        update(&ctx, "w", |p| p.detach());
        detached_window(&ctx, false);
        assert!(take_window_keys(&ctx).is_empty());
        let stop = key(Key::Pause, Modifiers::COMMAND, true);
        let pause = key(Key::Pause, Modifiers::NONE, true);
        *keys.borrow_mut() = vec![stop.clone(), key(Key::Pause, Modifiers::COMMAND, false), pause.clone()];
        detached_window(&ctx, false);
        assert_eq!(take_window_keys(&ctx), vec![stop, pause.clone()], "the window's own keys reach the app");
        assert!(take_window_keys(&ctx).is_empty(), "once");
        // Pause is still held: the system sends it again, as a repeat.
        *keys.borrow_mut() = vec![pause];
        detached_window(&ctx, false);
        assert!(take_window_keys(&ctx).is_empty(), "a held key counts once");
        *keys.borrow_mut() = vec![key(Key::Pause, Modifiers::NONE, false)];
        detached_window(&ctx, true);
        *keys.borrow_mut() = vec![key(Key::S, Modifiers::COMMAND, true)];
        detached_window(&ctx, true);
        assert!(take_window_keys(&ctx).is_empty(), "keys typed into a field are the field's");
    }

    #[test]
    fn opening_a_window_that_is_open_already_brings_it_into_view() {
        use egui::{Order, ViewportCommand, ViewportId, ViewportInfo};
        let keys = Keys::default();
        let info = Info::default();
        native_windows_with(&keys, &info, &Opened::default());
        let ctx = Context::default();
        crate::fonts::setup_bundled_fonts(&ctx);
        ctx.set_embed_viewports(false);
        update(&ctx, "docked", |p| p.dock("RightTop"));
        update(&ctx, "native", |p| p.detach());
        let native = ViewportId::from_hash_of(("db_window", "native"));
        let card = std::cell::Cell::new(None);
        // One pass of the main window, which has the system focus or not: what it asks of the
        // main window and of the native one.
        let draw = |main_focused: bool| {
            let viewports = std::iter::once((ViewportId::ROOT, ViewportInfo { focused: Some(main_focused), ..Default::default() })).collect();
            let screen_rect = Some(Rect::from_min_size(Pos2::ZERO, vec2(1200.0, 800.0)));
            let mut out = ctx.run_ui(egui::RawInput { screen_rect, viewports, ..Default::default() }, |ui| {
                dock_target(ui.ctx(), "RightTop", Rect::from_min_size(Pos2::new(900.0, 40.0), vec2(300.0, 400.0)));
                for key in ["docked", "native", "card", "other"] {
                    super::super::Window::new(Id::unique(key), "", key).dockable(key).show(ui.ctx(), true, |ui| {
                        if key == "card" {
                            card.set(Some(ui.layer_id()));
                        }
                    });
                }
            });
            out.textures_delta.clear();
            let commands = |id| out.viewport_output.get(&id).map(|o| o.commands.clone()).unwrap_or_default();
            (commands(ViewportId::ROOT), commands(native))
        };
        let top = || ctx.memory(|m| m.areas().top_layer_id(Order::Middle));
        draw(true);
        draw(true);
        while take_activation(&ctx, "RightTop").is_some() {}
        let (main, own) = draw(true);
        assert!(main.is_empty() && own.is_empty(), "nothing asked, nothing moves: {main:?} {own:?}");
        assert_ne!(top(), card.get(), "the other card was opened after this one");

        // Another program has the focus: the tab still comes to the front, the focus stays put.
        *info.borrow_mut() = ViewportInfo { focused: Some(false), minimized: Some(true), ..Default::default() };
        for key in ["docked", "native", "card"] {
            raise(&ctx, key);
        }
        assert_eq!(take_activation(&ctx, "RightTop").as_deref(), Some("docked"), "a docked window's tab comes to the front");
        let (main, own) = draw(false);
        assert!(main.is_empty() && own.is_empty(), "the focus is not taken from another program: {main:?} {own:?}");
        assert_eq!(top(), card.get(), "the card comes on top of the other");

        // The main window has the focus: the minimised window of its own is restored and focused.
        raise(&ctx, "native");
        let (_, own) = draw(true);
        assert_eq!(own, vec![ViewportCommand::Minimized(false), ViewportCommand::Focus]);
        let (_, own) = draw(true);
        assert!(own.is_empty(), "once");

        // A command in the window of its own opens a window in the main one: the main window comes up.
        *info.borrow_mut() = ViewportInfo { focused: Some(true), minimized: Some(false), ..Default::default() };
        draw(false);
        for key in ["card", "docked"] {
            raise(&ctx, key);
            let (main, own) = draw(false);
            assert_eq!(main, vec![ViewportCommand::Focus], "{key}");
            assert!(own.is_empty(), "{key}");
        }
        raise(&ctx, "native");
        let (main, own) = draw(false);
        assert!(main.is_empty() && own.is_empty(), "a window that has the focus already stays as it is: {main:?} {own:?}");
    }

    #[test]
    fn a_window_of_its_own_first_opens_at_its_card_size_held_to_the_screen() {
        use egui::{ViewportId, ViewportInfo};
        let opened = Opened::default();
        native_windows_with(&Keys::default(), &Info::default(), &opened);
        let ctx = Context::default();
        crate::fonts::setup_bundled_fonts(&ctx);
        ctx.set_embed_viewports(false);
        // The main window is on a 1536 x 864 screen; returns the size the window asked for.
        let draw = |key: &str, cards: f32| {
            let main = ViewportInfo { monitor_size: Some(vec2(1536.0, 864.0)), ..Default::default() };
            let viewports = std::iter::once((ViewportId::ROOT, main)).collect();
            let screen_rect = Some(Rect::from_min_size(Pos2::ZERO, vec2(1200.0, 800.0)));
            let mut out = ctx.run_ui(egui::RawInput { screen_rect, viewports, ..Default::default() }, |ui| {
                super::super::Window::new(Id::unique(key), "", key).cards(cards).dockable(key).show(ui.ctx(), true, |ui| {
                    ui.label(key);
                });
            });
            out.textures_delta.clear();
            let id = ViewportId::from_hash_of(("db_window", key));
            opened.borrow().iter().rev().find(|(v, _)| *v == id).and_then(|(_, size)| *size)
        };
        let most = vec2(1536.0, 864.0) * 0.85;

        // A wide window popped out for the first time fits on the screen, footer and all.
        update(&ctx, "wide", |p| p.detach());
        let size = draw("wide", 16.0).expect("opened");
        assert!(size.x <= most.x && size.y <= most.y, "{size:?}");
        assert!(size.x >= MIN_SIZE.x && size.y >= MIN_SIZE.y, "{size:?}");

        // A card the user gave a size opens at that size.
        update(&ctx, "sized", |p| p.resize(vec2(500.0, 400.0)));
        draw("sized", 1.0);
        update(&ctx, "sized", |p| p.detach());
        assert_eq!(draw("sized", 1.0), Some(vec2(500.0, 400.0)));

        // A card left at its own size opens at the size it was drawn at.
        draw("drawn", 1.0);
        draw("drawn", 1.0);
        let card = egui::AreaState::load(&ctx, Id::unique("drawn")).and_then(|a| a.size).expect("drawn");
        update(&ctx, "drawn", |p| p.detach());
        assert_eq!(draw("drawn", 1.0), Some(card.max(MIN_SIZE)));
    }

    #[test]
    fn input_in_a_native_window_is_passed_to_the_app_once() {
        use egui::{Key, Modifiers};
        let keys = Keys::default();
        native_windows(&keys);
        let ctx = Context::default();
        crate::fonts::setup_bundled_fonts(&ctx);
        ctx.set_embed_viewports(false);
        update(&ctx, "w", |p| p.detach());
        detached_window(&ctx, false);
        assert!(!take_native_input(&ctx), "nothing done in it");
        let click = egui::Event::PointerButton {
            pos: Pos2::new(20.0, 20.0),
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        };
        for event in [
            click,
            key(Key::A, Modifiers::NONE, true),
            egui::Event::Text("a".into()),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Line,
                delta: vec2(0.0, 1.0),
                phase: egui::TouchPhase::Move,
                source: egui::MouseWheelSource::Unknown,
                modifiers: Modifiers::NONE,
            },
        ] {
            *keys.borrow_mut() = vec![event.clone()];
            detached_window(&ctx, false);
            assert!(take_native_input(&ctx), "{event:?}");
            assert!(!take_native_input(&ctx), "once: {event:?}");
        }
        *keys.borrow_mut() = vec![egui::Event::PointerMoved(Pos2::new(30.0, 30.0))];
        detached_window(&ctx, false);
        assert!(!take_native_input(&ctx), "the pointer passing over it changes nothing");
        // Moved or resized by the system: its new place is the app's to store.
        native_moved(&ctx, "w", Some(vec2(640.0, 480.0)), Some(Pos2::new(10.0, 20.0)));
        assert!(take_native_input(&ctx));
        native_moved(&ctx, "w", Some(vec2(640.0, 480.0)), Some(Pos2::new(10.0, 20.0)));
        assert!(!take_native_input(&ctx), "where it was already");
    }

    #[test]
    fn a_native_window_waits_while_the_main_window_asks_a_question() {
        let keys = Keys::default();
        native_windows(&keys);
        let ctx = Context::default();
        crate::fonts::setup_bundled_fonts(&ctx);
        ctx.set_embed_viewports(false);
        update(&ctx, "w", |p| p.detach());
        let enabled = std::cell::Cell::new(None);
        let draw = |question: bool| {
            frame(&ctx, |ui| {
                if question {
                    egui::Modal::new(Id::unique("question")).show(ui.ctx(), |ui| ui.label("Sure?"));
                }
                super::super::Window::new(Id::unique("w"), "", "W").dockable("w").show(ui.ctx(), true, |ui| enabled.set(Some(ui.is_enabled())));
            });
        };
        draw(false);
        assert_eq!(enabled.get(), Some(true));
        draw(true);
        draw(true);
        assert_eq!(enabled.get(), Some(false), "the window of its own waits for the answer");
        draw(false);
        draw(false);
        assert_eq!(enabled.get(), Some(true), "answered: it works again");
    }

    #[test]
    fn a_window_shown_inside_the_main_one_passes_no_keys() {
        let ctx = Context::default();
        crate::fonts::setup_bundled_fonts(&ctx);
        update(&ctx, "w", |p| p.detach());
        let events = vec![key(egui::Key::Pause, egui::Modifiers::NONE, true)];
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1200.0, 800.0))), events, ..Default::default() };
        let mut out = ctx.run_ui(input, |ui| {
            super::super::Window::new(Id::unique("w"), "", "W").dockable("w").show(ui.ctx(), true, |_| {});
        });
        out.textures_delta.clear();
        assert!(take_window_keys(&ctx).is_empty(), "the main window has these keys already");
    }

    /// One frame at time `t` (seconds) with these events, a dock area at the right and a
    /// window `a` (a movable card) over it, as the app has them. Gives the card's response.
    fn drop_frame(ctx: &Context, t: f64, events: Vec<egui::Event>) -> egui::Response {
        let mut card = None;
        let input =
            egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1200.0, 800.0))), time: Some(t), events, ..Default::default() };
        let mut out = ctx.run_ui(input, |ui| {
            dock_target(ui.ctx(), "Right", Rect::from_min_max(Pos2::new(400.0, 0.0), Pos2::new(1200.0, 800.0)));
            let shown = egui::Area::new(Id::unique("card")).default_pos(Pos2::new(450.0, 100.0)).show(ui.ctx(), |ui| {
                // The card's body: pressed and held there, as on a window's empty space.
                ui.allocate_space(vec2(200.0, 100.0));
            });
            dock_on_drop(ui.ctx(), "a", &shown.response);
            card = Some(shown.response);
        });
        out.textures_delta.clear();
        card.expect("drawn")
    }

    fn button(pos: Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE }
    }

    #[test]
    fn a_long_press_on_a_window_over_a_dock_area_is_not_a_drop() {
        let ctx = Context::default();
        drop_frame(&ctx, 0.0, vec![]);
        let card = drop_frame(&ctx, 0.05, vec![]).rect;
        let on = card.center();
        assert!(target_at(&ctx, on).is_some(), "the card sits over the dock area");
        drop_frame(&ctx, 0.08, vec![egui::Event::PointerMoved(on)]);
        assert!(drop_frame(&ctx, 0.1, vec![button(on, true)]).dragged(), "the press is on the card");
        // Held still for well over egui's longest click, which then counts it a drag.
        for i in 1..=8 {
            assert!(drop_frame(&ctx, 0.1 + f64::from(i) * 0.25, vec![]).dragged());
        }
        assert!(ctx.input(|i| i.pointer.is_decidedly_dragging()), "egui calls a press held this long a drag");
        drop_frame(&ctx, 2.5, vec![button(on, false)]);
        drop_frame(&ctx, 2.6, vec![]);
        assert_eq!(place(&ctx, "a").placement, Placement::Floating, "a press held still is not a drop");

        // A real drag by the same card is one: docked where it is let go.
        drop_frame(&ctx, 3.0, vec![button(on, true)]);
        for (i, dx) in [10.0, 40.0, 80.0].into_iter().enumerate() {
            drop_frame(&ctx, 3.1 + i as f64 * 0.1, vec![egui::Event::PointerMoved(on + vec2(dx, 20.0))]);
        }
        drop_frame(&ctx, 3.5, vec![button(on + vec2(80.0, 20.0), false)]);
        assert_eq!(place(&ctx, "a").placement, Placement::Docked("Right".into()));

        // A wobble smaller than a click's, held long, is not one either. (The card moved with the
        // drag: pressed where it is now.)
        update(&ctx, "a", |p| p.float(None));
        let on = drop_frame(&ctx, 4.0, vec![]).rect.center();
        drop_frame(&ctx, 4.05, vec![egui::Event::PointerMoved(on)]);
        assert!(drop_frame(&ctx, 4.1, vec![button(on, true)]).dragged());
        drop_frame(&ctx, 4.4, vec![egui::Event::PointerMoved(on + vec2(3.0, 2.0))]);
        drop_frame(&ctx, 5.5, vec![]);
        drop_frame(&ctx, 5.6, vec![button(on + vec2(3.0, 2.0), false)]);
        assert_eq!(place(&ctx, "a").placement, Placement::Floating, "a click's wobble is not a drop");
    }

    #[test]
    fn a_native_window_opens_on_the_screens_there_are_now() {
        let ctx = Context::default();
        // Stored on a monitor far to the right that is gone now, bigger than this one.
        update(&ctx, "far", |p| {
            p.detach();
            p.native_pos = Some([9000.0, 9000.0]);
            p.native_size = Some([4000.0, 3000.0]);
        });
        update(&ctx, "near", |p| {
            p.detach();
            p.native_pos = Some([200.0, 150.0]);
            p.native_size = Some([640.0, 480.0]);
        });
        let mut input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1200.0, 800.0))), ..Default::default() };
        input.viewports.insert(
            egui::ViewportId::ROOT,
            egui::ViewportInfo {
                outer_rect: Some(Rect::from_min_size(Pos2::new(100.0, 50.0), vec2(1200.0, 800.0))),
                monitor_size: Some(vec2(1920.0, 1080.0)),
                ..Default::default()
            },
        );
        let mut seen = None;
        let first = vec2(400.0, 300.0);
        let mut out = ctx.run_ui(input, |ui| seen = Some((native_opening(ui.ctx(), "far", first), native_opening(ui.ctx(), "near", first))));
        out.textures_delta.clear();
        let (far, near) = seen.expect("drawn");
        assert_eq!(far, ([1920.0, 1080.0], None), "the system places it, no bigger than the monitor");
        assert_eq!(near, ([640.0, 480.0], Some([200.0, 150.0])), "a place on this monitor is kept");
    }
}
