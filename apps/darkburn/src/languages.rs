//! The interface languages, in one table. Adding a language is a file and, for a code that is not
//! in [`LANGUAGES`] yet, one line there:
//!
//! - `assets/locales/<code>.json` holds the translated text (a flat key to text object, keys as in
//!   `en.json`). `build.rs` embeds every file in that folder, so nothing else names the file.
//! - The language shows in the Language menu once its file is there. A key the file lacks shows
//!   its English text.
//! - A script the interface font (IBM Plex Sans: Latin, Greek, Cyrillic) cannot draw is drawn
//!   with a font the computer already has (`language_fonts.rs`), looked for in the order of
//!   [`Script::preferred_fonts`].
//! - A right-to-left language is not offered: egui lays out every line left to right (see
//!   `docs/research/translation-glossary.md`).

mod files {
    include!(concat!(env!("OUT_DIR"), "/locale_files.rs"));
}

/// The writing system of a language: what the interface font has to draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Script {
    Latin,
    Greek,
    Cyrillic,
    Japanese,
    Korean,
    SimplifiedChinese,
    TraditionalChinese,
    Arabic,
}

impl Script {
    /// Written right to left.
    pub fn right_to_left(self) -> bool {
        matches!(self, Script::Arabic)
    }

    /// System font families to draw this script with, best first, when the interface font lacks
    /// it: Windows, then macOS, then the usual Linux packages. The first one on the computer that
    /// draws the most of the text is used; failing all of them, any face that has the glyph.
    pub fn preferred_fonts(self) -> &'static [&'static str] {
        match self {
            Script::Latin | Script::Greek | Script::Cyrillic => &[],
            Script::Japanese => &[
                "Yu Gothic UI",
                "Yu Gothic",
                "Meiryo UI",
                "Meiryo",
                "MS UI Gothic",
                "Hiragino Sans",
                "Hiragino Kaku Gothic ProN",
                "Noto Sans CJK JP",
                "Noto Sans JP",
                "Source Han Sans JP",
                "IPAexGothic",
                "IPAGothic",
                "TakaoGothic",
                "VL Gothic",
            ],
            Script::Korean => &[
                "Malgun Gothic",
                "Apple SD Gothic Neo",
                "Noto Sans CJK KR",
                "Noto Sans KR",
                "Source Han Sans KR",
                "NanumGothic",
                "Nanum Gothic",
                "UnDotum",
            ],
            Script::SimplifiedChinese => &[
                "Microsoft YaHei UI",
                "Microsoft YaHei",
                "PingFang SC",
                "Hiragino Sans GB",
                "Noto Sans CJK SC",
                "Noto Sans SC",
                "Source Han Sans SC",
                "WenQuanYi Micro Hei",
                "WenQuanYi Zen Hei",
                "SimHei",
            ],
            Script::TraditionalChinese => &[
                "Microsoft JhengHei UI",
                "Microsoft JhengHei",
                "PingFang TC",
                "Noto Sans CJK TC",
                "Noto Sans TC",
                "Source Han Sans TC",
                "AR PL UMing TW",
                "PMingLiU",
            ],
            Script::Arabic => &["Segoe UI", "Geeza Pro", "Noto Sans Arabic", "Noto Naskh Arabic", "DejaVu Sans", "Tahoma", "Arial"],
        }
    }
}

/// One interface language.
#[derive(Clone, Copy, Debug)]
pub struct Language {
    /// The code: the file is `assets/locales/<code>.json` and the command `lang.<code>`.
    pub code: &'static str,
    /// The name in the language itself, as the Language menu shows it.
    pub name: &'static str,
    pub script: Script,
}

const fn lang(code: &'static str, name: &'static str, script: Script) -> Language {
    Language { code, name, script }
}

use Script::*;

/// Every interface language, in the order of the Language menu. English is the program's own
/// text; the others need their file to be shown.
pub const LANGUAGES: &[Language] = &[
    lang("en", "English", Latin),
    lang("ar", "\u{627}\u{644}\u{639}\u{631}\u{628}\u{64a}\u{629}", Arabic),
    lang("cs", "\u{10c}e\u{161}tina", Latin),
    lang("da", "Dansk", Latin),
    lang("de", "Deutsch", Latin),
    lang("el", "\u{395}\u{3bb}\u{3bb}\u{3b7}\u{3bd}\u{3b9}\u{3ba}\u{3ac}", Greek),
    lang("es-ES", "Espa\u{f1}ol (Espa\u{f1}a)", Latin),
    lang("es-LA", "Espa\u{f1}ol (Latinoam\u{e9}rica)", Latin),
    lang("fi", "Suomi", Latin),
    lang("fr", "Fran\u{e7}ais", Latin),
    lang("hu", "Magyar", Latin),
    lang("it", "Italiano", Latin),
    lang("ja", "\u{65e5}\u{672c}\u{8a9e}", Japanese),
    lang("ko", "\u{d55c}\u{ad6d}\u{c5b4}", Korean),
    lang("nb", "Norsk bokm\u{e5}l", Latin),
    lang("nl", "Nederlands", Latin),
    lang("pl", "Polski", Latin),
    lang("pt", "Portugu\u{ea}s", Latin),
    lang("ru", "\u{420}\u{443}\u{441}\u{441}\u{43a}\u{438}\u{439}", Cyrillic),
    lang("sl", "Sloven\u{161}\u{10d}ina", Latin),
    lang("sv", "Svenska", Latin),
    lang("tr", "T\u{fc}rk\u{e7}e", Latin),
    lang("zh-CN", "\u{7b80}\u{4f53}\u{4e2d}\u{6587}", SimplifiedChinese),
    lang("zh-TW", "\u{7e41}\u{9ad4}\u{4e2d}\u{6587}", TraditionalChinese),
];

