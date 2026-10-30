//! Read-only NFSv3/TCP view of an imported medium. Wire protocol is provided by
//! nfsserve (BSD-3-Clause); no host mounts or unpacked installation tree needed.
use async_trait::async_trait;
use nfsserve::{
    nfs::*,
    tcp::{NFSTcp, NFSTcpListener},
    vfs::*,
};
use std::{
    collections::BTreeMap,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

struct Node {
    path: String,
    directory: bool,
    size: u64,
    mode: u16,
    link: Option<String>,
}
pub struct MediaFs {
    root: PathBuf,
    nodes: BTreeMap<u64, Node>,
    paths: BTreeMap<String, u64>,
}
impl MediaFs {
    pub fn total_bytes(&self) -> u64 {
        self.nodes.values().map(|n| n.size).sum()
    }
    pub fn new(root: &Path) -> Result<Self, String> {
        let mut paths = BTreeMap::new();
        paths.insert(String::new(), (true, 0, 0o40555, None));
        for (path, entry) in crate::media_source::list(root)? {
            let path = path.trim_matches('/').to_string();
            if path.split('/').any(|part| part == ".." || part == ".") {
                return Err("Unsafe NFS media path".into());
            }
            let mut parent = Path::new(&path).parent();
            while let Some(p) = parent {
                paths
                    .entry(p.to_string_lossy().into_owned())
                    .or_insert((true, 0, 0o40555, None));
                parent = p.parent();
            }
            paths.insert(path, (entry.is_dir, entry.size, entry.mode, entry.link));
        }
        let mut fs = Self {
            root: root.into(),
            nodes: BTreeMap::new(),
            paths: BTreeMap::new(),
        };
        for (index, (path, (directory, size, mode, link))) in paths.into_iter().enumerate() {
            let id = index as u64 + 1;
            fs.paths.insert(path.clone(), id);
            fs.nodes.insert(
                id,
                Node {
                    path,
                    directory,
                    size,
                    mode,
                    link,
                },
            );
        }
        Ok(fs)
    }
    fn node(&self, id: u64) -> Result<&Node, nfsstat3> {
        self.nodes.get(&id).ok_or(nfsstat3::NFS3ERR_STALE)
    }
    fn directory(&self, id: u64) -> Result<&Node, nfsstat3> {
        let n = self.node(id)?;
        if n.directory {
            Ok(n)
        } else {
            Err(nfsstat3::NFS3ERR_NOTDIR)
        }
    }
    fn attr(&self, id: u64) -> Result<fattr3, nfsstat3> {
        let n = self.node(id)?;
        Ok(fattr3 {
            ftype: if n.link.is_some() {
                ftype3::NF3LNK
            } else if n.directory {
                ftype3::NF3DIR
            } else {
                ftype3::NF3REG
            },
            mode: (n.mode as u32 & 0o777) & !0o222,
            nlink: if n.directory { 2 } else { 1 },
            size: n.size,
            used: n.size,
            fileid: id,
            fsid: 1,
            ..Default::default()
        })
    }
}
#[async_trait]
impl NFSFileSystem for MediaFs {
    fn capabilities(&self) -> VFSCapabilities {
        VFSCapabilities::ReadOnly
    }
    fn root_dir(&self) -> u64 {
        1
    }
    async fn lookup(&self, dirid: u64, filename: &filename3) -> Result<u64, nfsstat3> {
        let dir = self.directory(dirid)?;
        let name = std::str::from_utf8(filename.as_ref()).map_err(|_| nfsstat3::NFS3ERR_INVAL)?;
        let path = match name {
            "." => dir.path.clone(),
            ".." => Path::new(&dir.path)
                .parent()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default(),
            _ => {
                if name.is_empty() || name.contains('/') || name.contains('\0') {
                    return Err(nfsstat3::NFS3ERR_INVAL);
                }
                if dir.path.is_empty() {
                    name.into()
                } else {
                    format!("{}/{name}", dir.path)
                }
            }
        };
        self.paths
            .get(&path)
            .copied()
            .ok_or(nfsstat3::NFS3ERR_NOENT)
    }
    async fn getattr(&self, id: u64) -> Result<fattr3, nfsstat3> {
        self.attr(id)
    }
    async fn read(&self, id: u64, offset: u64, count: u32) -> Result<(Vec<u8>, bool), nfsstat3> {
        let n = self.node(id)?;
        if n.directory {
            return Err(nfsstat3::NFS3ERR_ISDIR);
        }
        if n.link.is_some() {
            return Err(nfsstat3::NFS3ERR_INVAL);
        }
        if offset >= n.size {
            return Ok((Vec::new(), true));
        }
        let mut resource =
            crate::media_source::open(&self.root, &n.path).map_err(|_| nfsstat3::NFS3ERR_IO)?;
        resource
            .seek(SeekFrom::Start(offset))
            .map_err(|_| nfsstat3::NFS3ERR_IO)?;
        let length = (n.size - offset).min(count as u64).min(1024 * 1024) as usize;
        let mut data = vec![0; length];
        resource
            .read_exact(&mut data)
            .map_err(|_| nfsstat3::NFS3ERR_IO)?;
        Ok((data, offset + length as u64 >= n.size))
    }
    async fn readdir(
        &self,
        dirid: u64,
        start_after: u64,
        max_entries: usize,
    ) -> Result<ReadDirResult, nfsstat3> {
        let dir = self.directory(dirid)?;
        let children: Vec<_> = self
            .nodes
            .iter()
            .filter(|(_, n)| {
                !n.path.is_empty()
                    && Path::new(&n.path)
                        .parent()
                        .is_some_and(|p| p == Path::new(&dir.path))
            })
            .collect();
        let start = if start_after == 0 {
            0
        } else {
            children
                .iter()
                .position(|(id, _)| **id == start_after)
                .ok_or(nfsstat3::NFS3ERR_BAD_COOKIE)?
                + 1
        };
        let end = (start + max_entries.min(4096)).min(children.len());
        let mut entries = Vec::new();
        for (id, n) in &children[start..end] {
            entries.push(DirEntry {
                fileid: **id,
                name: Path::new(&n.path)
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .as_bytes()
                    .into(),
                attr: self.attr(**id)?,
            });
        }
        Ok(ReadDirResult {
            entries,
            end: end == children.len(),
        })
    }
    async fn readlink(&self, id: u64) -> Result<nfspath3, nfsstat3> {
        let n = self.node(id)?;
        n.link
            .as_ref()
            .map(|s| s.as_bytes().into())
            .ok_or(nfsstat3::NFS3ERR_INVAL)
    }
    async fn setattr(&self, _: u64, _: sattr3) -> Result<fattr3, nfsstat3> {
        Err(nfsstat3::NFS3ERR_ROFS)
    }
    async fn write(&self, _: u64, _: u64, _: &[u8]) -> Result<fattr3, nfsstat3> {
        Err(nfsstat3::NFS3ERR_ROFS)
    }
    async fn create(&self, _: u64, _: &filename3, _: sattr3) -> Result<(u64, fattr3), nfsstat3> {
        Err(nfsstat3::NFS3ERR_ROFS)
    }
    async fn create_exclusive(&self, _: u64, _: &filename3) -> Result<u64, nfsstat3> {
        Err(nfsstat3::NFS3ERR_ROFS)
    }
    async fn mkdir(&self, _: u64, _: &filename3) -> Result<(u64, fattr3), nfsstat3> {
        Err(nfsstat3::NFS3ERR_ROFS)
    }
    async fn remove(&self, _: u64, _: &filename3) -> Result<(), nfsstat3> {
        Err(nfsstat3::NFS3ERR_ROFS)
    }
    async fn rename(&self, _: u64, _: &filename3, _: u64, _: &filename3) -> Result<(), nfsstat3> {
        Err(nfsstat3::NFS3ERR_ROFS)
    }
    async fn symlink(
        &self,
        _: u64,
        _: &filename3,
        _: &nfspath3,
        _: &sattr3,
    ) -> Result<(u64, fattr3), nfsstat3> {
        Err(nfsstat3::NFS3ERR_ROFS)
    }
}

// RFC 1057 Appendix A: fixed registrations, never allow remote mutations.
/// RPC bootparams has no fixed well-known port; the wire adapter advertises 10000.
pub const WIRE_BOOTPARAM_PORT: u16 = 10000;
pub fn wire_portmap(packet: &[u8]) -> Option<Vec<u8>> {
    portmap_reply_ports(packet, 2049, WIRE_BOOTPARAM_PORT, 111, false)
}
fn portmap_reply_ports(
    packet: &[u8],
    port: u16,
    bootport: u16,
    mapper_port: u16,
    tcp: bool,
) -> Option<Vec<u8>> {
    fn word(p: &[u8], pos: &mut usize) -> Option<u32> {
        let b = p.get(*pos..*pos + 4)?;
        *pos += 4;
        Some(u32::from_be_bytes(b.try_into().ok()?))
    }
    fn push(out: &mut Vec<u8>, v: u32) {
        out.extend(v.to_be_bytes())
    }
    let mut pos = 0;
    let xid = word(packet, &mut pos)?;
    if word(packet, &mut pos)? != 0 || word(packet, &mut pos)? != 2 {
        return None;
    }
    let prog = word(packet, &mut pos)?;
    let vers = word(packet, &mut pos)?;
    let proc = word(packet, &mut pos)?;
    for _ in 0..2 {
        word(packet, &mut pos)?;
        let len = word(packet, &mut pos)? as usize;
        if len > 400 {
            return None;
        }
        pos = pos.checked_add((len + 3) & !3)?;
        packet.get(..pos)?;
    }
    let mut out = Vec::new();
    for v in [xid, 1, 0, 0, 0] {
        push(&mut out, v)
    }
    if prog != 100000 {
        push(&mut out, 1);
        return Some(out);
    }
    if vers != 2 {
        for v in [2, 2, 2] {
            push(&mut out, v)
        }
        return Some(out);
    }
    push(&mut out, 0);
    match proc {
        0 => {}
        1 | 2 => push(&mut out, 0),
        3 => {
            let program = word(packet, &mut pos)?;
            let version = word(packet, &mut pos)?;
            let protocol = word(packet, &mut pos)?;
            word(packet, &mut pos)?;
            push(
                &mut out,
                if (tcp
                    && (program == 100003 || program == 100005)
                    && version == 3
                    && protocol == 6)
                    || (program == 100003 && version == 2 && protocol == 17)
                    || (program == 100005 && version == 1 && protocol == 17)
                {
                    port as u32
                } else if program == 100026 && version == 1 && protocol == 17 {
                    bootport as u32
                } else if program == 100000 && version == 2 && (protocol == 6 || protocol == 17) {
                    mapper_port as u32
                } else {
                    0
                },
            );
        }
        4 => {
            if tcp {
                for program in [100003, 100005] {
                    for v in [1, program, 3, 6, port as u32] {
                        push(&mut out, v)
                    }
                }
            }
            if bootport != 0 {
                for v in [1, 100026, 1, 17, bootport as u32] {
                    push(&mut out, v)
                }
            }
            for (program, version) in [(100003, 2), (100005, 1)] {
                for v in [1, program, version, 17, port as u32] {
                    push(&mut out, v)
                }
            }
            push(&mut out, 0)
        }
        _ => {
            out.truncate(20);
            push(&mut out, 3)
        }
    }
    Some(out)
}

/// Start RFC1057 UDP and TCP discovery, returning handles owned by the caller.
/// Bind failures propagate instead of silently falling back to a private port.
async fn portmapper_assigned(
    bind: &str,
    nfs_port: u16,
    state: Option<crate::model::Shared>,
) -> Result<Vec<tokio::task::JoinHandle<()>>, String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mapper_port = bind
        .parse::<std::net::SocketAddr>()
        .map_err(|e| e.to_string())?
        .port();
    let address: std::net::SocketAddrV4 = bind
        .parse()
        .map_err(|e: std::net::AddrParseError| e.to_string())?;
    let udp = crate::privilege::udp((*address.ip(), address.port())).await?;
    let tcp = crate::privilege::tcp((*address.ip(), address.port())).await?;
    let tcp_state = state.clone();
    let udp_task = tokio::spawn(async move {
        let mut buf = [0u8; 4096];
        while let Ok((n, peer)) = udp.recv_from(&mut buf).await {
            let callit = state.as_ref().and_then(|state| {
                let std::net::IpAddr::V4(ip) = peer.ip() else {
                    return None;
                };
                crate::bootparams::callit(state, ip, &buf[..n])
            });
            let bootport = state
                .as_ref()
                .and_then(|s| crate::bootparams::configured_port(s).ok().flatten())
                .unwrap_or(0);
            if let Some(reply) = callit
                .or_else(|| portmap_reply_ports(&buf[..n], nfs_port, bootport, mapper_port, true))
            {
                let _ = udp.send_to(&reply, peer).await;
            }
        }
    });
    let tcp_task = tokio::spawn(async move {
        let mut children = tokio::task::JoinSet::new();
        while let Ok((mut stream, _)) = tcp.accept().await {
            let _ = stream.set_nodelay(true);
            while children.try_join_next().is_some() {}
            let state = tcp_state.clone();
            children.spawn(async move {
                loop {
                    let mut packet = Vec::new();
                    loop {
                        let Ok(marker) = stream.read_u32().await else {
                            return;
                        };
                        let len = (marker & 0x7fffffff) as usize;
                        if packet.len() + len > 4096 {
                            return;
                        }
                        let at = packet.len();
                        packet.resize(at + len, 0);
                        if stream.read_exact(&mut packet[at..]).await.is_err() {
                            return;
                        }
                        if marker & 0x80000000 != 0 {
                            break;
                        }
                    }
                    let bootport = state
                        .as_ref()
                        .and_then(|s| crate::bootparams::configured_port(s).ok().flatten())
                        .unwrap_or(0);
                    let Some(reply) =
                        portmap_reply_ports(&packet, nfs_port, bootport, mapper_port, true)
                    else {
                        return;
                    };
                    let mut record = Vec::with_capacity(4 + reply.len());
                    record.extend_from_slice(&(0x80000000 | reply.len() as u32).to_be_bytes());
                    record.extend_from_slice(&reply);
                    if stream.write_all(&record).await.is_err() {
                        return;
                    }
                }
            });
        }
    });
    Ok(vec![udp_task, tcp_task])
}

