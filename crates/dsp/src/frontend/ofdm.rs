//! 64-carrier overlapping-Walsh OFDM core — the MT63 modem's DSP heart.
//!
//! Port of fldigi's MT63 base modem (`fldigi/src/mt63/{mt63base.cxx,dsp.cxx}` +
//! `symbol.dat`/`mt63intl.dat`, upstream 4.1.23 @ 61b97f413). MT63 spreads a
//! 7-bit character across 64 carriers with an inverse **Walsh** transform, block
//! **interleaves** the resulting bits over 32 (short) or 64 (long) past symbols,
//! and transmits each carrier as **differential BPSK** in a windowed,
//! half-symbol-staggered ("overlapping") OFDM. This module holds the reusable
//! pieces: the Walsh transforms, the interleave patterns and symbol-shape window,
//! the bit-exact `Mt63Encoder`/`Mt63Decoder`, the per-carrier phase-index
//! accumulation (`tx_phase_indices`), and the audio modulator/demodulator engine
//! (`Mt63Modem`). Phase 16's OFDM data modes reuse the same core.
//!
//! ## Two equivalence classes (Doctrine §3)
//! - **Bit-exact:** `Mt63Encoder` output bits and the `TxVect` phase indices are
//!   asserted byte-for-byte against golden vectors from the *unmodified* reference
//!   (`tests/vectors/mt63.json`, `scratch/refvectors/mt63_dump.cxx`). These fully
//!   determine the DBPSK constellation on the wire.
//! - **FP / loopback:** the windowed OFDM audio is ported faithfully but gated on
//!   a loopback decode, never asserted sample-exact — matching the DominoEX/PSK
//!   precedent (fldigi's audio path is FLTK/op-order entangled).
//!
//! The audio engine synthesises MT63 as the coherent sum of the 64 windowed DBPSK
//! carriers (mathematically the reference's dual-IFFT + `SymbolShape` overlap-add,
//! expressed directly at the 8 kHz audio rate). The full ±8-carrier FEC-scan
//! synchroniser/AFC (`MT63rx::SyncProcess`) is deferred: like DominoEX/Olivia the
//! streaming demod assumes symbol alignment; the fldigi cross-decode is the
//! `#[ignore]` gate. ref: mt63base.cxx (encoder/tx/decoder), dsp.cxx (Walsh).

use crate::types::Cplx;

// ---- geometry (ref: src/mt63/symbol.dat) ----------------------------------
/// FFT + symbol-shape length (`SymbolLen`).
pub const SYMBOL_LEN: usize = 512;
/// Baseband samples between successive symbols on a carrier (`SymbolSepar`).
pub const SYMBOL_SEPAR: usize = 200;
/// FFT-bin separation between carriers (`DataCarrSepar`).
pub const DATA_CARR_SEPAR: usize = 4;
/// Number of data carriers.
pub const DATA_CARRIERS: usize = 64;
/// FFT size (`FFT.Size`), equal to `SymbolLen`.
pub const FFT_SIZE: usize = SYMBOL_LEN;
const PHASE_MASK: i32 = (FFT_SIZE - 1) as i32;
const PHASE_FLIP: i32 = (FFT_SIZE / 2) as i32;

/// Interleave depth. ref: mt63base.cxx:130-137.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interleave {
    /// `ShortIntlvPatt`, depth 32.
    Short,
    /// `LongIntlvPatt`, depth 64.
    Long,
}

impl Interleave {
    pub fn depth(self) -> usize {
        match self {
            Interleave::Short => 32,
            Interleave::Long => 64,
        }
    }
    fn pattern(self) -> &'static [i32; DATA_CARRIERS] {
        match self {
            Interleave::Short => &SHORT_INTLV_PATT,
            Interleave::Long => &LONG_INTLV_PATT,
        }
    }
}

// ---- Walsh transforms (ref: dsp.cxx:2589-2621) ----------------------------

/// In-place fast Walsh transform. `data.len()` must be a power of two.
/// ref: dsp.cxx:2589-2604 (`dspWalshTrans`).
pub fn walsh_trans(data: &mut [f64]) {
    let len = data.len();
    let mut step = 1;
    while step < len {
        let mut ptr = 0;
        while ptr < len {
            for ptr2 in ptr..ptr + step {
                let b1 = data[ptr2];
                let b2 = data[ptr2 + step];
                data[ptr2] = b1 + b2;
                data[ptr2 + step] = b2 - b1;
            }
            ptr += 2 * step;
        }
        step *= 2;
    }
}

/// In-place inverse fast Walsh transform. ref: dsp.cxx:2606-2621
/// (`dspWalshInvTrans`).
pub fn walsh_inv_trans(data: &mut [f64]) {
    let len = data.len();
    let mut step = len / 2;
    while step >= 1 {
        let mut ptr = 0;
        while ptr < len {
            for ptr2 in ptr..ptr + step {
                let b1 = data[ptr2];
                let b2 = data[ptr2 + step];
                data[ptr2] = b1 - b2;
                data[ptr2 + step] = b1 + b2;
            }
            ptr += 2 * step;
        }
        step /= 2;
    }
}

// ---- character encoder + block interleaver (ref: mt63base.cxx:369-432) -----

/// The MT63 source encoder: character → inverse-Walsh spread → block interleaver
/// → 64 output bits (`1` = no phase flip, `0` = phase flip). Bit-exact vs the
/// reference. The interleaver pipe is zero-prefilled (RandFill=0): the live
/// modem seeds it with rand() as an anti-strong-carrier startup measure, which is
/// irrelevant to the codec. ref: mt63base.cxx:369-432.
pub struct Mt63Encoder {
    intlv_len: usize,
    intlv_size: usize,
    intlv_patt: [usize; DATA_CARRIERS],
    intlv_pipe: Vec<u8>,
    intlv_ptr: usize,
    walsh: [f64; DATA_CARRIERS],
    /// Latest 64 output bits.
    pub output: [u8; DATA_CARRIERS],
}

