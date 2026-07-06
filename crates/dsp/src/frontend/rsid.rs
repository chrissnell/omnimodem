//! RSID — Reed-Solomon Identifier (fldigi parity), both directions.
//!
//! RSID is a short 15-tone MFSK burst, emitted ahead of a transmission, that
//! identifies the mode (and, on receive, the audio offset). It is cross-cutting
//! rather than an on-air text mode: it touches every mode's ID table and the
//! daemon mode-switch path. This module ports fldigi's `cRsId` both ways —
//! `encode` + the TX burst synthesizer, and `RsidDetector` for receive.
//!
//! Provenance:
//!   upstream: w1hkj/fldigi (checked out at ../fldigi)
//!   ref: src/rsid/rsid.cxx      — Encode (RS(15,3)/GF(16)), receive/search/
//!        CalculateBuckets/HammingDistance/search_amp/apply, send/send_eot.
//!   ref: src/rsid/rsid_defs.cxx — RSID_LIST / RSID_LIST2 ID tables.
//!   ref: src/include/rsid.h     — constants.
//!
//! Two equivalence classes (plan Doctrine §3):
//!   * bit-exact — the 15 RS symbols (KAT vs `tests/vectors/rsid.json`) and the
//!     TX tone-index sequence;
//!   * FP tolerance — the modulated burst audio and the reported detect
//!     frequency (never asserted bit-exact).

use crate::frontend::resample::Resampler;
use crate::types::Sample;
use rustfft::{num_complex::Complex, Fft, FftPlanner};
use std::sync::Arc;

// ref: rsid.h — constants.
/// RSID internal sample rate; all detection math is at this rate.
pub const RSID_SAMPLE_RATE: f32 = 11025.0;
/// Number of RS symbols (tones) per burst.
pub const NSYMBOLS: usize = 15;
/// FFT size used for detection (`RSID_ARRAY_SIZE`).
const FFT_SIZE: usize = 2048;
/// Usable positive-frequency half (`RSID_FFT_SIZE`).
const NBINS: usize = 1024;
/// Number of time slices retained (`RSID_NTIMES`).
const NTIMES: usize = 30;
/// FFT hop, in samples (`RSID_FFT_SAMPLES`).
const HOP: usize = 512;
/// Symbol length, in RSID-rate samples (`1024/11025 s`).
const SYMLEN_SAMPLES: usize = 1024;
/// Reserved code: end-of-transmission marker.
pub const RSID_EOT: u16 = 263;
/// Reserved code: escape to the secondary (extended) table.
pub const RSID_ESCAPE: u16 = 6;
/// Detected-frequency precision, in Hz (`RSID_PRECISION`).
pub const RSID_PRECISION: f32 = 2.7;

// ref: rsid.cxx:58-75 — GF(16) multiply table: Squares[(a<<4)+b] == a*b in GF(16).
#[rustfmt::skip]
static SQUARES: [u8; 256] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9,10,11,12,13,14,15,
    0, 2, 4, 6, 8,10,12,14, 9,11,13,15, 1, 3, 5, 7,
    0, 3, 6, 5,12,15,10, 9, 1, 2, 7, 4,13,14,11, 8,
    0, 4, 8,12, 9,13, 1, 5,11,15, 3, 7, 2, 6,10,14,
    0, 5,10,15,13, 8, 7, 2, 3, 6, 9,12,14,11, 4, 1,
    0, 6,12,10, 1, 7,13,11, 2, 4,14, 8, 3, 5,15, 9,
    0, 7,14, 9, 5, 2,11,12,10,13, 4, 3,15, 8, 1, 6,
    0, 8, 9, 1,11, 3, 2,10,15, 7, 6,14, 4,12,13, 5,
    0, 9,11, 2,15, 6, 4,13, 7,14,12, 5, 8, 1, 3,10,
    0,10,13, 7, 3, 9,14, 4, 6,12,11, 1, 5,15, 8, 2,
    0,11,15, 4, 7,12, 8, 3,14, 5, 1,10, 9, 2, 6,13,
    0,12, 1,13, 2,14, 3,15, 4, 8, 5, 9, 6,10, 7,11,
    0,13, 3,14, 6,11, 5, 8,12, 1,15, 2,10, 7, 9, 4,
    0,14, 5,11,10, 4,15, 1,13, 3, 8, 6, 7, 9, 2,12,
    0,15, 7, 8,14, 1, 9, 6, 5,10, 2,13,11, 4,12, 3,
];

// ref: rsid.cxx:77-79 — the 12 generator roots.
static INDICES: [u8; 12] = [2, 4, 8, 9, 11, 15, 7, 14, 5, 10, 13, 3];

