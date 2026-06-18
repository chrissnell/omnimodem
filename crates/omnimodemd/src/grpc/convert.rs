//! Domain <-> proto conversions. The only module that bridges the two.

use crate::core::error::CoreError;
use crate::core::event::{FrameEvent, TelemetryEvent};
use crate::proto;
use crate::supervisor::ModemSnapshot;
use tonic::Status;

/// Map a core error to a gRPC status.
pub fn core_error_to_status(e: CoreError) -> Status {
    match e {
        CoreError::UnknownChannel(_) => Status::not_found(e.to_string()),
        CoreError::Persist(_) => Status::internal(e.to_string()),
        CoreError::Closed => Status::unavailable(e.to_string()),
    }
}

/// Build a proto `ModemState` from a snapshot.
pub fn snapshot_to_proto(snap: &ModemSnapshot) -> proto::ModemState {
    let channels = snap
        .channels
        .iter()
        .zip(snap.running.iter())
        .map(|(c, running)| proto::ChannelInfo {
            channel: c.id.0,
            name: c.name.clone(),
            mode: c.mode.clone(),
            device_id: c.device_id.to_canonical_string(),
            running: *running,
        })
        .collect();
    proto::ModemState { channels }
}

/// Wrap a frame event as a proto `Event`.
pub fn frame_event_to_proto(ev: FrameEvent) -> proto::Event {
    let kind = match ev {
        FrameEvent::RxFrame { channel, data, timestamp_ns } => {
            proto::event::Kind::RxFrame(proto::RxFrame {
                channel: channel.0,
                data,
                timestamp_ns,
            })
        }
    };
    proto::Event { kind: Some(kind) }
}

/// Wrap a telemetry event as a proto `Event`.
pub fn telemetry_event_to_proto(ev: TelemetryEvent) -> proto::Event {
    use proto::event::Kind;
    let kind = match ev {
        TelemetryEvent::ChannelConfigured { channel } => {
            Kind::ChannelConfigured(proto::ChannelConfigured { channel: channel.0 })
        }
        TelemetryEvent::TransmitStarted { channel, transmit_id } => {
            Kind::TransmitStarted(proto::TransmitStarted {
                channel: channel.0,
                transmit_id: transmit_id.0,
            })
        }
        TelemetryEvent::TransmitComplete { channel, transmit_id } => {
            Kind::TransmitComplete(proto::TransmitComplete {
                channel: channel.0,
                transmit_id: transmit_id.0,
            })
        }
        TelemetryEvent::AudioLevel { channel, dbfs } => {
            Kind::AudioLevel(proto::AudioLevel { channel: channel.0, dbfs })
        }
        TelemetryEvent::Status { channel, tx_frames } => {
            Kind::Status(proto::Status { channel: channel.0, tx_frames })
        }
    };
    proto::Event { kind: Some(kind) }
}
