//! GRBL driver for DarkBurn.
//!
//! * [`protocol`]: the GRBL 1.1 wire protocol (real-time bytes, responses, status reports).
//! * [`codes`]: error, alarm and setting tables in plain English.
//! * [`transport`]: the [`Transport`] trait with serial (feature `serial`), TCP and in-memory
//!   links.
//! * [`emulator`]: an in-memory GRBL 1.1 controller used by every test.

pub mod codes;
pub mod emulator;
pub mod protocol;
pub mod transport;

pub use codes::{SettingInfo, alarm_message, error_message, setting_info};
pub use emulator::{EmulatorConfig, EmulatorFlavor, EmulatorStats, GrblEmulator};
pub use protocol::{FirmwareInfo, FirmwareKind, MachineState, RapidOverride, Response, StatusReport};
#[cfg(feature = "serial")]
pub use transport::SerialTransport;
pub use transport::{EmulatorHandle, MemoryTransport, PortInfo, TcpTransport, Transport, list_ports};
