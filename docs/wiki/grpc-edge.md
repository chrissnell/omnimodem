# The control plane: gRPC edge ⇄ sync core

The daemon's spine. This page is about *how the daemon works internally*; for the
client-facing contract see [`../grpc-api.md`](../grpc-api.md).

## The hard split

```
   gRPC client
      │  (UDS / mTLS TCP)
┌─────▼──────────────── async control edge (tonic + tokio) ───────────────┐
│  ControlService (grpc/service.rs)   subscribe stream (grpc/subscribe.rs) │
│        │ Command (+ oneshot reply)            ▲ FrameEvent / TelemetryEvent
└────────┼──────────────────────────────────────┼─────────────────────────┘
         │ mpsc::sync_channel                    │ tokio::broadcast (x2 rings)
┌────────▼──────────────────────────────────────┼──── synchronous core (std::thread) ┐
│  core/mod.rs command loop → Supervisor + LiveBindings                              │
│     ├─ RxWorker threads (per channel): capture → resample → demod → FrameEvent     │
│     ├─ TxWorker threads (per channel): queue → modulate → play + key PTT           │
│     ├─ PortRegistry / RxTxInterlock / TxLeaseRegistry                              │
│     └─ DeviceCache + HotplugWatcher, SQLite Store                                  │
└───────────────────────────────────────────────────────────────────────────────────┘
```

- **Edge** does only validation + translation. `grpc/service.rs` maps each RPC to a
  `Command` (with a `oneshot` reply channel), sends it over the bounded command
  queue, and awaits the reply. No DSP, no blocking locks held across the core.
- **Core** is one command loop (`core/mod.rs::spawn`) owning the `Supervisor` and
  the per-channel `LiveBindings` (capture/playback handles, workers, PTT drivers,
  metrics, gains, spectra). All demod/mod happens on worker `std::thread`s spawned
  from here — never on the async edge.

Why: preserve a real-time audio path with no async scheduler in the sample loop,
while still getting polyglot gRPC codegen and standard streaming for clients. This
is a deliberate carry-over from Graywolf's "never async-ify the hot path."

## Commands (edge → core)

`core/command.rs::Command` — one variant per RPC action, each carrying operands plus
a `oneshot::Sender<Result<T, CoreError>>`. The core answers synchronously; the edge
turns `CoreError` into a `tonic::Status` via `grpc/convert.rs::core_error_to_status`.
The queue is bounded (`COMMAND_QUEUE_DEPTH`); a full queue backpressures the handler
(returns unavailable) rather than growing unbounded.

## Events (core → edge): two classes

`core/event.rs` splits events into two `tokio::broadcast` rings created in
`core/mod.rs::spawn`:

| Class | Type | Ring | Overflow policy |
|---|---|---|---|
| **Lossless** | `FrameEvent` (decoded `RxFrame`) | `FRAME_RING = 1024` | A subscriber that lags is **disconnected** (resource-exhausted), never silently dropped. |
| **Lossy** | `TelemetryEvent` (levels, status, metrics, PTT, spectrum, clock, device, RSID) | `TELEMETRY_RING = 256` | A subscriber that lags **skips** intermediate values and continues. |

A decoded frame is expensive and irreplaceable, so losing one is a correctness bug;
a stale audio-level reading is harmless. Clients must treat `RxFrame` as a reliable
log and telemetry as a gauge.

## Snapshot-on-subscribe

`grpc/subscribe.rs::subscribe` guarantees a reconnecting client is never stale:

1. Subscribe to **both** broadcast rings first (so nothing emitted after this point
   is missed).
2. Then request a state snapshot from the core.
3. Yield the snapshot as the **first** stream message (`Event.snapshot`).
4. Merge the two live rings into the output stream.

Ordering is at-least-once: a change applied between steps 1 and 2 can appear in both
the snapshot and a follow-up event. Clients treat the snapshot as authoritative and
tolerate a duplicate.

## Per-channel workers

Spawned/torn down by the core as channels are (re)configured (`try_spawn_workers`,
and dropped on re-bind or device departure in `core/mod.rs`).

- **RxWorker** (`core/rx_worker.rs`): owns the capture handle, resamples to the
  mode's native rate, runs the demod, and emits `FrameEvent`s on the lossless ring.
  It is **muted by the interlock while the rig is keyed** (so we don't decode our
  own transmission), updates `SharedMetrics`, taps the waterfall when spectrum is
  enabled, and runs the RSID detector when `rsid_rx` is set. Two shapes:
  `spawn_streaming` (`feed`) and `spawn_windowed` (`decode_window` per slot).
- **TxWorker** (`core/tx_worker.rs`): a cooperative queue serializing frames onto
  the channel's on-air timeline. A `TxJob` is either a frame to modulate or
  pre-built audio (picture sends). It resamples, keys the rig through the keying
  sequence, and honors the TX lease. Per-channel (independent radios don't
  serialize), but two channels on one physical rig serialize via the shared PTT
  registry.

## Runtime updates without respawn

Two controls take effect on the running RX worker without tearing it down:

- **Audio gain** (`core/gain.rs::AudioGain`): RX/TX linear multipliers stored as
  `AtomicU32` bits; the worker reads one relaxed load per chunk. `SetAudioGain` is
  cheap and lock-free.
- **Spectrum** (`core/spectrum.rs::SpectrumControl`): a generation counter the
  worker polls once per chunk; `ConfigureSpectrum` bumps it and the worker
  reconciles (build/drop the FFT tap) on the next chunk. Off by default, so the FFT
  costs nothing when unwatched.

## TX arbitration: interlock vs lease

Two distinct per-rig mechanisms (`ptt/interlock.rs`, `ptt/lease.rs`):

- **Interlock** is a nesting-safe counter that mutes RX on a rig while *any* channel
  keys it — always on, prevents self-decode/feedback.
- **Lease** is optional exclusivity: a channel holding a rig's lease is the only one
  allowed to key it. `AcquireTxLease`/`ReleaseTxLease`. Without a lease the
  cooperative queue still serializes TX correctly; the lease is for sessions that
  can't tolerate interleaving at all.

## Hotplug

The core polls the `HotplugWatcher` on an idle tick. On departure it emits a
`DeviceDeparted` telemetry event, evicts the PTT driver from the registry, drops the
audio handles/workers/metrics/spectrum for affected channels, and releases any TX
lease held on that rig. The persisted channel config remains, so reconfiguring (or a
re-arrival + restore) brings the channel back live. See
[`audio-devices-ptt.md`](audio-devices-ptt.md).

## Metrics

`metrics/mod.rs::ChannelMetrics` is a per-channel accumulator (good/bad frames, SNR,
dBFS, AFC offset, DCD, PTT state, over/underruns, clip count, last decoder). It is
served by `GetMetrics`, pushed as a lossy `channel_metrics` event, and — when
`OMNIMODEM_PROMETHEUS_ADDR` is set — exported by `metrics/prometheus.rs` (which pulls
a snapshot from the core via a `Command::GetMetrics`).
