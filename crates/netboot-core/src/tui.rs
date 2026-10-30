use crate::{media, model::*, network, web::Manager};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Row, Table, TableState, Wrap},
};
use serde_json::Value;
use std::{io, net::Ipv4Addr, time::Duration};

const FIRMWARE_KEYS: &[&str] = &[
    "x86-bios",
    "ia64",
    "ia32-efi",
    "x64-efi",
    "ebc-efi",
    "alpha",
    "powerpc-of",
    "powerpc-epapr",
    "power-opal",
    "arm32-efi",
    "arm64-efi",
    "ia32-http",
    "x64-http",
    "arm32-http",
    "arm64-http",
    "arm32-uboot",
    "arm64-uboot",
    "riscv32-efi",
    "riscv64-http",
    "loongarch64-efi",
    "riscv64-efi",
    "hp-rmp",
];
#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Devices,
    Profiles,
    Activity,
    Network,
}
#[derive(Clone, Copy)]
enum DialogKind {
    NewDevice,
    EditDevice,
    Import,
    Network,
    Assign,
    DefaultProfile,
    ImageDetails,
    PermissionDetails,
    DeleteDevice,
    DeviceConflict,
    Interface,
    Smb,
    SmbConfirm,
}
struct Dialog {
    kind: DialogKind,
    fields: Vec<(String, String)>,
    focus: usize,
}
struct App {
    page: Page,
    selected: usize,
    defaults_scroll: u16,
    dialog: Option<Dialog>,
    notice: String,
    language: String,
    fallback: String,
    tr: Value,
    should_quit: bool,
    job: Option<tokio::task::JoinHandle<Result<String, String>>>,
    job_running: bool,
    permissions_checked: bool,
    authorization_available: bool,
    permissions_job: Option<tokio::task::JoinHandle<Result<crate::permissions::Report, String>>>,
    active_conflict: Option<String>,
}

impl App {
    fn tr(&self, key: &str) -> String {
        if key.contains('\n') {
            return key
                .lines()
                .map(|line| self.tr(line))
                .collect::<Vec<_>>()
                .join("\n");
        }
        for prefix in ["TFTP: ", "DHCP: ", "PXE: ", "NFS: ", "SMB: "] {
            if let Some(detail) = key.strip_prefix(prefix) {
                return format!("{prefix}{}", self.tr(detail));
            }
        }
        self.tr
            .get(key)
            .and_then(|v| v.get(&self.language))
            .and_then(Value::as_str)
            .or_else(|| {
                self.tr
                    .get(key)
                    .and_then(|v| v.get(&self.fallback))
                    .and_then(Value::as_str)
            })
            .unwrap_or(key)
            .to_string()
    }
    fn label(&self) -> String {
        match self.page {
            Page::Devices => self.tr("Devices"),
            Page::Profiles => self.tr("Images"),
            Page::Activity => self.tr("Activity"),
            Page::Network => self.tr("Network settings"),
        }
    }
}

fn select_language(explicit: Option<&str>) -> Result<(String, String), String> {
    let catalog: Value = serde_json::from_str(include_str!("../../../i18n/terms.json"))
        .map_err(|e| e.to_string())?;
    let locales = catalog["locales"]
        .as_array()
        .ok_or("Invalid locale catalog")?;
    let fallback = catalog["fallbackLocale"]
        .as_str()
        .ok_or("Invalid fallback locale")?
        .to_string();
    let find = |candidate: &str| {
        let normalized = candidate.replace('_', "-").to_ascii_lowercase();
        locales
            .iter()
            .filter_map(Value::as_str)
            .find(|locale| locale.replace('_', "-").eq_ignore_ascii_case(&normalized))
            .or_else(|| {
                let primary = normalized.split('-').next().unwrap_or_default();
                locales.iter().filter_map(Value::as_str).find(|locale| {
                    locale
                        .split(['-', '_'])
                        .next()
                        .is_some_and(|language| language.eq_ignore_ascii_case(primary))
                })
            })
            .map(str::to_string)
    };
    if let Some(language) = explicit {
        let supported = locales
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(", ");
        return find(language)
            .map(|locale| (locale, fallback))
            .ok_or_else(|| format!("Unsupported language '{language}'; available: {supported}"));
    }
    let system = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .filter_map(|key| std::env::var(key).ok())
        .find(|v| !v.is_empty())
        .unwrap_or_default();
    let system = system.split(['.', '@']).next().unwrap_or_default();
    Ok((find(system).unwrap_or_else(|| fallback.clone()), fallback))
}

pub async fn run(manager: Manager, explicit_language: Option<&str>) -> Result<(), String> {
    let (language, fallback) = select_language(explicit_language)?;
    if !crossterm::tty::IsTty::is_tty(&io::stdin()) || !crossterm::tty::IsTty::is_tty(&io::stdout())
    {
        return Err("TUI requires an interactive terminal (stdin and stdout must be a TTY)".into());
    }
    let catalog: Value = serde_json::from_str(include_str!("../../../i18n/terms.json"))
        .map_err(|e| e.to_string())?;
    let mut app = App {
        page: Page::Devices,
        selected: 0,
        defaults_scroll: 0,
        dialog: None,
        notice: String::new(),
        language,
        fallback,
        tr: catalog["terms"].clone(),
        should_quit: false,
        job: None,
        job_running: false,
        permissions_checked: false,
        authorization_available: !cfg!(target_os = "macos"),
        permissions_job: None,
        active_conflict: None,
    };
    detect_job(&mut app, &manager, network::Detection::default());
    let mut terminal = ratatui::init();
    let result = run_loop(&mut terminal, &mut app, manager).await;
    ratatui::restore();
    result
}

