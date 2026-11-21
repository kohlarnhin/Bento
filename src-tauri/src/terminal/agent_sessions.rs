use super::{
    await_command, lock, project_config, validate_slot, Session, TerminalManager,
    TerminalSessionInfo,
};
use crate::processes::{process_cwd, quote_shell};
use crate::projects::EnvVar;
use crate::terminal_agent::sessions::{Catalog, ProjectSessionSummary, SessionRef, SessionSummary};
use crate::terminal_agent::{AgentKind, AgentProcess};
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::ffi::{CString, OsString};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::State;

const LAUNCH_WINDOW_MS: u64 = 5_000;

struct Binding {
    reference: SessionRef,
    requested_at: u64,
    process: Option<AgentProcess>,
}

pub(super) struct AgentLink {
    pub(super) recovery: Option<SessionRef>,
    binding: Option<Binding>,
}

impl AgentLink {
    pub(super) fn new(recovery: Option<SessionRef>) -> Self {
        Self {
            recovery,
            binding: None,
        }
    }

    fn active(
        &mut self,
        process: Option<AgentProcess>,
        now: u64,
        pending: bool,
    ) -> Option<SessionRef> {
        let binding = self.binding.as_mut()?;
        if let Some(bound) = binding.process {
            if process == Some(bound) {
                return Some(binding.reference.clone());
            }
        } else if let Some(process) = process {
            if process.kind == binding.reference.tool
                && process
                    .started_at
                    .checked_sub(binding.requested_at)
                    .is_some_and(|elapsed| elapsed <= LAUNCH_WINDOW_MS)
            {
                binding.process = Some(process);
                return Some(binding.reference.clone());
            }
        } else if now.saturating_sub(binding.requested_at) <= LAUNCH_WINDOW_MS {
            return pending.then(|| binding.reference.clone());
        }
        self.binding = None;
        None
    }
}

impl Session {
    fn agent_process(&self) -> Option<AgentProcess> {
        self.attention.agent_process(&self.key, &self.session_id)
    }

    pub(super) fn saved_agent_reference(&self) -> Option<SessionRef> {
        let process = self.agent_process();
        let mut link = lock(&self.agent_link);
        link.active(process, now(), false)
            .or_else(|| process.is_none().then(|| link.recovery.clone()).flatten())
    }
}

impl TerminalManager {
    fn all_sessions(&self) -> Vec<Session> {
        lock(&self.sessions).values().cloned().collect()
    }

    fn associate_agent_sessions(&self, catalog: &Catalog, sessions: &[Session]) {
        let mut claimed = HashSet::new();
        let mut unlinked = Vec::new();
        let now = now();
        for session in sessions {
            let process = session.agent_process();
            let linked = lock(&session.agent_link).active(process, now, true);
            if let Some(reference) = linked {
                claimed.insert(reference);
            } else if let Some(process) = process {
                unlinked.push((session, process));
            }
        }
        // Start with the newest process, so two terminals do not claim the same log.
        unlinked.sort_unstable_by_key(|(session, process)| {
            (
                Reverse(process.started_at),
                &session.key.project_id,
                session.key.slot,
            )
        });
        for (session, process) in unlinked {
            let cwd = process_cwd(process.pid).unwrap_or_else(|| session.current_directory());
            let Some(reference) =
                catalog.written_since(process.kind, &cwd, process.started_at, &claimed)
            else {
                continue;
            };
            if session.agent_process() != Some(process) {
                continue;
            }
            let mut link = lock(&session.agent_link);
            if link.active(Some(process), now, true).is_none() {
                claimed.insert(reference.clone());
                link.binding = Some(Binding {
                    reference,
                    requested_at: process.started_at,
                    process: Some(process),
                });
            }
        }
    }

    pub fn prepare_agent_links(&self) {
        if !self.store.as_ref().is_some_and(|store| lock(store).enabled) {
            return;
        }
        let sessions = self.all_sessions();
        let now = now();
        if sessions.iter().any(|session| {
            session.agent_process().is_some_and(|process| {
                lock(&session.agent_link)
                    .active(Some(process), now, false)
                    .is_none()
            })
        }) {
            let mut catalog = lock(&self.agent_catalog);
            catalog.refresh();
            self.associate_agent_sessions(&catalog, &sessions);
        }
    }

    fn agent_recoveries(&self) -> HashMap<String, SessionSummary> {
        let pending: Vec<_> = self
            .all_sessions()
            .into_iter()
            .filter_map(|session| {
                let recovery = lock(&session.agent_link).recovery.clone()?;
                Some((session.session_id, recovery))
            })
            .collect();
        if pending.is_empty() {
            return HashMap::new();
        }
        let mut catalog = lock(&self.agent_catalog);
        catalog.refresh();
        pending
            .into_iter()
            .filter_map(|(id, reference)| Some((id, catalog.find(&reference, None)?)))
            .collect()
    }

