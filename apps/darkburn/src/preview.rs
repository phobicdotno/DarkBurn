//! Preview data [WIN-02, PREV-01 to PREV-10]: the simulated job as drawable segments, the colours
//! of power shading, and the preview image export. The window that shows it is in `job_ui.rs`.

use std::sync::Arc;

use darkburn_core::kurbo::{Point, Rect, Vec2};
use darkburn_job::sim::{SegmentPath, Simulation};

use crate::job_build::{AdditionalSettings, BuiltJob, JobStats};

/// One straight piece of the simulated path.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DrawSeg {
    pub a: Point,
    pub b: Point,
    /// Percent; 0 for travel or a laser-off move.
    pub power: f32,
    /// A rapid (travel) move.
    pub rapid: bool,
    /// Time the piece starts and how long it takes, seconds.
    pub t0: f64,
    pub dur: f64,
    /// The operation it came from.
    pub op: usize,
}

/// A built job with its simulation, ready to draw.
pub struct PreviewData {
    pub job: Arc<BuiltJob>,
    pub sim: Simulation,
    pub stats: JobStats,
    pub segs: Vec<DrawSeg>,
    /// Revision of the document it was built from.
    pub revision: u64,
}

impl PreviewData {
    pub fn new(job: Arc<BuiltJob>, motion: &AdditionalSettings, revision: u64) -> PreviewData {
        let sim = job.simulate(motion);
        let stats = JobStats::of(&job, &sim);
        let segs = draw_segments(&sim);
        PreviewData { job, sim, stats, segs, revision }
    }

    /// Number of pieces that have started by time `t`.
    pub fn started(&self, t: f64) -> usize {
        self.segs.partition_point(|s| s.t0 <= t)
    }

    /// "Layer C00 Cut (Line), pass 2 of 3" for time `t`.
    pub fn readout(&self, t: f64) -> String {
        let Some(st) = self.sim.state_at(t) else { return "No motion".into() };
        match st.layer.and_then(|l| self.job.toolpath.layers.get(l)) {
            Some(l) => {
                let name = if l.name.is_empty() { format!("C{:02}", l.layer_index) } else { l.name.clone() };
                let pass = if st.pass > 0 { format!(", pass {} of {}", st.pass, l.passes) } else { String::new() };
                format!("Layer {name} ({:?}{}){pass}", l.mode, if l.sublayer > 0 { format!(", sub-layer {}", l.sublayer) } else { String::new() })
            }
            None => "Between layers".into(),
        }
    }
}

/// The simulated path as straight pieces; arcs are split so they look round.
pub fn draw_segments(sim: &Simulation) -> Vec<DrawSeg> {
    let mut out = Vec::with_capacity(sim.segments.len());
    for s in &sim.segments {
        let power = s.power as f32;
        match s.path {
            SegmentPath::Line { from, to } => out.push(DrawSeg { a: from, b: to, power, rapid: s.rapid, t0: s.t0, dur: s.duration, op: s.op_index }),
            SegmentPath::Arc { from, center, sweep } => {
                let r = from.distance(center);
                let n = ((r * sweep.abs()) / 0.5).ceil().clamp(2.0, 96.0) as usize;
                let mut prev = from;
                for k in 1..=n {
                    let p = center + darkburn_job::geom::rotate(from - center, sweep * k as f64 / n as f64);
                    out.push(DrawSeg {
                        a: prev,
                        b: p,
                        power,
                        rapid: s.rapid,
                        t0: s.t0 + s.duration * (k - 1) as f64 / n as f64,
                        dur: s.duration / n as f64,
                        op: s.op_index,
                    });
                    prev = p;
                }
            }
            SegmentPath::Still { at } => {
                if power > 0.0 {
                    out.push(DrawSeg { a: at, b: at + Vec2::new(0.05, 0.0), power, rapid: false, t0: s.t0, dur: s.duration, op: s.op_index });
                }
            }
        }
    }
    out
}

/// How the preview is drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewOptions {
    /// Burnt lines darker with more power (PREV-02).
    pub shade_by_power: bool,
    /// Draw the laser-off travel moves (PREV-03).
    pub show_travel: bool,
    /// White on black instead of black on white (PREV-04).
    pub invert: bool,
}

impl Default for ViewOptions {
    fn default() -> Self {
        ViewOptions { shade_by_power: true, show_travel: false, invert: false }
    }
}

