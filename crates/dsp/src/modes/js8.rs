//! JS8 (JS8Call) — 8-GFSK windowed mode built on the early-FT8 core.
//!
//! Port of js8call (upstream `js8call/js8call` @ a7ff1be). JS8 shares FT8's
//! 79-symbol, 8-tone frame but uses the LDPC(174,87)/CRC-12 channel code
//! ([`crate::fec::ldpc_js8`]) and its own Costas arrays. The four on-air
//! submodes **Normal / Fast / Turbo / Slow** (plus the disabled **Ultra**) are
//! one parametric family differing only in samples-per-symbol, tone spacing,
//! T-R period, and which Costas variant they use.
//!
//! This module currently provides the submode parameter table, the Costas
//! arrays, and the **TX channel-symbol assembly** (`js8_symbols`, bit-exact vs
//! the reference `genjs8`). The `Modulator`/`BlockDemodulator` waveform + decode
//! and daemon/TUI registration are assembled on top of this foundation.
//!
//! ref: js8call/lib/js8/genjs8.f90 (tone assembly + Costas), JS8Submode.cpp +
//! commons.h + lib/js8/js8{a,b,c,e,i}_params.f90 (submode grid).

use crate::fec::ldpc_js8::{encode174, extract_message, js8_174_87_code};
use crate::fec::llr::demap_fsk_identity;
use crate::fec::osd::osd_decode;
use crate::framing::js8_frames::build_directed;
use crate::framing::js8_message::{
    decode_frame, pack_fast_data, pack_frame, JS8_DATA, JS8_FIRST, JS8_LAST,
};
use crate::frontend::modulate::Gfsk;
use crate::mode::{BlockDemodulator, DemodShape, Duplex, ModError, ModeCaps, Modulator};
use crate::types::{Frame, FrameMeta, FramePayload, Sample};

/// Default audio sub-carrier (tone-0 frequency) for TX.
pub const JS8_BASE_HZ: f32 = 1500.0;
/// LDPC codeword length.
const N174: usize = 174;

/// Sample rate (all submodes). ref: commons.h `JS8_RX_SAMPLE_RATE`.
pub const JS8_RATE: u32 = 12_000;
/// Total channel symbols: 21 sync (3×7 Costas) + 58 data. ref: genjs8.f90 `NN`.
pub const JS8_NSYM: usize = 79;
/// Data symbols (each 3 LDPC codeword bits). ref: genjs8.f90 `ND`.
pub const JS8_ND: usize = 58;
/// Costas group start positions within the 79-symbol frame: 0–6, 36–42, 72–78.
pub const JS8_COSTAS_STARTS: [usize; 3] = [0, 36, 72];

/// Original 7×7 Costas array (JS8 Normal, `NCOSTAS=1` — same array in all three
/// groups). ref: genjs8.f90:23-25.
pub const JS8_COSTAS_ORIG: [u8; 7] = [4, 2, 5, 6, 1, 3, 0];
/// Symmetrical Costas arrays A/B/C (JS8 Fast/Turbo/Slow/Ultra, `NCOSTAS=2` —
/// distinct arrays per group). ref: genjs8.f90:27-31.
pub const JS8_COSTAS_SYM_A: [u8; 7] = [0, 6, 2, 3, 5, 4, 1];
pub const JS8_COSTAS_SYM_B: [u8; 7] = [1, 5, 0, 2, 3, 6, 4];
pub const JS8_COSTAS_SYM_C: [u8; 7] = [2, 5, 0, 6, 4, 1, 3];

/// JS8 submodes. Discriminants match `Varicode::SubmodeType`. ref: varicode.h:26.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Js8Submode {
    Normal = 0,
    Fast = 1,
    Turbo = 2,
    Slow = 4,
    /// Defined in the reference but disabled in its calling code; ported for
    /// completeness, not registered as a selectable mode.
    Ultra = 8,
}

/// Per-submode constant parameters. ref: JS8Submode.cpp + `js8{a,b,c,e,i}_params.f90`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Js8Params {
    pub name: &'static str,
    /// Samples per symbol at 12 kHz (`NSPS`).
    pub nsps: usize,
    /// Nominal T-R period / transmission length, seconds (`NTXDUR`).
    pub tx_seconds: u32,
    /// TX start delay, ms (`ASTART`).
    pub start_delay_ms: u32,
    /// Tone spacing = 12000 / nsps, Hz.
    pub tone_spacing: f64,
    /// Costas variant: `false` → original (Normal), `true` → symmetrical.
    pub symmetrical_costas: bool,
}

