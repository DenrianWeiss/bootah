use crate::media_source::Image;

pub struct DefaultLoader {
    pub architecture: &'static str,
    pub firmware_key: &'static str,
    pub machine: u16,
    pub paths: &'static [&'static str],
}

pub const DEFAULT_LOADERS: &[DefaultLoader] = &[
    DefaultLoader {
        architecture: "x86",
        firmware_key: "ia32-efi",
        machine: 0x14c,
        paths: &["efi/boot/bootia32.efi"],
    },
    DefaultLoader {
        architecture: "x64",
        firmware_key: "x64-efi",
        machine: 0x8664,
        paths: &["efi/boot/bootx64.efi"],
    },
    DefaultLoader {
        architecture: "ia64",
        firmware_key: "ia64",
        machine: 0x200,
        paths: &["efi/boot/bootia64.efi"],
    },
    DefaultLoader {
        architecture: "arm32",
        firmware_key: "arm32-efi",
        machine: 0x1c2,
        paths: &["efi/boot/bootarm.efi"],
    },
    DefaultLoader {
        architecture: "arm64",
        firmware_key: "arm64-efi",
        machine: 0xaa64,
        paths: &["efi/boot/bootaa64.efi"],
    },
    DefaultLoader {
        architecture: "riscv64",
        firmware_key: "riscv64-efi",
        machine: 0x5064,
        paths: &["efi/boot/bootriscv64.efi"],
    },
    DefaultLoader {
        architecture: "loongarch64",
        firmware_key: "loongarch64-efi",
        machine: 0x6264,
        paths: &["efi/boot/bootloongarch64.efi", "efi/boot/bootla64.efi"],
    },
];

pub fn for_arch(architecture: &str) -> Option<&'static DefaultLoader> {
    DEFAULT_LOADERS
        .iter()
        .find(|entry| entry.architecture == architecture)
}

pub fn from_machine(machine: u16) -> Option<&'static DefaultLoader> {
    DEFAULT_LOADERS
        .iter()
        .find(|entry| entry.machine == machine)
}

pub fn matches_default(architecture: &str, path: &str, machine: u16) -> bool {
    for_arch(architecture).is_some_and(|entry| {
        entry.machine == machine
            && entry
                .paths
                .iter()
                .any(|name| name.eq_ignore_ascii_case(path))
    })
}

pub fn pe_machine(bytes: &[u8]) -> Option<u16> {
    if bytes.get(..2)? != b"MZ" {
        return None;
    }
    let offset = u32::from_le_bytes(bytes.get(60..64)?.try_into().ok()?) as usize;
    if bytes.get(offset..offset.checked_add(4)?)? != b"PE\0\0" {
        return None;
    }
    Some(u16::from_le_bytes(
        bytes.get(offset + 4..offset + 6)?.try_into().ok()?,
    ))
}

pub fn find_in_image<'a>(image: &'a Image, architecture: &str) -> Option<&'a str> {
    let entry = for_arch(architecture)?;
    entry.paths.iter().find_map(|path| {
        let found = image.find(path)?;
        let header = image.read_range(found, 0, 65536).ok()?;
        pe_machine(&header)
            .filter(|machine| matches_default(architecture, found, *machine))
            .map(|_| found)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_names_match_pe_machine_types() {
        for loader in DEFAULT_LOADERS {
            let mut bytes = vec![0; 128];
            bytes[..2].copy_from_slice(b"MZ");
            bytes[60..64].copy_from_slice(&64u32.to_le_bytes());
            bytes[64..68].copy_from_slice(b"PE\0\0");
            bytes[68..70].copy_from_slice(&loader.machine.to_le_bytes());
            assert_eq!(pe_machine(&bytes), Some(loader.machine));
            assert_eq!(
                from_machine(loader.machine).unwrap().architecture,
                loader.architecture
            );
            assert!(
                loader
                    .paths
                    .iter()
                    .all(|path| path.starts_with("efi/boot/"))
            );
        }
        assert_eq!(
            for_arch("loongarch64").unwrap().paths[1],
            "efi/boot/bootla64.efi"
        );
        assert!(matches_default(
            "loongarch64",
            "EFI/BOOT/BOOTLA64.EFI",
            0x6264
        ));
        assert!(!matches_default(
            "loongarch64",
            "EFI/BOOT/BOOTLA64.EFI",
            0x8664
        ));
        assert!(!matches_default("x64", "EFI/BOOT/BOOTLA64.EFI", 0x6264));
    }

    #[test]
    fn finds_loongarch_alias_in_indexed_media() {
        struct Cleanup(std::path::PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.0);
            }
        }
        let path = std::env::temp_dir().join(format!(
            "bootah-efi-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _cleanup = Cleanup(path.clone());
        let mut bytes = vec![0; 128];
        bytes[..2].copy_from_slice(b"MZ");
        bytes[60..64].copy_from_slice(&64u32.to_le_bytes());
        bytes[64..68].copy_from_slice(b"PE\0\0");
        bytes[68..70].copy_from_slice(&0x6264u16.to_le_bytes());
        std::fs::write(&path, &bytes).unwrap();
        let modified = path
            .metadata()
            .unwrap()
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let image: Image = serde_json::from_value(serde_json::json!({
            "source": path, "length": 128, "modified": modified, "external": false,
            "entries": {"EFI/BOOT/BOOTLA64.EFI": {"size": 128, "is_dir": false, "extents": [[0, 128]]}}
        }))
        .unwrap();
        assert_eq!(
            find_in_image(&image, "loongarch64"),
            Some("EFI/BOOT/BOOTLA64.EFI")
        );
        assert_eq!(find_in_image(&image, "x64"), None);
    }
}
