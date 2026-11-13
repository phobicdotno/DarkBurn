//! Quick nest: a max-rects packer over the parts' bounding boxes.
//!
//! Every part takes the free rectangle spot whose top edge is lowest (then leftmost) over its
//! allowed rotations. Parts are padded by the gap on their right and top; the free space of a
//! sheet is its usable area grown by the gap on the same sides, so the far edges keep the
//! margin and neighbours keep the gap.

use super::{NestPart, NestProgress, Placed, Run, SheetNestResult, Ticker, finish, orders};
use crate::path::{bounding_box, contours, flatten_contour};
use kurbo::{Affine, Point, Rect};

/// One way a part can lie: its rotation and the bounding box after rotating.
#[derive(Clone, Copy, Debug)]
struct Variant {
    rot: Affine,
    bbox: Rect,
}

struct Part {
    variants: Vec<Variant>,
    /// Bounding box where the part is now.
    orig: Rect,
    area: f64,
}

/// Free space of one sheet.
struct Bin {
    free: Vec<Rect>,
}

const EPS: f64 = 1e-9;

fn overlaps(a: &Rect, b: &Rect) -> bool {
    a.x0 < b.x1 - EPS && a.x1 > b.x0 + EPS && a.y0 < b.y1 - EPS && a.y1 > b.y0 + EPS
}

fn contains(outer: &Rect, inner: &Rect) -> bool {
    outer.x0 <= inner.x0 + EPS && outer.y0 <= inner.y0 + EPS && outer.x1 >= inner.x1 - EPS && outer.y1 >= inner.y1 - EPS
}

impl Bin {
    fn new(domain: Rect) -> Self {
        Self { free: vec![domain] }
    }

    /// Lowest-top, then leftmost, spot for a `w` × `h` box: (x, y, top).
    fn find(&self, w: f64, h: f64) -> Option<(f64, f64)> {
        let mut best: Option<(f64, f64)> = None;
        for f in &self.free {
            if f.width() + EPS >= w && f.height() + EPS >= h {
                let better = match best {
                    None => true,
                    Some((bx, by)) => (f.y0, f.x0) < (by, bx),
                };
                if better {
                    best = Some((f.x0, f.y0));
                }
            }
        }
        best
    }

    /// Marks `r` as used: splits every free rectangle it overlaps into the maximal pieces
    /// around it and drops pieces that lie inside another free rectangle.
    fn occupy(&mut self, r: Rect) {
        let mut added: Vec<Rect> = Vec::new();
        self.free.retain(|f| {
            if !overlaps(f, &r) {
                return true;
            }
            let pieces = [
                Rect::new(f.x0, f.y0, r.x0, f.y1),
                Rect::new(r.x1, f.y0, f.x1, f.y1),
                Rect::new(f.x0, f.y0, f.x1, r.y0),
                Rect::new(f.x0, r.y1, f.x1, f.y1),
            ];
            added.extend(pieces.into_iter().filter(|p| p.x1 - p.x0 > EPS && p.y1 - p.y0 > EPS && contains(f, p)));
            false
        });
        let keep: Vec<bool> = (0..added.len())
            .map(|i| {
                let a = &added[i];
                !self.free.iter().any(|f| contains(f, a))
                    && !added.iter().enumerate().any(|(j, b)| j != i && contains(b, a) && (!contains(a, b) || j < i))
            })
            .collect();
        self.free.extend(added.into_iter().zip(keep).filter(|(_, k)| *k).map(|(a, _)| a));
    }
}

/// Bounding boxes the parts must keep clear of: one per obstacle contour. A closed contour
/// around the whole usable sheet counts by its line, one box per flattened edge.
fn obstacle_boxes(obstacles: &[NestPart], run: &Run) -> Vec<Rect> {
    let around = run.usable.inflate(-run.tol, -run.tol);
    let mut out = Vec::new();
    for c in obstacles.iter().flat_map(NestPart::all).flat_map(contours) {
        let Some(b) = c.bounding_box() else { continue };
        if c.is_closed_within(run.tol) && contains(&b, &around) {
            let pl = flatten_contour(&c, run.tol);
            let n = pl.points.len();
            for i in 0..n {
                let (a, z) = (pl.points[i], pl.points[(i + 1) % n]);
                out.push(Rect::from_points(a, z));
            }
        } else {
            out.push(b);
        }
    }
    out
}

fn variants(part: &NestPart, rotations: &[f64]) -> Vec<Variant> {
    let mut out: Vec<Variant> = Vec::new();
    for &deg in rotations {
        let rot = Affine::rotate(deg.to_radians());
        let bbox = part.all().filter_map(|p| bounding_box(&(rot * p.clone()))).reduce(|a, b| a.union(b));
        let Some(bbox) = bbox else { continue };
        let same = |v: &Variant| (v.bbox.width() - bbox.width()).abs() < 1e-9 && (v.bbox.height() - bbox.height()).abs() < 1e-9;
        if !out.iter().any(same) {
            out.push(Variant { rot, bbox });
        }
    }
    out
}

