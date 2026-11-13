//! Headless GUI test harness: the real app, driven with simulated input, drawn offscreen.
//!
//! No desktop window and no OS-level input is involved. The app runs inside `egui_kittest`'s
//! harness (egui's own test runner): the same `App` as `main.rs` (so `logic()` and `ui()` both
//! run every frame), fed `egui::Event`s. Pictures are drawn with wgpu into an offscreen texture
//! (a software adapter when there is one). See `docs/UI-TESTING.md`.
//!
//! Finding things. Most of DarkBurn's chrome is painted by hand (ribbon, tool column, menus), so
//! the accessibility tree only has the standard egui widgets (dialog buttons, fields, panels).
//! The harness therefore finds a control three ways:
//! - by the text painted on screen ([`Gui::click_label`], [`Gui::find_text`]);
//! - by the name the widget published to the UI registry ([`Gui::click_id`]):
//!   `ribbon:<command id>`, `ribbon:tab:<tab>`, `tool:<command id>`, `appbar:menu:<name>`,
//!   `dialog:button:<label>`, `canvas`, ...;
//! - by the accessibility tree ([`Gui::a11y`], [`Gui::click_a11y`]).
//!
//! Safety. `Gui::new` makes the app safe: a temp settings dir, safe mode (nothing is saved),
//! simulators only (the default device list), MCP off, the file dialogs scripted (no native dialog
//! ever opens: queue answers with [`Gui::answer_dialog`]), and no browser, printer, file manager,
//! second window, clipboard or network (those paths return early under `cfg(test)`).
//! egui's duplicate-widget-ID warning is on, and any frame that shows one fails the test at once
//! (turn that off with [`Gui::allow_id_clashes`]).

#![cfg(test)]
#![allow(dead_code, reason = "a harness: not every helper is used by every test")]

use std::cell::{RefCell, RefMut};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use egui::{Event, Key, Modifiers, PointerButton, Pos2, Rect, Vec2, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::NodeT as _;

use crate::prefs::Prefs;
use crate::shell::DarkBurn;

/// Where screenshots go.
pub fn snapshot_dir() -> PathBuf {
    let target =
        std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from).unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target"));
    target.join("ui-snapshots")
}

/// Where the golden pictures live.
pub fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("golden-ui")
}

/// One node of the accessibility tree.
#[derive(Clone, Debug)]
pub struct A11y {
    pub role: String,
    pub label: Option<String>,
    pub value: Option<String>,
    pub rect: Rect,
    /// The widget's egui id (the accessibility node id is the same number).
    pub id: u64,
}

/// One piece of text painted on screen, topmost last.
#[derive(Clone, Debug)]
pub struct Painted {
    pub text: String,
    /// The text's rectangle clipped to what is visible.
    pub rect: Rect,
    /// The rectangle of the whole text, even where it is clipped.
    pub full: Rect,
    /// A word was broken across rows (a label squeezed into too narrow a space).
    pub broken_word: bool,
    /// The text was cut short with an ellipsis to fit its room.
    pub elided: bool,
}

/// Whether a row that ends with `last` and a row that starts with `first` split a word. Japanese
/// and Chinese write no spaces between words, and a line may break between any two of their
/// characters (egui breaks them so), so a break next to one splits nothing. Korean puts spaces
/// between words like English does.
pub fn splits_a_word(last: char, first: char) -> bool {
    let breaks_anywhere = |c: char| {
        matches!(c as u32,
            0x3040..=0x30FF // hiragana and katakana
            | 0x31F0..=0x31FF // katakana extensions
            | 0x3400..=0x4DBF // CJK ideographs, extension A
            | 0x4E00..=0x9FFF // CJK ideographs
            | 0xF900..=0xFAFF // CJK compatibility ideographs
            | 0xFF66..=0xFF9F) // half-width katakana
    };
    last.is_alphanumeric() && first.is_alphanumeric() && !breaks_anywhere(last) && !breaks_anywhere(first)
}

/// One lock for everything that makes the software rasteriser work: making a GPU device,
/// destroying one, and drawing a picture take it alone; ordinary frames share it.
///
/// Tests run in parallel threads, each with its own device, and all of them end up on the one
/// software adapter the harness prefers (WARP on Windows). The process crashed now and then
/// with an access violation inside `d3d10warp.dll`, on one of its own worker threads. The
/// crash dumps showed some tests drawing a screenshot while others made or destroyed their
/// device; serialising only creation and teardown did not stop it. So drawing takes the lock
/// too. Ordinary frames draw nothing, but they make textures and upload them; on Linux the
/// Vulkan loader crashed naming such an upload buffer while another test made or destroyed its
/// device. So frames take the lock shared: side by side with each other, never beside a device
/// being made, destroyed or drawn.
///
/// Everything that owns a piece of the device lives inside the harness (the renderer and the
/// copies of its render state that eframe keeps), and the harness only ever exists inside a
/// [`Guarded`]: [`Guarded::build`] makes it under the lock and `Drop` destroys it under the
/// lock, also when a test panics or the app fails to start. The app itself holds nothing of
/// the GPU, so it may outlive the harness.
static GPU: std::sync::RwLock<()> = std::sync::RwLock::new(());

