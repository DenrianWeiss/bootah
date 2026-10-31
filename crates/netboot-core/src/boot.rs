pub(crate) fn pxe_loader<'a>(profile: &'a crate::model::Profile, architecture: &str) -> &'a str {
    let configured = profile.boot_files.get(architecture)
        .or_else(|| profile.boot_files.get("default"))
        .map(String::as_str).unwrap_or("");
    if architecture == "x64-efi"
        && matches!(profile.kind.as_str(), "windows" | "windows-pe-iso")
        && configured == "ipxe-x64.efi"
        && crate::media_source::exists(&profile.root, "snponly-x64.efi")
    {
        "snponly-x64.efi"
    } else {
        configured
    }
}

use crate::model::*;
use serde::Serialize;
#[derive(Serialize)]
pub struct ManualBoot {
    pub firmware: String,
    pub title: String,
    pub status: String,
    pub commands: String,
    pub notes: Vec<String>,
}
fn manual(
    firmware: &str,
    title: &str,
    status: &str,
    commands: impl Into<String>,
    notes: Vec<String>,
) -> ManualBoot {
    ManualBoot {
        firmware: firmware.into(),
        title: title.into(),
        status: status.into(),
        commands: commands.into(),
        notes,
    }
}
fn note(chinese: bool, english: impl Into<String>, chinese_text: impl Into<String>) -> String {
    if chinese {
        chinese_text.into()
    } else {
        english.into()
    }
}
pub fn manual_boot(profile: &Profile, settings: &Settings, chinese: bool) -> ManualBoot {
    match profile.architecture.as_str() {
        "alpha" => alpha(profile, settings, chinese),
        arch if arch.starts_with("sparc") => openbios(profile, settings, chinese),
        "ia64" => ia64(profile, chinese),
        "hppa" | "hppa64" => palo(profile, chinese),
        "mips" | "mipsel" => sgi(chinese),
        "powerpc-of" | "powerpc-epapr" | "power-opal" | "ppc" | "ppc64" | "ppc64le" => {
            openfirmware(profile, chinese)
        }
        _ if profile
            .boot_files
            .keys()
            .any(|key| key.ends_with("-efi") || key == "x86-bios" || key.ends_with("-http")) =>
        {
            pxe_auto(profile, chinese)
        }
        _ => uboot_manual(profile, settings, chinese),
    }
}
fn pxe_auto(profile: &Profile, chinese: bool) -> ManualBoot {
    let file = profile
        .boot_files
        .values()
        .next()
        .cloned()
        .unwrap_or_default();
    let verified = !crate::secure_boot::enabled(profile) && matches!(profile.architecture.as_str(), "x64" | "arm64");
    manual(
        "pxe",
        "UEFI/BIOS PXE",
        if verified { "verified" } else { "documented" },
        "",
        vec![
            note(
                chinese,
                format!(
                    "Select the NIC's PXE/IPv4 boot entry in the firmware boot menu; Bootah returns `{file}` as the network boot file. No command needs to be typed."
                ),
                format!("Select the NIC's PXE/IPv4 entry in the firmware menu; Bootah supplies `{file}` as the network boot file, so no command is needed."),
            ),
            note(
                chinese,
                "If the firmware opens a shell, return to its boot menu and select the network boot option.",
                "Return to the firmware boot menu and select the network boot option if a shell opens.",
            ),
        ],
    )
}
fn uboot_manual(profile: &Profile, settings: &Settings, chinese: bool) -> ManualBoot {
    let riscv =
        profile.architecture == "riscv64" && profile.kind == "linux" && !profile.initrd.is_empty();
    let mut notes = Vec::new();
    if !riscv {
        notes.push(note(
            chinese,
            "`pxe get` reads the generated PXE/extlinux entry and `pxe boot` runs it.",
            "`pxe get` reads the generated PXE/extlinux entry and `pxe boot` runs it.",
        ));
    }
    notes.push(note(
        chinese,
        "Board-specific DTB, load addresses and the `fdt_addr` variable must match the device; `setenv fdt_addr ${fdtcontroladdr}` is only valid where the firmware provides it.",
        "The board DTB, load addresses, and `fdt_addr` must match the device; set `fdt_addr` from `${fdtcontroladdr}` only when the firmware provides it.",
    ));
    if profile.kind == "fit" {
        notes.push(note(chinese, "FIT profiles use `tftpboot` and `bootm`; the load address must match the board memory map.", "FIT profiles use `tftpboot` and `bootm`; the load address must match the board memory map."));
    }
    if !profile.arguments.is_empty() {
        if riscv || profile.kind == "fit" {
            notes.push(note(
                chinese,
                "Kernel parameters are assigned to U-Boot `bootargs` before `booti`/`bootm`.",
                "Kernel parameters are written to U-Boot `bootargs` before running `booti`/`bootm`.",
            ));
        } else {
            notes.push(note(
                chinese,
                "The generated PXE/extlinux entry passes profile arguments through its `APPEND` line.",
                "The generated PXE/extlinux entry passes profile arguments through its `APPEND` line.",
            ));
        }
    }
    if settings.tftp_port != 69 {
        notes.push(note(
            chinese,
            format!(
                "The firmware must support `tftpdstp` for the non-standard TFTP port {}.",
                settings.tftp_port
            ),
            format!(
                "Firmware must support the `tftpdstp` environment variable for non-standard TFTP port {}.",
                settings.tftp_port
            ),
        ));
    }
    let verified = profile.architecture.ends_with("-uboot")
        || profile.boot_files.keys().any(|key| key.ends_with("-uboot"));
    if riscv {
        notes.push(note(
            chinese,
            "Debian RISC-V netboot `linux` is a PE/EFI image. Use `booti`; raw initrds require the downloaded size (`${filesize}`). `bootefi` has no initrd argument. Load addresses must match the board.",
            "Debian RISC-V netboot `linux` is a PE/EFI image. Use `booti`; raw initrds need the downloaded size (`${filesize}`). `bootefi` has no initrd argument. Load addresses must match the board.",
        ));
    }
    let commands = if riscv {
        let mut lines = vec![
            format!("setenv serverip {}", settings.server_ip),
            "setenv autoload no".into(),
            "dhcp".into(),
        ];
        match uboot_bootargs(profile, settings) {
            Ok(Some(command)) => lines.push(command),
            Ok(None) => {}
            Err(error) => {
                notes.push(note(chinese, error.clone(), error));
                return manual("uboot", "U-Boot", "unavailable", "", notes);
            }
        }
        if settings.tftp_port != 69 {
            lines.push(format!("setenv tftpdstp {}", settings.tftp_port));
        }
        lines.push(format!("tftpboot ${{kernel_addr_r}} {}", profile.kernel));
        lines.push(format!("tftpboot ${{ramdisk_addr_r}} {}", profile.initrd));
        lines.push("booti ${kernel_addr_r} ${ramdisk_addr_r}:${filesize} ${fdtcontroladdr}".into());
        lines.join("\n")
    } else {
        match uboot(profile, settings) {
            Ok(commands) => commands,
            Err(error) => {
                notes.push(note(chinese, error.clone(), error));
                return manual("uboot", "U-Boot", "unavailable", "", notes);
            }
        }
    };
    manual(
        "uboot",
        "U-Boot",
        if verified || riscv {
            "verified"
        } else {
            "documented"
        },
        commands,
        notes,
    )
}
fn alpha(profile: &Profile, settings: &Settings, chinese: bool) -> ManualBoot {
    if crate::alpha_srm::enabled(profile) {
        return manual(
            "srm", "Alpha SRM", "documented",
            "show device\nboot -protocols bootp <net-device> -file bootah-srm.bin",
            vec![
                note(chinese, "Replace <net-device> with the firmware-supported Ethernet device from show device (for example ewa0, ewb0 or era0). Check the machine manual for console-specific syntax. The small loader downloads the Debian kernel and initrd separately.", "把 <net-device> 替换为 show device 列出的固件支持的以太网设备（如 ewa0、ewb0 或 era0），并按该机型手册核对控制台语法。小型引导器分别下载 Debian 内核和 initrd。"),
                note(chinese, "With external DHCP, configure a BOOTP reservation, boot server and filename there; SRM does not use PXE ProxyDHCP.", "已有 DHCP 时，请在外部服务器配置 BOOTP 地址、启动服务器及文件名；SRM 不使用 PXE ProxyDHCP。"),
                note(chinese, "Each target platform requires separate validation; ES40 results do not certify other Alpha systems.", "目标平台必须单独验证；ES40 的验收结果不代表其他 Alpha 平台已通过。"),
            ],
        );
    }
    let openvms = profile.boot_files.contains_key("lad")
        || profile
            .boot_files
            .keys()
            .any(|key| key.to_ascii_uppercase().starts_with("APB_"))
        || profile.kernel.to_ascii_uppercase().starts_with("APB_")
        || profile.notes.to_ascii_uppercase().contains("OPENVMS");
    let mop = openvms
        || profile.boot_files.contains_key("mop")
        || profile.kernel.to_ascii_lowercase().ends_with(".sys")
        || profile.notes.to_ascii_lowercase().contains("mop");
    let arguments = render_arguments(profile, settings);
    let flags = if arguments.is_empty() {
        String::new()
    } else {
        format!(" -flags \"{}\"", arguments.replace(['"', '\n', '\r'], ""))
    };
    let mut notes = vec![note(
        chinese,
        "At the SRM `>>>` prompt; run `show device` if the Ethernet device is not `ewa0`.",
        "Run these commands at the SRM `>>>` prompt. If the NIC is not `ewa0`, use `show device` to find its name.",
    )];
    if openvms {
        let file = profile
            .boot_files
            .get("mop")
            .filter(|file| !file.is_empty())
            .unwrap_or(&profile.kernel);
        let file = file.rsplit(['/', '\\']).next().unwrap_or(file);
        let file = file
            .rsplit_once('.')
            .filter(|(_, extension)| extension.eq_ignore_ascii_case("sys"))
            .map(|(name, _)| name)
            .unwrap_or(file);
        notes.push(note(
            chinese,
            "MOP loads the first-stage bootstrap and opens the OpenVMS Network Initial System Load menu.",
            "MOP loads the first-stage bootstrap and opens the OpenVMS Network Initial System Load menu.",
        ));
        notes.push(note(
            chinese,
            "A complete OpenVMS network installation requires an InfoServer LAD/LAST media service.",
            "A complete OpenVMS network installation requires InfoServer LAD/LAST media services.",
        ));
        manual(
            "srm",
            "Alpha SRM",
            "verified",
            format!("set ewa0_protocols mop\nboot ewa0 -flags 0,0 -file {file}"),
            notes,
        )
    } else {
        if mop {
            notes.push(note(
                chinese,
                "MOP loads the second-stage bootstrap before entering the Linux kernel.",
                "MOP loads the second-stage bootstrap, which then starts the Linux kernel.",
            ));
        } else {
            notes.push(note(
                chinese,
                "SRM obtains the boot file name from BOOTP/DHCP; Bootah serves the file over TFTP.",
                "SRM obtains the boot filename through BOOTP/DHCP, and Bootah serves the file over TFTP.",
            ));
        }
        if !flags.is_empty() {
            notes.push(note(
                chinese,
                "The profile kernel arguments are passed as SRM `-flags`; remove them if the console rejects them.",
                "SRM passes this system's kernel parameters through `-flags`; remove the parameter if the console rejects it.",
            ));
        }
        notes.push(note(
            chinese,
            "Alpha Linux uses aboot to load the Linux kernel.",
            "Alpha Linux uses aboot to load the Linux kernel.",
        ));
        notes.push(note(
            chinese,
            "Bootah does not provide the NFS root used by NetBSD/OpenBSD diskless boot.",
            "Bootah does not provide the NFS root filesystem required for NetBSD/OpenBSD diskless boot.",
        ));
        manual(
            "srm",
            "Alpha SRM",
            "verified",
            format!(
                "set ewa0_protocols {}\nboot ewa0{flags}",
                if mop { "mop" } else { "bootp" }
            ),
            notes,
        )
    }
}
fn sgi(chinese: bool) -> ManualBoot {
    manual(
        "sgi",
        "SGI ARC/PROM",
        "documented",
        "unsetenv netaddr\nbootp():",
        vec![note(
            chinese,
            "At the SGI PROM `>>` prompt. If `bootp():` is rejected, use `boot -f bootp()/netboot-boot.img`.",
            "Run this at the SGI PROM `>>` prompt. If `bootp():` is rejected, use `boot -f bootp()/netboot-boot.img`.",
        )],
    )
}
fn openbios(profile: &Profile, settings: &Settings, chinese: bool) -> ManualBoot {
    let (commands, mut notes) = if settings.dhcp_enabled {
        (
            "boot net:dhcp",
            vec![note(
                chinese,
                "OpenBoot uses RARP with `boot net`; `boot net:dhcp` asks the firmware to configure the client with DHCP.",
                "OpenBoot `boot net` uses RARP; `boot net:dhcp` requires the firmware to configure the client through DHCP.",
            )],
        )
    } else {
        (
            "boot net",
            vec![note(
                chinese,
                "OpenBoot uses RARP by default; enable DHCP in Bootah and use `boot net:dhcp` to use DHCP instead.",
                "OpenBoot uses RARP by default; use `boot net:dhcp` after enabling DHCP in Bootah.",
            )],
        )
    };
    notes.push(note(
        chinese,
        "Assign this profile before booting. BOOTP/DHCP supplies a filename; RARP supplies an IP address, from which OpenBoot derives its TFTP filename.",
        "Assign this system to the device before booting. BOOTP/DHCP provides the filename; RARP provides only the IP, from which OpenBoot derives the TFTP filename.",
    ));
    notes.push(note(
        chinese,
        "These commands require OpenBoot network boot support. Solaris 11 AI/IPS and WAN boot services are not supported.",
        "These commands require OpenBoot network boot support. Solaris 11 AI/IPS and WAN boot services are not supported.",
    ));
    let commands = if profile.kind == "solaris" {
        let arguments = render_arguments(profile, settings);
        format!(
            "{commands} - install{}{}",
            if arguments.is_empty() { "" } else { " " },
            arguments.replace(['\n', '\r'], " ")
        )
    } else {
        commands.into()
    };
    manual("openbios", "SPARC OpenBIOS", "documented", commands, notes)
}
fn ia64(profile: &Profile, chinese: bool) -> ManualBoot {
    let mut notes = vec![
        note(
            chinese,
            "IA64 ELILO netboot is started from the EFI firmware network boot option: firmware PXE/DHCP loads `elilo.efi`, then ELILO reads `elilo-ia64.conf`, the kernel and initrd from the same TFTP directory.",
            "IA64 ELILO netboot starts from the EFI firmware network boot option: firmware loads `elilo.efi` through PXE/DHCP, then ELILO reads `elilo-ia64.conf`, the kernel, and initrd from the same TFTP directory.",
        ),
        note(
            chinese,
            "An EFI shell can configure the network with `ifconfig -s eth0 dhcp`, but the standard EFI shell has no command that starts a PXE/ELILO network boot.",
            "The EFI shell can configure networking with `ifconfig -s eth0 dhcp`, but the standard EFI shell has no command to start a PXE/ELILO network boot.",
        ),
    ];
    if profile.boot_files.contains_key("ia64") {
        notes.push(note(
            chinese,
            "This profile already prepares `elilo.efi` and `elilo-ia64.conf` for TFTP.",
            "This profile prepares `elilo.efi` and `elilo-ia64.conf` for TFTP.",
        ));
    }
    manual("efi", "IA64 EFI/ELILO", "unavailable", "", notes)
}
fn palo(profile: &Profile, chinese: bool) -> ManualBoot {
    let mut notes = vec![
        note(
            chinese,
            "At the PDC `BOOT_ADMIN>` prompt, `boot lan` asks the firmware to use BOOTP/TFTP; the boot file name is supplied by the DHCP/BOOTP reply.",
            "At the PDC `BOOT_ADMIN>` prompt, run `boot lan` to use BOOTP/TFTP; the DHCP/BOOTP reply supplies the boot filename.",
        ),
        note(
            chinese,
            "Newer Series 700 and B/C/J class systems use BOOTP/TFTP; older RMP-only models use RMP instead.",
            "Newer Series 700 and B/C/J systems use BOOTP/TFTP; older RMP-only models use RMP.",
        ),
        note(
            chinese,
            "Bootah generates a PALO LIF boot file (`netboot-hppa.lif` or `netboot-hppa64.lif`).",
            "Bootah generates a PALO LIF boot file (`netboot-hppa.lif` or `netboot-hppa64.lif`).",
        ),
    ];
    if !profile.boot_files.is_empty() {
        notes.push(note(
            chinese,
            "Make sure the firmware boot file matches the `default` boot file mapping of this profile.",
            "Confirm that the firmware boot filename matches this profile's `default` boot-file mapping.",
        ));
    }
    manual("palo", "PA-RISC PALO", "documented", "boot lan", notes)
}
fn openfirmware(profile: &Profile, chinese: bool) -> ManualBoot {
    let of = profile.boot_files.contains_key("powerpc-of");
    let opal = profile.boot_files.contains_key("power-opal");
    manual(
        if opal && !of { "petitboot" } else { "openfirmware" },
        if opal && !of { "PowerNV / Petitboot" } else { "Open Firmware / SLOF / Petitboot" },
        if of || opal { "documented" } else { "unavailable" },
        "",
        vec![
            crate::i18n::message(
                "Select the network device in SLOF, or the discovered network installation entry in Petitboot. Available firmware types are listed in this image's boot files.",
                if chinese { "zh-CN" } else { "en" }, "en",
            ),
            crate::i18n::message(
                "Manual Open Firmware commands depend on the device aliases provided by the firmware. Firmware types are not interchangeable.",
                if chinese { "zh-CN" } else { "en" }, "en",
            ),
        ],
    )
}
pub(crate) fn http_root(settings: &Settings, profile: &Profile) -> String {
    format!(
        "http://{}:{}/boot/{}",
        settings.server_ip, settings.http_port, profile.id
    )
}
pub(crate) fn render_arguments(profile: &Profile, settings: &Settings) -> String {
    let base = http_root(settings, profile);
    let http = format!("http://{}:{}", settings.server_ip, settings.http_port);
    profile
        .arguments
        .replace("{base}", &base)
        .replace("{http}", &http)
        .replace("{server}", &settings.server_ip.to_string())
        .replace("{http_port}", &settings.http_port.to_string())
        .replace("{profile}", &profile.id)
        .replace(
            "{nfs_port}",
            &crate::nfs::configured_override("BOOTAH_NFS_PORT", Some(2049))
                .ok()
                .flatten()
                .unwrap_or(2049)
                .to_string(),
        )
        .replace(
            "{nbd_port}",
            &crate::nbd::port()
                .ok()
                .flatten()
                .unwrap_or(10809)
                .to_string(),
        )
}
pub fn ipxe(profile: &Profile, settings: &Settings, auto: Option<&AutoConfig>) -> String {
    if crate::signed_ipxe::enabled(profile) { return crate::signed_ipxe::script(profile, settings, auto); }
    let base = if settings.boot_transport == "tftp" {
        format!("tftp://{}", settings.server_ip)
    } else {
        http_root(settings, profile)
    };
    let arguments = crate::automation::arguments(profile, settings, auto);
    match profile.kind.as_str() {
        "solaris" => crate::solaris::ipxe(profile, settings),
        "windows-pe-iso" => format!(
            "#!ipxe\nsanboot {}/media.iso\n",
            http_root(settings, profile)
        ),
        "linux" => {
            let mut words = arguments.split_whitespace().collect::<Vec<_>>();
            if !words.iter().any(|arg| arg.starts_with("BOOTIF=")) {
                let index = words
                    .iter()
                    .position(|arg| *arg == "---" || *arg == "--")
                    .unwrap_or(words.len());
                words.insert(index, "BOOTIF=01-${netX/mac:hexhyp}");
            }
            format!(
                "#!ipxe\nkernel {base}/{} {}\n{}boot\n",
                profile.kernel,
                words.join(" "),
                if profile.initrd.is_empty() {
                    String::new()
                } else {
                    format!("initrd {base}/{}\n", profile.initrd)
                }
            )
        }
        "bsd" => {
            let bios = profile
                .boot_files
                .get("x86-bios")
                .unwrap_or(&profile.kernel);
            let key = crate::efi::for_arch(&profile.architecture).map(|loader| loader.firmware_key);
            let efi = key
                .and_then(|key| profile.boot_files.get(key))
                .map(|file| format!("chain {base}/{file}\n"))
                .unwrap_or_else(|| {
                    "echo No matching EFI boot file for this architecture\nexit\n".into()
                });
            format!(
                "#!ipxe\niseq ${{platform}} efi && goto efi || goto bios\n:efi\n{efi}:bios\nchain {base}/{bios}\n"
            )
        }
        "windows" => {
            let kernel = if profile.kernel.is_empty() {
                "wimboot"
            } else {
                &profile.kernel
            };
            let image = if profile.initrd.is_empty() {
                "boot.wim"
            } else {
                &profile.initrd
            };
            let mut script = format!("#!ipxe\nimgfree\nkernel {base}/{kernel}\n");
            for file in ["BCD", "boot.sdi"] {
                if crate::media_source::exists(&profile.root, file) {
                    script.push_str(&format!("initrd {base}/{file} {file}\n"));
                }
            }
            for file in ["bootah-setup.cmd", "winpeshl.ini"] {
                if crate::media_source::exists(&profile.root, file) {
                    script.push_str(&format!("initrd {base}/{file} {file}\n"));
                }
            }
            if let Some(auto) = auto.filter(|auto| auto.method == "unattend") {
                script.push_str(&format!(
                    "initrd {}unattend.xml unattend.xml\n",
                    crate::automation::url(settings, auto)
                ));
            }
            script.push_str(&format!("initrd {base}/{image} boot.wim\nboot\n"));
            script
        }
        _ => format!("#!ipxe\nchain {base}/{}\n", profile.kernel),
    }
}
pub fn pxe(profile: &Profile, settings: &Settings, auto: Option<&AutoConfig>) -> String {
    let arguments = crate::automation::arguments(profile, settings, auto);
    let prefix = if matches!(profile.architecture.as_str(), "ppc64le" | "ppc64") {
        if settings.boot_transport == "http" { format!("{}/", http_root(settings, profile)) }
        else { "/".into() }
    } else { String::new() };
    let kernel = format!("{prefix}{}", profile.kernel);
    format!(
        "DEFAULT boot\nLABEL boot\n  KERNEL {}\n{}{}  APPEND {}\n",
        kernel,
        if profile.initrd.is_empty() {
            String::new()
        } else {
            format!("  INITRD {prefix}{}\n", profile.initrd)
        },
        if profile.dtb.is_empty() {
            String::new()
        } else {
            format!("  FDT {}\n", profile.dtb)
        },
        arguments
    )
}
fn uboot_bootargs(profile: &Profile, settings: &Settings) -> Result<Option<String>, String> {
    let arguments = render_arguments(profile, settings);
    if arguments.is_empty() {
        return Ok(None);
    }
    let arguments = arguments.replace(['\r','\n'], " ").replace('\'', "'\\''");
    Ok(Some(format!("setenv bootargs '{arguments}'")))
}
pub fn uboot(profile: &Profile, settings: &Settings) -> Result<String, String> {
    let mut commands = vec![
        format!("setenv serverip {}", settings.server_ip),
        "setenv autoload no".into(),
        "dhcp".into(),
    ];
    if settings.tftp_port != 69 {
        commands.push(format!("setenv tftpdstp {}", settings.tftp_port));
    }
    if profile.kind == "fit" {
        commands.push(format!("tftpboot ${{loadaddr}} {}", profile.kernel));
        if let Some(bootargs) = uboot_bootargs(profile, settings)? {
            commands.push(bootargs);
        }
        commands.push("bootm ${loadaddr}".into());
    } else {
        commands.push("pxe get".into());
        commands.push("pxe boot".into());
    }
    Ok(commands.join("\n"))
}
pub fn virtual_file(
    profile: &Profile,
    name: &str,
    settings: &Settings,
    auto: Option<&AutoConfig>,
) -> Option<Vec<u8>> {
    if name == "bootah-srm.cfg" {
        return crate::alpha_srm::config(profile, settings, auto);
    }
    if (profile.kind == "windows" || profile.kind == "windows-pe-iso" && crate::native_uefi::enabled(profile)) && !profile.arguments.is_empty()
        && (name.eq_ignore_ascii_case("boot/bcd") || name.eq_ignore_ascii_case("bcd")) {
        return Some(crate::boot_options::windows(profile).unwrap_or_default());
    }
    if crate::native_uefi::enabled(profile)
        && matches!(profile.kind.as_str(), "windows" | "windows-pe-iso")
        && (name.eq_ignore_ascii_case("boot/bcd") || name.eq_ignore_ascii_case("bcd"))
    {
        return Some(
            crate::model::safe_file(&profile.root, "bootah-native/bcd")
                .ok()
                .and_then(|p| std::fs::read(p).ok())
                .unwrap_or_default(),
        );
    }
    if matches!(
        name,
        "grub.cfg"
            | "grub/grub.cfg"
            | "grub2/grub.cfg"
            | "boot/grub2/grub.cfg"
            | "boot/grub/grub.cfg"
            | "boot/grub/x86_64-efi/grub.cfg"
            | "bootah-native/grub.cfg"
            | "bootah-native/grub/grub.cfg"
    ) {
        if (profile.kind == "linux" || profile.system_id == "freebsd") && profile.architecture == "ppc64le" {
            let arguments = crate::automation::arguments(profile, settings, auto);
            return Some(format!("set timeout=0\nset default=0\nmenuentry 'Bootah' {{\n linux /{} {}\n initrd /{}\n}}\n", profile.kernel, arguments, profile.initrd).into_bytes());
        }
        if let Some(config) = crate::native_uefi::grub(profile, settings, auto) {
            return Some(config);
        }
    }
    if profile.kind == "windows-ris" && name.eq_ignore_ascii_case("winnt.sif") {
        return Some(crate::windows::ris_answer(
            profile,
            &settings.server_ip.to_string(),
            &[],
        ));
    }
    if profile.architecture == "ia64" && profile.kind == "linux" {
        let base = name.rsplit('/').next().unwrap_or(name);
        if base == "elilo.conf" || base == "elilo-ia64.conf" {
            let arguments = render_arguments(profile, settings).replace(['\n', '\r', '"'], " ");
            return Some(format!("prompt\ntimeout=10\ndefault=install\nimage=/{}\n  label=install\n  initrd=/{}\n  append=\"{}\"\n", profile.kernel, profile.initrd, arguments).into_bytes());
        }
    }
    if profile.kind == "windows" && name == "bootah-setup.cmd" {
        return Some(b"@echo off\r\nwpeinit\r\necho Bootah: Windows PE is ready. Use a complete installation ISO for Windows Setup.\r\nexit /b 0\r\n".to_vec());
    }
    if profile.kind == "bsd" && name == "boot.cfg" && profile.kernel.starts_with("netbsd") {
        let flags = bsd_flags(&profile.arguments);
        let boot = if profile.initrd.is_empty() {
            format!("boot tftp:{}{}", profile.kernel, flags)
        } else {
            format!("initrd {};boot {}{}", profile.initrd, profile.kernel, flags)
        };
        return Some(format!("menu=Install NetBSD:{boot}\ntimeout=0\n").into_bytes());
    }
    if profile.kind == "bsd" && name == "etc/boot.conf" && profile.kernel.ends_with("bsd.rd") {
        let flags = bsd_flags(&profile.arguments);
        return Some(format!("boot {}{}\n", profile.kernel, flags).into_bytes());
    }
    if crate::signed_ipxe::is_script(profile, name) {
        return Some(crate::signed_ipxe::script(profile, settings, auto).into_bytes());
    }
    if name == "boot.ipxe" {
        return Some(ipxe(profile, settings, auto).into_bytes());
    }
    if name.strip_prefix("pxelinux.cfg/").is_some_and(|tail| !tail.is_empty() && !tail.contains('/'))
        || name == "bootah-petitboot" || name == "extlinux/extlinux.conf" {
        let mut config = pxe(profile, settings, auto);
        if name.starts_with("pxelinux.cfg/") && profile.kind == "linux" {
            config.push_str("  IPAPPEND 2\n");
        }
        return Some(config.into_bytes());
    }
    None
}

