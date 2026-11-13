//! The canvas tools of the shape-editing commands: edit nodes, add tabs, measure, warp, deform,
//! start points, round corners, mirror line and two-point rotate and scale. Pointer handling
//! and overlays live here; every change goes through a registered command in `shape_ops.rs`,
//! `shape_nodes.rs` or `shape_arrays.rs` (or `shape_ops::set_shape_path` inside one undo step),
//! so it works without a window too. The geometry is `darkburn_geom`'s.
//!
//! Keys in the node tool (no text field focused): I insert, D delete, Shift+D delete segment,
//! C corner, S smooth, L line, V curve, B break, T trim, E extend, arrows nudge (Shift 10 mm,
//! Ctrl 0.1 mm), Alt+arrows / Alt+H / Alt+V align the selected nodes, Esc leaves.
//!
//! The state between frames lives in egui's temporary memory, like the select tool's.

use darkburn_core::kurbo::{Affine, BezPath, ParamCurve, PathSeg, Point, Rect, Vec2};
use darkburn_core::snap::{self, SnapOptions};
use darkburn_core::{ShapeId, select};
use darkburn_geom::cleanup;
use darkburn_geom::node::{self as gnode, NodeError, NodeRef};
use darkburn_geom::path::{self as gpath, Contour, contours, from_contours};
use darkburn_geom::query;
use darkburn_geom::transform as gxf;
use egui::{Align2, Color32, FontId, Key, Modifiers, Painter, PointerButton, Pos2, Response, Stroke, Ui, vec2};
use serde_json::{Value, json};

use crate::render::{self, Xf};
use crate::shell::DarkBurn;
use crate::tools::{Mods, Tool};

/// Hit distance for nodes, handles and outlines (screen points).
const HIT_PX: f32 = 8.0;
const NODE_PX: f32 = 4.0;
const ORANGE: Color32 = Color32::from_rgb(255, 170, 40);
const GREEN: Color32 = Color32::from_rgb(60, 190, 90);
const RED: Color32 = Color32::from_rgb(230, 70, 60);

/// The status bar hint for `tool`.
pub fn hint(tool: Tool) -> &'static str {
    match tool {
        Tool::Nodes => {
            "Edit nodes: drag nodes and handles, drag a line to curve it, Ctrl+click a line to add a node, box-select nodes. Keys: I insert, D delete, Shift+D delete segment, C corner, S smooth, L line, V curve, B break, T trim, E extend, arrows nudge, Alt+arrows align, Shift constrains."
        }
        Tool::Tabs => "Click an outline to add a tab, click a tab to remove it. Orange marks are the tabs the layer adds by itself.",
        Tool::Measure => "Click two points to measure, a third for an arc. Shift locks an axis, U switches mm and inch, Esc clears.",
        Tool::Warp => "Drag the four corners to warp the selection. B switches perspective and bilinear, Enter applies, Esc cancels.",
        Tool::Deform => "Drag the 16 points to deform the selection. Enter applies, Esc cancels.",
        Tool::StartPoints => "Click an outline to start it there (Shift: every sub-path), click the arrow or press R to reverse the direction.",
        Tool::RoundCorners => {
            "Click a corner to round it with the Modifiers radius (Shift: chamfer). Ctrl+click empty space rounds every corner of the selection."
        }
        Tool::Mirror => "Click two points on the mirror line. Ctrl keeps the original.",
        Tool::TwoPoint => "Click the two source points, then the two target points. Shift stretches along the line only.",
        _ => "",
    }
}

// ================================================================ pure geometry

/// Every node of `cs` with its address.
pub fn node_points(cs: &[Contour]) -> Vec<(NodeRef, Point)> {
    let mut out = Vec::new();
    for (si, c) in cs.iter().enumerate() {
        for i in 0..c.node_count() {
            if let Some(p) = c.node(i) {
                out.push((NodeRef::new(si, i), p));
            }
        }
    }
    out
}

/// The node of `cs` nearest `p` within `tol`.
pub fn hit_node(cs: &[Contour], p: Point, tol: f64) -> Option<NodeRef> {
    node_points(cs).into_iter().map(|(n, q)| (n, q.distance(p))).filter(|(_, d)| *d <= tol).min_by(|a, b| a.1.total_cmp(&b.1)).map(|(n, _)| n)
}

/// A segment under the pointer: subpath, segment, parameter.
pub fn hit_segment(path: &BezPath, p: Point, tol: f64) -> Option<(usize, usize, f64)> {
    let h = query::nearest_point(path, p)?;
    (h.distance <= tol).then_some((h.subpath, h.at.seg, h.at.t))
}

fn incoming(c: &Contour, i: usize) -> Option<usize> {
    let n = c.segs.len();
    if n == 0 {
        None
    } else if c.closed {
        Some((i + n - 1) % n)
    } else if i > 0 && i <= n {
        Some(i - 1)
    } else {
        None
    }
}

fn outgoing(c: &Contour, i: usize) -> Option<usize> {
    (i < c.segs.len()).then_some(i)
}

/// Which handle of a node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandleKind {
    In,
    Out,
}

/// The control point of node `i`'s handle, if the segment there is a curve.
pub fn handle_point(c: &Contour, i: usize, kind: HandleKind) -> Option<Point> {
    match kind {
        HandleKind::Out => match c.segs.get(outgoing(c, i)?)? {
            PathSeg::Cubic(b) => Some(b.p1),
            _ => None,
        },
        HandleKind::In => match c.segs.get(incoming(c, i)?)? {
            PathSeg::Cubic(b) => Some(b.p2),
            _ => None,
        },
    }
}

/// Handles worth drawing for node `i` (those of zero length are left out).
pub fn handles_of(c: &Contour, i: usize) -> Vec<(HandleKind, Point)> {
    let Some(n) = c.node(i) else { return Vec::new() };
    [HandleKind::In, HandleKind::Out].into_iter().filter_map(|k| handle_point(c, i, k).filter(|h| h.distance(n) > 1e-6).map(|h| (k, h))).collect()
}

/// Whether node `i` is smooth: both handles exist and point in opposite directions.
pub fn is_smooth(c: &Contour, i: usize) -> bool {
    let (Some(n), Some(a), Some(b)) = (c.node(i), handle_point(c, i, HandleKind::In), handle_point(c, i, HandleKind::Out)) else { return false };
    let (va, vb) = (a - n, b - n);
    let (la, lb) = (va.hypot(), vb.hypot());
    la > 1e-9 && lb > 1e-9 && (va.cross(vb) / (la * lb)).abs() < 0.02 && va.dot(vb) < 0.0
}

fn set_control(c: &mut Contour, i: usize, kind: HandleKind, to: Point) {
    let idx = match kind {
        HandleKind::Out => outgoing(c, i),
        HandleKind::In => incoming(c, i),
    };
    if let Some(idx) = idx
        && let PathSeg::Cubic(b) = &mut c.segs[idx]
    {
        match kind {
            HandleKind::Out => b.p1 = to,
            HandleKind::In => b.p2 = to,
        }
    }
}

/// Move a node's handle to `to`; at a smooth node the other handle turns to stay opposite.
pub fn set_handle(c: &mut Contour, i: usize, kind: HandleKind, to: Point) {
    let smooth = is_smooth(c, i);
    let other_kind = if kind == HandleKind::In { HandleKind::Out } else { HandleKind::In };
    let other = handle_point(c, i, other_kind);
    set_control(c, i, kind, to);
    if smooth && let (Some(n), Some(o)) = (c.node(i), other) {
        let d = to - n;
        if d.hypot() > 1e-9 {
            let len = (o - n).hypot();
            set_control(c, i, other_kind, n - d.normalize() * len);
        }
    }
}

/// `path` with node `node`'s handle moved to `to`.
pub fn move_handle(path: &BezPath, node: NodeRef, kind: HandleKind, to: Point) -> BezPath {
    let mut cs = contours(path);
    if let Some(c) = cs.get_mut(node.subpath) {
        set_handle(c, node.node, kind, to);
    }
    from_contours(cs.iter())
}

/// `path` with `nodes` moved by `delta` (their handles go along).
pub fn move_nodes(path: &BezPath, nodes: &[NodeRef], delta: Vec2) -> BezPath {
    let mut p = path.clone();
    for n in nodes {
        if let Ok(q) = gnode::move_node(&p, *n, delta) {
            p = q;
        }
    }
    p
}

/// Drag segment `seg` of `sub` so the curve passes through `to` at parameter `t`: a line turns
/// into a curve first. `None` when the segment does not exist.
pub fn drag_curve(path: &BezPath, sub: usize, seg: usize, t: f64, to: Point) -> Option<BezPath> {
    let mut cs = contours(path);
    let c = cs.get_mut(sub)?;
    let s = *c.segs.get(seg)?;
    let mut b = s.to_cubic();
    let t = t.clamp(0.1, 0.9);
    let d = to - b.eval(t);
    let a = 1.0 / (3.0 * t * (1.0 - t));
    b.p1 += d * a;
    b.p2 += d * a;
    c.segs[seg] = PathSeg::Cubic(b);
    Some(from_contours(cs.iter()))
}

/// The pointer with Shift held: moved onto the nearest horizontal, vertical or 45 degree ray
/// from `origin`.
pub fn constrain(origin: Point, p: Point) -> Point {
    snap::snap_angle(origin, p, 45.0)
}

/// What a node key does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NodeOp {
    Insert,
    Delete,
    DeleteSegment,
    Corner,
    Smooth,
    ToLine,
    ToCurve,
    Break,
    Trim,
    Extend,
    Align(gnode::Align),
}

/// The node operation a key (with modifiers) stands for.
pub fn key_op(key: Key, m: Modifiers) -> Option<NodeOp> {
    if m.alt {
        return Some(NodeOp::Align(match key {
            Key::ArrowLeft => gnode::Align::Left,
            Key::ArrowRight => gnode::Align::Right,
            Key::ArrowUp => gnode::Align::Top,
            Key::ArrowDown => gnode::Align::Bottom,
            Key::H => gnode::Align::CenterH,
            Key::V => gnode::Align::CenterV,
            _ => return None,
        }));
    }
    if m.command {
        return (key == Key::D).then_some(NodeOp::DeleteSegment);
    }
    Some(match key {
        Key::I => NodeOp::Insert,
        Key::D if m.shift => NodeOp::DeleteSegment,
        Key::D => NodeOp::Delete,
        Key::C => NodeOp::Corner,
        Key::S => NodeOp::Smooth,
        Key::L => NodeOp::ToLine,
        Key::V => NodeOp::ToCurve,
        Key::B => NodeOp::Break,
        Key::T => NodeOp::Trim,
        Key::E => NodeOp::Extend,
        _ => return None,
    })
}

fn err_text(e: NodeError) -> String {
    e.to_string()
}

