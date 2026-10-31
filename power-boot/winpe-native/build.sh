#!/bin/sh
set -eu
cd "$(dirname "$0")/../.."
mkdir -p .artifacts/boot-assets
"${BOOTAH_MINGW_CC:-x86_64-w64-mingw32-gcc}" -Os -s -municode -static -o .artifacts/boot-assets/bootah-native-x64.exe tools/winpe-native/bootstrap.c -lwinhttp -ladvapi32
