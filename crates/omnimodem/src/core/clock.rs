//! Host time base for windowed modes. WSJT-X-family modes need an accurate
//! clock (design §"Time synchronization"); we depend on NTP/PTP disciplining
//! and surface the offset as a metric. `SlotClock` computes the wall-clock
//! delay to the next slot boundary; `ClockSource` reports the host offset.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Computes the delay until the next `slot_s`-aligned UTC boundary (FT8: 15 s).
pub struct SlotClock {
    slot: Duration,
}

impl SlotClock {
    pub fn new(slot_s: f32) -> Self {
        SlotClock { slot: Duration::from_secs_f32(slot_s) }
    }

    /// Delay from `now` (UNIX time) to the next slot boundary.
    pub fn delay_from(&self, now: Duration) -> Duration {
        let slot_ns = self.slot.as_nanos() as u64;
        if slot_ns == 0 {
            return Duration::ZERO;
        }
        let into = (now.as_nanos() as u64) % slot_ns;
        if into == 0 {
            Duration::ZERO
        } else {
            Duration::from_nanos(slot_ns - into)
        }
    }

    /// Delay until the next boundary from the real clock.
    pub fn delay_until_next(&self) -> Duration {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO);
        self.delay_from(now)
    }
}

/// A reading of the host clock discipline state.
#[derive(Debug, Clone, Copy)]
pub struct ClockReading {
    /// Current NTP offset estimate (seconds; how far the clock is being steered).
    pub offset_s: f64,
    /// Estimated error (seconds).
    pub est_error_s: f64,
    /// Whether the kernel reports the clock as synchronized (not UNSYNC).
    pub synchronized: bool,
}

/// Reads the host clock-discipline state (Linux `ntp_adjtime`). On other
/// platforms or when the syscall is unavailable it reports an unsynchronized
/// zero-offset reading so callers always get a finite metric.
pub struct ClockSource;

impl ClockSource {
    pub fn new() -> Self {
        ClockSource
    }

    /// Read the kernel NTP discipline state.
    pub fn read(&self) -> ClockReading {
        #[cfg(target_os = "linux")]
        {
            // SAFETY: `ntp_adjtime` fully writes the timex on success; we only
            // read scalar fields out of it afterward.
            let mut tx: libc::timex = unsafe { std::mem::zeroed() };
            let ret = unsafe { libc::ntp_adjtime(&mut tx) };
            if ret >= 0 {
                // STA_NANO selects ns for `offset`; default is µs. `esterror` is
                // always µs.
                let nano = (tx.status & libc::STA_NANO) != 0;
                let off_scale = if nano { 1e9 } else { 1e6 };
                return ClockReading {
                    offset_s: tx.offset as f64 / off_scale,
                    est_error_s: tx.esterror as f64 / 1e6,
                    synchronized: ret != libc::TIME_ERROR,
                };
            }
        }
        ClockReading { offset_s: 0.0, est_error_s: f64::from(u16::MAX), synchronized: false }
    }
}

impl Default for ClockSource {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delay_to_next_15s_boundary() {
        let c = SlotClock::new(15.0);
        // 7 s into a slot → 8 s to the next boundary.
        assert_eq!(c.delay_from(Duration::from_secs(7)), Duration::from_secs(8));
        // Exactly on a boundary → no wait.
        assert_eq!(c.delay_from(Duration::from_secs(30)), Duration::ZERO);
        // 14.5 s in → 0.5 s.
        assert_eq!(c.delay_from(Duration::from_millis(14_500)), Duration::from_millis(500));
    }

    #[test]
    fn clock_source_reports_a_finite_offset() {
        let r = ClockSource::new().read();
        assert!(r.est_error_s.is_finite() && r.est_error_s >= 0.0);
        assert!(r.offset_s.is_finite());
    }
}
