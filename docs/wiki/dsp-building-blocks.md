# DSP building blocks & the mode framework

The `omnimodem-dsp` crate ([`../../crates/dsp/`](../../crates/dsp/)) is a **pure
library** — no dependency on the daemon. Every block is individually testable and
gated by known-answer vectors (`crates/dsp/tests/kat.rs`). A mode is an *assembly*
of these blocks, declared in one registry module, so adding a mode touches one place
instead of five `match` arms.

## The soft-information (LLR) contract — the spine

[`types.rs`](../../crates/dsp/src/types.rs). The single highest-leverage
abstraction: a defined soft-value interface between the detector/demapper and the
FEC decoder. Every modern weak-signal gain (LDPC BP/OSD, soft Viterbi, soft RS,
Walsh/FHT correlation) depends on carrying **soft values, not hard bits**, across
that boundary.

| Type | Meaning |
|---|---|
| `Sample = f32` | Normalized real audio sample. |
| `Cplx` | Complex baseband sample. |
| `Llr = f32` | Log-likelihood ratio `ln(P(bit=0)/P(bit=1))`. Positive ⇒ bit 0 more likely; hard-slice is `bit = (L < 0)`. Magnitude scales with confidence/SNR. |
| `SoftBits` | A vector of `Llr` carried demapper → FEC. |
| `Frame` | A decoded result: a `FramePayload` + `FrameMeta`. |
| `FramePayload` | `Packet` / `Text` / `Message77` / `Vocoder` / `Image` — payload-agnostic so voice and raster modes fit the same pipeline. |
| `FrameMeta` | SNR, frequency/time offset, decoder name, sample offset (dedup key), CRC status. |

**Rule:** the detector↔FEC boundary is defined entirely by `Llr` and its sign
convention. Every FEC decoder in `fec/` consumes that exact convention; don't mix
sign conventions across the boundary.

## The mode framework

[`mode.rs`](../../crates/dsp/src/mode.rs). Two demod shapes are first-class, because
a "feed samples, get frames" trait alone cannot express the WSJT-X windowed modes.

```rust
trait Demodulator {          // streaming: AFSK, PSK31, RTTY, CW, MFSK, …
    fn caps(&self) -> ModeCaps;
    fn feed(&mut self, samples: &[Sample]) -> Vec<Frame>;
    fn reset(&mut self);
    fn flush(&mut self) -> Vec<Frame> { Vec::new() }
}
trait BlockDemodulator {     // windowed/time-aligned: FT8, JS8, WSPR, …
    fn caps(&self) -> ModeCaps;
    fn decode_window(&mut self, window: &[Sample], window_start_ns: u64) -> Vec<Frame>;
}
trait Modulator {            // symmetric TX
    fn caps(&self) -> ModeCaps;
    fn modulate(&mut self, frame: &Frame) -> Result<Vec<Sample>, ModError>;
}
```

`ModeCaps` declares `native_rate`, `bandwidth_hz`, `tx` support, `duplex`
(`Duplex::Half`/`Full`), and `shape` (`DemodShape::Streaming` vs
`Windowed { window_s, period_s }`). The daemon's RX worker switches on
`caps().shape`: streaming modes get `feed()` per audio chunk, windowed modes get
`decode_window()` per slot boundary.

### Registering a mode (daemon side)

[`../../crates/omnimodem/src/mode/`](../../crates/omnimodem/src/mode/):

1. `mode/mod.rs::ModeConfig` — a parametric enum, one variant per mode family, with
   `ModeConfig::parse(mode_string)` handling bare labels (`"ft8"`) and parametric
   forms (`"cw:wpm=25,tone=600"`). Typed proto `ModeParams` are first turned into
   the equivalent string in `grpc/service.rs::effective_mode`.
2. `mode/registry.rs::demod_kind(cfg)` — returns `DemodKind::None` /
   `Streaming(Box<dyn Demodulator>)` / `Windowed(Box<dyn BlockDemodulator>,
   window_s)`; `build_modulator(cfg)` returns the TX side (or `None` for RX-only).
   Each arm just constructs the DSP crate's mode type (e.g. `Afsk1200Demod::ensemble(9)`).