impl Mt63Encoder {
    pub fn new(intlv: Interleave) -> Self {
        let intlv_len = intlv.depth();
        let intlv_size = intlv_len * DATA_CARRIERS;
        let pattern = intlv.pattern();
        // IntlvPatt[i] = p * DataCarriers, p += Pattern[i] (mod IntlvLen).
        let mut intlv_patt = [0usize; DATA_CARRIERS];
        let mut p = 0i32;
        for i in 0..DATA_CARRIERS {
            intlv_patt[i] = (p as usize) * DATA_CARRIERS;
            p += pattern[i];
            if p >= intlv_len as i32 {
                p -= intlv_len as i32;
            }
        }
        Mt63Encoder {
            intlv_len,
            intlv_size,
            intlv_patt,
            intlv_pipe: vec![0u8; intlv_size],
            intlv_ptr: 0,
            walsh: [0.0; DATA_CARRIERS],
            output: [0u8; DATA_CARRIERS],
        }
    }

    /// Encode one character; `self.output` holds the 64 bits. ref:
    /// mt63base.cxx:403-432.
    pub fn process(&mut self, code: u8) -> [u8; DATA_CARRIERS] {
        let code = (code & (2 * DATA_CARRIERS as u8 - 1)) as usize; // CodeMask = 127
        for w in self.walsh.iter_mut() {
            *w = 0.0;
        }
        if code < DATA_CARRIERS {
            self.walsh[code] = 1.0;
        } else {
            self.walsh[code - DATA_CARRIERS] = -1.0;
        }
        walsh_inv_trans(&mut self.walsh);

        if self.intlv_len != 0 {
            for i in 0..DATA_CARRIERS {
                self.intlv_pipe[self.intlv_ptr + i] = (self.walsh[i] < 0.0) as u8;
            }
            for i in 0..DATA_CARRIERS {
                let mut k = self.intlv_ptr + self.intlv_patt[i];
                if k >= self.intlv_size {
                    k -= self.intlv_size;
                }
                self.output[i] = self.intlv_pipe[k + i];
            }
            self.intlv_ptr += DATA_CARRIERS;
            if self.intlv_ptr >= self.intlv_size {
                self.intlv_ptr -= self.intlv_size;
            }
        } else {
            for i in 0..DATA_CARRIERS {
                self.output[i] = (self.walsh[i] < 0.0) as u8;
            }
        }
        self.output
    }
}

// ---- deinterleaver + Walsh decoder (ref: mt63base.cxx:475-579) -------------

/// The MT63 decoder: per-carrier soft differential values → deinterleave → Walsh
/// transform → decoded character. Ported at the reference's `Margin = 0`
/// (`CarrOfs = 0`) special case — a matched deinterleaver for the aligned/known-
/// carrier loopback path. The ±`ScanMargin` FEC carrier scan and the SNR
/// integration/`DecodePipe` smoothing (used only for the off-air carrier-offset
/// search) are deferred with the synchroniser. ref: mt63base.cxx:475-579.
pub struct Mt63Decoder {
    intlv_len: usize,
    intlv_size: usize,
    scan_size: usize,
    intlv_patt: [usize; DATA_CARRIERS],
    intlv_pipe: Vec<f64>,
    intlv_ptr: usize,
    walsh: [f64; DATA_CARRIERS],
    /// Latest decoded character code (0..127).
    pub output: u8,
}

impl Mt63Decoder {
    pub fn new(intlv: Interleave) -> Self {
        let intlv_len = intlv.depth();
        let scan_size = DATA_CARRIERS; // ScanSize = DataCarriers + 2*Margin, Margin=0
        let pattern = intlv.pattern();
        let mut intlv_patt = [0usize; DATA_CARRIERS];
        let mut p = 0i32;
        for i in 0..DATA_CARRIERS {
            intlv_patt[i] = (p as usize) * scan_size; // p * ScanSize
            p += pattern[i];
            if p >= intlv_len as i32 {
                p -= intlv_len as i32;
            }
        }
        let intlv_size = (intlv_len + 1) * scan_size;
        Mt63Decoder {
            intlv_len,
            intlv_size,
            scan_size,
            intlv_patt,
            intlv_pipe: vec![0.0; intlv_size],
            intlv_ptr: 0,
            walsh: [0.0; DATA_CARRIERS],
            output: 0,
        }
    }

    /// Decode one symbol's 64 soft carrier values (`+1` ≈ no flip, `-1` ≈ flip).
    /// Returns the decoded character. ref: mt63base.cxx:521-579 (Margin=0).
    pub fn process(&mut self, data: &[f64; DATA_CARRIERS]) -> u8 {
        // IntlvPipe[IntlvPtr..+ScanSize] = data
        self.intlv_pipe[self.intlv_ptr..self.intlv_ptr + self.scan_size].copy_from_slice(data);

        // single scan (s = 0)
        for i in 0..DATA_CARRIERS {
            let mut k = self.intlv_ptr as isize - self.scan_size as isize - self.intlv_patt[i] as isize;
            if k < 0 {
                k += self.intlv_size as isize;
            }
            self.walsh[i] = self.intlv_pipe[k as usize + i];
        }
        walsh_trans(&mut self.walsh);
        let (mut min, mut min_pos) = (self.walsh[0], 0usize);
        let (mut max, mut max_pos) = (self.walsh[0], 0usize);
        for (i, &v) in self.walsh.iter().enumerate() {
            if v < min {
                min = v;
                min_pos = i;
            }
            if v > max {
                max = v;
                max_pos = i;
            }
        }
        let code = if max.abs() > min.abs() {
            (max_pos + DATA_CARRIERS) as u8
        } else {
            min_pos as u8
        };

        self.intlv_ptr += self.scan_size;
        if self.intlv_ptr >= self.intlv_size {
            self.intlv_ptr = 0;
        }
        self.output = code;
        code
    }

