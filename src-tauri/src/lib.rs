use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};
use tokio::sync::Mutex;

mod terminal;
mod terminal_attention;
mod terminal_agent;
mod terminal_escape;
mod terminal_shell;
mod terminal_notifications;
mod shell_env;
mod projects;
mod processes;
mod lifecycle;
mod dashboard;
mod logs;
mod quit;
mod tray;
mod pet;
mod git;
mod settings;
mod snippets;
mod sidebar;
mod autostart;
mod traffic_lights;
mod artifacts;
use lifecycle::ProcessMap;
use projects::{PortConflictCheck, ProjectDraft, ProjectDirInspection};
#[cfg(debug_assertions)]
mod debug_e2e;

fn default_project_type() -> Option<String> {
    Some("web".to_string())
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ProjectConfig {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    pub path: String,
    #[serde(default = "projects::default_runnable")]
    pub runnable: bool,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub services: Vec<projects::Service>,
    #[serde(default = "projects::default_inject_ports")]
    pub inject_ports: bool,
    pub start_command: String,
    #[serde(alias = "projectType", rename = "type", default = "default_project_type")]
    pub project_type: Option<String>,
    #[serde(default)]
    pub env: Vec<projects::EnvVar>,
    #[serde(default)]
    pub commands: Vec<projects::ExtraCommand>,
    #[serde(default)]
    pub auto_open: bool,
    #[serde(default)]
    pub group_id: Option<String>,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub archived: bool,
}

impl ProjectConfig {
    pub fn ports(&self) -> impl Iterator<Item = u16> + '_ {
        self.port.into_iter().filter(|_| self.services.is_empty())
            .chain(self.services.iter().filter_map(|service| service.port))
            .filter(|port| *port > 0)
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PortConflict {
    pub port: u16,
    pub project_ids: Vec<String>,
    pub project_names: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ProjectsResponse {
    pub projects: Vec<ProjectConfig>,
    pub groups: Vec<sidebar::ProjectGroup>,
    pub conflicts: Vec<PortConflict>,
    pub config_path: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LogPayload {
    pub project_id: String,
    pub text: String,
    pub stream: String, // "stdout", "stderr", "system"
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RunningProjectInfo {
    pub project_id: String,
    pub pid: u32,
    pub port: Option<u16>,
}

pub fn get_db_path() -> PathBuf {
    #[cfg(debug_assertions)]
    if let Some(path) = debug_e2e::database_path() {
        return path;
    }
    if let Some(home) = dirs::home_dir() {
        let dir = home.join(".apphub");
        let _ = std::fs::create_dir_all(&dir);
        let new_db = dir.join("apphub.db");
        // Migration: If new_db doesn't exist but old ~/.fronthub/fronthub.db exists, copy it
        if !new_db.exists() {
            let old_db = home.join(".fronthub").join("fronthub.db");
            if old_db.exists() {
                let _ = std::fs::copy(&old_db, &new_db);
            }
        }
        new_db
    } else {
        PathBuf::from("apphub.db")
    }
}

pub fn init_db(conn: &Connection) -> Result<(), rusqlite::Error> {
    let transaction = conn.unchecked_transaction()?;
    transaction.execute(
        "CREATE TABLE IF NOT EXISTS settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL,
            updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
        )", [],
    )?;
    transaction.execute(
        "CREATE TABLE IF NOT EXISTS groups (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            sort_order INTEGER NOT NULL,
            collapsed INTEGER NOT NULL DEFAULT 0
        )", [],
    )?;
    transaction.execute(
        "CREATE TABLE IF NOT EXISTS projects (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            display_name TEXT,
            description TEXT,
            path TEXT NOT NULL,
            port INTEGER UNIQUE,
            start_command TEXT NOT NULL,
            type TEXT DEFAULT 'web',
            sort_order INTEGER,
            env TEXT,
            commands TEXT,
            auto_open INTEGER NOT NULL DEFAULT 0,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
        );",
        [],
    )?;

    let columns = {
        let mut statement = transaction.prepare("PRAGMA table_info(projects)")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(1))?;
        rows.collect::<rusqlite::Result<HashSet<_>>>()?
    };
    if !columns.contains("type") {
        transaction.execute("ALTER TABLE projects ADD COLUMN type TEXT DEFAULT 'web'", [])?;
    }
    if !columns.contains("sort_order") {
        transaction.execute("ALTER TABLE projects ADD COLUMN sort_order INTEGER", [])?;
        let ids = {
            let mut statement = transaction.prepare("SELECT id FROM projects ORDER BY port ASC, created_at, id")?;
            let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        for (position, id) in ids.iter().enumerate() {
            transaction.execute("UPDATE projects SET sort_order = ?1 WHERE id = ?2", params![position as i64, id])?;
        }
    }
    for (column, definition) in [
        ("env", "TEXT"), ("commands", "TEXT"), ("auto_open", "INTEGER NOT NULL DEFAULT 0"),
        ("group_id", "TEXT REFERENCES groups(id) ON DELETE SET NULL"),
        ("pinned", "INTEGER NOT NULL DEFAULT 0"), ("archived", "INTEGER NOT NULL DEFAULT 0"),
        ("services", "TEXT NOT NULL DEFAULT '[]'"), ("inject_ports", "INTEGER NOT NULL DEFAULT 1"),
        ("runnable", "INTEGER NOT NULL DEFAULT 1"),
    ] {
        if !columns.contains(column) {
            transaction.execute(&format!("ALTER TABLE projects ADD COLUMN {column} {definition}"), [])?;
        }
    }
    projects::migrate_service_keys(&transaction)?;
    dashboard::migrate(&transaction)?;
    snippets::create_table(&transaction)?;
    transaction.commit()
}

fn expand_tilde(path_str: &str) -> String {
    if path_str.starts_with("~/") || path_str == "~" {
        if let Some(home) = dirs::home_dir() {
            let rest = if path_str.len() > 2 { &path_str[2..] } else { "" };
            return home.join(rest).to_string_lossy().to_string();
        }
    }
    path_str.to_string()
}

fn calculate_conflicts(projects: &[ProjectConfig]) -> Vec<PortConflict> {
    let mut port_map: HashMap<u16, Vec<(String, String)>> = HashMap::new();
    for p in projects {
        for port in p.ports().collect::<HashSet<_>>() {
            let display = p.display_name.clone().unwrap_or_else(|| p.name.clone());
            port_map.entry(port).or_default().push((p.id.clone(), display));
        }
    }

    let mut conflicts = Vec::new();
    for (port, list) in port_map {
        if list.len() > 1 {
            conflicts.push(PortConflict {
                port,
                project_ids: list.iter().map(|(id, _)| id.clone()).collect(),
                project_names: list.iter().map(|(_, name)| name.clone()).collect(),
            });
        }
    }
    conflicts.sort_by_key(|c| c.port);
    conflicts
}

#[derive(Default)]
struct DatabaseInitializers(std::sync::Mutex<HashSet<PathBuf>>);

impl DatabaseInitializers {
    fn initialize(&self, path: &Path, conn: &Connection) -> rusqlite::Result<()> {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let mut initialized = self.0.lock().unwrap_or_else(|err| err.into_inner());
        if !initialized.contains(&path) {
            init_db(conn)?;
            initialized.insert(path);
            #[cfg(debug_assertions)]
            debug_e2e::record_db_initialization();
        }
        Ok(())
    }
}

fn initialize_project_db(path: &Path, conn: &Connection) -> rusqlite::Result<()> {
    static INITIALIZERS: OnceLock<DatabaseInitializers> = OnceLock::new();
    INITIALIZERS.get_or_init(DatabaseInitializers::default).initialize(path, conn)
}

pub fn get_projects_sync() -> Result<ProjectsResponse, String> {
    let db_path = get_db_path();
    let conn = Connection::open(&db_path)
        .map_err(|e| format!("无法连接 SQLite 数据库: {}", e))?;
    initialize_project_db(&db_path, &conn).map_err(|e| format!("初始化数据库失败: {}", e))?;

    let projects = read_projects(&conn)?;
    let conflicts = calculate_conflicts(&projects);
    Ok(ProjectsResponse {
        groups: sidebar::read_groups(&conn)?,
        projects,
        conflicts,
        config_path: db_path.to_string_lossy().to_string(),
    })
}

fn read_projects(conn: &Connection) -> Result<Vec<ProjectConfig>, String> {
    let mut stmt = conn
        .prepare("SELECT id, name, display_name, description, path, port, start_command, type, env, commands, auto_open, group_id, pinned, archived, services, inject_ports, runnable FROM projects ORDER BY sort_order IS NULL, sort_order, created_at, id")
        .map_err(|e| format!("查询失败: {}", e))?;

    let project_iter = stmt
        .query_map([], |row| {
            let port_opt: Option<i64> = row.get(5)?;
            let ptype: Option<String> = row.get(7).unwrap_or(Some("web".to_string()));
            let id: String = row.get(0)?;
            let env: Option<String> = row.get(8).unwrap_or(None);
            let commands: Option<String> = row.get(9).unwrap_or(None);
            let services: String = row.get(14)?;
            Ok(ProjectConfig {
                env: projects::parse_list(env.as_deref(), &id, "env"),
                commands: projects::parse_list(commands.as_deref(), &id, "commands"),
                services: projects::parse_list(Some(&services), &id, "services"),
                inject_ports: row.get(15)?,
                runnable: row.get(16)?,
                auto_open: row.get::<_, i64>(10).unwrap_or(0) != 0,
                group_id: row.get(11)?,
                pinned: row.get(12)?,
                archived: row.get(13)?,
                id,
                name: row.get(1)?,
                display_name: row.get(2)?,
                description: row.get(3)?,
                path: row.get(4)?,
                port: port_opt.map(|p| p as u16),
                start_command: row.get(6)?,
                project_type: ptype.or_else(|| Some("web".to_string())),
            })
        })
        .map_err(|e| format!("映射数据失败: {}", e))?;

    let mut projects = Vec::new();
    for p in project_iter {
        projects.push(p.map_err(|e| format!("读取项目失败: {}", e))?);
    }

    Ok(projects)
}

#[tauri::command]
async fn get_projects(terminals: State<'_, terminal::TerminalManager>) -> Result<ProjectsResponse, String> {
    let terminals = terminals.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let response = get_projects_sync()?;
        terminals.retain_projects(&response.projects.iter().map(|p| p.id.clone()).collect());
        Ok(response)
    }).await.map_err(|err| format!("加载项目失败：{err}"))?
}

