use crate::{
    dashboard, get_projects_sync, logs, processes, projects, shell_env, LogPayload, ProjectConfig,
    RunningProjectInfo,
};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, Read};
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
    PortTimeout,
}

#[derive(Clone)]
pub struct ManagedProject {
    pub id: String,
    pub port: Option<u16>,
    pub phase: Phase,
    pub started_at: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct ActualPorts {
    primary: Option<u16>,
    services: HashMap<String, u16>,
}

#[derive(Clone)]
pub struct ProcessInfo {
    pub pid: u32,
    pgid: i32,
    port: Option<u16>,
    phase: Phase,
    identity: Option<processes::ProcessIdentity>,
    services: Vec<projects::Service>,
    inject_ports: bool,
    tree: Option<Arc<processes::TrackedTree>>,
    actual_ports: ActualPorts,
    pub run: dashboard::Run,
}
impl ProcessInfo {
    pub fn root(&self) -> processes::ManagedRoot {
        processes::ManagedRoot {
            pid: self.pid,
            pgid: self.pgid,
            identity: self.identity,
        }
    }

    pub fn pids(&self) -> HashSet<u32> {
        match &self.tree {
            Some(tree) => tree
                .capture(
                    &processes::process_table(),
                    pids_on_ports(self.configured_ports(), &processes::listening_ports()),
                )
                .pids()
                .collect(),
            None => processes::managed_pids(&self.root()),
        }
    }

    pub fn metrics_pids(&self, rows: &[processes::ProcessRow], listeners: &HashMap<u16, Vec<u32>>) -> HashSet<u32> {
        let tree = match &self.tree {
            Some(tree) => tree.capture(rows, pids_on_ports(self.configured_ports(), listeners)),
            None => Arc::new(processes::ProcessTree::for_root(&self.root(), rows)),
        };
        tree.pids().collect()
    }

    pub fn configured_ports(&self) -> impl Iterator<Item = u16> + '_ {
        self.port
            .into_iter()
            .chain(self.services.iter().filter_map(|service| service.port))
    }

    pub fn checks_injected_port(&self, id: &str, port: u16) -> bool {
        self.inject_ports && if self.services.is_empty() {
            id == "legacy:root" && self.port == Some(port)
        } else {
            self.services.iter().any(|service| service.id == id && service.port == Some(port))
        }
    }

    pub fn actual_service_port(&self, id: &str) -> Option<u16> {
        if self.services.is_empty() && id == "legacy:root" {
            self.actual_ports.primary
        } else {
            self.actual_ports.services.get(id).copied()
        }
    }

    fn observe_ports(&mut self, actual_ports: ActualPorts, timed_out: bool) -> bool {
        let phase = if actual_ports.primary.is_some() || self.configured_ports().next().is_none() {
            Phase::Running
        } else if timed_out && self.phase == Phase::Starting {
            Phase::PortTimeout
        } else {
            self.phase
        };
        let changed = self.phase != phase || self.actual_ports != actual_ports;
        self.phase = phase;
        self.actual_ports = actual_ports;
        changed
    }
}

