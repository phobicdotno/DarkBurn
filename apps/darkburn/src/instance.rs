//! One running copy (PLAT-01, start): with the "single instance" preference on, starting DarkBurn
//! again with files hands them to the copy already running and exits.
//!
//! The running copy writes a heartbeat file into the settings folder every second or so. A new
//! copy that finds a fresh heartbeat drops the paths it was given into the `inbox` folder, one
//! small text file per launch, and quits; the running copy reads and removes them. No sockets,
//! no pipes, so nothing here needs a permission or a firewall rule.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const HEARTBEAT: &str = "instance.alive";
const INBOX: &str = "inbox";
/// A heartbeat older than this means the copy is gone.
pub const STALE: Duration = Duration::from_secs(5);
/// How often the running copy refreshes the heartbeat and looks in the inbox.
pub const TICK: Duration = Duration::from_secs(1);

fn now_ms() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis())
}

/// The running copy is alive: refresh the heartbeat.
pub fn heartbeat(dir: &Path) {
    let _ = std::fs::create_dir_all(dir);
    let _ = std::fs::write(dir.join(HEARTBEAT), now_ms().to_string());
}

/// The running copy is closing.
pub fn clear(dir: &Path) {
    let _ = std::fs::remove_file(dir.join(HEARTBEAT));
}

/// Whether another copy is running (its heartbeat is fresh).
pub fn running(dir: &Path) -> bool {
    running_at(dir, now_ms())
}

fn running_at(dir: &Path, now: u128) -> bool {
    let Ok(text) = std::fs::read_to_string(dir.join(HEARTBEAT)) else { return false };
    let Ok(then) = text.trim().parse::<u128>() else { return false };
    now.saturating_sub(then) < STALE.as_millis()
}

/// Hand `paths` to the running copy. An empty list asks it for a new empty window.
pub fn forward(dir: &Path, paths: &[PathBuf]) -> std::io::Result<()> {
    let inbox = dir.join(INBOX);
    std::fs::create_dir_all(&inbox)?;
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let name = format!("{:016}-{}-{n:06}.txt", now_ms(), std::process::id());
    let tmp = inbox.join(format!("{name}.tmp"));
    let text: String =
        paths.iter().map(|p| std::path::absolute(p).unwrap_or_else(|_| p.clone()).to_string_lossy().into_owned()).collect::<Vec<_>>().join("\n");
    std::fs::write(&tmp, text)?;
    std::fs::rename(tmp, inbox.join(name))
}

/// What other launches handed over, oldest first; each entry is the paths of one launch. The
/// files are removed.
pub fn take_inbox(dir: &Path) -> Vec<Vec<PathBuf>> {
    let inbox = dir.join(INBOX);
    let Ok(read) = std::fs::read_dir(&inbox) else { return Vec::new() };
    let mut files: Vec<PathBuf> = read.filter_map(Result::ok).map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "txt")).collect();
    files.sort();
    files
        .into_iter()
        .filter_map(|f| {
            let text = std::fs::read_to_string(&f).ok();
            let _ = std::fs::remove_file(&f);
            text.map(|t| t.lines().map(str::trim).filter(|l| !l.is_empty()).map(PathBuf::from).collect())
        })
        .collect()
}

/// The single-instance preference, read from the saved settings before the window exists.
pub fn wanted(dir: &Path, prefs_key: &str) -> bool {
    let Ok(text) = std::fs::read_to_string(dir.join("app.ron")) else { return false };
    let Ok(kv) = ron::from_str::<std::collections::HashMap<String, String>>(&text) else { return false };
    kv.get(prefs_key).map(|p| crate::prefs::Prefs::from_json(p).files.single_instance).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("darkburn-instance-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_fresh_heartbeat_means_a_copy_is_running() {
        let d = temp("beat");
        assert!(!running(&d));
        heartbeat(&d);
        assert!(running(&d));
        assert!(!running_at(&d, now_ms() + 10_000), "an old heartbeat is stale");
        clear(&d);
        assert!(!running(&d));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn paths_pass_through_the_inbox_once() {
        let d = temp("inbox");
        forward(&d, &[PathBuf::from("a.dxf"), PathBuf::from("b.svg")]).unwrap();
        forward(&d, &[]).unwrap();
        let got = take_inbox(&d);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].len(), 2);
        assert!(got[0][0].ends_with("a.dxf"));
        assert!(got[1].is_empty(), "an empty launch asks for a new window");
        assert!(take_inbox(&d).is_empty(), "taken files are gone");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_preference_is_read_from_saved_settings() {
        let d = temp("pref");
        assert!(!wanted(&d, "darkburn.prefs"));
        let mut p = crate::prefs::Prefs::default();
        p.files.single_instance = true;
        let kv: std::collections::HashMap<String, String> = [("darkburn.prefs".to_string(), p.to_json())].into();
        std::fs::write(d.join("app.ron"), ron::to_string(&kv).unwrap()).unwrap();
        assert!(wanted(&d, "darkburn.prefs"));
        let _ = std::fs::remove_dir_all(&d);
    }
}
