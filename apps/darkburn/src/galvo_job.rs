//! A galvo job from the document [GALVO-06..08, GALVO-13, GALVO-14, GALVO-16, LT-17, LT-18]:
//! the layers map to `darkburn_galvo::Layer`s (line, fill, flood fill, 3D sliced image) with the
//! per-layer galvo fields of the cut settings (frequency, Q-pulse, wobble, timings, global
//! passes), and the job is compiled with the device's galvo settings.
//!
//! Coordinates: the document is the square field with its lower-left corner at the bed origin;
//! the board is centred, so every shape moves by minus half the field.

use darkburn_core::kurbo::{Affine, BezPath, Point, Rect, Shape as _};
use darkburn_core::layer::{self, CutSetting, Mode};
use darkburn_core::{Document, ShapeKind};
use darkburn_galvo as galvo;
use galvo::{AngleMode, FillGrouping, FillSettings, LayerMode, MarkParams};

use crate::devices::Device;
use crate::galvo_settings::GalvoDeviceSettings;
use crate::machine_api::{JobData, JobRequest};

/// A galvo job and what was left out of it.
#[derive(Clone, Debug)]
pub struct GalvoBuild {
    pub job: galvo::Job,
    pub warnings: Vec<String>,
}

fn mark_params(s: &CutSetting, dev: &GalvoDeviceSettings) -> MarkParams {
    let g = &s.galvo;
    MarkParams {
        frequency_khz: if g.frequency_khz > 0.0 { g.frequency_khz } else { dev.default_frequency_khz },
        q_pulse_ns: dev.source.has_q_pulse().then(|| g.q_pulse_ns.round().clamp(1.0, 100_000.0) as u32),
        power_pct: s.max_power.clamp(0.0, 100.0),
        speed_mm_s: s.speed.max(0.1),
        timings: g.timing_override.then(|| galvo::Timings {
            jump_speed_mm_s: g.jump_speed,
            min_jump_delay: g.min_jump_delay_us.round() as i32,
            max_jump_delay: g.max_jump_delay_us.round() as i32,
            jump_distance_limit_mm: g.jump_distance_limit,
            laser_on_tc: g.laser_on_tc_us.round() as i32,
            laser_off_tc: g.laser_off_tc_us.round() as i32,
            end_tc: g.end_tc_us.round() as i32,
            polygon_tc: g.polygon_tc_us.round() as i32,
        }),
    }
}

fn fill_settings(s: &CutSetting) -> FillSettings {
    FillSettings {
        interval_mm: s.interval.max(0.005),
        angle_deg: s.scan_angle,
        angle_mode: if s.galvo.auto_rotate {
            AngleMode::AutoRotate
        } else if s.angle_increment != 0.0 {
            AngleMode::PerPass { increment_deg: s.angle_increment }
        } else {
            AngleMode::Fixed
        },
        bidirectional: s.bidirectional,
        cross_hatch: s.cross_hatch,
        grouping: if s.fill_grouping == layer::FillGrouping::Individually { FillGrouping::Individually } else { FillGrouping::AllAtOnce },
    }
}

