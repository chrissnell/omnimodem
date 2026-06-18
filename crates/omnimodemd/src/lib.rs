//! Omnimodem daemon library surface.
//!
//! The binary in `main.rs` is a thin wrapper; everything testable lives here so
//! integration tests in `tests/` can spawn the server in-process.

/// Crate version, surfaced to clients in the gRPC handshake metadata.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod proto;

pub mod ids;

pub mod persist;

pub mod supervisor;

pub mod core;

pub mod grpc;

pub mod authz;

use std::path::Path;

/// Spawn the full control plane (core + gRPC) listening on a UDS at `path`.
/// Returns the core's join handle and a shutdown trigger. Used by integration
/// tests and by `main.rs`. Authz is applied by `authz::serve_uds` (Task 10);
/// this no-authz variant exists for unary/subscribe tests.
pub async fn serve_uds_no_authz(
    db_path: &Path,
    sock_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    use grpc::ControlService;
    use proto::modem_control_server::ModemControlServer;
    use tokio_stream::wrappers::UnixListenerStream;

    let store = persist::Store::open(db_path)?;
    let supervisor = supervisor::Supervisor::new(store)?;
    let (core, _join) = core::spawn(supervisor);
    let svc = ControlService::new(core);

    let _ = std::fs::remove_file(sock_path);
    let listener = tokio::net::UnixListener::bind(sock_path)?;
    let incoming = UnixListenerStream::new(listener);

    tonic::transport::Server::builder()
        .add_service(ModemControlServer::new(svc))
        .serve_with_incoming(incoming)
        .await?;
    Ok(())
}