impl Js8Submode {
    pub fn params(self) -> Js8Params {
        match self {
            // name, nsps, tx_seconds, start_delay_ms, tone_spacing, symmetrical
            Js8Submode::Normal => Js8Params { name: "NORMAL", nsps: 1920, tx_seconds: 15, start_delay_ms: 500, tone_spacing: 12000.0 / 1920.0, symmetrical_costas: false },
            Js8Submode::Fast => Js8Params { name: "FAST", nsps: 1200, tx_seconds: 10, start_delay_ms: 200, tone_spacing: 12000.0 / 1200.0, symmetrical_costas: true },
            Js8Submode::Turbo => Js8Params { name: "TURBO", nsps: 600, tx_seconds: 6, start_delay_ms: 100, tone_spacing: 12000.0 / 600.0, symmetrical_costas: true },
            Js8Submode::Slow => Js8Params { name: "SLOW", nsps: 3840, tx_seconds: 30, start_delay_ms: 500, tone_spacing: 12000.0 / 3840.0, symmetrical_costas: true },
            Js8Submode::Ultra => Js8Params { name: "ULTRA", nsps: 384, tx_seconds: 4, start_delay_ms: 100, tone_spacing: 12000.0 / 384.0, symmetrical_costas: true },
        }
    }

    /// The three 7-symbol Costas arrays for this submode's frame groups.
    pub fn costas(self) -> [[u8; 7]; 3] {
        if self.params().symmetrical_costas {
            [JS8_COSTAS_SYM_A, JS8_COSTAS_SYM_B, JS8_COSTAS_SYM_C]
        } else {
            [JS8_COSTAS_ORIG; 3]
        }
    }
}

/// Assemble the 79 channel-symbol tones (0–7) from an 87-bit message, bit-exact
/// with `genjs8`: LDPC-encode → three Costas groups at 0/36/72 → 58 data tones
/// via the **plain-binary** 3-bit map `cw[3j]·4 + cw[3j+1]·2 + cw[3j+2]`. Frame
/// layout `S7 D29 S7 D29 S7`. ref: genjs8.f90:44-56.
pub fn js8_symbols(msgbits: &[u8; 87], submode: Js8Submode) -> [u8; JS8_NSYM] {
    let cw = encode174(msgbits);
    let costas = submode.costas();
    let mut itone = [0u8; JS8_NSYM];
    for (g, &start) in JS8_COSTAS_STARTS.iter().enumerate() {
        itone[start..start + 7].copy_from_slice(&costas[g]);
    }
    for j in 0..JS8_ND {
        // First 29 data symbols occupy 7..36, next 29 occupy 43..72.
        let pos = if j < 29 { 7 + j } else { 43 + (j - 29) };
        let b = 3 * j;
        itone[pos] = cw[b] * 4 + cw[b + 1] * 2 + cw[b + 2];
    }
    itone
}

// ---------------------------------------------------------------------------
// Transmit
// ---------------------------------------------------------------------------

/// JS8 modulator for one submode. Accepts a `Text` payload, JSC-compresses it
/// into a fast-data frame (`JS8_DATA`), builds the 79 tones, and shapes them
/// with GFSK at the submode's rate. Text longer than one frame is truncated to
/// what fits; multi-frame transmissions are the daemon's job (this is one
/// window). ref: varicode.cpp `packFastDataMessage`.
pub struct Js8Mod {
    submode: Js8Submode,
    base_hz: f32,
    /// The station's own callsign. When set, a `Text` payload that parses as a
    /// directed message (`W1AW SNR?`) is sent as a directed frame; otherwise it
    /// falls back to a JSC free-text data frame.
    mycall: Option<String>,
}

impl Js8Mod {
    pub fn new(submode: Js8Submode) -> Self {
        Js8Mod { submode, base_hz: JS8_BASE_HZ, mycall: None }
    }
    pub fn with_base(submode: Js8Submode, base_hz: f32) -> Self {
        Js8Mod { submode, base_hz, mycall: None }
    }
    /// Set the station callsign so directed messages can be composed.
    pub fn with_call(submode: Js8Submode, mycall: impl Into<String>) -> Self {
        Js8Mod { submode, base_hz: JS8_BASE_HZ, mycall: Some(mycall.into()) }
    }
}

