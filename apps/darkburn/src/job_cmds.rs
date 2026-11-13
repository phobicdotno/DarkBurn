//! Job commands and state: preview, G-code export, optimization settings, job origin, user
//! origin, material test and framing. Every action is a registered command; the window code in
//! `job_ui.rs` calls these. The commands that touch a machine are registered as `hardware` and
//! answer "no machine" until the machine side connects them.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use darkburn_core::kurbo::{Point, Rect};
use darkburn_job::{FinishPosition, JobAnchor, OrderKey, PlannerSettings, ScanDirection};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::commands::Cmd;
use crate::job_build::{
    self, AdditionalSettings, BuildInput, BuildTask, BuiltJob, DeviceGcodeSettings, FrameKind, JobDevice, JobProjectSettings, StartMode,
};
use crate::material_test::{self, MaterialTestParams, Preset};
use crate::preview::{PreviewData, ViewOptions};
use crate::shell::DarkBurn;

const fn cmd(id: &'static str, label: &'static str, key: &'static str, args: &'static str) -> Cmd {
    Cmd { id, label, key, icon: "", args }
}

/// The commands of this area. `commands::registry` adds them to the registry.
pub const COMMANDS: &[Cmd] = &[
    cmd("job.generate", "Generate Job", "", ""),
    cmd(
        "job.save_gcode",
        "Save G-code to a File",
        "",
        r#"{"type":"object","properties":{"path":{"type":"string"},"from_op":{"type":"integer","description":"start at this operation (Save from here)"},"allow_out_of_bounds":{"type":"boolean"}},"required":["path"]}"#,
    ),
    cmd("job.optimization_settings", "Optimization Settings", "", ""),
    cmd(
        "job.set_optimization",
        "Change Optimization Settings",
        "",
        r#"{"type":"object","properties":{"optimize":{"type":"boolean"},"order_by":{"type":"array","items":{"enum":["layer","group","priority"]}},"inner_first":{"type":"boolean"},"direction_order":{"enum":[null,"bottom_to_top","top_to_bottom","left_to_right","right_to_left"]},"reduce_travel":{"type":"boolean"},"hide_backlash":{"type":"boolean"},"backlash_approach":{"type":"number"},"reduce_direction_changes":{"type":"boolean"},"best_start":{"type":"boolean"},"prefer_corners":{"type":"boolean"},"best_direction":{"type":"boolean"},"remove_overlaps":{"type":"boolean"},"overlap_distance":{"type":"number"}}}"#,
    ),
    cmd(
        "job.optimization_defaults",
        "Optimization Defaults",
        "",
        r#"{"type":"object","properties":{"action":{"enum":["set","reset"],"description":"set: remember the project's settings as the default; reset: put the default back in the project"}},"required":["action"]}"#,
    ),
    cmd(
        "job.set_origin",
        "Change Job Origin and Start From",
        "",
        r#"{"type":"object","properties":{"start_from":{"enum":["absolute","current_position","user_origin"]},"anchor":{"enum":["top_left","top","top_right","left","centre","right","bottom_left","bottom","bottom_right"]},"use_selection_origin":{"type":"boolean"},"cut_selected":{"type":"boolean"},"discard_out_of_bounds":{"type":"boolean"},"show_last_position":{"type":"boolean"},"return_to_start":{"type":"boolean"}}}"#,
    ),
    cmd(
        "job.set_user_origin",
        "Set the User Origin",
        "",
        r#"{"type":"object","properties":{"x":{"type":"number","description":"machine coordinates, mm"},"y":{"type":"number"}},"required":["x","y"]}"#,
    ),
    cmd("job.clear_user_origin", "Clear the User Origin", "", ""),
    cmd("job.gcode_settings", "G-code and Simulation Settings", "", ""),
    cmd(
        "job.set_gcode_settings",
        "Change G-code Settings",
        "",
        r#"{"type":"object","properties":{"flavor":{"enum":["grbl","grblhal","fluidnc","marlin","smoothieware","linuxcnc"]},"s_max":{"type":"number"},"start_script":{"type":"string"},"end_script":{"type":"string"},"auto_home":{"type":"boolean"},"swap_xy":{"type":"boolean"},"overscan_as_g0":{"type":"boolean"},"s_every_g1":{"type":"boolean"},"accel":{"type":"number"},"engrave_accel":{"type":"number"},"rapid_speed":{"type":"number"},"max_speed":{"type":"number"}}}"#,
    ),
    cmd(
        "job.material_test",
        "Generate a Material Test",
        "",
        r#"{"type":"object","properties":{"x_axis":{"enum":["speed","power","interval","frequency","q_pulse"]},"x_min":{"type":"number"},"x_max":{"type":"number"},"columns":{"type":"integer"},"y_axis":{"enum":["speed","power","interval","frequency","q_pulse"]},"y_min":{"type":"number"},"y_max":{"type":"number"},"rows":{"type":"integer"},"cell_width":{"type":"number"},"cell_height":{"type":"number"},"gap":{"type":"number"},"x":{"type":"number"},"y":{"type":"number"},"fill":{"type":"boolean"},"passes":{"type":"integer"},"base_speed":{"type":"number"},"base_power":{"type":"number"},"base_interval":{"type":"number"},"labels":{"type":"boolean"},"label_height":{"type":"number"},"label_layer":{"type":"integer"},"label_speed":{"type":"number"},"label_power":{"type":"number"},"first_layer":{"type":"integer"},"galvo":{"type":"boolean","description":"the galvo variant: small filled cells, speed against pulse frequency, sized for the lens field"},"base_frequency":{"type":"number","description":"kHz, galvo variant"},"base_q_pulse":{"type":"number","description":"ns, galvo variant with a MOPA source"}}}"#,
    ),
    cmd(
        "job.material_test_presets",
        "Material Test Presets",
        "",
        r#"{"type":"object","properties":{"action":{"enum":["list","save","load","delete","export","import"]},"name":{"type":"string"},"path":{"type":"string"}},"required":["action"]}"#,
    ),
    cmd(
        "job.frame_path",
        "Frame Path",
        "",
        r#"{"type":"object","properties":{"kind":{"enum":["rectangle","rubber_band","selection"]},"speed":{"type":"number"},"power":{"type":"number","description":"percent; leave out for laser off"}}}"#,
    ),
    cmd(
        "job.preview_start_here",
        "Start Here (Preview)",
        "",
        r#"{"type":"object","properties":{"op_index":{"type":"integer"}},"required":["op_index"]}"#,
    ),
    cmd(
        "job.preview_save_from_here",
        "Save from Here (Preview)",
        "",
        r#"{"type":"object","properties":{"path":{"type":"string"},"op_index":{"type":"integer"}},"required":["path","op_index"]}"#,
    ),
    cmd(
        "job.preview_save_image",
        "Save Preview Image",
        "",
        r#"{"type":"object","properties":{"path":{"type":"string"},"transparent":{"type":"boolean"},"width":{"type":"integer"},"shade_by_power":{"type":"boolean"},"show_travel":{"type":"boolean"},"invert":{"type":"boolean"}},"required":["path"]}"#,
    ),
    // Machine commands: registered now, connected by the machine side.
    cmd("job.start", "Start", "Alt+S", ""),
    cmd(
        "job.pause",
        "Pause",
        "",
        r#"{"type":"object","properties":{"only_pause":{"type":"boolean","description":"never resume a job that is already paused (left out, the window's button resumes it)"}}}"#,
    ),
    cmd("job.stop", "Stop", "", ""),
    cmd("job.frame", "Frame", "Alt+R", ""),
    cmd("job.frame_rubber", "Frame (rubber band)", "Alt+Shift+R", ""),
    cmd("job.frame_selection", "Frame Selection on the Laser", "", ""),
    cmd("job.send", "Send", "Alt+Shift+S", ""),
    cmd("job.run_gcode", "Run G-code", "", ""),
    cmd("job.home", "Home", "", ""),
    cmd("job.go_origin", "Go to Origin", "", ""),
    cmd(
        "job.preview_run_from_here",
        "Run from Here (Preview)",
        "",
        r#"{"type":"object","properties":{"op_index":{"type":"integer"}},"required":["op_index"]}"#,
    ),
    cmd(
        "job.preview_send_from_here",
        "Send from Here (Preview)",
        "",
        r#"{"type":"object","properties":{"op_index":{"type":"integer"}},"required":["op_index"]}"#,
    ),
];

