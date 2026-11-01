//! Solaris bootparam_prot.x v1 WHOAMI/GETFILE; assigned clients only.
use crate::model::Shared;
use std::net::Ipv4Addr;
fn word(data: &[u8], pos: &mut usize) -> Option<u32> {
    let b = data.get(*pos..*pos + 4)?;
    *pos += 4;
    Some(u32::from_be_bytes(b.try_into().ok()?))
}
fn string(data: &[u8], pos: &mut usize, limit: usize) -> Option<String> {
    let n = word(data, pos)? as usize;
    if n > limit {
        return None;
    }
    let s = std::str::from_utf8(data.get(*pos..*pos + n)?)
        .ok()?
        .to_string();
    *pos += n.div_ceil(4) * 4;
    data.get(..*pos)?;
    Some(s)
}
fn putword(out: &mut Vec<u8>, n: u32) {
    out.extend(n.to_be_bytes())
}
fn putstring(out: &mut Vec<u8>, s: &str) {
    putword(out, s.len() as u32);
    out.extend(s.as_bytes());
    while !out.len().is_multiple_of(4) {
        out.push(0)
    }
}
fn address(out: &mut Vec<u8>, ip: Ipv4Addr) {
    putword(out, 1);
    for b in ip.octets() {
        putword(out, b as i8 as i32 as u32)
    }
}
fn body(
    state: &Shared,
    ip: Ipv4Addr,
    procedure: u32,
    args: &[u8],
    rootopts: &str,
) -> Option<Vec<u8>> {
    let store = state.lock().ok()?;
    let profile = store
        .profile_for(ip)
        .filter(|p| (p.kind == "solaris" && p.architecture == "sparc64") || crate::openbsd::network_root(p))?;
    let device = store.device_for(ip)?;
    let client = format!(
        "bootah-{}",
        if device.mac.is_empty() {
            ip.to_string().replace('.', "-")
        } else {
            device.mac.replace(':', "")
        }
    );
    let settings = &store.config.settings;
    let mut pos = 0;
    let mut out = Vec::new();
    match procedure {
        0 => {}
        1 => {
            if word(args, &mut pos)? != 1 {
                return None;
            }
            let mut a = [0u8; 4];
            for b in &mut a {
                *b = word(args, &mut pos)? as u8
            }
            if Ipv4Addr::from(a) != ip {
                return None;
            }
            putstring(&mut out, &client);
            putstring(&mut out, "");
            address(&mut out, settings.router.unwrap_or(Ipv4Addr::UNSPECIFIED));
        }
        2 => {
            if string(args, &mut pos, 255)? != client {
                return None;
            }
            let key = string(args, &mut pos, 32)?;
            let path = match key.as_str() {
                "root" if crate::openbsd::network_root(&profile) => format!("/{}", profile.id),
                "root" => format!("/{}/Solaris_10/Tools/Boot", profile.id),
                "install" => format!("/{}", profile.id),
                "rootopts" => rootopts.into(),
                _ => return None,
            };
            putstring(&mut out, "bootah");
            address(&mut out, settings.server_ip);
            putstring(&mut out, &path);
        }
        _ => return None,
    }
    Some(out)
}
pub fn reply(state: &Shared, ip: Ipv4Addr, packet: &[u8]) -> Option<Vec<u8>> {
    reply_options(state, ip, packet, "vers=3,proto=tcp")
}
pub fn reply_options(
    state: &Shared,
    ip: Ipv4Addr,
    packet: &[u8],
    rootopts: &str,
) -> Option<Vec<u8>> {
    let mut pos = 0;
    let xid = word(packet, &mut pos)?;
    if word(packet, &mut pos)? != 0
        || word(packet, &mut pos)? != 2
        || word(packet, &mut pos)? != 100026
        || word(packet, &mut pos)? != 1
    {
        return None;
    }
    let procedure = word(packet, &mut pos)?;
    for _ in 0..2 {
        word(packet, &mut pos)?;
        let n = word(packet, &mut pos)? as usize;
        if n > 400 {
            return None;
        }
        pos = pos.checked_add(n.div_ceil(4) * 4)?;
        packet.get(..pos)?;
    }
    let body = body(state, ip, procedure, packet.get(pos..)?, rootopts)?;
    let mut out = Vec::new();
    for v in [xid, 1, 0, 0, 0, 0] {
        putword(&mut out, v)
    }
    out.extend(body);
    Some(out)
}
pub async fn start(
    state: Shared,
    config: &crate::model::Config,
) -> Result<Vec<tokio::task::JoinHandle<()>>, String> {
    let Some(port) = configured_port_for(config)? else {
        return Ok(Vec::new());
    };
    let bind = state
        .lock()
        .map_err(|e| e.to_string())?
        .config
        .settings
        .bind_ip;
    let socket = tokio::net::UdpSocket::bind((bind, port))
        .await
        .map_err(|e| e.to_string())?;
    Ok(vec![tokio::spawn(async move {
        let mut data = [0; 4096];
        while let Ok((n, peer)) = socket.recv_from(&mut data).await {
            let std::net::IpAddr::V4(ip) = peer.ip() else {
                continue;
            };
            if let Some(out) = reply(&state, ip, &data[..n]) {
                let _ = socket.send_to(&out, peer).await;
            }
        }
    })])
}

