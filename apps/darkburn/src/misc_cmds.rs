//! Small commands around a job that are not machine commands [LASER-25, LASER-26]: where the job
//! log is, opening its folder, and trying a sound. The Settings window asks for them with
//! [`request`]; the shell runs what was asked for after the window is drawn.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::commands::Cmd;
use crate::shell::DarkBurn;
use crate::sounds::Sound;

const fn cmd(id: &'static str, label: &'static str, args: &'static str) -> Cmd {
    Cmd { id, label, key: "", icon: "", args }
}

pub const COMMANDS: &[Cmd] = &[
    cmd(
        "job.log_info",
        "Job Log: Where It Is and What Is in It",
        r#"{"type":"object","properties":{"rows":{"type":"integer","description":"how many of the latest rows to return (default 20)"}}}"#,
    ),
    cmd("job.log_show", "Show the Job Log in Its Folder", r#"{"type":"object","properties":{}}"#),
    cmd(
        "job.sound_test",
        "Play a Sound",
        r#"{"type":"object","properties":{"sound":{"type":"string","enum":["done","error"],"description":"the job-finished chime or the error buzz"}},"required":["sound"]}"#,
    ),
];

pub fn works(id: &str) -> bool {
    COMMANDS.iter().any(|c| c.id == id)
}

pub fn checklist(id: &str) -> &'static [&'static str] {
    match id {
        "job.log_info" | "job.log_show" => &["LASER-26", "SET-23"],
        "job.sound_test" => &["LASER-25", "SET-22"],
        _ => &[],
    }
}