struct Guarded(Option<Harness<'static, crate::App>>);

impl Guarded {
    /// Runs `make` (which creates the device) under the GPU lock. If `make` panics, what it
    /// had built so far is dropped while the lock is still held.
    fn build(make: impl FnOnce() -> Harness<'static, crate::App>) -> Guarded {
        let _gpu = GPU.write().unwrap_or_else(|e| e.into_inner());
        Guarded(Some(make()))
    }

    /// One ordinary frame, under the shared side of the GPU lock.
    fn step(&mut self) {
        let _gpu = GPU.read().unwrap_or_else(|e| e.into_inner());
        self.0.as_mut().expect("the harness is alive").step();
    }

    /// Draws the last frame offscreen, under the GPU lock.
    fn render(&mut self) -> Result<image::RgbaImage, String> {
        let _gpu = GPU.write().unwrap_or_else(|e| e.into_inner());
        self.0.as_mut().expect("the harness is alive").render()
    }
}

impl std::ops::Deref for Guarded {
    type Target = Harness<'static, crate::App>;
    fn deref(&self) -> &Self::Target {
        self.0.as_ref().expect("the harness is alive")
    }
}

impl std::ops::DerefMut for Guarded {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0.as_mut().expect("the harness is alive")
    }
}

impl Drop for Guarded {
    fn drop(&mut self) {
        let _gpu = GPU.write().unwrap_or_else(|e| e.into_inner());
        self.0 = None;
    }
}

pub struct Gui {
    h: Guarded,
    app: Rc<RefCell<DarkBurn>>,
    pub dir: PathBuf,
    scale: f32,
    /// What the last action was, for failure messages.
    last: String,
    strict_ids: bool,
    clashes: Vec<String>,
    pub shots: Vec<PathBuf>,
}

static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

impl Gui {
    /// A fresh app in a `width` x `height` point window at `scale` pixels per point.
    pub fn new(width: f32, height: f32, scale: f32) -> Gui {
        Gui::with_prefs(width, height, scale, Prefs::default())
    }

