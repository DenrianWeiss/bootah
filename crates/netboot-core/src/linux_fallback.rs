use crate::{
    media::ImportResult,
    media_source::Image,
    model::{BootDiagnostic, BootEvidence, Profile},
};
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Seek, SeekFrom},
    path::Path,
};

#[derive(Clone, PartialEq, Eq)]
struct Entry {
    kernel: String,
    initrd: String,
    arguments: String,
    config: String,
    platform: &'static str,
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn catalog(source: &Path) -> Result<Option<Vec<(u8, u32)>>, String> {
    let mut file = fs::File::open(source).map_err(err)?;
    let mut descriptor = [0u8; 2048];
    let mut sector = None;
    for lba in 17..64u64 {
        file.seek(SeekFrom::Start(lba * 2048)).map_err(err)?;
        file.read_exact(&mut descriptor).map_err(err)?;
        if descriptor[0] == 255 {
            break;
        }
        if descriptor[0] == 0
            && &descriptor[1..6] == b"CD001"
            && descriptor[7..39].starts_with(b"EL TORITO SPECIFICATION")
        {
            sector = Some(u32::from_le_bytes(descriptor[71..75].try_into().unwrap()));
            break;
        }
    }
    let Some(sector) = sector else {
        return Ok(None);
    };
    let mut data = [0u8; 2048];
    file.seek(SeekFrom::Start(u64::from(sector) * 2048))
        .map_err(err)?;
    file.read_exact(&mut data).map_err(err)?;
    if data[0] != 1
        || data[30..32] != [0x55, 0xaa]
        || data[..32]
            .chunks_exact(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]) as u32)
            .sum::<u32>()
            & 0xffff
            != 0
    {
        return Err("Invalid El Torito boot catalog".into());
    }
    let mut platform = data[1];
    let mut remaining = 1usize;
    let mut out = Vec::new();
    for record in data[32..].chunks_exact(32).take(63) {
        if record[0] == 0x90 || record[0] == 0x91 {
            platform = record[1];
            remaining = u16::from_le_bytes([record[2], record[3]]) as usize;
            continue;
        }
        if remaining == 0 {
            continue;
        }
        remaining -= 1;
        if record[0] == 0x88 && record[1] == 0 && matches!(platform, 0 | 0xef) {
            let lba = u32::from_le_bytes(record[8..12].try_into().unwrap());
            if lba == 0 {
                return Err("El Torito boot image has no extent".into());
            }
            out.push((platform, lba));
        }
    }
    if out.is_empty() {
        return Err("No supported BIOS/EFI El Torito boot entry".into());
    }
    Ok(Some(out))
}

fn path(image: &Image, config: &str, token: &str) -> Result<String, String> {
    if token.is_empty() || token.contains(['$', '`', '\\', ':', ';']) {
        return Err("Dynamic or unsafe boot path".into());
    }
    let joined = if let Some(absolute) = token.strip_prefix('/') {
        absolute.to_string()
    } else {
        let parent = config.rsplit_once('/').map(|(p, _)| p).unwrap_or("");
        if parent.is_empty() {
            token.into()
        } else {
            format!("{parent}/{token}")
        }
    };
    if joined
        .split('/')
        .any(|p| p.is_empty() || p == "." || p == "..")
    {
        return Err("Unsafe boot path".into());
    }
    image
        .find(&joined)
        .map(str::to_string)
        .ok_or_else(|| format!("Boot file missing: {joined}"))
}

fn family_path(image: &Image, config: &str, token: &str) -> Result<String, String> {
    path(image, config, token).or_else(|_| path(image, "", token))
}

