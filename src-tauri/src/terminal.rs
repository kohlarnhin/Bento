use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};
use crate::terminal_attention::{AttentionManager, Sinks as AttentionSinks};

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
    pub alive: bool,
    pub exit_code: Option<u32>,
    pub pid: Option<u32>,
    pub attention: bool,
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
    chunks: VecDeque<(u64, String)>,
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
        self.chunks.push_back((self.seq, data));
        while self.bytes > self.cap {
            if let Some((_, oldest)) = self.chunks.pop_front() {
                self.bytes -= oldest.len();
            }
        }
        self.seq
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            data: self.chunks.iter().map(|(_, text)| text.as_str()).collect(),
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
    key: TerminalKey,
    pid: Option<u32>,
    state: Arc<(Mutex<SessionState>, Condvar)>,
    master: Arc<Mutex<Option<Box<dyn MasterPty + Send>>>>,
    writer: mpsc::Sender<Input>,
    killer: Arc<Mutex<Box<dyn ChildKiller + Send + Sync>>>,
    scrollback: Arc<Mutex<Scrollback>>,
    attention: Arc<AttentionManager>,
}

impl Session {
    fn info(&self) -> TerminalSessionInfo {
        let state = lock(&self.state.0);
        TerminalSessionInfo {
            project_id: self.key.project_id.clone(),
            slot: self.key.slot,
            session_id: self.session_id.clone(),
            alive: state.alive,
            exit_code: state.exit_code,
            pid: self.pid,
            attention: self.attention.raised(&self.key, &self.session_id),
        }
    }

