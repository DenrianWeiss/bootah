use std::{collections::BTreeMap, net::Ipv4Addr};

type Cache = (String, std::time::Instant, BTreeMap<Ipv4Addr, String>);
static CACHE: std::sync::OnceLock<std::sync::Mutex<Cache>> = std::sync::OnceLock::new();
pub fn cached_mac(ip: Ipv4Addr, interface: &str) -> Option<String> {
    let cache = CACHE.get()?.lock().ok()?;
    if cache.0 != interface || cache.1.elapsed() > std::time::Duration::from_secs(10) {
        return None;
    }
    cache.2.get(&ip).cloned()
}
pub fn resolve(ip: Ipv4Addr, interface: &str) -> Option<String> {
    type Misses = BTreeMap<(String, Ipv4Addr), std::time::Instant>;
    static MISSES: std::sync::OnceLock<std::sync::Mutex<Misses>> = std::sync::OnceLock::new();
    let key = (interface.to_string(), ip);
    {
        let mut misses = MISSES.get_or_init(Default::default).lock().ok()?;
        misses.retain(|_, time| time.elapsed() < std::time::Duration::from_secs(2));
        if misses.contains_key(&key) {
            return None;
        }
        if misses.len() >= 1024 {
            misses.clear();
        }
        misses.insert(key.clone(), std::time::Instant::now());
    }
    let mac = lookup(interface).remove(&ip);
    if mac.is_some() {
        MISSES.get()?.lock().ok()?.remove(&key);
    }
    mac
}
pub fn lookup(interface: &str) -> BTreeMap<Ipv4Addr, String> {
    let mut result = BTreeMap::new();
    for (ip, mac, nic) in entries() {
        if (!interface.is_empty() && !interface.eq_ignore_ascii_case(&nic))
            || !crate::network::usable(ip)
            || ip.is_loopback()
        {
            continue;
        }
        if mac.len() != 6 || mac[0] & 1 != 0 || mac.iter().all(|b| *b == 0) {
            continue;
        }
        let mac = crate::model::mac(&mac);
        if let Some(previous) = result.insert(ip, mac.clone()) {
            if previous != mac {
                result.insert(ip, String::new());
            }
        }
    }
    result.retain(|_, mac| !mac.is_empty());
    *CACHE
        .get_or_init(|| {
            std::sync::Mutex::new((String::new(), std::time::Instant::now(), BTreeMap::new()))
        })
        .lock()
        .unwrap() = (interface.into(), std::time::Instant::now(), result.clone());
    result
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn entries() -> Vec<(Ipv4Addr, Vec<u8>, String)> {
    std::fs::read_to_string("/proc/net/arp")
        .unwrap_or_default()
        .lines()
        .skip(1)
        .filter_map(|line| {
            let fields: Vec<_> = line.split_whitespace().collect();
            if fields.len() != 6
                || fields[1] != "0x1"
                || u32::from_str_radix(fields[2].trim_start_matches("0x"), 16).ok()? & 2 == 0
            {
                return None;
            }
            let mac = fields[3]
                .split(':')
                .map(|s| u8::from_str_radix(s, 16))
                .collect::<Result<Vec<_>, _>>()
                .ok()?;
            Some((fields[0].parse().ok()?, mac, fields[5].into()))
        })
        .collect()
}

#[cfg(target_os = "macos")]
fn entries() -> Vec<(Ipv4Addr, Vec<u8>, String)> {
    let mut result = vec![];
    unsafe {
        let mut mib = [
            libc::CTL_NET,
            libc::PF_ROUTE,
            0,
            libc::AF_INET,
            libc::NET_RT_FLAGS,
            libc::RTF_LLINFO,
        ];
        let mut len = 0;
        if libc::sysctl(
            mib.as_mut_ptr(),
            6,
            std::ptr::null_mut(),
            &mut len,
            std::ptr::null_mut(),
            0,
        ) != 0
            || len > 16 * 1024 * 1024
        {
            return result;
        }
        let mut bytes = vec![0u8; len];
        if libc::sysctl(
            mib.as_mut_ptr(),
            6,
            bytes.as_mut_ptr().cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        ) != 0
        {
            return result;
        }
        bytes.truncate(len);
        let mut pos = 0;
        while pos + std::mem::size_of::<libc::rt_msghdr>() <= bytes.len() {
            let header =
                std::ptr::read_unaligned(bytes.as_ptr().add(pos).cast::<libc::rt_msghdr>());
            let end = pos + header.rtm_msglen as usize;
            if end > bytes.len()
                || header.rtm_msglen as usize <= std::mem::size_of::<libc::rt_msghdr>()
            {
                break;
            }
            let mut next = pos + std::mem::size_of::<libc::rt_msghdr>();
            let mut ip = None;
            let mut mac = None;
            for slot in 0..libc::RTAX_MAX {
                if header.rtm_addrs & (1 << slot) == 0 {
                    continue;
                }
                if next + 2 > end {
                    break;
                }
                let size = bytes[next] as usize;
                let family = bytes[next + 1] as i32;
                if size < 2 || next + size > end {
                    break;
                }
                if slot == libc::RTAX_DST && family == libc::AF_INET && size >= 8 {
                    ip = Some(Ipv4Addr::new(
                        bytes[next + 4],
                        bytes[next + 5],
                        bytes[next + 6],
                        bytes[next + 7],
                    ));
                }
                if slot == libc::RTAX_GATEWAY && family == libc::AF_LINK && size >= 8 {
                    let name = bytes[next + 5] as usize;
                    let count = bytes[next + 6] as usize;
                    if count == 6 && 8 + name + count <= size {
                        mac = Some(bytes[next + 8 + name..next + 8 + name + count].to_vec());
                    }
                }
                next += (size + 3) & !3;
            }
            if (header.rtm_rmx.rmx_expire == 0
                || header.rtm_rmx.rmx_expire as u64 > crate::model::now())
                && header.rtm_flags & (libc::RTF_GATEWAY | libc::RTF_REJECT | libc::RTF_BLACKHOLE)
                    == 0
            {
                if let (Some(ip), Some(mac)) = (ip, mac) {
                    let mut name = [0i8; libc::IF_NAMESIZE];
                    if !libc::if_indextoname(header.rtm_index.into(), name.as_mut_ptr()).is_null() {
                        result.push((
                            ip,
                            mac,
                            std::ffi::CStr::from_ptr(name.as_ptr())
                                .to_string_lossy()
                                .into_owned(),
                        ));
                    }
                }
            }
            pos = end;
        }
    }
    result
}

#[cfg(windows)]
fn entries() -> Vec<(Ipv4Addr, Vec<u8>, String)> {
    use windows_sys::Win32::{NetworkManagement::IpHelper::*, Networking::WinSock::*};
    let mut result = vec![];
    unsafe {
        let mut table = std::ptr::null_mut();
        if GetIpNetTable2(AF_INET, &mut table) != 0 {
            return result;
        }
        for row in std::slice::from_raw_parts((*table).Table.as_ptr(), (*table).NumEntries as usize)
        {
            if row.PhysicalAddressLength != 6
                || ![
                    NlnsReachable,
                    NlnsStale,
                    NlnsDelay,
                    NlnsProbe,
                    NlnsPermanent,
                ]
                .contains(&row.State)
            {
                continue;
            }
            let ip = Ipv4Addr::from(row.Address.Ipv4.sin_addr.S_un.S_addr.to_ne_bytes());
            let mut guid = std::mem::zeroed();
            if ConvertInterfaceLuidToGuid(&row.InterfaceLuid, &mut guid) != 0 {
                continue;
            }
            let nic = format!(
                "\\\\Device\\NPF_{{{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}",
                guid.data1,
                guid.data2,
                guid.data3,
                guid.data4[0],
                guid.data4[1],
                guid.data4[2],
                guid.data4[3],
                guid.data4[4],
                guid.data4[5],
                guid.data4[6],
                guid.data4[7]
            );
            result.push((ip, row.PhysicalAddress[..6].to_vec(), nic));
        }
        FreeMibTable(table.cast());
    }
    result
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "linux",
    target_os = "android",
    windows
)))]
fn entries() -> Vec<(Ipv4Addr, Vec<u8>, String)> {
    vec![]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn aliases_use_mac_assignment_but_independent_ip_and_conflicts_remain_separate() {
        use crate::model::{Device, Store};
        let path = std::env::temp_dir().join(format!("bootah-alias-{}.json", uuid::Uuid::new_v4()));
        let state = Store::open(path).unwrap();
        let mut s = state.lock().unwrap();
        s.config.settings.interface = "test-neighbor-alias".into();
        let ip = "192.0.2.21".parse().unwrap();
        let mac = "52:54:00:00:00:01".to_string();
        s.config.devices.push(Device {
            id: "mac-device".into(),
            mac: mac.clone(),
            ip: Some("192.0.2.20".parse().unwrap()),
            ..Default::default()
        });
        *CACHE.get_or_init(||std::sync::Mutex::new((String::new(),std::time::Instant::now(),BTreeMap::new()))).lock().unwrap() = (
            "test-neighbor-alias".into(),
            std::time::Instant::now(),
            BTreeMap::from([(ip, mac)]),
        );
        assert_eq!(s.device_for(ip).unwrap().id, "mac-device");
        s.config.devices.push(Device {
            id: "independent-ip".into(),
            ip: Some(ip),
            ..Default::default()
        });
        assert_eq!(s.device_for(ip).unwrap().id, "independent-ip");
        s.config.devices[1].mac = "52:54:00:00:00:02".into();
        assert!(s.device_for(ip).is_none());
        s.config.devices.pop();
        CACHE.get().unwrap().lock().unwrap().1 =
            std::time::Instant::now() - std::time::Duration::from_secs(11);
        assert!(s.device_for(ip).is_none());
    }
}
