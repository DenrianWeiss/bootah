#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
target="${1:?Usage: package_linux_core.sh RUST_TARGET}"
case "$target" in *-unknown-linux-gnu|*-unknown-linux-gnueabihf|*-unknown-linux-musl|*-unknown-linux-musleabihf) ;; *) echo 'Choose a Linux Rust target' >&2;exit 1;; esac
rustup target add "$target"
case "$target" in
    *-musl|*-musleabihf)
        host=$(rustc -vV | sed -n 's/^host: //p')
        linker="$(rustc --print sysroot)/lib/rustlib/$host/bin/rust-lld"
        variable=$(printf '%s' "$target" | tr '[:lower:]-' '[:upper:]_')
        variable="CARGO_TARGET_${variable}_LINKER"
        if [ -z "$(printenv "$variable" || true)" ] && [ -x "$linker" ]; then export "$variable=$linker"; fi
        ;;
esac
cargo build --release --target "$target"
output=".artifacts/linux-$target"
mkdir -p "$output"
cp "target/$target/release/netboot-core" "$output/netboot-core"
cp "target/$target/release/netboot-tui" "$output/netboot-tui"
case "$target" in x86_64-*) tools=linux-x64;; aarch64-*) tools=linux-arm64;; armv7-*) tools=linux-arm;; *) tools=;; esac
sh scripts/copy_package_assets.sh "$output/assets" "$tools"
printf '%s\n' "Run: $output/netboot-tui --config state.json"

cp -R LICENSES "$output/"
mkdir -p release
COPYFILE_DISABLE=1 tar -czf "release/Bootah-core-$target.tar.gz" -C "$output" .
python3 scripts/update_package_checksums.py