/// Apply `op` to `path`: `nodes` are the selected nodes, `seg` the selected segment, `at` the
/// pointer (insert and trim go there), `others` the other shapes' outlines.
pub fn apply_op(
    path: &BezPath,
    op: NodeOp,
    nodes: &[NodeRef],
    seg: Option<(usize, usize)>,
    at: Point,
    others: &[BezPath],
) -> Result<BezPath, String> {
    let mut desc: Vec<NodeRef> = nodes.to_vec();
    desc.sort_by_key(|n| std::cmp::Reverse((n.subpath, n.node)));
    desc.dedup();
    // A selected node stands for the line or curve that leaves it; the end node of an open
    // subpath has none.
    let segs: Vec<(usize, usize)> = match seg {
        Some(s) => vec![s],
        None => {
            let cs = contours(path);
            desc.iter().filter(|n| cs.get(n.subpath).is_none_or(|c| c.closed || n.node < c.segs.len())).map(|n| (n.subpath, n.node)).collect()
        }
    };
    let mut p = path.clone();
    let each = |p: &mut BezPath, f: &dyn Fn(&BezPath, usize, usize) -> Result<BezPath, NodeError>, items: &[(usize, usize)]| -> Result<(), String> {
        for (s, i) in items {
            *p = f(p, *s, *i).map_err(err_text)?;
        }
        Ok(())
    };
    match op {
        NodeOp::Insert => {
            let (s, g, t) = match seg {
                Some((s, g)) => (s, g, 0.5),
                None => {
                    let h = query::nearest_point(path, at).ok_or("nothing to insert into")?;
                    (h.subpath, h.at.seg, h.at.t)
                }
            };
            p = gnode::insert_node(&p, s, g, t).map_err(err_text)?;
        }
        NodeOp::Delete => {
            if desc.is_empty() {
                return Err("select nodes first".into());
            }
            // A subpath that runs out of nodes is gone as a whole: its other selected nodes went
            // with it, and its index now belongs to the subpath after it.
            let mut gone = None;
            for n in &desc {
                if gone == Some(n.subpath) {
                    continue;
                }
                let before = contours(&p).len();
                p = gnode::delete_node(&p, *n).map_err(err_text)?;
                if contours(&p).len() < before {
                    gone = Some(n.subpath);
                }
            }
        }
        NodeOp::DeleteSegment => {
            if segs.is_empty() {
                return Err("select a segment or node first".into());
            }
            let mut items = segs.clone();
            items.sort_by_key(|x| std::cmp::Reverse(*x));
            // Taking a line out of a closed subpath opens it, and the open subpath starts after
            // that line: the lines before it move up by `shift`.
            let mut opened: Option<(usize, usize)> = None;
            for (s, i) in items {
                let cs = contours(&p);
                let i = match opened {
                    Some((sub, shift)) if sub == s => i + shift,
                    _ => i,
                };
                if let Some(c) = cs.get(s).filter(|c| c.closed && i < c.segs.len()) {
                    opened = Some((s, c.segs.len() - 1 - i));
                }
                p = gnode::delete_segment(&p, s, i).map_err(err_text)?;
            }
        }
        NodeOp::ToLine | NodeOp::ToCurve => {
            if segs.is_empty() {
                return Err("select a segment or node first".into());
            }
            if op == NodeOp::ToLine {
                each(&mut p, &|p, s, i| gnode::segment_to_line(p, s, i), &segs)?;
            } else {
                each(&mut p, &|p, s, i| gnode::segment_to_curve(p, s, i), &segs)?;
            }
        }
        NodeOp::Corner | NodeOp::Smooth | NodeOp::Break => {
            if desc.is_empty() {
                return Err("select nodes first".into());
            }
            // Breaking a closed subpath opens it with the break as its first node: the nodes
            // before the break move up by `shift`.
            let mut opened: Option<(usize, usize)> = None;
            for n in &desc {
                let mut n = *n;
                if let Some((sub, shift)) = opened
                    && sub == n.subpath
                {
                    n.node += shift;
                }
                if op == NodeOp::Break
                    && let Some(c) = contours(&p).get(n.subpath).filter(|c| c.closed && n.node < c.segs.len())
                {
                    opened = Some((n.subpath, c.segs.len() - n.node));
                }
                p = match op {
                    NodeOp::Corner => gnode::set_node_corner(&p, n),
                    NodeOp::Smooth => gnode::set_node_smooth(&p, n),
                    _ => gnode::break_at_node(&p, n),
                }
                .map_err(err_text)?;
            }
        }
        NodeOp::Trim => p = gnode::trim_at_intersection(&p, others, at).map_err(err_text)?,
        NodeOp::Extend => {
            let n = *desc.last().ok_or("select an end node first")?;
            p = gnode::extend_to_intersection(&p, others, n, 1.0e6).map_err(err_text)?;
        }
        NodeOp::Align(how) => {
            if nodes.len() < 2 {
                return Err("select at least two nodes".into());
            }
            p = gnode::align_nodes(&p, nodes, how).map_err(err_text)?;
        }
    }
    Ok(p)
}

/// Join open ends that sit on each other (an end node dropped on another end).
pub fn join_touching_ends(path: &BezPath) -> BezPath {
    let subs = gpath::subpaths(path);
    if subs.len() < 2 {
        return path.clone();
    }
    match cleanup::auto_join(&subs, 1e-4, &mut |_, _| true) {
        Some(joined) if joined.len() < subs.len() => {
            let mut out = BezPath::new();
            for j in joined {
                out.extend(j);
            }
            out
        }
        _ => path.clone(),
    }
}

/// Corner nodes of `path` (a turn of more than one degree between the segments either side).
pub fn corner_nodes(path: &BezPath) -> Vec<(NodeRef, Point)> {
    let cs = contours(path);
    let mut out = Vec::new();
    for (si, c) in cs.iter().enumerate() {
        for i in 0..c.node_count() {
            let (Some(a), Some(b)) = (incoming(c, i), outgoing(c, i)) else { continue };
            let (ta, tb) = (gpath::seg_tangent(&c.segs[a], 1.0), gpath::seg_tangent(&c.segs[b], 0.0));
            if ta.dot(tb) < 0.9998
                && let Some(p) = c.node(i)
            {
                out.push((NodeRef::new(si, i), p));
            }
        }
    }
    out
}

/// A tab marker: the two ends of a bar of length `size` along the outline at `p`.
pub fn tab_marker(path: &BezPath, p: Point, size: f64) -> Option<(Point, Point)> {
    let h = query::nearest_point(path, p)?;
    let cs = contours(path);
    let seg = cs.get(h.subpath)?.segs.get(h.at.seg)?;
    let t = gpath::seg_tangent(seg, h.at.t);
    Some((h.point - t * (size / 2.0), h.point + t * (size / 2.0)))
}

/// Start and end markers of every sub-path: (start, end of an open one, unit direction at the
/// start).
pub fn start_markers(path: &BezPath) -> Vec<(Point, Option<Point>, Vec2)> {
    contours(path)
        .iter()
        .filter_map(|c| {
            let s = c.start()?;
            let dir = gpath::seg_tangent(c.segs.first()?, 0.0);
            Some((s, (!c.closed).then(|| c.end()).flatten(), dir))
        })
        .collect()
}

/// A length for the readout.
pub fn fmt_len(mm: f64, inches: bool) -> String {
    if inches { format!("{:.4} in", mm / 25.4) } else { format!("{mm:.3} mm") }
}

fn fmt_area(mm2: f64, inches: bool) -> String {
    if inches { format!("{:.4} in²", mm2 / 645.16) } else { format!("{mm2:.2} mm²") }
}

/// The readout lines for the measured points (and the pointer as the next point).
pub fn measure_lines(pts: &[Point], pointer: Option<Point>, inches: bool) -> Vec<String> {
    let mut all: Vec<Point> = pts.to_vec();
    if let (Some(p), true) = (pointer, pts.len() < 3) {
        all.push(p);
    }
    let mut out = Vec::new();
    if all.len() >= 2 {
        let d = all[1] - all[0];
        out.push(format!("Length {}", fmt_len(d.hypot(), inches)));
        out.push(format!("dx {}  dy {}", fmt_len(d.x, inches), fmt_len(d.y, inches)));
        out.push(format!("Angle {:.2}°", d.y.atan2(d.x).to_degrees()));
    }
    if all.len() >= 3
        && let Some(m) = query::arc_through(all[0], all[1], all[2])
    {
        out.push(format!("Radius {}", fmt_len(m.radius, inches)));
        out.push(format!("Arc {:.2}°  length {}", m.sweep.to_degrees().abs(), fmt_len(m.length, inches)));
    }
    out
}

/// The warped (or deformed) outlines for the preview.
pub fn warp_preview(paths: &[BezPath], src: Rect, pts: &[Point], deform: bool, perspective: bool) -> Vec<BezPath> {
    paths
        .iter()
        .filter_map(|p| {
            if deform {
                if pts.len() != 16 {
                    return None;
                }
                let mut g = [[Point::ZERO; 4]; 4];
                for (i, q) in pts.iter().enumerate() {
                    g[i / 4][i % 4] = *q;
                }
                gxf::deform_grid(p, src, g, 0.05)
            } else {
                if pts.len() != 4 {
                    return None;
                }
                let dst = [pts[0], pts[1], pts[2], pts[3]];
                if perspective {
                    gxf::warp_perspective(p, src, dst, 0.05).or_else(|| gxf::warp_bilinear(p, src, dst, 0.05))
                } else {
                    gxf::warp_bilinear(p, src, dst, 0.05)
                }
            }
        })
        .collect()
}

/// The initial control points over `src`: the four corners, or the 4 x 4 grid.
pub fn warp_points(src: Rect, deform: bool) -> Vec<Point> {
    if deform { gxf::identity_grid(src).iter().flatten().copied().collect() } else { gxf::rect_corners(src).to_vec() }
}

// ================================================================ state

#[derive(Clone, Debug, Default)]
enum Gesture {
    #[default]
    None,
    /// Dragging selected nodes: the shapes' paths as they were, the nodes in each, the start.
    Nodes {
        origs: Vec<(ShapeId, BezPath, Vec<NodeRef>)>,
        origin: Point,
        moved: bool,
    },
    Handle {
        shape: ShapeId,
        orig: BezPath,
        node: NodeRef,
        kind: HandleKind,
        moved: bool,
    },
    Curve {
        shape: ShapeId,
        orig: BezPath,
        sub: usize,
        seg: usize,
        t: f64,
        moved: bool,
    },
    Box {
        start: Point,
        now: Point,
    },
    Warp(usize),
}

#[derive(Clone, Debug)]
struct WarpState {
    ids: Vec<ShapeId>,
    src: Rect,
    pts: Vec<Point>,
    perspective: bool,
    preview: Vec<BezPath>,
}

/// What stays between measurements, also when another tool is used meanwhile.
#[derive(Clone, Debug, Default)]
struct MeasureMem {
    pts: Vec<Point>,
    inches: bool,
}

#[derive(Clone, Debug, Default)]
struct State {
    tool: Tool,
    gesture: Gesture,
    /// The pointer in document mm (snapped where the tool snaps).
    hover: Option<Point>,
    snapped: Option<snap::Snap>,
    nodes: Vec<(ShapeId, NodeRef)>,
    seg: Option<(ShapeId, usize, usize)>,
    warp: Option<WarpState>,
    pick: Vec<Point>,
    canvas: Option<egui::Rect>,
    sel_info: (u64, Vec<String>),
    status: Option<String>,
}

fn state_id() -> egui::Id {
    egui::Id::unique("darkburn_shape_tools")
}

fn measure_id() -> egui::Id {
    egui::Id::unique("darkburn_measure_memory")
}

/// Escape for the tool: an open dialog or window gets the key first (it closes), as on the canvas.
fn take_escape(ctx: &egui::Context) -> Option<Modifiers> {
    if crate::escape::window_open(ctx) { None } else { take_key(ctx, Key::Escape) }
}

fn press_origin(ui: &Ui, xf: &Xf) -> Option<Point> {
    ui.input(|i| i.pointer.press_origin()).map(|p| xf.to_doc(p))
}

/// Take the first press of `key` out of this frame's input (so no other handler sees it).
fn take_key(ctx: &egui::Context, key: Key) -> Option<Modifiers> {
    ctx.input_mut(|i| {
        let pos = i.events.iter().position(|e| matches!(e, egui::Event::Key { key: k, pressed: true, .. } if *k == key))?;
        match i.events.remove(pos) {
            egui::Event::Key { modifiers, .. } => Some(modifiers),
            _ => None,
        }
    })
}