async fn run_loop(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    manager: Manager,
) -> Result<(), String> {
    loop {
        if app.dialog.is_none() && app.active_conflict.is_none() {
            let conflict = manager
                .state
                .lock()
                .unwrap()
                .config
                .pending_device_conflicts
                .first()
                .cloned();
            if let Some(conflict) = conflict {
                let details = if conflict.reason == "ip-mac" {
                    format!(
                        "IP {}: {} → {}",
                        conflict.ip.map(|ip| ip.to_string()).unwrap_or_default(),
                        conflict.old_mac,
                        conflict.new_mac
                    )
                } else {
                    format!(
                        "{}: {} → {}",
                        conflict.new_mac, conflict.old_architecture, conflict.new_architecture
                    )
                };
                app.active_conflict = Some(conflict.id);
                app.dialog = Some(Dialog {
                    kind: DialogKind::DeviceConflict,
                    fields: vec![
                        ("Device identity changed".into(), details),
                        (
                            "The old device has manual settings. After removal, the new device will use its architecture's default image at the next network boot."
                                .into(),
                            String::new(),
                        ),
                    ],
                    focus: 0,
                });
            }
        }
        terminal
            .draw(|frame| draw(frame, app, &manager))
            .map_err(|e| e.to_string())?;
        if let Some(job) = app.job.as_mut() {
            if job.is_finished() {
                let job = app.job.take().unwrap();
                app.job_running = false;
                app.notice = match job.await {
                    Ok(Ok(message)) => app.tr(&message),
                    Ok(Err(e)) => app.tr(&e),
                    Err(e) => e.to_string(),
                };
                app.selected = 0;
                if !app.permissions_checked {
                    app.permissions_checked = true;
                    let manager = manager.clone();
                    app.permissions_job =
                        Some(tokio::spawn(
                            async move { manager.check_permissions().await },
                        ));
                }
                continue;
            }
        }
        if app.dialog.is_none()
            && app
                .permissions_job
                .as_ref()
                .is_some_and(|job| job.is_finished())
        {
            let job = app.permissions_job.take().unwrap();
            let issues=match job.await {Ok(Ok(report))=>{if cfg!(target_os="macos"){app.authorization_available=report.helper_available;}report.issues},_=>vec!["Network permission checks could not complete. Check the core connection and retry from network settings.".into()]};
            if !issues.is_empty() {
                app.dialog = Some(Dialog {
                    kind: DialogKind::PermissionDetails,
                    fields: issues
                        .into_iter()
                        .map(|message| ("Permissions requiring attention".into(), app.tr(&message)))
                        .collect(),
                    focus: 0,
                });
            }
        }
        if event::poll(Duration::from_millis(100)).map_err(|e| e.to_string())? {
            loop {
                if let Event::Key(key) = event::read().map_err(|e| e.to_string())? {
                    handle_key(app, key, &manager).await;
                    if app.should_quit {
                        break;
                    }
                }
                if !event::poll(Duration::ZERO).map_err(|e| e.to_string())? {
                    break;
                }
            }
        }
        if app.should_quit {
            manager.stop().await;
            return Ok(());
        }
    }
}

fn draw(frame: &mut Frame, app: &App, manager: &Manager) {
    let area = frame.area();
    if area.width < 58 || area.height < 14 {
        frame.render_widget(
            Paragraph::new(app.tr("Terminal too small; resize the window")).wrap(Wrap { trim: true }),
            area,
        );
        return;
    }
    let [header, nav, body, footer] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Length(2),
            Constraint::Min(5),
            Constraint::Length(2),
        ])
        .areas(area);
    let running = manager.is_running();
    let title = format!(
        " Bootah TUI {} ({})  {} {} ({})  {}  {}  ",
        env!("BOOTAH_TUI_VERSION"), env!("BOOTAH_TUI_BUILD"),
        app.tr("Core version"), env!("BOOTAH_CORE_VERSION"), env!("BOOTAH_CORE_BUILD"),
        app.tr(&app.label()),
        app.tr(if running { "Running" } else { "Stopped" })
    );
    frame.render_widget(
        Paragraph::new(title)
            .style(
                Style::default()
                    .fg(if running { Color::Green } else { Color::Yellow })
                    .add_modifier(Modifier::BOLD),
            )
            .block(Block::default().borders(Borders::BOTTOM)),
        header,
    );
    let pages = [Page::Devices, Page::Profiles, Page::Activity, Page::Network];
    let navline = Line::from(
        pages
            .iter()
            .map(|p| {
                let text = format!(
                    " {} ",
                    app.tr(match p {
                        Page::Devices => "Devices",
                        Page::Profiles => "Images",
                        Page::Activity => "Activity",
                        Page::Network => "Network settings",
                    })
                );
                Span::styled(
                    text,
                    if *p == app.page {
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
                    } else {
                        Style::default().fg(Color::Gray)
                    },
                )
            })
            .collect::<Vec<_>>(),
    );
    frame.render_widget(Paragraph::new(navline), nav);
    draw_page(frame, body, app, manager);
    let keys = match app.page {
        Page::Devices => "←/→ Pages   ↑/↓ Select   Enter Assign image   a Add   d Delete   q Quit",
        Page::Profiles => {
            "←/→ Pages   ↑/↓ Select   PgUp/PgDn Defaults   v Details   i Import ISO   d Default images   q Quit"
        }
        Page::Activity => "←/→ Pages   ↑/↓ Scroll   Home Top   c Clear logs   q Quit",
        Page::Network => "←/→ Pages   e Edit settings   s Start/stop services   q Quit",
    };
    let footer_text = if app.job_running {
        app.tr("Working…")
    } else if app.notice.is_empty() {
        app.tr(keys)
    } else {
        format!("{}  |  {}", app.tr(keys), app.notice)
    };
    frame.render_widget(
        Paragraph::new(footer_text)
            .style(Style::default().fg(Color::DarkGray))
            .block(Block::default().borders(Borders::TOP)),
        footer,
    );
    if let Some(dialog) = &app.dialog {
        draw_dialog(frame, app, dialog, manager);
    }
}

