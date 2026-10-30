use crate::macos_privilege::{authenticate, read_message, serve_authenticated, write_message};
use std::{
    io::Read,
    os::unix::{
        fs::{DirBuilderExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

pub const GUIDANCE: &str = "Start services and approve the macOS administrator prompt to allow the required network ports. Packet capture access does not grant permission to bind these ports.";
const CANCELLED: &str = "Administrator authorization was cancelled. Services remain stopped.";
const SCRIPT: &str = r#"on run argv
set commandText to "exec " & quoted form of (item 1 of argv) & " --macos-authorized-network " & quoted form of (item 2 of argv) & " " & quoted form of (item 3 of argv) & " " & quoted form of (item 4 of argv)
do shell script commandText with administrator privileges
end run"#;

struct Authorization {
    control: UnixStream,
    child: Child,
    directory: PathBuf,
}
impl Drop for Authorization {
    fn drop(&mut self) {
        let _ = self.control.shutdown(std::net::Shutdown::Both);
        let _ = std::fs::remove_file(self.directory.join("gate"));
        let _ = std::fs::remove_file(self.directory.join("network"));
        let _ = std::fs::remove_dir(&self.directory);
    }
}
fn session() -> &'static Mutex<Option<Authorization>> {
    static SESSION: OnceLock<Mutex<Option<Authorization>>> = OnceLock::new();
    SESSION.get_or_init(|| Mutex::new(None))
}
pub fn available() -> bool {
    session().try_lock().is_ok_and(|mut value| {
        value
            .as_mut()
            .is_some_and(|a| matches!(a.child.try_wait(), Ok(None)))
    })
}
pub fn connect() -> Result<UnixStream, String> {
    let mut value = session().lock().map_err(|e| e.to_string())?;
    if let Some(a) = value.as_mut() {
        if matches!(a.child.try_wait(), Ok(None)) {
            if let Ok(stream) = UnixStream::connect(a.directory.join("network")) {
                return Ok(stream);
            }
        }
    }
    *value = None;
    let directory = PathBuf::from(format!(
        "/private/tmp/bootah-{}",
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&directory)
        .map_err(|e| e.to_string())?;
    let result = (|| {
        let gate = directory.join("gate");
        let listener =
            UnixListener::bind(&gate).map_err(|e| format!("Create authorization listener: {e}"))?;
        listener
            .set_nonblocking(true)
            .map_err(|e| format!("Configure authorization listener: {e}"))?;
        let executable = std::env::current_exe()
            .and_then(|p| p.canonicalize())
            .map_err(|e| e.to_string())?;
        let mut child = Command::new("/usr/bin/osascript")
            .args(["-e", SCRIPT, "--"])
            .arg(executable)
            .arg(&gate)
            .arg(unsafe { libc::geteuid() }.to_string())
            .arg(std::process::id().to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| e.to_string())?;
        let begin = Instant::now();
        let connected = loop {
            match listener.accept() {
                Ok((control, _)) => {
                    if let Err(e) = authenticate(&control, 0, None) {
                        break Err(e);
                    }
                    if let Err(e) = control.set_nonblocking(false) {
                        break Err(format!("Configure authorization response: {e}"));
                    }
                    let _ = control.set_read_timeout(Some(Duration::from_secs(10)));
                    match read_message(&control) {
                        Ok((0, data)) if data.is_empty() => break Ok(control),
                        Ok((_, data)) => break Err(String::from_utf8_lossy(&data).into_owned()),
                        Err(e) => break Err(format!("Read authorization status: {e}")),
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => break Err(format!("Administrator connection: {e}")),
            }
            match child.try_wait() {
                Ok(Some(_)) => {
                    let mut error = String::new();
                    if let Some(stderr) = child.stderr.take() {
                        let _ = stderr.take(65536).read_to_string(&mut error);
                    }
                    break Err(if error.contains("(-128)") {
                        CANCELLED.into()
                    } else {
                        format!(
                            "Administrator authorization failed. Retry starting services.\n{error}"
                        )
                    });
                }
                Err(e) => break Err(format!("Administrator connection: {e}")),
                Ok(None) => {}
            }
            if begin.elapsed() > Duration::from_secs(120) {
                break Err(
                    "Administrator authorization timed out. Retry starting services.".into(),
                );
            }
            std::thread::sleep(Duration::from_millis(30));
        };
        match connected {
            Ok(control) => Ok(Authorization {
                control,
                child,
                directory: directory.clone(),
            }),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                Err(error)
            }
        }
    })();
    match result {
        Ok(authorization) => {
            let stream = UnixStream::connect(directory.join("network"))
                .map_err(|e| format!("Connect authorized network process: {e}"))?;
            *value = Some(authorization);
            Ok(stream)
        }
        Err(error) => {
            let _ = std::fs::remove_file(directory.join("gate"));
            let _ = std::fs::remove_file(directory.join("network"));
            let _ = std::fs::remove_dir(directory);
            Err(error)
        }
    }
}

pub fn run(args: &[String]) -> Result<(), String> {
    if args.len() != 3 || unsafe { libc::geteuid() } != 0 {
        return Err("Administrator authorization is required for network access.".into());
    }
    let uid: u32 = args[1]
        .parse()
        .map_err(|_| "Invalid network authorization identity")?;
    let pid: u32 = args[2]
        .parse()
        .map_err(|_| "Invalid network authorization process")?;
    run_network(&args[0], uid, pid)
}
fn run_network(gate: &str, uid: u32, pid: u32) -> Result<(), String> {
    let control =
        UnixStream::connect(gate).map_err(|e| format!("Connect authorization channel: {e}"))?;
    authenticate(&control, uid, Some(pid))?;
    let directory = PathBuf::from(gate)
        .parent()
        .ok_or("Missing network authorization directory")?
        .to_path_buf();
    let path = directory.join("network");
    let result = (|| {
        let listener =
            UnixListener::bind(&path).map_err(|e| format!("Create network listener: {e}"))?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666))
            .map_err(|e| format!("Set network socket access: {e}"))?;
        listener
            .set_nonblocking(true)
            .map_err(|e| format!("Configure authorization listener: {e}"))?;
        control
            .set_nonblocking(true)
            .map_err(|e| format!("Configure authorization channel: {e}"))?;
        write_message(&control, 0, &[])?;
        let active = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        loop {
            let mut byte = [0];
            match (&control).read(&mut byte) {
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                _ => return Ok(()),
            }
            match listener.accept() {
                Ok((stream, _)) => {
                    if active.load(std::sync::atomic::Ordering::SeqCst) >= 16 {
                        continue;
                    }
                    active.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let active = active.clone();
                    std::thread::spawn(move || {
                        if authenticate(&stream, uid, Some(pid)).is_ok() {
                            let _ = serve_authenticated(stream);
                        }
                        active.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(format!("Network connection: {e}")),
            }
            std::thread::sleep(Duration::from_millis(30));
        }
    })();
    let _ = write_message(
        &control,
        1,
        result
            .as_ref()
            .err()
            .map(String::as_bytes)
            .unwrap_or_default(),
    );
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(directory.join("gate"));
    let _ = std::fs::remove_dir(directory);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn applescript_quotes_paths_without_interpolation() {
        let script = SCRIPT.replace(
            "do shell script commandText with administrator privileges",
            "return commandText",
        );
        let executable = "/tmp/Bootah's $(echo injected) \"app\"/netboot-core";
        let gate = "/tmp/session's socket";
        let output = Command::new("/usr/bin/osascript")
            .args(["-e", &script, "--", executable, gate, "501", "123"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let command = String::from_utf8(output.stdout).unwrap();
        let command = command.trim().strip_prefix("exec ").unwrap();
        let output = Command::new("/bin/sh")
            .args(["-c", &format!("set -- {command}; printf '%s\\n' \"$@\"")])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8(output.stdout)
                .unwrap()
                .lines()
                .collect::<Vec<_>>(),
            [executable, "--macos-authorized-network", gate, "501", "123"]
        );
    }
    #[test]
    fn temporary_network_session_transfers_socket_and_exits_on_disconnect() {
        let directory = PathBuf::from(format!(
            "/private/tmp/bootah-test-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .unwrap();
        let gate = directory.join("gate");
        let listener = UnixListener::bind(&gate).unwrap();
        let thread = std::thread::spawn(move || {
            run_network(
                gate.to_str().unwrap(),
                unsafe { libc::geteuid() },
                std::process::id(),
            )
        });
        let (control, _) = listener.accept().unwrap();
        control
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        assert_eq!(read_message(&control).unwrap(), (0, vec![]));
        let network = UnixStream::connect(directory.join("network")).unwrap();
        network
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        write_message(&network, 0, br#"{"kind":"udp","address":"127.0.0.1:4011"}"#).unwrap();
        assert_eq!(read_message(&network).unwrap(), (0, vec![]));
        let fd = crate::macos_privilege::transfer(&network, None)
            .unwrap()
            .unwrap();
        write_message(&network, 3, &[]).unwrap();
        let udp: std::net::UdpSocket = fd.into();
        assert_eq!(udp.local_addr().unwrap().port(), 4011);
        udp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let sender = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        sender.send_to(b"test", udp.local_addr().unwrap()).unwrap();
        let mut data = [0; 4];
        assert_eq!(udp.recv(&mut data).unwrap(), 4);
        assert_eq!(&data, b"test");
        udp.set_nonblocking(true).unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let socket = tokio::net::UdpSocket::from_std(udp).unwrap();
            assert_eq!(socket.local_addr().unwrap().port(), 4011);
        });
        drop(network);
        drop(control);
        thread.join().unwrap().unwrap();
        assert!(!directory.exists());
    }
    #[test]
    fn network_mode_requires_root_and_complete_arguments() {
        assert!(run(&[]).is_err());
        assert!(run(&["missing".into()]).is_err());
        assert!(!available());
    }
}
