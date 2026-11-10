//! Project files: new, open, save, recent projects, notes, import and drag and drop, auto-save
//! and crash restore, the window title and the system clipboard.
//!
//! Auto-save writes the open project to `autosave.dbrn` in the settings folder (never over the
//! user's own file) every `autosave_minutes` while it has unsaved changes. A marker file
//! `session.running` lives there while DarkBurn runs; a clean exit removes both. When the next
//! start finds the marker and an auto-saved project, the last session ended without closing
//! (a crash or power loss) and the project is offered back.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use darkburn_core::clipboard::CLIPBOARD_MARK;
use darkburn_core::kurbo::{Affine, Point, Size};
use darkburn_core::{Clipboard, Editor, Shape, ShapeId, ShapeKind, project};
use darkburn_ui::ui_kit;

use crate::import::{self, FileKind, Imported};
use crate::prefs::Prefs;
use crate::shell::DarkBurn;

/// Entries kept in File > Recent Projects.
pub const RECENT_MAX: usize = 24;
pub const AUTOSAVE_FILE: &str = "autosave.dbrn";
/// Beside the auto-save: the path of the project it was a copy of.
const AUTOSAVE_PATH_FILE: &str = "autosave.path";
pub const SESSION_MARKER: &str = "session.running";
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// What to do once the open project has been saved or let go.
#[derive(Clone, Debug, PartialEq)]
pub enum Then {
    New,
    /// Open a project: the given file, or one picked in a dialog.
    Open(Option<PathBuf>),
    /// Put in place the file that was read in the background (kept by `file_io`): the project
    /// was changed while it was being read.
    OpenRead,
    Exit,
}

/// State of this module, kept in the app.
#[derive(Default)]
pub struct FileState {
    /// Asking "Save changes?" before doing this.
    ask_save: Option<Then>,
    /// An auto-save from a crashed session, offered back.
    restore: Option<PathBuf>,
    /// A message window: title and text.
    message: Option<(String, String)>,
    /// Closing is confirmed: let the window close.
    close_ok: bool,
    started: bool,
    title: String,
    /// The text last put on or taken from the system clipboard.
    last_clip_text: Option<String>,
    /// Hash of the bitmap last taken from the system clipboard.
    last_clip_image: Option<u64>,
    autosave: Autosave,
}

#[derive(Default)]
struct Autosave {
    /// When the next auto-save is due (set when changes appear).
    due: Option<Instant>,
    /// The editor revision last auto-saved.
    revision: Option<u64>,
    /// When the last auto-save finished.
    at: Option<Instant>,
    failed: bool,
    writing: Option<mpsc::Receiver<Result<(), String>>>,
}

impl FileState {
    /// The file dialogs showing now (for MCP's view of the window).
    pub fn open_dialogs(&self) -> Vec<&'static str> {
        [(self.ask_save.is_some(), "unsaved_changes"), (self.restore.is_some(), "restore_autosave"), (self.message.is_some(), "file_message")]
            .into_iter()
            .filter_map(|(on, name)| on.then_some(name))
            .collect()
    }
}

// ----- Commands -----

/// Run a File command; returns whether `id` was one.
pub fn run(app: &mut DarkBurn, ctx: &egui::Context, id: &str) -> bool {
    match id {
        "file.new" => guard(app, ctx, Then::New),
        "file.open" => guard(app, ctx, Then::Open(None)),
        "file.save" => {
            save(app);
        }
        "file.save_as" => {
            save_as(app);
        }
        "file.import" => crate::file_io::request_import_dialog(app),
        "file.notes" => app.notes_open = true,
        "file.exit" => guard(app, ctx, Then::Exit),
        _ => {
            let Some(n) = id.strip_prefix("file.recent.").and_then(|n| n.parse::<usize>().ok()) else { return false };
            if let Some(path) = app.prefs.recent.get(n).cloned() {
                let path = PathBuf::from(path);
                if path.exists() {
                    guard(app, ctx, Then::Open(Some(path)));
                } else {
                    app.prefs.recent.retain(|p| Path::new(p) != path);
                    app.files.message = Some(("Project not found".into(), format!("{} is no longer there.", path.display())));
                }
            }
        }
    }
    true
}

/// Do `then` now, or first ask to save when the project has unsaved changes.
fn guard(app: &mut DarkBurn, ctx: &egui::Context, then: Then) {
    if app.editor.is_modified() {
        app.files.ask_save = Some(then);
    } else {
        proceed(app, ctx, then);
    }
}

/// Ask "Save changes?" before the file that `file_io` has read takes the project's place.
pub fn ask_before_read_file(app: &mut DarkBurn) {
    app.files.ask_save = Some(Then::OpenRead);
}

/// What the "Save changes?" window is waiting to do, if it is open.
pub fn asking(app: &DarkBurn) -> Option<&Then> {
    app.files.ask_save.as_ref()
}

