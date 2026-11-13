//! Sheet nesting: packs parts into a sheet so they use little material.
//!
//! Two methods share one set of options and one result:
//!
//! - [`quick_nest`] packs the parts' bounding boxes (a max-rects packer). It is fast and good
//!   for rectangular parts, but parts never interlock.
//! - [`true_nest`] packs the parts' real outlines: every part drops into the lowest, then
//!   leftmost spot it can slide to over a set of start positions and rotations. Parts
//!   interlock, and small parts can go into the cut-out holes of bigger ones. Several passes
//!   with different part orders run until the time limit and the best one wins.
//!
//! Parts fill the sheet from its low corner (smallest x, then smallest y). When one sheet is
//! not enough and more are allowed, further sheets of the same size are laid out side by side
//! in +x, [`SheetNestOptions::sheet_spacing`] apart; see [`sheet_rect`].
//!
//! Obstacles (shapes that must stay where they are) count on the first sheet only. A closed
//! obstacle contour that surrounds the whole usable sheet (a drawn material outline) counts by
//! its line only, so the parts can go inside it.
//!
//! Every result is checked before it is returned: placed parts keep the gap (within the
//! tolerance) from each other and from obstacles, stay inside the sheet margin, and never sit
//! inside each other except in a usable hole. Bad input gives an error instead of a
//! panic, and every run stops at its time limit.

mod prep;
mod quick;
mod shapes;
mod space;
#[cfg(test)]
mod tests;

use crate::path::bounding_box_all;
use kurbo::{Affine, BezPath, PathEl, Point, Rect, Vec2};
use std::fmt;
use std::time::{Duration, Instant};

/// Most parts plus obstacles one run takes.
pub const MAX_PARTS: usize = 20_000;
/// Most path elements (all parts and obstacles together) one run takes.
pub const MAX_ELEMENTS: usize = 2_000_000;
/// Largest coordinate accepted, mm.
pub const MAX_COORD: f64 = 1e9;
/// Most sheets one run may use.
pub const MAX_SHEETS: usize = 64;
/// Most rotations tried per part; a finer list is thinned out evenly.
pub const MAX_ROTATIONS: usize = 72;
/// Longest time limit, seconds.
pub const MAX_TIME_LIMIT: f64 = 3600.0;

/// One part to nest. Its pieces move together and keep their relative positions.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NestPart {
    /// Contours that cut through the material. A closed cut that lies inside another closed
    /// contour of the part and has nothing of the part inside it is a hole, which other parts
    /// may use when [`SheetNestOptions::allow_holes`] is on.
    pub cuts: Vec<BezPath>,
    /// Everything else (engraving, fills, score lines): takes room, never makes a hole.
    pub marks: Vec<BezPath>,
}

impl NestPart {
    /// A part made of cuts only.
    pub fn cut(paths: Vec<BezPath>) -> Self {
        Self { cuts: paths, marks: Vec::new() }
    }

    fn all(&self) -> impl Iterator<Item = &BezPath> {
        self.cuts.iter().chain(&self.marks)
    }
}

/// Options shared by [`quick_nest`] and [`true_nest`].
#[derive(Clone, Debug, PartialEq)]
pub struct SheetNestOptions {
    /// Smallest distance kept between parts, and between parts and obstacles, mm.
    pub gap: f64,
    /// Empty border kept inside the sheet edge, mm.
    pub margin: f64,
    /// Rotations tried for every part, degrees. Empty means no rotation.
    pub rotations: Vec<f64>,
    /// Let small parts go into the holes of bigger parts (and of obstacles). True nest only.
    pub allow_holes: bool,
    /// Most sheets to use (1 = only the given sheet).
    pub max_sheets: usize,
    /// Distance between sheets laid out side by side, mm.
    pub sheet_spacing: f64,
    /// Time after which the run stops and returns the best result so far.
    pub time_limit: Duration,
    /// Curve flattening tolerance, mm; the gap is kept within this.
    pub tolerance: f64,
}

impl Default for SheetNestOptions {
    fn default() -> Self {
        Self {
            gap: 2.0,
            margin: 0.0,
            rotations: quarter_turns(true),
            allow_holes: true,
            max_sheets: 1,
            sheet_spacing: 10.0,
            time_limit: Duration::from_secs(10),
            tolerance: 0.05,
        }
    }
}

/// The four quarter turns, or no rotation.
pub fn quarter_turns(on: bool) -> Vec<f64> {
    if on { vec![0.0, 90.0, 180.0, 270.0] } else { vec![0.0] }
}

/// Progress of a run, in placement steps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NestProgress {
    pub done: usize,
    pub total: usize,
}

/// Result of a nesting run.
#[derive(Clone, Debug, PartialEq)]
pub struct SheetNestResult {
    /// Transform per part (from where it is now to where it goes), or `None` when it was not
    /// placed.
    pub placements: Vec<Option<Affine>>,
    /// Sheet index per placed part.
    pub sheets: Vec<Option<usize>>,
    /// Number of sheets with at least one part.
    pub sheets_used: usize,
    /// Number of parts not placed.
    pub unplaced: usize,
    /// Placed part area over the usable area of the sheets used (0..=1).
    pub utilization: f64,
    /// The run was cancelled or ran out of time before it finished every pass.
    pub stopped: bool,
}

