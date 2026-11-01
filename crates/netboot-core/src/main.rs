mod aosc_compat;
mod archive;
mod automation;
mod binl;
mod boot;
mod boot_options;
mod bootparams;
mod casper_compat;
mod casper_nfs;
mod debian_compat;
mod dhcp;
mod efi;
mod fedora_compat;
mod initramfs;
mod i18n;
mod last;
mod linux_compat;
mod linux_fallback;
mod linux_media;
mod media;
mod alpha_srm;
mod sparc_obp;
mod media_source;
mod model;
mod mop;
mod native_uefi;
mod signed_ipxe;
mod secure_boot;
mod nbd;
mod netbsd;
mod freebsd;
mod openbsd;
mod network;
mod neighbors;
mod nfs;
mod nfs2;
mod ods;
mod privilege;
mod permissions;
#[cfg(target_os = "macos")]
mod macos_privilege;
#[cfg(target_os = "macos")]
mod macos_elevation;
mod diagnostics;
mod raw;
mod smb1;
mod smb2;
mod solaris;
mod system_identity;
mod tftp;
mod tui;
mod ufs;
mod uos_live;
mod web;
mod windows;
use model::*;
use std::{net::SocketAddr, path::PathBuf};

fn is_tui_entry() -> bool {
    env!("CARGO_BIN_NAME") == "netboot-tui"
}

#[tokio::main]
async fn main() {
    if let Err(e) = start().await {
        eprintln!("{e}");
        std::process::exit(1)
    }
}
async fn start() -> Result<(), String> {
    let args = std::env::args().collect::<Vec<_>>();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        let (version, build) = if is_tui_entry() { (env!("BOOTAH_TUI_VERSION"), env!("BOOTAH_TUI_BUILD")) } else { (env!("BOOTAH_CORE_VERSION"), env!("BOOTAH_CORE_BUILD")) };
        println!("{} {} ({})", env!("CARGO_BIN_NAME"), version, build);
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    if args.get(1).is_some_and(|a| a == "--macos-authorized-network") {
        return macos_elevation::run(&args[2..]);
    }
    #[cfg(target_os = "macos")]
    if args.get(1).is_some_and(|a| a == "--macos-network-daemon") {
        return macos_privilege::daemon();
    }
    #[cfg(target_os = "macos")]
    if args.get(1).is_some_and(|a| a == "--macos-network-status") {
        println!("{}",serde_json::to_string(&macos_privilege::availability()).map_err(|e|e.to_string())?);
        return Ok(());
    }
    if args.iter().any(|a| a == "--licenses") {
        print!(
            "{}\n{}\n{}\n{}",
            include_str!("../../../LICENSES/NetBSD-mopd.txt"),
            include_str!("../../../LICENSES/ods2.txt"),
            include_str!("../../../LICENSES/nfsserve.txt"),
            include_str!("../../../LICENSES/bootah-linux-media.txt")
        );
        return Ok(());
    }
    #[cfg(target_os = "linux")]
    if args.get(1).is_some_and(|v| v == "--socket-helper") {
        return privilege::helper(&args[1..]);
    }
    let value = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    #[cfg(target_os = "linux")]
    privilege::configure(&value("--authorization").unwrap_or_else(|| {
        if is_tui_entry() || args.iter().any(|v| v == "--tui") {
            "sudo".into()
        } else {
            "polkit".into()
        }
    }));
    let path = PathBuf::from(value("--config").unwrap_or("state.json".into()));
    if args.iter().any(|a| a == "--export-diagnostics") {
        let level = value("--log-level").unwrap_or_else(|| "info".into());
        return diagnostics::export(&path, &level, args.iter().any(|a| a == "--include-private"));
    }
    let state = Store::open(path)?;
    if args.iter().any(|a| a == "--init") {
        state.lock().unwrap().save()?;
        return Ok(());
    }
    let listen: SocketAddr = value("--api")
        .unwrap_or("127.0.0.1:9080".into())
        .parse::<SocketAddr>()
        .map_err(|e| e.to_string())?;
    let manager = web::Manager::new(state.clone());
    if is_tui_entry() || args.iter().any(|a| a == "--tui") {
        let language = value("--lang");
        return tui::run(manager, language.as_deref()).await;
    }
    let token = if args.iter().any(|arg| arg == "--api-token-stdin") {
        use std::io::Read;
        let mut token = Vec::new();
        let mut input = std::io::stdin().lock();
        loop {
            let mut byte = [0u8; 1];
            if input.read(&mut byte).map_err(|e| e.to_string())? == 0 {
                return Err("API token input ended before newline".into());
            }
            if byte[0] == b'\n' { break; }
            token.push(byte[0]);
            if token.len() > 512 { return Err("Invalid API token".into()); }
        }
        String::from_utf8(token).map_err(|_| "Invalid API token")?
    } else if args.iter().any(|a| a == "--api-token") {
        value("--api-token").ok_or("Missing API token")?
    } else {
        std::env::var("BOOTAH_API_TOKEN").map_err(|_| "API token required: use --api-token-stdin, --api-token, or BOOTAH_API_TOKEN")?
    };
    if !(32..=512).contains(&token.len()) || !token.bytes().all(|c| c.is_ascii_alphanumeric() || b"-_.~".contains(&c)) {
        return Err("Invalid API token: use 32–512 URL-safe characters".into());
    }
    if args.iter().any(|a| a == "--serve") {
        manager.start().await?
    }
    if let Some(address) = value("--wire") {
        let address = address.parse::<SocketAddr>().map_err(|e| e.to_string())?;
        let state = state.clone();
        tokio::spawn(async move {
            if let Err(e) = raw::wire(state, address).await {
                eprintln!("Emulator wire: {e}")
            }
        });
    }
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .map_err(|e| e.to_string())?;
    println!("Bootah management: http://{listen}");
    let exit_on_close = args.iter().any(|a| a == "--exit-on-stdin-close");
    let (closed, input_closed) = tokio::sync::oneshot::channel();
    if exit_on_close {
        std::thread::spawn(move || {
            use std::io::Read;
            let mut input = std::io::stdin().lock();
            let mut buffer = [0; 64];
            while matches!(input.read(&mut buffer), Ok(n) if n > 0) {}
            let _ = closed.send(());
        });
    }
    let shutdown = manager.clone();
    axum::serve(
        listener,
        web::router(manager, token).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = async { if exit_on_close { let _ = input_closed.await; } else { std::future::pending::<()>().await; } } => {}
        }
        shutdown.stop().await;
    })
    .await
    .map_err(|e| e.to_string())
}