fn grub(image: &Image, config: &str, platform: &'static str) -> Result<Entry, String> {
    let bytes = image.read(config, 256 * 1024)?;
    let source = std::str::from_utf8(&bytes).map_err(err)?;
    let mut default = 0usize;
    let mut entries: Vec<(String, String, String)> = Vec::new();
    let mut current: Option<(String, String, String)> = None;
    for raw in source.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.contains(['$', '`', '\\', ';']) {
            return Err("Dynamic GRUB syntax needs a family adapter".into());
        }
        if let Some(value) = line.strip_prefix("set default=") {
            if current.is_some() {
                return Err("GRUB default changed inside menu entry".into());
            }
            default = value
                .trim_matches(['\'', '"'])
                .parse()
                .map_err(|_| "Dynamic GRUB default entry")?;
        } else if line.starts_with("menuentry ") && line.ends_with('{') {
            if current.is_some() {
                return Err("Nested GRUB menu needs a family adapter".into());
            }
            current = Some((String::new(), String::new(), String::new()));
        } else if line == "}" {
            if let Some(entry) = current.take() {
                entries.push(entry);
            } else {
                return Err("Conditional GRUB configuration needs a family adapter".into());
            }
        } else if let Some(entry) = current.as_mut() {
            let mut words = line.split_whitespace();
            match words.next().unwrap_or("") {
                "linux" | "linuxefi" | "linux16" => {
                    if !entry.0.is_empty() {
                        return Err("Multiple GRUB kernels in one entry".into());
                    }
                    entry.0 = path(image, config, words.next().ok_or("Missing GRUB kernel")?)?;
                    entry.2 = words.collect::<Vec<_>>().join(" ");
                }
                "initrd" | "initrdefi" | "initrd16" => {
                    if !entry.1.is_empty() {
                        return Err("Multiple GRUB initrd commands".into());
                    }
                    entry.1 = path(image, config, words.next().ok_or("Missing GRUB initrd")?)?;
                    if words.next().is_some() {
                        return Err("Multiple GRUB initrd files need a family adapter".into());
                    }
                }
                "insmod" | "echo" | "loadfont" => {}
                "set" if line.starts_with("set timeout=") => {}
                _ => return Err("Dynamic GRUB command needs a family adapter".into()),
            }
        } else if !(line.starts_with("set timeout=")
            || line.starts_with("insmod ")
            || line.starts_with("loadfont "))
        {
            return Err("Dynamic GRUB configuration needs a family adapter".into());
        }
    }
    if current.is_some() {
        return Err("Unterminated GRUB menu entry".into());
    }
    let (kernel, initrd, arguments) = entries
        .into_iter()
        .nth(default)
        .ok_or("GRUB default entry missing")?;
    if kernel.is_empty() || initrd.is_empty() {
        return Err("GRUB default is not a Linux kernel/initrd entry".into());
    }
    if arguments.contains(['\'', '"']) {
        return Err("Quoted GRUB kernel arguments need a family adapter".into());
    }
    Ok(Entry {
        kernel,
        initrd,
        arguments,
        config: config.into(),
        platform,
    })
}

