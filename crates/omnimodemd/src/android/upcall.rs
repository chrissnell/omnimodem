//! JNI upcall helpers — Rust → Kotlin callbacks for PTT and TX audio.
//!
//! **Android runtime path** (`target_os = "android"`): each helper attaches the
//! current thread to the JVM, looks up a cached `GlobalRef` + `JMethodID`,
//! invokes the Kotlin callback, and returns. The callbacks are installed once
//! during `GraywolfService`/app startup via the JNI `installPttCallback` /
//! `installAudioTxCallback` exports (see `android/mod.rs`).
//!
//! **Host stub path** (`feature = "android-test-stub"`, not Android): helpers
//! record every call into a `Mutex<Vec<…>>` so the dispatch can be unit-tested
//! on Linux without a JVM. `take_recorded()` drains the log for assertions.
//!
//! Lifted from Graywolf `src/android/upcall.rs`, adapted to omnimodem's stub
//! shape (a recording `Vec` instead of installable mock closures).

// ── Android runtime impl ─────────────────────────────────────────────────────

#[cfg(target_os = "android")]
mod android_impl {
    use std::sync::{Mutex, OnceLock};

    use jni::objects::{GlobalRef, JMethodID, JObject, JShortArray};
    use jni::JavaVM;
    use log::error;

    // ── PTT callback storage ─────────────────────────────────────────────────

    struct PttCallback {
        obj: GlobalRef,
        method: JMethodID,
    }
    // SAFETY: GlobalRef + JMethodID are valid across threads; we only mutate
    // under the Mutex and never expose raw pointers.
    unsafe impl Send for PttCallback {}

    static PTT_CB: OnceLock<Mutex<Option<PttCallback>>> = OnceLock::new();

