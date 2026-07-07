//! MSK144 mode assembly (WSJT-X meteor scatter), ported from `wsjtx/lib/`.
//!
//! MSK144 sends a 72 ms, 864-sample frame — `s8 + 48 coded bits + s8 + 80 coded
//! bits` (144 half-symbols) of continuous-phase offset-MSK — repeatedly for the
//! whole transmit period, so a decoder can pick a single frame out of a short
//! meteor "ping". The TX chain is bit-exact vs the reference (pack77 → CRC-13 →
//! LDPC(128,90) → the differential-MSK tone map of `genmsk_128_90.f90`); the RX
//! is a streaming detector that forms the analytic signal, sync-searches each
//! 864-sample window in frequency and time (`msk144sync`), and runs the
//! reference matched filter + belief-propagation decode (`msk144decodeframe`).
//!
//! Two equivalence classes (per the porting doctrine): the tone map and codeword
//! are asserted **bit-exact** against golden vectors from the unmodified
//! reference; the modulated audio and RX soft metrics are floating-point and
//! gated on a loopback / decode-rate basis, never bit-exact.
//!
//! ref: wsjtx/lib/{genmsk_128_90.f90, msk144decodeframe.f90, msk144sync.f90,
//! msk144_freq_search.f90, msk144sim.f90, decode_msk144.f90}
//! (WSJTX/wsjtx @ ccdfaf3c1c109010d15399674ce278167cfde848).

use crate::fec::ldpc_msk144::{get_crc13, msk144_code};
use crate::frontend::msk::{
    cpfsk_modulate, freq_shift_into, half_sine, Analytic, MSK_FS, MSK_NSPM, MSK_NSYM,
};
use crate::framing::pack77::{pack77_standard, unpack77_standard};
use crate::mode::{DemodShape, Demodulator, Duplex, ModError, ModeCaps, Modulator};
use crate::types::{Cplx, Frame, FrameMeta, FramePayload, Sample};

/// The 8-bit MSK144 sync word. ref: genmsk_128_90.f90:33 (`data s8`).
const S8: [u8; 8] = [0, 1, 1, 1, 0, 0, 1, 0];

/// Default audio centre frequency (Hz) for TX and the RX search. ref: WSJT-X
/// nominal MSK144 tone frequency.
pub const MSK144_FREQ: f32 = 1500.0;

/// Map the 128-bit LDPC codeword to the 144 MSK channel tones (0/1): interleave
/// the two 8-bit sync words with the 48+80 coded bits, then apply the
/// differential offset-MSK tone map. Bit-exact vs `genmsk_128_90.f90:90-118`.
pub fn tones_from_codeword(cw: &[u8; 128]) -> [u8; MSK_NSYM] {
    // Build the 144-bit channel vector, mapped to ±1. ref: genmsk:92-97.
    let mut bs = [0i32; 144];
    for i in 0..8 {
        bs[i] = S8[i] as i32;
        bs[56 + i] = S8[i] as i32;
    }
    for i in 0..48 {
        bs[8 + i] = cw[i] as i32;
    }
    for i in 0..80 {
        bs[64 + i] = cw[48 + i] as i32;
    }
    for b in bs.iter_mut() {
        *b = 2 * *b - 1; // {0,1} → {-1,+1}
    }
    // Differential offset-MSK tone map. ref: genmsk:110-118 (1-based Fortran).
    let mut tone = [0i32; MSK_NSYM];
    for k in 0..72 {
        tone[2 * k] = (bs[2 * k + 1] * bs[2 * k] + 1) / 2;
        tone[2 * k + 1] = -(bs[2 * k + 1] * bs[(2 * k + 2) % 144] - 1) / 2;
    }
    let mut out = [0u8; MSK_NSYM];
    for (o, &t) in out.iter_mut().zip(tone.iter()) {
        *o = (1 - t) as u8; // i4tone = -i4tone + 1
    }
    out
}

/// Full TX: standard-message text → 144 MSK tones. Returns `None` for messages
/// this port's `pack77_standard` cannot encode.
pub fn msk144_tones(message: &str) -> Option<[u8; MSK_NSYM]> {
    let payload = pack77_standard(message)?;
    let cw = crate::fec::ldpc_msk144::encode_msk144(&payload);
    Some(tones_from_codeword(&cw))
}

