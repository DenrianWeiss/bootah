use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

#[derive(Clone, Serialize, Deserialize)]
pub struct Entry {
    pub size: u64,
    pub is_dir: bool,
    #[serde(default)]
    pub mode: u16,
    #[serde(default)]
    pub link: Option<String>,
    #[serde(default)]
    link_root: Option<String>,
    extents: Vec<(u64, u64)>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Image {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    filesystem: String,
    #[serde(default)]
    fallback_reason: Option<String>,
    source: PathBuf,
    length: u64,
    modified: u128,
    entries: BTreeMap<String, Entry>,
    external: bool,
    #[serde(default)]
    aliases: BTreeMap<String, String>,
}
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
pub(crate) fn valid(name: &str) -> bool {
    !name.is_empty()
        && !name.contains(['\\', ':', '\0'])
        && !name.chars().any(char::is_control)
        && name
            .split('/')
            .all(|p| !p.is_empty() && p != "." && p != ".." && !p.starts_with(".bootah-"))
}
pub fn sanitize(name: &str) -> Result<String, String> {
    let name = name
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("/");
    if !valid(&name) {
        return Err("Unsafe file path".into());
    }
    Ok(name)
}
fn stamp(file: &File) -> Result<(u64, u128), String> {
    let m = file.metadata().map_err(err)?;
    Ok((
        m.len(),
        m.modified()
            .map_err(err)?
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(err)?
            .as_nanos(),
    ))
}
fn read_bytes(file: &mut File, offset: u64, size: usize, length: u64) -> Result<Vec<u8>, String> {
    if offset.checked_add(size as u64).is_none_or(|n| n > length) {
        return Err("Image extent outside source".into());
    }
    file.seek(SeekFrom::Start(offset)).map_err(err)?;
    let mut b = vec![0; size];
    file.read_exact(&mut b).map_err(err)?;
    Ok(b)
}
fn rock_ridge_name(file: &mut File, data: &[u8], length: u64) -> Result<(String, bool), String> {
    let mut areas = vec![(data.to_vec(), 0)];
    let mut seen = HashSet::new();
    let mut name = String::new();
    let mut link = false;
    let mut continued = false;
    while let Some((area, mut pos)) = areas.pop() {
        while pos + 4 <= area.len() {
            let len = area[pos + 2] as usize;
            if len < 4 || pos + len > area.len() {
                break;
            }
            let entry = &area[pos..pos + len];
            if matches!(&entry[..2], b"NM" | b"CE") && entry[3] != 1 {
                return Err("Unsupported ISO name extension".into());
            }
            match &entry[..2] {
                b"NM" if len > 5 && entry[4] & 6 == 0 => {
                    name.push_str(std::str::from_utf8(&entry[5..]).map_err(err)?);
                    continued = entry[4] & 1 != 0;
                    if name.len() > 4096 {
                        return Err("ISO name too long".into());
                    }
                }
                b"SL" => link = true,
                b"ST" => break,
                b"CE" if len >= 28 => {
                    let block = u32::from_le_bytes(entry[4..8].try_into().unwrap()) as u64;
                    let offset = u32::from_le_bytes(entry[12..16].try_into().unwrap()) as u64;
                    let size = u32::from_le_bytes(entry[20..24].try_into().unwrap()) as usize;
                    let start = block
                        .checked_mul(2048)
                        .and_then(|v| v.checked_add(offset))
                        .ok_or("Invalid ISO continuation")?;
                    if size > 1024 * 1024 || seen.len() >= 32 || !seen.insert((start, size)) {
                        return Err("Invalid ISO continuation".into());
                    }
                    let continuation = read_bytes(file, start, size, length)?;
                    areas.push((area, pos + len));
                    areas.push((continuation, 0));
                    break;
                }
                _ => {}
            }
            pos += len;
        }
    }
    if continued {
        return Err("Incomplete Rock Ridge filename".into());
    }
    Ok((name, link))
}
impl Image {
    pub fn open(source: &Path) -> Result<Self, String> {
        let source = source.canonicalize().map_err(err)?;
        let mut file = File::open(&source).map_err(err)?;
        let (length, modified) = stamp(&file)?;
        let mut image = Self {
            version: 3,
            filesystem: String::new(),
            fallback_reason: None,
            source,
            length,
            modified,
            entries: BTreeMap::new(),
            external: false,
            aliases: BTreeMap::new(),
        };
        let descriptor = read_bytes(&mut file, 16 * 2048, 2048, length)?;
        let udf = (17..32).any(|sector| {
            read_bytes(&mut file, sector * 2048, 2048, length)
                .is_ok_and(|d| matches!(&d[1..6], b"NSR02" | b"NSR03"))
        });
        if udf {
            match image.udf_index() {
                Ok(()) => image.filesystem = "udf".into(),
                Err(udf_error) => {
                    image.entries.clear();
                    if let Err(iso_error) = image.iso_index(&mut file, &descriptor) {
                        return Err(format!(
                            "Cannot decode UDF filesystem\n{udf_error}\nCannot decode ISO bridge filesystem\n{iso_error}"
                        ));
                    }
                    image.fallback_reason = Some(udf_error);
                }
            }
        } else {
            image.iso_index(&mut file, &descriptor)?;
        }
        if image.entries.is_empty() {
            return Err("Image contains no readable files".into());
        }
        Ok(image)
    }
    fn iso_index(&mut self, file: &mut File, descriptor: &[u8]) -> Result<(), String> {
        let length = self.length;
        if descriptor[0] != 1 || &descriptor[1..6] != b"CD001" {
            return Err("ISO9660/Joliet bridge not found".into());
        }
        let mut root = descriptor[156..190].to_vec();
        let sector = u32::from_le_bytes(root[2..6].try_into().unwrap()) as u64;
        let size = u32::from_le_bytes(root[10..14].try_into().unwrap()) as usize;
        let directory = read_bytes(file, sector * 2048, size, length)?;
        let susp_skip = directory
            .get(..directory.first().copied().unwrap_or(0) as usize)
            .and_then(|r| r.windows(7).find(|w| &w[..6] == b"SP\x07\x01\xbe\xef"))
            .map(|sp| sp[6] as usize);
        let rock_ridge = susp_skip.is_some();
        let mut joliet = false;
        if !rock_ridge {
            for sector in 17..64 {
                let d = read_bytes(file, sector * 2048, 2048, length)?;
                if &d[1..6] != b"CD001" || d[0] == 255 {
                    break;
                }
                if d[0] == 2 && matches!(&d[88..91], b"%/@" | b"%/C" | b"%/E") {
                    root = d[156..190].to_vec();
                    joliet = true;
                }
            }
        }
        self.walk(
            file,
            u32::from_le_bytes(root[2..6].try_into().unwrap()) as u64 * 2048,
            u32::from_le_bytes(root[10..14].try_into().unwrap()) as usize,
            "",
            0,
            &mut HashSet::new(),
            joliet,
            susp_skip.unwrap_or(0),
        )?;
        self.filesystem = if joliet {
            "joliet"
        } else if rock_ridge {
            "rock-ridge"
        } else {
            "iso9660"
        }
        .into();
        if self.entries.is_empty() {
            return Err("Image contains no readable files".into());
        }
        Ok(())
    }
    fn walk(
        &mut self,
        file: &mut File,
        offset: u64,
        size: usize,
        parent: &str,
        depth: usize,
        seen: &mut HashSet<u64>,
        joliet: bool,
        susp_skip: usize,
    ) -> Result<(), String> {
        if depth > 32 || size > 32 * 1024 * 1024 || !seen.insert(offset) {
            return Err("Invalid ISO directory".into());
        }
        let data = read_bytes(file, offset, size, self.length)?;
        let mut pos = 0;
        let mut multi = HashSet::new();
        while pos < data.len() {
            let n = data[pos] as usize;
            if n == 0 {
                pos = (pos / 2048 + 1) * 2048;
                continue;
            }
            if n < 34 || pos + n > data.len() {
                return Err("Invalid ISO record".into());
            }
            let r = &data[pos..pos + n];
            pos += n;
            let ns = r[32] as usize;
            if 33 + ns > r.len() {
                return Err("Invalid ISO name".into());
            }
            let raw = &r[33..33 + ns];
            if raw == [0] || raw == [1] {
                continue;
            }
            let mut name = if joliet {
                if !raw.len().is_multiple_of(2) {
                    return Err("Invalid Joliet filename".into());
                }
                String::from_utf16(
                    &raw.chunks_exact(2)
                        .map(|b| u16::from_be_bytes([b[0], b[1]]))
                        .collect::<Vec<_>>(),
                )
                .map_err(err)?
            } else {
                String::from_utf8(raw.to_vec()).map_err(err)?
            };
            if let Some((base, version)) = name.rsplit_once(';') {
                if !version.is_empty() && version.bytes().all(|b| b.is_ascii_digit()) {
                    name = base.trim_end_matches('.').into();
                }
            }
            if !joliet {
                let extra = 33 + ns + usize::from(ns.is_multiple_of(2));
                let (nm, link) = rock_ridge_name(
                    file,
                    r.get(extra + susp_skip..).unwrap_or_default(),
                    self.length,
                )?;
                if link {
                    continue;
                }
                if !nm.is_empty() {
                    name = nm;
                }
            }
            let path = if parent.is_empty() {
                name
            } else {
                format!("{parent}/{name}")
            };
            if !valid(&path) {
                return Err("Unsafe ISO name".into());
            }
            let start =
                (u32::from_le_bytes(r[2..6].try_into().unwrap()) as u64 + u64::from(r[1])) * 2048;
            let bytes = u32::from_le_bytes(r[10..14].try_into().unwrap()) as u64;
            let is_dir = r[25] & 2 != 0;
            if start.checked_add(bytes).is_none_or(|n| n > self.length) {
                return Err("ISO extent outside image".into());
            }
            if let Some(e) = self.entries.get_mut(&path) {
                if is_dir || e.is_dir || !multi.contains(&path) {
                    return Err("Duplicate ISO path".into());
                }
                e.size = e.size.checked_add(bytes).ok_or("ISO size overflow")?;
                e.extents.push((start, bytes));
            } else {
                self.entries.insert(
                    path.clone(),
                    Entry {
                        size: bytes,
                        is_dir,
                        mode: 0,
                        link: None,
                        link_root: None,
                        extents: vec![(start, bytes)],
                    },
                );
            }
            if r[25] & 0x80 != 0 {
                multi.insert(path.clone());
            } else {
                multi.remove(&path);
            }
            if self.entries.len() > 100_000 {
                return Err("Too many ISO entries".into());
            }
            if is_dir {
                self.walk(
                    file,
                    start,
                    bytes as usize,
                    &path,
                    depth + 1,
                    seen,
                    joliet,
                    susp_skip,
                )?
            }
        }
        if !multi.is_empty() {
            return Err("Incomplete ISO multi-extent file".into());
        }
        Ok(())
    }
    pub fn entries(&self) -> &BTreeMap<String, Entry> {
        &self.entries
    }
    pub fn path_at_iso_sector(&self, sector: u32) -> Option<&str> {
        let offset = u64::from(sector) * 2048;
        self.entries.iter().find_map(|(path, entry)| {
            entry
                .extents
                .first()
                .is_some_and(|(start, _)| *start == offset)
                .then_some(path.as_str())
        })
    }
    pub fn find(&self, name: &str) -> Option<&str> {
        self.entries
            .get_key_value(name)
            .map(|(k, _)| k.as_str())
            .or_else(|| {
                self.entries
                    .keys()
                    .find(|k| k.eq_ignore_ascii_case(name))
                    .map(String::as_str)
            })
    }
    fn check(&self) -> Result<File, String> {
        let f = File::open(&self.source).map_err(err)?;
        if stamp(&f)? != (self.length, self.modified) {
            return Err("Source ISO changed; import it again".into());
        }
        Ok(f)
    }
    pub fn read_range(&self, name: &str, offset: u64, length: usize) -> Result<Vec<u8>, String> {
        let mut r = self.resource(name, None)?;
        if offset > r.size {
            return Err("Read outside component".into());
        }
        r.seek(SeekFrom::Start(offset)).map_err(err)?;
        let mut bytes = vec![0; (length as u64).min(r.size - offset) as usize];
        r.read_exact(&mut bytes).map_err(err)?;
        Ok(bytes)
    }
    pub fn read(&self, name: &str, limit: usize) -> Result<Vec<u8>, String> {
        let mut resource = self.resource(name, None)?;
        if resource.size > limit as u64 {
            return Err("Image component exceeds read budget".into());
        }
        let mut b = Vec::with_capacity(resource.size as usize);
        resource.read_to_end(&mut b).map_err(err)?;
        Ok(b)
    }
    pub fn materialize(&self, name: &str, dest: &Path) -> Result<(), String> {
        if let Some(p) = dest.parent() {
            fs::create_dir_all(p).map_err(err)?
        }
        let mut src = self.resource(name, None)?;
        let mut out = File::create(dest).map_err(err)?;
        let size = src.size;
        if std::io::copy(&mut src, &mut out).map_err(err)? != size {
            return Err("Truncated media component".into());
        }
        Ok(())
    }
    fn resolve(&self, name: &str) -> Result<String, String> {
        let mut current = name.to_string();
        for (alias, target) in &self.aliases {
            if current == *alias || current.starts_with(&format!("{alias}/")) {
                current = format!("{target}{}", &current[alias.len()..]);
                break;
            }
        }
        for _ in 0..32 {
            let parts: Vec<_> = current.split('/').collect();
            let mut changed = false;
            for count in 1..=parts.len() {
                let prefix = parts[..count].join("/");
                let Some(entry) = self.find(&prefix).and_then(|key| self.entries.get(key)) else {
                    continue;
                };
                let Some(target) = &entry.link else { continue };
                let anchor = entry.link_root.as_deref().unwrap_or("");
                let base = if target.starts_with('/') {
                    anchor.to_string()
                } else {
                    parts[..count - 1].join("/")
                };
                let combined = format!(
                    "{base}/{}/{}",
                    target.trim_start_matches('/'),
                    parts[count..].join("/")
                );
                let mut normalized = Vec::new();
                for part in combined.split('/') {
                    match part {
                        "" | "." => {}
                        ".." => {
                            normalized.pop().ok_or("Link outside medium")?;
                        }
                        _ => normalized.push(part),
                    }
                }
                current = normalized.join("/");
                if !valid(&current)
                    || (!anchor.is_empty()
                        && current != anchor
                        && !current.starts_with(&format!("{anchor}/")))
                {
                    return Err("Link outside medium".into());
                }
                changed = true;
                break;
            }
            if !changed {
                return Ok(current);
            }
        }
        Err("Cyclic media symlink".into())
    }
    fn resource(&self, name: &str, _cache: Option<&Path>) -> Result<Resource, String> {
        let file = self.check()?;
        if name == "media.iso" {
            return Ok(Resource {
                file,
                size: self.length,
                extents: vec![(0, self.length)],
                position: 0,
            });
        }
        let resolved = self.resolve(name)?;
        let key = self.find(&resolved).ok_or("Image file not found")?;
        let entry = &self.entries[key];
        if entry.is_dir {
            return Err("Not a file".into());
        }
        Ok(Resource {
            file,
            size: entry.size,
            extents: entry.extents.clone(),
            position: 0,
        })
    }
    fn udf_index(&mut self) -> Result<(), String> {
        struct Source {
            file: std::cell::RefCell<File>,
            length: u64,
        }
        impl libmkiso::ReadAt for Source {
            fn len(&self) -> u64 {
                self.length
            }
            fn read_at(&self, offset: u64, buffer: &mut [u8]) -> std::io::Result<usize> {
                let mut file = self.file.borrow_mut();
                file.seek(SeekFrom::Start(offset))?;
                file.read(buffer)
            }
        }
        let source = Source {
            file: std::cell::RefCell::new(self.check()?),
            length: self.length,
        };
        let limits = libmkiso::UdfLimits {
            max_entries: 100_000,
            max_metadata_bytes: 64 * 1024 * 1024,
            max_entry_bytes: 128 * 1024 * 1024 * 1024,
            max_total_bytes: 512 * 1024 * 1024 * 1024,
            max_input_bytes: self.length,
            max_nesting_depth: 32,
        };
        let io_limits = libmkiso::udf::SourceLimits {
            max_read_bytes: 256 * 1024 * 1024,
            ..Default::default()
        };
        let reader =
            libmkiso::UdfReader::open_source_with_limits(source, limits, io_limits).map_err(err)?;
        let mut indexed = BTreeMap::new();
        let mut extent_count = 0usize;
        for (index, entry) in reader.entries().iter().enumerate() {
            if !matches!(
                entry.kind,
                libmkiso::UdfEntryKind::File | libmkiso::UdfEntryKind::Directory
            ) {
                continue;
            }
            if !valid(&entry.name) {
                return Err("Unsafe UDF filename".into());
            }
            let mut extents = vec![];
            let mut position = 0;
            if !entry.directory {
                reader
                    .visit_extents(index, |logical, physical, length| {
                        extent_count += 1;
                        if extent_count > 100_000
                            || logical != position
                            || length == 0
                            || logical
                                .checked_add(length)
                                .is_none_or(|end| end > entry.size)
                            || physical.is_some_and(|offset| {
                                offset
                                    .checked_add(length)
                                    .is_none_or(|end| end > self.length)
                            })
                        {
                            return Err(libmkiso::UdfError::Malformed(
                                "Invalid or excessive UDF allocations".into(),
                            ));
                        }
                        extents.push((physical.unwrap_or(u64::MAX), length));
                        position += length;
                        Ok(())
                    })
                    .map_err(err)?;
                if position != entry.size {
                    return Err("Incomplete UDF allocation map".into());
                }
            }
            if indexed
                .insert(
                    entry.name.clone(),
                    Entry {
                        size: entry.size,
                        is_dir: entry.directory,
                        mode: 0,
                        link: None,
                        link_root: None,
                        extents,
                    },
                )
                .is_some()
            {
                return Err("Duplicate UDF path".into());
            }
        }
        if indexed.is_empty() {
            return Err("Image contains no readable files".into());
        }
        self.entries = indexed;
        Ok(())
    }
}
pub const UDF_FALLBACK_NOTICE: &str = "UDF is unavailable; using the ISO9660/Joliet bridge filesystem. Check that the installation files are complete.";
pub fn fallback_notice(root: &Path) -> Option<&'static str> {
    registered(root)
        .ok()?
        .fallback_reason
        .as_ref()
        .map(|_| UDF_FALLBACK_NOTICE)
}
pub fn register_ufs(
    source: &Path,
    root: &Path,
    entries: &BTreeMap<String, crate::ufs::Entry>,
    prefix: &str,
) -> Result<(), String> {
    let mut image = Image::open(source)?;
    let prefix = prefix.trim_matches('/');
    image
        .entries
        .retain(|name, _| name != prefix && !name.starts_with(&format!("{prefix}/")));
    for (name, entry) in entries {
        if ![0o100000, 0o40000, 0o120000].contains(&(entry.mode & 0o170000)) {
            continue;
        }
        let path = if name.is_empty() {
            prefix.to_string()
        } else {
            format!("{prefix}/{name}")
        };
        if !valid(&path) {
            return Err("Unsafe UFS path".into());
        }
        image.entries.insert(
            path,
            Entry {
                size: entry.size,
                is_dir: entry.mode & 0o170000 == 0o40000,
                mode: entry.mode,
                link: entry.link.clone(),
                link_root: Some(prefix.into()),
                extents: entry.extents.clone(),
            },
        );
    }
    fs::create_dir_all(root).map_err(err)?;
    fs::write(
        root.join(".bootah-media.json"),
        serde_json::to_vec(&image).map_err(err)?,
    )
    .map_err(err)
}
pub fn register(source: &Path, root: &Path) -> Result<(), String> {
    fs::create_dir_all(root).map_err(err)?;
    let image = Image::open(source)?;
    fs::write(
        root.join(".bootah-media.json"),
        serde_json::to_vec(&image).map_err(err)?,
    )
    .map_err(err)
}
pub fn alias(root: &Path, from: &str, to: &str) -> Result<(), String> {
    if !valid(from) || !valid(to) {
        return Err("Invalid virtual alias".into());
    }
    let mut image = (*registered(root)?).clone();
    if !image.entries.contains_key(to) {
        return Err("Alias target unavailable".into());
    }
    image.aliases.insert(from.into(), to.into());
    fs::write(
        root.join(".bootah-media.json"),
        serde_json::to_vec(&image).map_err(err)?,
    )
    .map_err(err)
}
fn registered(root: &Path) -> Result<std::sync::Arc<Image>, String> {
    use std::sync::{Arc, Mutex, OnceLock};
    type Cached = (u64, std::time::SystemTime, Arc<Image>);
    static CACHE: OnceLock<Mutex<BTreeMap<PathBuf, Cached>>> = OnceLock::new();
    let path = root.join(".bootah-media.json");
    let metadata = fs::symlink_metadata(&path).map_err(err)?;
    if metadata.file_type().is_symlink() || metadata.len() > 64 * 1024 * 1024 {
        return Err("Invalid media manifest".into());
    }
    let modified = metadata.modified().map_err(err)?;
    let cache = CACHE.get_or_init(|| Mutex::new(BTreeMap::new()));
    if let Some((_, _, image)) = cache
        .lock()
        .map_err(err)?
        .get(&path)
        .filter(|(size, time, _)| *size == metadata.len() && *time == modified)
    {
        return Ok(image.clone());
    }
    let mut image: Image = serde_json::from_slice(&fs::read(&path).map_err(err)?).map_err(err)?;
    if image.external || image.version < 3 {
        image.check()?;
        let mut updated = Image::open(&image.source)?;
        updated.aliases = image.aliases;
        updated.entries.extend(
            image
                .entries
                .into_iter()
                .filter(|(_, e)| e.link_root.is_some()),
        );
        image = updated;
        let temporary = root.join(format!(".bootah-media-{}.tmp", uuid::Uuid::new_v4()));
        if let Ok(data) = serde_json::to_vec(&image) {
            if fs::write(&temporary, data).is_ok() {
                let _ = fs::rename(&temporary, &path);
            }
            let _ = fs::remove_file(&temporary);
        }
    }
    let image = Arc::new(image);
    let mut cache = cache.lock().map_err(err)?;
    if cache.len() >= 8 {
        cache.clear()
    }
    cache.insert(path, (metadata.len(), modified, image.clone()));
    Ok(image)
}

