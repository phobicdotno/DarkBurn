//! Turns text, QR code and barcode shapes into outlines. The outline of a text shape is kept in
//! its `backup` path and the outline of a code shape in its `cache`, so everything that reads
//! `Shape::outline()` (the canvas, the job builder, the exporters) sees real letters. [`refresh`]
//! renders again every shape whose inputs changed since it was last rendered; the inputs are
//! hashed into the shape's `render_key`. Variable text is evaluated here, so the same call
//! evaluates a document before a job or an export (`prepare_for_output`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, RwLock, RwLockReadGuard};

use darkburn_core::kurbo::{Affine, BezPath, Shape as _};
use darkburn_core::shape::{AlignX, AlignY, VariableText};
use darkburn_core::{CodeShape, CodeSpec, Document, LAYER_COUNT, Shape, ShapeKind, TextShape};
use darkburn_text::{
    CutValues, Direction, EvalContext, FontLibrary, HAlign, LayoutSource, PathTextOptions, ShxFont, TextLayout, TextSpec, VAlign, VarKind,
    VariableState, geomutil, layout_with_backup, text_on_path,
};

/// Prefix of a font name that is an SHX single-line font file.
pub const SHX_PREFIX: &str = "shx:";

fn library() -> &'static RwLock<FontLibrary> {
    static LIB: OnceLock<RwLock<FontLibrary>> = OnceLock::new();
    LIB.get_or_init(|| {
        #[cfg(test)]
        let mut l = FontLibrary::new();
        #[cfg(not(test))]
        let mut l = FontLibrary::with_system_fonts();
        #[cfg(test)]
        {
            let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates/darkburn-text/tests/fonts");
            for f in ["IBMPlexMono-Regular.ttf", "IBMPlexSans-Variable.ttf"] {
                let _ = l.load_font_file(dir.join(f));
            }
            l.set_default_family("IBM Plex Sans");
        }
        #[cfg(not(test))]
        if !l.has_family("Arial") && l.face_count() > 0 {
            let first = l.families().first().cloned().unwrap_or_default();
            l.set_default_family(first);
        }
        RwLock::new(l)
    })
}

/// The shared font library (system fonts, loaded on first use).
pub fn lib() -> RwLockReadGuard<'static, FontLibrary> {
    library().read().unwrap_or_else(|e| e.into_inner())
}

fn shx_cache() -> &'static Mutex<HashMap<String, Option<Arc<ShxFont>>>> {
    static C: OnceLock<Mutex<HashMap<String, Option<Arc<ShxFont>>>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// The SHX font at `path`, read once.
pub fn shx_font(path: &str) -> Option<Arc<ShxFont>> {
    let mut c = shx_cache().lock().unwrap_or_else(|e| e.into_inner());
    c.entry(path.to_string()).or_insert_with(|| ShxFont::from_file(path).ok().map(Arc::new)).clone()
}

/// Forget a cached SHX font (after the file changed).
pub fn forget_shx(path: &str) {
    shx_cache().lock().unwrap_or_else(|e| e.into_inner()).remove(path);
}

/// FNV-1a of a text.
fn fnv(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h | 1
}

fn var_kind(v: VariableText) -> VarKind {
    match v {
        VariableText::Normal => VarKind::Normal,
        VariableText::DateTime => VarKind::DateTime,
        VariableText::Merge => VarKind::Merge,
        VariableText::Serial => VarKind::Serial,
        VariableText::CutSetting => VarKind::CutSetting,
    }
}

/// The state moved on by a copy's index offset.
fn shifted(state: &VariableState, offset: i64) -> VariableState {
    state.stepped(offset.clamp(-100_000, 100_000))
}

/// The cut values a shape on `layer` shows in `{cut:..}` tokens.
pub fn cut_values(doc: &Document, layer: usize) -> CutValues {
    let l = doc.layer(layer.min(LAYER_COUNT - 1));
    let s = l.settings.iter().find(|s| s.enabled).or(l.settings.first());
    match s {
        Some(s) => {
            CutValues { speed: s.speed, power: s.max_power, interval: s.interval, passes: s.passes, layer: layer as u32, laser_name: l.name.clone() }
        }
        None => CutValues { layer: layer as u32, ..Default::default() },
    }
}

