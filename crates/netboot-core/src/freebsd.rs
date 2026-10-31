use crate::{media::ImportResult, media_source::Image, model::Profile};
use std::{collections::BTreeMap, fs, path::Path};

pub fn prepare(source: &Path, root: &Path, id: &str) -> Result<Option<ImportResult>, String> {
    let image = Image::open(source)?;
    if image.find("usr/sbin/bsdinstall").is_none()
        || image.find("boot/kernel/kernel").is_none()
        || (image.find("boot/loader").is_none() && image.find("boot/loader.efi").is_none())
    {
        return Ok(None);
    }
    let kernel = root.join("freebsd-kernel");
    image.materialize("boot/kernel/kernel", &kernel)?;
    let architecture = crate::media::binary_arch(&kernel).unwrap_or("unknown");
    if matches!(architecture, "x86" | "x64" | "arm64") {
        fs::remove_file(&kernel).map_err(|e| e.to_string())?;
        return native(&image, source, root, id, architecture).map(Some);
    }
    if architecture != "ppc64le" {
        fs::remove_file(kernel).map_err(|e| e.to_string())?;
        return Ok(None);
    }
    let mut legacy = fs::read(&kernel).map_err(|e| e.to_string())?;
    let legacy_available = legacy_entry(&mut legacy).is_ok();
    if legacy_available {
        fs::write(root.join("freebsd-legacy-kexec"), legacy).map_err(|e| e.to_string())?;
    }
    fs::remove_file(kernel).map_err(|e| e.to_string())?;
    crate::media_source::register(source, root)?;
    let mut boot_files = BTreeMap::new();
    fs::write(
        root.join("bootah-petitboot"),
        "# Generated dynamically by Bootah\n",
    )
    .map_err(|e| e.to_string())?;
    for key in ["power-opal", "default"] {
        boot_files.insert(key.into(), "bootah-petitboot".into());
    }
    let profile_id = format!("{id}-freebsd");
    let standard = Profile {
            secure_boot: None, system_id: "freebsd".into(), system_name: "FreeBSD".into(),
            id: profile_id, name: format!("{} (PowerNV)", source.file_stem().unwrap_or_default().to_string_lossy()),
            architecture: "ppc64le".into(), kind: "bsd".into(), root: root.into(),
            boot_files,
            kernel: "boot/kernel/kernel".into(), initrd: "media.iso".into(), dtb: String::new(), arguments: "vfs.root.mountfrom=cd9660:/dev/md0".into(),
            media: source.display().to_string(), status: "prepared".into(),
            notes: "FreeBSD PowerPC64 LE on PowerNV/Petitboot only: original kernel and original ISO as initrd, using the kernel MD_ROOT_MEM/Open Firmware initrd ABI. Requires guest memory for the ISO. Installation remains interactive.".into(),
            boot_evidence: None,
        };
    let mut profiles = vec![standard.clone()];
    if legacy_available {
        let mut compatibility = standard;
        compatibility.id.push_str("-legacy-kexec");
        compatibility.name.push_str(" (legacy kexec-lite)");
        compatibility.kernel = "freebsd-legacy-kexec".into();
        compatibility.status = "needs-attention".into();
        compatibility.notes.push_str(" Explicit workaround for the firmware kexec-lite entry-point regression documented by FreeBSD: only the derived ELF e_entry differs from the ISO kernel. Use the standard profile on fixed firmware; this profile is not selected automatically. Runtime validation of this alternate profile is pending.");
        profiles.push(compatibility);
    }
    Ok(Some(ImportResult {
        profiles,
        warnings: vec![if legacy_available {
            "PowerNV only. Choose the legacy kexec-lite profile only for the firmware regression documented by FreeBSD; pSeries/SLOF is not offered."
        } else {
            "PowerNV only. This kernel's ELF layout does not support the legacy kexec-lite workaround; only the standard profile is available."
        }.into()],
    }))
}

