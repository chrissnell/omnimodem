//! The Supervisor: owns live channels, the device cache, the PTT registry, the
//! RX/TX interlock, and the persistence store. Evolution of Graywolf's
//! `struct Modem` state role.

pub mod channel;

use crate::audio::MAX_SAMPLE_RATE;
use crate::device::DeviceCache;
use crate::ids::{ChannelId, DeviceId};
use crate::persist::Store;
use crate::ptt::interlock::RxTxInterlock;
use crate::ptt::registry::{DriverOpener, PortRegistry, PttConfig};
use channel::{ChannelConfig, ChannelState};
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
    ptt: PortRegistry,
    interlock: RxTxInterlock,
    store: Store,
}

impl Supervisor {
    /// Build a Supervisor, restoring any persisted channels from `store`. The
    /// `opener` builds real PTT drivers (production) or test doubles.
    pub fn new(store: Store, opener: Box<dyn DriverOpener>) -> Result<Self, crate::persist::StoreError> {
        let mut channels = BTreeMap::new();
        for mut cfg in store.load_channels()? {
            // Enforce the invariant that a live channel's mode is always
            // resolvable. The configure path validates before persisting, but a
            // hand-edited or downgraded DB could carry a mode this build no
            // longer understands; coerce it to "none" (fail-safe) rather than
            // letting an unresolvable string survive into the runtime.
            if crate::mode::ModeConfig::parse(&cfg.mode).is_none() {
                tracing::warn!(
                    channel = ?cfg.id,
                    mode = %cfg.mode,
                    "persisted channel mode is not resolvable; coercing to \"none\""
                );
                cfg.mode = "none".to_string();
            }
            channels.insert(cfg.id, ChannelState::new(cfg));
        }
        Ok(Supervisor {
            channels,
            devices: DeviceCache::new(),
            ptt: PortRegistry::new(opener),
            interlock: RxTxInterlock::new(),
            store,
        })
    }

    /// Apply a channel's identity and mode, then persist. For an existing
    /// channel this touches only `name`/`mode` and keeps its audio and PTT
    /// bindings intact — reconfiguring (e.g. switching modes) must not silently
    /// drop the operator's RX/TX/PTT device choices. A brand-new channel starts
    /// on the placeholder device with no PTT; `configure_audio`/`configure_ptt`
    /// set the real bindings.
    pub fn configure_channel(
        &mut self,
        id: ChannelId,
        name: String,
        mode: String,
    ) -> Result<(), crate::persist::StoreError> {
        let cfg = match self.channels.get(&id) {
            Some(state) => {
                let mut cfg = state.config.clone();
                cfg.name = name;
                cfg.mode = mode;
                cfg
            }
            None => ChannelConfig {
                id,
                name,
                mode,
                device_id: DeviceId::placeholder(),
                sample_rate: MAX_SAMPLE_RATE,
                fanout: 1,
                tx_device_id: DeviceId::placeholder(),
                tx_sample_rate: 0,
                ptt: None,
            },
        };
        self.store.upsert_channel(&cfg)?;
        self.channels
            .entry(id)
            .and_modify(|s| s.config = cfg.clone())
            .or_insert_with(|| ChannelState::new(cfg));
        Ok(())
    }

    /// Bind a channel's audio to a device and persist. No-op if the channel is
    /// unknown (the core guards existence first).
    pub fn configure_audio(
        &mut self,
        id: ChannelId,
        device_id: DeviceId,
        sample_rate: u32,
        fanout: u32,
        tx_device_id: DeviceId,
        tx_sample_rate: u32,
    ) -> Result<(), crate::persist::StoreError> {
        let Some(state) = self.channels.get_mut(&id) else {
            return Ok(());
        };
        state.config.device_id = device_id;
        state.config.sample_rate = if sample_rate == 0 { MAX_SAMPLE_RATE } else { sample_rate };
        state.config.fanout = fanout;
        state.config.tx_device_id = tx_device_id;
        state.config.tx_sample_rate = tx_sample_rate;
        let cfg = state.config.clone();
        self.store.upsert_channel(&cfg)
    }

    /// Bind a channel's PTT and persist. No-op if the channel is unknown.
    pub fn configure_ptt(
        &mut self,
        id: ChannelId,
        ptt: PttConfig,
    ) -> Result<(), crate::persist::StoreError> {
        let Some(state) = self.channels.get_mut(&id) else {
            return Ok(());
        };
        state.config.ptt = Some(ptt);
        let cfg = state.config.clone();
        self.store.upsert_channel(&cfg)
    }

