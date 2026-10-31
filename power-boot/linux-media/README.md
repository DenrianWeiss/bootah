# Bootah Linux media client

MIT-licensed static Linux helper for original installer initrds. Build with
`./tools/linux-media/build.sh` (Rust x86_64-unknown-linux-musl target required).
The script installs `assets/bootah-media-x64`; packaging must include that file.
x86-64 and ppc64le are supported. For the Power build and standalone GRUB asset, see `../power-boot/README.md`. No host kernel module is bundled.

`bootah-media nbd SERVER PORT EXPORT /dev/nbd0` negotiates the read-only ISO
export, attaches the socket to the original installer's Linux NBD driver, and
returns after generic-netlink NBD_CONNECT acknowledges the requested device
and sysfs reports the export size. The kernel owns the socket; no userspace
worker must survive switch-root. Load the ISO's matching nbd.ko first.
Unsupported kernel netlink interfaces fail explicitly. For diagnosis only,
append `--legacy-ioctl` to use the older forked NBD_DO_IT worker.

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
