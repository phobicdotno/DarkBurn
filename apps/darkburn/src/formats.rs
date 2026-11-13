//! File formats between the document and the `darkburn-import` library: reading DXF, PDF,
//! Illustrator, PLT/HPGL and LightBurn projects into shapes and layers, and writing the document
//! out as SVG, Illustrator, DXF, bitmaps, LightBurn `.lbrn2` and the Gweike files.
//!
//! Nothing here touches a window, a dialog or the disk: `file_io.rs` does the dialogs and the
//! worker thread, `files.rs` the project files. Everything is plain functions so the same code
//! runs from the menus, from `api::execute_headless` and from the tests.
//!
//! LightBurn projects. Their Y axis points down the screen, DarkBurn's points up. A project is
//! flipped about the height of the work area on the way in and flipped back on the way out, so
//! a round trip returns the same numbers. (Checked against the format notes only; the first real
//! file should be looked at: if designs arrive upside down, [`LBRN_Y_DOWN`] is the switch.)

use std::collections::{BTreeMap, HashMap};
use std::io::Cursor;
use std::path::Path;
use std::sync::Arc;

use darkburn_core::kurbo::{Affine, BezPath, Ellipse, Point, Shape as _, Vec2};
use darkburn_core::layer::{LAYER_COUNT, Layer, Mode, T1, T2, layer_colour};
use darkburn_core::shape::{AlignX, AlignY};
use darkburn_core::{Document, ImageData, ImageShape, Shape, ShapeId, ShapeKind, TextShape};
use darkburn_import as di;
use serde::{Deserialize, Serialize};

use crate::import::{self, Imported, Options};

/// LightBurn files use a Y axis that points down (see the module notes).
pub const LBRN_Y_DOWN: bool = true;
/// Files larger than this are refused instead of being read into memory.
pub const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;

// ----- Preferences -----

/// Length unit for DXF files that carry none.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LengthUnit {
    #[default]
    Mm,
    Cm,
    Metre,
    Inch,
    Foot,
}

impl LengthUnit {
    pub const ALL: [LengthUnit; 5] = [LengthUnit::Mm, LengthUnit::Cm, LengthUnit::Metre, LengthUnit::Inch, LengthUnit::Foot];

    pub fn mm(self) -> f64 {
        match self {
            LengthUnit::Mm => 1.0,
            LengthUnit::Cm => 10.0,
            LengthUnit::Metre => 1000.0,
            LengthUnit::Inch => 25.4,
            LengthUnit::Foot => 304.8,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            LengthUnit::Mm => "Millimetres",
            LengthUnit::Cm => "Centimetres",
            LengthUnit::Metre => "Metres",
            LengthUnit::Inch => "Inches",
            LengthUnit::Foot => "Feet",
        }
    }
}

/// How a DXF file's units are chosen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum UnitMode {
    /// The file's own units; millimetres when it states none.
    #[default]
    Auto,
    /// The file's own units; the chosen unit when it states none.
    Fallback,
    /// The chosen unit, whatever the file says.
    Forced,
}

impl UnitMode {
    pub const ALL: [UnitMode; 3] = [UnitMode::Auto, UnitMode::Fallback, UnitMode::Forced];

    pub fn label(self) -> &'static str {
        match self {
            UnitMode::Auto => "Use the file's units",
            UnitMode::Fallback => "File's units, else the unit below",
            UnitMode::Forced => "Always the unit below",
        }
    }
}

/// How colours become layers on import.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ColourMap {
    /// Each colour goes on the layer whose colour is closest.
    #[default]
    Closest,
    /// Each different colour gets a layer of its own, in order of appearance.
    Unique,
}

impl ColourMap {
    pub const ALL: [ColourMap; 2] = [ColourMap::Closest, ColourMap::Unique];

    pub fn label(self) -> &'static str {
        match self {
            ColourMap::Closest => "Closest layer colour",
            ColourMap::Unique => "A layer for each colour",
        }
    }
}

/// File and format preferences (kept in [`crate::prefs::Prefs`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FilePrefs {
    pub dxf_units: UnitMode,
    pub dxf_unit: LengthUnit,
    /// Join open DXF paths whose ends are this close.
    pub auto_join: bool,
    pub auto_join_tolerance: f32,
    /// Illustrator layers that are switched off are imported too.
    pub hidden_ai_layers: bool,
    pub colour_map: ColourMap,
    /// Plotter units per millimetre in PLT files.
    pub hpgl_units_per_mm: f32,
    /// DXF export writes arcs (else straight segments).
    pub dxf_arcs: bool,
    /// DXF export puts each DarkBurn layer on a DXF layer (else each colour).
    pub dxf_layer_names: bool,
    /// The extension last used in Export.
    pub export_ext: String,
    /// The folder last exported to.
    pub export_dir: Option<String>,
    /// Export only the selection.
    pub export_selection: bool,
    /// Opening a file or a New Window while DarkBurn runs hands it to the running copy.
    pub single_instance: bool,
}

impl Default for FilePrefs {
    fn default() -> Self {
        FilePrefs {
            dxf_units: UnitMode::Auto,
            dxf_unit: LengthUnit::Mm,
            auto_join: false,
            auto_join_tolerance: 0.05,
            hidden_ai_layers: false,
            colour_map: ColourMap::Closest,
            hpgl_units_per_mm: 40.0,
            dxf_arcs: true,
            dxf_layer_names: false,
            export_ext: "svg".into(),
            export_dir: None,
            export_selection: false,
            single_instance: false,
        }
    }
}

