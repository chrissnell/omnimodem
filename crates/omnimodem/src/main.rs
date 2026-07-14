//! omnimodem entrypoint: wire the sync core to the authorized gRPC edge.

use omnimodem::authz::{self, Transport};
use omnimodem::core::command::Command;
use omnimodem::grpc::ControlService;
use omnimodem::metrics::ChannelMetricsSnapshot;
use omnimodem::persist::Store;
use nix::fcntl::{Flock, FlockArg};
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Capture the spawning parent's pid before any startup work. The parent-death
    // watchdog compares against this, so it must be read before our own (possibly
    // slow) init could let the parent exit and reparent us — otherwise we'd record
    // pid 1 as the "original" and never detect the death. SAFETY: getppid() always
    // succeeds and has no preconditions.
    let original_ppid = unsafe { libc::getppid() };

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

    // Single-instance guard: hold an exclusive advisory lock on the runtime dir for
    // the whole process. Two daemons sharing a runtime dir would race the same
    // socket and SQLite file (and a leftover daemon would keep the SDR claimed for
    // exclusive access); the kernel drops this lock the instant we exit — even on a
    // crash or SIGKILL — so a stale lock can never wedge the next start. Bound to
    // `_instance_lock` so it lives until `main` returns. See GRA-371.
    let _instance_lock = match acquire_instance_lock(&runtime_dir) {
        Ok(lock) => lock,
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
            tracing::error!(
                lock = %runtime_dir.join("omnimodem.lock").display(),
                "another omnimodem instance already holds the runtime lock; exiting",
            );
            std::process::exit(1);
        }
        Err(e) => return Err(e.into()),
    };

    // Optional daemon config file: registers `rtl_tcp` SDR endpoints so
    // ListDevices can surface them. Defaults to <runtime_dir>/omnimodem.conf;
    // a missing file is normal (no registered devices).
    let config_path = std::env::var("OMNIMODEM_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|_| runtime_dir.join("omnimodem.conf"));
    let registered_devices = omnimodem::config::load_registered_devices(&config_path);
    if !registered_devices.is_empty() {
        tracing::info!(
            count = registered_devices.len(),
            config = %config_path.display(),
            "registered devices from config",
        );
    }

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

    tracing::info!(
        version = omnimodem::VERSION,
        runtime_dir = %runtime_dir.display(),
        "omnimodem starting",
    );
    let store = Store::open(&db_path)?;
    let (core_handle, _join) = omnimodem::production_core(store, registered_devices)?;

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
            if let Err(e) = omnimodem::metrics::prometheus::serve(addr, fetch).await {
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
                tracing::info!(%addr, "omnimodem {} serving (routable mTLS)", omnimodem::VERSION);
                authz::serve_routable(svc, *addr).await
            }
            _ => {
                tracing::info!(socket = %sock_path.display(), "omnimodem {} serving (uds)", omnimodem::VERSION);
                authz::serve_uds(svc, &sock_path).await
            }
        }
    };
    // Exit when the parent app goes away, so a crash or force-quit that skips the
    // app's clean teardown can't leave us orphaned holding the SDR. Opt-in via the
    // spawn env because a standalone daemon (systemd, manual run) reparents to
    // pid 1 legitimately and must not treat that as its parent dying.
    let exit_with_parent = std::env::var("OMNIMODEM_EXIT_WITH_PARENT")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    tokio::select! {
        res = serve => { res?; }
        _ = shutdown_signal(exit_with_parent, original_ppid) => {}
    }

    let _ = std::fs::remove_file(&sock_path);
    Ok(())
}

/// Acquire the exclusive advisory lock that enforces one daemon per runtime dir.
/// Returns the held lock (keep it alive for the process lifetime). A lock already
/// held by another instance surfaces as `ErrorKind::WouldBlock` so the caller can
/// distinguish "someone else is running" from an I/O failure opening the file.
fn acquire_instance_lock(runtime_dir: &std::path::Path) -> std::io::Result<Flock<std::fs::File>> {
    let lock_path = runtime_dir.join("omnimodem.lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)?;
    // A non-blocking exclusive lock returns EWOULDBLOCK when contended, which
    // `from_raw_os_error` maps to `ErrorKind::WouldBlock`.
    Flock::lock(file, FlockArg::LockExclusiveNonblock)
        .map_err(|(_, errno)| std::io::Error::from_raw_os_error(errno as i32))
}

/// Resolve once any of our shutdown triggers fires: SIGINT (Ctrl-C), SIGTERM (the
/// app's graceful stop), or — when `watch_parent` is set — the parent process
/// (identified by `original_ppid`) exiting.
async fn shutdown_signal(watch_parent: bool, original_ppid: libc::pid_t) {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
        tracing::info!("received SIGINT; shutting down");
    };

    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
                tracing::info!("received SIGTERM; shutting down");
            }
            Err(e) => {
                tracing::warn!("could not install SIGTERM handler: {e}");
                std::future::pending::<()>().await;
            }
        }
    };

    let parent_death = async {
        if watch_parent {
            wait_for_parent_death(original_ppid).await;
            tracing::warn!("parent process exited; shutting down");
        } else {
            std::future::pending::<()>().await;
        }
    };

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
        _ = parent_death => {}
    }
}

/// Poll until our parent goes away. Returns when the current ppid no longer matches
/// the spawner, or is pid 1 (launchd/init) — the app is a direct parent and is never
/// pid 1, so reparenting there means it died. Assumes a direct parent-child spawn;
/// an intermediate launcher that outlives the app would mask its exit. Polling (vs.
/// kqueue EVFILT_PROC) keeps this dependency-free; sub-second latency is irrelevant
/// for releasing an SDR. Checks before the first sleep so an already-dead parent is
/// caught immediately.
async fn wait_for_parent_death(original_ppid: libc::pid_t) {
    loop {
        // SAFETY: getppid() always succeeds and has no preconditions.
        let current = unsafe { libc::getppid() };
        if current == 1 || current != original_ppid {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instance_lock_is_exclusive_per_runtime_dir() {
        let dir = tempfile::tempdir().expect("tempdir");

        let first = acquire_instance_lock(dir.path()).expect("first lock should succeed");

        let second = acquire_instance_lock(dir.path());
        assert_eq!(
            second.err().map(|e| e.kind()),
            Some(std::io::ErrorKind::WouldBlock),
            "a second lock on the same runtime dir must be contended",
        );

        // Releasing the first lock frees the runtime dir for a fresh acquire.
        drop(first);
        acquire_instance_lock(dir.path()).expect("lock should be reacquirable after release");
    }

    #[test]
    fn separate_runtime_dirs_do_not_contend() {
        let feed = tempfile::tempdir().expect("tempdir");
        let scan = tempfile::tempdir().expect("tempdir");

        // Mirrors the app's feed vs. USB-scan split: distinct runtime dirs each hold
        // their own lock without blocking the other.
        let _feed_lock = acquire_instance_lock(feed.path()).expect("feed lock");
        let _scan_lock = acquire_instance_lock(scan.path()).expect("scan lock");
    }
}
