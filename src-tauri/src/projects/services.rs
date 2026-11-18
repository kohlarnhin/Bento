use super::{
    add_port_candidate, dependency, env_ports, literal_port, probe_project_ports, read_json,
    script_command, script_ports, without_js_comments, EnvVar, PortCandidate, PortConfidence,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Service {
    pub id: String,
    pub name: String,
    pub dir: String,
    pub port: Option<u16>,
    #[serde(default)]
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    pub open_web: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_command: Option<String>,
}

pub(super) fn independent_commands(services: &[Service]) -> Result<bool, String> {
    let count = services.iter().filter(|service| {
        service.start_command.as_deref().is_some_and(|command| !command.trim().is_empty())
    }).count();
    if count > 0 && count != services.len() {
        return Err("请填写所有服务的独立启动命令，或全部留空使用总启动命令".into());
    }
    Ok(count > 0)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ScannedService {
    #[serde(flatten)]
    pub service: Service,
    pub port_candidate: Option<PortCandidate>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct StartScript {
    pub command: String,
    pub source: String,
}

pub fn default_inject_ports() -> bool {
    true
}

fn dir_name(dir: &str) -> &str {
    Path::new(dir)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("root")
}

fn default_service_key(kind: Option<&str>, dir: &str) -> String {
    match kind {
        Some("web") => return "WEB".into(),
        Some("api") => return "API".into(),
        _ => {}
    }
    dir_name(dir)
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect()
}

pub(super) fn port_variable(key: &str) -> String {
    format!("BENTO_PORT_{key}")
}

pub(super) fn fill_service_keys<'a>(services: impl IntoIterator<Item = &'a mut Service>) {
    let mut services: Vec<_> = services.into_iter().collect();
    let mut used: HashSet<String> = services
        .iter()
        .filter(|service| !service.key.is_empty())
        .map(|service| service.key.clone())
        .collect();
    for service in services.iter_mut().filter(|service| service.key.is_empty()) {
        let base = default_service_key(service.kind.as_deref(), &service.dir);
        let mut key = base.clone();
        let mut number = 2;
        while !used.insert(key.clone()) {
            key = format!("{base}_{number}");
            number += 1;
        }
        service.key = key;
    }
}

pub(super) fn normalize_services(mut services: Vec<Service>) -> Result<Vec<Service>, String> {
    if !services.is_empty() && services.iter().filter(|service| service.open_web).count() != 1 {
        return Err("请选择一个服务用于打开网页".into());
    }
    let mut ids = HashSet::new();
    let mut directories = HashSet::new();
    let mut ports = HashSet::new();
    for service in &mut services {
        service.name = service.name.trim().into();
        service.start_command = service.start_command.take().map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        if service.name.is_empty() {
            return Err("请输入服务名称".into());
        }
        if service.id.trim().is_empty() || !ids.insert(service.id.clone()) {
            return Err(format!("服务「{}」的编号无效或重复", service.name));
        }
        let dir = service.dir.trim();
        let path = Path::new(dir);
        if dir.is_empty()
            || path
                .components()
                .any(|part| !matches!(part, Component::CurDir | Component::Normal(_)))
        {
            return Err(format!("服务「{}」需要使用项目内的相对目录", service.name));
        }
        let normalized: PathBuf = path
            .components()
            .filter(|part| *part != Component::CurDir)
            .collect();
        service.dir = if normalized.as_os_str().is_empty() {
            ".".into()
        } else {
            normalized.to_string_lossy().into_owned()
        };
        if !directories.insert(service.dir.clone()) {
            return Err(format!("服务目录「{}」重复", service.dir));
        }
        if let Some(port) = service.port {
            if port == 0 {
                return Err(format!(
                    "服务「{}」的端口必须是 1–65535 之间的整数",
                    service.name
                ));
            }
            if !ports.insert(port) {
                return Err(format!("服务端口 {port} 重复"));
            }
        }
    }
    independent_commands(&services)?;
    fill_service_keys(&mut services);
    let mut keys = HashSet::new();
    for service in &services {
        if !service
            .key
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
        {
            return Err(format!(
                "服务「{}」的角色 key 只能使用大写字母、数字和下划线",
                service.name
            ));
        }
        if !keys.insert(service.key.as_str()) {
            return Err(format!("服务角色 key {} 重复", service.key));
        }
    }
    Ok(services)
}

pub(super) fn launch_environment(explicit: &[EnvVar], port: Option<u16>, services: &[Service], enabled: bool) -> Vec<EnvVar> {
    let mut env = explicit.to_vec();
    if !enabled {
        return env;
    }
    let mut inject = |key: String, value: String| {
        if !explicit.iter().any(|entry| entry.key == key) {
            env.push(EnvVar { key, value });
        }
    };
    if let Some(port) = port {
        inject("PORT".into(), port.to_string());
    }
    if services.len() < 2 {
        return env;
    }
    let mut ports = Vec::new();
    for service in services {
        if let Some(port) = service.port {
            inject(port_variable(&service.key), port.to_string());
            ports.push(format!("{}:{port}", service.key));
        }
    }
    if !ports.is_empty() {
        inject("BENTO_PORTS".into(), ports.join(","));
    }
    env
}

const FRONTEND: &[&str] = &["frontend", "front", "web", "client", "ui", "admin"];
const BACKEND: &[&str] = &["backend", "back", "server", "api", "service"];
const SERVICE_MANIFESTS: &[&str] = &[
    "package.json",
    "pyproject.toml",
    "requirements.txt",
    "go.mod",
    "Cargo.toml",
    "pom.xml",
    "build.gradle",
    "build.gradle.kts",
];
const SKIPPED: &[&str] = &[
    "node_modules",
    ".git",
    "dist",
    "build",
    "target",
    ".venv",
    "vendor",
];
const SHELL_SCRIPTS: &[&str] = &["dev.sh", "start.sh", "run.sh", "scripts/dev.sh"];

fn shell_scripts(root: &Path) -> Vec<String> {
    let mut generated: Vec<_> = std::fs::read_dir(root.join("scripts"))
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let stem = name.strip_prefix("bento-dev")?.strip_suffix(".sh")?;
            let number = if stem.is_empty() {
                1
            } else {
                stem.strip_prefix('-')?.parse::<usize>().ok()?
            };
            entry
                .file_type()
                .ok()?
                .is_file()
                .then_some((number, format!("scripts/{name}")))
        })
        .collect();
    generated.sort_unstable();
    generated
        .into_iter()
        .map(|(_, file)| file)
        .chain(SHELL_SCRIPTS.iter().map(|name| (*name).into()))
        .collect()
}

