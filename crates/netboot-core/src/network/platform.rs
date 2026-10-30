use super::{Address, Interface};
use std::net::Ipv4Addr;

#[cfg(unix)]
pub fn interfaces() -> Result<Vec<Interface>, String> {
    use std::{collections::BTreeMap, ffi::CStr};
    let mut head = std::ptr::null_mut();
    if unsafe { libc::getifaddrs(&mut head) } != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let mut entries: BTreeMap<String, Interface> = BTreeMap::new();
    let mut current = head;
    while !current.is_null() {
        let i = unsafe { &*current };
        let name = unsafe { CStr::from_ptr(i.ifa_name) }
            .to_string_lossy()
            .into_owned();
        let entry = entries.entry(name.clone()).or_insert_with(|| Interface {
            id: name.clone(),
            name: name.clone(),
            up: i.ifa_flags & libc::IFF_UP as u32 != 0
                && i.ifa_flags & libc::IFF_RUNNING as u32 != 0,
            physical: false,
            ..Interface::default()
        });
        if !i.ifa_addr.is_null() {
            let family = unsafe { (*i.ifa_addr).sa_family as i32 };
            if family == libc::AF_INET && !i.ifa_netmask.is_null() {
                let addr = unsafe { &*i.ifa_addr.cast::<libc::sockaddr_in>() };
                let mask = unsafe { &*i.ifa_netmask.cast::<libc::sockaddr_in>() };
                entry.addresses.push(Address {
                    ip: Ipv4Addr::from(addr.sin_addr.s_addr.to_ne_bytes()),
                    subnet: Ipv4Addr::from(mask.sin_addr.s_addr.to_ne_bytes()),
                });
            }
            #[cfg(any(target_os = "linux", target_os = "android"))]
            if family == libc::AF_PACKET {
                let addr = unsafe { &*i.ifa_addr.cast::<libc::sockaddr_ll>() };
                if addr.sll_halen == 6 {
                    entry.mac = crate::model::mac(&addr.sll_addr[..6]);
                }
                entry.physical =
                    std::path::Path::new(&format!("/sys/class/net/{name}/device")).exists();
            }
            #[cfg(target_os = "macos")]
            if family == libc::AF_LINK {
                let addr = unsafe { &*i.ifa_addr.cast::<libc::sockaddr_dl>() };
                if addr.sdl_alen == 6 && usize::from(addr.sdl_nlen) + 6 <= addr.sdl_data.len() {
                    let bytes = unsafe {
                        std::slice::from_raw_parts(
                            addr.sdl_data
                                .as_ptr()
                                .add(addr.sdl_nlen as usize)
                                .cast::<u8>(),
                            6,
                        )
                    };
                    entry.mac = crate::model::mac(bytes);
                }
                entry.physical =
                    name.starts_with("en") && name[2..].chars().all(|c| c.is_ascii_digit());
            }
        }
        current = i.ifa_next;
    }
    unsafe { libc::freeifaddrs(head) };
    let mut entries: Vec<_> = entries.into_values().collect();
    for entry in &mut entries {
        #[cfg(target_os = "macos")]
        {
            if let Some(mac) = super::interface_mac(&entry.id) {
                entry.mac = mac;
            } else if entry.mac == "02:00:00:00:00:00" {
                entry.mac.clear();
            }
        }
        entry.addresses.sort_by_key(|a| a.ip);
        entry.addresses.dedup();
        #[cfg(target_os = "macos")]
        macos_dhcp(entry);
    }
    Ok(entries)
}

