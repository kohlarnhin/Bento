use super::services::{normalize_services, port_variable, Service};
use super::{dependency, expanded_path, package_manager, read_json, without_js_comments};
use crate::processes::quote_shell;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

const MAX_SOURCE_BYTES: u64 = 512 * 1024;
const MAX_CALLS: usize = 32;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServicePortCheck {
    pub service_id: String,
    pub variable: String,
    pub references: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortStandardCheck {
    pub checked_files: Vec<String>,
    pub services: Vec<ServicePortCheck>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StandardScript {
    pub relative_path: String,
    pub script: String,
    pub preserved: Vec<String>,
    pub notes: Vec<String>,
}

fn project_root(path: &str) -> Result<PathBuf, String> {
    let root = expanded_path(path);
    if path.trim().is_empty() || !root.is_dir() {
        return Err("请选择有效的项目目录".into());
    }
    root.canonicalize()
        .map_err(|err| format!("读取项目目录失败：{err}"))
}

fn local_file(root: &Path, path: &Path) -> Option<PathBuf> {
    let canonical = path.canonicalize().ok()?;
    if !canonical.starts_with(root) {
        return None;
    }
    let metadata = canonical.metadata().ok()?;
    (metadata.is_file() && metadata.len() <= MAX_SOURCE_BYTES).then_some(canonical)
}

fn label(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into()
}

fn read_source(root: &Path, path: &Path) -> Option<String> {
    fs::read_to_string(local_file(root, path)?).ok()
}

/// Extract shell words and simple command boundaries for static inspection.
fn shell_commands(text: &str) -> Vec<Vec<String>> {
    let mut result = Vec::new();
    let mut group = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if let Some(delimiter) = quote {
            if ch == delimiter {
                quote = None;
            } else if ch == '\\'
                && delimiter == '"'
                && matches!(chars.peek(), Some('"' | '\\' | '$'))
            {
                word.push(chars.next().unwrap());
            } else {
                word.push(ch);
            }
            continue;
        }
        match ch {
            '\'' | '"' => quote = Some(ch),
            '\\' => {
                if let Some(ch) = chars.next().filter(|ch| *ch != '\n') {
                    word.push(ch);
                }
            }
            '#' if word.is_empty() => {
                while chars.peek().is_some_and(|ch| *ch != '\n') {
                    chars.next();
                }
            }
            '\n' | ';' | '|' | '&' | '(' | ')' => {
                if !word.is_empty() {
                    group.push(std::mem::take(&mut word));
                }
                if !group.is_empty() {
                    result.push(std::mem::take(&mut group));
                }
            }
            ch if ch.is_whitespace() => {
                if !word.is_empty() {
                    group.push(std::mem::take(&mut word));
                }
            }
            _ => word.push(ch),
        }
    }
    if !word.is_empty() {
        group.push(word);
    }
    if !group.is_empty() {
        result.push(group);
    }
    result
}

fn shell_command(words: &[String]) -> &[String] {
    let skip = words
        .iter()
        .take_while(|word| {
            matches!(
                word.as_str(),
                "exec" | "command" | "env" | "if" | "then" | "do" | "else" | "time" | "!"
            ) || word.split_once('=').is_some_and(|(key, _)| {
                !key.is_empty()
                    && key
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            })
        })
        .count();
    &words[skip..]
}

fn script_filename(word: &str) -> bool {
    Path::new(word)
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            matches!(
                ext,
                "sh" | "bash" | "zsh" | "py" | "js" | "mjs" | "cjs" | "ts" | "mts" | "cts"
            )
        })
}

fn local_path(root: &Path, cwd: &Path, caller: &Path, word: &str) -> Vec<PathBuf> {
    if let Some(rest) = word
        .split_once(")/")
        .filter(|(prefix, _)| prefix.contains("dirname"))
        .map(|(_, rest)| rest)
    {
        return vec![caller.join(rest)];
    }
    if word.starts_with('$') {
        return word
            .split_once('/')
            .map(|(_, rest)| vec![root.join(rest), caller.join(rest)])
            .unwrap_or_default();
    }
    vec![cwd.join(word), caller.join(word), root.join(word)]
}

struct LaunchSource {
    name: String,
    cwd: PathBuf,
    caller: PathBuf,
    text: String,
    shell: bool,
}

