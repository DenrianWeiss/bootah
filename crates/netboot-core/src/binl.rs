use crate::{
    media_source::Image,
    model::{Profile, Shared},
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs, path::Path};

#[derive(Clone, Serialize, Deserialize)]
struct Driver {
    binary: String,
    service: String,
    characteristics: u32,
    bus_type: u32,
}

pub fn prepare(image: &Image, root: &Path, directory: &str) -> Result<usize, String> {
    if let Some(archive) = image.find(&format!("{directory}/driver.cab")) {
        image.materialize(archive, &root.join(".bootah-ris-driver.cab"))?;
    }
    let mut drivers = BTreeMap::<String, Driver>::new();
    for (path, entry) in image.entries() {
        if entry.is_dir
            || !path
                .to_ascii_lowercase()
                .starts_with(&format!("{directory}/net"))
        {
            continue;
        }
        let lower = path.to_ascii_lowercase();
        if !lower.ends_with(".in_") && !lower.ends_with(".inf") {
            continue;
        }
        let bytes = if lower.ends_with(".in_") {
            match crate::windows::read_cab(image, path, root) {
                Ok(bytes) => bytes,
                Err(_) => continue,
            }
        } else {
            match image.read(path, 8 * 1024 * 1024) {
                Ok(bytes) => bytes,
                Err(_) => continue,
            }
        };
        index_inf(&bytes, directory, &mut drivers);
    }
    fs::write(
        root.join(".bootah-ris-drivers.json"),
        serde_json::to_vec(&drivers).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    Ok(drivers.len())
}

fn index_inf(bytes: &[u8], directory: &str, drivers: &mut BTreeMap<String, Driver>) {
    let mut sections = BTreeMap::<String, Vec<(String, String)>>::new();
    let mut section = String::new();
    for raw in bytes.split(|b| *b == b'\n') {
        let line = String::from_utf8_lossy(raw);
        let line = line.split(';').next().unwrap_or("").trim();
        if line.starts_with('[') && line.ends_with(']') {
            section = line[1..line.len() - 1].to_ascii_lowercase();
        } else if let Some((key, value)) = line.split_once('=') {
            sections.entry(section.clone()).or_default().push((
                key.trim().to_ascii_lowercase(),
                value.trim().trim_matches('"').to_string(),
            ));
        }
    }
    for entries in sections.values() {
        for (_, value) in entries {
            let mut fields = value.split(',').map(str::trim);
            let Some(install) = fields.next() else {
                continue;
            };
            let Some(hardware) = fields.next() else {
                continue;
            };
            let hardware = hardware.to_ascii_uppercase();
            let Some(vendor) = hardware.split("VEN_").nth(1).and_then(|s| s.get(..4)) else {
                continue;
            };
            let Some(device) = hardware.split("DEV_").nth(1).and_then(|s| s.get(..4)) else {
                continue;
            };
            if !vendor
                .bytes()
                .chain(device.bytes())
                .all(|c| c.is_ascii_hexdigit())
            {
                continue;
            }
            let install = install.to_ascii_lowercase();
            let architecture = if directory == "amd64" {
                "ntamd64"
            } else {
                "ntx86"
            };
            let service_section = [
                format!("{install}.{architecture}.services"),
                format!("{install}.ntx86.services"),
                format!("{install}.nt.services"),
                format!("{install}.services"),
            ]
            .iter()
            .find_map(|name| sections.get(name));
            let Some((_, add_service)) = service_section
                .and_then(|entries| entries.iter().find(|(key, _)| key == "addservice"))
            else {
                continue;
            };
            let mut parts = add_service.split(',').map(str::trim);
            let Some(service) = parts.next().filter(|s| !s.is_empty()) else {
                continue;
            };
            let Some(binary_section) = parts.nth(1) else {
                continue;
            };
            let Some(binary) = sections
                .get(&binary_section.to_ascii_lowercase())
                .and_then(|entries| entries.iter().find(|(key, _)| key == "servicebinary"))
                .map(|(_, value)| {
                    value
                        .rsplit(['\\', '/'])
                        .next()
                        .unwrap_or("")
                        .trim_matches('"')
                        .to_string()
                })
                .filter(|s| {
                    !s.is_empty()
                        && s.bytes()
                            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'-'))
                })
            else {
                continue;
            };
            let install_section = [
                format!("{install}.{architecture}"),
                format!("{install}.ntx86"),
                format!("{install}.nt"),
                install.clone(),
            ]
            .iter()
            .find_map(|name| sections.get(name));
            let number = |key: &str, fallback: u32| {
                install_section
                    .and_then(|entries| entries.iter().find(|(name, _)| name == key))
                    .and_then(|(_, value)| {
                        let value = value.trim();
                        value
                            .strip_prefix("0x")
                            .and_then(|s| u32::from_str_radix(s, 16).ok())
                            .or_else(|| value.parse().ok())
                    })
                    .unwrap_or(fallback)
            };
            let key = format!("{vendor}:{device}");
            drivers.entry(key).or_insert(Driver {
                binary,
                service: service.to_string(),
                characteristics: number("characteristics", 132),
                bus_type: number("bustype", 5),
            });
        }
    }
}

