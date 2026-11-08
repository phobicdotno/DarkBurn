//! DarkBurn: laser cutter and engraver. The desktop host: window, settings and startup.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod actions;
mod api;
mod canvas;
mod canvas_input;
mod commands;
mod cut_settings;
mod cuts_panel;
mod files;
mod import;
mod job_build;
mod job_cmds;
mod job_ui;
mod material_test;
#[cfg(target_os = "windows")]
mod native_frame;
mod numeric;
mod numeric_edits;
mod palette;
mod panels;
mod prefs;
mod preview;
mod render;
mod ribbon;
mod settings;
mod shape_props;
mod shell;
mod startup;
mod thumbnail;
mod tools;
mod view;
#[cfg(any(target_os = "windows", test))]
mod window_fit;

use prefs::{Prefs, STORAGE_KEY};
use shell::DarkBurn;

const APP_NAME: &str = "DarkBurn";

/// The settings folder: `%APPDATA%\DarkBurn\data` on Windows.
fn settings_dir() -> Option<std::path::PathBuf> {
    eframe::storage_dir(APP_NAME)
}

/// Windows: put the restored window back inside the monitor's work area if it is out of reach.
/// Adapted from SolidCraft's main.rs (MIT OR Apache-2.0).
#[cfg(target_os = "windows")]
fn fit_window_to_work_area(ctx: &egui::Context, frame: &eframe::Frame) {
    use window_fit::Area;
    let Some((x, y, w, h)) = native_frame::work_area(frame) else {
        return;
    };
    let (outer, inner, scale, maximized) = ctx.input(|i| {
        let v = i.viewport();
        (v.outer_rect, v.inner_rect, v.native_pixels_per_point, v.maximized.unwrap_or(false) || v.fullscreen.unwrap_or(false))
    });
    let (Some(outer), Some(inner), Some(scale)) = (outer, inner, scale.filter(|s| s.is_finite() && *s > 0.0)) else {
        return;
    };
    if maximized {
        return;
    }
    let work = Area::new(x as f32 / scale, y as f32 / scale, w as f32 / scale, h as f32 / scale);
    let now = Area::new(outer.min.x, outer.min.y, outer.width(), outer.height());
    let fitted = window_fit::fit(now, work, (960.0, 600.0));
    if fitted != now {
        let inner_size = egui::vec2((inner.width() - (now.w - fitted.w)).max(1.0), (inner.height() - (now.h - fitted.h)).max(1.0));
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(inner_size));
        ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(fitted.x, fitted.y)));
    }
}

/// Settings written soon after they change, at most once a second, instead of eframe's 30 s
/// timer, so a change survives the app being killed. The pattern is SolidCraft's `PrefsWriter`.
#[derive(Default)]
struct PrefsWriter {
    written: Option<String>,
    input_before: bool,
    last_write: Option<std::time::Instant>,
    pending_until: Option<std::time::Instant>,
}

impl PrefsWriter {
    const INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

    fn read(prefs: String) -> Self {
        Self { written: Some(prefs), ..Self::default() }
    }

    fn due(&mut self, input: bool, now: std::time::Instant) -> bool {
        let due = input || self.input_before || self.pending_until.is_some_and(|at| now >= at);
        self.input_before = input;
        due
    }

    fn store(&mut self, storage: &mut dyn eframe::Storage, prefs: String, now: std::time::Instant) -> Option<std::time::Duration> {
        if self.written.as_deref() == Some(prefs.as_str()) {
            self.pending_until = None;
            return None;
        }
        if let Some(wait) = self.last_write.map(|at| Self::INTERVAL.saturating_sub(now.saturating_duration_since(at))).filter(|w| !w.is_zero()) {
            self.pending_until = Some(now + wait);
            return Some(wait);
        }
        storage.set_string(STORAGE_KEY, prefs.clone());
        storage.flush();
        self.written = Some(prefs);
        self.last_write = Some(now);
        self.pending_until = None;
        None
    }
}

