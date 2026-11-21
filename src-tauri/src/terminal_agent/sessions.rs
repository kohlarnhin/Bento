use super::AgentKind;
use chrono::{DateTime, Datelike, NaiveDate};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs::{self, DirEntry, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(test)]
mod tests;

const HEAD_LINES: usize = 96;
const HEAD_BYTES: u64 = 256 * 1024;
const TAIL_BYTES: u64 = 128 * 1024;
const RECENT: u64 = 90 * 24 * 60 * 60 * 1000;
const PROJECT_LIMIT: usize = 20;

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct SessionRef {
    pub tool: AgentKind,
    pub id: String,
}

impl SessionRef {
    pub fn new(tool: AgentKind, id: &str) -> Option<Self> {
        // Both CLI formats use UUIDs. Only these IDs may become shell arguments.
        let valid = id.len() == 36
            && id.bytes().enumerate().all(|(index, byte)| {
                if matches!(index, 8 | 13 | 18 | 23) {
                    byte == b'-'
                } else {
                    byte.is_ascii_hexdigit()
                }
            });
        valid.then(|| Self {
            tool,
            id: id.to_ascii_lowercase(),
        })
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    #[serde(flatten)]
    pub reference: SessionRef,
    pub cwd: PathBuf,
    pub title: String,
    pub started_at: u64,
    pub last_at: u64,
    pub duration_ms: u64,
    pub file_count: Option<usize>,
}

struct IndexedSession {
    summary: SessionSummary,
    canonical_cwd: PathBuf,
}

struct CachedFile {
    modified: SystemTime,
    len: u64,
    parsed: Option<IndexedSession>,
}

pub struct Catalog {
    home: Option<PathBuf>,
    files: HashMap<PathBuf, CachedFile>,
}

impl Catalog {
    pub fn new(home: Option<PathBuf>) -> Self {
        Self {
            home,
            files: HashMap::new(),
        }
    }

    pub fn refresh(&mut self) {
        self.refresh_at(unix_millis(SystemTime::now()));
    }

    fn refresh_at(&mut self, now: u64) {
        let Some(home) = &self.home else {
            return;
        };
        let cutoff = now.saturating_sub(RECENT);
        let claude: Vec<_> = children(&home.join(".claude/projects"), true).collect();
        let codex = codex_days(&home.join(".codex/sessions"), cutoff);
        let mut seen = HashSet::new();
        for (tool, dirs) in [(AgentKind::Claude, claude), (AgentKind::Codex, codex)] {
            for directory in dirs {
                for entry in children(&directory.path(), false) {
                    let path = entry.path();
                    if !session_filename(tool, &path) {
                        continue;
                    }
                    let Ok(metadata) = entry.metadata() else {
                        continue;
                    };
                    let Ok(modified) = metadata.modified() else {
                        continue;
                    };
                    if unix_millis(modified) < cutoff {
                        continue;
                    }
                    let len = metadata.len();
                    if self
                        .files
                        .get(&path)
                        .is_none_or(|file| file.modified != modified || file.len != len)
                    {
                        let parsed = read_session(tool, &path, len, unix_millis(modified))
                            .ok()
                            .flatten();
                        self.files.insert(
                            path.clone(),
                            CachedFile {
                                modified,
                                len,
                                parsed,
                            },
                        );
                    }
                    seen.insert(path);
                }
            }
        }
        self.files.retain(|path, _| seen.contains(path));
    }

    fn sessions(&self) -> impl Iterator<Item = (&IndexedSession, u64)> {
        self.files
            .values()
            .filter_map(|file| Some((file.parsed.as_ref()?, unix_millis(file.modified))))
    }

    pub fn project_sessions(&self, project: &Path) -> Vec<SessionSummary> {
        let project = ProjectPath::new(project);
        let mut matches: Vec<_> = self
            .sessions()
            .filter(|(session, _)| project.contains(session))
            .collect();
        matches.sort_unstable_by(|(left, left_mtime), (right, right_mtime)| {
            right
                .summary
                .last_at
                .cmp(&left.summary.last_at)
                .then(right_mtime.cmp(left_mtime))
                .then(left.summary.reference.id.cmp(&right.summary.reference.id))
        });
        let mut seen = HashSet::new();
        matches
            .into_iter()
            .filter(|(session, _)| seen.insert(&session.summary.reference))
            .take(PROJECT_LIMIT)
            .map(|(session, _)| session.summary.clone())
            .collect()
    }

    pub fn find(&self, reference: &SessionRef, project: Option<&Path>) -> Option<SessionSummary> {
        let project = project.map(ProjectPath::new);
        self.sessions()
            .filter(|(session, _)| {
                &session.summary.reference == reference
                    && project.as_ref().is_none_or(|root| root.contains(session))
            })
            .max_by_key(|(_, modified)| *modified)
            .map(|(session, _)| session.summary.clone())
    }