fn syslinux(image: &Image, config: &str, platform: &'static str) -> Result<Entry, String> {
    let bytes = image.read(config, 256 * 1024)?;
    let source = std::str::from_utf8(&bytes).map_err(err)?;
    let mut default = None;
    let mut menu_default = None;
    let mut ontimeout = None;
    let mut labels: Vec<(String, String, String, String)> = Vec::new();
    let mut global_append = String::new();
    for raw in source.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.contains(['$', '`', '\\', ';']) {
            return Err("Dynamic SYSLINUX syntax needs a family adapter".into());
        }
        let (key, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        let rest = rest.trim();
        match key.to_ascii_uppercase().as_str() {
            "DEFAULT" => default = Some(rest.to_string()),
            "ONTIMEOUT" => ontimeout = Some(rest.to_string()),
            "LABEL" => labels.push((
                rest.to_string(),
                String::new(),
                String::new(),
                String::new(),
            )),
            "KERNEL" | "LINUX" => {
                labels.last_mut().ok_or("SYSLINUX kernel outside LABEL")?.1 = rest.into()
            }
            "INITRD" => labels.last_mut().ok_or("SYSLINUX initrd outside LABEL")?.2 = rest.into(),
            "APPEND" => {
                if let Some(label) = labels.last_mut() {
                    label.3 = rest.into()
                } else {
                    global_append = rest.into()
                }
            }
            "MENU" if rest.eq_ignore_ascii_case("DEFAULT") => {
                let index = labels
                    .len()
                    .checked_sub(1)
                    .ok_or("MENU DEFAULT outside LABEL")?;
                if menu_default.replace(index).is_some() {
                    return Err("Multiple SYSLINUX menu defaults".into());
                }
            }
            "MENU" | "UI" | "PROMPT" | "TIMEOUT" | "SERIAL" | "DISPLAY" | "SAY" => {}
            "INCLUDE" | "CONFIG" => {
                return Err("SYSLINUX include/chain needs a family adapter".into());
            }
            _ => return Err("Unsupported SYSLINUX directive needs a family adapter".into()),
        }
    }
    let explicit = ontimeout.as_deref().or(default.as_deref());
    let mut names = std::collections::HashSet::new();
    if labels.iter().any(|label| !names.insert(&label.0)) {
        return Err("Duplicate SYSLINUX labels".into());
    }
    let index = if let Some(label) = explicit.filter(|l| !l.ends_with(".c32")) {
        if label.split_whitespace().count() != 1 {
            return Err("SYSLINUX default command needs a family adapter".into());
        }
        labels
            .iter()
            .position(|v| v.0 == label)
            .ok_or("SYSLINUX DEFAULT label missing")?
    } else if let Some(index) = menu_default {
        index
    } else if labels.len() == 1 {
        0
    } else {
        return Err("SYSLINUX default entry is ambiguous".into());
    };
    if let Some(menu_index) = menu_default
        && menu_index != index
    {
        return Err("SYSLINUX menu and timeout defaults differ".into());
    }
    let (_, kernel, mut initrd, append) = labels.swap_remove(index);
    let mut args = if append.is_empty() {
        global_append
    } else {
        append
    };
    let mut kept = Vec::new();
    for word in args.split_whitespace() {
        if let Some(value) = word.strip_prefix("initrd=") {
            if !initrd.is_empty() {
                return Err("Duplicate SYSLINUX initrd".into());
            }
            initrd = value.into();
        } else {
            kept.push(word);
        }
    }
    args = kept.join(" ");
    if kernel.is_empty() || initrd.is_empty() || initrd.contains(',') || args.contains(['\'', '"'])
    {
        return Err("SYSLINUX default needs one static Linux kernel/initrd".into());
    }
    Ok(Entry {
        kernel: path(image, config, &kernel)?,
        initrd: path(image, config, &initrd)?,
        arguments: args,
        config: config.into(),
        platform,
    })
}

pub(crate) fn original_arguments(
    image: &Image,
    kernel: &str,
    initrd: &str,
) -> Option<(String, String)> {
    let mut found = Vec::new();
    for config in [
        "boot/grub/grub.cfg",
        "boot/grub.cfg",
        "efi/boot/grub.cfg",
        "grub/grub.cfg",
        "isolinux/isolinux.cfg",
        "syslinux.cfg",
    ] {
        let Some(config) = image.find(config) else {
            continue;
        };
        let entry = if config.ends_with("isolinux.cfg") || config.ends_with("syslinux.cfg") {
            syslinux(image, config, "original")
        } else {
            grub(image, config, "original").or_else(|_| grub_family_default(image, config))
        };
        if let Ok(entry) = entry
            && entry.kernel.eq_ignore_ascii_case(kernel)
            && entry.initrd.eq_ignore_ascii_case(initrd)
            && !found.iter().any(|(_, args)| args == &entry.arguments)
        {
            found.push((entry.config, entry.arguments));
        }
    }
    (found.len() == 1).then(|| found.remove(0))
}

pub(crate) fn original_grub_default(image: &Image, config: &str) -> Option<(String, String)> {
    let entry = grub_family_default(image, config).ok()?;
    Some((entry.kernel, entry.initrd))
}