    fn ptt_slot() -> &'static Mutex<Option<PttCallback>> {
        PTT_CB.get_or_init(|| Mutex::new(None))
    }

    // ── AudioTx callback storage ─────────────────────────────────────────────

    struct AudioTxCallback {
        obj: GlobalRef,
        method: JMethodID,
    }
    unsafe impl Send for AudioTxCallback {}

    static AUDIO_TX_CB: OnceLock<Mutex<Option<AudioTxCallback>>> = OnceLock::new();

    fn audio_tx_slot() -> &'static Mutex<Option<AudioTxCallback>> {
        AUDIO_TX_CB.get_or_init(|| Mutex::new(None))
    }

    // ── Install helpers (called from JNI exports in mod.rs) ──────────────────

    /// Store the Kotlin `UsbPttCallback` instance + resolved `pttSet(IZ)Z`
    /// method ID. `obj` is promoted to a `GlobalRef` so it survives beyond the
    /// JNI frame. Replaces any prior installation. Errors are logged, not
    /// panicked — crashing the cdylib at startup is worse than a dead callback.
    pub fn install_ptt_callback(env: &mut jni::JNIEnv<'_>, obj: JObject<'_>) {
        let global = match env.new_global_ref(&obj) {
            Ok(g) => g,
            Err(e) => {
                error!("installPttCallback: new_global_ref failed: {e}");
                return;
            }
        };
        // jni 0.21's get_method_id takes a JClass, not a JObject — resolve the
        // class via get_object_class first.
        let class = match env.get_object_class(&obj) {
            Ok(c) => c,
            Err(e) => {
                error!("installPttCallback: get_object_class failed: {e}");
                return;
            }
        };
        let method = match env.get_method_id(&class, "pttSet", "(IZ)Z") {
            Ok(m) => m,
            Err(e) => {
                error!("installPttCallback: get_method_id(pttSet) failed: {e}");
                return;
            }
        };
        *ptt_slot().lock().unwrap() = Some(PttCallback { obj: global, method });
        log::info!("installPttCallback: installed");
    }

    /// Store the Kotlin `AudioTxCallback` instance + resolved `pushSamples([SI)I`
    /// method ID. Replaces any prior installation.
    pub fn install_audio_tx_callback(env: &mut jni::JNIEnv<'_>, obj: JObject<'_>) {
        let global = match env.new_global_ref(&obj) {
            Ok(g) => g,
            Err(e) => {
                error!("installAudioTxCallback: new_global_ref failed: {e}");
                return;
            }
        };
        let class = match env.get_object_class(&obj) {
            Ok(c) => c,
            Err(e) => {
                error!("installAudioTxCallback: get_object_class failed: {e}");
                return;
            }
        };
        let method = match env.get_method_id(&class, "pushSamples", "([SI)I") {
            Ok(m) => m,
            Err(e) => {
                error!("installAudioTxCallback: get_method_id(pushSamples) failed: {e}");
                return;
            }
        };
        *audio_tx_slot().lock().unwrap() = Some(AudioTxCallback { obj: global, method });
        log::info!("installAudioTxCallback: installed");
    }

    // ── Upcall helpers ───────────────────────────────────────────────────────

    fn get_vm() -> Result<JavaVM, String> {
        let ctx = ndk_context::android_context();
        // SAFETY: ndk_context stores the JavaVM pointer installed in JNI_OnLoad.
        unsafe { JavaVM::from_raw(ctx.vm().cast()) }
            .map_err(|e| format!("JavaVM::from_raw: {e}"))
    }

    /// Invoke the installed `UsbPttCallback.pttSet(method, keyed) -> boolean`.
    /// `Ok(())` when Kotlin returned `true`; `Err` when no callback is
    /// installed, the JVM attach/call fails, or Kotlin returned `false`.
    pub fn jni_ptt_set(method: i32, keyed: bool) -> Result<(), String> {
        let vm = get_vm()?;

        // Clone the GlobalRef (Clone) and copy the JMethodID (Copy) under the
        // lock, then drop the lock before the JNI call so a re-entrant upcall
        // path can't deadlock.
        let (callback, method_id) = {
            let slot = ptt_slot().lock().unwrap();
            let cb = slot
                .as_ref()
                .ok_or_else(|| "no PTT callback installed".to_string())?;
            (cb.obj.clone(), cb.method)
        };

        let mut env = vm
            .attach_current_thread()
            .map_err(|e| format!("pttSet: attach_current_thread: {e}"))?;

        let keyed_jni: jni::sys::jboolean = keyed as u8;

        // SAFETY: method ID was resolved against this object's class at install
        // time; the GlobalRef keeps the object alive.
        let result = unsafe {
            env.call_method_unchecked(
                callback.as_obj(),
                method_id,
                jni::signature::ReturnType::Primitive(jni::signature::Primitive::Boolean),
                &[
                    jni::sys::jvalue { i: method },
                    jni::sys::jvalue { z: keyed_jni },
                ],
            )
        }
        .map_err(|e| format!("pttSet JNI call failed: {e}"))?;

        let returned = result
            .z()
            .map_err(|e| format!("pttSet bad return type: {e}"))?;

        if returned {
            Ok(())
        } else {
            Err(format!("pttSet(method={method}, keyed={keyed}) returned false"))
        }
    }

    /// Invoke the installed `AudioTxCallback.pushSamples(samples, count) -> int`.
    /// Allocates a JVM `short[]`, fills it, and calls Kotlin. `Ok(())` on a
    /// non-negative return (matches `AudioTrack.write`: bytes/samples written);
    /// `Err` when no callback is installed, allocation/attach/call fails, or
    /// Kotlin returned a negative error code.
    pub fn jni_tx_push_samples(samples: &[i16]) -> Result<(), String> {
        if samples.is_empty() {
            return Ok(());
        }
        let vm = get_vm()?;

        let (callback, method_id) = {
            let slot = audio_tx_slot().lock().unwrap();
            let cb = slot
                .as_ref()
                .ok_or_else(|| "no AudioTx callback installed".to_string())?;
            (cb.obj.clone(), cb.method)
        };

        let mut env = vm
            .attach_current_thread()
            .map_err(|e| format!("tx_push_samples: attach_current_thread: {e}"))?;

        let arr: JShortArray = env
            .new_short_array(samples.len() as jni::sys::jsize)
            .map_err(|e| format!("tx_push_samples: new_short_array: {e}"))?;
        env.set_short_array_region(&arr, 0, samples)
            .map_err(|e| format!("tx_push_samples: set_short_array_region: {e}"))?;

        let count = samples.len() as i32;

        // SAFETY: method ID and GlobalRef are valid for this callback object.
        let result = unsafe {
            env.call_method_unchecked(
                callback.as_obj(),
                method_id,
                jni::signature::ReturnType::Primitive(jni::signature::Primitive::Int),
                &[
                    jni::sys::jvalue { l: arr.as_raw() as *mut _ },
                    jni::sys::jvalue { i: count },
                ],
            )
        }
        .map_err(|e| format!("tx_push_samples JNI call failed: {e}"))?;

        let n = result
            .i()
            .map_err(|e| format!("tx_push_samples bad return type: {e}"))?;

        if n < 0 {
            Err(format!(
                "AudioTxCallback.pushSamples returned {} for {} samples",
                n,
                samples.len()
            ))
        } else {
            Ok(())
        }
    }
}

