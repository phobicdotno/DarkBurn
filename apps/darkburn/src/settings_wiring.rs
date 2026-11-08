//! Behaviour settings that used to be stored only [SET-15, SET-16, SET-18, SET-20, SET-22,
//! SET-23, SET-28, SET-36, SB-05]: the pure rules, so they can be tested without a window, and
//! the few functions the shell calls.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use darkburn_core::Document;
use darkburn_grbl::PortInfo;
use darkburn_job::Toolpath;

use crate::job_build::JobProjectSettings;
use crate::options::Options;
use crate::shell::DarkBurn;

// ----- Output curve tolerance (SET-36) -------------------------------------------------------

/// The curve flattening tolerance jobs use, mm.
pub fn output_tolerance_mm(opts: &Options) -> f64 {
    if opts.output_tolerance.is_finite() { opts.output_tolerance.clamp(0.0001, 10.0) } else { 0.01 }
}

// ----- Job settings defaults (SET-15, SET-16, SET-18) ----------------------------------------

/// The job settings a new project starts with: the user's default states for "Cut selected
/// graphics" and "Use selection origin".
pub fn default_job_settings(opts: &Options, planner: &darkburn_job::PlannerSettings) -> JobProjectSettings {
    JobProjectSettings {
        planner: planner.clone(),
        cut_selected: opts.default_cut_selected,
        use_selection_origin: opts.default_use_selection_origin,
        ..Default::default()
    }
}

/// With "Save job origin and cut selected in the project" off, a project's saved start/anchor,
/// cut selected and selection origin are ignored on open: the user's defaults apply instead.
pub fn apply_save_job_origin_rule(doc: &mut Document, opts: &Options, planner: &darkburn_job::PlannerSettings) {
    if opts.save_job_origin || doc.job_settings.is_none() {
        return;
    }
    let mut s = JobProjectSettings::from_doc(doc, planner);
    let d = default_job_settings(opts, planner);
    s.start_from = d.start_from;
    s.anchor = d.anchor;
    s.cut_selected = d.cut_selected;
    s.use_selection_origin = d.use_selection_origin;
    s.store(doc);
}

/// Whether shapes outside the work area are left out of a job (the project's switch or the
/// user's preference).
pub fn discard_out_of_bounds(project: &JobProjectSettings, opts: &Options) -> bool {
    project.discard_out_of_bounds || opts.ignore_out_of_bounds
}

/// Called after a project is started from scratch: the user's default job states, and the
/// device's default layers when "Load default layers on new project" is on (SET-20).
pub fn on_new_project(app: &mut DarkBurn) {
    let opts = app.prefs.opts.clone();
    if opts.default_cut_selected || opts.default_use_selection_origin {
        let s = default_job_settings(&opts, &app.job.default_planner);
        s.store(&mut app.editor.doc);
    }
    if opts.default_layers_on_new
        && let Some(text) = app.machine.active().state.default_layers.clone()
        && let Ok(layers) = serde_json::from_str::<Vec<darkburn_core::layer::Layer>>(&text)
        && layers.len() == app.editor.doc.layers.len()
    {
        app.editor.doc.layers = layers;
    }
}

/// Called after a project file is opened.
pub fn on_open_project(app: &mut DarkBurn) {
    let opts = app.prefs.opts.clone();
    let planner = app.job.default_planner.clone();
    apply_save_job_origin_rule(&mut app.editor.doc, &opts, &planner);
}

// ----- Serial ports (SET-28) -----------------------------------------------------------------

/// The ports DarkBurn lists: everything except the ones the user excluded.
pub fn filter_ports(ports: &mut Vec<PortInfo>, excluded: &[String]) {
    ports.retain(|p| !excluded.iter().any(|e| !e.trim().is_empty() && e.trim().eq_ignore_ascii_case(&p.name)));
}

// ----- Job log (SET-23) ----------------------------------------------------------------------

/// One finished or stopped job.
#[derive(Clone, Debug, PartialEq)]
pub struct JobLogEntry {
    pub project: String,
    pub device: String,
    /// "finished" or "stopped".
    pub result: String,
    pub detail: String,
    pub seconds: f64,
    pub lines: usize,
    pub errors: usize,
}

