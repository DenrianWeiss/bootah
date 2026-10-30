#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
project="$PWD"
rm -f apps/android/app/src/main/jniLibs/*/libnetboot_iso.so apps/android/app/src/main/assets/licenses/7zip.txt
python3 scripts/prepare_boot_assets.py
export ANDROID_NDK_HOME="${ANDROID_NDK_HOME:-$project/.tools/android-ndk-r27c}"
export ANDROID_HOME="${ANDROID_HOME:-$project/.tools/android-sdk}"
export GRADLE_USER_HOME="${GRADLE_USER_HOME:-$project/.tools/gradle-home}"
if [ -z "${JAVA_HOME:-}" ] && [ -d "$project/.tools/jdk17/Contents/Home" ]; then
    export JAVA_HOME="$project/.tools/jdk17/Contents/Home"
fi
case "$(uname -s)" in Darwin) host=darwin-x86_64;; Linux) host=linux-x86_64;; *) echo 'Build on macOS or Linux' >&2;exit 1;; esac
for abi in ${ANDROID_ABIS:-arm64-v8a}; do
    case "$abi" in
        arm64-v8a) target=aarch64-linux-android; clang=aarch64-linux-android;;
        x86_64) target=x86_64-linux-android; clang=x86_64-linux-android;;
        *) echo "Unsupported ABI: $abi" >&2; exit 1;;
    esac
    linker="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/$host/bin/${clang}26-clang"
    [ -x "$linker" ] || { echo "Missing NDK linker: $linker" >&2; exit 1; }
    rustup target add "$target"
    variable=$(printf '%s' "$target" | tr '[:lower:]-' '[:upper:]_')
    env "CARGO_TARGET_${variable}_LINKER=$linker" cargo build --release --target "$target"
    mkdir -p "apps/android/app/src/main/jniLibs/$abi"
    cp "target/$target/release/netboot-core" "apps/android/app/src/main/jniLibs/$abi/libnetboot_core.so"
done
if [ -x "$project/.tools/gradle-8.11.1/bin/gradle" ]; then
    gradle="$project/.tools/gradle-8.11.1/bin/gradle"
else
    gradle=gradle
fi
sh scripts/copy_package_assets.sh apps/android/app/src/main/assets/boot
mkdir -p apps/android/app/src/main/assets/licenses
cp LICENSES/* apps/android/app/src/main/assets/licenses/
"$gradle" -p apps/android assembleDebug --no-daemon
mkdir -p release
cp apps/android/app/build/outputs/apk/debug/app-debug.apk release/Bootah-android-debug.apk
python3 scripts/update_package_checksums.py
