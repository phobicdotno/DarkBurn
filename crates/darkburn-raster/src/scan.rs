//! Scanline extraction for the motion engine.

use std::collections::BTreeMap;

use kurbo::{Affine, Point};
use serde::{Deserialize, Serialize};

use crate::process::ProcessedImage;

/// How processed images become scanlines.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScanSettings {
    /// Alternate direction on every line that burns.
    pub bidirectional: bool,
    /// Extra travel before and after each burnt span, so the head is at speed when firing.
    pub overscan_mm: f64,
    /// Gaps of blank image longer than this split a line into separate spans that the
    /// engine can rapid between. `None` scans each line as one span.
    pub blank_skip_mm: Option<f64>,
    /// Each burnt stretch is shortened by this much (half at each end) to allow for the
    /// width of the laser dot. Stretches never shrink below half a line interval.
    pub dot_width_mm: f64,
    /// Power (percent) for the lightest non-zero grey level.
    pub min_power: f32,
    /// Power (percent) for full black and for every dot of a 1-bit map.
    pub max_power: f32,
}

impl Default for ScanSettings {
    fn default() -> Self {
        Self { bidirectional: true, overscan_mm: 2.5, blank_skip_mm: Some(5.0), dot_width_mm: 0.0, min_power: 0.0, max_power: 100.0 }
    }
}

/// A burnt stretch along a scanline, in scan-frame mm, `x0 < x1`, power in percent.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScanRun {
    pub x0: f64,
    pub x1: f64,
    pub power: f32,
}

/// A stretch of a scanline scanned in one move, with overscan.
///
/// `runs` are listed in travel order. `travel_start` and `travel_end` are where the head
/// starts and stops including overscan (`travel_start > travel_end` on reversed lines).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScanSpan {
    pub runs: Vec<ScanRun>,
    pub travel_start: f64,
    pub travel_end: f64,
}

/// One scanline: lattice row, scan-frame Y (mm), direction and spans in travel order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScanLine {
    pub row: i64,
    pub y: f64,
    pub reverse: bool,
    pub spans: Vec<ScanSpan>,
}

/// All scanlines for images that share a scan angle and line interval, bottom line first.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScanPlan {
    pub angle_deg: f64,
    pub interval_mm: f64,
    pub lines: Vec<ScanLine>,
}

impl ScanPlan {
    /// Scan frame to machine mm.
    pub fn scan_to_mm(&self) -> Affine {
        Affine::rotate(self.angle_deg.to_radians())
    }

    /// A point on line `line` at scan-frame X `x`, in machine mm.
    pub fn point_mm(&self, line: &ScanLine, x: f64) -> Point {
        self.scan_to_mm() * Point::new(x, line.y)
    }

    /// Total burnt length, mm.
    pub fn burn_length(&self) -> f64 {
        self.lines.iter().flat_map(|l| &l.spans).flat_map(|s| &s.runs).map(|r| r.x1 - r.x0).sum()
    }
}

/// Turns processed images into scan plans. Images whose grids share the scan angle and line
/// interval are merged into one plan (their grids sit on the same lattice); where they
/// overlap the higher power wins.
pub fn scan_plans(images: &[&ProcessedImage], settings: &ScanSettings) -> Vec<ScanPlan> {
    let mut groups: Vec<(f64, f64, Vec<&ProcessedImage>)> = Vec::new();
    for &img in images {
        let key = (img.grid.angle_deg.rem_euclid(360.0), img.grid.interval_mm);
        match groups.iter_mut().find(|g| (g.0 - key.0).abs() < 1e-9 && (g.1 - key.1).abs() < 1e-12) {
            Some(g) => g.2.push(img),
            None => groups.push((key.0, key.1, vec![img])),
        }
    }
    groups.into_iter().map(|(angle, interval, imgs)| plan_group(angle, interval, &imgs, settings)).collect()
}

fn plan_group(angle_deg: f64, interval: f64, images: &[&ProcessedImage], s: &ScanSettings) -> ScanPlan {
    // Raw runs per lattice row, in lattice columns (end exclusive) with power.
    let mut rows: BTreeMap<i64, Vec<(i64, i64, f32)>> = BTreeMap::new();
    for img in images {
        let map = &img.map;
        for y in 0..map.height {
            let row = map.row(y);
            let lattice_row = img.grid.lattice_row(y);
            let mut x = 0usize;
            while x < row.len() {
                let v = row[x];
                if v == 0 {
                    x += 1;
                    continue;
                }
                let start = x;
                while x < row.len() && row[x] == v {
                    x += 1;
                }
                let power = if map.binary { s.max_power } else { s.min_power + (s.max_power - s.min_power) * f32::from(v) / 255.0 };
                let c0 = img.grid.col0 + start as i64;
                rows.entry(lattice_row).or_default().push((c0, c0 + (x - start) as i64, power));
            }
        }
    }
    let mut lines = Vec::new();
    let mut reverse = false;
    for (row, raw) in rows {
        let merged = merge_runs(raw);
        if merged.is_empty() {
            continue;
        }
        let mut runs: Vec<ScanRun> =
            merged.into_iter().map(|(a, b, p)| ScanRun { x0: a as f64 * interval, x1: b as f64 * interval, power: p }).collect();
        if s.dot_width_mm > 0.0 {
            dot_width_correct(&mut runs, s.dot_width_mm, interval * 0.5);
        }
        let spans = split_spans(runs, s, reverse);
        lines.push(ScanLine { row, y: (row as f64 + 0.5) * interval, reverse, spans });
        if s.bidirectional {
            reverse = !reverse;
        }
    }
    ScanPlan { angle_deg, interval_mm: interval, lines }
}