So a new mode = its implementation in `crates/dsp/src/modes/` + one arm each in
`ModeConfig`/`demod_kind`/`build_modulator` (+ a `ModeParams` variant in the proto
if it's parametric).

## The diversity ensemble ("hydra")

[`ensemble.rs`](../../crates/dsp/src/ensemble.rs) — `ParallelDemodulator<D>`
generalizes Graywolf's standout multi-decoder pattern: run N decoder configurations
in parallel and union/dedup their outputs (dedup by content + sample-offset within a
short window). The *pattern* is reusable; the specific profiles are per-mode
(AFSK's profile set differs from PSK's diversity axis).

## The pipeline stages (building-block catalog)

RX pipeline: `front-end DSP → synchronizer → symbol detector → soft-LLR demapper →
de-interleave/descramble → FEC decode → frame/message decode`. TX is the mirror.
Blocks are grouped by stage.

### A. Front-end DSP & waveform — [`frontend/`](../../crates/dsp/src/frontend/)

| Block | File |
|---|---|
| NCO / sine oscillator | `osc.rs`, `nco.rs` |
| FIR filter design/apply | `fir.rs` |
| Resampler | `resample.rs` |
| Overlapped STFT/FFT engine (waterfall + tone detection) | `stft.rs` |
| Waterfall spectrum quantization (dBFS → uint8 bins) | `spectrum.rs` |
| AGC (peak/valley + decision-feedback) | `agc.rs` |
| Hard limiter | `limiter.rs` |
| FM discriminator / envelope detector + squelch | `detector.rs` |
| Noise-floor / SNR estimator | `noise.rs` |
| MSK modulate/demodulate | `msk.rs` |
| OFDM core (overlapping-Walsh carriers) | `ofdm.rs` |
| Multicarrier tone bank | `multicarrier.rs` |
| Symmetric modulators (FSK/PSK/AM/FM) | `modulate.rs` |
| RSID burst detector/generator | `rsid.rs` |

### B. Synchronization & acquisition — [`sync/`](../../crates/dsp/src/sync/)

| Block | File |
|---|---|
| DPLL bit-clock recovery (locked/searching inertia) | `dpll.rs` |
| Symbol-timing recovery (Gardner/early-late) | `timing.rs` |
| DCD scoring with hysteresis | `dcd.rs` |
| Costas *loop* (coherent carrier recovery + AFC) | `costas.rs` |
| Costas *array* generator + correlator (FT8/FT4 sync) | `costas_array.rs` |
| Sync-word / preamble correlator | `syncword.rs` |
| Wideband candidate finder (`(freq, time, metric)` list) | `candidate.rs` |

Note the two Costas blocks are different things: the *loop* tracks carrier phase;
the *array* is a frequency-hop sync pattern with a thumbtack autocorrelation.

### C. FEC & coding (soft-decision throughout) — [`fec/`](../../crates/dsp/src/fec/)

| Block | File |
|---|---|
| Soft-LLR demapper (tone/phase → per-bit LLR) | `llr.rs` |
| LDPC encoder + belief-propagation/min-sum decoder (FT8/FT4) | `ldpc.rs` |
| LDPC variants: FST4, JS8, MSK144 | `ldpc_fst4.rs`, `ldpc_js8.rs`, `ldpc_msk144.rs` |
| Ordered-statistics decoding (OSD) layer | `osd.rs` |
| Convolutional encoder + soft Viterbi | `conv.rs` |
| Fano sequential decoder (K=32: JT9, WSPR) | `fano.rs` |
| Reed-Solomon GF(256), parametric `(nroots, fcr, prim)` (FX.25 fcr=1, IL2P fcr=0) | `rs.rs` |
| Reed-Solomon GF(2⁶) soft (Franke-Taylor; JT65) | `rs_gf64.rs` |
| Golay(24,12) | `golay.rs` |
| Fast-Hadamard-Transform block codec (Olivia/Contestia) | `fht.rs` |
| Constant-ratio CCIR-476 (NAVTEX/SITOR-B) | `ccir476.rs` |
| Interleavers | `interleave.rs` |
| Scramblers | `scramble.rs` |
| NRZI codec | `nrzi.rs` |
| Gray-code mapper | `gray.rs` |
| CRC library (parametric) | `crc.rs` |
| LLR → hard-bit slicer | `slicer.rs` |
| FT8 / JS8 code tables (LDPC matrices etc.) | `ft8_tables.rs`, `js8_tables.rs` |

### D. Source / message / framing coding — [`framing/`](../../crates/dsp/src/framing/)

| Block | File |
|---|---|
| HDLC (flag/stuff/destuff, CRC-16/X.25 FCS) | `hdlc.rs` |
| AX.25 UI frames | `ax25.rs` |
| FX.25 (RS wrap of an intact HDLC frame) | `fx25.rs` |
| IL2P (RS-coded header/payload, HDLC-replacing) | `il2p.rs` |
| Varicode (PSK) + pluggable nibble tables | `varicode.rs`, `dominoex_varicode.rs`, `thor_varicode.rs`, `ifkp_varicode.rs`, `fsq_varicode.rs` |
| Baudot/ITA2 (RTTY) | `baudot.rs` |
| Morse + fuzzy decoder (CW) | `morse.rs` |
| WSJT-X 77-bit message codec + callsign hashing | `pack77.rs`, `message77.rs` |
| JS8 message/frame/callsign + dictionary compression | `js8_message.rs`, `js8_frames.rs`, `js8_callsign.rs`, `jsc.rs` (+ `jsc_dict.bin`) |
| Hellschreiber font rasteriser | `hellfont.rs` |

### E. Decode orchestration / mode framework

`mode.rs` (traits), `ensemble.rs` (hydra), and — for windowed modes — the
candidate-finder + per-candidate decode + soft interference handling live inside the
individual mode files under `modes/` (see [`mode-catalog.md`](mode-catalog.md)).

## Testing model

`testutil.rs` provides a **seeded** RNG (deterministic, no system randomness), AWGN,
and a Watterson HF-fading channel, so BER sweeps and corpus generation are
bit-reproducible. The proof bar for a mode is not coverage percentage but
cross-decode interop against the reference implementation (both directions) and a
BER/decode-rate curve at least as good as the reference at every SNR point. Test
tiers live in `crates/dsp/tests/` (`kat.rs`, `roundtrip.rs`, `ber.rs`,
`snapshots.rs`, `loopback.rs`, `alloc_guard.rs`) and `crates/dsp/benches/hotpath.rs`.
