use crate::{
    media::ImportResult,
    media_source::Image,
    model::{BootDiagnostic, BootEvidence, Profile},
};
use std::{collections::BTreeMap, fs, path::Path};

fn directive<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    let mut parts = line.trim().splitn(2, char::is_whitespace);
    (parts.next()?.eq_ignore_ascii_case(name)).then(|| parts.next().unwrap_or("").trim_start())
}

#[cfg(test)]
mod directive_tests {
    #[test]
    fn casper_live_entry_does_not_inherit_install_only_flag() {
        let live = super::merge_original_arguments(
            "boot=casper live quiet",
            "ip=dhcp only-ubiquity url={base}/media.iso",
            "casper",
        );
        assert!(!live.split_whitespace().any(|arg| arg == "only-ubiquity"));
        assert!(live.contains("url={base}/media.iso"));
        let install = super::merge_original_arguments(
            "boot=casper only-ubiquity",
            "ip=dhcp only-ubiquity",
            "casper",
        );
        assert!(install.contains("only-ubiquity"));
    }
    #[test]
    fn accepts_boot_directives_with_variable_whitespace() {
        assert_eq!(
            super::directive("linux   /live/vmlinuz boot=live --", "linux"),
            Some("/live/vmlinuz boot=live --")
        );
        assert_eq!(
            super::directive("\tINITRD\t\t/live/initrd.img", "initrd"),
            Some("/live/initrd.img")
        );
        assert_eq!(
            super::directive("DEFAULT\tinstall", "default"),
            Some("install")
        );
        assert_eq!(super::directive("linuxefi /vmlinuz", "linux"), None);
    }
}

fn nixos_iso_default(image: &Image) -> Option<(String, String, String)> {
    image.find("EFI/nixos-installer-image")?;
    image.find("nix-store.squashfs")?;
    let config = image.find("isolinux/isolinux.cfg")?;
    let source = image.read(config, 256 * 1024).ok()?;
    let source = String::from_utf8_lossy(&source);
    let default = source.lines().find_map(|line| directive(line, "DEFAULT"))?;
    let mut selected = false;
    let mut kernel = None;
    let mut initrd = None;
    let mut arguments = None;
    for line in source.lines().map(str::trim) {
        if let Some(label) = directive(line, "LABEL") {
            selected = label.trim() == default;
        } else if selected {
            if let Some(path) = directive(line, "LINUX") {
                kernel = Some(path.trim());
            }
            if let Some(path) = directive(line, "INITRD") {
                initrd = Some(path.trim());
            }
            if let Some(args) = directive(line, "APPEND") {
                arguments = Some(args.trim().to_string());
            }
        }
    }
    let resolve = |path: &str| {
        if path.contains(['$', '`', '\\', ';']) {
            return None;
        }
        let parts = path
            .split('/')
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>();
        if parts.iter().any(|part| *part == "." || *part == "..") {
            return None;
        }
        image.find(&parts.join("/")).map(str::to_string)
    };
    let kernel = resolve(kernel?)?;
    let initrd = resolve(initrd?)?;
    let arguments = arguments?;
    if arguments.contains(['$', '`', '\\', ';']) {
        return None;
    }
    let grub = image.find("EFI/BOOT/grub.cfg")?;
    let grub = image.read(grub, 256 * 1024).ok()?;
    let grub = String::from_utf8_lossy(&grub);
    let grub_kernel = grub
        .lines()
        .find_map(|line| directive(line, "linux"))?
        .split_whitespace()
        .next()?;
    let grub_initrd = grub
        .lines()
        .find_map(|line| directive(line, "initrd"))?
        .split_whitespace()
        .next()?;
    if resolve(grub_kernel)? != kernel || resolve(grub_initrd)? != initrd {
        return None;
    }
    Some((kernel, initrd, arguments))
}

fn clonezilla_default(image: &Image) -> Option<(String, String, String)> {
    image.find("live/Clonezilla-Live-Version")?;
    let config = image.find("boot/grub/grub.cfg")?;
    let source = image.read(config, 256 * 1024).ok()?;
    let source = String::from_utf8_lossy(&source);
    let kernel_line = source
        .lines()
        .find_map(|line| directive(line, "$linux_cmd"))?;
    let initrd_line = source
        .lines()
        .find_map(|line| directive(line, "$initrd_cmd"))?;
    let (kernel_path, arguments) = kernel_line.split_once(char::is_whitespace)?;
    let kernel = image.find(kernel_path.trim_start_matches('/'))?.to_string();
    let arguments = arguments.trim_start().to_string();
    let initrd = image
        .find(
            initrd_line
                .split_whitespace()
                .next()?
                .trim_start_matches('/'),
        )?
        .to_string();
    if arguments.contains(['$', '`', '\\', ';']) {
        return None;
    }
    Some((kernel, initrd, arguments))
}