/// Encode an RSID code into its 15 Reed-Solomon symbols (tone indices 0..15).
///
/// ref: rsid.cxx:184-196 (verbatim, rebased to a returned array). Bit-exact.
pub fn encode(code: u16) -> [u8; NSYMBOLS] {
    let mut rsid = [0u8; NSYMBOLS];
    rsid[0] = (code >> 8) as u8;
    rsid[1] = ((code >> 4) & 0x0f) as u8;
    rsid[2] = (code & 0x0f) as u8;
    for &root in INDICES.iter() {
        let idx = root as usize;
        for j in (1..NSYMBOLS).rev() {
            rsid[j] = rsid[j - 1] ^ SQUARES[((rsid[j] as usize) << 4) + idx];
        }
        rsid[0] = SQUARES[((rsid[0] as usize) << 4) + idx];
    }
    rsid
}

// ---------------------------------------------------------------------------
// ID tables — ref: rsid_defs.cxx. `mode` is the omnimodem `ModeConfig` string
// this code maps to, or `None` when fldigi assigns the code but omnimodem has
// not (yet) ported the mode. The `tag` is always known and is what a detection
// reports to the user, matching fldigi's "RSID: <tag>" behaviour.
// ---------------------------------------------------------------------------

/// One entry of an RSID ID table.
#[derive(Debug, Clone, Copy)]
pub struct RsidEntry {
    pub code: u16,
    pub tag: &'static str,
    /// The omnimodem `ModeConfig` string, or `None` if the mode is unported.
    pub mode: Option<&'static str>,
}

macro_rules! e {
    ($code:expr, $tag:expr, $mode:expr) => {
        RsidEntry { code: $code, tag: $tag, mode: $mode }
    };
}

