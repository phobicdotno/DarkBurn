//! Opening, importing and exporting files: the commands, the dialogs, the worker thread and the
//! JSON commands agents use. The conversions themselves are in `formats.rs`.
//!
//! Importing runs on a worker thread: the window stays responsive, a progress window shows the
//! file being read and offers Cancel (a file already being read is let go when it finishes; its
//! result is dropped). The same reading code runs synchronously for `project.import`,
//! `project.open` and the tests.
//!
//! Anything that needs a native file picker is only started from the frame loop (never from a
//! command run without a window), so `api::execute_headless` can never block on a dialog.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use darkburn_core::kurbo::{Point, Size};
use darkburn_core::layer::Layer;
use darkburn_core::{Editor, ShapeId};
use darkburn_ui::ui_kit;
use serde_json::{Value, json};

use crate::formats::{self, ExportFormat, ExportRequest, Loaded, PageChoice, ReadOptions, ReadOutcome};
use crate::import::{self, FileKind};
use crate::prefs_files;
use crate::shell::DarkBurn;

type Results = Vec<(PathBuf, Result<ReadOutcome, String>)>;

/// State of this module, kept in the app.
#[derive(Default)]
pub struct FileIo {
    job: Option<Job>,
    page_pick: Option<PagePick>,
    /// The warnings of the last import, shown in a window.
    pub warnings: Option<Vec<(String, Vec<String>)>>,
    options_open: bool,
    export: Option<ExportDialog>,
    print: Option<PrintDialog>,
    backups: Option<Vec<prefs_files::Backup>>,
    /// Something that needs a native file picker, started by the frame loop.
    pick: Option<Pick>,
    /// Device profiles that were opened or dropped, for the devices code to take.
    pub device_profiles: Vec<PathBuf>,
    /// Files to open once the window is up (the command line).
    pub pending_open: Vec<PathBuf>,
    /// This copy was started as an extra window: it leaves the session files alone.
    pub secondary: bool,
    next_tick: Option<Instant>,
    started: bool,
    /// The project's revision when File > Open began reading a file in the background.
    open_revision: Option<u64>,
    /// A file that was read while the project was changed: it waits for "Save changes?".
    held: Option<Box<Loaded>>,
}

#[derive(Clone, Debug, PartialEq)]
enum Pick {
    Import,
    PrefsExport,
    PrefsImport,
    SaveExport(Box<ExportDialogResult>),
    SavePrintPdf { black_only: bool, selection_only: bool },
    SaveLightBurn,
}

struct Job {
    rx: mpsc::Receiver<Results>,
    cancel: Arc<AtomicBool>,
    done: Arc<AtomicUsize>,
    total: usize,
    current: Arc<Mutex<String>>,
    at: Option<Point>,
    open: bool,
}

struct PagePick {
    path: PathBuf,
    count: usize,
    page: u32,
    all: bool,
    at: Option<Point>,
    open: bool,
}

/// What the report of an import holds.
#[derive(Debug, Default)]
pub struct Report {
    pub ids: Vec<ShapeId>,
    pub errors: Vec<String>,
    pub warnings: Vec<(String, Vec<String>)>,
    /// PDFs that need a page chosen.
    pub needs_page: Vec<(PathBuf, usize)>,
}

fn file_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string())
}

/// The reading options for the open project's preferences.
pub fn read_options(app: &DarkBurn) -> ReadOptions {
    ReadOptions::new(crate::files::import_options(app), app.prefs.files.clone())
}

// ----- Importing -----

/// Reads and inserts `paths` now (no window needed). PDFs use page 1 unless `page` says.
pub fn import_sync(app: &mut DarkBurn, paths: &[PathBuf], at: Option<Point>, page: Option<PageChoice>) -> Report {
    let mut opts = read_options(app);
    opts.pdf_page = Some(page.unwrap_or(PageChoice::Page(1)));
    let results: Results = paths.iter().map(|p| (p.clone(), formats::read_file(p, &opts))).collect();
    apply_results(app, results, at)
}

/// Puts what was read into the document as one undo step.
pub fn apply_results(app: &mut DarkBurn, results: Results, at: Option<Point>) -> Report {
    let page: Size = crate::files::import_options(app).page;
    let mut report = Report::default();
    let mut imports = Vec::new();
    let mut layers: Vec<Layer> = Vec::new();
    let used = app.editor.doc.used_layers();
    for (path, result) in results {
        let name = file_name(&path);
        match result {
            Ok(ReadOutcome::Loaded(loaded)) => {
                let mut loaded = *loaded;
                if !loaded.warnings.is_empty() {
                    report.warnings.push((name, loaded.warnings.clone()));
                }
                loaded.imported.place(page, at);
                for l in loaded.layers {
                    if !used.contains(&l.index) && !layers.iter().any(|x| x.index == l.index) {
                        layers.push(l);
                    }
                }
                imports.push(loaded.imported);
            }
            Ok(ReadOutcome::NeedsPage(n)) => report.needs_page.push((path, n)),
            Err(e) => report.errors.push(format!("{name}: {e}")),
        }
    }
    report.ids = crate::files::insert_with_layers(app, imports, layers);
    report
}

/// Shows what an import reported: errors in the message window, warnings in their own.
fn show_report(app: &mut DarkBurn, report: &Report) {
    if !report.errors.is_empty() {
        crate::files::show_message(app, "Some files were not imported", report.errors.join("\n"));
    }
    if !report.warnings.is_empty() {
        app.fio.warnings = Some(report.warnings.clone());
    }
}

/// Imports from the window: on a worker thread with progress and Cancel.
pub fn start_import(app: &mut DarkBurn, paths: Vec<PathBuf>, at: Option<Point>) {
    start_job(app, paths, at, false, None);
}

