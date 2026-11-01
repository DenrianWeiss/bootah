use flate2::{Compression, write::GzEncoder};
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
};

fn target(architecture: &str) -> Option<(&'static str, &'static str)> {
    Some(match architecture {
        "x64" | "x86_64" | "amd64" => ("x64", "x86_64-unknown-linux-musl"),
        "x86" | "ia32" | "i386" | "i586" | "i686" => ("x86", "i586-unknown-linux-musl"),
        "arm64" | "aarch64" => ("arm64", "aarch64-unknown-linux-musl"),
        "ppc" | "powerpc" => ("powerpc", "powerpc-unknown-linux-gnu"),
        "ppc64" | "powerpc64" => ("ppc64", "powerpc64-unknown-linux-gnu"),
        "ppc64le" | "powerpc64le" => ("ppc64le", "powerpc64le-unknown-linux-gnu"),
        "riscv64" => ("riscv64", "riscv64gc-unknown-linux-musl"),
        "loongarch64" => ("loongarch64", "loongarch64-unknown-linux-musl"),
        "sparc64" => ("sparc64", "sparc64-unknown-linux-gnu"),
        "alpha" => ("alpha", "alpha-linux-gnu"),
        "hppa" => ("hppa", "hppa-linux-gnu"),
        "ia64" => ("ia64", "ia64-linux-gnu"),
        _ => return None,
    })
}
pub fn supported(architecture: &str) -> bool {
    target(architecture).is_some() || architecture == "hppa64"
}
pub fn client(architecture: &str) -> Result<PathBuf, String> {
    let (suffix, triple) = target(architecture).ok_or_else(|| {
        format!("Linux media client is unavailable for architecture {architecture}")
    })?;
    let builder = if matches!(suffix, "alpha" | "hppa" | "ia64") {
        "tools/linux-media-c/build.sh"
    } else {
        "tools/linux-media/build.sh"
    };
    let asset = crate::media::asset(&format!("bootah-media-{suffix}")).ok_or_else(|| {
        format!("Missing bootah-media-{suffix}; run {builder} {triple} and set NETBOOT_ASSETS")
    })?;
    if crate::media::binary_arch(&asset) != Some(suffix) {
        return Err(format!(
            "Wrong executable architecture in bootah-media-{suffix}"
        ));
    }
    Ok(asset)
}