/// Primary ID table (`RSID_LIST`). ref: rsid_defs.cxx:29-217.
pub static TABLE1: &[RsidEntry] = &[
    e!(263, "EOT", None),
    e!(6, "ESCAPE", None),
    e!(1, "BPSK31", Some("psk31")),
    e!(110, "QPSK31", Some("qpsk31")),
    e!(2, "BPSK63", Some("psk63")),
    e!(3, "QPSK63", Some("qpsk63")),
    e!(4, "BPSK125", Some("psk125")),
    e!(5, "QPSK125", Some("qpsk125")),
    e!(126, "BPSK250", Some("psk250")),
    e!(127, "QPSK250", Some("qpsk250")),
    e!(173, "BPSK500", Some("psk500")),
    e!(183, "PSK125R", Some("psk125r")),
    e!(186, "PSK250R", Some("psk250r")),
    e!(187, "PSK500R", Some("psk500r")),
    e!(7, "PSKFEC31", None),
    e!(8, "PSK10", None),
    e!(9, "MT63_500_LG", Some("mt63_500l")),
    e!(10, "MT63_500_ST", Some("mt63_500s")),
    e!(11, "MT63_500_VST", None),
    e!(12, "MT63_1000_LG", Some("mt63_1000l")),
    e!(13, "MT63_1000_ST", Some("mt63_1000s")),
    e!(14, "MT63_1000_VST", None),
    e!(15, "MT63_2000_LG", Some("mt63_2000l")),
    e!(17, "MT63_2000_ST", Some("mt63_2000s")),
    e!(18, "MT63_2000_VST", None),
    e!(19, "PSKAM10", None),
    e!(20, "PSKAM31", None),
    e!(21, "PSKAM50", None),
    e!(22, "PSK63F", Some("psk63f")),
    e!(23, "PSK220F", None),
    e!(24, "CHIP64", None),
    e!(25, "CHIP128", None),
    e!(26, "CW", Some("cw")),
    e!(27, "CCW_OOK_12", None),
    e!(28, "CCW_OOK_24", None),
    e!(29, "CCW_OOK_48", None),
    e!(30, "CCW_FSK_12", None),
    e!(31, "CCW_FSK_24", None),
    e!(33, "CCW_FSK_48", None),
    e!(34, "PACTOR1_FEC", None),
    e!(113, "PACKET_110", None),
    e!(35, "PACKET_300", None),
    e!(36, "PACKET_1200", None),
    e!(37, "RTTY_ASCII_7", None),
    e!(38, "RTTY_ASCII_8", None),
    e!(39, "RTTY_45", Some("rtty")),
    e!(40, "RTTY_50", Some("rtty:baud=50,shift=170")),
    e!(41, "RTTY_75", Some("rtty:baud=75,shift=850")),
    e!(42, "AMTOR_FEC", None),
    e!(43, "THROB_1", None),
    e!(44, "THROB_2", None),
    e!(45, "THROB_4", None),
    e!(46, "THROBX_1", None),
    e!(47, "THROBX_2", None),
    e!(146, "THROBX_4", None),
    e!(204, "CONTESTIA_4_125", Some("contestia4_125")),
    e!(55, "CONTESTIA_4_250", Some("contestia4_250")),
    e!(54, "CONTESTIA_4_500", Some("contestia4_500")),
    e!(255, "CONTESTIA_4_1000", Some("contestia4_1000")),
    e!(254, "CONTESTIA_4_2000", Some("contestia4_2000")),
    e!(169, "CONTESTIA_8_125", Some("contestia8_125")),
    e!(49, "CONTESTIA_8_250", Some("contestia8_250")),
    e!(52, "CONTESTIA_8_500", Some("contestia8_500")),
    e!(117, "CONTESTIA_8_1000", Some("contestia8_1000")),
    e!(247, "CONTESTIA_8_2000", Some("contestia8_2000")),
    e!(275, "CONTESTIA_16_250", Some("contestia16_250")),
    e!(50, "CONTESTIA_16_500", Some("contestia16_500")),
    e!(53, "CONTESTIA_16_1000", Some("contestia16_1000")),
    e!(259, "CONTESTIA_16_2000", Some("contestia16_2000")),
    e!(51, "CONTESTIA_32_1000", Some("contestia32_1000")),
    e!(201, "CONTESTIA_32_2000", Some("contestia32_2000")),
    e!(194, "CONTESTIA_64_500", Some("contestia64_500")),
    e!(193, "CONTESTIA_64_1000", Some("contestia64_1000")),
    e!(191, "CONTESTIA_64_2000", Some("contestia64_2000")),
    e!(56, "VOICE", None),
    e!(60, "MFSK8", Some("mfsk8")),
    e!(57, "MFSK16", Some("mfsk16")),
    e!(147, "MFSK32", Some("mfsk32")),
    e!(148, "MFSK11", Some("mfsk11")),
    e!(152, "MFSK22", Some("mfsk22")),
    e!(61, "RTTYM_8_250", None),
    e!(62, "RTTYM_16_500", None),
    e!(63, "RTTYM_32_1000", None),
    e!(65, "RTTYM_8_500", None),
    e!(66, "RTTYM_16_1000", None),
    e!(67, "RTTYM_4_500", None),
    e!(68, "RTTYM_4_250", None),
    e!(119, "RTTYM_8_1000", None),
    e!(170, "RTTYM_8_125", None),
    e!(203, "OLIVIA_4_125", Some("olivia:tones=4,bw=125")),
    e!(75, "OLIVIA_4_250", Some("olivia:tones=4,bw=250")),
    e!(74, "OLIVIA_4_500", Some("olivia:tones=4,bw=500")),
    e!(229, "OLIVIA_4_1000", Some("olivia:tones=4,bw=1000")),
    e!(238, "OLIVIA_4_2000", Some("olivia:tones=4,bw=2000")),
    e!(163, "OLIVIA_8_125", Some("olivia:tones=8,bw=125")),
    e!(69, "OLIVIA_8_250", Some("olivia:tones=8,bw=250")),
    e!(72, "OLIVIA_8_500", Some("olivia:tones=8,bw=500")),
    e!(116, "OLIVIA_8_1000", Some("olivia:tones=8,bw=1000")),
    e!(214, "OLIVIA_8_2000", Some("olivia:tones=8,bw=2000")),
    e!(70, "OLIVIA_16_500", Some("olivia:tones=16,bw=500")),
    e!(73, "OLIVIA_16_1000", Some("olivia:tones=16,bw=1000")),
    e!(234, "OLIVIA_16_2000", Some("olivia:tones=16,bw=2000")),
    e!(71, "OLIVIA_32_1000", Some("olivia:tones=32,bw=1000")),
    e!(221, "OLIVIA_32_2000", Some("olivia:tones=32,bw=2000")),
    e!(211, "OLIVIA_64_2000", Some("olivia:tones=64,bw=2000")),
    e!(76, "PAX", None),
    e!(77, "PAX2", None),
    e!(78, "DOMINOF", None),
    e!(79, "FAX", None),
    e!(81, "SSTV", None),
    e!(84, "DOMINOEX_4", Some("dominoex4")),
    e!(85, "DOMINOEX_5", Some("dominoex5")),
    e!(86, "DOMINOEX_8", Some("dominoex8")),
    e!(87, "DOMINOEX_11", Some("dominoex11")),
    e!(88, "DOMINOEX_16", Some("dominoex16")),
    e!(90, "DOMINOEX_22", Some("dominoex22")),
    e!(92, "DOMINOEX_4_FEC", None),
    e!(93, "DOMINOEX_5_FEC", None),
    e!(97, "DOMINOEX_8_FEC", None),
    e!(98, "DOMINOEX_11_FEC", None),
    e!(99, "DOMINOEX_16_FEC", None),
    e!(101, "DOMINOEX_22_FEC", None),
    e!(104, "FELD_HELL", Some("feldhell")),
    e!(105, "PSK_HELL", None),
    e!(106, "HELL_80", Some("hell80")),
    e!(107, "FM_HELL_105", None),
    e!(108, "FM_HELL_245", None),
    e!(114, "MODE_141A", None),
    e!(123, "DTMF", None),
    e!(125, "ALE400", None),
    e!(131, "FDMDV", None),
    e!(132, "JT65_A", Some("jt65")),
    e!(134, "JT65_B", None),
    e!(135, "JT65_C", None),
    e!(136, "THOR_4", Some("thor4")),
    e!(137, "THOR_8", Some("thor8")),
    e!(138, "THOR_16", Some("thor16")),
    e!(139, "THOR_5", Some("thor5")),
    e!(143, "THOR_11", Some("thor11")),
    e!(145, "THOR_22", Some("thor22")),
    e!(153, "CALL_ID", None),
    e!(155, "PACKET_PSK1200", None),
    e!(156, "PACKET_PSK250", None),
    e!(159, "PACKET_PSK63", None),
    e!(172, "MODE_188_110A_8N1", None),
    e!(0, "NONE", None),
];

