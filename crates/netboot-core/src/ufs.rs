//! Solaris big-endian UFS1 on Sun-labelled installation media.
//! Layout references: illumos ufs_fs.h, ufs_inode.h and ufs_fsdir.h.
use std::{
    collections::{BTreeMap, HashSet},
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
};
#[derive(Clone)]
pub struct Entry {
    pub mode: u16,
    pub size: u64,
    pub extents: Vec<(u64, u64)>,
    pub link: Option<String>,
}
pub struct Image {
    file: File,
    start: u64,
    end: u64,
    block: u64,
    frag: u64,
    iblk: u64,
    cgoffset: u64,
    cgmask: u32,
    ipg: u64,
    fpg: u64,
}
fn u32be(b: &[u8], n: usize) -> u32 {
    u32::from_be_bytes(b[n..n + 4].try_into().unwrap())
}
impl Image {
    pub fn open(path: &Path) -> Result<Option<Self>, String> {
        let mut file = File::open(path).map_err(|e| e.to_string())?;
        let len = file.metadata().map_err(|e| e.to_string())?.len();
        let mut label = [0; 512];
        file.read_exact(&mut label).map_err(|e| e.to_string())?;
        if label[508..510] != [0xda, 0xbe] {
            return Ok(None);
        }
        if label
            .chunks_exact(2)
            .fold(0u16, |a, b| a ^ u16::from_be_bytes([b[0], b[1]]))
            != 0
        {
            return Err("Sun label checksum mismatch".into());
        }
        let spc = u16::from_be_bytes([label[436], label[437]]) as u64
            * u16::from_be_bytes([label[438], label[439]]) as u64;
        for part in 0..8 {
            let n = 444 + part * 8;
            let start = u32be(&label, n) as u64 * spc * 512;
            let end = start + u32be(&label, n + 4) as u64 * 512;
            if end > len || end < start + 16384 {
                continue;
            }
            file.seek(SeekFrom::Start(start + 8192))
                .map_err(|e| e.to_string())?;
            let mut sb = [0; 8192];
            file.read_exact(&mut sb).map_err(|e| e.to_string())?;
            if u32be(&sb, 1372) != 0x11954 {
                continue;
            }
            let block = u32be(&sb, 48) as u64;
            let frag = u32be(&sb, 52) as u64;
            if ![4096, 8192, 16384, 32768, 65536].contains(&block)
                || !frag.is_power_of_two()
                || frag < 512
                || frag > block
                || u32be(&sb, 184) == 0
            {
                return Err("Unsupported UFS geometry".into());
            }
            return Ok(Some(Self {
                file,
                start,
                end,
                block,
                frag,
                iblk: u32be(&sb, 16) as u64,
                cgoffset: u32be(&sb, 24) as u64,
                cgmask: u32be(&sb, 28),
                ipg: u32be(&sb, 184) as u64,
                fpg: u32be(&sb, 188) as u64,
            }));
        }
        Ok(None)
    }
    fn read(&mut self, offset: u64, length: usize) -> Result<Vec<u8>, String> {
        if offset < self.start
            || offset
                .checked_add(length as u64)
                .is_none_or(|e| e > self.end)
        {
            return Err("UFS read outside slice".into());
        }
        let mut b = vec![0; length];
        self.file
            .seek(SeekFrom::Start(offset))
            .map_err(|e| e.to_string())?;
        self.file.read_exact(&mut b).map_err(|e| e.to_string())?;
        Ok(b)
    }
    fn indirect(
        &mut self,
        block: u32,
        level: u32,
        remaining: &mut u64,
        out: &mut Vec<(u64, u64)>,
    ) -> Result<(), String> {
        if *remaining == 0 {
            return Ok(());
        }
        if block == 0 {
            return Err("Sparse UFS blocks unsupported".into());
        }
        let data = self.read(self.start + block as u64 * self.frag, self.block as usize)?;
        for part in data.chunks_exact(4) {
            if *remaining == 0 {
                break;
            }
            let p = u32::from_be_bytes(part.try_into().unwrap());
            if level == 1 {
                self.push(p, remaining, out)?
            } else {
                self.indirect(p, level - 1, remaining, out)?
            }
        }
        Ok(())
    }
    fn push(
        &self,
        block: u32,
        remaining: &mut u64,
        out: &mut Vec<(u64, u64)>,
    ) -> Result<(), String> {
        let size = (*remaining).min(self.block);
        let at = self.start + block as u64 * self.frag;
        if block == 0 || at.checked_add(size).is_none_or(|e| e > self.end) {
            return Err("Invalid UFS data block".into());
        }
        out.push((at, size));
        *remaining -= size;
        Ok(())
    }
    fn inode(&mut self, number: u32) -> Result<Entry, String> {
        let ino = number as u64;
        let cg = ino / self.ipg;
        let local = ino % self.ipg;
        let inopb = self.block / 128;
        let address = self.start
            + (cg * self.fpg
                + self.cgoffset * (cg & !(self.cgmask as u64))
                + self.iblk
                + (local / inopb) * (self.block / self.frag))
                * self.frag
            + (local % inopb) * 128;
        let data = self.read(address, 128)?;
        let mode = u16::from_be_bytes([data[0], data[1]]);
        let size = u64::from_be_bytes(data[8..16].try_into().unwrap());
        if size > 8 * 1024 * 1024 * 1024 {
            return Err("UFS inode exceeds supported size".into());
        }
        let mut entry = Entry {
            mode,
            size,
            extents: Vec::new(),
            link: None,
        };
        if ![0o100000, 0o40000, 0o120000].contains(&(mode & 0o170000)) {
            return Ok(entry);
        }
        if mode & 0o170000 == 0o120000 && size <= 56 && u32be(&data, 104) == 0 {
            entry.link = Some(
                String::from_utf8(data[40..40 + size as usize].to_vec())
                    .map_err(|_| "Invalid UFS link")?,
            );
            return Ok(entry);
        }
        let mut remaining = size;
        for i in 0..12 {
            if remaining == 0 {
                break;
            }
            self.push(u32be(&data, 40 + i * 4), &mut remaining, &mut entry.extents)?
        }
        for level in 1..=3 {
            if remaining == 0 {
                break;
            }
            self.indirect(
                u32be(&data, 88 + (level as usize - 1) * 4),
                level,
                &mut remaining,
                &mut entry.extents,
            )?
        }
        if remaining != 0 {
            return Err("Incomplete UFS block map".into());
        }
        if mode & 0o170000 == 0o120000 {
            entry.link = Some(
                String::from_utf8(self.contents(&entry, 65536)?).map_err(|_| "Invalid UFS link")?,
            )
        }
        Ok(entry)
    }
    pub fn contents(&mut self, entry: &Entry, limit: usize) -> Result<Vec<u8>, String> {
        if entry.size > limit as u64 {
            return Err("UFS read limit exceeded".into());
        }
        let mut data = Vec::new();
        for &(offset, size) in &entry.extents {
            data.extend(self.read(offset, size as usize)?)
        }
        Ok(data)
    }
    pub fn entries(&mut self) -> Result<BTreeMap<String, Entry>, String> {
        let mut entries = BTreeMap::new();
        self.walk(2, "", 0, &mut HashSet::new(), &mut entries)?;
        Ok(entries)
    }
    fn walk(
        &mut self,
        ino: u32,
        path: &str,
        depth: usize,
        seen: &mut HashSet<u32>,
        out: &mut BTreeMap<String, Entry>,
    ) -> Result<(), String> {
        if depth > 40 || out.len() > 100000 {
            return Err("UFS directory limit exceeded".into());
        }
        let entry = self.inode(ino)?;
        if entry.mode & 0o170000 == 0o40000 && seen.insert(ino) {
            let data = self.contents(&entry, 16 * 1024 * 1024)?;
            let mut pos = 0;
            while pos + 8 <= data.len() {
                let child = u32be(&data, pos);
                let reclen = u16::from_be_bytes([data[pos + 4], data[pos + 5]]) as usize;
                let namesize = u16::from_be_bytes([data[pos + 6], data[pos + 7]]) as usize;
                if reclen < 8
                    || !reclen.is_multiple_of(4)
                    || pos + reclen > data.len()
                    || namesize + 8 > reclen
                {
                    return Err("Invalid UFS directory record".into());
                }
                if child != 0 {
                    let name = std::str::from_utf8(&data[pos + 8..pos + 8 + namesize])
                        .map_err(|_| "Invalid UFS filename")?;
                    if name != "." && name != ".." {
                        if name.contains(['/', '\0']) {
                            return Err("Unsafe UFS filename".into());
                        }
                        let p = if path.is_empty() {
                            name.into()
                        } else {
                            format!("{path}/{name}")
                        };
                        self.walk(child, &p, depth + 1, seen, out)?
                    }
                }
                pos += reclen;
            }
        }
        if !path.is_empty() {
            out.insert(path.into(), entry);
        }
        Ok(())
    }
}
