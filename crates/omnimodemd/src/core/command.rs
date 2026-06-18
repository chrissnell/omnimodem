//! Commands sent from the async control edge into the sync core.
//!
//! Each command that needs an acknowledgement carries a `tokio::oneshot`
//! reply sender. `oneshot::Sender::send` is not async, so the sync core thread
//! can answer without a runtime; the async handler awaits the receiver.

use crate::core::error::CoreError;
use crate::ids::{ChannelId, TransmitId};
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
    Shutdown,
}