/// Why a run did not start.
#[derive(Clone, Debug, PartialEq)]
pub enum SheetNestError {
    /// The sheet is empty, not finite, or has no room inside the margin.
    Sheet,
    /// A coordinate is not a finite number or lies beyond [`MAX_COORD`].
    NotFinite,
    /// More than [`MAX_PARTS`] parts and obstacles.
    TooManyParts(usize),
    /// More than [`MAX_ELEMENTS`] path elements in all.
    TooComplex,
    /// An option is out of range; names the option.
    Option(&'static str),
}

impl fmt::Display for SheetNestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sheet => write!(f, "the sheet has no room inside its margin"),
            Self::NotFinite => write!(f, "a shape has a coordinate that is not a usable number"),
            Self::TooManyParts(n) => write!(f, "{n} shapes is more than the {MAX_PARTS} nesting takes"),
            Self::TooComplex => write!(f, "the shapes have more than {MAX_ELEMENTS} path elements"),
            Self::Option(name) => write!(f, "the {name} is out of range"),
        }
    }
}

impl std::error::Error for SheetNestError {}

/// Sheet number `k` when sheets are laid out side by side in +x, `spacing` apart.
pub fn sheet_rect(sheet: Rect, spacing: f64, k: usize) -> Rect {
    sheet + Vec2::new(k as f64 * (sheet.width() + spacing.max(0.0)), 0.0)
}

/// Checked and clamped run settings.
#[derive(Clone, Debug)]
struct Run {
    sheet: Rect,
    usable: Rect,
    gap: f64,
    rotations: Vec<f64>,
    allow_holes: bool,
    max_sheets: usize,
    spacing: f64,
    tol: f64,
    deadline: Instant,
}

impl Run {
    /// Usable area of sheet `k`.
    fn usable(&self, k: usize) -> Rect {
        self.usable + (sheet_rect(self.sheet, self.spacing, k).origin() - self.sheet.origin())
    }
}

fn finite_rect(r: Rect) -> bool {
    [r.x0, r.y0, r.x1, r.y1].iter().all(|v| v.is_finite() && v.abs() <= MAX_COORD)
}

fn option(v: f64, name: &'static str, max: f64) -> Result<f64, SheetNestError> {
    if v.is_finite() && (0.0..=max).contains(&v) { Ok(v) } else { Err(SheetNestError::Option(name)) }
}

/// Normalises rotations to `[0, 360)`, drops repeats and thins a long list evenly.
fn rotations(list: &[f64]) -> Result<Vec<f64>, SheetNestError> {
    let mut out: Vec<f64> = Vec::new();
    for &a in list {
        if !a.is_finite() || a.abs() > 1e6 {
            return Err(SheetNestError::Option("rotation"));
        }
        let a = a.rem_euclid(360.0);
        let a = if a > 360.0 - 1e-9 { 0.0 } else { a };
        if !out.iter().any(|b| (a - b).abs() < 1e-9) {
            out.push(a);
        }
    }
    if out.is_empty() {
        out.push(0.0);
    }
    if out.len() > MAX_ROTATIONS {
        let n = out.len();
        out = (0..MAX_ROTATIONS).map(|i| out[i * n / MAX_ROTATIONS]).collect();
    }
    Ok(out)
}

fn check_paths<'a>(paths: impl Iterator<Item = &'a BezPath>, elements: &mut usize) -> Result<(), SheetNestError> {
    for p in paths {
        *elements += p.elements().len();
        if *elements > MAX_ELEMENTS {
            return Err(SheetNestError::TooComplex);
        }
        for el in p.elements() {
            let ok = |q: &Point| q.x.is_finite() && q.y.is_finite() && q.x.abs() <= MAX_COORD && q.y.abs() <= MAX_COORD;
            let fine = match el {
                PathEl::MoveTo(a) | PathEl::LineTo(a) => ok(a),
                PathEl::QuadTo(a, b) => ok(a) && ok(b),
                PathEl::CurveTo(a, b, c) => ok(a) && ok(b) && ok(c),
                PathEl::ClosePath => true,
            };
            if !fine {
                return Err(SheetNestError::NotFinite);
            }
        }
    }
    Ok(())
}

