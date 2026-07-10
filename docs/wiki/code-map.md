# Code map

Where to look for a given concern. One section per major component, table per
section. For *what* a mode does in operator terms, see
[`mode-catalog.md`](mode-catalog.md); for the DSP framework, see
[`dsp-building-blocks.md`](dsp-building-blocks.md). This page only routes you to
source.

## Daemon: control edge (`crates/omnimodem/src/`)

Async tonic/tokio surface. See [`grpc-edge.md`](grpc-edge.md) for how it works.

| Concern | File | Key symbol |
|---|---|---|
| Binary entrypoint (wiring, env, transport select, Prometheus) | `main.rs` | `main` |
| Library surface + production wiring | `lib.rs` | `production_core`, `serve_uds_*` |
| gRPC service impl (all unary handlers) | `grpc/service.rs` | `ControlService`, `effective_mode` |
| gRPC module re-exports | `grpc/mod.rs` | — |
| `SubscribeEvents` stream: snapshot-first + dual-class merge | `grpc/subscribe.rs` | `subscribe` |
| Proto ⇄ core conversions | `grpc/convert.rs` | `snapshot_to_proto`, `*_event_to_proto`, `core_error_to_status` |
| Generated proto types (re-export of tonic-build output) | `proto.rs` | — |

## Daemon: sync core (`crates/omnimodem/src/core/`)

| Concern | File | Key symbol |
|---|---|---|
| Core thread spawn + command loop; broadcast rings; live-binding maps; hotplug poll | `core/mod.rs` | `spawn`, `CoreHandle`, `FRAME_RING`, `TELEMETRY_RING` |
| Command enum (edge → core) + reply channels | `core/command.rs` | `Command` |
| Event types + lossless/lossy classes | `core/event.rs` | `FrameEvent`, `TelemetryEvent` |
| Core error → gRPC status | `core/error.rs` | `CoreError` |
| RX worker (streaming + windowed demod loops; interlock mute; spectrum tap; RSID) | `core/rx_worker.rs` | `RxWorker` (`spawn_streaming`, `spawn_windowed`) |
| TX worker (cooperative queue; modulate or play pre-built audio; keying) | `core/tx_worker.rs` | `TxWorker`, `TxJob` |
| Runtime waterfall control (generation-based, no worker respawn) | `core/spectrum.rs` | `SpectrumControl` |
| Runtime audio gain (lock-free `AtomicU32`, no respawn) | `core/gain.rs` | `AudioGain` |
| Host clock-offset metric source | `core/clock.rs` | `ClockSource`, `SlotClock` |

## Daemon: supervisor (`crates/omnimodem/src/supervisor/`)

| Concern | File | Key symbol |
|---|---|---|
| State owner: channels, device cache, PTT registry, interlock, store | `supervisor/mod.rs` | `Supervisor` |
| Persisted channel config + runtime state | `supervisor/channel.rs` | `ChannelConfig`, `ChannelState` |

## Daemon: mode registry (`crates/omnimodem/src/mode/`)

The one place a mode string/params becomes a boxed demod/mod. Adding a mode touches
here + the DSP crate, not five `match` sites.

| Concern | File | Key symbol |
|---|---|---|
| Parametric per-mode config enum + `parse(mode_string)` | `mode/mod.rs` | `ModeConfig`, `ModeConfig::parse` |
| Registry: mode → `Box<dyn Demodulator/BlockDemodulator>` / modulator / native rate | `mode/registry.rs` | `demod_kind`, `DemodKind`, `build_modulator` |
| Image transmit assembly (header + pixel-FSK per picture mode) | `mode/picture_tx.rs` | `PictureSend`, `build` |

Typed `ModeParams` (proto) → mode string happens in `grpc/service.rs::effective_mode`;
that string is then parsed by `ModeConfig::parse`.

## Daemon: audio (`crates/omnimodem/src/audio/`)

| Concern | File | Key symbol |
|---|---|---|
| `AudioBackend` trait + capture/playback handles + null backend | `audio/backend.rs` | `AudioBackend`, `CaptureHandle`, `PlaybackHandle` |
| cpal hardware backend (stream rebuild + backoff) | `audio/cpal_backend.rs` | `enumerate_default_host` |
| `rtl_tcp` SDR backend (IQ→audio, tune/gain control, RF waterfall, reconnect + drop-oldest overrun) | `audio/rtlsdr.rs` | `RtlTcpBackend`, `SdrControl`, `connect_and_handshake`, `deliver_audio` |
| ALSA rate/format hardening (48 kHz ceiling, `plughw` trap) | `audio/alsa.rs` | rate/format selection helpers |
| Module constants + rate ceiling | `audio/mod.rs` | `MAX_SAMPLE_RATE` |
| File backend (deterministic test replay) | `audio/file.rs` | `FileBackend` |
| stdin raw i16 PCM backend | `audio/stdin.rs` | — |
| Streaming rational resampler | `audio/resample.rs` | `RationalResampler` |
| Capture fan-out (1 source → N consumers) | `audio/fanout.rs` | — |

