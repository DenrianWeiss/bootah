use crate::media_source::Image;
use flate2::read::GzDecoder;
use std::{fs, io::Read, path::Path};

fn kernel_release(original: &Path) -> Result<(String, bool), String> {
    let mut bytes = Vec::new();
    GzDecoder::new(fs::File::open(original).map_err(|e| e.to_string())?)
        .take(128 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 128 * 1024 * 1024 {
        return Err("Debian CD initrd exceeds adapter budget".into());
    }
    let mut offset = 0usize;
    while offset + 110 <= bytes.len() {
        let header = &bytes[offset..offset + 110];
        if !matches!(&header[..6], b"070701" | b"070702") {
            return Err("Unsupported Debian CD initrd CPIO".into());
        }
        let hex = |a, b| {
            std::str::from_utf8(&header[a..b])
                .map_err(|e| e.to_string())
                .and_then(|s| usize::from_str_radix(s, 16).map_err(|e| e.to_string()))
        };
        let size = hex(54, 62)?;
        let namesize = hex(94, 102)?;
        let start = offset + 110;
        let end = start
            .checked_add(namesize)
            .filter(|n| *n <= bytes.len())
            .ok_or("Truncated Debian CPIO name")?;
        if namesize == 0 {
            return Err("Empty Debian CPIO name".into());
        }
        let name = std::str::from_utf8(&bytes[start..end - 1])
            .map_err(|e| e.to_string())?
            .trim_start_matches("./");
        let merged_usr = name.starts_with("usr/lib/modules/");
        if let Some(rest) = name
            .strip_prefix("lib/modules/")
            .or_else(|| name.strip_prefix("usr/lib/modules/"))
            && let Some((version, _)) = rest.split_once('/')
            && !version.is_empty()
            && version
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".+-_".contains(&b))
        {
            return Ok((version.into(), merged_usr));
        }
        if name == "TRAILER!!!" {
            break;
        }
        offset = ((end + 3) & !3)
            .checked_add(size)
            .and_then(|n| n.checked_add(3))
            .map(|n| n & !3)
            .filter(|n| *n <= bytes.len())
            .ok_or("Truncated Debian CPIO contents")?;
    }
    Err("Original Debian CD initrd has no kernel module release".into())
}

pub fn prepare(
    image: &Image,
    original: &Path,
    output: &Path,
    cache: &Path,
    architecture: &str,
) -> Result<(), String> {
    let (version, merged_usr) = kernel_release(original)?;
    let overlay = cache.join("overlay");
    fs::create_dir_all(&overlay).map_err(|e| e.to_string())?;
    for package in ["nic-modules", "nbd-modules"] {
        let prefix = format!("{package}-{version}-di_");
        let path = image
            .entries()
            .iter()
            .find(|(name, e)| {
                !e.is_dir
                    && name
                        .rsplit('/')
                        .next()
                        .is_some_and(|base| base.starts_with(&prefix) && base.ends_with(".udeb"))
            })
            .map(|(name, _)| name)
            .ok_or_else(|| format!("Debian original ISO lacks matching {package} for {version}"))?;
        let archive = cache.join(format!("{package}.udeb"));
        let unpack = cache.join(package);
        fs::create_dir_all(&unpack).map_err(|e| e.to_string())?;
        image.materialize(path, &archive)?;
        crate::media::extract_external(&archive, &unpack, 16)?;
        let tar = fs::read_dir(&unpack)
            .map_err(|e| e.to_string())?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .find(|p| {
                p.file_name().is_some_and(|n| {
                    n.to_string_lossy() == "data.tar"
                        || n.to_string_lossy().starts_with("data.tar.")
                })
            })
            .ok_or_else(|| format!("Missing Debian {package} data archive"))?;
        if !tar.is_file() {
            return Err(format!("Cannot unpack Debian {package} data archive"));
        }
        crate::media::extract_external(&tar, &overlay, 16)?;
    }
    if merged_usr && overlay.join("lib").is_dir() {
        fs::create_dir_all(overlay.join("usr")).map_err(|e| e.to_string())?;
        fs::rename(overlay.join("lib"), overlay.join("usr/lib")).map_err(|e| e.to_string())?;
    }
    prepare_hook(&overlay, architecture, merged_usr)?;
    crate::linux_compat::append(original, output, &overlay)
}