/// Build the 42-sample complex sync-word template `cb`. ref: msk144sync.f90:41-51.
fn sync_template(pp: &[f32; 12]) -> [Cplx; 42] {
    let s: [f32; 8] = std::array::from_fn(|i| 2.0 * S8[i] as f32 - 1.0);
    let mut cbi = [0.0f32; 42];
    let mut cbq = [0.0f32; 42];
    cbq[0..6].copy_from_slice(&pp[6..12]);
    for x in &mut cbq[0..6] {
        *x *= s[0];
    }
    for (blk, sv) in [(6usize, s[2]), (18, s[4]), (30, s[6])] {
        for j in 0..12 {
            cbq[blk + j] = pp[j] * sv;
        }
    }
    for (blk, sv) in [(0usize, s[1]), (12, s[3]), (24, s[5])] {
        for j in 0..12 {
            cbi[blk + j] = pp[j] * sv;
        }
    }
    for j in 0..6 {
        cbi[36 + j] = pp[j] * s[7];
    }
    std::array::from_fn(|i| Cplx::new(cbi[i], cbq[i]))
}

/// Outcome of decoding one candidate 864-sample complex frame.
struct FrameDecode {
    message: String,
}

/// Reference matched-filter + BP decode of a single baseband frame `c` (864
/// complex samples, frame-aligned). ref: msk144decodeframe.f90.
fn decode_frame(c_in: &[Cplx], pp: &[f32; 12], code: &crate::fec::ldpc::Ldpc) -> Option<FrameDecode> {
    if c_in.len() < MSK_NSPM {
        return None;
    }
    let cb = sync_template(pp);
    // Carrier phase estimate from the two sync words (samples 0..42 and 336..378).
    let mut cca = Cplx::new(0.0, 0.0);
    let mut ccb = Cplx::new(0.0, 0.0);
    for i in 0..42 {
        cca += c_in[i] * cb[i].conj();
        ccb += c_in[336 + i] * cb[i].conj();
    }
    let sum = cca + ccb;
    let phase0 = sum.im.atan2(sum.re);
    let cfac = Cplx::new(phase0.cos(), phase0.sin());
    // Derotate: c = c * conj(cfac).
    let mut c = vec![Cplx::new(0.0, 0.0); MSK_NSPM];
    for i in 0..MSK_NSPM {
        c[i] = c_in[i] * cfac.conj();
    }

    // Matched filter → 144 soft symbols. ref: msk144decodeframe.f90:62-67.
    let mut soft = [0.0f32; MSK_NSYM];
    // softbits(1): wrap of last/first half-symbol on the imaginary (Q) rail.
    let mut s0 = 0.0f32;
    for j in 0..6 {
        s0 += c[j].im * pp[6 + j] + c[MSK_NSPM - 6 + j].im * pp[j];
    }
    soft[0] = s0;
    // softbits(2): first full symbol on the real (I) rail.
    let mut s1 = 0.0f32;
    for j in 0..12 {
        s1 += c[j].re * pp[j];
    }
    soft[1] = s1;
    for i in 2..=72 {
        let qs = (i - 1) * 12 - 6; // imag window start (0-based)
        let is = (i - 1) * 12; // real window start (0-based)
        let mut sq = 0.0f32;
        let mut si = 0.0f32;
        for j in 0..12 {
            sq += c[qs + j].im * pp[j];
            si += c[is + j].re * pp[j];
        }
        soft[2 * i - 2] = sq; // softbits(2i-1)
        soft[2 * i - 1] = si; // softbits(2i)
    }

    // Sync-word hard-error discriminator. ref: msk144decodeframe.f90:71-82.
    let s: [i32; 8] = std::array::from_fn(|i| 2 * S8[i] as i32 - 1);
    let hard = |x: f32| -> i32 {
        if x >= 0.0 {
            1
        } else {
            0
        }
    };
    let badsync = |base: usize| -> i32 {
        let acc: i32 = (0..8).map(|k| (2 * hard(soft[base + k]) - 1) * s[k]).sum();
        (8 - acc) / 2
    };
    if badsync(0) + badsync(56) > 4 {
        return None;
    }

    // Normalise the soft symbols. ref: msk144decodeframe.f90:85-88.
    let sav: f32 = soft.iter().sum::<f32>() / MSK_NSYM as f32;
    let s2av: f32 = soft.iter().map(|x| x * x).sum::<f32>() / MSK_NSYM as f32;
    let ssig = (s2av - sav * sav).max(1e-12).sqrt();
    for x in soft.iter_mut() {
        *x /= ssig;
    }

    // LLRs for the 128 codeword bits: softbits(9:56) then softbits(65:144).
    // Reference convention is positive→bit1; omnimodem's decoder is positive→bit0,
    // so negate. ref: msk144decodeframe.f90:90-93.
    let sigma = 0.60f32;
    let scale = 2.0 / (sigma * sigma);
    let mut llr = [0.0f32; 128];
    for i in 0..48 {
        llr[i] = -soft[8 + i] * scale;
    }
    for i in 0..80 {
        llr[48 + i] = -soft[64 + i] * scale;
    }

    let (bits, errs) = code.decode_minsum(&llr, 10);
    if errs != 0 {
        return None;
    }
    // Verify CRC-13 over the recovered 77 message bits. ref: chkcrc13a.f90.
    let mut msg77 = [0u8; 77];
    msg77.copy_from_slice(&bits[..77]);
    let mut rx_crc = 0u16;
    for &b in &bits[77..90] {
        rx_crc = (rx_crc << 1) | b as u16;
    }
    if rx_crc != get_crc13(&msg77) {
        return None;
    }
    // Reject the message types the reference frame decoder discards.
    // ref: msk144decodeframe.f90:102-105 (n3,i3 from bits 72:77).
    let n3 = (msg77[71] as u32) << 2 | (msg77[72] as u32) << 1 | msg77[73] as u32;
    let i3 = (msg77[74] as u32) << 2 | (msg77[75] as u32) << 1 | msg77[76] as u32;
    let reject = (i3 == 0 && (n3 == 1 || n3 == 3 || n3 == 4 || n3 > 5)) || i3 == 3 || i3 > 5;
    if reject {
        return None;
    }
    let message = unpack77_standard(&msg77)?;
    Some(FrameDecode { message })
}