/// Explicit opt-in only. Public TCP traffic is routed by the assigned client's
/// IPv4 address; each backend exports only that client's selected medium.
pub type Backends = std::sync::Arc<std::sync::Mutex<BTreeMap<(String, PathBuf), u16>>>;

pub async fn prepare_exports(
    config: &crate::model::Config,
    backends: &Backends,
) -> Result<Vec<tokio::task::JoinHandle<()>>, String> {
    let mut pending = Vec::new();
    for profile in config.profiles.iter().filter(|p| {
        eligible(p)
            && (std::env::var("BOOTAH_NFS_PORT").is_ok()
                || config.default_profiles.values().any(|id| id == &p.id)
                || config
                    .devices
                    .iter()
                    .any(|d| d.profile_id.as_ref() == Some(&p.id)))
    }) {
        let key = (profile.id.clone(), profile.root.clone());
        if backends.lock().unwrap().contains_key(&key) {
            continue;
        }
        let mut listener = NFSTcpListener::bind("127.0.0.1:0", MediaFs::new(&profile.root)?)
            .await
            .map_err(|e| e.to_string())?;
        listener.with_export_name(&profile.id);
        pending.push((key, listener));
    }
    let mut tasks = Vec::new();
    for (key, listener) in pending {
        backends
            .lock()
            .unwrap()
            .insert(key, listener.get_listen_port());
        tasks.push(tokio::spawn(async move {
            let _ = listener.handle_forever().await;
        }));
    }
    Ok(tasks)
}

