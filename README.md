# omnimodem

A gRPC-driven, building-block-based multi-mode software modem for amateur radio.

omnimodem runs many amateur-radio modes simultaneously from a single binary —
each bound to its own audio interface and PTT — and is operated **entirely over a
stable, versioned gRPC API** so developers can build their own frontends in any
language. It generalizes best-of-class reception techniques into mode-agnostic
DSP/FEC building blocks, so adding a new mode is an assembly job rather than a
from-scratch DSP project.

## Why omnimodem

- **Easy to integrate with.** One `ModemControl` gRPC service, versioned
  `omnimodem.v1`, additive-only within a major. Codegen a client in any language;
  no bespoke framing to reimplement. See [`docs/grpc-api.md`](docs/grpc-api.md).
- **Many modes, concurrently.** Packet (AX.25 1200 AFSK), the WSJT-X weak-signal
  family (FT8, FT4, JT65, JT9, WSPR, FST4, MSK144, JS8), the fldigi keyboard modes
  (PSK31/63/125…, RTTY, Olivia, Contestia, MFSK, DominoEX, THOR, IFKP, FSQ, Throb,
  MT63, CW), and the image/facsimile modes (Hell, SSTV, NAVTEX, WEFAX) plus in-band
  picture sub-protocols. See [`docs/wiki/mode-catalog.md`](docs/wiki/mode-catalog.md).
- **Rock-solid audio & PTT.** Stable cross-platform device identity that survives
  renames and hotplug, structured errors, hotplug eviction/reopen, RX/TX interlock,
  and unkey-on-Drop safety.
- **Provable correctness.** Every DSP/FEC building block is individually testable
  and gated by known-answer vectors; modes are validated by cross-decode interop
  against their reference implementations.

## Architecture in one paragraph

A hard line separates an **async control edge** (tonic + tokio; gRPC handlers only)
from a **synchronous DSP/audio/PTT core** (plain `std::thread`; no async on the
sample path). Commands flow edge→core over an `mpsc`; events flow core→edge over a
`tokio::broadcast` fanned out to subscriber streams, with a lossless class for
decoded RX frames and a lossy class for telemetry. A **Supervisor** owns the live
channels, the device cache, the SQLite config store, and the shared PTT registry.
The DSP lives in a pure, daemon-independent crate (`omnimodem-dsp`) whose spine is
a soft-information (LLR) contract between the detector/demapper and the FEC decoder.
Full detail: [`docs/design/2026-06-17-omnimodem-design.md`](docs/design/2026-06-17-omnimodem-design.md)
and the [wiki](docs/wiki/README.md).

## Repository layout

| Path | What |
|---|---|
| `crates/omnimodem/` | The daemon: gRPC edge, sync core, audio, devices, PTT, KISS, persistence, metrics, mode registry. |
| `crates/dsp/` | `omnimodem-dsp` — pure DSP/FEC/framing building blocks and the mode implementations. |
| `crates/wavtool/` | Small WAV helper used by tests/tooling. |
| `clients/omnimodem-tui/` | Reference terminal frontend (Go), talks to the daemon over gRPC. |
| `proto/omnimodem.proto` | The single gRPC contract. `proto/VERSIONING.md` is the stability policy. |
| `docs/grpc-api.md` | Integrator-facing gRPC protocol reference. |
| `docs/wiki/` | LLM/developer wiki: where things connect, code map, invariants, glossary. |
| `docs/running.md` | Build + run the daemon and the TUI. |

## Building

Requires a Rust toolchain and the Protocol Buffers compiler, `protoc`, which the
gRPC codegen (`tonic-build`) invokes at build time. On Linux the audio backend
links ALSA (`libasound2-dev` + `pkg-config`). The Go TUI needs Go 1.26+.

```sh
# Debian/Ubuntu: apt install -y protobuf-compiler libasound2-dev pkg-config
# macOS:         brew install protobuf
make            # builds both → target/release/omnimodem and bin/omnimodem-tui
make modem      # just the daemon
make tui        # just the TUI
make test       # Rust workspace + Go TUI
```

If `protoc` is not on `PATH`, point the build at it: `PROTOC=/path/to/protoc cargo build`.

## Running

Start the daemon, then a client. The daemon logs the UDS it binds (default
`/tmp/omnimodem/omnimodem.sock`); the reference TUI auto-connects to it.

```sh
make run-modem   # terminal 1 — the daemon
make run-tui     # terminal 2 — the reference TUI
```

Daemon environment knobs: `OMNIMODEM_RUNTIME_DIR` (socket + state DB location),
`OMNIMODEM_ROUTABLE_ADDR` (bind a routable mTLS TCP endpoint instead of the UDS),
`OMNIMODEM_PROMETHEUS_ADDR` (metrics exporter), `RUST_LOG` (log level). Full
setup and custom-socket instructions: [`docs/running.md`](docs/running.md).

## Building a client

The API is one service, `omnimodem.v1.ModemControl`. Configure a channel, subscribe
to the event stream (you always get a state snapshot first), and transmit:

```
ConfigureChannel { channel: 0, mode: "ft8" }
SubscribeEvents {}                     // → Event.snapshot, then live RxFrame/telemetry
Transmit { channel: 0, payload: ... }  // → transmit_id; TransmitStarted/Complete follow
```

Start with [`docs/grpc-api.md`](docs/grpc-api.md) for the RPC-by-RPC reference,
event classes, versioning, and authorization. The Go reference client under
`clients/omnimodem-tui/` is a complete worked example.

## Design & documentation

- [`docs/grpc-api.md`](docs/grpc-api.md) — gRPC protocol reference (the integration surface).
- [`docs/wiki/`](docs/wiki/README.md) — where the pieces connect, the code map, invariants, glossary, mode catalog, DSP building blocks, and the TUI client.
- [`docs/design/2026-06-17-omnimodem-design.md`](docs/design/2026-06-17-omnimodem-design.md) — the full design and rationale.
- [`docs/running.md`](docs/running.md) — build and run.

## License

MIT. See `Cargo.toml`.
