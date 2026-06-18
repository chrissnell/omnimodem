//! Events emitted by the core, split into two backpressure classes.
//!
//! Design policy (locked in Phase 1): decoded frames are LOSSLESS — never
//! silently dropped; a client that can't keep up is disconnected. Telemetry
//! (levels, status, transmit notifications) is LOSSY — only the latest value
//! matters, so dropping intermediates under lag is fine.

use crate::ids::{ChannelId, DeviceId, TransmitId};

/// LOSSLESS class. Carried on a dedicated broadcast; a subscriber that lags is
/// disconnected rather than allowed to miss a frame.
#[derive(Debug, Clone)]
pub enum FrameEvent {
    RxFrame {
        channel: ChannelId,
        data: Vec<u8>,
        timestamp_ns: u64,
    },
}

/// LOSSY class. Carried on a separate broadcast; lag drops intermediates.
#[derive(Debug, Clone)]
pub enum TelemetryEvent {
    ChannelConfigured { channel: ChannelId },
    TransmitStarted { channel: ChannelId, transmit_id: TransmitId },
    TransmitComplete { channel: ChannelId, transmit_id: TransmitId },
    AudioLevel { channel: ChannelId, dbfs: f32 },
    Status { channel: ChannelId, tx_frames: u64 },
    DeviceArrived { device_id: DeviceId, label: String },
    DeviceDeparted { device_id: DeviceId },
    PttKeyed { channel: ChannelId, keyed: bool },
}