fn project_role(name: &str) -> Option<&'static str> {
    let name = name.to_ascii_lowercase();
    let token = name.rsplit(['-', '_', '.', ' ']).next().unwrap_or_default();
    if FRONTEND.contains(&token) {
        Some("web")
    } else if BACKEND.contains(&token) {
        Some("api")
    } else {
        None
    }
}

fn directories(path: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(path)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry.file_type().is_ok_and(|file| file.is_dir())
                && !SKIPPED.contains(&entry.file_name().to_string_lossy().as_ref())
        })
        .map(|entry| entry.path())
        .collect()
}

fn start_scripts(root: &Path, package: &Value, pm: Option<&str>) -> Vec<StartScript> {
    let mut scripts: Vec<_> = shell_scripts(root)
        .into_iter()
        .filter(|file| root.join(file).is_file())
        .map(|file| StartScript {
            command: format!("./{file}"),
            source: file,
        })
        .collect();
    if let Ok(makefile) = std::fs::read_to_string(root.join("Makefile")) {
        for target in ["dev", "start"] {
            if makefile.lines().any(|line| {
                !line.starts_with('\t')
                    && line
                        .split('#')
                        .next()
                        .and_then(|line| line.split_once(':'))
                        .is_some_and(|(targets, rest)| {
                            !rest.starts_with('=')
                                && targets.split_whitespace().any(|name| name == target)
                        })
            }) {
                scripts.push(StartScript {
                    command: format!("make {target}"),
                    source: format!("Makefile · {target}"),
                });
            }
        }
    }
    for file in ["Procfile.dev", "Procfile"] {
        if root.join(file).is_file() {
            scripts.push(StartScript {
                command: format!("foreman start -f {file}"),
                source: file.into(),
            });
        }
    }
    if let Some(pm) = pm {
        let mut package_scripts: Vec<_> = ["dev", "start"]
            .into_iter()
            .filter_map(|name| {
                let body = package["scripts"][name]
                    .as_str()
                    .filter(|value| !value.trim().is_empty())?;
                let orchestrates = ["concurrently", "turbo", "npm-run-all", "run-p", "run-s"]
                    .iter()
                    .any(|tool| body.contains(tool));
                Some((!orchestrates, name))
            })
            .collect();
        package_scripts.sort_by_key(|(standalone, _)| *standalone);
        for (_, name) in package_scripts {
            scripts.push(StartScript {
                command: script_command(pm, name),
                source: format!("package.json · {name}"),
            });
        }
    }
    scripts
}

