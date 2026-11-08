//! DarkBurn's document model.
//!
//! - Geometry is in millimetres, Y up, origin at the front-left corner of the work area.
//! - Every shape exposes its outline as a [`kurbo::BezPath`] in document coordinates
//!   ([`Shape::outline`]); geometry, raster, job and import crates work on those.
//! - [`Editor`] holds a [`Document`], the [`Selection`], unlimited undo and the clipboard.
//! - [`project`] reads and writes `.dbrn` project files.

pub mod arrange;
pub mod clipboard;
pub mod document;
pub mod editor;
pub mod layer;
pub mod project;
pub mod select;
mod serde_kurbo;
pub mod shape;

pub use clipboard::{Clipboard, PasteAt};
pub use document::{Document, ImageData, ImageStore};
pub use editor::{Editor, History, Selection};
pub use kurbo;
pub use layer::{CutSetting, ImageMode, LAYER_COUNT, Layer, MAX_SUB_LAYERS, Mode, T1, T2};
pub use shape::{ImageAdjust, ImageShape, Shape, ShapeId, ShapeKind, TextShape};
