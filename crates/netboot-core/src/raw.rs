use crate::{dhcp, model::*, tftp};
#[cfg(not(any(target_os = "linux", target_os = "android")))]
use libloading::Library;
use std::{
    collections::HashMap,
    ffi::CString,
    io::{Read, Seek, SeekFrom},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

#[cfg(not(any(target_os = "linux", target_os = "android")))]
mod capture {
    use super::*;
    use std::ffi::{CStr, c_char, c_int, c_long, c_void};
    #[repr(C)]
    struct Header {
        sec: c_long,
        usec: c_long,
        caplen: u32,
        len: u32,
    }
    type Open =
        unsafe extern "C" fn(*const c_char, c_int, c_int, c_int, *mut c_char) -> *mut c_void;
    type Next = unsafe extern "C" fn(*mut c_void, *mut *const Header, *mut *const u8) -> c_int;
    type Send = unsafe extern "C" fn(*mut c_void, *const u8, c_int) -> c_int;
    type Close = unsafe extern "C" fn(*mut c_void);
    type Nonblock = unsafe extern "C" fn(*mut c_void, c_int, *mut c_char) -> c_int;
    type Datalink = unsafe extern "C" fn(*mut c_void) -> c_int;
    pub(super) struct Capture {
        _library: Library,
        handle: *mut c_void,
        next: Next,
        send: Send,
        close: Close,
    }
    impl Capture {
        #[cfg(not(target_os = "macos"))]
        pub(super) fn open(interface: &str) -> Result<Self, String> {
            Self::open_local(interface)
        }

        pub(super) fn open_local(interface: &str) -> Result<Self, String> {
            let names: &[&str] = if cfg!(target_os = "windows") {
                &["wpcap.dll"]
            } else if cfg!(target_os = "macos") {
                &["/usr/lib/libpcap.A.dylib"]
            } else {
                &["libpcap.so.1", "libpcap.so"]
            };
            let library = names
                .iter()
                .find_map(|name| unsafe { Library::new(name).ok() })
                .ok_or(if cfg!(target_os = "windows") { "Install Npcap from https://npcap.com/#download with WinPcap API-compatible mode enabled, then restart Bootah." } else { "The system libpcap library could not be loaded. Restore the system library and restart Bootah." })?;
            #[cfg(target_os = "windows")]
            let interface = crate::network::windows_capture_id(interface);
            let interface = CString::new(interface).map_err(|e| e.to_string())?;
            let mut error = [0i8; 256];
            unsafe {
                let open: Open = *library
                    .get(b"pcap_open_live\0")
                    .map_err(|e| e.to_string())?;
                let next: Next = *library.get(b"pcap_next_ex\0").map_err(|e| e.to_string())?;
                let send: Send = *library
                    .get(b"pcap_sendpacket\0")
                    .map_err(|e| e.to_string())?;
                let close: Close = *library.get(b"pcap_close\0").map_err(|e| e.to_string())?;
                let nonblock: Nonblock = *library
                    .get(b"pcap_setnonblock\0")
                    .map_err(|e| e.to_string())?;
                let datalink: Datalink =
                    *library.get(b"pcap_datalink\0").map_err(|e| e.to_string())?;
                let handle = open(interface.as_ptr(), 65536, 1, 100, error.as_mut_ptr());
                if handle.is_null() {
                    let detail = CStr::from_ptr(error.as_ptr())
                        .to_string_lossy()
                        .into_owned();
                    let lower = detail.to_lowercase();
                    if lower.contains("permission")
                        || lower.contains("permitted")
                        || lower.contains("denied")
                        || lower.contains("access is denied")
                    {
                        let guidance = if cfg!(target_os = "macos") {
                            "Open Network permissions in Bootah and enable its bundled network helper in macOS System Settings. Alternatively, install ChmodBPF from the official Wireshark macOS disk image: https://www.wireshark.org/download.html . Then retry."
                        } else {
                            "Allow the Npcap authorization prompt. If access is still denied, restart Bootah using Run as administrator."
                        };
                        return Err(format!("{guidance}\n{detail}"));
                    }
                    return Err(detail);
                }
                if datalink(handle) != 1 {
                    close(handle);
                    return Err("Ethernet interface required".into());
                }
                if nonblock(handle, 1, error.as_mut_ptr()) != 0 {
                    close(handle);
                    let detail = CStr::from_ptr(error.as_ptr())
                        .to_string_lossy()
                        .into_owned();
                    let lower = detail.to_lowercase();
                    if lower.contains("permission")
                        || lower.contains("permitted")
                        || lower.contains("denied")
                        || lower.contains("access is denied")
                    {
                        let guidance = if cfg!(target_os = "macos") {
                            "Open Network permissions in Bootah and enable its bundled network helper in macOS System Settings. Alternatively, install ChmodBPF from the official Wireshark macOS disk image: https://www.wireshark.org/download.html . Then retry."
                        } else {
                            "Allow the Npcap authorization prompt. If access is still denied, restart Bootah using Run as administrator."
                        };
                        return Err(format!("{guidance}\n{detail}"));
                    }
                    return Err(detail);
                }
                Ok(Self {
                    _library: library,
                    handle,
                    next,
                    send,
                    close,
                })
            }
        }
        #[cfg(not(target_os = "macos"))]
        pub(super) fn probe(interface: &str, authorize: bool) -> Result<Self, String> {
            #[cfg(target_os = "windows")]
            {
                #[link(name = "shell32")]
                unsafe extern "system" {
                    fn IsUserAnAdmin() -> i32;
                }
                if !authorize && unsafe { IsUserAnAdmin() } == 0 {
                    return Err(
                        "Allow the Npcap authorization prompt. If access is still denied, restart Bootah using Run as administrator.".into(),
                    );
                }
            }
            let _ = authorize;
            Self::open(interface)
        }
        pub(super) fn receive(&self) -> Result<Option<Vec<u8>>, String> {
            unsafe {
                let mut header = std::ptr::null();
                let mut data = std::ptr::null();
                match (self.next)(self.handle, &mut header, &mut data) {
                    1 => {
                        if header.is_null() || data.is_null() || (*header).caplen > 65536 {
                            return Err("Invalid capture frame".into());
                        }
                        Ok(Some(
                            std::slice::from_raw_parts(data, (*header).caplen as usize).to_vec(),
                        ))
                    }
                    0 => Ok(None),
                    _ => Err("Packet capture stopped".into()),
                }
            }
        }
        pub(super) fn send(&self, frame: &[u8]) -> Result<(), String> {
            if unsafe { (self.send)(self.handle, frame.as_ptr(), frame.len() as c_int) } != 0 {
                Err("Packet injection failed".into())
            } else {
                Ok(())
            }
        }
    }
    impl Drop for Capture {
        fn drop(&mut self) {
            unsafe { (self.close)(self.handle) }
        }
    }
}
#[cfg(any(target_os = "linux", target_os = "android"))]
mod capture {
    use super::*;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    pub(super) struct Capture {
        fd: OwnedFd,
    }
    pub(super) fn open_packet(interface: &str) -> Result<OwnedFd, String> {
        let name = CString::new(interface).map_err(|e| e.to_string())?;
        let index = unsafe { libc::if_nametoindex(name.as_ptr()) };
        if index == 0 {
            return Err("Unknown Ethernet interface".into());
        }
        let raw = unsafe {
            libc::socket(
                libc::AF_PACKET,
                libc::SOCK_RAW | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
                (libc::ETH_P_ALL as u16).to_be() as i32,
            )
        };
        if raw < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::PermissionDenied {
                return Err(format!(
                    "Network permission is required. Use Authorize detection for DHCP probing. To start services on Linux, install the Bootah network helper and approve polkit or sudo authorization.\n{error}"
                ));
            }
            return Err(error.to_string());
        }
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        let mut address: libc::sockaddr_ll = unsafe { std::mem::zeroed() };
        address.sll_family = libc::AF_PACKET as u16;
        address.sll_protocol = (libc::ETH_P_ALL as u16).to_be();
        address.sll_ifindex = index as i32;
        if unsafe {
            libc::bind(
                raw,
                (&address as *const libc::sockaddr_ll).cast(),
                std::mem::size_of_val(&address) as u32,
            )
        } != 0
        {
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok(fd)
    }
    impl Capture {
        pub(super) fn open(interface: &str) -> Result<Self, String> {
            match open_packet(interface) {
                Ok(fd) => Ok(Self { fd }),
                #[cfg(target_os = "linux")]
                Err(e)
                    if unsafe { libc::geteuid() } != 0
                        && e.starts_with("Network permission is required.") =>
                {
                    Ok(Self {
                        fd: crate::privilege::request("ethernet", interface)?,
                    })
                }
                Err(e) => Err(e),
            }
        }
        pub(super) fn probe(interface: &str, authorize: bool) -> Result<Self, String> {
            if authorize {
                Self::open(interface)
            } else {
                open_packet(interface).map(|fd| Self { fd })
            }
        }
        pub(super) fn receive(&self) -> Result<Option<Vec<u8>>, String> {
            let mut frame = vec![0; 65536];
            let n = unsafe {
                libc::recv(
                    self.fd.as_raw_fd(),
                    frame.as_mut_ptr().cast(),
                    frame.len(),
                    0,
                )
            };
            if n < 0 {
                let e = std::io::Error::last_os_error();
                return if e.kind() == std::io::ErrorKind::WouldBlock {
                    Ok(None)
                } else {
                    Err(e.to_string())
                };
            }
            frame.truncate(n as usize);
            Ok(Some(frame))
        }
        pub(super) fn send(&self, frame: &[u8]) -> Result<(), String> {
            let n =
                unsafe { libc::send(self.fd.as_raw_fd(), frame.as_ptr().cast(), frame.len(), 0) };
            if n != frame.len() as isize {
                Err(std::io::Error::last_os_error().to_string())
            } else {
                Ok(())
            }
        }
    }
}
#[cfg(not(target_os = "macos"))]
use capture::Capture;
#[cfg(target_os = "macos")]
enum Capture {
    Local(capture::Capture),
    Authorized(crate::macos_privilege::Session),
}
#[cfg(target_os = "macos")]
impl Capture {
    fn open(interface: &str) -> Result<Self, String> {
        Self::probe(interface, true)
    }
    fn probe(interface: &str, authorize: bool) -> Result<Self, String> {
        match capture::Capture::open_local(interface) {
            Ok(c) => Ok(Self::Local(c)),
            Err(e)
                if authorize
                    && unsafe { libc::geteuid() } != 0
                    && e.contains(crate::macos_privilege::GUIDANCE) =>
            {
                crate::macos_privilege::capture(interface).map(Self::Authorized)
            }
            Err(e)
                if e.contains(crate::macos_privilege::GUIDANCE)
                    && !crate::macos_privilege::availability().helper_available =>
            {
                Err(crate::macos_privilege::BPF_GUIDANCE.into())
            }
            Err(e) => Err(e),
        }
    }
    fn receive(&self) -> Result<Option<Vec<u8>>, String> {
        match self {
            Self::Local(c) => c.receive(),
            Self::Authorized(s) => s.request(1, &[]).map(|v| (!v.is_empty()).then_some(v)),
        }
    }
    fn send(&self, frame: &[u8]) -> Result<(), String> {
        match self {
            Self::Local(c) => c.send(frame),
            Self::Authorized(s) => s.request(2, frame).map(|_| ()),
        }
    }
}
#[cfg(target_os = "macos")]
pub fn serve_authorized_capture(
    stream: &std::os::unix::net::UnixStream,
    interface: &str,
) -> Result<(), String> {
    use crate::macos_privilege::{read_message, write_message};
    let capture = capture::Capture::open_local(interface)?;
    write_message(stream, 0, &[])?;
    loop {
        let (operation, data) = match read_message(stream) {
            Ok(message) => message,
            Err(_) => return Ok(()),
        };
        let result = match operation {
            1 if data.is_empty() => capture.receive().map(Option::unwrap_or_default),
            2 if (14..=65536).contains(&data.len()) => capture.send(&data).map(|_| Vec::new()),
            _ => Err("Invalid network helper capture operation".into()),
        };
        match result {
            Ok(data) => write_message(stream, 0, &data)?,
            Err(error) => {
                write_message(stream, 1, error.as_bytes())?;
                return Ok(());
            }
        }
    }
}
struct Session {
    file: std::fs::File,
    id: u16,
    last_seen: u64,
}
struct Transfer {
    ip: std::net::Ipv4Addr,
    mac: [u8; 6],
    port: u16,
    request: Vec<u8>,
    session: tftp::Session,
}
pub struct Ethernet {
    state: Shared,
    server_mac: [u8; 6],
    host_mac: Option<String>,
    sessions: HashMap<String, Session>,
    next_session: u16,
    transfers: HashMap<u16, Transfer>,
    next_port: u16,
    virtual_wire: bool,
    mop: crate::mop::Server,
    last: crate::last::Server,
    pending_frames: Vec<Vec<u8>>,
}
impl Ethernet {
    pub fn new(state: Shared, server_mac: [u8; 6], virtual_wire: bool) -> Self {
        #[cfg(target_os = "macos")]
        let host_mac = if virtual_wire {
            None
        } else {
            crate::network::interface_mac(&state.lock().unwrap().config.settings.interface)
        };
        #[cfg(not(target_os = "macos"))]
        let host_mac = None;
        Self {
            state,
            server_mac,
            host_mac,
            sessions: HashMap::new(),
            next_session: 1,
            transfers: HashMap::new(),
            next_port: 20000,
            virtual_wire,
            mop: crate::mop::Server::default(),
            last: crate::last::Server::default(),
            pending_frames: Vec::new(),
        }
    }
    pub fn handle(&mut self, frame: &[u8]) -> Option<Vec<u8>> {
        if frame.len() < 14 || frame[6..12] == self.server_mac {
            return None;
        }
        if frame[12..14] == [8, 6] {
            if self.host_mac.is_some() {
                self.observe_arp(frame);
            }
            return if self.virtual_wire {
                self.arp(frame)
            } else {
                None
            };
        }
        if let Some((ip, port, destination, data)) = udp(frame) {
            let settings = self.state.lock().unwrap().config.settings.clone();
            if destination == 4011 && data.starts_with(b"\x81NCQ") {
                let response = crate::binl::reply(&self.state, ip, data)?;
                let client_mac = frame[6..12].try_into().ok()?;
                return Some(datagram(
                    self.server_mac,
                    client_mac,
                    settings.server_ip,
                    ip,
                    4011,
                    port,
                    &response,
                ));
            }
            if (destination == 67
                || destination == settings.pxe_port
                || (self.virtual_wire && destination == 4011))
                && (settings.dhcp_enabled || settings.proxy_dhcp_enabled)
            {
                return self.dhcp(frame);
            }
            if self.virtual_wire && (destination == 69 || self.transfers.contains_key(&destination))
            {
                let mut client_mac = [0; 6];
                client_mac.copy_from_slice(&frame[6..12]);
                return self.tftp(ip, port, destination, client_mac, data);
            }
        }
        if frame[12..14] == [0x60, 1]
            || frame.get(14..22) == Some(&[0xaa, 0xaa, 3, 8, 0, 0x2b, 0x60, 1])
        {
            return self.mop.handle(&self.state, self.server_mac, frame);
        }
        if frame[12..14] == [0x80, 0x41] {
            return self.last.handle(&self.state, self.server_mac, frame);
        }
        if frame[12..14] == [0x80, 0x35] {
            return self.rarp(frame);
        }
        if frame.len() >= 25
            && u16::from_be_bytes([frame[12], frame[13]]) <= 1500
            && frame[14..24] == [0xf8, 0xf8, 3, 0, 0, 0, 6, 8, 6, 9]
        {
            return self.rmp(frame);
        }
        None
    }
    fn arp(&self, frame: &[u8]) -> Option<Vec<u8>> {
        if frame.len() < 42 || frame[14..22] != [0, 1, 8, 0, 6, 4, 0, 1] {
            return None;
        }
        let server = self.state.lock().unwrap().config.settings.server_ip;
        if frame[38..42] != server.octets() {
            return None;
        }
        let mut out = Vec::from(&frame[..42]);
        out[..6].copy_from_slice(&frame[6..12]);
        out[6..12].copy_from_slice(&self.server_mac);
        out[20..22].copy_from_slice(&2u16.to_be_bytes());
        out[32..42].copy_from_slice(&frame[22..32]);
        out[22..28].copy_from_slice(&self.server_mac);
        out[28..32].copy_from_slice(&server.octets());
        out.resize(60, 0);
        Some(out)
    }
    fn observe_arp(&self, frame: &[u8]) {
        if frame.len() < 42
            || frame[14..20] != [0, 1, 8, 0, 6, 4]
            || frame[20] != 0
            || !matches!(frame[21], 1 | 2)
            || frame[6..12] != frame[22..28]
        {
            return;
        }
        let ip = std::net::Ipv4Addr::new(frame[28], frame[29], frame[30], frame[31]);
        let mac = crate::model::mac(&frame[6..12]);
        if self.host_mac.as_ref() == Some(&mac) {
            return;
        }
        let mut store = self.state.lock().unwrap();
        let known_ip = store
            .config
            .devices
            .iter()
            .find(|device| device.mac == mac)
            .and_then(|device| device.ip);
        if frame[38..42] == store.config.settings.server_ip.octets()
            && ip != store.config.settings.server_ip
            && crate::network::usable(ip)
            && known_ip.is_some_and(|previous| previous != ip)
            && store.associate(ip, &mac)
        {
            if let Err(error) = store.save() {
                store.event("network", "storage", error);
            }
        }
    }
    fn tftp(
        &mut self,
        ip: std::net::Ipv4Addr,
        port: u16,
        destination: u16,
        client_mac: [u8; 6],
        data: &[u8],
    ) -> Option<Vec<u8>> {
        let server = self.state.lock().unwrap().config.settings.server_ip;
        if destination == 69 {
            if let Some((&tid, transfer)) = self.transfers.iter().find(|(_, t)| {
                t.ip == ip && t.port == port && t.mac == client_mac && t.request == data
            }) {
                return Some(datagram(
                    self.server_mac,
                    client_mac,
                    server,
                    ip,
                    tid,
                    port,
                    &transfer.session.packet,
                ));
            }
            self.transfers.retain(|_, t| t.ip != ip || t.port != port);
            if self.transfers.len() >= 64 {
                return None;
            }
            let session = match tftp::Session::begin(&self.state, data, ip) {
                Ok(s) => s,
                Err(packet) => {
                    self.state.lock().unwrap().event(
                        &ip.to_string(),
                        "TFTP",
                        format!(
                            "Rejected: {}",
                            String::from_utf8_lossy(&packet[4..packet.len() - 1])
                        ),
                    );
                    return Some(datagram(
                        self.server_mac,
                        client_mac,
                        server,
                        ip,
                        69,
                        port,
                        &packet,
                    ));
                }
            };
            let mut tid = self.next_port;
            while self.transfers.contains_key(&tid) {
                tid = if tid == 60000 { 20000 } else { tid + 1 }
            }
            self.next_port = if tid == 60000 { 20000 } else { tid + 1 };
            let out = datagram(
                self.server_mac,
                client_mac,
                server,
                ip,
                tid,
                port,
                &session.packet,
            );
            self.transfers.insert(
                tid,
                Transfer {
                    ip,
                    port,
                    request: data.to_vec(),
                    mac: client_mac,
                    session,
                },
            );
            return Some(out);
        }
        let transfer = self.transfers.get_mut(&destination)?;
        if transfer.ip != ip || transfer.port != port || transfer.mac != client_mac {
            return Some(datagram(
                self.server_mac,
                client_mac,
                server,
                ip,
                destination,
                port,
                &tftp::error(5, "Unknown transfer ID"),
            ));
        }
        match transfer.session.acknowledge(data) {
            Ok(tftp::Ack::Next) => {
                self.pending_frames.extend(
                    transfer.session.packets().skip(1).map(|packet| {
                        datagram(self.server_mac, client_mac, server, ip, destination, port, packet)
                    }),
                );
                Some(datagram(
                    self.server_mac,
                    client_mac,
                    server,
                    ip,
                    destination,
                    port,
                    &transfer.session.packet,
                ))
            }
            Ok(tftp::Ack::Ignored) => None,
            Ok(tftp::Ack::Done) => {
                self.transfers.remove(&destination);
                self.state
                    .lock()
                    .unwrap()
                    .event(&ip.to_string(), "TFTP", "Completed transfer");
                None
            }
            Err(e) => {
                self.transfers.remove(&destination);
                self.state.lock().unwrap().event(&ip.to_string(), "TFTP", e);
                Some(datagram(
                    self.server_mac,
                    client_mac,
                    server,
                    ip,
                    destination,
                    port,
                    &tftp::error(0, "Transfer failed"),
                ))
            }
        }
    }
    pub fn tick(&mut self) -> Vec<Vec<u8>> {
        let server = self.state.lock().unwrap().config.settings.server_ip;
        let mut out = std::mem::take(&mut self.pending_frames);
        out.extend(self.last.tick());
        let mut expired = vec![];
        for (port, t) in &mut self.transfers {
            if t.session.expired() {
                expired.push(*port);
                self.state.lock().unwrap().event(
                    &t.ip.to_string(),
                    "TFTP",
                    t.session.upload_summary(),
                );
                continue;
            }
            match t.session.retry() {
                Ok(true) => out.extend(t.session.packets().map(|packet| datagram(
                    self.server_mac,
                    t.mac,
                    server,
                    t.ip,
                    *port,
                    t.port,
                    packet,
                ))),
                Ok(false) => {}
                Err(e) => {
                    expired.push(*port);
                    self.state
                        .lock()
                        .unwrap()
                        .event(&t.ip.to_string(), "TFTP", e)
                }
            }
        }
        for port in expired {
            self.transfers.remove(&port);
        }
        out
    }
    fn dhcp(&self, frame: &[u8]) -> Option<Vec<u8>> {
        let (source_ip, source_port, port, payload) = udp(frame)?;
        let settings = self.state.lock().unwrap().config.settings.clone();
        if frame[30..34] != settings.server_ip.octets() && frame[..6] != [255; 6] {
            return None;
        }
        let boot_port = port == settings.pxe_port || (self.virtual_wire && port == 4011);
        let request = dhcp::Request::parse(payload)?;
        if boot_port && request.relay().is_unspecified() && source_ip != request.current() {
            return None;
        }
        let response = dhcp::reply(&self.state, &request, boot_port)?;
        let peer = std::net::SocketAddr::from((source_ip, source_port));
        let target = response.destination(&request, peer, false);
        let std::net::IpAddr::V4(destination) = target.ip() else {
            return None;
        };
        let destination_mac = if destination.is_broadcast() {
            [255; 6]
        } else {
            frame[6..12].try_into().ok()?
        };
        Some(datagram(
            self.server_mac,
            destination_mac,
            settings.server_ip,
            destination,
            port,
            target.port(),
            &response.bytes,
        ))
    }
    fn rarp(&self, frame: &[u8]) -> Option<Vec<u8>> {
        if frame.len() < 42 || frame[14..22] != [0, 1, 8, 0, 6, 4, 0, 3] {
            return None;
        }
        let address = mac(&frame[32..38]);
        let mut s = self.state.lock().unwrap();
        let ip = s.config.devices.iter().find(|d| d.mac == address)?.ip?;
        let mut response = Vec::from(&frame[..42]);
        response[..6].copy_from_slice(&frame[6..12]);
        response[6..12].copy_from_slice(&self.server_mac);
        response[20..22].copy_from_slice(&4u16.to_be_bytes());
        response[22..28].copy_from_slice(&self.server_mac);
        response[28..32].copy_from_slice(&s.config.settings.server_ip.octets());
        response[38..42].copy_from_slice(&ip.octets());
        response.resize(60, 0);
        s.event(&address, "RARP", ip.to_string());
        Some(response)
    }
    fn rmp(&mut self, frame: &[u8]) -> Option<Vec<u8>> {
        let length = u16::from_be_bytes([frame[12], frame[13]]) as usize;
        let data = frame.get(24..14 + length)?;
        let address = mac(&frame[6..12]);
        let profile = {
            let mut s = self.state.lock().unwrap();
            let i = if let Some(i) = s.config.devices.iter().position(|d| d.mac == address) {
                i
            } else {
                s.config.devices.push(Device {
                    id: address.replace(':', ""),
                    mac: address.clone(),
                    ..Default::default()
                });
                s.config.devices.len() - 1
            };
            let d = &mut s.config.devices[i];
            d.last_seen = now();
            d.protocol = "RMP".into();
            if data.first() == Some(&1) && data.len() >= 31 {
                let model = String::from_utf8_lossy(&data[10..30])
                    .trim_matches(char::from(0))
                    .trim()
                    .to_string();
                if d.model.is_empty() {
                    d.model = model
                }
                if d.architecture == "unknown" || d.architecture.is_empty() {
                    d.architecture = "hp-rmp".into()
                }
            }
            let id = d
                .profile_id
                .clone()
                .or_else(|| s.config.default_profiles.get("hp-rmp").cloned());
            s.config
                .profiles
                .iter()
                .find(|p| Some(&p.id) == id.as_ref())
                .cloned()
        }?;
        let default = profile
            .boot_files
            .get("rmp")
            .or_else(|| profile.boot_files.get("default"))
            .unwrap_or(&profile.kernel);
        let payload = match *data.first()? {
            1 if data.len() >= 31 => {
                let sequence = u32::from_be_bytes(data[2..6].try_into().ok()?);
                let session = u16::from_be_bytes([data[6], data[7]]);
                if data[8..10] != [0, 2] {
                    return None;
                }
                let (status, sid, name) = if session == 65535 {
                    if sequence == 0 {
                        (0, 0, "Netboot".to_string())
                    } else if sequence == 1 {
                        (0, 0, default.clone())
                    } else {
                        (18, 0, String::new())
                    }
                } else {
                    let n = data[30] as usize;
                    let requested = std::str::from_utf8(data.get(31..31 + n)?).ok()?;
                    let name = if requested.is_empty() {
                        default.clone()
                    } else {
                        requested.to_string()
                    };
                    let file = safe_file(&profile.root, &name)
                        .and_then(|p| std::fs::File::open(p).map_err(|e| e.to_string()));
                    match file {
                        Ok(file) => {
                            let sid = self.next_session;
                            self.next_session = if self.next_session >= 65534 {
                                1
                            } else {
                                self.next_session + 1
                            };
                            self.sessions
                                .retain(|_, s| now().saturating_sub(s.last_seen) < 600);
                            if self.sessions.len() >= 128 {
                                return None;
                            }
                            self.sessions.insert(
                                address.clone(),
                                Session {
                                    file,
                                    id: sid,
                                    last_seen: now(),
                                },
                            );
                            self.state.lock().unwrap().event(
                                &address,
                                "RMP",
                                format!("Boot {name}"),
                            );
                            (0, sid, name)
                        }
                        Err(_) => (16, 0, name),
                    }
                };
                let mut p = vec![129, status];
                p.extend(&data[2..6]);
                p.extend(sid.to_be_bytes());
                p.extend(2u16.to_be_bytes());
                let name = name.as_bytes();
                let n = name.len().min(255);
                p.push(n as u8);
                p.extend(&name[..n]);
                p
            }
            2 if data.len() >= 10 => {
                let session = self.sessions.get_mut(&address)?;
                let sid = u16::from_be_bytes([data[6], data[7]]);
                let offset = u32::from_be_bytes(data[2..6].try_into().ok()?);
                let mut bytes = vec![0; u16::from_be_bytes([data[8], data[9]]).min(1482) as usize];
                let (status, n) = if sid != session.id {
                    (25, 0)
                } else {
                    session.last_seen = now();
                    match session
                        .file
                        .seek(SeekFrom::Start(offset as u64))
                        .and_then(|_| session.file.read(&mut bytes))
                    {
                        Ok(0) => (2, 0),
                        Ok(n) => (0, n),
                        Err(_) => (3, 0),
                    }
                };
                let mut p = vec![130, status];
                p.extend(&data[2..8]);
                p.extend(&bytes[..n]);
                p.push(0);
                p
            }
            3 => {
                self.sessions.remove(&address);
                self.state
                    .lock()
                    .unwrap()
                    .event(&address, "RMP", "Boot completed");
                return None;
            }
            _ => return None,
        };
        let mut out = Vec::from(&frame[..24]);
        out[..6].copy_from_slice(&frame[6..12]);
        out[6..12].copy_from_slice(&self.server_mac);
        out[20..24].copy_from_slice(&[6, 9, 6, 8]);
        out[12..14].copy_from_slice(&((10 + payload.len()) as u16).to_be_bytes());
        out.extend(payload);
        if out.len() < 60 {
            out.resize(60, 0)
        }
        Some(out)
    }
}
#[cfg(target_os = "macos")]
pub fn capture_environment_ready() -> bool {
    if unsafe { Library::new("/usr/lib/libpcap.A.dylib") }.is_err() {
        return false;
    }
    let Ok(devices) = std::fs::read_dir("/dev") else {
        return false;
    };
    devices.flatten().any(|device| {
        let name = device.file_name();
        let name = name.to_string_lossy();
        let Some(number) = name.strip_prefix("bpf") else { return false };
        if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
            return false;
        }
        let Ok(path) = CString::new(device.path().as_os_str().as_encoded_bytes()) else {
            return false;
        };
        unsafe { libc::access(path.as_ptr(), libc::R_OK | libc::W_OK) == 0 }
    })
}

