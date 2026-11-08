//! Shared geometry helpers: bulge arcs, joining open paths, circle and arc recognition.

use std::collections::HashMap;

use kurbo::{Arc, BezPath, ParamCurve, PathEl, PathSeg, Point, Vec2};

use crate::model::{Item, ItemKind};

/// Tolerance used when turning arcs into cubic Béziers (mm).
pub const ARC_TOLERANCE: f64 = 1e-4;

/// Appends an arc from the current point `p0` to `p1` with DXF bulge `b`
/// (`tan(sweep/4)`, positive = counter-clockwise). A zero bulge is a straight line.
pub fn bulge_to(path: &mut BezPath, p0: Point, p1: Point, b: f64) {
    if !b.is_finite() || b.abs() < 1e-9 || (p1 - p0).hypot() < 1e-12 {
        path.line_to(p1);
        return;
    }
    let chord = p1 - p0;
    let d = chord.hypot();
    let sweep = 4.0 * b.atan();
    let r = d / (2.0 * (sweep / 2.0).sin()).abs();
    let mid = p0.midpoint(p1);
    let left = Vec2::new(-chord.y, chord.x) / d;
    // Signed distance from the chord midpoint to the centre.
    let h = d * (1.0 - b * b) / (4.0 * b);
    let center = mid + left * h;
    let start = (p0 - center).atan2();
    append_arc(path, center, r, r, 0.0, start, sweep);
    // Snap the end to the exact vertex to avoid drift.
    if let Some(PathEl::CurveTo(a, c, _)) = path.elements().last().copied() {
        path.pop();
        path.curve_to(a, c, p1);
    }
}

/// Appends an elliptical arc (without a move-to) to `path`.
pub fn append_arc(path: &mut BezPath, center: Point, rx: f64, ry: f64, x_rotation: f64, start: f64, sweep: f64) {
    if !(rx.is_finite() && ry.is_finite() && start.is_finite() && sweep.is_finite()) || rx <= 0.0 || ry <= 0.0 {
        return;
    }
    let arc = Arc::new(center, (rx, ry), start, sweep, x_rotation);
    let tol = (ARC_TOLERANCE * rx.max(ry)).max(1e-7);
    for el in arc.append_iter(tol) {
        path.push(el);
    }
}

/// A standalone arc / ellipse path starting with a move-to.
pub fn arc_path(center: Point, rx: f64, ry: f64, x_rotation: f64, start: f64, sweep: f64) -> BezPath {
    let mut p = BezPath::new();
    if !(rx.is_finite() && ry.is_finite() && start.is_finite() && sweep.is_finite()) || rx <= 0.0 || ry <= 0.0 {
        return p;
    }
    let (s, c) = start.sin_cos();
    let local = Vec2::new(rx * c, ry * s);
    let (rs, rc) = x_rotation.sin_cos();
    let first = center + Vec2::new(local.x * rc - local.y * rs, local.x * rs + local.y * rc);
    p.move_to(first);
    append_arc(&mut p, center, rx, ry, x_rotation, start, sweep);
    p
}

/// A full circle as a closed path, starting at angle 0 and running counter-clockwise.
pub fn circle_path(center: Point, r: f64) -> BezPath {
    let mut p = arc_path(center, r, r, 0.0, 0.0, std::f64::consts::TAU);
    if !p.is_empty() {
        p.close_path();
    }
    p
}

/// Splits a path into its subpaths, each starting with a `MoveTo`.
pub fn split_subpaths(path: &BezPath) -> Vec<BezPath> {
    let mut out = Vec::new();
    let mut cur = BezPath::new();
    for el in path.elements() {
        if let PathEl::MoveTo(_) = el
            && !cur.is_empty()
        {
            out.push(std::mem::take(&mut cur));
        }
        if cur.is_empty() && !matches!(el, PathEl::MoveTo(_)) {
            // Path without a leading move-to: start from the origin like SVG does.
            cur.move_to(Point::ZERO);
        }
        cur.push(*el);
    }
    if cur.elements().len() > 1 {
        out.push(cur);
    }
    out.retain(|p| p.elements().len() > 1);
    out
}