    fn result(&self) -> OpenResult {
        OpenResult {
            snapshot: lock(&self.scrollback).snapshot(),
            session: self.info(),
        }
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
}

type OutputSink = Arc<dyn Fn(TerminalOutput) + Send + Sync>;
type ExitSink = Arc<dyn Fn(TerminalExit) + Send + Sync>;
type OpenedSink = Arc<dyn Fn(TerminalSessionInfo) + Send + Sync>;
type ClosedSink = Arc<dyn Fn(TerminalClosed) + Send + Sync>;
type LimitSource = Arc<dyn Fn() -> Result<u8, String> + Send + Sync>;

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
        }
    }

    pub fn for_app(app: AppHandle) -> Self {
        let output_app = app.clone();
        let exit_app = app.clone();
        let opened_app = app.clone();
        let closed_app = app.clone();
        let change_app = app.clone();
        let badge_app = app.clone();
        let inactive_app = app.clone();
        Self::with_attention(
            Arc::new(move |event| {
                let _ = output_app.emit("terminal-output", event);
            }),
            Arc::new(move |event| {
                let _ = exit_app.emit("terminal-exit", event);
            }),
            Arc::new(move |event| { let _ = opened_app.emit("terminal-opened", event); }),
            Arc::new(move |event| { let _ = closed_app.emit("terminal-closed", event); }),
            Arc::new(|| crate::settings::max_per_project(&crate::open_project_db()?)),
            AttentionSinks {
                change: Arc::new(move |event| { let _ = change_app.emit("terminal-attention", event); }),
                badge: Arc::new(move |count| {
                    if let Some(window) = badge_app.get_webview_window("main") {
                        let _ = window.set_badge_count((count > 0).then_some(count as i64));
                    }
                    #[cfg(debug_assertions)]
                    crate::debug_e2e::record_terminal_badge(count);
                }),
                inactive: Arc::new(move || {
                    inactive_app.get_webview_window("main").is_none_or(|window| {
                        !window.is_visible().unwrap_or(false) || !window.is_focused().unwrap_or(false)
                    })
                }),
                bounce: Arc::new(move || {
                    if let Some(window) = app.get_webview_window("main") {
                        let _ = window.request_user_attention(Some(tauri::UserAttentionType::Informational));
                    }
                    #[cfg(debug_assertions)]
                    crate::debug_e2e::record_terminal_bounce();
                }),
            },
        )
    }

    fn get(&self, project_id: &str, slot: u8) -> Result<Session, String> {
        lock(&self.sessions)
            .get(&TerminalKey::new(project_id, slot))
            .cloned()
            .ok_or_else(|| "该终端已关闭".to_string())
    }

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
        let existing = self.get(project_id, slot).ok();
        if let Some(existing) = &existing {
            if !restart {
                return Ok(existing.result());
            }
            existing.terminate();
        } else if slot > 1 {
            return Err("该终端已关闭".into());
        }
        let result = self.spawn_and_insert(project_id, slot, cwd, cols, rows, env);
        if result.is_err() {
            if let Some(existing) = existing {
                lock(&self.sessions).remove(&existing.key);
                self.emit_closed(&existing);
            }
        }
        result
    }

    fn spawn_and_insert(&self, project_id: &str, slot: u8, cwd: &Path, cols: u16, rows: u16, env: &[crate::projects::EnvVar]) -> Result<OpenResult, String> {
        if !cwd.is_dir() {
            return Err(format!("项目目录不存在：{}", cwd.display()));
        }
        let session = self.spawn(project_id, slot, cwd, cols.max(1), rows.max(1), env)?;
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
        self.spawn_and_insert(project_id, slot, cwd, cols, rows, env)
    }

    fn spawn(
        &self,
        project_id: &str,
        slot: u8,
        cwd: &Path,
        cols: u16,
        rows: u16,
        env: &[crate::projects::EnvVar],
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
        let mut command = CommandBuilder::new_default_prog(); // Login shell: loads .zprofile and .zshrc.
        command.cwd(cwd);
        configure_environment(&mut command);
        for entry in env {
            command.env(&entry.key, &entry.value);
        }
        let child = pair
            .slave
            .spawn_command(command)
            .map_err(|err| format!("启动终端失败：{err}"))?;
        drop(pair.slave); // Otherwise EOF is never observed by the master reader.
        let pid = child.process_id();
        let killer = child.clone_killer();
        let (sender, receiver) = mpsc::channel::<Input>();
        let session = Session {
            session_id: format!(
                "{}-{}",
                std::process::id(),
                NEXT_SESSION.fetch_add(1, Ordering::Relaxed)
            ),
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
        };
        self.attention.register(&session.key, &session.session_id);

        // Retain ownership outside the closure until the waiter has successfully started.
        let pending_child = Arc::new(Mutex::new(Some(child)));
        let waiter_child = pending_child.clone();
        let waiter_session = session.clone();
        let exit = self.exit.clone();
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
                exit(TerminalExit {
                    project_id: waiter_session.key.project_id,
                    slot: waiter_session.key.slot,
                    session_id: waiter_session.session_id,
                    exit_code,
                });
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
        let writer_result =
            thread::Builder::new()
                .name("terminal-writer".into())
                .spawn(move || {
                    while let Ok(input) = receiver.recv() {
                        match input {
                            Input::Bytes(data) => {
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
        let reader_result =
            thread::Builder::new()
                .name("terminal-reader".into())
                .spawn(move || {
                    let mut decoder = Utf8Decoder::default();
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
                                attention.output(&output_key, &output_session, &buffer[..len]);
                                emit(decoder.decode(&buffer[..len], false));
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
        sessions.sort_by(|a, b| a.project_id.cmp(&b.project_id).then(a.slot.cmp(&b.slot)));
        sessions
    }

    fn write(&self, project_id: &str, slot: u8, data: Vec<u8>) -> Result<(), String> {
        let session = self.get(project_id, slot)?;
        if !session.info().alive {
            return Err("终端已退出，请重新启动终端".into());
        }
        session
            .writer
            .send(Input::Bytes(data))
            .map_err(|_| "终端输入已断开".to_string())
    }

    fn run(&self, project_id: &str, slot: u8, cwd: &Path, command: String, env: &[crate::projects::EnvVar]) -> Result<RunResult, String> {
        validate_slot(slot)?;
        if command.trim().is_empty() {
            return Err("请输入要运行的命令".into());
        }
        let (reply, result) = mpsc::channel();
        let used_slot;
        {
            let _mutation = lock(&self.mutation);
            let mut sessions: Vec<_> = lock(&self.sessions).values()
                .filter(|session| session.key.project_id == project_id).cloned().collect();
            sessions.sort_by_key(|session| (session.key.slot != slot, session.key.slot));
            let (session, check_idle) = if let Some(session) = sessions.iter().find(|session| session.idle()) {
                (session.clone(), true)
            } else if let Some(session) = sessions.iter().find(|session| !session.info().alive) {
                self.open_locked(project_id, session.key.slot, cwd, 80, 24, true, env)?;
                (self.get(project_id, session.key.slot)?, false)
            } else if sessions.len() < (self.limit)()?.clamp(1, 6) as usize || sessions.is_empty() {
                let opened = self.create_locked(project_id, cwd, 80, 24, env)?;
                (self.get(project_id, opened.session.slot)?, false)
            } else {
                return Err("所有终端都在运行程序，请结束其中一个，或在设置中调高终端数量上限".into());
            };
            used_slot = session.key.slot;
            session.writer.send(Input::Run {
                command,
                check_idle,
                reply,
                deadline: std::time::Instant::now() + Duration::from_secs(5),
            })
            .map_err(|_| "终端输入已断开".to_string())?;
        }
        // Neither the map nor the mutation lock is held while the PTY writer replies.
        result
            .recv_timeout(Duration::from_secs(5))
            .map_err(|_| "终端未及时响应，请稍后重试".to_string())??;
        Ok(RunResult { slot: used_slot })
    }

    fn resize(&self, project_id: &str, slot: u8, cols: u16, rows: u16) -> Result<(), String> {
        if cols == 0 || rows == 0 {
            return Ok(());
        }
        let session = self.get(project_id, slot)?;
        self.attention.resize(&session.key, &session.session_id);
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
        self.remove_matching(|key| key.project_id == project_id);
    }

    fn close_terminal(&self, project_id: &str, slot: u8) -> Result<(), String> {
        validate_slot(slot)?;
        let _mutation = lock(&self.mutation);
        self.remove_matching(|key| key.project_id == project_id && key.slot == slot);
        Ok(())
    }

    fn emit_closed(&self, session: &Session) {
        (self.closed)(TerminalClosed {
            project_id: session.key.project_id.clone(), slot: session.key.slot,
            session_id: session.session_id.clone(),
        });
    }

    fn remove_matching(&self, remove: impl Fn(&TerminalKey) -> bool) {
        let removed: Vec<_> = {
            let mut sessions = lock(&self.sessions);
            let keys: Vec<_> = sessions.keys().filter(|key| remove(key)).cloned().collect();
            keys.into_iter().filter_map(|key| sessions.remove(&key)).collect()
        };
        for session in removed {
            session.terminate();
            self.emit_closed(&session);
        }
    }

    pub fn retain_projects(&self, ids: &HashSet<String>) {
        let _mutation = lock(&self.mutation);
        self.remove_matching(|key| !ids.contains(&key.project_id));
    }

    pub fn kill_all(&self) {
        self.retain_projects(&HashSet::new());
        self.attention.shutdown();
    }

    pub fn focus(&self, key: Option<TerminalKey>) {
        self.attention.focus(key);
    }
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

fn configure_environment(command: &mut CommandBuilder) {
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
) -> Result<OpenResult, String> {
    let manager = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let project = project_config(&project_id)?;
        let path = crate::expand_tilde(&project.path);
        manager.open(
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
) -> Result<OpenResult, String> {
    let manager = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let project = project_config(&project_id)?;
        let path = crate::expand_tilde(&project.path);
        manager.open(
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
    state: State<'_, TerminalManager>, project_id: String, slot: u8,
) -> Result<(), String> {
    let manager = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || manager.close_terminal(&project_id, slot))
        .await.map_err(|err| format!("关闭终端失败：{err}"))?
}

#[tauri::command]
pub fn terminal_list(state: State<'_, TerminalManager>) -> Vec<TerminalSessionInfo> {
    state.list()
}

#[tauri::command]
pub fn terminal_focus(state: State<'_, TerminalManager>, project_id: Option<String>, slot: Option<u8>) {
    let key = project_id.zip(slot).map(|(project_id, slot)| TerminalKey { project_id, slot });
    #[cfg(debug_assertions)]
    crate::debug_e2e::record_terminal_focus(key.as_ref());
    state.focus(key);
}

#[tauri::command]
pub async fn terminal_run(
    state: State<'_, TerminalManager>,
    project_id: String,
    slot: u8,
    command: String,
) -> Result<RunResult, String> {
    let manager = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let project = project_config(&project_id)?;
        manager.run(&project_id, slot, Path::new(&crate::expand_tilde(&project.path)), command, &project.env)
    })
        .await
        .map_err(|err| format!("运行命令失败：{err}"))?
}

#[tauri::command]
pub fn terminal_write(
    state: State<'_, TerminalManager>,
    project_id: String,
    slot: u8,
    data: String,
) -> Result<(), String> {
    state.write(&project_id, slot, data.into_bytes())
}

#[tauri::command]
pub fn terminal_write_bytes(
    state: State<'_, TerminalManager>,
    project_id: String,
    slot: u8,
    data: Vec<u8>,
) -> Result<(), String> {
    state.write(&project_id, slot, data)
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
            self.wait_until(id, slot, "shell exited", |session| !session.info().alive);
            assert_eq!(session.info().exit_code, Some(0));
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
            self.manager.run(id, slot, &self.dir, command.into(), &self.env).unwrap()
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
    fn create_enforces_live_db_limit_counts_exited_slots_and_reuses_smallest_free_slot() {
        let fixture = Fixture::new();
        assert!(fixture.manager.open("a", 2, &fixture.dir, 80, 24, false, &fixture.env).unwrap_err().contains("该终端已关闭"));
        assert!(fixture.manager.open("a", 2, &fixture.dir, 80, 24, true, &fixture.env).is_err());
        assert!(fixture.manager.open("a", 0, &fixture.dir, 80, 24, false, &fixture.env).is_err());
        assert_eq!(fixture.create("a").session.slot, 1);
        let two = fixture.create("a");
        assert_eq!(two.session.slot, 2);
        fixture.exit("a", 2);
        assert_eq!(fixture.manager.create("a", &fixture.dir, 80, 24, &fixture.env).unwrap_err(), "每个项目最多 2 个终端，可在设置中调整");
        fixture.limit(3);
        assert_eq!(fixture.create("a").session.slot, 3);
        fixture.manager.open("a", 2, &fixture.dir, 80, 24, true, &fixture.env).unwrap();
        let before = fixture.manager.list();
        fixture.limit(1);
        assert_eq!(fixture.manager.list().iter().map(|s| &s.session_id).collect::<Vec<_>>(), before.iter().map(|s| &s.session_id).collect::<Vec<_>>());
        assert!(fixture.manager.list().iter().all(|s| s.alive));
        assert!(fixture.manager.create("a", &fixture.dir, 80, 24, &fixture.env).is_err());
        let restarted = fixture.manager.open("a", 3, &fixture.dir, 80, 24, true, &fixture.env).unwrap();
        assert_ne!(restarted.session.session_id, before[2].session_id);
        fixture.manager.close_terminal("a", 2).unwrap();
        assert!(fixture.manager.create("a", &fixture.dir, 80, 24, &fixture.env).is_err());
        fixture.limit(3);
        assert_eq!(fixture.create("a").session.slot, 2);
        assert_eq!(fixture.manager.list().iter().map(|s| s.slot).collect::<Vec<_>>(), [1, 2, 3]);
        assert_eq!(lock(&fixture.opened).len(), 6);
        assert_eq!(lock(&fixture.closed).iter().map(|s| s.slot).collect::<Vec<_>>(), [2]);
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
    fn run_routes_preferred_idle_other_idle_preferred_exited_then_lowest_exited() {
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
        let error = fixture.manager.run("a", 1, &fixture.dir, "FORBIDDEN".into(), &fixture.env).unwrap_err();
        assert_eq!(error, "所有终端都在运行程序，请结束其中一个，或在设置中调高终端数量上限");
        assert!(fixture.manager.list().iter().all(|s| !fixture.manager.get("a", s.slot).unwrap().result().snapshot.data.contains("FORBIDDEN")));
        fixture.exit("a", 2);
        fixture.exit("a", 3);
        let old = fixture.manager.get("a", 3).unwrap().session_id;
        fixture.limit(1); // Restarting an exited slot above the lowered limit is still allowed.
        assert_eq!(fixture.run("a", 3, "printf 'restarted-preferred\\n'").slot, 3);
        fixture.wait_for_slot("a", 3, "\r\nrestarted-preferred\r\n");
        assert_ne!(fixture.manager.get("a", 3).unwrap().session_id, old);
        fixture.busy("a", 3);
        assert_eq!(fixture.run("a", 1, "printf 'restarted-lowest\\n'").slot, 2);
        fixture.wait_for_slot("a", 2, "\r\nrestarted-lowest\r\n");
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
        fixture.manager.open("bell", 1, &fixture.dir, 80, 24, false, &fixture.env).unwrap();
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
        fixture.manager.write("bell", 1, b"exit\n".to_vec()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while fixture.manager.list()[0].alive {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        }
        assert!(!fixture.manager.list()[0].attention);
        fixture.manager.close("bell");
        assert!(fixture.manager.list().is_empty());
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
        assert_eq!(buffer.chunks.front().unwrap().0, 2);
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
    fn concurrent_get_or_create_and_exited_session_are_stable() {
        let fixture = Fixture::new();
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
        fixture.manager.write("same", 1, b"exit 7\n".to_vec()).unwrap();
        fixture.wait_until("same", 1, "shell exited with status 7", |session| !session.info().alive);
        let exited = fixture
            .manager
            .open("same", 1, &fixture.dir, 80, 24, false, &fixture.env)
            .unwrap();
        assert_eq!(exited.session.session_id, sessions[0].session_id);
        assert_eq!(exited.session.exit_code, Some(7));
        let restarted = fixture
            .manager
            .open("same", 1, &fixture.dir, 80, 24, true, &fixture.env)
            .unwrap();
        assert_ne!(restarted.session.session_id, sessions[0].session_id);
        assert_ne!(restarted.session.pid, sessions[0].pid);
        assert!(restarted.session.alive);
    }

    #[test]
    fn real_pty_cwd_unicode_resize_and_process_group_cleanup() {
        let fixture = Fixture::new();
        let first = fixture
            .manager
            .open("pty", 1, &fixture.dir, 80, 24, false, &fixture.env)
            .unwrap();
        let pgid = first.session.pid.unwrap() as i32;
        fixture.idle("pty", 1);
        fixture
            .manager
            .write("pty", 1, b"pwd\necho \xe4\xbd\xa0\xe5\xa5\xbd\n".to_vec())
            .unwrap();
        fixture.wait_for("pty", &format!("\r\n{}\r\n", fixture.dir.display()));
        fixture.wait_for("pty", "\r\n你好\r\n");
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
            .open(
                "missing",
                1,
                &fixture.dir.join("does-not-exist"),
                80,
                24,
                false,
                &fixture.env
            )
            .unwrap_err()
            .contains("项目目录不存在"));
        assert!(fixture.manager.list().is_empty());
    }
}
