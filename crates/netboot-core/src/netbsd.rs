use crate::{media::ImportResult, media_source::Image, model::Profile};
use std::{collections::BTreeMap, fs, path::Path};

/// Recognize only installation ramdisks, never a disk-root GENERIC kernel.
pub fn prepare(source: &Path, root: &Path, id: &str) -> Result<Option<ImportResult>, String> {
    let image = Image::open(source)?;
    let paths: Vec<_> = image.entries().keys().cloned().collect();
    let mut profiles = Vec::new();
    let mut warnings = Vec::new();
    for (port, architecture) in [
        ("amd64", "x64"),
        ("i386", "x86"),
        ("evbarm-aarch64", "arm64"),
        ("riscv-riscv64", "riscv64"),
        ("alpha", "alpha"),
        ("sparc64", "sparc64"),
        ("macppc", "ppc"),
    ] {
        let instkernel = image
            .find(&format!("{port}/installation/instkernel/netbsd.gz"))
            .map(str::to_owned);
        let prefixes = [
            format!("{port}/installation/kernel/"),
            format!("{port}/binary/kernel/"),
        ];
        let separate_ramdisk = if architecture == "arm64" {
            image
                .find(&format!("{port}/installation/ramdisk/ramdisk.fs"))
                .map(str::to_owned)
        } else {
            None
        };
        let generic_with_ramdisk = separate_ramdisk
            .as_ref()
            .and_then(|_| image.find(&format!("{port}/binary/kernel/netbsd-GENERIC64.gz")))
            .map(str::to_owned);
        let kernel = paths
            .iter()
            .find(|p| {
                let p = p.to_ascii_lowercase();
                prefixes.iter().any(|prefix| p.starts_with(prefix))
                    && matches!(
                        p.rsplit('/').next(),
                        Some(
                            "netbsd-install.gz"
                                | "netbsd-install"
                                | "netbsd-install64.gz"
                                | "netbsd-install64"
                        )
                    )
            })
            .or(instkernel.as_ref())
            .or(generic_with_ramdisk.as_ref());
        let Some(kernel) = kernel else {
            if paths
                .iter()
                .any(|p| p.to_ascii_lowercase().starts_with(&format!("{port}/")))
            {
                warnings.push(format!("NetBSD {port}: no supported INSTALL ramdisk in image; GENERIC is not an installer"));
            }
            continue;
        };
        let profile_root = root.join(port);
        fs::create_dir_all(&profile_root).map_err(|e| e.to_string())?;
        let mut boot_files = BTreeMap::new();
        let loaders: &[(&str, &str)] = match architecture {
            "arm64" => &[("bootaa64.efi", "arm64-efi")],
            "riscv64" => &[("bootriscv64.efi", "riscv64-efi")],
            "alpha" => &[("netboot", "default")],
            "sparc64" => &[("ofwboot.net", "default")],
            "ppc" => &[("ofwboot.xcf", "default")],
            _ => &[
                ("pxeboot_ia32.bin", "x86-bios"),
                ("bootx64.efi", "x64-efi"),
                ("bootia32.efi", "ia32-efi"),
            ],
        };
        for &(suffix, key) in loaders {
            if (architecture == "x86" && key == "x64-efi")
                || (architecture == "x64" && key == "ia32-efi")
            {
                continue;
            }
            if let Some(path) = paths.iter().find(|p| {
                !image.entries().get(*p).is_some_and(|entry| entry.is_dir)
                    && p.to_ascii_lowercase().ends_with(&format!("/{suffix}"))
                    && (p.starts_with(&format!("{port}/"))
                        || p.to_ascii_lowercase().starts_with("efi/")
                        || p.to_ascii_lowercase().starts_with("usr/mdec/"))
            }) {
                image.materialize(path, &profile_root.join(suffix))?;
                boot_files.insert(key.into(), suffix.into());
            }
        }
        image.materialize(kernel, &profile_root.join("netbsd"))?;
        if architecture == "alpha" && kernel.to_ascii_lowercase().ends_with(".gz") {
            use std::io::Read;
            let input = fs::File::open(profile_root.join("netbsd")).map_err(|e| e.to_string())?;
            let mut decoder = flate2::read::GzDecoder::new(input).take(128 * 1024 * 1024 + 1);
            let temporary = profile_root.join("netbsd.expanded");
            let result = (|| {
                let mut output = fs::File::create(&temporary).map_err(|e| e.to_string())?;
                let size = std::io::copy(&mut decoder, &mut output).map_err(|e| e.to_string())?;
                if size > 128 * 1024 * 1024 {
                    return Err("NetBSD installation kernel exceeds cache budget".to_string());
                }
                drop(output);
                fs::copy(&temporary, profile_root.join("netbsd")).map_err(|e| e.to_string())?;
                fs::remove_file(&temporary).map_err(|e| e.to_string())
            })();
            if result.is_err() {
                let _ = fs::remove_file(temporary);
            }
            result?;
        }
        if let Some(ramdisk) = &separate_ramdisk {
            image.materialize(ramdisk, &profile_root.join("ramdisk.fs"))?;
        }
        if matches!(architecture, "x64" | "x86" | "arm64" | "riscv64") {
            fs::write(
                profile_root.join("boot.cfg"),
                if separate_ramdisk.is_some() {
                    "menu=Install NetBSD:initrd ramdisk.fs;boot netbsd\ntimeout=0\n"
                } else {
                    "menu=Install NetBSD:boot tftp:netbsd\ntimeout=0\n"
                },
            )
            .map_err(|e| e.to_string())?;
        }
        if matches!(architecture, "alpha" | "sparc64" | "ppc") {
            warnings.push(format!("NetBSD {port}: native loader uses the prepared NFS export; RPC/NFS service must be reachable"));
        }
        if matches!(architecture, "alpha" | "sparc64" | "ppc") {
            let manifest = serde_json::json!({"version": 1, "profile": format!("{id}-{port}"), "architecture": architecture, "port": port});
            fs::write(
                profile_root.join(".bootah-netbsd-nfs.json"),
                serde_json::to_vec(&manifest).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
        }
        let sets = paths
            .iter()
            .any(|p| p.starts_with(&format!("{port}/binary/sets/")));
        let note = format!(
            "NetBSD {port} INSTALL ramdisk. Installation sets: {}. Select the ISO HTTP tree /{port}/binary/sets/ in sysinst; disk selection remains interactive.",
            if sets {
                "present"
            } else {
                "absent (external matching release required)"
            }
        );
        if boot_files.is_empty() {
            warnings.push(format!(
                "NetBSD {port}: ISO lacks a matching network loader"
            ));
        }
        if !sets {
            warnings.push(format!("NetBSD {port}: ISO lacks installation sets"));
        }
        profiles.push(Profile { secure_boot: None,
            system_id: String::new(),
            system_name: String::new(),
            id: format!("{id}-{port}"),
            name: format!("NetBSD {port}"),
            architecture: architecture.into(),
            kind: "bsd".into(),
            root: profile_root,
            boot_files,
            kernel: "netbsd".into(),
            initrd: if separate_ramdisk.is_some() {
                "ramdisk.fs".into()
            } else {
                String::new()
            },
            dtb: String::new(),
            arguments: String::new(),
            media: source.to_string_lossy().into_owned(),
            status: "prepared".into(),
            notes: note,
            boot_evidence: None,
        });
    }
    Ok((!profiles.is_empty()).then_some(ImportResult { profiles, warnings }))
}
