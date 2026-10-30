use crate::{media_source::Image, model::Profile};
use std::path::Path;

pub fn detect(source: &Path, profiles: &[Profile]) -> (String, String) {
    let mut evidence = String::new();
    let mut disc_title = String::new();
    if let Ok(image) = Image::open(source) {
        for path in [
            ".disk/info",
            ".treeinfo",
            "README.diskdefines",
            "etc/os-release",
            "sources/idwbinfo.txt",
            "sources/cversion.ini",
        ] {
            if let Some(entry) = image.find(path) {
                if let Ok(bytes) = image.read(entry, 16 * 1024) {
                    let text = String::from_utf8_lossy(&bytes);
                    if path == ".disk/info" {
                        disc_title = text
                            .lines()
                            .next()
                            .unwrap_or("")
                            .trim()
                            .chars()
                            .take(80)
                            .collect();
                    }
                    evidence.push_str(&text.to_ascii_lowercase());
                    evidence.push(' ');
                }
            }
        }
    }
    let profile = profiles.first();
    let kind = profile.map(|p| p.kind.as_str()).unwrap_or("");
    let fallback = format!(
        "{} {}",
        source.file_name().unwrap_or_default().to_string_lossy(),
        profile.map(|p| p.name.as_str()).unwrap_or("")
    )
    .to_ascii_lowercase();
    if kind == "windows" || kind == "windows-pe-iso" || kind == "windows-ris" {
        let text = format!("{evidence} {fallback}").replace(['_', '-'], " ");
        let version = if text.contains("server 2025") {
            ("windowsserver", "Windows Server 2025")
        } else if text.contains("server 2022") {
            ("windowsserver", "Windows Server 2022")
        } else if text.contains("server 2019") {
            ("windowsserver", "Windows Server 2019")
        } else if text.contains("server 2016") {
            ("windowsserver", "Windows Server 2016")
        } else if text.contains("server 2008 r2") || text.contains("server2008r2") {
            ("windowsserver2008", "Windows Server 2008 R2")
        } else if text.contains("server 2008") || text.contains("server2008") {
            ("windowsserver2008", "Windows Server 2008")
        } else if text.contains("server 2003 r2") || text.contains("server2003r2") {
            ("windowsserver2003", "Windows Server 2003 R2")
        } else if text.contains("server 2003") || text.contains("server2003") {
            ("windowsserver2003", "Windows Server 2003")
        } else if text.contains("windows server") {
            ("windowsserver", "Windows Server")
        } else if text.contains("windows 11") || text.contains("win11") || text.contains("w11_") {
            ("windows11", "Windows 11")
        } else if text.contains("windows 10") || text.contains("win10") || text.contains("w10_") {
            ("windows10", "Windows 10")
        } else if text.contains("windows 8.1") || text.contains("win8.1") {
            ("windows8", "Windows 8.1")
        } else if text.contains("windows 8") || text.contains("win8") {
            ("windows8", "Windows 8")
        } else if text.contains("windows 7") || text.contains("win7") {
            ("windows7", "Windows 7")
        } else if text.contains("windows xp") || text.contains("winxp") {
            ("windowsxp", "Windows XP")
        } else {
            ("windows", "Windows")
        };
        return (version.0.into(), version.1.into());
    }
    let lookup = |text: &str| -> Option<(&'static str, &'static str)> {
        [
            ("linux mint", "linuxmint", "Linux Mint"),
            ("linuxmint", "linuxmint", "Linux Mint"),
            ("opensuse", "opensuse", "openSUSE"),
            ("suse linux", "opensuse", "SUSE Linux"),
            ("almalinux", "almalinux", "AlmaLinux"),
            ("rocky linux", "rockylinux", "Rocky Linux"),
            ("red hat", "redhat", "Red Hat Enterprise Linux"),
            ("centos", "centos", "CentOS"),
            ("fedora", "fedora", "Fedora"),
            ("ubuntu", "ubuntu", "Ubuntu"),
            ("debian", "debian", "Debian"),
            ("arch linux", "archlinux", "Arch Linux"),
            ("archlinux", "archlinux", "Arch Linux"),
            ("nixos", "nixos", "NixOS"),
            ("alpine", "alpinelinux", "Alpine Linux"),
            ("kylin", "kylin", "Kylin"),
            ("uniontech", "uos", "UOS"),
            ("uos server", "uos", "UOS"),
            ("aosc", "aosc", "AOSC OS"),
            ("netbsd", "netbsd", "NetBSD"),
            ("freebsd", "freebsd", "FreeBSD"),
            ("openbsd", "openbsd", "OpenBSD"),
            ("solaris", "solaris", "Solaris"),
            ("windows", "windows", "Windows"),
        ]
        .into_iter()
        .find(|(needle, _, _)| text.contains(needle))
        .map(|(_, id, name)| (id, name))
    };
    let identified = lookup(&evidence)
        .or_else(|| lookup(&fallback))
        .or_else(|| match kind {
            "windows" | "windows-pe-iso" => Some(("windows", "Windows")),
            "solaris" => Some(("solaris", "Solaris")),
            "bsd" => Some(("bsd", "BSD")),
            "linux" => Some(("linux", "Linux")),
            _ => None,
        })
        .unwrap_or(("unknown", "Unknown system"));
    let title = if !disc_title.is_empty()
        && disc_title
            .to_ascii_lowercase()
            .contains(&identified.1.to_ascii_lowercase())
    {
        disc_title
    } else {
        identified.1.into()
    };
    (identified.0.into(), title)
}

#[cfg(test)]
mod tests {
    use super::detect;
    use crate::model::Profile;
    use std::collections::BTreeMap;
    use std::path::Path;

    fn profile(kind: &str) -> Profile {
        Profile {
            id: String::new(),
            name: String::new(),
            system_id: String::new(),
            system_name: String::new(),
            architecture: String::new(),
            kind: kind.into(),
            root: Path::new("/").into(),
            boot_files: BTreeMap::new(),
            kernel: String::new(),
            initrd: String::new(),
            dtb: String::new(),
            arguments: String::new(),
            media: String::new(),
            status: String::new(),
            notes: String::new(),
            boot_evidence: None,
        }
    }

    #[test]
    fn classifies_filenames_without_claiming_unknown_distros() {
        assert_eq!(
            detect(Path::new("/tmp/linuxmint-22.iso"), &[]).0,
            "linuxmint"
        );
        assert_eq!(detect(Path::new("/tmp/mystery.iso"), &[]).0, "unknown");
        assert_eq!(
            detect(Path::new("/tmp/Win11_24H2.iso"), &[profile("windows")]).1,
            "Windows 11"
        );
        for (name, id, title) in [
            ("cn_windows_xp_sp3.iso", "windowsxp", "Windows XP"),
            ("cn_windows_7_sp1.iso", "windows7", "Windows 7"),
            (
                "en_windows_server_2003_r2.iso",
                "windowsserver2003",
                "Windows Server 2003 R2",
            ),
            (
                "en_windows_server_2008_r2.iso",
                "windowsserver2008",
                "Windows Server 2008 R2",
            ),
        ] {
            let identity = detect(Path::new(name), &[profile("windows")]);
            assert_eq!((identity.0.as_str(), identity.1.as_str()), (id, title));
        }
        assert_eq!(
            detect(Path::new("/tmp/FreeBSD-14.iso"), &[profile("bsd")]).0,
            "freebsd"
        );
        assert_eq!(
            detect(Path::new("/tmp/unknown.iso"), &[profile("linux")]).0,
            "linux"
        );
    }
}
