# Omnimodem wiki

Index for cross-system questions about this repo, written for developers (and LLM
agents) who need to find the right code fast. The wiki *navigates*; the code, the
design doc, the plan files, and the proto keep their existing roles.

## When to use this wiki vs. other docs

| If you want | Look at |
|---|---|
| Where pieces connect, what runs where, what to touch when changing X | This wiki |
| How to integrate a frontend — the gRPC contract, RPC by RPC | [`../grpc-api.md`](../grpc-api.md) and [`../../proto/omnimodem.proto`](../../proto/omnimodem.proto) |
| Feature overview / project pitch | [`../../README.md`](../../README.md) |
| Build and run the daemon + TUI | [`../running.md`](../running.md) |
| Why a subsystem was built that way (design rationale) | [`../design/2026-06-17-omnimodem-design.md`](../design/2026-06-17-omnimodem-design.md) and `../plans/*.md` |
| What a single function does | The code |

## Pages

- [`system-topology.md`](system-topology.md) — processes, transports, sockets, env knobs, persistence, ports, the async-edge/sync-core split.
- [`code-map.md`](code-map.md) — concern → file lookup, one table per component (daemon + DSP + registry).
- [`grpc-edge.md`](grpc-edge.md) — the control plane: command/event spine, backpressure, snapshot-on-subscribe, workers, leases, interlock.
- [`audio-devices-ptt.md`](audio-devices-ptt.md) — stable `DeviceId`, audio backends, resampling, fan-out, hotplug, the PTT drivers, KISS bridge, authz.
- [`dsp-building-blocks.md`](dsp-building-blocks.md) — the `omnimodem-dsp` crate: the soft-LLR contract, the pipeline stages, the mode framework, and how a mode is registered.
- [`mode-catalog.md`](mode-catalog.md) — every mode: what it is, its submodes, RX/TX, output type, and the picture sub-protocols.
- [`invariants.md`](invariants.md) — cross-cutting "if you change X, also touch Y" rules with reasons.
- [`glossary.md`](glossary.md) — domain terms as omnimodem uses them, with source pointers.
- [`tui-client.md`](tui-client.md) — the Go reference frontend: how it connects, its views, and which RPCs/events it uses.

## The shape of the system, in three sentences

The daemon (`omnimodemd`, Rust) is a hard split between an **async gRPC control
edge** and a **synchronous DSP/audio/PTT core**, bridged by an `mpsc` command queue
and two `tokio::broadcast` event rings (lossless frames, lossy telemetry). The DSP
itself is a pure, daemon-independent crate (`omnimodem-dsp`) built from composable,
individually-tested building blocks whose spine is a soft-information (LLR) contract
between the detector/demapper and the FEC decoder; a mode is an *assembly* of those
blocks registered in one module. Everything is driven over one versioned gRPC
service, `omnimodem.v1.ModemControl`, and the Go TUI under `clients/` is a complete
worked client.

## Maintenance

A stale wiki is worse than none, because it gets trusted. If you grep for something
this wiki should have answered, add it. If the wiki disagrees with the code, fix the
wiki in the same change. When you add or change a mode, an RPC, a device/PTT driver,
or the command/event spine, update the relevant page (`mode-catalog.md`,
`grpc-api.md`, `code-map.md`, or `invariants.md`) in the same PR.