    pub fn interleave_depth(&self) -> usize {
        self.intlv_len
    }
}

// ---- per-carrier phase-index accumulation (ref: mt63base.cxx:164-181, 262-271)

/// Per-mode geometry derived from bandwidth + centre. ref: mt63base.cxx:104-122.
#[derive(Debug, Clone, Copy)]
pub struct Mt63Geometry {
    pub bandwidth: u32,
    pub decimate: usize,
    pub first_data_carr: i32,
}

impl Mt63Geometry {
    /// ref: mt63base.cxx:104-122 — `FirstDataCarr`/`DecimateRatio` per bandwidth.
    pub fn new(bandwidth: u32, center_hz: f32) -> Self {
        let (k, decimate) = match bandwidth {
            500 => (256.0, 8usize),
            1000 => (128.0, 4usize),
            2000 => (64.0, 2usize),
            _ => panic!("MT63 bandwidth must be 500/1000/2000, got {bandwidth}"),
        };
        let first_data_carr =
            ((center_hz as f64 - bandwidth as f64 / 2.0) * k / 500.0 + 0.5).floor() as i32;
        Mt63Geometry { bandwidth, decimate, first_data_carr }
    }

    /// Baseband complex sample rate (`8000 / DecimateRatio`).
    pub fn baseband_rate(&self) -> f32 {
        8000.0 / self.decimate as f32
    }

    /// Carrier spacing in Hz (`bandwidth / 64`).
    pub fn carrier_spacing(&self) -> f32 {
        self.bandwidth as f32 / DATA_CARRIERS as f32
    }
}

/// The initial `TxVect`/`dspPhaseCorr` phase-index state. ref:
/// mt63base.cxx:164-181.
fn phase_init(geo: &Mt63Geometry) -> ([i32; DATA_CARRIERS], [i32; DATA_CARRIERS]) {
    let mut tx_vect = [0i32; DATA_CARRIERS];
    let mut phase_corr = [0i32; DATA_CARRIERS];
    // TxVect init: step/incr quadratic ramp.
    let (mut step, incr, mut p) = (0i32, 1i32, 0i32);
    for slot in tx_vect.iter_mut() {
        *slot = p;
        step += incr;
        p = (p + step) & PHASE_MASK;
    }
    // dspPhaseCorr: p starts at SymbolSepar*FirstDataCarr, increments by
    // SymbolSepar*DataCarrSepar.
    let incr = ((SYMBOL_SEPAR * DATA_CARR_SEPAR) as i32) & PHASE_MASK;
    let mut p = ((SYMBOL_SEPAR as i32) * geo.first_data_carr) & PHASE_MASK;
    for slot in phase_corr.iter_mut() {
        *slot = p;
        p = (p + incr) & PHASE_MASK;
    }
    (tx_vect, phase_corr)
}

/// The full per-symbol `TxVect` phase-index sequence (0..511) for a character
/// stream, bit-exact vs the reference `MT63tx::SendChar` accumulation. ref:
/// mt63base.cxx:262-271.
pub fn tx_phase_indices(
    geo: &Mt63Geometry,
    intlv: Interleave,
    chars: &[u8],
) -> Vec<[i32; DATA_CARRIERS]> {
    let (mut tx_vect, phase_corr) = phase_init(geo);
    let mut enc = Mt63Encoder::new(intlv);
    let mut out = Vec::with_capacity(chars.len());
    for &ch in chars {
        let bits = enc.process(ch);
        for i in 0..DATA_CARRIERS {
            if bits[i] != 0 {
                tx_vect[i] = (tx_vect[i] + phase_corr[i]) & PHASE_MASK;
            } else {
                tx_vect[i] = (tx_vect[i] + phase_corr[i] + PHASE_FLIP) & PHASE_MASK;
            }
        }
        out.push(tx_vect);
    }
    out
}

// ---- audio modulator / demodulator engine ---------------------------------

/// Resolved audio-domain parameters for one MT63 submode.
#[derive(Debug, Clone, Copy)]
pub struct Mt63Modem {
    geo: Mt63Geometry,
    intlv: Interleave,
    center_hz: f32,
    /// Audio samples per symbol (`SymbolSepar * DecimateRatio`).
    pub sym_len: usize,
    /// Window length in audio samples (`SymbolLen * DecimateRatio`).
    pub win_len: usize,
}

/// The MT63 audio sample rate is fixed at 8 kHz (ref: mt63.cxx:386).
pub const AUDIO_RATE: f32 = 8000.0;

impl Mt63Modem {
    pub fn new(bandwidth: u32, intlv: Interleave, center_hz: f32) -> Self {
        let geo = Mt63Geometry::new(bandwidth, center_hz);
        Mt63Modem {
            geo,
            intlv,
            center_hz,
            sym_len: SYMBOL_SEPAR * geo.decimate,
            win_len: SYMBOL_LEN * geo.decimate,
        }
    }

    pub fn interleave(&self) -> Interleave {
        self.intlv
    }
    pub fn geometry(&self) -> Mt63Geometry {
        self.geo
    }

    /// Lowest carrier audio frequency (`center - bandwidth/2`); carrier `i` sits
    /// at `base + i * spacing`. ref: mt63.cxx:75 (tx tone at `txfreq - bw/2`).
    fn base_hz(&self) -> f32 {
        self.center_hz - self.geo.bandwidth as f32 / 2.0
    }

    fn carrier_hz(&self, i: usize) -> f32 {
        self.base_hz() + i as f32 * self.geo.carrier_spacing()
    }