pub const JOB_LOG_FILE: &str = "job_log.csv";
pub const JOB_LOG_HEADER: &str = "time (UTC),project,device,result,detail,seconds,lines,errors";

fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_string() }
}

/// "2026-11-09 14:03:07" for a Unix time.
pub fn utc_text(unix_secs: u64) -> String {
    let days = (unix_secs / 86_400) as i64;
    let rem = unix_secs % 86_400;
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
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
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}", rem / 3600, (rem / 60) % 60, rem % 60)
}

pub fn log_line(time: &str, e: &JobLogEntry) -> String {
    [
        csv_field(time),
        csv_field(&e.project),
        csv_field(&e.device),
        csv_field(&e.result),
        csv_field(&e.detail),
        format!("{:.1}", e.seconds),
        e.lines.to_string(),
        e.errors.to_string(),
    ]
    .join(",")
}

/// Append `entry` to `dir/job_log.csv`, writing the header first when the file is new.
pub fn append_job_log(dir: &Path, entry: &JobLogEntry, unix_secs: u64) -> std::io::Result<PathBuf> {
    use std::io::Write;
    std::fs::create_dir_all(dir)?;
    let path = dir.join(JOB_LOG_FILE);
    let fresh = !path.exists() || std::fs::metadata(&path).map_or(true, |m| m.len() == 0);
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&path)?;
    if fresh {
        writeln!(f, "{JOB_LOG_HEADER}")?;
    }
    writeln!(f, "{}", log_line(&utc_text(unix_secs), entry))?;
    Ok(path)
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// What the shell does with the machine's job events: the job log and the sounds.
pub fn on_machine_events(app: &mut DarkBurn, events: &[crate::machine_api::MachineEvent]) {
    use crate::machine_api::MachineEvent as E;
    let opts = app.prefs.opts.clone();
    for e in events {
        let (entry, sound) = match e {
            E::JobFinished(p) => (
                JobLogEntry {
                    project: project_name(app),
                    device: app.machine.active().name.clone(),
                    result: "finished".into(),
                    detail: String::new(),
                    seconds: p.elapsed.as_secs_f64(),
                    lines: p.lines_done,
                    errors: p.errors,
                },
                crate::sounds::Sound::JobDone,
            ),
            E::JobAborted(r) => (
                JobLogEntry {
                    project: project_name(app),
                    device: app.machine.active().name.clone(),
                    result: "stopped".into(),
                    detail: r.to_string(),
                    seconds: 0.0,
                    lines: 0,
                    errors: 0,
                },
                crate::sounds::Sound::Error,
            ),
            E::Error(_) | E::Alarm(_) | E::ConnectFailed(_) => {
                crate::sounds::play_if(&opts, crate::sounds::Sound::Error);
                continue;
            }
            _ => continue,
        };
        crate::sounds::play_if(&opts, sound);
        if opts.job_log
            && let Some(dir) = app.settings_dir.clone()
            && let Err(err) = append_job_log(&dir, &entry, now_secs())
        {
            log::warn!("could not write the job log: {err}");
        }
    }
}

fn project_name(app: &DarkBurn) -> String {
    app.project_path.as_deref().and_then(Path::file_name).map_or_else(|| "Untitled".to_string(), |n| n.to_string_lossy().into_owned())
}

// ----- Status bar progress (SB-05) -----------------------------------------------------------

/// Where a running job is: layer name, layer number out of the layers burnt, pass out of passes.
#[derive(Clone, Debug, PartialEq)]
pub struct LayerPass {
    pub layer: String,
    pub layer_no: usize,
    pub layers: usize,
    pub pass: u32,
    pub passes: u32,
}

impl LayerPass {
    pub fn text(&self) -> String {
        format!("{} ({}/{}), pass {}/{}", self.layer, self.layer_no, self.layers, self.pass, self.passes)
    }
}

/// The layer and pass the job is in when `fraction` (0..1) of its operations are done.
pub fn layer_pass_at(tp: &Toolpath, fraction: f64) -> Option<LayerPass> {
    if tp.ops.is_empty() || tp.layers.is_empty() {
        return None;
    }
    let idx = ((fraction.clamp(0.0, 1.0) * tp.ops.len() as f64) as usize).min(tp.ops.len() - 1);
    let st = tp.state_before(idx + 1);
    let li = st.layer?;
    let l = tp.layers.get(li)?;
    Some(LayerPass { layer: l.name.clone(), layer_no: li + 1, layers: tp.layers.len(), pass: st.pass.unwrap_or(1), passes: l.passes.max(1) })
}