pub(crate) fn default_title(image: &Image, config: &str) -> String {
    let Ok(bytes) = image.read(config, 256 * 1024) else {
        return "default".into();
    };
    let source = String::from_utf8_lossy(&bytes);
    if config.to_ascii_lowercase().contains("grub") {
        let default = source
            .lines()
            .find_map(|line| {
                line.trim()
                    .strip_prefix("set default=")
                    .or_else(|| line.trim().strip_prefix("default="))
                    .and_then(|value| value.trim_matches(['\'', '"']).parse::<usize>().ok())
            })
            .unwrap_or(0);
        let mut depth = 0usize;
        let mut titles = Vec::new();
        for line in source.lines().map(str::trim) {
            if depth == 0 && line.starts_with("menuentry ") {
                titles.push(line);
            }
            if line.ends_with('{') {
                depth += 1;
            }
            if line == "}" {
                depth = depth.saturating_sub(1);
            }
        }
        return titles
            .get(default)
            .map(|line| {
                let value = line.trim_start_matches("menuentry ").trim_start();
                let quote = value.chars().next().unwrap_or(' ');
                if quote == '\'' || quote == '"' {
                    value[1..]
                        .split(quote)
                        .next()
                        .unwrap_or("default")
                        .to_string()
                } else {
                    value.split_whitespace().next().unwrap_or("default").into()
                }
            })
            .unwrap_or_else(|| "default".into());
    }
    let mut selected = None;
    let mut menu_default = None;
    let mut label = None;
    for line in source.lines().map(str::trim) {
        let upper = line.to_ascii_uppercase();
        if upper.starts_with("LABEL ") {
            label = Some(line[6..].trim().to_string());
        }
        if upper.starts_with("DEFAULT ") && !upper.ends_with(".C32") {
            selected = Some(line[8..].trim().to_string());
        }
        if upper.starts_with("ONTIMEOUT ") {
            selected = Some(line[10..].trim().to_string());
        }
        if upper == "MENU DEFAULT" {
            menu_default = label.clone();
        }
    }
    selected
        .or(menu_default)
        .or(label)
        .unwrap_or_else(|| "default".into())
}

fn grub_family_default(image: &Image, config: &str) -> Result<Entry, String> {
    let bytes = image.read(config, 256 * 1024)?;
    let source = std::str::from_utf8(&bytes).map_err(err)?;
    let mut default = 0usize;
    let mut depth = 0usize;
    let mut current: Option<(String, String, String)> = None;
    let mut entries = Vec::new();
    for raw in source.lines() {
        let line = raw.trim();
        if let Some(value) = line
            .strip_prefix("set default=")
            .or_else(|| line.strip_prefix("default="))
        {
            default = value
                .trim_matches(['\'', '"'])
                .parse()
                .map_err(|_| "Dynamic GRUB default")?;
        }
        if depth == 0 && line.starts_with("menuentry ") && line.ends_with('{') {
            current = Some((String::new(), String::new(), String::new()));
        } else if let Some(entry) = current.as_mut() {
            let mut words = line.split_whitespace();
            match words.next().unwrap_or("") {
                "linux" | "linuxefi" | "linux16" if depth == 1 => {
                    entry.0 = family_path(image, config, words.next().ok_or("Missing kernel")?)?;
                    entry.2 = words.collect::<Vec<_>>().join(" ");
                }
                "initrd" | "initrdefi" | "initrd16" if depth == 1 => {
                    entry.1 = family_path(image, config, words.next().ok_or("Missing initrd")?)?;
                }
                _ => {}
            }
        }
        if line.ends_with('{') {
            depth += 1;
        }
        if line == "}" {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                if let Some(entry) = current.take() {
                    entries.push(entry);
                }
            }
        }
    }
    let (kernel, initrd, arguments) = entries
        .into_iter()
        .nth(default)
        .ok_or("GRUB default missing")?;
    if kernel.is_empty() || initrd.is_empty() || arguments.contains(['$', '`', '\\']) {
        return Err("GRUB default is dynamic or incomplete".into());
    }
    Ok(Entry {
        kernel,
        initrd,
        arguments,
        config: config.into(),
        platform: "original",
    })
}