/// The depth image of a picture as it lies in the design, over its bounding box `bbox`
/// (document millimetres): dark is deep, row 0 at the top. The picture's turn, mirror and crop
/// are followed; outside the picture, outside its crop and where it is transparent the depth
/// is zero.
fn depth_of(doc: &Document, leaf: &darkburn_core::Shape, img: &darkburn_core::ImageShape, bbox: Rect) -> Option<(u32, u32, Vec<u8>)> {
    /// The most pixels along one side of a depth image.
    const MAX_SIDE: f64 = 16384.0;
    let gray = crate::images::source_gray(doc, img).ok()?;
    let to_mm = crate::images::pixel_to_doc(leaf, img, (gray.width, gray.height));
    let px_mm = to_mm.determinant().abs().sqrt();
    if !(px_mm.is_finite() && px_mm > 0.0 && bbox.width() > 0.0 && bbox.height() > 0.0) {
        return None;
    }
    let to_px = to_mm.inverse();
    let w = (bbox.width() / px_mm).round().clamp(1.0, MAX_SIDE) as u32;
    let h = (bbox.height() / px_mm).round().clamp(1.0, MAX_SIDE) as u32;
    let (pw, ph) = (bbox.width() / f64::from(w), bbox.height() / f64::from(h));
    let mut depth = vec![0u8; w as usize * h as usize];
    for row in 0..h {
        for col in 0..w {
            let at = to_px * darkburn_core::kurbo::Point::new(bbox.x0 + (f64::from(col) + 0.5) * pw, bbox.y1 - (f64::from(row) + 0.5) * ph);
            let (sx, sy) = (at.x.floor(), at.y.floor());
            if sx < 0.0 || sy < 0.0 || sx >= f64::from(gray.width) || sy >= f64::from(gray.height) {
                continue;
            }
            let i = sy as usize * gray.width as usize + sx as usize;
            if gray.clear.get(i).copied().unwrap_or(false) {
                continue;
            }
            let v = (f64::from(gray.data[i]).clamp(0.0, 1.0) * 255.0).round() as u8;
            depth[row as usize * w as usize + col as usize] = 255 - v;
        }
    }
    Some((w, h, depth))
}

/// The document a galvo job is made from: the whole project, or only the selected shapes when
/// "Cut selected graphics" is on (LASER-13).
pub fn job_document(app: &crate::shell::DarkBurn) -> Result<Document, String> {
    let mut doc = app.editor.doc.clone();
    if crate::job_cmds::current_settings(app).cut_selected {
        let selected = app.editor.selected();
        if selected.is_empty() {
            return Err(crate::job_build::BuildError::NothingSelected.to_string());
        }
        doc.shapes.retain(|s| selected.contains(&s.id));
    }
    Ok(doc)
}

/// One shape of a layer on its way into the galvo job (document coordinates).
struct Item {
    path: BezPath,
    priority: i32,
    /// Power scale, 0 to 1.
    scale: f64,
    /// Forced start points.
    starts: Vec<Point>,
    /// How many other shapes of the layer this one lies inside (odd: a hole).
    depth: usize,
    /// The smallest shape of the layer this one lies inside.
    parent: Option<usize>,
    /// How many cut outlines of the whole job this one lies inside (odd: the kerf shrinks it).
    kerf_depth: usize,
    /// The cut direction set on the shape.
    direction: darkburn_core::CutDirection,
    /// Manually placed tabs (document coordinates).
    tabs: Vec<Point>,
}

impl Item {
    /// The outline as it is marked: grown by the kerf (shrunk for a hole), started at its forced
    /// start points and moved to the board's coordinates.
    fn placed(&self, kerf: f64, shift: Affine) -> BezPath {
        let mut p = self.grown(kerf);
        if !self.starts.is_empty() {
            p = started_at(&p, &self.starts);
        }
        p = directed(&p, self.direction);
        p.apply_affine(shift);
        p
    }

    /// The outline grown by the kerf (shrunk for a hole), in document coordinates.
    fn grown(&self, kerf: f64) -> BezPath {
        if kerf != 0.0 {
            darkburn_geom::offset::kerf_offset(&self.path, if self.kerf_depth % 2 == 1 { -kerf } else { kerf }, crate::job_build::TOLERANCE)
        } else {
            self.path.clone()
        }
    }

