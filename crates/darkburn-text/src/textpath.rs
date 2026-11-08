//! Text on a path ("bend") and "apply path to text".
//!
//! The layout's x axis becomes distance along the path and its y axis becomes the offset to the left
//! of the path direction. So with the default bottom alignment the baseline sits on the path and
//! the letters stand on its left side.

use crate::geomutil;
use crate::layout::{GlyphOutline, TextLayout};
use kurbo::{Affine, BezPath, PathEl, Point, Vec2};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PathTextOptions {
    /// Distance along the path where layout x = 0 is placed (mm).
    pub start_offset: f64,
    /// Place layout x = 0 at the middle of the path instead (handy for centred text).
    pub center_on_path: bool,
    /// Perpendicular shift (mm), positive to the left of the path direction.
    pub offset: f64,
    /// Walk the path backwards, which puts the text on the other side of the path.
    pub flip: bool,
    /// Bend the outlines to follow the curve; otherwise each glyph stays rigid and only rotates.
    pub distort: bool,
}

impl Default for PathTextOptions {
    fn default() -> Self {
        Self { start_offset: 0.0, center_on_path: false, offset: 0.0, flip: false, distort: false }
    }
}

/// Arc-length sampler over the first sub-path of a path.
pub struct PathSampler {
    pts: Vec<Point>,
    cum: Vec<f64>,
    closed: bool,
}

impl PathSampler {
    /// Uses the longest sub-path of `path`.
    pub fn new(path: &BezPath, tol: f64) -> Option<PathSampler> {
        let best = geomutil::flatten_polylines(path, tol)
            .into_iter()
            .map(|(mut pts, closed)| {
                if closed && pts.first() != pts.last() {
                    let f = pts[0];
                    pts.push(f);
                }
                pts.dedup_by(|a, b| (*a - *b).hypot() < 1e-12);
                (pts, closed)
            })
            .filter(|(p, _)| p.len() >= 2)
            .max_by(|a, b| len_of(&a.0).total_cmp(&len_of(&b.0)))?;
        let (pts, closed) = best;
        let mut cum = vec![0.0];
        for w in pts.windows(2) {
            cum.push(cum.last().unwrap() + (w[1] - w[0]).hypot());
        }
        Some(PathSampler { pts, cum, closed })
    }

    pub fn length(&self) -> f64 {
        *self.cum.last().unwrap()
    }

    pub fn is_closed(&self) -> bool {
        self.closed
    }

    fn reversed(&self) -> PathSampler {
        let pts: Vec<Point> = self.pts.iter().rev().copied().collect();
        let mut cum = vec![0.0];
        for w in pts.windows(2) {
            cum.push(cum.last().unwrap() + (w[1] - w[0]).hypot());
        }
        PathSampler { pts, cum, closed: self.closed }
    }

    /// Point and unit tangent at distance `s`. Closed paths wrap; open paths extrapolate along the end tangents.
    pub fn at(&self, s: f64) -> (Point, Vec2) {
        let total = self.length();
        let s = if self.closed && total > 0.0 { s.rem_euclid(total) } else { s };
        let n = self.pts.len();
        let seg = if s <= 0.0 {
            0
        } else if s >= total {
            n - 2
        } else {
            match self.cum.binary_search_by(|c| c.total_cmp(&s)) {
                Ok(i) => i.min(n - 2),
                Err(i) => i - 1,
            }
        };
        let a = self.pts[seg];
        let b = self.pts[seg + 1];
        let l = (b - a).hypot();
        let t = (b - a) / l;
        let u = s - self.cum[seg];
        (a + t * u, t)
    }
}

fn len_of(p: &[Point]) -> f64 {
    p.windows(2).map(|w| (w[1] - w[0]).hypot()).sum()
}

fn flatten_fine(path: &BezPath, max_seg: f64) -> Vec<(Vec<Point>, bool)> {
    geomutil::flatten_polylines(path, 0.002)
        .into_iter()
        .map(|(pts, closed)| {
            let mut out: Vec<Point> = Vec::with_capacity(pts.len());
            for w in pts.windows(2) {
                let (a, b) = (w[0], w[1]);
                let n = (((b - a).hypot()) / max_seg).ceil().max(1.0) as usize;
                for k in 0..n {
                    out.push(a.lerp(b, k as f64 / n as f64));
                }
            }
            if let Some(l) = pts.last() {
                out.push(*l);
            }
            (out, closed)
        })
        .collect()
}

/// Place a laid-out text on `path`. Returns new glyphs (same text/advance metadata, new outlines).
pub fn text_on_path(layout: &TextLayout, path: &BezPath, opts: &PathTextOptions) -> Vec<GlyphOutline> {
    let Some(base) = PathSampler::new(path, 0.01) else { return Vec::new() };
    let sampler = if opts.flip { base.reversed() } else { base };
    let start = opts.start_offset + if opts.center_on_path { sampler.length() / 2.0 } else { 0.0 };
    let mut out = Vec::with_capacity(layout.glyphs.len());
    for g in &layout.glyphs {
        let mut ng = g.clone();
        if g.path.elements().is_empty() {
            ng.path = BezPath::new();
            out.push(ng);
            continue;
        }
        if opts.distort {
            let lines = flatten_fine(&g.path, 0.25);
            let mapped: Vec<(Vec<Point>, bool)> = lines
                .into_iter()
                .map(|(pts, closed)| {
                    let pts = pts
                        .into_iter()
                        .map(|p| {
                            let (q, t) = sampler.at(start + p.x);
                            let n = Vec2::new(-t.y, t.x);
                            q + n * (p.y + opts.offset)
                        })
                        .collect();
                    (pts, closed)
                })
                .collect();
            ng.path = geomutil::polylines_to_path(&mapped);
        } else {
            // Rigid: anchor at the middle of the glyph's advance.
            let half = g.advance / 2.0;
            let xc = g.origin_x + half;
            let (q, t) = sampler.at(start + xc);
            let n = Vec2::new(-t.y, t.x);
            let rot = Affine::new([t.x, t.y, -t.y, t.x, 0.0, 0.0]);
            // Layout y stays the offset from the path (so multi-line and vertical alignment survive).
            let anchor = Point::new(xc, 0.0);
            ng.path = Affine::translate(q.to_vec2() + n * opts.offset) * rot * Affine::translate(-anchor.to_vec2()) * g.path.clone();
        }
        out.push(ng);
    }
    out
}

/// SHAPE-28: take a plain text layout and a drawn path and return the bent glyph outlines
/// (distorted, left-to-right along the path, text on the path's left).
pub fn apply_path_to_text(layout: &TextLayout, path: &BezPath) -> Vec<BezPath> {
    let opts = PathTextOptions { distort: true, ..Default::default() };
    text_on_path(layout, path, &opts).into_iter().map(|g| g.path).filter(|p| !p.elements().is_empty()).collect()
}

/// Number of nodes in a path (test/UI helper).
pub fn node_count(p: &BezPath) -> usize {
    p.elements().iter().filter(|e| !matches!(e, PathEl::ClosePath)).count()
}