    /// Symbol-shape window resampled to the audio rate (linear interpolation of
    /// `SYMBOL_SHAPE` from `SymbolLen` to `win_len`).
    fn window(&self) -> Vec<f32> {
        let mut w = vec![0.0f32; self.win_len];
        let scale = SYMBOL_LEN as f32 / self.win_len as f32;
        for (m, wm) in w.iter_mut().enumerate() {
            let x = m as f32 * scale;
            let i0 = x.floor() as usize;
            let frac = x - i0 as f32;
            let a = SYMBOL_SHAPE[i0.min(SYMBOL_LEN - 1)];
            let b = SYMBOL_SHAPE[(i0 + 1).min(SYMBOL_LEN - 1)];
            *wm = a + (b - a) * frac;
        }
        w
    }

    /// Half-symbol stagger (audio samples) for odd carriers. ref: even/odd IFFT
    /// slices offset by `SymbolSepar/2` (mt63base.cxx:311, DataProcess).
    fn stagger(&self, i: usize) -> usize {
        if i & 1 == 1 {
            self.sym_len / 2
        } else {
            0
        }
    }

    /// Synthesise the OFDM audio for a character stream. The caller is
    /// responsible for framing (flush NULs). Output is peak-normalised. Uses a
    /// per-carrier complex-phasor recurrence (no per-sample `cos`).
    pub fn modulate_chars(&self, chars: &[u8]) -> Vec<f32> {
        let win = self.window();
        let n_sym = chars.len();
        let total = n_sym * self.sym_len + self.win_len + self.sym_len;
        let mut audio = vec![0.0f32; total];
        let mut enc = Mt63Encoder::new(self.intlv);
        let mut flip = [0u8; DATA_CARRIERS]; // running DBPSK phase state (0 or 1)
        for (k, &ch) in chars.iter().enumerate() {
            let bits = enc.process(ch);
            for i in 0..DATA_CARRIERS {
                if bits[i] == 0 {
                    flip[i] ^= 1; // phase flip
                }
                let wk = std::f64::consts::TAU * self.carrier_hz(i) as f64 / AUDIO_RATE as f64;
                let n0 = k * self.sym_len + self.stagger(i);
                let sign = if flip[i] == 1 { -1.0f32 } else { 1.0f32 };
                // phasor p = exp(j·wk·n), stepped by exp(j·wk); Re(p) = cos(wk·n)
                let (mut pr, mut pi) = ((wk * n0 as f64).cos(), (wk * n0 as f64).sin());
                let (ec, es) = (wk.cos(), wk.sin());
                for m in 0..self.win_len {
                    audio[n0 + m] += sign * win[m] * pr as f32;
                    let npr = pr * ec - pi * es;
                    pi = pr * es + pi * ec;
                    pr = npr;
                    if m & 255 == 255 {
                        let mg = (pr * pr + pi * pi).sqrt();
                        pr /= mg;
                        pi /= mg;
                    }
                }
            }
        }
        // peak-normalise (fldigi normalises each block by its max; ref: mt63.cxx)
        let peak = audio.iter().fold(0.0f32, |a, &v| a.max(v.abs()));
        if peak > 0.0 {
            let g = 0.9 / peak;
            for v in audio.iter_mut() {
                *v *= g;
            }
        }
        audio
    }

    /// Correlate carrier `i`'s symbol window against its complex exponential —
    /// the reference's windowed FFT-bin extraction (mt63base.cxx:1324-1339). The
    /// symbol's even-carrier window starts at absolute sample `k_start`; odd
    /// carriers are staggered by half a symbol. `audio[0]` is absolute index
    /// `audio_base`. Complex-phasor recurrence, no per-sample `cos`.
    fn carrier_corr(
        &self,
        audio: &[f32],
        audio_base: usize,
        win: &[f32],
        k_start: usize,
        i: usize,
    ) -> Cplx {
        let wk = std::f64::consts::TAU * self.carrier_hz(i) as f64 / AUDIO_RATE as f64;
        let abs_n0 = k_start + self.stagger(i);
        let local = abs_n0 - audio_base;
        // conj phasor: exp(-j·wk·n) = (cos(wk·n), -sin(wk·n))
        let (mut pr, mut pi) = ((wk * abs_n0 as f64).cos(), -(wk * abs_n0 as f64).sin());
        let (ec, es) = (wk.cos(), -wk.sin());
        let (mut ar, mut ai) = (0.0f64, 0.0f64);
        for (m, &wm) in win.iter().enumerate() {
            let idx = local + m;
            if idx >= audio.len() {
                break;
            }
            let s = (wm * audio[idx]) as f64;
            ar += s * pr;
            ai += s * pi;
            let npr = pr * ec - pi * es;
            pi = pr * es + pi * ec;
            pr = npr;
            if m & 255 == 255 {
                let mg = (pr * pr + pi * pi).sqrt();
                pr /= mg;
                pi /= mg;
            }
        }
        Cplx::new(ar as f32, ai as f32)
    }

    /// Differential-BPSK soft values for symbol `k`'s carriers vs the previous
    /// symbol: `+1` ≈ no flip (bit 1), `-1` ≈ flip (bit 0). ref: DataProcess
    /// (mt63base.cxx:1408-1418, `Re(DataVect)/power`).
    fn soft_from(cur: &[Cplx; DATA_CARRIERS], prev: &[Cplx; DATA_CARRIERS]) -> [f64; DATA_CARRIERS] {
        let mut soft = [0.0f64; DATA_CARRIERS];
        for i in 0..DATA_CARRIERS {
            let d = cur[i] * prev[i].conj();
            let mag = (cur[i].norm() * prev[i].norm()).max(1e-12);
            soft[i] = (d.re / mag) as f64;
        }
        soft
    }