/// RPC portmapper CALLIT used by diskless clients' broadcast WHOAMI.
pub fn callit(state: &Shared, ip: Ipv4Addr, packet: &[u8]) -> Option<Vec<u8>> {
    callit_options(state, ip, packet, "vers=3,proto=tcp")
}
pub fn callit_options(
    state: &Shared,
    ip: Ipv4Addr,
    packet: &[u8],
    rootopts: &str,
) -> Option<Vec<u8>> {
    let mut pos = 0;
    let xid = word(packet, &mut pos)?;
    if word(packet, &mut pos)? != 0
        || word(packet, &mut pos)? != 2
        || word(packet, &mut pos)? != 100000
        || word(packet, &mut pos)? != 2
        || word(packet, &mut pos)? != 5
    {
        return None;
    }
    for _ in 0..2 {
        word(packet, &mut pos)?;
        let n = word(packet, &mut pos)? as usize;
        if n > 400 {
            return None;
        }
        pos = pos.checked_add(n.div_ceil(4) * 4)?;
        packet.get(..pos)?;
    }
    if word(packet, &mut pos)? != 100026 || word(packet, &mut pos)? != 1 {
        return None;
    }
    let procedure = word(packet, &mut pos)?;
    let length = word(packet, &mut pos)? as usize;
    let args = packet.get(pos..pos.checked_add(length)?)?;
    let body = body(state, ip, procedure, args, rootopts)?;
    let port = configured_port(state).ok()??;
    let mut out = Vec::new();
    for v in [xid, 1, 0, 0, 0, 0, port as u32, body.len() as u32] {
        putword(&mut out, v)
    }
    out.extend(body);
    while !out.len().is_multiple_of(4) {
        out.push(0)
    }
    Some(out)
}

pub fn configured_port(state: &Shared) -> Result<Option<u16>, String> {
    configured_port_for(&state.lock().map_err(|e| e.to_string())?.config)
}
pub fn configured_port_for(config: &crate::model::Config) -> Result<Option<u16>, String> {
    let enabled = config.profiles.iter().any(|p| {
        ((p.kind == "solaris" && p.architecture == "sparc64") || crate::openbsd::network_root(p))
            && (config
                .devices
                .iter()
                .any(|d| d.profile_id.as_ref() == Some(&p.id))
                || config.default_profiles.values().any(|id| id == &p.id))
    });
    crate::nfs::configured_override("BOOTAH_BOOTPARAM_PORT", enabled.then_some(10000))
}
