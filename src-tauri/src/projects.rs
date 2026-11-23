use crate::{check_port_in_use_sync, expand_tilde, processes, ProjectConfig};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

mod services;
mod standard;
mod launch;
pub use launch::{launch_plan, LaunchConfig, LaunchPlan};
pub use services::{default_inject_ports, Service};
pub use standard::{check_port_standard, standard_script_preview, write_standard_script, PortStandardCheck, StandardScript};
use services::{fill_service_keys, independent_commands, normalize_services, scan_services, ScannedService, StartScript};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct EnvVar {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ExtraCommand {
    pub name: String,
    pub command: String,
}

pub fn validate_env(env: &[EnvVar]) -> Result<(), String> {
    let mut keys = HashSet::new();
    for (index, entry) in env.iter().enumerate() {
        let mut chars = entry.key.chars();
        if !chars
            .next()
            .is_some_and(|ch| ch.is_ascii_alphabetic() || ch == '_')
            || !chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        {
            return Err(format!(
                "第 {} 个环境变量名称须以字母或下划线开头，且只能包含字母、数字和下划线",
                index + 1
            ));
        }
        if matches!(
            entry.key.as_str(),
            "TERM" | "COLORTERM" | "SHELL" | "HOME" | "USER" | "PWD" | processes::MANAGED_RUN_ENV
        ) {
            return Err(format!("不能覆盖终端保留变量 {}", entry.key));
        }
        if !keys.insert(&entry.key) {
            return Err(format!("环境变量名称 {} 重复", entry.key));
        }
    }
    Ok(())
}

pub fn parse_list<T: serde::de::DeserializeOwned>(
    raw: Option<&str>,
    id: &str,
    column: &str,
) -> Vec<T> {
    let Some(raw) = raw else {
        return Vec::new();
    };
    match serde_json::from_str(raw) {
        Ok(entries) => entries,
        Err(_) => {
            static WARNED: OnceLock<Mutex<HashSet<(String, String)>>> = OnceLock::new();
            if WARNED
                .get_or_init(Mutex::default)
                .lock()
                .unwrap_or_else(|err| err.into_inner())
                .insert((id.into(), column.into()))
            {
                eprintln!("[Bento] 项目 {id} 的 {column} 配置无效，已按空列表读取");
            }
            Vec::new()
        }
    }
}

pub fn default_runnable() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDraft {
    pub name: String,
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub path: String,
    #[serde(default = "default_runnable")]
    pub runnable: bool,
    pub port: Option<i64>,
    #[serde(default)]
    pub services: Vec<Service>,
    #[serde(default = "default_inject_ports")]
    pub inject_ports: bool,
    pub start_command: String,
    #[serde(rename = "type")]
    pub project_type: Option<String>,
    #[serde(default)]
    pub env: Vec<EnvVar>,
    #[serde(default)]
    pub commands: Vec<ExtraCommand>,
    #[serde(default)]
    pub auto_open: bool,
    #[serde(default)]
    pub group_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExistingProject {
    id: String,
    name: String,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum PortConfidence {
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortCandidate {
    port: u16,
    source: String,
    confidence: PortConfidence,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortInUse {
    pub(crate) owners: Vec<processes::PortOwner>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortConflictCheck {
    project_conflict: Option<ExistingProject>,
    pub(crate) in_use: Option<PortInUse>,
    reserved: Vec<String>,
    pub(crate) suggestion: Option<u16>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDirInspection {
    path: String,
    exists: bool,
    is_dir: bool,
    name: String,
    project_type: String,
    package_manager: Option<String>,
    commands: Vec<String>,
    port: Option<u16>,
    port_source: Option<String>,
    port_candidates: Vec<PortCandidate>,
    existing_project: Option<ExistingProject>,
    services: Vec<ScannedService>,
    start_scripts: Vec<StartScript>,
    standard: Option<PortStandardCheck>,
}

pub fn migrate_service_keys(conn: &Connection) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare("SELECT id, services FROM projects WHERE services != '[]'")?;
    let rows = stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?;
    let mut updates = Vec::new();
    for row in rows {
        let (id, raw) = row?;
        let Ok(mut services) = serde_json::from_str::<Vec<Service>>(&raw) else { continue };
        if services.iter().any(|service| service.key.is_empty()) {
            fill_service_keys(&mut services);
            let json = serde_json::to_string(&services).map_err(|err| rusqlite::Error::ToSqlConversionFailure(Box::new(err)))?;
            updates.push((id, json));
        }
    }
    for (id, services) in updates {
        conn.execute("UPDATE projects SET services=?1 WHERE id=?2", params![services, id])?;
    }
    Ok(())
}

fn optional_text(value: Option<String>) -> Option<String> {
    value
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn expanded_path(path: &str) -> PathBuf {
    let trimmed = path.trim().trim_end_matches('/');
    let input = if trimmed.is_empty() && path.trim().starts_with('/') {
        "/"
    } else {
        trimmed
    };
    let expanded = PathBuf::from(expand_tilde(input));
    expanded.canonicalize().unwrap_or(expanded)
}

fn stored_path(path: &Path) -> String {
    if let Some(home) = dirs::home_dir() {
        let relative = path.strip_prefix(&home).ok().or_else(|| {
            let canonical_home = home.canonicalize().ok()?;
            path.strip_prefix(canonical_home).ok()
        });
        if let Some(rest) = relative {
            return if rest.as_os_str().is_empty() {
                "~".into()
            } else {
                format!("~/{}", rest.display())
            };
        }
    }
    let value = path.to_string_lossy();
    let trimmed = value.trim_end_matches('/');
    if trimmed.is_empty() && value.starts_with('/') {
        "/".into()
    } else {
        trimmed.into()
    }
}

fn normalize_project(
    conn: &Connection,
    draft: ProjectDraft,
    id: Option<&str>,
) -> Result<ProjectConfig, String> {
    let name = draft.name.trim().to_string();
    if name.is_empty() {
        return Err("请输入项目名称".into());
    }
    let runnable = draft.runnable;
    let (start_command, services) = if runnable {
        (draft.start_command.trim().to_string(), normalize_services(draft.services)?)
    } else {
        (String::new(), Vec::new())
    };
    if runnable && start_command.is_empty() && !independent_commands(&services)? {
        return Err("请输入启动命令".into());
    }
    let project_type = draft.project_type.as_deref().unwrap_or("web").trim();
    if !matches!(project_type, "web" | "app") {
        return Err("项目类型必须为 Web 项目或桌面应用".into());
    }
    let port = if !runnable {
        None
    } else if services.is_empty() {
        match draft.port {
            Some(port) if (1..=65535).contains(&port) => Some(port as u16),
            Some(_) => return Err("端口必须是 1–65535 之间的整数".into()),
            None => None,
        }
    } else {
        services.iter().find(|service| service.open_web).and_then(|service| service.port)
    };
    if draft.path.trim().is_empty() {
        return Err("请输入项目目录".into());
    }
    let path = expanded_path(&draft.path);
    if !path.exists() {
        return Err("目录不存在".into());
    }
    if !path.is_dir() {
        return Err("不是文件夹".into());
    }
    let ports = port.filter(|_| services.is_empty()).into_iter()
        .chain(services.iter().filter_map(|service| service.port));
    for port in ports {
        validate_project_port(conn, port, id)?;
    }
    validate_env(&draft.env)?;
    if draft.commands.len() > 10 {
        return Err("其他命令最多添加 10 个".into());
    }
    let mut command_names = HashSet::new();
    let mut commands = Vec::new();
    for entry in draft.commands {
        let name = entry.name.trim().to_string();
        let command = entry.command.trim().to_string();
        if name.is_empty() {
            return Err("请输入其他命令名称".into());
        }
        if name.chars().count() > 20 {
            return Err("命令名称不能超过 20 个字符".into());
        }
        if !command_names.insert(name.clone()) {
            return Err(format!("命令名称「{name}」重复"));
        }
        if command.is_empty() {
            return Err(format!("请输入「{name}」的命令"));
        }
        commands.push(ExtraCommand { name, command });
    }
    let auto_open = runnable && draft.auto_open;
    if auto_open && (project_type != "web" || port.is_none()) {
        return Err("仅填写端口的 Web 项目可在启动就绪后自动打开网页".into());
    }
    crate::sidebar::validate_group(conn, draft.group_id.as_deref())?;
    let (pinned, archived) = match id {
        Some(id) => conn.query_row("SELECT pinned, archived FROM projects WHERE id=?1", [id], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(|err| format!("读取项目状态失败：{err}"))?,
        None => (false, false),
    };
    Ok(ProjectConfig {
        id: id.unwrap_or_default().into(),
        name,
        display_name: optional_text(draft.display_name),
        description: optional_text(draft.description),
        path: stored_path(&path),
        runnable,
        port,
        services,
        inject_ports: draft.inject_ports,
        start_command,
        project_type: Some(project_type.into()),
        env: draft.env,
        commands,
        auto_open,
        group_id: draft.group_id,
        pinned,
        archived,
    })
}

fn slug(value: &str) -> String {
    let mut result = String::new();
    let mut separator = false;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            if separator && !result.is_empty() {
                result.push('-');
            }
            result.push(ch.to_ascii_lowercase());
            separator = false;
        } else {
            separator = true;
        }
    }
    result
}

pub fn insert_project(conn: &Connection, draft: ProjectDraft) -> Result<ProjectConfig, String> {
    let transaction =
        rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)
            .map_err(|err| format!("新增项目失败：{err}"))?;
    let mut project = normalize_project(&transaction, draft, None)?;
    let mut base = slug(&project.name);
    if base.is_empty() {
        base = slug(
            Path::new(&expand_tilde(&project.path))
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .as_ref(),
        );
    }
    if base.is_empty() {
        base = "project".into();
    }
    let mut id = base.clone();
    let mut suffix = 2;
    while transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM projects WHERE id = ?1)",
            [&id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(|e| format!("生成项目编号失败：{e}"))?
    {
        id = format!("{base}-{suffix}");
        suffix += 1;
    }
    project.id = id;
    transaction.execute(
        "INSERT INTO projects (id, name, display_name, description, path, port, start_command, type, sort_order, env, commands, auto_open, group_id, services, inject_ports, runnable) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, (SELECT COALESCE(MAX(sort_order), -1) + 1 FROM projects), ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
        params![project.id, project.name, project.display_name, project.description, project.path, project.port, project.start_command, project.project_type, serde_json::to_string(&project.env).map_err(|err| err.to_string())?, serde_json::to_string(&project.commands).map_err(|err| err.to_string())?, project.auto_open, project.group_id, serde_json::to_string(&project.services).map_err(|err| err.to_string())?, project.inject_ports, project.runnable],
    ).map_err(|e| format!("新增项目失败：{e}"))?;
    transaction
        .commit()
        .map_err(|err| format!("新增项目失败：{err}"))?;
    Ok(project)
}

pub fn reorder_project_rows(conn: &mut Connection, ids: &[String]) -> Result<(), String> {
    let transaction = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|error| format!("保存项目排序失败：{error}"))?;
    write_project_order(&transaction, ids)?;
    transaction.commit().map_err(|error| format!("保存项目排序失败：{error}"))
}

pub fn write_project_order(conn: &Connection, ids: &[String]) -> Result<(), String> {
    let current = {
        let mut statement = conn
            .prepare("SELECT id FROM projects")
            .map_err(|error| format!("读取项目列表失败：{error}"))?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| format!("读取项目列表失败：{error}"))?;
        rows.collect::<rusqlite::Result<HashSet<_>>>()
            .map_err(|error| format!("读取项目列表失败：{error}"))?
    };
    let requested: HashSet<_> = ids.iter().cloned().collect();
    if ids.len() != current.len() || requested != current {
        return Err("项目列表已变化，请刷新后重试".into());
    }
    for (position, id) in ids.iter().enumerate() {
        conn
            .execute(
                "UPDATE projects SET sort_order = ?1 WHERE id = ?2",
                params![position as i64, id],
            )
            .map_err(|error| format!("保存项目排序失败：{error}"))?;
    }
    Ok(())
}

pub fn update_project_row(
    conn: &Connection,
    id: &str,
    draft: ProjectDraft,
) -> Result<ProjectConfig, String> {
    let transaction =
        rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)
            .map_err(|err| format!("更新项目失败：{err}"))?;
    let exists: bool = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM projects WHERE id = ?1)",
            [id],
            |row| row.get(0),
        )
        .map_err(|e| format!("查询项目失败：{e}"))?;
    if !exists {
        return Err("未找到项目".into());
    }
    let project = normalize_project(&transaction, draft, Some(id))?;
    transaction.execute(
        "UPDATE projects SET name=?1, display_name=?2, description=?3, path=?4, port=?5, start_command=?6, type=?7, env=?9, commands=?10, auto_open=?11, group_id=?12, services=?13, inject_ports=?14, runnable=?15, updated_at=CURRENT_TIMESTAMP WHERE id=?8",
        params![project.name, project.display_name, project.description, project.path, project.port, project.start_command, project.project_type, id, serde_json::to_string(&project.env).map_err(|err| err.to_string())?, serde_json::to_string(&project.commands).map_err(|err| err.to_string())?, project.auto_open, project.group_id, serde_json::to_string(&project.services).map_err(|err| err.to_string())?, project.inject_ports, project.runnable],
    ).map_err(|e| format!("更新项目失败：{e}"))?;
    transaction
        .commit()
        .map_err(|err| format!("更新项目失败：{err}"))?;
    Ok(project)
}