    /// The outline as a Line layer marks it when the layer has tabs: what is marked at full
    /// power goes to `cut`, the stretches of partly cut tabs to `partial` with their power.
    /// Tabs sit where they do on every other machine (`darkburn_job::line`): the automatic ones
    /// spaced evenly from the drawn start, a manual one on the outline nearest to it.
    fn tabbed(
        &self,
        kerf: f64,
        shift: Affine,
        tabs: &darkburn_job::TabSettings,
        power: f64,
        cut: &mut Vec<BezPath>,
        partial: &mut Vec<(f64, BezPath)>,
    ) {
        use darkburn_job::{Op, geom, line};
        let tol = crate::job_build::TOLERANCE / 4.0;
        let drawn = geom::flatten(&self.grown(kerf), tol);
        let placed = geom::flatten(&self.placed(kerf, shift), tol);
        let ring = |poly: &geom::Poly| {
            let mut pts = poly.pts.clone();
            if poly.closed && !pts.is_empty() {
                pts.push(pts[0]);
            }
            pts
        };
        // A manual tab belongs to the sub-path nearest to it.
        let mut manual: Vec<Vec<Point>> = vec![Vec::new(); drawn.len()];
        for t in &self.tabs {
            let nearest = (0..drawn.len()).map(|i| (geom::nearest_arclen(&ring(&drawn[i]), *t).1, i)).min_by(|a, b| a.0.total_cmp(&b.0));
            if let Some((_, i)) = nearest {
                manual[i].push(*t);
            }
        }
        let mut settings = darkburn_job::CutSettings::line(1.0, power);
        settings.line.tabs = *tabs;
        for (i, poly) in placed.iter().enumerate() {
            let centres: Vec<Point> = match drawn.get(i) {
                Some(d) if drawn.len() == placed.len() => {
                    line::tab_centres(&d.pts, d.closed, self.depth, tabs, (!self.tabs.is_empty()).then_some(manual[i].as_slice()))
                        .into_iter()
                        .map(|c| shift * c)
                        .collect()
                }
                _ => Vec::new(),
            };
            let pts = ring(poly);
            if centres.is_empty() || pts.len() < 2 {
                cut.push(polyline(&poly.pts, poly.closed));
                continue;
            }
            let ctx = line::PathContext { settings: &settings, power_scale: 1.0, depth: self.depth, tabs: &centres, backlash: None };
            let mut ops = Vec::new();
            line::emit_path(&pts, poly.closed, &ctx, &mut ops);
            // Runs of cuts at one power become one polyline each; laser-off stretches are left out.
            let mut pos = pts[0];
            let mut run: Option<(f64, Vec<Point>)> = None;
            let mut flush = |run: &mut Option<(f64, Vec<Point>)>| {
                if let Some((pw, pts)) = run.take()
                    && pw > 0.0
                    && pts.len() > 1
                {
                    if pw >= power - 1e-9 { cut.push(polyline(&pts, false)) } else { partial.push((pw, polyline(&pts, false))) }
                }
            };
            for op in &ops {
                match op {
                    Op::Cut { to, power: pw, .. } => {
                        if run.as_ref().is_none_or(|(p, _)| (p - pw).abs() > 1e-9) {
                            flush(&mut run);
                            run = Some((*pw, vec![pos]));
                        }
                        if let Some((_, r)) = run.as_mut() {
                            r.push(*to);
                        }
                        pos = *to;
                    }
                    other => {
                        flush(&mut run);
                        if let Some(t) = other.target() {
                            pos = t;
                        }
                    }
                }
            }
            flush(&mut run);
        }
    }
}

fn polyline(pts: &[Point], closed: bool) -> BezPath {
    let mut out = BezPath::new();
    let mut it = pts.iter();
    if let Some(p) = it.next() {
        out.move_to(*p);
    }
    for p in it {
        out.line_to(*p);
    }
    if closed {
        out.close_path();
    }
    out
}

