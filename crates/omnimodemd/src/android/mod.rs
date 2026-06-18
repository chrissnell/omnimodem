//! Android JNI surface for the omnimodem daemon.
//!
//! On Android, Kotlin owns `AudioRecord` / `AudioTrack` and the USB transport;
//! Rust exposes a thin JNI bridge:
//!   - Kotlin pushes captured PCM down via `modemPushSamples` (see `audio.rs`),
//!     which feeds the capture channel an [`AndroidBackend`] hands out.
//!   - Rust pushes TX PCM and PTT key/unkey up via cached Kotlin callbacks
//!     (see `upcall.rs`), installed once at app startup.
//!
//! Lifted from Graywolf `src/android/mod.rs`, reduced to the modem-crate audio
//! seam (`crate::audio::backend::AudioBackend`); the IPC/demod orchestration
//! that lived in Graywolf's `run_demod` is the daemon's concern, not this
//! bridge's.

#![cfg(target_os = "android")]

use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::sync_channel;
use std::sync::{Arc, OnceLock};

use jni::objects::{JClass, JObject, JShortArray, JString};
use jni::sys::{jboolean, jint, jstring, JNI_VERSION_1_6};
use jni::{JNIEnv, JavaVM};
use log::{error, info};
use tokio::sync::oneshot;

use crate::audio::backend::{AudioBackend, CaptureHandle, PlaybackHandle};
use crate::audio::{AudioChunk, AudioError, CHUNK_QUEUE_DEPTH};
use crate::core::command::Command;
use crate::ids::{ChannelId, DeviceId};

pub mod audio;
pub mod upcall;

// Re-export the JNI capture-ingest symbol at the crate's android module root so
// the cdylib's exported symbol table carries it (the `#[no_mangle]` name is
// what the linker emits; this `use` keeps the path reachable for docs/tests).
pub use audio::Java_com_omnimodem_app_jni_ModemBridge_modemPushSamples;

const LOG_TAG: &str = "omnimodemd";

/// JVM entry point. Initializes `android_logger` and stores the `JavaVM`
/// pointer in `ndk_context` so `upcall` can re-attach worker threads later.
#[no_mangle]
pub extern "system" fn JNI_OnLoad(vm: JavaVM, _reserved: *mut c_void) -> jint {
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Info)
            .with_tag(LOG_TAG),
    );
    info!("JNI_OnLoad: omnimodemd {}", crate::VERSION);
    let raw_vm = vm.get_java_vm_pointer() as *mut c_void;
    // SAFETY: the JavaVM pointer is valid for the process lifetime; the
    // activity context is null because the modem never touches the Activity.
    unsafe {
        ndk_context::initialize_android_context(raw_vm, std::ptr::null_mut());
    }
    JNI_VERSION_1_6
}

/// Guards against booting the core twice across app restarts.
static STARTED: OnceLock<()> = OnceLock::new();

/// The running core's command handle, exposed to the JNI control edge so the
/// Kotlin app can drive the modem in-process (configure / key / transmit)
/// without a gRPC round-trip to its own UDS.
static CORE: OnceLock<crate::core::CoreHandle> = OnceLock::new();

/// `modemVersion()` — the crate version string, for the service notification.
#[no_mangle]
pub extern "system" fn Java_com_omnimodem_app_jni_ModemBridge_modemVersion<'local>(
    env: JNIEnv<'local>,
    _class: JClass<'local>,
) -> jstring {
    env.new_string(crate::VERSION)
        .map(|s| s.into_raw())
        .unwrap_or(std::ptr::null_mut())
}

/// `modemStart(socketPath)` — boot the core + gRPC control plane over a UDS at
/// `socketPath`, using the Android audio backend and Kotlin-actuated PTT. A
/// gRPC client (e.g. the app or an adb-forwarded tool) then drives
/// ConfigureChannel/Audio/Ptt/Transmit. Returns true once the boot thread is
/// launched. Idempotent across restarts.
#[no_mangle]
pub extern "system" fn Java_com_omnimodem_app_jni_ModemBridge_modemStart<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    sock_path: JString<'local>,
) -> jboolean {
    let path: String = match env.get_string(&sock_path) {
        Ok(s) => s.into(),
        Err(e) => {
            error!("modemStart: bad socket path: {e}");
            return 0;
        }
    };
    if STARTED.set(()).is_err() {
        info!("modemStart: core already running");
        return 1;
    }
    let spawned = std::thread::Builder::new()
        .name("omnimodemd-android-core".into())
        .spawn(move || {
            if let Err(e) = run_core(&path) {
                error!("android core exited: {e}");
            }
        });
    if spawned.is_err() {
        return 0;
    }
    info!("modemStart: core thread launched on {} ", crate::VERSION);
    1
}