/// Opens a file that is not a DarkBurn project: a LightBurn project becomes the open project,
/// artwork starts a new project holding it. On a worker thread.
pub fn start_open(app: &mut DarkBurn, path: PathBuf) {
    if formats::is_device_profile(&path) {
        device_profile_hook(app, &path);
        return;
    }
    app.fio.open_revision = Some(app.editor.revision());
    start_job(app, vec![path], None, true, None);
}

fn start_job(app: &mut DarkBurn, paths: Vec<PathBuf>, at: Option<Point>, open: bool, page: Option<PageChoice>) {
    let (devices, paths): (Vec<PathBuf>, Vec<PathBuf>) = paths.into_iter().partition(|p| formats::is_device_profile(p));
    for d in devices {
        device_profile_hook(app, &d);
    }
    if paths.is_empty() {
        return;
    }
    if app.fio.job.is_some() {
        crate::files::show_message(app, "Busy", "DarkBurn is still reading the last files. Wait for it or cancel it first.".into());
        return;
    }
    let mut opts = read_options(app);
    opts.pdf_page = page;
    let (tx, rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let done = Arc::new(AtomicUsize::new(0));
    let current = Arc::new(Mutex::new(String::new()));
    let job = Job { rx, cancel: cancel.clone(), done: done.clone(), total: paths.len(), current: current.clone(), at, open };
    std::thread::spawn(move || {
        let mut results: Results = Vec::new();
        for p in paths {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            if let Ok(mut c) = current.lock() {
                *c = file_name(&p);
            }
            let r = formats::read_file(&p, &opts);
            results.push((p, r));
            done.fetch_add(1, Ordering::Relaxed);
        }
        let _ = tx.send(results);
    });
    app.fio.job = Some(job);
}

/// Takes a finished worker's results.
fn poll_job(app: &mut DarkBurn) {
    let Some(job) = &app.fio.job else { return };
    let results = match job.rx.try_recv() {
        Ok(r) => r,
        Err(mpsc::TryRecvError::Empty) => return,
        Err(mpsc::TryRecvError::Disconnected) => Vec::new(),
    };
    let Some(job) = app.fio.job.take() else { return };
    if job.cancel.load(Ordering::Relaxed) {
        return;
    }
    if job.open {
        finish_open(app, results);
    } else {
        let report = apply_results(app, results, job.at);
        show_report(app, &report);
        queue_pages(app, report.needs_page, job.at, false);
    }
}

fn queue_pages(app: &mut DarkBurn, needs: Vec<(PathBuf, usize)>, at: Option<Point>, open: bool) {
    if let Some((path, count)) = needs.into_iter().next() {
        app.fio.page_pick = Some(PagePick { path, count, page: 1, all: false, at, open });
    }
}

fn finish_open(app: &mut DarkBurn, results: Results) {
    for (path, result) in results {
        match result {
            // The window stayed usable while the file was read. Anything done to the project in
            // that time would be thrown away with it, so the question is asked (again).
            Ok(ReadOutcome::Loaded(loaded)) if app.fio.open_revision.take().is_some_and(|r| r != app.editor.revision()) => {
                app.fio.held = Some(loaded);
                crate::files::ask_before_read_file(app);
            }
            Ok(ReadOutcome::Loaded(loaded)) => open_loaded(app, *loaded),
            Ok(ReadOutcome::NeedsPage(n)) => queue_pages(app, vec![(path, n)], None, true),
            Err(e) => {
                app.fio.open_revision = None;
                crate::files::show_message(app, "Could not open", format!("{}: {e}", path.display()));
            }
        }
    }
}

/// The answer to "Save changes?" lets the project go: the file read in the background opens.
pub fn open_held(app: &mut DarkBurn) {
    if let Some(loaded) = app.fio.held.take() {
        open_loaded(app, *loaded);
    }
}

/// Makes `loaded` the open project: a LightBurn project with its layers and notes, or artwork in
/// a new project. Unsaved either way.
pub fn open_loaded(app: &mut DarkBurn, mut loaded: Loaded) {
    let warnings = std::mem::take(&mut loaded.warnings);
    let page: Size = crate::files::import_options(app).page;
    crate::files::new_project(app);
    if loaded.project {
        let doc = formats::project_document(loaded);
        app.notes_open = doc.show_notes_on_load;
        let clipboard = std::mem::take(&mut app.editor.clipboard);
        app.editor = Editor::with_document(doc);
        app.editor.clipboard = clipboard;
        app.editor.touch();
    } else {
        loaded.imported.place(page, None);
        let layers = loaded.layers.clone();
        crate::files::insert_with_layers(app, vec![loaded.imported], layers);
        app.editor.history.clear();
    }
    if !warnings.is_empty() {
        app.fio.warnings = Some(vec![("Opened file".into(), warnings)]);
    }
}

/// Opens a non-project file now (no window needed).
pub fn open_sync(app: &mut DarkBurn, path: &Path) -> Result<(), String> {
    if formats::is_device_profile(path) {
        device_profile_hook(app, path);
        return Ok(());
    }
    let mut opts = read_options(app);
    opts.pdf_page = Some(PageChoice::Page(1));
    match formats::read_file(path, &opts)? {
        ReadOutcome::Loaded(l) => {
            open_loaded(app, *l);
            Ok(())
        }
        ReadOutcome::NeedsPage(_) => Err("choose a page to open".into()),
    }
}

/// A device profile (`.dbdev`) was opened or dropped. The devices code takes the paths from
/// [`FileIo::device_profiles`]; until it exists the user is told.
pub fn device_profile_hook(app: &mut DarkBurn, path: &Path) {
    app.fio.device_profiles.push(path.to_path_buf());
    crate::machine_ui::import_profile_file(app, path);
}

// ----- Export -----

/// The folder and a file name to offer in a Save dialog.
fn export_start(app: &DarkBurn, ext: &str) -> (Option<PathBuf>, String) {
    let stem = app.project_path.as_deref().and_then(Path::file_stem).map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Untitled".into());
    let dir = app
        .prefs
        .files
        .export_dir
        .as_deref()
        .map(PathBuf::from)
        .filter(|d| d.is_dir())
        .or_else(|| app.project_path.as_deref().and_then(Path::parent).map(Path::to_path_buf));
    (dir, format!("{stem}.{ext}"))
}

/// A request from the app's preferences and selection.
pub fn request(app: &DarkBurn, format: ExportFormat, selection_only: bool) -> ExportRequest {
    let mut r = ExportRequest::new(format, (app.prefs.page_width as f64, app.prefs.page_height as f64));
    r.dxf_arcs = app.prefs.files.dxf_arcs;
    r.dxf_layer_names = app.prefs.files.dxf_layer_names;
    if selection_only || format.is_bitmap() {
        r.selection = Some(app.editor.selected());
    }
    r
}

/// Writes `req` to `path` and remembers the extension and folder. Returns the warnings.
pub fn export_to(app: &mut DarkBurn, path: &Path, req: &ExportRequest) -> Result<Vec<String>, String> {
    let out = formats::export(&app.editor.doc, req)?;
    crate::files::write_atomic(path, &out.bytes).map_err(|e| e.to_string())?;
    if !req.format.is_gweike()
        && let Some(ext) = path.extension().and_then(|e| e.to_str())
        && ExportFormat::from_ext(ext).is_some()
    {
        app.prefs.files.export_ext = ext.to_ascii_lowercase();
    }
    app.prefs.files.export_dir = path.parent().map(|p| p.to_string_lossy().into_owned()).filter(|p| !p.is_empty());
    app.prefs.files.export_selection = req.selection.is_some();
    Ok(out.warnings)
}

/// Writes the PDF for printing or saving.
pub fn print_to(app: &DarkBurn, path: &Path, black_only: bool, selection_only: bool) -> Result<Vec<String>, String> {
    let sel = app.editor.selected();
    let ids = selection_only.then_some(sel.as_slice());
    let out = formats::print_pdf(&app.editor.doc, ids, black_only)?;
    crate::files::write_atomic(path, &out.bytes).map_err(|e| e.to_string())?;
    Ok(out.warnings)
}

#[derive(Clone, Debug, PartialEq)]
struct ExportDialogResult {
    format_ext: String,
    format: usize,
    selection_only: bool,
    svg_work_area: bool,
    dxf_arcs: bool,
    dxf_layers: bool,
    materials: Vec<(String, String)>,
    default_material: String,
    nc_on: String,
    nc_off: String,
}

struct ExportDialog {
    title: &'static str,
    formats: Vec<ExportFormat>,
    format: usize,
    selection_only: bool,
    svg_work_area: bool,
    dxf_arcs: bool,
    dxf_layers: bool,
    materials: std::collections::BTreeMap<String, String>,
    layer_names: Vec<String>,
    default_material: String,
    nc_on: String,
    nc_off: String,
    has_selection: bool,
}

struct PrintDialog {
    black_only: bool,
    selection_only: bool,
}

fn open_export(app: &mut DarkBurn, title: &'static str, formats_list: Vec<ExportFormat>) {
    let sel = app.editor.selected();
    let want = ExportFormat::from_ext(&app.prefs.files.export_ext);
    let format = formats_list.iter().position(|f| Some(*f) == want).unwrap_or(0);
    let layer_names = formats::export_layer_names(&app.editor.doc, None);
    app.fio.export = Some(ExportDialog {
        title,
        formats: formats_list,
        format,
        selection_only: app.prefs.files.export_selection && !sel.is_empty(),
        svg_work_area: false,
        dxf_arcs: app.prefs.files.dxf_arcs,
        dxf_layers: app.prefs.files.dxf_layer_names,
        materials: app.machine.active().settings.gweike.materials(&app.editor.doc),
        layer_names,
        default_material: String::new(),
        nc_on: "M07".into(),
        nc_off: "M08".into(),
        has_selection: !sel.is_empty(),
    });
}

fn standard_formats(app: &DarkBurn) -> Vec<ExportFormat> {
    let mut v = vec![ExportFormat::Svg, ExportFormat::Ai, ExportFormat::Dxf];
    if formats::selection_has_bitmap(&app.editor.doc, &app.editor.selected()) {
        v.extend([ExportFormat::Png, ExportFormat::Jpg, ExportFormat::Bmp]);
    }
    v.push(ExportFormat::Lbrn2);
    v
}

// ----- Commands -----

/// Runs a file command; returns whether `id` was one of this module's.
pub fn run(app: &mut DarkBurn, ctx: &egui::Context, id: &str) -> bool {
    match id {
        "file.import_options" => app.fio.options_open = true,
        "file.export" => {
            let list = standard_formats(app);
            open_export(app, "Export", list);
        }
        "file.export_gweike_dxf" => open_export(app, "Export Gweike DXF", vec![ExportFormat::GweikeDxf]),
        "file.export_gweike_nc" => open_export(app, "Export Gweike G-code", vec![ExportFormat::GweikeNc]),
        "file.save_lbrn2" => app.fio.pick = Some(Pick::SaveLightBurn),
        "file.print" => app.fio.print = Some(PrintDialog { black_only: true, selection_only: false }),
        "file.print_colour" => app.fio.print = Some(PrintDialog { black_only: false, selection_only: false }),
        "file.prefs_export" => app.fio.pick = Some(Pick::PrefsExport),
        "file.prefs_import" => app.fio.pick = Some(Pick::PrefsImport),
        "file.prefs_backup" => match &app.settings_dir {
            Some(dir) => app.fio.backups = Some(prefs_files::list_backups(dir)),
            None => crate::files::show_message(app, "Preferences backups", "There is no settings folder in this session.".into()),
        },
        "file.new_window" => new_window(app, ctx),
        _ => return false,
    }
    true
}

impl FileIo {
    /// Whether a file window (a picker, the export window or the print window) waits to open.
    pub fn file_window_waiting(&self) -> bool {
        self.pick.is_some() || self.export.is_some() || self.print.is_some()
    }

    /// Forget a file window that is waiting to open.
    pub fn drop_file_window(&mut self) {
        self.pick = None;
        self.export = None;
        self.print = None;
    }
}

/// File > Import: pick files and read them in the background.
pub fn request_import_dialog(app: &mut DarkBurn) {
    app.fio.pick = Some(Pick::Import);
}

fn new_window(app: &mut DarkBurn, ctx: &egui::Context) {
    if app.prefs.files.single_instance {
        // One window only: a new window is a new project in this one.
        crate::files::run(app, ctx, "file.new");
        return;
    }
    if let Err(e) = spawn_window(&[]) {
        crate::files::show_message(app, "New window", format!("Could not start another copy of DarkBurn: {e}"));
    }
}

/// Starts another copy of DarkBurn (an extra window with its own project).
pub fn spawn_window(paths: &[PathBuf]) -> Result<(), String> {
    if cfg!(test) {
        return Ok(()); // tests never start another copy
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    std::process::Command::new(exe).arg("--secondary").args(paths).spawn().map(|_| ()).map_err(|e| e.to_string())
}

// ----- Frame -----

/// Once a frame: finished workers, start-up files, the other copies' inbox, and any file picker
/// a command asked for.
pub fn frame(ctx: &egui::Context, app: &mut DarkBurn) {
    if !app.fio.started {
        app.fio.started = true;
        if let Some(dir) = app.settings_dir.clone()
            && !app.safe_mode
            && !app.fio.secondary
        {
            prefs_files::write_backup(&dir, &app.prefs, false);
        }
        let pending = std::mem::take(&mut app.fio.pending_open);
        open_arguments(app, ctx, pending);
    }
    poll_job(app);
    // "Save changes?" was answered with Cancel: the file that was read is let go.
    if app.fio.held.is_some() && crate::files::asking(app) != Some(&crate::files::Then::OpenRead) {
        app.fio.held = None;
    }
    if app.fio.job.is_some() {
        ctx.request_repaint_after(Duration::from_millis(100));
    }
    instance_tick(ctx, app);
    if let Some(pick) = app.fio.pick.take() {
        run_pick(app, pick);
    }
}

/// Files given to a running copy or on the command line: a project opens, artwork is imported.
fn open_arguments(app: &mut DarkBurn, ctx: &egui::Context, paths: Vec<PathBuf>) {
    crate::files::drop_files(app, ctx, paths, None);
}

pub(crate) fn instance_tick(ctx: &egui::Context, app: &mut DarkBurn) {
    let Some(dir) = app.settings_dir.clone() else { return };
    if !app.prefs.files.single_instance || app.fio.secondary || app.safe_mode {
        return;
    }
    let now = Instant::now();
    ctx.request_repaint_after(crate::instance::TICK);
    if app.fio.next_tick.is_some_and(|t| now < t) {
        return;
    }
    app.fio.next_tick = Some(now + crate::instance::TICK);
    crate::instance::heartbeat(&dir);
    for launch in crate::instance::take_inbox(&dir) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        if launch.is_empty() {
            crate::files::run(app, ctx, "file.new");
        } else {
            open_arguments(app, ctx, launch);
        }
    }
}

fn run_pick(app: &mut DarkBurn, pick: Pick) {
    match pick {
        Pick::Import => {
            let all: Vec<&str> = import::import_extensions().into_iter().chain(formats::LIBRARY_EXTENSIONS.iter().copied()).collect();
            let mut dialog = rfd::FileDialog::new()
                .add_filter("All artwork and images", &all)
                .add_filter("SVG", import::VECTOR_EXTENSIONS)
                .add_filter("DXF, PDF, Illustrator, PLT, RD", &["dxf", "pdf", "ai", "plt", "hpgl", "hgl", "hpg", "rd"])
                .add_filter("LightBurn projects", &["lbrn", "lbrn2"])
                .add_filter("Images", import::BITMAP_EXTENSIONS);
            if let Some(dir) = app.project_path.as_deref().and_then(Path::parent) {
                dialog = dialog.set_directory(dir);
            }
            if let Some(paths) = dialog.pick_files() {
                start_import(app, paths, None);
            }
        }
        Pick::PrefsExport => {
            if let Some(path) =
                rfd::FileDialog::new().add_filter("DarkBurn preferences", &["json"]).set_file_name("darkburn-preferences.json").save_file()
            {
                let r = prefs_files::export(&app.prefs, &path);
                if let Err(e) = r {
                    crate::files::show_message(app, "Could not export preferences", e);
                }
            }
        }
        Pick::PrefsImport => {
            if let Some(path) = rfd::FileDialog::new().add_filter("DarkBurn preferences", &["json"]).pick_file()
                && let Err(e) = import_prefs(app, &path)
            {
                crate::files::show_message(app, "Could not import preferences", format!("{}: {e}", path.display()));
            }
        }
        Pick::SaveLightBurn => {
            let (dir, name) = export_start(app, "lbrn2");
            let mut d = rfd::FileDialog::new().add_filter("LightBurn project", &["lbrn2"]).set_file_name(name);
            if let Some(dir) = dir {
                d = d.set_directory(dir);
            }
            if let Some(mut path) = d.save_file() {
                if path.extension().is_none() {
                    path.set_extension("lbrn2");
                }
                let req = request(app, ExportFormat::Lbrn2, false);
                finish_export(app, &path, &req);
            }
        }
        Pick::SaveExport(r) => {
            let format = r.format_ext.as_str();
            let ext = ExportFormat::from_name(format).unwrap_or(ExportFormat::Svg);
            let (dir, name) = export_start(app, ext.ext());
            let mut d = rfd::FileDialog::new().add_filter(ext.label(), &[ext.ext()]).set_file_name(name);
            if let Some(dir) = dir {
                d = d.set_directory(dir);
            }
            if let Some(mut path) = d.save_file() {
                if path.extension().is_none() {
                    path.set_extension(ext.ext());
                }
                let mut req = request(app, ext, r.selection_only);
                req.svg_work_area = r.svg_work_area;
                req.dxf_arcs = r.dxf_arcs;
                req.dxf_layer_names = r.dxf_layers;
                req.materials = r.materials.iter().filter(|(_, m)| !m.trim().is_empty()).cloned().collect();
                req.default_material = Some(r.default_material.clone());
                req.nc_on = r.nc_on.clone();
                req.nc_off = r.nc_off.clone();
                app.prefs.files.dxf_arcs = r.dxf_arcs;
                app.prefs.files.dxf_layer_names = r.dxf_layers;
                finish_export(app, &path, &req);
            }
        }
        Pick::SavePrintPdf { black_only, selection_only } => {
            let (dir, name) = export_start(app, "pdf");
            let mut d = rfd::FileDialog::new().add_filter("PDF", &["pdf"]).set_file_name(name);
            if let Some(dir) = dir {
                d = d.set_directory(dir);
            }
            if let Some(path) = d.save_file() {
                match print_to(app, &path, black_only, selection_only) {
                    Ok(w) => report_warnings(app, "PDF", w),
                    Err(e) => crate::files::show_message(app, "Could not save the PDF", e),
                }
            }
        }
    }
}

fn finish_export(app: &mut DarkBurn, path: &Path, req: &ExportRequest) {
    match export_to(app, path, req) {
        Ok(w) => {
            app.note = Some(format!("Exported {}", path.display()));
            report_warnings(app, &file_name(path), w);
        }
        Err(e) => crate::files::show_message(app, "Could not export", format!("{}: {e}", path.display())),
    }
}

fn report_warnings(app: &mut DarkBurn, name: &str, warnings: Vec<String>) {
    if !warnings.is_empty() {
        app.fio.warnings = Some(vec![(name.to_string(), warnings)]);
    }
}

/// Imports preferences from a file (the old ones are backed up first).
pub fn import_prefs(app: &mut DarkBurn, path: &Path) -> Result<(), String> {
    let dir = app.settings_dir.clone().filter(|_| !app.safe_mode);
    prefs_files::import(&mut app.prefs, path, dir.as_deref())?;
    // The file's work area is another machine's: the job is built for the active device, so
    // the canvas has to show that device's bed.
    crate::machine_ui::work_area_from_device(app);
    Ok(())
}

/// Starts the OS print of a PDF.
pub fn os_print(path: &Path) -> Result<(), String> {
    if cfg!(test) {
        return Ok(()); // tests never print
    }
    #[cfg(target_os = "windows")]
    let child = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-WindowStyle",
            "Hidden",
            "-Command",
            &format!("Start-Process -FilePath '{}' -Verb Print", path.display().to_string().replace('\'', "''")),
        ])
        .spawn();
    #[cfg(not(target_os = "windows"))]
    let child = std::process::Command::new("lp").arg(path).spawn();
    child.map(|_| ()).map_err(|e| e.to_string())
}