fn config_for(image: &Image, boot_image: &str, loader: &str) -> Result<String, String> {
    let parent = boot_image.rsplit_once('/').map(|(p, _)| p).unwrap_or("");
    let local = |name: &str| {
        if parent.is_empty() {
            name.to_string()
        } else {
            format!("{parent}/{name}")
        }
    };
    let names: Vec<String> = if loader == "syslinux" {
        vec![
            local("isolinux.cfg"),
            local("syslinux.cfg"),
            "isolinux/isolinux.cfg".into(),
        ]
    } else {
        vec![
            "boot/grub/grub.cfg".into(),
            "EFI/BOOT/grub.cfg".into(),
            "grub/grub.cfg".into(),
        ]
    };
    let mut found = names
        .iter()
        .filter_map(|n| image.find(n))
        .collect::<Vec<_>>();
    found.sort_unstable();
    found.dedup();
    if found.len() != 1 {
        return Err(format!(
            "Expected one {loader} config referenced by El Torito image; found {}",
            found.len()
        ));
    }
    Ok(found[0].into())
}

fn source_note(arguments: &str, profile_id: &str) -> String {
    let local = arguments.split_whitespace().any(|word| {
        let lower = word.to_ascii_lowercase();
        lower.contains("cdlabel=")
            || lower.contains("label=")
            || lower.contains("/dev/sr")
            || lower.contains("cdrom:")
            || lower.contains("iso-scan/filename=")
    });
    let remote = arguments
        .split_whitespace()
        .any(|word| word.contains("http://") || word.contains("https://") || word.contains("nfs:"));
    let mut note = format!(
        " Read-only files are available at http://<Bootah-server>:<HTTP-port>/boot/{profile_id}/ and the original ISO at the same URL plus media.iso."
    );
    if local {
        note.push_str(" The original command line refers to local optical media or a volume label, so the installer may stop after the kernel starts without a family-specific source handoff.");
    }
    if remote {
        note.push_str(" Existing network URLs are preserved without replacement.");
    }
    note
}

fn compatible_family(image: &Image, arguments: &str) -> Result<&'static str, String> {
    let words = arguments.split_whitespace().collect::<Vec<_>>();
    if words.iter().any(|word| {
        (word.starts_with("ip=") && *word != "ip=dhcp")
            || word.starts_with("ifname=")
            || word.starts_with("BOOTIF=")
    }) {
        return Err("Original boot entry pins a network interface; automatic NBD fallback requires interface-neutral DHCP".into());
    }
    if words.iter().any(|word| {
        ["url=", "iso-url=", "inst.repo=", "inst.stage2="]
            .iter()
            .any(|prefix| word.starts_with(prefix))
            && (word.contains("http://") || word.contains("https://") || word.contains("nfs:"))
    }) {
        return Err("Original boot entry already has a remote source; automatic NBD fallback will not replace it".into());
    }
    let mut families = Vec::new();
    if words.contains(&"boot=casper") && image.find("casper/filesystem.squashfs").is_some() {
        families.push("casper");
    }
    if words
        .iter()
        .any(|word| word.starts_with("inst.stage2=") || word.starts_with("inst.repo="))
        && image.find(".treeinfo").is_some()
        && image.find("images/install.img").is_some()
    {
        families.push("anaconda");
    }
    if words
        .iter()
        .any(|word| word.starts_with("root=aosc-livekit:"))
    {
        families.push("aosc-livekit");
    }
    match families.as_slice() {
        [family] => Ok(family),
        [] => Err("Original initrd framework has no supported read-only NBD adapter".into()),
        _ => Err("Multiple installer frameworks match; NBD adapter is ambiguous".into()),
    }
}

