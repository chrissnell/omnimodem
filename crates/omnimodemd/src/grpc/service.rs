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

/// Shared gRPC service state: a handle to the sync core plus the KISS listener
/// registry (async-edge only; not part of the DSP core).
#[derive(Clone)]
pub struct ControlService {
    pub(crate) core: CoreHandle,
    pub(crate) kiss: crate::kiss::listener::KissRegistry,
}

impl ControlService {
    pub fn new(core: CoreHandle) -> Self {
        ControlService { core, kiss: crate::kiss::listener::KissRegistry::default() }
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
        let mode = effective_mode(req.mode, req.mode_params);
        let (tx, rx) = oneshot::channel();
        self.send_command(Command::ConfigureChannel {
            id: ChannelId(req.channel),
            name: req.name,
            mode,
            rsid_tx: req.rsid_tx,
            rsid_rx: req.rsid_rx,
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

    async fn transmit_image(
        &self,
        request: Request<proto::TransmitImageRequest>,
    ) -> Result<Response<proto::TransmitResponse>, Status> {
        let req = request.into_inner();
        let send = crate::mode::picture_tx::PictureSend {
            rgb: req.rgb,
            width: req.width,
            height: req.height,
            color: req.color,
            txspp: req.txspp as u8,
        };
        let (tx, rx) = oneshot::channel();
        self.send_command(Command::TransmitImage { channel: ChannelId(req.channel), send, reply: tx })?;
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
        // Empty tx_device_id == single-rig: TX plays on the capture device.
        let tx_device_id = if req.tx_device_id.is_empty() {
            device_id.clone()
        } else {
            DeviceId::parse(&req.tx_device_id).ok_or_else(|| {
                Status::invalid_argument(format!("unparseable tx_device_id {}", req.tx_device_id))
            })?
        };
        let (tx, rx) = oneshot::channel();
        self.send_command(Command::ConfigureAudio {
            id: ChannelId(req.channel),
            device_id,
            sample_rate: req.sample_rate,
            fanout: req.fanout,
            tx_device_id,
            tx_sample_rate: req.tx_sample_rate,
            reply: tx,
        })?;
        let ok = rx
            .await
            .map_err(|_| Status::unavailable("core dropped reply"))?
            .map_err(core_error_to_status)?;
        Ok(Response::new(proto::ConfigureAudioResponse {
            actual_sample_rate: ok.rx_rate,
            actual_tx_sample_rate: ok.tx_rate,
        }))
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

    async fn configure_kiss_listener(
        &self,
        request: Request<proto::ConfigureKissListenerRequest>,
    ) -> Result<Response<proto::ConfigureKissListenerResponse>, Status> {
        let req = request.into_inner();
        let channel = ChannelId(req.channel);

        if !req.enable {
            self.kiss.stop(channel).await;
            return Ok(Response::new(proto::ConfigureKissListenerResponse {
                bound_addr: String::new(),
                active: false,
            }));
        }

        // Validate: the channel must exist and be a packet mode. Query state.
        let (tx, rx) = oneshot::channel();
        self.send_command(Command::GetState { reply: tx })?;
        let snap = rx.await.map_err(|_| Status::unavailable("core dropped reply"))?;
        let ch = snap
            .channels
            .iter()
            .find(|c| c.id == channel)
            .ok_or_else(|| Status::not_found(format!("unknown channel {}", req.channel)))?;
        if !is_packet_mode(&ch.mode) {
            return Err(Status::failed_precondition(format!(
                "channel {} mode '{}' is not a packet mode; KISS needs AFSK 1200 (AX.25)",
                req.channel, ch.mode
            )));
        }

        if req.bind_addr.is_empty() {
            return Err(Status::invalid_argument("bind_addr must be set when enable=true"));
        }
        let bound = self
            .kiss
            .start(self.core.clone(), channel, &req.bind_addr)
            .await
            .map_err(|e| match e {
                crate::kiss::listener::KissError::Bind(io) => {
                    Status::failed_precondition(format!("bind {}: {}", req.bind_addr, io))
                }
            })?;
        Ok(Response::new(proto::ConfigureKissListenerResponse {
            bound_addr: bound.to_string(),
            active: true,
        }))
    }

    async fn set_audio_gain(
        &self,
        request: Request<proto::SetAudioGainRequest>,
    ) -> Result<Response<proto::SetAudioGainResponse>, Status> {
        let req = request.into_inner();
        let (tx, rx) = oneshot::channel();
        self.send_command(Command::SetAudioGain {
            channel: ChannelId(req.channel),
            rx_gain: req.rx_gain,
            tx_gain: req.tx_gain,
            reply: tx,
        })?;
        rx.await
            .map_err(|_| Status::unavailable("core dropped reply"))?
            .map_err(core_error_to_status)?;
        Ok(Response::new(proto::SetAudioGainResponse {}))
    }

    async fn configure_spectrum(
        &self,
        request: Request<proto::ConfigureSpectrumRequest>,
    ) -> Result<Response<proto::ConfigureSpectrumResponse>, Status> {
        let req = request.into_inner();
        let (tx, rx) = oneshot::channel();
        self.send_command(Command::ConfigureSpectrum {
            channel: ChannelId(req.channel),
            enable: req.enable,
            bin_count: req.bin_count,
            fft_size: req.fft_size,
            rate_hz: req.rate_hz,
            freq_lo_hz: req.freq_lo_hz,
            freq_hi_hz: req.freq_hi_hz,
            reply: tx,
        })?;
        let ok = rx
            .await
            .map_err(|_| Status::unavailable("core dropped reply"))?
            .map_err(core_error_to_status)?;
        Ok(Response::new(proto::ConfigureSpectrumResponse {
            bin_count: ok.bin_count,
            fft_size: ok.fft_size,
            rate_hz: ok.rate_hz,
            freq_start_hz: ok.freq_start_hz,
            freq_step_hz: ok.freq_step_hz,
        }))
    }

    async fn set_sdr_tune(
        &self,
        request: Request<proto::SetSdrTuneRequest>,
    ) -> Result<Response<proto::SetSdrTuneResponse>, Status> {
        let req = request.into_inner();
        if !req.freq_hz.is_finite() || req.freq_hz <= 0.0 {
            return Err(Status::invalid_argument("freq_hz must be a positive frequency"));
        }
        let (tx, rx) = oneshot::channel();
        self.send_command(Command::SetSdrTune {
            channel: ChannelId(req.channel),
            freq_hz: req.freq_hz,
            reply: tx,
        })?;
        let ok = rx
            .await
            .map_err(|_| Status::unavailable("core dropped reply"))?
            .map_err(core_error_to_status)?;
        Ok(Response::new(proto::SetSdrTuneResponse {
            actual_freq_hz: ok.actual_freq_hz,
            center_hz: ok.center_hz,
            offset_hz: ok.offset_hz,
        }))
    }

    async fn set_sdr_gain(
        &self,
        request: Request<proto::SetSdrGainRequest>,
    ) -> Result<Response<proto::SetSdrGainResponse>, Status> {
        let req = request.into_inner();
        if !req.r#auto && !req.gain_db.is_finite() {
            return Err(Status::invalid_argument("gain_db must be finite"));
        }
        let (tx, rx) = oneshot::channel();
        self.send_command(Command::SetSdrGain {
            channel: ChannelId(req.channel),
            auto: req.r#auto,
            gain_db: req.gain_db,
            reply: tx,
        })?;
        let ok = rx
            .await
            .map_err(|_| Status::unavailable("core dropped reply"))?
            .map_err(core_error_to_status)?;
        Ok(Response::new(proto::SetSdrGainResponse { actual_gain_db: ok.actual_gain_db }))
    }

    async fn configure_sdr(
        &self,
        request: Request<proto::ConfigureSdrRequest>,
    ) -> Result<Response<proto::ConfigureSdrResponse>, Status> {
        let req = request.into_inner();
        // Reject a `demod_mode` that is not a defined `DemodMode` value up front —
        // an undefined code must not silently fold into NBFM. Every defined mode
        // (NBFM/AM/WFM/SSB) is implemented and passes through to the core.
        let demod_mode = proto::DemodMode::try_from(req.demod_mode)
            .map_err(|_| Status::invalid_argument(format!("unknown demod_mode {}", req.demod_mode)))?;
        let (tx, rx) = oneshot::channel();
        self.send_command(Command::ConfigureSdr {
            channel: ChannelId(req.channel),
            capture_rate: req.capture_rate,
            demod_mode: demod_mode as u8,
            squelch_db: req.squelch_db,
            ppm: req.ppm,
            bias_tee: req.bias_tee,
            direct_sampling: req.direct_sampling,
            reply: tx,
        })?;
        let ok = rx
            .await
            .map_err(|_| Status::unavailable("core dropped reply"))?
            .map_err(core_error_to_status)?;
        Ok(Response::new(proto::ConfigureSdrResponse {
            actual_capture_rate: ok.actual_capture_rate,
        }))
    }

    async fn get_sdr_caps(
        &self,
        request: Request<proto::GetSdrCapsRequest>,
    ) -> Result<Response<proto::GetSdrCapsResponse>, Status> {
        let req = request.into_inner();
        let (tx, rx) = oneshot::channel();
        self.send_command(Command::GetSdrCaps { channel: ChannelId(req.channel), reply: tx })?;
        let ok = rx
            .await
            .map_err(|_| Status::unavailable("core dropped reply"))?
            .map_err(core_error_to_status)?;
        Ok(Response::new(proto::GetSdrCapsResponse {
            tuner: ok.tuner,
            freq_min_hz: ok.freq_min_hz,
            freq_max_hz: ok.freq_max_hz,
            sample_rates: ok.sample_rates,
            gains_db: ok.gains_db,
            bias_tee_supported: ok.bias_tee_supported,
            direct_sampling_supported: ok.direct_sampling_supported,
        }))
    }

    type SubscribeEventsStream = crate::grpc::subscribe::EventStream;

    async fn subscribe_events(
        &self,
        request: Request<proto::SubscribeRequest>,
    ) -> Result<Response<Self::SubscribeEventsStream>, Status> {
        crate::grpc::subscribe::subscribe(self, request).await
    }
}

/// KISS only makes sense for AX.25 packet modes. Today that is AFSK 1200.
fn is_packet_mode(mode: &str) -> bool {
    matches!(mode, "afsk1200")
}

/// Resolve a `ConfigureChannel` request's mode. Typed `mode_params`, when present,
/// is authoritative: its oneof variant selects the mode and supplies its
/// parameters, encoded into the canonical mode string the core persists. Absent ⇒
/// the `mode` string is used unchanged (backward compatible).
fn effective_mode(mode: String, params: Option<proto::ModeParams>) -> String {
    use crate::mode::ModeConfig;
    use proto::mode_params::Params;
    let Some(p) = params.and_then(|p| p.params) else {
        return mode;
    };
    match p {
        Params::Cw(c) => ModeConfig::Cw { wpm: c.wpm as u16, tone_hz: c.tone_hz }.to_mode_string(),
        Params::Rtty(r) => {
            // center_hz unset (0) ⇒ the default US-ham 2210 Hz center.
            let center_hz = if r.center_hz > 0.0 {
                r.center_hz
            } else {
                omnimodem_dsp::modes::rtty::CENTER_HZ
            };
            ModeConfig::Rtty { baud: r.baud, shift_hz: r.shift_hz, center_hz, reverse: r.reverse }
                .to_mode_string()
        }
        Params::Psk31(p) => {
            ModeConfig::Psk { submode: "psk31".into(), center_hz: p.center_hz }.to_mode_string()
        }
        Params::Psk(p) => {
            // A known submode encodes canonically; an unknown one falls back to
            // the bare `mode` string (which `ModeConfig::parse` then validates).
            match ModeConfig::parse(&format!("{}:center={}", p.submode, p.center_hz)) {
                Some(cfg) => cfg.to_mode_string(),
                None => mode,
            }
        }
        Params::Dominoex(p) => {
            // A known submode encodes canonically; an unknown one falls back to
            // the bare `mode` string (which `ModeConfig::parse` then validates).
            match ModeConfig::parse(&format!("{}:center={}", p.submode, p.center_hz)) {
                Some(cfg) => cfg.to_mode_string(),
                None => mode,
            }
        }
        Params::Hell(p) => {
            // A known submode encodes canonically; an unknown one falls back to
            // the bare `mode` string (which `ModeConfig::parse` then validates).
            match ModeConfig::parse(&format!("{}:center={}", p.submode, p.center_hz)) {
                Some(cfg) => cfg.to_mode_string(),
                None => mode,
            }
        }
        Params::Mfsk(p) => {
            // A known submode encodes canonically; an unknown one falls back to
            // the bare `mode` string (which `ModeConfig::parse` then validates).
            match ModeConfig::parse(&format!("{}:center={}", p.submode, p.center_hz)) {
                Some(cfg) => cfg.to_mode_string(),
                None => mode,
            }
        }
        Params::Mt63(p) => {
            // A known submode encodes canonically; an unknown one falls back to
            // the bare `mode` string (which `ModeConfig::parse` then validates).
            match ModeConfig::parse(&format!("{}:center={}", p.submode, p.center_hz)) {
                Some(cfg) => cfg.to_mode_string(),
                None => mode,
            }
        }
        Params::Navtex(p) => {
            // A known submode encodes canonically; an unknown one falls back to
            // the bare `mode` string (which `ModeConfig::parse` then validates).
            match ModeConfig::parse(&format!("{}:center={}", p.submode, p.center_hz)) {
                Some(cfg) => cfg.to_mode_string(),
                None => mode,
            }
        }
        Params::Wefax(p) => {
            // A known submode encodes canonically; an unknown one falls back to
            // the bare `mode` string (which `ModeConfig::parse` then validates).
            match ModeConfig::parse(&format!("{}:center={}", p.submode, p.center_hz)) {
                Some(cfg) => cfg.to_mode_string(),
                None => mode,
            }
        }
        Params::Contestia(c) => {
            ModeConfig::Contestia { tones: c.tones as u16, bandwidth_hz: c.bandwidth_hz as u16 }
                .to_mode_string()
        }
        Params::Thor(p) => {
            // A known submode encodes canonically; an unknown one falls back to
            // the bare `mode` string (which `ModeConfig::parse` then validates).
            match ModeConfig::parse(&format!("{}:center={}", p.submode, p.center_hz)) {
                Some(cfg) => cfg.to_mode_string(),
                None => mode,
            }
        }
        Params::Throb(p) => {
            // A known submode encodes canonically; an unknown one falls back to
            // the bare `mode` string (which `ModeConfig::parse` then validates).
            match ModeConfig::parse(&format!("{}:center={}", p.submode, p.center_hz)) {
                Some(cfg) => cfg.to_mode_string(),
                None => mode,
            }
        }
        Params::Olivia(o) => {
            ModeConfig::Olivia { tones: o.tones as u16, bandwidth_hz: o.bandwidth_hz as u16 }
                .to_mode_string()
        }
        Params::Ifkp(p) => {
            // A known speed encodes canonically; an unknown one falls back to the
            // bare `mode` string (which `ModeConfig::parse` then validates).
            match ModeConfig::parse(&format!("{}:center={}", p.speed, p.center_hz)) {
                Some(cfg) => cfg.to_mode_string(),
                None => mode,
            }
        }
        Params::Fsq(p) => {
            match ModeConfig::parse(&format!(
                "{}:center={},mycall={},directed={}",
                p.speed, p.center_hz, p.mycall, p.directed
            )) {
                Some(cfg) => cfg.to_mode_string(),
                None => mode,
            }
        }
        Params::Afsk1200(a) => ModeConfig::Afsk1200 { tx: a.tx }.to_mode_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{effective_mode, is_packet_mode};
    use crate::proto;

    #[test]
    fn only_afsk_is_a_packet_mode() {
        assert!(is_packet_mode("afsk1200"));
        assert!(!is_packet_mode("ft8"));
        assert!(!is_packet_mode("none"));
    }

    #[test]
    fn effective_mode_passes_bare_string_through() {
        assert_eq!(effective_mode("ft8".into(), None), "ft8");
        assert_eq!(effective_mode("cw".into(), Some(proto::ModeParams { params: None })), "cw");
    }

    #[test]
    fn effective_mode_encodes_typed_params_to_canonical_string() {
        let mp = proto::ModeParams {
            params: Some(proto::mode_params::Params::Cw(proto::CwParams { wpm: 25, tone_hz: 600.0 })),
        };
        assert_eq!(effective_mode("ignored".into(), Some(mp)), "cw:wpm=25,tone=600");
    }

    #[test]
    fn effective_mode_encodes_psk_family_params() {
        let mp = proto::ModeParams {
            params: Some(proto::mode_params::Params::Psk(proto::PskParams {
                submode: "psk250".into(),
                center_hz: 1500.0,
            })),
        };
        assert_eq!(effective_mode("ignored".into(), Some(mp)), "psk250:center=1500");
        // Legacy Psk31Params still maps onto the parametric Psk config.
        let legacy = proto::ModeParams {
            params: Some(proto::mode_params::Params::Psk31(proto::Psk31Params { center_hz: 1000.0 })),
        };
        assert_eq!(effective_mode("ignored".into(), Some(legacy)), "psk31:center=1000");
    }

    #[test]
    fn effective_mode_encodes_dominoex_params() {
        let mp = proto::ModeParams {
            params: Some(proto::mode_params::Params::Dominoex(proto::DominoParams {
                submode: "dominoex16".into(),
                center_hz: 1500.0,
            })),
        };
        assert_eq!(effective_mode("ignored".into(), Some(mp)), "dominoex16:center=1500");
    }

    #[test]
    fn effective_mode_encodes_thor_params() {
        let mp = proto::ModeParams {
            params: Some(proto::mode_params::Params::Thor(proto::ThorParams {
                submode: "thor16".into(),
                center_hz: 1500.0,
            })),
        };
        assert_eq!(effective_mode("ignored".into(), Some(mp)), "thor16:center=1500");
    }

    #[test]
    fn effective_mode_encodes_ifkp_params() {
        let mp = proto::ModeParams {
            params: Some(proto::mode_params::Params::Ifkp(proto::IfkpParams {
                speed: "ifkp-slow".into(),
                center_hz: 1500.0,
            })),
        };
        assert_eq!(effective_mode("ignored".into(), Some(mp)), "ifkp-slow:center=1500");
    }

    #[test]
    fn effective_mode_encodes_fsq_params() {
        let mp = proto::ModeParams {
            params: Some(proto::mode_params::Params::Fsq(proto::FsqParams {
                speed: "fsq".into(),
                center_hz: 1500.0,
                mycall: "k1abc".into(),
                directed: true,
            })),
        };
        assert_eq!(
            effective_mode("ignored".into(), Some(mp)),
            "fsq:center=1500,mycall=k1abc,directed=true"
        );
    }
}
