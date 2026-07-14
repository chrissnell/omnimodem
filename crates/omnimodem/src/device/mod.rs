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
        // cpal talks to the platform audio HAL (CoreAudio on macOS), which can
        // panic — e.g. under the macOS App Sandbox when the host lacks the
        // audio-input entitlement. Isolate it so a panic yields no audio devices
        // rather than taking down the caller (and, in the daemon, the live feed);
        // USB SDR enumeration below still runs.
        let audio = std::panic::catch_unwind(|| {
            crate::audio::cpal_backend::enumerate_default_host()
        })
        .unwrap_or_else(|_| {
            tracing::error!("audio device enumeration panicked; reporting no audio devices");
            Vec::new()
        });
        for (id, backend) in audio {
            // Report the real direction(s) cpal advertises, so the TX picker only
            // offers true output devices (a mic is capture-only, a speaker is
            // playback-only) and a channel isn't silently bound RX-only.
            out.push(DeviceDescriptor {
                id: id.clone(),
                label: id.to_canonical_string(),
                has_capture: backend.has_capture(),
                has_playback: backend.has_playback(),
                needs_setup: false,
            });
        }
        // Append locally-attached RTL-SDR dongles discovered over USB.
        out.extend(enumerate::scan_rtl(&enumerate::NusbLister));
        out
    }
}
