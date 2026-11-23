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

mod cache;
#[cfg(test)]
mod tests;
mod title;

pub use cache::SharedCatalog;

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

#[derive(Clone, Debug, Deserialize, Serialize)]
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSessionSummary {
    pub project_id: String,
    #[serde(flatten)]
    pub session: SessionSummary,
}

#[derive(Clone, Deserialize, Serialize)]
struct IndexedSession {
    summary: SessionSummary,
    canonical_cwd: PathBuf,
    evidence: SessionEvidence,
}

#[derive(Clone, Deserialize, Serialize)]
struct SessionEvidence {
    user_messages: Option<usize>,
    duration_ms: Option<u64>,
    file_edit: Option<bool>,
}

impl IndexedSession {
    fn in_history(&self, protected: &HashSet<SessionRef>) -> bool {
        protected.contains(&self.summary.reference)
            || !matches!(
                (self.evidence.user_messages, self.evidence.duration_ms, self.evidence.file_edit),
                (Some(1), Some(duration), Some(false)) if duration < 60_000
            )
    }
}

#[derive(Clone, Deserialize, Serialize)]
struct CachedFile {
    modified: SystemTime,
    len: u64,
    parsed: Option<IndexedSession>,
}

#[derive(Clone)]
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

    pub fn refresh(&mut self) -> bool {
        self.refresh_at(unix_millis(SystemTime::now()))
    }

    fn refresh_at(&mut self, now: u64) -> bool {
        let Some(home) = &self.home else {
            return false;
        };
        let cutoff = now.saturating_sub(RECENT);
        let claude: Vec<_> = children(&home.join(".claude/projects"), true).collect();
        let codex = codex_days(&home.join(".codex/sessions"), cutoff);
        let mut seen = HashSet::new();
        let mut changed = false;
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
                        if let Ok(parsed) = read_session(tool, &path, len, unix_millis(modified)) {
                            self.files.insert(
                                path.clone(),
                                CachedFile {
                                    modified,
                                    len,
                                    parsed,
                                },
                            );
                            changed = true;
                        }
                    }
                    seen.insert(path);
                }
            }
        }
        let before = self.files.len();
        self.files.retain(|path, _| seen.contains(path));
        changed || self.files.len() != before
    }

    fn sessions(&self) -> impl Iterator<Item = (&IndexedSession, u64)> {
        self.files
            .values()
            .filter_map(|file| Some((file.parsed.as_ref()?, unix_millis(file.modified))))
    }

    pub fn project_sessions(
        &self,
        project: &Path,
        protected: &HashSet<SessionRef>,
    ) -> Vec<SessionSummary> {
        let project = ProjectPath::new(project);
        let mut matches: Vec<_> = self
            .sessions()
            .filter(|(session, _)| project.contains(session) && session.in_history(protected))
            .collect();
        matches.sort_unstable_by(|left, right| most_recent(*left, *right));
        let mut seen = HashSet::new();
        matches
            .into_iter()
            .filter(|(session, _)| seen.insert(&session.summary.reference))
            .take(PROJECT_LIMIT)
            .map(|(session, _)| session.summary.clone())
            .collect()
    }

    pub fn all_project_sessions<'a>(
        &self,
        projects: impl IntoIterator<Item = (&'a str, PathBuf)>,
        protected: &HashSet<SessionRef>,
    ) -> Vec<ProjectSessionSummary> {
        let roots: Vec<_> = projects
            .into_iter()
            .map(|(id, path)| (id, ProjectPath::new(&path)))
            .collect();
        let mut matches: Vec<_> = self
            .sessions()
            .filter(|(session, _)| session.in_history(protected))
            .filter_map(|(session, modified)| {
                let cwd = absolute_path(&session.summary.cwd);
                // A session in a nested project belongs to its closest registered root.
                let (_, project_id) = roots
                    .iter()
                    .filter_map(|(id, root)| Some((root.matching_depth(session, &cwd)?, *id)))
                    .max_by(|left, right| left.0.cmp(&right.0).then(right.1.cmp(left.1)))?;
                Some(((session, modified), project_id))
            })
            .collect();
        matches.sort_unstable_by(|left, right| most_recent(left.0, right.0));
        let mut seen = HashSet::new();
        matches
            .into_iter()
            .filter(|((session, _), _)| seen.insert(&session.summary.reference))
            .map(|((session, _), project_id)| ProjectSessionSummary {
                project_id: project_id.into(),
                session: session.summary.clone(),
            })
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

    pub fn reference_for_logs(&self, tool: AgentKind, paths: &[PathBuf]) -> Option<SessionRef> {
        let home = self.home.as_deref()?;
        let canonical_home = canonical_path(home);
        let mut references = paths.iter().filter_map(|path| {
            let file = self.files.get(path).or_else(|| {
                let relative = path.strip_prefix(&canonical_home).ok()?;
                self.files.get(&home.join(relative))
            })?;
            let reference = &file.parsed.as_ref()?.summary.reference;
            (reference.tool == tool).then_some(reference)
        });
        let reference = references.next()?;
        // A process can also open nested session logs. Require a unique ID.
        references.all(|other| other == reference).then(|| reference.clone())
    }
}