pub fn set_default_service_row(conn: &Connection, id: &str, service_id: &str) -> Result<(), String> {
    let transaction =
        rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)
            .map_err(|err| format!("设置默认服务失败：{err}"))?;
    let raw: String = transaction.query_row("SELECT services FROM projects WHERE id=?1", [id], |row| row.get(0))
        .optional().map_err(|err| err.to_string())?.ok_or("未找到项目")?;
    let mut services: Vec<Service> = serde_json::from_str(&raw).map_err(|err| err.to_string())?;
    let port = services.iter().find(|service| service.id == service_id).ok_or("未找到服务")?.port;
    if let Some(port) = port {
        validate_project_port(&transaction, port, Some(id))?;
    }
    for service in &mut services {
        service.open_web = service.id == service_id;
    }
    transaction.execute(
        "UPDATE projects SET services=?1, port=?2, auto_open=auto_open AND ?2 IS NOT NULL, updated_at=CURRENT_TIMESTAMP WHERE id=?3",
        params![serde_json::to_string(&services).map_err(|err| err.to_string())?, port, id]
    ).map_err(|err| format!("设置默认服务失败：{err}"))?;
    transaction.commit().map_err(|err| format!("设置默认服务失败：{err}"))
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

fn package_manager(path: &Path, package: &Value) -> Option<String> {
    package["packageManager"].as_str()
        .and_then(|s| s.split('@').next())
        .filter(|s| matches!(*s, "pnpm" | "yarn" | "bun" | "npm"))
        .map(String::from)
        .or_else(|| {
            [("pnpm-lock.yaml", "pnpm"), ("yarn.lock", "yarn"), ("bun.lockb", "bun"),
                ("bun.lock", "bun"), ("package-lock.json", "npm")]
                .iter().find(|(file, _)| path.join(file).is_file()).map(|(_, pm)| pm.to_string())
        })
        .or_else(|| path.join("package.json").is_file().then(|| "npm".into()))
}

fn dependency(package: &Value, name: &str) -> bool {
    ["dependencies", "devDependencies"]
        .iter()
        .any(|key| package[*key].get(name).is_some())
}

fn script_command(pm: &str, script: &str) -> String {
    match pm {
        "pnpm" | "yarn" => format!("{pm} {script}"),
        "npm" if script == "start" => "npm start".into(),
        _ => format!("{pm} run {script}"),
    }
}

fn literal_port(value: &str) -> Option<u16> {
    value
        .trim()
        .trim_matches(['\'', '"'])
        .parse::<u16>()
        .ok()
        .filter(|port| *port > 0)
}

fn script_ports(script: &str) -> Vec<u16> {
    let mut ports = Vec::new();
    let mut words = script
        .split(|ch: char| ch.is_whitespace() || matches!(ch, ';' | '&' | '|'))
        .filter(|word| !word.is_empty());
    while let Some(word) = words.next() {
        let word = word.trim_matches(['\'', '"']);
        let value = if matches!(word, "--port" | "-p") {
            words.next()
        } else {
            word.strip_prefix("--port=")
                .or_else(|| word.strip_prefix("-p="))
                .or_else(|| word.strip_prefix("PORT="))
                .or_else(|| word.strip_prefix("VITE_PORT="))
        };
        if let Some(port) = value.and_then(literal_port) {
            ports.push(port);
        }
    }
    ports
}

fn env_ports(config: &str) -> Vec<u16> {
    config
        .lines()
        .filter_map(|line| {
            let line = line.trim().strip_prefix("export ").unwrap_or(line.trim());
            let (key, value) = line.split_once('=')?;
            if !matches!(key.trim(), "PORT" | "VITE_PORT" | "UVICORN_PORT" | "FLASK_RUN_PORT" | "SERVER_PORT" | "GIN_PORT") {
                return None;
            }
            literal_port(value.split('#').next()?)
        })
        .collect()
}

fn without_js_comments(config: &str) -> String {
    let mut result = String::new();
    let mut chars = config.chars().peekable();
    let mut quote = None;
    while let Some(ch) = chars.next() {
        if let Some(delimiter) = quote {
            result.push(ch);
            if ch == '\\' {
                if let Some(escaped) = chars.next() {
                    result.push(escaped);
                }
            } else if ch == delimiter {
                quote = None;
            }
        } else if matches!(ch, '\'' | '"' | '`') {
            quote = Some(ch);
            result.push(ch);
        } else if ch == '/' && chars.peek() == Some(&'/') {
            for next in chars.by_ref() {
                if next == '\n' {
                    result.push('\n');
                    break;
                }
            }
        } else if ch == '/' && chars.peek() == Some(&'*') {
            chars.next();
            while let Some(next) = chars.next() {
                if next == '*' && chars.peek() == Some(&'/') {
                    chars.next();
                    break;
                }
                if next == '\n' {
                    result.push('\n');
                }
            }
            result.push(' ');
        } else {
            result.push(ch);
        }
    }
    result
}

