use crate::model::{AutoConfig, Config, Profile, Settings, Store};
use std::{fs, io::Read, path::PathBuf};

pub fn methods(profile: &Profile) -> Vec<&'static str> {
    if profile.kind == "windows-ris" {
        return vec!["winnt-sif"];
    }
    if profile.kind == "windows" {
        let setup = crate::media_source::exists(&profile.root, "setup.exe")
            || crate::media_source::exists(&profile.root, "sources/setup.exe");
        let source = [
            "sources/install.wim",
            "sources/install.esd",
            "sources/install.swm",
        ]
        .iter()
        .any(|name| crate::media_source::exists(&profile.root, name));
        return if setup && source {
            vec!["unattend"]
        } else {
            Vec::new()
        };
    }
    if profile.kind != "linux" {
        return Vec::new();
    }
    let args = &profile.arguments;
    let mut found = Vec::new();
    if args.contains("inst.stage2=") || args.contains("inst.repo=") {
        found.push("kickstart");
    }
    if args.contains("install=")
        && (profile.kernel.contains("/loader/linux") || profile.system_id == "opensuse")
    {
        found.push("autoyast");
    }
    if args.contains("cdrom-detect/") || profile.kernel.contains("debian-installer/") {
        found.push("preseed");
        if has_kickseed(profile) {
            found.push("kickseed");
        }
    }
    if profile.system_id == "ubuntu" && args.contains("boot=casper") && ubuntu_autoinstall(profile)
    {
        found.push("autoinstall");
    }
    found
}

fn ubuntu_autoinstall(profile: &Profile) -> bool {
    let Ok(image) = crate::media_source::Image::open(std::path::Path::new(&profile.media)) else {
        return false;
    };
    image
        .find(".disk/info")
        .and_then(|path| image.read(path, 4096).ok())
        .is_some_and(|bytes| {
            String::from_utf8_lossy(&bytes)
                .to_ascii_lowercase()
                .contains("server")
        })
}

fn has_kickseed(profile: &Profile) -> bool {
    let Ok(resource) = crate::media_source::open(&profile.root, &profile.initrd) else {
        return false;
    };
    let mut decoder = flate2::read::GzDecoder::new(resource);
    let mut chunk = [0u8; 64 * 1024];
    let mut overlap = Vec::new();
    let mut total = 0usize;
    while total < 128 * 1024 * 1024 {
        let Ok(n) = decoder.read(&mut chunk) else {
            return false;
        };
        if n == 0 {
            break;
        }
        total += n;
        overlap.extend_from_slice(&chunk[..n]);
        if overlap
            .windows(b"initrd-kickseed".len())
            .any(|x| x == b"initrd-kickseed")
        {
            return true;
        }
        let keep = overlap.len().min(32);
        overlap.drain(..overlap.len() - keep);
    }
    false
}

pub fn key(scope: &str, id: &str) -> String {
    format!("{scope}:{id}")
}

pub fn effective<'a>(
    config: &'a Config,
    profile: &Profile,
    device_id: Option<&str>,
) -> Option<&'a AutoConfig> {
    device_id
        .and_then(|id| config.automations.get(&key("device", id)))
        .filter(|auto| auto.profile_id == profile.id)
        .or_else(|| config.automations.get(&key("profile", &profile.id)))
}

pub fn file(store: &Store, token: &str) -> PathBuf {
    store
        .path
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .join("automation")
        .join(token)
}

pub fn url(settings: &Settings, auto: &AutoConfig) -> String {
    format!(
        "http://{}:{}/auto/{}/",
        settings.server_ip, settings.http_port, auto.token
    )
}

pub fn arguments(profile: &Profile, settings: &Settings, auto: Option<&AutoConfig>) -> String {
    let mut args = crate::boot::render_arguments(profile, settings);
    let Some(auto) = auto else {
        return args;
    };
    let base = url(settings, auto);
    let (key, value, extra) = match auto.method.as_str() {
        "kickstart" => ("inst.ks=", format!("inst.ks={base}config"), ""),
        "kickseed" => ("ks=", format!("ks={base}config"), ""),
        "preseed" => (
            "preseed/url=",
            format!("preseed/url={base}config"),
            "auto=true priority=critical",
        ),
        "autoyast" => ("autoyast=", format!("autoyast={base}config"), ""),
        "autoinstall" => (
            "ds=nocloud",
            format!("'ds=nocloud-net;s={base}'"),
            "autoinstall",
        ),
        _ => return args,
    };
    let (head, tail) = if let Some((head, tail)) = args.split_once(" --- ") {
        (head.to_string(), format!(" --- {tail}"))
    } else if let Some(head) = args.strip_suffix(" --") {
        (head.to_string(), " --".to_string())
    } else {
        (args.clone(), String::new())
    };
    let mut words = head
        .split_whitespace()
        .filter(|word| !word.trim_start_matches(['\'', '"']).starts_with(key))
        .map(str::to_string)
        .collect::<Vec<_>>();
    if !extra.is_empty() {
        for word in extra.split_whitespace() {
            if !words.iter().any(|existing| existing == word) {
                words.push(word.into());
            }
        }
    }
    words.push(value);
    args = words.join(" ") + &tail;
    args
}

pub fn read_token(store: &Store, token: &str, name: &str) -> Option<Vec<u8>> {
    let auto = store
        .config
        .automations
        .values()
        .find(|auto| auto.token == token)?;
    match (auto.method.as_str(), name) {
        ("autoinstall", "user-data") => fs::read(file(store, token)).ok(),
        ("autoinstall", "meta-data") => Some(format!("instance-id: bootah-{token}\n").into_bytes()),
        ("autoinstall", "vendor-data") => Some(Vec::new()),
        ("unattend", "unattend.xml") => fs::read(file(store, token)).ok(),
        ("kickstart" | "kickseed" | "preseed" | "autoyast", "config") => {
            fs::read(file(store, token)).ok()
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_original_boot_arguments() {
        let profile = Profile { secure_boot: None,
            id: "test".into(),
            name: "test".into(),
            system_id: String::new(),
            system_name: String::new(),
            architecture: "x64".into(),
            kind: "linux".into(),
            root: PathBuf::new(),
            boot_files: Default::default(),
            kernel: String::new(),
            initrd: String::new(),
            dtb: String::new(),
            arguments: "console=ttyS0 acpi=off inst.ks=http://old/config --- quiet splash".into(),
            media: String::new(),
            status: String::new(),
            notes: String::new(),
            boot_evidence: None,
        };
        let settings = Settings::default();
        let auto = AutoConfig {
            method: "kickstart".into(),
            token: "secret".into(),
            ..Default::default()
        };
        let args = arguments(&profile, &settings, Some(&auto));
        assert!(args.contains("console=ttyS0 acpi=off"));
        assert!(args.contains(" --- quiet splash"));
        assert!(!args.contains("http://old/config"));
        assert!(args.contains("inst.ks=http://"));
        let mut config = Config::default();
        config
            .automations
            .insert(key("profile", "test"), auto.clone());
        let override_auto = AutoConfig {
            profile_id: "test".into(),
            token: "device-token".into(),
            ..auto
        };
        config
            .automations
            .insert(key("device", "machine"), override_auto);
        assert_eq!(
            effective(&config, &profile, Some("machine")).unwrap().token,
            "device-token"
        );
        assert_eq!(effective(&config, &profile, None).unwrap().token, "secret");
    }
}