/// The status bar text while a job runs, or `None`.
pub fn status_text(app: &DarkBurn) -> Option<String> {
    let snap = app.machine.handle.snapshot();
    let p = snap.job.as_ref()?;
    let job = app.job.last_job.as_ref()?;
    let lp = layer_pass_at(&job.toolpath, p.fraction?)?;
    Some(lp.text())
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use super::*;
    use darkburn_core::Shape;
    use darkburn_core::kurbo::Rect;
    use darkburn_core::layer::Mode;

    #[test]
    fn tolerance_is_clamped() {
        let mut o = Options::default();
        o.output_tolerance = 0.05;
        assert_eq!(output_tolerance_mm(&o), 0.05);
        o.output_tolerance = f64::NAN;
        assert_eq!(output_tolerance_mm(&o), 0.01);
        o.output_tolerance = 0.0;
        assert_eq!(output_tolerance_mm(&o), 0.0001);
    }

    #[test]
    fn new_projects_start_with_the_default_job_states() {
        let mut a = DarkBurn::new(crate::prefs::Prefs::default());
        crate::files::new_project(&mut a);
        assert!(!crate::job_cmds::current_settings(&a).cut_selected);
        a.prefs.opts.default_cut_selected = true;
        a.prefs.opts.default_use_selection_origin = true;
        crate::files::new_project(&mut a);
        let s = crate::job_cmds::current_settings(&a);
        assert!(s.cut_selected && s.use_selection_origin);
    }

    #[test]
    fn default_layers_load_on_new_only_when_asked() {
        let mut a = DarkBurn::new(crate::prefs::Prefs::default());
        a.editor.doc.layer_mut(2).main_mut().speed = 77.0;
        assert!(crate::machine_ui::save_default_layers(&mut a));
        a.prefs.opts.default_layers_on_new = false;
        crate::files::new_project(&mut a);
        assert_ne!(a.editor.doc.layer(2).main().speed, 77.0);
        a.prefs.opts.default_layers_on_new = true;
        crate::files::new_project(&mut a);
        assert_eq!(a.editor.doc.layer(2).main().speed, 77.0);
    }

    #[test]
    fn saved_job_origin_is_ignored_when_the_option_is_off() {
        let planner = darkburn_job::PlannerSettings::default();
        let mut doc = Document::new();
        JobProjectSettings { cut_selected: true, anchor: darkburn_job::JobAnchor::Center, ..Default::default() }.store(&mut doc);
        let mut o = Options::default();
        o.save_job_origin = true;
        apply_save_job_origin_rule(&mut doc, &o, &planner);
        assert!(JobProjectSettings::from_doc(&doc, &planner).cut_selected, "kept when saving is on");
        o.save_job_origin = false;
        apply_save_job_origin_rule(&mut doc, &o, &planner);
        let s = JobProjectSettings::from_doc(&doc, &planner);
        assert!(!s.cut_selected && s.anchor == darkburn_job::JobAnchor::BottomLeft);
    }

    #[test]
    fn ignore_out_of_bounds_reaches_the_job() {
        let mut a = DarkBurn::new(crate::prefs::Prefs::default());
        a.editor.doc.layer_mut(0).main_mut().mode = Mode::Line;
        a.editor.doc.add(Shape::rect(Rect::new(10.0, 10.0, 20.0, 20.0), 0.0, 0));
        a.editor.doc.add(Shape::rect(Rect::new(5000.0, 5000.0, 5010.0, 5010.0), 0.0, 0));
        let off = crate::job_cmds::build_input(&a);
        assert!(!off.settings.discard_out_of_bounds);
        a.prefs.opts.ignore_out_of_bounds = true;
        let on = crate::job_cmds::build_input(&a);
        assert!(on.settings.discard_out_of_bounds);
        let built = crate::job_build::build_job_simple(&on).unwrap();
        assert_eq!(built.dropped_shapes, 1);
    }

    #[test]
    fn excluded_ports_are_not_listed() {
        let p = |n: &str| PortInfo { name: n.into(), vid: None, pid: None, manufacturer: None, product: None, serial_number: None };
        let mut ports = vec![p("COM3"), p("COM9"), p("COM4")];
        filter_ports(&mut ports, &["com9".into(), " ".into()]);
        assert_eq!(ports.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["COM3", "COM4"]);
    }

    #[test]
    fn utc_text_is_right() {
        assert_eq!(utc_text(0), "1970-01-01 00:00:00");
        assert_eq!(utc_text(1_709_210_096), "2024-02-29 12:34:56");
    }

    #[test]
    fn job_log_is_csv_with_one_header() {
        let dir = std::env::temp_dir().join(format!("darkburn-joblog-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let e = JobLogEntry {
            project: "a, \"b\".dbrn".into(),
            device: "Simulator".into(),
            result: "finished".into(),
            detail: String::new(),
            seconds: 12.34,
            lines: 100,
            errors: 0,
        };
        append_job_log(&dir, &e, 0).unwrap();
        let p = append_job_log(&dir, &e, 60).unwrap();
        let text = std::fs::read_to_string(p).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], JOB_LOG_HEADER);
        assert_eq!(lines[1], "1970-01-01 00:00:00,\"a, \"\"b\"\".dbrn\",Simulator,finished,,12.3,100,0");
        assert!(lines[2].starts_with("1970-01-01 00:01:00"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_finished_job_event_writes_the_log_only_when_on() {
        let dir = std::env::temp_dir().join(format!("darkburn-joblog2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut a = DarkBurn::new(crate::prefs::Prefs::default());
        a.settings_dir = Some(dir.clone());
        let ev = [crate::machine_api::MachineEvent::JobAborted("Stopped by the user".into())];
        on_machine_events(&mut a, &ev);
        assert!(!dir.join(JOB_LOG_FILE).exists());
        a.prefs.opts.job_log = true;
        on_machine_events(&mut a, &ev);
        let text = std::fs::read_to_string(dir.join(JOB_LOG_FILE)).unwrap();
        assert!(text.contains("stopped") && text.contains("Stopped by the user"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn layer_and_pass_follow_the_job() {
        let mut d = Document::new();
        d.layer_mut(0).main_mut().passes = 2;
        d.layer_mut(0).name = "Cut".into();
        d.add(Shape::rect(Rect::new(0.0, 0.0, 10.0, 10.0), 0.0, 0));
        d.layer_mut(1).main_mut().mode = Mode::Line;
        d.add(Shape::rect(Rect::new(20.0, 0.0, 30.0, 10.0), 0.0, 1));
        let mut i = crate::job_cmds::build_input(&DarkBurn::new(crate::prefs::Prefs::default()));
        i.doc = d;
        let b = crate::job_build::build_job_simple(&i).unwrap();
        let first = layer_pass_at(&b.toolpath, 0.0).unwrap();
        assert_eq!((first.layer_no, first.pass, first.passes), (1, 1, 2));
        let last = layer_pass_at(&b.toolpath, 1.0).unwrap();
        assert_eq!(last.layer_no, 2);
        assert_eq!(last.layers, 2);
        assert!(first.text().contains("pass 1/2"));
    }

    #[test]
    fn a_second_launch_hands_its_files_to_the_running_copy() {
        let dir = std::env::temp_dir().join(format!("darkburn-inbox-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut a = DarkBurn::new(crate::prefs::Prefs::default());
        a.settings_dir = Some(dir.clone());
        a.prefs.files.single_instance = true;
        let svg = dir.join("box.svg");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&svg, r#"<svg xmlns="http://www.w3.org/2000/svg" width="20mm" height="10mm"><rect width="20" height="10"/></svg>"#).unwrap();
        crate::instance::heartbeat(&dir);
        assert!(crate::instance::running(&dir));
        crate::instance::forward(&dir, std::slice::from_ref(&svg)).unwrap();
        let ctx = egui::Context::default();
        crate::file_io::instance_tick(&ctx, &mut a);
        for _ in 0..200 {
            crate::file_io::frame(&ctx, &mut a);
            if a.editor.doc.iter().count() > 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        assert!(a.editor.doc.iter().count() > 0, "the file arrived in the running copy");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
