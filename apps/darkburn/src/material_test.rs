//! Material test generator [LT-12, MT-01 to MT-07]: a grid of small squares, each on its own
//! layer with its own speed, power, interval, frequency or Q-pulse, with the values written
//! beside the rows and columns. The grid lands in the document as ordinary layers and shapes, so
//! it runs on G-code, DSP and galvo devices alike.

use darkburn_core::kurbo::{Affine, BezPath, Point, Rect};
use darkburn_core::layer::{self, Mode};
use darkburn_core::{Document, Shape, ShapeId};
use serde::{Deserialize, Serialize};

/// What a grid axis varies.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Axis {
    Speed,
    Power,
    Interval,
    Frequency,
    QPulse,
}

impl Axis {
    pub const ALL: [Axis; 5] = [Axis::Speed, Axis::Power, Axis::Interval, Axis::Frequency, Axis::QPulse];

    pub fn label(self) -> &'static str {
        match self {
            Axis::Speed => "Speed (mm/s)",
            Axis::Power => "Power (%)",
            Axis::Interval => "Interval (mm)",
            Axis::Frequency => "Frequency (kHz)",
            Axis::QPulse => "Q-pulse (ns)",
        }
    }

    /// The unit written on the test sheet next to the numbers (letters the stroke font has).
    pub fn unit(self) -> &'static str {
        match self {
            Axis::Speed => "mm/s",
            Axis::Power => "%",
            Axis::Interval => "mm",
            Axis::Frequency => "kHz",
            Axis::QPulse => "ns",
        }
    }

    pub fn from_name(name: &str) -> Option<Axis> {
        Some(match name {
            "speed" => Axis::Speed,
            "power" => Axis::Power,
            "interval" => Axis::Interval,
            "frequency" => Axis::Frequency,
            "q_pulse" | "qpulse" => Axis::QPulse,
            _ => return None,
        })
    }
}

/// The test's parameters. A preset is a named copy of these.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MaterialTestParams {
    pub x_axis: Axis,
    pub x_min: f64,
    pub x_max: f64,
    pub columns: usize,
    pub y_axis: Axis,
    pub y_min: f64,
    pub y_max: f64,
    pub rows: usize,
    /// Cell size and gap, mm.
    pub cell_w: f64,
    pub cell_h: f64,
    pub gap: f64,
    /// Bottom-left corner of the grid including its labels, mm.
    pub x: f64,
    pub y: f64,
    /// Cells are filled squares (Fill) or outlines (Line).
    pub fill: bool,
    pub passes: u32,
    /// Values for the axes the grid does not vary.
    pub base_speed: f64,
    pub base_power: f64,
    pub base_interval: f64,
    pub labels: bool,
    pub label_height: f64,
    /// The layer the labels burn on, with its own speed and power (MT-04).
    pub label_layer: usize,
    pub label_speed: f64,
    pub label_power: f64,
    /// The first layer used for cells; cells take the next free colour layers.
    pub first_layer: usize,
}

impl Default for MaterialTestParams {
    fn default() -> Self {
        MaterialTestParams {
            x_axis: Axis::Speed,
            x_min: 50.0,
            x_max: 250.0,
            columns: 5,
            y_axis: Axis::Power,
            y_min: 10.0,
            y_max: 50.0,
            rows: 5,
            cell_w: 10.0,
            cell_h: 10.0,
            gap: 2.0,
            x: 10.0,
            y: 10.0,
            fill: false,
            passes: 1,
            base_speed: 100.0,
            base_power: 20.0,
            base_interval: 0.1,
            labels: true,
            label_height: 3.0,
            label_layer: 29,
            label_speed: 150.0,
            label_power: 10.0,
            first_layer: 0,
        }
    }
}

/// What a run created.
#[derive(Clone, Debug, PartialEq)]
pub struct MaterialTestResult {
    pub shapes: Vec<ShapeId>,
    pub layers: Vec<usize>,
    pub bounds: Rect,
}

/// `count` values from `min` to `max` (one value when `count` is 1).
pub fn steps(min: f64, max: f64, count: usize) -> Vec<f64> {
    if count <= 1 {
        return vec![min];
    }
    (0..count).map(|i| min + (max - min) * i as f64 / (count - 1) as f64).collect()
}

