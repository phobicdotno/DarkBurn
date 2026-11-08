//! DarkBurn file import and export.
//!
//! Every importer turns a file into the neutral [`Imported`] model (paths in millimetres,
//! Y up, with colours, layers, groups, text records and embedded images). Exporters take
//! a slice of [`Item`]s and write a file.
//!
//! | Format | Import | Export |
//! |---|---|---|
//! | DXF | [`dxf_import`] | [`dxf_export`] |
//! | PDF and PDF-compatible Adobe Illustrator | [`pdf_import`] | [`ai_export`] (AI/PDF) |
//! | PLT / HPGL | [`hpgl`] | [`hpgl`] |
//! | LightBurn `.lbrn` / `.lbrn2` | [`lbrn`] | [`lbrn`] (`.lbrn2`) |
//! | SVG | done in the app with usvg | [`svg_export`] |
//! | SignCut `.sc` | not publicly documented: [`ImportError::Unsupported`] | - |
//! | Gweike / Mlaser | - | [`gweike`] (DXF per material, `.nc` G-code) |
//!
//! SVG import is not here on purpose; the app uses usvg for it.

pub mod ai_export;
pub mod dxf_export;
pub mod dxf_import;
pub mod geom;
pub mod gweike;
pub mod hpgl;
pub mod lbrn;
pub mod model;
pub mod pdf_import;
pub mod svg_export;
mod xml;

use std::fmt;

pub use model::*;

/// Why an import failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImportError {
    /// The file is of a kind or version this crate cannot read. The message says why and what to do.
    Unsupported(String),
    /// The file is damaged or not what its name says.
    Parse(String),
}

impl fmt::Display for ImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ImportError::Unsupported(m) => write!(f, "unsupported file: {m}"),
            ImportError::Parse(m) => write!(f, "could not read file: {m}"),
        }
    }
}

impl std::error::Error for ImportError {}

/// The file formats this crate reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Format {
    Dxf,
    Pdf,
    /// Adobe Illustrator. PDF-compatible files go through the PDF reader.
    Ai,
    Hpgl,
    /// LightBurn project, `.lbrn` or `.lbrn2`.
    LightBurn,
    /// SignCut `.sc` / `.scpro2` (recognised, not readable).
    SignCut,
}

impl Format {
    /// Picks a format from a file extension (without the dot, any case).
    pub fn from_extension(ext: &str) -> Option<Format> {
        Some(match ext.to_ascii_lowercase().as_str() {
            "dxf" => Format::Dxf,
            "pdf" => Format::Pdf,
            "ai" => Format::Ai,
            "plt" | "hpgl" | "hgl" | "hpg" => Format::Hpgl,
            "lbrn" | "lbrn2" => Format::LightBurn,
            "sc" | "scpro2" | "scpro" => Format::SignCut,
            _ => return None,
        })
    }
}

/// Guesses the format from the file name (if any) and the first bytes.
pub fn detect_format(file_name: Option<&str>, bytes: &[u8]) -> Option<Format> {
    let head = &bytes[..bytes.len().min(1024)];
    let head_text = String::from_utf8_lossy(head);
    let trimmed = head_text.trim_start_matches('\u{feff}').trim_start();
    let by_name = file_name.and_then(|n| n.rsplit_once('.')).and_then(|(_, e)| Format::from_extension(e));
    if trimmed.starts_with("%PDF") {
        return Some(if by_name == Some(Format::Ai) { Format::Ai } else { Format::Pdf });
    }
    if trimmed.starts_with("%!PS-Adobe") && by_name == Some(Format::Ai) {
        return Some(Format::Ai);
    }
    if head_text.contains("<LightBurnProject") {
        return Some(Format::LightBurn);
    }
    if head.starts_with(b"AutoCAD Binary DXF") {
        return Some(Format::Dxf);
    }
    if by_name.is_some() {
        return by_name;
    }
    let lines: Vec<&str> = trimmed.lines().take(4).map(str::trim).collect();
    if lines.len() >= 2 && lines[0] == "0" && lines[1] == "SECTION" {
        return Some(Format::Dxf);
    }
    let upper = trimmed.to_ascii_uppercase();
    if upper.starts_with("IN;") || upper.starts_with("IN ") || upper.starts_with("\u{1b}.") || upper.starts_with("SP") {
        return Some(Format::Hpgl);
    }
    None
}

/// Options for all importers.
#[derive(Clone, Debug, Default)]
pub struct ImportOptions {
    pub dxf: dxf_import::DxfOptions,
    pub pdf: pdf_import::PdfOptions,
    pub hpgl: hpgl::HpglOptions,
}

/// Imports a file whose format is already known.
pub fn import(format: Format, bytes: &[u8], options: &ImportOptions) -> Result<Imported, ImportError> {
    match format {
        Format::Dxf => dxf_import::import(bytes, &options.dxf),
        Format::Pdf => pdf_import::import(bytes, &options.pdf),
        Format::Ai => pdf_import::import_ai(bytes, &options.pdf),
        Format::Hpgl => hpgl::import(bytes, &options.hpgl),
        Format::LightBurn => lbrn::import(bytes),
        Format::SignCut => Err(ImportError::Unsupported(SIGNCUT_UNSUPPORTED.into())),
    }
}

/// Detects the format and imports.
pub fn import_auto(file_name: Option<&str>, bytes: &[u8], options: &ImportOptions) -> Result<Imported, ImportError> {
    match detect_format(file_name, bytes) {
        Some(f) => import(f, bytes, options),
        None => Err(ImportError::Unsupported("unknown file type".into())),
    }
}

/// SignCut's `.sc`/`.scpro2` layout has no public description, so it is not read.
pub const SIGNCUT_UNSUPPORTED: &str = "SignCut files (.sc, .scpro2) use an undocumented format and cannot be opened. \
     Export the design from SignCut as PLT/HPGL, DXF or PDF and import that instead.";

/// Largest coordinate kept on import (mm). Anything beyond 1000 km is treated as damage.
pub const MAX_COORDINATE: f64 = 1e9;

/// Drops items with non-finite or absurd coordinates so later stages stay well-behaved.
pub(crate) fn finish(mut doc: Imported) -> Imported {
    let before = doc.items.len();
    doc.items.retain(|i| i.bounds().is_none_or(|b| [b.x0, b.y0, b.x1, b.y1].iter().all(|v| v.is_finite() && v.abs() <= MAX_COORDINATE)));
    let dropped = before - doc.items.len();
    if dropped > 0 {
        doc.warn(format!("{dropped} objects with impossible coordinates were left out"));
    }
    doc
}

/// Runs a third-party parser and turns a panic inside it into an error, so damaged files
/// can never take the app down.
pub(crate) fn guard<T>(what: &str, f: impl FnOnce() -> T) -> Result<T, ImportError> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).map_err(|_| ImportError::Parse(format!("{what}: internal parser failure")))
}
