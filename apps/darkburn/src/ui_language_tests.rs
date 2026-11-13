//! The interface in German, French, Italian, Spanish (Spain and Latin America), Portuguese,
//! Dutch, Danish, Swedish, Finnish, Polish, Czech, Hungarian, Slovenian, Turkish, Greek,
//! Russian, Japanese, Korean and Chinese (Simplified and Traditional): with every panel open, in the settings and in a long help topic, the translated text fits
//! where the English text fits [I18N-04].
use crate::ui_harness::Gui;

fn show_every_panel(g: &mut Gui) {
    for p in crate::panels::PANELS {
        if !g.app().prefs.panel(p.id).visible {
            g.run_command(&format!("panel.{}", p.id));
        }
    }
    g.frames(3);
}

/// What one view shows in one language: its layout problems and the texts cut short with "...".
struct View {
    name: &'static str,
    problems: Vec<String>,
    elided: Vec<String>,
}

/// Draws frames until the interface fonts draw every character of `text` in language `code`,
/// and says whether they do. Japanese and Korean take their faces from the computer's fonts,
/// worked out on a thread of their own; until they arrive their text shows as boxes. On a
/// computer with no font for the script there is nothing to wait for: false at once, and the
/// caller skips the language, as the font tests do (CI installs fonts-noto-cjk).
fn wait_for_glyphs(g: &mut Gui, code: &str, text: &str) -> bool {
    if !crate::language_fonts::this_computer_draws(code, text) {
        eprintln!("{code}: no font on this computer draws {text:?}, skipped");
        return false;
    }
    let font = egui::FontId::proportional(14.0);
    let start = std::time::Instant::now();
    while !g.ctx().fonts_mut(|f| text.chars().all(|c| c.is_whitespace() || f.has_glyph(&font, c))) {
        assert!(start.elapsed() < std::time::Duration::from_secs(120), "{code}: the faces that draw {text:?} never arrived");
        std::thread::sleep(std::time::Duration::from_millis(10));
        g.frames(1);
    }
    g.settle();
    true
}

fn look(g: &mut Gui, code: &str, name: &'static str) -> View {
    g.screenshot(&format!("lang-{code}-{name}"));
    let elided = g.painted().into_iter().filter(|p| p.elided).map(|p| p.text).collect();
    View { name, problems: g.layout_problems(), elided }
}

/// Every panel, the settings and the longest help topic in `code`. `edit`, `general` and
/// `help_title` are the Edit menu, the first settings page and the help topic's title in it.
/// None on a computer with no font for the language.
fn views_in(code: &str, edit: &str, general: &str, help_title: &str) -> Option<Vec<View>> {
    let mut g = Gui::standard();
    g.run_command(&format!("lang.{code}"));
    g.settle();
    if !wait_for_glyphs(&mut g, code, edit) {
        crate::i18n::set_language("en");
        return None;
    }
    assert_eq!(crate::i18n::language(), code);
    assert!(g.has_text(edit), "{code}: no {edit:?} in {:?}", g.visible_texts());
    let mut out = Vec::new();

    show_every_panel(&mut g);
    g.settle();
    out.push(look(&mut g, code, "panels"));

    g.run_command("edit.settings");
    g.settle();
    assert!(g.has_text(general), "{code}: no {general:?} in the settings: {:?}", g.visible_texts());
    out.push(look(&mut g, code, "settings"));
    g.close_dialogs();

    g.run_command("help.quick");
    g.app().extras.help_topic = "camera_selection".into();
    g.settle();
    assert!(g.has_text(help_title), "{code}: no {help_title:?} in the help: {:?}", g.visible_texts());
    out.push(look(&mut g, code, "help"));
    g.close_dialogs();

    crate::i18n::set_language("en");
    Some(out)
}

