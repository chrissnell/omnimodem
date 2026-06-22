//! omnimodemd entrypoint: wire the sync core to the authorized gRPC edge.

use omnimodemd::authz::{self, Transport};
use omnimodemd::core::command::Command;
use omnimodemd::grpc::ControlService;
use omnimodemd::metrics::ChannelMetricsSnapshot;
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

    // Routable mTLS bind if OMNIMODEM_ROUTABLE_ADDR is set, else the UDS default.
    let routable_addr = std::env::var("OMNIMODEM_ROUTABLE_ADDR")
        .ok()
        .and_then(|a| a.parse::<std::net::SocketAddr>().ok());
    let transport = match routable_addr {
        Some(addr) => Transport::Routable { addr },
        None => Transport::Uds { path: sock_path.clone() },
    };
    // Fails closed here for a routable bind without TLS material.
    if let Some(warning) = authz::validate_transport(&transport)? {
        tracing::warn!("{warning}");
    }

    let store = Store::open(&db_path)?;
    let (core_handle, _join) = omnimodemd::production_core(store)?;

    // Optional Prometheus exporter (off unless OMNIMODEM_PROMETHEUS_ADDR is set).
    if let Some(addr) = std::env::var("OMNIMODEM_PROMETHEUS_ADDR")
        .ok()
        .and_then(|a| a.parse::<std::net::SocketAddr>().ok())
    {
        let cmds = core_handle.commands.clone();
        let fetch = move || -> Vec<ChannelMetricsSnapshot> {
            let (tx, rx) = tokio::sync::oneshot::channel();
            if cmds.try_send(Command::GetMetrics { channel: None, reply: tx }).is_err() {
                return Vec::new();
            }
            // The serve loop runs on a multi-thread runtime worker; tell tokio we
            // are about to block on the core's reply.
            tokio::task::block_in_place(|| rx.blocking_recv()).unwrap_or_default()
        };
        tokio::spawn(async move {
            if let Err(e) = omnimodemd::metrics::prometheus::serve(addr, fetch).await {
                tracing::warn!("prometheus exporter exited: {e}");
            }
        });
        tracing::info!(%addr, "prometheus exporter enabled");
    }

    let svc = ControlService::new(core_handle);

    // Serve over the selected transport until signalled; Ctrl-C tears it down.
    let serve = async {
        match &transport {
            Transport::Routable { addr } => {
                tracing::info!(%addr, "omnimodemd {} serving (routable mTLS)", omnimodemd::VERSION);
                authz::serve_routable(svc, *addr).await
            }
            _ => {
                tracing::info!(socket = %sock_path.display(), "omnimodemd {} serving (uds)", omnimodemd::VERSION);
                authz::serve_uds(svc, &sock_path).await
            }
        }
    };
    tokio::select! {
        res = serve => { res?; }
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("shutdown signal received");
        }
    }

    let _ = std::fs::remove_file(&sock_path);
    Ok(())
}