// Static defaults such as Number(env.WEB_PORT || 3100), env.get("API_PORT", "8100"),
// and the shell's ${API_PORT:-8100}. Never evaluate a project's configuration.
fn env_port_defaults(text: &str) -> Vec<(&str, u16)> {
    let mut defaults = Vec::new();
    for line in text
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with('#'))
    {
        let mut offset = 0;
        for key in line
            .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
            .filter(|key| !key.is_empty())
        {
            let start = offset + line[offset..].find(key).unwrap();
            offset = start + key.len();
            if key != "PORT" && !key.ends_with("_PORT") && !key.starts_with("BENTO_PORT_") {
                continue;
            }
            let before = &line[..start];
            let after = line[offset..].trim_start();
            let fallback = if before.ends_with("${") {
                after.strip_prefix(":-")
            } else if before.ends_with('.') {
                after
                    .strip_prefix("||")
                    .or_else(|| after.strip_prefix("??"))
            } else if before.ends_with(".get(\"") || before.ends_with(".get('") {
                after
                    .trim_start_matches(['\'', '"'])
                    .trim_start()
                    .strip_prefix(',')
            } else {
                None
            };
            if let Some(port) = fallback.and_then(|value| leading_port(value.trim_start())) {
                defaults.push((key, port));
            }
        }
    }
    defaults
}

fn leading_port(value: &str) -> Option<u16> {
    let value = value.trim_start_matches(['\'', '"']);
    let end = value
        .find(|ch: char| !ch.is_ascii_digit())
        .unwrap_or(value.len());
    if value[end..]
        .starts_with(|ch: char| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '+' | '-'))
    {
        return None;
    }
    literal_port(&value[..end])
}

fn spring_port(value: &str) -> Option<u16> {
    literal_port(value).or_else(|| {
        let (_, fallback) = value
            .trim()
            .strip_prefix("${")?
            .strip_suffix('}')?
            .rsplit_once(':')?;
        literal_port(fallback)
    })
}

