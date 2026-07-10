# Invariants

Cross-cutting "if you change X, also touch Y" rules and safety properties. Each
entry: the rule, a one-line *why*, and the source of truth.

### 1. No DSP on the async edge; no async on the sample path

*Why:* a real-time modem must not run the sample loop under an async scheduler
(underruns drop frames); and the async edge must not block on DSP. The split is the
whole architecture.

Source: [`../../crates/omnimodemd/src/core/mod.rs`](../../crates/omnimodemd/src/core/mod.rs)
(command loop + worker threads), `grpc/service.rs` (handlers only translate
`Command`s). See [`grpc-edge.md`](grpc-edge.md).

### 2. Decoded RX frames are lossless; telemetry is lossy

*Why:* a decoded frame is irreplaceable, so losing one is a correctness bug; a stale
level/metric is harmless. `tokio::broadcast` drops on lag, so the two must be
separate rings with separate overflow policy.

Source: `core/event.rs` (`FrameEvent` vs `TelemetryEvent`), `core/mod.rs:52-53`
(`FRAME_RING = 1024`, `TELEMETRY_RING = 256`),
`grpc/subscribe.rs` (frames disconnect a lagging subscriber; telemetry skips). A new
event variant must be filed into the correct class.

### 3. `SubscribeEvents` always yields the snapshot first

*Why:* a reconnecting client must never be stale. Subscribe to the rings **before**
snapshotting, so nothing emitted in between is lost; emit the snapshot as message #1.

Source: `grpc/subscribe.rs::subscribe`. Ordering is at-least-once — clients tolerate
a duplicate between snapshot and the first live event.

### 4. `proto/omnimodem.proto` is additive-only within `omnimodem.v1`

*Why:* third-party frontends are the whole point; a renumbered/removed field breaks
every generated client silently.

Source: [`../../proto/omnimodem.proto`](../../proto/omnimodem.proto),
[`../../proto/VERSIONING.md`](../../proto/VERSIONING.md). Any PR touching the proto
must confirm the change is additive (new messages/fields/RPCs/enum values only; tags
never reused), or that it opens a new major package. Both the daemon
(`crates/omnimodemd/build.rs`) and the TUI (`clients/omnimodem-tui/gen.sh`,
`make proto`) regenerate from this one file.

### 5. Config keys on the stable `DeviceId`, never a `/dev` path

*Why:* the whole point of `DeviceId` is that a channel binding survives renames and
hotplug. Persisting a volatile path would defeat it.

Source: [`../../crates/omnimodemd/src/persist/mod.rs`](../../crates/omnimodemd/src/persist/mod.rs)
(stores `DeviceId::to_canonical_string()`), [`../../crates/omnimodemd/src/ids.rs`](../../crates/omnimodemd/src/ids.rs).
Persistence writes run on the core thread, off the DSP hot path, so a disk hiccup
can't cause an audio underrun.

### 6. Audio capture is capped at 48 kHz; resampling is additive

*Why:* ALSA `plughw` advertises synthetic rates the codec can't honor, silently
desyncing bit timing and failing FCS on every frame. The ceiling avoids the trap;
resampling bridges to the mode's native rate *after* the capped capture, it does not
replace the defensive rate/format selection.

Source: [`../../crates/omnimodemd/src/audio/mod.rs`](../../crates/omnimodemd/src/audio/mod.rs)
(`MAX_SAMPLE_RATE`), `audio/alsa.rs`, `audio/resample.rs`.

### 7. Every PTT driver unkeys on `Drop`

*Why:* a stuck transmitter is a licensing/safety hazard. Releasing PTT in `Drop` is
the last-resort guarantee even on panic/teardown/hotplug eviction.

Source: `impl Drop` in `ptt/serial.rs`, `ptt/cm108.rs`, `ptt/gpio.rs`, and the
`MockPtt` in `ptt/none.rs`.

### 8. RX is muted on a rig while that rig is keyed (interlock)