pub fn check_capture_permission(interface: &str) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        unsafe{Library::new("wpcap.dll")}.map_err(|_|"Install Npcap from https://npcap.com/#download with WinPcap API-compatible mode enabled, then restart Bootah.")?;
    }
    if interface.is_empty() {
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    return capture::Capture::open_local(interface).map(|_| ());
    #[cfg(not(target_os = "macos"))]
    Capture::probe(interface, false).map(|_| ())
}
pub async fn start(
    state: Shared,
    stop: Arc<AtomicBool>,
) -> Result<tokio::task::JoinHandle<()>, String> {
    let settings = state.lock().unwrap().config.settings.clone();
    #[cfg(target_os = "macos")]
    let settings = {
        let mut settings = settings;
        settings.server_mac = crate::network::interface_mac(&settings.interface)
            .ok_or("Cannot read the selected interface MAC address")?;
        settings
    };
    let interface = settings.interface;
    let mac = normalize_mac(&settings.server_mac).ok_or("Set the interface MAC address")?;
    let mut server_mac = [0; 6];
    for (i, b) in mac.split(':').enumerate() {
        server_mac[i] = u8::from_str_radix(b, 16).unwrap()
    }
    if interface.is_empty() {
        return Err("Select an Ethernet interface".into());
    }
    let (tx, rx) = tokio::sync::oneshot::channel();
    let worker = tokio::task::spawn_blocking(move || {
        let capture = match Capture::open(&interface) {
            Ok(c) => c,
            Err(e) => {
                let _ = tx.send(Err(e));
                return;
            }
        };
        let _ = tx.send(Ok(()));
        let mut ethernet = Ethernet::new(state.clone(), server_mac, false);
        while !stop.load(Ordering::Relaxed) {
            match capture.receive() {
                Ok(Some(frame)) => {
                    if let Some(response) = ethernet.handle(&frame)
                        && let Err(e) = capture.send(&response)
                    {
                        state.lock().unwrap().event("server", "Ethernet", e)
                    }
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(2)),
                Err(e) => {
                    state.lock().unwrap().event("server", "Ethernet", e);
                    break;
                }
            }
            for frame in ethernet.tick() {
                let _ = capture.send(&frame);
            }
        }
    });
    rx.await.map_err(|e| e.to_string())??;
    Ok(worker)
}

