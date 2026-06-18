//! Resolves a durable `DeviceId` to a live device and caches the present set.
//! Replaces the Phase-1 placeholder `DeviceCache`.

use super::enumerate::{DeviceDescriptor, DeviceEnumerator};
use crate::ids::DeviceId;
use std::collections::HashMap;

/// Caches the current enumeration, indexed by `DeviceId`.
pub struct DeviceCache {
    present: HashMap<DeviceId, DeviceDescriptor>,
}

impl DeviceCache {
    pub fn new() -> Self {
        DeviceCache { present: HashMap::new() }
    }

    /// Re-enumerate and replace the cached set. Returns the new descriptors.
    pub fn refresh(&mut self, enumerator: &dyn DeviceEnumerator) -> Vec<DeviceDescriptor> {
        let devices = enumerator.enumerate();
        self.present = devices.iter().cloned().map(|d| (d.id.clone(), d)).collect();
        devices
    }

    /// Resolve a stored identity to a present device, or `None` if absent
    /// (unplugged). Callers key config on the `DeviceId`; this is the only
    /// place a `DeviceId` becomes a live device.
    pub fn resolve(&self, id: &DeviceId) -> Option<&DeviceDescriptor> {
        self.present.get(id)
    }

    pub fn is_present(&self, id: &DeviceId) -> bool {
        self.present.contains_key(id)
    }

    pub fn list(&self) -> Vec<DeviceDescriptor> {
        let mut v: Vec<_> = self.present.values().cloned().collect();
        v.sort_by(|a, b| a.id.cmp(&b.id));
        v
    }
}

impl Default for DeviceCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::enumerate::FakeEnumerator;

    fn desc(tag: &str) -> DeviceDescriptor {
        DeviceDescriptor {
            id: DeviceId::AlsaCard { card_name: tag.into() },
            label: tag.into(),
            has_capture: true,
            has_playback: true,
        }
    }

    #[test]
    fn refresh_then_resolve() {
        let en = FakeEnumerator::new(vec![desc("Device"), desc("PCH")]);
        let mut cache = DeviceCache::new();
        cache.refresh(&en);
        assert!(cache.is_present(&DeviceId::AlsaCard { card_name: "Device".into() }));
        assert_eq!(cache.list().len(), 2);
    }

    #[test]
    fn unplugged_device_no_longer_resolves() {
        let en = FakeEnumerator::new(vec![desc("Device")]);
        let mut cache = DeviceCache::new();
        cache.refresh(&en);
        let id = DeviceId::AlsaCard { card_name: "Device".into() };
        assert!(cache.resolve(&id).is_some());

        en.set(vec![]); // device pulled
        cache.refresh(&en);
        assert!(cache.resolve(&id).is_none());
    }
}
