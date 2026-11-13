//! # darkburn-mcc
//!
//! Library support for the Gweike M3 Ultra (Au3tech MCC100 motion card, 1200 W fibre plus CO2
//! flatbed): a frame codec, a register map with typed writes, FIFO encoding and a planner dry run,
//! `.chf` craft-file reading and writing, fibre/CO2 craft parameters, and an in-memory card
//! simulator with a safety-gate model.
//!
//! ## What this crate is not
//!
//! * **It has no transport.** There is no UDP socket, TCP stream or serial port anywhere in it, and
//!   no dependency that could open one. Frames are byte vectors. A future backend owns the socket
//!   and goes through [`gate::SafetyGate`], [`frame::Request`] and [`command::SafeWrite`].
//! * **It does not bypass the machine's interlocks.** The height follower, the pierce sequence and
//!   the gas logic stay in charge. The planner emits no laser, gas, pierce or follower data.
//! * **It cannot write the forbidden registers.** [`command::SafeWrite`] has private fields and one
//!   allow-list classifier. Registers 150 and 151, the licence and clock area (59500 to 59599),
//!   hardware parameters (59600 and up), follower properties, register 100, block 5000 to 5012,
//!   the test-data window and the single-word commands `0x65 <- [0x65]` and `0x65 <- [0x66]` are
//!   refused, whether built through the typed constructors, decoded from the wire or passed raw
//!   to [`command::SafeWrite::try_from_raw`]. Byte-block (0x26) and stream (0x20, 0x21) requests
//!   cannot be built at all. Laser-enabling data cannot be encoded through the public API.
//! * **It has no remote-monitor or cloud support.** The vendor software's monitor endpoint is
//!   absent from this crate.
//!
//! ## Real-card use needs a capture first
//!
//! Everything here is derived from the owner's own static analysis of the vendor software
//! (`phobicMlaserLinux`, clean-room, never from the vendor's code) and from a handful of logged
//! frames. No traffic has been captured from the real card. `99-gaps.md` lists what is still open:
//! stop and E-stop vectors beyond what the logs show, status and alarm bit decoding, the tick
//! period, the unit of register 1016, and the resume primitive. Until the read-only capture
//! described there has closed those gaps, nothing from this crate may be sent to a real machine.
//! The list of unconfirmed facts is in `docs/hardware-checks-mcc.md`.
//!
//! ## Map
//!
//! | Module | Contents |
//! |---|---|
//! | [`crc`] | CRC-16/MODBUS and the two stored byte orders |
//! | [`frame`] | [`frame::RawFrame`], [`frame::Request`], [`frame::Reply`], function codes |
//! | [`registers`] | blocks, status words, alarm and follower bits, read allow-list, [`registers::Forbidden`] |
//! | [`command`] | [`command::SafeWrite`] and its constructors, the classifier, [`command::SafetyConfig`] |
//! | [`fifo`] | 12-byte tick items, frame packing, flow control ([`fifo::FifoFeeder`]) |
//! | [`planner`] | toolpath to ticks and a time estimate ([`planner::dry_run`]) |
//! | [`chf`] | `.chf` reader and writer (MCC-02) |
//! | [`craft`] | [`craft::FibreCraft`], [`craft::Co2Craft`] and validation (MCC-04) |
//! | [`paramxml`] | tolerant reader and byte-faithful writer of Mlaser's parameter XML |
//! | [`recipe`] | Mlaser recipe files and layer tables to and from crafts, recipe-name parsing |
//! | [`profile`] | Mlaser machine settings merged into a [`profile::MachineProfile`], typed views, diff |
//! | [`mlaser_files`] | compensation tables, `.ini` files, `scFlie` value lists, job statistics |
//! | [`library`] | folder scanner: recipes, settings, small files and unreadable files ([`library::scan`]) |
//! | [`gate`] | arming state machine and PC-side checks |
//! | [`sim`] | [`sim::CardSimulator`] (MCC-05) and the loopback [`sim::SimLink`] |
//!
//! DXF with one layer per material and the `.nc` export live in `darkburn-import`.

pub mod chf;
pub mod command;
pub mod craft;
pub mod crc;
pub mod fifo;
pub mod frame;
pub mod gate;
pub mod library;
pub mod mlaser_files;
pub mod paramxml;
pub mod planner;
pub mod profile;
pub mod recipe;
pub mod registers;
pub mod sim;

pub use chf::{ChfDocument, ChfError, read_chf, write_chf};
pub use command::{MachineParams, Policy, SafeWrite, SafetyConfig};
pub use craft::{Co2Craft, Craft, CraftTable, FibreCraft, GasType, PierceStage, ProcessType};
pub use crc::{CrcOrder, crc16_modbus};
pub use fifo::{FifoFeeder, FifoItem, PackedFrame};
pub use frame::{RawFrame, Reply, Request, RequestError};
pub use gate::{ArmState, ArmingStateMachine, SafetyGate};
pub use library::{Library, RecipeKind, scan};
pub use mlaser_files::{CompensationTable, IniFile, ScFile};
pub use paramxml::{MlaserError, ParamDoc};
pub use planner::{DryRunPlan, PathSegment, PlanConfig, dry_run};
pub use profile::MachineProfile;
pub use recipe::{LayerKind, LayerTable, RecipeFile, RecipeName, read_recipe, write_recipe};
pub use registers::Forbidden;
pub use sim::{CardSimulator, SimConfig, SimLink};