fn uos_kernel_arguments(image: &Image, kernel: &str, initrd: &str) -> Option<String> {
    for config in ["boot/grub/grub.cfg", "efi/boot/grub.cfg", "boot/grub.cfg"] {
        let Some(config) = image.find(config) else {
            continue;
        };
        let Ok(source) = image.read(config, 256 * 1024) else {
            continue;
        };
        let source = String::from_utf8_lossy(&source);
        let mut pending = None;
        for line in source.lines() {
            if let Some(value) = directive(line, "linux") {
                let mut words = value.splitn(2, char::is_whitespace);
                let path = words.next()?.trim_start_matches('/');
                pending = (image.find(path) == Some(kernel))
                    .then(|| words.next().unwrap_or("").trim_start().to_string());
            } else if let Some(value) = directive(line, "initrd") {
                let path = value.split_whitespace().next()?.trim_start_matches('/');
                if image.find(path) == Some(initrd) {
                    if let Some(arguments) = pending.take() {
                        if !arguments.contains(['$', '`', '\\', ';']) {
                            return Some(arguments);
                        }
                    }
                }
                pending = None;
            }
        }
    }
    None
}

fn merge_original_arguments(original: &str, additions: &str, family: &str) -> String {
    let source_keys: &[&str] = match family {
        "anaconda" => &["inst.repo=", "inst.stage2="],
        "casper" => &["url=", "iso-url=", "live-media=", "findiso="],
        "live-boot" => &["fetch=", "httpfs="],
        "debian-netboot" => &[
            "cdrom-detect/load_media=",
            "mirror/country=",
            "mirror/http/hostname=",
            "mirror/http/directory=",
            "mirror/http/proxy=",
        ],
        "suse" => &["install="],
        "alpine" => &["alpine_repo=", "modloop="],
        "archiso" => &["archiso_http_srv="],
        _ => &[],
    };
    let (original, tail, separator) = if let Some((head, tail)) = original.split_once(" --- ") {
        (head, Some(tail), " --- ")
    } else if let Some(head) = original.strip_suffix(" --") {
        (head, Some(""), " --")
    } else {
        (original, None, "")
    };
    let remote_source = original.split_whitespace().any(|word| {
        source_keys.iter().any(|key| word.starts_with(key))
            && (word.contains("http://") || word.contains("https://") || word.contains("nfs:"))
    });
    let mut words = original
        .split_whitespace()
        .filter(|word| {
            !source_keys.iter().any(|key| word.starts_with(key))
                || word.contains("http://")
                || word.contains("https://")
                || word.contains("nfs:")
        })
        .map(str::to_string)
        .collect::<Vec<_>>();
    for word in additions.split_whitespace() {
        if family == "casper"
            && word == "only-ubiquity"
            && original.split_whitespace().any(|arg| arg == "live")
        {
            continue;
        }
        if remote_source && source_keys.iter().any(|key| word.starts_with(key)) {
            continue;
        }
        let key = word.split_once('=').map(|(key, _)| key);
        if key.is_some_and(|key| {
            words
                .iter()
                .any(|item| item.starts_with(&format!("{key}=")))
        }) || words.iter().any(|item| item == word)
        {
            continue;
        }
        words.push(word.into());
    }
    let mut merged = words.join(" ");
    if let Some(tail) = tail {
        merged.push_str(separator);
        merged.push_str(tail);
    }
    merged
}

