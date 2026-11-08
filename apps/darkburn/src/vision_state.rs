//! Everything camera, rotary, cylinder and print-and-cut setup the project remembers, and the
//! small map the job builder applies to every path. Stored in the document as opaque JSON
//! (`Document::vision_settings`), so it is saved, undone and loaded with the project.

use darkburn_core::Document;
use darkburn_core::kurbo::{BezPath, Point};
use darkburn_vision::camera::CaptureSettings;
use darkburn_vision::cylinder::{CylinderAxis, CylinderCorrection, CylinderKind};
use darkburn_vision::printcut::{PnCFit, PnCMode, PnCWizard};
use darkburn_vision::rotary::RotaryParams;
use darkburn_vision::testpatterns::{FocusTestParams, IntervalTestParams};
use serde::{Deserialize, Serialize};

pub const VERSION: u32 = 1;

/// How a rotary job is cut into pieces for galvo machines (data only; the machine side sends it).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SplitMode {
    #[default]
    Off,
    Slice,
    Shape,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SplitSettings {
    pub mode: SplitMode,
    /// Largest piece the galvo field can mark, mm.
    pub max_size: f64,
}

impl Default for SplitSettings {
    fn default() -> Self {
        SplitSettings { mode: SplitMode::Off, max_size: 50.0 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CylinderSetup {
    pub enabled: bool,
    pub params: CylinderCorrection,
}

impl Default for CylinderSetup {
    fn default() -> Self {
        CylinderSetup {
            enabled: false,
            params: CylinderCorrection { radius: 40.0, kind: CylinderKind::Convex, axis: CylinderAxis::AlongX, centre: 0.0 },
        }
    }
}

/// Print and cut: the design targets, where the laser found them, and the registration in force.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PnCState {
    /// Target positions in the design (mm); the first two are the "first" and "second" targets.
    pub design: Vec<Option<Point>>,
    /// Where the laser was when each target was set (mm).
    pub measured: Vec<Option<Point>>,
    pub mode: PnCMode,
    /// The registration applied to jobs; None when none is active.
    pub fit: Option<PnCFit>,
    /// The wizard's state, kept so a project reopens mid-way.
    pub wizard: Option<PnCWizard>,
}

impl PnCState {
    pub fn set_target(&mut self, index: usize, design: Option<Point>, measured: Point) {
        while self.design.len() <= index {
            self.design.push(None);
            self.measured.push(None);
        }
        if design.is_some() {
            self.design[index] = design;
        }
        self.measured[index] = Some(measured);
        self.fit = None;
    }

    pub fn reset(&mut self) {
        *self = PnCState { mode: self.mode, ..Default::default() };
    }

    /// The targets that have both a design and a measured position.
    pub fn complete_pairs(&self) -> (Vec<Point>, Vec<Point>) {
        let mut d = Vec::new();
        let mut m = Vec::new();
        for (a, b) in self.design.iter().zip(&self.measured) {
            if let (Some(a), Some(b)) = (a, b) {
                d.push(*a);
                m.push(*b);
            }
        }
        (d, m)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FeederSettings {
    pub window_height: f64,
    pub overlap: f64,
    pub total_length: f64,
    pub roller_diameter: f64,
    pub steps_per_rotation: f64,
    pub current_steps_per_mm: f64,
    pub commanded: f64,
    pub measured: f64,
    pub speed_mm_s: f64,
    pub settle_ms: f64,
}

impl Default for FeederSettings {
    fn default() -> Self {
        FeederSettings {
            window_height: 300.0,
            overlap: 10.0,
            total_length: 1000.0,
            roller_diameter: 25.0,
            steps_per_rotation: 1600.0,
            current_steps_per_mm: 80.0,
            commanded: 100.0,
            measured: 100.0,
            speed_mm_s: 50.0,
            settle_ms: 250.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VisionState {
    pub version: u32,
    pub camera: CaptureSettings,
    pub rotary: RotaryParams,
    pub rotary_split: SplitSettings,
    pub cylinder: CylinderSetup,
    pub pnc: PnCState,
    pub focus: FocusTestParams,
    pub interval: IntervalTestParams,
    pub feeder: FeederSettings,
}

impl Default for VisionState {
    fn default() -> Self {
        VisionState {
            version: VERSION,
            camera: CaptureSettings::default(),
            rotary: RotaryParams::default(),
            rotary_split: SplitSettings::default(),
            cylinder: CylinderSetup::default(),
            pnc: PnCState::default(),
            focus: FocusTestParams::default(),
            interval: IntervalTestParams::default(),
            feeder: FeederSettings::default(),
        }
    }
}

impl VisionState {
    /// The state saved in `doc`, or the defaults when the project has none or it cannot be read.
    pub fn from_doc(doc: &Document) -> VisionState {
        doc.vision_settings.as_ref().and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default()
    }

    pub fn to_value(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }

    /// Whether it equals a fresh project's setup (nothing is stored then).
    pub fn is_default(&self) -> bool {
        *self == VisionState::default()
    }
}

/// The path changes the job builder applies: cylinder correction, then rotary, then the
/// print-and-cut registration. Built once per job from the document.
pub struct JobMap {
    cylinder: Option<CylinderCorrection>,
    rotary: Option<RotaryParams>,
    fit: Option<PnCFit>,
    /// Said once per job when a path could not be mapped.
    pub warnings: std::cell::RefCell<Vec<String>>,
}

impl JobMap {
    /// None when the document needs no mapping (the usual case).
    pub fn from_doc(doc: &Document) -> Option<JobMap> {
        doc.vision_settings.as_ref()?;
        let s = VisionState::from_doc(doc);
        let map = JobMap {
            cylinder: s.cylinder.enabled.then_some(s.cylinder.params),
            rotary: (s.rotary.enabled && s.rotary.validate().is_ok()).then_some(s.rotary),
            fit: s.pnc.fit,
            warnings: Default::default(),
        };
        (map.cylinder.is_some() || map.rotary.is_some() || map.fit.is_some()).then_some(map)
    }

    /// Map one design path to the path the machine should cut.
    pub fn apply(&self, path: BezPath) -> BezPath {
        let mut path = path;
        if let Some(c) = &self.cylinder {
            match c.map_path(&path, 0.5) {
                Ok(p) => path = p,
                Err(e) => self.warn(format!("Cylinder correction left a shape alone: {e}.")),
            }
        }
        if let Some(r) = &self.rotary {
            match r.map_path(&path) {
                Ok(p) => path = p,
                Err(e) => self.warn(format!("Rotary left a shape alone: {e}.")),
            }
        }
        if let Some(f) = &self.fit {
            path = f.apply_path(&path);
        }
        path
    }

    fn warn(&self, w: String) {
        let mut v = self.warnings.borrow_mut();
        if !v.contains(&w) {
            v.push(w);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use darkburn_core::kurbo::Shape as _;

    #[test]
    fn no_settings_no_map() {
        assert!(JobMap::from_doc(&Document::new()).is_none());
        let mut d = Document::new();
        d.vision_settings = Some(VisionState::default().to_value());
        assert!(JobMap::from_doc(&d).is_none(), "everything off means no mapping");
    }

    #[test]
    fn state_round_trips_through_json() {
        let mut s = VisionState::default();
        s.rotary.enabled = true;
        s.pnc.set_target(0, Some(Point::new(1.0, 2.0)), Point::new(3.0, 4.0));
        s.cylinder.enabled = true;
        let mut d = Document::new();
        d.vision_settings = Some(s.to_value());
        assert_eq!(VisionState::from_doc(&d), s);
        // a broken value falls back to defaults
        d.vision_settings = Some(serde_json::json!("junk"));
        assert!(VisionState::from_doc(&d).is_default());
    }

    #[test]
    fn rotary_changes_the_path_in_the_map() {
        let mut s = VisionState::default();
        s.rotary.enabled = true;
        let mut d = Document::new();
        d.vision_settings = Some(s.to_value());
        let map = JobMap::from_doc(&d).unwrap();
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((10.0, 10.0));
        let out = map.apply(p.clone());
        assert_ne!(out.bounding_box(), p.bounding_box());
    }
}
