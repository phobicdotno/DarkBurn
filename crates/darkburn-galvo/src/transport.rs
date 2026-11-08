//! Byte transports between the driver and a board.
//!
//! The driver only ever talks through [`Transport`]. Tests and demos use [`MemoryTransport`],
//! which is wired into the in-memory [`GalvoEmulator`]. [`UsbTransport`] (cargo feature `usb`)
//! reaches a real BJJCZ board and is only opened when the application explicitly asks for it.
//! Nothing in this crate lists or opens USB devices unless the `usb` feature is on and
//! [`UsbTransport::open`] is called.

use std::collections::VecDeque;
use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::codec::{COMMAND_LEN, PACKET_LEN, REPLY_LEN};
use crate::emulator::GalvoEmulator;

/// USB vendor id of BJJCZ boards (9588 hex).
pub const USB_VID: u16 = 0x9588;
/// Product ids: the running board and its bootloader ("Loader").
pub const USB_PID: u16 = 0x9899;
pub const USB_PID_LOADER: u16 = 0x9980;
/// Bulk OUT endpoint.
pub const EP_OUT: u8 = 0x02;
/// Bulk IN endpoint.
pub const EP_IN: u8 = 0x88;

/// A link to a board: 12-byte or 3072-byte writes, 8-byte replies.
pub trait Transport: Send {
    /// Writes exactly one command (12 bytes) or one list packet (3072 bytes).
    fn write(&mut self, data: &[u8]) -> io::Result<()>;
    /// Reads one 8-byte reply, waiting at most `timeout`. `Ok(None)` on timeout; `Err` means
    /// the link is gone.
    fn read(&mut self, timeout: Duration) -> io::Result<Option<[u8; REPLY_LEN]>>;
    /// A human name for logs, e.g. `emulator` or `USB 9588:9899`.
    fn name(&self) -> String;
}

pub(crate) fn check_write_len(data: &[u8]) -> io::Result<()> {
    if data.len() == COMMAND_LEN || data.len() == PACKET_LEN {
        Ok(())
    } else {
        Err(io::Error::new(io::ErrorKind::InvalidInput, format!("a write must be {COMMAND_LEN} or {PACKET_LEN} bytes, not {}", data.len())))
    }
}

/// Shared handle to an emulator so tests can inspect and drive it while a driver owns the
/// transport.
pub type EmulatorHandle = Arc<Mutex<GalvoEmulator>>;

/// In-memory link into a [`GalvoEmulator`].
pub struct MemoryTransport {
    emu: EmulatorHandle,
    replies: VecDeque<[u8; REPLY_LEN]>,
}

impl MemoryTransport {
    /// Wraps an emulator and returns the transport plus a handle for inspection.
    pub fn new(emu: GalvoEmulator) -> (Self, EmulatorHandle) {
        let handle = Arc::new(Mutex::new(emu));
        (Self { emu: handle.clone(), replies: VecDeque::new() }, handle)
    }

    pub fn from_handle(handle: EmulatorHandle) -> Self {
        Self { emu: handle, replies: VecDeque::new() }
    }
}

impl Transport for MemoryTransport {
    fn write(&mut self, data: &[u8]) -> io::Result<()> {
        check_write_len(data)?;
        let mut emu = self.emu.lock().map_err(|_| io::Error::other("emulator poisoned"))?;
        if emu.is_disconnected() {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "emulator disconnected"));
        }
        if let Some(reply) = emu.write(data) {
            self.replies.push_back(reply);
        }
        Ok(())
    }

    fn read(&mut self, _timeout: Duration) -> io::Result<Option<[u8; REPLY_LEN]>> {
        if self.emu.lock().map_err(|_| io::Error::other("emulator poisoned"))?.is_disconnected() {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "emulator disconnected"));
        }
        Ok(self.replies.pop_front())
    }

    fn name(&self) -> String {
        "emulator".to_string()
    }
}

#[cfg(feature = "usb")]
pub use usb::UsbTransport;

#[cfg(feature = "usb")]
mod usb {
    //! Real USB: interface 0, bulk OUT 0x02, bulk IN 0x88, about 100 ms timeouts.
    //! **Never used in tests.**

    use super::*;
    use futures_lite::future::{block_on, or, yield_now};
    use nusb::transfer::RequestBuffer;

    pub struct UsbTransport {
        iface: nusb::Interface,
        label: String,
    }

    impl UsbTransport {
        /// Lists attached boards as `(vid, pid, serial)` without opening them.
        pub fn list() -> io::Result<Vec<(u16, u16, Option<String>)>> {
            Ok(nusb::list_devices()
                .map_err(io::Error::other)?
                .filter(|d| d.vendor_id() == USB_VID && (d.product_id() == USB_PID || d.product_id() == USB_PID_LOADER))
                .map(|d| (d.vendor_id(), d.product_id(), d.serial_number().map(str::to_string)))
                .collect())
        }

        /// Opens the `index`th board (0 = first). Only the application calls this, after the
        /// user chose the device.
        pub fn open(index: usize) -> io::Result<Self> {
            let info = nusb::list_devices()
                .map_err(io::Error::other)?
                .filter(|d| d.vendor_id() == USB_VID && d.product_id() == USB_PID)
                .nth(index)
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no BJJCZ galvo board found (is the WinUSB driver installed?)"))?;
            let dev = info.open().map_err(io::Error::other)?;
            let iface = dev.claim_interface(0).map_err(io::Error::other)?;
            Ok(Self { iface, label: format!("USB {:04X}:{:04X}", info.vendor_id(), info.product_id()) })
        }
    }

    impl Transport for UsbTransport {
        fn write(&mut self, data: &[u8]) -> io::Result<()> {
            check_write_len(data)?;
            let fut = self.iface.bulk_out(EP_OUT, data.to_vec());
            let deadline = std::time::Instant::now() + Duration::from_millis(1000);
            let res = block_on(or(async { Some(fut.await) }, async {
                while std::time::Instant::now() < deadline {
                    yield_now().await;
                }
                None
            }));
            match res {
                Some(c) => c.into_result().map(|_| ()).map_err(io::Error::other),
                None => Err(io::Error::new(io::ErrorKind::TimedOut, "USB write timed out")),
            }
        }

        fn read(&mut self, timeout: Duration) -> io::Result<Option<[u8; REPLY_LEN]>> {
            let fut = self.iface.bulk_in(EP_IN, RequestBuffer::new(REPLY_LEN));
            let deadline = std::time::Instant::now() + timeout;
            let res = block_on(or(async { Some(fut.await) }, async {
                while std::time::Instant::now() < deadline {
                    yield_now().await;
                }
                None
            }));
            let Some(c) = res else { return Ok(None) };
            let buf = c.into_result().map_err(io::Error::other)?;
            let mut out = [0u8; REPLY_LEN];
            if buf.len() != REPLY_LEN {
                return Err(io::Error::new(io::ErrorKind::InvalidData, format!("reply was {} bytes", buf.len())));
            }
            out.copy_from_slice(&buf);
            Ok(Some(out))
        }

        fn name(&self) -> String {
            self.label.clone()
        }
    }
}