fn udp(frame: &[u8]) -> Option<(std::net::Ipv4Addr, u16, u16, &[u8])> {
    if frame.len() < 42 || frame[12..14] != [8, 0] {
        return None;
    }
    let ip = &frame[14..];
    if ip[0] >> 4 != 4 || ip[9] != 17 || (u16::from_be_bytes([ip[6], ip[7]]) & 0x3fff) != 0 {
        return None;
    }
    let ihl = (ip[0] & 15) as usize * 4;
    if ihl < 20 {
        return None;
    }
    let udp = ip.get(ihl..)?;
    if udp.len() < 8 {
        return None;
    }
    let size = u16::from_be_bytes([udp[4], udp[5]]) as usize;
    Some((
        std::net::Ipv4Addr::new(ip[12], ip[13], ip[14], ip[15]),
        u16::from_be_bytes([udp[0], udp[1]]),
        u16::from_be_bytes([udp[2], udp[3]]),
        udp.get(8..size)?,
    ))
}
fn datagram(
    source_mac: [u8; 6],
    destination_mac: [u8; 6],
    source: std::net::Ipv4Addr,
    destination: std::net::Ipv4Addr,
    source_port: u16,
    destination_port: u16,
    data: &[u8],
) -> Vec<u8> {
    let mut out = vec![0; 42];
    out[..6].copy_from_slice(&destination_mac);
    out[6..12].copy_from_slice(&source_mac);
    out[12..14].copy_from_slice(&[8, 0]);
    out[14] = 0x45;
    out[16..18].copy_from_slice(&((data.len() + 28) as u16).to_be_bytes());
    out[22] = 64;
    out[23] = 17;
    out[26..30].copy_from_slice(&source.octets());
    out[30..34].copy_from_slice(&destination.octets());
    let sum = out[14..34]
        .chunks_exact(2)
        .map(|b| u16::from_be_bytes([b[0], b[1]]) as u32)
        .sum::<u32>();
    let sum = (sum & 65535) + (sum >> 16);
    let sum = (sum & 65535) + (sum >> 16);
    out[24..26].copy_from_slice(&(!(sum as u16)).to_be_bytes());
    out[34..36].copy_from_slice(&source_port.to_be_bytes());
    out[36..38].copy_from_slice(&destination_port.to_be_bytes());
    out[38..40].copy_from_slice(&((data.len() + 8) as u16).to_be_bytes());
    out.extend(data);
    if out.len() < 60 {
        out.resize(60, 0)
    }
    out
}
pub async fn wire(state: Shared, address: std::net::SocketAddr) -> Result<(), String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    if !address.ip().is_loopback() {
        return Err("Emulator wire must bind to loopback".into());
    }
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .map_err(|e| e.to_string())?;
    loop {
        let (stream, _) = listener.accept().await.map_err(|e| e.to_string())?;
        stream.set_nodelay(true).map_err(|e| e.to_string())?;
        let state = state.clone();
        tokio::spawn(async move {
            let (mut reader, mut writer) = stream.into_split();
            let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(64);
            let receive = tokio::spawn(async move {
                loop {
                    let size = match reader.read_u32().await {
                        Ok(n) if n > 0 && n <= 65536 => n as usize,
                        _ => break,
                    };
                    let mut frame = vec![0; size];
                    if reader.read_exact(&mut frame).await.is_err() {
                        break;
                    }
                    if tx.send(frame).await.is_err() {
                        break;
                    }
                }
            });
            let mut rpc = crate::nfs2::Server::new(state.clone());
            let rpc_state = state.clone();
            let mut ethernet = Ethernet::new(state, [2, 0, 0, 0, 0, 1], true);
            let mut timer = tokio::time::interval(Duration::from_millis(100));
            loop {
                let frames = tokio::select! {
                    Some(frame)=rx.recv()=>{
                        let mut reply=None;
                        if let Some((ip,port,destination,data))=udp(&frame) {
                            let (server,assigned)={let store=rpc_state.lock().unwrap();(store.config.settings.server_ip,store.profile_for(ip).is_some_and(|p|crate::nfs::eligible(&p)))};
                            if assigned && (frame.get(30..34)==Some(server.octets().as_slice()) || frame.get(30..34)==Some(&[255,255,255,255])) {
                                let payload=match destination {
                                    111=>crate::bootparams::callit_options(&rpc_state,ip,data,"vers=2,proto=udp").or_else(||crate::nfs::wire_portmap(data)),
                                    2049=>rpc.handle(ip,data).await,
                                    crate::nfs::WIRE_BOOTPARAM_PORT=>crate::bootparams::reply_options(&rpc_state,ip,data,"vers=2,proto=udp"),
                                    _=>None,
                                };
                                if let Some(payload)=payload {let mut mac=[0;6];mac.copy_from_slice(&frame[6..12]);reply=Some(datagram([2,0,0,0,0,1],mac,server,ip,destination,port,&payload));}
                            }
                        }
                        let mut responses=reply.or_else(||ethernet.handle(&frame)).into_iter().collect::<Vec<_>>();
                        responses.append(&mut ethernet.pending_frames);
                        responses
                    },
                    _=timer.tick()=>ethernet.tick(),else=>break
                };
                if rx.is_closed() && rx.is_empty() {
                    break;
                }
                for frame in frames.into_iter().flat_map(fragment) {
                    if writer.write_u32(frame.len() as u32).await.is_err()
                        || writer.write_all(&frame).await.is_err()
                    {
                        receive.abort();
                        return;
                    }
                }
            }
            receive.abort();
        });
    }
}

