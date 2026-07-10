//! Integration: the SDR (rtl_tcp) control RPCs over an in-process UDS server,
//! driven against a fake `rtl_tcp` endpoint. Covers the full wiring — proto →
//! Command → core → SdrControl — plus the `SdrState` broadcast, per-mode demod
//! selectability (Phase B), and the Phase-C dongle extras (bias-tee,
//! direct-sampling).

use omnimodemd::proto::event::Kind;
use omnimodemd::proto::modem_control_client::ModemControlClient;
use omnimodemd::proto::{
    ConfigureAudioRequest, ConfigureChannelRequest, ConfigureSdrRequest, DemodMode,
    GetSdrCapsRequest, SetSdrGainRequest, SetSdrTuneRequest, SubscribeRequest,
};
use std::io::{Read, Write};
use std::net::TcpListener;
use tokio::net::UnixStream;
use tokio_stream::StreamExt;
use tonic::transport::{Endpoint, Uri};
use tower::service_fn;

async fn connect(sock: std::path::PathBuf) -> ModemControlClient<tonic::transport::Channel> {
    let channel = Endpoint::try_from("http://[::]:50051")
        .unwrap()
        .connect_with_connector(service_fn(move |_: Uri| {
            let sock = sock.clone();
            async move {
                Ok::<_, std::io::Error>(hyper_util::rt::TokioIo::new(
                    UnixStream::connect(sock).await?,
                ))
            }
        }))
        .await
        .unwrap();
    ModemControlClient::new(channel)
}

/// A fake `rtl_tcp` server: write the 12-byte header (advertising an R820T), drain
/// client tuning commands, dribble a little IQ, then keep the socket open so the
/// binding stays live for the duration of the test. Returns the bound port.
fn spawn_fake_rtl_tcp() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let (mut sock, _) = listener.accept().unwrap();
        let mut header = [0u8; 12];
        header[0..4].copy_from_slice(b"RTL0");
        header[4..8].copy_from_slice(&5u32.to_be_bytes()); // R820T
        header[8..12].copy_from_slice(&29u32.to_be_bytes());
        sock.write_all(&header).unwrap();
        // Drain client commands so its writes never block.
        let mut drain = sock.try_clone().unwrap();
        std::thread::spawn(move || {
            let mut sink = [0u8; 64];
            while let Ok(n) = drain.read(&mut sink) {
                if n == 0 {
                    break;
                }
            }
        });
        // A trickle of IQ, then hold the connection open until the test drops.
        let iq = vec![127u8; 4096];
        let _ = sock.write_all(&iq);
        std::thread::sleep(std::time::Duration::from_secs(10));
    });
    port
}