    pub fn written_since(
        &self,
        tool: AgentKind,
        cwd: &Path,
        started_at: u64,
        claimed: &HashSet<SessionRef>,
    ) -> Option<SessionRef> {
        let cwd = canonical_path(cwd);
        self.sessions()
            .filter(|(session, modified)| {
                *modified >= started_at
                    && session.canonical_cwd == cwd
                    && session.summary.reference.tool == tool
                    && !claimed.contains(&session.summary.reference)
            })
            .max_by(|(left, left_mtime), (right, right_mtime)| {
                left_mtime
                    .cmp(right_mtime)
                    .then(left.summary.last_at.cmp(&right.summary.last_at))
                    .then(left.summary.reference.id.cmp(&right.summary.reference.id))
            })
            .map(|(session, _)| session.summary.reference.clone())
    }
}

struct ProjectPath {
    original: PathBuf,
    canonical: PathBuf,
}

impl ProjectPath {
    fn new(path: &Path) -> Self {
        Self {
            original: absolute_path(path),
            canonical: canonical_path(path),
        }
    }

    fn contains(&self, session: &IndexedSession) -> bool {
        session.canonical_cwd.starts_with(&self.canonical)
            || absolute_path(&session.summary.cwd).starts_with(&self.original)
    }
}

fn children(path: &Path, directories: bool) -> impl Iterator<Item = DirEntry> {
    fs::read_dir(path)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(move |entry| {
            entry.file_type().is_ok_and(|kind| {
                if directories {
                    kind.is_dir()
                } else {
                    kind.is_file()
                }
            })
        })
}

fn codex_days(path: &Path, cutoff: u64) -> Vec<DirEntry> {
    // Directory dates are local, while timestamps are UTC. Allow the overlap day
    // and apply the exact age limit to each file's mtime.
    let Some(date) = DateTime::from_timestamp_millis(cutoff as i64) else {
        return Vec::new();
    };
    let date = date.date_naive().pred_opt().unwrap_or(date.date_naive());
    let mut days = Vec::new();
    for year in children(path, true) {
        let Some(y) = number(&year).filter(|year| *year as i32 >= date.year()) else {
            continue;
        };
        for month in children(&year.path(), true) {
            let Some(m) = number(&month).filter(|month| {
                (1..=12).contains(month) && (y as i32, *month) >= (date.year(), date.month())
            }) else {
                continue;
            };
            days.extend(children(&month.path(), true).filter(|day| {
                number(day)
                    .and_then(|d| NaiveDate::from_ymd_opt(y as i32, m, d))
                    .is_some_and(|day| day >= date)
            }));
        }
    }
    days
}

fn number(entry: &DirEntry) -> Option<u32> {
    entry.file_name().to_str()?.parse().ok()
}

fn session_filename(tool: AgentKind, path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension == "jsonl")
        && path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .is_some_and(|stem| match tool {
                AgentKind::Claude => SessionRef::new(tool, stem).is_some(),
                AgentKind::Codex => stem.starts_with("rollout-"),
            })
}

fn unix_millis(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn timestamp(value: &str) -> Option<u64> {
    u64::try_from(DateTime::parse_from_rfc3339(value).ok()?.timestamp_millis()).ok()
}

fn absolute_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for part in path.components() {
        match part {
            Component::ParentDir => {
                normalized.pop();
            }
            Component::CurDir => (),
            part => normalized.push(part),
        }
    }
    normalized
}

fn canonical_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| absolute_path(path))
}

fn read_session(
    tool: AgentKind,
    path: &Path,
    len: u64,
    modified: u64,
) -> io::Result<Option<IndexedSession>> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Ok(None);
    }
    let mut parser = Parser::new(tool, path);
    let mut head_end = 0;
    let mut head = BufReader::new((&mut file).take(HEAD_BYTES));
    let mut line = Vec::new();
    for _ in 0..HEAD_LINES {
        line.clear();
        let read = head.read_until(b'\n', &mut line)?;
        if read == 0 || (line.last() != Some(&b'\n') && head_end + (read as u64) < len) {
            break;
        }
        head_end += read as u64;
        parser.line(&line);
    }
    let tail_start = len.saturating_sub(TAIL_BYTES).max(head_end);
    let complete = tail_start == head_end;
    if tail_start < len {
        file.seek(SeekFrom::Start(if complete {
            tail_start
        } else {
            tail_start - 1
        }))?;
        let mut tail = Vec::new();
        (&mut file)
            .take(len - tail_start + u64::from(!complete))
            .read_to_end(&mut tail)?;
        let lines = tail
            .split(|&byte| byte == b'\n')
            .skip(usize::from(!complete));
        for line in lines {
            if complete {
                parser.line(line);
            } else {
                parser.tail_timestamp(line);
            }
        }
    }
    Ok(parser.finish(complete, modified))
}

