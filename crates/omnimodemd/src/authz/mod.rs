//! Transport selection and authorization for the control plane.

pub mod tls;
pub mod uds;

use crate::grpc::ControlService;
use crate::proto::modem_control_server::ModemControlServer;
use tokio::net::UnixListener;
use tokio_stream::wrappers::UnixListenerStream;
use tonic::transport::server::UdsConnectInfo;
use tonic::{Request, Status};

/// Which transport the daemon binds. UDS is the secure default; routable binds
/// require mTLS (Phase 1 only stubs the hook — see `tls`).
pub enum Transport {
    /// Unix domain socket (default). Peer-uid checked via SO_PEERCRED.
    Uds { path: std::path::PathBuf },
    /// Loopback TCP. NOTE: exposes EVERY local user — no peer isolation.
    TcpLoopback { addr: std::net::SocketAddr },
    /// Routable bind. Requires mTLS, which is not implemented in Phase 1.
    Routable { addr: std::net::SocketAddr },
}

/// Serve the control plane over a UDS with SO_PEERCRED authorization.
pub async fn serve_uds(
    svc: ControlService,
    path: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let _ = std::fs::remove_file(path);
    let listener = UnixListener::bind(path)?;
    uds::harden_socket_mode(path)?;
    let incoming = UnixListenerStream::new(listener);

    let server_uid = uds::current_uid();
    let interceptor = move |req: Request<()>| -> Result<Request<()>, Status> {
        match req.extensions().get::<UdsConnectInfo>() {
            Some(info) => {
                let peer_uid = info
                    .peer_cred
                    .ok_or_else(|| Status::unauthenticated("no peer credentials"))?
                    .uid();
                if uds::peer_uid_allowed(server_uid, peer_uid) {
                    Ok(req)
                } else {
                    Err(Status::unauthenticated(format!(
                        "peer uid {peer_uid} not authorized"
                    )))
                }
            }
            None => Err(Status::unauthenticated("no connection info")),
        }
    };

    tonic::transport::Server::builder()
        .add_service(ModemControlServer::with_interceptor(svc, interceptor))
        .serve_with_incoming(incoming)
        .await?;
    Ok(())
}

/// Validate a chosen transport before binding. Returns a warning string for
/// transports that are allowed-but-risky, or an error for disallowed ones.
pub fn validate_transport(t: &Transport) -> Result<Option<String>, tls::TlsError> {
    match t {
        Transport::Uds { .. } => Ok(None),
        Transport::TcpLoopback { .. } => Ok(Some(
            "loopback TCP exposes every local user; no per-peer authorization is enforced"
                .to_string(),
        )),
        // Fails closed: routable requires mTLS, unimplemented in Phase 1.
        Transport::Routable { .. } => {
            tls::routable_tls_config()?;
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uds_is_clean() {
        let t = Transport::Uds { path: "/tmp/x.sock".into() };
        assert_eq!(validate_transport(&t).unwrap(), None);
    }

    #[test]
    fn loopback_warns() {
        let t = Transport::TcpLoopback { addr: "127.0.0.1:9000".parse().unwrap() };
        assert!(validate_transport(&t).unwrap().is_some());
    }

    #[test]
    fn routable_fails_closed() {
        let t = Transport::Routable { addr: "0.0.0.0:9000".parse().unwrap() };
        assert!(validate_transport(&t).is_err());
    }
}
