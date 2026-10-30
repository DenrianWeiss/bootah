use std::{
    ffi::c_void,
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::net::{UnixListener, UnixStream},
    },
    process::{Command, Stdio},
    time::{Duration, Instant},
};

pub const GUIDANCE: &str = "Open Network permissions in Bootah and enable its bundled network helper in macOS System Settings. Alternatively, install ChmodBPF from the official Wireshark macOS disk image: https://www.wireshark.org/download.html . Then retry.";
pub const PORT_GUIDANCE: &str = "Enable the Bootah network helper in macOS System Settings to allow the required network ports.";
pub const BPF_GUIDANCE: &str = "Network capture requires BPF access. Install ChmodBPF from the official Wireshark macOS disk image: https://www.wireshark.org/download.html . Then restart Bootah.";
#[derive(Clone, serde::Serialize)]
pub struct Availability {
    pub helper_available: bool,
    pub reason: String,
}
fn assessment_result(signature: bool, accepted: bool, output: &str) -> bool {
    signature
        && accepted
        && output
            .lines()
            .filter_map(|line| line.strip_prefix("source="))
            .last()
            == Some("Notarized Developer ID")
}
fn system_check(
    program: &str,
    args: &[&str],
    path: &std::path::Path,
) -> Result<(bool, String), String> {
    let mut child = Command::new(program)
        .env_clear()
        .env("LC_ALL", "C")
        .args(args)
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let stderr = child.stderr.take().unwrap();
    let reader = std::thread::spawn(move || {
        let mut output = String::new();
        let _ = stderr.take(65536).read_to_string(&mut output);
        output
    });
    let start = Instant::now();
    let result = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status.success()),
            Ok(None) if start.elapsed() < Duration::from_secs(15) => {
                std::thread::sleep(Duration::from_millis(30))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break Err("Signature assessment unavailable".to_string());
            }
        }
    };
    let output = reader.join().unwrap_or_default();
    result.map(|success| (success, output))
}
fn assess() -> Availability {
    let fallback = |reason: &str| Availability {
        helper_available: false,
        reason: reason.into(),
    };
    let Ok(executable) = std::env::current_exe().and_then(|p| p.canonicalize()) else {
        return fallback("unavailable");
    };
    let Some(macos) = executable.parent() else {
        return fallback("unbundled");
    };
    let Some(contents) = macos.parent() else {
        return fallback("unbundled");
    };
    let Some(bundle) = contents.parent() else {
        return fallback("unbundled");
    };
    if macos.file_name() != Some(std::ffi::OsStr::new("MacOS"))
        || contents.file_name() != Some(std::ffi::OsStr::new("Contents"))
        || bundle.extension() != Some(std::ffi::OsStr::new("app"))
    {
        return fallback("unbundled");
    }
    match system_check(
        "/usr/bin/codesign",
        &["--verify", "--deep", "--strict"],
        bundle,
    ) {
        Ok((true, _)) => {}
        Ok(_) => return fallback("invalid_signature"),
        Err(_) => return fallback("unavailable"),
    }
    match system_check(
        "/usr/sbin/spctl",
        &["--assess", "--type", "execute", "--verbose=2"],
        bundle,
    ) {
        Ok((accepted, output)) if assessment_result(true, accepted, &output) => Availability {
            helper_available: true,
            reason: "notarized".into(),
        },
        Ok(_) => fallback("notarization_unconfirmed"),
        Err(_) => fallback("unavailable"),
    }
}
fn cached_availability() -> &'static std::sync::Mutex<Option<Availability>> {
    static RESULT: std::sync::OnceLock<std::sync::Mutex<Option<Availability>>> =
        std::sync::OnceLock::new();
    RESULT.get_or_init(|| std::sync::Mutex::new(None))
}
pub fn availability() -> Availability {
    let mut result = cached_availability().lock().unwrap();
    result.get_or_insert_with(assess).clone()
}
pub fn refresh_availability() -> Availability {
    let mut result = cached_availability().lock().unwrap();
    let value = assess();
    *result = Some(value.clone());
    value
}
fn require_notarization() -> Result<(), String> {
    if assess().helper_available {
        Ok(())
    } else {
        Err(BPF_GUIDANCE.into())
    }
}
const LIMIT: usize = 65536;
type CFRef = *const c_void;
#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    static kSecGuestAttributeAudit: CFRef;
    fn SecCodeCopySelf(flags: u32, code: *mut CFRef) -> i32;
    fn SecCodeCopyDesignatedRequirement(code: CFRef, flags: u32, requirement: *mut CFRef) -> i32;
    fn SecCodeCopyGuestWithAttributes(
        host: CFRef,
        attributes: CFRef,
        flags: u32,
        code: *mut CFRef,
    ) -> i32;
    fn SecCodeCheckValidity(code: CFRef, flags: u32, requirement: CFRef) -> i32;
}
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFDataCreate(allocator: CFRef, bytes: *const u8, length: isize) -> CFRef;
    fn CFDictionaryCreate(
        allocator: CFRef,
        keys: *const CFRef,
        values: *const CFRef,
        count: isize,
        key_callbacks: CFRef,
        value_callbacks: CFRef,
    ) -> CFRef;
    fn CFRelease(object: CFRef);
}
struct Object(CFRef);
impl Drop for Object {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0) }
        }
    }
}
fn code_object(call: impl FnOnce(*mut CFRef) -> i32) -> Result<Object, String> {
    let mut value = std::ptr::null();
    let status = call(&mut value);
    if status != 0 {
        return Err(format!(
            "Network helper signature check failed ({status}). Use a signed Bootah package."
        ));
    }
    Ok(Object(value))
}

