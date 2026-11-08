// Adapted from SolidCraft apps/solidcraft/src/startup.rs (MIT OR Apache-2.0, see ATTRIBUTION.md):
// the settings check and the failed-start counter.
use std::path::{Path, PathBuf};

/// The environment variable that starts safe mode (any value but `0` or empty).
pub const SAFE_MODE_ENV: &str = "DARKBURN_SAFE_MODE";
/// Consecutive launches that died before the window was up, after which safe mode starts.
pub const FAILED_STARTS_FOR_SAFE_MODE: u32 = 2;
const MARKER: &str = "starting";

/// Whether an environment value switches something on.
pub fn env_on(v: Option<&str>) -> bool {
    v.map(str::trim).is_some_and(|v| !v.is_empty() && v != "0" && !v.eq_ignore_ascii_case("false"))
}

fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Move `path` aside as `<name>.bad-<time>` (kept for a bug report); returns where it went.
pub fn back_up(path: &Path) -> std::io::Result<PathBuf> {
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "settings".into());
    let mut to = path.with_file_name(format!("{name}.bad-{}", unix_now()));
    let mut n = 1;
    while to.exists() && n < 100 {
        to = path.with_file_name(format!("{name}.bad-{}-{n}", unix_now()));
        n += 1;
    }
    std::fs::rename(path, &to)?;
    Ok(to)
}

/// Whether eframe's `app.ron` reads, with the preferences inside it.
fn app_ron_reads(text: &str, prefs_keys: &[&str]) -> bool {
    let Ok(kv) = ron::from_str::<std::collections::HashMap<String, String>>(text) else {
        return false;
    };
    prefs_keys.iter().filter_map(|k| kv.get(*k)).all(|p| serde_json::from_str::<serde_json::Value>(p).is_ok_and(|v| v.is_object()))
}

/// Check the settings files in `dir` before anything reads them: one that doesn't read is moved
/// aside (the app then starts with defaults for it). Returns a line per file moved.
pub fn check_settings(dir: &Path, prefs_keys: &[&str]) -> Vec<String> {
    let mut notes = Vec::new();
    let mut check = |name: &str, reads: &dyn Fn(&str) -> bool| {
        let path = dir.join(name);
        let Ok(bytes) = std::fs::read(&path) else {
            return;
        };
        let ok = std::str::from_utf8(&bytes).is_ok_and(|t| t.trim().is_empty() || reads(t));
        if ok {
            return;
        }
        match back_up(&path) {
            Ok(to) => {
                log::warn!("settings: {} did not read; moved to {}", path.display(), to.display());
                notes.push(format!("{name} could not be read and was moved to {}; defaults are used.", to.display()));
            }
            Err(e) => log::warn!("settings: {} did not read and could not be moved: {e}", path.display()),
        }
    };
    check("app.ron", &|t| app_ron_reads(t, prefs_keys));
    notes
}

/// A launch begins: count it in the marker in `dir`. Returns how many launches before this one
/// died before their window was up.
pub fn begin(dir: &Path) -> u32 {
    let marker = dir.join(MARKER);
    let before = std::fs::read_to_string(&marker).ok().and_then(|s| s.trim().parse::<u32>().ok()).unwrap_or(0).min(1000);
    if std::fs::create_dir_all(dir).is_ok() {
        let _ = std::fs::write(&marker, (before + 1).to_string());
    }
    before
}

/// The window is up: this launch did not fail to start.
pub fn started(dir: &Path) {
    let _ = std::fs::remove_file(dir.join(MARKER));
}

/// The file whose presence next to the program makes it portable (the zip build has one).
pub const PORTABLE_MARKER: &str = "portable.txt";

/// The settings folder of a portable copy: `settings` beside the program, when `exe_dir` holds
/// the marker file. `None` for an installed copy (settings then live in the user's profile).
pub fn portable_dir(exe_dir: &Path) -> Option<PathBuf> {
    exe_dir.join(PORTABLE_MARKER).is_file().then(|| exe_dir.join("settings"))
}

#[cfg(test)]
mod portable_tests {
    use super::*;

    #[test]
    fn marker_beside_the_program_moves_settings_beside_it() {
        let dir = std::env::temp_dir().join(format!("darkburn-portable-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(portable_dir(&dir), None, "no marker: settings stay in the profile");
        std::fs::create_dir_all(dir.join(PORTABLE_MARKER)).unwrap();
        assert_eq!(portable_dir(&dir), None, "a folder with that name is not the marker");
        std::fs::remove_dir(dir.join(PORTABLE_MARKER)).unwrap();
        std::fs::write(dir.join(PORTABLE_MARKER), "portable").unwrap();
        assert_eq!(portable_dir(&dir), Some(dir.join("settings")));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
