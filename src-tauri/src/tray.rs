use crate::lifecycle::{ManagedProject, Phase, ProcessMap, SHUTTING_DOWN};
use crate::terminal::{TerminalKey, TerminalManager};
use crate::terminal_agent::AgentSession;
use crate::ProjectConfig;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Mutex,
};
use tauri::{
    image::Image,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, Rect, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

pub const ID: &str = "apphub-tray";
pub const PANEL: &str = "bento-tray-panel";
const RECENT_KEY: &str = "tray.recentProjects";
const PANEL_WIDTH: f64 = 320.0;
const PROJECT_PREFIX: &str = "apphub-tray-project-";
const STOP_ALL: &str = "apphub-tray-stop-all";
const SHOW: &str = "apphub-tray-show";
const QUIT: &str = "apphub-tray-quit";

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrayProject {
    id: String,
    name: String,
    port: Option<u16>,
    starting: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PanelProject {
    id: String,
    name: String,
    port: Option<u16>,
    status: &'static str,
    started_at: Option<u64>,
    agent_detail: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PanelView {
    running: usize,
    can_stop: bool,
    rows: Vec<PanelProject>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TrayItem {
    pub id: String,
    pub text: String,
    pub enabled: bool,
    pub separator: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TrayView {
    pub title: String,
    pub tooltip: String,
    pub projects: Vec<TrayProject>,
    pub items: Vec<TrayItem>,
    pub panel: PanelView,
}

impl TrayView {
    fn from_snapshot(projects: &[ProjectConfig], managed: &[ManagedProject]) -> Self {
        let mut remaining: HashMap<_, _> = managed
            .iter()
            .map(|project| (project.id.as_str(), project))
            .collect();
        let mut entries = Vec::new();
        for project in projects {
            if let Some(info) = remaining.remove(project.id.as_str()) {
                entries.push(TrayProject {
                    id: project.id.clone(),
                    name: project
                        .display_name
                        .as_ref()
                        .filter(|name| !name.is_empty())
                        .unwrap_or(&project.name)
                        .clone(),
                    port: info.port,
                    starting: info.phase == Phase::Starting,
                });
            }
        }
        // A CLI deletion can leave a managed process without its configuration.
        // Keep it reachable and stoppable until the process actually ends.
        for info in managed
            .iter()
            .filter(|info| remaining.contains_key(info.id.as_str()))
        {
            entries.push(TrayProject {
                id: info.id.clone(),
                name: info.id.clone(),
                port: info.port,
                starting: info.phase == Phase::Starting,
            });
        }
        let count = managed.iter().filter(|info| info.phase == Phase::Running).count();
        let item = |id: &str, text: String, enabled| TrayItem {
            id: id.into(),
            text,
            enabled,
            separator: false,
        };
        let separator = |index| TrayItem {
            id: format!("apphub-tray-separator-{index}"),
            text: String::new(),
            enabled: false,
            separator: true,
        };
        let mut items = vec![item(
            "apphub-tray-summary",
            if count == 0 {
                "没有运行中的项目".into()
            } else {
                format!("Bento 启动的项目 · {count} 个运行中")
            },
            false,
        )];
        for project in &entries {
            let port = project
                .port
                .map(|port| format!("  :{port}"))
                .unwrap_or_default();
            let phase = if project.starting {
                "（启动中）"
            } else {
                ""
            };
            items.push(item(
                &format!("{PROJECT_PREFIX}{}", project.id),
                format!("{}{port}{phase}", project.name),
                true,
            ));
        }
        items.extend([
            separator(1),
            item(STOP_ALL, "全部停止…".into(), !managed.is_empty()),
            item(SHOW, "显示 Bento".into(), true),
            separator(2),
            item(QUIT, "退出 Bento".into(), true),
        ]);
        Self {
            title: if count == 0 {
                String::new()
            } else {
                count.to_string()
            },
            tooltip: format!("Bento · {count} 个项目运行中"),
            projects: entries,
            items,
            panel: PanelView::default(),
        }
    }

    fn confirmation(&self) -> String {
        format!(
            "将停止以下由 Bento 启动的项目：{}。外部启动的进程不受影响。",
            self.projects
                .iter()
                .map(|project| project.name.as_str())
                .collect::<Vec<_>>()
                .join("、")
        )
    }
}

fn panel_view(projects: &[ProjectConfig], managed: &[ManagedProject], recent: &[String], agents: &[(TerminalKey, AgentSession)]) -> PanelView {
    let mut active: Vec<_> = managed.iter().collect();
    active.sort_by_key(|info| info.phase != Phase::Running);
    let mut by_project = HashMap::<&str, (u8, &AgentSession)>::new();
    for (key, agent) in agents {
        let rank = |slot, agent: &AgentSession| (agent.priority(), agent.state_started_at, slot);
        if by_project.get(key.project_id.as_str()).is_none_or(|&(slot, previous)| rank(key.slot, agent) < rank(slot, previous)) {
            by_project.insert(&key.project_id, (key.slot, agent));
        }
    }
    let agent_detail = |id: &str| by_project.get(id).map(|(_, agent)| {
        format!("{} · {}", agent.kind.label(), agent.state.label())
    });
    let name = |id: &str| {
        projects.iter().find(|project| project.id == id).map(|project| {
            project.display_name.as_ref().filter(|name| !name.is_empty()).unwrap_or(&project.name).clone()
        })
    };
    let agent_projects = projects.iter().filter(|project| by_project.contains_key(project.id.as_str())).map(|project| &project.id);
    let mut seen: HashSet<_> = managed.iter().map(|project| project.id.as_str()).collect();
    let rows = active.into_iter().map(|info| PanelProject {
        id: info.id.clone(), name: name(&info.id).unwrap_or_else(|| info.id.clone()), port: info.port,
        status: match info.phase { Phase::Running => "running", Phase::Starting => "starting", Phase::PortTimeout => "error" },
        started_at: Some(info.started_at),
        agent_detail: agent_detail(&info.id),
    }).chain(agent_projects.chain(recent.iter()).filter_map(|id| {
        if !seen.insert(id.as_str()) { return None; }
        Some(PanelProject { id: id.clone(), name: name(id)?, port: None, status: "stopped", started_at: None,
            agent_detail: agent_detail(id) })
    })).take(8).collect();
    PanelView { running: managed.iter().filter(|info| info.phase == Phase::Running).count(),
        can_stop: !managed.is_empty(), rows }
}

#[derive(Default)]
pub struct TrayState {
    generation: AtomicU64,
    stop_pending: AtomicBool,
    applied: Mutex<Option<(TrayView, Menu<tauri::Wry>)>>,
    recent: Mutex<Vec<String>>,
    enabled: AtomicBool,
}

fn menu(app: &AppHandle, view: &TrayView) -> tauri::Result<Menu<tauri::Wry>> {
    let menu = Menu::new(app)?;
    for item in &view.items {
        if item.separator {
            menu.append(&PredefinedMenuItem::separator(app)?)?;
        } else {
            menu.append(&MenuItem::with_id(
                app,
                &item.id,
                &item.text,
                item.enabled,
                None::<&str>,
            )?)?;
        }
    }
    Ok(menu)
}

pub fn install(app: &AppHandle) -> tauri::Result<()> {
    let view = TrayView::from_snapshot(&[], &[]);
    let menu = menu(app, &view)?;
    // tray-icon scales the 36px image to 18pt on macOS, preserving Retina detail.
    let tray = TrayIconBuilder::with_id(ID)
        .icon(Image::from_bytes(include_bytes!(
            "../icons/tray-icon@2x.png"
        ))?)
        .icon_as_template(true)
        .title(&view.title)
        .tooltip(&view.tooltip)
        .show_menu_on_left_click(false)
        .menu(&menu)
        .on_tray_icon_event(|icon, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, rect, .. } = event {
                toggle_panel(icon.app_handle(), rect);
            }
        })
        .build(app)?;
    let state = app.state::<TrayState>();
    let mut enabled = true;
    if let Ok(database) = crate::open_project_db() {
        enabled = crate::settings::enabled(&database, crate::settings::SHOW_TRAY).unwrap_or(true);
        if let Ok(value) = database.query_row("SELECT value FROM settings WHERE key=?1", [RECENT_KEY], |row| row.get::<_, String>(0)) {
            if let Ok(recent) = serde_json::from_str::<Vec<String>>(&value) {
                *state.recent.lock().unwrap_or_else(|err| err.into_inner()) = recent.into_iter().take(8).collect();
            }
        }
    }
    state.enabled.store(enabled, Ordering::SeqCst);
    tray.set_visible(enabled)?;
    *app.state::<TrayState>()
        .applied
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = Some((view, menu));
    Ok(())
}

async fn current(app: &AppHandle) -> Result<TrayView, String> {
    let managed = app.state::<ProcessMap>().lock().await.managed_snapshot();
    let projects = tauri::async_runtime::spawn_blocking(crate::get_projects_sync)
        .await
        .map_err(|error| error.to_string())??;
    let recent = app.state::<TrayState>().recent.lock().unwrap_or_else(|err| err.into_inner()).clone();
    let mut view = TrayView::from_snapshot(&projects.projects, &managed);
    let agents = app.state::<TerminalManager>().agents();
    view.panel = panel_view(&projects.projects, &managed, &recent, &agents);
    Ok(view)
}

#[tauri::command]
pub async fn get_tray_panel(app: AppHandle) -> Result<PanelView, String> {
    Ok(current(&app).await?.panel)
}

pub fn set_enabled(app: &AppHandle, enabled: bool) -> Result<(), String> {
    let tray = app.tray_by_id(ID).ok_or("菜单栏图标不可用")?;
    tray.set_visible(enabled).map_err(|error| error.to_string())?;
    app.state::<TrayState>().enabled.store(enabled, Ordering::SeqCst);
    if !enabled {
        if let Some(panel) = app.get_webview_window(PANEL) { let _ = panel.hide(); }
    } else {
        refresh(app);
    }
    Ok(())
}

#[tauri::command]
pub fn note_tray_project(app: AppHandle, project_id: String) -> Result<(), String> {
    let state = app.state::<TrayState>();
    let mut recent = state.recent.lock().unwrap_or_else(|err| err.into_inner());
    if recent.first() == Some(&project_id) { return Ok(()); }
    let conn = crate::open_project_db()?;
    let exists: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM projects WHERE id=?1)", [&project_id], |row| row.get(0))
        .map_err(|error| error.to_string())?;
    if !exists { return Ok(()); }
    let mut next = vec![project_id.clone()];
    next.extend(recent.iter().filter(|id| **id != project_id).take(7).cloned());
    let json = serde_json::to_string(&next).map_err(|error| error.to_string())?;
    conn.execute("INSERT INTO settings(key,value) VALUES(?1,?2)
        ON CONFLICT(key) DO UPDATE SET value=excluded.value, updated_at=CURRENT_TIMESTAMP", [RECENT_KEY, &json])
        .map_err(|error| error.to_string())?;
    *recent = next;
    drop(recent);
    refresh(&app);
    Ok(())
}

fn resize_panel(window: &WebviewWindow, rows: usize) -> tauri::Result<()> {
    window.set_size(LogicalSize::new(PANEL_WIDTH, 104.0 + rows.max(1) as f64 * 44.0))
}

pub fn toggle_panel(app: &AppHandle, icon: Rect) {
    let state = app.state::<TrayState>();
    if !state.enabled.load(Ordering::SeqCst) { return; }
    if let Some(window) = app.get_webview_window(PANEL) {
        if window.is_visible().unwrap_or(false) {
            let _ = window.hide();
            return;
        }
    }
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let rows = current(&handle).await.map_or(0, |view| view.panel.rows.len());
        let app = handle.clone();
        let _ = handle.run_on_main_thread(move || {
            if !app.state::<TrayState>().enabled.load(Ordering::SeqCst) { return; }
            if let Err(error) = show_panel(&app, icon, rows) {
                eprintln!("Bento tray panel: {error}");
            }
        });
    });
}

fn show_panel(app: &AppHandle, icon: Rect, rows: usize) -> tauri::Result<()> {
    let Some(main) = app.get_webview_window("main") else { return Ok(()); };
    let main_scale = main.scale_factor()?;
    let position = icon.position.to_physical::<f64>(main_scale);
    let monitor = main.monitor_from_point(position.x, position.y)?.or(main.primary_monitor()?);
    let scale = monitor.as_ref().map_or(main_scale, |display| display.scale_factor());
    let size = icon.size.to_physical::<f64>(scale);
    let mut x = position.x + size.width / 2.0 - PANEL_WIDTH * scale / 2.0;
    let mut y = position.y + size.height + 6.0 * scale;
    if let Some(display) = monitor {
        let area = display.work_area();
        let left = f64::from(area.position.x);
        let top = f64::from(area.position.y);
        let right = left + f64::from(area.size.width) - (PANEL_WIDTH + 6.0) * scale;
        x = x.clamp(left + 6.0 * scale, right.max(left + 6.0 * scale));
        y = y.max(top + 4.0 * scale);
    }
    let window = match app.get_webview_window(PANEL) {
        Some(window) => window,
        None => WebviewWindowBuilder::new(app, PANEL, WebviewUrl::App("index.html".into()))
            .title("Bento")
            .decorations(false)
            .transparent(true)
            .shadow(true)
            .resizable(false)
            .skip_taskbar(true)
            .always_on_top(true)
            .visible_on_all_workspaces(true)
            .accept_first_mouse(true)
            .theme(crate::settings::saved_window_theme())
            .visible(false)
            .build()?,
    };
    resize_panel(&window, rows)?;
    window.set_position(PhysicalPosition::new(x.round() as i32, y.round() as i32))?;
    let _ = app.emit_to(PANEL, "tray-panel-refresh", ());
    window.show()?;
    window.set_focus()
}

#[tauri::command]
pub fn tray_panel_action(app: AppHandle, action: String, project_id: Option<String>) -> Result<(), String> {
    match action.as_str() {
        "show" => {
            if let Some(panel) = app.get_webview_window(PANEL) { let _ = panel.hide(); }
            if let Some(id) = project_id { handle_menu(&app, &format!("{PROJECT_PREFIX}{id}")); }
            else { handle_menu(&app, SHOW); }
        }
        "stop-all" => request_stop_all(&app),
        "hide" => {
            if let Some(window) = app.get_webview_window(PANEL) { window.hide().map_err(|err| err.to_string())?; }
        }
        _ => return Err("未知的菜单栏操作".into()),
    }
    Ok(())
}

pub fn refresh(app: &AppHandle) {
    let Some(state) = app.try_state::<TrayState>() else {
        return;
    };
    let generation = state.generation.fetch_add(1, Ordering::SeqCst) + 1;
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        if SHUTTING_DOWN.load(Ordering::SeqCst)
            || app.state::<TrayState>().generation.load(Ordering::SeqCst) != generation
        {
            return;
        }
        let view = match current(&app).await {
            Ok(view) => view,
            Err(error) => {
                eprintln!("Bento tray: {error}");
                return;
            }
        };
        let handle = app.clone();
        let _ = app.run_on_main_thread(move || {
            let state = handle.state::<TrayState>();
            if SHUTTING_DOWN.load(Ordering::SeqCst)
                || state.generation.load(Ordering::SeqCst) != generation
            {
                return;
            }
            if let Some(panel) = handle.get_webview_window(PANEL).filter(|panel| panel.is_visible().unwrap_or(false)) {
                let _ = resize_panel(&panel, view.panel.rows.len());
                let _ = handle.emit_to(PANEL, "tray-panel-refresh", ());
            }
            if state
                .applied
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .as_ref()
                .is_some_and(|(previous, _)| previous == &view)
            {
                return;
            }
            let update = || -> tauri::Result<()> {
                let menu = menu(&handle, &view)?;
                if let Some(tray) = handle.tray_by_id(ID) {
                    tray.set_title(Some(&view.title))?;
                    tray.set_tooltip(Some(&view.tooltip))?;
                    tray.set_menu(Some(menu.clone()))?;
                }
                *state
                    .applied
                    .lock()
                    .unwrap_or_else(|error| error.into_inner()) = Some((view.clone(), menu));
                Ok(())
            };
            if let Err(error) = update() {
                eprintln!("Bento tray: {error}");
            }
        });
    });
}