fn prepare_hook(overlay: &Path, architecture: &str, merged_usr: bool) -> Result<(), String> {
    let client = crate::linux_compat::client(architecture)?;
    fs::create_dir_all(overlay.join("usr/bin")).map_err(|e| e.to_string())?;
    fs::copy(client, overlay.join("usr/bin/bootah-media")).map_err(|e| e.to_string())?;
    let hook = overlay.join(if merged_usr {
        "usr/lib/debian-installer-startup.d/S25bootah-media"
    } else {
        "lib/debian-installer-startup.d/S25bootah-media"
    });
    fs::create_dir_all(hook.parent().unwrap()).map_err(|e| e.to_string())?;
    fs::write(&hook, HOOK).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for path in [&hook, &overlay.join("usr/bin/bootah-media")] {
            fs::set_permissions(path, fs::Permissions::from_mode(0o755))
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

const HOOK: &str = r#"#!/bin/sh
set -eu
transport=nbd
server=
port=10809
export_name=
iface=
for arg in $(cat /proc/cmdline); do
    case "$arg" in
        bootah.media=*) transport=${arg#*=} ;;
        bootah.server=*) server=${arg#*=} ;;
        bootah.port=*) port=${arg#*=} ;;
        bootah.export=*) export_name=${arg#*=} ;;
        bootah.iface=*) iface=${arg#*=} ;;
    esac
done
fail() { logger -t bootah-media "$*"; echo "Bootah media: $*" >/dev/console; exit 1; }
[ -n "$server" ] || fail 'missing server parameter'
depmod -a
udevadm trigger --action=add
udevadm settle --timeout=10 || true
if [ -z "$iface" ]; then
    for interface in /sys/class/net/*; do
        [ "${interface##*/}" = lo ] && continue
        candidate=${interface##*/}
        ip link set "$candidate" up || continue
        [ -n "$iface" ] || iface=$candidate
        if [ "$(cat "$interface/carrier" 2>/dev/null || true)" = 1 ]; then
            iface=$candidate
            break
        fi
    done
fi
[ -n "$iface" ] || fail 'no network interface'
ip link set "$iface" up
echo "Bootah media: DHCP on $iface ($(cat /sys/class/net/$iface/address))" >/dev/console
udhcpc -n -q -t 3 -T 3 -i "$iface" -s /etc/udhcpc/default.script || fail 'DHCP failed'
mkdir -p /cdrom
case "$transport" in
    nbd)
        modprobe nbd nbds_max=1 max_part=0 || fail 'matching NBD module failed'
        udevadm settle --timeout=10 || true
        echo "Bootah media: connecting NBD $server:$port/$export_name" >/dev/console
        /usr/bin/bootah-media nbd "$server" "$port" "$export_name" /dev/nbd0 || fail 'NBD connection failed'
        mount -t iso9660 -o ro /dev/nbd0 /cdrom || fail 'NBD ISO mount failed'
        ;;
    *) fail 'unsupported transport' ;;
esac
[ -f /cdrom/.disk/info ] || fail 'mounted image is not Debian installation media'
debconf-set cdrom-detect/cdrom_device /dev/nbd0
debconf-set cdrom-detect/cdrom_fs iso9660
debconf-set cdrom-detect/hybrid true
for package in eject-udeb apt-mirror-setup; do
    if [ -n "$(find /cdrom/pool -name "${package}_*.udeb" | head -n 1)" ]; then
        DEBIAN_FRONTEND=noninteractive anna-install "$package" || true
    fi
done
if [ -e /cdrom/.disk/base_installable ]; then
    DEBIAN_FRONTEND=noninteractive anna-install apt-cdrom-setup
    for release in /cdrom/dists/*/Release; do
        [ -f "$release" ] || continue
        codename=$(sed -n 's/^Codename:[[:space:]]*//p' "$release" | head -n 1)
        case "$codename" in
            ''|*[!a-zA-Z0-9-]*) ;;
            *)
                if [ -n "$(find /cdrom/pool -name "${codename}-support_*.udeb" | head -n 1)" ]; then
                    DEBIAN_FRONTEND=noninteractive anna-install "${codename}-support" || true
                fi
                ;;
        esac
        break
    done
else
    DEBIAN_FRONTEND=noninteractive anna-install choose-mirror
fi
echo 'Bootah media: original CD installer state and component queue prepared' >/dev/console
logger -t bootah-media 'Original ISO mounted read-only at /cdrom'
echo 'Bootah media: original ISO mounted at /cdrom' >/dev/console
grep ' /cdrom ' /proc/mounts >/dev/console
cat /cdrom/.disk/info >/dev/console
"#;
