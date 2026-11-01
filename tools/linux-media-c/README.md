# Bootah C Linux media client

Original MIT implementation for Linux architectures where the Rust media
client cannot be built with a usable standard library/toolchain. Rust-capable
targets use `../linux-media`; this directory builds Alpha EV56+, PA-RISC (32-bit userspace on 32/64-bit kernels), and IA64.

Build with `./tools/linux-media-c/build.sh alpha`, `hppa` or `ia64`, or the
corresponding `*-linux-gnu` triple. `CC` selects the cross compiler;
`CFLAGS` and `LDFLAGS` can select an original-installer sysroot. The builder
validates ELF identity and installs `bootah-media-ARCH` in `assets/` and
`.artifacts/boot-assets/`, recording source/binary hashes and backend.
Packaging must include the generated asset. Alternatively use
`python3 scripts/prepare_boot_assets.py --linux-media-alpha`.

The binaries use the original installer's libc. The current Alpha/PA-RISC
cross builds require glibc 2.34+; original tested media supply 2.36 and 2.41.
IA64 is built against Debian 7 libc 2.13 and Linux 3.2 headers. That installer's
libc is reduced: the client uses numeric IPv4 directly, without DNS/NSS APIs.
No host module or runtime library is injected. Both byte orders and native
netlink integer widths are handled.

The tested IA64 compiler is kernel.org's ARM64 GCC 12.3 nolibc cross toolchain.
Provide an IA64 sysroot extracted from `libc6.1-dev`, `libc6.1` and
`linux-libc-dev` packages. For that compiler, in addition to `--sysroot=DIR`,
set `CFLAGS` to include `-isystem DIR/usr/include/ia64-linux-gnu -isystem DIR/usr/include`
and `LDFLAGS` to include `-BDIR/usr/lib/ia64-linux-gnu/ -LDIR/usr/lib/ia64-linux-gnu -LDIR/lib/ia64-linux-gnu`.
A complete libc cross compiler can use its own sysroot normally.

`bootah-media nbd SERVER PORT EXPORT /dev/nbdN` negotiates a read-only,
sector-aligned fixed-newstyle export with EXPORT_NAME and NO_ZEROES, then
attaches its socket with Linux generic-netlink NBD_CONNECT. It requires the
matching original ISO NBD module. Both the indexed reply and kernel ACK, then
the negotiated sysfs size, must be observed before success. The kernel retains
the socket after exit. A kernel that explicitly lacks the NBD netlink family
uses the legacy ioctl path automatically; `--legacy-ioctl` can force it.
That path sets the block device read-only and requires a live worker PID and
matching capacity before success. It uses target UAPI ioctl definitions and
checks 32-bit size limits. The worker is marked `@bootah-media` in a systemd
initrd; its storage unit must also survive switch-root. Other netlink errors
remain errors. Legacy ioctl was verified on the original IA64 Linux 3.2 kernel.

References: [official NBD protocol](https://github.com/NetworkBlockDevice/nbd/blob/master/doc/proto.md)
and [Linux NBD netlink UAPI](https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/tree/include/uapi/linux/nbd-netlink.h?h=v6.1).
The implementation includes Linux UAPI headers and uses libc; it incorporates
no GPL client implementation. License: `../../LICENSES/bootah-linux-media.txt`.

Original ES40 SRM / AXPbox validation reached read-only ISO mount, installer
component loading 100%, successful DHCP reconfiguration and disk detection.
The user accepted Debian Alpha at this stage; complete disk installation and
independent disk boot were not established. Evidence and platform limits:
`../../docs/alpha-srm.md`, `.artifacts/alpha-nbd/`.

2026-11-01 mapping checks also passed on original PA-RISC 6.12 kernels in
B160L and C3700 QEMU models, with the same 32-bit binary on both kernel widths.
The test mounted a 5 GiB sparse ISO, verified read hashes after setup exited,
and confirmed write rejection. IA64 used stock emulator firmware, local
FAT/ELILO, original Debian kernel/initrd and test-only Linux PCI resource
alignment/rescan to initialize its NIC. These are mapping tests, not firmware
network-boot acceptance. Evidence: `.artifacts/linux-media-confirmed/`.
