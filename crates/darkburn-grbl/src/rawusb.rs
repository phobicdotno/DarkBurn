//! Raw USB: a controller reached through two bulk endpoints, with no serial driver in between.
//!
//! [`RawUsbTransport`] is the [`Transport`]; it talks through a [`BulkPipe`]. Tests use an
//! in-memory pipe. The real pipe ([`UsbPipe`], cargo feature `rawusb`) is only opened when the
//! application asks for a device after the user pressed Connect, and is never used in tests.
//! Listing devices ([`list_usb_devices`]) reads descriptors the operating system already has and
//! opens nothing.

use std::collections::VecDeque;
use std::io;
use std::time::Duration;

use crate::transport::Transport;

/// The two bulk pipes of a raw USB link.
pub trait BulkPipe: Send {
    /// Sends one packet (at most [`BulkPipe::max_packet`] bytes).
    fn write(&mut self, data: &[u8]) -> io::Result<()>;
    /// Waits up to `timeout` for bytes; an empty vector means nothing arrived.
    fn read(&mut self, timeout: Duration) -> io::Result<Vec<u8>>;
    /// Sets the virtual DTR line where the device has one (USB CDC). Others ignore it.
    fn set_dtr(&mut self, _level: bool) -> io::Result<()> {
        Ok(())
    }
    fn max_packet(&self) -> usize {
        64
    }
    fn name(&self) -> String;
}

/// Which device and endpoints to use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawUsbAddress {
    pub vid: u16,
    pub pid: u16,
    /// Only this serial number; `None` takes the first match.
    pub serial: Option<String>,
    pub interface: u8,
    /// Bulk OUT endpoint address, e.g. `0x02`.
    pub ep_out: u8,
    /// Bulk IN endpoint address, e.g. `0x82`.
    pub ep_in: u8,
}

impl RawUsbAddress {
    /// What is wrong with the numbers, in plain words; `None` when they are usable.
    pub fn problem(&self) -> Option<String> {
        if self.vid == 0 || self.pid == 0 {
            return Some("Enter the USB vendor and product ID of the controller.".into());
        }
        if self.ep_out & 0x80 != 0 || self.ep_out & 0x0F == 0 {
            return Some("The OUT endpoint must be a number from 01 to 0F.".into());
        }
        if self.ep_in & 0x80 == 0 || self.ep_in & 0x0F == 0 {
            return Some("The IN endpoint must be a number from 81 to 8F.".into());
        }
        None
    }
    pub fn label(&self) -> String {
        format!("USB {:04X}:{:04X} interface {}", self.vid, self.pid, self.interface)
    }
}

/// A USB device as the operating system lists it. Listing opens nothing.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct UsbDeviceInfo {
    pub vid: u16,
    pub pid: u16,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
    pub serial_number: Option<String>,
}

impl UsbDeviceInfo {
    pub fn label(&self) -> String {
        let name = self.product.clone().or_else(|| self.manufacturer.clone()).unwrap_or_else(|| "USB device".into());
        format!("{name} ({:04X}:{:04X})", self.vid, self.pid)
    }
}

/// Lists USB devices without opening any. Empty when the `rawusb` feature is off.
pub fn list_usb_devices() -> Vec<UsbDeviceInfo> {
    #[cfg(feature = "rawusb")]
    {
        real::list()
    }
    #[cfg(not(feature = "rawusb"))]
    {
        Vec::new()
    }
}

/// G-code over a [`BulkPipe`].
pub struct RawUsbTransport<P: BulkPipe> {
    pipe: P,
    pending: VecDeque<u8>,
}

impl<P: BulkPipe> RawUsbTransport<P> {
    pub fn new(pipe: P) -> Self {
        Self { pipe, pending: VecDeque::new() }
    }
}

