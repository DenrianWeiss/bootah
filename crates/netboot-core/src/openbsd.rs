use crate::{media::ImportResult, media_source::Image, model::Profile};
use std::{collections::BTreeMap, fs, path::Path};

pub fn prepare(source: &Path, root: &Path, id: &str) -> Result<Option<ImportResult>, String> {
    let image = Image::open(source)?;
    if let Some(kernel) = image.entries().keys().find(|p| p.as_str() == "bsd.rd" || p.ends_with("/bsd.rd")) {
        let cached = root.join("openbsd-ramdisk");
        image.materialize(kernel, &cached)?;
        let architecture = crate::media::binary_arch(&cached).unwrap_or("unknown");
        fs::remove_file(&cached).map_err(|e| e.to_string())?;
        if architecture == "sparc64" {
            return sparc64(&image, source, root, id, kernel).map(Some);
        }
        if matches!(architecture, "x86" | "x64" | "arm64") {
            return native(&image, source, root, id, kernel, architecture).map(Some);
        }
    }
    let Some(kernel) = image.entries().keys().find(|p| {
        p.ends_with("/powerpc64/bsd.rd")
            || (p.as_str() == "bsd.rd"
                && image
                    .entries()
                    .keys()
                    .any(|n| n.ends_with("INSTALL.powerpc64")))
    }) else {
        if image.entries().keys().any(|p| p.as_str() == "bsd.rd" || p.ends_with("/bsd.rd")) {
            return Err("OpenBSD installation media was recognized, but its architecture is not supported by this importer.".into());
        }
        return Ok(None);
    };
    crate::media_source::register(source, root)?;
    image.materialize(kernel, &root.join("bsd.rd"))?;
    if crate::media::binary_arch(&root.join("bsd.rd")) != Some("ppc64") {
        return Err("OpenBSD powerpc64 requires its original big-endian ELF ramdisk kernel".into());
    }
    // Match OpenBSD's own Petitboot GRUB entry: kernel and initrd are the same file.
    fs::write(
        root.join("bootah-petitboot"),
        "DEFAULT install\nLABEL install\n KERNEL bsd.rd\n INITRD bsd.rd\n",
    )
    .map_err(|e| e.to_string())?;
    Ok(Some(ImportResult {
        profiles: vec![Profile {
            secure_boot: None, system_id: "openbsd".into(), system_name: "OpenBSD".into(),
            id: format!("{id}-openbsd"), name: "OpenBSD powerpc64 (PowerNV)".into(),
            architecture: "ppc64".into(), kind: "bsd".into(), root: root.into(),
            boot_files: BTreeMap::from([("power-opal".into(),"bootah-petitboot".into()),("default".into(),"bootah-petitboot".into())]),
            kernel: "bsd.rd".into(), initrd: "bsd.rd".into(), dtb: String::new(), arguments: String::new(),
            media: source.display().to_string(), status: "prepared".into(),
            notes: "OpenBSD powerpc64: original bsd.rd for PowerNV/Petitboot only. PowerVM/pSeries is not supported by this OpenBSD port. Documentation-based adaptation; not boot-tested.".into(),
            boot_evidence: None,
        }], warnings: vec!["OpenBSD powerpc64 adaptation follows official documentation; runtime acceptance pending.".into()],
    }))
}

