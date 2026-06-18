//! The `ModemControl` gRPC service implementation (unary handlers here;
//! `SubscribeEvents` lives in `subscribe.rs` and is added via the same struct).

use crate::core::command::Command;
use crate::core::CoreHandle;
use crate::grpc::convert::{core_error_to_status, snapshot_to_proto};
use crate::ids::ChannelId;
use crate::proto;
use crate::proto::modem_control_server::ModemControl;
use tokio::sync::oneshot;
use tonic::{Request, Response, Status};

/// Shared gRPC service state: just a handle to the sync core.
#[derive(Clone)]
pub struct ControlService {
    pub(crate) core: CoreHandle,
}

impl ControlService {
    pub fn new(core: CoreHandle) -> Self {
        ControlService { core }
    }

    /// Push a command into the core, mapping a full/closed queue to a status.
    pub(crate) fn send_command(&self, cmd: Command) -> Result<(), Status> {
        self.core
            .commands
            .try_send(cmd)
            .map_err(|_| Status::unavailable("core command queue full or closed"))
    }
}

#[tonic::async_trait]
impl ModemControl for ControlService {
    async fn configure_channel(
        &self,
        request: Request<proto::ConfigureChannelRequest>,
    ) -> Result<Response<proto::ConfigureChannelResponse>, Status> {
        let req = request.into_inner();
        if req.name.is_empty() {
            return Err(Status::invalid_argument("channel name must not be empty"));
        }
        let (tx, rx) = oneshot::channel();
        self.send_command(Command::ConfigureChannel {
            id: ChannelId(req.channel),
            name: req.name,
            mode: req.mode,
            reply: tx,
        })?;
        rx.await
            .map_err(|_| Status::unavailable("core dropped reply"))?
            .map_err(core_error_to_status)?;
        Ok(Response::new(proto::ConfigureChannelResponse { channel: req.channel }))
    }

    async fn get_state(
        &self,
        _request: Request<proto::GetStateRequest>,
    ) -> Result<Response<proto::ModemState>, Status> {
        let (tx, rx) = oneshot::channel();
        self.send_command(Command::GetState { reply: tx })?;
        let snap = rx.await.map_err(|_| Status::unavailable("core dropped reply"))?;
        Ok(Response::new(snapshot_to_proto(&snap)))
    }

    async fn transmit(
        &self,
        request: Request<proto::TransmitRequest>,
    ) -> Result<Response<proto::TransmitResponse>, Status> {
        let req = request.into_inner();
        let (tx, rx) = oneshot::channel();
        self.send_command(Command::Transmit {
            channel: ChannelId(req.channel),
            payload: req.payload,
            reply: tx,
        })?;
        let transmit_id = rx
            .await
            .map_err(|_| Status::unavailable("core dropped reply"))?
            .map_err(core_error_to_status)?;
        Ok(Response::new(proto::TransmitResponse { transmit_id: transmit_id.0 }))
    }

    // SubscribeEvents is implemented in subscribe.rs as part of this same impl
    // block via an `include!`-free split: see Task 9, which replaces this file's
    // impl with the full trait. (Until Task 9, the streaming method is stubbed
    // below so the trait is satisfied and unary tests can run.)
    type SubscribeEventsStream = std::pin::Pin<
        Box<dyn tokio_stream::Stream<Item = Result<proto::Event, Status>> + Send + 'static>,
    >;

    async fn subscribe_events(
        &self,
        _request: Request<proto::SubscribeRequest>,
    ) -> Result<Response<Self::SubscribeEventsStream>, Status> {
        Err(Status::unimplemented("SubscribeEvents lands in Task 9"))
    }
}