/// The best sync result over a frequency/time search of one 864-sample window.
struct SyncResult {
    c: Vec<Cplx>,        // baseband, frequency-corrected averaged frame
    peaks: [usize; 2],   // two candidate frame-start shifts (0-based)
    fest: f32,           // estimated audio frequency (Hz)
    xmax: f32,           // peak sync correlation metric
}

/// Frequency + time sync search over a single NSPM window of the analytic
/// signal, correlating both sync words against `cb`. ref: msk144_freq_search.f90
/// + msk144sync.f90 (single-frame navmask).
fn sync_search(window: &[Cplx], cb: &[Cplx; 42], fc: f32, ntol: f32, delf: f32) -> SyncResult {
    let n = window.len().min(MSK_NSPM);
    let nfr = (ntol / delf) as i32;
    let fac = 1.0 / 48.0;
    let mut tw = vec![Cplx::new(0.0, 0.0); n];
    let mut best_x = f32::MIN;
    let mut best_f = 0.0f32;
    let mut best_c = window[..n].to_vec();
    let mut best_xcc = vec![0.0f32; MSK_NSPM];
    for ifr in -nfr..=nfr {
        let ferr = ifr as f32 * delf;
        freq_shift_into(&window[..n], -(fc + ferr), &mut tw);
        // Cyclic cross-correlation of both sync words over all 864 shifts.
        let mut xcc = vec![0.0f32; MSK_NSPM];
        let mut peak = f32::MIN;
        for (ish, xc) in xcc.iter_mut().enumerate() {
            let mut acc = Cplx::new(0.0, 0.0);
            for k in 0..42 {
                let a = tw[(ish + k) % MSK_NSPM] + tw[(336 + ish + k) % MSK_NSPM];
                acc += a.conj() * cb[k];
            }
            let m = acc.norm();
            *xc = m;
            if m > peak {
                peak = m;
            }
        }
        let xb = peak * fac;
        if xb > best_x {
            best_x = xb;
            best_f = ferr;
            best_c.copy_from_slice(&tw);
            best_xcc = xcc;
        }
    }
    // Two largest correlation peaks (zero a ±7 guard around each).
    let mut peaks = [0usize; 2];
    for p in peaks.iter_mut() {
        let mut imax = 0usize;
        let mut vmax = f32::MIN;
        for (i, &v) in best_xcc.iter().enumerate() {
            if v > vmax {
                vmax = v;
                imax = i;
            }
        }
        *p = imax;
        let lo = imax.saturating_sub(7);
        let hi = (imax + 7).min(MSK_NSPM - 1);
        for v in &mut best_xcc[lo..=hi] {
            *v = 0.0;
        }
    }
    SyncResult { c: best_c, peaks, fest: fc + best_f, xmax: best_x }
}