fn native(image: &Image, source: &Path, root: &Path, id: &str, kernel: &str, architecture: &str) -> Result<ImportResult, String> {
    use std::io::{Cursor, Read};
    crate::media_source::register(source, root)?;
    image.materialize(kernel, &root.join("bsd.rd"))?;
    let mut boot_files = BTreeMap::new();
    let mut warnings = Vec::new();
    let parent = kernel.rsplit_once('/').map(|(p, _)| p).unwrap_or("");
    if matches!(architecture, "x86" | "x64") {
        let port = if architecture == "x64" { "amd64" } else { "i386" };
        if let Some(path) = image.entries().keys().find(|p| {
            p.rsplit('/').next() == Some("pxeboot") &&
                (p.as_str() == "pxeboot" || p.starts_with(&format!("{parent}/")) || p.as_str() == "usr/mdec/pxeboot")
        }) {
            image.materialize(path, &root.join("pxeboot"))?;
            boot_files.insert("x86-bios".into(), "pxeboot".into());
        } else {
            warnings.push(format!("OpenBSD {port}: ISO lacks the release-matched PXE loader; BIOS network boot is unavailable"));
        }
    }
    let loader = crate::efi::for_arch(architecture).unwrap();
    let filename = loader.paths[0].rsplit('/').next().unwrap();
    let output = root.join(filename);
    if let Some(path) = image.entries().keys().find(|p| p.to_ascii_lowercase().ends_with(&format!("/{filename}")) || p.eq_ignore_ascii_case(filename)) {
        image.materialize(path, &output)?;
    } else {
        // OpenBSD's ISO Makefiles place the original EFI loader in a small FAT image.
        for path in image.entries().keys().filter(|p| p.ends_with("/eficdboot") || p.ends_with("/cdbr")) {
            let bytes = image.read(path, 8 * 1024 * 1024)?;
            let mut spans = vec![(0, bytes.len())];
            if bytes.get(510..512) == Some(&[0x55, 0xaa]) {
                for record in bytes[446..510].chunks_exact(16) {
                    if matches!(record[4], 0x01 | 0x04 | 0x06 | 0x0b | 0x0c | 0x0e | 0xef) {
                        let start = (u32::from_le_bytes(record[8..12].try_into().unwrap()) as usize).checked_mul(512);
                        let size = (u32::from_le_bytes(record[12..16].try_into().unwrap()) as usize).checked_mul(512);
                        if let (Some(start), Some(size)) = (start, size)
                            && size != 0
                            && let Some(end) = start.checked_add(size).filter(|end| *end <= bytes.len())
                        {
                            spans.push((start, end));
                        }
                    }
                }
            }
            for (start, end) in spans {
                let Some(partition) = bytes.get(start..end) else { continue; };
                let Ok(fat) = fatfs::FileSystem::new(Cursor::new(partition.to_vec()), fatfs::FsOptions::new()) else { continue; };
                let directory = fat.root_dir();
                if let Ok(file) = directory.open_file(&format!("efi/boot/{filename}")) {
                    let mut content = Vec::new();
                    file.take(2 * 1024 * 1024 + 1).read_to_end(&mut content).map_err(|e| e.to_string())?;
                    if content.len() > 2 * 1024 * 1024 { return Err("OpenBSD EFI loader exceeds cache budget".into()); }
                    fs::write(&output, content).map_err(|e| e.to_string())?;
                    break;
                }
            }
            if output.is_file() { break; }
        }
    }
    if output.is_file() {
        if crate::media::binary_arch(&output) != Some(architecture) {
            return Err("OpenBSD EFI loader architecture mismatch".into());
        }
        boot_files.insert(loader.firmware_key.into(), filename.into());
    }
    let missing = boot_files.is_empty();
    if missing { warnings.push("OpenBSD ISO lacks a matching network loader".into()); }
    let sets = image.entries().keys().any(|p| {
        (parent.is_empty() || p.starts_with(&format!("{parent}/")))
            && p.rsplit('/').next().is_some_and(|base| base.starts_with("base") && base.ends_with(".tgz"))
    });
    if !sets { warnings.push("OpenBSD ISO lacks installation sets; an external matching release is required.".into()); }
    fs::create_dir_all(root.join("etc")).map_err(|e| e.to_string())?;
    fs::write(root.join("etc/boot.conf"), "boot tftp:bsd.rd\n").map_err(|e| e.to_string())?;
    Ok(ImportResult { profiles: vec![Profile {
        secure_boot: None, system_id: "openbsd".into(), system_name: "OpenBSD".into(),
        id: format!("{id}-openbsd"), name: source.file_stem().unwrap_or_default().to_string_lossy().into(),
        architecture: architecture.into(), kind: "bsd".into(), root: root.into(), boot_files,
        kernel: "bsd.rd".into(), initrd: String::new(), dtb: String::new(), arguments: String::new(),
        media: source.to_string_lossy().into(), status: if missing { "needs-attention" } else { "prepared" }.into(),
        notes: if sets {
            "OpenBSD original installation ramdisk. Select the ISO HTTP file tree for installation sets; disk selection remains interactive."
        } else {
            "OpenBSD original installation ramdisk. Installation sets are absent; select a matching external mirror in the installer."
        }.into(),
        boot_evidence: None,
    }], warnings })
}


