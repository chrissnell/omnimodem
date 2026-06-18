//! omnimodemd entrypoint: wire the sync core to the authorized gRPC edge.

use omnimodemd::authz::{self, Transport};
use omnimodemd::grpc::ControlService;
use omnimodemd::persist::Store;
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    // Phase 1 config is environment-driven; a real arg parser arrives later.
    let runtime_dir = std::env::var("OMNIMODEM_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir().join("omnimodem"));
    std::fs::create_dir_all(&runtime_dir)?;
    let sock_path = runtime_dir.join("omnimodem.sock");
    let db_path = runtime_dir.join("omnimodem.sqlite");

    let transport = Transport::Uds { path: sock_path.clone() };
    if let Some(warning) = authz::validate_transport(&transport)? {
        tracing::warn!("{warning}");
    }

    let store = Store::open(&db_path)?;
    let (core_handle, _join) = omnimodemd::production_core(store)?;
    let svc = ControlService::new(core_handle);

    tracing::info!(socket = %sock_path.display(), "omnimodemd {} serving", omnimodemd::VERSION);

    // serve_uds runs until the process is signalled; Ctrl-C tears it down.
    tokio::select! {
        res = authz::serve_uds(svc, &sock_path) => { res?; }
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("shutdown signal received");
        }
    }

    let _ = std::fs::remove_file(&sock_path);
    Ok(())
}