/// `modemStop()` — best-effort. Android tears the process down when the
/// foreground service stops; a clean in-process shutdown is a follow-on.
#[no_mangle]
pub extern "system" fn Java_com_omnimodem_app_jni_ModemBridge_modemStop<'local>(
    _env: JNIEnv<'local>,
    _class: JClass<'local>,
) {
    info!("modemStop");
}

/// Boot the core and serve the gRPC control plane over `sock_path` (UDS) with
/// the Android audio backend + Kotlin PTT opener.
fn run_core(sock_path: &str) -> Result<(), String> {
    let store = crate::persist::Store::open(std::path::Path::new(&format!("{sock_path}.sqlite")))
        .map_err(|e| e.to_string())?;
    let supervisor =
        crate::supervisor::Supervisor::new(store, Box::new(crate::ptt::registry::RealOpener))
            .map_err(|e| e.to_string())?;
    let enumerator = Box::new(AndroidEnumerator);
    let factory: crate::core::AudioBackendFactory = Box::new(|desc: &crate::device::DeviceDescriptor| {
        Box::new(AndroidBackend::new(desc.id.clone(), crate::audio::MAX_SAMPLE_RATE))
            as Box<dyn AudioBackend>
    });
    let (core, _join) = crate::core::spawn(supervisor, enumerator, factory);
    let _ = CORE.set(core.clone()); // expose to the JNI control edge
    let svc = crate::grpc::ControlService::new(core);
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    rt.block_on(async move {
        crate::authz::serve_uds(svc, std::path::Path::new(sock_path))
            .await
            .map_err(|e| e.to_string())
    })
}

// ── JNI control edge ──────────────────────────────────────────────────────────
//
// The Android app drives the modem in-process by sending the same `core::Command`s
// the gRPC edge sends — no gRPC client / UDS round-trip to its own daemon. Each
// call blocks the calling thread on the core's oneshot reply (short ops; for
// `modemTransmit` the call blocks for the tone's playback duration, so call it
// off the UI thread). Kotlin signatures (class `com.omnimodem.app.jni.ModemBridge`):
//
//   external fun modemListDevices(): String           // "device_id\tlabel" per line
//   external fun modemConfigure(channel: Int, pttMethod: Int): Int  // actual rate, or -1
//   external fun modemKeyPtt(channel: Int, keyed: Boolean): Boolean
//   external fun modemTransmit(channel: Int, samples: ShortArray): Boolean
//
// `pttMethod` is one of `ptt::android` method ints (CP2102N_RTS, CM108_HID, …);
// the modem actuates it back through the Kotlin USB layer via the PTT callback.

