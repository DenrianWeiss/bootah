use crate::model::*;
use std::{
    collections::{HashMap, VecDeque},
    fs::File,
    io::{Read, Seek, SeekFrom},
    time::{Duration, Instant},
};

const SERVICE: &[u8] = b"NETBOOT";
fn word(b: &[u8], p: usize) -> u16 {
    u16::from_le_bytes(b[p..p + 2].try_into().unwrap())
}
fn long(b: &[u8], p: usize) -> u32 {
    u32::from_le_bytes(b[p..p + 4].try_into().unwrap())
}
fn short(out: &mut Vec<u8>, n: u16) {
    out.extend(n.to_le_bytes());
}
fn number(out: &mut Vec<u8>, n: u32) {
    out.extend(n.to_le_bytes());
}
fn text(out: &mut Vec<u8>, value: &[u8]) {
    out.push(value.len() as u8);
    out.extend(value);
}
fn string<'a>(b: &'a [u8], p: &mut usize) -> Option<&'a [u8]> {
    let n = usize::from(*b.get(*p)?);
    *p += 1;
    let value = b.get(*p..p.checked_add(n)?)?;
    *p += n;
    Some(value)
}
fn frame(server: [u8; 6], client: [u8; 6], kind: u8, circuit: u16, body: &[u8]) -> Vec<u8> {
    let mut message = Vec::new();
    short(&mut message, (16 + body.len()) as u16);
    message.extend([kind, 1]);
    short(&mut message, circuit);
    message.extend(server);
    message.extend([0; 4]);
    message.extend(body);
    message.resize(message.len().div_ceil(4) * 4, 0);
    let sum = message
        .chunks_exact(4)
        .fold(0u32, |n, b| n.wrapping_add(long(b, 0)));
    number(&mut message, sum);
    let mut out = Vec::from(client);
    out.extend(server);
    out.extend([0x80, 0x41]);
    out.extend(message);
    out.resize(out.len().max(60), 0);
    out
}
fn run(subtype: u8, association: u16, reference: &[u8]) -> Vec<u8> {
    let mut out = vec![subtype, 0];
    short(&mut out, association);
    out.extend(reference);
    out
}
fn disk_description(request: &[u8], kind: u8, server: [u8; 6], blocks: u32) -> Vec<u8> {
    let mut out = request[..6].to_vec();
    out.extend([kind, if kind == 11 { 5 } else { 0 }]);
    short(&mut out, 3);
    short(&mut out, 1);
    if kind == 9 {
        out.extend([1, 5]);
        out.extend(server);
    }
    for n in [512, blocks, 32768, 32, 0, 1, 0] {
        number(&mut out, n);
    }
    if kind == 9 {
        out.extend(&request[10..14]);
        short(&mut out, 100);
    } else {
        short(&mut out, 1);
        short(&mut out, 0);
    }
    for value in [SERVICE, SERVICE, b"DKA0", b"RRD42"] {
        text(&mut out, value);
    }
    out.push(0);
    out
}
struct Association {
    peer: u16,
    segment: usize,
    profile: String,
    size: u64,
}
struct Circuit {
    peer: u16,
    touched: Instant,
    association: Option<Association>,
}
#[derive(Default)]
pub struct Server {
    circuits: HashMap<[u8; 6], Circuit>,
    pending: VecDeque<Vec<u8>>,
}
impl Server {
    pub fn tick(&mut self) -> Vec<Vec<u8>> {
        self.circuits
            .retain(|_, c| c.touched.elapsed() < Duration::from_secs(120));
        self.pending.drain(..self.pending.len().min(32)).collect()
    }
    pub fn handle(&mut self, state: &Shared, server: [u8; 6], ethernet: &[u8]) -> Option<Vec<u8>> {
        if ethernet.len() < 30 || ethernet[6] & 1 != 0 || ethernet[6..12] == [0; 6] {
            return None;
        }
        let destination = &ethernet[..6];
        if destination != server && destination[..4] != [9, 0, 0x2b, 4] {
            return None;
        }
        let client: [u8; 6] = ethernet[6..12].try_into().ok()?;
        let payload = &ethernet[14..];
        let size = usize::from(word(payload, 0));
        if !(16..=1500).contains(&size) || size > payload.len() {
            return None;
        }
        if payload[3] & 1 != 0 {
            let end = size.div_ceil(4) * 4;
            let bytes = payload.get(..end + 4)?;
            if bytes[..end]
                .chunks_exact(4)
                .fold(0u32, |n, b| n.wrapping_add(long(b, 0)))
                != long(bytes, end)
            {
                return None;
            }
        }
        let message = &payload[..size];
        let profile = {
            let mut store = state.lock().unwrap();
            let index = store.observe_mac(&mac(&client), "LASTport")?;
            let assigned = store.config.devices[index].profile_id.as_ref()?;
            store
                .config
                .profiles
                .iter()
                .find(|p| &p.id == assigned)?
                .clone()
        };
        let path = safe_file(&profile.root, profile.boot_files.get("lad")?).ok()?;
        let mut file = File::open(path).ok()?;
        let media_size = file.metadata().ok()?.len();
        if media_size == 0
            || !media_size.is_multiple_of(512)
            || media_size / 512 > u64::from(u32::MAX)
        {
            return None;
        }
        let blocks = (media_size / 512) as u32;
        match message[2] {
            6 => {
                if size < 51 || message[16] != 3 {
                    return None;
                }
                let mut p = 48;
                let name = string(message, &mut p)?;
                let length = usize::from(word(message.get(p..p + 2)?, 0));
                p += 2;
                let descriptor = message.get(p..p + length)?;
                let class = word(message, 42);
                let application = if class == 1 {
                    if descriptor.len() < 22
                        || descriptor[0] != 3
                        || descriptor[6] != 8
                        || !matches!(word(descriptor, 8), 3 | 65535)
                    {
                        return None;
                    }
                    let mut position = 16;
                    let requested = string(descriptor, &mut position)?;
                    if !requested.is_empty() && !requested.eq_ignore_ascii_case(SERVICE) {
                        return None;
                    }
                    let _ = string(descriptor, &mut position)?;
                    let node = string(descriptor, &mut position)?;
                    if !node.is_empty() && !node.eq_ignore_ascii_case(SERVICE) {
                        return None;
                    }
                    disk_description(descriptor, 9, server, blocks)
                } else if class == 0 {
                    Vec::new()
                } else {
                    return None;
                };
                let mut body = vec![3, message[17], 3, 3, 2, SERVICE.len() as u8];
                body.extend(SERVICE);
                body.resize(22, 0);
                body.extend(&message[38..42]);
                short(&mut body, class);
                short(&mut body, 100);
                short(&mut body, 1);
                text(&mut body, name);
                short(&mut body, application.len() as u16);
                body.extend(application);
                state.lock().unwrap().event(
                    &mac(&client),
                    "LAD",
                    "Offered read-only NETBOOT media",
                );
                Some(frame(server, client, 7, 0, &body))
            }
            1 => {
                if size < 49
                    || message[22] != 3
                    || (self.circuits.len() >= 32 && !self.circuits.contains_key(&client))
                {
                    return None;
                }
                let peer = word(message, 16);
                if peer == 0 {
                    return None;
                }
                self.circuits.insert(
                    client,
                    Circuit {
                        peer,
                        touched: Instant::now(),
                        association: None,
                    },
                );
                let mut body = Vec::new();
                short(&mut body, 1);
                short(&mut body, word(message, 18) & 2);
                short(&mut body, word(message, 20).clamp(512, 1492));
                body.extend([3, message[23]]);
                short(&mut body, 1);
                short(&mut body, 0);
                short(&mut body, word(message, 28).clamp(3, 60));
                short(&mut body, 1);
                body.push(SERVICE.len() as u8);
                body.extend(SERVICE);
                body.resize(33, 0);
                Some(frame(server, client, 2, peer, &body))
            }
            3 => {
                self.circuits.remove(&client);
                None
            }
            0 => {
                if size < 24 || word(message, 4) != 1 {
                    return None;
                }
                let circuit = self.circuits.get_mut(&client)?;
                circuit.touched = Instant::now();
                let reference = &message[20..24];
                match message[16] {
                    2 => {
                        if size < 34 || word(message, 26) != 1 || word(message, 24) == 0 {
                            return None;
                        }
                        let mut p = 31;
                        let name = string(message, &mut p)?;
                        let length = usize::from(word(message.get(p..p + 2)?, 0));
                        p += 2;
                        let request = message.get(p..p + length)?;
                        if request.len() < 33
                            || request[0] != 3
                            || request[6] != 10
                            || word(request, 8) != 3
                        {
                            return None;
                        }
                        let mut at = 32;
                        let instance = string(request, &mut at)?;
                        if !instance.eq_ignore_ascii_case(SERVICE) || word(request, 12) & 4 != 0 {
                            return None;
                        }
                        let segment = usize::from(word(message, 28)).clamp(256, 1400);
                        let peer = word(message, 24);
                        let mut body = run(3, peer, reference);
                        short(&mut body, 1);
                        short(&mut body, segment as u16);
                        body.push(1);
                        text(&mut body, name);
                        let application = disk_description(request, 11, server, blocks);
                        short(&mut body, application.len() as u16);
                        body.extend(application);
                        circuit.association = Some(Association {
                            peer,
                            segment,
                            profile: profile.id,
                            size: media_size,
                        });
                        state.lock().unwrap().event(
                            &mac(&client),
                            "LAD",
                            "Opened read-only NETBOOT media",
                        );
                        Some(frame(server, client, 0, circuit.peer, &body))
                    }
                    0 => {
                        let association = circuit.association.as_mut()?;
                        if word(message, 18) != 1
                            || association.profile != profile.id
                            || association.size != media_size
                            || size < 44
                            || message[24] == 0
                            || message[26..28] != [1, 1]
                        {
                            return None;
                        }
                        let length = usize::from(word(message, 30));
                        let request = message.get(32..32 + length)?;
                        if request.len() < 12 {
                            return None;
                        }
                        let count = long(request, 8) as usize;
                        let offset = u64::from(long(request, 4)) * 512;
                        let (kind, status, data) = match request[0] {
                            2 if count > 0
                                && count <= 128 * 1024
                                && count + 8 <= association.segment * 255
                                && offset
                                    .checked_add(count as u64)
                                    .is_some_and(|end| end <= association.size) =>
                            {
                                let mut data = vec![0; count];
                                match file
                                    .seek(SeekFrom::Start(offset))
                                    .and_then(|_| file.read_exact(&mut data))
                                {
                                    Ok(()) => (4, 1, data),
                                    Err(_) => (4, -5, Vec::new()),
                                }
                            }
                            2 => (4, -7, Vec::new()),
                            3 => (5, -2, Vec::new()),
                            6 => (7, 1, Vec::new()),
                            _ => return None,
                        };
                        let mut application = vec![kind, 0, status as i8 as u8];
                        number(&mut application, data.len() as u32);
                        application.push(0);
                        application.extend(data);
                        if self.pending.len() + application.len().div_ceil(association.segment)
                            > 512
                        {
                            return None;
                        }
                        let segments = application.len().div_ceil(association.segment) as u8;
                        for (index, part) in application.chunks(association.segment).enumerate() {
                            let mut body = run(1, association.peer, reference);
                            body.extend([
                                message[24],
                                message[25],
                                segments,
                                index as u8 + 1,
                                0,
                                0,
                            ]);
                            short(&mut body, part.len() as u16);
                            body.extend(part);
                            self.pending
                                .push_back(frame(server, client, 0, circuit.peer, &body));
                        }
                        self.pending.pop_front()
                    }
                    6 => {
                        let association = circuit.association.take()?;
                        let mut body = run(7, association.peer, reference);
                        short(&mut body, 4);
                        Some(frame(server, client, 0, circuit.peer, &body))
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }
}