thread_local! {
    static REQUESTS: RefCell<Vec<(&'static str, Value)>> = const { RefCell::new(Vec::new()) };
    #[cfg(test)]
    static REVEALED: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
}

/// Ask for a command from a place that has no app at hand (the Settings pages). It runs later
/// in the same frame.
pub fn request(id: &'static str, args: Value) {
    REQUESTS.with(|r| r.borrow_mut().push((id, args)));
}

/// Run what [`request`] collected.
pub fn run_requests(app: &mut DarkBurn) {
    let todo = REQUESTS.with(|r| std::mem::take(&mut *r.borrow_mut()));
    for (id, args) in todo {
        if let Some(Err(e)) = execute(app, id, &args) {
            app.note = Some(e);
        }
    }
}

/// Open the system file manager on `dir`. Nothing opens under test: the folder is only noted.
fn reveal(dir: &Path) {
    #[cfg(test)]
    {
        REVEALED.with(|r| r.borrow_mut().push(dir.to_path_buf()));
    }
    if cfg!(test) {
        return; // tests never open a file manager
    }
    #[cfg(target_os = "windows")]
    let program = "explorer";
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let program = "xdg-open";
    if let Err(e) = std::process::Command::new(program).arg(dir).spawn() {
        log::warn!("could not open {}: {e}", dir.display());
    }
}

/// The folders [`reveal`] was asked to open on this thread (tests only).
#[cfg(test)]
pub fn take_revealed() -> Vec<PathBuf> {
    REVEALED.with(|r| std::mem::take(&mut *r.borrow_mut()))
}

fn log_info(app: &DarkBurn, args: &Value) -> Result<Value, String> {
    let path = crate::settings_wiring::job_log_path(app).ok_or("there is no settings folder to keep the job log in")?;
    let want = args.get("rows").and_then(Value::as_u64).unwrap_or(20) as usize;
    let rows = if path.exists() { crate::settings_wiring::read_job_log(&path).map_err(|e| e.to_string())? } else { Vec::new() };
    let columns: Vec<&str> = crate::settings_wiring::JOB_LOG_HEADER.split(',').collect();
    let latest: Vec<&Vec<String>> = rows.iter().skip(rows.len().saturating_sub(want)).collect();
    Ok(json!({
        "enabled": app.prefs.opts.job_log,
        "path": path.display().to_string(),
        "exists": path.exists(),
        "jobs": rows.len(),
        "columns": columns,
        "rows": latest,
    }))
}

pub fn execute(app: &mut DarkBurn, id: &str, args: &Value) -> Option<Result<Value, String>> {
    Some(match id {
        "job.log_info" => log_info(app, args),
        "job.log_show" => match crate::settings_wiring::job_log_path(app) {
            Some(path) => {
                let dir = path.parent().map_or_else(|| PathBuf::from("."), Path::to_path_buf);
                match std::fs::create_dir_all(&dir) {
                    Ok(()) => {
                        reveal(&dir);
                        Ok(json!({ "folder": dir.display().to_string(), "path": path.display().to_string() }))
                    }
                    Err(e) => Err(format!("could not open {}: {e}", dir.display())),
                }
            }
            None => Err("there is no settings folder to keep the job log in".into()),
        },
        "job.sound_test" => match args.get("sound").and_then(Value::as_str) {
            Some("done") => {
                crate::sounds::play(Sound::JobDone);
                Ok(json!({ "played": "done" }))
            }
            Some("error") => {
                crate::sounds::play(Sound::Error);
                Ok(json!({ "played": "error" }))
            }
            _ => Err("sound must be \"done\" or \"error\"".into()),
        },
        _ => return None,
    })
}

/// From a menu or a shortcut: the job log's folder. The others need an argument.
pub fn run(app: &mut DarkBurn, _ctx: &egui::Context, id: &str) -> bool {
    if !works(id) {
        return false;
    }
    if id == "job.log_show"
        && let Some(Err(e)) = execute(app, id, &Value::Null)
    {
        app.note = Some(e);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::execute_headless;
    use crate::prefs::Prefs;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("darkburn-misc-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn the_commands_are_registered_and_do_not_touch_the_machine() {
        for c in COMMANDS {
            let info = crate::commands::describe(c.id).unwrap_or_else(|| panic!("{} is not registered", c.id));
            assert!(info.implemented && !info.hardware, "{}", c.id);
            assert!(!info.checklist.is_empty(), "{}", c.id);
        }
    }

    #[test]
    fn the_test_sound_command_asks_for_the_sound_whatever_the_setting() {
        let mut a = DarkBurn::new(Prefs::default());
        crate::sounds::take_requests();
        assert_eq!(execute_headless(&mut a, "job.sound_test", &json!({ "sound": "done" })).unwrap()["played"], "done");
        execute_headless(&mut a, "job.sound_test", &json!({ "sound": "error" })).unwrap();
        assert_eq!(crate::sounds::take_requests(), [Sound::JobDone, Sound::Error]);
        assert!(execute_headless(&mut a, "job.sound_test", &json!({ "sound": "loud" })).is_err());
        assert!(crate::sounds::take_requests().is_empty());
    }

    #[test]
    fn job_log_info_tells_where_the_log_is_and_what_is_in_it() {
        let dir = temp("info");
        let mut a = DarkBurn::new(Prefs::default());
        assert!(execute_headless(&mut a, "job.log_info", &Value::Null).is_err(), "no folder yet");
        a.settings_dir = Some(dir.clone());
        let v = execute_headless(&mut a, "job.log_info", &Value::Null).unwrap();
        assert_eq!((v["enabled"].as_bool(), v["exists"].as_bool(), v["jobs"].as_u64()), (Some(false), Some(false), Some(0)));
        assert_eq!(v["path"], dir.join(crate::settings_wiring::JOB_LOG_FILE).display().to_string());
        assert_eq!(v["columns"][0], "start (UTC)");
        // Turn the log on through the settings command, run three jobs past it.
        execute_headless(&mut a, "setting.set", &json!({ "key": "job_log", "value": true })).unwrap();
        for _ in 0..3 {
            let done = crate::machine_api::MachineEvent::JobFinished(Default::default());
            crate::settings_wiring::on_machine_events(&mut a, &[crate::machine_api::MachineEvent::JobStarted, done]);
        }
        let v = execute_headless(&mut a, "job.log_info", &json!({ "rows": 2 })).unwrap();
        assert_eq!((v["enabled"].as_bool(), v["exists"].as_bool(), v["jobs"].as_u64()), (Some(true), Some(true), Some(3)));
        assert_eq!(v["rows"].as_array().unwrap().len(), 2);
        assert_eq!(v["rows"][0][4], "finished");
        // Another folder from the setting.
        let other = temp("info-other");
        execute_headless(&mut a, "setting.set", &json!({ "key": "job_log_dir", "value": other.display().to_string() })).unwrap();
        let v = execute_headless(&mut a, "job.log_info", &Value::Null).unwrap();
        assert_eq!(v["path"], other.join(crate::settings_wiring::JOB_LOG_FILE).display().to_string());
        assert_eq!(v["jobs"], 0);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&other);
    }

    #[test]
    fn show_makes_the_folder_and_asks_the_file_manager_for_it() {
        let dir = temp("show").join("logs");
        let mut a = DarkBurn::new(Prefs::default());
        a.prefs.opts.job_log_dir = dir.display().to_string();
        take_revealed();
        let v = execute_headless(&mut a, "job.log_show", &Value::Null).unwrap();
        assert_eq!(v["folder"], dir.display().to_string());
        assert!(dir.is_dir());
        assert_eq!(take_revealed(), std::slice::from_ref(&dir));
        // The same from the Settings window's button.
        request("job.log_show", Value::Null);
        run_requests(&mut a);
        assert_eq!(take_revealed(), std::slice::from_ref(&dir));
        run_requests(&mut a);
        assert!(take_revealed().is_empty(), "a request runs once");
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
    }
}
