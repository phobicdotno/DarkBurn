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

/// The depth image of a picture: dark is deep, row 0 at the top.
fn depth_of(doc: &Document, img: &darkburn_core::ImageShape) -> Option<(u32, u32, Vec<u8>)> {
    let data = doc.images.get(&img.image)?;
    let decoded = image::load_from_memory(&data.bytes).ok()?.to_luma8();
    let (w, h) = decoded.dimensions();
    Some((w, h, decoded.into_raw().into_iter().map(|v| 255 - v).collect()))
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
}

impl Item {
    /// The outline as it is marked: grown by the kerf (shrunk for a hole), started at its forced
    /// start points and moved to the board's coordinates.
    fn placed(&self, kerf: f64, shift: Affine) -> BezPath {
        let mut p = if kerf != 0.0 {
            darkburn_geom::offset::kerf_offset(&self.path, if self.depth % 2 == 1 { -kerf } else { kerf }, crate::job_build::TOLERANCE)
        } else {
            self.path.clone()
        };
        if !self.starts.is_empty() {
            p = started_at(&p, &self.starts);
        }
        p.apply_affine(shift);
        p
    }
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
    let order: Vec<usize> = doc.layer_order.clone();
    for li in order {
        let l = doc.layer(li);
        if !l.outputs() || layer::is_tool_layer(li) {
            continue;
        }
        let mut items: Vec<Item> = Vec::new();
        let mut images = Vec::new();
        for top in doc.iter() {
            for leaf in top.leaves() {
                if leaf.layer.min(layer::LAYER_COUNT - 1) != li {
                    continue;
                }
                if let ShapeKind::Image(img) = &leaf.kind {
                    images.push((leaf.bbox(), img.clone()));
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
                items.push(Item {
                    path: p,
                    priority: if by_priority { leaf.priority } else { 0 },
                    scale: (leaf.power_scale / 100.0).clamp(0.0, 1.0),
                    starts: leaf.start_points().map(|q| leaf.transform * Point::new(q[0], q[1])).collect(),
                    depth: 0,
                    parent: None,
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
                let mut push = |mode: LayerMode, tag: &str, geometry: Vec<BezPath>| {
                    if geometry.is_empty() {
                        return;
                    }
                    let mut gl = galvo::Layer::new(&format!("{name}{tag}"), mode, params.clone(), geometry);
                    gl.passes = passes;
                    gl.wobble = gs.wobble_for(&s.galvo);
                    job.layers.push(gl);
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
                    Mode::Line => push(LayerMode::Line, "", outlines(group, s.kerf)),
                    Mode::Fill | Mode::OffsetFill => {
                        let (mode, geometry) = fill(s);
                        push(mode, "", geometry);
                    }
                    Mode::FillLine => {
                        let (mode, geometry) = fill(s);
                        push(mode, " fill", geometry);
                        push(LayerMode::Line, " line", outlines(group, s.kerf));
                    }
                    Mode::Image => {}
                }
            }
            match s.mode {
                Mode::Line | Mode::Fill | Mode::OffsetFill | Mode::FillLine => {}
                Mode::Image => {
                    for (bbox, img) in &images {
                        let Some((w, h, depth)) = depth_of(doc, img) else {
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
