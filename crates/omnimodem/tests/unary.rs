//! Integration: unary RPCs over an in-process UDS server (no authz).

use omnimodem::proto::modem_control_client::ModemControlClient;
use omnimodem::proto::{ConfigureChannelRequest, GetStateRequest, TransmitRequest};
use tokio::net::UnixStream;
use tonic::transport::{Endpoint, Uri};
use tower::service_fn;

async fn connect(sock: std::path::PathBuf) -> ModemControlClient<tonic::transport::Channel> {
    // The URI authority is ignored; the connector dials the UDS path.
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
async fn configure_get_transmit_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("omnimodem.sock");
    let db = dir.path().join("omnimodem.sqlite");

    let sock_srv = sock.clone();
    tokio::spawn(async move {
        omnimodem::serve_uds_no_authz(&db, &sock_srv).await.unwrap();
    });
    // Give the server a moment to bind.
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;

    let mut client = connect(sock).await;

    // Configure.
    let resp = client
        .configure_channel(ConfigureChannelRequest {
            channel: 0,
            name: "vfo-a".into(),
            mode: "none".into(),
            mode_params: None,
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(resp.channel, 0);

    // GetState reflects it.
    let state = client.get_state(GetStateRequest {}).await.unwrap().into_inner();
    assert_eq!(state.channels.len(), 1);
    assert_eq!(state.channels[0].name, "vfo-a");
    assert_eq!(state.channels[0].device_id, "virtual:virtual:0");

    // Transmit returns a monotonic id.
    let tx = client
        .transmit(TransmitRequest { channel: 0, payload: vec![1, 2, 3] })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(tx.transmit_id, 1);

    // Empty name is rejected.
    let err = client
        .configure_channel(ConfigureChannelRequest { channel: 1, name: "".into(), mode: "none".into(), mode_params: None, ..Default::default() })
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::InvalidArgument);
}
