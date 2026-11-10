//! DarkBurn's project file (`.dbrn`): a zip holding `project.json` (versioned schema), the
//! embedded images under `images/`, and `thumbnail.png`. Unknown fields are ignored and missing
//! ones take defaults, so older files open in newer versions; a file from a newer DarkBurn opens
//! with a warning.

use std::io::{Cursor, Read, Write};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::document::{Document, ImageData};
use crate::layer::{LAYER_COUNT, Layer};
use crate::shape::Shape;

/// File extension.
pub const EXTENSION: &str = "dbrn";
/// The schema version this build writes.
pub const FORMAT_VERSION: u32 = 3;
const FORMAT_NAME: &str = "darkburn-project";

/// Why a project did not open.
#[derive(Debug)]
pub enum ProjectError {
    NotAProject(String),
    Damaged(String),
    Io(std::io::Error),
}

impl std::fmt::Display for ProjectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProjectError::NotAProject(m) => write!(f, "not a DarkBurn project: {m}"),
            ProjectError::Damaged(m) => write!(f, "the project is damaged: {m}"),
            ProjectError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ProjectError {}

impl From<std::io::Error> for ProjectError {
    fn from(e: std::io::Error) -> Self {
        ProjectError::Io(e)
    }
}

/// A project read from disk.
#[derive(Debug)]
pub struct Loaded {
    pub doc: Document,
    /// The file was written by a newer DarkBurn; some of it may be missing.
    pub newer_version: bool,
    /// Version of the DarkBurn that wrote it.
    pub app_version: String,
    pub thumbnail: Option<Vec<u8>>,
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct ProjectJson {
    format: String,
    version: u32,
    app_version: String,
    notes: String,
    show_notes_on_load: bool,
    layers: Vec<Layer>,
    layer_order: Vec<usize>,
    shapes: Vec<Shape>,
    images: Vec<ImageEntry>,
    next_id: u64,
    /// Versioned job settings (optimization, job origin); the app owns the shape.
    job_settings: Option<serde_json::Value>,
    /// Variable text state (version 2).
    variables: darkburn_text::VariableState,
    /// Camera, rotary, cylinder and print-and-cut setup; the app owns the shape.
    vision_settings: Option<serde_json::Value>,
}

impl Default for ProjectJson {
    fn default() -> Self {
        ProjectJson {
            format: String::new(),
            version: 0,
            app_version: String::new(),
            notes: String::new(),
            show_notes_on_load: false,
            layers: Vec::new(),
            layer_order: Vec::new(),
            shapes: Vec::new(),
            images: Vec::new(),
            next_id: 1,
            job_settings: None,
            variables: Default::default(),
            vision_settings: None,
        }
    }
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct ImageEntry {
    key: String,
    format: String,
    file: String,
    px_width: u32,
    px_height: u32,
}

/// The project as file bytes. `app_version` is recorded; `thumbnail` is a PNG.
pub fn save(doc: &Document, app_version: &str, thumbnail: Option<&[u8]>) -> Result<Vec<u8>, ProjectError> {
    let mut doc = doc.clone();
    doc.prune_images();
    let images: Vec<ImageEntry> = doc
        .images
        .iter()
        .map(|(k, d)| ImageEntry {
            key: k.clone(),
            format: d.format.clone(),
            file: format!("images/{k}.{}", d.format),
            px_width: d.px_width,
            px_height: d.px_height,
        })
        .collect();
    let json = ProjectJson {
        format: FORMAT_NAME.into(),
        version: FORMAT_VERSION,
        app_version: app_version.into(),
        notes: doc.notes.clone(),
        show_notes_on_load: doc.show_notes_on_load,
        layers: doc.layers.clone(),
        layer_order: doc.layer_order.clone(),
        shapes: doc.iter().cloned().collect(),
        images,
        next_id: doc.next_id(),
        job_settings: doc.job_settings.clone(),
        variables: doc.variables.clone(),
        vision_settings: doc.vision_settings.clone(),
    };
    let text = serde_json::to_vec_pretty(&json).map_err(|e| ProjectError::Damaged(e.to_string()))?;
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let deflate = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let stored = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let zerr = |e: zip::result::ZipError| ProjectError::Damaged(e.to_string());
    zip.start_file("project.json", deflate).map_err(zerr)?;
    zip.write_all(&text)?;
    for (k, d) in &doc.images {
        zip.start_file(format!("images/{k}.{}", d.format), stored).map_err(zerr)?;
        zip.write_all(&d.bytes)?;
    }
    if let Some(png) = thumbnail {
        zip.start_file("thumbnail.png", stored).map_err(zerr)?;
        zip.write_all(png)?;
    }
    Ok(zip.finish().map_err(zerr)?.into_inner())
}

/// Read a project from its file bytes.
pub fn load(bytes: &[u8]) -> Result<Loaded, ProjectError> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| ProjectError::NotAProject(e.to_string()))?;
    let mut text = Vec::new();
    zip.by_name("project.json").map_err(|_| ProjectError::NotAProject("no project.json".into()))?.read_to_end(&mut text)?;
    let json: ProjectJson = serde_json::from_slice(&text).map_err(|e| ProjectError::Damaged(e.to_string()))?;
    if json.format != FORMAT_NAME {
        return Err(ProjectError::NotAProject(format!("format is {:?}", json.format)));
    }
    let mut doc = Document::new();
    doc.notes = json.notes;
    doc.show_notes_on_load = json.show_notes_on_load;
    doc.job_settings = json.job_settings;
    doc.variables = json.variables;
    doc.vision_settings = json.vision_settings;
    for mut l in json.layers {
        if l.index < LAYER_COUNT {
            if l.settings.is_empty() {
                l.settings.push(Default::default());
            }
            l.settings.truncate(crate::layer::MAX_SUB_LAYERS);
            for s in &mut l.settings {
                s.sanitise();
            }
            let i = l.index;
            doc.layers[i] = l;
        }
    }
    // Every layer exactly once in the order: keep the saved order, add any missing at the end.
    let mut order: Vec<usize> = Vec::new();
    for l in json.layer_order.into_iter().chain(0..LAYER_COUNT) {
        if l < LAYER_COUNT && !order.contains(&l) {
            order.push(l);
        }
    }
    doc.layer_order = order;
    for entry in json.images {
        let mut data = Vec::new();
        let Ok(mut f) = zip.by_name(&entry.file) else { continue };
        f.read_to_end(&mut data)?;
        doc.images.insert(
            entry.key,
            Arc::new(ImageData { format: entry.format, bytes: Arc::from(data), px_width: entry.px_width, px_height: entry.px_height }),
        );
    }
    doc.shapes = json.shapes.into_iter().map(Arc::new).collect();
    doc.set_next_id(json.next_id);
    let thumbnail = zip.by_name("thumbnail.png").ok().and_then(|mut f| {
        let mut v = Vec::new();
        f.read_to_end(&mut v).ok().map(|_| v)
    });
    Ok(Loaded { doc, newer_version: json.version > FORMAT_VERSION, app_version: json.app_version, thumbnail })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layer::Mode;
    use crate::shape::{ImageShape, ShapeKind, TextShape};
    use kurbo::{Affine, BezPath, Rect};

