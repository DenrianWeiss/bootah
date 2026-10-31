//! Read-only NFS/UDP: NFSv2/MOUNTv1 plus the shared NFSv3/MOUNTv3 backend.
use crate::{model::Shared, nfs::MediaFs};
use nfsserve::{
    nfs::{fattr3, ftype3, nfsstat3},
    vfs::NFSFileSystem,
};
use std::{
    collections::{BTreeMap, hash_map::DefaultHasher},
    hash::{Hash, Hasher},
    net::Ipv4Addr,
};
fn put(out: &mut Vec<u8>, v: u32) {
    out.extend(v.to_be_bytes())
}
fn opaque(out: &mut Vec<u8>, data: &[u8]) {
    put(out, data.len() as u32);
    out.extend(data);
    while !out.len().is_multiple_of(4) {
        out.push(0)
    }
}
struct Xdr<'a> {
    data: &'a [u8],
    pos: usize,
}
impl<'a> Xdr<'a> {
    fn word(&mut self) -> Option<u32> {
        let b = self.data.get(self.pos..self.pos + 4)?;
        self.pos += 4;
        Some(u32::from_be_bytes(b.try_into().ok()?))
    }
    fn bytes(&mut self, n: usize) -> Option<&'a [u8]> {
        let b = self.data.get(self.pos..self.pos.checked_add(n)?)?;
        self.pos += n;
        Some(b)
    }
    fn string(&mut self, limit: usize) -> Option<String> {
        let n = self.word()? as usize;
        if n > limit {
            return None;
        }
        let b = self.bytes(n)?;
        self.bytes((4 - n % 4) % 4)?;
        Some(std::str::from_utf8(b).ok()?.into())
    }
}
struct Export {
    fs: MediaFs,
    key: u64,
}
fn status(error: nfsstat3) -> u32 {
    match error {
        nfsstat3::NFS3ERR_BADHANDLE | nfsstat3::NFS3ERR_STALE => 70,
        nfsstat3::NFS3ERR_BAD_COOKIE | nfsstat3::NFS3ERR_INVAL => 5,
        _ => {
            let n = error as u32;
            if n < 10000 { n } else { 5 }
        }
    }
}
fn attributes(out: &mut Vec<u8>, a: fattr3) {
    let kind = a.ftype as u32;
    let bits = match a.ftype {
        ftype3::NF3DIR => 0o40000,
        ftype3::NF3LNK => 0o120000,
        _ => 0o100000,
    };
    for v in [
        kind,
        a.mode | bits,
        a.nlink,
        a.uid,
        a.gid,
        a.size.min(u32::MAX as u64) as u32,
        8192,
        0,
        a.used.div_ceil(512).min(u32::MAX as u64) as u32,
        a.fsid as u32,
        a.fileid as u32,
        a.atime.seconds,
        a.atime.nseconds / 1000,
        a.mtime.seconds,
        a.mtime.nseconds / 1000,
        a.ctime.seconds,
        a.ctime.nseconds / 1000,
    ] {
        put(out, v)
    }
}
fn handle(export: &Export, generation: u64, id: u64) -> [u8; 32] {
    let mut b = [0; 32];
    b[..8].copy_from_slice(&generation.to_be_bytes());
    b[8..16].copy_from_slice(&export.key.to_be_bytes());
    b[16..24].copy_from_slice(&id.to_be_bytes());
    b[24..].copy_from_slice(&(id ^ generation ^ export.key).to_be_bytes());
    b
}
fn fileid(x: &mut Xdr<'_>, export: &Export, generation: u64) -> Result<u64, u32> {
    let bytes = x.bytes(32).ok_or(5u32)?;
    let id = u64::from_be_bytes(bytes[16..24].try_into().unwrap());
    if bytes != handle(export, generation, id) {
        return Err(70);
    }
    Ok(id)
}
fn rpc_error(xid: u32, accept: u32) -> Vec<u8> {
    let mut out = Vec::new();
    for v in [xid, 1, 0, 0, 0, accept] {
        put(&mut out, v)
    }
    out
}
async fn request(packet: &[u8], export: &Export, name: &str, generation: u64) -> Option<Vec<u8>> {
    let mut x = Xdr {
        data: packet,
        pos: 0,
    };
    let xid = x.word()?;
    if x.word()? != 0 || x.word()? != 2 {
        return None;
    }
    let program = x.word()?;
    let version = x.word()?;
    let procedure = x.word()?;
    for _ in 0..2 {
        x.word()?;
        let n = x.word()? as usize;
        if n > 400 {
            return None;
        }
        x.bytes(n.div_ceil(4) * 4)?;
    }
    if program != 100003 && program != 100005 {
        return Some(rpc_error(xid, 1));
    }
    let expected = if program == 100003 { 2 } else { 1 };
    if version != expected {
        let mut out = rpc_error(xid, 2);
        put(&mut out, expected);
        put(&mut out, expected);
        return Some(out);
    }
    let mut out = rpc_error(xid, 0);
    if procedure == 0 {
        return Some(out);
    }
    if program == 100005 {
        match procedure {
            1 => {
                let path = x.string(1024)?;
                let prefix = format!("/{name}");
                let path = path.trim_end_matches('/');
                let relative = if path == prefix {
                    ""
                } else if let Some(p) = path.strip_prefix(&format!("{prefix}/")) {
                    p
                } else {
                    put(&mut out, 13);
                    return Some(out);
                };
                if relative.split('/').any(|p| p == ".." || p == ".") {
                    put(&mut out, 13);
                    return Some(out);
                }
                match export.fs.path_to_id(relative.as_bytes()).await {
                    Ok(id) => {
                        put(&mut out, 0);
                        out.extend(handle(export, generation, id))
                    }
                    Err(e) => put(&mut out, status(e)),
                }
            }
            2 => put(&mut out, 0),
            3 => {
                x.string(1024)?;
            }
            4 => {}
            5 => {
                put(&mut out, 1);
                opaque(&mut out, format!("/{name}").as_bytes());
                put(&mut out, 0);
                put(&mut out, 0)
            }
            _ => return Some(rpc_error(xid, 3)),
        }
        return Some(out);
    }
    if [2, 8, 9, 10, 11, 12, 13, 14, 15].contains(&procedure) {
        put(&mut out, 30);
        return Some(out);
    }
    if ![1, 4, 5, 6, 16, 17].contains(&procedure) {
        return Some(rpc_error(xid, 3));
    }
    let id = match fileid(&mut x, export, generation) {
        Ok(id) => id,
        Err(e) => {
            put(&mut out, e);
            return Some(out);
        }
    };
    match procedure {
        1 => match export.fs.getattr(id).await {
            Ok(a) => {
                put(&mut out, 0);
                attributes(&mut out, a)
            }
            Err(e) => put(&mut out, status(e)),
        },
        4 => {
            let filename = x.string(255)?;
            match export.fs.lookup(id, &filename.as_bytes().into()).await {
                Ok(child) => match export.fs.getattr(child).await {
                    Ok(a) => {
                        put(&mut out, 0);
                        out.extend(handle(export, generation, child));
                        attributes(&mut out, a)
                    }
                    Err(e) => put(&mut out, status(e)),
                },
                Err(e) => put(&mut out, status(e)),
            }
        }
        5 => match export.fs.readlink(id).await {
            Ok(link) => {
                put(&mut out, 0);
                opaque(&mut out, link.as_ref())
            }
            Err(e) => put(&mut out, status(e)),
        },
        6 => {
            let offset = x.word()? as u64;
            let count = x.word()?.min(8192);
            x.word()?;
            if std::env::var_os("BOOTAH_NFS_TRACE").is_some() {
                eprintln!(
                    "NFS2 READ profile={name} xid={xid} file={id} offset={offset} count={count}"
                );
            }
            match export.fs.read(id, offset, count).await {
                Ok((data, _)) => match export.fs.getattr(id).await {
                    Ok(a) => {
                        put(&mut out, 0);
                        attributes(&mut out, a);
                        opaque(&mut out, &data)
                    }
                    Err(e) => put(&mut out, status(e)),
                },
                Err(e) => put(&mut out, status(e)),
            }
        }
        16 => {
            let cookie = x.word()? as u64;
            let count = x.word()?.min(32768) as usize;
            match export
                .fs
                .readdir(id, cookie, (count / 16).clamp(1, 2048))
                .await
            {
                Ok(list) => {
                    put(&mut out, 0);
                    let mut end = list.end;
                    let mut used = 8usize;
                    for entry in list.entries {
                        let n: &[u8] = entry.name.as_ref();
                        let size = 16 + n.len().div_ceil(4) * 4;
                        if used + size > count {
                            end = false;
                            break;
                        }
                        put(&mut out, 1);
                        put(&mut out, entry.fileid as u32);
                        opaque(&mut out, n);
                        put(&mut out, entry.fileid as u32);
                        used += size;
                    }
                    put(&mut out, 0);
                    put(&mut out, u32::from(end));
                }
                Err(e) => put(&mut out, status(e)),
            }
        }
        17 => match export.fs.getattr(id).await {
            Ok(_) => {
                for v in [
                    0,
                    8192,
                    8192,
                    export.fs.total_bytes().div_ceil(8192).min(u32::MAX as u64) as u32,
                    0,
                    0,
                ] {
                    put(&mut out, v)
                }
            }
            Err(e) => put(&mut out, status(e)),
        },
        _ => {}
    }
    Some(out)
}