## Daemon: devices & identity (`crates/omnimodem/src/`)

| Concern | File | Key symbol |
|---|---|---|
| Stable cross-platform `DeviceId` + canonical string round-trip | `ids.rs` | `DeviceId` |
| Enumerator trait + descriptor + fakes | `device/enumerate.rs` | `DeviceEnumerator`, `DeviceDescriptor`, `FakeEnumerator` |
| Production enumerator (cpal + nusb) | `device/mod.rs` | `RealEnumerator` |
| Device cache (`DeviceId` → live device) | `device/cache.rs` | `DeviceCache` |
| Hotplug diff (arrivals/departures) | `device/hotplug.rs` | `HotplugWatcher` |

## Daemon: PTT (`crates/omnimodem/src/ptt/`)

| Concern | File | Key symbol |
|---|---|---|
| `PttDriver` trait + structured `PttError` | `ptt/mod.rs` | `PttDriver`, `PttError` |
| Driver factory + registry + hotplug eviction | `ptt/registry.rs` | `PortRegistry`, `DriverOpener` |
| Serial RTS/DTR driver (unkey on Drop) | `ptt/serial.rs` | `SerialLinePtt` |
| CM108 HID GPIO driver | `ptt/cm108.rs` | `Cm108Ptt` |
| Linux gpiochip (chardev v2) driver | `ptt/gpio.rs` | `GpioPtt` |
| None / VOX no-op driver + mock | `ptt/none.rs` | `NonePtt`, `MockPtt` |
| Keying sequence (tx_delay/tx_tail, watermark drain, cancel) | `ptt/sequence.rs` | `drive_tx_cycle` |
| RX/TX interlock (per-rig, nesting-safe) | `ptt/interlock.rs` | `RxTxInterlock` |
| Exclusive TX lease (per-rig) | `ptt/lease.rs` | `TxLeaseRegistry` |
| udev rule suggestion (never writes) | `ptt/udev.rs` | `suggest` |
| USB attribute probing for identity/rules | `ptt/udev.rs` | — |

## Daemon: KISS bridge, persistence, metrics, authz

| Concern | File | Key symbol |
|---|---|---|
| KISS-over-TCP listener registry (packet modes) | `kiss/listener.rs` | `KissRegistry` |
| KISS FEND/FESC framing codec | `kiss/codec.rs` | `KissDecoder`, `encode_data_frame` |
| SQLite config store (keyed on `DeviceId`) | `persist/mod.rs` | `Store` |
| Per-channel metrics accumulator | `metrics/mod.rs` | `ChannelMetrics`, `ChannelMetricsSnapshot` |
| Prometheus exporter | `metrics/prometheus.rs` | `serve`, `render` |
| Transport abstraction + serve helpers + validation | `authz/mod.rs` | `Transport`, `serve_uds`, `serve_routable`, `validate_transport` |
| UDS `SO_PEERCRED` authz + socket hardening | `authz/uds.rs` | `peer_uid_allowed` |
| Routable mTLS config (fails closed) | `authz/tls.rs` | `routable_tls_config` |

## DSP library (`crates/dsp/src/`)

Full stage-by-stage detail in [`dsp-building-blocks.md`](dsp-building-blocks.md).
Crate name `omnimodem-dsp`.

| Concern | File |
|---|---|
| Public surface + re-exports | `lib.rs` |
| Soft-info spine types (`Llr`, `SoftBits`, `Frame`, `FramePayload`, `FrameMeta`, `Sample`, `Cplx`) | `types.rs` |
| Mode traits (`Demodulator`, `BlockDemodulator`, `Modulator`, `ModeCaps`, `DemodShape`, `Duplex`, `ModError`) | `mode.rs` |
| Diversity ensemble ("hydra") | `ensemble.rs` |
| Front-end DSP (agc, detector, fir, limiter, modulate, msk, multicarrier, nco, noise, ofdm, osc, resample, rsid, spectrum, stft) | `frontend/` |
| Synchronization (candidate, costas, costas_array, dcd, dpll, syncword, timing) | `sync/` |
| FEC & coding (ldpc*, rs*, conv, fano, osd, golay, fht, crc, llr, nrzi, gray, interleave, scramble, slicer, ccir476, *_tables) | `fec/` |
| Source/message/framing (ax25, hdlc, fx25, il2p, baudot, morse, *varicode, pack77, message77, hellfont, js8_*, jsc) | `framing/` |
| Mode implementations | `modes/` (see [`mode-catalog.md`](mode-catalog.md)) |
| Seeded RNG, AWGN, Watterson fading, KAT helpers | `testutil.rs` |

## Tests

| Scope | Location |
|---|---|
| Daemon integration (unary, subscribe, e2e, e2e_hardware) | `crates/omnimodem/tests/` |
| DSP known-answer, round-trip, BER, snapshots, loopback, alloc-guard | `crates/dsp/tests/` |
| DSP hot-path benchmarks (`criterion`) | `crates/dsp/benches/hotpath.rs` |
| TUI unit tests | `clients/omnimodem-tui/internal/**/**_test.go` |
