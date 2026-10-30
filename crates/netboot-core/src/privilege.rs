#[cfg(target_os = "linux")]
mod linux {
    use std::{
        io,
        os::{
            fd::{AsRawFd, FromRawFd, OwnedFd},
            unix::{
                fs::{DirBuilderExt, MetadataExt},
                net::{UnixListener, UnixStream},
            },
        },
        process::Command,
        sync::OnceLock,
    };
    static AUTH: OnceLock<String> = OnceLock::new();
    pub fn configure(mode: &str) {
        let _ = AUTH.set(mode.into());
    }
    fn error() -> String {
        io::Error::last_os_error().to_string()
    }
    fn peer(stream: &UnixStream) -> Result<u32, String> {
        let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
        let mut size = std::mem::size_of_val(&cred) as libc::socklen_t;
        if unsafe {
            libc::getsockopt(
                stream.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                (&mut cred as *mut libc::ucred).cast(),
                &mut size,
            )
        } != 0
        {
            return Err(error());
        }
        Ok(cred.uid)
    }
    pub fn helper(args: &[String]) -> Result<(), String> {
        if args.len() != 4 || unsafe { libc::geteuid() } != 0 {
            return Err("Invalid privileged socket request".into());
        }
        let stream = UnixStream::connect(&args[3]).map_err(|e| e.to_string())?;
        let caller = std::env::var("PKEXEC_UID")
            .or_else(|_| std::env::var("SUDO_UID"))
            .map_err(|_| "Missing authorization identity")?
            .parse::<u32>()
            .map_err(|_| "Invalid authorization identity")?;
        if peer(&stream)? != caller {
            return Err("Socket peer does not match authorized user".into());
        }
        let fd = match args[1].as_str() {
            "ethernet" => crate::raw::open_packet(&args[2])?,
            "udp" => {
                let address = args[2]
                    .parse::<std::net::SocketAddrV4>()
                    .map_err(|e| e.to_string())?;
                if !matches!(address.port(), 67 | 69 | 111 | 4011) {
                    return Err("Unsupported privileged UDP port".into());
                }
                std::net::UdpSocket::bind(address)
                    .map_err(|e| e.to_string())?
                    .into()
            }
            "tcp" => {
                let address = args[2]
                    .parse::<std::net::SocketAddrV4>()
                    .map_err(|e| e.to_string())?;
                if !matches!(address.port(), 80 | 111 | 139 | 445) {
                    return Err("Unsupported privileged TCP port".into());
                }
                std::net::TcpListener::bind(address)
                    .map_err(|e| e.to_string())?
                    .into()
            }
            _ => return Err("Unknown privileged socket type".into()),
        };
        transfer(&stream, Some(fd.as_raw_fd()))?;
        Ok(())
    }
    fn transfer(stream: &UnixStream, fd: Option<i32>) -> Result<Option<OwnedFd>, String> {
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
        msg.msg_controllen = unsafe { libc::CMSG_SPACE(4) } as _;
        unsafe {
            if let Some(fd) = fd {
                let c = libc::CMSG_FIRSTHDR(&msg);
                (*c).cmsg_level = libc::SOL_SOCKET;
                (*c).cmsg_type = libc::SCM_RIGHTS;
                (*c).cmsg_len = libc::CMSG_LEN(4) as _;
                std::ptr::write(libc::CMSG_DATA(c).cast::<i32>(), fd);
                if libc::sendmsg(stream.as_raw_fd(), &msg, 0) != 1 {
                    return Err(error());
                }
                return Ok(None);
            }
            if libc::recvmsg(stream.as_raw_fd(), &mut msg, libc::MSG_CMSG_CLOEXEC) != 1 {
                return Err(error());
            }
            let c = libc::CMSG_FIRSTHDR(&msg);
            if c.is_null()
                || (*c).cmsg_level != libc::SOL_SOCKET
                || (*c).cmsg_type != libc::SCM_RIGHTS
                || (*c).cmsg_len as usize != libc::CMSG_LEN(4) as usize
            {
                return Err("Invalid privileged socket response".into());
            }
            Ok(Some(OwnedFd::from_raw_fd(std::ptr::read(
                libc::CMSG_DATA(c).cast::<i32>(),
            ))))
        }
    }
    pub fn permission_hint() -> &'static str {
        let installed = std::path::Path::new("/usr/libexec/netboot-socket-helper");
        let executable = if installed.is_file() {
            Some(installed.to_path_buf())
        } else {
            std::env::current_exe().ok()
        };
        let trusted = executable
            .and_then(|p| p.canonicalize().ok())
            .is_some_and(|p| {
                p.ancestors().all(|part| {
                    std::fs::metadata(part).is_ok_and(|m| {
                        m.uid() == 0
                            && (m.mode() & 0o022 == 0 || m.is_dir() && m.mode() & 0o1000 != 0)
                    })
                })
            });
        if !trusted {
            return "Install the Bootah network helper using sudo sh scripts/install_linux_core.sh /path/to/netboot-core from the Bootah distribution.";
        }
        let executable = if AUTH.get().is_some_and(|mode| mode == "sudo") {
            "sudo"
        } else {
            "pkexec"
        };
        let found = std::env::var_os("PATH").is_some_and(|paths| {
            std::env::split_paths(&paths).any(|p| p.join(executable).is_file())
        });
        if found {
            return "Network privileges are required. Start services and approve the polkit or sudo prompt.";
        }
        "Install polkit (pkexec) and a desktop authentication agent using your distribution package manager, then retry and approve the authorization prompt. For TUI, install sudo and use --authorization sudo."
    }
    pub fn request(kind: &str, address: &str) -> Result<OwnedFd, String> {
        let mode = AUTH.get().map(String::as_str).unwrap_or("polkit");
        if mode == "none" {
            return Err("Network permission is required. Use Authorize detection for DHCP probing. To start services on Linux, install the Bootah network helper and approve polkit or sudo authorization.".into());
        }
        let id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("netboot-auth-{}-{id:x}", std::process::id()));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .map_err(|e| e.to_string())?;
        let result = (|| {
            let path = directory.join("socket");
            let listener = UnixListener::bind(&path).map_err(|e| e.to_string())?;
            listener.set_nonblocking(true).map_err(|e| format!("Configure network listener: {e}"))?;
            let installed = std::path::Path::new("/usr/libexec/netboot-socket-helper");
            let executable = if installed.is_file() {
                installed.to_path_buf()
            } else {
                std::env::current_exe().map_err(|e| e.to_string())?
            }
            .canonicalize()
            .map_err(|e| e.to_string())?;
            for path in executable.ancestors() {
                let metadata = std::fs::metadata(path).map_err(|e| e.to_string())?;
                if metadata.uid() != 0
                    || (metadata.mode() & 0o022 != 0
                        && !(metadata.is_dir() && metadata.mode() & 0o1000 != 0))
                {
                    return Err(
                        "Install the Bootah network helper using sudo sh scripts/install_linux_core.sh /path/to/netboot-core from the Bootah distribution."
                            .into(),
                    );
                }
            }
            let mut command = Command::new(if mode == "sudo" { "sudo" } else { "pkexec" });
            if mode == "sudo" {
                command.arg("--");
            }
            let mut child = command
                .arg(executable)
                .args(["--socket-helper", kind, address])
                .arg(&path)
                .spawn()
                .map_err(|e| format!("Install polkit (pkexec) and a desktop authentication agent using your distribution package manager, then retry and approve the authorization prompt. For TUI, install sudo and use --authorization sudo.\n{e}"))?;
            let begin = std::time::Instant::now();
            loop {
                match listener.accept() {
                    Ok((stream, _)) => {
                        if peer(&stream)? != 0 {
                            return Err("Privileged helper must run as root".into());
                        }
                        stream
                            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                            .map_err(|e| e.to_string())?;
                        let fd = transfer(&stream, None)?.ok_or("Missing socket descriptor")?;
                        let status = child.wait().map_err(|e| e.to_string())?;
                        if !status.success() {
                            return Err("Privileged helper failed".into());
                        }
                        return Ok(fd);
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                    Err(e) => return Err(e.to_string()),
                }
                if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
                    return Err(format!(
                        "Network access was not authorized. Retry and approve the system authorization prompt; on Linux ensure a desktop polkit authentication agent is running.\n{status}"
                    ));
                }
                if begin.elapsed().as_secs() > 120 {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("Network access was not authorized. Retry and approve the system authorization prompt; on Linux ensure a desktop polkit authentication agent is running.".into());
                }
                std::thread::sleep(std::time::Duration::from_millis(30));
            }
        })();
        let _ = std::fs::remove_file(directory.join("socket"));
        let _ = std::fs::remove_dir(directory);
        result
    }
}
#[cfg(target_os = "linux")]
pub use linux::{configure, helper, permission_hint, request};

