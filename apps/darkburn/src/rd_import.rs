//! Ruida `.rd` files as artwork (IMP-07, RUIDA-13): the cut paths of every layer, each layer in
//! its own colour so they land on separate layers of the project, with the layer's speed, power,
//! air assist, PWM frequency and dot mode as its cut settings.
//!
//! What an `.rd` file cannot give back: curves (it holds straight moves only), fills and images
//! (they are scan lines, which come in as lines), text, groups and the names of layers.

use darkburn_core::Layer;
use darkburn_import as di;
use darkburn_ruida::{LayerSettings, RdJob, RdOp};
use kurbo::{BezPath, Point};

/// The extension of Ruida job files.
pub const EXTENSION: &str = "rd";

/// Whether `name` looks like a Ruida job file.
pub fn is_rd(name: &std::path::Path) -> bool {
    name.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case(EXTENSION))
}

/// Read `.rd` bytes as an imported drawing in millimetres, Y up.
#[cfg(test)]
pub fn import(bytes: &[u8]) -> Result<di::Imported, String> {
    import_with_settings(bytes).map(|(imported, _)| imported)
}

/// The cut settings of one layer of the file and which of the imported items are its paths.
#[derive(Clone, Debug, PartialEq)]
pub struct LayerCuts {
    /// Index of the layer's first item in the imported drawing.
    pub first_item: usize,
    pub items: usize,
    pub settings: LayerSettings,
}

/// As [`import`], with the cut settings of each layer that holds paths.
pub fn import_with_settings(bytes: &[u8]) -> Result<(di::Imported, Vec<LayerCuts>), String> {
    let job = RdJob::from_rd_file(bytes, None).map_err(|e| format!("this is not a readable Ruida job file: {e}"))?;
    Ok(to_imported(&job))
}

/// A power as the user typed it: the file holds it in steps of about 0.006 %.
fn percent(p: f64) -> f64 {
    if p >= 99.99 { 100.0 } else { (p * 100.0).round() / 100.0 }
}

/// The layer `index` with the cut settings an `.rd` layer carries.
pub fn layer_from_rd(index: usize, s: &LayerSettings) -> Layer {
    let mut layer = Layer::new(index);
    layer.air = s.air_assist;
    let main = layer.main_mut();
    main.mode = darkburn_core::Mode::Line;
    main.speed = s.speed_mm_s;
    main.min_power = percent(s.min_power[0]);
    main.max_power = percent(s.max_power[0]);
    main.min_power_2 = percent(s.min_power[1]);
    main.max_power_2 = percent(s.max_power[1]);
    if let Some(hz) = s.frequency_hz {
        main.pwm_override = true;
        main.pwm_khz = f64::from(hz) / 1000.0;
    }
    if let Some(d) = s.dot_mode {
        main.dot_mode = true;
        main.dot_time_ms = d.dot_time_ms;
        main.dot_spacing = d.spacing_mm;
    }
    layer
}

/// After the drawing became shapes: give each layer the file's cut settings. `items` is the
/// number of items the drawing had; when the shapes do not match it one to one the settings are
/// left out and the user is told.
pub fn carry_cut_settings(loaded: &mut crate::formats::Loaded, cuts: &[LayerCuts], items: usize) {
    if loaded.imported.shapes.len() != items {
        loaded.warnings.push("the cut settings of the file were not carried over".into());
        return;
    }
    for c in cuts {
        let Some(shape) = loaded.imported.shapes.get(c.first_item) else { continue };
        let index = shape.layer;
        // Two layers of the file with one colour share a layer: the first one's settings win.
        if !loaded.layers.iter().any(|l| l.index == index) {
            loaded.layers.push(layer_from_rd(index, &c.settings));
        }
    }
}