// ----- JSON commands -----

fn text<'a>(a: &'a Value, key: &str) -> Result<&'a str, String> {
    a.get(key).and_then(Value::as_str).ok_or_else(|| format!("argument {key} must be text"))
}

/// The commands that take arguments; None when `id` is not one of them.
pub fn with_args(app: &mut DarkBurn, id: &str, a: &Value) -> Option<Result<Value, String>> {
    Some(match id {
        "project.export" => (|| {
            let path = PathBuf::from(text(a, "path")?);
            let format = match a.get("format").and_then(Value::as_str) {
                Some(name) => ExportFormat::from_name(name).ok_or_else(|| format!("unknown export format {name}"))?,
                None => path
                    .extension()
                    .and_then(|e| e.to_str())
                    .and_then(ExportFormat::from_ext)
                    .ok_or("give a format, or a file name with a known extension")?,
            };
            let flag = |k: &str| a.get(k).and_then(Value::as_bool);
            let mut req = request(app, format, flag("selection_only").unwrap_or(false));
            if let Some(v) = flag("dxf_arcs") {
                req.dxf_arcs = v;
            }
            if let Some(v) = flag("dxf_layer_names") {
                req.dxf_layer_names = v;
            }
            req.svg_work_area = flag("svg_work_area").unwrap_or(false);
            if let Some(m) = a.get("materials").and_then(Value::as_object) {
                req.materials = m.iter().filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string()))).collect();
            }
            req.default_material = a.get("default_material").and_then(Value::as_str).map(str::to_string);
            if format.is_gweike() && req.materials.is_empty() {
                // The layer to material mapping of the Gweike window.
                req.materials = app.machine.active().settings.gweike.materials(&app.editor.doc).into_iter().collect();
            }
            let warnings = export_to(app, &path, &req)?;
            Ok(json!({ "path": path.display().to_string(), "warnings": warnings }))
        })(),
        "project.print_pdf" => (|| {
            let path = PathBuf::from(text(a, "path")?);
            let black = a.get("black_only").and_then(Value::as_bool).unwrap_or(true);
            let sel = a.get("selection_only").and_then(Value::as_bool).unwrap_or(false);
            let warnings = print_to(app, &path, black, sel)?;
            Ok(json!({ "path": path.display().to_string(), "warnings": warnings }))
        })(),
        "project.prefs_export" => text(a, "path").and_then(|p| prefs_files::export(&app.prefs, Path::new(p))).map(|_| json!({ "ok": true })),
        "project.prefs_import" => text(a, "path").and_then(|p| import_prefs(app, Path::new(p))).map(|_| json!({ "ok": true })),
        "project.import" => (|| {
            let paths: Vec<PathBuf> = a
                .get("paths")
                .and_then(Value::as_array)
                .ok_or("argument paths must be a list")?
                .iter()
                .filter_map(Value::as_str)
                .map(PathBuf::from)
                .collect();
            let at = match (a.get("x").and_then(Value::as_f64), a.get("y").and_then(Value::as_f64)) {
                (Some(x), Some(y)) if x.is_finite() && y.is_finite() => Some(Point::new(x, y)),
                _ => None,
            };
            let page = match a.get("pdf_page") {
                Some(Value::String(s)) if s == "all" => Some(PageChoice::All),
                Some(v) => v.as_u64().map(|p| PageChoice::Page(p as usize)),
                None => None,
            };
            let report = import_sync(app, &paths, at, page);
            if report.ids.is_empty() && !report.errors.is_empty() {
                return Err(report.errors.join("; "));
            }
            show_report(app, &report);
            Ok(json!({
                "imported": report.ids.iter().map(|i| i.0).collect::<Vec<_>>(),
                "errors": report.errors,
                "warnings": report.warnings.iter().flat_map(|(f, w)| w.iter().map(move |w| format!("{f}: {w}"))).collect::<Vec<_>>(),
            }))
        })(),
        _ => return None,
    })
}

