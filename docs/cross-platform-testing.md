# Cross-platform smoke testing

This is a manual, per-platform smoke procedure for the `omnimodemd` daemon and
the Android shell. The flow on every desktop platform is the same control-plane
sequence: **ListDevices -> ConfigureAudio -> ConfigurePtt -> Transmit**, then
confirm the radio keys, audio plays, and the daemon drains and unkeys.

## Getting a per-platform build (CI artifacts)

The **Release** GitHub Actions workflow (`.github/workflows/release.yml`) builds
every platform on each push / tag and uploads the result as a downloadable
artifact. That is the intended way to obtain a build to test:

1. GitHub -> **Actions** -> **Release** -> open the run for your commit.
2. In the run summary's **Artifacts** box, download the one for your platform:

   | Platform                  | Artifact name              | Contents          |
   |---------------------------|----------------------------|-------------------|
   | Linux x86_64              | `omnimodemd-linux-amd64`   | `omnimodemd`      |
   | Linux arm64 (Pi, etc.)    | `omnimodemd-linux-arm64`   | `omnimodemd`      |
   | Windows x86_64            | `omnimodemd-windows-amd64` | `omnimodemd.exe`  |
   | macOS Intel               | `omnimodemd-darwin-amd64`  | `omnimodemd`      |
   | macOS Apple Silicon       | `omnimodemd-darwin-arm64`  | `omnimodemd`      |
   | Android (arm64 + x86_64)  | `omnimodem-debug-apk`      | `app-debug.apk`   |

3. Unzip. On Unix/macOS `chmod +x omnimodemd` first.

## Running the daemon (Linux / macOS / Windows)

`omnimodemd` is environment-driven (no arg parser yet). It creates a runtime
dir, opens a SQLite store, and serves the gRPC `ModemControl` service over a
Unix-domain socket at `<runtime-dir>/omnimodem.sock`.

```sh
# Linux / macOS
OMNIMODEM_RUNTIME_DIR=/tmp/omnimodem RUST_LOG=info ./omnimodemd
```

```powershell
# Windows (PowerShell). UDS is supported on Windows 10 1803+.
$env:OMNIMODEM_RUNTIME_DIR="$env:TEMP\omnimodem"; $env:RUST_LOG="info"; .\omnimodemd.exe
```

The log line `omnimodemd <version> serving socket=<path>` confirms it is up.

### Driving the control plane

