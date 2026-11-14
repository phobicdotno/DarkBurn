//! Everything the app shell added around the core: the state of the library panels, the hotkey
//! editor and the help window, the per-frame application of the settings, the commands for
//! help, preferences files, libraries and hotkeys, and the shared defaults file.

use std::path::{Path, PathBuf};

use darkburn_ui::ui_kit::{self, BackingTooltipExt as _};
use egui::{Context, RichText, Ui};
use serde_json::{Value, json};
use solidcraft_theme::Tokens;

use crate::art_library::{self, ArtState, OnDuplicate};
use crate::i18n::{self, tr};
use crate::keymap::{self, Refused};
use crate::library::{self, LibraryState};
use crate::shell::DarkBurn;
use crate::{help, options};

/// The shared defaults file a workshop puts next to the program or in the settings folder.
pub const DEFAULTS_FILE: &str = "defaults.dbprefs";
pub const DEFAULTS_EXTENSION: &str = "dbprefs";

#[derive(Default)]
pub struct HotkeyUi {
    pub open: bool,
    pub filter: String,
    /// The command waiting for a key press.
    pub capturing: Option<String>,
    /// A key another command uses: (command being set, key text, command that has it).
    pub conflict: Option<(String, String, &'static str)>,
    pub status: String,
}

#[derive(Default)]
pub struct Extras {
    pub library: LibraryState,
    pub art: ArtState,
    pub hotkeys: HotkeyUi,
    pub help_open: bool,
    pub help_topic: String,
    /// The panel the pointer was over last frame (for F1).
    pub hover_topic: Option<&'static str>,
    last_hover: Option<&'static str>,
    pub support_status: Option<String>,
    pub updates: crate::updates::State,
    pub updater: crate::updater::State,
    locks_loaded: bool,
}

impl Extras {
    /// Whether the hotkey editor is waiting for a key (the shortcut handler must not run).
    pub fn capturing(&self) -> bool {
        self.hotkeys.capturing.is_some()
    }
}

/// Once a frame, before anything is drawn: share the settings with the code that reads them
/// (renderer, canvas input, menus) and apply the ones egui itself owns.
pub fn frame(ctx: &Context, app: &mut DarkBurn) {
    app.prefs.opts.sanitise();
    if ui_kit::number_parser(ctx).is_none() {
        // Sums and units in every number field of the dialogs and panels [CUT-10].
        ui_kit::install_number_parser(ctx, std::sync::Arc::new(|text: &str, unit_mm: f64| darkburn_core::expr::eval_in(text, unit_mm)));
    }
    options::publish(&app.prefs.opts);
    keymap::publish(&app.prefs.hotkeys);
    i18n::set_language(&app.prefs.language);
    crate::language_fonts::frame(ctx, &i18n::language());
    let o = &app.prefs.opts;
    let zoom = o.ui_scale / 100.0;
    if (ctx.zoom_factor() - zoom).abs() > 1e-3 {
        ctx.set_zoom_factor(zoom);
    }
    ctx.tessellation_options_mut(|t| t.feathering = o.antialias);
    let animation = if o.reduce_motion { 0.0 } else { 1.0 / 12.0 };
    ctx.all_styles_mut(|s| s.animation_time = animation);
    if !app.extras.locks_loaded {
        app.extras.locks_loaded = true;
        if o.remember_locks {
            app.locks = o.locks;
        }
    } else if o.remember_locks && app.locks != o.locks {
        app.prefs.opts.locks = app.locks;
    }
    let dir = app.settings_dir.clone();
    app.extras.library.autosave(dir.as_deref(), std::time::Instant::now());
    art_library::frame(app);
    app.extras.last_hover = app.extras.hover_topic.take();
    crate::updates::frame(ctx, app);
    crate::updater::frame(ctx, app);
    if app.extras.library.is_dirty() || !app.extras.art.libs.is_empty() {
        ctx.request_repaint_after(std::time::Duration::from_secs(5));
    }
}

// ----- Shared defaults -----

/// The shared defaults file, if one exists: `$DARKBURN_DEFAULTS`, next to the program, or in the
/// settings folder (in that order).
pub fn shared_defaults(settings_dir: Option<&Path>) -> Option<crate::prefs::Prefs> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(p) = std::env::var_os("DARKBURN_DEFAULTS") {
        candidates.push(PathBuf::from(p));
    }
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
        candidates.push(dir.join(DEFAULTS_FILE));
    }
    if let Some(dir) = settings_dir {
        candidates.push(dir.join(DEFAULTS_FILE));
    }
    let mut prefs = candidates.into_iter().filter(|p| p.is_file()).find_map(|p| read_prefs_file(&p).ok())?;
    // AI agent access is switched on by the person at this computer, never by a file.
    prefs.mcp = Default::default();
    // So is the pendant: it opens a USB device.
    prefs.opts.pendant = false;
    Some(prefs)
}