/// The text a text shape shows now: its template evaluated for the project's variable state.
pub fn evaluated_text(doc: &Document, layer: usize, t: &TextShape, now: &EvalContext) -> String {
    if t.variable == VariableText::Normal {
        return t.text.clone();
    }
    let ctx = now.clone().with_cut(cut_values(doc, layer));
    shifted(&doc.variables, t.offset).evaluate_kind(var_kind(t.variable), &t.text, &ctx)
}

/// The library settings of a text shape showing `text`.
pub fn spec_of(t: &TextShape, text: String) -> TextSpec {
    TextSpec {
        text,
        family: t.font.clone(),
        size: t.height,
        bold: t.bold,
        italic: t.italic,
        upper_case: t.upper_case,
        letter_spacing: t.letter_spacing,
        line_spacing: t.line_spacing,
        h_align: match t.align_x {
            AlignX::Left => HAlign::Left,
            AlignX::Middle => HAlign::Center,
            AlignX::Right => HAlign::Right,
        },
        v_align: match t.align_y {
            AlignY::Baseline | AlignY::Bottom => VAlign::Bottom,
            AlignY::Top => VAlign::Top,
            AlignY::Middle => VAlign::Middle,
        },
        weld: t.welded,
        direction: Direction::Auto,
        ..Default::default()
    }
}

/// Whether the font a text shape asks for is installed (SHX files count when they can be read).
pub fn font_present(font: &str) -> bool {
    match font.strip_prefix(SHX_PREFIX) {
        Some(path) => shx_font(path).is_some(),
        None => font.is_empty() || lib().has_family(font),
    }
}

/// Lay out a text shape. The second value says where the glyphs came from.
pub fn layout_of(t: &TextShape, spec: &TextSpec) -> Option<(TextLayout, LayoutSource)> {
    if let Some(path) = t.font.strip_prefix(SHX_PREFIX) {
        if let Some(f) = shx_font(path) {
            return Some((f.layout(spec), LayoutSource::Font));
        }
        let plain = TextSpec { family: String::new(), ..spec.clone() };
        return layout_with_backup(&plain, &lib(), t.backup_glyphs.as_ref()).ok();
    }
    layout_with_backup(spec, &lib(), t.backup_glyphs.as_ref()).ok()
}

/// Final outlines of a laid-out text: welded or per letter, bent along its path when it has one.
pub fn outlines_of(t: &TextShape, layout: &TextLayout) -> Vec<BezPath> {
    let paths: Vec<BezPath> = match &t.on_path {
        Some(p) if !p.path.elements().is_empty() => {
            let opts = PathTextOptions {
                start_offset: p.start_offset,
                center_on_path: p.center_on_path,
                offset: p.offset,
                flip: p.flip,
                distort: t.distort,
            };
            text_on_path(layout, &p.path, &opts).into_iter().map(|g| g.path).filter(|p| !p.elements().is_empty()).collect()
        }
        _ => layout.glyphs.iter().map(|g| g.path.clone()).filter(|p| !p.elements().is_empty()).collect(),
    };
    if t.welded { geomutil::weld(&paths, 0.005) } else { paths }
}

/// The inputs of a text shape's rendering, hashed.
fn text_key(t: &TextShape, text: &str) -> u64 {
    let on_path = t.on_path.as_ref().map(|p| (p.path.to_svg(), p.start_offset, p.offset, p.center_on_path, p.flip));
    fnv(&format!(
        "{text}|{}|{}|{}{}{}{}{}|{}|{}|{:?}|{:?}|{on_path:?}|{}",
        t.font,
        t.height,
        t.bold as u8,
        t.italic as u8,
        t.upper_case as u8,
        t.welded as u8,
        t.distort as u8,
        t.letter_spacing,
        t.line_spacing,
        t.align_x,
        t.align_y,
        font_present(&t.font)
    ))
}

