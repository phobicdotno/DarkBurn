//! G-code machine drivers for DarkBurn other than GRBL.
//!
//! * [`machine`]: the [`GcodeMachine`] trait every G-code controller implements, plus the
//!   shared snapshot, event and request types. [`GrblAdapter`] wraps
//!   [`darkburn_grbl::GrblMachine`] so the application treats all controllers alike.
//! * [`line`]: [`LineMachine`], one worker-thread driver for `ok`-paced controllers
//!   (Smoothieware, Marlin, Snapmaker, custom devices). Same threading model as the GRBL
//!   driver: commands go to a worker over a channel, events come back over a second channel
//!   ([`GcodeMachine::poll_events`]) and the latest state sits in a snapshot.
//! * [`smoothie`], [`marlin`], [`snapmaker`], [`custom`]: the per-firmware configuration.
//! * [`linuxcnc`]: file output only (no live link; see that module).
//! * [`translate`]: rewrites a GRBL-style job stream into a target dialect.
//! * [`profile`]: the device profile (all Device Settings) with JSON import and export.
//! * [`detect`]: firmware guess from a banner string (no port probing).
//! * [`emu`]: in-memory Smoothie, Marlin, Snapmaker and generic controllers for the tests.
//!
//! Nothing here opens a port or a network connection by itself. Drivers talk through the
//! [`Transport`] trait from `darkburn-grbl`.

pub mod custom;
pub mod detect;
pub mod emu;
pub mod grbl_adapter;
pub mod line;
pub mod linuxcnc;
pub mod machine;
pub mod marlin;
pub mod profile;
pub mod smoothie;
pub mod snapmaker;
pub mod template;
pub mod translate;

pub use darkburn_grbl::Transport;
pub use detect::{DetectResult, FirmwareGuess, baud_candidates, identify, identify_lines};
pub use emu::{EmuConfig, EmuFlavor, LineEmuHandle, LineEmulator, LineTransport};
pub use grbl_adapter::GrblAdapter;
pub use line::{LineConfig, LineMachine};
pub use machine::{Capabilities, GcodeMachine, MachineError, MachineEvent, MachineFamily, MachineSnapshot};
pub use profile::DeviceProfile;