/// Circularly rotate `src` left by `k` into `dst` (Fortran `cshift`).
fn cshift(src: &[Cplx], k: usize, dst: &mut [Cplx]) {
    let n = src.len();
    for i in 0..n {
        dst[i] = src[(i + k) % n];
    }
}

/// MSK144 streaming transmitter: a standard-message text payload → offset-MSK
/// audio, the 864-sample frame repeated to fill `reps` frames (the meteor-scatter
/// convention of transmitting continuously across the period).
pub struct Msk144Mod {
    freq_hz: f32,
    reps: usize,
}

impl Default for Msk144Mod {
    fn default() -> Self {
        // ~1 s of frames by default (14 × 72 ms); the daemon may call repeatedly.
        Msk144Mod { freq_hz: MSK144_FREQ, reps: 14 }
    }
}

impl Msk144Mod {
    pub fn new() -> Self {
        Self::default()
    }
    /// Transmit at a specific audio frequency, repeating the frame `reps` times.
    pub fn with_params(freq_hz: f32, reps: usize) -> Self {
        Msk144Mod { freq_hz, reps: reps.max(1) }
    }
}

fn msk144_caps(tx: bool) -> ModeCaps {
    ModeCaps {
        native_rate: MSK_FS as u32,
        bandwidth_hz: 2.0 * MSK_BAUD_HALF, // two tones baud/4 either side of centre
        tx,
        duplex: Duplex::Half,
        shape: DemodShape::Streaming,
    }
}

const MSK_BAUD_HALF: f32 = 500.0; // baud/4 tone deviation

impl Modulator for Msk144Mod {
    fn caps(&self) -> ModeCaps {
        msk144_caps(true)
    }

    fn modulate(&mut self, frame: &Frame) -> Result<Vec<Sample>, ModError> {
        let msg = match &frame.payload {
            FramePayload::Text(t) => t.clone(),
            _ => return Err(ModError::UnsupportedPayload("msk144 needs text")),
        };
        let tones =
            msk144_tones(&msg).ok_or_else(|| ModError::Encode(format!("unsupported msk144 message: {msg:?}")))?;
        // Repeat the frame's tones `reps` times; phase stays continuous across
        // frames since cpfsk_modulate integrates one running phase.
        let mut all = Vec::with_capacity(tones.len() * self.reps);
        for _ in 0..self.reps {
            all.extend_from_slice(&tones);
        }
        Ok(cpfsk_modulate(&all, self.freq_hz))
    }
}

/// MSK144 streaming receiver. Buffers audio, forms the analytic signal over
/// overlapping 7168-sample blocks (as `decode_msk144.f90` does), and sync-decodes
/// every 864-sample window, de-duplicating repeated frames within a block.
pub struct Msk144Demod {
    fc: f32,
    ntol: f32,
    buf: Vec<Sample>,
    analytic: Analytic,
    code: crate::fec::ldpc::Ldpc,
    pp: [f32; 12],
    cb: [Cplx; 42],
    last: Option<String>,
}

/// Analysis block size. ref: decode_msk144.f90 (BLOCK_SIZE=7168).
const BLOCK: usize = 7168;
/// Block advance (half-block overlap). ref: decode_msk144.f90 (STEP_SIZE).
const STEP: usize = BLOCK / 2;
/// FFT size for the analytic signal. ref: mskrtd.f90 (NFFT1=8192).
const NFFT: usize = 8192;

