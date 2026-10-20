use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};
use tokio::sync::Mutex;

mod terminal;
mod projects;
mod processes;
mod lifecycle;
mod logs;
mod quit;
mod tray;
mod git;
mod settings;
mod autostart;
use lifecycle::ProcessMap;
use projects::{ProjectDraft, ProjectDirInspection};
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
    #[serde(default)]
    pub port: Option<u16>,
    pub start_command: String,
    #[serde(alias = "projectType", rename = "type", default = "default_project_type")]
    pub project_type: Option<String>,
    #[serde(default)]
    pub env: Vec<projects::EnvVar>,
    #[serde(default)]
    pub commands: Vec<projects::ExtraCommand>,
    #[serde(default)]
    pub auto_open: bool,
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
    for (column, definition) in [("env", "TEXT"), ("commands", "TEXT"), ("auto_open", "INTEGER NOT NULL DEFAULT 0")] {
        if !columns.contains(column) {
            transaction.execute(&format!("ALTER TABLE projects ADD COLUMN {column} {definition}"), [])?;
        }
    }
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
        if let Some(port) = p.port {
            if port > 0 {
                let display = p.display_name.clone().unwrap_or_else(|| p.name.clone());
                port_map.entry(port).or_default().push((p.id.clone(), display));
            }
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
        projects,
        conflicts,
        config_path: db_path.to_string_lossy().to_string(),
    })
}

fn read_projects(conn: &Connection) -> Result<Vec<ProjectConfig>, String> {
    let mut stmt = conn
        .prepare("SELECT id, name, display_name, description, path, port, start_command, type, env, commands, auto_open FROM projects ORDER BY sort_order IS NULL, sort_order, created_at, id")
        .map_err(|e| format!("查询失败: {}", e))?;

    let project_iter = stmt
        .query_map([], |row| {
            let port_opt: Option<i64> = row.get(5)?;
            let ptype: Option<String> = row.get(7).unwrap_or(Some("web".to_string()));
            let id: String = row.get(0)?;
            let env: Option<String> = row.get(8).unwrap_or(None);
            let commands: Option<String> = row.get(9).unwrap_or(None);
            Ok(ProjectConfig {
                env: projects::parse_list(env.as_deref(), &id, "env"),
                commands: projects::parse_list(commands.as_deref(), &id, "commands"),
                auto_open: row.get::<_, i64>(10).unwrap_or(0) != 0,
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
async fn delete_project(
    app: AppHandle,
    id: String,
    state: State<'_, ProcessMap>,
    terminals: State<'_, terminal::TerminalManager>,
) -> Result<(), String> {
    let operation = lifecycle::operation(&state, &id).await;
    let _guard = operation.lock().await;
    lifecycle::stop_managed(&app, &id, &state, false).await?;
    let terminals = terminals.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        terminals.close(&id);
        open_project_db()?.execute("DELETE FROM projects WHERE id = ?1", params![id])
            .map_err(|e| format!("删除项目失败: {}", e))?;
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
async fn open_url(url: String) -> Result<(), String> {
    open_url_sync(&url)
}

fn open_url_sync(url: &str) -> Result<(), String> {
    #[cfg(debug_assertions)]
    if debug_e2e::record_open_url(url) { return Ok(()); }
    std::process::Command::new("open")
        .arg(&url)
        .spawn()
        .map_err(|e| format!("打开链接失败: {}", e))?;
    Ok(())
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
        .manage(quit::QuitPending::default())
        .manage(tray::TrayState::default())
        .setup(|app| {
            app.manage(terminal::TerminalManager::for_app(app.handle().clone()));
            quit::install_menu(app.handle())?;
            tray::install(app.handle())?;
            if let Err(error) = autostart::initialize(app.handle()) {
                eprintln!("Bento launch at login: {error}");
            }
            show_initial_window(app.handle());
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == "main" {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        });
    let commands: fn(tauri::ipc::Invoke<tauri::Wry>) -> bool = tauri::generate_handler![
        get_projects,
        add_project,
        reorder_projects,
        update_project,
        delete_project,
        inspect_project_dir,
        lifecycle::get_running_projects,
        lifecycle::get_projects_runtime_status,
        check_port_occupied,
        lifecycle::start_project,
        lifecycle::stop_project,
        lifecycle::stop_all_projects,
        lifecycle::restart_project,
        lifecycle::get_port_owners,
        lifecycle::stop_external_process,
        open_url,
        reveal_path,
        open_config_file,
        settings::get_settings,
        settings::set_setting,
        autostart::autostart_status,
        autostart::set_autostart,
        terminal::terminal_open,
        terminal::terminal_restart,
        terminal::terminal_list,
        terminal::terminal_write,
        terminal::terminal_run,
        terminal::terminal_write_bytes,
        terminal::terminal_resize,
        git::git_info,
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
    #[cfg(debug_assertions)]
    let builder = builder.on_page_load(debug_e2e::on_page_load);
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
                port: Some(5173),
                start_command: "npm run dev".to_string(),
                project_type: Some("web".to_string()),
                env: Vec::new(), commands: Vec::new(), auto_open: false,
            },
            ProjectConfig {
                id: "p2".to_string(),
                name: "proj2".to_string(),
                display_name: None,
                description: None,
                path: "~/Projects/p2".to_string(),
                port: Some(5174),
                start_command: "npm run dev".to_string(),
                project_type: Some("web".to_string()),
                env: Vec::new(), commands: Vec::new(), auto_open: false,
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
                port: Some(5173),
                start_command: "npm run dev".to_string(),
                project_type: Some("web".to_string()),
                env: Vec::new(), commands: Vec::new(), auto_open: false,
            },
            ProjectConfig {
                id: "p2".to_string(),
                name: "proj2".to_string(),
                display_name: Some("项目二".to_string()),
                description: None,
                path: "~/Projects/p2".to_string(),
                port: Some(5173),
                start_command: "npm run dev".to_string(),
                project_type: Some("web".to_string()),
                env: Vec::new(), commands: Vec::new(), auto_open: false,
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