/// Preferences from a `.dbprefs` file; an error when it is not a preferences object.
pub fn read_prefs_file(path: &Path) -> Result<crate::prefs::Prefs, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    match serde_json::from_str::<Value>(&text) {
        Ok(v) if v.is_object() => Ok(crate::prefs::Prefs::from_json(&text)),
        _ => Err(tr("prefs.bad_file", "This is not a preferences file")),
    }
}

// ----- Commands -----

/// Commands this file runs from menus and buttons; false when `id` is not one of them.
pub fn run(app: &mut DarkBurn, ctx: &Context, id: &str) -> bool {
    match id {
        "help.quick" => {
            let topic = if app.settings_open {
                "settings"
            } else if app.extras.hotkeys.open {
                "hotkeys"
            } else if app.cut_settings_open.is_some() {
                "cut_settings"
            } else {
                app.extras.last_hover.unwrap_or("canvas")
            };
            app.extras.help_topic = topic.to_string();
            app.extras.help_open = true;
        }
        "help.docs" => help::open_url(help::DOCS_URL),
        "help.support_data" => {
            if let Some(path) = rfd::FileDialog::new().add_filter("Zip", &["zip"]).set_file_name("darkburn-support.zip").save_file() {
                app.extras.support_status = Some(match help::write_support_zip(&path, &app.prefs.to_json(), app.settings_dir.as_deref()) {
                    Ok(()) => format!("{} {}", tr("help.support_saved", "Support data saved to"), path.display()),
                    Err(e) => e,
                });
            }
        }
        "help.camera" => {
            app.extras.help_topic = "camera_selection".into();
            app.extras.help_open = true;
        }
        "help.updates" | "help.check_updates" => crate::updates::run_command(app),
        "help.debug_log" => {
            let on = !help::debug_on();
            help::set_debug(on, app.settings_dir.as_deref());
            app.note = Some(if on { tr("help.debug_on", "Debug log is on.") } else { tr("help.debug_off", "Debug log is off.") });
        }
        "file.hotkeys" => app.extras.hotkeys.open = true,
        _ => {
            let _ = ctx;
            return false;
        }
    }
    true
}

// ----- Commands with arguments -----

fn num(a: &Value, key: &str) -> Result<usize, String> {
    a.get(key).and_then(Value::as_u64).map(|v| v as usize).ok_or_else(|| format!("argument {key} must be a whole number"))
}

fn txt<'a>(a: &'a Value, key: &str) -> Result<&'a str, String> {
    a.get(key).and_then(Value::as_str).ok_or_else(|| format!("argument {key} must be text"))
}

/// The layer a cut library command names: one that exists (a larger number is not taken for
/// the last tool layer).
fn layer_arg(a: &Value) -> Result<usize, String> {
    let layer = num(a, "layer")?;
    if layer >= darkburn_core::LAYER_COUNT {
        return Err(format!("layer {layer} does not exist"));
    }
    Ok(layer)
}

/// The cut library's file is the one opened for its device from now on, as after the panel's
/// Load and Save as.
fn remember_library_file(app: &mut DarkBurn) {
    let st = &app.extras.library;
    if let Some(p) = &st.path {
        app.prefs.library_paths.insert(st.device.clone(), p.to_string_lossy().into_owned());
    }
}

