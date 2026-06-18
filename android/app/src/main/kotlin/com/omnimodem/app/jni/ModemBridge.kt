package com.omnimodem.app.jni

/**
 * Called by the Rust modem TX governor to actuate the radio's PTT line via the
 * operator-configured USB transport. Implementation lives in UsbPttAdapter;
 * installed once at ModemService.onCreate via ModemBridge.installPttCallback.
 *
 * @param method one of UsbPttAdapter.PTT_METHOD_* (CP2102N_RTS=1, CM108_HID=2,
 *               AIOC_CDC_DTR=3, VOX=4)
 * @param keyed  true to key the radio, false to unkey
 * @return true on success, false to propagate as Err back into Rust
 */
interface UsbPttCallback {
    fun pttSet(method: Int, keyed: Boolean): Boolean
}

/**
 * Called by the Rust modem TX governor on every PCM frame. Implementation lives
 * in AudioTxPump; installed once at ModemService.onCreate via
 * ModemBridge.installAudioTxCallback.
 *
 * Blocking call — the Rust TX thread blocks on AudioTrack.write so the audio
 * buffer drains naturally.
 *
 * @param samples PCM16 mono samples at the modem sample rate
 * @param count   number of samples to consume from the start of `samples`
 */
interface AudioTxCallback {
    /** @return samples accepted (>=0); a negative AudioTrack error code maps to Err in Rust. */
    fun pushSamples(samples: ShortArray, count: Int): Int
}

/**
 * JNI bridge to the omnimodemd Rust core (libomnimodemd.so). The native symbols
 * are exported by the crate as Java_com_omnimodem_app_jni_ModemBridge_* — the
 * package path here is load-bearing and must stay com.omnimodem.app.jni.
 */
object ModemBridge {
    init {
        // Matches the cdylib filename libomnimodemd.so produced by cargo-ndk
        // from crate `omnimodemd` (crate-type must include "cdylib" on Android).
        System.loadLibrary("omnimodemd")
    }

    /** Crate version string, for the foreground-service notification / logs. */
    external fun modemVersion(): String

    /** Start the gRPC/core on the given UDS path. Returns true on success. */
    external fun modemStart(socketPath: String): Boolean

    /** Feed captured mic/USB PCM16 mono samples into the modem RX path. */
    external fun modemPushSamples(samples: ShortArray, len: Int)

    /** Stop the core and release its threads. */
    external fun modemStop()

    /** Register the PTT actuator the Rust TX governor calls on key/unkey. */
    external fun installPttCallback(cb: UsbPttCallback)

    /** Register the TX audio sink the Rust TX governor pushes PCM into. */
    external fun installAudioTxCallback(cb: AudioTxCallback)
}
