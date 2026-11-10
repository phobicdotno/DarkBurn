//! Preference files: export, import, automatic backups and the backup browser
//! (FILE-16..18, AUX-04, PLAT-07), plus the names of the other auxiliary files.
//!
//! A preferences file is JSON: `{"format":"darkburn-preferences","version":1,"prefs":{...}}`. A
//! bare preferences object (what is stored inside the settings folder) is accepted too. The list
//! of recent projects is machine-specific and is never taken from a file. Importing first writes
//! a backup of the current settings, so it can always be undone from the backup browser.
//!
//! Automatic backups live in `backups/` inside the settings folder, one file per start in which
//! the settings changed; the newest [`KEEP`] are kept.

use std::path::{Path, PathBuf};

use crate::prefs::Prefs;

pub const FORMAT: &str = "darkburn-preferences";
pub const VERSION: u32 = 1;
pub const BACKUP_DIR: &str = "backups";
/// Backups kept.
pub const KEEP: usize = 20;
/// Where the hotkey table will be stored (AUX-08, written by the hotkeys editor when it lands).
#[allow(dead_code, reason = "named for the hotkeys editor, which reads and writes it")]
pub const HOTKEYS_FILE: &str = "hotkeys.json";
/// Device profile files (AUX-03) are read by the devices code; this is their extension.
pub const DEVICE_PROFILE_EXT: &str = "dbdev";

/// The preferences as a file's text.
pub fn to_file_text(prefs: &Prefs) -> String {
    let value = serde_json::from_str::<serde_json::Value>(&prefs.to_json()).unwrap_or(serde_json::Value::Null);
    serde_json::to_string_pretty(&serde_json::json!({ "format": FORMAT, "version": VERSION, "prefs": value })).unwrap_or_default()
}

/// Reads preferences from a file's text. Anything that is not a preferences object is refused
/// (unlike [`Prefs::from_json`], which falls back to the defaults).
pub fn from_file_text(text: &str) -> Result<Prefs, String> {
    let value: serde_json::Value = serde_json::from_str(text).map_err(|_| "this is not a preferences file".to_string())?;
    let obj = match value.get("format").and_then(|f| f.as_str()) {
        Some(FORMAT) => value.get("prefs").cloned().unwrap_or_default(),
        Some(_) => return Err("this is not a DarkBurn preferences file".into()),
        None => value,
    };
    if !obj.is_object() {
        return Err("this is not a preferences file".into());
    }
    Ok(Prefs::from_json(&obj.to_string()))
}

/// The settings to write when another DarkBurn window may have written its own since this one
/// last read or wrote them (`base`). `ours` is what this window has now, `theirs` what is
/// stored. A setting this window changed is ours; one it left alone takes the stored value, so
/// two windows do not undo each other's changes. Text that is not preferences gives `ours`.
pub fn merge_stored(base: &str, ours: &str, theirs: &str) -> String {
    use serde_json::Value;
    fn merge(base: Option<&Value>, ours: &Value, theirs: Option<&Value>) -> Value {
        let Some(theirs) = theirs else { return ours.clone() };
        if base == Some(ours) {
            return theirs.clone();
        }
        match (ours, theirs) {
            (Value::Object(o), Value::Object(t)) => {
                let mut out = serde_json::Map::new();
                for (key, value) in o {
                    out.insert(key.clone(), merge(base.and_then(|b| b.get(key)), value, t.get(key)));
                }
                // What only the other window has (a panel, a key) and this one never had.
                for (key, value) in t {
                    if !o.contains_key(key) && base.and_then(|b| b.get(key)).is_none() {
                        out.insert(key.clone(), value.clone());
                    }
                }
                Value::Object(out)
            }
            _ => ours.clone(),
        }
    }
    let parse = |t: &str| serde_json::from_str::<Value>(t).ok().filter(Value::is_object);
    let (Some(b), Some(o), Some(t)) = (parse(base), parse(ours), parse(theirs)) else { return ours.to_string() };
    Prefs::from_json(&merge(Some(&b), &o, Some(&t)).to_string()).to_json()
}

