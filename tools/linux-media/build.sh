#!/bin/sh
set -eu
cd "$(dirname "$0")"
target=${1:-x86_64-unknown-linux-musl}
if [ "$target" = all ]; then
    for item in x86_64-unknown-linux-musl i586-unknown-linux-musl aarch64-unknown-linux-musl powerpc-unknown-linux-gnu powerpc64-unknown-linux-gnu powerpc64le-unknown-linux-gnu riscv64gc-unknown-linux-musl loongarch64-unknown-linux-musl sparc64-unknown-linux-gnu; do
        sh ./build.sh "$item"
    done
    exit 0
fi
case "$target" in
    x64|x86_64-unknown-linux-musl) target=x86_64-unknown-linux-musl; suffix=x64 ;;
    x86|ia32|i586-unknown-linux-musl) target=i586-unknown-linux-musl; suffix=x86 ;;
    arm64|aarch64-unknown-linux-musl) target=aarch64-unknown-linux-musl; suffix=arm64 ;;
    powerpc|ppc|powerpc-unknown-linux-gnu) target=powerpc-unknown-linux-gnu; suffix=powerpc; compiler=powerpc-linux-gnu-gcc ;;
    ppc64|powerpc64-unknown-linux-gnu) target=powerpc64-unknown-linux-gnu; suffix=ppc64; compiler=powerpc64-linux-gnu-gcc ;;
    ppc64le|powerpc64le-unknown-linux-gnu) target=powerpc64le-unknown-linux-gnu; suffix=ppc64le; compiler=powerpc64le-linux-gnu-gcc ;;
    riscv64|riscv64gc-unknown-linux-musl) target=riscv64gc-unknown-linux-musl; suffix=riscv64 ;;
    loongarch64|loongarch64-unknown-linux-musl) target=loongarch64-unknown-linux-musl; suffix=loongarch64 ;;
    sparc64|sparc64-unknown-linux-gnu) target=sparc64-unknown-linux-gnu; suffix=sparc64; compiler=sparc64-linux-gnu-gcc ;;
    *) echo "Unsupported Rust media target: $target" >&2; exit 1 ;;
esac
key=CARGO_TARGET_$(printf '%s' "$target" | tr 'a-z-' 'A-Z_')_LINKER
# An explicit cross linker takes precedence; musl uses Rust's bundled linker.
if ! printenv "$key" >/dev/null 2>&1; then
    if [ "$suffix" = ppc64 ] && ! command -v "$compiler" >/dev/null 2>&1; then
        compiler="$PWD/ppc64-linker.sh"
    fi
    export "$key=${compiler:-rust-lld}"
fi
export RUSTFLAGS="${RUSTFLAGS:-} -C target-feature=+crt-static"
cargo build --locked --offline --release --target "$target"
output=${CARGO_TARGET_DIR:-target}/$target/release/bootah-media
python3 ../../scripts/record_linux_media.py "$suffix" "$target" rust "$output"