fn draw_page(frame: &mut Frame, area: Rect, app: &App, manager: &Manager) {
    match app.page {
        Page::Devices => {
            let store = manager.state.lock().unwrap();
            let rows = store
                .config
                .devices
                .iter()
                .map(|d| {
                    let profile = d
                        .profile_id
                        .as_ref()
                        .and_then(|id| store.config.profiles.iter().find(|p| &p.id == id))
                        .map(|p| p.name.as_str())
                        .unwrap_or("—");
                    Row::new(vec![
                        if d.name.is_empty() {
                            if d.mac.is_empty() {
                                d.ip.map(|ip| ip.to_string()).unwrap_or_default()
                            } else {
                                d.mac.clone()
                            }
                        } else {
                            d.name.clone()
                        },
                        d.mac.clone(),
                        d.ip.map(|v| v.to_string()).unwrap_or_default(),
                        d.architecture.clone(),
                        profile.into(),
                    ])
                })
                .collect::<Vec<_>>();
            let table = Table::new(
                rows,
                [
                    Constraint::Percentage(22),
                    Constraint::Percentage(21),
                    Constraint::Percentage(18),
                    Constraint::Percentage(17),
                    Constraint::Percentage(22),
                ],
            )
            .header(
                Row::new([
                    app.tr("Name"),
                    app.tr("MAC"),
                    app.tr("Address"),
                    app.tr("Architecture"),
                    app.tr("Image"),
                ])
                .style(Style::default().add_modifier(Modifier::BOLD)),
            )
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(app.tr("Devices")),
            )
            .row_highlight_style(Style::default().bg(Color::DarkGray).fg(Color::White));
            let mut state = TableState::default();
            state.select(if rows_len(&store.config.devices) == 0 {
                None
            } else {
                Some(app.selected.min(store.config.devices.len() - 1))
            });
            frame.render_stateful_widget(table, area, &mut state);
        }
        Page::Profiles => {
            let store = manager.state.lock().unwrap();
            let defaults = &store.config.default_profiles;
            let items = store
                .config
                .profiles
                .iter()
                .map(|p| {
                    let default_for = defaults
                        .iter()
                        .filter(|(_, id)| **id == p.id)
                        .map(|(arch, _)| arch.as_str())
                        .collect::<Vec<_>>()
                        .join(", ");
                    ListItem::new(vec![
                        Line::from(Span::styled(
                            format!("{}  {}  {}", p.name, p.architecture, p.kind),
                            Style::default().add_modifier(Modifier::BOLD),
                        )),
                        Line::from(format!(
                            "{}  {}  {}",
                            p.root.display(),
                            app.tr("Default architecture"),
                            if default_for.is_empty() {
                                "—".into()
                            } else {
                                default_for
                            }
                        )),
                    ])
                })
                .collect::<Vec<_>>();
            let list = List::new(items)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(app.tr("Images & profiles")),
                )
                .highlight_style(Style::default().bg(Color::DarkGray));
            let mut state = ratatui::widgets::ListState::default();
            state.select(
                (!store.config.profiles.is_empty())
                    .then_some(app.selected.min(store.config.profiles.len() - 1)),
            );
            let sections = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
                .split(area);
            let summary = FIRMWARE_KEYS
                .iter()
                .filter(|key| {
                    store
                        .config
                        .profiles
                        .iter()
                        .any(|p| p.boot_files.contains_key(**key))
                })
                .map(|key| {
                    let name = defaults
                        .get(*key)
                        .and_then(|id| store.config.profiles.iter().find(|p| &p.id == id))
                        .map(|p| p.name.clone())
                        .unwrap_or_else(|| app.tr("Not set"));
                    Line::from(format!("{key:<18} {name}"))
                })
                .collect::<Vec<_>>();
            let defaults_scroll = app
                .defaults_scroll
                .min(summary.len().saturating_sub(1) as u16);
            frame.render_widget(
                Paragraph::new(summary)
                    .scroll((defaults_scroll, 0))
                    .wrap(Wrap { trim: false })
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .title(app.tr("Default boot image by architecture")),
                    ),
                sections[0],
            );
            frame.render_stateful_widget(list, sections[1], &mut state);
        }
        Page::Activity => {
            let store = manager.state.lock().unwrap();
            let items = store
                .events
                .iter()
                .rev()
                .map(|e| {
                    ListItem::new(format!(
                        "{}  {}  [{}]  {}",
                        e.time, e.device, e.protocol, e.message
                    ))
                })
                .collect::<Vec<_>>();
            let list = List::new(items)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(app.tr("Activity (newest first)")),
                )
                .highlight_style(Style::default().bg(Color::DarkGray));
            let mut state = ratatui::widgets::ListState::default();
            state.select(
                (!store.events.is_empty()).then_some(app.selected.min(store.events.len() - 1)),
            );
            frame.render_stateful_widget(list, area, &mut state);
        }
        Page::Network => {
            let s = manager.state.lock().unwrap().config.settings.clone();
            let text = format!(
                "{}: {}\n{}: {}\n{}: {} ({})\n{}: {}\n{}: {}\n{}: {}\n\nDHCP: {}  ProxyDHCP: {}\n{}: {}  {}: {}  PXE: {}\n\n{}",
                app.tr("Server IPv4"),
                s.server_ip,
                app.tr("Listen IPv4"),
                s.bind_ip,
                app.tr("Network interface"),
                if s.interface.is_empty() {
                    "—"
                } else {
                    &s.interface
                },
                s.server_mac,
                app.tr("Transport"),
                s.boot_transport,
                app.tr("Address pool start"),
                s.pool_start,
                app.tr("Address pool end"),
                s.pool_end,
                s.dhcp_enabled,
                s.proxy_dhcp_enabled,
                app.tr("DHCP port"),
                s.dhcp_port,
                app.tr("TFTP port"),
                s.tftp_port,
                s.pxe_port,
                app.tr(
                    match (
                        manager.network.lock().unwrap().show_interface_selector,
                        app.authorization_available
                    ) {
                        (true, true) => "Press e to edit, i to select an interface, r to check, p to check permissions, s to start/stop",
                        (false, true) => "Press e to edit, r to check, p to check permissions, s to start/stop",
                        (true, false) => "Press e to edit, i to select an interface, r to check, s to start/stop",
                        (false, false) => "Press e to edit, r to check, s to start/stop",
                    }
                )
            );
            let smb = manager.smb_status_value();
            let text = format!(
                "{}\n\nSMB: {}  {}:{}\n{}",
                text,
                smb["mode"].as_str().unwrap_or("off"),
                smb["bind_ip"].as_str().unwrap_or(""),
                smb["port"],
                app.tr("Press m to choose the Windows installation source service")
            );
            let report = manager.network.lock().unwrap();
            let text = format!(
                "{}: {}\n{}  {}  {}\n\n{}",
                app.tr("Automatic DHCP mode"),
                s.dhcp_auto,
                app.tr(&report.status),
                report.selected,
                report
                    .servers
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", "),
                text
            );
            frame.render_widget(
                Paragraph::new(text).wrap(Wrap { trim: false }).block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(app.tr("Network settings")),
                ),
                area,
            );
        }
    }
}
fn interface_choices(manager: &Manager) -> Vec<(network::Interface, Option<network::Address>)> {
    manager
        .network
        .lock()
        .unwrap()
        .interfaces
        .iter()
        .filter(|i| !i.mac.is_empty())
        .flat_map(|i| {
            if i.addresses.is_empty() {
                vec![(i.clone(), None)]
            } else {
                i.addresses
                    .iter()
                    .map(|a| (i.clone(), Some(a.clone())))
                    .collect()
            }
        })
        .collect()
}
fn detect_job(app: &mut App, manager: &Manager, request: network::Detection) {
    if app.job_running || manager.is_running() {
        return;
    }
    let manager = manager.clone();
    app.job_running = true;
    app.notice = app.tr("Checking…");
    app.job = Some(tokio::spawn(async move {
        manager.detect_network(request).await.map(|r| r.status)
    }));
}
fn rows_len<T>(items: &[T]) -> usize {
    items.len()
}