/// Writes the preferences to `path`.
pub fn export(prefs: &Prefs, path: &Path) -> Result<(), String> {
    std::fs::write(path, to_file_text(prefs)).map_err(|e| e.to_string())
}

/// Replaces `current` with the preferences in `path`, keeping this machine's recent projects
/// and its AI agent settings.
/// The old settings are backed up first when a settings folder is given.
pub fn import(current: &mut Prefs, path: &Path, settings_dir: Option<&Path>) -> Result<(), String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let mut new = from_file_text(&text)?;
    if let Some(dir) = settings_dir {
        write_backup(dir, current, true);
    }
    new.recent = std::mem::take(&mut current.recent);
    // What AI agents may do is decided on this computer, in Settings: a file never changes it.
    new.mcp = current.mcp.clone();
    *current = new;
    Ok(())
}

pub fn backups_dir(settings_dir: &Path) -> PathBuf {
    settings_dir.join(BACKUP_DIR)
}

/// A backup that exists: its file and when it was written (seconds since 1970).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Backup {
    pub path: PathBuf,
    pub time: u64,
}

fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// The backups, newest first.
pub fn list_backups(settings_dir: &Path) -> Vec<Backup> {
    let Ok(read) = std::fs::read_dir(backups_dir(settings_dir)) else { return Vec::new() };
    let mut all: Vec<Backup> = read
        .filter_map(Result::ok)
        .filter_map(|e| {
            let path = e.path();
            let stem = path.file_stem()?.to_str()?;
            let time = stem.strip_prefix("prefs-")?.split('-').next()?.parse::<u64>().ok()?;
            (path.extension()? == "json").then_some(Backup { path, time })
        })
        .collect();
    all.sort_by(|a, b| b.time.cmp(&a.time).then_with(|| b.path.cmp(&a.path)));
    all
}

/// Writes a backup of `prefs`; unless `force`, only when it differs from the newest backup.
/// Old backups beyond [`KEEP`] are removed. Returns the file written.
pub fn write_backup(settings_dir: &Path, prefs: &Prefs, force: bool) -> Option<PathBuf> {
    let text = to_file_text(prefs);
    let existing = list_backups(settings_dir);
    if !force
        && let Some(newest) = existing.first()
        && std::fs::read_to_string(&newest.path).is_ok_and(|t| t == text)
    {
        return None;
    }
    let dir = backups_dir(settings_dir);
    std::fs::create_dir_all(&dir).ok()?;
    let now = unix_now();
    let mut n = 0;
    let path = loop {
        let p = dir.join(format!("prefs-{now}-{n}.json"));
        if !p.exists() {
            break p;
        }
        n += 1;
    };
    std::fs::write(&path, text).ok()?;
    for old in list_backups(settings_dir).into_iter().skip(KEEP) {
        let _ = std::fs::remove_file(old.path);
    }
    Some(path)
}