    fn sample() -> Document {
        let mut d = Document::new();
        d.add(Shape::rect(Rect::new(0.0, 0.0, 20.0, 10.0), -2.0, 3));
        d.add(Shape::ellipse(Rect::new(0.0, 0.0, 20.0, 10.0), 4));
        d.add(Shape::polygon(kurbo::Point::new(5.0, 5.0), 7, 3.0, 5));
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.curve_to((1.0, 2.0), (3.0, 4.0), (5.0, 0.5));
        d.add(Shape::path(p, 0));
        let key = d.add_image(ImageData { format: "png".into(), bytes: Arc::from(&b"\x89PNG fake"[..]), px_width: 4, px_height: 2 });
        let mut img =
            Shape::new(ShapeKind::Image(ImageShape { image: key, width: 40.0, height: 20.0, ..Default::default() }), Affine::rotate(0.3), 6);
        img.name = "photo".into();
        img.power_scale = 80.0;
        d.add(img);
        d.add(Shape::new(ShapeKind::Text(TextShape { text: "Hello".into(), ..Default::default() }), Affine::IDENTITY, 7));
        let g = vec![Shape::rect(Rect::new(0.0, 0.0, 1.0, 1.0), 0.0, 1), Shape::rect(Rect::new(2.0, 0.0, 3.0, 1.0), 0.0, 2)];
        d.add(Shape::new(ShapeKind::Group { children: g }, Affine::translate((9.0, 9.0)), 1));
        d.layer_mut(3).main_mut().mode = Mode::Fill;
        d.layer_mut(3).main_mut().speed = 33.0;
        d.layer_mut(3).add_sub_layer();
        d.move_layer(4, -1);
        d.notes = "Use 3 mm birch".into();
        d.job_settings = Some(serde_json::json!({ "version": 1, "x": 2 }));
        d.vision_settings = Some(serde_json::json!({ "version": 1, "rotary": { "enabled": true } }));
        d.show_notes_on_load = true;
        d
    }

