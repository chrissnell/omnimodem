//! Omnimodem daemon library surface.
//!
//! The binary in `main.rs` is a thin wrapper; everything testable lives here so
//! integration tests in `tests/` can spawn the server in-process.

/// Crate version, surfaced to clients in the gRPC handshake metadata.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod proto;

pub mod config;

pub mod ids;

pub mod audio;

pub mod persist;

pub mod device;

pub mod ptt;

pub mod supervisor;

pub mod mode;

pub mod core;

pub mod metrics;

pub mod grpc;

pub mod authz;

pub mod kiss;

#[cfg(not(test))]
use std::path::Path;

/// Build the production core: a real PTT registry, the cpal/nusb enumerator,
/// and a cpal-backend factory that resolves a `DeviceId` to its live device.
/// Excluded from unit-test builds, where `RealEnumerator`/`cpal_backend` (which
/// link ALSA) are compiled out; integration tests and the binary build the lib
/// without `cfg(test)`, so this is available there.
#[cfg(not(test))]
pub fn production_core(
    store: persist::Store,
    registered_devices: Vec<device::DeviceDescriptor>,
) -> Result<(core::CoreHandle, std::thread::JoinHandle<()>), persist::StoreError> {
    use audio::backend::{AudioBackend, NullBackend};
    use device::{DeviceDescriptor, RealEnumerator};

    let supervisor = supervisor::Supervisor::new(store, Box::new(ptt::registry::RealOpener))?;
    let enumerator = Box::new(RealEnumerator);
    let factory: core::AudioBackendFactory =
        Box::new(|desc: &DeviceDescriptor| -> Box<dyn AudioBackend> {
            // An rtl_tcp SDR endpoint is bound by its host:port identity, not by
            // hardware enumeration. The core injects the per-channel telemetry
            // sink + SdrControl via attach_sdr_context before open_capture.
            if let ids::DeviceId::RtlTcp { host, port } = &desc.id {
                return Box::new(audio::sdr::RtlTcpBackend::new(host.clone(), *port));
            }
            for (id, backend) in audio::cpal_backend::enumerate_default_host() {
                if id == desc.id {
                    return Box::new(backend);
                }
            }
            Box::new(NullBackend::new(audio::MAX_SAMPLE_RATE))
        });
    Ok(core::spawn_with_devices(supervisor, enumerator, factory, registered_devices))
}

/// Spawn the full control plane (core + gRPC) listening on a UDS at `path`.
/// Authz is applied by `authz::serve_uds`; this no-authz variant exists for
/// unary/subscribe tests.
#[cfg(not(test))]
pub async fn serve_uds_no_authz(
    db_path: &Path,
    sock_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    use grpc::ControlService;
    use proto::modem_control_server::ModemControlServer;
    use tokio_stream::wrappers::UnixListenerStream;

    let store = persist::Store::open(db_path)?;
    let (core, _join) = production_core(store, Vec::new())?;
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

/// Spawn the control plane over an AUTHORIZED UDS (SO_PEERCRED enforced).
/// Used by the e2e exit-criterion test and by anything that wants the real
/// production transport in-process.
#[cfg(not(test))]
pub async fn serve_uds_authz_for_test(
    db_path: &Path,
    sock_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let store = persist::Store::open(db_path)?;
    let (core, _join) = production_core(store, Vec::new())?;
    let svc = grpc::ControlService::new(core);
    authz::serve_uds(svc, sock_path).await
}

/// A `DriverOpener` that hands out `MockPtt` drivers, for the Phase-2
/// exit-criterion test (keys/unkeys without hardware).
#[cfg(not(test))]
struct MockOpener;

#[cfg(not(test))]
impl ptt::registry::DriverOpener for MockOpener {
    fn open(
        &self,
        _cfg: &ptt::registry::PttConfig,
    ) -> Result<Box<dyn ptt::PttDriver>, ptt::PttError> {
        Ok(Box::new(ptt::none::MockPtt::new()))
    }
}

/// Spawn the control plane with deterministic audio + PTT backends for the
/// Phase-2 exit-criterion test: a `FakeEnumerator` advertising one loopback
/// device, a `FileBackend` audio factory, and a `MockPtt` opener — so the full
/// ListDevices -> ConfigureAudio -> ConfigurePtt -> Transmit sequence runs over
/// the authorized UDS surface without any hardware.
#[cfg(not(test))]
pub async fn serve_uds_phase2_for_test(
    db_path: &Path,
    sock_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    use audio::backend::AudioBackend;
    use audio::file::FileBackend;
    use device::enumerate::FakeEnumerator;
    use device::DeviceDescriptor;
    use ids::DeviceId;

    let store = persist::Store::open(db_path)?;
    let supervisor = supervisor::Supervisor::new(store, Box::new(MockOpener))?;

    let loopback = DeviceDescriptor {
        id: DeviceId::AlsaCard { card_name: "loopback".into() },
        label: "loopback".into(),
        has_capture: true,
        has_playback: true,
    };
    let enumerator = Box::new(FakeEnumerator::new(vec![loopback]));
    let factory: core::AudioBackendFactory =
        Box::new(|_d| Box::new(FileBackend::from_samples(vec![], 48_000)) as Box<dyn AudioBackend>);

    let (core, _join) = core::spawn(supervisor, enumerator, factory);
    let svc = grpc::ControlService::new(core);
    authz::serve_uds(svc, sock_path).await
}
