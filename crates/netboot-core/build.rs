use std::{env, fs, path::PathBuf};
fn main() {
    let path = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("../../versions.json");
    println!("cargo:rerun-if-changed={}", path.display());
    let versions: serde_json::Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    for name in ["core", "tui"] {
    let version = versions[name]["display"].as_str().expect("display version");
    let components: Vec<_> = version.split('.').collect();
    assert!(components.len() == 3 && components.iter().all(|s| !s.is_empty() && (s.len() == 1 || !s.starts_with('0')) && s.parse::<u16>().is_ok_and(|n| n < 65535)), "invalid core display version");
    let build = versions[name]["build"].as_u64().filter(|n| (1..=65534).contains(n)).expect("invalid build version");
    println!("cargo:rustc-env=BOOTAH_{}_VERSION={version}", name.to_uppercase());
    println!("cargo:rustc-env=BOOTAH_{}_BUILD={build}", name.to_uppercase());
    }
}
