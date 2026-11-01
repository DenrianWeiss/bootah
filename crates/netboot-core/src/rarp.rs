use super::{datagram, udp};
use crate::{dhcp, model::*};
use std::{
    collections::HashMap,
    net::Ipv4Addr,
    time::{Duration, Instant},
};

#[derive(Clone)]
struct Lease {
    ip: Ipv4Addr,
    server: Ipv4Addr,
    obtained: Instant,
    seconds: u32,
    t1: u32,
    expires: u64,
}
#[derive(PartialEq)]
enum Phase {
    Discover,
    Request,
    Probe,
    Bound,
    Confirm,
    Backoff,
}
struct Client {
    mac: [u8; 6],
    request: Vec<u8>,
    xid: [u8; 4],
    phase: Phase,
    started: Instant,
    sent: Instant,
    tries: u32,
    select_secs: u16,
    offer: Option<(Ipv4Addr, Ipv4Addr)>,
    lease: Option<Lease>,
}
#[derive(Default)]
pub(super) struct Server {
    clients: HashMap<String, Client>,
    declined: HashMap<Ipv4Addr, Instant>,
}
fn usable(ip: Ipv4Addr) -> bool {
    crate::network::usable(ip) && !ip.is_loopback() && !ip.is_link_local()
}
fn xid() -> [u8; 4] {
    uuid::Uuid::new_v4().as_bytes()[..4].try_into().unwrap()
}
fn opt32(req: &dhcp::Request<'_>, tag: u8) -> Option<u32> {
    Some(u32::from_be_bytes(req.option(tag)?.try_into().ok()?))
}
fn valid_datagram(frame: &[u8]) -> bool {
    if frame.len() < 42 || frame[12..14] != [8, 0] {
        return false;
    }
    let ip = &frame[14..];
    let ihl = usize::from(ip[0] & 15) * 4;
    let total = usize::from(u16::from_be_bytes([ip[2], ip[3]]));
    if ihl < 20 || total < ihl + 8 || total > ip.len() {
        return false;
    }
    let sum = |bytes: &[u8]| {
        bytes
            .chunks(2)
            .map(|b| u32::from(b[0]) * 256 + u32::from(*b.get(1).unwrap_or(&0)))
            .sum::<u32>()
    };
    let valid = |mut n: u32| {
        while n > 65535 {
            n = (n & 65535) + (n >> 16);
        }
        n == 65535
    };
    if !valid(sum(&ip[..ihl])) {
        return false;
    }
    let udp = &ip[ihl..total];
    let size = usize::from(u16::from_be_bytes([udp[4], udp[5]]));
    size >= 8
        && size <= udp.len()
        && (udp[6..8] == [0, 0] || valid(sum(&ip[12..20]) + 17 + size as u32 + sum(&udp[..size])))
}
fn packet(c: &Client, kind: u8, renewing: bool) -> Vec<u8> {
    let mut p = vec![0; 240];
    p[..3].copy_from_slice(&[1, 1, 6]);
    p[4..8].copy_from_slice(&c.xid);
    let secs = if kind == 4 {
        0
    } else if c.phase == Phase::Request {
        c.select_secs
    } else {
        c.started.elapsed().as_secs().min(65535) as u16
    };
    p[8..10].copy_from_slice(&secs.to_be_bytes());
    p[10] = 0x80;
    p[28..34].copy_from_slice(&c.mac);
    p[236..240].copy_from_slice(&[99, 130, 83, 99]);
    p.extend_from_slice(&[53, 1, kind, 61, 7, 1]);
    p.extend_from_slice(&c.mac);
    if kind != 4 {
        p.extend_from_slice(&[55, 6, 1, 3, 51, 54, 58, 59]);
    }
    if renewing {
        p.extend_from_slice(&[50, 4]);
        p.extend_from_slice(&c.lease.as_ref().unwrap().ip.octets());
    } else if let Some((ip, server)) = c.offer {
        p.extend_from_slice(&[50, 4]);
        p.extend_from_slice(&ip.octets());
        p.extend_from_slice(&[54, 4]);
        p.extend_from_slice(&server.octets());
    }
    p.push(255);
    p.resize(300, 0);
    p
}
fn dhcp_frame(c: &Client, host: [u8; 6], kind: u8, renewing: bool) -> Vec<u8> {
    datagram(
        host,
        [255; 6],
        Ipv4Addr::UNSPECIFIED,
        Ipv4Addr::BROADCAST,
        68,
        67,
        &packet(c, kind, renewing),
    )
}
fn probe(c: &Client, host: [u8; 6]) -> Vec<u8> {
    let mut f = vec![0; 60];
    f[..6].fill(255);
    f[6..12].copy_from_slice(&host);
    f[12..22].copy_from_slice(&[8, 6, 0, 1, 8, 0, 6, 4, 0, 1]);
    f[22..28].copy_from_slice(&host);
    f[38..42].copy_from_slice(&c.lease.as_ref().unwrap().ip.octets());
    f
}
pub(super) fn answer(request: &[u8], host: [u8; 6], server: Ipv4Addr, ip: Ipv4Addr) -> Vec<u8> {
    let mut out = request[..42].to_vec();
    out[..6].copy_from_slice(&request[6..12]);
    out[6..12].copy_from_slice(&host);
    out[20..22].copy_from_slice(&4u16.to_be_bytes());
    out[22..28].copy_from_slice(&host);
    out[28..32].copy_from_slice(&server.octets());
    out[38..42].copy_from_slice(&ip.octets());
    out.resize(60, 0);
    out
}
impl Server {
    pub(super) fn request(
        &mut self,
        state: &Shared,
        host: [u8; 6],
        frame: &[u8],
    ) -> Option<Vec<u8>> {
        let mac = crate::model::mac(&frame[32..38]);
        let mut store = state.lock().unwrap();
        let device = store.config.devices.iter().find(|d| d.mac == mac)?;
        if let Some(ip) = device.ip {
            store.event(&mac, "RARP", ip.to_string());
            return Some(answer(frame, host, store.config.settings.server_ip, ip));
        }
        let profile = device
            .profile_id
            .as_ref()
            .or_else(|| store.config.default_profiles.get(&device.architecture));
        if !store.config.profiles.iter().any(|p| Some(&p.id) == profile) {
            return None;
        }
        let server = store.config.settings.server_ip;
        let excluded: Vec<_> = self
            .declined
            .iter()
            .filter(|(_, t)| t.elapsed() < Duration::from_secs(600))
            .map(|(ip, _)| *ip)
            .collect();
        let local =
            if store.config.settings.dhcp_enabled && !store.config.settings.proxy_dhcp_enabled {
                dhcp::available_ip_excluding(&store, &excluded)
            } else {
                None
            };
        let local_mode =
            store.config.settings.dhcp_enabled && !store.config.settings.proxy_dhcp_enabled;
        drop(store);
        if let Some(c) = self.clients.get_mut(&mac) {
            c.request = frame[..42].to_vec();
            if c.phase != Phase::Probe
                && let Some(l) = &c.lease
                && l.obtained.elapsed().as_secs() < u64::from(l.seconds)
            {
                Self::publish(state, &mac, c);
                let l = c.lease.as_ref().unwrap();
                return Some(answer(frame, host, server, l.ip));
            }
            if c.phase != Phase::Backoff || c.started.elapsed() < Duration::from_secs(30) {
                return None;
            }
            self.clients.remove(&mac);
        }
        if self.clients.len() >= 128 {
            return None;
        }
        let now = Instant::now();
        let mut c = Client {
            mac: frame[32..38].try_into().ok()?,
            request: frame[..42].to_vec(),
            xid: xid(),
            phase: Phase::Discover,
            started: now,
            sent: now,
            tries: 1,
            select_secs: 0,
            offer: None,
            lease: None,
        };
        if local_mode {
            let ip = local?;
            c.offer = Some((ip, server));
            c.phase = Phase::Probe;
            c.lease = Some(Lease {
                ip,
                server,
                obtained: now,
                seconds: 3600,
                t1: 1800,
                expires: crate::model::now() + 3600,
            });
            Self::publish(state, &mac, &c);
            let frame = probe(&c, host);
            self.clients.insert(mac.clone(), c);
            return Some(frame);
        }
        let frame = dhcp_frame(&c, host, 1, false);
        self.clients.insert(mac.clone(), c);
        state
            .lock()
            .unwrap()
            .event(&mac, "RARP", "Requesting a DHCP lease for RARP client");
        Some(frame)
    }
    pub(super) fn receive(
        &mut self,
        state: &Shared,
        host: [u8; 6],
        frame: &[u8],
    ) -> Option<Vec<u8>> {
        if frame.len() >= 42
            && frame[12..20] == [8, 6, 0, 1, 8, 0, 6, 4]
            && matches!(frame[21], 1 | 2)
            && frame[20] == 0
        {
            for (mac, c) in &mut self.clients {
                if c.phase != Phase::Probe {
                    continue;
                }
                let ip = c.lease.as_ref()?.ip;
                if (frame[28..32] == ip.octets()
                    || frame[28..32] == [0; 4] && frame[38..42] == ip.octets())
                    && frame[22..28] != host
                    && frame[22..28] != c.mac
                {
                    let decline = dhcp_frame(c, host, 4, false);
                    Self::forget(state, mac, c);
                    self.declined.insert(ip, Instant::now());
                    c.phase = Phase::Backoff;
                    c.started = Instant::now();
                    c.lease = None;
                    state.lock().unwrap().event(
                        mac,
                        "RARP",
                        "DHCP address conflict; lease declined",
                    );
                    return Some(decline);
                }
            }
            return None;
        }
        if !valid_datagram(frame) {
            return None;
        }
        let (_, source, destination, bytes) = udp(frame)?;
        if source != 67 || destination != 68 || bytes.len() < 240 || bytes[0] != 2 {
            return None;
        }
        let mut parsed = bytes.to_vec();
        parsed[0] = 1;
        let reply = dhcp::Request::parse(&parsed)?;
        let c = self.clients.get_mut(&reply.mac)?;
        if bytes[4..8] != c.xid || bytes[28..34] != c.mac {
            return None;
        }
        let server = reply.server.filter(|ip| usable(*ip))?;
        let address = Ipv4Addr::new(bytes[16], bytes[17], bytes[18], bytes[19]);
        if c.phase == Phase::Discover && reply.message == 2 && usable(address) {
            c.select_secs = c
                .sent
                .saturating_duration_since(c.started)
                .as_secs()
                .min(65535) as u16;
            c.offer = Some((address, server));
            c.phase = Phase::Request;
            c.started = Instant::now();
            c.sent = c.started;
            c.tries = 1;
            return Some(dhcp_frame(c, host, 3, false));
        }
        let renewing = c.phase == Phase::Confirm;
        if c.phase != Phase::Request && !renewing {
            return None;
        }
        let (_, selected) = c.offer?;
        if server != selected && !renewing {
            return None;
        }
        if reply.message == 6 {
            Self::forget(state, &reply.mac, c);
            c.phase = Phase::Backoff;
            c.started = Instant::now();
            c.lease = None;
            state.lock().unwrap().event(
                &reply.mac,
                "RARP",
                "DHCP rejected the RARP lease; retry later",
            );
            return None;
        }
        if reply.message != 5 || !usable(address) || address != c.offer?.0 {
            return None;
        }
        let seconds = opt32(&reply, 51)?;
        let mask = opt32(&reply, 1)?;
        if seconds < 4 || mask == 0 || ((!mask).wrapping_add(1) & !mask) != 0 {
            return None;
        }
        let store = state.lock().unwrap();
        if address == store.config.settings.server_ip
            || u32::from(address) & mask != u32::from(store.config.settings.server_ip) & mask
            || store
                .config
                .devices
                .iter()
                .any(|d| d.mac != reply.mac && d.ip == Some(address))
            || store
                .boot_addresses
                .get(&address)
                .is_some_and(|(mac, until)| mac != &reply.mac && *until > now())
        {
            return None;
        }
        drop(store);
        let t1 = opt32(&reply, 58).unwrap_or(seconds / 2);
        let t2 = opt32(&reply, 59).unwrap_or((u64::from(seconds) * 7 / 8) as u32);
        if t1 == 0 || t1 >= t2 || t2 >= seconds {
            return None;
        }
        let elapsed = c.started.elapsed().as_secs();
        if elapsed + 2 >= u64::from(seconds) {
            return None;
        }
        c.lease = Some(Lease {
            ip: address,
            server,
            obtained: c.started,
            seconds,
            t1,
            expires: now() + u64::from(seconds) - elapsed,
        });
        Self::publish(state, &reply.mac, c);
        c.offer = Some((address, server));
        c.started = Instant::now();
        c.sent = c.started;
        if renewing {
            c.phase = Phase::Bound;
            Self::publish(state, &reply.mac, c);
            None
        } else {
            c.phase = Phase::Probe;
            c.tries = 1;
            Some(probe(c, host))
        }
    }
    fn publish(state: &Shared, mac: &str, c: &Client) {
        let l = c.lease.as_ref().unwrap();
        let mut store = state.lock().unwrap();
        store.boot_addresses.insert(l.ip, (mac.into(), l.expires));
    }
    fn forget(state: &Shared, mac: &str, c: &Client) {
        if let Some(l) = &c.lease {
            state
                .lock()
                .unwrap()
                .boot_addresses
                .retain(|ip, (owner, expiry)| *ip != l.ip || owner != mac || *expiry != l.expires);
        }
    }
    pub(super) fn tick(&mut self, state: &Shared, host: [u8; 6]) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        self.declined
            .retain(|_, t| t.elapsed() < Duration::from_secs(600));
        self.clients.retain(|mac, c| {
            let configured = state
                .lock()
                .unwrap()
                .config
                .devices
                .iter()
                .find(|d| &d.mac == mac)
                .is_none_or(|d| d.ip.is_some());
            if configured {
                Self::forget(state, mac, c);
                return false;
            }
            if let Some(l) = &c.lease
                && l.obtained.elapsed().as_secs() >= u64::from(l.seconds)
            {
                Self::forget(state, mac, c);
                state.lock().unwrap().event(
                    mac,
                    "RARP",
                    "RARP DHCP lease expired; no address will be returned",
                );
                return false;
            }
            match c.phase {
                Phase::Probe => {
                    if c.started.elapsed() >= Duration::from_secs(2) {
                        c.phase = Phase::Bound;
                        Self::publish(state, mac, c);
                        let server = state.lock().unwrap().config.settings.server_ip;
                        out.push(answer(
                            &c.request,
                            host,
                            server,
                            c.lease.as_ref().unwrap().ip,
                        ));
                        state.lock().unwrap().event(
                            mac,
                            "RARP",
                            format!("DHCP lease confirmed: {}", c.lease.as_ref().unwrap().ip),
                        );
                    } else if c.tries == 1 && c.sent.elapsed() >= Duration::from_secs(1) {
                        out.push(probe(c, host));
                        c.tries += 1;
                    }
                }
                Phase::Bound => {
                    if c.lease.as_ref().unwrap().obtained.elapsed().as_secs()
                        >= u64::from(c.lease.as_ref().unwrap().t1)
                    {
                        let local = {
                            let store = state.lock().unwrap();
                            store.config.settings.dhcp_enabled
                                && !store.config.settings.proxy_dhcp_enabled
                                && store.config.settings.server_ip
                                    == c.lease.as_ref().unwrap().server
                        };
                        if local {
                            let l = c.lease.as_mut().unwrap();
                            l.obtained = Instant::now();
                            l.expires = now() + u64::from(l.seconds);
                            Self::publish(state, mac, c);
                            return true;
                        }
                        c.phase = Phase::Confirm;
                        c.xid = xid();
                        c.started = Instant::now();
                        c.sent = c.started;
                        c.tries = 1;
                        out.push(dhcp_frame(c, host, 3, true));
                    }
                }
                Phase::Discover | Phase::Request | Phase::Confirm => {
                    let wait = Duration::from_secs(
                        (4u64 << c.tries.min(3).saturating_sub(1)).min(32)
                            + u64::from(c.xid[3] % 3)
                            - 1,
                    );
                    if c.sent.elapsed() >= wait {
                        if c.phase != Phase::Confirm && c.tries >= 3 {
                            c.phase = Phase::Backoff;
                            c.started = Instant::now();
                            state.lock().unwrap().event(
                                mac,
                                "RARP",
                                "DHCP lease request timed out; no RARP address returned",
                            );
                        } else {
                            out.push(dhcp_frame(
                                c,
                                host,
                                if c.phase == Phase::Discover { 1 } else { 3 },
                                c.phase == Phase::Confirm,
                            ));
                            c.sent = Instant::now();
                            c.tries = c.tries.saturating_add(1);
                        }
                    }
                }
                Phase::Backoff => {
                    if c.started.elapsed() > Duration::from_secs(60) {
                        return false;
                    }
                }
            }
            true
        });
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const HOST: [u8; 6] = [2, 0, 0, 0, 0, 1];
    const MAC: [u8; 6] = [0, 0x14, 0x4f, 0xf3, 0xd5, 0xe8];
    fn setup() -> (Shared, Vec<u8>, std::path::PathBuf) {
        let path = std::env::temp_dir().join(format!("rarp-dhcp-{}.json", uuid::Uuid::new_v4()));
        let state = Store::open(path.clone()).unwrap();
        let mut s = state.lock().unwrap();
        s.config.settings.server_ip = "192.0.2.1".parse().unwrap();
        s.config.settings.dhcp_enabled = false;
        s.config.settings.proxy_dhcp_enabled = true;
        s.config.settings.pool_start = "192.0.2.100".parse().unwrap();
        s.config.settings.pool_end = "192.0.2.110".parse().unwrap();
        s.config.profiles.push(serde_json::from_value(serde_json::json!({"id":"bsd","name":"BSD","architecture":"sparc64","kind":"bsd","root":"/tmp","boot_files":{"default":"ofwboot.net"}})).unwrap());
        s.config.devices.push(Device {
            id: crate::model::mac(&MAC),
            mac: crate::model::mac(&MAC),
            profile_id: Some("bsd".into()),
            ..Default::default()
        });
        drop(s);
        let mut frame = vec![0; 60];
        frame[..6].fill(255);
        frame[6..12].copy_from_slice(&MAC);
        frame[12..22].copy_from_slice(&[0x80, 0x35, 0, 1, 8, 0, 6, 4, 0, 3]);
        frame[22..28].copy_from_slice(&MAC);
        frame[32..38].copy_from_slice(&MAC);
        (state, frame, path)
    }
    fn response(c: &Client, kind: u8) -> Vec<u8> {
        let mut p = packet(c, kind, false);
        p[0] = 2;
        p[16..20].copy_from_slice(&[192, 0, 2, 100]);
        p.truncate(240);
        p.extend_from_slice(&[
            53, 1, kind, 54, 4, 192, 0, 2, 10, 51, 4, 0, 0, 0, 60, 1, 4, 255, 255, 255, 0, 255,
        ]);
        datagram(
            [2, 0, 0, 0, 0, 10],
            [255; 6],
            "192.0.2.10".parse().unwrap(),
            Ipv4Addr::BROADCAST,
            67,
            68,
            &p,
        )
    }
    #[test]
    fn external_lease_ack_probe_renew_and_static_override() {
        let (state, frame, path) = setup();
        let mut server = Server::default();
        let key = crate::model::mac(&MAC);
        let discover = server.request(&state, HOST, &frame).unwrap();
        let (_, _, _, bytes) = udp(&discover).unwrap();
        let req = dhcp::Request::parse(bytes).unwrap();
        assert_eq!(req.message, 1);
        assert_eq!(
            req.option(61),
            Some([vec![1], MAC.to_vec()].concat().as_slice())
        );
        assert!(server.request(&state, HOST, &frame).is_none());
        let offer = response(&server.clients[&key], 2);
        let request = server.receive(&state, HOST, &offer).unwrap();
        assert_eq!(
            dhcp::Request::parse(udp(&request).unwrap().3)
                .unwrap()
                .requested,
            Some("192.0.2.100".parse().unwrap())
        );
        assert!(server.tick(&state, HOST).is_empty());
        let mut bad = response(&server.clients[&key], 5);
        bad[46] ^= 1;
        assert!(server.receive(&state, HOST, &bad).is_none());
        let ack = response(&server.clients[&key], 5);
        let probe = server.receive(&state, HOST, &ack).unwrap();
        assert_eq!(&probe[12..14], &[8, 6]);
        assert!(server.request(&state, HOST, &frame).is_none());
        server.clients.get_mut(&key).unwrap().started -= Duration::from_secs(3);
        let replies = server.tick(&state, HOST);
        assert_eq!(replies.len(), 1);
        assert_eq!(&replies[0][20..22], &[0, 4]);
        assert_eq!(&replies[0][38..42], &[192, 0, 2, 100]);
        assert!(state.lock().unwrap().config.devices[0].ip.is_none());
        state.lock().unwrap().boot_addresses.clear();
        assert!(server.request(&state, HOST, &frame).is_some());
        assert!(
            state
                .lock()
                .unwrap()
                .profile_for("192.0.2.100".parse().unwrap())
                .is_some()
        );
        server
            .clients
            .get_mut(&key)
            .unwrap()
            .lease
            .as_mut()
            .unwrap()
            .obtained -= Duration::from_secs(31);
        let renew = server.tick(&state, HOST);
        let req = dhcp::Request::parse(udp(&renew[0]).unwrap().3).unwrap();
        assert_eq!(req.current(), Ipv4Addr::UNSPECIFIED);
        assert_eq!(req.requested, Some("192.0.2.100".parse().unwrap()));
        assert!(req.server.is_none());
        let ack = response(&server.clients[&key], 5);
        assert!(server.receive(&state, HOST, &ack).is_none());
        assert!(server.clients[&key].phase == Phase::Bound);
        server
            .clients
            .get_mut(&key)
            .unwrap()
            .lease
            .as_mut()
            .unwrap()
            .obtained -= Duration::from_secs(61);
        server.tick(&state, HOST);
        assert!(!server.clients.contains_key(&key));
        assert!(state.lock().unwrap().boot_addresses.is_empty());
        state.lock().unwrap().config.devices[0].ip = Some("192.0.2.77".parse().unwrap());
        let reply = server.request(&state, HOST, &frame).unwrap();
        assert_eq!(&reply[38..42], &[192, 0, 2, 77]);
        assert!(server.clients.is_empty());
        let _ = std::fs::remove_file(path);
    }
    #[test]
    fn local_pool_conflict_is_not_returned_and_candidate_is_reserved() {
        let (state, frame, path) = setup();
        let mut server = Server::default();
        let key = crate::model::mac(&MAC);
        {
            let mut s = state.lock().unwrap();
            s.config.settings.dhcp_enabled = true;
            s.config.settings.proxy_dhcp_enabled = false;
        }
        let mut conflict = server.request(&state, HOST, &frame).unwrap();
        assert_eq!(&conflict[12..14], &[8, 6]);
        assert_eq!(
            dhcp::available_ip(&state.lock().unwrap()),
            Some("192.0.2.101".parse().unwrap())
        );
        conflict[6..12].fill(3);
        conflict[22..28].fill(3);
        conflict[28..32].copy_from_slice(&[192, 0, 2, 100]);
        let decline = server.receive(&state, HOST, &conflict).unwrap();
        assert_eq!(
            dhcp::Request::parse(udp(&decline).unwrap().3)
                .unwrap()
                .message,
            4
        );
        assert!(state.lock().unwrap().boot_addresses.is_empty());
        assert!(server.request(&state, HOST, &frame).is_none());
        server.clients.get_mut(&key).unwrap().started -= Duration::from_secs(31);
        let probe = server.request(&state, HOST, &frame).unwrap();
        assert_eq!(&probe[38..42], &[192, 0, 2, 101]);
        let _ = std::fs::remove_file(path);
    }
}
