//! The in-app updater. With the setting `Check for updates` on (the default), a few seconds
//! after start and then once an hour a background thread asks GitHub for the latest DarkBurn
//! release. When it is newer than this build, a small amber "↑ Install vX" link appears next to
//! the version in the header; its tooltip lists one line per newer release from CHANGELOG.md.
//!
//! "Newer" is the latest published release (`releases/latest`), not the version on main: only a
//! release has binaries to download. Clicking the link asks first, then downloads this
//! platform's archive from that release, unpacks it, moves the running program aside (which
//! also works on Windows, where a running program cannot be overwritten but can be renamed),
//! puts the new one in its place and restarts. Any failure stops there and puts the old program
//! back. When the release has no archive for this platform the link opens the release page.
//!
//! All network work runs on threads; the window never waits for it.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use darkburn_ui::ui_kit;
use egui::{Context, RichText};
use solidcraft_theme::Tokens;

use crate::i18n::tr;
use crate::shell::DarkBurn;
use crate::updates;

/// The repository whose releases are installed.
pub const REPO: &str = "phobicdotno/DarkBurn";
/// The colour of the link in the header.
pub const AMBER: egui::Color32 = egui::Color32::from_rgb(0xf2, 0xa5, 0x36);
/// The first check after start.
const FIRST_CHECK: Duration = Duration::from_secs(4);
/// Then one every hour.
const EVERY: Duration = Duration::from_secs(3600);
/// No release archive is anywhere near this big.
const MAX_DOWNLOAD: u64 = 400 << 20;

/// One downloadable file of a release.
#[derive(Clone, Debug, PartialEq)]
pub struct Asset {
    pub name: String,
    pub url: String,
}

/// A release newer than this build.
#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    /// Without the leading "v".
    pub version: String,
    /// The release's page on GitHub.
    pub page: String,
    /// This platform's archive, when the release has one.
    pub asset: Option<Asset>,
    /// One line per release newer than this build, newest first.
    pub notes: Vec<String>,
}

/// The operating system and architecture words used in release file names, for this build.
pub fn this_platform() -> (&'static str, &'static str) {
    (std::env::consts::OS, std::env::consts::ARCH)
}

/// The release file for `version` on `os`/`arch` (Rust's `std::env::consts` words), as
/// `.github/workflows/release.yml` names them; None for a platform no release is built for.
pub fn asset_name(version: &str, os: &str, arch: &str) -> Option<String> {
    let v = version.trim().trim_start_matches(['v', 'V']);
    let platform = match (os, arch) {
        ("windows", "x86_64") => return Some(format!("darkburn-{v}-windows-x64-portable.zip")),
        ("linux", "x86_64") => "linux-x86_64",
        ("linux", "aarch64") => "linux-aarch64",
        ("macos", "aarch64") => "macos-arm64",
        ("macos", "x86_64") => "macos-x86_64",
        _ => return None,
    };
    Some(format!("darkburn-{v}-{platform}.tar.gz"))
}

/// The asset of `release_json`'s `assets` list that is meant for `os`/`arch`, with an https
/// address.
pub fn pick_asset(release_json: &serde_json::Value, version: &str, os: &str, arch: &str) -> Option<Asset> {
    let want = asset_name(version, os, arch)?;
    release_json.get("assets")?.as_array()?.iter().find_map(|a| {
        let name = a.get("name")?.as_str()?;
        let url = a.get("browser_download_url")?.as_str()?;
        (name == want && url.starts_with("https://")).then(|| Asset { name: name.to_string(), url: url.to_string() })
    })
}

