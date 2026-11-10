//! Behaviour settings that used to be stored only [SET-15, SET-16, SET-18, SET-20, SET-22,
//! SET-23, SET-28, SET-36, SB-05, LASER-25, LASER-26]: the pure rules, so they can be tested without a window, and
//! the few functions the shell calls.

use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

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

// ----- Job log (SET-23, LASER-26) and sounds (SET-22, LASER-25) --------------------------------

/// One run job: a row of the log.
#[derive(Clone, Debug, PartialEq)]
pub struct JobLogEntry {
    /// The project file (its full path), or "Untitled".
    pub file: String,
    pub device: String,
    /// "finished", "stopped" (by the user) or "failed".
    pub result: String,
    /// Why it stopped or failed; empty for a finished job.
    pub detail: String,
    /// How long the job ran.
    pub seconds: f64,
    pub lines: usize,
    pub errors: usize,
}

pub const JOB_LOG_FILE: &str = "job_log.csv";
pub const JOB_LOG_HEADER: &str = "start (UTC),duration (s),file,device,result,detail,lines,errors";

/// The job under way, noted when it starts, so its row has the start time and the duration
/// even when it is stopped half way.
#[derive(Clone, Copy, Debug, Default)]
pub struct JobRun {
    started: Option<(u64, Instant)>,
    /// The job handed to the machine last was made from the project (not a G-code file run as
    /// it is): when it finishes, variable text moves on if the project asks for that.
    pub from_project: bool,
}

#[cfg(test)]
impl JobRun {
    pub fn is_running(&self) -> bool {
        self.started.is_some()
    }
}

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

/// One row: when the job started, then the entry.
pub fn log_line(start: &str, e: &JobLogEntry) -> String {
    [
        csv_field(start),
        format!("{:.1}", e.seconds),
        csv_field(&e.file),
        csv_field(&e.device),
        csv_field(&e.result),
        csv_field(&e.detail),
        e.lines.to_string(),
        e.errors.to_string(),
    ]
    .join(",")
}

/// The folder the job log is written to: the one chosen in Settings, else the settings folder.
pub fn job_log_dir(opts: &Options, settings_dir: Option<&Path>) -> Option<PathBuf> {
    let chosen = opts.job_log_dir.trim();
    if chosen.is_empty() { settings_dir.map(Path::to_path_buf) } else { Some(PathBuf::from(chosen)) }
}

/// The job log file, whether or not it exists yet.
pub fn job_log_path(app: &DarkBurn) -> Option<PathBuf> {
    job_log_dir(&app.prefs.opts, app.settings_dir.as_deref()).map(|d| d.join(JOB_LOG_FILE))
}

/// A log written with other columns (an older DarkBurn) is kept under a free
/// `job_log-old.csv`, `job_log-old-2.csv`, ... name, so one file never mixes two layouts.
fn set_aside_other_layout(dir: &Path, path: &Path) -> std::io::Result<()> {
    let Ok(text) = std::fs::read_to_string(path) else { return Ok(()) };
    match text.lines().next() {
        None => return Ok(()),
        Some(first) if first.trim_start_matches('\u{feff}') == JOB_LOG_HEADER => return Ok(()),
        Some(_) => {}
    }
    let mut n = 1;
    loop {
        let name = if n == 1 { "job_log-old.csv".to_string() } else { format!("job_log-old-{n}.csv") };
        let to = dir.join(name);
        if !to.exists() {
            return std::fs::rename(path, to);
        }
        n += 1;
    }
}

/// Append `entry` to `dir/job_log.csv`, writing the header first when the file is new.
/// `start_unix` is when the job started.
pub fn append_job_log(dir: &Path, entry: &JobLogEntry, start_unix: u64) -> std::io::Result<PathBuf> {
    use std::io::Write;
    std::fs::create_dir_all(dir)?;
    let path = dir.join(JOB_LOG_FILE);
    set_aside_other_layout(dir, &path)?;
    let fresh = !path.exists() || std::fs::metadata(&path).map_or(true, |m| m.len() == 0);
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&path)?;
    if fresh {
        writeln!(f, "{JOB_LOG_HEADER}")?;
    }
    writeln!(f, "{}", log_line(&utc_text(start_unix), entry))?;
    Ok(path)
}

