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
    #[error("core shutting down")]
    Closed,
}

impl From<crate::persist::StoreError> for CoreError {
    fn from(e: crate::persist::StoreError) -> Self {
        CoreError::Persist(e.to_string())
    }
}