pub fn reply(state: &Shared, ip: std::net::Ipv4Addr, request: &[u8]) -> Option<Vec<u8>> {
    if request.len() < 40 || &request[..4] != b"\x81NCQ" {
        return None;
    }
    let profile = state.lock().ok()?.profile_for(ip)?;
    if profile.kind != "windows-ris" {
        return None;
    }
    response(&profile, request)
}

fn response(profile: &Profile, request: &[u8]) -> Option<Vec<u8>> {
    let vendor = u16::from_le_bytes(request[36..38].try_into().ok()?);
    let device = u16::from_le_bytes(request[38..40].try_into().ok()?);
    let key = format!("{vendor:04X}:{device:04X}");
    let index = fs::read(profile.root.join(".bootah-ris-drivers.json")).ok()?;
    let drivers: BTreeMap<String, Driver> = serde_json::from_slice(&index).ok()?;
    let mut packet = b"\x82NCR".to_vec();
    let Some(driver) = drivers
        .get(&key)
        .filter(|driver| ensure_driver(profile, driver))
    else {
        packet.extend_from_slice(&12u32.to_le_bytes());
        packet.extend_from_slice(&0xc000000du32.to_le_bytes());
        return Some(packet);
    };
    packet.resize(36, 0);
    packet[12..16].copy_from_slice(&2u32.to_le_bytes());
    packet[16..20].copy_from_slice(&36u32.to_le_bytes());
    utf16(
        &mut packet,
        &format!("PCI\\VEN_{vendor:04X}&DEV_{device:04X}"),
    );
    let offset = packet.len() as u32;
    packet[20..24].copy_from_slice(&offset.to_le_bytes());
    utf16(&mut packet, &driver.binary);
    let offset = packet.len() as u32;
    packet[24..28].copy_from_slice(&offset.to_le_bytes());
    utf16(&mut packet, &driver.service);
    let offset = packet.len() as u32;
    packet[32..36].copy_from_slice(&offset.to_le_bytes());
    let params = format!(
        "Description\0{}\0Bootah network adapter\0Characteristics\01\0{}\0BusType\01\0{}\0",
        2, driver.characteristics, driver.bus_type
    );
    packet[28..32].copy_from_slice(&(params.len() as u32).to_le_bytes());
    packet.extend_from_slice(params.as_bytes());
    packet.extend_from_slice(&[0, 0]);
    let length = packet.len() as u32;
    packet[4..8].copy_from_slice(&length.to_le_bytes());
    Some(packet)
}

fn ensure_driver(profile: &Profile, driver: &Driver) -> bool {
    let directory = crate::windows::ris_install_dir(profile);
    let name = format!("{directory}/{}", driver.binary);
    if crate::media_source::exists(&profile.root, &name)
        || crate::media_source::exists(&profile.root, &format!("{}_", &name[..name.len() - 1]))
    {
        return true;
    }
    let archive = profile.root.join(".bootah-ris-driver.cab");
    if !archive.is_file() {
        return false;
    }
    let Ok(bytes) = crate::archive::cab_file(&archive, Some(&driver.binary), 8 * 1024 * 1024)
    else {
        return false;
    };
    let destination = profile.root.join(name);
    destination
        .parent()
        .is_some_and(|parent| fs::create_dir_all(parent).is_ok())
        && fs::write(destination, bytes).is_ok()
}

fn utf16(out: &mut Vec<u8>, value: &str) {
    for unit in value.encode_utf16() {
        out.extend_from_slice(&unit.to_le_bytes());
    }
    out.extend_from_slice(&[0, 0]);
}
