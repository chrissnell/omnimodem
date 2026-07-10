# Omnimodem Cross-Platform PTT & Audio Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make omnimodem's PTT and audio work on **Linux, macOS, Windows, and Android** by porting Graywolf's proven per-platform adapters in behind omnimodem's *existing* trait seams, and improving on Graywolf where omnimodem's structure already allows it.

**Architecture:** Phase 2 already shipped the right shape — an `AudioBackend` trait, a `PttDriver` trait, three hardware-seam traits (`ModemControlLines`, `Cm108Hid`, `GpiochipLine`), a structured `PttError`, a `PortRegistry` with DeviceId-keyed eviction, and a `DeviceId` enum built for durable USB identity. The Linux adapters are implemented; everything else fails closed. This plan fills the matrix: it adds the missing per-OS adapters (Windows serial, macOS/Windows CM108), a fully portable `rigctld` network method, an Android JNI audio+PTT path, and fixes the two cross-platform audio gaps (playback is hardcoded to I16; device identity is not USB-durable). The work is structured so the platform-agnostic logic is unit-tested in CI and each OS-specific adapter is compile-gated (via `cross`/`cargo-ndk`) with a documented manual hardware gate.

**Tech Stack:** Rust 2021; `cpal` 0.17 (audio, all desktop OSes + Android/AAudio), `nix` 0.29 (unix serial/CM108 ioctls), `hidapi` 2.6 (macOS/Windows CM108 HID), `gpiocdev` 0.7 (Linux GPIO), `nusb` 0.1 (USB topology/identity), the `windows` 0.59 crate (Windows serial + HID), `jni` 0.21 + `ndk-context` 0.1 + `android_logger` (Android). Builds: `cross` (desktop cross-compile) and `cargo-ndk` (Android cdylib). Lifts the per-platform mechanics from Graywolf `graywolf-modem/src/tx/ptt*.rs`, `src/audio/soundcard.rs`, `src/tx/ptt_rigctld.rs`, and `src/android/` (checked out alongside for reference).

---

## Scope

**In scope:** make the existing Phase-2 control surface (`ListDevices`, `ConfigureAudio`, `ConfigurePtt`, `KeyPtt`, `Transmit`) function on macOS, Windows, and Android, plus add a portable `rigctld` PTT method that works on every desktop OS. No DSP, no mode — same exit criterion as Phase 2, now satisfiable off-Linux.

**Two-track structure.** PTT (Parts B–C, E1) and audio (Part D, E2) are largely independent and can be built in parallel after Part A; the Android JNI work (Part E) depends on both and on a Kotlin app shell that is **out of scope for the daemon crate** — Part E delivers the Rust-side JNI seam + host-testable dispatch and documents the Kotlin contract, leaving the app shell as a follow-on.

**Explicitly out of scope (deferred):** SDR/JACK audio backends; DSP/demod/mode; mTLS/routable binds (Phase 5); the full Android *application* (Kotlin UI, gradle app, Play Store packaging) — only the Rust JNI bridge + build glue land here.

**Improvements over Graywolf baked into this plan** (each tagged `[IMPROVEMENT]` at its task):
1. **Structured errors everywhere.** Graywolf's `PttDriver` is `Result<(), String>`; omnimodem already has a structured `PttError`. Every new adapter (Windows serial, macOS/Windows CM108, rigctld, Android) maps OS errors to `PttError` variants so the core can distinguish retry/surface/evict — Graywolf can only do this for GPIO.
2. **USB-durable device identity.** Graywolf reads USB VID/PID/serial for *display only* and keys identity on the ALSA card name (volatile across replug when the kernel assigns a numeric index). omnimodem's `DeviceId::Usb { vid, pid, serial }` is built to key on the durable triple; this plan wires `nusb` to actually *produce* `Usb` identities, so config survives replug on every OS.
3. **Format/rate hardening on both directions.** Graywolf applies its I16-preferring, rate-capped selection to both capture and playback; omnimodem currently only does capture. This plan generalizes it to playback, fixing the I16-only output that breaks macOS/Windows (which typically offer F32).
4. **All-driver hotplug eviction.** Graywolf only auto-recovers GPIO and rigctld; serial/CM108 stay broken after replug. omnimodem's `PortRegistry` already evicts by `DeviceId`; this plan ensures the macOS/Windows adapters surface `DeviceGone` so eviction fires on every platform/method.
5. **No magic-number sentinels.** Graywolf's rigctld returns a `-9999` sentinel for malformed replies that every caller special-cases; omnimodem's rigctld maps directly to `PttError`.
6. **Host-testable JNI dispatch.** Lifts Graywolf's `android-test-stub` feature so the Android PTT/audio dispatch is unit-tested on the Linux CI host with no JVM.

---

## File Structure

```
crates/omnimodem/
  Cargo.toml                          + target-gated windows/jni/ndk-context/android_logger; android-test-stub feature
  src/
    ids.rs                            (unchanged; DeviceId::Usb already exists)
    audio/
      alsa.rs                         rename pick_input_sample_format -> pick_sample_format (used by capture AND playback)
      cpal_backend.rs                 playback gains format-matched build_output (I16/F32/U16); Windows device-id identity
      identity.rs        NEW          pure: map a cpal device (+ nusb scan) to the most durable DeviceId
    device/
      mod.rs                          RealEnumerator upgrades ids to DeviceId::Usb via audio::identity
    ptt/
      mod.rs                          declares the new platform adapter modules via #[path]+cfg aliasing
      serial.rs                       unix adapter unchanged; add platform alias for the Unix/Windows serial seam
      serial_win.rs      NEW          WinSerialLines: CreateFileW + EscapeCommFunction (ModemControlLines impl)
      cm108.rs                        unix adapter unchanged
      cm108_hidapi.rs    NEW          HidApiCm108: hidapi-backed Cm108Hid impl (macOS + Windows)
      rigctld.rs         NEW          RigctldPtt: portable std::net TCP PttDriver + structured errors
      registry.rs                     RealOpener: platform-aliased adapters + Rigctld arm; PttMethod gains Rigctld
      android.rs         NEW          AndroidPtt (JNI method-int delegation) + android-test-stub
    android/             NEW (cfg)    jni bridge: audio ingest (Kotlin->Rust), tx sink + ptt upcalls (Rust->Kotlin)
      mod.rs
      upcall.rs
    grpc/convert.rs                   proto PttMethod::Rigctld -> domain; rigctld node carries host:port
  proto/omnimodem.proto               additive: PTT_METHOD_RIGCTLD = 7
  Cross.toml             NEW          per-target pre-build: cross ALSA/udev dev libs + protoc; pkg-config passthrough
  .cargo/config.toml     NEW          armv7 NEON split (optional, for Pi targets)
```

**Boundaries.** Each per-OS adapter is one file implementing one existing seam trait; the portable `rigctld` is one file implementing `PttDriver` directly. `audio/identity.rs` is pure (testable). The `#[path]`+`use … as Platform…` aliasing keeps `registry.rs` free of inline `#[cfg]` soup. The Android JNI code is isolated under `src/android/` behind `#[cfg(target_os = "android")]` (plus the `android-test-stub` feature for host tests), mirroring Graywolf's `lib.rs:111-130` layout.