// ── Host stub impl (android-test-stub, not Android) ──────────────────────────

#[cfg(all(not(target_os = "android"), feature = "android-test-stub"))]
mod stub_impl {
    use std::sync::Mutex;

    /// One recorded upcall: either a PTT set or a TX push.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Recorded {
        /// `jni_ptt_set(method, keyed)`
        PttSet { method: i32, keyed: bool },
        /// `jni_tx_push_samples(samples)`
        TxPush { samples: Vec<i16> },
    }

    static RECORDED: Mutex<Vec<Recorded>> = Mutex::new(Vec::new());

    /// Drain and return everything recorded since the last call. Test-only
    /// accessor; resets the log so each test sees only its own calls.
    pub fn take_recorded() -> Vec<Recorded> {
        std::mem::take(&mut *RECORDED.lock().unwrap())
    }

    /// Host no-op: there is no JVM, so installing a callback only logs.
    /// Present so call-sites compile identically across configs.
    pub fn install_ptt_callback() {}

    /// Host no-op counterpart to the Android install export.
    pub fn install_audio_tx_callback() {}

    /// Record a PTT set. Always `Ok(())` on the host — the dispatch under test
    /// is "did the driver forward (method, keyed)", not the actuator result.
    pub fn jni_ptt_set(method: i32, keyed: bool) -> Result<(), String> {
        RECORDED
            .lock()
            .unwrap()
            .push(Recorded::PttSet { method, keyed });
        Ok(())
    }

    /// Record a TX push of `samples`.
    pub fn jni_tx_push_samples(samples: &[i16]) -> Result<(), String> {
        RECORDED
            .lock()
            .unwrap()
            .push(Recorded::TxPush { samples: samples.to_vec() });
        Ok(())
    }
}

// ── Public surface — re-export whichever impl is active ──────────────────────

#[cfg(target_os = "android")]
pub use android_impl::{
    install_audio_tx_callback, install_ptt_callback, jni_ptt_set, jni_tx_push_samples,
};

#[cfg(all(not(target_os = "android"), feature = "android-test-stub"))]
pub use stub_impl::{
    install_audio_tx_callback, install_ptt_callback, jni_ptt_set, jni_tx_push_samples,
    take_recorded, Recorded,
};

// ── Unit tests (host stub only) ──────────────────────────────────────────────

#[cfg(all(test, not(target_os = "android"), feature = "android-test-stub"))]
mod tests {
    use super::stub_impl::Recorded;
    use super::{jni_ptt_set, jni_tx_push_samples, take_recorded};
    use serial_test::serial;

    #[test]
    #[serial(android_stub)]
    fn ptt_set_records_method_and_keyed() {
        let _ = take_recorded(); // clear any prior state
        jni_ptt_set(2, true).unwrap();
        let rec = take_recorded();
        assert_eq!(rec, vec![Recorded::PttSet { method: 2, keyed: true }]);
    }

    #[test]
    #[serial(android_stub)]
    fn tx_push_records_the_slice() {
        let _ = take_recorded();
        jni_tx_push_samples(&[10i16, 20, 30]).unwrap();
        let rec = take_recorded();
        assert_eq!(rec, vec![Recorded::TxPush { samples: vec![10, 20, 30] }]);
    }
}