pub fn handle_menu(app: &AppHandle, id: &str) {
    match id {
        SHOW => crate::reopen_main_window(app),
        QUIT => crate::quit::request(app),
        STOP_ALL => request_stop_all(app),
        id if id.starts_with(PROJECT_PREFIX) => {
            crate::reopen_main_window(app);
            #[derive(Clone, Serialize)]
            #[serde(rename_all = "camelCase")]
            struct Selection {
                project_id: String,
            }
            let _ = app.emit(
                "apphub-select-project",
                Selection {
                    project_id: id[PROJECT_PREFIX.len()..].into(),
                },
            );
        }
        _ => {}
    }
}

fn request_stop_all(app: &AppHandle) {
    if app
        .state::<TrayState>()
        .stop_pending
        .swap(true, Ordering::SeqCst)
    {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let view = match current(&app).await {
            Ok(view) => view,
            Err(error) => {
                finish_stop_all(&app, false);
                eprintln!("Bento tray: {error}");
                return;
            }
        };
        if view.projects.is_empty() {
            finish_stop_all(&app, false);
            return;
        }
        let message = view.confirmation();
        #[cfg(debug_assertions)]
        if let Some(answer) = crate::debug_e2e::stop_all_answer("停止全部项目？", &message) {
            finish_stop_all(&app, answer);
            return;
        }
        let dialog_app = app.clone();
        app.dialog()
            .message(message)
            .title("停止全部项目？")
            .kind(MessageDialogKind::Warning)
            .buttons(MessageDialogButtons::OkCancelCustom(
                "全部停止".into(),
                "取消".into(),
            ))
            .show(move |confirmed| finish_stop_all(&dialog_app, confirmed));
    });
}