fn bsd_flags(arguments: &str) -> String {
    if arguments.is_empty() {String::new()} else {format!(" {}", arguments.replace(['\r','\n']," "))}
}

#[cfg(test)]
mod efi_tests {
    #[test]
    fn bsd_efi_chain_requires_matching_architecture() {
        let mut profile: crate::model::Profile = serde_json::from_value(serde_json::json!({
            "id":"bsd", "name":"BSD", "architecture":"arm64", "kind":"bsd", "root":"/tmp",
            "kernel":"netbsd", "boot_files":{"x64-efi":"bootx64.efi", "arm64-efi":"bootaa64.efi"}
        }))
        .unwrap();
        let settings = crate::model::Settings::default();
        let script = super::ipxe(&profile, &settings, None);
        assert!(script.contains("chain http://192.168.50.1:8080/boot/bsd/bootaa64.efi"));
        assert!(!script.contains("bootx64.efi"));
        profile.architecture = "loongarch64".into();
        let script = super::ipxe(&profile, &settings, None);
        assert!(script.contains("No matching EFI boot file"));
        assert!(!script.contains("bootx64.efi"));
    }
}

#[cfg(test)]
mod snp_tests {
    #[test]
    fn pe_snp_selection_preserves_other_boot_paths() {
        let root = std::env::temp_dir().join(format!("bootah-snp-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("snponly-x64.efi"), b"fixture").unwrap();
        let mut profile: crate::model::Profile = serde_json::from_value(serde_json::json!({
            "id":"pe", "name":"PE", "architecture":"x64", "kind":"windows",
            "root":root, "boot_files":{"x64-efi":"ipxe-x64.efi", "x86-bios":"undionly.kpxe"}
        })).unwrap();
        assert_eq!(super::pxe_loader(&profile, "x64-efi"), "snponly-x64.efi");
        assert_eq!(super::pxe_loader(&profile, "x86-bios"), "undionly.kpxe");
        profile.kind = "windows-pe-iso".into();
        assert_eq!(super::pxe_loader(&profile, "x64-efi"), "snponly-x64.efi");
        profile.kind = "linux".into();
        assert_eq!(super::pxe_loader(&profile, "x64-efi"), "ipxe-x64.efi");
        profile.kind = "windows".into();
        profile.boot_files.insert("x64-efi".into(), "custom.efi".into());
        assert_eq!(super::pxe_loader(&profile, "x64-efi"), "custom.efi");
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod power_tests {
    #[test]
    fn imported_power_firmware_configs_render_original_media_arguments() {
        let profile: crate::model::Profile = serde_json::from_value(serde_json::json!({
            "id":"power", "name":"Power", "architecture":"ppc64le", "kind":"linux",
            "root":"/unused", "kernel":"install/vmlinux", "initrd":"bootah-compat-initrd-0",
            "arguments":"bootah.server={server} bootah.export={profile}",
            "boot_files":{"powerpc-of":"grub-powerpc.elf","power-opal":"bootah-petitboot"}
        })).unwrap();
        let mut settings = crate::model::Settings::default();
        settings.boot_transport = "tftp".into();
        let grub = String::from_utf8(super::virtual_file(&profile, "boot/grub/grub.cfg", &settings, None).unwrap()).unwrap();
        let pxe = String::from_utf8(super::virtual_file(&profile, "pxelinux.cfg/default", &settings, None).unwrap()).unwrap();
        assert!(grub.contains("linux /install/vmlinux bootah.server=192.168.50.1 bootah.export=power"));
        assert!(grub.contains("initrd /bootah-compat-initrd-0"));
        assert!(pxe.contains("KERNEL /install/vmlinux"));
        assert!(pxe.contains("bootah.export=power"));
        settings.boot_transport = "http".into();
        assert!(super::pxe(&profile, &settings, None).contains("KERNEL http://192.168.50.1:8080/boot/power/install/vmlinux"));
        assert_eq!(super::pxe_loader(&profile, "powerpc-of"), "grub-powerpc.elf");
        assert_eq!(super::pxe_loader(&profile, "power-opal"), "bootah-petitboot");
        assert!(super::virtual_file(&profile, "pxelinux.cfg/install/vmlinux", &settings, None).is_none());
    }
}
