mod netlink;
use std::{
    fs::OpenOptions,
    io::{Read, Write},
    net::TcpStream,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::process::CommandExt,
    },
    time::Duration,
};
fn run() -> Result<(), String> {
    let a: Vec<String> = std::env::args().collect();
    if !(a.len() == 6 || (a.len() == 7 && a[6] == "--legacy-ioctl")) || a[1] != "nbd" {
        return Err("usage: bootah-media nbd SERVER PORT EXPORT DEVICE [--legacy-ioctl]".into());
    }
    let port: u16 = a[3].parse().map_err(|_| "invalid port")?;
    let mut s = TcpStream::connect((a[2].as_str(), port)).map_err(|e| e.to_string())?;
    s.set_nodelay(true).map_err(|e| e.to_string())?;
    s.set_read_timeout(Some(Duration::from_secs(15)))
        .map_err(|e| e.to_string())?;
    s.set_write_timeout(Some(Duration::from_secs(15)))
        .map_err(|e| e.to_string())?;
    let mut hello = [0u8; 18];
    s.read_exact(&mut hello).map_err(|e| e.to_string())?;
    if &hello[..8] != b"NBDMAGIC"
        || u64::from_be_bytes(hello[8..16].try_into().unwrap()) != 0x49484156454f5054
        || u16::from_be_bytes(hello[16..18].try_into().unwrap()) & 3 != 3
    {
        return Err("server lacks fixed newstyle/no-zeroes NBD".into());
    }
    s.write_all(&3u32.to_be_bytes())
        .map_err(|e| e.to_string())?;
    let name = a[4].as_bytes();
    if name.is_empty() || name.len() > 4096 || name.contains(&0) {
        return Err("invalid export name".into());
    }
    s.write_all(&0x49484156454f5054u64.to_be_bytes())
        .and_then(|_| s.write_all(&1u32.to_be_bytes()))
        .and_then(|_| s.write_all(&(name.len() as u32).to_be_bytes()))
        .and_then(|_| s.write_all(name))
        .map_err(|e| e.to_string())?;
    let mut export = [0u8; 10];
    s.read_exact(&mut export).map_err(|e| e.to_string())?;
    let size = u64::from_be_bytes(export[..8].try_into().unwrap());
    let flags = u16::from_be_bytes(export[8..].try_into().unwrap());
    if size == 0 || size % 512 != 0 || flags & 3 != 3 {
        return Err("export must be nonempty, sector aligned and read-only".into());
    }
    s.set_read_timeout(None).map_err(|e| e.to_string())?;
    s.set_write_timeout(None).map_err(|e| e.to_string())?;
    if a.len() == 6 {
        return netlink::connect(s.as_raw_fd(), size, flags, &a[5]);
    }
    let dev = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&a[5])
        .map_err(|e| e.to_string())?;
    let fd = dev.as_raw_fd();
    let ioctl = |request: libc::c_ulong, value: libc::c_ulong| -> Result<(), String> {
        if unsafe { libc::ioctl(fd, request as _, value) } < 0 {
            Err(format!(
                "NBD ioctl {request:#x}: {}",
                std::io::Error::last_os_error()
            ))
        } else {
            Ok(())
        }
    };
    ioctl(0xab01, 512)?;
    ioctl(0xab07, size / 512)?;
    ioctl(0xab0a, 3)?;
    ioctl(0xab00, s.as_raw_fd() as _)?;
    s.set_read_timeout(None).map_err(|e| e.to_string())?;
    s.set_write_timeout(None).map_err(|e| e.to_string())?;
    let mut pipe = [0; 2];
    if unsafe { libc::pipe(pipe.as_mut_ptr()) } < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        ioctl(0xab04, 0)?;
        return Err(std::io::Error::last_os_error().to_string());
    }
    if pid == 0 {
        unsafe {
            libc::close(pipe[0]);
            libc::setsid();
            libc::signal(libc::SIGPIPE, libc::SIG_IGN);
        }
        if std::path::Path::new("/etc/initrd-release").exists() {
            // Exec preserves the actual kernel argv, unlike std::env's copied strings.
            for inherited in [fd, s.as_raw_fd(), pipe[1]] {
                if unsafe { libc::fcntl(inherited, libc::F_SETFD, 0) } < 0 {
                    worker_exit(
                        fd,
                        pipe[1],
                        std::io::Error::last_os_error().raw_os_error().unwrap_or(5),
                    );
                }
            }
            match std::env::current_exe() {
                Ok(executable) => {
                    let error = std::process::Command::new(executable)
                        .arg0("@bootah-media")
                        .args(["--nbd-worker", &fd.to_string(), &pipe[1].to_string()])
                        .exec();
                    worker_exit(fd, pipe[1], error.raw_os_error().unwrap_or(5));
                }
                Err(error) => worker_exit(fd, pipe[1], error.raw_os_error().unwrap_or(5)),
            }
        }
        worker(fd, pipe[1]);
    }
    unsafe {
        libc::close(pipe[1]);
        libc::fcntl(pipe[0], libc::F_SETFL, libc::O_NONBLOCK);
    }
    let mut status = unsafe { std::fs::File::from_raw_fd(pipe[0]) };
    let device = std::path::Path::new(&a[5])
        .file_name()
        .ok_or("invalid device")?
        .to_str()
        .ok_or("invalid device")?;
    for _ in 0..100 {
        let mut e = [0u8; 4];
        if status.read(&mut e).unwrap_or(0) > 0 {
            return Err(format!("NBD worker exited: {}", i32::from_ne_bytes(e)));
        }
        if std::fs::read_to_string(format!("/sys/block/{device}/size"))
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            == Some(size / 512)
        {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    unsafe {
        libc::ioctl(fd, 0xab08, 0);
    }
    Err("NBD kernel device did not become ready".into())
}
fn worker_exit(fd: i32, status: i32, error: i32) -> ! {
    unsafe {
        libc::write(status, (&error as *const i32).cast(), 4);
        libc::ioctl(fd, 0xab04, 0);
        libc::_exit(if error == 0 { 0 } else { 1 });
    }
}
fn worker(fd: i32, status: i32) -> ! {
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }
    let result = unsafe { libc::ioctl(fd, 0xab03, 0) };
    let error = if result < 0 {
        std::io::Error::last_os_error().raw_os_error().unwrap_or(5)
    } else {
        0
    };
    worker_exit(fd, status, error)
}
fn main() {
    let arguments: Vec<String> = std::env::args().collect();
    if arguments.len() == 4 && arguments[1] == "--nbd-worker" {
        if let (Ok(fd), Ok(status)) = (arguments[2].parse::<i32>(), arguments[3].parse::<i32>()) {
            worker(fd, status);
        }
        eprintln!("bootah-media: invalid worker descriptors");
        std::process::exit(1);
    }
    if let Err(e) = run() {
        eprintln!("bootah-media: {e}");
        std::process::exit(1)
    }
}
