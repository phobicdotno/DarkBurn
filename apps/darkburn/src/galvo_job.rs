//! A galvo job from the document [GALVO-06..08, GALVO-13, GALVO-14, GALVO-16, LT-17, LT-18]:
//! the layers map to `darkburn_galvo::Layer`s (line, fill, flood fill, 3D sliced image) with the
//! per-layer galvo fields of the cut settings (frequency, Q-pulse, wobble, timings, global
//! passes), and the job is compiled with the device's galvo settings.
//!
//! Coordinates: the document is the square field with its lower-left corner at the bed origin;
//! the board is centred, so every shape moves by minus half the field.

use darkburn_core::kurbo::{Affine, BezPath, Rect};
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

    let order: Vec<usize> = doc.layer_order.clone();
    for li in order {
        let l = doc.layer(li);
        if !l.outputs() || layer::is_tool_layer(li) {
            continue;
        }
        let mut paths: Vec<BezPath> = Vec::new();
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
                let mut p = leaf.outline();
                if p.elements().is_empty() {
                    continue;
                }
                p.apply_affine(shift);
                paths.push(p);
            }
        }
        if paths.is_empty() && images.is_empty() {
            continue;
        }
        let base = if l.name.is_empty() { layer::layer_label(li) } else { l.name.clone() };
        for (si, s) in l.settings.iter().enumerate().filter(|(_, s)| s.enabled) {
            let name = if si == 0 { base.clone() } else { format!("{base} / {}", if s.name.is_empty() { si.to_string() } else { s.name.clone() }) };
            let params = mark_params(s, gs);
            let passes = s.passes.max(1) * s.galvo.global_passes.max(1);
            if s.galvo.pause_before_ms != 0.0 || s.galvo.pause_after_ms != 0.0 {
                warnings.push(format!("Layer {name}: pause before and after are not used by the galvo board."));
            }
            let mut push = |mode: LayerMode, tag: &str, geometry: Vec<BezPath>| {
                let mut gl = galvo::Layer::new(&format!("{name}{tag}"), mode, params.clone(), geometry);
                gl.passes = passes;
                gl.wobble = gs.wobble_for(&s.galvo);
                job.layers.push(gl);
            };
            match s.mode {
                Mode::Line if s.galvo.drill => push(
                    LayerMode::Drill(galvo::DrillSettings {
                        dwell_ms: s.galvo.drill_dwell_ms.clamp(0.01, 60_000.0),
                        spacing_mm: s.galvo.drill_spacing_mm.max(0.0),
                    }),
                    "",
                    paths.clone(),
                ),
                Mode::Line => push(LayerMode::Line, "", paths.clone()),
                Mode::Fill | Mode::OffsetFill => {
                    if s.mode == Mode::OffsetFill {
                        warnings.push(format!("Layer {name}: offset fill is marked as a normal fill on a galvo."));
                    }
                    let f = fill_settings(s);
                    push(if s.flood_fill { LayerMode::Flood(f) } else { LayerMode::Fill(f) }, "", paths.clone());
                }
                Mode::FillLine => {
                    let f = fill_settings(s);
                    push(if s.flood_fill { LayerMode::Flood(f) } else { LayerMode::Fill(f) }, " fill", paths.clone());
                    push(LayerMode::Line, " line", paths.clone());
                }
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