// ----- Windows -----

/// A short explanation under a control; wraps instead of running out of the window.
fn note(ui: &mut egui::Ui, text: &str) {
    ui.add(egui::Label::new(egui::RichText::new(text).small().weak()).wrap());
}

/// The windows of this module.
pub fn dialogs(ctx: &egui::Context, app: &mut DarkBurn) {
    progress_window(ctx, app);
    page_window(ctx, app);
    warnings_window(ctx, app);
    options_window(ctx, app);
    export_window(ctx, app);
    print_window(ctx, app);
    backups_window(ctx, app);
}

fn progress_window(ctx: &egui::Context, app: &mut DarkBurn) {
    let Some(job) = &app.fio.job else { return };
    let (done, total) = (job.done.load(Ordering::Relaxed), job.total);
    let current = job.current.lock().map(|c| c.clone()).unwrap_or_default();
    let (_, action) = ui_kit::Window::new(egui::Id::unique("db_import_progress"), "import", "Reading files")
        .actions("", "Cancel", None)
        .required()
        .show(ctx, true, |ui| {
            ui.label(format!("Reading {current} ({} of {total})", (done + 1).min(total)));
            ui.add(egui::ProgressBar::new(done as f32 / total.max(1) as f32).animate(true));
            note(ui, "You can keep working while large files are read.");
        });
    if action == ui_kit::Action::Cancel
        && let Some(job) = app.fio.job.take()
    {
        job.cancel.store(true, Ordering::Relaxed);
    }
}

