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
            // Capability probing happens at open time; the backend is rebuilt
            // from the cache on demand.
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