---

## PART A — Foundation: platform-module aliasing + proto

## Task 1: Adopt Graywolf's `#[path]` + alias module pattern for serial

Today `registry.rs`'s `RealOpener` inlines `#[cfg(unix)]` arms. Refactor to Graywolf's pattern (`graywolf-modem/src/tx/ptt.rs:99-130`): each platform adapter is a `#[cfg]`-gated module aliased to one neutral type, so the factory names a single type regardless of OS. This is purely structural and keeps Linux behavior identical.

**Files:**
- Modify: `crates/omnimodem/src/ptt/serial.rs`
- Modify: `crates/omnimodem/src/ptt/mod.rs`

- [ ] **Step 1: In `ptt/serial.rs`, keep `pub mod unix` as-is and add a platform alias at the bottom**

```rust
/// The serial-line adapter for this target. Unix uses TIOCMSET ioctls; Windows
/// (Task 5) uses EscapeCommFunction. Both implement `ModemControlLines`.
#[cfg(unix)]
pub use unix::UnixSerialLines as PlatformSerialLines;
#[cfg(windows)]
pub use super::serial_win::WinSerialLines as PlatformSerialLines;
```

- [ ] **Step 2: Run the existing serial tests to confirm no behavior change**

Run: `cargo test -p omnimodem ptt::serial::`
Expected: PASS (3 tests) — the alias is additive; Linux still uses `UnixSerialLines`.

- [ ] **Step 3: Commit**

```bash
git add crates/omnimodem/src/ptt/serial.rs crates/omnimodem/src/ptt/mod.rs
git commit -m "Introduce platform-aliased serial adapter type"
```

---

## Task 2: Add the `Rigctld` PTT method to the proto and domain enums

`rigctld` is additive within `omnimodem.v1`: a new `PttMethod` enum value and a new domain variant. The `node` field carries `host:port`.

**Files:**
- Modify: `proto/omnimodem.proto`
- Modify: `crates/omnimodem/src/ptt/registry.rs`
- Modify: `crates/omnimodem/src/grpc/convert.rs`

- [ ] **Step 1: Add the proto enum value (new tag only; nothing renumbered)**

In `proto/omnimodem.proto`, in `enum PttMethod`, after `PTT_METHOD_GPIO = 6;`:

```proto
  PTT_METHOD_RIGCTLD = 7;   // Hamlib rigctld over TCP (host:port in `node`)
```

- [ ] **Step 2: Add the domain variant**

In `crates/omnimodem/src/ptt/registry.rs`, in `pub enum PttMethod`, add:

```rust
    /// Hamlib `rigctld` over TCP. `addr` is `host:port` (e.g. "127.0.0.1:4532").
    Rigctld { addr: String },
```

- [ ] **Step 3: Map it in `proto_ptt_to_config`**

In `crates/omnimodem/src/grpc/convert.rs`, add a match arm alongside the others:

```rust
        Ok(proto::PttMethod::Rigctld) => PttMethod::Rigctld { addr: req.node.clone() },
```

- [ ] **Step 4: Build (codegen) and run the proto smoke test**

Run: `cargo build -p omnimodem && cargo test -p omnimodem proto::tests::phase2_types_are_constructible`
Expected: PASS — `PttMethod::Rigctld as i32 == 7` is constructible; the service still compiles.

- [ ] **Step 5: Commit**

```bash
git add proto/omnimodem.proto crates/omnimodem/src/ptt/registry.rs crates/omnimodem/src/grpc/convert.rs
git commit -m "Add rigctld PTT method (additive within v1)"
```

---

## PART B — Portable PTT: rigctld (works on every desktop OS today)

## Task 3: `RigctldPtt` — Hamlib network PTT over TCP

The highest-ROI cross-platform task: one pure-`std::net` driver that keys a radio on Linux, macOS, and Windows identically — no per-OS adapter. Lifted from Graywolf `tx/ptt_rigctld.rs`, but with `[IMPROVEMENT] #1/#5`: it returns structured `PttError` (not `Result<(),String>`) and has no `-9999` sentinel. The line-protocol parsing is pure and fully unit-tested; the socket path is exercised against an in-test fake rigctld server.

**Files:**
- Create: `crates/omnimodem/src/ptt/rigctld.rs`
- Modify: `crates/omnimodem/src/ptt/mod.rs` (declare `pub mod rigctld;`)

- [ ] **Step 1: Write the protocol parser with failing tests**

Create `crates/omnimodem/src/ptt/rigctld.rs`:

```rust
//! Hamlib `rigctld` PTT over TCP. Portable on every OS (pure std::net). The
//! line protocol: `T 1`/`T 0` sets PTT and replies `RPRT <n>` (0 = ok); `t`
//! gets PTT and replies a bare `0`/`1` line (NOT followed by RPRT) on success.
//! Lifted from Graywolf `tx/ptt_rigctld.rs`, mapped to structured `PttError`.

use super::{PttDriver, PttError};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::time::Duration;

const IO_TIMEOUT: Duration = Duration::from_millis(500);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const UNKEY_SAFETY_RETRIES: u32 = 3;

/// Parse an `RPRT <n>` reply. `Ok(())` on `RPRT 0`; otherwise a structured error.
/// A malformed line is an `Io` error (no magic sentinel — improvement over Graywolf).
pub fn parse_rprt(line: &str) -> Result<(), PttError> {
    let line = line.trim();
    match line.strip_prefix("RPRT ") {
        Some(code) => match code.trim().parse::<i32>() {
            Ok(0) => Ok(()),
            Ok(n) => Err(PttError::Io(format!("rigctld RPRT {n}"))),
            Err(_) => Err(PttError::Io(format!("malformed rigctld reply: {line:?}"))),
        },
        None => Err(PttError::Io(format!("expected RPRT, got: {line:?}"))),
    }
}

/// Parse the bare `t` (get-PTT) reply: a single `0` or `1` line.
pub fn parse_get_ptt(line: &str) -> Result<bool, PttError> {
    match line.trim() {
        "1" => Ok(true),
        "0" => Ok(false),
        // An error surfaces as RPRT here; reuse parse_rprt to extract it.
        other => parse_rprt(other).map(|_| false),
    }
}

#[cfg(test)]
mod parse_tests {
    use super::*;

    #[test]
    fn rprt_zero_is_ok() {
        assert!(parse_rprt("RPRT 0").is_ok());
        assert!(parse_rprt("RPRT 0\n").is_ok());
    }

    #[test]
    fn rprt_nonzero_is_err() {
        assert!(matches!(parse_rprt("RPRT -1"), Err(PttError::Io(_))));
    }

    #[test]
    fn malformed_is_err_not_sentinel() {
        assert!(matches!(parse_rprt("garbage"), Err(PttError::Io(_))));
        assert!(matches!(parse_rprt(""), Err(PttError::Io(_))));
    }

    #[test]
    fn get_ptt_parses_bare_line() {
        assert_eq!(parse_get_ptt("1").unwrap(), true);
        assert_eq!(parse_get_ptt("0").unwrap(), false);
        assert!(parse_get_ptt("RPRT -5").is_err());
    }
}
```