impl<P: BulkPipe> Transport for RawUsbTransport<P> {
    fn write_all(&mut self, data: &[u8]) -> io::Result<()> {
        for packet in data.chunks(self.pipe.max_packet().max(1)) {
            self.pipe.write(packet)?;
        }
        Ok(())
    }
    fn read(&mut self, buf: &mut [u8], timeout: Duration) -> io::Result<usize> {
        if self.pending.is_empty() {
            self.pending.extend(self.pipe.read(timeout)?);
        }
        let n = buf.len().min(self.pending.len());
        for (slot, b) in buf.iter_mut().zip(self.pending.drain(..n)) {
            *slot = b;
        }
        Ok(n)
    }
    fn set_dtr(&mut self, level: bool) -> io::Result<()> {
        self.pipe.set_dtr(level)
    }
    fn name(&self) -> String {
        self.pipe.name()
    }
}

#[cfg(feature = "rawusb")]
pub use real::UsbPipe;

#[cfg(feature = "rawusb")]
mod real {
    use std::future::Future;
    use std::pin::pin;
    use std::sync::Arc;
    use std::task::{Context, Poll, Wake, Waker};
    use std::thread::Thread;
    use std::time::Instant;

    use nusb::transfer::{Control, ControlType, Recipient, RequestBuffer};

    use super::*;

    pub(super) fn list() -> Vec<UsbDeviceInfo> {
        let Ok(devices) = nusb::list_devices() else {
            return Vec::new();
        };
        devices
            .map(|d| UsbDeviceInfo {
                vid: d.vendor_id(),
                pid: d.product_id(),
                manufacturer: d.manufacturer_string().map(str::to_owned),
                product: d.product_string().map(str::to_owned),
                serial_number: d.serial_number().map(str::to_owned),
            })
            .collect()
    }

    struct Unpark(Thread);

    impl Wake for Unpark {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }

    /// Runs `fut` on this thread for at most `timeout`. `None` when the time ran out.
    fn block_on_timeout<F: Future>(fut: F, timeout: Duration) -> Option<F::Output> {
        let waker = Waker::from(Arc::new(Unpark(std::thread::current())));
        let mut cx = Context::from_waker(&waker);
        let mut fut = pin!(fut);
        let deadline = Instant::now() + timeout;
        loop {
            if let Poll::Ready(v) = fut.as_mut().poll(&mut cx) {
                return Some(v);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return None;
            }
            std::thread::park_timeout(left);
        }
    }

    const IN_TRANSFERS: usize = 4;
    const IN_SIZE: usize = 4096;
    const WRITE_TIMEOUT: Duration = Duration::from_secs(3);

    /// A claimed USB interface with one bulk OUT and one bulk IN endpoint.
    pub struct UsbPipe {
        iface: nusb::Interface,
        reads: nusb::transfer::Queue<RequestBuffer>,
        addr: RawUsbAddress,
    }

