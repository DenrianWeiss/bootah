use flate2::{Compression, write::GzEncoder};
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::Path,
};
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
pub(crate) fn entry(
    out: &mut impl Write,
    name: &str,
    mode: u32,
    size: u32,
    input: &mut impl Read,
) -> Result<(), String> {
    write!(out, "070701").map_err(err)?;
    for value in [
        0,
        mode,
        0,
        0,
        1,
        0,
        size,
        0,
        0,
        0,
        0,
        name.len() as u32 + 1,
        0,
    ] {
        write!(out, "{value:08x}").map_err(err)?;
    }
    out.write_all(name.as_bytes()).map_err(err)?;
    out.write_all(&[0]).map_err(err)?;
    out.write_all(&[0; 3][..(4 - (110 + name.len() + 1) % 4) % 4])
        .map_err(err)?;
    let copied = std::io::copy(&mut input.take(size as u64), out).map_err(err)?;
    if copied != size as u64 {
        return Err("Truncated initramfs input".into());
    }
    out.write_all(&[0; 3][..(4 - size as usize % 4) % 4])
        .map_err(err)
}
fn tree(out: &mut impl Write, root: &Path, name: &str) -> Result<(), String> {
    entry(out, name, 0o040755, 0, &mut std::io::empty())?;
    let mut entries = fs::read_dir(root)
        .map_err(err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(err)?;
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let path = e.path();
        let name = format!("{name}/{}", e.file_name().to_string_lossy());
        let kind = e.file_type().map_err(err)?;
        if kind.is_dir() {
            tree(out, &path, &name)?;
        } else if kind.is_file() {
            let size = u32::try_from(e.metadata().map_err(err)?.len()).map_err(err)?;
            entry(
                out,
                &name,
                0o100644,
                size,
                &mut File::open(path).map_err(err)?,
            )?;
        } else {
            return Err("Unsupported modloop entry".into());
        }
    }
    Ok(())
}
pub fn alpine(original: &Path, modloop: &Path, output: &Path) -> Result<(), String> {
    let mut file = File::create(output).map_err(err)?;
    std::io::copy(&mut File::open(original).map_err(err)?, &mut file).map_err(err)?;
    let mut gzip = GzEncoder::new(file, Compression::fast());
    for name in ["usr", "usr/lib", "usr/lib/modules"] {
        entry(&mut gzip, name, 0o040755, 0, &mut std::io::empty())?;
    }
    for e in fs::read_dir(modloop.join("modules")).map_err(err)? {
        let e = e.map_err(err)?;
        if !e.file_type().map_err(err)?.is_dir() {
            continue;
        }
        let name = e.file_name().to_string_lossy().to_string();
        tree(
            &mut gzip,
            &e.path(),
            &if name == "firmware" {
                "usr/lib/firmware".into()
            } else {
                format!("usr/lib/modules/{name}")
            },
        )?;
    }
    entry(&mut gzip, "TRAILER!!!", 0, 0, &mut std::io::empty())?;
    gzip.finish().map_err(err)?;
    Ok(())
}

// The overlay is staged locally; reject special files and unsafe archive names.
pub(crate) fn overlay(
    out: &mut impl Write,
    root: &Path,
    prefix: &str,
    budget: &mut (u64, usize),
) -> Result<(), String> {
    let mut entries = fs::read_dir(root)
        .map_err(err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(err)?;
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        budget.1 += 1;
        if budget.1 > 65536 {
            return Err("Linux overlay has too many entries".into());
        }
        let filename = e
            .file_name()
            .into_string()
            .map_err(|_| "Non-UTF8 overlay path")?;
        if filename.is_empty()
            || filename == "."
            || filename == ".."
            || filename.contains(['/', '\\', '\0'])
        {
            return Err("Unsafe Linux overlay path".into());
        }
        let name = if prefix.is_empty() {
            filename
        } else {
            format!("{prefix}/{filename}")
        };
        if name.len() > 4096 || name.split('/').count() > 64 {
            return Err("Linux overlay path exceeds limit".into());
        }
        let meta = fs::symlink_metadata(e.path()).map_err(err)?;
        let kind = meta.file_type();
        #[cfg(unix)]
        let permissions = {
            use std::os::unix::fs::PermissionsExt;
            meta.permissions().mode() & 0o777
        };
        #[cfg(not(unix))]
        let permissions = if kind.is_dir()
            || name.starts_with("usr/bin/")
            || name.starts_with("scripts/")
            || name.starts_with("lib/debian-installer-startup.d/")
            || name.starts_with("usr/lib/debian-installer-startup.d/")
            || name.ends_with(".sh")
        {
            0o755
        } else {
            0o644
        };
        if kind.is_dir() {
            entry(out, &name, 0o040000 | permissions, 0, &mut std::io::empty())?;
            overlay(out, &e.path(), &name, budget)?;
        } else if kind.is_symlink() {
            let target = fs::read_link(e.path()).map_err(err)?;
            let target = target
                .to_str()
                .ok_or("Non-UTF8 overlay symlink")?
                .as_bytes();
            if target.len() > 4096 {
                return Err("Overlay symlink exceeds limit".into());
            }
            entry(out, &name, 0o120777, target.len() as u32, &mut &target[..])?;
        } else if kind.is_file() {
            budget.0 = budget
                .0
                .checked_add(meta.len())
                .ok_or("Linux overlay size overflow")?;
            if budget.0 > 512 * 1024 * 1024 {
                return Err("Linux overlay exceeds 512 MiB".into());
            }
            let size = u32::try_from(meta.len()).map_err(err)?;
            entry(
                out,
                &name,
                0o100000 | permissions,
                size,
                &mut File::open(e.path()).map_err(err)?,
            )?;
        } else {
            return Err("Unsupported Linux overlay entry".into());
        }
    }
    Ok(())
}