#[cfg(target_os = "linux")]
pub fn open_packet(interface: &str) -> Result<std::os::fd::OwnedFd, String> {
    capture::open_packet(interface)
}

// IPv4 fragments keep the emulated Ethernet link at its normal 1500-byte MTU.
fn fragment(frame: Vec<u8>) -> Vec<Vec<u8>> {
    if frame.len() <= 1514 || frame.get(12..15) != Some(&[8, 0, 0x45]) {
        return vec![frame];
    }
    static NEXT: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(1);
    let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let length = u16::from_be_bytes([frame[16], frame[17]]) as usize;
    if length < 20 || 14 + length > frame.len() {
        return vec![];
    }
    let payload = &frame[34..14 + length];
    let mut out = Vec::new();
    for (index, chunk) in payload.chunks(1480).enumerate() {
        let mut part = frame[..34].to_vec();
        let offset = index * 1480;
        part[16..18].copy_from_slice(&((20 + chunk.len()) as u16).to_be_bytes());
        part[18..20].copy_from_slice(&id.to_be_bytes());
        let flags = (offset / 8) as u16
            | if offset + chunk.len() < payload.len() {
                0x2000
            } else {
                0
            };
        part[20..22].copy_from_slice(&flags.to_be_bytes());
        part[24..26].fill(0);
        let mut sum = part[14..34]
            .chunks_exact(2)
            .map(|b| u16::from_be_bytes([b[0], b[1]]) as u32)
            .sum::<u32>();
        sum = (sum & 65535) + (sum >> 16);
        sum = (sum & 65535) + (sum >> 16);
        part[24..26].copy_from_slice(&(!(sum as u16)).to_be_bytes());
        part.extend_from_slice(chunk);
        part.resize(part.len().max(60), 0);
        out.push(part);
    }
    out
}

