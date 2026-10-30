#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
python3 scripts/prepare_boot_assets.py
: "${1:?Usage: package_desktop.sh win-x64|linux-x64|linux-arm64|linux-arm}"
case "$1" in
 win-x64) target=x86_64-pc-windows-gnu;extension=.exe;;
 linux-x64) target=x86_64-unknown-linux-musl;extension=;;
 linux-arm) target=armv7-unknown-linux-musleabihf;extension=;;
 linux-arm64) target=aarch64-unknown-linux-musl;extension=;;
 *) echo 'Unsupported runtime' >&2;exit 1;;
esac
rustup target add "$target"
case "$target" in
 *-musl*)
  host=$(rustc -vV | sed -n 's/^host: //p')
  variable=$(printf '%s' "$target" | tr '[:lower:]-' '[:upper:]_')
  export "CARGO_TARGET_${variable}_LINKER=$(rustc --print sysroot)/lib/rustlib/$host/bin/rust-lld";;
esac
cargo build --release --target "$target"
if command -v dotnet >/dev/null 2>&1; then dotnet=dotnet; else dotnet="$PWD/.tools/dotnet/dotnet"; fi
export DOTNET_CLI_HOME="${DOTNET_CLI_HOME:-$PWD/.tools/dotnet-home}"
"$dotnet" publish apps/desktop -c Release -r "$1" --self-contained -o ".artifacts/desktop-$1"
cp "target/$target/release/netboot-core$extension" ".artifacts/desktop-$1/netboot-core$extension"
cp "target/$target/release/netboot-tui$extension" ".artifacts/desktop-$1/netboot-tui$extension"

sh scripts/copy_package_assets.sh ".artifacts/desktop-$1/assets" "$1"

cp assets/branding/bootah.png ".artifacts/desktop-$1/bootah.png"
cp -R LICENSES ".artifacts/desktop-$1/"
mkdir -p release
release="$PWD/release"
case "$1" in
 linux-*)
  mkdir -p ".artifacts/desktop-$1/scripts" ".artifacts/desktop-$1/packaging/linux"
  cp scripts/install_linux_core.sh ".artifacts/desktop-$1/scripts/"
  cp packaging/linux/dev.netboot.network.policy ".artifacts/desktop-$1/packaging/linux/"
  cat > ".artifacts/desktop-$1/work.premutation.bootah.desktop" <<'ENTRY'
[Desktop Entry]
Type=Application
Name=Bootah
Exec=Bootah
Icon=bootah
Categories=System;Network;
Terminal=false
ENTRY
  python3 - ".artifacts/desktop-$1/Bootah.desktop" <<'PY'
from pathlib import Path
import sys
Path(sys.argv[1]).unlink(missing_ok=True)
PY
  COPYFILE_DISABLE=1 tar -czf "$release/Bootah-$1.tar.gz" -C ".artifacts/desktop-$1" .;;
 win-*)
  archive="$release/.Bootah-$1.tmp.zip"
  rm -f "$archive"
  (cd ".artifacts/desktop-$1" && zip -qr "$archive" .)
  mv "$archive" "$release/Bootah-$1.zip";;
esac
python3 scripts/update_package_checksums.py