pub fn prepare(
    source: &Path,
    root: &Path,
    id: &str,
    trust_local_iso: bool,
    linux_compat: bool,
) -> Result<Option<ImportResult>, String> {
    let image = Image::open(source)?;
    let entries = image.entries();
    let mut candidates = Vec::new();
    let info = image
        .find(".disk/info")
        .and_then(|p| image.read(p, 65536).ok())
        .map(|b| String::from_utf8_lossy(&b).to_ascii_lowercase())
        .unwrap_or_default();
    let debian = info.contains("debian");
    let clonezilla = clonezilla_default(&image);
    let uos_desktop = (info.contains("uos") || info.contains("uniontech"))
        && image.find("live/filesystem.squashfs").is_some();
    let kylin_desktop = info.contains("kylin") && image.find("casper/initrd.lz").is_some();
    let casper_source = image.find(".disk/info").is_some()
        && (image.find("casper/filesystem.squashfs").is_some()
            || (image.find("casper/install-sources.yaml").is_some()
                && entries.keys().any(|p| p.starts_with("casper/") && p.ends_with(".squashfs"))));
    let anaconda_stage2 = image.find("images/install.img").is_some();
    let anaconda_repository = entries
        .keys()
        .any(|name| name == "repodata/repomd.xml" || name.ends_with("/repodata/repomd.xml"));
    let uos_server = [
        "boot/grub/grub.cfg",
        "efi/boot/grub.cfg",
        "isolinux/isolinux.cfg",
    ]
    .iter()
    .filter_map(|name| image.find(name))
    .filter_map(|name| image.read(name, 256 * 1024).ok())
    .any(|data| {
        let text = String::from_utf8_lossy(&data).to_ascii_lowercase();
        text.contains("uos server") || text.contains("uniontech os server")
    });
    let live_boot_config = [
        "boot/grub/grub.cfg",
        "boot/grub.cfg",
        "efi/boot/grub.cfg",
        "isolinux/live.cfg",
        "isolinux/isolinux.cfg",
        "syslinux.cfg",
    ]
    .iter()
    .filter_map(|name| image.find(name))
    .filter_map(|name| image.read(name, 256 * 1024).ok())
    .any(|data| String::from_utf8_lossy(&data).contains("boot=live"));
    let uos_default = if uos_desktop {
        ["boot/grub/grub.cfg", "efi/boot/grub.cfg", "boot/grub.cfg"]
            .iter()
            .filter_map(|path| image.find(path))
            .find_map(|path| crate::linux_fallback::original_grub_default(&image, path))
    } else {
        None
    };
    let uos_signed = uos_desktop && crate::uos_live::signed_grub(&image).is_some();
    let kylin_server = image
        .find(".treeinfo")
        .and_then(|p| image.read(p, 65536).ok())
        .is_some_and(|b| String::from_utf8_lossy(&b).contains("Kylin Linux Advanced Server"));
    let aosc_livekit = image.find("boot/kernel").is_some()
        && image.find("boot/live-initramfs.img").is_some()
        && image
            .find("boot/grub/grub.cfg")
            .and_then(|p| image.read(p, 65536).ok())
            .is_some_and(|b| String::from_utf8_lossy(&b).contains("aosc-livekit:CDLABEL="));
    let nixos_iso = nixos_iso_default(&image);
    let mut release_args = String::new();
    let mut unsigned_repository = false;
    let mut aliases = Vec::new();
    if let Some(release) = entries
        .keys()
        .find(|p| p.starts_with("dists/") && p.ends_with("/Release"))
        && let Ok(bytes) = image.read(release, 1024 * 1024)
    {
        unsigned_repository = debian
            && image.find(&format!("{release}.gpg")).is_none()
            && release
                .rsplit_once('/')
                .is_some_and(|(parent, _)| image.find(&format!("{parent}/InRelease")).is_none());
        let text = String::from_utf8_lossy(&bytes);
        for (field, key) in [("Suite:", "mirror/suite"), ("Codename:", "mirror/codename")] {
            if let Some(value) = text
                .lines()
                .find_map(|l| l.strip_prefix(field))
                .map(str::trim)
                .filter(|v| {
                    v.bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b".-_".contains(&c))
                })
            {
                release_args.push_str(&format!(" {key}={value}"));
                if let Some((target, _)) = release.rsplit_once('/') {
                    let from = format!("dists/{value}");
                    if from != target {
                        aliases.push((from, target.to_string()));
                    }
                }
                let alias = root.join(format!("dists/{value}/Release"));
                if !alias.exists() {
                    fs::create_dir_all(alias.parent().unwrap()).map_err(|e| e.to_string())?;
                    fs::write(alias, &bytes).map_err(|e| e.to_string())?;
                }
            }
        }
    }
    let companion = source
        .parent()
        .unwrap_or(Path::new("."))
        .join("netboot.tar.gz");
    let mut network_files = Vec::new();
    if debian
        && !linux_compat
        && companion.is_file()
        && !entries.keys().any(|name| {
            let parent = name.rsplit_once('/').map(|(p, _)| p).unwrap_or("");
            name.starts_with("install")
                && (name.ends_with("/vmlinuz") || name.ends_with("/vmlinux"))
                && image.find(&format!("{parent}/initrd.gz")).is_some()
        })
        && !["linux", "vmlinux", "vmlinuz"].iter().any(|p| image.find(p).is_some())
        && !entries.keys().any(|p| p.starts_with("debian-installer/"))
    {
        let cache = root.join("bootah-netboot");
        fs::create_dir_all(&cache).map_err(|e| e.to_string())?;
        crate::media::extract_external(&companion, &cache, 4)?;
        let tar = cache.join("netboot.tar");
        if tar.is_file() {
            crate::media::extract_external(&tar, &cache, 4)?;
            fs::remove_file(tar).map_err(|e| e.to_string())?;
        }
        fn walk(root: &Path, p: &Path, out: &mut Vec<String>) -> Result<(), String> {
            for e in fs::read_dir(p).map_err(|e| e.to_string())? {
                let e = e.map_err(|e| e.to_string())?;
                if e.file_type().map_err(|e| e.to_string())?.is_dir() {
                    walk(root, &e.path(), out)?
                } else if matches!(e.file_name().to_str(), Some("linux" | "vmlinux" | "vmlinuz"))
                    && e.path().parent().unwrap().join("initrd.gz").is_file()
                {
                    out.push(
                        e.path()
                            .strip_prefix(root)
                            .map_err(|e| e.to_string())?
                            .to_string_lossy()
                            .replace('\\', "/"),
                    );
                }
            }
            Ok(())
        }
        walk(root, &cache, &mut network_files)?;
        for kernel in &network_files {
            let parent = kernel.rsplit_once('/').map(|(p, _)| p).unwrap_or("");
            let initrd = if parent.is_empty() { "initrd.gz".into() } else { format!("{parent}/initrd.gz") };
            candidates.push(("debian-netboot",kernel.clone(),initrd,format!("ip=dhcp cdrom-detect/load_media=false mirror/country=manual mirror/http/hostname={{server}}:{{http_port}} mirror/http/directory=/boot/{{profile}} mirror/http/proxy={release_args}")));
        }
    }

    for name in entries.keys() {
        let lower = name.to_ascii_lowercase();
        let parent = name.rsplit_once('/').map(|(p, _)| p).unwrap_or("");
        let base = lower.rsplit('/').next().unwrap_or("");
        if base.ends_with("_sign") || base.ends_with(".sig") {
            continue;
        }
        let (family, initrd, args) = if casper_source
            && lower.starts_with("casper/")
            && (base.starts_with("vmlinuz") || base == "vmlinux")
        {
            (
                "casper",
                ["casper/initrd", "casper/initrd.lz", "casper/initrd.gz"]
                    .iter()
                    .find_map(|p| image.find(p)),
                if kylin_desktop {
                    "ip=dhcp boot=casper fsck.mode=skip only-ubiquity locale=zh_CN quiet splash audit=0 security= ignore_uuid lsm= url={base}/media.iso".to_string()
                } else {
                    "ip=dhcp boot=casper cloud-config-url=/dev/null url={base}/media.iso"
                        .to_string()
                },
            )
        } else if (anaconda_stage2 || image.find(".treeinfo").is_some() || uos_server)
            && matches!(lower.as_str(), "images/pxeboot/vmlinuz" | "ppc/ppc64/vmlinuz")
        {
            (
                "anaconda",
                image.find(&format!("{parent}/initrd.img")),
                if kylin_server {
                    "ip=dhcp inst.repo={base} inst.stage2={base} fpi_to_tail=off".into()
                } else if uos_server && !anaconda_repository {
                    "ip=dhcp inst.stage2={base}".into()
                } else {
                    "ip=dhcp inst.repo={base} inst.stage2={base}".into()
                },
            )
        } else if uos_server && lower == "vmlinuz" {
            (
                "anaconda",
                image.find("initrd.img"),
                if anaconda_repository {
                    "ip=dhcp inst.repo={base} inst.stage2={base}".into()
                } else {
                    "ip=dhcp inst.stage2={base}".into()
                },
            )
        } else if aosc_livekit && lower == "boot/kernel" {
            (
                "aosc-livekit",
                image.find("boot/live-initramfs.img"),
                "root=aosc-livekit:CDLABEL=AOSCINSTALL livekit.boot=livekit".into(),
            )
        } else if entries.keys().any(|p| p.starts_with("apks/"))
            && image
                .find(&format!(
                    "boot/modloop-{}",
                    base.trim_start_matches("vmlinuz-")
                ))
                .is_some()
            && lower.starts_with("boot/")
            && base.starts_with("vmlinuz-")
        {
            let flavor = base.trim_start_matches("vmlinuz-");
            (
                "alpine",
                image.find(&format!("{parent}/initramfs-{flavor}")),
                format!(
                    "ip=dhcp modules=loop,squashfs alpine_repo={{base}}/apks modloop={{base}}/boot/modloop-{flavor}"
                ),
            )
        } else if (image.find("content").is_some() || image.find("media.1/media").is_some())
            && lower.contains("/loader/")
            && base == "linux"
        {
            (
                "suse",
                image.find(&format!("{parent}/initrd")),
                "ifcfg=*=try,dhcp install={base}".into(),
            )
        } else if let Some((nixos_kernel, nixos_initrd, nixos_args)) = &nixos_iso
            && name == nixos_kernel
        {
            ("nixos-iso", image.find(nixos_initrd), nixos_args.clone())
        } else if entries
            .keys()
            .any(|p| p.starts_with("arch/") && p.ends_with("/airootfs.sfs"))
            && lower.starts_with("arch/boot/")
            && base.starts_with("vmlinuz")
        {
            (
                "archiso",
                image.find(&format!("{parent}/initramfs-linux.img")),
                "ip=dhcp archisobasedir=arch archiso_http_srv={base}/".into(),
            )
        } else if uos_default
            .as_ref()
            .is_some_and(|(kernel, _)| kernel == name)
        {
            let initrd = uos_default.as_ref().and_then(|(_, path)| image.find(path));
            (
                "live-boot",
                initrd,
                format!(
                    "boot=live components fetch={{base}}/live/filesystem.squashfs initrd={}",
                    initrd
                        .and_then(|p| p.rsplit('/').next())
                        .unwrap_or("initrd.img")
                ),
            )
        } else if (debian || clonezilla.is_some() || uos_desktop)
            && live_boot_config
            && image.find("live/filesystem.squashfs").is_some()
            && lower.starts_with("live/")
            && base.starts_with("vmlinuz")
        {
            let suffix = base.trim_start_matches("vmlinuz");
            let initrd = image
                .find(&format!("{parent}/initrd{suffix}"))
                .or_else(|| image.find(&format!("{parent}/initrd.img{suffix}")))
                .or_else(|| {
                    if uos_desktop && suffix == ".efi" {
                        image.find("live/initrd.lz")
                    } else {
                        None
                    }
                })
                .or_else(|| {
                    if uos_desktop {
                        image.find(&format!("{parent}/initrd{suffix}.lz"))
                    } else {
                        None
                    }
                })
                .or_else(|| image.find("live/initrd.img"));
            (
                "live-boot",
                initrd,
                format!(
                    "boot=live components fetch={{base}}/live/filesystem.squashfs initrd={}",
                    initrd
                        .and_then(|p| p.rsplit('/').next())
                        .unwrap_or("initrd.img")
                ),
            )
        } else if (lower.starts_with("debian-installer/") || (debian && parent.is_empty()))
            && matches!(base, "linux" | "vmlinux" | "vmlinuz")
        {
            (
                "debian-netboot",
                image.find(&if parent.is_empty() {
                    "initrd.gz".to_string()
                } else {
                    format!("{parent}/initrd.gz")
                }),
                if entries.keys().any(|p| p.starts_with("dists/")) {
                    format!(
                        "ip=dhcp cdrom-detect/load_media=false mirror/country=manual mirror/http/hostname={{server}}:{{http_port}} mirror/http/directory=/boot/{{profile}} mirror/http/proxy={release_args}"
                    )
                } else {
                    "ip=dhcp cdrom-detect/load_media=false".into()
                },
            )
        } else if debian
            && network_files.is_empty()
            && lower.starts_with("install")
            && !lower.contains("/gtk/")
            && (base == "vmlinuz" || base == "vmlinux")
        {
            (
                "debian-cd",
                image.find(&format!("{parent}/initrd.gz")),
                String::new(),
            )
        } else {
            continue;
        };
        if let Some(initrd) = initrd {
            if uos_desktop
                && family == "live-boot"
                && uos_kernel_arguments(&image, name, initrd).is_none()
            {
                continue;
            }
            candidates.push((family, name.clone(), initrd.to_string(), args))
        }
    }
    if let Some((default_kernel, _)) = &uos_default {
        candidates.sort_by_key(|(_, kernel, _, _)| kernel != default_kernel);
    }
    if linux_compat {
        candidates.retain(|(family, kernel, _, _)| {
            (matches!(*family, "debian-cd" | "anaconda" | "aosc-livekit")
                || (*family == "casper" && kylin_desktop))
                && !kernel.contains("/gtk/")
        });
    }
    if candidates.is_empty() {
        return Ok(None);
    }
    crate::media_source::register(source, root)?;
    for (from, to) in aliases {
        crate::media_source::alias(root, &from, &to)?;
    }
    let mut profiles = Vec::new();
    let mut warnings = Vec::new();
    for (index, (family, kernel, initrd, mut arguments)) in candidates.into_iter().enumerate() {
        let original = if family == "nixos-iso" {
            Some(("isolinux/isolinux.cfg".into(), arguments.clone()))
        } else if family == "live-boot"
            && clonezilla
                .as_ref()
                .is_some_and(|(k, i, _)| k == &kernel && i == &initrd)
        {
            clonezilla
                .as_ref()
                .map(|(_, _, args)| ("boot/grub/grub.cfg".into(), args.clone()))
        } else if uos_desktop && family == "live-boot" {
            uos_kernel_arguments(&image, &kernel, &initrd).map(|args| ("UOS GRUB".into(), args))
        } else {
            crate::linux_fallback::original_arguments(&image, &kernel, &initrd)
        };
        if let Some((_, original_arguments)) = &original {
            arguments = merge_original_arguments(original_arguments, &arguments, family);
        }
        let cached = root.join(format!("bootah-kernel-{index}"));
        if root.join(&kernel).is_file() {
            fs::copy(root.join(&kernel), &cached).map_err(|e| e.to_string())?;
        } else {
            image.materialize(&kernel, &cached)?;
        }
        let architecture = crate::media::binary_arch(&cached)
            .unwrap_or("unknown")
            .to_string();
        let automatic_compat = !linux_compat
            && matches!(family, "debian-cd" | "aosc-livekit")
            && matches!(architecture.as_str(), "x64" | "ppc64le");
        let linux_compat = linux_compat || automatic_compat;
        let mut note = format!("Detected {family}; original ISO retained, files served on demand.");
        if family == "debian-netboot" && !entries.keys().any(|p| p.starts_with("dists/")) {
            note.push_str(" This netboot medium needs an external package mirror; no repository is present in the ISO.")
        }
        if !network_files.is_empty() {
            note.push_str(" Companion netboot components must match this release and architecture.")
        }
        let trusted_repository =
            family == "debian-netboot" && unsigned_repository && trust_local_iso;
        if trusted_repository {
            arguments.push_str(" debian-installer/allow_unauthenticated=true");
            note.push_str(" User explicitly trusted this local ISO's unsigned repository. This profile permits unsigned installer metadata for its generated local mirror. If changing to an external mirror in the installer, restore signature verification first.");
        } else if family == "debian-netboot" && unsigned_repository {
            note.push_str(" The selected ISO repository lacks Release.gpg/InRelease. Import again with explicit local-ISO trust to use this repository; signature verification has not been disabled.")
        }
        if family == "nixos-iso" {
            note.push_str(" The ISO initrd requires the local nix-store.squashfs. NixOS documents a separate netboot ramdisk.");
        }
        if family == "anaconda" && !anaconda_stage2 {
            note.push_str(
                " The ISO has no images/install.img; configure a matching installer stage2 source.",
            )
        }
        if family == "anaconda" && !anaconda_repository {
            note.push_str(" No RPM repository index was found; the installer may require an external package source.")
        }
        if family == "anaconda" && uos_server {
            note.push_str(
                " UOS Server PXE parameters follow the official V20 (1070) installation manual.",
            )
        }
        let mut boot_files = BTreeMap::new();
        for &(asset, key) in crate::media::network_loaders(&architecture) {
            if let Some(path) = crate::media::asset(asset) {
                fs::copy(path, root.join(asset)).map_err(|e| e.to_string())?;
                boot_files.insert(key.into(), asset.into());
            }
        }
        if architecture == "ppc64le" {
            if let Some(path) = crate::media::asset("grub-powerpc.elf") {
                fs::copy(path, root.join("grub-powerpc.elf")).map_err(|e| e.to_string())?;
                boot_files.insert("powerpc-of".into(), "grub-powerpc.elf".into());
            }
            // Served dynamically; keep a file for profile validation and external tools.
            fs::write(root.join("bootah-petitboot"), "# Generated dynamically by Bootah\n")
                .map_err(|e| e.to_string())?;
            for key in ["power-opal", "powerpc-epapr", "default"] {
                boot_files.insert(key.into(), "bootah-petitboot".into());
            }
            if !boot_files.contains_key("powerpc-of") {
                warnings.push("SLOF/Open Firmware requires grub-powerpc.elf; run tools/power-boot/build.sh with GRUB >= 2.14.".into());
            }
        }
        let mut prepared_initrd = initrd.clone();
        if family == "casper" && !linux_compat {
            if let Some(nfs_arguments) = crate::casper_nfs::arguments(&arguments) {
                let cache = root.join(format!(".bootah-nfs-{index}"));
                fs::create_dir_all(&cache).map_err(|e| e.to_string())?;
                let original = cache.join("original-initrd");
                image.materialize(&initrd, &original)?;
                let supported = crate::casper_nfs::supported(&original, &cache);
                let _ = fs::remove_dir_all(&cache);
                if supported.unwrap_or(false) {
                    arguments = nfs_arguments;
                    note.push_str(" Native Casper NFS: the original initrd reads the ISO file tree on demand.");
                }
            }
        }
        if linux_compat {
            let cache = root.join(format!(".bootah-compat-{index}"));
            fs::create_dir_all(&cache).map_err(|e| e.to_string())?;
            let original = cache.join("original-initrd");
            image.materialize(&initrd, &original)?;
            let output_name = format!("bootah-compat-initrd-{index}");
            let output = root.join(&output_name);
            match family {
                "debian-cd" => crate::debian_compat::prepare(
                    &image,
                    &original,
                    &output,
                    &cache,
                    &architecture,
                )?,
                "anaconda" => crate::fedora_compat::prepare(
                    &image,
                    &original,
                    &output,
                    &cache,
                    &architecture,
                )?,
                "aosc-livekit" => {
                    crate::aosc_compat::prepare(&original, &output, &cache, &architecture)?
                }
                "casper" if kylin_desktop => {
                    crate::casper_compat::prepare(&original, &output, &cache, &architecture)?
                }
                _ => return Err("Unsupported Linux compatibility family".into()),
            }
            prepared_initrd = output_name;
            fs::write(
                root.join(".bootah-linux-compat.json"),
                br#"{"version":1,"transport":"nbd"}"#,
            )
            .map_err(|e| e.to_string())?;
            arguments = crate::linux_fallback::compatibility_arguments(&arguments, family);
            note.push_str(if automatic_compat {
                " Automatically selected original ISO compatibility after validating this image's kernel modules and initramfs layout; the target kernel reads the original ISO over read-only NBD."
            } else {
                " Explicit Linux compatibility mode: original ISO kernel and derived initramfs connect the read-only ISO using the target kernel's NBD client."
            });
        }
        if uos_desktop && family == "live-boot" && !linux_compat && uos_signed {
            let cache = root.join(format!(".bootah-uos-{index}"));
            fs::create_dir_all(&cache).map_err(|e| e.to_string())?;
            let original = cache.join("original-initrd");
            image.materialize(&initrd, &original)?;
            let output_name = format!("bootah-uos-initrd-{index}");
            crate::uos_live::prepare(&image, &original, &root.join(&output_name), &cache)?;
            if let Some(original_name) = initrd.rsplit('/').next() {
                arguments = arguments.replace(
                    &format!("initrd={original_name}"),
                    &format!("initrd={output_name}"),
                );
            }
            prepared_initrd = output_name;
            note.push_str(" UOS verifies signed files from its original media; the initrd fetches those unchanged files from the imported ISO before verification.");
        }

        if family == "alpine" {
            let flavor = kernel
                .rsplit('/')
                .next()
                .unwrap_or("")
                .trim_start_matches("vmlinuz-");
            if let Some(modloop) = image.find(&format!("boot/modloop-{flavor}")) {
                let temp = root.join(format!(".bootah-alpine-{index}"));
                fs::create_dir_all(&temp).map_err(|e| e.to_string())?;
                let result = (|| {
                    image.materialize(modloop, &temp.join("modloop"))?;
                    image.materialize(&initrd, &temp.join("initrd"))?;
                    let modules = temp.join("modules");
                    fs::create_dir_all(&modules).map_err(|e| e.to_string())?;
                    crate::media::extract_external(&temp.join("modloop"), &modules, 32)?;
                    let name = format!("bootah-initrd-{index}");
                    crate::initramfs::alpine(&temp.join("initrd"), &modules, &root.join(&name))?;
                    Ok::<_, String>(name)
                })();
                let _ = fs::remove_dir_all(temp);
                match result {
                    Ok(name) => prepared_initrd = name,
                    Err(e) => note.push_str(&format!(" Network initramfs preparation failed: {e}")),
                }
            }
        }
        if boot_files.is_empty() {
            note.push_str(" Matching firmware loader unavailable; use documented board-specific U-Boot configuration.")
        }
        let source_kind = if linux_compat {
            "nbd"
        } else if matches!(family, "aosc-livekit" | "debian-cd" | "nixos-iso") {
            "local-media"
        } else if arguments.contains("nfsroot={server}:/{profile}") {
            "nfs-tree"
        } else if arguments.contains("{base}/media.iso") {
            "http-iso"
        } else if arguments.contains("{base}") {
            "http-tree"
        } else {
            "external"
        };
        let mut diagnostics = Vec::new();
        if original.is_none() {
            diagnostics.push(BootDiagnostic {
                code: "BOOT_CONFIG_FAMILY_LAYOUT".into(),
                detail: family.into(),
            });
        }
        if family == "anaconda" && !anaconda_stage2 {
            diagnostics.push(BootDiagnostic {
                code: "SOURCE_STAGE2_MISSING".into(),
                detail: "images/install.img".into(),
            });
        }
        if family == "anaconda" && !anaconda_repository {
            diagnostics.push(BootDiagnostic {
                code: "SOURCE_REPOSITORY_MISSING".into(),
                detail: "repodata/repomd.xml".into(),
            });
        }
        if family == "anaconda" && uos_server {
            diagnostics.push(BootDiagnostic {
                code: "UOS_MANUAL_ONLY".into(),
                detail: "V20 (1070)".into(),
            });
        }
        if family == "debian-netboot" && !entries.keys().any(|p| p.starts_with("dists/")) {
            warnings.push("This image requires an external package mirror.".into());
        }
        if family == "debian-netboot" && unsigned_repository && !trusted_repository {
            warnings.push("The ISO repository has no signature. Enable local ISO trust to use this repository.".into());
        }
        if family == "nixos-iso" {
            warnings.push("This image requires a local nix-store.squashfs. Use the NixOS netboot ramdisk for network installation.".into());
        }
        if family == "anaconda" && !anaconda_stage2 {
            warnings.push("Missing installer file: images/install.img.".into());
        }
        if family == "anaconda" && !anaconda_repository {
            warnings.push(
                "No RPM repository index found. Configure an external package source.".into(),
            );
        }
        if boot_files.is_empty() {
            warnings.push(
                "Matching firmware loader unavailable. Configure a loader for this architecture."
                    .into(),
            );
        }
        let no_boot_files = boot_files.is_empty();
        profiles.push(Profile { secure_boot: None,
            system_id: String::new(),
            system_name: String::new(),
            id: format!("{id}-{index}"),
            name: format!(
                "{}  {family}  {architecture}",
                source.file_stem().unwrap_or_default().to_string_lossy()
            ),
            architecture,
            kind: "linux".into(),
            root: root.into(),
            boot_files,
            kernel: if linux_compat {
                kernel.clone()
            } else {
                format!("bootah-kernel-{index}")
            },
            initrd: prepared_initrd,
            dtb: String::new(),
            arguments,
            media: source.display().to_string(),
            status: if no_boot_files
                || (matches!(family, "debian-cd" | "aosc-livekit" | "nixos-iso") && !linux_compat)
                || (family == "anaconda" && (!anaconda_stage2 || !anaconda_repository))
                || (family == "debian-netboot" && unsigned_repository && !trusted_repository)
            {
                "needs-attention"
            } else {
                "prepared"
            }
            .into(),
            notes: note,
            boot_evidence: Some(BootEvidence {
                config: original
                    .as_ref()
                    .map(|(config, _)| config.clone())
                    .unwrap_or_else(|| "family layout".into()),
                entry: original
                    .as_ref()
                    .map(|(config, _)| crate::linux_fallback::default_title(&image, config))
                    .unwrap_or_else(|| family.into()),
                kernel,
                initrd,
                source: source_kind.into(),
                verification: "static".into(),
                diagnostics,
            }),
        });
    }
    Ok(Some(ImportResult { profiles, warnings }))
}