/// The commands for libraries, art and hotkeys that take arguments; None for any other id.
pub fn with_args(app: &mut DarkBurn, id: &str, a: &Value) -> Option<Result<Value, String>> {
    if id.starts_with("library.") {
        // The device's own library, as the panel would show it.
        library::ensure_loaded(app);
    }
    Some(match id {
        "library.new" => {
            app.extras.library.lib = library::Library { device: app.extras.library.device.clone(), ..Default::default() };
            app.extras.library.path = None;
            app.extras.library.selected = None;
            app.extras.library.editing = None;
            app.extras.library.dirty_since = None;
            app.extras.library.touch();
            // As in the panel: the file of the library before is no longer the device's.
            app.prefs.library_paths.remove(&app.extras.library.device);
            Ok(json!({ "ok": true }))
        }
        "library.open" => txt(a, "path").and_then(|p| app.extras.library.open(Path::new(p))).map(|_| {
            remember_library_file(app);
            json!({ "entries": app.extras.library.lib.entries.len() })
        }),
        "library.save" => app.extras.library.save().map(|_| json!({ "ok": true })),
        "library.save_as" => txt(a, "path").and_then(|p| app.extras.library.save_as(Path::new(p))).map(|_| {
            remember_library_file(app);
            json!({ "ok": true })
        }),
        "library.list" => Ok(json!({
            "name": app.extras.library.lib.name,
            "entries": app.extras.library.lib.entries.iter().map(|e| json!({ "name": e.link_name(), "mode": e.mode().label() })).collect::<Vec<_>>(),
        })),
        "library.create_from_layer" => (|| {
            let layer = layer_arg(a)?;
            let thickness = match a.get("thickness") {
                Some(Value::String(t)) => library::parse_thickness(t, app.prefs.numeric_inches),
                Some(v) => v.as_f64(),
                None => None,
            };
            let i = library::create_from_layer(app, layer, txt(a, "material").unwrap_or(""), thickness, txt(a, "description").unwrap_or(""));
            Ok(json!({ "entry": i, "name": app.extras.library.lib.entries[i].link_name() }))
        })(),
        "library.assign" => (|| library::assign_to_layer(app, num(a, "entry")?, layer_arg(a)?).map(|_| json!({ "ok": true })))(),
        "library.link" => (|| library::link_layer(app, num(a, "entry")?, layer_arg(a)?).map(|_| json!({ "ok": true })))(),
        "library.unlink" => (|| {
            library::unlink_layer(app, layer_arg(a)?);
            Ok(json!({ "ok": true }))
        })(),
        "library.delete" => (|| library::delete_entry(app, num(a, "entry")?).map(|_| json!({ "ok": true })))(),
        "library.duplicate" => (|| library::duplicate_entry(app, num(a, "entry")?).map(|i| json!({ "entry": i })))(),
        "art.new" => Ok(json!({ "library": app.extras.art.new_library() })),
        "art.open" => txt(a, "path").and_then(|p| app.extras.art.open(Path::new(p))).map(|i| json!({ "library": i })),
        "art.save_as" => (|| {
            let l = num(a, "library")?;
            let path = txt(a, "path")?;
            app.extras.art.libs.get_mut(l).ok_or("no such library")?.save_as(Path::new(path)).map(|_| json!({ "ok": true }))
        })(),
        "art.add_selection" => (|| {
            let on = if a.get("replace").and_then(Value::as_bool).unwrap_or(false) { OnDuplicate::Replace } else { OnDuplicate::KeepBoth };
            let i = art_library::add_selection(
                app,
                num(a, "library")?,
                a.get("folder").and_then(Value::as_u64).unwrap_or(0) as usize,
                txt(a, "name")?,
                on,
            )?;
            Ok(json!({ "item": i }))
        })(),
        "art.insert" => (|| {
            let n =
                art_library::insert_item(app, num(a, "library")?, a.get("folder").and_then(Value::as_u64).unwrap_or(0) as usize, num(a, "item")?)?;
            Ok(json!({ "inserted": n }))
        })(),
        "hotkeys.set" => (|| {
            let take = a.get("take").and_then(Value::as_bool).unwrap_or(false);
            match keymap::assign(&mut app.prefs.hotkeys, txt(a, "command")?, txt(a, "key")?, take) {
                Ok(()) => Ok(json!({ "ok": true })),
                Err(Refused::UnknownCommand) => Err("unknown command".into()),
                Err(Refused::BadKey) => Err("that is not a key".into()),
                Err(Refused::Taken(other)) => Err(format!("the key is used by {other}; pass take to move it")),
            }
        })(),
        "hotkeys.reset" => {
            match a.get("command").and_then(Value::as_str) {
                Some(c) => keymap::reset(&mut app.prefs.hotkeys, c),
                None => app.prefs.hotkeys.clear(),
            }
            Ok(json!({ "ok": true }))
        }
        "hotkeys.list" => Ok(json!({
            "keys": keymap::effective(&app.prefs.hotkeys),
            "conflicts": keymap::conflicts(&app.prefs.hotkeys).into_iter().map(|(k, ids)| json!({ "key": k, "commands": ids })).collect::<Vec<_>>(),
        })),
        "hotkeys.export" => txt(a, "path")
            .and_then(|p| std::fs::write(p, keymap::to_file(&app.prefs.hotkeys)).map_err(|e| e.to_string()))
            .map(|_| json!({ "ok": true })),
        "hotkeys.import" => txt(a, "path").and_then(|p| {
            let text = std::fs::read_to_string(p).map_err(|e| e.to_string())?;
            app.prefs.hotkeys = keymap::from_file(&text)?;
            Ok(json!({ "ok": true, "conflicts": keymap::conflicts(&app.prefs.hotkeys).len() }))
        }),
        "help.topic" => {
            let t = help::topic(txt(a, "topic").unwrap_or("general"));
            Ok(json!({ "id": t.id, "title": t.title, "text": t.paragraphs }))
        }
        "support.write" => txt(a, "path")
            .and_then(|p| help::write_support_zip(Path::new(p), &app.prefs.to_json(), app.settings_dir.as_deref()))
            .map(|_| json!({ "ok": true })),
        _ => return None,
    })
}