- [ ] **Step 2: Run the parser tests (verify they pass)**

Run: `cargo test -p omnimodem ptt::rigctld::parse_tests`
Expected: PASS (4 tests).

- [ ] **Step 3: Add the `RigctldPtt` driver over the connection**

Append to `crates/omnimodem/src/ptt/rigctld.rs`:

```rust
/// A rigctld connection. `key`/`unkey` send `T 1`/`T 0`. Unkey is safety-retried
/// (a stuck-keyed radio is worse than a failed key); Drop force-unkeys.
pub struct RigctldPtt {
    stream: TcpStream,
    reader: BufReader<TcpStream>,
    addr: String,
}

impl RigctldPtt {
    pub fn connect(addr: &str) -> Result<Self, PttError> {
        let sockaddr = addr
            .to_socket_addrs_first()
            .ok_or_else(|| PttError::Config(format!("unresolvable rigctld addr {addr}")))?;
        let stream = TcpStream::connect_timeout(&sockaddr, CONNECT_TIMEOUT)
            .map_err(|e| map_io(addr, e))?;
        stream.set_read_timeout(Some(IO_TIMEOUT)).ok();
        stream.set_write_timeout(Some(IO_TIMEOUT)).ok();
        stream.set_nodelay(true).ok();
        let reader = BufReader::new(stream.try_clone().map_err(|e| map_io(addr, e))?);
        let mut d = RigctldPtt { stream, reader, addr: addr.to_string() };
        // Probe + force RX, parity with the other drivers' startup-unkey.
        d.unkey()?;
        Ok(d)
    }

    fn command_rprt(&mut self, cmd: &str) -> Result<(), PttError> {
        self.stream.write_all(cmd.as_bytes()).map_err(|e| map_io(&self.addr, e))?;
        let mut line = String::new();
        self.reader.read_line(&mut line).map_err(|e| map_io(&self.addr, e))?;
        parse_rprt(&line)
    }
}

impl PttDriver for RigctldPtt {
    fn key(&mut self) -> Result<(), PttError> {
        self.command_rprt("T 1\n")
    }
    fn unkey(&mut self) -> Result<(), PttError> {
        let mut last = self.command_rprt("T 0\n");
        let mut tries = 0;
        while last.is_err() && tries < UNKEY_SAFETY_RETRIES {
            std::thread::sleep(Duration::from_millis(150));
            last = self.command_rprt("T 0\n");
            tries += 1;
        }
        last
    }
}

impl Drop for RigctldPtt {
    fn drop(&mut self) {
        let _ = self.command_rprt("T 0\n"); // never leave a rig keyed
    }
}

/// Map a socket io error to a structured PttError (improvement #1).
fn map_io(addr: &str, e: std::io::Error) -> PttError {
    use std::io::ErrorKind::*;
    match e.kind() {
        PermissionDenied => PttError::PermissionDenied { device: addr.into() },
        ConnectionRefused | ConnectionReset | NotConnected | BrokenPipe => {
            PttError::DeviceGone { device: addr.into() }
        }
        _ => PttError::Io(format!("{addr}: {e}")),
    }
}

/// Tiny helper: first resolved socket address.
trait FirstSocketAddr {
    fn to_socket_addrs_first(&self) -> Option<std::net::SocketAddr>;
}
impl FirstSocketAddr for str {
    fn to_socket_addrs_first(&self) -> Option<std::net::SocketAddr> {
        use std::net::ToSocketAddrs;
        self.to_socket_addrs().ok()?.next()
    }
}
```

- [ ] **Step 4: Add an integration test against a fake rigctld server**

Append to `crates/omnimodem/src/ptt/rigctld.rs`:

```rust
#[cfg(test)]
mod server_tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;

    /// A minimal rigctld stand-in: answers `T 0/1` with `RPRT 0` and records keys.
    fn fake_rigctld() -> (String, mpsc::Receiver<bool>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            if let Ok((sock, _)) = listener.accept() {
                let mut w = sock.try_clone().unwrap();
                let mut r = BufReader::new(sock);
                let mut line = String::new();
                while r.read_line(&mut line).unwrap_or(0) > 0 {
                    if let Some(rest) = line.trim().strip_prefix("T ") {
                        let _ = tx.send(rest == "1");
                    }
                    w.write_all(b"RPRT 0\n").unwrap();
                    line.clear();
                }
            }
        });
        (addr, rx)
    }

    #[test]
    fn connect_keys_and_unkeys() {
        let (addr, rx) = fake_rigctld();
        let mut d = RigctldPtt::connect(&addr).unwrap();
        // connect() did a startup unkey:
        assert_eq!(rx.recv().unwrap(), false);
        d.key().unwrap();
        assert_eq!(rx.recv().unwrap(), true);
        d.unkey().unwrap();
        assert_eq!(rx.recv().unwrap(), false);
    }
}
```

- [ ] **Step 5: Declare the module and run the tests**

Add `pub mod rigctld;` to `crates/omnimodem/src/ptt/mod.rs`. Run:

Run: `cargo test -p omnimodem ptt::rigctld::`
Expected: PASS (parser 4 + server 1) — connect/key/unkey round-trip against the fake server, startup-unkey observed.

- [ ] **Step 6: Commit**

```bash
git add crates/omnimodem/src/ptt/rigctld.rs crates/omnimodem/src/ptt/mod.rs
git commit -m "Add portable rigctld PTT driver with structured errors"
```

---

## Task 4: Wire `Rigctld` into `RealOpener`

**Files:**
- Modify: `crates/omnimodem/src/ptt/registry.rs`

- [ ] **Step 1: Add the match arm (no cfg gate — portable)**

In `RealOpener::open`, alongside the existing arms:

```rust
            PttMethod::Rigctld { addr } => {
                use super::rigctld::RigctldPtt;
                Ok(Box::new(RigctldPtt::connect(addr)?))
            }
```

- [ ] **Step 2: Add a registry test proving the arm builds via a fake server**

In `registry.rs` `mod tests`, add a test that starts the same fake rigctld (factor the helper into `crate::ptt::rigctld` `#[cfg(test)] pub(crate)` or duplicate the 15-line server) and calls `RealOpener.open(&PttConfig { method: Rigctld { addr }, .. })`, asserting `is_ok()`.

- [ ] **Step 3: Run and commit**

Run: `cargo test -p omnimodem ptt::registry::`
Expected: PASS — including the new rigctld-opens test.

```bash
git add crates/omnimodem/src/ptt/registry.rs
git commit -m "Wire rigctld into the PTT driver factory"
```

---

## PART C — Desktop per-OS PTT adapters

> Verification reality: these adapters call OS APIs absent on the Linux CI host. Each is verified by **(a)** a unit test of any pure logic against the existing seam fakes, **(b)** `cargo build --target <triple>` via `cross`/native runners (compile gate), and **(c)** a documented manual hardware smoke. Do **not** claim runtime success without the manual gate.