pub fn network_root(p: &Profile) -> bool {
    if p.system_id != "openbsd" || p.architecture != "sparc64" || p.kind != "bsd" { return false; }
    let Ok(bytes) = fs::read(p.root.join(".bootah-openbsd-nfs.json")) else { return false; };
    let Ok(marker) = serde_json::from_slice::<serde_json::Value>(&bytes) else { return false; };
    marker["version"] == 1 && marker["profile"].as_str() == Some(&p.id)
}

fn sparc64(image: &Image, source: &Path, root: &Path, id: &str, kernel: &str) -> Result<ImportResult, String> {
    use std::io::Read;
    let parent = kernel.rsplit_once('/').map(|(p, _)| p).unwrap_or("");
    let in_port = |name: &str| if parent.is_empty() { name.into() } else { format!("{parent}/{name}") };
    if image.find(&in_port("INSTALL.sparc64")).is_none() {
        return Err("OpenBSD SPARC64 installation notes are missing".into());
    }
    crate::media_source::register(source, root)?;
    image.materialize(kernel, &root.join("bsd.rd"))?;
    let mut loader = None;
    if let Some(path) = image.find(&in_port("ofwboot.net")) {
        loader = Some(image.read(path, 2 * 1024 * 1024)?);
    } else if let Some(base) = image.entries().keys().find(|p| {
        p.rsplit_once('/').map(|(dir, _)| dir).unwrap_or("") == parent
            && p.rsplit('/').next().is_some_and(|n| n.starts_with("base") && n.ends_with(".tgz"))
    }) {
        let source = crate::media_source::open(root, base)?;
        let gzip = flate2::read::GzDecoder::new(source).take(1024 * 1024 * 1024);
        let mut archive = tar::Archive::new(gzip);
        for entry in archive.entries().map_err(|e| e.to_string())? {
            let mut entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path().map_err(|e| e.to_string())?;
            if path.components().filter(|p| *p != std::path::Component::CurDir).collect::<Vec<_>>()
                != Path::new("usr/mdec/ofwboot.net").components().collect::<Vec<_>>() { continue; }
            if !entry.header().entry_type().is_file() || entry.size() > 2 * 1024 * 1024 {
                return Err("Invalid OpenBSD SPARC64 network loader".into());
            }
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
            loader = Some(bytes);
            break;
        }
    }
    let id = format!("{id}-openbsd-sparc64");
    let mut files = BTreeMap::new();
    let mut warnings = Vec::new();
    if let Some(bytes) = loader {
        if bytes.get(..7) != Some(b"\x7fELF\x02\x02\x01") || crate::media::binary_arch_bytes(&bytes) != Some("sparc64") {
            return Err("Invalid OpenBSD SPARC64 network loader".into());
        }
        fs::write(root.join("ofwboot.net"), bytes).map_err(|e| e.to_string())?;
        files.insert("default".into(), "ofwboot.net".into());
        files.insert("sparc64".into(), "ofwboot.net".into());
        fs::write(root.join(".bootah-openbsd-nfs.json"),
            serde_json::to_vec(&serde_json::json!({"version":1,"profile":id})).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    } else {
        warnings.push("OpenBSD SPARC64: release-matched ofwboot.net is missing; use installation media containing the base set.".into());
    }
    let missing = files.is_empty();
    Ok(ImportResult { profiles: vec![Profile {
        secure_boot: None, system_id: "openbsd".into(), system_name: "OpenBSD".into(),
        id, name: "OpenBSD sparc64".into(), architecture: "sparc64".into(), kind: "bsd".into(),
        root: root.into(), boot_files: files, kernel: "bsd.rd".into(), initrd: String::new(), dtb: String::new(), arguments: String::new(),
        media: source.to_string_lossy().into(), status: if missing { "needs-attention" } else { "prepared" }.into(),
        notes: crate::sparc_obp::OPENBSD_NOTES.into(),
        boot_evidence: None,
    }], warnings })
}