pub(crate) fn compatibility_arguments(arguments: &str, family: &str) -> String {
    let mut kept = Vec::new();
    let mut tail = Vec::new();
    let mut after_separator = false;
    for word in arguments.split_whitespace() {
        if word == "---" {
            after_separator = true;
        }
        if after_separator {
            tail.push(word);
            continue;
        }
        let replace = match family {
            "casper" => ["url=", "iso-url=", "live-media=", "findiso="]
                .iter()
                .any(|prefix| word.starts_with(prefix)),
            "anaconda" => ["inst.repo=", "inst.stage2="]
                .iter()
                .any(|prefix| word.starts_with(prefix)),
            "aosc-livekit" => word.starts_with("root="),
            _ => false,
        };
        if !replace {
            kept.push(word.to_string());
        }
    }
    if !kept.iter().any(|word| word.starts_with("ip=")) {
        kept.push("ip=dhcp".into());
    }
    match family {
        "casper" => kept.push("live-media=/dev/nbd0".into()),
        "anaconda" => {
            kept.push("rd.neednet=1".into());
            kept.push("inst.repo=file:///run/install/repo".into());
            kept.push("inst.stage2=cdrom:/dev/nbd0".into());
        }
        "aosc-livekit" => {
            kept.push("rd.neednet=1".into());
            kept.push("root=aosc-livekit:/dev/nbd0".into());
        }
        _ => {}
    }
    kept.extend([
        "bootah.media=nbd".into(),
        "bootah.server={server}".into(),
        "bootah.port={nbd_port}".into(),
        "bootah.export={profile}".into(),
        "bootah.iso={base}/media.iso".into(),
    ]);
    kept.extend(tail.into_iter().map(str::to_string));
    kept.join(" ")
}

