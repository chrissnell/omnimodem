//! SubscribeEvents: snapshot-on-subscribe + dual-class backpressure fan-out.
//!
//! Backpressure policy (design, locked in Phase 1):
//!   * Frames are LOSSLESS — if this subscriber lags the frame ring, we end the
//!     stream with `resource_exhausted` rather than silently dropping a frame.
//!   * Telemetry is LOSSY — on lag we skip dropped intermediates and continue.
//!
//! Ordering: we subscribe to both broadcasts BEFORE asking the core for the
//! snapshot. Any event the core emits after our subscription is therefore
//! captured in our receivers, so the snapshot + live stream is at-least-once
//! (a change applied between subscribe and snapshot may appear in both; clients
//! treat the snapshot as authoritative and tolerate a duplicate follow-up).

use crate::core::command::Command;
use crate::grpc::convert::{frame_event_to_proto, snapshot_to_proto, telemetry_event_to_proto};
use crate::grpc::service::ControlService;
use crate::proto;
use std::pin::Pin;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::oneshot;
use tonic::{Request, Response, Status};

/// The boxed stream type returned to tonic.
pub type EventStream =
    Pin<Box<dyn tokio_stream::Stream<Item = Result<proto::Event, Status>> + Send + 'static>>;

pub async fn subscribe(
    svc: &ControlService,
    _request: Request<proto::SubscribeRequest>,
) -> Result<Response<EventStream>, Status> {
    // Subscribe FIRST so nothing emitted after this point is lost.
    let mut frame_rx = svc.core.frames.subscribe();
    let mut tele_rx = svc.core.telemetry.subscribe();

    // Then request the snapshot.
    let (tx, rx) = oneshot::channel();
    svc.send_command(Command::GetState { reply: tx })?;
    let snapshot = rx
        .await
        .map_err(|_| Status::unavailable("core dropped snapshot reply"))?;

    let stream = async_stream::try_stream! {
        // 1) Snapshot is always the first message.
        yield proto::Event {
            kind: Some(proto::event::Kind::Snapshot(snapshot_to_proto(&snapshot))),
        };

        // 2) Merge both classes until the client goes away or a frame is lost.
        // A frame-ring lag is fatal (LOSSLESS); record it and propagate the
        // error after the loop, since `?` cannot cross a `select!` arm boundary.
        let mut frame_lag: Option<u64> = None;
        loop {
            tokio::select! {
                frame = frame_rx.recv() => match frame {
                    Ok(ev) => yield frame_event_to_proto(ev),
                    // LOSSLESS: we would have to drop a frame — disconnect instead.
                    Err(RecvError::Lagged(n)) => { frame_lag = Some(n); break; }
                    Err(RecvError::Closed) => break,
                },
                tele = tele_rx.recv() => match tele {
                    Ok(ev) => yield telemetry_event_to_proto(ev),
                    // LOSSY: skip the dropped intermediates and keep going.
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => break,
                },
            }
        }
        if let Some(n) = frame_lag {
            Err::<(), Status>(Status::resource_exhausted(
                format!("client lagged frame stream by {n}; disconnecting to avoid dropping frames"),
            ))?;
        }
    };

    Ok(Response::new(Box::pin(stream) as EventStream))
}