/// Union of runs, max power where they overlap, neighbours with equal power joined.
fn merge_runs(mut raw: Vec<(i64, i64, f32)>) -> Vec<(i64, i64, f32)> {
    raw.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    let overlapping = raw.windows(2).any(|w| w[1].0 < w[0].1);
    let mut pieces: Vec<(i64, i64, f32)> = if overlapping {
        let mut edges: Vec<i64> = raw.iter().flat_map(|r| [r.0, r.1]).collect();
        edges.sort_unstable();
        edges.dedup();
        let mut out = Vec::new();
        let mut first = 0usize;
        for e in edges.windows(2) {
            let (a, b) = (e[0], e[1]);
            while first < raw.len() && raw[first].1 <= a {
                first += 1;
            }
            let p = raw[first..].iter().take_while(|r| r.0 <= a).filter(|r| r.1 > a).map(|r| r.2).fold(0.0f32, f32::max);
            if p > 0.0 {
                out.push((a, b, p));
            }
        }
        out
    } else {
        raw
    };
    let mut joined: Vec<(i64, i64, f32)> = Vec::with_capacity(pieces.len());
    for r in pieces.drain(..) {
        match joined.last_mut() {
            Some(last) if last.1 == r.0 && last.2 == r.2 => last.1 = r.1,
            _ => joined.push(r),
        }
    }
    joined
}

/// Shortens each contiguous burnt stretch (abutting runs count as one) by `dot` mm.
fn dot_width_correct(runs: &mut Vec<ScanRun>, dot: f64, min_len: f64) {
    let mut i = 0;
    while i < runs.len() {
        let mut j = i;
        while j + 1 < runs.len() && (runs[j + 1].x0 - runs[j].x1).abs() < 1e-9 {
            j += 1;
        }
        let (a, b) = (runs[i].x0, runs[j].x1);
        let len = b - a;
        let new_len = (len - dot).max(min_len.min(len));
        let mid = (a + b) / 2.0;
        let (na, nb) = (mid - new_len / 2.0, mid + new_len / 2.0);
        for r in &mut runs[i..=j] {
            r.x0 = r.x0.max(na);
            r.x1 = r.x1.min(nb);
        }
        i = j + 1;
    }
    runs.retain(|r| r.x1 - r.x0 > 1e-12);
}

