//! Text to outlines: shaping (RTL, complex scripts, CJK fallback), alignment, spacing, bold/italic,
//! weld, and the "backup outline" mechanism for projects opened without the font.
//!
//! Output is in millimetres, Y up. The text origin is `(0, 0)`:
//! * horizontal: x = 0 is the left edge / centre / right edge of each line per [`HAlign`];
//! * vertical: [`VAlign::Top`] puts the cap-height line of the first line at y = 0,
//!   [`VAlign::Bottom`] puts the baseline of the last line at y = 0 and [`VAlign::Middle`] centres
//!   the block between the two. Further lines go down (negative y).
//!
//! Size: [`SizeMode::CapHeight`] (default, what a user means by "height 10 mm") scales so a capital
//! letter is `size` tall; [`SizeMode::Em`] makes the em square `size` tall.

use crate::error::{Result, TextError};
use crate::fonts::{FontLibrary, ResolvedFace};
use crate::geomutil;
use kurbo::{Affine, BezPath, Rect};
use serde::{Deserialize, Serialize};
use unicode_bidi::{BidiInfo, Level};

/// Fill rule to use when filling combined glyph paths (counters are holes).
pub const GLYPH_FILL_RULE_EVEN_ODD: bool = true;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum HAlign {
    #[default]
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum VAlign {
    Top,
    Middle,
    #[default]
    Bottom,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SizeMode {
    /// `size` is the height of capital letters.
    #[default]
    CapHeight,
    /// `size` is the em square.
    Em,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Direction {
    /// Per paragraph from the first strong character.
    #[default]
    Auto,
    Ltr,
    Rtl,
}

/// Everything that defines a text object's look. Editable and serialisable.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TextSpec {
    pub text: String,
    /// Font family name; empty or unknown falls back (see [`layout_with_backup`]).
    pub family: String,
    /// Height in mm (see [`SizeMode`]).
    pub size: f64,
    pub size_mode: SizeMode,
    pub bold: bool,
    pub italic: bool,
    pub upper_case: bool,
    /// Extra advance per glyph in mm (may be negative).
    pub letter_spacing: f64,
    /// Extra distance between lines in mm on top of the font's natural line height.
    pub line_spacing: f64,
    pub h_align: HAlign,
    pub v_align: VAlign,
    /// Union overlapping glyph outlines (see [`TextLayout::outlines`]).
    pub weld: bool,
    pub direction: Direction,
}

impl Default for TextSpec {
    fn default() -> Self {
        Self {
            text: String::new(),
            family: String::new(),
            size: 10.0,
            size_mode: SizeMode::CapHeight,
            bold: false,
            italic: false,
            upper_case: false,
            letter_spacing: 0.0,
            line_spacing: 0.0,
            h_align: HAlign::Left,
            v_align: VAlign::Bottom,
            weld: false,
            direction: Direction::Auto,
        }
    }
}

/// One shaped glyph (or SHX character) placed in the layout.
#[derive(Clone, Debug)]
pub struct GlyphOutline {
    /// The text of the cluster this glyph belongs to (first char for ligatures).
    pub text: String,
    pub line: usize,
    /// Outline in layout space (already positioned).
    pub path: BezPath,
    /// X of the glyph origin (pen position) in layout space.
    pub origin_x: f64,
    /// Baseline y of its line in layout space.
    pub baseline_y: f64,
    pub advance: f64,
    pub rtl: bool,
    /// The font had no glyph (notdef box drawn or nothing).
    pub missing: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct LineMetrics {
    pub baseline_y: f64,
    /// Advance width of the line.
    pub width: f64,
    /// X of the line start after alignment.
    pub x_start: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct TextMetrics {
    pub em_mm: f64,
    pub cap_height_mm: f64,
    pub ascent_mm: f64,
    pub descent_mm: f64,
    pub line_height_mm: f64,
}

#[derive(Clone, Debug)]
pub struct TextLayout {
    pub glyphs: Vec<GlyphOutline>,
    pub lines: Vec<LineMetrics>,
    pub metrics: TextMetrics,
    /// Union of glyph bounding boxes (zero rect when nothing is drawn).
    pub bounds: Rect,
    /// Family actually used for the primary face.
    pub family_used: String,
}

impl TextLayout {
    /// All glyph outlines in one path, meant to be filled even-odd.
    pub fn combined_path(&self) -> BezPath {
        let mut p = BezPath::new();
        for g in &self.glyphs {
            p.extend(g.path.elements().iter().copied());
        }
        p
    }

    /// Outlines for output: one path per glyph, or the welded union (one path per outer shape,
    /// holes inside it as extra closed sub-paths, flattened to 0.005 mm) when `weld` is set.
    pub fn outlines(&self, weld: bool) -> Vec<BezPath> {
        let per_glyph: Vec<BezPath> = self.glyphs.iter().filter(|g| !g.path.elements().is_empty()).map(|g| g.path.clone()).collect();
        if weld { geomutil::weld(&per_glyph, 0.005) } else { per_glyph }
    }

    /// Weld with a caller supplied union (for example the app's boolean engine).
    pub fn outlines_with(&self, union: &dyn Fn(&[BezPath]) -> Vec<BezPath>) -> Vec<BezPath> {
        let per_glyph: Vec<BezPath> = self.glyphs.iter().filter(|g| !g.path.elements().is_empty()).map(|g| g.path.clone()).collect();
        union(&per_glyph)
    }

    /// Width of the widest line.
    pub fn width(&self) -> f64 {
        self.lines.iter().map(|l| l.width).fold(0.0, f64::max)
    }

    /// Serialisable snapshot to store with the project (see [`layout_with_backup`]).
    pub fn backup(&self, spec: &TextSpec) -> BackupOutline {
        BackupOutline {
            spec: spec.clone(),
            glyphs: self
                .glyphs
                .iter()
                .map(|g| BackupGlyph {
                    text: g.text.clone(),
                    line: g.line,
                    svg: g.path.to_svg(),
                    origin_x: g.origin_x,
                    baseline_y: g.baseline_y,
                    advance: g.advance,
                    rtl: g.rtl,
                })
                .collect(),
            lines: self.lines.clone(),
            metrics: self.metrics,
            family_used: self.family_used.clone(),
        }
    }
}

/// Outline snapshot saved with a project so it opens on a machine without the font.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BackupOutline {
    pub spec: TextSpec,
    pub glyphs: Vec<BackupGlyph>,
    pub lines: Vec<LineMetrics>,
    pub metrics: TextMetrics,
    pub family_used: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BackupGlyph {
    pub text: String,
    pub line: usize,
    /// SVG path data in layout space (mm).
    pub svg: String,
    pub origin_x: f64,
    pub baseline_y: f64,
    pub advance: f64,
    pub rtl: bool,
}

impl BackupOutline {
    /// Rebuild the layout without any font.
    pub fn to_layout(&self) -> Result<TextLayout> {
        let mut glyphs = Vec::new();
        for g in &self.glyphs {
            let path = BezPath::from_svg(&g.svg).map_err(|e| TextError::Font(format!("backup outline: {e}")))?;
            glyphs.push(GlyphOutline {
                text: g.text.clone(),
                line: g.line,
                path,
                origin_x: g.origin_x,
                baseline_y: g.baseline_y,
                advance: g.advance,
                rtl: g.rtl,
                missing: false,
            });
        }
        let bounds = geomutil::union_bbox(glyphs.iter().map(|g| &g.path)).unwrap_or(Rect::ZERO);
        Ok(TextLayout { glyphs, lines: self.lines.clone(), metrics: self.metrics, bounds, family_used: self.family_used.clone() })
    }
}

/// Where a layout came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutSource {
    /// The requested family.
    Font,
    /// Requested family missing; the default font was used.
    Fallback,
    /// Requested family missing; the stored backup outline was used.
    Backup,
}

/// True when the spec's family is non-empty and not installed.
pub fn font_missing(spec: &TextSpec, lib: &FontLibrary) -> bool {
    !spec.family.is_empty() && !lib.has_family(&spec.family)
}

/// Lay out a text object, using `backup` when the font is missing and the backup still matches the spec.
pub fn layout_with_backup(spec: &TextSpec, lib: &FontLibrary, backup: Option<&BackupOutline>) -> Result<(TextLayout, LayoutSource)> {
    if font_missing(spec, lib) {
        if let Some(b) = backup.filter(|b| b.spec == *spec) {
            return Ok((b.to_layout()?, LayoutSource::Backup));
        }
        return Ok((layout_text(spec, lib)?, LayoutSource::Fallback));
    }
    Ok((layout_text(spec, lib)?, LayoutSource::Font))
}

struct FaceCtx<'a> {
    face: rustybuzz::Face<'a>,
    upem: f64,
    synth_bold: bool,
    synth_italic: bool,
}

const ITALIC_SHEAR: f64 = 0.2126; // tan(12 degrees)
const SYNTH_BOLD_EM: f64 = 0.012; // outline growth per side, in em

fn open_face<'a>(data: &'a crate::fonts::FaceData, rf: &ResolvedFace) -> Result<FaceCtx<'a>> {
    let mut face = rustybuzz::Face::from_slice(&data.data, data.index).ok_or_else(|| TextError::Font("cannot parse font face".into()))?;
    if let Some(w) = rf.weight_axis {
        face.set_variations(&[rustybuzz::Variation { tag: ttf_parser::Tag::from_bytes(b"wght"), value: w }]);
    }
    let upem = face.units_per_em() as f64;
    Ok(FaceCtx { face, upem, synth_bold: rf.synthetic_bold, synth_italic: rf.synthetic_italic })
}

