use crate::{media_source, model::Shared};
use std::{
    collections::BTreeMap,
    io::{Read, Seek, SeekFrom},
    net::Ipv4Addr,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

const STATUS_NOT_SUPPORTED: u32 = 0xc00000bb;
const STATUS_ACCESS_DENIED: u32 = 0xc0000022;
const STATUS_NO_SUCH_FILE: u32 = 0xc000000f;
const STATUS_INVALID_HANDLE: u32 = 0xc0000008;
const STATUS_INVALID_PARAMETER: u32 = 0xc000000d;

fn u16at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}
fn u32at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}
fn put16(b: &mut Vec<u8>, n: u16) {
    b.extend(n.to_le_bytes());
}
fn put32(b: &mut Vec<u8>, n: u32) {
    b.extend(n.to_le_bytes());
}
fn put64(b: &mut Vec<u8>, n: u64) {
    b.extend(n.to_le_bytes());
}

fn string(b: &[u8], unicode: bool) -> String {
    if unicode {
        let chars = b
            .chunks_exact(2)
            .take_while(|c| c != &[0, 0])
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect::<Vec<_>>();
        String::from_utf16_lossy(&chars)
    } else {
        String::from_utf8_lossy(b.split(|c| *c == 0).next().unwrap_or_default()).into_owned()
    }
}
fn wildcard(pattern: &str, name: &str) -> bool {
    if pattern == "*.*" {
        return true;
    }
    let pattern = pattern.to_ascii_lowercase().into_bytes();
    let name = name.to_ascii_lowercase().into_bytes();
    let mut previous = vec![false; name.len() + 1];
    previous[0] = true;
    for part in pattern {
        let mut current = vec![false; name.len() + 1];
        if part == b'*' {
            current[0] = previous[0];
            for i in 1..current.len() {
                current[i] = previous[i] || current[i - 1];
            }
        } else {
            for i in 1..current.len() {
                current[i] = previous[i - 1] && (part == b'?' || part == name[i - 1]);
            }
        }
        previous = current;
    }
    previous[name.len()]
}
fn tree_path<'a>(w: &[u8], data: &'a [u8], unicode: bool, data_offset: usize) -> Option<String> {
    let password = u16at(w, 6)? as usize;
    let path = data.get(password..)?;
    let alignment = if unicode && (data_offset + password) & 1 != 0 {
        1
    } else {
        0
    };
    Some(string(path.get(alignment..)?, unicode))
}
fn words<'a>(packet: &'a [u8], offset: usize) -> Option<(&'a [u8], &'a [u8])> {
    let count = *packet.get(offset)? as usize;
    let w = packet.get(offset + 1..offset + 1 + count * 2)?;
    let data_len = u16at(packet, offset + 1 + count * 2)? as usize;
    let data = packet.get(offset + 3 + count * 2..offset + 3 + count * 2 + data_len)?;
    Some((w, data))
}
fn response(request: &[u8], status: u32, tid: u16, uid: u16, words: &[u8], data: &[u8]) -> Vec<u8> {
    let mut out = request[..32].to_vec();
    out[5..9].copy_from_slice(&status.to_le_bytes());
    out[9] = 0x80;
    out[10..12].copy_from_slice(&(u16at(request, 10).unwrap_or(0) & 0x8000 | 0x4000).to_le_bytes());
    out[24..26].copy_from_slice(&tid.to_le_bytes());
    out[28..30].copy_from_slice(&uid.to_le_bytes());
    out.push((words.len() / 2) as u8);
    out.extend_from_slice(words);
    put16(&mut out, data.len() as u16);
    out.extend_from_slice(data);
    out
}
fn trans2_response(request: &[u8], tid: u16, uid: u16, parameters: &[u8], data: &[u8]) -> Vec<u8> {
    let mut words = Vec::with_capacity(20);
    let mut bytes = vec![0];
    let parameter_offset = 56;
    bytes.extend_from_slice(parameters);
    bytes.extend(std::iter::repeat_n(
        0,
        (4 - (56 + parameters.len()) % 4) % 4,
    ));
    let data_offset = 55 + bytes.len();
    bytes.extend_from_slice(data);
    put16(&mut words, parameters.len() as u16);
    put16(&mut words, data.len() as u16);
    put16(&mut words, 0);
    put16(&mut words, parameters.len() as u16);
    put16(&mut words, parameter_offset);
    put16(&mut words, 0);
    put16(&mut words, data.len() as u16);
    put16(&mut words, data_offset as u16);
    put16(&mut words, 0);
    put16(&mut words, 0);
    response(request, 0, tid, uid, &words, &bytes)
}
fn query_info(level: u16, size: u64, directory: bool) -> Option<Vec<u8>> {
    let mut data = Vec::new();
    match level {
        0x0101 => {
            let unix = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .ok()?;
            let stamp =
                (unix.as_secs() + 11_644_473_600) * 10_000_000 + unix.subsec_nanos() as u64 / 100;
            for _ in 0..4 {
                put64(&mut data, stamp);
            }
            put32(&mut data, if directory { 0x10 } else { 0x20 });
            put32(&mut data, 0);
        }
        0x0102 => {
            put64(&mut data, size);
            put64(&mut data, size);
            put32(&mut data, 1);
            data.extend([0, u8::from(directory)]);
        }
        _ => return None,
    }
    Some(data)
}

