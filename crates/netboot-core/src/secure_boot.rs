use crate::{
    media_source::Image,
    model::{BootEvidence, Profile, Settings},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs, io::Read, path::Path};

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Support {
    pub eligible: bool,
    pub enabled: bool,
    pub state: String,
    pub detail: String,
    pub fingerprint: String,
    pub source_stamp: String,
    pub checked_at: u64,
    pub components: Vec<Component>,
    pub normal: Option<Route>,
}
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Component {
    pub file: String,
    pub sha256: String,
    pub signer: String,
    pub issuer: String,
    pub certificate_sha256: String,
}
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Route {
    boot_files: BTreeMap<String, String>,
    kernel: String,
    initrd: String,
    status: String,
    notes: String,
    boot_evidence: Option<BootEvidence>,
}
impl Route {
    fn capture(p: &Profile) -> Self {
        Self {
            boot_files: p.boot_files.clone(),
            kernel: p.kernel.clone(),
            initrd: p.initrd.clone(),
            status: p.status.clone(),
            notes: p.notes.clone(),
            boot_evidence: p.boot_evidence.clone(),
        }
    }
    fn restore(&self, p: &mut Profile) {
        p.boot_files = self.boot_files.clone();
        p.kernel = self.kernel.clone();
        p.initrd = self.initrd.clone();
        p.status = self.status.clone();
        p.notes = self.notes.clone();
        p.boot_evidence = self.boot_evidence.clone();
    }
}
pub fn eligible(p: &Profile) -> bool {
    !p.media.is_empty()
        && matches!(p.architecture.as_str(), "x64" | "arm64")
        && (matches!(p.kind.as_str(), "windows" | "windows-pe-iso")
            || p.kind == "linux"
                && matches!(
                    p.system_id.as_str(),
                    "ubuntu"
                        | "debian"
                        | "fedora"
                        | "redhat"
                        | "centos"
                        | "rockylinux"
                        | "almalinux"
                        | "opensuse"
                        | "suse"
                        | "kylin"
                ))
}
pub fn support(p: &Profile) -> Support {
    let mut s = p.secure_boot.clone().unwrap_or_default();
    s.eligible = eligible(p);
    s.enabled = crate::native_uefi::enabled(p);
    if s.state.is_empty() {
        s.state = "unchecked".into();
    }
    if !s.source_stamp.is_empty() && source_stamp(p).ok().as_ref() != Some(&s.source_stamp) {
        s.state = "stale".into();
        s.detail = "Image changed; detect EFI certificate again".into();
    }
    s
}
fn normal(p: &Profile) -> Result<Profile, String> {
    let mut original = p.clone();
    if crate::native_uefi::enabled(p) {
        if let Some(route) = p.secure_boot.as_ref().and_then(|s| s.normal.as_ref()) {
            route.restore(&mut original);
        } else {
            return Err("Reimport this legacy image before switching".into());
        }
    }
    original.secure_boot = None;
    Ok(original)
}
fn hash_file(path: &Path) -> Result<String, String> {
    let mut f = fs::File::open(path).map_err(|e| e.to_string())?;
    let mut h = Sha256::new();
    let mut buf = vec![0; 1024 * 1024];
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(format!("{:x}", h.finalize()))
}
fn source_stamp(p: &Profile) -> Result<String, String> {
    let m = fs::metadata(&p.media).map_err(|e| e.to_string())?;
    Ok(format!(
        "{}:{:?}",
        m.len(),
        m.modified().map_err(|e| e.to_string())?
    ))
}
pub fn fingerprint(p: &Profile) -> Result<String, String> {
    let mut h = Sha256::new();
    h.update(b"bootah-efi-certificate-v2");
    h.update(source_stamp(p)?);
    h.update(p.media.as_bytes());
    h.update(p.root.to_string_lossy().as_bytes());
    h.update(
        serde_json::to_vec(&(
            &p.architecture,
            &p.kind,
            &p.kernel,
            &p.initrd,
            &p.arguments,
            &p.boot_files,
        ))
        .map_err(|e| e.to_string())?,
    );
    for name in p.boot_files.values() {
        if !name.is_empty()
            && let Ok(path) = crate::model::safe_file(&p.root, name)
            && path.is_file()
        {
            h.update(name);
            let metadata = path.metadata().map_err(|e| e.to_string())?;
            h.update(format!(
                "{}:{:?}",
                metadata.len(),
                metadata.modified().map_err(|e| e.to_string())?
            ));
            if metadata.len() <= 32 * 1024 * 1024 {
                h.update(hash_file(&path)?);
            }
        }
    }
    Ok(format!("{:x}", h.finalize()))
}
pub fn cached(p: &Profile) -> Option<Support> {
    let s = support(p);
    if !s.eligible {
        return Some(s);
    }
    if !matches!(s.state.as_str(), "passed" | "absent" | "failed") || s.fingerprint.is_empty() {
        return None;
    }
    if s.state == "failed" && crate::model::now().saturating_sub(s.checked_at) >= 30 {
        return None;
    }
    if s.fingerprint != fingerprint(p).ok()? {
        return None;
    }
    Some(s)
}
pub fn validate_current(p: &Profile) -> Result<(), String> {
    let s = p.secure_boot.as_ref().ok_or("Secure Boot check required")?;
    if s.state != "passed" || s.fingerprint != fingerprint(p)? {
        return Err("Secure Boot check expired; check again".into());
    }
    Ok(())
}
fn inspect_certificate(
    bytes: &[u8],
    machine: u16,
    _file: &str,
) -> Result<Option<Component>, String> {
    if !crate::native_uefi::signed_for(bytes, machine) {
        return Ok(None);
    }
    let info = cross_authenticode::AuthenticodeInfo::try_from(bytes)
        .map_err(|e| format!("Cannot read EFI certificate: {e}"))?;
    let Some(cert) = info.certificates.first() else {
        return Ok(None);
    };
    Ok(Some(Component {
        file: _file.into(),
        sha256: format!("{:x}", Sha256::digest(bytes)),
        signer: cert.certificate.tbs_certificate.subject.to_string(),
        issuer: cert.certificate.tbs_certificate.issuer.to_string(),
        certificate_sha256: cert.sha256.iter().map(|b| format!("{b:02x}")).collect(),
    }))
}
#[cfg(test)]
fn certificate(bytes: &[u8], machine: u16, file: &str) -> Result<Component, String> {
    inspect_certificate(bytes, machine, file)?
        .ok_or_else(|| format!("{file}: No matching EFI certificate"))
}
pub fn check(p: &Profile, settings: &Settings) -> Result<Support, String> {
    let mut s = support(p);
    s.components.clear();
    s.fingerprint.clear();
    if !s.eligible {
        s.state = "unsupported".into();
        s.detail = "Unsupported image".into();
        return Ok(s);
    }
    let before = source_stamp(p)?;
    let result = (|| {
        let image = Image::open(Path::new(&p.media))?;
        let loader = crate::efi::for_arch(&p.architecture).ok_or("Unsupported architecture")?;
        let Some(path) = image.find(loader.paths[0]) else {
            return Ok(None);
        };
        let bytes = image.read(path, 32 * 1024 * 1024)?;
        inspect_certificate(&bytes, loader.machine, loader.paths[0])
    })();
    let _ = settings;
    match result {
        Ok(Some(component)) => {
            s.components.push(component);
            s.state = "passed".into();
            s.detail = "EFI certificate detected".into();
        }
        Ok(None) => {
            s.state = "absent".into();
            s.detail = "No matching EFI certificate".into();
        }
        Err(error) => {
            s.state = "failed".into();
            s.detail = error;
        }
    }
    if before != source_stamp(p)? {
        return Err("Image changed during check".into());
    }
    s.source_stamp = before;
    s.fingerprint = fingerprint(p)?;
    s.checked_at = crate::model::now();
    Ok(s)
}
pub fn switch(p: &Profile, settings: &Settings, enabled: bool) -> Result<Profile, String> {
    if !enabled {
        let mut changed = normal(p)?;
        let mut s = support(p);
        s.enabled = false;
        s.fingerprint = fingerprint(&changed).unwrap_or_default();
        changed.secure_boot = Some(s);
        return Ok(changed);
    }
    if !eligible(p) {
        return Err("Unsupported image".into());
    }
    validate_current(p)?;
    let mut changed = normal(p)?;
    let route = Route::capture(&changed);
    crate::native_uefi::prepare(&mut changed, settings)?;
    let mut s = support(p);
    s.enabled = true;
    s.normal = Some(route);
    s.fingerprint = fingerprint(&changed)?;
    changed.secure_boot = Some(s);
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unsigned_component_cannot_pass() {
        assert!(certificate(b"MZ unsigned", 0x8664, "test.efi").is_err());
    }
    #[test]
    fn off_restores_route_without_trusting_client_result() {
        let mut p:Profile=serde_json::from_value(serde_json::json!({"id":"a","name":"a","kind":"windows","root":"/tmp","architecture":"x64","media":"x.iso","kernel":"wimboot","initrd":"sources/boot.wim","boot_files":{"x64-efi":"ipxe.efi"}})).unwrap();
        let r = Route::capture(&p);
        p.kernel = "bootah-native/bootx64.efi".into();
        p.boot_files.insert("x64-efi".into(), p.kernel.clone());
        p.secure_boot = Some(Support {
            normal: Some(r),
            enabled: true,
            ..Default::default()
        });
        let p = switch(&p, &Settings::default(), false).unwrap();
        assert_eq!(p.kernel, "wimboot");
        assert!(!crate::native_uefi::enabled(&p));
        assert!(validate_current(&p).is_err());
    }
    #[test]
    fn detection_cache_tracks_inputs_and_expires_failures() {
        let root = std::env::temp_dir().join(format!("bootah-cache-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let media = root.join("source.iso");
        fs::write(&media, b"source").unwrap();
        fs::write(root.join("entry.efi"), b"efi").unwrap();
        let mut p:Profile=serde_json::from_value(serde_json::json!({"id":"cached","name":"cached","kind":"windows","root":root,"architecture":"x64","media":media,"boot_files":{"x64-efi":"entry.efi"}})).unwrap();
        p.secure_boot = Some(Support {
            state: "passed".into(),
            fingerprint: fingerprint(&p).unwrap(),
            source_stamp: source_stamp(&p).unwrap(),
            checked_at: 1,
            ..Default::default()
        });
        assert!(cached(&p).is_some());
        let mut changed = p.clone();
        changed.kernel = "other".into();
        assert!(cached(&changed).is_none());
        fs::write(root.join("entry.efi"), b"efj").unwrap();
        assert!(cached(&p).is_none());
        p.secure_boot.as_mut().unwrap().fingerprint = fingerprint(&p).unwrap();
        p.secure_boot.as_mut().unwrap().state = "absent".into();
        p.secure_boot.as_mut().unwrap().checked_at = 0;
        assert!(cached(&p).is_some());
        let report = p.secure_boot.as_mut().unwrap();
        report.state = "failed".into();
        report.checked_at = crate::model::now();
        assert!(cached(&p).is_some());
        p.secure_boot.as_mut().unwrap().checked_at = 0;
        assert!(cached(&p).is_none());
        p.secure_boot.as_mut().unwrap().state = "passed".into();
        fs::write(&media, b"changed source").unwrap();
        assert!(cached(&p).is_none());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    #[ignore = "Uses a supplied real ISO"]
    fn inspect_iso() {
        let iso = std::env::var("BOOTAH_CHECK_ISO").unwrap();
        let root = std::env::var("BOOTAH_CHECK_OUTPUT").unwrap();
        let imported =
            crate::media::import_with_options(Path::new(&iso), Path::new(&root), false, false)
                .unwrap();
        let p = &imported.profiles[0];
        let report = check(p, &Settings::default()).unwrap();
        fs::write(
            Path::new(&root).join("check.json"),
            serde_json::to_vec_pretty(&report).unwrap(),
        )
        .unwrap();
        if report.state == "passed" {
            let image = Image::open(Path::new(&p.media)).unwrap();
            let loader = crate::efi::for_arch(&p.architecture).unwrap();
            let mut bytes = image.read(loader.paths[0], 32 * 1024 * 1024).unwrap();
            let pe = u32::from_le_bytes(bytes[60..64].try_into().unwrap()) as usize;
            let optional = u16::from_le_bytes(bytes[pe + 20..pe + 22].try_into().unwrap()) as usize;
            let section = pe + 24 + optional;
            let code =
                u32::from_le_bytes(bytes[section + 20..section + 24].try_into().unwrap()) as usize;
            bytes[code] ^= 1;
            assert!(
                certificate(&bytes, loader.machine, loader.paths[0]).is_ok(),
                "Presence detection must not claim signature integrity validation"
            );
        }
        println!(
            "{}: {} ({} components): {}",
            p.name,
            report.state,
            report.components.len(),
            report.detail
        );
        if std::env::var_os("BOOTAH_EXPECT_PASS").is_some() {
            assert_eq!(report.state, "passed");
        }
    }
}
