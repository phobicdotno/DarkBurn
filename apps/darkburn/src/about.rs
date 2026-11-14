//! The About window body: logo, version with build number, links and a "What's new" list read
//! from the changelog that is built into the program.

use darkburn_ui::fonts;
use solidcraft_theme::Tokens;

/// The workspace changelog, embedded at compile time.
pub const CHANGELOG: &str = include_str!("../../../CHANGELOG.md");
/// The program's version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Where the source lives.
pub const SOURCE_URL: &str = "https://github.com/phobicdotno/DarkBurn";
const LOGO_PNG: &[u8] = include_bytes!("../../../assets/icons/app/darkburn-256.png");

/// One release in the changelog.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Release {
    pub version: String,
    pub build: Option<u32>,
    pub lines: Vec<String>,
}

/// Reads `## vX (build N)` headings and their `- ` bullets, newest (first in the file) first.
/// Text before the first heading is ignored; an indented line continues the bullet above it.
pub fn parse_changelog(markdown: &str) -> Vec<Release> {
    let mut out: Vec<Release> = Vec::new();
    for line in markdown.lines() {
        if let Some(head) = line.strip_prefix("## ") {
            let head = head.trim();
            let head = head.strip_prefix('v').unwrap_or(head);
            let (version, rest) = head.split_once(char::is_whitespace).unwrap_or((head, ""));
            let build = rest
                .trim()
                .strip_prefix("(build ")
                .and_then(|r| r.split(')').next())
                .and_then(|n| n.trim().parse().ok());
            out.push(Release { version: version.to_string(), build, lines: Vec::new() });
            continue;
        }
        let Some(release) = out.last_mut() else { continue };
        if let Some(bullet) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
            release.lines.push(bullet.trim().to_string());
        } else if line.starts_with("  ") && !line.trim().is_empty() {
            if let Some(last) = release.lines.last_mut() {
                last.push(' ');
                last.push_str(line.trim());
            }
        }
    }
    out
}

/// The build number of `version` in the changelog, if its heading names one.
pub fn build_of(releases: &[Release], version: &str) -> Option<u32> {
    releases.iter().find(|r| r.version == version).and_then(|r| r.build)
}

fn releases() -> &'static [Release] {
    static R: std::sync::OnceLock<Vec<Release>> = std::sync::OnceLock::new();
    R.get_or_init(|| parse_changelog(CHANGELOG))
}

fn logo(ctx: &egui::Context) -> Option<egui::TextureHandle> {
    let id = egui::Id::new("db_about_logo");
    if let Some(t) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return Some(t);
    }
    let img = image::load_from_memory_with_format(LOGO_PNG, image::ImageFormat::Png).ok()?.to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    let colour = egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw());
    let tex = ctx.load_texture("db_about_logo", colour, egui::TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(id, tex.clone()));
    Some(tex)
}

/// Draws the About window's contents.
pub fn body(ui: &mut egui::Ui) {
    let t = Tokens::get();
    let tr = crate::i18n::tr;
    ui.vertical_centered(|ui| {
        if let Some(tex) = logo(ui.ctx()) {
            ui.add(egui::Image::new((tex.id(), egui::vec2(96.0, 96.0))));
        }
        ui.label(egui::RichText::new("DarkBurn").font(fonts::semibold(t.font_subtitle)));
        ui.label(crate::i18n::tr("about.tagline", "A laser cutter and engraver program."));
        let version = match build_of(releases(), VERSION) {
            Some(b) => format!("{} {VERSION} ({} {b})", tr("about.version", "Version"), tr("about.build", "build")),
            None => format!("{} {VERSION}", tr("about.version", "Version")),
        };
        ui.label(version);
        ui.horizontal(|ui| {
            if ui.link(crate::i18n::tr("about.docs", "Online documentation")).clicked() {
                crate::help::open_url(crate::help::DOCS_URL);
            }
            ui.label("·");
            if ui.link(crate::i18n::tr("about.source", "Source code on GitHub")).clicked() {
                crate::help::open_url(SOURCE_URL);
            }
        });
    });
    ui.add_space(t.space_8);
    ui.label(egui::RichText::new(crate::i18n::tr("about.whats_new", "What's new")).font(fonts::semibold(t.font_body)));
    egui::ScrollArea::vertical().id_salt("db_about_whats_new").max_height(260.0).auto_shrink([false, true]).show(ui, |ui| {
        for r in releases() {
            let head = match r.build {
                Some(b) => format!("v{} ({} {b})", r.version, tr("about.build", "build")),
                None => format!("v{}", r.version),
            };
            ui.label(egui::RichText::new(head).strong());
            for line in &r.lines {
                ui.add(egui::Label::new(egui::RichText::new(format!("• {line}")).font(egui::FontId::proportional(t.font_small))).wrap());
            }
            ui.add_space(t.space_6);
        }
    });
    ui.add_space(t.space_8);
    ui.label(egui::RichText::new("Licensed under the MIT License or the Apache License 2.0.").color(t.text_dim));
    ui.label(
        egui::RichText::new("Widgets, icons and themes from SolidCraft (MIT OR Apache-2.0). Interface font IBM Plex (SIL OFL 1.1). See ATTRIBUTION.md.")
            .color(t.text_dim),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn about_parses_headings_with_and_without_build() {
        let md = "# Changelog\n\nintro\n\n## v1.2.3 (build 9)\n- New: a\n- Fix: b\n  wrapped\n\n## v1.2.2\n- Fix: c\n## 1.0.0 (build 1)\n";
        let r = parse_changelog(md);
        assert_eq!(r.len(), 3);
        assert_eq!(r[0], Release { version: "1.2.3".into(), build: Some(9), lines: vec!["New: a".into(), "Fix: b wrapped".into()] });
        assert_eq!(r[1], Release { version: "1.2.2".into(), build: None, lines: vec!["Fix: c".into()] });
        assert_eq!(r[2], Release { version: "1.0.0".into(), build: Some(1), lines: vec![] });
        assert_eq!(build_of(&r, "1.2.3"), Some(9));
        assert_eq!(build_of(&r, "1.2.2"), None);
        assert_eq!(build_of(&r, "9.9.9"), None);
    }

    #[test]
    fn about_empty_changelog_has_no_releases() {
        assert!(parse_changelog("").is_empty());
        assert!(parse_changelog("# Changelog\n- stray bullet\n").is_empty());
    }

    #[test]
    fn about_real_changelog_starts_with_the_current_version() {
        let r = parse_changelog(CHANGELOG);
        assert!(!r.is_empty());
        assert_eq!(r[0].version, VERSION, "the newest changelog heading must be the workspace version");
        assert!(r[0].build.is_some(), "the current release names its build");
        assert!(!r[0].lines.is_empty());
        assert!(r.iter().all(|x| !x.version.is_empty()));
    }

    #[test]
    fn about_logo_decodes() {
        let img = image::load_from_memory_with_format(LOGO_PNG, image::ImageFormat::Png).unwrap();
        assert!(img.width() > 0 && img.height() > 0);
    }
}
