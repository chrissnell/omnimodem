//! Device enumeration behind a trait, so the cache and hotplug logic are
//! testable without hardware. `RealEnumerator` bridges cpal + nusb; tests use
//! `FakeEnumerator`. Local USB RTL-SDR dongles are discovered here too, via a
//! mockable [`UsbLister`] seam so [`scan_rtl`] is unit-testable without a real
//! dongle attached.

use crate::ids::{DeviceId, RtlKey};
use std::collections::HashMap;

/// What enumeration knows about one present device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceDescriptor {
    pub id: DeviceId,
    /// Operator-facing label (cpal device name / USB product string).
    pub label: String,
    pub has_capture: bool,
    pub has_playback: bool,
    /// The device is present but not usable until an OS-level setup step runs
    /// (Linux DVB driver still bound, Windows without a WinUSB driver, …). It is
    /// surfaced so the operator can be told *what* to do rather than the device
    /// silently vanishing. `false` for ordinary cpal / `rtl_tcp` devices.
    pub needs_setup: bool,
}

/// Snapshot the set of currently-present devices.
pub trait DeviceEnumerator: Send {
    fn enumerate(&self) -> Vec<DeviceDescriptor>;
}

/// A fixed-list enumerator whose contents a test can swap between snapshots.
pub struct FakeEnumerator {
    pub devices: std::sync::Mutex<Vec<DeviceDescriptor>>,
}

impl FakeEnumerator {
    pub fn new(devices: Vec<DeviceDescriptor>) -> Self {
        FakeEnumerator { devices: std::sync::Mutex::new(devices) }
    }
    pub fn set(&self, devices: Vec<DeviceDescriptor>) {
        *self.devices.lock().unwrap() = devices;
    }
}

impl DeviceEnumerator for FakeEnumerator {
    fn enumerate(&self) -> Vec<DeviceDescriptor> {
        self.devices.lock().unwrap().clone()
    }
}

/// One USB device as seen by a lister, reduced to the attributes RTL discovery
/// needs. Deliberately owns its strings so a test can build one without any USB
/// stack present.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsbDev {
    pub vid: u16,
    pub pid: u16,
    /// The USB serial string, if the device reports one. Cheap RTL clones ship
    /// blank or duplicated serials, so this is not a safe key on its own.
    pub serial: Option<String>,
    pub bus: u8,
    /// USB port chain within the bus (e.g. "4.2"), stable for "the dongle in
    /// this physical port".
    pub ports: String,
    /// USB product string, used as the operator-facing label.
    pub product: String,
    /// Whether the OS will let us claim the device without a manual setup step.
    /// `false` maps to `needs_setup` on the resulting descriptor.
    pub claimable: bool,
}

/// A mockable enumeration of the USB bus. `NusbLister` is the production impl;
/// tests use a fixed-list fake so RTL discovery is exercised without hardware.
pub trait UsbLister {
    fn list(&self) -> Vec<UsbDev>;
}

