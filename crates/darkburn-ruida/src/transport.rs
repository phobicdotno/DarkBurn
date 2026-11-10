//! Transports. Every driver call goes through [`Transport`], so tests run against
//! [`MemoryTransport`] + [`crate::emulator::RuidaEmulator`] and never touch the network or USB.
//!
//! * [`UdpTransport`]: Ruida Ethernet (UDP 50200 data, 50207 jog). Tests only ever point it at
//!   [`crate::loopback::UdpEmulatorServer`] on 127.0.0.1.
//! * [`StreamTransport`]: Ruida over a byte stream (USB). `SerialTransport` (feature `serial`)
//!   is the FTDI chip's serial port; tests use an in-memory [`ByteStream`].
//! * `UsbTransport` (feature `usb`): FTDI FT245R packet USB without a serial driver. Never used
//!   in tests.
//! * [`MemoryTransport`]: in-process link to an emulator, with fault injection.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::emulator::RuidaEmulator;
use crate::error::{Result, RuidaError};

/// Which logical channel a message goes on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Channel {
    /// Swizzled command channel (UDP 50200 or the USB stream).
    Data,
    /// Unswizzled jog / wireless-panel channel (UDP 50207). Not available on USB.
    Jog,
}

/// How the data channel is framed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Framing {
    /// Datagrams with a 2-byte checksum; the controller ACKs every datagram.
    Udp,
    /// A plain byte stream: no checksum, no ACK.
    Stream,
}

/// A link to a controller. `send` sends one datagram (or a block of stream bytes);
/// `recv` returns one received datagram (or whatever stream bytes are available).
pub trait Transport: Send {
    fn framing(&self) -> Framing;
    fn send(&mut self, channel: Channel, data: &[u8]) -> Result<()>;
    /// `Ok(None)` on timeout.
    fn recv(&mut self, channel: Channel, timeout: Duration) -> Result<Option<Vec<u8>>>;
    /// Largest payload to put in one `send` on the data channel.
    fn max_payload(&self) -> usize {
        crate::packet::DEFAULT_CHUNK
    }
    /// Short human-readable description ("UDP 10.0.0.5", "memory").
    fn describe(&self) -> String;
}

impl Transport for Box<dyn Transport> {
    fn framing(&self) -> Framing {
        (**self).framing()
    }
    fn send(&mut self, channel: Channel, data: &[u8]) -> Result<()> {
        (**self).send(channel, data)
    }
    fn recv(&mut self, channel: Channel, timeout: Duration) -> Result<Option<Vec<u8>>> {
        (**self).recv(channel, timeout)
    }
    fn max_payload(&self) -> usize {
        (**self).max_payload()
    }
    fn describe(&self) -> String {
        (**self).describe()
    }
}

/// Faults a [`MemoryTransport`] injects, for testing retries and timeouts.
#[derive(Debug, Clone, Default)]
pub struct Faults {
    /// Drop this many outgoing data datagrams before they reach the emulator (the sender sees a timeout).
    pub drop_sends: usize,
    /// Corrupt the checksum of this many outgoing datagrams (the emulator answers NAK).
    pub corrupt_sends: usize,
    /// Swallow this many replies from the emulator.
    pub drop_replies: usize,
    /// Answer this many datagrams with ERR instead of passing them on.
    pub err_replies: usize,
    /// Count of datagrams that reached the emulator.
    pub delivered: usize,
}

/// In-memory transport wired to a shared emulator.
#[derive(Clone)]
pub struct MemoryTransport {
    emu: Arc<Mutex<RuidaEmulator>>,
    framing: Framing,
    data_rx: Arc<Mutex<VecDeque<Vec<u8>>>>,
    jog_rx: Arc<Mutex<VecDeque<Vec<u8>>>>,
    faults: Arc<Mutex<Faults>>,
    max_payload: usize,
}