fn snap_pt(app: &DarkBurn, xf: &Xf, p: Point, m: Mods, exclude: &[ShapeId]) -> (Point, Option<snap::Snap>) {
    if m.ctrl {
        return (p, None);
    }
    let (major, minor) = crate::view::grid_steps(xf.zoom as f32);
    let radius = xf.mm(if m.alt { 24.0 } else { 8.0 });
    let opts = SnapOptions { grid: Some(minor.unwrap_or(major) as f64), objects: true, radius };
    match snap::snap(&app.editor.doc, p, &opts, exclude) {
        Some(s) => (s.point, Some(s)),
        None => (p, None),
    }
}

/// The selected shapes node tools can work on, with their document outlines.
fn plain_selected(app: &DarkBurn) -> Vec<(ShapeId, BezPath)> {
    app.editor.selected().into_iter().filter_map(|id| crate::shape_ops::shape_path(&app.editor.doc, id).map(|p| (id, p))).collect()
}

/// Selected shapes' outlines with their forced start point and direction applied.
fn marker_selected(app: &DarkBurn) -> Vec<(ShapeId, BezPath)> {
    app.editor.selected().into_iter().filter_map(|id| crate::shape_nodes::marker_path(&app.editor.doc, id).map(|p| (id, p))).collect()
}

fn others_of(app: &DarkBurn, id: ShapeId) -> Vec<BezPath> {
    app.editor.doc.iter().filter(|s| s.id != id).flat_map(|s| s.leaves()).filter(|l| !l.is_image()).map(|l| l.outline()).collect()
}

fn say(st: &mut State, app: &mut DarkBurn, msg: impl Into<String>) {
    let m = msg.into();
    app.shape_ui.status = Some(m.clone());
    st.status = Some(m);
}

fn run_cmd(app: &mut DarkBurn, ctx: &egui::Context, st: &mut State, id: &str, args: Value) -> bool {
    match crate::api::execute(app, ctx, id, &args) {
        Ok(v) => {
            if let Some(n) = v.get("changed").and_then(Value::as_bool) {
                return n;
            }
            true
        }
        Err(e) => {
            say(st, app, e);
            false
        }
    }
}

/// Write `edits` (document-space paths) in one undo step.
fn commit(app: &mut DarkBurn, label: &str, edits: Vec<(ShapeId, BezPath)>) -> bool {
    app.editor.edit(label, |d, _| {
        let mut any = false;
        for (id, p) in edits {
            any |= crate::shape_ops::set_shape_path(d, id, p);
        }
        any
    })
}

/// Handle the pointer for the active shape tool for one frame.
pub fn interact(ui: &Ui, app: &mut DarkBurn, response: &Response, xf: &Xf, m: Mods, raw: Option<Point>) {
    let ctx = ui.ctx().clone();
    let mut st = ctx.data_mut(|d| d.get_temp::<State>(state_id())).unwrap_or_default();
    if st.tool != app.tool {
        cancel_gesture(app, &mut st);
        st = State { tool: app.tool, ..State::default() };
    }
    st.canvas = Some(response.rect);
    st.hover = raw;
    st.snapped = None;
    // The keys are the tool's only while nothing else has them: no field is being typed in and no
    // dialog, window or menu is open over the canvas.
    let free = ctx.memory(|mem| mem.focused().is_none()) && !egui::Popup::is_any_open(&ctx) && !crate::escape::window_open(&ctx);
    match app.tool {
        Tool::Nodes => nodes_tool(ui, &ctx, app, &mut st, response, xf, m, raw, free),
        Tool::Tabs => tabs_tool(&ctx, app, &mut st, response, xf, raw),
        Tool::Measure => measure_tool(ui, &ctx, app, &mut st, response, xf, m, raw, free),
        Tool::Warp | Tool::Deform => warp_tool(ui, &ctx, app, &mut st, response, xf, m, raw, free),
        Tool::StartPoints => start_tool(&ctx, app, &mut st, response, xf, m, raw, free),
        Tool::RoundCorners => round_tool(&ctx, app, &mut st, response, xf, m, raw),
        Tool::Mirror | Tool::TwoPoint => pick_tool(&ctx, app, &mut st, response, xf, m, raw, free),
        Tool::PositionLaser => crate::laser_position::interact(app, response, raw),
        _ => {}
    }
    if !matches!(st.gesture, Gesture::None) {
        ctx.request_repaint();
    }
    ctx.data_mut(|d| d.insert_temp(state_id(), st));
}

fn cancel_gesture(app: &mut DarkBurn, st: &mut State) {
    if matches!(st.gesture, Gesture::Nodes { .. } | Gesture::Handle { .. } | Gesture::Curve { .. }) {
        app.editor.drop_checkpoint();
    }
    st.gesture = Gesture::None;
}

// ================================================================ node tool

#[allow(clippy::too_many_arguments)]
fn nodes_tool(
    ui: &Ui,
    ctx: &egui::Context,
    app: &mut DarkBurn,
    st: &mut State,
    response: &Response,
    xf: &Xf,
    m: Mods,
    raw: Option<Point>,
    free: bool,
) {
    let tol = xf.mm(HIT_PX);
    let targets = plain_selected(app);
    st.nodes.retain(|(id, _)| targets.iter().any(|t| t.0 == *id));
    if st.seg.is_some_and(|(id, _, _)| !targets.iter().any(|t| t.0 == id)) {
        st.seg = None;
    }
    let cs: Vec<(ShapeId, Vec<Contour>)> = targets.iter().map(|(id, p)| (*id, contours(p))).collect();

    // Press: handle, node, segment, shape or box.
    if response.drag_started_by(PointerButton::Primary)
        && matches!(st.gesture, Gesture::None)
        && let Some(origin) = press_origin(ui, xf)
    {
        let handle = st.nodes.iter().find_map(|(id, n)| {
            let c = cs.iter().find(|c| c.0 == *id)?.1.get(n.subpath)?;
            handles_of(c, n.node).into_iter().find(|(_, h)| h.distance(origin) <= tol).map(|(k, _)| (*id, *n, k))
        });
        if let Some((id, node, kind)) = handle {
            let orig = targets.iter().find(|t| t.0 == id).map(|t| t.1.clone()).unwrap_or_default();
            app.editor.checkpoint("Move handle");
            st.gesture = Gesture::Handle { shape: id, orig, node, kind, moved: false };
        } else if let Some((id, n)) = cs.iter().find_map(|(id, c)| hit_node(c, origin, tol).map(|n| (*id, n))) {
            if !st.nodes.contains(&(id, n)) {
                if m.shift {
                    st.nodes.push((id, n));
                } else {
                    st.nodes = vec![(id, n)];
                }
            }
            st.seg = None;
            let origs: Vec<(ShapeId, BezPath, Vec<NodeRef>)> = targets
                .iter()
                .map(|(sid, p)| (*sid, p.clone(), st.nodes.iter().filter(|(i, _)| i == sid).map(|(_, n)| *n).collect::<Vec<_>>()))
                .filter(|o| !o.2.is_empty())
                .collect();
            app.editor.checkpoint("Move nodes");
            st.gesture = Gesture::Nodes { origs, origin, moved: false };
        } else if let Some((id, orig, (sub, seg, t))) = targets.iter().find_map(|(id, p)| hit_segment(p, origin, tol).map(|h| (*id, p.clone(), h)))
            && !m.ctrl
        {
            app.editor.checkpoint("Curve segment");
            st.gesture = Gesture::Curve { shape: id, orig, sub, seg, t, moved: false };
        } else if let Some(id) = select::hit(&app.editor.doc, origin, tol) {
            if !app.editor.selection.contains(id) {
                app.editor.selection.set([id]);
                st.nodes.clear();
                st.seg = None;
            }
        } else {
            st.gesture = Gesture::Box { start: origin, now: origin };
        }
    }

    // Gesture updates.
    let down = ui.input(|i| i.pointer.primary_down());
    let mut gesture = std::mem::take(&mut st.gesture);
    match &mut gesture {
        Gesture::Nodes { origs, origin, moved } => {
            if let Some(p) = raw {
                let exclude: Vec<ShapeId> = origs.iter().map(|o| o.0).collect();
                let moving: Vec<Point> = origs
                    .iter()
                    .flat_map(|(_, path, ns)| node_points(&contours(path)).into_iter().filter(|(n, _)| ns.contains(n)).map(|x| x.1))
                    .collect();
                let anchor = moving.first().copied().unwrap_or(*origin);
                let mut q = if m.shift { constrain(*origin, p) } else { p };
                // Snap the dragged node (not the pointer) onto other features, own nodes first.
                let target = q + (anchor - *origin);
                let own = origs
                    .iter()
                    .flat_map(|(_, path, ns)| node_points(&contours(path)).into_iter().filter(|(n, _)| !ns.contains(n)).map(|x| x.1))
                    .min_by(|a, b| a.distance(target).total_cmp(&b.distance(target)))
                    .filter(|n| n.distance(target) <= tol);
                let snapped = if m.shift || m.ctrl {
                    target
                } else if let Some(n) = own {
                    n
                } else {
                    let (s, info) = snap_pt(app, xf, target, m, &exclude);
                    st.snapped = info;
                    s
                };
                q = snapped - (anchor - *origin);
                let delta = q - *origin;
                if delta.hypot() > 1e-9 || *moved {
                    *moved = true;
                    for (id, path, ns) in origs.iter() {
                        let new = move_nodes(path, ns, delta);
                        crate::shape_ops::set_shape_path(&mut app.editor.doc, *id, new);
                    }
                    app.editor.touch();
                }
            }
            if !down {
                if *moved {
                    for (id, _, _) in origs.iter() {
                        if let Some(p) = crate::shape_ops::shape_path(&app.editor.doc, *id) {
                            let j = join_touching_ends(&p);
                            if j != p {
                                crate::shape_ops::set_shape_path(&mut app.editor.doc, *id, j);
                                st.nodes.retain(|(i, _)| i != id);
                                app.editor.touch();
                            }
                        }
                    }
                } else {
                    app.editor.drop_checkpoint();
                }
                gesture = Gesture::None;
            }
        }
        Gesture::Handle { shape, orig, node, kind, moved } => {
            if let Some(p) = raw {
                let (q, info) = snap_pt(app, xf, p, m, &[*shape]);
                st.snapped = info;
                let to = if m.shift { constrain(orig_node(orig, *node).unwrap_or(q), q) } else { q };
                *moved = true;
                crate::shape_ops::set_shape_path(&mut app.editor.doc, *shape, move_handle(orig, *node, *kind, to));
                app.editor.touch();
            }
            if !down {
                if !*moved {
                    app.editor.drop_checkpoint();
                }
                gesture = Gesture::None;
            }
        }
        Gesture::Curve { shape, orig, sub, seg, t, moved } => {
            if let Some(p) = raw
                && let Some(new) = drag_curve(orig, *sub, *seg, *t, p)
            {
                *moved = true;
                crate::shape_ops::set_shape_path(&mut app.editor.doc, *shape, new);
                app.editor.touch();
            }
            if !down {
                if !*moved {
                    app.editor.drop_checkpoint();
                }
                gesture = Gesture::None;
            }
        }
        Gesture::Box { now, start } => {
            if let Some(p) = raw {
                *now = p;
            }
            if !down {
                let r = Rect::from_points(*start, *now);
                let mut picked = Vec::new();
                for (id, c) in &cs {
                    for (n, p) in node_points(c) {
                        if r.contains(p) {
                            picked.push((*id, n));
                        }
                    }
                }
                if m.shift {
                    for x in picked {
                        if !st.nodes.contains(&x) {
                            st.nodes.push(x);
                        }
                    }
                } else {
                    st.nodes = picked;
                }
                st.seg = None;
                gesture = Gesture::None;
            }
        }
        _ => {}
    }
    st.gesture = gesture;

    // Clicks: select node or segment, Ctrl+click inserts.
    if response.clicked_by(PointerButton::Primary)
        && let Some(p) = raw
    {
        if let Some((id, n)) = cs.iter().find_map(|(id, c)| hit_node(c, p, tol).map(|n| (*id, n))) {
            if m.shift {
                if let Some(i) = st.nodes.iter().position(|x| *x == (id, n)) {
                    st.nodes.remove(i);
                } else {
                    st.nodes.push((id, n));
                }
            } else {
                st.nodes = vec![(id, n)];
            }
            st.seg = None;
        } else if let Some((id, path, (sub, seg, t))) = targets.iter().find_map(|(id, path)| hit_segment(path, p, tol).map(|h| (*id, path, h))) {
            if m.ctrl {
                if let Ok(new) = gnode::insert_node(path, sub, seg, t) {
                    commit(app, "Insert node", vec![(id, new)]);
                    st.nodes.clear();
                    st.seg = None;
                }
            } else {
                st.seg = Some((id, sub, seg));
                st.nodes.clear();
            }
        } else if let Some(id) = select::hit(&app.editor.doc, p, tol) {
            if m.shift {
                app.editor.selection.add(id);
            } else {
                app.editor.selection.set([id]);
            }
            st.nodes.clear();
            st.seg = None;
        } else if !m.shift {
            st.nodes.clear();
            st.seg = None;
        }
    }

    if !free {
        return;
    }
    // Escape cancels a gesture in progress.
    if !matches!(st.gesture, Gesture::None) {
        if take_escape(ctx).is_some() {
            cancel_gesture(app, st);
        }
        return;
    }
    node_keys(ctx, app, st, raw, tol);
}