    /// A fresh app that starts with `prefs`, as after a restart with those preferences stored.
    pub fn with_prefs(width: f32, height: f32, scale: f32, prefs: Prefs) -> Gui {
        let dir =
            std::env::temp_dir().join(format!("darkburn-ui-{}-{}", std::process::id(), NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        darkburn_dialogs_script();
        let cell: Rc<RefCell<Option<Rc<RefCell<DarkBurn>>>>> = Rc::new(RefCell::new(None));
        let (cell2, dir2) = (cell.clone(), dir.clone());
        let h = Guarded::build(|| {
            Harness::builder()
                .with_size(vec2(width, height))
                .with_pixels_per_point(scale)
                .with_max_steps(60)
                .allow_missing_glyphs()
                .with_accessibility_check(false)
                .with_context_setup(|ctx| {
                    darkburn_ui::fonts::setup_bundled_fonts(ctx);
                    ctx.options_mut(|o| o.warn_on_id_clash = true);
                })
                .wgpu()
                .build_eframe(move |cc| {
                    let mut app = DarkBurn::new(prefs.clone());
                    app.settings_dir = Some(dir2.clone());
                    app.safe_mode = true;
                    app.custom_titlebar = true;
                    crate::image_ui::load_tuning(&app);
                    app.apply_theme(&cc.egui_ctx);
                    let app = Rc::new(RefCell::new(app));
                    *cell2.borrow_mut() = Some(app.clone());
                    let mcp = crate::mcp_ui::Host::new(app.clone());
                    crate::App { app, writer: crate::PrefsWriter::default(), frames: 2, mcp }
                })
        });
        let app = cell.borrow_mut().take().expect("the app was built");
        let mut g = Gui { h, app, dir, scale, last: "start".into(), strict_ids: true, clashes: Vec::new(), shots: Vec::new() };
        g.frames(3);
        g
    }

    /// 1600 x 1000 points at scale 1, like the app's default window.
    pub fn standard() -> Gui {
        Gui::new(1600.0, 1000.0, 1.0)
    }

    // ----- the app -----

    /// The app, for assertions and for setting things up.
    pub fn app(&self) -> RefMut<'_, DarkBurn> {
        self.app.borrow_mut()
    }

    pub fn ctx(&self) -> egui::Context {
        self.h.ctx.clone()
    }

    pub fn screen(&self) -> Rect {
        self.h.ctx.content_rect()
    }

    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// Run a registered command as the UI does (menu, ribbon, shortcut), then draw a few frames.
    pub fn run_command(&mut self, id: &str) {
        self.last = format!("run_command({id})");
        let ctx = self.ctx();
        self.app().run(&ctx, id);
        self.frames(3);
    }

    // ----- frames -----

    /// Draw `n` frames.
    pub fn frames(&mut self, n: usize) -> &mut Self {
        for _ in 0..n {
            self.h.step();
            self.after_frame();
        }
        self
    }

    /// Draw frames until nothing asks for another one (at most 30).
    pub fn settle(&mut self) -> &mut Self {
        for _ in 0..30 {
            self.frames(1);
            if !self.h.ctx.has_requested_repaint() {
                break;
            }
        }
        self
    }

    fn after_frame(&mut self) {
        // Hardware safety: a test never talks to a real machine.
        if self.app.try_borrow().is_ok_and(|a| a.machine.handle.is_real_machine()) {
            panic!("a UI test connected to a real machine after `{}`", self.last);
        }
        if !self.strict_ids {
            return;
        }
        let found: Vec<String> = self.painted().into_iter().map(|p| p.text).filter(|t| t.contains(" use of ") && t.contains(" ID ")).collect();
        if !found.is_empty() {
            let msg = format!("egui widget ID clash after `{}`: {found:?}", self.last);
            self.shoot_quietly("id-clash");
            self.clashes.extend(found);
            panic!("{msg}");
        }
    }

    /// Stop failing on egui's duplicate-ID warning (it is still collected in [`Gui::clashes`]).
    pub fn allow_id_clashes(&mut self) -> &mut Self {
        self.strict_ids = false;
        self
    }

    /// Duplicate-ID warnings seen so far.
    pub fn clashes(&self) -> &[String] {
        &self.clashes
    }

    /// Run frames and return the duplicate-ID warnings of the last one (for `allow_id_clashes`).
    pub fn id_clashes_now(&self) -> Vec<String> {
        self.painted().into_iter().map(|p| p.text).filter(|t| t.contains(" use of ") && t.contains(" ID ")).collect()
    }

    // ----- what is on screen -----

    /// All text painted in the last frame, bottom to top.
    pub fn painted(&self) -> Vec<Painted> {
        fn walk(s: &egui::epaint::Shape, clip: Rect, out: &mut Vec<Painted>) {
            match s {
                egui::epaint::Shape::Text(t) => {
                    let full = t.galley.rect.translate(t.pos.to_vec2());
                    let rect = full.intersect(clip);
                    let text = t.galley.text().to_string();
                    let rows = &t.galley.rows;
                    let broken_word = rows.windows(2).any(|w| {
                        let (a, b) = (&w[0], &w[1]);
                        !a.ends_with_newline && a.glyphs.last().zip(b.glyphs.first()).is_some_and(|(last, first)| splits_a_word(last.chr, first.chr))
                    });
                    if !text.trim().is_empty() && rect.is_positive() {
                        out.push(Painted { text, rect, full, broken_word, elided: t.galley.elided });
                    }
                }
                egui::epaint::Shape::Vec(v) => v.iter().for_each(|x| walk(x, clip, out)),
                _ => {}
            }
        }
        let mut out = Vec::new();
        for cs in &self.h.output().shapes {
            walk(&cs.shape, cs.clip_rect, &mut out);
        }
        out
    }

    /// The topmost visible text equal to `label` (else containing it).
    pub fn find_text(&self, label: &str) -> Option<Rect> {
        let all = self.painted();
        all.iter().rev().find(|p| p.text.trim() == label).or_else(|| all.iter().rev().find(|p| p.text.contains(label))).map(|p| p.rect)
    }

    /// Every visible text equal to `label`, bottom to top.
    pub fn find_texts(&self, label: &str) -> Vec<Rect> {
        self.painted().iter().filter(|p| p.text.trim() == label).map(|p| p.rect).collect()
    }

    pub fn has_text(&self, label: &str) -> bool {
        self.find_text(label).is_some()
    }

    /// The rectangle a widget published under `name` in the UI registry (latest frame).
    pub fn rect_of(&self, name: &str) -> Option<Rect> {
        solidcraft_ui_registry::rect_of(name)
    }

    /// Every published rectangle whose name starts with `prefix`.
    pub fn rects(&self, prefix: &str) -> Vec<(String, Rect)> {
        solidcraft_ui_registry::rects(prefix)
    }

    /// The accessibility tree (standard egui widgets only).
    pub fn a11y(&self) -> Vec<A11y> {
        let mut out = Vec::new();
        for node in self.h.root().children_recursive() {
            let ak = node.accesskit_node();
            let Some(b) = ak.bounding_box() else { continue };
            let ppp = self.scale;
            let rect = Rect::from_min_max(pos2(b.x0 as f32 / ppp, b.y0 as f32 / ppp), pos2(b.x1 as f32 / ppp, b.y1 as f32 / ppp));
            out.push(A11y { role: format!("{:?}", ak.role()), label: ak.label(), value: ak.value(), rect, id: ak.locate().0.0 });
        }
        out
    }

    /// Accessibility nodes with this label (and role, when given).
    pub fn find_a11y(&self, role: Option<&str>, label: &str) -> Vec<A11y> {
        self.a11y().into_iter().filter(|n| n.label.as_deref() == Some(label) && role.is_none_or(|r| n.role == r)).collect()
    }

    /// Dialog windows drawn in the last frame (ui_kit windows publish `dialog:window`).
    pub fn dialog_windows(&self) -> Vec<Rect> {
        self.rects("dialog:window").into_iter().map(|(_, r)| r).collect()
    }

    // ----- pointer -----

    fn modifiers_for(&mut self, m: Modifiers) {
        self.h.event(Event::ModifiersChanged(m));
    }

    pub fn hover_pos(&mut self, p: Pos2) -> &mut Self {
        self.h.event(Event::PointerMoved(p));
        self.frames(1)
    }

    fn button(&mut self, p: Pos2, button: PointerButton, pressed: bool, m: Modifiers) {
        self.h.event(Event::PointerButton { pos: p, button, pressed, modifiers: m });
    }

    /// Press the primary button at a screen point and keep it down (one frame is drawn).
    pub fn press_pos(&mut self, p: Pos2) -> &mut Self {
        self.last = format!("press at {p:?}");
        self.h.event(Event::PointerMoved(p));
        self.h.step();
        self.after_frame();
        self.button(p, PointerButton::Primary, true, Modifiers::NONE);
        self.frames(1)
    }

    /// Let the primary button go at a screen point (one frame is drawn).
    pub fn release_pos(&mut self, p: Pos2) -> &mut Self {
        self.last = format!("release at {p:?}");
        self.button(p, PointerButton::Primary, false, Modifiers::NONE);
        self.frames(1)
    }

    /// Click at a screen point (points, not pixels).
    pub fn click_pos(&mut self, p: Pos2) -> &mut Self {
        self.click_button(p, PointerButton::Primary, Modifiers::NONE)
    }

    pub fn right_click_pos(&mut self, p: Pos2) -> &mut Self {
        self.click_button(p, PointerButton::Secondary, Modifiers::NONE)
    }

    pub fn click_button(&mut self, p: Pos2, b: PointerButton, m: Modifiers) -> &mut Self {
        self.last = format!("click at {p:?}");
        self.h.event(Event::PointerMoved(p));
        self.h.step();
        self.after_frame();
        if m != Modifiers::NONE {
            self.modifiers_for(m);
        }
        self.button(p, b, true, m);
        self.button(p, b, false, m);
        if m != Modifiers::NONE {
            self.modifiers_for(Modifiers::NONE);
        }
        self.frames_for_queue();
        self.frames(2)
    }

    /// Run the frames the queued events need (one each).
    fn frames_for_queue(&mut self) {
        self.h.step();
        self.after_frame();
    }

    pub fn double_click_pos(&mut self, p: Pos2) -> &mut Self {
        self.last = format!("double click at {p:?}");
        self.h.event(Event::PointerMoved(p));
        self.h.step();
        self.after_frame();
        // Both clicks in one frame: the harness's frames are 250 ms apart, too slow for egui's
        // double-click window (300 ms), and its queue gives each event a frame of its own.
        let events = &mut self.h.input_mut().events;
        for _ in 0..2 {
            for pressed in [true, false] {
                events.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
            }
        }
        self.h.step();
        self.after_frame();
        self.frames(2)
    }

    /// Click the topmost visible text equal to `label` (else containing it).
    #[track_caller]
    pub fn click_label(&mut self, label: &str) -> &mut Self {
        let Some(r) = self.find_text(label) else {
            self.shoot_quietly("missing-label");
            panic!("click_label: no text {label:?} on screen. Visible: {:?}", self.visible_texts());
        };
        self.last = format!("click_label({label})");
        self.click_pos(r.center())
    }

    /// Click the `n`th (0 = bottom-most) visible text equal to `label`.
    #[track_caller]
    pub fn click_label_nth(&mut self, label: &str, n: usize) -> &mut Self {
        let rs = self.find_texts(label);
        let r = rs.get(n).unwrap_or_else(|| panic!("click_label_nth: only {} texts {label:?} on screen", rs.len()));
        self.click_pos(r.center())
    }

    pub fn double_click_label(&mut self, label: &str) -> &mut Self {
        let r = self.find_text(label).unwrap_or_else(|| panic!("double_click_label: no text {label:?} on screen"));
        self.double_click_pos(r.center())
    }

    /// Click a widget by the name it published to the UI registry (`ribbon:file.new`,
    /// `tool:tools.rect`, `appbar:menu:File`, `dialog:button:Close`, ...).
    #[track_caller]
    pub fn click_id(&mut self, name: &str) -> &mut Self {
        let Some(r) = self.rect_of(name) else {
            self.shoot_quietly("missing-id");
            panic!("click_id: nothing published as {name:?}");
        };
        self.last = format!("click_id({name})");
        self.click_pos(r.center())
    }

    /// Click an accessibility node by label (and role).
    #[track_caller]
    pub fn click_a11y(&mut self, role: Option<&str>, label: &str) -> &mut Self {
        let nodes = self.find_a11y(role, label);
        let n = nodes.last().unwrap_or_else(|| panic!("click_a11y: no {role:?} node labelled {label:?}"));
        self.click_pos(n.rect.center())
    }

    /// Press at `from`, move to `to` in `steps` frames, release. Screen points.
    pub fn drag(&mut self, from: Pos2, to: Pos2) -> &mut Self {
        self.drag_steps(from, to, 8)
    }

    pub fn drag_steps(&mut self, from: Pos2, to: Pos2, steps: usize) -> &mut Self {
        self.last = format!("drag {from:?} -> {to:?}");
        self.h.event(Event::PointerMoved(from));
        self.h.step();
        self.after_frame();
        self.button(from, PointerButton::Primary, true, Modifiers::NONE);
        self.h.step();
        self.after_frame();
        for i in 1..=steps {
            let p = from + (to - from) * (i as f32 / steps as f32);
            self.h.event(Event::PointerMoved(p));
            self.h.step();
            self.after_frame();
        }
        self.button(to, PointerButton::Primary, false, Modifiers::NONE);
        self.h.step();
        self.after_frame();
        self.frames(2)
    }

    /// Scroll the wheel at a point (points; positive y scrolls content up).
    pub fn scroll(&mut self, at: Pos2, delta: Vec2) -> &mut Self {
        self.last = format!("scroll at {at:?}");
        self.h.event(Event::PointerMoved(at));
        self.h.event(Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta,
            modifiers: Modifiers::NONE,
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Wheel,
        });
        self.h.step();
        self.after_frame();
        self.frames(2)
    }