/// What a command gets that tries to switch the pendant: only the person at this computer does
/// that, in the Settings window, because it opens a USB device and makes its keys live.
pub const PENDANT_ONLY_BY_THE_USER: &str = "the pendant can only be switched on or off by the user in DarkBurn's Settings window";

/// Set any field of the options by name (the `setting.set` command falls back to this).
pub fn set_option(app: &mut DarkBurn, key: &str, value: &Value) -> Result<(), String> {
    if key == "pendant" {
        return Err(PENDANT_ONLY_BY_THE_USER.into());
    }
    let mut all = serde_json::to_value(&app.prefs.opts).map_err(|e| e.to_string())?;
    let field = all.get_mut(key).ok_or_else(|| format!("unknown setting {key}"))?;
    if std::mem::discriminant(field) != std::mem::discriminant(value) && !(field.is_number() && value.is_number()) {
        return Err(format!("{key} needs a different kind of value"));
    }
    *field = value.clone();
    let mut new: options::Options = serde_json::from_value(all).map_err(|e| format!("{key}: {e}"))?;
    new.sanitise();
    // Whatever name it came under, no command changes the pendant switch.
    new.pendant = app.prefs.opts.pendant;
    app.prefs.opts = new;
    if key == "show_rotary_toggles" {
        // The rotary and cylinder toggles are the Modes toolbar.
        app.prefs.toolbars.insert("modes".into(), app.prefs.opts.show_rotary_toggles);
    }
    Ok(())
}

// ----- Windows -----

/// The hotkey editor and the help window.
pub fn dialogs(ctx: &Context, app: &mut DarkBurn) {
    crate::updates::dialog(ctx, app);
    crate::updater::dialogs(ctx, app);
    if app.extras.help_open {
        let topic = help::topic(&app.extras.help_topic);
        let (closed, action) = ui_kit::Window::new(egui::Id::unique("db_help"), "help", &tr("help.title", "Help"))
            .dockable("db_help")
            .actions("", &tr("ui.close", "Close"), None)
            .show(ctx, true, |ui| {
                let t = Tokens::get();
                ui.set_min_width(380.0);
                ui.label(RichText::new(&topic.title).font(darkburn_ui::fonts::semibold(t.font_subtitle)));
                ui.add_space(t.space_6);
                for p in &topic.paragraphs {
                    ui.add(egui::Label::new(p.as_str()).wrap());
                    ui.add_space(t.space_6);
                }
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(tr("help.other_topics", "Other topics:")).color(t.text_dim));
                    for id in help::TOPICS {
                        let title = help::topic(id).title;
                        if ui.small_button(title).clicked() {
                            app.extras.help_topic = id.to_string();
                        }
                    }
                });
            });
        if closed || action != ui_kit::Action::None {
            app.extras.help_open = false;
        }
    }
    if app.extras.hotkeys.open {
        let mut drawn = false;
        let (closed, action) = ui_kit::Window::new(egui::Id::unique("db_hotkeys"), "settings", &tr("hotkeys.title", "Edit Hotkeys"))
            .dockable("db_hotkeys")
            .cards(2.7)
            .fields()
            .actions("", &tr("ui.close", "Close"), None)
            .show(ctx, true, |ui| {
                drawn = true;
                // The key comes from the window the page is in: a window of its own has keys of
                // its own. Taken before the footer's, so Escape ends the wait, not the window.
                capture_key(ui.ctx(), app);
                hotkey_page(ui, app);
            });
        if !drawn {
            // A tab behind another one: a wait nobody can see would take the next key pressed.
            app.extras.hotkeys.capturing = None;
        }
        if closed || action != ui_kit::Action::None {
            app.extras.hotkeys.open = false;
            app.extras.hotkeys.capturing = None;
        }
    }
    if let Some(s) = &app.extras.support_status {
        app.note = Some(s.clone());
        app.extras.support_status = None;
    }
}