fn draw_dialog(frame: &mut Frame, app: &App, dialog: &Dialog, manager: &Manager) {
    let area = centered(
        frame.area(),
        72,
        (if matches!(dialog.kind, DialogKind::PermissionDetails) {
            18
        } else {
            dialog.fields.len() as u16 + 6
        })
        .min(frame.area().height.saturating_sub(2)),
    );
    frame.render_widget(Clear, area);
    if matches!(
        dialog.kind,
        DialogKind::ImageDetails | DialogKind::PermissionDetails
    ) {
        let lines = dialog
            .fields
            .iter()
            .skip(dialog.focus)
            .flat_map(|(label, value)| {
                [
                    Line::from(Span::styled(
                        app.tr(label),
                        Style::default().fg(Color::Cyan),
                    )),
                    Line::from(value.clone()),
                    Line::from(""),
                ]
            })
            .collect::<Vec<_>>();
        frame.render_widget(
            Paragraph::new(lines).wrap(Wrap { trim: false }).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(
                        app.tr(if matches!(dialog.kind, DialogKind::PermissionDetails) {
                            "Network permissions"
                        } else {
                            "Image details"
                        }),
                    )
                    .title_bottom(app.tr("↑/↓ Scroll   Enter/Esc Close")),
            ),
            area,
        );
        return;
    }
    if matches!(dialog.kind, DialogKind::Interface) {
        let choices = interface_choices(manager);
        let items: Vec<_> = choices
            .iter()
            .map(|(i, a)| {
                ListItem::new(format!(
                    "{}  {}  {}{}",
                    i.name,
                    a.as_ref()
                        .map(|a| a.ip.to_string())
                        .unwrap_or_else(|| app.tr("No IPv4")),
                    app.tr(if i.up { "Connected" } else { "Interface disconnected" }),
                    if i.physical {
                        String::new()
                    } else {
                        format!("  {}", app.tr("Virtual interface"))
                    }
                ))
            })
            .collect();
        let mut state = ratatui::widgets::ListState::default();
        state.select(Some(dialog.focus));
        let area = centered(frame.area(), 78, 20);
        frame.render_widget(Clear, area);
        frame.render_stateful_widget(
            List::new(items)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(app.tr("Select network interface")),
                )
                .highlight_style(Style::default().bg(Color::DarkGray)),
            area,
            &mut state,
        );
        return;
    }
    let title = app.tr(match dialog.kind {
        DialogKind::NewDevice => "Add device",
        DialogKind::EditDevice => "Edit device",
        DialogKind::Import => "Import ISO",
        DialogKind::Network => "Network settings",
        DialogKind::Assign => "Assign image",
        DialogKind::DefaultProfile => "Set default image",
        DialogKind::ImageDetails => "Image details",
        DialogKind::PermissionDetails => "Network permissions",
        DialogKind::DeleteDevice => "Delete device?",
        DialogKind::DeviceConflict => "Remove old device record?",
        DialogKind::Interface => "Select network interface",
        DialogKind::Smb => "Windows installation source",
        DialogKind::SmbConfirm => "Stop the current installation source service?",
    });
    let lines = dialog
        .fields
        .iter()
        .enumerate()
        .map(|(i, (label, value))| {
            let value = if matches!(dialog.kind, DialogKind::Smb) {
                match value.as_str() {
                    "auto" => app.tr("Automatic (by image)"),
                    "smb1" => "SMB1 — XP / Server 2003".into(),
                    "smb2" => "SMB2 — Vista / Server 2008+".into(),
                    _ => app.tr("Close"),
                }
            } else if i == 1
                && matches!(dialog.kind, DialogKind::Assign | DialogKind::DefaultProfile)
            {
                let choice = value.parse::<usize>().unwrap_or(0);
                let store = manager.state.lock().unwrap();
                if choice == 0 {
                    app.tr(if matches!(dialog.kind, DialogKind::Assign) {
                        "Unassigned"
                    } else {
                        "Not set"
                    })
                } else {
                    store
                        .config
                        .profiles
                        .get(choice.saturating_sub(1))
                        .map(|profile| profile.name.clone())
                        .unwrap_or_else(|| app.tr("No available images"))
                }
            } else {
                value.clone()
            };
            Line::from(vec![
                Span::styled(
                    format!("{}: ", app.tr(label)),
                    Style::default().fg(Color::Cyan),
                ),
                Span::styled(
                    value,
                    if i == dialog.focus || matches!(dialog.kind, DialogKind::Smb) {
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::UNDERLINED)
                    } else {
                        Style::default()
                    },
                ),
            ])
        })
        .collect::<Vec<_>>();
    let help = if matches!(dialog.kind, DialogKind::SmbConfirm) {
        "y Stop and switch   n Cancel"
    } else if matches!(dialog.kind, DialogKind::Smb) {
        "↑/↓ Select   Enter Confirm   Esc Cancel"
    } else if matches!(dialog.kind, DialogKind::DeviceConflict) {
        "y Remove   n Keep"
    } else if matches!(dialog.kind, DialogKind::Assign | DialogKind::DefaultProfile) {
        "↑/↓ Select   Enter Confirm   Esc Cancel"
    } else {
        "Tab/Shift+Tab Move   Enter Next/Save   Space Toggle   Esc Cancel"
    };
    let mut all = lines;
    all.push(Line::from(""));
    all.push(Line::from(app.tr(help)));
    frame.render_widget(
        Paragraph::new(all)
            .block(Block::default().borders(Borders::ALL).title(title))
            .wrap(Wrap { trim: true }),
        area,
    );
}
fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width.saturating_sub(2));
    let height = height.min(area.height.saturating_sub(2));
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

