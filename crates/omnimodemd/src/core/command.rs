//! Commands sent from the async control edge into the sync core.
//!
//! Each command that needs an acknowledgement carries a `tokio::oneshot`
//! reply sender. `oneshot::Sender::send` is not async, so the sync core thread
//! can answer without a runtime; the async handler awaits the receiver.

use crate::core::error::CoreError;
use crate::device::DeviceDescriptor;
use crate::ids::{ChannelId, DeviceId, TransmitId};
use crate::supervisor::ModemSnapshot;
use tokio::sync::oneshot;

pub enum Command {
    ConfigureChannel {
        id: ChannelId,
        name: String,
        mode: String,
        reply: oneshot::Sender<Result<(), CoreError>>,
    },
    Transmit {
        channel: ChannelId,
        payload: Vec<u8>,
        reply: oneshot::Sender<Result<TransmitId, CoreError>>,
    },
    GetState {
        reply: oneshot::Sender<ModemSnapshot>,
    },
    ConfigureAudio {
        id: ChannelId,
        device_id: DeviceId,
        sample_rate: u32,
        fanout: u32,
        tx_device_id: DeviceId,
        tx_sample_rate: u32,
        reply: oneshot::Sender<Result<ConfigureAudioOk, CoreError>>,
    },
    ConfigurePtt {
        id: ChannelId,
        ptt: crate::ptt::registry::PttConfig,
        reply: oneshot::Sender<Result<(), CoreError>>,
    },
    KeyPtt {
        channel: ChannelId,
        keyed: bool,
        reply: oneshot::Sender<Result<(), CoreError>>,
    },
    ListDevices {
        reply: oneshot::Sender<Vec<DeviceDescriptor>>,
    },
    SuggestUdevRule {
        device_id: DeviceId,
        reply: oneshot::Sender<Result<(String, String), CoreError>>,
    },
    /// Snapshot per-channel metrics. `channel: None` returns every channel.
    GetMetrics {
        channel: Option<ChannelId>,
        reply: oneshot::Sender<Vec<crate::metrics::ChannelMetricsSnapshot>>,
    },
    /// Acquire the exclusive TX lease on `channel`'s bound rig.
    AcquireTxLease {
        channel: ChannelId,
        reply: oneshot::Sender<Result<LeaseGrant, CoreError>>,
    },
    /// Release the exclusive TX lease `channel` holds on its bound rig.
    ReleaseTxLease {
        channel: ChannelId,
        reply: oneshot::Sender<Result<(), CoreError>>,
    },
    /// Set a channel's runtime RX/TX audio gain (linear multipliers).
    SetAudioGain {
        channel: ChannelId,
        rx_gain: f32,
        tx_gain: f32,
        reply: oneshot::Sender<Result<(), CoreError>>,
    },
    Shutdown,
}

/// Opened rates from `ConfigureAudio`: the capture rate and the playback rate.
#[derive(Debug, Clone, Copy)]
pub struct ConfigureAudioOk {
    pub rx_rate: u32,
    pub tx_rate: u32,
}

/// Outcome of an `AcquireTxLease`: whether it was granted, and the current
/// holder when it was not.
#[derive(Debug, Clone, Copy)]
pub struct LeaseGrant {
    pub granted: bool,
    pub held_by: Option<ChannelId>,
}
