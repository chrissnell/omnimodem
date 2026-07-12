//! Events emitted by the core, split into two backpressure classes.
//!
//! Design policy (locked in Phase 1): decoded frames are LOSSLESS — never
//! silently dropped; a client that can't keep up is disconnected. Telemetry
//! (levels, status, transmit notifications) is LOSSY — only the latest value
//! matters, so dropping intermediates under lag is fine.

use crate::ids::{ChannelId, DeviceId, TransmitId};

/// A decoded raster payload (Hell, WEFAX, picture sub-protocols): `pixels` is
/// row-major 8-bit samples, `channels` interleaved values per pixel (1 =
/// grayscale, 3 = RGB). `pixels.len() == width * rows * channels`.
#[derive(Debug, Clone)]
pub struct RxImage {
    pub width: u16,
    pub channels: u8,
    pub pixels: Vec<u8>,
}

/// LOSSLESS class. Carried on a dedicated broadcast; a subscriber that lags is
/// disconnected rather than allowed to miss a frame.
#[derive(Debug, Clone)]
pub enum FrameEvent {
    RxFrame {
        channel: ChannelId,
        /// Opaque/text bytes for non-raster payloads; empty when `image` is set.
        data: Vec<u8>,
        /// Typed raster for facsimile modes; `None` for byte payloads.
        image: Option<RxImage>,
        timestamp_ns: u64,
    },
}

/// LOSSY class. Carried on a separate broadcast; lag drops intermediates.
#[derive(Debug, Clone)]
pub enum TelemetryEvent {
    ChannelConfigured { channel: ChannelId },
    TransmitStarted { channel: ChannelId, transmit_id: TransmitId },
    TransmitComplete { channel: ChannelId, transmit_id: TransmitId },
    /// A transmit that never keyed because the frame could not be encoded in the
    /// channel's mode. Carries a human-readable reason so the client can surface
    /// it instead of leaving the operator with unexplained silence.
    TransmitFailed { channel: ChannelId, transmit_id: TransmitId, reason: String },
    AudioLevel { channel: ChannelId, dbfs: f32 },
    Status { channel: ChannelId, tx_frames: u64 },
    DeviceArrived { device_id: DeviceId, label: String },
    DeviceDeparted { device_id: DeviceId },
    PttKeyed { channel: ChannelId, keyed: bool },
    /// Host clock-discipline metric so operators can tell a time-sync problem
    /// (windowed modes need an accurate clock) from a signal problem.
    ClockOffset { offset_s: f64, est_error_s: f64, synchronized: bool },
    /// Per-channel decode/health metrics (lossy: only the latest matters).
    ChannelMetrics {
        channel: ChannelId,
        good_frames: u64,
        bad_frames: u64,
        snr_db: f32,
        dbfs: f32,
        afc_offset_hz: f32,
        dcd: bool,
        last_decoder: Option<String>,
    },
    /// A received RSID burst was identified (lossy: advisory annotation). `tag`
    /// is the fldigi RSID tag (always known); `mode` is the omnimodem mode string
    /// (empty if unported); `freq_hz` is the detected audio offset.
    RsidDetected {
        channel: ChannelId,
        tag: String,
        mode: String,
        freq_hz: f32,
        extended: bool,
    },
    /// One waterfall line (lossy: a dropped line is invisible). `bins` is uint8
    /// dBFS over `[db_floor, db_ceiling]`, low→high frequency.
    SpectrumFrame {
        channel: ChannelId,
        timestamp_ns: u64,
        freq_start_hz: f32,
        freq_step_hz: f32,
        db_floor: f32,
        db_ceiling: f32,
        bins: Vec<u8>,
        /// True for the transmitted (TX) spectrum, false for received (RX).
        transmit: bool,
    },
    /// Current SDR tuner/demod state for a channel (lossy: only the latest
    /// matters). Broadcast on each SetSdrTune/SetSdrGain/ConfigureSdr so multiple
    /// and late-joining clients stay in sync. `demod_mode` is `DemodMode as u8`.
    SdrState {
        channel: ChannelId,
        center_hz: f64,
        offset_hz: f64,
        freq_hz: f64,
        gain_auto: bool,
        gain_db: f32,
        demod_mode: u8,
        squelch_db: f32,
    },
    /// Per-aircraft ADS-B state from an ADS-B channel (lossy: intermediate
    /// reports may be dropped under lag). Emitted when a decoded squitter
    /// changes the aircraft's reportable state; position/velocity/altitude are
    /// optional until their carrying squitter has been heard.
    AircraftReport {
        channel: ChannelId,
        icao: u32,
        callsign: Option<String>,
        latitude: Option<f64>,
        longitude: Option<f64>,
        altitude_ft: Option<i32>,
        ground_speed_kt: Option<f64>,
        track_deg: Option<f64>,
        vertical_rate_fpm: Option<i32>,
        last_seen_ms: u64,
        /// Running count of squitters folded into this track — "how many packets
        /// we received from this plane".
        messages: u32,
    },
}
