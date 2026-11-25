pub(crate) mod agent_sessions;

use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager, State};
use crate::terminal_agent::{AgentSession, AgentStatus, Probe};
use crate::terminal_agent::sessions::SharedCatalog as AgentCatalog;
use crate::terminal_attention::{AttentionManager, Sinks as AttentionSinks};
use crate::terminal_escape::{osc_cwd, osc_notification, EscapeEvent, EscapeStream};
use crate::terminal_shell::{self, Boundaries, Completion};

const SCROLLBACK_BYTES: usize = 2 * 1024 * 1024;
static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct TerminalKey {
    pub project_id: String,
    pub slot: u8,
}

impl TerminalKey {
    pub fn new(project_id: &str, slot: u8) -> Self {
        Self { project_id: project_id.into(), slot }
    }
}

// Recover a poisoned lock: a failed worker must not take down the app or other terminals.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|err| err.into_inner())
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSessionInfo {
    pub project_id: String,
    pub slot: u8,
    pub session_id: String,
    pub tab_order: u64,
    pub alive: bool,
    pub exit_code: Option<u32>,
    pub pid: Option<u32>,
    pub attention: bool,
    pub title: Option<String>,
    pub completion: Option<Completion>,
    #[serde(flatten)]
    pub agent_status: AgentStatus,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionEvent {
    pub project_id: String,
    pub slot: u8,
    pub session_id: String,
    pub completion: Option<Completion>,
}

/// A desktop notification (OSC 9 / OSC 777) from a program that is not a known AI CLI.
#[derive(Clone, Debug)]
pub struct ProgramNotice {
    pub project_id: String,
    pub slot: u8,
    pub session_id: String,
    pub message: String,
    pub at: u64,
}

// Programs often send OSC 9 and OSC 777 together; forward one of them.
const PROGRAM_NOTICE_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Serialize)]
pub struct TerminalContext {
    shell: Option<String>,
    cwd: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalOutput {
    pub project_id: String,
    pub slot: u8,
    pub session_id: String,
    pub seq: u64,
    pub data: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalExit {
    pub project_id: String,
    pub slot: u8,
    pub session_id: String,
    pub exit_code: Option<u32>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalClosed {
    pub project_id: String,
    pub slot: u8,
    pub session_id: String,
}

#[derive(Debug, Serialize)]
pub struct RunResult {
    slot: u8,
}

#[derive(Debug, Serialize)]
pub struct Snapshot {
    data: String,
    seq: u64,
}

#[derive(Debug, Serialize)]
pub struct OpenResult {
    session: TerminalSessionInfo,
    snapshot: Snapshot,
}

#[derive(Default)]
struct Utf8Decoder {
    pending: Vec<u8>,
}

impl Utf8Decoder {
    fn decode(&mut self, bytes: &[u8], eof: bool) -> String {
        self.pending.extend_from_slice(bytes);
        let mut text = String::new();
        let mut offset = 0;
        while offset < self.pending.len() {
            match std::str::from_utf8(&self.pending[offset..]) {
                Ok(valid) => {
                    text.push_str(valid);
                    offset = self.pending.len();
                }
                Err(err) => {
                    let valid_end = offset + err.valid_up_to();
                    // This prefix was validated by from_utf8 above.
                    text.push_str(
                        std::str::from_utf8(&self.pending[offset..valid_end]).unwrap_or(""),
                    );
                    offset = valid_end;
                    if let Some(len) = err.error_len() {
                        text.push('\u{fffd}');
                        offset += len;
                    } else {
                        break; // Keep an incomplete trailing character for the next read.
                    }
                }
            }
        }
        self.pending.drain(..offset);
        if eof && !self.pending.is_empty() {
            text.push_str(&String::from_utf8_lossy(&self.pending));
            self.pending.clear();
        }
        text
    }
}

struct Scrollback {
    chunks: VecDeque<String>,
    bytes: usize,
    seq: u64,
    cap: usize,
}

impl Scrollback {
    fn new(cap: usize) -> Self {
        Self {
            chunks: VecDeque::new(),
            bytes: 0,
            seq: 0,
            cap,
        }
    }

    fn append(&mut self, data: String) -> u64 {
        self.seq += 1;
        self.bytes += data.len();
        self.chunks.push_back(data);
        while self.bytes > self.cap {
            if let Some(oldest) = self.chunks.pop_front() {
                self.bytes -= oldest.len();
            }
        }
        self.seq
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            data: self.chunks.iter().map(String::as_str).collect(),
            seq: self.seq,
        }
    }
}

struct SessionState {
    alive: bool,
    exit_code: Option<u32>,
}

enum Input {
    Bytes(Vec<u8>),
    Run {
        command: String,
        check_idle: bool,
        reply: mpsc::Sender<Result<(), String>>,
        deadline: std::time::Instant,
    },
}

#[derive(Clone)]
struct Session {
    session_id: String,
    tab_order: u64,
    key: TerminalKey,
    pid: Option<u32>,
    state: Arc<(Mutex<SessionState>, Condvar)>,
    master: Arc<Mutex<Option<Box<dyn MasterPty + Send>>>>,
    writer: mpsc::Sender<Input>,
    killer: Arc<Mutex<Box<dyn ChildKiller + Send + Sync>>>,
    scrollback: Arc<Mutex<Scrollback>>,
    attention: Arc<AttentionManager>,
    project_dir: PathBuf,
    metadata: Arc<Mutex<ShellMetadata>>,
    agent_link: Arc<Mutex<agent_sessions::AgentLink>>,
}

#[derive(Default)]
struct ShellMetadata {
    title: Option<String>,
    cwd: Option<PathBuf>,
    completion: Option<Completion>,
}

impl Session {
    fn completion_event(&self, completion: Option<Completion>) -> CompletionEvent {
        CompletionEvent {
            project_id: self.key.project_id.clone(),
            slot: self.key.slot,
            session_id: self.session_id.clone(),
            completion,
        }
    }

    fn info(&self) -> TerminalSessionInfo {
        let state = lock(&self.state.0);
        let metadata = lock(&self.metadata);
        let (attention, agent_status) = self.attention.snapshot(&self.key, &self.session_id);
        TerminalSessionInfo {
            project_id: self.key.project_id.clone(),
            slot: self.key.slot,
            session_id: self.session_id.clone(),
            tab_order: self.tab_order,
            alive: state.alive,
            exit_code: state.exit_code,
            pid: self.pid,
            attention,
            title: metadata.title.clone(),
            completion: metadata.completion.clone(),
            agent_status,
        }
    }

    fn result(&self) -> OpenResult {
        let snapshot = lock(&self.scrollback).snapshot();
        OpenResult {
            snapshot,
            session: self.info(),
        }
    }

    fn context(&self) -> Option<TerminalContext> {
        let pid = self.pid.filter(|_| lock(&self.state.0).alive)?;
        let mut executable = [0; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
        let size = unsafe { libc::proc_pidpath(pid as i32, executable.as_mut_ptr().cast(), executable.len() as u32) };
        let shell = (size > 0).then(|| {
            let path = String::from_utf8_lossy(std::ffi::CStr::from_bytes_until_nul(&executable).ok()?.to_bytes());
            Path::new(path.as_ref()).file_name().map(|name| name.to_string_lossy().into_owned())
        }).flatten();
        let cwd = Some(self.current_directory()).map(|path| {
            if let Some(home) = dirs::home_dir() {
                if let Ok(relative) = path.strip_prefix(home) {
                    return Path::new("~").join(relative).to_string_lossy().into_owned();
                }
            }
            path.to_string_lossy().into_owned()
        });
        Some(TerminalContext { shell, cwd })
    }

    fn current_directory(&self) -> PathBuf {
        let cwd = lock(&self.metadata).cwd.clone();
        cwd.filter(|path| path.is_dir())
            .or_else(|| self.pid.filter(|_| lock(&self.state.0).alive).and_then(crate::processes::process_cwd))
            .unwrap_or_else(|| self.project_dir.clone())
    }

    fn terminate(&self) {
        self.attention.remove(&self.key, &self.session_id);
        // An interactive foreground job can have its own process group.
        let foreground = lock(&self.master)
            .as_ref()
            .and_then(|pty| pty.process_group_leader());
        let signal = |sig| {
            if let Some(pid) = self.pid {
                unsafe {
                    libc::killpg(pid as i32, sig);
                }
                if let Some(pgid) = foreground.filter(|pgid| *pgid > 0 && *pgid != pid as i32) {
                    unsafe {
                        libc::killpg(pgid, sig);
                    }
                }
            } else {
                let _ = lock(&self.killer).kill();
            }
        };
        signal(libc::SIGHUP);
        let state = lock(&self.state.0);
        let _ = self
            .state
            .1
            .wait_timeout_while(state, Duration::from_secs(1), |s| s.alive)
            .unwrap_or_else(|err| err.into_inner());
        // Also clean up jobs which outlived the shell. ESRCH is harmless.
        signal(libc::SIGKILL);
        lock(&self.master).take();
    }

    fn idle(&self) -> bool {
        if !lock(&self.state.0).alive { return false; }
        let master = lock(&self.master);
        let foreground = master.as_ref().and_then(|pty| pty.process_group_leader());
        let shell_group = self.pid.map(|pid| unsafe { libc::getpgid(pid as i32) })
            .filter(|pgid| *pgid > 0);
        shell_group.is_some() && foreground == shell_group
    }

    fn queue_command(&self, command: String, check_idle: bool) -> Result<mpsc::Receiver<Result<(), String>>, String> {
        let (reply, result) = mpsc::channel();
        self.writer.send(Input::Run {
            command, check_idle, reply, deadline: Instant::now() + Duration::from_secs(5),
        }).map_err(|_| "终端输入已断开".to_string())?;
        Ok(result)
    }
}

fn await_command(result: mpsc::Receiver<Result<(), String>>) -> Result<(), String> {
    result.recv_timeout(Duration::from_secs(5)).map_err(|_| "终端未及时响应，请稍后重试".to_string())?
}

type OutputSink = Arc<dyn Fn(TerminalOutput) + Send + Sync>;
type ExitSink = Arc<dyn Fn(TerminalExit) + Send + Sync>;
type OpenedSink = Arc<dyn Fn(TerminalSessionInfo) + Send + Sync>;
type ClosedSink = Arc<dyn Fn(TerminalClosed) + Send + Sync>;
type LimitSource = Arc<dyn Fn() -> Result<u8, String> + Send + Sync>;
type CompletionSink = Arc<dyn Fn(CompletionEvent, bool) + Send + Sync>;
type ProgramNoticeSink = Arc<dyn Fn(ProgramNotice) + Send + Sync>;

#[derive(Clone)]
pub struct TerminalManager {
    sessions: Arc<Mutex<HashMap<TerminalKey, Session>>>,
    // Serialize mutations (including concurrent StrictMode opens), never PTY I/O under the map lock.
    mutation: Arc<Mutex<()>>,
    output: OutputSink,
    exit: ExitSink,
    opened: OpenedSink,
    closed: ClosedSink,
    limit: LimitSource,
    attention: Arc<AttentionManager>,
    integration: Option<PathBuf>,
    completion_enabled: Arc<AtomicBool>,
    agent_notices: Arc<AtomicBool>,
    program_notices: Arc<AtomicBool>,
    completion: CompletionSink,
    program_notice: ProgramNoticeSink,
    inactive: Arc<dyn Fn() -> bool + Send + Sync>,
    visible_tabs: Arc<Mutex<HashSet<TerminalKey>>>,
    agent_catalog: Arc<AgentCatalog>,
}

impl TerminalManager {
    #[cfg(test)]
    fn new(output: OutputSink, exit: ExitSink) -> Self {
        Self::with_attention(output, exit, Arc::new(|_| {}), Arc::new(|_| {}), Arc::new(|| Ok(2)), AttentionSinks::default())
    }

    fn with_attention(output: OutputSink, exit: ExitSink, opened: OpenedSink, closed: ClosedSink, limit: LimitSource, sinks: AttentionSinks) -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            mutation: Arc::new(Mutex::new(())),
            output,
            exit,
            opened,
            closed,
            limit,
            attention: AttentionManager::new(sinks),
            integration: None,
            completion_enabled: Arc::new(AtomicBool::new(true)),
            agent_notices: Arc::new(AtomicBool::new(true)),
            program_notices: Arc::new(AtomicBool::new(true)),
            completion: Arc::new(|_, _| {}),
            program_notice: Arc::new(|_| {}),
            inactive: Arc::new(|| false),
            visible_tabs: Arc::new(Mutex::new(HashSet::new())),
            agent_catalog: Arc::new(AgentCatalog::new(None, None)),
        }
    }

    pub fn for_app(app: AppHandle) -> Self {
        let output_app = app.clone();
        let exit_app = app.clone();
        let opened_app = app.clone();
        let closed_app = app.clone();
        let change_app = app.clone();
        let agent_app = app.clone();
        let badge_app = app.clone();
        let inactive_app = app.clone();
        let inactive = Arc::new(move || {
            inactive_app.get_webview_window("main").is_none_or(|window| {
                !window.is_visible().unwrap_or(false) || !window.is_focused().unwrap_or(false)
            })
        });
        let notification_app = app.clone();
        let program_app = app.clone();
        let directory = terminal_data_dir(&app);
        let agent_notices = Arc::new(AtomicBool::new(crate::open_project_db()
            .and_then(|db| crate::settings::enabled(&db, crate::settings::AGENT_NOTICES)).unwrap_or(true)));
        let notify_agent = agent_notices.clone();
        let mut manager = Self::with_attention(
            Arc::new(move |event| {
                let _ = output_app.emit("terminal-output", event);
            }),
            Arc::new(move |event| {
                let _ = exit_app.emit("terminal-exit", event);
            }),
            Arc::new(move |event| { let _ = opened_app.emit("terminal-opened", event); }),
            Arc::new(move |event| {
                crate::terminal_notifications::remove(&closed_app, &event);
                let _ = closed_app.emit("terminal-closed", event);
            }),
            Arc::new(|| crate::settings::max_per_project(&crate::open_project_db()?)),
            AttentionSinks {
                change: Arc::new(move |event| { let _ = change_app.emit("terminal-attention", event); }),
                agent: Arc::new(move |event, background| {
                    let _ = agent_app.emit_to("main", "terminal-agent", &event);
                    crate::tray::refresh(&agent_app);
                    if background && notify_agent.load(Ordering::Relaxed) {
                        crate::terminal_notifications::post_agent(&agent_app, event);
                    }
                }),
                badge: Arc::new(move |count| {
                    if let Some(window) = badge_app.get_webview_window("main") {
                        let _ = window.set_badge_count((count > 0).then_some(count as i64));
                    }
                    #[cfg(debug_assertions)]
                    crate::debug_e2e::record_terminal_badge(count);
                }),
                inactive: inactive.clone(),
                bounce: Arc::new(move || {
                    if let Some(window) = app.get_webview_window("main") {
                        let _ = window.request_user_attention(Some(tauri::UserAttentionType::Informational));
                    }
                    #[cfg(debug_assertions)]
                    crate::debug_e2e::record_terminal_bounce();
                }),
            },
        );
        manager.integration = directory.as_ref().and_then(|path| match terminal_shell::install(path) {
            Ok(integration) => Some(integration),
            Err(error) => { eprintln!("初始化终端 shell 集成失败：{error}"); None }
        });
        manager.agent_catalog = Arc::new(AgentCatalog::new(dirs::home_dir(), directory.as_deref()));
        manager.inactive = inactive;
        manager.agent_notices = agent_notices;
        let enabled = crate::open_project_db().and_then(|db| crate::settings::enabled(&db, crate::settings::COMMAND_NOTICES)).unwrap_or(true);
        manager.completion_enabled.store(enabled, Ordering::Relaxed);
        manager.completion = Arc::new(move |event, background| {
            let _ = notification_app.emit("terminal-completion", &event);
            crate::pet::note_completion(&notification_app, &event);
            if background && event.completion.is_some() {
                crate::terminal_notifications::post(&notification_app, event);
            }
        });
        manager.program_notices.store(crate::open_project_db()
            .and_then(|db| crate::settings::enabled(&db, crate::settings::PROGRAM_NOTICES)).unwrap_or(true), Ordering::Relaxed);
        manager.program_notice = Arc::new(move |notice| crate::terminal_notifications::post_program(&program_app, notice));
        manager
    }

    pub fn set_command_notices(&self, enabled: bool) {
        self.completion_enabled.store(enabled, Ordering::Relaxed);
        if !enabled {
            for session in lock(&self.sessions).values() {
                if lock(&session.metadata).completion.take().is_some() {
                    (self.completion)(session.completion_event(None), false);
                }
            }
        }
    }

    pub fn set_agent_notices(&self, enabled: bool) {
        self.agent_notices.store(enabled, Ordering::Relaxed);
    }

    pub fn set_program_notices(&self, enabled: bool) {
        self.program_notices.store(enabled, Ordering::Relaxed);
    }

    pub fn agents(&self) -> Vec<(TerminalKey, AgentSession)> {
        self.attention.agents()
    }

    fn get(&self, project_id: &str, slot: u8) -> Result<Session, String> {
        lock(&self.sessions)
            .get(&TerminalKey::new(project_id, slot))
            .cloned()
            .ok_or_else(|| "该终端已关闭".to_string())
    }

    #[cfg(test)]
    fn open(
        &self,
        project_id: &str,
        slot: u8,
        cwd: &Path,
        cols: u16,
        rows: u16,
        restart: bool,
        env: &[crate::projects::EnvVar],
    ) -> Result<OpenResult, String> {
        let _mutation = lock(&self.mutation);
        self.open_locked(project_id, slot, cwd, cols, rows, restart, env)
    }

    fn open_locked(&self, project_id: &str, slot: u8, cwd: &Path, cols: u16, rows: u16, restart: bool, env: &[crate::projects::EnvVar]) -> Result<OpenResult, String> {
        validate_slot(slot)?;
        let existing = self.get(project_id, slot)?;
        if !lock(&existing.state.0).alive {
            return Err("该终端已关闭".into());
        }
        if !restart {
            return Ok(existing.result());
        }
        existing.terminate();
        let result = self.spawn_and_insert(project_id, slot, cwd, cols, rows, env, Some(existing.tab_order));
        if result.is_err() {
            self.remove_matching(|session| session.session_id == existing.session_id);
        }
        result
    }

    fn spawn_and_insert(&self, project_id: &str, slot: u8, cwd: &Path, cols: u16, rows: u16, env: &[crate::projects::EnvVar], tab_order: Option<u64>) -> Result<OpenResult, String> {
        if !cwd.is_dir() {
            return Err(format!("项目目录不存在：{}", cwd.display()));
        }
        let session = self.spawn(project_id, slot, cwd, cols.max(1), rows.max(1), env, tab_order)?;
        let result = session.result();
        lock(&self.sessions).insert(session.key.clone(), session);
        (self.opened)(result.session.clone());
        Ok(result)
    }

    fn create(&self, project_id: &str, cwd: &Path, cols: u16, rows: u16, env: &[crate::projects::EnvVar]) -> Result<OpenResult, String> {
        let _mutation = lock(&self.mutation);
        self.create_locked(project_id, cwd, cols, rows, env)
    }

    fn create_locked(&self, project_id: &str, cwd: &Path, cols: u16, rows: u16, env: &[crate::projects::EnvVar]) -> Result<OpenResult, String> {
        let max = (self.limit)()?.clamp(1, 6);
        let slots: HashSet<_> = lock(&self.sessions).keys().filter(|key| key.project_id == project_id).map(|key| key.slot).collect();
        if slots.len() >= max as usize {
            return Err(format!("每个项目最多 {max} 个终端，可在设置中调整"));
        }
        let slot = (1..=6).find(|slot| !slots.contains(slot)).ok_or("每个项目最多 6 个终端，可在设置中调整")?;
        self.spawn_and_insert(project_id, slot, cwd, cols, rows, env, None)
    }

    fn spawn(
        &self,
        project_id: &str,
        slot: u8,
        cwd: &Path,
        cols: u16,
        rows: u16,
        env: &[crate::projects::EnvVar],
        tab_order: Option<u64>,
    ) -> Result<Session, String> {
        crate::projects::validate_env(env)?;
        let pair = native_pty_system()
            .openpty(size(cols, rows))
            .map_err(|err| format!("创建终端失败：{err}"))?;
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|err| format!("读取终端失败：{err}"))?;
        let mut writer = pair
            .master
            .take_writer()
            .map_err(|err| format!("写入终端失败：{err}"))?;
        let command = terminal_shell::command(self.integration.as_deref(), cwd, env);
        let child = pair
            .slave
            .spawn_command(command)
            .map_err(|err| format!("启动终端失败：{err}"))?;
        drop(pair.slave); // Otherwise EOF is never observed by the master reader.
        let pid = child.process_id();
        let killer = child.clone_killer();
        let (sender, receiver) = mpsc::channel::<Input>();
        let sequence = NEXT_SESSION.fetch_add(1, Ordering::Relaxed);
        let session = Session {
            session_id: format!("{}-{sequence}", std::process::id()),
            tab_order: tab_order.unwrap_or(sequence),
            key: TerminalKey::new(project_id, slot),
            pid,
            state: Arc::new((
                Mutex::new(SessionState {
                    alive: true,
                    exit_code: None,
                }),
                Condvar::new(),
            )),
            master: Arc::new(Mutex::new(Some(pair.master))),
            writer: sender,
            killer: Arc::new(Mutex::new(killer)),
            scrollback: Arc::new(Mutex::new(Scrollback::new(SCROLLBACK_BYTES))),
            attention: self.attention.clone(),
            project_dir: cwd.to_owned(),
            metadata: Arc::new(Mutex::new(ShellMetadata::default())),
            agent_link: Arc::new(Mutex::new(agent_sessions::AgentLink::default())),
        };
        let master = Arc::downgrade(&session.master);
        let probe = Probe::new(move || {
            let master = master.upgrade()?;
            let group = lock(&master).as_ref()?.process_group_leader()?;
            // portable-pty starts the shell as the leader of its session and group.
            (Some(group as u32) != pid).then_some(group)
        }, cols, rows);
        self.attention.register_pty(&session.key, &session.session_id, probe);

        // Retain ownership outside the closure until the waiter has successfully started.
        let pending_child = Arc::new(Mutex::new(Some(child)));
        let waiter_child = pending_child.clone();
        let waiter_session = session.clone();
        let manager = self.clone();
        let waiter = thread::Builder::new()
            .name("terminal-waiter".into())
            .spawn(move || {
                let child = lock(&waiter_child).take();
                let exit_code =
                    child.and_then(|mut child| child.wait().ok().map(|status| status.exit_code()));
                waiter_session.attention.remove(&waiter_session.key, &waiter_session.session_id);
                {
                    let mut state = lock(&waiter_session.state.0);
                    state.alive = false;
                    state.exit_code = exit_code;
                    waiter_session.state.1.notify_all();
                }
                (manager.exit)(TerminalExit {
                    project_id: waiter_session.key.project_id.clone(),
                    slot: waiter_session.key.slot,
                    session_id: waiter_session.session_id.clone(),
                    exit_code,
                });
                // Notify a concurrent terminate() before waiting for its mutation lock.
                let _mutation = lock(&manager.mutation);
                manager.remove_matching(|session| session.session_id == waiter_session.session_id);
            });
        if let Err(err) = waiter {
            self.attention.remove(&session.key, &session.session_id);
            if let Some(mut child) = lock(&pending_child).take() {
                let _ = child.kill();
                let _ = child.wait();
            }
            return Err(format!("启动终端等待线程失败：{err}"));
        }

        let writer_master = session.master.clone();
        let writer_attention = self.attention.clone();
        let writer_key = session.key.clone();
        let writer_session = session.session_id.clone();
        let writer_result =
            thread::Builder::new()
                .name("terminal-writer".into())
                .spawn(move || {
                    while let Ok(input) = receiver.recv() {
                        match input {
                            Input::Bytes(data) => {
                                if data.contains(&b'\r') || data.windows(6).any(|part| matches!(part, b"\x1b[200~" | b"\x1b[201~")) {
                                    writer_attention.input(&writer_key, &writer_session, &data);
                                }
                                if writer
                                    .write_all(&data)
                                    .and_then(|_| writer.flush())
                                    .is_err()
                                {
                                    break;
                                }
                            }
                            Input::Run {
                                command,
                                check_idle,
                                reply,
                                deadline,
                            } => {
                                // Check at the actual write, after any previously queued terminal input.
                                let master = lock(&writer_master);
                                let foreground =
                                    master.as_ref().and_then(|pty| pty.process_group_leader());
                                let shell_group = pid
                                    .map(|pid| unsafe { libc::getpgid(pid as i32) })
                                    .filter(|pgid| *pgid > 0);
                                let result = if std::time::Instant::now() >= deadline {
                                    Err("终端未及时响应，请稍后重试".into())
                                } else if check_idle && (shell_group.is_none() || foreground != shell_group) {
                                    Err("终端中有程序正在运行，请结束后再运行命令".into())
                                } else {
                                    writer
                                        .write_all(format!("\x15{command}\r").as_bytes())
                                        .and_then(|_| writer.flush())
                                        .map_err(|err| format!("写入终端失败：{err}"))
                                };
                                let _ = reply.send(result);
                            }
                        }
                    }
                });
        if let Err(err) = writer_result {
            session.terminate();
            return Err(format!("启动终端输入线程失败：{err}"));
        }

        let scrollback = session.scrollback.clone();
        let output = self.output.clone();
        let output_key = session.key.clone();
        let output_session = session.session_id.clone();
        let attention = self.attention.clone();
        let metadata = session.metadata.clone();
        let completion_sink = self.completion.clone();
        let completion_enabled = self.completion_enabled.clone();
        let program_notices = self.program_notices.clone();
        let program_notice = self.program_notice.clone();
        let visible_tabs = self.visible_tabs.clone();
        let inactive = self.inactive.clone();
        let reader_result =
            thread::Builder::new()
                .name("terminal-reader".into())
                .spawn(move || {
                    let mut decoder = Utf8Decoder::default();
                let mut escapes = EscapeStream::default();
                let mut boundaries = Boundaries::default();
                let mut last_notice: Option<Instant> = None;
                    let mut buffer = [0; 32 * 1024];
                    let emit = |data: String| {
                        if data.is_empty() {
                            return;
                        }
                        let seq = lock(&scrollback).append(data.clone());
                        output(TerminalOutput {
                            project_id: output_key.project_id.clone(),
                            slot: output_key.slot,
                            session_id: output_session.clone(),
                            seq,
                            data,
                        });
                    };
                    loop {
                        match reader.read(&mut buffer) {
                            Ok(0) => break,
                            Ok(len) => {
                                let mut consumed = 0;
                                for (index, &byte) in buffer[..len].iter().enumerate() {
                                    if let EscapeEvent::Osc(osc) = escapes.event(byte) {
                                        let text = String::from_utf8_lossy(&osc);
                                        match text.split_once(';') {
                                            Some(("7", value)) => lock(&metadata).cwd = osc_cwd(value),
                                            Some(("0" | "2", value)) => lock(&metadata).title =
                                                Some(value.chars().filter(|char| !char.is_control()).take(160).collect()),
                                            Some((command @ ("9" | "777"), value)) => {
                                                let Some(message) = osc_notification(command, value) else { continue };
                                                // Known AI CLIs report through their session state instead.
                                                if program_notices.load(Ordering::Relaxed)
                                                    && last_notice.is_none_or(|at| at.elapsed() >= PROGRAM_NOTICE_INTERVAL)
                                                    && inactive() && !lock(&visible_tabs).contains(&output_key)
                                                    && attention.agent_process(&output_key, &output_session).is_none()
                                                {
                                                    last_notice = Some(Instant::now());
                                                    program_notice(ProgramNotice {
                                                        project_id: output_key.project_id.clone(),
                                                        slot: output_key.slot,
                                                        session_id: output_session.clone(),
                                                        message,
                                                        at: SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64,
                                                    });
                                                }
                                            }
                                            Some(("133", value)) => {
                                                let result = boundaries.observe(value, Instant::now());
                                                if let Some(result) = result.filter(|_| completion_enabled.load(Ordering::Relaxed)) {
                                                    emit(decoder.decode(&buffer[consumed..=index], false));
                                                    emit(result.divider());
                                                    consumed = index + 1;
                                                    let background = inactive();
                                                    let visible = lock(&visible_tabs);
                                                    if background || !visible.contains(&output_key) {
                                                        lock(&metadata).completion = Some(result.clone());
                                                        completion_sink(CompletionEvent {
                                                            project_id: output_key.project_id.clone(),
                                                            slot: output_key.slot,
                                                            session_id: output_session.clone(),
                                                            completion: Some(result),
                                                        }, background);
                                                    }
                                                }
                                            }
                                            _ => (),
                                        }
                                    }
                                }
                                attention.output(&output_key, &output_session, &buffer[..len]);
                                emit(decoder.decode(&buffer[consumed..len], false));
                            }
                            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
                            Err(_) => break, // macOS/Linux may report EIO at the end of a PTY.
                        }
                    }
                    emit(decoder.decode(&[], true));
                });
        if let Err(err) = reader_result {
            session.terminate();
            return Err(format!("启动终端输出线程失败：{err}"));
        }
        Ok(session)
    }

