use regex::Regex;
use serde_json::Value;
use std::{
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
    sync::OnceLock,
};

fn sanitized(input: &str) -> String {
    let mut text = strip_secrets(input);
    static PATTERNS: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();
    for (pattern, replacement) in PATTERNS.get_or_init(|| {
        [
            (r#"(?i)\b(?:https?|ftp)://[^\s"'<>]+"#, "[URL]"),
            (r#"(?i)\b[A-Z]:\\[^\s"'<>]+"#, "[PATH]"),
            (r#"/(?:[^\s"'<>:/]+/)+[^\s"'<>:]*"#, "[PATH]"),
            (r"(?i)\b(?:[0-9a-f]{2}:){5}[0-9a-f]{2}\b", "[MAC]"),
            (r"(?i)\b(?:[0-9a-f]{2}-){5}[0-9a-f]{2}\b", "[MAC]"),
            (r"\b(?:\d{1,3}\.){3}\d{1,3}\b", "[IP]"),
            (r"(?i)\b(?:[0-9a-f]{0,4}:){2,}[0-9a-f:.%]+\b", "[IP]"),
            (r"(?i)\b[\w.+-]+@[\w.-]+\.[a-z]{2,}\b", "[EMAIL]"),
            (
                r#"(?i)\b(?:password|passwd|token|secret|apikey|api_key)\s*[:=]\s*[^\s,;"}]+"#,
                "[SECRET]",
            ),
        ]
        .into_iter()
        .map(|(pattern, replacement)| (Regex::new(pattern).unwrap(), replacement))
        .collect()
    }) {
        text = pattern.replace_all(&text, *replacement).into_owned();
    }
    text
}

fn strip_secrets(input: &str) -> String {
    static SECRETS: OnceLock<Vec<Regex>> = OnceLock::new();
    let mut text = input.to_owned();
    for regex in SECRETS.get_or_init(|| [
        r"(?i)\bBearer\s+[A-Za-z0-9._~\-]+",
        r#"(?i)["'](?:password|passwd|token|secret|apikey|api_key|api-token|BOOTAH_API_TOKEN)["']\s*:\s*["'][^"']*["']"#,
        r#"(?i)\b(?:password|passwd|token|secret|apikey|api_key|api-token|BOOTAH_API_TOKEN)\s*(?:[:=]|\s)\s*[^\s,;"}]+"#,
    ].into_iter().map(|p| Regex::new(p).unwrap()).collect()) {
        text = regex.replace_all(&text, "[SECRET]").into_owned();
    }
    text
}

fn error_event(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    [
        "error", "fail", "reject", "denied", "timeout", "invalid", "refused", "失败", "错误",
        "拒绝", "超时",
    ]
    .iter()
    .any(|word| message.contains(word))
}

pub fn export(config: &Path, level: &str, include_private: bool) -> Result<(), String> {
    if !["error", "info", "debug"].contains(&level) {
        return Err("Log level must be error, info, or debug".into());
    }
    let mut output = std::io::stdout().lock();
    writeln!(
        output,
        "Bootah diagnostics v1\nlevel={level}\nprivacy={}",
        if include_private { "raw" } else { "redacted" }
    )
    .map_err(|e| e.to_string())?;
    let current = config.with_extension("events.jsonl");
    let previous = current.with_extension("jsonl.1");
    for path in [&previous, &current] {
        if !path.is_file() {
            continue;
        }
        writeln!(
            output,
            "\n--- {} ---",
            path.file_name().unwrap_or_default().to_string_lossy()
        )
        .map_err(|e| e.to_string())?;
        let content = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        for line in content.lines() {
            let parsed: Value = match serde_json::from_str(line) {
                Ok(value) => value,
                Err(_) => continue,
            };
            let message = parsed["message"].as_str().unwrap_or("");
            if level == "error" && !error_event(message) {
                continue;
            }
            let line = if include_private {
                strip_secrets(line)
            } else {
                sanitized(line)
            };
            writeln!(output, "{line}").map_err(|e| e.to_string())?;
        }
    }
    if level != "info" {
        let path = config.with_file_name("core.log");
        if path.is_file() {
            writeln!(output, "\n--- core.log ---").map_err(|e| e.to_string())?;
            let mut file = std::fs::File::open(&path).map_err(|e| e.to_string())?;
            let length = file.metadata().map_err(|e| e.to_string())?.len();
            file.seek(SeekFrom::Start(length.saturating_sub(4 * 1024 * 1024)))
                .map_err(|e| e.to_string())?;
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
            let tail = String::from_utf8_lossy(&bytes);
            for line in tail.lines() {
                if level == "error" && !error_event(line) {
                    continue;
                }
                writeln!(
                    output,
                    "{}",
                    if include_private {
                        strip_secrets(line)
                    } else {
                        sanitized(line)
                    }
                )
                .map_err(|e| e.to_string())?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{sanitized,strip_secrets};
    #[test]
    fn removes_tokens_even_from_raw_export() {
        for input in ["Authorization: Bearer abc123_secret", "--api-token abc123_secret", "BOOTAH_API_TOKEN=abc123_secret", "{\"token\":\"abc123_secret\"}"] {
            assert!(!strip_secrets(input).contains("abc123_secret"));
        }
    }
    #[test]
    fn hides_network_ids_and_paths() {
        let text = sanitized(
            "52:54:00:12:34:56 192.168.1.2 /home/alice/test.iso C:\\Users\\alice\\test.iso https://example.com/a",
        );
        assert!(!text.contains("52:54"));
        assert!(!text.contains("192.168"));
        assert!(!text.contains("alice"));
        assert!(!text.contains("example.com"));
    }
}
