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

/// How long audio (cpal/CoreAudio) enumeration may run before we give up and
/// return the USB devices alone. Audio enumeration opens every HAL device and,
/// on the first run, triggers a per-input-device TCC (microphone-consent)
/// round-trip on macOS — a cold scan of a machine with several audio devices can
/// take many seconds. `ListDevices` for an SDR scan discards audio entirely, so
/// a slow audio side must never keep the RPC (or hotplug poll) waiting.
#[cfg(not(test))]
const AUDIO_ENUMERATION_BUDGET: std::time::Duration = std::time::Duration::from_secs(2);

#[cfg(not(test))]
impl DeviceEnumerator for RealEnumerator {
    fn enumerate(&self) -> Vec<DeviceDescriptor> {
        // USB SDR enumeration is cheap and never touches the audio HAL, so run it
        // first and unconditionally: a slow or hung audio probe must never keep a
        // freshly-plugged RTL dongle out of a scan.
        let usb = enumerate::scan_rtl(&enumerate::NusbLister);

        // cpal talks to the platform audio HAL (CoreAudio on macOS), which is both
        // slow (see AUDIO_ENUMERATION_BUDGET) and can panic — e.g. under the macOS
        // App Sandbox when the host lacks the audio-input entitlement. Run it on a
        // worker thread bounded by a deadline so neither a panic nor a multi-second
        // HAL stall blocks the caller (and, in the daemon, the ListDevices RPC and
        // the live feed). On overrun we return the SDR devices alone; the worker
        // finishes in the background and its result is dropped when the channel
        // receiver goes away.
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let audio = std::panic::catch_unwind(|| {
                crate::audio::cpal_backend::enumerate_default_host()
            })
            .unwrap_or_else(|_| {
                tracing::error!("audio device enumeration panicked; reporting no audio devices");
                Vec::new()
            });
            let descriptors: Vec<DeviceDescriptor> = audio
                .into_iter()
                // Report the real direction(s) cpal advertises, so the TX picker
                // only offers true output devices (a mic is capture-only, a
                // speaker is playback-only) and a channel isn't silently bound
                // RX-only.
                .map(|(id, backend)| DeviceDescriptor {
                    id: id.clone(),
                    label: id.to_canonical_string(),
                    has_capture: backend.has_capture(),
                    has_playback: backend.has_playback(),
                    needs_setup: false,
                })
                .collect();
            let _ = tx.send(descriptors);
        });

        // Audio devices first (preserving prior ordering) when they arrive in
        // time, then the USB SDRs; USB-only on overrun.
        let mut out = match rx.recv_timeout(AUDIO_ENUMERATION_BUDGET) {
            Ok(audio) => audio,
            Err(_) => {
                tracing::warn!(
                    budget_ms = AUDIO_ENUMERATION_BUDGET.as_millis() as u64,
                    "audio device enumeration exceeded its budget; reporting SDR devices only"
                );
                Vec::new()
            }
        };
        out.extend(usb);
        out
    }
}
