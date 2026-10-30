use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
};

fn word(b: &[u8], p: usize) -> u16 {
    u16::from_le_bytes([b[p], b[p + 1]])
}
fn long(b: &[u8], p: usize) -> u32 {
    u32::from_le_bytes(b[p..p + 4].try_into().unwrap())
}
fn checked(b: &[u8]) -> bool {
    b.len() == 512
        && b[..510]
            .chunks_exact(2)
            .fold(0u16, |s, w| s.wrapping_add(word(w, 0)))
            == word(b, 510)
}
type Extents = Vec<(u32, u32)>;
struct Volume {
    file: File,
    length: u64,
    index: Extents,
    headers: u32,
}
impl Volume {
    fn block(&mut self, lbn: u32) -> Result<Vec<u8>, String> {
        let offset = u64::from(lbn) * 512;
        if offset + 512 > self.length {
            return Err("ODS-2 block outside image".into());
        }
        self.file
            .seek(SeekFrom::Start(offset))
            .map_err(|e| e.to_string())?;
        let mut b = vec![0; 512];
        self.file.read_exact(&mut b).map_err(|e| e.to_string())?;
        Ok(b)
    }
    fn mapped(&mut self, extents: &Extents, mut vbn: u32) -> Result<Vec<u8>, String> {
        for &(lbn, count) in extents {
            if vbn < count {
                return self.block(lbn.checked_add(vbn).ok_or("ODS-2 extent overflow")?);
            }
            vbn -= count;
        }
        Err("ODS-2 virtual block outside file".into())
    }
    fn header(&mut self, fid: &[u8]) -> Result<Vec<u8>, String> {
        let number = u32::from(word(fid, 0)) | u32::from(fid[5]) << 16;
        let vbn = self
            .headers
            .checked_add(number)
            .and_then(|n| n.checked_sub(2))
            .ok_or("Invalid ODS-2 file ID")?;
        let b = self.mapped(&self.index.clone(), vbn)?;
        if !checked(&b) || &b[8..14] != fid {
            return Err("Invalid ODS-2 file header".into());
        }
        Ok(b)
    }
    fn contents(&mut self, fid: &[u8]) -> Result<Vec<u8>, String> {
        let mut header = self.header(fid)?;
        let eof = (u32::from(word(&header, 28)) << 16) | u32::from(word(&header, 30));
        let size = u64::from(eof.saturating_sub(1)) * 512 + u64::from(word(&header, 32));
        if size > 64 * 1024 * 1024 {
            return Err("ODS-2 bootstrap file exceeds size limit".into());
        }
        let mut extents = Vec::new();
        let mut seen = std::collections::HashSet::new();
        loop {
            extents.extend(decode_extents(&header)?);
            let next = header[14..20].to_vec();
            if next.iter().all(|b| *b == 0) {
                break;
            }
            if seen.len() >= 128 || !seen.insert(next.clone()) {
                return Err("Invalid ODS-2 extension chain".into());
            }
            header = self.header(&next)?;
        }
        let mut data = Vec::with_capacity(size as usize);
        for vbn in 0..size.div_ceil(512) {
            data.extend(self.mapped(&extents, vbn as u32)?);
        }
        data.truncate(size as usize);
        Ok(data)
    }
    fn lookup(&mut self, directory: &[u8], name: &str) -> Result<Vec<u8>, String> {
        let data = self.contents(directory)?;
        let mut found = None;
        for block in data.chunks(512) {
            let mut p = 0;
            while p + 6 <= block.len() {
                let size = usize::from(word(block, p));
                if size == 65535 {
                    break;
                }
                let end = p + size + 2;
                let n = usize::from(block[p + 5]);
                let entries = p + 6 + n.div_ceil(2) * 2;
                if size < 6
                    || end > block.len()
                    || p + 6 + n > end
                    || entries > end
                    || !(end - entries).is_multiple_of(8)
                {
                    return Err("Invalid ODS-2 directory record".into());
                }
                if block[p + 6..p + 6 + n].eq_ignore_ascii_case(name.as_bytes()) {
                    for e in (entries..end).step_by(8) {
                        let version = word(block, e);
                        if found.as_ref().is_none_or(|(v, _)| version > *v) {
                            found = Some((version, block[e + 2..e + 8].to_vec()));
                        }
                    }
                }
                p = end;
            }
        }
        found
            .map(|(_, fid)| fid)
            .ok_or_else(|| format!("ODS-2 file not found: {name}"))
    }
}
fn decode_extents(b: &[u8]) -> Result<Extents, String> {
    let mut p = usize::from(b[1]) * 2;
    let end = p + usize::from(b[58]) * 2;
    if p < 80 || end > 510 {
        return Err("Invalid ODS-2 retrieval map".into());
    }
    let mut out = Vec::new();
    while p < end {
        let w = word(b, p);
        let format = w >> 14;
        let size = [2, 4, 6, 8][format as usize];
        if p + size > end {
            return Err("Truncated ODS-2 retrieval pointer".into());
        }
        let extent = match format {
            1 => Some((
                (u32::from(w & 0x3f00) << 8) | u32::from(word(b, p + 2)),
                u32::from(w & 255) + 1,
            )),
            2 => Some((long(b, p + 2), u32::from(w & 0x3fff) + 1)),
            3 => Some((
                long(b, p + 4),
                ((u32::from(w & 0x3fff) << 16) | u32::from(word(b, p + 2))) + 1,
            )),
            _ => None,
        };
        if let Some(e) = extent {
            out.push(e);
        }
        p += size;
    }
    Ok(out)
}
pub fn alpha_bootstrap(path: &Path) -> Result<Option<(String, Vec<u8>)>, String> {
    let file = File::open(path).map_err(|e| e.to_string())?;
    let length = file.metadata().map_err(|e| e.to_string())?.len();
    if length < 1024 {
        return Ok(None);
    }
    let mut volume = Volume {
        file,
        length,
        index: vec![],
        headers: 0,
    };
    let home = volume.block(1)?;
    if &home[496..508] != b"DECFILE11B  " {
        return Ok(None);
    }
    if !checked(&home) || word(&home, 12) >> 8 != 2 {
        return Err("Invalid ODS-2 home block".into());
    }
    volume.headers = u32::from(word(&home, 22)) + u32::from(word(&home, 32));
    let index = volume.block(
        long(&home, 24)
            .checked_add(u32::from(word(&home, 32)))
            .ok_or("ODS-2 index overflow")?,
    )?;
    if !checked(&index) {
        return Err("Invalid ODS-2 index header".into());
    }
    volume.index = decode_extents(&index)?;
    let mut fid = vec![4, 0, 4, 0, 0, 0];
    for name in ["SYS0.DIR", "SYSEXE.DIR"] {
        fid = volume.lookup(&fid, name)?;
    }
    let directory = volume.contents(&fid)?;
    for block in directory.chunks(512) {
        let mut p = 0;
        while p + 6 <= block.len() {
            let size = usize::from(word(block, p));
            if size == 65535 {
                break;
            }
            let n = usize::from(block[p + 5]);
            if size < 6 || p + size + 2 > block.len() || p + 6 + n > p + size + 2 {
                return Err("Invalid ODS-2 bootstrap directory".into());
            }
            let name = String::from_utf8_lossy(&block[p + 6..p + 6 + n]);
            if name.starts_with("APB_") && name.ends_with(".SYS") {
                let bootstrap = volume.lookup(&fid, &name)?;
                let bytes = volume.contents(&bootstrap)?;
                if bytes.len() < 512 || word(&bytes, 510) != 5 {
                    return Err("Unsupported OpenVMS bootstrap architecture".into());
                }
                return Ok(Some((name.into_owned(), bytes)));
            }
            p += size + 2;
        }
    }
    Err("OpenVMS Alpha network bootstrap not found".into())
}
