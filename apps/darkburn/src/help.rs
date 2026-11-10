//! Help: the short in-app topics (F1), the debug log, and the support data bundle. Everything
//! here works offline; nothing is sent anywhere.

use std::collections::VecDeque;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::i18n::tr;

/// Where the documentation lives.
pub const DOCS_URL: &str = "https://github.com/phobicdotno/DarkBurn";

/// A help topic: title and paragraphs, in the active language.
pub struct Topic {
    pub id: &'static str,
    pub title: String,
    pub paragraphs: Vec<String>,
}

/// The ids of all topics.
pub const TOPICS: &[&str] = &[
    "general",
    "canvas",
    "cuts",
    "console",
    "camera",
    "camera_selection",
    "variable_text",
    "shape_properties",
    "laser",
    "gweike",
    "library",
    "move",
    "art_library",
    "file_list",
    "settings",
    "hotkeys",
    "cut_settings",
];

/// The topic for a panel id (or dialog name); the general one when there is none.
pub fn topic_id_for(panel: &str) -> &'static str {
    TOPICS.iter().copied().find(|t| *t == panel).unwrap_or("general")
}

/// The text of topic `id`.
pub fn topic(id: &str) -> Topic {
    let id = topic_id_for(id);
    let (title, paragraphs): (String, Vec<String>) = match id {
        "canvas" => (
            tr("help.canvas.title", "The canvas"),
            vec![
                tr(
                    "help.canvas.p1",
                    "Click a shape to select it. Shift adds to the selection, Ctrl and Shift removes. Drag a box from left to right to select what it encloses, from right to left to select what it touches.",
                ),
                tr(
                    "help.canvas.p2",
                    "Scroll to zoom, hold the middle button or Space to pan. Arrow keys nudge the selection by the distances set in Settings.",
                ),
            ],
        ),
        "cuts" => (
            tr("help.cuts.title", "Cuts / Layers"),
            vec![
                tr(
                    "help.cuts.p1",
                    "Each layer is a colour with its own cut settings. Shapes take the settings of the layer they are on. Click a layer to make it current, double-click it to open all of its settings.",
                ),
                tr(
                    "help.cuts.p2",
                    "Drag layers to change the order they are output in. The tick boxes switch output, visibility and air assist for a layer.",
                ),
            ],
        ),
        "console" => (
            tr("help.console.title", "Console"),
            vec![tr(
                "help.console.p1",
                "The console shows what the machine says back and lets you type a command to send. It becomes active when a machine is connected.",
            )],
        ),
        "camera" => (
            tr("help.camera.title", "Camera Control"),
            vec![tr(
                "help.camera.p1",
                "Place a live or captured picture of the bed behind your design so artwork can be lined up with the material. It becomes active when a camera is set up.",
            )],
        ),
        "camera_selection" => (
            tr("help.camera_selection.title", "Choosing a camera"),
            vec![
                tr(
                    "help.camera_selection.p1",
                    "The camera is chosen in the Camera Control panel (Window menu). Tick Allow live camera, press Find cameras, pick yours in the Camera list and press Start. DarkBurn never opens a camera before that.",
                ),
                tr(
                    "help.camera_selection.p2",
                    "Not in the list? Check the cable and press Find cameras again. Close other programs that use the camera, such as video calls or the system's camera app: only one program can use a camera at a time. On Windows, also look under Settings, Privacy and security, Camera: desktop apps must be allowed to use it.",
                ),
                tr(
                    "help.camera_selection.p3",
                    "Wrong picture? A laptop's own webcam is usually first in the list and the laser's camera comes after it. Pick the other one and press Start again.",
                ),
                tr(
                    "help.camera_selection.p4",
                    "A black, slow or striped picture often means the camera cannot deliver the size asked for. Lower the width and height under Camera capture in Settings, and plug the camera straight into the computer instead of a hub.",
                ),
                tr(
                    "help.camera_selection.p5",
                    "No camera? The panel also takes a picture file of the bed, and everything except the live view works with it.",
                ),
            ],
        ),
        "variable_text" => (
            tr("help.variable_text.title", "Variable Text"),
            vec![tr(
                "help.variable_text.p1",
                "Run a job many times with text that changes each time: a serial number counting up, or lines read from a spreadsheet file.",
            )],
        ),
        "shape_properties" => (
            tr("help.shape_properties.title", "Shape Properties"),
            vec![tr(
                "help.shape_properties.p1",
                "Shows and changes everything about the selected shape: its name, layer, position, size, and for text the font and spacing.",
            )],
        ),
        "laser" => (
            tr("help.laser.title", "Laser"),
            vec![
                tr(
                    "help.laser.p1",
                    "Start, pause and stop a job, frame it, or save it as a file. Start from chooses where the job is placed: at the page's own coordinates, at the laser's present position, or at a user origin.",
                ),
                tr("help.laser.p2", "Nothing here talks to a machine until you connect one."),
            ],
        ),
        "gweike" => (
            tr("help.gweike.title", "Gweike M3 Ultra"),
            vec![
                tr(
                    "help.gweike.p1",
                    "DarkBurn writes the files the Gweike software opens: DXF with a layer per material, G-code (.nc) and the .chf craft file. Map each layer to a material and a craft in the Gweike window.",
                ),
                tr(
                    "help.gweike.p2",
                    "The card simulator plans the job and shows the time estimate. Nothing is sent to the machine. Direct control waits for a read-only capture on the real card (see docs/research/mlaser-rea-findings.md). The machine's own height follower, piercing and gas logic always stay in charge.",
                ),
            ],
        ),
        "library" => (
            tr("help.library.title", "Material library"),
            vec![
                tr(
                    "help.library.p1",
                    "A library keeps cut settings that worked, sorted by material, thickness and description. Assign an entry to put its settings on the current layer and link them, or create an entry from the layer you tuned.",
                ),
                tr(
                    "help.library.p2",
                    "A linked layer shows which entry it follows. Deleting an entry unlinks its layers. Libraries are saved as files, one can be chosen for each machine.",
                ),
            ],
        ),
        "move" => (
            tr("help.move.title", "Move"),
            vec![tr(
                "help.move.p1",
                "Jog the laser head by a set distance, go home, or set the origin. It becomes active when a machine is connected.",
            )],
        ),
        "art_library" => (
            tr("help.art_library.title", "Art Library"),
            vec![
                tr(
                    "help.art_library.p1",
                    "Collections of ready-made artwork in folders. Hover a thumbnail to see it larger, click it to put it on the page. Add your selection to the chosen folder, or drop files onto the panel.",
                ),
                tr("help.art_library.p2", "Right-click an item to rename it, replace it with the selection, or delete it."),
            ],
        ),
        "file_list" => (
            tr("help.file_list.title", "File List"),
            vec![tr(
                "help.file_list.p1",
                "Lists the jobs stored in the controller of machines that keep files, so they can be run, uploaded or deleted.",
            )],
        ),
        "settings" => (
            tr("help.settings.title", "Settings"),
            vec![
                tr(
                    "help.settings.p1",
                    "Settings are grouped by topic. They are saved as you change them. Units, speed units, snapping, nudge distances and the interface scale apply at once.",
                ),
                tr("help.settings.p2", "A shared defaults file lets a workshop give every computer the same starting settings."),
            ],
        ),
        "hotkeys" => (
            tr("help.hotkeys.title", "Hotkeys"),
            vec![
                tr(
                    "help.hotkeys.p1",
                    "Choose a command, press Set, then press the key combination you want. If another command already uses it you can take the key over.",
                ),
                tr("help.hotkeys.p2", "Hotkey sets can be exported to a file and imported on another computer."),
            ],
        ),
        "cut_settings" => (
            tr("help.cut_settings.title", "Cut Settings"),
            vec![tr(
                "help.cut_settings.p1",
                "All the settings of one layer: mode, speed, power, passes and more. Beginner mode in Settings hides the advanced tab.",
            )],
        ),
        _ => (
            tr("help.general.title", "Getting started"),
            vec![
                tr("help.general.p1", "Draw or import shapes, give each colour its cut settings in Cuts / Layers, then send the job to the laser."),
                tr(
                    "help.general.p2",
                    "Press F1 with the pointer over a panel or a dialog to read about it. Settings and Edit Hotkeys are in the application bar and the File menu.",
                ),
            ],
        ),
    };
    Topic { id, title, paragraphs }
}