fn orig_node(path: &BezPath, n: NodeRef) -> Option<Point> {
    contours(path).get(n.subpath)?.node(n.node)
}

/// Keys the node tool handles itself, so the shell must leave them alone while it is active:
/// every plain key, and Ctrl+D (delete segment). Delete, Backspace and Ctrl+D fall back to the
/// ordinary delete and duplicate commands when no node or segment is selected.
pub fn owns_shortcut(s: &egui::KeyboardShortcut) -> bool {
    s.modifiers == Modifiers::NONE || (s.modifiers.command && !s.modifiers.shift && !s.modifiers.alt && s.logical_key == Key::D)
}

fn node_keys(ctx: &egui::Context, app: &mut DarkBurn, st: &mut State, raw: Option<Point>, tol: f64) {
    // Nudge the selected nodes.
    for key in [Key::ArrowLeft, Key::ArrowRight, Key::ArrowUp, Key::ArrowDown] {
        while let Some(m) = take_key(ctx, key) {
            if st.nodes.is_empty() {
                // Nothing selected: let the shapes move as usual (put the key back).
                ctx.input_mut(|i| i.events.push(egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: m }));
                break;
            }
            if m.alt {
                run_node_op(ctx, app, st, key_op(key, m).unwrap_or(NodeOp::Insert), raw, tol);
                continue;
            }
            let step = crate::tools::nudge_step(m.shift, m.command);
            let d = match key {
                Key::ArrowLeft => Vec2::new(-step, 0.0),
                Key::ArrowRight => Vec2::new(step, 0.0),
                Key::ArrowUp => Vec2::new(0.0, step),
                _ => Vec2::new(0.0, -step),
            };
            let edits: Vec<(ShapeId, BezPath)> = plain_selected(app)
                .into_iter()
                .filter_map(|(id, p)| {
                    let ns: Vec<NodeRef> = st.nodes.iter().filter(|(i, _)| *i == id).map(|(_, n)| *n).collect();
                    (!ns.is_empty()).then(|| (id, move_nodes(&p, &ns, d)))
                })
                .collect();
            commit(app, "Nudge nodes", edits);
        }
    }
    // Delete and Backspace remove the selected nodes (or segment); with none selected they delete
    // the shapes as everywhere else. Ctrl+D deletes the selected segment, else duplicates.
    for key in [Key::Delete, Key::Backspace, Key::D] {
        let taken = ctx.input_mut(|i| {
            let pos = i.events.iter().position(
                |e| matches!(e, egui::Event::Key { key: k, pressed: true, modifiers, .. } if *k == key && (key != Key::D || modifiers.command)),
            )?;
            match i.events.remove(pos) {
                egui::Event::Key { modifiers, .. } => Some(modifiers),
                _ => None,
            }
        });
        let Some(m) = taken else { continue };
        let have = !st.nodes.is_empty() || st.seg.is_some();
        if have {
            let op = if key == Key::D || st.nodes.is_empty() { NodeOp::DeleteSegment } else { NodeOp::Delete };
            run_node_op(ctx, app, st, op, raw, tol);
        } else if key == Key::D {
            run_cmd(app, ctx, st, "edit.duplicate", json!({}));
        } else if !m.alt {
            run_cmd(app, ctx, st, "edit.delete", json!({}));
        }
    }
    for key in [Key::I, Key::D, Key::C, Key::S, Key::L, Key::V, Key::B, Key::T, Key::E, Key::H] {
        // Alt+H and Alt+V align; the plain H and V are not ours.
        if matches!(key, Key::H) {
            if let Some(m) = take_key(ctx, key) {
                if m.alt {
                    run_node_op(ctx, app, st, NodeOp::Align(gnode::Align::CenterH), raw, tol);
                } else {
                    ctx.input_mut(|i| i.events.push(egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: m }));
                }
            }
            continue;
        }
        if let Some(m) = take_key(ctx, key) {
            if key == Key::V && m.alt {
                run_node_op(ctx, app, st, NodeOp::Align(gnode::Align::CenterV), raw, tol);
            } else if let Some(op) = key_op(key, m) {
                run_node_op(ctx, app, st, op, raw, tol);
            }
        }
    }
}

fn run_node_op(_ctx: &egui::Context, app: &mut DarkBurn, st: &mut State, op: NodeOp, raw: Option<Point>, tol: f64) {
    let at = raw.unwrap_or_default();
    let targets = plain_selected(app);
    let mut edits = Vec::new();
    let mut last_err = None;
    let needs_pointer = matches!(op, NodeOp::Insert | NodeOp::Trim);
    for (id, path) in &targets {
        let nodes: Vec<NodeRef> = st.nodes.iter().filter(|(i, _)| i == id).map(|(_, n)| *n).collect();
        let seg = st.seg.filter(|s| s.0 == *id).map(|s| (s.1, s.2));
        if nodes.is_empty() && seg.is_none() && !(needs_pointer && query::distance_to_path(path, at) <= tol * 3.0) {
            continue;
        }
        let seg_for_op = if matches!(op, NodeOp::Insert) && !nodes.is_empty() { None } else { seg };
        match apply_op(path, op, &nodes, seg_for_op, at, &others_of(app, *id)) {
            Ok(new) if new != *path => edits.push((*id, new)),
            Ok(_) => {}
            Err(e) => last_err = Some(e),
        }
    }
    if edits.is_empty() {
        if let Some(e) = last_err {
            say(st, app, e);
        }
        return;
    }
    commit(app, "Edit nodes", edits);
    if !matches!(op, NodeOp::Align(_)) {
        st.nodes.clear();
        st.seg = None;
    }
}

// ================================================================ tabs

/// A shape's manual tabs in document coordinates.
pub fn tab_points(app: &DarkBurn, id: ShapeId) -> Vec<Point> {
    crate::shape_nodes::tab_points(&app.editor.doc, id)
}

/// The shape under `p` that can carry tabs and start points: a plain shape, or a group (its
/// shapes carry them).
fn cut_hit(app: &DarkBurn, p: Point, tol: f64) -> Option<ShapeId> {
    let id = select::hit(&app.editor.doc, p, tol)?;
    crate::shape_nodes::cut_outline(&app.editor.doc, id).map(|_| id)
}

fn tab_size(app: &DarkBurn, id: ShapeId) -> f64 {
    app.editor.doc.get(id).map(|s| app.editor.doc.layer(s.layer).main().tabs.size).unwrap_or(1.0).max(0.1)
}

/// The plain shape under `p`, if any.
fn plain_hit(app: &DarkBurn, p: Point, tol: f64) -> Option<ShapeId> {
    let id = select::hit(&app.editor.doc, p, tol)?;
    crate::shape_ops::shape_path(&app.editor.doc, id).map(|_| id)
}

fn tabs_tool(ctx: &egui::Context, app: &mut DarkBurn, st: &mut State, response: &Response, xf: &Xf, raw: Option<Point>) {
    let tol = xf.mm(HIT_PX);
    if response.clicked_by(PointerButton::Primary)
        && let Some(p) = raw
    {
        let mut ids = app.editor.selected();
        if let Some(h) = cut_hit(app, p, tol)
            && !ids.contains(&h)
        {
            ids.push(h);
        }
        // An existing tab under the pointer comes off.
        for id in &ids {
            let size = tab_size(app, *id);
            if tab_points(app, *id).iter().any(|t| t.distance(p) <= (size / 2.0).max(tol)) {
                app.editor.selection.set([*id]);
                run_cmd(app, ctx, st, "tabs.remove", json!({ "shape": id.0, "x": p.x, "y": p.y, "radius": (size / 2.0).max(tol) }));
                return;
            }
        }
        if let Some(id) = cut_hit(app, p, tol) {
            app.editor.selection.set([id]);
            run_cmd(app, ctx, st, "tabs.add", json!({ "shape": id.0, "x": p.x, "y": p.y }));
        }
    }
}

// ================================================================ measure

fn measure_tool(
    ui: &Ui,
    ctx: &egui::Context,
    app: &mut DarkBurn,
    st: &mut State,
    response: &Response,
    xf: &Xf,
    m: Mods,
    raw: Option<Point>,
    free: bool,
) {
    let mut mem = ctx.data_mut(|d| d.get_temp::<MeasureMem>(measure_id())).unwrap_or_default();
    // The pointer, snapped; Shift locks an axis from the last point.
    st.hover = raw.map(|p| {
        let (q, info) = snap_pt(app, xf, p, m, &[]);
        st.snapped = info;
        match (m.shift, mem.pts.last()) {
            (true, Some(last)) if mem.pts.len() < 3 => snap::axis_lock(*last, q),
            _ => q,
        }
    });
    // Selection numbers, recomputed when the selection or document changes.
    let key = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (app.editor.revision(), app.editor.selected().iter().map(|i| i.0).collect::<Vec<u64>>()).hash(&mut h);
        h.finish()
    };
    if st.sel_info.0 != key || st.sel_info.1.is_empty() {
        let lines = match crate::shape_ops::execute(app, "measure.selection", &Value::Null) {
            Some(Ok(v)) => selection_lines(&v, mem.inches),
            _ => Vec::new(),
        };
        st.sel_info = (key, lines);
    }
    // Click on the readout switches units; elsewhere it adds a point.
    let readout = egui::Rect::from_min_size(response.rect.left_top() + vec2(10.0, 10.0), vec2(260.0, 120.0));
    let _ = ui;
    if response.clicked_by(PointerButton::Primary)
        && let Some(sp) = ui.input(|i| i.pointer.interact_pos())
    {
        if readout.contains(sp) {
            mem.inches = !mem.inches;
            st.sel_info.0 = u64::MAX;
        } else if let Some(p) = st.hover {
            if mem.pts.len() >= 3 {
                mem.pts.clear();
            }
            mem.pts.push(p);
        }
    }
    if free {
        if take_key(ctx, Key::U).is_some() {
            mem.inches = !mem.inches;
            st.sel_info.0 = u64::MAX;
        }
        if take_escape(ctx).is_some() {
            if mem.pts.is_empty() {
                app.set_tool(Tool::Select);
            }
            mem.pts.clear();
        }
    }
    ctx.data_mut(|d| d.insert_temp(measure_id(), mem));
}