pub fn client_for_initrd(original: &Path, kernel_arch: &str) -> Result<PathBuf, String> {
    client(initrd_architecture(original, kernel_arch)?)
}
fn initrd_architecture(original: &Path, kernel_arch: &str) -> Result<&'static str, String> {
    let kernel_arch = target(kernel_arch).map_or(kernel_arch, |(name, _)| name);
    let mut input = crate::archive::decoded(original)?.take(1024 * 1024 * 1024);
    for _ in 0..100_000 {
        let mut header = [0u8; 110];
        loop {
            input
                .read_exact(&mut header[..1])
                .map_err(|e| e.to_string())?;
            if header[0] != 0 {
                break;
            }
        }
        input
            .read_exact(&mut header[1..])
            .map_err(|e| e.to_string())?;
        if !matches!(&header[..6], b"070701" | b"070702") {
            return Err("Cannot determine installer userspace ABI from newc initrd".into());
        }
        let field = |a, b| {
            std::str::from_utf8(&header[a..b])
                .map_err(|e| e.to_string())
                .and_then(|s| u64::from_str_radix(s, 16).map_err(|e| e.to_string()))
        };
        let size = field(54, 62)?;
        let namesize = field(94, 102)?;
        let mode = field(14, 22)?;
        if !(1..=8192).contains(&namesize) {
            return Err("Invalid initrd ABI inspection path".into());
        }
        let mut name = vec![0; namesize as usize];
        input.read_exact(&mut name).map_err(|e| e.to_string())?;
        if name.last() != Some(&0) {
            return Err("Unterminated initrd ABI inspection path".into());
        }
        let name = std::str::from_utf8(&name[..name.len() - 1])
            .map_err(|e| e.to_string())?
            .trim_start_matches("./");
        let skip = |input: &mut dyn Read, n| -> Result<(), String> {
            if std::io::copy(&mut input.take(n), &mut std::io::sink()).map_err(|e| e.to_string())?
                != n
            {
                return Err("Truncated initrd during ABI inspection".into());
            }
            Ok(())
        };
        skip(&mut input, (4 - (110 + namesize) % 4) % 4)?;
        let user_binary = mode & 0xf000 == 0x8000
            && [
                "bin/",
                "sbin/",
                "usr/bin/",
                "usr/sbin/",
                "lib/systemd/",
                "usr/lib/systemd/",
            ]
            .iter()
            .any(|prefix| name.starts_with(prefix));
        let mut prefix = [0u8; 64];
        let n = if user_binary {
            size.min(64) as usize
        } else {
            0
        };
        input
            .read_exact(&mut prefix[..n])
            .map_err(|e| e.to_string())?;
        if prefix[..n].starts_with(b"\x7fELF")
            && let Some(user_arch) = crate::media::binary_arch_bytes(&prefix[..n])
        {
            let compatible = user_arch == kernel_arch
                || matches!(
                    (kernel_arch, user_arch),
                    ("x64", "x86") | ("ppc64", "powerpc") | ("hppa64", "hppa")
                );
            if !compatible {
                return Err(format!(
                    "Installer userspace {user_arch} does not match kernel {kernel_arch}"
                ));
            }
            if !supported(user_arch) {
                return Err(format!(
                    "Linux media client is unavailable for architecture {user_arch}"
                ));
            }
            return Ok(user_arch);
        }
        skip(&mut input, size - n as u64)?;
        skip(&mut input, (4 - size % 4) % 4)?;
    }
    Err("Installer initrd has no supported userspace ELF executable".into())
}
pub fn append(original: &Path, output: &Path, overlay: &Path) -> Result<(), String> {
    if original == output
        || fs::canonicalize(original)
            .ok()
            .is_some_and(|source| fs::canonicalize(output).ok().as_ref() == Some(&source))
    {
        return Err("Initrd input and output must differ".into());
    }
    if fs::metadata(original).map_err(|e| e.to_string())?.len() > 1024 * 1024 * 1024 {
        return Err("Original initrd exceeds 1 GiB".into());
    }
    let mut out = File::create(output).map_err(|e| e.to_string())?;
    std::io::copy(
        &mut File::open(original).map_err(|e| e.to_string())?,
        &mut out,
    )
    .map_err(|e| e.to_string())?;
    let mut gzip = GzEncoder::new(out, Compression::fast());
    let mut budget = (0u64, 0usize);
    crate::initramfs::overlay(&mut gzip, overlay, "", &mut budget)?;
    crate::initramfs::entry(&mut gzip, "TRAILER!!!", 0, 0, &mut std::io::empty())?;
    gzip.finish()
        .map_err(|e| e.to_string())?
        .flush()
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn initrd(machine: u16, class: u8, endian: u8) -> PathBuf {
        let path = std::env::temp_dir().join(format!("bootah-abi-{}", uuid::Uuid::new_v4()));
        let mut elf = [0u8; 64];
        elf[..7].copy_from_slice(&[0x7f, b'E', b'L', b'F', class, endian, 1]);
        elf[18..20].copy_from_slice(&if endian == 2 {
            machine.to_be_bytes()
        } else {
            machine.to_le_bytes()
        });
        let mut archive = Vec::new();
        crate::initramfs::entry(
            &mut archive,
            "bin/busybox",
            0o100755,
            elf.len() as u32,
            &mut elf.as_slice(),
        )
        .unwrap();
        crate::initramfs::entry(&mut archive, "TRAILER!!!", 0, 0, &mut std::io::empty()).unwrap();
        fs::write(&path, archive).unwrap();
        path
    }
    #[test]
    fn selects_installer_userspace_instead_of_kernel_width() {
        for (machine, class, endian, kernel, expected) in [
            (15, 1, 2, "hppa64", "hppa"),
            (20, 1, 2, "ppc64", "powerpc"),
            (3, 1, 1, "x64", "x86"),
            (21, 2, 2, "ppc64", "ppc64"),
        ] {
            let path = initrd(machine, class, endian);
            let actual = initrd_architecture(&path, kernel);
            fs::remove_file(path).unwrap();
            assert_eq!(actual.unwrap(), expected);
        }
    }
    #[test]
    fn rejects_excluded_architectures_and_mismatched_endianness() {
        for name in [
            "arm32", "aarch32", "sparc", "sparc32", "riscv32", "s390x", "mips", "mipsel", "mips64",
            "mips64el", "m68k",
        ] {
            assert!(!supported(name), "{name}");
        }
        let path = initrd(21, 2, 1);
        assert!(initrd_architecture(&path, "ppc64").is_err());
        fs::remove_file(path).unwrap();
    }
}
