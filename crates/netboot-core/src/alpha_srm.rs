use crate::{media::ImportResult, model::*};
use std::{collections::BTreeMap, fs, io::Read, path::Path};

const LOADER: &[u8] = include_bytes!("../../../assets/bootah-srm.bin");
const LIMIT: u64 = 512 * 1024 * 1024;

fn unpack_kernel(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut kernel = Vec::new();
    if bytes.starts_with(&[0x1f, 0x8b]) {
        flate2::read::GzDecoder::new(bytes)
            .take(LIMIT + 1)
            .read_to_end(&mut kernel)
            .map_err(|e| e.to_string())?;
    } else {
        kernel.extend_from_slice(bytes);
    }
    if kernel.len() as u64 > LIMIT
        || kernel.get(..7) != Some(b"\x7fELF\x02\x01\x01")
        || kernel.get(18..20) != Some(&[0x26, 0x90])
    {
        return Err("SRM loader requires an Alpha ELF64 little-endian Linux kernel".into());
    }
    Ok(kernel)
}

pub fn import_netboot(
    source: &Path,
    root: &Path,
    id: &str,
) -> Result<Option<ImportResult>, String> {
    let size = fs::metadata(source).map_err(|e| e.to_string())?.len();
    if size > LIMIT {
        return Err("Alpha netboot image exceeds 512 MiB".into());
    }
    let image = fs::read(source).map_err(|e| e.to_string())?;
    if image.get(0x60..0x68) != Some(b"aboot-00") {
        return Ok(None);
    }
    let mut found = None;
    for offset in (512..image.len().min(1024 * 1024)).step_by(512) {
        let Some(header) = image.get(offset..offset + 12) else {
            break;
        };
        let field = |i| u32::from_le_bytes(header[i..i + 4].try_into().unwrap()) as usize;
        let (header_size, kernel_size, initrd_size) = (field(0), field(4), field(8));
        if !(12..=212).contains(&header_size)
            || kernel_size < 64
            || initrd_size == 0
            || kernel_size > image.len()
            || initrd_size > image.len()
        {
            continue;
        }
        let kernel_offset = (offset + header_size).next_multiple_of(512);
        let Some(initrd_offset) = kernel_offset
            .checked_add(kernel_size)
            .map(|n| n.next_multiple_of(512))
        else {
            continue;
        };
        if initrd_offset.checked_add(initrd_size) != Some(image.len()) {
            continue;
        }
        let Some(compressed) = image.get(kernel_offset..kernel_offset + kernel_size) else {
            continue;
        };
        let Ok(kernel) = unpack_kernel(compressed) else {
            continue;
        };
        if found.is_some() {
            return Err("Ambiguous aboot payload layout".into());
        }
        let Some(argument_bytes) = image.get(offset + 12..offset + header_size) else {
            continue;
        };
        let arguments = if argument_bytes.is_empty() {
            String::new()
        } else {
            let Some(end) = argument_bytes.iter().position(|b| *b == 0) else {
                continue;
            };
            let Ok(value) = std::str::from_utf8(&argument_bytes[..end]) else {
                continue;
            };
            if value.contains(['\n', '\r']) {
                continue;
            }
            value.to_string()
        };
        found = Some((kernel, initrd_offset, arguments));
    }
    let Some((kernel, initrd_offset, arguments)) = found else {
        return Ok(None);
    };
    let initrd = &image[initrd_offset..];
    // Debian installer identification; the original initrd is served unchanged.
    let mut archive = Vec::new();
    if !initrd.starts_with(&[0x1f, 0x8b]) {
        return Ok(None);
    }
    flate2::read::GzDecoder::new(initrd)
        .take(LIMIT + 1)
        .read_to_end(&mut archive)
        .map_err(|e| e.to_string())?;
    if archive.len() as u64 > LIMIT
        || !archive.starts_with(b"070701")
        || !archive
            .windows(b"debian-installer".len())
            .any(|b| b == b"debian-installer")
    {
        return Ok(None);
    }
    fs::write(root.join("bootah-srm.bin"), LOADER).map_err(|e| e.to_string())?;
    fs::write(root.join("vmlinux"), kernel).map_err(|e| e.to_string())?;
    fs::write(root.join("initrd.gz"), initrd).map_err(|e| e.to_string())?;
    let notes = "Debian Alpha: small SRM loader with separate original kernel/initrd; platform validation recorded in docs/alpha-srm.md".to_string();
    Ok(Some(ImportResult {
        profiles: vec![Profile {
            secure_boot: None, system_id: "debian".into(), system_name: "Debian".into(),
            id: format!("{id}-0"), name: "Debian Alpha (SRM)".into(),
            architecture: "alpha".into(), kind: "linux".into(), root: root.into(),
            boot_files: BTreeMap::from([("default".into(), "bootah-srm.bin".into())]),
            kernel: "vmlinux".into(), initrd: "initrd.gz".into(), dtb: String::new(),
            arguments, media: source.file_name().unwrap_or_default().to_string_lossy().into(),
            status: "prepared".into(), notes, boot_evidence: None,
        }],
        warnings: vec!["SRM loader compatibility must be verified for the target firmware; ES40 evidence does not certify other platforms".into()],
    }))
}