/// `YYYY-MM-DD HH:MM:SS` in UTC for seconds since 1970.
pub fn format_time(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Days since 1970-01-01 to a calendar date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02} UTC", rem / 3600, rem % 3600 / 60, rem % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("darkburn-prefsfiles-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn export_and_import_round_trip_but_keep_recent_projects() {
        let d = temp("rt");
        let mut a = Prefs { theme: "nord".into(), invert_wheel: true, ..Default::default() };
        a.recent = vec!["x.dbrn".into()];
        a.files.dxf_arcs = false;
        let file = d.join("prefs.json");
        export(&a, &file).unwrap();
        let mut b = Prefs { recent: vec!["mine.dbrn".into()], ..Default::default() };
        import(&mut b, &file, Some(&d.join("settings"))).unwrap();
        assert_eq!(b.theme, "nord");
        assert!(b.invert_wheel && !b.files.dxf_arcs);
        assert_eq!(b.recent, vec!["mine.dbrn".to_string()], "recent projects belong to the machine");
        assert_eq!(list_backups(&d.join("settings")).len(), 1, "the old settings were backed up first");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Two windows: each keeps what it changed and takes over what the other changed.
    #[test]
    fn merging_keeps_both_windows_changes() {
        let base = Prefs::default();
        let mut ours = base.clone();
        ours.theme = "light".into();
        ours.opts.nudge = 2.0;
        ours.panels.get_mut("move").unwrap().visible = true;
        let mut theirs = base.clone();
        theirs.invert_wheel = true;
        theirs.opts.nudge_shift = 50.0;
        theirs.opts.nudge = 7.0;
        theirs.panels.get_mut("cuts").unwrap().visible = false;
        theirs.hotkeys.insert("file.new".into(), "Ctrl+Alt+F9".into());
        let merged = Prefs::from_json(&merge_stored(&base.to_json(), &ours.to_json(), &theirs.to_json()));
        assert_eq!((merged.theme.as_str(), merged.invert_wheel), ("light", true));
        assert_eq!(
            (merged.opts.nudge, merged.opts.nudge_shift),
            (2.0, 50.0),
            "changed by both: this window's; changed by the other only: the other's"
        );
        assert!(merged.panel("move").visible && !merged.panel("cuts").visible);
        assert_eq!(merged.hotkeys.get("file.new").map(String::as_str), Some("Ctrl+Alt+F9"));
        // Nothing stored by another window, or nothing readable: ours.
        assert_eq!(merge_stored(&base.to_json(), &ours.to_json(), &base.to_json()), ours.to_json());
        assert_eq!(merge_stored("a", "b", "c"), "b");
        assert_eq!(merge_stored(&base.to_json(), &ours.to_json(), "{ damaged"), ours.to_json());
    }

    #[test]
    fn importing_never_changes_what_ai_agents_may_do() {
        let d = temp("mcp");
        let mut theirs = Prefs { theme: "nord".into(), ..Default::default() };
        theirs.mcp.enabled = true;
        theirs.mcp.port = 4242;
        theirs.mcp.allow_hardware = true;
        theirs.mcp.allow_gweike = true;
        theirs.mcp.require_confirmation = false;
        let file = d.join("prefs.json");
        export(&theirs, &file).unwrap();
        // A computer where agents are off stays that way.
        let mut mine = Prefs::default();
        import(&mut mine, &file, None).unwrap();
        assert_eq!(mine.theme, "nord", "the rest of the file is taken");
        assert_eq!(mine.mcp, crate::mcp_ui::McpPrefs::default(), "a file from elsewhere switched agent access on");
        // And the owner's own choices are kept, whatever the file says.
        let mut mine = Prefs::default();
        mine.mcp.enabled = true;
        mine.mcp.allow_file_write = false;
        let chosen = mine.mcp.clone();
        import(&mut mine, &file, None).unwrap();
        assert_eq!(mine.mcp, chosen);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn bad_files_are_refused_and_bare_objects_accepted() {
        assert!(from_file_text("nope").is_err());
        assert!(from_file_text("[1,2]").is_err());
        assert!(from_file_text(r#"{"format":"other","prefs":{}}"#).is_err());
        assert_eq!(from_file_text(r#"{"theme":"light"}"#).unwrap().theme, "light");
        let mut p = Prefs::default();
        let err = import(&mut p, Path::new("does-not-exist.json"), None);
        assert!(err.is_err());
    }

    #[test]
    fn backups_are_written_on_change_only_and_pruned() {
        let d = temp("bk");
        let mut p = Prefs::default();
        assert!(write_backup(&d, &p, false).is_some());
        assert!(write_backup(&d, &p, false).is_none(), "unchanged settings are not backed up again");
        p.theme = "light".into();
        assert!(write_backup(&d, &p, false).is_some());
        for _ in 0..KEEP + 5 {
            write_backup(&d, &p, true);
        }
        assert_eq!(list_backups(&d).len(), KEEP);
        let newest = &list_backups(&d)[0];
        let loaded = from_file_text(&std::fs::read_to_string(&newest.path).unwrap()).unwrap();
        assert_eq!(loaded.theme, "light");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn times_are_shown_as_utc_dates() {
        assert_eq!(format_time(0), "1970-01-01 00:00:00 UTC");
        assert_eq!(format_time(1_700_000_000), "2023-11-14 22:13:20 UTC");
    }
}