// ----- The debug log -----

static DEBUG_ON: AtomicBool = AtomicBool::new(false);
static LINES: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());
static FILE: Mutex<Option<PathBuf>> = Mutex::new(None);
const KEEP_LINES: usize = 1000;

struct Logger;

impl log::Log for Logger {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= log::Level::Info || DEBUG_ON.load(Ordering::Relaxed)
    }

    fn log(&self, r: &log::Record) {
        if !self.enabled(r.metadata()) {
            return;
        }
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let line = format!("{secs} {:5} {}: {}", r.level(), r.target(), r.args());
        if let Ok(mut l) = LINES.lock() {
            if l.len() >= KEEP_LINES {
                l.pop_front();
            }
            l.push_back(line.clone());
        }
        if DEBUG_ON.load(Ordering::Relaxed)
            && let Ok(f) = FILE.lock()
            && let Some(path) = f.as_ref()
            && let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path)
        {
            let _ = writeln!(file, "{line}");
        }
    }

    fn flush(&self) {}
}

/// Start collecting log lines (a ring of the most recent ones, for the support bundle).
pub fn install_logger() {
    if log::set_logger(&Logger).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
}

/// Switch the debug log on or off. On: everything down to debug level is also appended to
/// `debug.log` in `dir`.
pub fn set_debug(on: bool, dir: Option<&Path>) {
    if let Ok(mut f) = FILE.lock() {
        *f = dir.map(|d| d.join("debug.log"));
    }
    DEBUG_ON.store(on, Ordering::Relaxed);
    log::set_max_level(if on { log::LevelFilter::Debug } else { log::LevelFilter::Info });
    if on && let Some(d) = dir {
        let _ = std::fs::create_dir_all(d);
    }
    log::info!("debug log {}", if on { "on" } else { "off" });
}

