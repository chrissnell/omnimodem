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
        reply: oneshot::Sender<Result<u32, CoreError>>, // actual rate
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
    Shutdown,
}