/// Whether a subpath ends with `ClosePath`.
pub fn is_closed(sub: &BezPath) -> bool {
    matches!(sub.elements().last(), Some(PathEl::ClosePath))
}

/// First and last point of an open subpath.
pub fn endpoints(sub: &BezPath) -> Option<(Point, Point)> {
    let start = match sub.elements().first()? {
        PathEl::MoveTo(p) => *p,
        _ => return None,
    };
    let end = sub.elements().last()?.end_point()?;
    Some((start, end))
}

/// Joins open subpaths of path items whose ends meet within `tolerance`, when they share
/// layer, colours and group. Closes chains whose ends meet. Returns how many joins were made.
pub fn join_open_paths(items: &mut Vec<Item>, tolerance: f64) -> usize {
    if !(tolerance > 0.0) {
        return 0;
    }
    type Key = (Option<String>, Option<u32>, Option<crate::Rgb>, Option<crate::Rgb>, Option<u32>, bool);
    let mut buckets: HashMap<Key, Vec<BezPath>> = HashMap::new();
    let mut order: Vec<Key> = Vec::new();
    let mut kept = Vec::with_capacity(items.len());
    for item in items.drain(..) {
        let ItemKind::Path(p) = &item.kind else {
            kept.push(item);
            continue;
        };
        if p.dash.is_some() || p.fill.is_some() {
            kept.push(item);
            continue;
        }
        let subs = split_subpaths(&p.path);
        let (open, closed): (Vec<_>, Vec<_>) = subs.into_iter().partition(|s| !is_closed(s));
        if open.is_empty() {
            kept.push(item);
            continue;
        }
        let key: Key = (item.layer.clone(), item.layer_index, p.stroke, p.fill, item.group, item.hidden);
        if !closed.is_empty() {
            let mut rest = item.clone();
            if let ItemKind::Path(rp) = &mut rest.kind {
                rp.path = concat(closed);
            }
            kept.push(rest);
        }
        if !buckets.contains_key(&key) {
            order.push(key.clone());
        }
        buckets.entry(key).or_default().extend(open);
    }
    let mut joins = 0;
    for key in order {
        let Some(subs) = buckets.remove(&key) else { continue };
        let (chains, j) = chain(subs, tolerance);
        joins += j;
        for c in chains {
            let mut item = Item::path(c, key.2, key.3);
            item.layer = key.0.clone();
            item.layer_index = key.1;
            item.group = key.4;
            item.hidden = key.5;
            kept.push(item);
        }
    }
    *items = kept;
    joins
}

fn concat(paths: Vec<BezPath>) -> BezPath {
    let mut out = BezPath::new();
    for p in paths {
        for el in p.elements() {
            out.push(*el);
        }
    }
    out
}

fn cell(p: Point, size: f64) -> (i64, i64) {
    ((p.x / size).floor() as i64, (p.y / size).floor() as i64)
}

