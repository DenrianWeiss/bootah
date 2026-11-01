use std::{
    fs,
    io::{Read, Seek, SeekFrom},
    path::Path,
};

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn inspect_cpio(input: &mut impl Read) -> Result<String, String> {
    let mut nbd = false;
    let mut casper = false;
    let mut premount = false;
    let mut order = None;
    let mut trailer = false;
    let mut total = 0u64;
    for _ in 0..100_000 {
        let mut header = [0; 110];
        input.read_exact(&mut header).map_err(err)?;
        if !matches!(&header[..6], b"070701" | b"070702") {
            return Err("Casper initrd requires a supported newc CPIO".into());
        }
        let field = |a, b| {
            std::str::from_utf8(&header[a..b])
                .map_err(err)
                .and_then(|s| u64::from_str_radix(s, 16).map_err(err))
        };
        let size = field(54, 62)?;
        let namesize = field(94, 102)?;
        if namesize == 0 || namesize > 8192 {
            return Err("Invalid Casper initrd path".into());
        }
        total = total
            .checked_add(size)
            .ok_or("Casper initrd size overflow")?;
        if total > 2 * 1024 * 1024 * 1024 {
            return Err("Casper initrd exceeds 2 GiB inspection limit".into());
        }
        let mut name = vec![0; namesize as usize];
        input.read_exact(&mut name).map_err(err)?;
        let name = std::str::from_utf8(&name[..name.len() - 1])
            .map_err(err)?
            .trim_start_matches("./");
        if name == "TRAILER!!!" {
            trailer = true;
            break;
        }
        nbd |= name.ends_with("/kernel/drivers/block/nbd.ko");
        casper |= name == "scripts/casper";
        premount |= name.starts_with("scripts/casper-premount/");
        let pad = (4 - (110 + namesize) % 4) % 4;
        std::io::copy(&mut input.take(pad), &mut std::io::sink()).map_err(err)?;
        if name == "scripts/casper-premount/ORDER" && size < 65536 {
            let mut script = vec![0; size as usize];
            input.read_exact(&mut script).map_err(err)?;
            order = Some(String::from_utf8(script).map_err(err)?);
        } else if std::io::copy(&mut input.take(size), &mut std::io::sink()).map_err(err)? != size {
            return Err("Truncated Casper initrd entry".into());
        }
        let pad = (4 - size % 4) % 4;
        std::io::copy(&mut input.take(pad), &mut std::io::sink()).map_err(err)?;
    }
    if !trailer || !nbd || !casper || !premount {
        return Err("Casper initrd lacks matching NBD module or expected premount hook".into());
    }
    order.ok_or_else(|| "Casper initrd lacks a premount ORDER script".into())
}

pub fn prepare(
    original: &Path,
    output: &Path,
    cache: &Path,
    architecture: &str,
) -> Result<(), String> {
    let mut input = fs::File::open(original).map_err(err)?;
    let mut prefix = vec![0; 32 * 1024 * 1024];
    let read = input.read(&mut prefix).map_err(err)?;
    let offset = prefix[..read]
        .windows(4)
        .position(|bytes| bytes == [0x28, 0xb5, 0x2f, 0xfd])
        .ok_or("Casper initrd has no Zstandard archive in the first 32 MiB")?;
    input.seek(SeekFrom::Start(offset as u64)).map_err(err)?;
    let compressed = cache.join("casper-core.zst");
    std::io::copy(&mut input, &mut fs::File::create(&compressed).map_err(err)?).map_err(err)?;
    let inspected = inspect_cpio(&mut crate::archive::decoded(&compressed)?)?;
    fs::remove_file(compressed).map_err(err)?;

    let overlay = cache.join("overlay");
    let client = overlay.join("usr/bin/bootah-media");
    fs::create_dir_all(client.parent().unwrap()).map_err(err)?;
    fs::copy(crate::linux_compat::client_for_initrd(original, architecture)?, &client).map_err(err)?;
    let hook = overlay.join("scripts/casper-premount/05bootah-media");
    fs::create_dir_all(hook.parent().unwrap()).map_err(err)?;
    fs::write(&hook, HOOK).map_err(err)?;
    fs::write(
        hook.parent().unwrap().join("ORDER"),
        format!("/scripts/casper-premount/05bootah-media \"$@\"\n{inspected}"),
    )
    .map_err(err)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for path in [&client, &hook] {
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).map_err(err)?;
        }
    }
    crate::linux_compat::append(original, output, &overlay)
}

const HOOK: &str = r#"#!/bin/sh
[ -e /run/bootah-media.done ] && exit 0
transport= server= export_name= port=10809
for arg in $(cat /proc/cmdline); do
    case "$arg" in
        bootah.media=*) transport=${arg#*=} ;;
        bootah.server=*) server=${arg#*=} ;;
        bootah.port=*) port=${arg#*=} ;;
        bootah.export=*) export_name=${arg#*=} ;;
    esac
done
[ "$transport" = nbd ] || exit 0
[ -n "$server" ] && [ -n "$export_name" ] || { echo 'Bootah Casper: missing NBD parameters' >/dev/console; exit 1; }
echo 'Bootah Casper: connecting original ISO via NBD' >/dev/console
modprobe nbd nbds_max=1 max_part=0 || exit 1
udevadm settle --timeout=10 || true
/usr/bin/bootah-media nbd "$server" "$port" "$export_name" /dev/nbd0 || exit 1
echo 'Bootah Casper: original ISO available at /dev/nbd0' >/dev/console
: > /run/bootah-media.done
"#;