fn fmt(v: f64) -> String {
    let s = format!("{v:.3}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn set_axis(s: &mut layer::CutSetting, axis: Axis, v: f64) {
    match axis {
        Axis::Speed => s.speed = v.max(0.01),
        Axis::Power => {
            s.max_power = v.clamp(0.0, 100.0);
            s.min_power = s.min_power.min(s.max_power);
        }
        Axis::Interval => s.interval = v.max(0.001),
        Axis::Frequency => s.galvo.frequency_khz = v.max(0.0),
        Axis::QPulse => s.galvo.q_pulse_ns = v.max(0.0),
    }
}

fn cell_layers(p: &MaterialTestParams) -> Vec<usize> {
    (p.first_layer..layer::COLOUR_LAYERS).filter(|l| *l != p.label_layer).collect()
}

/// Check the parameters; the message says what to change.
pub fn validate(p: &MaterialTestParams) -> Result<(), String> {
    if p.columns == 0 || p.rows == 0 {
        return Err("The grid needs at least one row and one column.".into());
    }
    if p.x_axis == p.y_axis && p.columns > 1 && p.rows > 1 {
        return Err("The columns and the rows cannot vary the same setting.".into());
    }
    if p.cell_w <= 0.0 || p.cell_h <= 0.0 || p.gap < 0.0 {
        return Err("Cell size must be above zero.".into());
    }
    if p.label_layer >= layer::COLOUR_LAYERS {
        return Err("The label layer must be a colour layer.".into());
    }
    let free = cell_layers(p).len();
    if p.columns * p.rows > free {
        return Err(format!(
            "{} cells need {} layers but only {free} colour layers are free; use fewer rows or columns.",
            p.columns * p.rows,
            p.columns * p.rows
        ));
    }
    Ok(())
}

/// Add the test to `doc`: layers get the cell settings, shapes are added on top. Call inside an
/// editor edit so it is one undo step.
pub fn generate(doc: &mut Document, p: &MaterialTestParams) -> Result<MaterialTestResult, String> {
    validate(p)?;
    let xs = steps(p.x_min, p.x_max, p.columns);
    let ys = steps(p.y_min, p.y_max, p.rows);
    let layers = cell_layers(p);
    let label_h = if p.labels { p.label_height } else { 0.0 };
    // Room for the row labels on the left and the column labels on top.
    let label_w = if p.labels {
        let unit_w = darkburn_vision::strokefont::text_width(p.y_axis.unit(), p.label_height);
        let longest = ys.iter().map(|v| darkburn_vision::strokefont::text_width(&fmt(*v), p.label_height)).fold(unit_w, f64::max);
        longest + p.gap + 1.0
    } else {
        0.0
    };
    let grid_x = p.x + label_w;
    let grid_h = p.rows as f64 * p.cell_h + (p.rows as f64 - 1.0) * p.gap;
    let grid_top = p.y + grid_h;
    let mut shapes = Vec::new();
    let mut used = Vec::new();
    for (r, yv) in ys.iter().enumerate() {
        for (c, xv) in xs.iter().enumerate() {
            let layer_index = layers[r * p.columns + c];
            {
                let l = doc.layer_mut(layer_index);
                l.output = true;
                l.settings.truncate(1);
                let s = l.main_mut();
                s.mode = if p.fill { Mode::Fill } else { Mode::Line };
                s.passes = p.passes.max(1);
                s.speed = p.base_speed;
                s.max_power = p.base_power;
                s.interval = p.base_interval;
                set_axis(s, p.x_axis, *xv);
                set_axis(s, p.y_axis, *yv);
                s.sanitise();
            }
            let x0 = grid_x + c as f64 * (p.cell_w + p.gap);
            let y1 = grid_top - r as f64 * (p.cell_h + p.gap);
            shapes.push(doc.add(Shape::rect(Rect::new(x0, y1 - p.cell_h, x0 + p.cell_w, y1), 0.0, layer_index)));
            used.push(layer_index);
        }
    }
    if p.labels {
        {
            let l = doc.layer_mut(p.label_layer);
            l.output = true;
            let s = l.main_mut();
            s.mode = Mode::Line;
            s.speed = p.label_speed;
            s.max_power = p.label_power;
            s.passes = 1;
            s.sanitise();
        }
        for (c, xv) in xs.iter().enumerate() {
            let text = fmt(*xv);
            let w = darkburn_vision::strokefont::text_width(&text, p.label_height);
            let cx = grid_x + c as f64 * (p.cell_w + p.gap) + (p.cell_w - w) / 2.0;
            shapes.push(doc.add(label(&text, p.label_height, Point::new(cx, grid_top + p.gap), p.label_layer)));
        }
        for (r, yv) in ys.iter().enumerate() {
            let text = fmt(*yv);
            let w = darkburn_vision::strokefont::text_width(&text, p.label_height);
            let cy = grid_top - r as f64 * (p.cell_h + p.gap) - (p.cell_h + p.label_height) / 2.0;
            shapes.push(doc.add(label(&text, p.label_height, Point::new(grid_x - p.gap - w, cy), p.label_layer)));
        }
        // The units: the rows' above their numbers, the columns' after the last column number.
        let top = grid_top + p.gap;
        shapes.push(doc.add(label(p.y_axis.unit(), p.label_height, Point::new(p.x, top), p.label_layer)));
        let right = grid_x + p.columns as f64 * (p.cell_w + p.gap) + p.gap;
        shapes.push(doc.add(label(p.x_axis.unit(), p.label_height, Point::new(right, top), p.label_layer)));
        used.push(p.label_layer);
    }
    let unit_w = if p.labels { darkburn_vision::strokefont::text_width(p.x_axis.unit(), p.label_height) + p.gap } else { 0.0 };
    let bounds = Rect::new(
        p.x,
        p.y,
        unit_w + grid_x + p.columns as f64 * p.cell_w + (p.columns as f64 - 1.0) * p.gap,
        grid_top + if p.labels { p.gap + label_h } else { 0.0 },
    );
    Ok(MaterialTestResult { shapes, layers: used, bounds })
}

fn label(text: &str, height: f64, at: Point, layer: usize) -> Shape {
    let path: BezPath = darkburn_vision::strokefont::text_path(text, height, at);
    let mut s = Shape::new(darkburn_core::ShapeKind::Path { path }, Affine::IDENTITY, layer);
    s.name = format!("Label {text}");
    s
}

// ----- Framing the test (MT-06) ---------------------------------------------------------------

/// The points that frame the test the dialog would add (just the test, not the rest of the
/// project), as work coordinates for the connected controller.
pub fn frame_points(app: &crate::shell::DarkBurn) -> Result<Vec<Point>, String> {
    let mut input = crate::job_cmds::build_input(app);
    input.doc.shapes.clear();
    input.selection.clear();
    input.settings.cut_selected = false;
    generate(&mut input.doc, &app.job.material)?;
    let tp = crate::job_build::build_frame(&input, crate::job_build::FrameKind::Rectangle, 100.0, None).map_err(|e| e.to_string())?;
    Ok(crate::machine_job::work_points(app, &tp))
}

/// Move the laser head round the test (laser off).
pub fn frame_test(app: &mut crate::shell::DarkBurn) -> Result<(), String> {
    if !app.machine.handle.is_connected() {
        return Err("The laser is not connected. Press Connect in the Laser panel first.".into());
    }
    let points = frame_points(app)?;
    app.machine.handle.frame_outline(points, crate::machine_api::FrameSpec { feed: 3000.0, laser_percent: None }).map_err(|e| e.to_string())
}

// ----- Presets (MT-05) ---------------------------------------------------------------------------

/// A named set of parameters.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Preset {
    pub name: String,
    pub params: MaterialTestParams,
}