/// The rows of a job log (without the header), each split into its fields.
pub fn read_job_log(path: &Path) -> std::io::Result<Vec<Vec<String>>> {
    let text = std::fs::read_to_string(path)?;
    Ok(parse_csv(&text).into_iter().skip(1).collect())
}

/// Split CSV text into rows of fields (quoted fields, doubled quotes, line breaks in quotes).
pub fn parse_csv(text: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    quoted = false;
                }
            } else {
                field.push(c);
            }
            continue;
        }
        match c {
            '"' => quoted = true,
            ',' => row.push(std::mem::take(&mut field)),
            '\r' => {}
            '\n' => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
            }
            _ => field.push(c),
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    rows
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// A job the user ended on purpose (not a failure: no error sound, logged as "stopped").
pub fn stopped_by_user(reason: &str) -> bool {
    let r = reason.trim().to_ascii_lowercase();
    r == "stopped" || r == "cancelled" || r.starts_with("stopped by")
}

/// What the shell does with the machine's job events: the job log and the sounds. A batch of
/// events plays each sound at most once (an error that also ends the job is one buzz, not two).
pub fn on_machine_events(app: &mut DarkBurn, events: &[crate::machine_api::MachineEvent]) {
    use crate::machine_api::MachineEvent as E;
    use crate::sounds::Sound;
    let opts = app.prefs.opts.clone();
    let mut done = false;
    let mut error = false;
    for e in events {
        let (result, detail, progress) = match e {
            E::JobStarted => {
                app.job_run.started = Some((now_secs(), Instant::now()));
                continue;
            }
            E::JobFinished(p) => {
                done = true;
                // Auto-advance of the Variable Text panel: the next serial number or CSV row.
                if std::mem::take(&mut app.job_run.from_project) {
                    crate::text_cmds::after_job(app);
                }
                ("finished", String::new(), Some(p))
            }
            E::JobAborted(r) => {
                app.job_run.from_project = false;
                let by_user = stopped_by_user(r);
                error |= !by_user;
                (if by_user { "stopped" } else { "failed" }, r.to_string(), None)
            }
            E::Error(_) | E::Alarm(_) | E::ConnectFailed(_) => {
                error = true;
                continue;
            }
            _ => continue,
        };
        // A job whose start was not seen is logged as starting now, with the time it reports.
        let (start, ran) = match app.job_run.started.take() {
            Some((unix, at)) => (unix, at.elapsed().as_secs_f64()),
            None => (now_secs(), 0.0),
        };
        let entry = JobLogEntry {
            file: project_file(app),
            device: app.machine.active().name.clone(),
            result: result.into(),
            detail,
            seconds: progress.map_or(ran, |p| p.elapsed.as_secs_f64()),
            lines: progress.map_or(0, |p| p.lines_done),
            errors: progress.map_or(0, |p| p.errors),
        };
        if opts.job_log
            && let Some(dir) = job_log_dir(&opts, app.settings_dir.as_deref())
            && let Err(err) = append_job_log(&dir, &entry, start)
        {
            log::warn!("could not write the job log: {err}");
        }
    }
    if done {
        crate::sounds::play_if(&opts, Sound::JobDone);
    }
    if error {
        crate::sounds::play_if(&opts, Sound::Error);
    }
}

