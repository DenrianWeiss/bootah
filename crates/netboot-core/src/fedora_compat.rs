use crate::media_source::Image;
use std::{
    fs,
    io::{Read, Seek, SeekFrom},
    path::Path,
};
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

// Validate the target's own module and supported hook location without unpacking its tree.
pub(crate) fn inspect_cpio(path: &Path) -> Result<String, String> {
    let mut file = fs::File::open(path).map_err(err)?;
    let length = file.metadata().map_err(err)?.len();
    let mut offset = 0u64;
    let mut nbd = false;
    let mut diskroot = false;
    let mut hooks = None;
    for _ in 0..100_000 {
        if offset + 110 > length {
            break;
        }
        file.seek(SeekFrom::Start(offset)).map_err(err)?;
        let mut header = [0; 110];
        file.read_exact(&mut header).map_err(err)?;
        if !matches!(&header[..6], b"070701" | b"070702") {
            return Err("Linux initrd requires a supported newc CPIO".into());
        }
        let hex = |a, b| {
            std::str::from_utf8(&header[a..b])
                .map_err(err)
                .and_then(|s| u64::from_str_radix(s, 16).map_err(err))
        };
        let size = hex(54, 62)?;
        let namesize = hex(94, 102)?;
        if namesize == 0 || namesize > 8192 {
            return Err("Invalid Linux initrd path".into());
        }
        let mut name = vec![0; namesize as usize];
        file.read_exact(&mut name).map_err(err)?;
        let name = std::str::from_utf8(&name[..name.len() - 1])
            .map_err(err)?
            .trim_start_matches("./");
        if name == "TRAILER!!!" {
            break;
        }
        if name.contains("/kernel/drivers/block/nbd.ko") {
            nbd = true;
        }
        diskroot |= name == "usr/bin/anaconda-diskroot" || name == "bin/anaconda-diskroot";
        for prefix in [
            "var/lib/dracut/hooks",
            "usr/lib/dracut/hooks",
            "lib/dracut/hooks",
        ] {
            if name.starts_with(&format!("{prefix}/cmdline/")) {
                hooks = Some(prefix.to_string());
            }
        }
        offset = ((offset + 110 + namesize + 3) & !3)
            .checked_add(size)
            .and_then(|v| v.checked_add(3))
            .map(|v| v & !3)
            .filter(|v| *v <= length)
            .ok_or("Truncated Linux initrd")?;
    }
    if !nbd {
        return Err("Original Linux initrd lacks its matching NBD kernel module".into());
    }
    if !diskroot {
        return Err("Original initrd lacks Anaconda diskroot handoff".into());
    }
    hooks.ok_or_else(|| "Original initrd lacks supported dracut hooks".into())
}

pub fn prepare(
    _image: &Image,
    original: &Path,
    output: &Path,
    cache: &Path,
    architecture: &str,
) -> Result<(), String> {
    let unpack = cache.join("inspect");
    fs::create_dir_all(&unpack).map_err(err)?;
    let archive = unpack.join("initrd.cpio");
    let limit = fs::metadata(original)
        .map_err(err)?
        .len()
        .saturating_mul(4)
        .min(512 * 1024 * 1024);
    let count = std::io::copy(
        &mut crate::archive::decoded(original)?.take(limit + 1),
        &mut fs::File::create(&archive).map_err(err)?,
    )
    .map_err(err)?;
    if count > limit {
        return Err("Fedora initrd exceeds inspection budget".into());
    }
    let hooks = inspect_cpio(&archive)?;
    let overlay = cache.join("overlay");
    fs::create_dir_all(overlay.join("usr/bin")).map_err(err)?;
    let client = overlay.join("usr/bin/bootah-media");
    fs::copy(crate::linux_compat::client(architecture)?, &client).map_err(err)?;
    let hook = overlay
        .join(hooks)
        .join("initqueue/online/05-bootah-media.sh");
    fs::create_dir_all(hook.parent().unwrap()).map_err(err)?;
    fs::write(&hook, HOOK).map_err(err)?;
    let connector = overlay.join("usr/bin/bootah-media-connect");
    fs::write(&connector, CONNECT).map_err(err)?;
    let units = overlay.join("etc/systemd/system");
    fs::create_dir_all(&units).map_err(err)?;
    fs::write(units.join("bootah-media.service"), UNIT).map_err(err)?;
    let pivot = hook
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("pre-pivot/05-bootah-media-unit.sh");
    fs::create_dir_all(pivot.parent().unwrap()).map_err(err)?;
    fs::write(&pivot, PIVOT).map_err(err)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for path in [&client, &hook, &connector, &pivot] {
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).map_err(err)?;
        }
    }
    crate::linux_compat::append(original, output, &overlay)?;
    fs::remove_dir_all(&unpack).map_err(err)?;
    Ok(())
}

