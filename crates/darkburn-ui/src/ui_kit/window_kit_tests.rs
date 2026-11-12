//! The dockable window's edges: a window of its own closed by the system, Esc in it, the order
//! of tabs docked together across a restart, and stored places that no longer fit the screen.

use std::cell::RefCell;
use std::rc::Rc;

use egui::{Context, Event, Id, Key, Modifiers, Pos2, RawInput, Rect, ViewportEvent, ViewportInfo, vec2};

use super::Window;
use super::window_host::{self as host, Placement, WindowPlace, WindowPlaces};

const SCREEN: Rect = Rect { min: Pos2::ZERO, max: Pos2 { x: 1200.0, y: 800.0 } };

fn root_frame(ctx: &Context, events: Vec<Event>, add: impl FnMut(&mut egui::Ui)) {
    let input = RawInput { screen_rect: Some(SCREEN), events, ..Default::default() };
    let mut out = ctx.run_ui(input, add);
    out.textures_delta.clear();
}

/// What the system sends a native window next frame: window events and keys.
#[derive(Default)]
struct Native {
    events: Vec<ViewportEvent>,
    keys: Vec<Event>,
    /// Frames the native window was drawn.
    drawn: usize,
}

/// A context that opens native windows as a desktop backend does: each one runs a pass of its
/// own with the input the system gave it (here: what `native` holds).
fn native_context(native: Rc<RefCell<Native>>) -> Context {
    let ctx = Context::default();
    ctx.set_embed_viewports(false);
    // Per thread, as each test runs on its own.
    Context::set_immediate_viewport_renderer(move |ctx, mut viewport| {
        let (events, keys) = {
            let mut n = native.borrow_mut();
            n.drawn += 1;
            (std::mem::take(&mut n.events), std::mem::take(&mut n.keys))
        };
        let mut input = RawInput {
            viewport_id: viewport.ids.this,
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(500.0, 400.0))),
            events: keys,
            ..Default::default()
        };
        input.viewports.insert(viewport.ids.this, ViewportInfo { events, ..Default::default() });
        let mut out = ctx.run_ui(input, |ui| (viewport.viewport_ui_cb)(ui));
        out.textures_delta.clear();
    });
    ctx
}

fn detached(key: &str) -> WindowPlaces {
    let mut places = WindowPlaces::new();
    places.insert(key.into(), WindowPlace { placement: Placement::Detached, ..Default::default() });
    places
}

fn escape() -> Event {
    Event::Key { key: Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }
}

/// One frame of the main window with a footerless dockable window `key`; whether it closed.
fn show_window(ctx: &Context, places: &mut WindowPlaces, key: &str, events: Vec<Event>, escape_here: bool) -> bool {
    let mut closed = false;
    root_frame(ctx, events, |ui| {
        host::sync(ui.ctx(), places);
        let (c, _) = Window::new(Id::unique(("kit_test", key)), "settings", "Test window")
            .dockable(key)
            .actions("", "", None)
            .escape_here(escape_here)
            .show(ui.ctx(), true, |ui| {
                ui.label("Contents");
            });
        closed |= c;
    });
    closed
}

#[test]
fn a_window_of_its_own_closed_by_the_system_is_closed_in_the_app() {
    let native = Rc::new(RefCell::new(Native::default()));
    let ctx = native_context(native.clone());
    let mut places = detached("kit_close");
    for _ in 0..3 {
        assert!(!show_window(&ctx, &mut places, "kit_close", vec![], true), "open while nothing closes it");
    }
    assert!(native.borrow().drawn >= 3, "drawn as a native window");
    // The system's close button: the app hears the window closed, as from its own close button.
    native.borrow_mut().events.push(ViewportEvent::Close);
    assert!(show_window(&ctx, &mut places, "kit_close", vec![], true), "closed by the system's close button");
    // It stays a window of its own for the next time it opens.
    assert_eq!(places["kit_close"].placement, Placement::Detached);
}

#[test]
fn esc_in_a_window_of_its_own_closes_it_as_when_floating() {
    let native = Rc::new(RefCell::new(Native::default()));
    let ctx = native_context(native.clone());
    let mut places = detached("kit_escape");
    for _ in 0..2 {
        assert!(!show_window(&ctx, &mut places, "kit_escape", vec![], false));
    }
    // Esc pressed in the main window is not this window's: it has keys of its own.
    assert!(!show_window(&ctx, &mut places, "kit_escape", vec![escape()], true));
    native.borrow_mut().keys.push(escape());
    assert!(show_window(&ctx, &mut places, "kit_escape", vec![], false), "Esc in the native window closes it");
}

#[test]
fn esc_closes_a_floating_footerless_window_only_when_it_is_its_own() {
    let ctx = Context::default();
    let mut places = WindowPlaces::new();
    for _ in 0..2 {
        assert!(!show_window(&ctx, &mut places, "kit_float", vec![], true));
    }
    // Another window or list is open beside it: the key is not this window's.
    assert!(!show_window(&ctx, &mut places, "kit_float", vec![escape()], false));
    assert!(show_window(&ctx, &mut places, "kit_float", vec![escape()], true));
}