/// Known RTL2832U-based dongle `(vendor, product)` pairs. Mirrors the
/// `known_devices` table in librtlsdr so the same hardware the reference driver
/// recognizes is discovered here.
const RTL_USB_IDS: &[(u16, u16)] = &[
    (0x0bda, 0x2832), // Generic RTL2832U
    (0x0bda, 0x2838), // Generic RTL2832U OEM (most common blog-v3 dongles)
    (0x0413, 0x6680), // DigitalNow Quad DVB-T PCI-E card
    (0x0413, 0x6f0f), // Leadtek WinFast DTV Dongle mini D
    (0x0458, 0x707f), // Genius TVGo DVB-T03 USB dongle (Ver. B)
    (0x0ccd, 0x00a9), // Terratec Cinergy T Stick Black (rev 1)
    (0x0ccd, 0x00b3), // Terratec NOXON DAB/DAB+ USB dongle (rev 1)
    (0x0ccd, 0x00b4), // Terratec Deutschlandradio DAB Stick
    (0x0ccd, 0x00b5), // Terratec NOXON DAB Stick - Radio Energy
    (0x0ccd, 0x00b7), // Terratec Media Broadcast DAB Stick
    (0x0ccd, 0x00b8), // Terratec BitStar
    (0x0ccd, 0x00b9), // Terratec Cinergy T Stick RC (Rev.3)
    (0x0ccd, 0x00c0), // Terratec T Stick PLUS
    (0x0ccd, 0x00d3), // Terratec Cinergy T Stick RC (Rev.3)
    (0x0ccd, 0x00d7), // Terratec T Stick PLUS
    (0x0ccd, 0x00e0), // Terratec NOXON DAB/DAB+ USB dongle (rev 2)
    (0x1554, 0x5020), // PixelView PV-DT235U(RN)
    (0x15f4, 0x0131), // Astrometa DVB-T/DVB-T2
    (0x15f4, 0x0133), // HanfTek DAB+FM+DVB-T
    (0x185b, 0x0620), // Compro Videomate U620F
    (0x185b, 0x0650), // Compro Videomate U650F
    (0x185b, 0x0680), // Compro Videomate U680F
    (0x1b80, 0xd393), // GIGABYTE GT-U7300
    (0x1b80, 0xd394), // DIKOM USB-DVBT HD
    (0x1b80, 0xd395), // Peak 102569AGPK
    (0x1b80, 0xd397), // KWorld KW-UB450-T USB DVB-T Pico TV
    (0x1b80, 0xd398), // Zaapa ZT-MINDVBZP
    (0x1b80, 0xd39d), // SVEON STV20 DVB-T USB & FM
    (0x1b80, 0xd3a4), // Twintech UT-40
    (0x1b80, 0xd3a8), // ASUS U3100MINI_PLUS_V2
    (0x1b80, 0xd3af), // SVEON STV27 DVB-T USB & FM
    (0x1b80, 0xd3b0), // SVEON STV21 DVB-T USB & FM
    (0x1d19, 0x1101), // Dexatek DK DVB-T Dongle (Logilink VG0002A)
    (0x1d19, 0x1102), // Dexatek DK DVB-T Dongle (MSI DigiVox mini II V3.0)
    (0x1d19, 0x1103), // Dexatek Technology Ltd. DK 5217 DVB-T Dongle
    (0x1d19, 0x1104), // MSI DigiVox Micro HD
    (0x1f4d, 0xa803), // Sweex DVB-T USB
    (0x1f4d, 0xb803), // GTek T803
    (0x1f4d, 0xc803), // Lifeview LV5TDeluxe
    (0x1f4d, 0xd286), // MyGica TD312
    (0x1f4d, 0xd803), // PROlectrix DV107669
];

fn is_rtl(vid: u16, pid: u16) -> bool {
    RTL_USB_IDS.contains(&(vid, pid))
}

/// Turn the RTL dongles found by `lister` into capture-only descriptors.
///
/// Keying: prefer [`RtlKey::Serial`] when the serial is non-empty and unique
/// among the RTL dongles in *this* scan; otherwise fall back to
/// [`RtlKey::Topo`] (bus + port chain) — the same disambiguation the sound-card
/// path performs for two identical adapters. `needs_setup` mirrors `claimable`.
pub fn scan_rtl(lister: &dyn UsbLister) -> Vec<DeviceDescriptor> {
    let rtls: Vec<UsbDev> =
        lister.list().into_iter().filter(|d| is_rtl(d.vid, d.pid)).collect();

    // A serial only disambiguates if exactly one dongle in the scan reports it.
    let mut serial_counts: HashMap<&str, usize> = HashMap::new();
    for d in &rtls {
        if let Some(s) = non_empty_serial(d) {
            *serial_counts.entry(s).or_default() += 1;
        }
    }

    rtls.iter()
        .map(|d| {
            let key = match non_empty_serial(d) {
                Some(s) if serial_counts[s] == 1 => RtlKey::Serial(s.to_string()),
                _ => RtlKey::Topo { bus: d.bus, ports: d.ports.clone() },
            };
            let label = if d.product.is_empty() {
                "RTL-SDR".to_string()
            } else {
                d.product.clone()
            };
            DeviceDescriptor {
                id: DeviceId::Rtl { key },
                label,
                has_capture: true,
                has_playback: false,
                needs_setup: !d.claimable,
            }
        })
        .collect()
}