const HOOK: &str = r#"#!/usr/bin/bash
# Sourced by the original dracut when a configured network interface is online.
[ -e /run/bootah-media.done ] && return 0
transport= server= export_name= selected_iface= port=10809
for arg in $(cat /proc/cmdline); do
    case "$arg" in
        bootah.media=*) transport=${arg#*=} ;;
        bootah.server=*) server=${arg#*=} ;;
        bootah.port=*) port=${arg#*=} ;;
        bootah.export=*) export_name=${arg#*=} ;;
        bootah.iface=*) selected_iface=${arg#*=} ;;
    esac
done
[ "$transport" = nbd ] || return 0
[ -n "$selected_iface" ] && [ "$selected_iface" != "$1" ] && return 0
[ -n "$server" ] && [ -n "$export_name" ] || { warn 'Bootah compatibility: missing NBD parameters'; return 1; }
echo "Bootah compatibility: Fedora dracut NBD hook on $1" >/dev/console
modprobe nbd nbds_max=1 max_part=0 || { warn 'Bootah matching NBD module failed'; return 1; }
udevadm settle --timeout=10 || true
systemctl start bootah-media.service || { warn 'Bootah NBD connection failed'; return 1; }
udevadm trigger --action=change /sys/class/block/nbd0 || true
/usr/bin/anaconda-diskroot /dev/nbd0 || { warn 'Bootah original Anaconda diskroot failed'; return 1; }
[ -f /run/install/repo/.treeinfo ] || { warn 'Bootah NBD ISO lacks installation tree'; return 1; }
: > /run/bootah-media.done
echo 'Bootah compatibility: Fedora original ISO mounted read-only via NBD' >/dev/console
grep ' /run/install/repo ' /proc/mounts >/dev/console
"#;

pub(crate) const UNIT: &str = r#"[Unit]
Description=Bootah read-only installation media connection
DefaultDependencies=no
IgnoreOnIsolate=yes
RefuseManualStop=yes
SurviveFinalKillSignal=yes
[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=/usr/bin/bootah-media-connect
"#;

pub(crate) const CONNECT: &str = r#"#!/usr/bin/bash
server= export_name= port=10809
for arg in $(cat /proc/cmdline); do
    case "$arg" in
        bootah.server=*) server=${arg#*=} ;;
        bootah.port=*) port=${arg#*=} ;;
        bootah.export=*) export_name=${arg#*=} ;;
    esac
done
[ -n "$server" ] && [ -n "$export_name" ] || exit 1
exec /usr/bin/bootah-media nbd "$server" "$port" "$export_name" /dev/nbd0
"#;

pub(crate) const PIVOT: &str = r#"#!/usr/bin/bash
[ -e /run/bootah-media.done ] || return 0
mkdir -p /sysroot/etc/systemd/system
cp /etc/systemd/system/bootah-media.service /sysroot/etc/systemd/system/
echo 'Bootah compatibility: NBD storage service preserved across switch-root' >/dev/console
"#;
