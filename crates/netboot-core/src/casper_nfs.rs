use std::{
    fs,
    io::{BufReader, Read, Seek, SeekFrom},
    path::Path,
};

#[derive(Default)]
struct Capabilities {
    script: bool,
    mount: bool,
    modules: std::collections::BTreeMap<String, u8>,
    total: u64,
}

pub fn supported(original: &Path, cache: &Path) -> Result<bool, String> {
    let mut input = BufReader::new(fs::File::open(original).map_err(|e| e.to_string())?);
    let mut capabilities = Capabilities::default();
    for _ in 0..64 {
        let mut byte = [0];
        while input.read(&mut byte).map_err(|e| e.to_string())? == 1 && byte[0] == 0 {}
        let offset = input
            .stream_position()
            .map_err(|e| e.to_string())?
            .saturating_sub(1);
        input
            .seek(SeekFrom::Start(offset))
            .map_err(|e| e.to_string())?;
        let mut magic = [0; 6];
        if input.read_exact(&mut magic).is_err() {
            break;
        }
        input
            .seek(SeekFrom::Start(offset))
            .map_err(|e| e.to_string())?;
        if matches!(&magic, b"070701" | b"070702") {
            if !inspect(&mut input, &mut capabilities)? {
                return Ok(false);
            }
            continue;
        }
        if magic[..4] != [0x28, 0xb5, 0x2f, 0xfd] && magic[..3] != [0x1f, 0x8b, 8] {
            return Ok(false);
        }
        let compressed = cache.join("nfs-inspect.archive");
        std::io::copy(
            &mut input,
            &mut fs::File::create(&compressed).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        let result = inspect(
            &mut crate::archive::decoded(&compressed)?,
            &mut capabilities,
        );
        let _ = fs::remove_file(compressed);
        if !result? {
            return Ok(false);
        }
        break;
    }
    Ok(
        capabilities.script
            && capabilities.mount
            && capabilities.modules.values().any(|v| *v == 15),
    )
}

fn inspect(input: &mut impl Read, caps: &mut Capabilities) -> Result<bool, String> {
    for _ in 0..100_000 {
        let mut h = [0; 110];
        input.read_exact(&mut h).map_err(|e| e.to_string())?;
        if !matches!(&h[..6], b"070701" | b"070702") {
            return Ok(false);
        }
        let field = |a, b| {
            u64::from_str_radix(std::str::from_utf8(&h[a..b]).unwrap_or(""), 16)
                .map_err(|e| e.to_string())
        };
        let size = field(54, 62)?;
        let n = field(94, 102)?;
        caps.total = caps
            .total
            .checked_add(size)
            .ok_or("Casper archive overflow")?;
        if n == 0 || n > 8192 || caps.total > 2 * 1024 * 1024 * 1024 {
            return Ok(false);
        }
        let mut name = vec![0; n as usize];
        input.read_exact(&mut name).map_err(|e| e.to_string())?;
        let name = std::str::from_utf8(&name[..name.len() - 1])
            .map_err(|e| e.to_string())?
            .trim_start_matches("./");
        if name == "TRAILER!!!" {
            std::io::copy(
                &mut input.take((4 - (110 + n) % 4) % 4),
                &mut std::io::sink(),
            )
            .map_err(|e| e.to_string())?;
            return Ok(true);
        }
        std::io::copy(
            &mut input.take((4 - (110 + n) % 4) % 4),
            &mut std::io::sink(),
        )
        .map_err(|e| e.to_string())?;
        caps.mount |= matches!(
            name,
            "usr/bin/nfsmount" | "bin/nfsmount" | "sbin/nfsmount" | "usr/sbin/nfsmount"
        ) && field(14, 22)? & 0o111 != 0;
        if let Some((version, path)) = name
            .strip_prefix("usr/lib/modules/")
            .or_else(|| name.strip_prefix("lib/modules/"))
            .and_then(|s| s.split_once('/'))
        {
            let path = path
                .strip_suffix(".zst")
                .or_else(|| path.strip_suffix(".xz"))
                .or_else(|| path.strip_suffix(".gz"))
                .unwrap_or(path);
            let bit = match path {
                "kernel/fs/nfs/nfs.ko" => 1,
                "kernel/fs/nfs/nfsv3.ko" => 2,
                "kernel/fs/lockd/lockd.ko" => 4,
                "kernel/net/sunrpc/sunrpc.ko" => 8,
                _ => 0,
            };
            *caps.modules.entry(version.into()).or_default() |= bit;
        }
        if name == "scripts/casper" {
            let mode = field(14, 22)?;
            caps.script = size > 0 && mode & 0o170000 == 0o100000 && mode & 0o444 != 0;
        }
        if std::io::copy(&mut input.take(size), &mut std::io::sink()).map_err(|e| e.to_string())?
            != size
        {
            return Ok(false);
        }
        std::io::copy(&mut input.take((4 - size % 4) % 4), &mut std::io::sink())
            .map_err(|e| e.to_string())?;
    }
    Ok(false)
}

pub fn arguments(arguments: &str) -> Option<String> {
    if !arguments.split_whitespace().any(|s| s == "boot=casper")
        || !arguments
            .split_whitespace()
            .any(|s| s == "url={base}/media.iso")
        || arguments.split_whitespace().any(|s| {
            s == "toram"
                || s.starts_with("toram=")
                || s.starts_with("nfsroot=")
                || s.starts_with("netboot=")
                || s.starts_with("nfsopts=")
                || s.starts_with("NFSOPTS=")
        })
    {
        return None;
    }
    Some(arguments.replace(
        "url={base}/media.iso",
        "netboot=nfs nfsroot={server}:/{profile} NFSOPTS=vers=3,tcp,nolock,port={nfs_port}",
    ))
}

#[cfg(test)]
mod tests {
    #[test]
    fn inspects_archive_boundaries_and_requires_matching_modules() {
        use std::io::Write;
        let root = std::env::temp_dir().join(format!("bootah-casper-nfs-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let entry = |out: &mut Vec<u8>, name: &str, data: &[u8]| {
            let mode = if name == "scripts/casper" { 0o100644 } else { 0o100755 };
            crate::initramfs::entry(out, name, mode, data.len() as u32, &mut &data[..])
                .unwrap();
        };
        for matching in [false, true] {
            let mut early = Vec::new();
            // A module may itself contain compression magic; it is not an archive boundary.
            for path in [
                "fs/nfs/nfs",
                "fs/nfs/nfsv3",
                "fs/lockd/lockd",
                "net/sunrpc/sunrpc",
            ] {
                let version = if !matching && path.ends_with("sunrpc") {
                    "other"
                } else {
                    "test"
                };
                entry(
                    &mut early,
                    &format!("usr/lib/modules/{version}/kernel/{path}.ko.zst"),
                    &[0x28, 0xb5, 0x2f, 0xfd],
                );
            }
            entry(&mut early, "TRAILER!!!", b"");
            early.extend([0; 16]);
            let mut payload = Vec::new();
            entry(
                &mut payload,
                "scripts/casper",
                b"arbitrary implementation of the Casper boot protocol",
            );
            entry(&mut payload, "usr/bin/nfsmount", b"mount");
            entry(&mut payload, "TRAILER!!!", b"");
            let mut gzip = flate2::write::GzEncoder::new(early, flate2::Compression::fast());
            gzip.write_all(&payload).unwrap();
            let original = root.join("initrd");
            std::fs::write(&original, gzip.finish().unwrap()).unwrap();
            assert_eq!(super::supported(&original, &root).unwrap(), matching);
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn only_replaces_our_casper_http_source() {
        let args = super::arguments("boot=casper live url={base}/media.iso --- quiet").unwrap();
        assert!(args.contains("netboot=nfs nfsroot={server}:/{profile}"));
        assert!(args.contains("NFSOPTS=vers=3,tcp"));
        assert!(args.ends_with("--- quiet"));
        for args in [
            "boot=casper url=https://example.test/custom.iso",
            "boot=live fetch={base}/live/filesystem.squashfs",
            "install={base}",
            "inst.stage2={base}/images/install.img",
            "boot=casper toram url={base}/media.iso",
            "boot=casper toram=filesystem.squashfs url={base}/media.iso",
            "url={base}/media.iso NFSOPTS=vers=4",
        ] {
            assert!(super::arguments(args).is_none(), "{args}");
        }
    }
}
