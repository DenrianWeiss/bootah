use crate::{
    media_source::Image,
    model::{Profile, Settings},
};
use sha2::{Digest, Sha256};
use std::{fs, path::Path, process::Command};

pub fn enabled(profile: &Profile) -> bool {
    crate::efi::for_arch(&profile.architecture).is_some_and(|loader| {
        profile
            .boot_files
            .get(loader.firmware_key)
            .is_some_and(|path| {
                path == &format!(
                    "bootah-native/{}",
                    loader.paths[0].rsplit('/').next().unwrap()
                )
            })
    })
}

// This detects a certificate table, not trust. Firmware and shim enforce trust.
pub(crate) fn signed_for(bytes: &[u8], machine: u16) -> bool {
    let read = |offset: usize| {
        bytes
            .get(offset..offset + 4)
            .and_then(|v| v.try_into().ok())
            .map(u32::from_le_bytes)
    };
    if crate::efi::pe_machine(bytes) != Some(machine) {
        return false;
    }
    let Some(pe) = read(60).map(|n| n as usize) else {
        return false;
    };
    let optional = pe + 24;
    if bytes.get(optional..optional + 2) != Some(&[0x0b, 0x02]) {
        return false;
    }
    if read(optional + 108).is_none_or(|n| n < 5) {
        return false;
    }
    let Some(start) = read(optional + 112 + 32).map(|n| n as usize) else {
        return false;
    };
    let Some(size) = read(optional + 112 + 36).map(|n| n as usize) else {
        return false;
    };
    size >= 8
        && start > 0
        && start
            .checked_add(size)
            .is_some_and(|end| end <= bytes.len())
        && read(start).is_some_and(|length| length >= 8 && length as usize <= size)
        && bytes.get(start + 4..start + 8) == Some(&[0, 2, 2, 0])
}
fn wim(command: &mut Command) -> Result<(), String> {
    let output = command.output().map_err(|e| {
        format!(
            "Native Windows boot needs wimlib-imagex: {e}"
        )
    })?;
    if !output.status.success() {
        return Err(format!(
            "wimlib: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}
fn wim_command() -> Command {
    Command::new(std::env::var_os("BOOTAH_WIMLIB_IMAGEX").unwrap_or_else(|| "wimlib-imagex".into()))
}

fn packaged_grub(
    image: &Image,
    root: &Path,
    architecture: &str,
    machine: u16,
) -> Result<(), String> {
    let packages: Vec<_> = image
        .entries()
        .iter()
        .filter(|(name, entry)| {
            !entry.is_dir
                && (name.starts_with(&format!(
                    "pool/main/g/grub2-signed/grub-efi-{architecture}-signed_"
                )) || name.starts_with(&format!(
                    "pool/main/g/grub-efi-{architecture}-signed/grub-efi-{architecture}-signed_"
                )))
                && name.ends_with(&format!("_{architecture}.deb"))
        })
        .collect();
    if packages.len() != 1 || packages[0].1.size > 32 * 1024 * 1024 {
        return Err("ISO must contain one matching signed network GRUB package".into());
    }
    let work = root.join(format!("grub-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&work).map_err(|e| e.to_string())?;
    let result = (|| {
        let deb = work.join("grub.deb");
        image.materialize(packages[0].0, &deb)?;
        let unpack = work.join("unpack");
        crate::archive::extract(&deb, &unpack, 64)?;
        let data = ["data.tar.xz", "data.tar.zst", "data.tar.gz", "data.tar"]
            .iter()
            .map(|name| unpack.join(name))
            .find(|path| path.is_file())
            .ok_or("Signed GRUB package lacks data archive")?;
        let contents = work.join("contents");
        crate::archive::extract(&data, &contents, 64)?;
        let source = crate::model::safe_file(
            &contents,
            &format!(
                "usr/lib/grub/{}-efi-signed/grubnet{}.efi.signed",
                if architecture == "arm64" {
                    "arm64"
                } else {
                    "x86_64"
                },
                if architecture == "arm64" {
                    "aa64"
                } else {
                    "x64"
                }
            ),
        )?;
        let bytes = fs::read(source).map_err(|e| e.to_string())?;
        if !signed_for(&bytes, machine) {
            return Err(
                "ISO network GRUB lacks matching signature"
                    .into(),
            );
        }
        fs::write(
            root.join(if architecture == "arm64" {
                "grubaa64.efi"
            } else {
                "grubx64.efi"
            }),
            bytes,
        )
        .map_err(|e| e.to_string())?;
        fs::write(root.join("grub-source.txt"), packages[0].0).map_err(|e| e.to_string())
    })();
    let _ = fs::remove_dir_all(work);
    result
}

pub fn prepare(profile: &mut Profile, settings: &Settings) -> Result<(), String> {
    if !matches!(profile.architecture.as_str(), "x64" | "arm64") {
        return Err("Secure Boot support requires x64 or ARM64".into());
    }
    let loader = crate::efi::for_arch(&profile.architecture).unwrap();
    let machine = loader.machine;
    let suffix = if profile.architecture == "arm64" {
        "aa64"
    } else {
        "x64"
    };
    let entry = format!("boot{suffix}.efi");
    let image = Image::open(Path::new(&profile.media))?;
    let root = profile.root.join("bootah-native");
    fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    match profile.kind.as_str() {
        "windows" | "windows-pe-iso" => {
            let loader = image.find(loader.paths[0]).ok_or("Missing matching Microsoft EFI loader")?;
            let bytes = image.read(loader, 32 * 1024 * 1024)?;
            if !signed_for(&bytes, machine) { return Err("Windows EFI has no matching signature table".into()); }
            for path in ["efi/microsoft/boot/bcd"] {
                if image.find(path).is_none() { return Err(format!("Native Windows boot requires: {path}")); }
            }
            fs::write(root.join(&entry), bytes).map_err(|e| e.to_string())?;
            let boot = root.clone();
            fs::create_dir_all(&boot).map_err(|e| e.to_string())?;
            image.materialize(image.find("efi/microsoft/boot/bcd").unwrap(), &boot.join("bcd"))?;
            // Preserve a maintenance PE's shell. Setup media needs a network-aware shell.
            if profile.kind == "windows" {
                prepare_windows_wim(profile, settings)?;
            }
        }
        "linux" => {
            let info = image.find(".disk/info").and_then(|name| image.read(name, 65536).ok()).unwrap_or_default();
            let info = String::from_utf8_lossy(&info).to_ascii_lowercase();
            let ubuntu = info.contains("ubuntu") && info.contains("server");
            for target in [entry.clone(), format!("mm{suffix}.efi"), "MokManager.efi".into()] {
                if target != entry && image.find(&format!("efi/boot/{target}")).is_none() { continue; }
                let source = image.find(&format!("efi/boot/{target}")).ok_or_else(|| format!("Missing signed ISO component: {target}"))?;
                let bytes = image.read(source, 32 * 1024 * 1024)?;
                if target == entry && !signed_for(&bytes, machine) { return Err(format!("Missing matching EFI certificate: {target}")); }
                fs::write(root.join(&target), bytes).map_err(|e| e.to_string())?;
            }
            let network = if ubuntu {packaged_grub(&image, &root, if suffix=="aa64" {"arm64"} else {"amd64"}, machine).is_ok()} else {false};
            if !network {
                let name=format!("grub{suffix}.efi");
                if let Some(source)=image.find(&format!("efi/boot/{name}")).or_else(||image.find("efi/boot/grub.efi")) {
                    fs::write(root.join(&name),image.read(source,32*1024*1024)?).map_err(|e|e.to_string())?;
                    fs::write(root.join("grub-source.txt"),source).map_err(|e|e.to_string())?;
                }
                if let Some(source)=image.find("efi/boot/grub.efi") {
                    fs::write(root.join("grub.efi"),image.read(source,32*1024*1024)?).map_err(|e|e.to_string())?;
                }
            }
        }
        _ => return Err("Secure Boot support is unavailable for this media".into()),
    }
    let mut components = Vec::new();
    for item in fs::read_dir(&root).map_err(|e| e.to_string())? {
        let path = item.map_err(|e| e.to_string())?.path();
        if path
            .extension()
            .is_some_and(|s| s.eq_ignore_ascii_case("efi"))
        {
            let bytes = fs::read(&path).map_err(|e| e.to_string())?;
            components.push(serde_json::json!({"file":path.file_name().unwrap().to_string_lossy(), "sha256":format!("{:x}", Sha256::digest(&bytes))}));
        }
    }
    components.sort_by_key(|v| v["file"].as_str().unwrap_or_default().to_string());
    let manifest = serde_json::json!({"architecture":profile.architecture, "iso":profile.media, "entry":entry,
        "grub_package_or_path":fs::read_to_string(root.join("grub-source.txt")).ok(),
        "trust":"firmware-and-loader-verification-required", "components":components});
    fs::write(
        root.join("components.json"),
        serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    profile.boot_files.clear();
    profile
        .boot_files
        .insert(loader.firmware_key.into(), format!("bootah-native/{entry}"));
    if matches!(profile.kind.as_str(), "windows" | "windows-pe-iso") {
        profile.kernel = format!("bootah-native/{entry}");
    }
    profile.status = "prepared".into();
    if let Some(evidence) = profile.boot_evidence.as_mut() {
        evidence.verification = "native-uefi-unverified".into();
        evidence.diagnostics.push(crate::model::BootDiagnostic {
            code: "native-uefi-unverified".into(),
            detail: "Original ISO adapter retained; firmware trust and complete boot still require validation.".into(),
        });
    }
    profile.notes.push_str(" Secure Boot support enabled.");
    Ok(())
}

pub fn prepare_windows_wim(profile: &Profile, settings: &Settings) -> Result<(), String> {
    let image = Image::open(Path::new(&profile.media))?;
    if ![
        "sources/install.wim",
        "sources/install.esd",
        "sources/install.swm",
    ]
    .iter()
    .any(|p| image.find(p).is_some())
    {
        return Ok(());
    }
    if !profile
        .id
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    {
        return Err("Invalid native profile ID".into());
    }
    let url = format!(
        "http://{}:{}/boot/{}/bootah-setup.cmd",
        settings.server_ip, settings.http_port, profile.id
    );
    let root = profile.root.join("bootah-native");
    let helper_name = format!("bootah-native-{}.exe", profile.architecture);
    let helper = crate::media::asset(&helper_name).ok_or_else(|| {
        format!("Missing native WinPE helper: {helper_name}")
    })?;
    let helper_bytes = fs::read(&helper).map_err(|e| e.to_string())?;
    let expected_machine = crate::efi::for_arch(&profile.architecture)
        .ok_or("Unsupported Windows architecture")?
        .machine;
    if crate::efi::pe_machine(&helper_bytes) != Some(expected_machine) {
        return Err("WinPE helper architecture mismatch".into());
    }
    let stamp = root.join("startup-cache.json");
    let fingerprint = serde_json::to_string(&serde_json::json!({"version":2,"url":url,"helper_sha256":format!("{:x}", Sha256::digest(&helper_bytes)), "media":profile.media,"initrd":profile.initrd})).map_err(|e| e.to_string())?;
    let destination = root.join("boot.wim");
    if fs::read_to_string(&stamp).ok().as_deref() == Some(&fingerprint) && destination.is_file() {
        return Ok(());
    }
    let work = root.join(format!("work-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let result = (|| {
        let original = work.join("original.wim");
        image.materialize(&profile.initrd, &original)?;
        let header = image.read_range(&profile.initrd, 0, 124)?;
        let index = header
            .get(120..124)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
            .filter(|i| *i > 0)
            .ok_or("WIM has no boot index")?;
        let output = work.join("boot.wim");
        wim(wim_command()
            .arg("export")
            .arg(&original)
            .arg(index.to_string())
            .arg(&output)
            .arg("--boot"))?;
        let ini = work.join("winpeshl.ini");
        fs::write(&ini, b"[LaunchApps]\r\n%SYSTEMROOT%\\system32\\cmd.exe, /k %SYSTEMROOT%\\system32\\bootah-native.cmd\r\n").map_err(|e| e.to_string())?;
        let script = work.join("bootah-native.cmd");
        fs::write(&script, format!("@echo off\r\nwpeinit\r\n\"%SystemRoot%\\System32\\bootah-native.exe\" \"{url}\" \"%TEMP%\\bootah-setup.cmd\"\r\nif errorlevel 1 goto failed\r\ncall \"%TEMP%\\bootah-setup.cmd\"\r\nexit /b\r\n:failed\r\necho Bootah: startup failed. Check network and server settings.\r\n")).map_err(|e| e.to_string())?;
        for (source, target) in [
            (&ini, "winpeshl.ini"),
            (&script, "bootah-native.cmd"),
            (&helper, "bootah-native.exe"),
        ] {
            let path = source
                .canonicalize()
                .map_err(|e| e.to_string())?
                .to_string_lossy()
                .into_owned();
            if path.contains(['"', '\n', '\r']) {
                return Err("WIM source path contains unsupported characters".into());
            }
            wim(wim_command()
                .arg("update")
                .arg(&output)
                .arg("1")
                .arg(format!(
                    "--command=add \"{path}\" /Windows/System32/{target}"
                )))?;
        }
        fs::create_dir_all(destination.parent().unwrap()).map_err(|e| e.to_string())?;
        // All tools work on a private copy; a failed preparation never serves a partial WIM.
        fs::rename(&output, &destination).map_err(|e| e.to_string())?;
        fs::write(stamp, fingerprint).map_err(|e| e.to_string())
    })();
    let _ = fs::remove_dir_all(work);
    result
}

pub fn grub(
    profile: &Profile,
    settings: &Settings,
    auto: Option<&crate::model::AutoConfig>,
) -> Option<Vec<u8>> {
    if !enabled(profile) || profile.kind != "linux" {
        return None;
    }
    let arguments = crate::automation::arguments(profile, settings, auto);
    let quote = |value: &str| {
        format!(
            "'{}'",
            value.replace('\'', "'\\''").replace(['\r', '\n'], " ")
        )
    };
    let args = arguments
        .split_whitespace()
        .map(|word| {
            let value = word.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')).unwrap_or(word);
            quote(value)
        })
        .collect::<Vec<_>>()
        .join(" ");
    let base = format!("(tftp,{})", settings.server_ip);
    Some(format!("set timeout=0\nset default=0\nmenuentry 'Bootah Secure Boot' {{\n linux {} {args}\n initrd {}\n}}\n", quote(&format!("{base}/{}", profile.kernel)), quote(&format!("{base}/{}", profile.initrd))).into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grub_quotes_autoinstall_seed_once() {
        let profile: Profile = serde_json::from_value(serde_json::json!({
            "id":"ubuntu","name":"Ubuntu","root":"/tmp","architecture":"arm64","kind":"linux",
            "kernel":"vmlinuz","initrd":"initrd","boot_files":{"arm64-efi":"bootah-native/bootaa64.efi"}
        })).unwrap();
        let auto = crate::model::AutoConfig { profile_id: "ubuntu".into(), method: "autoinstall".into(), token: "seed".into(), filename: "user-data".into() };
        let config=String::from_utf8(grub(&profile,&Settings::default(),Some(&auto)).unwrap()).unwrap();
        assert!(config.contains("'ds=nocloud-net;s=http://"));
        assert!(!config.contains("\\'ds="));
    }

    #[test]
    fn signature_table_must_be_in_bounds_and_pkcs7() {
        let mut pe = vec![0u8; 512];
        pe[..2].copy_from_slice(b"MZ");
        pe[60..64].copy_from_slice(&64u32.to_le_bytes());
        pe[64..68].copy_from_slice(b"PE\0\0");
        pe[68..70].copy_from_slice(&0x8664u16.to_le_bytes());
        pe[88..90].copy_from_slice(&0x20bu16.to_le_bytes());
        pe[196..200].copy_from_slice(&16u32.to_le_bytes());
        pe[232..236].copy_from_slice(&480u32.to_le_bytes());
        pe[236..240].copy_from_slice(&32u32.to_le_bytes());
        pe[480..484].copy_from_slice(&32u32.to_le_bytes());
        pe[484..488].copy_from_slice(&[0, 2, 2, 0]);
        assert!(signed_for(&pe, 0x8664));
        assert!(!signed_for(&pe[..500], 0x8664));
        pe[68..70].copy_from_slice(&0xaa64u16.to_le_bytes());
        assert!(signed_for(&pe, 0xaa64));
        assert!(!signed_for(&pe, 0x8664));
        pe[68..70].copy_from_slice(&0x8664u16.to_le_bytes());
        pe[486] = 1;
        assert!(!signed_for(&pe, 0x8664));
    }

    #[test]
    fn native_bcd_uses_efi_copy_and_does_not_fall_back_to_bios() {
        let root =
            std::env::temp_dir().join(format!("bootah-native-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("bootah-native")).unwrap();
        let profile: Profile = serde_json::from_value(serde_json::json!({
            "id":"native", "name":"Native", "root":root, "architecture":"x64", "kind":"windows",
            "boot_files":{"x64-efi":"bootah-native/bootx64.efi"}
        }))
        .unwrap();
        let settings = Settings::default();
        fs::write(root.join("bootah-native/bcd"), b"EFI BCD").unwrap();
        assert_eq!(
            crate::boot::virtual_file(&profile, "Boot/BCD", &settings, None),
            Some(b"EFI BCD".to_vec())
        );
        assert_eq!(crate::boot::virtual_file(&profile, "BCD", &settings, None), Some(b"EFI BCD".to_vec()));
        fs::remove_file(root.join("bootah-native/bcd")).unwrap();
        assert_eq!(
            crate::boot::virtual_file(&profile, "Boot/BCD", &settings, None),
            Some(vec![])
        );
        fs::remove_dir_all(root).unwrap();
    }
}

pub fn resource_name<'a>(profile: &Profile, name: &'a str) -> &'a str {
    if enabled(profile) && profile.kind == "windows" && name.eq_ignore_ascii_case(&profile.initrd) {
        "bootah-native/boot.wim"
    } else { name }
}
