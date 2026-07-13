//! The transport-agnostic DSP capture body. [`run_capture`] drives any
//! [`SdrTransport`] source of raw u8 IQ through the complex NCO channel-select,
//! the selected demodulator, decimation, squelch, the wideband RF waterfall tap,
//! and overrun-safe delivery to the modem — exactly the pipeline the `rtl_tcp`
//! backend ran inline, now shared so a second transport reuses it verbatim.

use super::{DemodMode, SdrControl, SdrTransport};
use crate::audio::{AudioChunk, CHUNK_QUEUE_DEPTH};
use crate::core::event::TelemetryEvent;
use crate::ids::ChannelId;
use omnimodem_dsp::frontend::complex_stft::ComplexStft;
use omnimodem_dsp::frontend::iq::u8_iq_to_cplx;
use omnimodem_dsp::frontend::sdr_demod::SdrDemod;
use omnimodem_dsp::frontend::spectrum::{full_spectrum_dbfs, SpectrumPlan};
use std::collections::VecDeque;
use std::sync::mpsc::{SyncSender, TrySendError};
use tokio::sync::broadcast;

/// FFT size for the wideband RF waterfall.
const WATERFALL_NFFT: usize = 1024;
/// Requested waterfall bin count (rendered uint8 line width).
const WATERFALL_BINS: usize = 256;
/// Scale applied to `RawMag` magnitude so the u8-IQ maximum (|±1 ±1j| = √2) fits in
/// [0,1] ahead of the i16 delivery clamp. The ADS-B PPM demod is scale-independent,
/// so this only prevents strong-pulse saturation.
pub(crate) const INV_SQRT2: f32 = std::f32::consts::FRAC_1_SQRT_2;
/// Log at most one overrun warning per this many dropped chunks, so a persistent
/// lag does not flood the log while still surfacing the running total.
const OVERRUN_LOG_EVERY: u64 = 64;

/// Outcome of a non-blocking audio delivery attempt.
enum Delivery {
    /// The consumer is still connected (chunk queued, or dropped under overrun).
    Live,
    /// The consumer dropped its receiver — the capture is terminal.
    ConsumerGone,
}

/// Deliver `chunk` to the bounded consumer channel without ever blocking the
/// source read. `backlog` stages whatever the channel won't accept right now; when
/// the *backlog* grows past `CHUNK_QUEUE_DEPTH` its oldest chunk is dropped (a live
/// modem wants fresh audio, not stale backlog) and counted on `control`. Note the
/// dropped chunk is the oldest *un-accepted* one — the strictly-oldest chunks are
/// already in the consumer channel — so worst-case buffering is the channel depth
/// plus the backlog depth (~2·`CHUNK_QUEUE_DEPTH`), which bounds latency without a
/// second thread. Returns `ConsumerGone` once the receiver is gone.
fn deliver_audio(
    tx: &SyncSender<AudioChunk>,
    backlog: &mut VecDeque<AudioChunk>,
    chunk: AudioChunk,
    control: &SdrControl,
) -> Delivery {
    backlog.push_back(chunk);
    // Push as much of the backlog as the consumer will take right now.
    while let Some(front) = backlog.pop_front() {
        match tx.try_send(front) {
            Ok(()) => {}
            Err(TrySendError::Full(front)) => {
                backlog.push_front(front);
                break;
            }
            Err(TrySendError::Disconnected(_)) => return Delivery::ConsumerGone,
        }
    }
    // Bound the staged backlog by dropping the oldest chunks the consumer is too
    // slow to accept, so latency stays bounded and capture keeps reading.
    while backlog.len() > CHUNK_QUEUE_DEPTH {
        backlog.pop_front();
        let total = control.incr_dropped();
        // Surface the onset (first-ever drop) immediately, then rate-limit so a
        // sustained lag reports its running total without flooding the log.
        if total == 1 || total.is_multiple_of(OVERRUN_LOG_EVERY) {
            tracing::warn!(
                dropped = total,
                "sdr capture overrun: consumer lagging, dropped oldest queued audio"
            );
        }
    }
    Delivery::Live
}