/// `path` with its outlines turned the way the shape's cut direction asks: clockwise and
/// counter-clockwise turn closed outlines (they keep their start), reversed turns every outline
/// round. Automatic leaves the path as it is. Turned outlines become polylines.
fn directed(path: &BezPath, direction: darkburn_core::CutDirection) -> BezPath {
    use darkburn_core::CutDirection as D;
    if direction == D::Auto {
        return path.clone();
    }
    let mut out = BezPath::new();
    for mut poly in darkburn_job::geom::flatten(path, crate::job_build::TOLERANCE / 4.0) {
        let counter_clockwise = darkburn_job::geom::signed_area(&poly.pts) > 0.0;
        let turn = match direction {
            D::Auto => false,
            D::Reversed => true,
            D::Clockwise => poly.closed && counter_clockwise,
            D::CounterClockwise => poly.closed && !counter_clockwise,
        };
        if turn && poly.closed && poly.pts.len() > 1 {
            poly.pts[1..].reverse();
        } else if turn {
            poly.pts.reverse();
        }
        out.extend(polyline(&poly.pts, poly.closed));
    }
    out
}

/// Work out which shapes lie inside which (the same test the kerf offset of the other machines
/// uses: the bounding box and the first point).
fn nest(items: &mut [Item]) {
    let tol = crate::job_build::TOLERANCE;
    let closed: Vec<Vec<(Rect, Vec<Point>)>> = items
        .iter()
        .map(|it| {
            darkburn_job::geom::flatten(&it.path, tol).into_iter().filter(|p| p.closed).map(|p| (darkburn_job::geom::bbox(&p.pts), p.pts)).collect()
        })
        .collect();
    for (i, item) in items.iter_mut().enumerate() {
        let bbox = item.path.bounding_box();
        let Some(darkburn_core::kurbo::PathEl::MoveTo(p0)) = item.path.elements().first().copied() else { continue };
        let mut around: Vec<(f64, usize)> = Vec::new();
        for (j, polys) in closed.iter().enumerate() {
            if j == i {
                continue;
            }
            let holds = polys.iter().filter(|(r, pts)| {
                r.area() > bbox.area() + 1e-9 && darkburn_job::geom::rect_inside(bbox, *r) && darkburn_job::geom::point_in_polygon(p0, pts)
            });
            if let Some(smallest) = holds.map(|(r, _)| r.area()).min_by(f64::total_cmp) {
                around.push((smallest, j));
            }
        }
        item.depth = around.len();
        item.parent = around.iter().min_by(|a, b| a.0.total_cmp(&b.0)).map(|a| a.1);
    }
}

/// `path` with each closed outline that a forced start point sits on started at that point.
/// Each point goes to the outline nearest to it. Outlines with a start point become polylines
/// (the board is sent polylines in any case); the others are kept as they are.
fn started_at(path: &BezPath, starts: &[Point]) -> BezPath {
    let mut polys = darkburn_job::geom::flatten(path, crate::job_build::TOLERANCE / 4.0);
    let mut moved = false;
    for &q in starts {
        // (distance, outline, segment, point on it)
        let mut best: Option<(f64, usize, usize, Point)> = None;
        for (pi, poly) in polys.iter().enumerate().filter(|(_, p)| p.closed) {
            let n = poly.pts.len();
            for i in 0..n {
                let (a, b) = (poly.pts[i], poly.pts[(i + 1) % n]);
                let ab = b - a;
                let t = if ab.hypot2() > 0.0 { ((q - a).dot(ab) / ab.hypot2()).clamp(0.0, 1.0) } else { 0.0 };
                let on = a + ab * t;
                let d = on.distance(q);
                if best.is_none_or(|(bd, ..)| d < bd) {
                    best = Some((d, pi, i, on));
                }
            }
        }
        let Some((_, pi, i, on)) = best else { continue };
        let pts = &polys[pi].pts;
        let n = pts.len();
        let mut out = vec![on];
        out.extend((1..=n).map(|k| pts[(i + k) % n]));
        out.dedup_by(|a, b| a.distance(*b) < 1e-9);
        if out.len() > 1 && out[0].distance(out[out.len() - 1]) < 1e-9 {
            out.pop();
        }
        if out.len() >= 3 {
            polys[pi].pts = out;
            moved = true;
        }
    }
    if !moved {
        return path.clone();
    }
    let mut out = BezPath::new();
    for poly in &polys {
        let mut pts = poly.pts.iter();
        if let Some(p) = pts.next() {
            out.move_to(*p);
        }
        for p in pts {
            out.line_to(*p);
        }
        if poly.closed {
            out.close_path();
        }
    }
    out
}