/// One line per release in `changelog` newer than `current`, newest first: "v0.0.765 — first
/// entry" (with "(+N more)" when the release lists more). Headings look like
/// `## v0.0.764 (build 7)`.
pub fn newer_notes(changelog: &str, current: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut version: Option<String> = None;
    let mut first: Option<String> = None;
    let mut more = 0usize;
    let flush = |version: &mut Option<String>, first: &mut Option<String>, more: &mut usize, out: &mut Vec<String>| {
        if let Some(v) = version.take() {
            let mut line = format!("v{v} \u{2014} {}", first.take().unwrap_or_default());
            if *more > 0 {
                line.push_str(&format!(" (+{more} more)"));
            }
            out.push(line.trim_end_matches([' ', '\u{2014}']).to_string());
        }
        *first = None;
        *more = 0;
    };
    for line in changelog.lines() {
        let l = line.trim();
        if let Some(head) = l.strip_prefix("## ") {
            flush(&mut version, &mut first, &mut more, &mut out);
            let v = head.split_whitespace().next().unwrap_or("").trim_start_matches(['v', 'V']);
            if updates::parse_version(v).is_none() {
                continue;
            }
            if !updates::is_newer(v, current) {
                break;
            }
            version = Some(v.to_string());
        } else if version.is_some()
            && let Some(item) = l.strip_prefix("- ").or_else(|| l.strip_prefix("* "))
        {
            if first.is_none() {
                first = Some(item.trim().to_string());
            } else {
                more += 1;
            }
        }
    }
    flush(&mut version, &mut first, &mut more, &mut out);
    out
}

/// GET `url`, following up to three redirects that keep to https. The response is returned
/// whatever its status.
fn get(agent: &ureq::Agent, url: &str, accept: &str) -> Result<ureq::http::Response<ureq::Body>, String> {
    if !updates::allowed_address(url) {
        return Err("the address must start with https://".into());
    }
    let mut url = url.to_owned();
    for _ in 0..4 {
        let response = agent.get(&url).header("Accept", accept).call().map_err(|e| e.to_string())?;
        if !matches!(response.status().as_u16(), 301 | 302 | 303 | 307 | 308) {
            return Ok(response);
        }
        let location = response.headers().get("location").and_then(|l| l.to_str().ok()).ok_or("a redirect without an address")?;
        url = updates::next_hop(&url, location)?;
    }
    Err("too many redirects".into())
}

/// Read a small text answer.
fn get_text(agent: &ureq::Agent, url: &str, accept: &str) -> Result<String, String> {
    let mut r = get(agent, url, accept)?;
    let status = r.status().as_u16();
    if status != 200 {
        return Err(format!("HTTP {status}"));
    }
    r.body_mut().with_config().limit(2 << 20).read_to_string().map_err(|e| e.to_string())
}

/// One check, blocking: the latest release when it is newer than `current`, None when it is
/// not, Err when the answer could not be had.
pub fn check(api_url: &str, current: &str, os: &str, arch: &str) -> Result<Option<Release>, String> {
    let agent = updates::agent(Duration::from_secs(15));
    let body = get_text(&agent, api_url, "application/json")?;
    let json: serde_json::Value = serde_json::from_str(&body).map_err(|_| "not JSON".to_string())?;
    let tag = json.get("tag_name").or_else(|| json.get("version")).and_then(|t| t.as_str()).unwrap_or("").trim();
    if json.get("draft").and_then(|d| d.as_bool()) == Some(true) || !updates::is_newer(tag, current) {
        return Ok(None);
    }
    let version = tag.trim_start_matches(['v', 'V']).to_string();
    let page = json.get("html_url").and_then(|u| u.as_str()).and_then(updates::safe_link).unwrap_or(updates::RELEASES_PAGE).to_string();
    let asset = pick_asset(&json, &version, os, arch);
    // The notes of the release's own CHANGELOG.md; without them the link still works.
    let notes = updates::safe_link(&format!("https://raw.githubusercontent.com/{REPO}/{tag}/CHANGELOG.md"))
        .filter(|_| api_url.starts_with("https://"))
        .and_then(|u| get_text(&agent, u, "text/plain").ok())
        .map(|c| newer_notes(&c, current))
        .unwrap_or_default();
    Ok(Some(Release { version, page, asset, notes }))
}

