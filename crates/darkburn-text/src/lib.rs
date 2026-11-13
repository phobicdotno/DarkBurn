//! DarkBurn text: font outlines, SHX single-line fonts, text on path, QR codes, barcodes and
//! variable text. All geometry is `kurbo::BezPath` in millimetres, Y up.

pub mod barcode;
pub mod error;
pub mod fontprefs;
pub mod fonts;
pub mod geomutil;
pub mod layout;
pub mod qr;
pub mod shx;
pub mod textpath;
pub mod vartext;

pub use barcode::{BarcodeKind, BarcodeResult, BarcodeSpec};
pub use error::{Result, TextError};
pub use fontprefs::{FontListing, FontPrefs, list_families, preview_outline};
pub use fonts::{FaceData, FaceId, FontLibrary, ResolvedFace};
pub use layout::{
    BackupGlyph, BackupOutline, Direction, GlyphOutline, HAlign, LayoutSource, LineMetrics, SizeMode, TextLayout, TextMetrics, TextSpec, VAlign,
    font_missing, layout_text, layout_with_backup,
};
pub use qr::{ContactCard, EccLevel, MAX_QR_BYTES, QrContent, QrResult, QrSpec, WifiSecurity};
pub use shx::ShxFont;
pub use textpath::{PathSampler, PathTextOptions, apply_path_to_text, text_on_path};
pub use vartext::{CutValues, EvalContext, MergeSource, SerialConfig, VarKind, VariableState};