fn split_spans(runs: Vec<ScanRun>, s: &ScanSettings, reverse: bool) -> Vec<ScanSpan> {
    let mut groups: Vec<Vec<ScanRun>> = Vec::new();
    for r in runs {
        match (groups.last_mut(), s.blank_skip_mm) {
            (Some(g), Some(skip)) if r.x0 - g.last().expect("non-empty").x1 <= skip => g.push(r),
            (Some(g), None) => g.push(r),
            _ => groups.push(vec![r]),
        }
    }
    let os = s.overscan_mm.max(0.0);
    let mut spans: Vec<ScanSpan> = groups
        .into_iter()
        .map(|mut runs| {
            let (a, b) = (runs[0].x0, runs[runs.len() - 1].x1);
            if reverse {
                runs.reverse();
                ScanSpan { runs, travel_start: b + os, travel_end: a - os }
            } else {
                ScanSpan { runs, travel_start: a - os, travel_end: b + os }
            }
        })
        .collect();
    if reverse {
        spans.reverse();
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gray::PowerMap;
    use crate::resample::ScanGrid;

    fn img(col0: i64, row_top: i64, rows: &[&[u8]], binary: bool) -> ProcessedImage {
        let w = rows[0].len() as u32;
        let h = rows.len() as u32;
        let grid = ScanGrid { interval_mm: 0.1, angle_deg: 0.0, col0, row_top, width: w, height: h };
        let data = rows.iter().flat_map(|r| r.iter().copied()).collect();
        ProcessedImage { grid, map: PowerMap { width: w, height: h, data, binary } }
    }

    fn settings() -> ScanSettings {
        ScanSettings { overscan_mm: 1.0, blank_skip_mm: None, ..Default::default() }
    }

    #[test]
    fn runs_positions_and_bidirectional() {
        let p = img(10, 1, &[&[0, 255, 255, 0, 255], &[0, 0, 0, 0, 0], &[255, 0, 0, 0, 0]], true);
        let plans = scan_plans(&[&p], &settings());
        assert_eq!(plans.len(), 1);
        let lines = &plans[0].lines;
        // Blank row skipped; bottom line (row -1) first.
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].row, -1);
        assert!((lines[0].y + 0.05).abs() < 1e-12);
        assert!(!lines[0].reverse && lines[1].reverse);
        let top = &lines[1].spans[0];
        assert_eq!(top.runs.len(), 2);
        // Reversed: rightmost run first.
        assert!((top.runs[0].x0 - 1.4).abs() < 1e-9 && (top.runs[0].x1 - 1.5).abs() < 1e-9);
        assert!((top.runs[1].x0 - 1.1).abs() < 1e-9 && (top.runs[1].x1 - 1.3).abs() < 1e-9);
        assert!((top.travel_start - 2.5).abs() < 1e-9 && (top.travel_end - 0.1).abs() < 1e-9);
    }

    #[test]
    fn blank_skip_splits_spans() {
        let mut row = vec![0u8; 200];
        row[0] = 255;
        row[10] = 255;
        row[150] = 255;
        let p = img(0, 0, &[&row], true);
        let s = ScanSettings { blank_skip_mm: Some(5.0), overscan_mm: 0.5, bidirectional: false, ..Default::default() };
        let plan = &scan_plans(&[&p], &s)[0];
        let spans = &plan.lines[0].spans;
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].runs.len(), 2);
        assert!((spans[1].travel_start - 14.5).abs() < 1e-9);
    }

    #[test]
    fn grayscale_power_mapping() {
        let p = img(0, 0, &[&[0, 1, 255, 128]], false);
        let s = ScanSettings { min_power: 20.0, max_power: 80.0, ..settings() };
        let runs = &scan_plans(&[&p], &s)[0].lines[0].spans[0].runs;
        assert_eq!(runs.len(), 3);
        assert!((runs[0].power - (20.0 + 60.0 / 255.0)).abs() < 1e-4);
        assert!((runs[1].power - 80.0).abs() < 1e-4);
        assert!((runs[2].power - (20.0 + 60.0 * 128.0 / 255.0)).abs() < 1e-4);
    }

    #[test]
    fn images_on_one_lattice_merge() {
        let a = img(0, 0, &[&[255, 255, 0, 0]], true);
        let b = img(2, 1, &[&[0, 0, 0], &[255, 255, 255]], true);
        let plans = scan_plans(&[&a, &b], &settings());
        assert_eq!(plans.len(), 1);
        let lines = &plans[0].lines;
        assert_eq!(lines.len(), 1, "row 1 of b is blank");
        let runs = &lines[0].spans[0].runs;
        assert_eq!(runs.len(), 1);
        assert!((runs[0].x0 - 0.0).abs() < 1e-9 && (runs[0].x1 - 0.5).abs() < 1e-9);
    }

    #[test]
    fn overlap_takes_max_power() {
        let merged = merge_runs(vec![(0, 10, 30.0), (5, 15, 50.0), (12, 13, 90.0)]);
        assert_eq!(merged, vec![(0, 5, 30.0), (5, 12, 50.0), (12, 13, 90.0), (13, 15, 50.0)]);
    }

    #[test]
    fn different_angles_stay_separate() {
        let a = img(0, 0, &[&[255]], true);
        let mut b = img(0, 0, &[&[255]], true);
        b.grid.angle_deg = 90.0;
        assert_eq!(scan_plans(&[&a, &b], &settings()).len(), 2);
    }

    #[test]
    fn dot_width_shortens_but_keeps_dots() {
        let p = img(0, 0, &[&[255, 255, 255, 255, 0, 255, 0, 0]], true);
        let s = ScanSettings { dot_width_mm: 0.1, bidirectional: false, ..settings() };
        let runs = &scan_plans(&[&p], &s)[0].lines[0].spans[0].runs;
        assert!((runs[0].x0 - 0.05).abs() < 1e-9 && (runs[0].x1 - 0.35).abs() < 1e-9);
        assert!((runs[1].x0 - 0.525).abs() < 1e-9 && (runs[1].x1 - 0.575).abs() < 1e-9);
    }

    #[test]
    fn rotated_plan_points() {
        let mut p = img(0, 0, &[&[255]], true);
        p.grid.angle_deg = 90.0;
        let plan = &scan_plans(&[&p], &settings())[0];
        let line = &plan.lines[0];
        let pt = plan.point_mm(line, 0.1);
        assert!((pt.x + 0.05).abs() < 1e-9 && (pt.y - 0.1).abs() < 1e-9);
        assert!((plan.burn_length() - 0.1).abs() < 1e-9);
    }
}