/// The points the measure tool has so far (for tests).
#[cfg(test)]
pub fn measure_points(ctx: &egui::Context) -> Vec<Point> {
    ctx.data_mut(|d| d.get_temp::<MeasureMem>(measure_id())).map(|m| m.pts).unwrap_or_default()
}

/// The selection lines the measure tool shows (for tests).
#[cfg(test)]
pub fn measure_selection_lines(ctx: &egui::Context) -> Vec<String> {
    ctx.data_mut(|d| d.get_temp::<State>(state_id())).map(|s| s.sel_info.1).unwrap_or_default()
}

fn selection_lines(v: &Value, inches: bool) -> Vec<String> {
    let f = |k: &str| v.get(k).and_then(Value::as_f64).unwrap_or(0.0);
    vec![
        format!("Selection {} × {}", fmt_len(f("width"), inches), fmt_len(f("height"), inches)),
        format!("Perimeter {}  Area {}", fmt_len(f("length"), inches), fmt_area(f("area"), inches)),
    ]
}

// ================================================================ warp and deform

fn warp_tool(
    ui: &Ui,
    ctx: &egui::Context,
    app: &mut DarkBurn,
    st: &mut State,
    response: &Response,
    xf: &Xf,
    m: Mods,
    raw: Option<Point>,
    free: bool,
) {
    let deform = app.tool == Tool::Deform;
    let ids = crate::shape_ops::in_draw_order(&app.editor.doc, &app.editor.selected());
    let bbox = app.editor.doc.bbox_of(&ids).filter(|r| r.width() > 1e-9 && r.height() > 1e-9);
    // The handles start again on the box of the shapes when they were moved or changed.
    if st.warp.as_ref().is_none_or(|w| w.ids != ids || Some(w.src) != bbox) {
        st.warp = None;
        if let Some(src) = bbox {
            let pts = warp_points(src, deform);
            st.warp = Some(WarpState { ids: ids.clone(), src, preview: Vec::new(), pts, perspective: true });
        }
    }
    let Some(mut w) = st.warp.take() else { return };
    let tol = xf.mm(HIT_PX + 2.0);
    if response.drag_started_by(PointerButton::Primary)
        && let Some(o) = press_origin(ui, xf)
        && let Some(i) = w.pts.iter().position(|p| p.distance(o) <= tol)
    {
        st.gesture = Gesture::Warp(i);
    }
    let mut changed = false;
    if let Gesture::Warp(i) = st.gesture {
        if let Some(p) = raw {
            let (q, info) = snap_pt(app, xf, p, m, &ids);
            st.snapped = info;
            if w.pts[i] != q {
                w.pts[i] = q;
                changed = true;
            }
        }
        if !ui.input(|i| i.pointer.primary_down()) {
            st.gesture = Gesture::None;
        }
    }
    if free {
        if !deform && take_key(ctx, Key::B).is_some() {
            w.perspective = !w.perspective;
            changed = true;
        }
        if take_escape(ctx).is_some() {
            if matches!(st.gesture, Gesture::Warp(_)) {
                st.gesture = Gesture::None;
            }
            app.set_tool(Tool::Select);
            return;
        }
        if take_key(ctx, Key::Enter).is_some() {
            let ok = if deform {
                let grid: Vec<Value> = w.pts.iter().map(|p| json!([p.x, p.y])).collect();
                run_cmd(app, ctx, st, "tools.deform", json!({ "grid": grid }))
            } else {
                let corners: Vec<Value> = w.pts.iter().map(|p| json!([p.x, p.y])).collect();
                run_cmd(app, ctx, st, "tools.warp", json!({ "corners": corners, "mode": if w.perspective { "perspective" } else { "bilinear" } }))
            };
            if ok {
                app.set_tool(Tool::Select);
            }
            return;
        }
    }
    if changed || w.preview.is_empty() {
        let paths: Vec<BezPath> = ids.iter().flat_map(|i| crate::shape_ops::parts_of(&app.editor.doc, *i)).map(|p| p.path).collect();
        w.preview = warp_preview(&paths, w.src, &w.pts, deform, w.perspective);
    }
    st.warp = Some(w);
}

// ================================================================ start points

fn start_tool(ctx: &egui::Context, app: &mut DarkBurn, st: &mut State, response: &Response, xf: &Xf, m: Mods, raw: Option<Point>, free: bool) {
    let tol = xf.mm(HIT_PX);
    let targets = marker_selected(app);
    if free && let Some(_) = take_key(ctx, Key::R) {
        run_cmd(app, ctx, st, "shape.reverse_direction", json!({}));
        return;
    }
    let Some(p) = raw else { return };
    if !response.clicked_by(PointerButton::Primary) {
        return;
    }
    // The direction arrow of a start marker reverses the shape.
    for (id, path) in &targets {
        for (s, _, dir) in start_markers(path) {
            if (s + dir * xf.mm(16.0)).distance(p) <= tol {
                run_cmd(app, ctx, st, "shape.reverse_direction", json!({ "shape": id.0 }));
                return;
            }
        }
    }
    let near = targets
        .iter()
        .filter_map(|(id, path)| query::nearest_point(path, p).map(|h| (*id, h.distance)))
        .filter(|(_, d)| *d <= tol)
        .min_by(|a, b| a.1.total_cmp(&b.1));
    let id = match near {
        Some((id, _)) => id,
        None => match cut_hit(app, p, tol) {
            Some(id) => {
                app.editor.selection.set([id]);
                return;
            }
            None => return,
        },
    };
    let at = start_target(&targets.iter().find(|t| t.0 == id).map(|t| t.1.clone()).unwrap_or_default(), p, tol);
    run_cmd(app, ctx, st, "shape.set_start_point", json!({ "shape": id.0, "x": at.x, "y": at.y, "all": m.shift }));
}

/// Where the start would go for a click at `p`: a node within `tol`, else the point on the
/// outline.
pub fn start_target(path: &BezPath, p: Point, tol: f64) -> Point {
    let cs = contours(path);
    match hit_node(&cs, p, tol).and_then(|n| cs[n.subpath].node(n.node)) {
        Some(n) => n,
        None => query::nearest_point(path, p).map(|h| h.point).unwrap_or(p),
    }
}

// ================================================================ round corners

fn round_tool(ctx: &egui::Context, app: &mut DarkBurn, st: &mut State, response: &Response, xf: &Xf, m: Mods, raw: Option<Point>) {
    let tol = xf.mm(HIT_PX);
    let Some(p) = raw else { return };
    if !response.clicked_by(PointerButton::Primary) {
        return;
    }
    let radius = app.shape_ui.corner_radius;
    let mut cands = plain_selected(app);
    if let Some(h) = plain_hit(app, p, tol * 2.0)
        && !cands.iter().any(|c| c.0 == h)
        && let Some(path) = crate::shape_ops::shape_path(&app.editor.doc, h)
    {
        cands.push((h, path));
    }
    let hit = cands.iter().find_map(|(id, path)| corner_nodes(path).into_iter().find(|(_, q)| q.distance(p) <= tol).map(|(n, _)| (*id, n)));
    match hit {
        Some((id, n)) => {
            app.editor.selection.set([id]);
            run_cmd(app, ctx, st, "tools.round_corners", json!({ "radius": radius, "chamfer": m.shift, "nodes": [[n.subpath, n.node]] }));
        }
        None if m.ctrl && !app.editor.selection.is_empty() => {
            run_cmd(app, ctx, st, "tools.round_corners", json!({ "radius": radius, "chamfer": m.shift }));
        }
        None => {}
    }
}

// ================================================================ mirror and two-point

#[allow(clippy::too_many_arguments)]
fn pick_tool(ctx: &egui::Context, app: &mut DarkBurn, st: &mut State, response: &Response, xf: &Xf, m: Mods, raw: Option<Point>, free: bool) {
    st.hover = raw.map(|p| {
        let (q, info) = snap_pt(app, xf, p, m, &[]);
        st.snapped = info;
        q
    });
    if free && take_escape(ctx).is_some() {
        if st.pick.is_empty() {
            app.set_tool(Tool::Select);
        }
        st.pick.clear();
        return;
    }
    if !response.clicked_by(PointerButton::Primary) {
        return;
    }
    let Some(p) = st.hover else { return };
    if app.editor.selection.is_empty() {
        say(st, app, "Select the shapes first");
        return;
    }
    st.pick.push(p);
    let pts = st.pick.clone();
    if app.tool == Tool::Mirror && pts.len() == 2 {
        let ok = run_cmd(app, ctx, st, "arrange.mirror", json!({ "a": [pts[0].x, pts[0].y], "b": [pts[1].x, pts[1].y], "copy": m.ctrl }));
        st.pick.clear();
        if ok {
            app.set_tool(Tool::Select);
        }
    } else if app.tool == Tool::TwoPoint && pts.len() == 4 {
        let a = |q: Point| json!([q.x, q.y]);
        let ok = run_cmd(
            app,
            ctx,
            st,
            "arrange.two_point",
            json!({ "from_a": a(pts[0]), "from_b": a(pts[1]), "to_a": a(pts[2]), "to_b": a(pts[3]), "uniform": !m.shift }),
        );
        st.pick.clear();
        if ok {
            app.set_tool(Tool::Select);
        }
    }
}

/// The transform a two-point pick has so far (needs the first three points; the pointer is
/// the fourth until it is clicked).
pub fn two_point_preview(pick: &[Point], pointer: Option<Point>, uniform: bool) -> Option<Affine> {
    if pick.len() < 3 {
        return None;
    }
    let to_b = pick.get(3).copied().or(pointer)?;
    gxf::two_point_rotate_scale(pick[0], pick[1], pick[2], to_b, uniform)
}

// ================================================================ painting

fn draw_path(painter: &Painter, xf: &Xf, path: &BezPath, stroke: Stroke) {
    for (mut pts, closed) in render::screen_polylines(path, xf, true) {
        if closed && !pts.is_empty() {
            pts.push(pts[0]);
        }
        painter.add(egui::Shape::line(pts, stroke));
    }
}

fn node_square(painter: &Painter, p: Pos2, r: f32, fill: Color32, edge: Color32) {
    painter.rect(egui::Rect::from_center_size(p, vec2(r * 2.0, r * 2.0)), 0.0, fill, Stroke::new(1.0, edge), egui::StrokeKind::Middle);
}

fn seg_path(s: &PathSeg) -> BezPath {
    let mut b = BezPath::new();
    b.move_to(s.start());
    match s {
        PathSeg::Line(l) => b.line_to(l.p1),
        PathSeg::Quad(q) => b.quad_to(q.p1, q.p2),
        PathSeg::Cubic(c) => b.curve_to(c.p1, c.p2, c.p3),
    }
    b
}

fn arrow(painter: &Painter, from: Pos2, to: Pos2, colour: Color32) {
    painter.line_segment([from, to], Stroke::new(1.5, colour));
    let d = (to - from).normalized();
    let n = vec2(-d.y, d.x);
    painter.add(egui::Shape::convex_polygon(vec![to, to - d * 7.0 + n * 3.5, to - d * 7.0 - n * 3.5], colour, Stroke::NONE));
}