fn backend_ports(path: &Path, candidates: &mut Vec<PortCandidate>) -> bool {
    let mut detected_backend = false;
    for file in [
        "pyproject.toml",
        "requirements.txt",
        "main.py",
        "app.py",
        "app/main.py",
        "src/main.py",
        "wsgi.py",
        "manage.py",
        ".flaskenv",
        "go.mod",
        "main.go",
        "server.go",
        "cmd/server/main.go",
        "cmd/api/main.go",
        "pom.xml",
        "build.gradle",
        "build.gradle.kts",
    ] {
        let Ok(text) = std::fs::read_to_string(path.join(file)) else {
            continue;
        };
        let lower = text.to_ascii_lowercase();
        if file == ".flaskenv" {
            for port in env_ports(&text) {
                detected_backend = true;
                add_port_candidate(candidates, port, file, PortConfidence::High);
            }
        }
        let frameworks = [
            (
                lower.contains("uvicorn") || lower.contains("fastapi"),
                8000,
                "Uvicorn 默认端口",
            ),
            (lower.contains("flask"), 5000, "Flask 默认端口"),
            (lower.contains("gin-gonic/gin"), 8080, "Gin 默认端口"),
            (lower.contains("spring-boot"), 8080, "Spring Boot 默认端口"),
        ];
        for (matched, port, source) in frameworks {
            if matched {
                detected_backend = true;
                add_port_candidate(candidates, port, source, PortConfidence::Low);
            }
        }
        for line in text
            .lines()
            .map(str::trim)
            .filter(|line| !line.starts_with('#') && !line.starts_with("//"))
        {
            let port = if file.ends_with(".go") {
                line.split_once(".Run(").and_then(|(_, args)| {
                    let address = args
                        .trim()
                        .strip_prefix(['"', '\x60'])?
                        .split(['"', '\x60'])
                        .next()?;
                    literal_port(address.rsplit_once(':')?.1)
                })
            } else {
                None
            };
            for port in port.into_iter().chain(script_ports(line)).chain(
                (file.ends_with(".py") || file == "pyproject.toml")
                    .then(|| {
                        line.match_indices("port").find_map(|(index, _)| {
                            if line[..index].ends_with(|ch: char| ch.is_alphanumeric() || ch == '_')
                            {
                                return None;
                            }
                            leading_port(
                                line[index + 4..]
                                    .trim_start()
                                    .strip_prefix('=')?
                                    .trim_start(),
                            )
                        })
                    })
                    .flatten(),
            ) {
                detected_backend = true;
                add_port_candidate(candidates, port, file, PortConfidence::High);
            }
        }
    }
    for dir in ["src/main/resources", "."] {
        for profile in ["application-dev", "application-local", "application"] {
            for extension in ["properties", "yml", "yaml"] {
                let file = if dir == "." {
                    format!("{profile}.{extension}")
                } else {
                    format!("{dir}/{profile}.{extension}")
                };
                let Ok(config) = std::fs::read_to_string(path.join(&file)) else {
                    continue;
                };
                let mut server_indent = None;
                for line in config.lines() {
                    let line = line.split('#').next().unwrap_or_default().trim_end();
                    let trimmed = line.trim_start();
                    if trimmed.is_empty() {
                        continue;
                    }
                    let indent = line.len() - trimmed.len();
                    let port = if extension == "properties" {
                        trimmed
                            .split_once(['=', ':'])
                            .filter(|(key, _)| key.trim() == "server.port")
                            .and_then(|(_, value)| spring_port(value.trim()))
                    } else if trimmed == "server:" {
                        server_indent = Some(indent);
                        None
                    } else if server_indent.is_some_and(|parent| indent > parent) {
                        trimmed
                            .strip_prefix("port:")
                            .and_then(|value| spring_port(value.trim()))
                    } else {
                        server_indent = None;
                        None
                    };
                    if let Some(port) = port {
                        detected_backend = true;
                        add_port_candidate(candidates, port, &file, PortConfidence::High);
                    }
                }
            }
        }
    }
    detected_backend
}

