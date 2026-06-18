//! Placeholder device cache.
//!
//! Phase 2 replaces this with real enumeration, stable `DeviceId` resolution,
//! and hotplug handling. Phase 1 only needs to vend the single placeholder
//! identity that channel config is keyed on.

use crate::ids::DeviceId;

/// Caches resolved devices. In Phase 1 it knows exactly one placeholder device.
#[derive(Debug, Default)]
pub struct DeviceCache;

impl DeviceCache {
    pub fn new() -> Self {
        DeviceCache
    }

    /// The device a newly-configured channel binds to in Phase 1.
    pub fn default_device(&self) -> DeviceId {
        DeviceId::placeholder()
    }
}
