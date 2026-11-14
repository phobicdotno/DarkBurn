//! The Gweike M3 Ultra file workflow [MCC-01..MCC-05, MCC-07 library side].
//!
//! DarkBurn does not talk to this machine. It writes files for the vendor software (DXF with one
//! layer per material, `.nc`, and the `.chf` craft file through `darkburn_mcc::write_chf`) and it
//! can run the design through the library's planner and an in-memory card simulator to show the
//! FIFO plan and a time estimate. There is no socket, serial port or USB code in this module or
//! anywhere it calls: `darkburn-mcc` has no transport. Direct control waits for a read-only
//! capture on the real card (`docs/research/mlaser-rea-findings.md`).
//!
//! Safety stays with the machine: nothing here writes laser, gas, pierce or follower data to a
//! card. The craft table written beside the `.chf` is for the operator to enter or check in the
//! vendor software, whose height follower, piercing and gas logic stay in charge [MCC-07].

use std::collections::BTreeMap;
use std::time::Duration;

use darkburn_core::kurbo::{BezPath, Point};
use darkburn_core::{Document, ShapeKind};
use darkburn_job::geom::flatten;
use darkburn_mcc as mcc;
use serde::{Deserialize, Serialize};

/// The note shown wherever the Gweike machine is mentioned.
pub const DIRECT_CONTROL_NOTE: &str = "DarkBurn does not connect to the Gweike M3 Ultra. It writes the files its own software opens (DXF, .nc and .chf). Direct control waits for a read-only capture on the real card, listed in docs/research/mlaser-rea-findings.md.";

/// Which head a layer is cut with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Head {
    #[default]
    Fibre,
    Co2,
    /// Drawn only: left out of the craft file.
    Skip,
}

impl Head {
    pub const ALL: [Head; 3] = [Head::Fibre, Head::Co2, Head::Skip];
    pub fn label(self) -> &'static str {
        match self {
            Head::Fibre => "Fibre (1200 W)",
            Head::Co2 => "CO2",
            Head::Skip => "Do not cut",
        }
    }
}

/// How one DarkBurn layer maps to the machine's material and craft slot [MCC-04].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LayerMap {
    /// The DXF layer name the material goes under.
    pub material: String,
    pub head: Head,
    /// The craft slot, 0 to 10, a contour in the `.chf` carries.
    pub slot: u8,
}

impl Default for LayerMap {
    fn default() -> Self {
        LayerMap { material: String::new(), head: Head::Fibre, slot: 0 }
    }
}

/// What a Gweike device remembers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GweikeSettings {
    /// By DarkBurn layer index.
    pub layers: BTreeMap<usize, LayerMap>,
    /// Always the eleven slots per head: a shorter table in a file is filled up when read.
    #[serde(deserialize_with = "full_craft_table")]
    pub crafts: mcc::CraftTable,
    /// Planner values for the card simulator.
    pub accel_mm_s2: f64,
    pub rapid_mm_s: f64,
    /// The recipe package folder read last, read again when the quick picker opens.
    pub recipe_folder: String,
    /// The recipes put on a layer lately, most recent first (paths below the package folder).
    pub recipe_recents: Vec<String>,
}

/// Read a craft table and fill it up to the eleven slots per head, so that a slot number that
/// passed its range check always has a craft behind it.
fn full_craft_table<'de, D: serde::Deserializer<'de>>(d: D) -> Result<mcc::CraftTable, D::Error> {
    let mut t = mcc::CraftTable::deserialize(d)?;
    if t.fibre.len() < mcc::craft::LAYERS_PER_HEAD {
        t.fibre.resize(mcc::craft::LAYERS_PER_HEAD, mcc::FibreCraft::default());
    }
    if t.co2.len() < mcc::craft::LAYERS_PER_HEAD {
        t.co2.resize(mcc::craft::LAYERS_PER_HEAD, mcc::Co2Craft::default());
    }
    Ok(t)
}

impl Default for GweikeSettings {
    fn default() -> Self {
        let p = mcc::PlanConfig::default();
        GweikeSettings {
            layers: BTreeMap::new(),
            crafts: mcc::CraftTable::default(),
            accel_mm_s2: p.accel_mm_s2,
            rapid_mm_s: p.rapid_speed_mm_s,
            recipe_folder: String::new(),
            recipe_recents: Vec::new(),
        }
    }
}

