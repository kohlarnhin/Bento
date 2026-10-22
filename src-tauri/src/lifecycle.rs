use crate::{get_projects_sync, logs, processes, shell_env, LogPayload, RunningProjectInfo};
use serde::Serialize;
use std::collections::HashMap;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, State};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::Mutex;

pub static SHUTTING_DOWN: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Starting,
    Running,
}

#[derive(Clone)]
pub struct ManagedProject {
    pub id: String,
    pub port: Option<u16>,
    pub phase: Phase,
}

#[derive(Clone)]
pub struct ProcessInfo {
    pub pid: u32,
    pgid: i32,
    port: Option<u16>,
    phase: Phase,
    identity: Option<processes::ProcessIdentity>,
}
impl ProcessInfo {
    fn root(&self) -> processes::ManagedRoot {
        processes::ManagedRoot {
            pid: self.pid,
            pgid: self.pgid,
            identity: self.identity,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ExitFailure {
    code: Option<i32>,
    signal: Option<i32>,
}

#[derive(Default)]
pub struct RuntimeState {
    pub managed: HashMap<String, ProcessInfo>,
    pub stopping: HashMap<u32, (String, Arc<processes::ProcessTree>)>,
    failures: HashMap<String, ExitFailure>,
    revisions: HashMap<String, u64>,
    sequence: u64,
    operations: HashMap<String, Arc<Mutex<()>>>,
}
pub type ProcessMap = Arc<Mutex<RuntimeState>>;

impl RuntimeState {
    pub fn managed_snapshot(&self) -> Vec<ManagedProject> {
        let mut projects: Vec<_> = self
            .managed
            .iter()
            .map(|(id, info)| ManagedProject {
                id: id.clone(),
                port: info.port,
                phase: info.phase,
            })
            .collect();
        projects.sort_by(|left, right| left.id.cmp(&right.id));
        projects
    }
    fn changed(&mut self, id: &str) {
        self.sequence += 1;
        self.revisions.insert(id.into(), self.sequence);
    }
    fn clear_failure(&mut self, id: &str) {
        self.failures.remove(id);
        self.changed(id);
    }
    fn owns(&self, id: &str, pid: u32) -> bool {
        self.managed.get(id).is_some_and(|info| info.pid == pid)
    }
    fn exited(&mut self, id: &str, pid: u32, code: Option<i32>, signal: Option<i32>) -> bool {
        if !self.owns(id, pid) {
            return false;
        }
        self.managed.remove(id);
        if code != Some(0) || signal.is_some() {
            self.failures
                .insert(id.into(), ExitFailure { code, signal });
        }
        self.changed(id);
        true
    }
    fn status(
        &self,
        id: &str,
        port: Option<u16>,
        listener: (bool, Option<u32>),
    ) -> ProjectRuntimeStatus {
        compute_status(
            id,
            port,
            self.managed.get(id),
            self.failures.get(id),
            listener,
            *self.revisions.get(id).unwrap_or(&0),
        )
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRuntimeStatus {
    project_id: String,
    port: Option<u16>,
    status: String,
    is_port_in_use: bool,
    is_managed: bool,
    external: bool,
    pid: Option<u32>,
    exit_code: Option<i32>,
    exit_signal: Option<i32>,
    revision: u64,
}

fn compute_status(
    id: &str,
    port: Option<u16>,
    managed: Option<&ProcessInfo>,
    failure: Option<&ExitFailure>,
    listener: (bool, Option<u32>),
    revision: u64,
) -> ProjectRuntimeStatus {
    let status = if let Some(info) = managed {
        if info.phase == Phase::Starting {
            "starting"
        } else {
            "running"
        }
    } else if listener.0 {
        "running"
    } else if failure.is_some() {
        "error"
    } else {
        "stopped"
    };
    let failure = if status == "error" { failure } else { None };
    ProjectRuntimeStatus {
        project_id: id.into(),
        port,
        status: status.into(),
        is_port_in_use: managed.map_or(listener.0, |info| info.phase == Phase::Running),
        is_managed: managed.is_some(),
        external: managed.is_none() && listener.0,
        pid: managed.map(|info| info.pid).or(listener.1),
        exit_code: failure.and_then(|exit| exit.code),
        exit_signal: failure.and_then(|exit| exit.signal),
        revision,
    }
}

pub fn log(app: &AppHandle, id: &str, text: String) {
    #[cfg(debug_assertions)]
    crate::debug_e2e::record_system_log();
    let _ = app.emit(
        "project-log",
        LogPayload {
            project_id: id.into(),
            text,
            stream: "system".into(),
        },
    );
}
fn emit_status(app: &AppHandle, data: &RuntimeState, id: &str, port: Option<u16>) {
    let _ = app.emit("project-status", data.status(id, port, (false, None)));
    crate::tray::refresh(app);
}

pub async fn operation(state: &ProcessMap, id: &str) -> Arc<Mutex<()>> {
    state
        .lock()
        .await
        .operations
        .entry(id.into())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

#[tauri::command]
pub async fn get_running_projects(
    state: State<'_, ProcessMap>,
) -> Result<Vec<RunningProjectInfo>, String> {
    Ok(state
        .lock()
        .await
        .managed
        .iter()
        .map(|(id, info)| RunningProjectInfo {
            project_id: id.clone(),
            pid: info.pid,
            port: info.port,
        })
        .collect())
}

#[tauri::command]
pub async fn get_projects_runtime_status(
    state: State<'_, ProcessMap>,
) -> Result<Vec<ProjectRuntimeStatus>, String> {
    let (managed, failures, revisions) = {
        let data = state.lock().await;
        (
            data.managed.clone(),
            data.failures.clone(),
            data.revisions.clone(),
        )
    };
    tauri::async_runtime::spawn_blocking(move || {
        let projects = get_projects_sync()?.projects;
        let listeners = processes::listening_ports();
        Ok(projects
            .into_iter()
            .map(|project| {
                let info = managed.get(&project.id);
                let listener = if info.is_some_and(|info| info.phase == Phase::Running) {
                    (true, info.map(|info| info.pid))
                } else {
                    project
                        .port
                        .map(|port| {
                            if let Some(pids) = listeners.get(&port) {
                                (true, pids.first().copied())
                            } else {
                                (processes::probe_loopback(port), None)
                            }
                        })
                        .unwrap_or((false, None))
                };
                compute_status(
                    &project.id,
                    project.port,
                    info,
                    failures.get(&project.id),
                    listener,
                    *revisions.get(&project.id).unwrap_or(&0),
                )
            })
            .collect())
    })
    .await
    .map_err(|err| format!("读取项目状态失败：{err}"))?
}

#[tauri::command]
pub async fn start_project(
    app: AppHandle,
    id: String,
    state: State<'_, ProcessMap>,
) -> Result<(), String> {
    let operation = operation(&state, &id).await;
    let _guard = operation.lock().await;
    start(&app, &id, state.inner()).await
}

async fn start(app: &AppHandle, id: &str, state: &ProcessMap) -> Result<(), String> {
    if state.lock().await.managed.contains_key(id) {
        return Err("项目已经在运行中".into());
    }
    let id_owned = id.to_string();
    let project = tauri::async_runtime::spawn_blocking(move || {
        let response = get_projects_sync()?;
        let project = response
            .projects
            .into_iter()
            .find(|p| p.id == id_owned)
            .ok_or("未找到项目配置")?;
        if response
            .conflicts
            .iter()
            .any(|conflict| conflict.project_ids.contains(&id_owned))
        {
            return Err("项目端口存在重复冲突，请更换端口后再启动".into());
        }
        if let Some(port) = project.port {
            let (in_use, pid) = crate::check_port_in_use_sync(port);
            if in_use {
                let pid = pid.map(|pid| format!("（PID: {pid}）")).unwrap_or_default();
                return Err(format!(
                    "端口 {port} 当前已被占用{pid}，请先确认并结束外部进程"
                ));
            }
        }
        Ok(project)
    })
    .await
    .map_err(|err| format!("启动项目失败：{err}"))??;
    let path = crate::expand_tilde(&project.path);
    if !std::path::Path::new(&path).is_dir() {
        return Err(format!("项目目录不存在：{}", project.path));
    }
    log(app, id, format!("[Bento] 正在启动项目: {}", project.name));
    log(app, id, format!("[Bento] 工作目录: {path}"));
    log(
        app,
        id,
        format!("[Bento] 执行命令: {}", project.start_command),
    );
    let script = processes::launch_script(&path, &project.start_command, &project.env)?;
    let environment = tauri::async_runtime::spawn_blocking(shell_env::capture)
        .await
        .unwrap_or_else(|err| Err(format!("读取环境任务失败：{err}")));
    let mut command = std::process::Command::new("zsh");
    match environment {
        Ok(env) => {
            command.args(["-f", "-c", &script]).env_clear().envs(env);
        }
        Err(reason) => {
            log(app, id, format!("[Bento] 读取终端环境失败（{reason}），已改用登录 shell 启动，.zshrc 中的设置可能不会生效"));
            command.args(["-lc", &script]);
            for (key, _) in std::env::vars_os() {
                if key.to_str().is_some_and(shell_env::should_strip) {
                    command.env_remove(key);
                }
            }
        }
    }
    command
        .process_group(0)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let (mut child, pid) = {
        let mut data = state.lock().await;
        if SHUTTING_DOWN.load(Ordering::SeqCst) {
            return Err("Bento 正在退出".into());
        }
        data.clear_failure(id);
        let child = tokio::process::Command::from(command)
            .spawn()
            .map_err(|err| format!("启动进程失败：{err}"))?;
        let pid = child.id().ok_or("无法获取子进程 PID")?;
        data.managed.insert(
            id.into(),
            ProcessInfo {
                pid,
                pgid: pid as i32,
                port: project.port,
                phase: Phase::Starting,
                identity: processes::identity(pid),
            },
        );
        emit_status(app, &data, id, project.port);
        (child, pid)
    };
    let (sender, receiver) = tokio::sync::mpsc::channel(2048);
    let (finish_logs, finished) = tokio::sync::oneshot::channel();
    let log_app = app.clone();
    let log_id = id.to_string();
    let mut delivery =
        tauri::async_runtime::spawn(logs::aggregate(receiver, finished, move |entries| {
            #[cfg(debug_assertions)]
            crate::debug_e2e::record_log_batch(entries.len());
            let _ = log_app.emit(
                "project-log-batch",
                logs::Batch {
                    project_id: log_id.clone(),
                    entries,
                },
            );
        }));
    for (stream, reader) in [
        (
            "stdout",
            child
                .stdout
                .take()
                .map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Send + Unpin>),
        ),
        (
            "stderr",
            child
                .stderr
                .take()
                .map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Send + Unpin>),
        ),
    ] {
        if let Some(reader) = reader {
            let sender = sender.clone();
            tauri::async_runtime::spawn(async move {
                let mut lines = BufReader::new(reader).lines();
                while let Ok(Some(text)) = lines.next_line().await {
                    if sender
                        .send(logs::Entry {
                            text,
                            stream: stream.into(),
                        })
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            });
        }
    }
    drop(sender);
    if let Some(port) = project.port {
        let auto_open = project.auto_open && project.project_type.as_deref() == Some("web");
        let app = app.clone();
        let state = state.clone();
        let id = id.to_string();
        tauri::async_runtime::spawn(async move {
            let started = Instant::now();
            let mut warned = false;
            let mut attempts = 0;
            loop {
                if !state.lock().await.owns(&id, pid) {
                    break;
                }
                attempts += 1;
                let check_lsof = attempts % 4 == 0;
                let ready = tauri::async_runtime::spawn_blocking(move || {
                    processes::probe_loopback(port)
                        || (check_lsof && !processes::port_pids(port).is_empty())
                })
                .await
                .unwrap_or(false);
                let mut data = state.lock().await;
                if !data.owns(&id, pid) {
                    break;
                }
                if ready {
                    data.managed.get_mut(&id).unwrap().phase = Phase::Running;
                    data.changed(&id);
                    log(&app, &id, format!("[Bento] 端口 {port} 已就绪"));
                    emit_status(&app, &data, &id, Some(port));
                    // Still under the pid-match lock: a stopped/replaced start cannot open a URL.
                    if auto_open {
                        if let Err(error) =
                            crate::open_url_sync(&format!("http://localhost:{port}"))
                        {
                            log(&app, &id, format!("[Bento] 自动打开网页失败：{error}"));
                        }
                    }
                    break;
                }
                if !warned && started.elapsed() >= Duration::from_secs(60) {
                    warned = true;
                    log(
                        &app,
                        &id,
                        format!(
                            "[Bento] 端口 {port} 仍未监听，请确认配置端口与项目实际监听端口一致"
                        ),
                    );
                }
                drop(data);
                tokio::time::sleep(if warned {
                    Duration::from_secs(3)
                } else {
                    Duration::from_millis(500)
                })
                .await;
            }
        });
    } else {
        let mut data = state.lock().await;
        if data.owns(id, pid) {
            data.managed.get_mut(id).unwrap().phase = Phase::Running;
            data.changed(id);
            emit_status(app, &data, id, None);
        }
    }
    let app = app.clone();
    let state = state.clone();
    let id = id.to_string();
    tauri::async_runtime::spawn(async move {
        let result = child.wait().await;
        let code = result.as_ref().ok().and_then(|status| status.code());
        let signal = result.as_ref().ok().and_then(|status| status.signal());
        if tokio::time::timeout(Duration::from_secs(1), &mut delivery)
            .await
            .is_err()
        {
            let _ = finish_logs.send(());
            let _ = delivery.await;
        }
        let mut data = state.lock().await;
        if !data.exited(&id, pid, code, signal) {
            return;
        }
        emit_status(&app, &data, &id, project.port);
        let reason = signal
            .map(|signal| format!("信号 {signal}"))
            .unwrap_or_else(|| {
                code.map(|code| format!("状态码 {code}"))
                    .unwrap_or_else(|| "无法读取退出状态".into())
            });
        log(
            &app,
            &id,
            format!(
                "[Bento] 进程{}退出（{reason}）",
                if code == Some(0) && signal.is_none() {
                    "已"
                } else {
                    "异常"
                }
            ),
        );
        if code == Some(127) {
            log(&app, &id, format!("[Bento] 状态码 127 通常表示找不到命令，请确认「{}」在项目终端中能直接运行", project.start_command));
        }
    });
    Ok(())
}

// Keep captured trees visible to synchronous quit cleanup even while an async stop is in flight.
pub async fn stop_managed(
    app: &AppHandle,
    id: &str,
    state: &ProcessMap,
    required: bool,
) -> Result<(), String> {
    let info = state.lock().await.managed.get(id).cloned();
    let Some(info) = info else {
        state.lock().await.clear_failure(id);
        return if required {
            Err("该项目不是由 Bento 启动的，请确认外部进程信息后再结束".into())
        } else {
            Ok(())
        };
    };
    let root = info.root();
    let trees = tauri::async_runtime::spawn_blocking(move || processes::capture_trees(&[root]))
        .await
        .map_err(|err| format!("读取进程树失败：{err}"))?;
    {
        let mut data = state.lock().await;
        if data.owns(id, info.pid) {
            data.managed.remove(id);
        }
        for tree in &trees {
            data.stopping.insert(info.pid, (id.into(), tree.clone()));
        }
    }
    crate::tray::refresh(app);
    log(
        app,
        id,
        format!(
            "[Bento] 正在停止进程树 (PID: {}, PGID: {})…",
            info.pid, info.pgid
        ),
    );
    tauri::async_runtime::spawn_blocking(move || processes::terminate_trees_blocking(&trees))
        .await
        .map_err(|err| format!("停止进程失败：{err}"))?;
    let remaining = if let Some(port) = info.port {
        tauri::async_runtime::spawn_blocking(move || processes::port_pids(port))
            .await
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    {
        let mut data = state.lock().await;
        data.stopping.remove(&info.pid);
        data.clear_failure(id);
        emit_status(app, &data, id, info.port);
    }
    if let Some(port) = info.port.filter(|_| !remaining.is_empty()) {
        log(
            app,
            id,
            format!(
                "[Bento] 端口 {port} 仍被外部进程 PID {} 占用",
                remaining
                    .iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join("、")
            ),
        );
    }
    log(app, id, "[Bento] 托管进程已停止".into());
    Ok(())
}

#[tauri::command]
pub async fn stop_project(
    app: AppHandle,
    id: String,
    state: State<'_, ProcessMap>,
) -> Result<(), String> {
    let operation = operation(&state, &id).await;
    let _guard = operation.lock().await;
    stop_managed(&app, &id, &state, true).await
}

pub async fn stop_all(app: &AppHandle, state: &ProcessMap) -> Result<Vec<String>, String> {
    let selected = state.lock().await.managed_snapshot();
    let mut tasks = tokio::task::JoinSet::new();
    for project in selected {
        let app = app.clone();
        let state = state.clone();
        tasks.spawn(async move {
            let operation = operation(&state, &project.id).await;
            let _guard = operation.lock().await;
            if !state.lock().await.managed.contains_key(&project.id) {
                return Ok(None);
            }
            stop_managed(&app, &project.id, &state, false).await?;
            Ok::<_, String>(Some(project.id))
        });
    }
    let mut stopped = Vec::new();
    let mut errors = Vec::new();
    while let Some(result) = tasks.join_next().await {
        match result {
            Ok(Ok(Some(id))) => stopped.push(id),
            Ok(Ok(None)) => {}
            Ok(Err(error)) => errors.push(error),
            Err(error) => errors.push(error.to_string()),
        }
    }
    if !errors.is_empty() {
        return Err(format!("部分项目停止失败：{}", errors.join("；")));
    }
    stopped.sort();
    Ok(stopped)
}

#[tauri::command]
pub async fn stop_all_projects(
    app: AppHandle,
    state: State<'_, ProcessMap>,
) -> Result<Vec<String>, String> {
    stop_all(&app, state.inner()).await
}

#[tauri::command]
pub async fn restart_project(
    app: AppHandle,
    id: String,
    state: State<'_, ProcessMap>,
) -> Result<(), String> {
    let operation = operation(&state, &id).await;
    let _guard = operation.lock().await;
    stop_managed(&app, &id, &state, true).await?;
    start(&app, &id, &state).await
}

#[tauri::command]
pub async fn get_port_owners(id: String) -> Result<Vec<processes::PortOwner>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let project = get_projects_sync()?
            .projects
            .into_iter()
            .find(|p| p.id == id)
            .ok_or("未找到项目")?;
        Ok(project
            .port
            .map(|port| processes::port_owners(port, &project.path))
            .unwrap_or_default())
    })
    .await
    .map_err(|err| format!("读取端口进程失败：{err}"))?
}

#[tauri::command]
pub async fn stop_external_process(
    app: AppHandle,
    id: String,
    pids: Vec<u32>,
    state: State<'_, ProcessMap>,
) -> Result<(), String> {
    let operation = operation(&state, &id).await;
    let _guard = operation.lock().await;
    if state.lock().await.managed.contains_key(&id) {
        return Err("该项目现已由 Bento 管理，请刷新后停止".into());
    }
    let project_id = id.clone();
    let port = tauri::async_runtime::spawn_blocking(move || {
        get_projects_sync()?
            .projects
            .into_iter()
            .find(|p| p.id == project_id)
            .ok_or_else(|| "未找到项目".to_string())
            .map(|p| p.port)
    })
    .await
    .map_err(|err| err.to_string())??;
    if let Some(port) = port {
        let messages =
            tauri::async_runtime::spawn_blocking(move || processes::stop_external(port, pids))
                .await
                .map_err(|err| format!("结束外部进程失败：{err}"))?;
        for message in messages {
            log(&app, &id, message);
        }
    }
    let mut data = state.lock().await;
    data.clear_failure(&id);
    emit_status(&app, &data, &id, port);
    Ok(())
}

pub fn shutdown(state: &ProcessMap, mut terminal_roots: Vec<processes::ManagedRoot>) {
    SHUTTING_DOWN.store(true, Ordering::SeqCst);
    let deadline = Instant::now() + Duration::from_millis(500);
    let (roots, mut trees) = loop {
        if let Ok(mut data) = state.try_lock() {
            let roots: Vec<_> = data.managed.drain().map(|(_, info)| info.root()).collect();
            let trees: Vec<_> = data.stopping.drain().map(|(_, (_, tree))| tree).collect();
            break (roots, trees);
        }
        if Instant::now() >= deadline {
            eprintln!("Bento quit: process state lock timed out");
            break (Vec::new(), Vec::new());
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    terminal_roots.extend(roots);
    trees.extend(processes::capture_trees(&terminal_roots));
    processes::terminate_trees_blocking(&trees);
}

#[cfg(test)]
mod tests {
    use super::*;
    fn info(pid: u32, phase: Phase) -> ProcessInfo {
        ProcessInfo {
            pid,
            pgid: pid as i32,
            port: Some(5301),
            phase,
            identity: None,
        }
    }
    #[test]
    fn stop_all_selects_only_managed_starting_and_running_projects() {
        let mut state = RuntimeState::default();
        state
            .managed
            .insert("starting".into(), info(10, Phase::Starting));
        state
            .managed
            .insert("running".into(), info(20, Phase::Running));
        state.failures.insert(
            "crashed".into(),
            ExitFailure {
                code: Some(3),
                signal: None,
            },
        );
        assert!(
            state
                .status("external", Some(5302), (true, Some(30)))
                .external
        );
        let selected = state.managed_snapshot();
        assert_eq!(
            selected
                .iter()
                .map(|project| project.id.as_str())
                .collect::<Vec<_>>(),
            ["running", "starting"]
        );
        assert_eq!(selected[0].phase, Phase::Running);
        assert_eq!(selected[1].phase, Phase::Starting);
        state.managed.clear();
        assert!(state.managed_snapshot().is_empty());
    }
    #[test]
    fn runtime_rules_and_failure_clearing() {
        let mut state = RuntimeState::default();
        assert_eq!(state.status("p", None, (false, None)).status, "stopped");
        state.managed.insert("p".into(), info(10, Phase::Starting));
        assert_eq!(
            state.status("p", Some(5301), (true, Some(11))).status,
            "starting"
        );
        state.managed.get_mut("p").unwrap().phase = Phase::Running;
        assert_eq!(
            state.status("p", Some(5301), (false, None)).status,
            "running"
        );
        assert!(state.exited("p", 10, Some(3), None));
        for _ in 0..3 {
            assert_eq!(state.status("p", None, (false, None)).exit_code, Some(3));
        }
        let external = state.status("p", Some(5301), (true, Some(20)));
        assert_eq!(external.status, "running");
        assert!(external.external && !external.is_managed);
        assert_eq!(state.status("p", None, (false, None)).status, "error");
        state.clear_failure("p");
        assert_eq!(state.status("p", None, (false, None)).status, "stopped");
        state.managed.insert("p".into(), info(30, Phase::Running));
        state.exited("p", 30, None, Some(9));
        assert_eq!(state.status("p", None, (false, None)).exit_signal, Some(9));
    }
    #[test]
    fn old_watcher_cannot_remove_or_emit_for_new_pid() {
        let mut state = RuntimeState::default();
        state.managed.insert("p".into(), info(20, Phase::Starting));
        assert!(!state.exited("p", 10, Some(3), None));
        assert!(state.owns("p", 20));
        assert!(state.failures.is_empty());
        assert_eq!(state.sequence, 0);
        state.managed.remove("p");
        assert!(!state.exited("p", 20, None, Some(15)));
        state.managed.insert("p".into(), info(30, Phase::Running));
        assert!(state.exited("p", 30, Some(0), None));
        assert_eq!(state.status("p", None, (false, None)).status, "stopped");
    }
}
