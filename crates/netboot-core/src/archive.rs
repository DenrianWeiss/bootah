use std::{
    fs::{self, File},
    io::{self, BufReader, Read, Seek},
    path::{Path, PathBuf},
};
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

pub fn destination(root: &Path, name: &str) -> Result<PathBuf, String> {
    let mut name = name;
    while let Some(rest) = name.strip_prefix("./") {
        name = rest;
    }
    if name.is_empty()
        || name.contains(['\\', ':', '\0'])
        || name.chars().any(char::is_control)
        || name.split('/').count() > 32
        || name
            .split('/')
            .any(|p| p.is_empty() || p == "." || p == ".." || p.starts_with(".bootah-"))
    {
        return Err("Unsafe archive path".into());
    }
    let root = root.canonicalize().map_err(err)?;
    let mut out = root.clone();
    for part in name.split('/') {
        out.push(part);
        match fs::symlink_metadata(&out) {
            Ok(m) if m.file_type().is_symlink() => {
                return Err("Archive path contains a link".into());
            }
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(err(e)),
        }
    }
    Ok(out)
}
fn write(
    root: &Path,
    name: &str,
    input: &mut impl Read,
    size: u64,
    budget: &mut u64,
) -> Result<(), String> {
    *budget = budget
        .checked_sub(size)
        .ok_or("Archive exceeds size budget")?;
    let target = destination(root, name)?;
    fs::create_dir_all(target.parent().ok_or("Invalid archive path")?).map_err(err)?;
    let temporary = target.with_file_name(format!(".bootah-{}.part", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(err)?;
        let copied = io::copy(&mut input.take(size), &mut output).map_err(err)?;
        if copied != size {
            return Err("Truncated archive component".into());
        }
        drop(output);
        fs::rename(&temporary, &target).map_err(err)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}
fn set_mode(root: &Path, name: &str, mode: u32) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            destination(root, name)?,
            fs::Permissions::from_mode(mode & 0o777),
        )
        .map_err(err)?;
    }
    #[cfg(not(unix))]
    let _ = (root, name, mode);
    Ok(())
}
pub fn decoded(path: &Path) -> Result<Box<dyn Read>, String> {
    let mut file = File::open(path).map_err(err)?;
    let mut magic = [0; 6];
    let n = file.read(&mut magic).map_err(err)?;
    file.rewind().map_err(err)?;
    if magic[..2] == [0x1f, 0x8b] {
        Ok(Box::new(flate2::read::MultiGzDecoder::new(file)))
    } else if magic == [0xfd, b'7', b'z', b'X', b'Z', 0] {
        Ok(Box::new(lzma_rust2::XzReader::new_mem_limit(
            file,
            true,
            256 * 1024,
        )))
    } else if &magic[..3] == b"BZh" {
        Ok(Box::new(bzip2::read::MultiBzDecoder::new(file)))
    } else if magic[..4] == [0x28, 0xb5, 0x2f, 0xfd] {
        Ok(Box::new(
            ruzstd::decoding::StreamingDecoder::new_with_max_window_size(file, 128 * 1024 * 1024)
                .map_err(err)?,
        ))
    } else if magic[..4] == [4, 0x22, 0x4d, 0x18] {
        Ok(Box::new(lz4_flex::frame::FrameDecoder::new(file)))
    } else if magic[..4] == [2, 0x21, 0x4c, 0x18] {
        Ok(Box::new(LegacyLz4 {
            input: file,
            block: io::Cursor::new(vec![]),
            done: false,
        }))
    } else if n >= 3 && magic[0] == 0x5d {
        Ok(Box::new(
            lzma_rust2::LzmaReader::new_mem_limit(file, 256 * 1024, None).map_err(err)?,
        ))
    } else {
        Ok(Box::new(file))
    }
}
struct LegacyLz4 {
    input: File,
    block: io::Cursor<Vec<u8>>,
    done: bool,
}
impl Read for LegacyLz4 {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        loop {
            let n = self.block.read(out)?;
            if n > 0 || self.done {
                return Ok(n);
            }
            let mut len = [0; 4];
            let n = self.input.read(&mut len[..1])?;
            if n == 0 {
                self.done = true;
                return Ok(0);
            }
            self.input.read_exact(&mut len[1..])?;
            let count = u32::from_le_bytes(len) as usize;
            if count == 0 || count > 9 * 1024 * 1024 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Invalid LZ4 block",
                ));
            }
            let mut compressed = vec![0; count];
            self.input.read_exact(&mut compressed)?;
            self.block = io::Cursor::new(
                lz4_flex::block::decompress(&compressed, 8 * 1024 * 1024)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?,
            );
        }
    }
}
pub fn cab_file(path: &Path, wanted: Option<&str>, limit: u64) -> Result<Vec<u8>, String> {
    let mut cab = cab::Cabinet::new(File::open(path).map_err(err)?).map_err(err)?;
    let names: Vec<_> = cab
        .folder_entries()
        .flat_map(|f| f.file_entries())
        .map(|e| (e.name().to_string(), e.uncompressed_size() as u64))
        .collect();
    let selected = match wanted {
        Some(w) => names.iter().find(|(n, _)| n.eq_ignore_ascii_case(w)),
        None if names.len() == 1 => names.first(),
        _ => None,
    }
    .ok_or("CAB component not found or ambiguous")?;
    if selected.1 > limit {
        return Err("CAB component exceeds size budget".into());
    }
    let mut bytes = Vec::new();
    cab.read_file(&selected.0)
        .map_err(err)?
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(err)?;
    if bytes.len() as u64 != selected.1 {
        return Err("Truncated CAB component".into());
    }
    Ok(bytes)
}
pub fn extract(path: &Path, root: &Path, ratio: u64) -> Result<(), String> {
    fs::create_dir_all(root).map_err(err)?;
    let mut budget = fs::metadata(path)
        .map_err(err)?
        .len()
        .saturating_mul(ratio)
        .min(if ratio > 2 {
            512 * 1024 * 1024
        } else {
            128 * 1024 * 1024 * 1024
        });
    let mut file = File::open(path).map_err(err)?;
    let mut magic = [0; 8];
    file.read_exact(&mut magic).map_err(err)?;
    file.rewind().map_err(err)?;
    if &magic == b"!<arch>\n" {
        let mut archive = ar::Archive::new(file);
        let mut count = 0;
        while let Some(entry) = archive.next_entry() {
            let mut entry = entry.map_err(err)?;
            count += 1;
            if count > 100_000 {
                return Err("Too many archive files".into());
            }
            let name = String::from_utf8(entry.header().identifier().to_vec()).map_err(err)?;
            let size = entry.header().size();
            write(root, &name, &mut entry, size, &mut budget)?;
        }
    } else if &magic[..4] == b"MSCF" {
        let mut cab = cab::Cabinet::new(file).map_err(err)?;
        let entries: Vec<_> = cab
            .folder_entries()
            .flat_map(|f| f.file_entries())
            .map(|e| {
                (
                    e.name().replace('\\', "/"),
                    e.name().to_string(),
                    e.uncompressed_size() as u64,
                )
            })
            .collect();
        if entries.len() > 100_000 {
            return Err("Too many CAB files".into());
        }
        for (name, key, size) in entries {
            write(
                root,
                &name,
                &mut cab.read_file(&key).map_err(err)?,
                size,
                &mut budget,
            )?;
        }
    } else if &magic[..4] == b"hsqs" {
        let squash = backhand::FilesystemReader::from_reader_with_offset_and_kind(
            BufReader::new(file),
            0,
            backhand::kind::Kind::new_v4(&SQUASH_DECODER),
        )
        .map_err(err)?;
        for (count, node) in squash.files().enumerate() {
            if count > 100_000 {
                return Err("Too many SquashFS files".into());
            }
            let name = node
                .fullpath
                .to_str()
                .ok_or("Invalid SquashFS filename")?
                .trim_start_matches('/');
            if name.is_empty() {
                continue;
            }
            match &node.inner {
                backhand::InnerNode::File(f) => {
                    write(
                        root,
                        name,
                        &mut squash.file(f).reader(),
                        f.file_len() as u64,
                        &mut budget,
                    )?;
                    set_mode(root, name, node.header.permissions as u32)?;
                }
                backhand::InnerNode::Dir(_) => {
                    fs::create_dir_all(destination(root, name)?).map_err(err)?;
                }
                _ => {}
            }
        }
    } else if {
        let mut sector = [0u8; 512];
        file.read_exact(&mut sector).is_ok()
            && (&sector[54..57] == b"FAT" || &sector[82..85] == b"FAT")
    } {
        file.rewind().map_err(err)?;
        let fat = fatfs::FileSystem::new(file, fatfs::FsOptions::new()).map_err(err)?;
        let mut count = 0;
        extract_fat(fat.root_dir(), root, "", 0, &mut count, &mut budget)?;
    } else {
        let mut stream = BufReader::new(decoded(path)?);
        use io::BufRead;
        if stream.fill_buf().map_err(err)?.starts_with(b"07070") {
            extract_cpio(&mut stream, root, &mut budget)?;
        } else {
            let mut archive = tar::Archive::new(stream);
            let mut count = 0;
            for entry in archive.entries().map_err(err)? {
                let mut entry = entry.map_err(err)?;
                count += 1;
                if count > 100_000 {
                    return Err("Too many archive files".into());
                }
                let name = entry
                    .path()
                    .map_err(err)?
                    .to_str()
                    .ok_or("Invalid archive filename")?
                    .trim_end_matches('/')
                    .to_string();
                if name == "." || name.is_empty() {
                    continue;
                }
                if entry.header().entry_type().is_file() {
                    let size = entry.size();
                    let mode = entry.header().mode().map_err(err)?;
                    write(root, &name, &mut entry, size, &mut budget)?;
                    set_mode(root, &name, mode)?;
                } else if entry.header().entry_type().is_dir() {
                    fs::create_dir_all(destination(root, &name)?).map_err(err)?;
                }
            }
        }
    }
    Ok(())
}
fn extract_cpio(input: &mut impl Read, root: &Path, budget: &mut u64) -> Result<(), String> {
    for _ in 0..100_000 {
        let mut h = [0; 110];
        input.read_exact(&mut h).map_err(err)?;
        if !matches!(&h[..6], b"070701" | b"070702") {
            return Err("Unsupported CPIO archive".into());
        }
        let hex = |a, b| {
            u64::from_str_radix(std::str::from_utf8(&h[a..b]).map_err(err)?, 16).map_err(err)
        };
        let size = hex(54, 62)?;
        let len = hex(94, 102)?;
        let mode = hex(14, 22)?;
        if len == 0 || len > 8192 {
            return Err("Invalid CPIO name".into());
        }
        let mut name = vec![0; len as usize];
        input.read_exact(&mut name).map_err(err)?;
        if name.pop() != Some(0) {
            return Err("Invalid CPIO name".into());
        }
        let name = String::from_utf8(name).map_err(err)?;
        io::copy(&mut input.take((4 - (110 + len) % 4) % 4), &mut io::sink()).map_err(err)?;
        if name == "TRAILER!!!" {
            return Ok(());
        }
        if mode & 0o170000 == 0o100000 {
            write(root, &name, input, size, budget)?;
            set_mode(root, &name, mode as u32)?;
        } else {
            if mode & 0o170000 == 0o40000 && name != "." {
                fs::create_dir_all(destination(root, &name)?).map_err(err)?;
            }
            *budget = budget
                .checked_sub(size)
                .ok_or("Archive exceeds size budget")?;
            if io::copy(&mut input.take(size), &mut io::sink()).map_err(err)? != size {
                return Err("Truncated CPIO".into());
            }
        }
        io::copy(&mut input.take((4 - size % 4) % 4), &mut io::sink()).map_err(err)?;
    }
    Err("Too many CPIO entries".into())
}