/// Commands that touch the machine.
pub fn hardware(id: &str) -> bool {
    matches!(
        id,
        "job.start"
            | "job.pause"
            | "job.stop"
            | "job.frame"
            | "job.frame_rubber"
            | "job.frame_selection"
            | "job.send"
            | "job.run_gcode"
            | "job.home"
            | "job.go_origin"
            | "job.preview_run_from_here"
            | "job.preview_send_from_here"
    )
}

/// Commands the generic `laser.` prefix rule would flag but that only edit the document.
pub fn not_hardware(id: &str) -> bool {
    matches!(id, "file.save_gcode" | "laser.material_test" | "laser.material_test_galvo")
}

/// Commands of this area that run in this build.
pub fn works(id: &str) -> bool {
    matches!(id, "window.preview" | "file.save_gcode" | "laser.material_test" | "laser.material_test_galvo")
        || (id.starts_with("job.") && !hardware(id))
}

/// The checklist items a command implements.
pub fn checklist(id: &str) -> &'static [&'static str] {
    match id {
        "window.preview" => &["WIN-02", "PREV-01", "PREV-02", "PREV-03", "PREV-04", "PREV-05", "PREV-06", "PREV-07", "PREV-10"],
        "file.save_gcode" | "job.save_gcode" => &["FILE-09", "EXP-06", "EXP-07", "LASER-06", "RUIDA-13"],
        "job.optimization_settings" | "job.set_optimization" | "job.optimization_defaults" => &["LASER-16", "PLAN-01"],
        "job.set_origin" => &["LASER-11", "LASER-12", "LASER-13", "LASER-14", "LASER-15", "ORIG-01", "ORIG-02", "ORIG-04", "ORIG-05"],
        "job.set_user_origin" | "job.clear_user_origin" => &["ORIG-03"],
        "job.gcode_settings" | "job.set_gcode_settings" => &["DS-09", "DS-10", "DS-11", "DS-12", "DS-13", "DS-14", "DS-15", "DS-19"],
        "laser.material_test" | "job.material_test" | "job.material_test_presets" => &["LT-12", "MT-01", "MT-02", "MT-04", "MT-05", "MT-07"],
        "laser.material_test_galvo" => &["LT-19"],
        "job.preview_start_here" | "job.preview_save_from_here" => &["PREV-09"],
        "job.preview_save_image" => &["PREV-08"],
        "job.frame_path" => &["LASER-04", "LASER-05"],
        _ => &[],
    }
}

// ----- State ----------------------------------------------------------------------------------

/// What happens when a build finishes.
#[derive(Clone, Debug)]
pub enum After {
    Preview,
    SaveGcode {
        path: PathBuf,
        from_op: Option<usize>,
    },
    /// Run or send the job on the connected machine (machine_job.rs).
    Machine(crate::machine_job::Action),
}

/// A job that was sent or saved: where it landed, for "Show last position".
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LastPosition {
    /// Design coordinates.
    pub bounds: Rect,
}

/// A message window.
#[derive(Clone, Debug)]
pub struct Message {
    pub title: String,
    pub text: String,
}

/// The job is out of bounds: ask what to do.
pub struct BoundsPrompt {
    pub job: Arc<BuiltJob>,
    pub then: After,
}

/// The preview window's state.
pub struct PreviewUi {
    pub open: bool,
    pub maximised: bool,
    pub data: Option<Arc<PreviewData>>,
    pub t: f64,
    pub playing: bool,
    pub speed_index: usize,
    pub view: ViewOptions,
    /// Screen points per millimetre and the pan, set when the view is fitted.
    pub zoom: f32,
    pub pan: egui::Vec2,
    pub fitted: bool,
    pub error: Option<String>,
}

impl Default for PreviewUi {
    fn default() -> Self {
        PreviewUi {
            open: false,
            maximised: false,
            data: None,
            t: 0.0,
            playing: false,
            speed_index: 3,
            view: ViewOptions::default(),
            zoom: 1.0,
            pan: egui::Vec2::ZERO,
            fitted: false,
            error: None,
        }
    }
}

/// Playback speeds: simulated seconds per real second.
pub const SPEEDS: [f64; 8] = [1.0, 2.0, 5.0, 10.0, 25.0, 50.0, 100.0, 500.0];

/// Everything about the job that is not in the project.
#[derive(Default)]
pub struct JobState {
    pub device: JobDevice,
    pub gcode: DeviceGcodeSettings,
    pub additional: AdditionalSettings,
    /// The user's default optimization settings (PLAN-11).
    pub default_planner: PlannerSettings,
    /// The user origin in machine coordinates (ORIG-03).
    pub user_origin: Option<Point>,
    /// Where the head is, machine coordinates; the machine side fills this in.
    pub head: Option<Point>,
    pub last_position: Option<LastPosition>,
    /// "Start here": the operation the next start, send or save begins at.
    pub start_op: Option<usize>,
    /// The document revision `start_op` was picked in: the number means nothing in the job of
    /// a changed design (see [`start_op`]).
    pub start_op_revision: u64,
    pub preview: PreviewUi,
    pub pending: Option<(BuildTask, After)>,
    pub pending_started: Option<std::time::Instant>,
    pub bounds_prompt: Option<BoundsPrompt>,
    pub message: Option<Message>,
    pub optimization_open: bool,
    pub optimization_draft: Option<PlannerSettings>,
    pub gcode_open: bool,
    pub gcode_draft: Option<(DeviceGcodeSettings, AdditionalSettings)>,
    pub material_open: bool,
    pub material: MaterialTestParams,
    pub presets: Vec<Preset>,
    pub preset_name: String,
    /// The most recent job built (for the machine side and tests).
    pub last_job: Option<Arc<BuiltJob>>,
    loaded: bool,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct Saved {
    device: JobDevice,
    gcode: DeviceGcodeSettings,
    additional: AdditionalSettings,
    default_planner: PlannerSettings,
    presets: Vec<Preset>,
}

const FILE: &str = "job.json";

impl JobState {
    /// Read the saved device, G-code and default settings the first time the settings folder is known.
    pub fn ensure_loaded(&mut self, dir: Option<&Path>) {
        if self.loaded {
            return;
        }
        self.loaded = true;
        if let Some(dir) = dir
            && let Ok(text) = std::fs::read_to_string(dir.join(FILE))
            && let Ok(s) = serde_json::from_str::<Saved>(&text)
        {
            self.device = s.device;
            self.gcode = s.gcode;
            self.additional = s.additional;
            self.default_planner = s.default_planner;
            self.presets = s.presets;
        }
    }

    pub fn save(&self, dir: Option<&Path>) {
        let Some(dir) = dir else { return };
        let s = Saved {
            device: self.device,
            gcode: self.gcode.clone(),
            additional: self.additional,
            default_planner: self.default_planner.clone(),
            presets: self.presets.clone(),
        };
        if let Ok(text) = serde_json::to_string_pretty(&s) {
            let _ = std::fs::create_dir_all(dir);
            let _ = std::fs::write(dir.join(FILE), text);
        }
    }

