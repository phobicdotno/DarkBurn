//! Lihuiyu "M2 Nano" family support for DarkBurn: the stock boards of K40 CO2 lasers.
//!
//! These boards run EGV ("LHYMICRO-GL"), a text language of relative moves in mils, sent in
//! 30-byte packets through a CH341 USB chip. The laser is only switched on and off; the power
//! is set by hand on the machine.
//!
//! * [`distance`]: how distances are written.
//! * [`speed`]: speed codes for every board (M2, M3, B2, M, M1, A, B, B1).
//! * [`egv`]: the EGV writer (rapid, program and raster modes).
//! * [`toolpath`]: a DarkBurn toolpath to EGV ([`toolpath_to_egv`]).
//! * [`egv_file`]: `.egv` files.
//! * [`packet`]: packet framing, CRC and status bytes.
//! * [`emulator`]: an EGV reader and an in-memory board used by every test.
//! * [`transport`]: the [`Transport`] trait, an in-memory link and the send loop.
//!
//! Ported from MeerK40t's `meerk40t/lihuiyu` (MIT, commit
//! `d511944e08788e88077727f682957759884ff5ca`), rewritten in Rust; see `ATTRIBUTION.md`.
//!
//! This crate never opens a USB device. A real CH341 link (USB `1a86:5512`) is left out on
//! purpose until the owner approves it and it has been tried on a machine.

pub mod distance;
pub mod egv;
pub mod egv_file;
pub mod emulator;
pub mod packet;
pub mod speed;
pub mod toolpath;
pub mod transport;

pub use egv::{EgvWriter, WriterOptions};
pub use egv_file::{egv_body, write_egv};
pub use emulator::EgvReader;
pub use speed::Board;
pub use toolpath::{ConvertError, EgvJob, K40Settings, toolpath_to_egv};
pub use transport::{MemoryTransport, SendOptions, Transport, send};
