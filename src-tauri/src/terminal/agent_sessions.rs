use super::{await_command, lock, project_config, Session, TerminalManager, TerminalSessionInfo};
use crate::projects::EnvVar;
use crate::terminal_agent::sessions::{Catalog, ProjectSessionSummary, SessionRef, SessionSummary};
use crate::terminal_agent::{agent_log_paths, AgentKind, AgentProcess};
use serde::Serialize;
use std::collections::HashSet;
use std::ffi::{CString, OsString};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::State;

const LAUNCH_WINDOW_MS: u64 = 5_000;
const PREPARE_WINDOW_MS: u64 = 30_000;
static NEXT_LAUNCH: AtomicU64 = AtomicU64::new(1);

struct Binding {
    reference: SessionRef,
    requested_at: u64,
    process: Option<AgentProcess>,
}

struct PendingLaunch {
    id: String,
    command: String,
    reference: Option<SessionRef>,
    expires_at: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreparedAgent {
    session: TerminalSessionInfo,
    launch_id: Option<String>,
}

#[derive(Default)]
pub(super) struct AgentLink {
    binding: Option<Binding>,
    pending: Option<PendingLaunch>,
}

impl AgentLink {
    fn active(&mut self, process: Option<&AgentProcess>, now: u64) -> Option<SessionRef> {
        if let Some(reference) = self.pending.as_ref()
            .filter(|launch| now <= launch.expires_at)
            .and_then(|launch| launch.reference.as_ref())
        {
            return Some(reference.clone());
        }
        let binding = self.binding.as_mut()?;
        if let Some(bound) = binding.process.as_ref() {
            if process == Some(bound) {
                return Some(binding.reference.clone());
            }
        } else if let Some(process) = process {
            if process.resume.as_ref() == Some(&binding.reference)
                && process
                    .started_at
                    .checked_sub(binding.requested_at)
                    .is_some_and(|elapsed| elapsed <= LAUNCH_WINDOW_MS)
            {
                binding.process = Some(process.clone());
                return Some(binding.reference.clone());
            }
        } else if now.saturating_sub(binding.requested_at) <= LAUNCH_WINDOW_MS {
            return Some(binding.reference.clone());
        }
        self.binding = None;
        None
    }
}

impl Session {
    fn agent_process(&self) -> Option<AgentProcess> {
        self.attention.agent_process(&self.key, &self.session_id)
    }
}

impl TerminalManager {
    fn all_sessions(&self) -> Vec<Session> {
        lock(&self.sessions).values().cloned().collect()
    }

    fn associate_agent_sessions(&self, catalog: &Catalog) {
        // Serialize claims with foreground launches.
        let _mutation = lock(&self.mutation);
        let sessions = self.all_sessions();
        let mut claimed = HashSet::new();
        let mut unlinked = Vec::new();
        let now = now();
        for session in &sessions {
            let process = session.agent_process();
            let linked = lock(&session.agent_link).active(process.as_ref(), now);
            if let Some(reference) = linked {
                claimed.insert(reference);
            } else if let Some(process) = process.filter(|process| process.kind.resumable()) {
                unlinked.push((session, process));
            }
        }
        for (session, process) in unlinked {
            // The literal resume argument or a writable log held by the job
            // identifies its session. Cwd and mtime cannot prove ownership.
            let reference = match &process.resume {
                Some(reference) => catalog.find(reference, None).map(|item| item.reference),
                None => catalog.reference_for_logs(process.kind, &agent_log_paths(&process)),
            };
            let Some(reference) = reference.filter(|reference| !claimed.contains(reference)) else {
                continue;
            };
            if session.agent_process().as_ref() != Some(&process) {
                continue;
            }
            let mut link = lock(&session.agent_link);
            if link.active(Some(&process), now).is_none() {
                claimed.insert(reference.clone());
                link.binding = Some(Binding {
                    reference,
                    requested_at: process.started_at,
                    process: Some(process),
                });
            }
        }
    }

