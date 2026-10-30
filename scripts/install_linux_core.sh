#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
binary="${1:?Usage: sudo sh scripts/install_linux_core.sh /path/to/netboot-core}"
root="${DESTDIR:-}"
install -Dm755 "$binary" "$root/usr/bin/netboot-core"
install -Dm755 "$binary" "$root/usr/libexec/netboot-socket-helper"
install -Dm644 packaging/linux/dev.netboot.network.policy "$root/usr/share/polkit-1/actions/dev.netboot.network.policy"
if [ -d .artifacts/boot-assets ]; then
    mkdir -p "$root/usr/share/netboot/assets"
    cp -R .artifacts/boot-assets/* "$root/usr/share/netboot/assets/"
fi