/// Extended ID table (`RSID_LIST2`), reached via an ESCAPE prefix.
/// ref: rsid_defs.cxx:232-306.
pub static TABLE2: &[RsidEntry] = &[
    e!(450, "PSK63RX4", Some("psk63rc4")),
    e!(457, "PSK63RX5", Some("psk63rc5")),
    e!(458, "PSK63RX10", Some("psk63rc10")),
    e!(460, "PSK63RX20", Some("psk63rc20")),
    e!(462, "PSK63RX32", Some("psk63rc32")),
    e!(467, "PSK125RX4", Some("psk125rc4")),
    e!(497, "PSK125RX5", Some("psk125rc5")),
    e!(513, "PSK125RX10", Some("psk125rc10")),
    e!(519, "PSK125X12", Some("psk125c12")),
    e!(522, "PSK125RX12", Some("psk125rc12")),
    e!(527, "PSK125RX16", Some("psk125rc16")),
    e!(529, "PSK250RX2", Some("psk250rc2")),
    e!(533, "PSK250RX3", Some("psk250rc3")),
    e!(539, "PSK250RX5", Some("psk250rc5")),
    e!(541, "PSK250X6", Some("psk250c6")),
    e!(545, "PSK250RX6", Some("psk250rc6")),
    e!(551, "PSK250RX7", Some("psk250rc7")),
    e!(553, "PSK500RX2", Some("psk500rc2")),
    e!(558, "PSK500RX3", Some("psk500rc3")),
    e!(564, "PSK500RX4", Some("psk500rc4")),
    e!(566, "PSK500X2", Some("psk500c2")),
    e!(569, "PSK500X4", Some("psk500c4")),
    e!(570, "PSK1000", Some("psk1000")),
    e!(580, "PSK1000R", Some("psk1000r")),
    e!(587, "PSK1000X2", Some("psk1000c2")),
    e!(595, "PSK1000RX2", None),
    e!(604, "PSK800RX2", None),
    e!(610, "PSK800X2", None),
    e!(620, "MFSK64", Some("mfsk64")),
    e!(625, "MFSK128", Some("mfsk128")),
    e!(639, "THOR25x4", Some("thor25x4")),
    e!(649, "THOR50x1", Some("thor50x1")),
    e!(653, "THOR50x2", Some("thor50x2")),
    e!(658, "THOR100", Some("thor100")),
    e!(662, "DOMINOEX_44", Some("dominoex44")),
    e!(681, "DOMINOEX_88", Some("dominoex88")),
    e!(687, "MFSK31", Some("mfsk31")),
    e!(691, "DOMINOEX_MICRO", Some("dominoexmicro")),
    e!(693, "THOR_MICRO", Some("thormicro")),
    e!(1026, "MFSK64L", Some("mfsk64l")),
    e!(1029, "MFSK128L", Some("mfsk128l")),
    e!(1066, "PSK8P125", None),
    e!(1071, "PSK8P250", None),
    e!(1076, "PSK8P500", None),
    e!(1047, "PSK8P1000", None),
    e!(1037, "PSK8P125F", None),
    e!(1038, "PSK8P250F", None),
    e!(1043, "PSK8P500F", None),
    e!(1078, "PSK8P1000F", None),
    e!(1058, "PSK8P1200F", None),
    e!(1239, "PSK8P125FL", None),
    e!(2052, "PSK8P250FL", None),
    e!(2053, "OFDM500F", None),
    e!(2094, "OFDM7F0F", None),
    e!(2118, "OFDM2000", None),
    e!(2110, "OFDM2000F", None),
    e!(1171, "IFKP", None),
    e!(0, "NONE2", None),
];

/// The RSID burst(s) to emit for an active mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RsidTx {
    /// A single primary-table burst of this rs code.
    Primary(u16),
    /// An ESCAPE burst followed by this secondary-table rs code burst.
    Extended(u16),
}

/// Resolve an omnimodem mode string to the RSID burst that identifies it, or
/// `None` if the mode has no assigned RSID. ref: rsid.cxx:746-950 (`assigned`).
pub fn tx_for_mode(mode: &str) -> Option<RsidTx> {
    if let Some(en) = TABLE1.iter().find(|e| e.mode == Some(mode)) {
        return Some(RsidTx::Primary(en.code));
    }
    if let Some(en) = TABLE2.iter().find(|e| e.mode == Some(mode)) {
        return Some(RsidTx::Extended(en.code));
    }
    None
}

