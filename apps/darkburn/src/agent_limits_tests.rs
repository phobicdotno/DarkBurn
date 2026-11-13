//! What an agent's command may not do: point settings at files or the network, write into
//! DarkBurn's own folders, or open a file dialog.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use darkburn_mcp::{Backend, CommandError, ErrorCode};
use serde_json::{Value, json};

use super::*;
use crate::mcp_backend::{McpBackend, SharedApp};

fn shared() -> SharedApp {
    Rc::new(RefCell::new(DarkBurn::new(Prefs::default())))
}

fn agent(app: &SharedApp) -> McpBackend {
    McpBackend::new(app.clone(), egui::Context::default(), true)
}

/// A fresh folder of its own for one test.
fn scratch(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("darkburn-agent-limits-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn prefs_json(app: &SharedApp) -> String {
    app.borrow().prefs.to_json()
}

// ----- settings that send files or the network somewhere ---------------------------------------

#[test]
fn an_agent_cannot_point_a_setting_at_a_file_an_address_or_the_agent_policy() {
    let app = shared();
    let mut b = agent(&app);
    let tries: Vec<(&str, Value)> = vec![
        ("opts.update_url", json!("https://example.invalid/latest.json")),
        ("update_url", json!("http://127.0.0.1:9/latest.json")),
        ("opts/update_url", json!("https://example.invalid/")),
        ("opts.job_log_dir", json!("/elsewhere/logs")),
        ("job_log_dir", json!("/elsewhere/logs")),
        ("opts.excluded_ports", json!([])),
        ("excluded_ports", json!(["COM9"])),
        ("recent", json!(["/elsewhere/a.dbrn"])),
        ("library_paths", json!({ "1": "/elsewhere/a.clb" })),
        ("art_libraries", json!(["/elsewhere/a.dbart"])),
        ("shx_fonts", json!(["/elsewhere/a.shx"])),
        ("files.export_dir", json!("/elsewhere")),
        ("mcp.allow_hardware", json!(true)),
        ("mcp/allow_hardware", json!(true)),
        ("MCP.allow_gweike", json!(true)),
        ("mcp", json!({})),
        ("opts.pendant", json!(true)),
    ];
    let before = prefs_json(&app);
    for id in ["settings.set", "setting.set"] {
        for (key, value) in &tries {
            let e = b.execute(id, json!({ "key": key, "value": value })).expect_err(&format!("{id} {key} was let through"));
            assert_eq!(e.code, ErrorCode::Denied, "{id} {key}: {}", e.message);
            assert!(e.message.contains("only be changed by the user") && e.message.contains("ask the user"), "{id} {key}: {}", e.message);
            assert_eq!(prefs_json(&app), before, "{id} {key} changed the preferences");
        }
    }
    // Ordinary settings are still the agent's to change.
    b.execute("settings.set", json!({ "key": "opts.snap_grid", "value": false })).unwrap();
    b.execute("setting.set", json!({ "key": "grid_spacing", "value": 25.0 })).unwrap();
    assert!(!app.borrow().prefs.opts.snap_grid);
}

#[test]
fn a_setting_reached_under_another_name_is_put_back_and_refused() {
    // Whatever name a key comes under, the values are compared after the command.
    let mut app = DarkBurn::new(Prefs::default());
    let kept = Kept::of(&app);
    app.prefs.opts.update_url = "https://example.invalid/".into();
    app.prefs.opts.job_log_dir = "/elsewhere".into();
    app.prefs.recent.push("/elsewhere/a.dbrn".into());
    let e = after(&mut app, "settings.set", &kept, Ok(json!({}))).unwrap_err();
    assert!(e.message.contains("ask the user"), "{}", e.message);
    assert_eq!(app.prefs.opts.update_url, Prefs::default().opts.update_url);
    assert_eq!(app.prefs.opts.job_log_dir, Prefs::default().opts.job_log_dir);
    assert!(app.prefs.recent.is_empty());
}

#[test]
fn other_agent_commands_keep_the_persons_settings_and_their_own_file_lists() {
    let mut app = DarkBurn::new(Prefs::default());
    // A preferences file an agent imports keeps this computer's paths and addresses.
    let kept = Kept::of(&app);
    app.prefs.opts.update_url = "https://example.invalid/".into();
    app.prefs.opts.excluded_ports = vec!["COM9".into()];
    app.prefs.files.export_dir = Some("/elsewhere".into());
    app.prefs.opts.snap_grid = !app.prefs.opts.snap_grid;
    after(&mut app, "project.prefs_import", &kept, Ok(json!({ "ok": true }))).unwrap();
    assert_eq!(app.prefs.opts.update_url, Prefs::default().opts.update_url);
    assert_eq!(app.prefs.opts.excluded_ports, Prefs::default().opts.excluded_ports);
    assert_eq!(app.prefs.files.export_dir, None);
    assert_ne!(app.prefs.opts.snap_grid, Prefs::default().opts.snap_grid, "the rest of the file is imported");
    // Opening a file adds it to the recent list as it should, but no command moves the job log.
    let kept = Kept::of(&app);
    app.prefs.recent.push("/somewhere/a.dbrn".into());
    app.prefs.opts.job_log_dir = "/elsewhere".into();
    after(&mut app, "file.open", &kept, Ok(json!({}))).unwrap();
    assert_eq!(app.prefs.recent, vec!["/somewhere/a.dbrn".to_string()]);
    assert_eq!(app.prefs.opts.job_log_dir, Prefs::default().opts.job_log_dir);
}

#[test]
fn the_person_still_changes_these_settings() {
    let dir = scratch("person");
    let mut app = DarkBurn::new(Prefs::default());
    let to = dir.join("logs").display().to_string();
    crate::api::execute_headless(&mut app, "setting.set", &json!({ "key": "job_log_dir", "value": to })).unwrap();
    assert_eq!(app.prefs.opts.job_log_dir, to);
    let _ = std::fs::remove_dir_all(&dir);
}

// ----- DarkBurn's own folders ------------------------------------------------------------------

/// An app whose settings folder is `<dir>/settings`, with an agent in front of it.
fn with_settings(dir: &Path) -> (SharedApp, McpBackend, PathBuf) {
    let settings = dir.join("settings");
    std::fs::create_dir_all(&settings).unwrap();
    let app = shared();
    app.borrow_mut().settings_dir = Some(settings.clone());
    let b = agent(&app);
    (app, b, settings)
}

fn refused_as_settings_folder(r: Result<Value, CommandError>, what: &str) {
    let e = r.expect_err(&format!("{what} was written"));
    assert_eq!(e.code, ErrorCode::Denied, "{what}: {}", e.message);
    assert!(e.message.contains("settings folder"), "{what}: {}", e.message);
}

#[test]
fn an_agent_never_saves_or_exports_into_the_settings_folder() {
    let dir = scratch("folder");
    let (_app, mut b, settings) = with_settings(&dir);
    std::fs::create_dir_all(dir.join("other")).unwrap();
    let p = |p: PathBuf| json!({ "path": p.display().to_string() });
    let tries = [
        ("project.save_as", p(settings.join("prefs.json"))),
        ("file.save_as", p(settings.join("new").join("deep.dbrn"))),
        ("project.save_as", p(dir.join("other").join("..").join("settings").join("a.dbrn"))),
        ("project.save_as", p(dir.join("other").join(".").join("..").join("settings"))),
        ("project.prefs_export", p(settings.join("prefs.json"))),
        ("file.prefs_export", p(settings.join("prefs.json"))),
        ("support.write", p(settings.join("s.zip"))),
        ("help.support_data", p(settings.join("s.zip"))),
        ("hotkeys.export", p(settings.join("k.dbhk"))),
        ("library.save_as", p(settings.join("l.clb"))),
        ("project.export", json!({ "path": settings.join("a.svg").display().to_string(), "format": "svg" })),
        ("job.save_gcode", p(settings.join("a.gc"))),
        ("job.material_test_presets", json!({ "action": "export", "path": settings.join("m.json").display().to_string() })),
    ];
    for (id, args) in tries {
        refused_as_settings_folder(b.execute(id, args.clone()), &format!("{id} {args}"));
    }
    assert_eq!(std::fs::read_dir(&settings).unwrap().count(), 0, "something was written into the settings folder");
    // Beside it is fine.
    b.execute("project.save_as", p(dir.join("other").join("a.dbrn"))).unwrap();
    assert!(dir.join("other").join("a.dbrn").is_file());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn saving_back_to_a_file_in_the_settings_folder_is_refused_too() {
    let dir = scratch("implicit");
    let (app, mut b, settings) = with_settings(&dir);
    app.borrow_mut().project_path = Some(settings.join("p.dbrn"));
    refused_as_settings_folder(b.execute("file.save", json!({})), "file.save");
    app.borrow_mut().extras.library.path = Some(settings.join("l.clb"));
    refused_as_settings_folder(b.execute("library.save", json!({})), "library.save");
    assert_eq!(std::fs::read_dir(&settings).unwrap().count(), 0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_relative_path_into_the_settings_folder_is_refused() {
    // The working folder is the settings folder here, so plain names land in it.
    let cwd = std::env::current_dir().unwrap();
    let app = shared();
    app.borrow_mut().settings_dir = Some(cwd.clone());
    let mut b = agent(&app);
    for rel in ["agent-limits-x.dbrn", "./agent-limits-x.dbrn", "no-such-folder/../agent-limits-x.dbrn"] {
        refused_as_settings_folder(b.execute("project.save_as", json!({ "path": rel })), rel);
        assert!(!cwd.join("agent-limits-x.dbrn").exists(), "{rel} was written");
    }
}

#[test]
fn darkburns_own_folders_are_all_held() {
    let app = shared();
    let folders = protected_folders(&app.borrow());
    let exe_dir = std::env::current_exe().unwrap().parent().unwrap().to_path_buf();
    assert!(folders.contains(&exe_dir.join("settings")), "the portable folder: {folders:?}");
    if let Some(d) = eframe::storage_dir("DarkBurn") {
        assert!(folders.contains(&d), "the installed folder: {folders:?}");
    }
    let control = darkburn_mcp::control::control_file_path();
    assert!(folders.iter().any(|f| control.starts_with(f)), "the control file: {folders:?}");
    // A write into the portable folder is refused even when this copy is not portable.
    let mut b = agent(&app);
    let target = exe_dir.join("settings").join("agent-limits-x.dbrn");
    refused_as_settings_folder(b.execute("project.save_as", json!({ "path": target.display().to_string() })), "portable");
    assert!(!target.exists());
}

#[cfg(unix)]
#[test]
fn a_link_into_the_settings_folder_is_followed() {
    let dir = scratch("link");
    let (_app, mut b, settings) = with_settings(&dir);
    // A folder link, and a file link whose target is not there yet.
    std::os::unix::fs::symlink(&settings, dir.join("looks-harmless")).unwrap();
    std::os::unix::fs::symlink(settings.join("prefs.json"), dir.join("note.json")).unwrap();
    std::os::unix::fs::symlink("settings/x", dir.join("relative-link")).unwrap();
    for target in [dir.join("looks-harmless").join("a.dbrn"), dir.join("note.json"), dir.join("relative-link")] {
        refused_as_settings_folder(b.execute("project.save_as", json!({ "path": target.display().to_string() })), &target.display().to_string());
    }
    assert_eq!(std::fs::read_dir(&settings).unwrap().count(), 0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(windows)]
#[test]
fn other_letter_case_and_windows_name_tricks_do_not_get_into_the_settings_folder() {
    let dir = scratch("case");
    let (_app, mut b, settings) = with_settings(&dir);
    let upper = dir.join("SETTINGS");
    for target in [upper.join("a.dbrn"), dir.join("Settings.").join("a.dbrn"), dir.join("settings ").join("b.dbrn")] {
        refused_as_settings_folder(b.execute("project.save_as", json!({ "path": target.display().to_string() })), &target.display().to_string());
    }
    assert_eq!(std::fs::read_dir(&settings).unwrap().count(), 0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn names_compare_the_way_the_system_reads_them() {
    let folder = Path::new("/u/AppData/DarkBurn");
    let inside_by = |t: &str, r| inside(Path::new(t), folder, r);
    assert!(inside_by("/u/AppData/DarkBurn/prefs.json", NameRules::Exact));
    assert!(inside_by("/u/AppData/DarkBurn", NameRules::Exact));
    assert!(!inside_by("/u/AppData/darkburn/prefs.json", NameRules::Exact));
    assert!(!inside_by("/u/AppData/DarkBurnOther/prefs.json", NameRules::Exact));
    assert!(!inside_by("/u/AppData", NameRules::Exact));
    assert!(inside_by("/u/appdata/DARKBURN/prefs.json", NameRules::AnyCase));
    assert!(inside_by("/U/APPDATA/darkburn/prefs.json", NameRules::Windows));
    assert!(inside_by("/u/AppData/DarkBurn. ./prefs.json", NameRules::Windows), "trailing dots and spaces");
    assert!(inside_by("/u/AppData/DarkBurn:stream", NameRules::Windows), "a stream of the folder");
    assert!(!inside_by("/u/AppData/DarkBurn2/prefs.json", NameRules::Windows));
}

#[test]
fn resolving_works_out_dots_and_the_working_folder() {
    let cwd = std::env::current_dir().unwrap();
    assert_eq!(resolve(Path::new("a/../b/./c")), resolve(&cwd.join("b").join("c")));
    let dir = scratch("resolve");
    assert_eq!(resolve(&dir.join("x").join("..").join("y")), resolve(&dir.join("y")));
    let _ = std::fs::remove_dir_all(&dir);
}

// ----- file dialogs ------------------------------------------------------------------------------

#[test]
fn no_agent_command_opens_a_file_dialog_without_a_path() {
    rfd::script_begin();
    let app = shared();
    let mut b = agent(&app);
    let mut ids: Vec<String> = DIALOG_WITHOUT_PATH.iter().map(|s| s.to_string()).collect();
    ids.extend(mcp_catalog::REDIRECTS.iter().map(|r| r.id.to_string()));
    for id in &ids {
        for args in [json!({}), Value::Null, json!({ "path": "" }), json!({ "paths": [] })] {
            let e = b.execute(id, args.clone()).expect_err(&format!("{id} {args} ran without a path"));
            assert!(e.message.contains("path required"), "{id}: {}", e.message);
        }
    }
    // Every other file command, run without arguments, opens no dialog and leaves none waiting.
    let all = crate::commands::list();
    for info in all.iter().filter(|i| i.id.starts_with("file.") || i.id.starts_with("project.")) {
        let id = info.id.as_str();
        if id == "file.exit" || id.starts_with("file.recent.") || crate::commands::hardware(id) {
            continue;
        }
        let _ = b.execute(id, json!({}));
        assert!(rfd::script_take_log().is_empty(), "{id} opened a file dialog");
        assert!(!app.borrow().fio.file_window_waiting(), "{id} left a file window waiting");
    }
    rfd::script_end();
}

#[test]
fn a_file_window_a_command_asks_for_is_dropped_and_the_agent_told() {
    let mut app = DarkBurn::new(Prefs::default());
    let kept = Kept::of(&app);
    crate::file_io::request_import_dialog(&mut app);
    let e = after(&mut app, "some.command", &kept, Ok(json!({}))).unwrap_err();
    assert!(e.message.contains("path required"), "{}", e.message);
    assert!(!app.fio.file_window_waiting());
    // One the person opened before stays.
    crate::file_io::request_import_dialog(&mut app);
    let kept = Kept::of(&app);
    after(&mut app, "some.command", &kept, Ok(json!({}))).unwrap();
    assert!(app.fio.file_window_waiting());
}

#[test]
fn support_data_takes_a_path_from_an_agent() {
    let dir = scratch("support");
    let app = shared();
    let mut b = agent(&app);
    let z = dir.join("s.zip");
    b.execute("help.support_data", json!({ "path": z.display().to_string() })).unwrap();
    assert!(z.is_file());
    let _ = std::fs::remove_dir_all(&dir);
}
