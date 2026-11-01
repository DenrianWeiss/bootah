use crate::model::*;
use std::{collections::BTreeMap, fs, path::Path};

pub const LOADER: &[u8] = include_bytes!("../../../assets/bootah-obp.bin");
pub const OPENBSD_NOTES: &str = "OpenBSD SPARC64: Bootah OBP loads original ofwboot.net and bsd.rd. Use boot net; enable Ethernet protocols and DHCP or ProxyDHCP, and assign an image. A configured IP takes priority; otherwise Bootah obtains a DHCP lease for RARP. Hardware boot validation is pending.";
pub fn enabled(p: &Profile) -> bool {
    linux(p) || native_file(p).is_some()
}
fn linux(p: &Profile) -> bool {
    p.architecture == "sparc64" && p.kind == "linux" && p.boot_files.contains_key("sparc64_grub")
}
fn native_kind(p: &Profile) -> Option<&'static str> {
    if p.architecture != "sparc64" || p.kind != "bsd" { return None; }
    if crate::openbsd::network_root(p) && p.kernel == "bsd.rd" { return Some("bsd.rd"); }
    let marker: serde_json::Value = serde_json::from_slice(&fs::read(p.root.join(".bootah-netbsd-nfs.json")).ok()?).ok()?;
    (marker["version"] == 1 && marker["profile"].as_str() == Some(&p.id)
        && marker["port"] == "sparc64" && p.kernel == "netbsd").then_some("netbsd")
}
pub fn upgrade_native(p: &mut Profile) {
    if native_kind(p).is_none() { return; }
    if let Some(target) = native_file(p) {
        p.boot_files.insert("sparc64_native".into(), target);
        p.boot_files.insert("default".into(), "bootah-obp.bin".into());
        p.boot_files.insert("sparc64".into(), "bootah-obp.bin".into());
        if p.system_id == "openbsd" && (p.notes.starts_with("OpenBSD SPARC64: original ofwboot.net and bsd.rd,") || p.notes.starts_with("OpenBSD SPARC64: Bootah OBP loads original ofwboot.net and bsd.rd.")) {
            p.notes = OPENBSD_NOTES.into();
        }
    }
}
fn native_file(p: &Profile) -> Option<String> {
    native_kind(p)?;
    let name = p.boot_files.get("sparc64_native").or_else(|| p.boot_files.get("default"))?;
    if name.len() > 127 || name.rsplit('/').next() != Some("ofwboot.net")
        || name.contains([':', ',', '\\']) || name.bytes().any(|b| !(33..127).contains(&b)) { return None; }
    let path = safe_file(&p.root, name).ok()?;
    if fs::metadata(&path).ok()?.len() > 2 * 1024 * 1024 { return None; }
    let bytes = fs::read(path).ok()?;
    if bytes.get(..7) != Some(b"\x7fELF\x02\x02\x01") || bytes.get(18..20) != Some(&[0,43]) { return None; }
    Some(name.clone())
}
pub fn native_config(p: &Profile, settings: &Settings, auto: Option<&AutoConfig>) -> Option<Vec<u8>> {
    let target = native_file(p)?;
    let kernel = native_kind(p)?;
    let arguments = crate::automation::arguments(p, settings, auto);
    if arguments.bytes().any(|b| b < 32 || b == 127) { return None; }
    let args = format!("{kernel}{}{}", if arguments.is_empty() { "" } else { " " }, arguments);
    if args.len() > 511 { return None; }
    Some(format!("BOOTAH-OBP/1\nloader={target}\nbootargs={args}\n").into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_upgrade_and_stage_routing() {
        let root = std::env::temp_dir().join(format!("bootah-obp-native-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join(".bootah-netbsd-nfs.json"), br#"{"version":1,"profile":"native","port":"sparc64"}"#).unwrap();
        let mut image = vec![0; 64];
        image[..7].copy_from_slice(b"\x7fELF\x02\x02\x01");
        image[19] = 43;
        fs::write(root.join("ofwboot.net"), image).unwrap();
        let mut profile: Profile = serde_json::from_value(serde_json::json!({
            "id":"native", "name":"Native BSD", "architecture":"sparc64", "kind":"bsd", "root":root,
            "kernel":"netbsd", "boot_files":{"default":"ofwboot.net"}
        })).unwrap();
        upgrade_native(&mut profile);
        assert_eq!(profile.boot_files["sparc64"], "bootah-obp.bin");
        assert_eq!(profile.boot_files["sparc64_native"], "ofwboot.net");
        assert_eq!(next_file(&profile).as_deref(), Some("bootah-obp.cfg"));
        assert_eq!(native_config(&profile, &Settings::default(), None).unwrap(),
            b"BOOTAH-OBP/1\nloader=ofwboot.net\nbootargs=netbsd\n");
        profile.boot_files.insert("sparc64_native".into(), "bootah-obp.bin".into());
        assert!(next_file(&profile).is_none());
        fs::remove_dir_all(root).unwrap();
    }

}
pub fn valid_grub(image: &[u8]) -> bool {
    let field = |at: usize| u32::from_be_bytes(image[at..at + 4].try_into().unwrap());
    image.len() >= 64 && image.len() <= 2 * 1024 * 1024
        && field(0) == 0x01030107 && field(4) as usize == image.len() - 32
        && [8, 12, 16, 24, 28].iter().all(|&at| field(at) == 0)
        && field(20) == 0x4400 && field(32) & 0xffc00000 == 0x10800000
        && field(36) == 0x9010000c
}
pub fn next_file(p: &Profile) -> Option<String> {
    if native_file(p).is_some() { return Some("bootah-obp.cfg".into()); }
    if !linux(p) { return None; }
    let name = p.boot_files.get("sparc64_grub")?;
    if name.is_empty() || name.len() > 127 || name.contains([':', ',', '\\'])
        || name.bytes().any(|b| !(33..127).contains(&b)) { return None; }
    let file = crate::model::safe_file(&p.root, name).ok()?;
    if fs::metadata(&file).ok()?.len() > 2 * 1024 * 1024 { return None; }
    let image = fs::read(file).ok()?;
    valid_grub(&image).then(|| name.clone())
}
pub fn prepare(root: &Path, files: &mut BTreeMap<String, String>) -> Result<(), String> {
    let path = crate::media::asset("grub-sparc64.img")
        .ok_or("SPARC64 requires grub-sparc64.img")?;
    let image = fs::read(path).map_err(|e| e.to_string())?;
    if !valid_grub(&image) { return Err("Invalid SPARC64 GRUB a.out network image".into()); }
    fs::write(root.join("bootah-obp.bin"), LOADER).map_err(|e| e.to_string())?;
    fs::create_dir_all(root.join("bootah-obp")).map_err(|e| e.to_string())?;
    fs::write(root.join("bootah-obp/grub.img"), image).map_err(|e| e.to_string())?;
    files.insert("default".into(), "bootah-obp.bin".into());
    files.insert("sparc64".into(), "bootah-obp.bin".into());
    files.insert("sparc64_grub".into(), "bootah-obp/grub.img".into());
    Ok(())
}
pub fn grub(p: &Profile, settings: &Settings, auto: Option<&AutoConfig>) -> Option<Vec<u8>> {
    if !linux(p) { return None; }
    let quote = |s: &str| format!("'{}'", s.replace('\\', "\\\\").replace('\'', "'\\''"));
    let args = crate::automation::arguments(p, settings, auto).replace(['\n', '\r'], " ");
    let initrd = if p.initrd.is_empty() { String::new() } else { format!("initrd {}\n", quote(&format!("/{}", p.initrd))) };
    Some(format!("set timeout=0\nset default=0\nmenuentry 'Bootah' {{\nlinux {} {}\n{initrd}boot\n}}\n", quote(&format!("/{}", p.kernel)), args).into_bytes())
}
