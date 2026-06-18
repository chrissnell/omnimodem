//! Errors surfaced by the sync core to the async control edge.

use crate::ids::ChannelId;

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("unknown channel {0:?}")]
    UnknownChannel(ChannelId),
    #[error("persistence error: {0}")]
    Persist(String),
    #[error("core shutting down")]
    Closed,
}

impl From<crate::persist::StoreError> for CoreError {
    fn from(e: crate::persist::StoreError) -> Self {
        CoreError::Persist(e.to_string())
    }
}
