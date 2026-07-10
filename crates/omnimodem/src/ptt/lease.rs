//! TX exclusive lease (design §"Phase 4 TX model" / §"Open questions": the lease
//! itself slips to Phase 5). A session that cannot tolerate interleaved TX
//! (contest/Winlink) takes an exclusive lease on a rig; while held, only the
//! lease-holding channel may transmit on that rig. Keyed by `DeviceId` (the rig),
//! because two channels sharing one physical radio contend for the same lease.
//! This composes with — does not replace — the `RxTxInterlock`: the interlock
//! mutes RX during any key; the lease governs *who may queue TX at all*.

use crate::ids::{ChannelId, DeviceId};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
pub struct TxLeaseRegistry {
    holders: Arc<Mutex<HashMap<DeviceId, ChannelId>>>,
}

/// Why an `acquire` failed.
#[derive(Debug, PartialEq, Eq)]
pub enum LeaseError {
    /// Another channel already holds the rig's exclusive lease.
    HeldBy(ChannelId),
}

impl TxLeaseRegistry {
    pub fn new() -> Self {
        TxLeaseRegistry::default()
    }

    /// Acquire the exclusive lease on `rig` for `channel`. Idempotent for the
    /// current holder; errors if a different channel holds it.
    pub fn acquire(&self, rig: &DeviceId, channel: ChannelId) -> Result<(), LeaseError> {
        let mut map = self.holders.lock().unwrap();
        match map.get(rig) {
            Some(&h) if h == channel => Ok(()),
            Some(&h) => Err(LeaseError::HeldBy(h)),
            None => {
                map.insert(rig.clone(), channel);
                Ok(())
            }
        }
    }

    /// Release the lease if `channel` holds it (no-op otherwise).
    pub fn release(&self, rig: &DeviceId, channel: ChannelId) {
        let mut map = self.holders.lock().unwrap();
        if map.get(rig) == Some(&channel) {
            map.remove(rig);
        }
    }

    /// May `channel` transmit on `rig` right now? True if the rig is unleased or
    /// leased to this channel. The TX worker checks this before keying.
    pub fn may_transmit(&self, rig: &DeviceId, channel: ChannelId) -> bool {
        match self.holders.lock().unwrap().get(rig) {
            Some(&h) => h == channel,
            None => true,
        }
    }

    /// The current holder of `rig`'s lease, if any.
    pub fn holder(&self, rig: &DeviceId) -> Option<ChannelId> {
        self.holders.lock().unwrap().get(rig).copied()
    }

    /// Release every lease a channel holds (called on channel teardown).
    pub fn release_all(&self, channel: ChannelId) {
        self.holders.lock().unwrap().retain(|_, &mut h| h != channel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rig(t: &str) -> DeviceId {
        DeviceId::AlsaCard { card_name: t.into() }
    }

    #[test]
    fn unleased_rig_allows_any_channel() {
        let r = TxLeaseRegistry::new();
        assert!(r.may_transmit(&rig("A"), ChannelId(1)));
    }

    #[test]
    fn holder_excludes_others() {
        let r = TxLeaseRegistry::new();
        r.acquire(&rig("A"), ChannelId(1)).unwrap();
        assert!(r.may_transmit(&rig("A"), ChannelId(1)));
        assert!(!r.may_transmit(&rig("A"), ChannelId(2)));
        assert_eq!(r.acquire(&rig("A"), ChannelId(2)), Err(LeaseError::HeldBy(ChannelId(1))));
        assert_eq!(r.holder(&rig("A")), Some(ChannelId(1)));
    }

    #[test]
    fn acquire_is_idempotent_for_holder() {
        let r = TxLeaseRegistry::new();
        r.acquire(&rig("A"), ChannelId(1)).unwrap();
        assert!(r.acquire(&rig("A"), ChannelId(1)).is_ok());
    }

    #[test]
    fn release_frees_the_rig() {
        let r = TxLeaseRegistry::new();
        r.acquire(&rig("A"), ChannelId(1)).unwrap();
        r.release(&rig("A"), ChannelId(1));
        assert!(r.may_transmit(&rig("A"), ChannelId(2)));
        assert_eq!(r.holder(&rig("A")), None);
    }

    #[test]
    fn release_by_non_holder_is_a_noop() {
        let r = TxLeaseRegistry::new();
        r.acquire(&rig("A"), ChannelId(1)).unwrap();
        r.release(&rig("A"), ChannelId(2)); // not the holder
        assert!(!r.may_transmit(&rig("A"), ChannelId(2)));
    }

    #[test]
    fn release_all_drops_every_lease_for_a_channel() {
        let r = TxLeaseRegistry::new();
        r.acquire(&rig("A"), ChannelId(1)).unwrap();
        r.acquire(&rig("B"), ChannelId(1)).unwrap();
        r.release_all(ChannelId(1));
        assert!(r.may_transmit(&rig("A"), ChannelId(2)));
        assert!(r.may_transmit(&rig("B"), ChannelId(2)));
    }

    #[test]
    fn two_rigs_are_independent() {
        let r = TxLeaseRegistry::new();
        r.acquire(&rig("A"), ChannelId(1)).unwrap();
        // A different rig is still freely leasable by another channel.
        assert!(r.acquire(&rig("B"), ChannelId(2)).is_ok());
    }
}