struct Collector(BezPath);
impl ttf_parser::OutlineBuilder for Collector {
    fn move_to(&mut self, x: f32, y: f32) {
        self.0.move_to((x as f64, y as f64));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.0.line_to((x as f64, y as f64));
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.0.quad_to((x1 as f64, y1 as f64), (x as f64, y as f64));
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.0.curve_to((x1 as f64, y1 as f64), (x2 as f64, y2 as f64), (x as f64, y as f64));
    }
    fn close(&mut self) {
        self.0.close_path();
    }
}

fn cap_height_units(f: &FaceCtx) -> f64 {
    if let Some(c) = f.face.capital_height().filter(|c| *c > 0) {
        return c as f64;
    }
    if let Some(b) = f.face.glyph_index('H').and_then(|g| f.face.glyph_bounding_box(g)).filter(|b| b.y_max > 0) {
        return b.y_max as f64;
    }
    0.7 * f.upem
}

fn is_attached(ch: char) -> bool {
    let c = ch as u32;
    (0x0300..=0x036F).contains(&c)
        || (0x200B..=0x200F).contains(&c)
        || (0xFE00..=0xFE0F).contains(&c)
        || (0x0591..=0x05BD).contains(&c)
        || (0x064B..=0x065F).contains(&c)
}

#[derive(Clone)]
struct Segment {
    face: usize, // index into faces
    text: String,
}