#[derive(Serialize, Deserialize, Default)]
struct PresetFile {
    format: String,
    version: u32,
    presets: Vec<Preset>,
}

const FORMAT: &str = "darkburn-material-test-presets";

/// The presets as a file's text.
pub fn presets_to_json(presets: &[Preset]) -> String {
    serde_json::to_string_pretty(&PresetFile { format: FORMAT.into(), version: 1, presets: presets.to_vec() }).unwrap_or_default()
}

/// Read presets from a file's text.
pub fn presets_from_json(text: &str) -> Result<Vec<Preset>, String> {
    let f: PresetFile = serde_json::from_str(text).map_err(|e| format!("not a material test preset file: {e}"))?;
    if f.format != FORMAT {
        return Err("not a material test preset file".into());
    }
    Ok(f.presets)
}

/// Add `incoming` to `list`; a preset with the same name is replaced. Returns how many were added.
pub fn merge_presets(list: &mut Vec<Preset>, incoming: Vec<Preset>) -> usize {
    let n = incoming.len();
    for p in incoming {
        match list.iter_mut().find(|q| q.name == p.name) {
            Some(q) => *q = p,
            None => list.push(p),
        }
    }
    n
}

#[cfg(test)]
mod tests {
    #[test]
    fn framing_the_test_frames_the_test_only() {
        let mut app = crate::shell::DarkBurn::new(crate::prefs::Prefs::default());
        app.editor.edit("far", |d, _| {
            d.add(Shape::rect(Rect::new(300.0, 300.0, 310.0, 310.0), 0.0, 0));
            true
        });
        app.job.material = MaterialTestParams { x: 10.0, y: 10.0, ..Default::default() };
        let pts = frame_points(&app).unwrap();
        assert!(pts.len() >= 4);
        let max_x = pts.iter().map(|p| p.x).fold(f64::MIN, f64::max);
        assert!(max_x < 200.0, "the far square is not part of the frame: {max_x}");
        assert!(pts.iter().map(|p| p.x).fold(f64::MAX, f64::min) >= 9.99);
        // Not connected: refused, not silently ignored.
        assert!(frame_test(&mut app).unwrap_err().contains("not connected"));
    }

