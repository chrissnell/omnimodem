//! Recently-seen ICAO roster for roster-gated frame recovery (R5).
//!
//! The address-overlaid downlink formats (DF0/4/5/16/20/21) fold their 24-bit
//! ICAO address into the parity field — a received frame checksums not to zero
//! but to the address itself (`AP = parity XOR ICAO`). So there is nothing in the
//! frame to validate against: any 56/112-bit noise slice "decodes" to *some*
//! address. Accepting them blindly manufactures aircraft.
//!
//! The discipline that makes them safe is a roster: only accept an overlaid frame
//! whose recovered address matches one already seen in a frame that *did* carry
//! its address in the clear and checksummed to zero — a DF11 all-call reply or a
//! DF17/18 extended squitter. A real aircraft squitters those repeatedly, so its
//! address is on the roster before (and long after) any of its surveillance
//! replies; a one-off noise slice's "address" is not, and is rejected. This is
//! the address-list gate readsb/dump1090 apply to the same frames.
//!
//! Recency-bounded by insertion count rather than wall-clock: the offline decode
//! path has no clock, and on a bounded capture a generous capacity keeps every
//! aircraft resident for the whole recording while still evicting stale addresses
//! on a long-running stream.

use std::collections::{HashSet, VecDeque};

/// Default roster capacity — how many distinct recently-seen ICAO addresses to
/// keep. Comfortably above the aircraft count of a single receiver's coverage, so
/// no live aircraft is evicted while it is still transmitting, yet bounded so a
/// long stream does not accumulate addresses without limit.
pub const DEFAULT_ROSTER_CAP: usize = 256;

/// A bounded set of recently-seen ICAO addresses, oldest evicted first.
#[derive(Clone, Debug)]
pub struct IcaoRoster {
    cap: usize,
    /// Insertion order for eviction; front is oldest. Re-noting a present address
    /// refreshes its recency by moving it to the back.
    order: VecDeque<u32>,
    set: HashSet<u32>,
}

impl IcaoRoster {
    /// A roster holding up to `cap` addresses (`cap >= 1`).
    pub fn new(cap: usize) -> Self {
        assert!(cap >= 1, "roster capacity must be >= 1");
        Self { cap, order: VecDeque::with_capacity(cap), set: HashSet::with_capacity(cap) }
    }

    /// Record `icao` as recently seen, evicting the oldest address if full. A
    /// re-noted address is refreshed to newest rather than duplicated.
    pub fn note(&mut self, icao: u32) {
        if self.set.contains(&icao) {
            if let Some(pos) = self.order.iter().position(|&a| a == icao) {
                self.order.remove(pos);
            }
            self.order.push_back(icao);
            return;
        }
        if self.order.len() == self.cap {
            if let Some(old) = self.order.pop_front() {
                self.set.remove(&old);
            }
        }
        self.order.push_back(icao);
        self.set.insert(icao);
    }

    /// Whether `icao` is on the roster.
    pub fn contains(&self, icao: u32) -> bool {
        self.set.contains(&icao)
    }

    /// Number of addresses currently held.
    pub fn len(&self) -> usize {
        self.order.len()
    }

    /// Whether the roster holds no addresses.
    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// Forget every address (used on demod reset).
    pub fn clear(&mut self) {
        self.order.clear();
        self.set.clear();
    }
}

impl Default for IcaoRoster {
    fn default() -> Self {
        Self::new(DEFAULT_ROSTER_CAP)
    }
}

/// Whether downlink format `df` carries its ICAO address XOR-folded into the
/// parity (the address/parity `AP` field) rather than in the clear. For these the
/// CRC residual of a correctly received frame equals the transmitting aircraft's
/// address — the value the roster gate checks. DF11's address is in the clear and
/// its residual encodes the interrogator id, so it is deliberately excluded; DF17
/// and DF18 carry the address in bits 8..32 and checksum to zero, also excluded.
pub fn is_address_overlaid(df: u8) -> bool {
    matches!(df, 0 | 4 | 5 | 16 | 20 | 21)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_and_contains() {
        let mut r = IcaoRoster::new(4);
        assert!(!r.contains(0xABCDEF));
        r.note(0xABCDEF);
        assert!(r.contains(0xABCDEF));
        assert_eq!(r.len(), 1);
    }

    #[test]
    fn re_noting_is_idempotent_and_refreshes_recency() {
        let mut r = IcaoRoster::new(2);
        r.note(0x1);
        r.note(0x2);
        r.note(0x1); // refresh 0x1, so 0x2 is now oldest
        r.note(0x3); // evicts the oldest (0x2), not the refreshed 0x1
        assert_eq!(r.len(), 2);
        assert!(r.contains(0x1));
        assert!(r.contains(0x3));
        assert!(!r.contains(0x2));
    }

    #[test]
    fn evicts_oldest_when_full() {
        let mut r = IcaoRoster::new(2);
        r.note(0x1);
        r.note(0x2);
        r.note(0x3);
        assert!(!r.contains(0x1)); // oldest evicted
        assert!(r.contains(0x2));
        assert!(r.contains(0x3));
    }

    #[test]
    fn address_overlaid_formats() {
        for df in [0, 4, 5, 16, 20, 21] {
            assert!(is_address_overlaid(df), "DF{df} should be address-overlaid");
        }
        for df in [11, 17, 18] {
            assert!(!is_address_overlaid(df), "DF{df} carries its address in the clear");
        }
    }
}
