use flate2::{Compression, write::GzEncoder};
use std::{
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
};

pub fn client(architecture: &str) -> Result<PathBuf, String> {
    let suffix = match architecture {
        "x64" | "x86_64" | "amd64" => "x64",
        _ => {
            return Err(format!(
                "Linux media client is unavailable for architecture {architecture}"
            ));
        }
    };
    crate::media::asset(&format!("bootah-media-{suffix}")).ok_or_else(|| {
        format!(
            "Missing bootah-media-{suffix}; run tools/linux-media/build.sh and set NETBOOT_ASSETS"
        )
    })
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
