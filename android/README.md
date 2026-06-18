# Omnimodem Android shell

A minimal Kotlin app that hosts the omnimodem Rust core (`libomnimodemd.so`)
directly over JNI. There is no Go subprocess: the app loads the cdylib, captures
mic / USB audio into the modem, plays TX audio, and actuates USB PTT. The Rust
gRPC control plane runs in-process over a Unix-domain socket.

## Layout

```
android/
  settings.gradle.kts        root project (:app)
  build.gradle.kts           plugin versions (AGP 8.7.3, Kotlin 1.9.24)
  gradle.properties
  gradle/wrapper/            Gradle 8.9 wrapper (jar + properties)
  gradlew / gradlew.bat
  app/build.gradle.kts       app module + cargoNdkBuild task (wired into preBuild)
  app/src/main/AndroidManifest.xml
  app/src/main/kotlin/com/omnimodem/app/
    MainActivity.kt          permissions + starts the service
    ModemService.kt          foreground service: installs callbacks, boots core
    jni/ModemBridge.kt        external fun declarations + callback interfaces
    audio/AudioPump.kt        AudioRecord MIC -> modemPushSamples (RX)
    audio/AudioTxPump.kt      AudioTrack <- AudioTxCallback.pushSamples (TX)
    usb/UsbPttAdapter.kt      UsbPttCallback: CP2102N RTS / AIOC DTR / CM108 HID
  app/src/main/res/...        strings, theme, USB device filter
```

## Prerequisites

- JDK 17
- Android SDK with **platform 35**, **build-tools 35.0.0**, **NDK 27** (set
  `ANDROID_HOME` / `ANDROID_NDK_HOME`).
- Rust with the Android targets:
  `rustup target add aarch64-linux-android x86_64-linux-android`
- `cargo-ndk` (v3): `cargo install cargo-ndk --version ^3`
- `protoc` on PATH (the crate's `build.rs` runs tonic-build).

## Required Rust-side change (owned by the Rust task)

`crates/omnimodemd/Cargo.toml` currently declares only a `[[bin]]`. For the
APK to load a `libomnimodemd.so`, the crate must also build a **cdylib**:

```toml
[lib]
name = "omnimodemd"
crate-type = ["cdylib", "rlib"]
path = "src/lib.rs"
```

(`rlib` is kept so the existing integration tests in `tests/` still link the
library.) Without this, `cargo ndk ... build --lib` produces no `.so` and
`System.loadLibrary("omnimodemd")` fails at runtime. The JNI entry points
(`Java_com_omnimodem_app_jni_ModemBridge_*`) and the callback bindings are also
supplied by the Rust task.

## Building the native library

`app/build.gradle.kts` registers a `cargoNdkBuild` task wired into `preBuild`,
so a normal Gradle build cross-compiles the cdylib automatically. The exact
invocation (run from the `android/` directory) is:

```
cargo ndk -t arm64-v8a -t x86_64 -P 26 \
  -o app/src/main/jniLibs \
  build --release --lib \
  --manifest-path ../crates/omnimodemd/Cargo.toml
```

This drops `app/src/main/jniLibs/{arm64-v8a,x86_64}/libomnimodemd.so`, which the
APK packages.

## Building the APK

```
cd android
./gradlew :app:assembleDebug          # unsigned debug APK
```

Output: `app/build/outputs/apk/debug/app-debug.apk`.

Release builds (`assembleRelease`) self-sign only if a keystore is provided via
env (`OMNIMODEM_KEYSTORE_PATH` + `OMNIMODEM_KEYSTORE_PASSWORD`, or
`OMNIMODEM_KEYSTORE_BASE64` in CI); otherwise the release APK is unsigned.

### Gradle wrapper jar

`gradle/wrapper/gradle-wrapper.jar` is committed (the standard Gradle 8.9
wrapper jar). If it is ever missing or you prefer to regenerate it, run once
with a system Gradle:

```
cd android && gradle wrapper --gradle-version 8.9
```

## Sideloading

```
adb install -r app/build/outputs/apk/debug/app-debug.apk
```

On first launch, grant the **microphone** permission (and notifications on
Android 13+). Plug in a USB radio interface (Digirig / AIOC / CM108 dongle) and
approve the USB-permission dialog. The foreground service starts the modem core,
routes audio to/from the USB dongle, and actuates PTT on TX.

You can also download a prebuilt `omnimodem-debug-apk` artifact from the
**Release** GitHub Actions run instead of building locally — see
`docs/cross-platform-testing.md`.
