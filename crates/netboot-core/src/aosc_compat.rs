use std::{fs, io::Read, path::Path};

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn inspect_cpio(input: &mut impl Read) -> Result<(), String> {
    let mut nbd = false;
    let mut livekit = false;
    let mut hooks = false;
    let mut total = 0u64;
    for _ in 0..100_000 {
        let mut header = [0u8; 110];
        input.read_exact(&mut header).map_err(err)?;
        if !matches!(&header[..6], b"070701" | b"070702") {
            return Err("AOSC initrd requires a supported newc CPIO".into());
        }
        let field = |a, b| {
            std::str::from_utf8(&header[a..b])
                .map_err(err)
                .and_then(|s| u64::from_str_radix(s, 16).map_err(err))
        };
        let size = field(54, 62)?;
        let namesize = field(94, 102)?;
        if namesize == 0 || namesize > 8192 {
            return Err("Invalid AOSC initrd path".into());
        }
        total = total.checked_add(size).ok_or("AOSC initrd size overflow")?;
        if total > 1024 * 1024 * 1024 {
            return Err("AOSC initrd exceeds 1 GiB inspection limit".into());
        }
        let mut name = vec![0; namesize as usize];
        input.read_exact(&mut name).map_err(err)?;
        let name = std::str::from_utf8(&name[..name.len() - 1])
            .map_err(err)?
            .trim_start_matches("./");
        if name == "TRAILER!!!" {
            break;
        }
        nbd |= name.ends_with("/kernel/drivers/block/nbd.ko");
        livekit |= name == "var/lib/dracut/hooks/cmdline/30-parse-aosc-cmdline.sh";
        hooks |= name == "var/lib/dracut/hooks/initqueue/online"
            || name.starts_with("var/lib/dracut/hooks/initqueue/online/");
        let pad = (4 - (110 + namesize) % 4) % 4;
        std::io::copy(&mut input.take(pad), &mut std::io::sink()).map_err(err)?;
        if std::io::copy(&mut input.take(size), &mut std::io::sink()).map_err(err)? != size {
            return Err("Truncated AOSC initrd entry".into());
        }
        let pad = (4 - size % 4) % 4;
        std::io::copy(&mut input.take(pad), &mut std::io::sink()).map_err(err)?;
    }
    if !nbd || !livekit || !hooks {
        return Err(
            "AOSC initrd lacks matching NBD module or expected LiveKit/dracut hooks".into(),
        );
    }
    Ok(())
}

pub fn prepare(
    original: &Path,
    output: &Path,
    cache: &Path,
    architecture: &str,
) -> Result<(), String> {
    inspect_cpio(&mut crate::archive::decoded(original)?)?;
    let hooks = "var/lib/dracut/hooks";
    let overlay = cache.join("overlay");
    let client = overlay.join("usr/bin/bootah-media");
    fs::create_dir_all(client.parent().unwrap()).map_err(err)?;
    fs::copy(crate::linux_compat::client_for_initrd(original, architecture)?, &client).map_err(err)?;
    let hook = overlay
        .join(hooks)
        .join("initqueue/online/05-bootah-media.sh");
    fs::create_dir_all(hook.parent().unwrap()).map_err(err)?;
    fs::write(&hook, HOOK).map_err(err)?;
    let connector = overlay.join("usr/bin/bootah-media-connect");
    fs::write(&connector, crate::fedora_compat::CONNECT).map_err(err)?;
    let units = overlay.join("etc/systemd/system");
    fs::create_dir_all(&units).map_err(err)?;
    fs::write(
        units.join("bootah-media.service"),
        crate::fedora_compat::UNIT,
    )
    .map_err(err)?;
    let pivot = overlay
        .join(hooks)
        .join("pre-pivot/05-bootah-media-unit.sh");
    fs::create_dir_all(pivot.parent().unwrap()).map_err(err)?;
    fs::write(&pivot, crate::fedora_compat::PIVOT).map_err(err)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for path in [&client, &hook, &connector, &pivot] {
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).map_err(err)?;
        }
    }
    crate::linux_compat::append(original, output, &overlay)?;
    Ok(())
}

const HOOK: &str = r#"#!/usr/bin/bash
[ -e /run/bootah-media.done ] && return 0
transport= server= export_name=
for arg in $(cat /proc/cmdline); do
    case "$arg" in
        bootah.media=*) transport=${arg#*=} ;;
        bootah.server=*) server=${arg#*=} ;;
        bootah.export=*) export_name=${arg#*=} ;;
    esac
done
[ "$transport" = nbd ] || return 0
[ -n "$server" ] && [ -n "$export_name" ] || { warn 'Bootah LiveKit: missing NBD parameters'; return 1; }
echo 'Bootah LiveKit: connecting original ISO via NBD' >/dev/console
modprobe nbd nbds_max=1 max_part=0 || { warn 'Bootah LiveKit: matching NBD module failed'; return 1; }
udevadm settle --timeout=10 || true
systemctl start bootah-media.service || { warn 'Bootah LiveKit: NBD connection failed'; return 1; }
udevadm trigger --action=change /sys/class/block/nbd0 || true
: > /run/bootah-media.done
echo 'Bootah LiveKit: original ISO available at /dev/nbd0' >/dev/console
"#;
