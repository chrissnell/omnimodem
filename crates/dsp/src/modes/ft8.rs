//! FT8 mode assembly: 12 kHz, 8-FSK, 79 symbols (3×7 Costas + 58 data), 6.25 Hz
//! tone spacing, 1920 samples/symbol. Block/windowed: a 15 s slot carries the
//! 12.64 s waveform.
//!
//! TX: text → `pack77` → CRC-14 → LDPC(174,91) encode → 58 data symbols (3 bits
//! each, FT8 Gray map) interleaved with the three Costas sync groups → Gaussian
//! 8-FSK.
//! RX: a symbol-rate tone-energy spectrogram → Costas-array 2D sync over base
//! frequency and time offset → per-symbol soft-LLR demap → LDPC min-sum (OSD
//! fallback) → CRC-14 check → `unpack77`. Single-pass BP+OSD per sync candidate
//! (SIC and a-priori decoding are Phase-5 differentiators).

use crate::fec::crc::ftx_compute_crc;
use crate::fec::ldpc::Ldpc;
use crate::fec::llr::{demap_fsk_ft8, FT8_GRAY_MAP};
use crate::fec::osd::osd_decode;
use crate::framing::message77::{pack77, unpack77};
use crate::frontend::modulate::Gfsk;
use crate::mode::{
    BlockDemodulator, DemodShape, Duplex, ModError, ModeCaps, Modulator,
};
use crate::sync::costas_array::{ft8_costas, CostasCorrelator};
use crate::types::{Frame, FrameMeta, FramePayload, Sample};

pub const FT8_RATE: u32 = 12_000;
pub const FT8_SPS: usize = 1920; // samples/symbol (0.16 s)
pub const FT8_TONE_SPACING: f32 = 6.25; // Hz
pub const FT8_BASE_HZ: f32 = 1000.0; // default audio sub-carrier (tone 0)
pub const FT8_NSYM: usize = 79;
pub const FT8_SLOT_S: f32 = 15.0;
pub const FT8_WINDOW_S: f32 = 15.0;

/// Costas group start positions within the 79-symbol frame: symbols 0–6, 36–42,
/// 72–78.
pub const FT8_COSTAS_STARTS: [usize; 3] = [0, 36, 72];

const K91: usize = 91; // LDPC message length (77 message + 14 CRC bits)
const N174: usize = 174; // LDPC codeword length
const CRC_BITS: usize = 14;
const MSG_BITS: usize = 77;

#[inline]
fn is_costas_symbol(s: usize) -> bool {
    FT8_COSTAS_STARTS.iter().any(|&start| (start..start + 7).contains(&s))
}

/// FT8 CRC-14 over the 77-bit payload, byte-exact with ft8_lib `ftx_add_crc`:
/// the 10-byte payload (low 3 bits of byte 9 are the message pad = 0) is
/// zero-extended to 82 bits (a trailing zero byte supplies bits 80..81) and
/// CRCed over `96 - 14 = 82` bits.
fn ft8_payload_crc(payload: &[u8; 10]) -> u16 {
    let mut buf = [0u8; 11];
    buf[..10].copy_from_slice(payload);
    buf[9] &= 0xF8; // clear the 3 bits after the 77-bit payload (already 0)
    ftx_compute_crc(&buf, 82)
}

// ---------------------------------------------------------------------------
// Transmit
// ---------------------------------------------------------------------------

/// Build the 79 channel-symbol tones (0–7) for a 77-bit message: LDPC-coded
/// data symbols interleaved with the three Costas sync groups.
pub fn ft8_symbols(message: &str) -> [u8; FT8_NSYM] {
    let payload = pack77(message); // [u8;10]: 77 message bits MSB-first + 3 zero pad
    let cksum = ft8_payload_crc(&payload);

    // 91 message+CRC bits: 77 message bits, then 14 CRC bits (MSB-first).
    let mut bits91 = vec![0u8; K91];
    for (i, b) in bits91.iter_mut().take(MSG_BITS).enumerate() {
        *b = (payload[i / 8] >> (7 - (i % 8))) & 1;
    }
    for i in 0..CRC_BITS {
        bits91[MSG_BITS + i] = ((cksum >> (CRC_BITS - 1 - i)) & 1) as u8;
    }

    let cw = Ldpc::ft8().encode(&bits91); // 174 = 91 systematic + 83 parity
    let costas = ft8_costas();
    let mut syms = [0u8; FT8_NSYM];
    let mut di = 0usize; // data-symbol index 0..58
    for (s, sym) in syms.iter_mut().enumerate() {
        if let Some(g) = FT8_COSTAS_STARTS.iter().position(|&st| (st..st + 7).contains(&s)) {
            *sym = costas[s - FT8_COSTAS_STARTS[g]] as u8;
        } else {
            let idx = ((cw[di * 3] as usize) << 2)
                | ((cw[di * 3 + 1] as usize) << 1)
                | (cw[di * 3 + 2] as usize);
            *sym = FT8_GRAY_MAP[idx];
            di += 1;
        }
    }
    syms
}

