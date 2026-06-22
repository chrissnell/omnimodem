//! The `ModemControl` gRPC service implementation (unary handlers here;
//! `SubscribeEvents` lives in `subscribe.rs` and is added via the same struct).

use crate::core::command::Command;
use crate::core::CoreHandle;
use crate::grpc::convert;
use crate::grpc::convert::{core_error_to_status, snapshot_to_proto};
use crate::ids::{ChannelId, DeviceId};
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
    #[allow(clippy::result_large_err)] // `Status` is the gRPC-boundary error type
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

    async fn list_devices(
        &self,
        _request: Request<proto::ListDevicesRequest>,
    ) -> Result<Response<proto::ListDevicesResponse>, Status> {
        let (tx, rx) = oneshot::channel();
        self.send_command(Command::ListDevices { reply: tx })?;
        let devices = rx.await.map_err(|_| Status::unavailable("core dropped reply"))?;
        Ok(Response::new(proto::ListDevicesResponse {
            devices: devices.iter().map(convert::device_descriptor_to_proto).collect(),
        }))
    }

    async fn configure_audio(
        &self,
        request: Request<proto::ConfigureAudioRequest>,
    ) -> Result<Response<proto::ConfigureAudioResponse>, Status> {
        let req = request.into_inner();
        if req.device_id.is_empty() {
            return Err(Status::invalid_argument("device_id must not be empty"));
        }
        let device_id = DeviceId::parse(&req.device_id)
            .ok_or_else(|| Status::invalid_argument(format!("unparseable device_id {}", req.device_id)))?;
        let (tx, rx) = oneshot::channel();
        self.send_command(Command::ConfigureAudio {
            id: ChannelId(req.channel),
            device_id,
            sample_rate: req.sample_rate,
            fanout: req.fanout,
            reply: tx,
        })?;
        let actual = rx
            .await
            .map_err(|_| Status::unavailable("core dropped reply"))?
            .map_err(core_error_to_status)?;
        Ok(Response::new(proto::ConfigureAudioResponse { actual_sample_rate: actual }))
    }

    async fn configure_ptt(
        &self,
        request: Request<proto::ConfigurePttRequest>,
    ) -> Result<Response<proto::ConfigurePttResponse>, Status> {
        let req = request.into_inner();
        let ptt = convert::proto_ptt_to_config(&req)?;
        let (tx, rx) = oneshot::channel();
        self.send_command(Command::ConfigurePtt { id: ChannelId(req.channel), ptt, reply: tx })?;
        rx.await
            .map_err(|_| Status::unavailable("core dropped reply"))?
            .map_err(core_error_to_status)?;
        Ok(Response::new(proto::ConfigurePttResponse {}))
    }

    async fn key_ptt(
        &self,
        request: Request<proto::KeyPttRequest>,
    ) -> Result<Response<proto::KeyPttResponse>, Status> {
        let req = request.into_inner();
        let (tx, rx) = oneshot::channel();
        self.send_command(Command::KeyPtt {
            channel: ChannelId(req.channel),
            keyed: req.keyed,
            reply: tx,
        })?;
        rx.await
            .map_err(|_| Status::unavailable("core dropped reply"))?
            .map_err(core_error_to_status)?;
        Ok(Response::new(proto::KeyPttResponse {}))
    }

    async fn suggest_udev_rule(
        &self,
        request: Request<proto::SuggestUdevRuleRequest>,
    ) -> Result<Response<proto::SuggestUdevRuleResponse>, Status> {
        let req = request.into_inner();
        if req.device_id.is_empty() {
            return Err(Status::invalid_argument("device_id must not be empty"));
        }
        let device_id = DeviceId::parse(&req.device_id)
            .ok_or_else(|| Status::invalid_argument(format!("unparseable device_id {}", req.device_id)))?;
        let (tx, rx) = oneshot::channel();
        self.send_command(Command::SuggestUdevRule { device_id, reply: tx })?;
        let (rule, instructions) = rx
            .await
            .map_err(|_| Status::unavailable("core dropped reply"))?
            .map_err(core_error_to_status)?;
        Ok(Response::new(proto::SuggestUdevRuleResponse { rule, instructions }))
    }

    async fn get_metrics(
        &self,
        request: Request<proto::GetMetricsRequest>,
    ) -> Result<Response<proto::GetMetricsResponse>, Status> {
        let req = request.into_inner();
        let channel = (req.channel != 0).then_some(ChannelId(req.channel));
        let (tx, rx) = oneshot::channel();
        self.send_command(Command::GetMetrics { channel, reply: tx })?;
        let snaps = rx.await.map_err(|_| Status::unavailable("core dropped reply"))?;
        Ok(Response::new(proto::GetMetricsResponse {
            metrics: snaps.iter().map(convert::metrics_to_proto).collect(),
        }))
    }

    async fn acquire_tx_lease(
        &self,
        request: Request<proto::TxLeaseRequest>,
    ) -> Result<Response<proto::TxLeaseResponse>, Status> {
        let channel = ChannelId(request.into_inner().channel);
        let (tx, rx) = oneshot::channel();
        self.send_command(Command::AcquireTxLease { channel, reply: tx })?;
        let grant = rx
            .await
            .map_err(|_| Status::unavailable("core dropped reply"))?
            .map_err(core_error_to_status)?;
        Ok(Response::new(proto::TxLeaseResponse {
            granted: grant.granted,
            held_by: grant.held_by.map(|c| c.0).unwrap_or(0),
        }))
    }

    async fn release_tx_lease(
        &self,
        request: Request<proto::TxLeaseRequest>,
    ) -> Result<Response<proto::TxLeaseResponse>, Status> {
        let channel = ChannelId(request.into_inner().channel);
        let (tx, rx) = oneshot::channel();
        self.send_command(Command::ReleaseTxLease { channel, reply: tx })?;
        rx.await
            .map_err(|_| Status::unavailable("core dropped reply"))?
            .map_err(core_error_to_status)?;
        Ok(Response::new(proto::TxLeaseResponse { granted: true, held_by: 0 }))
    }

    type SubscribeEventsStream = crate::grpc::subscribe::EventStream;

    async fn subscribe_events(
        &self,
        request: Request<proto::SubscribeRequest>,
    ) -> Result<Response<Self::SubscribeEventsStream>, Status> {
        crate::grpc::subscribe::subscribe(self, request).await
    }
}