    /// Demodulate a fully-buffered, symbol-aligned audio block into character
    /// codes (0..127), one per symbol after the first. For tests / one-shot
    /// decode; the streaming path is [`Mt63Rx`].
    pub fn demodulate_chars(&self, audio: &[f32]) -> Vec<u8> {
        let win = self.window();
        let mut dec = Mt63Decoder::new(self.intlv);
        let n_sym = if audio.len() > self.win_len {
            (audio.len() - self.win_len) / self.sym_len + 1
        } else {
            0
        };
        let mut prev: Option<[Cplx; DATA_CARRIERS]> = None;
        let mut out = Vec::new();
        for k in 0..n_sym {
            let mut cur = [Cplx::new(0.0, 0.0); DATA_CARRIERS];
            for (i, slot) in cur.iter_mut().enumerate() {
                *slot = self.carrier_corr(audio, 0, &win, k * self.sym_len, i);
            }
            if let Some(p) = &prev {
                out.push(dec.process(&Self::soft_from(&cur, p)));
            }
            prev = Some(cur);
        }
        out
    }
}

/// Streaming MT63 receiver: feed audio as it arrives, get decoded character
/// codes. Maintains the differential reference and deinterleave/Walsh decoder
/// across calls, draining consumed samples. Assumes symbol alignment to the
/// first fed sample (the ±carrier FEC-scan synchroniser is deferred; see module
/// docs). ref: MT63rx::Process / DataProcess (mt63base.cxx:944-1447).
pub struct Mt63Rx {
    modem: Mt63Modem,
    win: Vec<f32>,
    buf: Vec<f32>,
    /// absolute sample index of `buf[0]`
    abs0: usize,
    next_sym: usize,
    dec: Mt63Decoder,
    prev: Option<[Cplx; DATA_CARRIERS]>,
}

impl Mt63Rx {
    pub fn new(modem: Mt63Modem) -> Self {
        let win = modem.window();
        let dec = Mt63Decoder::new(modem.interleave());
        Mt63Rx { modem, win, buf: Vec::new(), abs0: 0, next_sym: 0, dec, prev: None }
    }

    /// Feed audio samples; returns any newly decoded character codes.
    pub fn feed(&mut self, samples: &[f32]) -> Vec<u8> {
        self.buf.extend_from_slice(samples);
        let (sym_len, win_len) = (self.modem.sym_len, self.modem.win_len);
        let mut out = Vec::new();
        loop {
            let k_start = self.next_sym * sym_len;
            // the odd-carrier window reaches furthest: k_start + sym_len/2 + win_len
            let need = k_start + sym_len / 2 + win_len;
            if self.abs0 + self.buf.len() < need {
                break;
            }
            let mut cur = [Cplx::new(0.0, 0.0); DATA_CARRIERS];
            for (i, slot) in cur.iter_mut().enumerate() {
                *slot = self.modem.carrier_corr(&self.buf, self.abs0, &self.win, k_start, i);
            }
            if let Some(p) = &self.prev {
                out.push(self.dec.process(&Mt63Modem::soft_from(&cur, p)));
            }
            self.prev = Some(cur);
            self.next_sym += 1;
            // drain everything before the next symbol's even-carrier window
            let keep_from = self.next_sym * sym_len;
            if keep_from > self.abs0 {
                let drop = (keep_from - self.abs0).min(self.buf.len());
                self.buf.drain(..drop);
                self.abs0 += drop;
            }
        }
        out
    }

    pub fn reset(&mut self) {
        self.buf.clear();
        self.abs0 = 0;
        self.next_sym = 0;
        self.prev = None;
        self.dec = Mt63Decoder::new(self.modem.interleave());
    }
}

// ---- interleave patterns (ref: src/mt63/mt63intl.dat) ----------------------

/// Short interleave pattern (`ShortIntlvPatt`, depth 32). ref: mt63intl.dat:24-40.
#[rustfmt::skip]
pub static SHORT_INTLV_PATT: [i32; DATA_CARRIERS] = [
    4, 5, 6, 7, 4, 5, 6, 7, 4, 5, 6, 7, 4, 5, 6, 7,
    4, 5, 6, 7, 4, 5, 6, 7, 4, 5, 6, 7, 4, 5, 6, 7,
    4, 5, 6, 7, 4, 5, 6, 7, 4, 5, 6, 7, 4, 5, 6, 7,
    4, 5, 6, 7, 4, 5, 6, 7, 4, 5, 6, 7, 4, 5, 6, 7,
];

/// Long interleave pattern (`LongIntlvPatt`, depth 64). ref: mt63intl.dat:43-48.
#[rustfmt::skip]
pub static LONG_INTLV_PATT: [i32; DATA_CARRIERS] = [
    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16,
    17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32,
    33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48,
    49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63, 0,
];


// ---- symbol-shape window (ref: src/mt63/symbol.dat) -----------------------

