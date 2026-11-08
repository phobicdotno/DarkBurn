//! Print and cut registration (PNC-01 to PNC-06, LT-01 to LT-06): fit the transform that moves the design onto the
//! printed material from two or more target positions measured on the laser, apply it, report the residuals,
//! and drive the wizard as plain data.

use crate::transform2d::{fit_affine, fit_similarity};
use kurbo::{Affine, BezPath, Point};
use serde::{Deserialize, Serialize};

/// How the design is allowed to change to meet the targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PnCMode {
    /// Move, rotate and scale uniformly (PNC-02).
    #[default]
    Scaled,
    /// Move and rotate only (PNC-03).
    Unscaled,
    /// Full affine including shear and independent X/Y scale; needs three or more targets.
    FullAffine,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PnCError {
    NotEnoughTargets {
        need: usize,
        have: usize,
    },
    LengthMismatch,
    /// Targets coincide or are collinear.
    Degenerate,
}

impl std::fmt::Display for PnCError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotEnoughTargets { need, have } => write!(f, "need {need} targets, have {have}"),
            Self::LengthMismatch => write!(f, "design and measured target lists differ in length"),
            Self::Degenerate => write!(f, "the targets coincide or lie on one line"),
        }
    }
}
impl std::error::Error for PnCError {}

/// One target's fit quality.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct TargetResidual {
    pub expected: Point,
    pub actual: Point,
    pub error_mm: f64,
}

/// How well the fitted transform lands the design targets on the measured ones.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResidualReport {
    pub per_target: Vec<TargetResidual>,
    pub rms_mm: f64,
    pub max_mm: f64,
}

impl ResidualReport {
    /// True when every target is within `tolerance_mm`.
    pub fn within(&self, tolerance_mm: f64) -> bool {
        self.max_mm <= tolerance_mm
    }
}

/// Result of a registration fit.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PnCFit {
    pub mode: PnCMode,
    pub affine: Affine,
    /// Uniform scale (geometric mean of the axis scales for a full affine).
    pub scale: f64,
    pub rotation_deg: f64,
    pub translation: (f64, f64),
    pub residuals: ResidualReport,
}

impl PnCFit {
    pub fn apply_point(&self, p: Point) -> Point {
        self.affine * p
    }

    pub fn apply_path(&self, path: &BezPath) -> BezPath {
        self.affine * path.clone()
    }

    pub fn apply_paths(&self, paths: &[BezPath]) -> Vec<BezPath> {
        paths.iter().map(|p| self.apply_path(p)).collect()
    }
}

fn minimum_targets(mode: PnCMode) -> usize {
    match mode {
        PnCMode::Scaled | PnCMode::Unscaled => 2,
        PnCMode::FullAffine => 3,
    }
}

/// Fit the registration. `design` are target positions in the design; `measured` where the laser found them.
/// Two or more targets are accepted; more than needed are fitted by least squares.
pub fn fit_registration(design: &[Point], measured: &[Point], mode: PnCMode) -> Result<PnCFit, PnCError> {
    if design.len() != measured.len() {
        return Err(PnCError::LengthMismatch);
    }
    let need = minimum_targets(mode);
    if design.len() < need {
        return Err(PnCError::NotEnoughTargets { need, have: design.len() });
    }
    let (affine, scale, rot, tr) = match mode {
        PnCMode::Scaled | PnCMode::Unscaled => {
            let s = fit_similarity(design, measured, mode == PnCMode::Scaled).ok_or(PnCError::Degenerate)?;
            (s.to_affine(), s.scale, s.angle.to_degrees(), s.translation)
        }
        PnCMode::FullAffine => {
            let a = fit_affine(design, measured).ok_or(PnCError::Degenerate)?;
            let c = a.as_coeffs();
            let sx = c[0].hypot(c[1]);
            let sy = c[2].hypot(c[3]);
            (a, (sx * sy).sqrt(), c[1].atan2(c[0]).to_degrees(), (c[4], c[5]))
        }
    };
    let per_target: Vec<TargetResidual> = design
        .iter()
        .zip(measured)
        .map(|(d, m)| {
            let e = affine * *d;
            TargetResidual { expected: *m, actual: e, error_mm: (e - *m).hypot() }
        })
        .collect();
    let rms = (per_target.iter().map(|t| t.error_mm * t.error_mm).sum::<f64>() / per_target.len() as f64).sqrt();
    let max = per_target.iter().map(|t| t.error_mm).fold(0.0, f64::max);
    if !(rms.is_finite() && affine.as_coeffs().iter().all(|v| v.is_finite())) {
        return Err(PnCError::Degenerate);
    }
    Ok(PnCFit { mode, affine, scale, rotation_deg: rot, translation: tr, residuals: ResidualReport { per_target, rms_mm: rms, max_mm: max } })
}