    pub fn list(&self) -> Vec<TerminalSessionInfo> {
        let mut sessions: Vec<_> = lock(&self.sessions).values().map(Session::info).collect();
        sessions.sort_by(|a, b| a.project_id.cmp(&b.project_id).then(a.tab_order.cmp(&b.tab_order)));
        sessions
    }

    pub fn display_number(&self, project_id: &str, slot: u8, session_id: &str) -> Option<usize> {
        let sessions = lock(&self.sessions);
        let tab = sessions.get(&TerminalKey::new(project_id, slot))?;
        if tab.session_id != session_id || !lock(&tab.state.0).alive { return None; }
        Some(1 + sessions.values().filter(|session| session.key.project_id == project_id
            && session.tab_order < tab.tab_order && lock(&session.state.0).alive).count())
    }

    #[cfg(test)]
    fn write(&self, project_id: &str, slot: u8, data: Vec<u8>) -> Result<(), String> {
        self.write_for_session(project_id, slot, None, data)
    }

    fn write_for_session(&self, project_id: &str, slot: u8, session_id: Option<&str>, data: Vec<u8>) -> Result<(), String> {
        let session = self.get(project_id, slot)?;
        if session_id.is_some_and(|id| id != session.session_id) {
            return Ok(());
        }
        if !lock(&session.state.0).alive {
            return Err("终端已退出，请重新启动终端".into());
        }
        session
            .writer
            .send(Input::Bytes(data))
            .map_err(|_| "终端输入已断开".to_string())
    }