fn open_project_db() -> Result<Connection, String> {
    let path = get_db_path();
    let conn = Connection::open(&path)
        .map_err(|e| format!("无法连接 SQLite 数据库: {}", e))?;
    initialize_project_db(&path, &conn).map_err(|e| format!("初始化数据库失败: {}", e))?;
    Ok(conn)
}

#[tauri::command]
fn add_project(project: ProjectDraft) -> Result<ProjectConfig, String> {
    projects::insert_project(&open_project_db()?, project)
}

#[tauri::command]
fn reorder_projects(ids: Vec<String>) -> Result<(), String> {
    projects::reorder_project_rows(&mut open_project_db()?, &ids)
}

#[tauri::command]
fn update_project(app: AppHandle, id: String, project: ProjectDraft) -> Result<ProjectConfig, String> {
    let project = projects::update_project_row(&open_project_db()?, &id, project)?;
    tray::refresh(&app);
    Ok(project)
}

#[tauri::command]
fn set_default_service(app: AppHandle, id: String, service_id: String) -> Result<(), String> {
    projects::set_default_service_row(&open_project_db()?, &id, &service_id)?;
    tray::refresh(&app);
    Ok(())
}

#[tauri::command]
async fn delete_project(
    app: AppHandle,
    id: String,
    state: State<'_, ProcessMap>,
    terminals: State<'_, terminal::TerminalManager>,
    external_runs: State<'_, dashboard::ExternalRuns>,
) -> Result<(), String> {
    let operation = lifecycle::operation(&state, &id).await;
    let _guard = operation.lock().await;
    lifecycle::stop_managed(&app, &id, &state, false).await?;
    let terminals = terminals.inner().clone();
    let external_runs = external_runs.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        terminals.close(&id);
        let mut conn = open_project_db()?;
        let mut delete = || -> rusqlite::Result<()> {
            let tx = conn.transaction()?;
            tx.execute("DELETE FROM project_last_runs WHERE project_id = ?1", params![id])?;
            tx.execute("DELETE FROM project_snippets WHERE project_id = ?1", params![id])?;
            tx.execute("DELETE FROM projects WHERE id = ?1", params![id])?;
            tx.commit()
        };
        delete().map_err(|e| format!("删除项目失败: {}", e))?;
        external_runs.remove(&id);
        Ok::<_, String>(())
    }).await.map_err(|err| format!("删除项目失败：{err}"))??;
    tray::refresh(&app);
    Ok(())
}

