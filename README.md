# Bootah the all-in-one netboot assistant

Bootah is an all-in-one assistant application supporting installing OS compeletely over ethernet.

## Features

- Built-in TFTP, NFS, NBD, SMBv1, SMBv2 and HTTP server for stages after loading kernel
- ProxyDHCP, to prevent breaking your net.
- Easy to use shell for your desktop
- Allow assigning different image for different system.
- Multi OS, arch guest and host support.

## Requirements

- PXE client must can connect back to the machine running the program
- 67/111/445/2049 port not occupied (No local SMB/NFS/TFTP server is running)
- (macOS) ChmodBPF installed.
- (Windows) npcap installed.

## Supported Image Types and CPUs

### AArch64

Only SBSA platform with PXE is supported.

- Windows after 11
- Linux*

### x86/x86_64

BIOS Platform:

- Windows XP or later, Windows Server 2000 or later.
- Linux*

EFI Platform

- Windows 7 or later
- Linux*
- OpenBSD, FreeBSD, NetBSD

### IA64

- Windows Server 2008 or later
- Linux*

### Alpha

SRM Firmware

- Linux*
- OpenVMS

### SPARC

- Linux
- OpenBSD
- NetBSD

### Power

PAPR Platform:

- Linux

### LoongArch64

- Linux

*: For linux, we support AOSC/ArchLinux/Centos/Debian/Fedora/KylinOS/SUSE/Ubuntu/UOS on x86/x86_64/AArch64 platform, AOSC/UOS/KylinOS/Debian for LoongArch64 platform. Debian for other platform(s), Limited support for CloneZilla and other dist exists. NixOS is specifically unsupported, since it requires special ramdisk.  
**: Secure Boot is currently only supported for Windows, SUSE, Ubuntu, CentOS/Fedora and Kylin OS, UOS is not supported.

## LICENSE

### Included Components

#### IPXE

This project includes pxe files from [IPXE Project](https://ipxe.org), which is licensed under GPL, these files are not modified from upstream project, you may get its source on the IPXE Project website

#### Grub

This project contains a modified version of GRUB for power platform, as a second-stage bootloader for PAPR platform, under `tools/power-boot` folder.

#### 

### Branding/Re-Licensing

If you need license other than AGPL, please mail me.