#[cfg(target_os = "macos")]
fn macos_dhcp(entry: &mut Interface) {
    use std::ffi::{c_char, c_void};
    type Ref = *const c_void;
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFStringCreateWithCString(a: Ref, s: *const c_char, encoding: u32) -> Ref;
        fn CFRelease(value: Ref);
        fn CFDictionaryGetValue(value: Ref, key: Ref) -> Ref;
        fn CFBooleanGetValue(value: Ref) -> bool;
        fn CFGetTypeID(value: Ref) -> usize;
        fn CFDictionaryGetTypeID() -> usize;
        fn CFBooleanGetTypeID() -> usize;
        fn CFArrayGetCount(value: Ref) -> isize;
        fn CFArrayGetValueAtIndex(value: Ref, index: isize) -> Ref;
        fn CFStringGetCString(value: Ref, buf: *mut c_char, size: isize, encoding: u32) -> bool;
        fn CFDataGetLength(value: Ref) -> isize;
        fn CFDataGetBytePtr(value: Ref) -> *const u8;
        fn CFDateGetAbsoluteTime(value: Ref) -> f64;
        fn CFAbsoluteTimeGetCurrent() -> f64;
    }
    #[link(name = "SystemConfiguration", kind = "framework")]
    unsafe extern "C" {
        fn SCPreferencesCreate(a: Ref, name: Ref, id: Ref) -> Ref;
        fn SCDynamicStoreCopyValue(store: Ref, key: Ref) -> Ref;
        fn SCNetworkServiceCopyAll(prefs: Ref) -> Ref;
        fn SCNetworkServiceGetInterface(service: Ref) -> Ref;
        fn SCNetworkInterfaceGetBSDName(interface: Ref) -> Ref;
        fn SCNetworkServiceGetServiceID(service: Ref) -> Ref;
        fn SCDynamicStoreCopyDHCPInfo(store: Ref, id: Ref) -> Ref;
        fn DHCPInfoGetOptionData(info: Ref, code: u8) -> Ref;
        fn DHCPInfoGetLeaseExpirationTime(info: Ref) -> Ref;
    }
    unsafe {
        let nil = std::ptr::null();
        let key_text =
            std::ffi::CString::new(format!("State:/Network/Interface/{}/Link", entry.id)).unwrap();
        let key = CFStringCreateWithCString(nil, key_text.as_ptr(), 0x08000100);
        let link = SCDynamicStoreCopyValue(nil, key);
        CFRelease(key);
        if !link.is_null() {
            let active_key = CFStringCreateWithCString(nil, c"Active".as_ptr(), 0x08000100);
            if CFGetTypeID(link) == CFDictionaryGetTypeID() {
                let active = CFDictionaryGetValue(link, active_key);
                if !active.is_null() && CFGetTypeID(active) == CFBooleanGetTypeID() {
                    entry.up &= CFBooleanGetValue(active);
                }
            }
            CFRelease(active_key);
            CFRelease(link);
        }
        if !entry.up {
            return;
        }
        let name = CFStringCreateWithCString(nil, c"Bootah".as_ptr(), 0x08000100);
        let prefs = SCPreferencesCreate(nil, name, nil);
        CFRelease(name);
        if prefs.is_null() {
            return;
        }
        let services = SCNetworkServiceCopyAll(prefs);
        if !services.is_null() {
            for index in 0..CFArrayGetCount(services) {
                let service = CFArrayGetValueAtIndex(services, index);
                let interface = SCNetworkServiceGetInterface(service);
                if interface.is_null() {
                    continue;
                }
                let bsd = SCNetworkInterfaceGetBSDName(interface);
                if bsd.is_null() {
                    continue;
                }
                let mut buffer = [0i8; 256];
                if !CFStringGetCString(bsd, buffer.as_mut_ptr(), 256, 0x08000100) {
                    continue;
                }
                if std::ffi::CStr::from_ptr(buffer.as_ptr()).to_string_lossy() != entry.id {
                    continue;
                }
                let info = SCDynamicStoreCopyDHCPInfo(nil, SCNetworkServiceGetServiceID(service));
                if info.is_null() {
                    continue;
                }
                let expiration = DHCPInfoGetLeaseExpirationTime(info);
                let valid = expiration.is_null()
                    || CFDateGetAbsoluteTime(expiration) > CFAbsoluteTimeGetCurrent();
                let data = DHCPInfoGetOptionData(info, 54);
                if valid && !data.is_null() && CFDataGetLength(data) == 4 {
                    let bytes = std::slice::from_raw_parts(CFDataGetBytePtr(data), 4);
                    entry
                        .dhcp_servers
                        .push(Ipv4Addr::new(bytes[0], bytes[1], bytes[2], bytes[3]));
                    entry.source = "system_configuration".into();
                }
                CFRelease(info);
            }
            CFRelease(services);
        }
        CFRelease(prefs);
    }
}