/// While a command waits for a key: take the next key press in the window `ctx` draws now.
fn capture_key(ctx: &Context, app: &mut DarkBurn) {
    let Some(cmd) = app.extras.hotkeys.capturing.clone() else { return };
    let pressed = ctx.input_mut(|i| {
        let found = i.events.iter().find_map(|e| match e {
            egui::Event::Key { key, physical_key, pressed: true, modifiers, repeat: false } => Some((*key, *physical_key, *modifiers)),
            _ => None,
        });
        if found.is_some() {
            i.events.clear();
        }
        found
    });
    let Some((key, physical, mods)) = pressed else { return };
    let hk = &mut app.extras.hotkeys;
    if key == egui::Key::Escape && !mods.any() {
        hk.capturing = None;
        return;
    }
    // A number pad key (NumLock off) is kept as that key, so it does not take the arrow's place.
    let text = match keymap::numpad_key(key, physical) {
        Some(pad) if !mods.command && !mods.alt => keymap::numpad_text(pad),
        _ => keymap::key_text(mods, key),
    };
    match keymap::assign(&mut app.prefs.hotkeys, &cmd, &text, false) {
        Ok(()) => {
            hk.status = text;
            hk.capturing = None;
        }
        Err(Refused::Taken(other)) => {
            hk.conflict = Some((cmd, text, other));
            hk.capturing = None;
        }
        Err(Refused::BadKey) => hk.status = tr("hotkeys.bad_key", "Not a key"),
        Err(Refused::UnknownCommand) => hk.capturing = None,
    }
}

/// Width of a key column in the hotkey editor.
const KEY_CELL: f32 = 120.0;

/// One key of a command in the hotkey editor (`key`, what it has now): click it, then press the
/// key; right-click removes just this key.
fn key_cell(ui: &mut Ui, app: &mut DarkBurn, slot: &str, key: String) {
    let waiting = app.extras.hotkeys.capturing.as_deref() == Some(slot);
    let text = if waiting {
        tr("hotkeys.press", "Press a key...")
    } else if key.is_empty() {
        tr("hotkeys.none", "none")
    } else {
        key
    };
    ui.allocate_ui_with_layout(egui::vec2(KEY_CELL, 22.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
        ui.set_width(KEY_CELL);
        let r = ui_kit::button(ui, text, ui_kit::Button::Small)
            .backing_on_hover_text(tr("hotkeys.key_tip", "Click, then press the new key. Esc cancels. Right-click removes the key."));
        if r.clicked() {
            app.extras.hotkeys.capturing = Some(slot.to_string());
            app.extras.hotkeys.conflict = None;
        }
        if r.secondary_clicked() {
            let _ = keymap::assign(&mut app.prefs.hotkeys, slot, "", false);
        }
    });
}

/// A fixed-width column cell with its content at the left.
fn left_cell(ui: &mut Ui, width: f32, height: f32, w: impl egui::Widget) {
    ui.allocate_ui_with_layout(egui::vec2(width, height), egui::Layout::left_to_right(egui::Align::Center), |ui| {
        ui.set_width(width);
        ui.add(w);
    });
}