    /// The user origin in design coordinates, for drawing.
    pub fn user_origin_design(&self, device: &JobDevice) -> Option<Point> {
        self.user_origin.map(|u| device.geometry().affine().inverse() * u)
    }
}

/// "Start here": the operation the next start, send or save begins at. It is dropped once the
/// design has changed since it was picked, because the new job has other operations.
pub fn start_op(app: &mut DarkBurn) -> Option<usize> {
    if app.job.start_op.is_some() && app.job.start_op_revision != app.editor.revision() {
        app.job.start_op = None;
    }
    app.job.start_op
}

/// The job settings saved in the open project (or the user's defaults).
pub fn current_settings(app: &DarkBurn) -> JobProjectSettings {
    JobProjectSettings::from_doc(&app.editor.doc, &app.job.default_planner)
}

/// Change the project's job settings as one undo step. Returns whether anything changed.
pub fn update_settings(app: &mut DarkBurn, label: &str, f: impl FnOnce(&mut JobProjectSettings)) -> bool {
    let before = current_settings(app);
    let mut after = before.clone();
    f(&mut after);
    if after == before {
        return false;
    }
    app.editor.edit(label, |d, _| {
        after.store(d);
        true
    })
}

/// Everything a job build needs, from the open project and the app's device state.
pub fn build_input(app: &DarkBurn) -> BuildInput {
    // The selected device profile decides the bed, origin corner, axes and Z.
    let mut device = app.machine.active().job_device();
    device.swap_xy |= app.job.gcode.swap_xy;
    if app.machine.active().family() == crate::machine_api::Family::Grbl && app.job.gcode.fast_whitespace_scan {
        device.fast_whitespace = Some(app.job.gcode.fast_whitespace_speed);
    }
    device.tolerance = crate::settings_wiring::output_tolerance_mm(&app.prefs.opts);
    let mut settings = current_settings(app).with_device_finish(app.job.gcode.return_to_finish);
    settings.discard_out_of_bounds = crate::settings_wiring::discard_out_of_bounds(&settings, &app.prefs.opts);
    BuildInput {
        doc: app.editor.doc.clone(),
        selection: app.editor.selected(),
        settings,
        device,
        user_origin: app.job.user_origin,
        head: app.job.head,
        motion: app.job.additional,
        tuning: None,
    }
}

/// Where the job lands in design coordinates, for "Show last position".
pub fn placed_bounds(job: &BuiltJob, input: &BuildInput) -> Option<Rect> {
    let geometry = input.device.geometry();
    let anchor = job.origin.anchor_point(job.bounds);
    let machine_anchor = match job.origin.start_from {
        darkburn_job::StartFrom::Absolute => return Some(job.bounds),
        darkburn_job::StartFrom::UserOrigin(u) => u,
        darkburn_job::StartFrom::CurrentPosition => input.head?,
    };
    let at = geometry.affine().inverse() * machine_anchor;
    Some(job.bounds + (at - anchor))
}

/// Remember `job` as the last one sent or saved. The machine side calls this when it starts a job.
pub fn record_last_job(app: &mut DarkBurn, job: &Arc<BuiltJob>) {
    let input = build_input(app);
    app.job.last_position = placed_bounds(job, &input).map(|bounds| LastPosition { bounds });
    app.job.last_job = Some(job.clone());
}

// ----- Running --------------------------------------------------------------------------------

fn no_machine(id: &str) -> String {
    format!("{id} needs a machine, and no machine is connected. Connect a device from Devices first.")
}

/// Start building for the window flow (progress and cancel in the UI).
pub fn start_build(app: &mut DarkBurn, then: After) {
    if app.job.pending.is_some() {
        return;
    }
    app.job.pending = Some((job_build::spawn_build(build_input(app)), then));
    app.job.pending_started = Some(std::time::Instant::now());
}

/// Open the preview and build the job.
pub fn open_preview(app: &mut DarkBurn) {
    app.job.preview.open = true;
    app.job.preview.error = None;
    app.job.preview.t = 0.0;
    app.job.preview.playing = false;
    app.job.preview.fitted = false;
    start_build(app, After::Preview);
}

/// Write the G-code of `job` to `path`. `from_op` starts part-way (Save from here).
pub fn write_gcode(app: &mut DarkBurn, job: &Arc<BuiltJob>, path: &Path, from_op: Option<usize>) -> Result<usize, String> {
    let text = crate::gcode_machines::job_text(app.machine.active(), job, from_op, &app.job.gcode);
    let text = crate::gcode_machines::with_finish_move(app.machine.active(), text, app.machine.jog.finish);
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, &text).map_err(|e| format!("{}: {e}", path.display()))?;
    record_last_job(app, job);
    Ok(text.lines().count())
}

/// What Save machine file wrote.
#[derive(Clone, Debug, PartialEq)]
pub struct SavedFile {
    /// "G-code" or "RD file".
    pub what: &'static str,
    /// Lines of G-code; 0 for a binary file.
    pub lines: usize,
    pub bytes: usize,
}

/// Save the machine file the active device takes (FILE-09, LASER-06): G-code for G-code
/// machines, an `.rd` job for Ruida controllers. Trocen and TopWisdom files are refused with
/// the reason.
pub fn write_machine_file(app: &mut DarkBurn, job: &Arc<BuiltJob>, path: &Path, from_op: Option<usize>) -> Result<SavedFile, String> {
    if let Some(why) = crate::machine_job::machine_file_unsupported(app.machine.active()) {
        return Err(why);
    }
    if !crate::machine_job::machine_file_kind(app.machine.active()).rd {
        let lines = write_gcode(app, job, path, from_op)?;
        let bytes = std::fs::metadata(path).map(|m| m.len() as usize).unwrap_or(0);
        return Ok(SavedFile { what: "G-code", lines, bytes });
    }
    let data = crate::machine_job::rd_file_bytes(app, job, from_op)?;
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, &data).map_err(|e| format!("{}: {e}", path.display()))?;
    record_last_job(app, job);
    Ok(SavedFile { what: "RD file", lines: 0, bytes: data.len() })
}

/// Handle a finished build in the window flow.
pub fn finish(app: &mut DarkBurn, job: Arc<BuiltJob>, then: After) {
    app.job.last_job = Some(job.clone());
    match then {
        After::Preview => {
            let rev = app.editor.revision();
            let data = PreviewData::new(job, &app.job.additional, rev);
            // Show the whole job first; Restart plays it from the start.
            app.job.preview.t = data.stats.seconds;
            app.job.preview.data = Some(Arc::new(data));
            app.job.preview.fitted = false;
        }
        After::SaveGcode { path, from_op } => {
            if job.out_of_bounds() {
                app.job.bounds_prompt = Some(BoundsPrompt { job, then: After::SaveGcode { path, from_op } });
                return;
            }
            save_and_report(app, &job, &path, from_op);
        }
        After::Machine(action) => {
            if job.out_of_bounds() {
                app.job.bounds_prompt = Some(BoundsPrompt { job, then: After::Machine(action) });
                return;
            }
            run_on_machine(app, job, action);
        }
    }
}

fn run_on_machine(app: &mut DarkBurn, job: Arc<BuiltJob>, action: crate::machine_job::Action) {
    if let Err(e) = crate::machine_job::after_build(app, job, action) {
        app.job.message = Some(Message { title: "Cannot run the job".into(), text: e });
    }
}

fn save_and_report(app: &mut DarkBurn, job: &Arc<BuiltJob>, path: &Path, from_op: Option<usize>) {
    match write_machine_file(app, job, path, from_op) {
        Ok(saved) => {
            let mut text = if saved.lines > 0 {
                format!("Wrote {} lines to {}.", saved.lines, path.display())
            } else {
                format!("Wrote {} bytes to {}.", saved.bytes, path.display())
            };
            for w in &job.warnings {
                text.push_str(&format!("\n{w}"));
            }
            app.job.message = Some(Message { title: format!("{} saved", saved.what), text });
        }
        Err(e) => app.job.message = Some(Message { title: "Could not save the machine file".into(), text: e }),
    }
}

/// The out-of-bounds prompt was answered. `discard`: build again without the shapes outside.
pub fn answer_bounds_prompt(app: &mut DarkBurn, go: bool, discard: bool) {
    let Some(p) = app.job.bounds_prompt.take() else { return };
    if discard {
        update_settings(app, "Discard out-of-bounds shapes", |s| s.discard_out_of_bounds = true);
        start_build(app, p.then);
    } else if go {
        match p.then {
            After::SaveGcode { path, from_op } => save_and_report(app, &p.job, &path, from_op),
            After::Machine(action) => run_on_machine(app, p.job, action),
            After::Preview => {}
        }
    }
}

/// A command from a menu, button or key. Returns whether it was one of this area's.
pub fn run(app: &mut DarkBurn, _ctx: &egui::Context, id: &str) -> bool {
    match id {
        "window.preview" => open_preview(app),
        "file.save_gcode" => {
            // The file follows the device: G-code, or an .rd job for a Ruida controller.
            if let Some(why) = crate::machine_job::machine_file_unsupported(app.machine.active()) {
                app.job.message = Some(Message { title: "Could not save the machine file".into(), text: why });
                return true;
            }
            let kind = crate::machine_job::machine_file_kind(app.machine.active());
            let ext = kind.extensions.first().copied().unwrap_or("gc");
            let name = app
                .project_path
                .as_deref()
                .and_then(Path::file_stem)
                .map(|n| format!("{}.{ext}", n.to_string_lossy()))
                .unwrap_or_else(|| format!("job.{ext}"));
            let Some(path) = rfd::FileDialog::new().add_filter(kind.filter, kind.extensions).set_file_name(name).save_file() else {
                return true;
            };
            let from_op = start_op(app);
            start_build(app, After::SaveGcode { path, from_op });
        }
        "laser.material_test" => app.job.material_open = true,
        "laser.material_test_galvo" => {
            // The same window, started on the galvo variant's values.
            if !app.job.material.galvo {
                app.job.material = MaterialTestParams::galvo_defaults(app.machine.active().bed());
            }
            app.job.material_open = true;
        }
        "job.optimization_settings" => {
            app.job.optimization_open = true;
            app.job.optimization_draft = None;
        }
        "job.gcode_settings" => {
            app.job.gcode_open = true;
            app.job.gcode_draft = None;
        }
        _ if hardware(id) => {
            app.job.message = Some(Message { title: "No machine".into(), text: no_machine(id) });
        }
        _ if id.starts_with("job.") && works(id) => {
            if let Err(e) = execute(app, id, &Value::Null).unwrap_or(Ok(Value::Null)) {
                app.job.message = Some(Message { title: "Cannot do that".into(), text: e });
            }
        }
        _ => return false,
    }
    true
}

