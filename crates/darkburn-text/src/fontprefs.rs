//! Font picker helpers: recent / favourite bookkeeping, default font, and preview outlines.

use crate::error::Result;
use crate::fonts::FontLibrary;
use crate::layout::{HAlign, SizeMode, TextSpec, VAlign, layout_text};
use kurbo::{Affine, BezPath};
use serde::{Deserialize, Serialize};

/// Persisted font preferences (store in the app settings).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FontPrefs {
    /// Most recently used first.
    pub recent: Vec<String>,
    pub favourites: Vec<String>,
    pub max_recent: usize,
    /// Family for new text objects.
    pub default_family: String,
    /// Height in mm for new text objects.
    pub default_size: f64,
}

impl Default for FontPrefs {
    fn default() -> Self {
        Self { recent: Vec::new(), favourites: Vec::new(), max_recent: 10, default_family: String::new(), default_size: 10.0 }
    }
}

/// The sections of the font drop-down.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FontListing {
    pub favourites: Vec<String>,
    pub recent: Vec<String>,
    pub all: Vec<String>,
}

impl FontPrefs {
    /// Record that `family` was used.
    pub fn touch(&mut self, family: &str) {
        if family.is_empty() {
            return;
        }
        self.recent.retain(|f| !f.eq_ignore_ascii_case(family));
        self.recent.insert(0, family.to_string());
        self.recent.truncate(self.max_recent.max(1));
    }

    pub fn is_favourite(&self, family: &str) -> bool {
        self.favourites.iter().any(|f| f.eq_ignore_ascii_case(family))
    }

    /// Returns the new favourite state.
    pub fn toggle_favourite(&mut self, family: &str) -> bool {
        if self.is_favourite(family) {
            self.favourites.retain(|f| !f.eq_ignore_ascii_case(family));
            false
        } else {
            self.favourites.push(family.to_string());
            true
        }
    }

    /// Build the drop-down: favourites and recents first (only fonts that still exist unless `keep_missing`),
    /// then everything, optionally filtered by a case-insensitive substring.
    pub fn listing(&self, installed: &[String], query: &str, keep_missing: bool) -> FontListing {
        let q = query.to_lowercase();
        let pass = |f: &String| q.is_empty() || f.to_lowercase().contains(&q);
        let exists = |f: &String| keep_missing || installed.iter().any(|i| i.eq_ignore_ascii_case(f));
        FontListing {
            favourites: self.favourites.iter().filter(|f| exists(f) && pass(f)).cloned().collect(),
            recent: self.recent.iter().filter(|f| exists(f) && pass(f)).cloned().collect(),
            all: installed.iter().filter(|f| pass(f)).cloned().collect(),
        }
    }

    /// Family for a new text object: the default if installed, otherwise the library default.
    pub fn family_for_new_text(&self, lib: &FontLibrary) -> String {
        if !self.default_family.is_empty() && lib.has_family(&self.default_family) {
            self.default_family.clone()
        } else {
            lib.default_family().to_string()
        }
    }
}

/// Sorted family names for a picker.
pub fn list_families(lib: &FontLibrary) -> Vec<String> {
    lib.families()
}

/// Outline of `text` in `family`, scaled to fit a `box_w` x `box_h` mm box (aspect kept), centred on the origin.
/// Meant for live previews in the font drop-down.
pub fn preview_outline(lib: &FontLibrary, family: &str, text: &str, box_w: f64, box_h: f64) -> Result<Vec<BezPath>> {
    let spec = TextSpec {
        text: text.to_string(),
        family: family.to_string(),
        size: 10.0,
        size_mode: SizeMode::Em,
        h_align: HAlign::Left,
        v_align: VAlign::Bottom,
        ..Default::default()
    };
    let layout = layout_text(&spec, lib)?;
    let b = layout.bounds;
    let paths = layout.outlines(false);
    if paths.is_empty() || b.width() <= 0.0 || b.height() <= 0.0 {
        return Ok(paths);
    }
    let s = (box_w / b.width()).min(box_h / b.height());
    let xf = Affine::scale(s) * Affine::translate((-b.center().x, -b.center().y));
    Ok(paths.into_iter().map(|p| xf * p).collect())
}

/// Bounding box width/height of a set of paths.
pub fn size_of(paths: &[BezPath]) -> (f64, f64) {
    let r = crate::layout::outlines_bbox(paths);
    (r.width(), r.height())
}