    pub fn has_channel(&self, id: ChannelId) -> bool {
        self.channels.contains_key(&id)
    }

    /// Resolve a channel's persisted mode string to its parametric config.
    /// Unknown/absent channels resolve to `None` (the inert fixture mode).
    pub fn channel_mode(&self, id: ChannelId) -> crate::mode::ModeConfig {
        self.channels
            .get(&id)
            .and_then(|s| crate::mode::ModeConfig::parse(&s.config.mode))
            .unwrap_or(crate::mode::ModeConfig::None)
    }

    /// A cloneable handle to the per-rig RX/TX interlock.
    pub fn interlock(&self) -> RxTxInterlock {
        self.interlock.clone()
    }

    pub fn device_cache_mut(&mut self) -> &mut DeviceCache {
        &mut self.devices
    }

    pub fn ptt_registry_mut(&mut self) -> &mut PortRegistry {
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
    use crate::ptt::registry::RealOpener;

    fn supervisor() -> Supervisor {
        let store = Store::open_in_memory().unwrap();
        Supervisor::new(store, Box::new(RealOpener)).unwrap()
    }

    #[test]
    fn configure_then_snapshot_reflects_channel() {
        let mut sup = supervisor();
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
        let mut sup = supervisor();
        sup.configure_channel(ChannelId(0), "first".into(), "none".into())
            .unwrap();
        sup.configure_channel(ChannelId(0), "second".into(), "none".into())
            .unwrap();

        let snap = sup.snapshot();
        assert_eq!(snap.channels.len(), 1);
        assert_eq!(snap.channels[0].name, "second");
    }

    #[test]
    fn reconfigure_preserves_audio_and_ptt_bindings() {
        // A mode/name change must not drop the operator's RX/TX/PTT device
        // choices — only identity and mode may change.
        let mut sup = supervisor();
        sup.configure_channel(ChannelId(0), "vfo-a".into(), "none".into())
            .unwrap();
        sup.configure_audio(
            ChannelId(0),
            DeviceId::AlsaCard { card_name: "Mic".into() },
            48_000,
            1,
            DeviceId::AlsaCard { card_name: "Speaker".into() },
            0,
        )
        .unwrap();
        sup.configure_ptt(
            ChannelId(0),
            PttConfig {
                device_id: DeviceId::AlsaCard { card_name: "Rig".into() },
                method: crate::ptt::registry::PttMethod::None,
                invert: false,
            },
        )
        .unwrap();

        // Reconfigure name + mode; bindings must survive.
        sup.configure_channel(ChannelId(0), "vfo-a".into(), "rtty".into())
            .unwrap();

        let snap = sup.snapshot();
        assert_eq!(snap.channels[0].mode, "rtty");
        assert_eq!(snap.channels[0].device_id, DeviceId::AlsaCard { card_name: "Mic".into() });
        assert_eq!(snap.channels[0].tx_device_id, DeviceId::AlsaCard { card_name: "Speaker".into() });
        assert_eq!(snap.channels[0].sample_rate, 48_000);
        let ptt = snap.channels[0].ptt.as_ref().expect("ptt binding must survive");
        assert_eq!(ptt.device_id, DeviceId::AlsaCard { card_name: "Rig".into() });
    }

    #[test]
    fn configure_audio_then_ptt_binds() {
        let mut sup = supervisor();
        sup.configure_channel(ChannelId(0), "vfo-a".into(), "none".into())
            .unwrap();
        sup.configure_audio(
            ChannelId(0),
            DeviceId::AlsaCard { card_name: "Device".into() },
            44_100,
            1,
            DeviceId::AlsaCard { card_name: "Device".into() },
            0,
        )
        .unwrap();
        sup.configure_ptt(
            ChannelId(0),
            PttConfig {
                device_id: DeviceId::AlsaCard { card_name: "Device".into() },
                method: crate::ptt::registry::PttMethod::None,
                invert: false,
            },
        )
        .unwrap();

        let snap = sup.snapshot();
        assert_eq!(snap.channels[0].sample_rate, 44_100);
        assert!(snap.channels[0].ptt.is_some());
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
                sample_rate: 48_000,
                fanout: 1,
                tx_device_id: crate::ids::DeviceId::placeholder(),
                tx_sample_rate: 0,
                ptt: None,
            })
            .unwrap();
        let sup = Supervisor::new(store, Box::new(RealOpener)).unwrap();
        assert!(sup.has_channel(ChannelId(3)));
    }
}
