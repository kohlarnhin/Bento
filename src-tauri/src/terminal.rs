use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;
use tauri::{AppHandle, Emitter, State};

const SCROLLBACK_BYTES: usize = 2 * 1024 * 1024;
static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);

// Recover a poisoned lock: a failed worker must not take down the app or other terminals.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|err| err.into_inner())
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSessionInfo {
    pub project_id: String,
    pub session_id: String,
    pub alive: bool,
    pub exit_code: Option<u32>,
    pub pid: Option<u32>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalOutput {
    pub project_id: String,
    pub session_id: String,
    pub seq: u64,
    pub data: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalExit {
    pub project_id: String,
    pub session_id: String,
    pub exit_code: Option<u32>,
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
        reply: mpsc::Sender<Result<(), String>>,
        deadline: std::time::Instant,
    },
}

#[derive(Clone)]
struct Session {
    session_id: String,
    project_id: String,
    pid: Option<u32>,
    state: Arc<(Mutex<SessionState>, Condvar)>,
    master: Arc<Mutex<Option<Box<dyn MasterPty + Send>>>>,
    writer: mpsc::Sender<Input>,
    killer: Arc<Mutex<Box<dyn ChildKiller + Send + Sync>>>,
    scrollback: Arc<Mutex<Scrollback>>,
}

impl Session {
    fn info(&self) -> TerminalSessionInfo {
        let state = lock(&self.state.0);
        TerminalSessionInfo {
            project_id: self.project_id.clone(),
            session_id: self.session_id.clone(),
            alive: state.alive,
            exit_code: state.exit_code,
            pid: self.pid,
        }
    }

    fn result(&self) -> OpenResult {
        OpenResult {
            snapshot: lock(&self.scrollback).snapshot(),
            session: self.info(),
        }
    }

    fn terminate(&self) {
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
}

type OutputSink = Arc<dyn Fn(TerminalOutput) + Send + Sync>;
type ExitSink = Arc<dyn Fn(TerminalExit) + Send + Sync>;

#[derive(Clone)]
pub struct TerminalManager {
    sessions: Arc<Mutex<HashMap<String, Session>>>,
    // Serialize mutations (including concurrent StrictMode opens), never PTY I/O under the map lock.
    mutation: Arc<Mutex<()>>,
    output: OutputSink,
    exit: ExitSink,
}

impl TerminalManager {
    fn new(output: OutputSink, exit: ExitSink) -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            mutation: Arc::new(Mutex::new(())),
            output,
            exit,
        }
    }

    pub fn for_app(app: AppHandle) -> Self {
        let output_app = app.clone();
        Self::new(
            Arc::new(move |event| {
                let _ = output_app.emit("terminal-output", event);
            }),
            Arc::new(move |event| {
                let _ = app.emit("terminal-exit", event);
            }),
        )
    }

    fn get(&self, project_id: &str) -> Result<Session, String> {
        lock(&self.sessions)
            .get(project_id)
            .cloned()
            .ok_or_else(|| "请先打开项目终端".to_string())
    }

    fn open(
        &self,
        project_id: &str,
        cwd: &Path,
        cols: u16,
        rows: u16,
        restart: bool,
        env: &[crate::projects::EnvVar],
    ) -> Result<OpenResult, String> {
        let _mutation = lock(&self.mutation);
        if let Ok(existing) = self.get(project_id) {
            if !restart {
                return Ok(existing.result());
            }
            lock(&self.sessions).remove(project_id);
            existing.terminate();
        }
        if !cwd.is_dir() {
            return Err(format!("项目目录不存在：{}", cwd.display()));
        }
        let session = self.spawn(project_id, cwd, cols.max(1), rows.max(1), env)?;
        let result = session.result();
        lock(&self.sessions).insert(project_id.to_string(), session);
        Ok(result)
    }

    fn spawn(
        &self,
        project_id: &str,
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
            project_id: project_id.to_string(),
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
        };

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
                {
                    let mut state = lock(&waiter_session.state.0);
                    state.alive = false;
                    state.exit_code = exit_code;
                    waiter_session.state.1.notify_all();
                }
                exit(TerminalExit {
                    project_id: waiter_session.project_id,
                    session_id: waiter_session.session_id,
                    exit_code,
                });
            });
        if let Err(err) = waiter {
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
                                } else if shell_group.is_none() || foreground != shell_group {
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
        let output_project = project_id.to_string();
        let output_session = session.session_id.clone();
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
                            project_id: output_project.clone(),
                            session_id: output_session.clone(),
                            seq,
                            data,
                        });
                    };
                    loop {
                        match reader.read(&mut buffer) {
                            Ok(0) => break,
                            Ok(len) => emit(decoder.decode(&buffer[..len], false)),
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
        lock(&self.sessions).values().map(Session::info).collect()
    }

    fn write(&self, project_id: &str, data: Vec<u8>) -> Result<(), String> {
        let session = self.get(project_id)?;
        if !session.info().alive {
            return Err("终端已退出，请重新启动终端".into());
        }
        session
            .writer
            .send(Input::Bytes(data))
            .map_err(|_| "终端输入已断开".to_string())
    }

    fn run(&self, project_id: &str, command: String) -> Result<(), String> {
        if command.trim().is_empty() {
            return Err("请输入要运行的命令".into());
        }
        let session = self.get(project_id)?;
        if !session.info().alive {
            return Err("终端已退出，请重新启动终端".into());
        }
        let (reply, result) = mpsc::channel();
        session
            .writer
            .send(Input::Run {
                command,
                reply,
                deadline: std::time::Instant::now() + Duration::from_secs(5),
            })
            .map_err(|_| "终端输入已断开".to_string())?;
        result
            .recv_timeout(Duration::from_secs(5))
            .map_err(|_| "终端未及时响应，请稍后重试".to_string())?
    }

    fn resize(&self, project_id: &str, cols: u16, rows: u16) -> Result<(), String> {
        if cols == 0 || rows == 0 {
            return Ok(());
        }
        let session = self.get(project_id)?;
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
        let session = lock(&self.sessions).remove(project_id);
        if let Some(session) = session {
            session.terminate();
        }
    }

    pub fn retain_projects(&self, ids: &HashSet<String>) {
        let _mutation = lock(&self.mutation);
        let removed: Vec<_> = {
            let mut sessions = lock(&self.sessions);
            let deleted: Vec<_> = sessions
                .keys()
                .filter(|id| !ids.contains(*id))
                .cloned()
                .collect();
            deleted
                .into_iter()
                .filter_map(|id| sessions.remove(&id))
                .collect()
        };
        for session in removed {
            session.terminate();
        }
    }

    pub fn kill_all(&self) {
        self.retain_projects(&HashSet::new());
    }
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
    cols: u16,
    rows: u16,
) -> Result<OpenResult, String> {
    let manager = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let project = project_config(&project_id)?;
        let path = crate::expand_tilde(&project.path);
        manager.open(
            &project_id,
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
    cols: u16,
    rows: u16,
) -> Result<OpenResult, String> {
    let manager = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let project = project_config(&project_id)?;
        let path = crate::expand_tilde(&project.path);
        manager.open(
            &project_id,
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
pub fn terminal_list(state: State<'_, TerminalManager>) -> Vec<TerminalSessionInfo> {
    state.list()
}

#[tauri::command]
pub async fn terminal_run(
    state: State<'_, TerminalManager>,
    project_id: String,
    command: String,
) -> Result<(), String> {
    let manager = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || manager.run(&project_id, command))
        .await
        .map_err(|err| format!("运行命令失败：{err}"))?
}

#[tauri::command]
pub fn terminal_write(
    state: State<'_, TerminalManager>,
    project_id: String,
    data: String,
) -> Result<(), String> {
    state.write(&project_id, data.into_bytes())
}

#[tauri::command]
pub fn terminal_write_bytes(
    state: State<'_, TerminalManager>,
    project_id: String,
    data: Vec<u8>,
) -> Result<(), String> {
    state.write(&project_id, data)
}

#[tauri::command]
pub fn terminal_resize(
    state: State<'_, TerminalManager>,
    project_id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    state.resize(&project_id, cols, rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::{Instant, SystemTime, UNIX_EPOCH};

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(1);

    struct Fixture {
        manager: TerminalManager,
        dir: PathBuf,
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
            Self {
                manager: TerminalManager::new(Arc::new(|_| {}), Arc::new(|_| {})),
                dir: dir.canonicalize().unwrap(),
            }
        }

        fn wait_for(&self, id: &str, text: &str) -> String {
            let deadline = Instant::now() + Duration::from_secs(15);
            loop {
                let data = self.manager.get(id).unwrap().result().snapshot.data;
                if data.contains(text) {
                    return data;
                }
                assert!(Instant::now() < deadline, "Missing {text:?} in {data:?}");
                thread::sleep(Duration::from_millis(30));
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            self.manager.kill_all();
            let _ = std::fs::remove_dir_all(&self.dir);
        }
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
                thread::spawn(move || {
                    manager
                        .open("same", &dir, 80, 24, false, &[])
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
        fixture.manager.write("same", b"exit 7\n".to_vec()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        while fixture.manager.get("same").unwrap().info().alive {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(30));
        }
        let exited = fixture
            .manager
            .open("same", &fixture.dir, 80, 24, false, &[])
            .unwrap();
        assert_eq!(exited.session.session_id, sessions[0].session_id);
        assert_eq!(exited.session.exit_code, Some(7));
        let restarted = fixture
            .manager
            .open("same", &fixture.dir, 80, 24, true, &[])
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
            .open("pty", &fixture.dir, 80, 24, false, &[])
            .unwrap();
        let pgid = first.session.pid.unwrap() as i32;
        fixture
            .manager
            .write("pty", b"pwd\necho \xe4\xbd\xa0\xe5\xa5\xbd\n".to_vec())
            .unwrap();
        fixture.wait_for("pty", &format!("\r\n{}\r\n", fixture.dir.display()));
        fixture.wait_for("pty", "\r\n你好\r\n");
        fixture.manager.resize("pty", 103, 37).unwrap();
        fixture.manager.resize("pty", 0, 0).unwrap(); // Hidden panes must not erase the last good size.
        fixture
            .manager
            .write("pty", b"stty size\n".to_vec())
            .unwrap();
        fixture.wait_for("pty", "37 103");
        // Explicitly exercise the byte input path with a byte that is not UTF-8.
        fixture
            .manager
            .write(
                "pty",
                b"stty -echo -icanon min 1 time 0; od -An -tu1 -N1; stty sane\n".to_vec(),
            )
            .unwrap();
        thread::sleep(Duration::from_millis(200));
        fixture.manager.write("pty", vec![0xff]).unwrap();
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
                &fixture.dir.join("does-not-exist"),
                80,
                24,
                false,
                &[]
            )
            .unwrap_err()
            .contains("项目目录不存在"));
        assert!(fixture.manager.list().is_empty());
    }
}