    #[test]
    fn start_and_direction_overrides_round_trip_and_old_files_load() {
        use crate::shape::CutDirection;
        let mut d = Document::new();
        let mut r = Shape::rect(Rect::new(0.0, 0.0, 10.0, 10.0), 0.0, 1);
        r.start_point = Some([5.0, 0.0]);
        r.direction = CutDirection::Clockwise;
        r.extra_start_points = vec![[7.0, 10.0], [0.0, 3.0]];
        r.tabs = vec![[1.0, 2.0]];
        let id = d.add(r);
        // Inside a group too.
        let mut child = Shape::rect(Rect::new(0.0, 0.0, 4.0, 4.0), 0.0, 2);
        child.start_point = Some([2.0, 0.0]);
        child.direction = CutDirection::Reversed;
        child.tabs = vec![[-2.0, 0.0], [2.0, 1.0]];
        let gid = d.add(Shape::new(ShapeKind::Group { children: vec![child.clone()] }, Affine::translate((30.0, 0.0)), 2));
        let bytes = save(&d, "t", None).unwrap();
        let back = load(&bytes).unwrap().doc;
        let s = back.get(id).unwrap();
        assert_eq!(s.start_point, Some([5.0, 0.0]));
        assert_eq!(s.extra_start_points, vec![[7.0, 10.0], [0.0, 3.0]]);
        assert_eq!(s.start_points().collect::<Vec<_>>(), vec![[5.0, 0.0], [7.0, 10.0], [0.0, 3.0]]);
        assert_eq!(s.direction, CutDirection::Clockwise);
        assert_eq!(s.tabs, vec![[1.0, 2.0]]);
        assert_eq!(back.get(id), d.get(id));
        let ShapeKind::Group { children } = &back.get(gid).unwrap().kind else { panic!("a group") };
        assert_eq!(children[0], Shape { id: children[0].id, ..child });
        // A version 2 file has neither field and opens with defaults.
        let mut d2 = Document::new();
        let id2 = d2.add(Shape::rect(Rect::new(0.0, 0.0, 1.0, 1.0), 0.0, 1));
        let b2 = save(&d2, "t", None).unwrap();
        let s2 = load(&b2).unwrap().doc;
        assert!(s2.get(id2).unwrap().start_point.is_none() && s2.get(id2).unwrap().direction == CutDirection::Auto);
    }

