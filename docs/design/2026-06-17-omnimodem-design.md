# Omnimodem — Design

**Date:** 2026-06-17
**Status:** Approved for planning (direction approved; review feedback incorporated)

## Summary

Omnimodem is a gRPC-driven, building-block-based multi-mode software modem written
in Rust. A single binary is operated entirely over gRPC and can run multiple
amateur-radio modes simultaneously, each bound to its own audio interface and
PTT. It generalizes the reception techniques that make the Graywolf AFSK
demodulator best-in-class into mode-agnostic building blocks so that adding a new
mode is cheap, and it fixes the audio-device-identity and PTT robustness gaps
that were the weakest parts of Graywolf.

Three design goals drive everything:

1. **Easy to add new modes** — reusable DSP/FEC "building blocks" and a one-module mode
   registry, so a new mode touches one place, not five `match` arms.
2. **Easy to integrate with** — a stable, versioned gRPC API so developers build
   their own frontends in any language.
3. **Rock-solid audio detection and PTT** — stable device identity across
   renames/hotplug, structured errors, and a transmitter you can trust.

Plus useful metrics (SNR, signal level, bad frames, DCD, duty cycle, etc.).

## Architecture

A hard line separates an **async control edge** from a **synchronous DSP/audio/PTT
core**. This preserves Graywolf's correct decision never to async-ify the hot
path (Graywolf's core is plain `std::thread`, no tokio — and it works).

- **Control edge** (tonic + tokio): gRPC handlers only. They validate requests
  and translate them into commands sent over an `mpsc` into the sync core.
  Events flow back out through a `tokio::broadcast` that fans out to subscriber
  streams. No DSP runs here.
- **Sync core** (plain threads): owns audio, demod/mod, and PTT. No async in the
  sample path.

Four concepts:

- **AudioInterface** — owns one audio device (capture and/or playback).
- **Mode instance** — pure DSP demod/mod for one mode.
- **Client** — a gRPC consumer (frontend).
- **Channel** — ties them together: `audio → demod → frames + metrics` on RX,
  and `frame → mod → audio + PTT` on TX.

A **Supervisor** owns the live channels, the device cache, the persistence store,
and the shared PTT registry. (Graywolf's `struct Modem` already plays this role
for in-memory state — `modem/mod.rs:139`; Supervisor is its evolution, not a
greenfield invention.)

### Concurrency note

Multi-channel concurrency is **not new** — Graywolf already runs multiple
channels concurrently by processing every channel on a single thread in
`pump_all_audio` (`modem/mod.rs`). Omnimodem moves to **per-channel RX/TX worker
threads** instead. This is a deliberate choice of parallelism (better for many
channels × ensemble demods on multi-core) over Graywolf's single-thread
simplicity, made consciously because best-of-class reception runs several
demodulators per channel. The only cross-channel shared state is the PTT
registry and the device cache.

## gRPC control surface

A `ModemControl` service:

- **Unary RPCs** for command-and-control (configure audio / channel / PTT,
  start/stop, transmit, query state) — each with acknowledgements and request
  validation.
- **Server-streaming `SubscribeEvents`** for RX frames, DCD transitions, status
  changes, audio levels, and metrics, with a **state-snapshot replay on
  subscribe** so a reconnecting client is never stale.

Graywolf already has a working custom protobuf-over-UDS/TCP IPC
(`ipc/server.rs`) but it is **single-client by design** (`ipc/server.rs:18-19`).
gRPC is chosen specifically to serve goal #2: polyglot codegen and standard
streaming let third parties build frontends without reimplementing a bespoke
framing protocol. The existing `proto/graywolf.proto` is a good starting
vocabulary to lift.

**Backpressure policy (must specify, not hand-wave).** `tokio::broadcast`
returns `Lagged(n)` and silently drops messages for slow receivers. Decoded RX
frames must **never** be silently dropped. Policy:

- **Frames: lossless.** Per-client bounded queue; on overflow, either buffer or
  disconnect the client — never discard a decoded frame.
- **Telemetry (levels, metrics): lossy.** Dropping intermediate samples is fine;
  only the latest value matters.

**Local authorization (transmitting is a legal act).** Opening the control
socket means the ability to key a transmitter under the operator's license, so
authz is required even on the default local transport, not only when routable:

- Default transport: **UDS** (or TCP loopback). On UDS, enforce socket-file mode
  and `SO_PEERCRED` peer-uid checks. Note explicitly that loopback TCP exposes
  every local user.
- **mTLS + per-method authz is mandatory** if the service is ever bound to a
  routable interface.

**API versioning.** Because third-party frontends are the whole point, publish a
stability/versioning policy for the proto from day one (semver on the package,
additive-only within a major).

## Mode framework

`trait Demodulator` / `trait Modulator`, each declaring a `ModeCaps`
(native sample rate, bandwidth, TX support, duplex). A **mode registry** means
adding a mode touches one module instead of ~5 `match` sites — directly fixing
Graywolf's #1 weakness (ad-hoc enum + flat config struct). Per-mode config is
**parametric**: `enum ModeConfig { Afsk{..}, G3ruh9600{..}, Psk{..}, Rtty{..},
Ft8{..} }`, not one flat struct.

### Streaming AND block/windowed modes — first-class from day one

This is the most important correction from review. Every Graywolf demod (AFSK,
PSK, 9600) emits hard-sliced bits **directly** into per-slicer `HdlcDecoder`s —
there is no symbol or soft-bit interface (`demod_afsk.rs:637-644`). A trait whose
contract is "feed samples, get HDLC frames" fits the continuous/HDLC family but
**cannot express FT8/JS8/WSPR**, which are an early target. WSJT-X-family modes
are:

- **windowed and time-aligned** (e.g. 15 s slots), decoded multi-pass over a
  whole buffer, producing multiple decodes per window;
- **FEC-heavy and soft-decision** (LDPC + Costas sync), with **no HDLC**;
- **dependent on an accurate clock**.

So the mode abstraction supports two demod shapes from the start:

1. **Streaming demod** — `feed(samples) -> Vec<Frame>` (AFSK/PSK/RTTY/9600).
2. **Block/windowed demod** — buffers a time-aligned window and runs a
   multi-pass decode, returning multiple decodes (FT8/JS8/WSPR).

And the pipeline carries **soft information (LLRs)** end-to-end, not just hard
bits — otherwise the listed Viterbi/LDPC building blocks are useless and FEC-mode
reception is not best-of-class. Frame assembly (HDLC and friends) moves
downstream of the demod rather than living inside it.

### Best-of-class reception, generalized

`ParallelDemodulator<D>` generalizes Graywolf's standout multi-decoder ensemble
("hydra") into a reusable pattern for any mode: run N decoder configurations in
parallel and union/dedup their outputs. Note carefully:

- The **pattern** generalizes; the **specific profiles do not.** Graywolf's
  Profile A (no-limit / hard-limit) + Profile B FM-discriminator are
  AFSK-specific. PSK's diversity axis is different (loop bandwidths, etc.). The
  registry composes per-mode ensembles, not one fixed profile set.
- Dedup-by-`(content, sample-offset)` within a ~3-symbol window
  (`demod_afsk_multi.rs`) is the HDLC-streaming mechanism. Windowed modes dedup
  by decoded-message + time-slot instead.

### The pipeline-stage model — why this is the key to "as good or better"

A mode is **not** a monolithic demod. Every reference implementation (WSJT-X,
fldigi, Direwolf, Codec2/FreeDV, M17, ARDOP) is internally a chain of stages:

```
RX:  front-end DSP → synchronizer → symbol detector → soft-LLR demapper
        → de-interleave / descramble → FEC decode → frame/message decode
TX:  message/frame encode → FEC encode → interleave / scramble
        → symbol map → pulse-shape / modulate → up-convert
```

omnimodem provides these stages as **composable, individually-testable
building blocks**, and a mode is an *assembly* of stages declared in its one registry
module. This is precisely what makes "as-good-or-better-than-the-reference"
tractable: we implement the best known version of each stage *once* (e.g. one
soft-decision LDPC belief-propagation decoder, one Costas-array correlator) and
every mode that needs it inherits best-in-class behaviour, rather than each mode
re-deriving a weaker version. The catalog below is sized so that **every mode in
the GRA-126 catalog composes from this set** — that is the completeness bar.

**The soft-information (LLR) contract is the spine.** The single
highest-leverage abstraction is a defined soft-value (log-likelihood-ratio)
interface between the detector/demapper and the FEC decoder. Every modern weak-
signal gain — LDPC BP/OSD (FT8/FreeDV), soft Viterbi (M17/MFSK16), Franke-Taylor
soft RS (JT65), Walsh/FHT correlation (Olivia), memory-ARQ soft-combine (ARDOP)
— depends on carrying soft values, not hard bits, across that boundary. (This is
the same point the review's "soft-decision plumbing" note raised, now made the
organizing principle of the toolkit.)

### Building-blocks catalog

Derived from auditing the GRA-126 reference codebases. Grouped by pipeline stage;
verified parameters are noted so the list is checkably complete, not hand-wavy.

**A. Front-end DSP & waveform (RX detectors + symmetric TX modulators)**
- Resampler / decimator (arbitrary source rate → per-mode working rate; 12 kHz is
  the WSJT-X norm, 8 kHz voice, 48 kHz AFSK) — also closes Graywolf's "source must
  match the demod rate" gap.
- Tunable NCO / complex down-converter + channelizer (passband isolation,
  click-to-tune on the waterfall).
- Overlapped STFT/FFT engine (waterfall **and** noncoherent tone detection;
  configurable window/hop — FT8 uses ~160 ms windows, ~40 ms hop).
- Filter toolkit: FIR/IIR, **RRC** (Direwolf AFSK, M17 α=0.5), raised-cosine
  envelope (PSK31/Throb/Hell), **Gaussian/CPFSK shaping** (configurable BT — FT8
  BT=2.0, FT4 BT=1.0), low-pass baseband shaping (G3RUH), Hilbert/analytic.
- Pre/de-emphasis + per-tone AGC (AFSK amplitude balancing); peak/valley &
  decision-feedback AGC (lifted from Graywolf — see reuse map).
- FM discriminator / phase-difference detector (WEFAX, FSK); envelope detector +
  adaptive attack/decay squelch (CW, Hell OOK).
- Modulators/detectors, each TX+RX: **CPFSK/GFSK** (FT8/FT4/Q65/FST4), **M-FSK**
  tone bank (MFSK16/32, Olivia, JT65 65-FSK, WSPR 4-FSK, M17/ARDOP 4-FSK),
  **M-PSK** incl. differential BPSK/QPSK/8PSK (PSK31, FreeDV 1600/700C, ARDOP),
  **16-QAM** (ARDOP), **OQPSK/MSK** coherent (MSK144), **OFDM core** (carrier bank,
  IFFT/FFT, cyclic-prefix, per-carrier EQ, **PAPR/clipping** — FreeDV 700D/E/2020,
  ARDOP), **2-FSK shift** (RTTY/NAVTEX, selectable shift), **OOK column-raster**
  (Hellschreiber).
- Per-bin noise-floor estimator + uniform SNR reporter (normalized to a reference
  bandwidth, WSJT-X-style).

**B. Synchronization & acquisition**
- DPLL clock/bit-sync with locked/searching inertia (lift from Graywolf;
  Direwolf parity).
- Symbol-timing recovery variants: Gardner/early-late (PSK), async start-bit edge
  (RTTY/NAVTEX), DFT-impulse timing (MFSK/DominoEX), transition-minimum (PSK31).
- Costas-loop / PLL carrier recovery (coherent PSK); **AFC** frequency-offset
  estimation + drift tracking (fldigi-grade, with matched-filter re-centering).
- **Costas-array** generator + correlator (parameterized N — 7×7 ×3 for FT8,
  4×4 ×4 for FT4) — distinct from the Costas *loop* above.
- Generic known-sequence / sync-word / preamble correlator: M17 16-bit sync words,
  ARDOP leader, JT65/JT9/WSPR pseudo-random sync vectors, FST4 sync groups, IL2P
  24-bit `0xF15E48`, **FX.25 64-bit CTAG with fuzzy/Hamming-distance matching**,
  NAVTEX phasing.
- Pilot-symbol OFDM coarse/fine freq+timing sync with drift tracking (FreeDV).
- **Candidate finder**: sweep the passband, produce a sync-metric-sorted candidate
  list `(freq, time, metric)` — the front half of wideband multi-decode.

**C. FEC & coding (soft-decision throughout)**
- **Soft-LLR demapper** (tone power / phase → per-bit LLR, noise-variance scaled) —
  the contract named above.
- **LDPC** encoder + belief-propagation/min-sum decoder (parametric H-matrix) +
  **ordered-statistics-decoding (OSD)** layer for the last ~2 dB. Ship matrices for
  (174,91) FT8/FT4, (128,80) MSK144, (240,101) FST4/FST4W, and the FreeDV
  rate-½/rate-0.8 matrices.
- **Convolutional** encoder + **soft Viterbi** (parametric R/K/polys — K=5 M17 &
  fldigi QPSK, K=7 MFSK16/THOR/DominoEX) + **puncturing/depuncturing** (M17 P1/P2/P3).
- **Convolutional K=32, r=½ + Fano sequential decoder** (JT9, WSPR — Viterbi is
  impractical at K=32).
- **Reed-Solomon over GF(256)**, parametric `(nroots, fcr, prim)` + shortened/zero-
  padded blocks — must instantiate **fcr=1 (FX.25)** *and* **fcr=0 (IL2P)** *and*
  ARDOP; nroots ∈ {2,4,6,8,16,32,64}.
- **Reed-Solomon over GF(2⁶) with a soft-decision (Franke-Taylor) decoder**
  (JT65 RS(63,12)); **QRA (Q-ary repeat-accumulate) over GF(2⁶)** (Q65 QRA(63,13)).
- **Golay(23,12)/(24,12)** (FreeDV 1600, M17 LICH).
- **Walsh–Hadamard / Fast-Hadamard-Transform block codec**, soft, parametric size
  (64 = Olivia, 32 = Contestia).
- **Constant-ratio (4-of-7) codec + time-diversity delay-and-combine** (CCIR-476 /
  NAVTEX / SITOR-B).
- **Interleavers**: block/QPP (M17), depth-L convolutional (DominoEX/THOR),
  self-synchronizing diagonal (MFSK16), bit-reversal (WSPR), long time-interleaver
  across fades (FreeDV 700D).
- **Scramblers — three distinct primitives** (a common pitfall): self-synchronizing
  multiplicative LFSR (G3RUH **x¹⁷+x¹²+1**), frame-reset additive LFSR (IL2P
  **x⁹+x⁴+1**, fixed seed), and additive PRBS/decorrelator (M17, Olivia whitening).
- **NRZI** codec; **Gray-code** mapper; **differential** encode/decode.
- **CRC library**, parametric: CRC-16/X.25 FCS (AX.25), CRC-14 `0x6757` (FT8/FT4),
  CRC-24 (FST4), CRC-16 `0x5935` (M17), CRC-8 (MSK144).
- **Diversity / soft-combiner** (FreeDV 700C frequency diversity) and **memory-ARQ
  soft-combine buffer** (ARDOP — reusable math even though the ARQ policy is not).
- **Frame-dedup across parallel decoders** (Direwolf `PROCESS_AFTER_BITS`-style
  hold window + CRC/retry scoring; Graywolf `(content, offset)` window).

**D. Source / message / framing coding**
- **Varicode** with pluggable tables (PSK, MFSK/IZ8BLY, DominoEX/THOR nibble).
- **Baudot/ITA2** (LTRS/FIGS shift), **CCIR-476** alphabet (NAVTEX), **Morse** +
  fuzzy/SOM best-fit decoder (CW).
- **WSJT-X 77-bit message codec**: type field, standard exchange, free text,
  telemetry; **28-bit callsign compression + 10/12/22-bit callsign hashing** (shared
  hash table); **15-bit Maidenhead grid** + power. Plus legacy **72-bit** (JT65/JT9)
  and **50-bit** (WSPR/FST4W) packers.
- **HDLC** (flag/stuff/destuff/FCS) + **AX.25/APRS** (UI-frame application convention);
  **FX.25** (CTAG table + RS wrap of an *intact* HDLC frame, legacy-compatible);
  **IL2P** (sync + 6-bit callsign-transposing header + per-block RS, HDLC-replacing).
- **Image raster** framer/renderer: WEFAX (IOC/LPM scaling, phasing, slant
  correction); Hell column-scan font rasteriser.
- **Vocoder interface** — a clean Codec2 (1300/3200/700C) and LPCNet boundary so
  voice modes plug a vocoder into the bit pipeline. Voice frames carry **opaque
  vocoder payload**, so the `Frame` type must be payload-agnostic (text, packet, or
  codec bits).

**E. Decode orchestration — the WSJT-X-class differentiators**
- **ParallelDemodulator<D>** diversity ensemble (the Graywolf "hydra") — diversity
  *within one signal*.
- **Wideband multi-signal decode** — decode *every* signal in the passband at once
  (dozens of FT8 stations per 15 s window; the fldigi PSK/CW "browser"). Parallel/
  threaded processing of the candidate-finder list, per-candidate timeout, and a
  decode-**depth** knob (BP-only vs BP+OSD search order). This is distinct from, and
  composes with, `ParallelDemodulator`.
- **Multi-pass successive interference cancellation**: decode → re-encode → estimate
  amplitude/phase/timing → **subtract from the time-domain waveform** → re-decode to
  expose masked signals. The single biggest reason WSJT-X out-decodes naïve
  implementations on crowded bands.
- **A-priori (AP) decoding**: a QSO-state manager seeds known/likely content (own
  call, DX call, `CQ`, grid) as strong a-priori LLRs (~+2–4 dB; compounds across a
  QSO). Optional for FT8/JT65, effectively always-on for FT4/Q65/FST4.
- **ARQ engine hooks** (sequencing, retransmit, bandwidth/rate negotiation, memory-
  ARQ) for ARDOP-class transports — a per-mode *protocol* layer above the DSP/coding
  library, not a shared DSP building block.

**F. Metrics & validation (feeds goal #4)**
- Per-mode metrics: SNR (normalized to reference BW), sync metric, freq/time offset,
  EVM / PSK31-style IMD / phase-quality, DCD, good/bad-CRC counts, decode pass#,
  duty cycle.
- **Reference-corpus regression harness**: golden recordings with known-good decode
  counts (e.g. the WA8LMF AFSK test CD where Graywolf already beats Direwolf; WSJT-X
  sample `.wav`s), run in CI to *prove* as-good-or-better and guard DSP regressions.
  This extends the record/replay + regression-harness already in the design and is
  how the "as good or better" claim is made falsifiable rather than aspirational.

### Layering: shared library vs per-mode protocol

The clean split is a **shared DSP/coding library** (everything in A–C above, plus
the source-coding codecs and the orchestration mechanisms in E that are pure math)
with the soft-LLR contract as its interface, and a **thin per-mode protocol layer**
on top (M17 LSF/Stream/Packet framing, ARDOP ARQ state machine, FreeDV bit-mapping,
Winlink session). A mode's registry module wires shared stages together and adds
only its protocol specifics.

### Honest scope note

This is a **large library, and extracting it is a real refactor, not a
lift-and-shift.** Today Graywolf's AFSK demod owns `Vec<HdlcDecoder>` and frames
internally, DCD scoring is copy-pasted between AFSK and 9600 (`demod_afsk.rs:691-723`
and `modem_9600/mod.rs:172-182`), and PSK/9600 share no code with AFSK; the reusable
`MultiSlicer` / `DcdScorer` / `DpllClockRecovery` / `ParallelDemodulator<D>`
abstractions do not exist yet, and the soft-LLR contract is new. The catalog above
is the *target* building block set; it must be phased. Suggested grouping by build phase:
the streaming/packet building blocks (A front-end, B sync, NRZI/scramblers/HDLC/RS/FX.25/
IL2P, the dedup window) land first with 1200/9600; the soft-LLR contract + LDPC
BP/OSD + Costas-array + 77-bit codec + SIC/AP/wideband orchestration land with FT8;
the convolutional/Viterbi/FHT/interleaver/Varicode family lands with the fldigi
modes; OFDM + vocoder + ARQ land with the FreeDV/M17/ARDOP family. A few low-level
constants (exact LDPC min-sum scaling, AP LLR seeding, FT8 sync-metric threshold, the
IL2P `set_field` bit map) should be confirmed against the reference sources at
implementation time rather than locked from secondary documentation now.

### Coverage map — reference software → building blocks

| Reference (GRA-126) | Modes | Key building blocks the framework must supply |
|---|---|---|
| **Direwolf / Graywolf** | AX.25 1200 AFSK, 9600 G3RUH, FX.25, IL2P, APRS | AFSK correlator + multi-slicer, baseband-FSK LPF/slicer, DPLL, NRZI, self-sync **and** frame-reset scramblers, HDLC+CRC-16/X.25, GF(256) RS (fcr=0 **and** 1), FX.25/IL2P framing, multi-decoder dedup |
| **WSJT-X** | FT8, FT4, JT65, JT9, WSPR, MSK144, Q65, FST4/W | STFT bank, CPFSK/MSK detect, soft-LLR, LDPC BP+OSD, GF(2⁶) soft-RS, QRA, conv-K32+Fano, Costas-array correlator, 77/72/50-bit codecs + call hashing, SIC + AP + wideband multi-decode, accurate time base |
| **fldigi** | PSK31/63/QPSK, RTTY, MFSK16/32, Olivia, Contestia, THOR, DominoEX, Hell, Throb, CW, WEFAX, NAVTEX | differential PSK + raised-cosine, 2-FSK + Baudot, MFSK tone bank, conv+soft-Viterbi+interleavers, Walsh/FHT, constant-ratio + time-diversity, Varicode tables, AFC, Morse+SOM, image raster |
| **Codec2/FreeDV, M17, ARDOP** | FreeDV 700C/D/E/1600/2020, M17, ARDOP | OFDM core + pilot sync + PAPR clipping, coherent/diff PSK, 4-FSK+RRC, QAM, LDPC soft, Golay, conv+puncture+QPP, diversity & memory-ARQ soft-combine, vocoder interface, sync-word correlator, ARQ engine |

## Audio subsystem (goal #3)

Lift Graywolf's hardened bits — defensive I16 format selection,
stream-rebuild-with-backoff, submitted/drained TX watermarks, ALSA
canonicalization pure-functions, `probe_capture`, in-use device caching
(all in `audio/soundcard.rs`). Fixes:

- **A real `trait AudioBackend`** (cpal / file / stdin / SDR / JACK pluggable)
  replacing Graywolf's ad-hoc spawn-functions dispatched by a `match`
  (`modem/mod.rs:449-480`).
- **A unified cross-platform `DeviceId`** collapsing Graywolf's two diverging
  enumeration paths (cpal/ALSA `list_audio.rs` vs nusb `list_usb.rs`) and their
  per-OS identity mismatch into one stable identity derived from durable
  attributes (USB `idVendor:idProduct` + serial, ALSA stable card *name* not
  index, USB port-topology as fallback; prefer `/dev/serial/by-id/` symlinks).
- **Resampling** so a source rate need not match the demod rate. Crucially, this
  must **retain** Graywolf's ALSA `plughw` format/rate hardening
  (`audio/mod.rs`, `soundcard.rs`) — the 48 kHz ceiling exists to avoid ALSA's
  synthetic-rate `plughw` trap that desyncs bit timing. Resampling is additive;
  it does not replace that defensive selection.
- **Capture fan-out** — one capture stream can feed several demods (1200 + 9600
  on the same audio, or SDR slices). Opt-in; 1:1 is the default. (Graywolf's
  `extra_demods` already proves this works.)

## PTT subsystem (goal #3)

Lift the whole PTT subsystem — `trait PttDriver` + factory + `PortRegistry`
(multi-handle) + per-OS drivers + `drive_tx_cycle` no-sleep sequencing +
unkey-on-Drop (`tx/ptt.rs`, `modem/tx_worker.rs`). Fixes:

- **Structured `PttError` enum** replacing the stringly-typed `Result<(),
  String>` errors (`ptt.rs:184-189`), so callers can distinguish
  device-went-away vs permission-denied vs busy.
- **Hotplug eviction for serial / CM108.** `PortRegistry` currently caches serial
  fds by path string and never evicts them (`ptt.rs:484-487` documents this as a
  known limitation); GPIO has `LineGone` eviction, serial does not. Since
  "rock-solid PTT" is a primary goal, the `DeviceId` work must **evict and reopen
  by `DeviceId` on device disappearance/hotplug**, not only resolve at startup.
- **RX/TX interlock per channel.** When a channel keys PTT on a shared device, RX
  decode on that device must be muted/skipped to avoid decoding our own
  transmission or feedback. Graywolf handles this implicitly on its single
  thread; the per-channel-thread model must make it explicit.

## Persistence

Config is persisted in a **SQLite** file owned by the modem (frontends are
arbitrary external clients, so config must outlive any single client and be
shared across them). Key rules:

- **Key config on the stable `DeviceId`, never on the volatile `/dev` path**, so a
  TNC that jumps `ttyUSB0 → ttyUSB1` still binds. At startup and on hotplug the
  core resolves each stored `DeviceId` to its current device node.
- **Keep SQLite off the DSP hot path.** Writes happen on the control edge or a
  dedicated thread — never in the audio pump — so a disk hiccup can't become an
  audio underrun.
- **`SuggestUdevRule(device_id)` RPC** returns ready-to-install udev rule text
  (keyed on vendor/product/serial or topology, producing a stable
  `/dev/omnimodem/<label>` symlink) plus instructions — useful for two identical
  adapters that `by-id` can't disambiguate. The modem only *suggests*; it never
  writes to `/etc/udev` (root-owned; operator stays in control).

## TX model

- **Cooperative queue + optional exclusive lease.** TX frames from any client
  queue on the channel's TX worker and serialize on-air. Sessions that can't
  tolerate interleaving (contest/Winlink) take an **optional exclusive TX lease**.
- **Per-channel TX worker** (improvement over Graywolf's single global TX worker
  in `tx_worker.rs`, which needlessly serializes TX across independent radios).
  **Rule:** two channels that share one physical rig must still serialize via the
  shared PTT registry — concurrency is per-rig, not per-channel.
- **Time-slot-aligned scheduling** for windowed modes: the TX worker must be able
  to transmit precisely on the next even/odd slot boundary (e.g. FT8's 15 s
  grid), not "as soon as queued."

## Time synchronization

WSJT-X-family modes require an accurate system clock. Omnimodem depends on the
host clock being disciplined (NTP/PTP) and **surfaces clock offset as a metric**
so operators can see when decode failures are a time-sync problem rather than a
signal problem.

## Metrics (goal #4)

Per-channel over gRPC plus an optional Prometheus exporter: SNR, dBFS level, DCD
state, good/bad-FCS counts, PTT state, duty cycle. Additional high-value metrics:

- **Which ensemble member / slicer decoded each frame** (Graywolf has this data;
  it's gold for tuning the hydra).
- **AFC / frequency-error offset.**
- **Audio over/underrun and clip counts.**
- **Clock offset** (for WSJT-X modes; see Time synchronization).

## Testing & verification (proving the modem does what it says)

For a modem, "good test coverage" is not a line-percentage target — the bar is
**provable correctness on the air**. The strategy is three layers of proof plus
engineering hygiene, and the architecture is deliberately shaped to make each
layer cheap: the pipeline-stage model makes every stage independently testable;
the soft-LLR contract lets a decoder be tested in isolation from any front-end;
`trait AudioBackend` / `trait PttDriver` let the whole RX/TX path run in CI
against file/loopback/mock backends with **no hardware**; and record/replay to
FLAC gives deterministic, re-runnable real-world signals.

### Layer 1 — Conformance (bit-exact to the published standard)

Proves interoperability, not just self-consistency.

- **Known-answer tests against published vectors** for every coding block: CRCs
  (CRC-16/X.25, CRC-14 `0x6757`, CRC-24, M17 CRC-16/`0x5935`), Reed-Solomon, LDPC,
  Golay, Viterbi, the scramblers, NRZI, Gray, Varicode/Baudot/CCIR-476, and the
  FT8 77-bit packer + callsign hashing — checked against vectors from the standards
  and the reference codebases (`ft8_lib`, `libm17`, Direwolf, codec2).
- **Cross-decode interop — the decisive test.** Modulate with omnimodem → decode
  with the reference software, *and* the reverse, in both directions: our FT8 TX
  into WSJT-X `jt9`; WSJT-X `ft8sim` output into our decoder; our AX.25 frames
  through Direwolf `atest`; Direwolf `gen_packets` into our demod; M17 against
  `libm17`. Passing both directions *is* the definition of "doing what we say."
- **Modulator golden snapshots.** Modulate a fixed message and diff the symbol
  stream / waveform against a stored golden vector (`insta`-style) so any change
  that alters on-air output is caught in review.

### Layer 2 — Performance (as-good-or-better, quantified)

Proves the best-of-class-reception claim with numbers, not adjectives.

- **BER / frame-decode-rate vs SNR curves.** Per mode, sweep Eb/N0 (or SNR in a
  2500 Hz reference BW) with a **seeded** AWGN source, measure bit-error / decode
  rate, assert the curve meets a committed threshold, **and compare against the
  reference implementation's curve on the same inputs** — CI fails if we regress or
  fall behind. This mirrors how WSJT-X (`ft8sim`/`wsprsim`), codec2 (OFDM/LDPC BER
  tooling), and Direwolf (WA8LMF TEST CD via `atest`) validate themselves; we adopt
  their method and gate on it.
- **Channel simulators, not just AWGN.** These modes are built for fading channels,
  so AWGN-only testing overstates performance. Ship deterministic, seedable
  fixtures: Watterson HF fading (CCIR good/moderate/poor), multipath, frequency
  offset + drift, fractional-symbol timing offset, and impulse noise.
- **Reference-corpus regression harness** (cross-referenced from Mode framework §F
  and Other features): golden recordings — WA8LMF AFSK CD (where Graywolf already
  beats Direwolf), WSJT-X / fldigi sample files — with known-good decode counts; CI
  asserts decode count ≥ reference and fails on regression. This is the headline
  "as good or better" proof.

### Layer 3 — Robustness (won't fall over)

- **Property-based tests** (`proptest`): round-trip invariants over randomized
  inputs/parameters — `descramble∘scramble = id`, `decode∘encode = id`, FEC corrects
  ≤ t errors and detects > t, interleaver/NRZI/Gray round-trip.
- **Fuzzing** (`cargo-fuzz`/`arbitrary`) of every parser/framer (HDLC, IL2P, FX.25,
  message decoders) and the gRPC surface: malformed input must never panic, over-read,
  or wedge the core — only reject cleanly.
- **Error-path & hardware-failure tests** via mock backends: audio stream-rebuild/
  backoff, TX watermark draining, unkey-on-Drop, serial/CM108 hotplug eviction, the
  `PttError` branches — all without real hardware.
- **gRPC contract tests:** request validation + acks; the backpressure policy (a
  deliberately slow subscriber drops telemetry but **never** loses a decoded frame);
  state-snapshot replay on subscribe; UDS peer-cred authz.

### Hygiene & CI

- **Real-time-path guards:** the sample loop is benchmarked (`criterion`; Graywolf
  has `demod_bench`) and asserted allocation-free/bounded — on a real-time modem a
  perf regression *is* a correctness bug (underruns drop frames).
- **Metrics accuracy:** feed a calibrated known-SNR signal and assert reported
  SNR/AFC-offset match within tolerance — prove the telemetry is true, not merely
  present.
- **Coverage measured** (`cargo-llvm-cov`) with a high bar on the DSP/FEC/framing
  crates specifically, treated as necessary-but-not-sufficient — Layers 1–2 are the
  real proof.
- **CI tiering:** fast unit / round-trip / conformance-vector tests on every PR; the
  slow BER sweeps, channel-sim runs, fuzz batches, and reference-binary interop +
  corpus jobs run nightly (or behind a label), since they need the reference
  toolchains installed and minutes of CPU.

**Definition of done for a mode:** its conformance vectors pass, cross-decode with
the reference works **both** directions, and its BER/decode-rate curve meets the
committed threshold. Until then the mode is not "done," regardless of a happy-path
loopback demo.

## Other features

- **Record/replay to FLAC** plus a **decode-rate regression harness** over a known
  corpus — serves the best-of-class-reception goal and guards DSP regressions.
- **Reference CLI/TUI client.**
- **KISS/AGWPE compatibility** is **out of scope for the core**: it belongs in a
  separate external KISS↔gRPC translator process (future work), so existing TNC
  apps work without polluting the core.

## Resolved decisions

1. **Language: Rust.** (Maximizes Graywolf reuse.)
2. **Persistence: SQLite**, keyed on stable `DeviceId`, with `SuggestUdevRule`
   for the device-rename problem.
3. **Build order:** (1) modem service structure — gRPC + control plane + PTT +
   audio I/O, provable end-to-end via loopback/level-metering and a real
   key/unkey before any DSP exists; (2) the audio building blocks — DSP/FEC
   toolkit + mode framework (streaming **and** block paths, soft-decision
   plumbing); (3) modes — 1200 AFSK first, then FT8, then outward.
4. **Multi-client TX:** cooperative queue + optional exclusive lease.
5. **KISS/AGWPE:** external translator, not in the core (future work).

## Graywolf reuse map — the AFSK "secret sauce" as mode-agnostic building blocks

Lift these out of the AFSK demod so PSK/RTTY/9600/FT8 inherit them. All verified
present in the Graywolf source:

- **Profile ensemble ("hydra")** — Profile A no-hard-limit, Profile A +
  hard-limit, Profile B FM-discriminator run in parallel, outputs unioned
  (`demod_afsk_multi.rs`) → generic `ParallelDemodulator<D>` (pattern only;
  profiles are AFSK-specific).
- **Multi-slicer** — N slicers (default 9) deciding mark-vs-space at different
  thresholds via the geometric space-gain table (`demod_afsk.rs:425-433, 530`) →
  generic `MultiSlicer`.
- **Decision-feedback AGC** (independent mark/space reference tracking, W7ION's
  technique) **and** peak/valley envelope AGC (fast-attack/slow-decay) — note
  these are **mutually exclusive** in Graywolf: DFB runs only in single-slicer
  mode (`demod_afsk.rs:507`), multi-slicer uses peak/valley. Model them in the
  registry as alternatives selected by slicer count, not as two independently
  composable building blocks.
- **Hard-limiter-before-bandpass** correlator stage — `sign(x)`, keep
  zero-crossing timing (`demod_afsk.rs:459-461, 545-547`).
- **Digital PLL clock recovery** with locked-vs-searching inertia
  (`demod_afsk.rs:626-678`) → generic `DpllClockRecovery`.
- **DCD scoring with hysteresis** (shift-register popcount on/off thresholds,
  `demod_afsk.rs:691-723`) → generic `DcdScorer` (currently duplicated in
  `modem_9600`).
- **Content+offset frame dedup** windowed to ~3 symbol times
  (`demod_afsk_multi.rs`).
- **SIMD-friendly 8-accumulator FIR** (`demod_afsk.rs:67-107`).
- **256-entry cos/sin oscillator lookup tables** — note these are **`f32`
  floating-point** tables indexed by the top 8 bits of a phase accumulator
  (`demod_afsk.rs:40-60`), not fixed-point as previously described.

W7ION (Ion Todirel) attribution for the decision-feedback AGC, the hard-limiter
correlator, and the hydra idea carries over into omnimodem's code, same as
Graywolf.

## Phasing

1. **Foundations + vertical slice** — workspace, lift DSP/HDLC, mode framework
   (streaming + block traits, soft-decision plumbing), 1200 AFSK with the
   ensemble end-to-end over gRPC + cpal + PTT. Prove gRPC backpressure and local
   authz here. **Stand up the test harness in this phase, not later:** the seeded
   AWGN/channel simulators, the BER/decode-rate runner, conformance-vector tests,
   and the reference-binary interop + corpus jobs (see Testing & verification).
   Every mode added thereafter ships with its Layer-1/2 gates or it isn't "done."
2. **Audio/device hardening** — `AudioBackend`, unified `DeviceId`, resampling,
   capture fan-out, hotplug + serial-PTT eviction.
3. **Breadth + observability** — 9600 / PSK31 / RTTY → WSJT-X family (exercising
   the block/windowed + time-slot-TX paths); metrics / Prometheus; record/replay
   + regression harness.
4. **Integration + safety** — reference TUI, TX exclusive lease, mTLS for
   routable binds. (KISS/AGWPE translator is separate future work.)
