//! Phase 1 EXIT CRITERION (design doc): over gRPC, a client configures a
//! virtual channel, subscribes to events (receiving a snapshot), and completes
//! a fake transmit round-trip end-to-end — with no audio devices or DSP, over
//! the authorized UDS transport.

use omnimodemd::proto::event::Kind;
use omnimodemd::proto::modem_control_client::ModemControlClient;
use omnimodemd::proto::{ConfigureChannelRequest, SubscribeRequest, TransmitRequest};
use tokio::net::UnixStream;
use tokio_stream::StreamExt;
use tonic::transport::{Endpoint, Uri};
use tower::service_fn;

async fn connect(sock: std::path::PathBuf) -> ModemControlClient<tonic::transport::Channel> {
    let channel = Endpoint::try_from("http://[::]:50051")
        .unwrap()
        .connect_with_connector(service_fn(move |_: Uri| {
            let sock = sock.clone();
            async move { Ok::<_, std::io::Error>(hyper_util::rt::TokioIo::new(UnixStream::connect(sock).await?)) }
        }))
        .await
        .unwrap();
    ModemControlClient::new(channel)
}

#[tokio::test]
async fn phase1_exit_criterion_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("omnimodem.sock");
    let db = dir.path().join("omnimodem.sqlite");

    let sock_srv = sock.clone();
    tokio::spawn(async move {
        omnimodemd::serve_uds_authz_for_test(&db, &sock_srv).await.unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Connecting as the same uid that runs the test passes SO_PEERCRED.
    let mut client = connect(sock).await;

    // (1) Configure a virtual channel.
    let cfg = client
        .configure_channel(ConfigureChannelRequest { channel: 0, name: "vfo-a".into(), mode: "none".into(), mode_params: None })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(cfg.channel, 0);

    // (2) Subscribe; the first event is the state snapshot.
    let mut stream = client.subscribe_events(SubscribeRequest {}).await.unwrap().into_inner();
    match stream.next().await.unwrap().unwrap().kind.unwrap() {
        Kind::Snapshot(s) => {
            assert_eq!(s.channels.len(), 1);
            assert_eq!(s.channels[0].name, "vfo-a");
            assert_eq!(s.channels[0].device_id, "virtual:virtual:0");
        }
        other => panic!("expected snapshot first, got {other:?}"),
    }

    // (3) Drive the fake transmit round-trip: unary ack...
    let tx = client
        .transmit(TransmitRequest { channel: 0, payload: b"hello".to_vec() })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(tx.transmit_id, 1);

    // ...and the streamed Started + Complete for the same transmit id.
    let mut started = None;
    let mut completed = None;
    while started.is_none() || completed.is_none() {
        match stream.next().await.unwrap().unwrap().kind.unwrap() {
            Kind::TransmitStarted(s) => started = Some(s.transmit_id),
            Kind::TransmitComplete(c) => completed = Some(c.transmit_id),
            _ => {}
        }
    }
    assert_eq!(started, Some(1));
    assert_eq!(completed, Some(1));
}
