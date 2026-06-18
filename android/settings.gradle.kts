pluginManagement {
    repositories {
        google()
        mavenCentral()
        gradlePluginPortal()
    }
}
dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
    repositories {
        google()
        mavenCentral()
        // usb-serial-for-android (com.github.mik3y) is published on JitPack.
        maven { url = uri("https://jitpack.io") }
    }
}

rootProject.name = "omnimodem-android"
include(":app")
