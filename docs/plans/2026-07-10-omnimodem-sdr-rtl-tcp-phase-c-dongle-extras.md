# RTL-SDR Phase C — dongle extras (bias-tee, direct-sampling, device registration)

**Date:** 2026-07-10
**Issue:** GRA-312
**Design source of truth:** `docs/design/2026-07-06-rtl-tcp-sdr-input-design.md` (Phase C row).

## Goal

Fill in the remaining `rtl_tcp` dongle controls that Phase A shipped as
accept-but-reject or hardcoded-unsupported:

1. **Bias-tee** — power an inline LNA/antenna over coax (`rtl_tcp` opcode `0x0e`).
2. **Direct sampling (HF)** — bypass the tuner to hear < ~24 MHz (opcode `0x09`).
3. **Config-file device registration** — let the daemon config list `rtl_tcp`
   endpoints so `ListDevices` surfaces them; ad-hoc binding still works.

ppm correction is already wired end-to-end (Phase A). **Out of scope — leave as-is.**

The gRPC/proto surface (`ConfigureSdrRequest.bias_tee/direct_sampling`,
`GetSdrCapsResponse.*_supported`) already ships from Phase A; this phase fills in
behavior only. No breaking proto change.

## Constraints

- `CARGO_TARGET_DIR=/tmp/omni-target CARGO_INCREMENTAL=0`. Never run `cargo fmt`.
- Keep edits localized to `RtlCmd`/caps/config paths (shares `rtlsdr.rs` with
  Phase B/D — minimize conflict surface).
- No stubs: a control closes only when it is (a) sent to the dongle, (b) reflected
  in `GetSdrCaps`, and (c) selectable via `ConfigureSdr`.
- TDD: byte-exact encoder tests + a fake-`rtl_tcp`-server test that asserts the
  bytes actually reach the socket.

## Wire protocol reference (`librtlsdr` `rtl_tcp.c`)

- `0x0e` set bias-tee: arg `0`/`1`.
- `0x09` set direct-sampling: arg `0` = off, `1` = I-branch, `2` = Q-branch.

The `ConfigureSdr.direct_sampling` proto field is a `bool`; `true` maps to
**Q-branch (2)** — the branch consumer HF-capable dongles (RTL-SDR Blog V3) wire
the HF input to — and `false` to off (0). The `RtlCmd` variant still carries the
full `u32` mode so the distinction is preserved at the wire layer.

## Steps (bite-sized, TDD)

### Step 1 — `RtlCmd::BiasTee` + `RtlCmd::DirectSampling` encoders
- Add `BiasTee(bool)` (0x0e) and `DirectSampling(u32)` (0x09) to the `RtlCmd` enum
  and its `encode()`.
- **Test:** byte-exact opcode + BE arg, mirroring the existing `-1 ppm` round-trip
  test: `BiasTee(true) == [0x0e,0,0,0,1]`, `BiasTee(false) == [0x0e,0,0,0,0]`,
  `DirectSampling(2) == [0x09,0,0,0,2]`.

### Step 2 — `SdrControl` fields + setters
- Add `bias_tee: AtomicBool` and `direct_sampling: AtomicU32` (mode) to
  `SdrControlInner`, defaults `false`/`0`.
- Add `bias_tee()`/`set_bias_tee(bool)` and `direct_sampling()`/
  `set_direct_sampling(u32)`; each `bump()`s the generation.
- **Test:** default is off; a set is visible through a clone and bumps generation.

### Step 3 — apply on connect + reapply
- `send_initial_commands` and `apply_hardware` send `BiasTee(control.bias_tee())`
  and `DirectSampling(control.direct_sampling())`.
- **Test (integration):** the recording fake server observes a `0x0e …1` and a
  `0x09 …2` frame after `set_bias_tee(true)` / `set_direct_sampling(2)` on a
  running capture (extends `spawn_recording_server` pattern).

### Step 4 — truthful caps
- `bias_tee_supported`: true for R820T/R828D-class tuners (types `5`/`6`); add a
  `bias_tee_supported(tuner_type)` helper.
- `direct_sampling_supported`: true for every RTL2832U dongle (ADC feature, not a
  tuner feature).
- `caps_from_header` reports both truthfully.
- **Test:** R820T caps report `bias_tee_supported && direct_sampling_supported`.

### Step 5 — wire `ConfigureSdr` in the core
- Remove the two Phase-C `Unimplemented` guards in `configure_sdr`.
- Call `control.set_bias_tee(bias_tee)` and
  `control.set_direct_sampling(if direct_sampling {2} else {0})`.
- When direct-sampling is on, `get_sdr_caps` widens the reported range down to HF
  (`freq_min_hz = 0`, `freq_max_hz = max(current, ~28.8 MHz Nyquist))`.
- **Test:** `configure_sdr` with `bias_tee=true`/`direct_sampling=true` succeeds and
  the control reflects it; `get_sdr_caps` shows the HF-widened range.

### Step 6 — config-file device registration
- New `config` module: a minimal, dependency-free daemon config file
  (`OMNIMODEM_CONFIG`, default `<runtime_dir>/omnimodem.conf`). Lines of the form
  `rtl_tcp <host>:<port> [label...]`, `#` comments, blank lines ignored. Parses to
  `Vec<DeviceDescriptor>` keyed by `rtltcp:<host>:<port>` (capture-only).
- Thread the parsed list through `core::spawn` → `run` → the `ListDevices` handler,
  which merges it with the live enumeration (registered entries deduped against
  enumerated by `DeviceId`).
- `main.rs`/`production_core` load the file; empty/missing file = no registered
  devices (normal).
- **Test:** config parse (valid lines, comments, bad lines skipped); `ListDevices`
  returns a registered `rtltcp:` device that the enumerator never produced; ad-hoc
  `ConfigureAudio` on an unregistered endpoint still binds (existing behavior).

### Step 7 — proto/doc comment refresh
- Update the `ConfigureSdrRequest`/`GetSdrCaps` proto comments (and the `rpc`
  comment) to drop the "Phase C returns UNIMPLEMENTED" note.
- Update the design doc / wiki if the config-file schema warrants it.

## Verification gate

- `CARGO_TARGET_DIR=/tmp/omni-target CARGO_INCREMENTAL=0 cargo test -p omnimodemd`
- `... cargo clippy --all-targets -- -D warnings` clean.
- Paste output into the PR. Commit as `chrissnell`, no AI attribution.