    fn prepare_agent(
        &self,
        project: &crate::ProjectConfig,
        argv: Vec<String>,
    ) -> Result<PreparedAgent, String> {
        let (tool, reference) = parse_command(&argv).ok_or("无效的 AI 会话命令")?;
        let project_dir = crate::expand_tilde(&project.path);
        let project_dir = Path::new(&project_dir);
        let summary = reference.as_ref().and_then(|reference| {
            let catalog = self.agent_catalog.refresh();
            self.associate_agent_sessions(&catalog);
            catalog.find(reference, Some(project_dir))
        });
        let cwd = summary
            .as_ref()
            .map_or(project_dir, |session| session.cwd.as_path());
        let available = available_tools(cwd, &project.env).contains(&tool);
        let command = argv.join(" ");
        let launch_id = NEXT_LAUNCH.fetch_add(1, Ordering::Relaxed).to_string();
        let session = {
            let _mutation = lock(&self.mutation);
            let requested_at = now();
            if let Some(reference) = &reference {
                if let Some(existing) = self.all_sessions().into_iter().find(|session| {
                    let process = session.agent_process();
                    lock(&session.agent_link)
                        .active(process.as_ref(), requested_at)
                        .as_ref()
                        == Some(reference)
                }) {
                    return Ok(PreparedAgent {
                        session: existing.info(),
                        launch_id: None,
                    });
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
            let opened = self.create_locked(&project.id, cwd, 80, 24, &project.env)?;
            let session = self.get(&project.id, opened.session.slot)?;
            lock(&session.agent_link).pending = Some(PendingLaunch {
                id: launch_id.clone(),
                command,
                reference,
                expires_at: requested_at + PREPARE_WINDOW_MS,
            });
            session
        };
        Ok(PreparedAgent {
            session: session.info(),
            launch_id: Some(launch_id),
        })
    }

    fn launch_agent(
        &self,
        project_id: &str,
        slot: u8,
        session_id: &str,
        launch_id: &str,
    ) -> Result<TerminalSessionInfo, String> {
        let requested_at = now();
        let (session, result) = {
            let _mutation = lock(&self.mutation);
            let session = self.get(project_id, slot)?;
            if session.session_id != session_id {
                return Err("AI 会话的终端已变更".into());
            }
            let mut link = lock(&session.agent_link);
            if link
                .pending
                .as_ref()
                .is_none_or(|launch| launch.id != launch_id || requested_at > launch.expires_at)
            {
                return Err("AI 会话启动请求已失效".into());
            }
            let launch = link.pending.take().unwrap();
            let result = session.queue_command(launch.command, false)?;
            link.binding = launch.reference.map(|reference| Binding {
                reference,
                requested_at,
                process: None,
            });
            drop(link);
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

#[tauri::command]
pub async fn terminal_agent_menu(
    state: State<'_, TerminalManager>,
    project_id: String,
    cached: Option<bool>,
) -> Result<AgentMenu, String> {
    let manager = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let project = project_config(&project_id)?;
        let path = crate::expand_tilde(&project.path);
        let cwd = Path::new(&path);
        let catalog = if cached == Some(true) {
            manager.agent_catalog.snapshot()
        } else {
            manager.agent_catalog.refresh()
        };
        let sessions = catalog.project_sessions(cwd);
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
    cached: Option<bool>,
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
        let catalog = if cached == Some(true) {
            manager.agent_catalog.snapshot()
        } else {
            manager.agent_catalog.refresh()
        };
        Ok(catalog.all_project_sessions(roots))
    })
    .await
    .map_err(|error| format!("读取项目的 AI 会话失败：{error}"))?
}

#[tauri::command]
pub async fn terminal_agent_start(
    state: State<'_, TerminalManager>,
    project_id: String,
    argv: Vec<String>,
) -> Result<PreparedAgent, String> {
    let manager = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        manager.prepare_agent(&project_config(&project_id)?, argv)
    })
    .await
    .map_err(|error| format!("打开 AI 会话失败：{error}"))?
}

#[tauri::command]
pub async fn terminal_agent_launch(
    state: State<'_, TerminalManager>,
    project_id: String,
    slot: u8,
    session_id: String,
    launch_id: String,
) -> Result<TerminalSessionInfo, String> {
    let manager = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        manager.launch_agent(&project_id, slot, &session_id, &launch_id)
    })
    .await
    .map_err(|error| format!("启动 AI 会话失败：{error}"))?
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
        let mut link = AgentLink::default();
        link.binding = Some(Binding {
            reference: reference.clone(),
            requested_at: 100,
            process: None,
        });
        assert_eq!(link.active(None, 120), Some(reference.clone()));
        let process = AgentProcess {
            kind: AgentKind::Codex,
            pid: 10,
            started_at: 200,
            resume: Some(reference.clone()),
        };
        assert_eq!(link.active(Some(&process), 10_000), Some(reference));
        assert_eq!(
            link.active(
                Some(&AgentProcess {
                    started_at: 12_000,
                    ..process
                }),
                12_001,
            ),
            None
        );
    }

    #[test]
    fn same_kind_processes_cannot_claim_a_pending_resume_without_the_matching_id() {
        let reference = SessionRef::new(AgentKind::Codex, ID).unwrap();
        let other = SessionRef::new(AgentKind::Codex, "383c4fef-fd6e-452a-9130-bdbdea62bc02");
        let mut link = AgentLink::default();
        let mut process = AgentProcess {
            kind: AgentKind::Codex,
            pid: 10,
            started_at: 200,
            resume: None,
        };
        for resume in [None, other] {
            link.binding = Some(Binding {
                reference: reference.clone(),
                requested_at: 100,
                process: None,
            });
            process.resume = resume;
            assert_eq!(link.active(Some(&process), 210), None);
        }
    }
}
