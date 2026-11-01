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
pub(crate) fn available_ip(store: &Store) -> Option<Ipv4Addr> {
    available_ip_excluding(store, &[])
}
pub(crate) fn available_ip_excluding(store: &Store, excluded: &[Ipv4Addr]) -> Option<Ipv4Addr> {
    let settings = &store.config.settings;
    let start = u32::from(settings.pool_start);
    let end = u32::from(settings.pool_end);
    if end < start || end - start > 65535 { return None; }
    (start..=end).map(Ipv4Addr::from).find(|ip| {
        usable(*ip) && *ip != settings.server_ip
            && !excluded.contains(ip)
            && !store.config.devices.iter().any(|d| d.ip == Some(*ip))
            && !store.boot_addresses.get(ip).is_some_and(|(_, until)| *until > now())
    })
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
    fn obp_stage2(&self) -> bool {
        if !self.pxe() || !self.architectures.is_empty() { return false; }
        let mut classes = self.option(77).unwrap_or_default();
        let mut found = false;
        while let Some((&size, rest)) = classes.split_first() {
            if size == 0 || rest.len() < usize::from(size) { return false; }
            let (value, tail) = rest.split_at(usize::from(size));
            found |= value == b"Bootah-OBP/1";
            classes = tail;
        }
        found
    }
    fn srm_stage2(&self) -> bool {
        if !self.pxe() || !self.architectures.contains(&3) { return false; }
        let mut classes = self.option(77).unwrap_or_default();
        let mut found = false;
        while let Some((&size, rest)) = classes.split_first() {
            if size == 0 || rest.len() < usize::from(size) { return false; }
            let (value, tail) = rest.split_at(usize::from(size));
            found |= value == b"Bootah-SRM";
            classes = tail;
        }
        found
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
    if let Some(ip) = claimed_ip.filter(|ip| *ip != settings.server_ip && !req.obp_stage2()) {
        if !store.reconcile_ip_identity(&req.mac, ip) {
            return None;
        }
    }
    if !req.obp_stage2() && (proxy
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
    if req.obp_stage2() {
        if !store.reconcile_architecture(idx, "sparc64", claimed_ip) { return None; }
        store.config.devices[idx].architecture = "sparc64".into();
    }
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
            if req.obp_stage2() {
                crate::sparc_obp::next_file(p).unwrap_or_default()
            } else if req.srm_stage2() {
                if crate::alpha_srm::enabled(p) { "bootah-srm.cfg".to_string() }
                else { String::new() }
            } else if (req.vendor.starts_with("NetBSD:i386:") || req.vendor.starts_with("NetBSD:amd64:")) && p.kind == "bsd" {
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
    let proxy_redirect = proxy && (req.ipxe || req.obp_stage2()) && !boot_service;
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
        if proxy && req.obp_stage2() {
            if store.boot_addresses.get(&current).is_some_and(|(mac, expiry)| mac != &req.mac && *expiry > now()) { return None; }
            let expiry = now() + 3600;
            store.boot_addresses.retain(|_, (mac, until)| mac != &req.mac && *until > now());
            store.boot_addresses.insert(current, (req.mac.clone(), expiry));
        } else if proxy {
            store.config.devices[idx].ip = Some(current);
        } else if store.config.devices[idx].ip != Some(current) {
            return None;
        }
        current
    } else if let Some(ip) = store.config.devices[idx].ip {
        ip
    } else {
        let ip = available_ip(&store)?;
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
        // Ethernet captures DHCP requests and uses the shared socket to reply. BINL retains its raw path.
        if raw_handles && (!boot_port || data[..size].starts_with(b"\x81NCQ")) {
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

#[cfg(test)]
mod srm_stage2_tests {
    use super::*;
    fn packet(message: u8, class: Option<&[u8]>, current: [u8; 4]) -> Vec<u8> {
        let mut b = vec![0; 240];
        b[..3].copy_from_slice(&[1, 1, 6]);
        b[12..16].copy_from_slice(&current);
        b[28..34].copy_from_slice(&[8, 0, 43, 229, 64, 0]);
        b[236..240].copy_from_slice(&[99, 130, 83, 99]);
        if message != 0 {
            add(&mut b, 53, &[message]);
            add(&mut b, 60, b"PXEClient:Arch:00003");
            add(&mut b, 93, &[0, 3]);
            if let Some(class) = class { add(&mut b, 77, class); }
        }
        b.push(255);
        b
    }
    fn filename(reply: Reply) -> String {
        String::from_utf8(reply.bytes[108..236].split(|b| *b == 0).next().unwrap().to_vec()).unwrap()
    }
    #[test]
    fn configuration_is_only_returned_to_identified_alpha_second_stage() {
        let root = std::env::temp_dir().join(format!("bootah-srm-dhcp-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let state = Store::open(root.join("state.json")).unwrap();
        {
            let mut s = state.lock().unwrap();
            s.config.settings.server_ip = "192.0.2.1".parse().unwrap();
            s.config.settings.dhcp_enabled = true;
            s.config.devices.push(Device {id:"08002be54000".into(), mac:"08:00:2b:e5:40:00".into(), ip:Some("192.0.2.100".parse().unwrap()), profile_id:Some("alpha".into()), ..Default::default()});
            s.config.profiles.push(serde_json::from_value(serde_json::json!({"id":"alpha","name":"Alpha","architecture":"alpha","kind":"linux","root":root,"boot_files":{"default":"bootah-srm.bin"}})).unwrap());
        }
        let first = packet(0, None, [0;4]);
        assert_eq!(filename(reply(&state, &Request::parse(&first).unwrap(), false).unwrap()), "bootah-srm.bin");
        {
            let mut s = state.lock().unwrap();
            s.config.settings.dhcp_enabled = false;
            s.config.settings.proxy_dhcp_enabled = true;
        }
        let second = packet(1, Some(b"\x0aBootah-SRM"), [0;4]);
        assert_eq!(filename(reply(&state, &Request::parse(&second).unwrap(), false).unwrap()), "bootah-srm.cfg");
        let discover = packet(3, Some(b"\x0aBootah-SRM"), [192,0,2,120]);
        assert_eq!(filename(reply(&state, &Request::parse(&discover).unwrap(), true).unwrap()), "bootah-srm.cfg");
        for class in [b"\x09Bootah-SRM".as_slice(), b"\x0aOtherClass".as_slice()] {
            let other = packet(1, Some(class), [0;4]);
            assert_eq!(filename(reply(&state, &Request::parse(&other).unwrap(), false).unwrap()), "bootah-srm.bin");
        }
        {
            let mut s = state.lock().unwrap();
            s.config.settings.dhcp_enabled = true;
            s.config.settings.proxy_dhcp_enabled = false;
        }
        assert_eq!(filename(reply(&state, &Request::parse(&first).unwrap(), false).unwrap()), "bootah-srm.bin");
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod obp_tests {
    use super::*;
    fn request(message: u8, stage: bool, ip: [u8; 4]) -> Vec<u8> {
        let mut b = vec![0; 240]; b[..3].copy_from_slice(&[1,1,6]);
        b[12..16].copy_from_slice(&ip); b[28..34].copy_from_slice(&[8,0,32,1,2,3]);
        b[236..240].copy_from_slice(&[99,130,83,99]);
        add(&mut b, 53, &[message]); add(&mut b, 60, b"PXEClient");
        if stage { add(&mut b, 77, b"\x0cBootah-OBP/1"); }
        b.push(255); b
    }
    #[tokio::test]
    async fn obp_boot_service_uses_udp_with_ethernet_enabled() {
        let root=std::env::temp_dir().join(format!("bootah-obp-udp-{}",uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join(".bootah-openbsd-nfs.json"),br#"{"version":1,"profile":"sparc"}"#).unwrap();
        let mut loader=vec![0;64];loader[..7].copy_from_slice(b"\x7fELF\x02\x02\x01");loader[19]=43;
        std::fs::write(root.join("ofwboot.net"),loader).unwrap();
        let state=Store::open(root.join("state.json")).unwrap();
        let server=UdpSocket::bind("127.0.0.1:0").await.unwrap();let target=server.local_addr().unwrap();
        {
            let mut s=state.lock().unwrap();
            s.config.settings.server_ip="127.0.0.2".parse().unwrap();
            s.config.settings.ethernet_enabled=true;s.config.settings.dhcp_enabled=false;s.config.settings.proxy_dhcp_enabled=true;
            s.config.settings.pxe_port=target.port();
            s.config.devices.push(Device{id:"080020010203".into(),mac:"08:00:20:01:02:03".into(),architecture:"sparc64".into(),profile_id:Some("sparc".into()),..Default::default()});
            s.config.profiles.push(serde_json::from_value(serde_json::json!({"id":"sparc","name":"SPARC","system_id":"openbsd","architecture":"sparc64","kind":"bsd","kernel":"bsd.rd","root":root,"boot_files":{"default":"bootah-obp.bin","sparc64_native":"ofwboot.net"}})).unwrap());
        }
        let client=UdpSocket::bind("127.0.0.1:0").await.unwrap();let source=client.local_addr().unwrap();
        let mut bytes=request(3,true,[127,0,0,1]);bytes.pop();add(&mut bytes,54,&[127,0,0,2]);add(&mut bytes,43,&[71,4,0,0,0,0,255]);bytes.push(255);
        let mut frame=vec![0;42];frame[..6].copy_from_slice(&[2,0,0,0,0,1]);frame[6..12].copy_from_slice(&[8,0,32,1,2,3]);frame[12..15].copy_from_slice(&[8,0,0x45]);frame[23]=17;
        frame[16..18].copy_from_slice(&((bytes.len()+28) as u16).to_be_bytes());frame[22]=64;
        frame[26..30].copy_from_slice(&[127,0,0,1]);frame[30..34].copy_from_slice(&[127,0,0,2]);
        frame[34..36].copy_from_slice(&source.port().to_be_bytes());frame[36..38].copy_from_slice(&target.port().to_be_bytes());frame[38..40].copy_from_slice(&((bytes.len()+8) as u16).to_be_bytes());
        let sum=frame[14..34].chunks_exact(2).map(|b|u32::from(u16::from_be_bytes([b[0],b[1]]))).sum::<u32>();let sum=(sum&65535)+(sum>>16);let sum=(sum&65535)+(sum>>16);frame[24..26].copy_from_slice(&(!(sum as u16)).to_be_bytes());frame.extend_from_slice(&bytes);
        let mut ethernet=crate::raw::Ethernet::new(state.clone(),[2,0,0,0,0,1],false);
        assert!(ethernet.handle(&frame).is_none());
        let sender=std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let sender_address=sender.local_addr().unwrap();sender.set_nonblocking(true).unwrap();
        ethernet.dhcp_socket=Some(sender);
        frame[36..38].copy_from_slice(&67u16.to_be_bytes());
        assert!(ethernet.handle(&frame).is_none());
        let mut native=[0;2048];
        let (n,from)=tokio::time::timeout(std::time::Duration::from_secs(1),client.recv_from(&mut native)).await.unwrap().unwrap();
        assert_eq!(from,sender_address);assert_eq!(native[0],2);
        assert_eq!(native[108..236].split(|b|*b==0).next().unwrap(),b"bootah-obp.cfg");
        assert_eq!(n,300);
        let task=tokio::spawn(run(state.clone(),server,true));
        client.send_to(&bytes,target).await.unwrap();
        let mut response=[0;2048];let (size,from)=tokio::time::timeout(std::time::Duration::from_secs(1),client.recv_from(&mut response)).await.unwrap().unwrap();
        assert_eq!(from,target);assert_eq!(response[0],2);assert_eq!(&response[16..20],&[127,0,0,1]);
        assert_eq!(response[108..236].split(|b|*b==0).next().unwrap(),b"bootah-obp.cfg");
        response[0]=1;let ack=Request::parse(&response[..size]).unwrap();assert_eq!(ack.message,5);assert_eq!(ack.server,Some("127.0.0.2".parse().unwrap()));
        task.abort();let _=task.await;std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn obp_stage_routes_grub_and_preserves_rarp_address() {
        let root = std::env::temp_dir().join(format!("bootah-obp-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let mut grub = vec![0u8; 64];
        for (at, word) in [(0, 0x01030107u32), (4, 32), (20, 0x4400), (32, 0x10800003), (36, 0x9010000c)] {
            grub[at..at+4].copy_from_slice(&word.to_be_bytes());
        }
        std::fs::write(root.join("grub.img"), grub).unwrap();
        let state = Store::open(root.join("state.json")).unwrap();
        {
            let mut s = state.lock().unwrap();
            s.config.settings.server_ip = "192.0.2.1".parse().unwrap();
            s.config.settings.dhcp_enabled = true;
            s.config.devices.push(Device { id:"080020010203".into(), mac:"08:00:20:01:02:03".into(), architecture:"sparc64".into(), ip:Some("192.0.2.100".parse().unwrap()), profile_id:Some("sparc".into()), ..Default::default() });
            s.config.profiles.push(serde_json::from_value(serde_json::json!({"id":"sparc","name":"SPARC","architecture":"sparc64","kind":"linux","root":root,"boot_files":{"default":"bootah-obp.bin","sparc64":"bootah-obp.bin","sparc64_grub":"grub.img"}})).unwrap());
        }
        let file = |r: Reply| String::from_utf8(r.bytes[108..236].split(|b| *b == 0).next().unwrap().to_vec()).unwrap();
        let first = request(1, false, [0;4]);
        assert_eq!(file(reply(&state, &Request::parse(&first).unwrap(), false).unwrap()), "bootah-obp.bin");
        let stage = request(1, true, [0;4]);
        assert!(Request::parse(&stage).unwrap().obp_stage2());
        let mut wrong_arch = stage[..stage.len()-1].to_vec();
        add(&mut wrong_arch, 93, &[0, 2]); wrong_arch.push(255);
        assert!(!Request::parse(&wrong_arch).unwrap().obp_stage2());
        let mut malformed = stage.clone();
        let class = malformed.windows(2).position(|v| v == [77, 13]).unwrap();
        malformed[class + 2] = 11;
        assert!(!Request::parse(&malformed).unwrap().obp_stage2());
        assert_eq!(file(reply(&state, &Request::parse(&stage).unwrap(), false).unwrap()), "grub.img");
        { let mut s = state.lock().unwrap(); s.config.settings.dhcp_enabled = false; s.config.settings.proxy_dhcp_enabled = true; }
        let offer = reply(&state, &Request::parse(&stage).unwrap(), false).unwrap();
        assert_eq!(&offer.bytes[16..20], &[0,0,0,0]);
        let boot = request(3, true, [192,0,2,120]);
        assert_eq!(file(reply(&state, &Request::parse(&boot).unwrap(), true).unwrap()), "grub.img");
        {
            let mut s = state.lock().unwrap();
            assert_eq!(s.config.devices[0].ip, Some("192.0.2.100".parse().unwrap()));
            assert_eq!(s.profile_for("192.0.2.120".parse().unwrap()).unwrap().id, "sparc");
            s.config.profiles[0].boot_files.insert("sparc64_grub".into(), "bootah-obp.bin".into());
        }
        assert!(reply(&state, &Request::parse(&stage).unwrap(), false).is_none());
        std::fs::remove_dir_all(root).unwrap();
    }
}
