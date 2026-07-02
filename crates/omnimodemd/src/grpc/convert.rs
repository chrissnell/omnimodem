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
    // device_id addresses a physical port only for the device-based methods.
    // None and Vox are deviceless — both build a NonePtt that ignores it — so an
    // empty id is valid there (it just keys the registry's presence cache). This
    // lets a channel Apply with the default VOX method and no PTT device picked,
    // instead of failing the whole bind with "device_id must not be empty".
    let device_id = if req.device_id.is_empty() {
        match &method {
            PttMethod::None | PttMethod::Vox => DeviceId::Placeholder { tag: "ptt-deviceless".into() },
            _ => return Err(Status::invalid_argument("device_id must not be empty")),
        }
    } else {
        DeviceId::parse(&req.device_id)
            .ok_or_else(|| Status::invalid_argument(format!("unparseable device_id {}", req.device_id)))?
    };
    Ok(PttConfig { device_id, method, invert: req.invert })
}

/// Build a proto `ModemState` from a snapshot.
pub fn snapshot_to_proto(snap: &ModemSnapshot) -> proto::ModemState {
    let channels = snap
        .channels
        .iter()
        .zip(snap.running.iter())
        .map(|(c, running)| {
            // Report TX empty when it mirrors the RX device (the "same as RX"
            // default), so a client renders "(same as RX)" rather than a literal
            // duplicate id.
            let tx_device_id = if c.tx_device_id == c.device_id {
                String::new()
            } else {
                c.tx_device_id.to_canonical_string()
            };
            // PTT: surface the method, and the device only for device-based
            // methods (None/Vox carry a deviceless placeholder id worth hiding).
            let (ptt_device_id, ptt_method) = match &c.ptt {
                Some(p) => (ptt_device_for_proto(p), ptt_method_to_proto(&p.method) as i32),
                None => (String::new(), proto::PttMethod::Unspecified as i32),
            };
            proto::ChannelInfo {
                channel: c.id.0,
                name: c.name.clone(),
                mode: c.mode.clone(),
                device_id: c.device_id.to_canonical_string(),
                running: *running,
                tx_device_id,
                ptt_device_id,
                ptt_method,
            }
        })
        .collect();
    proto::ModemState { channels }
}

/// The PTT device id to report. Hide only the internal placeholder that truly
/// deviceless configs carry; a real device the operator picked must be reported
/// back so the UI can preload it on reopen — even when the method is VOX/None
/// (the TUI lets a device be chosen independently of the method, and a hidden
/// choice reads as "not saved").
fn ptt_device_for_proto(p: &PttConfig) -> String {
    match &p.device_id {
        DeviceId::Placeholder { .. } => String::new(),
        d => d.to_canonical_string(),
    }
}