fn scan_service(
    root: &Path,
    path: &Path,
    launcher_ports: &[(String, String, u16)],
) -> Option<ScannedService> {
    let name = path.file_name()?.to_str()?;
    let role = project_role(name);
    if role.is_none()
        && !SERVICE_MANIFESTS
            .iter()
            .any(|file| path.join(file).is_file())
    {
        return None;
    }
    let dir = path.strip_prefix(root).ok()?.to_str()?.to_owned();
    let package = read_json(&path.join("package.json")).unwrap_or(Value::Null);
    let (mut candidates, recommended) =
        probe_project_ports(path, &package, None, path.join("start.sh").is_file());
    let mut browser = ["vite", "next", "nuxt", "astro", "@angular/core"]
        .iter()
        .any(|name| dependency(&package, name))
        || ["dev", "start"]
            .into_iter()
            .filter_map(|script| package["scripts"][script].as_str())
            .any(|script| {
                script
                    .split_whitespace()
                    .any(|word| matches!(word, "vite" | "next" | "nuxt" | "astro"))
            });
    for prefix in ["vite", "next", "nuxt", "astro"] {
        for extension in ["ts", "mts", "js", "mjs", "cjs", "cts"] {
            let file = format!("{prefix}.config.{extension}");
            if let Ok(config) = std::fs::read_to_string(path.join(&file)) {
                browser = true;
                for (_, port) in env_port_defaults(&without_js_comments(&config)) {
                    add_port_candidate(&mut candidates, port, &file, PortConfidence::Medium);
                }
            }
        }
    }
    let api = backend_ports(path, &mut candidates);
    let kind = if browser {
        Some("web")
    } else {
        role.or(api.then_some("api"))
    };
    for (file, key, port) in launcher_ports {
        let service_key = default_service_key(kind, &dir);
        let tokens: Vec<_> = key
            .trim_end_matches("_PORT")
            .to_ascii_lowercase()
            .split('_')
            .map(str::to_owned)
            .collect();
        let aliases = match kind {
            Some("web") => FRONTEND,
            Some("api") => BACKEND,
            _ => &[],
        };
        if key == &port_variable(&service_key)
            || key.trim_end_matches("_PORT").ends_with(&service_key)
            || tokens.iter().any(|token| aliases.contains(&token.as_str()))
        {
            add_port_candidate(
                &mut candidates,
                *port,
                &format!("{file} · {key}"),
                PortConfidence::Medium,
            );
        }
    }
    let port_candidate = recommended
        .filter(|port| port.confidence == PortConfidence::High)
        .or_else(|| candidates.into_iter().min_by_key(|port| port.confidence));
    Some(ScannedService {
        service: Service {
            id: format!("service:{dir}"),
            name: match name.to_ascii_lowercase().as_str() {
                "frontend" | "web" | "client" | "ui" => "前端",
                "backend" | "server" | "api" => "后端",
                _ => name,
            }
            .into(),
            port: port_candidate.as_ref().map(|candidate| candidate.port),
            key: String::new(),
            dir,
            kind: kind.map(str::to_owned),
            open_web: false,
            start_command: None,
        },
        port_candidate,
    })
}

pub(super) fn scan_services(
    root: &Path,
    package: &Value,
    pm: Option<&str>,
) -> (Vec<ScannedService>, Vec<StartScript>) {
    let top = directories(root);
    let mut candidates = Vec::new();
    for path in top {
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| matches!(name, "apps" | "packages" | "services"))
        {
            candidates.extend(directories(&path));
        }
        candidates.push(path);
    }
    candidates.sort();
    let launcher_ports: Vec<_> = shell_scripts(root)
        .into_iter()
        .filter_map(|file| {
            std::fs::read_to_string(root.join(&file))
                .ok()
                .map(|script| (file, script))
        })
        .flat_map(|(file, script)| {
            env_port_defaults(&script)
                .into_iter()
                .map(|(key, port)| (file.to_string(), key.to_string(), port))
                .collect::<Vec<_>>()
        })
        .collect();
    let mut services: Vec<_> = candidates
        .iter()
        .filter_map(|path| scan_service(root, path, &launcher_ports))
        .collect();
    services.sort_by_key(|row| row.service.kind.as_deref() != Some("web"));
    let primary = services
        .iter()
        .position(|row| row.service.kind.as_deref() == Some("web") && row.service.port.is_some())
        .or_else(|| {
            services
                .iter()
                .position(|row| row.service.kind.as_deref() == Some("web"))
        })
        .or_else(|| services.iter().position(|row| row.service.port.is_some()))
        .unwrap_or(0);
    if let Some(row) = services.get_mut(primary) {
        row.service.open_web = true;
    }
    fill_service_keys(services.iter_mut().map(|row| &mut row.service));
    (services, start_scripts(root, package, pm))
}