    fn run(&self, project_id: &str, slot: u8, cwd: &Path, command: String, env: &[crate::projects::EnvVar], current_only: bool) -> Result<RunResult, String> {
        validate_slot(slot)?;
        if command.trim().is_empty() {
            return Err("请输入要运行的命令".into());
        }
        let (used_slot, result) = {
            let _mutation = lock(&self.mutation);
            let mut sessions: Vec<_> = lock(&self.sessions).values()
                .filter(|session| session.key.project_id == project_id).cloned().collect();
            sessions.retain(|session| lock(&session.state.0).alive);
            sessions.sort_by_key(|session| (session.key.slot != slot, session.tab_order));
            if current_only {
                if let Some(current) = sessions.first().filter(|session| session.key.slot == slot) {
                    if !current.idle() {
                        return Err("当前终端中有程序正在运行".into());
                    }
                    sessions.truncate(1);
                }
            }
            let (session, check_idle) = if let Some(session) = sessions.iter().find(|session| session.idle()) {
                (session.clone(), true)
            } else if sessions.len() < (self.limit)()?.clamp(1, 6) as usize || sessions.is_empty() {
                let opened = self.create_locked(project_id, cwd, 80, 24, env)?;
                (self.get(project_id, opened.session.slot)?, false)
            } else {
                return Err("所有终端都在运行程序，请结束其中一个，或在设置中调高终端数量上限".into());
            };
            (session.key.slot, session.queue_command(command, check_idle)?)
        };
        // Neither the map nor the mutation lock is held while the PTY writer replies.
        await_command(result)?;
        Ok(RunResult { slot: used_slot })
    }