/// Lay out text with the given font library.
pub fn layout_text(spec: &TextSpec, lib: &FontLibrary) -> Result<TextLayout> {
    let primary_rf = lib.resolve_or_default(&spec.family, spec.bold, spec.italic).ok_or_else(|| TextError::Font("no fonts available".into()))?;
    let primary_data = lib.face_data(primary_rf.id).ok_or_else(|| TextError::Font("cannot load font data".into()))?;
    let primary = open_face(&primary_data, &primary_rf)?;
    let family_used = lib.family_of(primary_rf.id).unwrap_or_default();

    let size = if spec.size.is_finite() && spec.size > 0.0 { spec.size } else { 10.0 };
    let em_mm = match spec.size_mode {
        SizeMode::Em => size,
        SizeMode::CapHeight => size * primary.upem / cap_height_units(&primary),
    };
    let scale_p = em_mm / primary.upem;
    let asc = primary.face.ascender() as f64;
    let desc = primary.face.descender() as f64;
    let gap = primary.face.line_gap() as f64;
    let metrics = TextMetrics {
        em_mm,
        cap_height_mm: cap_height_units(&primary) * scale_p,
        ascent_mm: asc * scale_p,
        descent_mm: desc * scale_p,
        line_height_mm: ((asc - desc + gap) * scale_p + spec.line_spacing).max(0.0),
    };

    // Fallback faces opened on demand: owned data kept alive here.
    let mut fb_ids: Vec<(fontdb::ID, std::sync::Arc<crate::fonts::FaceData>)> = Vec::new();

    let text: String = if spec.upper_case { spec.text.to_uppercase() } else { spec.text.clone() };
    let raw_lines: Vec<&str> = text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).collect();

    // First pass: choose a face index per char for all lines (index 0 = primary).
    // Face table: 0 primary, then fallbacks in fb_ids order + 1.
    let mut line_segments: Vec<Vec<(Vec<Segment>, bool)>> = Vec::new(); // per line: visual runs -> segments, rtl
    for line in &raw_lines {
        let mut runs_out = Vec::new();
        if !line.is_empty() {
            let default_level = match spec.direction {
                Direction::Auto => None,
                Direction::Ltr => Some(Level::ltr()),
                Direction::Rtl => Some(Level::rtl()),
            };
            let info = BidiInfo::new(line, default_level);
            for para in &info.paragraphs {
                let (levels, runs) = info.visual_runs(para, para.range.clone());
                for run in runs {
                    let rtl = levels[run.start].is_rtl();
                    let mut segs: Vec<Segment> = Vec::new();
                    for ch in line[run.clone()].chars() {
                        let mut face = 0usize;
                        let has = primary.face.glyph_index(ch).is_some() || ch.is_whitespace() || ch.is_control();
                        if !has {
                            if is_attached(ch) && !segs.is_empty() {
                                face = segs.last().unwrap().face;
                            } else if let Some(fid) = lib.fallback_for(ch) {
                                let pos = match fb_ids.iter().position(|(i, _)| *i == fid) {
                                    Some(p) => p,
                                    None => {
                                        if let Some(d) = lib.face_data(fid) {
                                            fb_ids.push((fid, d));
                                            fb_ids.len() - 1
                                        } else {
                                            usize::MAX
                                        }
                                    }
                                };
                                if pos != usize::MAX {
                                    face = pos + 1;
                                }
                            }
                        } else if is_attached(ch) && !segs.is_empty() {
                            face = segs.last().unwrap().face;
                        }
                        match segs.last_mut() {
                            Some(s) if s.face == face => s.text.push(ch),
                            _ => segs.push(Segment { face, text: ch.to_string() }),
                        }
                    }
                    if rtl {
                        segs.reverse();
                    }
                    runs_out.push((segs, rtl));
                }
            }
        }
        line_segments.push(runs_out);
    }

    let fb_faces: Vec<FaceCtx> = fb_ids
        .iter()
        .map(|(id, d)| {
            let rf = ResolvedFace { id: *id, synthetic_bold: spec.bold, synthetic_italic: spec.italic, weight_axis: None };
            open_face(d, &rf)
        })
        .collect::<Result<_>>()?;
    let face_of = |i: usize| -> &FaceCtx { if i == 0 { &primary } else { &fb_faces[i - 1] } };

    let mut glyphs: Vec<GlyphOutline> = Vec::new();
    let mut lines: Vec<LineMetrics> = Vec::new();
    for (li, runs) in line_segments.iter().enumerate() {
        let baseline_y = -(li as f64) * metrics.line_height_mm;
        let mut pen = 0.0f64;
        let first_glyph = glyphs.len();
        for (segs, rtl) in runs {
            for seg in segs {
                let fc = face_of(seg.face);
                let s = em_mm / fc.upem;
                let mut buf = rustybuzz::UnicodeBuffer::new();
                buf.push_str(&seg.text);
                buf.set_direction(if *rtl { rustybuzz::Direction::RightToLeft } else { rustybuzz::Direction::LeftToRight });
                buf.guess_segment_properties();
                let out = rustybuzz::shape(&fc.face, &[], buf);
                let infos = out.glyph_infos();
                let poss = out.glyph_positions();
                for (gi, gp) in infos.iter().zip(poss.iter()) {
                    let gid = ttf_parser::GlyphId(gi.glyph_id as u16);
                    let mut c = Collector(BezPath::new());
                    let has_outline = fc.face.outline_glyph(gid, &mut c).is_some();
                    let mut path = c.0;
                    let mut adv = gp.x_advance as f64 * s;
                    if fc.synth_bold && has_outline {
                        path = geomutil::embolden(&path, SYNTH_BOLD_EM * fc.upem);
                    }
                    if fc.synth_bold {
                        adv += 2.0 * SYNTH_BOLD_EM * em_mm;
                    }
                    let shear = if fc.synth_italic { ITALIC_SHEAR } else { 0.0 };
                    let origin_x = pen + gp.x_offset as f64 * s;
                    let xf = Affine::translate((origin_x, baseline_y + gp.y_offset as f64 * s))
                        * Affine::scale(s)
                        * Affine::new([1.0, 0.0, shear, 1.0, 0.0, 0.0]);
                    let path = xf * path;
                    let cluster = gi.cluster as usize;
                    let ctext = seg.text.get(cluster..).and_then(|t| t.chars().next()).map(|c| c.to_string()).unwrap_or_default();
                    glyphs.push(GlyphOutline {
                        text: ctext,
                        line: li,
                        path,
                        origin_x,
                        baseline_y,
                        advance: adv + if adv != 0.0 { spec.letter_spacing } else { 0.0 },
                        rtl: *rtl,
                        missing: gid.0 == 0,
                    });
                    if adv != 0.0 {
                        pen += adv + spec.letter_spacing;
                    }
                }
            }
        }
        let width = pen;
        let x_start = match spec.h_align {
            HAlign::Left => 0.0,
            HAlign::Center => -width / 2.0,
            HAlign::Right => -width,
        };
        for g in &mut glyphs[first_glyph..] {
            g.path.apply_affine(Affine::translate((x_start, 0.0)));
            g.origin_x += x_start;
        }
        lines.push(LineMetrics { baseline_y, width, x_start });
    }

    // Vertical alignment.
    let n = lines.len().max(1) as f64;
    let top = metrics.cap_height_mm;
    let bottom = -(n - 1.0) * metrics.line_height_mm;
    let dy = match spec.v_align {
        VAlign::Top => -top,
        VAlign::Bottom => -bottom,
        VAlign::Middle => -(top + bottom) / 2.0,
    };
    if dy != 0.0 {
        for g in &mut glyphs {
            g.path.apply_affine(Affine::translate((0.0, dy)));
            g.baseline_y += dy;
        }
        for l in &mut lines {
            l.baseline_y += dy;
        }
    }
    let bounds = geomutil::union_bbox(glyphs.iter().map(|g| &g.path)).unwrap_or(Rect::ZERO);
    Ok(TextLayout { glyphs, lines, metrics, bounds, family_used })
}

/// Bounding box helper for any outline list.
pub fn outlines_bbox(paths: &[BezPath]) -> Rect {
    geomutil::union_bbox(paths.iter()).unwrap_or(Rect::ZERO)
}
