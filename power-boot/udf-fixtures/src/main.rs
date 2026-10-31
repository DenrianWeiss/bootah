use libmkiso::{AllocationMode, UdfImage, UdfOptions, UdfPartition, UdfRevision};
use std::{fs, io::Write, path::PathBuf};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = PathBuf::from(std::env::args().nth(1).ok_or("Specify output directory")?);
    fs::create_dir_all(&root)?;
    let name = format!("目录/{}-complete.bin", "long-name-".repeat(20));
    for (label, revision, partition) in [
        ("vat150", UdfRevision::V150, UdfPartition::Virtual),
        ("vat201", UdfRevision::V201, UdfPartition::Virtual),
        (
            "sparable",
            UdfRevision::V201,
            UdfPartition::Sparable { packet_blocks: 32 },
        ),
        (
            "metadata250",
            UdfRevision::V250,
            UdfPartition::Metadata { mirror: true },
        ),
        (
            "metadata260",
            UdfRevision::V260,
            UdfPartition::MetadataSparable {
                mirror: true,
                packet_blocks: 32,
            },
        ),
    ] {
        let mut image = UdfImage::new();
        image.add_bytes(&name, (0..20_003).map(|n| (n % 251) as u8).collect())?;
        image.add_bytes("empty", vec![])?;
        image.add_bytes("目录/中文.txt", b"unicode".to_vec())?;
        let options = UdfOptions {
            revision,
            partition,
            allocation: AllocationMode::Long,
            extent_blocks: 2,
            metadata_extent_blocks: if label.starts_with("metadata") { 32 } else { 0 },
            sparing_packets: if label == "sparable" { vec![0] } else { vec![] },
            ..Default::default()
        };
        let output = root.join(format!("{label}.udf"));
        if output.exists() {
            fs::remove_file(&output)?;
        }
        image.write(&output, &options)?;
        let mut gzip = flate2::write::GzEncoder::new(
            fs::File::create(root.join(format!("{label}.udf.gz")))?,
            flate2::Compression::best(),
        );
        gzip.write_all(&fs::read(&output)?)?;
        gzip.finish()?;
    }
    Ok(())
}