    fn resize(&self, project_id: &str, slot: u8, cols: u16, rows: u16) -> Result<(), String> {
        if cols == 0 || rows == 0 {
            return Ok(());
        }
        let session = self.get(project_id, slot)?;
        self.attention.resize_pty(&session.key, &session.session_id, cols, rows);
        let master = lock(&session.master);
        if let Some(master) = master.as_ref() {
            master
                .resize(size(cols, rows))
                .map_err(|err| format!("调整终端大小失败：{err}"))?;
        }
        Ok(())
    }

    pub fn close(&self, project_id: &str) {
        let _mutation = lock(&self.mutation);
        self.remove_matching(|session| session.key.project_id == project_id);
    }

    fn close_terminal(&self, project_id: &str, slot: u8, session_id: Option<&str>) -> Result<(), String> {
        validate_slot(slot)?;
        let _mutation = lock(&self.mutation);
        self.remove_matching(|session| session.key.project_id == project_id && session.key.slot == slot
            && session_id.is_none_or(|id| session.session_id == id));
        Ok(())
    }

    fn emit_closed(&self, session: &Session) {
        (self.closed)(TerminalClosed {
            project_id: session.key.project_id.clone(), slot: session.key.slot,
            session_id: session.session_id.clone(),
        });
    }

    // The caller holds mutation, so an exit from a replaced shell cannot close its successor.
    fn remove_matching(&self, remove: impl Fn(&Session) -> bool) {
        let removed = {
            let mut sessions = lock(&self.sessions);
            let keys: Vec<_> = sessions.values().filter(|session| remove(session)).map(|session| session.key.clone()).collect();
            keys.into_iter().filter_map(|key| sessions.remove(&key)).collect::<Vec<_>>()
        };
        if removed.is_empty() { return; }
        {
            let mut visible = lock(&self.visible_tabs);
            for session in &removed {
                visible.remove(&session.key);
            }
        }
        for session in &removed {
            self.emit_closed(session);
        }
        for session in removed {
            session.terminate();
        }
    }

    pub fn retain_projects(&self, ids: &HashSet<String>) {
        let _mutation = lock(&self.mutation);
        self.remove_matching(|session| !ids.contains(&session.key.project_id));
    }

    pub fn kill_all(&self) {
        self.retain_projects(&HashSet::new());
        self.attention.shutdown();
    }

    pub fn focus(&self, key: Option<TerminalKey>) {
        self.focus_visible(key, None, None);
    }

    fn focus_visible(&self, key: Option<TerminalKey>, visible_slots: Option<Vec<u8>>, session_id: Option<&str>) {
        let mut visible = lock(&self.visible_tabs);
        let sessions = lock(&self.sessions);
        let key = key.filter(|key| sessions.get(key).is_some_and(|session| {
            session_id.is_none_or(|id| session.session_id == id) && lock(&session.state.0).alive
        }));
        visible.clear();
        if let Some(key) = &key {
            visible.insert(key.clone());
            if let Some(slots) = visible_slots {
                visible.extend(slots.into_iter().filter(|slot| validate_slot(*slot).is_ok())
                    .map(|slot| TerminalKey::new(&key.project_id, slot)).filter(|key| sessions.contains_key(key)));
            }
        }
        for session in visible.iter().filter_map(|key| sessions.get(key)) {
            if lock(&session.metadata).completion.take().is_some() {
                (self.completion)(session.completion_event(None), false);
            }
        }
        drop(sessions);
        self.attention.focus_visible(key, &visible);
    }
}

fn terminal_data_dir(app: &AppHandle) -> Option<PathBuf> {
    #[cfg(debug_assertions)]
    if let Some(database) = crate::debug_e2e::database_path() {
        return database.parent().map(Path::to_owned);
    }
    app.path().app_data_dir().ok()
}

fn validate_slot(slot: u8) -> Result<(), String> {
    if (1..=6).contains(&slot) { Ok(()) } else { Err("终端编号必须是 1–6 的整数".into()) }
}

fn size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        cols,
        rows,
        pixel_width: 0,
        pixel_height: 0,
    }
}

