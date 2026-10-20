use crate::{check_port_in_use_sync, expand_tilde, ProjectConfig};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

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
            "TERM" | "COLORTERM" | "SHELL" | "HOME" | "USER" | "PWD"
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

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDraft {
    pub name: String,
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub path: String,
    pub port: Option<i64>,
    pub start_command: String,
    #[serde(rename = "type")]
    pub project_type: Option<String>,
    #[serde(default)]
    pub env: Vec<EnvVar>,
    #[serde(default)]
    pub commands: Vec<ExtraCommand>,
    #[serde(default)]
    pub auto_open: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExistingProject {
    id: String,
    name: String,
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
    existing_project: Option<ExistingProject>,
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
        if let Ok(rest) = path.strip_prefix(home) {
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
    let start_command = draft.start_command.trim().to_string();
    if start_command.is_empty() {
        return Err("请输入启动命令".into());
    }
    let project_type = draft.project_type.as_deref().unwrap_or("web").trim();
    if !matches!(project_type, "web" | "app") {
        return Err("项目类型必须为 Web 项目或桌面应用".into());
    }
    let port = match draft.port {
        Some(port) if (1..=65535).contains(&port) => Some(port as u16),
        Some(_) => return Err("端口必须是 1–65535 之间的整数".into()),
        None if project_type == "web" => return Err("Web 项目需要填写端口".into()),
        None => None,
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
    if let Some(port) = port {
        let existing: Option<String> = conn.query_row(
            "SELECT COALESCE(display_name, name) FROM projects WHERE port = ?1 AND (?2 IS NULL OR id != ?2)",
            params![port, id], |row| row.get(0),
        ).optional().map_err(|e| format!("检查端口失败：{e}"))?;
        if let Some(existing) = existing {
            return Err(format!(
                "端口 {} 已被项目「{}」使用！各项目端口必须唯一，请更换端口。",
                port, existing
            ));
        }
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
    if draft.auto_open && (project_type != "web" || port.is_none()) {
        return Err("仅填写端口的 Web 项目可在启动就绪后自动打开网页".into());
    }
    Ok(ProjectConfig {
        id: id.unwrap_or_default().into(),
        name,
        display_name: optional_text(draft.display_name),
        description: optional_text(draft.description),
        path: stored_path(&path),
        port,
        start_command,
        project_type: Some(project_type.into()),
        env: draft.env,
        commands,
        auto_open: draft.auto_open,
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
    let mut project = normalize_project(conn, draft, None)?;
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
    while conn
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
    conn.execute(
        "INSERT INTO projects (id, name, display_name, description, path, port, start_command, type, sort_order, env, commands, auto_open) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, (SELECT COALESCE(MAX(sort_order), -1) + 1 FROM projects), ?9, ?10, ?11)",
        params![project.id, project.name, project.display_name, project.description, project.path, project.port, project.start_command, project.project_type, serde_json::to_string(&project.env).map_err(|err| err.to_string())?, serde_json::to_string(&project.commands).map_err(|err| err.to_string())?, project.auto_open],
    ).map_err(|e| format!("新增项目失败：{e}"))?;
    Ok(project)
}

pub fn reorder_project_rows(conn: &mut Connection, ids: &[String]) -> Result<(), String> {
    let transaction = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|error| format!("保存项目排序失败：{error}"))?;
    let current = {
        let mut statement = transaction
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
        transaction
            .execute(
                "UPDATE projects SET sort_order = ?1 WHERE id = ?2",
                params![position as i64, id],
            )
            .map_err(|error| format!("保存项目排序失败：{error}"))?;
    }
    transaction
        .commit()
        .map_err(|error| format!("保存项目排序失败：{error}"))
}

pub fn update_project_row(
    conn: &Connection,
    id: &str,
    draft: ProjectDraft,
) -> Result<ProjectConfig, String> {
    let exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM projects WHERE id = ?1)",
            [id],
            |row| row.get(0),
        )
        .map_err(|e| format!("查询项目失败：{e}"))?;
    if !exists {
        return Err("未找到项目".into());
    }
    let project = normalize_project(conn, draft, Some(id))?;
    conn.execute(
        "UPDATE projects SET name=?1, display_name=?2, description=?3, path=?4, port=?5, start_command=?6, type=?7, env=?9, commands=?10, auto_open=?11, updated_at=CURRENT_TIMESTAMP WHERE id=?8",
        params![project.name, project.display_name, project.description, project.path, project.port, project.start_command, project.project_type, id, serde_json::to_string(&project.env).map_err(|err| err.to_string())?, serde_json::to_string(&project.commands).map_err(|err| err.to_string())?, project.auto_open],
    ).map_err(|e| format!("更新项目失败：{e}"))?;
    Ok(project)
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
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

fn script_port(script: &str) -> Option<u16> {
    let mut words = script.split_whitespace();
    while let Some(word) = words.next() {
        let value = if matches!(word, "--port" | "-p") {
            words.next()
        } else {
            word.strip_prefix("--port=")
        };
        if let Some(port) = value
            .and_then(|s| s.trim_matches(['\'', '"', ';']).parse::<u16>().ok())
            .filter(|p| *p > 0)
        {
            return Some(port);
        }
    }
    None
}

fn vite_port(config: &str) -> Option<u16> {
    // Read a literal `port: NNNN` without evaluating JavaScript or walking the project.
    for line in config
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
    {
        for (index, _) in line.match_indices("port") {
            if index > 0
                && line[..index]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_alphanumeric() || c == '_')
            {
                continue;
            }
            let Some(rest) = line[index + 4..].trim_start().strip_prefix(':') else {
                continue;
            };
            let digits: String = rest
                .trim_start()
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            if let Some(port) = digits.parse::<u16>().ok().filter(|p| *p > 0) {
                return Some(port);
            }
        }
    }
    None
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
        existing_project: None,
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
    let tauri_path = path.join("src-tauri/tauri.conf.json");
    if tauri_path.is_file()
        || dependency(&package, "electron")
        || dependency(&package, "@tauri-apps/cli")
    {
        result.project_type = "app".into();
    }
    result.package_manager = package["packageManager"]
        .as_str()
        .and_then(|s| s.split('@').next())
        .filter(|s| matches!(*s, "pnpm" | "yarn" | "bun" | "npm"))
        .map(String::from)
        .or_else(|| {
            [
                ("pnpm-lock.yaml", "pnpm"),
                ("yarn.lock", "yarn"),
                ("bun.lockb", "bun"),
                ("bun.lock", "bun"),
                ("package-lock.json", "npm"),
            ]
            .iter()
            .find(|(file, _)| path.join(file).is_file())
            .map(|(_, pm)| pm.to_string())
        })
        .or_else(|| package_path.is_file().then(|| "npm".into()));
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
    let tauri_port = read_json(&tauri_path).and_then(|config| {
        tauri::Url::parse(config["build"]["devUrl"].as_str()?)
            .ok()?
            .port()
    });
    let mut detected = tauri_port.map(|p| (p, "tauri.conf.json".to_string()));
    if detected.is_none() {
        for file in [
            "vite.config.ts",
            "vite.config.mts",
            "vite.config.js",
            "vite.config.mjs",
        ] {
            if let Some(port) = std::fs::read_to_string(path.join(file))
                .ok()
                .and_then(|s| vite_port(&s))
            {
                detected = Some((port, file.into()));
                break;
            }
        }
    }
    if detected.is_none() {
        // Tauri's command launches the frontend dev script; its port belongs to that server.
        let dev_script = package["scripts"]["dev"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| {
                package["scripts"]["start"]
                    .as_str()
                    .filter(|s| !s.trim().is_empty())
            })
            .or(chosen_script);
        detected = dev_script
            .and_then(script_port)
            .map(|p| (p, "package.json".into()));
    }
    if detected.is_none() {
        detected = if dependency(&package, "vite") {
            Some((5173, "Vite 默认端口".into()))
        } else if dependency(&package, "next") {
            Some((3000, "Next.js 默认端口".into()))
        } else {
            None
        };
    }
    let mut stmt = conn.prepare("SELECT id, COALESCE(display_name, name), path, port FROM projects WHERE ?1 IS NULL OR id != ?1")
        .map_err(|e| format!("查询项目失败：{e}"))?;
    let rows = stmt
        .query_map([exclude_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<u16>>(3)?,
            ))
        })
        .map_err(|e| format!("读取项目失败：{e}"))?;
    let mut used_ports = HashSet::new();
    for row in rows {
        let (id, name, other_path, port) = row.map_err(|e| format!("读取项目失败：{e}"))?;
        if let Some(port) = port {
            used_ports.insert(port);
        }
        if result.existing_project.is_none() && expanded_path(&other_path) == path {
            result.existing_project = Some(ExistingProject { id, name });
        }
    }
    if detected.is_none() && result.project_type == "web" {
        for port in 5173..5373 {
            if !used_ports.contains(&port) && !check_port_in_use_sync(port).0 {
                detected = Some((port, "推荐空闲端口".into()));
                break;
            }
        }
    }
    if let Some((port, source)) = detected {
        result.port = Some(port);
        result.port_source = Some(source);
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
                port: Some(5199),
                start_command: "  pnpm dev  ".into(),
                project_type: None,
                env: Vec::new(),
                commands: Vec::new(),
                auto_open: false,
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
        assert!(draft.env.is_empty() && draft.commands.is_empty() && !draft.auto_open);
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
        for port in [None, Some(0), Some(-1), Some(65536)] {
            let mut draft = f.draft();
            draft.port = port;
            assert!(insert_project(&conn, draft)
                .unwrap_err()
                .contains(if port.is_none() {
                    "需要填写端口"
                } else {
                    "1–65535"
                }));
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
}
