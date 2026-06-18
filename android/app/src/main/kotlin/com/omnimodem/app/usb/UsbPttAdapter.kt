package com.omnimodem.app.usb

import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.hardware.usb.UsbConstants
import android.hardware.usb.UsbDevice
import android.hardware.usb.UsbDeviceConnection
import android.hardware.usb.UsbInterface
import android.hardware.usb.UsbManager
import android.os.Build
import android.util.Log
import com.hoho.android.usbserial.driver.CdcAcmSerialDriver
import com.hoho.android.usbserial.driver.Cp21xxSerialDriver
import com.hoho.android.usbserial.driver.UsbSerialDriver
import com.hoho.android.usbserial.driver.UsbSerialPort
import com.omnimodem.app.jni.UsbPttCallback

/**
 * USB PTT actuator. Implements the Rust-side UsbPttCallback: the modem TX
 * governor calls pttSet(method, keyed) over JNI on every key/unkey. Transports:
 *
 *   CP2102N_RTS (1)  — Digirig: CP2102N USB-serial, key on RTS=true.
 *   CM108_HID   (2)  — CM108-class dongles: 4-byte HID Output Report GPIO write.
 *   AIOC_CDC_DTR(3)  — AIOC firmware >=1.2.0: CDC-ACM, key on DTR=true / RTS=0.
 *   VOX         (4)  — no PTT wire; audio drives VOX. No-op (returns true).
 *
 * Lifecycle:
 *   ModemService.onCreate  -> UsbPttAdapter.init(applicationContext)
 *   MainActivity.onResume  -> UsbPttAdapter.enumerate()  (foreground host so
 *                             requestPermission can surface its dialog)
 *   ModemService.onDestroy -> UsbPttAdapter.closeAll()
 *
 * CM108 invariant: claim ONLY the HID interface, never the audio interfaces —
 * otherwise the kernel snd-usb-audio driver detaches and AudioRecord breaks.
 */
object UsbPttAdapter : UsbPttCallback {
    private const val TAG = "UsbPttAdapter"
    private const val ACTION_USB_PERMISSION = "com.omnimodem.app.USB_PERMISSION"

    // PTT method ints — canonical mapping shared with the Rust core and proto.
    const val PTT_METHOD_UNKNOWN = 0
    const val PTT_METHOD_CP2102N_RTS = 1
    const val PTT_METHOD_CM108_HID = 2
    const val PTT_METHOD_AIOC_CDC_DTR = 3
    const val PTT_METHOD_VOX = 4

    /** Best PTT method for the open/attached USB device, or VOX if none found.
     *  The modem configures PTT with this and actuates it back through pttSet(). */
    fun preferredMethod(): Int {
        if (cp2102n != null) return PTT_METHOD_CP2102N_RTS
        if (aioc != null) return PTT_METHOD_AIOC_CDC_DTR
        if (cm108 != null) return PTT_METHOD_CM108_HID
        if (!::usbManager.isInitialized) return PTT_METHOD_VOX
        for (dev in usbManager.deviceList.values) {
            when (classify(dev)) {
                DeviceRole.CP2102N -> return PTT_METHOD_CP2102N_RTS
                DeviceRole.AIOC -> return PTT_METHOD_AIOC_CDC_DTR
                DeviceRole.CM108 -> return PTT_METHOD_CM108_HID
                DeviceRole.UNKNOWN -> {}
            }
        }
        return PTT_METHOD_VOX
    }

    // Vendor / product IDs.
    private const val CP2102N_VID = 0x10C4
    private const val CP2102N_PID = 0xEA60
    private const val DIGIRIG_CM108_VID = 0x0D8C
    private const val DIGIRIG_CM108_PID = 0x0012
    private const val AIOC_VID = 0x1209
    private const val AIOC_PID = 0x7388

    // CM108 HID GPIO pin (1-indexed; datasheet GPIO3 -> pin 3 -> mask 0x04).
    @Volatile var cm108GpioBit: Int = 3
        private set

    private lateinit var appContext: Context
    private lateinit var usbManager: UsbManager
    private var receiverRegistered = false

    private data class Cp2102nHandle(val device: UsbDevice, val port: UsbSerialPort, val connection: UsbDeviceConnection)
    private data class Cm108Handle(val device: UsbDevice, val connection: UsbDeviceConnection, val hidIface: Int)
    private data class AiocHandle(val device: UsbDevice, val port: UsbSerialPort, val connection: UsbDeviceConnection)

    @Volatile private var cp2102n: Cp2102nHandle? = null
    @Volatile private var cm108: Cm108Handle? = null
    @Volatile private var aioc: AiocHandle? = null

    private val cp2102nLock = Any()
    private val cm108Lock = Any()
    private val aiocLock = Any()