#[tauri::command]
async fn inspect_project_dir(path: String, exclude_id: Option<String>) -> Result<ProjectDirInspection, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let db_path = get_db_path();
        let conn = if db_path.exists() {
            Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(|e| format!("读取项目数据库失败：{e}"))?
        } else {
            let conn = Connection::open_in_memory().map_err(|e| format!("检查项目目录失败：{e}"))?;
            init_db(&conn).map_err(|e| format!("检查项目目录失败：{e}"))?;
            conn
        };
        projects::inspect_project_dir(&conn, &path, exclude_id.as_deref())
    }).await.map_err(|err| format!("检查项目目录失败：{err}"))?
}

#[tauri::command]
async fn check_project_port_standard(path: String, start_command: String, services: Vec<projects::Service>) -> Result<projects::PortStandardCheck, String> {
    tauri::async_runtime::spawn_blocking(move || projects::check_port_standard(&path, &start_command, services))
        .await.map_err(|err| format!("检查启动标准失败：{err}"))?
}

#[tauri::command]
async fn preview_project_launch(config: projects::LaunchConfig) -> Result<projects::LaunchPlan, String> {
    tauri::async_runtime::spawn_blocking(move || projects::launch_plan(config))
        .await.map_err(|err| format!("预览启动命令失败：{err}"))?
}

#[tauri::command]
async fn preview_project_standard_script(path: String, services: Vec<projects::Service>) -> Result<projects::StandardScript, String> {
    tauri::async_runtime::spawn_blocking(move || projects::standard_script_preview(&path, services))
        .await.map_err(|err| format!("预览启动脚本失败：{err}"))?
}

