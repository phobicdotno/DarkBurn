//! Test pattern generators (OTHER-01 focus test, OTHER-02 interval test). They return rows of paths and the
//! value each row stands for; the app turns the rows into layers with the right Z offset or line interval.

use crate::strokefont::{text_path, text_width};
use kurbo::{BezPath, Point, Rect, Shape};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TestError {
    TooFewSteps,
    InvalidRange,
    InvalidSize,
}

impl std::fmt::Display for TestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::TooFewSteps => "at least two steps are needed",
            Self::InvalidRange => "the start and end values must differ and be valid",
            Self::InvalidSize => "sizes must be above zero",
        })
    }
}
impl std::error::Error for TestError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LengthUnit {
    #[default]
    Mm,
    Inch,
}

impl LengthUnit {
    pub fn from_mm(&self, mm: f64) -> f64 {
        match self {
            Self::Mm => mm,
            Self::Inch => mm / 25.4,
        }
    }

    pub fn suffix(&self) -> &'static str {
        match self {
            Self::Mm => "mm",
            Self::Inch => "in",
        }
    }
}

fn trim_number(v: f64, decimals: usize) -> String {
    let mut s = format!("{v:.decimals$}");
    if s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
    }
    if s == "-0" { "0".to_string() } else { s }
}

// ---------------------------------------------------------------- focus test

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct FocusTestParams {
    /// Z offset of the first row, mm (negative moves the head closer in LightBurn-style Z offset terms).
    pub z_start: f64,
    pub z_end: f64,
    pub steps: usize,
    pub line_length: f64,
    pub row_pitch: f64,
    pub text_height: f64,
    pub label_gap: f64,
    pub unit: LengthUnit,
    /// Also engrave the label text; otherwise only the lines are produced.
    pub with_labels: bool,
}

impl Default for FocusTestParams {
    fn default() -> Self {
        Self {
            z_start: -3.0,
            z_end: 3.0,
            steps: 7,
            line_length: 20.0,
            row_pitch: 8.0,
            text_height: 3.0,
            label_gap: 3.0,
            unit: LengthUnit::Mm,
            with_labels: true,
        }
    }
}

/// One row of the focus test: burn `line` and `label` with the Z offset `z_mm`.
#[derive(Clone, Debug, PartialEq)]
pub struct FocusRow {
    pub index: usize,
    pub z_mm: f64,
    pub label: String,
    pub line: BezPath,
    pub label_path: BezPath,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FocusTest {
    pub params: FocusTestParams,
    pub rows: Vec<FocusRow>,
    pub bounds: Rect,
}

pub fn focus_test(params: &FocusTestParams) -> Result<FocusTest, TestError> {
    if params.steps < 2 {
        return Err(TestError::TooFewSteps);
    }
    if !(params.z_start.is_finite() && params.z_end.is_finite()) || params.z_start == params.z_end {
        return Err(TestError::InvalidRange);
    }
    if !(params.line_length > 0.0 && params.row_pitch > 0.0 && params.text_height > 0.0) || params.row_pitch < params.text_height {
        return Err(TestError::InvalidSize);
    }
    let decimals = if params.unit == LengthUnit::Inch { 3 } else { 2 };
    let mut rows = Vec::with_capacity(params.steps);
    let mut bounds: Option<Rect> = None;
    for i in 0..params.steps {
        let z = params.z_start + (params.z_end - params.z_start) * i as f64 / (params.steps - 1) as f64;
        let y = i as f64 * params.row_pitch;
        let mut line = BezPath::new();
        line.move_to(Point::new(0.0, y + params.text_height / 2.0));
        line.line_to(Point::new(params.line_length, y + params.text_height / 2.0));
        let label = format!("{}{}", trim_number(params.unit.from_mm(z), decimals), params.unit.suffix());
        let label_path = if params.with_labels {
            text_path(&label, params.text_height, Point::new(params.line_length + params.label_gap, y))
        } else {
            BezPath::new()
        };
        let mut b = line.bounding_box();
        if !label_path.elements().is_empty() {
            b = b.union(label_path.bounding_box());
        }
        bounds = Some(bounds.map_or(b, |a| a.union(b)));
        rows.push(FocusRow { index: i, z_mm: z, label, line, label_path });
    }
    let _ = text_width;
    Ok(FocusTest { params: *params, rows, bounds: bounds.unwrap_or_default() })
}

impl FocusTest {
    /// Best Z from a sharpness score per row (higher is better), interpolating between neighbours.
    pub fn best_z(&self, scores: &[f64]) -> Option<f64> {
        if scores.len() != self.rows.len() || scores.is_empty() {
            return None;
        }
        let (bi, _) = scores.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1))?;
        let z = self.rows[bi].z_mm;
        if bi == 0 || bi + 1 == scores.len() {
            return Some(z);
        }
        let (l, c, r) = (scores[bi - 1], scores[bi], scores[bi + 1]);
        let denom = l - 2.0 * c + r;
        if denom.abs() < 1e-12 {
            return Some(z);
        }
        let shift = (0.5 * (l - r) / denom).clamp(-0.5, 0.5);
        let step = (self.params.z_end - self.params.z_start) / (self.params.steps - 1) as f64;
        Some(z + shift * step)
    }
}