    private val permissionReceiver = object : BroadcastReceiver() {
        override fun onReceive(ctx: Context, intent: Intent) {
            if (intent.action != ACTION_USB_PERMISSION) return
            val device = extraDevice(intent) ?: return
            val granted = intent.getBooleanExtra(UsbManager.EXTRA_PERMISSION_GRANTED, false)
            Log.i(TAG, "permission result device=${device.deviceName} granted=$granted")
            if (granted) tryOpen(device)
        }
    }

    private val hotPlugReceiver = object : BroadcastReceiver() {
        override fun onReceive(ctx: Context, intent: Intent) {
            val device = extraDevice(intent) ?: return
            when (intent.action) {
                UsbManager.ACTION_USB_DEVICE_ATTACHED -> {
                    if (classify(device) == DeviceRole.UNKNOWN) return
                    if (usbManager.hasPermission(device)) tryOpen(device)
                    else requestPermission(device)
                }
                UsbManager.ACTION_USB_DEVICE_DETACHED -> closeForDevice(device)
            }
        }
    }

    private fun extraDevice(intent: Intent): UsbDevice? =
        if (Build.VERSION.SDK_INT >= 33) {
            intent.getParcelableExtra(UsbManager.EXTRA_DEVICE, UsbDevice::class.java)
        } else {
            @Suppress("DEPRECATION")
            intent.getParcelableExtra(UsbManager.EXTRA_DEVICE)
        }

    fun init(ctx: Context) {
        if (this::appContext.isInitialized) return
        appContext = ctx.applicationContext
        usbManager = appContext.getSystemService(Context.USB_SERVICE) as UsbManager
        registerReceiverIfNeeded()
        Log.i(TAG, "init complete")
    }

    private fun registerReceiverIfNeeded() {
        if (receiverRegistered) return
        val permFilter = IntentFilter(ACTION_USB_PERMISSION)
        val hotPlugFilter = IntentFilter().apply {
            addAction(UsbManager.ACTION_USB_DEVICE_ATTACHED)
            addAction(UsbManager.ACTION_USB_DEVICE_DETACHED)
        }
        if (Build.VERSION.SDK_INT >= 33) {
            appContext.registerReceiver(permissionReceiver, permFilter, Context.RECEIVER_NOT_EXPORTED)
            appContext.registerReceiver(hotPlugReceiver, hotPlugFilter, Context.RECEIVER_NOT_EXPORTED)
        } else {
            @Suppress("UnspecifiedRegisterReceiverFlag")
            appContext.registerReceiver(permissionReceiver, permFilter)
            @Suppress("UnspecifiedRegisterReceiverFlag")
            appContext.registerReceiver(hotPlugReceiver, hotPlugFilter)
        }
        receiverRegistered = true
    }

    /**
     * Enumerate attached USB devices and request permission for any recognized
     * PTT-capable device the OS hasn't granted us. Already-permissioned devices
     * are opened directly. Idempotent; drive from a foreground Activity so
     * requestPermission can surface its dialog.
     */
    fun enumerate() {
        check(this::appContext.isInitialized) { "init(context) must be called first" }
        for ((_, dev) in usbManager.deviceList) {
            val role = classify(dev)
            Log.i(TAG, "device ${dev.deviceName} vid=0x${"%04X".format(dev.vendorId)} pid=0x${"%04X".format(dev.productId)} role=$role")
            if (role == DeviceRole.UNKNOWN) continue
            if (usbManager.hasPermission(dev)) tryOpen(dev) else requestPermission(dev)
        }
    }

    fun closeAll() {
        synchronized(cp2102nLock) { cp2102n?.let { runCatching { it.port.close() } }; cp2102n = null }
        synchronized(aiocLock) { aioc?.let { runCatching { it.port.close() } }; aioc = null }
        synchronized(cm108Lock) {
            cm108?.let { h ->
                runCatching {
                    ifaceById(h.device, h.hidIface)?.let { h.connection.releaseInterface(it) }
                    h.connection.close()
                }
            }
            cm108 = null
        }
    }

    private fun closeForDevice(dev: UsbDevice) {
        synchronized(cp2102nLock) {
            cp2102n?.takeIf { it.device.deviceName == dev.deviceName }?.let {
                runCatching { it.port.close() }; cp2102n = null
            }
        }
        synchronized(aiocLock) {
            aioc?.takeIf { it.device.deviceName == dev.deviceName }?.let {
                runCatching { it.port.close() }; aioc = null
            }
        }
        synchronized(cm108Lock) {
            cm108?.takeIf { it.device.deviceName == dev.deviceName }?.let { h ->
                runCatching {
                    ifaceById(h.device, h.hidIface)?.let { h.connection.releaseInterface(it) }
                    h.connection.close()
                }
                cm108 = null
            }
        }
    }