/// Synthesize the RSID burst audio for `tx` at transmit offset `tx_offset_hz`,
/// sampled at `sample_rate`. ref: rsid.cxx:996-1092 (`send`). Audio is
/// FP-domain (never asserted bit-exact); the tone-index sequence it renders is
/// `encode(code)` and is bit-exact.
pub fn encode_tx(tx: RsidTx, tx_offset_hz: f32, sample_rate: u32) -> Vec<Sample> {
    let sr = sample_rate as f32;
    // ref: rsid.cxx:1020 — symlen = floor(RSID_SYMLEN * sr), RSID_SYMLEN = 1024/11025.
    let symlen = (1024.0 / RSID_SAMPLE_RATE * sr).floor() as usize;
    let mut out = Vec::new();
    let mut phase = 0.0f64;

    // 5 symbol-periods of leading silence.
    out.extend(std::iter::repeat_n(0.0, 5 * symlen));

    let (first, second) = match tx {
        RsidTx::Primary(code) => (code, None),
        RsidTx::Extended(code) => (RSID_ESCAPE, Some(code)),
    };

    append_tones(&mut out, &mut phase, first, tx_offset_hz, sr, symlen);

    if let Some(code2) = second {
        // 10 symbol-periods of silence between the two bursts.
        out.extend(std::iter::repeat_n(0.0, 10 * symlen));
        // fldigi restarts the phase accumulator for the second burst.
        phase = 0.0;
        append_tones(&mut out, &mut phase, code2, tx_offset_hz, sr, symlen);
    }

    // 5 symbol-periods of trailing silence.
    out.extend(std::iter::repeat_n(0.0, 5 * symlen));
    out
}

/// Convenience: the burst audio for an omnimodem mode string, or `None`.
pub fn burst_for_mode(mode: &str, tx_offset_hz: f32, sample_rate: u32) -> Option<Vec<Sample>> {
    Some(encode_tx(tx_for_mode(mode)?, tx_offset_hz, sample_rate))
}

/// The end-of-transmission burst. ref: rsid.cxx:952-994 (`send_eot`).
pub fn encode_eot(tx_offset_hz: f32, sample_rate: u32) -> Vec<Sample> {
    encode_tx(RsidTx::Primary(RSID_EOT), tx_offset_hz, sample_rate)
}

// ref: rsid.cxx:1032-1049 — render one 15-tone burst with continuous phase.
fn append_tones(
    out: &mut Vec<Sample>,
    phase: &mut f64,
    code: u16,
    tx_offset_hz: f32,
    sr: f32,
    symlen: usize,
) {
    let rsid = encode(code);
    // fr = tx_offset - RSID_SAMPLE_RATE * 7 / 1024; tone step = RSID_SAMPLE_RATE/1024.
    let fr = tx_offset_hz as f64 - (RSID_SAMPLE_RATE as f64 * 7.0 / 1024.0);
    let step = RSID_SAMPLE_RATE as f64 / 1024.0;
    for &tone in rsid.iter() {
        let freq = fr + tone as f64 * step;
        let phaseincr = 2.0 * std::f64::consts::PI * freq / sr as f64;
        for _ in 0..symlen {
            *phase += phaseincr;
            if *phase > 2.0 * std::f64::consts::PI {
                *phase -= 2.0 * std::f64::consts::PI;
            }
            out.push(phase.sin() as f32);
        }
    }
}

// ---------------------------------------------------------------------------
// Receive detector — ref: rsid.cxx:228-740.
// ---------------------------------------------------------------------------

/// A detected RSID: the identified mode tag, its omnimodem mode string (if
/// ported), the audio offset it was found at, and whether it came from the
/// extended (ESCAPE-prefixed) table.
#[derive(Debug, Clone, PartialEq)]
pub struct RsidDetection {
    pub tag: &'static str,
    pub mode: Option<&'static str>,
    pub freq_hz: f32,
    pub extended: bool,
}

/// Streaming RSID detector. Feed it RX audio at any sample rate; it resamples to
/// 11025 Hz internally, runs a 2048-pt Hamming-windowed FFT hopped by 512
/// samples, reduces each frame to per-bin peak tone-slots, and matches a 30-deep
/// time ring against both ID tables.
pub struct RsidDetector {
    resampler: Option<Resampler>,
    fft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    inbuf: Vec<f32>,
    scratch: Vec<Complex<f32>>,
    ampl: Vec<f32>,
    /// `buckets[t][bin]` = peak tone-slot (0..15) at time slice `t`, oldest at 0.
    buckets: Vec<Vec<u8>>,
    codes1: Vec<[u8; NSYMBOLS]>,
    codes2: Vec<[u8; NSYMBOLS]>,
    /// Max Hamming distance still accepted (0/1/2, i.e. `RsID_label_type`).
    resolution: i32,
    /// Frames remaining in the ESCAPE→secondary search window (0 = primary).
    secondary_countdown: i32,
}