impl GweikeSettings {
    /// The mapping of a layer: what was set, else the layer's own number as the craft slot.
    pub fn map_of(&self, layer: usize, doc: &Document) -> LayerMap {
        self.layers.get(&layer).cloned().unwrap_or_else(|| LayerMap {
            material: doc.layer(layer).name.clone(),
            head: Head::Fibre,
            slot: (layer % mcc::craft::LAYERS_PER_HEAD) as u8,
        })
    }

    /// Material name per layer name, for the DXF export.
    pub fn materials(&self, doc: &Document) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        for li in doc.used_layers() {
            let m = self.map_of(li, doc);
            if !m.material.trim().is_empty() {
                let name = doc.layer(li).name.clone();
                let key = if name.is_empty() { darkburn_core::layer::layer_label(li) } else { name };
                out.insert(key, m.material.trim().to_owned());
            }
        }
        out
    }

    /// Problems the vendor software would stop at, with the path of the craft that has them.
    pub fn craft_problems(&self) -> Vec<String> {
        self.crafts
            .validate()
            .into_iter()
            .map(|i| format!("{}{}: {}", if i.severity == mcc::craft::Severity::Error { "" } else { "(warning) " }, i.path, i.message))
            .collect()
    }

    /// The problems that are errors: a value outside the machine's limits or a craft the vendor
    /// software would refuse. A table with one of these is not written out.
    pub fn craft_errors(&self) -> Vec<String> {
        errors_of(self.crafts.validate())
    }
}

fn errors_of(issues: Vec<mcc::craft::CraftIssue>) -> Vec<String> {
    issues.into_iter().filter(|i| i.severity == mcc::craft::Severity::Error).map(|i| format!("{}: {}", i.path, i.message)).collect()
}

// ----- Geometry ----------------------------------------------------------------------------------

/// A flattened outline with the layer it is on.
#[derive(Clone, Debug)]
pub struct Outline {
    pub layer: usize,
    pub pts: Vec<Point>,
    pub closed: bool,
}

/// Outlines of the document's shapes on layers that output, flattened to `tol` mm.
pub fn outlines(doc: &Document, tol: f64) -> Vec<Outline> {
    let mut out = Vec::new();
    for top in doc.iter() {
        for leaf in top.leaves() {
            let li = leaf.layer.min(darkburn_core::LAYER_COUNT - 1);
            if !doc.layer(li).outputs() || leaf.is_image() {
                continue;
            }
            if matches!(&leaf.kind, ShapeKind::Text(t) if t.backup.is_none()) {
                continue;
            }
            let path: BezPath = leaf.outline();
            for poly in flatten(&path, tol) {
                if poly.pts.len() >= 2 {
                    out.push(Outline { layer: li, pts: poly.pts, closed: poly.closed });
                }
            }
        }
    }
    out
}

// ----- The .chf file [MCC-02] ---------------------------------------------------------------------

/// What `build_chf` made.
#[derive(Clone, Debug, PartialEq)]
pub struct ChfBuild {
    pub bytes: Vec<u8>,
    pub contours: usize,
    pub skipped_layers: Vec<usize>,
    pub warnings: Vec<String>,
}

fn contour_of(o: &Outline, slot: i32) -> Result<mcc::chf::Contour, String> {
    let mut elements = Vec::new();
    let n = o.pts.len();
    for w in o.pts.windows(2) {
        if (w[0] - w[1]).hypot() > 1e-6 {
            elements.push(mcc::chf::ContourElement::forward(mcc::chf::Glyph::Segment { p0: w[0], p1: w[1] }));
        }
    }
    if o.closed && n >= 3 && (o.pts[n - 1] - o.pts[0]).hypot() > 1e-6 {
        elements.push(mcc::chf::ContourElement::forward(mcc::chf::Glyph::Segment { p0: o.pts[n - 1], p1: o.pts[0] }));
    }
    mcc::chf::Contour::from_elements(0.01, slot, elements).map_err(|e| e.to_string())
}