pub(crate) fn authenticate(stream: &UnixStream, uid: u32, pid: Option<u32>) -> Result<(), String> {
    let mut peer_uid = 0;
    let mut peer_gid = 0;
    if unsafe { libc::getpeereid(stream.as_raw_fd(), &mut peer_uid, &mut peer_gid) } != 0
        || peer_uid != uid
    {
        return Err("Network helper peer identity mismatch".into());
    }
    // An audit token identifies the process instance, avoiding PID reuse during signature checks.
    let mut token = [0u32; 8];
    let mut size = std::mem::size_of_val(&token) as libc::socklen_t;
    if unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            0,
            0x006,
            token.as_mut_ptr().cast(),
            &mut size,
        )
    } != 0
        || size as usize != std::mem::size_of_val(&token)
        || pid.is_some_and(|pid| token[5] != pid)
    {
        return Err("Cannot authenticate the network helper connection".into());
    }
    unsafe {
        let own = code_object(|out| SecCodeCopySelf(0, out))?;
        let requirement = code_object(|out| SecCodeCopyDesignatedRequirement(own.0, 0, out))?;
        let data = Object(CFDataCreate(
            std::ptr::null(),
            token.as_ptr().cast(),
            size as isize,
        ));
        let key = kSecGuestAttributeAudit;
        let attributes = Object(CFDictionaryCreate(
            std::ptr::null(),
            &key,
            &data.0,
            1,
            std::ptr::null(),
            std::ptr::null(),
        ));
        let peer = code_object(|out| {
            SecCodeCopyGuestWithAttributes(std::ptr::null(), attributes.0, 0, out)
        })?;
        let status = SecCodeCheckValidity(peer.0, 0, requirement.0);
        if status != 0 {
            return Err(format!("Network helper signature mismatch ({status})"));
        }
    }
    Ok(())
}

