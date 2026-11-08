//! DarkBurn job engine.
//!
//! This crate sits between the drawing and the machine. The app maps its document into a
//! [`JobInput`] (layers with cut settings, shapes as `kurbo::BezPath` in millimetres with Y up,
//! and optional raster scanlines) and gets back a [`Toolpath`]: a flat, serialisable list of
//! machine operations. From the toolpath it can
//!
//! - estimate time and simulate the job for the preview ([`sim`]),
//! - start part-way through ([`Toolpath::slice_from`]),
//! - check the job against the work area ([`check_bounds`]),
//! - (G-code output follows).
//!
//! The pipeline inside [`generate`] is: flatten shapes, build cut units per layer mode
//! (line, fill, fill + line, offset fill, image), plan the cut order ([`planner`]), expand the
//! units into operations, then map design coordinates to machine coordinates ([`origin`]).

pub mod bounds;
pub mod fill;
pub mod generate;
pub mod geom;
pub mod input;
pub mod ir;
pub mod line;
pub mod origin;
pub mod planner;
pub mod progress;
pub mod sim;

pub use bounds::{BoundsReport, OutOfBounds, check_bounds};
pub use generate::{JobError, JobOptions, generate};
pub use input::*;
pub use ir::{CoordSpace, IrLayer, Op, Toolpath};
pub use origin::{Corner, FinishPosition, JobAnchor, MachineGeometry, OriginSettings, StartFrom};
pub use planner::{OrderKey, PlannerSettings, ScanDirection};
pub use progress::{Cancelled, Progress};
pub use sim::{MotionSettings, SimState, Simulation};
