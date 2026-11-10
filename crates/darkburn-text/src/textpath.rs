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

/// The most pieces one straight stretch of a letter is cut into before it is bent.
const MAX_PIECES: f64 = 4096.0;

fn flatten_fine(path: &BezPath, max_seg: f64) -> Vec<(Vec<Point>, bool)> {
    geomutil::flatten_polylines(path, 0.002)
        .into_iter()
        .map(|(pts, closed)| {
            let mut out: Vec<Point> = Vec::with_capacity(pts.len());
            // A closed outline has one more edge than it has points between them: the one that
            // closes it. It bends like the others.
            let mut ring = pts.clone();
            if closed && ring.first() != ring.last() {
                ring.push(ring[0]);
            }
            for w in ring.windows(2) {
                let (a, b) = (w[0], w[1]);
                // Enough pieces to follow the bend, but never so many that an enormous letter
                // fills the memory.
                let n = (((b - a).hypot()) / max_seg).ceil().clamp(1.0, MAX_PIECES) as usize;
                for k in 0..n {
                    out.push(a.lerp(b, k as f64 / n as f64));
                }
            }
            if let Some(l) = pts.last().filter(|_| !closed) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{LineMetrics, TextMetrics};
    use kurbo::{Rect, Shape};

    fn one_glyph(size: f64) -> TextLayout {
        let path = Rect::new(0.0, 0.0, size, size).to_path(0.1);
        let glyph = GlyphOutline { text: "I".into(), line: 0, path, origin_x: 0.0, baseline_y: 0.0, advance: size, rtl: false, missing: false };
        TextLayout {
            glyphs: vec![glyph],
            lines: vec![LineMetrics { baseline_y: 0.0, width: size, x_start: 0.0 }],
            metrics: TextMetrics::default(),
            bounds: Rect::new(0.0, 0.0, size, size),
            family_used: String::new(),
        }
    }

    fn corner() -> BezPath {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((50.0, 0.0));
        p.line_to((50.0, 50.0));
        p
    }

    #[test]
    fn the_edge_that_closes_an_outline_bends_too() {
        // A flat bar whose closing edge is its long bottom, bent around a circle of radius 10.
        let mut bar = BezPath::new();
        bar.move_to((0.0, 0.0));
        bar.line_to((0.0, 1.0));
        bar.line_to((30.0, 1.0));
        bar.line_to((30.0, 0.0));
        bar.close_path();
        let mut layout = one_glyph(30.0);
        layout.glyphs[0].path = bar;
        let circle = kurbo::Circle::new((0.0, 0.0), 10.0).to_path(1e-4);
        let bent = text_on_path(&layout, &circle, &PathTextOptions { distort: true, ..Default::default() });
        let pts: Vec<Point> = geomutil::flatten_polylines(&bent[0].path, 0.001).into_iter().flat_map(|(p, _)| p).collect();
        let n = pts.len();
        for i in 0..n {
            let (a, b) = (pts[i], pts[(i + 1) % n]);
            let r = a.midpoint(b).to_vec2().hypot();
            assert!((8.9..=11.1).contains(&r), "an edge from {a:?} to {b:?} cuts across the circle (radius {r})");
        }
    }

    #[test]
    fn bending_an_enormous_letter_does_not_fill_the_memory() {
        let opts = PathTextOptions { distort: true, ..Default::default() };
        // A letter a thousand kilometres tall used to ask for tens of gigabytes here.
        let bent = text_on_path(&one_glyph(1e9), &corner(), &opts);
        assert_eq!(bent.len(), 1);
        let n = node_count(&bent[0].path);
        assert!(n > 4 && n <= 4 * 4096 + 8, "{n}");
        // An ordinary letter is still cut finely enough to follow the bend.
        let small = text_on_path(&one_glyph(10.0), &corner(), &opts);
        assert!(node_count(&small[0].path) >= 4 * 39, "{}", node_count(&small[0].path));
    }
}