/// How far the install is; shared between its thread and the dialog.
#[derive(Clone, Debug, Default)]
pub struct Progress {
    /// 0 downloading, 1 installing, 2 done.
    pub stage: usize,
    /// How far the download is (None while its size is unknown).
    pub download: Option<f32>,
    pub log: Vec<String>,
    /// Set when it stopped: Ok (the program to start) or what went wrong.
    pub finished: Option<Result<PathBuf, String>>,
    /// The program could not be replaced for lack of permission (e.g. an MSI install in
    /// Program Files): the release page is opened instead.
    pub no_permission: bool,
    /// The release page has been opened for `no_permission`.
    pub page_opened: bool,
}

/// The update check and install state.
#[derive(Default)]
pub struct State {
    rx: Option<Receiver<Result<Option<Release>, String>>>,
    next_check: Option<Instant>,
    /// The newer release, while there is one.
    pub available: Option<Release>,
    /// The "Install vX?" question is showing.
    pub ask_open: bool,
    /// The install in progress (or finished, while its dialog shows).
    pub job: Option<Arc<Mutex<Progress>>>,
    pub details: bool,
    restart_sent: bool,
    /// The newer version the Help menu's check last reported, so it is followed up once.
    seen_manual: Option<String>,
}

impl State {
    pub fn checking(&self) -> bool {
        self.rx.is_some()
    }

    /// Start a check on a thread unless one is running.
    pub fn start_check(&mut self, api_url: String) {
        if self.rx.is_some() || (cfg!(test) && !api_url.starts_with("http://127.0.0.1")) {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let (os, arch) = this_platform();
        let spawned = std::thread::Builder::new().name("update-check".into()).spawn(move || {
            let _ = tx.send(check(&api_url, env!("CARGO_PKG_VERSION"), os, arch));
        });
        if spawned.is_ok() {
            self.rx = Some(rx);
        }
    }

    /// Collect a check's answer. A failed check keeps what an earlier one found.
    pub fn poll(&mut self) {
        let Some(rx) = &self.rx else { return };
        match rx.try_recv() {
            Ok(Ok(found)) => {
                self.rx = None;
                self.available = found;
            }
            Ok(Err(e)) => {
                self.rx = None;
                log::info!("update check: {e}");
            }
            Err(TryRecvError::Disconnected) => self.rx = None,
            Err(TryRecvError::Empty) => {}
        }
    }
}

/// The program that was started, as a file to replace.
fn running_program() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("cannot tell where DarkBurn is: {e}"))?;
    Ok(std::fs::canonicalize(&exe).unwrap_or(exe))
}

/// The file in `dir` (searched two folders deep) named `name`.
fn find_file(dir: &Path, name: &str, depth: usize) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut subdirs = Vec::new();
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            subdirs.push(p);
        } else if p.file_name().and_then(|n| n.to_str()) == Some(name) {
            return Some(p);
        }
    }
    if depth == 0 {
        return None;
    }
    subdirs.iter().find_map(|d| find_file(d, name, depth - 1))
}

/// Unpack `archive` into `into` with the system's tar (GNU tar reads .tar.gz, and the bsdtar
/// that comes with Windows 10 and later reads .zip too).
fn unpack(archive: &Path, into: &Path) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    let tar = std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from).join(r"System32\tar.exe");
    #[cfg(not(target_os = "windows"))]
    let tar = PathBuf::from("tar");
    let out = std::process::Command::new(&tar)
        .arg("-xf")
        .arg(archive)
        .arg("-C")
        .arg(into)
        .output()
        .map_err(|e| format!("could not run {}: {e}", tar.display()))?;
    if out.status.success() { Ok(()) } else { Err(format!("unpacking failed: {}", String::from_utf8_lossy(&out.stderr).trim())) }
}

/// Put `new` where `target` is: the old file is first renamed to `<name>.old` beside it (a
/// running program may be renamed on every system), then the new one is copied in. On failure
/// the old one is put back.
pub fn replace_file(target: &Path, new: &Path) -> Result<(), String> {
    replace_checked(target, new).map_err(|(e, _)| e)
}