    // ----- keyboard -----

    /// Type text (as the OS sends it after a key press).
    pub fn type_text(&mut self, text: &str) -> &mut Self {
        self.last = format!("type_text({text})");
        self.h.event(Event::Text(text.to_string()));
        self.frames_for_queue();
        self.frames(1)
    }

    /// Press and release a key with modifiers.
    pub fn key(&mut self, key: Key, m: Modifiers) -> &mut Self {
        self.last = format!("key({key:?}, {m:?})");
        if m != Modifiers::NONE {
            self.modifiers_for(m);
        }
        for pressed in [true, false] {
            self.h.event(Event::Key { key, physical_key: Some(key), pressed, repeat: false, modifiers: m });
        }
        if m != Modifiers::NONE {
            self.modifiers_for(Modifiers::NONE);
        }
        self.h.step();
        self.after_frame();
        self.frames(2)
    }

    /// Press and release a key whose place on the keyboard (`physical`) differs from what it
    /// means (`key`): number pad 8 with NumLock off is `ArrowUp` on `Num8`, Shift+1 on a US
    /// keyboard is `Exclamationmark` on `Num1`.
    pub fn key_at(&mut self, key: Key, physical: Key, modifiers: Modifiers) -> &mut Self {
        self.last = format!("key_at({key:?}, {physical:?}, {modifiers:?})");
        for pressed in [true, false] {
            self.h.event(Event::Key { key, physical_key: Some(physical), pressed, repeat: false, modifiers });
        }
        self.h.step();
        self.after_frame();
        self.frames(2)
    }

