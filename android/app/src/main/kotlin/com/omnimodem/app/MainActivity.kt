package com.omnimodem.app

import android.Manifest
import android.app.Activity
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import android.util.Log
import android.widget.TextView
import com.omnimodem.app.usb.UsbPttAdapter

/**
 * Minimal host UI. Requests the runtime permissions the modem needs, starts the
 * foreground ModemService, and re-enumerates USB PTT devices on resume (the
 * Activity-foreground guarantee is what lets requestPermission surface its
 * dialog). The real control plane is the Rust gRPC core over the UDS; this shell
 * just hosts it and the audio/USB I/O.
 */
class MainActivity : Activity() {
    private lateinit var status: TextView

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        status = TextView(this).apply {
            textSize = 16f
            setPadding(48, 96, 48, 48)
            text = "Omnimodem\n\nStarting modem service..."
        }
        setContentView(status)
        ensurePerms()
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
        status.text = "Omnimodem\n\nModem service running.\nConnect a USB radio interface to enable PTT."
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