/// Measure each design target on the laser through `measure(index, design_position)`; the callback returns where the
/// operator positioned the laser (or `None` to abort). The library never talks to a machine itself.
pub fn measure_targets(design: &[Point], mut measure: impl FnMut(usize, Point) -> Option<Point>) -> Option<Vec<Point>> {
    design.iter().enumerate().map(|(i, d)| measure(i, *d)).collect()
}

// ---------------------------------------------------------------- wizard

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PnCStep {
    ChooseMode,
    /// Waiting for the operator to position the laser on target `n` (0 based).
    MeasureTarget(usize),
    /// All targets measured; fit is available for review.
    Review,
    Done,
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum PnCEvent {
    SelectMode(PnCMode),
    /// The laser position for the current target.
    Measured(Point),
    /// Go back one target.
    Back,
    /// Accept the fit shown in the review step.
    Confirm,
    /// Clear everything and start over (PNC-04).
    Reset,
    Cancel,
}

#[derive(Clone, Debug, PartialEq)]
pub enum WizardError {
    /// The event makes no sense in the current step.
    WrongStep(PnCStep),
    Fit(PnCError),
}

impl std::fmt::Display for WizardError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WrongStep(s) => write!(f, "that action is not available in step {s:?}"),
            Self::Fit(e) => write!(f, "{e}"),
        }
    }
}
impl std::error::Error for WizardError {}

/// What the main window shows (PNC-06).
#[derive(Clone, Debug, PartialEq)]
pub enum PnCStatus {
    /// Nothing set.
    Inactive,
    /// Measuring: `done` of `total` targets set.
    InProgress { done: usize, total: usize },
    /// A registration is active.
    Active { mode: PnCMode, rms_mm: f64, scale: f64, rotation_deg: f64 },
}

/// The print and cut wizard as a serialisable state machine.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PnCWizard {
    pub design_targets: Vec<Point>,
    pub mode: PnCMode,
    pub measured: Vec<Point>,
    pub step: PnCStep,
    pub fit: Option<PnCFit>,
}

impl PnCWizard {
    /// Start a wizard for two, three or four design targets.
    pub fn new(design_targets: Vec<Point>) -> Result<Self, PnCError> {
        if design_targets.len() < 2 {
            return Err(PnCError::NotEnoughTargets { need: 2, have: design_targets.len() });
        }
        Ok(Self { design_targets, mode: PnCMode::Scaled, measured: Vec::new(), step: PnCStep::ChooseMode, fit: None })
    }

    /// The design target the operator should now position the laser on.
    pub fn current_target(&self) -> Option<Point> {
        match self.step {
            PnCStep::MeasureTarget(i) => self.design_targets.get(i).copied(),
            _ => None,
        }
    }

    pub fn handle(&mut self, event: PnCEvent) -> Result<(), WizardError> {
        use PnCEvent::*;
        match (self.step, event) {
            (_, Reset) => {
                self.measured.clear();
                self.fit = None;
                self.step = PnCStep::ChooseMode;
                Ok(())
            }
            (_, Cancel) => {
                self.measured.clear();
                self.fit = None;
                self.step = PnCStep::Cancelled;
                Ok(())
            }
            (PnCStep::ChooseMode, SelectMode(m)) => {
                if m == PnCMode::FullAffine && self.design_targets.len() < 3 {
                    return Err(WizardError::Fit(PnCError::NotEnoughTargets { need: 3, have: self.design_targets.len() }));
                }
                self.mode = m;
                self.measured.clear();
                self.step = PnCStep::MeasureTarget(0);
                Ok(())
            }
            (PnCStep::MeasureTarget(i), Measured(p)) => {
                self.measured.truncate(i);
                self.measured.push(p);
                if i + 1 < self.design_targets.len() {
                    self.step = PnCStep::MeasureTarget(i + 1);
                } else {
                    self.fit = Some(fit_registration(&self.design_targets, &self.measured, self.mode).map_err(WizardError::Fit)?);
                    self.step = PnCStep::Review;
                }
                Ok(())
            }
            (PnCStep::MeasureTarget(i), Back) => {
                self.step = if i == 0 { PnCStep::ChooseMode } else { PnCStep::MeasureTarget(i - 1) };
                Ok(())
            }
            (PnCStep::Review, Back) => {
                self.fit = None;
                self.step = PnCStep::MeasureTarget(self.design_targets.len() - 1);
                Ok(())
            }
            (PnCStep::Review, Confirm) => {
                self.step = PnCStep::Done;
                Ok(())
            }
            (s, _) => Err(WizardError::WrongStep(s)),
        }
    }