*Why:* otherwise the receiver decodes our own transmission / feedback. On the
per-channel-thread model this must be explicit (Graywolf got it implicitly from a
single thread).

Source: [`../../crates/omnimodemd/src/ptt/interlock.rs`](../../crates/omnimodemd/src/ptt/interlock.rs)
(`RxTxInterlock`, a nesting-safe counter so two channels keying one rig nest
correctly); the RX worker checks it in `core/rx_worker.rs`.

### 9. TX contention is per-rig, not per-channel

*Why:* two channels can share one physical radio. Independent radios must not
serialize (per-channel TX workers), but two channels on one rig must (shared PTT
registry + lease).

Source: `ptt/lease.rs` (`TxLeaseRegistry`), `ptt/registry.rs` (`PortRegistry`),
`core/tx_worker.rs`. `AcquireTxLease`/`ReleaseTxLease` grant per-rig exclusivity;
`held_by` names the current holder.

### 10. PTT keying times off the DAC watermark, and aborts on cancel

*Why:* sleeping for the airtime is wrong under jitter; the drain watermark is the
truth. And a mode change / cancel must release PTT promptly rather than after a full
tail.

Source: [`../../crates/omnimodemd/src/ptt/sequence.rs`](../../crates/omnimodemd/src/ptt/sequence.rs)
(`drive_tx_cycle`): `tx_delay` lead-in → submit audio → wait on drained-sample
watermark → `tx_tail` hold → unkey; cancel checks throughout. `tx_delay_ms`/
`tx_tail_ms` are per-channel and apply to every mode on that channel.

### 11. The `omnimodem-dsp` crate is pure and daemon-independent

*Why:* it must be testable in isolation (KAT vectors, seeded BER sweeps) and reusable
by other clients/tools. It must not reach into the daemon.

Source: [`../../crates/dsp/src/lib.rs`](../../crates/dsp/src/lib.rs) doc. Randomness is
banned in library code — `testutil.rs` provides a seeded RNG so BER/corpus runs are
bit-reproducible.

### 12. The soft-LLR sign convention is the detector↔FEC boundary

*Why:* every FEC decoder assumes `Llr = ln(P(0)/P(1))`, positive ⇒ bit 0. Mixing
conventions across the boundary silently corrupts decoding.

Source: [`../../crates/dsp/src/types.rs`](../../crates/dsp/src/types.rs) (`Llr`,
`SoftBits`), consumed uniformly across `fec/`.

### 13. Adding a mode touches the registry, not five `match` sites

*Why:* the framework exists specifically to make a mode an assembly job. A mode that
special-cases itself across the daemon defeats it.

Source: `crates/dsp/src/modes/<mode>.rs` (implementation) + one arm each in
[`../../crates/omnimodemd/src/mode/mod.rs`](../../crates/omnimodemd/src/mode/mod.rs)
(`ModeConfig` + `parse`) and `mode/registry.rs` (`demod_kind` / `build_modulator`),
plus a `ModeParams` variant in the proto if parametric. Update
[`mode-catalog.md`](mode-catalog.md) in the same change.

### 14. KISS listeners are for packet modes only

*Why:* the KISS bridge translates AX.25 packet frames; it has no meaning for
keyboard/weak-signal modes.

Source: [`../../crates/omnimodemd/src/kiss/`](../../crates/omnimodemd/src/kiss/)
(`ConfigureKissListener` requires an AFSK-1200 packet channel).

### 15. Authorization is enforced even on the local UDS

*Why:* opening the control socket is the ability to transmit under the operator's
license. Loopback TCP would expose every local user; the UDS enforces
`SO_PEERCRED`, and routable binds require mTLS and fail closed without material.

Source: [`../../crates/omnimodemd/src/authz/`](../../crates/omnimodemd/src/authz/)
(`uds.rs`, `tls.rs`, `mod.rs::validate_transport`).