/// The language with this code.
pub fn find(code: &str) -> Option<&'static Language> {
    LANGUAGES.iter().find(|l| l.code == code)
}

/// Every translation file there is, by code.
pub fn files() -> &'static [(&'static str, &'static str)] {
    files::LOCALE_FILES
}

/// The text of `assets/locales/<code>.json`, when there is such a file.
pub fn file(code: &str) -> Option<&'static str> {
    files().iter().find(|(c, _)| *c == code).map(|(_, text)| *text)
}

/// Whether `language` goes in the Language menu: it has text, and egui can lay out its script.
fn offered_with(language: &Language, has_file: bool) -> bool {
    has_file && !language.script.right_to_left()
}

/// The languages of the Language menu, in its order.
pub fn offered() -> impl Iterator<Item = &'static Language> {
    LANGUAGES.iter().filter(|l| offered_with(l, file(l.code).is_some()))
}

/// Whether `code` is a language of the Language menu.
pub fn is_offered(code: &str) -> bool {
    offered().any(|l| l.code == code)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn locale_dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/locales")
    }

    #[test]
    fn the_table_has_english_norwegian_and_the_other_twenty_two() {
        let codes: Vec<&str> = LANGUAGES.iter().map(|l| l.code).collect();
        let want = [
            "en", "nb", "ar", "cs", "da", "de", "el", "es-ES", "es-LA", "fi", "fr", "hu", "it", "ja", "ko", "nl", "pl", "pt", "ru", "sl", "sv", "tr",
            "zh-CN", "zh-TW",
        ];
        assert_eq!(codes.len(), want.len(), "{codes:?}");
        for w in want {
            assert!(codes.contains(&w), "{w} is missing");
        }
        let mut seen = std::collections::HashSet::new();
        for l in LANGUAGES {
            assert!(seen.insert(l.code), "{} twice", l.code);
            assert!(!l.name.trim().is_empty(), "{} has no name", l.code);
            assert!(l.code.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'), "{}", l.code);
        }
        assert_eq!(LANGUAGES[0].code, "en", "English comes first");
    }

    #[test]
    fn every_file_in_the_locale_folder_is_embedded_and_has_a_line_in_the_table() {
        let mut on_disk = Vec::new();
        for entry in std::fs::read_dir(locale_dir()).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) == Some("json") {
                on_disk.push(path.file_stem().unwrap().to_str().unwrap().to_string());
            }
        }
        on_disk.sort();
        let embedded: Vec<String> = files().iter().map(|(c, _)| c.to_string()).collect();
        assert_eq!(embedded, on_disk, "build.rs embeds every file in assets/locales");
        for code in &on_disk {
            assert!(find(code).is_some(), "assets/locales/{code}.json has no line in languages::LANGUAGES");
        }
        assert!(file("en").is_some() && file("nb").is_some());
    }

    #[test]
    fn only_languages_with_a_file_are_offered() {
        for l in LANGUAGES {
            assert_eq!(is_offered(l.code), file(l.code).is_some() && !l.script.right_to_left(), "{}", l.code);
        }
        assert!(is_offered("en") && is_offered("nb"));
        assert!(!is_offered("xx"));
        // A language without a file stays out of the menu, also when its line is there.
        let de = find("de").unwrap();
        assert!(!offered_with(de, false));
        assert!(offered_with(de, true));
    }

    #[test]
    fn a_right_to_left_language_is_not_offered_even_with_a_file() {
        let ar = find("ar").unwrap();
        assert!(ar.script.right_to_left());
        assert!(!offered_with(ar, true));
        assert!(!is_offered("ar"));
        for l in LANGUAGES.iter().filter(|l| l.code != "ar") {
            assert!(!l.script.right_to_left(), "{}", l.code);
        }
    }

    #[test]
    fn scripts_the_interface_font_lacks_have_system_fonts_to_try() {
        for l in LANGUAGES {
            let needs = matches!(l.script, Japanese | Korean | SimplifiedChinese | TraditionalChinese | Arabic);
            assert_eq!(!l.script.preferred_fonts().is_empty(), needs, "{}", l.code);
        }
    }
}