/// The `.chf` for the document: one contour per outline, the layer index being the craft slot
/// of the layer's mapping. Layers mapped to "Do not cut" are left out.
pub fn build_chf(doc: &Document, s: &GweikeSettings) -> Result<ChfBuild, String> {
    let mut chf = mcc::ChfDocument::default();
    let mut skipped = Vec::new();
    let mut warnings = Vec::new();
    for o in outlines(doc, 0.01) {
        let m = s.map_of(o.layer, doc);
        if m.head == Head::Skip {
            if !skipped.contains(&o.layer) {
                skipped.push(o.layer);
            }
            continue;
        }
        if usize::from(m.slot) >= mcc::craft::LAYERS_PER_HEAD {
            return Err(format!("Layer {} maps to craft slot {}; the machine has slots 0 to {}.", o.layer, m.slot, mcc::craft::LAYERS_PER_HEAD - 1));
        }
        match contour_of(&o, i32::from(m.slot)) {
            Ok(c) => chf.graphs.push(mcc::chf::Graph::Contour(c)),
            Err(e) => warnings.push(format!("A shape on layer {} was left out: {e}", o.layer)),
        }
    }
    if chf.graphs.is_empty() {
        return Err("There is nothing to put in a craft file: no shapes on layers that cut.".into());
    }
    let heads: std::collections::BTreeSet<Head> = (0..darkburn_core::LAYER_COUNT)
        .filter(|l| doc.used_layers().contains(l))
        .map(|l| s.map_of(l, doc).head)
        .filter(|h| *h != Head::Skip)
        .map(|h| if h == Head::Fibre { Head::Fibre } else { Head::Co2 })
        .collect();
    if heads.len() > 1 {
        warnings.push("Layers use both heads. A .chf holds contours only; the head is chosen in the vendor software for each layer.".into());
    }
    let contours = chf.graphs.len();
    let bytes = mcc::write_chf(&chf).map_err(|e| e.to_string())?;
    Ok(ChfBuild { bytes, contours, skipped_layers: skipped, warnings })
}

/// The craft table as the JSON file written beside the `.chf` (MCC-04): what to enter for each
/// slot in the vendor software.
pub fn crafts_json(s: &GweikeSettings, doc: &Document) -> String {
    let layers: Vec<serde_json::Value> = s
        .layers
        .iter()
        .map(|(l, m)| serde_json::json!({ "darkburn_layer": l, "material": m.material, "head": m.head.label(), "slot": m.slot }))
        .collect();
    let _ = doc;
    serde_json::to_string_pretty(&serde_json::json!({
        "darkburn_gweike_crafts": 1,
        "note": "Enter these in the Gweike software for each craft slot. DarkBurn does not send them to the machine.",
        "layers": layers,
        "crafts": s.crafts,
    }))
    .unwrap_or_default()
}

/// Write `path` (the `.chf`) and `<path>.crafts.json`. Returns the build and the sidecar path.
///
/// Nothing is written while the craft table holds a value outside the machine's limits: the
/// table beside the `.chf` is what the operator enters in the vendor software.
pub fn export_chf(doc: &Document, s: &GweikeSettings, path: &std::path::Path) -> Result<(ChfBuild, std::path::PathBuf), String> {
    if let Some(e) = s.craft_errors().first() {
        return Err(format!("The craft table has a value the machine does not allow ({e}). Correct it, then export again."));
    }
    let build = build_chf(doc, s)?;
    std::fs::write(path, &build.bytes).map_err(|e| format!("Could not write {}: {e}", path.display()))?;
    let mut side = path.as_os_str().to_owned();
    side.push(".crafts.json");
    let side = std::path::PathBuf::from(side);
    std::fs::write(&side, crafts_json(s, doc)).map_err(|e| format!("Could not write {}: {e}", side.display()))?;
    Ok((build, side))
}

// ----- Craft editing [MCC-04] ---------------------------------------------------------------------