pub(crate) fn write_message(mut stream: &UnixStream, kind: u8, data: &[u8]) -> Result<(), String> {
    if data.len() > LIMIT {
        return Err("Network helper message too large".into());
    }
    stream
        .write_all(&[kind])
        .and_then(|_| stream.write_all(&(data.len() as u32).to_be_bytes()))
        .and_then(|_| stream.write_all(data))
        .map_err(|e| e.to_string())
}
pub(crate) fn read_message(mut stream: &UnixStream) -> Result<(u8, Vec<u8>), String> {
    let mut header = [0u8; 5];
    stream.read_exact(&mut header).map_err(|e| e.to_string())?;
    let length = u32::from_be_bytes(header[1..].try_into().unwrap()) as usize;
    if length > LIMIT {
        return Err("Network helper message too large".into());
    }
    let mut data = vec![0; length];
    stream.read_exact(&mut data).map_err(|e| e.to_string())?;
    Ok((header[0], data))
}
fn response(stream: &UnixStream) -> Result<Vec<u8>, String> {
    let (status, data) = read_message(stream)?;
    match status {
        0 => Ok(data),
        1 => Err(String::from_utf8_lossy(&data).into_owned()),
        _ => Err("Invalid network helper response".into()),
    }
}
fn timeout(stream: &UnixStream) -> Result<(), String> {
    stream.set_nonblocking(false).map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .and_then(|_| stream.set_write_timeout(Some(Duration::from_secs(10))))
        .map_err(|e| e.to_string())
}
const SOCKET_PATH: &str = "/var/run/work.premutation.bootah.network.sock";
pub fn authorization_available() -> bool {
    let Ok(stream) = UnixStream::connect(SOCKET_PATH) else {
        return false;
    };
    let mut uid = 0;
    let mut gid = 0;
    unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) == 0 && uid == 0 }
}
pub struct Session {
    stream: UnixStream,
}
impl Session {
    pub fn request(&self, kind: u8, data: &[u8]) -> Result<Vec<u8>, String> {
        write_message(&self.stream, kind, data)?;
        response(&self.stream)
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
    }
}
#[derive(serde::Serialize, serde::Deserialize)]
struct Request {
    kind: String,
    address: String,
}
fn connect(kind: &str, address: &str) -> Result<Session, String> {
    let stream = if availability().helper_available {
        UnixStream::connect(SOCKET_PATH).map_err(|e| format!("{}\n{e}", if kind == "capture" { GUIDANCE } else { PORT_GUIDANCE }))?
    } else {
        crate::macos_elevation::connect()?
    };
    // launchd creates this socket in the root-owned /var/run directory.
    let mut uid = 0;
    let mut gid = 0;
    if unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) } != 0 || uid != 0 {
        return Err("Network helper peer identity mismatch".into());
    }
    timeout(&stream).map_err(|e| format!("Configure network connection: {e}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(40)))
        .map_err(|e| format!("Configure socket response timeout: {e}"))?;
    let request = serde_json::to_vec(&Request {
        kind: kind.into(),
        address: address.into(),
    })
    .map_err(|e| e.to_string())?;
    write_message(&stream, 0, &request)?;
    response(&stream).map_err(|e| format!("Open authorized network socket: {e}"))?;
    timeout(&stream)?;
    Ok(Session { stream })
}
pub fn capture(interface: &str) -> Result<Session, String> {
    connect("capture", interface)
}
pub fn socket(kind: &str, address: &str) -> Result<OwnedFd, String> {
    allowed_address(kind, address)?;
    let session = connect(kind, address)?;
    let fd = transfer(&session.stream, None)?.ok_or("Missing network socket")?;
    write_message(&session.stream, 3, &[])?;
    Ok(fd)
}
pub(crate) fn transfer(stream: &UnixStream, fd: Option<i32>) -> Result<Option<OwnedFd>, String> {
    let mut byte = 0u8;
    let mut vector = libc::iovec {
        iov_base: (&mut byte as *mut u8).cast(),
        iov_len: 1,
    };
    let mut ancillary = [0usize; 8];
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut vector;
    msg.msg_iovlen = 1;
    msg.msg_control = ancillary.as_mut_ptr().cast();
    msg.msg_controllen = unsafe { libc::CMSG_SPACE(4) };
    unsafe {
        if let Some(fd) = fd {
            let c = libc::CMSG_FIRSTHDR(&msg);
            (*c).cmsg_level = libc::SOL_SOCKET;
            (*c).cmsg_type = libc::SCM_RIGHTS;
            (*c).cmsg_len = libc::CMSG_LEN(4);
            std::ptr::write(libc::CMSG_DATA(c).cast::<i32>(), fd);
            if libc::sendmsg(stream.as_raw_fd(), &msg, 0) != 1 {
                return Err(std::io::Error::last_os_error().to_string());
            }
            return Ok(None);
        }
        if libc::recvmsg(stream.as_raw_fd(), &mut msg, 0) != 1 {
            return Err("Missing network socket descriptor".into());
        }
        let c = libc::CMSG_FIRSTHDR(&msg);
        if c.is_null()
            || (*c).cmsg_level != libc::SOL_SOCKET
            || (*c).cmsg_type != libc::SCM_RIGHTS
            || (*c).cmsg_len != libc::CMSG_LEN(4)
            || msg.msg_flags & libc::MSG_CTRUNC != 0
        {
            return Err("Invalid network socket descriptor".into());
        }
        let fd = OwnedFd::from_raw_fd(std::ptr::read(libc::CMSG_DATA(c).cast::<i32>()));
        if libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) < 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok(Some(fd))
    }
}
fn allowed_address(kind: &str, address: &str) -> Result<std::net::SocketAddrV4, String> {
    let address: std::net::SocketAddrV4 = address
        .parse()
        .map_err(|_| "Invalid network helper address")?;
    let allowed = match kind {
        "udp" => matches!(address.port(), 67 | 69 | 111 | 4011),
        "tcp" => matches!(address.port(), 80 | 111 | 139 | 445),
        _ => false,
    };
    if !allowed {
        return Err("Unsupported network helper port".into());
    }
    Ok(address)
}
fn serve(stream: UnixStream) -> Result<(), String> {
    timeout(&stream)?;
    let mut uid = 0;
    let mut gid = 0;
    if unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) } != 0 {
        return Err("Cannot identify network helper client".into());
    }
    authenticate(&stream, uid, None)?;
    serve_authenticated(stream)
}
pub(crate) fn serve_authenticated(stream: UnixStream) -> Result<(), String> {
    timeout(&stream)?;
    let (operation, data) = read_message(&stream)?;
    if operation != 0 {
        return Err("Invalid network helper request".into());
    }
    let request: Request = serde_json::from_slice(&data).map_err(|e| e.to_string())?;
    let result = (|| match request.kind.as_str() {
        "capture" => crate::raw::serve_authorized_capture(&stream, &request.address),
        kind @ ("udp" | "tcp") => {
            let address = allowed_address(kind, &request.address)?;
            let fd: OwnedFd = if kind == "udp" {
                std::net::UdpSocket::bind(address)
                    .map_err(|e| e.to_string())?
                    .into()
            } else {
                std::net::TcpListener::bind(address)
                    .map_err(|e| e.to_string())?
                    .into()
            };
            write_message(&stream, 0, &[])?;
            transfer(&stream, Some(fd.as_raw_fd()))?;
            let (operation, data) = read_message(&stream)?;
            if operation != 3 || !data.is_empty() {
                return Err("Invalid network socket acknowledgement".into());
            }
            Ok(())
        }
        _ => Err("Unknown network helper operation".into()),
    })();
    if let Err(error) = &result {
        let _ = write_message(&stream, 1, error.as_bytes());
    }
    result
}

