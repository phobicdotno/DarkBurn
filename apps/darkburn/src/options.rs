//! The settings that are not window layout: units, drawing quality, snapping, behaviour
//! defaults, nudge distances and so on (checklist section 22). They live in [`Options`], inside
//! the preferences. Code far from the settings window (the renderer, the canvas input, the cut
//! panel) reads them through [`get`], a snapshot refreshed once per frame by [`publish`].

use std::cell::RefCell;

use serde::{Deserialize, Serialize};

/// How finely curves are drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CurveQuality {
    Coarse,
    #[default]
    Normal,
    Fine,
}

impl CurveQuality {
    pub const ALL: [CurveQuality; 3] = [CurveQuality::Coarse, CurveQuality::Normal, CurveQuality::Fine];

    /// Multiplier on the curve flattening tolerance (smaller is finer).
    pub fn tolerance_factor(self) -> f64 {
        match self {
            CurveQuality::Coarse => 2.0,
            CurveQuality::Normal => 1.0,
            CurveQuality::Fine => 0.4,
        }
    }
}

/// Size of toolbar icons.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum IconSize {
    Small,
    #[default]
    Medium,
    Large,
}

impl IconSize {
    pub const ALL: [IconSize; 3] = [IconSize::Small, IconSize::Medium, IconSize::Large];

    pub fn points(self) -> f32 {
        match self {
            IconSize::Small => 20.0,
            IconSize::Medium => 26.0,
            IconSize::Large => 32.0,
        }
    }
}

/// Which file dialogs to use.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileDialogs {
    #[default]
    System,
    BuiltIn,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Options {
    // Units and display (SET-01 to SET-04, SET-08)
    /// Speeds shown in mm/min instead of mm/s.
    pub speed_per_min: bool,
    pub curve_quality: CurveQuality,
    pub antialias: bool,
    pub reduce_motion: bool,
    /// Interface scale in percent (font size and everything that follows it).
    pub ui_scale: f32,
    pub icon_size: IconSize,
    /// Size of the crosshair cursor on the canvas in points.
    pub cursor_size: f32,
    /// Size of node handles in points.
    pub node_size: f32,
    pub fullscreen_cursor: bool,
    pub centre_cross: bool,
    pub show_rotary_toggles: bool,
    // Snapping and clicking (SET-06, SET-07)
    pub snap_grid: bool,
    pub snap_objects: bool,
    /// Grid snap spacing in mm; 0 follows the zoom.
    pub grid_snap_mm: f32,
    /// How far (screen points) a point may jump to a shape.
    pub object_snap_px: f32,
    /// How near (screen points) a click must be to pick a shape.
    pub click_tolerance_px: f32,
    // Behaviour (SET-11 to SET-28)
    pub check_updates: bool,
    /// Where the update check looks (empty: the DarkBurn releases on GitHub).
    pub update_url: String,
    pub default_cut_selected: bool,
    pub default_use_selection_origin: bool,
    pub save_job_origin: bool,
    pub remember_locks: bool,
    pub locks: [bool; 4],
    pub ignore_out_of_bounds: bool,
    pub ignore_start_when_monitor_off: bool,
    pub default_layers_on_new: bool,
    pub file_dialogs: FileDialogs,
    pub sound_error: bool,
    pub sound_done: bool,
    pub job_log: bool,
    /// The folder the job log is written to (empty: DarkBurn's settings folder).
    pub job_log_dir: String,
    pub system_clipboard: bool,
    /// Arrow key nudge distances in mm: plain, with Ctrl, with Shift.
    pub nudge: f64,
    pub nudge_ctrl: f64,
    pub nudge_shift: f64,
    pub font_name: String,
    pub font_height: f64,
    pub auto_weld: bool,
    /// Serial ports DarkBurn never lists, one per entry.
    pub excluded_ports: Vec<String>,
    /// Use the wireless handheld pendant (XHC receiver, USB 10ce:eba0). Off until the person at
    /// this computer turns it on; a settings file or shared defaults never turn it on.
    pub pendant: bool,
    // Output (SET-36); the other import and export settings are in formats.rs
    pub output_tolerance: f64,
    // Capture (SET-37)
    pub capture_system: String,
    pub capture_width: u32,
    pub capture_height: u32,
    pub capture_colour: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            speed_per_min: false,
            curve_quality: CurveQuality::Normal,
            antialias: true,
            reduce_motion: false,
            ui_scale: 100.0,
            icon_size: IconSize::Medium,
            cursor_size: 8.0,
            node_size: 4.0,
            fullscreen_cursor: false,
            centre_cross: true,
            show_rotary_toggles: true,
            snap_grid: true,
            snap_objects: true,
            grid_snap_mm: 0.0,
            object_snap_px: 8.0,
            click_tolerance_px: crate::canvas_input::CLICK_TOLERANCE_PX,
            check_updates: false,
            update_url: String::new(),
            default_cut_selected: false,
            default_use_selection_origin: false,
            save_job_origin: true,
            remember_locks: false,
            locks: [false; 4],
            ignore_out_of_bounds: false,
            ignore_start_when_monitor_off: false,
            default_layers_on_new: true,
            file_dialogs: FileDialogs::System,
            sound_error: false,
            sound_done: false,
            job_log: false,
            job_log_dir: String::new(),
            system_clipboard: true,
            nudge: 1.0,
            nudge_ctrl: 0.1,
            nudge_shift: 10.0,
            font_name: "IBM Plex Sans".into(),
            font_height: 10.0,
            auto_weld: false,
            excluded_ports: Vec::new(),
            pendant: false,
            output_tolerance: 0.01,
            capture_system: "default".into(),
            capture_width: 1280,
            capture_height: 720,
            capture_colour: true,
        }
    }
}

