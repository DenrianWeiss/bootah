use crate::model::Config;
#[derive(Clone, serde::Serialize)]
pub struct Report {
    pub platform: &'static str,
    pub helper_available: bool,
    pub capture_ready: bool,
    pub reason: String,
    pub issues: Vec<String>,
}
#[cfg(target_os = "android")]
pub const ANDROID_GUIDANCE: &str = "The configured network services require Root access. In Bootah settings, stop the local core, choose Root startup, and approve access in your Root manager. If this device does not provide Root, use a desktop host for standard network boot services.";
pub fn inspect(config: &Config, running: bool) -> Report {
    let mut report = Report {
        platform: std::env::consts::OS,
        helper_available: false,
        capture_ready: false,
        reason: "platform_permissions".into(),
        issues: Vec::new(),
    };
    #[cfg(target_os = "macos")]
    {
        report.capture_ready = crate::raw::capture_environment_ready();
        let status = crate::macos_privilege::refresh_availability();
        report.helper_available = status.helper_available;
        report.reason = status.reason;
    }
    if running {
        return report;
    }
    #[cfg(target_os = "macos")]
    let helper_authorized = (report.helper_available
        && crate::macos_privilege::authorization_available())
        || crate::macos_elevation::available();
    let settings = &config.settings;
    let mut add = |message: String| {
        if !report.issues.contains(&message) {
            report.issues.push(message);
        }
    };
    if settings.ethernet_enabled {
        if let Err(error) = crate::raw::check_capture_permission(&settings.interface) {
            let lower = error.to_lowercase();
            if lower.contains("permission")
                || lower.contains("permitted")
                || lower.contains("denied")
                || lower.contains("npcap")
                || lower.contains("libpcap")
                || lower.contains("chmodbpf")
            {
                #[cfg(any(target_os = "linux", target_os = "android"))]
                add(crate::privilege::port_guidance());
                #[cfg(target_os = "windows")]
                add(error);
                #[cfg(target_os = "macos")]
                if lower.contains("libpcap") {
                    add(error);
                } else if !helper_authorized {
                    add(if report.helper_available {
                        crate::macos_privilege::GUIDANCE.into()
                    } else {
                        crate::macos_privilege::BPF_GUIDANCE.into()
                    });
                }
            } else {
                add(error);
            }
        }
    }
    let mut udp = vec![settings.tftp_port];
    if settings.dhcp_enabled || settings.proxy_dhcp_enabled {
        udp.extend([settings.dhcp_port, settings.pxe_port]);
    }
    let mut tcp = vec![settings.http_port];
    if settings.smb_mode == "smb2"
        || (settings.smb_mode == "smb1" && config.profiles.iter().any(|p| p.kind == "windows-ris"))
        || (settings.smb_mode == "auto"
            && config
                .profiles
                .iter()
                .any(|p| matches!(p.kind.as_str(), "windows" | "windows-ris")))
    {
        tcp.push(settings.ris_smb_port);
    }
    if config.profiles.iter().any(|p| {
        crate::nfs::eligible(p)
            && (config
                .devices
                .iter()
                .any(|d| d.profile_id.as_ref() == Some(&p.id))
                || config.default_profiles.values().any(|id| id == &p.id))
    }) {
        udp.push(111);
        tcp.push(111);
    }
    for port in udp {
        #[cfg(target_os = "macos")]
        if helper_authorized && matches!(port, 67 | 69 | 111 | 4011) {
            continue;
        }
        if let Err(error) = std::net::UdpSocket::bind((settings.bind_ip, port)) {
            if error.kind() == std::io::ErrorKind::AddrInUse {
                add(format!(
                    "UDP {}:{port}: Address already in use",
                    settings.bind_ip
                ));
            } else if error.kind() == std::io::ErrorKind::PermissionDenied {
                add(format!("UDP {}:{port}: {error}", settings.bind_ip));
                add(crate::privilege::port_guidance());
            }
        }
    }
    for port in tcp {
        #[cfg(target_os = "macos")]
        if helper_authorized && matches!(port, 80 | 111 | 139 | 445) {
            continue;
        }
        if let Err(error) = std::net::TcpListener::bind((settings.bind_ip, port)) {
            if error.kind() == std::io::ErrorKind::AddrInUse {
                add(format!(
                    "TCP {}:{port}: Address already in use",
                    settings.bind_ip
                ));
            } else if error.kind() == std::io::ErrorKind::PermissionDenied {
                add(format!("TCP {}:{port}: {error}", settings.bind_ip));
                add(crate::privilege::port_guidance());
            }
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn automatic_windows_source_reports_port_conflicts_without_starting_services() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let mut config = Config::default();
        config.settings.bind_ip = std::net::Ipv4Addr::LOCALHOST;
        config.settings.tftp_port = 0;
        config.settings.http_port = 0;
        config.settings.ris_smb_port = port;
        config.profiles.push(
            serde_json::from_value(serde_json::json!({
                "id":"windows", "name":"Windows", "architecture":"x64", "kind":"windows", "root":"."
            }))
            .unwrap(),
        );
        assert!(inspect(&config, false)
            .issues
            .iter()
            .any(|issue| issue.contains(&format!("TCP 127.0.0.1:{port}: Address already in use"))));
        assert!(inspect(&config, true).issues.is_empty());
        config.settings.smb_mode = "off".into();
        assert!(inspect(&config, false).issues.is_empty());
    }
}