pub fn debug_on() -> bool {
    DEBUG_ON.load(Ordering::Relaxed)
}

/// The most recent log lines.
pub fn recent_log() -> String {
    LINES.lock().map(|l| l.iter().cloned().collect::<Vec<_>>().join("\n")).unwrap_or_default()
}

// ----- The support data bundle -----

fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (i, slot) in table.iter_mut().enumerate() {
        let mut c = i as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
        *slot = c;
    }
    !data.iter().fold(!0u32, |c, b| table[((c ^ *b as u32) & 0xFF) as usize] ^ (c >> 8))
}

/// A zip file (entries stored, not compressed) holding `files`.
pub fn zip(files: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in files {
        let offset = out.len() as u32;
        let crc = crc32(data);
        let (n, size) = (name.as_bytes(), data.len() as u32);
        out.extend_from_slice(&[0x50, 0x4b, 0x03, 0x04, 20, 0, 0, 0, 0, 0, 0, 0, 0x21, 0]);
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&(n.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(n);
        out.extend_from_slice(data);
        central.extend_from_slice(&[0x50, 0x4b, 0x01, 0x02, 20, 0, 20, 0, 0, 0, 0, 0, 0, 0, 0x21, 0]);
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&(n.len() as u16).to_le_bytes());
        central.extend_from_slice(&[0; 12]);
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(n);
    }
    let cd_offset = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&[0x50, 0x4b, 0x05, 0x06, 0, 0, 0, 0]);
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

