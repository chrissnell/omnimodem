//! Placeholder PTT registry.
//!
//! The real `PortRegistry` (multi-handle, hotplug eviction, per-OS drivers)
//! lands in Phase 2. Phase 1 only records key/unkey intent so the transmit
//! simulation has something to flip, and so the cross-channel shared-state
//! shape exists from the start.

use crate::ids::ChannelId;
use std::collections::HashMap;

/// Tracks simulated PTT state per channel.
#[derive(Debug, Default)]
pub struct PttRegistry {
    keyed: HashMap<ChannelId, bool>,
}

impl PttRegistry {
    pub fn new() -> Self {
        PttRegistry::default()
    }

    /// Simulate keying the transmitter for a channel. Returns the prior state.
    pub fn key(&mut self, channel: ChannelId) -> bool {
        self.keyed.insert(channel, true).unwrap_or(false)
    }

    /// Simulate releasing PTT for a channel.
    pub fn unkey(&mut self, channel: ChannelId) {
        self.keyed.insert(channel, false);
    }

    pub fn is_keyed(&self, channel: ChannelId) -> bool {
        self.keyed.get(&channel).copied().unwrap_or(false)
    }
}