pub fn prepare(
    source: &Path,
    root: &Path,
    id: &str,
    linux_compat: bool,
) -> Result<Option<ImportResult>, String> {
    let image = Image::open(source)?;
    let has_config = image.entries().keys().any(|p| {
        matches!(
            p.to_ascii_lowercase().as_str(),
            "boot/grub/grub.cfg"
                | "efi/boot/grub.cfg"
                | "isolinux/isolinux.cfg"
                | "isolinux/syslinux.cfg"
                | "syslinux.cfg"
                | "grub/grub.cfg"
        )
    });
    if !has_config {
        return Ok(None);
    }
    let boot =
        catalog(source)?.ok_or("Linux boot config exists but ISO has no El Torito catalog")?;
    let mut found = Vec::new();
    for (platform, lba) in boot {
        let boot_image = image
            .path_at_iso_sector(lba)
            .ok_or("El Torito boot image is not an indexed ISO file")?;
        let bytes = image.read_range(boot_image, 0, 4 * 1024 * 1024)?;
        let lower = boot_image.to_ascii_lowercase();
        let loader = if bytes
            .windows(8)
            .any(|w| w.eq_ignore_ascii_case(b"ISOLINUX"))
            && lower.ends_with("isolinux.bin")
        {
            "syslinux"
        } else if lower.ends_with("/grub/i386-pc/eltorito.img")
            || bytes.windows(4).any(|w| w.eq_ignore_ascii_case(b"GRUB"))
        {
            "grub"
        } else {
            return Err("El Torito loader is neither identifiable GRUB nor ISOLINUX".into());
        };
        let config = config_for(&image, boot_image, loader)?;
        let entry = if loader == "grub" {
            grub(
                &image,
                &config,
                if platform == 0xef { "EFI" } else { "BIOS" },
            )?
        } else {
            syslinux(
                &image,
                &config,
                if platform == 0xef { "EFI" } else { "BIOS" },
            )?
        };
        found.push(entry);
    }
    let first = found.first().ok_or("No usable El Torito boot entry")?;
    if found.iter().any(|entry| {
        (
            entry.kernel.as_str(),
            entry.initrd.as_str(),
            entry.arguments.as_str(),
        ) != (
            first.kernel.as_str(),
            first.initrd.as_str(),
            first.arguments.as_str(),
        )
    }) {
        return Err(
            "BIOS and EFI default Linux entries differ; no automatic fallback selected".into(),
        );
    }
    let kernel = root.join("bootah-fallback-kernel");
    image.materialize(&first.kernel, &kernel)?;
    let architecture =
        crate::media::binary_arch(&kernel).ok_or("Fallback kernel architecture is unknown")?;
    let loaders = crate::media::network_loaders(architecture);
    if loaders.is_empty() {
        return Err(format!("No fallback firmware loader for {architecture}"));
    }
    let mut boot_files = BTreeMap::new();
    for &(name, key) in loaders {
        if let Some(asset) = crate::media::asset(name) {
            fs::copy(asset, root.join(name)).map_err(err)?;
            boot_files.insert(key.into(), name.into());
        }
    }
    if boot_files.is_empty() {
        return Err("No matching network firmware loader asset".into());
    }
    let (initrd, arguments, family) = if linux_compat {
        let family = compatible_family(&image, &first.arguments)?;
        let cache = root.join(".bootah-fallback-compat");
        fs::create_dir_all(&cache).map_err(err)?;
        let original = cache.join("original-initrd");
        image.materialize(&first.initrd, &original)?;
        let output_name = "bootah-fallback-compat-initrd".to_string();
        let output = root.join(&output_name);
        match family {
            "casper" => crate::casper_compat::prepare(&original, &output, &cache, architecture)?,
            "anaconda" => {
                crate::fedora_compat::prepare(&image, &original, &output, &cache, architecture)?
            }
            "aosc-livekit" => {
                crate::aosc_compat::prepare(&original, &output, &cache, architecture)?
            }
            _ => unreachable!(),
        }
        fs::remove_dir_all(&cache).map_err(err)?;
        fs::write(
            root.join(".bootah-linux-compat.json"),
            br#"{"version":1,"transport":"nbd"}"#,
        )
        .map_err(err)?;
        (
            output_name,
            compatibility_arguments(&first.arguments, family),
            Some(family),
        )
    } else {
        (first.initrd.clone(), first.arguments.clone(), None)
    };
    crate::media_source::register(source, root)?;
    let profile_id = format!("{id}-0");
    let mut note = format!(
        "Automatic Linux fallback from {} default entry in {}; original kernel {}, initrd {}.{}",
        first.platform,
        first.config,
        first.kernel,
        first.initrd,
        source_note(&first.arguments, &profile_id)
    );
    if let Some(family) = family {
        note.push_str(&format!(
            " {family} compatibility: a derived initrd connects the original ISO read-only."
        ));
    } else {
        note.push_str(" Original boot parameters are retained without a source handoff.");
    }
    let profile = Profile {
        system_id: String::new(),
        system_name: String::new(),
        id: profile_id,
        name: format!(
            "{}  Linux fallback  {architecture}",
            source.file_stem().unwrap_or_default().to_string_lossy()
        ),
        architecture: architecture.into(),
        kind: "linux".into(),
        root: root.into(),
        boot_files,
        kernel: "bootah-fallback-kernel".into(),
        initrd,
        dtb: String::new(),
        arguments,
        media: source.display().to_string(),
        status: "needs-attention".into(),
        notes: note.clone(),
        boot_evidence: Some(BootEvidence {
            config: first.config.clone(),
            entry: default_title(&image, &first.config),
            kernel: first.kernel.clone(),
            initrd: first.initrd.clone(),
            source: if linux_compat { "nbd" } else { "unspecified" }.into(),
            verification: "static".into(),
            diagnostics: if linux_compat {
                Vec::new()
            } else {
                vec![BootDiagnostic {
                    code: "SOURCE_REQUIRED".into(),
                    detail: first.config.clone(),
                }]
            },
        }),
    };
    Ok(Some(ImportResult {
        profiles: vec![profile],
        warnings: if linux_compat {
            Vec::new()
        } else {
            vec!["The original boot parameters require an installation source. Configure a network source or enable Linux ISO compatibility mode.".into()]
        },
    }))
}