pub async fn udp(address: (std::net::Ipv4Addr, u16)) -> Result<tokio::net::UdpSocket, String> {
    match tokio::net::UdpSocket::bind(address).await {
        Ok(socket) => Ok(socket),
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            let bind_error = format!("UDP {}: {e}", std::net::SocketAddrV4::new(address.0, address.1));
            let address = std::net::SocketAddrV4::new(address.0, address.1).to_string();
            let fd = tokio::task::spawn_blocking(move || platform_request("udp", &address))
                .await
                .map_err(|error| format!("{bind_error}\n{error}"))?
                .map_err(|error| format!("{bind_error}\n{error}"))?;
            let socket: std::net::UdpSocket = fd.into();
            socket.set_nonblocking(true).map_err(|e| format!("Configure authorized UDP socket: {e}"))?;
            tokio::net::UdpSocket::from_std(socket).map_err(|e| format!("Register authorized UDP socket: {e}"))
        }
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => Err(format!(
            "UDP {}: {e}\n{}", std::net::SocketAddrV4::new(address.0, address.1), port_guidance()
        )),
        Err(e) => Err(format!("UDP {}: {e}", std::net::SocketAddrV4::new(address.0, address.1))),
    }
}

/// The Linux privileged helper allows the RPC mapper and RIS SMB listeners.
pub async fn tcp(address: (std::net::Ipv4Addr, u16)) -> Result<tokio::net::TcpListener, String> {
    match tokio::net::TcpListener::bind(address).await {
        Ok(listener) => Ok(listener),
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            let bind_error = format!("TCP {}: {e}", std::net::SocketAddrV4::new(address.0, address.1));
            let address = std::net::SocketAddrV4::new(address.0, address.1).to_string();
            let fd = tokio::task::spawn_blocking(move || platform_request("tcp", &address))
                .await
                .map_err(|error| format!("{bind_error}\n{error}"))?
                .map_err(|error| format!("{bind_error}\n{error}"))?;
            let listener: std::net::TcpListener = fd.into();
            listener.set_nonblocking(true).map_err(|e| format!("Configure network listener: {e}"))?;
            tokio::net::TcpListener::from_std(listener).map_err(|e| format!("Register authorized TCP listener: {e}"))
        }
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => Err(format!(
            "TCP {}: {e}\n{}", std::net::SocketAddrV4::new(address.0, address.1), port_guidance()
        )),
        Err(e) => Err(format!("TCP {}: {e}", std::net::SocketAddrV4::new(address.0, address.1))),
    }
}

#[cfg(target_os = "macos")]
use crate::macos_privilege::socket as platform_request;
#[cfg(target_os = "linux")]
use request as platform_request;

pub fn port_guidance() -> String {
    #[cfg(target_os = "macos")]
    return if crate::macos_privilege::availability().helper_available {
        crate::macos_privilege::PORT_GUIDANCE.into()
    } else {
        crate::macos_elevation::GUIDANCE.into()
    };
    #[cfg(target_os = "linux")]
    return permission_hint().into();
    #[cfg(target_os = "android")]
    return crate::permissions::ANDROID_GUIDANCE.into();
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "android")))]
    "Network port access was denied. Check for another service using an exclusive bind, reserved ports, or system security restrictions. Installing Npcap does not grant access to this port.".into()
}
