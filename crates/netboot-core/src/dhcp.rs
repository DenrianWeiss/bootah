use crate::model::*;
use std::{
    collections::BTreeMap,
    net::{Ipv4Addr, SocketAddr},
};
use tokio::net::UdpSocket;

const BOOT_TYPE: u16 = 32768;

pub struct Request<'a> {
    pub bytes: &'a [u8],
    pub mac: String,
    pub architectures: Vec<u16>,
    pub vendor: String,
    pub hostname: String,
    pub uuid: String,
    pub message: u8,
    pub requested: Option<Ipv4Addr>,
    pub server: Option<Ipv4Addr>,
    pub ipxe: bool,
    options: BTreeMap<u8, Vec<u8>>,
    boot_item: Option<[u8; 4]>,
}
fn parse_options(data: &[u8], options: &mut BTreeMap<u8, Vec<u8>>) -> Option<()> {
    let mut i = 0;
    while i < data.len() {
        let code = data[i];
        i += 1;
        if code == 255 {
            break;
        }
        if code == 0 {
            continue;
        }
        let size = *data.get(i)? as usize;
        i += 1;
        options
            .entry(code)
            .or_default()
            .extend_from_slice(data.get(i..i + size)?);
        i += size;
    }
    Some(())
}
fn address(bytes: &[u8]) -> Ipv4Addr {
    Ipv4Addr::new(bytes[0], bytes[1], bytes[2], bytes[3])
}
fn usable(ip: Ipv4Addr) -> bool {
    !ip.is_unspecified() && !ip.is_broadcast() && !ip.is_multicast() && ip.octets()[0] != 0
}
impl<'a> Request<'a> {
    pub fn option(&self, code: u8) -> Option<&[u8]> {
        self.options.get(&code).map(Vec::as_slice)
    }
    pub fn current(&self) -> Ipv4Addr {
        address(&self.bytes[12..16])
    }
    pub fn relay(&self) -> Ipv4Addr {
        address(&self.bytes[24..28])
    }
    fn pxe(&self) -> bool {
        self.vendor.starts_with("PXEClient")
    }
    pub fn parse(bytes: &'a [u8]) -> Option<Self> {
        if bytes.len() < 236 || bytes[0] != 1 || bytes[1] != 1 || bytes[2] != 6 {
            return None;
        }
        let mut options = BTreeMap::new();
        if bytes.get(236..240) == Some(&[99, 130, 83, 99]) {
            parse_options(&bytes[240..], &mut options)?;
            let overload = options.get(&52).map(Vec::as_slice);
            if overload.is_some_and(|b| b.len() != 1 || !(1..=3).contains(&b[0])) {
                return None;
            }
            let overload = overload.map(|b| b[0]).unwrap_or(0);
            if overload & 1 != 0 {
                parse_options(&bytes[108..236], &mut options)?;
            }
            if overload & 2 != 0 {
                parse_options(&bytes[44..108], &mut options)?;
            }
        }
        for (code, len) in [(53, 1), (50, 4), (54, 4), (94, 3), (97, 17), (57, 2)] {
            if options.get(&code).is_some_and(|b| b.len() != len) {
                return None;
            }
        }
        let architectures = if let Some(b) = options.get(&93) {
            if b.is_empty() || b.len() % 2 != 0 {
                return None;
            }
            b.chunks_exact(2)
                .map(|b| u16::from_be_bytes([b[0], b[1]]))
                .collect()
        } else {
            Vec::new()
        };
        let text = |code| {
            options
                .get(&code)
                .map(|b| {
                    String::from_utf8_lossy(b)
                        .chars()
                        .filter(|c| !c.is_control())
                        .take(200)
                        .collect::<String>()
                })
                .unwrap_or_default()
        };
        let vendor = text(60);
        let mut vendor_options = BTreeMap::new();
        if vendor.starts_with("PXEClient")
            && let Some(data) = options.get(&43)
        {
            parse_options(data, &mut vendor_options)?;
        }
        let boot_item = vendor_options
            .get(&71)
            .map(|b| b.as_slice().try_into())
            .transpose()
            .ok()?;
        Some(Self {
            bytes,
            mac: mac(&bytes[28..34]),
            architectures,
            vendor,
            hostname: text(12),
            uuid: options.get(&97).map(|b| mac(b)).unwrap_or_default(),
            message: options.get(&53).map(|b| b[0]).unwrap_or(0),
            requested: options.get(&50).map(|b| address(b)),
            server: options.get(&54).map(|b| address(b)),
            ipxe: text(77).contains("iPXE") || options.contains_key(&175),
            options,
            boot_item,
        })
    }
}
fn add(out: &mut Vec<u8>, tag: u8, value: &[u8]) {
    for part in value.chunks(255) {
        out.extend([tag, part.len() as u8]);
        out.extend_from_slice(part);
    }
}

