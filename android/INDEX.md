# PdfCraft for Android: project index

A map of this repository's Android port: what each file is for, how the pieces connect, and where to
change things. (`README.md` next to this file is the how-to; this is the reference.)

## What this repo is

A fork of `storytold/pdfcraft` that holds **only the Android port**. It is an *overlay*: the build
clones the current upstream PdfCraft, copies these files in, and applies two small edits to
upstream's own files (`script/android/apply_port.py`). Nothing here is a copy of upstream, so there is
nothing to merge. Everything else in the fork (`crates/`, `apps/pdfcraft*`, `vendor/`, ...) is upstream
as of when the fork was made and is **ignored by the Android workflow**.

```
 GitHub Actions: android.yml
   checkout this repo ("port")  +  checkout storytold/pdfcraft@ref ("app")
        copy port files into app  ->  apply_port.py edits 2 upstream files
        cargo ndk build (Rust -> libpdfcraft_android.so)  ->  Gradle (Kotlin + .so -> APK)
```

## Files (the port: 32 files)

| Path | Lines | What it is |
|---|---:|---|
| `.github/workflows/android.yml` | 100 | The build. Inputs: `ref` (upstream branch/tag/commit, default `main`), `profile` (release/debug). Output artifact: `PdfCraft-android` |
| `script/android/apply_port.py` | 173 | Applies the port's edits to a PdfCraft checkout; idempotent; fails with a clear message if upstream's structure changed |
| `script/android/build_apk.sh` | 40 | `cargo ndk` build of `pdfcraft-android`, bundles `libc++_shared.so` if needed, runs Gradle, writes `dist/PdfCraft.apk` |
| `android/` | | The Gradle project (below) |
| `apps/pdfcraft-android/src/lib.rs` | 152 | **The app's entry point**: `android_main` and `AndroidShell`, a wrapper around upstream's `PdfCraftApp` |
| `apps/pdfcraft-android/Cargo.toml` | 17 | `cdylib` named `pdfcraft_android`; depends on `craft-android`, `pdfcraft-ui-egui`, `pdfcraft-engine` |
| `crates/craft-android/` | | The reusable Android runtime (below) |
| `crates/craft-rfd-android/src/lib.rs` | 276 | Android stand-in for `rfd` (file dialogs) |
| `crates/craft-arboard-android/src/lib.rs` | 219 | Android stand-in for `arboard` (clipboard) |

### `android/` (Gradle project)

| Path | Lines | What it is |
|---|---:|---|
| `app/src/main/java/ai/craft/android/MainActivity.kt` | 598 | The Kotlin half: `GameActivity` subclass. Links, document picker, write-back sync, gallery export, clipboard, key capture |
| `app/src/main/AndroidManifest.xml` | 47 | App id, `lib_name`, "Open with" PDF filter, `FileProvider` for clipboard files |
| `app/build.gradle.kts` | 43 | `ai.storyteller.pdfcraft`, versionName 0.21.1, minSdk 29, targetSdk 34, compileSdk 35, arm64-v8a only |
| `app/src/main/res/xml/clip_paths.xml` | 7 | Folders `FileProvider` may share (cache `clip/` and `picks/`, files) |
| `app/src/main/res/values/styles.xml` | 9 | AppCompat theme (dark bars) |
| `app/src/main/res/mipmap-*/ic_launcher.png` | | Launcher icon at five densities |
| `build.gradle.kts`, `settings.gradle.kts`, `gradle.properties` | | AGP 8.7.3, Kotlin 2.0.21, `androidx.games:games-activity:4.4.0` |

### `crates/craft-android/src/` (Android-only crate; empty on other targets)

| File | Lines | What it is | Tests |
|---|---:|---|---:|
| `lib.rs` | 79 | `run()`: sets `HOME`/`TMPDIR`, logging, JNI init, starts the publish watcher and IME setup, launches eframe | |
| `bridge.rs` | 162 | JNI plumbing: attach to the JVM, `open_url`, `publish_media`, `files_dir`, `downloads_dir`, the downloads watcher, the `nativeOnPicked` export | |
| `pick.rs` | 209 | Document picker: keyed mailbox (`request_pick`/`take_picked`) and awaitable `pick_async` | 5 |
| `clipboard.rs` | 175 | Clipboard over JNI: text, HTML, image, file list | 2 |
| `storage.rs` | 150 | `JsonFileStorage`: an `eframe::Storage` in a file (eframe has no storage path on Android) | 4 |
| `text.rs` | 111 | Typed-text glue: JNI exports `nativeOnTypedKey`/`nativeOnTypedText`, IME buffer, `process_raw_input` | |
| `text_core.rs` | 243 | Pure logic: `text_for_key` (with QWERTY fallback), `TextMerger` (de-duplicate routes), `sanitize_ime_text` | 11 |

## How it fits together