    /// A project written before shapes had tabs, start points or a cut direction (format
    /// version 2) still opens, and a shape without them is written exactly as it was then.
    #[test]
    fn a_project_from_before_start_points_and_tabs_opens() {
        let old = r#"{
          "format": "darkburn-project", "version": 2, "app_version": "0.1.0", "next_id": 3,
          "shapes": [
            { "id": 1, "name": "", "kind": { "type": "rect", "width": 20.0, "height": 10.0, "radius": 0.0 },
              "transform": [1.0, 0.0, 0.0, 1.0, 15.0, 25.0], "layer": 1, "locked": false, "priority": 0, "power_scale": 100.0 },
            { "id": 2, "kind": { "type": "group", "children": [
                { "id": 3, "kind": { "type": "ellipse", "rx": 4.0, "ry": 4.0 }, "transform": [1.0, 0.0, 0.0, 1.0, 0.0, 0.0], "layer": 2 } ] },
              "transform": [1.0, 0.0, 0.0, 1.0, 50.0, 50.0], "layer": 2 }
          ]
        }"#;
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        w.start_file("project.json", zip::write::SimpleFileOptions::default()).unwrap();
        w.write_all(old.as_bytes()).unwrap();
        let loaded = load(&w.finish().unwrap().into_inner()).unwrap();
        assert!(!loaded.newer_version);
        let doc = loaded.doc;
        assert_eq!(doc.len(), 2);
        for s in doc.iter().flat_map(|s| s.leaves()) {
            assert!(s.tabs.is_empty() && s.start_point.is_none() && s.extra_start_points.is_empty());
            assert_eq!(s.direction, crate::shape::CutDirection::Auto);
        }
        let r = doc.get(crate::ShapeId(1)).unwrap();
        assert_eq!(r.bbox(), Rect::new(5.0, 20.0, 25.0, 30.0));
        // Saving it again writes none of the new fields.
        let again = save(&doc, "t", None).unwrap();
        let mut zip = zip::ZipArchive::new(Cursor::new(&again[..])).unwrap();
        let mut text = String::new();
        zip.by_name("project.json").unwrap().read_to_string(&mut text).unwrap();
        let json: serde_json::Value = serde_json::from_str(&text).unwrap();
        let rect = json["shapes"][0].as_object().unwrap();
        for key in ["tabs", "start_point", "extra_start_points", "direction"] {
            assert!(!rect.contains_key(key), "{key} is only written when set");
        }
    }

    #[test]
    fn save_and_open_round_trip() {
        let d = sample();
        let bytes = save(&d, "0.1.0", Some(b"thumb")).unwrap();
        let back = load(&bytes).unwrap();
        assert!(!back.newer_version);
        assert_eq!(back.app_version, "0.1.0");
        assert_eq!(back.thumbnail.as_deref(), Some(&b"thumb"[..]));
        assert_eq!(back.doc.notes, d.notes);
        assert_eq!(back.doc.job_settings, d.job_settings);
        assert_eq!(back.doc.vision_settings, d.vision_settings);
        assert!(back.doc.show_notes_on_load);
        assert_eq!(back.doc.layers, d.layers);
        assert_eq!(back.doc.layer_order, d.layer_order);
        assert_eq!(back.doc.images, d.images);
        assert_eq!(back.doc.len(), d.len());
        for (a, b) in back.doc.iter().zip(d.iter()) {
            assert_eq!(a.id, b.id);
            assert_eq!(a.kind_name(), b.kind_name());
            assert_eq!(a.layer, b.layer);
            assert_eq!(a.name, b.name);
            assert!((a.bbox().x0 - b.bbox().x0).abs() < 1e-9 && (a.bbox().y1 - b.bbox().y1).abs() < 1e-9);
        }
        let mut again = back.doc;
        let new_id = again.alloc_id();
        assert!(d.iter().all(|s| s.id != new_id), "ids keep counting after open");
    }

    #[test]
    fn newer_files_open_with_a_warning_and_junk_is_refused() {
        let d = sample();
        let bytes = save(&d, "0.1.0", None).unwrap();
        // Rewrite project.json with a higher version and an unknown field.
        let mut zip = zip::ZipArchive::new(Cursor::new(&bytes[..])).unwrap();
        let mut text = String::new();
        zip.by_name("project.json").unwrap().read_to_string(&mut text).unwrap();
        let text = text.replacen(&format!("\"version\": {FORMAT_VERSION}"), "\"version\": 99, \"future_thing\": [1,2]", 1);
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        w.start_file("project.json", zip::write::SimpleFileOptions::default()).unwrap();
        w.write_all(text.as_bytes()).unwrap();
        let newer = w.finish().unwrap().into_inner();
        let loaded = load(&newer).unwrap();
        assert!(loaded.newer_version);
        assert_eq!(loaded.doc.len(), d.len());
        assert!(matches!(load(b"not a zip"), Err(ProjectError::NotAProject(_))));
    }
}