/// Set a craft slot from JSON (the shape of `mcc::FibreCraft` or `mcc::Co2Craft`). A craft with
/// a value outside the machine's limits is refused and the slot keeps what it had.
pub fn set_craft(s: &mut GweikeSettings, head: Head, slot: usize, value: &serde_json::Value) -> Result<(), String> {
    if slot >= mcc::craft::LAYERS_PER_HEAD {
        return Err(format!("Craft slots are 0 to {}.", mcc::craft::LAYERS_PER_HEAD - 1));
    }
    let refuse = |issues: Vec<mcc::craft::CraftIssue>| match errors_of(issues).first() {
        Some(e) => Err(format!("The craft was not stored, it has a value the machine does not allow ({e}).")),
        None => Ok(()),
    };
    match head {
        Head::Fibre => {
            let c: mcc::FibreCraft = serde_json::from_value(value.clone()).map_err(|e| format!("fibre craft: {e}"))?;
            refuse(c.validate())?;
            s.crafts.fibre[slot] = c;
        }
        Head::Co2 => {
            let c: mcc::Co2Craft = serde_json::from_value(value.clone()).map_err(|e| format!("CO2 craft: {e}"))?;
            refuse(c.validate())?;
            s.crafts.co2[slot] = c;
        }
        Head::Skip => return Err("A layer that is not cut has no craft.".into()),
    }
    Ok(())
}

// ----- The card simulator [MCC-05] ------------------------------------------------------------------

/// The FIFO plan of a design: the planner dry run, nothing more.
#[derive(Clone, Debug, PartialEq)]
pub struct PlanSummary {
    pub ticks: u64,
    pub frames: usize,
    pub bytes: u64,
    pub estimated_s: f64,
    pub cut_s: f64,
    pub rapid_s: f64,
    pub cut_length_mm: f64,
    pub rapid_length_mm: f64,
    pub pulses: (i64, i64),
}

/// The toolpath the planner takes: rapid to the start of each outline, cut along it at the
/// speed of its craft (default 100 mm/s).
pub fn path_segments(doc: &Document, s: &GweikeSettings) -> Vec<mcc::PathSegment> {
    let mut out = Vec::new();
    for o in outlines(doc, 0.05) {
        let m = s.map_of(o.layer, doc);
        let speed = match m.head {
            Head::Skip => continue,
            Head::Fibre => s.crafts.fibre_layer(i32::from(m.slot)).map_or(100.0, |c| c.cut.speed_mm_s),
            Head::Co2 => s.crafts.co2_layer(i32::from(m.slot)).map_or(100.0, |c| c.speed_mm_s),
        }
        .max(1.0);
        out.push(mcc::PathSegment::rapid(o.pts[0].x, o.pts[0].y));
        for p in &o.pts[1..] {
            out.push(mcc::PathSegment::cut(p.x, p.y, speed));
        }
        if o.closed {
            out.push(mcc::PathSegment::cut(o.pts[0].x, o.pts[0].y, speed));
        }
    }
    out
}

fn plan_config(s: &GweikeSettings, bed: (f64, f64)) -> mcc::PlanConfig {
    mcc::PlanConfig { work_area_mm: bed, accel_mm_s2: s.accel_mm_s2.max(1.0), rapid_speed_mm_s: s.rapid_mm_s.max(1.0), ..Default::default() }
}

/// Run the planner dry run over the document. No motion path, no transport.
pub fn plan(doc: &Document, s: &GweikeSettings, bed: (f64, f64)) -> Result<(mcc::DryRunPlan, PlanSummary), String> {
    let segs = path_segments(doc, s);
    if segs.is_empty() {
        return Err("There is nothing to plan: no shapes on layers that cut.".into());
    }
    plan_path(&segs, s, bed)
}

fn plan_path(segs: &[mcc::PathSegment], s: &GweikeSettings, bed: (f64, f64)) -> Result<(mcc::DryRunPlan, PlanSummary), String> {
    // The same ceiling the card gate holds jogs and go-tos to.
    let max_acc = f64::from(mcc::SafetyConfig::default().max_acc);
    if s.accel_mm_s2.is_nan() || s.accel_mm_s2 > max_acc {
        return Err(format!("The acceleration in the device settings ({} mm/s2) is above what the machine allows ({max_acc} mm/s2).", s.accel_mm_s2));
    }
    let p = mcc::dry_run(segs, &plan_config(s, bed)).map_err(|e| e.to_string())?;
    let sum = PlanSummary {
        ticks: p.tick_count,
        frames: p.frames.len(),
        bytes: p.tick_bytes(),
        estimated_s: p.estimated_seconds,
        cut_s: p.cut_seconds,
        rapid_s: p.rapid_seconds,
        cut_length_mm: p.cut_length_mm,
        rapid_length_mm: p.rapid_length_mm,
        pulses: (p.total_pulses_x, p.total_pulses_y),
    };
    Ok((p, sum))
}