```
  Android                Kotlin                         Rust (libpdfcraft_android.so)
  -------                ------                         ------------------------------
  GameActivity  --->  MainActivity  <--- JNI --->  craft-android  <---  AndroidShell (apps/pdfcraft-android)
  (surface, IME,       picker, SAF write-back,        bridge / pick /            |  wraps
   key events)         MediaStore, clipboard,         clipboard / text /         v
                       key capture                    storage                PdfCraftApp (upstream)
                                                                                |  calls rfd:: / arboard::
                                                                                v
                                                     craft-rfd-android, craft-arboard-android
                                                     (aliased over rfd / arboard on Android only)
```

## Data flows

**Opening a file.** App calls `rfd::AsyncFileDialog::pick_file()` -> `craft-rfd-android` ->
`craft_android::pick_async` -> JNI `requestPick` -> system document picker. Kotlin copies the chosen
document to `cache/picks/<nanos>_<name>`, remembers `copy -> original URI`, and calls
`nativeOnPicked(key, paths)`; Rust resolves the awaiting future. "Open with" does the same through
`handleIntent`, posting under `OPEN_KEY`; `AndroidShell::logic` collects it and calls
`PdfCraftApp::open_path`.

**Saving (overwrite the original).** PdfCraft saves in place to the cache copy. Kotlin's
`document-sync` thread (every 2 s) sees the copy changed and stable, then writes it to the original URI
(`openOutputStream(uri, "wt")`). If that fails, it exports the copy to `Downloads/PdfCraft`,
replacing any earlier export of that name, and shows a toast.

**Save As / Export.** `save_file()` returns a fresh name under `files/downloads/`; a Rust watcher
(`bridge.rs`) calls `publishMedia` once the file stops changing -> MediaStore (Pictures, Movies, Music or
Downloads, in a `PdfCraft` folder; replaces an earlier export of the same name).

**Clipboard.** `arboard` calls -> `craft-arboard-android` -> `craft_android::clipboard_*` -> JNI ->
`ClipboardManager` (images and files go through `FileProvider`). egui's own text copy is forwarded by
`AndroidShell::ui` (`OutputCommand::CopyText`/`CopyImage`); a paste event is replaced with the system
text in `raw_input_hook`.

**Typing.** Three routes into egui, merged so each character appears once: winit's own text events;
`MainActivity.dispatchKeyEvent` -> `nativeOnTypedKey` -> `text_for_key` (Android's key map, then without Alt,
then plain QWERTY); and the soft keyboard's IME buffer, polled by `process_raw_input`. The IME is asked
for plain characters (no suggestions or composing).

**Settings.** `AndroidShell` restores from `files/settings.json` at start (`PdfCraftApp::restore`), and saves
(`PdfCraftApp::save` -> `JsonFileStorage`) every 5 s when changed and when focus is lost.

## JNI contract (cross-checked: every call matches a signature)

Rust calls Kotlin (`MainActivity` methods; in descriptors `String` is `Ljava/lang/String;` and `[String` is `[Ljava/lang/String;`, `Z` is boolean, `V` is void):

| Method | Descriptor | Used by |
|---|---|---|
| `openUrl` | `(String)Z` | `open_url` |
| `requestPick` | `(I [String Z)V` | `request_pick`, `pick_async` |
| `publishMedia` | `(String)V` | `publish_media`, downloads watcher |
| `getClipboardText` / `getClipboardHtml` | `()String` | `clipboard_text` / `clipboard_html` |
| `setClipboardText` | `(String)Z` | `set_clipboard_text` |
| `setClipboardHtml` | `(String String)Z` | `set_clipboard_html` |
| `getClipboardImage` | `()[B` (w, h as 4-byte big-endian, then RGBA) | `clipboard_image` |
| `setClipboardImage` | `(I I [B)Z` | `set_clipboard_image` |
| `getClipboardFiles` / `setClipboardFiles` | `()[String` / `([String)Z` | `clipboard_files` / `set_clipboard_files` |

Kotlin calls Rust (exports named `Java_ai_craft_android_MainActivity_*`):

| Export | Posts |
|---|---|
| `nativeOnPicked(key, paths)` | Picker or "Open with" result -> `pick.rs` |
| `nativeOnTypedKey(keyCode, unicode, unicodeNoAlt, shift, caps, ctrlOrMeta)` | A key press -> `text.rs` |
| `nativeOnTypedText(text)` | A multi-character key event -> `text.rs` |

Changing the Kotlin package or class name changes every export's symbol: keep them in step.

## `craft-android` public API

`run(app, title, creator)` · `open_url` · `files_dir` · `downloads_dir` · `publish_media` ·
`request_pick(key, mimes, multiple)` / `take_picked(key)` · `pick_async(mimes, multiple)` / `PickFuture` ·
`clipboard_text` / `set_clipboard_text` · `clipboard_html` / `set_clipboard_html` ·
`clipboard_image` / `set_clipboard_image` (`ClipImage`) · `clipboard_files` / `set_clipboard_files` ·
`JsonFileStorage` (`open`, `get_string`, `set_string`, `remove_string`, `flush`) ·
`process_raw_input(ctx, raw)` · `OPEN_KEY` · `AndroidApp`.