/// No layout problems, and no more texts cut short in any view than in English. Skipped on a
/// computer with no font for the language.
fn fits_like_english(code: &str, edit: &str, general: &str, help_title: &str) {
    let Some(views) = views_in(code, edit, general, help_title) else { return };
    let english = views_in("en", "Edit", "General", "Choosing a camera").expect("English needs no system font");
    let mut bad = Vec::new();
    for (en, v) in english.iter().zip(&views) {
        bad.extend(v.problems.iter().map(|p| format!("{code}, {}: {p}", v.name)));
        if v.elided.len() > en.elided.len() {
            bad.push(format!("{code}, {}: cut short {:?}, in English only {:?}", v.name, v.elided, en.elided));
        }
    }
    assert!(bad.is_empty(), "{} problems:\n{}", bad.len(), bad.join("\n"));
}

#[test]
fn german_fits_the_window() {
    fits_like_english("de", "Bearbeiten", "Allgemein", "Eine Kamera wählen");
}

#[test]
fn french_fits_the_window() {
    fits_like_english("fr", "Édition", "Général", "Choisir une caméra");
}

#[test]
fn italian_fits_the_window() {
    fits_like_english("it", "Modifica", "Generale", "Scegliere una telecamera");
}

#[test]
fn spanish_for_spain_fits_the_window() {
    fits_like_english("es-ES", "Edición", "General", "Elegir una cámara");
}

#[test]
fn spanish_for_latin_america_fits_the_window() {
    fits_like_english("es-LA", "Editar", "General", "Elegir una cámara");
}

#[test]
fn portuguese_fits_the_window() {
    fits_like_english("pt", "Editar", "Geral", "Escolher uma câmera");
}

#[test]
fn dutch_fits_the_window() {
    fits_like_english("nl", "Bewerken", "Algemeen", "Een camera kiezen");
}

#[test]
fn danish_fits_the_window() {
    fits_like_english("da", "Rediger", "Generelt", "Vælg et kamera");
}

#[test]
fn swedish_fits_the_window() {
    fits_like_english("sv", "Redigera", "Allmänt", "Välja kamera");
}

#[test]
fn finnish_fits_the_window() {
    fits_like_english("fi", "Muokkaa", "Yleiset", "Kameran valinta");
}

#[test]
fn polish_fits_the_window() {
    fits_like_english("pl", "Edycja", "Ogólne", "Wybór kamery");
}

#[test]
fn czech_fits_the_window() {
    fits_like_english("cs", "Úpravy", "Obecné", "Výběr kamery");
}

#[test]
fn hungarian_fits_the_window() {
    fits_like_english("hu", "Szerkesztés", "Általános", "Kamera kiválasztása");
}

#[test]
fn slovenian_fits_the_window() {
    fits_like_english("sl", "Urejanje", "Splošno", "Izbira kamere");
}

#[test]
fn turkish_fits_the_window() {
    fits_like_english("tr", "Düzen", "Genel", "Kamera seçimi");
}

#[test]
fn greek_fits_the_window() {
    fits_like_english("el", "Επεξεργασία", "Γενικά", "Επιλογή κάμερας");
}

#[test]
fn russian_fits_the_window() {
    fits_like_english("ru", "Правка", "Общие", "Выбор камеры");
}

#[test]
fn japanese_fits_the_window() {
    fits_like_english("ja", "編集", "一般", "カメラの選び方");
}

#[test]
fn korean_fits_the_window() {
    fits_like_english("ko", "편집", "일반", "카메라 선택");
}

#[test]
fn simplified_chinese_fits_the_window() {
    fits_like_english("zh-CN", "编辑", "常规", "选择摄像头");
}

#[test]
fn traditional_chinese_fits_the_window() {
    fits_like_english("zh-TW", "編輯", "一般", "選擇攝影機");
}

