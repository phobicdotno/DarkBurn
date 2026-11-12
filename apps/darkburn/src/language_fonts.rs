//! System fonts for the interface languages the bundled interface font cannot draw.
//!
//! IBM Plex Sans draws Latin, Greek and Cyrillic. For Japanese, Korean and Chinese the program
//! looks among the fonts the computer already has (the same font list the Text tool uses,
//! `darkburn_text::FontLibrary`) for faces that draw the characters of the language's file, and
//! puts them right after Plex in every interface family. Nothing is added to the program itself.
//!
//! The faces follow the language in use, plus, while the Language menu is open, the faces the
//! names in that menu need. The system font list is read once, the first time a language needs
//! it; with only Latin, Greek and Cyrillic languages it is never read.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use darkburn_text::FontLibrary;
use egui::epaint::text::{FontData, FontDefinitions, Fonts, TextOptions};
use egui::{FontFamily, FontId};

use crate::languages::{self, Language};

/// One face to add: a name for the font definitions and the face.
type Face = (String, FontData);

/// The fonts of this computer, read on first use.
fn system() -> &'static FontLibrary {
    static LIB: OnceLock<FontLibrary> = OnceLock::new();
    LIB.get_or_init(FontLibrary::with_system_fonts)
}

/// The characters of `text` worth a glyph: not spaces, not control characters, each once.
fn characters(text: &str) -> Vec<char> {
    let mut out: Vec<char> = text.chars().filter(|c| !c.is_whitespace() && !c.is_control()).collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Every character a language shows: the texts of its file and its own name.
pub fn language_characters(language: &Language) -> Vec<char> {
    let mut text = language.name.to_string();
    if let Some(file) = languages::file(language.code)
        && let Ok(table) = serde_json::from_str::<HashMap<String, String>>(file)
    {
        for v in table.values() {
            text.push_str(v);
        }
    }
    characters(&text)
}

/// The characters of `chars` that `fonts` cannot draw in the interface font.
fn missing(fonts: &mut Fonts, chars: &[char]) -> Vec<char> {
    let id = FontId::new(14.0, FontFamily::Proportional);
    chars.iter().copied().filter(|c| !fonts.has_glyph(&id, *c)).collect()
}

/// The characters of `chars` that the interface fonts in `fonts` cannot draw.
pub fn undrawable(fonts: FontDefinitions, chars: &[char]) -> Vec<char> {
    missing(&mut Fonts::new(TextOptions::default(), fonts), chars)
}

/// Faces of `lib` that together draw as much of `chars` as the computer can: first the
/// families of `preferred` (the one that draws the most of what is left, earlier on a tie), then
/// any face that has a character still left. At most four faces.
fn pick(lib: &FontLibrary, preferred: &[&str], chars: &[char]) -> Vec<Arc<darkburn_text::FaceData>> {
    let mut left = chars.to_vec();
    let mut ids = Vec::new();
    while !left.is_empty() && ids.len() < 4 {
        let mut best = None;
        for family in preferred {
            if let Some(face) = lib.resolve(family, false, false) {
                let n = lib.glyph_count(face.id, &left);
                if n > 0 && best.is_none_or(|(_, m)| n > m) {
                    best = Some((face.id, n));
                }
            }
        }
        let id = match best.map(|(id, _)| id).or_else(|| lib.fallback_for(left[0])) {
            Some(id) => id,
            None => {
                // No face on this computer has it: it stays an empty box.
                left.remove(0);
                continue;
            }
        };
        let had = left.len();
        left.retain(|c| lib.glyph_count(id, &[*c]) == 0);
        if left.len() == had {
            left.remove(0);
        }
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    ids.into_iter().filter_map(|id| lib.face_data(id)).collect()
}

/// The faces a language needs on this computer (none for a script Plex draws).
fn faces_for(language: &Language) -> Vec<Arc<darkburn_text::FaceData>> {
    if language.script.preferred_fonts().is_empty() {
        return Vec::new();
    }
    let missing = undrawable(darkburn_ui::fonts::base_font_definitions(), &language_characters(language));
    if missing.is_empty() {
        return Vec::new();
    }
    pick(system(), language.script.preferred_fonts(), &missing)
}

/// The faces for the menu names of the offered languages.
fn faces_for_names() -> Vec<Arc<darkburn_text::FaceData>> {
    let mut out: Vec<Arc<darkburn_text::FaceData>> = Vec::new();
    let mut base: Option<Fonts> = None;
    for l in languages::offered().filter(|l| !l.script.preferred_fonts().is_empty()) {
        let fonts = base.get_or_insert_with(|| Fonts::new(TextOptions::default(), darkburn_ui::fonts::base_font_definitions()));
        let lack = missing(fonts, &characters(l.name));
        if !lack.is_empty() {
            for f in pick(system(), l.script.preferred_fonts(), &lack) {
                if !out.iter().any(|g| Arc::ptr_eq(g, &f)) {
                    out.push(f);
                }
            }
        }
    }
    out
}

/// What to add for `code` (and the menu names when `names`), worked out once per process.
fn wanted(code: &str, names: bool) -> Vec<Face> {
    type Cache = Mutex<HashMap<(String, bool), Arc<Vec<Arc<darkburn_text::FaceData>>>>>;
    static CACHE: OnceLock<Cache> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let key = (code.to_string(), names);
    let hit = cache.lock().unwrap_or_else(|e| e.into_inner()).get(&key).cloned();
    let faces = match hit {
        Some(f) => f,
        None => {
            let mut faces = languages::find(code).map(faces_for).unwrap_or_default();
            if names {
                for f in faces_for_names() {
                    if !faces.iter().any(|g| Arc::ptr_eq(g, &f)) {
                        faces.push(f);
                    }
                }
            }
            let faces = Arc::new(faces);
            cache.lock().unwrap_or_else(|e| e.into_inner()).insert(key, faces.clone());
            faces
        }
    };
    faces
        .iter()
        .map(|f| {
            let name = format!("system-{:p}", Arc::as_ptr(f));
            let index = f.index;
            (name, FontData::from_blob(f.clone(), index))
        })
        .collect()
}

/// The interface font definitions for `code`: the bundled ones with the system faces it needs.
#[cfg(test)]
fn definitions_for(code: &str) -> FontDefinitions {
    darkburn_ui::fonts::with_fallbacks(darkburn_ui::fonts::base_font_definitions(), wanted(code, false))
}

fn names_id() -> egui::Id {
    egui::Id::unique("darkburn_language_names_wanted")
}

fn installed_id() -> egui::Id {
    egui::Id::unique("darkburn_language_fonts_installed")
}

/// The Language menu is open: its names need their fonts from the next frame on.
pub fn want_language_names(ctx: &egui::Context) {
    ctx.data_mut(|d| d.insert_temp(names_id(), true));
}

/// Once a frame: the faces for `code` (and, after [`want_language_names`], the menu names),
/// installed when they changed.
pub fn frame(ctx: &egui::Context, code: &str) {
    let names = ctx.data(|d| d.get_temp::<bool>(names_id())).unwrap_or(false);
    let faces = if code == "en" && !names { Vec::new() } else { wanted(code, names) };
    let want: Vec<String> = faces.iter().map(|(n, _)| n.clone()).collect();
    let have = ctx.data(|d| d.get_temp::<Arc<Vec<String>>>(installed_id())).unwrap_or_default();
    if *have != want {
        ctx.data_mut(|d| d.insert_temp(installed_id(), Arc::new(want)));
        darkburn_ui::fonts::setup_fonts_with_fallbacks(ctx, faces);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::languages::{LANGUAGES, Script};

    /// One language per script; the check runs for each that has its file.
    const ONE_PER_SCRIPT: &[&str] = &["nb", "el", "ru", "ja", "ko", "zh-CN", "zh-TW", "ar"];

    #[test]
    fn one_language_per_script_is_checked() {
        let mut scripts: Vec<Script> = ONE_PER_SCRIPT.iter().map(|c| languages::find(c).unwrap().script).collect();
        scripts.dedup();
        assert_eq!(scripts.len(), ONE_PER_SCRIPT.len());
        for l in LANGUAGES {
            assert!(scripts.contains(&l.script), "{:?} of {} is not checked", l.script, l.code);
        }
    }

    /// Every character of a language's file has a glyph in the fonts loaded for that language.
    /// A language without a file is skipped until it has one. Japanese, Korean and Chinese are
    /// drawn with the computer's own fonts, so a computer without any for the script fails here.
    #[test]
    fn every_character_of_a_locale_file_has_a_glyph_in_the_fonts_loaded_for_it() {
        for code in ONE_PER_SCRIPT.iter().copied().chain(languages::files().iter().map(|(c, _)| *c)) {
            let Some(language) = languages::find(code) else { continue };
            if languages::file(code).is_none() {
                eprintln!("{code}: no file yet, skipped");
                continue;
            }
            let chars = language_characters(language);
            let missing = undrawable(definitions_for(code), &chars);
            assert!(
                missing.is_empty(),
                "{code}: no glyph for {} characters in the fonts loaded for it, e.g. {:?}{}",
                missing.len(),
                missing.iter().take(12).collect::<String>(),
                if language.script.preferred_fonts().is_empty() {
                    String::new()
                } else {
                    format!(" (looked for {:?} among the system fonts)", language.script.preferred_fonts())
                }
            );
        }
    }

    #[test]
    fn plex_draws_latin_greek_and_cyrillic_and_not_the_rest() {
        let base = darkburn_ui::fonts::base_font_definitions();
        let latin = characters("\u{c5}\u{c6}\u{d8}\u{10c}\u{161}\u{17e}\u{151}\u{171}\u{142}\u{11f}\u{131}\u{15f}\u{f1}\u{e7}\u{df}\u{2026}");
        let greek = characters("\u{395}\u{3bb}\u{3bb}\u{3b7}\u{3bd}\u{3b9}\u{3ba}\u{3ac}");
        let cyrillic = characters("\u{420}\u{443}\u{441}\u{441}\u{43a}\u{438}\u{439}\u{451}\u{44b}");
        assert!(undrawable(base.clone(), &latin).is_empty());
        assert!(undrawable(base.clone(), &greek).is_empty());
        assert!(undrawable(base.clone(), &cyrillic).is_empty());
        for name in ["\u{65e5}\u{672c}\u{8a9e}", "\u{d55c}\u{ad6d}\u{c5b4}", "\u{7b80}\u{4f53}", "\u{627}\u{644}\u{639}\u{631}\u{628}"] {
            let chars = characters(name);
            assert!(!undrawable(base.clone(), &chars).is_empty(), "{name} is drawn by the bundled fonts");
        }
    }

    #[test]
    fn a_latin_language_needs_no_system_font() {
        assert!(wanted("en", false).is_empty());
        assert!(wanted("nb", false).is_empty());
        assert!(wanted("xx", false).is_empty());
    }

    #[test]
    fn faces_are_picked_from_the_preferred_families_first() {
        // The two bundled faces as a stand-in for a computer's fonts.
        let mut lib = FontLibrary::new();
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates/darkburn-text/tests/fonts");
        lib.load_font_file(dir.join("IBMPlexMono-Regular.ttf")).unwrap();
        lib.load_font_file(dir.join("IBMPlexSans-Variable.ttf")).unwrap();
        let mono = lib.resolve("IBM Plex Mono", false, false).unwrap().id;
        let picked = pick(&lib, &["Not On This Computer", "IBM Plex Mono"], &['A', 'B']);
        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0].data, lib.face_data(mono).unwrap().data);
        // Nothing draws it: nothing is picked, and the search ends.
        assert!(pick(&lib, &["IBM Plex Mono"], &['\u{65e5}']).is_empty());
    }

    #[test]
    fn the_harness_installs_nothing_extra_for_english() {
        let mut g = crate::ui_harness::Gui::standard();
        g.frames(2);
        let installed = g.ctx().data(|d| d.get_temp::<Arc<Vec<String>>>(installed_id())).unwrap_or_default();
        assert!(installed.is_empty(), "{installed:?}");
    }

    /// egui 0.37 shapes each run of one font (Arabic letters join), but lays the runs out left to
    /// right in the order they are written. In an Arabic phrase the space comes from the interface
    /// font, so each word is its own run and the words come out in the wrong order. That is why
    /// Arabic is not offered. When this test fails, egui orders right-to-left text: look again.
    #[test]
    fn egui_lays_out_arabic_words_left_to_right_so_arabic_is_not_offered() {
        let (first, second) = ("\u{646}\u{639}\u{645}", "\u{634}\u{643}\u{631}\u{627}");
        let phrase = format!("{first} {second}");
        let missing = undrawable(darkburn_ui::fonts::base_font_definitions(), &characters(&phrase));
        let faces = pick(system(), Script::Arabic.preferred_fonts(), &missing);
        if faces.is_empty() {
            eprintln!("no Arabic font on this computer: skipped");
            return;
        }
        let extra = faces.into_iter().enumerate().map(|(i, f)| (format!("arabic-{i}"), FontData::from_blob(f.clone(), f.index))).collect::<Vec<_>>();
        let mut fonts = Fonts::new(TextOptions::default(), darkburn_ui::fonts::with_fallbacks(darkburn_ui::fonts::base_font_definitions(), extra));
        let galley = fonts.with_pixels_per_point(1.0).layout_no_wrap(phrase, FontId::proportional(14.0), egui::Color32::WHITE);
        let row = &galley.rows[0];
        let x_of = |word: &str| {
            let xs: Vec<f32> = row.glyphs.iter().filter(|g| word.contains(g.chr)).map(|g| g.pos.x).collect();
            assert!(!xs.is_empty(), "no glyph of {word} in {:?}", row.glyphs.iter().map(|g| g.chr).collect::<String>());
            xs.iter().copied().fold(f32::INFINITY, f32::min)
        };
        assert!(x_of(first) < x_of(second), "the first word is on the left: egui does not lay out right to left");
        assert!(!languages::is_offered("ar"));
    }
}
