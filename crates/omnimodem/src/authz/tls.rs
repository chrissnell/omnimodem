//! mTLS for routable binds (design §"Local authorization": mTLS + per-method
//! authz is MANDATORY for any routable interface). Phases 1–4 only stubbed this
//! to fail closed; Phase 5 loads a server cert/key + a client-CA bundle and
//! builds a tonic `ServerTlsConfig` that REQUIRES client certificates. A routable
//! bind still fails closed if any of the three PEM paths is missing or unreadable.

use tonic::transport::{Identity, ServerTlsConfig};

#[derive(Debug, thiserror::Error)]
pub enum TlsError {
    #[error("routable bind requires OMNIMODEM_TLS_CERT, _KEY and _CLIENT_CA to be set")]
    NotConfigured,
    #[error("reading TLS material: {0}")]
    Io(#[from] std::io::Error),
}

/// Paths to the PEM material for a routable mTLS bind.
pub struct TlsPaths {
    pub server_cert: std::path::PathBuf,
    pub server_key: std::path::PathBuf,
    pub client_ca: std::path::PathBuf,
}

impl TlsPaths {
    /// Read the three paths from the environment; `None` if any is unset.
    pub fn from_env() -> Option<TlsPaths> {
        Some(TlsPaths {
            server_cert: std::env::var_os("OMNIMODEM_TLS_CERT")?.into(),
            server_key: std::env::var_os("OMNIMODEM_TLS_KEY")?.into(),
            client_ca: std::env::var_os("OMNIMODEM_TLS_CLIENT_CA")?.into(),
        })
    }
}

/// Build a tonic mTLS server config that REQUIRES a client cert chained to
/// `client_ca`. Fails closed (`NotConfigured`) if `paths` is `None`.
pub fn routable_tls_config(paths: Option<TlsPaths>) -> Result<ServerTlsConfig, TlsError> {
    let p = paths.ok_or(TlsError::NotConfigured)?;
    let cert = std::fs::read(&p.server_cert)?;
    let key = std::fs::read(&p.server_key)?;
    let client_ca = std::fs::read(&p.client_ca)?;
    Ok(ServerTlsConfig::new()
        .identity(Identity::from_pem(cert, key))
        .client_ca_root(tonic::transport::Certificate::from_pem(client_ca)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unconfigured_fails_closed() {
        assert!(matches!(routable_tls_config(None), Err(TlsError::NotConfigured)));
    }

    #[test]
    fn missing_file_is_an_io_error_not_a_silent_open() {
        let paths = TlsPaths {
            server_cert: "/nonexistent/cert.pem".into(),
            server_key: "/nonexistent/key.pem".into(),
            client_ca: "/nonexistent/ca.pem".into(),
        };
        assert!(matches!(routable_tls_config(Some(paths)), Err(TlsError::Io(_))));
    }
}