impl FilePrefs {
    /// Keep values in range after loading.
    pub fn sanitise(&mut self) {
        if !(self.auto_join_tolerance.is_finite() && (0.0..=10.0).contains(&self.auto_join_tolerance)) {
            self.auto_join_tolerance = 0.05;
        }
        if !(self.hpgl_units_per_mm.is_finite() && (1.0..=10_000.0).contains(&self.hpgl_units_per_mm)) {
            self.hpgl_units_per_mm = 40.0;
        }
        if ExportFormat::from_ext(&self.export_ext).is_none() {
            self.export_ext = "svg".into();
        }
    }

    /// The import library's options for these preferences.
    pub fn library_options(&self, pages: di::pdf_import::PageSelection) -> di::ImportOptions {
        let unit = self.dxf_unit.mm();
        di::ImportOptions {
            dxf: di::dxf_import::DxfOptions {
                fallback_mm_per_unit: if self.dxf_units == UnitMode::Fallback { unit } else { 1.0 },
                force_mm_per_unit: (self.dxf_units == UnitMode::Forced).then_some(unit),
                auto_join_tolerance: self.auto_join.then_some(self.auto_join_tolerance as f64).filter(|t| *t > 0.0),
                group_blocks: true,
            },
            pdf: di::pdf_import::PdfOptions { pages, include_hidden_layers: self.hidden_ai_layers, include_images: true },
            hpgl: di::hpgl::HpglOptions { units_per_mm: self.hpgl_units_per_mm as f64 },
        }
    }
}

// ----- Reading -----

/// Which PDF page(s) to import.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageChoice {
    Page(usize),
    All,
}

/// Everything a reader needs, cloneable onto a worker thread.
#[derive(Clone, Debug)]
pub struct ReadOptions {
    pub art: Options,
    pub files: FilePrefs,
    pub pdf_page: Option<PageChoice>,
}

impl ReadOptions {
    pub fn new(art: Options, files: FilePrefs) -> ReadOptions {
        ReadOptions { art, files, pdf_page: None }
    }
}

/// What was read from one file.
#[derive(Clone, Debug, Default)]
pub struct Loaded {
    /// Shapes and images, in document coordinates, not yet placed.
    pub imported: Imported,
    /// Layers carried by the file (LightBurn cut settings).
    pub layers: Vec<Layer>,
    /// Output priority per layer index, lower first.
    pub priorities: BTreeMap<usize, i64>,
    /// Project notes and whether they show on opening.
    pub notes: Option<(String, bool)>,
    pub warnings: Vec<String>,
    /// The file is a whole project (LightBurn), not artwork.
    pub project: bool,
}

/// The result of reading a file.
#[derive(Clone, Debug)]
pub enum ReadOutcome {
    Loaded(Box<Loaded>),
    /// A PDF with several pages and no page chosen yet.
    NeedsPage(usize),
}

/// File extensions the Open and Import dialogs offer besides the SVG and bitmap ones.
pub const LIBRARY_EXTENSIONS: &[&str] = &["dxf", "pdf", "ai", "plt", "hpgl", "hgl", "hpg", "lbrn", "lbrn2", "rd"];

/// Whether `path` names a LightBurn project.
pub fn is_lightburn(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "lbrn" | "lbrn2"))
}

/// Whether `path` names a device profile (handled by the devices code).
pub fn is_device_profile(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case(crate::prefs_files::DEVICE_PROFILE_EXT))
}

/// Reads a file from disk. Never panics: a failure inside a parser comes back as an error.
pub fn read_file(path: &Path, opts: &ReadOptions) -> Result<ReadOutcome, String> {
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if meta.len() > MAX_FILE_BYTES {
        return Err("the file is too large to open".into());
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    read_bytes(path, &bytes, opts)
}

/// Reads file contents. `name` picks the format by its extension, then by content.
pub fn read_bytes(name: &Path, bytes: &[u8], opts: &ReadOptions) -> Result<ReadOutcome, String> {
    let name_text = name.file_name().map(|n| n.to_string_lossy().into_owned());
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| read_inner(name, name_text.as_deref(), bytes, opts)));
    match result {
        Ok(r) => r,
        Err(_) => Err("the file could not be read (internal error)".into()),
    }
}