/// Map a domain `PttMethod` to its proto enum.
fn ptt_method_to_proto(m: &PttMethod) -> proto::PttMethod {
    match m {
        PttMethod::None => proto::PttMethod::None,
        PttMethod::Vox => proto::PttMethod::Vox,
        PttMethod::SerialRts { .. } => proto::PttMethod::SerialRts,
        PttMethod::SerialDtr { .. } => proto::PttMethod::SerialDtr,
        PttMethod::Cm108 { .. } => proto::PttMethod::Cm108,
        PttMethod::Gpio { .. } => proto::PttMethod::Gpio,
    }
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
            transmit,
        } => Kind::SpectrumFrame(proto::SpectrumFrame {
            channel: channel.0,
            timestamp_ns,
            freq_start_hz,
            freq_step_hz,
            db_floor,
            db_ceiling,
            bins,
            transmit,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn ptt_req(method: proto::PttMethod, device_id: &str) -> proto::ConfigurePttRequest {
        proto::ConfigurePttRequest {
            channel: 0,
            device_id: device_id.to_string(),
            method: method as i32,
            ..Default::default()
        }
    }

    #[test]
    fn deviceless_methods_accept_empty_device_id() {
        for m in [proto::PttMethod::None, proto::PttMethod::Vox] {
            let cfg = proto_ptt_to_config(&ptt_req(m, ""))
                .expect("None/Vox are deviceless and must accept an empty device_id");
            assert!(matches!(cfg.method, PttMethod::None | PttMethod::Vox));
        }
    }

    #[test]
    fn device_based_methods_still_require_device_id() {
        let err = proto_ptt_to_config(&ptt_req(proto::PttMethod::SerialRts, "")).unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }

    #[test]
    fn valid_device_id_is_parsed() {
        let cfg = proto_ptt_to_config(&ptt_req(proto::PttMethod::SerialRts, "serial:usb-FTDI-if00")).unwrap();
        assert_eq!(cfg.device_id, DeviceId::Serial { by_id: "usb-FTDI-if00".into() });
    }

    fn chan_cfg(
        rx: DeviceId,
        tx: DeviceId,
        ptt: Option<PttConfig>,
    ) -> crate::supervisor::channel::ChannelConfig {
        crate::supervisor::channel::ChannelConfig {
            id: crate::ids::ChannelId(0),
            name: "vfo-a".into(),
            mode: "psk31".into(),
            device_id: rx,
            sample_rate: 48_000,
            fanout: 1,
            tx_device_id: tx,
            tx_sample_rate: 0,
            ptt,
        }
    }

    #[test]
    fn snapshot_surfaces_split_tx_and_device_ptt() {
        let rx = DeviceId::AlsaCard { card_name: "Mic".into() };
        let tx = DeviceId::AlsaCard { card_name: "Speakers".into() };
        let ptt = PttConfig {
            device_id: DeviceId::Serial { by_id: "usb-FTDI-if00".into() },
            method: PttMethod::SerialRts { node: "/dev/ttyUSB0".into() },
            invert: false,
        };
        let snap = ModemSnapshot {
            channels: vec![chan_cfg(rx.clone(), tx.clone(), Some(ptt))],
            running: vec![false],
        };
        let ci = &snapshot_to_proto(&snap).channels[0];
        assert_eq!(ci.device_id, rx.to_canonical_string());
        assert_eq!(ci.tx_device_id, tx.to_canonical_string());
        assert_eq!(ci.ptt_device_id, "serial:usb-FTDI-if00");
        assert_eq!(ci.ptt_method, proto::PttMethod::SerialRts as i32);
    }

    #[test]
    fn snapshot_hides_same_as_rx_tx_and_deviceless_ptt() {
        let rx = DeviceId::AlsaCard { card_name: "Mic".into() };
        let ptt = PttConfig {
            device_id: DeviceId::Placeholder { tag: "ptt-deviceless".into() },
            method: PttMethod::Vox,
            invert: false,
        };
        let snap = ModemSnapshot {
            channels: vec![chan_cfg(rx.clone(), rx.clone(), Some(ptt))],
            running: vec![true],
        };
        let ci = &snapshot_to_proto(&snap).channels[0];
        assert_eq!(ci.tx_device_id, "", "TX mirroring RX must report empty");
        assert_eq!(ci.ptt_device_id, "", "deviceless PTT must report empty");
        assert_eq!(ci.ptt_method, proto::PttMethod::Vox as i32);
    }

    #[test]
    fn snapshot_reports_real_ptt_device_even_with_vox_method() {
        // The operator picked a real PTT device but left the method at the
        // default (VOX). The device must still be reported so the UI preloads it
        // on reopen — hiding it read as "my PTT choice wasn't saved".
        let rx = DeviceId::AlsaCard { card_name: "Mic".into() };
        let ptt = PttConfig {
            device_id: DeviceId::Serial { by_id: "usb-FTDI-if00".into() },
            method: PttMethod::Vox,
            invert: false,
        };
        let snap = ModemSnapshot {
            channels: vec![chan_cfg(rx.clone(), rx.clone(), Some(ptt))],
            running: vec![true],
        };
        let ci = &snapshot_to_proto(&snap).channels[0];
        assert_eq!(ci.ptt_device_id, "serial:usb-FTDI-if00");
        assert_eq!(ci.ptt_method, proto::PttMethod::Vox as i32);
    }
}