pub struct Reply {
    pub bytes: Vec<u8>,
    pub boot_service: bool,
    pub nak: bool,
}
impl Reply {
    pub fn destination(
        &self,
        req: &Request<'_>,
        peer: SocketAddr,
        nonstandard: bool,
    ) -> SocketAddr {
        if !req.relay().is_unspecified() {
            return (req.relay(), 67).into();
        }
        if nonstandard {
            return peer;
        }
        if self.boot_service {
            return (req.current(), peer.port()).into();
        }
        if self.nak || req.bytes[10] & 0x80 != 0 || req.current().is_unspecified() {
            (Ipv4Addr::BROADCAST, 68).into()
        } else {
            (req.current(), 68).into()
        }
    }
}

pub fn reply(state: &Shared, req: &Request<'_>, boot_port: bool) -> Option<Reply> {
    let mut store = state.lock().unwrap();
    let settings = store.config.settings.clone();
    let proxy = settings.proxy_dhcp_enabled;
    if !settings.dhcp_enabled && !proxy {
        return None;
    }
    let boot_service =
        boot_port || (req.pxe() && req.boot_item.is_some() && matches!(req.message, 3 | 8));
    if req.server.is_some_and(|s| s != settings.server_ip) {
        return None;
    }
    if boot_service {
        if !req.pxe() || !matches!(req.message, 3 | 8) || !usable(req.current()) {
            return None;
        }
        if let Some(item) = req.boot_item {
            let kind = u16::from_be_bytes([item[0], item[1]]);
            if !matches!(kind, 0 | BOOT_TYPE) || item[2..] != [0, 0] {
                return None;
            }
        }
    } else if proxy {
        if req.message != 1 || !req.pxe() {
            return None;
        }
    } else if !matches!(req.message, 0 | 1 | 3 | 8) {
        return None;
    }
    let current = req.current();
    let claimed_ip = if usable(current) {
        Some(current)
    } else if req.message == 3 {
        req.requested.filter(|ip| usable(*ip))
    } else {
        None
    };
    if let Some(ip) = claimed_ip.filter(|ip| *ip != settings.server_ip) {
        if !store.reconcile_ip_identity(&req.mac, ip) {
            return None;
        }
    }
    if (proxy
        || store
            .config
            .devices
            .iter()
            .any(|d| d.ip == Some(current) && d.mac.is_empty()))
        && usable(current)
        && current != settings.server_ip
    {
        store.associate(current, &req.mac);
    }
    let idx = store.observe_mac(
        &req.mac,
        if boot_service {
            "PXE"
        } else if proxy {
            "ProxyDHCP"
        } else if req.message == 0 {
            "BOOTP"
        } else {
            "DHCP"
        },
    )?;
    let d = &mut store.config.devices[idx];
    d.vendor = req.vendor.clone();
    d.uuid = req.uuid.clone();
    if d.name.is_empty() {
        d.name = req.hostname.clone();
    }
    let explicit = store
        .config
        .profiles
        .iter()
        .find(|p| Some(&p.id) == store.config.devices[idx].profile_id.as_ref())
        .cloned();
    let selected = req
        .architectures
        .iter()
        .copied()
        .find(|code| {
            let key = architecture(*code);
            explicit
                .as_ref()
                .is_some_and(|p| p.boot_files.contains_key(key))
                || (explicit.is_none()
                    && store.config.default_profiles.get(key).is_some_and(|id| {
                        store
                            .config
                            .profiles
                            .iter()
                            .any(|p| &p.id == id && p.boot_files.contains_key(key))
                    }))
        })
        .or_else(|| req.architectures.first().copied());
    if let Some(code) = selected {
        let detected = architecture(code);
        if !store.reconcile_architecture(idx, detected, claimed_ip) {
            return None;
        }
        store.config.devices[idx].architecture = detected.into();
    }
    let explicit = if store.config.devices[idx].profile_id.is_some() {
        explicit
    } else {
        None
    };
    let profile = explicit.or_else(|| {
        let key = &store.config.devices[idx].architecture;
        let id = store.config.default_profiles.get(key)?;
        store.config.profiles.iter().find(|p| &p.id == id).cloned()
    });
    if profile.is_none() {
        let _ = store.save();
        return None;
    }
    let firmware = selected.map(architecture).unwrap_or(&store.config.devices[idx].architecture).to_owned();
    let file = profile
        .as_ref()
        .map(|p| {
            if (req.vendor.starts_with("NetBSD:i386:") || req.vendor.starts_with("NetBSD:amd64:")) && p.kind == "bsd" {
                let raw = req.option(67).unwrap_or(&req.bytes[108..236]);
                let requested =
                    std::str::from_utf8(raw.split(|b| *b == 0).next().unwrap_or_default())
                        .unwrap_or("");
                let name = if requested.is_empty() {
                    "netbsd"
                } else {
                    requested
                };
                if crate::media_source::exists(&p.root, name) {
                    format!("tftp:{name}")
                } else {
                    String::new()
                }
            } else if req.ipxe && crate::native_uefi::enabled(p) {
                String::new()
            } else if req.ipxe && settings.boot_transport == "tftp" {
                format!("tftp://{}/boot.ipxe", settings.server_ip)
            } else if req.ipxe {
                format!(
                    "http://{}:{}/boot/{}/boot.ipxe",
                    settings.server_ip, settings.http_port, p.id
                )
            } else if req.vendor.starts_with("HTTPClient") {
                p.boot_files
                    .get(&firmware)
                    .map(|f| {
                        format!(
                            "http://{}:{}/boot/{}/{}",
                            settings.server_ip, settings.http_port, p.id, f
                        )
                    })
                    .unwrap_or_default()
            } else {
                crate::boot::pxe_loader(p, &firmware).to_owned()
            }
        })
        .unwrap_or_default();
    if file.len() > 127 && !req.vendor.starts_with("HTTPClient") {
        store.event(&req.mac, "DHCP", "Boot filename exceeds BOOTP field");
        return None;
    }
    if (req.pxe() || req.vendor.starts_with("HTTPClient")) && file.is_empty() {
        store.event(&req.mac, "PXE", "No boot file for client architectures");
        return None;
    }
    // iPXE EFI need not implement the legacy PXE menu API. Ask it to query
    // ProxyDHCP on 4011 before supplying its script (PXE 2.1 section 2.2.3).
    let proxy_redirect = proxy && req.ipxe && !boot_service;
    let current = req.current();
    let ip = if proxy && !boot_service {
        Ipv4Addr::UNSPECIFIED
    } else if boot_service || req.message == 8 {
        if !usable(current) || current == settings.server_ip {
            return None;
        }
        if store
            .config
            .devices
            .iter()
            .enumerate()
            .any(|(i, d)| i != idx && d.ip == Some(current))
        {
            return None;
        }
        if proxy {
            store.config.devices[idx].ip = Some(current);
        } else if store.config.devices[idx].ip != Some(current) {
            return None;
        }
        current
    } else if let Some(ip) = store.config.devices[idx].ip {
        ip
    } else {
        let start = u32::from(settings.pool_start);
        let end = u32::from(settings.pool_end);
        if end < start || end - start > 65535 {
            return None;
        }
        let ip = (start..=end).map(Ipv4Addr::from).find(|ip| {
            usable(*ip)
                && *ip != settings.server_ip
                && !store.config.devices.iter().any(|d| d.ip == Some(*ip))
        })?;
        store.config.devices[idx].ip = Some(ip);
        ip
    };
    let requested = req
        .requested
        .or_else(|| (!current.is_unspecified()).then_some(current));
    let mismatch =
        !boot_service && !proxy && req.message == 3 && requested.is_some_and(|r| r != ip);
    let mut out = vec![0u8; 240];
    out[..236].copy_from_slice(&req.bytes[..236]);
    out[0] = 2;
    out[3] = 0;
    out[8..10].fill(0);
    out[12..24].fill(0);
    out[44..236].fill(0);
    out[236..240].copy_from_slice(&[99, 130, 83, 99]);
    if req.message != 0 {
        add(
            &mut out,
            53,
            &[if mismatch {
                6
            } else if req.message == 1 {
                2
            } else {
                5
            }],
        );
        add(&mut out, 54, &settings.server_ip.octets());
        if let Some(client) = req.option(61) {
            add(&mut out, 61, client);
        }
    }
    if !mismatch {
        if !boot_service && req.message != 1 {
            out[12..16].copy_from_slice(&current.octets());
        }
        if req.message != 8 || boot_service {
            out[16..20].copy_from_slice(&ip.octets());
        }
        out[20..24].copy_from_slice(&settings.server_ip.octets());
        if file.len() <= 127 && !proxy_redirect {
            out[108..108 + file.len()].copy_from_slice(file.as_bytes());
        }
        if !proxy && !boot_service {
            if matches!(req.message, 1 | 3) {
                add(&mut out, 51, &3600u32.to_be_bytes());
            }
            if let Some(p) = &profile
                && crate::freebsd::network_root(p)
            {
                add(&mut out, 17, format!("{}:/{}", settings.server_ip, p.id).as_bytes());
            }
            if let Some(p) = &profile
                && p.kind == "bsd"
                && matches!(
                    p.architecture.as_str(),
                    "alpha" | "sparc64" | "powerpc-of" | "powerpc" | "ppc"
                )
            {
                add(&mut out, 17, format!("/{}", p.id).as_bytes());
                add(&mut out, 16, &settings.server_ip.octets());
            }
            add(&mut out, 1, &settings.subnet.octets());
            if let Some(router) = settings.router {
                add(&mut out, 3, &router.octets());
            }
        }
        if req.pxe() {
            if let Some(code) = selected {
                add(&mut out, 93, &code.to_be_bytes());
            }
            for code in [94, 97] {
                if let Some(value) = req.option(code) {
                    add(&mut out, code, value);
                }
            }
            add(&mut out, 60, b"PXEClient");
            let mut vendor = Vec::new();
            if boot_service {
                add(&mut vendor, 71, &req.boot_item.unwrap_or([0, 0, 0, 0]));
            } else {
                // PXE 2.1: unicast-only discovery; iPXE may directly consume its script.
                add(&mut vendor, 6, &[if req.ipxe && !proxy { 15 } else { 7 }]);
                let mut servers = BOOT_TYPE.to_be_bytes().to_vec();
                servers.push(1);
                servers.extend(settings.server_ip.octets());
                add(&mut vendor, 8, &servers);
                let mut menu = BOOT_TYPE.to_be_bytes().to_vec();
                menu.extend([6]);
                menu.extend(b"Bootah");
                add(&mut vendor, 9, &menu);
                add(&mut vendor, 10, &[0]);
            }
            if !proxy_redirect {
                vendor.push(255);
                add(&mut out, 43, &vendor);
            }
        }
        if req.vendor.starts_with("HTTPClient") {
            add(&mut out, 60, b"HTTPClient");
            add(&mut out, 67, file.as_bytes());
        }
    } else if !req.relay().is_unspecified() {
        out[10] |= 0x80;
    }
    out.push(255);
    out.resize(out.len().max(300), 0);
    let label = if mismatch {
        "NAK"
    } else if boot_service {
        "Boot ACK"
    } else if proxy {
        "Proxy Offer"
    } else if req.message == 1 {
        "Offer"
    } else {
        "Reply"
    };
    store.event(
        &req.mac,
        if boot_service { "PXE" } else { "DHCP" },
        format!("{label} {ip} {file}"),
    );
    if let Err(e) = store.save() {
        store.event(&req.mac, "storage", e);
    }
    Some(Reply {
        bytes: out,
        boot_service,
        nak: mismatch,
    })
}
pub async fn run(state: Shared, socket: UdpSocket, boot_port: bool) -> Result<(), String> {
    let raw_handles = state.lock().unwrap().config.settings.ethernet_enabled;
    let nonstandard = !boot_port && socket.local_addr().map_err(|e| e.to_string())?.port() != 67;
    let mut data = [0; 4096];
    loop {
        let (size, peer) = socket
            .recv_from(&mut data)
            .await
            .map_err(|e| e.to_string())?;
        // Keep DHCP/PXE ports bound in raw mode so the host does not send ICMP port-unreachable.
        if raw_handles {
            continue;
        }
        if boot_port
            && socket
                .local_addr()
                .is_ok_and(|address| address.port() == 4011)
            && let std::net::IpAddr::V4(ip) = peer.ip()
            && let Some(response) = crate::binl::reply(&state, ip, &data[..size])
        {
            let _ = socket.send_to(&response, peer).await;
            continue;
        }
        let Some(req) = Request::parse(&data[..size]) else {
            continue;
        };
        if boot_port && req.relay().is_unspecified() && peer.ip() != req.current() {
            continue;
        }
        let Some(response) = reply(&state, &req, boot_port) else {
            continue;
        };
        let destination = response.destination(&req, peer, nonstandard);
        if let Err(e) = socket.send_to(&response.bytes, destination).await {
            state.lock().unwrap().event(&req.mac, "DHCP", e.to_string());
        }
    }
}