pub struct Ft8Mod {
    base_hz: f32,
}

impl Ft8Mod {
    pub fn new() -> Self {
        Ft8Mod { base_hz: FT8_BASE_HZ }
    }
    /// Build a modulator on a non-default audio sub-carrier (tone-0 frequency).
    pub fn with_base(base_hz: f32) -> Self {
        Ft8Mod { base_hz }
    }
}

impl Default for Ft8Mod {
    fn default() -> Self {
        Self::new()
    }
}

fn ft8_caps(tx: bool) -> ModeCaps {
    ModeCaps {
        native_rate: FT8_RATE,
        bandwidth_hz: 50.0,
        tx,
        duplex: Duplex::Half,
        shape: DemodShape::Windowed { window_s: FT8_WINDOW_S, period_s: FT8_SLOT_S },
    }
}

impl Modulator for Ft8Mod {
    fn caps(&self) -> ModeCaps {
        ft8_caps(true)
    }

    fn modulate(&mut self, frame: &Frame) -> Result<Vec<Sample>, ModError> {
        let message = match &frame.payload {
            FramePayload::Text(t) => t.clone(),
            FramePayload::Message77(m) => unpack77(m),
            _ => return Err(ModError::UnsupportedPayload("ft8 needs text/message77")),
        };
        let syms = ft8_symbols(&message);
        let sym_u32: Vec<u32> = syms.iter().map(|&s| s as u32).collect();
        let gfsk = Gfsk::new(FT8_RATE as f32, FT8_SPS, self.base_hz, FT8_TONE_SPACING, 2.0);
        Ok(gfsk.modulate(&sym_u32))
    }
}

// ---------------------------------------------------------------------------
// Receive
// ---------------------------------------------------------------------------

/// Block (windowed) FT8 demodulator.
pub struct Ft8Demod {
    f_lo: f32,
    f_hi: f32,
    /// Maximum decodes to emit per window.
    max_decodes: usize,
}

impl Ft8Demod {
    pub fn new() -> Self {
        // Sweep the usual FT8 audio passband; f_lo is on the 6.25 Hz grid.
        Ft8Demod { f_lo: 200.0, f_hi: 3000.0, max_decodes: 32 }
    }

    /// The windowed grid period in seconds (for the daemon RX worker).
    pub fn window_s(&self) -> f32 {
        FT8_WINDOW_S
    }
}

impl Default for Ft8Demod {
    fn default() -> Self {
        Self::new()
    }
}

/// Single-bin power via Goertzel over `x` at frequency `f`. FT8 tones spaced
/// 6.25 Hz over a 1920-sample symbol at 12 kHz are exactly orthogonal, so a
/// rectangular window is optimal (no leakage between tones).
fn goertzel(x: &[Sample], f: f32, rate: f32) -> f32 {
    let w = std::f32::consts::TAU * f / rate;
    let coeff = 2.0 * w.cos();
    let (mut s1, mut s2) = (0.0f32, 0.0f32);
    for &v in x {
        let s0 = coeff * s1 - s2 + v;
        s2 = s1;
        s1 = s0;
    }
    (s1 * s1 + s2 * s2 - coeff * s1 * s2).max(0.0)
}

/// A located sync peak: base-tone bin, start time slot, and Costas metric.
#[derive(Clone, Copy)]
struct SyncPeak {
    f_bin: usize,
    t_slot: usize,
    metric: f32,
}

impl Ft8Demod {
    /// Half-symbol time step → 2 slots per symbol of timing resolution.
    fn slot_step() -> usize {
        FT8_SPS / 2
    }