async fn handle_key(app: &mut App, key: KeyEvent, manager: &Manager) {
    if app.job_running && matches!(key.code, KeyCode::Char('r' | 'p' | 'i' | 'e' | 's')) {
        return;
    }
    if app
        .dialog
        .as_ref()
        .is_some_and(|d| matches!(d.kind, DialogKind::Interface))
    {
        let choices = interface_choices(manager);
        let dialog = app.dialog.as_mut().unwrap();

        match key.code {
            KeyCode::Esc => app.dialog = None,
            KeyCode::Up if !choices.is_empty() => {
                dialog.focus = (dialog.focus + choices.len() - 1) % choices.len()
            }
            KeyCode::Down if !choices.is_empty() => {
                dialog.focus = (dialog.focus + 1) % choices.len()
            }
            KeyCode::Enter if !choices.is_empty() => {
                let (i, a) = choices[dialog.focus].clone();
                app.dialog = None;
                detect_job(
                    app,
                    manager,
                    network::Detection {
                        interface: Some(i.id),
                        address: a.map(|a| a.ip),
                        ..Default::default()
                    },
                );
            }
            _ => {}
        }
        return;
    }
    if app.dialog.is_none() && app.page == Page::Network {
        match key.code {
            KeyCode::Char('r' | 'p') => {
                if key.code == KeyCode::Char('p') && !app.authorization_available {
                    return;
                }
                detect_job(
                    app,
                    manager,
                    network::Detection {
                        authorize: key.code == KeyCode::Char('p'),
                        ..Default::default()
                    },
                );
                return;
            }
            KeyCode::Char('i')
                if !manager.is_running()
                    && manager.network.lock().unwrap().show_interface_selector =>
            {
                app.dialog = Some(Dialog {
                    kind: DialogKind::Interface,
                    fields: vec![],
                    focus: 0,
                });
                return;
            }
            _ => {}
        }
    }

    if app.dialog.is_some() {
        handle_dialog(app, key, manager).await;
        return;
    }
    if key.code == KeyCode::Char('q')
        || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
    {
        app.should_quit = true;
        return;
    }
    if matches!(key.code, KeyCode::Left | KeyCode::Right) {
        let pages = [Page::Devices, Page::Profiles, Page::Activity, Page::Network];
        let index = pages.iter().position(|p| *p == app.page).unwrap_or(0) as i32;
        app.page =
            pages[(index + if key.code == KeyCode::Right { 1 } else { 3 }) as usize % pages.len()];
        app.selected = 0;
        app.notice.clear();
        return;
    }
    if app.page == Page::Profiles && matches!(key.code, KeyCode::PageUp | KeyCode::PageDown) {
        app.defaults_scroll = if key.code == KeyCode::PageUp {
            app.defaults_scroll.saturating_sub(1)
        } else {
            (app.defaults_scroll + 1).min(FIRMWARE_KEYS.len().saturating_sub(1) as u16)
        };
        return;
    }
    let count = {
        let s = manager.state.lock().unwrap();
        match app.page {
            Page::Devices => s.config.devices.len(),
            Page::Profiles => s.config.profiles.len(),
            Page::Activity => s.events.len(),
            Page::Network => 1,
        }
    };
    match key.code {
        KeyCode::Home if app.page == Page::Activity => app.selected = 0,
        KeyCode::Char('c') if app.page == Page::Activity => {
            match manager.state.lock().unwrap().clear_events() {
                Ok(()) => app.selected = 0,
                Err(e) => app.notice = e,
            }
        }
        KeyCode::Up => app.selected = app.selected.saturating_sub(1),
        KeyCode::Down => app.selected = app.selected.saturating_add(1).min(count.saturating_sub(1)),
        KeyCode::Char('a') if app.page == Page::Devices => {
            app.dialog = Some(Dialog {
                kind: DialogKind::NewDevice,
                fields: vec![
                    ("MAC".into(), String::new()),
                    ("IP".into(), String::new()),
                    ("Name".into(), String::new()),
                ],
                focus: 0,
            })
        }
        KeyCode::Char('e') if app.page == Page::Devices && count > 0 => {
            let s = manager.state.lock().unwrap();
            let d = &s.config.devices[app.selected.min(s.config.devices.len() - 1)];
            app.dialog = Some(Dialog {
                kind: DialogKind::EditDevice,
                fields: vec![
                    ("Name".into(), d.name.clone()),
                    ("MAC".into(), d.mac.clone()),
                    (
                        "IP".into(),
                        d.ip.map(|ip| ip.to_string()).unwrap_or_default(),
                    ),
                    ("Model".into(), d.model.clone()),
                ],
                focus: 0,
            });
        }
        KeyCode::Char('v') if app.page == Page::Profiles && count > 0 => {
            let store = manager.state.lock().unwrap();
            let p = &store.config.profiles[app.selected.min(count - 1)];
            let mut fields = vec![
                ("Name".into(), p.name.clone()),
                ("Architecture".into(), p.architecture.clone()),
                ("Type".into(), p.kind.clone()),
                ("Directory".into(), p.root.display().to_string()),
                ("Image file".into(), p.media.clone()),
            ];
            if let Some(e) = &p.boot_evidence {
                fields.extend([
                    ("Entry".into(), e.entry.clone()),
                    ("Boot configuration".into(), e.config.clone()),
                    ("Kernel".into(), e.kernel.clone()),
                    ("Initrd".into(), e.initrd.clone()),
                    (
                        "Installation source".into(),
                        app.tr(match e.source.as_str() {
                            "http-tree" => "HTTP install tree",
                            "http-iso" => "HTTP ISO",
                            "nfs-tree" => "Read-only NFS file tree",
                            "nbd" => "Read-only NBD ISO",
                            "local-media" => "Local media required",
                            "external" => "External installation source",
                            _ => "Unrecognized installation source",
                        }),
                    ),
                ]);
            }
            fields.push(("Arguments".into(), p.arguments.clone()));
            fields.extend(p.boot_files.iter().map(|(k, v)| (k.clone(), v.clone())));
            fields.retain(|(_, v)| !v.is_empty());
            app.dialog = Some(Dialog {
                kind: DialogKind::ImageDetails,
                fields,
                focus: 0,
            });
        }
        KeyCode::Char('i') if app.page == Page::Profiles => {
            app.dialog = Some(Dialog {
                kind: DialogKind::Import,
                fields: vec![
                    ("ISO path".into(), String::new()),
                    ("Trust unsigned source?".into(), "n".into()),
                ],
                focus: 0,
            })
        }
        KeyCode::Enter if app.page == Page::Devices && count > 0 => {
            let s = manager.state.lock().unwrap();
            let device = &s.config.devices[app.selected.min(s.config.devices.len() - 1)];
            let mut fields = vec![
                (
                    "Device".into(),
                    if device.name.is_empty() {
                        device.mac.clone()
                    } else {
                        device.name.clone()
                    },
                ),
                ("Image".into(), "Unassigned".into()),
            ];
            if let Some(p) = device
                .profile_id
                .as_ref()
                .and_then(|id| s.config.profiles.iter().position(|p| &p.id == id))
            {
                fields[1].1 = (p + 1).to_string();
            }
            app.dialog = Some(Dialog {
                kind: DialogKind::Assign,
                fields,
                focus: 1,
            });
        }
        KeyCode::Char('d') if app.page == Page::Devices && count > 0 => {
            app.dialog = Some(Dialog {
                kind: DialogKind::DeleteDevice,
                fields: vec![("Confirm device deletion (y/n)".into(), "n".into())],
                focus: 0,
            })
        }
        KeyCode::Char('d') if app.page == Page::Profiles => {
            let s = manager.state.lock().unwrap();
            let architecture = FIRMWARE_KEYS[0].to_string();
            let choice = s
                .config
                .default_profiles
                .get(&architecture)
                .and_then(|id| s.config.profiles.iter().position(|p| &p.id == id))
                .map(|i| i + 1)
                .unwrap_or(0);
            app.dialog = Some(Dialog {
                kind: DialogKind::DefaultProfile,
                fields: vec![
                    ("Architecture".into(), architecture),
                    ("Image".into(), choice.to_string()),
                ],
                focus: 0,
            });
        }
        KeyCode::Char('e') if app.page == Page::Network => {
            if manager.is_running() {
                app.notice = app.tr("Stop services first");
            } else {
                let s = manager.state.lock().unwrap();
                let n = &s.config.settings;
                app.dialog = Some(Dialog {
                    kind: DialogKind::Network,
                    fields: vec![
                        ("Server IPv4".into(), n.server_ip.to_string()),
                        ("Listen IPv4".into(), n.bind_ip.to_string()),
                        ("Network interface".into(), n.interface.clone()),
                        ("Adapter MAC".into(), n.server_mac.clone()),
                        ("Transport (http/tftp)".into(), n.boot_transport.clone()),
                        ("Address pool start".into(), n.pool_start.to_string()),
                        ("Address pool end".into(), n.pool_end.to_string()),
                        ("Subnet mask".into(), n.subnet.to_string()),
                        (
                            "DHCP mode (0 off/1 allocate/2 proxy/3 auto)".into(),
                            if n.dhcp_auto {
                                "3"
                            } else if n.dhcp_enabled {
                                "1"
                            } else if n.proxy_dhcp_enabled {
                                "2"
                            } else {
                                "0"
                            }
                            .into(),
                        ),
                        (
                            "Ethernet protocol (y/n)".into(),
                            if n.ethernet_enabled { "y" } else { "n" }.into(),
                        ),
                        (
                            "Router IPv4 (optional)".into(),
                            n.router.map(|ip| ip.to_string()).unwrap_or_default(),
                        ),
                        ("DHCP port".into(), n.dhcp_port.to_string()),
                        ("TFTP port".into(), n.tftp_port.to_string()),
                        ("HTTP port".into(), n.http_port.to_string()),
                        ("PXE port".into(), n.pxe_port.to_string()),
                    ],
                    focus: 0,
                });
            }
        }
        KeyCode::Char('m') if app.page == Page::Network => {
            let state = manager.smb_status_value();
            let modes = ["off", "auto", "smb1", "smb2"];
            app.dialog = Some(Dialog {
                kind: DialogKind::Smb,
                fields: vec![(
                    "Mode".into(),
                    state["configured_mode"].as_str().unwrap_or("off").into(),
                )],
                focus: modes
                    .iter()
                    .position(|m| Some(*m) == state["configured_mode"].as_str())
                    .unwrap_or(0),
            });
        }
        KeyCode::Char('s') if app.page == Page::Network => {
            app.notice.clear();
            if manager.is_running() {
                manager.stop().await;
                app.notice = app.tr("Services stopped");
            } else {
                app.notice = match manager.start().await {
                    Ok(()) => app.tr("Services started"),
                    Err(e) => e,
                };
            }
        }
        _ => {}
    }
}

