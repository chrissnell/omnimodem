//! Device identity, enumeration, caching, and hotplug — the spine both the
//! audio and PTT subsystems key on.

pub mod cache;
pub mod enumerate;
pub mod hotplug;

pub use cache::DeviceCache;
pub use enumerate::{DeviceDescriptor, DeviceEnumerator};
pub use hotplug::{HotplugEvent, HotplugWatcher};

/// The production enumerator: cpal for audio devices, upgraded with USB
/// identity from nusb where a card maps to a USB device. On a host with no
/// audio this yields an empty list (valid: the virtual backends still work).
#[cfg(not(test))]
pub struct RealEnumerator;

#[cfg(not(test))]
impl DeviceEnumerator for RealEnumerator {
    fn enumerate(&self) -> Vec<DeviceDescriptor> {
        let mut out = Vec::new();
        for (id, _backend) in crate::audio::cpal_backend::enumerate_default_host() {
            // Upgrade an ALSA-card identity to a durable USB identity when the
            // card maps to a USB device (replug-stable). Capability probing
            // happens at open time; the backend is rebuilt from the cache.
            let id = upgrade_identity(id);
            out.push(DeviceDescriptor {
                id: id.clone(),
                label: id.to_canonical_string(),
                has_capture: true,
                has_playback: true,
            });
        }
        out
    }
}

/// Resolve an ALSA-card identity to the most durable identity available. On
/// Linux this reads the card's USB VID/PID/serial from sysfs; elsewhere the id
/// is returned unchanged.
#[cfg(not(test))]
fn upgrade_identity(id: crate::ids::DeviceId) -> crate::ids::DeviceId {
    #[cfg(target_os = "linux")]
    if let crate::ids::DeviceId::AlsaCard { card_name } = &id {
        if let Some(usb) = linux_usb_for_alsa_card(card_name) {
            return crate::audio::identity::best_identity(Some(card_name), Some(&usb));
        }
    }
    id
}

/// Read the USB VID/PID/serial backing an ALSA card via sysfs
/// (`/sys/class/sound/cardN/{id,device/...}`). `None` for non-USB cards.
#[cfg(all(not(test), target_os = "linux"))]
fn linux_usb_for_alsa_card(card_name: &str) -> Option<crate::audio::identity::UsbInfo> {
    use crate::audio::identity::UsbInfo;
    use std::fs;
    for entry in fs::read_dir("/sys/class/sound").ok()?.flatten() {
        let fname = entry.file_name();
        let fname = fname.to_string_lossy();
        if !(fname.starts_with("card") && fname[4..].chars().all(|c| c.is_ascii_digit())) {
            continue;
        }
        let card_path = entry.path();
        if fs::read_to_string(card_path.join("id")).unwrap_or_default().trim() != card_name {
            continue;
        }
        // Walk up from the card's `device` symlink until a dir exposes idVendor.
        let mut cur = fs::canonicalize(card_path.join("device")).ok();
        while let Some(dir) = cur {
            let vid = fs::read_to_string(dir.join("idVendor")).ok();
            let pid = fs::read_to_string(dir.join("idProduct")).ok();
            if let (Some(v), Some(p)) = (vid, pid) {
                let vid = u16::from_str_radix(v.trim(), 16).ok()?;
                let pid = u16::from_str_radix(p.trim(), 16).ok()?;
                let serial =
                    fs::read_to_string(dir.join("serial")).ok().map(|s| s.trim().to_string());
                let bus = fs::read_to_string(dir.join("busnum"))
                    .ok()
                    .and_then(|s| s.trim().parse().ok())
                    .unwrap_or(0);
                let ports = dir
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                return Some(UsbInfo { vid, pid, serial, bus, ports });
            }
            cur = dir.parent().map(|p| p.to_path_buf());
        }
    }
    None
}