impl ViewOptions {
    pub fn background(&self) -> [u8; 3] {
        if self.invert { [18, 18, 20] } else { [250, 250, 250] }
    }

    /// The colour of a piece with the laser at `power` percent.
    pub fn burn_colour(&self, power: f32) -> [u8; 3] {
        let level = if self.shade_by_power { (0.3 + 0.7 * (power / 100.0).clamp(0.0, 1.0)).min(1.0) } else { 1.0 };
        if self.invert {
            let v = (255.0 * level) as u8;
            [v, v, v]
        } else {
            let v = (255.0 * (1.0 - level)) as u8;
            [v, v, v]
        }
    }

    pub fn travel_colour(&self) -> [u8; 3] {
        if self.invert { [70, 110, 160] } else { [90, 140, 200] }
    }

    /// The piece's colour, or None when it is not drawn.
    pub fn colour_of(&self, s: &DrawSeg) -> Option<[u8; 3]> {
        if s.power > 0.0 {
            Some(self.burn_colour(s.power))
        } else if self.show_travel {
            Some(self.travel_colour())
        } else {
            None
        }
    }
}

/// The head position at time `t`.
pub fn head_at(data: &PreviewData, t: f64) -> Option<Point> {
    data.sim.state_at(t).map(|s| s.pos)
}