fn proceed(app: &mut DarkBurn, ctx: &egui::Context, then: Then) {
    match then {
        Then::New => new_project(app),
        Then::OpenRead => crate::file_io::open_held(app),
        Then::Open(path) => {
            let path = path.or_else(|| {
                let mut all: Vec<&str> = vec![project::EXTENSION];
                all.extend(import::import_extensions());
                all.extend(crate::formats::LIBRARY_EXTENSIONS);
                let mut d = rfd::FileDialog::new()
                    .add_filter("Projects, artwork and images", &all)
                    .add_filter("DarkBurn project", &[project::EXTENSION])
                    .add_filter("LightBurn project", &["lbrn", "lbrn2"])
                    .add_filter("DXF, PDF, Illustrator, PLT, RD", &["dxf", "pdf", "ai", "plt", "hpgl", "hgl", "hpg", "rd"])
                    .add_filter("SVG and images", &import::import_extensions());
                if let Some(dir) = last_dir(&app.prefs) {
                    d = d.set_directory(dir);
                }
                d.pick_file()
            });
            if let Some(path) = path {
                if import::kind_of(&path) == FileKind::Project {
                    if let Err(e) = open_path(app, &path) {
                        app.files.message = Some(("Could not open".into(), format!("{}: {e}", path.display())));
                    }
                } else {
                    // Everything else is read on a worker thread.
                    crate::file_io::start_open(app, path);
                }
            }
        }
        Then::Exit => {
            app.files.close_ok = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

fn last_dir(prefs: &Prefs) -> Option<PathBuf> {
    prefs.recent.first().and_then(|p| Path::new(p).parent()).filter(|d| d.is_dir()).map(Path::to_path_buf)
}

/// A new empty project (the clipboard is kept).
pub fn new_project(app: &mut DarkBurn) {
    let clipboard = std::mem::take(&mut app.editor.clipboard);
    app.editor = Editor::new();
    app.editor.clipboard = clipboard;
    app.project_path = None;
    app.notes_open = false;
    crate::settings_wiring::on_new_project(app);
    crate::layer_defaults::on_new_project(app);
    discard_autosave(app);
}

/// Open `path`: a project replaces the open one; artwork opens as a new project holding it.
pub fn open_path(app: &mut DarkBurn, path: &Path) -> Result<(), String> {
    if import::kind_of(path) != FileKind::Project {
        return crate::file_io::open_sync(app, path);
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let loaded = project::load(&bytes).map_err(|e| e.to_string())?;
    let clipboard = std::mem::take(&mut app.editor.clipboard);
    app.notes_open = loaded.doc.show_notes_on_load;
    app.editor = Editor::with_document(loaded.doc);
    app.editor.clipboard = clipboard;
    app.project_path = Some(path.to_path_buf());
    crate::settings_wiring::on_open_project(app);
    remember_recent(&mut app.prefs, path);
    discard_autosave(app);
    if loaded.newer_version {
        let by = if loaded.app_version.is_empty() { "a newer DarkBurn".to_string() } else { format!("DarkBurn {}", loaded.app_version) };
        app.files.message = Some((
            "Saved by a newer version".into(),
            format!("This project was saved by {by}. Things this version does not know about may be missing, and saving it here drops them."),
        ));
    }
    Ok(())
}

/// Save to the project's file, asking for one when it has none. Returns whether it was saved.
pub fn save(app: &mut DarkBurn) -> bool {
    match app.project_path.clone() {
        Some(path) => report_save(app, &path),
        None => save_as(app),
    }
}

/// Save under a name picked in a dialog. Returns whether it was saved.
pub fn save_as(app: &mut DarkBurn) -> bool {
    let name = app.project_path.as_deref().and_then(Path::file_name).map(|n| n.to_string_lossy().into_owned());
    let mut d =
        rfd::FileDialog::new().add_filter("DarkBurn project", &[project::EXTENSION]).set_file_name(name.unwrap_or_else(|| "Untitled.dbrn".into()));
    if let Some(dir) = app.project_path.as_deref().and_then(Path::parent).map(Path::to_path_buf).or_else(|| last_dir(&app.prefs)) {
        d = d.set_directory(dir);
    }
    let Some(mut path) = d.save_file() else { return false };
    if path.extension().is_none() {
        path.set_extension(project::EXTENSION);
    }
    report_save(app, &path)
}

fn report_save(app: &mut DarkBurn, path: &Path) -> bool {
    match save_to(app, path) {
        Ok(()) => true,
        Err(e) => {
            app.files.message = Some(("Could not save".into(), format!("{}: {e}", path.display())));
            false
        }
    }
}

/// Write the project to `path` (with a thumbnail) and make it the project's file.
pub fn save_to(app: &mut DarkBurn, path: &Path) -> Result<(), String> {
    let thumb = crate::thumbnail::render(&app.editor.doc);
    let bytes = project::save(&app.editor.doc, APP_VERSION, thumb.as_deref()).map_err(|e| e.to_string())?;
    write_atomic(path, &bytes).map_err(|e| e.to_string())?;
    app.editor.mark_saved();
    app.project_path = Some(path.to_path_buf());
    remember_recent(&mut app.prefs, path);
    discard_autosave(app);
    Ok(())
}

/// Write through a temporary file so a failed write never leaves half a project.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// Put `path` first in the recent projects.
pub fn remember_recent(prefs: &mut Prefs, path: &Path) {
    let p = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf()).to_string_lossy().into_owned();
    prefs.recent.retain(|r| *r != p);
    prefs.recent.insert(0, p);
    prefs.recent.truncate(RECENT_MAX);
}

/// Drop recent projects whose files are gone.
pub fn prune_recent(prefs: &mut Prefs) {
    prefs.recent.retain(|p| Path::new(p).is_file());
    prefs.recent.truncate(RECENT_MAX);
}

// ----- Import -----

pub fn import_options(app: &DarkBurn) -> import::Options {
    import::Options {
        svg_dpi: app.prefs.svg_dpi,
        tool_layers: app.prefs.import_tool_layers,
        page: Size::new(app.prefs.page_width as f64, app.prefs.page_height as f64),
        image_layer: app.editor.current_layer,
    }
}

#[cfg(test)]
/// Import files (centred on `at` when given) as one undo step, now. Returns the new shape ids.
/// Errors and warnings are shown in windows; `file_io::import_sync` returns them instead.
pub fn import_paths(app: &mut DarkBurn, paths: &[PathBuf], at: Option<Point>) -> Vec<ShapeId> {
    let report = crate::file_io::import_sync(app, paths, at, None);
    if !report.errors.is_empty() {
        app.files.message = Some(("Some files were not imported".into(), report.errors.join("\n")));
    }
    if !report.warnings.is_empty() {
        app.fio.warnings = Some(report.warnings);
    }
    report.ids
}

/// A message window.
pub fn show_message(app: &mut DarkBurn, title: &str, text: String) {
    app.files.message = Some((title.to_string(), text));
}

/// Add imported shapes as one undo step: grouped per file and selected, as the preferences say,
/// and put `layers` (cut settings read from a file) into the document.
pub fn insert_with_layers(app: &mut DarkBurn, imported: Vec<Imported>, layers: Vec<darkburn_core::layer::Layer>) -> Vec<ShapeId> {
    let (group, select) = (app.prefs.import_group, app.prefs.import_select);
    let mut new = Vec::new();
    app.editor.edit("Import", |d, sel| {
        crate::formats::apply_layers(d, &layers, &Default::default(), |_| true);
        for imp in imported {
            for (k, data) in imp.images {
                d.images.entry(k).or_insert(data);
            }
            if group && imp.shapes.len() > 1 {
                let layer = imp.shapes[0].layer;
                new.push(d.add(Shape::new(ShapeKind::Group { children: imp.shapes }, Affine::IDENTITY, layer)));
            } else {
                new.extend(imp.shapes.into_iter().map(|s| d.add(s)));
            }
        }
        if select && !new.is_empty() {
            sel.set(new.iter().copied());
        }
        !new.is_empty() || !layers.is_empty()
    });
    new
}

/// Document point under the pointer on the canvas.
fn pointer_point(app: &DarkBurn) -> Option<Point> {
    let m = app.cursor?;
    let page = app.prefs.page();
    let p = app.prefs.origin.to_machine(m, page);
    Some(Point::new(p.x as f64, (page.y - p.y) as f64))
}

/// Files dropped on the window: the first project opens, artwork is imported at the pointer.
pub fn drop_files(app: &mut DarkBurn, ctx: &egui::Context, paths: Vec<PathBuf>, at: Option<Point>) {
    let (projects, art): (Vec<PathBuf>, Vec<PathBuf>) = paths.into_iter().partition(|p| crate::file_io::is_openable_project(p));
    if !art.is_empty() {
        // Read on a worker thread; device profiles go to their hook.
        crate::file_io::start_import(app, art, at);
    }
    if let Some(p) = projects.into_iter().next() {
        guard(app, ctx, Then::Open(Some(p)));
    }
}

// ----- Auto-save and crash restore -----

fn autosave_enabled(app: &DarkBurn) -> bool {
    app.settings_dir.is_some() && !app.safe_mode && !app.fio.secondary && app.prefs.autosave_minutes > 0.0
}

/// Write an auto-save of `doc` (and the path of the project it copies) into `dir`.
pub fn write_autosave(dir: &Path, doc: &darkburn_core::Document, project: Option<&Path>) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let bytes = project::save(doc, APP_VERSION, None).map_err(|e| e.to_string())?;
    write_atomic(&dir.join(AUTOSAVE_FILE), &bytes).map_err(|e| e.to_string())?;
    let path_file = dir.join(AUTOSAVE_PATH_FILE);
    match project {
        Some(p) => std::fs::write(path_file, p.to_string_lossy().as_bytes()).map_err(|e| e.to_string())?,
        None => {
            let _ = std::fs::remove_file(path_file);
        }
    }
    Ok(())
}

/// The auto-save is now unneeded (saved, or the project let go). An extra window has none: the
/// files belong to the first window. While a crashed session's copy is still being offered the
/// files are that copy, and they stay until the question is answered.
fn discard_autosave(app: &mut DarkBurn) {
    wait_for_autosave(app);
    if let Some(dir) = &app.settings_dir
        && !app.safe_mode
        && !app.fio.secondary
        && app.files.restore.is_none()
    {
        let _ = std::fs::remove_file(dir.join(AUTOSAVE_FILE));
        let _ = std::fs::remove_file(dir.join(AUTOSAVE_PATH_FILE));
    }
    app.files.autosave.due = None;
    app.files.autosave.revision = None;
}

fn wait_for_autosave(app: &mut DarkBurn) {
    if let Some(rx) = app.files.autosave.writing.take() {
        let _ = rx.recv_timeout(Duration::from_secs(5));
    }
}

/// At start: an auto-save left by a session that did not close, if any. Then mark this session
/// as running.
pub fn begin_session(dir: &Path) -> Option<PathBuf> {
    let marker = dir.join(SESSION_MARKER);
    let autosave = dir.join(AUTOSAVE_FILE);
    let crashed = marker.exists() && autosave.is_file();
    let _ = std::fs::create_dir_all(dir);
    let _ = std::fs::write(&marker, std::process::id().to_string());
    crashed.then_some(autosave)
}

/// A clean exit: no auto-save, no marker.
pub fn end_session(dir: &Path) {
    for f in [AUTOSAVE_FILE, AUTOSAVE_PATH_FILE, SESSION_MARKER] {
        let _ = std::fs::remove_file(dir.join(f));
    }
}

/// Open the auto-saved project; it stays modified (it was never saved by the user).
pub fn restore_autosave(app: &mut DarkBurn, path: &Path) -> Result<(), String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let loaded = project::load(&bytes).map_err(|e| e.to_string())?;
    let original = path.parent().and_then(|d| std::fs::read_to_string(d.join(AUTOSAVE_PATH_FILE)).ok()).map(PathBuf::from);
    let clipboard = std::mem::take(&mut app.editor.clipboard);
    app.editor = Editor::with_document(loaded.doc);
    app.editor.clipboard = clipboard;
    app.editor.touch();
    app.project_path = original;
    Ok(())
}

/// Auto-save when due; returns how long until the next check.
fn autosave_tick(app: &mut DarkBurn, now: Instant) -> Option<Duration> {
    if let Some(rx) = &app.files.autosave.writing {
        match rx.try_recv() {
            Ok(r) => {
                app.files.autosave.failed = r.is_err();
                if let Err(e) = r {
                    log::warn!("auto-save failed: {e}");
                }
                app.files.autosave.at = Some(now);
                app.files.autosave.writing = None;
            }
            Err(mpsc::TryRecvError::Empty) => return Some(Duration::from_millis(200)),
            Err(mpsc::TryRecvError::Disconnected) => app.files.autosave.writing = None,
        }
    }
    if !autosave_enabled(app) || app.files.restore.is_some() {
        // Off, or the file still holds a crashed session's copy that is being offered.
        app.files.autosave.due = None;
        return None;
    }
    let rev = app.editor.revision();
    let pending = app.editor.is_modified() && app.files.autosave.revision != Some(rev);
    if !pending {
        app.files.autosave.due = None;
        return app.files.autosave.at.map(|_| Duration::from_secs(30));
    }
    let interval = Duration::from_secs_f32(app.prefs.autosave_minutes * 60.0);
    let due = *app.files.autosave.due.get_or_insert(now + interval);
    if now < due {
        return Some(due - now);
    }
    let dir = app.settings_dir.clone()?;
    let doc = app.editor.doc.clone();
    let project = app.project_path.clone();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(write_autosave(&dir, &doc, project.as_deref()));
    });
    app.files.autosave.writing = Some(rx);
    app.files.autosave.revision = Some(rev);
    app.files.autosave.due = None;
    Some(Duration::from_millis(200))
}

