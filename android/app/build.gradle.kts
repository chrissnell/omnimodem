import java.util.Base64

// Version metadata is read from the omnimodemd crate's Cargo.toml so the APK
// stays in lockstep with the Rust core's published version. omnimodem has no
// repo-root VERSION file (unlike Graywolf); the crate manifest is the single
// source of truth. Format: `version = "X.Y.Z"` with an optional `-pre` suffix
// that we drop for versionCode (Android needs a pure integer) but keep for
// versionName.
val omnimodemVersionName: String = run {
    val manifest = rootProject.projectDir.parentFile
        .resolve("crates/omnimodemd/Cargo.toml")
    require(manifest.exists()) { "Cargo.toml not found at ${manifest.absolutePath}" }
    val line = manifest.readLines()
        .map { it.trim() }
        .firstOrNull { it.startsWith("version") && it.contains("=") }
        ?: error("no `version = \"X.Y.Z\"` line in ${manifest.absolutePath}")
    line.substringAfter('"').substringBefore('"')
}
val omnimodemVersionCode: Int = run {
    val core = omnimodemVersionName.substringBefore('-')
    val parts = core.split(".")
    require(parts.size == 3) { "version must be X.Y.Z; got '$omnimodemVersionName'" }
    val (major, minor, patch) = parts.map { it.toInt() }
    major * 1_000_000 + minor * 10_000 + patch * 100
}

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "com.omnimodem.app"
    compileSdk = 36
    buildToolsVersion = "36.0.0"

    defaultConfig {
        applicationId = "com.omnimodem.app"
        minSdk = 28
        targetSdk = 36
        versionCode = omnimodemVersionCode
        versionName = omnimodemVersionName
    }

    sourceSets {
        getByName("main") {
            kotlin.srcDirs("src/main/kotlin")
            jniLibs.srcDirs("src/main/jniLibs")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    kotlinOptions {
        jvmTarget = "17"
    }

    // Release signing reads from env so CI can inject secrets without a
    // committed keystore. Local release builds: export OMNIMODEM_KEYSTORE_PATH
    // + OMNIMODEM_KEYSTORE_PASSWORD before ./gradlew assembleRelease. CI passes
    // OMNIMODEM_KEYSTORE_BASE64 (decoded inline) instead. With no keystore env
    // the release build emits an UNSIGNED APK (debug builds always self-sign).
    val keystorePath = System.getenv("OMNIMODEM_KEYSTORE_PATH")
    val keystoreBase64 = System.getenv("OMNIMODEM_KEYSTORE_BASE64")
    val keystorePassword = System.getenv("OMNIMODEM_KEYSTORE_PASSWORD")
    val keyAlias = System.getenv("OMNIMODEM_KEY_ALIAS") ?: "omnimodem-upload"
    val keyPassword = System.getenv("OMNIMODEM_KEY_PASSWORD") ?: keystorePassword

    val resolvedKeystoreFile: java.io.File? = when {
        keystorePath != null -> file(keystorePath)
        keystoreBase64 != null -> {
            val tmp = layout.buildDirectory.file("upload.keystore").get().asFile
            tmp.parentFile.mkdirs()
            tmp.writeBytes(Base64.getDecoder().decode(keystoreBase64))
            tmp
        }
        else -> null
    }

    signingConfigs {
        if (resolvedKeystoreFile != null && keystorePassword != null) {
            create("release") {
                storeFile = resolvedKeystoreFile
                storePassword = keystorePassword
                this.keyAlias = keyAlias
                this.keyPassword = keyPassword!!
            }
        }
    }

    buildTypes {
        debug {
            isMinifyEnabled = false
        }
        release {
            isMinifyEnabled = false
            signingConfigs.findByName("release")?.let { signingConfig = it }
        }
    }

    buildFeatures {
        // AGP 8 disables BuildConfig by default; ModemService reads
        // BuildConfig.VERSION_NAME for the foreground-service notification.
        buildConfig = true
    }
}

dependencies {
    implementation("androidx.core:core-ktx:1.13.1")
    implementation("androidx.lifecycle:lifecycle-service:2.8.4")
    // Provides the CP2102N (CP21xx) and CDC-ACM USB-serial drivers used by
    // UsbPttAdapter to toggle RTS/DTR for the Digirig and AIOC PTT paths.
    implementation("com.github.mik3y:usb-serial-for-android:3.10.0")
}

// --- Rust cdylib cross-compile via cargo-ndk -----------------------------
//
// Produces libomnimodemd.so for each Android ABI into src/main/jniLibs, which
// the APK packages. The crate's [lib] crate-type MUST include "cdylib" on
// Android (see android/README.md) or cargo-ndk emits nothing to load.
//
// -P 26 sets the Android API platform level passed to the NDK clang (>= minSdk
// 28's floor is fine; 26 is cargo-ndk's conventional baseline). The manifest
// path points back up out of android/app to the workspace crate.
val jniLibsDir = file("src/main/jniLibs")
val repoRoot = rootProject.projectDir.parentFile  // android/.. = repo root

val cargoNdkBuild by tasks.registering(Exec::class) {
    group = "build"
    description = "Cross-compile the omnimodemd cdylib for Android via cargo-ndk."
    // Run from the android/ project dir: both `-o app/src/main/jniLibs` and
    // `--manifest-path ../crates/omnimodemd/Cargo.toml` are resolved relative
    // to it (android/.. = repo root, so ../crates is the workspace crate).
    workingDir = rootProject.projectDir
    // Declare inputs/outputs so Gradle's UP-TO-DATE check skips the cargo-ndk
    // process launch when nothing changed.
    inputs.dir(repoRoot.resolve("crates/omnimodemd/src"))
    inputs.file(repoRoot.resolve("crates/omnimodemd/Cargo.toml"))
    inputs.file(repoRoot.resolve("Cargo.lock"))
    inputs.dir(repoRoot.resolve("proto"))
    outputs.dir(jniLibsDir)
    commandLine = listOf(
        "cargo", "ndk",
        "-t", "arm64-v8a",
        "-t", "x86_64",
        "-p", "26",
        "-o", "app/src/main/jniLibs",
        "build", "--release", "--lib",
        "--manifest-path", "../crates/omnimodemd/Cargo.toml",
    )
}

tasks.named("preBuild") {
    dependsOn(cargoNdkBuild)
}
