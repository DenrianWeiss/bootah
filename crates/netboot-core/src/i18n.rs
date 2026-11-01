use serde_json::Value;
use std::sync::OnceLock;

pub fn message(text: &str, locale: &str, fallback: &str) -> String {
    static CATALOG: OnceLock<Value> = OnceLock::new();
    let catalog = CATALOG
        .get_or_init(|| serde_json::from_str(include_str!("../../../i18n/terms.json")).unwrap());
    let translate = |key: &str| {
        catalog["terms"][key][locale]
            .as_str()
            .or_else(|| catalog["terms"][key][fallback].as_str())
            .unwrap_or(key)
            .to_owned()
    };
    let mut result = text.to_owned();
    let mut legacy: Vec<_> = catalog["legacyMessages"]
        .as_object()
        .unwrap()
        .iter()
        .collect();
    legacy.sort_by_key(|(old, _)| std::cmp::Reverse(old.len()));
    for (old, new) in legacy {
        result = result.replace(old, new.as_str().unwrap());
    }
    if catalog["terms"].get(&result).is_some() {
        return translate(&result);
    }
    for key in catalog["messageKeys"].as_array().unwrap() {
        let key = key.as_str().unwrap();
        result = result.replace(key, &translate(key));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn english_catalog_keys_have_both_locales_and_preserve_placeholders() {
        let catalog: Value = serde_json::from_str(include_str!("../../../i18n/terms.json")).unwrap();
        for (key, values) in catalog["terms"].as_object().unwrap() {
            for locale in ["en", "zh-CN"] {
                assert_eq!(message(key, locale, "en"), values[locale].as_str().unwrap(), "{locale}: {key}");
            }
        }
        let key = "The firmware must support `tftpdstp` for the non-standard TFTP port {port}.";
        assert_eq!(message(key, "zh-CN", "en").replace("{port}", "1069"), "使用非标准 TFTP 端口 1069 时，固件必须支持 `tftpdstp`。");
    }
    #[test]
    fn current_and_saved_diagnostics_use_one_language_and_preserve_parameters() {
        for input in [
            "EFI certificate detected",
            "EFI certificate detected / 已检测到 EFI 证书",
        ] {
            assert_eq!(message(input, "en", "en"), "EFI certificate detected");
            assert_eq!(message(input, "zh-CN", "en"), "已检测到 EFI 证书");
        }
        assert_eq!(
            message("NetBSD amd64 INSTALL ramdisk. Installation sets: present.", "zh-CN", "en"),
            "NetBSD amd64 安装内存盘。已包含安装集。"
        );
        let input = "/tmp/镜像 / boot.iso: No matching EFI certificate / 未找到匹配架构的 EFI 证书";
        assert_eq!(
            message(input, "zh-CN", "en"),
            "/tmp/镜像 / boot.iso: 未找到匹配架构的 EFI 证书"
        );
        assert_eq!(
            message(input, "en", "en"),
            "/tmp/镜像 / boot.iso: No matching EFI certificate"
        );
        assert_eq!(
            message(
                "Missing native WinPE helper / 缺少原生 WinPE 助手：bootah-native-arm64.exe",
                "en",
                "en"
            ),
            "Missing native WinPE helper: bootah-native-arm64.exe"
        );
        assert_eq!(
            message("https://example.test/a/b / 镜像.iso", "zh-CN", "en"),
            "https://example.test/a/b / 镜像.iso"
        );
    }
}