/// Build the galvo job for the document and device.
pub fn build_job(doc: &Document, dev: &Device) -> Result<GalvoBuild, String> {
    let gs = &dev.settings.galvo;
    let bed = crate::galvo_machine::bed_of(gs);
    let shift = Affine::translate((-bed.0 / 2.0, -bed.1 / 2.0));
    let mut job = galvo::Job::new();
    let mut warnings = Vec::new();
    job.repeat = galvo::RepeatMarking {
        count: gs.repeat.count.max(1),
        axis_step: gs.repeat.axis_on.then_some((gs.repeat.axis.min(1), gs.repeat.axis_step_mm)),
        wait_foot_each: gs.repeat.wait_foot_each,
    };
    job.rotary = gs.rotary.on.then(|| galvo::RotaryConfig {
        mode: if gs.rotary.by_shape { galvo::RotaryMode::ByShape } else { galvo::RotaryMode::BySlice },
        max_size_mm: gs.rotary.max_size_mm.clamp(1.0, gs.field_size_mm),
        axis: gs.rotary.axis.min(1),
    });
    job.door_protect = gs.door_protect;
    job.optimize = gs.optimize;

    // Cut order priority counts when the project's optimization settings order by it, as in the
    // job of every other machine.
    let planner = crate::job_build::JobProjectSettings::from_doc(doc, &darkburn_job::PlannerSettings::default()).planner;
    let by_priority = planner.optimize && planner.order_by.contains(&darkburn_job::OrderKey::Priority);
    // A kerf offset grows a part and shrinks a hole. What is a hole is counted over the outlines
    // that are cut on every layer of the job, as in the job of the other machines
    // (`job_build::collect`): holes are often given a layer of their own. Each outline comes
    // with the number of its shape; they are only needed when some layer has a kerf offset.
    let marked = |li: usize| doc.layer(li).outputs() && !layer::is_tool_layer(li);
    let cut_as_line = |s: &CutSetting| s.enabled && (s.mode == Mode::FillLine || (s.mode == Mode::Line && !s.galvo.drill));
    let any_kerf = (0..layer::LAYER_COUNT).any(|li| marked(li) && doc.layer(li).settings.iter().any(|s| cut_as_line(s) && s.kerf != 0.0));
    let mut cut_outlines: Vec<(usize, Rect, Vec<Point>)> = Vec::new();
    let mut leaf_no = 0usize;
    for top in doc.iter() {
        for leaf in top.leaves() {
            leaf_no += 1;
            let li = leaf.layer.min(layer::LAYER_COUNT - 1);
            if !any_kerf || !marked(li) || leaf.is_image() || !doc.layer(li).settings.iter().any(cut_as_line) {
                continue;
            }
            for poly in darkburn_job::geom::flatten(&leaf.outline(), crate::job_build::TOLERANCE) {
                if poly.closed {
                    cut_outlines.push((leaf_no, darkburn_job::geom::bbox(&poly.pts), poly.pts));
                }
            }
        }
    }
    let order: Vec<usize> = doc.layer_order.clone();
    for li in order {
        let l = doc.layer(li);
        if !l.outputs() || layer::is_tool_layer(li) {
            continue;
        }
        let mut items: Vec<Item> = Vec::new();
        let mut images = Vec::new();
        let mut leaf_no = 0usize;
        for top in doc.iter() {
            for leaf in top.leaves() {
                leaf_no += 1;
                if leaf.layer.min(layer::LAYER_COUNT - 1) != li {
                    continue;
                }
                if let ShapeKind::Image(img) = &leaf.kind {
                    images.push((leaf.bbox(), leaf.clone(), img.clone()));
                    continue;
                }
                if matches!(&leaf.kind, ShapeKind::Text(t) if t.backup.is_none()) {
                    warnings.push("A text shape has no outline yet and was left out.".to_owned());
                    continue;
                }
                let p = leaf.outline();
                if p.elements().is_empty() {
                    continue;
                }
                let bbox = p.bounding_box();
                let kerf_depth = match p.elements().first() {
                    Some(darkburn_core::kurbo::PathEl::MoveTo(p0)) => cut_outlines
                        .iter()
                        .filter(|(no, r, pts)| {
                            *no != leaf_no
                                && r.area() > bbox.area() + 1e-9
                                && darkburn_job::geom::rect_inside(bbox, *r)
                                && darkburn_job::geom::point_in_polygon(*p0, pts)
                        })
                        .count(),
                    _ => 0,
                };
                items.push(Item {
                    path: p,
                    priority: if by_priority { leaf.priority } else { 0 },
                    scale: (leaf.power_scale / 100.0).clamp(0.0, 1.0),
                    starts: leaf.start_points().map(|q| leaf.transform * Point::new(q[0], q[1])).collect(),
                    depth: 0,
                    parent: None,
                    kerf_depth,
                    direction: leaf.direction,
                    tabs: leaf.tabs.iter().map(|q| leaf.transform * Point::new(q[0], q[1])).collect(),
                });
            }
        }
        if items.is_empty() && images.is_empty() {
            continue;
        }
        nest(&mut items);
        // Shapes that differ in power scale or cut order priority are marked as layers of
        // their own, the lower priority number first; shapes that agree stay together.
        let mut groups: Vec<(i32, f64)> = Vec::new();
        for it in &items {
            if !groups.contains(&(it.priority, it.scale)) {
                groups.push((it.priority, it.scale));
            }
        }
        groups.sort_by_key(|g| g.0);
        let outlines = |group: (i32, f64), kerf: f64| -> Vec<BezPath> {
            items.iter().filter(|it| (it.priority, it.scale) == group).map(|it| it.placed(kerf, shift)).collect()
        };
        // What a group fills when all shapes are filled as one: its own shapes that are filled
        // (not holes), each with the shapes directly inside it as its holes, whatever group
        // those are in.
        let fill_of = |group: (i32, f64)| -> Vec<BezPath> {
            if groups.len() == 1 {
                return outlines(group, 0.0);
            }
            let owner = |i: usize| (items[i].priority, items[i].scale) == group && items[i].depth.is_multiple_of(2);
            (0..items.len()).filter(|&i| owner(i) || items[i].parent.is_some_and(owner)).map(|i| items[i].placed(0.0, shift)).collect()
        };
        let base = if l.name.is_empty() { layer::layer_label(li) } else { l.name.clone() };
        for (si, s) in l.settings.iter().enumerate().filter(|(_, s)| s.enabled) {
            let name = if si == 0 { base.clone() } else { format!("{base} / {}", if s.name.is_empty() { si.to_string() } else { s.name.clone() }) };
            let layer_params = mark_params(s, gs);
            let passes = s.passes.max(1) * s.galvo.global_passes.max(1);
            if s.galvo.pause_before_ms != 0.0 || s.galvo.pause_after_ms != 0.0 {
                warnings.push(format!("Layer {name}: pause before and after are not used by the galvo board."));
            }
            if s.mode == Mode::OffsetFill {
                warnings.push(format!("Layer {name}: offset fill is marked as a normal fill on a galvo."));
            }
            let params = layer_params.clone();
            for &group in &groups {
                if s.mode == Mode::Image {
                    break;
                }
                let mut params = layer_params.clone();
                params.power_pct = (params.power_pct * group.1).clamp(0.0, 100.0);
                let mut push_at = |mode: LayerMode, tag: &str, geometry: Vec<BezPath>, power: f64| {
                    if geometry.is_empty() {
                        return;
                    }
                    let mut params = params.clone();
                    params.power_pct = power;
                    let mut gl = galvo::Layer::new(&format!("{name}{tag}"), mode, params, geometry);
                    gl.passes = passes;
                    gl.wobble = gs.wobble_for(&s.galvo);
                    job.layers.push(gl);
                };
                let mut push = |mode: LayerMode, tag: &str, geometry: Vec<BezPath>| push_at(mode, tag, geometry, params.power_pct);
                // The outlines of a Line layer: whole, or with the layer's tabs left out. Partly
                // cut tabs are marked as layers of their own at the tab power.
                let line = |push_at: &mut dyn FnMut(LayerMode, &str, Vec<BezPath>, f64), tag: &str| {
                    let tabs = crate::job_build::tab_settings(s);
                    if !tabs.enabled || tabs.size <= 0.0 {
                        return push_at(LayerMode::Line, tag, outlines(group, s.kerf), params.power_pct);
                    }
                    let (mut cut, mut partial) = (Vec::new(), Vec::new());
                    for it in items.iter().filter(|it| (it.priority, it.scale) == group) {
                        it.tabbed(s.kerf, shift, &tabs, params.power_pct, &mut cut, &mut partial);
                    }
                    push_at(LayerMode::Line, tag, cut, params.power_pct);
                    let mut powers: Vec<f64> = partial.iter().map(|(p, _)| *p).collect();
                    powers.sort_by(f64::total_cmp);
                    powers.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
                    for pw in powers {
                        let pieces = partial.iter().filter(|(p, _)| (*p - pw).abs() < 1e-9).map(|(_, b)| b.clone()).collect();
                        push_at(LayerMode::Line, &format!("{tag} tabs"), pieces, pw);
                    }
                };
                let fill = |s: &CutSetting| {
                    let f = fill_settings(s);
                    let geometry = if matches!(f.grouping, FillGrouping::Individually) { outlines(group, 0.0) } else { fill_of(group) };
                    (if s.flood_fill { LayerMode::Flood(f) } else { LayerMode::Fill(f) }, geometry)
                };
                match s.mode {
                    Mode::Line if s.galvo.drill => push(
                        LayerMode::Drill(galvo::DrillSettings {
                            dwell_ms: s.galvo.drill_dwell_ms.clamp(0.01, 60_000.0),
                            spacing_mm: s.galvo.drill_spacing_mm.max(0.0),
                        }),
                        "",
                        outlines(group, 0.0),
                    ),
                    Mode::Line => line(&mut push_at, ""),
                    Mode::Fill | Mode::OffsetFill => {
                        let (mode, geometry) = fill(s);
                        push(mode, "", geometry);
                    }
                    Mode::FillLine => {
                        let (mode, geometry) = fill(s);
                        push(mode, " fill", geometry);
                        line(&mut push_at, " line");
                    }
                    Mode::Image => {}
                }
            }
            match s.mode {
                Mode::Line | Mode::Fill | Mode::OffsetFill | Mode::FillLine => {}
                Mode::Image => {
                    for (bbox, leaf, img) in &images {
                        let Some((w, h, depth)) = depth_of(doc, leaf, img, *bbox) else {
                            warnings.push(format!("Layer {name}: a picture could not be read."));
                            continue;
                        };
                        let r = Rect::new(bbox.x0 - bed.0 / 2.0, bbox.y0 - bed.1 / 2.0, bbox.x1 - bed.0 / 2.0, bbox.y1 - bed.1 / 2.0);
                        let sliced = galvo::SlicedImage {
                            width_px: w,
                            height_px: h,
                            depth,
                            origin: darkburn_core::kurbo::Point::new(r.x0, r.y0),
                            size_mm: (r.width(), r.height()),
                            slices: s.passes.max(1),
                            z_step_mm: s.z_step,
                            interval_mm: s.interval.max(0.005),
                            bidirectional: s.bidirectional,
                            cleanup: None,
                        };
                        let mut gl = galvo::Layer::new(&format!("{name} 3D"), LayerMode::Sliced(sliced), params.clone(), Vec::new());
                        gl.passes = s.galvo.global_passes.max(1);
                        job.layers.push(gl);
                    }
                }
            }
        }
        if !images.is_empty() && !l.settings.iter().any(|s| s.enabled && s.mode == Mode::Image) {
            warnings.push(format!("Layer {} holds pictures but is not in Image mode; they were left out.", layer::layer_label(li)));
        }
        if !items.is_empty() && l.settings.iter().any(|s| s.enabled) && l.settings.iter().filter(|s| s.enabled).all(|s| s.mode == Mode::Image) {
            warnings.push(format!(
                "Layer {} is in Image mode but holds shapes that are not pictures; those shapes were left out.",
                layer::layer_label(li)
            ));
        }
    }
    if job.layers.is_empty() {
        return Err("There is nothing to mark: no shapes on layers that output.".into());
    }
    Ok(GalvoBuild { job, warnings })
}

