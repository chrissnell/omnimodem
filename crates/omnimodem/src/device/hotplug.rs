//! Hotplug detection by diffing enumeration snapshots. Emits arrivals and
//! departures keyed on `DeviceId` so config rebinds across renames/hotplug.

use super::enumerate::{DeviceDescriptor, DeviceEnumerator};
use crate::ids::DeviceId;
use std::collections::HashSet;

/// A hotplug transition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HotplugEvent {
    Arrived(DeviceDescriptor),
    Departed(DeviceId),
}

/// Diffs successive snapshots. Hold the previous id set; `poll` returns the
/// changes since the last call.
pub struct HotplugWatcher {
    known: HashSet<DeviceId>,
}

impl HotplugWatcher {
    pub fn new() -> Self {
        HotplugWatcher { known: HashSet::new() }
    }

    /// Enumerate once and return arrivals/departures vs. the previous poll.
    pub fn poll(&mut self, enumerator: &dyn DeviceEnumerator) -> Vec<HotplugEvent> {
        let now: Vec<DeviceDescriptor> = enumerator.enumerate();
        let now_ids: HashSet<DeviceId> = now.iter().map(|d| d.id.clone()).collect();
        let mut events = Vec::new();
        for d in &now {
            if !self.known.contains(&d.id) {
                events.push(HotplugEvent::Arrived(d.clone()));
            }
        }
        for old in &self.known {
            if !now_ids.contains(old) {
                events.push(HotplugEvent::Departed(old.clone()));
            }
        }
        self.known = now_ids;
        events
    }
}

impl Default for HotplugWatcher {
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
            needs_setup: false,
        }
    }

    #[test]
    fn first_poll_reports_all_present_as_arrivals() {
        let en = FakeEnumerator::new(vec![desc("A"), desc("B")]);
        let mut w = HotplugWatcher::new();
        let evs = w.poll(&en);
        assert_eq!(evs.len(), 2);
        assert!(evs.iter().all(|e| matches!(e, HotplugEvent::Arrived(_))));
    }

    #[test]
    fn steady_state_reports_nothing() {
        let en = FakeEnumerator::new(vec![desc("A")]);
        let mut w = HotplugWatcher::new();
        w.poll(&en);
        assert!(w.poll(&en).is_empty());
    }

    #[test]
    fn departure_is_detected() {
        let en = FakeEnumerator::new(vec![desc("A"), desc("B")]);
        let mut w = HotplugWatcher::new();
        w.poll(&en);
        en.set(vec![desc("A")]); // B unplugged
        let evs = w.poll(&en);
        assert_eq!(evs, vec![HotplugEvent::Departed(DeviceId::AlsaCard { card_name: "B".into() })]);
    }

    #[test]
    fn arrival_after_departure_is_detected() {
        let en = FakeEnumerator::new(vec![desc("A")]);
        let mut w = HotplugWatcher::new();
        w.poll(&en);
        en.set(vec![desc("A"), desc("C")]); // C plugged in
        let evs = w.poll(&en);
        assert_eq!(evs, vec![HotplugEvent::Arrived(desc("C"))]);
    }
}