struct Parser {
    tool: AgentKind,
    reference: Option<SessionRef>,
    cwd: Option<PathBuf>,
    title: Option<String>,
    first: Option<u64>,
    last: Option<u64>,
    files: HashSet<String>,
}

impl Parser {
    fn new(tool: AgentKind, path: &Path) -> Self {
        let reference = (tool == AgentKind::Claude)
            .then(|| SessionRef::new(tool, path.file_stem()?.to_str()?))
            .flatten();
        Self {
            tool,
            reference,
            cwd: None,
            title: None,
            first: None,
            last: None,
            files: HashSet::new(),
        }
    }

    fn line(&mut self, line: &[u8]) {
        let Ok(value) = serde_json::from_slice::<Value>(line) else {
            return;
        };
        let entry_type = value["type"].as_str().unwrap_or_default();
        match self.tool {
            AgentKind::Claude => {
                if !self.claude_id(value["sessionId"].as_str())
                    || value["isSidechain"].as_bool() == Some(true)
                {
                    return;
                }
                self.set_cwd(value["cwd"].as_str());
                if self.title.is_none()
                    && entry_type == "user"
                    && value["isMeta"].as_bool() != Some(true)
                {
                    self.title = message_title(&value["message"]["content"], "text");
                }
                if entry_type == "assistant" {
                    self.files
                        .extend(claude_files(&value["message"]["content"]).map(str::to_owned));
                }
            }
            AgentKind::Codex => {
                let payload = &value["payload"];
                let payload_type = payload["type"].as_str().unwrap_or_default();
                if entry_type == "session_meta" && self.reference.is_none() {
                    self.reference = payload["id"]
                        .as_str()
                        .and_then(|id| SessionRef::new(self.tool, id));
                    self.set_cwd(payload["cwd"].as_str());
                    self.record_time(payload["timestamp"].as_str());
                }
                if self.reference.is_none() {
                    return;
                }
                if self.title.is_none() {
                    self.title = match (entry_type, payload_type) {
                        ("response_item", "message")
                            if payload["role"].as_str() == Some("user") =>
                        {
                            message_title(&payload["content"], "input_text")
                        }
                        ("event_msg", "user_message") => {
                            payload["message"].as_str().and_then(title)
                        }
                        _ => None,
                    };
                }
                collect_codex_files(entry_type, payload_type, payload, &mut self.files);
            }
        }
        self.record_time(value["timestamp"].as_str());
    }

    fn claude_id(&self, id: Option<&str>) -> bool {
        self.reference
            .as_ref()
            .is_some_and(|reference| id.is_some_and(|id| id.eq_ignore_ascii_case(&reference.id)))
    }

    fn set_cwd(&mut self, cwd: Option<&str>) {
        if self.cwd.is_none() {
            self.cwd = cwd
                .map(Path::new)
                .filter(|path| path.is_absolute())
                .map(Path::to_owned);
        }
    }

    fn record_time(&mut self, value: Option<&str>) {
        if let Some(time) = value.and_then(timestamp) {
            self.first = Some(self.first.map_or(time, |first| first.min(time)));
            self.last = Some(self.last.map_or(time, |last| last.max(time)));
        }
    }

    fn tail_timestamp(&mut self, line: &[u8]) {
        #[derive(Deserialize)]
        struct Tail<'a> {
            timestamp: Option<&'a str>,
            #[serde(rename = "sessionId")]
            session_id: Option<&'a str>,
        }
        if let Ok(tail) = serde_json::from_slice::<Tail>(line) {
            if self.tool == AgentKind::Codex || self.claude_id(tail.session_id) {
                self.record_time(tail.timestamp);
            }
        }
    }

    fn finish(self, complete: bool, modified: u64) -> Option<IndexedSession> {
        let cwd = self.cwd?;
        let started_at = self.first.unwrap_or(modified);
        let last_at = self.last.unwrap_or(modified);
        let canonical_cwd = canonical_path(&cwd);
        // A skipped middle can contain more edits. Show counts only when both samples cover the log.
        let file_count = (complete && !self.files.is_empty()).then(|| {
            self.files
                .iter()
                .map(|file| absolute_path(&cwd.join(file)))
                .collect::<HashSet<_>>()
                .len()
        });
        Some(IndexedSession {
            canonical_cwd,
            summary: SessionSummary {
                reference: self.reference?,
                cwd,
                started_at,
                last_at,
                duration_ms: last_at.saturating_sub(started_at),
                title: self.title.unwrap_or_else(|| "未命名会话".into()),
                file_count,
            },
        })
    }
}