#[tauri::command]
async fn generate_project_standard_script(path: String, services: Vec<projects::Service>, preview: projects::StandardScript) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || projects::write_standard_script(&path, services, preview))
        .await.map_err(|err| format!("生成启动脚本失败：{err}"))?
}

pub fn check_port_in_use_sync(port: u16) -> (bool, Option<u32>) {
    if processes::probe_loopback(port) {
        return (true, None);
    }
    let pid = processes::port_pids(port).first().copied();
    (pid.is_some(), pid)
}

#[tauri::command]
async fn check_port_occupied(port: u16) -> Result<bool, String> {
    tauri::async_runtime::spawn_blocking(move || check_port_in_use_sync(port).0).await
        .map_err(|err| format!("检查端口失败：{err}"))
}

#[tauri::command]
async fn check_port_conflicts(
    port: u16,
    exclude_project_id: Option<String>,
    state: State<'_, ProcessMap>,
) -> Result<PortConflictCheck, String> {
    let info = {
        let data = state.lock().await;
        exclude_project_id.as_ref().and_then(|id| data.managed.get(id).cloned())
    };
    tauri::async_runtime::spawn_blocking(move || {
        let managed_pids = info.as_ref().map(lifecycle::ProcessInfo::pids).unwrap_or_default();
        projects::check_port_conflicts(&open_project_db()?, port, exclude_project_id.as_deref(), &managed_pids)
    }).await.map_err(|err| format!("检查端口失败：{err}"))?
}

#[tauri::command]
async fn check_service_port_conflicts(
    ports: Vec<u16>,
    exclude_project_id: Option<String>,
    state: State<'_, ProcessMap>,
) -> Result<HashMap<u16, PortConflictCheck>, String> {
    let info = {
        let data = state.lock().await;
        exclude_project_id.as_ref().and_then(|id| data.managed.get(id).cloned())
    };
    tauri::async_runtime::spawn_blocking(move || {
        let managed_pids = info.as_ref().map(lifecycle::ProcessInfo::pids).unwrap_or_default();
        projects::check_ports(&open_project_db()?, &ports, exclude_project_id.as_deref(), &managed_pids)
    }).await.map_err(|err| format!("检查服务端口失败：{err}"))?
}

/// The machine's IPv4 address on the local network, used to show LAN URLs for running services.
/// Reads the physical interfaces (en*) from ifconfig instead of the default route: with a TUN proxy
/// (Clash, Surge, VPNs) the route points at a utun/198.18.x address that other devices cannot reach.
#[tauri::command]
fn local_network_ip() -> Option<String> {
    let output = std::process::Command::new("/sbin/ifconfig").output().ok()?;
    lan_ip_from_ifconfig(&String::from_utf8_lossy(&output.stdout))
}

fn lan_ip_from_ifconfig(text: &str) -> Option<String> {
    // (interface name, active, private IPv4 addresses), in ifconfig order.
    let mut interfaces: Vec<(String, bool, Vec<std::net::Ipv4Addr>)> = Vec::new();
    for line in text.lines() {
        if !line.starts_with(char::is_whitespace) && !line.is_empty() {
            let name = line.split(':').next().unwrap_or_default().to_string();
            interfaces.push((name, false, Vec::new()));
            continue;
        }
        let Some(current) = interfaces.last_mut() else { continue };
        let line = line.trim();
        if line == "status: active" { current.1 = true; }
        if let Some(rest) = line.strip_prefix("inet ") {
            if let Some(ip) = rest.split_whitespace().next().and_then(|value| value.parse::<std::net::Ipv4Addr>().ok()) {
                if ip.is_private() && !ip.is_loopback() && !ip.is_link_local() { current.2.push(ip); }
            }
        }
    }
    let physical = |name: &str| name.starts_with("en");
    interfaces.iter().find(|(name, active, ips)| physical(name) && *active && !ips.is_empty())
        .or_else(|| interfaces.iter().find(|(name, _, ips)| physical(name) && !ips.is_empty()))
        .and_then(|(_, _, ips)| ips.first()).map(|ip| ip.to_string())
}

#[cfg(test)]
mod lan_ip_tests {
    #[test]
    fn prefers_active_physical_interface_over_tun_and_bridges() {
        let text = "lo0: flags=8049<UP,LOOPBACK>\n\tinet 127.0.0.1 netmask 0xff000000\n\
bridge100: flags=8863<UP>\n\tinet 192.168.0.1 netmask 0xffffff00\n\tstatus: active\n\
en0: flags=8863<UP>\n\tinet 192.168.1.43 netmask 0xfffffc00 broadcast 192.168.3.255\n\tstatus: active\n\
utun6: flags=8051<UP>\n\tinet 198.18.0.1 --> 198.18.0.1 netmask 0xfffffffc\n";
        assert_eq!(super::lan_ip_from_ifconfig(text).as_deref(), Some("192.168.1.43"));
        assert_eq!(super::lan_ip_from_ifconfig("utun6: flags=1\n\tinet 198.18.0.1 --> 198.18.0.1\n"), None);
    }
}

