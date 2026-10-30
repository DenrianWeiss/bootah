use crate::{automation, boot, dhcp, media, model::*, network, raw, tftp};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{ConnectInfo, Path, Query, State},
    http::{HeaderMap, Method, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
};
use serde::Deserialize;
use std::{
    net::{IpAddr, SocketAddr},
    sync::{Arc, Mutex},
};
use tokio::task::JoinHandle;
type ApiError = (StatusCode, String);
fn bad(s: impl Into<String>) -> ApiError {
    (StatusCode::BAD_REQUEST, s.into())
}
#[derive(Default)]
struct MediaServices {
    nfs: Vec<JoinHandle<()>>,
    exports: Vec<JoinHandle<()>>,
    backends: crate::nfs::Backends,
    nbd: Vec<JoinHandle<()>>,
    bootparams: Vec<JoinHandle<()>>,
}
impl Drop for MediaServices {
    fn drop(&mut self) {
        for task in self.nfs.iter().chain(&self.exports).chain(&self.nbd).chain(&self.bootparams) {
            task.abort();
        }
    }
}
#[derive(Clone)]
pub struct Manager {
    pub state: Shared,
    tasks: Arc<Mutex<Vec<JoinHandle<()>>>>,
    pub network: Arc<Mutex<network::Report>>,
    lifecycle: Arc<tokio::sync::Mutex<()>>,
    smb_tasks: Arc<Mutex<Vec<JoinHandle<()>>>>,
    smb_active: Arc<Mutex<String>>,
    smb_credentials: crate::smb2::Credentials,
    media_services: Arc<tokio::sync::Mutex<MediaServices>>,
    raw_stop: Arc<std::sync::atomic::AtomicBool>,
}
impl Manager {
    pub fn new(state: Shared) -> Self {
        let weak = Arc::downgrade(&state);
        tokio::spawn(async move {
            loop {
                let Some(state) = weak.upgrade() else {
                    break;
                };
                let interface = state.lock().unwrap().config.settings.interface.clone();
                if let Ok(neighbors) =
                    tokio::task::spawn_blocking(move || crate::neighbors::lookup(&interface)).await
                {
                    let mut store = state.lock().unwrap();
                    let mut changed = false;
                    let macs: std::collections::BTreeSet<_> = store
                        .config
                        .devices
                        .iter()
                        .filter(|d| !d.mac.is_empty())
                        .map(|d| d.mac.clone())
                        .collect();
                    let ip_only: std::collections::BTreeSet<_> = store
                        .config
                        .devices
                        .iter()
                        .filter(|d| d.mac.is_empty())
                        .filter_map(|d| d.ip)
                        .collect();
                    for (ip, mac) in &neighbors {
                        if !macs.contains(mac) && !ip_only.contains(ip) {
                            continue;
                        }

                        let current = store
                            .config
                            .devices
                            .iter()
                            .find(|d| d.mac == *mac)
                            .and_then(|d| d.ip);
                        if current == Some(*ip) && !ip_only.contains(ip) {
                            continue;
                        }
                        if current.is_some_and(|old| old != *ip && neighbors.get(&old) == Some(mac))
                            && !store
                                .config
                                .devices
                                .iter()
                                .any(|d| d.ip == Some(*ip) && d.mac.is_empty())
                        {
                            continue;
                        }
                        changed |= store.associate(*ip, mac);
                    }
                    if changed {
                        if let Err(error) = store.save() {
                            store.event("network", "storage", error);
                        }
                    }
                }
                drop(state);
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
        });
        Self {
            state,
            network: Arc::new(Mutex::new(network::Report::default())),
            tasks: Arc::new(Mutex::new(vec![])),
            smb_tasks: Arc::new(Mutex::new(vec![])),
            smb_active: Arc::new(Mutex::new("off".into())),
            smb_credentials: Arc::new(std::sync::RwLock::new(vec![])),
            lifecycle: Arc::new(tokio::sync::Mutex::new(())),
            media_services: Arc::new(tokio::sync::Mutex::new(MediaServices::default())),
            raw_stop: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }
    pub async fn check_permissions(&self) -> Result<crate::permissions::Report, String> {
        let _guard = self.lifecycle.lock().await;
        let config = self.state.lock().unwrap().config.clone();
        let running = self.is_running();
        tokio::task::spawn_blocking(move || crate::permissions::inspect(&config, running))
            .await
            .map_err(|e| e.to_string())
    }
    pub async fn detect_network(
        &self,
        request: network::Detection,
    ) -> Result<network::Report, String> {
        let _guard = self.lifecycle.lock().await;
        self.detect_network_inner(request).await
    }
    async fn detect_network_inner(
        &self,
        request: network::Detection,
    ) -> Result<network::Report, String> {
        if self.is_running() {
            return Err("Stop services before detecting network".into());
        }
        let mut settings = self.state.lock().unwrap().config.settings.clone();
        if let Some(interface) = &request.interface {
            settings.interface = interface.clone();
        }
        if let Some(address) = request.address {
            settings.server_ip = address;
        }
        #[cfg(target_os = "android")]
        if let Some(evidence) = &request.android {
            settings.interface = evidence.id.clone();
        }
        let explicit_selection = request.interface.is_some() || request.address.is_some();
        let scan_settings = settings.clone();
        let mut report =
            tokio::task::spawn_blocking(move || network::scan(&scan_settings, &request))
                .await
                .map_err(|e| e.to_string())??;
        let previous_report = self.network.lock().unwrap().clone();
        if report.status == "probe_unavailable"
            && previous_report.status == "response_found"
            && previous_report.selected == report.selected
            && previous_report.address == report.address
            && now().saturating_sub(previous_report.checked_at) < 30
            && previous_report
                .interfaces
                .iter()
                .find(|i| i.id == report.selected)
                .map(|i| &i.mac)
                == report
                    .interfaces
                    .iter()
                    .find(|i| i.id == report.selected)
                    .map(|i| &i.mac)
        {
            report.servers = previous_report.servers;
            report.status = previous_report.status;
            report.source = previous_report.source;
            report.checked_at = previous_report.checked_at;
            report.detail.clear();
        }
        if explicit_selection {
            network::configure_interface(&mut settings, &report);
        }
        network::apply(&mut settings, &report);
        let mut store = self.state.lock().unwrap();
        let old = std::mem::replace(&mut store.config.settings, settings);
        if let Err(e) = store.save() {
            store.config.settings = old;
            return Err(e);
        }
        *self.network.lock().unwrap() = report.clone();
        Ok(report)
    }
    pub async fn switch_smb(
        &self,
        mode: String,
        confirm: bool,
        expected_mode: Option<String>,
    ) -> Result<serde_json::Value, (StatusCode, serde_json::Value)> {
        let request = SmbSwitch {
            mode,
            confirm,
            expected_mode,
        };
        if !matches!(request.mode.as_str(), "off" | "auto" | "smb1" | "smb2") {
            return Err((
                StatusCode::BAD_REQUEST,
                serde_json::json!({"message":"Invalid SMB mode"}),
            ));
        }
        let _guard = self.lifecycle.lock().await;
        let current = self.smb_active.lock().unwrap().clone();
        if (request.confirm && request.expected_mode.as_deref() != Some(&current))
            || (current != "off" && current != request.mode && !request.confirm)
        {
            return Err((
                StatusCode::CONFLICT,
                serde_json::json!({
                    "code":"smb_switch_confirmation","current_mode":current,"requested_mode":request.mode,
                    "message":"Stopping the current SMB service may interrupt installations. Stop it and switch?",
                    "message_zh":"停止当前 SMB 服务可能中断安装，是否停止并切换？"
                }),
            ));
        }
        let previous = self.state.lock().unwrap().config.settings.clone();
        let mut settings = previous.clone();
        settings.smb_mode = request.mode.clone();
        if current != request.mode {
            self.stop_smb_inner().await;
            if self.is_running()
                && let Err(error) = self.start_smb_inner(&request.mode, &settings).await
            {
                let restore_error = self.start_smb_inner(&current, &previous).await.err();
                return Err((
                    StatusCode::CONFLICT,
                    serde_json::json!({"code":"smb_start_failed","message":error,"restore_error":restore_error,"state":self.smb_status_value()}),
                ));
            }
        }
        let saved = {
            let mut store = self.state.lock().unwrap();
            store.config.settings.smb_mode = request.mode.clone();
            let saved = store.save();
            if saved.is_err() { store.config.settings.smb_mode = previous.smb_mode.clone(); }
            saved
        };
        if let Err(error) = saved {
            if current != request.mode {
                self.stop_smb_inner().await;
                if self.is_running() { let _ = self.start_smb_inner(&current, &previous).await; }
            }
            return Err((StatusCode::BAD_REQUEST, serde_json::json!({"message":error,"state":self.smb_status_value()})));
        }
        self.state.lock().unwrap().event(
            "server",
            "SMB",
            format!("Selected {} installation source service", request.mode),
        );
        Ok(self.smb_status_value())
    }
    pub fn smb_status_value(&self) -> serde_json::Value {
        let settings = self.state.lock().unwrap().config.settings.clone();
        serde_json::json!({"mode": *self.smb_active.lock().unwrap(), "configured_mode":settings.smb_mode,"bind_ip":settings.bind_ip,"port":settings.ris_smb_port})
    }
    async fn stop_smb_inner(&self) {
        let handles = std::mem::take(&mut *self.smb_tasks.lock().unwrap());
        for handle in &handles {
            handle.abort();
        }
        for handle in handles {
            let _ = handle.await;
        }
        self.smb_credentials.write().unwrap().clear();
        *self.smb_active.lock().unwrap() = "off".into();
    }
    async fn start_smb_inner(&self, mode: &str, settings: &Settings) -> Result<(), String> {
        let config = self.state.lock().unwrap().config.clone();
        let handles = match mode {
            "off" => vec![],
            "smb1" => {
                crate::smb1::start(self.state.clone(), settings.bind_ip, settings.ris_smb_port,
                    &config)
                    .await?
            }
            "auto" if config.profiles.iter().any(|p| matches!(p.kind.as_str(), "windows" | "windows-ris")) => crate::smb2::start_mode(self.state.clone(), settings.bind_ip, settings.ris_smb_port, self.smb_credentials.clone(), true).await?,
            "auto" => vec![],
            "smb2" => {
                crate::smb2::start(
                    self.state.clone(),
                    settings.bind_ip,
                    settings.ris_smb_port,
                    self.smb_credentials.clone(),
                )
                .await?
            }
            _ => return Err("Invalid SMB mode".into()),
        };
        *self.smb_active.lock().unwrap() = if handles.is_empty() { "off" } else { mode }.into();
        *self.smb_tasks.lock().unwrap() = handles;
        Ok(())
    }
    fn smb_credential(
        &self,
        profile_id: &str,
        device_id: &str,
    ) -> Result<crate::smb2::Credential, String> {
        let mut credentials = self.smb_credentials.write().unwrap();

        if let Some(existing) = credentials.iter().find(|c| {
            c.profile_id == profile_id && c.device_id == device_id && c.expires_at > now()
        }) {
            return Ok(existing.clone());
        }
        if credentials.len() >= 4096 {
            return Err("SMB installation credential capacity reached; finish active installations before restarting the SMB service.".into());
        }
        let credential = crate::smb2::Credential {
            username: format!("bootah_{}", uuid::Uuid::new_v4().simple()),
            password: uuid::Uuid::new_v4().simple().to_string(),
            profile_id: profile_id.into(),
            device_id: device_id.into(),
            expires_at: now() + 24 * 60 * 60,
        };
        credentials.push(credential.clone());
        Ok(credential)
    }
    async fn prepare_media(&self, config: &Config) -> Result<(), String> {
        let native_config = config.clone();
        tokio::task::spawn_blocking(move || {
            for profile in &native_config.profiles {
                if crate::native_uefi::enabled(profile) { crate::secure_boot::validate_current(profile)?; }
                if crate::native_uefi::enabled(profile) && profile.kind == "windows" {
                    crate::native_uefi::prepare_windows_wim(profile, &native_config.settings)?;
                }
            }
            Ok::<_, String>(())
        })
        .await
        .map_err(|e| e.to_string())??;
        let mut services = self.media_services.lock().await;
        let lengths = (services.nfs.len(), services.exports.len(), services.nbd.len(), services.bootparams.len());
        let previous_backends = services.backends.lock().unwrap().clone();
        let result: Result<(), String> = async {
            if crate::nfs::configured_port_for(config)?.is_some() {
                if services.nfs.is_empty() {
                    services.nfs = crate::nfs::start(self.state.clone(), config, services.backends.clone()).await?;
                }
                let exports = crate::nfs::prepare_exports(config, &services.backends).await?;
                services.exports.extend(exports);
            }
            if services.bootparams.is_empty() {
                services.bootparams = crate::bootparams::start(self.state.clone(), config).await?;
            }
            if services.nbd.is_empty() {
                services.nbd = crate::nbd::start(self.state.clone(), config).await?;
            }
            if self.is_running() && matches!(config.settings.smb_mode.as_str(), "smb1" | "auto")
                && self.smb_active.lock().unwrap().as_str() == "off"
                && config.profiles.iter().any(|p| p.kind == "windows-ris" || (config.settings.smb_mode == "auto" && p.kind == "windows"))
            {
                let handles = if config.settings.smb_mode == "auto" { crate::smb2::start_mode(self.state.clone(), config.settings.bind_ip, config.settings.ris_smb_port, self.smb_credentials.clone(), true).await? } else { crate::smb1::start(self.state.clone(), config.settings.bind_ip, config.settings.ris_smb_port, config).await? };
                *self.smb_tasks.lock().unwrap() = handles;
                *self.smb_active.lock().unwrap() = config.settings.smb_mode.clone();
            }
            Ok(())
        }.await;
        if result.is_err() {
            let mut added = services.nfs.split_off(lengths.0);
            added.extend(services.exports.split_off(lengths.1));
            added.extend(services.nbd.split_off(lengths.2));
            added.extend(services.bootparams.split_off(lengths.3));
            for task in &added { task.abort(); }
            for task in added { let _ = task.await; }
            *services.backends.lock().unwrap() = previous_backends;
        }
        if result.is_ok() && lengths != (services.nfs.len(), services.exports.len(), services.nbd.len(), services.bootparams.len()) {
            self.state.lock().unwrap().event("server", "service", "Prepared required media services and exports");
        }
        result.map_err(|error| format!("Cannot prepare boot services; selection unchanged: {error}"))
    }
    pub fn is_running(&self) -> bool {
        !self.tasks.lock().unwrap().is_empty()
    }
    async fn stop_inner(&self) {
        self.stop_smb_inner().await;
        let mut media = self.media_services.lock().await;
        let mut handles = std::mem::take(&mut media.nfs);
        handles.append(&mut media.exports);
        handles.append(&mut media.nbd);
        handles.append(&mut media.bootparams);
        media.backends.lock().unwrap().clear();
        for handle in &handles { handle.abort(); }
        for handle in handles { let _ = handle.await; }
        drop(media);
        self.raw_stop
            .store(true, std::sync::atomic::Ordering::Relaxed);
        let tasks = std::mem::take(&mut *self.tasks.lock().unwrap());
        for task in &tasks {
            task.abort()
        }
        for task in tasks {
            let _ = task.await;
        }
        self.state
            .lock()
            .unwrap()
            .event("server", "service", "Stopped");
    }
    pub async fn stop(&self) {
        let _guard = self.lifecycle.lock().await;
        self.stop_inner().await;
    }
    pub async fn start(&self) -> Result<(), String> {
        let _guard = self.lifecycle.lock().await;
        let automatic = self.state.lock().unwrap().config.settings.dhcp_auto;
        if automatic {
            #[cfg(not(target_os = "android"))]
            let report = self
                .detect_network_inner(network::Detection {
                    authorize: cfg!(target_os = "macos"),
                    ..Default::default()
                })
                .await?;
            #[cfg(target_os = "android")]
            let report = self.network.lock().unwrap().clone();
            if !matches!(report.status.as_str(), "lease_found" | "response_found")
                || report.address.is_none()
                || now().saturating_sub(report.checked_at) > 30
            {
                if report.status == "probe_unavailable" && !report.detail.is_empty() {
                    return Err(report.detail);
                }
                return Err("Automatic DHCP mode needs a selected network and DHCP evidence; detect again or select manual mode".into());
            }
        }
        self.stop_inner().await;
        let s = self.state.lock().unwrap().config.settings.clone();
        if s.server_ip.is_unspecified() || s.server_ip.is_broadcast() {
            return Err("Set a server IPv4 address".into());
        }
        if s.dhcp_enabled && s.proxy_dhcp_enabled {
            return Err("DHCP address allocation and ProxyDHCP are mutually exclusive".into());
        }
        if s.pxe_port == 0 || s.pxe_port == s.dhcp_port || s.pxe_port == s.tftp_port {
            return Err("Invalid PXE port".into());
        }
        let http = crate::privilege::tcp((s.bind_ip, s.http_port))
            .await
            .map_err(|e| format!("HTTP: {e}"))?;
        let tftp_socket = crate::privilege::udp((s.bind_ip, s.tftp_port))
            .await
            .map_err(|e| format!("TFTP: {e}"))?;
        let dhcp_socket = if s.dhcp_enabled || s.proxy_dhcp_enabled {
            let socket = crate::privilege::udp((s.bind_ip, s.dhcp_port))
                .await
                .map_err(|e| format!("DHCP: {e}"))?;
            socket.set_broadcast(true).map_err(|e| e.to_string())?;
            Some(socket)
        } else {
            None
        };
        let pxe_socket = if s.dhcp_enabled || s.proxy_dhcp_enabled {
            let socket = crate::privilege::udp((s.bind_ip, s.pxe_port))
                .await
                .map_err(|e| format!("PXE: {e}"))?;
            socket.set_broadcast(true).map_err(|e| e.to_string())?;
            Some(socket)
        } else {
            None
        };
        let raw_task = if s.ethernet_enabled {
            self.raw_stop
                .store(false, std::sync::atomic::Ordering::Relaxed);
            Some(raw::start(self.state.clone(), self.raw_stop.clone()).await?)
        } else {
            None
        };
        let state = self.state.clone();
        let t = tokio::spawn(async move {
            if let Err(e) = tftp::run(state.clone(), tftp_socket).await {
                state.lock().unwrap().event("server", "TFTP", e)
            }
        });
        let mut tasks = vec![t];
        let config = self.state.lock().unwrap().config.clone();
        if let Err(error) = self.prepare_media(&config).await {
            for task in tasks { task.abort(); }
            if let Some(task) = raw_task { task.abort(); }
            self.raw_stop.store(true, std::sync::atomic::Ordering::Relaxed);
            *self.media_services.lock().await = MediaServices::default();
            return Err(error);
        }
        if let Some(task) = raw_task { tasks.push(task); }
        match self.start_smb_inner(&s.smb_mode, &s).await {
            Ok(()) => {}
            Err(error) => {
                *self.media_services.lock().await = MediaServices::default();
                for task in tasks {
                    task.abort();
                }
                self.raw_stop
                    .store(true, std::sync::atomic::Ordering::Relaxed);
                return Err(error);
            }
        }
        if let Some(socket) = dhcp_socket {
            let state = self.state.clone();
            tasks.push(tokio::spawn(async move {
                if let Err(e) = dhcp::run(state.clone(), socket, false).await {
                    state.lock().unwrap().event("server", "DHCP", e)
                }
            }));
        }
        if let Some(socket) = pxe_socket {
            let state = self.state.clone();
            tasks.push(tokio::spawn(async move {
                if let Err(e) = dhcp::run(state.clone(), socket, true).await {
                    state.lock().unwrap().event("server", "PXE", e)
                }
            }));
        }
        let app = Router::new()
            .route("/boot/{id}/{*file}", get(boot_file))
            .route("/auto/{token}/{name}", get(automation_file))
            .with_state(self.clone());
        tasks.push(tokio::spawn(async move {
            let _ = axum::serve(
                http,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await;
        }));
        *self.tasks.lock().unwrap() = tasks;
        self.state
            .lock()
            .unwrap()
            .event("server", "service", "Started");
        Ok(())
    }
}
pub fn router(manager: Manager, token: String) -> Router {
    Router::new()
        .route("/api/state", get(snapshot))
        .route("/api/events", delete(clear_events))
        .route("/api/settings", put(settings))
        .route("/api/smb", get(smb_status).post(smb_switch))
        .route("/api/network/detect", post(detect_network))
        .route("/api/network/permissions", get(network_permissions))
        .route("/api/default-profiles", put(default_profiles))
        .route("/api/devices", post(device))
        .route("/api/devices/{id}", delete(remove_device))
        .route("/api/device-conflicts/{id}", post(resolve_device_conflict))
        .route("/api/profiles", post(profile))
        .route("/api/media/import", post(import_media))
        .route(
            "/api/automation/{scope}/{id}",
            put(set_automation).delete(clear_automation),
        )
        .route(
            "/api/profiles/{id}/automation-methods",
            get(automation_methods),
        )
        .route("/api/profiles/{id}", delete(remove_profile))
        .route("/api/profiles/{id}/compatibility", put(profile_compatibility))
        .route("/api/profiles/{id}/secure-boot", get(secure_boot_status).put(secure_boot_switch))
        .route("/api/profiles/{id}/secure-boot/check", post(secure_boot_check))
        .route("/api/profiles/{id}/boot-commands", get(boot_commands))
        .route("/api/profiles/{id}/uboot", get(uboot))
        .route("/api/service/start", post(start))
        .route("/api/service/stop", post(stop))
        .layer(axum::middleware::from_fn_with_state(Arc::new(token), api_auth))
        .route("/boot/{id}/{*file}", get(boot_file))
        .route("/auto/{token}/{name}", get(automation_file))
        .layer(axum::middleware::from_fn(management_guard))
        .with_state(manager)
}
async fn api_auth(
    State(token): State<Arc<String>>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    use subtle::ConstantTimeEq;
    let supplied = request.headers().get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split_once(' '))
        .and_then(|(scheme, token)| scheme.eq_ignore_ascii_case("Bearer").then_some(token))
        .unwrap_or("");
    if supplied.len() != token.len() || !bool::from(supplied.as_bytes().ct_eq(token.as_bytes())) {
        return (StatusCode::UNAUTHORIZED, [(header::WWW_AUTHENTICATE, "Bearer")]).into_response();
    }
    let mut response = next.run(request).await;
    response.headers_mut().insert("x-bootah-authentication", "bearer-v1".parse().unwrap());
    response
}
async fn management_guard(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    if request.headers().contains_key(header::ORIGIN) {
        return StatusCode::FORBIDDEN.into_response();
    }
    next.run(request).await
}
async fn network_permissions(
    State(m): State<Manager>,
) -> Result<Json<crate::permissions::Report>, ApiError> {
    m.check_permissions().await.map(Json).map_err(bad)
}
async fn smb_status(State(m): State<Manager>) -> Json<serde_json::Value> {
    Json(m.smb_status_value())
}
#[derive(Deserialize)]
struct SmbSwitch {
    mode: String,
    #[serde(default)]
    confirm: bool,
    expected_mode: Option<String>,
}
async fn smb_switch(State(m): State<Manager>, Json(request): Json<SmbSwitch>) -> Response {
    match m
        .switch_smb(request.mode, request.confirm, request.expected_mode)
        .await
    {
        Ok(state) => Json(state).into_response(),
        Err((status, value)) => (status, Json(value)).into_response(),
    }
}

async fn clear_events(State(m): State<Manager>) -> Result<StatusCode, ApiError> {
    m.state.lock().unwrap().clear_events().map_err(bad)?;
    Ok(StatusCode::NO_CONTENT)
}
async fn snapshot(State(m): State<Manager>) -> Json<serde_json::Value> {
    let s = m.state.lock().unwrap();
    let mut value =
        serde_json::json!({"network":*m.network.lock().unwrap(),"settings":s.config.settings,"devices":s.config.devices,"device_conflicts":s.config.pending_device_conflicts,"profiles":s.config.profiles.iter().map(|p| {let mut p=p.clone();p.secure_boot=Some(crate::secure_boot::support(&p));p}).collect::<Vec<_>>(),"default_profiles":s.config.default_profiles,"automations":s.config.automations.iter().map(|(key,value)| (key.clone(),serde_json::json!({"method":value.method,"filename":value.filename,"profile_id":value.profile_id}))).collect::<std::collections::BTreeMap<_,_>>(),"events":s.events,"running":!m.tasks.lock().unwrap().is_empty(),"protocols":[{"name":"BOOTP/DHCPv4","implemented":true},{"name":"PXE / ProxyDHCP (unicast)","implemented":true},{"name":"TFTP","implemented":true},{"name":"HTTP","implemented":true},{"name":"NBD (read-only)","implemented":true},{"name":"RARP","implemented":true},{"name":"RMP","implemented":true},{"name":"MOP","implemented":true},{"name":"LAD/LAST","implemented":false},{"name":"ND","implemented":false},{"name":"DHCPv6","implemented":false}]});
    value["core_version"] = serde_json::json!({"display": env!("BOOTAH_CORE_VERSION"), "build": env!("BOOTAH_CORE_BUILD").parse::<u32>().unwrap()});
    Json(value)
}
async fn detect_network(
    State(m): State<Manager>,
    Json(request): Json<network::Detection>,
) -> Result<Json<network::Report>, ApiError> {
    m.detect_network(request).await.map(Json).map_err(bad)
}
async fn settings(
    State(m): State<Manager>,
    Json(value): Json<Settings>,
) -> Result<StatusCode, ApiError> {
    let _guard = m.lifecycle.lock().await;
    if value.smb_mode != m.state.lock().unwrap().config.settings.smb_mode {
        return Err(bad("Change SMB mode through /api/smb"));
    }
    if value.http_port == 0
        || value.tftp_port == 0
        || value.dhcp_port == 0
        || value.pxe_port == 0
        || !matches!(value.smb_mode.as_str(), "off" | "auto" | "smb1" | "smb2")
        || value.ris_smb_port == 0
        || value.ris_smb_port == value.http_port
        || value.pxe_port == value.dhcp_port
        || value.pxe_port == value.tftp_port
        || (value.dhcp_auto && value.dhcp_enabled)
        || (value.dhcp_enabled && value.proxy_dhcp_enabled)
        || u32::from(value.pool_end) < u32::from(value.pool_start)
        || !matches!(value.boot_transport.as_str(), "http" | "tftp")
    {
        return Err(bad("Invalid network settings"));
    }
    if !m.tasks.lock().unwrap().is_empty() {
        return Err(bad("Stop services before changing network settings"));
    }
    let mut s = m.state.lock().unwrap();
    let previous = s.config.settings.clone();
    s.config.settings = value;
    *m.network.lock().unwrap() = network::Report::default();
    if let Err(e) = s.save() {
        s.config.settings = previous;
        return Err(bad(e));
    }
    Ok(StatusCode::NO_CONTENT)
}
async fn default_profiles(
    State(m): State<Manager>,
    Json(value): Json<std::collections::BTreeMap<String, String>>,
) -> Result<StatusCode, ApiError> {
    let _guard = m.lifecycle.lock().await;
    let mut candidate = m.state.lock().unwrap().config.clone();
    for (architecture, id) in &value {
        if !known_firmware_architecture(architecture)
            || !candidate
                .profiles
                .iter()
                .any(|p| &p.id == id && p.boot_files.contains_key(architecture))
        {
            return Err(bad(
                "Default profile must provide a boot file for its firmware architecture",
            ));
        }
    }
    candidate.default_profiles = value.clone();
    if m.is_running() { m.prepare_media(&candidate).await.map_err(bad)?; }
    let mut s = m.state.lock().unwrap();
    let previous = std::mem::replace(&mut s.config.default_profiles, value);
    if let Err(error) = s.save() {
        s.config.default_profiles = previous;
        return Err(bad(error));
    }
    Ok(StatusCode::NO_CONTENT)
}
async fn device(
    State(m): State<Manager>,
    Json(mut d): Json<Device>,
) -> Result<Json<Device>, ApiError> {
    let _guard = m.lifecycle.lock().await;
    let requested_id = d.id.clone();
    if d.mac.is_empty() {
        if let Some(ip) = d.ip {
            let (known, interface) = {
                let s = m.state.lock().unwrap();
                (
                    s.config
                        .devices
                        .iter()
                        .find(|other| other.ip == Some(ip) && !other.mac.is_empty())
                        .map(|other| other.mac.clone()),
                    s.config.settings.interface.clone(),
                )
            };
            d.mac = match known {
                Some(mac) => mac,
                None => tokio::task::spawn_blocking(move || {
                    crate::neighbors::lookup(&interface).remove(&ip)
                })
                .await
                .ok()
                .flatten()
                .unwrap_or_default(),
            };
        }
    }
    if d.mac.is_empty() {
        let ip = d.ip.ok_or_else(|| bad("MAC or IP address required"))?;
        d.id = format!("ip-{ip}");
    } else {
        d.mac = normalize_mac(&d.mac).ok_or_else(|| bad("Invalid MAC address"))?;
        d.id = d.mac.replace(':', "");
    }
    d.auto_registered = false;
    if m.is_running() {
        let mut candidate = m.state.lock().unwrap().config.clone();
        let mut assignment = d.clone();
        if requested_id != d.id && assignment.profile_id.is_none() {
            assignment.profile_id = candidate.devices.iter().find(|other| other.id == d.id).and_then(|other| other.profile_id.clone());
        }
        if assignment.profile_id.as_ref().is_some_and(|id| !candidate.profiles.iter().any(|p| &p.id == id)) {
            return Err(bad("Unknown system"));
        }
        if d.ip.is_some() && candidate.devices.iter().any(|other| {
            other.id != d.id && other.ip == d.ip && (!other.mac.is_empty() || d.mac.is_empty())
        }) { return Err(bad("IP address already assigned")); }
        candidate.devices.retain(|other| other.id != d.id);
        candidate.devices.push(assignment);
        m.prepare_media(&candidate).await.map_err(bad)?;
    }
    let mut s = m.state.lock().unwrap();
    if d.profile_id
        .as_ref()
        .is_some_and(|id| !s.config.profiles.iter().any(|p| &p.id == id))
    {
        return Err(bad("Unknown system"));
    }
    if d.ip.is_some()
        && s.config.devices.iter().any(|other| {
            other.id != d.id && other.ip == d.ip && (!other.mac.is_empty() || d.mac.is_empty())
        })
    {
        return Err(bad("IP address already assigned"));
    }
    let previous = s.config.devices.clone();
    let previous_auto = s.config.automations.clone();
    if !d.mac.is_empty() {
        if let Some(ip) = d.ip {
            s.associate(ip, &d.mac);
            if s.config
                .devices
                .iter()
                .any(|other| other.ip == Some(ip) && other.id != d.id)
            {
                s.config.devices = previous;
                s.config.automations = previous_auto;
                return Err(bad(
                    "Device association conflicts with an existing assignment",
                ));
            }
        }
    }
    if let Some(other) = s.config.devices.iter_mut().find(|other| other.id == d.id) {
        if requested_id != d.id {
            if d.profile_id.is_none() {
                d.profile_id = other.profile_id.clone();
            }
            if d.name.is_empty() {
                d.name = other.name.clone();
            }
            if d.model.is_empty() {
                d.model = other.model.clone();
            }
            if d.architecture.is_empty() {
                d.architecture = other.architecture.clone();
            }
            if d.uuid.is_empty() {
                d.uuid = other.uuid.clone();
            }
            if d.vendor.is_empty() {
                d.vendor = other.vendor.clone();
            }
            d.last_seen = d.last_seen.max(other.last_seen);
        }
        *other = d.clone()
    } else {
        s.config.devices.push(d.clone())
    }
    if let Err(e) = s.save() {
        s.config.devices = previous;
        s.config.automations = previous_auto;
        return Err(bad(e));
    }
    Ok(Json(d))
}
#[derive(Deserialize)]
struct DeviceConflictDecision {
    accept: bool,
}
async fn resolve_device_conflict(
    State(m): State<Manager>,
    Path(id): Path<String>,
    Json(decision): Json<DeviceConflictDecision>,
) -> Result<StatusCode, ApiError> {
    let _guard = m.lifecycle.lock().await;
    m.state
        .lock()
        .unwrap()
        .resolve_device_conflict(&id, decision.accept)
        .map_err(bad)?;
    Ok(StatusCode::NO_CONTENT)
}
async fn remove_device(
    State(m): State<Manager>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let _guard = m.lifecycle.lock().await;
    let mut s = m.state.lock().unwrap();
    let previous = s.config.devices.clone();
    let old_auto = s.config.automations.remove(&automation::key("device", &id));
    s.config.devices.retain(|d| d.id != id);
    if let Err(e) = s.save() {
        s.config.devices = previous;
        if let Some(auto) = old_auto {
            s.config
                .automations
                .insert(automation::key("device", &id), auto);
        }
        return Err(bad(e));
    }
    if let Some(auto) = old_auto {
        let _ = std::fs::remove_file(automation::file(&s, &auto.token));
    }
    Ok(StatusCode::NO_CONTENT)
}
async fn profile(
    State(m): State<Manager>,
    Json(mut p): Json<Profile>,
) -> Result<Json<Profile>, ApiError> {
    if !valid_id(&p.id) || p.name.trim().is_empty() {
        return Err(bad("Invalid system name or ID"));
    }
    p.root = p.root.canonicalize().map_err(|e| bad(e.to_string()))?;
    if !p.root.is_dir() {
        return Err(bad("System root must be a directory"));
    }
    for file in p.boot_files.values().chain([&p.kernel, &p.initrd, &p.dtb]) {
        if !file.is_empty() && file != "boot.ipxe" {
            if file.chars().any(char::is_whitespace) {
                return Err(bad("Boot file names must not contain whitespace"));
            }
            if !crate::media_source::exists(&p.root, file) {
                return Err(bad("Profile file unavailable"));
            }
        }
    }
    if p.arguments.contains(['\n', '\r']) {
        return Err(bad("Kernel arguments must be one line"));
    }
    if !matches!(
        p.kind.as_str(),
        "linux" | "bsd" | "windows" | "windows-pe-iso" | "windows-ris" | "fit" | "raw" | "solaris"
    ) {
        return Err(bad("Unknown system kind"));
    }
    let _guard = m.lifecycle.lock().await;
    {
        let state=m.state.lock().unwrap();
        if let Some(old)=state.config.profiles.iter().find(|o|o.id==p.id) {
            if crate::native_uefi::enabled(old) {return Err(bad("Turn off Secure Boot support before editing"));}
        }
    }
    if crate::native_uefi::enabled(&p) {return Err(bad("Use the Secure Boot support setting"));}
    p.secure_boot=Some(crate::secure_boot::Support {eligible:crate::secure_boot::eligible(&p),state:"unchecked".into(),..Default::default()});
    if m.is_running() {
        let mut candidate = m.state.lock().unwrap().config.clone();
        if candidate.default_profiles.iter().any(|(architecture, id)| id == &p.id && !p.boot_files.contains_key(architecture)) {
            return Err(bad("Default profile must keep its firmware boot file"));
        }
        candidate.profiles.retain(|other| other.id != p.id);
        candidate.profiles.push(p.clone());
        m.prepare_media(&candidate).await.map_err(bad)?;
    }
    let mut s = m.state.lock().unwrap();
    if s.config
        .default_profiles
        .iter()
        .any(|(architecture, id)| id == &p.id && !p.boot_files.contains_key(architecture))
    {
        return Err(bad("Default profile must keep its firmware boot file"));
    }
    let previous = s.config.profiles.clone();
    if let Some(other) = s.config.profiles.iter_mut().find(|o| o.id == p.id) {
        *other = p.clone()
    } else {
        s.config.profiles.push(p.clone())
    }
    if let Err(e) = s.save() {
        s.config.profiles = previous;
        return Err(bad(e));
    }
    Ok(Json(p))
}
async fn remove_profile(
    State(m): State<Manager>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let _guard = m.lifecycle.lock().await;
    let mut s = m.state.lock().unwrap();
    if s.config
        .devices
        .iter()
        .any(|d| d.profile_id.as_ref() == Some(&id))
    {
        return Err(bad("System is assigned to a device"));
    }
    if s.config
        .default_profiles
        .values()
        .any(|profile| profile == &id)
    {
        return Err(bad("System is assigned as a default profile"));
    }
    let previous = s.config.profiles.clone();
    let profile_key = automation::key("profile", &id);
    let removed_auto = s
        .config
        .automations
        .iter()
        .filter(|(key, auto)| *key == &profile_key || auto.profile_id == id)
        .map(|(key, auto)| (key.clone(), auto.clone()))
        .collect::<Vec<_>>();
    for (key, _) in &removed_auto {
        s.config.automations.remove(key);
    }
    s.config.profiles.retain(|p| p.id != id);
    if let Err(e) = s.save() {
        s.config.profiles = previous;
        for (key, auto) in removed_auto {
            s.config.automations.insert(key, auto);
        }
        return Err(bad(e));
    }
    for (_, auto) in removed_auto {
        let _ = std::fs::remove_file(automation::file(&s, &auto.token));
    }
    Ok(StatusCode::NO_CONTENT)
}
async fn boot_commands(
    State(m): State<Manager>,
    Path(id): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<boot::ManualBoot>, ApiError> {
    let s = m.state.lock().unwrap();
    let p = s
        .config
        .profiles
        .iter()
        .find(|p| p.id == id)
        .ok_or_else(|| bad("Unknown system"))?;
    let chinese = params
        .get("lang")
        .is_some_and(|language| language.to_ascii_lowercase().starts_with("zh"));
    Ok(Json(boot::manual_boot(p, &s.config.settings, chinese)))
}
async fn uboot(State(m): State<Manager>, Path(id): Path<String>) -> Result<String, ApiError> {
    let s = m.state.lock().unwrap();
    let p = s
        .config
        .profiles
        .iter()
        .find(|p| p.id == id)
        .ok_or_else(|| bad("Unknown system"))?;
    Ok(boot::uboot(p, &s.config.settings).map_err(bad)?)
}
async fn start(State(m): State<Manager>) -> Result<StatusCode, ApiError> {
    m.start().await.map_err(|e| {
        m.state
            .lock()
            .unwrap()
            .event("server", "service", format!("Start failed: {e}"));
        bad(e)
    })?;
    Ok(StatusCode::NO_CONTENT)
}
async fn stop(State(m): State<Manager>) -> StatusCode {
    m.stop().await;
    StatusCode::NO_CONTENT
}
async fn boot_file(
    State(m): State<Manager>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Path((id, file)): Path<(String, String)>,
    method: Method,
    headers: HeaderMap,
) -> Response {
    let file = match crate::media_source::sanitize(&file) {
        Ok(file) => file,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };
    if let IpAddr::V4(ip) = peer.ip() {
        let interface = {
            let s = m.state.lock().unwrap();
            s.device_for(ip)
                .is_none()
                .then(|| s.config.settings.interface.clone())
        };
        if let Some(interface) = interface {
            if let Ok(Some(mac)) =
                tokio::task::spawn_blocking(move || crate::neighbors::resolve(ip, &interface)).await
            {
                let mut s = m.state.lock().unwrap();
                if s.associate(ip, &mac) {
                    if let Err(error) = s.save() {
                        s.event("network", "storage", error);
                    }
                }
            }
        }
    }
    let (p, settings, auto, device_id) = {
        let mut s = m.state.lock().unwrap();
        if let IpAddr::V4(ip) = peer.ip() {
            let mac = s
                .device_for(ip)
                .filter(|d| d.ip != Some(ip) && !d.mac.is_empty())
                .map(|d| d.mac.clone());
            if let Some(mac) = mac {
                if s.associate(ip, &mac) {
                    if let Err(error) = s.save() {
                        s.event("network", "storage", error);
                    }
                }
            }
        }
        let p = s.config.profiles.iter().find(|p| p.id == id).cloned();
        let assigned = match peer.ip() {
            IpAddr::V4(ip) => s.profile_for(ip).is_some_and(|p| p.id == id),
            _ => false,
        };
        if !assigned {
            s.event(
                &peer.ip().to_string(),
                "HTTP",
                "Rejected unassigned system request",
            );
            return StatusCode::FORBIDDEN.into_response();
        }
        let device_id = match peer.ip() {
            IpAddr::V4(ip) => s.device_for(ip).map(|d| d.id.as_str()),
            _ => None,
        };
        let auto = p
            .as_ref()
            .and_then(|p| automation::effective(&s.config, p, device_id))
            .cloned();
        (
            p,
            s.config.settings.clone(),
            auto,
            device_id.map(str::to_owned),
        )
    };
    let Some(p) = p else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if p.kind == "windows"
        && file == "bootah-setup.cmd"
        && crate::media_source::exists(&p.root, "bootah-setup.cmd")
        && [
            "sources/install.wim",
            "sources/install.esd",
            "sources/install.swm",
        ]
        .iter()
        .any(|path| crate::media_source::exists(&p.root, path))
    {
        let _guard = m.lifecycle.lock().await;
        if matches!(m.smb_active.lock().unwrap().as_str(), "smb2" | "auto") {
            if settings.ris_smb_port != 445 {
                return (StatusCode::SERVICE_UNAVAILABLE, "Windows Setup requires SMB on TCP 445; the configured port is for protocol testing only.").into_response();
            }
            let Some(device_id) = device_id.as_deref() else {
                return (
                    StatusCode::FORBIDDEN,
                    "Register this device before requesting SMB installation credentials.",
                )
                    .into_response();
            };
            let credential = match m.smb_credential(&p.id, device_id) {
                Ok(credential) => credential,
                Err(error) => return (StatusCode::SERVICE_UNAVAILABLE, error).into_response(),
            };
            match crate::windows::smb_setup_script(
                &p,
                settings.server_ip,
                &credential.username,
                &credential.password,
            ) {
                Ok(script) => {
                    let script = if crate::native_uefi::enabled(&p) {
                        if let Some(auto) = auto.as_ref().filter(|a| a.method == "unattend") {
                            format!("@echo off\r\n\"%SystemRoot%\\System32\\bootah-native.exe\" \"{}unattend.xml\" \"%~dp0unattend.xml\"\r\nif errorlevel 1 exit /b 1\r\n{script}", automation::url(&settings, auto))
                        } else { script }
                    } else { script };
                    return (
                        [
                            (header::CONTENT_TYPE, "text/plain"),
                            (header::CACHE_CONTROL, "no-store"),
                        ],
                        script,
                    )
                        .into_response();
                }
                Err(error) => return (StatusCode::BAD_REQUEST, error).into_response(),
            }
        }
        return (StatusCode::SERVICE_UNAVAILABLE, "Windows Setup requires the SMB2 installation source service; enable automatic SMB mode.").into_response();
    }
    if let Some(bytes) = boot::virtual_file(&p, &file, &settings, auto.as_ref()) {
        m.state.lock().unwrap().event(
            &peer.ip().to_string(),
            "HTTP",
            format!("Serving {file} {} bytes", bytes.len()),
        );
        return ([(header::CONTENT_TYPE, "text/plain")], bytes).into_response();
    }
    let file = crate::native_uefi::resource_name(&p, &file).to_string();
    if let Ok(path) = safe_file(&p.root, &file) {
        use tower::ServiceExt;
        let mut request = axum::http::Request::builder()
            .method(method.clone())
            .uri("/")
            .body(axum::body::Body::empty())
            .unwrap();
        *request.headers_mut() = headers;
        m.state
            .lock()
            .unwrap()
            .event(&peer.ip().to_string(), "HTTP", format!("Serving {file}"));
        return match tower_http::services::ServeFile::new(path)
            .with_buf_chunk_size(128 * 1024)
            .oneshot(request)
            .await
        {
            Ok(response) => response.map(axum::body::Body::new),
            Err(error) => match error {},
        };
    }
    let root = p.root.clone();
    let requested = file.clone();
    let resource =
        tokio::task::spawn_blocking(move || crate::media_source::open(&root, &requested)).await;
    let resource = match resource {
        Ok(Ok(resource)) => resource,
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    let size = resource.size;
    let mut start = 0;
    let mut end = size.saturating_sub(1);
    let mut status = StatusCode::OK;
    if let Some(value) = headers.get(header::RANGE) {
        let range = value
            .to_str()
            .ok()
            .and_then(|v| v.strip_prefix("bytes="))
            .filter(|v| !v.contains(','))
            .and_then(|v| v.split_once('-'))
            .and_then(|(a, b)| {
                if a.is_empty() {
                    let count = b.parse::<u64>().ok()?.min(size);
                    (count > 0).then_some((size - count, size.saturating_sub(1)))
                } else {
                    let first = a.parse::<u64>().ok()?;
                    let last = if b.is_empty() {
                        size.saturating_sub(1)
                    } else {
                        b.parse::<u64>().ok()?.min(size.saturating_sub(1))
                    };
                    (first < size && first <= last).then_some((first, last))
                }
            });
        let Some((first, last)) = range else {
            return (
                StatusCode::RANGE_NOT_SATISFIABLE,
                [(header::CONTENT_RANGE, format!("bytes */{size}"))],
            )
                .into_response();
        };
        start = first;
        end = last;
        status = StatusCode::PARTIAL_CONTENT;
    }
    let length = if size == 0 { 0 } else { end - start + 1 };
    let mut response = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_LENGTH, length);
    if status == StatusCode::PARTIAL_CONTENT {
        response = response.header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{size}"));
    }
    m.state.lock().unwrap().event(
        &peer.ip().to_string(),
        "HTTP",
        format!("Serving {file} bytes {start}+{length}"),
    );
    if method == Method::HEAD {
        return response.body(axum::body::Body::empty()).unwrap();
    }
    response
        .body(axum::body::Body::from_stream(
            resource.stream(start, length),
        ))
        .unwrap()
}

#[derive(Deserialize)]
struct CompatibilityRequest {
    enabled: bool,
}
async fn profile_compatibility(
    State(m): State<Manager>,
    Path(id): Path<String>,
    Json(request): Json<CompatibilityRequest>,
) -> Result<Json<Profile>, ApiError> {
    let _guard = m.lifecycle.lock().await;
    if m.is_running() {
        return Err(bad("Stop services before changing compatibility mode."));
    }
    let (old, library) = {
        let s = m.state.lock().unwrap();
        let old = s.config.profiles.iter().find(|p| p.id == id).cloned().ok_or_else(|| bad("Unknown system"))?;
        if crate::native_uefi::enabled(&old) {return Err(bad("Turn off Secure Boot support before changing compatibility mode"));}
        if old.kind != "linux" || old.media.is_empty() {
            return Err(bad("Compatibility mode requires an imported Linux ISO."));
        }
        (old, s.path.parent().unwrap_or(std::path::Path::new(".")).join("media"))
    };
    let source = old.media.clone();
    let result = tokio::task::spawn_blocking(move || media::import_with_options(std::path::Path::new(&source), &library, false, request.enabled))
        .await.map_err(|e| bad(e.to_string()))?.map_err(bad)?;
    let mut candidates = result.profiles.into_iter().filter(|p| p.kind == old.kind && p.architecture == old.architecture);
    let mut changed = candidates.next().ok_or_else(|| bad("No matching boot entry in requested mode"))?;
    if candidates.next().is_some() {
        return Err(bad("Multiple boot entries match; import this ISO separately."));
    }
    changed.id = old.id.clone();
    changed.name = old.name;
    let mut s = m.state.lock().unwrap();
    let index = s.config.profiles.iter().position(|p| p.id == id).ok_or_else(|| bad("System removed during conversion"))?;
    if s.config.default_profiles.iter().any(|(architecture, profile)| profile == &id && !changed.boot_files.contains_key(architecture)) {
        return Err(bad("Requested mode does not support an assigned default architecture"));
    }
    let previous = std::mem::replace(&mut s.config.profiles[index], changed.clone());
    if let Err(error) = s.save() {
        s.config.profiles[index] = previous;
        return Err(bad(error));
    }
    Ok(Json(changed))
}

async fn import_media(
    State(m): State<Manager>,
    Json(request): Json<media::ImportRequest>,
) -> Result<Json<media::ImportResult>, ApiError> {
    if request.native_uefi { return Err(bad("Use the image Secure Boot support setting after import")); }
    let library = {
        let s = m.state.lock().unwrap();
        s.path
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .join("media")
    };
    let mut result = tokio::task::spawn_blocking(move || {
        let mut result = media::import_with_options(
            &request.path,
            &library,
            request.trust_local_iso,
            request.linux_compat,
        )?;
        for profile in &mut result.profiles {profile.secure_boot=Some(crate::secure_boot::check(profile,&Settings::default())?);}
        Ok::<_, String>(result)
    })
    .await
    .map_err(|e| bad(e.to_string()))?
    .map_err(bad)?;
    let _guard = m.lifecycle.lock().await;
    if m.is_running() {
        let mut candidate = m.state.lock().unwrap().config.clone();
        candidate.profiles.extend(result.profiles.clone());
        m.prepare_media(&candidate).await.map_err(bad)?;
    }
    let needs_smb = {
        let mut s = m.state.lock().unwrap();
        let previous = s.config.profiles.clone();
        let needs_smb = !previous.iter().any(|p| p.kind == "windows-ris")
            && result.profiles.iter().any(|p| p.kind == "windows-ris");
        s.config.profiles.extend(result.profiles.clone());
        if let Err(e) = s.save() {
            s.config.profiles = previous;
            return Err(bad(e));
        }
        s.event(
            "library",
            "ISO",
            format!("Imported {} system(s)", result.profiles.len()),
        );
        needs_smb
    };
    if needs_smb {
        if m.is_running() {
            let settings = m.state.lock().unwrap().config.settings.clone();
            let active = m.smb_active.lock().unwrap().clone();
            if active == "smb2" {
                result.warnings.push("SMB2 is active. Starting XP/2003 SMB1 requires confirmation through /api/smb; the existing service was not stopped.".into());
            } else if settings.smb_mode == "smb1"
                && active == "off"
                && let Err(error) = m.start_smb_inner("smb1", &settings).await
            {
                result.warnings.push(error);
            }
        }
    }
    Ok(Json(result))
}

#[derive(Deserialize)]
struct AutomationQuery {
    method: String,
    filename: String,
}

async fn automation_methods(
    State(m): State<Manager>,
    Path(id): Path<String>,
) -> Result<Json<Vec<&'static str>>, ApiError> {
    let profile = m
        .state
        .lock()
        .unwrap()
        .config
        .profiles
        .iter()
        .find(|p| p.id == id)
        .cloned()
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Profile not found".into()))?;
    Ok(Json(automation::methods(&profile)))
}

fn automation_profile(s: &Store, scope: &str, id: &str) -> Option<Profile> {
    let profile_id = match scope {
        "profile" => id,
        "device" => {
            let device = s.config.devices.iter().find(|d| d.id == id)?;
            device.profile_id.as_deref().or_else(|| {
                s.config
                    .default_profiles
                    .get(&device.architecture)
                    .map(String::as_str)
            })?
        }
        _ => return None,
    };
    s.config
        .profiles
        .iter()
        .find(|p| p.id == profile_id)
        .cloned()
}

async fn set_automation(
    State(m): State<Manager>,
    Path((scope, id)): Path<(String, String)>,
    Query(query): Query<AutomationQuery>,
    bytes: Bytes,
) -> Result<StatusCode, ApiError> {
    if bytes.is_empty()
        || bytes.len() > 1024 * 1024
        || query.filename.len() > 240
        || query.filename.chars().any(char::is_control)
    {
        return Err(bad(
            "Automatic configuration file is empty, too large, or has an invalid name",
        ));
    }
    let mut s = m.state.lock().unwrap();
    let profile = automation_profile(&s, &scope, &id)
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Profile or device not found".into()))?;
    if !automation::methods(&profile).contains(&query.method.as_str()) {
        return Err(bad(
            "This installer does not support the selected configuration method",
        ));
    }
    if query.method == "autoinstall"
        && (!bytes.starts_with(b"#cloud-config")
            || !bytes
                .windows(b"autoinstall:".len())
                .any(|v| v == b"autoinstall:"))
    {
        return Err(bad(
            "Ubuntu autoinstall user-data requires #cloud-config and autoinstall:",
        ));
    }
    let token = uuid::Uuid::new_v4().simple().to_string();
    let path = automation::file(&s, &token);
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| bad(e.to_string()))?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    use std::io::Write;
    if let Err(error) = options
        .open(&path)
        .and_then(|mut file| file.write_all(&bytes))
    {
        let _ = std::fs::remove_file(&path);
        return Err(bad(error.to_string()));
    }
    let key = automation::key(&scope, &id);
    let previous = s.config.automations.insert(
        key.clone(),
        AutoConfig {
            profile_id: profile.id,
            method: query.method,
            token,
            filename: query.filename,
        },
    );
    if let Err(error) = s.save() {
        let new = s.config.automations.remove(&key).unwrap();
        if let Some(previous) = previous {
            s.config.automations.insert(key, previous);
        }
        let _ = std::fs::remove_file(automation::file(&s, &new.token));
        return Err(bad(error));
    }
    if let Some(previous) = previous {
        let _ = std::fs::remove_file(automation::file(&s, &previous.token));
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn clear_automation(
    State(m): State<Manager>,
    Path((scope, id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let mut s = m.state.lock().unwrap();
    if !match scope.as_str() {
        "profile" => s.config.profiles.iter().any(|p| p.id == id),
        "device" => s.config.devices.iter().any(|d| d.id == id),
        _ => false,
    } {
        return Err((StatusCode::NOT_FOUND, "Profile or device not found".into()));
    }
    let key = automation::key(&scope, &id);
    let previous = s.config.automations.remove(&key);
    if let Err(error) = s.save() {
        if let Some(previous) = previous {
            s.config.automations.insert(key, previous);
        }
        return Err(bad(error));
    }
    if let Some(previous) = previous {
        let _ = std::fs::remove_file(automation::file(&s, &previous.token));
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn automation_file(
    State(m): State<Manager>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Path((token, name)): Path<(String, String)>,
) -> Response {
    let mut s = m.state.lock().unwrap();
    let result = automation::read_token(&s, &token, &name);
    if let Some(ref bytes) = result {
        s.event(
            &peer.ip().to_string(),
            "HTTP",
            format!("Automatic configuration {name} {} bytes", bytes.len()),
        );
    }
    match result {
        Some(bytes) => (
            [
                (header::CACHE_CONTROL, "no-store"),
                (header::CONTENT_TYPE, "application/octet-stream"),
            ],
            bytes,
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

#[cfg(test)]
mod smb_lifecycle_tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    #[tokio::test]
    async fn failed_smb_start_preserves_selection() {
        let root = std::env::temp_dir().join(format!("bootah-smb-failure-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let busy = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut config = Config::default();
        config.settings.bind_ip = std::net::Ipv4Addr::LOCALHOST;
        config.settings.ris_smb_port = busy.local_addr().unwrap().port();
        let state = Arc::new(Mutex::new(Store { config, events: vec![], path: root.join("state.json") }));
        let manager = Manager::new(state);
        manager.tasks.lock().unwrap().push(tokio::spawn(std::future::pending()));
        let error = manager.switch_smb("smb2".into(), false, None).await.unwrap_err();
        assert_eq!(error.0, StatusCode::CONFLICT);
        assert_eq!(manager.smb_status_value()["mode"], "off");
        assert_eq!(manager.smb_status_value()["configured_mode"], "auto");
        manager.stop().await;
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn switching_requires_current_confirmation_and_closes_sessions() {
        let root = std::env::temp_dir().join(format!("bootah-smb-switch-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let state = Arc::new(Mutex::new(Store {
            config: Config::default(),
            events: vec![],
            path: root.join("state.json"),
        }));
        let manager = Manager::new(state.clone());
        let first = manager.smb_credential("media-a", "device-a").unwrap();
        let again = manager.smb_credential("media-a", "device-a").unwrap();
        let other = manager.smb_credential("media-a", "device-b").unwrap();
        assert_eq!(first.username, again.username);
        assert_ne!(first.username, other.username);
        assert!(
            !manager
                .smb_status_value()
                .to_string()
                .contains(&first.password)
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        manager
            .smb_tasks
            .lock()
            .unwrap()
            .push(tokio::spawn(crate::smb1::run(state, listener, false)));
        *manager.smb_active.lock().unwrap() = "smb1".into();
        let mut client = tokio::net::TcpStream::connect(address).await.unwrap();
        tokio::task::yield_now().await;
        let error = manager
            .switch_smb("off".into(), false, None)
            .await
            .unwrap_err();
        assert_eq!(error.0, StatusCode::CONFLICT);
        assert_eq!(manager.smb_status_value()["mode"], "smb1");
        assert!(
            manager
                .switch_smb("off".into(), true, Some("smb2".into()))
                .await
                .is_err()
        );
        let mut changed = manager.state.lock().unwrap().config.settings.clone();
        changed.smb_mode = "smb2".into();
        assert!(
            settings(State(manager.clone()), Json(changed))
                .await
                .is_err()
        );
        assert_eq!(manager.smb_status_value()["mode"], "smb1");
        let (one, two) = tokio::join!(
            manager.switch_smb("off".into(), true, Some("smb1".into())),
            manager.switch_smb("off".into(), true, Some("smb1".into())),
        );
        assert!(one.is_ok() ^ two.is_ok());
        let error = one.err().or_else(|| two.err()).unwrap();
        assert_eq!(error.0, StatusCode::CONFLICT);
        assert_eq!(manager.smb_status_value()["mode"], "off");
        let mut byte = [0];
        let read = tokio::time::timeout(std::time::Duration::from_secs(2), client.read(&mut byte))
            .await
            .unwrap();
        assert!(matches!(read, Ok(0) | Err(_)));
        assert!(tokio::net::TcpListener::bind(address).await.is_ok());
        std::fs::remove_dir_all(root).unwrap();
    }
}

async fn secure_boot_status(State(m):State<Manager>,Path(id):Path<String>) -> Result<impl IntoResponse,ApiError> {
    Ok(([(header::CACHE_CONTROL,"no-store")],Json(refresh_secure_boot(m,id,false).await?)))
}
async fn secure_boot_check(State(m):State<Manager>,Path(id):Path<String>) -> Result<Json<crate::secure_boot::Support>,ApiError> {
    refresh_secure_boot(m,id,true).await.map(Json)
}
async fn refresh_secure_boot(m:Manager,id:String,force:bool) -> Result<crate::secure_boot::Support,ApiError> {
    let _guard=m.lifecycle.lock().await;
    let (old,settings)={let s=m.state.lock().unwrap();(s.config.profiles.iter().find(|p|p.id==id).cloned().ok_or_else(||bad("Unknown system"))?,s.config.settings.clone())};
    let mut result=tokio::task::spawn_blocking(move || {
        if !force {if let Some(cached)=crate::secure_boot::cached(&old) {return Ok((cached,false));}}
        crate::secure_boot::check(&old,&settings).map(|s|(s,true))
    }).await.map_err(|e|bad(e.to_string()))?.map_err(bad)?;
    if !result.1 {return Ok(result.0);}
    let mut s=m.state.lock().unwrap();let previous=s.config.profiles.clone();
    let p=s.config.profiles.iter_mut().find(|p|p.id==id).ok_or_else(||bad("Unknown system"))?;
    result.0.enabled=crate::native_uefi::enabled(p);
    p.secure_boot=Some(result.0.clone());
    if let Err(e)=s.save(){s.config.profiles=previous;return Err(bad(e));}Ok(result.0)
}
async fn secure_boot_switch(State(m):State<Manager>,Path(id):Path<String>,Json(request):Json<CompatibilityRequest>) -> Result<Json<Profile>,ApiError> {
    let _guard=m.lifecycle.lock().await;
    let (old,settings)={let s=m.state.lock().unwrap();(s.config.profiles.iter().find(|p|p.id==id).cloned().ok_or_else(||bad("Unknown system"))?,s.config.settings.clone())};
    let changed=tokio::task::spawn_blocking(move ||crate::secure_boot::switch(&old,&settings,request.enabled)).await.map_err(|e|bad(e.to_string()))?.map_err(bad)?;
    let mut s=m.state.lock().unwrap();
    if s.config.default_profiles.iter().any(|(arch,p)|p==&id && !changed.boot_files.contains_key(arch)) {return Err(bad("Remove incompatible architecture defaults before switching"));}
    let previous=s.config.profiles.clone();let p=s.config.profiles.iter_mut().find(|p|p.id==id).ok_or_else(||bad("Unknown system"))?;*p=changed.clone();
    if let Err(e)=s.save(){s.config.profiles=previous;return Err(bad(e));}Ok(Json(changed))
}

pub async fn secure_boot_action(m:Manager,id:String,enabled:Option<bool>) -> Result<String,String> {
    if let Some(enabled)=enabled {secure_boot_switch(State(m),Path(id),Json(CompatibilityRequest{enabled})).await.map(|_|"Secure Boot support updated".into()).map_err(|(_,e)|e)}
    else {secure_boot_check(State(m),Path(id)).await.map(|Json(s)|s.detail).map_err(|(_,e)|e)}
}

pub async fn secure_boot_details(m:Manager,id:String) -> Result<crate::secure_boot::Support,String> {
    refresh_secure_boot(m,id,false).await.map_err(|(_,error)|error)
}
