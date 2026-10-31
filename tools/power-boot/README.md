# Power network boot assets

Build on Linux with GRUB >= 2.14 and the matching `powerpc-ieee1275`
module directory (for example Debian's `grub-ieee1275-bin` package).

```
GRUB_MKIMAGE=grub-mkimage MODDIR=/usr/lib/grub/powerpc-ieee1275 sh tools/power-boot/build.sh
```

The embedded configuration discovers the server using DHCP, then reads the
profile-specific virtual `/boot/grub/grub.cfg` over TFTP. No emulator address
or board device path is compiled into the loader. GRUB 2.12's ofnet heap
corruption under SLOF is avoided by requiring 2.14 or later. Use matching
mkimage and modules; mixing GRUB versions is unsupported.

The separate `grub-powerpc.elf` asset is GPL-3.0-or-later, not linked to Bootah.
Retain GRUB notices and provide the exact corresponding source when shipping.
Validation used Debian GRUB 2.14-4 (`grub2` source package):
https://sources.debian.org/src/grub2/2.14-4/ .
Upstream: https://www.gnu.org/software/grub/ .

For the original-ISO NBD adapter, build the separate MIT helper on Linux:

```
rustup target add powerpc64le-unknown-linux-gnu
# Install the target C compiler and static GNU C libraries first.
sh tools/linux-media/build.sh powerpc64le-unknown-linux-gnu
```

This writes `bootah-media-ppc64le`. It is a 64-bit little-endian static Linux
program, distinct from any old `bootah-media-powerpc` 32-bit big-endian asset.
GNU libc is LGPL-2.1-or-later; retain the runtime notices and corresponding
source/rebuild materials when redistributing the static executable. The
complete helper source and build script are in `tools/linux-media`, and may
be rebuilt with a modified target C library. See Debian `glibc` source for
the selected cross toolchain; validation used Debian glibc 2.41.
Both build scripts copy outputs to `assets/` and `.artifacts/boot-assets/`,
the existing packaging inputs. Missing runtime assets produce an explicit
import error or a firmware-specific warning.