pub fn list(root: &Path) -> Result<BTreeMap<String, Entry>, String> {
    let image = registered(root)?;
    image.check()?;
    let mut entries = image.entries.clone();
    fn local(
        root: &Path,
        path: &Path,
        prefix: &str,
        depth: usize,
        out: &mut BTreeMap<String, Entry>,
    ) -> Result<(), String> {
        if depth > 32 || out.len() > 100_000 {
            return Err("Media tree exceeds index limits".into());
        }
        for item in fs::read_dir(path).map_err(err)? {
            let item = item.map_err(err)?;
            let name = item.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            let relative = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            if !valid(&relative) {
                continue;
            }
            let kind = item.file_type().map_err(err)?;
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                out.insert(
                    relative.clone(),
                    Entry {
                        size: 0,
                        is_dir: true,
                        mode: 0o40555,
                        link: None,
                        link_root: None,
                        extents: vec![],
                    },
                );
                local(root, &item.path(), &relative, depth + 1, out)?;
            } else if kind.is_file() && crate::model::safe_file(root, &relative).is_ok() {
                out.insert(
                    relative,
                    Entry {
                        size: item.metadata().map_err(err)?.len(),
                        is_dir: false,
                        mode: 0o100444,
                        link: None,
                        link_root: None,
                        extents: vec![],
                    },
                );
            }
        }
        Ok(())
    }
    local(root, root, "", 0, &mut entries)?;
    Ok(entries)
}
pub fn open(root: &Path, name: &str) -> Result<Resource, String> {
    if !valid(name) {
        return Err("Unsafe file path".into());
    }
    if let Ok(path) = crate::model::safe_file(root, name) {
        return Resource::file(&path);
    }
    if name == "snponly-x64.efi" {
        if let Some(path) = crate::media::asset(name) {
            return Resource::file(&path);
        }
    }
    registered(root)?.resource(name, Some(root))
}
pub fn exists(root: &Path, name: &str) -> bool {
    if crate::model::safe_file(root, name).is_ok() {
        return true;
    }
    (name == "snponly-x64.efi" && crate::media::asset(name).is_some())
        || registered(root).is_ok_and(|i| name == "media.iso" || i.find(name).is_some())
}
pub struct Resource {
    file: File,
    pub size: u64,
    extents: Vec<(u64, u64)>,
    position: u64,
}
impl Resource {
    fn file(path: &Path) -> Result<Self, String> {
        let file = File::open(path).map_err(err)?;
        let size = file.metadata().map_err(err)?.len();
        Ok(Self {
            file,
            size,
            extents: vec![(0, size)],
            position: 0,
        })
    }
}
impl Resource {
    pub fn stream(
        self,
        start: u64,
        length: u64,
    ) -> impl futures_util::Stream<Item = Result<axum::body::Bytes, std::io::Error>> + Send {
        let file = tokio::fs::File::from_std(self.file);
        let mut base = 0;
        let mut ranges = std::collections::VecDeque::new();
        let end = start.saturating_add(length);
        for (offset, len) in self.extents {
            let first = start.max(base);
            let last = end.min(base + len);
            if first < last {
                ranges.push_back((
                    if offset == u64::MAX {
                        offset
                    } else {
                        offset + first - base
                    },
                    last - first,
                ));
            }
            base += len;
        }
        futures_util::stream::try_unfold(
            (file, ranges, length, Some(0u64)),
            |(mut file, mut ranges, mut remaining, mut position)| async move {
                use tokio::io::{AsyncReadExt, AsyncSeekExt};
                let Some((offset, len)) = ranges.pop_front() else {
                    return if remaining == 0 {
                        Ok(None)
                    } else {
                        Err(std::io::Error::new(
                            std::io::ErrorKind::UnexpectedEof,
                            "Incomplete media extent map",
                        ))
                    };
                };
                let size = len.min(128 * 1024) as usize;
                let mut bytes = vec![0; size];
                if offset != u64::MAX {
                    if position != Some(offset) {
                        file.seek(SeekFrom::Start(offset)).await?;
                    }
                    file.read_exact(&mut bytes).await?;
                    position = Some(offset + size as u64);
                }
                if len > size as u64 {
                    ranges.push_front((
                        if offset == u64::MAX {
                            offset
                        } else {
                            offset + size as u64
                        },
                        len - size as u64,
                    ));
                }
                remaining -= size as u64;
                Ok(Some((
                    axum::body::Bytes::from(bytes),
                    (file, ranges, remaining, position),
                )))
            },
        )
    }
}
impl Read for Resource {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        if self.position >= self.size {
            return Ok(0);
        }
        let mut base = 0;
        for &(offset, len) in &self.extents {
            if self.position < base + len {
                let inner = self.position - base;
                let count = (b.len() as u64)
                    .min(len - inner)
                    .min(self.size - self.position) as usize;
                if offset == u64::MAX {
                    b[..count].fill(0);
                    self.position += count as u64;
                    return Ok(count);
                }
                self.file.seek(SeekFrom::Start(offset + inner))?;
                let n = self.file.read(&mut b[..count])?;
                if n == 0 && count > 0 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "Truncated media extent",
                    ));
                }
                self.position += n as u64;
                return Ok(n);
            }
            base += len;
        }
        if b.is_empty() {
            Ok(0)
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "Incomplete media extent map",
            ))
        }
    }
}
impl Seek for Resource {
    fn seek(&mut self, p: SeekFrom) -> std::io::Result<u64> {
        let n = match p {
            SeekFrom::Start(n) => n as i128,
            SeekFrom::Current(n) => self.position as i128 + n as i128,
            SeekFrom::End(n) => self.size as i128 + n as i128,
        };
        if n < 0 || n > u64::MAX as i128 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Invalid seek",
            ));
        }
        self.position = n as u64;
        Ok(self.position)
    }
}

