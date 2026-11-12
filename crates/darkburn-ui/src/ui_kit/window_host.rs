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
    opened: BTreeMap<String, (Option<[f32; 2]>, Option<[f32; 2]>)>,
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

/// The size and position a native window opens with: what was stored when it opened, kept the
/// same while it is open so the system's own moves are not fought. A stored position that is no
/// longer on a screen near the main window (a monitor since unplugged) is left to the system,
/// which opens the window where it can be seen; a stored size is kept to the monitor's.
pub(crate) fn native_opening(ctx: &Context, key: &str) -> (Option<[f32; 2]>, Option<[f32; 2]>) {
    let (main, monitor) = ctx.input(|i| (i.viewport().outer_rect, i.viewport().monitor_size));
    with(ctx, |s| {
        let stored = s.places.get(key).map(|p| (p.native_size, p.native_pos)).unwrap_or_default();
        let size = stored.0.map(|v| match monitor.filter(|m| m.x > 0.0 && m.y > 0.0) {
            Some(m) => clamp_size([v[0].min(m.x), v[1].min(m.y)]),
            None => v,
        });
        let pos = stored.1.filter(|p| native_pos_reachable(main, monitor, *p));
        *s.opened.entry(key.to_string()).or_insert((size, pos))
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
    with(ctx, |s| {
        let place = s.places.entry(key.to_string()).or_default();
        let size = size.filter(|v| v.x.is_finite() && v.y.is_finite() && v.x > 0.0 && v.y > 0.0).map(|v| clamp_size([v.x, v.y]));
        let pos = pos.filter(|p| p.x.is_finite() && p.y.is_finite()).map(|p| [p.x, p.y]);
        let changed = (size.is_some() && size != place.native_size) || (pos.is_some() && pos != place.native_pos);
        if changed {
            place.native_size = size.or(place.native_size);
            place.native_pos = pos.or(place.native_pos);
            s.dirty = true;
        }
    });
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
    let painter = ctx.layer_painter(LayerId::new(egui::Order::Foreground, Id::unique("db_dock_drop_hint")));
    painter.rect(rect.shrink(2.0), t.radius_sm, t.accent.gamma_multiply(0.18), egui::Stroke::new(2.0, t.accent), egui::StrokeKind::Inside);
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        solidcraft_ui_text::tr_label("Dock as tab"),
        crate::fonts::semibold(t.font_heading),
        t.text,
    );
    solidcraft_ui_registry::publish_rect("dialog:dock_hint", rect);
}

/// The header button that moves the window: back into the main window, into a window of its
/// own, or into a panel area as a tab.
pub(crate) fn place_button(ui: &mut egui::Ui, rect: Rect, key: &str) {
    let mut pick: Option<Placement> = None;
    super::Menu::button_with(
        ui,
        |ui, _| {
            let response = super::icon_button_at(ui, rect, "prefs_layout", solidcraft_ui_text::tr_label("Move this window"));
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
    let mut pick: Option<Placement> = None;
    ui.set_min_width(super::LIST_W);
    if place.placement != Placement::Floating
        && super::MenuItem::new("window.float", solidcraft_ui_text::tr_label("Float in the main window"), "").show(ui).clicked()
    {
        pick = Some(Placement::Floating);
    }
    if place.placement != Placement::Detached
        && super::MenuItem::new("window.detach", solidcraft_ui_text::tr_label("Open in its own window"), "").show(ui).clicked()
    {
        pick = Some(Placement::Detached);
    }
    if !areas.is_empty() {
        super::Menu::separator(ui);
        super::Menu::heading(ui, solidcraft_ui_text::tr_label("Dock as tab"));
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
    if !(response.dragged() || response.drag_stopped()) || !moved_since_press(ctx) {
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

/// The pointer was moved since its button went down, beyond a click's wobble: a press and
/// release on a button in a window (its Cancel, say) is not a drop, even held long. (6 points:
/// egui's own wobble allowance for a click.)
fn moved_since_press(ctx: &Context) -> bool {
    ctx.input(|i| {
        i.pointer.is_decidedly_dragging() && i.pointer.press_origin().zip(i.pointer.latest_pos()).is_some_and(|(from, to)| from.distance(to) > 6.0)
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
    fn a_native_window_keeps_its_opening_size_while_open() {
        let ctx = Context::default();
        update(&ctx, "a", |p| {
            p.detach();
            p.native_size = Some([640.0, 480.0]);
        });
        assert_eq!(native_opening(&ctx, "a"), (Some([640.0, 480.0]), None));
        native_moved(&ctx, "a", Some(vec2(700.0, 500.0)), Some(Pos2::new(10.0, 20.0)));
        assert_eq!(native_opening(&ctx, "a"), (Some([640.0, 480.0]), None), "the system's own resize is not sent back");
        assert_eq!(place(&ctx, "a").native_size, Some([700.0, 500.0]), "but it is remembered");
        // Closed (not drawn for two frames) and opened again: the next opening uses it.
        frame(&ctx, |ui| shown(ui.ctx(), "a", "A"));
        frame(&ctx, |_| {});
        frame(&ctx, |_| {});
        frame(&ctx, |ui| shown(ui.ctx(), "a", "A"));
        assert_eq!(native_opening(&ctx, "a"), (Some([700.0, 500.0]), Some([10.0, 20.0])));
    }
}
