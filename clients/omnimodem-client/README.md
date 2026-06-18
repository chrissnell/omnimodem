# omnimodem-client

A small Go gRPC client for exercising an `omnimodemd` daemon during development
and hardware bring-up. It enumerates audio/PTT devices, then drives a channel
through the full Phase-2 surface: configure audio, configure PTT, key/unkey the
transmitter, and transmit a generated PCM tone — printing the event stream
throughout. It connects over the daemon's authorized Unix-domain socket
(SO_PEERCRED is satisfied by running as the same user as the daemon).

## Build

```sh
cd clients/omnimodem-client
go build -o omnimodem-client .
```

## Use

```sh
# Against a running daemon:
omnimodem-client -socket /run/omnimodem/omnimodem.sock

# Pick a method/node for real hardware:
omnimodem-client -socket /run/omnimodem/omnimodem.sock \
  -ptt-method rigctld -ptt-node 127.0.0.1:4532
```

Flags: `-socket`, `-channel`, `-device` (default: first from ListDevices),
`-rate`, `-ptt-method` (none|vox|serial_rts|serial_dtr|cm108|gpio|rigctld),
`-ptt-node`, `-ptt-pin`, `-ptt-invert`, `-tone`, `-tone-ms`.

## Try it with no hardware

A deterministic loopback modem (one synthetic device, MockPtt, file audio)
ships as a daemon example:

```sh
# terminal 1 — start the loopback modem
cargo run -p omnimodemd --example loopback_server -- /tmp/omni.sock

# terminal 2 — drive it
cd clients/omnimodem-client && go run . -socket /tmp/omni.sock
```

Expected: ListDevices shows one `alsa:loopback` device, ConfigureAudio/Ptt
succeed, and the event stream prints `ptt_state` (keyed / unkeyed) plus
`transmit_started` / `transmit_complete`.

## Regenerating the stubs

`omnimodemv1/*.pb.go` is generated from `proto/omnimodem.proto`. Re-run
`./generate.sh` after changing the proto (requires `protoc`, `protoc-gen-go`,
`protoc-gen-go-grpc`).