#[cfg(test)]
mod continuation_tests {
    use super::*;
    #[test]
    fn reads_continued_name_and_rejects_cycles() {
        let path = std::env::temp_dir().join(format!("bootah-ce-{}", uuid::Uuid::new_v4()));
        let mut ce = [0u8; 28];
        ce[..4].copy_from_slice(b"CE\x1c\x01");
        ce[4..8].copy_from_slice(&1u32.to_le_bytes());
        ce[20..24].copy_from_slice(&9u32.to_le_bytes());
        let mut contents = vec![0u8; 2048];
        contents.extend_from_slice(b"NM\x09\x01\x00tail");
        fs::write(&path, &contents).unwrap();
        let mut f = File::open(&path).unwrap();
        let mut first = b"NM\x09\x01\x01head".to_vec();
        first.extend_from_slice(&ce);
        assert_eq!(
            rock_ridge_name(&mut f, &first, contents.len() as u64)
                .unwrap()
                .0,
            "headtail"
        );
        assert!(rock_ridge_name(&mut f, b"NM\x09\x01\x01head", contents.len() as u64).is_err());
        contents[2052] = 1;
        fs::write(&path, &contents).unwrap();
        let mut trailing = first.clone();
        trailing.extend_from_slice(b"NM\x09\x01\x00more");
        assert_eq!(
            rock_ridge_name(&mut f, &trailing, contents.len() as u64)
                .unwrap()
                .0,
            "headtailmore"
        );
        ce[20..24].copy_from_slice(&28u32.to_le_bytes());
        contents.truncate(2048);
        contents.extend_from_slice(&ce);
        fs::write(&path, &contents).unwrap();
        assert!(rock_ridge_name(&mut f, &ce, contents.len() as u64).is_err());
        fs::remove_file(path).unwrap();
    }
}