fn native(image: &Image, source: &Path, root: &Path, id: &str, architecture: &str) -> Result<ImportResult, String> {
    let mut boot_files = BTreeMap::new();
    if matches!(architecture, "x86" | "x64") && image.find("boot/pxeboot").is_some() {
        boot_files.insert("x86-bios".into(), "boot/pxeboot".into());
    }
    if let Some(loader) = image.find("boot/loader.efi") {
        let cached = root.join("freebsd-loader.efi");
        image.materialize(loader, &cached)?;
        if crate::media::binary_arch(&cached) == Some(architecture) {
            let key = crate::efi::for_arch(architecture).unwrap().firmware_key;
            boot_files.insert(key.into(), "freebsd-loader.efi".into());
        } else {
            fs::remove_file(cached).map_err(|e| e.to_string())?;
        }
    }
    crate::media_source::register(source, root)?;
    let profile_id = format!("{id}-freebsd");
    fs::create_dir_all(root.join("boot")).map_err(|e| e.to_string())?;
    let mut config = image.find("boot/loader.conf.local")
        .map(|p| image.read(p, 65536)).transpose()?.unwrap_or_default();
    config.extend_from_slice(if architecture == "x86" { b"\nnfs.read_size=\"1024\"\n" } else { b"\nnfs.read_size=\"8192\"\n" });
    config.extend_from_slice(b"mdroot_load=\"YES\"\nmdroot_type=\"md_image\"\nautoboot_delay=\"3\"\n");
    if architecture == "x86" {
        for module in ["boot/kernel/geom_uzip.ko", "boot/kernel/zlib.ko"] {
            if image.find(module).is_none() { return Err(format!("FreeBSD ISO lacks a matching compressed-disk module: {module}")); }
        }
        let cached = root.join("bootah-media.uzip");
        uzip(source, &cached)?;
        if fs::metadata(&cached).map_err(|e| e.to_string())?.len() > u64::from(u32::MAX) {
            return Err("FreeBSD i386 compressed image exceeds the 32-bit preload address space; use a smaller bootonly image.".into());
        }
        config.extend_from_slice(b"geom_uzip_load=\"YES\"\nmdroot_name=\"/bootah-media.uzip\"\nvfs.root.mountfrom=\"cd9660:/dev/md0.uzip\"\n");
    } else {
        config.extend_from_slice(b"mdroot_name=\"/media.iso\"\nvfs.root.mountfrom=\"cd9660:/dev/md0\"\n");
    }
    fs::write(root.join("boot/loader.conf.local"), config).map_err(|e| e.to_string())?;
    let marker = serde_json::json!({"version":1,"profile":profile_id,"architecture":architecture,"source":"iso-mdroot"});
    fs::write(root.join(".bootah-freebsd-nfs.json"), serde_json::to_vec(&marker).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let missing = boot_files.is_empty();
    let mut note = "FreeBSD native network loader: original kernel and ISO memory disk over read-only NFS. Requires NFS/portmapper and enough client memory for the entire ISO. Installation remains interactive.".to_string();
    if architecture == "x86" {
        note = "FreeBSD native network loader with the original GEOM_UZIP module and a compressed read-only ISO memory disk. Requires NFS/portmapper and enough client memory for the compressed image and kernel. Installation remains interactive. Use bootonly media on memory-limited 32-bit clients.".into();
    }
    let local_distribution = image.find("usr/freebsd-dist/base.txz").is_some()
        || (image.find("usr/freebsd-packages/offline/packagesite.pkg").is_some()
            && image.find("usr/freebsd-packages/repos/FreeBSD-base-offline.conf").is_some()
            && image.entries().keys().any(|p| p.starts_with("usr/freebsd-packages/offline/FreeBSD-runtime-") && p.ends_with(".pkg")));
    if !local_distribution {
        note.push_str(" This boot medium has no local distribution sets; select a matching FreeBSD mirror in bsdinstall.");
    }
    Ok(ImportResult {
        profiles: vec![Profile {
            secure_boot: None, system_id: "freebsd".into(), system_name: "FreeBSD".into(),
            id: profile_id, name: source.file_stem().unwrap_or_default().to_string_lossy().into(),
            architecture: architecture.into(), kind: "bsd".into(), root: root.into(), boot_files,
            kernel: "boot/kernel/kernel".into(), initrd: if architecture == "x86" { "bootah-media.uzip" } else { "media.iso" }.into(), dtb: String::new(), arguments: String::new(),
            media: source.to_string_lossy().into(), status: if missing { "needs-attention" } else { "prepared" }.into(),
            notes: note.clone(), boot_evidence: None,
        }],
        warnings: if missing { vec!["FreeBSD ISO lacks a matching native network loader".into()] } else { vec![note.into()] },
    })
}

// FreeBSD mkuzip CLOOP v2: 128-byte magic, BE geometry and BE block offsets.
fn uzip(source: &Path, output: &Path) -> Result<(), String> {
    use std::io::{Read, Seek, SeekFrom, Write};
    let mut input = fs::File::open(source).map_err(|e| e.to_string())?;
    let size = input.metadata().map_err(|e| e.to_string())?.len();
    let block_size = 16384u32;
    let blocks = u32::try_from(size.div_ceil(u64::from(block_size))).map_err(|_| "FreeBSD memory disk index exceeds cache budget")?;
    if blocks == 0 || blocks > 2 * 1024 * 1024 { return Err("FreeBSD memory disk index exceeds cache budget".into()); }
    let mut offsets = Vec::with_capacity(blocks as usize + 1);
    let start = 136 + (u64::from(blocks) + 1) * 8;
    let mut disk = std::io::BufWriter::new(fs::File::create(output).map_err(|e| e.to_string())?);
    disk.seek(SeekFrom::Start(start)).map_err(|e| e.to_string())?;
    let mut offset = start;
    let mut block = vec![0; block_size as usize];
    let mut encoded = Vec::with_capacity(block.len() * 2);
    let mut codec = flate2::Compress::new(flate2::Compression::default(), true);
    let mut remaining = size;
    for _ in 0..blocks {
        block.fill(0);
        let count = remaining.min(u64::from(block_size)) as usize;
        input.read_exact(&mut block[..count]).map_err(|e| e.to_string())?;
        remaining -= count as u64;
        encoded.clear();
        codec.reset();
        if codec.compress_vec(&block, &mut encoded, flate2::FlushCompress::Finish).map_err(|e| e.to_string())? != flate2::Status::StreamEnd {
            return Err("FreeBSD memory disk compression did not finish".into());
        }
        offsets.push(offset);
        disk.write_all(&encoded).map_err(|e| e.to_string())?;
        offset += encoded.len() as u64;
    }
    offsets.push(offset);
    let padding = (512 - offset % 512) % 512;
    disk.write_all(&vec![0; padding as usize]).map_err(|e| e.to_string())?;
    disk.rewind().map_err(|e| e.to_string())?;
    let mut magic = [0; 128];
    let prefix = b"#!/bin/sh\n#V2.0 Format\n";
    magic[..prefix.len()].copy_from_slice(prefix);
    disk.write_all(&magic).map_err(|e| e.to_string())?;
    disk.write_all(&block_size.to_be_bytes()).map_err(|e| e.to_string())?;
    disk.write_all(&blocks.to_be_bytes()).map_err(|e| e.to_string())?;
    for offset in offsets { disk.write_all(&offset.to_be_bytes()).map_err(|e| e.to_string())?; }
    disk.flush().map_err(|e| e.to_string())
}

pub fn network_root(profile: &Profile) -> bool {
    if profile.system_id != "freebsd" || profile.kind != "bsd" || !matches!(profile.architecture.as_str(), "x86" | "x64" | "arm64") {
        return false;
    }
    let Ok(bytes) = fs::read(profile.root.join(".bootah-freebsd-nfs.json")) else { return false; };
    let Ok(marker) = serde_json::from_slice::<serde_json::Value>(&bytes) else { return false; };
    marker["version"] == 1 && marker["profile"] == profile.id && marker["architecture"] == profile.architecture && marker["source"] == "iso-mdroot"
}

fn legacy_entry(bytes: &mut [u8]) -> Result<(), String> {
    if bytes.len() < 64
        || &bytes[..4] != b"\x7fELF"
        || bytes[4..9] != [2, 1, 1, 9, 0]
        || u16::from_le_bytes(bytes[18..20].try_into().unwrap()) != 21
    {
        return Err("Expected FreeBSD PPC64 LE ELF for legacy kexec-lite workaround".into());
    }
    let read = |offset: usize| u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap());
    let entry = read(24);
    let phoff = usize::try_from(read(32)).map_err(|_| "ELF header offset overflow")?;
    let size = usize::from(u16::from_le_bytes(bytes[54..56].try_into().unwrap()));
    let count = usize::from(u16::from_le_bytes(bytes[56..58].try_into().unwrap()));
    if size < 56
        || count == 0
        || count > 1024
        || size
            .checked_mul(count)
            .and_then(|n| phoff.checked_add(n))
            .is_none_or(|n| n > bytes.len())
    {
        return Err("Invalid FreeBSD ELF program headers".into());
    }
    let first = (0..count)
        .map(|i| phoff + i * size)
        .find(|&p| u32::from_le_bytes(bytes[p..p + 4].try_into().unwrap()) == 1)
        .ok_or("FreeBSD kernel has no loadable segment")?;
    let base = read(first + 16);
    let length = read(first + 40);
    let adjusted = entry
        .checked_sub(base)
        .filter(|e| *e < length)
        .ok_or("FreeBSD entry is outside the first loadable segment")?;
    bytes[24..32].copy_from_slice(&adjusted.to_le_bytes());
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn uzip_preserves_source_and_zero_pads_only_the_final_block() {
        use std::{fs, io::Read};
        let root = std::env::temp_dir().join(format!("bootah-uzip-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let source: Vec<u8> = (0..32905).map(|i| (i % 251) as u8).collect();
        fs::write(root.join("source"), &source).unwrap();
        super::uzip(&root.join("source"), &root.join("disk.uzip")).unwrap();
        let disk = fs::read(root.join("disk.uzip")).unwrap();
        assert_eq!(disk.len() % 512, 0);
        assert_eq!(&disk[11..13], b"V2");
        assert_eq!(u32::from_be_bytes(disk[128..132].try_into().unwrap()), 16384);
        assert_eq!(u32::from_be_bytes(disk[132..136].try_into().unwrap()), 3);
        let offsets: Vec<_> = disk[136..168].chunks_exact(8).map(|b| u64::from_be_bytes(b.try_into().unwrap()) as usize).collect();
        let mut decoded = Vec::new();
        for pair in offsets.windows(2) {
            flate2::read::ZlibDecoder::new(&disk[pair[0]..pair[1]]).read_to_end(&mut decoded).unwrap();
        }
        assert_eq!(&decoded[..source.len()], &source);
        assert!(decoded[source.len()..].iter().all(|b| *b == 0));
        assert_eq!(decoded.len(), 3 * 16384);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn legacy_kexec_changes_only_entry_and_rejects_invalid_headers() {
        let mut bytes = vec![0u8; 120];
        bytes[..9].copy_from_slice(b"\x7fELF\x02\x01\x01\x09\x00");
        bytes[18..20].copy_from_slice(&21u16.to_le_bytes());
        bytes[24..32].copy_from_slice(&0x101540u64.to_le_bytes());
        bytes[32..40].copy_from_slice(&64u64.to_le_bytes());
        bytes[54..56].copy_from_slice(&56u16.to_le_bytes());
        bytes[56..58].copy_from_slice(&1u16.to_le_bytes());
        bytes[64..68].copy_from_slice(&1u32.to_le_bytes());
        bytes[80..88].copy_from_slice(&0x100000u64.to_le_bytes());
        bytes[104..112].copy_from_slice(&0x200000u64.to_le_bytes());
        let original = bytes.clone();
        super::legacy_entry(&mut bytes).unwrap();
        assert_eq!(
            u64::from_le_bytes(bytes[24..32].try_into().unwrap()),
            0x1540
        );
        assert_eq!(&bytes[..24], &original[..24]);
        assert_eq!(&bytes[32..], &original[32..]);
        assert!(super::legacy_entry(&mut bytes).is_err());
        bytes[54..56].copy_from_slice(&65535u16.to_le_bytes());
        assert!(super::legacy_entry(&mut bytes).is_err());
    }
}