pub(crate) fn configure_environment(command: &mut CommandBuilder) {
    let keys: Vec<_> = command
        .iter_full_env_as_str()
        .map(|(key, _)| key.to_string())
        .collect();
    for key in keys {
        if crate::shell_env::should_strip(&key) {
            command.env_remove(key);
        }
    }
    command.env("TERM", "xterm-256color");
    command.env("COLORTERM", "truecolor");
    if !command
        .iter_full_env_as_str()
        .any(|(key, value)| key == "LANG" && !value.is_empty())
    {
        command.env("LANG", "en_US.UTF-8");
    }
}

fn project_config(project_id: &str) -> Result<crate::ProjectConfig, String> {
    let project = crate::get_projects_sync()?
        .projects
        .into_iter()
        .find(|p| p.id == project_id)
        .ok_or_else(|| "未找到该项目，请刷新项目列表".to_string())?;
    Ok(project)
}

// Open/restart can spawn or wait for a shell; run them off the webview's IPC thread.
#[tauri::command]
pub async fn terminal_open(
    state: State<'_, TerminalManager>,
    project_id: String,
    slot: u8,
    cols: u16,
    rows: u16,
    session_id: Option<String>,
) -> Result<OpenResult, String> {
    let manager = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let project = project_config(&project_id)?;
        let path = crate::expand_tilde(&project.path);
        let _mutation = lock(&manager.mutation);
        if let Some(id) = &session_id {
            if manager.get(&project_id, slot)?.session_id != *id {
                return Err("该终端已关闭".into());
            }
        }
        manager.open_locked(
            &project_id,
            slot,
            Path::new(&path),
            cols,
            rows,
            false,
            &project.env,
        )
    })
    .await
    .map_err(|err| format!("打开终端失败：{err}"))?
}

#[tauri::command]
pub async fn terminal_restart(
    state: State<'_, TerminalManager>,
    project_id: String,
    slot: u8,
    cols: u16,
    rows: u16,
    session_id: Option<String>,
) -> Result<OpenResult, String> {
    let manager = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let project = project_config(&project_id)?;
        let path = crate::expand_tilde(&project.path);
        let _mutation = lock(&manager.mutation);
        if let Some(id) = &session_id {
            if manager.get(&project_id, slot)?.session_id != *id {
                return Err("该终端已关闭".into());
            }
        }
        manager.open_locked(
            &project_id,
            slot,
            Path::new(&path),
            cols,
            rows,
            true,
            &project.env,
        )
    })
    .await
    .map_err(|err| format!("重启终端失败：{err}"))?
}

#[tauri::command]
pub async fn terminal_create(
    state: State<'_, TerminalManager>, project_id: String, cols: u16, rows: u16,
) -> Result<OpenResult, String> {
    let manager = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let project = project_config(&project_id)?;
        manager.create(&project_id, Path::new(&crate::expand_tilde(&project.path)), cols, rows, &project.env)
    }).await.map_err(|err| format!("新建终端失败：{err}"))?
}

#[tauri::command]
pub async fn terminal_close(
    state: State<'_, TerminalManager>, project_id: String, slot: u8, session_id: Option<String>,
) -> Result<(), String> {
    let manager = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || manager.close_terminal(&project_id, slot, session_id.as_deref()))
        .await.map_err(|err| format!("关闭终端失败：{err}"))?
}

#[tauri::command]
pub fn terminal_list(state: State<'_, TerminalManager>) -> Vec<TerminalSessionInfo> {
    state.list()
}

#[tauri::command]
pub fn terminal_context(
    state: State<'_, TerminalManager>, project_id: String, slot: u8, session_id: String,
) -> Option<TerminalContext> {
    let session = state.get(&project_id, slot).ok()?;
    (session.session_id == session_id).then(|| session.context()).flatten()
}

#[tauri::command]
pub fn terminal_focus(state: State<'_, TerminalManager>, project_id: Option<String>, slot: Option<u8>, visible_slots: Option<Vec<u8>>, session_id: Option<String>) {
    let key = project_id.zip(slot).map(|(project_id, slot)| TerminalKey { project_id, slot });
    #[cfg(debug_assertions)]
    crate::debug_e2e::record_terminal_focus(key.as_ref());
    state.focus_visible(key, visible_slots, session_id.as_deref());
}

#[tauri::command]
pub async fn terminal_run(
    state: State<'_, TerminalManager>,
    project_id: String,
    slot: u8,
    command: String,
    current_only: Option<bool>,
) -> Result<RunResult, String> {
    let manager = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let project = project_config(&project_id)?;
        manager.run(&project_id, slot, Path::new(&crate::expand_tilde(&project.path)), command, &project.env, current_only.unwrap_or(false))
    })
        .await
        .map_err(|err| format!("运行命令失败：{err}"))?
}

#[tauri::command]
pub fn terminal_write(
    state: State<'_, TerminalManager>,
    project_id: String,
    slot: u8,
    session_id: Option<String>,
    data: String,
) -> Result<(), String> {
    state.write_for_session(&project_id, slot, session_id.as_deref(), data.into_bytes())
}

#[tauri::command]
pub fn terminal_write_bytes(
    state: State<'_, TerminalManager>,
    project_id: String,
    slot: u8,
    session_id: Option<String>,
    data: Vec<u8>,
) -> Result<(), String> {
    state.write_for_session(&project_id, slot, session_id.as_deref(), data)
}

