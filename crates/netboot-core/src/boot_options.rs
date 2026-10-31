use crate::model::Profile;
use std::io::Read;

fn read(p: &Profile, name: &str) -> Result<Vec<u8>, String> {
    let mut data = Vec::new();
    crate::media_source::open(&p.root, name)
        .map_err(|e| e.to_string())?
        .take(8 * 1024 * 1024 + 1)
        .read_to_end(&mut data)
        .map_err(|e| e.to_string())?;
    if data.len() > 8 * 1024 * 1024 {
        return Err("Boot configuration exceeds size limit".into());
    }
    Ok(data)
}
fn windows_source(p: &Profile) -> &'static str {
    if crate::native_uefi::enabled(p) {
        "bootah-native/bcd"
    } else {
        "BCD"
    }
}
pub fn windows(p: &Profile) -> Result<Vec<u8>, String> {
    use regf::{
        RegistryHive,
        hive::RegistryKey,
        structures::DataType,
        writer::{HiveBuilder, KeyTreeNode, KeyTreeValue},
    };
    fn tree(key: RegistryKey<'_>, depth: usize, budget: &mut usize) -> Result<KeyTreeNode, String> {
        if depth > 32 || *budget == 0 {
            return Err("BCD tree exceeds size limit".into());
        }
        *budget -= 1;
        let mut node = KeyTreeNode::new(&key.name());
        for v in key.values().map_err(|e| e.to_string())? {
            node.values.push(KeyTreeValue {
                name: v.name(),
                data_type: v.data_type(),
                data: v.raw_data().map_err(|e| e.to_string())?,
            });
        }
        for child in key.subkeys().map_err(|e| e.to_string())? {
            node.children.push(tree(child, depth + 1, budget)?);
        }
        Ok(node)
    }
    let bytes = read(p, windows_source(p))?;
    if p.arguments.is_empty() {
        return Ok(bytes);
    }
    let hive = RegistryHive::from_bytes(bytes).map_err(|e| e.to_string())?;
    let objects = hive.open_key("Objects").map_err(|e| e.to_string())?;
    let mut target = None;
    let mut options = Vec::new();
    let mut raw = Vec::new();
    for token in p.arguments.split_whitespace() {
        if let Some(value) = token.strip_prefix("entry=") {
            target = Some(value.to_string());
            continue;
        }
        let structured = token.split_once('=').and_then(|(key, value)| {
            let element = match key {
                "bootlog" => "26000090",
                "sos" => "26000091",
                "debug" => "260000a0",
                _ => return None,
            };
            let enabled = match value {
                "on" => 1,
                "off" => 0,
                _ => return None,
            };
            Some((element, enabled))
        });
        if let Some(option) = structured {
            options.push(option);
        } else {
            raw.push(token);
        }
    }
    let loaders = objects
        .subkeys()
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|key| {
            key.open_subkey("Description")
                .and_then(|k| k.value("Type"))
                .and_then(|v| v.dword_data())
                .is_ok_and(|v| v == 0x10200003)
        })
        .map(|key| key.name())
        .collect::<Vec<_>>();
    let default = objects
        .open_subkey("{9dea862c-5cdd-4e70-acc1-f32b344d4795}")
        .and_then(|key| key.open_subkey("Elements"))
        .and_then(|key| key.open_subkey("23000003"))
        .and_then(|key| key.value("Element"))
        .and_then(|value| value.raw_data())
        .ok()
        .and_then(|bytes| {
            if let Ok(bytes) = <[u8; 16]>::try_from(bytes.as_slice()) {
                Some(format!("{{{}}}", uuid::Uuid::from_bytes_le(bytes)))
            } else {
                String::from_utf16(
                    &bytes
                        .chunks_exact(2)
                        .map(|p| u16::from_le_bytes([p[0], p[1]]))
                        .take_while(|v| *v != 0)
                        .collect::<Vec<_>>(),
                )
                .ok()
            }
        });
    let target = target.or_else(|| {
        default.filter(|id| loaders.iter().any(|loader| loader.eq_ignore_ascii_case(id)))
    });
    let target = match target {
        Some(id) if loaders.iter().any(|s| s.eq_ignore_ascii_case(&id)) => loaders
            .iter()
            .find(|s| s.eq_ignore_ascii_case(&id))
            .unwrap()
            .clone(),
        None if loaders.len() == 1 => loaders[0].clone(),
        _ => return Err("Select a Windows OS loader using entry={GUID}".into()),
    };
    let mut root = tree(hive.root_key().map_err(|e| e.to_string())?, 0, &mut 16384)?;
    for (element, enabled) in options {
        let key = root.get_or_create_path(&format!("Objects\\{target}\\Elements\\{element}"));
        key.values
            .retain(|v| !v.name.eq_ignore_ascii_case("Element"));
        key.values.push(KeyTreeValue {
            name: "Element".into(),
            data_type: DataType::Binary,
            data: vec![enabled],
        });
    }
    if !raw.is_empty() {
        let key = root.get_or_create_path(&format!("Objects\\{target}\\Elements\\12000030"));
        key.values
            .retain(|v| !v.name.eq_ignore_ascii_case("Element"));
        let value = if raw.len() == p.arguments.split_whitespace().count() {
            p.arguments.clone()
        } else {
            raw.join(" ")
        };
        let data = value
            .encode_utf16()
            .chain(std::iter::once(0))
            .flat_map(u16::to_le_bytes)
            .collect();
        key.values.push(KeyTreeValue {
            name: "Element".into(),
            data_type: DataType::Binary,
            data,
        });
    }
    let (major, minor) = hive.version();
    HiveBuilder::from_tree_with_version(root, major, minor)
        .build()
        .map_err(|e| e.to_string())
}
fn words(input: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut start = None;
    let mut quote = None;
    let mut escaped = false;
    for (at, ch) in input.char_indices() {
        if start.is_none() && !ch.is_whitespace() {
            start = Some(at);
        }
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            }
            continue;
        }
        if ch == '\'' || ch == '"' {
            quote = Some(ch);
            continue;
        }
        if ch.is_whitespace() {
            if let Some(from) = start.take() {
                result.push(&input[from..at]);
            }
        }
    }
    if let Some(from) = start {
        result.push(&input[from..]);
    }
    result
}
fn freebsd_configuration(p: &Profile, previous: &[u8]) -> Result<Vec<u8>, String> {
    let marker = b"\n# Bootah user boot flags\n";
    let end = previous
        .windows(marker.len())
        .position(|w| w == marker)
        .unwrap_or(previous.len());
    let mut data = previous[..end].to_vec();
    if !p.arguments.is_empty() {
        data.extend_from_slice(marker);
        let quote = |value: &str| {
            value
                .replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace(['\r', '\n'], " ")
        };
        let mut flags = Vec::new();
        let mut variables = Vec::new();
        for word in words(&p.arguments) {
            if let Some((key, value)) = word.split_once('=') {
                variables.push((key, value));
            } else {
                flags.push(word);
            }
        }
        if !flags.is_empty() {
            data.extend_from_slice(
                format!("kernel_options=\"{}\"\n", quote(&flags.join(" "))).as_bytes(),
            );
        }
        for (key, value) in variables {
            let value = if value.starts_with('"') && value.ends_with('"') && value.len() > 1 {
                value.to_string()
            } else {
                format!("\"{}\"", quote(value))
            };
            data.extend_from_slice(format!("{key}={value}\n").as_bytes());
        }
    }
    Ok(data)
}

