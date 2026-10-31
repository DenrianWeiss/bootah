#!/bin/sh
# Run on Linux with GRUB >= 2.14 and matching powerpc-ieee1275 modules.
set -eu
here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
mkimage=${GRUB_MKIMAGE:-grub-mkimage}
modules=${MODDIR:?set MODDIR to GRUB >= 2.14 powerpc-ieee1275 modules}
version=$("$mkimage" --version | sed 's/.*) //')
if ! printf '%s\n' "$version" | awk -F. '{ exit !(($1+0)>2 || (($1+0)==2 && ($2+0)>=14)) }'; then
    echo 'GRUB >= 2.14 required (older ofnet corrupts memory under SLOF)' >&2
    exit 1
fi
mkdir -p "$here/../../assets" "$here/../../.artifacts/boot-assets"
"$mkimage" -O powerpc-ieee1275 -d "$modules" \
    -o "$here/../../assets/grub-powerpc.elf" -p '(tftp)/boot/grub' \
    -c "$here/early.cfg" ofnet tftp linux normal configfile echo
cp "$here/../../assets/grub-powerpc.elf" "$here/../../.artifacts/boot-assets/"
