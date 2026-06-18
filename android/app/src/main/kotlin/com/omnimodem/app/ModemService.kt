package com.omnimodem.app

import android.Manifest
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.content.pm.PackageManager
import android.content.pm.ServiceInfo
import android.hardware.usb.UsbManager
import android.os.Build
import android.os.IBinder
import android.util.Log
import androidx.core.content.ContextCompat
import com.omnimodem.app.audio.AudioPump
import com.omnimodem.app.audio.AudioTxPump
import com.omnimodem.app.jni.ModemBridge
import com.omnimodem.app.usb.UsbPttAdapter
import java.io.File
import kotlin.concurrent.thread

/**
 * Foreground service that hosts the Rust omnimodemd core. On boot it:
 *   1. installs the PTT + TX-audio JNI callbacks (before starting the core so
 *      any TX/PTT activation on boot finds a registered callback),
 *   2. initializes the USB PTT adapter and starts the TX audio pump,
 *   3. starts the modem core on a UDS, then starts the RX mic pump.
 *
 * All blocking work runs off the main thread so onCreate can't ANR.
 */
class ModemService : Service() {
    private val audioPump = AudioPump()
    private var audioTxPump: AudioTxPump? = null
    @Volatile private var bootThread: Thread? = null
    @Volatile private var stopping = false

    private fun socketPath(): String = File(cacheDir, "omnimodemd.sock").absolutePath

    override fun onCreate() {
        super.onCreate()
        val mgr = getSystemService(NotificationManager::class.java)!!
        mgr.createNotificationChannel(
            NotificationChannel(CHANNEL_ID, "Omnimodem", NotificationManager.IMPORTANCE_LOW)
        )
        val notif: Notification = Notification.Builder(this, CHANNEL_ID)
            .setContentTitle("Omnimodem")
            .setContentText("Modem core running")
            .setSmallIcon(android.R.drawable.stat_sys_data_bluetooth)
            .build()
        startForegroundCompat(notif)

        val v = try { ModemBridge.modemVersion() } catch (t: Throwable) {
            Log.e(TAG, "modemVersion threw: $t"); "ERROR"
        }
        Log.i(TAG, "omnimodemd cdylib version=$v")

        // Install JNI callbacks immediately after loadLibrary (modemVersion
        // triggered it). Must precede modemStart so a boot-time TX/PTT finds them.
        ModemBridge.installPttCallback(UsbPttAdapter)
        val txPump = AudioTxPump(applicationContext)
        audioTxPump = txPump
        ModemBridge.installAudioTxCallback(txPump)

        UsbPttAdapter.init(applicationContext)

        bootThread = thread(start = true, isDaemon = true, name = "omnimodem-boot") {
            if (stopping) return@thread
            // AudioTrack build + USB opens are synchronous HAL calls; keep them
            // off the main thread. Callbacks already installed above tolerate the
            // brief window before this completes.
            txPump.start()
            UsbPttAdapter.enumerate()

            if (!ModemBridge.modemStart(socketPath())) {
                Log.e(TAG, "modemStart returned false")
                stopSelf()
                return@thread
            }
            if (stopping) { ModemBridge.modemStop(); return@thread }
            audioPump.start()

            // Self-drive: bind channel 0 to the Android audio device + the
            // attached USB PTT method, so the app operates the modem on its own.
            val method = UsbPttAdapter.preferredMethod()
            val rate = ModemBridge.modemConfigure(CHANNEL, method)
            if (rate > 0) {
                ready = true
                Log.i(TAG, "modem booted; channel $CHANNEL configured (rate=$rate ptt=$method)")
            } else {
                Log.w(TAG, "modem booted but modemConfigure(channel=$CHANNEL) failed")
            }
        }
    }

    private fun startForegroundCompat(notif: Notification) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            // MICROPHONE pairs with RECORD_AUDIO (granted by MainActivity before
            // launch). MEDIA_PLAYBACK needs no runtime perm. CONNECTED_DEVICE
            // requires at least one already-granted USB device, else
            // startForeground throws — include it only when one exists.
            var fgsType = ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE or
                ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PLAYBACK
            val usb = getSystemService(UsbManager::class.java)
            val hasGrantedUsb = usb?.deviceList?.values?.any { usb.hasPermission(it) } == true
            if (hasGrantedUsb) {
                fgsType = fgsType or ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE
            }
            if (ContextCompat.checkSelfPermission(this, Manifest.permission.RECORD_AUDIO)
                != PackageManager.PERMISSION_GRANTED) {
                // RECORD_AUDIO somehow denied: drop MICROPHONE to avoid SecurityException.
                fgsType = fgsType and ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE.inv()
            }
            startForeground(NOTIF_ID, notif, fgsType)
        } else {
            startForeground(NOTIF_ID, notif)
        }
    }

    override fun onStartCommand(intent: android.content.Intent?, flags: Int, startId: Int): Int = START_STICKY

    override fun onBind(intent: android.content.Intent?): IBinder? = null

    override fun onDestroy() {
        stopping = true
        bootThread?.let { it.interrupt(); it.join(2_500) }
        bootThread = null
        audioPump.stop()
        audioTxPump?.stop()
        audioTxPump = null
        UsbPttAdapter.closeAll()
        ModemBridge.modemStop()
        super.onDestroy()
    }

    companion object {
        private const val TAG = "ModemService"
        private const val CHANNEL_ID = "omnimodem-foreground"
        private const val NOTIF_ID = 0x6F4D

        /** Channel the service self-configures and the UI drives. */
        const val CHANNEL = 0

        /** True once the core is booted and channel 0 is configured (audio+PTT
         *  bound). The UI gates Key/Transmit on this. */
        @Volatile var ready = false
            private set
    }
}
