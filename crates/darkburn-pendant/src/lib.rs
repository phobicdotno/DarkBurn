//! The XHC wireless handheld pendant (the orange "MLASER" pendant, PHB02) with its USB receiver
//! `10ce:eba0`, the one receiver checked on a real device.
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

pub use events::{KeyTracker, PendantEvent, never_chords};
#[cfg(feature = "hid")]
pub use hid::{HidConnector, HidTransport};
pub use poll::{Pendant, PollOptions, Stamped};
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

    /// The receiver ids a text names: four hex digits starting with `eb` after `0x` (the form of
    /// the checked id), or any four hex digits after `10ce:` or `idProduct}=="`.
    fn product_ids_named(text: &str) -> Vec<u16> {
        let lower = text.to_ascii_lowercase();
        let mut ids = Vec::new();
        for (prefix, skip) in [("0xeb", 2), ("10ce:", 5), ("idproduct}==\"", 13)] {
            for (at, _) in lower.match_indices(prefix) {
                let hex = &lower[at + skip..];
                let digits: String = hex.chars().take_while(char::is_ascii_hexdigit).take(5).collect();
                if digits.len() == 4 {
                    ids.push(u16::from_str_radix(&digits, 16).unwrap());
                }
            }
        }
        ids
    }

    /// The public texts about the pendant name only receivers checked on a real device: no
    /// other product ids and no ranges of them.
    #[test]
    fn texts_name_only_the_checked_receiver() {
        let texts = [
            ("docs/research/xhc-pendant.md", include_str!("../../../docs/research/xhc-pendant.md")),
            ("crates/darkburn-pendant/Cargo.toml", include_str!("../Cargo.toml")),
            ("crates/darkburn-pendant/src/lib.rs", include_str!("lib.rs")),
            ("crates/darkburn-pendant/src/protocol.rs", include_str!("protocol.rs")),
            ("crates/darkburn-pendant/src/events.rs", include_str!("events.rs")),
            ("crates/darkburn-pendant/src/poll.rs", include_str!("poll.rs")),
            ("crates/darkburn-pendant/src/transport.rs", include_str!("transport.rs")),
            ("crates/darkburn-pendant/src/hid.rs", include_str!("hid.rs")),
            ("packaging/linux/70-darkburn-pendant.rules", include_str!("../../../packaging/linux/70-darkburn-pendant.rules")),
            ("packaging/linux/install-pendant-rule.sh", include_str!("../../../packaging/linux/install-pendant-rule.sh")),
            ("apps/darkburn/src/pendant_ui.rs", include_str!("../../../apps/darkburn/src/pendant_ui.rs")),
        ];
        for (name, text) in texts {
            for id in product_ids_named(text) {
                assert!(super::PRODUCT_IDS.contains(&id), "{name} names receiver {id:04x}, which is not checked on a real device");
            }
        }
        assert_eq!(product_ids_named("vendor 0x10ce, product 0xeba0, 10ce:eba0"), vec![0xeba0, 0xeba0]);
        // Built at run time, so this file does not name the ids it tests with.
        let range = format!("ids 0x{}..=0x{}", "EB61", "eba1"); // 60257 and 60321
        assert_eq!(product_ids_named(&range), vec![60257, 60321], "a range names both its ends");
    }
}