    /// The transform to apply to jobs, once the wizard has been confirmed.
    pub fn active_fit(&self) -> Option<&PnCFit> {
        if self.step == PnCStep::Done { self.fit.as_ref() } else { None }
    }

    pub fn status(&self) -> PnCStatus {
        match (&self.step, &self.fit) {
            (PnCStep::Done, Some(f)) => PnCStatus::Active { mode: f.mode, rms_mm: f.residuals.rms_mm, scale: f.scale, rotation_deg: f.rotation_deg },
            (PnCStep::MeasureTarget(i), _) => PnCStatus::InProgress { done: *i, total: self.design_targets.len() },
            (PnCStep::Review, _) => PnCStatus::InProgress { done: self.design_targets.len(), total: self.design_targets.len() },
            _ => PnCStatus::Inactive,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transform2d::Similarity;
    use kurbo::PathEl;

    fn design() -> Vec<Point> {
        vec![Point::new(10.0, 10.0), Point::new(210.0, 10.0), Point::new(210.0, 150.0), Point::new(10.0, 150.0)]
    }

    fn truth() -> Affine {
        Similarity { scale: 1.003, angle: 0.02, translation: (3.0, -2.0) }.to_affine()
    }

    #[test]
    fn two_targets_scaled_exact() {
        let d = design();
        let m: Vec<Point> = d.iter().map(|p| truth() * *p).collect();
        let fit = fit_registration(&d[..2], &m[..2], PnCMode::Scaled).unwrap();
        assert!((fit.scale - 1.003).abs() < 1e-9);
        assert!((fit.rotation_deg - 0.02f64.to_degrees()).abs() < 1e-9);
        assert!(fit.residuals.rms_mm < 1e-9);
        // third point (not used for the fit) lands correctly
        assert!((fit.apply_point(d[2]) - m[2]).hypot() < 1e-9);
    }

    #[test]
    fn unscaled_keeps_size_and_reports_error() {
        let d = design();
        let m: Vec<Point> = d.iter().map(|p| truth() * *p).collect();
        let fit = fit_registration(&d, &m, PnCMode::Unscaled).unwrap();
        assert_eq!(fit.scale, 1.0);
        // 0.3 % scale difference over ~250 mm leaves residual error but within a millimetre
        assert!(fit.residuals.rms_mm > 0.05 && fit.residuals.max_mm < 1.0);
        assert!(fit.residuals.within(1.0) && !fit.residuals.within(0.01));
        let scaled = fit_registration(&d, &m, PnCMode::Scaled).unwrap();
        assert!(scaled.residuals.rms_mm < 1e-9);
    }

    #[test]
    fn four_noisy_targets_least_squares_and_full_affine() {
        let d = design();
        let a = Affine::new([1.002, 0.001, -0.002, 0.998, 4.0, 5.0]);
        let m: Vec<Point> = d.iter().map(|p| a * *p).collect();
        let fit = fit_registration(&d, &m, PnCMode::FullAffine).unwrap();
        assert!(fit.residuals.rms_mm < 1e-9);
        let similarity = fit_registration(&d, &m, PnCMode::Scaled).unwrap();
        assert!(similarity.residuals.rms_mm > fit.residuals.rms_mm);
        assert_eq!(fit_registration(&d[..2], &m[..2], PnCMode::FullAffine).unwrap_err(), PnCError::NotEnoughTargets { need: 3, have: 2 });
    }

    #[test]
    fn errors() {
        let d = design();
        assert_eq!(fit_registration(&d[..1], &d[..1], PnCMode::Scaled).unwrap_err(), PnCError::NotEnoughTargets { need: 2, have: 1 });
        assert_eq!(fit_registration(&d, &d[..3], PnCMode::Scaled).unwrap_err(), PnCError::LengthMismatch);
        let same = vec![Point::new(1.0, 1.0); 2];
        assert_eq!(fit_registration(&same, &same, PnCMode::Scaled).unwrap_err(), PnCError::Degenerate);
    }

    #[test]
    fn applies_to_paths() {
        let d = design();
        let m: Vec<Point> = d.iter().map(|p| truth() * *p).collect();
        let fit = fit_registration(&d, &m, PnCMode::Scaled).unwrap();
        let path = BezPath::from_vec(vec![PathEl::MoveTo(Point::new(10.0, 10.0)), PathEl::LineTo(Point::new(210.0, 150.0)), PathEl::ClosePath]);
        let out = fit.apply_paths(&[path]);
        match out[0].elements()[1] {
            PathEl::LineTo(q) => assert!((q - m[2]).hypot() < 1e-9),
            _ => panic!("expected a line"),
        }
        assert!(matches!(out[0].elements()[2], PathEl::ClosePath));
    }

    #[test]
    fn measure_callback() {
        let d = design();
        let m = measure_targets(&d, |i, p| Some(Point::new(p.x + i as f64, p.y))).unwrap();
        assert_eq!(m[3], Point::new(13.0, 150.0));
        assert!(measure_targets(&d, |i, p| if i < 2 { Some(p) } else { None }).is_none());
    }

    #[test]
    fn wizard_full_run_with_reset_and_back() {
        let d = design();
        let m: Vec<Point> = d.iter().map(|p| truth() * *p).collect();
        let mut w = PnCWizard::new(d.clone()).unwrap();
        assert_eq!(w.status(), PnCStatus::Inactive);
        assert!(matches!(w.handle(PnCEvent::Confirm), Err(WizardError::WrongStep(PnCStep::ChooseMode))));
        w.handle(PnCEvent::SelectMode(PnCMode::Scaled)).unwrap();
        assert_eq!(w.current_target(), Some(d[0]));
        w.handle(PnCEvent::Measured(m[0])).unwrap();
        w.handle(PnCEvent::Measured(m[1])).unwrap();
        assert_eq!(w.status(), PnCStatus::InProgress { done: 2, total: 4 });
        w.handle(PnCEvent::Back).unwrap();
        assert_eq!(w.step, PnCStep::MeasureTarget(1));
        w.handle(PnCEvent::Measured(m[1])).unwrap();
        w.handle(PnCEvent::Measured(m[2])).unwrap();
        w.handle(PnCEvent::Measured(m[3])).unwrap();
        assert_eq!(w.step, PnCStep::Review);
        assert!(w.active_fit().is_none());
        w.handle(PnCEvent::Confirm).unwrap();
        let fit = w.active_fit().unwrap();
        assert!(fit.residuals.rms_mm < 1e-9);
        assert!(matches!(w.status(), PnCStatus::Active { mode: PnCMode::Scaled, .. }));

        // JSON round trip of the whole wizard
        let json = serde_json::to_string(&w).unwrap();
        let back: PnCWizard = serde_json::from_str(&json).unwrap();
        assert_eq!(back.step, PnCStep::Done);

        w.handle(PnCEvent::Reset).unwrap();
        assert_eq!(w.step, PnCStep::ChooseMode);
        assert!(w.measured.is_empty() && w.fit.is_none());
        assert_eq!(w.status(), PnCStatus::Inactive);
        w.handle(PnCEvent::Cancel).unwrap();
        assert_eq!(w.step, PnCStep::Cancelled);
    }

    #[test]
    fn wizard_rules() {
        assert!(PnCWizard::new(vec![Point::ZERO]).is_err());
        let mut w = PnCWizard::new(vec![Point::new(0.0, 0.0), Point::new(10.0, 0.0)]).unwrap();
        assert!(w.handle(PnCEvent::SelectMode(PnCMode::FullAffine)).is_err());
        w.handle(PnCEvent::SelectMode(PnCMode::Unscaled)).unwrap();
        w.handle(PnCEvent::Measured(Point::new(1.0, 1.0))).unwrap();
        // coincident measurement of the second target makes the fit degenerate at review time
        w.handle(PnCEvent::Measured(Point::new(1.0, 1.0))).unwrap();
        assert_eq!(w.step, PnCStep::Review);
        assert!(w.fit.as_ref().unwrap().residuals.max_mm > 1.0);
        w.handle(PnCEvent::Back).unwrap();
        assert_eq!(w.step, PnCStep::MeasureTarget(1));
    }
}