/// The files of the support bundle: version, preferences, recent log lines, and the logs and
/// notes found in the settings folder. Nothing is read from the network.
pub fn support_files(prefs_json: &str, settings_dir: Option<&Path>) -> Vec<(String, Vec<u8>)> {
    let version = format!(
        "DarkBurn {}\nos {}\narch {}\ndebug log {}\n",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        if debug_on() { "on" } else { "off" }
    );
    let mut files = vec![
        ("version.txt".to_string(), version.into_bytes()),
        ("preferences.json".to_string(), prefs_json.as_bytes().to_vec()),
        ("recent-log.txt".to_string(), recent_log().into_bytes()),
    ];
    if let Some(dir) = settings_dir {
        for name in ["debug.log", "darkburn.log"] {
            if let Ok(data) = std::fs::read(dir.join(name)) {
                // The tail is what matters; keep bundles small.
                let keep = data.len().saturating_sub(512 * 1024);
                files.push((name.to_string(), data[keep..].to_vec()));
            }
        }
    }
    files
}

/// Write the support bundle to `path`.
pub fn write_support_zip(path: &Path, prefs_json: &str, settings_dir: Option<&Path>) -> Result<(), String> {
    std::fs::write(path, zip(&support_files(prefs_json, settings_dir))).map_err(|e| e.to_string())
}

#[cfg(test)]
thread_local! {
    static OPENED: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// The addresses [`open_url`] was asked to open on this thread (tests only: no browser starts).
#[cfg(test)]
pub fn take_opened() -> Vec<String> {
    OPENED.with(|o| std::mem::take(&mut *o.borrow_mut()))
}

/// Open `url` in the system browser. No other network access happens.
pub fn open_url(url: &str) {
    #[cfg(test)]
    {
        OPENED.with(|o| o.borrow_mut().push(url.to_string()));
    }
    if cfg!(test) {
        return; // UI tests never start a browser
    }
    #[cfg(target_os = "windows")]
    let r = std::process::Command::new("cmd").args(["/C", "start", "", url]).spawn();
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("open").arg(url).spawn();
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let r = std::process::Command::new("xdg-open").arg(url).spawn();
    if let Err(e) = r {
        log::warn!("could not open {url}: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads the entry names and contents of a stored zip.
    fn read_zip(z: &[u8]) -> Vec<(String, Vec<u8>)> {
        let mut out = Vec::new();
        let mut p = 0;
        while z[p..].starts_with(&[0x50, 0x4b, 0x03, 0x04]) {
            let crc = u32::from_le_bytes(z[p + 14..p + 18].try_into().unwrap());
            let size = u32::from_le_bytes(z[p + 18..p + 22].try_into().unwrap()) as usize;
            let n = u16::from_le_bytes(z[p + 26..p + 28].try_into().unwrap()) as usize;
            let name = String::from_utf8(z[p + 30..p + 30 + n].to_vec()).unwrap();
            let data = z[p + 30 + n..p + 30 + n + size].to_vec();
            assert_eq!(crc32(&data), crc, "{name}");
            out.push((name, data));
            p += 30 + n + size;
        }
        out
    }

    #[test]
    fn crc_matches_the_known_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn support_bundle_holds_version_prefs_and_logs() {
        let dir = std::env::temp_dir().join(format!("darkburn-support-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("debug.log"), "line one\n").unwrap();
        let path = dir.join("support.zip");
        write_support_zip(&path, "{\"theme\":\"dark\"}", Some(&dir)).unwrap();
        let entries = read_zip(&std::fs::read(&path).unwrap());
        let names: Vec<_> = entries.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["version.txt", "preferences.json", "recent-log.txt", "debug.log"]);
        assert!(String::from_utf8_lossy(&entries[0].1).contains(env!("CARGO_PKG_VERSION")));
        assert_eq!(entries[1].1, b"{\"theme\":\"dark\"}");
        let z = std::fs::read(&path).unwrap();
        assert!(z.windows(4).any(|w| w == [0x50, 0x4b, 0x05, 0x06]), "has an end record");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn every_panel_and_dialog_has_a_topic_and_unknown_falls_back() {
        for p in crate::panels::PANELS {
            assert_eq!(topic_id_for(p.id), p.id, "panel {} has no help topic", p.id);
        }
        for id in TOPICS {
            let t = topic(id);
            assert!(!t.title.is_empty() && !t.paragraphs.is_empty(), "{id}");
        }
        assert_eq!(topic("no_such_thing").id, "general");
    }
}
