//! Read-only SMB 2.0.2/2.1 installation shares (MS-SMB2, MS-FSCC, MS-NLMP).
mod auth;

use crate::{media_source, model::Shared};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Read, Seek, SeekFrom},
    net::Ipv4Addr,
    path::PathBuf,
    sync::{Arc, RwLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    task::JoinSet,
};

const DENIED: u32 = 0xc0000022;
const INVALID: u32 = 0xc000000d;
const NOT_FOUND: u32 = 0xc0000034;
const BAD_HANDLE: u32 = 0xc0000008;
const UNSUPPORTED: u32 = 0xc00000bb;
const BUFFER_SMALL: u32 = 0xc0000023;
const MAX_IO: usize = 65536;
const MAX_FRAME: usize = 1024 * 1024;
const MAX_HANDLES: usize = 256;
const SIGNED: u32 = 8;

type Result<T> = std::result::Result<T, u32>;
#[derive(Clone)]
pub struct Credential {
    pub username: String,
    pub password: String,
    pub profile_id: String,
    pub device_id: String,
    pub expires_at: u64,
}
pub type Credentials = Arc<RwLock<Vec<Credential>>>;
pub(super) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn stamp() -> u64 {
    (now() + 11_644_473_600) * 10_000_000
}
pub(super) fn get16(b: &[u8], at: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(
        b.get(at..at + 2).ok_or(INVALID)?.try_into().unwrap(),
    ))
}
pub(super) fn get32(b: &[u8], at: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(
        b.get(at..at + 4).ok_or(INVALID)?.try_into().unwrap(),
    ))
}
pub(super) fn get64(b: &[u8], at: usize) -> Result<u64> {
    Ok(u64::from_le_bytes(
        b.get(at..at + 8).ok_or(INVALID)?.try_into().unwrap(),
    ))
}
fn set16(b: &mut [u8], at: usize, n: u16) {
    b[at..at + 2].copy_from_slice(&n.to_le_bytes());
}
fn set32(b: &mut [u8], at: usize, n: u32) {
    b[at..at + 4].copy_from_slice(&n.to_le_bytes());
}
fn set64(b: &mut [u8], at: usize, n: u64) {
    b[at..at + 8].copy_from_slice(&n.to_le_bytes());
}
pub(super) fn wide(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(u16::to_le_bytes).collect()
}
pub(super) fn utf16(b: &[u8]) -> Result<String> {
    if !b.len().is_multiple_of(2) {
        return Err(INVALID);
    }
    String::from_utf16(
        &b.chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect::<Vec<_>>(),
    )
    .map_err(|_| INVALID)
}
fn range(b: &[u8], offset: usize, length: usize, minimum: usize) -> Result<&[u8]> {
    if offset < minimum || length > MAX_FRAME {
        return Err(INVALID);
    }
    b.get(offset..offset.checked_add(length).ok_or(INVALID)?)
        .ok_or(INVALID)
}
fn path(s: &str) -> Result<String> {
    if s.is_empty() {
        return Ok(String::new());
    }
    let s = s.strip_prefix('\\').unwrap_or(s);
    if s.contains([':', '\0', '/']) || s.chars().any(char::is_control) {
        return Err(DENIED);
    }
    let components = s.split('\\').collect::<Vec<_>>();
    if components.iter().any(|p| {
        p.is_empty()
            || *p == "."
            || *p == ".."
            || p.ends_with(['.', ' '])
            || p.contains(['*', '?', '<', '>', '"', '|'])
    }) {
        return Err(DENIED);
    }
    Ok(components.join("/"))
}
fn fold(s: &str) -> String {
    s.to_uppercase()
}
fn wildcard(pattern: &str, name: &str) -> bool {
    if matches!(pattern, "*" | "*.*" | "<\"*") {
        return true;
    }
    let p = fold(pattern).chars().collect::<Vec<_>>();
    let n = fold(name).chars().collect::<Vec<_>>();
    let mut prev = vec![false; n.len() + 1];
    prev[0] = true;
    for c in p {
        let mut next = vec![false; n.len() + 1];
        match c {
            '*' | '<' => {
                next[0] = prev[0];
                for i in 1..next.len() {
                    next[i] = prev[i] || next[i - 1];
                }
            }
            '?' | '>' => {
                next[1..].copy_from_slice(&prev[..n.len()]);
                if c == '>' {
                    next[n.len()] |= prev[n.len()];
                }
            }
            '"' => {
                for i in 1..next.len() {
                    next[i] = prev[i - 1] && n[i - 1] == '.';
                }
                next[n.len()] |= prev[n.len()];
            }
            _ => {
                for i in 1..next.len() {
                    next[i] = prev[i - 1] && n[i - 1] == c;
                }
            }
        }
        prev = next;
    }
    prev[n.len()]
}
#[derive(Clone)]
struct Node {
    name: String,
    size: u64,
    directory: bool,
    id: u64,
}
impl Node {
    fn attrs(&self) -> u32 {
        if self.directory { 0x11 } else { 0x21 }
    }
    fn allocation(&self) -> u64 {
        self.size.saturating_add(4095) & !4095
    }
}
fn source_entries(root: &std::path::Path) -> Result<BTreeMap<String, (u64, bool)>> {
    if root.join(".bootah-media.json").symlink_metadata().is_ok() {
        return Ok(media_source::list(root)
            .map_err(|_| NOT_FOUND)?
            .into_iter()
            .filter(|(_, e)| e.link.is_none())
            .map(|(n, e)| (n, (e.size, e.is_dir)))
            .collect());
    }
    fn walk(
        root: &std::path::Path,
        directory: &std::path::Path,
        depth: usize,
        out: &mut BTreeMap<String, (u64, bool)>,
    ) -> Result<()> {
        if depth > 32 || out.len() > 100_000 {
            return Err(INVALID);
        }
        for entry in std::fs::read_dir(directory).map_err(|_| NOT_FOUND)? {
            let entry = entry.map_err(|_| NOT_FOUND)?;
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let kind = entry.file_type().map_err(|_| NOT_FOUND)?;
            if kind.is_symlink() || !(kind.is_dir() || kind.is_file()) {
                continue;
            }
            let full = entry.path();
            let relative = full
                .strip_prefix(root)
                .map_err(|_| DENIED)?
                .to_str()
                .ok_or(INVALID)?
                .replace(std::path::MAIN_SEPARATOR, "/");
            if path(&relative.replace('/', "\\")).is_err() {
                continue;
            }
            if !full
                .canonicalize()
                .map_err(|_| NOT_FOUND)?
                .starts_with(root.canonicalize().map_err(|_| NOT_FOUND)?)
            {
                return Err(DENIED);
            }
            let metadata = entry.metadata().map_err(|_| NOT_FOUND)?;
            out.insert(
                relative,
                (
                    if kind.is_file() { metadata.len() } else { 0 },
                    kind.is_dir(),
                ),
            );
            if kind.is_dir() {
                walk(root, &full, depth + 1, out)?;
            }
            if out.len() > 100_000 {
                return Err(INVALID);
            }
        }
        Ok(())
    }
    let mut out = BTreeMap::new();
    walk(root, root, 0, &mut out)?;
    Ok(out)
}
struct Tree {
    root: PathBuf,
    profile: String,
    nodes: BTreeMap<String, Arc<Node>>,
}
impl Tree {
    fn load(state: &Shared, profile_id: &str) -> Result<Self> {
        let profile = state
            .lock()
            .map_err(|_| DENIED)?
            .config
            .profiles
            .iter()
            .find(|p| p.kind == "windows" && p.id == profile_id)
            .cloned()
            .ok_or(DENIED)?;
        let mut nodes = BTreeMap::new();
        nodes.insert(
            String::new(),
            Node {
                name: String::new(),
                size: 0,
                directory: true,
                id: 1,
            },
        );
        nodes.insert(
            fold(profile_id),
            Node {
                name: profile_id.into(),
                size: 0,
                directory: true,
                id: 2,
            },
        );
        for (name, (size, directory)) in source_entries(&profile.root)? {
            if name.split('/').any(|s| s.starts_with('.')) {
                continue;
            }
            if path(&name.replace('/', "\\")).is_err() {
                continue;
            }
            let full = format!("{profile_id}/{name}");
            let mut parents = full.split('/').collect::<Vec<_>>();
            parents.pop();
            while parents.len() > 1 {
                let parent = parents.join("/");
                let id = nodes.len() as u64 + 1;
                nodes.entry(fold(&parent)).or_insert(Node {
                    name: parent,
                    size: 0,
                    directory: true,
                    id,
                });
                parents.pop();
            }
            let key = fold(&full);
            if let Some(existing) = nodes.get(&key) {
                if existing.name != full || !existing.directory || !directory {
                    return Err(INVALID);
                }
            } else {
                let id = nodes.len() as u64 + 1;
                nodes.insert(
                    key,
                    Node {
                        name: full,
                        size,
                        directory,
                        id,
                    },
                );
            }
        }
        Ok(Self {
            root: profile.root,
            profile: profile_id.into(),
            nodes: nodes.into_iter().map(|(k, n)| (k, Arc::new(n))).collect(),
        })
    }
    fn node(&self, name: &str) -> Result<Arc<Node>> {
        self.nodes.get(&fold(name)).cloned().ok_or(NOT_FOUND)
    }
    fn open(&self, node: &Node) -> Result<Option<media_source::Resource>> {
        if node.directory {
            return Ok(None);
        }
        let relative = node
            .name
            .strip_prefix(&format!("{}/", self.profile))
            .ok_or(DENIED)?;
        media_source::open(&self.root, relative)
            .map(Some)
            .map_err(|_| NOT_FOUND)
    }
}
struct Handle {
    node: Arc<Node>,
    resource: Option<media_source::Resource>,
    listing: Option<Vec<Arc<Node>>>,
    cursor: usize,
    pattern: String,
    tree: u32,
}
struct Session {
    state: Shared,
    credentials: Credentials,
    dialect: u16,
    guid: [u8; 16],
    challenge: Option<auth::Challenge>,
    identity: Option<Credential>,
    key: Option<[u8; 16]>,
    sid: u64,
    trees: BTreeMap<u32, Arc<Tree>>,
    handles: BTreeMap<u64, Handle>,
    next_tree: u32,
    next_handle: u64,
    cookie: u64,
    credits: BTreeSet<u64>,
    next_credit: u64,
    last_file: Option<u64>,
    start_time: u64,
    closed: bool,
    client_negotiate: Vec<u8>,
    peer: String,
}
impl Session {
    fn new(state: Shared, credentials: Credentials, guid: [u8; 16]) -> Self {
        let random = *uuid::Uuid::new_v4().as_bytes();
        Self {
            state,
            credentials,
            dialect: 0,
            guid,
            challenge: None,
            identity: None,
            key: None,
            sid: u64::from_le_bytes(random[..8].try_into().unwrap()).max(1),
            trees: BTreeMap::new(),
            handles: BTreeMap::new(),
            next_tree: 1,
            next_handle: 1,
            cookie: u64::from_le_bytes(random[8..].try_into().unwrap()),
            credits: BTreeSet::from([0]),
            next_credit: 1,
            last_file: None,
            start_time: stamp(),
            closed: false,
            client_negotiate: Vec::new(),
            peer: String::new(),
        }
    }
    fn audit(&self, message: &str) {
        if let Ok(mut state) = self.state.lock() {
            let device = self
                .identity
                .as_ref()
                .map(|c| c.device_id.as_str())
                .unwrap_or("");
            let profile = self
                .identity
                .as_ref()
                .map(|c| c.profile_id.as_str())
                .unwrap_or("");
            state.event(
                device,
                "SMB2",
                format!("peer={} profile={profile} {message}", self.peer),
            );
        }
    }
    fn authorized(&self) -> bool {
        self.identity.as_ref().is_some_and(|identity| {
            self.state.lock().is_ok_and(|s| {
                s.config
                    .profiles
                    .iter()
                    .any(|p| p.id == identity.profile_id && p.kind == "windows")
                    && s.config.devices.iter().any(|d| d.id == identity.device_id)
            }) && self.credentials.read().is_ok_and(|list| {
                list.iter().any(|c| {
                    c.username == identity.username
                        && c.password == identity.password
                        && c.profile_id == identity.profile_id
                })
            })
        })
    }
    fn file_id(&self, req: &[u8], at: usize, related: bool) -> Result<u64> {
        let id = get64(req, at)?;
        let cookie = get64(req, at + 8)?;
        if related && id == u64::MAX && cookie == u64::MAX {
            return self
                .last_file
                .filter(|id| self.handles.contains_key(id))
                .ok_or(BAD_HANDLE);
        }
        if cookie != self.cookie || !self.handles.contains_key(&id) {
            return Err(BAD_HANDLE);
        }
        Ok(id)
    }
    fn header(req: &[u8], status: u32, credit: u16, sid: u64, tid: u32) -> Vec<u8> {
        let mut out = vec![0; 64];
        out[..4].copy_from_slice(b"\xfeSMB");
        set16(&mut out, 4, 64);
        set16(&mut out, 6, get16(req, 6).unwrap_or(0));
        set32(&mut out, 8, status);
        set16(&mut out, 12, get16(req, 12).unwrap_or(0));
        set16(&mut out, 14, credit);
        set32(&mut out, 16, 1);
        out[24..36].copy_from_slice(&req[24..36]);
        set32(&mut out, 36, tid);
        set64(&mut out, 40, sid);
        out
    }
    fn multi_protocol_negotiate(&mut self, frame: &[u8]) -> Result<Vec<u8>> {
        if self.dialect != 0
            || self.next_credit != 1
            || frame.len() < 35
            || frame.len() > 4096
            || frame[4] != 0x72
            || frame[9] & 0x80 != 0
            || frame[32] != 0
            || get16(frame, 33)? as usize != frame.len() - 35
        {
            return Err(INVALID);
        }
        let mut dialects = &frame[35..];
        let mut smb202 = false;
        let mut wildcard = false;
        while !dialects.is_empty() {
            if dialects[0] != 2 {
                return Err(INVALID);
            }
            let end = dialects[1..].iter().position(|b| *b == 0).ok_or(INVALID)? + 1;
            smb202 |= &dialects[1..end] == b"SMB 2.002";
            wildcard |= &dialects[1..end] == b"SMB 2.???";
            dialects = &dialects[end + 1..];
        }
        if !smb202 && !wildcard {
            return Err(UNSUPPORTED);
        }
        let mut request = vec![0; 102];
        request[..4].copy_from_slice(b"\xfeSMB");
        set16(&mut request, 4, 64);
        set16(&mut request, 14, 1);
        set16(&mut request, 64, 36);
        set16(&mut request, 66, 1);
        set16(&mut request, 68, 1);
        set16(&mut request, 100, 0x202);
        let mut response = self.process(&request)?;
        if wildcard {
            self.dialect = 0;
            self.client_negotiate.clear();
            set16(&mut response, 68, 0x2ff);
            set32(&mut response, 88, 4);
        }
        Ok(response)
    }
    fn process(&mut self, frame: &[u8]) -> Result<Vec<u8>> {
        if frame.starts_with(b"\xffSMB") {
            return self.multi_protocol_negotiate(frame);
        }
        let mut offset = 0;
        let mut replies: Vec<Vec<u8>> = Vec::new();
        let mut related_tree = 0;
        let mut compound_failed = false;
        self.last_file = None;
        loop {
            let remaining = frame.get(offset..).ok_or(INVALID)?;
            if self.closed
                || remaining.len() < 64
                || &remaining[..4] != b"\xfeSMB"
                || get16(remaining, 4)? != 64
            {
                return Err(INVALID);
            }
            let next = get32(remaining, 20)? as usize;
            if next != 0
                && (next < 64
                    || !next.is_multiple_of(8)
                    || next > remaining.len().saturating_sub(64))
            {
                return Err(INVALID);
            }
            let req = if next == 0 {
                remaining
            } else {
                &remaining[..next]
            };
            let flags = get32(req, 16)?;
            if flags & !0x1000000c != 0 {
                return Err(INVALID);
            }
            let related = flags & 4 != 0;
            if related && replies.is_empty() {
                return Err(INVALID);
            }
            let cmd = get16(req, 12)?;
            let charge = if self.dialect == 0x210 {
                get16(req, 6)?.max(1) as u64
            } else {
                1
            };
            let mid = get64(req, 24)?;
            let end = mid.checked_add(charge).ok_or(INVALID)?;
            if charge > 32 || !(mid..end).all(|id| self.credits.contains(&id)) {
                return Err(INVALID);
            }
            for id in mid..end {
                self.credits.remove(&id);
            }
            let requested = get16(req, 14)?.max(1) as usize;
            let granted = requested.min(32usize.saturating_sub(self.credits.len()));
            for _ in 0..granted {
                self.credits.insert(self.next_credit);
                self.next_credit = self.next_credit.checked_add(1).ok_or(INVALID)?;
            }
            let mut tid = if related {
                related_tree
            } else {
                get32(req, 36)?
            };
            let sid = if related { self.sid } else { get64(req, 40)? };
            let mut result = if self.dialect == 0 && cmd != 0 {
                Err(INVALID)
            } else if (self.key.is_none() && cmd > 1)
                || (self.key.is_some()
                    && (!self.authorized()
                        || sid != self.sid
                        || flags & SIGNED == 0
                        || !auth::equal(
                            &req[48..64],
                            &auth::signature(self.key.as_ref().unwrap(), req),
                        )))
            {
                Err(DENIED)
            } else if related && compound_failed {
                Err(0xc0000001)
            } else {
                self.command(req, cmd, &mut tid, related)
            };
            let mut status = 0;
            if cmd == 1 && self.challenge.is_some() && self.key.is_none() && result.is_ok() {
                status = 0xc0000016;
            }
            if let Err(error) = result {
                if cmd == 1 {
                    self.closed = true;
                }
                if cmd == 1 || error == DENIED {
                    self.audit(&format!("command={cmd} status={error:08x}"));
                }
                status = error;
                compound_failed = true;
                result = Ok(vec![9, 0, 0, 0, 0, 0, 0, 0, 0]);
            }
            let response_sid = if cmd == 0 { 0 } else { self.sid };
            let mut response = Self::header(req, status, granted as u16, response_sid, tid);
            response.extend(result.unwrap());
            related_tree = tid;
            replies.push(response);
            if replies.len() > 64 {
                return Err(INVALID);
            }
            if next == 0 {
                break;
            }
            offset += next;
        }
        let last = replies.len() - 1;
        let mut out = Vec::new();
        for (i, mut response) in replies.into_iter().enumerate() {
            if i != last {
                let length = (response.len() + 7) & !7;
                response.resize(length, 0);
                set32(&mut response, 20, length as u32);
            }
            if let Some(key) = self.key.as_ref() {
                set32(&mut response, 16, 1 | SIGNED);
                let sig = auth::signature(key, &response);
                response[48..64].copy_from_slice(&sig);
            }
            out.extend(response);
        }
        Ok(out)
    }
    fn command(&mut self, req: &[u8], cmd: u16, tid: &mut u32, related: bool) -> Result<Vec<u8>> {
        let size = get16(req, 64)?;
        match cmd {
            0 => {
                if self.dialect != 0 || size != 36 {
                    return Err(INVALID);
                }
                let count = get16(req, 66)? as usize;
                if count == 0 || count > 64 {
                    return Err(INVALID);
                }
                let dialects = range(req, 100, count * 2, 100)?;
                self.client_negotiate = req[66..100 + count * 2].to_vec();
                self.dialect = if dialects.chunks_exact(2).any(|d| d == [0x10, 2]) {
                    0x210
                } else if dialects.chunks_exact(2).any(|d| d == [2, 2]) {
                    0x202
                } else {
                    return Err(UNSUPPORTED);
                };
                let token = auth::initial_token();
                let mut out = vec![0; 64];
                set16(&mut out, 0, 65);
                set16(&mut out, 2, 3);
                set16(&mut out, 4, self.dialect);
                out[8..24].copy_from_slice(&self.guid);
                if self.dialect == 0x210 {
                    set32(&mut out, 24, 4);
                }
                for at in [28, 32, 36] {
                    set32(&mut out, at, MAX_IO as u32);
                }
                set64(&mut out, 40, stamp());
                set64(&mut out, 48, self.start_time);
                set16(&mut out, 56, 128);
                set16(&mut out, 58, token.len() as u16);
                out.extend(token);
                Ok(out)
            }
            1 => {
                if size != 25 || self.key.is_some() {
                    return Err(INVALID);
                }
                let security = range(req, get16(req, 76)? as usize, get16(req, 78)? as usize, 88)?;
                let token = if let Some(challenge) = self.challenge.take() {
                    let list = self.credentials.read().map_err(|_| DENIED)?;
                    let (identity, key) = challenge
                        .authenticate(security, &list)
                        .ok_or(0xc000006du32)?;
                    drop(list);
                    self.identity = Some(identity);
                    self.key = Some(key);
                    self.audit("authenticated; signing required");
                    auth::completed_token()
                } else {
                    let (challenge, token) = auth::Challenge::new(security).ok_or(0xc000006du32)?;
                    self.challenge = Some(challenge);
                    token
                };
                let mut out = vec![0; 8];
                set16(&mut out, 0, 9);
                set16(&mut out, 4, 72);
                set16(&mut out, 6, token.len() as u16);
                out.extend(token);
                Ok(out)
            }
            2 => {
                if size != 4 {
                    return Err(INVALID);
                }
                self.trees.clear();
                self.handles.clear();
                self.closed = true;
                Ok(vec![4, 0, 0, 0])
            }
            3 => {
                if size != 9 || self.trees.len() >= 8 {
                    return Err(INVALID);
                }
                let share = utf16(range(
                    req,
                    get16(req, 68)? as usize,
                    get16(req, 70)? as usize,
                    72,
                )?)?;
                let pieces = share.split('\\').collect::<Vec<_>>();
                if pieces.len() != 4
                    || !pieces[0].is_empty()
                    || !pieces[1].is_empty()
                    || pieces[2].is_empty()
                    || !pieces[3].eq_ignore_ascii_case("Bootah")
                {
                    return Err(0xc00000cc);
                }
                let tree = if let Some(tree) = self.trees.values().next() {
                    tree.clone()
                } else {
                    Arc::new(Tree::load(
                        &self.state,
                        &self.identity.as_ref().ok_or(DENIED)?.profile_id,
                    )?)
                };
                *tid = self.next_tree;
                self.next_tree += 1;
                self.trees.insert(*tid, tree);
                let mut out = vec![0; 16];
                set16(&mut out, 0, 16);
                out[2] = 1;
                set32(&mut out, 12, 0x001200a9);
                Ok(out)
            }
            4 => {
                if size != 4 || self.trees.remove(tid).is_none() {
                    return Err(INVALID);
                }
                self.handles.retain(|_, h| h.tree != *tid);
                Ok(vec![4, 0, 0, 0])
            }
            5 => self.create(req, *tid),
            6 => {
                if size != 24 {
                    return Err(INVALID);
                }
                let id = self.file_id(req, 72, related)?;
                if self.handles.get(&id).ok_or(BAD_HANDLE)?.tree != *tid {
                    return Err(BAD_HANDLE);
                }
                let handle = self.handles.remove(&id).ok_or(BAD_HANDLE)?;
                let mut out = vec![0; 60];
                set16(&mut out, 0, 60);
                if get16(req, 66)? & 1 != 0 {
                    set16(&mut out, 2, 1);
                    self.meta(&mut out, 8, &handle.node);
                }
                Ok(out)
            }
            7 => {
                if size != 24 {
                    return Err(INVALID);
                }
                let id = self.file_id(req, 72, related)?;
                if self.handles.get(&id).ok_or(BAD_HANDLE)?.tree != *tid {
                    return Err(BAD_HANDLE);
                }
                Ok(vec![4, 0, 0, 0])
            }
            8 => {
                if size != 49 {
                    return Err(INVALID);
                }
                let length = get32(req, 68)? as usize;
                if length > MAX_IO {
                    return Err(INVALID);
                }
                let offset = get64(req, 72)?;
                let id = self.file_id(req, 80, related)?;
                let handle = self.handles.get_mut(&id).ok_or(BAD_HANDLE)?;
                if handle.tree != *tid {
                    return Err(BAD_HANDLE);
                }
                if handle.node.directory {
                    return Err(0xc00000ba);
                }
                if offset >= handle.node.size && length > 0 {
                    return Err(0xc0000011);
                }
                let amount = (handle.node.size.saturating_sub(offset)).min(length as u64) as usize;
                let resource = handle.resource.as_mut().ok_or(BAD_HANDLE)?;
                resource
                    .seek(SeekFrom::Start(offset))
                    .map_err(|_| 0xc0000185u32)?;
                let mut data = vec![0; amount];
                resource.read_exact(&mut data).map_err(|_| 0xc0000185u32)?;
                let mut out = vec![0; 16];
                set16(&mut out, 0, 17);
                out[2] = 80;
                set32(&mut out, 4, amount as u32);
                out.extend(data);
                Ok(out)
            }
            9 | 17 => Err(DENIED),
            10 => Err(UNSUPPORTED),
            11 => self.ioctl(req, *tid),
            13 => {
                if size != 4 {
                    return Err(INVALID);
                }
                Ok(vec![4, 0, 0, 0])
            }
            14 => self.directory(req, *tid, related),
            15 => Err(UNSUPPORTED),
            16 => self.query(req, *tid, related),
            _ => Err(UNSUPPORTED),
        }
    }
    fn meta(&self, out: &mut [u8], at: usize, node: &Node) {
        for offset in [0, 8, 16, 24] {
            set64(out, at + offset, self.start_time);
        }
        set64(out, at + 32, node.allocation());
        set64(out, at + 40, node.size);
        set32(out, at + 48, node.attrs());
    }
    fn create(&mut self, req: &[u8], tid: u32) -> Result<Vec<u8>> {
        if get16(req, 64)? != 57 || self.handles.len() >= MAX_HANDLES {
            return Err(INVALID);
        }
        let access = get32(req, 88)?;
        let disposition = get32(req, 100)?;
        let options = get32(req, 104)?;
        if access & 0x500d0116 != 0 || disposition != 1 || options & 0x1000 != 0 {
            return Err(DENIED);
        }
        let len = get16(req, 110)? as usize;
        let name = if len == 0 {
            String::new()
        } else {
            path(&utf16(range(req, get16(req, 108)? as usize, len, 120)?)?)?
        };
        let tree = self.trees.get(&tid).ok_or(0xc00000c9u32)?;
        let node = tree.node(&name)?;
        if options & 1 != 0 && !node.directory {
            return Err(0xc0000103);
        }
        if options & 0x40 != 0 && node.directory {
            return Err(0xc00000ba);
        }
        let resource = tree.open(&node)?;
        let id = self.next_handle;
        self.next_handle = self.next_handle.checked_add(1).ok_or(INVALID)?;
        let mut out = vec![0; 88];
        set16(&mut out, 0, 89);
        set32(&mut out, 4, 1);
        self.meta(&mut out, 8, &node);
        set64(&mut out, 64, id);
        set64(&mut out, 72, self.cookie);
        self.handles.insert(
            id,
            Handle {
                node,
                resource,
                listing: None,
                cursor: 0,
                pattern: String::new(),
                tree: tid,
            },
        );
        self.last_file = Some(id);
        Ok(out)
    }
    fn ioctl(&self, req: &[u8], tid: u32) -> Result<Vec<u8>> {
        if get16(req, 64)? != 57 || !self.trees.contains_key(&tid) {
            return Err(INVALID);
        }
        let code = get32(req, 68)?;
        if code != 0x00140204 {
            return Err(if code == 0x00060194 {
                0xc000019c
            } else {
                UNSUPPORTED
            });
        }
        let input = range(req, get32(req, 88)? as usize, get32(req, 92)? as usize, 120)?;
        if input.len() < 24 {
            return Err(INVALID);
        }
        let count = get16(input, 22)? as usize;
        if input.len() != 24 + count * 2
            || get32(input, 0)? != get32(&self.client_negotiate, 6)?
            || input[4..20] != self.client_negotiate[10..26]
            || get16(input, 20)? != get16(&self.client_negotiate, 2)?
            || count != get16(&self.client_negotiate, 0)? as usize
            || input[24..] != self.client_negotiate[34..]
        {
            return Err(DENIED);
        }
        if get32(req, 108)? < 24 {
            return Err(BUFFER_SMALL);
        }
        let mut out = vec![0; 48];
        set16(&mut out, 0, 49);
        set32(&mut out, 4, code);
        out[8..24].copy_from_slice(&req[72..88]);
        set32(&mut out, 32, 112);
        set32(&mut out, 36, 24);
        let mut data = vec![0; 24];
        if self.dialect == 0x210 {
            set32(&mut data, 0, 4);
        }
        data[4..20].copy_from_slice(&self.guid);
        set16(&mut data, 20, 3);
        set16(&mut data, 22, self.dialect);
        out.extend(data);
        Ok(out)
    }
    fn directory(&mut self, req: &[u8], tid: u32, related: bool) -> Result<Vec<u8>> {
        if get16(req, 64)? != 33 {
            return Err(INVALID);
        }
        let class = *req.get(66).ok_or(INVALID)?;
        let flags = *req.get(67).ok_or(INVALID)?;
        if !matches!(class, 1 | 2 | 3 | 12 | 37 | 38) {
            return Err(UNSUPPORTED);
        }
        let capacity = (get32(req, 92)? as usize).min(MAX_IO);
        let id = self.file_id(req, 72, related)?;
        let listing_budget = 100_000usize.saturating_sub(
            self.handles
                .iter()
                .filter(|(key, _)| **key != id)
                .map(|(_, h)| h.listing.as_ref().map_or(0, Vec::len))
                .sum::<usize>(),
        );
        let handle = self.handles.get_mut(&id).ok_or(BAD_HANDLE)?;
        if handle.tree != tid {
            return Err(BAD_HANDLE);
        }
        if !handle.node.directory {
            return Err(0xc0000103);
        }
        let name_len = get16(req, 90)? as usize;
        let pattern = if name_len == 0 {
            handle.pattern.clone()
        } else {
            utf16(range(req, get16(req, 88)? as usize, name_len, 96)?)?
        };
        if pattern.len() > 1024 || pattern.contains(['\\', '/', ':', '\0']) {
            return Err(INVALID);
        }
        if handle.listing.is_none() || flags & 0x10 != 0 {
            handle.pattern = if pattern.is_empty() {
                "*".into()
            } else {
                pattern
            };
            let tree = self.trees.get(&tid).ok_or(BAD_HANDLE)?;
            handle.listing = Some(
                tree.nodes
                    .values()
                    .filter(|n| {
                        !n.name.is_empty()
                            && n.name.rsplit_once('/').map(|x| x.0).unwrap_or("")
                                == handle.node.name
                            && wildcard(&handle.pattern, n.name.rsplit('/').next().unwrap_or(""))
                    })
                    .take(listing_budget + 1)
                    .cloned()
                    .collect(),
            );
            if handle.listing.as_ref().unwrap().len() > listing_budget {
                handle.listing = None;
                return Err(0xc000009a);
            }
            handle.cursor = 0;
        }
        if flags & 1 != 0 {
            handle.cursor = 0;
        }
        if flags & 4 != 0 {
            handle.cursor = get32(req, 68)? as usize;
        }
        let listing = handle.listing.as_ref().unwrap();
        if handle.cursor >= listing.len() {
            return Err(0x80000006);
        }
        let mut data = Vec::new();
        let mut previous: Option<usize> = None;
        while let Some(node) = listing.get(handle.cursor) {
            let basename = node.name.rsplit('/').next().unwrap_or("");
            let name = wide(basename);
            let header = match class {
                1 => 64,
                2 => 68,
                3 => 94,
                12 => 12,
                37 => 104,
                38 => 80,
                _ => unreachable!(),
            };
            let mut entry = vec![0; header];
            set32(&mut entry, 4, handle.cursor as u32);
            if class == 12 {
                set32(&mut entry, 8, name.len() as u32);
            } else {
                for at in [8, 16, 24, 32] {
                    set64(&mut entry, at, self.start_time);
                }
                set64(&mut entry, 40, node.size);
                set64(&mut entry, 48, node.allocation());
                set32(&mut entry, 56, node.attrs());
                set32(&mut entry, 60, name.len() as u32);
                if class == 37 {
                    set64(&mut entry, 96, node.id);
                }
                if class == 38 {
                    set64(&mut entry, 72, node.id);
                }
            }
            entry.extend(name);
            let start = (data.len() + 7) & !7;
            if start + entry.len() > capacity {
                if data.is_empty() {
                    return Err(BUFFER_SMALL);
                }
                break;
            }
            if let Some(prev) = previous {
                set32(&mut data, prev, (start - prev) as u32);
            }
            data.resize(start, 0);
            previous = Some(start);
            data.extend(entry);
            handle.cursor += 1;
            if flags & 2 != 0 {
                break;
            }
        }
        let mut out = vec![0; 8];
        set16(&mut out, 0, 9);
        set16(&mut out, 2, 72);
        set32(&mut out, 4, data.len() as u32);
        out.extend(data);
        Ok(out)
    }
    fn query(&self, req: &[u8], tid: u32, related: bool) -> Result<Vec<u8>> {
        if get16(req, 64)? != 41 {
            return Err(INVALID);
        }
        let kind = *req.get(66).ok_or(INVALID)?;
        let class = *req.get(67).ok_or(INVALID)?;
        let capacity = (get32(req, 68)? as usize).min(MAX_IO);
        let id = self.file_id(req, 88, related)?;
        let handle = self.handles.get(&id).ok_or(BAD_HANDLE)?;
        if handle.tree != tid {
            return Err(BAD_HANDLE);
        }
        let n = &handle.node;
        let basic = || {
            let mut b = vec![0; 40];
            for at in [0, 8, 16, 24] {
                set64(&mut b, at, self.start_time);
            }
            set32(&mut b, 32, n.attrs());
            b
        };
        let standard = || {
            let mut b = vec![0; 24];
            set64(&mut b, 0, n.allocation());
            set64(&mut b, 8, n.size);
            set32(&mut b, 16, 1);
            b[21] = u8::from(n.directory);
            b
        };
        let name = || {
            let text = wide(&format!("\\{}", n.name.replace('/', "\\")));
            let mut b = (text.len() as u32).to_le_bytes().to_vec();
            b.extend(text);
            b
        };
        let data = match (kind, class) {
            (1, 4) => basic(),
            (1, 5) => standard(),
            (1, 6) => n.id.to_le_bytes().to_vec(),
            (1, 7) => vec![0; 4],
            (1, 8) => 0x001200a9u32.to_le_bytes().to_vec(),
            (1, 9 | 48) => name(),
            (1, 14) => vec![0; 8],
            (1, 16 | 17) => vec![0; 4],
            (1, 18) => {
                let mut b = basic();
                b.extend(standard());
                b.extend(n.id.to_le_bytes());
                b.extend([0; 4]);
                b.extend(0x001200a9u32.to_le_bytes());
                b.extend([0; 16]);
                b.extend(name());
                b
            }
            (1, 22) => {
                let mut b = vec![0; 24];
                set32(&mut b, 4, 14);
                set64(&mut b, 8, n.size);
                set64(&mut b, 16, n.allocation());
                b.extend(wide("::$DATA"));
                b
            }
            (1, 34) => {
                let mut b = vec![0; 56];
                self.meta(&mut b, 0, n);
                b
            }
            (2, 1) => {
                let label = wide("Bootah");
                let mut b = vec![0; 18];
                set64(&mut b, 0, self.start_time);
                set32(&mut b, 8, 0x424f4f54);
                set32(&mut b, 12, label.len() as u32);
                b.extend(label);
                b
            }
            (2, 3) => {
                let mut b = vec![0; 24];
                set64(&mut b, 0, 1 << 30);
                set32(&mut b, 16, 8);
                set32(&mut b, 20, 512);
                b
            }
            (2, 4) => {
                let mut b = vec![0; 8];
                set32(&mut b, 0, 7);
                set32(&mut b, 4, 0x10);
                b
            }
            (2, 5) => {
                let mut b = vec![0; 12];
                set32(&mut b, 0, 0x00080006);
                set32(&mut b, 4, 255);
                set32(&mut b, 8, 8);
                b.extend(wide("NTFS"));
                b
            }
            (2, 7) => {
                let mut b = vec![0; 32];
                set64(&mut b, 0, 1 << 30);
                set32(&mut b, 24, 8);
                set32(&mut b, 28, 512);
                b
            }
            (3, _) => security_descriptor(get32(req, 80)?)?,
            _ => return Err(UNSUPPORTED),
        };
        if data.len() > capacity {
            return Err(BUFFER_SMALL);
        }
        let mut out = vec![0; 8];
        set16(&mut out, 0, 9);
        set16(&mut out, 2, 72);
        set32(&mut out, 4, data.len() as u32);
        out.extend(data);
        Ok(out)
    }
}
fn security_descriptor(flags: u32) -> Result<Vec<u8>> {
    if flags & !7 != 0 {
        return Err(DENIED);
    }
    let sid = [1, 1, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0]; // Everyone; share authentication still gates access.
    let mut out = vec![0; 20];
    out[0] = 1;
    set16(&mut out, 2, 0x8000 | if flags & 4 != 0 { 4 } else { 0 });
    if flags & 1 != 0 {
        let at = out.len();
        set32(&mut out, 4, at as u32);
        out.extend(sid);
    }
    if flags & 2 != 0 {
        let at = out.len();
        set32(&mut out, 8, at as u32);
        out.extend(sid);
    }
    if flags & 4 != 0 {
        let at = out.len();
        set32(&mut out, 16, at as u32);
        out.extend([2, 0, 28, 0, 1, 0, 0, 0, 0, 0, 20, 0]);
        out.extend(0x001200a9u32.to_le_bytes());
        out.extend(sid);
    }
    Ok(out)
}
async fn serve(mut stream: TcpStream, mut session: Session) -> std::result::Result<(), String> {
    stream.set_nodelay(true).map_err(|e| e.to_string())?;
    let authentication_deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let peer = stream
        .peer_addr()
        .map(|p| p.to_string())
        .unwrap_or_default();
    session.peer = peer.clone();
    loop {
        let mut frame = [0; 4];
        let deadline = if session.key.is_none() {
            authentication_deadline
        } else {
            tokio::time::Instant::now() + Duration::from_secs(1800)
        };
        tokio::time::timeout_at(deadline, stream.read_exact(&mut frame))
            .await
            .map_err(|_| "SMB2 idle timeout")?
            .map_err(|e| e.to_string())?;
        let length = u32::from_be_bytes(frame) as usize;
        if frame[0] != 0 || !(35..=MAX_FRAME).contains(&length) {
            return Err("Invalid SMB2 frame".into());
        }
        let mut data = vec![0; length];
        tokio::time::timeout(Duration::from_secs(30), stream.read_exact(&mut data))
            .await
            .map_err(|_| "SMB2 frame timeout")?
            .map_err(|e| e.to_string())?;
        if session.key.is_none() && tokio::time::Instant::now() > authentication_deadline {
            return Err("SMB2 authentication deadline".into());
        }
        let (next, result) = tokio::task::spawn_blocking(move || {
            let result = session.process(&data);
            (session, result)
        })
        .await
        .map_err(|e| e.to_string())?;
        session = next;
        let response = result.map_err(|e| {
            session.audit(&format!("peer={peer} protocol error {e:08x}"));
            format!("SMB2 protocol error {e:08x}")
        })?;
        let mut output = (response.len() as u32).to_be_bytes().to_vec();
        output.extend(response);
        tokio::time::timeout(Duration::from_secs(30), stream.write_all(&output))
            .await
            .map_err(|_| "SMB2 write timeout")?
            .map_err(|e| e.to_string())?;
        if session.closed {
            return Ok(());
        }
    }
}
pub async fn start(
    state: Shared,
    bind_ip: Ipv4Addr,
    port: u16,
    credentials: Credentials,
) -> std::result::Result<Vec<tokio::task::JoinHandle<()>>, String> {
    start_mode(state, bind_ip, port, credentials, false).await
}
pub async fn start_mode(state: Shared, bind_ip: Ipv4Addr, port: u16, credentials: Credentials, automatic: bool) -> std::result::Result<Vec<tokio::task::JoinHandle<()>>, String> {
    let listener = crate::privilege::tcp((bind_ip, port))
        .await
        .map_err(|e| format!("SMB2 {port}: {e}"))?;
    let mut tasks = Vec::new();
    if automatic && port == 445 {
        if let Ok(netbios) = crate::privilege::tcp((bind_ip, 139)).await {
            tasks.push(tokio::spawn(crate::smb1::run(state.clone(), netbios, true)));
        }
    }
    let guid = *uuid::Uuid::new_v4().as_bytes();
    tasks.push(tokio::spawn(async move {
        let mut clients = JoinSet::new();
        loop {
            tokio::select! {
                accepted=listener.accept()=>{let Ok((stream,_))=accepted else{break};if clients.len()>=16{drop(stream);continue;}
                    let legacy = automatic && stream.peer_addr().ok().and_then(|peer| match peer.ip() { std::net::IpAddr::V4(ip) => state.lock().ok()?.profile_for(ip), _ => None }).is_some_and(|p| p.kind == "windows-ris");
                    let state = state.clone();
                    let session=Session::new(state.clone(),credentials.clone(),guid);clients.spawn(async move{if legacy { let _=crate::smb1::serve(stream,state,false).await; } else { let _=serve(stream,session).await; }});},
                _=clients.join_next(),if !clients.is_empty()=>{}
            }
        }
    }));
    Ok(tasks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Config, Store};
    use std::sync::Mutex;
    fn fixture(root: &std::path::Path) -> Shared {
        let mut config = Config::default();
        config.devices.push(crate::model::Device {
            id: "test-device".into(),
            ..Default::default()
        });
        for id in ["allowed", "other"] {
            config.profiles.push(serde_json::from_value(serde_json::json!({"id":id,"name":id,"architecture":"x64","kind":"windows","root":root})).unwrap());
        }
        Arc::new(Mutex::new(Store {
            config,
            events: vec![],
            path: root.join("unused-state"),
        }))
    }
    #[tokio::test]
    async fn automatic_routes_each_connection_by_selected_image() {
        let state = fixture(std::path::Path::new("/tmp"));
        {
            let mut store = state.lock().unwrap();
            store.config.devices[0].ip = Some(Ipv4Addr::LOCALHOST);
            store.config.devices[0].profile_id = Some("allowed".into());
            store.config.profiles[1].kind = "windows-ris".into();
        }
        let reserved = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = reserved.local_addr().unwrap().port();
        drop(reserved);
        let tasks = start_mode(state.clone(), Ipv4Addr::LOCALHOST, port, Default::default(), true).await.unwrap();
        async fn negotiate(stream: &mut TcpStream, offer: &[u8]) -> Vec<u8> {
            let mut packet = vec![0u8;35];
            packet[..4].copy_from_slice(b"\xffSMB"); packet[4]=0x72;
            packet[33..35].copy_from_slice(&(offer.len() as u16).to_le_bytes()); packet.extend_from_slice(offer);
            stream.write_u32(packet.len() as u32).await.unwrap();
            stream.write_all(&packet).await.unwrap();
            let length = stream.read_u32().await.unwrap();
            let mut reply=vec![0;length as usize]; stream.read_exact(&mut reply).await.unwrap(); reply
        }
        let mut modern = TcpStream::connect((Ipv4Addr::LOCALHOST,port)).await.unwrap();
        assert_eq!(&negotiate(&mut modern,b"\x02SMB 2.002\0").await[..4],b"\xfeSMB");
        state.lock().unwrap().config.devices[0].profile_id = Some("other".into());
        let mut legacy = TcpStream::connect((Ipv4Addr::LOCALHOST,port)).await.unwrap();
        assert_eq!(&negotiate(&mut legacy,b"\x02NT LM 0.12\0").await[..4],b"\xffSMB");
        for task in tasks { task.abort(); let _ = task.await; }
    }
    #[test]
    fn rejects_unsafe_names_and_write_access() {
        for name in [
            "..\\secret",
            "allowed\\..\\secret",
            "allowed\\file:stream",
            "\\\\server\\share",
            "allowed\\x\0",
            "allowed\\x.",
        ] {
            assert!(path(name).is_err(), "{name:?}");
        }
        let root = std::env::temp_dir().join(format!("bootah-smb2-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("setup.exe"), b"setup").unwrap();
        let state = fixture(&root);
        let tree = Tree::load(&state, "allowed").unwrap();
        assert!(tree.node("other/setup.exe").is_err());
        assert!(tree.node("allowed/SETUP.EXE").is_ok());
        let credentials = Arc::new(RwLock::new(vec![]));
        let mut s = Session::new(state, credentials, [0; 16]);
        s.trees.insert(1, Arc::new(tree));
        let mut req = vec![0; 120];
        set16(&mut req, 64, 57);
        set32(&mut req, 88, 2);
        set32(&mut req, 100, 1);
        assert_eq!(s.create(&req, 1).unwrap_err(), DENIED);
        let filename = wide("allowed\\setup.exe");
        set32(&mut req, 88, 0x80000000);
        set16(&mut req, 108, 120);
        set16(&mut req, 110, filename.len() as u16);
        req.extend(filename);
        let created = s.create(&req, 1).unwrap();
        let id = get64(&created, 64).unwrap();
        let mut info = vec![0; 104];
        set16(&mut info, 64, 41);
        info[66] = 1;
        info[67] = 34;
        set32(&mut info, 68, 56);
        set64(&mut info, 88, id);
        set64(&mut info, 96, s.cookie);
        let info = s.query(&info, 1, false).unwrap();
        assert_eq!(get64(&info, 8 + 40).unwrap(), 5);
        assert_eq!(get32(&info, 8 + 48).unwrap(), 0x21);
        assert_eq!(get64(&info, 8).unwrap(), s.start_time);

        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn multi_protocol_handshake() {
        let root = std::env::temp_dir();
        let state = fixture(&root);
        for (offers, expected) in [
            (b"\x02NT LM 0.12\0\x02SMB 2.002\0".as_slice(), 0x202),
            (b"\x02SMB 2.002\0\x02SMB 2.???\0".as_slice(), 0x2ff),
        ] {
            let mut session = Session::new(state.clone(), Arc::new(RwLock::new(vec![])), [0; 16]);
            let mut request = vec![0; 35];
            request[..4].copy_from_slice(b"\xffSMB");
            request[4] = 0x72;
            set16(&mut request, 33, offers.len() as u16);
            request.extend(offers);
            let response = session.process(&request).unwrap();
            assert_eq!(&response[..4], b"\xfeSMB");
            assert_eq!(get16(&response, 68).unwrap(), expected);
            assert_eq!(get64(&response, 24).unwrap(), 0);
            assert!(session.process(&request).is_err());
            if expected == 0x2ff {
                let mut request = vec![0; 104];
                request[..4].copy_from_slice(b"\xfeSMB");
                set16(&mut request, 4, 64);
                set16(&mut request, 14, 1);
                set64(&mut request, 24, 1);
                set16(&mut request, 64, 36);
                set16(&mut request, 66, 2);
                set16(&mut request, 68, 1);
                set16(&mut request, 100, 0x202);
                set16(&mut request, 102, 0x210);
                assert_eq!(
                    get16(&session.process(&request).unwrap(), 68).unwrap(),
                    0x210
                );
            }
        }
        let mut session = Session::new(state, Arc::new(RwLock::new(vec![])), [0; 16]);
        let offers = b"\x02NT LM 0.12\0";
        let mut request = vec![0; 35];
        request[..4].copy_from_slice(b"\xffSMB");
        request[4] = 0x72;
        set16(&mut request, 33, offers.len() as u16);
        request.extend(offers);
        assert_eq!(session.process(&request).unwrap_err(), UNSUPPORTED);
    }
    #[tokio::test]
    #[ignore = "requires BOOTAH_SMB2_CLIENT pointing to an independent client script and BOOTAH_SMB2_PYTHON"]
    async fn independent_client() {
        let script = std::env::var("BOOTAH_SMB2_CLIENT").expect("client script");
        let python = std::env::var("BOOTAH_SMB2_PYTHON").expect("python interpreter");
        let root =
            std::env::temp_dir().join(format!("bootah-smb2-interop-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("sources")).unwrap();
        std::fs::write(
            root.join("sources/test.bin"),
            b"Bootah SMB2 independent client payload",
        )
        .unwrap();
        let state = fixture(&root);
        let credentials = Arc::new(RwLock::new(vec![Credential {
            username: "test".into(),
            password: "TestingPassword42".into(),
            profile_id: "allowed".into(),
            device_id: "test-device".into(),
            expires_at: now() + 600,
        }]));
        let expected_file = root.join("iso-ranges.json");
        if let Ok(media_root) = std::env::var("BOOTAH_SMB2_MEDIA_ROOT") {
            let media_root = PathBuf::from(media_root);
            let entries = media_source::list(&media_root).expect("registered ISO source");
            let (name, entry) = entries
                .iter()
                .filter(|(_, e)| !e.is_dir && e.link.is_none())
                .max_by_key(|(_, e)| e.size)
                .unwrap();
            let mut source = media_source::open(&media_root, name).unwrap();
            let mut ranges = Vec::new();
            for offset in [0, entry.size / 2, entry.size.saturating_sub(4096)] {
                source.seek(SeekFrom::Start(offset)).unwrap();
                let mut bytes = vec![0; 4096.min(entry.size.saturating_sub(offset) as usize)];
                source.read_exact(&mut bytes).unwrap();
                ranges.push(serde_json::json!({"offset":offset,"bytes":bytes}));
            }
            std::fs::write(
                &expected_file,
                serde_json::to_vec(&serde_json::json!({"path":name,"ranges":ranges})).unwrap(),
            )
            .unwrap();
            state.lock().unwrap().config.profiles.push(serde_json::from_value(serde_json::json!({"id":"media","name":"media","architecture":"arm64","kind":"windows","root":media_root})).unwrap());
            credentials.write().unwrap().push(Credential {
                username: "media".into(),
                password: "TestingPassword42".into(),
                profile_id: "media".into(),
                device_id: "test-device".into(),
                expires_at: now() + 600,
            });
        }
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let task = tokio::spawn(async move {
            let mut clients = JoinSet::new();
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let s = Session::new(state.clone(), credentials.clone(), [0; 16]);
                clients.spawn(async move {
                    if let Err(error) = serve(stream, s).await {
                        eprintln!("interop: {error}");
                    }
                });
            }
        });
        let result = tokio::time::timeout(
            Duration::from_secs(90),
            tokio::process::Command::new(python)
                .arg(script)
                .arg(port.to_string())
                .arg(expected_file)
                .status(),
        )
        .await;
        task.abort();
        let _ = task.await;
        std::fs::remove_dir_all(root).unwrap();
        assert!(result.unwrap().unwrap().success());
    }
}