fn package_script(root: &Path, cwd: &Path, words: &[String]) -> Option<LaunchSource> {
    let (program, args) = words.split_first()?;
    if !matches!(program.as_str(), "npm" | "pnpm" | "yarn" | "bun") {
        return None;
    }
    let (directory, args) = match args {
        [flag, dir, rest @ ..]
            if matches!(flag.as_str(), "-C" | "--dir" | "--cwd" | "--prefix") =>
        {
            (cwd.join(dir).canonicalize().ok()?, rest)
        }
        args => (cwd.to_owned(), args),
    };
    let args = match args {
        [word, rest @ ..] if word == "run" => rest,
        args => args,
    };
    let (name, _) = args.split_first()?;
    if name.starts_with('-')
        || matches!(
            name.as_str(),
            "exec" | "install" | "add" | "dlx" | "test" | "build"
        )
    {
        return None;
    }
    let manifest = directory.join("package.json");
    let package: Value = serde_json::from_str(&read_source(root, &manifest)?).ok()?;
    let body = package["scripts"][name.as_str()].as_str()?.to_owned();
    Some(LaunchSource {
        name: format!("{} · {name}", label(root, &manifest)),
        cwd: directory.clone(),
        caller: directory,
        text: body,
        shell: true,
    })
}

fn task_script(root: &Path, cwd: &Path, words: &[String]) -> Option<LaunchSource> {
    let (program, args) = words.split_first()?;
    let (name, recipes) = match program.as_str() {
        "make" => {
            let target = args.first()?;
            let path = cwd.join("Makefile");
            let text = read_source(root, &path)?;
            let mut in_target = false;
            let recipes = text
                .lines()
                .filter_map(|line| {
                    if let Some(recipe) = line.strip_prefix('\t') {
                        return in_target.then(|| recipe.trim_start_matches(['@', '-', '+']));
                    }
                    let line = line.split('#').next().unwrap_or_default();
                    if !line.trim().is_empty() {
                        in_target = line.split_once(':').is_some_and(|(targets, rest)| {
                            !rest.starts_with('=')
                                && targets.split_whitespace().any(|word| word == target)
                        });
                    }
                    None
                })
                .collect::<Vec<_>>()
                .join("\n");
            (format!("{} · {target}", label(root, &path)), recipes)
        }
        "foreman" if args.first().is_some_and(|arg| arg == "start") => {
            let path = args
                .windows(2)
                .find(|pair| pair[0] == "-f")
                .map_or("Procfile", |pair| &pair[1]);
            let path = cwd.join(path);
            let text = read_source(root, &path)?;
            let recipes = text
                .lines()
                .filter_map(|line| {
                    let (name, command) = line.split_once(':')?;
                    let name = name.trim();
                    (!name.is_empty()
                        && name
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte)))
                    .then_some(command.trim())
                })
                .collect::<Vec<_>>()
                .join("\n");
            (label(root, &path), recipes)
        }
        _ => return None,
    };
    (!recipes.is_empty()).then(|| LaunchSource {
        name,
        cwd: cwd.to_owned(),
        caller: cwd.to_owned(),
        text: recipes,
        shell: true,
    })
}

fn collect_calls(
    root: &Path,
    source: &LaunchSource,
    files: &mut Vec<LaunchSource>,
    tasks: &mut Vec<LaunchSource>,
) {
    let mut cwd = source.cwd.clone();
    for words in shell_commands(&source.text) {
        let words = shell_command(&words);
        let Some((program, args)) = words.split_first() else {
            continue;
        };
        if program == "cd" {
            if let Some(next) = args
                .iter()
                .find(|word| !word.starts_with('-'))
                .and_then(|word| {
                    local_path(root, &cwd, &source.caller, word)
                        .into_iter()
                        .filter_map(|path| path.canonicalize().ok())
                        .find(|path| path.is_dir() && path.starts_with(root))
                })
            {
                cwd = next;
            }
            continue;
        }
        if tasks.len() < MAX_CALLS {
            if let Some(script) =
                package_script(root, &cwd, words).or_else(|| task_script(root, &cwd, words))
            {
                if !tasks.iter().any(|current| current.name == script.name) {
                    tasks.push(script);
                }
            }
        }
        let command_name = Path::new(program)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        let script = if matches!(
            command_name,
            "bash" | "sh" | "zsh" | "python" | "python3" | "node" | "source" | "."
        ) {
            args.iter()
                .find(|word| !word.starts_with('-'))
                .filter(|word| script_filename(word))
        } else {
            script_filename(program).then_some(program)
        };
        if let Some(path) = script.and_then(|word| {
            local_path(root, &cwd, &source.caller, word)
                .into_iter()
                .find_map(|path| local_file(root, &path))
        }) {
            let name = label(root, &path);
            if files.len() < MAX_CALLS
                && !files
                    .iter()
                    .any(|file| file.name == name && file.cwd == cwd)
            {
                if let Ok(text) = fs::read_to_string(&path) {
                    let shell = shell_source(&path);
                    files.push(LaunchSource {
                        name,
                        cwd: cwd.clone(),
                        caller: path.parent().unwrap_or(root).to_owned(),
                        text: if shell {
                            text
                        } else {
                            without_js_comments(&text)
                        },
                        shell,
                    });
                }
            }
        }
    }
}