/// [`replace_file`], also telling whether it failed for lack of permission.
fn replace_checked(target: &Path, new: &Path) -> Result<(), (String, bool)> {
    let denied = |e: &std::io::Error| e.kind() == std::io::ErrorKind::PermissionDenied;
    let mut old_name = target.file_name().ok_or(("no file name".to_string(), false))?.to_os_string();
    old_name.push(".old");
    let old = target.with_file_name(old_name);
    // Left by an earlier update; on Windows it may still be running, then it stays.
    let _ = std::fs::remove_file(&old);
    let had_old = target.exists();
    if had_old {
        std::fs::rename(target, &old).map_err(|e| (format!("cannot move {} aside: {e}", target.display()), denied(&e)))?;
    }
    let copied = std::fs::copy(new, target).map(|_| ()).and_then(|()| make_runnable(target));
    if let Err(e) = copied {
        let _ = std::fs::remove_file(target);
        if had_old {
            let _ = std::fs::rename(&old, target);
        }
        return Err((format!("cannot write {}: {e}", target.display()), denied(&e)));
    }
    Ok(())
}

/// Let the program run (Unix: mode 755; elsewhere nothing to do).
fn make_runnable(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

/// The install, blocking: download, unpack, replace. Reports through `progress`.
fn install(asset: &Asset, version: &str, progress: &Arc<Mutex<Progress>>) -> Result<PathBuf, String> {
    let say = |line: String| {
        if let Ok(mut p) = progress.lock() {
            p.log.push(line);
        }
    };
    let exe = running_program()?;
    let dir = std::env::temp_dir().join(format!("darkburn-update-{version}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot make {}: {e}", dir.display()))?;
    let archive = dir.join(&asset.name);

    say(format!("Downloading {}", asset.url));
    let agent = updates::agent(Duration::from_secs(600));
    let mut r = get(&agent, &asset.url, "application/octet-stream")?;
    let status = r.status().as_u16();
    if status != 200 {
        return Err(format!("download failed: HTTP {status}"));
    }
    let total = r.headers().get("content-length").and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<u64>().ok());
    let mut body = r.body_mut().with_config().limit(MAX_DOWNLOAD).reader();
    let mut file = std::fs::File::create(&archive).map_err(|e| format!("cannot write {}: {e}", archive.display()))?;
    let mut buf = vec![0u8; 64 * 1024];
    let mut got = 0u64;
    loop {
        let n = body.read(&mut buf).map_err(|e| format!("download failed: {e}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|e| format!("cannot write {}: {e}", archive.display()))?;
        got += n as u64;
        if let (Some(t), Ok(mut p)) = (total, progress.lock()) {
            p.download = Some(got as f32 / t.max(1) as f32);
        }
    }
    drop(file);
    if total.is_some_and(|t| t != got) {
        return Err(format!("download stopped after {got} bytes"));
    }
    say(format!("Downloaded {got} bytes to {}", archive.display()));
    if let Ok(mut p) = progress.lock() {
        p.download = Some(1.0);
        p.stage = 1;
    }

    let unpacked = dir.join("unpacked");
    std::fs::create_dir_all(&unpacked).map_err(|e| e.to_string())?;
    unpack(&archive, &unpacked)?;
    say("Unpacked".into());
    let exe_name = exe.file_name().and_then(|n| n.to_str()).ok_or("no program name")?.to_string();
    let program = if cfg!(windows) { "darkburn.exe" } else { "darkburn" };
    let new_exe = find_file(&unpacked, program, 2).ok_or_else(|| format!("{program} is not in the archive"))?;
    if let Err((e, no_permission)) = replace_checked(&exe, &new_exe) {
        if no_permission && let Ok(mut p) = progress.lock() {
            p.no_permission = true;
        }
        return Err(e);
    }
    say(format!("Replaced {exe_name}"));
    // The MCP helper beside it, when this install has one.
    let mcp = if cfg!(windows) { "darkburn-mcp.exe" } else { "darkburn-mcp" };
    if let (Some(new_mcp), Some(folder)) = (find_file(&unpacked, mcp, 2), exe.parent()) {
        let target = folder.join(mcp);
        if target.exists() {
            match replace_file(&target, &new_mcp) {
                Ok(()) => say(format!("Replaced {mcp}")),
                Err(e) => say(format!("{mcp} left as it was: {e}")),
            }
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    if let Ok(mut p) = progress.lock() {
        p.stage = 2;
    }
    Ok(exe)
}

/// The program to start once the window has closed (set by a finished install).
static RESTART: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Call after the window has closed: starts the new version when an install asked for it.
pub fn restart_if_installed() {
    if let Some(exe) = RESTART.lock().ok().and_then(|mut r| r.take()) {
        let _ = std::process::Command::new(exe).spawn();
    }
}

/// The header link was clicked.
pub fn link_clicked(app: &mut DarkBurn) {
    let u = &mut app.extras.updater;
    let Some(rel) = &u.available else { return };
    if rel.asset.is_none() {
        crate::help::open_url(updates::safe_link(&rel.page).unwrap_or(updates::RELEASES_PAGE));
        return;
    }
    if u.job.is_none() {
        u.ask_open = true;
    }
}

/// Once a frame: the timed checks and their answers.
pub fn frame(ctx: &Context, app: &mut DarkBurn) {
    let now = Instant::now();
    let u = &mut app.extras.updater;
    let next = *u.next_check.get_or_insert(now + FIRST_CHECK);
    if app.prefs.opts.check_updates && !app.safe_mode && now >= next && u.job.is_none() {
        u.next_check = Some(now + EVERY);
        let url = updates::address(&app.prefs.opts.update_url);
        u.start_check(url);
    }
    // "Check for updates now" found a newer version: look up its archive and notes for the link.
    if let Some(updates::Outcome::Available { latest, .. }) = &app.extras.updates.result
        && u.seen_manual.as_deref() != Some(latest.as_str())
        && u.available.is_none()
    {
        u.seen_manual = Some(latest.clone());
        u.start_check(updates::address(&app.prefs.opts.update_url));
    }
    if u.checking() {
        u.poll();
        ctx.request_repaint_after(Duration::from_millis(250));
    } else if app.prefs.opts.check_updates {
        ctx.request_repaint_after(next.saturating_duration_since(now).max(Duration::from_millis(250)));
    }
    if u.job.is_some() {
        ctx.request_repaint_after(Duration::from_millis(100));
    }
}

/// Start the install thread.
fn start_install(app: &mut DarkBurn) {
    let Some(rel) = app.extras.updater.available.clone() else { return };
    let Some(asset) = rel.asset.clone() else { return };
    let progress = Arc::new(Mutex::new(Progress::default()));
    app.extras.updater.job = Some(progress.clone());
    app.extras.updater.details = false;
    app.extras.updater.restart_sent = false;
    if cfg!(test) && !asset.url.starts_with("http://127.0.0.1") {
        if let Ok(mut p) = progress.lock() {
            p.finished = Some(Err("no network in tests".into()));
        }
        return;
    }
    let spawned = std::thread::Builder::new().name("update-install".into()).spawn(move || {
        let r = install(&asset, &rel.version, &progress);
        if let Ok(mut p) = progress.lock() {
            if let Err(e) = &r {
                p.log.push(format!("Stopped: {e}"));
            }
            p.finished = Some(r);
        }
    });
    if spawned.is_err()
        && let Some(job) = &app.extras.updater.job
        && let Ok(mut p) = job.lock()
    {
        p.finished = Some(Err("could not start the install".into()));
    }
}

/// The question and the install dialog.
pub fn dialogs(ctx: &Context, app: &mut DarkBurn) {
    let Some(rel) = app.extras.updater.available.clone() else {
        install_dialog(ctx, app, "");
        return;
    };
    if app.extras.updater.ask_open {
        let title = tr("updater.title", "Update");
        let install = tr("updater.install", "Install");
        let (closed, action) = ui_kit::Window::new(egui::Id::unique("db_updater_ask"), "help", &title)
            .actions(&install, &tr("ui.cancel", "Cancel"), None)
            .show(ctx, true, |ui| {
                let t = Tokens::get();
                ui.set_min_width(380.0);
                ui.label(RichText::new(format!("{} v{}?", tr("updater.ask", "Install DarkBurn"), rel.version)).strong());
                if !rel.notes.is_empty() {
                    ui.add_space(t.space_6);
                    ui.add(egui::Label::new(RichText::new(rel.notes.join("\n")).color(t.text_dim)).wrap());
                }
                ui.add_space(t.space_6);
                ui.add(
                    egui::Label::new(tr(
                        "updater.ask_body",
                        "Downloads the new version from GitHub, puts it in place of this one and restarts DarkBurn. You are asked to save unsaved changes before the restart.",
                    ))
                    .wrap(),
                );
            });
        if action == ui_kit::Action::Ok {
            app.extras.updater.ask_open = false;
            start_install(app);
        } else if closed || action != ui_kit::Action::None {
            app.extras.updater.ask_open = false;
        }
    }
    open_page_if_denied(app, &rel.page);
    if app.extras.updater.job.is_some() {
        install_dialog(ctx, app, &rel.version);
    }
}

/// An install that had no permission to replace the program opens the release page, once.
fn open_page_if_denied(app: &mut DarkBurn, page: &str) {
    let Some(job) = &app.extras.updater.job else { return };
    let Ok(mut p) = job.lock() else { return };
    if p.no_permission && !p.page_opened {
        p.page_opened = true;
        crate::help::open_url(updates::safe_link(page).unwrap_or(updates::RELEASES_PAGE));
    }
}

fn install_dialog(ctx: &Context, app: &mut DarkBurn, version: &str) {
    let Some(job) = app.extras.updater.job.clone() else { return };
    let p = job.lock().map(|p| p.clone()).unwrap_or_default();
    let running = p.finished.is_none();
    let head = match &p.finished {
        None if p.stage == 0 => format!("{} v{version}\u{2026}", tr("updater.downloading", "Downloading")),
        None => format!("{} v{version}\u{2026}", tr("updater.installing", "Installing")),
        Some(Ok(_)) => format!("v{version} {}", tr("updater.installed", "installed \u{2014} restarting")),
        Some(Err(_)) if p.no_permission => tr(
            "updater.no_permission",
            "DarkBurn may not replace itself in this folder (for example when installed in Program Files). The release page is open: install the new version from there.",
        ),
        Some(Err(e)) => format!("{} {e}", tr("updater.failed", "The update stopped; this version is unchanged:")),
    };
    let title = format!("{} v{version}", tr("updater.installing", "Installing"));
    let mut close = false;
    let mut details = app.extras.updater.details;
    let (closed, _) = ui_kit::Window::new(egui::Id::unique("db_updater_install"), "help", &title).actions("", "", None).show(ctx, true, |ui| {
        let t = Tokens::get();
        ui.set_min_width(480.0);
        ui.add(egui::Label::new(RichText::new(&head).strong()).wrap());
        let stage_bar = |ui: &mut egui::Ui, label: &str, key: &str, fraction: Option<f32>, active: bool| {
            ui.add_space(t.space_6);
            ui.horizontal(|ui| {
                ui.label(RichText::new(label).color(t.text_dim));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let pct = match fraction {
                        Some(f) => format!("{:.0}%", f * 100.0),
                        None if active => "\u{2026}".into(),
                        None => String::new(),
                    };
                    ui.label(RichText::new(pct).color(t.text_dim));
                });
            });
            ui_kit::progress(ui, key, fraction.unwrap_or(0.0));
        };
        let failed = matches!(p.finished, Some(Err(_)));
        let dl = if p.stage > 0 { Some(1.0) } else { p.download };
        stage_bar(ui, &tr("updater.downloading", "Downloading"), "updater:download", dl, running && p.stage == 0);
        let inst = if p.stage > 1 { Some(1.0) } else { None };
        stage_bar(ui, &tr("updater.installing", "Installing"), "updater:install", inst, running && p.stage == 1 && !failed);
        ui.add_space(t.space_6);
        ui.horizontal(|ui| {
            let arrow = if details { "\u{25B4}" } else { "\u{25BE}" };
            if ui.button(format!("{} {arrow}", tr("updater.details", "Details"))).clicked() {
                details = !details;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add_enabled(!running, egui::Button::new(tr("ui.close", "Close"))).clicked() {
                    close = true;
                }
            });
        });
        if details {
            egui::ScrollArea::vertical().max_height(220.0).stick_to_bottom(true).show(ui, |ui| {
                ui.add(egui::Label::new(RichText::new(p.log.join("\n")).monospace()).wrap());
            });
        }
    });
    app.extras.updater.details = details;
    // While a step runs the dialog stays: the header's close and Esc do nothing.
    if !running && (close || closed) {
        app.extras.updater.job = None;
        if matches!(p.finished, Some(Ok(_))) {
            app.extras.updater.available = None;
        }
    }
    if let Some(Ok(exe)) = &p.finished
        && !app.extras.updater.restart_sent
    {
        app.extras.updater.restart_sent = true;
        app.extras.updater.available = None;
        if let Ok(mut r) = RESTART.lock() {
            *r = Some(exe.clone());
        }
        // The usual close: unsaved work is saved or asked about first. Cancelling it keeps
        // this window, and the new version starts when DarkBurn is closed.
        if !cfg!(test) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_as_numbers() {
        assert!(updates::is_newer("v0.0.765", "0.0.764"));
        assert!(updates::is_newer("0.1.0", "0.0.999"));
        assert!(updates::is_newer("0.0.1000", "0.0.999"));
        assert!(!updates::is_newer("v0.0.764", "0.0.764"));
        assert!(!updates::is_newer("0.0.763", "0.0.764"));
        assert!(!updates::is_newer("", "0.0.764"));
    }

    const CHANGELOG: &str = "# Changelog\n\n## v0.0.766 (build 9)\n- Fix: a\n- New: b\n- New: c\n\n## v0.0.765 (build 8)\n- New: only one\n\n## v0.0.764 (build 7)\n- Fix: old\n\n## v0.0.763 (build 6)\n- Fix: older\n";

    #[test]
    fn changelog_gives_one_line_per_newer_release() {
        assert_eq!(newer_notes(CHANGELOG, "0.0.764"), ["v0.0.766 \u{2014} Fix: a (+2 more)", "v0.0.765 \u{2014} New: only one"]);
        assert!(newer_notes(CHANGELOG, "0.0.766").is_empty());
        assert_eq!(newer_notes(CHANGELOG, "0.0.765").len(), 1);
        assert!(newer_notes("not a changelog", "0.0.1").is_empty());
        // A release heading without entries still gets its line.
        assert_eq!(newer_notes("## v1.0.0 (build 1)\n", "0.0.1"), ["v1.0.0"]);
    }

    #[test]
    fn asset_names_match_the_release_workflow() {
        assert_eq!(asset_name("v0.0.765", "windows", "x86_64").unwrap(), "darkburn-0.0.765-windows-x64-portable.zip");
        assert_eq!(asset_name("0.0.765", "linux", "x86_64").unwrap(), "darkburn-0.0.765-linux-x86_64.tar.gz");
        assert_eq!(asset_name("0.0.765", "linux", "aarch64").unwrap(), "darkburn-0.0.765-linux-aarch64.tar.gz");
        assert_eq!(asset_name("0.0.765", "macos", "aarch64").unwrap(), "darkburn-0.0.765-macos-arm64.tar.gz");
        assert_eq!(asset_name("0.0.765", "macos", "x86_64").unwrap(), "darkburn-0.0.765-macos-x86_64.tar.gz");
        assert_eq!(asset_name("0.0.765", "windows", "aarch64"), None);
        assert_eq!(asset_name("0.0.765", "freebsd", "x86_64"), None);
    }

    #[test]
    fn the_asset_for_this_platform_is_picked_and_others_are_not() {
        let json: serde_json::Value = serde_json::json!({
            "tag_name": "v0.0.765",
            "assets": [
                {"name": "darkburn-0.0.765-windows-x64.msi", "browser_download_url": "https://x/msi"},
                {"name": "darkburn-0.0.765-windows-x64-portable.zip", "browser_download_url": "https://x/zip"},
                {"name": "darkburn-0.0.765-windows-x64-portable.zip.sha256", "browser_download_url": "https://x/sha"},
                {"name": "darkburn-0.0.765-linux-x86_64.tar.gz", "browser_download_url": "http://x/plain"},
                {"name": "darkburn-0.0.765-macos-arm64.tar.gz", "browser_download_url": "https://x/mac"}
            ]
        });
        assert_eq!(pick_asset(&json, "0.0.765", "windows", "x86_64").unwrap().url, "https://x/zip");
        assert_eq!(pick_asset(&json, "0.0.765", "macos", "aarch64").unwrap().url, "https://x/mac");
        assert_eq!(pick_asset(&json, "0.0.765", "linux", "x86_64"), None, "plain http is refused");
        assert_eq!(pick_asset(&json, "0.0.765", "linux", "aarch64"), None, "no archive for it");
        assert_eq!(pick_asset(&serde_json::json!({}), "0.0.765", "linux", "x86_64"), None);
    }

    #[test]
    fn replacing_moves_the_old_file_aside_first() {
        let dir = std::env::temp_dir().join(format!("darkburn-updater-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("darkburn");
        let new = dir.join("new");
        std::fs::write(&target, b"old").unwrap();
        std::fs::write(&new, b"new").unwrap();
        replace_file(&target, &new).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"new");
        assert_eq!(std::fs::read(dir.join("darkburn.old")).unwrap(), b"old");
        // A new file that cannot be read leaves the old one where it was.
        assert!(replace_file(&target, &dir.join("missing")).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"new");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_release_without_an_archive_for_this_platform_opens_its_page() {
        let mut app = DarkBurn::new(crate::prefs::Prefs::default());
        app.extras.updater.available = Some(Release {
            version: "9.0.0".into(),
            page: "https://github.com/phobicdotno/DarkBurn/releases/tag/v9.0.0".into(),
            asset: None,
            notes: vec![],
        });
        crate::help::take_opened();
        link_clicked(&mut app);
        assert_eq!(crate::help::take_opened(), ["https://github.com/phobicdotno/DarkBurn/releases/tag/v9.0.0"]);
        assert!(!app.extras.updater.ask_open);
        app.extras.updater.available.as_mut().unwrap().asset = Some(Asset { name: "a".into(), url: "https://x/a".into() });
        link_clicked(&mut app);
        assert!(app.extras.updater.ask_open, "with an archive it asks first");
    }

    #[cfg(unix)]
    #[test]
    fn a_folder_without_write_permission_is_reported_as_such() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("darkburn-updater-ro-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("ro")).unwrap();
        let target = dir.join("ro").join("darkburn");
        let new = dir.join("new");
        std::fs::write(&target, b"old").unwrap();
        std::fs::write(&new, b"new").unwrap();
        std::fs::set_permissions(dir.join("ro"), std::fs::Permissions::from_mode(0o555)).unwrap();
        let r = replace_checked(&target, &new);
        std::fs::set_permissions(dir.join("ro"), std::fs::Permissions::from_mode(0o755)).unwrap();
        // Root may write anyway; then there is nothing to check.
        if let Err((_, no_permission)) = r {
            assert!(no_permission);
            assert_eq!(std::fs::read(&target).unwrap(), b"old");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_permission_opens_the_release_page_once() {
        let mut app = DarkBurn::new(crate::prefs::Prefs::default());
        let page = "https://github.com/phobicdotno/DarkBurn/releases/tag/v9.0.0";
        let progress = Progress { finished: Some(Err("cannot move aside".into())), no_permission: true, ..Default::default() };
        app.extras.updater.job = Some(Arc::new(Mutex::new(progress)));
        crate::help::take_opened();
        open_page_if_denied(&mut app, page);
        open_page_if_denied(&mut app, page);
        assert_eq!(crate::help::take_opened(), [page]);
    }

    #[test]
    fn the_setting_is_on_by_default() {
        assert!(crate::prefs::Prefs::default().opts.check_updates);
    }
}
