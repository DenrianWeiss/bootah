#!/bin/sh
set -eu
# Debian's ARM64 cross-toolchain packages provide the BE runtime separately.
compiler=${PPC64_LE_CC:-powerpc64le-linux-gnu-gcc}
libc=${PPC64_LIBC:-/usr/powerpc64-linux-gnu/lib}
runtime=${PPC64_GCC_RUNTIME:-$("$compiler" -print-libgcc-file-name | sed 's/powerpc64le/powerpc64/g; s@/libgcc.a$@@')}
if [ ! -f "$libc/crt1.o" ] || [ ! -f "$runtime/libgcc.a" ]; then
    echo "PPC64 BE libc/libgcc missing; install libc6-dev-ppc64-cross and libgcc-dev-ppc64-cross, or select a PPC64 linker explicitly" >&2
    exit 1
fi
exec "$compiler" -mbig-endian -mabi=elfv1 -B"$runtime/" -B"$libc/" -L"$libc" "$@"
