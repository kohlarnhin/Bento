use super::{
    adapt, executable, program_start, words, Adapted, EnvVar, Manager, PackageScript, Word,
};
use crate::processes::quote_shell;
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
use tauri::Url;
use tauri_utils::{config::parse, platform::Target};

pub(crate) fn has_config(root: &Path) -> bool {
    parse::folder_has_configuration_file(Target::current(), &root.join("src-tauri"))
}

fn read_config(root: &Path) -> Option<(Value, Vec<PathBuf>)> {
    parse::read_from(Target::current(), &root.join("src-tauri")).ok()
}

fn dev_url(build: &Value) -> Option<(&'static str, Url)> {
    let (key, value) = ["devUrl", "dev-url", "devPath", "dev-path"]
        .into_iter()
        .find_map(|key| build[key].as_str().map(|value| (key, value)))?;
    let url = Url::parse(value).ok()?;
    matches!(url.scheme(), "http" | "https").then_some((key, url))
}

pub(crate) fn detected_port(root: &Path) -> Option<(u16, String)> {
    let (config, paths) = read_config(root)?;
    let (_, url) = dev_url(&config["build"])?;
    let source = paths.first()?.file_name()?.to_str()?;
    Some((url.port()?, format!("src-tauri/{source}")))
}

fn tauri_args(words: &[Word], start: usize) -> Option<&[Word]> {
    let command = executable(words.get(start)?);
    let mut next = start + 1;
    match command {
        "tauri" => return Some(&words[next..]),
        "cargo" | "npx" | "bunx" => {}
        "pnpm" | "yarn" | "bun" => {
            if matches!(
                words.get(next)?.value.as_str(),
                "exec" | "run" | "run-script"
            ) {
                next += 1;
            }
            if words.get(next)?.value == "--" {
                next += 1;
            }
        }
        "npm" if words.get(next)?.value == "exec" && words.get(next + 1)?.value == "--" => {
            next += 2
        }
        _ => return None,
    }
    (executable(words.get(next)?) == "tauri").then_some(&words[next + 1..])
}

fn rewrite_config(
    root: &Path,
    port: u16,
    env: &mut Vec<EnvVar>,
    explicit: &[EnvVar],
) -> Option<String> {
    let (config, _) = read_config(root)?;
    let build = &config["build"];
    let (url_key, mut url) = dev_url(build)?;
    url.set_port(Some(port)).ok()?;
    let hook_key = ["beforeDevCommand", "before-dev-command"]
        .into_iter()
        .find(|key| !build[key].is_null())?;
    let hook = &build[hook_key];
    let (script, cwd) = match hook {
        Value::String(script) => (script.as_str(), root.to_path_buf()),
        Value::Object(_) => {
            // Tauri resolves an explicit hook cwd from src-tauri and defaults to the frontend root.
            let cwd = if hook["cwd"].is_null() {
                root.to_path_buf()
            } else {
                root.join("src-tauri")
                    .join(hook["cwd"].as_str()?)
                    .canonicalize()
                    .ok()?
            };
            (hook["script"].as_str()?, cwd)
        }
        _ => return None,
    };
    let mut updated_env = env.clone();
    let adapted = adapt(script.trim(), &cwd, port, &mut updated_env, explicit, false);
    if adapted.fallback {
        return None;
    }
    let mut new_hook = hook.clone();
    if new_hook.is_object() {
        new_hook["script"] = Value::String(adapted.command);
    } else {
        new_hook = Value::String(adapted.command);
    }
    let overrides = Map::from_iter([
        (url_key.into(), Value::String(url.into())),
        (hook_key.into(), new_hook),
    ]);
    let json = serde_json::to_string(&serde_json::json!({ "build": overrides })).ok()?;
    *env = updated_env;
    Some(json)
}

pub(super) fn adapt_tauri(
    command: &str,
    cwd: &Path,
    outer: &[Word],
    start: usize,
    package: Option<&PackageScript>,
    port: u16,
    env: &mut Vec<EnvVar>,
    explicit: &[EnvVar],
) -> Option<Adapted> {
    let inner_words;
    let (root, inner, forwarded) = if let Some(script) = package {
        inner_words = words(&script.command)?;
        (
            script.cwd.as_path(),
            tauri_args(&inner_words, program_start(&inner_words))?,
            &outer[script.name_index
                + 1
                + usize::from(script.manager == Manager::Npm && script.separator)..],
        )
    } else {
        (cwd, &[][..], tauri_args(outer, start)?)
    };
    let mut args = inner.iter().chain(forwarded);
    if !args
        .next()
        .is_some_and(|arg| arg.literal && arg.value == "dev")
    {
        return None;
    }
    let fallback = |description: &str| {
        Some(Adapted {
            command: command.into(),
            description: description.into(),
            fallback: true,
        })
    };
    // The v1 CLI accepts only one config. Existing overlays can change both the URL and hook.
    if inner.iter().any(|arg| arg.value == "--")
        || args.take_while(|arg| arg.value != "--").any(|arg| {
            arg.value == "--config"
                || arg.value.starts_with("--config=")
                || arg.value.starts_with("-c")
        })
    {
        return fallback("已识别 Tauri dev，保留已有配置或脚本参数，已提供 PORT");
    }
    let Some(json) = rewrite_config(root, port, env, explicit) else {
        return fallback("已识别 Tauri dev，无法自动适配开发地址或前端命令，已提供 PORT");
    };
    let insertion = forwarded
        .iter()
        .find(|arg| arg.value == "--")
        .map_or(command.len(), |arg| arg.span.start);
    let mut adjusted = command[..insertion].trim_end().to_owned();
    if let Some(script) =
        package.filter(|script| script.manager == Manager::Npm && !script.separator)
    {
        adjusted.insert_str(outer[script.name_index].span.end, " --");
    }
    adjusted.push_str(" --config ");
    adjusted.push_str(&quote_shell(&json));
    if insertion < command.len() {
        adjusted.push(' ');
        adjusted.push_str(&command[insertion..]);
    }
    Some(Adapted {
        command: adjusted,
        description: format!("已识别 Tauri dev，使用 :{port} 启动前端并覆盖开发地址"),
        fallback: false,
    })
}
