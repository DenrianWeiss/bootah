use crate::{
    media::ImportResult,
    media_source::Image,
    model::{Profile, Settings},
};
use std::{collections::BTreeMap, path::Path};

/// Prepare the paired Solaris 10 x86 multiboot program and installation miniroot.
pub fn prepare(source: &Path, root: &Path, id: &str) -> Result<Option<ImportResult>, String> {
    if let Some(mut ufs) = crate::ufs::Image::open(source)? {
        let tree = ufs.entries()?;
        if tree.contains_key("usr/platform/sun4u/lib/fs/nfs/inetboot") {
            crate::media_source::register_ufs(source, root, &tree, "Solaris_10/Tools/Boot")?;
            let mut boot_files = BTreeMap::new();
            for platform in ["sun4u", "sun4us", "sun4v"] {
                let path = format!("usr/platform/{platform}/lib/fs/nfs/inetboot");
                if let Some(entry) = tree.get(&path) {
                    let name = format!("inetboot-{platform}");
                    std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
                    std::fs::write(root.join(&name), ufs.contents(entry, 4 * 1024 * 1024)?)
                        .map_err(|e| e.to_string())?;
                    boot_files.insert(platform.into(), name);
                }
            }
            if let Some(name) = boot_files.get("sun4u").cloned() {
                boot_files.insert("sparc64".into(), name.clone());
                boot_files.insert("default".into(), name);
            }
            return Ok(Some(ImportResult {profiles: vec![Profile { system_id: String::new(), system_name: String::new(),id:id.into(),name:"Solaris 10 SPARC installation".into(),architecture:"sparc64".into(),kind:"solaris".into(),root:root.into(),boot_files,kernel:String::new(),initrd:String::new(),dtb:String::new(),arguments:String::new(),media:source.display().to_string(),status:"prepared".into(),notes:"Original inetboot and UFS installation root indexed without unpacking the medium. sun4u is the default; select the matching sun4us/sun4v loader for those systems.".into(),boot_evidence:None}],warnings:vec!["The default loader is sun4u. Select the matching sun4us or sun4v loader for those systems.".into()]}));
        }
    }
    let image = Image::open(source)?;
    let entries = image.entries();
    let find = |name: &str| {
        entries
            .keys()
            .find(|p| p.eq_ignore_ascii_case(name))
            .cloned()
    };
    let Some(kernel) = find("platform/i86pc/multiboot") else {
        return Ok(None);
    };
    let Some(initrd) = find("boot/x86.miniroot") else {
        return Err("Solaris x86 multiboot media lacks boot/x86.miniroot; do not substitute a boot archive from another release".into());
    };
    image.materialize(&kernel, &root.join("solaris-multiboot"))?;
    image.materialize(&initrd, &root.join("solaris-miniroot"))?;
    Ok(Some(ImportResult {
        profiles: vec![Profile { system_id: String::new(), system_name: String::new(),
            id: id.into(), name: "Solaris 10 x86 installation miniroot".into(),
            architecture: "x86-bios".into(), kind: "solaris".into(), root: root.into(),
            boot_files: BTreeMap::new(), kernel: "solaris-multiboot".into(),
            initrd: "solaris-miniroot".into(), dtb: String::new(),
            arguments: "-B console=ttya".into(), media: source.display().to_string(),
            status: "needs-attention".into(),
            notes: "Solaris 10 1/06+ x86 Multiboot/miniroot candidate. Installation requires an independently configured matching NFS installation tree. Solaris 11 AI and SPARC are not handled by this adapter.".into(),
            boot_evidence: None,
        }],
        warnings: vec!["Solaris miniroot preparation only: a matching NFS installation server is still required; importing this ISO does not provide complete installation support.".into()],
    }))
}

pub fn ipxe(profile: &Profile, settings: &Settings) -> String {
    let base = format!(
        "http://{}:{}/boot/{}",
        settings.server_ip, settings.http_port, profile.id
    );
    let arguments = crate::boot::render_arguments(profile, settings);
    format!(
        "#!ipxe\nimgfree\nkernel {base}/{} {}\ninitrd {base}/{}\nboot\n",
        profile.kernel, arguments, profile.initrd
    )
}
