use crate::model::{Profile, Shared};
use std::{
    io::{Read, Seek, SeekFrom},
    net::{IpAddr, Ipv4Addr},
    path::PathBuf,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    task::{JoinHandle, JoinSet},
};

const OPTIONS: u64 = 0x49484156454f5054;
const REPLY: u64 = 0x0003e889045565a9;
const MAX_READ: u32 = 32 * 1024 * 1024;

pub fn eligible(profile: &Profile) -> bool {
    profile.kind == "linux"
        && std::fs::read(profile.root.join(".bootah-linux-compat.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
            .is_some_and(|v| v["version"] == 1 && v["transport"] == "nbd")
}
pub fn port() -> Result<Option<u16>, String> {
    match std::env::var("BOOTAH_NBD_PORT").ok().as_deref() {
        Some("off" | "none") => Ok(None),
        Some(value) => value
            .parse::<u16>()
            .map_err(|e| e.to_string())
            .and_then(|p| {
                if p == 0 {
                    Err("NBD port must be nonzero".into())
                } else {
                    Ok(Some(p))
                }
            }),
        None => Ok(Some(10809)),
    }
}
fn assigned(state: &Shared, ip: Ipv4Addr, name: &str) -> Result<Profile, String> {
    state
        .lock()
        .map_err(|e| e.to_string())?
        .profile_for(ip)
        .filter(|p| p.id == name && eligible(p))
        .ok_or_else(|| "NBD export is not assigned".into())
}
async fn option_reply(
    stream: &mut TcpStream,
    option: u32,
    kind: u32,
    bytes: &[u8],
) -> Result<(), String> {
    stream.write_u64(REPLY).await.map_err(err)?;
    stream.write_u32(option).await.map_err(err)?;
    stream.write_u32(kind).await.map_err(err)?;
    stream.write_u32(bytes.len() as u32).await.map_err(err)?;
    stream.write_all(bytes).await.map_err(err)
}
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
struct Export {
    profile: Profile,
    length: u64,
    source: PathBuf,
    modified: std::time::SystemTime,
}
impl Export {
    fn new(profile: Profile) -> Result<Self, String> {
        let source = PathBuf::from(&profile.media).canonicalize().map_err(err)?;
        let metadata = std::fs::metadata(&source).map_err(err)?;
        if !metadata.is_file() || metadata.len() == 0 || metadata.len() % 512 != 0 {
            return Err("NBD requires a sector-aligned ISO".into());
        }
        Ok(Self {
            profile,
            length: metadata.len(),
            source,
            modified: metadata.modified().map_err(err)?,
        })
    }
    fn read(
        &self,
        state: &Shared,
        ip: Ipv4Addr,
        offset: u64,
        length: u32,
    ) -> Result<Vec<u8>, String> {
        let current = assigned(state, ip, &self.profile.id)?;
        if current.root != self.profile.root || current.media != self.profile.media {
            return Err("NBD export changed".into());
        }
        let mut file = std::fs::File::open(&self.source).map_err(err)?;
        let meta = file.metadata().map_err(err)?;
        if meta.len() != self.length || meta.modified().map_err(err)? != self.modified {
            return Err("NBD source changed; import again".into());
        }
        let mut data = vec![0; length as usize];
        file.seek(SeekFrom::Start(offset)).map_err(err)?;
        file.read_exact(&mut data).map_err(err)?;
        Ok(data)
    }
}
async fn session(
    state: Shared,
    mut stream: TcpStream,
    ip: Ipv4Addr,
    transfers: std::sync::Arc<tokio::sync::Semaphore>,
) -> Result<(), String> {
    stream.set_nodelay(true).map_err(err)?;
    stream.write_all(b"NBDMAGIC").await.map_err(err)?;
    stream.write_u64(OPTIONS).await.map_err(err)?;
    stream.write_u16(3).await.map_err(err)?;
    let flags = tokio::time::timeout(Duration::from_secs(30), stream.read_u32())
        .await
        .map_err(err)?
        .map_err(err)?;
    if flags & !3 != 0 {
        return Err("Unsupported NBD client flags".into());
    }
    let (export, minimum) = loop {
        if tokio::time::timeout(Duration::from_secs(30), stream.read_u64())
            .await
            .map_err(err)?
            .map_err(err)?
            != OPTIONS
        {
            return Err("Invalid NBD option magic".into());
        }
        let option = stream.read_u32().await.map_err(err)?;
        let len = stream.read_u32().await.map_err(err)?;
        if len > 65536 {
            return Err("NBD option too large".into());
        }
        let mut payload = vec![0; len as usize];
        stream.read_exact(&mut payload).await.map_err(err)?;
        if option == 2 {
            option_reply(&mut stream, option, 1, &[]).await?;
            return Ok(());
        }
        if !matches!(option, 1 | 6 | 7) {
            option_reply(&mut stream, option, 0x80000001, &[]).await?;
            continue;
        }
        let name = if option == 1 {
            payload.as_slice()
        } else {
            if payload.len() < 6 {
                option_reply(&mut stream, option, 0x80000003, &[]).await?;
                continue;
            }
            let n = u32::from_be_bytes(payload[..4].try_into().unwrap()) as usize;
            if n > 4096 || n + 6 > payload.len() {
                option_reply(&mut stream, option, 0x80000003, &[]).await?;
                continue;
            }
            let count = u16::from_be_bytes(payload[n + 4..n + 6].try_into().unwrap()) as usize;
            if n + 6 + count * 2 != payload.len() {
                option_reply(&mut stream, option, 0x80000003, &[]).await?;
                continue;
            }
            &payload[4..n + 4]
        };
        let candidate = std::str::from_utf8(name)
            .ok()
            .filter(|n| !n.is_empty() && n.len() <= 4096 && !n.contains('\0'))
            .ok_or_else(|| "Invalid NBD export name".to_string())
            .and_then(|name| assigned(&state, ip, name))
            .and_then(Export::new);
        let export = match candidate {
            Ok(value) => value,
            Err(e) if option == 1 => return Err(e),
            Err(_) => {
                option_reply(&mut stream, option, 0x80000006, &[]).await?;
                continue;
            }
        };
        if option == 1 {
            stream.write_u64(export.length).await.map_err(err)?;
            stream.write_u16(3).await.map_err(err)?;
            if flags & 2 == 0 {
                stream.write_all(&[0; 124]).await.map_err(err)?;
            }
            break (export, 1);
        }
        let mut info = 0u16.to_be_bytes().to_vec();
        info.extend(export.length.to_be_bytes());
        info.extend(3u16.to_be_bytes());
        option_reply(&mut stream, option, 3, &info).await?;
        let mut block = 3u16.to_be_bytes().to_vec();
        for value in [1u32, 4096, MAX_READ] {
            block.extend(value.to_be_bytes());
        }
        option_reply(&mut stream, option, 3, &block).await?;
        option_reply(&mut stream, option, 1, &[]).await?;
        if option == 7 {
            break (export, 1);
        }
    };
    state.lock().unwrap().event(
        &ip.to_string(),
        "NBD",
        format!(
            "Read-only export {} {} bytes",
            export.profile.id, export.length
        ),
    );
    let export = std::sync::Arc::new(export);
    let mut read_bytes = 0u64;
    let mut reported_bytes = 0u64;
    loop {
        if stream.read_u32().await.map_err(err)? != 0x25609513 {
            return Err("Invalid NBD request magic".into());
        }
        let command_flags = stream.read_u16().await.map_err(err)?;
        let command = stream.read_u16().await.map_err(err)?;
        let cookie = stream.read_u64().await.map_err(err)?;
        let offset = stream.read_u64().await.map_err(err)?;
        let length = stream.read_u32().await.map_err(err)?;
        if command == 2 {
            return Ok(());
        }
        if length > MAX_READ {
            return Err("NBD request too large".into());
        }
        if command == 1 {
            let mut remaining = length;
            let mut scratch = [0; 8192];
            while remaining > 0 {
                let n = remaining.min(scratch.len() as u32) as usize;
                stream.read_exact(&mut scratch[..n]).await.map_err(err)?;
                remaining -= n as u32;
            }
        }
        let valid = length > 0
            && command_flags == 0
            && offset.is_multiple_of(minimum)
            && (length as u64).is_multiple_of(minimum)
            && offset
                .checked_add(length as u64)
                .is_some_and(|end| end <= export.length);
        let _transfer = transfers.acquire().await.map_err(err)?;
        let (error, data) = if command != 0 {
            (1, Vec::new())
        } else if !valid {
            (22, Vec::new())
        } else {
            let export = export.clone();
            let state = state.clone();
            match tokio::task::spawn_blocking(move || export.read(&state, ip, offset, length))
                .await
                .map_err(err)?
            {
                Ok(data) => (0, data),
                Err(_) => (5, Vec::new()),
            }
        };
        stream.write_u32(0x67446698).await.map_err(err)?;
        stream.write_u32(error).await.map_err(err)?;
        stream.write_u64(cookie).await.map_err(err)?;
        if error == 0 {
            stream.write_all(&data).await.map_err(err)?;
            read_bytes += length as u64;
            if reported_bytes == 0 || read_bytes - reported_bytes >= 64 * 1024 * 1024 {
                state.lock().unwrap().event(
                    &ip.to_string(),
                    "NBD",
                    format!("Read {} total {read_bytes} bytes", export.profile.id),
                );
                reported_bytes = read_bytes;
            }
        }
    }
}
pub async fn start(
    state: Shared,
    config: &crate::model::Config,
) -> Result<Vec<JoinHandle<()>>, String> {
    let enabled = {
        config.profiles.iter().any(|p| {
            eligible(p)
                && (config
                    .devices
                    .iter()
                    .any(|d| d.profile_id.as_ref() == Some(&p.id))
                    || config.default_profiles.values().any(|id| id == &p.id))
        })
    };
    if !enabled {
        return Ok(Vec::new());
    }
    let Some(port) = port()? else {
        return Err("Assigned NBD medium requires NBD service".into());
    };
    let bind = state.lock().map_err(err)?.config.settings.bind_ip;
    let listener = TcpListener::bind((bind, port)).await.map_err(err)?;
    Ok(vec![tokio::spawn(async move {
        let mut clients = JoinSet::new();
        let transfers = std::sync::Arc::new(tokio::sync::Semaphore::new(4));
        loop {
            tokio::select! {
                connection = listener.accept() => {
                    let Ok((stream, peer)) = connection else { break; };
                    let IpAddr::V4(ip) = peer.ip() else { continue; };
                    if clients.len() >= 64 || !state.lock().unwrap().profile_for(ip).is_some_and(|p| eligible(&p)) { continue; }
                    let state = state.clone();
                    let transfers = transfers.clone();
                    clients.spawn(async move {
                        let log_state = state.clone();
                        if let Err(error) = session(state, stream, ip, transfers).await {
                            log_state.lock().unwrap().event(&ip.to_string(), "NBD", format!("Session closed: {error}"));
                        }
                    });
                }
                _ = clients.join_next(), if !clients.is_empty() => {}
            }
        }
    })])
}