fn read_inner(name: &Path, name_text: Option<&str>, bytes: &[u8], opts: &ReadOptions) -> Result<ReadOutcome, String> {
    if crate::rd_import::is_rd(name) {
        let (imported, cuts) = crate::rd_import::import_with_settings(bytes)?;
        let items = imported.items.len();
        let mut loaded = convert(imported, opts, false);
        crate::rd_import::carry_cut_settings(&mut loaded, &cuts, items);
        if loaded.imported.shapes.is_empty() {
            return Err("there is nothing in it to import".into());
        }
        return Ok(ReadOutcome::Loaded(Box::new(loaded)));
    }
    let by_ext = name.extension().and_then(|e| e.to_str()).and_then(di::Format::from_extension);
    let format = match (by_ext, import::kind_of(name)) {
        (Some(f), _) => Some(f),
        (None, import::FileKind::Other) => di::detect_format(name_text, bytes),
        _ => None,
    };
    let Some(format) = format else {
        // SVG and bitmaps.
        let imp = import::import_bytes(name, bytes, &opts.art, None)?;
        return Ok(ReadOutcome::Loaded(Box::new(Loaded { imported: imp, ..Default::default() })));
    };
    let mut pages = di::pdf_import::PageSelection::Page(1);
    if matches!(format, di::Format::Pdf | di::Format::Ai) {
        match opts.pdf_page {
            Some(PageChoice::Page(p)) => pages = di::pdf_import::PageSelection::Page(p.max(1)),
            Some(PageChoice::All) => pages = di::pdf_import::PageSelection::All,
            None => {
                let count = di::pdf_import::page_count(bytes).map_err(|e| e.to_string())?;
                if count > 1 {
                    return Ok(ReadOutcome::NeedsPage(count));
                }
            }
        }
    }
    let lib = opts.files.library_options(pages);
    // Editors on Windows sometimes put a byte order mark in front of text files.
    let bytes = if format == di::Format::Pdf || format == di::Format::Ai { bytes } else { bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes) };
    let imported = di::import(format, bytes, &lib).map_err(|e| e.to_string())?;
    let loaded = convert(imported, opts, format == di::Format::LightBurn);
    if loaded.imported.shapes.is_empty() && !loaded.project {
        let mut why = "there is nothing in it to import".to_string();
        if let Some(w) = loaded.warnings.first() {
            why.push_str(&format!(" ({w})"));
        }
        return Err(why);
    }
    Ok(ReadOutcome::Loaded(Box::new(loaded)))
}

// ----- Library model to shapes -----

/// The LightBurn y-flip, applied to items on the way in and out.
fn lightburn_flip(items: &mut [di::Item], page_height: f64) {
    if !LBRN_Y_DOWN {
        return;
    }
    let flip = Affine::new([1.0, 0.0, 0.0, -1.0, 0.0, page_height]);
    let v = Affine::new([1.0, 0.0, 0.0, -1.0, 0.0, 1.0]);
    for item in items {
        item.transform(flip);
        if let di::ItemKind::Image(i) = &mut item.kind {
            i.transform *= v;
        }
    }
}

struct Converter<'a> {
    opts: &'a ReadOptions,
    keep_tools: bool,
    colours: HashMap<di::Rgb, usize>,
    used: [bool; LAYER_COUNT],
    out: Imported,
    warnings: Vec<String>,
    text_without_outline: usize,
}

impl Converter<'_> {
    fn colour_layer(&mut self, c: di::Rgb) -> usize {
        if let Some(l) = self.colours.get(&c) {
            return *l;
        }
        let rgb = [c.0, c.1, c.2];
        let layer = match self.opts.files.colour_map {
            ColourMap::Closest => import::layer_for(rgb, self.opts.art.tool_layers),
            ColourMap::Unique => {
                let free = (0..LAYER_COUNT).filter(|i| self.opts.art.tool_layers || (*i != T1 && *i != T2)).find(|i| !self.used[*i]);
                free.unwrap_or_else(|| import::layer_for(rgb, self.opts.art.tool_layers))
            }
        };
        self.used[layer] = true;
        self.colours.insert(c, layer);
        layer
    }

    fn layer_of(&mut self, item: &di::Item) -> usize {
        if let Some(i) = item.layer_index {
            let i = i as usize;
            if i >= LAYER_COUNT {
                self.warn("Some shapes were on layers above 31 and were put on layer 29");
                return 29;
            }
            self.used[i] = true;
            return if self.keep_tools { i } else { import::allowed_layer(i, false) };
        }
        match item.colour() {
            Some(c) => self.colour_layer(c),
            None => import::allowed_layer(self.opts.art.image_layer, self.opts.art.tool_layers),
        }
    }

    fn warn(&mut self, msg: &str) {
        if !self.warnings.iter().any(|w| w == msg) {
            self.warnings.push(msg.to_string());
        }
    }

    fn shape(&mut self, item: &di::Item) -> Option<Shape> {
        let layer = self.layer_of(item);
        Some(match &item.kind {
            di::ItemKind::Path(p) => {
                if p.path.elements().is_empty() {
                    return None;
                }
                Shape::path(p.path.clone(), layer)
            }
            di::ItemKind::Text(t) => {
                let outline = t.outline.clone().filter(|o| !o.elements().is_empty());
                let mut text = TextShape {
                    text: t.text.clone(),
                    font: t.font.clone(),
                    height: if t.height.is_finite() && t.height > 0.0 { t.height } else { 5.0 },
                    align_x: match t.h_align {
                        di::HAlign::Left => AlignX::Left,
                        di::HAlign::Center => AlignX::Middle,
                        di::HAlign::Right => AlignX::Right,
                    },
                    align_y: match t.v_align {
                        di::VAlign::Baseline => AlignY::Baseline,
                        di::VAlign::Bottom => AlignY::Bottom,
                        di::VAlign::Middle => AlignY::Middle,
                        di::VAlign::Top => AlignY::Top,
                    },
                    ..Default::default()
                };
                // The outline is in drawing coordinates; a text keeps its letters in its own, so
                // that it stays where it is when it is rendered again.
                let local = outline.as_ref().and_then(|o| {
                    let mut l = o.clone();
                    l.apply_affine(t.transform.inverse());
                    (t.transform.determinant().abs() > 1e-12 && l.is_finite()).then_some(l)
                });
                let transform = if let Some(l) = local {
                    text.backup = Some(l);
                    // The file's letters hold what the file cannot say (bold, spacing, a path to
                    // follow): they stay until the text is changed.
                    crate::text_render::keep_letters(&mut text);
                    t.transform
                } else if outline.is_some() {
                    text.backup = outline;
                    Affine::IDENTITY
                } else {
                    self.text_without_outline += 1;
                    t.transform
                };
                Shape::new(ShapeKind::Text(text), transform, layer)
            }
            di::ItemKind::Image(i) => {
                let c = i.transform.as_coeffs();
                let (w, h) = (c[0].hypot(c[1]), c[2].hypot(c[3]));
                if !(w.is_finite() && h.is_finite()) || w <= 0.0 || h <= 0.0 || i.transform.determinant().abs() < 1e-12 {
                    return None;
                }
                let mut png = Cursor::new(Vec::new());
                if i.image.write_to(&mut png, image::ImageFormat::Png).is_err() {
                    self.warn("An image could not be kept and was skipped");
                    return None;
                }
                let bytes = png.into_inner();
                let key = ImageData::key_for(&bytes);
                let (px_width, px_height) = (i.image.width(), i.image.height());
                self.out
                    .images
                    .entry(key.clone())
                    .or_insert_with(|| Arc::new(ImageData { format: "png".into(), bytes: Arc::from(bytes), px_width, px_height }));
                let local = Affine::new([w, 0.0, 0.0, h, -w / 2.0, -h / 2.0]);
                Shape::new(
                    ShapeKind::Image(ImageShape { image: key, width: w, height: h, ..Default::default() }),
                    i.transform * local.inverse(),
                    layer,
                )
            }
        })
    }
}