/// Chains open subpaths end to end. Returns the chains and the number of joins.
fn chain(subs: Vec<BezPath>, tol: f64) -> (Vec<BezPath>, usize) {
    let n = subs.len();
    let ends: Vec<(Point, Point)> = subs.iter().map(|s| endpoints(s).unwrap_or((Point::ZERO, Point::ZERO))).collect();
    let size = tol.max(1e-9);
    let mut grid: HashMap<(i64, i64), Vec<(usize, bool)>> = HashMap::new();
    for (i, (s, e)) in ends.iter().enumerate() {
        grid.entry(cell(*s, size)).or_default().push((i, false));
        grid.entry(cell(*e, size)).or_default().push((i, true));
    }
    let mut used = vec![false; n];
    let find = |p: Point, used: &[bool]| -> Option<(usize, bool)> {
        let (cx, cy) = cell(p, size);
        let mut best: Option<(f64, usize, bool)> = None;
        for dx in -1..=1 {
            for dy in -1..=1 {
                if let Some(list) = grid.get(&(cx + dx, cy + dy)) {
                    for &(i, is_end) in list {
                        if used[i] {
                            continue;
                        }
                        let q = if is_end { ends[i].1 } else { ends[i].0 };
                        let d = (q - p).hypot();
                        if d <= tol && best.is_none_or(|b| d < b.0) {
                            best = Some((d, i, is_end));
                        }
                    }
                }
            }
        }
        best.map(|b| (b.1, b.2))
    };
    let mut out = Vec::new();
    let mut joins = 0;
    for start in 0..n {
        if used[start] {
            continue;
        }
        used[start] = true;
        let mut segs: Vec<BezPath> = vec![subs[start].clone()];
        let (mut head, mut tail) = ends[start];
        // Extend forward.
        while let Some((i, at_end)) = find(tail, &used) {
            used[i] = true;
            joins += 1;
            let s = if at_end { subs[i].reverse_subpaths() } else { subs[i].clone() };
            tail = if at_end { ends[i].0 } else { ends[i].1 };
            segs.push(s);
        }
        // Extend backward.
        while let Some((i, at_end)) = find(head, &used) {
            used[i] = true;
            joins += 1;
            let s = if at_end { subs[i].clone() } else { subs[i].reverse_subpaths() };
            head = if at_end { ends[i].0 } else { ends[i].1 };
            segs.insert(0, s);
        }
        let mut path = BezPath::new();
        for (k, s) in segs.iter().enumerate() {
            for (j, el) in s.elements().iter().enumerate() {
                if j == 0 && k > 0 {
                    continue;
                }
                path.push(*el);
            }
        }
        if (head - tail).hypot() <= tol && path.elements().len() > 2 {
            path.close_path();
        }
        out.push(path);
    }
    (out, joins)
}

/// Recognises a closed subpath that is a circle within `tol` mm. Returns centre and radius.
pub fn as_circle(sub: &BezPath, tol: f64) -> Option<(Point, f64)> {
    if !is_closed(sub) {
        return None;
    }
    let curves = sub.segments().filter(|s| !matches!(s, PathSeg::Line(_))).count();
    if curves < 2 {
        return None;
    }
    let mut pts = Vec::new();
    for seg in sub.segments() {
        for k in 0..8 {
            pts.push(seg.eval(f64::from(k) / 8.0));
        }
    }
    if pts.len() < 8 {
        return None;
    }
    let (mut minx, mut miny, mut maxx, mut maxy) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for p in &pts {
        minx = minx.min(p.x);
        miny = miny.min(p.y);
        maxx = maxx.max(p.x);
        maxy = maxy.max(p.y);
    }
    let c = Point::new((minx + maxx) / 2.0, (miny + maxy) / 2.0);
    let r = ((maxx - minx) + (maxy - miny)) / 4.0;
    if r <= tol {
        return None;
    }
    pts.iter().all(|p| ((*p - c).hypot() - r).abs() <= tol).then_some((c, r))
}

/// A polyline vertex with the DXF bulge of the segment that leaves it.
pub type BulgeVertex = (Point, f64);

/// Turns a subpath into polyline vertices with bulges. With `arcs`, curves that are circular
/// arcs within `tol` become bulge segments; everything else is flattened to lines.
/// Returns the vertices and whether the subpath is closed.
pub fn to_bulge_polyline(sub: &BezPath, tol: f64, arcs: bool) -> (Vec<BulgeVertex>, bool) {
    let closed = is_closed(sub);
    let mut verts: Vec<BulgeVertex> = Vec::new();
    let Some((start, _)) = endpoints(sub) else { return (verts, closed) };
    verts.push((start, 0.0));
    for seg in sub.segments() {
        match seg {
            PathSeg::Line(l) => verts.push((l.p1, 0.0)),
            _ => {
                if arcs {
                    fit_arcs(seg, tol, 0, &mut verts);
                } else {
                    flatten_seg(seg, tol, &mut verts);
                }
            }
        }
    }
    if closed && verts.len() > 1 {
        let first = verts[0].0;
        if (verts[verts.len() - 1].0 - first).hypot() < 1e-9 {
            // The last vertex equals the first: drop it and move its incoming bulge.
            verts.pop();
        }
    }
    (verts, closed)
}

fn flatten_seg(seg: PathSeg, tol: f64, verts: &mut Vec<BulgeVertex>) {
    let mut path = BezPath::new();
    path.move_to(seg.start());
    path.push(seg.as_path_el());
    kurbo::flatten(path.elements().iter().copied(), tol.max(1e-6), |el| {
        if let PathEl::LineTo(p) = el {
            verts.push((p, 0.0));
        }
    });
}