/// SymbolShape[512] — MT63 symbol/window shape, transcribed verbatim from
/// fldigi/src/mt63/symbol.dat (fldigi 4.1.23 @61b97f413): "taken directly from
/// the MT63ASC code for the EVM56K" — a precomputed shape, not a closed form.
///
/// The literals are the reference's `double` values kept verbatim for provenance;
/// the compiler rounds each to the nearest `f32` (the extra digits change no bit
/// of the stored value), so the `excessive_precision` lint is silenced rather
/// than lossily truncating the transcription.
#[rustfmt::skip]
#[allow(clippy::excessive_precision)]
pub static SYMBOL_SHAPE: [f32; SYMBOL_LEN] = [
    -0.00000000f32, 0.00000665f32, 0.00002657f32, 0.00005975f32, 0.00010613f32, 0.00016562f32,
    0.00023810f32, 0.00032341f32, 0.00042134f32, 0.00053162f32, 0.00065389f32, 0.00078773f32,
    0.00093261f32, 0.00108789f32, 0.00125283f32, 0.00142653f32, 0.00160798f32, 0.00179599f32,
    0.00198926f32, 0.00218628f32, 0.00238542f32, 0.00258487f32, 0.00278264f32, 0.00297662f32,
    0.00316452f32, 0.00334394f32, 0.00351232f32, 0.00366701f32, 0.00380526f32, 0.00392424f32,
    0.00402109f32, 0.00409288f32, 0.00413671f32, 0.00414969f32, 0.00412898f32, 0.00407182f32,
    0.00397555f32, 0.00383764f32, 0.00365574f32, 0.00342767f32, 0.00315145f32, 0.00282534f32,
    0.00244787f32, 0.00201781f32, 0.00153424f32, 0.00099653f32, 0.00040435f32, -0.00024231f32,
    -0.00094314f32, -0.00169753f32, -0.00250453f32, -0.00336293f32, -0.00427118f32,
    -0.00522749f32, -0.00622977f32, -0.00727569f32, -0.00836272f32, -0.00948809f32,
    -0.01064886f32, -0.01184193f32, -0.01306405f32, -0.01431189f32, -0.01558198f32,
    -0.01687083f32, -0.01817486f32, -0.01949051f32, -0.02081416f32, -0.02214223f32,
    -0.02347113f32, -0.02479733f32, -0.02611728f32, -0.02742752f32, -0.02872457f32,
    -0.03000504f32, -0.03126551f32, -0.03250262f32, -0.03371298f32, -0.03489320f32,
    -0.03603988f32, -0.03714954f32, -0.03821868f32, -0.03924367f32, -0.04022079f32,
    -0.04114620f32, -0.04201589f32, -0.04282570f32, -0.04357126f32, -0.04424801f32,
    -0.04485118f32, -0.04537575f32, -0.04581648f32, -0.04616787f32, -0.04642421f32,
    -0.04657955f32, -0.04662769f32, -0.04656225f32, -0.04637665f32, -0.04606414f32,
    -0.04561786f32, -0.04503082f32, -0.04429599f32, -0.04340631f32, -0.04235475f32,
    -0.04113436f32, -0.03973834f32, -0.03816006f32, -0.03639316f32, -0.03443155f32,
    -0.03226956f32, -0.02990192f32, -0.02732385f32, -0.02453112f32, -0.02152012f32,
    -0.01828789f32, -0.01483216f32, -0.01115146f32, -0.00724508f32, -0.00311317f32,
    0.00124328f32, 0.00582236f32, 0.01062127f32, 0.01563627f32, 0.02086273f32, 0.02629504f32,
    0.03192674f32, 0.03775043f32, 0.04375787f32, 0.04993995f32, 0.05628681f32, 0.06278780f32,
    0.06943159f32, 0.07620621f32, 0.08309914f32, 0.09009732f32, 0.09718730f32, 0.10435526f32,
    0.11158715f32, 0.11886870f32, 0.12618560f32, 0.13352351f32, 0.14086819f32, 0.14820561f32,
    0.15552198f32, 0.16280389f32, 0.17003841f32, 0.17721311f32, 0.18431620f32, 0.19133661f32,
    0.19826401f32, 0.20508896f32, 0.21180289f32, 0.21839823f32, 0.22486845f32, 0.23120806f32,
    0.23741270f32, 0.24347919f32, 0.24940549f32, 0.25519079f32, 0.26083547f32, 0.26634116f32,
    0.27171067f32, 0.27694807f32, 0.28205857f32, 0.28704860f32, 0.29192571f32, 0.29669855f32,
    0.30137684f32, 0.30597130f32, 0.31049362f32, 0.31495636f32, 0.31937292f32, 0.32375741f32,
    0.32812465f32, 0.33249001f32, 0.33686936f32, 0.34127898f32, 0.34573545f32, 0.35025554f32,
    0.35485613f32, 0.35955412f32, 0.36436627f32, 0.36930915f32, 0.37439902f32, 0.37965170f32,
    0.38508250f32, 0.39070609f32, 0.39653642f32, 0.40258662f32, 0.40886890f32, 0.41539446f32,
    0.42217341f32, 0.42921470f32, 0.43652603f32, 0.44411383f32, 0.45198311f32, 0.46013753f32,
    0.46857925f32, 0.47730896f32, 0.48632585f32, 0.49562756f32, 0.50521021f32, 0.51506840f32,
    0.52519520f32, 0.53558220f32, 0.54621950f32, 0.55709582f32, 0.56819849f32, 0.57951351f32,
    0.59102568f32, 0.60271860f32, 0.61457478f32, 0.62657574f32, 0.63870210f32, 0.65093366f32,
    0.66324951f32, 0.67562817f32, 0.68804763f32, 0.70048553f32, 0.71291922f32, 0.72532590f32,
    0.73768272f32, 0.74996688f32, 0.76215572f32, 0.77422687f32, 0.78615828f32, 0.79792836f32,
    0.80951602f32, 0.82090079f32, 0.83206287f32, 0.84298315f32, 0.85364335f32, 0.86402598f32,
    0.87411443f32, 0.88389296f32, 0.89334677f32, 0.90246195f32, 0.91122553f32, 0.91962547f32,
    0.92765062f32, 0.93529073f32, 0.94253642f32, 0.94937916f32, 0.95581122f32, 0.96182562f32,
    0.96741616f32, 0.97257728f32, 0.97730410f32, 0.98159233f32, 0.98543825f32, 0.98883864f32,
    0.99179079f32, 0.99429241f32, 0.99634163f32, 0.99793696f32, 0.99907728f32, 0.99976178f32,
    0.99999000f32, 0.99976178f32, 0.99907728f32, 0.99793696f32, 0.99634163f32, 0.99429241f32,
    0.99179079f32, 0.98883864f32, 0.98543825f32, 0.98159233f32, 0.97730410f32, 0.97257728f32,
    0.96741616f32, 0.96182562f32, 0.95581122f32, 0.94937916f32, 0.94253642f32, 0.93529073f32,
    0.92765062f32, 0.91962547f32, 0.91122553f32, 0.90246195f32, 0.89334677f32, 0.88389296f32,
    0.87411443f32, 0.86402598f32, 0.85364335f32, 0.84298315f32, 0.83206287f32, 0.82090079f32,
    0.80951602f32, 0.79792836f32, 0.78615828f32, 0.77422687f32, 0.76215572f32, 0.74996688f32,
    0.73768272f32, 0.72532590f32, 0.71291922f32, 0.70048553f32, 0.68804763f32, 0.67562817f32,
    0.66324951f32, 0.65093366f32, 0.63870210f32, 0.62657574f32, 0.61457478f32, 0.60271860f32,
    0.59102568f32, 0.57951351f32, 0.56819849f32, 0.55709582f32, 0.54621950f32, 0.53558220f32,
    0.52519520f32, 0.51506840f32, 0.50521021f32, 0.49562756f32, 0.48632585f32, 0.47730896f32,
    0.46857925f32, 0.46013753f32, 0.45198311f32, 0.44411383f32, 0.43652603f32, 0.42921470f32,
    0.42217341f32, 0.41539446f32, 0.40886890f32, 0.40258662f32, 0.39653642f32, 0.39070609f32,
    0.38508250f32, 0.37965170f32, 0.37439902f32, 0.36930915f32, 0.36436627f32, 0.35955412f32,
    0.35485613f32, 0.35025554f32, 0.34573545f32, 0.34127898f32, 0.33686936f32, 0.33249001f32,
    0.32812465f32, 0.32375741f32, 0.31937292f32, 0.31495636f32, 0.31049362f32, 0.30597130f32,
    0.30137684f32, 0.29669855f32, 0.29192571f32, 0.28704860f32, 0.28205857f32, 0.27694807f32,
    0.27171067f32, 0.26634116f32, 0.26083547f32, 0.25519079f32, 0.24940549f32, 0.24347919f32,
    0.23741270f32, 0.23120806f32, 0.22486845f32, 0.21839823f32, 0.21180289f32, 0.20508896f32,
    0.19826401f32, 0.19133661f32, 0.18431620f32, 0.17721311f32, 0.17003841f32, 0.16280389f32,
    0.15552198f32, 0.14820561f32, 0.14086819f32, 0.13352351f32, 0.12618560f32, 0.11886870f32,
    0.11158715f32, 0.10435526f32, 0.09718730f32, 0.09009732f32, 0.08309914f32, 0.07620621f32,
    0.06943159f32, 0.06278780f32, 0.05628681f32, 0.04993995f32, 0.04375787f32, 0.03775043f32,
    0.03192674f32, 0.02629504f32, 0.02086273f32, 0.01563627f32, 0.01062127f32, 0.00582236f32,
    0.00124328f32, -0.00311317f32, -0.00724508f32, -0.01115146f32, -0.01483216f32,
    -0.01828789f32, -0.02152012f32, -0.02453112f32, -0.02732385f32, -0.02990192f32,
    -0.03226956f32, -0.03443155f32, -0.03639316f32, -0.03816006f32, -0.03973834f32,
    -0.04113436f32, -0.04235475f32, -0.04340631f32, -0.04429599f32, -0.04503082f32,
    -0.04561786f32, -0.04606414f32, -0.04637665f32, -0.04656225f32, -0.04662769f32,
    -0.04657955f32, -0.04642421f32, -0.04616787f32, -0.04581648f32, -0.04537575f32,
    -0.04485118f32, -0.04424801f32, -0.04357126f32, -0.04282570f32, -0.04201589f32,
    -0.04114620f32, -0.04022079f32, -0.03924367f32, -0.03821868f32, -0.03714954f32,
    -0.03603988f32, -0.03489320f32, -0.03371298f32, -0.03250262f32, -0.03126551f32,
    -0.03000504f32, -0.02872457f32, -0.02742752f32, -0.02611728f32, -0.02479733f32,
    -0.02347113f32, -0.02214223f32, -0.02081416f32, -0.01949051f32, -0.01817486f32,
    -0.01687083f32, -0.01558198f32, -0.01431189f32, -0.01306405f32, -0.01184193f32,
    -0.01064886f32, -0.00948809f32, -0.00836272f32, -0.00727569f32, -0.00622977f32,
    -0.00522749f32, -0.00427118f32, -0.00336293f32, -0.00250453f32, -0.00169753f32,
    -0.00094314f32, -0.00024231f32, 0.00040435f32, 0.00099653f32, 0.00153424f32, 0.00201781f32,
    0.00244787f32, 0.00282534f32, 0.00315145f32, 0.00342767f32, 0.00365574f32, 0.00383764f32,
    0.00397555f32, 0.00407182f32, 0.00412898f32, 0.00414969f32, 0.00413671f32, 0.00409288f32,
    0.00402109f32, 0.00392424f32, 0.00380526f32, 0.00366701f32, 0.00351232f32, 0.00334394f32,
    0.00316452f32, 0.00297662f32, 0.00278264f32, 0.00258487f32, 0.00238542f32, 0.00218628f32,
    0.00198926f32, 0.00179599f32, 0.00160798f32, 0.00142653f32, 0.00125283f32, 0.00108789f32,
    0.00093261f32, 0.00078773f32, 0.00065389f32, 0.00053162f32, 0.00042134f32, 0.00032341f32,
    0.00023810f32, 0.00016562f32, 0.00010613f32, 0.00005975f32, 0.00002657f32, 0.00000665f32,
];