    private fun requestPermission(dev: UsbDevice) {
        val intent = Intent(ACTION_USB_PERMISSION).setPackage(appContext.packageName)
        val flags = if (Build.VERSION.SDK_INT >= 31) {
            PendingIntent.FLAG_MUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
        } else {
            PendingIntent.FLAG_UPDATE_CURRENT
        }
        val pi = PendingIntent.getBroadcast(appContext, 0, intent, flags)
        Log.i(TAG, "requestPermission ${dev.deviceName}")
        usbManager.requestPermission(dev, pi)
    }

    /** Open a permissioned device on a background thread (control transfers can
     *  stall the main thread long enough to ANR). */
    private fun tryOpen(dev: UsbDevice) {
        when (classify(dev)) {
            DeviceRole.CP2102N -> Thread({ openCp2102n(dev) }, "ptt-open-cp2102n").apply { isDaemon = true }.start()
            DeviceRole.CM108 -> Thread({ openCm108(dev) }, "ptt-open-cm108").apply { isDaemon = true }.start()
            DeviceRole.AIOC -> Thread({ openAioc(dev) }, "ptt-open-aioc").apply { isDaemon = true }.start()
            DeviceRole.UNKNOWN -> Log.w(TAG, "tryOpen on UNKNOWN device — skipping")
        }
    }

    private fun openCp2102n(dev: UsbDevice) = synchronized(cp2102nLock) {
        if (cp2102n?.device?.deviceName == dev.deviceName) return@synchronized
        try {
            val driver: UsbSerialDriver = Cp21xxSerialDriver(dev)
            val conn = usbManager.openDevice(dev) ?: return@synchronized
            val port = driver.ports.firstOrNull() ?: run { conn.close(); return@synchronized }
            port.open(conn)
            // Some CP210x variants need setParameters before RTS toggles take effect.
            port.setParameters(9600, 8, UsbSerialPort.STOPBITS_1, UsbSerialPort.PARITY_NONE)
            port.rts = false
            cp2102n = Cp2102nHandle(dev, port, conn)
            Log.i(TAG, "CP2102N opened ${dev.deviceName}")
        } catch (t: Throwable) {
            Log.e(TAG, "openCp2102n failed: $t")
        }
    }

    private fun openAioc(dev: UsbDevice) = synchronized(aiocLock) {
        if (aioc?.device?.deviceName == dev.deviceName) return@synchronized
        try {
            val driver: UsbSerialDriver = CdcAcmSerialDriver(dev)
            val conn = usbManager.openDevice(dev) ?: return@synchronized
            val port = driver.ports.firstOrNull() ?: run { conn.close(); return@synchronized }
            port.open(conn)
            port.setParameters(9600, 8, UsbSerialPort.STOPBITS_1, UsbSerialPort.PARITY_NONE)
            // AIOC firmware >=1.2.0: PTT asserts on DTR=1 AND RTS=0. Pre-set unkeyed.
            port.dtr = false
            port.rts = false
            aioc = AiocHandle(dev, port, conn)
            Log.i(TAG, "AIOC opened ${dev.deviceName}")
        } catch (t: Throwable) {
            Log.e(TAG, "openAioc failed: $t")
        }
    }

    private fun openCm108(dev: UsbDevice) = synchronized(cm108Lock) {
        if (cm108?.device?.deviceName == dev.deviceName) return@synchronized
        val ifaceId = findHidInterface(dev)
        if (ifaceId < 0) { Log.e(TAG, "openCm108: no HID interface on ${dev.deviceName}"); return@synchronized }
        val conn = usbManager.openDevice(dev) ?: return@synchronized
        val iface = ifaceById(dev, ifaceId)!!
        // Invariant: claim ONLY the HID interface so snd-usb-audio stays bound.
        if (!conn.claimInterface(iface, /* force = */ true)) {
            Log.e(TAG, "openCm108: claimInterface($ifaceId) failed"); conn.close(); return@synchronized
        }
        cm108 = Cm108Handle(dev, conn, ifaceId)
        Log.i(TAG, "CM108 opened ${dev.deviceName} hid_iface=$ifaceId")
    }

    /**
     * JNI entry: dispatch a PTT actuation by method int. Returns true on
     * success; the Rust side propagates false as Err back into the TX governor.
     */
    override fun pttSet(method: Int, keyed: Boolean): Boolean = when (method) {
        PTT_METHOD_CP2102N_RTS -> setRts(keyed)
        PTT_METHOD_AIOC_CDC_DTR -> setAiocDtr(keyed)
        PTT_METHOD_CM108_HID -> setHidGpio(keyed)
        PTT_METHOD_VOX -> true
        else -> { Log.w(TAG, "pttSet unknown method=$method"); false }
    }

