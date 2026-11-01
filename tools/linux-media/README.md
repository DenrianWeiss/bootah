# Bootah Linux media client

MIT-licensed static Linux helper for original installer initrds. The Rust
implementation supports the confirmed targets below; Alpha, PA-RISC and IA64
use the separate [C implementation](../linux-media-c/README.md).

| Asset suffix | Rust target | ABI |
| --- | --- | --- |
| x64 | x86_64-unknown-linux-musl | x86-64 |
| x86 | i586-unknown-linux-musl | IA32, Pentium baseline |
| arm64 | aarch64-unknown-linux-musl | AArch64 |
| powerpc | powerpc-unknown-linux-gnu | 32-bit, big endian |
| ppc64 | powerpc64-unknown-linux-gnu | 64-bit, big endian, ELFv1 |
| ppc64le | powerpc64le-unknown-linux-gnu | 64-bit, little endian, ELFv2 |
| riscv64 | riscv64gc-unknown-linux-musl | RV64GC |
| loongarch64 | loongarch64-unknown-linux-musl | LA64 LP64D, Rust target CPU baseline |
| sparc64 | sparc64-unknown-linux-gnu | SPARC V9 |

Build with `./tools/linux-media/build.sh TARGET`, an asset suffix, or `all`.
Install the target standard library with rustup and fetch locked Cargo
packages before offline builds. musl targets use Rust's bundled linker; GNU
targets require the matching cross GCC and static runtime libraries. An
explicit `CARGO_TARGET_<TRIPLE>_LINKER` overrides the default. On Debian ARM64,
PPC64 BE can use the packaged `ppc64-linker.sh` with the PPC64LE compiler plus
`libc6-dev-ppc64-cross` and matching BE libgcc development files. GNU platform
library notices/sources are retained in the existing `linux-media-sources`
packaging input; the helper's own source remains MIT.

The builder validates ELF class/machine/byte order and static linkage, then
installs the executable into `assets/` and `.artifacts/boot-assets/`, recording
binary/source hashes and backend in `linux-media-clients.json`. Packaging
excludes unapproved media clients. AArch32, s390x, all MIPS, m68k, SPARC32 and
RV32 are excluded from this mapper scope.

`bootah-media nbd SERVER PORT EXPORT /dev/nbdN` negotiates a read-only ISO.
With NBD generic netlink, both CONNECT reply/ACK and sysfs size must match;
the kernel then owns the socket and no userspace worker survives the setup
process. If the kernel explicitly lacks the NBD netlink family, the client
uses read-only legacy ioctl mapping; permission, malformed reply and other
errors do not silently fall back. `--legacy-ioctl` explicitly selects that
path. ioctl values use the target's Linux ABI, capacity is range-checked on
32-bit userspace, and legacy success requires both size and running worker
PID. Load the original ISO's matching NBD module first. Legacy worker
lifecycle details are below.

Protocol: [official NBD specification](https://github.com/NetworkBlockDevice/nbd/blob/master/doc/proto.md),
fixed newstyle with EXPORT_NAME and NO_ZEROES. Linux netlink constants and nesting follow
[include/uapi/linux/nbd-netlink.h](https://github.com/torvalds/linux/blob/master/include/uapi/linux/nbd-netlink.h);
[the upstream client](https://github.com/NetworkBlockDevice/nbd/blob/master/nbd-client.c)
was consulted for the CONNECT contract, without incorporating its GPL source.
This original MIT implementation uses libc and the generic-netlink wire ABI
directly. The kernel performs transmission requests.
Successful compilation does not establish real installer compatibility; verify
with stock firmware and original ISO kernel/initrd on the target architecture.

Debian CD and AOSC LiveKit select the validated original-ISO adapter automatically.
The interfaces no longer expose a manual compatibility-mode switch; the core
API retains `linux_compat` for supported explicit adapters. Trusting an unsigned
local Debian repository remains a separate option.

`python3 scripts/prepare_boot_assets.py --linux-media` optionally builds this
helper. The builder copies it to both `assets/` (development) and
`.artifacts/boot-assets/` (existing packaging inputs); default packaging does
not require a Linux Rust target. Full MIT notices are retained in
`LICENSES/bootah-linux-media.txt`.

2026-10-24 validation: static helper release build and identical packaging copy;
Swift release build; Avalonia release build (0 warnings/errors); Android
`compileDebugKotlin --offline` passed. These are compilation checks, without
GUI or complete installer acceptance. Swift used a workspace module cache;
Avalonia build statistics were disabled (`-p:UsedAvaloniaProducts=`);
Gradle required its normal local daemon socket outside the restricted sandbox.

Legacy ioctl mode only: when `/etc/initrd-release` exists, the background worker
executes the same static helper with kernel-visible argv[0] `@bootah-media`.
Its socket, NBD device and startup-status descriptors remain open across exec.
This follows [systemd's root storage daemon contract](https://systemd.io/ROOT_STORAGE_DAEMONS/)
for the final switch-root kill sweep. The worker's comm remains bootah-media.
The marker does not protect against stopping the service cgroup: a systemd
adapter must start it in its own persistent storage unit, with default
shutdown dependencies disabled and isolation protection, rather than leaving
it inside dracut-initqueue.service. Ordinary non-initrd invocations are unmarked.
The helper recompiles successfully; switch-root continuity must be established
by the fresh Fedora guest run, not inferred from compilation or pre-root mount.


2026-10-24 generic-netlink update: the default path resolves the `nbd` family,
sends one NBD_CONNECT with the selected index, negotiated byte size, 512-byte
blocks, read-only server flags, and one nested socket descriptor. Client flags
are zero, so closing the setup process does not request disconnect. Both the
matching indexed reply and kernel ACK are required. It does not silently fall
back to ioctl. The new helper compiles and both asset copies match; fresh Debian
mount/read and component loading passed with this binary; Fedora switch-root, complete Anaconda installation, and independent disk login without ISO or networking also passed. Complete installation and independent disk boot results are maintained in `docs/debian-cd-compat.md` and `docs/fedora-compatibility.md`, with binary versions distinguished.

For Alpha EV56+ builds, libc requirements and protocol details, see
[the C media client](../linux-media-c/README.md). Rust-capable targets remain
in this directory; the implementation and verification matrix is in `../../docs/linux-iso-compatibility.md`.