fn page_window(ctx: &egui::Context, app: &mut DarkBurn) {
    let Some(p) = &mut app.fio.page_pick else { return };
    let name = file_name(&p.path);
    let count = p.count as u32;
    let (_, action) = ui_kit::Window::new(egui::Id::unique("db_pdf_pages"), "import", "Choose a page")
        .fields()
        .actions("Import", "Cancel", None)
        .show(ctx, true, |ui| {
            ui.label(format!("{name} has {count} pages."));
            ui_kit::checkbox(ui, "Import every page (each in its own group, on top of each other)", &mut p.all);
            if !p.all {
                ui_kit::row(ui, "Page", |ui| {
                    ui_kit::number_field(ui, &mut p.page, 1..=count, 1.0, "");
                });
            }
        });
    match action {
        ui_kit::Action::Ok => {
            if let Some(p) = app.fio.page_pick.take() {
                let choice = if p.all { PageChoice::All } else { PageChoice::Page(p.page as usize) };
                start_job(app, vec![p.path], p.at, p.open, Some(choice));
            }
        }
        ui_kit::Action::Cancel => app.fio.page_pick = None,
        _ => {}
    }
}

fn warnings_window(ctx: &egui::Context, app: &mut DarkBurn) {
    let Some(list) = app.fio.warnings.clone() else { return };
    let (closed, action) = ui_kit::Window::new(egui::Id::unique("db_import_warnings"), "db_app", "Things to know about this file")
        .actions("", "Close", None)
        .show(ctx, true, |ui| {
            egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                for (file, warnings) in &list {
                    ui_kit::section_heading(ui, file);
                    for w in warnings {
                        ui.label(format!("- {w}"));
                    }
                }
            });
        });
    if closed || action != ui_kit::Action::None {
        app.fio.warnings = None;
    }
}