#[derive(Clone, Copy)]
enum Node {
    Group(u32),
    Item(usize),
}

fn build(c: &mut Converter, imp: &di::Imported, tree: &HashMap<Option<u32>, Vec<Node>>, at: Option<u32>, depth: usize) -> Vec<Shape> {
    let mut out = Vec::new();
    let Some(list) = tree.get(&at) else { return out };
    for node in list {
        match *node {
            Node::Item(i) => out.extend(c.shape(&imp.items[i])),
            Node::Group(g) if depth < 64 => {
                let kids = build(c, imp, tree, Some(g), depth + 1);
                if kids.is_empty() {
                    continue;
                }
                let layer = kids[0].layer;
                let mut shape = Shape::new(ShapeKind::Group { children: kids }, Affine::IDENTITY, layer);
                shape.name = imp.groups.iter().find(|x| x.id == g).and_then(|x| x.name.clone()).unwrap_or_default();
                out.push(shape);
            }
            Node::Group(_) => {}
        }
    }
    out
}

/// Turns the library's model into shapes, layers and notes.
fn convert(mut imp: di::Imported, opts: &ReadOptions, lightburn: bool) -> Loaded {
    if lightburn {
        lightburn_flip(&mut imp.items, opts.art.page.height);
    }
    let mut c = Converter {
        opts,
        keep_tools: lightburn,
        colours: HashMap::new(),
        used: [false; LAYER_COUNT],
        out: Imported::default(),
        warnings: imp.warnings.clone(),
        text_without_outline: 0,
    };
    let known: HashMap<u32, Option<u32>> = imp.groups.iter().map(|g| (g.id, g.parent)).collect();
    let parent_of = |id: u32| known.get(&id).copied().flatten().filter(|p| known.contains_key(p));
    let mut children: HashMap<Option<u32>, Vec<Node>> = HashMap::new();
    let mut seen = std::collections::HashSet::new();
    for (i, item) in imp.items.iter().enumerate() {
        let mut g = item.group.filter(|g| known.contains_key(g));
        let mut chain = Vec::new();
        while let Some(id) = g {
            if chain.len() > 64 || !seen.insert(id) {
                break;
            }
            chain.push(id);
            g = parent_of(id);
        }
        for id in chain.into_iter().rev() {
            children.entry(parent_of(id)).or_default().push(Node::Group(id));
        }
        children.entry(item.group.filter(|g| known.contains_key(g))).or_default().push(Node::Item(i));
    }
    c.out.shapes = build(&mut c, &imp, &children, None, 0);
    if c.text_without_outline > 0 {
        let n = c.text_without_outline;
        c.warnings.push(format!("{n} text object(s) came without an outline and show as boxes until their font is available"));
    }
    let mut loaded =
        Loaded { imported: std::mem::take(&mut c.out), warnings: std::mem::take(&mut c.warnings), project: lightburn, ..Default::default() };
    for ls in &imp.layer_settings {
        let idx = ls.index as usize;
        if idx >= LAYER_COUNT || loaded.layers.iter().any(|l| l.index == idx) {
            continue;
        }
        loaded.layers.push(layer_from_settings(ls));
        if let Some(p) = ls.priority() {
            loaded.priorities.insert(idx, p);
        }
    }
    if let Some(p) = &imp.project {
        if let Some(n) = &p.notes {
            loaded.notes = Some((n.clone(), p.show_notes_on_load));
        }
        if !p.variable_text.is_empty() {
            loaded.warnings.push("The project's variable text settings were not carried over".into());
        }
    }
    loaded
}

fn flag(v: Option<&str>, default: bool) -> bool {
    match v.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        Some("1" | "true" | "yes") => true,
        Some("0" | "false" | "no") => false,
        _ => default,
    }
}