    /// Build the tone-energy spectrogram `S[slot][bin]`: bin `b` is the power at
    /// `f_lo + b*6.25`; slot `j` covers a full symbol starting at `j*slot_step`.
    fn spectrogram(&self, window: &[Sample]) -> (Vec<Vec<f32>>, usize) {
        let rate = FT8_RATE as f32;
        let nbins = (((self.f_hi - self.f_lo) / FT8_TONE_SPACING).floor() as usize) + 1;
        let step = Self::slot_step();
        let mut s = Vec::new();
        let mut start = 0usize;
        while start + FT8_SPS <= window.len() {
            let seg = &window[start..start + FT8_SPS];
            let row: Vec<f32> =
                (0..nbins).map(|b| goertzel(seg, self.f_lo + b as f32 * FT8_TONE_SPACING, rate)).collect();
            s.push(row);
            start += step;
        }
        (s, nbins)
    }

    /// Extract the 79×8 tone-energy matrix at a given base bin / start slot from
    /// the spectrogram (2 slots per symbol).
    fn energy_at(spec: &[Vec<f32>], f_bin: usize, t_slot: usize) -> Vec<[f32; 8]> {
        let mut e = vec![[0.0f32; 8]; FT8_NSYM];
        for (t, row) in e.iter_mut().enumerate() {
            let slot = t_slot + 2 * t;
            if slot < spec.len() {
                for (k, cell) in row.iter_mut().enumerate() {
                    *cell = spec[slot][f_bin + k];
                }
            }
        }
        e
    }

    /// Score every (base bin, start slot) by the Costas sync metric, sorted by
    /// descending metric.
    fn find_peaks(&self, spec: &[Vec<f32>], nbins: usize) -> Vec<SyncPeak> {
        let corr = CostasCorrelator::ft8();
        let max_slot = spec.len().saturating_sub(2 * (FT8_NSYM - 1) + 1);
        let max_bin = nbins.saturating_sub(8);
        let mut peaks = Vec::with_capacity(max_bin * (max_slot + 1));
        for f_bin in 0..max_bin {
            for t_slot in 0..=max_slot {
                let e = Self::energy_at(spec, f_bin, t_slot);
                let ev: Vec<Vec<f32>> = e.iter().map(|t| t.to_vec()).collect();
                peaks.push(SyncPeak { f_bin, t_slot, metric: corr.metric(&ev, 0, 0) });
            }
        }
        peaks.sort_by(|a, b| b.metric.partial_cmp(&a.metric).unwrap_or(std::cmp::Ordering::Equal));
        peaks
    }
}

impl BlockDemodulator for Ft8Demod {
    fn caps(&self) -> ModeCaps {
        ft8_caps(false)
    }