    fn start_agent(
        &self,
        project: &crate::ProjectConfig,
        argv: Vec<String>,
        recovery: Option<RecoveryTarget>,
    ) -> Result<TerminalSessionInfo, String> {
        let (tool, reference) = parse_command(&argv).ok_or("无效的 AI 会话命令")?;
        let project_dir = crate::expand_tilde(&project.path);
        let project_dir = Path::new(&project_dir);
        let summary = reference.as_ref().and_then(|reference| {
            let mut catalog = lock(&self.agent_catalog);
            catalog.refresh();
            self.associate_agent_sessions(&catalog, &self.all_sessions());
            catalog.find(reference, recovery.is_none().then_some(project_dir))
        });
        let cwd = summary
            .as_ref()
            .map_or(project_dir, |session| session.cwd.as_path());
        let available = available_tools(cwd, &project.env).contains(&tool);
        let mut command = argv.join(" ");
        let requested_at;
        let (session, result) = {
            let _mutation = lock(&self.mutation);
            let current = if let Some(target) = recovery.as_ref() {
                validate_slot(target.slot)?;
                let session = self.get(&project.id, target.slot)?;
                let expected = lock(&session.agent_link).recovery.clone();
                if session.session_id != target.session_id
                    || expected.is_none()
                    || expected != reference
                {
                    return Err("这个终端的待恢复会话已变更".into());
                }
                Some(session)
            } else {
                None
            };
            requested_at = now();
            if let Some(reference) = &reference {
                if let Some(existing) = self.all_sessions().into_iter().find(|session| {
                    let process = session.agent_process();
                    lock(&session.agent_link)
                        .active(process, requested_at, true)
                        .as_ref()
                        == Some(reference)
                }) {
                    return Ok(existing.info());
                }
                if summary.is_none() {
                    return Err("找不到该 AI 会话的本地记录".into());
                }
            }
            if !available {
                return Err(format!("PATH 中找不到 {} CLI", tool.label()));
            }
            if !cwd.is_dir() {
                return Err(format!("会话目录不存在：{}", cwd.display()));
            }
            let check_idle = current.is_some();
            let session = match current {
                Some(session) => {
                    if !session.idle() {
                        return Err("当前终端中有程序正在运行".into());
                    }
                    if cwd != session.current_directory() {
                        let cwd = cwd
                            .to_str()
                            .filter(|cwd| !cwd.chars().any(char::is_control))
                            .ok_or("会话目录无法在当前终端中打开")?;
                        command = format!("cd -- {} && {command}", quote_shell(cwd));
                    }
                    session
                }
                None => {
                    let opened = self.create_locked(&project.id, cwd, 80, 24, &project.env)?;
                    self.get(&project.id, opened.session.slot)?
                }
            };
            let result = session.queue_command(command, check_idle)?;
            if let Some(reference) = reference.clone() {
                lock(&session.agent_link).binding = Some(Binding {
                    reference,
                    requested_at,
                    process: None,
                });
            }
            (session, result)
        };
        let result = await_command(result);
        {
            let mut link = lock(&session.agent_link);
            if result.is_err()
                && link
                    .binding
                    .as_ref()
                    .is_some_and(|binding| binding.requested_at == requested_at)
            {
                link.binding = None;
            }
            if result.is_ok() && recovery.is_some() {
                link.recovery = None;
            }
        }
        result?;
        Ok(session.info())
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn parse_command(argv: &[String]) -> Option<(AgentKind, Option<SessionRef>)> {
    let [tool, args @ ..] = argv else {
        return None;
    };
    let (tool, resume) = match tool.as_str() {
        "claude" => (AgentKind::Claude, "--resume"),
        "codex" => (AgentKind::Codex, "resume"),
        _ => return None,
    };
    match args {
        [] => Some((tool, None)),
        [flag, id] if flag == resume => Some((tool, Some(SessionRef::new(tool, id)?))),
        _ => None,
    }
}

fn available_tools(cwd: &Path, env: &[EnvVar]) -> Vec<AgentKind> {
    static LOGIN_PATH: OnceLock<Option<OsString>> = OnceLock::new();
    let path = env
        .iter()
        .find(|entry| entry.key == "PATH")
        .map(|entry| OsString::from(&entry.value))
        .or_else(|| {
            LOGIN_PATH
                .get_or_init(|| {
                    crate::shell_env::capture()
                        .ok()
                        .and_then(|mut env| env.remove("PATH").map(Into::into))
                        .or_else(|| std::env::var_os("PATH"))
                })
                .clone()
        });
    let Some(path) = path else {
        return Vec::new();
    };
    [(AgentKind::Claude, "claude"), (AgentKind::Codex, "codex")]
        .into_iter()
        .filter_map(|(tool, command)| {
            std::env::split_paths(&path)
                .any(|directory| {
                    let path = cwd.join(directory).join(command);
                    path.is_file()
                        && CString::new(path.as_os_str().as_bytes()).is_ok_and(|path| unsafe {
                            libc::access(path.as_ptr(), libc::X_OK) == 0
                        })
                })
                .then_some(tool)
        })
        .collect()
}

#[derive(Serialize)]
pub struct AgentMenu {
    tools: Vec<AgentKind>,
    sessions: Vec<SessionSummary>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryTarget {
    slot: u8,
    session_id: String,
}

#[tauri::command]
pub async fn terminal_agent_menu(
    state: State<'_, TerminalManager>,
    project_id: String,
) -> Result<AgentMenu, String> {
    let manager = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let project = project_config(&project_id)?;
        let path = crate::expand_tilde(&project.path);
        let cwd = Path::new(&path);
        let sessions = {
            let mut catalog = lock(&manager.agent_catalog);
            catalog.refresh();
            catalog.project_sessions(cwd)
        };
        Ok(AgentMenu {
            tools: available_tools(cwd, &project.env),
            sessions,
        })
    })
    .await
    .map_err(|error| format!("读取 AI 会话失败：{error}"))?
}

#[tauri::command]
pub async fn terminal_agent_project_sessions(
    state: State<'_, TerminalManager>,
) -> Result<Vec<ProjectSessionSummary>, String> {
    let manager = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let projects = crate::get_projects_sync()?.projects;
        if projects.is_empty() {
            return Ok(Vec::new());
        }
        let roots = projects.iter().map(|project| {
            (
                project.id.as_str(),
                PathBuf::from(crate::expand_tilde(&project.path)),
            )
        });
        let mut catalog = lock(&manager.agent_catalog);
        catalog.refresh();
        Ok(catalog.all_project_sessions(roots))
    })
    .await
    .map_err(|error| format!("读取项目的 AI 会话失败：{error}"))?
}