pub async fn start(
    state: crate::model::Shared,
    config: &crate::model::Config,
    backends: Backends,
) -> Result<Vec<tokio::task::JoinHandle<()>>, String> {
    use std::net::Ipv4Addr;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpStream,
        task::JoinSet,
    };
    let Some(port) = configured_port_for(config)? else {
        return Ok(Vec::new());
    };
    let bind = config.settings.bind_ip;
    let public = crate::privilege::tcp((bind, port)).await?;
    let maps = if let Some(map_port) = configured_override("BOOTAH_PORTMAP_PORT", Some(111))? {
        Some(portmapper_assigned(&format!("{bind}:{map_port}"), port, Some(state.clone())).await?)
    } else {
        None
    };
    struct Cleanup(Vec<tokio::task::JoinHandle<()>>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            for handle in &self.0 {
                handle.abort()
            }
        }
    }
    let mut cleanup = Cleanup(maps.unwrap_or_default());
    cleanup
        .0
        .extend(crate::nfs2::start(state.clone(), port).await?);
    let task = tokio::spawn(async move {
        let mut tasks = JoinSet::new();
        loop {
            tokio::select! {
                accepted=public.accept()=>{
                    let Ok((mut client,peer))=accepted else {break};
                    let std::net::IpAddr::V4(ip)=peer.ip() else {continue};
                    let assignment=|ip:Ipv4Addr|state.lock().ok()?.profile_for(ip).filter(eligible).map(|p|(p.id,p.root));
                    let Some(key)=assignment(ip) else {continue};
                    let Some(backend_port)=backends.lock().unwrap().get(&key).copied() else {continue};
                    let state=state.clone();
                    tasks.spawn(async move {
                        let Ok(mut backend)=TcpStream::connect((Ipv4Addr::LOCALHOST,backend_port)).await else {return};
                        let _ = client.set_nodelay(true);
                        let (mut client_read, mut client_write) = client.split();
                        let (mut backend_read, mut backend_write) = backend.split();
                        let responses = async {
                            loop {
                                let mut record = Vec::new();
                                loop {
                                    let marker = backend_read.read_u32().await?;
                                    let length = (marker & 0x7fffffff) as usize;
                                    if record.len() + length > 16 * 1024 * 1024 {
                                        return Err::<(), std::io::Error>(std::io::ErrorKind::InvalidData.into());
                                    }
                                    let at = record.len();
                                    record.resize(at + length, 0);
                                    backend_read.read_exact(&mut record[at..]).await?;
                                    if marker & 0x80000000 != 0 { break; }
                                }
                                let mut framed = Vec::with_capacity(4 + record.len());
                                framed.extend_from_slice(&(0x80000000 | record.len() as u32).to_be_bytes());
                                framed.extend_from_slice(&record);
                                client_write.write_all(&framed).await?;
                            }
                        };
                        let revoked = async {
                            loop {
                                let selected=state.lock().ok().and_then(|s|s.profile_for(ip)).filter(eligible).map(|p|(p.id,p.root));
                                if selected.as_ref()!=Some(&key){break}
                                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                            }
                        };
                        tokio::select! {
                            _=tokio::io::copy(&mut client_read, &mut backend_write)=>{},
                            _=responses=>{},
                            _=revoked=>{},
                        }
                    });
                },
                _=tasks.join_next(),if !tasks.is_empty()=>{},
            }
        }
    });
    let mut handles = std::mem::take(&mut cleanup.0);
    handles.push(task);
    Ok(handles)
}

