# PdfCraft for Android

PdfCraft's egui app (`pdfcraft-ui-egui`) running as `libpdfcraft_android.so` inside a Kotlin
`GameActivity` (`app/src/main/java/ai/craft/android/MainActivity.kt`). Rust and Kotlin talk over JNI
through `crates/craft-android`.

## Build

Run the **Android APK** workflow (Actions tab) and download the `PdfCraft-android` artifact. Each run
builds the *current* `storytold/pdfcraft`: it checks upstream out fresh, copies the port's files in
and applies the port's two small edits to upstream's own files (`script/android/apply_port.py`). To
build something older or a branch, set the `ref` input (a branch, tag or commit).

To build locally, make a PdfCraft checkout, copy `android/`, `apps/pdfcraft-android/`,
`crates/craft-*` and `script/android/` into it, run `python3 script/android/apply_port.py .`, then
`script/android/build_apk.sh` (needs Rust, `cargo install cargo-ndk`, Android SDK 35 + NDK r26+ with
`ANDROID_NDK_HOME`, JDK 17, Gradle 8.9+, cmake/nasm/libclang; `PROFILE=debug` builds faster).

## Staying up to date

Nothing here is a copy of upstream, so there is nothing to merge: run the workflow and you get
upstream's latest. `apply_port.py` re-derives its edits from upstream's files each time, and matches
the port's `eframe` version to upstream's. A build can still fail if upstream starts using more of
`rfd` or `arboard` than the stand-ins cover, or renames the crates the port depends on
(`pdfcraft-ui-egui`, `pdfcraft-engine`); the script and the compiler say exactly what changed.

## How it fits together

* `apps/pdfcraft-android`: `android_main` wraps `PdfCraftApp` (built as the web build builds it) in
  a small shell that restores and saves its settings, forwards copy and paste to the system
  clipboard, and opens documents handed over by "Open with PdfCraft".
* `crates/craft-android`: GameActivity runtime and JNI bridge: links, the document picker as a
  future, the clipboard, a settings file for eframe, and publishing saved files to Downloads.
* `crates/craft-rfd-android`, `crates/craft-arboard-android`: Android stand-ins for `rfd` and
  `arboard`. `crates/ui-egui` uses them on Android only; desktop and web builds are unchanged.

## Behaviour on Android

* **Settings** are kept in the app's private storage (`settings.json`) and restored at launch. They
  are written every 5 seconds when they change and as soon as the app loses focus.
* **Save overwrites the file you opened.** A document you pick (or "Open with") is edited as a copy
  in the app's cache, and a few seconds after you save, the copy is written back to the original
  (the picker asks for write access). If the original can't be changed (a read-only source, or an
  expired one-off grant), the copy is exported to `Downloads/PdfCraft`, replacing the previous export
  of that name, and a message says so. Save As and Export write to `Downloads/PdfCraft` the same way.
* **Clipboard**: text copied in the app goes to the system clipboard; Ctrl+V on a hardware keyboard
  pastes it. The app's image, HTML and file clipboard features work through Android's clipboard
  (file lists only for files inside the app's own folders). Android lets only the foreground app read
  the clipboard. There is no paste menu for touch input in egui text fields; the keyboard's own
  clipboard suggestion works, and PdfCraft's paste buttons do too.

* **Typing.** Characters can reach egui by three routes (winit's key translation, characters Android
  resolves in `MainActivity.dispatchKeyEvent`, and the soft keyboard's IME), and none is reliable on
  every device (on some, winit's lookup only answers while Alt is held). The shell merges the routes
  so each character is typed once; Java retries without Alt and falls back to plain QWERTY. The soft
  keyboard is asked for plain characters (no suggestions or composing).

## Known gaps

Folder pickers return the Downloads folder; no printing or update checks; the UI is the desktop
layout, best on tablets and in landscape. Write-back needs a source that supports writing: where it
doesn't, the export fallback applies.
