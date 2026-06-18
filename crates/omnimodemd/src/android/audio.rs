//! Kotlin → Rust capture ingest for the Android JNI path.
//!
//! Kotlin owns `android.media.AudioRecord` and pushes each captured PCM chunk
//! down via `modemPushSamples(short[], int len)`. This module converts the JVM
//! `short[]` to a `Vec<i16>` (`AudioChunk`) and `try_send`s it into a
//! process-global capture channel that `AndroidBackend::open_capture` drains.
//! On backpressure (the demod can't keep up) the chunk is dropped rather than
//! blocking Kotlin's high-priority audio thread.
//!
//! Lifted from Graywolf `src/android/audio.rs` + the `modemPushSamples` export
//! in `src/android/mod.rs`, simplified to omnimodem's `AudioBackend` seam: no
//! gain/level-meter machinery here (that lives in the desktop audio pipeline).

#![cfg(target_os = "android")]

use std::sync::mpsc::SyncSender;
use std::sync::OnceLock;

use jni::objects::{JClass, JShortArray};
use jni::sys::jint;
use jni::JNIEnv;
use log::error;

use crate::audio::AudioChunk;

/// Process-global capture sender. `AndroidBackend::open_capture` creates the
/// bounded channel and installs the sender here; the JNI `modemPushSamples`
/// entry reads it to forward captured chunks. One capture stream at a time
/// (the modem opens exactly one input device), so a single global slot is
/// sufficient.
static CAPTURE_TX: OnceLock<SyncSender<AudioChunk>> = OnceLock::new();

/// Install the capture sender. Called once by `open_capture`. Returns the
/// already-installed sender's existence as an error so a second open is a
/// no-op rather than silently shadowing the first (the `OnceLock` can't be
/// reset for the process lifetime — matching the single-stream invariant).
pub fn set_capture_tx(tx: SyncSender<AudioChunk>) -> Result<(), ()> {
    CAPTURE_TX.set(tx).map_err(|_| ())
}

/// Forward a captured chunk into the demod channel. Drops on backpressure.
fn ingest(samples: &[i16]) {
    let Some(tx) = CAPTURE_TX.get() else {
        // No capture stream open: nothing to feed.
        return;
    };
    // try_send: dropping on a full queue beats blocking Kotlin's audio thread.
    let _ = tx.try_send(samples.to_vec());
}

/// JNI capture-ingest entry. Kotlin calls this on every captured PCM chunk.
///
/// `buf` is a borrowed JVM `short[]`; Kotlin retains ownership. We copy the
/// first `len` samples into a `Vec<i16>` and forward it. Symbol must match the
/// Kotlin `external fun modemPushSamples(samples: ShortArray, count: Int)`
/// declared in `com.omnimodem.app.jni.ModemBridge`.
#[no_mangle]
pub extern "system" fn Java_com_omnimodem_app_jni_ModemBridge_modemPushSamples<'local>(
    env: JNIEnv<'local>,
    _class: JClass<'local>,
    buf: JShortArray<'local>,
    len: jint,
) {
    if len <= 0 {
        return;
    }
    let mut scratch = vec![0i16; len as usize];
    if let Err(e) = env.get_short_array_region(&buf, 0, &mut scratch) {
        error!("modemPushSamples: get_short_array_region: {e}");
        return;
    }
    ingest(&scratch);
}
