# System topology

What runs where, what talks to what, what persists. The authoritative design
rationale is [`../design/2026-06-17-omnimodem-design.md`](../design/2026-06-17-omnimodem-design.md);
build/run steps are in [`../running.md`](../running.md).

## Processes

| Process | Language | Source | Notes |
|---|---|---|---|
| `omnimodem` | Rust | [`../../crates/omnimodem/src/main.rs`](../../crates/omnimodem/src/main.rs) | The modem daemon. Owns audio, DSP, PTT; serves the gRPC control plane. Binary crate `omnimodem`. |
| `omnimodem-tui` | Go | [`../../clients/omnimodem-tui/cmd/omnimodem-tui/main.go`](../../clients/omnimodem-tui/cmd/omnimodem-tui/main.go) | Reference terminal client. Talks gRPC to the daemon. Not required by the daemon. |
| `omnimodem-dsp` | Rust (lib) | [`../../crates/dsp/`](../../crates/dsp/) | Pure DSP/FEC/framing library + mode implementations. No process of its own; linked into `omnimodem`. |
| `wavtool` | Rust | [`../../crates/wavtool/src/main.rs`](../../crates/wavtool/src/main.rs) | Small WAV helper for tooling/tests. |

## The async-edge / sync-core split

A hard line runs through the daemon (see [`grpc-edge.md`](grpc-edge.md)):

- **Async control edge** (tonic + tokio): the gRPC service and subscription
  streams. Validates requests, translates them into `Command`s, fans events out to
  subscribers. **No DSP here.**
- **Synchronous core** (plain `std::thread`): the `Supervisor`, the live per-channel
  RX/TX worker threads, audio, PTT. **No async on the sample path.**

Bridge, created in [`../../crates/omnimodem/src/core/mod.rs`](../../crates/omnimodem/src/core/mod.rs) (`spawn`):

| Item | Value | Source |
|---|---|---|
| Commands (edge → core) | bounded `std::sync::mpsc::sync_channel` | `core/mod.rs` (`COMMAND_QUEUE_DEPTH`) |
| Decoded frames (core → edge) | `tokio::broadcast`, ring `FRAME_RING = 1024`, **lossless** | `core/mod.rs:52` |
| Telemetry (core → edge) | `tokio::broadcast`, ring `TELEMETRY_RING = 256`, **lossy** | `core/mod.rs:53` |

## Control-plane transport

| Item | Value | Source |
|---|---|---|
| Service | `omnimodem.v1.ModemControl` | [`../../proto/omnimodem.proto`](../../proto/omnimodem.proto) |
| Default transport | Unix-domain socket | [`../../crates/omnimodem/src/main.rs`](../../crates/omnimodem/src/main.rs), [`../../crates/omnimodem/src/authz/uds.rs`](../../crates/omnimodem/src/authz/uds.rs) |
| Default socket path | `${OMNIMODEM_RUNTIME_DIR}/omnimodem.sock` → `<tempdir>/omnimodem/omnimodem.sock` (Linux `/tmp/omnimodem/omnimodem.sock`) | `main.rs` |
| UDS authz | socket-file mode hardened + `SO_PEERCRED` peer-uid == daemon uid | [`../../crates/omnimodem/src/authz/uds.rs`](../../crates/omnimodem/src/authz/uds.rs) |
| Routable transport | mTLS TCP when `OMNIMODEM_ROUTABLE_ADDR` set; **fails closed** without TLS material | [`../../crates/omnimodem/src/authz/tls.rs`](../../crates/omnimodem/src/authz/tls.rs), `authz/mod.rs` |
| Prometheus metrics | optional exporter on `OMNIMODEM_PROMETHEUS_ADDR` | [`../../crates/omnimodem/src/metrics/prometheus.rs`](../../crates/omnimodem/src/metrics/prometheus.rs) |
| KISS-over-TCP (per packet channel) | started on demand by `ConfigureKissListener`, binds `host:port` | [`../../crates/omnimodem/src/kiss/listener.rs`](../../crates/omnimodem/src/kiss/listener.rs) |

## Environment knobs (daemon)

| Var | Effect | Default |
|---|---|---|
| `OMNIMODEM_RUNTIME_DIR` | Directory holding the socket + SQLite state DB | `<tempdir>/omnimodem` |
| `OMNIMODEM_ROUTABLE_ADDR` | Bind a routable mTLS TCP endpoint instead of the UDS | unset (UDS) |
| `OMNIMODEM_PROMETHEUS_ADDR` | Bind the Prometheus exporter | unset (off) |
| `RUST_LOG` | Log level / filter (`tracing-subscriber` env filter) | `info` |

TUI knobs: `OMNIMODEM_RUNTIME_DIR` (to find the default socket), `--addr` (explicit
UDS path or `host:port`), and its own identity config (see [`tui-client.md`](tui-client.md)).

## Persistence

| Item | Value | Source |
|---|---|---|
| Store | SQLite | [`../../crates/omnimodem/src/persist/mod.rs`](../../crates/omnimodem/src/persist/mod.rs) |
| Path | `${OMNIMODEM_RUNTIME_DIR}/omnimodem.sqlite` | [`../../crates/omnimodem/src/main.rs`](../../crates/omnimodem/src/main.rs) |
| Keyed on | the stable **`DeviceId`** (canonical string), never the volatile `/dev` path | `persist/mod.rs`, [`../../crates/omnimodem/src/ids.rs`](../../crates/omnimodem/src/ids.rs) |
| What persists | channel config: name, mode string, RX/TX/PTT device ids, sample rates, fanout, PTT method + timing, RSID flags | `persist/mod.rs`, [`../../crates/omnimodem/src/supervisor/channel.rs`](../../crates/omnimodem/src/supervisor/channel.rs) |
| Write path | on the core thread / off the DSP hot path — a disk hiccup can't cause an audio underrun | `persist/mod.rs` |

On startup the core restores persisted channels and re-establishes live audio/PTT
bindings for channels bound to present devices (`restore_live_bindings` in
`core/mod.rs`), so RX/TX resume without operator re-config. Channels whose device is
absent stay config-only until reconfigured.

## Hardware surface

- **Audio**: cpal backends (ALSA on Linux) plus file/stdin backends for
  deterministic test replay. RX capture is clamped to a 48 kHz ceiling (ALSA
  `plughw` trap avoidance); resampling bridges any source rate to the mode's native
  rate. See [`audio-devices-ptt.md`](audio-devices-ptt.md).
- **PTT**: serial RTS/DTR, CM108 HID GPIO, and Linux gpiochip drivers, plus
  None/VOX. Drivers unkey on `Drop`. See [`audio-devices-ptt.md`](audio-devices-ptt.md).
- **Device identity**: a stable cross-platform `DeviceId` (USB VID:PID:serial, ALSA
  card name, USB topology, `/dev/serial/by-id`, or a placeholder for virtual
  backends) survives renames and hotplug.
