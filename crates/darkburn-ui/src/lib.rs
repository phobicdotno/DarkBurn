//! DarkBurn's widget kit.
//!
//! Most of this crate is copied from SolidCraft (<https://github.com/tangency-labs/solidcraft>,
//! MIT OR Apache-2.0): the `ui_kit` widgets, the bundled IBM Plex fonts, the SVG icon renderer,
//! the theme files and the title-bar geometry. See ATTRIBUTION.md at the repository root.
#![allow(dead_code, unused_imports, clippy::all)]

pub mod blur;
pub mod fonts;
pub mod frame;
pub mod icon_svg;
pub mod icons;
pub mod scenario;
pub mod theme_files;
pub mod theme_options;
pub mod titlebar;
pub mod ui_kit;
pub mod value_step;

pub use solidcraft_theme as theme;