async fn switch_smb(app: &mut App, manager: &Manager, mode: String, expected: Option<String>) {
    match manager
        .switch_smb(mode.clone(), expected.is_some(), expected)
        .await
    {
        Ok(state) => app.notice = format!("SMB: {}", state["mode"].as_str().unwrap_or("off")),
        Err((status, detail))
            if status == axum::http::StatusCode::CONFLICT
                && detail["code"] == "smb_switch_confirmation" =>
        {
            let message = detail[if app.language.starts_with("zh") {
                "message_zh"
            } else {
                "message"
            }]
            .as_str()
            .unwrap_or("")
            .to_string();
            app.dialog = Some(Dialog {
                kind: DialogKind::SmbConfirm,
                focus: 0,
                fields: vec![
                    (
                        "Current mode".into(),
                        detail["current_mode"].as_str().unwrap_or("off").into(),
                    ),
                    ("Target mode".into(), mode),
                    ("Warning".into(), message),
                ],
            });
        }
        Err((_, detail)) => {
            app.notice = detail["message"]
                .as_str()
                .unwrap_or("SMB service failed")
                .into()
        }
    }
}

async fn handle_dialog(app: &mut App, key: KeyEvent, manager: &Manager) {
    let dialog = app.dialog.as_mut().unwrap();
    if matches!(dialog.kind, DialogKind::Smb) {
        let modes = ["off", "auto", "smb1", "smb2"];
        match key.code {
            KeyCode::Esc => app.dialog = None,
            KeyCode::Up => {
                dialog.focus = (dialog.focus + modes.len() - 1) % modes.len();
                dialog.fields[0].1 = modes[dialog.focus].into();
            }
            KeyCode::Down => {
                dialog.focus = (dialog.focus + 1) % modes.len();
                dialog.fields[0].1 = modes[dialog.focus].into();
            }
            KeyCode::Enter => {
                let mode = modes[dialog.focus].to_string();
                app.dialog = None;
                switch_smb(app, manager, mode, None).await;
            }
            _ => {}
        }
        return;
    }
    if matches!(dialog.kind, DialogKind::SmbConfirm) {
        match key.code {
            KeyCode::Char('n') | KeyCode::Esc => app.dialog = None,
            KeyCode::Char('y') => {
                let mode = dialog.fields[1].1.clone();
                let expected = dialog.fields[0].1.clone();
                app.dialog = None;
                switch_smb(app, manager, mode, Some(expected)).await;
            }
            _ => {}
        }
        return;
    }
    if matches!(dialog.kind, DialogKind::DeviceConflict) {
        let accept = match key.code {
            KeyCode::Char('y') | KeyCode::Enter => true,
            KeyCode::Char('n') | KeyCode::Esc => false,
            _ => return,
        };
        app.dialog = None;
        if let Some(id) = app.active_conflict.take() {
            app.notice = match manager
                .state
                .lock()
                .unwrap()
                .resolve_device_conflict(&id, accept)
            {
                Ok(()) => app.tr(if accept {
                    "Old device record removed"
                } else {
                    "Old device record kept"
                }),
                Err(error) => error,
            };
        }
        return;
    }
    if matches!(
        dialog.kind,
        DialogKind::ImageDetails | DialogKind::PermissionDetails
    ) {
        match key.code {
            KeyCode::Esc | KeyCode::Enter => {
                app.dialog = None;
            }
            KeyCode::Down => {
                dialog.focus = (dialog.focus + 1).min(dialog.fields.len().saturating_sub(1));
            }
            KeyCode::Up => {
                dialog.focus = dialog.focus.saturating_sub(1);
            }
            _ => {}
        }
        return;
    }
    match key.code {
        KeyCode::Esc => {
            app.dialog = None;
            return;
        }
        KeyCode::Tab | KeyCode::Char('\t') | KeyCode::Down
            if !matches!(dialog.kind, DialogKind::Assign | DialogKind::DefaultProfile) =>
        {
            dialog.focus = (dialog.focus + 1) % dialog.fields.len();
            return;
        }
        KeyCode::BackTab | KeyCode::Up
            if !matches!(dialog.kind, DialogKind::Assign | DialogKind::DefaultProfile) =>
        {
            dialog.focus = (dialog.focus + dialog.fields.len() - 1) % dialog.fields.len();
            return;
        }
        KeyCode::Tab | KeyCode::Char('\t') | KeyCode::BackTab
            if matches!(dialog.kind, DialogKind::DefaultProfile) =>
        {
            dialog.focus = 1 - dialog.focus;
            return;
        }
        KeyCode::Up | KeyCode::Down
            if matches!(dialog.kind, DialogKind::DefaultProfile) && dialog.focus == 0 =>
        {
            let index = FIRMWARE_KEYS
                .iter()
                .position(|key| *key == dialog.fields[0].1)
                .unwrap_or(0);
            let next = if key.code == KeyCode::Down {
                (index + 1) % FIRMWARE_KEYS.len()
            } else {
                (index + FIRMWARE_KEYS.len() - 1) % FIRMWARE_KEYS.len()
            };
            dialog.fields[0].1 = FIRMWARE_KEYS[next].into();
            let s = manager.state.lock().unwrap();
            dialog.fields[1].1 = s
                .config
                .default_profiles
                .get(FIRMWARE_KEYS[next])
                .and_then(|id| s.config.profiles.iter().position(|p| &p.id == id))
                .map(|i| i + 1)
                .unwrap_or(0)
                .to_string();
            return;
        }
        KeyCode::Up | KeyCode::Down
            if matches!(dialog.kind, DialogKind::Assign)
                || (matches!(dialog.kind, DialogKind::DefaultProfile) && dialog.focus == 1) =>
        {
            let s = manager.state.lock().unwrap();
            let n = s.config.profiles.len() + 1;
            if n > 0 {
                let value = dialog.fields[1].1.parse::<usize>().unwrap_or(0);
                let mut next = value;
                for _ in 0..n {
                    next = if key.code == KeyCode::Down {
                        (next + 1) % n
                    } else {
                        (next + n - 1) % n
                    };
                    if matches!(dialog.kind, DialogKind::Assign)
                        || next == 0
                        || s.config.profiles[next - 1]
                            .boot_files
                            .contains_key(&dialog.fields[0].1)
                    {
                        break;
                    }
                }
                dialog.fields[1].1 = next.to_string();
            }
            return;
        }
        KeyCode::Char(' ')
            if matches!(dialog.kind, DialogKind::Import)
                || (matches!(dialog.kind, DialogKind::Network) && dialog.focus == 9) =>
        {
            let v = &mut dialog.fields[dialog.focus].1;
            *v = if v.eq_ignore_ascii_case("y") {
                "n".into()
            } else {
                "y".into()
            };
            return;
        }
        KeyCode::Backspace => {
            dialog.fields[dialog.focus].1.pop();
            return;
        }
        KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            if matches!(dialog.kind, DialogKind::DeleteDevice) {
                dialog.fields[dialog.focus].1 = ch.to_string();
            } else {
                dialog.fields[dialog.focus].1.push(ch);
            }
            return;
        }
        KeyCode::Enter | KeyCode::Char('\r') | KeyCode::Char('\n') => {
            if matches!(dialog.kind, DialogKind::DefaultProfile) && dialog.focus == 0 {
                dialog.focus = 1;
                return;
            }
            if dialog.focus + 1 < dialog.fields.len()
                && !matches!(
                    dialog.kind,
                    DialogKind::Assign | DialogKind::DefaultProfile | DialogKind::DeleteDevice
                )
            {
                dialog.focus += 1;
                return;
            }
        }
        KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if matches!(dialog.kind, DialogKind::DefaultProfile) && dialog.focus == 0 {
                dialog.focus = 1;
                return;
            }
            if dialog.focus + 1 < dialog.fields.len()
                && !matches!(
                    dialog.kind,
                    DialogKind::Assign | DialogKind::DefaultProfile | DialogKind::DeleteDevice
                )
            {
                dialog.focus += 1;
                return;
            }
        }
        _ => return,
    }
    let dialog = app.dialog.take().unwrap();
    let result: Result<String, String> = match dialog.kind {
        DialogKind::NewDevice => add_device(manager, &dialog.fields),
        DialogKind::EditDevice => edit_device(manager, app.selected, &dialog.fields),
        DialogKind::Import => {
            let path = dialog.fields[0].1.clone();
            let trust = dialog.fields[1].1.eq_ignore_ascii_case("y");
            let library = manager
                .state
                .lock()
                .unwrap()
                .path
                .parent()
                .unwrap_or(std::path::Path::new("."))
                .join("media");
            let state = manager.state.clone();
            app.job_running = true;
            app.job = Some(tokio::task::spawn_blocking(move || {
                let imported = media::import_with_options(
                    std::path::Path::new(&path),
                    &library,
                    trust,
                    false,
                )?;
                let mut s = state.lock().unwrap();
                let old = s.config.profiles.clone();
                s.config.profiles.extend(imported.profiles.clone());
                if let Err(e) = s.save() {
                    s.config.profiles = old;
                    return Err(e);
                };
                Ok(if imported.warnings.is_empty() {
                    "ISO import complete".into()
                } else {
                    imported.warnings.join("\n")
                })
            }));
            return;
        }
        DialogKind::Interface | DialogKind::Smb | DialogKind::SmbConfirm => unreachable!(),
        DialogKind::Network => match save_network(manager, &dialog.fields) {
            Ok(_) => {
                detect_job(app, manager, network::Detection::default());
                return;
            }
            Err(e) => Err(e),
        },
        DialogKind::Assign => assign_profile(manager, app.selected, &dialog.fields),
        DialogKind::DefaultProfile => set_default(manager, &dialog.fields),
        DialogKind::ImageDetails | DialogKind::PermissionDetails => unreachable!(),
        DialogKind::DeleteDevice => delete_device(manager, app.selected, &dialog.fields),
        DialogKind::DeviceConflict => unreachable!(),
    };
    app.notice = match result {
        Ok(message) => app.tr(&message),
        Err(error) => app.tr(&error),
    };
}