fn most_recent(left: (&IndexedSession, u64), right: (&IndexedSession, u64)) -> std::cmp::Ordering {
    right
        .0
        .summary
        .last_at
        .cmp(&left.0.summary.last_at)
        .then(right.1.cmp(&left.1))
        .then(
            left.0
                .summary
                .reference
                .id
                .cmp(&right.0.summary.reference.id),
        )
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

    fn matching_depth(&self, session: &IndexedSession, cwd: &Path) -> Option<usize> {
        [
            (&self.canonical, session.canonical_cwd.as_path()),
            (&self.original, cwd),
        ]
        .into_iter()
        .filter(|(root, cwd)| root.is_absolute() && cwd.starts_with(root))
        .map(|(root, _)| root.components().count())
        .max()
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
    fallback_title: Option<String>,
    first: Option<u64>,
    last: Option<u64>,
    files: HashSet<String>,
    user_messages: usize,
    file_edit: bool,
    evidence_known: bool,
    last_codex_user: Option<(bool, u64, String)>,
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
            fallback_title: None,
            first: None,
            last: None,
            files: HashSet::new(),
            user_messages: 0,
            file_edit: false,
            evidence_known: true,
            last_codex_user: None,
        }
    }

    fn line(&mut self, line: &[u8]) {
        if line.iter().all(u8::is_ascii_whitespace) {
            return;
        }
        let Ok(value) = serde_json::from_slice::<Value>(line) else {
            self.evidence_known = false;
            return;
        };
        let entry_type = value["type"].as_str().unwrap_or_default();
        if entry_type.is_empty() {
            self.evidence_known = false;
        }
        let recorded = value["timestamp"].as_str().and_then(timestamp);
        match self.tool {
            AgentKind::Claude => {
                if value["isSidechain"].as_bool() == Some(true) {
                    return;
                }
                let session_id = value["sessionId"].as_str();
                if !self.claude_id(session_id) {
                    if session_id.is_none() && matches!(entry_type, "user" | "assistant") {
                        self.evidence_known = false;
                    }
                    return;
                }
                self.set_cwd(value["cwd"].as_str());
                let content = &value["message"]["content"];
                if matches!(entry_type, "user" | "assistant") && recorded.is_none() {
                    self.evidence_known = false;
                }
                if entry_type == "user"
                    && value["isMeta"].as_bool() != Some(true)
                    && value["isCompactSummary"].as_bool() != Some(true)
                {
                    self.record_user(user_content(content, "text"), None, recorded);
                }
                if entry_type == "assistant" {
                    self.files.extend(claude_files(content).map(str::to_owned));
                    if let Some(blocks) = content.as_array() {
                        for block in blocks {
                            if block["type"].as_str() == Some("tool_use") {
                                match block["name"].as_str() {
                                    Some("Edit" | "Write" | "MultiEdit" | "NotebookEdit") => {
                                        self.file_edit = true
                                    }
                                    Some(_) => (),
                                    None => self.evidence_known = false,
                                }
                            } else if block["type"].as_str().is_none() {
                                self.evidence_known = false;
                            }
                        }
                    } else if !content.is_string() {
                        self.evidence_known = false;
                    }
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
                    self.record_time(payload["timestamp"].as_str().and_then(timestamp));
                }
                if self.reference.is_none() {
                    return;
                }
                if matches!(entry_type, "event_msg" | "response_item")
                    && (recorded.is_none() || payload_type.is_empty())
                {
                    self.evidence_known = false;
                }
                match (entry_type, payload_type) {
                    ("response_item", "message") if payload["role"].as_str() == Some("user") => {
                        self.record_user(
                            user_content(&payload["content"], "input_text"),
                            Some(false),
                            recorded,
                        );
                    }
                    ("response_item", "message") if !payload["role"].is_string() => {
                        self.evidence_known = false;
                    }
                    ("event_msg", "user_message") => {
                        let content = payload["message"].as_str().map(|text| {
                            let images = ["images", "local_images"].into_iter().any(|key| {
                                payload[key]
                                    .as_array()
                                    .is_some_and(|items| !items.is_empty())
                            });
                            (text.to_owned(), images)
                        });
                        self.record_user(content, Some(true), recorded);
                    }
                    ("response_item", "custom_tool_call" | "function_call") => {
                        match payload["name"].as_str() {
                            Some(
                                "apply_patch" | "write_file" | "edit_file" | "create_file"
                                | "delete_file",
                            ) => self.file_edit = true,
                            Some(_) => (),
                            None => self.evidence_known = false,
                        }
                    }
                    ("event_msg", "patch_apply_begin" | "patch_apply_end") => self.file_edit = true,
                    _ => (),
                }
                collect_codex_files(entry_type, payload_type, payload, &mut self.files);
            }
        }
        self.record_time(recorded);
    }

    fn record_user(
        &mut self,
        content: Option<(String, bool)>,
        codex_event: Option<bool>,
        recorded: Option<u64>,
    ) {
        let Some((raw, attached)) = content else {
            self.evidence_known = false;
            return;
        };
        let body = title::strip_wrappers(&raw).unwrap_or_default();
        if !attached && !title::user_text(&body) {
            return;
        }
        if self.title.is_none() {
            self.title = title::clean_title(&body);
            if self.fallback_title.is_none() {
                self.fallback_title = title::legacy_title(&body);
            }
        }
        if let (Some(event), Some(time)) = (codex_event, recorded) {
            // Older Codex logs may record the same turn as both a response item and an event.
            let text = body.trim();
            if self
                .last_codex_user
                .as_ref()
                .is_some_and(|(previous, at, body)| {
                    *previous != event && time.abs_diff(*at) <= 1_000 && body == text
                })
            {
                self.last_codex_user = None;
                return;
            }
            self.last_codex_user = Some((event, time, text.to_owned()));
        }
        self.user_messages += 1;
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

    fn record_time(&mut self, timestamp: Option<u64>) {
        if let Some(time) = timestamp {
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
                self.record_time(tail.timestamp.and_then(timestamp));
            }
        }
    }

    fn finish(self, complete: bool, modified: u64) -> Option<IndexedSession> {
        let cwd = self.cwd?;
        let started_at = self.first.unwrap_or(modified);
        let last_at = self.last.unwrap_or(modified);
        let known = complete && self.evidence_known;
        let evidence = SessionEvidence {
            user_messages: known.then_some(self.user_messages),
            duration_ms: known.then(|| self.last?.checked_sub(self.first?)).flatten(),
            file_edit: (known || self.file_edit).then_some(self.file_edit),
        };
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
            evidence,
            summary: SessionSummary {
                reference: self.reference?,
                cwd,
                started_at,
                last_at,
                duration_ms: last_at.saturating_sub(started_at),
                title: self
                    .title
                    .or(self.fallback_title)
                    .unwrap_or_else(|| "未命名会话".into()),
                file_count,
            },
        })
    }
}

fn user_content(content: &Value, text_type: &str) -> Option<(String, bool)> {
    if let Some(text) = content.as_str() {
        return Some((text.to_owned(), false));
    }
    let mut text = String::new();
    let mut attached = false;
    for block in content.as_array()? {
        match block["type"].as_str()? {
            kind if kind == text_type => {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(block["text"].as_str()?);
            }
            "image" | "input_image" | "document" | "input_file" => attached = true,
            "tool_result" => (),
            _ => return None,
        }
    }
    Some((text, attached))
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