fn config_ports(config: &str) -> Vec<u16> {
    // Read literal port properties without evaluating JavaScript or walking the project.
    let mut ports = Vec::new();
    for line in without_js_comments(config).lines() {
        for (index, _) in line.match_indices("port") {
            if index > 0
                && line[..index]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_alphanumeric() || c == '_')
            {
                continue;
            }
            let rest = line[index + 4..]
                .trim_start()
                .trim_start_matches(['\'', '"']);
            let Some(rest) = rest.trim_start().strip_prefix(':') else {
                continue;
            };
            let value = rest.trim_start().trim_start_matches(['\'', '"']);
            let digits: String = value.chars().take_while(char::is_ascii_digit).collect();
            if !value[digits.len()..].starts_with(|ch: char| {
                ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '+' | '-')
            }) {
                if let Some(port) = literal_port(&digits) {
                    ports.push(port);
                }
            }
        }
    }
    ports
}

fn compose_ports(config: &str) -> Vec<u16> {
    let mut ports = Vec::new();
    let mut ports_indent = None;
    for line in config.lines() {
        let text = line.split('#').next().unwrap_or_default().trim();
        if text.is_empty() {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        let mappings = if let Some(value) = text.strip_prefix("ports:") {
            ports_indent = Some(indent);
            value
        } else if ports_indent
            .is_some_and(|parent| indent > parent || (indent == parent && text.starts_with('-')))
        {
            text.strip_prefix('-').unwrap_or_default()
        } else {
            ports_indent = None;
            continue;
        };
        for mapping in mappings
            .split(|ch: char| ch.is_whitespace() || matches!(ch, '\'' | '"' | ',' | '[' | ']'))
        {
            let mut fields = mapping.rsplit(':');
            let container = fields
                .next()
                .unwrap_or_default()
                .split('/')
                .next()
                .unwrap_or_default();
            if literal_port(container).is_some() {
                if let Some(host) = fields.next().and_then(literal_port) {
                    ports.push(host);
                }
            }
        }
    }
    ports
}

fn add_port_candidate(
    candidates: &mut Vec<PortCandidate>,
    port: u16,
    source: &str,
    confidence: PortConfidence,
) {
    if port == 0 {
        return;
    }
    if let Some(existing) = candidates
        .iter_mut()
        .find(|candidate| candidate.port == port)
    {
        if !existing.source.split("、").any(|item| item == source) {
            existing.source.push_str("、");
            existing.source.push_str(source);
        }
        existing.confidence = existing.confidence.min(confidence);
    } else {
        candidates.push(PortCandidate {
            port,
            source: source.into(),
            confidence,
        });
    }
}

const CONFIGURED_PORTS: &str = "WITH configured_ports AS (
    SELECT id, port FROM projects WHERE port IS NOT NULL
    UNION
    SELECT p.id, j.atom FROM projects p,
        json_tree(CASE WHEN json_valid(p.services) THEN p.services ELSE '[]' END) j
    WHERE j.key = 'port' AND j.type = 'integer' AND j.atom BETWEEN 1 AND 65535
)";

fn project_using_port(
    conn: &Connection,
    port: u16,
    exclude_id: Option<&str>,
) -> Result<Option<ExistingProject>, String> {
    conn.query_row(
        &format!("{CONFIGURED_PORTS} SELECT p.id, COALESCE(NULLIF(p.display_name, ''), p.name)
        FROM configured_ports c JOIN projects p ON p.id=c.id
        WHERE c.port=?1 AND (?2 IS NULL OR p.id != ?2) ORDER BY p.id LIMIT 1"),
        params![port, exclude_id],
        |row| Ok(ExistingProject { id: row.get(0)?, name: row.get(1)? }),
    ).optional().map_err(|err| format!("检查端口失败：{err}"))
}

fn validate_project_port(conn: &Connection, port: u16, exclude_id: Option<&str>) -> Result<(), String> {
    if let Some(existing) = project_using_port(conn, port, exclude_id)? {
        return Err(format!("端口 {port} 已被项目「{}」使用！各项目端口必须唯一，请更换端口。", existing.name));
    }
    Ok(())
}

fn configured_ports(conn: &Connection, exclude_id: Option<&str>) -> Result<HashSet<u16>, String> {
    // Directory inspection can open a pre-migration database read-only.
    let has_services: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('projects') WHERE name = 'services')",
        [], |row| row.get(0),
    ).map_err(|err| format!("检查端口失败：{err}"))?;
    let sql = if has_services {
        format!("{CONFIGURED_PORTS} SELECT port FROM configured_ports WHERE ?1 IS NULL OR id != ?1")
    } else {
        "SELECT port FROM projects WHERE port IS NOT NULL AND (?1 IS NULL OR id != ?1)".into()
    };
    let mut statement = conn
        .prepare(&sql)
        .map_err(|err| format!("检查端口失败：{err}"))?;
    let rows = statement.query_map([exclude_id], |row| row.get::<_, u16>(0))
        .map_err(|err| format!("检查端口失败：{err}"))?;
    rows.collect::<rusqlite::Result<_>>().map_err(|err| format!("检查端口失败：{err}"))
}

fn suggest_port(
    port: u16,
    used_ports: &HashSet<u16>,
    mut listening: impl FnMut(u16) -> bool,
) -> Option<u16> {
    (port.checked_add(1)?..=u16::MAX)
        .find(|candidate| !used_ports.contains(candidate) && !listening(*candidate))
}

fn conflicting_owners(
    owners: Vec<processes::PortOwner>,
    managed_pids: &HashSet<u32>,
) -> Vec<processes::PortOwner> {
    owners
        .into_iter()
        .filter(|owner| !owner.in_project && !managed_pids.contains(&owner.pid))
        .collect()
}

fn reserved_port_warnings(port: u16) -> Vec<String> {
    let mut warnings = Vec::new();
    if port < 1024 {
        warnings.push("1024 以下的端口可能需要管理员权限".into());
    }
    if cfg!(target_os = "macos") && matches!(port, 5000 | 7000) {
        warnings.push(format!(
            "macOS 的 AirPlay 接收器常占用端口 {port}，可在系统设置中关闭 AirPlay 接收器"
        ));
    }
    warnings
}

pub fn check_port_conflicts(
    conn: &Connection,
    port: u16,
    exclude_id: Option<&str>,
    managed_pids: &HashSet<u32>,
) -> Result<PortConflictCheck, String> {
    Ok(check_ports(conn, &[port], exclude_id, managed_pids)?.remove(&port).unwrap())
}

pub fn check_ports(
    conn: &Connection,
    ports: &[u16],
    exclude_id: Option<&str>,
    managed_pids: &HashSet<u32>,
) -> Result<std::collections::HashMap<u16, PortConflictCheck>, String> {
    if ports.contains(&0) {
        return Err("端口必须是 1–65535 之间的整数".into());
    }
    let project_path: Option<String> = if let Some(id) = exclude_id {
        conn.query_row("SELECT path FROM projects WHERE id = ?1", [id], |row| {
            row.get(0)
        })
        .optional()
        .map_err(|err| format!("查询项目失败：{err}"))?
    } else {
        None
    };
    let mut used_ports = configured_ports(conn, exclude_id)?;
    used_ports.extend(ports);
    let listeners = processes::listening_ports();
    ports.iter().copied().collect::<HashSet<_>>().into_iter().map(|port| {
        let pids = listeners.get(&port).cloned().unwrap_or_default();
        let unknown_listener = pids.is_empty() && processes::probe_loopback(port);
        let owners = conflicting_owners(
            processes::owners_for_pids(pids, project_path.as_deref()),
            managed_pids,
        );
        let in_use = (unknown_listener || !owners.is_empty()).then_some(PortInUse { owners });
        Ok((port, PortConflictCheck {
            project_conflict: project_using_port(conn, port, exclude_id)?,
            in_use,
            reserved: reserved_port_warnings(port),
            suggestion: suggest_port(port, &used_ports, |candidate| {
                listeners.contains_key(&candidate) || processes::probe_loopback(candidate)
            }),
        }))
    }).collect()
}