fn uncomment_shell_line(line: &str) -> &str {
    let mut quote = None;
    let mut escaped = false;
    let mut boundary = true;
    for (position, ch) in line.char_indices() {
        if escaped {
            escaped = false;
        } else if ch == '\\' && quote != Some('\'') {
            escaped = true;
        } else if let Some(delimiter) = quote {
            if ch == delimiter {
                quote = None;
            }
        } else if matches!(ch, '\'' | '"') {
            quote = Some(ch);
        } else if ch == '#' && boundary {
            return &line[..position];
        }
        boundary = ch.is_whitespace() || matches!(ch, ';' | '(' | ')' | '&' | '|');
    }
    line
}

fn shell_expands(prefix: &str) -> bool {
    let mut quote = None;
    let mut escaped = false;
    for ch in prefix.chars() {
        if escaped {
            escaped = false;
        } else if ch == '\\' && quote != Some('\'') {
            escaped = true;
        } else if let Some(delimiter) = quote {
            if ch == delimiter {
                quote = None;
            }
        } else if matches!(ch, '\'' | '"') {
            quote = Some(ch);
        }
    }
    quote != Some('\'') && !escaped
}

fn identifier(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

fn reads_variable(source: &str, variable: &str, shell: bool) -> bool {
    source
        .lines()
        .map(|line| {
            if shell {
                uncomment_shell_line(line)
            } else {
                line
            }
        })
        .any(|line| {
            line.match_indices(variable).any(|(start, _)| {
                let before = &line[..start];
                let after = &line[start + variable.len()..];
                if after.starts_with(identifier) {
                    return false;
                }
                for prefix in ["${", "$"] {
                    if shell && before.ends_with(prefix) {
                        return shell_expands(&before[..before.len() - prefix.len()]);
                    }
                }
                let before = before.trim_end_matches(['\'', '"']).trim_end();
                [
                    "env.",
                    "env[",
                    "environ[",
                    "environ.get(",
                    "getenv(",
                    "printenv ",
                ]
                .iter()
                .any(|suffix| before.ends_with(suffix))
                    || (!shell && before.contains('{') && after.contains("} = process.env"))
            })
        })
}

fn shell_source(path: &Path) -> bool {
    !matches!(
        path.extension().and_then(|ext| ext.to_str()),
        Some("js" | "mjs" | "cjs" | "ts" | "mts" | "cts")
    )
}

pub(super) fn audit(root: &Path, command: &str, services: &[Service]) -> PortStandardCheck {
    let entry = LaunchSource {
        name: "启动命令".into(),
        cwd: root.to_owned(),
        caller: root.to_owned(),
        text: command.into(),
        shell: true,
    };
    let mut entry_files = Vec::new();
    let mut entry_tasks = Vec::new();
    collect_calls(root, &entry, &mut entry_files, &mut entry_tasks);
    let mut calls = Vec::new();
    let mut tasks = Vec::new();
    for source in entry_files
        .iter()
        .chain(&entry_tasks)
        .filter(|source| source.shell)
    {
        collect_calls(root, source, &mut calls, &mut tasks);
    }
    // A launch wrapper can invoke a package script or task that directly names a file.
    for task in &tasks {
        collect_calls(root, task, &mut calls, &mut Vec::new());
    }
    let shared: Vec<_> = std::iter::once(entry)
        .chain(entry_files)
        .chain(calls)
        .chain(entry_tasks)
        .chain(tasks)
        .collect();
    let mut checked_files: HashSet<_> = shared.iter().map(|source| source.name.clone()).collect();
    let results = services
        .iter()
        .filter(|service| service.port.is_some())
        .map(|service| {
            let variable = if services.len() < 2 {
                "PORT".into()
            } else {
                port_variable(&service.key)
            };
            let references = |text: &str, shell| {
                reads_variable(text, &variable, shell)
                    || (service.open_web
                        && variable != "PORT"
                        && reads_variable(text, "PORT", shell))
            };
            let mut sources: HashSet<_> = shared
                .iter()
                .filter(|source| references(&source.text, source.shell))
                .map(|source| source.name.clone())
                .collect();
            for framework in ["vite", "next"] {
                for extension in ["ts", "mts", "js", "mjs", "cjs", "cts"] {
                    let config = root
                        .join(&service.dir)
                        .join(format!("{framework}.config.{extension}"));
                    if let Some(body) = read_source(root, &config) {
                        let filename = label(root, &config);
                        checked_files.insert(filename.clone());
                        if references(&without_js_comments(&body), false) {
                            sources.insert(filename);
                        }
                    }
                }
            }
            let mut references: Vec<_> = sources.into_iter().collect();
            references.sort_unstable();
            ServicePortCheck {
                service_id: service.id.clone(),
                variable,
                references,
            }
        })
        .collect();
    let mut checked_files: Vec<_> = checked_files.into_iter().collect();
    checked_files.sort_unstable();
    PortStandardCheck {
        checked_files,
        services: results,
    }
}

pub fn check_port_standard(
    path: &str,
    command: &str,
    services: Vec<Service>,
) -> Result<PortStandardCheck, String> {
    Ok(audit(
        &project_root(path)?,
        command,
        &normalize_services(services)?,
    ))
}

enum Launch {
    Node {
        pm: String,
        script: Option<&'static str>,
        tool: &'static str,
    },
    Uvicorn {
        python: &'static str,
        module: &'static str,
    },
    Flask {
        python: &'static str,
        module: &'static str,
    },
    Go {
        target: &'static str,
        port_flag: bool,
    },
    Spring(&'static str),
    Cargo,
    Unknown,
}

impl Launch {
    fn detect(path: &Path) -> Self {
        let package = read_json(&path.join("package.json")).unwrap_or(Value::Null);
        let script = ["dev", "start"].into_iter().find(|name| {
            package["scripts"][name]
                .as_str()
                .is_some_and(|body| !body.trim().is_empty())
        });
        if let Some(pm) = package_manager(path, &package) {
            let tool = ["vite", "next", "nuxt", "astro"].into_iter().find(|tool| {
                dependency(&package, tool)
                    || script.is_some_and(|name| {
                        package["scripts"][name]
                            .as_str()
                            .unwrap_or_default()
                            .split_whitespace()
                            .any(|word| word == *tool)
                    })
                    || ["js", "mjs", "ts", "cjs", "mts", "cts"]
                        .iter()
                        .any(|extension| path.join(format!("{tool}.config.{extension}")).is_file())
            });
            if script.is_some() || tool.is_some() {
                return Self::Node {
                    pm,
                    script,
                    tool: tool.unwrap_or(""),
                };
            }
        }
        let dependencies = ["pyproject.toml", "requirements.txt"]
            .iter()
            .filter_map(|file| fs::read_to_string(path.join(file)).ok())
            .collect::<Vec<_>>()
            .join("\n")
            .to_ascii_lowercase();
        let python = if path.join(".venv/bin/python").is_file() {
            "./.venv/bin/python -m"
        } else if path.join("uv.lock").is_file() {
            "uv run"
        } else {
            "python3 -m"
        };
        if dependencies.contains("uvicorn") || dependencies.contains("fastapi") {
            let module = [
                ("app/main.py", "app.main:app"),
                ("src/main.py", "src.main:app"),
                ("main.py", "main:app"),
            ]
            .iter()
            .find(|(file, _)| path.join(file).is_file())
            .map_or("main:app", |(_, module)| *module);
            return Self::Uvicorn { python, module };
        }
        if dependencies.contains("flask") {
            let module = if path.join("wsgi.py").is_file() {
                "wsgi"
            } else if path.join("main.py").is_file() {
                "main"
            } else {
                "app"
            };
            return Self::Flask { python, module };
        }
        if path.join("go.mod").is_file() {
            let (file, target) = [
                ("main.go", "."),
                ("cmd/server/main.go", "./cmd/server"),
                ("cmd/api/main.go", "./cmd/api"),
            ]
            .into_iter()
            .find(|(file, _)| path.join(file).is_file())
            .unwrap_or(("main.go", "."));
            let code = fs::read_to_string(path.join(file)).unwrap_or_default();
            let port_flag = [
                "flag.Int(",
                "flag.String(",
                "flag.IntVar(",
                "flag.StringVar(",
            ]
            .iter()
            .any(|flag| {
                code.lines()
                    .any(|line| line.contains(flag) && line.contains("\"port\""))
            });
            return Self::Go { target, port_flag };
        }
        if path.join("pom.xml").is_file() {
            return Self::Spring(if path.join("mvnw").is_file() {
                "./mvnw spring-boot:run"
            } else {
                "mvn spring-boot:run"
            });
        }
        if path.join("build.gradle").is_file() || path.join("build.gradle.kts").is_file() {
            return Self::Spring(if path.join("gradlew").is_file() {
                "./gradlew bootRun"
            } else {
                "gradle bootRun"
            });
        }
        if path.join("Cargo.toml").is_file() {
            return Self::Cargo;
        }
        Self::Unknown
    }

    fn default_port(&self) -> u16 {
        match self {
            Self::Node { tool: "vite", .. } => 5173,
            Self::Node { tool: "astro", .. } => 4321,
            Self::Node { .. } => 3000,
            Self::Uvicorn { .. } => 8000,
            Self::Flask { .. } => 5000,
            _ => 8080,
        }
    }

    fn command(&self, variable: &str) -> Option<String> {
        let port = format!("\"${variable}\"");
        Some(match self {
            Self::Node { pm, script, tool } => {
                let command = match script {
                    Some(script) if pm == "npm" => format!("npm run {script}"),
                    Some(script) => format!("{pm} run {script}"),
                    None if pm == "npm" => format!("npx --no-install {tool}"),
                    None if pm == "bun" => format!("bunx --no-install {tool}"),
                    None => format!("{pm} exec {tool}"),
                };
                let flags = match *tool {
                    "vite" => format!("--port {port} --strictPort"),
                    "next" if script.is_none() => format!("dev -p {port}"),
                    "next" => format!("-p {port}"),
                    "nuxt" | "astro" if script.is_none() => format!("dev --port {port}"),
                    "nuxt" | "astro" => format!("--port {port}"),
                    _ => return Some(format!("PORT={port} exec {command}")),
                };
                let separator = if pm == "npm" && script.is_some() {
                    " --"
                } else {
                    ""
                };
                format!("exec {command}{separator} {flags}")
            }
            Self::Uvicorn { python, module } => {
                format!("exec {python} uvicorn {module} --host 127.0.0.1 --port {port}")
            }
            Self::Flask { python, module } => {
                format!("exec {python} flask --app {module} run --host 127.0.0.1 --port {port}")
            }
            Self::Go {
                target,
                port_flag: true,
            } => format!("exec go run {target} --port {port}"),
            Self::Go {
                target,
                port_flag: false,
            } => format!("PORT={port} exec go run {target}"),
            Self::Spring(command) => format!("SERVER_PORT={port} exec {command}"),
            Self::Cargo => format!("PORT={port} exec cargo run"),
            Self::Unknown => return None,
        })
    }
}

fn local_variables(services: &[Service]) -> HashMap<&str, String> {
    let mut names: HashSet<_> = services
        .iter()
        .filter(|service| !service.key.starts_with(|ch: char| ch.is_ascii_digit()))
        .map(|service| format!("{}_PORT", service.key))
        .collect();
    services
        .iter()
        .map(|service| {
            let key = service.key.as_str();
            let mut name = format!("{key}_PORT");
            if key.starts_with(|ch: char| ch.is_ascii_digit()) {
                while !names.insert(name.clone())
                    || name.starts_with(|ch: char| ch.is_ascii_digit())
                {
                    name = format!("PORT_{name}");
                }
            }
            (key, name)
        })
        .collect()
}

fn script_path(root: &Path) -> Result<(String, Vec<String>), String> {
    let directory = root.join("scripts");
    match fs::symlink_metadata(&directory) {
        Ok(meta) if !meta.is_dir() => return Err("项目的 scripts 路径须为项目内的普通目录".into()),
        Ok(_) => {}
        Err(err) if err.kind() == ErrorKind::NotFound => {}
        Err(err) => return Err(format!("检查 scripts 目录失败：{err}")),
    }
    let mut preserved = Vec::new();
    if fs::symlink_metadata(directory.join("dev.sh")).is_ok() {
        preserved.push("scripts/dev.sh".into());
    }
    for number in 1.. {
        let name = if number == 1 {
            "scripts/bento-dev.sh".into()
        } else {
            format!("scripts/bento-dev-{number}.sh")
        };
        match fs::symlink_metadata(root.join(&name)) {
            Ok(_) => preserved.push(name),
            Err(err) if err.kind() == ErrorKind::NotFound => return Ok((name, preserved)),
            Err(err) => return Err(format!("检查启动脚本失败：{err}")),
        }
    }
    unreachable!()
}

fn render_script(root: &Path, services: &[Service]) -> Result<StandardScript, String> {
    if services.is_empty() {
        return Err("请选择至少一个服务".into());
    }
    let multi = services.len() >= 2;
    let (relative_path, preserved) = if multi {
        script_path(root)?
    } else {
        ("scripts/bento-dev.sh".into(), Vec::new())
    };
    let mut script = String::from(
        r#"#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
"#,
    );
    let mut notes = Vec::new();
    let variables = local_variables(services);
    let mut ports: HashSet<_> = services.iter().filter_map(|service| service.port).collect();
    let mut commands = Vec::new();
    for service in services {
        let path = root
            .join(&service.dir)
            .canonicalize()
            .map_err(|_| format!("服务目录「{}」不存在", service.dir))?;
        if !path.starts_with(root) || !path.is_dir() {
            return Err(format!("服务「{}」需要使用项目内的目录", service.name));
        }
        let launch = Launch::detect(&path);
        let port = service.port.unwrap_or_else(|| {
            let default = (launch.default_port()..=u16::MAX)
                .find(|port| ports.insert(*port))
                .unwrap_or(launch.default_port());
            notes.push(format!("{} 未配置端口，模板回退到 {default}", service.name));
            default
        });
        let variable = &variables[service.key.as_str()];
        let fallback = if multi && service.open_web {
            format!("${{PORT:-{port}}}")
        } else {
            port.to_string()
        };
        let injected = if multi {
            port_variable(&service.key)
        } else {
            "PORT".into()
        };
        script.push_str(&format!("{variable}=\"${{{injected}:-{fallback}}}\"\n"));
        commands.push((service, launch.command(variable)));
    }
    if let Some(api) = services
        .iter()
        .find(|service| service.key == "API")
        .or_else(|| {
            services
                .iter()
                .find(|service| service.kind.as_deref() == Some("api"))
        })
        .filter(|_| multi)
    {
        let variable = &variables[api.key.as_str()];
        if variable == "API_PORT" {
            script.push_str("export API_PORT\n");
        } else {
            script.push_str(&format!("export API_PORT=\"${variable}\"\n"));
        }
    }
    script.push_str(
        r#"
# 仅在当前脚本拥有进程组时使用 kill 0。
if [[ "$(ps -o pgid= -p "$$" | tr -d '[:space:]')" == "$$" ]]; then
  trap 'kill 0' EXIT
else
  set -m
  cleanup() {
    local pid
    for pid in $(jobs -pr); do
      kill -TERM -- "-$pid" 2>/dev/null || true
    done
  }
  trap cleanup EXIT
fi
trap 'exit 130' INT
trap 'exit 143' TERM
trap 'exit 129' HUP
"#,
    );
    for (service, command) in commands {
        let name = service.name.replace(['\n', '\r'], " ");
        script.push_str(&format!(
            "\n# {name}\n(\n  cd -- \"$ROOT_DIR\"/{}\n",
            quote_shell(&service.dir)
        ));
        if let Some(command) = command {
            script.push_str(&format!("  {command}\n"));
        } else {
            notes.push(format!(
                "{} 的框架未识别，请在运行前填写启动命令",
                service.name
            ));
            script.push_str(&format!(
                "  printf '%s\\n' {} >&2\n  exit 1\n",
                quote_shell(&format!("请填写 {name} 的启动命令"))
            ));
        }
        script.push_str(") &\n");
    }
    script.push_str("\nwait\n");
    Ok(StandardScript {
        relative_path,
        script,
        preserved,
        notes,
    })
}

pub fn standard_script_preview(
    path: &str,
    services: Vec<Service>,
) -> Result<StandardScript, String> {
    render_script(&project_root(path)?, &normalize_services(services)?)
}

pub fn write_standard_script(
    path: &str,
    services: Vec<Service>,
    preview: StandardScript,
) -> Result<String, String> {
    let root = project_root(path)?;
    if services.len() < 2 {
        return Err("至少需要两个服务才能生成多服务启动脚本".into());
    }
    let current = render_script(&root, &normalize_services(services)?)?;
    if current != preview {
        return Err("项目或目标文件已变化，请重新预览后确认".into());
    }
    let directory = root.join("scripts");
    match fs::create_dir(&directory) {
        Ok(()) => {}
        Err(err) if err.kind() == ErrorKind::AlreadyExists => {
            if !fs::symlink_metadata(&directory).is_ok_and(|meta| meta.is_dir()) {
                return Err("项目的 scripts 路径须为项目内的普通目录".into());
            }
        }
        Err(err) => return Err(format!("创建 scripts 目录失败：{err}")),
    }
    let target = root.join(&current.relative_path);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o755)
        .open(&target)
        .map_err(|err| format!("创建 {} 失败：{err}", current.relative_path))?;
    if let Err(err) = file
        .write_all(current.script.as_bytes())
        .and_then(|_| file.sync_all())
    {
        drop(file);
        let _ = fs::remove_file(&target);
        return Err(format!("写入启动脚本失败：{err}"));
    }
    Ok(format!("./{}", current.relative_path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "bento-standard-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path.canonicalize().unwrap())
        }

        fn write(&self, name: &str, body: &str) {
            let destination = self.0.join(name);
            fs::create_dir_all(destination.parent().unwrap()).unwrap();
            fs::write(destination, body).unwrap();
        }

        fn executable(&self, name: &str, body: &str) {
            self.write(name, body);
            fs::set_permissions(self.0.join(name), fs::Permissions::from_mode(0o755)).unwrap();
        }

        fn services() -> Vec<Service> {
            vec![
                Service {
                    id: "front".into(),
                    name: "前端".into(),
                    dir: "apps/my web".into(),
                    port: Some(3100),
                    kind: Some("web".into()),
                    key: "WEB".into(),
                    open_web: true,
                },
                Service {
                    id: "back".into(),
                    name: "后端".into(),
                    dir: "server".into(),
                    port: Some(8100),
                    kind: Some("api".into()),
                    key: "API".into(),
                    open_web: false,
                },
            ]
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn static_audit_reads_direct_scripts_and_framework_config_without_evaluating_them() {
        let fixture = Fixture::new();
        fixture.write(
            "scripts/dev.sh",
            r#"
            # printf "$BENTO_PORT_WEB"
            echo '$BENTO_PORT_API'
            echo scripts/not-called.sh
            bash ./scripts/backend.sh
        "#,
        );
        fixture.write("scripts/not-called.sh", "echo $BENTO_PORT_WEB");
        fixture.write("scripts/backend.sh", "python3 ./scripts/backend-helper.py");
        fixture.write(
            "scripts/backend-helper.py",
            "import os\nprint(os.getenv('BENTO_PORT_API'))",
        );
        fixture.write(
            "apps/my web/vite.config.ts",
            r#"// process.env.BENTO_PORT_WEB
            /* process.env.BENTO_PORT_WEB */
            const hint = 'BENTO_PORT_WEB';
            const wrong = process.env.BENTO_PORT_WEB_2;
        "#,
        );
        let services = Fixture::services();
        let first = audit(&fixture.0, "./scripts/dev.sh", &services);
        assert!(first
            .services
            .iter()
            .all(|service| service.references.is_empty()));
        assert!(first.checked_files.contains(&"scripts/backend.sh".into()));
        assert!(!first
            .checked_files
            .iter()
            .any(|file| file.contains("not-called") || file.contains("backend-helper")));
        fixture.write(
            "scripts/backend.sh",
            r#"API_PORT="${BENTO_PORT_API:-8100}""#,
        );
        fixture.write(
            "apps/my web/next.config.mjs",
            r#"const port = process.env["BENTO_PORT_WEB"];"#,
        );
        let checked = audit(&fixture.0, "bash scripts/dev.sh", &services);
        assert_eq!(
            checked.services[0].references,
            ["apps/my web/next.config.mjs"]
        );
        assert_eq!(checked.services[1].references, ["scripts/backend.sh"]);
    }

    #[test]
    fn default_service_accepts_port_and_package_runner_references_keep_their_scope() {
        let fixture = Fixture::new();
        fixture.write(
            "package.json",
            r#"{"scripts":{"dev":"bash scripts/package-dev.sh"}}"#,
        );
        fixture.write("scripts/package-dev.sh", "echo \"$PORT\"");
        fixture.write("server/vite.config.ts", "const port = process.env.PORT;");
        let mut services = Fixture::services();
        let checked = audit(&fixture.0, "pnpm dev", &services);
        assert_eq!(checked.services[0].references, ["scripts/package-dev.sh"]);
        assert!(checked.services[1].references.is_empty());
        fixture.write("scripts/dev.sh", "pnpm run dev");
        let checked = audit(&fixture.0, "./scripts/dev.sh", &services);
        assert_eq!(checked.services[0].references, ["scripts/package-dev.sh"]);
        services[0].open_web = false;
        services[1].open_web = true;
        let checked = audit(&fixture.0, "pnpm run dev", &services);
        assert!(checked.services[0].references.is_empty());
        assert_eq!(
            checked.services[1].references,
            ["scripts/package-dev.sh", "server/vite.config.ts"]
        );
        services.remove(0);
        assert_eq!(
            audit(&fixture.0, "pnpm dev", &services).services[0].variable,
            "PORT"
        );
    }

    #[test]
    fn checks_scanned_make_and_foreman_launch_recipes_and_their_direct_scripts() {
        let fixture = Fixture::new();
        fixture.write(
            "Makefile",
            "dev:\n\t@bash scripts/web.sh\nunused:\n\t@echo $$BENTO_PORT_API\n",
        );
        fixture.write(
            "Procfile.dev",
            "web: ./scripts/web.sh\napi: bash scripts/api.sh\n",
        );
        fixture.write("scripts/web.sh", r#"vite --port "$BENTO_PORT_WEB""#);
        fixture.write(
            "scripts/api.sh",
            r#"uvicorn main:app --port "$BENTO_PORT_API""#,
        );
        let services = Fixture::services();
        let make = audit(&fixture.0, "make dev", &services);
        assert!(make.checked_files.contains(&"Makefile · dev".into()));
        assert_eq!(make.services[0].references, ["scripts/web.sh"]);
        assert!(make.services[1].references.is_empty());
        let foreman = audit(&fixture.0, "foreman start -f Procfile.dev", &services);
        assert!(foreman.checked_files.contains(&"Procfile.dev".into()));
        assert_eq!(foreman.services[0].references, ["scripts/web.sh"]);
        assert_eq!(foreman.services[1].references, ["scripts/api.sh"]);
    }

    #[test]
    fn single_service_template_reads_only_port_and_cannot_be_written_by_the_multi_service_action() {
        let fixture = Fixture::new();
        fixture.write("apps/my web/package.json", r#"{"scripts":{"dev":"vite"}}"#);
        let services = vec![Fixture::services().remove(0)];
        let path = fixture.0.to_str().unwrap();
        let preview = standard_script_preview(path, services.clone()).unwrap();
        assert!(preview.script.contains(r#"WEB_PORT="${PORT:-3100}""#));
        assert!(!preview.script.contains("BENTO_PORT"));
        assert!(write_standard_script(path, services, preview).is_err());
        assert!(!fixture.0.join("scripts").exists());
    }

    #[test]
    fn templates_use_actual_services_and_create_only_the_confirmed_file() {
        let fixture = Fixture::new();
        fixture.write(
            "apps/my web/package.json",
            r#"{"scripts":{"dev":"vite"},"devDependencies":{"vite":"*"}}"#,
        );
        fixture.write("server/pyproject.toml", r#"dependencies = ["uvicorn"]"#);
        fixture.write("server/app/main.py", "app = object()");
        fixture.write("scripts/dev.sh", "# existing launcher\n");
        let input = fixture.0.to_str().unwrap();
        let services = Fixture::services();
        let preview = standard_script_preview(input, services.clone()).unwrap();
        assert_eq!(preview.relative_path, "scripts/bento-dev.sh");
        assert_eq!(preview.preserved, ["scripts/dev.sh"]);
        for fragment in [
            "set -euo pipefail",
            r#"WEB_PORT="${BENTO_PORT_WEB:-${PORT:-3100}}""#,
            r#"API_PORT="${BENTO_PORT_API:-8100}""#,
            "export API_PORT",
            "trap 'kill 0' EXIT",
            r#"cd -- "$ROOT_DIR"/'apps/my web'"#,
            r#"npm run dev -- --port "$WEB_PORT" --strictPort"#,
            r#"uvicorn app.main:app --host 127.0.0.1 --port "$API_PORT""#,
            "\nwait\n",
        ] {
            assert!(preview.script.contains(fragment), "{fragment}");
        }
        assert!(!fixture.0.join(&preview.relative_path).exists());
        fixture.write("scripts/bento-dev.sh", "# concurrent writer\n");
        assert!(write_standard_script(input, services.clone(), preview)
            .unwrap_err()
            .contains("重新预览"));
        let preview = standard_script_preview(input, services.clone()).unwrap();
        assert_eq!(preview.relative_path, "scripts/bento-dev-2.sh");
        let script = preview.script.clone();
        let command = write_standard_script(input, services.clone(), preview).unwrap();
        let path = fixture.0.join(&command);
        assert_eq!(fs::read_to_string(&path).unwrap(), script);
        assert_eq!(
            fs::read_to_string(fixture.0.join("scripts/dev.sh")).unwrap(),
            "# existing launcher\n"
        );
        assert_eq!(
            fs::read_to_string(fixture.0.join("scripts/bento-dev.sh")).unwrap(),
            "# concurrent writer\n"
        );
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o111,
            0o111
        );
        assert!(Command::new("/bin/bash")
            .arg("-n")
            .arg(&path)
            .status()
            .unwrap()
            .success());
        assert!(audit(&fixture.0, &command, &services)
            .services
            .iter()
            .all(|service| service.references == ["scripts/bento-dev-2.sh"]));
    }

    #[test]
    fn generated_script_passes_port_arguments_and_exports_the_api_port() {
        let fixture = Fixture::new();
        fixture.write(
            "apps/my web/package.json",
            r#"{"scripts":{"dev":"next dev"},"dependencies":{"next":"*"}}"#,
        );
        fixture.write("server/requirements.txt", "Flask");
        fixture.executable(
            "bin/npm",
            "#!/bin/sh\nprintf '%s\\n' \"$*\" \"$API_PORT\" > \"$FRONT_LOG\"\n",
        );
        fixture.executable(
            "server/.venv/bin/python",
            "#!/bin/sh\nprintf '%s\\n' \"$*\" > \"$API_LOG\"\n",
        );
        let path = fixture.0.to_str().unwrap();
        let services = Fixture::services();
        let preview = standard_script_preview(path, services.clone()).unwrap();
        let command = write_standard_script(path, services, preview).unwrap();
        let front_log = fixture.0.join("front.log");
        let api_log = fixture.0.join("api.log");
        let status = Command::new("/bin/bash")
            .arg(fixture.0.join(command))
            .env_clear()
            .env("PATH", format!("{}/bin:/usr/bin:/bin", fixture.0.display()))
            .env("BENTO_PORT_WEB", "3195")
            .env("BENTO_PORT_API", "8195")
            .env("FRONT_LOG", &front_log)
            .env("API_LOG", &api_log)
            .stdin(Stdio::null())
            .process_group(0)
            .status()
            .unwrap();
        assert!(status.success() || status.code() == Some(143));
        assert_eq!(
            fs::read_to_string(front_log).unwrap(),
            "run dev -- -p 3195\n8195\n"
        );
        assert_eq!(
            fs::read_to_string(api_log).unwrap(),
            "-m flask --app app run --host 127.0.0.1 --port 8195\n"
        );
    }
}
