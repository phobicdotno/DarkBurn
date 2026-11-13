//! What an AI agent's command may not do to this computer. `McpBackend::execute_inner` asks
//! [`before`] ahead of every agent command and hands the result to [`after`]:
//!
//! * Settings that send files or the network somewhere stay the person's: the agent policy, the
//!   pendant, the update address, the job log folder, the serial ports DarkBurn leaves alone and
//!   the remembered file and folder lists. `settings.set` and `setting.set` refuse them; any other
//!   command that changed one of the first five has it put back (a preferences file an agent
//!   imports keeps all of them).
//! * A save or export never writes into DarkBurn's own settings folder (the installed one, a
//!   portable one, or the folder of the control file), however the path is written.
//! * A command that would open a file dialog without a path is refused: an agent cannot answer
//!   the dialog, and the window would wait for the person.
//!
//! The person's own clicks and the Settings window never come through here.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

use darkburn_mcp::CommandError;
use serde_json::Value;

use crate::mcp_catalog;
use crate::prefs::Prefs;
use crate::shell::DarkBurn;

/// What the person has set that an agent's command must leave as it was.
#[derive(Clone, PartialEq)]
pub struct Kept {
    mcp: crate::mcp_ui::McpPrefs,
    pendant: bool,
    update_url: String,
    job_log_dir: String,
    excluded_ports: Vec<String>,
    // Lists of files and folders: opening a file or a library adds to them as it should, so only
    // `settings.set` and a preferences import are held to them.
    recent: Vec<String>,
    library_paths: BTreeMap<String, String>,
    art_libraries: Vec<String>,
    shx_fonts: Vec<String>,
    export_dir: Option<String>,
    /// A file window (picker, export or print) was already waiting for the person.
    window_waiting: bool,
}

impl Kept {
    pub fn of(app: &DarkBurn) -> Kept {
        let p = &app.prefs;
        Kept {
            mcp: p.mcp.clone(),
            pendant: p.opts.pendant,
            update_url: p.opts.update_url.clone(),
            job_log_dir: p.opts.job_log_dir.clone(),
            excluded_ports: p.opts.excluded_ports.clone(),
            recent: p.recent.clone(),
            library_paths: p.library_paths.clone(),
            art_libraries: p.art_libraries.clone(),
            shx_fonts: p.shx_fonts.clone(),
            export_dir: p.files.export_dir.clone(),
            window_waiting: app.fio.file_window_waiting(),
        }
    }

    fn put_back_settings(&self, p: &mut Prefs) {
        p.mcp = self.mcp.clone();
        p.opts.pendant = self.pendant;
        p.opts.update_url.clone_from(&self.update_url);
        p.opts.job_log_dir.clone_from(&self.job_log_dir);
        p.opts.excluded_ports.clone_from(&self.excluded_ports);
    }

    fn put_back_lists(&self, p: &mut Prefs) {
        p.recent.clone_from(&self.recent);
        p.library_paths.clone_from(&self.library_paths);
        p.art_libraries.clone_from(&self.art_libraries);
        p.shx_fonts.clone_from(&self.shx_fonts);
        p.files.export_dir.clone_from(&self.export_dir);
    }
}

/// Why `key` (as `settings.set` or `setting.set` names it) is the person's alone; `None` when an
/// agent may set it.
pub fn persons_setting(key: &str) -> Option<&'static str> {
    let k = key.trim().trim_start_matches(['.', '/']).replace('/', ".").to_ascii_lowercase();
    let k = k.strip_prefix("opts.").unwrap_or(&k);
    let is = |name: &str| k == name || k.strip_prefix(name).is_some_and(|rest| rest.starts_with('.'));
    if is("mcp") {
        Some("it is one of the AI-agent settings")
    } else if is("pendant") {
        Some("it switches the pendant, a USB device")
    } else if is("update_url") {
        Some("it is the address Check for Updates fetches")
    } else if is("job_log_dir") {
        Some("it is the folder the job log is written to")
    } else if is("excluded_ports") {
        Some("it decides which serial ports DarkBurn opens")
    } else if ["recent", "library_paths", "art_libraries", "shx_fonts", "files.export_dir"].iter().any(|n| is(n)) {
        Some("it holds file or folder paths")
    } else {
        None
    }
}

fn only_the_person(id: &str, key: &str, why: &str) -> CommandError {
    CommandError::denied(format!(
        "`{id}`: `{key}` can only be changed by the user in DarkBurn's Settings window, because {why}; ask the user to change it there"
    ))
}

/// Commands that open a system file dialog when they are run without a path.
const DIALOG_WITHOUT_PATH: &[&str] =
    &["help.support_data", "image.save_bitmap", "image.replace", "edit.image_replace", "edit.image_replace_fit", "file.open", "file.import"];

/// Whether `id`, run by an agent, needs a `path` (or `paths`) argument.
pub fn needs_a_path(id: &str) -> bool {
    DIALOG_WITHOUT_PATH.contains(&id) || mcp_catalog::REDIRECTS.iter().any(|r| r.id == id)
}

fn has_a_path(args: &Value) -> bool {
    let text = |v: &Value| v.as_str().is_some_and(|s| !s.trim().is_empty());
    args.get("path").is_some_and(text) || args.get("paths").and_then(Value::as_array).is_some_and(|a| !a.is_empty() && a.iter().all(text))
}

fn path_required(id: &str) -> CommandError {
    CommandError::invalid(format!(
        "`{id}`: path required: without one this opens a file dialog, which an agent cannot answer; pass the file as the path argument"
    ))
}