fn finish_stop_all(app: &AppHandle, confirmed: bool) {
    if !confirmed {
        app.state::<TrayState>()
            .stop_pending
            .store(false, Ordering::SeqCst);
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(error) = crate::lifecycle::stop_all(&app, &app.state::<ProcessMap>()).await {
            app.dialog()
                .message(error)
                .title("停止项目失败")
                .kind(MessageDialogKind::Error)
                .show(|_| {});
        }
        app.state::<TrayState>()
            .stop_pending
            .store(false, Ordering::SeqCst);
    });
}

#[cfg(debug_assertions)]
pub fn debug_state(app: &AppHandle) -> Option<(TrayView, Menu<tauri::Wry>)> {
    app.state::<TrayState>()
        .applied
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(id: &str, display: Option<&str>) -> ProjectConfig {
        ProjectConfig {
            id: id.into(),
            name: id.into(),
            display_name: display.map(String::from),
            description: None,
            path: "/tmp".into(),
            port: None,
            services: Vec::new(),
            inject_ports: true,
            start_command: "sleep 1".into(),
            project_type: Some("app".into()),
                env: Vec::new(), commands: Vec::new(), auto_open: false,
                group_id: None, pinned: false, archived: false,
        }
    }

    #[test]
    fn derives_empty_title_and_disabled_stop_all() {
        let view = TrayView::from_snapshot(&[config("external", None)], &[]);
        assert_eq!(view.title, "");
        assert_eq!(view.tooltip, "Bento · 0 个项目运行中");
        assert_eq!(view.items[0].text, "没有运行中的项目");
        assert!(!view.items[0].enabled);
        assert!(
            !view
                .items
                .iter()
                .find(|item| item.id == STOP_ALL)
                .unwrap()
                .enabled
        );
        assert!(view.projects.is_empty());
    }

    #[test]
    fn panel_caps_rows_and_puts_running_before_recent_stopped_projects() {
        let projects: Vec<_> = (0..10).map(|index| config(&format!("p{index}"), None)).collect();
        let managed = [
            ManagedProject { id: "p2".into(), phase: Phase::Starting, port: None, started_at: 2000 },
            ManagedProject { id: "p5".into(), phase: Phase::Running, port: Some(5411), started_at: 1000 },
        ];
        let recent: Vec<_> = ["p5", "p7", "p2", "p3", "missing", "p8", "p9", "p4", "p0", "p1", "p6"]
            .into_iter().map(String::from).collect();
        let panel = panel_view(&projects, &managed, &recent, &[]);
        assert_eq!(panel.running, 1);
        assert!(panel.can_stop);
        assert_eq!(panel.rows.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
            ["p5", "p2", "p7", "p3", "p8", "p9", "p4", "p0"]);
        assert_eq!(panel.rows[0].port, Some(5411));
        assert_eq!(panel.rows[0].started_at, Some(1000));
        assert_eq!(panel.rows[2].status, "stopped");
    }

    #[test]
    fn panel_includes_agent_projects_outside_recents_and_shows_priority_state() {
        use crate::terminal_agent::{AgentKind, AgentState};
        let projects = [config("ai", None), config("recent", None)];
        let thinking = AgentSession { kind: AgentKind::Codex, state: AgentState::Thinking,
            state_started_at: 100, round_started_at: Some(100), acknowledged: false };
        let waiting = AgentSession { kind: AgentKind::Claude, state: AgentState::Waiting, ..thinking };
        let panel = panel_view(&projects, &[], &["recent".into()], &[
            (TerminalKey::new("ai", 1), thinking), (TerminalKey::new("ai", 2), waiting),
        ]);
        assert_eq!(panel.rows[0].id, "ai");
        assert_eq!(panel.rows[0].agent_detail.as_deref(), Some("Claude · 等你确认"));
        assert_eq!(panel.rows[1].id, "recent");
        assert_eq!(panel.running, 0);
    }

    #[test]
    fn derives_managed_names_ports_and_starting_suffix_only() {
        let projects = [
            config("running", Some("正在运行")),
            config("external", None),
            config("starting", None),
        ];
        let snapshot = [
            ManagedProject {
                id: "starting".into(),
                port: Some(5301),
                phase: Phase::Starting,
                started_at: 1000,
            },
            ManagedProject {
                id: "running".into(),
                port: None,
                phase: Phase::Running,
                started_at: 2000,
            },
        ];
        let view = TrayView::from_snapshot(&projects, &snapshot);
        assert_eq!(view.title, "1");
        assert_eq!(view.tooltip, "Bento · 1 个项目运行中");
        assert_eq!(view.items[0].text, "Bento 启动的项目 · 1 个运行中");
        assert_eq!(view.items[1].text, "正在运行");
        assert_eq!(view.items[2].text, "starting  :5301（启动中）");
        assert!(
            view.items
                .iter()
                .find(|item| item.id == STOP_ALL)
                .unwrap()
                .enabled
        );
        assert!(!view.items.iter().any(|item| item.text.contains("external")));
        assert_eq!(
            view.confirmation(),
            "将停止以下由 Bento 启动的项目：正在运行、starting。外部启动的进程不受影响。"
        );
        let orphan = TrayView::from_snapshot(&[], &snapshot);
        assert_eq!(orphan.projects.len(), 2);
    }
}