## Keys, paths and constants

| Thing | Value |
|---|---|
| App id / lib | `ai.storyteller.pdfcraft` / `libpdfcraft_android.so` |
| `OPEN_KEY` (Kotlin and Rust, keep equal) | `0x7fff0001` |
| Async picker keys | start at `0x40000000` (`pick.rs`) |
| Settings key and file | `"pdfcraft"` in `files/settings.json` (5 s autosave) |
| Cache copies | `cache/picks/<nanos>_<name>`; clipboard images in `cache/clip/` |
| Original-location memory | SharedPreferences `document-sources` |
| Downloads staging | `files/downloads/`; exports go to `<Pictures/Movies/Music/Download>/PdfCraft` |
| Sync / duplicate windows | write-back every 2 s; text de-duplication window 400 ms |

## Build pipeline

1. `android.yml`: checkout port + upstream; `pip install tomlkit`; copy port files; `apply_port.py`.
2. JDK 17, Rust stable + `aarch64-linux-android`, build tools, `cargo-ndk`, SDK 35 + NDK 27.2.12479018, Gradle 8.9.
3. `build_apk.sh`: `cargo ndk -t arm64-v8a --platform 29 build -p pdfcraft-android --lib [--release]`, then
   `gradle assembleRelease|assembleDebug`, copies to `dist/PdfCraft.apk` (debug-signed, so it sideloads).

`apply_port.py` does exactly this to upstream: (1) in `crates/ui-egui/Cargo.toml`, `rfd` and `arboard` stay for
desktop and web and Android depends on the two stand-ins; (2) in `crates/ui-egui/src/lib.rs`, adds
`#[cfg(target_os = "android")] extern crate craft_rfd_android as rfd;` (and `arboard`); (3) matches `eframe`
and the version to upstream's workspace. Cargo forbids one dependency name with different sources per target,
hence the alias rather than a rename.

## Coupling to upstream (what can break on an upstream change)

| Depends on | Where | If it changes |
|---|---|---|
| Crates `pdfcraft-ui-egui`, `pdfcraft-engine` in the root `[workspace.dependencies]` | `apps/pdfcraft-android` | `apply_port.py` stops with a message |
| `PdfCraftApp::{new, restore, open_path, save, logic, ui, raw_input_hook, on_exit}` | `AndroidShell` | Compile error in `apps/pdfcraft-android` |
| `pdfcraft_engine::sign::init_crypto` | `android_main` | Compile error |
| The subset of `rfd` (`AsyncFileDialog`, `FileHandle`) and `arboard` (`Clipboard`, `ImageData`, `Error`, `Set`, `Get`) the UI uses | the two stand-ins | Compile error in `pdfcraft-ui-egui`: add the missing method to the stand-in |
| `eframe` version | `apply_port.py` matches it | `craft-android` may need adapting to eframe's Android API |

## Verification status

- **CI:** the APK builds (Actions run on commit `138b2c0`: 24 min, `PdfCraft-android` artifact 39.9 MB).
  That covers the NDK compile, linking, Kotlin and Gradle.
- **Host checks (Linux):** 25 unit tests pass (craft-android 22, rfd stand-in 3); `pdfcraft-ui-egui` type-checks as
  Android code against the stand-ins on latest upstream; the entry point type-checks against the real `PdfCraftApp`;
  the JNI contracts above are consistent. Note `cargo test -p craft-android` runs nothing on a desktop (the crate
  is Android-only): its tests were run in a scratch harness.
- **Not verified:** behaviour on a device: typing (hardware and soft keyboard), write-back, clipboard, picker,
  settings persistence, release performance, x86_64/emulator builds (the Gradle project builds arm64-v8a only).

## Known gaps

No printing or update checks; folder pickers return the Downloads folder; no paste menu for touch input in egui
text fields; desktop-style layout (best on tablets and landscape); write-back needs a source that allows writing
(otherwise the export fallback applies); picks that include PDFs ask for write access, which may hide read-only
files in the system picker.

## Where to change things

| To change... | Edit |
|---|---|
| Which upstream edits are applied | `script/android/apply_port.py` |
| Build steps, NDK/SDK versions, inputs | `.github/workflows/android.yml`, `script/android/build_apk.sh` |
| App id, version, SDK levels, ABIs | `android/app/build.gradle.kts` |
| Permissions, "Open with" types | `android/app/src/main/AndroidManifest.xml` |
| Picker, write-back, export, clipboard (Java side) | `MainActivity.kt` |
| Typing rules (what a key types) | `crates/craft-android/src/text_core.rs` |
| What file dialogs / clipboard calls do | the two stand-in crates |
| Startup, settings, copy/paste forwarding, opening documents | `apps/pdfcraft-android/src/lib.rs` |
