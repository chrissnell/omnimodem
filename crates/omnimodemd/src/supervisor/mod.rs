//! The Supervisor: owns live channels, the device cache, the PTT registry, and
//! the persistence store. Evolution of Graywolf's `struct Modem` state role.

pub mod channel;
pub mod ptt;

use crate::ids::ChannelId;
use crate::persist::Store;
use channel::{ChannelConfig, ChannelState};
use crate::device::DeviceCache;
use ptt::PttRegistry;
use std::collections::BTreeMap;

/// An immutable point-in-time view of modem state, used for snapshot-on-subscribe.
#[derive(Debug, Clone)]
pub struct ModemSnapshot {
    pub channels: Vec<ChannelConfig>,
    pub running: Vec<bool>,
}

/// Owns all live in-memory state plus the persistence store.
pub struct Supervisor {
    channels: BTreeMap<ChannelId, ChannelState>,
    devices: DeviceCache,
    ptt: PttRegistry,
    store: Store,
}

impl Supervisor {
    /// Build a Supervisor, restoring any persisted channels from `store`.
    pub fn new(store: Store) -> Result<Self, crate::persist::StoreError> {
        let mut channels = BTreeMap::new();
        for cfg in store.load_channels()? {
            channels.insert(cfg.id, ChannelState::new(cfg));
        }
        Ok(Supervisor {
            channels,
            devices: DeviceCache::new(),
            ptt: PttRegistry::new(),
            store,
        })
    }

    /// Apply a channel configuration: persist it, then update live state.
    pub fn configure_channel(
        &mut self,
        id: ChannelId,
        name: String,
        mode: String,
    ) -> Result<(), crate::persist::StoreError> {
        let _ = &self.devices; // real binding wired in Task 16
        let cfg = ChannelConfig {
            id,
            name,
            mode,
            device_id: crate::ids::DeviceId::placeholder(),
        };
        self.store.upsert_channel(&cfg)?;
        self.channels
            .entry(id)
            .and_modify(|s| s.config = cfg.clone())
            .or_insert_with(|| ChannelState::new(cfg));
        Ok(())
    }

    pub fn has_channel(&self, id: ChannelId) -> bool {
        self.channels.contains_key(&id)
    }

    /// Mutable access to the PTT registry (for the transmit simulation).
    pub fn ptt_mut(&mut self) -> &mut PttRegistry {
        &mut self.ptt
    }

    /// Produce an immutable snapshot of current state.
    pub fn snapshot(&self) -> ModemSnapshot {
        let mut channels = Vec::with_capacity(self.channels.len());
        let mut running = Vec::with_capacity(self.channels.len());
        for st in self.channels.values() {
            channels.push(st.config.clone());
            running.push(st.running);
        }
        ModemSnapshot { channels, running }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configure_then_snapshot_reflects_channel() {
        let store = Store::open_in_memory().unwrap();
        let mut sup = Supervisor::new(store).unwrap();
        sup.configure_channel(ChannelId(0), "vfo-a".into(), "none".into())
            .unwrap();

        let snap = sup.snapshot();
        assert_eq!(snap.channels.len(), 1);
        assert_eq!(snap.channels[0].name, "vfo-a");
        assert!(snap.running[0]);
        assert!(sup.has_channel(ChannelId(0)));
    }

    #[test]
    fn reconfigure_updates_in_place() {
        let store = Store::open_in_memory().unwrap();
        let mut sup = Supervisor::new(store).unwrap();
        sup.configure_channel(ChannelId(0), "first".into(), "none".into())
            .unwrap();
        sup.configure_channel(ChannelId(0), "second".into(), "none".into())
            .unwrap();

        let snap = sup.snapshot();
        assert_eq!(snap.channels.len(), 1);
        assert_eq!(snap.channels[0].name, "second");
    }

    #[test]
    fn new_supervisor_restores_persisted_channels() {
        let store = Store::open_in_memory().unwrap();
        store
            .upsert_channel(&ChannelConfig {
                id: ChannelId(3),
                name: "restored".into(),
                mode: "none".into(),
                device_id: crate::ids::DeviceId::placeholder(),
            })
            .unwrap();
        let sup = Supervisor::new(store).unwrap();
        assert!(sup.has_channel(ChannelId(3)));
    }
}