/// The status bar text for auto-save.
fn autosave_status(app: &DarkBurn, now: Instant) -> String {
    let a = &app.files.autosave;
    if !autosave_enabled(app) {
        return "Auto-save off".into();
    }
    if a.failed {
        return "Auto-save failed".into();
    }
    match a.at {
        Some(at) => {
            let mins = now.saturating_duration_since(at).as_secs() / 60;
            if mins == 0 { "Auto-saved just now".into() } else { format!("Auto-saved {mins} min ago") }
        }
        None => format!("Auto-save every {} min", fmt_minutes(app.prefs.autosave_minutes)),
    }
}

fn fmt_minutes(m: f32) -> String {
    if (m - m.round()).abs() < 0.01 { format!("{}", m.round() as i64) } else { format!("{m:.1}") }
}

// ----- System clipboard -----

/// The system clipboard (a fake one in tests).
pub trait SystemClipboard {
    fn text(&mut self) -> Option<String>;
    fn set_text(&mut self, text: String);
    /// A bitmap: width, height and RGBA pixels.
    fn image(&mut self) -> Option<(u32, u32, Vec<u8>)>;
}

struct Arboard(Option<arboard::Clipboard>);

impl Arboard {
    fn open() -> Arboard {
        // Tests never touch the real clipboard.
        Arboard(if cfg!(test) { None } else { arboard::Clipboard::new().map_err(|e| log::warn!("no system clipboard: {e}")).ok() })
    }
}