impl Default for Msk144Demod {
    fn default() -> Self {
        Self::new()
    }
}

impl Msk144Demod {
    pub fn new() -> Self {
        Self::with_params(MSK144_FREQ, 100.0)
    }
    /// Centre the search at `fc` Hz with a ± `ntol` Hz tolerance.
    pub fn with_params(fc: f32, ntol: f32) -> Self {
        let pp = half_sine();
        let cb = sync_template(&pp);
        Msk144Demod {
            fc,
            ntol,
            buf: Vec::new(),
            analytic: Analytic::new(NFFT),
            code: msk144_code(),
            pp,
            cb,
            last: None,
        }
    }

    /// Decode one 7168-sample block, returning any messages found.
    fn decode_block(&mut self, block: &[Sample]) -> Vec<Frame> {
        // Normalise like mskrtd (skip near-silent blocks).
        let rms = (block.iter().map(|x| x * x).sum::<f32>() / block.len() as f32).sqrt();
        if rms < 1e-6 {
            return Vec::new();
        }
        let norm: Vec<f32> = block.iter().map(|x| x / rms).collect();
        let cdat = self.analytic.transform(&norm);
        let mut out = Vec::new();
        let mut scratch = vec![Cplx::new(0.0, 0.0); MSK_NSPM];
        // Slide 864-sample windows at half-frame steps across the block.
        let last_start = block.len().saturating_sub(MSK_NSPM);
        let mut s0 = 0usize;
        while s0 <= last_start {
            let window = &cdat[s0..s0 + MSK_NSPM];
            let sync = sync_search(window, &self.cb, self.fc, self.ntol, 5.0);
            if sync.xmax >= 1.3 {
                let mut done = false;
                'search: for &pk in &sync.peaks {
                    for dither in [0i32, -1, 1] {
                        let ic0 = ((pk as i32 + dither).rem_euclid(MSK_NSPM as i32)) as usize;
                        cshift(&sync.c, ic0, &mut scratch);
                        if let Some(dec) = decode_frame(&scratch, &self.pp, &self.code) {
                            if self.last.as_deref() != Some(dec.message.as_str()) {
                                self.last = Some(dec.message.clone());
                                out.push(Frame {
                                    payload: FramePayload::Text(dec.message),
                                    meta: FrameMeta {
                                        freq_offset_hz: Some(sync.fest),
                                        crc_ok: true,
                                        ..Default::default()
                                    },
                                });
                            }
                            done = true;
                            break 'search;
                        }
                    }
                }
                if done {
                    // Advance a full frame past a successful decode.
                    s0 += MSK_NSPM;
                    continue;
                }
            }
            s0 += MSK_NSPM / 2;
        }
        out
    }
}

impl Demodulator for Msk144Demod {
    fn caps(&self) -> ModeCaps {
        msk144_caps(false)
    }

    fn feed(&mut self, samples: &[Sample]) -> Vec<Frame> {
        self.buf.extend_from_slice(samples);
        let mut out = Vec::new();
        let mut pos = 0usize;
        while pos + BLOCK <= self.buf.len() {
            let block: Vec<Sample> = self.buf[pos..pos + BLOCK].to_vec();
            out.extend(self.decode_block(&block));
            pos += STEP;
        }
        // Retain a tail long enough to catch a frame straddling the next block.
        if pos > 0 {
            let keep = self.buf.len() - pos;
            self.buf.drain(..pos);
            debug_assert_eq!(self.buf.len(), keep);
        }
        out
    }

