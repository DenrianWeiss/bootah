use crate::{boot, model::*};
use std::{
    io::{BufReader, Cursor, Read, Write},
    net::{IpAddr, Ipv4Addr, SocketAddr},
    time::{Duration, Instant},
};
use tokio::net::UdpSocket;

pub fn error(code: u16, message: &str) -> Vec<u8> {
    let mut p = vec![0, 5];
    p.extend(code.to_be_bytes());
    p.extend(message.bytes());
    p.push(0);
    p
}
struct Upload {
    file: std::fs::File,
    temporary: std::path::PathBuf,
    destination: std::path::PathBuf,
    bytes: u64,
}
impl Drop for Upload {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.temporary);
    }
}
pub struct Session {
    upload: Option<Upload>,
    pub completed: bool,
    reader: Box<dyn Read + Send>,
    pub packet: Vec<u8>,
    pub block: u16,
    block_size: usize,
    final_packet: bool,
    pub sent: Instant,
    retries: u8,
}
impl Session {
    pub fn begin(state: &Shared, request: &[u8], ip: Ipv4Addr) -> Result<Self, Vec<u8>> {
        let writing = request.get(..2) == Some(&[0, 2]);
        if !writing && request.get(..2) != Some(&[0, 1]) {
            return Err(error(4, "Read or write request required"));
        }
        if request.last() != Some(&0) {
            return Err(error(4, "Malformed request"));
        }
        let parts = request[2..request.len() - 1]
            .split(|b| *b == 0)
            .map(std::str::from_utf8)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| error(4, "Invalid request encoding"))?;
        if parts.len() < 2 || !parts[1].eq_ignore_ascii_case("octet") || (parts.len() - 2) % 2 != 0
        {
            return Err(error(4, "Octet mode required"));
        }
        let (profile, settings, auto, ris_answer) = {
            let s = state.lock().unwrap();
            let profile = s.profile_for(ip);
            let device_id = s.device_for(ip).map(|d| d.id.as_str());
            let auto = profile
                .as_ref()
                .and_then(|p| crate::automation::effective(&s.config, p, device_id))
                .cloned();
            let ris_answer = auto
                .as_ref()
                .filter(|a| a.method == "winnt-sif")
                .map(|a| std::fs::read(crate::automation::file(&s, &a.token)));
            (profile, s.config.settings.clone(), auto, ris_answer)
        };
        let mut upload = None;
        let (reader, size): (Box<dyn Read + Send>, u64) = if writing {
            let store = state.lock().unwrap();
            let device = store
                .device_for(ip)
                .ok_or_else(|| error(2, "Unknown device"))?;
            let key = if device.mac.is_empty() {
                ip.to_string()
            } else {
                device.mac.replace(':', "-")
            };
            if !key
                .bytes()
                .all(|c| c.is_ascii_hexdigit() || c == b'-' || c == b'.')
            {
                return Err(error(2, "Invalid device address"));
            }
            let name = parts[0];
            if name.is_empty()
                || name.len() > 240
                || name.starts_with('.')
                || name.contains(['/', '\\', ':'])
                || name.chars().any(char::is_control)
            {
                return Err(error(2, "A plain filename is required"));
            }
            let root = store
                .path
                .parent()
                .unwrap_or(std::path::Path::new("."))
                .join("uploads");
            let directory = root.join(key);
            for path in [&root, &directory] {
                if std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
                    return Err(error(2, "Invalid upload directory"));
                }
                std::fs::create_dir_all(path)
                    .map_err(|_| error(2, "Upload directory unavailable"))?;
            }
            let destination = directory.join(name);
            if destination.exists() {
                return Err(error(6, "File already exists"));
            }
            let temporary = directory.join(format!(
                ".upload-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ));
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let file = options
                .open(&temporary)
                .map_err(|_| error(2, "Cannot create upload"))?;
            upload = Some(Upload {
                file,
                temporary,
                destination,
                bytes: 0,
            });
            (Box::new(Cursor::new(Vec::<u8>::new())), 0)
        } else {
            let profile = profile.ok_or_else(|| error(2, "No system assigned"))?;

            let ris_name = matches!(
                profile.kind.as_str(),
                "windows-ris" | "windows" | "windows-pe-iso"
            )
            .then(|| {
                let path = parts[0].replace('\\', "/");
                let path = path.trim_start_matches('/');
                path.split_once('/')
                    .filter(|(prefix, _)| {
                        profile.kind == "windows-ris" && prefix.eq_ignore_ascii_case(&profile.id)
                    })
                    .map(|(_, rest)| rest)
                    .unwrap_or(path)
                    .to_string()
            });
            let requested = ris_name
                .as_deref()
                .unwrap_or(&parts[0])
                .trim_start_matches('/');
            let hex = requested.split('.').next().unwrap_or(requested);
            let name = if matches!(
                profile.architecture.as_str(),
                "sparc" | "sparc32" | "sparc64"
            ) && hex.len() == 8
                && hex.chars().all(|c| c.is_ascii_hexdigit())
            {
                profile
                    .boot_files
                    .get("default")
                    .or_else(|| profile.boot_files.get("sparc64"))
                    .map(String::as_str)
                    .unwrap_or(requested)
            } else {
                requested
            };
            let virtual_bytes =
                if profile.kind == "windows-ris" && name.eq_ignore_ascii_case("winnt.sif") {
                    let selected = match ris_answer.as_ref() {
                        Some(Ok(bytes)) => bytes.as_slice(),
                        Some(Err(_)) => return Err(error(2, "Selected answer file unavailable")),
                        None => &[],
                    };
                    Some(crate::windows::ris_answer(
                        &profile,
                        &settings.server_ip.to_string(),
                        selected,
                    ))
                } else {
                    boot::virtual_file(&profile, name, &settings, auto.as_ref())
                };
            if let Some(bytes) = virtual_bytes {
                let size = bytes.len() as u64;
                (Box::new(Cursor::new(bytes)), size)
            } else {
                let file = crate::media_source::open(&profile.root, crate::native_uefi::resource_name(&profile, name))
                    .or_else(|err| {
                        if profile.kind != "windows-ris" {
                            return Err(err);
                        }
                        let path = name.replace('\\', "/");
                        let fallback =
                            crate::windows::ris_fallback_name(&profile, &path).ok_or(err)?;
                        crate::media_source::open(&profile.root, &fallback)
                    })
                    .map_err(|_| error(1, "File unavailable"))?;
                let size = file.size;
                (Box::new(BufReader::new(file)), size)
            }
        };
        let mut block_size = 512;
        let mut oack = vec![0, 6];
        let mut keys = std::collections::HashSet::new();
        for pair in parts[2..].chunks_exact(2) {
            let key = pair[0].to_ascii_lowercase();
            if !keys.insert(key.clone()) {
                return Err(error(8, "Duplicate option"));
            }
            let value = match key.as_str() {
                "blksize" => {
                    let n = pair[1]
                        .parse::<usize>()
                        .map_err(|_| error(8, "Invalid block size"))?;
                    if !(8..=65464).contains(&n) {
                        return Err(error(8, "Invalid block size"));
                    }
                    block_size = n.min(1468);
                    Some(block_size.to_string())
                }
                "tsize" => {
                    if !writing && pair[1] != "0" {
                        return Err(error(8, "Invalid transfer size"));
                    }
                    if writing {
                        let size = pair[1]
                            .parse::<u64>()
                            .map_err(|_| error(8, "Invalid transfer size"))?;
                        if size > 1024 * 1024 * 1024 {
                            return Err(error(3, "Upload exceeds 1 GiB"));
                        }
                        Some(size.to_string())
                    } else {
                        Some(size.to_string())
                    }
                }
                "timeout" => {
                    let n = pair[1]
                        .parse::<u8>()
                        .map_err(|_| error(8, "Invalid timeout"))?;
                    if n == 0 {
                        return Err(error(8, "Invalid timeout"));
                    }
                    None
                }
                _ => None,
            };
            if let Some(value) = value {
                oack.extend(key.bytes());
                oack.push(0);
                oack.extend(value.bytes());
                oack.push(0)
            }
        }
        let mut session = Self {
            reader,
            upload,
            completed: false,
            packet: oack,
            block: 0,
            block_size,
            final_packet: false,
            sent: Instant::now(),
            retries: 0,
        };
        if session.packet.len() == 2 {
            if writing {
                session.packet = vec![0, 4, 0, 0];
            } else {
                session.advance().map_err(|_| error(0, "Read error"))?
            }
        }
        state.lock().unwrap().event(
            &ip.to_string(),
            "TFTP",
            if writing {
                format!("Upload requested {}", parts[0])
            } else {
                format!("Download {} {size} bytes", parts[0])
            },
        );
        Ok(session)
    }
    fn advance(&mut self) -> std::io::Result<()> {
        self.block = self.block.wrapping_add(1);
        let mut bytes = vec![0; self.block_size];
        let mut n = 0;
        while n < bytes.len() {
            let count = self.reader.read(&mut bytes[n..])?;
            if count == 0 {
                break;
            }
            n += count
        }
        self.final_packet = n < self.block_size;
        self.packet = vec![0, 3];
        self.packet.extend(self.block.to_be_bytes());
        self.packet.extend(&bytes[..n]);
        self.sent = Instant::now();
        self.retries = 0;
        Ok(())
    }
    pub fn acknowledge(&mut self, data: &[u8]) -> Result<Ack, String> {
        if data.get(..2) == Some(&[0, 5]) {
            return Err("Client aborted transfer".into());
        }
        if let Some(upload) = self.upload.as_mut() {
            if data.len() < 4 || data[..2] != [0, 3] || data.len() > self.block_size + 4 {
                return Ok(Ack::Ignored);
            }
            let block = u16::from_be_bytes([data[2], data[3]]);
            if block == self.block {
                return Ok(Ack::Next);
            }
            if self.completed || block != self.block.wrapping_add(1) {
                return Ok(Ack::Ignored);
            }
            upload.bytes += (data.len() - 4) as u64;
            if upload.bytes > 1024 * 1024 * 1024 {
                return Err("Upload exceeds 1 GiB".into());
            }
            upload
                .file
                .write_all(&data[4..])
                .map_err(|e| e.to_string())?;
            self.block = block;
            self.packet = vec![0, 4, data[2], data[3]];
            self.sent = Instant::now();
            self.retries = 0;
            if data.len() < self.block_size + 4 {
                upload.file.sync_all().map_err(|e| e.to_string())?;
                std::fs::hard_link(&upload.temporary, &upload.destination)
                    .map_err(|e| e.to_string())?;
                self.completed = true;
            }
            return Ok(Ack::Next);
        }
        if data.len() != 4 || data[..2] != [0, 4] || data[2..4] != self.block.to_be_bytes() {
            return Ok(Ack::Ignored);
        }
        if self.final_packet {
            return Ok(Ack::Done);
        }
        self.advance().map_err(|e| e.to_string())?;
        Ok(Ack::Next)
    }
    pub fn upload_summary(&self) -> String {
        self.upload
            .as_ref()
            .map(|upload| {
                format!(
                    "Upload saved {} {} bytes",
                    upload
                        .destination
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy(),
                    upload.bytes
                )
            })
            .unwrap_or_default()
    }
    pub fn expired(&self) -> bool {
        self.completed && self.sent.elapsed() >= Duration::from_secs(10)
    }
    pub fn retry(&mut self) -> Result<bool, String> {
        if self.completed {
            return Ok(false);
        }
        if self.sent.elapsed() < Duration::from_secs(2) {
            return Ok(false);
        }
        if self.retries >= 5 {
            return Err("TFTP acknowledgement timed out".into());
        }
        self.retries += 1;
        self.sent = Instant::now();
        Ok(true)
    }
}
pub enum Ack {
    Ignored,
    Next,
    Done,
}
async fn transfer(state: Shared, request: Vec<u8>, peer: SocketAddr) -> Result<(), String> {
    let bind = state.lock().unwrap().config.settings.bind_ip;
    let socket = UdpSocket::bind((bind, 0))
        .await
        .map_err(|e| e.to_string())?;
    let IpAddr::V4(ip) = peer.ip() else {
        return Err("IPv4 required".into());
    };
    let mut session = match Session::begin(&state, &request, ip) {
        Ok(s) => s,
        Err(packet) => {
            state.lock().unwrap().event(
                &ip.to_string(),
                "TFTP",
                format!(
                    "Rejected: {}",
                    String::from_utf8_lossy(&packet[4..packet.len() - 1])
                ),
            );
            let _ = socket.send_to(&packet, peer).await;
            return Ok(());
        }
    };
    socket
        .send_to(&session.packet, peer)
        .await
        .map_err(|e| e.to_string())?;
    let mut buffer = [0; 4096];
    loop {
        tokio::select! {
            result=socket.recv_from(&mut buffer)=>{
                let (n,from)=result.map_err(|e|e.to_string())?;
                if from!=peer{let _=socket.send_to(&error(5,"Unknown transfer ID"),from).await;continue}
                let acknowledgement = match session.acknowledge(&buffer[..n]) {
                    Ok(ack) => ack,
                    Err(e) => { let _ = socket.send_to(&error(0, "Transfer failed"), peer).await; return Err(e); }
                };
                match acknowledgement {Ack::Done=>{state.lock().unwrap().event(&ip.to_string(),"TFTP","Completed transfer");return Ok(())},Ack::Next=>{socket.send_to(&session.packet,peer).await.map_err(|e|e.to_string())?;},Ack::Ignored=>{}}
            }
            _=tokio::time::sleep(Duration::from_millis(250))=>{
                if session.expired() { state.lock().unwrap().event(&ip.to_string(), "TFTP", session.upload_summary()); return Ok(()); }
                if session.retry()?{socket.send_to(&session.packet,peer).await.map_err(|e|e.to_string())?;}
            }
        }
    }
}
pub async fn run(state: Shared, socket: UdpSocket) -> Result<(), String> {
    let mut transfers = tokio::task::JoinSet::new();
    let mut buffer = [0; 4096];
    loop {
        tokio::select! {
            packet=socket.recv_from(&mut buffer)=>{let(n,peer)=packet.map_err(|e|e.to_string())?;if transfers.len()>=64{continue}let packet=buffer[..n].to_vec();let state=state.clone();transfers.spawn(async move{if let Err(e)=transfer(state.clone(),packet,peer).await{state.lock().unwrap().event(&peer.to_string(),"TFTP",e)}});},
            _=transfers.join_next(),if !transfers.is_empty()=>{}
        }
    }
}