/// `modemListDevices()` — newline-joined `device_id<TAB>label`.
#[no_mangle]
pub extern "system" fn Java_com_omnimodem_app_jni_ModemBridge_modemListDevices<'local>(
    env: JNIEnv<'local>,
    _class: JClass<'local>,
) -> jstring {
    let text = match CORE.get() {
        Some(core) => {
            let (tx, rx) = oneshot::channel();
            if core.commands.send(Command::ListDevices { reply: tx }).is_ok() {
                rx.blocking_recv()
                    .map(|devs| {
                        devs.iter()
                            .map(|d| format!("{}\t{}", d.id.to_canonical_string(), d.label))
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .unwrap_or_default()
            } else {
                String::new()
            }
        }
        None => String::new(),
    };
    env.new_string(text).map(|s| s.into_raw()).unwrap_or(std::ptr::null_mut())
}

/// `modemConfigure(channel, pttMethod)` — bind the channel to the Android audio
/// device + an Android PTT actuator. Returns the actual sample rate, or -1.
#[no_mangle]
pub extern "system" fn Java_com_omnimodem_app_jni_ModemBridge_modemConfigure<'local>(
    _env: JNIEnv<'local>,
    _class: JClass<'local>,
    channel: jint,
    ptt_method: jint,
) -> jint {
    configure_channel(channel as u32, ptt_method).unwrap_or(-1)
}

fn configure_channel(ch: u32, ptt_method: i32) -> Option<i32> {
    let core = CORE.get()?;
    let id = ChannelId(ch);

    let (tx, rx) = oneshot::channel();
    core.commands
        .send(Command::ConfigureChannel { id, name: "android".into(), mode: "none".into(), reply: tx })
        .ok()?;
    rx.blocking_recv().ok()?.ok()?;

    // Resolve the single synthetic Android audio device.
    let (tx, rx) = oneshot::channel();
    core.commands.send(Command::ListDevices { reply: tx }).ok()?;
    let dev = rx.blocking_recv().ok()?.into_iter().next()?;

    let (tx, rx) = oneshot::channel();
    core.commands
        .send(Command::ConfigureAudio {
            id,
            device_id: dev.id.clone(),
            sample_rate: crate::audio::MAX_SAMPLE_RATE,
            fanout: 1,
            reply: tx,
        })
        .ok()?;
    let rate = rx.blocking_recv().ok()?.ok()?;

    let ptt = crate::ptt::registry::PttConfig {
        device_id: dev.id,
        method: crate::ptt::registry::PttMethod::Android { method: ptt_method },
        invert: false,
    };
    let (tx, rx) = oneshot::channel();
    core.commands.send(Command::ConfigurePtt { id, ptt, reply: tx }).ok()?;
    rx.blocking_recv().ok()?.ok()?;

    Some(rate as i32)
}

/// `modemKeyPtt(channel, keyed)` — key/unkey the channel's PTT.
#[no_mangle]
pub extern "system" fn Java_com_omnimodem_app_jni_ModemBridge_modemKeyPtt<'local>(
    _env: JNIEnv<'local>,
    _class: JClass<'local>,
    channel: jint,
    keyed: jboolean,
) -> jboolean {
    let ok = (|| {
        let core = CORE.get()?;
        let (tx, rx) = oneshot::channel();
        core.commands
            .send(Command::KeyPtt { channel: ChannelId(channel as u32), keyed: keyed != 0, reply: tx })
            .ok()?;
        rx.blocking_recv().ok()?.ok()?;
        Some(())
    })()
    .is_some();
    ok as jboolean
}

/// `modemTransmit(channel, samples)` — play `samples` (mono i16 PCM) out the
/// channel with PTT asserted for the duration. Blocks for the playback time.
#[no_mangle]
pub extern "system" fn Java_com_omnimodem_app_jni_ModemBridge_modemTransmit<'local>(
    env: JNIEnv<'local>,
    _class: JClass<'local>,
    channel: jint,
    samples: JShortArray<'local>,
) -> jboolean {
    let len = env.get_array_length(&samples).unwrap_or(0);
    if len <= 0 {
        return 0;
    }
    let mut scratch = vec![0i16; len as usize];
    if env.get_short_array_region(&samples, 0, &mut scratch).is_err() {
        return 0;
    }
    let mut payload = Vec::with_capacity(scratch.len() * 2);
    for s in scratch {
        payload.extend_from_slice(&s.to_le_bytes());
    }
    let ok = (|| {
        let core = CORE.get()?;
        let (tx, rx) = oneshot::channel();
        core.commands
            .send(Command::Transmit { channel: ChannelId(channel as u32), payload, reply: tx })
            .ok()?;
        rx.blocking_recv().ok()?.ok()?;
        Some(())
    })()
    .is_some();
    ok as jboolean
}

/// Synthetic enumerator: Android has one logical audio device (Kotlin owns the
/// real `AudioRecord`/`AudioTrack` routing to the USB dongle).
struct AndroidEnumerator;

impl crate::device::DeviceEnumerator for AndroidEnumerator {
    fn enumerate(&self) -> Vec<crate::device::DeviceDescriptor> {
        vec![crate::device::DeviceDescriptor {
            id: DeviceId::Placeholder { tag: "android-audio".into() },
            label: "Android audio (Kotlin-owned)".into(),
            has_capture: true,
            has_playback: true,
        }]
    }
}