fn probe_project_ports(
    path: &Path, package: &Value, chosen_script: Option<&str>, start_sh_primary: bool,
) -> (Vec<PortCandidate>, Option<PortCandidate>) {
    let mut candidates = Vec::new();
    let candidates = &mut candidates;
    if let Some((port, source)) = launch::tauri_dev::detected_port(path) {
        add_port_candidate(candidates, port, &source, PortConfidence::High);
    }
    for prefix in ["vite", "next", "nuxt", "astro"] {
        for extension in ["ts", "mts", "js", "mjs", "cjs", "cts"] {
            let file = format!("{prefix}.config.{extension}");
            if let Ok(config) = std::fs::read_to_string(path.join(&file)) {
                for port in config_ports(&config) {
                    add_port_candidate(candidates, port, &file, PortConfidence::High);
                }
            }
        }
    }
    if let Some(angular) = read_json(&path.join("angular.json")) {
        if let Some(projects) = angular["projects"].as_object() {
            for project in projects.values() {
                for key in ["architect", "targets"] {
                    if let Some(port) = project[key]["serve"]["options"]["port"]
                        .as_u64()
                        .and_then(|port| u16::try_from(port).ok())
                    {
                        add_port_candidate(candidates, port, "angular.json", PortConfidence::High);
                    }
                }
            }
        }
    }
    // Tauri launches its frontend dev script; inspect it before alternate scripts.
    let dev_script = package["scripts"]["dev"]
        .as_str()
        .filter(|script| !script.trim().is_empty())
        .or_else(|| {
            package["scripts"]["start"]
                .as_str()
                .filter(|script| !script.trim().is_empty())
        })
        .or(chosen_script);
    if let Some(script) = dev_script {
        for port in script_ports(script) {
            add_port_candidate(candidates, port, "package.json", PortConfidence::High);
        }
    }
    for file in [".env.development", ".env.local", ".env"] {
        if let Ok(config) = std::fs::read_to_string(path.join(file)) {
            for port in env_ports(&config) {
                add_port_candidate(candidates, port, file, PortConfidence::High);
            }
        }
    }
    if let Some(scripts) = package["scripts"].as_object() {
        for script in scripts.values().filter_map(Value::as_str) {
            for port in script_ports(script) {
                add_port_candidate(candidates, port, "package.json", PortConfidence::Medium);
            }
        }
    }
    if let Ok(script) = std::fs::read_to_string(path.join("start.sh")) {
        let confidence = if start_sh_primary {
            PortConfidence::High
        } else {
            PortConfidence::Medium
        };
        for port in script
            .lines()
            .filter(|line| !line.trim_start().starts_with('#'))
            .flat_map(script_ports)
        {
            add_port_candidate(candidates, port, "start.sh", confidence);
        }
    }
    for file in [
        "docker-compose.yml",
        "docker-compose.yaml",
        "compose.yaml",
        "compose.yml",
    ] {
        if let Ok(config) = std::fs::read_to_string(path.join(file)) {
            for port in compose_ports(&config) {
                add_port_candidate(candidates, port, file, PortConfidence::Medium);
            }
        }
    }
    let framework_default = [
        ("next", 3000, "Next.js 默认端口"),
        ("nuxt", 3000, "Nuxt 默认端口"),
        ("astro", 4321, "Astro 默认端口"),
        ("@angular/core", 4200, "Angular 默认端口"),
        ("vite", 5173, "Vite 默认端口"),
    ]
    .into_iter()
    .find(|(name, _, _)| dependency(&package, name));
    if let Some((_, port, _)) = framework_default {
        add_port_candidate(candidates, port, "框架默认", PortConfidence::Low);
    }
    candidates.sort_by_key(|candidate| candidate.confidence);
    // Alternate scripts/Compose remain candidates, but must not replace the dev server's default.
    let recommended = candidates
        .iter()
        .find(|candidate| candidate.confidence == PortConfidence::High)
        .or_else(|| {
            framework_default
                .and_then(|(_, port, _)| candidates.iter().find(|candidate| candidate.port == port))
        })
        .or_else(|| candidates.first());
    let detected = recommended.map(|candidate| {
        let source = candidate.source.split("、").next().unwrap_or_default();
        let source = match source {
            "框架默认" => framework_default
                .map(|(_, _, label)| label)
                .unwrap_or(source),
            _ => source.strip_prefix("src-tauri/").unwrap_or(source),
        };
        PortCandidate { port: candidate.port, source: source.to_string(), confidence: candidate.confidence }
    });
    (std::mem::take(candidates), detected)
}