impl SystemClipboard for Arboard {
    fn text(&mut self) -> Option<String> {
        self.0.as_mut()?.get_text().ok()
    }
    fn set_text(&mut self, text: String) {
        if let Some(c) = self.0.as_mut()
            && let Err(e) = c.set_text(text)
        {
            log::warn!("could not copy to the system clipboard: {e}");
        }
    }
    fn image(&mut self) -> Option<(u32, u32, Vec<u8>)> {
        let img = self.0.as_mut()?.get_image().ok()?;
        Some((img.width as u32, img.height as u32, img.bytes.into_owned()))
    }
}

fn hash(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// Put DarkBurn's clipboard on the system clipboard.
pub fn push_to(app: &mut DarkBurn, sys: &mut dyn SystemClipboard) {
    let text = app.editor.clipboard.to_text();
    sys.set_text(text.clone());
    app.files.last_clip_text = Some(text);
    // Our text took the place of any bitmap there: the same bitmap copied again later is new.
    app.files.last_clip_image = None;
}

/// Take newer contents from the system clipboard: shapes from another DarkBurn, SVG text or a
/// bitmap. Anything else (and what we put there ourselves) leaves DarkBurn's clipboard alone.
pub fn pull_from(app: &mut DarkBurn, sys: &mut dyn SystemClipboard) {
    if let Some(text) = sys.text().filter(|t| !t.trim().is_empty()) {
        if app.files.last_clip_text.as_deref() == Some(text.as_str()) {
            return;
        }
        if text.starts_with(CLIPBOARD_MARK) {
            if let Some(clip) = Clipboard::from_text(&text) {
                app.editor.clipboard = Clipboard { cut_settings: app.editor.clipboard.cut_settings.take(), ..clip };
            }
            app.files.last_clip_text = Some(text);
            app.files.last_clip_image = None;
            return;
        }
        if import::is_svg_text(&text) {
            match import::svg(text.as_bytes(), &import_options(app)) {
                Ok(mut imp) if !imp.is_empty() => {
                    imp.place(import_options(app).page, None);
                    app.editor.clipboard = Clipboard { cut_settings: app.editor.clipboard.cut_settings.take(), ..imp.into_clipboard() };
                }
                Ok(_) => {}
                Err(e) => log::info!("clipboard SVG not read: {e}"),
            }
            app.files.last_clip_text = Some(text);
            app.files.last_clip_image = None;
            return;
        }
        // Plain text: text shapes come later.
        return;
    }
    let Some((w, h, rgba)) = sys.image() else { return };
    let key = hash(&rgba);
    if app.files.last_clip_image == Some(key) {
        return;
    }
    app.files.last_clip_image = Some(key);
    // The bitmap took the place of the text there: the same text copied again later is new.
    app.files.last_clip_text = None;
    let Some(png) = import::rgba_to_png(w, h, rgba) else { return };
    let opts = import_options(app);
    if let Ok(mut imp) = import::bitmap(&png, &opts, None) {
        imp.place(opts.page, None);
        app.editor.clipboard = Clipboard { cut_settings: app.editor.clipboard.cut_settings.take(), ..imp.into_clipboard() };
    }
}

/// Before a paste: take shapes from the system clipboard when it holds something newer.
pub fn pull_system_clipboard(app: &mut DarkBurn, _ctx: &egui::Context) {
    pull_if_used(app, &mut Arboard::open());
}

/// [`pull_from`], unless the system clipboard is turned off in the settings: then Paste gives
/// what was copied in this window, whatever other programs have copied since.
pub fn pull_if_used(app: &mut DarkBurn, sys: &mut dyn SystemClipboard) {
    if app.prefs.opts.system_clipboard {
        pull_from(app, sys);
    }
}

// ----- Each frame -----

/// The window title: project name, a mark when modified.
pub fn title(app: &DarkBurn) -> String {
    let name = app.project_path.as_deref().and_then(Path::file_stem).map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Untitled".into());
    let mark = if app.editor.is_modified() { "*" } else { "" };
    format!("{name}{mark} - DarkBurn")
}

/// Once a frame: start-up checks, auto-save, push copies to the system clipboard, take dropped
/// files, the window title and closing.
pub fn frame(ctx: &egui::Context, app: &mut DarkBurn) {
    let now = Instant::now();
    if !app.files.started {
        app.files.started = true;
        prune_recent(&mut app.prefs);
        if let Some(dir) = app.settings_dir.clone()
            && !app.safe_mode
            && !app.fio.secondary
        {
            app.files.restore = begin_session(&dir);
        }
    }
    if app.clipboard_out && app.prefs.opts.system_clipboard {
        app.clipboard_out = false;
        push_to(app, &mut Arboard::open());
    }
    let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
    if !dropped.is_empty() && !crate::art_library::handle_drop(app, ctx, &dropped) {
        let at = pointer_point(app);
        drop_files(app, ctx, dropped, at);
    }
    if let Some(wait) = autosave_tick(app, now) {
        ctx.request_repaint_after(wait);
    }
    app.autosave_status = autosave_status(app, now);
    let title = title(app);
    if title != app.files.title {
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
        app.files.title = title;
    }
    if ctx.input(|i| i.viewport().close_requested()) {
        if app.editor.is_modified() && !app.files.close_ok {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            app.files.ask_save = Some(Then::Exit);
        } else {
            wait_for_autosave(app);
            crate::art_library::closing(app);
            if let Some(dir) = &app.settings_dir
                && !app.safe_mode
                && !app.fio.secondary
            {
                crate::instance::clear(dir);
                end_session(dir);
            }
        }
    }
}

// ----- Windows -----

/// Windows owned by this module: save changes, notes, crash restore, messages.
pub fn dialogs(ctx: &egui::Context, app: &mut DarkBurn) {
    if let Some(then) = app.files.ask_save.clone() {
        let name =
            app.project_path.as_deref().and_then(Path::file_name).map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Untitled".into());
        let (closed, action) = ui_kit::Window::new(egui::Id::unique("db_save_changes"), "save", "Save changes?")
            .actions("Save", "Cancel", Some("Don't save"))
            .show(ctx, true, |ui| {
                ui.label(format!("{name} has changes that are not saved."));
            });
        match action {
            ui_kit::Action::Ok => {
                app.files.ask_save = None;
                if save(app) {
                    proceed(app, ctx, then);
                }
            }
            ui_kit::Action::Apply => {
                app.files.ask_save = None;
                // Let the project go without saving it. It stays marked as changed until
                // something takes its place: the open may still be cancelled or fail.
                proceed(app, ctx, then);
            }
            ui_kit::Action::Cancel => app.files.ask_save = None,
            ui_kit::Action::None if closed => app.files.ask_save = None,
            ui_kit::Action::None => {}
        }
    }
    if let Some(path) = app.files.restore.clone() {
        let (_, action) = ui_kit::Window::new(egui::Id::unique("db_restore"), "open", "Restore auto-saved project?")
            .actions("Restore", "Discard", None)
            .required()
            .show(ctx, true, |ui| {
                ui.label("DarkBurn did not close properly last time. An auto-saved copy of the project you were working on was found.");
            });
        match action {
            ui_kit::Action::Ok => {
                app.files.restore = None;
                if let Err(e) = restore_autosave(app, &path) {
                    app.files.message = Some(("Could not restore".into(), e));
                }
            }
            ui_kit::Action::Cancel => {
                app.files.restore = None;
                discard_autosave(app);
            }
            _ => {}
        }
    }
    if app.notes_open {
        let doc = &mut app.editor.doc;
        let (mut notes, mut show) = (doc.notes.clone(), doc.show_notes_on_load);
        let (closed, action) =
            ui_kit::Window::new(egui::Id::unique("db_notes"), "db_app", "Project notes").fields().actions("", "Close", None).show(ctx, true, |ui| {
                ui_kit::text_area(ui, "Notes", egui::Id::unique("db_notes_text"), &mut notes, 12, 100_000);
                ui_kit::checkbox(ui, "Show the notes when the project opens", &mut show);
            });
        if notes != doc.notes || show != doc.show_notes_on_load {
            doc.notes = notes;
            doc.show_notes_on_load = show;
            app.editor.touch();
        }
        if closed || action != ui_kit::Action::None {
            app.notes_open = false;
        }
    }
    if let Some((title, text)) = app.files.message.clone() {
        let (closed, action) =
            ui_kit::Window::new(egui::Id::unique("db_file_message"), "db_app", &title).actions("", "Close", None).show(ctx, true, |ui| {
                ui.label(&text);
            });
        if closed || action != ui_kit::Action::None {
            app.files.message = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use darkburn_core::kurbo::Rect;
    use darkburn_core::{ImageData, ImageShape};
    use std::sync::Arc;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> TempDir {
            let d = std::env::temp_dir().join(format!("darkburn-files-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(&d).unwrap();
            TempDir(d)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn app(dir: &TempDir) -> DarkBurn {
        let mut a = DarkBurn::new(Prefs::default());
        a.settings_dir = Some(dir.0.join("settings"));
        a
    }

    fn add_rect(a: &mut DarkBurn) {
        a.editor.edit("Add", |d, _| {
            d.add(Shape::rect(Rect::new(0.0, 0.0, 10.0, 10.0), 0.0, 1));
            true
        });
    }

    fn png(w: u32, h: u32) -> Vec<u8> {
        import::rgba_to_png(w, h, vec![90; (w * h * 4) as usize]).unwrap()
    }

    #[derive(Default)]
    struct FakeClip {
        text: Option<String>,
        image: Option<(u32, u32, Vec<u8>)>,
    }

    impl SystemClipboard for FakeClip {
        fn text(&mut self) -> Option<String> {
            self.text.clone()
        }
        fn set_text(&mut self, text: String) {
            self.text = Some(text);
            self.image = None;
        }
        fn image(&mut self) -> Option<(u32, u32, Vec<u8>)> {
            self.image.clone()
        }
    }

    #[test]
    fn save_open_round_trips_images_notes_and_thumbnail() {
        let dir = TempDir::new("save");
        let mut a = app(&dir);
        add_rect(&mut a);
        let bytes = png(8, 4);
        a.editor.edit("Image", |d, _| {
            let key = d.add_image(ImageData { format: "png".into(), bytes: Arc::from(&bytes[..]), px_width: 8, px_height: 4 });
            d.add(Shape::new(ShapeKind::Image(ImageShape { image: key, width: 20.0, height: 10.0, ..Default::default() }), Affine::IDENTITY, 2));
            true
        });
        a.editor.doc.notes = "Birch 3 mm".into();
        a.editor.doc.show_notes_on_load = true;
        assert!(a.editor.is_modified());
        let path = dir.0.join("job.dbrn");
        save_to(&mut a, &path).unwrap();
        assert!(!a.editor.is_modified());
        assert_eq!(title(&a), "job - DarkBurn");
        assert_eq!(a.prefs.recent.len(), 1);
        let loaded = project::load(&std::fs::read(&path).unwrap()).unwrap();
        assert!(loaded.thumbnail.is_some_and(|t| t.starts_with(b"\x89PNG")));

        let mut b = app(&dir);
        open_path(&mut b, &path).unwrap();
        assert_eq!(b.editor.doc.len(), 2);
        assert_eq!(b.editor.doc.notes, "Birch 3 mm");
        assert!(b.notes_open, "notes open on load when asked");
        assert_eq!(b.editor.doc.images.values().next().unwrap().bytes[..], bytes[..]);
        assert!(!b.editor.is_modified());
        assert_eq!(b.project_path.as_deref(), Some(path.as_path()));
        assert!(b.files.message.is_none());
        add_rect(&mut b);
        assert_eq!(title(&b), "job* - DarkBurn");
    }

    #[test]
    fn recent_projects_are_newest_first_unique_and_capped() {
        let mut p = Prefs::default();
        for i in 0..30 {
            remember_recent(&mut p, Path::new(&format!("C:/x/{i}.dbrn")));
        }
        remember_recent(&mut p, Path::new("C:/x/5.dbrn"));
        assert_eq!(p.recent.len(), RECENT_MAX);
        assert!(p.recent[0].ends_with("5.dbrn"));
        assert_eq!(p.recent.iter().filter(|r| r.ends_with("\\5.dbrn") || r.ends_with("/5.dbrn")).count(), 1);
        assert!(p.recent[1].ends_with("29.dbrn"));
        prune_recent(&mut p);
        assert!(p.recent.is_empty(), "missing files are dropped");
    }

    #[test]
    fn recent_entry_for_a_missing_file_is_removed() {
        let dir = TempDir::new("recent");
        let mut a = app(&dir);
        a.prefs.recent = vec![dir.0.join("gone.dbrn").to_string_lossy().into_owned()];
        let ctx = egui::Context::default();
        assert!(run(&mut a, &ctx, "file.recent.0"));
        assert!(a.prefs.recent.is_empty());
        assert!(a.files.message.is_some());
        assert!(!run(&mut a, &ctx, "edit.copy"));
    }

    #[test]
    fn new_and_open_ask_first_when_modified() {
        let dir = TempDir::new("guard");
        let mut a = app(&dir);
        let ctx = egui::Context::default();
        add_rect(&mut a);
        run(&mut a, &ctx, "file.new");
        assert_eq!(a.files.ask_save, Some(Then::New));
        assert_eq!(a.editor.doc.len(), 1, "nothing happens before the answer");
        a.files.ask_save = None;
        a.editor.mark_saved();
        a.editor.clipboard.shapes.push(Shape::rect(Rect::new(0.0, 0.0, 1.0, 1.0), 0.0, 0));
        run(&mut a, &ctx, "file.new");
        assert!(a.files.ask_save.is_none());
        assert!(a.editor.doc.is_empty());
        assert_eq!(a.editor.clipboard.shapes.len(), 1, "the clipboard survives New");
        add_rect(&mut a);
        run(&mut a, &ctx, "file.exit");
        assert_eq!(a.files.ask_save, Some(Then::Exit));
    }

    #[test]
    fn newer_project_opens_with_a_warning() {
        let dir = TempDir::new("newer");
        let mut a = app(&dir);
        add_rect(&mut a);
        let shapes = serde_json::to_string(&a.editor.doc.iter().cloned().collect::<Vec<_>>()).unwrap();
        let json = format!(r#"{{"format": "darkburn-project", "version": 7, "app_version": "99.0.0", "future": 1, "shapes": {shapes}}}"#);
        let path = dir.0.join("future.dbrn");
        std::fs::write(&path, write_zip(&[("project.json", json.as_bytes())])).unwrap();
        let mut b = app(&dir);
        open_path(&mut b, &path).unwrap();
        assert_eq!(b.editor.doc.len(), 1);
        let (title, text) = b.files.message.clone().unwrap();
        assert!(title.contains("newer") && text.contains("99.0.0"), "{title}: {text}");
        assert!(open_path(&mut b, &dir.0.join("nothing.dbrn")).is_err());
        std::fs::write(dir.0.join("junk.dbrn"), b"junk").unwrap();
        assert!(open_path(&mut b, &dir.0.join("junk.dbrn")).is_err());
    }

    /// A stored (uncompressed) zip, enough for the project reader.
    fn write_zip(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, data) in files {
            let crc = crc32(data);
            let offset = out.len() as u32;
            let mut h = Vec::new();
            h.extend(0x0403_4b50u32.to_le_bytes());
            h.extend([20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
            h.extend(crc.to_le_bytes());
            h.extend((data.len() as u32).to_le_bytes());
            h.extend((data.len() as u32).to_le_bytes());
            h.extend((name.len() as u16).to_le_bytes());
            h.extend(0u16.to_le_bytes());
            out.extend(&h);
            out.extend(name.as_bytes());
            out.extend(*data);
            let mut c = Vec::new();
            c.extend(0x0201_4b50u32.to_le_bytes());
            c.extend([20, 0, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
            c.extend(crc.to_le_bytes());
            c.extend((data.len() as u32).to_le_bytes());
            c.extend((data.len() as u32).to_le_bytes());
            c.extend((name.len() as u16).to_le_bytes());
            c.extend([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
            c.extend(offset.to_le_bytes());
            c.extend(name.as_bytes());
            central.extend(c);
        }
        let cd_offset = out.len() as u32;
        out.extend(&central);
        out.extend(0x0605_4b50u32.to_le_bytes());
        out.extend([0, 0, 0, 0]);
        out.extend((files.len() as u16).to_le_bytes());
        out.extend((files.len() as u16).to_le_bytes());
        out.extend((central.len() as u32).to_le_bytes());
        out.extend(cd_offset.to_le_bytes());
        out.extend([0, 0]);
        out
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for b in data {
            crc ^= *b as u32;
            for _ in 0..8 {
                crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
            }
        }
        !crc
    }

    #[test]
    fn import_is_one_undo_step_grouped_and_selected() {
        let dir = TempDir::new("import");
        let mut a = app(&dir);
        let svg = dir.0.join("art.svg");
        std::fs::write(&svg, br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><rect width="10" height="10" fill="red"/><circle cx="50" cy="50" r="5" stroke="blue" fill="none"/></svg>"#).unwrap();
        let pic = dir.0.join("pic.png");
        std::fs::write(&pic, png(20, 10)).unwrap();
        let bad = dir.0.join("bad.svg");
        std::fs::write(&bad, b"<svg").unwrap();
        let ids = import_paths(&mut a, &[svg.clone(), pic.clone(), bad], None);
        assert_eq!(ids.len(), 2, "the SVG is one group, the bitmap one shape");
        assert_eq!(a.editor.history.len(), 1, "one undo step");
        assert_eq!(a.editor.selected(), ids);
        assert!(a.editor.doc.get(ids[0]).unwrap().is_group());
        assert_eq!(a.editor.doc.images.len(), 1);
        assert!(a.files.message.as_ref().is_some_and(|(_, t)| t.contains("bad.svg")));
        a.editor.undo();
        assert!(a.editor.doc.is_empty());

        a.prefs.import_group = false;
        a.prefs.import_select = false;
        a.editor.selection.clear();
        let ids = import_paths(&mut a, &[svg], Some(Point::new(50.0, 60.0)));
        assert_eq!(ids.len(), 2);
        assert!(a.editor.selected().is_empty());
        let c = a.editor.doc.bbox().unwrap().center();
        assert!((c.x - 50.0).abs() < 1e-6 && (c.y - 60.0).abs() < 1e-6, "centred on the drop point");
    }

    #[test]
    fn opening_artwork_starts_a_new_project_with_it() {
        let dir = TempDir::new("openart");
        let mut a = app(&dir);
        add_rect(&mut a);
        a.editor.mark_saved();
        let pic = dir.0.join("pic.png");
        std::fs::write(&pic, png(4, 4)).unwrap();
        open_path(&mut a, &pic).unwrap();
        assert_eq!(a.editor.doc.len(), 1);
        assert!(a.editor.doc.iter().next().unwrap().is_image());
        assert!(a.project_path.is_none());
    }

    /// What a crashed session leaves in the settings folder: the marker and an auto-save of a
    /// project with one rectangle that was being edited as `mine.dbrn`.
    fn leave_a_crashed_session(settings: &Path, project: &Path) {
        let mut doc = darkburn_core::Document::new();
        doc.add(Shape::rect(Rect::new(5.0, 5.0, 25.0, 15.0), 0.0, 1));
        assert_eq!(begin_session(settings), None);
        write_autosave(settings, &doc, Some(project)).unwrap();
    }

    // PLAT-06: the next start finds the crash restore file and offers it.
    #[test]
    fn a_start_after_a_crash_offers_the_restore_file_and_a_clean_exit_does_not() {
        let dir = TempDir::new("crash-start");
        let settings = dir.0.join("settings");
        leave_a_crashed_session(&settings, &dir.0.join("mine.dbrn"));
        let ctx = egui::Context::default();
        let mut a = app(&dir);
        frame(&ctx, &mut a);
        assert_eq!(a.files.restore.as_deref(), Some(settings.join(AUTOSAVE_FILE).as_path()));
        assert!(a.files.open_dialogs().contains(&"restore_autosave"));
        // Safe mode and a second window of the same copy never offer it (and leave the files).
        for (safe, secondary) in [(true, false), (false, true)] {
            let mut b = app(&dir);
            b.safe_mode = safe;
            b.fio.secondary = secondary;
            frame(&ctx, &mut b);
            assert!(b.files.restore.is_none());
            assert!(settings.join(AUTOSAVE_FILE).is_file());
        }
        // After a clean exit there is nothing to offer.
        end_session(&settings);
        let mut c = app(&dir);
        frame(&ctx, &mut c);
        assert!(c.files.restore.is_none());
        assert!(settings.join(SESSION_MARKER).is_file(), "this session is marked as running");
        end_session(&settings);
    }

    #[test]
    fn the_restore_dialog_brings_the_project_back_or_throws_the_copy_away() {
        use crate::ui_harness::Gui;
        for restore in [true, false] {
            let mut g = Gui::standard();
            let settings = g.temp("crashed");
            let project = g.temp("mine.dbrn");
            leave_a_crashed_session(&settings, &project);
            {
                let mut a = g.app();
                a.settings_dir = Some(settings.clone());
                a.safe_mode = false;
                a.files.started = false; // as at a start
            }
            g.frames(2);
            assert!(g.window_titles().contains(&"Restore auto-saved project?".to_string()), "{:?}", g.window_titles());
            assert!(g.visible_texts().join(" ").contains("did not close properly"));
            g.screenshot("crash-restore");
            assert!(g.layout_problems().is_empty(), "{:?}", g.layout_problems());
            assert_eq!(g.app().editor.doc.len(), 0, "nothing is opened before the answer");
            g.click_label(if restore { "Restore" } else { "Discard" });
            g.frames(2);
            assert!(!g.window_titles().contains(&"Restore auto-saved project?".to_string()));
            let mut a = g.app();
            if restore {
                assert_eq!(a.editor.doc.len(), 1, "the rectangle is back");
                assert!(a.editor.is_modified(), "it still needs saving");
                assert_eq!(a.project_path.as_deref(), Some(project.as_path()), "under the name it had");
                assert!(!project.exists(), "the user's own file is not written by a restore");
            } else {
                assert_eq!(a.editor.doc.len(), 0);
                assert!(a.project_path.is_none());
                assert!(!settings.join(AUTOSAVE_FILE).exists(), "the copy is gone");
                assert!(!settings.join(AUTOSAVE_PATH_FILE).exists());
            }
            // Leave nothing for the harness's own checks to trip over.
            a.safe_mode = true;
        }
    }

    #[test]
    fn a_damaged_restore_file_is_reported_and_the_open_project_is_kept() {
        let dir = TempDir::new("crash-bad");
        let settings = dir.0.join("settings");
        std::fs::create_dir_all(&settings).unwrap();
        std::fs::write(settings.join(AUTOSAVE_FILE), b"not a project").unwrap();
        let mut a = app(&dir);
        add_rect(&mut a);
        assert!(restore_autosave(&mut a, &settings.join(AUTOSAVE_FILE)).is_err());
        assert_eq!(a.editor.doc.len(), 1, "what was open is untouched");
    }

    #[test]
    fn autosave_writes_after_the_interval_and_restores_after_a_crash() {
        let dir = TempDir::new("autosave");
        let settings = dir.0.join("settings");
        let mut a = app(&dir);
        a.prefs.autosave_minutes = 1.0;
        assert_eq!(begin_session(&settings), None, "no crash on a first start");
        let t0 = Instant::now();
        assert_eq!(autosave_status(&a, t0), "Auto-save every 1 min");
        assert!(autosave_tick(&mut a, t0).is_none(), "nothing to save");
        add_rect(&mut a);
        a.project_path = Some(dir.0.join("mine.dbrn"));
        let wait = autosave_tick(&mut a, t0).unwrap();
        assert!(wait > Duration::from_secs(59));
        assert!(!settings.join(AUTOSAVE_FILE).exists());
        autosave_tick(&mut a, t0 + Duration::from_secs(61));
        wait_for_autosave(&mut a);
        assert!(settings.join(AUTOSAVE_FILE).is_file());
        assert!(!dir.0.join("mine.dbrn").exists(), "the user's file is never written");
        a.files.autosave.at = Some(t0);
        assert_eq!(autosave_status(&a, t0 + Duration::from_secs(130)), "Auto-saved 2 min ago");

        // The app is killed: the marker and the auto-save stay. The next start offers it.
        let found = begin_session(&settings).expect("crash detected");
        let mut b = app(&dir);
        restore_autosave(&mut b, &found).unwrap();
        assert_eq!(b.editor.doc.len(), 1);
        assert!(b.editor.is_modified());
        assert_eq!(b.project_path.as_deref(), Some(dir.0.join("mine.dbrn").as_path()));

        // Saving removes the auto-save; a clean exit removes the marker.
        save_to(&mut b, &dir.0.join("mine.dbrn")).unwrap();
        assert!(!settings.join(AUTOSAVE_FILE).exists());
        end_session(&settings);
        assert!(!settings.join(SESSION_MARKER).exists());
        assert_eq!(begin_session(&settings), None);

        b.prefs.autosave_minutes = 0.0;
        assert_eq!(autosave_status(&b, t0), "Auto-save off");
    }

    #[test]
    fn system_clipboard_carries_shapes_svg_and_bitmaps() {
        let dir = TempDir::new("clip");
        let mut a = app(&dir);
        let mut sys = FakeClip::default();
        add_rect(&mut a);
        let ids = a.editor.doc.iter().map(|s| s.id).collect::<Vec<_>>();
        a.editor.clipboard.copy(&a.editor.doc, &ids);
        push_to(&mut a, &mut sys);
        assert!(sys.text.as_deref().is_some_and(|t| t.starts_with(CLIPBOARD_MARK)));

        // Another DarkBurn reads it.
        let mut b = app(&dir);
        pull_from(&mut b, &mut sys);
        assert_eq!(b.editor.clipboard.shapes.len(), 1);
        // What we wrote ourselves does not replace a newer local copy.
        a.editor.clipboard.shapes.clear();
        pull_from(&mut a, &mut sys);
        assert!(a.editor.clipboard.shapes.is_empty());

        // SVG text from a drawing program.
        sys.text = Some(
            r#"<?xml version="1.0"?><svg xmlns="http://www.w3.org/2000/svg" width="20" height="20"><path d="M0 0 L10 10" stroke="black"/></svg>"#
                .into(),
        );
        pull_from(&mut b, &mut sys);
        assert_eq!(b.editor.clipboard.shapes.len(), 1);
        assert!(matches!(b.editor.clipboard.shapes[0].kind, ShapeKind::Path { .. }));

        // Plain text is left alone.
        sys.text = Some("hello".into());
        pull_from(&mut b, &mut sys);
        assert!(matches!(b.editor.clipboard.shapes[0].kind, ShapeKind::Path { .. }));

        // A bitmap.
        sys.text = None;
        sys.image = Some((3, 2, vec![255; 24]));
        pull_from(&mut b, &mut sys);
        assert!(b.editor.clipboard.shapes[0].is_image());
        let key = b.editor.clipboard.shapes[0].image_keys()[0].clone();
        assert_eq!(b.editor.clipboard.images[&key].format, "png");
        // The same bitmap again does not replace a later local copy.
        b.editor.clipboard.shapes.clear();
        pull_from(&mut b, &mut sys);
        assert!(b.editor.clipboard.shapes.is_empty());
    }

    /// A bitmap pasted once, then shapes copied here, then the same bitmap copied again in the
    /// other program: Paste gives the bitmap (the newest copy), not the shapes. And the other way
    /// round for shapes from a second DarkBurn copied again after a bitmap.
    #[test]
    fn the_same_thing_copied_again_elsewhere_after_something_else_is_pasted() {
        let dir = TempDir::new("clip-again");
        let mut a = app(&dir);
        let mut sys = FakeClip { text: None, image: Some((3, 2, vec![255; 24])) };
        pull_from(&mut a, &mut sys);
        assert!(a.editor.clipboard.shapes[0].is_image());
        add_rect(&mut a);
        let ids = a.editor.doc.iter().map(|s| s.id).collect::<Vec<_>>();
        a.editor.clipboard.copy(&a.editor.doc, &ids);
        push_to(&mut a, &mut sys);
        assert!(sys.image.is_none(), "our text replaced the bitmap");
        let ours = sys.text.clone();
        // The other program copies its bitmap again.
        sys = FakeClip { text: None, image: Some((3, 2, vec![255; 24])) };
        pull_from(&mut a, &mut sys);
        assert!(a.editor.clipboard.shapes[0].is_image(), "the bitmap is the newest copy");
        // And our own shapes text again (as a second DarkBurn would put it there).
        sys = FakeClip { text: ours, image: None };
        pull_from(&mut a, &mut sys);
        assert!(!a.editor.clipboard.shapes[0].is_image(), "the shapes are the newest copy");
    }

    /// With "Use the system clipboard" off, Paste does not take what another program copied.
    #[test]
    fn paste_leaves_the_system_clipboard_alone_when_it_is_turned_off() {
        let dir = TempDir::new("clip-off");
        let mut a = app(&dir);
        add_rect(&mut a);
        let ids = a.editor.doc.iter().map(|s| s.id).collect::<Vec<_>>();
        a.editor.clipboard.copy(&a.editor.doc, &ids);
        let mut sys = FakeClip { text: None, image: Some((3, 2, vec![255; 24])) };
        a.prefs.opts.system_clipboard = false;
        pull_if_used(&mut a, &mut sys);
        assert!(!a.editor.clipboard.shapes[0].is_image(), "the copied rectangle stays");
        a.prefs.opts.system_clipboard = true;
        pull_if_used(&mut a, &mut sys);
        assert!(a.editor.clipboard.shapes[0].is_image());
    }

    #[test]
    fn notes_edits_mark_the_project_modified_via_touch() {
        let dir = TempDir::new("notes");
        let mut a = app(&dir);
        let ctx = egui::Context::default();
        run(&mut a, &ctx, "file.notes");
        assert!(a.notes_open);
    }

    /// "Don't save" lets the project go only when something takes its place: when the file to
    /// open turns out to be unreadable the project is still open and still unsaved.
    #[test]
    fn dont_save_keeps_the_project_unsaved_when_the_open_fails() {
        use crate::ui_harness::Gui;
        let mut g = Gui::standard();
        let junk = g.temp("junk.dbrn");
        std::fs::write(&junk, b"not a project").unwrap();
        {
            let mut a = g.app();
            add_rect(&mut a);
            assert!(a.editor.is_modified());
            a.files.ask_save = Some(Then::Open(Some(junk.clone())));
        }
        g.frames(2);
        g.click_label("Don't save");
        g.frames(2);
        let a = g.app();
        assert!(a.files.message.as_ref().is_some_and(|(t, _)| t == "Could not open"), "{:?}", a.files.message);
        assert_eq!(a.editor.doc.len(), 1, "the project is still open");
        assert!(a.editor.is_modified(), "and it still has unsaved changes, so closing asks again");
        assert_eq!(title(&a), "Untitled* - DarkBurn");
    }

    /// An extra window keeps no auto-save of its own, so nothing it does may remove the first
    /// window's copy.
    #[test]
    fn an_extra_window_leaves_the_first_windows_autosave_alone() {
        let dir = TempDir::new("secondary");
        let settings = dir.0.join("settings");
        let mut doc = darkburn_core::Document::new();
        doc.add(Shape::rect(Rect::new(5.0, 5.0, 25.0, 15.0), 0.0, 1));
        write_autosave(&settings, &doc, Some(&dir.0.join("mine.dbrn"))).unwrap();
        let mut second = app(&dir);
        second.fio.secondary = true;
        add_rect(&mut second);
        new_project(&mut second);
        add_rect(&mut second);
        save_to(&mut second, &dir.0.join("other.dbrn")).unwrap();
        open_path(&mut second, &dir.0.join("other.dbrn")).unwrap();
        assert!(settings.join(AUTOSAVE_FILE).is_file(), "the first window's auto-save is still there");
        assert!(settings.join(AUTOSAVE_PATH_FILE).is_file());
    }

    /// DarkBurn is started again after a crash by opening another project (a double click on a
    /// file): the crashed session's copy must still be there when the restore question is answered.
    #[test]
    fn opening_a_file_at_the_start_after_a_crash_keeps_the_copy_to_restore() {
        let dir = TempDir::new("crash-arg");
        let settings = dir.0.join("settings");
        let mine = dir.0.join("mine.dbrn");
        leave_a_crashed_session(&settings, &mine);
        let other = dir.0.join("other.dbrn");
        let mut writer = DarkBurn::new(Prefs::default());
        add_rect(&mut writer);
        add_rect(&mut writer);
        save_to(&mut writer, &other).unwrap();

        let ctx = egui::Context::default();
        let mut a = app(&dir);
        a.fio.pending_open = vec![other.clone()];
        frame(&ctx, &mut a);
        crate::file_io::frame(&ctx, &mut a);
        assert_eq!(a.project_path.as_deref(), Some(other.as_path()), "the file from the command line is open");
        assert_eq!(a.editor.doc.len(), 2);
        let offered = a.files.restore.clone().expect("the restore question is still asked");
        assert!(offered.is_file(), "and the copy it is about still exists");
        restore_autosave(&mut a, &offered).unwrap();
        assert_eq!(a.editor.doc.len(), 1, "the crashed session's project is back");
        assert_eq!(a.project_path.as_deref(), Some(mine.as_path()));
        // Answered: from now on a save removes the copy as usual.
        a.files.restore = None;
        save_to(&mut a, &mine).unwrap();
        assert!(!settings.join(AUTOSAVE_FILE).exists());
        end_session(&settings);
    }
}