/// What the simulated card did with a plan.
#[derive(Clone, Debug, PartialEq)]
pub struct CardReport {
    pub frames_accepted: u64,
    pub ticks_consumed: u64,
    pub underruns: u64,
    pub pulse_position: (i64, i64),
    pub forbidden_attempts: usize,
    pub laser_refusals: u64,
    pub sim_seconds: f64,
}

/// Stream a plan into the in-memory card (`CardSimulator` behind the safety gate) and run it.
/// Everything stays in this process: the "card" is a struct, not a device.
pub fn run_on_card_simulator(plan: &mcc::DryRunPlan) -> Result<CardReport, String> {
    let mut link = mcc::SimLink::new(mcc::CardSimulator::new(mcc::SimConfig { fifo_starvation_alarm: false, ..mcc::SimConfig::default() }));
    run_plan_on(&mut link, plan, 200_000)
}

/// Run `plan` on `link`, giving it at most `max_steps` quarter seconds of card time.
/// However the run ends, the card is left stopped: the stop sequence goes out last.
fn run_plan_on(link: &mut mcc::SimLink, plan: &mcc::DryRunPlan, max_steps: usize) -> Result<CardReport, String> {
    let r = stream_plan_on(link, plan, max_steps);
    link.stop_sequence(false);
    r
}

fn stream_plan_on(link: &mut mcc::SimLink, plan: &mcc::DryRunPlan, max_steps: usize) -> Result<CardReport, String> {
    stream_plan_with(link, plan, max_steps, &mut |_, _| {})
}

/// [`stream_plan_on`] with `hook` called before every poll (tests use it to change the card).
fn stream_plan_with(
    link: &mut mcc::SimLink,
    plan: &mcc::DryRunPlan,
    max_steps: usize,
    hook: &mut dyn FnMut(&mut mcc::SimLink, usize),
) -> Result<CardReport, String> {
    use mcc::command;
    link.gate.arming.arm_motion().map_err(|e| format!("{e:?}"))?;
    let lerr = |e: mcc::sim::LinkError| format!("{e:?}");
    link.poll_status().map_err(lerr)?;
    link.write(&command::fifo_clear().map_err(|e| format!("{e:?}"))?).map_err(lerr)?;
    let mut feeder = plan.feeder();
    let mut started = false;
    let mut elapsed = Duration::ZERO;
    let step = Duration::from_millis(250);
    let mut finished = false;
    for n in 0..max_steps {
        hook(link, n);
        let st = link.poll_status().map_err(lerr)?;
        // The gate stops the stream on an alarm; an empty, idle queue then is not a finished run.
        if let Some(fault) = &link.gate.machine_fault {
            let mut why = mcc::registers::alarm_labels(st.alarm1(), st.alarm2());
            if why.is_empty() {
                why.push(fault.clone());
            }
            return Err(format!("the simulated card stopped the run on an alarm: {}", why.join("; ")));
        }
        if link.gate.queue_held() {
            return Err("the simulated card stopped the run before it finished (an alarm or a stop)".into());
        }
        let r = feeder.fill(st.fifo_frame_id(), st.fifo_space_margin(), 50);
        for (id, frame) in r.frames {
            link.write(&command::fifo_packed_frame(id, &frame).map_err(|e| format!("{e:?}"))?).map_err(lerr)?;
        }
        if !started {
            link.write(&command::fifo_start().map_err(|e| format!("{e:?}"))?).map_err(lerr)?;
            started = true;
        }
        if feeder.pending() == 0 && !link.sim.fifo_running() {
            finished = true;
            break;
        }
        link.advance(step);
        elapsed += step;
    }
    if !finished {
        return Err(format!("the simulated card was still running after {:.0} s of card time; stopped", elapsed.as_secs_f64()));
    }
    Ok(CardReport {
        frames_accepted: link.sim.frames_accepted(),
        ticks_consumed: link.sim.ticks_consumed(),
        underruns: link.sim.underruns(),
        pulse_position: link.sim.pulse_position(),
        forbidden_attempts: link.sim.forbidden_attempts().len(),
        laser_refusals: link.sim.laser_refusals(),
        sim_seconds: elapsed.as_secs_f64(),
    })
}

