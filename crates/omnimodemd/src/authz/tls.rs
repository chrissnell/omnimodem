//! mTLS hook for routable binds.
//!
//! Phase 1 does NOT implement mTLS. This hook exists so the routable code path
//! fails CLOSED: any attempt to bind a routable interface errors here instead
//! of silently serving an unauthenticated, internet-reachable transmitter
//! control socket. Phase 5 fills this in (cert loading + per-method authz).

/// Error returned when a routable bind is attempted in Phase 1.
#[derive(Debug, thiserror::Error)]
pub enum TlsError {
    #[error("mTLS is required for routable binds and is not implemented yet (Phase 5)")]
    NotImplemented,
}

/// Build the mTLS server config for a routable bind. Always errors in Phase 1.
pub fn routable_tls_config() -> Result<(), TlsError> {
    Err(TlsError::NotImplemented)
}
