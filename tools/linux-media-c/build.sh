#!/bin/sh
set -eu
cd "$(dirname "$0")"
target=${1:-alpha-linux-gnu}
case "$target" in
    alpha|alpha-linux-gnu) target=alpha-linux-gnu; suffix=alpha; compiler=alpha-linux-gnu-gcc; cpu=-mcpu=ev56 ;;
    hppa|hppa-linux-gnu) target=hppa-linux-gnu; suffix=hppa; compiler=hppa-linux-gnu-gcc; cpu=-march=1.1 ;;
    ia64|ia64-linux-gnu) target=ia64-linux-gnu; suffix=ia64; compiler=ia64-linux-gnu-gcc; cpu= ;;
    *) echo "Unsupported C media target: $target" >&2; exit 1 ;;
esac
mkdir -p target/"$target"/release ../../assets ../../.artifacts/boot-assets
# CFLAGS/LDFLAGS permit an original-installer sysroot for older libc versions.
# shellcheck disable=SC2086
"${CC:-$compiler}" -std=c11 -Os $cpu -Wall -Wextra -Werror ${CFLAGS:-} main.c ${LDFLAGS:-} -o target/"$target"/release/bootah-media
python3 ../../scripts/record_linux_media.py "$suffix" "$target" c target/"$target"/release/bootah-media