#[cfg(test)]
mod association_tests {
    use super::*;
    #[test]
    fn proxy_address_changes_preserve_assignment_and_authoritative_dhcp_keeps_reservation() {
        let root =
            std::env::temp_dir().join(format!("bootah-dhcp-association-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let state = Store::open(root.join("state.json")).unwrap();
        {
            let mut s = state.lock().unwrap();
            s.config.settings.server_ip = "192.0.2.1".parse().unwrap();
            s.config.settings.dhcp_enabled = true;
            s.config.settings.boot_transport = "tftp".into();
            s.config.devices.push(crate::model::Device {
                id: "525400000001".into(),
                mac: "52:54:00:00:00:01".into(),
                ip: Some("192.0.2.10".parse().unwrap()),
                profile_id: Some("installer".into()),
                ..Default::default()
            });
            s.config.profiles.push(serde_json::from_value(serde_json::json!({"id":"installer","name":"Installer","architecture":"x64","kind":"raw","root":root,"boot_files":{"x64-efi":"kernel"}})).unwrap());
        }
        let mut bytes = vec![0; 240];
        bytes[..4].copy_from_slice(&[1, 1, 6, 0]);
        bytes[12..16].copy_from_slice(&[192, 0, 2, 11]);
        bytes[28..34].copy_from_slice(&[0x52, 0x54, 0, 0, 0, 1]);
        bytes[236..240].copy_from_slice(&[99, 130, 83, 99]);
        bytes.extend_from_slice(&[53, 1, 8, 93, 2, 0, 7, 60, 9]);
        bytes.extend_from_slice(b"PXEClient");
        bytes.push(255);
        let request = Request::parse(&bytes).unwrap();
        assert!(reply(&state, &request, true).is_none());
        assert_eq!(
            state.lock().unwrap().config.devices[0].ip,
            Some("192.0.2.10".parse().unwrap())
        );
        {
            let mut s = state.lock().unwrap();
            s.config.settings.dhcp_enabled = false;
            s.config.settings.proxy_dhcp_enabled = true;
        }
        assert!(reply(&state, &request, true).is_some());
        let s = state.lock().unwrap();
        assert_eq!(s.config.devices.len(), 1);
        assert_eq!(s.config.devices[0].ip, Some("192.0.2.11".parse().unwrap()));
        assert_eq!(s.config.devices[0].profile_id.as_deref(), Some("installer"));
        drop(s);
        std::fs::remove_dir_all(root).unwrap();
    }
}