fn project_file(app: &DarkBurn) -> String {
    app.project_path.as_deref().map_or_else(|| "Untitled".to_string(), |p| p.display().to_string())
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
        let p = |n: &str| PortInfo { name: n.into(), ..Default::default() };
        let mut ports = vec![p("COM3"), p("COM9"), p("COM4")];
        filter_ports(&mut ports, &["com9".into(), " ".into()]);
        assert_eq!(ports.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["COM3", "COM4"]);
    }

    #[test]
    fn utc_text_is_right() {
        assert_eq!(utc_text(0), "1970-01-01 00:00:00");
        assert_eq!(utc_text(1_709_210_096), "2024-02-29 12:34:56");
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("darkburn-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn entry() -> JobLogEntry {
        JobLogEntry {
            file: "C:\\jobs\\a, \"b\".dbrn".into(),
            device: "Simulator".into(),
            result: "finished".into(),
            detail: String::new(),
            seconds: 12.34,
            lines: 100,
            errors: 0,
        }
    }

    #[test]
    fn job_log_is_csv_with_one_header() {
        let dir = temp("joblog");
        let e = entry();
        append_job_log(&dir, &e, 0).unwrap();
        let p = append_job_log(&dir, &e, 60).unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], JOB_LOG_HEADER);
        assert_eq!(lines[1], "1970-01-01 00:00:00,12.3,\"C:\\jobs\\a, \"\"b\"\".dbrn\",Simulator,finished,,100,0");
        assert!(lines[2].starts_with("1970-01-01 00:01:00"));
        // Read back: one row per job, the quoted file name whole again.
        let rows = read_job_log(&p).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], ["1970-01-01 00:00:00", "12.3", e.file.as_str(), "Simulator", "finished", "", "100", "0"]);
        assert_eq!(rows[0].len(), JOB_LOG_HEADER.split(',').count());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn csv_parsing_handles_quotes_and_line_breaks() {
        let rows = parse_csv("a,\"b,1\",\"say \"\"hi\"\"\"\r\n\"two\nlines\",,x\n");
        assert_eq!(rows, [vec!["a", "b,1", "say \"hi\""], vec!["two\nlines", "", "x"]]);
        assert_eq!(parse_csv("no,newline"), [vec!["no", "newline"]]);
        assert!(parse_csv("").is_empty());
    }

    #[test]
    fn a_log_with_other_columns_is_set_aside_not_mixed() {
        let dir = temp("joblog-old");
        std::fs::create_dir_all(&dir).unwrap();
        let old = "time (UTC),project,device,result,detail,seconds,lines,errors\n2026-01-01 00:00:00,a,b,finished,,1.0,2,0\n";
        std::fs::write(dir.join(JOB_LOG_FILE), old).unwrap();
        let p = append_job_log(&dir, &entry(), 0).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("job_log-old.csv")).unwrap(), old);
        assert_eq!(read_job_log(&p).unwrap().len(), 1);
        // A second old-layout file gets the next free name.
        std::fs::write(dir.join(JOB_LOG_FILE), old).unwrap();
        append_job_log(&dir, &entry(), 0).unwrap();
        assert!(dir.join("job_log-old-2.csv").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_log_goes_to_the_chosen_folder_or_the_settings_folder() {
        let mut o = Options::default();
        let settings = Path::new("settings-here");
        assert_eq!(job_log_dir(&o, Some(settings)), Some(settings.to_path_buf()));
        assert_eq!(job_log_dir(&o, None), None);
        o.job_log_dir = "  elsewhere ".into();
        assert_eq!(job_log_dir(&o, Some(settings)), Some(PathBuf::from("elsewhere")));
        assert_eq!(job_log_dir(&o, None), Some(PathBuf::from("elsewhere")));
    }

    #[test]
    fn a_job_event_writes_the_log_only_when_on() {
        let dir = temp("joblog2");
        let mut a = DarkBurn::new(crate::prefs::Prefs::default());
        a.settings_dir = Some(dir.clone());
        let ev = [crate::machine_api::MachineEvent::JobAborted("Stopped".into())];
        on_machine_events(&mut a, &ev);
        assert!(!dir.join(JOB_LOG_FILE).exists());
        a.prefs.opts.job_log = true;
        on_machine_events(&mut a, &ev);
        let rows = read_job_log(&dir.join(JOB_LOG_FILE)).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0][2].as_str(), rows[0][4].as_str(), rows[0][5].as_str()), ("Untitled", "stopped", "Stopped"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_row_has_the_start_time_the_duration_the_file_and_the_device() {
        use crate::machine_api::MachineEvent as E;
        let dir = temp("joblog3");
        let other = temp("joblog3-chosen");
        let mut a = DarkBurn::new(crate::prefs::Prefs::default());
        a.settings_dir = Some(dir.clone());
        a.prefs.opts.job_log = true;
        a.project_path = Some(PathBuf::from("plates").join("door sign.dbrn"));
        let before = now_secs();
        on_machine_events(&mut a, &[E::JobStarted]);
        assert!(a.job_run.is_running());
        std::thread::sleep(std::time::Duration::from_millis(120));
        // A failure half way: the duration is the time since the start.
        on_machine_events(&mut a, &[E::JobAborted("Connection lost".into())]);
        assert!(!a.job_run.is_running());
        // A finished job reports its own time and line count; this one goes to the chosen folder.
        a.prefs.opts.job_log_dir = other.display().to_string();
        let p =
            crate::machine_api::JobProgress { elapsed: std::time::Duration::from_millis(61_500), lines_done: 42, errors: 1, ..Default::default() };
        on_machine_events(&mut a, &[E::JobStarted, E::JobFinished(p)]);
        let first = read_job_log(&dir.join(JOB_LOG_FILE)).unwrap();
        assert_eq!(first.len(), 1);
        let r = &first[0];
        assert!(r[0] >= utc_text(before) && r[0] <= utc_text(now_secs()), "start time {}", r[0]);
        let secs: f64 = r[1].parse().unwrap();
        assert!((0.1..5.0).contains(&secs), "ran for {secs} s");
        assert!(r[2].ends_with("door sign.dbrn") && r[2].contains("plates"), "{}", r[2]);
        assert_eq!(r[3], a.machine.active().name);
        assert_eq!((r[4].as_str(), r[5].as_str()), ("failed", "Connection lost"));
        let second = read_job_log(&job_log_path(&a).unwrap()).unwrap();
        assert_eq!(job_log_path(&a).unwrap(), other.join(JOB_LOG_FILE));
        assert_eq!(second.len(), 1);
        assert_eq!(second[0][1..], ["61.5", r[2].as_str(), r[3].as_str(), "finished", "", "42", "1"]);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&other);
    }

    #[test]
    fn sounds_are_asked_for_when_a_job_ends_or_something_fails() {
        use crate::machine_api::{JobProgress, MachineEvent as E};
        use crate::sounds::{Sound, take_requests};
        let mut a = DarkBurn::new(crate::prefs::Prefs::default());
        let finished = [E::JobStarted, E::JobFinished(JobProgress::default())];
        let failed = [E::Error("error:9".into()), E::JobAborted("Connection lost".into())];
        take_requests();
        // Off by default: nothing is asked for.
        on_machine_events(&mut a, &finished);
        on_machine_events(&mut a, &failed);
        assert!(take_requests().is_empty());
        a.prefs.opts.sound_done = true;
        on_machine_events(&mut a, &[E::JobStarted]);
        assert!(take_requests().is_empty(), "no sound while the job runs");
        on_machine_events(&mut a, &finished);
        on_machine_events(&mut a, &failed);
        assert_eq!(take_requests(), [Sound::JobDone], "only the finished job, once");
        a.prefs.opts.sound_done = false;
        a.prefs.opts.sound_error = true;
        on_machine_events(&mut a, &finished);
        assert!(take_requests().is_empty());
        on_machine_events(&mut a, &failed);
        assert_eq!(take_requests(), [Sound::Error], "an error that ends the job is one sound");
        for e in [E::Alarm("ALARM:1".into()), E::ConnectFailed("no port".into()), E::Error("error:2".into())] {
            on_machine_events(&mut a, &[e]);
            assert_eq!(take_requests(), [Sound::Error]);
        }
        // Stopping the job yourself is not an error.
        for r in ["Stopped", "stopped", "cancelled"] {
            on_machine_events(&mut a, &[E::JobAborted(r.into())]);
        }
        assert!(take_requests().is_empty());
        on_machine_events(&mut a, &[]);
        assert!(take_requests().is_empty());
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
