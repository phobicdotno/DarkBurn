//! Byte transports between the driver and a controller.
//!
//! The driver only ever talks through [`Transport`]. Tests and demos use [`MemoryTransport`],
//! which is wired straight into the in-memory [`GrblEmulator`].
//! [`SerialTransport`] (cargo feature `serial`) and [`TcpTransport`] reach real hardware and are
//! only opened when the application explicitly asks for a port or address.

use std::io;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::emulator::GrblEmulator;

/// A bidirectional byte link to a controller.
///
/// Implementations are moved into the driver's worker thread, so they must be `Send`.
pub trait Transport: Send {
    /// Writes every byte or fails.
    fn write_all(&mut self, data: &[u8]) -> io::Result<()>;
    /// Reads what is available, waiting at most `timeout`. Returns `Ok(0)` on timeout.
    /// An `Err` means the link is gone.
    fn read(&mut self, buf: &mut [u8], timeout: Duration) -> io::Result<usize>;
    /// Sets the DTR line where the link has one. Toggling DTR resets most Arduino-based
    /// GRBL boards. Links without DTR ignore it.
    fn set_dtr(&mut self, _level: bool) -> io::Result<()> {
        Ok(())
    }
    /// A human name for logs and the console, e.g. `COM7 @ 115200` or `emulator`.
    fn name(&self) -> String;
}

/// Description of a serial port found by [`list_ports`]. Listing never opens a port.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PortInfo {
    pub name: String,
    pub vid: Option<u16>,
    pub pid: Option<u16>,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
    pub serial_number: Option<String>,
    /// The operating system says this is a Bluetooth serial port.
    pub bluetooth: bool,
}

impl PortInfo {
    /// A label for a port picker, e.g. `COM7 (USB-SERIAL CH340, 1A86:7523)`.
    pub fn label(&self) -> String {
        let mut extra = Vec::new();
        if let Some(p) = &self.product {
            extra.push(p.clone());
        }
        if let (Some(v), Some(p)) = (self.vid, self.pid) {
            extra.push(format!("{v:04X}:{p:04X}"));
        }
        if self.bluetooth {
            extra.push("Bluetooth".to_owned());
        }
        if extra.is_empty() { self.name.clone() } else { format!("{} ({})", self.name, extra.join(", ")) }
    }
}

/// Lists serial ports without opening any of them. Returns an empty list when the `serial`
/// feature is off.
pub fn list_ports() -> Vec<PortInfo> {
    #[cfg(feature = "serial")]
    {
        serial::list()
    }
    #[cfg(not(feature = "serial"))]
    {
        Vec::new()
    }
}

#[cfg(feature = "serial")]
pub use serial::SerialTransport;

#[cfg(feature = "serial")]
mod serial {
    use super::*;

    pub(super) fn list() -> Vec<PortInfo> {
        let Ok(ports) = serialport::available_ports() else {
            return Vec::new();
        };
        ports
            .into_iter()
            .map(|p| match p.port_type {
                serialport::SerialPortType::UsbPort(u) => PortInfo {
                    name: p.port_name,
                    vid: Some(u.vid),
                    pid: Some(u.pid),
                    manufacturer: u.manufacturer,
                    product: u.product,
                    serial_number: u.serial_number,
                    bluetooth: false,
                },
                serialport::SerialPortType::BluetoothPort => PortInfo { name: p.port_name, bluetooth: true, ..Default::default() },
                _ => PortInfo { name: p.port_name, ..Default::default() },
            })
            .collect()
    }

    /// A USB serial port (8N1, no flow control). Only opened by [`SerialTransport::open`].
    pub struct SerialTransport {
        port: Box<dyn serialport::SerialPort>,
        label: String,
    }

    impl SerialTransport {
        /// Opens a real serial port. Never called by tests; only the application calls this
        /// after the user picks a port.
        pub fn open(path: &str, baud: u32) -> io::Result<Self> {
            let port = serialport::new(path, baud)
                .data_bits(serialport::DataBits::Eight)
                .parity(serialport::Parity::None)
                .stop_bits(serialport::StopBits::One)
                .flow_control(serialport::FlowControl::None)
                .timeout(Duration::from_millis(10))
                .open()
                .map_err(io::Error::other)?;
            Ok(Self { port, label: format!("{path} @ {baud}") })
        }
    }