    pub fn escape(&mut self) -> &mut Self {
        self.key(Key::Escape, Modifiers::NONE)
    }

    pub fn enter(&mut self) -> &mut Self {
        self.key(Key::Enter, Modifiers::NONE)
    }

    // ----- menus -----

    /// Open application-bar menu `menu` and click the item at `path` (`"Align > Left"` walks
    /// into submenus). The item's text is what the menu shows.
    #[track_caller]
    pub fn open_menu(&mut self, menu: &str, path: &str) -> &mut Self {
        self.open_menu_only(menu);
        let parts: Vec<&str> = path.split('>').map(str::trim).collect();
        for (i, part) in parts.iter().enumerate() {
            let Some(r) = self.find_menu_item(part) else {
                self.shoot_quietly("missing-menu-item");
                panic!("open_menu({menu:?}, {path:?}): no menu item {part:?}. Visible: {:?}", self.visible_texts());
            };
            self.last = format!("menu {menu} > {path} ({part})");
            if i + 1 < parts.len() {
                self.hover_pos(r.center());
                self.click_pos(r.center());
            } else {
                self.click_pos(r.center());
            }
        }
        self
    }

    /// Open application-bar menu `menu` and leave it open.
    #[track_caller]
    pub fn open_menu_only(&mut self, menu: &str) -> &mut Self {
        self.last = format!("open menu {menu}");
        let name = format!("appbar:menu:{menu}");
        let Some(r) = self.rect_of(&name) else {
            panic!("open_menu: no menu {menu:?} in the application bar");
        };
        self.click_pos(r.center())
    }