impl RsidDetector {
    /// Build a detector for `input_rate` audio. `resolution` is the max accepted
    /// symbol-mismatch count (fldigi's coarse/normal/precise = 2/1/0); 1 is a
    /// good default.
    pub fn new(input_rate: u32, resolution: u8) -> Self {
        let resampler = (input_rate != RSID_SAMPLE_RATE as u32)
            .then(|| Resampler::new(input_rate, RSID_SAMPLE_RATE as u32, 16));
        let mut planner = FftPlanner::new();
        let fft = planner.plan_fft_forward(FFT_SIZE);
        // ref: rsid.cxx:100 — hamming window over the full 2048-sample array.
        let window: Vec<f32> = (0..FFT_SIZE).map(|i| hamming(i as f32 / FFT_SIZE as f32)).collect();
        RsidDetector {
            resampler,
            fft,
            window,
            inbuf: Vec::with_capacity(FFT_SIZE * 2),
            scratch: vec![Complex::new(0.0, 0.0); FFT_SIZE],
            ampl: vec![0.0; NBINS],
            buckets: vec![vec![0u8; NBINS]; NTIMES],
            // Exclude the trailing NONE/NONE2 sentinel from the searchable set:
            // its code 0 encodes to all-zero symbols, which would match silence.
            // ref: rsid_defs.cxx:227,316 — `rsid_ids_size = sizeof/sizeof - 1`.
            codes1: TABLE1[..TABLE1.len() - 1].iter().map(|e| encode(e.code)).collect(),
            codes2: TABLE2[..TABLE2.len() - 1].iter().map(|e| encode(e.code)).collect(),
            resolution: resolution.min(2) as i32,
            secondary_countdown: 0,
        }
    }

    /// Drop all soft detection state.
    pub fn reset(&mut self) {
        self.inbuf.clear();
        for row in self.buckets.iter_mut() {
            row.iter_mut().for_each(|b| *b = 0);
        }
        self.secondary_countdown = 0;
    }

    /// Feed RX audio; returns any RSIDs identified in this batch.
    pub fn feed(&mut self, samples: &[Sample]) -> Vec<RsidDetection> {
        let resampled = match self.resampler.as_mut() {
            Some(r) => r.process(samples),
            None => samples.to_vec(),
        };
        self.inbuf.extend_from_slice(&resampled);
        let mut out = Vec::new();
        while self.inbuf.len() >= FFT_SIZE {
            self.search_frame(&mut out);
            self.inbuf.drain(0..HOP);
        }
        out
    }

    // ref: rsid.cxx:275-366 (`search`).
    fn search_frame(&mut self, out: &mut Vec<RsidDetection>) {
        // Windowed FFT over the leading 2048 samples.
        for i in 0..FFT_SIZE {
            self.scratch[i] = Complex::new(self.inbuf[i] * self.window[i], 0.0);
        }
        self.fft.process(&mut self.scratch);
        // ref: rsid.cxx:308 — pscale = 4 / (RSID_FFT_SIZE^2).
        let pscale = 4.0 / (NBINS as f32 * NBINS as f32);
        for i in 0..NBINS {
            self.ampl[i] = self.scratch[i].norm_sqr() * pscale;
        }

        // Slide the time ring up one slot; newest row is zeroed then filled.
        self.buckets.rotate_left(1);
        {
            let newest = self.buckets.last_mut().unwrap();
            newest.iter_mut().for_each(|b| *b = 0);
            // ref: rsid.cxx:330-331 — both bin parities, over
            // [bucket_low, bucket_high - NTIMES) == [3, 992 - 30). This is exactly
            // the bin span search_table reads, so no bucket is computed unused.
            calculate_buckets(&self.ampl, newest, 3, (NBINS - 32) - NTIMES);
            calculate_buckets(&self.ampl, newest, 4, (NBINS - 32) - NTIMES);
        }

        // ref: rsid.cxx:338-364 — primary table until an ESCAPE opens the
        // secondary window; then the extended table until it times out.
        if self.secondary_countdown <= 0 {
            if let Some((idx, bin)) = self.search_table(true) {
                let en = TABLE1[idx];
                if en.code == RSID_ESCAPE {
                    // 10 gap + 15 symbols + 2 slack, in symbols → frames (×2).
                    self.secondary_countdown = 27 * (SYMLEN_SAMPLES / HOP) as i32;
                } else {
                    out.push(self.detection(en, bin, false));
                    self.clear_ring();
                }
            }
        } else {
            self.secondary_countdown -= 1;
            if let Some((idx, bin)) = self.search_table(false) {
                let en = TABLE2[idx];
                if en.code != 0 {
                    out.push(self.detection(en, bin, true));
                }
                self.clear_ring();
            } else if self.secondary_countdown <= 0 {
                self.clear_ring();
            }
        }
    }

