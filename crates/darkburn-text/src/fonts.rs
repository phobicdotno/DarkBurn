//! Font discovery: system fonts, font files, family lists and face resolution
//! (real bold/italic faces, variable-weight axis, or synthetic fallback).

use crate::error::{Result, TextError};
use fontdb::{Database, Family, ID, Query, Stretch, Style, Weight};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// The id of one face in a [`FontLibrary`].
pub type FaceId = ID;

/// Raw bytes and collection index of one face.
#[derive(Debug)]
pub struct FaceData {
    pub data: Vec<u8>,
    pub index: u32,
}

/// The bytes of the face, so a face can be handed on without copying them (the interface
/// fonts take shared bytes).
impl AsRef<[u8]> for FaceData {
    fn as_ref(&self) -> &[u8] {
        &self.data
    }
}

/// A face picked for a family + bold/italic request.
#[derive(Debug, Clone)]
pub struct ResolvedFace {
    pub id: ID,
    /// Face is not bold but bold was asked for and no weight axis exists: embolden outlines.
    pub synthetic_bold: bool,
    /// Face is upright but italic was asked for: shear outlines.
    pub synthetic_italic: bool,
    /// Variable font: set the `wght` axis to this value.
    pub weight_axis: Option<f32>,
}

/// Font collection (system + user supplied files).
pub struct FontLibrary {
    db: Database,
    default_family: String,
    cache: Mutex<HashMap<ID, Arc<FaceData>>>,
    fallback_cache: Mutex<HashMap<char, Option<ID>>>,
}

impl Default for FontLibrary {
    fn default() -> Self {
        Self::new()
    }
}

impl FontLibrary {
    /// Empty library (no fonts at all).
    pub fn new() -> Self {
        Self { db: Database::new(), default_family: "Arial".into(), cache: Mutex::default(), fallback_cache: Mutex::default() }
    }

    /// Library with the operating system fonts loaded.
    pub fn with_system_fonts() -> Self {
        let mut l = Self::new();
        l.load_system_fonts();
        l
    }

    pub fn load_system_fonts(&mut self) {
        self.db.load_system_fonts();
        self.invalidate();
    }

    /// Add font bytes (TTF/OTF/TTC). Returns the family names that were added.
    pub fn load_font_data(&mut self, data: Vec<u8>) -> Result<Vec<String>> {
        let before = self.db.len();
        self.db.load_font_data(data);
        if self.db.len() == before {
            return Err(TextError::Font("no usable font face in data".into()));
        }
        self.invalidate();
        Ok(self.db.faces().skip(before).filter_map(|f| f.families.first().map(|x| x.0.clone())).collect())
    }

    /// Add a font file from disk.
    pub fn load_font_file(&mut self, path: impl AsRef<Path>) -> Result<Vec<String>> {
        let data = std::fs::read(path.as_ref())?;
        self.load_font_data(data)
    }

    /// Family used when the requested family is missing.
    pub fn set_default_family(&mut self, family: impl Into<String>) {
        self.default_family = family.into();
        self.invalidate();
    }

    pub fn default_family(&self) -> &str {
        &self.default_family
    }

    fn invalidate(&self) {
        self.fallback_cache.lock().unwrap().clear();
    }

    /// Number of faces known.
    pub fn face_count(&self) -> usize {
        self.db.len()
    }

    /// Sorted, de-duplicated family names.
    pub fn families(&self) -> Vec<String> {
        let mut v: Vec<String> = self.db.faces().filter_map(|f| f.families.first().map(|x| x.0.clone())).collect();
        v.sort_by_key(|a| a.to_lowercase());
        v.dedup();
        v
    }

    /// Whether a family with this exact (case-insensitive) name exists.
    pub fn has_family(&self, name: &str) -> bool {
        self.db.faces().any(|f| f.families.iter().any(|(n, _)| n.eq_ignore_ascii_case(name)))
    }

    /// Bytes of a face, cached.
    pub fn face_data(&self, id: ID) -> Option<Arc<FaceData>> {
        if let Some(d) = self.cache.lock().unwrap().get(&id) {
            return Some(d.clone());
        }
        let fd = self.db.with_face_data(id, |d, i| FaceData { data: d.to_vec(), index: i })?;
        let fd = Arc::new(fd);
        self.cache.lock().unwrap().insert(id, fd.clone());
        Some(fd)
    }

