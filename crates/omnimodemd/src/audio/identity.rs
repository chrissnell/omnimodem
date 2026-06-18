//! Derive the most durable `DeviceId` for an audio device. Preference order
//! mirrors `DeviceId`'s `Ord`: `Usb { vid, pid, serial }` > `AlsaCard` >
//! `Topology` > `Placeholder`. USB info comes from a `nusb` scan; the ranking
//! itself is pure and unit-tested. Improvement over Graywolf, which reads the
//! USB serial for display only and keys identity on the (volatile) ALSA index.

use crate::ids::DeviceId;

/// A USB device seen by `nusb`, reduced to the fields we key identity on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsbInfo {
    pub vid: u16,
    pub pid: u16,
    pub serial: Option<String>,
    pub bus: u8,
    pub ports: String,
}

/// Given the ALSA card token (Linux) or `None`, plus any matched USB info,
/// return the most durable identity. A USB serial is the most replug-stable
/// handle; without one, USB topology beats a volatile ALSA index alias.
pub fn best_identity(alsa_card: Option<&str>, usb: Option<&UsbInfo>) -> DeviceId {
    if let Some(u) = usb {
        if let Some(serial) = u.serial.as_deref().filter(|s| !s.is_empty()) {
            return DeviceId::Usb { vid: u.vid, pid: u.pid, serial: serial.to_string() };
        }
        return DeviceId::Topology { bus: u.bus, ports: u.ports.clone() };
    }
    if let Some(card) = alsa_card {
        return DeviceId::AlsaCard { card_name: card.to_string() };
    }
    DeviceId::placeholder()
}

/// Snapshot the USB devices present, via `nusb`. Empty on platforms/permissions
/// where it can't enumerate (caller then falls back to ALSA/name identity).
/// Compiled off Android (nusb is gated out there). Reserved for the macOS /
/// Windows device matchers; Linux resolves USB identity from sysfs directly.
#[cfg(all(not(test), not(target_os = "android")))]
#[allow(dead_code)]
pub fn nusb_scan() -> Vec<UsbInfo> {
    let Ok(devs) = nusb::list_devices() else {
        return Vec::new();
    };
    devs.map(|d| UsbInfo {
        vid: d.vendor_id(),
        pid: d.product_id(),
        serial: d.serial_number().map(|s| s.to_string()),
        bus: d.bus_number(),
        // NOTE: `device_address` is the bus enumeration address, which is NOT
        // stable across replug. It is only a best-effort disambiguator for a
        // serial-less device; the durable identity is `Usb { serial }` above.
        // nusb 0.1 exposes no cross-platform physical port-chain. When the
        // macOS/Windows matchers go live, prefer a stabler handle there.
        ports: d.device_address().to_string(),
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usb(serial: Option<&str>) -> UsbInfo {
        UsbInfo {
            vid: 0x0d8c,
            pid: 0x013c,
            serial: serial.map(String::from),
            bus: 1,
            ports: "1.2".into(),
        }
    }

    #[test]
    fn usb_with_serial_wins() {
        assert_eq!(
            best_identity(Some("Device"), Some(&usb(Some("A1B2")))),
            DeviceId::Usb { vid: 0x0d8c, pid: 0x013c, serial: "A1B2".into() }
        );
    }

    #[test]
    fn usb_without_serial_falls_to_topology() {
        assert_eq!(
            best_identity(Some("Device"), Some(&usb(None))),
            DeviceId::Topology { bus: 1, ports: "1.2".into() }
        );
    }

    #[test]
    fn usb_with_empty_serial_falls_to_topology() {
        assert_eq!(
            best_identity(Some("Device"), Some(&usb(Some("")))),
            DeviceId::Topology { bus: 1, ports: "1.2".into() }
        );
    }

    #[test]
    fn no_usb_uses_alsa_card() {
        assert_eq!(
            best_identity(Some("Device"), None),
            DeviceId::AlsaCard { card_name: "Device".into() }
        );
    }

    #[test]
    fn nothing_is_placeholder() {
        assert_eq!(best_identity(None, None), DeviceId::placeholder());
    }
}