// ---------------------------------------------------------------- interval test

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct IntervalTestParams {
    /// Line interval of the first row, mm.
    pub interval_start: f64,
    pub interval_end: f64,
    pub steps: usize,
    pub block_width: f64,
    pub block_height: f64,
    pub gap: f64,
    pub text_height: f64,
    pub label_gap: f64,
    pub with_labels: bool,
}

impl Default for IntervalTestParams {
    fn default() -> Self {
        Self {
            interval_start: 0.05,
            interval_end: 0.25,
            steps: 9,
            block_width: 20.0,
            block_height: 6.0,
            gap: 2.0,
            text_height: 3.0,
            label_gap: 3.0,
            with_labels: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct IntervalRow {
    pub index: usize,
    pub interval_mm: f64,
    pub lines_per_mm: f64,
    pub block: BezPath,
    pub label_path: BezPath,
}

#[derive(Clone, Debug, PartialEq)]
pub struct IntervalTest {
    pub params: IntervalTestParams,
    pub rows: Vec<IntervalRow>,
    pub bounds: Rect,
}

pub fn interval_test(params: &IntervalTestParams) -> Result<IntervalTest, TestError> {
    if params.steps < 2 {
        return Err(TestError::TooFewSteps);
    }
    if !(params.interval_start > 0.0 && params.interval_end > 0.0) || params.interval_start == params.interval_end {
        return Err(TestError::InvalidRange);
    }
    if !(params.block_width > 0.0 && params.block_height > 0.0 && params.text_height > 0.0) || params.gap < 0.0 {
        return Err(TestError::InvalidSize);
    }
    let pitch = params.block_height + params.gap;
    let mut rows = Vec::with_capacity(params.steps);
    let mut bounds: Option<Rect> = None;
    for i in 0..params.steps {
        let interval = params.interval_start + (params.interval_end - params.interval_start) * i as f64 / (params.steps - 1) as f64;
        let y = i as f64 * pitch;
        let rect = Rect::new(0.0, y, params.block_width, y + params.block_height);
        let block = rect.to_path(0.1);
        let label = format!("{}mm", trim_number(interval, 3));
        let ty = y + (params.block_height - params.text_height) / 2.0;
        let label_path = if params.with_labels {
            text_path(&label, params.text_height, Point::new(params.block_width + params.label_gap, ty))
        } else {
            BezPath::new()
        };
        let mut b = rect;
        if !label_path.elements().is_empty() {
            b = b.union(label_path.bounding_box());
        }
        bounds = Some(bounds.map_or(b, |a| a.union(b)));
        rows.push(IntervalRow { index: i, interval_mm: interval, lines_per_mm: 1.0 / interval, block, label_path });
    }
    Ok(IntervalTest { params: *params, rows, bounds: bounds.unwrap_or_default() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_rows_step_through_z() {
        let t = focus_test(&FocusTestParams::default()).unwrap();
        assert_eq!(t.rows.len(), 7);
        assert!((t.rows[0].z_mm + 3.0).abs() < 1e-12 && (t.rows[6].z_mm - 3.0).abs() < 1e-12);
        assert!((t.rows[1].z_mm - t.rows[0].z_mm - 1.0).abs() < 1e-12);
        assert_eq!(t.rows[0].label, "-3mm");
        assert_eq!(t.rows[3].label, "0mm");
        // rows stack upward and labels sit right of the line
        assert!(t.rows[1].line.bounding_box().y0 > t.rows[0].line.bounding_box().y1);
        assert!(t.rows[0].label_path.bounding_box().x0 >= t.params.line_length);
        assert!(t.bounds.width() > t.params.line_length);
    }

    #[test]
    fn focus_inches_and_no_labels() {
        let p = FocusTestParams { unit: LengthUnit::Inch, z_start: 0.0, z_end: 2.54, steps: 2, with_labels: false, ..Default::default() };
        let t = focus_test(&p).unwrap();
        assert_eq!(t.rows[1].label, "0.1in");
        assert!(t.rows[1].label_path.elements().is_empty());
    }

    #[test]
    fn focus_validation() {
        assert_eq!(focus_test(&FocusTestParams { steps: 1, ..Default::default() }).unwrap_err(), TestError::TooFewSteps);
        assert_eq!(focus_test(&FocusTestParams { z_end: -3.0, ..Default::default() }).unwrap_err(), TestError::InvalidRange);
        assert_eq!(focus_test(&FocusTestParams { row_pitch: 1.0, ..Default::default() }).unwrap_err(), TestError::InvalidSize);
    }

    #[test]
    fn best_focus_interpolates() {
        let t = focus_test(&FocusTestParams::default()).unwrap();
        // parabola with peak at z = 0.3
        let scores: Vec<f64> = t.rows.iter().map(|r| 10.0 - (r.z_mm - 0.3).powi(2)).collect();
        assert!((t.best_z(&scores).unwrap() - 0.3).abs() < 1e-9);
        // peak at the edge returns the edge
        let edge: Vec<f64> = (0..7).map(|i| i as f64).collect();
        assert_eq!(t.best_z(&edge), Some(3.0));
        assert!(t.best_z(&[1.0]).is_none());
    }

    #[test]
    fn interval_rows() {
        let t = interval_test(&IntervalTestParams::default()).unwrap();
        assert_eq!(t.rows.len(), 9);
        assert!((t.rows[0].interval_mm - 0.05).abs() < 1e-12 && (t.rows[8].interval_mm - 0.25).abs() < 1e-12);
        assert!((t.rows[0].lines_per_mm - 20.0).abs() < 1e-9);
        assert_eq!(t.rows[8].block.bounding_box().height(), 6.0);
        assert!(t.rows[2].block.bounding_box().y0 > t.rows[1].block.bounding_box().y1);
        assert!(!t.rows[0].label_path.elements().is_empty());
        assert_eq!(interval_test(&IntervalTestParams { interval_start: 0.0, ..Default::default() }).unwrap_err(), TestError::InvalidRange);
        assert_eq!(interval_test(&IntervalTestParams { steps: 0, ..Default::default() }).unwrap_err(), TestError::TooFewSteps);
    }

    #[test]
    fn number_trimming() {
        assert_eq!(trim_number(1.50, 2), "1.5");
        assert_eq!(trim_number(-0.001, 2), "0");
        assert_eq!(trim_number(3.0, 2), "3");
        assert_eq!(trim_number(0.125, 3), "0.125");
    }
}