unsafe extern "C" {
    fn launch_activate_socket(
        name: *const libc::c_char,
        fds: *mut *mut i32,
        count: *mut usize,
    ) -> i32;
}
pub fn daemon() -> Result<(), String> {
    if unsafe { libc::geteuid() } != 0 {
        return Err("Network helper requires system service authorization".into());
    }
    require_notarization()?;
    let mut fds = std::ptr::null_mut();
    let mut count = 0usize;
    let status = unsafe { launch_activate_socket(c"Network".as_ptr(), &mut fds, &mut count) };
    if status != 0 {
        return Err(format!(
            "Network helper launchd socket unavailable ({status})"
        ));
    }
    if count != 1 {
        for i in 0..count {
            unsafe {
                libc::close(*fds.add(i));
            }
        }
        unsafe {
            libc::free(fds.cast());
        }
        return Err("Invalid launchd network socket count".into());
    }
    let listener = unsafe { UnixListener::from_raw_fd(*fds) };
    unsafe {
        libc::free(fds.cast());
    }
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let active = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut idle = Instant::now();
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                if active.load(std::sync::atomic::Ordering::SeqCst) >= 16 {
                    drop(stream);
                    continue;
                }
                active.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let active = active.clone();
                std::thread::spawn(move || {
                    let _ = serve(stream);
                    active.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                });
                idle = Instant::now();
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => return Err(e.to_string()),
        }
        if active.load(std::sync::atomic::Ordering::SeqCst) > 0 {
            idle = Instant::now();
        } else if idle.elapsed() > Duration::from_secs(30) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(30));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn notarization_requires_both_valid_signature_and_explicit_acceptance() {
        assert!(assessment_result(
            true,
            true,
            "accepted\nsource=Notarized Developer ID\n"
        ));
        assert!(!assessment_result(
            false,
            true,
            "source=Notarized Developer ID"
        ));
        assert!(!assessment_result(
            true,
            false,
            "source=Notarized Developer ID"
        ));
        for source in [
            "Developer ID",
            "Unnotarized Developer ID",
            "Explicit Override",
            "",
        ] {
            assert!(!assessment_result(true, true, &format!("source={source}")));
        }
        assert!(!assessment_result(
            true,
            true,
            "source=Notarized Developer ID\nsource=Developer ID"
        ));
    }
    #[test]
    fn rejects_other_signed_processes() {
        let path = std::path::PathBuf::from(format!(
            "/private/tmp/bootah-peer-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let listener = UnixListener::bind(&path).unwrap();
        listener.set_nonblocking(true).unwrap();
        let mut child = Command::new("/usr/bin/nc")
            .arg("-U")
            .arg(&path)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let start = Instant::now();
        let result = loop {
            match listener.accept() {
                Ok((stream, _)) => break authenticate(&stream, unsafe { libc::geteuid() }, None),
                Err(_) if start.elapsed() < Duration::from_secs(3) => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                Err(e) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = std::fs::remove_file(&path);
                    panic!("{e}");
                }
            }
        };
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_file(path);
        assert!(result.is_err());
    }
    #[test]
    fn unbundled_build_cannot_use_registered_daemon() {
        assert!(!availability().helper_available);
        assert!(require_notarization().is_err());
    }
    #[test]
    fn bounded_messages_and_port_allowlist() {
        let (a, b) = UnixStream::pair().unwrap();
        write_message(&a, 2, b"frame").unwrap();
        assert_eq!(read_message(&b).unwrap(), (2, b"frame".to_vec()));
        assert!(write_message(&a, 2, &vec![0; LIMIT + 1]).is_err());
        assert!(allowed_address("udp", "127.0.0.1:69").is_ok());
        assert!(allowed_address("tcp", "127.0.0.1:22").is_err());
        assert!(allowed_address("udp", "[::1]:69").is_err());
    }
    #[test]
    fn descriptor_transfer_and_peer_identity() {
        let (a, b) = UnixStream::pair().unwrap();
        let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        transfer(&a, Some(socket.as_raw_fd())).unwrap();
        let passed: std::net::UdpSocket = transfer(&b, None).unwrap().unwrap().into();
        assert_eq!(socket.local_addr().unwrap(), passed.local_addr().unwrap());
        assert!(authenticate(&a, unsafe { libc::geteuid() } + 1, None).is_err());
        authenticate(&a, unsafe { libc::geteuid() }, Some(std::process::id())).unwrap();
    }
}
