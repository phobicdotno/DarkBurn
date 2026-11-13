//! Upper and lower case that follow the language of the text.
//!
//! Rust's own case mapping follows no language. Turkish and Azerbaijani write a dotted and a
//! dotless i as two letters (i and İ, ı and I), so there "Birimler" must become "BİRİMLER", not
//! "BIRIMLER". A text not translated yet is still English in a Turkish interface, and English
//! "Settings" stays "SETTINGS". Captions that change case go through this module, which asks the
//! program which language a text is written in.

use std::sync::OnceLock;

static SOURCE: OnceLock<fn(&str) -> String> = OnceLock::new();

/// Tell the widget kit how to find the language a shown text is written in (a code such as "tr"
/// or "en"): the interface language for a translated text, English for one not translated yet.
/// The first source given is kept; until one is given the case mapping is the plain one.
pub fn set_text_language(source: fn(&str) -> String) {
    let _ = SOURCE.set(source);
}

/// Whether `language` writes a dotted capital İ and a dotless small ı.
fn dotted_i(language: &str) -> bool {
    matches!(language.split(['-', '_']).next().unwrap_or_default(), "tr" | "az")
}

/// `text` in capitals, as the language it is written in writes them.
pub fn upper(text: &str) -> String {
    // Only a small i maps differently, so most captions never ask for their language.
    match SOURCE.get() {
        Some(language_of) if text.contains('i') => upper_in(&language_of(text), text),
        _ => text.to_uppercase(),
    }
}

/// `text` in capitals, as `language` writes them.
pub fn upper_in(language: &str, text: &str) -> String {
    if !dotted_i(language) {
        return text.to_uppercase();
    }
    let mut out = String::with_capacity(text.len() + 2);
    for c in text.chars() {
        match c {
            'i' => out.push('İ'),
            'ı' => out.push('I'),
            c => out.extend(c.to_uppercase()),
        }
    }
    out
}

/// `text` in small letters, as `language` writes them.
pub fn lower_in(language: &str, text: &str) -> String {
    let dotted = dotted_i(language);
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            'I' if dotted => out.push('ı'),
            // Plain lower case turns İ into i and a combining dot; one i is what is meant.
            'İ' => out.push('i'),
            c => out.extend(c.to_lowercase()),
        }
    }
    out
}

/// `text` folded for a search: small letters, and every kind of i (i, ı, I, İ) is one letter, so
/// a Turkish word is found whether it is typed with the Turkish letters or without them.
pub fn fold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            'I' | 'İ' | 'ı' => out.push('i'),
            c => out.extend(c.to_lowercase()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turkish_capitals_keep_the_dot_on_the_i() {
        assert_eq!(upper_in("tr", "Birimler"), "BİRİMLER");
        assert_eq!(upper_in("tr", "Sesler ve iş günlüğü"), "SESLER VE İŞ GÜNLÜĞÜ");
        assert_eq!(upper_in("tr", "Otomatik kaydet"), "OTOMATİK KAYDET");
        assert_eq!(upper_in("tr", "Kapalı"), "KAPALI");
        assert_eq!(upper_in("az", "fayl"), "FAYL");
    }

    #[test]
    fn other_languages_keep_the_plain_capitals() {
        assert_eq!(upper_in("en", "Units"), "UNITS");
        assert_eq!(upper_in("de", "Größe"), "GRÖSSE");
        assert_eq!(upper_in("", "Birimler"), "BIRIMLER");
        assert_eq!(upper_in("hu", "Mértékegységek"), "MÉRTÉKEGYSÉGEK");
    }

    #[test]
    fn turkish_small_letters_keep_the_dotless_i() {
        assert_eq!(lower_in("tr", "Izgara dizisi"), "ızgara dizisi");
        assert_eq!(lower_in("tr", "İçe aktar"), "içe aktar");
        assert_eq!(lower_in("en", "Import"), "import");
        assert_eq!(lower_in("en", "İ"), "i");
    }

    #[test]
    fn a_search_finds_a_turkish_word_with_or_without_the_turkish_letters() {
        for typed in ["ızgara", "izgara", "IZGARA", "Izgara"] {
            assert!(fold("Izgara dizisi").contains(&fold(typed)), "{typed}");
        }
        assert!(fold("İçe aktar").contains(&fold("içe")));
        assert!(fold("Import").contains(&fold("IMP")));
    }
}