pub struct Server {
    exports: BTreeMap<String, (std::path::PathBuf, Export)>,
    generation: u64,
    state: Shared,
    v3: BTreeMap<String, (std::path::PathBuf, crate::nfs::DatagramExport)>,
    trace_bytes: u64,
    trace_offset: Option<u32>,
}
impl Server {
    pub fn new(state: Shared) -> Self {
        let generation = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        Self {
            exports: BTreeMap::new(),
            generation,
            state,
            v3: BTreeMap::new(),
            trace_bytes: 0,
            trace_offset: None,
        }
    }
    pub async fn handle(&mut self, ip: Ipv4Addr, packet: &[u8]) -> Option<Vec<u8>> {
        let profile = self
            .state
            .lock()
            .ok()?
            .profile_for(ip)
            .filter(crate::nfs::eligible)?;
        if packet.len() >= 24 && packet[4..12] == [0,0,0,0,0,0,0,2]
            && matches!(u32::from_be_bytes(packet[12..16].try_into().ok()?), 100003 | 100005)
            && packet[16..20] == 3u32.to_be_bytes()
        {
            if self.v3.get(&profile.id).is_none_or(|(root, _)| root != &profile.root) {
                let backend = crate::nfs::DatagramExport::new(&profile.root, &profile.id).await.ok()?;
                self.v3.insert(profile.id.clone(), (profile.root.clone(), backend));
            }
            let response = self.v3.get_mut(&profile.id)?.1.request(packet).await;
            if std::env::var_os("BOOTAH_NFS_TRACE").is_some() {
                let procedure = u32::from_be_bytes(packet[20..24].try_into().ok()?);
                let bytes = response.as_ref().map_or(0, Vec::len) as u64;
                let before = self.trace_bytes;
                self.trace_bytes += bytes;
                if procedure != 6 || before / (8 * 1024 * 1024) != self.trace_bytes / (8 * 1024 * 1024) {
                    eprintln!("NFS3 UDP peer={ip} profile={} proc={procedure} reply={bytes} transferred={}", profile.id, self.trace_bytes);
                }
            }
            if response.is_none() { self.v3.remove(&profile.id); }
            return (assignment(&self.state, ip).as_deref() == Some(&profile.id)).then_some(response).flatten();
        }
        if self
            .exports
            .get(&profile.id)
            .is_none_or(|(root, _)| root != &profile.root)
        {
            let mut hash = DefaultHasher::new();
            profile.id.hash(&mut hash);
            profile.root.hash(&mut hash);
            let export = Export {
                fs: MediaFs::new(&profile.root).ok()?,
                key: hash.finish(),
            };
            self.exports
                .insert(profile.id.clone(), (profile.root.clone(), export));
        }
        let export = &self.exports.get(&profile.id)?.1;
        let out = request(packet, export, &profile.id, self.generation).await?;
        if std::env::var_os("BOOTAH_NFS_TRACE").is_some() {
            let mut x = Xdr {
                data: packet,
                pos: 0,
            };
            let header = (|| {
                let xid = x.word()?;
                x.word()?;
                x.word()?;
                let program = x.word()?;
                let version = x.word()?;
                let procedure = x.word()?;
                for _ in 0..2 {
                    x.word()?;
                    let n = x.word()? as usize;
                    x.bytes(n.div_ceil(4) * 4)?;
                }
                Some((xid, program, version, procedure))
            })();
            if let Some((xid, program, version, procedure)) = header {
                if program == 100003 && procedure == 6 {
                    if x.bytes(32).is_some()
                        && let (Some(offset), Some(count)) = (x.word(), x.word())
                    {
                        let bytes = out.len().saturating_sub(100) as u64;
                        let old = self.trace_bytes;
                        self.trace_bytes += bytes;
                        if old / (256 * 1024) != self.trace_bytes / (256 * 1024)
                            || self.trace_offset == Some(offset)
                        {
                            eprintln!(
                                "NFS2 TRACE peer={ip} profile={} xid={xid} READ offset={offset} count={count} reply={} transferred={} repeated={}",
                                profile.id,
                                out.len(),
                                self.trace_bytes,
                                self.trace_offset == Some(offset)
                            );
                        }
                        self.trace_offset = Some(offset);
                    }
                } else {
                    eprintln!(
                        "NFS2 TRACE peer={ip} profile={} xid={xid} program={program} version={version} proc={procedure} reply={}",
                        profile.id,
                        out.len()
                    );
                }
            }
        }

        (assignment(&self.state, ip).as_deref() == Some(&profile.id)).then_some(out)
    }
}
pub async fn start(state: Shared, port: u16) -> Result<Vec<tokio::task::JoinHandle<()>>, String> {
    let bind = state
        .lock()
        .map_err(|e| e.to_string())?
        .config
        .settings
        .bind_ip;
    let socket = tokio::net::UdpSocket::bind((bind, port))
        .await
        .map_err(|e| e.to_string())?;
    let mut server = Server::new(state);
    Ok(vec![tokio::spawn(async move {
        let mut data = vec![0; 65536];
        while let Ok((n, peer)) = socket.recv_from(&mut data).await {
            let std::net::IpAddr::V4(ip) = peer.ip() else {
                continue;
            };
            if let Some(out) = server.handle(ip, &data[..n]).await {
                let _ = socket.send_to(&out, peer).await;
            }
        }
    })])
}
fn assignment(state: &Shared, ip: Ipv4Addr) -> Option<String> {
    state
        .lock()
        .ok()?
        .profile_for(ip)
        .filter(crate::nfs::eligible)
        .map(|p| p.id)
}