    /// Pick the best face for `family` with the requested style. `None` if the family does not exist.
    pub fn resolve(&self, family: &str, bold: bool, italic: bool) -> Option<ResolvedFace> {
        if !self.has_family(family) {
            return None;
        }
        let q = Query {
            families: &[Family::Name(family)],
            weight: if bold { Weight::BOLD } else { Weight::NORMAL },
            stretch: Stretch::Normal,
            style: if italic { Style::Italic } else { Style::Normal },
        };
        let id = self.db.query(&q)?;
        let info = self.db.face(id)?;
        let real_bold = info.weight.0 >= 600;
        let real_italic = info.style != Style::Normal;
        let mut weight_axis = None;
        let mut synthetic_bold = false;
        if bold && !real_bold {
            if self.has_weight_axis(id) {
                weight_axis = Some(700.0);
            } else {
                synthetic_bold = true;
            }
        }
        Some(ResolvedFace { id, synthetic_bold, synthetic_italic: italic && !real_italic, weight_axis })
    }

    fn has_weight_axis(&self, id: ID) -> bool {
        self.db
            .with_face_data(id, |d, i| {
                ttf_parser::Face::parse(d, i)
                    .ok()
                    .is_some_and(|f| f.variation_axes().into_iter().any(|a| a.tag == ttf_parser::Tag::from_bytes(b"wght")))
            })
            .unwrap_or(false)
    }

    /// Resolve with fallback to the default family and then to any face.
    pub fn resolve_or_default(&self, family: &str, bold: bool, italic: bool) -> Option<ResolvedFace> {
        self.resolve(family, bold, italic).or_else(|| self.resolve(&self.default_family, bold, italic)).or_else(|| {
            let id = self.db.faces().find(|f| f.style == Style::Normal).or_else(|| self.db.faces().next())?.id;
            Some(ResolvedFace { id, synthetic_bold: bold, synthetic_italic: italic, weight_axis: None })
        })
    }

    /// A face able to draw `ch` (for CJK / symbol fallback). Regular upright faces preferred.
    pub fn fallback_for(&self, ch: char) -> Option<ID> {
        if let Some(r) = self.fallback_cache.lock().unwrap().get(&ch) {
            return *r;
        }
        let mut best: Option<(i32, ID)> = None;
        let ids: Vec<(ID, i32)> = self
            .db
            .faces()
            .map(|f| {
                let mut score = 0;
                if f.style == Style::Normal {
                    score += 4;
                }
                score -= (f.weight.0 as i32 - 400).abs() / 100;
                if f.families.iter().any(|(n, _)| n.eq_ignore_ascii_case(&self.default_family)) {
                    score += 3;
                }
                (f.id, score)
            })
            .collect();
        for (id, score) in ids {
            if best.is_some_and(|(s, _)| s >= score) {
                continue;
            }
            let has =
                self.db.with_face_data(id, |d, i| ttf_parser::Face::parse(d, i).ok().is_some_and(|f| f.glyph_index(ch).is_some())).unwrap_or(false);
            if has {
                best = Some((score, id));
            }
        }
        let r = best.map(|b| b.1);
        self.fallback_cache.lock().unwrap().insert(ch, r);
        r
    }

    /// How many of `chars` face `id` has a glyph for (0 for an unknown or unreadable face).
    pub fn glyph_count(&self, id: ID, chars: &[char]) -> usize {
        self.db
            .with_face_data(id, |d, i| {
                ttf_parser::Face::parse(d, i).map(|f| chars.iter().filter(|c| f.glyph_index(**c).is_some()).count()).unwrap_or(0)
            })
            .unwrap_or(0)
    }

    /// For each of `chars`, whether face `id` has a glyph for it (all false for an unknown or
    /// unreadable face). The face is read once for all of them.
    pub fn glyphs(&self, id: ID, chars: &[char]) -> Vec<bool> {
        self.db
            .with_face_data(id, |d, i| ttf_parser::Face::parse(d, i).ok().map(|f| chars.iter().map(|c| f.glyph_index(*c).is_some()).collect()))
            .flatten()
            .unwrap_or_else(|| vec![false; chars.len()])
    }

    /// Bytes of a face, read now and not kept by the library (unlike [`Self::face_data`]), so
    /// they are freed when the caller lets go of them.
    pub fn read_face_data(&self, id: ID) -> Option<FaceData> {
        self.db.with_face_data(id, |d, i| FaceData { data: d.to_vec(), index: i })
    }

    /// Family name of a face id.
    pub fn family_of(&self, id: ID) -> Option<String> {
        self.db.face(id).and_then(|f| f.families.first().map(|x| x.0.clone()))
    }
}