#[cfg(test)]
mod optical_tests {
    use super::*;
    fn record(name: &[u8], sector: u32, size: u32, flags: u8, extra: &[u8]) -> Vec<u8> {
        let mut r = vec![0; 33 + name.len() + usize::from(name.len().is_multiple_of(2))];
        r[2..6].copy_from_slice(&sector.to_le_bytes());
        r[6..10].copy_from_slice(&sector.to_be_bytes());
        r[10..14].copy_from_slice(&size.to_le_bytes());
        r[14..18].copy_from_slice(&size.to_be_bytes());
        r[25] = flags;
        r[28..30].copy_from_slice(&1u16.to_le_bytes());
        r[30..32].copy_from_slice(&1u16.to_be_bytes());
        r[32] = name.len() as u8;
        r[33..33 + name.len()].copy_from_slice(name);
        r.extend_from_slice(extra);
        r[0] = r.len() as u8;
        r
    }
    fn medium(joliet: bool, skip: usize) -> (PathBuf, String, String) {
        let path =
            std::env::temp_dir().join(format!("bootah-optical-{}.iso", uuid::Uuid::new_v4()));
        let mut bytes = vec![0; 32 * 2048];
        bytes[16 * 2048] = 1;
        bytes[16 * 2048 + 1..16 * 2048 + 6].copy_from_slice(b"CD001");
        bytes[16 * 2048 + 6] = 1;
        let root = record(&[0], 20, 2048, 2, &[]);
        bytes[16 * 2048 + 156..16 * 2048 + 190].copy_from_slice(&root[..34]);
        let (a, b, dir) = if joliet {
            let descriptor = bytes[16 * 2048..17 * 2048].to_vec();
            bytes[17 * 2048..18 * 2048].copy_from_slice(&descriptor);
            bytes[17 * 2048] = 2;
            bytes[17 * 2048 + 88..17 * 2048 + 91].copy_from_slice(b"%/E");
            bytes[18 * 2048] = 255;
            bytes[18 * 2048 + 1..18 * 2048 + 6].copy_from_slice(b"CD001");
            let a = "完整中文文件名-é.txt".to_string();
            let b = "非连续多段文件.bin".to_string();
            let encode = |s: &str| {
                s.encode_utf16()
                    .flat_map(u16::to_be_bytes)
                    .collect::<Vec<_>>()
            };
            let mut dir = root.clone();
            dir.extend(record(&encode(&a), 23, 5, 0, &[]));
            dir.extend(record(&encode(&b), 24, 3, 0x80, &[]));
            dir.extend(record(&encode(&b), 26, 4, 0, &[]));
            (a, b, dir)
        } else {
            bytes[17 * 2048] = 255;
            bytes[17 * 2048 + 1..17 * 2048 + 6].copy_from_slice(b"CD001");
            let a = format!("{}-complete-primary.xml.gz", "long-name-".repeat(16));
            let b = format!("{}-different-primary.xml.gz", "long-name-".repeat(16));
            let mut sp = b"SP\x07\x01\xbe\xef\x00".to_vec();
            sp[6] = skip as u8;
            let mut dir = record(&[0], 20, 2048, 2, &sp);
            for (i, name) in [&a, &b].iter().enumerate() {
                let mut nm = b"NM\x55\x01\x01".to_vec();
                nm.extend_from_slice(&name.as_bytes()[..80]);
                let rest = &name.as_bytes()[80..];
                let mut ce = [0u8; 28];
                ce[..4].copy_from_slice(b"CE\x1c\x01");
                ce[4..8].copy_from_slice(&(27 + i as u32).to_le_bytes());
                ce[20..24].copy_from_slice(&((5 + rest.len()) as u32).to_le_bytes());
                nm.extend_from_slice(&ce);
                let mut extra = vec![0; skip];
                extra.extend_from_slice(&nm);
                let offset = (27 + i) * 2048;
                bytes[offset..offset + 5].copy_from_slice(&[
                    b'N',
                    b'M',
                    (5 + rest.len()) as u8,
                    1,
                    0,
                ]);
                bytes[offset + 5..offset + 5 + rest.len()].copy_from_slice(rest);
                dir.extend(record(
                    if i == 0 { b"SHORTA;1" } else { b"SHORTB;1" },
                    23 + i as u32,
                    5,
                    0,
                    &extra,
                ));
            }
            (a, b, dir)
        };
        bytes[20 * 2048..20 * 2048 + dir.len()].copy_from_slice(&dir);
        bytes[23 * 2048..23 * 2048 + 5].copy_from_slice(b"hello");
        bytes[24 * 2048..24 * 2048 + 5].copy_from_slice(b"world");
        bytes[26 * 2048..26 * 2048 + 4].copy_from_slice(b"test");
        fs::write(&path, bytes).unwrap();
        (path, a, b)
    }
    #[test]
    fn unsupported_udf_falls_back_to_complete_iso_namespace() {
        for joliet in [false, true] {
            let (path, name, _) = medium(joliet, 0);
            let bridge = fs::read(&path).unwrap();
            let mut bytes = vec![];
            flate2::read::GzDecoder::new(
                include_bytes!("../tests/fixtures/udf/metadata250.udf.gz").as_slice(),
            )
            .read_to_end(&mut bytes)
            .unwrap();
            bytes[16 * 2048..32 * 2048].copy_from_slice(&bridge[16 * 2048..32 * 2048]);
            bytes[31 * 2048 + 1..31 * 2048 + 6].copy_from_slice(b"NSR03");
            let mut changed = 0;
            for descriptor in bytes.chunks_exact_mut(2048) {
                if descriptor[..2] != 6u16.to_le_bytes() {
                    continue;
                }
                descriptor[440] = 255;
                let length = u16::from_le_bytes(descriptor[10..12].try_into().unwrap()) as usize;
                let mut crc = 0u16;
                for byte in &descriptor[16..16 + length] {
                    crc ^= (*byte as u16) << 8;
                    for _ in 0..8 {
                        crc = if crc & 0x8000 != 0 {
                            (crc << 1) ^ 0x1021
                        } else {
                            crc << 1
                        };
                    }
                }
                descriptor[8..10].copy_from_slice(&crc.to_le_bytes());
                descriptor[4] = descriptor[..16]
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| *i != 4)
                    .fold(0u8, |sum, (_, byte)| sum.wrapping_add(*byte));
                changed += 1;
            }
            assert!(changed >= 2);
            fs::write(&path, &bytes).unwrap();
            let image = Image::open(&path).unwrap();
            assert!(
                image
                    .fallback_reason
                    .as_ref()
                    .unwrap()
                    .contains("unsupported UDF")
            );
            assert_eq!(
                image.filesystem,
                if joliet { "joliet" } else { "rock-ridge" }
            );
            assert_eq!(image.read(&name, 100).unwrap(), b"hello");
            bytes[16 * 2048] = 0;
            fs::write(&path, &bytes).unwrap();
            let error = Image::open(&path).err().unwrap();
            assert!(
                error.contains("Cannot decode UDF filesystem")
                    && error.contains("Cannot decode ISO bridge filesystem")
            );
            fs::remove_file(path).unwrap();
        }
    }
    #[test]
    fn virtual_sparable_and_metadata_partitions_preserve_names_and_ranges() {
        let name = format!("目录/{}-complete.bin", "long-name-".repeat(20));
        let expected: Vec<u8> = (0..20_003).map(|n| (n % 251) as u8).collect();
        for (kind, compressed) in [
            (
                "vat150",
                include_bytes!("../tests/fixtures/udf/vat150.udf.gz").as_slice(),
            ),
            (
                "vat201",
                include_bytes!("../tests/fixtures/udf/vat201.udf.gz").as_slice(),
            ),
            (
                "sparable",
                include_bytes!("../tests/fixtures/udf/sparable.udf.gz").as_slice(),
            ),
            (
                "metadata250",
                include_bytes!("../tests/fixtures/udf/metadata250.udf.gz").as_slice(),
            ),
            (
                "metadata260",
                include_bytes!("../tests/fixtures/udf/metadata260.udf.gz").as_slice(),
            ),
        ] {
            let path =
                std::env::temp_dir().join(format!("bootah-{kind}-{}.udf", uuid::Uuid::new_v4()));
            let mut bytes = vec![];
            flate2::read::GzDecoder::new(compressed)
                .read_to_end(&mut bytes)
                .unwrap();
            fs::write(&path, bytes).unwrap();
            let image = Image::open(&path).unwrap_or_else(|error| panic!("{kind}: {error}"));
            assert_eq!(image.filesystem, "udf");
            assert!(image.fallback_reason.is_none());
            assert_eq!(image.read(&name, 21_000).unwrap(), expected, "{kind}");
            assert_eq!(image.read("目录/中文.txt", 100).unwrap(), b"unicode");
            assert!(image.read("empty", 0).unwrap().is_empty());
            let mut resource = image.resource(&name, None).unwrap();
            resource.seek(SeekFrom::Start(3700)).unwrap();
            let mut range = vec![0; 9500];
            resource.read_exact(&mut range).unwrap();
            assert_eq!(range, expected[3700..13200], "{kind}");
            fs::remove_file(path).unwrap();
        }
    }
    #[tokio::test]
    async fn complete_names_joliet_and_noncontiguous_ranges() {
        use futures_util::TryStreamExt;
        for (joliet, skip) in [(false, 0), (false, 3), (true, 0)] {
            let (path, a, b) = medium(joliet, skip);
            let image = Image::open(&path).unwrap();
            assert!(image.entries.contains_key(&a));
            assert!(image.entries.contains_key(&b));
            assert_eq!(image.entries.len(), 2);
            let mut file = image.resource(&a, None).unwrap();
            let mut data = vec![];
            file.read_to_end(&mut data).unwrap();
            assert_eq!(data, b"hello");
            if joliet {
                let stream = image.resource(&b, None).unwrap().stream(1, 5);
                let data = stream
                    .try_fold(Vec::new(), |mut v, b| async move {
                        v.extend_from_slice(&b);
                        Ok(v)
                    })
                    .await
                    .unwrap();
                assert_eq!(data, b"ortes");
            }
            fs::remove_file(path).unwrap();
        }
        for path in [
            "../secret",
            "a/../b",
            "a\\b",
            "C:secret",
            ".bootah-media.json",
            "a/\0b",
        ] {
            assert!(sanitize(path).is_err());
        }
        assert_eq!(
            sanitize("repodata//primary.xml.gz").unwrap(),
            "repodata/primary.xml.gz"
        );
    }
}