#[cfg(test)]
mod hunt8_tests {
    use super::*;

    fn sim_link() -> mcc::SimLink {
        mcc::SimLink::new(mcc::CardSimulator::new(mcc::SimConfig { fifo_starvation_alarm: false, ..mcc::SimConfig::default() }))
    }

    #[test]
    fn a_simulated_run_that_runs_out_of_time_says_so_and_stops_the_card() {
        // 200 mm at 50 mm/s is four seconds of card time; the run gets one.
        let plan = mcc::dry_run(&[mcc::PathSegment::cut(200.0, 0.0, 50.0)], &mcc::PlanConfig::default()).unwrap();
        let mut link = sim_link();
        let r = run_plan_on(&mut link, &plan, 4);
        assert!(r.is_err(), "reported as a finished run: {r:?}");
        assert!(!link.sim.fifo_running(), "the stream was left running");
        // with enough time the same plan finishes and is reported
        let mut link = sim_link();
        let done = run_plan_on(&mut link, &plan, 200_000).unwrap();
        assert!(done.ticks_consumed > 0 && done.sim_seconds < 10.0, "{done:?}");
        assert!(!link.sim.fifo_running());
        assert_eq!(link.sim.outputs(), 0);
    }

    #[test]
    fn a_gweike_simulated_run_stopped_by_an_alarm_is_not_reported_as_finished() {
        // The drained stream raises the starvation alarm on this card; the gate stops the queue,
        // so "nothing pending and not running" no longer means the plan ran to the end.
        let plan = mcc::dry_run(&[mcc::PathSegment::cut(20.0, 0.0, 50.0)], &mcc::PlanConfig::default()).unwrap();
        let mut link = mcc::SimLink::new(mcc::CardSimulator::new(mcc::SimConfig::default()));
        let r = run_plan_on(&mut link, &plan, 200_000);
        let e = r.expect_err("a run the gate stopped on an alarm was reported as finished");
        assert!(e.contains("alarm"), "{e}");
        assert!(!link.sim.fifo_running(), "the stream was left running");
        // An alarm in the middle of a longer run stops it the same way.
        let plan = mcc::dry_run(&[mcc::PathSegment::cut(200.0, 0.0, 50.0)], &mcc::PlanConfig::default()).unwrap();
        let mut link = sim_link();
        let mut hook = |l: &mut mcc::SimLink, step: usize| {
            if step == 4 {
                l.sim.inject_alarm(None, Some(1 << 3));
            }
        };
        let r = stream_plan_with(&mut link, &plan, 200_000, &mut hook);
        link.stop_sequence(false);
        assert!(r.is_err(), "a run stopped by an alarm half way was reported as finished: {r:?}");
        assert!(!link.sim.fifo_running());
    }

    #[test]
    fn a_plan_does_not_accelerate_harder_than_the_machine_allows() {
        // The card's ceiling is 20000 mm/s2 (jogs and go-tos are held to it). A device file with
        // a larger value planned a stream that jumps to full speed in one tick.
        let segs = [mcc::PathSegment::cut(200.0, 0.0, 500.0)];
        let bed = (1300.0, 900.0);
        for accel in [20_000.1, 1.0e6, f64::INFINITY] {
            let s = GweikeSettings { accel_mm_s2: accel, ..GweikeSettings::default() };
            assert!(plan_path(&segs, &s, bed).is_err(), "planned with {accel} mm/s2");
        }
        let s = GweikeSettings { accel_mm_s2: 20_000.0, ..GweikeSettings::default() };
        plan_path(&segs, &s, bed).unwrap();
        plan_path(&segs, &GweikeSettings::default(), bed).unwrap();
    }
}