/// The Editing page of the settings holds the job options. The setting that saves the job origin
/// and "Cut selected graphics" with the project reads longer in Japanese, Korean and Chinese
/// since the review; it still shows whole, with the other job options, and nothing on the page
/// clips [review of I18N-04].
#[test]
fn the_job_options_in_the_settings_fit_in_japanese_korean_and_chinese() {
    let mut problems = Vec::new();
    for code in ["en", "ja", "ko", "zh-CN", "zh-TW"] {
        let mut g = Gui::standard();
        g.run_command(&format!("lang.{code}"));
        g.settle();
        let label = crate::i18n::tr("settings.save_job_origin", "Save the job origin and cut selected in the project");
        if !wait_for_glyphs(&mut g, code, &label) {
            crate::i18n::set_language("en");
            continue;
        }
        g.run_command("edit.settings");
        g.ctx().data_mut(|d| d.insert_temp(egui::Id::unique("db_settings_page"), 2usize));
        g.settle();
        g.screenshot(&format!("lang-{code}-settings-editing"));
        let shown: Vec<_> = g.painted().into_iter().filter(|p| p.text == label).collect();
        if shown.is_empty() {
            problems.push(format!("{code}: no {label:?} in {:?}", g.visible_texts()));
        }
        problems.extend(shown.iter().filter(|p| p.elided).map(|p| format!("{code}: {:?} is cut short", p.text)));
        problems.extend(g.layout_problems().into_iter().map(|p| format!("{code}: {p}")));
        g.close_dialogs();
        crate::i18n::set_language("en");
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// On a computer with no font for a text there is nothing to wait for: the language is skipped
/// at once, where the wait used to spin for two minutes and then fail [review of I18N-04].
/// U+10FFFD is a private-use character no ordinary font draws.
#[test]
fn a_text_no_font_on_the_computer_draws_is_skipped_at_once() {
    for code in ["ja", "ko", "zh-CN", "zh-TW"] {
        let mut g = Gui::standard();
        g.run_command(&format!("lang.{code}"));
        g.settle();
        let start = std::time::Instant::now();
        let drawn = wait_for_glyphs(&mut g, code, "\u{10fffd}");
        crate::i18n::set_language("en");
        assert!(!drawn, "{code}: a font draws U+10FFFD");
        assert!(start.elapsed() < std::time::Duration::from_secs(60), "{code}: waited {:?}", start.elapsed());
    }
    let mut g = Gui::standard();
    assert!(wait_for_glyphs(&mut g, "en", "Edit"), "English is drawn by the bundled font");
}

/// A row may end between two Japanese or Chinese characters, or next to one, without splitting
/// a word; between two letters of a Latin or Korean word it splits the word.
#[test]
fn a_line_break_between_japanese_characters_splits_no_word() {
    use crate::ui_harness::splits_a_word;
    assert!(splits_a_word('a', 'b'));
    assert!(splits_a_word('\u{d55c}', '\u{ae00}'), "Korean 한|글");
    assert!(!splits_a_word('\u{30f3}', '\u{30c9}'), "katakana ン|ド");
    assert!(!splits_a_word('\u{5b57}', 'a'), "kanji 字|a");
    assert!(!splits_a_word('a', '\u{3092}'), "a|hiragana を");
    assert!(!splits_a_word('a', ' '));
    assert!(!splits_a_word('\u{4e2d}', '\u{6587}'), "Simplified Chinese 中|文");
    assert!(!splits_a_word('\u{6a94}', '\u{6848}'), "Traditional Chinese 檔|案");
    assert!(!splits_a_word('\u{5b57}', '\u{ff0c}'), "字|full-width comma");
}

/// The cut table's column headers in `code`, left to right as painted, with the overlaps
/// between neighbours. A header longer than its narrow column runs into the next one. Nothing
/// on a computer with no font for the language: it is skipped, and the others are still checked.
fn cut_header_overlaps(code: &str) -> Vec<String> {
    use crate::i18n::tr;
    let mut g = Gui::standard();
    g.run_command(&format!("lang.{code}"));
    g.settle();
    if !wait_for_glyphs(&mut g, code, &tr("menu.File", "File")) {
        crate::i18n::set_language("en");
        return Vec::new();
    }
    let labels = [
        tr("cuts.col.layer", "Layer"),
        tr("cuts.col.mode", "Mode"),
        tr("cuts.col.speed_power", "Spd/Pwr"),
        tr("cuts.col.out", "Out"),
        tr("cuts.col.show", "Vis"),
        tr("cuts.col.air", "Air"),
    ];
    let painted = g.painted();
    let anchor = painted.iter().find(|p| p.text == labels[2]).map(|p| p.full);
    crate::i18n::set_language("en");
    let Some(anchor) = anchor else {
        return vec![format!("{code}: no cut table header {:?} in {:?}", labels[2], g.visible_texts())];
    };
    let mut rects = Vec::new();
    for l in &labels {
        let near = painted
            .iter()
            .filter(|p| &p.text == l && (p.full.center().y - anchor.center().y).abs() < 2.0 && (p.full.left() - anchor.left()).abs() < 400.0)
            .min_by(|a, b| (a.full.left() - anchor.left()).abs().total_cmp(&(b.full.left() - anchor.left()).abs()));
        match near {
            Some(p) => rects.push((l.clone(), p.full)),
            None => return vec![format!("{code}: cut table header {l:?} is not drawn")],
        }
    }
    let mut out = Vec::new();
    for w in rects.windows(2) {
        let ((a, ra), (b, rb)) = (&w[0], &w[1]);
        if ra.right() > rb.left() - 1.0 {
            out.push(format!("{code}: header {a:?} {ra:?} runs into {b:?} {rb:?}"));
        }
    }
    out
}

/// "Ausg." in German is longer than "Out": every language's headers still end before the
/// next column's header begins [I18N-04].
#[test]
fn cut_table_headers_fit_their_columns_in_every_language() {
    let bad: Vec<String> = [
        "en", "nb", "de", "fr", "it", "es-ES", "es-LA", "pt", "nl", "da", "sv", "fi", "pl", "cs", "hu", "sl", "tr", "el", "ru", "ja", "ko", "zh-CN",
        "zh-TW",
    ]
    .iter()
    .flat_map(|c| cut_header_overlaps(c))
    .collect();
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

/// Turkish capitals keep the dot on the i: the settings headings read "BİRİMLER" and
/// "SESLER VE İŞ GÜNLÜĞÜ", never "BIRIMLER" or "IŞ", while English keeps "UNITS".
#[test]
fn turkish_headings_keep_the_dotted_capital_i() {
    let mut g = Gui::standard();
    g.run_command("edit.settings");
    g.settle();
    assert!(g.has_text("UNITS"), "no UNITS heading in {:?}", g.visible_texts());
    g.close_dialogs();

    g.run_command("lang.tr");
    g.settle();
    g.run_command("edit.settings");
    g.settle();
    for heading in ["BİRİMLER", "SESLER VE İŞ GÜNLÜĞÜ", "OTOMATİK KAYDET"] {
        assert!(g.has_text(heading), "no {heading:?} in {:?}", g.visible_texts());
    }
    let wrong: Vec<String> =
        g.visible_texts().into_iter().filter(|t| t.contains("BIRIMLER") || t.contains(" IŞ ") || t.contains("OTOMATIK")).collect();
    assert!(wrong.is_empty(), "headings without the dotted İ: {wrong:?}");
    // A caption not translated yet is English, and keeps English capitals.
    let english: Vec<String> = g.visible_texts().into_iter().filter(|t| t.contains("SETTİNGS") || t.contains("CONNECTİON")).collect();
    assert!(english.is_empty(), "English captions with a Turkish İ: {english:?}");
    g.close_dialogs();
    crate::i18n::set_language("en");
}

/// The Edit Hotkeys filter in Turkish finds a command whether its Turkish letters are typed or
/// not: "farkli" and "FARKLI" find "Farklı kaydet", "içe" and "İÇE" find "İçe aktar". English
/// keeps working ("save" finds "Save As").
#[test]
fn turkish_hotkey_filter_finds_words_with_or_without_the_turkish_i() {
    let shows = |code: &str, typed: &str, want: &str, not: &str| {
        let mut g = Gui::standard();
        g.run_command(&format!("lang.{code}"));
        g.settle();
        g.app().extras.hotkeys.filter = typed.into();
        g.run_command("file.hotkeys");
        g.settle();
        if code == "tr" && typed == "farkli" {
            g.screenshot("lang-tr-hotkeys");
        }
        assert!(!g.find_texts(want).is_empty(), "{code}: {typed:?} does not find {want:?}: {:?}", g.visible_texts());
        assert!(g.find_texts(not).is_empty(), "{code}: {typed:?} also shows {not:?}");
        g.close_dialogs();
        crate::i18n::set_language("en");
    };
    shows("en", "save", "Save As", "New Window");
    for typed in ["farklı", "farkli", "FARKLI"] {
        shows("tr", typed, "Farklı kaydet", "Yeni pencere");
    }
    for typed in ["içe", "İÇE"] {
        shows("tr", typed, "İçe aktar", "Yeni pencere");
    }
}

/// Greek capitals carry no accent: the settings headings read "ΜΟΝΑΔΕΣ" and "ΑΥΤΟΜΑΤΗ
/// ΑΠΟΘΗΚΕΥΣΗ", never "ΜΟΝΆΔΕΣ" or "ΑΥΤΌΜΑΤΗ". Russian capitals are the plain ones.
#[test]
fn greek_headings_drop_the_accent() {
    let accented_capital = |t: &String| t.chars().any(|c| "ΆΈΉΊΌΎΏ".contains(c)) && *t == t.to_uppercase();
    let mut g = Gui::standard();
    g.run_command("lang.el");
    g.settle();
    g.run_command("edit.settings");
    g.settle();
    g.screenshot("lang-el-headings");
    for heading in ["ΜΟΝΑΔΕΣ", "ΗΧΟΙ ΚΑΙ ΚΑΤΑΓΡΑΦΗ ΕΡΓΑΣΙΩΝ", "ΑΥΤΟΜΑΤΗ ΑΠΟΘΗΚΕΥΣΗ"] {
        assert!(g.has_text(heading), "no {heading:?} in {:?}", g.visible_texts());
    }
    let wrong: Vec<String> = g.visible_texts().into_iter().filter(accented_capital).collect();
    assert!(wrong.is_empty(), "capitals with an accent: {wrong:?}");
    g.close_dialogs();

    g.run_command("lang.ru");
    g.settle();
    g.run_command("edit.settings");
    g.settle();
    assert!(g.has_text("ЕДИНИЦЫ"), "no ЕДИНИЦЫ in {:?}", g.visible_texts());
    g.close_dialogs();
    crate::i18n::set_language("en");
}

/// The Edit Hotkeys filter in Greek finds a command whether its accents are typed or not:
/// "ρυθμισεις" and "ΡΥΘΜΙΣΕΙΣ" find "Ρυθμίσεις συσκευής".
#[test]
fn greek_hotkey_filter_finds_words_with_or_without_the_accents() {
    for typed in ["ρυθμίσεις", "ρυθμισεις", "ΡΥΘΜΙΣΕΙΣ"] {
        let mut g = Gui::standard();
        g.run_command("lang.el");
        g.settle();
        g.app().extras.hotkeys.filter = typed.into();
        g.run_command("file.hotkeys");
        g.settle();
        assert!(!g.find_texts("Ρυθμίσεις συσκευής").is_empty(), "{typed:?} does not find it: {:?}", g.visible_texts());
        assert!(g.find_texts("Νέο παράθυρο").is_empty(), "{typed:?} also shows Νέο παράθυρο");
        g.close_dialogs();
        crate::i18n::set_language("en");
    }
}
