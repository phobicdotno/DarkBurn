//! DarkBurn geometry: pure functions over [`kurbo::BezPath`] in millimetres, Y up.
//!
//! Every tolerance is a parameter (mm). Nothing here depends on other DarkBurn crates.
//!
//! - [`path`]: contours, flattening, closed/open, area, length, bounds, direction, start point
//! - [`query`]: point in shape, hit testing, nearest point, containment/nesting, intersections, measuring
//! - [`boolean`]: union, difference, intersection, xor, weld, cut shapes
//! - [`offset`]: offsets with joins, open-path offsets, kerf offsets
//! - [`cleanup`]: close, auto-join, duplicates, overlapping lines, break apart, optimize, flatten
//! - [`node`]: node-level edits; [`fillet`]: round corners and chamfers
//! - [`tabs`]: tabs/bridges, perforation, lead-in/out; [`slots`]: slot resizing; [`hull`]: rubber-band outline
//! - [`transform`]: warp, deform, mirror, two-point rotate/scale; [`array`]: grid, circular, along path
//! - [`nest`]: simple strip nesting

pub use kurbo;

pub mod array;
pub mod boolean;
pub mod cleanup;
pub mod fillet;
pub mod hull;
pub mod nest;
pub mod node;
pub mod offset;
pub mod path;
pub mod query;
pub mod slots;
pub mod tabs;
pub mod transform;

pub use path::{Contour, Direction, FillRule, Polyline};
