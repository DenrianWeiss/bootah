#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
case "${1:-all}" in
 macos) sh scripts/package_macos.sh;;
 android) ANDROID_ABIS="${ANDROID_ABIS:-arm64-v8a x86_64}" sh scripts/package_android.sh;;
 win-x64|linux-x64|linux-arm64|linux-arm) sh scripts/package_desktop.sh "$1";;
 core) sh scripts/package_linux_core.sh "${2:?Specify a Linux Rust target}";;
 all)
  status=0
  for platform in macos win-x64 linux-x64 linux-arm64 android; do
   if sh scripts/package.sh "$platform"; then
    printf 'Packaged: %s\n' "$platform"
   else
    printf 'Failed: %s\n' "$platform" >&2; status=1
   fi
  done
  exit "$status";;
 *) echo 'Usage: package.sh all|macos|android|win-x64|linux-x64|linux-arm64|linux-arm|core [RUST_TARGET]' >&2;exit 2;;
esac
