//! The real receiver through `hidapi` (cargo feature `hid`). Only the app creates a
//! [`HidConnector`], and only after the user turns the pendant on. Tests never open it.

use std::ffi::CString;
use std::time::Duration;

use hidapi::{HidApi, HidDevice, HidError};

use crate::protocol::{PRODUCT_IDS, USAGE_PAGE, VENDOR_ID};
use crate::transport::{Connector, PendantError, PendantTransport};

/// Finds the first receiver in [`PRODUCT_IDS`] and opens it.
pub struct HidConnector {
    api: Option<HidApi>,
}

impl HidConnector {
    pub fn new() -> Self {
        HidConnector { api: None }
    }
}

impl Default for HidConnector {
    fn default() -> Self {
        Self::new()
    }
}

fn io(e: HidError) -> PendantError {
    PendantError::Io(e.to_string())
}

struct Found {
    path: CString,
    pid: u16,
}

impl HidConnector {
    fn find(&mut self) -> Result<Found, PendantError> {
        let api = match self.api.as_mut() {
            Some(api) => {
                api.reset_devices().map_err(io)?;
                for pid in PRODUCT_IDS {
                    api.add_devices(VENDOR_ID, *pid).map_err(io)?;
                }
                api
            }
            None => self.api.insert(HidApi::new().map_err(io)?),
        };
        let mut found: Vec<_> = api.device_list().filter(|d| d.vendor_id() == VENDOR_ID && PRODUCT_IDS.contains(&d.product_id())).collect();
        // A receiver may show more than one HID interface: the vendor page is the one with keys.
        found.sort_by_key(|d| d.usage_page() != USAGE_PAGE);
        let d = found.first().ok_or(PendantError::NotFound)?;
        Ok(Found { path: d.path().to_owned(), pid: d.product_id() })
    }
}

impl Connector for HidConnector {
    fn connect(&mut self) -> Result<Box<dyn PendantTransport>, PendantError> {
        let found = self.find()?;
        let path = found.path.to_string_lossy().into_owned();
        #[cfg(target_os = "linux")]
        check_access(&path)?;
        let api = self.api.as_ref().ok_or(PendantError::NotFound)?;
        let dev = api.open_path(&found.path).map_err(io)?;
        Ok(Box::new(HidTransport { dev, label: format!("XHC pendant {VENDOR_ID:04x}:{:04x} ({path})", found.pid) }))
    }
}

/// On Linux the receiver is a `/dev/hidraw*` node that only root may open until the udev rule
/// is installed. Opening it here first tells that case apart from other failures.
#[cfg(target_os = "linux")]
fn check_access(path: &str) -> Result<(), PendantError> {
    match std::fs::OpenOptions::new().read(true).write(true).open(path) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => Err(PendantError::PermissionDenied { path: path.to_owned() }),
        Err(e) => Err(PendantError::Io(format!("{path}: {e}"))),
    }
}

/// An open receiver.
pub struct HidTransport {
    dev: HidDevice,
    label: String,
}

impl PendantTransport for HidTransport {
    fn read_report(&mut self, buf: &mut [u8], timeout: Duration) -> Result<usize, PendantError> {
        let ms = i32::try_from(timeout.as_millis()).unwrap_or(i32::MAX);
        self.dev.read_timeout(buf, ms).map_err(io)
    }

    fn send_feature_report(&mut self, data: &[u8]) -> Result<(), PendantError> {
        self.dev.send_feature_report(data).map_err(io)
    }

    fn name(&self) -> String {
        self.label.clone()
    }
}
