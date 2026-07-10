# RTL-SDR (`rtl_tcp`) — Phase A, Plan 2: Backend + Control/Telemetry Seam — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: subagent-driven-development / executing-plans, strict TDD, frequent commits. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Land the `rtl_tcp` audio-input backend so omnimodem can receive off a
(local or remote) RTL-SDR dongle with no external glue. It connects to an
`rtl_tcp` server, tunes the dongle, reads raw u8 IQ, demodulates to mono audio
via the Plan-1 `NbfmReceiver`, and streams a wideband RF waterfall — all behind
the existing `AudioBackend` seam, so every downstream mode (AFSK1200/APRS first)
works unmodified.

**Depends on:** Plan 1 (merged, #100) — consumes `u8_iq_to_cplx`,
`NbfmReceiver`, `ComplexStft`, `full_spectrum_dbfs`, `SpectrumPlan::new_centered`.

**Build/test:** `CARGO_TARGET_DIR=/tmp/omni-target CARGO_INCREMENTAL=0 cargo test -p omnimodemd`.
Never run `cargo fmt`. Commit as `chrissnell` only, no AI attribution.

---

## The seam (concrete realization of the approved design)

The approved seam is **`SdrControl` = `Arc`-of-atomics for runtime control** +
**telemetry-sender-into-backend for the RF waterfall**. The wiring problem: the
`AudioBackendFactory` is `Fn(&DeviceDescriptor) -> Box<dyn AudioBackend>` — it
knows the device but not the channel, and the core-wide telemetry broadcast is
not in scope where the factory is *built*. Both *are* in scope in
`core::configure_audio` (it has the `ChannelId` and the `telemetry` sender).

Realization, honoring "factory branch in `production_core`":

1. **Factory branch** (`production_core`): matches `DeviceId::RtlTcp { host, port }`
   and constructs `RtlTcpBackend::new(host, port)` before the cpal fallback. No
   channel/telemetry needed at this point.
2. **Context injection** (`AudioBackend::attach_sdr_context`, default no-op):
   `configure_audio` — which owns the `ChannelId`, the `telemetry` sender, and
   the per-channel shared `SdrControl` (stored in `LiveBindings::sdr_controls`,
   mirroring `gains`/`spectra`) — injects them into the backend *after*
   construction and *before* `open_capture`. Non-SDR backends ignore it.
3. **One waterfall producer per channel:** for an SDR-bound channel,
   `try_spawn_workers` hands the RX worker a throwaway `SpectrumControl::default()`
   (never enable-able), so the audio-passband tap can never turn on; the SDR
   capture thread is the sole spectrum producer.
4. **Ad-hoc resolve:** `configure_audio` synthesizes a capture-only
   `DeviceDescriptor` for a `DeviceId::RtlTcp` instead of requiring it to be
   physically enumerated (bind `rtltcp:host:port` with no pre-registration).

`SdrControl` and the `rtl_tcp` wire code live in `crate::audio::rtlsdr`;
`backend.rs` and `core` `use` them. `TelemetryEvent` is `crate::core::event` — an
audio→core module reference, which Rust permits within one crate.

---

### Task 1: `DeviceId::RtlTcp` variant

**Files:** `crates/omnimodemd/src/ids.rs`

- [ ] Add `RtlTcp { host: String, port: u16 }` to `DeviceId`.
- [ ] `to_canonical_string` → `rtltcp:<host>:<port>`.
- [ ] `parse`: `rtltcp` scheme splits `body` on the last `:` into host + port.
- [ ] Round-trip test + a `rtltcp:127.0.0.1:1234` parse test + an IPv6-ish host
      (host may contain `:` only if bracketed — keep it simple: split on the last
      `:`, so bare IPv6 is out of scope, documented).
- [ ] Commit: `daemon: DeviceId::RtlTcp variant for rtl_tcp SDR endpoints`.

### Task 2: `rtl_tcp` header parser

**Files:** `crates/omnimodemd/src/audio/rtlsdr.rs` (new), `audio/mod.rs` (`pub mod rtlsdr;`)

- [ ] `struct RtlHeader { tuner_type: u32, tuner_gain_count: u32 }`.
- [ ] `fn parse_header(buf: &[u8; 12]) -> Result<RtlHeader, AudioError>`: validate
      magic `b"RTL0"`, decode two u32 BE.
- [ ] `fn tuner_name(t: u32) -> &'static str` (R820T/E4000/… map; "unknown" fallback).
- [ ] Tests: valid header parses; bad magic errors; tuner-name map.
- [ ] Commit: `sdr: rtl_tcp 12-byte header parser`.

### Task 3: command encoder

**Files:** `audio/rtlsdr.rs`

- [ ] `enum RtlCmd { CenterFreq(u32), SampleRate(u32), GainMode(bool),
      TunerGain(i32/*tenths dB*/), FreqCorrection(i32/*ppm*/) }`.
- [ ] `fn encode(&self) -> [u8; 5]`: opcode (0x01..0x05) + u32 BE arg.
- [ ] Tests: each opcode + big-endian arg round-trips the expected bytes.
- [ ] Commit: `sdr: rtl_tcp 5-byte command encoder`.

### Task 4: `SdrControl` runtime cell

**Files:** `audio/rtlsdr.rs`

- [ ] `Arc`-of-atomics: `center_hz` (f64 bits/AtomicU64), `offset_hz` (f32/AtomicU32),
      `gain_auto` (AtomicBool), `gain_db` (f32), `squelch_db` (f32), `ppm` (AtomicI32),
      `demod_mode` (AtomicU8), `generation` (AtomicU64).
- [ ] `DemodMode` enum (Nbfm/Am/Wfm/Usb/Lsb) as `u8`; only Nbfm implemented.
- [ ] Setters bump `generation`; getters are relaxed loads. `Default` = NBFM,
      center 0, offset 0, gain auto, squelch disabled (`f32::NEG_INFINITY` sentinel),
      ppm 0.
- [ ] `effective_squelch()` → `PowerSquelch` (disabled when sentinel).
- [ ] Tests: default; set-visible-through-clone bumps generation; squelch sentinel.
- [ ] Commit: `sdr: SdrControl runtime control cell (Arc-of-atomics)`.

### Task 5: `RtlTcpBackend` — connect, header, IQ→audio loop, RF waterfall

**Files:** `audio/rtlsdr.rs`, `audio/backend.rs` (`attach_sdr_context` default method)

- [ ] `struct RtlTcpBackend { host, port, capture_rate, deviation_hz, control,
      telemetry: Option<Sender<TelemetryEvent>>, channel: ChannelId }`. `new(host,port)`.
- [ ] `attach_sdr_context(&mut self, channel, telemetry, control)` on the trait
      (default no-op); `RtlTcpBackend` stores them.
- [ ] `open_capture`: TCP connect, read+validate header, send initial commands
      (rate, freq-correction, gain mode+gain, center freq), spawn a capture thread:
      read u8 IQ (carry the odd trailing byte across reads) → `u8_iq_to_cplx` →
      `NbfmReceiver::push_iq` → scale to i16 → `AudioChunk` via `SyncSender`.
      Each block: check `control.generation()`; on change apply NCO `retune`,
      `set_squelch`, and hardware retune/ppm/gain commands on the write half.
      Also feed IQ to `ComplexStft`; per frame emit `SpectrumFrame{transmit:false,
      freq_start_hz: center-rate/2, …}` via `full_spectrum_dbfs`+`new_centered`+`render`.
- [ ] `open_playback` → `Err(AudioError::Unsupported)` (RX-only).
- [ ] `device_id()` → `DeviceId::RtlTcp{..}`.
- [ ] Unit test with a fake in-process `rtl_tcp` server: header + a short scripted
      IQ stream → capture yields non-empty audio at the channel rate; playback errors.
- [ ] Commit: `sdr: RtlTcpBackend AudioBackend (connect, IQ->audio, RF waterfall)`.

### Task 6: factory + configure_audio wiring

**Files:** `lib.rs` (`production_core`), `core/mod.rs` (`configure_audio`, `try_spawn_workers`, `LiveBindings`)

- [ ] `LiveBindings::sdr_controls: HashMap<ChannelId, SdrControl>`; evict on hotplug/rebind.
- [ ] `configure_audio` gains a `telemetry` param; synthesizes a capture-only
      descriptor for `DeviceId::RtlTcp`; when the RX device is RtlTcp, creates/clones
      the channel `SdrControl`, `attach_sdr_context`s it before `open_capture`.
- [ ] `try_spawn_workers`: SDR RX device → pass `SpectrumControl::default()` to the
      RX worker (audio-passband tap stays off).
- [ ] `production_core` factory: `match DeviceId::RtlTcp{host,port} =>
      Box::new(RtlTcpBackend::new(host,port))` before the cpal loop.
- [ ] Thread `telemetry` through the two `configure_audio` call sites.
- [ ] Existing core tests still green.
- [ ] Commit: `sdr: wire RtlTcpBackend into the core factory + control seam`.

### Task 7: integration test — fake `rtl_tcp` → APRS decode (closing gate)

**Files:** `crates/omnimodemd/tests/rtl_tcp_sdr.rs` (new)

- [ ] In-process fake `rtl_tcp` server: TCP listener writes the 12-byte header,
      drains client commands, then streams u8 IQ of an **FM-modulated AFSK1200
      APRS burst**: build AX.25 via `Afsk1200Mod` (48 kHz audio) → upsample ×5 to
      240 kHz → FM-modulate at a chosen NCO offset → u8 IQ.
- [ ] Construct `RtlTcpBackend`, set `SdrControl` (center, offset, squelch off),
      `open_capture(48_000)`, drain `AudioChunk`s to EOF, feed to
      `Afsk1200Ensemble` → assert the decoded AX.25 frame equals the transmitted
      one and `crc_ok`.
- [ ] Commit: `sdr: fake rtl_tcp server -> AFSK1200 APRS decode integration test`.

### Task 8: verification

- [ ] `cargo test` (workspace) green; `cargo clippy --all-targets -- -D warnings` clean.
- [ ] Paste output, open PR.

---

## Self-review vs scope
- `DeviceId::RtlTcp` + canonical/parse → Task 1. ✓
- `RtlTcpBackend` (connect/header/commands/IQ→audio, RX-only playback) → Tasks 2,3,5. ✓
- `SdrControl` Arc-of-atomics (tune/gain/demod/squelch/ppm) → Task 4. ✓
- RF waterfall telemetry-out from the capture thread → Task 5. ✓
- RX worker audio tap OFF for SDR channels → Task 6. ✓
- Factory branch in `production_core` → Task 6. ✓
- Fake-server AFSK1200/APRS integration test → Task 7. ✓
- No stubs; unit tests for header/command/u8→demod; integration test is the gate. ✓
</content>