    impl UsbPipe {
        /// Opens a real USB device. Never called by tests; only the application calls this after
        /// the user pressed Connect.
        pub fn open(addr: &RawUsbAddress) -> io::Result<Self> {
            if let Some(p) = addr.problem() {
                return Err(io::Error::new(io::ErrorKind::InvalidInput, p));
            }
            let info = nusb::list_devices()
                .map_err(io::Error::other)?
                .find(|d| {
                    d.vendor_id() == addr.vid && d.product_id() == addr.pid && addr.serial.as_deref().is_none_or(|s| d.serial_number() == Some(s))
                })
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("no USB device {:04X}:{:04X} found", addr.vid, addr.pid)))?;
            let dev = info.open().map_err(io::Error::other)?;
            let iface = dev.claim_interface(addr.interface).map_err(io::Error::other)?;
            let reads = iface.bulk_in_queue(addr.ep_in);
            Ok(Self { iface, reads, addr: addr.clone() })
        }
    }

    impl BulkPipe for UsbPipe {
        fn write(&mut self, data: &[u8]) -> io::Result<()> {
            let done = block_on_timeout(self.iface.bulk_out(self.addr.ep_out, data.to_vec()), WRITE_TIMEOUT)
                .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "the USB device did not take the data"))?;
            done.into_result().map(|_| ()).map_err(io::Error::other)
        }
        fn read(&mut self, timeout: Duration) -> io::Result<Vec<u8>> {
            while self.reads.pending() < IN_TRANSFERS {
                self.reads.submit(RequestBuffer::new(IN_SIZE));
            }
            // Transfers stay queued when the wait times out, so no bytes are lost between calls.
            let Some(done) = block_on_timeout(self.reads.next_complete(), timeout) else {
                return Ok(Vec::new());
            };
            done.into_result().map_err(io::Error::other)
        }
        fn set_dtr(&mut self, level: bool) -> io::Result<()> {
            // USB CDC SET_CONTROL_LINE_STATE: bit 0 is DTR, bit 1 is RTS.
            let control = Control {
                control_type: ControlType::Class,
                recipient: Recipient::Interface,
                request: 0x22,
                value: if level { 0x03 } else { 0x00 },
                index: u16::from(self.addr.interface),
            };
            // Devices without a CDC control interface refuse this; that is not a link failure.
            let _ = self.iface.control_out_blocking(control, &[], Duration::from_millis(500));
            Ok(())
        }
        fn name(&self) -> String {
            self.addr.label()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emulator::{EmulatorConfig, GrblEmulator};
    use crate::transport::EmulatorHandle;

    /// An in-memory pipe into the GRBL emulator that counts packets.
    struct MemoryPipe {
        inner: crate::transport::MemoryTransport,
        packets: Vec<usize>,
        dtr: Vec<bool>,
    }

    impl BulkPipe for MemoryPipe {
        fn write(&mut self, data: &[u8]) -> io::Result<()> {
            self.packets.push(data.len());
            self.inner.write_all(data)
        }
        fn read(&mut self, timeout: Duration) -> io::Result<Vec<u8>> {
            let mut buf = [0u8; 64];
            let n = self.inner.read(&mut buf, timeout)?;
            Ok(buf[..n].to_vec())
        }
        fn set_dtr(&mut self, level: bool) -> io::Result<()> {
            self.dtr.push(level);
            Ok(())
        }
        fn name(&self) -> String {
            "memory pipe".into()
        }
    }

    #[test]
    fn writes_are_split_into_packets_and_reads_are_buffered() {
        let emu = EmulatorHandle::new(GrblEmulator::new(EmulatorConfig::atomstack().with_time_scale(100.0)));
        let mut t = RawUsbTransport::new(MemoryPipe { inner: emu.transport(), packets: Vec::new(), dtr: Vec::new() });
        assert_eq!(t.name(), "memory pipe");
        // Wait for the banner, reading two bytes at a time.
        let mut got = Vec::new();
        let end = std::time::Instant::now() + Duration::from_secs(5);
        while !String::from_utf8_lossy(&got).contains("for help]") && std::time::Instant::now() < end {
            let mut two = [0u8; 2];
            let n = t.read(&mut two, Duration::from_millis(20)).unwrap();
            got.extend(&two[..n]);
        }
        assert!(String::from_utf8_lossy(&got).contains("Grbl 1.1h"), "{}", String::from_utf8_lossy(&got));
        let long = format!("G1 X1 F100 {}\n", "(pad)".repeat(20));
        assert!(long.len() > 64 && long.len() < 128);
        t.write_all(long.as_bytes()).unwrap();
        assert_eq!(t.pipe.packets, vec![64, long.len() - 64]);
        t.set_dtr(false).unwrap();
        assert_eq!(t.pipe.dtr, vec![false]);
    }

    #[test]
    fn addresses_are_checked_before_anything_is_opened() {
        let ok = RawUsbAddress { vid: 0x1A86, pid: 0x7523, serial: None, interface: 0, ep_out: 0x02, ep_in: 0x82 };
        assert_eq!(ok.problem(), None);
        assert_eq!(ok.label(), "USB 1A86:7523 interface 0");
        assert!(RawUsbAddress { vid: 0, ..ok.clone() }.problem().is_some());
        assert!(RawUsbAddress { ep_out: 0x82, ..ok.clone() }.problem().is_some());
        assert!(RawUsbAddress { ep_in: 0x02, ..ok.clone() }.problem().is_some());
        assert_eq!(
            UsbDeviceInfo { vid: 0x1A86, pid: 0x7523, product: Some("USB Serial".into()), ..Default::default() }.label(),
            "USB Serial (1A86:7523)"
        );
    }
}
