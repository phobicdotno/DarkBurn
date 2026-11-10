//! Ruida DSP laser controller support for DarkBurn.
//!
//! * [`swizzle`], [`encoding`], [`packet`], [`command`]: the wire codec. Byte scrambling with any
//!   magic value, number encodings, UDP checksums and framing, and a typed [`Command`] enum.
//! * [`rd`]: `.rd` job files, written from and read into a neutral [`RdJob`].
//! * [`transport`]: the [`Transport`] trait with UDP, USB (the FTDI serial port with feature
//!   `serial`, raw FTDI with feature `usb`) and in-memory links.
//! * [`loopback`]: the emulator behind UDP sockets on 127.0.0.1, for transport tests.
//! * [`emulator`]: [`RuidaEmulator`], a software controller used by every test.
//! * [`machine`]: [`RuidaMachine`], the blocking driver.
//! * [`worker`]: [`RuidaHandle`], the thread + channel wrapper an egui app polls each frame.
//! * [`settings`]: controller memory addresses, backups, controller info and status.
//!
//! Safety: nothing in this crate opens a network or USB connection on its own. Only
//! [`transport::UdpTransport::connect`], `SerialTransport::open` and `UsbTransport::open` do.
//! Tests never call them; the only sockets tests open are the loopback pair of
//! [`loopback::UdpEmulatorServer`].

pub mod command;
pub mod emulator;
pub mod encoding;
pub mod error;
pub mod loopback;
pub mod machine;
pub mod packet;
pub mod rd;
pub mod settings;
pub mod swizzle;
pub mod transport;
pub mod worker;

pub use command::{Axis, Command, JogKey, Laser, LayerFlag, OverscanMode, PanelCommand, PanelKey, RapidOption};
pub use emulator::{EmulatorConfig, MotionKind, MotionRecord, RuidaEmulator};
pub use encoding::Power;
pub use error::{Result, RuidaError};
pub use loopback::UdpEmulatorServer;
pub use machine::{ControllerFile, HomeAxes, MachineConfig, OverwriteChoice, RuidaMachine, SendOptions, SendOutcome};
pub use rd::{DotMode, LayerSettings, RdJob, RdLayer, RdOp, RefPoint};
pub use settings::{ControllerInfo, MachineSettings, MachineState, MachineStatus};
pub use swizzle::Magic;
pub use transport::{ByteStream, Channel, Framing, MemoryTransport, StreamTransport, Transport, UdpPorts, UdpTransport};
pub use worker::{Event, Request, Response, RuidaHandle, WorkerConfig};
