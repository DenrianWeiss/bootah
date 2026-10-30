use crate::model::{Settings, now};
use serde::{Deserialize, Serialize};
use std::net::Ipv4Addr;
mod platform;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Address {
    pub ip: Ipv4Addr,
    pub subnet: Ipv4Addr,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Interface {
    pub id: String,
    pub name: String,
    pub mac: String,
    pub up: bool,
    pub physical: bool,
    pub addresses: Vec<Address>,
    pub dhcp_servers: Vec<Ipv4Addr>,
    pub source: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Report {
    pub interfaces: Vec<Interface>,
    pub selected: String,
    #[serde(default)]
    pub show_interface_selector: bool,
    pub address: Option<Address>,
    pub status: String,
    pub source: String,
    pub servers: Vec<Ipv4Addr>,
    pub detail: String,
    pub checked_at: u64,
}
#[derive(Clone, Default, Deserialize)]
#[serde(default)]
pub struct Detection {
    pub authorize: bool,
    pub interface: Option<String>,
    pub address: Option<Ipv4Addr>,
    pub android: Option<Interface>,
}

pub fn usable(ip: Ipv4Addr) -> bool {
    !ip.is_unspecified()
        && !ip.is_loopback()
        && !ip.is_broadcast()
        && !ip.is_multicast()
        && !ip.is_link_local()
}

#[cfg(target_os = "macos")]
pub fn interface_mac(interface: &str) -> Option<String> {
    if interface.is_empty()
        || interface.starts_with('-')
        || !interface
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        return None;
    }
    let output = std::process::Command::new("/usr/sbin/networksetup")
        .args(["-getmacaddress", interface])
        .output()
        .ok()?;
    if !output.status.success() || output.stdout.len() > 4096 {
        return None;
    }
    let text = std::str::from_utf8(&output.stdout).ok()?;
    let address = text
        .strip_prefix("Ethernet Address: ")?
        .split_whitespace()
        .next()?;
    let address = crate::model::normalize_mac(address)?;
    if matches!(
        address.as_str(),
        "00:00:00:00:00:00" | "02:00:00:00:00:00" | "ff:ff:ff:ff:ff:ff"
    ) {
        return None;
    }
    Some(address)
}

#[cfg(any(target_os = "windows", test))]
pub fn windows_capture_id(interface: &str) -> String {
    if interface.starts_with(r"\\Device\NPF_") {
        interface[1..].into()
    } else {
        interface.into()
    }
}

pub fn select(settings: &Settings, interfaces: Vec<Interface>) -> Report {
    #[cfg(target_os = "windows")]
    let normalized = Settings {
        interface: windows_capture_id(&settings.interface),
        ..settings.clone()
    };
    #[cfg(target_os = "windows")]
    let settings = &normalized;
    let selectable = interfaces
        .iter()
        .filter(|i| !i.mac.is_empty() && ((i.up && i.physical) || i.id == settings.interface))
        .count();
    let missing =
        !settings.interface.is_empty() && !interfaces.iter().any(|i| i.id == settings.interface);
    let mut report = Report {
        show_interface_selector: selectable > 1 || missing,
        interfaces,
        checked_at: now(),
        ..Report::default()
    };
    let candidates: Vec<_> = report
        .interfaces
        .iter()
        .filter(|i| i.up && i.physical && !i.mac.is_empty())
        .collect();
    let chosen = if !settings.interface.is_empty() {
        report
            .interfaces
            .iter()
            .find(|i| i.id == settings.interface)
    } else {
        let matching: Vec<_> = candidates
            .iter()
            .copied()
            .filter(|i| i.addresses.iter().any(|a| a.ip == settings.server_ip))
            .collect();
        if matching.len() == 1 {
            Some(matching[0])
        } else if candidates.len() == 1 {
            Some(candidates[0])
        } else {
            None
        }
    };
    let Some(chosen) = chosen else {
        report.status = if !settings.interface.is_empty() {
            "interface_missing"
        } else if candidates.is_empty() {
            "no_interface"
        } else {
            "choose_interface"
        }
        .into();
        return report;
    };
    report.selected = chosen.id.clone();
    if !chosen.up {
        report.status = "disconnected".into();
        return report;
    }
    let addresses: Vec<_> = chosen.addresses.iter().filter(|a| usable(a.ip)).collect();
    report.address = addresses
        .iter()
        .find(|a| a.ip == settings.server_ip)
        .copied()
        .or_else(|| {
            if addresses.len() == 1 {
                Some(addresses[0])
            } else {
                None
            }
        })
        .cloned();
    if addresses.len() > 1 && report.address.is_none() {
        report.status = "choose_address".into();
        return report;
    }
    report.servers = chosen
        .dhcp_servers
        .iter()
        .copied()
        .filter(|ip| usable(*ip))
        .collect();
    report.source = chosen.source.clone();
    report.status = if report.servers.is_empty() {
        "unknown"
    } else {
        "lease_found"
    }
    .into();
    report
}

pub fn scan(settings: &Settings, request: &Detection) -> Result<Report, String> {
    #[cfg(not(target_os = "android"))]
    let interfaces = platform::interfaces()?;
    #[cfg(target_os = "android")]
    let interfaces = {
        let mut evidence = request.android.clone();
        if let Some(ref mut i) = evidence
            && i.mac.is_empty()
            && let Ok(native) = platform::interfaces()
            && let Some(found) = native.iter().find(|native| native.id == i.id)
        {
            i.mac = found.mac.clone();
        }
        evidence.into_iter().collect()
    };
    let mut report = select(settings, interfaces);
    #[cfg(target_os = "linux")]
    if report.status == "unknown" {
        if let Some(i) = report
            .interfaces
            .iter_mut()
            .find(|i| i.id == report.selected)
        {
            platform::linux_dhcp(i);
        }
        report = select(settings, report.interfaces);
    }
    if report.status != "unknown" {
        return Ok(report);
    }
    let selected = report
        .interfaces
        .iter()
        .find(|i| i.id == report.selected)
        .unwrap();
    match crate::raw::detect_dhcp(
        &selected.id,
        &selected.mac,
        report.address.as_ref().map(|a| a.ip),
        request.authorize,
    ) {
        Ok(servers) => {
            report.status = if servers.is_empty() {
                "no_response"
            } else {
                "response_found"
            }
            .into();
            report.servers = servers;
            report.source = "probe".into();
        }
        Err(e) => {
            report.status = "probe_unavailable".into();
            report.detail = e;
        }
    }
    Ok(report)
}

pub fn apply(settings: &mut Settings, report: &Report) {
    if !settings.dhcp_auto {
        return;
    }
    settings.dhcp_enabled = false;
    settings.proxy_dhcp_enabled =
        matches!(report.status.as_str(), "lease_found" | "response_found")
            && report.address.is_some();
    configure_interface(settings, report);
}

pub fn configure_interface(settings: &mut Settings, report: &Report) {
    if !report.selected.is_empty()
        && let Some(a) = report.address.as_ref()
    {
        settings.interface = report.selected.clone();
        let iface = report
            .interfaces
            .iter()
            .find(|i| i.id == report.selected)
            .unwrap();
        settings.server_mac = iface.mac.clone();
        settings.server_ip = a.ip;
        #[cfg(not(target_os = "android"))]
        {
            settings.bind_ip = a.ip;
            settings.ethernet_enabled = true;
        }
        settings.subnet = a.subnet;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn nic(id: &str, ip: &str) -> Interface {
        Interface {
            id: id.into(),
            up: true,
            physical: true,
            mac: "02:00:00:00:00:01".into(),
            addresses: vec![Address {
                ip: ip.parse().unwrap(),
                subnet: "255.255.255.0".parse().unwrap(),
            }],
            ..Interface::default()
        }
    }
    #[test]
    fn windows_capture_ids_accept_legacy_configuration() {
        let canonical = r"\Device\NPF_{01234567-89AB-CDEF-0123-456789ABCDEF}";
        assert_eq!(windows_capture_id(canonical), canonical);
        assert_eq!(windows_capture_id(&format!(r"\{canonical}")), canonical);
        assert_eq!(windows_capture_id("en0"), "en0");
    }
    #[test]
    fn selection_never_silently_switches_networks() {
        let mut s = Settings::default();
        let interfaces = vec![nic("wired", "192.0.2.2"), nic("wifi", "198.51.100.2")];
        assert_eq!(select(&s, interfaces.clone()).status, "choose_interface");
        assert!(select(&s, interfaces.clone()).show_interface_selector);
        let mut virtual_nic = nic("tunnel", "203.0.113.2");
        virtual_nic.physical = false;
        assert!(!select(&s, vec![interfaces[0].clone(), virtual_nic]).show_interface_selector);
        s.interface = "removed".into();
        assert_eq!(select(&s, interfaces.clone()).status, "interface_missing");
        assert!(select(&s, vec![interfaces[0].clone()]).show_interface_selector);
        s.interface = "wired".into();
        let mut disconnected = interfaces.clone();
        disconnected[0].up = false;
        assert_eq!(select(&s, disconnected).status, "disconnected");
        s.interface.clear();
        let mut one_connected = interfaces.clone();
        one_connected[1].up = false;
        assert!(!select(&s, one_connected).show_interface_selector);
        s.server_ip = "198.51.100.2".parse().unwrap();
        assert_eq!(select(&s, interfaces).selected, "wifi");
        let mut multi = nic("wired", "192.0.2.2");
        multi
            .addresses
            .push(nic("", "192.0.2.3").addresses.remove(0));
        assert_eq!(select(&s, vec![multi]).status, "choose_address");
    }
    #[test]
    fn unknown_never_allocates_and_manual_is_preserved() {
        let mut s = Settings {
            dhcp_enabled: true,
            ..Settings::default()
        };
        apply(&mut s, &Report::default());
        assert!(s.dhcp_enabled);
        s.dhcp_auto = true;
        apply(&mut s, &Report::default());
        assert!(!s.dhcp_enabled && !s.proxy_dhcp_enabled);
        let mut i = nic("wired", "192.0.2.2");
        i.dhcp_servers.push("192.0.2.1".parse().unwrap());
        let r = select(&s, vec![i]);
        apply(&mut s, &r);
        assert!(s.proxy_dhcp_enabled && !s.dhcp_enabled);
    }
}
