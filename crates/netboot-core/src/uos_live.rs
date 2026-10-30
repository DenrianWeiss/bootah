use crate::media_source::Image;
use std::{
    fs,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};

fn append_before_lz4(original: &Path, output: &Path, overlay: &Path) -> Result<(), String> {
    let mut input = fs::File::open(original).map_err(|e| e.to_string())?;
    let mut offset = 0u64;
    loop {
        let mut header = [0u8; 110];
        input.read_exact(&mut header).map_err(|e| e.to_string())?;
        if !matches!(&header[..6], b"070701" | b"070702") {
            return Err("UOS initrd has no supported early CPIO archive".into());
        }
        let field = |start, end| -> Result<u64, String> {
            let value = std::str::from_utf8(&header[start..end]).map_err(|e| e.to_string())?;
            u64::from_str_radix(value, 16).map_err(|e| e.to_string())
        };
        let size = field(54, 62)?;
        let name_size = field(94, 102)?;
        if name_size == 0 || name_size > 8192 || size > 64 * 1024 * 1024 {
            return Err("UOS early CPIO entry is invalid".into());
        }
        let mut name = vec![0; name_size as usize];
        input.read_exact(&mut name).map_err(|e| e.to_string())?;
        offset += 110 + name_size;
        offset += (4 - offset % 4) % 4;
        offset = offset.checked_add(size).ok_or("UOS initrd size overflow")?;
        offset += (4 - offset % 4) % 4;
        if offset > 64 * 1024 * 1024 {
            return Err("UOS early CPIO archive is too large".into());
        }
        input
            .seek(SeekFrom::Start(offset))
            .map_err(|e| e.to_string())?;
        if name.starts_with(b"TRAILER!!!\0") {
            break;
        }
    }
    let split = (offset + 511) & !511;
    input
        .seek(SeekFrom::Start(split))
        .map_err(|e| e.to_string())?;
    let mut magic = [0; 4];
    input.read_exact(&mut magic).map_err(|e| e.to_string())?;
    if magic != [0x02, 0x21, 0x4c, 0x18] {
        return Err("UOS initrd does not have the expected LZ4 segment".into());
    }
    input.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    let mut out = fs::File::create(output).map_err(|e| e.to_string())?;
    std::io::copy(&mut Read::by_ref(&mut input).take(split), &mut out)
        .map_err(|e| e.to_string())?;
    let mut budget = (0u64, 0usize);
    crate::initramfs::overlay(&mut out, overlay, "", &mut budget)?;
    crate::initramfs::entry(&mut out, "TRAILER!!!", 0, 0, &mut std::io::empty())?;
    input
        .seek(SeekFrom::Start(split))
        .map_err(|e| e.to_string())?;
    std::io::copy(&mut input, &mut out).map_err(|e| e.to_string())?;
    out.flush().map_err(|e| e.to_string())
}

pub fn prepare(image: &Image, original: &Path, output: &Path, cache: &Path) -> Result<(), String> {
    let grub = signed_grub(image).ok_or("UOS live media has no signed GRUB configuration")?;
    let mut files = vec![grub.to_string(), format!("{grub}_sign")];
    for path in image.entries().keys() {
        if let Some(kernel) = path.strip_suffix("_sign") {
            if kernel
                .rsplit('/')
                .next()
                .is_some_and(|name| name.starts_with("vmlinuz"))
                && image.find(kernel).is_some()
            {
                files.push(kernel.to_string());
                files.push(path.clone());
            }
        }
    }
    files.sort();
    files.dedup();
    for path in &files {
        image
            .find(path)
            .ok_or_else(|| format!("UOS live media is missing {path}"))?;
    }
    let overlay = cache.join("uos-overlay");
    let manifest = overlay.join("etc/bootah-uos-media-files");
    fs::create_dir_all(manifest.parent().unwrap()).map_err(|e| e.to_string())?;
    fs::write(&manifest, files.join("\n") + "\n").map_err(|e| e.to_string())?;
    let hook = overlay.join("scripts/live-bottom/bootah-uos-source");
    fs::create_dir_all(hook.parent().unwrap()).map_err(|e| e.to_string())?;
    fs::write(&hook, HOOK).map_err(|e| e.to_string())?;
    fs::write(
        overlay.join("scripts/live-bottom/ORDER"),
        "/scripts/live-bottom/bootah-uos-source \"$@\"\n",
    )
    .map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    }
    append_before_lz4(original, output, &overlay)
}

pub fn signed_grub(image: &Image) -> Option<&str> {
    ["boot/grub/grub.cfg", "efi/boot/grub.cfg", "boot/grub.cfg"]
        .iter()
        .filter_map(|path| image.find(path))
        .find(|path| image.find(&format!("{path}_sign")).is_some())
}

const HOOK: &str = r#"#!/bin/sh
url=
for arg in $(cat /proc/cmdline); do
    case "$arg" in fetch=*) url=${arg#*=} ;; esac
done
case "$url" in
    */live/filesystem.squashfs) base=${url%/live/filesystem.squashfs} ;;
    *) echo 'Bootah UOS: missing live source URL' >/dev/console; exit 1 ;;
esac
medium=/root/usr/lib/live/mount/medium
[ -f "$medium/live/filesystem.squashfs" ] || medium=/root/run/live/medium
[ -f "$medium/live/filesystem.squashfs" ] || exit 1
while IFS= read -r name; do
    [ -n "$name" ] || continue
    target=$medium/$name
    mkdir -p "${target%/*}" || exit 1
    wget -q -O "$target" "$base/$name" || { echo "Bootah UOS: cannot fetch $name" >/dev/console; exit 1; }
done < /etc/bootah-uos-media-files
echo 'Bootah UOS: original signed boot files restored from ISO' >/dev/console
"#;
