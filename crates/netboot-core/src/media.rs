use crate::model::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

#[derive(Deserialize)]
pub struct ImportRequest {
    pub path: PathBuf,
    #[serde(default)]
    pub trust_local_iso: bool,
    #[serde(default)]
    pub linux_compat: bool,
    #[serde(default)]
    pub native_uefi: bool,
}
#[derive(Serialize)]
pub struct ImportResult {
    pub profiles: Vec<Profile>,
    pub warnings: Vec<String>,
}
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
pub(crate) fn extract_external(path: &Path, root: &Path, ratio: u64) -> Result<(), String> {
    crate::archive::extract(path, root, ratio)
}
fn files(root: &Path) -> Result<BTreeMap<String, PathBuf>, String> {
    fn walk(
        root: &Path,
        dir: &Path,
        map: &mut BTreeMap<String, PathBuf>,
        depth: usize,
    ) -> Result<(), String> {
        if depth > 32 || map.len() > 100_000 {
            return Err("Media tree too large".into());
        }
        for entry in fs::read_dir(dir).map_err(err)? {
            let e = entry.map_err(err)?;
            let p = e.path();
            let kind = e.file_type().map_err(err)?;
            if kind.is_symlink() {
                return Err("Extracted media contains a link".into());
            }
            if kind.is_dir() {
                walk(root, &p, map, depth + 1)?;
            } else if kind.is_file() {
                map.insert(
                    p.strip_prefix(root)
                        .map_err(err)?
                        .to_string_lossy()
                        .replace('\\', "/")
                        .to_ascii_lowercase(),
                    p,
                );
            }
        }
        Ok(())
    }
    let mut map = BTreeMap::new();
    walk(root, root, &mut map, 0)?;
    Ok(map)
}
fn prefix(path: &Path, size: usize) -> Vec<u8> {
    let mut b = Vec::new();
    if let Ok(file) = File::open(path) {
        let _ = file.take(size as u64).read_to_end(&mut b);
    }
    b
}
pub(crate) fn binary_arch(path: &Path) -> Option<&'static str> {
    let mut b = prefix(path, 65536);
    if b.starts_with(&[0x1f, 0x8b]) {
        let mut decoded = Vec::new();
        flate2::read::GzDecoder::new(File::open(path).ok()?)
            .take(65536)
            .read_to_end(&mut decoded)
            .ok()?;
        b = decoded;
    }
    if b.starts_with(b"MZ") {
        let machine = crate::efi::pe_machine(&b)?;
        return crate::efi::from_machine(machine)
            .map(|entry| entry.architecture)
            .or_else(|| (machine == 0x1c4).then_some("arm32"));
    }
    if b.starts_with(b"\x7fELF") && b.len() > 20 {
        let m = if b[5] == 2 {
            u16::from_be_bytes([b[18], b[19]])
        } else {
            u16::from_le_bytes([b[18], b[19]])
        };
        return match m {
            3 => Some("x86"),
            4 => Some("m68k"),
            8 => Some(match (b[4], b[5]) {
                (2, 1) => "mips64el",
                (2, _) => "mips64",
                (_, 1) => "mipsel",
                _ => "mips",
            }),
            15 => Some(if b[4] == 2 { "hppa64" } else { "hppa" }),
            50 => Some("ia64"),
            75 => Some("vax"),
            62 => Some("x64"),
            183 => Some("arm64"),
            40 => Some("arm32"),
            21 => Some(if b[5] == 1 { "ppc64le" } else { "ppc64" }),
            20 => Some("powerpc"),
            243 => Some(if b[4] == 2 { "riscv64" } else { "riscv32" }),
            258 => Some("loongarch64"),
            22 => Some("s390x"),
            2 => Some("sparc"),
            43 => Some("sparc64"),
            0x9026 => Some("alpha"),
            _ => None,
        };
    }
    if b.get(514..518) == Some(b"HdrS") {
        return Some(if b.get(0x236).is_some_and(|v| v & 1 != 0) {
            "x64"
        } else {
            "x86"
        });
    }
    if b.get(36..40) == Some(&[0x18, 0x28, 0x6f, 0x01]) {
        return Some("arm32");
    }
    if b.get(56..60) == Some(b"ARM\x64") {
        return Some("arm64");
    }
    None
}
fn wim_arch(path: &Path) -> Option<String> {
    let mut f = File::open(path).ok()?;
    let mut header = [0; 96];
    f.read_exact(&mut header).ok()?;
    if &header[..8] != b"MSWIM\0\0\0" {
        return None;
    }
    let size = u64::from_le_bytes(header[72..80].try_into().ok()?) & 0x00ff_ffff_ffff_ffff;
    let offset = u64::from_le_bytes(header[80..88].try_into().ok()?);
    if size > 8 * 1024 * 1024 || offset.checked_add(size)? > f.metadata().ok()?.len() {
        return None;
    }
    f.seek(SeekFrom::Start(offset)).ok()?;
    let mut xml = vec![0; size as usize];
    f.read_exact(&mut xml).ok()?;
    let text = String::from_utf16_lossy(
        &xml.chunks_exact(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect::<Vec<_>>(),
    );
    let arch = text.split("<ARCH>").nth(1)?.split('<').next()?;
    Some(
        match arch {
            "0" => "x86",
            "9" => "x64",
            "12" => "arm64",
            "6" => "ia64",
            _ => "unknown",
        }
        .into(),
    )
}
fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/")
}
pub(crate) fn asset(name: &str) -> Option<PathBuf> {
    let mut dirs = vec![];
    if let Some(p) = std::env::var_os("NETBOOT_ASSETS") {
        dirs.push(PathBuf::from(p));
    }
    if cfg!(target_os = "linux") {
        dirs.push(PathBuf::from("/usr/share/netboot/assets"));
    }
    if let Ok(p) = std::env::current_exe()
        && let Some(parent) = p.parent()
    {
        dirs.push(parent.join("assets"));
    }
    dirs.into_iter().map(|d| d.join(name)).find(|p| p.is_file())
}
pub(crate) fn network_loaders(architecture: &str) -> &'static [(&'static str, &'static str)] {
    match architecture {
        "x64" => &[("undionly.kpxe", "x86-bios"), ("ipxe-x64.efi", "x64-efi")],
        "x86" => &[("undionly.kpxe", "x86-bios"), ("ipxe-ia32.efi", "ia32-efi")],
        "arm32" => &[("ipxe-arm32.efi", "arm32-efi")],
        "arm64" => &[("ipxe-arm64.efi", "arm64-efi")],
        "loongarch64" => &[("ipxe-loongarch64.efi", "loongarch64-efi")],
        "riscv64" => &[("ipxe-riscv64.efi", "riscv64-efi")],
        _ => &[],
    }
}
fn palo_lif(
    source: &Path,
    ipl: &Path,
    kernel: &Path,
    initrd: &Path,
    output: &Path,
    wide: bool,
    arguments: &str,
) -> Result<(), String> {
    let mut image = prefix(source, 2048);
    if image.len() != 2048 || &image[..8] != b"\x80\0PALO\0\x05" {
        return Err("Unsupported PALO ISO header".into());
    }
    let put = |b: &mut [u8], p: usize, n: u32| b[p..p + 4].copy_from_slice(&n.to_be_bytes());
    let append = |image: &mut Vec<u8>, path: &Path| -> Result<(u32, u32), String> {
        let size = fs::metadata(path).map_err(err)?.len();
        if size > 256 * 1024 * 1024 {
            return Err("PALO component exceeds size limit".into());
        }
        let offset = image.len() as u32;
        File::open(path)
            .map_err(err)?
            .read_to_end(image)
            .map_err(err)?;
        image.resize(image.len() + 2048 - image.len() % 2048, 0);
        Ok((offset, size as u32))
    };
    let boot = prefix(ipl, 4);
    if boot != [0xe8, 0, 0, 0x32] || !fs::metadata(ipl).map_err(err)?.len().is_multiple_of(2048) {
        return Err("Invalid PALO IPL image".into());
    }
    let (offset, size) = append(&mut image, ipl)?;
    if u32::from_be_bytes(image[248..252].try_into().unwrap()) >= size {
        return Err("PALO IPL entry outside bootloader".into());
    }
    put(&mut image, 240, offset);
    put(&mut image, 244, size);
    let (offset, size) = append(&mut image, kernel)?;
    put(&mut image, 8, if wide { 0 } else { offset });
    put(&mut image, 12, if wide { 0 } else { size });
    put(&mut image, 232, if wide { offset } else { 0 });
    put(&mut image, 236, if wide { size } else { 0 });
    let (offset, size) = append(&mut image, initrd)?;
    put(&mut image, 16, offset);
    put(&mut image, 20, size);
    image[446..512].fill(0);
    image[1024..2048].fill(0);
    if arguments.contains(['\n', '\r']) {
        return Err("PALO kernel arguments must fit on one line".into());
    }
    let command = format!(
        "0/vmlinux{} initrd=0/ramdisk console=ttyS0{}{}",
        if wide { "64" } else { "32" },
        if arguments.is_empty() { "" } else { " " },
        arguments
    );
    if command.len() >= 1024 {
        return Err("PALO kernel command line exceeds the LIF command area".into());
    }
    image[1024..1024 + command.len()].copy_from_slice(command.as_bytes());
    fs::write(output, image).map_err(err)
}
fn debian_release_fields(root: &Path) -> Option<(String, String)> {
    let dists = root.join("dists");
    for entry in fs::read_dir(dists).ok()?.flatten() {
        let release = entry.path().join("Release");
        let text = fs::read_to_string(release).ok()?;
        let field = |name: &str| {
            text.lines()
                .find_map(|line| line.strip_prefix(name))
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        };
        if let (Some(suite), Some(codename)) = (field("Suite:"), field("Codename:")) {
            return Some((suite, codename));
        }
    }
    None
}
fn prepare_debian_release_aliases(root: &Path) -> Result<(), String> {
    let dists = root.join("dists");
    if !dists.is_dir() {
        return Ok(());
    }
    let entries = fs::read_dir(&dists)
        .map_err(err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(err)?;
    for entry in entries {
        if !entry.file_type().map_err(err)?.is_dir() {
            continue;
        }
        let release = entry.path().join("Release");
        if !release.is_file() {
            continue;
        }
        let text = fs::read_to_string(&release).map_err(err)?;
        let field = |name: &str| {
            text.lines()
                .find_map(|line| line.strip_prefix(name))
                .map(str::trim)
                .map(str::to_string)
        };
        let suite = field("Suite:");
        let codename = field("Codename:");
        for name in [suite.as_deref(), codename.as_deref()]
            .into_iter()
            .flatten()
        {
            if name.is_empty()
                || !name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
            {
                continue;
            }
            let directory = dists.join(name);
            let target = directory.join("Release");
            if !target.exists() {
                fs::create_dir_all(&directory).map_err(err)?;
                fs::copy(&release, &target).map_err(err)?;
            }
        }
    }
    Ok(())
}
pub fn import_with_options(
    source: &Path,
    library: &Path,
    trust_local_iso: bool,
    linux_compat: bool,
) -> Result<ImportResult, String> {
    let source = source.canonicalize().map_err(err)?;
    if !source.is_file() {
        return Err("Choose an ISO file".into());
    }
    fs::create_dir_all(library).map_err(err)?;
    let id = format!(
        "iso-{:x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(err)?
            .as_nanos()
    );
    let root = library.join(&id);
    fs::create_dir(&root).map_err(err)?;
    let result: Result<ImportResult, String> = (|| {
        let raw_header = prefix(&source, 128);
        if raw_header.get(0x60..0x68) == Some(b"aboot-00") {
            fs::copy(&source, root.join("boot.img")).map_err(err)?;
            let note = "aboot netboot image; SRM BOOTP/TFTP loads it directly".to_string();
            return Ok(ImportResult {
                profiles: vec![Profile { secure_boot: None,
                    system_id: String::new(),
                    system_name: String::new(),
                    id: format!("{id}-0"),
                    name: format!(
                        "{}  alpha",
                        source.file_stem().unwrap_or_default().to_string_lossy()
                    ),
                    architecture: "alpha".into(),
                    kind: "raw".into(),
                    root: root.clone(),
                    boot_files: BTreeMap::from([("default".into(), "boot.img".into())]),
                    kernel: "boot.img".into(),
                    initrd: String::new(),
                    dtb: String::new(),
                    arguments: "console=ttyS0,9600".into(),
                    media: source
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into(),
                    status: "prepared".into(),
                    notes: note.clone(),
                    boot_evidence: None,
                }],
                warnings: Vec::new(),
            });
        }
        if let Some((name, bytes)) = crate::ods::alpha_bootstrap(&source)? {
            fs::write(root.join(&name), bytes).map_err(err)?;
            fs::copy(&source, root.join("media.iso")).map_err(err)?;
            let software = name.trim_end_matches(".SYS").to_string();
            let note =
                "A complete OpenVMS installation requires an InfoServer LAD/LAST media service"
                    .to_string();
            return Ok(ImportResult {
                profiles: vec![Profile { secure_boot: None,
                    system_id: String::new(),
                    system_name: String::new(),
                    id: format!("{id}-0"),
                    name: format!(
                        "{}  alpha",
                        source.file_stem().unwrap_or_default().to_string_lossy()
                    ),
                    architecture: "alpha".into(),
                    kind: "raw".into(),
                    root: root.clone(),
                    boot_files: BTreeMap::from([
                        ("mop".into(), name.clone()),
                        ("lad".into(), "media.iso".into()),
                        (software, name.clone()),
                    ]),
                    kernel: name,
                    initrd: String::new(),
                    dtb: String::new(),
                    arguments: String::new(),
                    media: source
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into(),
                    status: "needs-attention".into(),
                    notes: note.clone(),
                    boot_evidence: None,
                }],
                warnings: vec![note],
            });
        }
        for prepare in [
            crate::netbsd::prepare,
            crate::freebsd::prepare,
            crate::openbsd::prepare,
            crate::solaris::prepare,
            crate::windows::prepare,
        ] {
            if let Some(result) = prepare(&source, &root, &id)? {
                for profile in &result.profiles {
                    if !profile.root.join(".bootah-media.json").is_file() {
                        crate::media_source::register(&source, &profile.root)?;
                    }
                }
                return Ok(result);
            }
        }
        if let Some(result) =
            crate::linux_media::prepare(&source, &root, &id, trust_local_iso, linux_compat)?
        {
            for profile in &result.profiles {
                if !profile.root.join(".bootah-media.json").is_file() {
                    crate::media_source::register(&source, &profile.root)?;
                }
            }
            return Ok(result);
        }
        if linux_compat {
            return crate::linux_fallback::prepare(&source, &root, &id, true)?.ok_or_else(|| {
                "No supported Linux compatibility adapter found for this ISO".into()
            });
        }
        if let Some(result) = crate::linux_fallback::prepare(&source, &root, &id, false)? {
            return Ok(result);
        }
        let indexed = crate::media_source::Image::open(&source)?;
        let linux_like = indexed.entries().keys().any(|name| {
            let base = name.rsplit('/').next().unwrap_or(name).to_ascii_lowercase();
            base == "linux" || base == "vmlinux" || base.starts_with("vmlinuz")
        });
        if linux_like {
            return Err("Linux kernel found, but no unambiguous static default boot entry could be traced from El Torito and its configuration".into());
        }
        crate::media_source::register(&source, &root)?;
        for (name, entry) in indexed.entries() {
            let base = name.rsplit('/').next().unwrap_or(name).to_ascii_lowercase();
            if !entry.is_dir
                && (base == "bsd.rd"
                    || base == "pxeboot"
                    || base.starts_with("netbsd-install")
                    || base.ends_with(".efi")
                    || base == "pxeboot_ia32.bin"
                    || name == "boot/boot.img")
            {
                let target = crate::archive::destination(&root, name)?;
                fs::create_dir_all(target.parent().unwrap()).map_err(err)?;
                indexed.materialize(name, &target)?;
            }
        }
        let mut map = files(&root)?;
        prepare_debian_release_aliases(&root)?;
        if !map.keys().any(|name| name.starts_with("debian-installer/")) {
            let companion = source
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join("netboot.tar.gz");
            if companion.is_file() {
                extract_external(&companion, &root, 4)?;
                map = files(&root)?;
            }
        }
        if let Some(image) = map.get("boot.img") {
            let header = prefix(image, 128);
            if header.get(0x60..0x68) == Some(b"aboot-00") {
                let note = "aboot netboot image; SRM BOOTP/TFTP loads it directly".to_string();
                return Ok(ImportResult {
                    profiles: vec![Profile { secure_boot: None,
                        system_id: String::new(),
                        system_name: String::new(),
                        id: format!("{id}-0"),
                        name: format!(
                            "{}  alpha",
                            source.file_stem().unwrap_or_default().to_string_lossy()
                        ),
                        architecture: "alpha".into(),
                        kind: "raw".into(),
                        root: root.clone(),
                        boot_files: BTreeMap::from([("default".into(), "boot.img".into())]),
                        kernel: "boot.img".into(),
                        initrd: String::new(),
                        dtb: String::new(),
                        arguments: "console=ttyS0,9600".into(),
                        media: source
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into(),
                        status: "prepared".into(),
                        notes: note.clone(),
                        boot_evidence: None,
                    }],
                    warnings: Vec::new(),
                });
            }
        }
        if let Some(image) = map.get("boot/boot.img") {
            let header = prefix(image, 512);
            if header.len() == 512 && (&header[54..57] == b"FAT" || &header[82..85] == b"FAT") {
                let firmware = root.join("netboot-efi-media");
                fs::create_dir(&firmware).map_err(err)?;
                extract_external(image, &firmware, 2)?;
                map = files(&root)?;
            }
        }
        let mut warnings = vec![];
        let mut profiles = vec![];
        let windows = map.get("sources/boot.wim");
        let candidates: Vec<_> = if let Some(wim) = windows {
            vec![("windows", wim.clone(), None, "".to_string())]
        } else {
            let netboot = map
                .keys()
                .any(|name| name.starts_with("debian-installer/") && name.ends_with("/linux"));
            let (debian_suite, debian_codename) =
                debian_release_fields(&root).unwrap_or_else(|| ("stable".into(), "stable".into()));
            map.iter().filter_map(|(name,path)|{
                let parent=path.parent()?;
                let base=path.file_name()?.to_string_lossy().to_ascii_lowercase();
                let initrd_in = |name: &str| parent.join(name).is_file().then(|| parent.join(name));
                let (kind,initrd,args) =
                    if name.starts_with("casper/") && base.starts_with("vmlinuz") {
                        ("linux",["initrd","initrd.lz"].iter().find_map(|n|map.get(&format!("casper/{n}"))).cloned(),"ip=dhcp boot=casper url={base}/media.iso".into())
                    }
                    else if name.starts_with("images/pxeboot/") && base=="vmlinuz" {
                        ("linux",map.get("images/pxeboot/initrd.img").cloned(),"ip=dhcp inst.repo={base}/ inst.stage2={base}/".into())
                    }
                    else if name.starts_with("boot/") && base.starts_with("vmlinuz-") {
                        let flavor=base.trim_start_matches("vmlinuz-");
                        ("linux",map.get(&format!("boot/initramfs-{flavor}")).cloned(),format!("ip=dhcp modules=loop,squashfs alpine_repo={{base}}/apks modloop={{base}}/boot/modloop-{flavor}"))
                    }
                    else if name.contains("/loader/") && base=="linux" && initrd_in("initrd").is_some() {
                        ("linux",initrd_in("initrd"),"install={base}/".into())
                    }
                    else if name.contains("arch/boot/") && base.starts_with("vmlinuz") && initrd_in("initramfs-linux.img").is_some() {
                        ("linux",initrd_in("initramfs-linux.img"),"archiso_http_srv={base}/".into())
                    }
                    else if name.starts_with("install") && netboot { return None }
                    else if name=="linux" || name.ends_with("/linux") || base=="vmlinux" || (name.starts_with("install")&&base.starts_with("vmlinuz")&&!name.contains("/gtk/")) {
                        ("linux",["initrd.gz","initrd"].iter().map(|n|parent.join(n)).find(|p|p.is_file()),if netboot {format!("auto=true priority=critical cdrom-detect/load_media=false mirror/suite={debian_suite} mirror/codename={debian_codename} mirror/http/hostname={{server}}:{{http_port}} mirror/http/directory=/boot/{{profile}}/ mirror/http/proxy= debian-installer/allow_unauthenticated=true")}else{String::new()})
                    }
                    else if base=="bsd.rd" || base.starts_with("netbsd-install") {("bsd",None,String::new())}
                    else {return None;};
                if kind=="linux"&&initrd.is_none(){return None;}
                Some((kind,path.clone(),initrd,args))
            }).collect()
        };
        if candidates.is_empty() {
            if map.contains_key("solaris_10/tools/setup_install_server") {
                return Err("Solaris 10 SPARC detected. Its separate UFS boot slice, bootparams and NFS installation service are not yet prepared".into());
            }
            return Err("No supported installer found. Supported layouts: Ubuntu casper, Fedora/Anaconda, Alpine, Debian mini/netboot, OpenBSD/NetBSD ramdisk, Windows PE".into());
        }
        for (index, (mut kind, kernel, initrd, args)) in candidates.into_iter().enumerate() {
            let arch = if kind == "windows" {
                wim_arch(&kernel)
            } else {
                binary_arch(&kernel).map(str::to_string)
            }
            .unwrap_or_else(|| "unknown".into());
            let pid = format!("{id}-{index}");
            let mut boot_files = BTreeMap::new();
            let mut note = String::new();
            let mut informational_note = false;
            let mut boot_kernel = relative(&root, &kernel);
            let mut boot_initrd = initrd
                .as_ref()
                .map(|p| relative(&root, p))
                .unwrap_or_default();
            if matches!(arch.as_str(), "hppa" | "hppa64")
                && let (Some(ipl), Some(initrd)) = (map.get("install/iplboot"), initrd.as_ref())
            {
                let name = format!("netboot-{arch}.lif");
                palo_lif(
                    &source,
                    ipl,
                    &kernel,
                    initrd,
                    &root.join(&name),
                    arch == "hppa64",
                    &args,
                )?;
                boot_kernel = name.clone();
                boot_initrd.clear();
                kind = "raw";
                boot_files.insert("default".into(), name);
                note = "PALO LIF boot file prepared".into();
                informational_note = true;
            }
            if kind == "linux"
                && let Some(flavor) = kernel
                    .file_name()
                    .and_then(|n| n.to_str())
                    .and_then(|n| n.strip_prefix("vmlinuz-"))
                && let (Some(initrd), Some(modloop)) =
                    (initrd.as_ref(), map.get(&format!("boot/modloop-{flavor}")))
            {
                let temporary = root.join(format!(".modloop-{index}"));
                let prepared = root.join(format!("boot/netboot-initramfs-{flavor}"));
                let prepare = (|| {
                    fs::create_dir(&temporary).map_err(err)?;
                    extract_external(modloop, &temporary, 32)?;
                    crate::initramfs::alpine(initrd, &temporary, &prepared)
                })();
                let _ = fs::remove_dir_all(&temporary);
                match prepare {
                    Ok(()) => boot_initrd = relative(&root, &prepared),
                    Err(e) => {
                        let _ = fs::remove_file(prepared);
                        note = format!("Network initrd preparation required: {e}");
                    }
                }
            }
            if kind == "windows" && arch == "ia64" {
                boot_initrd = boot_kernel;
                boot_kernel = String::new();
                for name in [
                    "bootmgr.efi",
                    "efi/boot/bootia64.efi",
                    "efi/microsoft/boot/bootmgfw.efi",
                ] {
                    if let Some(path) = map.get(name)
                        && binary_arch(path) == Some("ia64")
                    {
                        boot_kernel = relative(&root, path);
                        boot_files.insert("ia64".into(), boot_kernel.clone());
                        break;
                    }
                }
                note = "IA64 Windows requires native EFI network boot and a matching RAM disk BCD"
                    .into();
            } else if kind == "windows" {
                boot_initrd = boot_kernel;
                let loader = if arch == "arm64" {
                    "wimboot-arm64"
                } else {
                    "wimboot"
                };
                boot_kernel = loader.into();
                if let Some(p) = asset(loader) {
                    fs::copy(p, root.join(loader)).map_err(err)?;
                } else {
                    note = format!("Missing matching Windows PE loader: {loader}");
                }
                for (from, to) in [("boot/bcd", "BCD"), ("boot/boot.sdi", "boot.sdi")] {
                    if let Some(p) = map.get(from) {
                        fs::copy(p, root.join(to)).map_err(err)?;
                    }
                }
            }
            if kind == "bsd" {
                let netbsd = kernel.file_name().is_some_and(|n| {
                    n.to_string_lossy()
                        .to_ascii_lowercase()
                        .starts_with("netbsd")
                });
                if netbsd {
                    for (name, path) in &map {
                        if matches!(arch.as_str(), "x86" | "x64")
                            && name.ends_with("pxeboot_ia32.bin")
                        {
                            boot_files.insert("x86-bios".into(), relative(&root, path));
                        }
                        if arch == "x64"
                            && name.ends_with("bootx64.efi")
                            && binary_arch(path) == Some("x64")
                        {
                            boot_files.insert("x64-efi".into(), relative(&root, path));
                        }
                    }
                    fs::write(
                        root.join("boot.cfg"),
                        format!(
                            "menu=Install:boot {}\ntimeout=0\n",
                            relative(&root, &kernel)
                        ),
                    )
                    .map_err(err)?;
                }
                if matches!(arch.as_str(), "x86" | "x64")
                    && let Some(p) = map.get("pxeboot")
                {
                    boot_files.insert("x86-bios".into(), relative(&root, p));
                }
                if let Some(loader) = crate::efi::for_arch(&arch) {
                    for &path in loader.paths {
                        if let Some(file) = map.get(path)
                            && binary_arch(file) == Some(loader.architecture)
                        {
                            boot_files.insert(loader.firmware_key.into(), relative(&root, file));
                            break;
                        }
                    }
                }
                fs::create_dir_all(root.join("etc")).map_err(err)?;
                fs::write(
                    root.join("etc/boot.conf"),
                    format!("boot {}\n", relative(&root, &kernel)),
                )
                .map_err(err)?;
            } else if arch == "ia64" && kind == "linux" {
                if let Some(path) = map.values().find(|path| {
                    path.file_name()
                        .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case("elilo.efi"))
                        && binary_arch(path) == Some("ia64")
                }) {
                    boot_files.insert("ia64".into(), relative(&root, path));
                    note = "ELILO network boot configuration prepared".into();
                    informational_note = true;
                }
            } else if matches!(arch.as_str(), "sparc" | "sparc64") {
                boot_files.insert("default".into(), boot_kernel.clone());
                note = "SPARC OpenBoot loads the ELF kernel over TFTP".into();
                informational_note = true;
            } else {
                for &(name, key) in network_loaders(&arch) {
                    if let Some(p) = asset(name) {
                        fs::copy(p, root.join(name)).map_err(err)?;
                        boot_files.insert(key.into(), name.into());
                    }
                }
            }
            if boot_files.is_empty() && note.is_empty() {
                note = "Firmware loader required".into();
                warnings.push(format!(
                    "{arch}: firmware loader required; U-Boot can use generated PXE configuration"
                ));
            }
            if arch == "unknown" {
                note = "Architecture requires confirmation".into();
                warnings
                    .push("Architecture could not be determined from executable headers".into());
            }
            if kind == "linux" && map.keys().any(|k| k.starts_with("install.")) {
                warnings.push("Debian CD initrd may require installation media; use mini/netboot ISO for network installation".into());
                note = "CD installer may require a network initrd".into();
            }
            if !note.is_empty() && !informational_note {
                warnings.push(note.clone());
            }
            profiles.push(Profile { secure_boot: None,
                system_id: String::new(),
                system_name: String::new(),
                id: pid,
                name: format!(
                    "{}  {}",
                    source.file_stem().unwrap_or_default().to_string_lossy(),
                    arch
                ),
                architecture: arch,
                kind: kind.into(),
                root: root.clone(),
                boot_files,
                kernel: boot_kernel,
                initrd: boot_initrd,
                dtb: String::new(),
                arguments: args,
                media: source
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into(),
                status: if note.is_empty() {
                    "prepared".into()
                } else {
                    "needs-attention".into()
                },
                notes: note,
                boot_evidence: None,
            });
        }
        if root.join("media.iso").exists() {
            return Err("ISO uses reserved media.iso name".into());
        }
        Ok(ImportResult { profiles, warnings })
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&root);
    }
    result.map(|mut imported| {
        let (system_id, system_name) = crate::system_identity::detect(&source, &imported.profiles);
        for profile in &mut imported.profiles {
            profile.system_id = system_id.clone();
            profile.system_name = system_name.clone();
        }
        if imported
            .profiles
            .iter()
            .any(|profile| crate::media_source::fallback_notice(&profile.root).is_some())
        {
            imported
                .warnings
                .push(crate::media_source::UDF_FALLBACK_NOTICE.into());
        }
        imported
    })
}