fn options_window(ctx: &egui::Context, app: &mut DarkBurn) {
    if !app.fio.options_open {
        return;
    }
    let (closed, action) = {
        let prefs = &mut app.prefs;
        ui_kit::Window::new(egui::Id::unique("db_import_options"), "import", "Import options")
            .dockable("db_import_options")
            .fields()
            .actions("", "Close", None)
            .show(ctx, true, |ui| import_options_body(ui, prefs))
    };
    if closed || action != ui_kit::Action::None {
        app.fio.options_open = false;
    }
}

/// The import options (also shown in Settings).
pub fn import_options_body(ui: &mut egui::Ui, prefs: &mut crate::prefs::Prefs) {
    use formats::{ColourMap, LengthUnit, UnitMode};
    ui_kit::section_heading(ui, "All files");
    ui_kit::row(ui, "Colours become layers", |ui| {
        let mut i = ColourMap::ALL.iter().position(|c| *c == prefs.files.colour_map).unwrap_or(0);
        let labels: Vec<_> = ColourMap::ALL.iter().map(|c| c.label()).collect();
        ui_kit::dropdown(ui, "colour_map", &mut i, &labels);
        prefs.files.colour_map = ColourMap::ALL[i];
    });
    ui_kit::row(ui, "SVG resolution", |ui| {
        let mut i = usize::from(prefs.svg_dpi != 72.0);
        ui_kit::dropdown(ui, "opt_svg_dpi", &mut i, &["72 DPI", "96 DPI"]);
        prefs.svg_dpi = if i == 0 { 72.0 } else { 96.0 };
    });
    ui_kit::checkbox(ui, "Group imported shapes", &mut prefs.import_group);
    ui_kit::checkbox(ui, "Select imported shapes", &mut prefs.import_select);
    ui_kit::checkbox(ui, "Allow import to tool layers", &mut prefs.import_tool_layers);
    ui.add_space(8.0);
    ui_kit::section_heading(ui, "DXF");
    ui_kit::row(ui, "Units", |ui| {
        let mut i = UnitMode::ALL.iter().position(|c| *c == prefs.files.dxf_units).unwrap_or(0);
        let labels: Vec<_> = UnitMode::ALL.iter().map(|c| c.label()).collect();
        ui_kit::dropdown(ui, "dxf_units", &mut i, &labels);
        prefs.files.dxf_units = UnitMode::ALL[i];
    });
    if prefs.files.dxf_units != UnitMode::Auto {
        ui_kit::row(ui, "Unit", |ui| {
            let mut i = LengthUnit::ALL.iter().position(|c| *c == prefs.files.dxf_unit).unwrap_or(0);
            let labels: Vec<_> = LengthUnit::ALL.iter().map(|c| c.label()).collect();
            ui_kit::dropdown(ui, "dxf_unit", &mut i, &labels);
            prefs.files.dxf_unit = LengthUnit::ALL[i];
        });
    }
    ui_kit::checkbox(ui, "Join open paths whose ends meet", &mut prefs.files.auto_join);
    if prefs.files.auto_join {
        ui_kit::row(ui, "Join tolerance", |ui| {
            ui_kit::number_field(ui, &mut prefs.files.auto_join_tolerance, 0.0..=10.0, 0.01, " mm");
        });
    }
    ui.add_space(8.0);
    ui_kit::section_heading(ui, "Illustrator and PDF");
    ui_kit::checkbox(ui, "Import layers that are switched off", &mut prefs.files.hidden_ai_layers);
    ui.add_space(8.0);
    ui_kit::section_heading(ui, "PLT");
    ui_kit::row(ui, "Units per millimetre", |ui| {
        ui_kit::number_field(ui, &mut prefs.files.hpgl_units_per_mm, 1.0..=10_000.0, 1.0, "");
    });
    ui.add_space(8.0);
    ui_kit::section_heading(ui, "Export");
    ui_kit::checkbox(ui, "DXF export writes arcs (off: straight segments)", &mut prefs.files.dxf_arcs);
    ui_kit::checkbox(ui, "DXF export names layers after the cut layers (off: after colours)", &mut prefs.files.dxf_layer_names);
    ui.add_space(8.0);
    ui_kit::section_heading(ui, "Windows");
    ui_kit::checkbox(ui, "Only one DarkBurn at a time (files go to the running copy)", &mut prefs.files.single_instance);
}