#[tauri::command]
async fn open_url(url: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || open_url_sync(&url)).await
        .map_err(|err| format!("打开链接失败：{err}"))?
}

fn open_url_sync(url: &str) -> Result<(), String> {
    let parsed = tauri::Url::parse(url).map_err(|_| "链接地址无效")?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err("仅支持 HTTP 和 HTTPS 网页链接".into());
    }
    #[cfg(debug_assertions)]
    if let Some(result) = debug_e2e::record_open_url(url) { return result; }
    let output = std::process::Command::new("/usr/bin/open")
        .arg(parsed.as_str())
        .output()
        .map_err(|err| format!("无法启动系统浏览器：{err}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let detail = stderr.trim();
    Err(if detail.is_empty() { output.status.to_string() } else { detail.into() })
}

#[tauri::command]
async fn reveal_path(path: String) -> Result<(), String> {
    let expanded = expand_tilde(&path);
    std::process::Command::new("open")
        .arg("-R")
        .arg(&expanded)
        .spawn()
        .map_err(|e| format!("在 Finder 中定位失败: {}", e))?;
    Ok(())
}

#[tauri::command]
async fn open_config_file() -> Result<(), String> {
    let db_path = get_db_path();
    std::process::Command::new("open")
        .arg("-R")
        .arg(&db_path)
        .spawn()
        .map_err(|e| format!("打开数据库文件所在目录失败: {}", e))?;
    Ok(())
}

pub fn run() {
    let process_map: ProcessMap = Arc::new(Mutex::new(lifecycle::RuntimeState::default()));

    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(autostart::plugin())
        .manage(autostart::AutostartState::default())
        .manage(process_map)
        .manage(dashboard::ExternalRuns::default())
        .manage(quit::QuitPending::default())
        .manage(tray::TrayState::default())
        .manage(pet::PetState::default())
        .manage(terminal_notifications::PendingNotification::default())
        .setup(|app| {
            app.manage(terminal::TerminalManager::for_app(app.handle().clone()));
            terminal_notifications::install(app.handle());
            quit::install_menu(app.handle())?;
            tray::install(app.handle())?;
            pet::install(app.handle());
            if let Err(error) = autostart::initialize(app.handle()) {
                eprintln!("Bento launch at login: {error}");
            }
            if let Err(error) = settings::initialize_appearance(app.handle()) {
                eprintln!("Bento appearance: {error}");
            }
            #[cfg(target_os = "macos")]
            if let Some(window) = app.get_webview_window("main") {
                if let Err(error) = traffic_lights::install(&window) {
                    eprintln!("Bento traffic lights: {error}");
                }
            }
            show_initial_window(app.handle());
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == tray::PANEL {
                match event {
                    tauri::WindowEvent::Focused(false) => { let _ = window.hide(); }
                    tauri::WindowEvent::CloseRequested { api, .. } => {
                        api.prevent_close();
                        let _ = window.hide();
                    }
                    _ => (),
                }
            }
            if window.label() == pet::PET || window.label() == pet::PET_DOCK {
                match event {
                    tauri::WindowEvent::Moved(position) => pet::on_moved(window.app_handle(), *position),
                    tauri::WindowEvent::CloseRequested { api, .. } => api.prevent_close(),
                    _ => (),
                }
            }
            if window.label() == "main" {
                match event {
                    tauri::WindowEvent::CloseRequested { api, .. } => {
                        api.prevent_close();
                        let _ = window.hide();
                        pet::layout(window.app_handle());
                    }
                    tauri::WindowEvent::Moved(_) | tauri::WindowEvent::Resized(_) | tauri::WindowEvent::Focused(_)
                    | tauri::WindowEvent::ScaleFactorChanged { .. } => pet::layout(window.app_handle()),
                    _ => (),
                }
            }
        });
    let commands: fn(tauri::ipc::Invoke<tauri::Wry>) -> bool = tauri::generate_handler![
        get_projects,
        dashboard::dashboard_snapshot,
        dashboard::dashboard_health,
        dashboard::dashboard_release_port,
        add_project,
        reorder_projects,
        sidebar::add_group,
        sidebar::update_group,
        sidebar::delete_group,
        sidebar::reorder_groups,
        sidebar::move_project,
        sidebar::set_project_flags,
        update_project,
        set_default_service,
        delete_project,
        inspect_project_dir,
        preview_project_launch,
        check_project_port_standard,
        preview_project_standard_script,
        generate_project_standard_script,
        lifecycle::get_running_projects,
        lifecycle::get_projects_runtime_status,
        check_port_occupied,
        check_port_conflicts,
        check_service_port_conflicts,
        lifecycle::start_project,
        lifecycle::stop_project,
        lifecycle::stop_all_projects,
        lifecycle::restart_project,
        lifecycle::get_port_owners,
        lifecycle::stop_external_process,
        open_url,
        local_network_ip,
        reveal_path,
        artifacts::project_images,
        artifacts::read_image_file,
        open_config_file,
        settings::get_settings,
        settings::set_setting,
        traffic_lights::set_traffic_light_layout,
        autostart::autostart_status,
        autostart::set_autostart,
        terminal::terminal_open,
        terminal::terminal_restart,
        terminal::terminal_create,
        terminal::agent_sessions::terminal_agent_menu,
        terminal::agent_sessions::terminal_agent_project_sessions,
        terminal::agent_sessions::terminal_agent_start,
        terminal::agent_sessions::terminal_agent_launch,
        terminal::terminal_close,
        terminal::terminal_list,
        terminal_notifications::take_terminal_notification,
        snippets::get_snippets,
        snippets::change_snippet,
        snippets::suggest_snippets,
        tray::get_tray_panel,
        tray::note_tray_project,
        tray::tray_panel_action,
        pet::get_pet_view,
        pet::pet_menu,
        pet::pet_action,
        pet::pet_set_regions,
        pet::pet_drag_start,
        pet::pet_dock,
        pet::pet_set_edge,
        pet::pet_stop,
        pet::pet_jump,
        pet::pet_press,
        pet::pet_edge,
        pet::pet_style_init,
        pet::pet_tucked,
        terminal::terminal_context,
        terminal::terminal_focus,
        terminal::terminal_write,
        terminal::terminal_run,
        terminal::terminal_write_bytes,
        terminal::terminal_resize,
        git::git_info,
        git::dashboard_git,
        #[cfg(debug_assertions)]
        debug_e2e::e2e_report,
        #[cfg(debug_assertions)]
        debug_e2e::e2e_window,
        #[cfg(debug_assertions)]
        debug_e2e::e2e_snapshot,
        #[cfg(debug_assertions)]
        debug_e2e::e2e_click,
        #[cfg(debug_assertions)]
        debug_e2e::e2e_menu,
        #[cfg(debug_assertions)]
        debug_e2e::e2e_metrics,
        #[cfg(debug_assertions)]
        debug_e2e::e2e_tray,
    ];
    let builder = builder.invoke_handler(move |invoke| {
        #[cfg(debug_assertions)]
        debug_e2e::record_command(invoke.message.command());
        commands(invoke)
    });
    let builder = builder.on_page_load(|webview, payload| {
        if webview.label() == "main" && payload.event() == tauri::webview::PageLoadEvent::Started {
            if let Some(manager) = webview.try_state::<terminal::TerminalManager>() {
                manager.focus(None);
            }
        }
        #[cfg(debug_assertions)]
        debug_e2e::on_page_load(webview, payload);
    });
    builder.build(tauri::generate_context!())
        .expect("构建 Bento 失败")
        .run(handle_run_event);
}

fn show_initial_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let size_and_center = || -> tauri::Result<()> {
            if let Some(monitor) = window.current_monitor().ok().flatten() {
                let area = monitor.work_area();
                let scale = monitor.scale_factor();
                let width = (f64::from(area.size.width) / scale * 0.75)
                    .max(850.0)
                    .min(f64::from(area.size.width) / scale)
                    .round();
                let height = (f64::from(area.size.height) / scale * 0.80)
                    .max(600.0)
                    .min(f64::from(area.size.height) / scale)
                    .round();
                window.set_size(tauri::LogicalSize::new(width, height))?;
                // Native size changes are queued; outer_size can still report the old size here.
                window.set_position(tauri::PhysicalPosition::new(
                    area.position.x + ((f64::from(area.size.width) - width * scale) / 2.0).round() as i32,
                    area.position.y + ((f64::from(area.size.height) - height * scale) / 2.0).round() as i32,
                ))?;
            } else {
                window.set_size(tauri::LogicalSize::new(1440.0, 900.0))?;
                window.center()?;
            }
            Ok(())
        };
        if let Err(error) = size_and_center() {
            eprintln!("Bento initial window size: {error}");
        }
        // Size/monitor failures must never strand a hidden window.
        if !std::env::args_os().any(|arg| arg == "--autostart") {
            let _ = window.show();
            let _ = window.set_focus();
        }
    }
}

