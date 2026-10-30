use crate::model::*;
use std::{
    collections::HashMap,
    fs::File,
    io::Read,
    time::{Duration, Instant},
};

struct Load {
    file: File,
    address: u32,
    entry: u32,
    count: u8,
    block_size: usize,
    packet: Vec<u8>,
    final_packet: bool,
    seen: Instant,
}
impl Load {
    fn open(path: &std::path::Path, block_size: usize) -> Result<Self, String> {
        let mut file = File::open(path).map_err(|e| e.to_string())?;
        if file.metadata().map_err(|e| e.to_string())?.len() > 64 * 1024 * 1024 {
            return Err("MOP bootstrap exceeds 64 MiB".into());
        }
        let mut header = [0; 512];
        file.read_exact(&mut header).map_err(|e| e.to_string())?;
        let word = |offset| {
            header
                .get(offset..offset + 2)
                .map(|b| u16::from_le_bytes(b.try_into().unwrap()))
        };
        let long = |offset| {
            header
                .get(offset..offset + 4)
                .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
        };
        let (address, entry) = match word(510) {
            Some(5) => (0, 0),
            Some(0xffff) => {
                let section = word(0).unwrap() as usize;
                let activation = word(2).unwrap() as usize;
                (
                    word(section + 4).ok_or("Invalid MOP section header")? as u32 * 512,
                    long(activation).ok_or("Invalid MOP activation header")? & 0x7fffffff,
                )
            }
            _ => return Err("MOP requires a native Alpha/VAX bootstrap image".into()),
        };
        Ok(Self {
            file,
            address,
            entry,
            count: 0,
            block_size,
            packet: vec![],
            final_packet: false,
            seen: Instant::now(),
        })
    }
    fn advance(&mut self, server: [u8; 6]) -> Result<(), String> {
        let mut data = vec![0; self.block_size];
        let mut length = 0;
        while length < data.len() {
            let n = self
                .file
                .read(&mut data[length..])
                .map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            length += n;
        }
        if length == 0 {
            self.packet = vec![20, self.count, 3, 7];
            self.packet.extend(b"Netboot");
            self.packet.extend([4, 6]);
            self.packet.extend(server);
            self.packet.push(0);
            self.packet.extend(self.entry.to_le_bytes());
            self.final_packet = true;
        } else {
            self.packet = vec![2, self.count];
            self.packet.extend(self.address.to_le_bytes());
            self.packet.extend(&data[..length]);
            self.address = self
                .address
                .checked_add(length as u32)
                .ok_or("MOP address overflow")?;
        }
        Ok(())
    }
}
#[derive(Default)]
pub struct Server {
    loads: HashMap<[u8; 6], Load>,
}
impl Server {
    pub fn handle(&mut self, state: &Shared, server: [u8; 6], frame: &[u8]) -> Option<Vec<u8>> {
        let snap = frame.get(14..22) == Some(&[0xaa, 0xaa, 3, 8, 0, 0x2b, 0x60, 1]);
        let data = if snap {
            let length = u16::from_be_bytes(frame.get(12..14)?.try_into().ok()?) as usize;
            if !(8..=1500).contains(&length) {
                return None;
            }
            frame.get(22..14 + length)?
        } else {
            if frame.get(12..14) != Some(&[0x60, 1]) {
                return None;
            }
            let length = u16::from_le_bytes(frame.get(14..16)?.try_into().ok()?) as usize;
            if length > 1500 {
                return None;
            }
            frame.get(16..16 + length)?
        };
        let client: [u8; 6] = frame.get(6..12)?.try_into().ok()?;
        if client[0] & 1 != 0 || client == [0; 6] {
            return None;
        }
        let destination: [u8; 6] = frame.get(..6)?.try_into().ok()?;
        let multicast = destination == [0xab, 0, 0, 1, 0, 0];
        if !multicast && destination != server {
            return None;
        }
        self.loads
            .retain(|_, load| load.seen.elapsed() < Duration::from_secs(30));
        let reply = match *data.first()? {
            8 => {
                if !matches!(data.get(2), Some(1 | 4)) {
                    return None;
                }
                let length = *data.get(4)? as usize;
                let name = std::str::from_utf8(data.get(5..5 + length)?).ok()?;
                let mut index = 6 + length;
                if index > data.len() {
                    return None;
                }
                let mut block_size = 1492;
                while index < data.len() {
                    let kind = u16::from_le_bytes(data.get(index..index + 2)?.try_into().ok()?);
                    let size = *data.get(index + 2)? as usize;
                    let value = data.get(index + 3..index + 3 + size)?;
                    if kind == 401 && size == 2 {
                        block_size = u16::from_le_bytes(value.try_into().ok()?) as usize;
                    }
                    index += 3 + size;
                }
                block_size = if block_size == 1030 {
                    1000
                } else {
                    block_size.clamp(64, 1492)
                };
                if snap {
                    block_size -= 8;
                }
                let profile = {
                    let mut store = state.lock().unwrap();
                    let index = store.observe_mac(&mac(&client), "MOP")?;
                    let assigned = store.config.devices[index].profile_id.clone();
                    if let Err(e) = store.save() {
                        store.event(&mac(&client), "storage", e);
                    }
                    store
                        .config
                        .profiles
                        .iter()
                        .find(|p| Some(&p.id) == assigned.as_ref())?
                        .clone()
                };
                let mapped = profile.boot_files.get("mop").unwrap_or(&profile.kernel);
                let requested = if name.is_empty()
                    || std::path::Path::new(mapped).file_name().is_some_and(|n| {
                        n.to_string_lossy().eq_ignore_ascii_case(name)
                            || n.to_string_lossy()
                                .eq_ignore_ascii_case(&format!("{name}.SYS"))
                    }) {
                    mapped.clone()
                } else {
                    profile
                        .boot_files
                        .get(name)
                        .cloned()
                        .unwrap_or_else(|| name.to_string())
                };
                let path = safe_file(&profile.root, &requested).ok()?;
                if multicast {
                    vec![3]
                } else {
                    if self.loads.len() >= 32 && !self.loads.contains_key(&client) {
                        return None;
                    }
                    match Load::open(&path, block_size).and_then(|mut load| {
                        load.advance(server)?;
                        Ok(load)
                    }) {
                        Ok(load) => {
                            let reply = load.packet.clone();
                            self.loads.insert(client, load);
                            state.lock().unwrap().event(
                                &mac(&client),
                                "MOP",
                                format!("Loading {requested}"),
                            );
                            reply
                        }
                        Err(e) => {
                            state.lock().unwrap().event(&mac(&client), "MOP", e);
                            return None;
                        }
                    }
                }
            }
            10 if !multicast => {
                let count = *data.get(1)?;
                if *data.get(2)? != 0 {
                    self.loads.remove(&client);
                    state
                        .lock()
                        .unwrap()
                        .event(&mac(&client), "MOP", "Client aborted load");
                    return None;
                }
                let load = self.loads.get_mut(&client)?;
                load.seen = Instant::now();
                if count == load.count.wrapping_add(1) {
                    if load.final_packet {
                        self.loads.remove(&client);
                        state
                            .lock()
                            .unwrap()
                            .event(&mac(&client), "MOP", "Bootstrap transferred");
                        return None;
                    }
                    load.count = count;
                    if let Err(e) = load.advance(server) {
                        self.loads.remove(&client);
                        state.lock().unwrap().event(&mac(&client), "MOP", e);
                        return None;
                    }
                } else if count != load.count {
                    return None;
                }
                load.packet.clone()
            }
            _ => return None,
        };
        let mut output = client.to_vec();
        output.extend(server);
        if snap {
            output.extend(((reply.len() + 8) as u16).to_be_bytes());
            output.extend([0xaa, 0xaa, 3, 8, 0, 0x2b, 0x60, 1]);
        } else {
            output.extend([0x60, 1]);
            output.extend((reply.len() as u16).to_le_bytes());
        }
        output.extend(reply);
        output.resize(output.len().max(60), 0);
        Some(output)
    }
}
