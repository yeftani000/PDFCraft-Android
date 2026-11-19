# PdfCraft for Android

An Android port of [PdfCraft](https://github.com/storytold/pdfcraft), the open-source PDF reader and
editor written in Rust. This repository holds **only the port**; PdfCraft itself is fetched fresh from
upstream every time you build, so each build has upstream's latest features and there is nothing to
merge or keep in sync.

## Get the APK

1. Open the **Actions** tab, choose **Android APK**, and click **Run workflow**.
   * `ref`: the PdfCraft branch, tag or commit to build (default `main`, the latest).
   * `profile`: `release` (small, slower to build) or `debug` (larger, quicker: use while iterating).
2. When the run finishes (about 25 minutes), download **PdfCraft-android** from the run's Artifacts,
   unzip it, and install `PdfCraft.apk` on your device (it is debug-signed, so it sideloads).

The run's summary page records exactly which PdfCraft commit was built.

## What's here

| Path | Purpose |
|---|---|
| `.github/workflows/android.yml` | The build |
| `script/android/` | `apply_port.py` (applies the port to a PdfCraft checkout) and `build_apk.sh` |
| `android/` | The Gradle/Kotlin project that hosts the app on Android |
| `apps/pdfcraft-android/` | The Android entry point: wraps PdfCraft's app with settings, clipboard, typing and "Open with" |
| `crates/craft-android/` | Android runtime: JNI bridge, file picker, clipboard, settings storage, typed-text handling |
| `crates/craft-rfd-android/`, `crates/craft-arboard-android/` | Android stand-ins for the desktop file-dialog and clipboard libraries |

`android/INDEX.md` is the full reference (file map, data flows, JNI contract, where to change what);
`android/README.md` covers behaviour and known gaps.

## Licence

The port's crates declare `MIT OR Apache-2.0`, the same terms as PdfCraft. The APK you build contains
PdfCraft and its dependencies, which carry their own licences and notices (see the PdfCraft repository).
