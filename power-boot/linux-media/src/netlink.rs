use std::{
    io,
    os::fd::{AsRawFd, FromRawFd},
    time::Duration,
};
fn failure() -> String {
    io::Error::last_os_error().to_string()
}
fn align(n: usize) -> usize {
    (n + 3) & !3
}
fn attr(out: &mut Vec<u8>, kind: u16, value: &[u8]) {
    out.extend_from_slice(&((value.len() + 4) as u16).to_ne_bytes());
    out.extend_from_slice(&kind.to_ne_bytes());
    out.extend_from_slice(value);
    out.resize(align(out.len()), 0);
}
fn attributes(data: &[u8]) -> Result<Vec<(u16, &[u8])>, String> {
    let mut result = Vec::new();
    let mut offset = 0;
    while offset + 4 <= data.len() {
        let n = u16::from_ne_bytes(data[offset..offset + 2].try_into().unwrap()) as usize;
        if n < 4 || offset + n > data.len() {
            return Err("Malformed netlink attribute".into());
        }
        let kind = u16::from_ne_bytes(data[offset + 2..offset + 4].try_into().unwrap()) & 0x3fff;
        result.push((kind, &data[offset + 4..offset + n]));
        offset += align(n);
    }
    if offset != data.len() {
        return Err("Truncated netlink attributes".into());
    }
    Ok(result)
}
struct Netlink(std::fs::File);
impl Netlink {
    fn open() -> Result<Self, String> {
        let fd = unsafe {
            libc::socket(
                libc::AF_NETLINK,
                libc::SOCK_RAW | libc::SOCK_CLOEXEC,
                libc::NETLINK_GENERIC,
            )
        };
        if fd < 0 {
            return Err(failure());
        }
        let file = unsafe { std::fs::File::from_raw_fd(fd) };
        let mut address: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
        address.nl_family = libc::AF_NETLINK as _;
        if unsafe {
            libc::bind(
                fd,
                (&address as *const libc::sockaddr_nl).cast(),
                std::mem::size_of_val(&address) as _,
            )
        } < 0
        {
            return Err(failure());
        }
        let timeout = libc::timeval {
            tv_sec: 5,
            tv_usec: 0,
        };
        if unsafe {
            libc::setsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_RCVTIMEO,
                (&timeout as *const libc::timeval).cast(),
                std::mem::size_of_val(&timeout) as _,
            )
        } < 0
        {
            return Err(failure());
        }
        Ok(Self(file))
    }
    fn request(
        &self,
        family: u16,
        command: u8,
        version: u8,
        sequence: u32,
        attrs: Vec<u8>,
        expect_reply: bool,
    ) -> Result<Vec<u8>, String> {
        let mut message = Vec::new();
        message.extend_from_slice(&((20 + attrs.len()) as u32).to_ne_bytes());
        message.extend_from_slice(&family.to_ne_bytes());
        message.extend_from_slice(&5u16.to_ne_bytes());
        message.extend_from_slice(&sequence.to_ne_bytes());
        message.extend_from_slice(&0u32.to_ne_bytes());
        message.extend_from_slice(&[command, version, 0, 0]);
        message.extend(attrs);
        let mut address: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
        address.nl_family = libc::AF_NETLINK as _;
        let fd = self.0.as_raw_fd();
        let sent = unsafe {
            libc::sendto(
                fd,
                message.as_ptr().cast(),
                message.len(),
                0,
                (&address as *const libc::sockaddr_nl).cast(),
                std::mem::size_of_val(&address) as _,
            )
        };
        if sent < 0 {
            return Err(failure());
        }
        if sent as usize != message.len() {
            return Err("Truncated netlink send".into());
        }
        let mut acknowledged = false;
        let mut response = None;
        for _ in 0..32 {
            let mut data = [0u8; 65536];
            let mut sender: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
            let mut sender_len = std::mem::size_of_val(&sender) as libc::socklen_t;
            let n = unsafe {
                libc::recvfrom(
                    fd,
                    data.as_mut_ptr().cast(),
                    data.len(),
                    libc::MSG_TRUNC,
                    (&mut sender as *mut libc::sockaddr_nl).cast(),
                    &mut sender_len,
                )
            };
            if n < 0 {
                return Err(failure());
            }
            if n as usize > data.len() || sender.nl_pid != 0 {
                return Err("Invalid netlink sender or oversized reply".into());
            }
            let mut offset = 0;
            let count = n as usize;
            while offset + 16 <= count {
                let len = u32::from_ne_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
                if len < 16 || offset + len > count {
                    return Err("Malformed netlink message".into());
                }
                let kind = u16::from_ne_bytes(data[offset + 4..offset + 6].try_into().unwrap());
                let seq = u32::from_ne_bytes(data[offset + 8..offset + 12].try_into().unwrap());
                if seq == sequence {
                    let payload = &data[offset + 16..offset + len];
                    if kind == 2 {
                        if payload.len() < 4 {
                            return Err("Truncated netlink status".into());
                        }
                        let error = i32::from_ne_bytes(payload[..4].try_into().unwrap());
                        if error != 0 {
                            return Err(format!(
                                "NBD netlink: {}",
                                io::Error::from_raw_os_error(-error)
                            ));
                        }
                        acknowledged = true;
                    } else if kind == family {
                        if payload.len() < 4 {
                            return Err("Truncated generic netlink reply".into());
                        }
                        response = Some(payload[4..].to_vec());
                    }
                }
                offset += align(len);
            }
            if acknowledged && (!expect_reply || response.is_some()) {
                return Ok(response.unwrap_or_default());
            }
        }
        Err("Netlink response limit exceeded".into())
    }
}
pub fn connect(socket: i32, size: u64, flags: u16, device: &str) -> Result<(), String> {
    let number = device
        .strip_prefix("/dev/nbd")
        .ok_or("NBD requires a /dev/nbdN device")?;
    if number.is_empty() || !number.bytes().all(|c| c.is_ascii_digit()) {
        return Err("Invalid NBD device index".into());
    }
    let index: u32 = number.parse().map_err(|_| "Invalid NBD device index")?;
    let netlink = Netlink::open()?;
    let mut lookup = Vec::new();
    attr(&mut lookup, 2, b"nbd\0");
    let family = netlink.request(16, 3, 2, 1, lookup, true).map_err(|e| {
        format!("Kernel NBD generic-netlink unavailable (matching nbd module required): {e}")
    })?;
    let family_id = attributes(&family)?
        .into_iter()
        .find(|(kind, _)| *kind == 1)
        .and_then(|(_, value)| value.try_into().ok())
        .map(u16::from_ne_bytes)
        .ok_or("NBD family ID missing")?;
    let mut setup = Vec::new();
    attr(&mut setup, 1, &index.to_ne_bytes());
    attr(&mut setup, 2, &size.to_ne_bytes());
    attr(&mut setup, 3, &512u64.to_ne_bytes());
    attr(&mut setup, 5, &u64::from(flags).to_ne_bytes());
    // No DISCONNECT_ON_CLOSE client flag: the kernel owns this storage connection.
    attr(&mut setup, 6, &0u64.to_ne_bytes());
    let mut socket_attr = Vec::new();
    attr(&mut socket_attr, 1, &(socket as u32).to_ne_bytes());
    let mut items = Vec::new();
    attr(&mut items, 1 | 0x8000, &socket_attr);
    attr(&mut setup, 7 | 0x8000, &items);
    let connected = netlink.request(family_id, 1, 1, 2, setup, true)?;
    let actual = attributes(&connected)?
        .into_iter()
        .find(|(kind, _)| *kind == 1)
        .and_then(|(_, value)| value.try_into().ok())
        .map(u32::from_ne_bytes)
        .ok_or("NBD connection reply lacks index")?;
    if actual != index {
        return Err("Kernel connected an unexpected NBD index".into());
    }
    for _ in 0..100 {
        if std::fs::read_to_string(format!("/sys/block/nbd{index}/size"))
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            == Some(size / 512)
        {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let mut disconnect = Vec::new();
    attr(&mut disconnect, 1, &index.to_ne_bytes());
    let _ = netlink.request(family_id, 2, 1, 3, disconnect, false);
    Err("Netlink NBD device did not report the negotiated size".into())
}
