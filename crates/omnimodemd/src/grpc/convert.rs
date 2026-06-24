//! Domain <-> proto conversions. The only module that bridges the two.

use crate::core::error::CoreError;
use crate::core::event::{FrameEvent, TelemetryEvent};
use crate::device::DeviceDescriptor;
use crate::ids::DeviceId;
use crate::proto;
use crate::ptt::registry::{PttConfig, PttMethod};
use crate::ptt::PttError;
use crate::supervisor::ModemSnapshot;
use tonic::Status;

/// Map a core error to a gRPC status.
pub fn core_error_to_status(e: CoreError) -> Status {
    match &e {
        CoreError::UnknownChannel(_) => Status::not_found(e.to_string()),
        CoreError::UnknownMode(_) => Status::invalid_argument(e.to_string()),
        CoreError::Persist(_) => Status::internal(e.to_string()),
        CoreError::Audio(_) => Status::failed_precondition(e.to_string()),
        CoreError::Ptt(p) => match p {
            PttError::DeviceGone { .. } => Status::failed_precondition(e.to_string()),
            PttError::PermissionDenied { .. } => Status::permission_denied(e.to_string()),
            PttError::Busy { .. } => Status::unavailable(e.to_string()),
            PttError::Config(_) => Status::invalid_argument(e.to_string()),
            PttError::Unsupported => Status::unimplemented(e.to_string()),
            PttError::Io(_) => Status::internal(e.to_string()),
        },
        CoreError::Closed => Status::unavailable(e.to_string()),
    }
}

/// A device descriptor as the wire `DeviceInfo`.
pub fn device_descriptor_to_proto(d: &DeviceDescriptor) -> proto::DeviceInfo {
    proto::DeviceInfo {
        device_id: d.id.to_canonical_string(),
        label: d.label.clone(),
        has_capture: d.has_capture,
        has_playback: d.has_playback,
    }
}

/// Build a domain `PttConfig` from a `ConfigurePtt` request, validating the
/// method and device id.
// `tonic::Status` is intentionally the error type across the gRPC boundary; the
// large-err lint does not apply to handler/translation code.
#[allow(clippy::result_large_err)]
pub fn proto_ptt_to_config(req: &proto::ConfigurePttRequest) -> Result<PttConfig, Status> {
    if req.device_id.is_empty() {
        return Err(Status::invalid_argument("device_id must not be empty"));
    }
    let device_id = DeviceId::parse(&req.device_id)
        .ok_or_else(|| Status::invalid_argument(format!("unparseable device_id {}", req.device_id)))?;
    let method = match proto::PttMethod::try_from(req.method) {
        Ok(proto::PttMethod::None) => PttMethod::None,
        Ok(proto::PttMethod::Vox) => PttMethod::Vox,
        Ok(proto::PttMethod::SerialRts) => PttMethod::SerialRts { node: req.node.clone() },
        Ok(proto::PttMethod::SerialDtr) => PttMethod::SerialDtr { node: req.node.clone() },
        Ok(proto::PttMethod::Cm108) => {
            PttMethod::Cm108 { node: req.node.clone(), pin: req.pin_or_line as u8 }
        }
        Ok(proto::PttMethod::Gpio) => {
            PttMethod::Gpio { chip: req.node.clone(), line: req.pin_or_line }
        }
        Ok(proto::PttMethod::Unspecified) | Err(_) => {
            return Err(Status::invalid_argument("ptt method must be specified"));
        }
    };
    Ok(PttConfig { device_id, method, invert: req.invert })
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
        TelemetryEvent::DeviceArrived { device_id, label } => {
            Kind::DeviceArrived(proto::DeviceArrived {
                device_id: device_id.to_canonical_string(),
                label,
            })
        }
        TelemetryEvent::DeviceDeparted { device_id } => {
            Kind::DeviceDeparted(proto::DeviceDeparted {
                device_id: device_id.to_canonical_string(),
            })
        }
        TelemetryEvent::PttKeyed { channel, keyed } => {
            Kind::PttState(proto::PttState { channel: channel.0, keyed })
        }
        TelemetryEvent::ClockOffset { offset_s, est_error_s, synchronized } => {
            Kind::ClockOffset(proto::ClockOffset { offset_s, est_error_s, synchronized })
        }
        TelemetryEvent::ChannelMetrics {
            channel,
            good_frames,
            bad_frames,
            snr_db,
            dbfs,
            afc_offset_hz,
            dcd,
            last_decoder,
        } => Kind::ChannelMetrics(proto::ChannelMetrics {
            channel: channel.0,
            good_frames,
            bad_frames,
            snr_db,
            dbfs,
            afc_offset_hz,
            dcd,
            last_decoder: last_decoder.unwrap_or_default(),
        }),
        TelemetryEvent::SpectrumFrame {
            channel,
            timestamp_ns,
            freq_start_hz,
            freq_step_hz,
            db_floor,
            db_ceiling,
            bins,
        } => Kind::SpectrumFrame(proto::SpectrumFrame {
            channel: channel.0,
            timestamp_ns,
            freq_start_hz,
            freq_step_hz,
            db_floor,
            db_ceiling,
            bins,
        }),
    };
    proto::Event { kind: Some(kind) }
}

/// A metrics snapshot as the wire `ChannelMetrics`.
pub fn metrics_to_proto(snap: &crate::metrics::ChannelMetricsSnapshot) -> proto::ChannelMetrics {
    let m = &snap.metrics;
    proto::ChannelMetrics {
        channel: snap.channel.0,
        good_frames: m.good_frames,
        bad_frames: m.bad_frames,
        snr_db: m.snr_db,
        dbfs: m.dbfs,
        afc_offset_hz: m.afc_offset_hz,
        dcd: m.dcd,
        last_decoder: m.last_decoder.clone().unwrap_or_default(),
    }
}