fn js8_caps(submode: Js8Submode, tx: bool) -> ModeCaps {
    let p = submode.params();
    ModeCaps {
        native_rate: JS8_RATE,
        bandwidth_hz: (8.0 * p.tone_spacing) as f32,
        tx,
        duplex: Duplex::Half,
        shape: DemodShape::Windowed { window_s: p.tx_seconds as f32, period_s: p.tx_seconds as f32 },
    }
}

impl Modulator for Js8Mod {
    fn caps(&self) -> ModeCaps {
        js8_caps(self.submode, true)
    }

    fn modulate(&mut self, frame: &Frame) -> Result<Vec<Sample>, ModError> {
        let text = match &frame.payload {
            FramePayload::Text(t) => t.clone(),
            _ => return Err(ModError::UnsupportedPayload("js8 needs text")),
        };
        // If we have a callsign and the text is a directed message, send a
        // directed frame; otherwise one JSC fast-data frame (first = last for a
        // single window).
        let msgbits = self
            .mycall
            .as_deref()
            .and_then(|call| build_directed(call, &text, true, true))
            .map(|(payload, i3)| pack_frame(&payload, i3))
            .unwrap_or_else(|| {
                let (payload, _chars) = pack_fast_data(&text);
                pack_frame(&payload, JS8_DATA | JS8_FIRST | JS8_LAST)
            });
        let syms = js8_symbols(&msgbits, self.submode);
        let sym_u32: Vec<u32> = syms.iter().map(|&s| s as u32).collect();
        let p = self.submode.params();
        let gfsk = Gfsk::new(JS8_RATE as f32, p.nsps, self.base_hz, p.tone_spacing as f32, 2.0);
        Ok(gfsk.modulate(&sym_u32))
    }
}

// ---------------------------------------------------------------------------
// Receive
// ---------------------------------------------------------------------------

/// Single-bin power via Goertzel over `x` at frequency `f`. JS8 tones (spaced
/// `12000/nsps` over an `nsps`-sample symbol at 12 kHz) are exactly orthogonal,
/// so a rectangular window is optimal.
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

#[inline]
fn is_costas_symbol(s: usize) -> bool {
    JS8_COSTAS_STARTS.iter().any(|&start| (start..start + 7).contains(&s))
}

/// Block (windowed) JS8 demodulator for one submode.
pub struct Js8Demod {
    submode: Js8Submode,
    f_lo: f32,
    f_hi: f32,
    max_decodes: usize,
}

impl Js8Demod {
    pub fn new(submode: Js8Submode) -> Self {
        Js8Demod { submode, f_lo: 200.0, f_hi: 3000.0, max_decodes: 16 }
    }
    pub fn window_s(&self) -> f32 {
        self.submode.params().tx_seconds as f32
    }

    fn slot_step(&self) -> usize {
        self.submode.params().nsps / 2 // 2 slots/symbol
    }

    /// Tone-energy spectrogram `S[slot][bin]`: bin `b` = power at
    /// `f_lo + b*spacing`; slot `j` covers a symbol starting at `j*slot_step`.
    fn spectrogram(&self, window: &[Sample]) -> (Vec<Vec<f32>>, usize) {
        let rate = JS8_RATE as f32;
        let p = self.submode.params();
        let spacing = p.tone_spacing as f32;
        let nsps = p.nsps;
        let nbins = (((self.f_hi - self.f_lo) / spacing).floor() as usize) + 1;
        let step = nsps / 2;
        let mut s = Vec::new();
        let mut start = 0usize;
        while start + nsps <= window.len() {
            let seg = &window[start..start + nsps];
            let row: Vec<f32> =
                (0..nbins).map(|b| goertzel(seg, self.f_lo + b as f32 * spacing, rate)).collect();
            s.push(row);
            start += step;
        }
        (s, nbins)
    }

