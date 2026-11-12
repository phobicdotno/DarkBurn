//! The XHC wireless handheld pendant sold with Mlaser machines (orange "MLASER" pendant, model
//! PHB02, receiver USB `10ce:eba0`).
//!
//! * [`protocol`]: the key report and the LED report, decoded and encoded without any I/O.
//! * [`events`]: held-key sets turned into presses, releases and Fn chords.
//! * [`transport`]: the [`PendantTransport`] and [`Connector`] traits and the in-memory
//!   emulator every test uses.
//! * [`poll`]: [`Pendant`], the thread that reads keys, sends LED changes and reconnects.
//! * `hid` (cargo feature `hid`): the real receiver through `hidapi`.
//!
//! The pendant only reports keys and shows LEDs. What a key does is up to the app, which sends
//! it through the same commands and checks as its own buttons.

pub mod events;
#[cfg(feature = "hid")]
pub mod hid;
pub mod poll;
pub mod protocol;
pub mod transport;

pub use events::{KeyTracker, PendantEvent};
#[cfg(feature = "hid")]
pub use hid::{HidConnector, HidTransport};
pub use poll::{Pendant, PollOptions};
pub use protocol::{Key, KeyReport, KeySet, Leds, PRODUCT_IDS, VENDOR_ID, decode_input, encode_leds};
pub use transport::{Connector, EmulatorConnector, EmulatorHandle, EmulatorTransport, NoConnector, PendantError, PendantTransport};

/// The connector for the real receiver in this build: [`HidConnector`] with the `hid` feature,
/// [`NoConnector`] (always [`PendantError::Unsupported`]) without it.
pub fn system_connector() -> Box<dyn Connector> {
    #[cfg(feature = "hid")]
    {
        Box::new(HidConnector::new())
    }
    #[cfg(not(feature = "hid"))]
    {
        Box::new(NoConnector)
    }
}

/// Whether this build can open the real receiver.
pub const HID_SUPPORTED: bool = cfg!(feature = "hid");

/// The command that installs the Linux udev rule shipped as
/// `packaging/linux/70-darkburn-pendant.rules`, for showing to the user.
pub const LINUX_UDEV_RULE: &str = r#"SUBSYSTEM=="hidraw", ATTRS{idVendor}=="10ce", ATTRS{idProduct}=="eba0", MODE="0660", TAG+="uaccess""#;

/// A shell command that installs [`LINUX_UDEV_RULE`] and reloads udev. Shown to the user, never
/// run by DarkBurn.
pub fn linux_install_command() -> String {
    format!(
        "echo '{LINUX_UDEV_RULE}' | sudo tee /etc/udev/rules.d/70-darkburn-pendant.rules && sudo udevadm control --reload && sudo udevadm trigger"
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn udev_rule_matches_the_shipped_file() {
        let file = include_str!("../../../packaging/linux/70-darkburn-pendant.rules");
        assert!(file.lines().any(|l| l.trim() == super::LINUX_UDEV_RULE), "packaging/linux/70-darkburn-pendant.rules and LINUX_UDEV_RULE differ");
        assert!(super::linux_install_command().contains("udevadm control --reload && sudo udevadm trigger"));
    }
}