fn to_imported(job: &RdJob) -> (di::Imported, Vec<LayerCuts>) {
    // The controller's Y grows downwards; the drawing is turned over its own bounds so it stands
    // upright.
    let top = job.bounds_mm().y1;
    let at = |p: Point| Point::new(p.x, top - p.y);
    let mut out = di::Imported::default();
    let mut cuts = Vec::new();
    // The head stays where a layer left it: the next layer may cut on from there.
    let mut here = Point::ZERO;
    for layer in &job.layers {
        let s = &layer.settings;
        let mut paths: Vec<BezPath> = Vec::new();
        let mut cur: Option<BezPath> = None;
        for op in &layer.ops {
            match *op {
                RdOp::Move(p) => {
                    if let Some(c) = cur.take() {
                        paths.push(c);
                    }
                    here = p;
                }
                RdOp::Cut { to, .. } => {
                    let c = cur.get_or_insert_with(|| {
                        let mut b = BezPath::new();
                        b.move_to(at(here));
                        b
                    });
                    c.line_to(at(to));
                    here = to;
                }
                RdOp::Z(_) | RdOp::U(_) => {}
            }
        }
        if let Some(c) = cur.take() {
            paths.push(c);
        }
        // A line that comes back to where it began is a closed shape.
        for p in &mut paths {
            let pts: Vec<Point> = p.elements().iter().filter_map(|e| e.end_point()).collect();
            if pts.len() > 2 && pts[0].distance(pts[pts.len() - 1]) < 1e-3 {
                p.close_path();
            }
        }
        if paths.is_empty() {
            continue;
        }
        let colour = di::Rgb::from_packed(s.color & 0xFF_FFFF);
        let name = format!("Layer {}", s.index);
        cuts.push(LayerCuts { first_item: out.items.len(), items: paths.len(), settings: s.clone() });
        for path in paths {
            let mut item = di::Item::new(di::ItemKind::Path(di::PathItem { path, stroke: Some(colour), fill: None, dash: None }));
            item.layer = Some(name.clone());
            out.items.push(item);
        }
        out.warn(format!("{name}: {} mm/s, {}% to {}% power", s.speed_mm_s, percent(s.min_power[0]), percent(s.max_power[0])));
    }
    if out.items.is_empty() {
        out.warn("the job holds no cut paths");
    }
    (out, cuts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use darkburn_ruida::RdLayer;
    use darkburn_ruida::swizzle::Magic;

    fn job() -> RdJob {
        let square = vec![
            RdOp::Move(Point::new(10.0, 10.0)),
            RdOp::Cut { to: Point::new(30.0, 10.0), power: None, speed: None },
            RdOp::Cut { to: Point::new(30.0, 20.0), power: None, speed: None },
            RdOp::Cut { to: Point::new(10.0, 20.0), power: None, speed: None },
            RdOp::Cut { to: Point::new(10.0, 10.0), power: None, speed: None },
        ];
        let line = vec![RdOp::Move(Point::new(50.0, 10.0)), RdOp::Cut { to: Point::new(60.0, 10.0), power: None, speed: None }];
        RdJob {
            reference: Default::default(),
            layers: vec![
                RdLayer {
                    settings: LayerSettings {
                        index: 0,
                        color: 0xFF0000,
                        speed_mm_s: 40.0,
                        min_power: [12.5, 0.0],
                        max_power: [100.0, 0.0],
                        frequency_hz: Some(5000),
                        air_assist: false,
                        ..Default::default()
                    },
                    ops: square,
                },
                RdLayer { settings: LayerSettings { index: 1, color: 0x0000FF, ..Default::default() }, ops: line },
            ],
        }
    }

    #[test]
    fn a_written_job_reads_back_as_paths_in_layer_colours() {
        let bytes = job().to_rd_file(Magic::DEFAULT).unwrap();
        let imp = import(&bytes).unwrap();
        assert_eq!(imp.items.len(), 2);
        let colours: Vec<_> = imp
            .items
            .iter()
            .map(|i| match &i.kind {
                di::ItemKind::Path(p) => p.stroke.unwrap(),
                _ => panic!(),
            })
            .collect();
        assert_eq!(colours, [di::Rgb(255, 0, 0), di::Rgb(0, 0, 255)]);
        let b = imp.bounds().unwrap();
        assert!((b.width() - 50.0).abs() < 0.01 && (b.height() - 10.0).abs() < 0.01, "{b:?}");
        // The first shape closed on itself.
        let di::ItemKind::Path(p) = &imp.items[0].kind else { panic!() };
        assert!(matches!(p.path.elements().last(), Some(kurbo::PathEl::ClosePath)));
        assert!(imp.warnings.iter().any(|w| w.contains("40 mm/s")));
    }

    #[test]
    fn rd_files_open_through_the_ordinary_import_with_their_layers() {
        let dir = std::env::temp_dir().join(format!("darkburn-rd-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("part.rd");
        std::fs::write(&path, job().to_rd_file(Magic::DEFAULT).unwrap()).unwrap();
        let opts = crate::file_io::read_options(&crate::shell::DarkBurn::new(crate::prefs::Prefs::default()));
        let out = crate::formats::read_file(&path, &opts).unwrap();
        let crate::formats::ReadOutcome::Loaded(l) = out else { panic!("no page choice for rd files") };
        assert_eq!(l.imported.shapes.len(), 2);
        let layers: std::collections::BTreeSet<usize> = l.imported.shapes.iter().map(|s| s.layer).collect();
        assert_eq!(layers.len(), 2, "each Ruida layer becomes its own layer");
        // The cut settings come along with the layers.
        assert_eq!(l.layers.len(), 2);
        let first = l.layers.iter().find(|x| x.index == l.imported.shapes[0].layer).unwrap();
        let m = first.main();
        assert_eq!((m.speed, m.min_power, m.max_power), (40.0, 12.5, 100.0), "full power reads back as 100, not 99.99");
        assert!(m.pwm_override && m.pwm_khz == 5.0 && !first.air);
        let second = l.layers.iter().find(|x| x.index == l.imported.shapes[1].layer).unwrap();
        assert_eq!((second.main().speed, second.main().max_power, second.air), (100.0, 30.0, true));
        assert!(!second.main().pwm_override);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        assert!(import(&[]).is_err());
        let _ = import(&[1, 2, 3, 4, 5, 6, 7, 8]);
    }
}