/// Emit one RF-referenced waterfall line per complete STFT frame of the raw IQ.
/// `freq_start_hz` is absolute RF: bin[0] = hardware center − rate/2.
fn emit_waterfall(
    stft: &mut ComplexStft,
    iq: &[omnimodem_dsp::types::Cplx],
    capture_rate: u32,
    center_hz: f64,
    channel: ChannelId,
    telemetry: &broadcast::Sender<TelemetryEvent>,
) {
    // Geometry is invariant for a given center; build it once per block, not per
    // FFT frame.
    let plan = SpectrumPlan::new_centered(
        WATERFALL_NFFT,
        capture_rate as f32,
        center_hz as f32,
        WATERFALL_BINS,
        -(capture_rate as f32) / 2.0,
        (capture_rate as f32) / 2.0,
    );
    for frame in stft.feed(iq) {
        let dbfs = full_spectrum_dbfs(&frame, stft.window_sum());
        let bins = plan.render(&dbfs);
        let timestamp_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        let _ = telemetry.send(TelemetryEvent::SpectrumFrame {
            channel,
            timestamp_ns,
            freq_start_hz: plan.freq_start_hz,
            freq_step_hz: plan.freq_step_hz,
            db_floor: plan.db_floor,
            db_ceiling: plan.db_ceiling,
            bins,
            transmit: false,
        });
    }
}