## Task 5: Windows serial RTS/DTR — `WinSerialLines`

Implements the existing `ModemControlLines` seam with the Windows mechanism (Graywolf `tx/ptt_win.rs`): `CreateFileW` in shared mode + stateless `EscapeCommFunction(SETRTS/CLRRTS/SETDTR/CLRDTR)`. No termios analog. `[IMPROVEMENT] #1`: maps `CreateFileW`/`EscapeCommFunction` failures to `PttError`.

**Files:**
- Create: `crates/omnimodem/src/ptt/serial_win.rs`
- Modify: `crates/omnimodem/src/ptt/mod.rs` (add `#[cfg(windows)] pub mod serial_win;`)

- [ ] **Step 1: Write the Windows adapter**

Create `crates/omnimodem/src/ptt/serial_win.rs`:

```rust
//! Windows serial RTS/DTR via CreateFileW + EscapeCommFunction. Implements the
//! `ModemControlLines` seam. Lifted from Graywolf `tx/ptt_win.rs`.
#![cfg(windows)]

use super::serial::ModemControlLines;
use super::PttError;
use windows::core::PCWSTR;
use windows::Win32::Devices::Communication::{
    EscapeCommFunction, CLRDTR, CLRRTS, SETDTR, SETRTS,
};
use windows::Win32::Foundation::{CloseHandle, HANDLE, GENERIC_READ, GENERIC_WRITE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, FILE_FLAGS_AND_ATTRIBUTES,
};

pub struct WinSerialLines {
    handle: HANDLE,
    device: String,
}

impl WinSerialLines {
    pub fn open(path: &str) -> Result<Self, PttError> {
        // Bare "COM12" must be addressed as r"\\.\COM12".
        let full = if path.starts_with(r"\\.\") { path.to_string() } else { format!(r"\\.\{path}") };
        let wide: Vec<u16> = full.encode_utf16().chain(std::iter::once(0)).collect();
        let handle = unsafe {
            CreateFileW(
                PCWSTR(wide.as_ptr()),
                (GENERIC_READ | GENERIC_WRITE).0,
                FILE_SHARE_READ | FILE_SHARE_WRITE, // shared: rigctld/fldigi may hold it too
                None,
                OPEN_EXISTING,
                FILE_FLAGS_AND_ATTRIBUTES(0),
                None,
            )
        }
        .map_err(|e| map_win(path, e))?;
        Ok(WinSerialLines { handle, device: path.to_string() })
    }

    fn escape(&self, func: u32) -> Result<(), PttError> {
        unsafe { EscapeCommFunction(self.handle, func) }.map_err(|e| map_win(&self.device, e))
    }
}

impl ModemControlLines for WinSerialLines {
    fn write_rts(&mut self, high: bool) -> Result<(), PttError> {
        self.escape(if high { SETRTS } else { CLRRTS })
    }
    fn write_dtr(&mut self, high: bool) -> Result<(), PttError> {
        self.escape(if high { SETDTR } else { CLRDTR })
    }
}

impl Drop for WinSerialLines {
    fn drop(&mut self) {
        unsafe { let _ = CloseHandle(self.handle); }
    }
}

fn map_win(device: &str, e: windows::core::Error) -> PttError {
    use windows::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, ERROR_SHARING_VIOLATION};
    match e.code().0 as u32 & 0xFFFF {
        c if c == ERROR_ACCESS_DENIED.0 => PttError::PermissionDenied { device: device.into() },
        c if c == ERROR_SHARING_VIOLATION.0 => PttError::Busy { device: device.into() },
        c if c == ERROR_FILE_NOT_FOUND.0 => PttError::DeviceGone { device: device.into() },
        _ => PttError::Io(format!("{device}: {e}")),
    }
}
```

> Note: confirm the exact `windows` 0.59 import paths and the `EscapeCommFunction` signature (it returns `windows::core::Result<()>` in recent versions); adjust the `.map_err` shape if the crate returns `BOOL`. Keep all Windows specifics inside this file.

- [ ] **Step 2: Wire it into `RealOpener` via the alias (already done in Task 1)**

`RealOpener`'s serial arms already construct `PlatformSerialLines`; change the `#[cfg(unix)]` gate on the SerialRts/SerialDtr arms to `#[cfg(any(unix, windows))]` and use `PlatformSerialLines::open(node)?` instead of `UnixSerialLines::open`. On Windows this resolves to `WinSerialLines`.

```rust
            #[cfg(any(unix, windows))]
            PttMethod::SerialRts { node } => {
                use super::serial::{PlatformSerialLines, SerialLine, SerialLinePtt};
                let lines = PlatformSerialLines::open(node)?;
                Ok(Box::new(SerialLinePtt::new(lines, SerialLine::Rts, cfg.invert, node.clone())?))
            }
```

(Mirror for `SerialDtr`.)

- [ ] **Step 3: Compile-gate for Windows**

Run: `cargo build -p omnimodem --target x86_64-pc-windows-msvc` (native Windows runner or `cargo check` on a Windows box; the `windows` crate does not cross-compile cleanly from Linux without the MSVC toolchain — document this as a CI-on-Windows step).
Expected: compiles; `WinSerialLines` satisfies `ModemControlLines`.

- [ ] **Step 4: Manual hardware gate (Windows host with a USB serial CAT/PTT cable)**

Document in the task: run `omnimodem`, `ConfigurePtt { method: SERIAL_RTS, node: "COM5" }`, `KeyPtt { keyed: true }` → rig TX LED lights; `keyed: false` drops it. Confirm a `PttState{keyed}` event each way.

- [ ] **Step 5: Commit**

```bash
git add crates/omnimodem/src/ptt/serial_win.rs crates/omnimodem/src/ptt/mod.rs crates/omnimodem/src/ptt/registry.rs
git commit -m "Add Windows serial RTS/DTR PTT adapter"
```

---

## Task 6: macOS + Windows CM108 — `HidApiCm108`

