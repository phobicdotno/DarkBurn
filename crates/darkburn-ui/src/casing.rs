//! Upper and lower case that follow the language of the text.
//!
//! Rust's own case mapping follows no language. Turkish and Azerbaijani write a dotted and a
//! dotless i as two letters (i and İ, ı and I), so there "Birimler" must become "BİRİMLER", not
//! "BIRIMLER". A text not translated yet is still English in a Turkish interface, and English
//! "Settings" stays "SETTINGS". Captions that change case go through this module, which asks the
//! program which language a text is written in.
//!
//! Greek drops the accent (tonos) in capitals: "Ρυθμίσεις" becomes "ΡΥΘΜΙΣΕΙΣ", not
//! "ΡΥΘΜΊΣΕΙΣ". Greek letters are only written in Greek, so this needs no language.

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
        _ => plain_upper(text),
    }
}

/// Whether `c` is a Greek letter or a Greek accent.
fn greek(c: char) -> bool {
    matches!(c, '\u{370}'..='\u{3ff}' | '\u{1f00}'..='\u{1fff}')
}

/// `text` in capitals with no language rules, except that Greek capitals carry no accent.
fn plain_upper(text: &str) -> String {
    if !text.chars().any(greek) {
        return text.to_uppercase();
    }
    let mut out = String::with_capacity(text.len() + 4);
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            'ά' | 'έ' | 'ή' | 'ί' | 'ό' | 'ύ' | 'ώ' | 'ΐ' | 'ΰ' | 'Ά' | 'Έ' | 'Ή' | 'Ί' | 'Ό' | 'Ύ' | 'Ώ' => {
                out.push(greek_capital_without_accent(c));
                // The accent kept "άι" two vowels; without it a diaeresis does: "ΤΣΑΪ", not "ΤΣΑΙ".
                if matches!(c, 'ά' | 'έ' | 'ή' | 'ό' | 'ύ') {
                    match chars.peek() {
                        Some('ι') => {
                            out.push('Ϊ');
                            chars.next();
                        }
                        Some('υ') if c != 'ύ' => {
                            out.push('Ϋ');
                            chars.next();
                        }
                        _ => {}
                    }
                }
            }
            c => out.extend(c.to_uppercase()),
        }
    }
    out
}

/// The capital of an accented Greek vowel, without the accent; a diaeresis stays.
fn greek_capital_without_accent(c: char) -> char {
    match c {
        'ά' | 'Ά' => 'Α',
        'έ' | 'Έ' => 'Ε',
        'ή' | 'Ή' => 'Η',
        'ί' | 'Ί' => 'Ι',
        'ό' | 'Ό' => 'Ο',
        'ύ' | 'Ύ' => 'Υ',
        'ώ' | 'Ώ' => 'Ω',
        'ΐ' => 'Ϊ',
        'ΰ' => 'Ϋ',
        c => c,
    }
}

/// `text` in capitals, as `language` writes them.
pub fn upper_in(language: &str, text: &str) -> String {
    if !dotted_i(language) {
        return plain_upper(text);
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
/// a Turkish word is found whether it is typed with the Turkish letters or without them. Greek
/// vowels lose their accents and the final ς is σ, and the Russian ё is е, so a word typed
/// without accents (or in capitals) is found too.
pub fn fold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            'I' | 'İ' | 'ı' => out.push('i'),
            c => out.extend(c.to_lowercase().map(fold_letter)),
        }
    }
    out
}

/// A small letter without the accent a search ignores.
fn fold_letter(c: char) -> char {
    match c {
        'ά' => 'α',
        'έ' => 'ε',
        'ή' => 'η',
        'ί' | 'ϊ' | 'ΐ' => 'ι',
        'ό' => 'ο',
        'ύ' | 'ϋ' | 'ΰ' => 'υ',
        'ώ' => 'ω',
        'ς' => 'σ',
        'ё' => 'е',
        c => c,
    }
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
    fn greek_capitals_drop_the_accent() {
        assert_eq!(upper_in("el", "Ρυθμίσεις"), "ΡΥΘΜΙΣΕΙΣ");
        assert_eq!(upper_in("el", "Ήχοι και καταγραφή εργασιών"), "ΗΧΟΙ ΚΑΙ ΚΑΤΑΓΡΑΦΗ ΕΡΓΑΣΙΩΝ");
        assert_eq!(upper_in("el", "Μονάδες"), "ΜΟΝΑΔΕΣ");
        assert_eq!(upper_in("el", "Ώθηση"), "ΩΘΗΣΗ");
        // Where the accent kept two vowels apart, a diaeresis now does.
        assert_eq!(upper_in("el", "τσάι"), "ΤΣΑΪ");
        assert_eq!(upper_in("el", "άυλος"), "ΑΫΛΟΣ");
        assert_eq!(upper_in("el", "προϊόν"), "ΠΡΟΪΟΝ");
        assert_eq!(upper_in("el", "διϋλιστήριο"), "ΔΙΫΛΙΣΤΗΡΙΟ");
        // A diphthong without an accent on its first vowel stays as it is.
        assert_eq!(upper_in("el", "Γραμμές εργαλείων"), "ΓΡΑΜΜΕΣ ΕΡΓΑΛΕΙΩΝ");
        assert_eq!(upper_in("el", "Πλάτος"), "ΠΛΑΤΟΣ");
        // Greek needs no language: an English interface still writes it the Greek way.
        assert_eq!(upper_in("en", "Ελληνικά"), "ΕΛΛΗΝΙΚΑ");
        assert_eq!(upper("Ελληνικά"), "ΕΛΛΗΝΙΚΑ");
        assert_eq!(upper_in("ru", "Настройки"), "НАСТРОЙКИ");
    }

    #[test]
    fn a_search_finds_greek_and_russian_words_without_their_accents() {
        for typed in ["ρυθμίσεις", "ρυθμισεις", "ΡΥΘΜΙΣΕΙΣ", "ρυθμισεισ"] {
            assert!(fold("Ρυθμίσεις συσκευής").contains(&fold(typed)), "{typed}");
        }
        assert!(fold("Προϊόν").contains(&fold("προιον")));
        assert!(fold("Ещё").contains(&fold("еще")));
        assert!(fold("Сохранить как").contains(&fold("СОХРАН")));
        assert!(!fold("Ρυθμίσεις").contains(&fold("ρυθμοσ")));
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