fn pids_on_ports(
    ports: impl IntoIterator<Item = u16>,
    listeners: &HashMap<u16, Vec<u32>>,
) -> Vec<u32> {
    ports
        .into_iter()
        .filter_map(|port| listeners.get(&port))
        .flatten()
        .copied()
        .collect()
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
                port: info.actual_ports.primary,
                phase: info.phase,
                started_at: info.run.started_at,
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
    actual_port: Option<u16>,
    status: String,
    is_port_in_use: bool,
    is_managed: bool,
    external: bool,
    pid: Option<u32>,
    exit_code: Option<i32>,
    exit_signal: Option<i32>,
    started_at: Option<u64>,
    revision: u64,
    services: Vec<ServiceRuntimeStatus>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ServiceRuntimeStatus {
    service_id: String,
    port: Option<u16>,
    actual_port: Option<u16>,
    listening: bool,
    owner: Option<ServicePortOwner>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ServicePortOwner {
    pid: Option<u32>,
    in_project: bool,
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
        match info.phase {
            Phase::Starting => "starting",
            Phase::Running => "running",
            Phase::PortTimeout => "error",
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
        actual_port: managed.map_or_else(|| listener.0.then_some(port).flatten(), |info| info.actual_ports.primary),
        status: status.into(),
        is_port_in_use: managed.map_or(listener.0, |info| info.actual_ports.primary.is_some()),
        is_managed: managed.is_some(),
        external: managed.is_none() && listener.0,
        pid: managed.map(|info| info.pid).or(listener.1),
        exit_code: failure.and_then(|exit| exit.code),
        exit_signal: failure.and_then(|exit| exit.signal),
        started_at: managed.map(|info| info.run.started_at),
        revision,
        services: Vec::new(),
    }
}

fn service_status(
    service: &projects::Service,
    actual_port: Option<u16>,
    listeners: &HashMap<u16, Vec<u32>>,
    owns: impl Fn(u32) -> bool,
) -> (ServiceRuntimeStatus, Option<u32>) {
    let mut state = ServiceRuntimeStatus {
        service_id: service.id.clone(),
        port: service.port,
        actual_port,
        listening: false,
        owner: None,
    };
    let Some(port) = actual_port.or(service.port) else {
        return (state, None);
    };
    let pids = listeners.get(&port).map_or(&[][..], Vec::as_slice);
    let mut owned = None;
    let mut foreign = None;
    for &pid in pids {
        if owns(pid) {
            owned.get_or_insert(pid);
        } else {
            foreign.get_or_insert(pid);
        }
    }
    if let Some(pid) = foreign.or(owned) {
        state.owner = Some(ServicePortOwner {
            pid: Some(pid),
            in_project: foreign.is_none(),
        });
    } else if processes::probe_loopback(port) {
        state.owner = Some(ServicePortOwner {
            pid: None,
            in_project: false,
        });
    }
    state.listening = state.owner.is_some();
    state.actual_port = state.listening.then_some(port);
    (state, owned)
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
fn emit_status(
    app: &AppHandle,
    data: &RuntimeState,
    id: &str,
    port: Option<u16>,
    services: &[projects::Service],
) {
    let mut status = data.status(id, port, (false, None));
    let info = data.managed.get(id);
    status.services = services
        .iter()
        .map(|service| {
            let actual_port = info.and_then(|info| info.actual_service_port(&service.id));
            ServiceRuntimeStatus {
                service_id: service.id.clone(),
                port: service.port,
                actual_port,
                listening: actual_port.is_some(),
                owner: None,
            }
        })
        .collect();
    if !status.services.is_empty() {
        status.is_port_in_use = status.services.iter().any(|service| service.listening) || status.actual_port.is_some();
    }
    let _ = app.emit("project-status", status);
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
            port: info.actual_ports.primary,
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
        let mut managed_by_pid = HashMap::new();
        if projects.iter().any(|project| !project.services.is_empty()) && !managed.is_empty() {
            let rows = processes::process_table();
            for (id, info) in &managed {
                let pids: Vec<_> = if let Some(tree) = &info.tree {
                    tree.capture(&rows, pids_on_ports(info.configured_ports(), &listeners))
                        .pids()
                        .collect()
                } else {
                    processes::managed_pids(&info.root()).into_iter().collect()
                };
                managed_by_pid.extend(pids.into_iter().map(|pid| (pid, id.as_str())));
            }
        }
        Ok(projects
            .into_iter()
            .map(|project| {
                let info = managed.get(&project.id);
                let revision = *revisions.get(&project.id).unwrap_or(&0);
                let failure = failures.get(&project.id);
                if !project.services.is_empty() {
                    let path = std::path::PathBuf::from(crate::expand_tilde(&project.path))
                        .canonicalize()
                        .ok();
                    let mut owned_pid = None;
                    let services: Vec<_> = project
                        .services
                        .iter()
                        .map(|service| {
                            let actual = info.and_then(|info| info.actual_service_port(&service.id));
                            let (status, owned) = service_status(service, actual, &listeners, |pid| {
                                if let Some(owner) = managed_by_pid.get(&pid) {
                                    return *owner == project.id;
                                }
                                info.is_none()
                                    && path
                                        .as_ref()
                                        .is_some_and(|path| processes::cwd_matches(pid, path))
                            });
                            owned_pid = owned_pid.or(owned);
                            status
                        })
                        .collect();
                    let root_alive = info.is_some_and(|info| {
                        info.identity.is_some() && processes::identity(info.pid) == info.identity
                    });
                    let mut status = compute_status(
                        &project.id,
                        project.port,
                        info,
                        failure,
                        (owned_pid.is_some(), owned_pid),
                        revision,
                    );
                    if !root_alive {
                        status.pid = owned_pid.or(status.pid);
                    }
                    if info.is_none() {
                        status.actual_port = services.iter().filter_map(|service| service.actual_port)
                            .find(|port| Some(*port) == project.port)
                            .or_else(|| services.iter().filter_map(|service| service.actual_port).min());
                    }
                    status.is_port_in_use = services.iter().any(|service| service.listening) || status.actual_port.is_some();
                    status.services = services;
                    return status;
                }
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
                compute_status(&project.id, project.port, info, failure, listener, revision)
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

enum ProjectChild {
    Pipes(tokio::process::Child),
    Pty(
        Box<dyn portable_pty::Child + Send + Sync>,
        Box<dyn Read + Send>,
    ),
}

fn spawn_project_child(
    launch_marker: Option<&str>,
    path: &str,
    script: &str,
    environment: Option<HashMap<String, String>>,
) -> Result<(ProjectChild, u32), String> {
    let args = [if environment.is_some() { "-fc" } else { "-lc" }, script];
    if let Some(marker) = launch_marker {
        let pair = native_pty_system()
            .openpty(PtySize::default())
            .map_err(|err| format!("创建项目 PTY 失败：{err}"))?;
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|err| format!("读取项目 PTY 失败：{err}"))?;
        let mut command = CommandBuilder::new("/bin/zsh");
        command.args(args);
        command.cwd(path);
        if let Some(env) = environment {
            command.env_clear();
            for (key, value) in env {
                command.env(key, value);
            }
        }
        crate::terminal::configure_environment(&mut command);
        command.env(processes::MANAGED_RUN_ENV, marker);
        let child = pair
            .slave
            .spawn_command(command)
            .map_err(|err| format!("启动项目 PTY 失败：{err}"))?;
        let pid = child.process_id().ok_or("无法获取项目 PTY 的 PID")?;
        Ok((ProjectChild::Pty(child, reader), pid))
    } else {
        let mut command = std::process::Command::new("zsh");
        command.args(args);
        if let Some(env) = environment {
            command.env_clear().envs(env);
        } else {
            for (key, _) in std::env::vars_os() {
                if key.to_str().is_some_and(shell_env::should_strip) {
                    command.env_remove(key);
                }
            }
        }
        command
            .process_group(0)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let child = tokio::process::Command::from(command)
            .spawn()
            .map_err(|err| format!("启动进程失败：{err}"))?;
        let pid = child.id().ok_or("无法获取子进程 PID")?;
        Ok((ProjectChild::Pipes(child), pid))
    }
}

impl ProjectChild {
    async fn wait(
        self,
        sender: tokio::sync::mpsc::Sender<logs::Entry>,
    ) -> (Option<i32>, Option<i32>) {
        match self {
            Self::Pty(mut child, reader) => {
                tauri::async_runtime::spawn_blocking(move || {
                    let mut reader = std::io::BufReader::new(reader);
                    let mut line = Vec::new();
                    while let Ok(read) = reader.read_until(b'\n', &mut line) {
                        if read == 0 {
                            break;
                        }
                        let text = String::from_utf8_lossy(&line)
                            .trim_end_matches(['\r', '\n'])
                            .to_owned();
                        if sender
                            .blocking_send(logs::Entry {
                                text,
                                stream: "stdout".into(),
                            })
                            .is_err()
                        {
                            break;
                        }
                        line.clear();
                    }
                });
                tauri::async_runtime::spawn_blocking(move || {
                    let status = child.wait().ok()?;
                    let signal = status.signal().and_then(|name| {
                        (1..32).find(|&number| {
                            let description = unsafe { libc::strsignal(number) };
                            !description.is_null()
                                && unsafe { std::ffi::CStr::from_ptr(description) }.to_bytes()
                                    == name.as_bytes()
                        })
                    });
                    Some((
                        status
                            .signal()
                            .is_none()
                            .then_some(status.exit_code() as i32),
                        signal,
                    ))
                })
                .await
                .ok()
                .flatten()
                .unwrap_or_default()
            }
            Self::Pipes(mut child) => {
                for (stream, reader) in [
                    (
                        "stdout",
                        child.stdout.take().map(|read| {
                            Box::new(read) as Box<dyn tokio::io::AsyncRead + Send + Unpin>
                        }),
                    ),
                    (
                        "stderr",
                        child.stderr.take().map(|read| {
                            Box::new(read) as Box<dyn tokio::io::AsyncRead + Send + Unpin>
                        }),
                    ),
                ] {
                    let Some(reader) = reader else { continue };
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
                drop(sender);
                let status = child.wait().await.ok();
                (
                    status.and_then(|status| status.code()),
                    status.and_then(|status| status.signal()),
                )
            }
        }
    }
}

async fn start(app: &AppHandle, id: &str, state: &ProcessMap) -> Result<(), String> {
    if state.lock().await.managed.contains_key(id) {
        return Err("项目已经在运行中".into());
    }
    let id_owned = id.to_string();
    let (project, plan) = tauri::async_runtime::spawn_blocking(move || {
        let response = get_projects_sync()?;
        let project = response
            .projects
            .into_iter()
            .find(|p| p.id == id_owned)
            .ok_or("未找到项目配置")?;
        if !project.runnable {
            return Err("此项目无需运行，如需启动请在编辑配置中勾选「需要运行」".into());
        }
        if response
            .conflicts
            .iter()
            .any(|conflict| conflict.project_ids.contains(&id_owned))
        {
            return Err("项目端口存在重复冲突，请更换端口后再启动".into());
        }
        let ports: Vec<_> = project.ports().collect();
        if !ports.is_empty() {
            let listeners = processes::listening_ports();
            for port in ports {
                let pid = listeners.get(&port).and_then(|pids| pids.first());
                if pid.is_some() || processes::probe_loopback(port) {
                    let pid = pid.map(|pid| format!("（PID: {pid}）")).unwrap_or_default();
                    return Err(format!(
                        "端口 {port} 当前已被占用{pid}，请先确认并结束外部进程"
                    ));
                }
            }
        }
        let plan = projects::launch_plan((&project).into())?;
        Ok((project, plan))
    })
    .await
    .map_err(|err| format!("启动项目失败：{err}"))??;
    let path = crate::expand_tilde(&project.path);
    if !std::path::Path::new(&path).is_dir() {
        return Err(format!("项目目录不存在：{}", project.path));
    }
    let _ = app.emit("project-run-started", id);
    log(app, id, format!("[Bento] 正在启动项目: {}", project.name));
    log(app, id, format!("[Bento] 工作目录: {path}"));
    for (dir, command) in plan.commands() {
        log(app, id, format!("[Bento] 执行命令（{dir}）: {command}"));
    }
    let script = plan.script()?;
    let environment = tauri::async_runtime::spawn_blocking(shell_env::capture)
        .await
        .unwrap_or_else(|err| Err(format!("读取环境任务失败：{err}")));
    let environment = match environment {
        Ok(env) => Some(env),
        Err(reason) => {
            log(app, id, format!("[Bento] 读取终端环境失败（{reason}），已改用登录 shell 启动，.zshrc 中的设置可能不会生效"));
            None
        }
    };
    let multi_service = !project.services.is_empty();
    let marker = multi_service.then(processes::new_launch_marker);
    let run = dashboard::Run::new();
    let (child, pid, tree) = {
        let mut data = state.lock().await;
        if SHUTTING_DOWN.load(Ordering::SeqCst) {
            return Err("Bento 正在退出".into());
        }
        data.clear_failure(id);
        let (child, pid) = spawn_project_child(marker.as_deref(), &path, &script, environment)?;
        let root = processes::ManagedRoot {
            pid,
            pgid: pid as i32,
            identity: processes::identity(pid),
        };
        let tree = marker.map(|marker| Arc::new(processes::TrackedTree::new(root.clone(), marker)));
        data.managed.insert(
            id.into(),
            ProcessInfo {
                pid,
                pgid: pid as i32,
                port: project.port,
                phase: if project.ports().next().is_some() {
                    Phase::Starting
                } else {
                    Phase::Running
                },
                identity: root.identity,
                services: project.services.clone(),
                inject_ports: project.inject_ports,
                tree: tree.clone(),
                actual_ports: ActualPorts::default(),
                run: run.clone(),
            },
        );
        emit_status(app, &data, id, project.port, &project.services);
        (child, pid, tree)
    };
    let (sender, receiver) = tokio::sync::mpsc::channel(2048);
    let (finish_logs, finished) = tokio::sync::oneshot::channel();
    let log_app = app.clone();
    let log_id = id.to_string();
    let log_run = run.clone();
    let mut delivery =
        tauri::async_runtime::spawn(logs::aggregate(receiver, finished, move |entries| {
            log_run.record_logs(&entries);
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
    watch_resources(state.clone(), id.into(), pid, project.clone());
    watch_ports(app.clone(), state.clone(), id.into(), pid, project.clone());
    let app = app.clone();
    let state = state.clone();
    let id = id.to_string();
    tauri::async_runtime::spawn(async move {
        let (code, signal) = child.wait(sender).await;
        run.exit(code, signal);
        if let Some(tree) = tree {
            let ports: Vec<_> = project.ports().collect();
            let grace = Instant::now()
                + if code == Some(0) {
                    Duration::from_millis(500)
                } else {
                    Duration::ZERO
                };
            while state.lock().await.owns(&id, pid) {
                let tree = tree.clone();
                let ports = ports.clone();
                let descendants_alive = tauri::async_runtime::spawn_blocking(move || {
                    let listeners = if ports.is_empty() {
                        Vec::new()
                    } else {
                        pids_on_ports(ports, &processes::listening_ports())
                    };
                    tree.capture(&processes::process_table(), listeners)
                        .pids()
                        .next()
                        .is_some()
                })
                .await
                .unwrap_or(false);
                if !descendants_alive && Instant::now() >= grace {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
        if tokio::time::timeout(Duration::from_secs(1), &mut delivery)
            .await
            .is_err()
        {
            let _ = finish_logs.send(());
            let _ = delivery.await;
        }
        run.finish_logs();
        {
            let mut data = state.lock().await;
            if !data.exited(&id, pid, code, signal) {
                return;
            }
            emit_status(&app, &data, &id, project.port, &project.services);
        }
        persist_run(&app, &id, run, project.ports().collect(), false).await;
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
            log(
                &app,
                &id,
                format!(
                    "[Bento] 状态码 127 通常表示找不到命令，请确认「{}」在项目终端中能直接运行",
                    project.start_command
                ),
            );
        }
    });
    Ok(())
}

fn watch_resources(state: ProcessMap, id: String, pid: u32, project: ProjectConfig) {
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(5));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            let info = state.lock().await.managed.get(&id).filter(|info| info.pid == pid).cloned();
            let Some(info) = info else { break; };
            let project = project.clone();
            let _ = tauri::async_runtime::spawn_blocking(move || dashboard::observe_managed(&project, &info)).await;
        }
    });
}

async fn persist_run(app: &AppHandle, id: &str, run: dashboard::Run, ports: Vec<u16>, stopped: bool) {
    let key = id.to_owned();
    let result = tauri::async_runtime::spawn_blocking(move || dashboard::save_run(&key, &run, &ports, stopped)).await;
    match result {
        Ok(Ok(())) => { let _ = app.emit("dashboard-run-saved", id); }
        Ok(Err(error)) => eprintln!("保存上次运行失败 ({id}): {error}"),
        Err(error) => eprintln!("保存上次运行任务失败 ({id}): {error}"),
    }
}

fn preferred_listener(ports: &[u16], mut http: impl FnMut(u16) -> bool) -> Option<u16> {
    let mut ports = ports.iter().copied().collect::<Vec<_>>();
    ports.sort_unstable();
    ports.dedup();
    if ports.len() > 1 {
        if let Some(port) = ports.iter().copied().find(|port| http(*port)) {
            return Some(port);
        }
    }
    ports.first().copied()
}

fn detect_actual_ports(info: &ProcessInfo, project_dir: &std::path::Path) -> ActualPorts {
    let listeners = processes::listening_ports();
    let owned = info.metrics_pids(&processes::process_table(), &listeners);
    let configured = |port: u16| {
        listeners.get(&port).map_or_else(
            || processes::probe_loopback(port),
            |pids| pids.iter().any(|pid| owned.contains(pid)),
        )
    };
    let candidates: Vec<_> = listeners.iter().filter(|(_, pids)| pids.iter().any(|pid| owned.contains(pid)))
        .map(|(&port, _)| port).collect();
    let mut http = HashMap::new();
    let mut preferred = |ports: &[u16]| preferred_listener(ports, |port| *http.entry(port)
        .or_insert_with(|| processes::speaks_http(port)));
    let mut services = HashMap::new();
    for service in &info.services {
        if let Some(port) = service.port.filter(|port| configured(*port)) {
            services.insert(service.id.clone(), port);
        }
    }
    let claimed: HashSet<_> = services.values().copied().collect();
    let remaining: Vec<_> = info.services.iter().filter(|service| !services.contains_key(&service.id))
        .map(|service| (service, project_dir.join(&service.dir).canonicalize().ok())).collect();
    let mut per_service: HashMap<&str, Vec<u16>> = HashMap::new();
    for &port in &candidates {
        if claimed.contains(&port) { continue; }
        let pids = listeners.get(&port).into_iter().flatten().filter(|pid| owned.contains(pid));
        let matches = pids.flat_map(|&pid| remaining.iter().filter_map(move |(service, path)| {
            let path = path.as_ref()?;
            processes::cwd_matches(pid, path).then_some((service.id.as_str(), path.components().count()))
        }));
        let service = matches.max_by_key(|(_, depth)| *depth).map(|(id, _)| id)
            .or_else(|| (remaining.len() == 1).then(|| remaining[0].0.id.as_str()));
        if let Some(id) = service {
            per_service.entry(id).or_default().push(port);
        }
    }
    for (id, ports) in per_service {
        if let Some(port) = preferred(&ports) { services.insert(id.into(), port); }
    }
    let primary = info.port.filter(|port| configured(*port)).or_else(|| preferred(&candidates))
        .or_else(|| services.values().copied().min());
    ActualPorts { primary, services }
}

fn browser_url(port: u16) -> String {
    format!("http://localhost:{port}")
}

fn watch_ports(app: AppHandle, state: ProcessMap, id: String, pid: u32, project: ProjectConfig) {
    let auto_open = project.auto_open && project.project_type.as_deref() == Some("web");
    let project_dir = std::path::PathBuf::from(crate::expand_tilde(&project.path));
    tauri::async_runtime::spawn(async move {
        let started = Instant::now();
        let mut opened = false;
        loop {
            let info = state.lock().await.managed.get(&id).filter(|info| info.pid == pid).cloned();
            let Some(info) = info else { break; };
            let directory = project_dir.clone();
            let Ok(actual) = tauri::async_runtime::spawn_blocking(move || detect_actual_ports(&info, &directory)).await else { break; };
            let mut data = state.lock().await;
            let Some(info) = data.managed.get_mut(&id).filter(|info| info.pid == pid) else { break; };
            let old_phase = info.phase;
            let old_port = info.actual_ports.primary;
            let changed = info.observe_ports(actual, started.elapsed() >= Duration::from_secs(60));
            let phase = info.phase;
            let ready = info.actual_ports.primary;
            if changed {
                data.changed(&id);
                emit_status(&app, &data, &id, project.port, &project.services);
            }
            if old_phase != Phase::PortTimeout && phase == Phase::PortTimeout {
                log(&app, &id, "[Bento] 等待监听端口超时，进程仍在运行。可停止或重启项目".into());
            }
            if let Some(port) = ready.filter(|port| Some(*port) != old_port) {
                log(&app, &id, format!("[Bento] 端口 {port} 已就绪"));
                // Keep the PID match valid through auto-open.
                if auto_open && !opened {
                    opened = true;
                    if let Err(error) = crate::open_url(browser_url(port)).await {
                        log(&app, &id, format!("[Bento] 自动打开网页失败：{error}"));
                    }
                }
            }
            drop(data);
            tokio::time::sleep(if phase == Phase::Starting { Duration::from_millis(450) } else { Duration::from_secs(3) }).await;
        }
    });
}

fn verify_stopped_ports(
    ports: &HashSet<u16>,
    trees: &[Arc<processes::ProcessTree>],
    tracked: Option<&processes::TrackedTree>,
) -> (bool, Vec<String>) {
    if ports.is_empty() {
        return (true, Vec::new());
    }
    let deadline = Instant::now() + Duration::from_millis(600);
    let mut killed = HashSet::new();
    let mut messages = Vec::new();
    loop {
        let listeners = processes::listening_ports();
        let tracked = tracked.map(|tree| {
            tree.capture(
                &processes::process_table(),
                pids_on_ports(ports.iter().copied(), &listeners),
            )
        });
        let mut remaining_owned = false;
        let mut external = Vec::new();
        for &port in ports {
            let mut foreign = Vec::new();
            for &pid in listeners.get(&port).into_iter().flatten() {
                let tree = tracked.iter().chain(trees).find(|tree| tree.owns(pid));
                if let Some(tree) = tree {
                    remaining_owned = true;
                    if tree.kill_listener(pid) && killed.insert((port, pid)) {
                        messages.push(format!(
                            "[Bento] 已补杀仍在监听端口 {port} 的项目子进程 PID {pid}"
                        ));
                    }
                } else {
                    foreign.push(pid.to_string());
                }
            }
            if !foreign.is_empty() {
                external.push(format!(
                    "[Bento] 端口 {port} 仍被外部进程 PID {} 占用",
                    foreign.join("、")
                ));
            } else if !listeners.contains_key(&port) && processes::probe_loopback(port) {
                external.push(format!("[Bento] 端口 {port} 仍有监听，无法确认进程归属"));
            }
        }
        if Instant::now() >= deadline
            && (!remaining_owned || Instant::now() >= deadline + Duration::from_millis(600))
        {
            messages.extend(external);
            return (!remaining_owned, messages);
        }
        std::thread::sleep(Duration::from_millis(60));
    }
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
    let tracked = info.tree.clone();
    let root = info.root();
    let sampled = info.clone();
    let mut ports: HashSet<_> = info.configured_ports().collect();
    let project_id = id.to_owned();
    let (trees, ports) = tauri::async_runtime::spawn_blocking(move || {
        // Include edited ports as well as those used for this launch.
        if let Ok(response) = get_projects_sync() {
            if let Some(project) = response
                .projects
                .iter()
                .find(|project| project.id == project_id)
            {
                ports.extend(project.ports());
                dashboard::observe_managed(project, &sampled);
            }
        }
        let trees = match tracked {
            Some(tree) => vec![tree.capture(
                &processes::process_table(),
                pids_on_ports(ports.iter().copied(), &processes::listening_ports()),
            )],
            None => processes::capture_trees(&[root]),
        };
        (trees, ports)
    })
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
    let tracked = info.tree.clone();
    let stopped = tauri::async_runtime::spawn_blocking(move || {
        processes::terminate_trees_blocking(&trees);
        verify_stopped_ports(&ports, &trees, tracked.as_deref())
    })
    .await
    .map_err(|err| format!("停止进程失败：{err}"));
    let (released, messages) = stopped.unwrap_or_else(|error| (false, vec![error]));
    if released {
        let mut ports: Vec<_> = info.configured_ports().collect();
        ports.sort_unstable();
        ports.dedup();
        info.run.wait_logs().await;
        persist_run(app, id, info.run.clone(), ports, true).await;
    }
    {
        let mut data = state.lock().await;
        data.stopping.remove(&info.pid);
        if !released {
            data.managed.insert(id.into(), info.clone());
        }
        data.clear_failure(id);
        emit_status(app, &data, id, info.port, &info.services);
    }
    for message in messages {
        log(app, id, message);
    }
    if !released {
        return Err("仍有项目子进程监听端口，请再次停止".into());
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
        if project.services.is_empty() {
            return Ok(project
                .port
                .map(|port| processes::port_owners(port, &project.path))
                .unwrap_or_default());
        }
        let listeners = processes::listening_ports();
        let mut pids: Vec<_> = project
            .ports()
            .filter_map(|port| listeners.get(&port))
            .flatten()
            .copied()
            .collect();
        pids.sort_unstable();
        pids.dedup();
        Ok(processes::owners_for_pids(pids, Some(&project.path))
            .into_iter()
            .filter(|owner| owner.in_project)
            .collect())
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
    let (port, services) = tauri::async_runtime::spawn_blocking(move || {
        get_projects_sync()?
            .projects
            .into_iter()
            .find(|p| p.id == project_id)
            .ok_or_else(|| "未找到项目".to_string())
            .map(|p| {
                let mut messages = Vec::new();
                if p.services.is_empty() {
                    if let Some(port) = p.port {
                        messages = processes::stop_external(port, pids);
                    }
                } else {
                    let path = std::path::PathBuf::from(crate::expand_tilde(&p.path))
                        .canonicalize()
                        .ok();
                    let listeners = processes::listening_ports();
                    for port in p.ports() {
                        let selected = pids
                            .iter()
                            .copied()
                            .filter(|pid| {
                                listeners
                                    .get(&port)
                                    .is_some_and(|owners| owners.contains(pid))
                                    && path
                                        .as_ref()
                                        .is_some_and(|path| processes::cwd_matches(*pid, path))
                            })
                            .collect::<Vec<_>>();
                        if !selected.is_empty() {
                            messages.extend(processes::stop_external(port, selected));
                        }
                    }
                }
                (p.port, (p.services, messages))
            })
    })
    .await
    .map_err(|err| err.to_string())??;
    let (services, messages) = services;
    for message in messages {
        log(&app, &id, message);
    }
    let mut data = state.lock().await;
    data.clear_failure(&id);
    emit_status(&app, &data, &id, port, &services);
    Ok(())
}

pub fn shutdown(state: &ProcessMap, mut terminal_roots: Vec<processes::ManagedRoot>) {
    SHUTTING_DOWN.store(true, Ordering::SeqCst);
    let deadline = Instant::now() + Duration::from_millis(500);
    let (mut trees, active) = loop {
        if let Ok(mut data) = state.try_lock() {
            let active: Vec<_> = data.managed.drain().collect();
            let trees: Vec<_> = data.stopping.drain().map(|(_, (_, tree))| tree).collect();
            break (trees, active);
        }
        if Instant::now() >= deadline {
            eprintln!("Bento quit: process state lock timed out");
            break (Vec::new(), Vec::new());
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    terminal_roots.extend(active.iter().filter(|(_, info)| info.tree.is_none()).map(|(_, info)| info.root()));
    trees.extend(processes::capture_trees(&terminal_roots));
    if active.iter().any(|(_, info)| info.tree.is_some()) {
        let rows = processes::process_table();
        let listeners = processes::listening_ports();
        trees.extend(active.iter().filter_map(|(_, info)| {
            info.tree
                .as_ref()
                .map(|tree| tree.capture(&rows, pids_on_ports(info.configured_ports(), &listeners)))
        }));
    }
    processes::terminate_trees_blocking(&trees);
    for (id, info) in active {
        let ports: Vec<_> = info.configured_ports().collect::<HashSet<_>>().into_iter().collect();
        if let Err(error) = dashboard::save_run(&id, &info.run, &ports, true) {
            eprintln!("保存上次运行失败 ({id}): {error}");
        }
    }
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
            services: Vec::new(),
            inject_ports: true,
            tree: None,
            actual_ports: ActualPorts::default(),
            run: dashboard::Run::new(),
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
    fn listener_on_a_different_port_makes_project_ready_and_sets_browser_url() {
        let mut info = info(10, Phase::Starting);
        let primary = preferred_listener(&[5210, 5201, 5199], |port| port > 5200);
        assert_eq!(primary, Some(5201));
        assert!(info.observe_ports(ActualPorts { primary, ..Default::default() }, false));
        let status = compute_status("fixture", Some(5301), Some(&info), None, (false, None), 1);
        assert_eq!(status.status, "running");
        assert_eq!(status.port, Some(5301));
        assert_eq!(status.actual_port.map(browser_url).as_deref(), Some("http://localhost:5201"));
        assert!(status.is_port_in_use);
    }

    #[test]
    fn configured_project_can_recover_from_port_timeout() {
        let mut info = info(10, Phase::Starting);
        assert!(info.observe_ports(ActualPorts::default(), true));
        assert_eq!(info.phase, Phase::PortTimeout);
        assert!(info.observe_ports(ActualPorts { primary: Some(5301), ..Default::default() }, true));
        assert_eq!(info.phase, Phase::Running);
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
