//! Integration: snapshot-on-subscribe + live event delivery.

use omnimodem::proto::event::Kind;
use omnimodem::proto::modem_control_client::ModemControlClient;
use omnimodem::proto::{ConfigureChannelRequest, SubscribeRequest, TransmitRequest};
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
async fn snapshot_then_live_events() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("omnimodem.sock");
    let db = dir.path().join("omnimodem.sqlite");

    let sock_srv = sock.clone();
    tokio::spawn(async move {
        omnimodem::serve_uds_no_authz(&db, &sock_srv).await.unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;

    let mut client = connect(sock).await;

    // Pre-configure a channel so the snapshot is non-empty.
    client
        .configure_channel(ConfigureChannelRequest { channel: 0, name: "vfo-a".into(), mode: "none".into(), mode_params: None, ..Default::default() })
        .await
        .unwrap();

    // Subscribe; first message must be the snapshot.
    let mut stream = client.subscribe_events(SubscribeRequest {}).await.unwrap().into_inner();
    let first = stream.next().await.unwrap().unwrap();
    match first.kind.unwrap() {
        Kind::Snapshot(s) => {
            assert_eq!(s.channels.len(), 1);
            assert_eq!(s.channels[0].name, "vfo-a");
        }
        other => panic!("expected snapshot first, got {other:?}"),
    }

    // Now transmit and observe the live Started/Complete on the stream.
    client.transmit(TransmitRequest { channel: 0, payload: vec![9] }).await.unwrap();

    let mut saw_started = false;
    let mut saw_complete = false;
    while !(saw_started && saw_complete) {
        let ev = stream.next().await.unwrap().unwrap();
        match ev.kind.unwrap() {
            Kind::TransmitStarted(s) => { assert_eq!(s.channel, 0); saw_started = true; }
            Kind::TransmitComplete(c) => { assert_eq!(c.channel, 0); saw_complete = true; }
            // ChannelConfigured from any concurrent config is fine to ignore.
            _ => {}
        }
    }
    assert!(saw_started && saw_complete);
}