/// Compile with the device's galvo settings.
pub fn compile(job: &galvo::Job, dev: &Device) -> Result<galvo::CompiledJob, String> {
    galvo::compile(job, &dev.settings.galvo.to_library(), &galvo::CompileOptions::default()).map_err(|e| e.to_string())
}

/// What the estimate says, for the panel and `galvo.estimate`.
pub fn estimate_json(c: &galvo::CompiledJob) -> serde_json::Value {
    serde_json::json!({
        "total_s": c.estimate.total_s(), "mark_s": c.estimate.mark_s, "jump_s": c.estimate.jump_s,
        "delay_s": c.estimate.delay_s, "motion_s": c.estimate.motion_s,
        "packets": c.packet_count(), "steps": c.steps.len(),
        "bounds": [c.bounds_mm.x0, c.bounds_mm.y0, c.bounds_mm.x1, c.bounds_mm.y1],
    })
}

/// The job request for the machine. The laser must be armed by the user before it starts.
pub fn request(doc: &Document, dev: &Device, wait_for_foot: bool, skip_framing_check: bool) -> Result<(JobRequest, Vec<String>), String> {
    let b = build_job(doc, dev)?;
    let c = compile(&b.job, dev)?;
    let mut req = JobRequest::new(JobData::Galvo { job: Box::new(c.clone()), wait_for_foot, skip_framing_check });
    req.estimated = Some(std::time::Duration::from_secs_f64(c.estimate.total_s().max(0.0)));
    Ok((req, b.warnings))
}

