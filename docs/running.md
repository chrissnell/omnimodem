# Building and running omnimodem + the TUI

Two processes: the **modem daemon** (`omnimodemd`, Rust) and the **TUI client**
(`omnimodem-tui`, Go). The TUI talks to the daemon over a gRPC Unix-domain socket.

## Prerequisites

- Rust toolchain (`cargo`)
- `protoc` (protobuf-compiler) — the daemon compiles `proto/omnimodem.proto` at build time
- `libasound2-dev` + `pkg-config` — ALSA, linked by the audio backend on Linux
- Go 1.26+

The TUI's Go bindings are committed, so building the TUI needs only Go. Run
`make proto` (needs `protoc` + the Go plugins) only when `proto/omnimodem.proto`
changes.

## Build

```sh
make            # builds both → target/release/omnimodemd and bin/omnimodem-tui
# or individually:
make modem
make tui
```

## Run

Start the daemon first, then the TUI — in two terminals:

```sh
# terminal 1 — the modem daemon
make run-modem
#   (or: ./target/release/omnimodemd)
# It logs the socket it bound, e.g. /tmp/omnimodem/omnimodem.sock

# terminal 2 — the TUI
make run-tui
#   (or: ./bin/omnimodem-tui)
```

The TUI auto-connects to the daemon's **default** socket — no flags needed: both
default to `$OMNIMODEM_RUNTIME_DIR/omnimodem.sock`, or `<tempdir>/omnimodem/omnimodem.sock`
(`/tmp/omnimodem/omnimodem.sock` on Linux) when that env var is unset.

### Custom socket location

Point both at the same directory:

```sh
export OMNIMODEM_RUNTIME_DIR=/run/user/$(id -u)   # daemon + TUI both honor this
make run-modem
make run-tui
```

Or pass the TUI an explicit path/address:

```sh
./bin/omnimodem-tui --addr /tmp/omnimodem/omnimodem.sock   # UDS path
./bin/omnimodem-tui --addr 127.0.0.1:9000                  # TCP host:port
```

### Daemon environment knobs

- `OMNIMODEM_RUNTIME_DIR` — where the socket + state DB live (default `<tempdir>/omnimodem`).
- `OMNIMODEM_ROUTABLE_ADDR` — bind a routable mTLS TCP endpoint instead of the UDS (requires TLS material).
- `OMNIMODEM_PROMETHEUS_ADDR` — expose the Prometheus metrics exporter.
- `RUST_LOG` — log level (default `info`).

## Test

```sh
make test        # Rust workspace + Go TUI
```
