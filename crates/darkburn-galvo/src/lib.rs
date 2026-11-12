//! EzCad2 / BJJCZ "LMC" galvo laser support for DarkBurn (fibre, MOPA, UV and CO2 galvo).
//!
//! * [`codec`]: the 12-byte command format, 3072-byte list packets, command enums, replies.
//! * [`cor`]: `.cor` lens correction files and their upload sequence.
//! * [`field`]: field size / offset, galvo 1/2 calibration, mm <-> galvo units, the lens test
//!   pattern.
//! * [`settings`]: laser source, mark parameters, timings, ports and device settings.
//! * [`geom`]: flattening, hatch, wobble, clipping.
//! * [`job`]: the job compiler (marks, fills, flood fill, wobble, repeat, rotary, 3D slices,
//!   door-protect stages, red-light frames) with a time estimate.
//! * [`transport`]: the [`Transport`] trait, an in-memory link and (feature `usb`) real USB.
//! * [`emulator`]: an in-memory board used by every test.
//! * [`machine`]: the threaded driver, [`GalvoMachine`] (threading model and safety rules are
//!   documented there).
//! * [`ezcad`]: EZCad config import.
//!
//! No real USB device is ever opened unless the `usb` feature is on and
//! `UsbTransport::open` is called by the application.

// The lint named below is new; an older clippy does not know it and would stop on the name.
#![allow(unknown_lints)]
#![allow(clippy::chunks_exact_to_as_chunks)]

pub mod codec;
pub mod cor;
pub mod emulator;
pub mod ezcad;
pub mod field;
pub mod geom;
pub mod job;
pub mod machine;
pub mod settings;
pub mod transport;

pub use codec::{Command, ListCmd, ListPacket, Reply, SingleCmd, packetize, unpacketize};
pub use cor::{CorError, CorTable};
pub use emulator::{EmulatorConfig, EmulatorLog, GalvoEmulator, Motion, PointShot};
pub use ezcad::{EzcadImport, ImportError};
pub use field::{Field, FieldConfig, FieldOrigin, GalvoCal, LensTestPattern, OutOfField};
pub use geom::Wobble;
pub use job::{
    AngleMode, CompileError, CompileOptions, CompiledFrame, CompiledJob, DrillSettings, Estimate, FillGrouping, FillSettings, FrameSpec, Job, Layer,
    LayerMode, RepeatMarking, RotaryConfig, RotaryMode, SlicedImage, Step, bounds_key, compile, compile_frame, drill_points,
};
pub use machine::{Activity, ConnectOptions, ConnectionState, Event, GalvoError, GalvoMachine, Inputs, JobProgress, Snapshot, StartOptions};
pub use settings::{DeviceSettings, LaserSource, LightSettings, MarkParams, PortConfig, Timings};
#[cfg(feature = "usb")]
pub use transport::UsbTransport;
pub use transport::{EmulatorHandle, MemoryTransport, Transport};