fn export_window(ctx: &egui::Context, app: &mut DarkBurn) {
    let Some(d) = &mut app.fio.export else { return };
    let title = d.title;
    let current = d.formats[d.format.min(d.formats.len() - 1)];
    let (closed, action) =
        ui_kit::Window::new(egui::Id::unique("db_export"), "export", title).fields().actions("Export", "Cancel", None).show(ctx, true, |ui| {
            if d.formats.len() > 1 {
                ui_kit::row(ui, "Format", |ui| {
                    let labels: Vec<_> = d.formats.iter().map(|f| f.label()).collect();
                    ui_kit::dropdown(ui, "export_format", &mut d.format, &labels);
                });
            }
            if d.has_selection {
                ui_kit::checkbox(ui, "Only the selected shapes", &mut d.selection_only);
            } else {
                d.selection_only = false;
                note(ui, "Nothing is selected: everything is exported.");
            }
            match current {
                ExportFormat::Svg => {
                    ui_kit::checkbox(ui, "Page is the whole work area (else just the artwork)", &mut d.svg_work_area);
                }
                ExportFormat::Ai => {
                    note(ui, "A PDF-compatible Illustrator file with one layer per cut layer.");
                }
                ExportFormat::Dxf => {
                    ui_kit::checkbox(ui, "Write arcs (off: straight segments)", &mut d.dxf_arcs);
                    ui_kit::checkbox(ui, "Name DXF layers after the cut layers (off: after colours)", &mut d.dxf_layers);
                }
                ExportFormat::Lbrn2 => {
                    note(ui, "A LightBurn-format file for other software. Open it in DarkBurn with File > Open.");
                }
                ExportFormat::Png | ExportFormat::Jpg | ExportFormat::Bmp => {
                    note(ui, "Saves the selected bitmap as it is stored in the project.");
                }
                ExportFormat::GweikeDxf | ExportFormat::GweikeNc => {
                    ui_kit::section_heading(ui, "Material for each layer");
                    for name in d.layer_names.clone() {
                        let entry = d.materials.entry(name.clone()).or_default();
                        ui_kit::row(ui, &name, |ui| {
                            ui_kit::text_field(ui, entry, "material name");
                        });
                    }
                    ui_kit::row(ui, "Any other layer", |ui| {
                        ui_kit::text_field(ui, &mut d.default_material, "(its layer name)");
                    });
                    if current == ExportFormat::GweikeNc {
                        ui_kit::row(ui, "Cut on code", |ui| {
                            ui_kit::text_field(ui, &mut d.nc_on, "M07");
                        });
                        ui_kit::row(ui, "Cut off code", |ui| {
                            ui_kit::text_field(ui, &mut d.nc_off, "M08");
                        });
                    }
                    note(ui, "Geometry only: the Gweike software applies its own cutting settings.");
                }
            }
        });
    match action {
        ui_kit::Action::Ok => {
            if let Some(d) = app.fio.export.take() {
                let f = d.formats[d.format.min(d.formats.len() - 1)];
                let name = match f {
                    ExportFormat::GweikeDxf => "gweike_dxf",
                    ExportFormat::GweikeNc => "gweike_nc",
                    other => other.ext(),
                };
                app.fio.pick = Some(Pick::SaveExport(Box::new(ExportDialogResult {
                    format_ext: name.into(),
                    format: d.format,
                    selection_only: d.selection_only || f.is_bitmap(),
                    svg_work_area: d.svg_work_area,
                    dxf_arcs: d.dxf_arcs,
                    dxf_layers: d.dxf_layers,
                    materials: d.materials.into_iter().collect(),
                    default_material: d.default_material,
                    nc_on: d.nc_on,
                    nc_off: d.nc_off,
                })));
            }
        }
        ui_kit::Action::Cancel => app.fio.export = None,
        _ if closed => app.fio.export = None,
        _ => {}
    }
}