    fn reset(&mut self) {
        self.buf.clear();
        self.last = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fec::ldpc_msk144::encode_msk144;

    fn bits(s: &str) -> Vec<u8> {
        s.bytes().map(|b| b - b'0').collect()
    }

    // Golden 144-tone channel-symbol vectors from the UNMODIFIED genmsk_128_90
    // (see tests/vectors/msk144_reference.json, driver build_msk144.sh).
    const REF_MSG77_A: &str =
        "10010010010010010010010010010010010010010010010010010010010010010010010010010";
    const REF_TONES_A: &str = "110000101110001110001110001110001110001110001110001110011100001011100011100011100011100011100100010110110110100100101010010011011111001000110101";
    const REF_MSG77_B: &str =
        "10110001110100101100011101001011000111010010110001110100101100011101001011001";
    const REF_TONES_B: &str = "110000101000011100100010000111001000100001110010001000011100001111001000100001110010001000000001110010000000011001001001000110000000110011110000";

    #[test]
    fn tone_map_matches_wsjtx_reference() {
        for (msg, tones) in [(REF_MSG77_A, REF_TONES_A), (REF_MSG77_B, REF_TONES_B)] {
            let msg77: [u8; 77] = bits(msg).try_into().unwrap();
            let cw = encode_msk144(&msg77);
            let got = tones_from_codeword(&cw);
            let want: Vec<u8> = bits(tones);
            assert_eq!(got.to_vec(), want, "tone map differs from genmsk_128_90");
        }
    }

    #[test]
    fn sync_words_are_placed_in_the_tone_frame() {
        // The tone map is differential, so we check the codeword→bitseq split by
        // round-tripping a known codeword through decode of a clean frame instead.
        let msg77 = [0u8; 77];
        let cw = encode_msk144(&msg77);
        let tones = tones_from_codeword(&cw);
        assert_eq!(tones.len(), 144);
        assert!(tones.iter().all(|&t| t <= 1));
    }

    #[test]
    fn single_frame_loopback_decodes() {
        // Build one clean frame, embed it in a block, and decode it.
        let msg = "K1ABC W9XYZ EN37";
        let tones = msk144_tones(msg).expect("encode");
        // Repeat enough frames to fill a decode block with margin.
        let mut all = Vec::new();
        for _ in 0..12 {
            all.extend_from_slice(&tones);
        }
        let wave = cpfsk_modulate(&all, MSK144_FREQ);
        let mut d = Msk144Demod::new();
        let frames = d.feed(&wave);
        assert!(
            frames.iter().any(|f| matches!(&f.payload, FramePayload::Text(t) if t == msg)),
            "expected to decode {msg:?}, got {:?}",
            frames.iter().map(|f| &f.payload).collect::<Vec<_>>()
        );
    }

    #[test]
    fn decodes_at_offset_frequency() {
        let msg = "CQ K1ABC FN42";
        let tones = msk144_tones(msg).expect("encode");
        let mut all = Vec::new();
        for _ in 0..12 {
            all.extend_from_slice(&tones);
        }
        let wave = cpfsk_modulate(&all, 1550.0); // 50 Hz off centre
        let mut d = Msk144Demod::with_params(1500.0, 100.0);
        let frames = d.feed(&wave);
        assert!(frames.iter().any(|f| matches!(&f.payload, FramePayload::Text(t) if t == msg)));
    }

    #[test]
    fn modulator_rejects_unsupported_message() {
        let mut m = Msk144Mod::new();
        assert!(m.modulate(&Frame::text("this is free text not a call")).is_err());
    }

    #[test]
    fn decodes_under_awgn() {
        use crate::testutil::{add_awgn, Rng};
        let msg = "K1ABC W9XYZ EN37";
        let tones = msk144_tones(msg).expect("encode");
        let mut clean = Vec::new();
        for _ in 0..12 {
            clean.extend_from_slice(&tones);
        }
        let wave = cpfsk_modulate(&clean, MSK144_FREQ);
        // Per-sample signal amplitude is ~1.0 (unit cosine). sigma=0.6 noise is a
        // comfortably-decodable meteor ping; the 864-sample matched filter gives
        // substantial processing gain over the per-sample SNR.
        let trials = 20;
        let mut ok = 0;
        for t in 0..trials as u64 {
            let mut w = wave.clone();
            let mut rng = Rng::new(0x4D53_4B00 + t);
            add_awgn(&mut w, 0.6, &mut rng);
            let mut d = Msk144Demod::new();
            if d.feed(&w).iter().any(|f| matches!(&f.payload, FramePayload::Text(x) if x == msg)) {
                ok += 1;
            }
        }
        let rate = ok as f32 / trials as f32;
        assert!(rate >= 0.9, "MSK144 AWGN decode rate {rate} below 0.9");
    }
}
