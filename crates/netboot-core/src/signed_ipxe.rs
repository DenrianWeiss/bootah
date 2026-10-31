use crate::{media_source::Image, model::{AutoConfig, Profile, Settings}};
use sha2::{Digest, Sha256};
use std::{fs, path::Path};

pub fn directory(p: &Profile) -> Option<&str> {
    let loader = crate::efi::for_arch(&p.architecture)?;
    let entry = p.boot_files.get(loader.firmware_key)?;
    let dir = entry.strip_suffix("/ipxe-shim.efi")?;
    (dir.starts_with("bootah-ipxe-") && !dir.contains('/')).then_some(dir)
}
pub fn enabled(p: &Profile) -> bool { directory(p).is_some() }
pub fn is_script(p: &Profile, name: &str) -> bool {
    directory(p).is_some_and(|dir| name == format!("{dir}/autoexec.ipxe"))
}
pub fn prepare(p: &mut Profile) -> Result<(), String> {
    let loader = crate::efi::for_arch(&p.architecture).ok_or("Unsupported architecture")?;
    let image = Image::open(Path::new(&p.media))?;
    let name = image.find(loader.paths[0]).ok_or("Missing signed ISO component")?;
    let shim = image.read(name, 32 * 1024 * 1024)?;
    if !crate::native_uefi::signed_for(&shim, loader.machine) { return Err("No matching EFI certificate".into()); }
    let dir = format!("bootah-ipxe-{}", uuid::Uuid::new_v4());
    let root = p.root.join(&dir);
    fs::create_dir(&root).map_err(|e|e.to_string())?;
    let result = (|| {
        let manifest: serde_json::Value = serde_json::from_str(include_str!("../../../assets/signed-ipxe.json")).map_err(|e|e.to_string())?;
        let mut components = vec![];
        for file in ["ipxe-shim.efi", "ipxe.efi"] {
            let asset = format!("signed-{}-{file}",p.architecture);
            let source = crate::media::asset(&asset).ok_or_else(||format!("Missing signed iPXE asset: {asset}"))?;
            let bytes = fs::read(source).map_err(|e|e.to_string())?;
            let hash = format!("{:x}",Sha256::digest(&bytes));
            if manifest["files"][&asset]["sha256"].as_str() != Some(hash.as_str()) || !crate::native_uefi::signed_for(&bytes,loader.machine) {
                return Err(format!("Signed iPXE asset verification failed: {asset}"));
            }
            fs::write(root.join(file),bytes).map_err(|e|e.to_string())?;
            components.push(serde_json::json!({"file":file,"sha256":hash}));
        }
        fs::write(root.join("distro.efi"),&shim).map_err(|e|e.to_string())?;
        components.push(serde_json::json!({"file":"distro.efi","source":name,"sha256":format!("{:x}",Sha256::digest(&shim))}));
        fs::write(root.join("components.json"),serde_json::to_vec_pretty(&serde_json::json!({"release":manifest["release"],"components":components})).unwrap()).map_err(|e|e.to_string())?;
        Ok(())
    })();
    if let Err(error) = result { let _ = fs::remove_dir_all(root); return Err(error); }
    p.boot_files.clear();
    p.boot_files.insert(loader.firmware_key.into(),format!("{dir}/ipxe-shim.efi"));
    p.status = "prepared".into();
    if let Some(e) = p.boot_evidence.as_mut() { e.verification = "signed-ipxe-unverified".into(); }
    Ok(())
}
pub fn script(p: &Profile, settings: &Settings, auto: Option<&AutoConfig>) -> String {
    let dir = directory(p).unwrap_or_default();
    let http = crate::boot::http_root(settings,p);
    let base = if settings.boot_transport == "tftp" { format!("tftp://{}",settings.server_ip) } else { http.clone() };
    let mut script = "#!ipxe\nshow efi/SecureBoot\nshow efi/SetupMode\nisset ${netX/ip} || dhcp\n".to_string();
    if p.kind == "linux" {
        let args = crate::automation::arguments(p,settings,auto);
        let mut words: Vec<_> = args.split_whitespace().collect();
        if !words.iter().any(|w|w.starts_with("BOOTIF=")) {
            let at=words.iter().position(|w|matches!(*w,"--"|"---")).unwrap_or(words.len());
            words.insert(at,"BOOTIF=01-${netX/mac:hexhyp}");
        }
        script.push_str(&format!("kernel {base}/{} {} || goto failed\n",p.kernel,words.join(" ")));
        if !p.initrd.is_empty() { script.push_str(&format!("initrd {base}/{} || goto failed\n",p.initrd)); }
        script.push_str(&format!("shim {base}/{dir}/distro.efi || goto failed\nboot || goto failed\n"));
    } else {
        script.push_str(&format!("sanboot --no-describe {http}/media.iso || goto failed\n"));
    }
    script.push_str(":failed\necho Secure Boot failed; no fallback was attempted\nprompt Press any key to return\nexit\n");
    script
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scripts_keep_vendor_verification_and_isolate_autoexec() {
        let mut p:Profile=serde_json::from_value(serde_json::json!({"id":"p","name":"p","root":"/tmp","kind":"linux","architecture":"x64","kernel":"vmlinuz","initrd":"initrd","boot_files":{"x64-efi":"bootah-ipxe-test/ipxe-shim.efi"}})).unwrap();
        let settings=Settings::default();
        let text=script(&p,&settings,None);
        assert!(text.contains("/bootah-ipxe-test/distro.efi || goto failed"));
        assert!(text.contains("BOOTIF=01-${netX/mac:hexhyp}"));
        assert!(text.contains("boot || goto failed"));
        assert!(is_script(&p,"bootah-ipxe-test/autoexec.ipxe"));
        assert!(!is_script(&p,"bootah-ipxe-other/autoexec.ipxe"));
        p.kind="bsd".into();let text=script(&p,&settings,None);
        assert!(text.contains("sanboot --no-describe"));assert!(!text.contains("kernel "));
    }
}