    impl Transport for SerialTransport {
        fn write_all(&mut self, data: &[u8]) -> io::Result<()> {
            io::Write::write_all(&mut self.port, data)?;
            io::Write::flush(&mut self.port)
        }
        fn read(&mut self, buf: &mut [u8], timeout: Duration) -> io::Result<usize> {
            self.port.set_timeout(timeout.max(Duration::from_millis(1))).map_err(io::Error::other)?;
            match io::Read::read(&mut self.port, buf) {
                Ok(n) => Ok(n),
                Err(e) if matches!(e.kind(), io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock) => Ok(0),
                Err(e) => Err(e),
            }
        }
        fn set_dtr(&mut self, level: bool) -> io::Result<()> {
            self.port.write_data_terminal_ready(level).map_err(io::Error::other)
        }
        fn name(&self) -> String {
            self.label.clone()
        }
    }
}

/// GRBL over TCP (FluidNC Telnet on port 23, grblHAL Ethernet, serial bridges).
/// Only connects when the application asks; never used in tests.
pub struct TcpTransport {
    stream: std::net::TcpStream,
    label: String,
}

impl TcpTransport {
    pub fn connect(addr: std::net::SocketAddr, timeout: Duration) -> io::Result<Self> {
        let stream = std::net::TcpStream::connect_timeout(&addr, timeout)?;
        stream.set_nodelay(true)?;
        Ok(Self { stream, label: format!("tcp://{addr}") })
    }
}

impl Transport for TcpTransport {
    fn write_all(&mut self, data: &[u8]) -> io::Result<()> {
        io::Write::write_all(&mut self.stream, data)
    }
    fn read(&mut self, buf: &mut [u8], timeout: Duration) -> io::Result<usize> {
        self.stream.set_read_timeout(Some(timeout.max(Duration::from_millis(1))))?;
        match io::Read::read(&mut self.stream, buf) {
            Ok(0) => Err(io::Error::new(io::ErrorKind::UnexpectedEof, "connection closed")),
            Ok(n) => Ok(n),
            Err(e) if matches!(e.kind(), io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock) => Ok(0),
            Err(e) => Err(e),
        }
    }
    fn name(&self) -> String {
        self.label.clone()
    }
}

/// A shared handle to an emulator. Clone it to keep access to the emulator while a driver owns
/// a [`MemoryTransport`] to it; make a new transport for every (re)connect.
#[derive(Clone)]
pub struct EmulatorHandle(Arc<Mutex<GrblEmulator>>);

impl EmulatorHandle {
    pub fn new(emulator: GrblEmulator) -> Self {
        Self(Arc::new(Mutex::new(emulator)))
    }
    /// Opens a new in-memory link, like plugging in the USB cable.
    pub fn transport(&self) -> MemoryTransport {
        self.with(|e| e.port_opened());
        MemoryTransport { emu: self.clone(), last: Instant::now(), pending: Vec::new() }
    }
    /// Runs `f` with the emulator locked (for test hooks and inspection).
    pub fn with<R>(&self, f: impl FnOnce(&mut GrblEmulator) -> R) -> R {
        let mut guard = self.0.lock().unwrap_or_else(|p| p.into_inner());
        f(&mut guard)
    }
}

/// In-memory link to a [`GrblEmulator`]. The emulator's clock advances while the driver reads.
pub struct MemoryTransport {
    emu: EmulatorHandle,
    last: Instant,
    pending: Vec<u8>,
}

impl MemoryTransport {
    fn pump(&mut self) -> io::Result<()> {
        let now = Instant::now();
        let dt = now - self.last;
        self.last = now;
        self.emu.with(|e| {
            if e.is_unplugged() {
                return Err(io::Error::new(io::ErrorKind::BrokenPipe, "emulator unplugged"));
            }
            e.advance(dt);
            self.pending.extend(e.take_output());
            Ok(())
        })
    }
}

impl Transport for MemoryTransport {
    fn write_all(&mut self, data: &[u8]) -> io::Result<()> {
        self.pump()?;
        self.emu.with(|e| e.receive(data));
        Ok(())
    }
    fn read(&mut self, buf: &mut [u8], timeout: Duration) -> io::Result<usize> {
        let start = Instant::now();
        loop {
            self.pump()?;
            if !self.pending.is_empty() {
                let n = buf.len().min(self.pending.len());
                buf[..n].copy_from_slice(&self.pending[..n]);
                self.pending.drain(..n);
                return Ok(n);
            }
            let waited = start.elapsed();
            if waited >= timeout {
                return Ok(0);
            }
            std::thread::sleep((timeout - waited).min(Duration::from_millis(1)));
        }
    }
    fn set_dtr(&mut self, level: bool) -> io::Result<()> {
        self.emu.with(|e| e.set_dtr(level));
        Ok(())
    }
    fn name(&self) -> String {
        "emulator".to_owned()
    }
}

impl Drop for MemoryTransport {
    fn drop(&mut self) {
        self.emu.with(|e| e.port_closed());
    }
}
