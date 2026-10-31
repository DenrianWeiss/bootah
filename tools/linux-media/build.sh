#!/bin/sh
set -eu
cd "$(dirname "$0")"
target=${1:-x86_64-unknown-linux-musl}
case "$target" in
    x86_64-unknown-linux-musl)
        export CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=rust-lld
        suffix=x64 ;;
    powerpc64le-unknown-linux-gnu)
        export CARGO_TARGET_POWERPC64LE_UNKNOWN_LINUX_GNU_LINKER=${CARGO_TARGET_POWERPC64LE_UNKNOWN_LINUX_GNU_LINKER:-powerpc64le-linux-gnu-gcc}
        export RUSTFLAGS="${RUSTFLAGS:-} -C target-feature=+crt-static"
        suffix=ppc64le ;;
    *) echo "Unsupported target: $target" >&2; exit 1 ;;
esac
cargo build --offline --release --target "$target"
mkdir -p ../../assets
cp target/"$target"/release/bootah-media ../../assets/bootah-media-"$suffix"
mkdir -p ../../.artifacts/boot-assets
cp target/"$target"/release/bootah-media ../../.artifacts/boot-assets/bootah-media-"$suffix"