#[cfg(test)]
mod tests {
    use super::*;

    // The message the reference dump encodes (ref: mt63_dump.cxx MSG).
    const MSG: &str = "CQ CQ DE K1ABC K1ABC/7 --.,?!";
    const VECTORS: &str = include_str!("../../tests/vectors/mt63.json");

    /// Locate a config block's `encoder`/`txvect`/`first_data_carr` lines by a
    /// minimal line scan (no serde in the dsp test build, matching kat.rs).
    fn config_lines(cfg: &str) -> (i32, &'static str, &'static str) {
        let mut lines = VECTORS.lines();
        let needle = format!("\"{cfg}\": {{");
        for l in lines.by_ref() {
            if l.contains(&needle) {
                break;
            }
        }
        let (mut fdc, mut enc, mut tx) = (None, None, None);
        for l in lines.by_ref() {
            if l.contains("first_data_carr") {
                let v = l.split(':').nth(1).unwrap().trim().trim_end_matches(',');
                fdc = Some(v.parse::<i32>().unwrap());
            } else if l.contains("\"encoder\"") {
                enc = Some(l);
            } else if l.contains("\"txvect\"") {
                tx = Some(l);
                break;
            }
        }
        (fdc.unwrap(), enc.unwrap(), tx.unwrap())
    }