    // ref: rsid.cxx:700-740 (`search_amp`).
    fn search_table(&self, primary: bool) -> Option<(usize, usize)> {
        let codes = if primary { &self.codes1 } else { &self.codes2 };
        let mut best_dist = i32::MAX;
        let mut best = None;
        // ref: rsid.cxx:722 — bins [nBinLow, nBinHigh - NTIMES).
        let bin_hi = (NBINS - 32) - NTIMES;
        for (ci, code) in codes.iter().enumerate() {
            for bin in 3..bin_hi {
                let d = self.hamming(bin, code);
                if d < best_dist {
                    best_dist = d;
                    best = Some((ci, bin));
                    if d == 0 {
                        break;
                    }
                }
            }
        }
        if best_dist <= self.resolution {
            best
        } else {
            None
        }
    }

    // ref: rsid.cxx:687-698 (`HammingDistance`) — compare the 15 symbols against
    // the ring at odd time rows 1,3,…,29.
    fn hamming(&self, bin: usize, code: &[u8; NSYMBOLS]) -> i32 {
        let mut dist = 0;
        for (i, &sym) in code.iter().enumerate() {
            if self.buckets[2 * i + 1][bin] != sym {
                dist += 1;
                if dist > self.resolution {
                    break;
                }
            }
        }
        dist
    }

    fn detection(&self, en: RsidEntry, bin: usize, extended: bool) -> RsidDetection {
        // ref: rsid.cxx:609 — rsidfreq = (bin + NSYMBOLS - 0.5) * RSID_SAMPLE_RATE / 2048.
        let freq_hz = (bin as f32 + NSYMBOLS as f32 - 0.5) * RSID_SAMPLE_RATE / FFT_SIZE as f32;
        RsidDetection { tag: en.tag, mode: en.mode, freq_hz, extended }
    }

    /// Drop the detection state after a hit so the same burst is not re-reported.
    /// Unlike fldigi's `reset()` (rsid.cxx:162) this deliberately does NOT flush
    /// `inbuf`: as a streaming tap we keep unprocessed audio (a re-trigger needs a
    /// fresh 30-slot ring fill regardless), rather than discarding samples the
    /// caller already handed us.
    fn clear_ring(&mut self) {
        for row in self.buckets.iter_mut() {
            row.iter_mut().for_each(|b| *b = 0);
        }
        self.secondary_countdown = 0;
    }
}

// ref: rsid.cxx:198-226 (`CalculateBuckets`) — for each bin `i` in the range,
// store which of the 16 tone slots (0..15) in the window `[i, i+2*15]` peaks.
fn calculate_buckets(spectrum: &[f32], row: &mut [u8], ibegin: usize, iend: usize) {
    let mut amp_max = 0.0f32;
    let mut ibucket_max: i32 = ibegin as i32 - 2;
    let mut i = ibegin;
    while i < iend {
        if ibucket_max == i as i32 - 2 {
            amp_max = spectrum[i];
            ibucket_max = i as i32;
            let mut j = i + 2;
            while j < i + NTIMES + 2 {
                if spectrum[j] > amp_max {
                    amp_max = spectrum[j];
                    ibucket_max = j as i32;
                }
                j += 2;
            }
        } else {
            let j = i + NTIMES;
            if spectrum[j] > amp_max {
                amp_max = spectrum[j];
                ibucket_max = j as i32;
            }
        }
        row[i] = ((ibucket_max - i as i32) >> 1) as u8;
        i += 2;
    }
}