    /// The 79×8 tone-energy matrix at a given base bin / start slot.
    fn energy_at(spec: &[Vec<f32>], f_bin: usize, t_slot: usize) -> Vec<[f32; 8]> {
        let mut e = vec![[0.0f32; 8]; JS8_NSYM];
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
}

/// Sync metric: summed energy at the three Costas groups' tones (per-group
/// arrays support the symmetrical variant). `energy[t][k]`.
fn sync_metric(energy: &[[f32; 8]], costas: &[[u8; 7]; 3]) -> f32 {
    let mut sum = 0.0f32;
    for (g, &start) in JS8_COSTAS_STARTS.iter().enumerate() {
        for (i, &tone) in costas[g].iter().enumerate() {
            sum += energy[start + i][tone as usize];
        }
    }
    sum
}

impl BlockDemodulator for Js8Demod {
    fn caps(&self) -> ModeCaps {
        js8_caps(self.submode, false)
    }

    fn decode_window(&mut self, window: &[Sample], _window_start_ns: u64) -> Vec<Frame> {
        let (spec, nbins) = self.spectrogram(window);
        if spec.len() < 2 * (JS8_NSYM - 1) + 1 {
            return Vec::new();
        }
        let code = js8_174_87_code();
        let costas = self.submode.costas();
        let spacing = self.submode.params().tone_spacing as f32;

        // Score every (base bin, start slot) by the Costas sync metric.
        let max_slot = spec.len().saturating_sub(2 * (JS8_NSYM - 1) + 1);
        let max_bin = nbins.saturating_sub(8);
        let mut peaks: Vec<(usize, usize, f32)> = Vec::with_capacity(max_bin.max(1) * (max_slot + 1));
        for f_bin in 0..max_bin {
            for t_slot in 0..=max_slot {
                let e = Js8Demod::energy_at(&spec, f_bin, t_slot);
                peaks.push((f_bin, t_slot, sync_metric(&e, &costas)));
            }
        }
        peaks.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));