impl MemoryTransport {
    /// UDP-style framing (checksums, ACK per datagram).
    pub fn udp(emu: Arc<Mutex<RuidaEmulator>>) -> Self {
        Self::new(emu, Framing::Udp)
    }

    /// USB-style stream framing.
    pub fn stream(emu: Arc<Mutex<RuidaEmulator>>) -> Self {
        Self::new(emu, Framing::Stream)
    }

    pub fn new(emu: Arc<Mutex<RuidaEmulator>>, framing: Framing) -> Self {
        MemoryTransport {
            emu,
            framing,
            data_rx: Default::default(),
            jog_rx: Default::default(),
            faults: Default::default(),
            max_payload: crate::packet::DEFAULT_CHUNK,
        }
    }

    pub fn with_max_payload(mut self, n: usize) -> Self {
        self.max_payload = n;
        self
    }

    /// Shared fault settings; change them while a driver uses this transport.
    pub fn faults(&self) -> Arc<Mutex<Faults>> {
        self.faults.clone()
    }

    pub fn emulator(&self) -> Arc<Mutex<RuidaEmulator>> {
        self.emu.clone()
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Transport for MemoryTransport {
    fn framing(&self) -> Framing {
        self.framing
    }

    fn send(&mut self, channel: Channel, data: &[u8]) -> Result<()> {
        let mut faults = lock(&self.faults);
        let replies: Vec<Vec<u8>> = match (channel, self.framing) {
            (Channel::Jog, Framing::Stream) => return Err(RuidaError::Unsupported("jog port is UDP only".into())),
            (Channel::Jog, Framing::Udp) => lock(&self.emu).handle_jog_datagram(data),
            (Channel::Data, Framing::Udp) => {
                if faults.drop_sends > 0 {
                    faults.drop_sends -= 1;
                    return Ok(());
                }
                let mut d = data.to_vec();
                if faults.corrupt_sends > 0 && d.len() >= 2 {
                    faults.corrupt_sends -= 1;
                    d[1] ^= 0x55;
                }
                if faults.err_replies > 0 {
                    faults.err_replies -= 1;
                    let emu = lock(&self.emu);
                    vec![vec![crate::swizzle::swizzle_byte(0xCD, emu.magic())]]
                } else {
                    faults.delivered += 1;
                    lock(&self.emu).handle_udp_datagram(&d)
                }
            }
            (Channel::Data, Framing::Stream) => {
                faults.delivered += 1;
                let r = lock(&self.emu).handle_stream_bytes(data);
                if r.is_empty() { vec![] } else { vec![r] }
            }
        };
        let q = if channel == Channel::Jog { &self.jog_rx } else { &self.data_rx };
        let mut q = lock(q);
        for r in replies {
            if faults.drop_replies > 0 {
                faults.drop_replies -= 1;
                continue;
            }
            q.push_back(r);
        }
        Ok(())
    }

    fn recv(&mut self, channel: Channel, _timeout: Duration) -> Result<Option<Vec<u8>>> {
        // Everything is synchronous here, so an empty queue is an immediate timeout.
        let q = if channel == Channel::Jog { &self.jog_rx } else { &self.data_rx };
        Ok(lock(q).pop_front())
    }

    fn max_payload(&self) -> usize {
        self.max_payload
    }

    fn describe(&self) -> String {
        format!("memory ({:?})", self.framing)
    }
}

/// Ruida Ethernet over UDP. Binds local ports 40200/40207 and talks to the controller's
/// 50200/50207. It opens real sockets: the app builds one only after the user clicks Connect,
/// and tests only through [`crate::loopback::UdpEmulatorServer::transport`] (127.0.0.1).
pub struct UdpTransport {
    data: std::net::UdpSocket,
    jog: Option<std::net::UdpSocket>,
    peer: std::net::IpAddr,
}

impl UdpTransport {
    /// Open sockets to the controller at `addr`. The jog socket is optional: if port 40207
    /// is taken the data channel still works and jog falls back to the data channel.
    pub fn connect(addr: std::net::IpAddr) -> Result<Self> {
        use crate::packet::*;
        Self::connect_ports(addr, UdpPorts { data: DATA_PORT, jog: JOG_PORT, host_data: HOST_DATA_PORT, host_jog: HOST_JOG_PORT })
    }

    /// As [`connect`](Self::connect) with other port numbers. A host port of 0 lets the system
    /// pick one. A real controller answers to 40200/40207 whatever the source port was (as far
    /// as the public descriptions say), so only an emulator is reached this way.
    pub fn connect_ports(addr: std::net::IpAddr, ports: UdpPorts) -> Result<Self> {
        let any: std::net::IpAddr = if addr.is_loopback() {
            addr
        } else if addr.is_ipv4() {
            std::net::Ipv4Addr::UNSPECIFIED.into()
        } else {
            std::net::Ipv6Addr::UNSPECIFIED.into()
        };
        let data = std::net::UdpSocket::bind((any, ports.host_data))
            .map_err(|e| RuidaError::Transport(format!("could not open UDP port {}: {e} (is another laser program running?)", ports.host_data)))?;
        data.connect((addr, ports.data))?;
        let jog = std::net::UdpSocket::bind((any, ports.host_jog)).and_then(|s| s.connect((addr, ports.jog)).map(|_| s)).ok();
        Ok(UdpTransport { data, jog, peer: addr })
    }

    fn socket(&self, channel: Channel) -> Result<&std::net::UdpSocket> {
        match channel {
            Channel::Data => Ok(&self.data),
            Channel::Jog => self.jog.as_ref().ok_or_else(|| RuidaError::Unsupported("jog port 40207 could not be opened".into())),
        }
    }
}

impl Transport for UdpTransport {
    fn framing(&self) -> Framing {
        Framing::Udp
    }

    fn send(&mut self, channel: Channel, data: &[u8]) -> Result<()> {
        self.socket(channel)?.send(data)?;
        Ok(())
    }

    fn recv(&mut self, channel: Channel, timeout: Duration) -> Result<Option<Vec<u8>>> {
        let s = self.socket(channel)?;
        s.set_read_timeout(Some(timeout.max(Duration::from_millis(1))))?;
        let mut buf = [0u8; 2048];
        match s.recv(&mut buf) {
            Ok(n) => Ok(Some(buf[..n].to_vec())),
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn describe(&self) -> String {
        format!("UDP {}", self.peer)
    }
}

/// The four UDP port numbers of a Ruida link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UdpPorts {
    /// Controller data port (50200).
    pub data: u16,
    /// Controller jog port (50207).
    pub jog: u16,
    /// Host data port (40200); 0 = any free port.
    pub host_data: u16,
    /// Host jog port (40207); 0 = any free port.
    pub host_jog: u16,
}

/// A byte stream to a controller: what the FTDI chip's serial port gives.
pub trait ByteStream: Send {
    fn write_all(&mut self, data: &[u8]) -> std::io::Result<()>;
    /// Read what arrives within `timeout` into `buf`; 0 on timeout.
    fn read_some(&mut self, buf: &mut [u8], timeout: Duration) -> std::io::Result<usize>;
    /// Short name for messages ("COM5").
    fn name(&self) -> String;
}

/// Ruida over a byte stream (USB): the swizzled command stream with no checksum and no ACK,
/// and no jog port.
pub struct StreamTransport<S: ByteStream> {
    stream: S,
}

impl<S: ByteStream> StreamTransport<S> {
    pub fn new(stream: S) -> Self {
        StreamTransport { stream }
    }
    pub fn stream(&self) -> &S {
        &self.stream
    }
}

impl<S: ByteStream> Transport for StreamTransport<S> {
    fn framing(&self) -> Framing {
        Framing::Stream
    }

    fn send(&mut self, channel: Channel, data: &[u8]) -> Result<()> {
        if channel == Channel::Jog {
            return Err(RuidaError::Unsupported("jog port is UDP only".into()));
        }
        self.stream.write_all(data)?;
        Ok(())
    }

    fn recv(&mut self, channel: Channel, timeout: Duration) -> Result<Option<Vec<u8>>> {
        if channel == Channel::Jog {
            return Err(RuidaError::Unsupported("jog port is UDP only".into()));
        }
        let mut buf = [0u8; 1024];
        let n = self.stream.read_some(&mut buf, timeout)?;
        Ok((n > 0).then(|| buf[..n].to_vec()))
    }

    fn describe(&self) -> String {
        format!("USB {}", self.stream.name())
    }
}

/// USB IDs of the FTDI chip on Ruida controllers (FT245R / FT232R).
pub const FTDI_VID: u16 = 0x0403;
pub const FTDI_PID: u16 = 0x6001;

#[cfg(feature = "serial")]
pub use serial::{SerialStream, SerialTransport};

#[cfg(feature = "serial")]
mod serial {
    //! The FTDI chip through the operating system's serial driver (a COM port on Windows,
    //! `/dev/ttyUSB*` on Linux). **Never opened in tests.**

    use super::*;

    /// Baud rate asked for. The FT245R is a parallel FIFO and ignores it (MeerK40t notes the
    /// same); a serial FT232R board would need the right value, which no source gives.
    pub const BAUD: u32 = 115_200;

    pub struct SerialStream {
        port: Box<dyn serialport::SerialPort>,
        name: String,
    }

    pub type SerialTransport = StreamTransport<SerialStream>;

    impl StreamTransport<SerialStream> {
        /// Open the serial port `path`. Only the app calls this, after the user clicks Connect.
        pub fn open(path: &str) -> Result<Self> {
            let mut port = serialport::new(path, BAUD)
                .data_bits(serialport::DataBits::Eight)
                .parity(serialport::Parity::None)
                .stop_bits(serialport::StopBits::One)
                .flow_control(serialport::FlowControl::None)
                .timeout(Duration::from_millis(10))
                .open()
                .map_err(|e| RuidaError::Transport(format!("could not open {path}: {e}")))?;
            // DTR and RTS on, as the public USB notes describe. Failure is not fatal.
            let _ = port.write_data_terminal_ready(true);
            let _ = port.write_request_to_send(true);
            Ok(StreamTransport::new(SerialStream { port, name: path.to_owned() }))
        }
    }

    impl ByteStream for SerialStream {
        fn write_all(&mut self, data: &[u8]) -> std::io::Result<()> {
            std::io::Write::write_all(&mut self.port, data)?;
            std::io::Write::flush(&mut self.port)
        }

        fn read_some(&mut self, buf: &mut [u8], timeout: Duration) -> std::io::Result<usize> {
            self.port.set_timeout(timeout.max(Duration::from_millis(1))).map_err(std::io::Error::other)?;
            match std::io::Read::read(&mut self.port, buf) {
                Ok(n) => Ok(n),
                Err(e) if matches!(e.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock) => Ok(0),
                Err(e) => Err(e),
            }
        }

        fn name(&self) -> String {
            self.name.clone()
        }
    }
}

#[cfg(feature = "usb")]
pub use usb::UsbTransport;

#[cfg(feature = "usb")]
mod usb {
    //! FTDI FT245R (0403:6001) packet USB. Configures the chip with vendor control requests
    //! (reset, latency, DTR/RTS) and strips the two modem-status bytes FTDI puts at the start
    //! of every 64-byte IN packet. **Never used in tests.**

    use super::*;
    use futures_lite::future::block_on;
    use nusb::transfer::{Control, ControlType, Recipient, RequestBuffer};

    pub const FT245R_PID: u16 = FTDI_PID;
    const EP_OUT: u8 = 0x02;
    const EP_IN: u8 = 0x81;

    pub struct UsbTransport {
        iface: nusb::Interface,
        pending: VecDeque<u8>,
        serial: Option<String>,
    }

    impl UsbTransport {
        /// List connected FT245R devices by serial number.
        pub fn list() -> Result<Vec<Option<String>>> {
            Ok(nusb::list_devices()
                .map_err(|e| RuidaError::Transport(e.to_string()))?
                .filter(|d| d.vendor_id() == FTDI_VID && d.product_id() == FT245R_PID)
                .map(|d| d.serial_number().map(str::to_string))
                .collect())
        }

        /// Open the first FT245R, or the one with this serial number.
        pub fn open(serial: Option<&str>) -> Result<Self> {
            let info = nusb::list_devices()
                .map_err(|e| RuidaError::Transport(e.to_string()))?
                .find(|d| d.vendor_id() == FTDI_VID && d.product_id() == FT245R_PID && serial.is_none_or(|s| d.serial_number() == Some(s)))
                .ok_or_else(|| RuidaError::Transport("no FTDI FT245R controller found".into()))?;
            let serial = info.serial_number().map(str::to_string);
            let dev = info.open().map_err(|e| RuidaError::Transport(e.to_string()))?;
            let iface = dev.claim_interface(0).map_err(|e| RuidaError::Transport(e.to_string()))?;
            let t = UsbTransport { iface, pending: VecDeque::new(), serial };
            // FTDI SIO requests: 0 reset, 1 modem control, 3 baud, 9 latency timer.
            t.ctrl(0, 0)?;
            t.ctrl(3, 0x001A)?; // 115200 baud; reported irrelevant for FT245R.
            t.ctrl(9, 2)?; // 2 ms latency.
            t.ctrl(1, 0x0303)?; // DTR and RTS on.
            Ok(t)
        }

        fn ctrl(&self, request: u8, value: u16) -> Result<()> {
            self.iface
                .control_out_blocking(
                    Control { control_type: ControlType::Vendor, recipient: Recipient::Device, request, value, index: 1 },
                    &[],
                    Duration::from_millis(500),
                )
                .map(|_| ())
                .map_err(|e| RuidaError::Transport(e.to_string()))
        }
    }

    impl Transport for UsbTransport {
        fn framing(&self) -> Framing {
            Framing::Stream
        }

        fn send(&mut self, channel: Channel, data: &[u8]) -> Result<()> {
            if channel == Channel::Jog {
                return Err(RuidaError::Unsupported("jog port is UDP only".into()));
            }
            block_on(self.iface.bulk_out(EP_OUT, data.to_vec())).into_result().map_err(|e| RuidaError::Transport(e.to_string()))?;
            Ok(())
        }

        fn recv(&mut self, channel: Channel, timeout: Duration) -> Result<Option<Vec<u8>>> {
            if channel == Channel::Jog {
                return Err(RuidaError::Unsupported("jog port is UDP only".into()));
            }
            if self.pending.is_empty() {
                let fut = self.iface.bulk_in(EP_IN, RequestBuffer::new(512));
                let deadline = std::time::Instant::now() + timeout;
                let res = block_on(futures_lite::future::or(async { Some(fut.await) }, async {
                    while std::time::Instant::now() < deadline {
                        futures_lite::future::yield_now().await;
                    }
                    None
                }));
                let Some(c) = res else { return Ok(None) };
                let buf = c.into_result().map_err(|e| RuidaError::Transport(e.to_string()))?;
                for packet in buf.chunks(64) {
                    self.pending.extend(packet.iter().skip(2));
                }
            }
            if self.pending.is_empty() { Ok(None) } else { Ok(Some(self.pending.drain(..).collect())) }
        }

        fn describe(&self) -> String {
            format!("USB {}", self.serial.as_deref().unwrap_or("FT245R"))
        }
    }
}