/// Checks before an agent's command runs. `Some` is the refusal.
pub fn before(app: &DarkBurn, id: &str, args: &Value) -> Option<CommandError> {
    if needs_a_path(id) && !has_a_path(args) {
        return Some(path_required(id));
    }
    if matches!(id, "settings.set" | "setting.set")
        && let Some(key) = args.get("key").and_then(Value::as_str)
        && let Some(why) = persons_setting(key)
    {
        return Some(only_the_person(id, key, why));
    }
    if mcp_catalog::writes_files(id) {
        let folders: Vec<PathBuf> = protected_folders(app).iter().map(|f| resolve(f)).collect();
        for target in write_targets(app, id, args) {
            let real = resolve(&target);
            if let Some(folder) = folders.iter().find(|f| inside(&real, f, NAME_RULES)) {
                return Some(CommandError::denied(format!(
                    "`{id}`: {} is in DarkBurn's own settings folder ({}); an agent may not write there. Save to another folder.",
                    target.display(),
                    folder.display()
                )));
            }
        }
    }
    None
}

/// After an agent's command: put back what it may not change, and refuse what it should not
/// have done.
pub fn after(app: &mut DarkBurn, id: &str, kept: &Kept, r: Result<Value, CommandError>) -> Result<Value, CommandError> {
    let now = Kept { window_waiting: kept.window_waiting, ..Kept::of(app) };
    let mut r = r;
    if now != *kept {
        kept.put_back_settings(&mut app.prefs);
        if matches!(id, "settings.set" | "setting.set" | "file.prefs_import" | "project.prefs_import") {
            kept.put_back_lists(&mut app.prefs);
        }
        if matches!(id, "settings.set" | "setting.set") && r.is_ok() {
            r = Err(only_the_person(id, "that setting", "it holds a path, an address or the AI-agent settings"));
        }
    }
    // A file window the command asked for would wait for the person.
    if !kept.window_waiting && app.fio.file_window_waiting() {
        app.fio.drop_file_window();
        return Err(path_required(id));
    }
    r
}

/// The files `id` would write: its path arguments, or the file it saves back to.
fn write_targets(app: &DarkBurn, id: &str, args: &Value) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = Vec::new();
    v.extend(args.get("path").and_then(Value::as_str).map(PathBuf::from));
    if let Some(all) = args.get("paths").and_then(Value::as_array) {
        v.extend(all.iter().filter_map(Value::as_str).map(PathBuf::from));
    }
    match id {
        "file.save" => v.extend(app.project_path.clone()),
        "library.save" => v.extend(app.extras.library.path.clone()),
        _ => {}
    }
    v
}

/// DarkBurn's own folders: the settings folder in use, the portable one beside the program
/// (whether or not this copy runs portable), the installed one, and the control file's folder
/// (or the control file itself when it was put somewhere else).
pub fn protected_folders(app: &DarkBurn) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = Vec::new();
    v.extend(app.settings_dir.clone());
    if let Some(exe_dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
        v.push(exe_dir.join("settings"));
    }
    v.extend(eframe::storage_dir("DarkBurn"));
    let control = darkburn_mcp::control::control_file_path();
    if std::env::var_os("DARKBURN_CONTROL_FILE").is_some() {
        v.push(control);
    } else if let Some(dir) = control.parent() {
        v.push(dir.to_path_buf());
    }
    v
}

/// The real place `path` names: made absolute against the working folder, with `.` and `..`
/// worked out and every part that exists followed through links to its real name (and, on
/// Windows, its real letter case and long name). A link whose target is not there yet is followed
/// too, since writing through it makes the target.
pub fn resolve(path: &Path) -> PathBuf {
    resolve_at(path, 0)
}

fn resolve_at(path: &Path, depth: u32) -> PathBuf {
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut out = PathBuf::new();
    for c in abs.components() {
        match c {
            Component::Prefix(_) | Component::RootDir => out.push(c.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(n) => {
                out.push(n);
                if let Ok(real) = std::fs::canonicalize(&out) {
                    out = real;
                } else if depth < 40
                    && let Ok(target) = std::fs::read_link(&out)
                {
                    let from = out.parent().map(Path::to_path_buf).unwrap_or_default();
                    out = resolve_at(&from.join(target), depth + 1);
                }
            }
        }
    }
    out
}

/// How file names compare on this system.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NameRules {
    /// Letter for letter (Linux).
    Exact,
    /// Upper and lower case are the same letter (macOS).
    AnyCase,
    /// Any case, and Windows drops trailing dots and spaces and reads `name:stream` as a stream
    /// of `name`.
    Windows,
}

const NAME_RULES: NameRules = if cfg!(windows) {
    NameRules::Windows
} else if cfg!(target_os = "macos") {
    NameRules::AnyCase
} else {
    NameRules::Exact
};

fn name_key(s: &OsStr, rules: NameRules) -> String {
    let s = s.to_string_lossy();
    match rules {
        NameRules::Exact => s.into_owned(),
        NameRules::AnyCase => s.to_lowercase(),
        NameRules::Windows => {
            // `name:stream` is a stream of `name` (a drive's `c:` is cut the same on both sides).
            let s = s.to_lowercase();
            s.split(':').next().unwrap_or_default().trim_end_matches(['.', ' ']).to_string()
        }
    }
}

/// Whether `target` is `folder` or inside it; both already went through [`resolve`].
pub fn inside(target: &Path, folder: &Path, rules: NameRules) -> bool {
    let mut t = target.components();
    for f in folder.components() {
        match t.next() {
            Some(c) if name_key(c.as_os_str(), rules) == name_key(f.as_os_str(), rules) => {}
            _ => return false,
        }
    }
    true
}

#[cfg(test)]
#[path = "agent_limits_tests.rs"]
mod tests;