pub fn remember_defaults(profiles: &[Profile]) -> Result<(), String> {
    for p in profiles {
        let path = p.root.join(format!(".bootah-arguments-{}.json", p.id));
        if !path.exists() {
            replace(
                &path,
                &serde_json::to_vec(&p.arguments).map_err(|e| e.to_string())?,
            )?;
        }
    }
    Ok(())
}
pub fn editable(p: &Profile) -> bool {
    !(p.kind == "windows-pe-iso" && !crate::native_uefi::enabled(p)
        || p.kind == "bsd" && crate::secure_boot::enabled(p))
}
pub fn parameter_info(
    p: &Profile,
    settings: &crate::model::Settings,
) -> Result<serde_json::Value, String> {
    let original = std::fs::read(p.root.join(format!(".bootah-arguments-{}.json", p.id)))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<String>(&bytes).ok());
    let applies = editable(p);
    if !applies {
        return Ok(
            serde_json::json!({"original_arguments":original,"editable":false,"applies":false,"preview":""}),
        );
    }
    let preview = match p.kind.as_str() {
        "linux" => crate::boot::render_arguments(p, settings),
        "windows-ris" => String::from_utf8_lossy(&crate::windows::ris_answer(
            p,
            &settings.server_ip.to_string(),
            &[],
        ))
        .into_owned(),
        "windows" | "windows-pe-iso" if applies => match windows(p) {
            Ok(bytes) => format!("BCD ({} bytes)\n{}", bytes.len(), p.arguments),
            Err(error) => error,
        },
        "bsd" if p.system_id == "freebsd" && p.root.join(".bootah-freebsd-nfs.json").exists() => {
            let previous =
                std::fs::read(p.root.join("boot/loader.conf.local")).map_err(|e| e.to_string())?;
            String::from_utf8(freebsd_configuration(p, &previous)?).map_err(|e| e.to_string())?
        }
        "bsd" if p.kernel.starts_with("netbsd") => String::from_utf8(
            crate::boot::virtual_file(p, "boot.cfg", settings, None).unwrap_or_default(),
        )
        .map_err(|e| e.to_string())?,
        "bsd" if p.kernel.ends_with("bsd.rd") => String::from_utf8(
            crate::boot::virtual_file(p, "etc/boot.conf", settings, None).unwrap_or_default(),
        )
        .map_err(|e| e.to_string())?,
        _ => p.arguments.clone(),
    };
    Ok(
        serde_json::json!({"original_arguments":original,"editable":applies,"applies":applies,"preview":preview}),
    )
}

