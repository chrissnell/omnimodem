//! Phase 2 EXIT CRITERION: over gRPC, list a device, configure audio + PTT on
//! it, and transmit a PCM buffer — observing PTT key/unkey around audio that
//! actually plays, with NO mode attached. Deterministic backends (file audio +
//! MockPtt), so this runs in CI; the manual procedure below runs the identical
//! RPC sequence against real hardware.

// MANUAL REAL-HARDWARE GATE (run on a host with a sound card + radio):
//   1. cargo run -p omnimodemd   (Phase-1 daemon over UDS)
//   2. With grpcurl or the reference client:
//        a. ListDevices -> confirm the USB sound card appears with a stable
//           DeviceId (usb:VVVV:PPPP:serial or alsa:<card>).
//        b. ConfigureAudio { channel:0, device_id:<from a>, sample_rate:48000 }
//           -> actual_sample_rate is 48000 (or the card's real ceiling).
//        c. ConfigurePtt { channel:0, device_id:<ptt adapter>,
//           method:SERIAL_RTS|CM108|GPIO, node:/dev/..., invert:false }.
//        d. KeyPtt { channel:0, keyed:true } -> radio's TX LED lights; the
//           SubscribeEvents stream shows PttState{keyed:true}. KeyPtt{keyed:false}
//           drops it.
//        e. Transmit a short WAV/PCM buffer -> hear it on a second receiver with
//           PTT asserted only for the buffer's duration; PttState toggles around it.
//   3. Unplug the PTT adapter mid-session -> a DeviceDeparted event fires and the
//      next KeyPtt returns failed_precondition (eviction worked).
// Pass criterion: the radio keys, audio plays, PTT releases after drain, and
// hotplug eviction fires -- all over gRPC, with no DSP mode attached.

use omnimodemd::proto::event::Kind;
use omnimodemd::proto::modem_control_client::ModemControlClient;
use omnimodemd::proto::{
    ConfigureAudioRequest, ConfigureChannelRequest, ConfigurePttRequest, ListDevicesRequest,
    PttMethod, SubscribeRequest, TransmitRequest,
};
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

#[tokio::test]
async fn phase2_exit_criterion_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("omnimodem.sock");
    let db = dir.path().join("omnimodem.sqlite");

    let sock_srv = sock.clone();
    tokio::spawn(async move {
        omnimodemd::serve_uds_phase2_for_test(&db, &sock_srv).await.unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let mut client = connect(sock).await;

    // (1) A device is enumerable with a stable id.
    let devs = client.list_devices(ListDevicesRequest {}).await.unwrap().into_inner();
    assert!(!devs.devices.is_empty());
    let device_id = devs.devices[0].device_id.clone();

    // Configure the channel, then bind audio + PTT to the device.
    client
        .configure_channel(ConfigureChannelRequest { channel: 0, name: "vfo-a".into(), mode: "none".into(), mode_params: None })
        .await
        .unwrap();
    let audio = client
        .configure_audio(ConfigureAudioRequest {
            channel: 0,
            device_id: device_id.clone(),
            sample_rate: 48_000,
            fanout: 1,
            tx_device_id: String::new(), // single-rig: TX follows the capture device
            tx_sample_rate: 0,
        })
        .await
        .unwrap()
        .into_inner();
    assert!(audio.actual_sample_rate <= 48_000 && audio.actual_sample_rate > 0);

    client
        .configure_ptt(ConfigurePttRequest {
            channel: 0,
            device_id,
            method: PttMethod::SerialRts as i32,
            node: "/dev/ttyUSB-mock".into(),
            pin_or_line: 0,
            invert: false,
        })
        .await
        .unwrap();

    // (2) Subscribe, then transmit a PCM buffer; observe PTT keyed then released
    // and TransmitStarted/Complete — no mode involved.
    let mut stream = client.subscribe_events(SubscribeRequest {}).await.unwrap().into_inner();
    let _snapshot = stream.next().await.unwrap().unwrap();

    let pcm: Vec<u8> = (0..960).flat_map(|i| (i as i16).to_le_bytes()).collect();
    client.transmit(TransmitRequest { channel: 0, payload: pcm }).await.unwrap();

    let (mut keyed, mut unkeyed, mut started, mut completed) = (false, false, false, false);
    while !(started && completed && keyed && unkeyed) {
        match stream.next().await.unwrap().unwrap().kind.unwrap() {
            Kind::PttState(s) if s.keyed => keyed = true,
            Kind::PttState(s) if !s.keyed => unkeyed = true,
            Kind::TransmitStarted(_) => started = true,
            Kind::TransmitComplete(_) => completed = true,
            _ => {}
        }
    }
    assert!(keyed && unkeyed && started && completed);
}