/// Input that can change a setting: a click, a key, typed text, a wheel or a pinch.
fn changes_settings(event: &egui::Event) -> bool {
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

struct App {
    app: DarkBurn,
    writer: PrefsWriter,
    frames: u32,
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if self.frames < 2 {
            self.frames += 1;
            #[cfg(target_os = "windows")]
            if self.frames == 1 {
                fit_window_to_work_area(ctx, frame);
            }
            ctx.request_repaint();
            if self.frames == 2
                && let Some(dir) = &self.app.settings_dir
            {
                // The window is up: this start did not fail.
                startup::started(dir);
            }
        }
        self.app.apply_theme(ctx);
        self.app.shortcuts(ctx);
        if self.app.safe_mode {
            return;
        }
        let input = ctx.input(|i| i.events.iter().any(changes_settings));
        let now = std::time::Instant::now();
        if self.writer.due(input, now)
            && let Some(storage) = frame.storage_mut()
            && let Some(wait) = self.writer.store(storage, self.app.prefs.to_json(), now)
        {
            ctx.request_repaint_after(wait);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.app.ui(ui);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        if !self.app.safe_mode {
            storage.set_string(STORAGE_KEY, self.app.prefs.to_json());
        }
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        solidcraft_theme::Tokens::get().app_bar.to_normalized_gamma_f32()
    }
}

/// The window icon: DarkBurn's mark rendered from its SVG master.
fn app_icon() -> egui::IconData {
    let px = 64;
    let r = darkburn_ui::icon_svg::resolve("db_app");
    let look = darkburn_ui::icon_svg::Look { dark: true, accent: r.accent, vision: solidcraft_theme::ColorVision::Off, palette: None };
    let rgba = darkburn_ui::icon_svg::doc_for(r.master, px)
        .and_then(|doc| darkburn_ui::icon_svg::render(doc, look, px))
        .map(|pm| {
            pm.pixels()
                .iter()
                .flat_map(|p| {
                    let c = p.demultiply();
                    [c.red(), c.green(), c.blue(), c.alpha()]
                })
                .collect()
        })
        .unwrap_or_else(|| vec![0; (px * px * 4) as usize]);
    egui::IconData { rgba, width: px, height: px }
}

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = settings_dir();
    let mut notes = Vec::new();
    let mut safe_mode = args.iter().any(|a| a == "--safe-mode") || startup::env_on(std::env::var(startup::SAFE_MODE_ENV).ok().as_deref());
    if let Some(dir) = &dir {
        // Settings that do not read are moved aside before eframe sees them.
        notes.extend(startup::check_settings(dir, &[STORAGE_KEY]));
        let failed = startup::begin(dir);
        if failed >= startup::FAILED_STARTS_FOR_SAFE_MODE {
            safe_mode = true;
            notes.push(format!("DarkBurn did not start the last {failed} times; started with default settings."));
        }
    }
    if safe_mode && notes.is_empty() {
        notes.push("Safe mode: default settings, nothing is saved.".into());
    }
    let custom_titlebar = !cfg!(target_os = "macos");
    let viewport = egui::ViewportBuilder::default()
        .with_title(APP_NAME)
        .with_icon(app_icon())
        .with_inner_size([1600.0, 1000.0])
        .with_min_inner_size([960.0, 600.0])
        .with_drag_and_drop(true)
        .with_decorations(!custom_titlebar)
        .with_fullsize_content_view(true)
        .with_titlebar_shown(false)
        .with_title_shown(false)
        .with_app_id("no.phobic.darkburn");
    let options = eframe::NativeOptions {
        viewport,
        persistence_path: if safe_mode { None } else { dir.as_ref().map(|d| d.join("app.ron")) },
        persist_window: !safe_mode,
        ..Default::default()
    };
    eframe::run_native(
        APP_NAME,
        options,
        Box::new(move |cc| {
            darkburn_ui::fonts::setup_bundled_fonts(&cc.egui_ctx);
            let stored = if safe_mode { None } else { cc.storage.and_then(|s| s.get_string(STORAGE_KEY)) };
            let prefs = stored.as_deref().map(Prefs::from_json).unwrap_or_default();
            let mut app = DarkBurn::new(prefs);
            app.settings_dir = dir.clone();
            app.safe_mode = safe_mode;
            app.note = (!notes.is_empty()).then(|| notes.join(" "));
            app.custom_titlebar = custom_titlebar;
            #[cfg(target_os = "windows")]
            {
                if !native_frame::set_class_icon(cc) {
                    log::info!("no class icon; the taskbar may show a generic one");
                }
                app.native_resize_border = custom_titlebar && native_frame::install(cc);
            }
            if let Some(path) = args.iter().find(|a| !a.starts_with('-'))
                && let Err(e) = files::open_path(&mut app, std::path::Path::new(path))
            {
                app.note = Some(e);
            }
            app.apply_theme(&cc.egui_ctx);
            let writer = PrefsWriter::read(app.prefs.to_json());
            Ok(Box::new(App { app, writer, frames: 0 }))
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Disk(std::collections::HashMap<String, String>, usize);

    impl eframe::Storage for Disk {
        fn get_string(&self, key: &str) -> Option<String> {
            self.0.get(key).cloned()
        }
        fn set_string(&mut self, key: &str, value: String) {
            self.0.insert(key.to_string(), value);
        }
        fn remove_string(&mut self, key: &str) {
            self.0.remove(key);
        }
        fn flush(&mut self) {
            self.1 += 1;
        }
    }

    #[test]
    fn prefs_writer_flushes_changes_at_most_once_a_second() {
        let start = std::time::Instant::now();
        let mut disk = Disk::default();
        let mut w = PrefsWriter::read("a".into());
        assert_eq!(w.store(&mut disk, "a".into(), start), None, "unchanged settings are not written");
        assert_eq!(disk.1, 0);
        assert_eq!(w.store(&mut disk, "b".into(), start), None);
        assert_eq!((disk.0.get(STORAGE_KEY).map(String::as_str), disk.1), (Some("b"), 1));
        let wait = w.store(&mut disk, "c".into(), start + std::time::Duration::from_millis(200));
        assert!(wait.is_some(), "a second change within the interval waits");
        assert_eq!(disk.1, 1);
        assert!(w.due(false, start + std::time::Duration::from_millis(1300)));
        assert_eq!(w.store(&mut disk, "c".into(), start + std::time::Duration::from_millis(1300)), None);
        assert_eq!((disk.0.get(STORAGE_KEY).map(String::as_str), disk.1), (Some("c"), 2));
    }

    #[test]
    fn a_damaged_settings_file_is_moved_aside() {
        let dir = std::env::temp_dir().join(format!("darkburn-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("app.ron"), "{ not ron").unwrap();
        let notes = startup::check_settings(&dir, &[STORAGE_KEY]);
        assert_eq!(notes.len(), 1);
        assert!(!dir.join("app.ron").exists());
        assert_eq!(startup::begin(&dir), 0);
        assert_eq!(startup::begin(&dir), 1, "a start that never got its window up is counted");
        startup::started(&dir);
        assert_eq!(startup::begin(&dir), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn window_icon_is_drawn() {
        let icon = app_icon();
        assert_eq!(icon.rgba.len(), 64 * 64 * 4);
        assert!(icon.rgba.chunks(4).filter(|p| p[3] > 0).count() > 500);
    }
}
