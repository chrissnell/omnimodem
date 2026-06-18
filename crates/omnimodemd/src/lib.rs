//! Omnimodem daemon library surface.
//!
//! The binary in `main.rs` is a thin wrapper; everything testable lives here so
//! integration tests in `tests/` can spawn the server in-process.

/// Crate version, surfaced to clients in the gRPC handshake metadata.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