/// Only profiles with a known network-root adapter may export their media tree.
pub fn eligible(profile: &crate::model::Profile) -> bool {
    if profile.kind == "linux" {
        return profile
            .boot_evidence
            .as_ref()
            .is_some_and(|e| e.source == "nfs-tree")
            && profile
                .arguments
                .split_whitespace()
                .any(|s| s == "netboot=nfs");
    }
    if profile.kind == "solaris" {
        return true;
    }
    if profile.kind != "bsd"
        || !["alpha", "sparc64", "ppc"].contains(&profile.architecture.as_str())
    {
        return false;
    }
    let Ok(bytes) = std::fs::read(profile.root.join(".bootah-netbsd-nfs.json")) else {
        return false;
    };
    let Ok(marker) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return false;
    };
    if profile.architecture == "ppc" && marker["port"].as_str() != Some("macppc") {
        return false;
    }
    marker["version"] == 1
        && marker["profile"].as_str() == Some(&profile.id)
        && marker["architecture"].as_str() == Some(&profile.architecture)
}

pub fn configured_override(name: &str, default: Option<u16>) -> Result<Option<u16>, String> {
    match std::env::var(name) {
        Err(std::env::VarError::NotPresent) => Ok(default),
        Ok(value) if ["off", "none"].contains(&value.trim().to_ascii_lowercase().as_str()) => {
            Ok(None)
        }
        Ok(value) => {
            let port: u16 = value
                .parse()
                .map_err(|_| format!("Invalid {name}: expected nonzero port or off"))?;
            if port == 0 {
                Err(format!("{name} must be nonzero"))
            } else {
                Ok(Some(port))
            }
        }
        Err(e) => Err(e.to_string()),
    }
}
pub fn configured_port_for(config: &crate::model::Config) -> Result<Option<u16>, String> {
    let (enabled, casper) = {
        let assigned: Vec<_> = config
            .profiles
            .iter()
            .filter(|p| {
                eligible(p)
                    && (config
                        .devices
                        .iter()
                        .any(|d| d.profile_id.as_ref() == Some(&p.id))
                        || config.default_profiles.values().any(|id| id == &p.id))
            })
            .collect();
        (
            !assigned.is_empty(),
            assigned.iter().any(|p| p.kind == "linux"),
        )
    };
    let port = configured_override("BOOTAH_NFS_PORT", enabled.then_some(2049))?;
    if casper
        && (port.is_none() || configured_override("BOOTAH_PORTMAP_PORT", Some(111))? != Some(111))
    {
        return Err("Casper NFS requires NFS and portmapper on port 111.".into());
    }
    Ok(port)
}