    /// The text of an item in an open menu (the menu rows paint their text at the left).
    fn find_menu_item(&self, label: &str) -> Option<Rect> {
        let all = self.painted();
        // Popups are drawn after the bar: take the topmost exact match.
        all.iter().rev().find(|p| p.text.trim() == label).map(|p| p.rect)
    }

    // ----- window -----

    /// Resize the window (points).
    pub fn set_window_size(&mut self, width: f32, height: f32) -> &mut Self {
        self.last = format!("set_window_size({width}, {height})");
        self.h.set_size(vec2(width, height));
        self.frames(4)
    }

    pub fn set_scale(&mut self, scale: f32) -> &mut Self {
        self.scale = scale;
        self.h.set_pixels_per_point(scale);
        self.frames(4)
    }

    // ----- the canvas -----

    /// The canvas rectangle on screen (points), as published by the canvas.
    pub fn canvas_rect(&self) -> Rect {
        self.rect_of("canvas").expect("the canvas published its rectangle")
    }

    /// Design (page) millimetres, x right and y down from the page's top-left corner, to screen points.
    pub fn mm_to_screen(&self, x: f32, y: f32) -> Pos2 {
        let view = self.app().prefs.view;
        view.to_screen(self.canvas_rect(), pos2(x, y))
    }

    pub fn screen_to_mm(&self, p: Pos2) -> Pos2 {
        let view = self.app().prefs.view;
        view.to_page(self.canvas_rect(), p)
    }

    pub fn click_mm(&mut self, x: f32, y: f32) -> &mut Self {
        let p = self.mm_to_screen(x, y);
        self.click_pos(p)
    }

    /// Drag on the canvas between two design points (page millimetres).
    pub fn drag_mm(&mut self, from: (f32, f32), to: (f32, f32)) -> &mut Self {
        let (a, b) = (self.mm_to_screen(from.0, from.1), self.mm_to_screen(to.0, to.1));
        self.drag(a, b)
    }

    // ----- pictures -----

    /// Render the last frame to `target/ui-snapshots/<name>.png` and return the path.
    pub fn screenshot(&mut self, name: &str) -> PathBuf {
        let img = self.render_image();
        let path = snapshot_dir().join(format!("{name}.png"));
        std::fs::create_dir_all(path.parent().unwrap()).expect("snapshot dir");
        img.save(&path).expect("write the screenshot");
        self.shots.push(path.clone());
        path
    }

    fn shoot_quietly(&mut self, name: &str) {
        let img = self.render_image_opt();
        if let Some(img) = img {
            let path = snapshot_dir().join(format!("{name}.png"));
            let _ = std::fs::create_dir_all(snapshot_dir());
            let _ = img.save(path);
        }
    }

    /// The last frame as an image (pixels, so `scale` times the point size).
    pub fn render_image(&mut self) -> image::RgbaImage {
        let img = self.h.render().expect("render the frame offscreen");
        over_clear_colour(img)
    }

    fn render_image_opt(&mut self) -> Option<image::RgbaImage> {
        self.h.render().ok().map(over_clear_colour)
    }