Linux writes the 5-byte HID report to `/dev/hidrawN` directly (already implemented). macOS has no `/dev/hidraw` and Windows needs `WriteFile`; both are served by the cross-platform `hidapi` crate (Graywolf `tx/ptt_cm108_macos.rs` + `ptt_cm108_win.rs`). One adapter covers both. The report layout is identical to the Linux path (it's already in `Cm108Ptt::set`), so this only implements the `Cm108Hid` *transport* seam.

**Files:**
- Create: `crates/omnimodem/src/ptt/cm108_hidapi.rs`
- Modify: `crates/omnimodem/src/ptt/mod.rs`, `crates/omnimodem/src/ptt/registry.rs`

- [ ] **Step 1: Write the hidapi adapter**

Create `crates/omnimodem/src/ptt/cm108_hidapi.rs`:

```rust
//! CM108 HID transport via the `hidapi` crate (macOS IOKit, Windows HID). The
//! report layout lives in `Cm108Ptt`; this only opens the device and writes the
//! 5-byte report. Lifted from Graywolf `tx/ptt_cm108_{macos,win}.rs`.
#![cfg(all(not(target_os = "linux"), not(target_os = "android")))]

use super::cm108::Cm108Hid;
use super::PttError;
use hidapi::HidApi;

pub struct HidApiCm108 {
    device: hidapi::HidDevice,
    path: String,
}

impl HidApiCm108 {
    /// `path` is the platform HID path the operator selected (IOKit registry
    /// path on macOS, device interface path on Windows).
    pub fn open(path: &str) -> Result<Self, PttError> {
        let api = HidApi::new().map_err(|e| PttError::Io(format!("hidapi init: {e}")))?;
        let cpath = std::ffi::CString::new(path)
            .map_err(|_| PttError::Config(format!("invalid hid path {path}")))?;
        let device = api
            .open_path(&cpath)
            .map_err(|e| map_hid(path, e))?;
        Ok(HidApiCm108 { device, path: path.to_string() })
    }
}

impl Cm108Hid for HidApiCm108 {
    fn write_report(&mut self, report: [u8; 5]) -> Result<(), PttError> {
        // hidapi prepends a report-id byte convention; CM108 uses report id 0.
        self.device.write(&report).map(|_| ()).map_err(|e| map_hid(&self.path, e))
    }
}

fn map_hid(path: &str, e: hidapi::HidError) -> PttError {
    let s = e.to_string().to_lowercase();
    if s.contains("permission") || s.contains("access") {
        PttError::PermissionDenied { device: path.into() }
    } else if s.contains("not found") || s.contains("no such") || s.contains("disconnect") {
        PttError::DeviceGone { device: path.into() }
    } else {
        PttError::Io(format!("{path}: {e}"))
    }
}
```

> Note: verify whether `hidapi::HidDevice::write` expects a leading report-id byte (some platforms require `[report_id, ...]` = 6 bytes for a 5-byte CM108 payload). If so, send `[0x00, 0x00, 0x00, value, mask, 0x00]`. Keep that detail inside this file; the report *content* still comes from `Cm108Ptt::set`. Settle this on the first macOS smoke test.

- [ ] **Step 2: Wire it into `RealOpener`**

Replace the CM108 arm so Linux keeps `UnixCm108Hid` and other unix/windows use `HidApiCm108`:

```rust
            #[cfg(target_os = "linux")]
            PttMethod::Cm108 { node, pin } => {
                use super::cm108::{unix::UnixCm108Hid, Cm108Ptt};
                let hid = UnixCm108Hid::open(node)?;
                Ok(Box::new(Cm108Ptt::new(hid, *pin, cfg.invert)?))
            }
            #[cfg(all(not(target_os = "linux"), not(target_os = "android")))]
            PttMethod::Cm108 { node, pin } => {
                use super::cm108::Cm108Ptt;
                use super::cm108_hidapi::HidApiCm108;
                let hid = HidApiCm108::open(node)?;
                Ok(Box::new(Cm108Ptt::new(hid, *pin, cfg.invert)?))
            }
```

- [ ] **Step 3: Compile gates**

Run: `cargo build -p omnimodem --target x86_64-apple-darwin` (macOS runner) and `--target x86_64-pc-windows-msvc` (Windows runner).
Expected: compiles; `HidApiCm108` satisfies `Cm108Hid` on both.

- [ ] **Step 4: Manual hardware gate (macOS + Windows, CM108 dongle e.g. DigiRig/AIOC)**

Document: `--list-devices` / OS HID enumeration to find the CM108 path; `ConfigurePtt { method: CM108, node: <hid path>, pin_or_line: 3 }`; key/unkey toggles the dongle GPIO (multimeter or rig TX LED). On macOS use the IOKit path; note the interface-number `-1` gotcha from Graywolf `cm108.rs:43-49`.

- [ ] **Step 5: Commit**

```bash
git add crates/omnimodem/src/ptt/cm108_hidapi.rs crates/omnimodem/src/ptt/mod.rs crates/omnimodem/src/ptt/registry.rs
git commit -m "Add macOS/Windows CM108 PTT adapter via hidapi"
```

---

## PART D — Cross-platform audio

## Task 7: Format-matched playback (fixes the I16-only output)

`[IMPROVEMENT] #3`. Today `cpal_backend::open_playback` hardcodes `build_output_stream::<i16>`; on macOS/Windows the device commonly only offers F32, so the stream fails to build and playback (hence `Transmit`) is dead there. Generalize the capture-side format selection to playback, mirroring `build_input`.

**Files:**
- Modify: `crates/omnimodem/src/audio/alsa.rs` (rename for symmetry)
- Modify: `crates/omnimodem/src/audio/cpal_backend.rs`

- [ ] **Step 1: Generalize the format picker name**

In `audio/alsa.rs`, rename `pick_input_sample_format` → `pick_sample_format` (it is direction-agnostic), and update its one caller in `cpal_backend.rs` capture. Keep the existing tests, renaming references. Run:

Run: `cargo test -p omnimodem audio::alsa::`
Expected: PASS (7 tests) after the rename.

- [ ] **Step 2: Add `output_configs` + a format-matched `build_output` to `cpal_backend.rs`**

Add, mirroring `input_configs`/`build_input`:

```rust
    fn output_configs(&self) -> Vec<(SampleFmt, u32, u32)> {
        let Ok(ranges) = self.device.supported_output_configs() else { return Vec::new(); };
        ranges.filter_map(|r| {
            let fmt = match r.sample_format() {
                cpal::SampleFormat::I16 => SampleFmt::I16,
                cpal::SampleFormat::F32 => SampleFmt::F32,
                cpal::SampleFormat::U16 => SampleFmt::U16,
                _ => return None,
            };
            Some((fmt, r.min_sample_rate(), r.max_sample_rate()))
        }).collect()
    }
```

Then in `open_playback`, after choosing `rate`, pick the output format and build the stream in that format, converting the shared i16 queue to the device format in the callback (i16 passthrough; f32 = `s as f32 / 32768.0`; u16 = `(s as i32 + 32768) as u16`). Factor the queue-drain so all three formats share the drain + watermark logic (the `Some(v) => { *s = conv(v); d.fetch_add(1, …) } None => silence` shape from the recent drain-watermark fix). Default to I16 when advertised (the AIOC/SignaLink case); fall back to F32 on macOS/Windows.

- [ ] **Step 3: Compile gate + Linux regression**

Run: `cargo build -p omnimodem` and `cargo test -p omnimodem` (Linux: all existing audio tests still pass; the file/null backends are unaffected). Then `cargo build --target x86_64-apple-darwin` / `x86_64-pc-windows-msvc`.
Expected: builds on all three; Linux suite green.

- [ ] **Step 4: Manual gate (macOS + Windows sound card)**

Document: `ConfigureAudio` + `Transmit` a PCM buffer; confirm audio plays out and PTT releases after drain (now that the watermark advances on a real F32 device).

- [ ] **Step 5: Commit**

```bash
git add crates/omnimodem/src/audio/alsa.rs crates/omnimodem/src/audio/cpal_backend.rs
git commit -m "Select playback sample format per device (fix I16-only output)"
```

---

## Task 8: USB-durable device identity via nusb + Windows endpoint id

`[IMPROVEMENT] #2`. `nusb` is a declared dependency but unused; `RealEnumerator` derives ids only from the ALSA pcm name. Add a pure `audio::identity` module that, given a cpal device's reported name (and on Windows its `Device::id()`), and a `nusb` device scan, returns the most durable `DeviceId` — preferring `DeviceId::Usb { vid, pid, serial }`. This makes config replug-stable across OSes (better than Graywolf, which only displays the serial).

**Files:**
- Create: `crates/omnimodem/src/audio/identity.rs`
- Modify: `crates/omnimodem/src/audio/mod.rs`, `crates/omnimodem/src/audio/cpal_backend.rs`, `crates/omnimodem/src/device/mod.rs`

- [ ] **Step 1: Pure identity-ranking with failing tests**

Create `crates/omnimodem/src/audio/identity.rs` with a pure function:

```rust
//! Derive the most durable `DeviceId` for an audio device. Preference order
//! mirrors `DeviceId`'s `Ord`: Usb{vid,pid,serial} > AlsaCard > Topology >
//! Placeholder. USB info comes from a nusb scan (Task wires the real scan);
//! this module's ranking is pure and testable.

use crate::ids::DeviceId;

/// A USB device seen by nusb, reduced to the fields we key on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsbInfo {
    pub vid: u16,
    pub pid: u16,
    pub serial: Option<String>,
    pub bus: u8,
    pub ports: String,
}

/// Given the ALSA card token (Linux) or raw device name, plus any matched USB
/// info, return the most durable identity.
pub fn best_identity(alsa_card: Option<&str>, usb: Option<&UsbInfo>) -> DeviceId {
    if let Some(u) = usb {
        if u.serial.as_deref().map(|s| !s.is_empty()).unwrap_or(false) {
            return DeviceId::Usb { vid: u.vid, pid: u.pid, serial: u.serial.clone().unwrap() };
        }
        // No serial: USB topology is more durable than an ALSA index alias.
        return DeviceId::Topology { bus: u.bus, ports: u.ports.clone() };
    }
    if let Some(card) = alsa_card {
        return DeviceId::AlsaCard { card_name: card.to_string() };
    }
    DeviceId::placeholder()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usb(serial: Option<&str>) -> UsbInfo {
        UsbInfo { vid: 0x0d8c, pid: 0x013c, serial: serial.map(String::from), bus: 1, ports: "1.2".into() }
    }

    #[test]
    fn usb_with_serial_wins() {
        assert_eq!(
            best_identity(Some("Device"), Some(&usb(Some("A1B2")))),
            DeviceId::Usb { vid: 0x0d8c, pid: 0x013c, serial: "A1B2".into() }
        );
    }

    #[test]
    fn usb_without_serial_falls_to_topology() {
        assert_eq!(
            best_identity(Some("Device"), Some(&usb(None))),
            DeviceId::Topology { bus: 1, ports: "1.2".into() }
        );
    }

    #[test]
    fn no_usb_uses_alsa_card() {
        assert_eq!(best_identity(Some("Device"), None), DeviceId::AlsaCard { card_name: "Device".into() });
    }

    #[test]
    fn nothing_is_placeholder() {
        assert_eq!(best_identity(None, None), DeviceId::placeholder());
    }
}
```

- [ ] **Step 2: Run the pure tests**

Add `pub mod identity;` to `audio/mod.rs`. Run: `cargo test -p omnimodem audio::identity::`
Expected: PASS (4 tests).

- [ ] **Step 3: Wire the real nusb scan into enumeration**

In `cpal_backend::enumerate_default_host` (or a new helper called by `RealEnumerator`), build a one-shot map of present USB devices via `nusb::list_devices()` (vid/pid/serial/bus/port-chain), then for each cpal device call `identity::best_identity(alsa_card_token(name), matched_usb)`. Matching a cpal audio device to its USB parent is best-effort: on Linux, resolve the ALSA card to its USB VID/PID by reading `/sys/class/sound/cardN/device/{idVendor,idProduct,serial}` (Graywolf `mod.rs:1426-1494`); on Windows/macOS, match on the USB product string contained in the cpal device name. When no USB match is found, fall back to the existing `AlsaCard`/`Placeholder` behavior. On Windows, additionally key the `DeviceDescriptor.label` off `Device::id()` (the IMMDevice endpoint id) so two endpoints of one class are distinguishable (Graywolf `soundcard.rs:881-892`). Keep `nusb` calls behind `#[cfg(not(target_os = "android"))]` (it is already target-gated in Cargo.toml).

- [ ] **Step 4: Build gate (Linux + cross targets)**

Run: `cargo build -p omnimodem` then `cargo build --target aarch64-unknown-linux-gnu` (via cross, see Part F). The `nusb` path compiles on all non-Android targets.
Expected: builds; Linux unit suite still green.

- [ ] **Step 5: Manual gate**

Document: plug a USB sound card, `ListDevices` → expect a `usb:VVVV:PPPP:<serial>` id; replug into a different port → the id is unchanged (durability), and a previously-configured channel still resolves.

- [ ] **Step 6: Commit**

```bash
git add crates/omnimodem/src/audio/identity.rs crates/omnimodem/src/audio/mod.rs crates/omnimodem/src/audio/cpal_backend.rs crates/omnimodem/src/device/mod.rs
git commit -m "Derive USB-durable DeviceId via nusb (replug-stable identity)"
```

---

## PART E — Android (Rust JNI seam; Kotlin app is a follow-on)

> Android does not let Rust touch `AudioRecord`/`AudioTrack`/USB directly — Kotlin owns them. The Rust side exposes a JNI bridge: Kotlin pushes captured PCM down, Rust pushes TX PCM and PTT method-ints up via cached callbacks (Graywolf `src/android/`). This part lands the Rust bridge + the `android-test-stub` so dispatch is host-testable; the Kotlin app shell is documented but out of scope.

## Task 9: `android-test-stub` feature + AndroidPtt dispatch

`[IMPROVEMENT] #6`. Lift Graywolf's pattern (`lib.rs:111-130`, `tx/ptt_android.rs`, `android/upcall.rs`): keep the JNI upcall behind a thin `jni_ptt_set(method, keyed)` function with two impls — a real `#[cfg(target_os="android")]` one and a host stub under `feature = "android-test-stub"` — so `AndroidPtt` dispatch is unit-tested on Linux.

**Files:**
- Create: `crates/omnimodem/src/android/mod.rs`, `crates/omnimodem/src/android/upcall.rs`, `crates/omnimodem/src/ptt/android.rs`
- Modify: `crates/omnimodem/Cargo.toml` (feature + android deps), `crates/omnimodem/src/lib.rs`, `crates/omnimodem/src/ptt/registry.rs`

- [ ] **Step 1: Add the feature and android deps to `Cargo.toml`**

```toml
[features]
default = []
# Compile the JNI upcalls as host mocks so Android dispatch is testable off-device.
android-test-stub = []

[target.'cfg(target_os = "android")'.dependencies]
jni = "0.21"
ndk-context = "0.1"
android_logger = "0.15"
```

- [ ] **Step 2: Write `upcall.rs` with a stub impl + test**

Create `crates/omnimodem/src/android/upcall.rs` with `pub fn jni_ptt_set(method: i32, keyed: bool) -> Result<(), String>`: a real impl (`#[cfg(target_os="android")]`) that pulls a cached `GlobalRef` to the Kotlin `UsbPttCallback` from `ndk_context` and calls `pttSet(int,boolean)`, and a stub (`#[cfg(all(not(target_os="android"), feature="android-test-stub"))]`) that records calls into a `thread_local`/`Mutex<Vec<(i32,bool)>>` a test can read. Add a test asserting a key then unkey is recorded.

- [ ] **Step 3: Write `ptt/android.rs` (`AndroidPtt`)**

```rust
//! Android PTT: the actual transport (CP2102 RTS / AIOC DTR / CM108 / VOX) is
//! chosen by Kotlin; Rust just forwards a method-int + keyed bool over JNI.
//! Lifted from Graywolf `tx/ptt_android.rs`.
#![cfg(any(target_os = "android", feature = "android-test-stub"))]

use super::{PttDriver, PttError};

pub struct AndroidPtt { method: i32 }

impl AndroidPtt {
    pub fn new(method: i32) -> Self { AndroidPtt { method } }
}

impl PttDriver for AndroidPtt {
    fn key(&mut self) -> Result<(), PttError> {
        crate::android::upcall::jni_ptt_set(self.method, true).map_err(PttError::Io)
    }
    fn unkey(&mut self) -> Result<(), PttError> {
        crate::android::upcall::jni_ptt_set(self.method, false).map_err(PttError::Io)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn key_unkey_forward_to_jni_stub() {
        let mut p = AndroidPtt::new(2 /* CM108 */);
        p.key().unwrap();
        p.unkey().unwrap();
        // assert via the upcall stub's recorded calls
    }
}
```

- [ ] **Step 4: Wire module + feature-gated test run**

In `lib.rs`, add the cfg-gated module wiring (mirror Graywolf `lib.rs:111-130`): `#[cfg(target_os="android")] pub mod android;` plus a `#[cfg(all(not(target_os="android"), feature="android-test-stub"))]` `#[path]` include of `android/upcall.rs` so the stub compiles on the host. Add the `Android { method: i32 }` arm to `PttMethod` + `RealOpener` under `#[cfg(any(target_os="android", feature="android-test-stub"))]`.

Run: `cargo test -p omnimodem --features android-test-stub ptt::android::`
Expected: PASS — dispatch forwards to the stub, recorded as (method, keyed).

- [ ] **Step 5: Commit**

```bash
git add crates/omnimodem/Cargo.toml crates/omnimodem/src/android/ crates/omnimodem/src/ptt/android.rs crates/omnimodem/src/lib.rs crates/omnimodem/src/ptt/registry.rs
git commit -m "Add Android PTT JNI dispatch with host-testable stub"
```

---

## Task 10: Android audio JNI bridge (capture ingest + TX sink)

The Rust-side audio bridge: a JNI `modemPushSamples(short[], len)` entry that ingests Kotlin-captured PCM into the existing capture path, and an `AudioBackend`/sink that pushes TX PCM up to Kotlin's `AudioTrack` via a cached callback (Graywolf `android/mod.rs` + `android/audio_tx.rs`). On Android the `AudioBackend` factory returns this JNI-backed backend instead of cpal.

**Files:**
- Modify: `crates/omnimodem/src/android/mod.rs`, add `crates/omnimodem/src/android/audio.rs`
- Modify: `crates/omnimodem/src/lib.rs` (`production_core` audio factory on Android)

- [ ] **Step 1: JNI capture ingest**

Add `Java_<pkg>_ModemBridge_modemPushSamples` that converts the `jshortArray` to `Vec<i16>` and `try_send`s it into the same bounded capture channel a cpal capture would feed (drop-on-backpressure). Behind `#[cfg(target_os="android")]`.

- [ ] **Step 2: JNI TX sink as an `AudioBackend`**

Add an `AndroidBackend` implementing `AudioBackend`: `open_capture` returns a handle whose receiver is fed by `modemPushSamples`; `open_playback` returns a `PlaybackHandle` whose submit pushes to Kotlin via `jni_tx_push_samples(short[], count)` and whose drained watermark tracks submitted (Kotlin `AudioTrack.write(WRITE_BLOCKING)` blocks until drained — Graywolf `audio_tx.rs:17-23`). On Android, `production_core`'s factory returns `AndroidBackend` instead of the cpal factory.

- [ ] **Step 3: Compile gate via cargo-ndk**

Run: `cargo ndk -t arm64-v8a -t x86_64 -P 26 build -p omnimodem --release` (requires the Android NDK + `cargo-ndk`; see Part F).
Expected: produces `libomnimodem.so` per ABI; JNI symbols exported.

- [ ] **Step 4: Document the Kotlin contract (follow-on app)**

In `docs/`, record the JNI method signatures Kotlin must provide (`UsbPttCallback.pttSet(int,boolean)`, `AudioTxCallback.pushSamples(short[],int)`, and the `modem*` external funcs), the method-int constants (CP2102_RTS=1, CM108_HID=2, AIOC_CDC_DTR=3, VOX=4 — mirror Graywolf `ptt_android_consts.rs`), and that Kotlin owns `AudioRecord`/`AudioTrack`/USB-host. Note the app shell (gradle, JNI declarations, USB arbiter) is a separate deliverable.

- [ ] **Step 5: Commit**

```bash
git add crates/omnimodem/src/android/ crates/omnimodem/src/lib.rs docs/
git commit -m "Add Android audio JNI bridge (capture ingest + AudioTrack TX sink)"
```

---

## PART F — Build, cross-compile, CI

## Task 11: Target-gated deps + Cross.toml + cargo-ndk

Lift Graywolf's hard-won build recipe (`Cross.toml`, `.cargo/config.toml`, the pkg-config passthrough) so the matrix actually compiles. omnimodem is pure-Rust (no Go IPC), so this is simpler than Graywolf's hybrid.

**Files:**
- Modify: root `Cargo.toml` (workspace deps for `windows`)
- Modify: `crates/omnimodem/Cargo.toml` (target-gated `windows`; android deps from Task 9)
- Create: `Cross.toml`, `.cargo/config.toml`

- [ ] **Step 1: Add the `windows` workspace dep + target gate**

Root `[workspace.dependencies]`:
```toml
windows = { version = "0.59", features = ["Win32_Devices_Communication", "Win32_Foundation", "Win32_Storage_FileSystem"] }
```
`crates/omnimodem/Cargo.toml`:
```toml
[target.'cfg(windows)'.dependencies]
windows.workspace = true
```

- [ ] **Step 2: `Cross.toml` for Linux arm cross-compiles**

```toml
[target.aarch64-unknown-linux-gnu]
pre-build = [
  "dpkg --add-architecture arm64",
  "apt-get update && apt-get install -y libasound2-dev:arm64 libudev-dev:arm64",
]
[target.aarch64-unknown-linux-gnu.env]
passthrough = ["PKG_CONFIG_ALLOW_CROSS=1"]
```
(Mirror for `armv7-unknown-linux-gnueabihf` if Pi targets are needed; add the protoc install step Graywolf uses in `Cross.toml:2-3`.)

- [ ] **Step 3: Verify the cross-compile matrix**

Run (Linux host with `cross` + docker):
```bash
cross build -p omnimodem --target aarch64-unknown-linux-gnu
cargo build -p omnimodem            # x86_64 linux (native)
```
Expected: both succeed. (macOS/Windows/Android targets build on their respective runners / via cargo-ndk, per Tasks 5–10.)

- [ ] **Step 4: Add a CI matrix note**

Document in the task the target→runner matrix: `x86_64/aarch64-unknown-linux-gnu` (Linux + cross), `x86_64/aarch64-apple-darwin` (macOS runner), `x86_64-pc-windows-msvc` (Windows runner), `aarch64-linux-android`/`x86_64-linux-android` (cargo-ndk). Unit tests run on Linux + the `android-test-stub` feature; per-OS adapters are compile-gated + manually smoke-tested.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/omnimodem/Cargo.toml Cross.toml .cargo/config.toml
git commit -m "Add cross-compile build recipe and target-gated platform deps"
```

---

## Task 12: Exit criterion — cross-platform PTT/audio gate

The gate proving the work. Extends the Phase-2 exit-criterion e2e (`tests/e2e_hardware.rs`) and adds a rigctld e2e that runs on the Linux CI host (so a *real cross-platform PTT method* is exercised end-to-end in CI), plus a documented per-OS manual matrix.

**Files:**
- Create: `crates/omnimodem/tests/rigctld_e2e.rs`
- Modify: `crates/omnimodem/src/lib.rs` (a test-server variant whose `MockOpener` is replaced by `RealOpener` so a `Rigctld` config builds a real `RigctldPtt` against a fake server)

- [ ] **Step 1: Fake-rigctld e2e over the gRPC surface**

Create `tests/rigctld_e2e.rs`: start an in-test fake rigctld TCP server (the Task-3 helper), start the daemon over authorized UDS with a `RealOpener` (rigctld needs no hardware), then over gRPC: `ConfigureChannel` → `ConfigureAudio` (file backend) → `ConfigurePtt { method: RIGCTLD, node: "<fake addr>" }` → `Transmit`. Assert the fake server saw key-then-unkey and the stream emitted `PttState{keyed:true}` then `{false}` + `TransmitStarted`/`Complete`. This proves a portable PTT method works end-to-end with no platform hardware.

- [ ] **Step 2: Run it**

Run: `cargo test -p omnimodem --test rigctld_e2e`
Expected: PASS — full RPC sequence with rigctld keying observed.

- [ ] **Step 3: Document the per-OS manual matrix**

Append to the test file a manual-gate block: for each of {Linux, macOS, Windows} run `ListDevices → ConfigureAudio → ConfigurePtt(serial/cm108/rigctld) → Transmit` against real hardware and confirm key/unkey + audio-out + drain-release; for Android, run the host `android-test-stub` dispatch test + (once the Kotlin shell exists) the on-device APK smoke. State which methods are expected to work per OS (serial: Linux/macOS/Windows; CM108: Linux/macOS/Windows; GPIO: Linux; rigctld: all desktop; Android: Kotlin-selected).

- [ ] **Step 4: Full suite**

Run: `cargo test -p omnimodem && cargo test -p omnimodem --features android-test-stub`
Expected: every test passes on Linux, including the new rigctld e2e and the Android dispatch stub test.

- [ ] **Step 5: Commit**

```bash
git add crates/omnimodem/tests/rigctld_e2e.rs crates/omnimodem/src/lib.rs
git commit -m "Add cross-platform exit-criterion: rigctld e2e + per-OS manual matrix"
```

---

## Self-Review

**1. Spec coverage** (the comment's three asks → tasks):

| Ask | Tasks |
|---|---|
| Understand how Graywolf does cross-platform PTT | Captured in this plan's Part B/C/E (rigctld, Windows serial, macOS/Windows CM108, Android JNI) — each task cites the Graywolf source it lifts |
| Understand how Graywolf does cross-platform audio | Part D (format-matched playback, USB identity) + Part E2 (Android JNI audio), citing `soundcard.rs` / `android/` |
| Translate to omnimodem | Every task targets omnimodem's existing seams (`ModemControlLines`, `Cm108Hid`, `AudioBackend`, `PttDriver`, `DeviceId`) |
| Plan improvements over Graywolf | Six `[IMPROVEMENT]` items, each tied to a task: structured errors (3,5,6), USB-durable identity (8), format-hardened playback (7), all-driver eviction (5,6), no sentinels (3), host-testable JNI (9) |

**2. Platform coverage matrix** (method × OS after this plan):

| Method | Linux | macOS | Windows | Android |
|---|---|---|---|---|
| Serial RTS/DTR | ✅ (Phase 2) | ✅ Task 5 (unix) | ✅ Task 5 | via Kotlin (Task 9) |
| CM108 HID | ✅ (Phase 2) | ✅ Task 6 | ✅ Task 6 | via Kotlin |
| GPIO | ✅ (Phase 2) | — | — | — |
| rigctld | ✅ Task 3 | ✅ Task 3 | ✅ Task 3 | ✅ Task 3 (TCP) |
| Audio capture/playback | ✅ (Phase 2 + Task 7) | ✅ Task 7 | ✅ Task 7 | ✅ Task 10 (JNI) |
| Durable USB identity | ✅ Task 8 | ✅ Task 8 | ✅ Task 8 | Kotlin-owned |

**3. Verification honesty.** Portable logic (rigctld parsing + socket round-trip, format selection, USB identity ranking, Android dispatch via stub) has real unit/integration tests runnable on the Linux CI host. Per-OS hardware adapters are **compile-gated** (`cross`/native runners/`cargo-ndk`) plus a **documented manual hardware gate** — the plan never claims runtime success for an adapter without its manual gate.

**4. Type consistency.** `PttError` (existing) is the error type for every new driver; `ModemControlLines`/`Cm108Hid` (existing seams) are implemented by `WinSerialLines`/`HidApiCm108`; `PttMethod::Rigctld { addr }` matches the proto `PTT_METHOD_RIGCTLD` via `proto_ptt_to_config`; `AudioBackend` (existing) is implemented by `AndroidBackend`; `DeviceId::Usb/Topology/AlsaCard/Placeholder` (existing) are produced by `audio::identity::best_identity`. No new symbol is referenced before it is defined in an earlier task.

**Cross-task ordering:** Task 1–2 (foundation) first. Then Part B (3–4, portable rigctld — highest ROI, fully CI-testable) and Part D (7–8, audio) are independent of Part C (5–6, desktop PTT adapters). Part E (9–10, Android) depends on Task 9's feature scaffold. Part F (11) underpins the compile gates referenced throughout. Task 12 is the gate and depends on Task 3 (rigctld) at minimum.