fn message_title(content: &Value, text_type: &str) -> Option<String> {
    if let Some(text) = content.as_str() {
        return title(text);
    }
    content
        .as_array()?
        .iter()
        .filter(|block| block["type"].as_str() == Some(text_type))
        .filter_map(|block| block["text"].as_str())
        .find_map(title)
}

const WRAPPERS: &[&str] = &[
    "INSTRUCTIONS",
    "system-reminder",
    "system_instructions",
    "user_instructions",
    "environment_context",
    "command-name",
    "command-message",
    "command-args",
    "local-command-caveat",
    "local-command-stdout",
    "ide_opened_file",
    "ide_selection",
    "task-notification",
    "teammate-message",
    "turn_aborted",
    "permissions instructions",
    "collaboration_mode",
    "app-context",
];

fn title(raw: &str) -> Option<String> {
    let raw = if raw.trim_start().starts_with("# Context from my IDE setup:") {
        raw.split_once("## My request for Codex:")?.1
    } else {
        raw
    };
    let mut clean = raw.to_owned();
    for tag in WRAPPERS {
        let opening = format!("<{tag}");
        let closing = format!("</{tag}>");
        let mut offset = 0;
        while let Some(index) = clean[offset..].find(&opening) {
            let start = offset + index;
            offset = start + opening.len();
            if !clean[offset..].starts_with(['>', ' ', '\t', '/']) {
                continue;
            }
            let Some(tag_end) = clean[offset..].find('>').map(|index| offset + index) else {
                break;
            };
            let end = if clean[offset..tag_end].trim_end().ends_with('/') {
                tag_end + 1
            } else {
                clean[tag_end + 1..]
                    .find(&closing)
                    .map_or(clean.len(), |index| tag_end + 1 + index + closing.len())
            };
            clean.replace_range(start..end, "\n");
            offset = start + 1;
        }
    }
    let mut line = clean
        .lines()
        .map(str::trim)
        .find(|line| {
            !line.is_empty()
                && !line.starts_with('<')
                && !line.starts_with("# AGENTS.md instructions")
                && !matches!(
                    line.split_whitespace().next(),
                    Some(
                        "/exit"
                            | "/status"
                            | "/clear"
                            | "/help"
                            | "/resume"
                            | "/compact"
                            | "/model"
                    )
                )
                && !line
                    .get(..7)
                    .is_some_and(|prefix| prefix.eq_ignore_ascii_case("caveat:"))
        })?
        .chars()
        .filter(|character| !character.is_control());
    let mut result: String = line.by_ref().take(100).collect();
    if line.next().is_some() {
        result.push('…');
    }
    (!result.is_empty()).then_some(result)
}

fn claude_files(content: &Value) -> impl Iterator<Item = &str> {
    content
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|block| {
            if block["type"].as_str() != Some("tool_use") {
                return None;
            }
            let input = &block["input"];
            match block["name"].as_str()? {
                "Edit" | "Write" | "MultiEdit" => input["file_path"].as_str(),
                "NotebookEdit" => input["notebook_path"]
                    .as_str()
                    .or_else(|| input["file_path"].as_str()),
                _ => None,
            }
            .filter(|file| !file.is_empty())
        })
}

fn collect_codex_files(
    entry: &str,
    payload_type: &str,
    payload: &Value,
    files: &mut HashSet<String>,
) {
    if entry == "event_msg"
        && payload_type == "patch_apply_end"
        && payload["success"].as_bool() != Some(false)
    {
        if let Some(changes) = payload["changes"].as_object() {
            for (file, change) in changes {
                files.insert(file.clone());
                if let Some(destination) = change["move_path"].as_str() {
                    files.insert(destination.to_owned());
                }
            }
        }
    } else if entry == "response_item"
        && matches!(payload_type, "custom_tool_call" | "function_call")
        && payload["name"].as_str() == Some("apply_patch")
    {
        let arguments = payload["arguments"]
            .as_str()
            .and_then(|value| serde_json::from_str::<Value>(value).ok());
        let patch = payload["input"]
            .as_str()
            .or_else(|| payload["arguments"]["patch"].as_str())
            .or_else(|| arguments.as_ref().and_then(|value| value["patch"].as_str()))
            .or_else(|| payload["arguments"].as_str());
        for line in patch.into_iter().flat_map(str::lines) {
            if let Some(file) = [
                "*** Add File: ",
                "*** Update File: ",
                "*** Delete File: ",
                "*** Move to: ",
            ]
            .iter()
            .find_map(|prefix| line.strip_prefix(prefix))
            .filter(|file| !file.is_empty())
            {
                files.insert(file.to_owned());
            }
        }
    }
}