#[tauri::command]
pub async fn terminal_agent_recoveries(
    state: State<'_, TerminalManager>,
) -> Result<HashMap<String, SessionSummary>, String> {
    let manager = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || manager.agent_recoveries())
        .await
        .map_err(|error| format!("读取待恢复的 AI 会话失败：{error}"))
}

#[tauri::command]
pub async fn terminal_agent_start(
    state: State<'_, TerminalManager>,
    project_id: String,
    argv: Vec<String>,
    recovery: Option<RecoveryTarget>,
) -> Result<TerminalSessionInfo, String> {
    let manager = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        manager.start_agent(&project_config(&project_id)?, argv, recovery)
    })
    .await
    .map_err(|error| format!("打开 AI 会话失败：{error}"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "383c4fef-fd6e-452a-9130-bdbdea62bc01";

    #[test]
    fn accepts_only_the_two_agent_commands_and_literal_session_ids() {
        let parse = |argv: &[&str]| {
            parse_command(&argv.iter().map(|arg| arg.to_string()).collect::<Vec<_>>())
        };
        assert_eq!(parse(&["claude"]), Some((AgentKind::Claude, None)));
        assert_eq!(
            parse(&["codex", "resume", ID]),
            Some((AgentKind::Codex, SessionRef::new(AgentKind::Codex, ID)))
        );
        assert!(parse(&["claude", "--resume", ID]).is_some());
        assert!(parse(&["claude", "resume", ID]).is_none());
        assert!(parse(&["codex", "resume", "; echo invalid"]).is_none());
        assert!(parse(&["codex", "resume", ID, "extra"]).is_none());
    }

    #[test]
    fn binds_an_explicit_resume_when_the_process_started_and_ignores_replacements() {
        let reference = SessionRef::new(AgentKind::Codex, ID).unwrap();
        let mut link = AgentLink::new(None);
        link.binding = Some(Binding {
            reference: reference.clone(),
            requested_at: 100,
            process: None,
        });
        assert_eq!(link.active(None, 120, true), Some(reference.clone()));
        let process = AgentProcess {
            kind: AgentKind::Codex,
            pid: 10,
            started_at: 200,
        };
        assert_eq!(link.active(Some(process), 10_000, false), Some(reference));
        assert_eq!(
            link.active(
                Some(AgentProcess {
                    started_at: 12_000,
                    ..process
                }),
                12_001,
                true
            ),
            None
        );
    }
}