fn input_mac(manager: &Manager, mac: Option<String>, ip: Option<Ipv4Addr>) -> Option<String> {
    if mac.is_some() {
        return mac;
    }
    let ip = ip?;
    let (known, interface) = {
        let store = manager.state.lock().unwrap();
        (
            store
                .config
                .devices
                .iter()
                .find(|d| d.ip == Some(ip) && !d.mac.is_empty())
                .map(|d| d.mac.clone()),
            store.config.settings.interface.clone(),
        )
    };
    known.or_else(|| crate::neighbors::lookup(&interface).remove(&ip))
}
fn add_device(manager: &Manager, fields: &[(String, String)]) -> Result<String, String> {
    let mac = normalize_mac(&fields[0].1);
    let ip = if fields[1].1.trim().is_empty() {
        None
    } else {
        Some(
            fields[1]
                .1
                .trim()
                .parse::<Ipv4Addr>()
                .map_err(|e| e.to_string())?,
        )
    };
    if mac.is_none() && ip.is_none() {
        return Err("A device needs a MAC or IP address".into());
    }
    let mac = input_mac(manager, mac, ip).unwrap_or_default();
    let mut s = manager.state.lock().unwrap();
    if s.config
        .devices
        .iter()
        .any(|d| (!mac.is_empty() && d.mac == mac) || (ip.is_some() && d.ip == ip))
    {
        return Err("Device already exists".into());
    }
    s.config.devices.push(Device {
        id: if mac.is_empty() {
            format!("ip-{}", ip.unwrap())
        } else {
            mac.replace(':', "")
        },
        mac,
        ip,
        name: fields[2].1.trim().into(),
        ..Default::default()
    });
    if let Err(e) = s.save() {
        s.config.devices.pop();
        return Err(e);
    }
    Ok("Device added".into())
}
fn edit_device(
    manager: &Manager,
    index: usize,
    fields: &[(String, String)],
) -> Result<String, String> {
    let mac = normalize_mac(&fields[1].1);
    let ip = if fields[2].1.trim().is_empty() {
        None
    } else {
        Some(
            fields[2]
                .1
                .trim()
                .parse::<Ipv4Addr>()
                .map_err(|e| e.to_string())?,
        )
    };
    if mac.is_none() && ip.is_none() {
        return Err("A device needs a MAC or IP address".into());
    }
    let mac = input_mac(manager, mac, ip);
    let mut s = manager.state.lock().unwrap();
    if s.config.devices.iter().enumerate().any(|(i, d)| {
        i != index
            && ((!mac.as_deref().unwrap_or_default().is_empty()
                && d.mac == mac.as_deref().unwrap_or_default())
                || (ip.is_some() && d.ip == ip))
    }) {
        return Err("Device already exists".into());
    }
    let previous_devices = s.config.devices.clone();
    let previous_auto = s.config.automations.clone();
    if let Some(ip) = ip {
        if let Some(ref mac) = mac {
            s.associate(ip, mac);
        }
    }
    let index = mac
        .as_ref()
        .and_then(|mac| s.config.devices.iter().position(|d| &d.mac == mac))
        .unwrap_or(index);
    let device = s.config.devices.get_mut(index).ok_or("Device not found")?;
    device.name = fields[0].1.trim().into();
    device.mac = mac.unwrap_or_default();
    device.ip = ip;
    device.model = fields[3].1.trim().into();
    device.auto_registered = false;
    if let Err(e) = s.save() {
        s.config.devices = previous_devices;
        s.config.automations = previous_auto;
        return Err(e);
    }
    Ok("Device details saved".into())
}
fn assign_profile(
    manager: &Manager,
    index: usize,
    fields: &[(String, String)],
) -> Result<String, String> {
    let mut s = manager.state.lock().unwrap();
    if s.config.devices.is_empty() {
        return Err("Device not found".into());
    }
    let choice = fields[1].1.parse::<usize>().map_err(|_| "Invalid image selection")?;
    let profile = if choice == 0 {
        None
    } else {
        Some(
            s.config
                .profiles
                .get(choice - 1)
                .ok_or("Invalid image selection")?
                .id
                .clone(),
        )
    };
    let device = s.config.devices.get_mut(index).ok_or("Device not found")?;
    let previous = device.profile_id.clone();
    let previous_auto_registered = device.auto_registered;
    device.profile_id = profile;
    device.auto_registered = false;
    if let Err(e) = s.save() {
        s.config.devices[index].profile_id = previous;
        s.config.devices[index].auto_registered = previous_auto_registered;
        return Err(e);
    }
    Ok("Image assignment saved".into())
}
fn set_default(manager: &Manager, fields: &[(String, String)]) -> Result<String, String> {
    let mut s = manager.state.lock().unwrap();
    let architecture = fields[0].1.clone();
    if !known_firmware_architecture(&architecture) {
        return Err("Invalid architecture".into());
    }
    let choice = fields[1].1.parse::<usize>().map_err(|_| "Invalid image selection")?;
    if choice == 0 {
        let previous = s.config.default_profiles.remove(&architecture);
        if let Err(e) = s.save() {
            if let Some(id) = previous {
                s.config.default_profiles.insert(architecture, id);
            }
            return Err(e);
        }
        return Ok("Default image saved".into());
    }
    let profile = s.config.profiles.get(choice - 1).ok_or("Invalid image selection")?;
    if !profile.boot_files.contains_key(&architecture) {
        return Err("The selected image has no boot file for this architecture".into());
    }
    let profile_id = profile.id.clone();
    let previous = s
        .config
        .default_profiles
        .insert(architecture.clone(), profile_id);
    if let Err(e) = s.save() {
        if let Some(value) = previous {
            s.config.default_profiles.insert(architecture, value);
        } else {
            s.config.default_profiles.remove(&architecture);
        }
        return Err(e);
    }
    Ok("Default image saved".into())
}
fn delete_device(
    manager: &Manager,
    index: usize,
    fields: &[(String, String)],
) -> Result<String, String> {
    if !fields[0].1.eq_ignore_ascii_case("y") {
        return Ok("Cancelled".into());
    }
    let mut s = manager.state.lock().unwrap();
    if index >= s.config.devices.len() {
        return Err("Device not found".into());
    }
    let removed = s.config.devices.remove(index);
    if let Err(e) = s.save() {
        s.config.devices.insert(index, removed);
        return Err(e);
    }
    Ok("Device deleted".into())
}
fn save_network(manager: &Manager, fields: &[(String, String)]) -> Result<String, String> {
    let parse_ip = |i: usize| fields[i].1.parse::<Ipv4Addr>().map_err(|e| e.to_string());
    let number = |i: usize| fields[i].1.parse::<u16>().map_err(|e| e.to_string());
    let mut s = manager.state.lock().unwrap();
    if manager.is_running() {
        return Err("Stop services first".into());
    }
    let mut n = s.config.settings.clone();
    n.server_ip = parse_ip(0)?;
    n.bind_ip = parse_ip(1)?;
    n.interface = fields[2].1.clone();
    n.server_mac = if fields[3].1.is_empty() {
        String::new()
    } else {
        normalize_mac(&fields[3].1).ok_or("Invalid MAC address")?
    };
    n.boot_transport = fields[4].1.to_ascii_lowercase();
    if !matches!(n.boot_transport.as_str(), "http" | "tftp") {
        return Err("Transport must be http or tftp".into());
    }
    n.pool_start = parse_ip(5)?;
    n.pool_end = parse_ip(6)?;
    n.subnet = parse_ip(7)?;
    let mode = fields[8].1.parse::<u8>().map_err(|e| e.to_string())?;
    if mode > 3 {
        return Err("Invalid DHCP mode".into());
    }
    n.dhcp_auto = mode == 3;
    n.dhcp_enabled = mode == 1;
    n.proxy_dhcp_enabled = mode == 2;
    n.ethernet_enabled = fields[9].1.eq_ignore_ascii_case("y");
    n.router = if fields[10].1.trim().is_empty() {
        None
    } else {
        Some(parse_ip(10)?)
    };
    n.dhcp_port = number(11)?;
    n.tftp_port = number(12)?;
    n.http_port = number(13)?;
    n.pxe_port = number(14)?;
    if n.http_port == 0
        || n.tftp_port == 0
        || n.dhcp_port == 0
        || n.pxe_port == 0
        || n.pxe_port == n.dhcp_port
        || n.pxe_port == n.tftp_port
        || u32::from(n.pool_end) < u32::from(n.pool_start)
    {
        return Err("Invalid network settings (check ports and address pool)".into());
    }
    let previous = std::mem::replace(&mut s.config.settings, n);
    if let Err(e) = s.save() {
        s.config.settings = previous;
        return Err(e);
    }
    Ok("Network settings saved".into())
}