    fn parse_encoder(line: &str) -> Vec<[u8; DATA_CARRIERS]> {
        let inner = &line[line.find('[').unwrap() + 1..line.rfind(']').unwrap()];
        inner
            .split(',')
            .map(|tok| {
                let s = tok.trim().trim_matches('"');
                let mut out = [0u8; DATA_CARRIERS];
                for (i, c) in s.bytes().enumerate() {
                    out[i] = c - b'0';
                }
                out
            })
            .collect()
    }

    fn parse_txvect(line: &str) -> Vec<[i32; DATA_CARRIERS]> {
        let inner = &line[line.find('[').unwrap() + 1..line.rfind(']').unwrap()];
        // inner is "[a,b,...],[a,b,...],..."; split on "],["
        inner
            .trim_start_matches('[')
            .trim_end_matches(']')
            .split("],[")
            .map(|row| {
                let mut out = [0i32; DATA_CARRIERS];
                for (i, tok) in row.split(',').enumerate() {
                    out[i] = tok.trim().parse().unwrap();
                }
                out
            })
            .collect()
    }

    fn intlv_for(cfg: &str) -> Interleave {
        if cfg.ends_with('l') {
            Interleave::Long
        } else {
            Interleave::Short
        }
    }
    fn bandwidth_for(cfg: &str) -> u32 {
        cfg.trim_start_matches("mt63_").trim_end_matches(['s', 'l']).parse().unwrap()
    }

    /// Bit-exact: the ported `Mt63Encoder` output reproduces fldigi's
    /// `MT63encoder.Output` byte-for-byte across every config in the golden
    /// vector. Runs in the plain lib build (CI does not enable `testutil`).
    #[test]
    fn encoder_matches_fldigi_vector() {
        for cfg in ["mt63_500s", "mt63_1000s", "mt63_1000l", "mt63_2000s"] {
            let (_, enc_line, _) = config_lines(cfg);
            let want = parse_encoder(enc_line);
            let mut enc = Mt63Encoder::new(intlv_for(cfg));
            for (k, ch) in MSG.bytes().enumerate() {
                let got = enc.process(ch);
                assert_eq!(got, want[k], "{cfg}: encoder bits differ at char {k}");
            }
        }
    }

    /// Bit-exact: the ported `TxVect` phase-index accumulation reproduces
    /// fldigi's `MT63tx::SendChar` sequence byte-for-byte (0..511 per carrier).
    #[test]
    fn txvect_matches_fldigi_vector() {
        for cfg in ["mt63_500s", "mt63_1000s", "mt63_1000l", "mt63_2000s"] {
            let (fdc, _, tx_line) = config_lines(cfg);
            let want = parse_txvect(tx_line);
            let geo = Mt63Geometry::new(bandwidth_for(cfg), 1500.0);
            assert_eq!(geo.first_data_carr, fdc, "{cfg}: FirstDataCarr differs");
            let chars: Vec<u8> = MSG.bytes().collect();
            let got = tx_phase_indices(&geo, intlv_for(cfg), &chars);
            assert_eq!(got.len(), want.len());
            for (k, (g, w)) in got.iter().zip(&want).enumerate() {
                assert_eq!(g, w, "{cfg}: TxVect differs at symbol {k}");
            }
        }
    }

    /// The inverse Walsh transform is inverted by the forward transform up to the
    /// scale the decoder relies on (single-spike round-trip).
    #[test]
    fn walsh_round_trips_a_spike() {
        for pos in [0usize, 1, 17, 63] {
            let mut v = vec![0.0f64; DATA_CARRIERS];
            v[pos] = 1.0;
            walsh_inv_trans(&mut v);
            // every entry is ±(1) (a Walsh function)
            assert!(v.iter().all(|&x| (x.abs() - 1.0).abs() < 1e-9));
            walsh_trans(&mut v);
            // forward∘inverse concentrates back onto `pos`
            let peak = v.iter().enumerate().max_by(|a, b| a.1.abs().total_cmp(&b.1.abs())).unwrap().0;
            assert_eq!(peak, pos);
        }
    }

    // ---- audio loopback -----------------------------------------------------

    /// Frame a message the way fldigi's tx_process does: `IntlvLen` leading NUL
    /// flush chars, the text, then a generous trailing NUL flush.
    fn framed(text: &str, intlv: Interleave) -> Vec<u8> {
        let d = intlv.depth();
        let mut v = vec![0u8; d];
        v.extend(text.bytes());
        v.extend(std::iter::repeat_n(0u8, 2 * d + 8));
        v
    }

    fn loopback(bw: u32, intlv: Interleave, text: &str) -> String {
        let modem = Mt63Modem::new(bw, intlv, 1500.0);
        let audio = modem.modulate_chars(&framed(text, intlv));
        let codes = modem.demodulate_chars(&audio);
        // recover the printable run and search for the message
        let s: String = codes.iter().map(|&c| c as char).collect();
        s
    }

    #[test]
    fn loopback_recovers_message_all_submodes() {
        let text = "CQ DE K1ABC/7 2026";
        for bw in [500u32, 1000, 2000] {
            for intlv in [Interleave::Short, Interleave::Long] {
                let out = loopback(bw, intlv, text);
                assert!(
                    out.contains(text),
                    "MT63-{bw}{:?} did not recover message; got {out:?}",
                    intlv
                );
            }
        }
    }
}