#[tauri::command]
pub fn terminal_resize(
    state: State<'_, TerminalManager>,
    project_id: String,
    slot: u8,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    state.resize(&project_id, slot, cols, rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::{Instant, SystemTime, UNIX_EPOCH};

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(1);
    const TEST_PROMPT: &str = "BENTO_PTY_READY> ";

    struct Fixture {
        manager: TerminalManager,
        dir: PathBuf,
        env: Vec<crate::projects::EnvVar>,
        settings: Arc<Mutex<rusqlite::Connection>>,
        opened: Arc<Mutex<Vec<TerminalSessionInfo>>>,
        closed: Arc<Mutex<Vec<TerminalClosed>>>,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!(
                "apphub-pty-{}-{}-{}",
                std::process::id(),
                NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            let dir = dir.canonicalize().unwrap();
            // Keep real login shells, but isolate them from the developer's rc files/plugins.
            std::fs::write(dir.join(".zshenv"), "unsetopt GLOBAL_RCS\n").unwrap();
            std::fs::write(dir.join(".zprofile"), "export BENTO_TEST_LOGIN=login-env\n").unwrap();
            std::fs::write(dir.join(".zshrc"), format!("PROMPT='{TEST_PROMPT}'\nRPROMPT=''\n")).unwrap();
            let env = vec![crate::projects::EnvVar {
                key: "ZDOTDIR".into(),
                value: dir.to_str().unwrap().into(),
            }];
            let settings = Arc::new(Mutex::new(rusqlite::Connection::open_in_memory().unwrap()));
            crate::init_db(&lock(&settings)).unwrap();
            let db = settings.clone();
            let opened = Arc::new(Mutex::new(Vec::new()));
            let closed = Arc::new(Mutex::new(Vec::new()));
            let opened_sink = opened.clone();
            let closed_sink = closed.clone();
            let mut manager = TerminalManager::new(Arc::new(|_| {}), Arc::new(|_| {}));
            manager.limit = Arc::new(move || crate::settings::max_per_project(&lock(&db)));
            manager.opened = Arc::new(move |event| lock(&opened_sink).push(event));
            manager.closed = Arc::new(move |event| lock(&closed_sink).push(event));
            Self {
                manager, dir, env, settings, opened, closed,
            }
        }

        fn wait_for(&self, id: &str, text: &str) -> String {
            self.wait_for_slot(id, 1, text)
        }

        fn wait_for_slot(&self, id: &str, slot: u8, text: &str) -> String {
            let deadline = Instant::now() + Duration::from_secs(15);
            loop {
                let data = self.manager.get(id, slot).unwrap().result().snapshot.data;
                if data.contains(text) {
                    return data;
                }
                assert!(Instant::now() < deadline, "Missing {text:?}: {}", self.diagnostics(id, slot));
                thread::sleep(Duration::from_millis(30));
            }
        }

        fn limit(&self, max: u8) {
            crate::settings::set(&lock(&self.settings), "terminal.maxPerProject", serde_json::json!(max)).unwrap();
        }

        fn create(&self, id: &str) -> OpenResult {
            self.manager.create(id, &self.dir, 80, 24, &self.env).unwrap()
        }

        fn idle(&self, id: &str, slot: u8) {
            // A foreground process group alone does not mean the shell has finished starting.
            self.wait_for_slot(id, slot, TEST_PROMPT);
            self.wait_until(id, slot, "shell idle", |session| session.idle());
        }

        fn busy(&self, id: &str, slot: u8) {
            self.idle(id, slot);
            self.manager.write(id, slot, b"/bin/sleep 300\r".to_vec()).unwrap();
            self.wait_until(id, slot, "foreground job started", |session| session.info().alive && !session.idle());
        }

        fn exit(&self, id: &str, slot: u8) {
            self.wait_for_slot(id, slot, TEST_PROMPT);
            let session = self.manager.get(id, slot).unwrap();
            let shell_group = session.pid.unwrap() as i32;
            let foreground = lock(&session.master).as_ref().unwrap().process_group_leader().unwrap();
            if foreground != shell_group {
                // Only stop this fixture's foreground job. Ctrl-C can kill a starting shell or
                // flush typed-ahead `exit` before the shell regains control of the terminal.
                assert!(foreground > 0);
                assert_ne!(foreground, unsafe { libc::getpgrp() });
                assert_eq!(unsafe { libc::killpg(foreground, libc::SIGTERM) }, 0);
            }
            self.idle(id, slot);
            self.manager.write(id, slot, b"exit 0\r".to_vec()).unwrap();
            self.wait_closed(&session);
            assert_eq!(session.info().exit_code, Some(0));
        }

        fn wait_closed(&self, session: &Session) {
            let deadline = Instant::now() + Duration::from_secs(15);
            while !lock(&self.closed).iter().any(|event| event.session_id == session.session_id) {
                assert!(Instant::now() < deadline, "No close event for {:?}", session.key);
                thread::sleep(Duration::from_millis(10));
            }
            assert!(self.manager.get(&session.key.project_id, session.key.slot).is_err());
        }

        fn diagnostics(&self, id: &str, slot: u8) -> String {
            let session = self.manager.get(id, slot).unwrap();
            let result = session.result();
            let tail: String = result.snapshot.data.chars().rev().take(1000).collect::<Vec<_>>().into_iter().rev().collect();
            let foreground = lock(&session.master).as_ref().and_then(|pty| pty.process_group_leader());
            format!("{:?}, idle={}, foreground={foreground:?}, scrollback tail={tail:?}", result.session, session.idle())
        }

        fn wait_until(&self, id: &str, slot: u8, label: &str, mut condition: impl FnMut(&Session) -> bool) {
            let deadline = Instant::now() + Duration::from_secs(15);
            while !condition(&self.manager.get(id, slot).unwrap()) {
                assert!(Instant::now() < deadline, "PTY timed out waiting for {label}: {}", self.diagnostics(id, slot));
                thread::sleep(Duration::from_millis(10));
            }
        }

        fn run(&self, id: &str, slot: u8, command: &str) -> RunResult {
            self.manager.run(id, slot, &self.dir, command.into(), &self.env, false).unwrap()
        }
    }

    fn group_gone(pid: u32) {
        let deadline = Instant::now() + Duration::from_secs(15);
        while unsafe { libc::kill(-(pid as i32), 0) } == 0 {
            assert!(Instant::now() < deadline, "PTY process group {pid} survived cleanup");
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn create_enforces_live_db_limit_and_appends_a_reused_slot_after_exit() {
        let fixture = Fixture::new();
        assert!(fixture.manager.open("a", 2, &fixture.dir, 80, 24, false, &fixture.env).unwrap_err().contains("该终端已关闭"));
        assert!(fixture.manager.open("a", 2, &fixture.dir, 80, 24, true, &fixture.env).is_err());
        assert!(fixture.manager.open("a", 0, &fixture.dir, 80, 24, false, &fixture.env).is_err());
        assert_eq!(fixture.create("a").session.slot, 1);
        let two = fixture.create("a");
        assert_eq!(two.session.slot, 2);
        assert_eq!(fixture.manager.create("a", &fixture.dir, 80, 24, &fixture.env).unwrap_err(), "每个项目最多 2 个终端，可在设置中调整");
        fixture.exit("a", 2);
        assert_eq!(fixture.create("a").session.slot, 2);
        fixture.limit(3);
        assert_eq!(fixture.create("a").session.slot, 3);
        let before = fixture.manager.list();
        fixture.limit(1);
        assert_eq!(fixture.manager.list().iter().map(|s| &s.session_id).collect::<Vec<_>>(), before.iter().map(|s| &s.session_id).collect::<Vec<_>>());
        assert!(fixture.manager.list().iter().all(|s| s.alive));
        assert!(fixture.manager.create("a", &fixture.dir, 80, 24, &fixture.env).is_err());
        let restarted = fixture.manager.open("a", 3, &fixture.dir, 80, 24, true, &fixture.env).unwrap();
        assert_ne!(restarted.session.session_id, before[2].session_id);
        assert_eq!(restarted.session.tab_order, before[2].tab_order);
        fixture.manager.close_terminal("a", 2, Some(&before[1].session_id)).unwrap();
        assert!(fixture.manager.create("a", &fixture.dir, 80, 24, &fixture.env).is_err());
        fixture.limit(3);
        assert_eq!(fixture.create("a").session.slot, 2);
        assert_eq!(fixture.manager.list().iter().map(|s| s.slot).collect::<Vec<_>>(), [1, 3, 2]);
        assert_eq!(lock(&fixture.opened).len(), 6);
        assert_eq!(lock(&fixture.closed).iter().map(|s| s.slot).collect::<Vec<_>>(), [2, 2]);
    }

    #[test]
    fn concurrent_creates_cannot_exceed_the_limit() {
        let fixture = Fixture::new();
        fixture.create("a");
        let threads: Vec<_> = (0..6).map(|_| {
            let manager = fixture.manager.clone();
            let dir = fixture.dir.clone();
            let env = fixture.env.clone();
            thread::spawn(move || manager.create("a", &dir, 80, 24, &env))
        }).collect();
        let results: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
        assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
        assert_eq!(fixture.manager.list().len(), 2);
    }

    #[test]
    fn close_and_shell_exit_renumber_the_remaining_tabs_and_allow_slot_reuse() {
        let fixture = Fixture::new();
        fixture.limit(3);
        let tabs: Vec<_> = (0..3).map(|_| fixture.create("a").session).collect();
        let slots = || fixture.manager.list().into_iter().map(|tab| tab.slot).collect::<Vec<_>>();

        fixture.manager.close_terminal("a", 1, Some(&tabs[0].session_id)).unwrap();
        assert_eq!(slots(), [2, 3]);
        assert_eq!(fixture.manager.display_number("a", 2, &tabs[1].session_id), Some(1));
        assert_eq!(fixture.manager.display_number("a", 3, &tabs[2].session_id), Some(2));

        fixture.exit("a", 2);
        assert_eq!(slots(), [3]);
        assert_eq!(fixture.manager.display_number("a", 3, &tabs[2].session_id), Some(1));
        let new = fixture.create("a").session;
        assert_eq!(new.slot, 1);
        fixture.manager.close_terminal("a", 1, Some(&tabs[0].session_id)).unwrap();
        assert_eq!(fixture.manager.display_number("a", 1, &tabs[0].session_id), None);
        assert_eq!(fixture.manager.display_number("a", 1, &new.session_id), Some(2));
        assert_eq!(slots(), [3, 1]);
        fixture.manager.close_terminal("a", 1, Some(&new.session_id)).unwrap();
        fixture.manager.close_terminal("a", 3, Some(&tabs[2].session_id)).unwrap();
        assert!(slots().is_empty());
    }

    #[test]
    fn a_delayed_terminal_report_cannot_reach_a_reused_slot() {
        let fixture = Fixture::new();
        let previous = fixture.create("a").session;
        fixture.manager.close_terminal("a", previous.slot, Some(&previous.session_id)).unwrap();
        let current = fixture.create("a").session;
        assert_eq!(current.slot, previous.slot);
        fixture.idle("a", current.slot);
        let write = |id: &str, bytes: &[u8]| {
            fixture.manager.write_for_session("a", current.slot, Some(id), bytes.to_vec()).unwrap()
        };
        write(&previous.session_id, b"echo STALE_REPORT\r");
        write(&current.session_id, b"echo CURRENT_REPORT\r");
        let output = fixture.wait_for_slot("a", current.slot, "\r\nCURRENT_REPORT\r\n");
        assert!(!output.contains("STALE_REPORT"));
    }

    #[test]
    fn close_and_retain_remove_all_slots_only_for_the_target_project() {
        let fixture = Fixture::new();
        for id in ["b", "a"] { fixture.create(id); fixture.create(id); }
        let before = fixture.manager.list();
        assert_eq!(before.iter().map(|s| (s.project_id.as_str(), s.slot)).collect::<Vec<_>>(), [("a", 1), ("a", 2), ("b", 1), ("b", 2)]);
        fixture.busy("a", 2);
        let foreground = lock(&fixture.manager.get("a", 2).unwrap().master).as_ref().unwrap().process_group_leader().unwrap();
        fixture.manager.retain_projects(&HashSet::from(["b".into()]));
        for s in &before[..2] { group_gone(s.pid.unwrap()); }
        group_gone(foreground as u32);
        assert_eq!(fixture.manager.list().len(), 2);
        assert!(fixture.manager.list().iter().all(|s| s.project_id == "b" && s.alive));
        fixture.manager.close("b");
        for s in &before[2..] { group_gone(s.pid.unwrap()); }
        assert!(fixture.manager.list().is_empty());
        assert_eq!(lock(&fixture.closed).len(), 4);
    }

    #[test]
    fn extra_slot_has_independent_login_shell_cwd_env_output_and_size() {
        let fixture = Fixture::new();
        fixture.create("a");
        let mut env = fixture.env.clone();
        env.push(crate::projects::EnvVar { key: "BENTO_SLOT_TEST".into(), value: "extra-env".into() });
        fixture.manager.create("a", &fixture.dir, 80, 24, &env).unwrap();
        fixture.idle("a", 2);
        fixture.manager.resize("a", 2, 91, 31).unwrap();
        fixture.manager.write("a", 2, b"pwd; printf '%s\\n' $BENTO_SLOT_TEST $BENTO_TEST_LOGIN; stty size\r".to_vec()).unwrap();
        fixture.wait_for_slot("a", 2, &format!("\r\n{}\r\n", fixture.dir.display()));
        fixture.wait_for_slot("a", 2, "\r\nextra-env\r\n");
        fixture.wait_for_slot("a", 2, "\r\nlogin-env\r\n");
        fixture.wait_for_slot("a", 2, "31 91");
        assert!(!fixture.manager.get("a", 1).unwrap().result().snapshot.data.contains("extra-env"));
    }

    #[test]
    fn run_routes_preferred_idle_then_other_idle_and_reuses_capacity_after_exit() {
        let fixture = Fixture::new();
        fixture.limit(3);
        for _ in 0..3 { fixture.create("a"); }
        for slot in 1..=3 { fixture.idle("a", slot); }
        assert_eq!(fixture.run("a", 3, "printf 'preferred\\n'").slot, 3);
        fixture.wait_for_slot("a", 3, "\r\npreferred\r\n");
        fixture.busy("a", 3);
        assert_eq!(fixture.run("a", 3, "printf 'lowest-idle\\n'").slot, 1);
        fixture.wait_for_slot("a", 1, "\r\nlowest-idle\r\n");
        fixture.busy("a", 1);
        assert_eq!(fixture.run("a", 3, "printf 'other-idle\\n'").slot, 2);
        fixture.wait_for_slot("a", 2, "\r\nother-idle\r\n");
        fixture.busy("a", 2);
        let error = fixture.manager.run("a", 1, &fixture.dir, "FORBIDDEN".into(), &fixture.env, false).unwrap_err();
        assert_eq!(error, "所有终端都在运行程序，请结束其中一个，或在设置中调高终端数量上限");
        assert!(fixture.manager.list().iter().all(|s| !fixture.manager.get("a", s.slot).unwrap().result().snapshot.data.contains("FORBIDDEN")));
        fixture.exit("a", 2);
        fixture.exit("a", 3);
        fixture.limit(1);
        assert!(fixture.manager.run("a", 3, &fixture.dir, "FORBIDDEN".into(), &fixture.env, false).is_err());
        fixture.limit(2);
        assert_eq!(fixture.run("a", 3, "printf 'created-after-exit\\n'").slot, 2);
        fixture.wait_for_slot("a", 2, "\r\ncreated-after-exit\r\n");
        assert!(fixture.manager.get("a", 3).is_err());
    }

    #[test]
    fn run_creates_on_demand_and_writer_rechecks_idle_and_deadline() {
        let fixture = Fixture::new();
        assert_eq!(fixture.run("a", 5, "printf 'created-primary\\n'").slot, 1);
        fixture.wait_for("a", "\r\ncreated-primary\r\n");
        fixture.busy("a", 1);
        assert_eq!(fixture.run("a", 1, "printf 'created-extra\\n'").slot, 2);
        fixture.wait_for_slot("a", 2, "\r\ncreated-extra\r\n");
        let session = fixture.manager.get("a", 1).unwrap();
        for (deadline, expected) in [(Instant::now() + Duration::from_secs(5), "终端中有程序正在运行"), (Instant::now() - Duration::from_secs(1), "终端未及时响应")] {
            let (reply, result) = mpsc::channel();
            session.writer.send(Input::Run { command: "DO_NOT_WRITE".into(), check_idle: true, reply, deadline }).unwrap();
            assert!(result.recv_timeout(Duration::from_secs(5)).unwrap().unwrap_err().contains(expected));
        }
        assert!(!session.result().snapshot.data.contains("DO_NOT_WRITE"));
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            self.manager.kill_all();
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn real_pty_bell_raises_unwatched_and_focus_restart_close_exit_clear_it() {
        let fixture = Fixture::new();
        fixture.create("bell");
        let ring = || {
            fixture.idle("bell", 1);
            fixture.manager.write("bell", 1, b"printf '\\a'\n".to_vec()).unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            while !fixture.manager.get("bell", 1).unwrap().info().attention {
                assert!(Instant::now() < deadline, "PTY bell did not raise attention");
                thread::sleep(Duration::from_millis(10));
            }
        };
        ring();
        assert!(fixture.manager.list()[0].attention);
        fixture.manager.focus(Some(TerminalKey::new("bell", 1)));
        assert!(!fixture.manager.list()[0].attention);
        fixture.manager.focus(None);
        ring();
        fixture.manager.open("bell", 1, &fixture.dir, 80, 24, true, &fixture.env).unwrap();
        assert!(!fixture.manager.list()[0].attention);
        ring();
        let last = fixture.manager.get("bell", 1).unwrap();
        fixture.manager.write("bell", 1, b"exit\n".to_vec()).unwrap();
        fixture.wait_closed(&last);
        assert!(!last.info().attention);
        fixture.manager.close("bell");
        assert!(fixture.manager.list().is_empty());
    }

    #[test]
    fn foreground_agent_in_a_wrapper_claims_osc_alerts_until_it_returns_to_shell() {
        use crate::terminal_agent::{AgentKind, AgentState};
        let fixture = Fixture::new();
        let script = fixture.dir.join("claude");
        std::fs::write(&script, "#!/bin/sh\nprintf '\\033]9;Claude needs your permission\\007'\nread -r reply\n").unwrap();
        let mut permissions = std::fs::metadata(&script).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o700);
        std::fs::set_permissions(&script, permissions).unwrap();
        fixture.create("ai");
        fixture.idle("ai", 1);
        fixture.manager.write("ai", 1, format!("/bin/sh -c '{}; :'\r", script.display()).into_bytes()).unwrap();
        fixture.wait_until("ai", 1, "Claude foreground and waiting", |session| {
            let info = session.info();
            !info.attention && info.agent_status.agent.is_some_and(|agent|
                agent.kind == AgentKind::Claude && agent.state == AgentState::Waiting)
        });
        fixture.manager.write("ai", 1, b"exit\r".to_vec()).unwrap();
        fixture.wait_until("ai", 1, "shell regains foreground", |session| {
            session.idle() && session.info().agent_status.agent.is_none()
        });
        assert!(!fixture.manager.list()[0].attention);
    }

    #[test]
    fn foreground_generic_cli_reports_its_notification_as_a_session_state() {
        use crate::terminal_agent::{AgentKind, AgentState};
        let fixture = Fixture::new();
        let script = fixture.dir.join("devin");
        std::fs::write(&script, "#!/bin/sh\nprintf '\\033]777;notify;Devin;Devin needs input\\007'\nread -r reply\n").unwrap();
        let mut permissions = std::fs::metadata(&script).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o700);
        std::fs::set_permissions(&script, permissions).unwrap();
        fixture.create("ai");
        fixture.idle("ai", 1);
        fixture.manager.write("ai", 1, format!("/bin/sh -c '{}; :'\r", script.display()).into_bytes()).unwrap();
        fixture.wait_until("ai", 1, "Devin foreground and waiting", |session| {
            let info = session.info();
            !info.attention && info.agent_status.agent.is_some_and(|agent|
                agent.kind == AgentKind::Devin && agent.state == AgentState::Waiting)
        });
        fixture.manager.write("ai", 1, b"exit\r".to_vec()).unwrap();
        fixture.wait_until("ai", 1, "shell regains foreground", |session| {
            session.idle() && session.info().agent_status.agent.is_none()
        });
    }

    #[test]
    fn background_program_notifications_are_forwarded_once_and_follow_the_setting() {
        let mut fixture = Fixture::new();
        let notices = Arc::new(Mutex::new(Vec::new()));
        let sink = notices.clone();
        fixture.manager.inactive = Arc::new(|| true);
        fixture.manager.program_notice = Arc::new(move |notice| lock(&sink).push(notice.message));
        fixture.create("notes");
        fixture.idle("notes", 1);
        fixture.manager.write("notes", 1, b"printf '\\033]9;4;1;50\\007\\033]9;Build finished\\007\\033]777;notify;Build;finished\\007'; echo SENT_$((1+1))\r".to_vec()).unwrap();
        fixture.wait_for("notes", "SENT_2");
        assert_eq!(*lock(&notices), ["Build finished"]);
        fixture.manager.set_program_notices(false);
        thread::sleep(PROGRAM_NOTICE_INTERVAL);
        fixture.manager.write("notes", 1, b"printf '\\033]9;Second\\007'; echo SENT_$((2+1))\r".to_vec()).unwrap();
        fixture.wait_for("notes", "SENT_3");
        assert_eq!(*lock(&notices), ["Build finished"]);
    }

    #[test]
    fn utf8_carry_over_and_invalid_bytes() {
        let valid = "A你好，世界🫖╭─╮Z";
        for split in 0..=valid.len() {
            let mut decoder = Utf8Decoder::default();
            let first = decoder.decode(&valid.as_bytes()[..split], false);
            let rest = decoder.decode(&valid.as_bytes()[split..], true);
            assert_eq!(first + &rest, valid);
        }
        let invalid: &[u8] = b"ok\xff\xe4\xbd\xa0\xe2\x28\xa1\xf0\x9f";
        let mut decoder = Utf8Decoder::default();
        let mut actual = String::new();
        for byte in invalid {
            actual.push_str(&decoder.decode(&[*byte], false));
        }
        actual.push_str(&decoder.decode(&[], true));
        assert_eq!(actual, String::from_utf8_lossy(invalid));
    }

    #[test]
    fn scrollback_caps_whole_chunks_and_preserves_sequence() {
        let mut buffer = Scrollback::new(10);
        assert_eq!(buffer.append("abc".into()), 1);
        assert_eq!(buffer.append("你好".into()), 2);
        assert_eq!(buffer.append("def".into()), 3);
        assert_eq!(buffer.snapshot().data, "你好def");
        assert_eq!(buffer.snapshot().seq, 3);
        assert_eq!(buffer.chunks.front().map(String::as_str), Some("你好"));
        assert!(buffer.bytes <= 10);
        buffer.append("this chunk exceeds the cap".into());
        assert_eq!(buffer.snapshot().data, "");
        assert_eq!(buffer.snapshot().seq, 4);
    }

    #[test]
    fn terminal_environment_is_clean_and_utf8() {
        let mut command = CommandBuilder::new("/bin/sh");
        let removed_keys = [
            "CLAUDECODE",
            "CLAUDE_CODE_ENTRYPOINT",
            "CLAUDE_CODE_SSE_PORT",
            "CLAUDE_CODE_CHILD_SESSION",
            "CLAUDE_CODE_EXECPATH",
            "CLAUDE_CODE_MESSAGING_SOCKET",
            "CLAUDE_CODE_MESSAGING_TOKEN",
            "CLAUDE_CODE_SESSION_ATTENDED",
            "CLAUDE_CODE_SESSION_ID",
            "CLAUDE_CODE_USE_BEDROCK",
            "CLAUDE_PID",
            "CLAUDE_EFFORT",
            "CODEX_SANDBOX",
            "CODEX_SANDBOX_NETWORK_DISABLED",
            "CODEX_SANDBOX_FUTURE_MARKER",
            "CODEX_THREAD_ID",
            "CODEX_CI",
            "npm_lifecycle_event",
            "npm_config_prefix",
            "INIT_CWD",
            "PNPM_HOME",
        ];
        for key in removed_keys {
            command.env(key, "inherited");
        }
        command.env("LANG", "");
        command.env("APPHUB_TEST_KEEP", "keep");
        command.env("CODEX_HOME", "/tmp/apphub-test-codex-home");
        command.env("CODEX_MODEL", "keep-user-config");
        configure_environment(&mut command);
        let env: HashMap<_, _> = command.iter_full_env_as_str().collect();
        for key in removed_keys {
            assert!(
                !env.contains_key(key),
                "Inherited marker {key} was not removed"
            );
        }
        assert!(!env.keys().any(|key| key.starts_with("CLAUDE_CODE_")
            || key.starts_with("CODEX_SANDBOX")
            || key.starts_with("npm_")
            || key.starts_with("PNPM_")));
        assert_eq!(env["LANG"], "en_US.UTF-8");
        assert_eq!(env["TERM"], "xterm-256color");
        assert_eq!(env["COLORTERM"], "truecolor");
        assert_eq!(env["APPHUB_TEST_KEEP"], "keep");
        assert_eq!(env["CODEX_HOME"], "/tmp/apphub-test-codex-home");
        assert_eq!(env["CODEX_MODEL"], "keep-user-config");
        command.env("LANG", "zh_CN.UTF-8");
        configure_environment(&mut command);
        assert!(command
            .iter_full_env_as_str()
            .any(|(key, value)| key == "LANG" && value == "zh_CN.UTF-8"));

        // Inspect a real child shell without loading rc files that may deliberately re-export markers.
        // Only variable names and known test values are printed, never inherited secret values.
        command.args([
            "-c",
            "/usr/bin/env | /usr/bin/cut -d= -f1; printf '%s\\n' \"CODEX_HOME=$CODEX_HOME\" \"CODEX_MODEL=$CODEX_MODEL\" \"APPHUB_TEST_KEEP=$APPHUB_TEST_KEEP\" \"LANG=$LANG\" \"TERM=$TERM\" \"COLORTERM=$COLORTERM\"",
        ]);
        let pair = native_pty_system().openpty(size(120, 24)).unwrap();
        let mut reader = pair.master.try_clone_reader().unwrap();
        let mut child = pair.slave.spawn_command(command).unwrap();
        drop(pair.slave);
        let mut output = Vec::new();
        let mut buffer = [0; 4096];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(len) => output.extend_from_slice(&buffer[..len]),
                Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(err) if err.raw_os_error() == Some(libc::EIO) => break,
                Err(err) => panic!("Reading child shell environment failed: {err}"),
            }
        }
        assert_eq!(child.wait().unwrap().exit_code(), 0);
        let output = String::from_utf8(output).unwrap();
        let lines: HashSet<_> = output.lines().collect();
        for key in removed_keys {
            assert!(!lines.contains(key), "Child shell inherited marker {key}");
        }
        assert!(!lines.iter().any(|key| key.starts_with("CLAUDE_CODE_")
            || key.starts_with("CODEX_SANDBOX")
            || key.starts_with("npm_")
            || key.starts_with("PNPM_")));
        for expected in [
            "CODEX_HOME=/tmp/apphub-test-codex-home",
            "CODEX_MODEL=keep-user-config",
            "APPHUB_TEST_KEEP=keep",
            "LANG=zh_CN.UTF-8",
            "TERM=xterm-256color",
            "COLORTERM=truecolor",
        ] {
            assert!(
                lines.contains(expected),
                "Child shell did not preserve {expected}"
            );
        }
    }

    #[test]
    fn concurrent_attaches_share_one_pty_and_exited_tabs_cannot_reopen() {
        let fixture = Fixture::new();
        fixture.create("same");
        let handles: Vec<_> = (0..6)
            .map(|_| {
                let manager = fixture.manager.clone();
                let dir = fixture.dir.clone();
                let env = fixture.env.clone();
                thread::spawn(move || {
                    manager
                        .open("same", 1, &dir, 80, 24, false, &env)
                        .unwrap()
                        .session
                })
            })
            .collect();
        let sessions: Vec<_> = handles
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect();
        assert!(sessions
            .iter()
            .all(|session| session.session_id == sessions[0].session_id
                && session.pid == sessions[0].pid));
        assert_eq!(fixture.manager.list().len(), 1);
        fixture.idle("same", 1);
        let exited = fixture.manager.get("same", 1).unwrap();
        fixture.manager.write("same", 1, b"exit 7\n".to_vec()).unwrap();
        fixture.wait_closed(&exited);
        assert_eq!(exited.info().exit_code, Some(7));
        for restart in [false, true] {
            assert!(fixture.manager.open("same", 1, &fixture.dir, 80, 24, restart, &fixture.env).is_err());
        }
        let reopened = fixture.create("same");
        assert_ne!(reopened.session.session_id, sessions[0].session_id);
        assert_ne!(reopened.session.pid, sessions[0].pid);
        assert!(reopened.session.alive);
    }

    #[test]
    fn real_pty_cwd_unicode_resize_and_process_group_cleanup() {
        let fixture = Fixture::new();
        let first = fixture.create("pty");
        let pgid = first.session.pid.unwrap() as i32;
        fixture.idle("pty", 1);
        fixture
            .manager
            .write("pty", 1, b"pwd\necho \xe4\xbd\xa0\xe5\xa5\xbd\n".to_vec())
            .unwrap();
        fixture.wait_for("pty", &format!("\r\n{}\r\n", fixture.dir.display()));
        fixture.wait_for("pty", "\r\n你好\r\n");
        let context = fixture.manager.get("pty", 1).unwrap().context().unwrap();
        assert!(context.shell.is_some());
        assert_eq!(crate::expand_tilde(&context.cwd.unwrap()), fixture.dir.to_string_lossy());
        fixture.manager.resize("pty", 1, 103, 37).unwrap();
        fixture.manager.resize("pty", 1, 0, 0).unwrap(); // Hidden panes must not erase the last good size.
        fixture
            .manager
            .write("pty", 1, b"stty size\n".to_vec())
            .unwrap();
        fixture.wait_for("pty", "37 103");
        // Explicitly exercise the byte input path with a byte that is not UTF-8.
        fixture
            .manager
            .write(
                "pty",
                1,
                b"stty -echo -icanon min 1 time 0; od -An -tu1 -N1; stty sane\n".to_vec(),
            )
            .unwrap();
        thread::sleep(Duration::from_millis(200));
        fixture.manager.write("pty", 1, vec![0xff]).unwrap();
        fixture.wait_for("pty", "255");
        fixture.manager.kill_all();
        let deadline = Instant::now() + Duration::from_secs(3);
        while unsafe { libc::kill(-pgid, 0) } == 0 {
            assert!(
                Instant::now() < deadline,
                "PTY process group {pgid} survived cleanup"
            );
            thread::sleep(Duration::from_millis(20));
        }
        assert!(fixture.manager.list().is_empty());
    }

    #[test]
    fn failed_spawn_leaves_no_session() {
        let fixture = Fixture::new();
        assert!(fixture
            .manager
            .create(
                "missing",
                &fixture.dir.join("does-not-exist"),
                80,
                24,
                &fixture.env
            )
            .unwrap_err()
            .contains("项目目录不存在"));
        assert!(fixture.manager.list().is_empty());
    }
}