#[cfg(target_os = "linux")]
fn command(program: &str, args: &[&str]) -> Option<String> {
    use std::{
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    let mut child = Command::new(program)
        .args(args)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let begin = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if begin.elapsed() < Duration::from_secs(2) => {
                std::thread::sleep(Duration::from_millis(20))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let out = child.wait_with_output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}
#[cfg(target_os = "linux")]
pub(super) fn linux_dhcp(entry: &mut Interface) {
    if !entry.up || entry.addresses.is_empty() {
        return;
    }
    if let Some(output) = command(
        "nmcli",
        &["-g", "DHCP4.OPTION", "device", "show", &entry.id],
    ) {
        for line in output.lines() {
            if let Some(("dhcp_server_identifier", value)) = line.split_once(" = ")
                && let Ok(ip) = value.trim().parse()
            {
                entry.dhcp_servers.push(ip);
                entry.source = "network_manager".into();
            }
        }
    }
    if !entry.dhcp_servers.is_empty() {
        return;
    }
    let Some(output) = command("networkctl", &["--json=short", "status", &entry.id]) else {
        return;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&output) else {
        return;
    };
    let Some(addresses) = value["Addresses"].as_array() else {
        return;
    };
    for address in addresses {
        if address["ConfigSource"].as_str() != Some("DHCPv4") {
            continue;
        }
        let Some(server) = address["ConfigProvider"].as_array() else {
            continue;
        };
        let bytes: Option<Vec<u8>> = server
            .iter()
            .map(|v| v.as_u64().and_then(|n| u8::try_from(n).ok()))
            .collect();
        if let Some(bytes) = bytes.filter(|b| b.len() == 4) {
            entry
                .dhcp_servers
                .push(Ipv4Addr::new(bytes[0], bytes[1], bytes[2], bytes[3]));
            entry.source = "networkd".into();
        }
    }
}

#[cfg(target_os = "windows")]
pub fn interfaces() -> Result<Vec<Interface>, String> {
    use std::ffi::CStr;
    use windows_sys::Win32::{NetworkManagement::IpHelper::*, Networking::WinSock::*};
    unsafe {
        let mut size = 16384u32;
        let mut buffer: Vec<u64>;
        loop {
            buffer = vec![0; (size as usize).div_ceil(8)];
            let code = GetAdaptersAddresses(
                AF_INET as u32,
                GAA_FLAG_INCLUDE_PREFIX,
                std::ptr::null(),
                buffer.as_mut_ptr().cast(),
                &mut size,
            );
            if code == 111 {
                continue;
            }
            if code != 0 {
                return Err(format!("GetAdaptersAddresses: {code}"));
            }
            break;
        }
        let mut current = buffer.as_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
        let mut out = vec![];
        while !current.is_null() {
            let i = &*current;
            let id = CStr::from_ptr(i.AdapterName.cast()).to_string_lossy();
            let mut len = 0;
            if !i.FriendlyName.is_null() {
                while *i.FriendlyName.add(len) != 0 {
                    len += 1;
                }
            }
            let mut row: MIB_IF_ROW2 = std::mem::zeroed();
            row.InterfaceLuid = i.Luid;
            let physical =
                GetIfEntry2(&mut row) == 0 && row.InterfaceAndOperStatusFlags._bitfield & 1 != 0;
            let mut entry = Interface {
                id: format!(r"\Device\NPF_{id}"),
                name: if len > 0 {
                    String::from_utf16_lossy(std::slice::from_raw_parts(i.FriendlyName, len))
                } else {
                    id.into_owned()
                },
                mac: if i.PhysicalAddressLength == 6 {
                    crate::model::mac(&i.PhysicalAddress[..6])
                } else {
                    String::new()
                },
                up: i.OperStatus == 1,
                physical,
                ..Interface::default()
            };
            let mut addr = i.FirstUnicastAddress;
            while !addr.is_null() {
                let a = &*addr;
                if !a.Address.lpSockaddr.is_null() && (*a.Address.lpSockaddr).sa_family == AF_INET {
                    let v4 = &*a.Address.lpSockaddr.cast::<SOCKADDR_IN>();
                    let prefix = a.OnLinkPrefixLength.min(32);
                    entry.addresses.push(Address {
                        ip: Ipv4Addr::from(v4.sin_addr.S_un.S_addr.to_ne_bytes()),
                        subnet: Ipv4Addr::from(if prefix == 0 {
                            0
                        } else {
                            u32::MAX << (32 - prefix)
                        }),
                    });
                }
                addr = a.Next;
            }
            if i.Anonymous2.Flags & IP_ADAPTER_DHCP_ENABLED != 0
                && !i.Dhcpv4Server.lpSockaddr.is_null()
                && (*i.Dhcpv4Server.lpSockaddr).sa_family == AF_INET
            {
                let v4 = &*i.Dhcpv4Server.lpSockaddr.cast::<SOCKADDR_IN>();
                entry
                    .dhcp_servers
                    .push(Ipv4Addr::from(v4.sin_addr.S_un.S_addr.to_ne_bytes()));
                entry.source = "ip_helper".into();
            }
            out.push(entry);
            current = i.Next;
        }
        Ok(out)
    }
}