/// The preview as PNG bytes: the bed with everything burnt up to time `upto` (the whole job when
/// None), `width` pixels wide. With `transparent` the background has no colour (PREV-08).
pub fn render_png(data: &PreviewData, bed: Rect, width: u32, view: &ViewOptions, transparent: bool, upto: Option<f64>) -> Result<Vec<u8>, String> {
    let width = width.clamp(64, 8192);
    let scale = f64::from(width) / bed.width().max(1.0);
    let height = ((bed.height() * scale).round() as u32).clamp(1, 8192);
    let mut pm = tiny_skia::Pixmap::new(width, height).ok_or("the image is too big")?;
    if !transparent {
        let [r, g, b] = view.background();
        pm.fill(tiny_skia::Color::from_rgba8(r, g, b, 255));
    }
    let to_px = |p: Point| ((p.x - bed.x0) * scale, (bed.y1 - p.y) * scale);
    let n = upto.map_or(data.segs.len(), |t| data.started(t));
    let stroke = tiny_skia::Stroke { width: (0.15 * scale).clamp(1.0, 4.0) as f32, line_cap: tiny_skia::LineCap::Round, ..Default::default() };
    // One path per colour keeps this fast.
    let mut groups: std::collections::BTreeMap<[u8; 3], tiny_skia::PathBuilder> = Default::default();
    for s in &data.segs[..n] {
        let Some(c) = view.colour_of(s) else { continue };
        let pb = groups.entry(c).or_default();
        let (ax, ay) = to_px(s.a);
        let (bx, by) = to_px(s.b);
        pb.move_to(ax as f32, ay as f32);
        pb.line_to(bx as f32, by as f32);
    }
    // Travel first so burns end up on top.
    let travel = view.travel_colour();
    let mut keys: Vec<[u8; 3]> = groups.keys().copied().collect();
    keys.sort_by_key(|k| *k != travel);
    for k in keys {
        let Some(pb) = groups.remove(&k) else { continue };
        let Some(path) = pb.finish() else { continue };
        let mut paint = tiny_skia::Paint::default();
        paint.set_color_rgba8(k[0], k[1], k[2], 255);
        paint.anti_alias = true;
        pm.stroke_path(&path, &paint, &stroke, tiny_skia::Transform::identity(), None);
    }
    let mut img = image::RgbaImage::new(width, height);
    for (px, p) in img.pixels_mut().zip(pm.pixels()) {
        let c = p.demultiply();
        *px = image::Rgba([c.red(), c.green(), c.blue(), c.alpha()]);
    }
    let mut out = Vec::new();
    image::DynamicImage::ImageRgba8(img).write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).map_err(|e| e.to_string())?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::job_build::{BuildInput, JobDevice, build_job_simple};
    use darkburn_core::kurbo::Rect;
    use darkburn_core::{Document, Shape};

    fn data() -> PreviewData {
        let mut d = Document::new();
        {
            let s = d.layer_mut(0).main_mut();
            s.speed = 20.0;
            s.max_power = 60.0;
            s.passes = 2;
        }
        d.add(Shape::rect(Rect::new(10.0, 10.0, 30.0, 30.0), 0.0, 0));
        d.add(Shape::ellipse(Rect::new(50.0, 10.0, 70.0, 30.0), 0));
        let input = BuildInput {
            doc: d,
            selection: vec![],
            settings: Default::default(),
            device: JobDevice::new(100.0, 100.0),
            user_origin: None,
            head: None,
            motion: Default::default(),
        };
        let job = Arc::new(build_job_simple(&input).unwrap());
        PreviewData::new(job, &AdditionalSettings::default(), 0)
    }

    #[test]
    fn time_estimate_distances_and_readout() {
        let p = data();
        // Two passes over 80 mm of square and about 63 mm of circle at 20 mm/s: over 14 s of cutting.
        assert!(p.stats.seconds > 14.0, "{:?}", p.stats);
        assert!((p.stats.cut_mm - 2.0 * (80.0 + 62.83)).abs() < 1.0, "{:?}", p.stats);
        assert!(p.stats.travel_mm > 0.0);
        assert!(p.readout(1.0).contains("pass 1 of 2"), "{}", p.readout(1.0));
        assert!(p.readout(p.stats.seconds - 0.5).contains("pass 2 of 2"), "{}", p.readout(p.stats.seconds - 0.5));
        assert!(p.started(0.0) >= 1);
        assert!(p.started(p.stats.seconds) == p.segs.len());
        let h = head_at(&p, p.stats.seconds / 2.0).unwrap();
        assert!(h.x >= 0.0 && h.x <= 100.0);
    }

    #[test]
    fn a_faster_machine_is_estimated_faster() {
        let p = data();
        let fast = AdditionalSettings { accel: 50_000.0, engrave_accel: 50_000.0, ..Default::default() };
        let slow = AdditionalSettings { accel: 50.0, engrave_accel: 50.0, ..Default::default() };
        let (tf, ts) = (p.job.simulate(&fast).total_time, p.job.simulate(&slow).total_time);
        assert!(ts > tf, "{ts} vs {tf}");
    }

    #[test]
    fn shading_travel_and_invert() {
        let v = ViewOptions::default();
        assert!(v.burn_colour(100.0)[0] < v.burn_colour(10.0)[0], "more power is darker");
        let flat = ViewOptions { shade_by_power: false, ..v };
        assert_eq!(flat.burn_colour(5.0), flat.burn_colour(90.0));
        let inv = ViewOptions { invert: true, ..v };
        assert!(inv.burn_colour(100.0)[0] > inv.burn_colour(10.0)[0]);
        assert_ne!(v.background(), inv.background());
        let travel = DrawSeg { a: Point::ZERO, b: Point::new(1.0, 0.0), power: 0.0, rapid: true, t0: 0.0, dur: 1.0, op: 0 };
        assert_eq!(v.colour_of(&travel), None);
        assert!(ViewOptions { show_travel: true, ..v }.colour_of(&travel).is_some());
    }

    #[test]
    fn image_export_draws_the_job_and_can_be_transparent() {
        let p = data();
        let bed = Rect::new(0.0, 0.0, 100.0, 100.0);
        let png = render_png(&p, bed, 200, &ViewOptions::default(), false, None).unwrap();
        let img = image::load_from_memory(&png).unwrap().to_rgba8();
        assert_eq!(img.dimensions(), (200, 200));
        assert_eq!(img.get_pixel(5, 5).0, [250, 250, 250, 255]);
        // The square's left edge is at x = 10 mm = 20 px, y from 10 to 30 mm (bottom up).
        let dark = (0..200).any(|y| img.get_pixel(20, y).0[0] < 120);
        assert!(dark, "the square's edge is drawn");
        let t = render_png(&p, bed, 200, &ViewOptions::default(), true, None).unwrap();
        let timg = image::load_from_memory(&t).unwrap().to_rgba8();
        assert_eq!(timg.get_pixel(5, 5).0[3], 0, "transparent background");
        // Nothing burnt at time zero.
        let none = render_png(&p, bed, 200, &ViewOptions::default(), false, Some(-1.0)).unwrap();
        let nimg = image::load_from_memory(&none).unwrap().to_rgba8();
        assert!(nimg.pixels().all(|px| px.0 == [250, 250, 250, 255]));
    }
}