    fn decode_window(&mut self, window: &[Sample], _window_start_ns: u64) -> Vec<Frame> {
        let (spec, nbins) = self.spectrogram(window);
        if spec.len() < 2 * (FT8_NSYM - 1) + 1 {
            return Vec::new();
        }
        let code = Ldpc::ft8();
        let peaks = self.find_peaks(&spec, nbins);

        let mut out = Vec::new();
        let mut seen_msgs = std::collections::BTreeSet::new();
        let mut tried_pos: Vec<(isize, isize)> = Vec::new();
        let mut attempts = 0usize;

        for p in peaks {
            if out.len() >= self.max_decodes || attempts >= 200 {
                break;
            }
            // Skip near-identical sync positions (a signal's skirts) so we don't
            // burn the attempt budget re-decoding the same frame.
            let key = (p.f_bin as isize, p.t_slot as isize);
            if tried_pos.iter().any(|&(fb, ts)| (fb - key.0).abs() <= 1 && (ts - key.1).abs() <= 1) {
                continue;
            }
            tried_pos.push(key);
            attempts += 1;

            let e = Self::energy_at(&spec, p.f_bin, p.t_slot);
            // Per-window noise normalizer: mean tone power keeps LLRs O(1–10).
            let (mut sum, mut cnt) = (0.0f64, 0usize);
            for row in &e {
                for &v in row {
                    sum += v as f64;
                    cnt += 1;
                }
            }
            let noise_var = ((sum / cnt.max(1) as f64) as f32).max(f32::MIN_POSITIVE);

            // Demap the 58 data symbols (skipping the 21 Costas symbols) → 174 LLRs.
            let mut llrs: Vec<f32> = Vec::with_capacity(N174);
            for (s, tones) in e.iter().enumerate() {
                if is_costas_symbol(s) {
                    continue;
                }
                llrs.extend(demap_fsk_ft8(tones, noise_var));
            }
            if llrs.len() != N174 {
                continue;
            }

            // LDPC min-sum; OSD fallback when parity is unsatisfied.
            let (mut cw, perr) = code.decode_minsum(&llrs, 30);
            if perr != 0 {
                match osd_decode(&code, &llrs, 2) {
                    Some(better) => cw = better,
                    None => continue,
                }
            }

            // Recover the 77 message bits + 14 CRC bits; verify CRC-14.
            let mut payload = [0u8; 10];
            for (i, &b) in cw.iter().take(MSG_BITS).enumerate() {
                payload[i / 8] |= b << (7 - (i % 8));
            }
            let mut rx_crc = 0u16;
            for i in 0..CRC_BITS {
                rx_crc = (rx_crc << 1) | cw[MSG_BITS + i] as u16;
            }
            if ft8_payload_crc(&payload) != rx_crc {
                continue;
            }

            let text = unpack77(&payload);
            if text.is_empty() || !seen_msgs.insert(text.clone()) {
                continue;
            }
            let base_hz = self.f_lo + p.f_bin as f32 * FT8_TONE_SPACING;
            out.push(Frame {
                payload: FramePayload::Text(text),
                meta: FrameMeta {
                    crc_ok: true,
                    freq_offset_hz: Some(base_hz),
                    time_offset_s: Some(p.t_slot as f32 * Self::slot_step() as f32 / FT8_RATE as f32),
                    decoder: Some("ft8".into()),
                    sample_offset: (p.t_slot * Self::slot_step()) as u64,
                    ..Default::default()
                },
            });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbols_have_costas_groups() {
        let syms = ft8_symbols("CQ K1ABC FN42");
        let costas = ft8_costas();
        for &start in &FT8_COSTAS_STARTS {
            for k in 0..7 {
                assert_eq!(syms[start + k], costas[k] as u8, "Costas at {start}+{k}");
            }
        }
    }

    #[test]
    fn modulates_full_waveform() {
        let mut m = Ft8Mod::new();
        let s = m.modulate(&Frame::text("CQ K1ABC FN42")).unwrap();
        assert_eq!(s.len(), FT8_NSYM * FT8_SPS); // 79 × 1920 = 151_680
    }

    #[test]
    fn rejects_packet_payload() {
        let mut m = Ft8Mod::new();
        assert!(matches!(
            m.modulate(&Frame::packet(vec![1, 2, 3])).unwrap_err(),
            ModError::UnsupportedPayload(_)
        ));
    }

    fn padded_window(msg: &str) -> Vec<f32> {
        let wave = Ft8Mod::new().modulate(&Frame::text(msg)).unwrap();
        let mut win = vec![0.0f32; (FT8_RATE as f32 * FT8_WINDOW_S) as usize];
        win[..wave.len()].copy_from_slice(&wave);
        win
    }

    fn decoded_texts(frames: &[Frame]) -> Vec<String> {
        frames
            .iter()
            .filter_map(|f| match &f.payload {
                FramePayload::Text(t) => Some(t.clone()),
                FramePayload::Message77(m) => Some(unpack77(m)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn loopback_decodes_message() {
        let msg = "CQ K1ABC FN42";
        let window = padded_window(msg);
        let decodes = Ft8Demod::new().decode_window(&window, 0);
        let texts = decoded_texts(&decodes);
        assert!(texts.iter().any(|t| t == msg), "decoded: {texts:?}");
    }

    #[test]
    fn loopback_decodes_on_offset_subcarrier() {
        // A signal not on the default 1000 Hz tone-0 must still sync + decode.
        let msg = "W9XYZ K1ABC FN42";
        let wave = Ft8Mod::with_base(1500.0).modulate(&Frame::text(msg)).unwrap();
        let mut win = vec![0.0f32; (FT8_RATE as f32 * FT8_WINDOW_S) as usize];
        win[..wave.len()].copy_from_slice(&wave);
        let decodes = Ft8Demod::new().decode_window(&win, 0);
        assert!(decoded_texts(&decodes).iter().any(|t| t == msg));
    }

    #[test]
    fn loopback_decodes_under_awgn() {
        use crate::testutil::{add_awgn, Rng};
        let msg = "CQ K1ABC FN42";
        let mut window = padded_window(msg);
        let mut rng = Rng::new(20260620);
        add_awgn(&mut window, 0.1, &mut rng);
        let decodes = Ft8Demod::new().decode_window(&window, 0);
        assert!(decoded_texts(&decodes).iter().any(|t| t == msg), "AWGN decode failed");
    }
}
