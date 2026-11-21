use crate::lifecycle::{self, ProcessInfo, ProcessMap};
use crate::{logs, open_project_db, processes, projects, ProjectConfig};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::State;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const API_PATHS: &[&str] = &["/api/health", "/health", "/healthz", "/"];
const HEALTH_TIMEOUT: Duration = Duration::from_secs(2);

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub fn migrate(conn: &Connection) -> rusqlite::Result<usize> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS project_last_runs (
             project_id TEXT PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
             started_at INTEGER NOT NULL,
             summary TEXT NOT NULL
         )",
        [],
    )
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Health {
    port: u16,
    path: String,
    code: Option<u16>,
    elapsed_ms: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct LastService {
    port: Option<u16>,
    process: Option<String>,
    health: Option<Health>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RunSummary {
    started_at: u64,
    ended_at: u64,
    exit_code: Option<i32>,
    signal: Option<i32>,
    peak_kib: Option<u64>,
    errors: usize,
    #[serde(default)]
    external: bool,
    services: HashMap<String, LastService>,
    released_ports: Vec<u16>,
}

#[derive(Clone)]
pub struct Run {
    pub started_at: u64,
    data: Arc<Mutex<RunData>>,
    drained: Arc<tokio::sync::Notify>,
    external: bool,
}

#[derive(Default)]
struct RunData {
    peak_kib: Option<u64>,
    errors: usize,
    services: HashMap<String, LastService>,
    exit: Option<(Option<i32>, Option<i32>)>,
    history: HashMap<String, VecDeque<CpuPoint>>,
}

impl Run {
    pub fn new() -> Self {
        Self {
            started_at: now(),
            data: Default::default(),
            drained: Default::default(),
            external: false,
        }
    }

    fn external(started_at: u64) -> Self {
        Self {
            started_at,
            external: true,
            ..Self::new()
        }
    }

    pub fn record_logs(&self, entries: &[logs::Entry]) {
        let mut data = self.data.lock().unwrap();
        data.errors += entries
            .iter()
            .filter(|entry| logs::is_error(&entry.text))
            .count();
    }

    pub fn exit(&self, code: Option<i32>, signal: Option<i32>) {
        self.data.lock().unwrap().exit = Some((code, signal));
    }

    pub fn finish_logs(&self) {
        self.drained.notify_one();
    }

    pub async fn wait_logs(&self) {
        let _ = tokio::time::timeout(Duration::from_millis(1250), self.drained.notified()).await;
    }

    pub fn finish(&self, ports: &[u16], stopped: bool) -> RunSummary {
        let (exit_code, signal, peak_kib, errors, services) = {
            let data = self.data.lock().unwrap();
            let (code, signal) = data
                .exit
                .unwrap_or((None, stopped.then_some(libc::SIGTERM)));
            (
                code,
                signal,
                data.peak_kib,
                data.errors,
                data.services.clone(),
            )
        };
        let ended_at = now();
        let listeners = if ports.is_empty() {
            HashMap::new()
        } else {
            processes::listening_ports()
        };
        let released_ports = ports
            .iter()
            .copied()
            .filter(|port| !listeners.contains_key(port) && !processes::probe_loopback(*port))
            .collect();
        RunSummary {
            started_at: self.started_at,
            ended_at,
            exit_code,
            signal,
            peak_kib,
            errors,
            external: self.external,
            services,
            released_ports,
        }
    }

    fn sample(&self, total: Option<&Usage>, services: &[ServiceSample]) {
        let mut data = self.data.lock().unwrap();
        let sampled_at = now();
        if let Some(total) = total {
            data.peak_kib = Some(
                data.peak_kib
                    .map_or(total.rss_kib, |peak| peak.max(total.rss_kib)),
            );
            remember(&mut data.history, "project", sampled_at, total.cpu);
        }
        for service in services {
            if let Some(usage) = &service.usage {
                let last = data.services.entry(service.id.clone()).or_default();
                let port = service.actual_port.or(service.port);
                if last.port != port {
                    *last = LastService {
                        port,
                        ..Default::default()
                    };
                }
                last.process = usage.process.clone();
                remember(&mut data.history, &service.id, sampled_at, usage.cpu);
            }
        }
    }

    fn record_health(&self, health: &HashMap<String, Health>) {
        let mut data = self.data.lock().unwrap();
        for (id, record) in health {
            let last = data.services.entry(id.clone()).or_default();
            if last.port != Some(record.port) {
                *last = LastService {
                    port: Some(record.port),
                    ..Default::default()
                };
            }
            last.health = Some(record.clone());
        }
    }

    fn history(&self) -> HashMap<String, Vec<CpuPoint>> {
        let minimum = now().saturating_sub(600_000);
        self.data
            .lock()
            .unwrap()
            .history
            .iter()
            .map(|(id, points)| {
                (
                    id.clone(),
                    points
                        .iter()
                        .copied()
                        .filter(|point| point.at >= minimum)
                        .collect(),
                )
            })
            .collect()
    }
}

#[derive(Clone, Copy, Serialize)]
struct CpuPoint {
    at: u64,
    value: f64,
}

fn remember(history: &mut HashMap<String, VecDeque<CpuPoint>>, id: &str, at: u64, value: f64) {
    let points = history.entry(id.into()).or_default();
    if points
        .back()
        .is_some_and(|point| at.saturating_sub(point.at) < 4_500)
    {
        return;
    }
    while points.len() >= 120
        || points
            .front()
            .is_some_and(|point| at.saturating_sub(point.at) > 600_000)
    {
        points.pop_front();
    }
    points.push_back(CpuPoint { at, value });
}

fn save_summary(conn: &Connection, id: &str, summary: &RunSummary) -> Result<(), String> {
    let payload = serde_json::to_string(summary).map_err(|error| error.to_string())?;
    conn.execute(
        "INSERT INTO project_last_runs (project_id, started_at, summary)
         SELECT ?1, ?2, ?3 WHERE EXISTS(SELECT 1 FROM projects WHERE id = ?1)
         ON CONFLICT(project_id) DO UPDATE SET started_at=excluded.started_at, summary=excluded.summary
         WHERE excluded.started_at >= project_last_runs.started_at",
        params![id, summary.started_at, payload],
    ).map(|_| ()).map_err(|error| error.to_string())
}

fn last_run(conn: &Connection, id: &str) -> Result<Option<RunSummary>, String> {
    let summary = conn
        .query_row(
            "SELECT summary FROM project_last_runs WHERE project_id = ?1",
            [id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    summary
        .map(|payload| serde_json::from_str(&payload).map_err(|error| error.to_string()))
        .transpose()
}

pub fn save_run(id: &str, run: &Run, ports: &[u16], stopped: bool) -> Result<(), String> {
    save_summary(&open_project_db()?, id, &run.finish(ports, stopped))
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct Usage {
    cpu: f64,
    rss_kib: u64,
    process: Option<String>,
}

#[derive(Default)]
struct ProcessSample {
    rows: Vec<processes::ProcessRow>,
    usage: HashMap<u32, Usage>,
}

fn parse_process_sample(text: &str) -> ProcessSample {
    let mut sample = ProcessSample::default();
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let parsed = (|| {
            Some((
                processes::ProcessRow {
                    pid: fields.next()?.parse().ok()?,
                    ppid: fields.next()?.parse().ok()?,
                    pgid: fields.next()?.parse().ok()?,
                },
                fields.next()?.parse::<f64>().ok()?.max(0.0),
                fields.next()?.parse().ok()?,
            ))
        })();
        if let Some((row, cpu, rss_kib)) = parsed {
            let process = fields.collect::<Vec<_>>().join(" ");
            let process = if process.is_empty() {
                None
            } else {
                Path::new(&process)
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            };
            sample.usage.insert(
                row.pid,
                Usage {
                    cpu,
                    rss_kib,
                    process,
                },
            );
            sample.rows.push(row);
        }
    }
    sample
}

impl ProcessSample {
    fn read() -> Self {
        Command::new("/bin/ps")
            .args(["-ax", "-o", "pid=,ppid=,pgid=,%cpu=,rss=,comm="])
            .env("LC_ALL", "C")
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map(|out| parse_process_sample(&String::from_utf8_lossy(&out.stdout)))
            .unwrap_or_default()
    }

    fn tree_pids(&self, roots: &[u32]) -> HashSet<u32> {
        roots
            .iter()
            .flat_map(|&root| std::iter::once(root).chain(processes::descendants(&self.rows, root)))
            .collect()
    }

    fn total(&self, pids: &HashSet<u32>, root: Option<u32>) -> Option<Usage> {
        let mut entries = pids.iter().filter_map(|pid| self.usage.get(pid)).peekable();
        entries.peek()?;
        let (cpu, rss_kib) = entries.fold((0.0, 0u64), |(cpu, rss), entry| {
            (cpu + entry.cpu, rss.saturating_add(entry.rss_kib))
        });
        let process = root
            .and_then(|pid| self.usage.get(&pid))
            .and_then(|entry| entry.process.clone());
        Some(Usage {
            cpu,
            rss_kib,
            process,
        })
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
enum ListenerState {
    Idle,
    Listening,
    Occupied,
}

fn listener_state(owned: &[u32], foreign: bool) -> ListenerState {
    if foreign {
        ListenerState::Occupied
    } else if owned.is_empty() {
        ListenerState::Idle
    } else {
        ListenerState::Listening
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ServiceSample {
    id: String,
    port: Option<u16>,
    actual_port: Option<u16>,
    state: ListenerState,
    usage: Option<Usage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    unexpected_ports: Vec<u16>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Conflict {
    service_id: String,
    port: u16,
    owner: Option<processes::PortOwner>,
    identity: Option<String>,
    suggestion: Option<u16>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    sampled_at: u64,
    started_at: Option<u64>,
    external: bool,
    services: Vec<ServiceSample>,
    total: Option<Usage>,
    history: HashMap<String, Vec<CpuPoint>>,
    conflicts: Vec<Conflict>,
    last_run: Option<RunSummary>,
}

fn services_for(project: &ProjectConfig) -> Vec<projects::Service> {
    if !project.services.is_empty() {
        return project.services.clone();
    }
    vec![projects::Service {
        id: "legacy:root".into(),
        name: "服务".into(),
        dir: ".".into(),
        port: project.port,
        key: "PORT".into(),
        kind: None,
        open_web: project.port.is_some(),
        start_command: None,
    }]
}

fn owned_listeners(
    port: u16,
    listeners: &HashMap<u16, Vec<u32>>,
    project_dir: Option<&Path>,
    managed: Option<&HashSet<u32>>,
) -> (Vec<u32>, bool) {
    let pids = listeners.get(&port).map(Vec::as_slice).unwrap_or_default();
    let owned: Vec<_> = pids
        .iter()
        .copied()
        .filter(|&pid| {
            managed.map_or_else(
                || project_dir.is_some_and(|path| processes::cwd_matches(pid, path)),
                |pids| pids.contains(&pid),
            )
        })
        .collect();
    let foreign = owned.len() != pids.len() || (pids.is_empty() && processes::probe_loopback(port));
    (owned, foreign)
}

fn detect_unexpected_ports(
    info: &ProcessInfo,
    config: &[projects::Service],
    services: &mut [ServiceSample],
    directory: Option<&Path>,
    managed: &HashSet<u32>,
    listeners: &HashMap<u16, Vec<u32>>,
) {
    // Give slower servers a brief chance to bind their requested port.
    if now().saturating_sub(info.run.started_at) < 2_000 {
        return;
    }
    let waiting: Vec<_> = services.iter().enumerate().filter_map(|(index, service)| {
        let port = service.port?;
        ((service.state == ListenerState::Idle || service.actual_port != Some(port)) &&
            info.checks_injected_port(&service.id, port)).then_some(index)
    }).collect();
    if waiting.is_empty() {
        return;
    }
    let expected: HashSet<_> = info.configured_ports().collect();
    let ready_pids: HashSet<_> = expected.iter().filter_map(|port| listeners.get(port))
        .flatten().copied().collect();
    let paths: Vec<_> = config.iter().map(|service| {
        directory.and_then(|directory| directory.join(&service.dir).canonicalize().ok())
    }).collect();
    for (&port, pids) in listeners {
        if expected.contains(&port) {
            continue;
        }
        for &pid in pids {
            if !managed.contains(&pid) || ready_pids.contains(&pid) {
                continue;
            }
            let service = paths.iter().enumerate()
                .filter_map(|(index, path)| path.as_ref().map(|path| (index, path)))
                .filter(|(_, path)| processes::cwd_matches(pid, path))
                .max_by_key(|(_, path)| path.components().count())
                .map(|(index, _)| index)
                .or_else(|| (waiting.len() == 1).then_some(waiting[0]));
            if let Some(index) = service.filter(|index| waiting.contains(index)) {
                services[index].unexpected_ports.push(port);
            }
        }
    }
    for service in services {
        service.unexpected_ports.sort_unstable();
        service.unexpected_ports.dedup();
    }
}

fn sample_project(project: &ProjectConfig, info: Option<&ProcessInfo>) -> Snapshot {
    let processes = ProcessSample::read();
    let listeners = crate::processes::listening_ports();
    let managed = info.map(|info| info.metrics_pids(&processes.rows, &listeners));
    let directory = Path::new(&crate::expand_tilde(&project.path))
        .canonicalize()
        .ok();
    let mut external_roots = Vec::new();
    let mut services = Vec::new();
    let config = services_for(project);
    for service in &config {
        let port = info.and_then(|info| info.actual_service_port(&service.id)).or(service.port);
        let (owned, foreign) = port
            .map(|port| owned_listeners(port, &listeners, directory.as_deref(), managed.as_ref()))
            .unwrap_or_default();
        let state = listener_state(&owned, foreign);
        let usage = processes.total(&processes.tree_pids(&owned), owned.first().copied());
        external_roots.extend(owned);
        services.push(ServiceSample {
            id: service.id.clone(),
            port: service.port,
            actual_port: (state != ListenerState::Idle).then_some(port).flatten(),
            state,
            usage,
            unexpected_ports: Vec::new(),
        });
    }
    let external = info.is_none() && !external_roots.is_empty();
    let (started_at, total) = if let Some(info) = info {
        if let Some(pids) = &managed {
            detect_unexpected_ports(info, &config, &mut services, directory.as_deref(), pids, &listeners);
        }
        let total = managed
            .as_ref()
            .and_then(|pids| processes.total(pids, Some(info.pid)));
        info.run.sample(total.as_ref(), &services);
        (Some(info.run.started_at), total)
    } else {
        let started_at = external_roots
            .iter()
            .filter_map(|&pid| crate::processes::identity(pid))
            .map(crate::processes::ProcessIdentity::started_at_millis)
            .min();
        (
            started_at,
            processes.total(&processes.tree_pids(&external_roots), None),
        )
    };
    Snapshot {
        sampled_at: now(),
        started_at,
        external,
        services,
        total,
        history: HashMap::new(),
        conflicts: Vec::new(),
        last_run: None,
    }
}

pub fn observe_managed(project: &ProjectConfig, info: &ProcessInfo) {
    sample_project(project, Some(info));
}

#[derive(Clone, Default)]
pub struct ExternalRuns(Arc<Mutex<HashMap<String, Run>>>);

impl ExternalRuns {
    pub fn remove(&self, id: &str) {
        self.0.lock().unwrap().remove(id);
    }

    fn sample(&self, id: &str, snapshot: &mut Snapshot) -> Option<Run> {
        if !snapshot.external {
            return self.0.lock().unwrap().remove(id);
        }
        let run = self
            .0
            .lock()
            .unwrap()
            .entry(id.into())
            .or_insert_with(|| Run::external(snapshot.started_at.unwrap_or(snapshot.sampled_at)))
            .clone();
        snapshot.started_at = Some(run.started_at);
        run.sample(snapshot.total.as_ref(), &snapshot.services);
        snapshot.history = run.history();
        None
    }
}

fn get_project(id: &str) -> Result<ProjectConfig, String> {
    crate::get_projects_sync()?
        .projects
        .into_iter()
        .find(|project| project.id == id)
        .ok_or_else(|| "未找到项目".into())
}

#[tauri::command]
pub async fn dashboard_snapshot(
    id: String,
    state: State<'_, ProcessMap>,
    external: State<'_, ExternalRuns>,
) -> Result<Snapshot, String> {
    let info = state.lock().await.managed.get(&id).cloned();
    let external = external.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let project = get_project(&id)?;
        let mut snapshot = sample_project(&project, info.as_ref());
        if let Some(info) = &info {
            snapshot.history = info.run.history();
        }
        let conn = open_project_db()?;
        if let Some(previous) = external.sample(&id, &mut snapshot) {
            let ports: Vec<_> = project.ports().collect();
            save_summary(&conn, &id, &previous.finish(&ports, false))?;
        }
        snapshot.last_run = last_run(&conn, &id)?;
        if info.is_none()
            && !snapshot.external
            && snapshot
                .services
                .iter()
                .any(|service| service.state == ListenerState::Occupied)
        {
            let ports: Vec<_> = project.ports().collect();
            let checks = projects::check_ports(&conn, &ports, Some(&id), &HashSet::new())?;
            snapshot.conflicts = snapshot
                .services
                .iter()
                .filter(|service| service.state == ListenerState::Occupied)
                .filter_map(|service| {
                    let port = service.port?;
                    let check = checks.get(&port)?;
                    let owner = check
                        .in_use
                        .as_ref()
                        .and_then(|usage| usage.owners.first())
                        .map(|owner| processes::PortOwner {
                            pid: owner.pid,
                            command: owner.command.clone(),
                            cwd: owner.cwd.clone(),
                            in_project: owner.in_project,
                        });
                    let identity = owner
                        .as_ref()
                        .and_then(|owner| processes::identity(owner.pid))
                        .map(processes::ProcessIdentity::token);
                    Some(Conflict {
                        service_id: service.id.clone(),
                        port,
                        owner,
                        identity,
                        suggestion: check.suggestion,
                    })
                })
                .collect();
        }
        Ok(snapshot)
    })
    .await
    .map_err(|error| format!("读取仪表盘失败：{error}"))?
}

async fn read_status(port: u16, path: &str) -> Option<u16> {
    let mut stream = tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
        .await
        .ok()?;
    stream
        .write_all(
            format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .await
        .ok()?;
    let mut reader = tokio::io::AsyncReadExt::take(BufReader::new(stream), 1024);
    let mut bytes = Vec::new();
    reader.read_until(b'\n', &mut bytes).await.ok()?;
    let mut fields = std::str::from_utf8(&bytes).ok()?.split_whitespace();
    if !fields.next()?.starts_with("HTTP/") {
        return None;
    }
    fields
        .next()?
        .parse::<u16>()
        .ok()
        .filter(|code| (100..600).contains(code))
}

async fn health(port: u16, api: bool, preferred: Option<&str>) -> Health {
    let started = tokio::time::Instant::now();
    let deadline = started + HEALTH_TIMEOUT;
    let paths = if api { API_PATHS } else { &["/"] };
    let preferred = preferred.filter(|candidate| paths.contains(candidate));
    let candidates = preferred.into_iter().chain(
        paths
            .iter()
            .copied()
            .filter(|path| Some(*path) != preferred),
    );
    let mut result = Health {
        port,
        path: preferred.unwrap_or(paths[0]).into(),
        code: None,
        elapsed_ms: 0,
    };
    for path in candidates {
        let request_started = tokio::time::Instant::now();
        let Ok(code) = tokio::time::timeout_at(deadline, read_status(port, path)).await else {
            break;
        };
        let healthy = code.is_some_and(|code| (200..300).contains(&code));
        if healthy || (result.code.is_none() && code.is_some()) {
            result.path = path.into();
            result.code = code;
            result.elapsed_ms = request_started.elapsed().as_millis() as u64;
        }
        if healthy {
            break;
        }
    }
    if result.code.is_none() {
        result.elapsed_ms = started.elapsed().as_millis() as u64;
    }
    result
}

#[tauri::command]
pub async fn dashboard_health(
    id: String,
    preferred: HashMap<String, String>,
    state: State<'_, ProcessMap>,
    external: State<'_, ExternalRuns>,
) -> Result<HashMap<String, Health>, String> {
    let info = state.lock().await.managed.get(&id).cloned();
    let run = info
        .as_ref()
        .map(|info| info.run.clone())
        .or_else(|| external.0.lock().unwrap().get(&id).cloned());
    let active = tauri::async_runtime::spawn_blocking(move || {
        let project = get_project(&id)?;
        let listeners = processes::listening_ports();
        let managed = info.as_ref().map(|info| info.metrics_pids(&processes::process_table(), &listeners));
        let directory = Path::new(&crate::expand_tilde(&project.path))
            .canonicalize()
            .ok();
        Ok::<_, String>(
            services_for(&project)
                .into_iter()
                .filter_map(|service| {
                    let port = info.as_ref().and_then(|info| info.actual_service_port(&service.id)).or(service.port)?;
                    let (owned, foreign) =
                        owned_listeners(port, &listeners, directory.as_deref(), managed.as_ref());
                    (listener_state(&owned, foreign) == ListenerState::Listening).then_some((
                        service.id,
                        port,
                        service.kind.as_deref() == Some("api"),
                    ))
                })
                .collect::<Vec<_>>(),
        )
    })
    .await
    .map_err(|error| error.to_string())??;
    let mut tasks = tokio::task::JoinSet::new();
    for (service_id, port, api) in active {
        let preferred = preferred.get(&service_id).cloned();
        tasks.spawn(async move {
            let result = health(port, api, preferred.as_deref()).await;
            (service_id, result)
        });
    }
    let mut result = HashMap::new();
    while let Some(Ok((service_id, value))) = tasks.join_next().await {
        result.insert(service_id, value);
    }
    if let Some(run) = run {
        run.record_health(&result);
    }
    Ok(result)
}

#[tauri::command]
pub async fn dashboard_release_port(
    id: String,
    port: u16,
    pid: u32,
    identity: String,
    state: State<'_, ProcessMap>,
) -> Result<(), String> {
    let operation = lifecycle::operation(&state, &id).await;
    let _operation = operation.lock().await;
    let data = state.lock().await;
    if data.managed.contains_key(&id) || !data.stopping.is_empty() {
        return Err("项目状态已变化，请刷新端口占用信息".into());
    }
    let active: Vec<_> = data.managed.values().cloned().collect();
    drop(data);
    tauri::async_runtime::spawn_blocking(move || {
        let project = get_project(&id)?;
        let listeners = processes::listening_ports();
        let own_directory = Path::new(&crate::expand_tilde(&project.path))
            .canonicalize()
            .ok()
            .is_some_and(|directory| processes::cwd_matches(pid, &directory));
        if !project.ports().any(|configured| configured == port)
            || own_directory
            || !listeners.get(&port).is_some_and(|pids| pids.contains(&pid))
            || active.iter().any(|info| {
                info.metrics_pids(&processes::process_table(), &listeners)
                    .contains(&pid)
            })
            || processes::identity(pid)
                .map(processes::ProcessIdentity::token)
                .as_deref()
                != Some(identity.as_str())
        {
            return Err("占用端口的进程已变化，请刷新后再试".into());
        }
        if unsafe { libc::kill(pid as i32, libc::SIGTERM) } != 0 {
            return Err(format!(
                "无法向 PID {pid} 发送 SIGTERM：{}",
                std::io::Error::last_os_error()
            ));
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while processes::port_pids(port).contains(&pid) {
            if std::time::Instant::now() >= deadline {
                return Err(format!("PID {pid} 收到 SIGTERM 后仍在监听端口 {port}"));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Ok(())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_and_retains_latest_summary_with_services_and_peak() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE projects(id TEXT PRIMARY KEY)", [])
            .unwrap();
        conn.execute("INSERT INTO projects VALUES ('p')", [])
            .unwrap();
        migrate(&conn).unwrap();
        assert!(last_run(&conn, "p").unwrap().is_none());
        let mut run = Run::new();
        run.started_at = 10;
        run.sample(
            Some(&Usage {
                cpu: 2.0,
                rss_kib: 2000,
                process: None,
            }),
            &[],
        );
        run.sample(
            Some(&Usage {
                cpu: 1.0,
                rss_kib: 1000,
                process: None,
            }),
            &[],
        );
        run.record_health(&HashMap::from([(
            "api".into(),
            Health {
                port: 8100,
                path: "/healthz".into(),
                code: Some(204),
                elapsed_ms: 5,
            },
        )]));
        run.record_logs(&[logs::Entry {
            text: "ERROR Traceback".into(),
            stream: "stdout".into(),
        }]);
        run.exit(Some(3), None);
        let expected = run.finish(&[], false);
        save_summary(&conn, "p", &expected).unwrap();
        migrate(&conn).unwrap();
        assert_eq!(last_run(&conn, "p").unwrap(), Some(expected.clone()));
        assert_eq!(
            (expected.peak_kib, expected.errors, expected.exit_code),
            (Some(2000), 1, Some(3))
        );
        save_summary(
            &conn,
            "p",
            &RunSummary {
                started_at: 1,
                errors: 9,
                ..expected
            },
        )
        .unwrap();
        assert_eq!(last_run(&conn, "p").unwrap().unwrap().errors, 1);
        save_summary(&conn, "removed", &run.finish(&[], false)).unwrap();
        assert!(last_run(&conn, "removed").unwrap().is_none());
    }

    #[test]
    fn totals_overlapping_process_trees_only_once() {
        let sample = parse_process_sample("10 1 10 2.5 4000 /usr/bin/python3\n11 10 10 1.5 3500 /tmp/fixture/node\n12 11 10 1.0 2500 /tmp/more workers\n14 1 20 11 10000 unrelated\n");
        let tree = sample.tree_pids(&[10, 11]);
        assert_eq!(tree, HashSet::from([10, 11, 12]));
        assert_eq!(
            sample.total(&tree, Some(10)),
            Some(Usage {
                cpu: 5.0,
                rss_kib: 10000,
                process: Some("python3".into())
            })
        );
        assert_eq!(listener_state(&[], true), ListenerState::Occupied);
        assert_eq!(listener_state(&[10], false), ListenerState::Listening);
    }

    #[test]
    fn samples_keep_at_most_ten_minutes_and_120_points_per_service() {
        let mut histories = HashMap::new();
        for i in 0..130 {
            remember(&mut histories, "web", i * 5000, i as f64);
        }
        remember(&mut histories, "web", 646_000, 999.0);
        let web = &histories["web"];
        assert_eq!(
            (
                web.len(),
                web.front().unwrap().at,
                web.back().unwrap().value
            ),
            (120, 50_000, 129.0)
        );
    }

    #[test]
    fn external_runs_report_only_observable_exit_details() {
        let samples = ExternalRuns::default();
        let mut snapshot = Snapshot {
            sampled_at: now(),
            started_at: Some(now().saturating_sub(60_000)),
            external: true,
            services: Vec::new(),
            total: Some(Usage {
                cpu: 1.0,
                rss_kib: 2048,
                process: None,
            }),
            history: HashMap::new(),
            conflicts: Vec::new(),
            last_run: None,
        };
        assert!(samples.sample("fixture", &mut snapshot).is_none());
        assert_eq!(snapshot.history["project"].len(), 1);
        snapshot.external = false;
        let previous = samples
            .sample("fixture", &mut snapshot)
            .unwrap()
            .finish(&[], false);
        assert!(previous.external);
        assert_eq!(
            (previous.peak_kib, previous.exit_code, previous.signal),
            (Some(2048), None, None)
        );
        assert!(samples.sample("fixture", &mut snapshot).is_none());
    }

    #[tokio::test]
    async fn health_selects_and_reuses_api_path_and_reports_an_http_failure() {
        let server = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = server.local_addr().unwrap().port();
        let task = tokio::spawn(async move {
            let mut requests = Vec::new();
            for _ in 0..4 {
                let (mut stream, _) = server.accept().await.unwrap();
                let mut line = String::new();
                let mut reader = BufReader::new(&mut stream);
                reader.read_line(&mut line).await.unwrap();
                loop {
                    let mut header = String::new();
                    reader.read_line(&mut header).await.unwrap();
                    if header == "\r\n" || header.is_empty() {
                        break;
                    }
                }
                let path = line.split_whitespace().nth(1).unwrap().to_string();
                let code = match path.as_str() {
                    "/health" => 204,
                    "/" => 503,
                    _ => 404,
                };
                stream
                    .write_all(
                        format!("HTTP/1.1 {code} fixture\r\nContent-Length: 0\r\n\r\n").as_bytes(),
                    )
                    .await
                    .unwrap();
                requests.push(path);
            }
            requests
        });
        let first = health(port, true, None).await;
        let second = health(port, true, Some(&first.path)).await;
        let failed = health(port, false, None).await;
        assert_eq!(
            (first.code, second.code, failed.code),
            (Some(204), Some(204), Some(503))
        );
        assert_eq!(
            task.await.unwrap(),
            ["/api/health", "/health", "/health", "/"]
        );
    }

    #[tokio::test]
    async fn health_timeout_is_bounded_for_a_nonresponding_listener() {
        let server = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = server.local_addr().unwrap().port();
        let _hold = tokio::spawn(async move {
            let (_stream, _) = server.accept().await.unwrap();
            std::future::pending::<()>().await;
        });
        let result = health(port, true, None).await;
        assert!(result.code.is_none());
        assert!(result.elapsed_ms >= 2000);
    }
}