fn check(parts: &[NestPart], obstacles: &[NestPart], sheet: Rect, o: &SheetNestOptions) -> Result<Run, SheetNestError> {
    let count = parts.len() + obstacles.len();
    if count > MAX_PARTS {
        return Err(SheetNestError::TooManyParts(count));
    }
    let gap = option(o.gap, "gap", 1e5)?;
    let margin = option(o.margin, "margin", 1e6)?;
    let spacing = option(o.sheet_spacing, "sheet spacing", 1e6)?;
    if !o.tolerance.is_finite() || o.tolerance <= 0.0 {
        return Err(SheetNestError::Option("tolerance"));
    }
    let tol = o.tolerance.clamp(1e-3, 1.0);
    let sheet = sheet.abs();
    if !finite_rect(sheet) || sheet.width() <= 0.0 || sheet.height() <= 0.0 {
        return Err(SheetNestError::Sheet);
    }
    let usable = sheet.inset(-margin);
    if usable.width() <= 1e-9 || usable.height() <= 1e-9 {
        return Err(SheetNestError::Sheet);
    }
    let max_sheets = o.max_sheets.clamp(1, MAX_SHEETS);
    if !finite_rect(sheet_rect(sheet, spacing, max_sheets)) {
        return Err(SheetNestError::Sheet);
    }
    let mut elements = 0;
    check_paths(parts.iter().flat_map(NestPart::all), &mut elements)?;
    check_paths(obstacles.iter().flat_map(NestPart::all), &mut elements)?;
    let secs = o.time_limit.as_secs_f64().clamp(0.0, MAX_TIME_LIMIT);
    Ok(Run {
        sheet,
        usable,
        gap,
        rotations: rotations(&o.rotations)?,
        allow_holes: o.allow_holes,
        max_sheets,
        spacing,
        tol,
        deadline: Instant::now() + Duration::from_secs_f64(secs),
    })
}

/// Counts steps, reports them and decides when to stop.
struct Ticker<'a> {
    done: usize,
    total: usize,
    deadline: Instant,
    report: &'a mut dyn FnMut(NestProgress) -> bool,
    stopped: bool,
}

impl Ticker<'_> {
    /// Reports progress; true when the run must stop (cancelled or out of time).
    fn stop(&mut self) -> bool {
        if !self.stopped {
            let p = NestProgress { done: self.done.min(self.total), total: self.total };
            self.stopped = !(self.report)(p) || Instant::now() >= self.deadline;
        }
        self.stopped
    }

    fn step(&mut self) -> bool {
        self.done += 1;
        self.stop()
    }
}

/// A placed part: transform and sheet.
type Placed = Option<(Affine, usize)>;

fn finish(placed: Vec<Placed>, areas: &[f64], usable: Rect, stopped: bool) -> SheetNestResult {
    let sheets_used = placed.iter().flatten().map(|p| p.1 + 1).max().unwrap_or(0);
    let unplaced = placed.iter().filter(|p| p.is_none()).count();
    let area: f64 = placed.iter().zip(areas).filter(|(p, _)| p.is_some()).map(|(_, a)| a).sum();
    let utilization = if sheets_used == 0 { 0.0 } else { (area / (usable.area() * sheets_used as f64)).clamp(0.0, 1.0) };
    SheetNestResult {
        placements: placed.iter().map(|p| p.map(|p| p.0)).collect(),
        sheets: placed.iter().map(|p| p.map(|p| p.1)).collect(),
        sheets_used,
        unplaced,
        utilization,
        stopped,
    }
}

/// Orders tried by the passes: biggest area, tallest, widest and longest side first. Ties keep
/// the input order, so runs are repeatable. Repeated orders are dropped.
fn orders(n: usize, key: impl Fn(usize) -> (f64, f64, f64)) -> Vec<Vec<usize>> {
    let keys: Vec<(f64, f64, f64)> = (0..n).map(key).collect();
    let by = |f: &dyn Fn(&(f64, f64, f64)) -> f64| {
        let mut v: Vec<usize> = (0..n).collect();
        v.sort_by(|&a, &b| f(&keys[b]).total_cmp(&f(&keys[a])).then(a.cmp(&b)));
        v
    };
    let mut out: Vec<Vec<usize>> = Vec::new();
    for o in [by(&|k| k.0), by(&|k| k.2), by(&|k| k.1), by(&|k| k.1.max(k.2))] {
        if !out.contains(&o) {
            out.push(o);
        }
    }
    out
}

/// Nests parts by their bounding boxes. `obstacles` stay put and are kept clear of by their
/// contours' bounding boxes. `progress` is called after every part; returning false stops the
/// run, which then returns what it placed so far.
pub fn quick_nest(
    parts: &[NestPart],
    obstacles: &[NestPart],
    sheet: Rect,
    o: &SheetNestOptions,
    progress: &mut dyn FnMut(NestProgress) -> bool,
) -> Result<SheetNestResult, SheetNestError> {
    let run = check(parts, obstacles, sheet, o)?;
    Ok(quick::nest(parts, obstacles, &run, progress))
}

/// Nests parts by their real outlines. `obstacles` stay put and are kept clear of by their
/// outlines. `progress` is called as parts are placed; returning false stops the run, which then
/// returns the best result found so far.
pub fn true_nest(
    parts: &[NestPart],
    obstacles: &[NestPart],
    sheet: Rect,
    o: &SheetNestOptions,
    progress: &mut dyn FnMut(NestProgress) -> bool,
) -> Result<SheetNestResult, SheetNestError> {
    let run = check(parts, obstacles, sheet, o)?;
    Ok(shapes::nest(parts, obstacles, &run, progress))
}

/// Bounding box of a part (all its pieces), or `None` when it has none.
pub fn part_bounds(part: &NestPart) -> Option<Rect> {
    let all: Vec<BezPath> = part.all().cloned().collect();
    bounding_box_all(&all)
}