/// Fits circular arcs to a curve segment. The bulge of an arc is stored on the vertex where it starts.
fn fit_arcs(seg: PathSeg, tol: f64, depth: u32, verts: &mut Vec<BulgeVertex>) {
    let p0 = seg.start();
    let p1 = seg.end();
    let pm = seg.eval(0.5);
    if let Some((center, r, sweep)) = circle_through(p0, pm, p1) {
        let ok = [0.125, 0.25, 0.375, 0.625, 0.75, 0.875].iter().all(|&t| ((seg.eval(t) - center).hypot() - r).abs() <= tol);
        if ok && sweep.abs() < std::f64::consts::PI * 1.999 {
            if let Some(last) = verts.last_mut() {
                last.1 = (sweep / 4.0).tan();
            }
            verts.push((p1, 0.0));
            return;
        }
    } else {
        // Collinear: a straight line if the curve stays on the chord.
        let line = kurbo::Line::new(p0, p1);
        let ok = [0.25, 0.5, 0.75].iter().all(|&t| line.nearest(seg.eval(t), 1e-9).distance_sq.sqrt() <= tol);
        if ok {
            verts.push((p1, 0.0));
            return;
        }
    }
    if depth >= 10 {
        flatten_seg(seg, tol, verts);
        return;
    }
    let (a, b) = seg.subdivide();
    fit_arcs(a, tol, depth + 1, verts);
    fit_arcs(b, tol, depth + 1, verts);
}

use kurbo::ParamCurveNearest;

/// Circle through three points; returns centre, radius and the signed sweep from `a` to `c` via `b`.
pub fn circle_through(a: Point, b: Point, c: Point) -> Option<(Point, f64, f64)> {
    let cross = (b - a).cross(c - a);
    let scale = (b - a).hypot() * (c - a).hypot();
    if scale <= 0.0 || (cross / scale).abs() < 1e-9 {
        return None;
    }
    let d = 2.0 * (a.x * (b.y - c.y) + b.x * (c.y - a.y) + c.x * (a.y - b.y));
    if d.abs() < 1e-18 {
        return None;
    }
    let a2 = a.to_vec2().hypot2();
    let b2 = b.to_vec2().hypot2();
    let c2 = c.to_vec2().hypot2();
    let ux = (a2 * (b.y - c.y) + b2 * (c.y - a.y) + c2 * (a.y - b.y)) / d;
    let uy = (a2 * (c.x - b.x) + b2 * (a.x - c.x) + c2 * (b.x - a.x)) / d;
    let center = Point::new(ux, uy);
    let r = (a - center).hypot();
    let a0 = (a - center).atan2();
    let a1 = (c - center).atan2();
    let tau = std::f64::consts::TAU;
    let sweep = if cross > 0.0 { (a1 - a0).rem_euclid(tau) } else { -((a0 - a1).rem_euclid(tau)) };
    Some((center, r, sweep))
}

/// Flattens a path into polylines (points, closed flag).
pub fn flatten_polylines(path: &BezPath, tol: f64) -> Vec<(Vec<Point>, bool)> {
    let mut out: Vec<(Vec<Point>, bool)> = Vec::new();
    kurbo::flatten(path.elements().iter().copied(), tol.max(1e-6), |el| match el {
        PathEl::MoveTo(p) => out.push((vec![p], false)),
        PathEl::LineTo(p) => {
            if let Some(last) = out.last_mut() {
                last.0.push(p);
            } else {
                out.push((vec![Point::ZERO, p], false));
            }
        }
        PathEl::ClosePath => {
            if let Some(last) = out.last_mut() {
                last.1 = true;
            }
        }
        _ => {}
    });
    out.retain(|(p, _)| p.len() > 1);
    out
}

/// Formats a number with up to `decimals` decimals and no trailing zeros.
pub fn fmt_num(v: f64, decimals: usize) -> String {
    let v = if v.is_finite() { v } else { 0.0 };
    let mut s = format!("{v:.decimals$}");
    if s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
    }
    if s == "-0" { "0".into() } else { s }
}
