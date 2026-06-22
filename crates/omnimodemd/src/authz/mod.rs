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
// The peer-cred interceptor returns `Result<_, Status>`; `Status` is the
// gRPC-boundary error type, so the large-err lint does not apply.
#[allow(clippy::result_large_err)]
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
        // Fails closed: routable requires mTLS material (cert/key/client-CA) in
        // the environment. Building the config validates it can be loaded.
        Transport::Routable { .. } => {
            tls::routable_tls_config(tls::TlsPaths::from_env())?;
            Ok(None)
        }
    }
}

/// SHA-256 fingerprint (lowercase hex) of a DER-encoded certificate.
fn cert_fingerprint(der: &[u8]) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, der);
    digest.as_ref().iter().fold(String::with_capacity(64), |mut s, b| {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// Per-peer authorization for the routable mTLS path, symmetric with the UDS
/// SO_PEERCRED interceptor but keyed on the client certificate. The TLS layer
/// already requires a cert chained to the configured client CA; this interceptor
/// (a) fails closed if no verified peer cert is present, and (b) if
/// `OMNIMODEM_TLS_ALLOWED_FINGERPRINTS` is set (comma-separated SHA-256 hex),
/// restricts access to those specific client certs. With the env var unset, any
/// CA-verified client is allowed — the client CA is the trust boundary.
#[allow(clippy::result_large_err)]
fn routable_peer_authz(req: Request<()>) -> Result<Request<()>, Status> {
    use tonic::transport::server::{TcpConnectInfo, TlsConnectInfo};
    let certs = req
        .extensions()
        .get::<TlsConnectInfo<TcpConnectInfo>>()
        .and_then(|i| i.peer_certs())
        .ok_or_else(|| Status::unauthenticated("client certificate required"))?;
    let leaf = certs
        .first()
        .ok_or_else(|| Status::unauthenticated("empty client certificate chain"))?;
    if let Ok(allow) = std::env::var("OMNIMODEM_TLS_ALLOWED_FINGERPRINTS") {
        let fp = cert_fingerprint(leaf.as_ref());
        let permitted = allow.split(',').map(|s| s.trim().to_ascii_lowercase()).any(|a| a == fp);
        if !permitted {
            return Err(Status::permission_denied(format!(
                "client certificate {fp} not in OMNIMODEM_TLS_ALLOWED_FINGERPRINTS"
            )));
        }
    }
    Ok(req)
}

/// Serve the control plane over a routable TCP interface under mTLS. Client
/// certificates are REQUIRED (the config is built from the cert/key/client-CA in
/// the environment); the bind fails closed if any TLS material is absent. The
/// `routable_peer_authz` interceptor then enforces per-peer authorization on the
/// verified client cert — symmetric with the UDS path's SO_PEERCRED check.
pub async fn serve_routable(
    svc: ControlService,
    addr: std::net::SocketAddr,
) -> Result<(), Box<dyn std::error::Error>> {
    let tls = tls::routable_tls_config(tls::TlsPaths::from_env())?;
    tonic::transport::Server::builder()
        .tls_config(tls)?
        .add_service(ModemControlServer::with_interceptor(svc, routable_peer_authz))
        .serve(addr)
        .await?;
    Ok(())
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

    #[test]
    fn cert_fingerprint_is_deterministic_lowercase_hex_sha256() {
        let fp = cert_fingerprint(b"some-der-bytes");
        assert_eq!(fp.len(), 64, "SHA-256 hex is 64 chars");
        assert!(fp.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert_eq!(fp, cert_fingerprint(b"some-der-bytes"));
        assert_ne!(fp, cert_fingerprint(b"other-der-bytes"));
    }
}