struct OpenFile {
    resource: Option<media_source::Resource>,
    size: u64,
    directory: bool,
}
struct Search {
    entries: Vec<(String, u64, bool)>,
    cursor: usize,
}
struct Connection {
    state: Shared,
    uid: u16,
    tid: u16,
    files: BTreeMap<u16, OpenFile>,
    next_fid: u16,
    searches: BTreeMap<u16, Search>,
    next_sid: u16,
}
impl Connection {
    fn new(state: Shared) -> Self {
        Self {
            state,
            uid: 1,
            tid: 1,
            files: BTreeMap::new(),
            next_fid: 1,
            searches: BTreeMap::new(),
            next_sid: 1,
        }
    }
    fn open(&mut self, path: &str) -> Result<(u16, u64), u32> {
        let parts = path
            .trim_matches(['\\', '/'])
            .split(['\\', '/'])
            .collect::<Vec<_>>();
        if parts.is_empty()
            || parts
                .iter()
                .any(|p| p.is_empty() || *p == "." || *p == "..")
        {
            return Err(STATUS_ACCESS_DENIED);
        }
        let profile = self
            .state
            .lock()
            .unwrap()
            .config
            .profiles
            .iter()
            .find(|p| p.kind == "windows-ris" && p.id.eq_ignore_ascii_case(parts[0]))
            .cloned()
            .ok_or(STATUS_NO_SUCH_FILE)?;
        let (resource, size, directory) = if parts.len() == 1 {
            (None, 0, true)
        } else {
            let name = parts[1..].join("/");
            let resource = media_source::open(&profile.root, &name).or_else(|err| {
                let fallback = crate::windows::ris_fallback_name(&profile, &name).ok_or(err)?;
                media_source::open(&profile.root, &fallback)
            });
            match resource {
                Ok(resource) => {
                    let size = resource.size;
                    (Some(resource), size, false)
                }
                Err(_) => {
                    let (size, directory) = self.path_info(path)?;
                    if !directory {
                        return Err(STATUS_NO_SUCH_FILE);
                    }
                    (None, size, true)
                }
            }
        };
        self.next_fid = self.next_fid.wrapping_add(1).max(1);
        let fid = self.next_fid;
        self.files.insert(
            fid,
            OpenFile {
                resource,
                size,
                directory,
            },
        );
        Ok((fid, size))
    }
    fn path_info(&self, path: &str) -> Result<(u64, bool), u32> {
        let parts = path
            .trim_matches(['\\', '/'])
            .split(['\\', '/'])
            .collect::<Vec<_>>();
        if parts.is_empty()
            || parts
                .iter()
                .any(|p| p.is_empty() || *p == "." || *p == "..")
        {
            return Err(STATUS_ACCESS_DENIED);
        }
        let profile = self
            .state
            .lock()
            .unwrap()
            .config
            .profiles
            .iter()
            .find(|p| p.kind == "windows-ris" && p.id.eq_ignore_ascii_case(parts[0]))
            .cloned()
            .ok_or(STATUS_NO_SUCH_FILE)?;
        if parts.len() == 1 {
            return Ok((0, true));
        }
        let name = parts[1..].join("/");
        if let (Ok(root), Ok(path)) = (
            profile.root.canonicalize(),
            profile.root.join(&name).canonicalize(),
        ) {
            if path.starts_with(&root) {
                if let Ok(metadata) = path.metadata() {
                    if metadata.is_file() || metadata.is_dir() {
                        return Ok((metadata.len(), metadata.is_dir()));
                    }
                }
            }
        }
        let entries = media_source::list(&profile.root).map_err(|_| STATUS_NO_SUCH_FILE)?;
        let fallback = crate::windows::ris_fallback_name(&profile, &name);
        entries
            .into_iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(&name))
            .or_else(|| {
                fallback.as_ref().and_then(|fallback| {
                    media_source::list(&profile.root)
                        .ok()?
                        .into_iter()
                        .find(|(key, _)| key.eq_ignore_ascii_case(fallback))
                })
            })
            .map(|(_, entry)| (entry.size, entry.is_dir))
            .ok_or(STATUS_NO_SUCH_FILE)
    }
    fn find(&self, path: &str) -> Result<Vec<(String, u64, bool)>, u32> {
        let normalized = path.trim_matches(['\\', '/']).replace('\\', "/");
        let (directory, pattern) = normalized.rsplit_once('/').ok_or(STATUS_ACCESS_DENIED)?;
        if directory.is_empty() || directory.split('/').any(|part| part == "." || part == "..") {
            return Err(STATUS_ACCESS_DENIED);
        }
        if !self.path_info(directory)?.1 {
            return Err(STATUS_NO_SUCH_FILE);
        }
        let (profile_id, subdir) = directory.split_once('/').unwrap_or((directory, ""));
        let profile = self
            .state
            .lock()
            .unwrap()
            .config
            .profiles
            .iter()
            .find(|p| p.kind == "windows-ris" && p.id.eq_ignore_ascii_case(profile_id))
            .cloned()
            .ok_or(STATUS_NO_SUCH_FILE)?;
        let mut entries = Vec::new();
        for (name, entry) in media_source::list(&profile.root).map_err(|_| STATUS_NO_SUCH_FILE)? {
            let (parent, basename) = name.rsplit_once('/').unwrap_or(("", name.as_str()));
            if parent.eq_ignore_ascii_case(subdir) && wildcard(pattern, basename) {
                entries.push((basename.to_string(), entry.size, entry.is_dir));
            }
        }
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        if entries.is_empty() {
            if let Some(fallback) = crate::windows::ris_fallback_name(&profile, subdir) {
                for (name, entry) in
                    media_source::list(&profile.root).map_err(|_| STATUS_NO_SUCH_FILE)?
                {
                    let (parent, basename) = name.rsplit_once('/').unwrap_or(("", name.as_str()));
                    if parent.eq_ignore_ascii_case(&fallback) && wildcard(pattern, basename) {
                        entries.push((basename.to_string(), entry.size, entry.is_dir));
                    }
                }
            }
            if entries.is_empty() {
                return Err(STATUS_NO_SUCH_FILE);
            }
        }
        Ok(entries)
    }
    fn find_reply(
        &mut self,
        request: &[u8],
        tid: u16,
        uid: u16,
        sid: u16,
        limit: usize,
        count: usize,
        unicode: bool,
        first: bool,
    ) -> Option<Vec<u8>> {
        let search = self.searches.get_mut(&sid)?;
        let mut data = Vec::new();
        let mut returned = 0usize;
        let mut last_start = 0usize;
        while search.cursor < search.entries.len() && returned < count {
            let (name, size, directory) = &search.entries[search.cursor];
            let encoded = if unicode {
                name.encode_utf16()
                    .flat_map(u16::to_le_bytes)
                    .collect::<Vec<_>>()
            } else {
                name.as_bytes().to_vec()
            };
            let len = (94 + encoded.len() + 3) & !3;
            if data.len() + len > limit {
                break;
            }
            let start = data.len();
            last_start = start;
            put32(&mut data, len as u32);
            put32(&mut data, 0);
            let stamp = 116_444_736_000_000_000u64;
            for _ in 0..4 {
                put64(&mut data, stamp);
            }
            put64(&mut data, *size);
            put64(&mut data, *size);
            put32(&mut data, if *directory { 0x10 } else { 0x20 });
            put32(&mut data, encoded.len() as u32);
            put32(&mut data, 0);
            data.extend([0; 26]);
            data.extend(encoded);
            data.resize(start + len, 0);
            search.cursor += 1;
            returned += 1;
        }
        if returned == 0 {
            return None;
        }
        let end = search.cursor == search.entries.len();
        data[last_start..last_start + 4].copy_from_slice(&0u32.to_le_bytes());
        let mut parameters = Vec::new();
        if first {
            put16(&mut parameters, if end { 0 } else { sid });
        }
        put16(&mut parameters, returned as u16);
        put16(&mut parameters, u16::from(end));
        put16(&mut parameters, 0);
        put16(&mut parameters, 0);
        if end {
            self.searches.remove(&sid);
        }
        Some(trans2_response(request, tid, uid, &parameters, &data))
    }
    fn command(&mut self, packet: &[u8]) -> Option<Vec<u8>> {
        if packet.len() < 35 || &packet[..4] != b"\xffSMB" {
            return None;
        }
        let cmd = packet[4];
        let (w, data) = words(packet, 32)?;
        let tid = u16at(packet, 24)?;
        let uid = u16at(packet, 28)?;
        let unicode = u16at(packet, 10)? & 0x8000 != 0;
        let mut out_w = Vec::new();
        let mut out_data = Vec::new();
        let mut status = 0;
        let mut out_tid = tid;
        let mut out_uid = uid;
        match cmd {
            0x72 => {
                let mut dialect = 0xffff;
                for (index, item) in data.split(|b| *b == 2).skip(1).enumerate() {
                    if item.strip_suffix(&[0]) == Some(b"NT LM 0.12") {
                        dialect = index as u16;
                        break;
                    }
                }
                if dialect == 0xffff {
                    put16(&mut out_w, dialect);
                } else {
                    put16(&mut out_w, dialect);
                    out_w.push(3); // user-level access with NTLM challenge; session maps to guest
                    put16(&mut out_w, 1);
                    put16(&mut out_w, 1);
                    put32(&mut out_w, 64000);
                    put32(&mut out_w, 65536);
                    put32(&mut out_w, 0);
                    put32(&mut out_w, 0x70); // NT SMBs, remote APIs, NT status
                    put64(&mut out_w, 0);
                    put16(&mut out_w, 0);
                    out_w.push(8);
                    out_data.extend_from_slice(&uuid::Uuid::new_v4().as_bytes()[..8]);
                }
            }
            0x73 => {
                out_uid = self.uid;
                let chained_tree = w.first() == Some(&0x75)
                    && u16at(w, 2)
                        .is_some_and(|next| next as usize > 32 && (next as usize) < packet.len());
                out_w.extend([if chained_tree { 0x75 } else { 0xff }, 0, 0, 0]);
                put16(&mut out_w, 1); // guest
                if chained_tree {
                    let next = u16at(w, 2).unwrap() as usize;
                    if let Some((tree_words, tree_data)) = words(packet, next) {
                        let data_offset = next + 3 + tree_words.len();
                        if tree_path(tree_words, tree_data, unicode, data_offset)
                            .is_some_and(|path| path.to_ascii_lowercase().ends_with("\\reminst"))
                        {
                            let tree_start = 32 + 1 + out_w.len() + 2 + out_data.len();
                            out_w[2..4].copy_from_slice(&(tree_start as u16).to_le_bytes());
                            let mut combined =
                                response(packet, 0, self.tid, out_uid, &out_w, &out_data);
                            combined.push(3);
                            combined.extend([0xff, 0, 0, 0, 0, 0]);
                            put16(&mut combined, 0);
                            return Some(combined);
                        }
                    }
                    status = STATUS_ACCESS_DENIED;
                }
            }
            0x75 => {
                if uid != self.uid {
                    status = STATUS_ACCESS_DENIED;
                } else if !tree_path(w, data, unicode, 32 + 3 + w.len())
                    .is_some_and(|path| path.to_ascii_lowercase().ends_with("\\reminst"))
                {
                    status = STATUS_ACCESS_DENIED;
                } else {
                    out_tid = self.tid;
                    out_w.extend([0xff, 0, 0, 0]);
                    put16(&mut out_w, 0);
                    out_data.extend(b"A:\0NTFS\0");
                }
            }
            0xa2 => {
                if tid != self.tid || uid != self.uid || w.len() < 48 {
                    status = STATUS_ACCESS_DENIED;
                } else {
                    let name_len = u16at(w, 5).unwrap_or(0) as usize;
                    let data = if unicode && (32 + 3 + w.len()) & 1 != 0 {
                        data.get(1..).unwrap_or(data)
                    } else {
                        data
                    };
                    let name = string(data.get(..name_len).unwrap_or(data), unicode);
                    match self.open(&name) {
                        Ok((fid, size)) => {
                            let directory = self.files.get(&fid).is_some_and(|f| f.directory);
                            out_w.extend([0xff, 0, 0, 0, 0]);
                            put16(&mut out_w, fid);
                            put32(&mut out_w, 1);
                            out_w.extend([0; 32]);
                            put32(&mut out_w, if directory { 0x10 } else { 0x20 });
                            put64(&mut out_w, size);
                            put64(&mut out_w, size);
                            put16(&mut out_w, 0);
                            put16(&mut out_w, 0);
                            out_w.push(0);
                        }
                        Err(e) => status = e,
                    }
                }
            }
            0x2e => {
                let fid = u16at(w, 4)?;
                let offset = u32at(w, 6)? as u64 | ((u32at(w, 20).unwrap_or(0) as u64) << 32);
                let count = u16at(w, 10)?.min(60000) as usize;
                if let Some(file) = self.files.get_mut(&fid).and_then(|f| f.resource.as_mut()) {
                    let mut bytes = Vec::with_capacity(count);
                    if file.seek(SeekFrom::Start(offset)).is_err() {
                        status = STATUS_INVALID_PARAMETER;
                    } else if (&mut *file)
                        .take(count as u64)
                        .read_to_end(&mut bytes)
                        .is_ok()
                    {
                        out_w.extend([0xff, 0, 0, 0]);
                        put16(&mut out_w, 0);
                        put16(&mut out_w, 0);
                        put16(&mut out_w, 0);
                        put16(&mut out_w, bytes.len() as u16);
                        put16(&mut out_w, 60);
                        out_w.extend([0; 10]);
                        out_data.push(0);
                        out_data.extend(bytes);
                    } else {
                        status = STATUS_NO_SUCH_FILE;
                    }
                } else {
                    status = STATUS_INVALID_HANDLE;
                }
            }
            0x32 => {
                if tid != self.tid || uid != self.uid || w.len() < 30 || w[26] != 1 {
                    status = STATUS_NOT_SUPPORTED;
                } else {
                    let count = u16at(w, 18)? as usize;
                    let offset = u16at(w, 20)? as usize;
                    let parameters = packet.get(offset..offset.checked_add(count)?)?;
                    let item = match u16at(w, 28)? {
                        1 => {
                            let path = string(parameters.get(12..)?, unicode);
                            match self.find(&path) {
                                Ok(entries) => {
                                    self.next_sid = self.next_sid.wrapping_add(1).max(1);
                                    let sid = self.next_sid;
                                    self.searches.insert(sid, Search { entries, cursor: 0 });
                                    return self.find_reply(
                                        packet,
                                        tid,
                                        uid,
                                        sid,
                                        u16at(w, 6)? as usize,
                                        u16at(parameters, 2)? as usize,
                                        unicode,
                                        true,
                                    );
                                }
                                Err(error) => Err(error),
                            }
                        }
                        2 => {
                            let sid = u16at(parameters, 0)?;
                            return self.find_reply(
                                packet,
                                tid,
                                uid,
                                sid,
                                u16at(w, 6)? as usize,
                                u16at(parameters, 2)? as usize,
                                unicode,
                                false,
                            );
                        }
                        7 => {
                            let fid = u16at(parameters, 0)?;
                            let level = u16at(parameters, 2)?;
                            self.files
                                .get(&fid)
                                .map(|file| Ok((level, file.size, file.directory)))
                                .unwrap_or(Err(STATUS_INVALID_HANDLE))
                        }
                        5 => {
                            let level = u16at(parameters, 0)?;
                            let path = string(parameters.get(6..)?, unicode);
                            self.path_info(&path)
                                .map(|(size, directory)| (level, size, directory))
                        }
                        _ => Err(STATUS_NOT_SUPPORTED),
                    };
                    match item {
                        Ok((level, size, directory)) => {
                            if let Some(data) = query_info(level, size, directory).filter(|data| {
                                u16at(w, 6).is_some_and(|limit| limit as usize >= data.len())
                                    && u16at(w, 4).is_some_and(|limit| limit >= 2)
                            }) {
                                return Some(trans2_response(packet, tid, uid, &[0, 0], &data));
                            }
                            status = STATUS_NOT_SUPPORTED;
                        }
                        Err(error) => status = error,
                    }
                }
            }
            0x04 => {
                if let Some(fid) = u16at(w, 0) {
                    self.files.remove(&fid);
                }
            }
            0x34 => {
                if let Some(sid) = u16at(w, 0) {
                    self.searches.remove(&sid);
                }
            }
            0x71 | 0x74 => {}
            _ => status = STATUS_NOT_SUPPORTED,
        }
        if status != 0 {
            out_w.clear();
            out_data.clear();
        }
        Some(response(
            packet, status, out_tid, out_uid, &out_w, &out_data,
        ))
    }
}