/// A DarkBurn layer from a LightBurn cut setting.
pub fn layer_from_settings(ls: &di::LayerSettings) -> Layer {
    let idx = (ls.index as usize).min(LAYER_COUNT - 1);
    let mut layer = Layer::new(idx);
    layer.name = ls.name.clone();
    let main = layer.main_mut();
    main.mode = match ls.mode.as_str() {
        "Scan" => Mode::Fill,
        "Scan+Cut" | "ScanCut" => Mode::FillLine,
        "Image" => Mode::Image,
        "Offset" => Mode::OffsetFill,
        _ => Mode::Line,
    };
    if let Some(v) = ls.speed() {
        main.speed = v;
    }
    if let Some(v) = ls.max_power() {
        main.max_power = v;
    }
    if let Some(v) = ls.min_power() {
        main.min_power = v;
    }
    if let Some(v) = ls.passes() {
        main.passes = v.max(1);
    }
    if let Some(v) = ls.interval() {
        main.interval = v;
    }
    if let Some(v) = ls.angle() {
        main.scan_angle = v;
    }
    if let Some(v) = ls.get_f64("kerf") {
        main.kerf = v;
    }
    if let Some(v) = ls.get_f64("zOffset") {
        main.z_offset = v;
    }
    main.bidirectional = flag(ls.get("bidir"), main.bidirectional);
    main.cross_hatch = flag(ls.get("crossHatch"), main.cross_hatch);
    main.sanitise();
    layer.output = flag(ls.get("doOutput"), true) && !layer.is_tool();
    layer.show = !flag(ls.get("hide"), false);
    layer.air = flag(ls.get("enableAirAssist").or(ls.get("airAssist")), layer.air);
    layer
}

/// A new document holding a loaded project (File > Open of a LightBurn file).
pub fn project_document(loaded: Loaded) -> Document {
    let mut doc = Document::new();
    apply_layers(&mut doc, &loaded.layers, &loaded.priorities, |_| true);
    if let Some((notes, show)) = loaded.notes {
        doc.notes = notes;
        doc.show_notes_on_load = show;
    }
    for (k, data) in loaded.imported.images {
        doc.images.entry(k).or_insert(data);
    }
    for s in loaded.imported.shapes {
        doc.add(s);
    }
    doc
}

/// Puts file layers into `doc` where `take(index)` says so, and orders them by priority.
pub fn apply_layers(doc: &mut Document, layers: &[Layer], priorities: &BTreeMap<usize, i64>, take: impl Fn(usize) -> bool) {
    for l in layers {
        if take(l.index) {
            doc.layers[l.index] = l.clone();
        }
    }
    if !priorities.is_empty() {
        let key = |i: &usize| (priorities.get(i).copied().unwrap_or(i64::MAX), *i);
        let mut order = doc.layer_order.clone();
        order.sort_by_key(key);
        doc.layer_order = order;
    }
}

// ----- Writing -----

/// A file Export can write.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ExportFormat {
    Svg,
    Ai,
    Dxf,
    Png,
    Jpg,
    Bmp,
    /// LightBurn `.lbrn2`, for sharing a design with other software.
    Lbrn2,
    GweikeDxf,
    GweikeNc,
}

impl ExportFormat {
    pub fn ext(self) -> &'static str {
        match self {
            ExportFormat::Svg => "svg",
            ExportFormat::Ai => "ai",
            ExportFormat::Dxf | ExportFormat::GweikeDxf => "dxf",
            ExportFormat::Png => "png",
            ExportFormat::Jpg => "jpg",
            ExportFormat::Bmp => "bmp",
            ExportFormat::Lbrn2 => "lbrn2",
            ExportFormat::GweikeNc => "nc",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ExportFormat::Svg => "SVG (.svg)",
            ExportFormat::Ai => "Adobe Illustrator (.ai)",
            ExportFormat::Dxf => "DXF (.dxf)",
            ExportFormat::Png => "PNG picture (.png)",
            ExportFormat::Jpg => "JPEG picture (.jpg)",
            ExportFormat::Bmp => "BMP picture (.bmp)",
            ExportFormat::Lbrn2 => "LightBurn project for other software (.lbrn2)",
            ExportFormat::GweikeDxf => "Gweike DXF, one layer per material (.dxf)",
            ExportFormat::GweikeNc => "Gweike G-code (.nc, .cnc, .g, .txt)",
        }
    }

    /// The file name extensions the save dialog offers. The Gweike software reads G-code from
    /// `.nc`, `.cnc`, `.g` and `.txt` files (MCC-03).
    pub fn extensions(self) -> &'static [&'static str] {
        match self {
            ExportFormat::GweikeNc => &["nc", "cnc", "g", "txt"],
            ExportFormat::Svg => &["svg"],
            ExportFormat::Ai => &["ai"],
            ExportFormat::Dxf | ExportFormat::GweikeDxf => &["dxf"],
            ExportFormat::Png => &["png"],
            ExportFormat::Jpg => &["jpg", "jpeg"],
            ExportFormat::Bmp => &["bmp"],
            ExportFormat::Lbrn2 => &["lbrn2"],
        }
    }

    pub fn is_bitmap(self) -> bool {
        matches!(self, ExportFormat::Png | ExportFormat::Jpg | ExportFormat::Bmp)
    }

    pub fn is_gweike(self) -> bool {
        matches!(self, ExportFormat::GweikeDxf | ExportFormat::GweikeNc)
    }

    /// The format a file extension means for plain Export (not the Gweike DXF).
    pub fn from_ext(ext: &str) -> Option<ExportFormat> {
        Some(match ext.to_ascii_lowercase().as_str() {
            "svg" => ExportFormat::Svg,
            "ai" => ExportFormat::Ai,
            "dxf" => ExportFormat::Dxf,
            "png" => ExportFormat::Png,
            "jpg" | "jpeg" => ExportFormat::Jpg,
            "bmp" => ExportFormat::Bmp,
            "lbrn2" => ExportFormat::Lbrn2,
            "nc" | "cnc" | "g" => ExportFormat::GweikeNc,
            _ => return None,
        })
    }

    /// The format named in a command's arguments.
    pub fn from_name(name: &str) -> Option<ExportFormat> {
        Some(match name.to_ascii_lowercase().as_str() {
            "gweike_dxf" => ExportFormat::GweikeDxf,
            "gweike_nc" => ExportFormat::GweikeNc,
            other => return ExportFormat::from_ext(other),
        })
    }
}