    private fun setRts(state: Boolean): Boolean = synchronized(cp2102nLock) {
        val h = cp2102n ?: run { Log.w(TAG, "setRts but CP2102N not open"); return@synchronized false }
        try {
            h.port.rts = state
            Log.i(TAG, "ptt: cp2102n_rts=$state")
            true
        } catch (t: Throwable) {
            Log.e(TAG, "setRts failed: $t")
            runCatching { h.port.close() }; cp2102n = null
            false
        }
    }

    private fun setAiocDtr(state: Boolean): Boolean = synchronized(aiocLock) {
        val h = aioc ?: run { Log.w(TAG, "setAiocDtr but AIOC not open"); return@synchronized false }
        try {
            // AIOC firmware >=1.2.0: PTT asserted on DTR=1 AND RTS=0. RTS stays 0.
            h.port.rts = false
            h.port.dtr = state
            Log.i(TAG, "ptt: aioc_cdc dtr=$state rts=0")
            true
        } catch (t: Throwable) {
            Log.e(TAG, "setAiocDtr failed: $t")
            runCatching { h.port.close() }; aioc = null
            false
        }
    }

    private fun setHidGpio(state: Boolean): Boolean = synchronized(cm108Lock) {
        val h = cm108 ?: run { Log.w(TAG, "setHidGpio but CM108 not open"); return@synchronized false }
        // CM108 HID Output Report (matches the Unix/macOS desktop PTT path):
        //   byte 0 = HID_OR0  GPIO write mode (always 0)
        //   byte 1 = HID_OR1  GPIO output values
        //   byte 2 = HID_OR2  GPIO data direction (1=output) — MUST be set or the
        //                     write is silently a no-op even with rc>=0
        //   byte 3 = HID_OR3  SPDIF control (unused)
        // The HID report ID (0) is encoded in wValue (0x0200) of the SET_REPORT
        // control transfer, not the buffer — so the on-wire payload is 4 bytes.
        val pin = cm108GpioBit
        val mask: Byte = (1 shl (pin - 1)).toByte()
        val value: Byte = if (state) mask else 0
        val report = byteArrayOf(0x00, value, mask, 0x00)
        val rc = h.connection.controlTransfer(
            /* requestType = */ 0x21,   // HOST_TO_DEVICE | CLASS | INTERFACE
            /* request     = */ 0x09,   // SET_REPORT
            /* value       = */ 0x0200, // Output report, report id 0
            /* index       = */ h.hidIface,
            /* buffer      = */ report,
            /* length      = */ report.size,
            /* timeout_ms  = */ 200,
        )
        Log.i(TAG, "ptt: cm108_set_report pin=$pin state=$state rc=$rc")
        if (rc < 0) {
            runCatching {
                ifaceById(h.device, h.hidIface)?.let { h.connection.releaseInterface(it) }
                h.connection.close()
            }
            cm108 = null
            return@synchronized false
        }
        rc == report.size
    }

    /** Classify by vid/pid, falling back to a structural fingerprint. */
    private fun classify(dev: UsbDevice): DeviceRole {
        if (dev.vendorId == CP2102N_VID && dev.productId == CP2102N_PID) return DeviceRole.CP2102N
        if (dev.vendorId == AIOC_VID && dev.productId == AIOC_PID) return DeviceRole.AIOC
        if (dev.vendorId == DIGIRIG_CM108_VID && dev.productId == DIGIRIG_CM108_PID) return DeviceRole.CM108
        var hasHid = false; var hasAudio = false; var hasCdc = false
        for (i in 0 until dev.interfaceCount) {
            when (dev.getInterface(i).interfaceClass) {
                UsbConstants.USB_CLASS_HID -> hasHid = true
                UsbConstants.USB_CLASS_AUDIO -> hasAudio = true
                UsbConstants.USB_CLASS_COMM -> hasCdc = true
            }
        }
        return when {
            hasAudio && hasCdc -> DeviceRole.AIOC
            hasAudio && hasHid -> DeviceRole.CM108
            else -> DeviceRole.UNKNOWN
        }
    }

    private fun findHidInterface(dev: UsbDevice): Int {
        for (i in 0 until dev.interfaceCount) {
            val iface = dev.getInterface(i)
            if (iface.interfaceClass == UsbConstants.USB_CLASS_HID) return iface.id
        }
        return -1
    }

    private fun ifaceById(dev: UsbDevice, id: Int): UsbInterface? =
        (0 until dev.interfaceCount).map { dev.getInterface(it) }.firstOrNull { it.id == id }

    enum class DeviceRole { CP2102N, CM108, AIOC, UNKNOWN }
}
