# Bootah SRM loader

Small standalone Alpha SRM network loader for Debian, targeting EV56 and newer SRM platforms. It uses the SRM callback ABI and the architected 8 KiB bootstrap address space, with EV4-compatible integer instructions. It contains no chipset or NIC register access. Tru64/OpenVMS continue to use their original loaders.

Build on Linux with `gcc-alpha-linux-gnu`, `binutils-alpha-linux-gnu` and make:

```sh
make -C tools/srm-loader
make -C tools/srm-loader install
```

`CROSS_COMPILE` selects another Alpha GNU toolchain. The raw binary is loaded by SRM at the architected virtual address `0x20000000`. BSS is included in the load image to ensure all working buffers are mapped. The checked-in binary allows macOS/Windows builds without requiring an Alpha cross compiler.

Firmware console, network OPEN/READ/WRITE and environment variables are accessed through HWRPB/CRB. Memory allocation checks static or instance-private distributed memory descriptors, tested-page bitmaps, and active bootstrap mappings. Distributed FRU descriptors are reached using their physical offsets, including when the FRU is separate from HWRPB. The loader switches to the firmware-provided OSF PALcode, retains firmware mappings while downloading, validates ELF segments, and passes the Linux parameter page relative to the ELF entry point. Kernel and initrd transfers are bounded by their declared sizes. PAL_wrfen enables floating-point instructions before the final Linux entry, independently of the firmware callbacks.

The server expands the original gzip kernel to ELF and serves the original Debian installer initrd unchanged. The loader implements BOOTP, ARP, IPv4/UDP and TFTP with bounded retries and block-size negotiation and RFC 7440 windows (four blocks when accepted, single-block fallback). A non-PXE external DHCP server must support BOOTP reservations with `next-server` and a boot filename. No PXE/4011 exchange is used.

Sources for interface definitions:

- Alpha Architecture Reference Manual (fourth edition), Console Interface Architecture chapters 26–27: <https://docs.alexrp.com/alpha/alpha_4th.pdf>
- Linux Alpha kernel entry and parameter ABI: <https://github.com/torvalds/linux/blob/master/arch/alpha/include/asm/setup.h>
- NetBSD Alpha firmware network interface behavior: <https://github.com/NetBSD/src/blob/trunk/sys/arch/alpha/stand/netboot/if_prom.c>

Validation and platform limitations: [docs/alpha-srm.md](../../docs/alpha-srm.md). Building for EV4 does not imply that current Debian kernels support EV4. One platform's firmware validation does not certify other platforms.