pub(crate) async fn serve(mut stream: TcpStream, state: Shared, netbios: bool) -> Result<(), String> {
    let mut session = Connection::new(state);
    loop {
        let mut header = [0u8; 4];
        if stream.read_exact(&mut header).await.is_err() {
            return Ok(());
        }
        let length =
            ((header[1] as usize & 1) << 16) | ((header[2] as usize) << 8) | header[3] as usize;
        if netbios && header[0] == 0x81 {
            if length > 4096 {
                return Err("Invalid NetBIOS session request".into());
            }
            let mut discard = vec![0; length];
            stream
                .read_exact(&mut discard)
                .await
                .map_err(|e| e.to_string())?;
            stream
                .write_all(&[0x82, 0, 0, 0])
                .await
                .map_err(|e| e.to_string())?;
            continue;
        }
        if header[0] != 0 || length > 1024 * 1024 || length < 35 {
            return Err("Invalid SMB record".into());
        }
        let mut packet = vec![0; length];
        stream
            .read_exact(&mut packet)
            .await
            .map_err(|e| e.to_string())?;
        if let Some(reply) = session.command(&packet) {
            let mut framing = [0u8; 4];
            framing[1] = ((reply.len() >> 16) & 1) as u8;
            framing[2] = (reply.len() >> 8) as u8;
            framing[3] = reply.len() as u8;
            stream
                .write_all(&framing)
                .await
                .map_err(|e| e.to_string())?;
            stream.write_all(&reply).await.map_err(|e| e.to_string())?;
        }
    }
}