pub fn detect_dhcp(
    interface: &str,
    mac: &str,
    ip: Option<std::net::Ipv4Addr>,
    authorize: bool,
) -> Result<Vec<std::net::Ipv4Addr>, String> {
    let normalized = normalize_mac(mac).ok_or("Interface hardware address unavailable")?;
    let bytes: Vec<_> = normalized
        .split(':')
        .map(|b| u8::from_str_radix(b, 16).unwrap())
        .collect();
    let mac: [u8; 6] = bytes.try_into().unwrap();
    if mac == [0; 6] || mac == [2, 0, 0, 0, 0, 0] {
        return Err("Interface hardware address unavailable".into());
    }
    let capture = Capture::probe(interface, authorize)?;
    probe_exchange(mac, ip, |frame| capture.send(frame), || capture.receive())
}

fn probe_exchange(
    mac: [u8; 6],
    ip: Option<std::net::Ipv4Addr>,
    mut send: impl FnMut(&[u8]) -> Result<(), String>,
    mut receive: impl FnMut() -> Result<Option<Vec<u8>>, String>,
) -> Result<Vec<std::net::Ipv4Addr>, String> {
    use std::{net::Ipv4Addr, time::Instant};
    let xid: [u8; 4] = uuid::Uuid::new_v4().as_bytes()[..4].try_into().unwrap();
    let mut packet = vec![0; 240];
    packet[..4].copy_from_slice(&[1, 1, 6, 0]);
    packet[4..8].copy_from_slice(&xid);
    packet[28..34].copy_from_slice(&mac);
    packet[236..240].copy_from_slice(&[99, 130, 83, 99]);
    if let Some(ip) = ip {
        packet[12..16].copy_from_slice(&ip.octets());
    } else {
        packet[10] = 0x80;
    }
    packet.extend_from_slice(&[
        53,
        1,
        if ip.is_some() { 8 } else { 1 },
        55,
        3,
        1,
        3,
        54,
        255,
    ]);
    packet.resize(300, 0);
    let mut servers = vec![];
    let begin = Instant::now();
    let mut sent = 0;
    while begin.elapsed() < Duration::from_secs(5) {
        if sent == 0 || (sent == 1 && begin.elapsed() >= Duration::from_secs(2)) {
            packet[8..10].copy_from_slice(&(begin.elapsed().as_secs() as u16).to_be_bytes());
            send(&datagram(
                mac,
                [255; 6],
                ip.unwrap_or(Ipv4Addr::UNSPECIFIED),
                Ipv4Addr::BROADCAST,
                68,
                67,
                &packet,
            ))?;
            sent += 1;
        }
        if let Some(frame) = receive()? {
            if let Some(server) = probe_reply(&frame, xid, mac, ip.is_some())
                && !servers.contains(&server)
            {
                servers.push(server);
            }
        } else {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    Ok(servers)
}
fn probe_reply(
    frame: &[u8],
    xid: [u8; 4],
    mac: [u8; 6],
    inform: bool,
) -> Option<std::net::Ipv4Addr> {
    let (_, source, destination, bytes) = udp(frame)?;
    if source != 67
        || destination != 68
        || bytes.len() < 240
        || bytes[0] != 2
        || bytes[4..8] != xid
        || bytes[28..34] != mac
        || bytes[236..240] != [99, 130, 83, 99]
    {
        return None;
    }
    let mut bytes = bytes.to_vec();
    bytes[0] = 1;
    let response = dhcp::Request::parse(&bytes)?;
    if response.message != if inform { 5 } else { 2 } {
        return None;
    }
    if !inform && bytes[16..20] == [0; 4] {
        return None;
    }
    response.server.filter(|ip| crate::network::usable(*ip))
}

#[cfg(test)]
mod fragment_tests {
    use super::*;
    #[test]
    fn observed_arp_rebinds_known_client_before_http() {
        let path = std::env::temp_dir().join(format!(
            "bootah-arp-rebind-{}.json",
            uuid::Uuid::new_v4()
        ));
        let state = Store::open(path.clone()).unwrap();
        let mac = [0x52, 0x54, 0, 0x12, 0x34, 0x65];
        {
            let mut store = state.lock().unwrap();
            store.config.devices.push(Device {
                id: "525400123465".into(),
                mac: "52:54:00:12:34:65".into(),
                ip: Some("192.0.2.123".parse().unwrap()),
                ..Default::default()
            });
            store.config.devices.push(Device {
                id: "c435d9823223".into(),
                mac: "c4:35:d9:82:32:23".into(),
                ip: Some("192.0.2.125".parse().unwrap()),
                auto_registered: true,
                ..Default::default()
            });
            store.config.settings.server_ip = "192.0.2.191".parse().unwrap();
        }
        let mut ethernet = Ethernet::new(state.clone(), [2; 6], false);
        ethernet.host_mac = Some("c4:35:d9:82:32:23".into());
        let mut frame = [0u8; 42];
        frame[..6].fill(0xff);
        frame[6..12].copy_from_slice(&mac);
        frame[12..22].copy_from_slice(&[8, 6, 0, 1, 8, 0, 6, 4, 0, 1]);
        frame[22..28].copy_from_slice(&mac);
        frame[28..32].copy_from_slice(&[192, 0, 2, 124]);
        frame[38..42].copy_from_slice(&[192, 0, 2, 191]);
        assert!(ethernet.handle(&frame).is_none());
        assert_eq!(state.lock().unwrap().config.devices[0].ip, Some("192.0.2.124".parse().unwrap()));
        frame[22] = 0;
        frame[28..32].copy_from_slice(&[192, 0, 2, 125]);
        ethernet.handle(&frame);
        assert_eq!(state.lock().unwrap().config.devices[0].ip, Some("192.0.2.124".parse().unwrap()));
        assert_eq!(state.lock().unwrap().config.devices[1].ip, Some("192.0.2.125".parse().unwrap()));
        frame[6..12].copy_from_slice(&[0xc4, 0x35, 0xd9, 0x82, 0x32, 0x23]);
        frame[22..28].copy_from_slice(&[0xc4, 0x35, 0xd9, 0x82, 0x32, 0x23]);
        frame[28..32].copy_from_slice(&[192, 0, 2, 126]);
        ethernet.handle(&frame);
        assert_eq!(state.lock().unwrap().config.devices[0].ip, Some("192.0.2.124".parse().unwrap()));
        assert_eq!(state.lock().unwrap().config.devices[1].ip, Some("192.0.2.125".parse().unwrap()));
        drop(ethernet);
        drop(state);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("events.jsonl"));
    }
    #[test]
    fn discovery_rejects_unrelated_or_proxy_offers() {
        let xid = [1, 2, 3, 4];
        let mac = [2, 3, 4, 5, 6, 7];
        let mut data = vec![0; 240];
        data[..4].copy_from_slice(&[2, 1, 6, 0]);
        data[4..8].copy_from_slice(&xid);
        data[28..34].copy_from_slice(&mac);
        data[236..240].copy_from_slice(&[99, 130, 83, 99]);
        data.extend_from_slice(&[53, 1, 2, 54, 4, 192, 0, 2, 1, 255]);
        let frame = |data: &[u8]| {
            datagram(
                [4; 6],
                mac,
                "192.0.2.1".parse().unwrap(),
                std::net::Ipv4Addr::BROADCAST,
                67,
                68,
                data,
            )
        };
        assert!(probe_reply(&frame(&data), xid, mac, false).is_none());
        data[16..20].copy_from_slice(&[192, 0, 2, 100]);
        assert_eq!(
            probe_reply(&frame(&data), xid, mac, false),
            Some("192.0.2.1".parse().unwrap())
        );
        assert!(probe_reply(&frame(&data), [0; 4], mac, false).is_none());
        assert!(probe_reply(&frame(&data), xid, [0; 6], false).is_none());
        assert!(probe_reply(&frame(&data), xid, mac, true).is_none());
        data.truncate(248);
        assert!(probe_reply(&frame(&data), xid, mac, false).is_none());
    }
    #[test]
    fn reassembles_nfs_sized_ipv4_datagram() {
        let data: Vec<u8> = (0..8192).map(|n| n as u8).collect();
        let original = datagram(
            [2; 6],
            [4; 6],
            "192.0.2.1".parse().unwrap(),
            "192.0.2.2".parse().unwrap(),
            2049,
            900,
            &data,
        );
        let pieces = fragment(original.clone());
        assert!(pieces.len() > 1);
        let mut payload = vec![0; data.len() + 8];
        for (index, part) in pieces.iter().enumerate() {
            assert!(part.len() <= 1514);
            assert_eq!(&part[..14], &original[..14]);
            assert_eq!(&part[18..20], &pieces[0][18..20]);
            let mut sum = part[14..34]
                .chunks_exact(2)
                .map(|b| u16::from_be_bytes([b[0], b[1]]) as u32)
                .sum::<u32>();
            sum = (sum & 65535) + (sum >> 16);
            sum = (sum & 65535) + (sum >> 16);
            assert_eq!(sum, 65535);
            let flags = u16::from_be_bytes([part[20], part[21]]);
            let offset = ((flags & 0x1fff) as usize) * 8;
            let length = u16::from_be_bytes([part[16], part[17]]) as usize - 20;
            assert_eq!(flags & 0x2000 != 0, index + 1 < pieces.len());
            payload[offset..offset + length].copy_from_slice(&part[34..34 + length]);
        }
        assert_eq!(payload, &original[34..]);
        assert_eq!(&payload[8..], data);
    }
}
