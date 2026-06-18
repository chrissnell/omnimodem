//! Device enumeration behind a trait, so the cache and hotplug logic are
//! testable without hardware. `RealEnumerator` bridges cpal + nusb; tests use
//! `FakeEnumerator`.

use crate::ids::DeviceId;

/// What enumeration knows about one present device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceDescriptor {
    pub id: DeviceId,
    /// Operator-facing label (cpal device name / USB product string).
    pub label: String,
    pub has_capture: bool,
    pub has_playback: bool,
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