pub async fn run(state: Shared, listener: TcpListener, netbios: bool) {
    let mut sessions = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let Ok((stream, _)) = accepted else { break };
                let state = state.clone();
                sessions.spawn(async move { let _ = serve(stream, state, netbios).await; });
            }
            _ = sessions.join_next(), if !sessions.is_empty() => {}
        }
    }
}

pub async fn start(
    state: Shared,
    bind_ip: Ipv4Addr,
    port: u16,
    config: &crate::model::Config,
) -> Result<Vec<tokio::task::JoinHandle<()>>, String> {
    if !config
        .profiles
        .iter()
        .any(|p| p.kind == "windows-ris")
    {
        return Ok(vec![]);
    }
    let direct = crate::privilege::tcp((bind_ip, port))
        .await
        .map_err(|e| format!("SMB {port}: {e}"))?;
    let mut tasks = vec![tokio::spawn(run(state.clone(), direct, false))];
    if port == 445 {
        if let Ok(netbios) = crate::privilege::tcp((bind_ip, 139)).await {
            tasks.push(tokio::spawn(run(state, netbios, true)));
        }
    }
    Ok(tasks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Config, Store};
    use std::sync::{Arc, Mutex};

    fn request(command: u8, words: &[u8], data: &[u8], uid: u16, tid: u16) -> Vec<u8> {
        let mut p = vec![0; 32];
        p[..4].copy_from_slice(b"\xffSMB");
        p[4] = command;
        p[24..26].copy_from_slice(&tid.to_le_bytes());
        p[28..30].copy_from_slice(&uid.to_le_bytes());
        p.push((words.len() / 2) as u8);
        p.extend(words);
        put16(&mut p, data.len() as u16);
        p.extend(data);
        p
    }
    #[test]
    fn negotiate_guest_share() {
        let state = Arc::new(Mutex::new(Store {
            config: Config::default(),
            events: vec![],
            path: "unused".into(),
        }));
        let mut session = Connection::new(state);
        let reply = session
            .command(&request(
                0x72,
                &[],
                b"\x02PC NETWORK PROGRAM 1.0\0\x02NT LM 0.12\0",
                0,
                0,
            ))
            .unwrap();
        assert_eq!(reply[32], 17);
        assert_eq!(u16at(&reply, 33), Some(1));
        assert_eq!(reply[35], 3);
        assert_eq!(u32at(&reply, 52), Some(0x70));
    }
    #[test]
    fn read_only_profile_file() {
        let root = std::env::temp_dir().join(format!("bootah-smb-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("i386")).unwrap();
        std::fs::write(root.join("i386/test.bin"), b"RIS source").unwrap();
        let profile = crate::model::Profile { secure_boot: None,
            id: "xp".into(),
            name: "XP".into(),
            system_id: String::new(),
            system_name: String::new(),
            architecture: "x86".into(),
            kind: "windows-ris".into(),
            root: root.clone(),
            boot_files: Default::default(),
            kernel: String::new(),
            initrd: String::new(),
            dtb: String::new(),
            arguments: String::new(),
            media: String::new(),
            status: String::new(),
            notes: String::new(),
            boot_evidence: None,
        };
        let mut config = Config::default();
        config.profiles.push(profile);
        let state = Arc::new(Mutex::new(Store {
            config,
            events: vec![],
            path: "unused".into(),
        }));
        let mut session = Connection::new(state);
        let name = b"\\xp\\i386\\test.bin\0";
        let mut create = vec![0u8; 48];
        create[5..7].copy_from_slice(&(name.len() as u16).to_le_bytes());
        let created = session
            .command(&request(0xa2, &create, name, 1, 1))
            .unwrap();
        assert_eq!(u32at(&created, 5), Some(0));
        let fid = u16at(&created, 38).unwrap();
        let mut read = vec![0u8; 20];
        read[4..6].copy_from_slice(&fid.to_le_bytes());
        read[10..12].copy_from_slice(&32u16.to_le_bytes());
        let data = session.command(&request(0x2e, &read, &[], 1, 1)).unwrap();
        assert_eq!(u32at(&data, 5), Some(0));
        assert_eq!(u16at(&data, 43), Some(10));
        assert_eq!(u16at(&data, 45), Some(60));
        assert_eq!(&data[60..], b"RIS source");
        let mut query = vec![0u8; 30];
        query[4..6].copy_from_slice(&2u16.to_le_bytes());
        query[6..8].copy_from_slice(&24u16.to_le_bytes());
        query[18..20].copy_from_slice(&4u16.to_le_bytes());
        query[20..22].copy_from_slice(&68u16.to_le_bytes());
        query[26] = 1;
        query[28..30].copy_from_slice(&7u16.to_le_bytes());
        let mut params = vec![0, 0, 0];
        put16(&mut params, fid);
        put16(&mut params, 0x0102);
        let info = session
            .command(&request(0x32, &query, &params, 1, 1))
            .unwrap();
        assert_eq!(u32at(&info, 5), Some(0));
        let offset = u16at(&info, 47).unwrap() as usize;
        assert_eq!(
            u64::from_le_bytes(info[offset + 8..offset + 16].try_into().unwrap()),
            10
        );
        let path = b"\\xp\\i386\0";
        let mut path_params = vec![0, 0, 0];
        put16(&mut path_params, 0x0101);
        put32(&mut path_params, 0);
        path_params.extend(path);
        query[6..8].copy_from_slice(&40u16.to_le_bytes());
        query[18..20].copy_from_slice(&((path_params.len() - 3) as u16).to_le_bytes());
        query[28..30].copy_from_slice(&5u16.to_le_bytes());
        let path_info = session
            .command(&request(0x32, &query, &path_params, 1, 1))
            .unwrap();
        assert_eq!(u32at(&path_info, 5), Some(0));
        let offset = u16at(&path_info, 47).unwrap() as usize;
        assert_eq!(u32at(&path_info, offset + 32), Some(0x10));
        let directory = b"\\xp\0";
        create[5..7].copy_from_slice(&(directory.len() as u16).to_le_bytes());
        let opened = session
            .command(&request(0xa2, &create, directory, 1, 1))
            .unwrap();
        assert_eq!(u32at(&opened, 5), Some(0));
        assert_eq!(u32at(&opened, 76), Some(0x10));
        std::fs::remove_dir_all(root).unwrap();
    }
}
