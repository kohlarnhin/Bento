use super::services::{independent_commands, launch_environment, normalize_services, Service};
use super::{dependency, expanded_path, read_json, validate_env, EnvVar};
use crate::processes::{launch_script, quote_shell};
use crate::ProjectConfig;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::ops::Range;
use std::path::{Path, PathBuf};

pub(super) mod tauri_dev;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchConfig {
    path: String,
    start_command: String,
    port: Option<u16>,
    #[serde(default)]
    services: Vec<Service>,
    #[serde(default = "super::default_inject_ports")]
    inject_ports: bool,
    #[serde(default)]
    env: Vec<EnvVar>,
}

impl From<&ProjectConfig> for LaunchConfig {
    fn from(project: &ProjectConfig) -> Self {
        Self {
            path: project.path.clone(),
            start_command: project.start_command.clone(),
            port: project.port,
            services: project.services.clone(),
            inject_ports: project.inject_ports,
            env: project.env.clone(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
enum LaunchMode {
    Single,
    Shared,
    Independent,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LaunchStep {
    service_id: Option<String>,
    dir: String,
    command: String,
    description: String,
    placeholder_hint: bool,
    port_env: Vec<EnvVar>,
    #[serde(skip)]
    cwd: PathBuf,
    #[serde(skip)]
    env: Vec<EnvVar>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchPlan {
    mode: LaunchMode,
    steps: Vec<LaunchStep>,
}

impl LaunchPlan {
    pub fn script(&self) -> Result<String, String> {
        let commands = self
            .steps
            .iter()
            .map(|step| launch_script(&step.cwd.to_string_lossy(), &step.command, &step.env))
            .collect::<Result<Vec<_>, _>>()?;
        if commands.len() == 1 {
            return Ok(commands.into_iter().next().unwrap());
        }
        // Bento owns the parent and the whole process tree of these siblings.
        let jobs = commands
            .into_iter()
            .map(|command| format!("(\n{command}\n) &\n"))
            .collect::<String>();
        Ok(format!("{jobs}wait"))
    }

    pub fn commands(&self) -> impl Iterator<Item = (&str, &str)> {
        self.steps
            .iter()
            .map(|step| (step.dir.as_str(), step.command.as_str()))
    }
}

pub fn launch_plan(config: LaunchConfig) -> Result<LaunchPlan, String> {
    if config.path.trim().is_empty() {
        return Err("请选择有效的项目目录".into());
    }
    let root = expanded_path(&config.path)
        .canonicalize()
        .ok()
        .filter(|path| path.is_dir())
        .ok_or("请选择有效的项目目录")?;
    validate_env(&config.env)?;
    let services = normalize_services(config.services)?;
    let independent = independent_commands(&services)?;
    let mode = if independent {
        LaunchMode::Independent
    } else if services.len() > 1 {
        LaunchMode::Shared
    } else {
        LaunchMode::Single
    };
    let main_port = if services.is_empty() {
        config.port.filter(|port| *port != 0)
    } else {
        services
            .iter()
            .find(|service| service.open_web)
            .and_then(|service| service.port)
    };
    let mut steps = Vec::new();
    if independent {
        for service in &services {
            let cwd = root
                .join(&service.dir)
                .canonicalize()
                .ok()
                .filter(|path| path.is_dir() && path.starts_with(&root))
                .ok_or_else(|| format!("服务「{}」的目录不存在或位于项目外", service.name))?;
            steps.push(make_step(
                &config.env,
                config.inject_ports,
                mode,
                &services,
                service.port,
                service.start_command.as_deref().unwrap(),
                cwd,
                Some(service),
            )?);
        }
    } else {
        steps.push(make_step(
            &config.env,
            config.inject_ports,
            mode,
            &services,
            main_port,
            config.start_command.trim(),
            root,
            None,
        )?);
    }
    Ok(LaunchPlan { mode, steps })
}

fn make_step(
    explicit: &[EnvVar],
    inject: bool,
    mode: LaunchMode,
    services: &[Service],
    port: Option<u16>,
    command: &str,
    cwd: PathBuf,
    service: Option<&Service>,
) -> Result<LaunchStep, String> {
    if command.is_empty() {
        return Err("请输入启动命令".into());
    }
    let mut env = launch_environment(
        explicit,
        port,
        services,
        inject && (port.is_some() || mode == LaunchMode::Shared),
    );
    let mut command = command.to_owned();
    let mut placeholder_hint = false;
    let description = if !inject {
        "端口注入已关闭".into()
    } else if port.is_none()
        && (mode != LaunchMode::Shared || services.iter().all(|item| item.port.is_none()))
    {
        "端口留空，按项目默认运行".into()
    } else if let Some(replaced) = substitute_ports(&command, port, services)? {
        command = replaced;
        "启动时替换端口占位符".into()
    } else if mode == LaunchMode::Shared {
        "通过环境变量向总启动命令传递各服务端口".into()
    } else {
        let port = port.unwrap();
        let adapted = adapt(&command, &cwd, port, &mut env, explicit, true);
        command = adapted.command;
        placeholder_hint = adapted.fallback;
        adapted.description
    };
    let port_env = if !inject || (port.is_none() && mode != LaunchMode::Shared) {
        Vec::new()
    } else {
        env.iter()
            .filter(|entry| is_port_variable(&entry.key))
            .cloned()
            .collect()
    };
    Ok(LaunchStep {
        service_id: service.map(|item| item.id.clone()),
        dir: service.map_or(".", |item| item.dir.as_str()).into(),
        command,
        description,
        placeholder_hint,
        port_env,
        cwd,
        env,
    })
}

fn is_port_variable(key: &str) -> bool {
    matches!(
        key,
        "PORT" | "BENTO_PORTS" | "FLASK_RUN_PORT" | "SERVER_PORT"
    ) || key.starts_with("BENTO_PORT_")
}

fn substitute_ports(
    command: &str,
    port: Option<u16>,
    services: &[Service],
) -> Result<Option<String>, String> {
    let mut rest = command;
    let mut result = String::new();
    let mut changed = false;
    while let Some((before, opening)) = rest.split_once("{port") {
        result.push_str(before);
        if before.ends_with('$') {
            result.push_str("{port");
            rest = opening;
        } else if let Some(tail) = opening.strip_prefix('}') {
            let value = port.ok_or("{port} 需要配置当前服务或默认打开网页的端口")?;
            result.push_str(&value.to_string());
            changed = true;
            rest = tail;
        } else if let Some((key, tail)) = opening
            .strip_prefix(':')
            .and_then(|part| part.split_once('}'))
        {
            let value = services
                .iter()
                .find(|service| service.key == key)
                .and_then(|service| service.port)
                .ok_or_else(|| format!("{{port:{key}}} 对应的服务没有配置端口"))?;
            result.push_str(&value.to_string());
            changed = true;
            rest = tail;
        } else {
            result.push_str("{port");
            rest = opening;
        }
    }
    result.push_str(rest);
    Ok(changed.then_some(result))
}

#[derive(Debug)]
struct Word {
    value: String,
    span: Range<usize>,
    literal: bool,
}

// Only rewrite simple shell commands. Preserve untouched source ranges verbatim.
fn words(command: &str) -> Option<Vec<Word>> {
    let mut chars = command.char_indices().peekable();
    let mut result = Vec::new();
    while let Some(&(start, first)) = chars.peek() {
        if matches!(first, ' ' | '\t') {
            chars.next();
            continue;
        }
        if first == '#' {
            return None;
        }
        let mut value = String::new();
        let mut quote = None;
        let mut literal = true;
        while let Some(&(_, ch)) = chars.peek() {
            if quote.is_none() && matches!(ch, ' ' | '\t') {
                break;
            }
            chars.next();
            match (quote, ch) {
                (Some(delimiter), ch) if ch == delimiter => quote = None,
                (None, '\'' | '"') => quote = Some(ch),
                (None, ';' | '&' | '|' | '<' | '>' | '(' | ')' | '\n' | '\r') => return None,
                (None | Some('"'), '\\') => {
                    let &(_, next) = chars.peek()?;
                    if quote.is_none() || matches!(next, '"' | '\\' | '$' | '\x60' | '\n') {
                        chars.next();
                        if next != '\n' {
                            value.push(next);
                        }
                    } else {
                        value.push('\\');
                    }
                }
                (None | Some('"'), '$' | '\x60') => {
                    literal = false;
                    value.push(ch);
                }
                _ => value.push(ch),
            }
        }
        if quote.is_some() {
            return None;
        }
        let end = chars.peek().map_or(command.len(), |&(index, _)| index);
        result.push(Word {
            value,
            span: start..end,
            literal,
        });
    }
    (!result.is_empty()).then_some(result)
}

fn assignment(word: &Word) -> Option<(&str, &str)> {
    let (key, value) = word.value.split_once('=')?;
    let mut bytes = key.bytes();
    let first = bytes.next()?;
    ((first.is_ascii_alphabetic() || first == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_'))
    .then_some((key, value))
}

fn program_start(words: &[Word]) -> usize {
    words
        .iter()
        .take_while(|word| {
            assignment(word).is_some()
                || matches!(
                    word.value.as_str(),
                    "exec" | "command" | "env" | "cross-env" | "corepack"
                )
        })
        .count()
}

fn executable(word: &Word) -> &str {
    if !word.literal {
        return "";
    }
    Path::new(&word.value)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("")
}

fn is_python(program: &str) -> bool {
    program.strip_prefix("python").is_some_and(|version| {
        version
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'.')
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Syntax {
    Env,
    Long,
    Short,
    Django,
}

#[derive(Clone, Copy)]
struct Adapter {
    label: &'static str,
    syntax: Syntax,
    extra_env: Option<&'static str>,
    strict: bool,
}

impl Adapter {
    fn named(label: &'static str, syntax: Syntax) -> Self {
        Self {
            label,
            syntax,
            extra_env: None,
            strict: false,
        }
    }
    fn flag(self) -> &'static str {
        match self.syntax {
            Syntax::Short => "-p",
            _ => "--port",
        }
    }
}

fn adapter(words: &[Word], start: usize, package: Option<&Value>) -> (Adapter, usize) {
    let fallback = Adapter::named("", Syntax::Env);
    let Some(program) = words.get(start) else {
        return (fallback, start);
    };
    let args = &words[start + 1..];
    let word = |index: usize| args.get(index).map(|arg| arg.value.as_str()).unwrap_or("");
    let has = |value: &str| args.iter().any(|arg| arg.value == value);
    let server = |actions: &[&str]| {
        word(0).is_empty() || word(0).starts_with('-') || actions.contains(&word(0))
    };
    let depend = |name| package.is_some_and(|package| dependency(package, name));
    let mut arg_start = start + 1;
    let mut result = match executable(program) {
        "vite" if !matches!(word(0), "build" | "optimize") => {
            let mut vite = Adapter::named(
                if depend("@sveltejs/kit") {
                    "SvelteKit (Vite)"
                } else {
                    "Vite"
                },
                Syntax::Long,
            );
            vite.strict = true;
            vite
        }
        "next" if server(&["dev", "start"]) => Adapter::named("Next.js", Syntax::Short),
        "nuxt" | "nuxi" if server(&["dev", "start", "preview"]) => {
            Adapter::named("Nuxt", Syntax::Long)
        }
        "astro" if server(&["dev", "preview"]) => Adapter::named("Astro", Syntax::Long),
        "ng" if word(0) == "serve" => Adapter::named("Angular", Syntax::Long),
        "remix" if word(0) == "dev" => Adapter::named("Remix (经典编译器)", Syntax::Env),
        "remix" if matches!(word(0), "vite:dev" | "vite:preview") => {
            Adapter::named("Remix (Vite)", Syntax::Long)
        }
        "remix-serve" => Adapter::named("Remix", Syntax::Env),
        "svelte-kit" if word(0) == "dev" => Adapter::named("SvelteKit", Syntax::Long),
        "react-scripts" | "craco" | "react-app-rewired" if word(0) == "start" => {
            Adapter::named("CRA", Syntax::Env)
        }
        "nest" if word(0) == "start" => Adapter::named("Nest", Syntax::Env),
        "node" if depend("@nestjs/core") => Adapter::named("Nest", Syntax::Env),
        "node" if depend("express") => Adapter::named("Express", Syntax::Env),
        "node" | "tsx" | "nodemon" | "ts-node" => Adapter::named("Node", Syntax::Env),
        "rails" if matches!(word(0), "server" | "s") => Adapter::named("Rails", Syntax::Env),
        "puma" => Adapter::named("Puma", Syntax::Env),
        "uvicorn" => Adapter::named("Uvicorn", Syntax::Long),
        "flask" if has("run") => Adapter::named("Flask", Syntax::Long),
        "manage.py" | "django-admin" if has("runserver") => {
            Adapter::named("Django", Syntax::Django)
        }
        "mvn" | "mvnw" if has("spring-boot:run") => Adapter::named("Spring Boot", Syntax::Env),
        "gradle" | "gradlew" if has("bootRun") => Adapter::named("Spring Boot", Syntax::Env),
        "go" if word(0) == "run" => Adapter::named("Go", Syntax::Env),
        "cargo" if word(0) == "run" => Adapter::named("Rust", Syntax::Env),
        program if is_python(program) && word(0) == "-m" => {
            return adapter(words, start + 2, package)
        }
        program
            if is_python(program)
                && args
                    .first()
                    .is_some_and(|arg| executable(arg) == "manage.py") =>
        {
            return adapter(words, start + 1, package)
        }
        "bundle" if word(0) == "exec" => return adapter(words, start + 2, package),
        "uv" | "poetry" if word(0) == "run" => return adapter(words, start + 2, package),
        "npx" | "bunx" => return adapter(words, start + 1, package),
        "pnpm" | "yarn" if word(0) == "exec" => return adapter(words, start + 2, package),
        _ => fallback,
    };
    match result.label {
        "Django" => {
            arg_start += args
                .iter()
                .position(|arg| arg.value == "runserver")
                .unwrap()
                + 1
        }
        "Flask" => result.extra_env = Some("FLASK_RUN_PORT"),
        "Spring Boot" => result.extra_env = Some("SERVER_PORT"),
        _ => {}
    }
    (result, arg_start)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Manager {
    Npm,
    Pnpm,
    Yarn,
    Bun,
}

struct PackageScript {
    manager: Manager,
    command: String,
    package: Value,
    cwd: PathBuf,
    manager_prefix_end: usize,
    name_index: usize,
    separator: bool,
}

fn package_script(cwd: &Path, words: &[Word], start: usize) -> Option<PackageScript> {
    let manager = match executable(words.get(start)?) {
        "npm" => Manager::Npm,
        "pnpm" => Manager::Pnpm,
        "yarn" => Manager::Yarn,
        "bun" => Manager::Bun,
        _ => return None,
    };
    let mut directory = cwd.to_path_buf();
    let mut index = start + 1;
    while let Some(option) = words.get(index) {
        if !matches!(option.value.as_str(), "--prefix" | "-C" | "--dir" | "--cwd") {
            break;
        }
        let path = words.get(index + 1).filter(|arg| arg.literal)?;
        directory = directory.join(&path.value).canonicalize().ok()?;
        if !directory.starts_with(cwd) {
            return None;
        }
        index += 2;
    }
    let manager_prefix_end = words.get(index)?.span.start;
    match words.get(index)?.value.as_str() {
        "run" | "run-script" => index += 1,
        "start" => {}
        _ if manager == Manager::Npm => return None,
        _ => {}
    }
    let name = &words.get(index).filter(|word| word.literal)?.value;
    let package = read_json(&directory.join("package.json"))?;
    let command = package
        .get("scripts")?
        .get(name)?
        .as_str()?
        .trim()
        .to_owned();
    let separator = words.get(index + 1).is_some_and(|word| word.value == "--");
    Some(PackageScript {
        manager,
        command,
        package,
        cwd: directory,
        manager_prefix_end,
        name_index: index,
        separator,
    })
}

type Edit = (Range<usize>, String);

fn render(command: &str, mut edits: Vec<Edit>) -> String {
    let mut result = command.to_owned();
    edits.sort_by_key(|(span, _)| (span.start, span.end));
    for (span, text) in edits.into_iter().rev() {
        result.replace_range(span, &text);
    }
    result.trim_end().to_owned()
}

fn port_args(words: &[Word], shape: Adapter) -> Vec<(Range<usize>, &'static str)> {
    let mut ports = Vec::new();
    if shape.label == "Remix (经典编译器)" {
        // "remix dev --port" controls its compiler's hot reload server, not the app.
        return ports;
    }
    let mut iter = words.iter().peekable();
    while let Some(word) = iter.next() {
        let arg = word.value.as_str();
        if arg == "--" {
            break;
        }
        if shape.syntax == Syntax::Django {
            if matches!(arg, "--settings" | "--pythonpath" | "--verbosity" | "-v") {
                iter.next();
            } else if !arg.starts_with('-')
                && (!word.literal
                    || arg
                        .rsplit(':')
                        .next()
                        .is_some_and(|port| port.parse::<u16>().is_ok()))
            {
                ports.push((word.span.clone(), ""));
                break;
            }
            continue;
        }
        let flag = if arg == "--port" || arg.starts_with("--port=") {
            "--port"
        } else if (shape.syntax == Syntax::Short || matches!(shape.label, "Rails" | "Puma"))
            && (arg == "-p"
                || arg.starts_with("-p=")
                || arg
                    .strip_prefix("-p")
                    .is_some_and(|value| value.parse::<u16>().is_ok()))
        {
            "-p"
        } else {
            continue;
        };
        let mut span = word.span.clone();
        if arg == flag && iter.peek().is_some_and(|next| !next.value.starts_with('-')) {
            span.end = iter.next().unwrap().span.end;
        }
        ports.push((span, flag));
    }
    ports
}

fn string_port_env(env: &[EnvVar], key: &str, port: u16) -> String {
    env.iter()
        .find(|entry| entry.key == key)
        .map_or_else(|| port.to_string(), |entry| entry.value.clone())
}

fn assignment_edits(words: &[Word], adapter: Adapter, port: u16, env: &[EnvVar]) -> Vec<Edit> {
    words
        .iter()
        .filter_map(|word| {
            let (key, value) = assignment(word)?;
            if key != "PORT" && Some(key) != adapter.extra_env {
                return None;
            }
            let replacement = string_port_env(env, key, port);
            (replacement != value || !word.literal).then(|| {
                (
                    word.span.clone(),
                    format!("{key}={}", quote_shell(&replacement)),
                )
            })
        })
        .collect()
}

fn arg_edits(words: &[Word], adapter: Adapter, port: u16, end: usize) -> Vec<Edit> {
    let args = &words[..words
        .iter()
        .position(|word| word.value == "--")
        .unwrap_or(words.len())];
    let found = port_args(args, adapter);
    let mut edits = Vec::new();
    if adapter.extra_env == Some("SERVER_PORT") {
        edits.extend(args.iter().filter_map(|word| spring_port_edit(word, port)));
    }
    let mut suffix = String::new();
    let replacement = match adapter.syntax {
        Syntax::Django => format!("127.0.0.1:{port}"),
        Syntax::Env if !found.is_empty() => format!("{} {port}", found[0].1),
        _ => format!("{} {port}", adapter.flag()),
    };
    if found.is_empty() {
        if adapter.syntax != Syntax::Env {
            suffix.push_str(&format!(" {replacement}"));
        }
    } else {
        for (index, (span, _)) in found.into_iter().enumerate() {
            edits.push((
                span,
                if index == 0 {
                    replacement.clone()
                } else {
                    String::new()
                },
            ));
        }
    }
    if adapter.strict {
        let mut strict = false;
        for argument in args.iter().filter(|arg| strict_arg(arg)) {
            if !strict {
                strict = true;
                if argument.value == "--strictPort" {
                    continue;
                }
                edits.push((argument.span.clone(), "--strictPort".into()));
            } else {
                edits.push((argument.span.clone(), String::new()));
            }
        }
        if !strict {
            suffix.push_str(" --strictPort");
        }
    }
    if !suffix.is_empty() {
        let insertion = args.last().map_or_else(
            || {
                if let Some(separator) = words.first() {
                    suffix.push(' ');
                    separator.span.start
                } else {
                    end
                }
            },
            |argument| argument.span.end,
        );
        edits.push((insertion..insertion, suffix));
    }
    edits
}

fn spring_port_edit(word: &Word, port: u16) -> Option<Edit> {
    let replace_port = |argument: &str| {
        let (key, _) = argument.split_once('=')?;
        matches!(key, "--server.port" | "-Dserver.port").then(|| format!("{key}={port}"))
    };
    if let Some(replacement) = replace_port(&word.value) {
        return Some((word.span.clone(), replacement));
    }
    let (key, value) = word.value.split_once('=')?;
    if !word.literal
        || !matches!(
            key,
            "--args" | "-Dspring-boot.run.arguments" | "-Dspring-boot.run.jvmArguments"
        )
    {
        return None;
    }
    let embedded = words(value)?;
    let edits: Vec<_> = embedded
        .into_iter()
        .filter_map(|argument| {
            replace_port(&argument.value).map(|replacement| (argument.span, replacement))
        })
        .collect();
    if edits.is_empty() {
        return None;
    }
    Some((
        word.span.clone(),
        format!("{key}={}", quote_shell(&render(value, edits))),
    ))
}

fn strict_arg(word: &Word) -> bool {
    matches!(word.value.as_str(), "--strictPort" | "--no-strictPort")
        || word.value.starts_with("--strictPort=")
}

struct Adapted {
    command: String,
    description: String,
    fallback: bool,
}

fn adapt(
    command: &str,
    cwd: &Path,
    port: u16,
    env: &mut Vec<EnvVar>,
    explicit: &[EnvVar],
    allow_tauri: bool,
) -> Adapted {
    let unknown = || Adapted {
        command: command.into(),
        description: "未识别启动工具，已提供 PORT".into(),
        fallback: true,
    };
    let Some(outer_words) = words(command) else {
        return unknown();
    };
    let start = program_start(&outer_words);
    let package_script = package_script(cwd, &outer_words, start);
    if allow_tauri {
        if let Some(result) = tauri_dev::adapt_tauri(
            command, cwd, &outer_words, start, package_script.as_ref(), port, env, explicit,
        ) {
            return result;
        }
    }
    let package = package_script.as_ref().map(|script| &script.package);
    let direct_package = if package.is_none() {
        read_json(&cwd.join("package.json"))
    } else {
        None
    };
    let inner_words = package_script
        .as_ref()
        .and_then(|script| words(&script.command));
    let inner_start = inner_words.as_ref().map_or(0, |words| program_start(words));
    let (adapter, args_start) = if let Some(inner) = &inner_words {
        adapter(inner, inner_start, package)
    } else {
        adapter(&outer_words, start, direct_package.as_ref())
    };
    if let Some(key) = adapter.extra_env {
        if !explicit.iter().any(|entry| entry.key == key) {
            env.push(EnvVar {
                key: key.into(),
                value: port.to_string(),
            });
        }
    }
    let mut edits = assignment_edits(&outer_words[..start], adapter, port, env);
    let mut rewritten_script = false;
    if let (Some(script), Some(inner)) = (&package_script, &inner_words) {
        let boundary = outer_words[script.name_index].span.end;
        let first_arg = script.name_index + 1 + usize::from(script.separator);
        let outer_args = &outer_words[first_arg..];
        let mut inside = assignment_edits(&inner[..inner_start], adapter, port, env);
        let inner_has_port = !port_args(&inner[args_start..], adapter).is_empty()
            || (adapter.extra_env == Some("SERVER_PORT")
                && inner[args_start..]
                    .iter()
                    .any(|word| spring_port_edit(word, port).is_some()));
        let inner_strict: Vec<_> = inner[args_start..]
            .iter()
            .filter(|word| strict_arg(word))
            .collect();
        let replace_strict = adapter.strict
            && !inner_strict.is_empty()
            && (inner_strict.len() > 1 || inner_strict[0].value != "--strictPort");
        if inner_has_port || !inside.is_empty() || replace_strict {
            // A flag written in package.json cannot be removed by appending to "run".
            // Run the selected script with the edited arguments in the package's local tool environment.
            rewritten_script = true;
            inside.extend(arg_edits(
                &inner[args_start..],
                adapter,
                port,
                script.command.len(),
            ));
            let mut adjusted = render(&script.command, inside);
            if let Some(first) = outer_args.first() {
                adjusted.push(' ');
                let mut removed = port_args(outer_args, adapter);
                if adapter.strict {
                    removed.extend(
                        outer_args
                            .iter()
                            .filter(|word| strict_arg(word))
                            .map(|word| (word.span.clone(), "")),
                    );
                }
                let mut changes: Vec<_> = removed
                    .into_iter()
                    .map(|(span, _)| (span, String::new()))
                    .collect();
                if adapter.extra_env == Some("SERVER_PORT") {
                    changes.extend(
                        outer_args
                            .iter()
                            .filter_map(|word| spring_port_edit(word, port)),
                    );
                }
                adjusted.push_str(&render(
                    &command[first.span.start..],
                    changes
                        .into_iter()
                        .map(|(span, text)| {
                            (
                                span.start - first.span.start..span.end - first.span.start,
                                text,
                            )
                        })
                        .collect(),
                ));
            }
            let prefix =
                command[outer_words[start].span.start..script.manager_prefix_end].trim_end();
            let runner = match script.manager {
                Manager::Npm => format!("{prefix} exec --no -- /bin/sh -c"),
                Manager::Pnpm => format!("{prefix} exec /bin/sh -c"),
                Manager::Yarn => format!("{prefix} exec -- /bin/sh -c"),
                Manager::Bun => {
                    adjusted = format!(
                        r#"cd -- {} && export PATH="$PWD/node_modules/.bin:$PATH" && {adjusted}"#,
                        quote_shell(&script.cwd.to_string_lossy())
                    );
                    "/bin/sh -c".into()
                }
            };
            edits.push((
                outer_words[start].span.start..command.len(),
                format!("{runner} {}", quote_shell(adjusted.trim_end())),
            ));
        } else {
            let mut shape = adapter;
            shape.strict &= inner_strict.is_empty();
            let mut arg_changes = arg_edits(outer_args, shape, port, command.len());
            if adapter.strict && !shape.strict {
                arg_changes.extend(
                    outer_args
                        .iter()
                        .filter(|word| strict_arg(word))
                        .map(|word| (word.span.clone(), String::new())),
                );
            }
            if !arg_changes.is_empty() {
                if script.manager == Manager::Npm && !script.separator {
                    edits.push((boundary..boundary, " --".into()));
                } else if script.manager != Manager::Npm && script.separator {
                    edits.push((
                        outer_words[script.name_index + 1].span.clone(),
                        String::new(),
                    ));
                }
                edits.extend(arg_changes);
            }
        }
    } else if package_script.is_none() {
        edits.extend(arg_edits(
            &outer_words[args_start..],
            adapter,
            port,
            command.len(),
        ));
    }
    let description = match (adapter.label, adapter.syntax) {
        ("", _) => "未识别启动工具，已提供 PORT".to_owned(),
        (label, Syntax::Env) => format!(
            "已识别 {label}，启动时提供 {}",
            adapter.extra_env.unwrap_or("PORT")
        ),
        (label, Syntax::Django) => format!("已识别 {label}，使用 127.0.0.1:{port}"),
        (label, _) if rewritten_script => format!("已识别 {label}，按配置端口执行包脚本"),
        (label, _) => format!(
            "已识别 {label}，自动设置 {}{suffix}",
            adapter.flag(),
            suffix = if adapter.strict {
                " 和 --strictPort"
            } else {
                ""
            }
        ),
    };
    Adapted {
        command: render(command, edits),
        description,
        fallback: adapter.label.is_empty(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "bento-launch-{}-{}",
                std::process::id(),
                NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).unwrap();
            Self(root)
        }

        fn config(&self, command: &str, port: Option<u16>) -> LaunchConfig {
            LaunchConfig {
                path: self.0.to_string_lossy().into_owned(),
                start_command: command.into(),
                port,
                services: Vec::new(),
                inject_ports: true,
                env: Vec::new(),
            }
        }

        fn plan(&self, command: &str) -> LaunchPlan {
            launch_plan(self.config(command, Some(5201))).unwrap()
        }

        fn package(&self, script: &str) {
            let content = serde_json::json!({ "scripts": { "dev": script } });
            fs::write(self.0.join("package.json"), content.to_string()).unwrap();
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn value(step: &LaunchStep, key: &str) -> Option<String> {
        step.env
            .iter()
            .find(|entry| entry.key == key)
            .map(|entry| entry.value.clone())
    }

    fn tauri_config(fixture: &Fixture, file: &str, content: &str) {
        let directory = fixture.0.join("src-tauri");
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join(file), content).unwrap();
    }

    fn tauri_override(step: &LaunchStep) -> Value {
        let args = words(&step.command).unwrap();
        let index = args.iter().position(|arg| arg.value == "--config").unwrap();
        assert!(args[index + 1].literal);
        serde_json::from_str(&args[index + 1].value).unwrap()
    }

    #[test]
    fn tauri_dev_overrides_only_the_dev_port_and_hook_with_package_manager_forwarding() {
        let fixture = Fixture::new();
        let config = r#"{"build":{"devUrl":"https://localhost:5200/workspace/a?mode=dev#preview","beforeDevCommand":"pnpm dev","frontendDist":"../dist"}}"#;
        tauri_config(&fixture, "tauri.conf.json", config);
        fs::write(
            fixture.0.join("package.json"),
            r#"{"scripts":{"tauri":"tauri","desktop":"tauri dev --no-watch","dev":"vite"}}"#,
        )
        .unwrap();
        for (command, prefix) in [
            ("pnpm tauri dev", "pnpm tauri dev --config "),
            ("cargo tauri dev", "cargo tauri dev --config "),
            ("pnpm exec tauri dev", "pnpm exec tauri dev --config "),
            ("pnpm run desktop", "pnpm run desktop --config "),
            ("npm run desktop", "npm run desktop -- --config "),
            ("npm run tauri -- dev", "npm run tauri -- dev --config "),
            ("yarn desktop", "yarn desktop --config "),
            ("bun run desktop", "bun run desktop --config "),
        ] {
            let plan = fixture.plan(command);
            let step = &plan.steps[0];
            assert!(step.command.starts_with(prefix), "{}", step.command);
            assert!(!step.placeholder_hint);
            assert_eq!(value(step, "PORT").as_deref(), Some("5201"));
            assert_eq!(
                tauri_override(step),
                serde_json::json!({"build": {
                    "devUrl": "https://localhost:5201/workspace/a?mode=dev#preview",
                    "beforeDevCommand": "pnpm dev --port 5201 --strictPort"
                }})
            );
        }
        let plan = fixture.plan("pnpm tauri dev --no-watch -- --my-app-flag");
        assert!(plan.steps[0].command.ends_with("' -- --my-app-flag"));
        let plan = fixture.plan("pnpm run desktop -- --my-app-flag");
        assert!(plan.steps[0]
            .command
            .starts_with("pnpm run desktop --config "));
        assert!(plan.steps[0].command.ends_with("' -- --my-app-flag"));
        assert!(!fixture.plan("pnpm tauri build").steps[0]
            .command
            .contains("--config"));
        assert_eq!(
            fs::read_to_string(fixture.0.join("src-tauri/tauri.conf.json")).unwrap(),
            config
        );
    }

    #[test]
    fn tauri_v1_dev_path_and_toml_aliases_reuse_the_next_adapter() {
        let fixture = Fixture::new();
        fixture.package("next dev -p 5200");
        tauri_config(
            &fixture,
            "Tauri.toml",
            r#"
[build]
dev-path = "http://[::1]:5200/legacy/page?q=1"
before-dev-command = "npm run dev"
"#,
        );
        let plan = fixture.plan("tauri dev");
        let overrides = tauri_override(&plan.steps[0]);
        assert_eq!(
            overrides["build"]["dev-path"],
            "http://[::1]:5201/legacy/page?q=1"
        );
        let hook = overrides["build"]["before-dev-command"].as_str().unwrap();
        assert!(hook.contains("next dev -p 5201"), "{hook}");
        assert!(!hook.contains("5200"));
        assert_eq!(
            tauri_dev::detected_port(&fixture.0),
            Some((5200, "src-tauri/Tauri.toml".into()))
        );
    }

    #[test]
    fn tauri_json5_preserves_hook_options_and_quotes_as_one_json_shell_argument() {
        let fixture = Fixture::new();
        let web = fixture.0.join("frontend's app");
        fs::create_dir(&web).unwrap();
        fs::write(web.join("package.json"), r#"{"scripts":{"dev":"vite"}}"#).unwrap();
        tauri_config(
            &fixture,
            "tauri.conf.json5",
            r#"
{
  // Tauri v2, using a custom working directory.
  build: {
    devUrl: 'http://dev.local:5200/some/path',
    beforeDevCommand: {
      script: "pnpm dev --mode 'test\\profile'",
      cwd: "../frontend's app",
      wait: true,
    },
  },
}
"#,
        );
        let plan = fixture.plan("pnpm tauri dev");
        let step = &plan.steps[0];
        let value = tauri_override(step);
        let hook = &value["build"]["beforeDevCommand"];
        assert_eq!(hook["cwd"], "../frontend's app");
        assert_eq!(hook["wait"], true);
        assert_eq!(
            hook["script"],
            r"pnpm dev --mode 'test\profile' --port 5201 --strictPort"
        );
        let tail = step.command.split_once(" --config ").unwrap().1;
        let output = std::process::Command::new("/bin/sh")
            .args([
                "-c",
                &format!("set -- {tail}; test \"$#\" = 1 && printf '%s' \"$1\""),
            ])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stdout).unwrap(),
            value
        );
    }

    #[test]
    fn unsupported_tauri_config_retains_the_command_and_suggests_a_placeholder() {
        let fixture = Fixture::new();
        fixture.package("vite");
        for source in [
            "{invalid",
            r#"{"build":{"devPath":"../dist","beforeDevCommand":"pnpm dev"}}"#,
            r#"{"build":{"devUrl":"http://localhost:5200","beforeDevCommand":"./custom.sh"}}"#,
        ] {
            tauri_config(&fixture, "tauri.conf.json", source);
            let plan = fixture.plan("pnpm tauri dev");
            assert_eq!(plan.steps[0].command, "pnpm tauri dev");
            assert!(plan.steps[0].placeholder_hint);
        }
        tauri_config(
            &fixture,
            "tauri.conf.json",
            r#"{"build":{"devUrl":"http://localhost:5200","beforeDevCommand":"pnpm dev"}}"#,
        );
        let original = "pnpm tauri dev --config ./custom.json";
        let plan = fixture.plan(original);
        assert_eq!(plan.steps[0].command, original);
        assert!(plan.steps[0].placeholder_hint);
    }

    #[test]
    fn package_manager_matrix_adapts_vite_next_and_node_using_correct_argument_forwarding() {
        let fixture = Fixture::new();
        for manager in ["npm", "pnpm", "yarn", "bun"] {
            let command = format!("{manager} run dev");
            let separator = if manager == "npm" { " --" } else { "" };
            for (script, args, name) in [
                ("vite", " --port 5201 --strictPort", "Vite"),
                ("next dev", " -p 5201", "Next.js"),
                ("node app.mjs", "", "Node"),
            ] {
                fixture.package(script);
                let plan = fixture.plan(&command);
                let step = &plan.steps[0];
                let expected = if args.is_empty() {
                    command.clone()
                } else {
                    format!("{command}{separator}{args}")
                };
                assert_eq!(step.command, expected, "{manager} → {script}");
                assert!(step.description.contains(name));
                assert_eq!(value(step, "PORT").as_deref(), Some("5201"));
                assert_eq!(step.port_env, step.env);
                assert!(plan.script().unwrap().contains(&step.command));
            }
        }
    }

    #[test]
    fn replaces_existing_port_flags_and_preserves_quoted_args() {
        let fixture = Fixture::new();
        for (before, after) in [
            (
                "vite --port=5200 --host '::1' --strictPort",
                "vite --port 5201 --host '::1' --strictPort",
            ),
            (
                "next dev --port 5200 --hostname=127.0.0.1",
                "next dev -p 5201 --hostname=127.0.0.1",
            ),
            ("next start -p=5200", "next start -p 5201"),
            (
                "uvicorn app:app --port \"$PORT\" --host 0.0.0.0",
                "uvicorn app:app --port 5201 --host 0.0.0.0",
            ),
            ("puma -p 5200", "puma -p 5201"),
            ("node server.mjs --port=5200", "node server.mjs --port 5201"),
            (
                "vite --port 5000 --port=5200",
                "vite --port 5201  --strictPort",
            ),
            ("PORT=5200 node server.mjs", "PORT='5201' node server.mjs"),
        ] {
            assert_eq!(fixture.plan(before).steps[0].command, after, "{before}");
        }
        fixture.package("vite --strictPort");
        assert_eq!(
            fixture.plan("npm run dev -- --port 5200").steps[0].command,
            "npm run dev -- --port 5201"
        );
        assert_eq!(
            fixture.plan("pnpm run dev --port=5200").steps[0].command,
            "pnpm run dev --port 5201"
        );
    }

    #[test]
    fn replaces_ports_in_package_scripts_without_writing_the_manifest() {
        let fixture = Fixture::new();
        for (script, replacement) in [
            (
                "vite --host 127.0.0.1 --port 5200",
                "vite --host 127.0.0.1 --port 5201 --strictPort",
            ),
            ("next dev -p 5200", "next dev -p 5201"),
            ("node server.mjs --port=5200", "node server.mjs --port 5201"),
        ] {
            fixture.package(script);
            let original = fs::read(fixture.0.join("package.json")).unwrap();
            for manager in ["npm", "pnpm", "yarn", "bun"] {
                let plan = fixture.plan(&format!("{manager} run dev --port 5000"));
                let command = &plan.steps[0].command;
                assert!(command.contains(replacement), "{command}");
                if manager == "yarn" {
                    assert!(command.starts_with("yarn exec -- /bin/sh -c "));
                }
                assert!(
                    !command.contains("5200") && !command.contains("5000"),
                    "{command}"
                );
                assert_eq!(
                    command.matches("--port").count() + command.matches(" -p").count(),
                    1,
                    "{command}"
                );
            }
            assert_eq!(fs::read(fixture.0.join("package.json")).unwrap(), original);
        }
    }

    #[test]
    fn adapts_framework_arguments_and_preserves_explicit_port_environment() {
        let fixture = Fixture::new();
        for (command, expected, key) in [
            (
                "python3.11 manage.py runserver --verbosity 2 0.0.0.0:5200",
                "python3.11 manage.py runserver --verbosity 2 127.0.0.1:5201",
                "PORT",
            ),
            (
                "python3 manage.py runserver",
                "python3 manage.py runserver 127.0.0.1:5201",
                "PORT",
            ),
            (
                "uv run uvicorn app:app",
                "uv run uvicorn app:app --port 5201",
                "PORT",
            ),
            (
                "flask --app app run --port=5200",
                "flask --app app run --port 5201",
                "FLASK_RUN_PORT",
            ),
            (
                "python -m flask run",
                "python -m flask run --port 5201",
                "FLASK_RUN_PORT",
            ),
            ("nuxt dev", "nuxt dev --port 5201", "PORT"),
            ("astro dev", "astro dev --port 5201", "PORT"),
            ("ng serve", "ng serve --port 5201", "PORT"),
            ("remix vite:dev", "remix vite:dev --port 5201", "PORT"),
            ("remix dev --port 3001", "remix dev --port 3001", "PORT"),
            ("remix-serve build/index.js", "remix-serve build/index.js", "PORT"),
            ("mvn spring-boot:run", "mvn spring-boot:run", "SERVER_PORT"),
            ("./gradlew bootRun", "./gradlew bootRun", "SERVER_PORT"),
            (
                "mvn spring-boot:run -Dspring-boot.run.arguments='--server.port=5200 --spring.profiles.active=dev'",
                "mvn spring-boot:run -Dspring-boot.run.arguments='--server.port=5201 --spring.profiles.active=dev'",
                "SERVER_PORT",
            ),
            (
                "./gradlew bootRun --args='--server.port=5200'",
                "./gradlew bootRun --args='--server.port=5201'",
                "SERVER_PORT",
            ),
            ("react-scripts start", "react-scripts start", "PORT"),
            ("nest start --watch", "nest start --watch", "PORT"),
            (
                "bundle exec rails server",
                "bundle exec rails server",
                "PORT",
            ),
            ("go run .", "go run .", "PORT"),
            ("cargo run", "cargo run", "PORT"),
        ] {
            let plan = fixture.plan(command);
            let step = &plan.steps[0];
            assert_eq!(step.command, expected, "{command}");
            assert_eq!(value(step, key).as_deref(), Some("5201"), "{command}");
            assert!(!step.placeholder_hint, "{command}");
        }
        let mut config = fixture.config("PORT=5200 flask run", Some(5201));
        config.env.push(EnvVar {
            key: "FLASK_RUN_PORT".into(),
            value: "5290".into(),
        });
        config.env.push(EnvVar {
            key: "PORT".into(),
            value: "5291".into(),
        });
        let plan = launch_plan(config).unwrap();
        assert_eq!(plan.steps[0].command, "PORT='5291' flask run --port 5201");
        assert_eq!(
            value(&plan.steps[0], "FLASK_RUN_PORT").as_deref(),
            Some("5290")
        );
        fs::write(
            fixture.0.join("package.json"),
            r#"{"devDependencies":{"@sveltejs/kit":"*"},"scripts":{"dev":"vite dev"}}"#,
        )
        .unwrap();
        let plan = fixture.plan("pnpm run dev");
        assert_eq!(
            plan.steps[0].command,
            "pnpm run dev --port 5201 --strictPort"
        );
        assert!(plan.steps[0].description.contains("SvelteKit"));
    }

    fn services() -> Vec<Service> {
        serde_json::from_value(serde_json::json!([
            {"id":"web","name":"网页","dir":"web","port":5201,"key":"WEB","openWeb":true},
            {"id":"api","name":"API","dir":"api","port":8201,"key":"API","openWeb":false}
        ]))
        .unwrap()
    }

    #[test]
    fn placeholders_take_precedence_and_shared_commands_keep_v018_port_variables() {
        let fixture = Fixture::new();
        let plan = fixture.plan("vite --port '{port}' --host \"$HOST\" && echo \"$port\" ${port}");
        assert_eq!(
            plan.steps[0].command,
            "vite --port '5201' --host \"$HOST\" && echo \"$port\" ${port}"
        );
        assert_eq!(value(&plan.steps[0], "PORT").as_deref(), Some("5201"));
        let mut config = fixture.config(
            "./dev.sh --front={port:WEB} --api={port:API} --same={port}",
            Some(5201),
        );
        config.services = services();
        let plan = launch_plan(config).unwrap();
        assert!(plan.mode == LaunchMode::Shared);
        assert_eq!(
            plan.steps[0].command,
            "./dev.sh --front=5201 --api=8201 --same=5201"
        );
        assert_eq!(
            value(&plan.steps[0], "BENTO_PORT_WEB").as_deref(),
            Some("5201")
        );
        assert_eq!(
            value(&plan.steps[0], "BENTO_PORT_API").as_deref(),
            Some("8201")
        );
        assert_eq!(
            value(&plan.steps[0], "BENTO_PORTS").as_deref(),
            Some("WEB:5201,API:8201")
        );
        assert!(launch_plan(fixture.config("./dev.sh --port {port:API}", Some(5201))).is_err());
    }

    #[test]
    fn blank_or_disabled_ports_leave_the_command_and_explicit_env_alone() {
        let fixture = Fixture::new();
        let mut config = fixture.config("vite", None);
        config.env.push(EnvVar {
            key: "CUSTOM".into(),
            value: "kept".into(),
        });
        let plan = launch_plan(config).unwrap();
        assert_eq!(plan.steps[0].command, "vite");
        assert!(plan.steps[0].port_env.is_empty());
        assert_eq!(value(&plan.steps[0], "CUSTOM").as_deref(), Some("kept"));
        assert!(value(&plan.steps[0], "PORT").is_none());
        let mut config = fixture.config("vite --port 5200", Some(5201));
        config.inject_ports = false;
        let plan = launch_plan(config).unwrap();
        assert_eq!(plan.steps[0].command, "vite --port 5200");
        assert!(plan.steps[0].env.is_empty());
        let plan = fixture.plan("./wrap.sh && other-command");
        assert_eq!(plan.steps[0].command, "./wrap.sh && other-command");
        assert!(plan.steps[0].placeholder_hint);
    }

    #[test]
    fn independent_services_resolve_tooling_and_ports_in_their_own_directories() {
        let fixture = Fixture::new();
        fs::create_dir(fixture.0.join("web")).unwrap();
        fs::create_dir(fixture.0.join("api")).unwrap();
        fs::write(
            fixture.0.join("web/package.json"),
            r#"{"scripts":{"dev":"vite"}}"#,
        )
        .unwrap();
        let mut config = fixture.config("", None);
        config.services = services();
        config.services[0].start_command = Some("pnpm run dev".into());
        config.services[1].start_command = Some("uvicorn app:app".into());
        let plan = launch_plan(config).unwrap();
        assert!(plan.mode == LaunchMode::Independent);
        assert_eq!(plan.steps.len(), 2);
        assert_eq!(
            plan.steps[0].command,
            "pnpm run dev --port 5201 --strictPort"
        );
        assert_eq!(plan.steps[1].command, "uvicorn app:app --port 8201");
        assert_eq!(value(&plan.steps[0], "PORT").as_deref(), Some("5201"));
        assert_eq!(value(&plan.steps[1], "PORT").as_deref(), Some("8201"));
        assert!(plan.script().unwrap().ends_with(") &\nwait"));
    }
}