    /// Compare the current frame with `tests/golden-ui/<name>.png`. `tolerance` is the largest
    /// share of pixels (0..1) allowed to differ by more than 24 per channel. With
    /// `DARKBURN_BLESS=1` the golden file is written instead. The picture is also saved under
    /// `target/ui-snapshots/<name>.png`, and on a mismatch a `<name>.diff.png` beside it.
    #[track_caller]
    pub fn check_golden(&mut self, name: &str, tolerance: f64) {
        let img = self.render_image();
        let shot = self.screenshot(name);
        let golden = golden_dir().join(format!("{name}.png"));
        if std::env::var("DARKBURN_BLESS").is_ok_and(|v| v != "0" && !v.is_empty()) {
            std::fs::create_dir_all(golden.parent().unwrap()).expect("golden dir");
            std::fs::copy(&shot, &golden).expect("write the golden file");
            return;
        }
        let Ok(want) = image::open(&golden) else {
            panic!("no golden picture {} (run with DARKBURN_BLESS=1 to create it); this frame is at {}", golden.display(), shot.display());
        };
        let want = want.to_rgba8();
        assert_eq!(want.dimensions(), img.dimensions(), "golden {name} has another size");
        let bad = img.pixels().zip(want.pixels()).filter(|(a, b)| a.0.iter().zip(b.0.iter()).any(|(x, y)| x.abs_diff(*y) > 24)).count();
        let share = bad as f64 / (img.width() as f64 * img.height() as f64);
        if share > tolerance {
            let mut diff = img.clone();
            for (d, w) in diff.pixels_mut().zip(want.pixels()) {
                if d.0.iter().zip(w.0.iter()).any(|(x, y)| x.abs_diff(*y) > 24) {
                    *d = image::Rgba([255, 0, 255, 255]);
                }
            }
            let _ = diff.save(snapshot_dir().join(format!("{name}.diff.png")));
            panic!("{name}: {:.3}% of the pixels differ (allowed {:.3}%); see {}", share * 100.0, tolerance * 100.0, shot.display());
        }
    }

    // ----- helpers for tests -----

    /// The visible texts, for failure messages.
    pub fn visible_texts(&self) -> Vec<String> {
        self.painted().into_iter().map(|p| p.text).collect()
    }

    /// Titles of the windows in the accessibility tree (dialogs and floating windows).
    pub fn window_titles(&self) -> Vec<String> {
        self.a11y().into_iter().filter(|n| n.role == "Window").filter_map(|n| n.label).collect()
    }

    /// Run a command with JSON arguments, as an agent would (the document commands).
    #[track_caller]
    pub fn exec(&mut self, id: &str, args: serde_json::Value) -> serde_json::Value {
        self.last = format!("exec({id})");
        let ctx = self.ctx();
        let r = crate::api::execute(&mut self.app(), &ctx, id, &args);
        let v = r.unwrap_or_else(|e| panic!("{id} {args}: {e}"));
        self.frames(2);
        v
    }

    /// Put a few shapes in the document and select them all.
    pub fn sample_design(&mut self) -> &mut Self {
        use serde_json::json;
        self.exec("shape.add_rect", json!({"x": 20, "y": 20, "width": 80, "height": 50, "layer": 1}));
        self.exec("shape.add_ellipse", json!({"x": 140, "y": 30, "width": 60, "height": 40, "layer": 2}));
        self.exec("edit.select_all", json!({}));
        self
    }

    /// Things on screen that clip or overflow the window, one line each:
    /// - published controls (ribbon, tools, menus, panel tabs, status) not wholly inside the screen;
    /// - painted text that runs past the screen edge, or is cut off at the side by a clip
    ///   rectangle (a scroll area cutting a line at its top or bottom is normal and not counted,
    ///   nor is a ruler number cut off where the ruler ends);
    /// - ribbon buttons or panel tabs that overlap each other;
    /// - dialog windows not wholly inside the screen.
    pub fn layout_problems(&self) -> Vec<String> {
        let screen = self.screen();
        let mut out = Vec::new();
        for prefix in ["ribbon:", "tool:", "appbar:", "panel:", "status:", "dialog:window", "canvas"] {
            for (name, r) in self.rects(prefix) {
                if outside(screen, r) {
                    out.push(format!("{name} {r:?} is outside the screen {screen:?}"));
                }
            }
        }
        for prefix in ["ribbon:", "panel:tab:"] {
            let rs: Vec<_> = self.rects(prefix).into_iter().filter(|(n, _)| !n.starts_with("ribbon:tab:") || prefix == "ribbon:").collect();
            for (i, (a, ra)) in rs.iter().enumerate() {
                for (b, rb) in &rs[i + 1..] {
                    let both = ra.intersect(*rb);
                    if a != b && both.width() > 1.0 && both.height() > 1.0 {
                        out.push(format!("{a} {ra:?} overlaps {b} {rb:?}"));
                    }
                }
            }
        }
        // The rulers beside the canvas cut a number off where the ruler ends: that is how a ruler looks.
        let canvas = self.rect_of("canvas");
        let on_ruler =
            |r: Rect| canvas.is_some_and(|c| (c.top() - 24.0..c.top()).contains(&r.top()) || (c.left() - 24.0..c.left()).contains(&r.left()));
        for p in self.painted() {
            if on_ruler(p.full) && p.text.trim().parse::<f64>().is_ok() {
                continue;
            }
            if p.broken_word {
                out.push(format!("text {:?} {:?} is broken in the middle of a word", p.text, p.full));
            } else if outside(screen, p.full) {
                out.push(format!("text {:?} {:?} runs past the screen", p.text, p.full));
            } else if p.rect.width() < p.full.width() - 1.0 && p.rect.height() >= p.full.height() - 1.0 {
                out.push(format!("text {:?} {:?} is cut off at the side (visible {:?})", p.text, p.full, p.rect));
            }
        }
        out
    }

