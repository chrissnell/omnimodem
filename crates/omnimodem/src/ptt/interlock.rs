//! RX/TX interlock. While any channel keys a physical device, RX on that device
//! is muted so we don't decode our own transmission. Keyed by DeviceId because
//! two channels sharing one rig must interlock together (design: concurrency is
//! per-rig). A count, not a bool: overlapping keys on the same rig nest.

use crate::ids::DeviceId;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Shared interlock state. Cloneable handle over a shared map.
#[derive(Clone, Default)]
pub struct RxTxInterlock {
    keyed: Arc<Mutex<HashMap<DeviceId, u32>>>,
}

impl RxTxInterlock {
    pub fn new() -> Self {
        RxTxInterlock::default()
    }

    /// Mark a device keyed (begin TX). Nesting-safe.
    pub fn begin_tx(&self, id: &DeviceId) {
        *self.keyed.lock().unwrap().entry(id.clone()).or_insert(0) += 1;
    }

    /// Mark a device unkeyed (end TX). Saturating at zero.
    pub fn end_tx(&self, id: &DeviceId) {
        let mut map = self.keyed.lock().unwrap();
        if let Some(c) = map.get_mut(id) {
            *c = c.saturating_sub(1);
            if *c == 0 {
                map.remove(id);
            }
        }
    }

    /// True while the device is keyed — RX on it must be muted.
    pub fn is_muted(&self, id: &DeviceId) -> bool {
        self.keyed.lock().unwrap().get(id).map(|&c| c > 0).unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(tag: &str) -> DeviceId {
        DeviceId::AlsaCard { card_name: tag.into() }
    }

    #[test]
    fn idle_device_is_not_muted() {
        let il = RxTxInterlock::new();
        assert!(!il.is_muted(&dev("A")));
    }

    #[test]
    fn key_mutes_and_unkey_unmutes() {
        let il = RxTxInterlock::new();
        il.begin_tx(&dev("A"));
        assert!(il.is_muted(&dev("A")));
        il.end_tx(&dev("A"));
        assert!(!il.is_muted(&dev("A")));
    }

    #[test]
    fn only_the_keyed_device_is_muted() {
        let il = RxTxInterlock::new();
        il.begin_tx(&dev("A"));
        assert!(il.is_muted(&dev("A")));
        assert!(!il.is_muted(&dev("B")));
    }

    #[test]
    fn nested_keys_on_one_rig_need_matching_unkeys() {
        let il = RxTxInterlock::new();
        il.begin_tx(&dev("A")); // channel 1 keys
        il.begin_tx(&dev("A")); // channel 2 keys same rig
        il.end_tx(&dev("A")); // channel 1 done
        assert!(il.is_muted(&dev("A")), "still keyed by channel 2");
        il.end_tx(&dev("A")); // channel 2 done
        assert!(!il.is_muted(&dev("A")));
    }

    #[test]
    fn end_without_begin_is_safe() {
        let il = RxTxInterlock::new();
        il.end_tx(&dev("A")); // no panic, no underflow
        assert!(!il.is_muted(&dev("A")));
    }
}