#[test]
fn a_floating_window_stored_off_the_screen_comes_back_inside_it() {
    let ctx = Context::default();
    let mut places = WindowPlaces::new();
    places.insert("kit_far".into(), WindowPlace { pos: Some([5000.0, 4000.0]), size: Some([400.0, 300.0]), ..Default::default() });
    let mut rect = Rect::NOTHING;
    for _ in 0..4 {
        show_window(&ctx, &mut places, "kit_far", vec![], true);
        rect = egui::AreaState::load(&ctx, Id::unique(("kit_test", "kit_far"))).and_then(|a| a.rect().is_finite().then(|| a.rect())).unwrap_or(rect);
    }
    let keep = crate::titlebar::keep_clear_of_border(&ctx);
    assert!(SCREEN.shrink(keep - 0.5).contains_rect(rect), "{rect:?} inside {SCREEN:?}, {keep} clear of its edges");
    // Where it is now is what is stored, so it opens there next time.
    let pos = places["kit_far"].pos.expect("a stored place");
    assert!(SCREEN.contains(Pos2::from(pos)), "{pos:?}");
}

#[test]
fn a_native_window_stored_on_a_monitor_now_gone_opens_where_it_is_seen() {
    let main = Some(Rect::from_min_size(Pos2::new(100.0, 50.0), vec2(1400.0, 900.0)));
    let monitor = Some(vec2(1920.0, 1080.0));
    // On the main window's monitor, or on one beside it: kept.
    for pos in [[200.0, 120.0], [1900.0, 40.0], [-1700.0, 300.0], [2500.0, 900.0]] {
        assert!(host::native_pos_reachable(main, monitor, pos), "{pos:?} is reachable");
    }
    // Far away, or not a number: left to the system.
    for pos in [[5000.0, 5000.0], [-4000.0, 0.0], [300.0, 9000.0], [f32::NAN, 0.0]] {
        assert!(!host::native_pos_reachable(main, monitor, pos), "{pos:?} is not reachable");
    }
    // Nothing known about the screen: the stored place is trusted.
    assert!(host::native_pos_reachable(None, None, [5000.0, 5000.0]));
}

#[test]
fn windows_docked_in_one_area_keep_their_order_across_a_restart() {
    let ctx = Context::default();
    let mut places = WindowPlaces::new();
    let draw = |ctx: &Context, places: &mut WindowPlaces| {
        root_frame(ctx, vec![], |ui| {
            host::sync(ui.ctx(), places);
            host::dock_areas(ui.ctx(), &[("Right", "Right")]);
            for key in ["zeta", "alpha", "mid"] {
                Window::new(Id::unique(("kit_order", key)), "settings", key).dockable(key).actions("", "", None).show(ui.ctx(), true, |ui| {
                    ui.label(key);
                });
            }
        });
    };
    draw(&ctx, &mut places);
    // Docked in this order: not the order of their names.
    for key in ["zeta", "alpha", "mid"] {
        host::update(&ctx, key, |p| p.dock("Right"));
    }
    draw(&ctx, &mut places);
    draw(&ctx, &mut places);
    let order = |ctx: &Context| host::docked_tabs(ctx, "Right").into_iter().map(|t| t.key).collect::<Vec<_>>();
    assert_eq!(order(&ctx), ["zeta", "alpha", "mid"]);
    // Saved and read back by a new run of the app: the same order.
    let saved = serde_json::to_string(&places).unwrap();
    let mut back: WindowPlaces = host::lenient(&mut serde_json::Deserializer::from_str(&saved)).unwrap();
    let again = Context::default();
    draw(&again, &mut back);
    draw(&again, &mut back);
    assert_eq!(order(&again), ["zeta", "alpha", "mid"]);
    // One taken out and docked again goes to the end.
    host::update(&ctx, "zeta", |p| p.float(None));
    host::update(&ctx, "zeta", |p| p.dock("Right"));
    draw(&ctx, &mut places);
    assert_eq!(order(&ctx), ["alpha", "mid", "zeta"]);
}

#[test]
fn a_layout_reset_floats_every_window_at_its_first_place() {
    let ctx = Context::default();
    let mut places = WindowPlaces::new();
    let id = Id::unique(("kit_test", "kit_reset"));
    let first = |ctx: &Context| egui::AreaState::load(ctx, id).map(|a| a.rect());
    for _ in 0..3 {
        show_window(&ctx, &mut places, "kit_reset", vec![], true);
    }
    let home = first(&ctx).expect("shown");
    // Moved and sized by the user.
    host::update(&ctx, "kit_reset", |p| p.resize(vec2(500.0, 400.0)));
    show_window(&ctx, &mut places, "kit_reset", vec![], true);
    host::moved(&ctx, "kit_reset", Pos2::new(40.0, 300.0));
    for _ in 0..3 {
        show_window(&ctx, &mut places, "kit_reset", vec![], true);
    }
    assert_ne!(first(&ctx).unwrap(), home);
    // The app's reset: no stored places, and egui forgets where its areas were.
    places = WindowPlaces::new();
    ctx.memory_mut(|m| m.reset_areas());
    for _ in 0..3 {
        show_window(&ctx, &mut places, "kit_reset", vec![], true);
    }
    assert_eq!(first(&ctx).unwrap(), home, "back where it first opened, at its first size");
    assert!(places.get("kit_reset").is_none_or(|p| p.placement == Placement::Floating && p.size.is_none()), "{places:?}");
}