fn print_window(ctx: &egui::Context, app: &mut DarkBurn) {
    let Some(d) = &mut app.fio.print else { return };
    let has_selection = !app.editor.selection.is_empty();
    let (closed, action) = ui_kit::Window::new(egui::Id::unique("db_print"), "export", "Print")
        .fields()
        .actions("Print", "Cancel", Some("Save as PDF"))
        .show(ctx, true, |ui| {
            ui_kit::checkbox(ui, "Black lines only (off: keep the layer colours)", &mut d.black_only);
            if has_selection {
                ui_kit::checkbox(ui, "Only the selected shapes", &mut d.selection_only);
            } else {
                d.selection_only = false;
            }
            note(ui, "Prints through the system's PDF viewer. If nothing prints, use Save as PDF.");
        });
    let (black_only, selection_only) = (d.black_only, d.selection_only);
    match action {
        ui_kit::Action::Ok => {
            app.fio.print = None;
            let path = std::env::temp_dir().join(format!("DarkBurn-print-{}.pdf", std::process::id()));
            let result = print_to(app, &path, black_only, selection_only).and_then(|w| {
                report_warnings(app, "Print", w);
                os_print(&path)
            });
            if let Err(e) = result {
                crate::files::show_message(app, "Could not print", format!("{e}\nUse Save as PDF and print the file from a PDF viewer."));
            }
        }
        ui_kit::Action::Apply => {
            app.fio.print = None;
            app.fio.pick = Some(Pick::SavePrintPdf { black_only, selection_only });
        }
        ui_kit::Action::Cancel => app.fio.print = None,
        _ if closed => app.fio.print = None,
        _ => {}
    }
}

fn backups_window(ctx: &egui::Context, app: &mut DarkBurn) {
    let Some(list) = app.fio.backups.clone() else { return };
    let mut load: Option<PathBuf> = None;
    let (closed, action) = ui_kit::Window::new(egui::Id::unique("db_prefs_backups"), "folder", "Preferences backups")
        .dockable("db_prefs_backups")
        .actions("", "Close", None)
        .show(ctx, true, |ui| {
            if list.is_empty() {
                ui.label("There are no backups yet. One is made when DarkBurn starts with changed settings.");
            }
            egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                for b in &list {
                    ui_kit::row(ui, &prefs_files::format_time(b.time), |ui| {
                        if ui_kit::text_button(ui, "Load", true).clicked() {
                            load = Some(b.path.clone());
                        }
                    });
                }
            });
        });
    if let Some(path) = load {
        app.fio.backups = None;
        if let Err(e) = import_prefs(app, &path) {
            crate::files::show_message(app, "Could not load the backup", e);
        }
    } else if closed || action != ui_kit::Action::None {
        app.fio.backups = None;
    }
}

/// What `files::open_path` and `files::drop_files` need to know about a path.
pub fn is_openable_project(path: &Path) -> bool {
    import::kind_of(path) == FileKind::Project || formats::is_lightburn(path)
}

#[cfg(test)]
mod tests;
