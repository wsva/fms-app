# Android Thin-Client Bring-Up

Prerequisites (user action): Android Studio, Android SDK + NDK, JDK 17+.

## 1. Initialize the Android project

```sh
pnpm tauri android doctor
pnpm tauri android init          # generates src-tauri/gen/android
```

## 2. Manifest edits (after init)

In `src-tauri/gen/android/app/src/main/AndroidManifest.xml`, inside `<manifest>`:

```xml
<uses-permission android:name="android.permission.INTERNET" />
<uses-permission android:name="android.permission.ACCESS_NETWORK_STATE" />
<!-- PC discovery: UDP broadcast probe on WLAN -->
<uses-permission android:name="android.permission.CHANGE_WIFI_MULTICAST_STATE" />
<!-- Voice input / local STT: the WebView requests both as soon as JS calls
     getUserMedia(), and Android auto-denies any runtime permission that the
     manifest does not declare — so both entries are mandatory. -->
<uses-permission android:name="android.permission.RECORD_AUDIO" />
<uses-permission android:name="android.permission.MODIFY_AUDIO_SETTINGS" />
<uses-feature android:name="android.hardware.microphone" android:required="false" />
```

And on `<application>`:

```xml
android:usesCleartextTraffic="true"
```

(http:// PC URLs over LAN/Tailscale; switch to Tailscale Serve HTTPS later.)

## 3. Build / run

```sh
pnpm tauri android dev
pnpm tauri android build
```

No extra cargo flags are needed: feature selection is driven by config —
`tauri.conf.json` has `build.features = ["desktop"]`, and
`tauri.android.conf.json` overrides it to `[]` (JSON Merge Patch replaces
arrays), so the Android build compiles without the `desktop` feature while
desktop builds keep it. Cargo's `default = []` is intentionally empty.

Do NOT use `pnpm tauri android build -- --no-default-features`: args after
`--` go to the Gradle runner, not cargo.
