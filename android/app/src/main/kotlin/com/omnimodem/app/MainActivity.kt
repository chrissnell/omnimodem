package com.omnimodem.app

import android.Manifest
import android.app.Activity
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import android.util.Log
import android.view.ViewGroup
import android.widget.Button
import android.widget.LinearLayout
import android.widget.TextView
import android.widget.Toast
import com.omnimodem.app.jni.ModemBridge
import com.omnimodem.app.usb.UsbPttAdapter
import kotlin.concurrent.thread
import kotlin.math.sin

/**
 * Minimal host UI. Requests the runtime permissions the modem needs, starts the
 * foreground [ModemService] (which boots the Rust core and self-configures
 * channel 0), and gives the operator Key/Transmit controls that drive the modem
 * in-process through the [ModemBridge] JNI control edge. USB PTT devices are
 * re-enumerated on resume.
 */
class MainActivity : Activity() {
    private lateinit var status: TextView
    private lateinit var keyButton: Button
    private lateinit var txButton: Button
    @Volatile private var keyed = false

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val root = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(48, 96, 48, 48)
        }
        status = TextView(this).apply {
            textSize = 16f
            text = "Omnimodem\n\nStarting modem service..."
        }
        keyButton = Button(this).apply {
            text = "Key PTT"
            setOnClickListener { onKey() }
        }
        txButton = Button(this).apply {
            text = "Transmit test tone"
            setOnClickListener { onTransmit() }
        }
        val lp = LinearLayout.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT
        )
        root.addView(status, lp)
        root.addView(keyButton, lp)
        root.addView(txButton, lp)
        setContentView(root)
        ensurePerms()
    }

    private fun onKey() {
        if (!ModemService.ready) { notReady(); return }
        val next = !keyed
        thread(isDaemon = true) {
            val ok = ModemBridge.modemKeyPtt(ModemService.CHANNEL, next)
            runOnUiThread {
                if (ok) {
                    keyed = next
                    keyButton.text = if (keyed) "Unkey PTT" else "Key PTT"
                } else {
                    Toast.makeText(this, "KeyPtt failed", Toast.LENGTH_SHORT).show()
                }
            }
        }
    }

    private fun onTransmit() {
        if (!ModemService.ready) { notReady(); return }
        val tone = sineTone(rate = 48_000, hz = 1_000, ms = 500)
        thread(isDaemon = true) {
            // modemTransmit blocks for the tone duration; never call on the UI thread.
            val ok = ModemBridge.modemTransmit(ModemService.CHANNEL, tone)
            runOnUiThread {
                Toast.makeText(this, if (ok) "Transmitted" else "Transmit failed", Toast.LENGTH_SHORT).show()
            }
        }
    }

    private fun notReady() =
        Toast.makeText(this, "Modem not ready yet", Toast.LENGTH_SHORT).show()

    /** Mono i16 PCM sine wave. */
    private fun sineTone(rate: Int, hz: Int, ms: Int): ShortArray {
        val n = rate * ms / 1000
        return ShortArray(n) { i ->
            (sin(2.0 * Math.PI * hz * i / rate) * 12000.0).toInt().toShort()
        }
    }

    private fun ensurePerms() {
        val needed = mutableListOf<String>()
        if (checkSelfPermission(Manifest.permission.RECORD_AUDIO) != PackageManager.PERMISSION_GRANTED) {
            needed += Manifest.permission.RECORD_AUDIO
        }
        if (Build.VERSION.SDK_INT >= 33 &&
            checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED) {
            needed += Manifest.permission.POST_NOTIFICATIONS
        }
        if (needed.isNotEmpty()) requestPermissions(needed.toTypedArray(), REQ_PERMS)
        else startModem()
    }

    override fun onRequestPermissionsResult(requestCode: Int, permissions: Array<out String>, grantResults: IntArray) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        if (requestCode == REQ_PERMS) startModem()
    }

    private fun startModem() {
        startForegroundService(Intent(this, ModemService::class.java))
        status.text = "Omnimodem\n\nModem service running.\n" +
            "Connect a USB radio interface, then use Key / Transmit below."
    }

    override fun onResume() {
        super.onResume()
        // Re-enumerate USB on resume: USB_DEVICE_ATTACHED brings us to front on
        // plug-in, and an operator method switch may make a previously-ignored
        // device relevant. Tolerate the adapter not being init'd yet.
        try {
            UsbPttAdapter.enumerate()
        } catch (t: Throwable) {
            Log.w(TAG, "onResume enumerate threw: $t")
        }
    }

    companion object {
        private const val TAG = "MainActivity"
        private const val REQ_PERMS = 0x101
    }
}
