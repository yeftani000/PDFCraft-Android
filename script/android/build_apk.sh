#!/usr/bin/env bash
# Builds the PdfCraft Android APK: the Rust crate `pdfcraft-android` via cargo-ndk, then the Gradle project
# in android/. Needs Rust + cargo-ndk, Android SDK 35 + NDK (ANDROID_NDK_HOME),
# JDK 17, Gradle 8.9+, and for some crates cmake/nasm/libclang.
#   ABIS="arm64-v8a x86_64" script/android/build_apk.sh      # x86_64 for emulators
set -euo pipefail
cd "$(dirname "$0")/../.."
ROOT="$PWD"
ABIS="${ABIS:-arm64-v8a}"
PROFILE="${PROFILE:-release}"
JNI_LIBS="$ROOT/android/app/src/main/jniLibs"
: "${ANDROID_NDK_HOME:?set ANDROID_NDK_HOME to your NDK}"
export ANDROID_NDK_HOME ANDROID_NDK_ROOT="$ANDROID_NDK_HOME"

triple_for() { case "$1" in arm64-v8a) echo aarch64-linux-android;; armeabi-v7a) echo arm-linux-androideabi;; x86_64) echo x86_64-linux-android;; x86) echo i686-linux-android;; esac; }
rust_target_for() { case "$1" in armeabi-v7a) echo armv7-linux-androideabi;; *) triple_for "$1";; esac; }

ndk_args=()
for abi in $ABIS; do rustup target add "$(rust_target_for "$abi")"; ndk_args+=(-t "$abi"); done
cargo_args=(-p pdfcraft-android --lib)
[ "$PROFILE" = "release" ] && cargo_args+=(--release)

rm -rf "$JNI_LIBS"
cargo ndk "${ndk_args[@]}" --platform 29 -o "$JNI_LIBS" build "${cargo_args[@]}"

for abi in $ABIS; do
  so="$JNI_LIBS/$abi/libpdfcraft_android.so"
  if readelf -d "$so" 2>/dev/null | grep -q 'libc++_shared.so'; then
    stl="$(find "$ANDROID_NDK_HOME/toolchains/llvm/prebuilt" -path "*/sysroot/usr/lib/$(triple_for "$abi")/libc++_shared.so" | head -n1)"
    [ -n "$stl" ] || { echo "libc++_shared.so is needed but wasn't found in the NDK" >&2; exit 1; }
    cp "$stl" "$JNI_LIBS/$abi/" && echo "Bundled libc++_shared.so for $abi"
  fi
done

task=assembleDebug; dir=debug
[ "$PROFILE" = "release" ] && { task=assembleRelease; dir=release; }
if [ -x android/gradlew ]; then (cd android && ./gradlew --no-daemon "$task"); else gradle -p android --no-daemon "$task"; fi
mkdir -p dist
cp "$(find android/app/build/outputs/apk/$dir -name '*.apk' | head -n1)" "dist/PdfCraft.apk"
echo "Built dist/PdfCraft.apk"