    /// Whether any dialog or floating window (Preview, ...) is open.
    pub fn dialog_open(&self) -> bool {
        !self.dialog_windows().is_empty() || !self.window_titles().is_empty()
    }

    fn job_build_pending(&self) -> bool {
        self.app().job.pending.is_some()
    }

    /// Draws frames until the job being prepared on its worker thread has ended (finished or
    /// cancelled) and the app has taken the result. Returns at once when there is none.
    #[track_caller]
    pub fn wait_for_job_build(&mut self) {
        let end = std::time::Instant::now() + std::time::Duration::from_secs(120);
        while self.job_build_pending() {
            assert!(std::time::Instant::now() < end, "the job build never ended after `{}`", self.last);
            std::thread::sleep(std::time::Duration::from_millis(2));
            self.frames(1);
        }
    }

    /// Close every open dialog window: Escape first, then its close button or a closing action.
    /// A job still being prepared in the background is waited for, so no window of its own can
    /// come up after this returns. Returns how many rounds it took.
    #[track_caller]
    pub fn close_dialogs(&mut self) -> usize {
        let mut rounds = 0;
        for _ in 0..6 {
            if !self.dialog_open() {
                if !self.job_build_pending() {
                    break;
                }
                // A job is being prepared in the background. Its progress window, or the
                // window its result opens, would come up later, at a moment that depends on
                // how fast the computer is. Let it end, then look again.
                self.wait_for_job_build();
                continue;
            }
            rounds += 1;
            self.escape();
            if !self.dialog_open() {
                break;
            }
            for name in [
                "dialog:cancel",
                "dialog:close",
                "dialog:button:Close",
                "dialog:button:Cancel",
                "dialog:button:OK",
                "dialog:ok",
                "dialog:button:No",
                "dialog:action:Close",
            ] {
                if self.rect_of(name).is_some() {
                    self.click_id(name);
                    break;
                }
            }
            if self.dialog_open() {
                // Floating windows (Preview...): a button named for closing, in the accessibility tree.
                for label in ["Close", "Cancel", "OK", "Exit", "Done", "No", "Discard", "Don't save"] {
                    if let Some(n) = self.find_a11y(Some("Button"), label).last().cloned() {
                        self.click_pos(n.rect.center());
                        break;
                    }
                }
            }
            // Cancel only asks the worker to stop; the progress window stays until it has.
            self.wait_for_job_build();
        }
        assert!(
            !self.dialog_open(),
            "a dialog would not close; windows: {:?} {:?} texts: {:?}",
            self.window_titles(),
            self.dialog_windows(),
            self.visible_texts()
        );
        rounds
    }

    /// Queue the answer of the next file dialog (an empty list cancels it).
    pub fn answer_dialog(&mut self, path: impl AsRef<Path>) -> &mut Self {
        darkburn_dialogs_answer(vec![path.as_ref().to_path_buf()]);
        self
    }

    /// The file dialogs the app asked for since the last call.
    pub fn dialog_log(&self) -> Vec<String> {
        darkburn_dialogs_log()
    }

    /// A path in this test's temp dir.
    pub fn temp(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }
}

impl Drop for Gui {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

// The dialog seam is the crate the app calls `rfd`.
fn darkburn_dialogs_script() {
    ::rfd::script_begin();
}

fn darkburn_dialogs_answer(paths: Vec<PathBuf>) {
    ::rfd::script_answer(paths);
}

fn darkburn_dialogs_log() -> Vec<String> {
    ::rfd::script_take_log().into_iter().map(|r| format!("{} {:?}", r.kind, r.file_name)).collect()
}

/// The window shows the app's clear colour where nothing is painted (`App::clear_color`); the
/// offscreen render leaves those pixels transparent, so lay it over the same colour.
fn over_clear_colour(mut img: image::RgbaImage) -> image::RgbaImage {
    let c = solidcraft_theme::Tokens::get().app_bar;
    for p in img.pixels_mut() {
        let a = p.0[3] as u32;
        if a == 255 {
            continue;
        }
        let mix = |top: u8, under: u8| (top as u32 + under as u32 * (255 - a) / 255).min(255) as u8; // premultiplied
        *p = image::Rgba([mix(p.0[0], c.r()), mix(p.0[1], c.g()), mix(p.0[2], c.b()), 255]);
    }
    img
}

/// A widget rectangle that is not wholly inside `bounds` (allowing half a point of rounding).
pub fn outside(bounds: Rect, r: Rect) -> bool {
    !bounds.expand(0.5).contains_rect(r)
}