/// What to export and how.
#[derive(Clone, Debug)]
pub struct ExportRequest {
    pub format: ExportFormat,
    /// Only these shapes; everything when `None`.
    pub selection: Option<Vec<ShapeId>>,
    pub dxf_arcs: bool,
    pub dxf_layer_names: bool,
    /// SVG page is the whole work area (else the artwork's bounds).
    pub svg_work_area: bool,
    /// Work area size in mm (SVG page, LightBurn flip).
    pub page: (f64, f64),
    /// Gweike: material per DarkBurn layer name.
    pub materials: BTreeMap<String, String>,
    pub default_material: Option<String>,
    /// Gweike `.nc` cut on and cut off codes.
    pub nc_on: String,
    pub nc_off: String,
}

impl ExportRequest {
    pub fn new(format: ExportFormat, page: (f64, f64)) -> ExportRequest {
        ExportRequest {
            format,
            selection: None,
            dxf_arcs: true,
            dxf_layer_names: false,
            svg_work_area: false,
            page,
            materials: BTreeMap::new(),
            default_material: None,
            nc_on: "M07".into(),
            nc_off: "M08".into(),
        }
    }
}

/// A written file and anything that could not be written.
#[derive(Clone, Debug, Default)]
pub struct ExportOutput {
    pub bytes: Vec<u8>,
    pub warnings: Vec<String>,
}

/// Hook: the document as it is written to a file. Variable text (serial numbers, dates, merged
/// CSV rows) is turned into plain text here once the text code is wired in; until then the
/// document is written as it is.
pub fn prepare_for_output(doc: &Document) -> Document {
    crate::text_render::prepare_for_output(doc)
}

fn rgb_of(layer: usize) -> di::Rgb {
    let [r, g, b] = layer_colour(layer);
    di::Rgb(r, g, b)
}

struct Collect<'a> {
    doc: &'a Document,
    items: Vec<di::Item>,
    groups: Vec<di::Group>,
    warnings: Vec<String>,
    text_as_paths: bool,
    skipped_text: usize,
}

impl Collect<'_> {
    fn push(&mut self, mut item: di::Item, layer: usize, group: Option<u32>) {
        item.layer = Some(self.doc.layer(layer).name.clone());
        item.layer_index = Some(layer as u32);
        item.group = group;
        self.items.push(item);
    }

    fn shape(&mut self, shape: &Shape, parent: Affine, group: Option<u32>, depth: usize) {
        match &shape.kind {
            ShapeKind::Group { children } => {
                if depth > 64 {
                    return;
                }
                let id = self.groups.len() as u32 + 1;
                self.groups.push(di::Group { id, parent: group, name: (!shape.name.is_empty()).then(|| shape.name.clone()) });
                for c in children {
                    self.shape(c, parent * shape.transform, Some(id), depth + 1);
                }
            }
            ShapeKind::Image(img) => {
                let Some(data) = self.doc.images.get(&img.image) else { return };
                let Ok(decoded) = image::load_from_memory(&data.bytes) else {
                    self.warnings.push("An image could not be read back and was left out".into());
                    return;
                };
                let local = Affine::new([img.width, 0.0, 0.0, img.height, -img.width / 2.0, -img.height / 2.0]);
                let transform = parent * shape.transform * local;
                let item = di::Item::new(di::ItemKind::Image(di::ImageItem { image: decoded, transform, attributes: BTreeMap::new() }));
                self.push(item, shape.layer, group);
            }
            ShapeKind::Text(t) => {
                let mut s = shape.clone();
                s.transform = parent * s.transform;
                let outline = t.backup.as_ref().map(|_| s.outline());
                if self.text_as_paths {
                    match outline {
                        Some(o) if !o.elements().is_empty() => {
                            let item = di::Item::path(o, Some(rgb_of(shape.layer)), None);
                            self.push(item, shape.layer, group);
                        }
                        _ => self.skipped_text += 1,
                    }
                    return;
                }
                let item = di::Item::new(di::ItemKind::Text(di::TextItem {
                    text: t.text.clone(),
                    font: t.font.clone(),
                    style: None,
                    height: t.height,
                    h_align: match t.align_x {
                        AlignX::Left => di::HAlign::Left,
                        AlignX::Middle => di::HAlign::Center,
                        AlignX::Right => di::HAlign::Right,
                    },
                    v_align: match t.align_y {
                        AlignY::Baseline => di::VAlign::Baseline,
                        AlignY::Top => di::VAlign::Top,
                        AlignY::Middle => di::VAlign::Middle,
                        AlignY::Bottom => di::VAlign::Bottom,
                    },
                    transform: s.transform,
                    colour: rgb_of(shape.layer),
                    outline,
                    attributes: BTreeMap::new(),
                }));
                self.push(item, shape.layer, group);
            }
            _ => {
                let mut s = shape.clone();
                s.transform = parent * s.transform;
                let outline = match &s.kind {
                    // Ellipses are written finely enough for circles to be recognised as circles.
                    ShapeKind::Ellipse { rx, ry } => {
                        let mut p: BezPath = Ellipse::new(Point::ZERO, Vec2::new(*rx, *ry), 0.0).path_elements(0.0005).collect();
                        if !matches!(p.elements().last(), Some(darkburn_core::kurbo::PathEl::ClosePath)) {
                            p.close_path();
                        }
                        p.apply_affine(s.transform);
                        p
                    }
                    _ => s.outline(),
                };
                if outline.elements().is_empty() {
                    return;
                }
                let item = di::Item::path(outline, Some(rgb_of(shape.layer)), None);
                self.push(item, shape.layer, group);
            }
        }
    }
}