fn extract_fat(
    dir: fatfs::Dir<'_, File>,
    root: &Path,
    parent: &str,
    depth: usize,
    count: &mut usize,
    budget: &mut u64,
) -> Result<(), String> {
    if depth > 32 {
        return Err("FAT directory too deep".into());
    }
    for item in dir.iter() {
        let entry = item.map_err(err)?;
        let name = entry.file_name();
        if name == "." || name == ".." {
            continue;
        }
        *count += 1;
        if *count > 100_000 {
            return Err("Too many FAT entries".into());
        }
        let name = if parent.is_empty() {
            name
        } else {
            format!("{parent}/{name}")
        };
        if entry.is_dir() {
            fs::create_dir_all(destination(root, &name)?).map_err(err)?;
            extract_fat(entry.to_dir(), root, &name, depth + 1, count, budget)?;
        } else if entry.is_file() {
            write(root, &name, &mut entry.to_file(), entry.len(), budget)?;
        }
    }
    Ok(())
}

struct SquashDecoder;
static SQUASH_DECODER: SquashDecoder = SquashDecoder;
impl backhand::compression::CompressionAction for SquashDecoder {
    type Compressor = backhand::compression::Compressor;
    type FilesystemCompressor = backhand::FilesystemCompressor;
    type SuperBlock = backhand::SuperBlock;
    type Error = backhand::BackhandError;
    fn decompress(
        &self,
        bytes: &[u8],
        out: &mut Vec<u8>,
        kind: Self::Compressor,
    ) -> Result<(), Self::Error> {
        use backhand::compression::Compressor;
        let limit = out.capacity().max(8192).min(1024 * 1024) as u64;
        let mut decoded: Box<dyn Read> = match kind {
            Compressor::Uncompressed => Box::new(bytes),
            Compressor::Gzip => Box::new(flate2::read::ZlibDecoder::new(bytes)),
            Compressor::Xz => Box::new(lzma_rust2::XzReader::new_mem_limit(
                bytes,
                false,
                256 * 1024,
            )),
            Compressor::Zstd => Box::new(
                ruzstd::decoding::StreamingDecoder::new_with_max_window_size(
                    bytes,
                    128 * 1024 * 1024,
                )
                .map_err(|_| backhand::BackhandError::CorruptedOrInvalidSquashfs)?,
            ),
            Compressor::Lz4 => {
                out.resize(limit as usize, 0);
                let n = lz4_flex::block::decompress_into(bytes, out)
                    .map_err(|_| backhand::BackhandError::CorruptedOrInvalidSquashfs)?;
                out.truncate(n);
                return Ok(());
            }
            _ => return Err(backhand::BackhandError::CorruptedOrInvalidSquashfs),
        };
        out.clear();
        decoded.by_ref().take(limit + 1).read_to_end(out)?;
        if out.len() as u64 > limit {
            return Err(backhand::BackhandError::CorruptedOrInvalidSquashfs);
        }
        Ok(())
    }
    fn compress(
        &self,
        _: &[u8],
        _: Self::FilesystemCompressor,
        _: u32,
    ) -> Result<Vec<u8>, Self::Error> {
        Err(backhand::BackhandError::CorruptedOrInvalidSquashfs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_archives_preserve_full_paths_and_reject_traversal() {
        let root = std::env::temp_dir().join(format!("bootah-archive-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("out")).unwrap();
        let path = root.join("source.tar.gz");
        let name = format!(
            "{}/{}-startup.sh",
            "directory".repeat(10),
            "long-name".repeat(18)
        );
        let gzip = flate2::write::GzEncoder::new(
            File::create(&path).unwrap(),
            flate2::Compression::fast(),
        );
        let mut archive = tar::Builder::new(gzip);
        let mut header = tar::Header::new_gnu();
        header.set_size(5);
        header.set_mode(0o755);
        header.set_cksum();
        archive
            .append_data(&mut header, &name, &b"hello"[..])
            .unwrap();
        archive.into_inner().unwrap().finish().unwrap();
        extract(&path, &root.join("out"), 32).unwrap();
        assert_eq!(fs::read(root.join("out").join(&name)).unwrap(), b"hello");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(root.join("out").join(&name))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o755
            );
            std::os::unix::fs::symlink(&root, root.join("out/link")).unwrap();
            assert!(destination(&root.join("out"), "link/escape").is_err());
        }
        for name in [
            "../escape",
            "/absolute",
            "C:stream",
            "a\\escape",
            ".bootah-media.json",
        ] {
            assert!(destination(&root.join("out"), name).is_err());
        }
        assert!(extract(&path, &root.join("out"), 0).is_err());
        let cab_path = root.join("source.cab");
        let mut builder = cab::CabinetBuilder::new();
        builder
            .add_folder(cab::CompressionType::MsZip)
            .add_file("driver.sys");
        let mut cabinet = builder.build(File::create(&cab_path).unwrap()).unwrap();
        while let Some(mut file) = cabinet.next_file().unwrap() {
            std::io::Write::write_all(&mut file, b"cab payload").unwrap();
        }
        cabinet.finish().unwrap();
        assert_eq!(
            cab_file(&cab_path, Some("DRIVER.SYS"), 64).unwrap(),
            b"cab payload"
        );
        assert!(cab_file(&cab_path, None, 2).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