async fn start_server() -> (ModemControlClient<tonic::transport::Channel>, u16) {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("omnimodem.sock");
    let db = dir.path().join("omnimodem.sqlite");
    // Keep the tempdir alive for the whole test.
    std::mem::forget(dir);

    let sock_srv = sock.clone();
    tokio::spawn(async move {
        omnimodemd::serve_uds_no_authz(&db, &sock_srv).await.unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;

    let rtl_port = spawn_fake_rtl_tcp();
    let client = connect(sock).await;
    (client, rtl_port)
}

/// Configure a channel and bind it to the fake `rtltcp:` endpoint (RX-only).
async fn bind_sdr_channel(client: &mut ModemControlClient<tonic::transport::Channel>, port: u16) {
    client
        .configure_channel(ConfigureChannelRequest {
            channel: 0,
            name: "vfo-a".into(),
            mode: "none".into(),
            mode_params: None,
            ..Default::default()
        })
        .await
        .unwrap();
    client
        .configure_audio(ConfigureAudioRequest {
            channel: 0,
            device_id: format!("rtltcp:127.0.0.1:{port}"),
            sample_rate: 48_000,
            tx_device_id: String::new(),
            ..Default::default()
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn sdr_control_roundtrip() {
    let (mut client, port) = start_server().await;
    bind_sdr_channel(&mut client, port).await;

    // Caps come from the tuner header (R820T) + the static per-tuner tables.
    let caps = client
        .get_sdr_caps(GetSdrCapsRequest { channel: 0 })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(caps.tuner, "R820T");
    assert_eq!(caps.gains_db.len(), 29);
    assert!(caps.freq_min_hz < caps.freq_max_hz);
    assert!(caps.sample_rates.contains(&240_000));
    // R820T supports both bias-tee (tuner feature) and direct-sampling (ADC).
    assert!(caps.bias_tee_supported);
    assert!(caps.direct_sampling_supported);
    let vhf_min = caps.freq_min_hz; // remember the non-direct-sampling floor

    // Absolute tune → split into center + NCO offset; the effective freq matches.
    let tune = client
        .set_sdr_tune(SetSdrTuneRequest { channel: 0, freq_hz: 144_390_000.0 })
        .await
        .unwrap()
        .into_inner();
    assert!((tune.actual_freq_hz - 144_390_000.0).abs() < 1.0);
    assert!((tune.center_hz + tune.offset_hz - 144_390_000.0).abs() < 1.0);

    // Manual gain snaps to the nearest R820T table entry (20.0 → 19.7).
    let gain = client
        .set_sdr_gain(SetSdrGainRequest { channel: 0, auto: false, gain_db: 20.0 })
        .await
        .unwrap()
        .into_inner();
    assert!((gain.actual_gain_db - 19.7).abs() < 0.01);

    // NBFM config succeeds and echoes the effective capture rate.
    let cfg = client
        .configure_sdr(ConfigureSdrRequest {
            channel: 0,
            capture_rate: 0, // unchanged
            demod_mode: DemodMode::DemodNbfm as i32,
            squelch_db: -30.0,
            ppm: 1,
            bias_tee: false,
            direct_sampling: false,
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(cfg.actual_capture_rate, 240_000);

    // Every demod mode is now selectable end-to-end (Phase B): AM/WFM/USB/LSB each
    // round-trip through configure_sdr and echo the effective capture rate.
    for mode in [
        DemodMode::DemodAm,
        DemodMode::DemodWfm,
        DemodMode::DemodUsb,
        DemodMode::DemodLsb,
    ] {
        let cfg = client
            .configure_sdr(ConfigureSdrRequest {
                channel: 0,
                capture_rate: 0,
                demod_mode: mode as i32,
                squelch_db: -30.0,
                ppm: 0,
                bias_tee: false,
                direct_sampling: false,
            })
            .await
            .unwrap_or_else(|e| panic!("{mode:?} should be selectable, got {e:?}"))
            .into_inner();
        assert_eq!(cfg.actual_capture_rate, 240_000, "{mode:?}");
    }

    // Phase C: bias-tee and direct-sampling now apply successfully, and enabling
    // direct-sampling widens the reported tunable range down to HF.
    let cfg = client
        .configure_sdr(ConfigureSdrRequest {
            channel: 0,
            capture_rate: 0,
            demod_mode: DemodMode::DemodNbfm as i32,
            squelch_db: -30.0,
            ppm: 0,
            bias_tee: true,
            direct_sampling: true,
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(cfg.actual_capture_rate, 240_000);
    let hf_caps = client
        .get_sdr_caps(GetSdrCapsRequest { channel: 0 })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(hf_caps.freq_min_hz, 0.0); // HF floor at DC
    assert!(hf_caps.freq_max_hz >= 28_800_000.0); // at least the ADC Nyquist
    assert!(hf_caps.freq_min_hz < vhf_min); // strictly wider than the VHF-only range

    // An undefined demod_mode code must be rejected, not silently folded to NBFM.
    let err = client
        .configure_sdr(ConfigureSdrRequest {
            channel: 0,
            capture_rate: 0,
            demod_mode: 99,
            squelch_db: -30.0,
            ppm: 0,
            bias_tee: false,
            direct_sampling: false,
        })
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::InvalidArgument);

    // An unsupported capture rate is a client error (INVALID_ARGUMENT), not INTERNAL.
    let err = client
        .configure_sdr(ConfigureSdrRequest {
            channel: 0,
            capture_rate: 12_345, // not in the tuner's rate table
            demod_mode: DemodMode::DemodNbfm as i32,
            squelch_db: -30.0,
            ppm: 0,
            bias_tee: false,
            direct_sampling: false,
        })
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::InvalidArgument);

    // Auto gain reports 0 dB (AGC engaged); it must not error.
    let gain = client
        .set_sdr_gain(SetSdrGainRequest { channel: 0, auto: true, gain_db: 0.0 })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(gain.actual_gain_db, 0.0);
}

#[tokio::test]
async fn sdr_control_requires_sdr_binding() {
    let (mut client, _port) = start_server().await;
    // A plain (non-SDR) channel: SDR RPCs must fail with FAILED_PRECONDITION.
    client
        .configure_channel(ConfigureChannelRequest {
            channel: 0,
            name: "vfo-a".into(),
            mode: "none".into(),
            mode_params: None,
            ..Default::default()
        })
        .await
        .unwrap();
    let err = client
        .set_sdr_tune(SetSdrTuneRequest { channel: 0, freq_hz: 144_390_000.0 })
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::FailedPrecondition);
}

#[tokio::test]
async fn tune_broadcasts_sdr_state() {
    let (mut client, port) = start_server().await;
    bind_sdr_channel(&mut client, port).await;

    // Subscribe (first message is the snapshot), then tune and observe SdrState.
    let mut stream = client
        .subscribe_events(SubscribeRequest {})
        .await
        .unwrap()
        .into_inner();
    let first = stream.next().await.unwrap().unwrap();
    assert!(matches!(first.kind.unwrap(), Kind::Snapshot(_)));

    client
        .set_sdr_tune(SetSdrTuneRequest { channel: 0, freq_hz: 144_390_000.0 })
        .await
        .unwrap();

    // The SdrState event must arrive with the effective frequency we tuned to.
    let mut saw_state = false;
    for _ in 0..50 {
        let ev = stream.next().await.unwrap().unwrap();
        if let Kind::SdrState(s) = ev.kind.unwrap() {
            assert_eq!(s.channel, 0);
            assert!((s.freq_hz - 144_390_000.0).abs() < 1.0);
            saw_state = true;
            break;
        }
    }
    assert!(saw_state, "expected an SdrState event after tuning");
}
