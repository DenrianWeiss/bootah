use crate::{media::ImportResult, media_source::Image, model::Profile};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Seek, SeekFrom},
    path::Path,
};

const WINPESHL_INI: &[u8] =
    b"[LaunchApps]\r\n%SYSTEMROOT%\\system32\\cmd.exe, /k%SYSTEMROOT%\\system32\\bootah-setup.cmd\r\n";

#[derive(Serialize, Deserialize)]
struct RisLayout {
    boot_dir: String,
    install_dir: String,
    architecture: String,
}

fn ris_layout(image: &Image) -> Option<RisLayout> {
    let install_dir = ["amd64", "i386"]
        .into_iter()
        .find(|dir| image.find(&format!("{dir}/txtsetup.sif")).is_some())?;
    let boot_dir = ["i386", "amd64"].into_iter().find(|dir| {
        ["startrom.n1_", "setupldr.ex_", "ntdetect.com"]
            .iter()
            .all(|file| image.find(&format!("{dir}/{file}")).is_some())
    })?;
    Some(RisLayout {
        boot_dir: boot_dir.into(),
        install_dir: install_dir.into(),
        architecture: if install_dir == "amd64" { "x64" } else { "x86" }.into(),
    })
}

pub(crate) fn ris_install_dir(profile: &Profile) -> String {
    fs::read(profile.root.join(".bootah-ris-layout.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<RisLayout>(&bytes).ok())
        .map(|layout| layout.install_dir)
        .filter(|dir| matches!(dir.as_str(), "i386" | "amd64"))
        .unwrap_or_else(|| {
            if profile.architecture == "x64" {
                "amd64"
            } else {
                "i386"
            }
            .into()
        })
}

pub(crate) fn ris_fallback_name(profile: &Profile, name: &str) -> Option<String> {
    let layout = fs::read(profile.root.join(".bootah-ris-layout.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<RisLayout>(&bytes).ok())?;
    if !matches!(layout.boot_dir.as_str(), "i386" | "amd64")
        || !matches!(layout.install_dir.as_str(), "i386" | "amd64")
        || layout.boot_dir == layout.install_dir
    {
        return None;
    }
    let (directory, rest) = name.split_once('/')?;
    directory
        .eq_ignore_ascii_case(&layout.boot_dir)
        .then(|| format!("{}/{rest}", layout.install_dir))
}

fn bcd_wim_path(image: &Image) -> Option<String> {
    let bcd = image
        .find("boot/bcd")
        .or_else(|| image.find("efi/microsoft/boot/bcd"))?;
    let bytes = image.read(bcd, 8 * 1024 * 1024).ok()?;
    if bytes.len() < 4096 || &bytes[..4] != b"regf" {
        return None;
    }
    let mut found = std::collections::BTreeSet::new();
    let mut bin = 4096usize;
    while bin < bytes.len() {
        if bytes.get(bin..bin + 4)? != b"hbin" {
            return None;
        }
        let size = u32::from_le_bytes(bytes.get(bin + 8..bin + 12)?.try_into().ok()?) as usize;
        if size < 4096 || size % 4096 != 0 || bin.checked_add(size)? > bytes.len() {
            return None;
        }
        let end = bin + size;
        let mut cell = bin + 32;
        while cell < end {
            let length = i32::from_le_bytes(bytes.get(cell..cell + 4)?.try_into().ok()?);
            let span = length.unsigned_abs() as usize;
            if span < 8 || span % 8 != 0 || cell.checked_add(span)? > end {
                return None;
            }
            if length < 0 {
                let data = &bytes[cell + 4..cell + span];
                for start in (0..data.len().saturating_sub(8)).step_by(2) {
                    if data[start] != b'\\' || data[start + 1] != 0 {
                        continue;
                    }
                    let units: Vec<_> = data[start..]
                        .chunks_exact(2)
                        .take(260)
                        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                        .take_while(|ch| *ch >= 32 && *ch != u16::MAX)
                        .collect();
                    let Ok(path) = String::from_utf16(&units) else {
                        continue;
                    };
                    if !path.to_ascii_lowercase().ends_with(".wim") {
                        continue;
                    }
                    let name = path.trim_start_matches('\\').replace('\\', "/");
                    if name
                        .split('/')
                        .all(|part| !part.is_empty() && part != "." && part != "..")
                        && let Some(actual) = image.find(&name)
                    {
                        found.insert(actual.to_string());
                    }
                }
            }
            cell += span;
        }
        bin = end;
    }
    (found.len() == 1)
        .then(|| found.into_iter().next())
        .flatten()
}

fn has_el_torito_boot_record(source: &Path) -> bool {
    let Ok(mut file) = fs::File::open(source) else {
        return false;
    };
    let mut record = [0u8; 2048];
    for sector in 16..32 {
        if file.seek(SeekFrom::Start(sector * 2048)).is_err()
            || file.read_exact(&mut record).is_err()
        {
            return false;
        }
        if record[0] == 0
            && &record[1..6] == b"CD001"
            && record[7..39].starts_with(b"EL TORITO SPECIFICATION")
        {
            return true;
        }
        if record[0] == 255 && &record[1..6] == b"CD001" {
            break;
        }
    }
    false
}

pub fn prepare(source: &Path, root: &Path, id: &str) -> Result<Option<ImportResult>, String> {
    let image = Image::open(source)?;
    let discovered = bcd_wim_path(&image);
    let Some(wim) = image
        .find("sources/boot.wim")
        .or_else(|| discovered.as_deref())
    else {
        if legacy_setup(&image) {
            return prepare_ris(source, root, id, &image).map(Some);
        }
        return Ok(None);
    };
    if crate::efi::find_in_image(&image, "ia64").is_some() {
        return Err(
            "IA64 Windows requires native EFI RAM-disk boot; wimboot is not supported on IA64"
                .into(),
        );
    }
    let mut warnings = Vec::new();
    let is_setup = setup_source(&image).is_some();
    let efi_architectures = ["x86", "x64", "arm64"]
        .into_iter()
        .filter(|arch| crate::efi::find_in_image(&image, arch).is_some())
        .collect::<Vec<_>>();
    let iso_bootable = image.find("bootmgr").is_some() || !efi_architectures.is_empty();
    let sanboot = !is_setup && iso_bootable && has_el_torito_boot_record(source);
    if !is_setup && !sanboot {
        warnings.push("This image contains WinPE but lacks a complete Windows Setup source".into());
    }
    fs::create_dir_all(root).map_err(|e| e.to_string())?;
    crate::media_source::register(source, root)?;
    for (from, to) in [("boot/bcd", "BCD"), ("boot/boot.sdi", "boot.sdi")] {
        if sanboot {
            break;
        }
        if let Some(path) = image.find(from).or_else(|| {
            (from == "boot/bcd")
                .then(|| image.find("efi/microsoft/boot/bcd"))
                .flatten()
        }) {
            image.materialize(path, &root.join(to))?;
        } else {
            warnings.push(format!("Missing Windows boot component: {from}"));
        }
    }
    let architecture = if let Some(architecture) = wim_architecture(&image, wim) {
        architecture
    } else if efi_architectures.len() == 1 {
        efi_architectures[0]
    } else {
        "unknown"
    };
    if architecture == "ia64" {
        return Err(
            "IA64 Windows requires native EFI RAM-disk boot; wimboot is unsupported".into(),
        );
    }
    if architecture == "unknown" {
        return Err(
            "Cannot determine Windows image architecture; import matching WIM/EFI metadata".into(),
        );
    }
    let mut boot_files = BTreeMap::new();
    let efi = match architecture {
        "arm64" => ("arm64-efi", "ipxe-arm64.efi"),
        "x86" => ("ia32-efi", "ipxe-ia32.efi"),
        "x64" => ("x64-efi", "ipxe-x64.efi"),
        _ => return Err(format!("No Windows EFI bootstrap for {architecture}")),
    };
    for (key, name) in [efi, ("x86-bios", "undionly.kpxe")] {
        if key == "x86-bios" && architecture == "arm64" {
            continue;
        }
        if let Some(asset) = crate::media::asset(name) {
            fs::copy(asset, root.join(name)).map_err(|e| e.to_string())?;
            boot_files.insert(key.into(), name.into());
        } else {
            warnings.push(format!("Missing PXE bootstrap asset: {name}"));
        }
    }
    if architecture == "x64" {
        if let Some(asset) = crate::media::asset("snponly-x64.efi") {
            fs::copy(asset, root.join("snponly-x64.efi")).map_err(|e| e.to_string())?;
        }
    }
    let loader = if architecture == "arm64" {
        "wimboot-arm64"
    } else {
        "wimboot"
    };
    if !sanboot {
        if let Some(asset) = crate::media::asset(loader) {
            fs::copy(asset, root.join(loader)).map_err(|e| e.to_string())?;
        } else {
            warnings.push(format!("Missing Windows PE loader: {loader}"));
        }
    }
    if is_setup {
        fs::write(root.join("bootah-setup.cmd"), setup_script(&image)?)
            .map_err(|e| e.to_string())?;
        fs::write(root.join("winpeshl.ini"), WINPESHL_INI).map_err(|e| e.to_string())?;
    }
    Ok(Some(ImportResult {
        profiles: vec![Profile {
            system_id: String::new(),
            system_name: String::new(),
            id: id.into(),
            name: source
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into(),
            architecture: architecture.into(),
            kind: if sanboot { "windows-pe-iso" } else { "windows" }.into(),
            root: root.into(),
            boot_files,
            kernel: if sanboot { "media.iso" } else { loader }.into(),
            initrd: wim.into(),
            dtb: String::new(),
            arguments: String::new(),
            media: source.to_string_lossy().into(),
            status: if sanboot {
                "prepared"
            } else {
                "needs-attention"
            }
            .into(),
            notes: warnings.join("; "),
            boot_evidence: None,
        }],
        warnings,
    }))
}

fn prepare_ris(
    source: &Path,
    root: &Path,
    id: &str,
    image: &Image,
) -> Result<ImportResult, String> {
    let layout = ris_layout(image).ok_or("RIS BIOS boot files or TXTSETUP.SIF are missing")?;
    let startrom = image
        .find(&format!("{}/startrom.n1_", layout.boot_dir))
        .ok_or("RIS boot file STARTROM.N1_ is missing")?;
    let loader = image
        .find(&format!("{}/setupldr.ex_", layout.boot_dir))
        .ok_or("RIS boot file SETUPLDR.EX_ is missing")?;
    let detect = image
        .find(&format!("{}/ntdetect.com", layout.boot_dir))
        .ok_or("RIS boot file NTDETECT.COM is missing")?;
    fs::create_dir_all(root).map_err(|e| e.to_string())?;
    crate::media_source::register(source, root)?;
    fs::write(
        root.join(".bootah-ris-layout.json"),
        serde_json::to_vec(&layout).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    extract_cab(image, startrom, &root.join("startrom.n12"))?;
    extract_cab(image, loader, &root.join("ntldr"))?;
    image.materialize(detect, &root.join("ntdetect.com"))?;
    if let Some(answer) = image.find(&format!("{}/winnt.sif", layout.install_dir)) {
        image.materialize(answer, &root.join("winnt-original.sif"))?;
    }
    let driver_count = crate::binl::prepare(image, root, &layout.install_dir)?;
    let warning = format!(
        "RIS PXE/TFTP and internal read-only SMB1 RemInst source prepared; {driver_count} network adapters indexed for BINL"
    );
    Ok(ImportResult {
        profiles: vec![Profile {
            system_id: String::new(),
            system_name: String::new(),
            id: id.into(),
            name: source
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into(),
            architecture: layout.architecture,
            kind: "windows-ris".into(),
            root: root.into(),
            boot_files: BTreeMap::from([("x86-bios".into(), "startrom.n12".into())]),
            kernel: "startrom.n12".into(),
            initrd: String::new(),
            dtb: String::new(),
            arguments: String::new(),
            media: source.to_string_lossy().into(),
            status: "needs-attention".into(),
            notes: warning.clone(),
            boot_evidence: None,
        }],
        warnings: Vec::new(),
    })
}

fn extract_cab(image: &Image, entry: &str, dest: &Path) -> Result<(), String> {
    let bytes = read_cab(
        image,
        entry,
        dest.parent().ok_or("Missing RIS target directory")?,
    )?;
    fs::write(dest, bytes).map_err(|e| e.to_string())
}

pub(crate) fn read_cab(image: &Image, entry: &str, scratch: &Path) -> Result<Vec<u8>, String> {
    let archive = scratch.join(format!(
        ".bootah-{}-{}.cab.tmp",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    image.materialize(entry, &archive)?;
    let result = crate::archive::cab_file(&archive, None, 8 * 1024 * 1024);
    let _ = fs::remove_file(&archive);
    result
}

pub(crate) fn ris_answer(profile: &Profile, server: &str, original: &[u8]) -> Vec<u8> {
    let custom = !original.is_empty();
    let mut result = Vec::new();
    let mut section = String::new();
    let mut found = [false; 4];
    let mut options = false;
    let mut repartition = false;
    let mut computer_name = false;
    let mut unattended_setting = false;
    let directory = ris_install_dir(profile);
    let source = format!(r"\\{server}\RemInst\{}\{directory}", profile.id);
    let device = format!(r"\Device\LanmanRedirector\{server}\RemInst\{}", profile.id);
    for line in original.split(|b| *b == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        let text = String::from_utf8_lossy(line);
        let value = text.trim();
        if value.starts_with('[') && value.ends_with(']') {
            ris_answer_section(
                &mut result,
                &section,
                &source,
                &device,
                options,
                repartition,
                computer_name,
                custom,
                unattended_setting,
            );
            section = value[1..value.len() - 1].to_ascii_lowercase();
            options = false;
            repartition = false;
            computer_name = false;
            unattended_setting = false;
            if section == "data" {
                found[0] = true;
            }
            if section == "setupdata" {
                found[1] = true;
            }
            if section == "remoteinstall" {
                found[2] = true;
            }
            if section == "userdata" {
                found[3] = true;
            }
        }
        let key = value
            .split_once('=')
            .map(|(key, _)| key.trim().to_ascii_lowercase());
        if section == "data" && key.as_deref() == Some("unattendedinstall") {
            unattended_setting = true;
        }
        if section == "data"
            && key.as_deref().is_some_and(|key| {
                matches!(
                    key,
                    "floppyless" | "msdosinitiated" | "orisrc" | "orityp" | "localsourceoncd"
                )
            })
        {
            continue;
        }
        if section == "setupdata" && key.as_deref() == Some("setupsourcedevice") {
            continue;
        }
        if section == "setupdata" && key.as_deref() == Some("osloadoptions") {
            options = true;
        }
        if section == "remoteinstall" && key.as_deref() == Some("repartition") {
            repartition = true;
        }
        if section == "userdata" && key.as_deref() == Some("computername") {
            computer_name = true;
        }
        result.extend_from_slice(line);
        result.extend_from_slice(b"\r\n");
    }
    ris_answer_section(
        &mut result,
        &section,
        &source,
        &device,
        options,
        repartition,
        computer_name,
        custom,
        unattended_setting,
    );
    for (found, section) in
        found
            .into_iter()
            .zip(["Data", "SetupData", "RemoteInstall", "UserData"])
    {
        if !found {
            result.extend_from_slice(format!("[{section}]\r\n").as_bytes());
            ris_answer_section(
                &mut result,
                &section.to_ascii_lowercase(),
                &source,
                &device,
                false,
                false,
                false,
                custom,
                false,
            );
        }
    }
    result
}

fn ris_answer_section(
    out: &mut Vec<u8>,
    section: &str,
    source: &str,
    device: &str,
    options: bool,
    repartition: bool,
    computer_name: bool,
    custom: bool,
    unattended_setting: bool,
) {
    match section {
        "data" => {
            out.extend_from_slice(format!("floppyless = 1\r\nMsDosInitiated = 1\r\nOriSrc = \"{source}\"\r\nOriTyp = 4\r\nLocalSourceOnCD = 1\r\n").as_bytes());
            if !unattended_setting {
                out.extend_from_slice(if custom {
                    b"UnattendedInstall = Yes\r\n"
                } else {
                    b"UnattendedInstall = No\r\n"
                });
            }
        }
        "setupdata" => {
            if !options {
                out.extend_from_slice(b"OsLoadOptions = \"/noguiboot /fastdetect\"\r\n");
            }
            out.extend_from_slice(format!("SetupSourceDevice = \"{device}\"\r\n").as_bytes());
        }
        "remoteinstall" if !repartition => out.extend_from_slice(b"Repartition = No\r\n"),
        "userdata" if !computer_name => out.extend_from_slice(b"ComputerName = *\r\n"),
        _ => {}
    }
}

fn legacy_setup(image: &Image) -> bool {
    ris_layout(image).is_some()
}

fn setup_source(image: &Image) -> Option<(&str, &str)> {
    let launcher = image
        .find("setup.exe")
        .or_else(|| image.find("sources/setup.exe"))?;
    let install = [
        "sources/install.wim",
        "sources/install.esd",
        "sources/install.swm",
    ]
    .iter()
    .find_map(|p| image.find(p))?;
    Some((launcher, install))
}

pub(crate) fn smb_setup_script(
    profile: &Profile,
    server: std::net::Ipv4Addr,
    username: &str,
    password: &str,
) -> Result<String, String> {
    for value in [&profile.id, username, password] {
        if value.is_empty()
            || !value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        {
            return Err("Unsafe SMB setup credential or profile identifier".into());
        }
    }
    let entries = crate::media_source::list(&profile.root)?;
    let has = |name: &str| {
        entries
            .iter()
            .any(|(p, e)| !e.is_dir && p.eq_ignore_ascii_case(name))
    };
    let install = ["install.wim", "install.esd", "install.swm"]
        .into_iter()
        .find(|p| has(&format!("sources/{p}")))
        .ok_or("Windows installation image is missing")?;
    let launch = if has("setup.exe") {
        "\"%BOOTAH_SOURCE%\\setup.exe\" %BOOTAH_UNATTEND%".to_string()
    } else if has("sources/setup.exe") {
        format!(
            "\"%BOOTAH_SOURCE%\\sources\\setup.exe\" /InstallFrom:\"%BOOTAH_SOURCE%\\sources\\{install}\" %BOOTAH_UNATTEND%"
        )
    } else {
        return Err("Windows Setup launcher is missing".into());
    };
    let script = format!(
        r#"@echo off
setlocal DisableDelayedExpansion
set "BOOTAH_SOURCE=\\{server}\Bootah\{profile_id}"
set "BOOTAH_UNATTEND="
set "BOOTAH_LOG=%SystemRoot%\Temp\bootah-setup.log"
>"%BOOTAH_LOG%" echo Bootah setup started.
echo Bootah: Initializing Windows PE networking...
:initialize
wpeinit
set "BOOTAH_LAST_ERROR=%errorlevel%"
>>"%BOOTAH_LOG%" echo WPEINIT exit code %BOOTAH_LAST_ERROR%
if not "%BOOTAH_LAST_ERROR%"=="0" goto failed
echo Bootah: Connecting to the installation source...
:connect_source
net use "\\{server}\Bootah" "{password}" /user:"BOOTAH\{username}" /persistent:no >nul
set "BOOTAH_LAST_ERROR=%errorlevel%"
>>"%BOOTAH_LOG%" echo SMB connection exit code %BOOTAH_LAST_ERROR%
if not "%BOOTAH_LAST_ERROR%"=="0" goto failed
if not exist "%BOOTAH_SOURCE%\sources\{install}" set "BOOTAH_LAST_ERROR=2"
if not exist "%BOOTAH_SOURCE%\sources\{install}" goto failed
>>"%BOOTAH_LOG%" echo SMB source connected.
echo Bootah: Installation source connected.
echo Bootah: Starting Windows Setup...
:ready
if exist "%~dp0unattend.xml" set "BOOTAH_UNATTEND=/Unattend:%~dp0unattend.xml"
{launch}
set "BOOTAH_LAST_ERROR=%errorlevel%"
>>"%BOOTAH_LOG%" echo Windows Setup exit code %BOOTAH_LAST_ERROR%
if "%BOOTAH_LAST_ERROR%"=="0" exit /b 0
goto failed
:failed
>>"%BOOTAH_LOG%" echo Setup could not continue; error code %BOOTAH_LAST_ERROR%
echo Bootah: Installation could not continue (error %BOOTAH_LAST_ERROR%).
echo Check the network adapter, IP address and SMB service. Retry with X:\Windows\System32\bootah-setup.cmd.
echo Diagnostic log: %BOOTAH_LOG%
pause
exit /b 1

"#,
        profile_id = profile.id
    );
    Ok(script.replace('\n', "\r\n"))
}

fn setup_script(image: &Image) -> Result<String, String> {
    if setup_source(image).is_none() {
        return Ok("@echo off\r\nwpeinit\r\necho Bootah: Windows PE is ready. This ISO does not include a complete Windows Setup source.\r\nexit /b 0\r\n".into());
    }
    Ok("@echo off\r\necho Bootah: Windows Setup requires the SMB2 installation source service.\r\nexit /b 1\r\n".into())
}

fn wim_architecture(image: &Image, name: &str) -> Option<&'static str> {
    let header = image.read_range(name, 0, 208).ok()?;
    if header.get(..8)? != b"MSWIM\0\0\0" {
        return None;
    }
    let size = u64::from_le_bytes(header.get(72..80)?.try_into().ok()?) & 0x00ff_ffff_ffff_ffff;
    let offset = u64::from_le_bytes(header.get(80..88)?.try_into().ok()?);
    let original = u64::from_le_bytes(header.get(88..96)?.try_into().ok()?);
    if size != original || size > 1024 * 1024 || size % 2 != 0 {
        return None;
    }
    let xml = image.read_range(name, offset, size as usize).ok()?;
    let decoded = String::from_utf16(
        &xml.chunks_exact(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect::<Vec<_>>(),
    )
    .ok()?;
    let mut architectures = std::collections::BTreeSet::new();
    for part in decoded.split("<ARCH>").skip(1) {
        architectures.insert(part.split("</ARCH>").next()?);
    }
    if architectures.len() != 1 {
        return None;
    }
    match *architectures.first()? {
        "0" => Some("x86"),
        "9" => Some("x64"),
        "12" => Some("arm64"),
        "6" => Some("ia64"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn smb_setup_rejects_command_injection_before_reading_media() {
        let profile: Profile = serde_json::from_value(serde_json::json!({
            "id":"windows", "name":"Windows", "architecture":"arm64",
            "kind":"windows", "root":"nonexistent-smb-test-root"
        }))
        .unwrap();
        for value in ["password&whoami", "%PATH%", "abc\"def", "abc\r\ndef", ""] {
            let error = smb_setup_script(&profile, std::net::Ipv4Addr::LOCALHOST, "bootah", value)
                .unwrap_err();
            assert_eq!(error, "Unsafe SMB setup credential or profile identifier");
        }
    }
    #[test]
    fn smb_setup_shows_progress_and_records_errors() {
        let root = std::env::temp_dir().join(format!("bootah-smb-script-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("sources")).unwrap();
        let source = root.join("fixture.iso");
        std::fs::write(&source, b"fixture").unwrap();
        let metadata = std::fs::metadata(&source).unwrap();
        let modified = metadata
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::fs::write(
            root.join(".bootah-media.json"),
            serde_json::to_vec(&serde_json::json!({
                "version":3, "filesystem":"iso9660", "source":source, "length":metadata.len(),
                "modified":modified, "external":false, "entries":{
                    "sources/install.wim":{"size":0,"is_dir":false,"extents":[]},
                    "sources/setup.exe":{"size":0,"is_dir":false,"extents":[]}
                }
            }))
            .unwrap(),
        )
        .unwrap();
        std::fs::write(root.join("sources/install.wim"), b"").unwrap();
        std::fs::write(root.join("sources/setup.exe"), b"").unwrap();
        let profile: Profile = serde_json::from_value(serde_json::json!({
            "id":"windows-test", "name":"Windows test", "architecture":"x64",
            "kind":"windows", "root":root
        }))
        .unwrap();
        let script = smb_setup_script(
            &profile,
            std::net::Ipv4Addr::new(10, 0, 2, 2),
            "bootah_test",
            "0123456789abcdef",
        )
        .unwrap();
        assert!(script.contains("WPEINIT exit code %BOOTAH_LAST_ERROR%"));
        assert!(script.contains("SMB connection exit code %BOOTAH_LAST_ERROR%"));
        assert!(script.contains("Windows Setup exit code %BOOTAH_LAST_ERROR%"));
        assert!(script.contains("Diagnostic log: %BOOTAH_LOG%"));
        assert!(script.contains("Connecting to the installation source..."));
        std::fs::remove_dir_all(root).unwrap();
    }
    fn source(paths: &[&str]) -> Image {
        let entries = paths
            .iter()
            .map(|name| {
                (
                    (*name).to_string(),
                    serde_json::json!({"size":0,"is_dir":false,"extents":[]}),
                )
            })
            .collect::<serde_json::Map<_, _>>();
        serde_json::from_value(serde_json::json!({"source":"fixture.iso","length":0,"modified":0,"entries":entries,"external":false})).unwrap()
    }
    #[test]
    fn identifies_legacy_setup_without_a_wim() {
        assert!(legacy_setup(&source(&[
            "i386/txtsetup.sif",
            "i386/startrom.n1_",
            "i386/setupldr.ex_",
            "i386/ntdetect.com"
        ])));
        assert!(legacy_setup(&source(&[
            "amd64/txtsetup.sif",
            "i386/startrom.n1_",
            "i386/setupldr.ex_",
            "i386/ntdetect.com"
        ])));
        assert!(!legacy_setup(&source(&["i386/txtsetup.sif"])));
    }
    #[test]
    fn ris_transport_answer_keeps_optional_user_settings() {
        let profile: Profile = serde_json::from_value(serde_json::json!({"id":"xp","name":"XP","architecture":"x86","kind":"windows-ris","root":"."})).unwrap();
        let bare = String::from_utf8(ris_answer(&profile, "192.0.2.1", &[])).unwrap();
        assert!(bare.contains(
            "SetupSourceDevice = \"\\Device\\LanmanRedirector\\192.0.2.1\\RemInst\\xp\""
        ));
        assert!(bare.contains("OriSrc = \"\\\\192.0.2.1\\RemInst\\xp\\i386\""));
        assert!(bare.contains("LocalSourceOnCD = 1"));
        let mut x64 = profile.clone();
        x64.architecture = "x64".into();
        let x64_answer = String::from_utf8(ris_answer(&x64, "192.0.2.1", &[])).unwrap();
        assert!(x64_answer.contains("OriSrc = \"\\\\192.0.2.1\\RemInst\\xp\\amd64\""));
        assert!(bare.contains("ComputerName = *"));
        assert!(!bare.contains("ProductKey"));
        let custom = String::from_utf8(ris_answer(&profile, "192.0.2.1", b"[UserData]\r\nComputerName = OFFICE-PC\r\nProductKey = SAMPLE\r\n[SetupData]\r\nOsLoadOptions = \"/custom /fastdetect\"\r\n")).unwrap();
        assert!(custom.contains("ComputerName = OFFICE-PC"));
        assert!(custom.contains("ProductKey = SAMPLE"));
        assert!(custom.contains("/custom /fastdetect"));
        assert!(!custom.contains("ComputerName = *"));
    }
    #[test]
    fn pe_only_does_not_launch_missing_setup() {
        let script = setup_script(&source(&["sources/boot.wim"])).unwrap();
        assert!(script.contains("Windows PE is ready"));
        assert!(!script.contains("bootah-fetch.exe"));
        assert!(!script.contains("BOOTAH_STAGE"));
        let reduced = setup_script(&source(&["sources/setup.exe", "sources/install.esd"])).unwrap();
        assert!(reduced.contains("requires the SMB2"));
        assert!(!reduced.contains("BOOTAH_STAGE"));
    }
}
