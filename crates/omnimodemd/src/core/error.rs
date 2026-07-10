//! Errors surfaced by the sync core to the async control edge.

use crate::ids::ChannelId;

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("unknown channel {0:?}")]
    UnknownChannel(ChannelId),
    #[error("unknown or unsupported mode: {0:?}")]
    UnknownMode(String),
    #[error("picture transmit not possible: {0}")]
    Picture(String),
    #[error("persistence error: {0}")]
    Persist(String),
    #[error("audio error: {0}")]
    Audio(#[from] crate::audio::AudioError),
    #[error("ptt error: {0}")]
    Ptt(#[from] crate::ptt::PttError),
    /// A requested capability exists in the API but is not yet implemented (e.g. a
    /// non-NBFM demod mode, or bias-tee/direct-sampling in Phase A).
    #[error("not implemented: {0}")]
    Unimplemented(String),
    /// The RPC targets an SDR-only control but the channel is not bound to an
    /// `rtl_tcp` SDR source.
    #[error("channel {0:?} is not bound to an SDR source")]
    SdrRequired(ChannelId),
    /// A caller-supplied value was rejected (e.g. an unsupported SDR capture rate).
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("core shutting down")]
    Closed,
}

impl From<crate::persist::StoreError> for CoreError {
    fn from(e: crate::persist::StoreError) -> Self {
        CoreError::Persist(e.to_string())
    }
}
