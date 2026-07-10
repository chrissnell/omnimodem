# RTL-SDR (`rtl_tcp`) — Phase A, Plan 3: SDR control gRPC surface — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development
> (or executing-plans) to implement this plan task-by-task. Steps use checkbox
> (`- [ ]`) syntax for tracking.

**Goal:** Expose the SDR receive controls built in Plan 2 (`SdrControl` cell +
`RtlTcpBackend`) as a first-class, additive gRPC surface on `ModemControl`, so any
frontend — not just our TUI — can tune, set gain/ppm/demod-mode/squelch, and query
tuner capabilities at runtime, and so multiple/late clients stay in sync via an
`SdrState` event.

**Scope (new, additive-only in `omnimodem.v1`):** four RPCs + their messages, one
`DemodMode` enum, and one `SdrState` event added to the `Event` oneof. Existing
tags are untouched (VERSIONING.md additive rule).

- `SetSdrTune{channel, double freq_hz}` → `{actual_freq_hz, center_hz, offset_hz}`
  — absolute demod frequency; the daemon splits it into hardware center + NCO
  offset (the "gqrx model").
- `SetSdrGain{channel, bool auto, float gain_db}` → `{actual_gain_db}` — snaps a
  manual gain to the tuner's discrete table.
- `ConfigureSdr{channel, capture_rate, DemodMode demod_mode, squelch_db, ppm,
  bias_tee, direct_sampling}` → `{actual_capture_rate}`. The `DemodMode` enum ships
  complete; only `DEMOD_NBFM` is implemented in Phase A — AM/WFM/USB/LSB return
  `UNIMPLEMENTED`. `bias_tee`/`direct_sampling` are validated but deferred to Phase
  C: requesting either returns `UNIMPLEMENTED`; leaving them `false` is a no-op.
- `GetSdrCaps{channel}` → `{tuner, freq_min_hz, freq_max_hz, repeated sample_rates,
  repeated gains_db, bias_tee_supported, direct_sampling_supported}` — sourced from
  the dongle's `rtl_tcp` header (tuner type) plus per-tuner static tables, published
  by the backend into the shared control cell at connect.

**Wiring (mirror the `SetAudioGain` path exactly):** proto message + rpc → a
`Command` variant in `core/command.rs` carrying a `oneshot::Sender<Result<…,
CoreError>>` → an async handler in `grpc/service.rs` (build Command, send, await
reply, map errors) → a match arm in `core/mod.rs` that mutates the channel's
`SdrControl` cell and (for `GetSdrCaps`) reads the caps the backend published. On
each mutating RPC the core also broadcasts an `SdrState` telemetry event.

**Reference path (`SetAudioGain`):** `proto/omnimodem.proto`
(`SetAudioGainRequest`/rpc), `core/command.rs` (`Command::SetAudioGain`),
`grpc/service.rs` (`set_audio_gain`), `core/mod.rs` (dispatch arm). `SpectrumFrame`
event + `convert.rs::telemetry_event_to_proto` are the reference for `SdrState`.

**Depends on:** Plan 2 (merged, commit b17b913). The `SdrControl` cell already has
`center_hz`, `offset_hz`, gain (auto/db), `squelch_db`, `ppm`, `demod_mode`,
`generation`, and `effective_squelch`. This plan adds `capture_rate` and a `caps`
cell to `SdrControl` (the only Plan-2 struct additions), and makes the capture
thread honor a live `capture_rate` change.

**Build/test conventions:**
- `CARGO_TARGET_DIR=/tmp/omni-target CARGO_INCREMENTAL=0`. **Never run `cargo fmt`.**
- `protoc` present; `build.rs` runs tonic-build on `cargo build`.
- Daemon tests: `cargo test -p omnimodemd`. Round-trip gRPC tests use the generated
  client over an in-process UDS (`serve_uds_no_authz`), like `tests/unary.rs`.
- Full gate: `cargo test` + `cargo clippy --all-targets -- -D warnings`.

---

### Task 1: Extend `SdrControl` with `capture_rate` + a published caps cell

The tune split (`SetSdrTune`) needs the capture rate to know the band width, and
`GetSdrCaps`/`SetSdrGain` need the tuner caps + gain table. Both live in the shared
`SdrControl` so the async core reads what the capture thread learned at connect.

**Files:** `crates/omnimodemd/src/audio/rtlsdr.rs`.

- [ ] Add `TunerCaps { tuner: String, freq_min_hz, freq_max_hz, sample_rates:
  Vec<u32>, gains_db: Vec<f32>, bias_tee_supported: bool,
  direct_sampling_supported: bool }` (Clone, Debug).
- [ ] Static per-tuner tables: `tuner_freq_range(t) -> (f64, f64)`,
  `tuner_gains_db(t) -> Vec<f32>` (R820T/R828D 29-step table; E4000 table; a
  conservative default otherwise), and `supported_sample_rates() -> Vec<u32>` (the
  `rtl_tcp`/librtlsdr set: 250000, 1024000, 1536000, 1792000, 1920000, 2048000,
  2160000, 2400000, 2560000, 2880000, 3200000 plus our 240000 default).
