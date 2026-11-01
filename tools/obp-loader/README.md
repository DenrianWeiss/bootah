# Bootah OpenBoot loader

Experimental SPARC64 Linux/BSD second stage for original OpenBoot (sun4u/sun4v).
The firmware obtains an address using RARP and fetches the Sun a.out
`bootah-obp.bin` using TFTP. The loader uses CIF network read/write, obtains a
DHCP lease and DHCP/ProxyDHCP boot target, downloads a SPARC64 GRUB a.out image
for Linux or an original ELF64 `ofwboot.net` for BSD, and publishes network
context before entering the target with CIF in `%o4`. BSD gets an explicit
`netbsd` or `bsd.rd` boot argument through a bounded `bootah-obp.cfg` manifest.

Build on Linux with `gcc-sparc64-linux-gnu`, `binutils-sparc64-linux-gnu`, make
and Python 3:

```sh
make -C tools/obp-loader
make -C tools/obp-loader install
make -C tools/obp-loader check
```

The production a.out entry is `0x4000`. Its small bootstrap claims memory
through CIF, copies the linked body to `0x1000000`, synchronizes instructions
and enters it with the original CIF/firmware stack. Initialized BSS includes
the private stack and buffers. GRUB is claimed at `0x4000` (8 MiB), downloaded into the upper
part of that reservation and validated before copying its payload to `0x4400`.
The packaged GRUB 2.12 has `_end=0x270f8`; its modules, 256 KiB stack and 2 MiB
heap fit below the transfer buffer at `0x600000`. Other GRUB builds need their
runtime layout checked separately, even if the a.out header is accepted.

Before transfer, a low-memory trampoline switches back to the firmware stack
and releases the resident body at 16 MiB, leaving that address available for
the BSD kernel. Native ELF load segments must be within 0x4000–0x500000; the
trampoline and staging buffer sit outside that range. Published OBP properties
are read back and verified. GRUB receives a canonical device:server,file
bootpath and complete BOOTP context; BSD also receives client/network fields.

The wire identity is option 60 `PXEClient` plus RFC 3004 option 77
`Bootah-OBP/1`. No option 93 is sent: number 2 is Itanium, not SPARC. This
private user class selects the SPARC64 next-stage route in Bootah without
changing the shared architecture number registry or claiming UNDI support.

No firmware reboot fallback is performed. An unavailable DHCP/boot service,
unsafe/recursive filename, invalid GRUB image, expired lease or failed memory
claim stops at the firmware console. Solaris and SPARC32 retain their existing routes.

## Simulation control

Stock QEMU OpenBIOS maps `-kernel` at a fixed virtual address and its network
nodes may lack usable CIF drivers. A separate test-only link layout maps the entry stub for OpenBIOS and keeps
its source at physical 32 MiB, away from the relocation target. The embedded
production body is unchanged; the production binary remains at the Sun entry:

```sh
make -C tools/obp-loader qemu-control.elf
python3 scripts/sparc64_stage2_boot.py --loader tools/obp-loader/qemu-control.elf
```

The script locally loads the second stage and supplies a bootpath, deliberately
skipping firmware RARP/TFTP. It records serial output and packets. A successful
control requires GRUB to fetch its configuration and attempt the named probe
kernel. The current stock OpenBIOS/SunHME combination stops because its device
node has neither an active nor a local MAC address; no GRUB/network pass is
claimed. Original OpenBoot hardware testing remains required.

Sources and Chinese deployment instructions: [SPARC64 stage2 design](../../docs/sparc64-stage2-design.md).
CIF ABI: IEEE 1275, Sun OpenBoot client interface and SPARC V9 conventions.
MAC policy was checked against Sun OpenBoot source `pkg/dhcp/macaddr.fth` and
`dev/network/ophir/pkg.fth`; network open conventions against GRUB 2.12 ofnet;
GRUB entry/image format against its SPARC64 crt0 and mkimage sources. Protocols
follow RFC 2131/2132/3004/3396, TFTP RFC 1350/2347/2348/7440 and PXE 2.1.

`make cpu-control.elf` builds an isolated SPARC64 control payload using the
production trampoline. Load it with stock QEMU sun4u `-kernel`, then `boot`:
it claims low memory, returns to the firmware stack, releases the high body,
and uses CIF from the probe entry. Expected output is
`Bootah trampoline CIF passed`. This validates the CPU/CIF transfer only; it
skips networking and does not execute GRUB or ofwboot.

The server prioritizes a configured client IP. With no configured IP it can
obtain a DHCP lease on behalf of a registered, assigned RARP client before
answering. See [RARP address allocation](../../docs/rarp-address-allocation.md).

Sun CIF `setprop` has limited creation/update semantics. Correct existing
values are left alone; native BSD does not write the NIC MAC property.
`bootpath` and `bootargs` use Sun path-buf/args-buf only after checking that
the existing property points to that exact buffer (256/128 bytes including
NUL). Other firmware retains CIF string setters. This avoids the Sun
chosen-string setter/encoded-pair mismatch without replacing dynamic
properties. Binary properties fall back to a standard
Forth helper using `encode-bytes` and `property`, with saved/restored package
and instance state. Copies live in firmware storage after the resident body
is released. Every value still requires exact length/byte readback; errors
name the property and node. GRUB image, address, path semantics, arguments
and network-close ordering remain unchanged.

`make check` also covers readonly MAC preservation, verified return-length
quirks and owned binary copies. The CPU control forces the Forth copy path,
alters the original bytes and reads the property through CIF again after
release. It prints `Bootah property CIF copy passed` before the trampoline
pass. Original Sun ROM Forth property control is recorded separately;
complete original-firmware handoff still needs hardware validation.

Before address configuration, DHCP DISCOVER/REQUEST sets BROADCAST. If only
an address offer is received without boot metadata, discovery retransmits
with the same transaction/user class on bounded PXE retry windows, retaining
the valid offer. This is packet-loss recovery, not a firmware restart or
self-loader fallback. `test-network.c` can drop proxy offers with
`BOOTAH_TEST_DROP_PROXY=1`; the external-DHCP control must then show two
discovers and one dropped offer before its normal next-stage transfer.