There is no bundled CLI client, so use `grpcurl` over the UDS (substitute your
project's gRPC client if one exists). Point it at the socket and the proto in
`proto/omnimodem.proto`:

```sh
SOCK=/tmp/omnimodem/omnimodem.sock
GRPC="grpcurl -plaintext -unix -import-path proto -proto omnimodem.proto"

# 1. ListDevices — confirm your audio interface and PTT device are enumerated.
$GRPC $SOCK omnimodem.v1.ModemControl/ListDevices

# 2. ConfigureAudio — bind channel 0 to your USB dongle's device_id (from step 1).
$GRPC -d '{"channel":0,"device_id":"<DEVICE_ID>","sample_rate":48000}' \
  $SOCK omnimodem.v1.ModemControl/ConfigureAudio

# 3a. ConfigurePtt — serial RTS (Digirig / generic CP210x):
$GRPC -d '{"channel":0,"device_id":"<DEVICE_ID>","method":"PTT_METHOD_SERIAL_RTS","node":"/dev/ttyUSB0"}' \
  $SOCK omnimodem.v1.ModemControl/ConfigurePtt
# 3b. ConfigurePtt — CM108 HID GPIO (pin 3 is the datasheet PTT pin):
$GRPC -d '{"channel":0,"device_id":"<DEVICE_ID>","method":"PTT_METHOD_CM108","node":"/dev/hidrawN","pin_or_line":3}' \
  $SOCK omnimodem.v1.ModemControl/ConfigurePtt
# 3c. rigctld-style external control is configured the same ConfigurePtt call
#     with the method/node your build maps to it.

# 4. Transmit — send a frame on channel 0.
$GRPC -d '{"channel":0, ...}' $SOCK omnimodem.v1.ModemControl/Transmit
```

`omnimodemd`'s `PttMethod` enum (desktop control plane): `PTT_METHOD_SERIAL_RTS`
(3), `PTT_METHOD_SERIAL_DTR` (4), `PTT_METHOD_CM108` (5), `PTT_METHOD_GPIO` (6),
`PTT_METHOD_VOX` (2). (The Android JNI uses a *different*, smaller integer
mapping — see the Android section.)

### What to confirm on each desktop platform

- **Key:** at the start of `Transmit` the radio keys (PTT LED / SWR meter), or
  the configured PTT line toggles (verify with a multimeter on the serial
  RTS/DTR pin or `evtest`/`hidraw` for CM108).
- **Audio:** TX audio plays out of the USB dongle into the radio's mic input;
  on RX, audio captured from the dongle reaches the modem (visible as decode
  activity in the daemon log / `SubscribeEvents`).
- **Drain + release:** when the frame finishes, the daemon drains the audio tail
  and then **unkeys** (PTT releases). Confirm the unkey is not premature (no
  clipped tail) and not stuck keyed.

Platform notes:
- **Linux:** if the PTT device needs permissions, `SuggestUdevRule` returns a
  ready-to-install udev rule. ALSA must see the dongle (`arecord -l`).
- **macOS:** grant the terminal Microphone permission (System Settings ->
  Privacy & Security -> Microphone) or capture is silent.
- **Windows:** UDS works on Win10 1803+. CM108 HID PTT and serial RTS/DTR use
  the OS HID / COM stacks; pick the device by its enumerated `device_id`.

## Android smoke

1. **Sideload** the APK (from the `omnimodem-debug-apk` artifact, or a local
   `./gradlew :app:assembleDebug` — see `android/README.md`):
   ```sh
   adb install -r app-debug.apk
   ```
2. **Launch** the app. Grant **Microphone** (and, on Android 13+,
   **Notifications**) when prompted. The foreground "Omnimodem" notification
   confirms `ModemService` started and `libomnimodemd.so` loaded.
3. **Connect the USB radio interface** (Digirig / AIOC / CM108 dongle). Approve
   the **USB permission** dialog. The app re-enumerates on resume and on
   attach; `UsbPttAdapter` opens the matching transport.
4. **Run the smoke:** trigger a transmit through the in-app control plane (the
   Rust core's gRPC over the in-process UDS). Confirm:
   - the radio **keys** (the Android JNI PTT method ints are CP2102N_RTS=1,
     CM108_HID=2, AIOC_CDC_DTR=3, VOX=4 — the Rust core calls
     `UsbPttCallback.pttSet(method, keyed)`),
   - TX **audio** plays out of the USB dongle (`AudioTxPump` routes to the USB
     audio output automatically),
   - RX **audio** from the dongle's mic input reaches the modem (`AudioPump`),
   - on frame end the radio **unkeys** cleanly (drain-release).
5. Watch the logs while testing:
   ```sh
   adb logcat -s ModemService AudioPump AudioTxPump UsbPttAdapter
   ```

### Android troubleshooting

- **No `.so` / immediate crash on launch:** the crate was not built as a
  `cdylib`. Confirm `crates/omnimodemd/Cargo.toml` has
  `crate-type = ["cdylib","rlib"]` and that
  `app/src/main/jniLibs/{arm64-v8a,x86_64}/libomnimodemd.so` exists after the
  build (`cargoNdkBuild` produces them).
- **PTT never keys:** check the USB permission was granted and the device
  classified to the expected role in the `UsbPttAdapter` log. CM108 requires
  the HID interface to be claimable (audio interfaces are intentionally left to
  the kernel `snd-usb-audio` driver).
- **No TX audio:** confirm a USB audio output exists; `AudioTxPump` logs which
  device it routed to (or "system default" if none was found).