impl Options {
    /// Keep every value in a range the program can work with.
    pub fn sanitise(&mut self) {
        let d = Options::default();
        let fix = |v: &mut f32, lo: f32, hi: f32, dv: f32| {
            *v = if v.is_finite() { v.clamp(lo, hi) } else { dv };
        };
        fix(&mut self.ui_scale, 70.0, 250.0, d.ui_scale);
        fix(&mut self.cursor_size, 2.0, 60.0, d.cursor_size);
        fix(&mut self.node_size, 2.0, 16.0, d.node_size);
        fix(&mut self.grid_snap_mm, 0.0, 1000.0, 0.0);
        fix(&mut self.object_snap_px, 0.0, 40.0, d.object_snap_px);
        fix(&mut self.click_tolerance_px, 1.0, 20.0, d.click_tolerance_px);
        let fixd = |v: &mut f64, lo: f64, hi: f64, dv: f64| {
            *v = if v.is_finite() { v.clamp(lo, hi) } else { dv };
        };
        fixd(&mut self.nudge, 0.001, 1000.0, d.nudge);
        fixd(&mut self.nudge_ctrl, 0.001, 1000.0, d.nudge_ctrl);
        fixd(&mut self.nudge_shift, 0.001, 1000.0, d.nudge_shift);
        fixd(&mut self.font_height, 0.1, 1000.0, d.font_height);
        fixd(&mut self.output_tolerance, 0.0001, 10.0, d.output_tolerance);
        self.capture_width = self.capture_width.clamp(160, 7680);
        self.capture_height = self.capture_height.clamp(120, 4320);
        if self.font_name.trim().is_empty() {
            self.font_name = d.font_name;
        }
        self.excluded_ports.retain(|p| !p.trim().is_empty());
    }

    /// The nudge distance in mm for the held modifiers.
    pub fn nudge_step(&self, shift: bool, ctrl: bool) -> f64 {
        match (ctrl, shift) {
            (true, true) => self.nudge_ctrl / 10.0,
            (true, false) => self.nudge_ctrl,
            (false, true) => self.nudge_shift,
            (false, false) => self.nudge,
        }
    }

    /// A speed in mm/s as the text of the chosen unit, without the unit.
    pub fn speed_value(&self, mm_per_s: f64) -> f64 {
        if self.speed_per_min { mm_per_s * 60.0 } else { mm_per_s }
    }

    /// The chosen unit's value back to mm/s.
    pub fn speed_to_mm_s(&self, value: f64) -> f64 {
        if self.speed_per_min { value / 60.0 } else { value }
    }

    pub fn speed_unit(&self) -> &'static str {
        if self.speed_per_min { "mm/min" } else { "mm/s" }
    }

    /// Whether DarkBurn may list serial port `name` (device code calls this when it lists ports).
    #[allow(dead_code, reason = "called by the device list once it exists")]
    pub fn port_allowed(&self, name: &str) -> bool {
        !self.excluded_ports.iter().any(|p| p.eq_ignore_ascii_case(name.trim()))
    }
}

thread_local! {
    // One snapshot per thread that draws an app: every reader is on that thread, and two apps
    // in one process (the UI tests, side by side) do not see each other's settings.
    static CURRENT: RefCell<Option<Options>> = const { RefCell::new(None) };
}

/// The snapshot code outside the settings window reads.
pub fn get() -> Options {
    CURRENT.with(|c| c.borrow().clone()).unwrap_or_default()
}

/// Refresh the snapshot from the preferences (once per frame).
pub fn publish(o: &Options) {
    CURRENT.with(|c| {
        let mut c = c.borrow_mut();
        if c.as_ref() != Some(o) {
            *c = Some(o.clone());
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_snapshot_is_not_shared_with_an_app_on_another_thread() {
        publish(&Options { nudge: 7.0, ..Default::default() });
        std::thread::spawn(|| publish(&Options { nudge: 3.0, ..Default::default() })).join().unwrap();
        assert_eq!(get().nudge, 7.0, "another app's settings replaced this one's");
    }

    #[test]
    fn nudge_distances_follow_the_modifiers() {
        let o = Options { nudge: 2.0, nudge_ctrl: 0.5, nudge_shift: 20.0, ..Default::default() };
        assert_eq!(o.nudge_step(false, false), 2.0);
        assert_eq!(o.nudge_step(false, true), 0.5);
        assert_eq!(o.nudge_step(true, false), 20.0);
        assert_eq!(o.nudge_step(true, true), 0.05);
    }

    #[test]
    fn speed_units_convert_both_ways() {
        let o = Options { speed_per_min: true, ..Default::default() };
        assert_eq!(o.speed_value(10.0), 600.0);
        assert_eq!(o.speed_to_mm_s(600.0), 10.0);
        assert_eq!(o.speed_unit(), "mm/min");
        let o = Options::default();
        assert_eq!((o.speed_value(10.0), o.speed_unit()), (10.0, "mm/s"));
    }

    #[test]
    fn bad_values_are_pulled_into_range() {
        let mut o: Options =
            serde_json::from_str(r#"{"ui_scale": 9000, "click_tolerance_px": -1, "nudge": 0, "font_name": " ", "excluded_ports": ["", "COM3"]}"#)
                .unwrap();
        o.sanitise();
        assert_eq!((o.ui_scale, o.click_tolerance_px), (250.0, 1.0));
        assert_eq!(o.nudge, 0.001);
        assert_eq!(o.font_name, "IBM Plex Sans");
        assert_eq!(o.excluded_ports, ["COM3"]);
        assert!(!o.port_allowed("com3") && o.port_allowed("COM4"));
    }
}