/// The document's shapes (or only `ids`) as library items, with their groups and any warnings.
pub fn doc_items(doc: &Document, ids: Option<&[ShapeId]>, text_as_paths: bool) -> (Vec<di::Item>, Vec<di::Group>, Vec<String>) {
    let mut c = Collect { doc, items: Vec::new(), groups: Vec::new(), warnings: Vec::new(), text_as_paths, skipped_text: 0 };
    for s in doc.iter() {
        if ids.is_some_and(|ids| !ids.contains(&s.id)) {
            continue;
        }
        c.shape(s, Affine::IDENTITY, None, 0);
    }
    if c.skipped_text > 0 {
        c.warnings.push(format!("{} text object(s) have no outline yet and were left out", c.skipped_text));
    }
    (c.items, c.groups, c.warnings)
}

fn mode_name(m: Mode) -> &'static str {
    match m {
        Mode::Line => "Cut",
        Mode::Fill => "Scan",
        Mode::FillLine => "Scan+Cut",
        Mode::Image => "Image",
        Mode::OffsetFill => "Offset",
    }
}

fn layer_settings_for(doc: &Document, used: &[usize]) -> Vec<di::LayerSettings> {
    used.iter()
        .map(|&i| {
            let l = doc.layer(i);
            let m = l.main();
            let mut values = BTreeMap::new();
            let n = |v: f64| format!("{v}");
            values.insert("speed".to_string(), n(m.speed));
            values.insert("maxPower".to_string(), n(m.max_power));
            values.insert("minPower".to_string(), n(m.min_power));
            values.insert("numPasses".to_string(), m.passes.to_string());
            values.insert("interval".to_string(), n(m.interval));
            values.insert("angle".to_string(), n(m.scan_angle));
            values.insert("bidir".to_string(), u8::from(m.bidirectional).to_string());
            values.insert("crossHatch".to_string(), u8::from(m.cross_hatch).to_string());
            values.insert("kerf".to_string(), n(m.kerf));
            values.insert("zOffset".to_string(), n(m.z_offset));
            values.insert("enableAirAssist".to_string(), u8::from(l.air).to_string());
            values.insert("doOutput".to_string(), u8::from(l.output).to_string());
            values.insert("hide".to_string(), u8::from(!l.show).to_string());
            let prio = doc.layer_order.iter().position(|x| *x == i).unwrap_or(i);
            values.insert("priority".to_string(), prio.to_string());
            di::LayerSettings { index: i as u32, name: l.name.clone(), mode: mode_name(m.mode).to_string(), values }
        })
        .collect()
}