// ----- Headless commands ---------------------------------------------------------------------

fn num(a: &Value, key: &str) -> Option<f64> {
    a.get(key).and_then(Value::as_f64).filter(|v| v.is_finite())
}

fn flag(a: &Value, key: &str) -> Result<Option<bool>, String> {
    match a.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_bool().map(Some).ok_or_else(|| format!("{key} must be true or false")),
    }
}

fn text<'a>(a: &'a Value, key: &str) -> Result<&'a str, String> {
    a.get(key).and_then(Value::as_str).ok_or_else(|| format!("argument {key} must be text"))
}

fn index(a: &Value, key: &str) -> Result<usize, String> {
    a.get(key).and_then(Value::as_u64).map(|v| v as usize).ok_or_else(|| format!("argument {key} must be a whole number"))
}

fn anchor_of(name: &str) -> Result<JobAnchor, String> {
    Ok(match name {
        "top_left" => JobAnchor::TopLeft,
        "top" => JobAnchor::Top,
        "top_right" => JobAnchor::TopRight,
        "left" => JobAnchor::Left,
        "centre" | "center" => JobAnchor::Center,
        "right" => JobAnchor::Right,
        "bottom_left" => JobAnchor::BottomLeft,
        "bottom" => JobAnchor::Bottom,
        "bottom_right" => JobAnchor::BottomRight,
        other => return Err(format!("unknown anchor {other}")),
    })
}

/// Names for the 9 anchors in picker order (top row first).
pub const ANCHORS: [JobAnchor; 9] = [
    JobAnchor::TopLeft,
    JobAnchor::Top,
    JobAnchor::TopRight,
    JobAnchor::Left,
    JobAnchor::Center,
    JobAnchor::Right,
    JobAnchor::BottomLeft,
    JobAnchor::Bottom,
    JobAnchor::BottomRight,
];

fn apply_planner(p: &mut PlannerSettings, a: &Value) -> Result<(), String> {
    let set = |key: &str, field: &mut bool| -> Result<(), String> {
        if let Some(v) = flag(a, key)? {
            *field = v;
        }
        Ok(())
    };
    set("optimize", &mut p.optimize)?;
    set("inner_first", &mut p.inner_first)?;
    set("reduce_travel", &mut p.reduce_travel)?;
    set("hide_backlash", &mut p.hide_backlash)?;
    set("reduce_direction_changes", &mut p.reduce_direction_changes)?;
    set("best_start", &mut p.best_start)?;
    set("prefer_corners", &mut p.prefer_corners)?;
    set("best_direction", &mut p.best_direction)?;
    set("remove_overlaps", &mut p.remove_overlaps)?;
    if let Some(v) = num(a, "backlash_approach") {
        p.backlash_approach = v.clamp(0.0, 50.0);
    }
    if let Some(v) = num(a, "overlap_distance") {
        p.overlap_distance = v.clamp(0.0, 10.0);
    }
    if let Some(list) = a.get("order_by").and_then(Value::as_array) {
        let mut order = Vec::new();
        for v in list {
            let k = match v.as_str() {
                Some("layer") => OrderKey::Layer,
                Some("group") => OrderKey::Group,
                Some("priority") => OrderKey::Priority,
                _ => return Err("order_by holds layer, group or priority".into()),
            };
            if !order.contains(&k) {
                order.push(k);
            }
        }
        p.order_by = order;
    }
    if let Some(v) = a.get("direction_order") {
        p.direction_order = match v.as_str() {
            None if v.is_null() => None,
            Some("bottom_to_top") => Some(ScanDirection::BottomToTop),
            Some("top_to_bottom") => Some(ScanDirection::TopToBottom),
            Some("left_to_right") => Some(ScanDirection::LeftToRight),
            Some("right_to_left") => Some(ScanDirection::RightToLeft),
            _ => return Err("unknown direction_order".into()),
        };
    }
    Ok(())
}

fn flavor_of(name: &str) -> Result<usize, String> {
    Ok(match name {
        "grbl" => 0,
        "grblhal" => 1,
        "fluidnc" => 2,
        "marlin" => 3,
        "smoothieware" => 4,
        "linuxcnc" => 5,
        other => return Err(format!("unknown flavor {other}")),
    })
}

fn params_from(a: &Value, base: &MaterialTestParams, field: (f64, f64)) -> Result<MaterialTestParams, String> {
    let mut p = match flag(a, "galvo")? {
        // The galvo variant starts from its own values unless the dialog is already on it.
        Some(true) if !base.galvo => MaterialTestParams::galvo_defaults(field),
        Some(false) if base.galvo => MaterialTestParams::default(),
        _ => base.clone(),
    };
    let axis = |key: &str, field: &mut material_test::Axis| -> Result<(), String> {
        if let Some(n) = a.get(key).and_then(Value::as_str) {
            *field = material_test::Axis::from_name(n).ok_or_else(|| format!("unknown axis {n}"))?;
        }
        Ok(())
    };
    axis("x_axis", &mut p.x_axis)?;
    axis("y_axis", &mut p.y_axis)?;
    let f = |key: &str, field: &mut f64| {
        if let Some(v) = num(a, key) {
            *field = v;
        }
    };
    f("x_min", &mut p.x_min);
    f("x_max", &mut p.x_max);
    f("y_min", &mut p.y_min);
    f("y_max", &mut p.y_max);
    f("cell_width", &mut p.cell_w);
    f("cell_height", &mut p.cell_h);
    f("gap", &mut p.gap);
    f("x", &mut p.x);
    f("y", &mut p.y);
    f("base_speed", &mut p.base_speed);
    f("base_power", &mut p.base_power);
    f("base_interval", &mut p.base_interval);
    f("base_frequency", &mut p.base_frequency);
    f("base_q_pulse", &mut p.base_q_pulse);
    f("label_height", &mut p.label_height);
    f("label_speed", &mut p.label_speed);
    f("label_power", &mut p.label_power);
    let u = |key: &str, field: &mut usize| {
        if let Some(v) = a.get(key).and_then(Value::as_u64) {
            *field = v as usize;
        }
    };
    u("columns", &mut p.columns);
    u("rows", &mut p.rows);
    u("label_layer", &mut p.label_layer);
    u("first_layer", &mut p.first_layer);
    if let Some(v) = a.get("passes").and_then(Value::as_u64) {
        p.passes = v.clamp(1, 1000) as u32;
    }
    if let Some(v) = flag(a, "fill")? {
        p.fill = v;
    }
    if let Some(v) = flag(a, "labels")? {
        p.labels = v;
    }
    Ok(p)
}

/// Generate the material test into the project as one undo step.
pub fn add_material_test(app: &mut DarkBurn, p: &MaterialTestParams) -> Result<material_test::MaterialTestResult, String> {
    material_test::validate(p)?;
    let mut result = None;
    app.editor.edit("Material test", |d, sel| match material_test::generate(d, p) {
        Ok(r) => {
            sel.set(r.shapes.iter().copied());
            result = Some(r);
            true
        }
        Err(_) => false,
    });
    result.ok_or_else(|| "the material test could not be made".to_string())
}

fn save_state(app: &DarkBurn) {
    app.job.save(app.settings_dir.as_deref());
}

