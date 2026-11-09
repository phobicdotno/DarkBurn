//! Ruida `.rd` files as artwork (IMP-07): the cut paths of every layer, each layer in its own
//! colour so they land on separate layers of the project. Speeds and powers are listed in the
//! import warnings; they are not copied into the layers.

use darkburn_import as di;
use darkburn_ruida::{RdJob, RdOp};
use kurbo::{BezPath, Point};

/// The extension of Ruida job files.
pub const EXTENSION: &str = "rd";

/// Whether `name` looks like a Ruida job file.
pub fn is_rd(name: &std::path::Path) -> bool {
    name.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case(EXTENSION))
}

/// Read `.rd` bytes as an imported drawing in millimetres, Y up.
pub fn import(bytes: &[u8]) -> Result<di::Imported, String> {
    let job = RdJob::from_rd_file(bytes, None).map_err(|e| format!("this is not a readable Ruida job file: {e}"))?;
    Ok(to_imported(&job))
}

fn to_imported(job: &RdJob) -> di::Imported {
    // The controller's Y grows downwards; the drawing is turned over its own bounds so it stands
    // upright.
    let top = job.bounds_mm().y1;
    let at = |p: Point| Point::new(p.x, top - p.y);
    let mut out = di::Imported::default();
    for layer in &job.layers {
        let s = &layer.settings;
        let mut paths: Vec<BezPath> = Vec::new();
        let mut cur: Option<BezPath> = None;
        let mut here = Point::ZERO;
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
        for path in paths {
            let mut item = di::Item::new(di::ItemKind::Path(di::PathItem { path, stroke: Some(colour), fill: None, dash: None }));
            item.layer = Some(name.clone());
            out.items.push(item);
        }
        out.warn(format!("{name}: {} mm/s, {}% to {}% power (cut settings are not imported)", s.speed_mm_s, s.min_power[0], s.max_power[0]));
    }
    if out.items.is_empty() {
        out.warn("the job holds no cut paths");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use darkburn_ruida::swizzle::Magic;
    use darkburn_ruida::{LayerSettings, RdLayer};

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
            layers: vec![
                RdLayer { settings: LayerSettings { index: 0, color: 0xFF0000, speed_mm_s: 40.0, ..Default::default() }, ops: square },
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
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        assert!(import(&[]).is_err());
        let _ = import(&[1, 2, 3, 4, 5, 6, 7, 8]);
    }
}