// ── JNI install exports ───────────────────────────────────────────────────────
//
// Kotlin calls these once at startup (after System.loadLibrary) to hand the
// Rust modem a live reference to each callback object. Signatures Kotlin must
// match (class `com.omnimodem.app.jni.ModemBridge`):
//
//   external fun installPttCallback(cb: UsbPttCallback)
//     interface UsbPttCallback { fun pttSet(method: Int, keyed: Boolean): Boolean }
//   external fun installAudioTxCallback(cb: AudioTxCallback)
//     interface AudioTxCallback { fun pushSamples(samples: ShortArray, count: Int): Int }

/// Install the Kotlin `UsbPttCallback` implementation. Errors are logged, not
/// panicked. Idempotent across app restarts (replaces any prior installation).
#[no_mangle]
pub extern "system" fn Java_com_omnimodem_app_jni_ModemBridge_installPttCallback<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    callback: JObject<'local>,
) {
    upcall::install_ptt_callback(&mut env, callback);
}

/// Install the Kotlin `AudioTxCallback` implementation. Errors are logged, not
/// panicked.
#[no_mangle]
pub extern "system" fn Java_com_omnimodem_app_jni_ModemBridge_installAudioTxCallback<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    callback: JObject<'local>,
) {
    upcall::install_audio_tx_callback(&mut env, callback);
}

// ── AudioBackend ──────────────────────────────────────────────────────────────

/// The Android audio backend. Capture is fed by the JNI `modemPushSamples`
/// entry; playback routes submitted PCM up to Kotlin's `AudioTrack` via the
/// cached `AudioTxCallback`.
pub struct AndroidBackend {
    id: DeviceId,
    /// Working rate. Kotlin opens `AudioRecord`/`AudioTrack` at this rate; the
    /// modem caps it at `crate::audio::MAX_SAMPLE_RATE` upstream.
    rate: u32,
}

impl AndroidBackend {
    pub fn new(id: DeviceId, rate: u32) -> Self {
        AndroidBackend { id, rate }
    }
}

impl AudioBackend for AndroidBackend {
    /// Open a capture stream. Creates the bounded channel `modemPushSamples`
    /// feeds and installs its sender in `audio::set_capture_tx`. The stop hook
    /// is a no-op: Kotlin owns the `AudioRecord` lifecycle, so tearing down the
    /// Rust side just drops the receiver (no native stream to stop).
    fn open_capture(&self, _requested_rate: u32) -> Result<CaptureHandle, AudioError> {
        let (tx, rx) = sync_channel::<AudioChunk>(CHUNK_QUEUE_DEPTH);
        if audio::set_capture_tx(tx).is_err() {
            return Err(AudioError::Io(
                "android capture already open (single-stream invariant)".to_string(),
            ));
        }
        Ok(CaptureHandle::new(rx, self.rate, || {}))
    }

    /// Open a playback stream. A drain thread pulls submitted chunks and pushes
    /// each up to Kotlin via `upcall::jni_tx_push_samples`. Because
    /// `AudioTrack.write(WRITE_BLOCKING)` blocks until the samples are accepted
    /// into the ring buffer, the chunk is effectively drained by the time the
    /// upcall returns; we therefore advance `drained` by the full chunk length
    /// right after the push, so the TX cycle's `drained >= submitted` watermark
    /// check clears promptly.
    fn open_playback(&self, _requested_rate: u32) -> Result<PlaybackHandle, AudioError> {
        let (tx, rx) = sync_channel::<AudioChunk>(CHUNK_QUEUE_DEPTH);
        let submitted = Arc::new(AtomicUsize::new(0));
        let drained = Arc::new(AtomicUsize::new(0));
        let d2 = drained.clone();
        std::thread::Builder::new()
            .name("omnimodemd-android-tx".into())
            .spawn(move || {
                while let Ok(buf) = rx.recv() {
                    let n = buf.len();
                    if let Err(e) = upcall::jni_tx_push_samples(&buf) {
                        error!("android tx push ({n} samples): {e}");
                    }
                    // Advance the watermark whether or not the push succeeded:
                    // a stuck watermark would hang the TX cycle forever, which
                    // is a worse failure than a dropped output chunk.
                    d2.fetch_add(n, Ordering::Release);
                }
            })
            .map_err(|e| AudioError::Io(format!("spawn android tx thread: {e}")))?;
        Ok(PlaybackHandle::new(tx, submitted, drained, self.rate))
    }

    fn device_id(&self) -> DeviceId {
        self.id.clone()
    }
}