pub fn import_iso(source: &Path, root: &Path, id: &str) -> Result<Option<ImportResult>, String> {
    let image = crate::media_source::Image::open(source)?;
    let Ok(info) = image.read(".disk/info", 65536) else {
        return Ok(None);
    };
    if !info.starts_with(b"Debian") || !image.entries().contains_key("boot/initrd.gz") {
        return Ok(None);
    }
    let Ok(compressed) = image.read("boot/vmlinuz", LIMIT as usize) else {
        return Ok(None);
    };
    let Ok(kernel) = unpack_kernel(&compressed) else {
        return Ok(None);
    };
    crate::linux_compat::client("alpha")?;
    let cache = root.join(".bootah-alpha-cd");
    fs::create_dir_all(&cache).map_err(|e| e.to_string())?;
    let original = cache.join("original-initrd.gz");
    image.materialize("boot/initrd.gz", &original)?;
    crate::debian_compat::prepare(&image, &original, &root.join("initrd.gz"), &cache, "alpha")?;
    crate::media_source::register(source, root)?;
    fs::write(root.join("bootah-srm.bin"), LOADER).map_err(|e| e.to_string())?;
    fs::write(root.join("vmlinux"), kernel).map_err(|e| e.to_string())?;
    fs::write(
        root.join(".bootah-linux-compat.json"),
        br#"{"version":1,"transport":"nbd"}"#,
    )
    .map_err(|e| e.to_string())?;
    Ok(Some(ImportResult {
        profiles: vec![Profile {
            secure_boot: None, system_id: "debian".into(), system_name: "Debian".into(),
            id: format!("{id}-0"), name: "Debian Alpha CD installer (SRM/NBD)".into(),
            architecture: "alpha".into(), kind: "linux".into(), root: root.into(),
            boot_files: BTreeMap::from([("default".into(), "bootah-srm.bin".into())]),
            kernel: "vmlinux".into(), initrd: "initrd.gz".into(), dtb: String::new(),
            arguments: "bootah.media=nbd bootah.server={server} bootah.port={nbd_port} bootah.export={profile}".into(),
            media: source.to_string_lossy().into(), status: "prepared".into(),
            notes: "Original Debian Alpha CD kernel and initrd with matching ISO NIC/NBD modules and EV56 media client; read-only original ISO via NBD".into(),
            boot_evidence: None,
        }],
        warnings: vec!["Alpha ISO installation requires the NBD service; validate stock SRM and installer on the target platform".into()],
    }))
}

pub fn enabled(profile: &Profile) -> bool {
    profile.architecture == "alpha"
        && profile.kind == "linux"
        && profile
            .boot_files
            .get("default")
            .is_some_and(|f| f == "bootah-srm.bin")
}

pub fn config(
    profile: &Profile,
    settings: &Settings,
    auto: Option<&AutoConfig>,
) -> Option<Vec<u8>> {
    if !enabled(profile) {
        return None;
    }
    let kernel = crate::media_source::open(&profile.root, &profile.kernel).ok()?;
    let initrd = crate::media_source::open(&profile.root, &profile.initrd).ok()?;
    let args = crate::automation::arguments(profile, settings, auto);
    if args.len() > 255 || args.contains(['\n', '\r', '\0']) {
        return None;
    }
    Some(
        format!(
            "BOOTAH-SRM 1\nkernel={}\nkernel_size={}\ninitrd={}\ninitrd_size={}\nargs={}\n",
            profile.kernel, kernel.size, profile.initrd, initrd.size, args
        )
        .into_bytes(),
    )
}