/// Hamming window at fractional position `x ∈ [0,1)`. ref: fldigi misc.h `hamming`.
fn hamming(x: f32) -> f32 {
    0.54 - 0.46 * (2.0 * std::f32::consts::PI * x).cos()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A record parsed from the reference vector `tests/vectors/rsid.json`.
    struct Ref {
        table: u8,
        code: u16,
        tag: String,
        syms: [u8; NSYMBOLS],
    }

    fn load_reference() -> Vec<Ref> {
        let raw = include_str!("../../tests/vectors/rsid.json");
        let mut out = Vec::new();
        for line in raw.lines() {
            if !line.contains("\"syms\"") {
                continue; // skip the _meta line
            }
            let field = |key: &str| -> &str {
                let k = format!("\"{key}\":");
                let start = line.find(&k).unwrap() + k.len();
                &line[start..]
            };
            let num = |key: &str| -> i64 {
                field(key).trim_start_matches('"').split([',', '"']).next().unwrap().parse().unwrap()
            };
            let tag = {
                let s = field("tag").trim_start_matches('"');
                s[..s.find('"').unwrap()].to_string()
            };
            let syms_str = {
                let s = field("syms");
                &s[s.find('[').unwrap() + 1..s.find(']').unwrap()]
            };
            let mut syms = [0u8; NSYMBOLS];
            for (i, tok) in syms_str.split(',').enumerate() {
                syms[i] = tok.trim().parse().unwrap();
            }
            out.push(Ref { table: num("table") as u8, code: num("code") as u16, tag, syms });
        }
        out
    }

    #[test]
    fn encode_matches_reference_for_every_id() {
        // Bit-exact KAT: encode(code) == the reference RS symbols for all 204 IDs.
        let refs = load_reference();
        assert_eq!(refs.len(), 204, "expected 204 reference IDs, got {}", refs.len());
        for r in &refs {
            assert_eq!(encode(r.code), r.syms, "RS symbols differ for {} (code {})", r.tag, r.code);
        }
    }

    #[test]
    fn id_tables_agree_with_reference_codes_and_tags() {
        // Our in-code ID tables must carry exactly the reference (code, tag) pairs
        // in order, so a detection reports the same tag fldigi would.
        let refs = load_reference();
        let ours: Vec<(u8, u16, &str)> = TABLE1
            .iter()
            .map(|e| (1u8, e.code, e.tag))
            .chain(TABLE2.iter().map(|e| (2u8, e.code, e.tag)))
            .collect();
        assert_eq!(ours.len(), refs.len());
        for (o, r) in ours.iter().zip(refs.iter()) {
            assert_eq!((o.0, o.1, o.2), (r.table, r.code, r.tag.as_str()));
        }
    }

    #[test]
    fn known_symbol_sequences() {
        // Spot-check a couple of codes against the extractor output, independent
        // of the vector loader.
        assert_eq!(encode(263), [8, 10, 3, 14, 12, 12, 5, 1, 7, 1, 14, 5, 8, 3, 7]); // EOT
        assert_eq!(encode(57), [0, 1, 0, 14, 9, 15, 8, 1, 15, 7, 8, 14, 6, 6, 9]); // MFSK16
    }

    #[test]
    fn tx_for_mode_resolves_primary_and_extended() {
        assert_eq!(tx_for_mode("psk31"), Some(RsidTx::Primary(1)));
        assert_eq!(tx_for_mode("mfsk16"), Some(RsidTx::Primary(57)));
        assert_eq!(tx_for_mode("mfsk64"), Some(RsidTx::Extended(620)));
        assert_eq!(tx_for_mode("thor100"), Some(RsidTx::Extended(658)));
        assert_eq!(tx_for_mode("bogus"), None);
    }

    /// Round-trip: synth a burst for a mode at a known offset, feed it to the
    /// detector, and confirm the tag/mode and offset are recovered. Loopback gate
    /// (Doctrine §3): the audio is FP; the recovered tag is bit-exact and the
    /// frequency is within RSID precision.
    fn loopback(mode: &str, offset: f32, rate: u32) -> RsidDetection {
        let burst = burst_for_mode(mode, offset, rate).expect("mode has an RSID");
        let mut det = RsidDetector::new(rate, 1);
        // Pad with a little silence so the detector's ring warms up cleanly.
        let mut audio = vec![0.0f32; rate as usize / 10];
        audio.extend_from_slice(&burst);
        audio.extend(std::iter::repeat_n(0.0, rate as usize / 10));
        let found = det.feed(&audio);
        assert_eq!(found.len(), 1, "expected exactly one detection for {mode}, got {found:?}");
        found.into_iter().next().unwrap()
    }

    #[test]
    fn detector_recovers_primary_mode_at_8k() {
        let d = loopback("mfsk16", 1500.0, 8000);
        assert_eq!(d.tag, "MFSK16");
        assert_eq!(d.mode, Some("mfsk16"));
        assert!(!d.extended);
        assert!((d.freq_hz - 1500.0).abs() < 12.0, "freq {} not near 1500", d.freq_hz);
    }

    #[test]
    fn detector_recovers_primary_mode_at_12k() {
        let d = loopback("psk31", 1000.0, 12000);
        assert_eq!(d.tag, "BPSK31");
        assert_eq!(d.mode, Some("psk31"));
        assert!((d.freq_hz - 1000.0).abs() < 12.0, "freq {} not near 1000", d.freq_hz);
    }

    #[test]
    fn detector_recovers_extended_mode_via_escape() {
        // mfsk64 lives in the extended table; TX prefixes an ESCAPE burst, and the
        // detector must follow the ESCAPE into the secondary table.
        let d = loopback("mfsk64", 1500.0, 8000);
        assert_eq!(d.tag, "MFSK64");
        assert_eq!(d.mode, Some("mfsk64"));
        assert!(d.extended);
        assert!((d.freq_hz - 1500.0).abs() < 12.0, "freq {} not near 1500", d.freq_hz);
    }

    #[test]
    fn detector_finds_nothing_in_silence_or_noise() {
        let mut det = RsidDetector::new(8000, 1);
        let silence = vec![0.0f32; 8000];
        assert!(det.feed(&silence).is_empty());
        // A deterministic pseudo-noise burst should not trip a false positive.
        let noise: Vec<f32> = (0..8000u32)
            .map(|n| {
                let r = n.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                ((r >> 16 & 0x7fff) as f32 / 32768.0 - 0.5) * 0.2
            })
            .collect();
        assert!(det.feed(&noise).is_empty(), "false RSID detection on noise");
    }
}