pub(crate) fn summary(app: &DarkBurn, job: &BuiltJob) -> Value {
    let sim = job.simulate(&app.job.additional);
    let stats = job_build::JobStats::of(job, &sim);
    json!({
        "ops": stats.ops,
        "seconds": stats.seconds,
        "time": job_build::format_time(stats.seconds),
        "cut_mm": stats.cut_mm,
        "travel_mm": stats.travel_mm,
        "scan_mm": stats.scan_mm,
        "bounds": [job.bounds.x0, job.bounds.y0, job.bounds.x1, job.bounds.y1],
        "layers": job.toolpath.layers.len(),
        "warnings": job.warnings,
        "out_of_bounds": job.out_of_bounds(),
        "dropped_shapes": job.dropped_shapes,
    })
}

pub fn build_now(app: &DarkBurn) -> Result<Arc<BuiltJob>, String> {
    job_build::build_job_simple(&build_input(app)).map(Arc::new).map_err(|e| e.to_string())
}

/// Run one of this area's commands with JSON arguments. None when `id` is not one of them.
pub fn execute(app: &mut DarkBurn, id: &str, a: &Value) -> Option<Result<Value, String>> {
    let a = if a.is_null() { &json!({}) } else { a };
    Some(match id {
        _ if hardware(id) => Err(no_machine(id)),
        "window.preview" => {
            open_preview(app);
            Ok(json!({ "open": true }))
        }
        "job.generate" => build_now(app).map(|j| {
            app.job.last_job = Some(j.clone());
            summary(app, &j)
        }),
        "job.save_gcode" | "file.save_gcode" => (|| {
            let path = PathBuf::from(text(a, "path")?);
            let from_op = if a.get("from_op").is_some() { Some(index(a, "from_op")?) } else { start_op(app) };
            let job = build_now(app)?;
            if job.out_of_bounds() && !flag(a, "allow_out_of_bounds")?.unwrap_or(false) {
                return Err(format!(
                    "the job leaves the work area ({}); pass allow_out_of_bounds or turn on discard_out_of_bounds",
                    job.warnings.last().cloned().unwrap_or_default()
                ));
            }
            if let Some(op) = from_op
                && op > job.toolpath.ops.len()
            {
                return Err(format!("op_index {op} is past the end of the job ({} operations)", job.toolpath.ops.len()));
            }
            let saved = write_machine_file(app, &job, &path, from_op)?;
            Ok(json!({
                "path": path.display().to_string(),
                "format": if saved.lines > 0 { "gcode" } else { "rd" },
                "lines": saved.lines,
                "bytes": saved.bytes,
                "from_op": from_op,
                "warnings": job.warnings,
            }))
        })(),
        "job.optimization_settings" | "job.gcode_settings" => {
            app.job.optimization_open |= id == "job.optimization_settings";
            app.job.gcode_open |= id == "job.gcode_settings";
            Ok(json!({ "open": true }))
        }
        "job.set_optimization" => (|| {
            let mut p = current_settings(app).planner;
            apply_planner(&mut p, a)?;
            let changed = update_settings(app, "Optimization settings", |s| s.planner = p);
            Ok(json!({ "changed": changed }))
        })(),
        "job.optimization_defaults" => (|| match text(a, "action")? {
            "set" => {
                app.job.default_planner = current_settings(app).planner;
                save_state(app);
                Ok(json!({ "ok": true }))
            }
            "reset" => {
                let d = app.job.default_planner.clone();
                let changed = update_settings(app, "Reset optimization settings", |s| s.planner = d);
                Ok(json!({ "changed": changed }))
            }
            other => Err(format!("unknown action {other}")),
        })(),
        "job.set_origin" => (|| {
            let start = match a.get("start_from").and_then(Value::as_str) {
                Some(n) => Some(StartMode::from_name(n).ok_or_else(|| format!("unknown start_from {n}"))?),
                None => None,
            };
            let anchor = match a.get("anchor").and_then(Value::as_str) {
                Some(n) => Some(anchor_of(n)?),
                None => None,
            };
            let (sel, cut, discard, last, ret) = (
                flag(a, "use_selection_origin")?,
                flag(a, "cut_selected")?,
                flag(a, "discard_out_of_bounds")?,
                flag(a, "show_last_position")?,
                flag(a, "return_to_start")?,
            );
            let changed = update_settings(app, "Job origin", |s| {
                if let Some(v) = start {
                    s.start_from = v;
                }
                if let Some(v) = anchor {
                    s.anchor = v;
                }
                if let Some(v) = sel {
                    s.use_selection_origin = v;
                }
                if let Some(v) = cut {
                    s.cut_selected = v;
                }
                if let Some(v) = discard {
                    s.discard_out_of_bounds = v;
                }
                if let Some(v) = last {
                    s.show_last_position = v;
                }
                if let Some(v) = ret {
                    s.finish = if v { FinishPosition::StartPosition } else { FinishPosition::Stay };
                }
            });
            Ok(json!({ "changed": changed }))
        })(),
        "job.set_user_origin" => (|| {
            let (x, y) = (num(a, "x").ok_or("argument x must be a number")?, num(a, "y").ok_or("argument y must be a number")?);
            app.job.user_origin = Some(Point::new(x, y));
            Ok(json!({ "user_origin": [x, y] }))
        })(),
        "job.clear_user_origin" => {
            let had = app.job.user_origin.take().is_some();
            Ok(json!({ "cleared": had }))
        }
        "job.set_gcode_settings" => (|| {
            let g = &mut app.job.gcode;
            if let Some(n) = a.get("flavor").and_then(Value::as_str) {
                g.set_flavor_index(flavor_of(n)?);
            }
            if let Some(v) = num(a, "s_max") {
                g.s_max = v.clamp(1.0, 1_000_000.0);
            }
            if let Some(v) = a.get("start_script").and_then(Value::as_str) {
                g.start_script = v.to_string();
            }
            if let Some(v) = a.get("end_script").and_then(Value::as_str) {
                g.end_script = v.to_string();
            }
            if let Some(v) = flag(a, "auto_home")? {
                g.auto_home = v;
            }
            if let Some(v) = flag(a, "swap_xy")? {
                g.swap_xy = v;
            }
            if let Some(v) = flag(a, "overscan_as_g0")? {
                g.overscan_as_g0 = v;
            }
            if let Some(v) = flag(a, "s_every_g1")? {
                g.s_every_g1 = v;
            }
            let m = &mut app.job.additional;
            for (key, field) in [
                ("accel", &mut m.accel),
                ("engrave_accel", &mut m.engrave_accel),
                ("rapid_speed", &mut m.rapid_speed),
                ("max_speed", &mut m.max_speed),
            ] {
                if let Some(v) = num(a, key) {
                    *field = v.clamp(1.0, 1_000_000.0);
                }
            }
            save_state(app);
            Ok(json!({ "ok": true }))
        })(),
        "job.material_test" => (|| {
            let p = params_from(a, &app.job.material, app.machine.active().bed())?;
            material_test::check_for_device(&p, app.machine.active(), None)?;
            let r = add_material_test(app, &p)?;
            let warnings = material_test::check_for_device(&p, app.machine.active(), Some(r.bounds))?;
            app.job.material = p;
            Ok(json!({
                "shapes": r.shapes.iter().map(|i| i.0).collect::<Vec<_>>(),
                "layers": r.layers,
                "bounds": [r.bounds.x0, r.bounds.y0, r.bounds.x1, r.bounds.y1],
                "galvo": app.job.material.galvo,
                "warnings": warnings,
            }))
        })(),
        "job.material_test_presets" => (|| match text(a, "action")? {
            "list" => Ok(json!({ "presets": app.job.presets.iter().map(|p| p.name.clone()).collect::<Vec<_>>() })),
            "save" => {
                let name = text(a, "name")?.trim().to_string();
                if name.is_empty() {
                    return Err("a preset needs a name".into());
                }
                let p = Preset { name, params: app.job.material.clone() };
                material_test::merge_presets(&mut app.job.presets, vec![p]);
                save_state(app);
                Ok(json!({ "count": app.job.presets.len() }))
            }
            "load" => {
                let name = text(a, "name")?;
                let p = app.job.presets.iter().find(|p| p.name == name).ok_or_else(|| format!("no preset called {name}"))?;
                app.job.material = p.params.clone();
                Ok(json!({ "ok": true }))
            }
            "delete" => {
                let name = text(a, "name")?;
                let before = app.job.presets.len();
                app.job.presets.retain(|p| p.name != name);
                save_state(app);
                Ok(json!({ "deleted": before - app.job.presets.len() }))
            }
            "export" => {
                let path = text(a, "path")?;
                std::fs::write(path, material_test::presets_to_json(&app.job.presets)).map_err(|e| e.to_string())?;
                Ok(json!({ "exported": app.job.presets.len() }))
            }
            "import" => {
                let t = std::fs::read_to_string(text(a, "path")?).map_err(|e| e.to_string())?;
                let n = material_test::merge_presets(&mut app.job.presets, material_test::presets_from_json(&t)?);
                save_state(app);
                Ok(json!({ "imported": n }))
            }
            other => Err(format!("unknown action {other}")),
        })(),
        "job.frame_path" => (|| {
            let kind = match a.get("kind").and_then(Value::as_str).unwrap_or("rectangle") {
                "rectangle" => FrameKind::Rectangle,
                "rubber_band" => FrameKind::RubberBand,
                "selection" => FrameKind::Selection,
                other => return Err(format!("unknown frame kind {other}")),
            };
            let tp = job_build::build_frame(&build_input(app), kind, num(a, "speed").unwrap_or(100.0), num(a, "power")).map_err(|e| e.to_string())?;
            let pts: Vec<[f64; 2]> = tp.ops.iter().filter_map(|o| o.target()).map(|p| [p.x, p.y]).collect();
            Ok(json!({ "points": pts, "space": format!("{:?}", tp.space) }))
        })(),
        "job.preview_start_here" => (|| {
            let op = index(a, "op_index")?;
            app.job.start_op = Some(op);
            app.job.start_op_revision = app.editor.revision();
            Ok(json!({ "start_op": op }))
        })(),
        "job.preview_save_from_here" => {
            let mut b = a.clone();
            if let Some(obj) = b.as_object_mut() {
                obj.insert("from_op".into(), a.get("op_index").cloned().unwrap_or(Value::Null));
            }
            return execute(app, "job.save_gcode", &b);
        }
        "job.preview_save_image" => (|| {
            let path = text(a, "path")?;
            let job = build_now(app)?;
            let data = PreviewData::new(job, &app.job.additional, app.editor.revision());
            let mut view = app.job.preview.view;
            for (key, field) in [("shade_by_power", &mut view.shade_by_power), ("show_travel", &mut view.show_travel), ("invert", &mut view.invert)] {
                if let Some(v) = flag(a, key)? {
                    *field = v;
                }
            }
            let bed = build_input(app).device.area();
            let width = a.get("width").and_then(Value::as_u64).unwrap_or(1600) as u32;
            let png = crate::preview::render_png(&data, bed, width, &view, flag(a, "transparent")?.unwrap_or(false), None)?;
            std::fs::write(path, &png).map_err(|e| e.to_string())?;
            Ok(json!({ "path": path, "bytes": png.len() }))
        })(),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::execute_headless;
    use crate::prefs::Prefs;

    fn app() -> DarkBurn {
        DarkBurn::new(Prefs::default())
    }

    fn run(app: &mut DarkBurn, id: &str, args: Value) -> Result<Value, String> {
        execute_headless(app, id, &args)
    }

    fn with_square(app: &mut DarkBurn) {
        run(app, "shape.add_rect", json!({ "x": 20, "y": 20, "width": 30, "height": 20, "layer": 0 })).unwrap();
        run(app, "layer.set_param", json!({ "layer": 0, "field": "speed", "value": 30 })).unwrap();
        run(app, "layer.set_param", json!({ "layer": 0, "field": "max_power", "value": 40 })).unwrap();
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("darkburn-job-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn generate_and_save_gcode_headless() {
        let mut a = app();
        assert!(run(&mut a, "job.generate", Value::Null).unwrap_err().contains("nothing to burn"));
        with_square(&mut a);
        let r = run(&mut a, "job.generate", Value::Null).unwrap();
        assert!(r["seconds"].as_f64().unwrap() > 0.0);
        assert!((r["cut_mm"].as_f64().unwrap() - 100.0).abs() < 1.0);
        assert_eq!(r["bounds"], json!([20.0, 20.0, 50.0, 40.0]));
        let dir = temp("save");
        let file = dir.join("square.gc");
        run(&mut a, "job.set_gcode_settings", json!({ "s_max": 255, "start_script": "G54" })).unwrap();
        let r = run(&mut a, "file.save_gcode", json!({ "path": file.to_string_lossy() })).unwrap();
        assert!(r["lines"].as_u64().unwrap() > 8);
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("G54"), "{text}");
        assert!(text.contains("S102"), "40 % of 255: {text}");
        assert!(text.contains("F1800"), "30 mm/s: {text}");
        // Golden: the square is one rapid, one move that sets feed and power, then three more cuts.
        let body: Vec<&str> = text.lines().skip_while(|l| !l.starts_with("M8")).collect();
        assert_eq!(body, ["M8", "G0 X20 Y20", "G1 X50 F1800 S102", "G1 Y40", "G1 X20", "G1 Y20", "M9", "M5", "M2"]);
        assert!(a.job.last_position.is_some(), "saving remembers where the job landed");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The job builder ends its own G-code with M2. A translated device writes its own end
    /// (script, laser off, program end), so that M2 must not sit in front of it.
    #[test]
    fn a_translated_file_ends_once_and_after_the_end_script() {
        let mut a = app();
        with_square(&mut a);
        let dir = temp("ends");
        let file = dir.join("job.gc");
        let saved = |a: &mut DarkBurn, ty: &str, end_script: Option<&str>| -> Vec<String> {
            let id = run(a, "device.add", json!({ "type": ty })).unwrap()["id"].as_u64().unwrap();
            if let Some(e) = end_script {
                a.machine.devices.get_mut(id).unwrap().settings.gcode.end_script = e.into();
            }
            run(a, "device.select", json!({ "id": id })).unwrap();
            run(a, "job.save_gcode", json!({ "path": file.to_string_lossy() })).unwrap();
            std::fs::read_to_string(&file).unwrap().lines().map(str::to_owned).collect()
        };
        // LinuxCNC stops reading at the first M2: an end script after it would never run.
        let lines = saved(&mut a, "LinuxCnc", Some("G0 X0 Y0"));
        let ends: Vec<usize> = lines.iter().enumerate().filter(|(_, l)| *l == "M2").map(|(i, _)| i).collect();
        assert_eq!(ends, [lines.len() - 1], "one program end, as the last line: {lines:?}");
        let park = lines.iter().position(|l| l == "G0 X0 Y0").unwrap_or_else(|| panic!("no end script: {lines:?}"));
        assert!(park > lines.iter().rposition(|l| l.starts_with("G1")).unwrap(), "{lines:?}");
        // The device as it is made: still one program end.
        let lines = saved(&mut a, "LinuxCnc", None);
        assert_eq!(lines.iter().filter(|l| *l == "M2").count(), 1, "{lines:?}");
        assert_eq!(lines.last().map(String::as_str), Some("M2"), "{lines:?}");
        // Marlin and Smoothieware have no M2.
        for ty in ["Marlin", "Smoothie"] {
            let lines = saved(&mut a, ty, None);
            assert!(!lines.iter().any(|l| l == "M2"), "{ty}: {lines:?}");
            assert!(lines.iter().any(|l| l.starts_with("G1")), "{ty}: {lines:?}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Inches on the screen is how sizes are shown, not what the machine is sent. The translated
    /// devices used to get a job in inches (G20), which a Marlin as it is usually built does not
    /// know: it would read the numbers as millimetres.
    #[test]
    fn the_unit_on_the_screen_does_not_change_the_g_code() {
        let mut a = app();
        with_square(&mut a);
        run(&mut a, "job.generate", Value::Null).unwrap();
        let job = a.job.last_job.clone().expect("a job");
        use crate::devices::DeviceType;
        for ty in [DeviceType::Grbl, DeviceType::Marlin, DeviceType::Smoothie, DeviceType::CustomGcode, DeviceType::LinuxCnc, DeviceType::Snapmaker] {
            let mut dev = crate::devices::Device::new(ty);
            let mm = crate::gcode_machines::job_text(&dev, &job, None, &dev.settings.gcode);
            dev.state.inches = true;
            let inches = crate::gcode_machines::job_text(&dev, &job, None, &dev.settings.gcode);
            assert!(mm.contains("X50"), "{ty:?}: {mm}");
            assert_eq!(inches, mm, "{ty:?}: the job changed with the unit shown on the screen");
            assert!(!inches.lines().any(|l| l.split_whitespace().any(|w| w == "G20")), "{ty:?}: {inches}");
        }
    }

    #[test]
    fn the_finish_position_of_the_move_panel_reaches_a_saved_file() {
        let mut a = app();
        with_square(&mut a);
        let dir = temp("finish");
        let file = dir.join("job.gc");
        let tail = |a: &mut DarkBurn| -> Vec<String> {
            run(a, "job.save_gcode", json!({ "path": file.to_string_lossy() })).unwrap();
            let text = std::fs::read_to_string(&file).unwrap();
            let lines: Vec<String> = text.lines().map(str::to_owned).collect();
            lines[lines.len() - 3..].to_vec()
        };
        assert_eq!(tail(&mut a), ["M9", "M5", "M2"], "no finish position: the file ends where the job ends");
        a.machine.jog.finish = Some((350.0, 12.5));
        // After the laser is off, in machine coordinates as a started job sends it, before the program ends.
        assert_eq!(tail(&mut a), ["M5", "G90 G53 G0 X350 Y12.5", "M2"]);

        // A Marlin file (no G53 there): an absolute move, as its driver sends it at the end of a job.
        let id = run(&mut a, "device.add", json!({ "type": "Marlin" })).unwrap()["id"].as_u64().unwrap();
        run(&mut a, "device.select", json!({ "id": id })).unwrap();
        a.machine.jog.finish = Some((100.0, 20.0));
        let _ = tail(&mut a);
        let text = std::fs::read_to_string(&file).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        let go = lines.iter().position(|l| *l == "G0 X100 Y20").unwrap_or_else(|| panic!("no finish move: {text}"));
        assert_eq!(lines[go - 1], "G90", "{text}");
        assert!(go > lines.iter().rposition(|l| l.starts_with("G1")).unwrap(), "after the last cut: {text}");

        // A LinuxCNC program stops at its first M2: the move comes before it.
        let id = run(&mut a, "device.add", json!({ "type": "LinuxCnc" })).unwrap()["id"].as_u64().unwrap();
        run(&mut a, "device.select", json!({ "id": id })).unwrap();
        a.machine.jog.finish = Some((100.0, 20.0));
        let _ = tail(&mut a);
        let text = std::fs::read_to_string(&file).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        let go = lines.iter().position(|l| *l == "G0 X100 Y20").unwrap_or_else(|| panic!("no finish move: {text}"));
        assert!(go < lines.iter().position(|l| *l == "M2").unwrap(), "{text}");
        assert!(go > lines.iter().rposition(|l| l.starts_with("G1")).unwrap(), "{text}");

        // A Ruida file: one last move with the laser off.
        let id = run(&mut a, "device.add", json!({ "type": "Ruida" })).unwrap()["id"].as_u64().unwrap();
        run(&mut a, "device.select", json!({ "id": id })).unwrap();
        let rd = dir.join("job.rd");
        let last_op = |a: &mut DarkBurn| {
            run(a, "job.save_gcode", json!({ "path": rd.to_string_lossy() })).unwrap();
            let job = darkburn_ruida::RdJob::from_rd_file(&std::fs::read(&rd).unwrap(), None).unwrap();
            job.layers.last().unwrap().ops.last().cloned().unwrap()
        };
        a.machine.jog.finish = None;
        assert!(matches!(last_op(&mut a), darkburn_ruida::RdOp::Cut { .. }), "{:?}", last_op(&mut a));
        a.machine.jog.finish = Some((300.0, 200.0));
        match last_op(&mut a) {
            darkburn_ruida::RdOp::Move(p) => assert!(p.distance(Point::new(300.0, 200.0)) < 0.002, "{p:?}"),
            other => panic!("the file ends with {other:?}"),
        }
        // A file that is measured from the head cannot hold a bed position.
        run(&mut a, "job.set_origin", json!({ "start_from": "current_position" })).unwrap();
        run(&mut a, "job.save_gcode", json!({ "path": rd.to_string_lossy(), "allow_out_of_bounds": true })).unwrap();
        let job = darkburn_ruida::RdJob::from_rd_file(&std::fs::read(&rd).unwrap(), None).unwrap();
        assert!(matches!(job.layers.last().unwrap().ops.last(), Some(darkburn_ruida::RdOp::Cut { .. })));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_from_here_starts_part_way() {
        let mut a = app();
        with_square(&mut a);
        for i in 0..6 {
            run(&mut a, "shape.add_rect", json!({ "x": 60 + i * 20, "y": 20, "width": 10, "height": 10, "layer": 0 })).unwrap();
        }
        let dir = temp("from");
        let (full, part) = (dir.join("full.gc"), dir.join("part.gc"));
        run(&mut a, "job.save_gcode", json!({ "path": full.to_string_lossy() })).unwrap();
        let job = a.job.last_job.clone().unwrap();
        let mid = job.toolpath.ops.len() * 3 / 4;
        run(&mut a, "job.preview_save_from_here", json!({ "path": part.to_string_lossy(), "op_index": mid })).unwrap();
        let (f, p) = (std::fs::read_to_string(&full).unwrap(), std::fs::read_to_string(&part).unwrap());
        assert!(p.lines().count() < f.lines().count(), "{} vs {}", p.lines().count(), f.lines().count());
        assert!(run(&mut a, "job.preview_save_from_here", json!({ "path": part.to_string_lossy(), "op_index": 99999 })).is_err());
        run(&mut a, "job.preview_start_here", json!({ "op_index": 3 })).unwrap();
        assert_eq!(a.job.start_op, Some(3));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn start_here_is_dropped_when_the_design_changes() {
        let mut a = app();
        with_square(&mut a);
        let dir = temp("stale");
        let path = dir.join("job.gc");
        run(&mut a, "job.preview_start_here", json!({ "op_index": 3 })).unwrap();
        let r = run(&mut a, "job.save_gcode", json!({ "path": path.to_string_lossy() })).unwrap();
        assert_eq!(r["from_op"], 3, "the same design: Start here holds");
        // Another shape: operation 3 of the new job is some other move.
        run(&mut a, "shape.add_rect", json!({ "x": 60, "y": 20, "width": 10, "height": 10, "layer": 0 })).unwrap();
        let r = run(&mut a, "job.save_gcode", json!({ "path": path.to_string_lossy() })).unwrap();
        assert!(r["from_op"].is_null(), "the whole new job is written: {r}");
        assert_eq!(a.job.start_op, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn out_of_bounds_jobs_are_refused_unless_allowed_or_discarded() {
        let mut a = app();
        with_square(&mut a);
        run(&mut a, "shape.add_rect", json!({ "x": 390, "y": 390, "width": 50, "height": 50, "layer": 0 })).unwrap();
        let dir = temp("oob");
        let file = dir.join("o.gc");
        let err = run(&mut a, "job.save_gcode", json!({ "path": file.to_string_lossy() })).unwrap_err();
        assert!(err.contains("work area"), "{err}");
        run(&mut a, "job.save_gcode", json!({ "path": file.to_string_lossy(), "allow_out_of_bounds": true })).unwrap();
        run(&mut a, "job.set_origin", json!({ "discard_out_of_bounds": true })).unwrap();
        let r = run(&mut a, "job.generate", Value::Null).unwrap();
        assert_eq!(r["dropped_shapes"], 1);
        assert_eq!(r["out_of_bounds"], false);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn optimization_settings_are_saved_in_the_project_with_undo_and_defaults() {
        let mut a = app();
        with_square(&mut a);
        run(
            &mut a,
            "job.set_optimization",
            json!({ "optimize": false, "order_by": ["priority", "layer"], "direction_order": "left_to_right", "hide_backlash": true }),
        )
        .unwrap();
        let s = current_settings(&a);
        assert!(!s.planner.optimize && s.planner.hide_backlash);
        assert_eq!(s.planner.order_by, vec![OrderKey::Priority, OrderKey::Layer]);
        assert_eq!(s.planner.direction_order, Some(ScanDirection::LeftToRight));
        // Saved in the project file and read back.
        let bytes = darkburn_core::project::save(&a.editor.doc, "t", None).unwrap();
        let back = darkburn_core::project::load(&bytes).unwrap().doc;
        assert!(!JobProjectSettings::from_doc(&back, &PlannerSettings::default()).planner.optimize);
        // Undo.
        run(&mut a, "edit.undo", Value::Null).unwrap();
        assert!(current_settings(&a).planner.optimize);
        run(&mut a, "edit.redo", Value::Null).unwrap();
        // Set as default, then a new project starts with it; reset puts it back.
        run(&mut a, "job.optimization_defaults", json!({ "action": "set" })).unwrap();
        assert!(!a.job.default_planner.optimize);
        let fresh = JobProjectSettings::from_doc(&darkburn_core::Document::new(), &a.job.default_planner);
        assert!(!fresh.planner.optimize);
        run(&mut a, "job.set_optimization", json!({ "optimize": true })).unwrap();
        run(&mut a, "job.optimization_defaults", json!({ "action": "reset" })).unwrap();
        assert!(!current_settings(&a).planner.optimize);
        assert!(run(&mut a, "job.set_optimization", json!({ "order_by": ["nope"] })).is_err());
    }

    #[test]
    fn job_origin_start_from_and_user_origin() {
        let mut a = app();
        with_square(&mut a);
        run(&mut a, "job.set_origin", json!({ "start_from": "user_origin", "anchor": "centre" })).unwrap();
        assert!(run(&mut a, "job.generate", Value::Null).unwrap_err().contains("user origin"));
        run(&mut a, "job.set_user_origin", json!({ "x": 100, "y": 100 })).unwrap();
        let r = run(&mut a, "job.generate", Value::Null).unwrap();
        assert!(r["seconds"].as_f64().unwrap() > 0.0);
        let job = a.job.last_job.clone().unwrap();
        let pts: Vec<Point> = job.toolpath.ops.iter().filter_map(|o| o.target()).collect();
        let cx = (pts.iter().map(|p| p.x).fold(f64::MAX, f64::min) + pts.iter().map(|p| p.x).fold(f64::MIN, f64::max)) / 2.0;
        assert!((cx - 100.0).abs() < 1e-6, "the job centre sits on the user origin: {cx}");
        // The design preview keeps the drawing where it is.
        assert!(job.design.ops.iter().filter_map(|o| o.target()).all(|p| p.x >= 19.9 && p.x <= 50.1));
        run(&mut a, "job.clear_user_origin", Value::Null).unwrap();
        assert!(a.job.user_origin.is_none());
        assert!(run(&mut a, "job.set_origin", json!({ "anchor": "middle" })).is_err());
        run(&mut a, "job.set_origin", json!({ "start_from": "current_position", "cut_selected": true })).unwrap();
        run(&mut a, "selection.clear", Value::Null).unwrap();
        assert!(run(&mut a, "job.generate", Value::Null).unwrap_err().contains("selected"));
        let s = current_settings(&a);
        assert_eq!(s.start_from, StartMode::CurrentPosition);
        assert!(s.cut_selected);
    }

    #[test]
    fn last_position_follows_where_the_job_landed() {
        let mut a = app();
        with_square(&mut a);
        run(&mut a, "job.set_origin", json!({ "start_from": "user_origin", "anchor": "bottom_left" })).unwrap();
        run(&mut a, "job.set_user_origin", json!({ "x": 200, "y": 150 })).unwrap();
        let dir = temp("last");
        run(&mut a, "job.save_gcode", json!({ "path": dir.join("a.gc").to_string_lossy() })).unwrap();
        let r = a.job.last_position.unwrap().bounds;
        assert!((r.x0 - 200.0).abs() < 1e-6 && (r.y0 - 150.0).abs() < 1e-6 && (r.width() - 30.0).abs() < 1e-6, "{r:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn machine_commands_are_hardware_and_say_no_machine() {
        let mut a = app();
        for id in [
            "job.start",
            "job.pause",
            "job.stop",
            "job.frame",
            "job.frame_rubber",
            "job.frame_selection",
            "job.send",
            "job.run_gcode",
            "job.home",
            "job.go_origin",
        ] {
            let info = crate::commands::describe(id).unwrap();
            // The machine side runs these on the simulators; with nothing connected they refuse.
            assert!(info.hardware && info.implemented, "{id}");
            let e = run(&mut a, id, Value::Null).unwrap_err();
            assert!(e.contains("not connected"), "{id}: {e}");
        }
        for id in ["job.preview_run_from_here", "job.preview_send_from_here"] {
            assert!(crate::commands::describe(id).unwrap().hardware);
            assert!(run(&mut a, id, json!({ "op_index": 1 })).unwrap_err().contains("not connected"));
        }
        for id in ["file.save_gcode", "window.preview", "laser.material_test", "job.preview_save_from_here"] {
            let info = crate::commands::describe(id).unwrap();
            assert!(!info.hardware && info.implemented, "{id}");
        }
        // Through the window path the same answer shows as the machine panel's message.
        let ctx = egui::Context::default();
        a.run(&ctx, "job.start");
        assert!(a.machine.message.as_ref().unwrap().text.contains("not connected"));
    }

    #[test]
    fn material_test_through_commands_with_undo_and_presets() {
        let mut a = app();
        let r = run(&mut a, "job.material_test", json!({ "columns": 3, "rows": 2, "x_min": 100, "x_max": 300, "y_min": 20, "y_max": 60 })).unwrap();
        assert_eq!(r["shapes"].as_array().unwrap().len(), 6 + 5 + 2);
        assert_eq!(a.editor.doc.layer(0).main().speed, 100.0);
        assert_eq!(a.editor.doc.layer(2).main().speed, 300.0);
        run(&mut a, "edit.undo", Value::Null).unwrap();
        assert!(a.editor.doc.is_empty());
        assert!(run(&mut a, "job.material_test", json!({ "columns": 20, "rows": 20 })).is_err());
        assert!(run(&mut a, "job.material_test", json!({ "x_axis": "colour" })).is_err());
        let dir = temp("presets");
        let file = dir.join("p.json");
        run(&mut a, "job.material_test_presets", json!({ "action": "save", "name": "Birch" })).unwrap();
        run(&mut a, "job.material_test_presets", json!({ "action": "export", "path": file.to_string_lossy() })).unwrap();
        run(&mut a, "job.material_test_presets", json!({ "action": "delete", "name": "Birch" })).unwrap();
        assert!(a.job.presets.is_empty());
        run(&mut a, "job.material_test_presets", json!({ "action": "import", "path": file.to_string_lossy() })).unwrap();
        let l = run(&mut a, "job.material_test_presets", json!({ "action": "list" })).unwrap();
        assert_eq!(l["presets"], json!(["Birch"]));
        run(&mut a, "job.material_test_presets", json!({ "action": "load", "name": "Birch" })).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn frame_paths_and_preview_image() {
        let mut a = app();
        with_square(&mut a);
        let r = run(&mut a, "job.frame_path", json!({ "kind": "rectangle" })).unwrap();
        assert_eq!(r["points"].as_array().unwrap().len(), 5);
        let r = run(&mut a, "job.frame_path", json!({ "kind": "rubber_band", "power": 1.5 })).unwrap();
        assert!(r["points"].as_array().unwrap().len() >= 4);
        run(&mut a, "selection.clear", Value::Null).unwrap();
        assert!(run(&mut a, "job.frame_path", json!({ "kind": "selection" })).is_err());
        let dir = temp("img");
        let file = dir.join("p.png");
        run(&mut a, "job.preview_save_image", json!({ "path": file.to_string_lossy(), "transparent": true, "width": 300 })).unwrap();
        let img = image::open(&file).unwrap().to_rgba8();
        assert_eq!(img.width(), 300);
        assert_eq!(img.get_pixel(1, 1).0[3], 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_preview_window_flow_builds_off_the_ui_thread() {
        let mut a = app();
        with_square(&mut a);
        run(&mut a, "window.preview", Value::Null).unwrap();
        assert!(a.job.preview.open && a.job.pending.is_some());
        let mut result = None;
        for _ in 0..200 {
            if let Some((task, _)) = a.job.pending.as_mut()
                && let Some(r) = task.poll()
            {
                result = Some(r.clone());
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let (_, then) = a.job.pending.take().unwrap();
        finish(&mut a, result.unwrap().unwrap(), then);
        let data = a.job.preview.data.clone().unwrap();
        assert!(data.stats.seconds > 0.0);
    }

    #[test]
    fn state_persists_to_the_settings_folder() {
        let dir = temp("state");
        let mut a = app();
        a.settings_dir = Some(dir.clone());
        run(&mut a, "job.set_gcode_settings", json!({ "flavor": "marlin", "accel": 3000 })).unwrap();
        let mut b = app();
        b.job.ensure_loaded(Some(&dir));
        assert_eq!(b.job.gcode.flavor, darkburn_job::gcode::Flavor::Marlin);
        assert_eq!(b.job.gcode.s_max, 255.0);
        assert_eq!(b.job.additional.accel, 3000.0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
