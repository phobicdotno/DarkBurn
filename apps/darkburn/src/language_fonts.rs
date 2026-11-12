//! System fonts for the interface languages the bundled interface font cannot draw.
//!
//! IBM Plex Sans draws Latin, Greek and Cyrillic. For Japanese, Korean and Chinese the program
//! looks among the fonts the computer already has (the same font list the Text tool uses,
//! `darkburn_text::FontLibrary`) for faces that draw the characters of the language's file, and
//! puts them right after Plex in every interface family. Nothing is added to the program itself.
//!
//! The faces follow the language in use, plus, while the Language menu is open, the faces the
//! names in that menu need; when the menu closes they leave the interface families again, and
//! their bytes are let go of. The system font list is read once, the first time a language needs
//! it; with only Latin, Greek and Cyrillic languages it is never read. Which faces a language
//! needs is worked out once per run, on a thread of its own, so the window never waits for it:
//! the text shows as boxes for the moment it takes.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, Weak};

use darkburn_text::{FaceData, FaceId, FontLibrary};
use egui::epaint::text::{FontData, FontDefinitions, Fonts, TextOptions};
use egui::{FontFamily, FontId};

use crate::languages::{self, Language};

/// One face to add: a name for the font definitions and the face.
type Face = (String, FontData);

/// The faces picked for a language, in order.
type Picked = Arc<Vec<FaceId>>;

/// At most this many system faces for one language.
const MAX_FACES: usize = 4;

/// Searches through all the computer's faces for a character that find nothing, before the search
/// stops. Each such search reads every face, so without a font for the script one search per
/// character of the file would hold everything up for a long time.
const MAX_MISSES: usize = 2;

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

/// Where [`pick`] looks for faces: the computer's fonts, or a stand-in in the tests.
trait FaceSource {
    type Id: Copy + PartialEq;
    /// The regular face of a family, if the computer has the family.
    fn family(&self, name: &str) -> Option<Self::Id>;
    /// For each of `chars`, whether face `id` draws it.
    fn glyphs(&self, id: Self::Id, chars: &[char]) -> Vec<bool>;
    /// Any face that draws `c`: a search through every face.
    fn any_with(&self, c: char) -> Option<Self::Id>;
}

impl FaceSource for FontLibrary {
    type Id = FaceId;
    fn family(&self, name: &str) -> Option<FaceId> {
        self.resolve(name, false, false).map(|f| f.id)
    }
    fn glyphs(&self, id: FaceId, chars: &[char]) -> Vec<bool> {
        FontLibrary::glyphs(self, id, chars)
    }
    fn any_with(&self, c: char) -> Option<FaceId> {
        self.fallback_for(c)
    }
}

