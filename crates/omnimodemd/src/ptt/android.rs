//! Android PTT driver — proxies `key`/`unkey` through to Kotlin's USB PTT
//! actuator via the JNI upcall helpers in `crate::android::upcall`.
//!
//! The `method` field carries one of the PTT method int values below; the
//! Kotlin side interprets it to pick which transport (CP2102N RTS, CM108 HID,
//! AIOC CDC-ACM DTR, VOX) to actuate. A `pttSet` returning `false` (actuator
//! failure) surfaces as `PttError::Io`.
//!
//! Lifted from Graywolf `src/tx/ptt_android.rs` + `ptt_android_consts.rs`,
//! adapted to omnimodem's `PttError`-typed `PttDriver` seam.

#![cfg(any(target_os = "android", feature = "android-test-stub"))]

use super::{PttDriver, PttError};

// ── PTT method int constants ──────────────────────────────────────────────────
//
// Canonical mapping in Rust; mirrors the Kotlin `PttMethodConsts` object and
// the spec's PTT method enum. Keep both sides in sync.

/// CP2102N USB-serial RTS line (Digirig-class adapters).
pub const CP2102N_RTS: i32 = 1;
/// CM108 HID GPIO (wired-GPIO sound-card dongles).
pub const CM108_HID: i32 = 2;
/// AIOC firmware CDC-ACM DTR line.
pub const AIOC_CDC_DTR: i32 = 3;
/// No PTT wire; audio drives VOX.
pub const VOX: i32 = 4;

/// PTT driver that forwards to the Kotlin actuator over JNI.
pub struct AndroidPtt {
    method: i32,
}

impl AndroidPtt {
    pub fn new(method: i32) -> Self {
        Self { method }
    }
}

impl PttDriver for AndroidPtt {
    fn key(&mut self) -> Result<(), PttError> {
        crate::android::upcall::jni_ptt_set(self.method, true).map_err(PttError::Io)
    }

    fn unkey(&mut self) -> Result<(), PttError> {
        crate::android::upcall::jni_ptt_set(self.method, false).map_err(PttError::Io)
    }
}

#[cfg(all(test, not(target_os = "android"), feature = "android-test-stub"))]
mod tests {
    use super::*;
    use crate::android::upcall::{take_recorded, Recorded};
    use serial_test::serial;

    #[test]
    #[serial(android_stub)]
    fn key_then_unkey_records_method_true_then_false() {
        let _ = take_recorded(); // clear any prior state

        let mut ptt = AndroidPtt::new(CP2102N_RTS);
        ptt.key().expect("key forwards through the stub");
        ptt.unkey().expect("unkey forwards through the stub");

        let rec = take_recorded();
        assert_eq!(
            rec,
            vec![
                Recorded::PttSet { method: CP2102N_RTS, keyed: true },
                Recorded::PttSet { method: CP2102N_RTS, keyed: false },
            ],
            "key+unkey must record (method, true) then (method, false)"
        );
    }
}
