#![allow(dead_code)]
use darkburn_text::FontLibrary;
use std::path::PathBuf;

pub fn font_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("fonts")
}

/// Library with only the two OFL test fonts (IBM Plex Mono static, IBM Plex Sans variable).
pub fn lib() -> FontLibrary {
    let mut l = FontLibrary::new();
    l.load_font_file(font_dir().join("IBMPlexMono-Regular.ttf")).unwrap();
    l.load_font_file(font_dir().join("IBMPlexSans-Variable.ttf")).unwrap();
    l.set_default_family("IBM Plex Sans");
    l
}

pub const MONO: &str = "IBM Plex Mono";
pub const SANS: &str = "IBM Plex Sans";