/// Drive `transport` through the shared RX pipeline until the consumer drops or the
/// transport reports a terminal stop. Builds the RX chain from `control`, then per
/// block: read IQ, reconcile any runtime control change (rebuild the chain on a
/// rate/mode change or retune the NCO/squelch otherwise, re-applying hardware to
/// the transport), emit the RF waterfall, demodulate, and deliver audio with the
/// drop-oldest overrun policy. `channel_rate` is the sample rate delivered
/// downstream (the audio channel rate, or the full capture rate for `RawMag`).
pub(crate) fn run_capture<T: SdrTransport>(
    mut transport: T,
    control: SdrControl,
    telemetry: Option<broadcast::Sender<TelemetryEvent>>,
    channel: ChannelId,
    deviation_hz: f32,
    channel_rate: u32,
    tx: SyncSender<AudioChunk>,
) {
    let mut stft = ComplexStft::new(WATERFALL_NFFT, WATERFALL_NFFT);
    let mut buf = vec![0u8; WATERFALL_NFFT * 2];
    let mut backlog: VecDeque<AudioChunk> = VecDeque::new();

    // Build the RX chain from current control. Params may have been set before the
    // capture opened; the transport was just commanded to match at connect.
    let mut cur_rate = control.capture_rate();
    let mut cur_mode = control.demod_mode();
    // `RawMag` (ADS-B) bypasses the channelizing demod entirely and emits the
    // full-rate magnitude envelope; build an `SdrDemod` only for the audio modes.
    let mut raw_mag = cur_mode == DemodMode::RawMag;
    let mut rx_chain = (!raw_mag).then(|| {
        SdrDemod::new(
            cur_mode.to_dsp(),
            cur_rate,
            channel_rate,
            control.offset_hz(),
            deviation_hz,
            control.effective_squelch(),
        )
    });
    let mut seen_gen = control.generation();

    loop {
        let n = match transport.read_iq(&mut buf) {
            Ok(0) => break, // terminal: shutdown signalled (or a hard stop)
            Ok(n) => n,
            // Terminal read error. A local USB transport reports a mid-capture
            // removal as `AudioError::UsbLost`; unlike `rtl_tcp` (which reconnects
            // transparently inside `read_iq`), there is no recovery here, so the
            // capture ends — the channel then unbinds and hotplug reports Departed.
            Err(e) => {
                tracing::warn!(error = %e, "sdr capture: terminal read error, ending capture");
                break;
            }
        };

        // The transport hands back whole IQ pairs (even byte count) and drops any
        // half-pair across a reconnect, so no boundary carry is reassembled here.
        let iq = u8_iq_to_cplx(&buf[..n]);
        if iq.is_empty() {
            continue;
        }

        // Reconcile runtime control changes before demodulating.
        let gen = control.generation();
        if gen != seen_gen {
            seen_gen = gen;
            // A capture-rate or demod-mode change rebuilds the whole RX chain: the
            // decimation ratio and NCO base rate depend on the rate, and the
            // back-end (and WFM's wide IF) depend on the mode. `apply_hardware`
            // (below) re-commands the dongle, including the sample rate.
            let want_rate = control.capture_rate();
            let want_mode = control.demod_mode();
            let rate_changed = want_rate != cur_rate && want_rate != 0;
            if rate_changed || want_mode != cur_mode {
                if rate_changed {
                    cur_rate = want_rate;
                }
                cur_mode = want_mode;
                // Rebuild the RX chain for the new rate/mode. Crossing the `RawMag`
                // boundary at runtime changes the delivered sample rate, which a
                // live capture can't re-negotiate; the core re-opens the capture on
                // such a mode switch, so here we only track the flag and skip the
                // audio demod.
                raw_mag = cur_mode == DemodMode::RawMag;
                rx_chain = (!raw_mag).then(|| {
                    SdrDemod::new(
                        cur_mode.to_dsp(),
                        cur_rate,
                        channel_rate,
                        control.offset_hz(),
                        deviation_hz,
                        control.effective_squelch(),
                    )
                });
            } else if let Some(rc) = rx_chain.as_mut() {
                rc.retune(control.offset_hz());
                rc.set_squelch(control.effective_squelch());
            }
            let _ = transport.apply_hardware(&control);
        }

        if let Some(tele) = telemetry.as_ref() {
            emit_waterfall(&mut stft, &iq, cur_rate, control.center_hz(), channel, tele);
        }

        let audio = match rx_chain.as_mut() {
            Some(rc) => rc.push_iq(&iq),
            // `RawMag`: emit the full-rate magnitude envelope, scaled by 1/√2 so
            // the u8-IQ maximum (|±1 ±1j| = √2) maps into [0,1] without clipping
            // the i16 delivery path. The PPM demod is scale-independent, so the
            // scale is otherwise free; it only keeps strong pulses from saturating.
            None => iq.iter().map(|c| c.norm() * INV_SQRT2).collect(),
        };
        if audio.is_empty() {
            continue;
        }
        let chunk: AudioChunk = audio
            .iter()
            .map(|&s| (s.clamp(-1.0, 1.0) * 32767.0) as i16)
            .collect();
        // Never block the source read: stage + drop-oldest on lag.
        if let Delivery::ConsumerGone = deliver_audio(&tx, &mut backlog, chunk, &control) {
            break; // consumer dropped — terminal
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::sdr::{
        supported_sample_rates, tuner_freq_range, tuner_gains_db, TunerCaps, DEFAULT_CAPTURE_RATE,
        DEFAULT_DEVIATION_HZ,
    };
    use std::time::Duration;

    /// A scripted, non-`rtl_tcp` [`SdrTransport`]: hands `run_capture` a fixed IQ
    /// slice in `buf`-sized reads, then signals a terminal stop. Proves the DSP
    /// pipeline is transport-agnostic — it demodulates identically whether its IQ
    /// arrived over a socket or from this in-memory fake.
    struct FakeTransport {
        iq: Vec<u8>,
        pos: usize,
        reads: usize,
        /// After this many successful reads, the next one fails with the terminal
        /// [`AudioError::UsbLost`] — modelling a dongle unplugged mid-capture.
        error_after: Option<usize>,
    }

    impl FakeTransport {
        fn new(iq: Vec<u8>) -> Self {
            FakeTransport { iq, pos: 0, reads: 0, error_after: None }
        }
    }

    impl SdrTransport for FakeTransport {
        fn read_iq(&mut self, buf: &mut [u8]) -> Result<usize, crate::audio::AudioError> {
            if self.error_after == Some(self.reads) {
                return Err(crate::audio::AudioError::UsbLost(
                    "bulk in transfer: device disconnected".into(),
                ));
            }
            if self.pos >= self.iq.len() {
                return Ok(0); // exhausted — terminal, ends the capture loop
            }
            let n = buf.len().min(self.iq.len() - self.pos);
            buf[..n].copy_from_slice(&self.iq[self.pos..self.pos + n]);
            self.pos += n;
            self.reads += 1;
            Ok(n)
        }

        fn apply_hardware(&mut self, _control: &SdrControl) -> Result<(), crate::audio::AudioError> {
            Ok(())
        }

        fn caps(&self) -> TunerCaps {
            let (freq_min_hz, freq_max_hz) = tuner_freq_range(5);
            TunerCaps {
                tuner: "R820T".into(),
                freq_min_hz,
                freq_max_hz,
                sample_rates: supported_sample_rates(),
                gains_db: tuner_gains_db(5),
                bias_tee_supported: true,
                direct_sampling_supported: true,
            }
        }

        fn shutdown_handle(&self) -> Box<dyn FnOnce() + Send> {
            Box::new(|| {})
        }
    }

    /// FM-modulate a `tone_hz` sine at `offset_hz` into u8 IQ at `rate`.
    fn fm_iq_u8(rate: f32, offset_hz: f32, tone_hz: f32, dev_hz: f32, n: usize) -> Vec<u8> {
        let mut phase = 0.0f32;
        let mut out = Vec::with_capacity(n * 2);
        for k in 0..n {
            let t = k as f32 / rate;
            let inst = offset_hz + dev_hz * (std::f32::consts::TAU * tone_hz * t).sin();
            phase += std::f32::consts::TAU * inst / rate;
            let i = ((phase.cos() * 0.9 * 127.5) + 127.5).round().clamp(0.0, 255.0) as u8;
            let q = ((phase.sin() * 0.9 * 127.5) + 127.5).round().clamp(0.0, 255.0) as u8;
            out.push(i);
            out.push(q);
        }
        out
    }

    #[test]
    fn run_capture_is_transport_agnostic_over_a_fake_transport() {
        // The pipeline must demodulate an NBFM tone driven by ANY transport, not
        // just `rtl_tcp`. Feed a scripted FM burst through the fake and assert the
        // shared `run_capture` delivers demodulated audio — the same expectation
        // the socket-backed capture test makes.
        let iq = fm_iq_u8(
            DEFAULT_CAPTURE_RATE as f32,
            30_000.0,
            1_200.0,
            DEFAULT_DEVIATION_HZ,
            48_000,
        );
        let control = SdrControl::default();
        control.set_offset_hz(30_000.0); // channel-select the tone
        let transport = FakeTransport::new(iq);

        let (tx, rx) = std::sync::mpsc::sync_channel::<AudioChunk>(CHUNK_QUEUE_DEPTH);
        // No telemetry sink: exercise the pure demod path (waterfall is optional).
        run_capture(transport, control, None, ChannelId(0), DEFAULT_DEVIATION_HZ, 48_000, tx);

        // The transport is exhausted, so `run_capture` returns; drain the delivered
        // audio (the sender is already dropped, so recv ends on disconnect).
        let mut total = 0usize;
        while let Ok(chunk) = rx.recv_timeout(Duration::from_millis(200)) {
            total += chunk.len();
        }
        assert!(total > 0, "pipeline delivered no audio through the fake transport");
    }

    #[test]
    fn run_capture_exits_on_mid_capture_usb_removal() {
        // A dongle unplugged mid-capture: the transport streams a few blocks, then
        // `read_iq` returns the terminal `UsbLost`. `run_capture` must exit (not spin
        // or reconnect like rtl_tcp), which drops `tx` and lets the channel unbind.
        let iq = fm_iq_u8(DEFAULT_CAPTURE_RATE as f32, 30_000.0, 1_200.0, DEFAULT_DEVIATION_HZ, 48_000);
        let mut transport = FakeTransport::new(iq);
        transport.error_after = Some(3); // unplugged after 3 blocks

        let control = SdrControl::default();
        control.set_offset_hz(30_000.0);
        let (tx, rx) = std::sync::mpsc::sync_channel::<AudioChunk>(CHUNK_QUEUE_DEPTH);

        // Runs on this thread: if the terminal error is not honored, this never
        // returns and the test hangs. Returning at all proves the capture exited.
        run_capture(transport, control, None, ChannelId(0), DEFAULT_DEVIATION_HZ, 48_000, tx);

        // The sender is dropped, so the consumer channel is now disconnected: after
        // draining any delivered audio, the next recv reports the unbind.
        while rx.recv_timeout(Duration::from_millis(200)).is_ok() {}
        assert!(
            matches!(rx.recv(), Err(std::sync::mpsc::RecvError)),
            "capture thread must drop the sender so the channel unbinds"
        );
    }

    #[test]
    fn slow_consumer_drops_oldest_without_stalling_the_source() {
        // The shared drop-oldest overrun policy must protect the USB source too: a
        // consumer that never reads must not stall the capture. Drive a long stream
        // through `run_capture` with a full, unread channel and assert it (a) reads
        // the whole source to exhaustion (never blocked on delivery) and (b) counted
        // the dropped chunks on the control cell.
        //
        // `RawMag` emits one chunk per read with no demod warm-up, so the arithmetic
        // is exact: far more than the channel + backlog depth (~2·CHUNK_QUEUE_DEPTH)
        // guarantees the drop-oldest path fires.
        let reads = CHUNK_QUEUE_DEPTH * 8;
        let iq = vec![200u8; WATERFALL_NFFT * 2 * reads];

        let control = SdrControl::default();
        control.set_demod_mode(DemodMode::RawMag);
        assert_eq!(control.dropped_chunks(), 0);

        let transport = FakeTransport::new(iq);
        // Hold the receiver but never read it, so the channel stays full.
        let (tx, _rx) = std::sync::mpsc::sync_channel::<AudioChunk>(CHUNK_QUEUE_DEPTH);
        let capture_rate = control.capture_rate();
        run_capture(transport, control.clone(), None, ChannelId(0), DEFAULT_DEVIATION_HZ, capture_rate, tx);

        assert!(
            control.dropped_chunks() > 0,
            "a stalled consumer must increment dropped_chunks via the overrun path"
        );
    }
}