/// Writes the document as `req.format` says.
pub fn export(doc: &Document, req: &ExportRequest) -> Result<ExportOutput, String> {
    let doc = prepare_for_output(doc);
    let ids = req.selection.as_deref();
    if req.format.is_bitmap() {
        return export_bitmap(&doc, req);
    }
    let text_as_paths = !matches!(req.format, ExportFormat::Lbrn2);
    let (mut items, groups, mut warnings) = doc_items(&doc, ids, text_as_paths);
    if items.is_empty() {
        return Err(if ids.is_some() { "the selection has nothing to export".into() } else { "there is nothing to export".into() });
    }
    let bytes = match req.format {
        ExportFormat::Svg => {
            let origin = if req.svg_work_area {
                di::svg_export::SvgOrigin::Page { width: req.page.0, height: req.page.1 }
            } else {
                di::svg_export::SvgOrigin::Bounds
            };
            di::svg_export::export(&items, &di::svg_export::SvgExportOptions { origin, ..Default::default() }).into_bytes()
        }
        ExportFormat::Ai => {
            let out = di::ai_export::export(&items, &di::ai_export::AiExportOptions::default());
            warnings.extend(out.warnings);
            out.bytes
        }
        ExportFormat::Dxf => {
            let opts = di::dxf_export::DxfExportOptions {
                curves: if req.dxf_arcs { di::dxf_export::CurveMode::Arcs } else { di::dxf_export::CurveMode::Segments },
                layers: if req.dxf_layer_names { di::dxf_export::LayerNaming::ByItemLayer } else { di::dxf_export::LayerNaming::ByColour },
                circles: req.dxf_arcs,
                ..Default::default()
            };
            let out = di::dxf_export::export(&items, &opts);
            warnings.extend(out.warnings);
            out.bytes
        }
        ExportFormat::GweikeDxf => {
            let out = di::gweike::export_dxf_per_material(&items, &gweike_options(req));
            warnings.extend(out.warnings);
            out.bytes
        }
        ExportFormat::GweikeNc => {
            let opts = di::gweike::NcOptions {
                cut_on: req.nc_on.clone(),
                cut_off: req.nc_off.clone(),
                materials: gweike_options(req),
                ..Default::default()
            };
            di::gweike::export_nc(&items, &opts).into_bytes()
        }
        ExportFormat::Lbrn2 => {
            lightburn_flip(&mut items, req.page.1);
            let mut used: Vec<usize> = items.iter().filter_map(|i| i.layer_index.map(|l| l as usize)).collect();
            used.sort_unstable();
            used.dedup();
            let project = di::ProjectInfo {
                notes: (!doc.notes.is_empty()).then(|| doc.notes.clone()),
                show_notes_on_load: doc.show_notes_on_load,
                ..Default::default()
            };
            let model = di::Imported { items, groups, layer_settings: layer_settings_for(&doc, &used), project: Some(project), ..Default::default() };
            di::lbrn::write_lbrn2(&model, &di::lbrn::LbrnWriteOptions::default()).into_bytes()
        }
        ExportFormat::Png | ExportFormat::Jpg | ExportFormat::Bmp => unreachable!("bitmaps are handled above"),
    };
    Ok(ExportOutput { bytes, warnings })
}

fn gweike_options(req: &ExportRequest) -> di::gweike::GweikeDxfOptions {
    di::gweike::GweikeDxfOptions {
        materials: req.materials.clone(),
        default_material: req.default_material.clone().filter(|m| !m.trim().is_empty()),
        ..Default::default()
    }
}

/// The layer names that hold shapes (of the selection, when given), in output order: what the
/// Gweike material mapping lists.
pub fn export_layer_names(doc: &Document, ids: Option<&[ShapeId]>) -> Vec<String> {
    let mut used = std::collections::BTreeSet::new();
    for s in doc.iter().filter(|s| ids.is_none_or(|ids| ids.contains(&s.id))) {
        used.extend(s.layers());
    }
    doc.layer_order.iter().filter(|l| used.contains(l)).map(|l| doc.layer(*l).name.clone()).collect()
}

fn export_bitmap(doc: &Document, req: &ExportRequest) -> Result<ExportOutput, String> {
    let ids = req.selection.as_deref().ok_or("select a bitmap to export it as a picture")?;
    let shape = doc.iter().filter(|s| ids.contains(&s.id)).flat_map(|s| s.leaves()).find(|s| s.is_image()).ok_or("the selection holds no bitmap")?;
    let ShapeKind::Image(img) = &shape.kind else { return Err("the selection holds no bitmap".into()) };
    let data = doc.images.get(&img.image).ok_or("the bitmap's pixels are missing")?;
    let decoded = image::load_from_memory(&data.bytes).map_err(|e| format!("the bitmap could not be read ({e})"))?;
    let (format, decoded) = match req.format {
        ExportFormat::Png => (image::ImageFormat::Png, decoded),
        ExportFormat::Jpg => (image::ImageFormat::Jpeg, image::DynamicImage::ImageRgb8(decoded.to_rgb8())),
        _ => (image::ImageFormat::Bmp, image::DynamicImage::ImageRgba8(decoded.to_rgba8())),
    };
    let mut out = Cursor::new(Vec::new());
    decoded.write_to(&mut out, format).map_err(|e| format!("the picture could not be written ({e})"))?;
    Ok(ExportOutput { bytes: out.into_inner(), warnings: vec![] })
}

/// Whether the selection can be saved as a picture: it holds a bitmap.
pub fn selection_has_bitmap(doc: &Document, ids: &[ShapeId]) -> bool {
    doc.iter().filter(|s| ids.contains(&s.id)).flat_map(|s| s.leaves()).any(|s| s.is_image())
}

/// The artwork as a one-page PDF for printing or saving: black lines only, or in layer colours.
pub fn print_pdf(doc: &Document, ids: Option<&[ShapeId]>, black_only: bool) -> Result<ExportOutput, String> {
    let doc = prepare_for_output(doc);
    let (mut items, _, mut warnings) = doc_items(&doc, ids, true);
    if black_only {
        items.retain(|i| !matches!(i.kind, di::ItemKind::Image(_)));
        for i in &mut items {
            if let di::ItemKind::Path(p) = &mut i.kind {
                p.stroke = Some(di::Rgb::BLACK);
                p.fill = None;
            }
        }
    }
    if items.is_empty() {
        return Err("there is nothing to print".into());
    }
    let out = di::ai_export::export(&items, &di::ai_export::AiExportOptions { stroke_width: 0.15, margin: 5.0 });
    warnings.extend(out.warnings);
    Ok(ExportOutput { bytes: out.bytes, warnings })
}
