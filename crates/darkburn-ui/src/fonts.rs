// Copied from SolidCraft crates/ui-egui/src/fonts.rs (MIT OR Apache-2.0), trimmed to the bundled faces.
//! Bundled IBM Plex interface faces, user overrides, and language-specific system fallbacks
//! (CJK, Hangul, Thai, Hebrew).
//! The same definitions are used on desktop and web; the web shell adds Japanese and Chinese
//! faces on demand.

use std::sync::Arc;

use egui::{FontDefinitions, FontFamily, FontId, RichText};

const PLEX_SANS: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/fonts/ibm-plex-sans/IBMPlexSans-Variable.ttf"));
const PLEX_MONO: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/fonts/ibm-plex-mono/IBMPlexMono-Regular.ttf"));
const WEIGHTS: [(&str, f32); 4] = [("plex-sans", 400.0), ("ui-medium", 500.0), ("ui-semibold", 600.0), ("ui-bold", 700.0)];

fn sans_families() -> [FontFamily; 4] {
    [FontFamily::Proportional, medium_family(), semibold_family(), bold_family()]
}

fn all_families() -> impl Iterator<Item = FontFamily> {
    sans_families().into_iter().chain([FontFamily::Monospace])
}

/// IBM Plex Sans at 400/500/600/700 and Plex Mono Regular, followed by egui's symbol/emoji
/// fallbacks. Each variable instance shares the embedded bytes and selects a real weight.
pub fn base_font_definitions() -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    let fallbacks = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    for ((name, weight), family) in WEIGHTS.into_iter().zip(sans_families()) {
        let mut data = egui::FontData::from_static(PLEX_SANS);
        data.tweak.coords = egui::epaint::text::VariationCoords::new([(b"wght", weight)]);
        fonts.font_data.insert(name.to_owned(), Arc::new(data));
        fonts.families.insert(family, std::iter::once(name.to_owned()).chain(fallbacks.iter().cloned()).collect());
    }
    fonts.font_data.insert("plex-mono".to_owned(), Arc::new(egui::FontData::from_static(PLEX_MONO)));
    fonts.families.entry(FontFamily::Monospace).or_default().insert(0, "plex-mono".to_owned());
    with_symbol_fallback(fonts)
}

fn medium_family() -> FontFamily {
    FontFamily::Name("ui-medium".into())
}

fn semibold_family() -> FontFamily {
    FontFamily::Name("ui-semibold".into())
}

fn bold_family() -> FontFamily {
    FontFamily::Name("ui-bold".into())
}

/// Medium (500) interface text, retaining the language fallbacks and user override.
pub fn medium(size: f32) -> FontId {
    FontId::new(size, medium_family())
}

/// Semibold (600) interface text, retaining the language fallbacks and user override.
pub fn semibold(size: f32) -> FontId {
    FontId::new(size, semibold_family())
}

/// Bold (700) interface text, retaining the language fallbacks and user override.
pub fn bold(size: f32) -> FontId {
    FontId::new(size, bold_family())
}

/// Select a real interface weight without changing the text's size or colour.
pub trait RichTextExt {
    fn medium(self) -> Self;
    fn semibold(self) -> Self;
    fn bold_face(self) -> Self;
}

impl RichTextExt for RichText {
    fn medium(self) -> Self {
        self.family(medium_family())
    }
    fn semibold(self) -> Self {
        self.family(semibold_family())
    }
    fn bold_face(self) -> Self {
        self.family(bold_family())
    }
}

// Keep pending definitions too: set_fonts takes effect on the next frame, while the web
// fetch and preference update can both finish before that frame starts.
fn installed_fonts_id() -> egui::Id {
    egui::Id::unique("solidcraft_interface_fonts")
}

fn install(ctx: &egui::Context, fonts: FontDefinitions) {
    let fonts = with_symbol_fallback(fonts);
    ctx.data_mut(|d| d.insert_temp(installed_fonts_id(), fonts.clone()));
    ctx.set_fonts(fonts);
    ctx.global_style_mut(|style| {
        if let Some(heading) = style.text_styles.get_mut(&egui::TextStyle::Heading) {
            heading.family = semibold(heading.size).family;
        }
    });
}

/// Install only the bundled definitions (also used by the offscreen scenario shell).
pub fn setup_bundled_fonts(ctx: &egui::Context) {
    install(ctx, base_font_definitions());
}

/// `fonts` with `extra` faces right after the interface face of every family: system fonts for a
/// script the bundled faces cannot draw (Japanese, Korean, Chinese). Ahead of egui's own
/// fallbacks, whose emoji font has a few CJK characters that would otherwise win.
pub fn with_fallbacks(mut fonts: FontDefinitions, extra: Vec<(String, egui::FontData)>) -> FontDefinitions {
    for (i, (name, data)) in extra.into_iter().enumerate() {
        fonts.font_data.insert(name.clone(), Arc::new(data));
        for family in all_families() {
            let list = fonts.families.entry(family).or_default();
            list.retain(|n| *n != name);
            list.insert((1 + i).min(list.len()), name.clone());
        }
    }
    fonts
}

/// Install the bundled definitions with `extra` faces as in [`with_fallbacks`]. An empty list
/// gives the bundled definitions again.
pub fn setup_fonts_with_fallbacks(ctx: &egui::Context, extra: Vec<(String, egui::FontData)>) {
    install(ctx, with_fallbacks(base_font_definitions(), extra));
}

/// egui's built-in monospace face, also the last fallback of every sans family (#2955).
pub const SYMBOL_FALLBACK: &str = "Hack";

/// `fonts` with egui's built-in Hack last in every sans family. Ubuntu-Light, NotoEmoji and
/// emoji-icon-font lack common symbols such as ▲ ▼ ↑ ↓ → ⁻, which Hack has, so they no longer
/// show as empty boxes where no system font fills in (#2955). Text the other faces draw keeps them.
pub fn with_symbol_fallback(mut fonts: egui::FontDefinitions) -> egui::FontDefinitions {
    if fonts.font_data.contains_key(SYMBOL_FALLBACK) {
        for family in sans_families() {
            let list = fonts.families.entry(family).or_default();
            list.retain(|name| name != SYMBOL_FALLBACK);
            list.push(SYMBOL_FALLBACK.to_owned());
        }
    }
    fonts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_faces_come_right_after_the_interface_face_in_every_family() {
        let extra = vec![("a".to_string(), egui::FontData::from_static(PLEX_MONO)), ("b".to_string(), egui::FontData::from_static(PLEX_MONO))];
        let fonts = with_fallbacks(base_font_definitions(), extra);
        for family in all_families() {
            let list = &fonts.families[&family];
            assert_eq!(&list[1..3], &["a".to_string(), "b".to_string()], "{family:?}: {list:?}");
            if family != FontFamily::Monospace {
                assert_eq!(list.last().map(String::as_str), Some(SYMBOL_FALLBACK), "{family:?}: {list:?}");
            }
        }
        assert!(fonts.font_data.contains_key("a") && fonts.font_data.contains_key("b"));
        // No extra faces: the bundled definitions as they were.
        let plain = with_fallbacks(base_font_definitions(), Vec::new());
        assert_eq!(plain.families, base_font_definitions().families);
    }
}