pub fn inspect_project_dir(
    conn: &Connection,
    input: &str,
    exclude_id: Option<&str>,
) -> Result<ProjectDirInspection, String> {
    let path = expanded_path(input);
    let exists = !input.trim().is_empty() && path.exists();
    let is_dir = exists && path.is_dir();
    let mut result = ProjectDirInspection {
        path: if input.trim().is_empty() {
            String::new()
        } else {
            stored_path(&path)
        },
        exists,
        is_dir,
        name: path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into(),
        project_type: "web".into(),
        package_manager: None,
        commands: Vec::new(),
        port: None,
        port_source: None,
        port_candidates: Vec::new(),
        existing_project: None,
        services: Vec::new(),
        start_scripts: Vec::new(),
        standard: None,
    };
    if !is_dir {
        return Ok(result);
    }
    let package_path = path.join("package.json");
    let package = read_json(&package_path).unwrap_or(Value::Null);
    if let Some(name) = package["name"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        result.name = name.into();
    }
    if launch::tauri_dev::has_config(&path)
        || dependency(&package, "electron")
        || dependency(&package, "@tauri-apps/cli")
    {
        result.project_type = "app".into();
    }
    result.package_manager = package_manager(&path, &package);
    let mut chosen_script = None;
    if let Some(pm) = &result.package_manager {
        for (script, suffix) in [("tauri", " dev"), ("dev", ""), ("start", "")] {
            if script == "tauri" && result.project_type != "app" {
                continue;
            }
            if let Some(value) = package["scripts"][script]
                .as_str()
                .filter(|s| !s.trim().is_empty())
            {
                result
                    .commands
                    .push(format!("{}{suffix}", script_command(pm, script)));
                if chosen_script.is_none() {
                    chosen_script = Some(value);
                }
            }
        }
    }
    if path.join("start.sh").is_file() {
        result.commands.push("./start.sh".into());
    }
    result.commands.dedup();
    result.commands.truncate(4);
    let (port_candidates, mut detected) = probe_project_ports(&path, &package, chosen_script,
        result.commands.first().is_some_and(|command| command == "./start.sh"));
    result.port_candidates = port_candidates;
    (result.services, result.start_scripts) = scan_services(&path, &package, result.package_manager.as_deref());
    if result.services.len() >= 2 {
        if let Some(script) = result.start_scripts.first() {
            result.commands.retain(|command| command != &script.command);
            result.commands.insert(0, script.command.clone());
            result.commands.truncate(4);
        }
        detected = result.services.iter().find(|row| row.service.open_web).and_then(|row| row.port_candidate.clone());
    }
    let mut stmt = conn.prepare("SELECT id, COALESCE(display_name, name), path FROM projects WHERE ?1 IS NULL OR id != ?1")
        .map_err(|e| format!("查询项目失败：{e}"))?;
    let rows = stmt.query_map([exclude_id], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?))
    }).map_err(|e| format!("读取项目失败：{e}"))?;
    for row in rows {
        let (id, name, other_path) = row.map_err(|e| format!("读取项目失败：{e}"))?;
        if expanded_path(&other_path) == path {
            result.existing_project = Some(ExistingProject { id, name });
            break;
        }
    }
    if detected.is_none() && result.project_type == "web" && result.services.len() < 2 {
        let used_ports = configured_ports(conn, exclude_id)?;
        for port in 5173..5373 {
            if !used_ports.contains(&port) && !check_port_in_use_sync(port).0 {
                detected = Some(PortCandidate { port, source: "推荐空闲端口".into(), confidence: PortConfidence::Low });
                break;
            }
        }
    }
    if let Some(port) = detected {
        result.port = Some(port.port);
        result.port_source = Some(port.source);
    }
    if result.services.len() >= 2 {
        let audit_services: Vec<_> = result.services.iter().map(|row| row.service.clone()).collect();
        let command = result.commands.first().map(String::as_str).unwrap_or_default();
        result.standard = Some(standard::audit(&path, command, &audit_services));
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "apphub-projects-{}-{}",
                std::process::id(),
                NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn write(&self, file: &str, text: &str) {
            let path = self.0.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        fn draft(&self) -> ProjectDraft {
            ProjectDraft {
                name: " My Project! ".into(),
                display_name: Some("  ".into()),
                description: Some("  描述  ".into()),
                path: format!("{}/", self.0.display()),
                runnable: true,
                port: Some(5199),
                services: Vec::new(),
                inject_ports: true,
                start_command: "  pnpm dev  ".into(),
                project_type: None,
                env: Vec::new(),
                commands: Vec::new(),
                auto_open: false,
                group_id: None,
            }
        }
        fn inspect(&self, conn: &Connection) -> ProjectDirInspection {
            inspect_project_dir(conn, self.0.to_str().unwrap(), None).unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn database() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::init_db(&conn).unwrap();
        conn
    }

    #[test]
    fn scans_frontends_backends_and_workspace_directories_with_provenance() {
        let fixture = Fixture::new();
        fixture.write("apps/frontend/package.json", r#"{"scripts":{"dev":"vite --port 5177"}}"#);
        fixture.write("packages/backend/pyproject.toml", r#"dependencies = ["uvicorn"]"#);
        fixture.write("packages/backend/.env", "PORT=8040\n");
        fixture.write("services/spring/pom.xml", "<artifactId>spring-boot</artifactId>");
        fixture.write("services/spring/src/main/resources/application.yml", "server:\n  port: 8093\n");
        fixture.write("gin/go.mod", "require github.com/gin-gonic/gin v1.0.0");
        fixture.write("gin/main.go", r#"func main() { router.Run(":8094") }"#);
        fixture.write("flask/requirements.txt", "Flask");
        fixture.write("flask/.flaskenv", "FLASK_RUN_PORT=8095\n");
        fixture.write("worker/Cargo.toml", "[package]");
        fixture.write("scripts/dev.sh", "#!/bin/sh\nwait\n");
        fixture.write("Makefile", "dev: boot\nstart: dev\n");
        fixture.write("Procfile.dev", "web: pnpm dev\n");
        fixture.write("package.json", r#"{"packageManager":"pnpm@9","scripts":{"dev":"vite","start":"concurrently web api"}}"#);
        for dir in ["node_modules", ".git", "dist", "build", "target", ".venv", "vendor"] {
            fixture.write(&format!("{dir}/package.json"), "{}");
            fixture.write(&format!("apps/{dir}/package.json"), "{}");
        }
        let result = fixture.inspect(&database());
        assert_eq!(result.services.len(), 6);
        let front = &result.services[0];
        assert_eq!(front.service.dir, "apps/frontend");
        assert_eq!(front.service.name, "前端");
        assert!(front.service.open_web);
        assert_eq!(result.port, Some(5177));
        let candidates = [
            ("apps/frontend", 5177, "package.json"),
            ("packages/backend", 8040, ".env"),
            ("services/spring", 8093, "src/main/resources/application.yml"),
            ("gin", 8094, "main.go"),
            ("flask", 8095, ".flaskenv"),
        ];
        for (dir, port, source) in candidates {
            let service = result.services.iter().find(|row| row.service.dir == dir).unwrap();
            let candidate = service.port_candidate.as_ref().unwrap();
            assert_eq!((candidate.port, candidate.source.as_str(), candidate.confidence), (port, source, PortConfidence::High));
        }
        assert!(result.services.iter().find(|row| row.service.dir == "worker").unwrap().service.port.is_none());
        assert_eq!(result.commands[0], "./scripts/dev.sh");
        assert_eq!(result.start_scripts.iter().map(|script| script.command.as_str()).collect::<Vec<_>>(),
            ["./scripts/dev.sh", "make dev", "make start", "foreman start -f Procfile.dev", "pnpm start", "pnpm dev"]);
    }

    #[test]
    fn scans_prefixed_service_directories_and_literal_environment_fallbacks() {
        let fixture = Fixture::new();
        fixture.write("demo-frontend/package.json", r#"{"devDependencies":{"vite":"*"},"scripts":{"dev":"vite"}}"#);
        fixture.write("demo-frontend/vite.config.ts", "const port = Number(env.DEMO_WEB_PORT || 3100);");
        fixture.write("demo-api/pyproject.toml", r#"dependencies = ["uvicorn"]"#);
        fixture.write("scripts/dev.sh", r#"api_port = int(os.environ.get("DEMO_API_PORT", "8100"))"#);
        let result = fixture.inspect(&database());
        assert_eq!(result.services.iter().map(|row| (row.service.dir.as_str(), row.service.port, row.service.open_web)).collect::<Vec<_>>(),
            [("demo-frontend", Some(3100), true), ("demo-api", Some(8100), false)]);
        let api = result.services[1].port_candidate.as_ref().unwrap();
        assert_eq!(api.confidence, PortConfidence::Medium);
        assert_eq!(api.source, "scripts/dev.sh · DEMO_API_PORT");
    }

    #[test]
    fn service_ports_round_trip_are_unique_and_follow_the_default_choice() {
        let conn = database();
        let fixture = Fixture::new();
        let mut draft = fixture.draft();
        draft.port = Some(9999);
        draft.services = vec![
            Service { key: String::new(), id: "web".into(), name: "前端".into(), dir: "./apps/frontend/".into(), port: Some(5199), kind: Some("web".into()), open_web: true, start_command: None },
            Service { key: String::new(), id: "api".into(), name: "后端".into(), dir: "packages/back-end".into(), port: Some(8500), kind: None, open_web: false, start_command: None },
        ];
        draft.inject_ports = false;
        let project = insert_project(&conn, draft.clone()).unwrap();
        assert_eq!(project.port, Some(5199));
        assert_eq!(project.services[0].dir, "apps/frontend");
        assert_eq!(crate::read_projects(&conn).unwrap()[0].services, project.services);
        assert!(!crate::read_projects(&conn).unwrap()[0].inject_ports);
        let mut other = fixture.draft();
        other.port = Some(8500);
        assert!(insert_project(&conn, other).unwrap_err().contains("8500"));
        assert_eq!(project_using_port(&conn, 8500, None).unwrap().unwrap().id, project.id);
        assert!(project_using_port(&conn, 8500, Some(&project.id)).unwrap().is_none());
        set_default_service_row(&conn, &project.id, "api").unwrap();
        let project = crate::read_projects(&conn).unwrap().remove(0);
        assert_eq!(project.port, Some(8500));
        assert!(project.services[1].open_web && !project.services[0].open_web);
        draft.services[1].open_web = true;
        assert!(update_project_row(&conn, &project.id, draft.clone()).unwrap_err().contains("请选择"));
        draft.services[0].open_web = false;
        draft.services[0].dir = "../frontend".into();
        assert!(update_project_row(&conn, &project.id, draft).unwrap_err().contains("相对目录"));
    }

    #[test]
    fn injected_environment_preserves_explicit_values_and_supports_legacy_ports() {
        let conn = database();
        let fixture = Fixture::new();
        let mut project = insert_project(&conn, fixture.draft()).unwrap();
        let launch_environment = |project: &ProjectConfig| {
            services::launch_environment(&project.env, project.port, &project.services, project.inject_ports)
        };
        assert_eq!(launch_environment(&project), [EnvVar { key: "PORT".into(), value: "5199".into() }]);
        project.services = vec![
            Service { key: "WEB".into(), id: "web".into(), name: "前端".into(), dir: "apps/front-end".into(), port: Some(5199), kind: Some("web".into()), open_web: true, start_command: None },
            Service { key: "API".into(), id: "api".into(), name: "后端".into(), dir: "server".into(), port: Some(8110), kind: Some("api".into()), open_web: false, start_command: None },
            Service { key: "WORKER".into(), id: "worker".into(), name: "Worker".into(), dir: "worker".into(), port: None, kind: None, open_web: false, start_command: None },
        ];
        project.env = vec![EnvVar { key: "PORT".into(), value: "explicit".into() }, EnvVar { key: "BENTO_PORT_API".into(), value: "8111".into() }];
        let env = launch_environment(&project);
        assert_eq!(env.into_iter().map(|entry| (entry.key, entry.value)).collect::<Vec<_>>(), [
            ("PORT".into(), "explicit".into()),
            ("BENTO_PORT_API".into(), "8111".into()),
            ("BENTO_PORT_WEB".into(), "5199".into()),
            ("BENTO_PORTS".into(), "WEB:5199,API:8110".into()),
        ]);
        project.inject_ports = false;
        assert_eq!(launch_environment(&project), project.env);
        project.services.truncate(1);
        project.inject_ports = true;
        project.env.clear();
        assert_eq!(launch_environment(&project), [EnvVar { key: "PORT".into(), value: "5199".into() }]);
    }

    #[test]
    fn v6_service_key_migration_preserves_explicit_keys_and_numbers_role_duplicates() {
        let conn = database();
        let entries = serde_json::json!([
            {"id":"web-1","name":"Web","dir":"apps/front","port":3100,"kind":"web","openWeb":true},
            {"id":"web-2","name":"Admin","dir":"apps/admin","port":null,"kind":"web","openWeb":false},
            {"id":"api","name":"API","dir":"services/api","port":8100,"kind":"api","openWeb":false},
            {"id":"worker","name":"Worker","dir":"my.worker","port":null,"openWeb":false},
            {"id":"custom","name":"Custom","dir":"existing","port":8120,"key":"WEB","openWeb":false}
        ]);
        conn.execute("INSERT INTO projects(id,name,path,port,start_command,services) VALUES('v6','V6','/path',3100,'./dev.sh',?1)",
            [entries.to_string()]).unwrap();
        crate::init_db(&conn).unwrap();
        let read = || conn.query_row("SELECT services FROM projects WHERE id='v6'", [], |row| row.get::<_, String>(0)).unwrap();
        let migrated = read();
        let services: Vec<Service> = serde_json::from_str(&migrated).unwrap();
        assert_eq!(services.iter().map(|service| service.key.as_str()).collect::<Vec<_>>(), ["WEB_2", "WEB_3", "API", "MY_WORKER", "WEB"]);
        assert_eq!(normalize_services(services.clone()).unwrap(), services);
        crate::init_db(&conn).unwrap();
        assert_eq!(read(), migrated);
        let mut duplicate = services.clone();
        duplicate[1].key = duplicate[0].key.clone();
        assert!(normalize_services(duplicate).unwrap_err().contains("重复"));
        let mut invalid = services;
        invalid[0].key = "web-key".into();
        assert!(normalize_services(invalid).unwrap_err().contains("大写字母"));
    }

    #[test]
    fn environment_validation_and_reserved_names() {
        let entry = |key: &str| EnvVar {
            key: key.into(),
            value: "' \" $HOME ` 中文\n".into(),
        };
        for key in ["_", "A1", "a", "PATH", "term"] {
            validate_env(&[entry(key)]).unwrap();
        }
        for key in ["", "1A", "bad-key", "中文", "A B", "A;echo", "A\n"] {
            assert!(validate_env(&[entry(key)]).unwrap_err().contains("名称须"));
        }
        for key in ["TERM", "COLORTERM", "SHELL", "HOME", "USER", "PWD"] {
            assert!(validate_env(&[entry(key)])
                .unwrap_err()
                .contains("保留变量"));
        }
        assert!(validate_env(&[entry("A"), entry("A")])
            .unwrap_err()
            .contains("重复"));
        validate_env(&[entry("A"), entry("a")]).unwrap();
    }

    #[test]
    fn advanced_configuration_round_trips_and_validates_commands() {
        let conn = database();
        let fixture = Fixture::new();
        let mut draft = fixture.draft();
        draft.env = vec![
            EnvVar {
                key: "EMPTY".into(),
                value: "".into(),
            },
            EnvVar {
                key: "TEXT".into(),
                value: "  中文\n'$HOME`  ".into(),
            },
        ];
        draft.commands = vec![ExtraCommand {
            name: " build ".into(),
            command: " pnpm build ".into(),
        }];
        draft.auto_open = true;
        let saved = insert_project(&conn, draft.clone()).unwrap();
        let read = crate::read_projects(&conn).unwrap().remove(0);
        assert_eq!(read.env, draft.env);
        assert_eq!(
            read.commands,
            vec![ExtraCommand {
                name: "build".into(),
                command: "pnpm build".into()
            }]
        );
        assert!(read.auto_open);
        draft.commands.push(ExtraCommand {
            name: "build".into(),
            command: "echo 2".into(),
        });
        assert!(update_project_row(&conn, &saved.id, draft.clone())
            .unwrap_err()
            .contains("重复"));
        for (name, command, expected) in [
            ("", "echo", "名称"),
            ("abcdefghijklmnopqrstu", "echo", "20"),
            ("test", "  ", "请输入"),
        ] {
            draft.commands = vec![ExtraCommand {
                name: name.into(),
                command: command.into(),
            }];
            assert!(update_project_row(&conn, &saved.id, draft.clone())
                .unwrap_err()
                .contains(expected));
        }
        draft.commands = (0..11)
            .map(|index| ExtraCommand {
                name: index.to_string(),
                command: "echo".into(),
            })
            .collect();
        assert!(update_project_row(&conn, &saved.id, draft.clone())
            .unwrap_err()
            .contains("10"));
        draft.commands.clear();
        draft.project_type = Some("app".into());
        assert!(update_project_row(&conn, &saved.id, draft.clone())
            .unwrap_err()
            .contains("Web"));
        draft.auto_open = false;
        draft.env.clear();
        let updated = update_project_row(&conn, &saved.id, draft).unwrap();
        assert!(updated.env.is_empty() && updated.commands.is_empty() && !updated.auto_open);
    }

    #[test]
    fn projects_without_running_need_no_command_or_port_and_keep_terminal_settings() {
        let conn = database();
        let fixture = Fixture::new();
        let mut draft = fixture.draft();
        draft.runnable = false;
        draft.start_command = "  ".into();
        draft.port = Some(70000);
        draft.services = vec![Service { key: "WEB".into(), id: "web".into(), name: "前端".into(), dir: "web".into(), port: Some(5199), kind: Some("web".into()), open_web: true, start_command: None }];
        draft.auto_open = true;
        draft.env = vec![EnvVar { key: "MODE".into(), value: "local".into() }];
        draft.commands = vec![ExtraCommand { name: "test".into(), command: "pnpm test".into() }];
        let saved = insert_project(&conn, draft.clone()).unwrap();
        let read = crate::read_projects(&conn).unwrap().remove(0);
        assert!(!read.runnable && read.start_command.is_empty() && read.port.is_none() && read.services.is_empty() && !read.auto_open);
        assert_eq!((read.env, read.commands), (draft.env.clone(), vec![ExtraCommand { name: "test".into(), command: "pnpm test".into() }]));
        let other = insert_project(&conn, fixture.draft()).unwrap();
        assert_eq!(other.port, Some(5199));
        assert!(crate::calculate_conflicts(&crate::read_projects(&conn).unwrap()).is_empty());
        draft.runnable = true;
        draft.services.clear();
        draft.auto_open = false;
        assert!(update_project_row(&conn, &saved.id, draft.clone()).unwrap_err().contains("启动命令"));
        draft.start_command = "pnpm dev".into();
        draft.port = Some(5199);
        assert!(update_project_row(&conn, &saved.id, draft.clone()).unwrap_err().contains("5199"));
        draft.port = None;
        let running = update_project_row(&conn, &saved.id, draft).unwrap();
        assert!(running.runnable && running.start_command == "pnpm dev");
        assert!(crate::read_projects(&conn).unwrap().iter().all(|project| project.runnable));
    }

    #[test]
    fn parses_json_lists_and_defaults_older_payloads() {
        for raw in [
            None,
            Some("null"),
            Some("not JSON"),
            Some("{}"),
            Some("[{}]"),
            Some("[{\"key\":5,\"value\":true}]"),
        ] {
            assert!(parse_list::<EnvVar>(raw, "fixture", "env").is_empty());
            assert!(parse_list::<ExtraCommand>(raw, "fixture", "commands").is_empty());
        }
        assert_eq!(
            parse_list::<EnvVar>(Some("[{\"key\":\"A\",\"value\":\"\"}]"), "fixture", "env"),
            vec![EnvVar {
                key: "A".into(),
                value: "".into()
            }]
        );
        let old =
            serde_json::json!({"id":"old","name":"old","path":"/tmp","startCommand":"sleep 1"});
        let config: ProjectConfig = serde_json::from_value(old.clone()).unwrap();
        let draft: ProjectDraft = serde_json::from_value(old).unwrap();
        assert!(config.env.is_empty() && config.commands.is_empty() && !config.auto_open);
        assert!(config.services.is_empty() && config.inject_ports && config.runnable);
        assert!(draft.env.is_empty() && draft.commands.is_empty() && !draft.auto_open);
        assert!(draft.services.is_empty() && draft.inject_ports && draft.runnable);
    }

    #[test]
    fn insert_appends_after_maximum_sort_order() {
        let conn = database();
        let fixture = Fixture::new();
        let first = insert_project(&conn, fixture.draft()).unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT sort_order FROM projects WHERE id = ?1",
                [&first.id],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        conn.execute("UPDATE projects SET sort_order = 7", [])
            .unwrap();
        conn.execute("INSERT INTO projects (id,name,path,start_command) VALUES ('cli','CLI','/tmp','sleep 1')", []).unwrap();
        let mut draft = fixture.draft();
        draft.name = "Second".into();
        draft.port = Some(5198);
        let second = insert_project(&conn, draft).unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT sort_order FROM projects WHERE id = ?1",
                [&second.id],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            8
        );
        assert_eq!(
            crate::read_projects(&conn)
                .unwrap()
                .into_iter()
                .map(|project| project.id)
                .collect::<Vec<_>>(),
            [first.id, second.id, "cli".into()]
        );
    }

    fn ordering_fixture() -> Connection {
        let conn = database();
        conn.execute_batch("INSERT INTO projects (id,name,path,start_command,sort_order) VALUES ('a','a','/tmp','sleep 1',0),('b','b','/tmp','sleep 1',1),('c','c','/tmp','sleep 1',2)").unwrap();
        conn
    }

    #[test]
    fn reordering_validates_exact_id_set_and_persists_all_positions() {
        let mut conn = ordering_fixture();
        for invalid in [
            vec!["a", "b"],
            vec!["a", "a", "c"],
            vec!["a", "b", "other"],
            vec!["a", "b", "c", "a"],
        ] {
            assert_eq!(
                reorder_project_rows(
                    &mut conn,
                    &invalid.into_iter().map(String::from).collect::<Vec<_>>()
                )
                .unwrap_err(),
                "项目列表已变化，请刷新后重试"
            );
            assert_eq!(
                crate::read_projects(&conn)
                    .unwrap()
                    .into_iter()
                    .map(|project| project.id)
                    .collect::<Vec<_>>(),
                ["a", "b", "c"]
            );
        }
        let ids = ["c", "a", "b"].map(String::from);
        reorder_project_rows(&mut conn, &ids).unwrap();
        assert_eq!(
            crate::read_projects(&conn)
                .unwrap()
                .into_iter()
                .map(|project| project.id)
                .collect::<Vec<_>>(),
            ids
        );
        for (position, id) in ids.iter().enumerate() {
            assert_eq!(
                conn.query_row("SELECT sort_order FROM projects WHERE id=?1", [id], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
                position as i64
            );
        }
        let mut empty = database();
        reorder_project_rows(&mut empty, &[]).unwrap();
    }

    #[test]
    fn reordering_rolls_back_every_update_if_a_write_fails() {
        let mut conn = ordering_fixture();
        conn.execute_batch("CREATE TRIGGER fail_reorder BEFORE UPDATE ON projects WHEN NEW.id='b' BEGIN SELECT RAISE(ABORT, 'forced reorder failure'); END").unwrap();
        let error =
            reorder_project_rows(&mut conn, &["c".into(), "b".into(), "a".into()]).unwrap_err();
        assert!(error.contains("forced reorder failure"));
        assert_eq!(
            crate::read_projects(&conn)
                .unwrap()
                .into_iter()
                .map(|project| project.id)
                .collect::<Vec<_>>(),
            ["a", "b", "c"]
        );
        assert_eq!(
            conn.query_row("SELECT sort_order FROM projects WHERE id='c'", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
            2
        );
    }

    #[test]
    fn ids_and_normalization() {
        let conn = database();
        let f = Fixture::new();
        let first = insert_project(&conn, f.draft()).unwrap();
        assert_eq!(first.id, "my-project");
        assert_eq!(first.name, "My Project!");
        assert_eq!(first.start_command, "pnpm dev");
        assert_eq!(first.description.as_deref(), Some("描述"));
        assert!(first.display_name.is_none());
        assert!(!first.path.ends_with('/'));
        let mut draft = f.draft();
        draft.port = None;
        draft.project_type = Some("app".into());
        assert_eq!(
            insert_project(&conn, draft.clone()).unwrap().id,
            "my-project-2"
        );
        draft.name = "中文名称".into();
        assert_eq!(
            insert_project(&conn, draft.clone()).unwrap().id,
            slug(f.0.file_name().unwrap().to_str().unwrap())
        );
        let chinese = f.0.join("中文目录");
        std::fs::create_dir(&chinese).unwrap();
        draft.path = chinese.to_str().unwrap().into();
        assert_eq!(insert_project(&conn, draft).unwrap().id, "project");
    }

    #[test]
    fn validation_and_immutable_update_id() {
        let conn = database();
        let f = Fixture::new();
        f.write("file.txt", "not a directory");
        for (field, value, expected) in [
            (
                "path",
                f.0.join("missing").to_string_lossy().to_string(),
                "目录不存在",
            ),
            (
                "path",
                f.0.join("file.txt").to_string_lossy().to_string(),
                "不是文件夹",
            ),
            ("path", " ".into(), "请输入项目目录"),
            ("name", " ".into(), "请输入项目名称"),
            ("command", " ".into(), "请输入启动命令"),
            ("type", "invalid".into(), "项目类型必须"),
        ] {
            let mut draft = f.draft();
            match field {
                "path" => draft.path = value,
                "name" => draft.name = value,
                "command" => draft.start_command = value,
                _ => draft.project_type = Some(value),
            }
            assert!(insert_project(&conn, draft).unwrap_err().contains(expected));
        }
        for port in [Some(0), Some(-1), Some(65536)] {
            let mut draft = f.draft();
            draft.port = port;
            assert!(insert_project(&conn, draft).unwrap_err().contains("1–65535"));
        }
        let project = insert_project(&conn, f.draft()).unwrap();
        assert!(insert_project(&conn, f.draft())
            .unwrap_err()
            .contains("已被项目「My Project!」使用"));
        let mut updated = f.draft();
        updated.name = "New name".into();
        assert_eq!(
            update_project_row(&conn, &project.id, updated.clone())
                .unwrap()
                .id,
            project.id
        );
        assert_eq!(
            update_project_row(&conn, "missing", updated).unwrap_err(),
            "未找到项目"
        );
        let mut blank = f.draft();
        blank.port = None;
        assert_eq!(update_project_row(&conn, &project.id, blank).unwrap().port, None);
        assert_eq!(crate::read_projects(&conn).unwrap()[0].port, None);
        conn.execute("DELETE FROM projects", []).unwrap();
        crate::init_db(&conn).unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM projects", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    #[test]
    fn home_collapse_and_trailing_slashes() {
        let conn = database();
        let f = Fixture::new();
        let mut draft = f.draft();
        draft.path = format!("{}///", dirs::home_dir().unwrap().display());
        assert_eq!(insert_project(&conn, draft).unwrap().path, "~");
        let mut draft = f.draft();
        draft.path = "~/".into();
        draft.port = Some(5200);
        assert_eq!(insert_project(&conn, draft).unwrap().path, "~");
        let home = dirs::home_dir().unwrap();
        assert_eq!(
            stored_path(&home.join("Projects/example")),
            "~/Projects/example"
        );
        assert_eq!(expanded_path("~///"), home.canonicalize().unwrap());
    }

    #[test]
    fn inspect_pnpm_vite_and_existing_project() {
        let conn = database();
        let f = Fixture::new();
        f.write("package.json", r#"{"name":"vite-fixture","scripts":{"dev":"vite --port=5201","start":"vite preview"},"devDependencies":{"vite":"*"}}"#);
        f.write("pnpm-lock.yaml", "lockfileVersion: '9.0'");
        f.write(
            "vite.config.ts",
            "export default { server: { port: 5199 } }",
        );
        let inspection = f.inspect(&conn);
        assert_eq!(inspection.name, "vite-fixture");
        assert_eq!(inspection.project_type, "web");
        assert_eq!(inspection.package_manager.as_deref(), Some("pnpm"));
        assert_eq!(inspection.commands, ["pnpm dev", "pnpm start"]);
        assert_eq!(inspection.port, Some(5199));
        assert_eq!(inspection.port_source.as_deref(), Some("vite.config.ts"));
        let project = insert_project(&conn, f.draft()).unwrap();
        assert_eq!(f.inspect(&conn).existing_project.unwrap().id, project.id);
        assert!(
            inspect_project_dir(&conn, f.0.to_str().unwrap(), Some(&project.id))
                .unwrap()
                .existing_project
                .is_none()
        );
    }

    #[test]
    fn inspect_tauri_app() {
        let conn = database();
        let f = Fixture::new();
        f.write("package.json", r#"{"name":"desktop","packageManager":"pnpm@9.0.0","scripts":{"tauri":"tauri","dev":"vite --port 5198"},"devDependencies":{"vite":"*"}}"#);
        f.write("yarn.lock", "");
        f.write(
            "src-tauri/tauri.conf.json",
            r#"{"build":{"devUrl":"http://localhost:1499"}}"#,
        );
        let inspection = f.inspect(&conn);
        assert_eq!(inspection.project_type, "app");
        assert_eq!(inspection.package_manager.as_deref(), Some("pnpm"));
        assert_eq!(inspection.commands, ["pnpm tauri dev", "pnpm dev"]);
        assert_eq!(inspection.port, Some(1499));
        assert_eq!(inspection.port_source.as_deref(), Some("tauri.conf.json"));
        f.write("src-tauri/tauri.conf.json", "{invalid");
        assert_eq!(f.inspect(&conn).port, Some(5198));
        f.write("package.json", r#"{"devDependencies":{"electron":"*"}}"#);
        assert!(f.inspect(&conn).port.is_none());
    }

    #[test]
    fn inspect_package_managers_scripts_and_defaults() {
        let conn = database();
        let npm = Fixture::new();
        npm.write(
            "package.json",
            r#"{"scripts":{"start":"next start -p 3100"},"dependencies":{"next":"*"}}"#,
        );
        assert_eq!(npm.inspect(&conn).commands, ["npm start"]);
        assert_eq!(npm.inspect(&conn).port, Some(3100));
        assert_eq!(
            npm.inspect(&conn).port_source.as_deref(),
            Some("package.json")
        );
        for (file, pm, command) in [
            ("yarn.lock", "yarn", "yarn dev"),
            ("bun.lockb", "bun", "bun run dev"),
            ("bun.lock", "bun", "bun run dev"),
            ("package-lock.json", "npm", "npm run dev"),
        ] {
            let f = Fixture::new();
            f.write(
                "package.json",
                r#"{"scripts":{"dev":"vite --port 5202","start":"vite"}}"#,
            );
            f.write(file, "");
            assert_eq!(f.inspect(&conn).package_manager.as_deref(), Some(pm));
            assert_eq!(f.inspect(&conn).commands[0], command);
            assert_eq!(f.inspect(&conn).port, Some(5202));
        }
        npm.write("package.json", r#"{"dependencies":{"next":"*"}}"#);
        assert_eq!(
            npm.inspect(&conn).port_source.as_deref(),
            Some("Next.js 默认端口")
        );
        npm.write("package.json", r#"{"devDependencies":{"vite":"*"}}"#);
        assert_eq!(
            npm.inspect(&conn).port_source.as_deref(),
            Some("Vite 默认端口")
        );
        npm.write(
            "package.json",
            r#"{"scripts":{"tauri":"tauri"},"devDependencies":{"@tauri-apps/cli":"*"}}"#,
        );
        assert_eq!(npm.inspect(&conn).commands, ["npm run tauri dev"]);
    }

    #[test]
    fn inspect_invalid_paths_malformed_json_and_free_port() {
        let conn = database();
        let f = Fixture::new();
        let mut draft = f.draft();
        draft.port = Some(5173);
        insert_project(&conn, draft).unwrap();
        f.write("package.json", "{malformed json");
        f.write("start.sh", "#!/bin/sh\ntrue\n");
        let result = f.inspect(&conn);
        assert_eq!(result.commands, ["./start.sh"]);
        assert_eq!(result.port_source.as_deref(), Some("推荐空闲端口"));
        let port = result.port.unwrap();
        assert!((5174..5373).contains(&port));
        assert!(!check_port_in_use_sync(port).0);
        let missing =
            inspect_project_dir(&conn, f.0.join("missing").to_str().unwrap(), None).unwrap();
        assert!(
            !missing.exists
                && !missing.is_dir
                && missing.commands.is_empty()
                && missing.port.is_none()
        );
        let file =
            inspect_project_dir(&conn, f.0.join("package.json").to_str().unwrap(), None).unwrap();
        assert!(file.exists && !file.is_dir);
    }

    #[test]
    fn scan_env_scripts_compose_and_merge_sources_by_confidence() {
        let conn = database();
        let f = Fixture::new();
        f.write(
            ".env",
            "# PORT=9999\nPORT=5300\nVITE_PORT=0\nOTHER_PORT=9998\nPORT=65536\n",
        );
        f.write(
            ".env.local",
            "export VITE_PORT = '5301' # local\nPORT=not-a-number\n",
        );
        f.write(".env.development", "PORT=5302\n");
        f.write("package.json", r#"{"scripts":{"dev":"cross-env PORT=5303 vite --port   5304","preview":"vite preview -p\t5305","serve":"PORT='5306' node server.js; node other.js --port=5307"},"devDependencies":{"vite":"*"}}"#);
        f.write("docker-compose.yml", "services:\n  web:\n    ports:\n      - \"5300:80\"\n      - '127.0.0.1:5308:8080'\n      - 5309:3000/tcp\n    volumes:\n      - \"9000:9001\"\n");
        f.write(
            "compose.yaml",
            "services:\n  other:\n    ports: [\"5310:80\", \"5300:3000\"]\n",
        );
        let result = f.inspect(&conn);
        assert_eq!(result.port, Some(5303));
        assert_eq!(result.port_source.as_deref(), Some("package.json"));
        for port in 5300..=5310 {
            assert!(
                result
                    .port_candidates
                    .iter()
                    .any(|candidate| candidate.port == port),
                "missing {port}"
            );
        }
        let merged = result
            .port_candidates
            .iter()
            .find(|candidate| candidate.port == 5300)
            .unwrap();
        assert_eq!(merged.source, ".env、docker-compose.yml、compose.yaml");
        assert_eq!(merged.confidence, PortConfidence::High);
        assert_eq!(
            result
                .port_candidates
                .iter()
                .filter(|candidate| candidate.port == 5300)
                .count(),
            1
        );
        assert!(
            !result
                .port_candidates
                .iter()
                .any(|candidate| [0, 80, 3000, 8080, 9000, 9001, 9998, 9999]
                    .contains(&candidate.port))
        );
        assert!(result
            .port_candidates
            .windows(2)
            .all(|pair| pair[0].confidence <= pair[1].confidence));
        assert_eq!(result.port_candidates.last().unwrap().source, "框架默认");
        assert_eq!(
            result.port_candidates.last().unwrap().confidence,
            PortConfidence::Low
        );
        let json = serde_json::to_value(result).unwrap();
        assert_eq!(json["portCandidates"][0]["confidence"], "high");
    }

    #[test]
    fn scan_framework_configs_and_all_literal_ports() {
        let conn = database();
        let f = Fixture::new();
        f.write("next.config.mjs", "// port: 9999\nexport default { /* port: 9998 */ devServer: { port: 3101 }, other: { 'port': '3102' }, report: 9997, url: 'http://localhost' }; // port: 9996");
        f.write(
            "nuxt.config.ts",
            "export default defineNuxtConfig({ devServer: { port: 3103 } })",
        );
        f.write(
            "astro.config.mjs",
            "export default defineConfig({ server: { port: 3104 } })",
        );
        f.write("angular.json", r#"{"projects":{"one":{"architect":{"serve":{"options":{"port":3105}}}},"two":{"targets":{"serve":{"options":{"port":3106}}}}}}"#);
        f.write(
            "start.sh",
            "#!/bin/sh\n# PORT=9995\nPORT=3107 node server.js\n",
        );
        let result = f.inspect(&conn);
        assert_eq!(result.port, Some(3101));
        assert_eq!(
            result
                .port_candidates
                .iter()
                .map(|candidate| candidate.port)
                .collect::<Vec<_>>(),
            (3101..=3107).collect::<Vec<_>>()
        );
        assert_eq!(result.port_candidates[4].source, "angular.json");
        assert_eq!(result.port_candidates[6].source, "start.sh");
    }

    #[test]
    fn scan_env_precedence_tauri_sources_and_framework_defaults() {
        let conn = database();
        let f = Fixture::new();
        f.write(".env", "PORT=5400");
        f.write(".env.local", "VITE_PORT=5401");
        f.write(".env.development", "PORT=5402");
        assert_eq!(f.inspect(&conn).port, Some(5402));
        f.write(
            "src-tauri/tauri.conf.json",
            r#"{"build":{"devUrl":"http://localhost:5400"}}"#,
        );
        let result = f.inspect(&conn);
        assert_eq!(result.port, Some(5400));
        assert_eq!(result.port_source.as_deref(), Some("tauri.conf.json"));
        assert_eq!(
            result.port_candidates[0].source,
            "src-tauri/tauri.conf.json、.env"
        );
        for (framework, port) in [("nuxt", 3000), ("astro", 4321), ("@angular/core", 4200)] {
            let f = Fixture::new();
            f.write(
                "package.json",
                &format!(r#"{{"dependencies":{{"{framework}":"*"}}}}"#),
            );
            let result = f.inspect(&conn);
            assert_eq!(result.port, Some(port));
            assert_eq!(result.port_candidates[0].source, "框架默认");
            assert_eq!(result.port_candidates[0].confidence, PortConfidence::Low);
        }
    }

    #[test]
    fn alternate_script_candidates_preserve_the_main_dev_server_default() {
        let conn = database();
        let f = Fixture::new();
        f.write("package.json", r#"{"scripts":{"dev":"vite","preview":"vite preview --port 4188"},"devDependencies":{"vite":"*"}}"#);
        let result = f.inspect(&conn);
        assert_eq!(result.port, Some(5173));
        assert_eq!(result.port_source.as_deref(), Some("Vite 默认端口"));
        assert_eq!(result.port_candidates[0].port, 4188);
        assert_eq!(result.port_candidates[0].confidence, PortConfidence::Medium);
        assert_eq!(result.port_candidates[1].confidence, PortConfidence::Low);
        f.write("package.json", r#"{"devDependencies":{"vite":"*"}}"#);
        f.write("start.sh", "#!/bin/sh\nvite --port 5403\n");
        let result = f.inspect(&conn);
        assert_eq!(result.port, Some(5403));
        assert_eq!(result.port_source.as_deref(), Some("start.sh"));
    }

    #[test]
    fn port_conflicts_report_project_names_and_exclude_current_project() {
        let conn = database();
        let f = Fixture::new();
        let mut draft = f.draft();
        draft.display_name = Some("端口测试项目".into());
        let project = insert_project(&conn, draft.clone()).unwrap();
        let conflict =
            check_port_conflicts(&conn, project.port.unwrap(), None, &HashSet::new()).unwrap();
        assert_eq!(conflict.project_conflict.as_ref().unwrap().id, project.id);
        assert_eq!(conflict.project_conflict.unwrap().name, "端口测试项目");
        let own = check_port_conflicts(
            &conn,
            project.port.unwrap(),
            Some(&project.id),
            &HashSet::new(),
        )
        .unwrap();
        assert!(own.project_conflict.is_none());
        assert_eq!(
            insert_project(&conn, draft.clone()).unwrap_err(),
            "端口 5199 已被项目「端口测试项目」使用！各项目端口必须唯一，请更换端口。"
        );
        update_project_row(&conn, &project.id, draft.clone()).unwrap();
        draft.port = Some(5200);
        let second = insert_project(&conn, draft.clone()).unwrap();
        draft.port = Some(5199);
        assert!(update_project_row(&conn, &second.id, draft)
            .unwrap_err()
            .contains("端口测试项目"));
        assert!(check_port_conflicts(&conn, 0, None, &HashSet::new()).is_err());
    }

    #[test]
    fn own_listeners_are_excluded_without_hiding_other_processes() {
        let owner = |pid, in_project| processes::PortOwner {
            pid,
            command: format!("node {pid}"),
            cwd: None,
            in_project,
        };
        let result = conflicting_owners(
            vec![owner(21, true), owner(22, false), owner(23, false)],
            &HashSet::from([22]),
        );
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].pid, 23);
        assert!(conflicting_owners(
            vec![owner(21, true), owner(22, false)],
            &HashSet::from([22])
        )
        .is_empty());
    }

    #[test]
    fn conflict_detection_reports_real_listener_and_ignores_project_cwd() {
        let conn = database();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let conflict = check_port_conflicts(&conn, port, None, &HashSet::new()).unwrap();
        let owners = conflict.in_use.unwrap().owners;
        assert!(owners
            .iter()
            .any(|owner| owner.pid == std::process::id() && !owner.command.is_empty()));
        let f = Fixture::new();
        let mut draft = f.draft();
        draft.path = std::env::current_dir()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        draft.port = Some(port.into());
        let project = insert_project(&conn, draft).unwrap();
        assert!(
            check_port_conflicts(&conn, port, Some(&project.id), &HashSet::new())
                .unwrap()
                .in_use
                .is_none()
        );
    }

    #[test]
    fn suggestions_skip_saved_and_listening_ports_and_stop_at_maximum() {
        let used = HashSet::from([5174, 5176]);
        let listening = HashSet::from([5175]);
        assert_eq!(
            suggest_port(5173, &used, |port| listening.contains(&port)),
            Some(5177)
        );
        assert_eq!(suggest_port(65534, &HashSet::new(), |_| false), Some(65535));
        assert_eq!(suggest_port(65535, &HashSet::new(), |_| false), None);
        assert_eq!(suggest_port(65534, &HashSet::new(), |_| true), None);
        assert!(!reserved_port_warnings(80).is_empty());
        assert!(reserved_port_warnings(1024).is_empty());
        if cfg!(target_os = "macos") {
            assert!(reserved_port_warnings(5000)[0].contains("AirPlay"));
            assert!(reserved_port_warnings(7000)[0].contains("AirPlay"));
        }
    }
}