        let mut out = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        let mut tried: Vec<(isize, isize)> = Vec::new();
        let mut attempts = 0usize;
        for (f_bin, t_slot, _m) in peaks {
            if out.len() >= self.max_decodes || attempts >= 200 {
                break;
            }
            let key = (f_bin as isize, t_slot as isize);
            if tried.iter().any(|&(fb, ts)| (fb - key.0).abs() <= 1 && (ts - key.1).abs() <= 1) {
                continue;
            }
            tried.push(key);
            attempts += 1;

            let e = Js8Demod::energy_at(&spec, f_bin, t_slot);
            let (mut sum, mut cnt) = (0.0f64, 0usize);
            for row in &e {
                for &v in row {
                    sum += v as f64;
                    cnt += 1;
                }
            }
            let noise_var = ((sum / cnt.max(1) as f64) as f32).max(f32::MIN_POSITIVE);

            // Demap the 58 data symbols (skip the 21 Costas symbols) → 174 LLRs,
            // plain-binary (identity) tone→symbol map.
            let mut llrs: Vec<f32> = Vec::with_capacity(N174);
            for (s, tones) in e.iter().enumerate() {
                if is_costas_symbol(s) {
                    continue;
                }
                llrs.extend(demap_fsk_identity(tones, noise_var));
            }
            if llrs.len() != N174 {
                continue;
            }

            let (mut cw, perr) = code.decode_minsum(&llrs, 30);
            if perr != 0 {
                match osd_decode(&code, &llrs, 2) {
                    Some(better) => cw = better,
                    None => continue,
                }
            }
            let msgbits = extract_message(&cw);
            let (frame, _i3) = match decode_frame(&msgbits) {
                Some(v) => v,
                None => continue,
            };
            let text = frame.display();
            if text.is_empty() || !seen.insert(text.clone()) {
                continue;
            }
            let base_hz = self.f_lo + f_bin as f32 * spacing;
            out.push(Frame {
                payload: FramePayload::Text(text),
                meta: FrameMeta {
                    crc_ok: true,
                    freq_offset_hz: Some(base_hz),
                    time_offset_s: Some(t_slot as f32 * self.slot_step() as f32 / JS8_RATE as f32),
                    decoder: Some("js8".into()),
                    sample_offset: (t_slot * self.slot_step()) as u64,
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

    fn parse_vec(field: &str, raw: &str) -> Vec<u8> {
        let i = raw.find(field).unwrap() + field.len();
        let s = &raw[i..raw[i..].find('"').unwrap() + i];
        s.split_whitespace().map(|t| t.parse().unwrap()).collect()
    }
    fn parse_bits(field: &str, raw: &str) -> Vec<u8> {
        let i = raw.find(field).unwrap() + field.len();
        raw[i..raw[i..].find('"').unwrap() + i].bytes().map(|c| c - b'0').collect()
    }

    /// Bit-exact: `js8_symbols` reproduces the reference `genjs8` tone sequence
    /// for both Costas variants. Provenance: `tests/vectors/js8_symbols.json`
    /// (js8call @ a7ff1be, driver `scratch/refvectors/js8/build_ldpc.sh`).
    #[test]
    fn js8_symbols_match_reference() {
        let raw = include_str!("../../tests/vectors/js8_symbols.json");
        let msgbits = parse_bits("\"msgbits\": \"", raw);
        let mut m = [0u8; 87];
        m.copy_from_slice(&msgbits);
        let orig = parse_vec("\"itone_orig\": \"", raw);
        let sym = parse_vec("\"itone_sym\": \"", raw);

        assert_eq!(js8_symbols(&m, Js8Submode::Normal).to_vec(), orig, "Normal (original Costas) itone mismatch");
        // Fast/Turbo/Slow/Ultra all share the symmetrical Costas + the same data map.
        for sm in [Js8Submode::Fast, Js8Submode::Turbo, Js8Submode::Slow, Js8Submode::Ultra] {
            assert_eq!(js8_symbols(&m, sm).to_vec(), sym, "{:?} (symmetrical Costas) itone mismatch", sm);
        }
    }

    /// Costas groups sit at 0/36/72 and only they differ between variants; the
    /// 58 data tones are identical across all submodes.
    #[test]
    fn data_tones_shared_costas_differ() {
        let m = [1u8; 87];
        let normal = js8_symbols(&m, Js8Submode::Normal);
        let fast = js8_symbols(&m, Js8Submode::Fast);
        for j in 0..JS8_NSYM {
            let in_costas = JS8_COSTAS_STARTS.iter().any(|&s| (s..s + 7).contains(&j));
            if in_costas {
                continue; // may differ
            }
            assert_eq!(normal[j], fast[j], "data tone {j} differs between submodes");
        }
        // Costas groups are exactly the declared arrays.
        assert_eq!(&normal[0..7], &JS8_COSTAS_ORIG);
        assert_eq!(&fast[0..7], &JS8_COSTAS_SYM_A);
        assert_eq!(&fast[36..43], &JS8_COSTAS_SYM_B);
        assert_eq!(&fast[72..79], &JS8_COSTAS_SYM_C);
    }

    /// End-to-end loopback: modulate a message to GFSK audio and recover it
    /// through the full RX chain (spectrogram → JS8 Costas sync → plain-binary
    /// soft demap → LDPC(174,87) BP+OSD → CRC-12 → unpack). Proves the waveform,
    /// sync, demod, decode, and pack/unpack agree.
    #[test]
    fn loopback_normal() {
        // 12-char message over the JS8 alphabet (no padding ambiguity).
        let msg = "CQ K1ABC";
        let wave = Js8Mod::new(Js8Submode::Normal).modulate(&Frame::text(msg)).unwrap();
        assert_eq!(wave.len(), JS8_NSYM * Js8Submode::Normal.params().nsps);
        // Pad to a full window as the daemon would present it.
        let win_len = (JS8_RATE as f32 * Js8Submode::Normal.params().tx_seconds as f32) as usize;
        let mut win = vec![0.0f32; win_len.max(wave.len())];
        win[..wave.len()].copy_from_slice(&wave);
        let decodes = Js8Demod::new(Js8Submode::Normal).decode_window(&win, 0);
        let texts: Vec<String> = decodes
            .iter()
            .filter_map(|f| match &f.payload {
                FramePayload::Text(t) => Some(t.clone()),
                _ => None,
            })
            .collect();
        assert!(texts.iter().any(|t| t == msg), "JS8 loopback failed; got {texts:?}");
    }

    fn loopback_ok(submode: Js8Submode, msg: &str, noise: Option<f32>) -> bool {
        let wave = Js8Mod::new(submode).modulate(&Frame::text(msg)).unwrap();
        let win_len = (JS8_RATE as f32 * submode.params().tx_seconds as f32) as usize;
        let mut win = vec![0.0f32; win_len.max(wave.len())];
        win[..wave.len()].copy_from_slice(&wave);
        if let Some(sigma) = noise {
            use crate::testutil::{add_awgn, Rng};
            let mut rng = Rng::new(20260707);
            add_awgn(&mut win, sigma, &mut rng);
        }
        Js8Demod::new(submode)
            .decode_window(&win, 0)
            .iter()
            .any(|f| matches!(&f.payload, FramePayload::Text(t) if t == msg))
    }

    /// Loopback through the **Fast** submode exercises the symmetrical Costas
    /// arrays and the parametric spectrogram (nsps=1200, spacing=10 Hz).
    #[test]
    fn loopback_fast() {
        assert!(loopback_ok(Js8Submode::Fast, "FAST QSO", None), "Fast loopback failed");
    }

    /// Turbo (nsps=600, 20-baud, symmetrical Costas) round-trips end to end.
    #[test]
    fn loopback_turbo() {
        assert!(loopback_ok(Js8Submode::Turbo, "TURBO", None), "Turbo loopback failed");
    }

    /// Slow (nsps=3840, 3.125-baud, symmetrical Costas) round-trips end to end.
    #[test]
    fn loopback_slow() {
        assert!(loopback_ok(Js8Submode::Slow, "SLOW", None), "Slow loopback failed");
    }

    /// Full multi-frame data transport through audio: a message longer than one
    /// frame is JSC-split, each frame modulated and decoded through Turbo audio,
    /// then reassembled to the original. Exercises the FrameData path end to end.
    #[test]
    fn multi_frame_audio_roundtrip() {
        use crate::framing::js8_message::pack_data_frames;
        let message = "CQ CQ DE K1ABC PSE QSL 73 GL";
        let frames = pack_data_frames(message);
        assert!(frames.len() > 1, "expected a multi-frame message, got {}", frames.len());
        let sm = Js8Submode::Turbo;
        let p = sm.params();
        let mut got = String::new();
        for (i, payload) in frames.iter().enumerate() {
            let i3 = JS8_DATA
                | if i == 0 { JS8_FIRST } else { 0 }
                | if i + 1 == frames.len() { JS8_LAST } else { 0 };
            let msgbits = pack_frame(payload, i3);
            let syms = js8_symbols(&msgbits, sm);
            let gfsk = Gfsk::new(JS8_RATE as f32, p.nsps, JS8_BASE_HZ, p.tone_spacing as f32, 2.0);
            let wave = gfsk.modulate(&syms.iter().map(|&s| s as u32).collect::<Vec<_>>());
            let win_len = (JS8_RATE as f32 * p.tx_seconds as f32) as usize;
            let mut win = vec![0.0f32; win_len.max(wave.len())];
            win[..wave.len()].copy_from_slice(&wave);
            let decodes = Js8Demod::new(sm).decode_window(&win, 0);
            let frame_text = decodes
                .iter()
                .find_map(|f| match &f.payload {
                    FramePayload::Text(t) => Some(t.clone()),
                    _ => None,
                })
                .expect("frame did not decode");
            got.push_str(&frame_text);
        }
        assert_eq!(got, message, "reassembled multi-frame message mismatch");
    }

    /// `Js8Mod::with_call` composes a directed frame from operator text and it
    /// decodes through audio to the rendered directed message.
    #[test]
    fn modulator_sends_directed_from_text() {
        let sm = Js8Submode::Turbo;
        let wave = Js8Mod::with_call(sm, "K1ABC")
            .modulate(&Frame::text("W1AW SNR?"))
            .unwrap();
        let win_len = (JS8_RATE as f32 * sm.params().tx_seconds as f32) as usize;
        let mut win = vec![0.0f32; win_len.max(wave.len())];
        win[..wave.len()].copy_from_slice(&wave);
        let decodes = Js8Demod::new(sm).decode_window(&win, 0);
        assert!(
            decodes
                .iter()
                .any(|f| matches!(&f.payload, FramePayload::Text(t) if t == "K1ABC: W1AW SNR?")),
            "directed TX did not decode; got {:?}",
            decodes.iter().filter_map(|f| match &f.payload { FramePayload::Text(t) => Some(t.clone()), _ => None }).collect::<Vec<_>>()
        );
    }

    /// Without a callsign, the modulator falls back to a JSC free-text data frame
    /// even when the text looks command-like.
    #[test]
    fn modulator_falls_back_to_data_without_call() {
        let sm = Js8Submode::Turbo;
        let wave = Js8Mod::new(sm).modulate(&Frame::text("HELLO WORLD")).unwrap();
        let win_len = (JS8_RATE as f32 * sm.params().tx_seconds as f32) as usize;
        let mut win = vec![0.0f32; win_len.max(wave.len())];
        win[..wave.len()].copy_from_slice(&wave);
        let decodes = Js8Demod::new(sm).decode_window(&win, 0);
        assert!(decodes.iter().any(|f| matches!(&f.payload, FramePayload::Text(t) if t == "HELLO WORLD")));
    }

    /// A directed frame (non-data) survives the full audio channel and decodes
    /// to its rendered form, exercising the payload-type routing in the demod.
    #[test]
    fn directed_frame_audio_roundtrip() {
        use crate::framing::js8_frames::{directed_cmd_code, pack_directed_frame};
        let cmd = directed_cmd_code(" SNR?").unwrap();
        let payload = pack_directed_frame("K1ABC", "W1AW", cmd, 0).unwrap();
        let bits = pack_frame(&payload, JS8_FIRST | JS8_LAST); // no DATA flag
        let sm = Js8Submode::Turbo;
        let p = sm.params();
        let syms = js8_symbols(&bits, sm);
        let gfsk = Gfsk::new(JS8_RATE as f32, p.nsps, JS8_BASE_HZ, p.tone_spacing as f32, 2.0);
        let wave = gfsk.modulate(&syms.iter().map(|&s| s as u32).collect::<Vec<_>>());
        let win_len = (JS8_RATE as f32 * p.tx_seconds as f32) as usize;
        let mut win = vec![0.0f32; win_len.max(wave.len())];
        win[..wave.len()].copy_from_slice(&wave);
        let decodes = Js8Demod::new(sm).decode_window(&win, 0);
        let texts: Vec<String> = decodes
            .iter()
            .filter_map(|f| match &f.payload {
                FramePayload::Text(t) => Some(t.clone()),
                _ => None,
            })
            .collect();
        assert!(
            texts.iter().any(|t| t == "K1ABC: W1AW SNR?"),
            "directed frame did not decode; got {texts:?}"
        );
    }

    /// Loopback survives AWGN.
    #[test]
    fn loopback_under_awgn() {
        assert!(loopback_ok(Js8Submode::Normal, "CQ K1ABC", Some(0.1)), "AWGN loopback failed");
    }

    /// Loopback survives an audio subcarrier offset.
    #[test]
    fn loopback_offset_subcarrier() {
        let msg = "TEST 73";
        let wave = Js8Mod::with_base(Js8Submode::Normal, 1200.0)
            .modulate(&Frame::text(msg))
            .unwrap();
        let win_len = (JS8_RATE as f32 * 15.0) as usize;
        let mut win = vec![0.0f32; win_len];
        win[..wave.len()].copy_from_slice(&wave);
        let decodes = Js8Demod::new(Js8Submode::Normal).decode_window(&win, 0);
        assert!(
            decodes.iter().any(|f| matches!(&f.payload, FramePayload::Text(t) if t == msg)),
            "offset loopback failed"
        );
    }

    /// Submode grid matches the reference constants (`JS8Submode.cpp`).
    #[test]
    fn submode_grid() {
        assert_eq!(Js8Submode::Normal.params().nsps, 1920);
        assert_eq!(Js8Submode::Fast.params().nsps, 1200);
        assert_eq!(Js8Submode::Turbo.params().nsps, 600);
        assert_eq!(Js8Submode::Slow.params().nsps, 3840);
        assert_eq!(Js8Submode::Ultra.params().nsps, 384);
        assert!((Js8Submode::Normal.params().tone_spacing - 6.25).abs() < 1e-9);
        assert!(!Js8Submode::Normal.params().symmetrical_costas);
        assert!(Js8Submode::Fast.params().symmetrical_costas);
    }
}