/// Render a text shape now. Returns false when nothing changed.
fn render_text(doc_vars: &Document, layer: usize, t: &mut TextShape, now: &EvalContext) -> bool {
    let text = evaluated_text(doc_vars, layer, t, now);
    let key = text_key(t, &text);
    if t.render_key == key && t.backup.is_some() {
        return false;
    }
    let spec = spec_of(t, text);
    let (path, glyphs) = match layout_of(t, &spec) {
        Some((layout, source)) => {
            let mut p = BezPath::new();
            for o in outlines_of(t, &layout) {
                p.extend(o.elements().iter().copied());
            }
            (p, (source != LayoutSource::Backup).then(|| layout.backup(&spec)))
        }
        None => (BezPath::new(), None),
    };
    t.backup = Some(path);
    if glyphs.is_some() {
        t.backup_glyphs = glyphs;
    }
    t.render_key = key;
    true
}

/// The generated parts of a code shape: the outline (centred) and an error message.
pub fn render_code_paths(doc: &Document, layer: usize, c: &CodeShape, now: &EvalContext) -> (BezPath, Option<String>, f64) {
    let ctx = now.clone().with_cut(cut_values(doc, layer));
    let state = shifted(&doc.variables, c.offset);
    let eval = |s: &str| if c.variable { state.evaluate(s, &ctx) } else { s.to_string() };
    let (paths, err, size) = match &c.spec {
        CodeSpec::Qr(q) => match q.generate_with(&eval) {
            Ok(r) => (r.paths, None, r.size_mm),
            Err(e) => (Vec::new(), Some(e.to_string()), 0.0),
        },
        CodeSpec::Barcode(b) => match b.generate_with(&eval) {
            Ok(r) => (r.paths, None, r.width_mm),
            Err(e) => (Vec::new(), Some(e.to_string()), 0.0),
        },
    };
    let mut p = BezPath::new();
    for q in &paths {
        p.extend(q.elements().iter().copied());
    }
    if !p.elements().is_empty() {
        let bb = p.bounding_box();
        p.apply_affine(Affine::translate((-bb.center().x, -bb.center().y)));
    }
    (p, err, size)
}

fn code_key(c: &CodeShape, payload_probe: &str) -> u64 {
    fnv(&format!("{:?}|{payload_probe}|{}", c.spec, c.variable))
}

fn code_probe(doc: &Document, layer: usize, c: &CodeShape, now: &EvalContext) -> String {
    let ctx = now.clone().with_cut(cut_values(doc, layer));
    let state = shifted(&doc.variables, c.offset);
    let eval = |s: &str| if c.variable { state.evaluate(s, &ctx) } else { s.to_string() };
    match &c.spec {
        CodeSpec::Qr(q) => q.content.payload_with(&eval),
        CodeSpec::Barcode(b) => eval(&b.data),
    }
}

fn render_code(doc: &Document, layer: usize, c: &mut CodeShape, now: &EvalContext) -> bool {
    let probe = code_probe(doc, layer, c, now);
    let key = code_key(c, &probe);
    if c.render_key == key && c.cache.is_some() {
        return false;
    }
    let (p, err, _) = render_code_paths(doc, layer, c, now);
    c.cache = Some(p);
    c.error = err;
    c.render_key = key;
    true
}

/// Shapes to render again: ids with the shape's layer.
fn pending(doc: &Document, now: &EvalContext) -> Vec<darkburn_core::ShapeId> {
    fn walk(doc: &Document, s: &Shape, now: &EvalContext, out: &mut Vec<darkburn_core::ShapeId>) {
        match &s.kind {
            ShapeKind::Text(t) => {
                let key = text_key(t, &evaluated_text(doc, s.layer, t, now));
                if t.render_key != key || t.backup.is_none() {
                    out.push(s.id);
                }
            }
            ShapeKind::Code(c) => {
                let key = code_key(c, &code_probe(doc, s.layer, c, now));
                if c.render_key != key || c.cache.is_none() {
                    out.push(s.id);
                }
            }
            ShapeKind::Group { children } => children.iter().for_each(|c| walk(doc, c, now, out)),
            _ => {}
        }
        if let Some(a) = &s.array {
            a.source.iter().for_each(|c| walk(doc, c, now, out));
        }
    }
    let mut out = Vec::new();
    for s in doc.iter() {
        walk(doc, s, now, &mut out);
    }
    out
}

