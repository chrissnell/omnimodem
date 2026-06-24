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
        for (id, backend) in crate::audio::cpal_backend::enumerate_default_host() {
            // Report the real direction(s) cpal advertises, so the TX picker only
            // offers true output devices (a mic is capture-only, a speaker is
            // playback-only) and a channel isn't silently bound RX-only.
            out.push(DeviceDescriptor {
                id: id.clone(),
                label: id.to_canonical_string(),
                has_capture: backend.has_capture(),
                has_playback: backend.has_playback(),
            });
        }
        out
    }
}
