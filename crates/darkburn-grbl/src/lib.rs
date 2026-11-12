//! GRBL driver for DarkBurn.
//!
//! * [`protocol`]: the GRBL 1.1 wire protocol (real-time bytes, responses, status reports).
//! * [`codes`]: error, alarm and setting tables in plain English.
//! * [`transport`]: the [`Transport`] trait with serial (feature `serial`), TCP and in-memory
//!   links.
//! * [`rawusb`]: raw USB bulk endpoints (feature `rawusb`), behind the same trait.
//! * [`detect`]: recognising common diode lasers from their welcome and `$I` text.
//! * [`emulator`]: an in-memory GRBL 1.1 controller used by every test.
//! * [`machine`]: the driver, [`GrblMachine`] (threading model documented there).

pub mod codes;
pub mod detect;
pub mod emulator;
pub mod machine;
pub mod protocol;
pub mod rawusb;
pub mod transport;

pub use codes::{SettingInfo, alarm_message, error_message, setting_info};
pub use detect::{DetectedLaser, LaserDetector, identify_laser};
pub use emulator::{EmulatorConfig, EmulatorFlavor, EmulatorStats, GrblEmulator};
pub use machine::{
    Activity, BoundsCheck, ConnectOptions, ConnectionState, ConsoleKind, ConsoleLine, Coords, EtaHook, Event, FinishPosition, FrameOptions,
    GrblError, GrblMachine, Job, JobKind, JobProgress, JogDirection, JogStep, LaserStatus, MachineStatus, MoveTarget, OriginStrategy, Overrides,
    SettingValue, SettingsBackup, Snapshot, StreamMode, TravelLimits,
};
pub use protocol::{FirmwareInfo, FirmwareKind, MachineState, RapidOverride, Response, StatusReport};
#[cfg(feature = "rawusb")]
pub use rawusb::UsbPipe;
pub use rawusb::{BulkPipe, RawUsbAddress, RawUsbTransport, UsbDeviceInfo, list_usb_devices};
#[cfg(feature = "serial")]
pub use transport::SerialTransport;
pub use transport::{EmulatorHandle, MemoryTransport, PortInfo, TcpTransport, Transport, list_ports};