fn render_in(doc: &Document, s: &mut Shape, ids: &[darkburn_core::ShapeId], now: &EvalContext, n: &mut usize) {
    let layer = s.layer;
    if ids.contains(&s.id) {
        match &mut s.kind {
            ShapeKind::Text(t) => *n += render_text(doc, layer, t, now) as usize,
            ShapeKind::Code(c) => *n += render_code(doc, layer, c, now) as usize,
            _ => {}
        }
    }
    if let ShapeKind::Group { children } = &mut s.kind {
        for c in children {
            render_in(doc, c, ids, now, n);
        }
    }
    if let Some(a) = &mut s.array {
        for c in &mut a.source {
            render_in(doc, c, ids, now, n);
        }
    }
}

/// Render again every text and code shape whose inputs changed. Returns how many were rendered.
pub fn refresh_at(doc: &mut Document, now: &EvalContext) -> usize {
    let ids = pending(doc, now);
    if ids.is_empty() {
        return 0;
    }
    let reference = doc.clone();
    let tops: Vec<darkburn_core::ShapeId> = doc.iter().map(|s| s.id).collect();
    let mut n = 0;
    for id in tops {
        if let Some(s) = doc.get_mut(id) {
            render_in(&reference, s, &ids, now, &mut n);
        }
    }
    n
}

/// [`refresh_at`] with the clock.
pub fn refresh(doc: &mut Document) -> usize {
    refresh_at(doc, &EvalContext::local_now())
}

/// Hook for the job builder and the exporters: the document with every variable text,
/// QR code and barcode evaluated for right now.
pub fn prepare_for_output(doc: &Document) -> Document {
    let mut d = doc.clone();
    refresh(&mut d);
    d
}

/// Whether the document holds any variable text or code (auto-advance only counts these).
pub fn has_variable_text(doc: &Document) -> bool {
    fn walk(s: &Shape) -> bool {
        let own = match &s.kind {
            ShapeKind::Text(t) => t.variable != VariableText::Normal,
            ShapeKind::Code(c) => c.variable,
            ShapeKind::Group { children } => children.iter().any(walk),
            _ => false,
        };
        own || s.array.as_ref().is_some_and(|a| a.source.iter().any(walk))
    }
    doc.iter().any(walk)
}

/// Convert the shape's evaluated text to plain text (Bake / Convert to Path): variable text
/// becomes normal text showing what it shows now.
pub fn bake_text(doc: &Document, s: &mut Shape, now: &EvalContext) {
    let layer = s.layer;
    match &mut s.kind {
        ShapeKind::Text(t) if t.variable != VariableText::Normal => {
            t.text = evaluated_text(doc, layer, t, now);
            t.variable = VariableText::Normal;
            t.offset = 0;
        }
        ShapeKind::Code(c) if c.variable => {
            let payload = code_probe(doc, layer, c, now);
            match &mut c.spec {
                CodeSpec::Qr(q) => q.content = darkburn_text::QrContent::Text(payload),
                CodeSpec::Barcode(b) => b.data = payload,
            }
            c.variable = false;
            c.offset = 0;
        }
        _ => {}
    }
}

/// Edit a code shape's size so the symbol's side is `size` mm (module size follows).
pub fn set_code_size(doc: &Document, layer: usize, c: &mut CodeShape, size: f64) {
    let now = EvalContext::local_now();
    let (_, _, current) = render_code_paths(doc, layer, c, &now);
    if current > 0.0 && size > 0.0 {
        let k = size / current;
        match &mut c.spec {
            CodeSpec::Qr(q) => q.module_mm *= k,
            CodeSpec::Barcode(b) => {
                b.module_mm *= k;
                b.height_mm *= k;
            }
        }
    }
}