/// Frame adjustments from the red-light dialog [GALVO-09].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameAdjust {
    pub nudge_mm: (f64, f64),
    pub scale_pct: f64,
    /// Trace the convex hull instead of the bounding rectangle.
    pub hull: bool,
    pub repeat: bool,
}

impl Default for FrameAdjust {
    fn default() -> Self {
        FrameAdjust { nudge_mm: (0.0, 0.0), scale_pct: 100.0, hull: false, repeat: true }
    }
}

/// The red-light frame of the job: the laser is off, the pointer on, and the frame ties to the
/// job so "require framing" is satisfied once a lap has completed.
pub fn frame_for(job: &galvo::Job, dev: &Device, adj: &FrameAdjust) -> Result<galvo::CompiledFrame, String> {
    let tol = 0.01;
    let mut spec = if adj.hull { galvo::FrameSpec::from_job_hull(job, tol) } else { galvo::FrameSpec::from_job_bounds(job, tol) }
        .ok_or_else(|| "There is nothing to frame.".to_string())?;
    spec.nudge_mm = adj.nudge_mm;
    spec.scale_pct = adj.scale_pct;
    galvo::compile_frame(&spec, &dev.settings.galvo.to_library()).map_err(|e| e.to_string())
}

/// The lens correction test pattern as paths in the field (centred on the job origin) [LT-18].
pub fn lens_test_paths(size_mm: f64, divisions: u32, cross_mm: f64, grid_lines: bool) -> Vec<BezPath> {
    galvo::LensTestPattern { size_mm, divisions, cross_mm, grid_lines }.paths()
}
