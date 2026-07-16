# Building and running omnimodem + the TUI

Two processes: the **modem daemon** (`omnimodem`, Rust) and the **TUI client**
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
make            # builds both → target/release/omnimodem and bin/omnimodem-tui
# or individually:
make modem
make tui
```

## Run

Start the daemon first, then the TUI — in two terminals:

```sh
# terminal 1 — the modem daemon
make run-modem
#   (or: ./target/release/omnimodem)
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
- `OMNIMODEM_CONFIG` — path to the daemon config file (default `$OMNIMODEM_RUNTIME_DIR/omnimodem.conf`). A missing file is fine.
- `RUST_LOG` — log level (default `info`).
- `NO_COLOR` — set to any non-empty value to strip ANSI color from log output (same effect as the `--no-color` flag below).

### Daemon flags

- `--no-color` — suppress ANSI color escape sequences in log output. Useful when a
  parent process captures the daemon's stderr into a plain-text sink that would
  render the escapes literally (the ADS-B Enjoyer app passes this).

### Daemon config file

An optional, line-oriented file registers `rtl_tcp` SDR endpoints so `ListDevices`
surfaces them for selection — useful for remote dongles that no hardware scan can
find. `#` starts a comment; blank lines are ignored. Malformed lines are skipped
with a warning rather than failing daemon start. Registration is a convenience:
any `rtltcp:host:port` can still be bound ad-hoc via `ConfigureAudio` without a
config entry.

```text
# <runtime_dir>/omnimodem.conf
rtl_tcp 192.168.1.50:1234 Rooftop R820T
rtl_tcp 127.0.0.1:1234
```

For the full RTL-SDR setup — starting `rtl_tcp`, binding, tuning, the waterfall,
demod modes, gain/squelch/ppm, and the reconnect/overrun behavior — see the operator's
guide [`sdr-rtl-tcp.md`](sdr-rtl-tcp.md).

## Native (local USB) RTL-SDR dongles

A dongle plugged straight into this machine is discovered automatically — no
`rtl_tcp`, no config entry — and shows up in `ListDevices` as an `rtl:` device.
The daemon claims the USB interface directly (pure-Rust `nusb`, no `librtlsdr`).
Each OS needs a one-time permission or driver step; when it hasn't been done the
device still appears in `ListDevices` with **`needs_setup`** set, so the TUI can
tell you *what* to fix instead of the dongle silently failing to open.

For the full operator guide — auto/manual/remote selection, tuning, demod modes,
gain, and the plug-in→decode-APRS bring-up checklist — see
[`sdr-rtl-usb.md`](sdr-rtl-usb.md). The per-OS setup commands are below.

### Linux — udev rule + DVB driver blacklist

Two independent things can keep the daemon from opening the dongle:

1. **Permissions.** By default only root can open a raw USB device. Install the
   bundled udev rule to grant your desktop user access:

   ```sh
   sudo cp packaging/udev/99-omnimodem-rtlsdr.rules /etc/udev/rules.d/
   sudo udevadm control --reload-rules && sudo udevadm trigger
   # then unplug and re-plug the dongle
   ```

   The rule tags every recognized RTL2832U id (mirroring `RTL_USB_IDS` in
   `crates/omnimodem/src/device/enumerate.rs`) with `uaccess`, handing access to
   the logged-in seat's user, with `MODE="0660"` as a fallback.

2. **The kernel DVB-T driver.** On most distros `dvb_usb_rtl28xxu` binds the
   dongle at plug-in and treats it as a TV tuner. omnimodem detaches it at claim
   time, but the clean fix is to blacklist it so it never grabs the device:

   ```sh
   # /etc/modprobe.d/blacklist-omnimodem-rtlsdr.conf
   blacklist dvb_usb_rtl28xxu
   ```

   Then unload it for the current session (`sudo rmmod dvb_usb_rtl28xxu`) or
   reboot. If the daemon logs a claim/`needs_setup` error on Linux, this driver
   is almost always the cause.

### Windows — Zadig (WinUSB) driver

Windows will not let the daemon claim the dongle until a generic **WinUSB** (or
libusb) driver is bound to it. Until then the device is listed with
`needs_setup` set. One-time fix with [Zadig](https://zadig.akeo.ie/):

1. Plug in the dongle and run Zadig as Administrator.
2. **Options → List All Devices**, then pick the RTL-SDR interface (a
   `Bulk-In, Interface (Interface 0)` on a `0bda`-family dongle).
3. Select the **WinUSB** driver in the target box and click *Replace Driver*.
4. Re-run `ListDevices` — `needs_setup` clears and the dongle is bindable.

Zadig replaces the driver per physical port; move the dongle to a different USB
port and you may need to repeat it for that port.

### macOS

No per-device setup is required. macOS has no in-kernel DVB driver competing for
the dongle, so the daemon claims interface 0 directly on plug-in and the device
is immediately bindable — `needs_setup` stays `false`. If a claim ever fails,
make sure no other SDR application (SDR++, CubicSDR, GQRX via SoapySDR) already
holds the device.

## Test

```sh
make test        # Rust workspace + Go TUI
```