/// The serial iff it is present and non-empty.
fn non_empty_serial(d: &UsbDev) -> Option<&str> {
    d.serial.as_deref().filter(|s| !s.is_empty())
}

/// Production `UsbLister` backed by `nusb::list_devices()`.
#[cfg(not(test))]
pub struct NusbLister;

#[cfg(not(test))]
impl UsbLister for NusbLister {
    fn list(&self) -> Vec<UsbDev> {
        match nusb::list_devices() {
            Ok(devs) => devs
                .map(|d| UsbDev {
                    vid: d.vendor_id(),
                    pid: d.product_id(),
                    serial: d.serial_number().map(str::to_string),
                    bus: d.bus_number(),
                    ports: port_chain(&d),
                    product: d.product_string().unwrap_or_default().to_string(),
                    claimable: claimable(&d),
                })
                .collect(),
            Err(e) => {
                tracing::warn!(error = %e, "USB enumeration failed; no local RTL dongles listed");
                Vec::new()
            }
        }
    }
}

/// Best-effort port-chain string within the bus. `nusb` 0.1 exposes no portable
/// port chain, so we read it from the Linux kernel device name (`<bus>-<ports>`)
/// where available and fall back to the device address elsewhere. Only used to
/// build a topology key when the serial is unusable.
#[cfg(all(not(test), any(target_os = "linux", target_os = "android")))]
fn port_chain(d: &nusb::DeviceInfo) -> String {
    // The sysfs leaf is e.g. "1-4.2"; the part after the bus prefix is the chain.
    d.sysfs_path()
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.split_once('-'))
        .map(|(_, ports)| ports.to_string())
        .unwrap_or_else(|| d.device_address().to_string())
}

#[cfg(all(not(test), not(any(target_os = "linux", target_os = "android"))))]
fn port_chain(d: &nusb::DeviceInfo) -> String {
    d.device_address().to_string()
}

/// Whether the OS will let us claim the dongle without operator setup. On
/// Linux/macOS the DVB kernel driver is detachable at claim time (P2-A), so the
/// device is claimable; a definitive answer only comes from the claim attempt.
/// Windows needs a WinUSB-family driver bound — refined in P3-B.
#[cfg(all(not(test), not(target_os = "windows")))]
fn claimable(_d: &nusb::DeviceInfo) -> bool {
    true
}