/// Faces of `lib` that together draw as much of `chars` as the computer can: first the
/// families of `preferred` (the one that draws the most of what is left, earlier on a tie), then
/// any face that has a character still left. At most [`MAX_FACES`] faces, and the search for any
/// face gives up after [`MAX_MISSES`] characters no face has.
fn pick<S: FaceSource>(lib: &S, preferred: &[&str], chars: &[char]) -> Vec<S::Id> {
    let mut left = chars.to_vec();
    let mut ids: Vec<S::Id> = Vec::new();
    let mut misses = 0;
    while !left.is_empty() && ids.len() < MAX_FACES {
        let mut best: Option<(S::Id, usize)> = None;
        for family in preferred {
            if let Some(id) = lib.family(family)
                && !ids.contains(&id)
            {
                let n = lib.glyphs(id, &left).into_iter().filter(|has| *has).count();
                if n > 0 && best.is_none_or(|(_, m)| n > m) {
                    best = Some((id, n));
                }
            }
        }
        let id = match best {
            Some((id, _)) => id,
            None if misses >= MAX_MISSES => break,
            None => match lib.any_with(left[0]) {
                Some(id) => id,
                None => {
                    // No face on this computer has it: it stays an empty box.
                    misses += 1;
                    left.remove(0);
                    continue;
                }
            },
        };
        let has = lib.glyphs(id, &left);
        let had = left.len();
        let mut drawn = has.into_iter();
        left.retain(|_| !drawn.next().unwrap_or(false));
        if left.len() == had {
            left.remove(0);
        }
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    ids
}

/// The faces a language needs on this computer (none for a script Plex draws).
fn faces_for(language: &Language) -> Vec<FaceId> {
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
fn faces_for_names() -> Vec<FaceId> {
    let mut out: Vec<FaceId> = Vec::new();
    let mut base: Option<Fonts> = None;
    for l in languages::offered().filter(|l| !l.script.preferred_fonts().is_empty()) {
        let fonts = base.get_or_insert_with(|| Fonts::new(TextOptions::default(), darkburn_ui::fonts::base_font_definitions()));
        let lack = missing(fonts, &characters(l.name));
        if !lack.is_empty() {
            for f in pick(system(), l.script.preferred_fonts(), &lack) {
                if !out.contains(&f) {
                    out.push(f);
                }
            }
        }
    }
    out
}

/// The faces for `code` (and the menu names when `names`), worked out now.
fn work_out(code: &str, names: bool) -> Vec<FaceId> {
    let mut faces = languages::find(code).map(faces_for).unwrap_or_default();
    if names {
        for f in faces_for_names() {
            if !faces.contains(&f) {
                faces.push(f);
            }
        }
    }
    faces
}

/// The faces for `code` (and the menu names when `names`) once they are worked out. The first
/// time they are worked out on a thread of their own, which asks `ctx` for a frame when done;
/// until then `None`.
fn picked(ctx: &egui::Context, code: &str, names: bool) -> Option<Picked> {
    type Slots = Mutex<HashMap<(String, bool), Option<Picked>>>;
    static SLOTS: OnceLock<Slots> = OnceLock::new();
    let slots = SLOTS.get_or_init(Default::default);
    let key = (code.to_string(), names);
    {
        let mut s = slots.lock().unwrap_or_else(|e| e.into_inner());
        match s.get(&key) {
            Some(slot) => return slot.clone(),
            None => {
                s.insert(key.clone(), None);
            }
        }
    }
    let ctx = ctx.clone();
    let work = move || {
        let faces = Arc::new(work_out(&key.0, key.1));
        slots.lock().unwrap_or_else(|e| e.into_inner()).insert(key, Some(faces));
        ctx.request_repaint();
    };
    if let Err(e) = std::thread::Builder::new().name("language-fonts".into()).spawn(work) {
        log::warn!("no thread for the language fonts: {e}");
    }
    None
}

/// The bytes of face `id`, shared with the interface fonts while they use them and read from
/// the file again when they have let go of them.
fn bytes(id: FaceId) -> Option<Arc<FaceData>> {
    static LOADED: OnceLock<Mutex<HashMap<FaceId, Weak<FaceData>>>> = OnceLock::new();
    let mut loaded = LOADED.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    if let Some(f) = loaded.get(&id).and_then(Weak::upgrade) {
        return Some(f);
    }
    let f = Arc::new(system().read_face_data(id)?);
    loaded.retain(|_, w| w.strong_count() > 0);
    loaded.insert(id, Arc::downgrade(&f));
    Some(f)
}

/// The faces `ids` of the computer, ready for the font definitions.
fn faces(ids: &[FaceId]) -> Vec<Face> {
    ids.iter()
        .filter_map(|id| {
            let f = bytes(*id)?;
            let index = f.index;
            Some((format!("system-{id:?}"), FontData::from_blob(f, index)))
        })
        .collect()
}

/// The interface font definitions for `code`: the bundled ones with the system faces it needs.
#[cfg(test)]
fn definitions_for(code: &str) -> FontDefinitions {
    darkburn_ui::fonts::with_fallbacks(darkburn_ui::fonts::base_font_definitions(), faces(&work_out(code, false)))
}

fn names_id() -> egui::Id {
    egui::Id::unique("darkburn_language_names_wanted")
}

fn installed_id() -> egui::Id {
    egui::Id::unique("darkburn_language_fonts_installed")
}

/// The Language menu is open this frame: its names need their fonts. Called every frame the
/// menu is drawn; the frame after it is not, they go again.
pub fn want_language_names(ctx: &egui::Context) {
    ctx.data_mut(|d| d.insert_temp(names_id(), true));
}

/// Once a frame, before anything is drawn: the faces for `code` (and, while the Language menu is
/// open, the menu names), installed when they changed. Faces still being worked out leave the
/// fonts as they are for now.
pub fn frame(ctx: &egui::Context, code: &str) {
    let names = ctx.data_mut(|d| d.remove_temp::<bool>(names_id())).unwrap_or(false);
    let want = if code == "en" && !names {
        Picked::default()
    } else {
        match picked(ctx, code, names) {
            Some(p) => p,
            None => return,
        }
    };
    let have = ctx.data(|d| d.get_temp::<Picked>(installed_id())).unwrap_or_default();
    if *have != *want {
        ctx.data_mut(|d| d.insert_temp(installed_id(), want.clone()));
        darkburn_ui::fonts::setup_fonts_with_fallbacks(ctx, faces(&want));
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

    /// The names of Japanese, Korean, Chinese and Arabic in themselves get a glyph for every
    /// character from the system fonts picked for them. A script with no font on this computer
    /// is skipped (CI installs fonts-noto-cjk).
    #[test]
    fn the_system_fonts_picked_for_a_script_draw_its_language_name() {
        for l in LANGUAGES.iter().filter(|l| !l.script.preferred_fonts().is_empty()) {
            let chars = characters(l.name);
            let lack = undrawable(darkburn_ui::fonts::base_font_definitions(), &chars);
            assert!(!lack.is_empty(), "{} is drawn without a system font", l.code);
            let ids = pick(system(), l.script.preferred_fonts(), &lack);
            if ids.is_empty() {
                eprintln!("{}: no font for {:?} on this computer, skipped", l.code, l.script);
                continue;
            }
            assert!(ids.len() <= MAX_FACES);
            let defs = darkburn_ui::fonts::with_fallbacks(darkburn_ui::fonts::base_font_definitions(), faces(&ids));
            assert_eq!(undrawable(defs, &chars), Vec::<char>::new(), "{}", l.code);
        }
    }

    #[test]
    fn a_latin_language_needs_no_system_font() {
        assert!(work_out("en", false).is_empty());
        assert!(work_out("nb", false).is_empty());
        assert!(work_out("xx", false).is_empty());
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
        assert_eq!(picked, vec![mono]);
        // Nothing draws it: nothing is picked, and the search ends.
        assert!(pick(&lib, &["IBM Plex Mono"], &['\u{65e5}']).is_empty());
    }

    #[test]
    fn the_harness_installs_nothing_extra_for_english() {
        let mut g = crate::ui_harness::Gui::standard();
        g.frames(2);
        assert!(installed(&g.ctx()).is_empty(), "{:?}", installed(&g.ctx()));
    }

    fn installed(ctx: &egui::Context) -> Picked {
        ctx.data(|d| d.get_temp::<Picked>(installed_id())).unwrap_or_default()
    }

    /// A stand-in for a computer's fonts: named faces with the characters they draw, and a count
    /// of the searches through all of them.
    struct Stand {
        faces: Vec<(&'static str, Vec<char>)>,
        searches: std::cell::Cell<usize>,
    }

    impl Stand {
        fn new(faces: Vec<(&'static str, Vec<char>)>) -> Self {
            Self { faces, searches: std::cell::Cell::new(0) }
        }
    }

    impl FaceSource for Stand {
        type Id = usize;
        fn family(&self, name: &str) -> Option<usize> {
            self.faces.iter().position(|(n, _)| *n == name)
        }
        fn glyphs(&self, id: usize, chars: &[char]) -> Vec<bool> {
            chars.iter().map(|c| self.faces[id].1.contains(c)).collect()
        }
        fn any_with(&self, c: char) -> Option<usize> {
            self.searches.set(self.searches.get() + 1);
            self.faces.iter().position(|(_, cs)| cs.contains(&c))
        }
    }

    /// A Japanese file has well over a thousand different characters. On a computer with no font
    /// for them, every search through all its faces finds nothing; the picking gives up after a
    /// couple of them instead of one search per character [review of I18N-04].
    #[test]
    fn a_computer_without_a_font_for_the_script_is_searched_only_a_few_times() {
        let lib = Stand::new(vec![("Latin Sans", ('A'..='z').collect()), ("Symbols", vec!['\u{2192}', '\u{25b2}'])]);
        let kanji: Vec<char> = (0..1500).filter_map(|i| char::from_u32(0x4e00 + i)).collect();
        assert!(pick(&lib, &["Yu Gothic UI", "Noto Sans CJK JP"], &kanji).is_empty());
        assert!(lib.searches.get() <= MAX_MISSES, "{} searches through every face", lib.searches.get());
    }

    #[test]
    fn picking_takes_the_preferred_family_that_draws_most_then_any_face_that_draws_the_rest() {
        let lib =
            Stand::new(vec![("Small", vec!['\u{65e5}']), ("Big", vec!['\u{65e5}', '\u{672c}']), ("Other", vec!['\u{8a9e}']), ("Nothing", vec![])]);
        let chars = ['\u{65e5}', '\u{672c}', '\u{8a9e}'];
        assert_eq!(pick(&lib, &["Missing", "Small", "Big"], &chars), vec![1, 2]);
        assert_eq!(lib.searches.get(), 1);
        // A character no face has is searched for once, and the rest are still drawn.
        let lib = Stand::new(vec![("Big", vec!['\u{65e5}']), ("Other", vec!['\u{8a9e}'])]);
        assert_eq!(pick(&lib, &["Big"], &['\u{65e5}', '\u{672c}', '\u{8a9e}']), vec![0, 1]);
        assert_eq!(lib.searches.get(), 2);
        // Never more than four faces.
        let many: Vec<(&'static str, Vec<char>)> = ["a", "b", "c", "d", "e", "f"].iter().zip('\u{4e00}'..).map(|(n, c)| (*n, vec![c])).collect();
        let chars: Vec<char> = many.iter().map(|(_, c)| c[0]).collect();
        assert_eq!(pick(&Stand::new(many), &[], &chars).len(), MAX_FACES);
    }

    /// One pass of an egui context, as a frame of the program has.
    fn pass(ctx: &egui::Context) {
        let _ = ctx.run_ui(egui::RawInput::default(), |_| {});
    }

    /// The whole way for a language the interface font cannot draw: [`frame`] works out its
    /// faces on the side, puts them into the context, and the context then draws every character
    /// of the language; back in English they are gone again. Skipped on a computer with no
    /// Japanese font (CI installs one).
    #[test]
    fn a_language_that_needs_system_fonts_gets_them_installed_and_they_draw_its_text() {
        let ja = languages::find("ja").unwrap();
        let chars = language_characters(ja);
        let lack = undrawable(darkburn_ui::fonts::base_font_definitions(), &chars);
        assert!(!lack.is_empty());
        if pick(system(), ja.script.preferred_fonts(), &lack).is_empty() {
            eprintln!("no Japanese font on this computer: skipped");
            return;
        }
        let ctx = egui::Context::default();
        darkburn_ui::fonts::setup_fonts_with_fallbacks(&ctx, Vec::new());
        pass(&ctx);
        let font = FontId::proportional(14.0);
        let drawn = |ctx: &egui::Context| ctx.fonts_mut(|f| chars.iter().copied().filter(|c| f.has_glyph(&font, *c)).count());
        assert_eq!(drawn(&ctx), chars.len() - lack.len());
        let start = std::time::Instant::now();
        while installed(&ctx).is_empty() {
            frame(&ctx, "ja");
            pass(&ctx);
            assert!(start.elapsed() < std::time::Duration::from_secs(120), "the Japanese faces never arrived");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        pass(&ctx);
        assert_eq!(drawn(&ctx), chars.len(), "{:?} with {:?}", ja.name, installed(&ctx));
        frame(&ctx, "en");
        pass(&ctx);
        pass(&ctx);
        assert!(installed(&ctx).is_empty());
        assert_eq!(drawn(&ctx), chars.len() - lack.len(), "the system faces stayed after English was chosen again");
    }

    /// The faces for the names in the Language menu are wanted while the menu is open and not
    /// after it closes, so they do not stay in the interface fonts for the rest of the session
    /// [review of I18N-04].
    #[test]
    fn the_menu_names_are_wanted_only_while_the_language_menu_is_open() {
        let mut g = crate::ui_harness::Gui::standard();
        let wanted = |g: &crate::ui_harness::Gui| g.ctx().data(|d| d.get_temp::<bool>(names_id())).unwrap_or(false);
        g.frames(2);
        assert!(!wanted(&g));
        g.open_menu_only("Language");
        g.frames(2);
        assert!(wanted(&g), "the open menu asks for the fonts of its names");
        g.escape();
        g.frames(2);
        assert!(!wanted(&g), "the menu is closed but its names still want their fonts");
        // In a bare context: one frame takes the wish, the next has none.
        let ctx = egui::Context::default();
        want_language_names(&ctx);
        frame(&ctx, "en");
        assert!(ctx.data(|d| d.get_temp::<bool>(names_id())).is_none());
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
        let ids = pick(system(), Script::Arabic.preferred_fonts(), &missing);
        if ids.is_empty() {
            eprintln!("no Arabic font on this computer: skipped");
            return;
        }
        let extra = faces(&ids);
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