fn handle_run_event(app: &AppHandle, event: tauri::RunEvent) {
    match event {
        tauri::RunEvent::Reopen { .. } => reopen_main_window(app),
        tauri::RunEvent::Exit => {
            let terminal_roots = app.state::<terminal::TerminalManager>().list().into_iter()
                .filter(|session| session.alive).filter_map(|session| session.pid)
                .map(|pid| processes::ManagedRoot { pid, pgid: unsafe { libc::getpgid(pid as i32) }, identity: processes::identity(pid) })
                .collect();
            lifecycle::shutdown(&app.state::<ProcessMap>(), terminal_roots);
            app.state::<terminal::TerminalManager>().kill_all();
        },
        _ => {}
    }
}

fn reopen_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advanced_migration_is_idempotent_and_corrupt_json_stays_readable() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE projects (id TEXT PRIMARY KEY, name TEXT NOT NULL, display_name TEXT, description TEXT, path TEXT NOT NULL, port INTEGER, start_command TEXT NOT NULL, type TEXT DEFAULT 'web', sort_order INTEGER, created_at TEXT, updated_at TEXT);
            INSERT INTO projects (id,name,path,start_command,sort_order) VALUES ('old','old','/tmp','sleep 1',4);").unwrap();
        init_db(&conn).unwrap();
        let old = read_projects(&conn).unwrap().remove(0);
        assert!(old.env.is_empty() && old.commands.is_empty() && !old.auto_open);
        assert!(old.services.is_empty() && old.inject_ports && old.runnable);
        conn.execute("UPDATE projects SET env='[{\"key\":\"A\",\"value\":\"B\"}]', commands='broken', auto_open=1", []).unwrap();
        init_db(&conn).unwrap();
        init_db(&conn).unwrap();
        let updated = read_projects(&conn).unwrap().remove(0);
        assert_eq!(updated.env[0].value, "B");
        assert!(updated.commands.is_empty() && updated.auto_open);
        assert_eq!(conn.query_row("SELECT sort_order FROM projects", [], |row| row.get::<_, i64>(0)).unwrap(), 4);
        conn.execute("UPDATE projects SET env='[{}]', commands=NULL", []).unwrap();
        let corrupt = read_projects(&conn).unwrap().remove(0);
        assert!(corrupt.env.is_empty() && corrupt.commands.is_empty());
    }

    #[test]
    fn migrates_legacy_order_with_null_ports_first_and_keeps_it_on_restart() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE projects (id TEXT PRIMARY KEY, name TEXT NOT NULL, display_name TEXT, description TEXT, path TEXT NOT NULL, port INTEGER UNIQUE, start_command TEXT NOT NULL, created_at TEXT, updated_at TEXT);
            INSERT INTO projects (id, name, path, port, start_command, created_at) VALUES
            ('late', 'late', '/tmp', 9000, 'sleep 1', '2026-01-01'),
            ('null-z', 'null-z', '/tmp', NULL, 'sleep 1', '2026-02-01'),
            ('null-b', 'null-b', '/tmp', NULL, 'sleep 1', '2026-01-01'),
            ('early', 'early', '/tmp', 3000, 'sleep 1', '2026-01-01'),
            ('null-a', 'null-a', '/tmp', NULL, 'sleep 1', '2026-01-01');").unwrap();
        init_db(&conn).unwrap();
        let ids: Vec<_> = read_projects(&conn).unwrap().into_iter().map(|project| project.id).collect();
        assert_eq!(ids, ["null-a", "null-b", "null-z", "early", "late"]);
        for (position, id) in ids.iter().enumerate() {
            assert_eq!(conn.query_row("SELECT sort_order FROM projects WHERE id = ?1", [id], |row| row.get::<_, i64>(0)).unwrap(), position as i64);
        }
        conn.execute("UPDATE projects SET sort_order = 10 WHERE id = 'null-a'", []).unwrap();
        let ordered = read_projects(&conn).unwrap().into_iter().map(|project| project.id).collect::<Vec<_>>();
        init_db(&conn).unwrap();
        assert_eq!(read_projects(&conn).unwrap().into_iter().map(|project| project.id).collect::<Vec<_>>(), ordered);
    }

    #[test]
    fn reads_null_sort_order_last_with_stable_ties() {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        conn.execute_batch("INSERT INTO projects (id, name, path, start_command, sort_order, created_at) VALUES
            ('cli-z', 'z', '/tmp', 'sleep 1', NULL, '2026-02-01'),
            ('b', 'b', '/tmp', 'sleep 1', 4, '2026-01-01'),
            ('a', 'a', '/tmp', 'sleep 1', 4, '2026-01-01'),
            ('first', 'first', '/tmp', 'sleep 1', 0, '2026-03-01'),
            ('cli-a', 'a', '/tmp', 'sleep 1', NULL, '2026-01-01');").unwrap();
        assert_eq!(read_projects(&conn).unwrap().into_iter().map(|project| project.id).collect::<Vec<_>>(), ["first", "a", "b", "cli-a", "cli-z"]);
    }

    #[test]
    fn initializes_each_path_once_and_leaves_in_memory_initialization_independent() {
        let initializers = DatabaseInitializers::default();
        let conn = Connection::open_in_memory().unwrap();
        let path = Path::new("apphub-init-once-test.db");
        initializers.initialize(path, &conn).unwrap();
        conn.execute("DROP TABLE projects", []).unwrap();
        initializers.initialize(path, &conn).unwrap();
        assert!(conn.prepare("SELECT * FROM projects").is_err());
        initializers.initialize(Path::new("apphub-other-path-test.db"), &conn).unwrap();
        assert!(conn.prepare("SELECT * FROM projects").is_ok());

        for _ in 0..2 {
            let memory = Connection::open_in_memory().unwrap();
            init_db(&memory).unwrap();
            assert!(memory.prepare("SELECT * FROM projects").is_ok());
        }
    }

    #[test]
    fn failed_initialization_can_be_retried() {
        let initializers = DatabaseInitializers::default();
        let conn = Connection::open_in_memory().unwrap();
        let path = Path::new("apphub-init-retry-test.db");
        conn.execute_batch("PRAGMA query_only = ON").unwrap();
        assert!(initializers.initialize(path, &conn).is_err());
        conn.execute_batch("PRAGMA query_only = OFF").unwrap();
        initializers.initialize(path, &conn).unwrap();
        assert!(conn.prepare("SELECT * FROM projects").is_ok());
    }

    #[test]
    fn test_sqlite_init_and_uniqueness() {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();

        // Empty databases stay empty, including after the last project is deleted.
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM projects", [], |row| row.get(0)).unwrap();
        assert_eq!(count, 0);
        conn.execute(
            "INSERT INTO projects (id, name, path, port, start_command) VALUES (?1, ?2, ?3, ?4, ?5)",
            params!["p1", "first-proj", "~/Projects/first", 5173, "pnpm dev"],
        ).unwrap();

        // 2. Duplicate port insertion should fail with UNIQUE constraint
        let duplicate_res = conn.execute(
            "INSERT INTO projects (id, name, path, port, start_command) VALUES (?1, ?2, ?3, ?4, ?5)",
            params!["p2", "another-proj", "~/Projects/other", 5173, "pnpm dev"],
        );
        assert!(duplicate_res.is_err());
    }

    #[test]
    fn test_calculate_conflicts_none() {
        let projects = vec![
            ProjectConfig {
                id: "p1".to_string(),
                name: "proj1".to_string(),
                display_name: None,
                description: None,
                path: "~/Projects/p1".to_string(),
                runnable: true,
                port: Some(5173),
                services: Vec::new(),
                inject_ports: true,
                start_command: "npm run dev".to_string(),
                project_type: Some("web".to_string()),
                env: Vec::new(), commands: Vec::new(), auto_open: false,
                group_id: None, pinned: false, archived: false,
            },
            ProjectConfig {
                id: "p2".to_string(),
                name: "proj2".to_string(),
                display_name: None,
                description: None,
                path: "~/Projects/p2".to_string(),
                runnable: true,
                port: Some(5174),
                services: Vec::new(),
                inject_ports: true,
                start_command: "npm run dev".to_string(),
                project_type: Some("web".to_string()),
                env: Vec::new(), commands: Vec::new(), auto_open: false,
                group_id: None, pinned: false, archived: false,
            },
        ];
        let conflicts = calculate_conflicts(&projects);
        assert!(conflicts.is_empty());
    }

    #[test]
    fn test_calculate_conflicts_duplicate_port() {
        let projects = vec![
            ProjectConfig {
                id: "p1".to_string(),
                name: "proj1".to_string(),
                display_name: Some("项目一".to_string()),
                description: None,
                path: "~/Projects/p1".to_string(),
                runnable: true,
                port: Some(5173),
                services: Vec::new(),
                inject_ports: true,
                start_command: "npm run dev".to_string(),
                project_type: Some("web".to_string()),
                env: Vec::new(), commands: Vec::new(), auto_open: false,
                group_id: None, pinned: false, archived: false,
            },
            ProjectConfig {
                id: "p2".to_string(),
                name: "proj2".to_string(),
                display_name: Some("项目二".to_string()),
                description: None,
                path: "~/Projects/p2".to_string(),
                runnable: true,
                port: Some(5173),
                services: Vec::new(),
                inject_ports: true,
                start_command: "npm run dev".to_string(),
                project_type: Some("web".to_string()),
                env: Vec::new(), commands: Vec::new(), auto_open: false,
                group_id: None, pinned: false, archived: false,
            },
        ];
        let conflicts = calculate_conflicts(&projects);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].port, 5173);
        assert_eq!(conflicts[0].project_ids, vec!["p1", "p2"]);
        assert_eq!(conflicts[0].project_names, vec!["项目一", "项目二"]);
    }

    #[test]
    fn test_expand_tilde() {
        let path = "~/Projects/my-app";
        let expanded = expand_tilde(path);
        assert!(!expanded.starts_with('~'));
        assert!(expanded.contains("/Projects/my-app"));
    }
}