- [ ] `caps_from_header(&RtlHeader) -> TunerCaps` composing those. bias-tee /
  direct-sampling `*_supported = false` in Phase A (Phase C wires them).
- [ ] Add to `SdrControlInner`: `capture_rate: AtomicU32` (default
  `DEFAULT_CAPTURE_RATE`) and `caps: Mutex<Option<TunerCaps>>` (default `None`).
  Add `SdrControl` accessors: `capture_rate()`, `set_capture_rate(u32)` (bumps
  generation), `caps() -> Option<TunerCaps>`, `set_caps(TunerCaps)`.
- [ ] Unit tests: gain table snapping helper (below), caps default `None`,
  `set_caps` visible through a clone, `capture_rate` default + set bumps generation.
- [ ] `snap_gain(table: &[f32], want: f32) -> f32`: nearest table entry (returns
  `want` if the table is empty). Unit-tested.

### Task 2: Publish caps + honor live `capture_rate` in the capture thread

**Files:** `crates/omnimodemd/src/audio/rtlsdr.rs`.

- [ ] In `open_capture`, after `parse_header`, `self.control.set_caps(
  caps_from_header(&hdr))` so the core can answer `GetSdrCaps`.
- [ ] Drive the initial `SampleRate`/build from `self.control.capture_rate()`
  (defaults to `DEFAULT_CAPTURE_RATE`, matching today's behavior) so `ConfigureSdr`
  can change it. Track `cur_rate` in the loop; on a generation change where
  `control.capture_rate() != cur_rate`, re-send `RtlCmd::SampleRate`, rebuild the
  `NbfmReceiver` + waterfall geometry at the new rate, and update `cur_rate`.
- [ ] Tests: existing Plan-2 capture/decup tests still pass (default rate
  unchanged); a new test asserts caps are published after connect (`control.caps()`
  is `Some` with the fake server's R820T tuner).

### Task 3: CoreError variants for the SDR surface

**Files:** `crates/omnimodemd/src/core/error.rs`, `crates/omnimodemd/src/grpc/convert.rs`.

- [ ] Add `CoreError::Unimplemented(String)` → `Status::unimplemented` and
  `CoreError::SdrRequired(ChannelId)` → `Status::failed_precondition` ("channel is
  not bound to an SDR").
- [ ] Extend `core_error_to_status` to map both.

### Task 4: Proto surface (additive)

**Files:** `proto/omnimodem.proto`.

- [ ] Four rpcs on `ModemControl`; four request/response message pairs; the
  `DemodMode` enum (exact numbering from the design doc). All new tags.
- [ ] Add `SdrState sdr_state = 15;` to the `Event` oneof and define
  `message SdrState { uint32 channel; double center_hz; double offset_hz; double
  freq_hz; bool gain_auto; float gain_db; DemodMode demod_mode; float squelch_db; }`.
- [ ] `cargo build -p omnimodemd` regenerates stubs. Confirm additive in the PR body.

### Task 5: `Command` variants + result structs

**Files:** `crates/omnimodemd/src/core/command.rs`.

- [ ] `SetSdrTune{channel, freq_hz, reply: oneshot<Result<SdrTuneOk, CoreError>>}`,
  `SetSdrGain{channel, auto, gain_db, reply: …<SdrGainOk…>}`,
  `ConfigureSdr{channel, capture_rate, demod_mode: u8, squelch_db, ppm, bias_tee,
  direct_sampling, reply: …<ConfigureSdrOk…>}`,
  `GetSdrCaps{channel, reply: …<SdrCapsOk…>}`.
- [ ] Result structs: `SdrTuneOk{actual_freq_hz, center_hz, offset_hz}`,
  `SdrGainOk{actual_gain_db}`, `ConfigureSdrOk{actual_capture_rate}`,
  `SdrCapsOk{tuner, freq_min_hz, freq_max_hz, sample_rates, gains_db,
  bias_tee_supported, direct_sampling_supported}`.

### Task 6: `TelemetryEvent::SdrState` + proto conversion

**Files:** `crates/omnimodemd/src/core/event.rs`, `crates/omnimodemd/src/grpc/convert.rs`.

- [ ] `TelemetryEvent::SdrState { channel, center_hz, offset_hz, freq_hz,
  gain_auto, gain_db, demod_mode: u8, squelch_db }` (LOSSY).
- [ ] `telemetry_event_to_proto` arm → `proto::event::Kind::SdrState`.

### Task 7: `plan_tune` split + core dispatch arms

**Files:** `crates/omnimodemd/src/audio/rtlsdr.rs` (`plan_tune`),
`crates/omnimodemd/src/core/mod.rs` (dispatch).

- [ ] `pub fn plan_tune(center_hz: f64, capture_rate: u32, target_hz: f64) -> (f64,
  f32)`: if `center_hz != 0` and `|target - center| <= MAX_OFFSET` (=
  `capture_rate * 0.4`), keep center and move the NCO offset; else re-center so the
  signal sits at a quarter-band offset (`capture_rate/4`, avoiding the center DC
  spike): `center = target - offset`. Returns `(center, offset)` with `center +
  offset == target`. Unit-tested (in-band move, re-center from cold, re-center on
  overshoot, `center + offset == target`).
- [ ] A `sdr_control_mut(live, supervisor, channel) -> Result<&SdrControl,
  CoreError>` helper: `SdrRequired` unless the channel is SDR-bound and has a
  control cell.
- [ ] Dispatch arms mutate the cell:
  - `SetSdrTune`: `plan_tune` → `set_center_hz` + `set_offset_hz`; reply the actual
    triple; emit `SdrState`.
  - `SetSdrGain`: snap manual gain against `caps().gains_db`; `set_gain`; reply
    `actual_gain_db` (0.0 when auto); emit `SdrState`.
  - `ConfigureSdr`: reject non-NBFM `demod_mode` and any `bias_tee`/`direct_sampling
    == true` with `Unimplemented`; otherwise apply `demod_mode`/`squelch_db`
    (sentinel `<= -200` disables)/`ppm`/`capture_rate` (0 = unchanged, else validate
    against `supported_sample_rates`); reply `actual_capture_rate`; emit `SdrState`.
  - `GetSdrCaps`: read `control.caps()` → `SdrCapsOk`; `SdrRequired` if absent.
- [ ] A private `emit_sdr_state(telemetry, channel, control)` reads the cell and
  broadcasts the event (used by the three mutating arms).

### Task 8: gRPC handlers

**Files:** `crates/omnimodemd/src/grpc/service.rs`.

- [ ] `set_sdr_tune`, `set_sdr_gain`, `configure_sdr`, `get_sdr_caps` — each mirrors
  `set_audio_gain`: validate cheap invariants in the handler (e.g. `freq_hz` finite
  and `> 0`), build the Command, `send_command`, await the oneshot, map with
  `core_error_to_status`, return the response. `configure_sdr` passes `demod_mode as
  i32` from the proto enum down as `u8`.

### Task 9: Tests — handler/core unit + gRPC round-trip

**Files:** `crates/omnimodemd/src/core/mod.rs` (`#[cfg(test)]`),
`crates/omnimodemd/tests/sdr_control.rs` (new integration).

- [ ] Core-level tests (drive `Command`s through a `TestCore`, mirroring the
  existing `SetAudioGain` core tests): tune split moves the NCO within band and
  re-centers on overshoot; gain snaps to the table; `ConfigureSdr` NBFM ok, AM →
  `Unimplemented`, `bias_tee=true` → `Unimplemented`; `GetSdrCaps` returns the
  published tuner.
- [ ] gRPC round-trip (`serve_uds_no_authz`): bind a channel to a fake
  `rtltcp:127.0.0.1:<port>` endpoint, then exercise all four RPCs over the generated
  client and assert the responses; subscribe first and assert an `SdrState` event
  arrives after a tune. Reuse the in-process fake `rtl_tcp` server pattern from
  `tests/rtl_tcp_sdr.rs`.

### Task 10: Docs + gate

**Files:** `docs/grpc-api.md`, `docs/wiki/grpc-edge.md`, this plan.

- [ ] Document the four RPCs + `SdrState` event + `DemodMode` in `docs/grpc-api.md`;
  add a wiki pointer in `docs/wiki/grpc-edge.md` so future agents find the SDR
  control seam.
- [ ] `cargo test` (workspace) + `cargo clippy --all-targets -- -D warnings`. Paste
  output in the result comment. Open a PR (commit as `chrissnell`, additive-only
  note in the body).

---

## Self-review against scope

- Four RPCs + messages + `DemodMode` enum + `SdrState` event — Tasks 4/5/6/7/8. ✓
- Additive only (new tags/messages/rpcs; `Event` gets tag 15) — Task 4. ✓
- Mirrors the `SetAudioGain` path end to end — Tasks 5/7/8. ✓
- No stubs on the NBFM path; AM/WFM/SSB + bias-tee/direct-sampling return
  `UNIMPLEMENTED` and that is tested — Tasks 7/9. ✓
- `GetSdrCaps` sourced from the dongle header + per-tuner tables via the Plan-2
  backend (published into the shared cell) — Tasks 1/2/7. ✓
- Tune split hidden from callers (gqrx center+NCO model) — Task 7. ✓
- `SdrState` broadcast for multi-client sync — Tasks 6/7. Full ModemState-snapshot
  integration for a truly cold late-joiner is deferred (documented): the event fires
  on every change, matching how gain/spectrum sync today.