/// Draw the active tool's overlay.
pub fn paint(ui: &Ui, app: &DarkBurn, xf: &Xf, painter: &Painter, accent: Color32, dark: bool) {
    if !matches!(
        app.tool,
        Tool::Nodes | Tool::Tabs | Tool::Measure | Tool::Warp | Tool::Deform | Tool::StartPoints | Tool::RoundCorners | Tool::Mirror | Tool::TwoPoint
    ) {
        return;
    }
    let Some(st) = ui.ctx().data(|d| d.get_temp::<State>(state_id())) else { return };
    if st.tool != app.tool {
        return;
    }
    let text = if dark { Color32::from_gray(230) } else { Color32::from_gray(25) };
    let font = FontId::proportional(11.0);
    let hover = st.hover;
    let targets = if app.tool == Tool::StartPoints { marker_selected(app) } else { plain_selected(app) };
    let tol = xf.mm(HIT_PX);
    match app.tool {
        Tool::Nodes => {
            for (id, path) in &targets {
                let cs = contours(path);
                if let Some((sid, sub, seg)) = st.seg
                    && sid == *id
                    && let Some(s) = cs.get(sub).and_then(|c| c.segs.get(seg))
                {
                    let b = seg_path(s);
                    draw_path(painter, xf, &b, Stroke::new(3.0, accent));
                }
                if let Some(p) = hover.filter(|_| matches!(st.gesture, Gesture::None))
                    && let Some((sub, seg, _)) = hit_segment(path, p, tol)
                    && let Some(s) = cs.get(sub).and_then(|c| c.segs.get(seg))
                {
                    draw_path(painter, xf, &seg_path(s), Stroke::new(2.0, accent.gamma_multiply(0.5)));
                }
                for (n, q) in node_points(&cs) {
                    let sp = xf.to_screen(q);
                    let selected = st.nodes.contains(&(*id, n));
                    let hovered = hover.is_some_and(|h| q.distance(h) <= tol);
                    if n.node == 0 {
                        // The start node: bigger, orange, with the direction arrow.
                        painter.circle(sp, NODE_PX + 2.0, if selected { accent } else { ORANGE }, Stroke::new(1.0, text));
                        if let Some(s) = cs[n.subpath].segs.first() {
                            let d = gpath::seg_tangent(s, 0.0);
                            let tip = xf.to_screen(q + d * xf.mm(22.0));
                            arrow(painter, sp, tip, ORANGE);
                        }
                    } else {
                        node_square(
                            painter,
                            sp,
                            if hovered { NODE_PX + 1.5 } else { NODE_PX },
                            if selected { accent } else { Color32::TRANSPARENT },
                            if hovered { ORANGE } else { text },
                        );
                    }
                    if selected {
                        for (_, h) in handles_of(&cs[n.subpath], n.node) {
                            let hp = xf.to_screen(h);
                            painter.line_segment([sp, hp], Stroke::new(1.0, accent.gamma_multiply(0.7)));
                            painter.circle(hp, 3.5, accent, Stroke::new(1.0, text));
                        }
                    }
                }
            }
            if let Gesture::Box { start, now } = st.gesture {
                let r = egui::Rect::from_two_pos(xf.to_screen(start), xf.to_screen(now));
                painter.rect(r, 0.0, accent.gamma_multiply(0.12), Stroke::new(1.0, accent), egui::StrokeKind::Middle);
            }
        }
        Tool::Tabs => {
            let mut ids: Vec<ShapeId> = app.editor.selected();
            if let Some(h) = hover.and_then(|p| cut_hit(app, p, tol))
                && !ids.contains(&h)
            {
                ids.push(h);
            }
            for id in &ids {
                let Some(path) = crate::shape_nodes::cut_outline(&app.editor.doc, *id) else { continue };
                let size = tab_size(app, *id);
                for g in crate::shape_nodes::auto_gaps(&app.editor.doc, *id, false) {
                    painter.line_segment([xf.to_screen(g.from), xf.to_screen(g.to)], Stroke::new(5.0, ORANGE.gamma_multiply(0.8)));
                }
                for t in tab_points(app, *id) {
                    if let Some((a, b)) = tab_marker(&path, t, size) {
                        painter.line_segment([xf.to_screen(a), xf.to_screen(b)], Stroke::new(5.0, accent));
                    }
                    painter.circle_stroke(xf.to_screen(t), 4.0, Stroke::new(1.0, text));
                }
                if let Some(p) = hover
                    && let Some(h) = query::nearest_point(&path, p).filter(|h| h.distance <= tol * 2.0)
                    && let Some((a, b)) = tab_marker(&path, h.point, size)
                {
                    painter.line_segment([xf.to_screen(a), xf.to_screen(b)], Stroke::new(5.0, GREEN.gamma_multiply(0.7)));
                }
            }
        }
        Tool::Measure => {
            let mem = ui.ctx().data(|d| d.get_temp::<MeasureMem>(measure_id())).unwrap_or_default();
            let mut pts = mem.pts.clone();
            if mem.pts.len() < 3
                && let Some(h) = hover
            {
                pts.push(h);
            }
            for w in pts.windows(2).take(1) {
                painter.line_segment([xf.to_screen(w[0]), xf.to_screen(w[1])], Stroke::new(1.5, accent));
            }
            if mem.pts.len() >= 2 && pts.len() >= 3 {
                painter.line_segment([xf.to_screen(pts[1]), xf.to_screen(pts[2])], Stroke::new(1.0, accent.gamma_multiply(0.6)));
            }
            if pts.len() >= 3
                && let Some(a) = query::arc_through(pts[0], pts[1], pts[2])
            {
                let mut arc = BezPath::new();
                let steps = 64;
                let a0 = (pts[0] - a.center).atan2();
                for i in 0..=steps {
                    let q = a.center + Vec2::from_angle(a0 + a.sweep * i as f64 / steps as f64) * a.radius;
                    if i == 0 { arc.move_to(q) } else { arc.line_to(q) }
                }
                draw_path(painter, xf, &arc, Stroke::new(1.5, GREEN));
            }
            for p in &mem.pts {
                painter.circle(xf.to_screen(*p), 3.5, accent, Stroke::new(1.0, text));
            }
            let mut lines = measure_lines(&mem.pts, hover, mem.inches);
            lines.extend(st.sel_info.1.iter().cloned());
            lines.push(format!("Click here or press U for {}", if mem.inches { "mm" } else { "inch" }));
            if let Some(c) = st.canvas {
                let r = egui::Rect::from_min_size(c.left_top() + vec2(10.0, 10.0), vec2(260.0, 14.0 * lines.len() as f32 + 8.0));
                painter.rect(
                    r,
                    3.0,
                    if dark { Color32::from_black_alpha(190) } else { Color32::from_white_alpha(220) },
                    Stroke::new(1.0, accent),
                    egui::StrokeKind::Middle,
                );
                for (i, l) in lines.iter().enumerate() {
                    painter.text(r.left_top() + vec2(6.0, 4.0 + 14.0 * i as f32), Align2::LEFT_TOP, l, font.clone(), text);
                }
            }
        }
        Tool::Warp | Tool::Deform => {
            if let Some(w) = &st.warp {
                for p in &w.preview {
                    draw_path(painter, xf, p, Stroke::new(1.5, accent));
                }
                let deform = app.tool == Tool::Deform;
                let q: Vec<Pos2> = w.pts.iter().map(|p| xf.to_screen(*p)).collect();
                if deform {
                    for r in 0..4 {
                        for c in 0..4 {
                            if c < 3 {
                                painter.line_segment([q[r * 4 + c], q[r * 4 + c + 1]], Stroke::new(1.0, accent.gamma_multiply(0.5)));
                            }
                            if r < 3 {
                                painter.line_segment([q[r * 4 + c], q[(r + 1) * 4 + c]], Stroke::new(1.0, accent.gamma_multiply(0.5)));
                            }
                        }
                    }
                } else {
                    painter.add(egui::Shape::closed_line(q.clone(), Stroke::new(1.0, accent.gamma_multiply(0.7))));
                }
                for p in q {
                    painter.circle(p, 5.0, accent, Stroke::new(1.0, text));
                }
            }
        }
        Tool::StartPoints => {
            for (_, path) in &targets {
                for (s, e, dir) in start_markers(path) {
                    let sp = xf.to_screen(s);
                    painter.circle(sp, 5.0, GREEN, Stroke::new(1.0, text));
                    arrow(painter, sp, xf.to_screen(s + dir * xf.mm(16.0)), GREEN);
                    if let Some(e) = e {
                        node_square(painter, xf.to_screen(e), 4.5, RED, text);
                    }
                }
                if let Some(p) = hover {
                    let at = start_target(path, p, tol);
                    if at.distance(p) <= tol * 3.0 {
                        painter.circle_stroke(xf.to_screen(at), 8.0, Stroke::new(2.0, ORANGE));
                    }
                }
            }
        }
        Tool::RoundCorners => {
            let mut paths: Vec<BezPath> = targets.iter().map(|t| t.1.clone()).collect();
            if let Some(h) = hover.and_then(|p| plain_hit(app, p, tol * 2.0)).and_then(|id| crate::shape_ops::shape_path(&app.editor.doc, id)) {
                paths.push(h);
            }
            for path in &paths {
                for (_, q) in corner_nodes(path) {
                    let near = hover.is_some_and(|h| q.distance(h) <= tol);
                    painter.circle(
                        xf.to_screen(q),
                        if near { 7.0 } else { 3.0 },
                        if near { ORANGE } else { Color32::TRANSPARENT },
                        Stroke::new(1.0, if near { text } else { accent }),
                    );
                }
            }
        }
        Tool::Mirror | Tool::TwoPoint => {
            let sel = app.editor.selected();
            let outlines: Vec<BezPath> = sel.iter().flat_map(|i| crate::shape_ops::parts_of(&app.editor.doc, *i)).map(|p| p.path).collect();
            let dash = |a: Point, b: Point| {
                painter.extend(egui::Shape::dashed_line(&[xf.to_screen(a), xf.to_screen(b)], Stroke::new(1.0, accent), 5.0, 4.0))
            };
            if app.tool == Tool::Mirror {
                let line = match (st.pick.first(), st.pick.get(1).copied().or(hover)) {
                    (Some(a), Some(b)) => Some((*a, b)),
                    _ => None,
                };
                if let Some((a, b)) = line {
                    dash(a, b);
                    for p in &outlines {
                        draw_path(painter, xf, &gxf::mirror_path(p, a, b), Stroke::new(1.0, accent.gamma_multiply(0.7)));
                    }
                }
            } else {
                if st.pick.len() >= 2 {
                    dash(st.pick[0], st.pick[1]);
                }
                if st.pick.len() >= 3 {
                    dash(st.pick[2], st.pick.get(3).copied().or(hover).unwrap_or(st.pick[2]));
                }
                let shift = ui.input(|i| i.modifiers.shift);
                if let Some(a) = two_point_preview(&st.pick, hover, !shift) {
                    for p in &outlines {
                        draw_path(painter, xf, &(a * p.clone()), Stroke::new(1.0, accent.gamma_multiply(0.7)));
                    }
                }
            }
            for p in &st.pick {
                painter.circle(xf.to_screen(*p), 3.5, accent, Stroke::new(1.0, text));
            }
        }
        _ => {}
    }
    if let Some(s) = st.snapped {
        painter.circle_stroke(xf.to_screen(s.point), 5.0, Stroke::new(1.5, ORANGE));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prefs::Prefs;
    use darkburn_core::{Shape, ShapeKind};

    fn square() -> BezPath {
        Rect::new(0.0, 0.0, 10.0, 10.0).to_path_closed()
    }

    trait ToPathClosed {
        fn to_path_closed(&self) -> BezPath;
    }
    impl ToPathClosed for Rect {
        fn to_path_closed(&self) -> BezPath {
            let mut p = BezPath::new();
            p.move_to((self.x0, self.y0));
            p.line_to((self.x1, self.y0));
            p.line_to((self.x1, self.y1));
            p.line_to((self.x0, self.y1));
            p.close_path();
            p
        }
    }

    #[test]
    fn hit_testing_nodes_and_segments() {
        let cs = contours(&square());
        assert_eq!(hit_node(&cs, Point::new(10.2, 0.1), 0.5), Some(NodeRef::new(0, 1)));
        assert_eq!(hit_node(&cs, Point::new(5.0, 5.0), 0.5), None);
        let (sub, seg, t) = hit_segment(&square(), Point::new(5.0, 0.2), 0.5).unwrap();
        assert_eq!((sub, seg), (0, 0));
        assert!((t - 0.5).abs() < 1e-6);
        assert!(hit_segment(&square(), Point::new(5.0, 5.0), 0.5).is_none());
    }

    #[test]
    fn dragging_a_line_makes_it_pass_the_pointer() {
        let new = drag_curve(&square(), 0, 0, 0.5, Point::new(5.0, -3.0)).unwrap();
        let c = &contours(&new)[0];
        assert!(matches!(c.segs[0], PathSeg::Cubic(_)));
        assert!((c.segs[0].eval(0.5).y + 3.0).abs() < 1e-9);
        assert_eq!(c.segs[0].start(), Point::new(0.0, 0.0));
    }

    /// Feed one key press through `node_keys` inside an egui frame.
    fn press(app: &mut DarkBurn, st: &mut State, key: Key, modifiers: Modifiers) {
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            events: vec![egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }],
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| node_keys(ui.ctx(), app, st, Some(Point::new(0.0, 0.0)), 1.0));
        out.textures_delta.clear();
    }

    #[test]
    fn delete_and_ctrl_d_work_in_the_node_tool() {
        use crate::shape_ops::tests::{add_rect, app, run, select};
        let mut app = app();
        let r = add_rect(&mut app, 0.0, 0.0, 10.0, 10.0, 0);
        select(&mut app, &[r]);
        run(&mut app, "node.edit", json!({ "op": "insert", "subpath": 0, "seg": 0, "t": 0.5 })).unwrap();
        let count = |app: &DarkBurn| crate::shape_ops::shape_path(&app.editor.doc, r).map(|p| darkburn_geom::cleanup::node_count(&p)).unwrap_or(0);
        assert_eq!(count(&app), 5);
        let mut st = State::default();
        st.nodes.push((r, NodeRef::new(0, 1)));
        press(&mut app, &mut st, Key::Delete, Modifiers::NONE);
        assert_eq!(count(&app), 4, "Delete removed the selected node");
        // Nothing selected: Delete removes the shape, Ctrl+D duplicates it.
        let mut st = State::default();
        let n = app.editor.doc.len();
        press(&mut app, &mut st, Key::D, Modifiers::COMMAND);
        assert_eq!(app.editor.doc.len(), n + 1, "Ctrl+D duplicated");
        press(&mut app, &mut st, Key::Delete, Modifiers::NONE);
        assert_eq!(app.editor.doc.len(), n, "Delete deleted the selection");
    }

    #[test]
    fn the_node_tool_owns_delete_and_ctrl_d_but_not_other_ctrl_keys() {
        let sc = |m: Modifiers, k: Key| egui::KeyboardShortcut::new(m, k);
        assert!(owns_shortcut(&sc(Modifiers::NONE, Key::Delete)));
        assert!(owns_shortcut(&sc(Modifiers::COMMAND, Key::D)));
        assert!(!owns_shortcut(&sc(Modifiers::COMMAND, Key::C)));
        assert!(!owns_shortcut(&sc(Modifiers::COMMAND | Modifiers::SHIFT, Key::D)));
    }

    #[test]
    fn handles_keep_smooth_nodes_symmetric() {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.curve_to((3.0, 3.0), (7.0, 3.0), (10.0, 0.0));
        p.curve_to((13.0, -3.0), (17.0, -3.0), (20.0, 0.0));
        let n = NodeRef::new(0, 1);
        let c = &contours(&p)[0];
        assert!(is_smooth(c, 1));
        let q = move_handle(&p, n, HandleKind::Out, Point::new(10.0, -5.0));
        let c2 = &contours(&q)[0];
        let inn = handle_point(c2, 1, HandleKind::In).unwrap();
        assert!((inn.x - 10.0).abs() < 1e-9 && inn.y > 0.0, "in-handle swings opposite: {inn:?}");
        assert!(((inn - Point::new(10.0, 0.0)).hypot() - (Point::new(7.0, 3.0) - Point::new(10.0, 0.0)).hypot()).abs() < 1e-9);
        // A corner node moves one handle only.
        let corner = gnode::set_node_corner(&p, n).unwrap();
        let cc = &contours(&corner)[0];
        if !is_smooth(cc, 1) {
            let other = handle_point(cc, 1, HandleKind::In).unwrap();
            let q = move_handle(&corner, n, HandleKind::Out, Point::new(10.0, -5.0));
            assert_eq!(handle_point(&contours(&q)[0], 1, HandleKind::In).unwrap(), other);
        }
    }

    #[test]
    fn shift_constrains_to_45_degrees() {
        let q = constrain(Point::new(0.0, 0.0), Point::new(10.0, 1.0));
        assert!(q.y.abs() < 1e-9 && q.x > 9.0);
        let q = constrain(Point::new(0.0, 0.0), Point::new(7.0, 8.0));
        assert!((q.x - q.y).abs() < 1e-9);
    }

    #[test]
    fn keys_map_to_operations() {
        let none = Modifiers::NONE;
        assert_eq!(key_op(Key::I, none), Some(NodeOp::Insert));
        assert_eq!(key_op(Key::D, none), Some(NodeOp::Delete));
        assert_eq!(key_op(Key::D, Modifiers::SHIFT), Some(NodeOp::DeleteSegment));
        assert_eq!(key_op(Key::C, none), Some(NodeOp::Corner));
        assert_eq!(key_op(Key::S, none), Some(NodeOp::Smooth));
        assert_eq!(key_op(Key::L, none), Some(NodeOp::ToLine));
        assert_eq!(key_op(Key::B, none), Some(NodeOp::Break));
        assert_eq!(key_op(Key::T, none), Some(NodeOp::Trim));
        assert_eq!(key_op(Key::E, none), Some(NodeOp::Extend));
        assert_eq!(key_op(Key::ArrowLeft, Modifiers::ALT), Some(NodeOp::Align(gnode::Align::Left)));
        assert_eq!(key_op(Key::Q, none), None);
    }

    #[test]
    fn operations_on_paths() {
        let sq = square();
        let ins = apply_op(&sq, NodeOp::Insert, &[], Some((0, 0)), Point::ZERO, &[]).unwrap();
        assert_eq!(cleanup::node_count(&ins), 5);
        let del = apply_op(&ins, NodeOp::Delete, &[NodeRef::new(0, 1)], None, Point::ZERO, &[]).unwrap();
        assert_eq!(cleanup::node_count(&del), 4);
        let two = apply_op(&sq, NodeOp::Delete, &[NodeRef::new(0, 1), NodeRef::new(0, 2)], None, Point::ZERO, &[]).unwrap();
        assert_eq!(cleanup::node_count(&two), 2, "several nodes go in descending order");
        let brk = apply_op(&sq, NodeOp::Break, &[NodeRef::new(0, 2)], None, Point::ZERO, &[]).unwrap();
        assert!(!contours(&brk)[0].closed);
        let curved = apply_op(&sq, NodeOp::ToCurve, &[], Some((0, 1)), Point::ZERO, &[]).unwrap();
        assert!(matches!(contours(&curved)[0].segs[1], PathSeg::Cubic(_)));
        let line = apply_op(&curved, NodeOp::ToLine, &[], Some((0, 1)), Point::ZERO, &[]).unwrap();
        assert!(matches!(contours(&line)[0].segs[1], PathSeg::Line(_)));
        let al = apply_op(&sq, NodeOp::Align(gnode::Align::Left), &[NodeRef::new(0, 0), NodeRef::new(0, 1)], None, Point::ZERO, &[]).unwrap();
        assert!(contours(&al)[0].node(1).unwrap().x.abs() < 1e-9);
        assert!(apply_op(&sq, NodeOp::Delete, &[], None, Point::ZERO, &[]).is_err());
        let moved = move_nodes(&sq, &[NodeRef::new(0, 2)], Vec2::new(5.0, 5.0));
        assert_eq!(contours(&moved)[0].node(2), Some(Point::new(15.0, 15.0)));
    }

    fn line_and_square() -> BezPath {
        let mut p = BezPath::new();
        p.move_to((-20.0, 0.0));
        p.line_to((-10.0, 0.0));
        p.extend(square());
        p
    }

    #[test]
    fn deleting_every_node_of_a_subpath_removes_only_that_subpath() {
        // Both nodes of the line: the line goes, the square after it keeps its four nodes.
        let both = [NodeRef::new(0, 0), NodeRef::new(0, 1)];
        let out = apply_op(&line_and_square(), NodeOp::Delete, &both, None, Point::ZERO, &[]).unwrap();
        assert_eq!(contours(&out).len(), 1);
        assert_eq!(out, square());
        // Every node of the only subpath: nothing is left, and that is not an error.
        let mut line = BezPath::new();
        line.move_to((0.0, 0.0));
        line.line_to((10.0, 0.0));
        assert!(apply_op(&line, NodeOp::Delete, &both, None, Point::ZERO, &[]).unwrap().elements().is_empty());
        let all: Vec<NodeRef> = (0..4).map(|i| NodeRef::new(0, i)).collect();
        assert!(apply_op(&square(), NodeOp::Delete, &all, None, Point::ZERO, &[]).unwrap().elements().is_empty());
    }

    #[test]
    fn several_breaks_or_deleted_lines_on_a_closed_shape_hit_the_chosen_ones() {
        // Delete the bottom and the top line of the square: the right and the left line stay.
        let nodes = [NodeRef::new(0, 0), NodeRef::new(0, 2)];
        let out = apply_op(&square(), NodeOp::DeleteSegment, &nodes, None, Point::ZERO, &[]).unwrap();
        let cs = contours(&out);
        assert_eq!(cs.len(), 2);
        for c in &cs {
            assert_eq!(c.segs.len(), 1);
            assert!((c.start().unwrap().x - c.end().unwrap().x).abs() < 1e-9, "an upright line is left: {c:?}");
        }
        // Break at two opposite corners: two halves of two lines each, meeting at those corners.
        let nodes = [NodeRef::new(0, 1), NodeRef::new(0, 3)];
        let out = apply_op(&square(), NodeOp::Break, &nodes, None, Point::ZERO, &[]).unwrap();
        let cs = contours(&out);
        assert_eq!(cs.len(), 2);
        let corners = [Point::new(10.0, 0.0), Point::new(0.0, 10.0)];
        for c in &cs {
            assert!(!c.closed);
            assert_eq!(c.segs.len(), 2);
            assert!(corners.contains(&c.start().unwrap()) && corners.contains(&c.end().unwrap()), "{c:?}");
        }
    }

    #[test]
    fn the_end_node_of_an_open_line_does_not_stop_segment_edits() {
        // Every node of an open line selected: its end node has no line of its own.
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((10.0, 0.0));
        p.line_to((10.0, 10.0));
        let all = [NodeRef::new(0, 0), NodeRef::new(0, 1), NodeRef::new(0, 2)];
        let curved = apply_op(&p, NodeOp::ToCurve, &all, None, Point::ZERO, &[]).unwrap();
        assert!(contours(&curved)[0].segs.iter().all(|s| matches!(s, PathSeg::Cubic(_))));
        let lines = apply_op(&curved, NodeOp::ToLine, &all, None, Point::ZERO, &[]).unwrap();
        assert_eq!(lines, p);
        let gone = apply_op(&p, NodeOp::DeleteSegment, &all, None, Point::ZERO, &[]).unwrap();
        assert!(gone.elements().is_empty());
        // Only the end node: there is nothing to change, and the message says what to pick.
        let err = apply_op(&p, NodeOp::ToCurve, &[NodeRef::new(0, 2)], None, Point::ZERO, &[]).unwrap_err();
        assert!(err.contains("select"), "{err}");
    }

    #[test]
    fn deleting_the_last_nodes_of_a_shape_removes_the_shape() {
        use crate::shape_ops::tests::{app, run, select};
        let mut app = app();
        let id = ShapeId(run(&mut app, "shape.add_line", json!({ "points": [[0, 0], [10, 0]], "connect": false })).unwrap()["id"].as_u64().unwrap());
        let other = crate::shape_ops::tests::add_rect(&mut app, 50.0, 50.0, 10.0, 10.0, 0);
        select(&mut app, &[id]);
        // The node tool: both nodes selected, Delete.
        let mut st = State::default();
        st.nodes.push((id, NodeRef::new(0, 0)));
        st.nodes.push((id, NodeRef::new(0, 1)));
        press(&mut app, &mut st, Key::Delete, Modifiers::NONE);
        assert!(app.editor.doc.get(id).is_none(), "no empty shape is left behind");
        assert_eq!(app.editor.doc.len(), 1);
        assert!(app.editor.selected().is_empty());
        app.editor.undo();
        assert_eq!(crate::shape_ops::shape_path(&app.editor.doc, id).map(|p| cleanup::node_count(&p)), Some(2), "undo brings the line back");
        // The command: deleting one node of a two-node line leaves nothing to draw.
        select(&mut app, &[id]);
        run(&mut app, "node.edit", json!({ "op": "delete", "subpath": 0, "node": 0 })).unwrap();
        assert!(app.editor.doc.get(id).is_none());
        assert!(app.editor.doc.get(other).is_some());
        let bb = app.editor.doc.bbox_of(&darkburn_core::select::all(&app.editor.doc)).unwrap();
        assert!((bb.x0 - 50.0).abs() < 1e-9 && (bb.y0 - 50.0).abs() < 1e-9, "{bb:?}");
    }

    #[test]
    fn dropping_an_end_on_an_end_joins() {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((10.0, 0.0));
        p.move_to((10.0, 0.0));
        p.line_to((10.0, 10.0));
        let j = join_touching_ends(&p);
        assert_eq!(contours(&j).len(), 1);
        let mut far = BezPath::new();
        far.move_to((0.0, 0.0));
        far.line_to((10.0, 0.0));
        far.move_to((12.0, 0.0));
        far.line_to((12.0, 10.0));
        assert_eq!(contours(&join_touching_ends(&far)).len(), 2);
    }

    #[test]
    fn corners_start_markers_and_tabs() {
        assert_eq!(corner_nodes(&square()).len(), 4);
        let e = kurbo_ellipse();
        assert!(corner_nodes(&e).is_empty(), "smooth outlines have no corners");
        let m = start_markers(&square());
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].0, Point::new(0.0, 0.0));
        assert!(m[0].1.is_none(), "a closed shape has no end marker");
        let mut open = BezPath::new();
        open.move_to((0.0, 0.0));
        open.line_to((5.0, 0.0));
        assert_eq!(start_markers(&open)[0].1, Some(Point::new(5.0, 0.0)));
        let (a, b) = tab_marker(&square(), Point::new(5.0, 0.3), 2.0).unwrap();
        assert!((a.y).abs() < 1e-9 && (b.x - a.x - 2.0).abs() < 1e-9, "{a:?} {b:?}");
        assert_eq!(start_target(&square(), Point::new(9.8, 0.1), 0.5), Point::new(10.0, 0.0));
        assert_eq!(start_target(&square(), Point::new(5.0, 0.2), 0.5), Point::new(5.0, 0.0));
    }

    fn kurbo_ellipse() -> BezPath {
        use darkburn_core::kurbo::Shape as _;
        darkburn_core::kurbo::Circle::new((0.0, 0.0), 5.0).to_path(0.01)
    }

    #[test]
    fn measuring_and_units() {
        let l = measure_lines(&[Point::new(0.0, 0.0), Point::new(3.0, 4.0)], None, false);
        assert!(l[0].contains("5.000 mm") && l[2].contains("53.13"), "{l:?}");
        let inch = measure_lines(&[Point::new(0.0, 0.0), Point::new(25.4, 0.0)], None, true);
        assert!(inch[0].contains("1.0000 in"));
        let arc = measure_lines(&[Point::new(10.0, 0.0), Point::new(0.0, 10.0), Point::new(-10.0, 0.0)], None, false);
        assert!(arc.iter().any(|s| s.contains("Radius 10.000")) && arc.iter().any(|s| s.contains("180.00")), "{arc:?}");
        assert!(measure_lines(&[], None, false).is_empty());
    }

    #[test]
    fn warp_and_deform_previews() {
        let src = Rect::new(0.0, 0.0, 10.0, 10.0);
        let mut pts = warp_points(src, false);
        assert_eq!(pts.len(), 4);
        pts[2] = Point::new(20.0, 10.0);
        let pre = warp_preview(&[square()], src, &pts, false, true);
        assert_eq!(pre.len(), 1);
        assert!((gpath::bounding_box(&pre[0]).unwrap().x1 - 20.0).abs() < 0.05);
        let g = warp_points(src, true);
        assert_eq!(g.len(), 16);
        let id = warp_preview(&[square()], src, &g, true, true);
        assert!((gpath::bounding_box(&id[0]).unwrap().x1 - 10.0).abs() < 0.05, "an untouched grid changes nothing");
        assert!(warp_preview(&[square()], src, &pts[..3], false, true).is_empty());
    }

    #[test]
    fn two_point_preview_needs_three_points() {
        let pick = [Point::new(0.0, 0.0), Point::new(10.0, 0.0), Point::new(0.0, 0.0)];
        assert!(two_point_preview(&pick[..2], None, true).is_none());
        let a = two_point_preview(&pick, Some(Point::new(0.0, 20.0)), true).unwrap();
        assert!(((a * Point::new(10.0, 0.0)) - Point::new(0.0, 20.0)).hypot() < 1e-9);
    }

    /// The commit path the tools use: one undo step, a rectangle becomes a path.
    #[test]
    fn node_edits_are_one_undo_step_and_work_headless() {
        let mut app = DarkBurn::new(Prefs::default());
        let id = app.editor.edit("add", |d, sel| {
            let id = d.add(Shape::rect(Rect::new(0.0, 0.0, 10.0, 10.0), 0.0, 0));
            sel.set([id]);
            true
        });
        assert!(id);
        let sid = app.editor.selected()[0];
        let path = crate::shape_ops::shape_path(&app.editor.doc, sid).unwrap();
        let before = app.editor.history.len();
        let moved = move_nodes(&path, &[NodeRef::new(0, 2)], Vec2::new(3.0, 3.0));
        assert!(commit(&mut app, "Move nodes", vec![(sid, moved)]));
        assert_eq!(app.editor.history.len(), before + 1);
        assert!(matches!(app.editor.doc.get(sid).unwrap().kind, ShapeKind::Path { .. }));
        assert!((app.editor.doc.get(sid).unwrap().bbox().x1 - 13.0).abs() < 1e-9);
        // A tool-style gesture: checkpoint, live edits, nothing moved: dropped.
        app.editor.checkpoint("Move nodes");
        app.editor.drop_checkpoint();
        assert_eq!(app.editor.history.len(), before + 1);
        // Pick tools call the registered commands.
        let ctx = egui::Context::default();
        let mut st = State::default();
        app.tool = Tool::Mirror;
        assert!(run_cmd(&mut app, &ctx, &mut st, "arrange.mirror", json!({ "a": [0, 0], "b": [0, 10], "copy": false })));
        assert!(app.editor.doc.get(sid).unwrap().bbox().x1 <= 1e-9);
        assert!(run_cmd(&mut app, &ctx, &mut st, "tabs.add", json!({ "shape": sid.0, "x": -5.0, "y": 0.0 })));
        assert_eq!(tab_points(&app, sid).len(), 1);
        assert!(run_cmd(&mut app, &ctx, &mut st, "tools.round_corners", json!({ "radius": 1.0, "nodes": [[0, 0]] })));
        assert!(!run_cmd(&mut app, &ctx, &mut st, "node.edit", json!({ "op": "bogus" })));
        assert!(st.status.is_some());
    }

    #[test]
    fn a_frame_with_each_tool_draws() {
        let ctx = egui::Context::default();
        let mut app = DarkBurn::new(Prefs::default());
        app.editor.edit("add", |d, sel| {
            let id = d.add(Shape::rect(Rect::new(10.0, 10.0, 60.0, 40.0), 0.0, 1));
            sel.set([id]);
            true
        });
        for tool in
            [Tool::Nodes, Tool::Tabs, Tool::Measure, Tool::Warp, Tool::Deform, Tool::StartPoints, Tool::RoundCorners, Tool::Mirror, Tool::TwoPoint]
        {
            app.tool = tool;
            for _ in 0..2 {
                let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, vec2(1200.0, 900.0))), ..Default::default() };
                let mut out = ctx.run_ui(input, |ui| {
                    crate::canvas::show(ui, &mut app, None);
                });
                out.textures_delta.clear();
            }
            assert!(!hint(tool).is_empty());
        }
        assert_eq!(app.editor.doc.len(), 1, "drawing changes nothing");
    }

    /// KEY-04: the single letters work on the selected nodes through the real key handler.
    #[test]
    fn node_letter_keys_edit_the_selected_node() {
        let ctx = egui::Context::default();
        let press = |app: &mut DarkBurn, st: &mut State, key: Key| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0))),
                events: vec![egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }],
                ..Default::default()
            };
            let mut out = ctx.run_ui(input, |ui| node_keys(ui.ctx(), app, st, None, 1.0));
            out.textures_delta.clear();
        };
        let mut app = DarkBurn::new(Prefs::default());
        app.editor.edit("add", |d, sel| {
            let id = d.add(Shape::rect(Rect::new(0.0, 0.0, 10.0, 10.0), 0.0, 0));
            sel.set([id]);
            true
        });
        let sid = app.editor.selected()[0];
        // Make it a path first, as the node tool does.
        let path = crate::shape_ops::shape_path(&app.editor.doc, sid).unwrap();
        assert!(commit(&mut app, "To path", vec![(sid, path)]));
        let count = |app: &DarkBurn| cleanup::node_count(&crate::shape_ops::shape_path(&app.editor.doc, sid).unwrap());
        let mut st = State::default();
        assert_eq!(count(&app), 4);
        st.nodes = vec![(sid, NodeRef::new(0, 1))];
        press(&mut app, &mut st, Key::D);
        assert_eq!(count(&app), 3, "D deletes the node");
        st.nodes = vec![(sid, NodeRef::new(0, 1))];
        press(&mut app, &mut st, Key::B);
        assert!(!contours(&crate::shape_ops::shape_path(&app.editor.doc, sid).unwrap())[0].closed, "B breaks the path open");
        let before = count(&app);
        st.nodes.clear();
        st.seg = Some((sid, 0, 0));
        press(&mut app, &mut st, Key::I);
        assert_eq!(count(&app), before + 1, "I inserts a node on the selected segment");
    }
}