pub struct FreeBsdEdit {
    path: std::path::PathBuf,
    previous: Vec<u8>,
    committed: bool,
}
impl FreeBsdEdit {
    pub fn stage(p: &Profile) -> Result<Option<Self>, String> {
        if p.system_id != "freebsd" || !p.root.join(".bootah-freebsd-nfs.json").exists() {
            return Ok(None);
        }
        let path = p.root.join("boot/loader.conf.local");
        let previous = std::fs::read(&path).map_err(|e| e.to_string())?;
        let data = freebsd_configuration(p, &previous)?;
        if data == previous {
            return Ok(None);
        }
        replace(&path, &data)?;
        Ok(Some(Self {
            path,
            previous,
            committed: false,
        }))
    }
    pub fn commit(&mut self) {
        self.committed = true;
    }
}
fn replace(path: &std::path::Path, data: &[u8]) -> Result<(), String> {
    let temp = path.with_extension(format!("bootah-{}", uuid::Uuid::new_v4()));
    let result = std::fs::write(&temp, data).and_then(|_| std::fs::rename(&temp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(temp);
    }
    result.map_err(|e| e.to_string())
}
impl Drop for FreeBsdEdit {
    fn drop(&mut self) {
        if !self.committed {
            let _ = replace(&self.path, &self.previous);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bcd_overrides_preserve_devices_and_original_bytes() {
        use regf::{RegistryHive, structures::DataType, writer::HiveBuilder};
        let root = std::env::temp_dir().join(format!("bootah-bcd-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let mut builder = HiveBuilder::new();
        let entry = "{12345678-1234-1234-1234-123456789abc}";
        let desc = builder
            .create_key(&format!("Objects\\{entry}\\Description"))
            .unwrap();
        builder
            .add_value(desc, "Type", DataType::Dword, &0x10200003u32.to_le_bytes())
            .unwrap();
        let device = builder
            .create_key(&format!("Objects\\{entry}\\Elements\\21000001"))
            .unwrap();
        builder
            .add_value(
                device,
                "Element",
                DataType::Binary,
                b"original ramdisk device",
            )
            .unwrap();
        let other = builder
            .create_key("Objects\\{22222222-2222-2222-2222-222222222222}\\Description")
            .unwrap();
        builder
            .add_value(other, "Type", DataType::Dword, &0x10200003u32.to_le_bytes())
            .unwrap();
        let default = builder
            .create_key("Objects\\{9dea862c-5cdd-4e70-acc1-f32b344d4795}\\Elements\\23000003")
            .unwrap();
        let id = entry
            .encode_utf16()
            .chain(std::iter::once(0))
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();
        builder
            .add_value(default, "Element", DataType::Binary, &id)
            .unwrap();
        let original = builder.build().unwrap();
        std::fs::write(root.join("BCD"), &original).unwrap();
        let mut p:Profile=serde_json::from_value(serde_json::json!({"id":"p","name":"p","root":root,"kind":"windows","architecture":"x64","arguments":"bootlog=on sos=off"})).unwrap();
        remember_defaults(&[p.clone()]).unwrap();
        let mut changed = p.clone();
        changed.arguments = "sos=on".into();
        remember_defaults(&[changed.clone()]).unwrap();
        let info = parameter_info(&changed, &crate::model::Settings::default()).unwrap();
        assert_eq!(info["original_arguments"], p.arguments);
        assert!(info["preview"].as_str().unwrap().contains("sos=on"));
        let bytes = windows(&p).unwrap();
        let hive = RegistryHive::from_bytes(bytes).unwrap();
        assert_eq!(
            hive.open_key(&format!("Objects\\{entry}\\Elements\\26000090"))
                .unwrap()
                .value("Element")
                .unwrap()
                .raw_data()
                .unwrap(),
            [1]
        );
        assert_eq!(
            hive.open_key(&format!("Objects\\{entry}\\Elements\\21000001"))
                .unwrap()
                .value("Element")
                .unwrap()
                .raw_data()
                .unwrap(),
            b"original ramdisk device"
        );
        assert_eq!(std::fs::read(root.join("BCD")).unwrap(), original);
        p.arguments = "UNKNOWN_OPTION /USER_FLAG nointegritychecks=on".into();
        let changed = RegistryHive::from_bytes(windows(&p).unwrap()).unwrap();
        let raw = changed
            .open_key(&format!("Objects\\{entry}\\Elements\\12000030"))
            .unwrap()
            .value("Element")
            .unwrap()
            .raw_data()
            .unwrap();
        let raw = String::from_utf16(
            &raw.chunks_exact(2)
                .map(|p| u16::from_le_bytes([p[0], p[1]]))
                .collect::<Vec<_>>(),
        )
        .unwrap();
        assert!(raw.contains("UNKNOWN_OPTION /USER_FLAG nointegritychecks=on"));
        p.kind = "windows-pe-iso".into();
        assert_eq!(
            parameter_info(&p, &crate::model::Settings::default()).unwrap()["editable"],
            false
        );
        std::fs::create_dir_all(root.join("bootah-native")).unwrap();
        std::fs::write(root.join("bootah-native/bcd"), &original).unwrap();
        p.boot_files
            .insert("x64-efi".into(), "bootah-native/bootx64.efi".into());
        assert!(editable(&p));
        let bytes =
            crate::boot::virtual_file(&p, "Boot/BCD", &crate::model::Settings::default(), None)
                .unwrap();
        let hive = RegistryHive::from_bytes(bytes).unwrap();
        assert!(
            hive.open_key(&format!("Objects\\{entry}\\Elements\\12000030"))
                .is_ok()
        );
        p.kind = "windows-ris".into();
        assert!(editable(&p));
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn freebsd_failed_save_restores_loader_configuration() {
        let root = std::env::temp_dir().join(format!("bootah-loader-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("boot")).unwrap();
        std::fs::write(root.join(".bootah-freebsd-nfs.json"), b"{}").unwrap();
        let base = b"mdroot_load=\"YES\"\n";
        std::fs::write(root.join("boot/loader.conf.local"), base).unwrap();
        let mut p:Profile=serde_json::from_value(serde_json::json!({"id":"p","name":"p","root":root,"kind":"bsd","system_id":"freebsd","architecture":"x64","arguments":"-sv kern.hz=100"})).unwrap();
        {
            let _edit = FreeBsdEdit::stage(&p).unwrap().unwrap();
            assert!(
                String::from_utf8(std::fs::read(root.join("boot/loader.conf.local")).unwrap())
                    .unwrap()
                    .contains("kernel_options=\"-sv\"")
            );
        }
        assert_eq!(
            std::fs::read(root.join("boot/loader.conf.local")).unwrap(),
            base
        );
        let mut edit = FreeBsdEdit::stage(&p).unwrap().unwrap();
        edit.commit();
        drop(edit);
        p.arguments = "-v".into();
        let mut edit = FreeBsdEdit::stage(&p).unwrap().unwrap();
        edit.commit();
        drop(edit);
        assert!(
            !String::from_utf8(std::fs::read(root.join("boot/loader.conf.local")).unwrap())
                .unwrap()
                .contains("boot_single=")
        );
        p.arguments = "-UNKNOWN vfs.root.mountfrom=custom".into();
        let mut edit = FreeBsdEdit::stage(&p).unwrap().unwrap();
        edit.commit();
        drop(edit);
        let text =
            String::from_utf8(std::fs::read(root.join("boot/loader.conf.local")).unwrap()).unwrap();
        assert!(text.contains("vfs.root.mountfrom=\"custom\""));
        assert!(text.contains("-UNKNOWN"));
        p.arguments.clear();
        let mut edit = FreeBsdEdit::stage(&p).unwrap().unwrap();
        edit.commit();
        drop(edit);
        assert_eq!(
            std::fs::read(root.join("boot/loader.conf.local")).unwrap(),
            base
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
