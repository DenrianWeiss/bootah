use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    net::Ipv4Addr,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub server_ip: Ipv4Addr,
    pub bind_ip: Ipv4Addr,
    pub interface: String,
    pub server_mac: String,
    pub boot_transport: String,
    pub ethernet_enabled: bool,
    pub dhcp_port: u16,
    pub tftp_port: u16,
    pub http_port: u16,
    pub pool_start: Ipv4Addr,
    pub pool_end: Ipv4Addr,
    pub subnet: Ipv4Addr,
    pub router: Option<Ipv4Addr>,
    pub dhcp_auto: bool,
    pub dhcp_enabled: bool,
    pub proxy_dhcp_enabled: bool,
    pub pxe_port: u16,
    pub ris_smb_port: u16,
    pub smb_mode: String,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            server_ip: Ipv4Addr::new(192, 168, 50, 1),
            bind_ip: Ipv4Addr::UNSPECIFIED,
            interface: String::new(),
            server_mac: String::new(),
            boot_transport: "http".into(),
            ethernet_enabled: false,
            dhcp_port: 67,
            tftp_port: 69,
            http_port: 8080,
            pool_start: Ipv4Addr::new(192, 168, 50, 100),
            pool_end: Ipv4Addr::new(192, 168, 50, 200),
            subnet: Ipv4Addr::new(255, 255, 255, 0),
            router: None,
            dhcp_auto: false,
            dhcp_enabled: false,
            proxy_dhcp_enabled: false,
            pxe_port: 4011,
            ris_smb_port: 445,
            smb_mode: "auto".into(),
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Profile {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub system_id: String,
    #[serde(default)]
    pub system_name: String,
    pub architecture: String,
    pub kind: String,
    pub root: PathBuf,
    #[serde(default)]
    pub boot_files: BTreeMap<String, String>,
    #[serde(default)]
    pub kernel: String,
    #[serde(default)]
    pub initrd: String,
    #[serde(default)]
    pub dtb: String,
    #[serde(default)]
    pub arguments: String,
    #[serde(default)]
    pub media: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub boot_evidence: Option<BootEvidence>,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BootEvidence {
    pub config: String,
    pub entry: String,
    pub kernel: String,
    pub initrd: String,
    pub source: String,
    pub verification: String,
    pub diagnostics: Vec<BootDiagnostic>,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BootDiagnostic {
    pub code: String,
    pub detail: String,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Device {
    pub id: String,
    pub mac: String,
    pub ip: Option<Ipv4Addr>,
    pub name: String,
    pub model: String,
    pub architecture: String,
    pub protocol: String,
    pub vendor: String,
    pub uuid: String,
    pub profile_id: Option<String>,
    pub last_seen: u64,
    pub auto_registered: bool,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub settings: Settings,
    pub devices: Vec<Device>,
    pub profiles: Vec<Profile>,
    pub default_profiles: BTreeMap<String, String>,
    pub automations: BTreeMap<String, AutoConfig>,
    #[serde(skip)]
    pub pending_device_conflicts: Vec<DeviceConflict>,
    #[serde(skip)]
    pub declined_device_conflicts: Vec<DeviceConflict>,
}
#[derive(Clone, Serialize)]
pub struct DeviceConflict {
    pub id: String,
    pub old_id: String,
    pub old_mac: String,
    pub new_mac: String,
    pub ip: Option<Ipv4Addr>,
    pub old_architecture: String,
    pub new_architecture: String,
    pub reason: String,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AutoConfig {
    pub profile_id: String,
    pub method: String,
    pub token: String,
    pub filename: String,
}
#[derive(Clone, Serialize)]
pub struct Event {
    pub time: u64,
    pub device: String,
    pub protocol: String,
    pub message: String,
}
pub struct Store {
    pub config: Config,
    pub events: Vec<Event>,
    pub path: PathBuf,
}
pub type Shared = Arc<Mutex<Store>>;
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
pub fn mac(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect::<Vec<_>>()
        .join(":")
}
pub fn normalize_mac(s: &str) -> Option<String> {
    let compact: String = s
        .chars()
        .filter(|c| !matches!(c, ':' | '-' | '.'))
        .collect();
    if compact.len() != 12 || !compact.is_ascii() {
        return None;
    }
    let bytes = (0..6)
        .map(|i| u8::from_str_radix(&compact[i * 2..i * 2 + 2], 16))
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    Some(mac(&bytes))
}
pub fn architecture(code: u16) -> &'static str {
    match code {
        0 => "x86-bios",
        2 => "ia64",
        6 => "ia32-efi",
        7 => "x64-efi",
        9 => "ebc-efi",
        3 => "alpha",
        12 => "powerpc-of",
        13 => "powerpc-epapr",
        14 => "power-opal",
        10 => "arm32-efi",
        11 => "arm64-efi",
        15 => "ia32-http",
        16 => "x64-http",
        18 => "arm32-http",
        19 => "arm64-http",
        21 => "arm32-uboot",
        22 => "arm64-uboot",
        25 => "riscv32-efi",
        28 => "riscv64-http",
        39 => "loongarch64-efi",
        27 => "riscv64-efi",
        _ => "unknown",
    }
}
pub fn known_firmware_architecture(name: &str) -> bool {
    matches!(
        name,
        "x86-bios"
            | "ia64"
            | "ia32-efi"
            | "x64-efi"
            | "ebc-efi"
            | "alpha"
            | "powerpc-of"
            | "powerpc-epapr"
            | "power-opal"
            | "arm32-efi"
            | "arm64-efi"
            | "ia32-http"
            | "x64-http"
            | "arm32-http"
            | "arm64-http"
            | "arm32-uboot"
            | "arm64-uboot"
            | "riscv32-efi"
            | "riscv64-http"
            | "loongarch64-efi"
            | "riscv64-efi"
            | "hp-rmp"
    )
}
impl Store {
    pub fn open(path: PathBuf) -> Result<Shared, String> {
        let mut config: Config = if path.exists() {
            serde_json::from_slice(&std::fs::read(&path).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?
        } else {
            let mut config = Config::default();
            config.settings.dhcp_auto = true;
            config
        };
        if matches!(config.settings.smb_mode.as_str(), "smb1" | "smb2") { config.settings.smb_mode = "auto".into(); }
        #[cfg(target_os = "windows")]
        {
            config.settings.interface = crate::network::windows_capture_id(&config.settings.interface);
        }
        Ok(Arc::new(Mutex::new(Self {
            config,
            events: vec![],
            path,
        })))
    }
    pub fn save(&self) -> Result<(), String> {
        let temporary = self.path.with_extension("json.tmp");
        std::fs::write(
            &temporary,
            serde_json::to_vec_pretty(&self.config).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        std::fs::rename(temporary, &self.path).map_err(|e| e.to_string())
    }
    pub fn clear_events(&mut self) -> Result<(), String> {
        let path = self.path.with_extension("events.jsonl");
        for file in [path.with_extension("jsonl.1"), path] {
            match std::fs::remove_file(file) {
                Ok(()) => (),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(e) => return Err(e.to_string()),
            }
        }
        self.events.clear();
        Ok(())
    }
    pub fn event(&mut self, device: &str, protocol: &str, message: impl Into<String>) {
        self.events.push(Event {
            time: now(),
            device: device.into(),
            protocol: protocol.into(),
            message: message.into(),
        });
        let entry = self.events.last().unwrap();
        let path = self.path.with_extension("events.jsonl");
        let result = (|| -> std::io::Result<()> {
            use std::io::Write;
            if std::fs::metadata(&path).is_ok_and(|m| m.len() >= 4 * 1024 * 1024) {
                let previous = path.with_extension("jsonl.1");
                if previous.exists() {
                    std::fs::remove_file(&previous)?;
                }
                std::fs::rename(&path, previous)?;
            }
            let mut options = std::fs::OpenOptions::new();
            options.create(true).append(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(path)?;
            serde_json::to_writer(&mut file, entry)?;
            file.write_all(b"\n")
        })();
        if let Err(e) = result {
            eprintln!("Event log: {e}");
        }
        if self.events.len() > 1000 {
            self.events.drain(..100);
        }
    }
    pub fn associate(&mut self, ip: Ipv4Addr, mac: &str) -> bool {
        let Some(mac) = normalize_mac(mac) else {
            return false;
        };
        if self
            .config
            .devices
            .iter()
            .any(|d| d.ip == Some(ip) && !d.mac.is_empty() && d.mac != mac)
        {
            return false;
        }
        let by_mac = self.config.devices.iter().position(|d| d.mac == mac);
        let by_ip = self
            .config
            .devices
            .iter()
            .position(|d| d.ip == Some(ip) && d.mac.is_empty());
        if by_mac.is_none() && by_ip.is_none() {
            return false;
        }
        if self.config.settings.dhcp_enabled
            && by_ip.is_none()
            && by_mac
                .is_some_and(|index| self.config.devices[index].ip.is_some_and(|old| old != ip))
        {
            return false;
        }
        let merging = by_mac.is_some() && by_ip.is_some();
        let id = mac.replace(':', "");
        if let (Some(a), Some(b)) = (by_mac, by_ip) {
            if self.config.devices[a].profile_id.is_some()
                && self.config.devices[b].profile_id.is_some()
                && self.config.devices[a].profile_id != self.config.devices[b].profile_id
            {
                return false;
            }
            let old_key = format!("device:{}", self.config.devices[b].id);
            let new_key = format!("device:{id}");
            if let (Some(a), Some(b)) = (
                self.config.automations.get(&old_key),
                self.config.automations.get(&new_key),
            ) {
                if serde_json::to_value(a).ok() != serde_json::to_value(b).ok() {
                    return false;
                }
            }
            let old = self.config.devices[b].clone();
            let target = &mut self.config.devices[a];
            if target.profile_id.is_none() {
                target.profile_id = old.profile_id;
            }
            if target.name.is_empty() {
                target.name = old.name;
            }
            if target.model.is_empty() {
                target.model = old.model;
            }
            if target.architecture.is_empty() {
                target.architecture = old.architecture;
            }
            if target.uuid.is_empty() {
                target.uuid = old.uuid;
            }
            let key = format!("device:{}", old.id);
            if let Some(auto) = self.config.automations.remove(&key) {
                self.config
                    .automations
                    .entry(format!("device:{id}"))
                    .or_insert(auto);
            }
            self.config.devices.remove(b);
        }
        let index = self
            .config
            .devices
            .iter()
            .position(|d| d.mac == mac)
            .or_else(|| {
                self.config
                    .devices
                    .iter()
                    .position(|d| d.ip == Some(ip) && d.mac.is_empty())
            })
            .unwrap();
        let device = &mut self.config.devices[index];
        if device.ip == Some(ip) && device.mac == mac {
            return merging;
        }
        let old_id = device.id.clone();
        device.id = id.clone();
        device.mac = mac;
        device.ip = Some(ip);
        if old_id != id {
            if let Some(auto) = self.config.automations.remove(&format!("device:{old_id}")) {
                self.config
                    .automations
                    .entry(format!("device:{id}"))
                    .or_insert(auto);
            }
        }
        self.event(&id, "network", format!("Associated {ip}"));
        true
    }
    pub fn observe_mac(&mut self, address: &str, protocol: &str) -> Option<usize> {
        let index = if let Some(index) = self.config.devices.iter().position(|d| d.mac == address) {
            index
        } else {
            if self.config.devices.len() >= 4096 {
                return None;
            }
            self.config.devices.push(Device {
                id: address.replace(':', ""),
                mac: address.into(),
                auto_registered: true,
                ..Default::default()
            });
            self.event(address, protocol, "Discovered device");
            self.config.devices.len() - 1
        };
        let device = &mut self.config.devices[index];
        device.last_seen = now();
        device.protocol = protocol.into();
        Some(index)
    }
    fn queue_device_conflict(
        &mut self,
        old: &Device,
        new_mac: &str,
        ip: Option<Ipv4Addr>,
        new_architecture: &str,
        reason: &str,
    ) {
        let same = |item: &DeviceConflict| {
            item.old_id == old.id
                && item.new_mac == new_mac
                && item.ip == ip
                && item.new_architecture == new_architecture
                && item.reason == reason
        };
        if self.config.pending_device_conflicts.iter().any(same)
            || self.config.declined_device_conflicts.iter().any(same)
        {
            return;
        }
        self.config.pending_device_conflicts.push(DeviceConflict {
            id: uuid::Uuid::new_v4().to_string(),
            old_id: old.id.clone(),
            old_mac: old.mac.clone(),
            new_mac: new_mac.into(),
            ip,
            old_architecture: old.architecture.clone(),
            new_architecture: new_architecture.into(),
            reason: reason.into(),
        });
        self.event(
            new_mac,
            "device",
            "Device identity changed; confirmation required",
        );
    }
    fn clear_device(&mut self, index: usize) -> Device {
        let old = self.config.devices.remove(index);
        self.config
            .automations
            .remove(&format!("device:{}", old.id));
        old
    }
    pub fn reconcile_ip_identity(&mut self, mac: &str, ip: Ipv4Addr) -> bool {
        let Some(index) = self
            .config
            .devices
            .iter()
            .position(|d| d.ip == Some(ip) && !d.mac.is_empty() && d.mac != mac)
        else {
            return true;
        };
        let old = self.config.devices[index].clone();
        if !old.auto_registered {
            self.queue_device_conflict(&old, mac, Some(ip), "", "ip-mac");
            return false;
        }
        self.clear_device(index);
        if !self.config.devices.iter().any(|d| d.mac == mac) {
            self.config.devices.push(Device {
                id: mac.replace(':', ""),
                mac: mac.into(),
                ip: Some(ip),
                auto_registered: true,
                ..Default::default()
            });
        }
        self.event(
            mac,
            "device",
            "Replaced automatically registered device at reused IP",
        );
        true
    }
    pub fn reconcile_architecture(
        &mut self,
        index: usize,
        architecture: &str,
        observed_ip: Option<Ipv4Addr>,
    ) -> bool {
        let old = self.config.devices[index].clone();
        if old.architecture.is_empty()
            || old.architecture == "unknown"
            || old.architecture == architecture
        {
            return true;
        }
        if !old.auto_registered {
            self.queue_device_conflict(
                &old,
                &old.mac,
                observed_ip,
                architecture,
                "mac-architecture",
            );
            return false;
        }
        self.clear_device(index);
        self.config.devices.insert(
            index,
            Device {
                id: old.id.clone(),
                mac: old.mac.clone(),
                ip: observed_ip,
                architecture: architecture.into(),
                protocol: old.protocol,
                last_seen: now(),
                auto_registered: true,
                ..Default::default()
            },
        );
        self.event(&old.mac,"device",format!("Replaced automatically registered device after architecture changed to {architecture}"));
        true
    }
    pub fn resolve_device_conflict(&mut self, id: &str, accept: bool) -> Result<(), String> {
        let position = self
            .config
            .pending_device_conflicts
            .iter()
            .position(|item| item.id == id)
            .ok_or("Device conflict not found")?;
        let conflict = self.config.pending_device_conflicts[position].clone();
        if !accept {
            self.config.pending_device_conflicts.remove(position);
            self.config.declined_device_conflicts.push(conflict);
            return Ok(());
        }
        let index = self
            .config
            .devices
            .iter()
            .position(|d| d.id == conflict.old_id && d.mac == conflict.old_mac)
            .ok_or("Old device no longer exists")?;
        self.config.pending_device_conflicts.remove(position);
        self.clear_device(index);
        if conflict.reason == "mac-architecture" {
            self.config.devices.push(Device {
                id: conflict.new_mac.replace(':', ""),
                mac: conflict.new_mac.clone(),
                ip: conflict.ip,
                architecture: conflict.new_architecture,
                auto_registered: true,
                ..Default::default()
            });
        } else if let Some(device) = self
            .config
            .devices
            .iter_mut()
            .find(|d| d.mac == conflict.new_mac)
        {
            device.ip = conflict.ip;
        } else {
            self.config.devices.push(Device {
                id: conflict.new_mac.replace(':', ""),
                mac: conflict.new_mac.clone(),
                ip: conflict.ip,
                auto_registered: true,
                ..Default::default()
            });
        }
        self.event(
            &conflict.new_mac,
            "device",
            "Replaced device after user confirmation",
        );
        self.save()
    }
    pub fn device_for(&self, address: Ipv4Addr) -> Option<&Device> {
        let observed = crate::neighbors::cached_mac(address, &self.config.settings.interface);
        if let Some(device) = self.config.devices.iter().find(|d| d.ip == Some(address)) {
            if device.mac.is_empty() || observed.as_ref().is_none_or(|mac| mac == &device.mac) {
                return Some(device);
            }
            return None;
        }
        let mac = observed?;
        self.config.devices.iter().find(|d| d.mac == mac)
    }
    pub fn profile_for(&self, address: Ipv4Addr) -> Option<Profile> {
        let device = self.device_for(address)?;
        let id = device
            .profile_id
            .as_ref()
            .or_else(|| self.config.default_profiles.get(&device.architecture))?;
        self.config.profiles.iter().find(|p| &p.id == id).cloned()
    }
}
pub fn safe_file(root: &Path, requested: &str) -> Result<PathBuf, String> {
    if requested.contains('\\') || requested.contains('\0') {
        return Err("Invalid path".into());
    }
    let relative = Path::new(requested.trim_start_matches('/'));
    if relative
        .components()
        .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return Err("Invalid path".into());
    }
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let path = root
        .join(relative)
        .canonicalize()
        .map_err(|e| e.to_string())?;
    if !path.starts_with(root) || !path.is_file() {
        return Err("File unavailable".into());
    }
    Ok(path)
}
pub fn valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 80
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

#[cfg(test)]
mod association_tests {
    use super::*;
    #[test]
    fn clear_events_removes_history_and_allows_new_events() {
        let directory = std::env::temp_dir().join(format!("bootah-events-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("state.json");
        let state = Store::open(path.clone()).unwrap();
        let mut store = state.lock().unwrap();
        store.event("device", "HTTP", "before clearing");
        let log = path.with_extension("events.jsonl");
        let previous = log.with_extension("jsonl.1");
        std::fs::copy(&log, &previous).unwrap();
        store.clear_events().unwrap();
        assert!(store.events.is_empty());
        assert!(!log.exists() && !previous.exists());
        store.clear_events().unwrap();
        store.event("device", "HTTP", "after clearing");
        let history = std::fs::read_to_string(log).unwrap();
        assert!(history.contains("after clearing") && !history.contains("before clearing"));
        drop(store);
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn changed_identity_replaces_auto_device_and_waits_for_manual_confirmation() {
        let path = std::env::temp_dir().join(format!(
            "bootah-device-conflict-{}.json",
            uuid::Uuid::new_v4()
        ));
        let state = Store::open(path.clone()).unwrap();
        let mut s = state.lock().unwrap();
        let ip = "192.0.2.20".parse().unwrap();
        let old = "52:54:00:00:00:01";
        let new = "52:54:00:00:00:02";
        s.config.devices.push(Device {
            id: old.replace(':', ""),
            mac: old.into(),
            ip: Some(ip),
            architecture: "x86-bios".into(),
            auto_registered: true,
            ..Default::default()
        });
        assert!(s.reconcile_ip_identity(new, ip));
        assert_eq!(s.config.devices.len(), 1);
        assert_eq!(s.config.devices[0].mac, new);
        s.config.devices[0].architecture = "x86-bios".into();
        assert!(s.reconcile_architecture(0, "x64-efi", Some(ip)));
        assert_eq!(s.config.devices[0].architecture, "x64-efi");
        s.config.devices[0].auto_registered = false;
        assert!(!s.reconcile_architecture(0, "arm64-efi", Some(ip)));
        assert_eq!(s.config.devices[0].architecture, "x64-efi");
        let id = s.config.pending_device_conflicts[0].id.clone();
        s.resolve_device_conflict(&id, true).unwrap();
        assert_eq!(s.config.devices.len(), 1);
        assert_eq!(s.config.devices[0].architecture, "arm64-efi");
        assert!(s.config.pending_device_conflicts.is_empty());
        drop(s);
        drop(state);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("events.jsonl"));
    }
    #[test]
    fn ip_fallback_merges_without_losing_assignment_or_overwriting_conflicts() {
        let path =
            std::env::temp_dir().join(format!("bootah-association-{}.json", uuid::Uuid::new_v4()));
        let state = Store::open(path.clone()).unwrap();
        let mut s = state.lock().unwrap();
        let first = "192.0.2.10".parse().unwrap();
        let second = "192.0.2.11".parse().unwrap();
        s.config.profiles.push(serde_json::from_value(serde_json::json!({"id":"installer","name":"Installer","architecture":"x64","kind":"linux","root":"/tmp"})).unwrap());
        s.config.devices.push(Device {
            id: "ip-192.0.2.10".into(),
            ip: Some(first),
            profile_id: Some("installer".into()),
            ..Default::default()
        });
        s.config
            .automations
            .insert("device:ip-192.0.2.10".into(), AutoConfig::default());
        assert_eq!(s.profile_for(first).unwrap().id, "installer");
        assert!(s.associate(first, "52:54:00:00:00:01"));
        assert_eq!(s.config.devices.len(), 1);
        assert_eq!(s.config.devices[0].mac, "52:54:00:00:00:01");
        assert!(s.config.automations.contains_key("device:525400000001"));
        assert!(s.associate(second, "52:54:00:00:00:01"));
        assert!(s.profile_for(first).is_none());
        assert_eq!(s.profile_for(second).unwrap().id, "installer");
        assert!(!s.associate(second, "52:54:00:00:00:02"));
        s.save().unwrap();
        drop(s);
        drop(state);
        assert_eq!(
            Store::open(path.clone())
                .unwrap()
                .lock()
                .unwrap()
                .config
                .devices[0]
                .profile_id
                .as_deref(),
            Some("installer")
        );
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("events.jsonl"));
    }

    #[test]
    fn default_profile_survives_client_ip_change() {
        let path = std::env::temp_dir().join(format!(
            "bootah-default-ip-change-{}.json",
            uuid::Uuid::new_v4()
        ));
        let state = Store::open(path.clone()).unwrap();
        let mut s = state.lock().unwrap();
        let first = "192.0.2.10".parse().unwrap();
        let second = "192.0.2.11".parse().unwrap();
        s.config.profiles.push(serde_json::from_value(serde_json::json!({"id":"installer","name":"Installer","architecture":"x64","kind":"linux","root":"/tmp"})).unwrap());
        s.config.default_profiles.insert("x64-efi".into(), "installer".into());
        s.config.devices.push(Device {
            id: "525400000001".into(),
            mac: "52:54:00:00:00:01".into(),
            ip: Some(first),
            architecture: "x64-efi".into(),
            ..Default::default()
        });
        assert_eq!(s.profile_for(first).unwrap().id, "installer");
        assert!(s.associate(second, "52:54:00:00:00:01"));
        assert!(s.profile_for(first).is_none());
        assert_eq!(s.profile_for(second).unwrap().id, "installer");
        drop(s);
        drop(state);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("events.jsonl"));
    }
    #[test]
    fn reused_auto_ip_does_not_duplicate_known_mac() {
        let path = std::env::temp_dir().join(format!(
            "bootah-reused-auto-ip-{}.json",
            uuid::Uuid::new_v4()
        ));
        let state = Store::open(path.clone()).unwrap();
        let mut s = state.lock().unwrap();
        let reused = "192.0.2.124".parse().unwrap();
        s.config.devices.push(Device {
            id: "old".into(),
            mac: "c4:35:d9:82:32:23".into(),
            ip: Some(reused),
            auto_registered: true,
            ..Default::default()
        });
        s.config.devices.push(Device {
            id: "525400123465".into(),
            mac: "52:54:00:12:34:65".into(),
            ip: Some("192.0.2.123".parse().unwrap()),
            auto_registered: true,
            ..Default::default()
        });
        assert!(s.reconcile_ip_identity("52:54:00:12:34:65", reused));
        assert!(s.associate(reused, "52:54:00:12:34:65"));
        assert_eq!(s.config.devices.len(), 1);
        assert_eq!(s.config.devices[0].ip, Some(reused));
        drop(s);
        drop(state);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("events.jsonl"));
    }
}