/// Area of a part for the utilization figure: its bounding box when it has no closed contour.
fn part_area(part: &NestPart, bbox: Rect, tol: f64) -> f64 {
    let a: f64 = part.all().map(|p| crate::path::signed_area(p).abs()).sum();
    if a > tol * tol { a.min(bbox.area()) } else { 0.0 }
}

fn pass(parts: &[Part], order: &[usize], obstacles: &[Rect], run: &Run, tick: &mut Ticker) -> Vec<Placed> {
    let g = run.gap;
    let domain = |k: usize| {
        let u = run.usable(k);
        Rect::new(u.x0, u.y0, u.x1 + g, u.y1 + g)
    };
    let mut bins: Vec<Bin> = Vec::new();
    let mut placed: Vec<Placed> = vec![None; parts.len()];
    for &i in order {
        if tick.step() {
            break;
        }
        let part = &parts[i];
        let mut k = 0;
        loop {
            if k == bins.len() {
                if k >= run.max_sheets {
                    break;
                }
                let mut bin = Bin::new(domain(k));
                if k == 0 {
                    for o in obstacles {
                        let r = Rect::new(o.x0, o.y0, o.x1 + g, o.y1 + g);
                        if overlaps(&r, &domain(0)) {
                            bin.occupy(r);
                        }
                    }
                }
                bins.push(bin);
            }
            let mut best: Option<(f64, f64, usize)> = None;
            for (vi, v) in part.variants.iter().enumerate() {
                let (w, h) = (v.bbox.width() + g, v.bbox.height() + g);
                if let Some((x, y)) = bins[k].find(w, h) {
                    let better = match best {
                        None => true,
                        Some((bx, by, bvi)) => {
                            let bh = part.variants[bvi].bbox.height();
                            (y + v.bbox.height(), x) < (by + bh, bx)
                        }
                    };
                    if better {
                        best = Some((x, y, vi));
                    }
                }
            }
            if let Some((x, y, vi)) = best {
                let v = part.variants[vi];
                bins[k].occupy(Rect::new(x, y, x + v.bbox.width() + g, y + v.bbox.height() + g));
                let xf = Affine::translate(Point::new(x, y) - v.bbox.origin()) * v.rot;
                placed[i] = Some((xf, k));
                break;
            }
            // A part that does not fit an empty sheet never fits a new one either.
            let fresh = k > 0 && bins[k].free.len() == 1 && contains(&bins[k].free[0], &domain(k));
            if fresh {
                bins.pop();
                break;
            }
            k += 1;
        }
    }
    placed
}

fn pass_key(placed: &[Placed], parts: &[Part]) -> (usize, usize, f64, f64) {
    let unplaced = placed.iter().filter(|p| p.is_none()).count();
    let sheets = placed.iter().flatten().map(|p| p.1 + 1).max().unwrap_or(0);
    let mut top = 0.0f64;
    let mut right = 0.0f64;
    for (p, part) in placed.iter().zip(parts) {
        if let Some((xf, k)) = p
            && k + 1 == sheets
        {
            let b = xf.transform_rect_bbox(part.orig);
            top = top.max(b.y1);
            right = right.max(b.x1);
        }
    }
    (unplaced, sheets, top, right)
}

pub(super) fn nest(input: &[NestPart], obstacles: &[NestPart], run: &Run, report: &mut dyn FnMut(NestProgress) -> bool) -> SheetNestResult {
    let parts: Vec<Part> = input
        .iter()
        .map(|p| {
            let variants = variants(p, &run.rotations);
            let orig = super::part_bounds(p).unwrap_or_default();
            let area = if variants.is_empty() { 0.0 } else { part_area(p, orig, run.tol) };
            Part { variants, orig, area }
        })
        .collect();
    let boxes = obstacle_boxes(obstacles, run);
    let orders = orders(parts.len(), |i| {
        let b = parts[i].variants.first().map(|v| v.bbox).unwrap_or_default();
        (b.area(), b.width(), b.height())
    });
    let orders = &orders[..orders.len().min(2)];
    let mut tick = Ticker { done: 0, total: parts.len() * orders.len(), deadline: run.deadline, report, stopped: false };
    let mut best: Option<(Vec<Placed>, (usize, usize, f64, f64))> = None;
    for order in orders {
        let placed = pass(&parts, order, &boxes, run, &mut tick);
        let key = pass_key(&placed, &parts);
        let better = match &best {
            None => true,
            Some((_, b)) => key.0 < b.0 || (key.0 == b.0 && (key.1, key.2, key.3) < (b.1, b.2, b.3)),
        };
        if better {
            best = Some((placed, key));
        }
        if tick.stopped {
            break;
        }
    }
    let placed = best.map(|b| b.0).unwrap_or_else(|| vec![None; parts.len()]);
    let areas: Vec<f64> = parts.iter().map(|p| p.area).collect();
    finish(placed, &areas, run.usable, tick.stopped)
}