    #[test]
    fn every_axis_unit_can_be_written_with_the_stroke_font() {
        for a in Axis::ALL {
            assert!(a.unit().chars().all(|c| darkburn_vision::strokefont::glyph(c).is_some()), "{}", a.unit());
        }
        // The unit labels are on the label layer and carry the unit in their name.
        let mut d = Document::new();
        let r = generate(&mut d, &MaterialTestParams::default()).unwrap();
        let names: Vec<String> = r.shapes.iter().filter_map(|i| d.get(*i)).map(|s| s.name.clone()).collect();
        assert!(names.iter().any(|n| n == "Label mm/s") && names.iter().any(|n| n == "Label %"), "{names:?}");
    }

    use super::*;

    #[test]
    fn grid_of_speed_by_power() {
        let mut d = Document::new();
        let p = MaterialTestParams::default();
        let r = generate(&mut d, &p).unwrap();
        // 25 cells, 10 numbers and a unit for each axis.
        assert_eq!(r.shapes.len(), 25 + 10 + 2, "cells, the numbers and the two units");
        // The first cell is the slowest and weakest, on layer 0.
        let first = d.get(r.shapes[0]).unwrap();
        assert_eq!(first.layer, 0);
        assert_eq!(d.layer(0).main().speed, 50.0);
        assert_eq!(d.layer(0).main().max_power, 10.0);
        // The last cell is the fastest and strongest.
        let last = d.get(r.shapes[24]).unwrap().layer;
        assert_eq!(d.layer(last).main().speed, 250.0);
        assert_eq!(d.layer(last).main().max_power, 50.0);
        // Labels are on the label layer with their own settings.
        assert_eq!(d.get(r.shapes[25]).unwrap().layer, 29);
        assert_eq!(d.layer(29).main().speed, 150.0);
        assert!(r.bounds.width() > 5.0 * 10.0 && r.bounds.height() > 5.0 * 10.0);
        let b = d.get(r.shapes[0]).unwrap().bbox();
        assert!((b.width() - 10.0).abs() < 1e-6);
    }

    #[test]
    fn other_axes_and_modes() {
        let mut d = Document::new();
        let p = MaterialTestParams {
            x_axis: Axis::Interval,
            x_min: 0.05,
            x_max: 0.2,
            columns: 4,
            y_axis: Axis::Frequency,
            y_min: 20.0,
            y_max: 60.0,
            rows: 2,
            fill: true,
            labels: false,
            ..Default::default()
        };
        let r = generate(&mut d, &p).unwrap();
        assert_eq!(r.shapes.len(), 8);
        assert_eq!(d.layer(0).main().mode, Mode::Fill);
        assert!((d.layer(0).main().interval - 0.05).abs() < 1e-9);
        assert_eq!(d.layer(0).main().galvo.frequency_khz, 20.0);
        assert_eq!(d.layer(7).main().galvo.frequency_khz, 60.0);
    }

    #[test]
    fn too_many_cells_and_bad_axes_are_refused() {
        let mut d = Document::new();
        let big = MaterialTestParams { columns: 10, rows: 10, ..Default::default() };
        assert!(generate(&mut d, &big).unwrap_err().contains("layers"));
        assert!(d.is_empty());
        let same = MaterialTestParams { y_axis: Axis::Speed, ..Default::default() };
        assert!(generate(&mut d, &same).is_err());
        assert!(generate(&mut d, &MaterialTestParams { rows: 0, ..Default::default() }).is_err());
    }

    #[test]
    fn presets_round_trip_and_merge() {
        let a = Preset { name: "Birch 3 mm".into(), params: MaterialTestParams::default() };
        let b = Preset { name: "Acrylic".into(), params: MaterialTestParams { columns: 3, ..Default::default() } };
        let text = presets_to_json(&[a.clone(), b.clone()]);
        let back = presets_from_json(&text).unwrap();
        assert_eq!(back, vec![a.clone(), b]);
        let mut list = vec![Preset { name: "Birch 3 mm".into(), params: MaterialTestParams { rows: 2, ..Default::default() } }];
        assert_eq!(merge_presets(&mut list, back), 2);
        assert_eq!(list.len(), 2);
        assert_eq!(list[0], a);
        assert!(presets_from_json("{}").is_err());
    }

    #[test]
    fn the_test_becomes_a_job() {
        let mut d = Document::new();
        generate(&mut d, &MaterialTestParams { columns: 3, rows: 2, ..Default::default() }).unwrap();
        let input = crate::job_build::BuildInput {
            doc: d,
            selection: vec![],
            settings: Default::default(),
            device: crate::job_build::JobDevice::new(300.0, 300.0),
            user_origin: None,
            head: None,
            motion: Default::default(),
        };
        let b = crate::job_build::build_job_simple(&input).unwrap();
        assert!(b.toolpath.layers.len() >= 7, "six cells and the labels");
    }
}