#[cfg(all(not(test), target_os = "windows"))]
fn claimable(d: &nusb::DeviceInfo) -> bool {
    // A generic (WinUSB/libusb) driver is claimable; a vendor DVB driver or none
    // is not, and surfaces as needs_setup so the operator can run Zadig (P3-B).
    match d.driver() {
        Some(drv) => {
            let drv = drv.to_ascii_lowercase();
            drv.contains("winusb") || drv.contains("libusb") || drv.contains("usbccgp")
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::hotplug::{HotplugEvent, HotplugWatcher};

    struct FakeUsbLister(Vec<UsbDev>);
    impl UsbLister for FakeUsbLister {
        fn list(&self) -> Vec<UsbDev> {
            self.0.clone()
        }
    }

    #[test]
    fn rtl_dongle_becomes_a_capture_only_descriptor() {
        let lister = FakeUsbLister(vec![UsbDev {
            vid: 0x0bda,
            pid: 0x2838,
            serial: Some("00000001".into()),
            bus: 1,
            ports: "4".into(),
            product: "RTL2838UHIDIR".into(),
            claimable: true,
        }]);
        let devs = scan_rtl(&lister);
        assert_eq!(devs.len(), 1);
        assert_eq!(devs[0].id, DeviceId::Rtl { key: RtlKey::Serial("00000001".into()) });
        assert_eq!(devs[0].label, "RTL2838UHIDIR");
        assert!(devs[0].has_capture && !devs[0].has_playback && !devs[0].needs_setup);
    }

    #[test]
    fn duplicate_serials_fall_back_to_topology() {
        let dup = |bus, ports: &str| UsbDev {
            vid: 0x0bda,
            pid: 0x2832,
            serial: Some("00000001".into()),
            bus,
            ports: ports.into(),
            product: "RTL".into(),
            claimable: true,
        };
        let devs = scan_rtl(&FakeUsbLister(vec![dup(1, "1"), dup(1, "2")]));
        assert!(matches!(devs[0].id, DeviceId::Rtl { key: RtlKey::Topo { .. } }));
        assert!(matches!(devs[1].id, DeviceId::Rtl { key: RtlKey::Topo { .. } }));
        // The two topology keys stay distinct so both dongles are addressable.
        assert_ne!(devs[0].id, devs[1].id);
    }

    #[test]
    fn unclaimable_dongle_is_reported_with_needs_setup() {
        let d = UsbDev {
            vid: 0x0bda,
            pid: 0x2838,
            serial: None,
            bus: 1,
            ports: "4".into(),
            product: "RTL".into(),
            claimable: false,
        };
        let devs = scan_rtl(&FakeUsbLister(vec![d]));
        assert!(devs[0].needs_setup && devs[0].has_capture);
        // No serial → topology key.
        assert!(matches!(devs[0].id, DeviceId::Rtl { key: RtlKey::Topo { .. } }));
    }

    #[test]
    fn a_blank_serial_falls_back_to_topology() {
        let d = UsbDev {
            vid: 0x0bda,
            pid: 0x2838,
            serial: Some(String::new()),
            bus: 2,
            ports: "1.3".into(),
            product: "RTL".into(),
            claimable: true,
        };
        let devs = scan_rtl(&FakeUsbLister(vec![d]));
        assert_eq!(
            devs[0].id,
            DeviceId::Rtl { key: RtlKey::Topo { bus: 2, ports: "1.3".into() } }
        );
    }

    #[test]
    fn non_rtl_usb_devices_are_ignored() {
        let keyboard = UsbDev {
            vid: 0x046d,
            pid: 0xc31c,
            serial: None,
            bus: 1,
            ports: "2".into(),
            product: "USB Keyboard".into(),
            claimable: true,
        };
        assert!(scan_rtl(&FakeUsbLister(vec![keyboard])).is_empty());
    }

    /// Hotplug needs no USB-specific code: the descriptor diff spine already
    /// keys on `DeviceId`, so an `rtl:` descriptor arriving/leaving is detected
    /// the same as any other. This proves it end to end.
    #[test]
    fn rtl_hotplug_arrives_and_departs() {
        let rtl = DeviceDescriptor {
            id: DeviceId::Rtl { key: RtlKey::Serial("00000001".into()) },
            label: "RTL2838UHIDIR".into(),
            has_capture: true,
            has_playback: false,
            needs_setup: false,
        };
        let en = FakeEnumerator::new(vec![]);
        let mut w = HotplugWatcher::new();
        assert!(w.poll(&en).is_empty());

        en.set(vec![rtl.clone()]); // dongle plugged in
        assert_eq!(w.poll(&en), vec![HotplugEvent::Arrived(rtl.clone())]);

        en.set(vec![]); // dongle unplugged
        assert_eq!(w.poll(&en), vec![HotplugEvent::Departed(rtl.id)]);
    }
}