fn hotkey_page(ui: &mut Ui, app: &mut DarkBurn) {
    let t = Tokens::get();
    ui.set_min_width(660.0);
    let search = tr("hotkeys.search", "Search commands");
    ui_kit::named_control(ui, &search, |ui| ui_kit::hint_field(ui, &mut app.extras.hotkeys.filter, &search));
    ui.add_space(t.space_6);
    let clashes = keymap::conflicts(&app.prefs.hotkeys);
    for (key, ids) in &clashes {
        ui_kit::status_line(ui, &format!("{key}: {} ({})", tr("hotkeys.clash", "used by several commands"), ids.join(", ")), ui_kit::StatusTone::Dim);
    }
    if let Some((cmd, key, other)) = app.extras.hotkeys.conflict.clone() {
        let name = |id: &str| {
            let cmd = keymap::command_of(id);
            crate::commands::find(cmd).map(i18n::cmd_label).unwrap_or_else(|| cmd.to_string())
        };
        ui_kit::status_line(ui, &format!("{key} {} \"{}\".", tr("hotkeys.taken_by", "is already used by"), name(other)), ui_kit::StatusTone::Dim);
        ui.horizontal(|ui| {
            if ui_kit::button(ui, tr("hotkeys.take", "Take the key"), ui_kit::Button::Small).clicked() {
                let _ = keymap::assign(&mut app.prefs.hotkeys, &cmd, &key, true);
                app.extras.hotkeys.conflict = None;
            }
            if ui_kit::button(ui, tr("hotkeys.keep", "Keep as it was"), ui_kit::Button::Small).clicked() {
                app.extras.hotkeys.conflict = None;
            }
        });
    }
    // Folded, so a Turkish "ızgara" finds "Izgara dizisi" (and "izgara" does too).
    let filter = darkburn_ui::casing::fold(&app.extras.hotkeys.filter);
    ui.horizontal(|ui| {
        let head = |ui: &mut Ui, w: f32, text: String| {
            left_cell(ui, w, 18.0, egui::Label::new(RichText::new(text).size(t.font_small).color(t.text_dim)).truncate());
        };
        head(ui, 250.0, tr("hotkeys.command", "Command"));
        head(ui, KEY_CELL, tr("hotkeys.key", "Key"));
        head(ui, KEY_CELL, tr("hotkeys.second", "Second key"));
    });
    // The keys as the key handler sees them: a default another command was given is not shown.
    let eff = keymap::effective(&app.prefs.hotkeys);
    let now = |slot: &str| eff.get(slot).cloned().unwrap_or_default();
    egui::ScrollArea::vertical().max_height(380.0).auto_shrink([false, true]).show(ui, |ui| {
        for (id, label, default) in keymap::bindable() {
            let cmd = crate::commands::find(id);
            let shown = cmd.map(i18n::cmd_label).unwrap_or_else(|| label.to_string());
            if !filter.is_empty() && !darkburn_ui::casing::fold(&shown).contains(&filter) && !id.contains(&filter) {
                continue;
            }
            let second = keymap::second_of(id);
            ui.horizontal(|ui| {
                left_cell(ui, 250.0, 22.0, egui::Label::new(shown).truncate());
                key_cell(ui, app, id, now(id));
                key_cell(ui, app, &second, now(&second));
                if ui_kit::button(ui, tr("hotkeys.clear", "Clear"), ui_kit::Button::Small).clicked() {
                    let _ = keymap::assign(&mut app.prefs.hotkeys, id, "", false);
                    let _ = keymap::assign(&mut app.prefs.hotkeys, &second, "", false);
                }
                let changed = app.prefs.hotkeys.contains_key(id) || app.prefs.hotkeys.contains_key(&second);
                let defaults = [default, keymap::second_default(id)].into_iter().filter(|k| !k.is_empty()).collect::<Vec<_>>().join(", ");
                ui.add_enabled_ui(changed, |ui| {
                    if ui_kit::button(ui, tr("hotkeys.reset", "Default"), ui_kit::Button::Small).backing_on_hover_text(defaults).clicked() {
                        keymap::reset(&mut app.prefs.hotkeys, id);
                    }
                });
            });
        }
    });
    ui.add_space(t.space_6);
    ui.horizontal_wrapped(|ui| {
        if ui_kit::button(ui, tr("hotkeys.export", "Export..."), ui_kit::Button::Small).clicked()
            && let Some(p) =
                rfd::FileDialog::new().add_filter("DarkBurn hotkeys", &[keymap::FILE_EXTENSION]).set_file_name("darkburn.dbhk").save_file()
        {
            app.extras.hotkeys.status = std::fs::write(p, keymap::to_file(&app.prefs.hotkeys)).err().map(|e| e.to_string()).unwrap_or_default();
        }
        if ui_kit::button(ui, tr("hotkeys.import", "Import..."), ui_kit::Button::Small).clicked()
            && let Some(p) = rfd::FileDialog::new().add_filter("DarkBurn hotkeys", &[keymap::FILE_EXTENSION]).pick_file()
        {
            match std::fs::read_to_string(p).map_err(|e| e.to_string()).and_then(|t| keymap::from_file(&t)) {
                Ok(o) => app.prefs.hotkeys = o,
                Err(e) => app.extras.hotkeys.status = e,
            }
        }
        if ui_kit::button(ui, tr("hotkeys.reset_all", "Reset all"), ui_kit::Button::Small).clicked() {
            app.prefs.hotkeys.clear();
        }
    });
    if !app.extras.hotkeys.status.is_empty() {
        ui_kit::status_line(ui, &app.extras.hotkeys.status, ui_kit::StatusTone::Dim);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::execute_headless;
    use crate::prefs::Prefs;

    fn app() -> DarkBurn {
        DarkBurn::new(Prefs::default())
    }

    #[test]
    fn options_can_be_set_by_name_and_wrong_kinds_are_refused() {
        let mut app = app();
        set_option(&mut app, "speed_per_min", &json!(true)).unwrap();
        assert!(app.prefs.opts.speed_per_min);
        set_option(&mut app, "nudge", &json!(2.5)).unwrap();
        assert_eq!(app.prefs.opts.nudge, 2.5);
        set_option(&mut app, "ui_scale", &json!(9999)).unwrap();
        assert_eq!(app.prefs.opts.ui_scale, 250.0, "out-of-range values are pulled in");
        assert!(set_option(&mut app, "nudge", &json!("big")).is_err());
        assert!(set_option(&mut app, "no_such", &json!(1)).is_err());
    }

    #[test]
    fn settings_survive_a_save_and_load_of_the_preferences() {
        let mut p = Prefs { numeric_inches: true, ..Default::default() };
        p.opts.speed_per_min = true;
        p.opts.snap_grid = false;
        p.opts.nudge_shift = 25.0;
        p.opts.excluded_ports = vec!["COM9".into()];
        p.opts.curve_quality = options::CurveQuality::Fine;
        p.hotkeys.insert("file.save".into(), "F2".into());
        let back = Prefs::from_json(&p.to_json());
        assert_eq!(back, p);
        // Settings from an older version (without the new fields) still load.
        let old = Prefs::from_json(r#"{"theme":"dark","svg_dpi":72}"#);
        assert_eq!(old.opts, options::Options::default());
        assert_eq!(old.svg_dpi, 72.0);
    }

    #[test]
    fn shared_defaults_file_is_read_and_a_bad_one_is_not() {
        let dir = std::env::temp_dir().join(format!("darkburn-defaults-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut p = Prefs::default();
        p.opts.speed_per_min = true;
        p.page_width = 600.0;
        std::fs::write(dir.join(DEFAULTS_FILE), p.to_json()).unwrap();
        let got = shared_defaults(Some(&dir)).expect("found in the settings folder");
        assert!(got.opts.speed_per_min);
        assert_eq!(got.page_width, 600.0);
        std::fs::write(dir.join(DEFAULTS_FILE), "[1,2]").unwrap();
        assert!(read_prefs_file(&dir.join(DEFAULTS_FILE)).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn shared_defaults_never_switch_ai_agent_access_on() {
        let dir = std::env::temp_dir().join(format!("darkburn-defaults-mcp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut p = Prefs { page_width: 600.0, ..Default::default() };
        p.mcp.enabled = true;
        p.mcp.allow_hardware = true;
        p.mcp.allow_gweike = true;
        p.mcp.require_confirmation = false;
        std::fs::write(dir.join(DEFAULTS_FILE), p.to_json()).unwrap();
        let got = shared_defaults(Some(&dir)).expect("found in the settings folder");
        assert_eq!(got.page_width, 600.0);
        assert_eq!(got.mcp, crate::mcp_ui::McpPrefs::default(), "a defaults file switched agent access on");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hotkey_commands_remap_detect_conflicts_and_round_trip_files() {
        let mut app = app();
        let r = execute_headless(&mut app, "hotkeys.set", &json!({ "command": "edit.redo", "key": "Ctrl+Z" }));
        assert!(r.unwrap_err().contains("edit.undo"));
        execute_headless(&mut app, "hotkeys.set", &json!({ "command": "edit.redo", "key": "Ctrl+Z", "take": true })).unwrap();
        assert_eq!(keymap::effective(&app.prefs.hotkeys)["edit.undo"], "");
        let dir = std::env::temp_dir().join(format!("darkburn-hk-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("mine.dbhk");
        execute_headless(&mut app, "hotkeys.export", &json!({ "path": f.to_string_lossy() })).unwrap();
        execute_headless(&mut app, "hotkeys.reset", &json!({})).unwrap();
        assert!(app.prefs.hotkeys.is_empty());
        execute_headless(&mut app, "hotkeys.import", &json!({ "path": f.to_string_lossy() })).unwrap();
        assert_eq!(keymap::effective(&app.prefs.hotkeys)["edit.redo"], "Ctrl+Z");
        let list = execute_headless(&mut app, "hotkeys.list", &json!({})).unwrap();
        assert_eq!(list["conflicts"].as_array().unwrap().len(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Edit Hotkeys in a window of its own takes the key pressed there, and not one pressed in
    /// the main window. The key was read from the main window only, so F7 pressed in the
    /// hotkey window was never taken and Delete pressed over the drawing was.
    #[test]
    fn edit_hotkeys_in_a_window_of_its_own_takes_the_key_pressed_there() {
        use darkburn_ui::ui_kit::window_host;
        use egui::{Context, Event, Key, Modifiers, Pos2, RawInput, Rect, vec2};
        let own = std::rc::Rc::new(std::cell::RefCell::new(Vec::<Event>::new()));
        let events = own.clone();
        // Native windows as a desktop backend runs them: each in a pass of its own, with keys of its own.
        Context::set_immediate_viewport_renderer(move |ctx, mut viewport| {
            let info = egui::ViewportInfo { parent: Some(viewport.ids.parent), ..Default::default() };
            let input = RawInput {
                viewport_id: viewport.ids.this,
                viewports: std::iter::once((viewport.ids.this, info)).collect(),
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(700.0, 600.0))),
                events: std::mem::take(&mut *events.borrow_mut()),
                ..Default::default()
            };
            let mut out = ctx.run_ui(input, |ui| (viewport.viewport_ui_cb)(ui));
            out.textures_delta.clear();
        });
        let ctx = Context::default();
        darkburn_ui::fonts::setup_bundled_fonts(&ctx);
        ctx.set_embed_viewports(false);
        window_host::update(&ctx, "db_hotkeys", |p| p.detach());
        let mut app = app();
        app.extras.hotkeys.open = true;
        let frame = |app: &mut DarkBurn, main: Vec<Event>, window: Vec<Event>| {
            *own.borrow_mut() = window;
            let input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1200.0, 800.0))), events: main, ..Default::default() };
            let mut out = ctx.run_ui(input, |ui| dialogs(ui.ctx(), app));
            out.textures_delta.clear();
        };
        let tap =
            |key| [true, false].map(|pressed| Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE }).to_vec();
        frame(&mut app, vec![], vec![]);
        let cmd = "tools.rect";
        app.extras.hotkeys.capturing = Some(cmd.into());
        frame(&mut app, tap(Key::Delete), vec![]);
        assert_eq!(app.extras.hotkeys.capturing.as_deref(), Some(cmd), "a key pressed in the main window is not taken");
        frame(&mut app, vec![], tap(Key::F7));
        assert_eq!(app.extras.hotkeys.capturing, None);
        assert_eq!(keymap::effective(&app.prefs.hotkeys)[cmd], "F7");
        assert!(app.extras.hotkeys.open);
        // Escape in the window ends a wait, not the window.
        app.extras.hotkeys.capturing = Some(cmd.into());
        frame(&mut app, vec![], tap(Key::Escape));
        frame(&mut app, vec![], vec![]);
        assert_eq!((app.extras.hotkeys.capturing.as_deref(), app.extras.hotkeys.open), (None, true));
    }

    #[test]
    fn library_commands_work_without_a_window() {
        let mut app = app();
        execute_headless(&mut app, "layer.set_param", &json!({ "layer": 2, "field": "speed", "value": 33 })).unwrap();
        let made = execute_headless(
            &mut app,
            "library.create_from_layer",
            &json!({ "layer": 2, "material": "Ply", "thickness": "1/8 in", "description": "Cut" }),
        )
        .unwrap();
        assert_eq!(made["entry"], 0);
        execute_headless(&mut app, "library.assign", &json!({ "entry": 0, "layer": 4 })).unwrap();
        assert_eq!(app.editor.doc.layer(4).main().speed, 33.0);
        let dir = std::env::temp_dir().join(format!("darkburn-libcmd-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.dbclb");
        execute_headless(&mut app, "library.save_as", &json!({ "path": f.to_string_lossy() })).unwrap();
        execute_headless(&mut app, "library.new", &json!({})).unwrap();
        assert_eq!(execute_headless(&mut app, "library.list", &json!({})).unwrap()["entries"].as_array().unwrap().len(), 0);
        execute_headless(&mut app, "library.open", &json!({ "path": f.to_string_lossy() })).unwrap();
        let list = execute_headless(&mut app, "library.list", &json!({})).unwrap();
        assert_eq!(list["entries"][0]["name"], "Ply - 3.175 mm - Cut");
        execute_headless(&mut app, "library.delete", &json!({ "entry": 0 })).unwrap();
        assert!(app.editor.doc.layer(4).library_link.is_none());
        assert!(execute_headless(&mut app, "library.assign", &json!({ "entry": 7, "layer": 4 })).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn help_and_support_commands_run_headless() {
        let mut app = app();
        let t = execute_headless(&mut app, "help.topic", &json!({ "topic": "library" })).unwrap();
        assert_eq!(t["id"], "library");
        let dir = std::env::temp_dir().join(format!("darkburn-sup-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let z = dir.join("s.zip");
        execute_headless(&mut app, "support.write", &json!({ "path": z.to_string_lossy() })).unwrap();
        assert!(std::fs::metadata(&z).unwrap().len() > 100);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
