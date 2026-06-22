//! Per-channel metrics (design §"Metrics & observability"). The RX worker feeds
//! decode outcomes in; `snapshot()` produces an immutable view emitted on the
//! lossy telemetry channel, served by `GetMetrics`, and rendered by the optional
//! Prometheus exporter. Fields cover the design's goal-#4 data sources: SNR,
//! level, DCD, good/bad-frame counts, PTT state, AFC offset, audio over/underrun
//! & clip counts, and which ensemble member decoded the most recent frame.

pub mod prometheus;

use crate::ids::ChannelId;

/// Mutable per-channel accumulator. Cheap to update on the hot path (plain
/// integer/float writes); a snapshot is taken on the control edge.
#[derive(Debug, Default, Clone)]
pub struct ChannelMetrics {
    pub good_frames: u64,
    pub bad_frames: u64,
    pub tx_frames: u64,
    pub snr_db: f32,
    pub dbfs: f32,
    pub afc_offset_hz: f32,
    pub dcd: bool,
    pub ptt_keyed: bool,
    pub audio_overruns: u64,
    pub audio_underruns: u64,
    pub clip_count: u64,
    /// Which ensemble member / slicer decoded the most recent frame.
    pub last_decoder: Option<String>,
}

impl ChannelMetrics {
    /// Record a decoded frame: bump good/bad by CRC validity and remember which
    /// ensemble member produced it.
    pub fn record_frame(&mut self, crc_ok: bool, decoder: Option<&str>) {
        if crc_ok {
            self.good_frames += 1;
        } else {
            self.bad_frames += 1;
        }
        if let Some(d) = decoder {
            self.last_decoder = Some(d.to_string());
        }
    }

    /// Frame error rate over all decoded frames (0.0 when none seen).
    pub fn fer(&self) -> f32 {
        let total = self.good_frames + self.bad_frames;
        if total == 0 {
            0.0
        } else {
            self.bad_frames as f32 / total as f32
        }
    }

    pub fn snapshot(&self, channel: ChannelId) -> ChannelMetricsSnapshot {
        ChannelMetricsSnapshot { channel, metrics: self.clone() }
    }
}

/// Immutable snapshot carried over telemetry / served by `GetMetrics`.
#[derive(Debug, Clone)]
pub struct ChannelMetricsSnapshot {
    pub channel: ChannelId,
    pub metrics: ChannelMetrics,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_frame_counts_good_and_bad() {
        let mut m = ChannelMetrics::default();
        m.record_frame(true, Some("afsk1200/hydra"));
        m.record_frame(false, None);
        m.record_frame(true, Some("afsk1200/single"));
        assert_eq!(m.good_frames, 2);
        assert_eq!(m.bad_frames, 1);
        assert!((m.fer() - 1.0 / 3.0).abs() < 1e-6);
        assert_eq!(m.last_decoder.as_deref(), Some("afsk1200/single"));
    }

    #[test]
    fn fer_is_zero_with_no_frames() {
        assert_eq!(ChannelMetrics::default().fer(), 0.0);
    }

    #[test]
    fn snapshot_carries_channel_and_copy() {
        let mut m = ChannelMetrics::default();
        m.good_frames = 4;
        let snap = m.snapshot(ChannelId(7));
        assert_eq!(snap.channel, ChannelId(7));
        assert_eq!(snap.metrics.good_frames, 4);
    }
}
