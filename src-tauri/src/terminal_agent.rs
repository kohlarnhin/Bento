mod foreground;
mod screen;
pub(crate) mod sessions;

use crate::terminal_escape::{osc_notification, EscapeEvent, EscapeStream};
pub(crate) use foreground::agent_log_paths;
use foreground::{Detector, ForegroundAgent};
use screen::Screen;
use serde::{Deserialize, Serialize};
use sessions::SessionRef;
use std::sync::Mutex;

const CODEX_QUIET_MS: u64 = 3_000;
// Claude can stop animating its title just before it paints the approval panel.
const CLAUDE_IDLE_SETTLE_MS: u64 = 500;
// CLIs in GENERIC: a short-lived `--version` call never shows up as a session,
// a startup animation cannot start a round, and a round ends after 3 s without
// output unless a BEL / OSC 9 / OSC 777 notification ends it first.
const GENERIC_READY_MS: u64 = 1_000;
const GENERIC_STARTUP_MS: u64 = 5_000;
const GENERIC_QUIET_MS: u64 = 3_000;
const GENERIC_BELL_SETTLE_MS: u64 = 500;
// Output continuing this long, with gaps shorter than GENERIC_BURST_GAP_MS and
// not right after a keystroke, means the CLI is working again.
const GENERIC_SUSTAIN_MS: u64 = 2_000;
const GENERIC_BURST_GAP_MS: u64 = 700;
const GENERIC_ECHO_MS: u64 = 400;

#[derive(Clone, Copy, Debug, Deserialize, Hash, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentKind {
    Claude,
    Codex,
    Devin,
    Droid,
    Gemini,
    Cursor,
    Copilot,
    OpenCode,
    Aider,
    Qwen,
    Kimi,
    Amp,
    Crush,
    IFlow,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentState {
    Thinking,
    Waiting,
    Done,
}

impl AgentState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Thinking => "思考中",
            Self::Waiting => "等你确认",
            Self::Done => "完成",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSession {
    pub kind: AgentKind,
    pub state: AgentState,
    pub state_started_at: u64,
    pub round_started_at: Option<u64>,
    pub acknowledged: bool,
}

impl AgentSession {
    pub fn priority(self) -> u8 {
        match self.state {
            AgentState::Waiting => 0,
            AgentState::Done if !self.acknowledged => 1,
            AgentState::Thinking => 2,
            AgentState::Done => 3,
        }
    }

    pub fn pending(self) -> bool {
        self.priority() < 2
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStatus {
    pub agent: Option<AgentSession>,
    pub agent_revision: u64,
}

struct Features {
    kind: AgentKind,
    command: &'static [u8],
    node_script: &'static [u8],
    prompt: char,
    busy_titles: &'static [char],
    idle_title: Option<char>,
    waiting_notice: &'static [&'static str],
    done_notice: &'static [&'static str],
    questions: &'static [&'static str],
    answers: &'static [&'static str],
    alternatives: &'static [&'static str],
}

// Claude Code 2.1.322 (local native bundle) and Codex CLI 0.170.0
// (bottom_pane/approval_overlay.rs at the matching release tag).
const FEATURES: [Features; 2] = [
    Features {
        kind: AgentKind::Claude,
        command: b"claude",
        node_script: b"/@anthropic-ai/claude-code/cli.js",
        prompt: '❯',
        busy_titles: &['◐', '◑'],
        idle_title: Some('✳'),
        waiting_notice: &["claude needs your permission"],
        done_notice: &["claude is waiting for your input"],
        questions: &["do you want to "],
        answers: &["1. yes"],
        alternatives: &["no, and tell claude what to do differently"],
    },
    Features {
        kind: AgentKind::Codex,
        command: b"codex",
        node_script: b"/@openai/codex/bin/codex.js",
        prompt: '›',
        busy_titles: &[],
        idle_title: None,
        waiting_notice: &[],
        done_notice: &[],
        questions: &[
            "would you like to run the following command?",
            "would you like to make the following edits?",
            "would you like to grant these permissions?",
            "would you like to send input to ",
            "do you want to approve network access to ",
            "needs your approval.",
        ],
        answers: &[
            "1. yes, proceed",
            "1. yes, just this once",
            "1. yes, grant",
            "1. accept",
        ],
        alternatives: &[
            "to confirm",
            "no, and tell codex what to do differently",
            "no, continue without running it",
            "no, continue without permissions",
        ],
    },
];

/// Agent CLIs tracked through the signals every TUI shares. `commands` match the
/// executable or argv[0]; `scripts` match the entry script run by Node, Bun, Python
/// or a shell, by file name or, when starting with '/', by a path fragment.
struct Generic {
    kind: AgentKind,
    label: &'static str,
    commands: &'static [&'static [u8]],
    scripts: &'static [&'static [u8]],
}

const GENERIC: [Generic; 12] = [
    Generic { kind: AgentKind::Devin, label: "Devin", commands: &[b"devin"], scripts: &[] },
    Generic { kind: AgentKind::Droid, label: "Droid", commands: &[b"droid"], scripts: &[] },
    Generic { kind: AgentKind::Gemini, label: "Gemini", commands: &[], scripts: &[b"gemini", b"/@google/gemini-cli/"] },
    Generic { kind: AgentKind::Cursor, label: "Cursor", commands: &[b"cursor-agent"], scripts: &[b"cursor-agent", b"/cursor-agent/versions/"] },
    // AWS Copilot is a native `copilot` binary, so only the npm CLI is matched.
    Generic { kind: AgentKind::Copilot, label: "Copilot", commands: &[], scripts: &[b"copilot", b"/@github/copilot/"] },
    Generic { kind: AgentKind::OpenCode, label: "OpenCode", commands: &[b"opencode"], scripts: &[b"opencode"] },
    Generic { kind: AgentKind::Aider, label: "Aider", commands: &[], scripts: &[b"aider"] },
    Generic { kind: AgentKind::Qwen, label: "Qwen", commands: &[], scripts: &[b"qwen", b"/@qwen-code/qwen-code/"] },
    Generic { kind: AgentKind::Kimi, label: "Kimi", commands: &[], scripts: &[b"kimi"] },
    Generic { kind: AgentKind::Amp, label: "Amp", commands: &[], scripts: &[b"amp", b"/@sourcegraph/amp/"] },
    Generic { kind: AgentKind::Crush, label: "Crush", commands: &[b"crush"], scripts: &[] },
    Generic { kind: AgentKind::IFlow, label: "iFlow", commands: &[], scripts: &[b"iflow", b"/@iflow-ai/iflow-cli/"] },
];

// Notification text that asks for a decision; anything else ends the round.
const GENERIC_WAITING_WORDS: &[&str] = &[
    "permission", "approv", "confirm", "allow", "authoriz", "needs input", "needs your", "requires your",
    "批准", "确认", "授权", "权限",
];

impl AgentKind {
    fn features(self) -> Option<&'static Features> {
        FEATURES.iter().find(|feature| feature.kind == self)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
            Self::Codex => "Codex",
            kind => GENERIC.iter().find(|agent| agent.kind == kind).map_or("AI", |agent| agent.label),
        }
    }

    /// Only Claude Code and Codex sessions can be listed and resumed.
    pub fn resumable(self) -> bool {
        matches!(self, Self::Claude | Self::Codex)
    }
}

fn basename(path: &[u8]) -> &[u8] {
    path.rsplit(|&byte| byte == b'/').next().unwrap_or_default()
}

fn script_matches(pattern: &[u8], script: &[u8]) -> bool {
    if pattern.starts_with(b"/") {
        script.windows(pattern.len()).any(|part| part == pattern)
    } else {
        basename(script) == pattern
    }
}

fn identify<'a, I: Iterator<Item = &'a [u8]>>(
    executable: &[u8],
    mut argv: I,
) -> Option<(AgentKind, std::iter::Peekable<I>)> {
    let executable = basename(executable);
    let command = basename(argv.next().unwrap_or_default());
    if let Some(feature) = FEATURES
        .iter()
        .find(|feature| feature.command == executable || feature.command == command)
    {
        return Some((feature.kind, argv.peekable()));
    }
    if let Some(agent) = GENERIC.iter().find(|agent| {
        agent.commands.iter().any(|name| *name == executable || *name == command)
    }) {
        return Some((agent.kind, argv.peekable()));
    }
    let interpreter = if executable.is_empty() {
        command
    } else {
        executable
    };
    let runtime = matches!(interpreter, b"node" | b"nodejs" | b"bun") || interpreter.starts_with(b"python");
    let shell = matches!(interpreter, b"sh" | b"bash" | b"zsh" | b"fish");
    // A Node or Python CLI that sets its process title replaces argv[0].
    if let Some(agent) = GENERIC.iter().find(|agent| runtime && agent.scripts.contains(&command)) {
        return Some((agent.kind, argv.peekable()));
    }
    let mut args = argv.peekable();
    // Only inspect the entry script. A shell or Node program can have 'claude'
    // or 'codex' anywhere in its own arguments.
    while let Some(arg) = args.peek() {
        match *arg {
            b"-c" | b"-e" | b"-p" | b"--eval" | b"--print" => return None,
            b"--" => {
                args.next();
                break;
            }
            b"-r" | b"--require" | b"--import" | b"--loader" => {
                args.next();
                args.next();
            }
            arg if arg.starts_with(b"-") => {
                args.next();
            }
            _ => break,
        }
    }
    let script = args.next()?;
    FEATURES
        .iter()
        .find(|feature| match interpreter {
            b"node" | b"nodejs" | b"bun" => {
                script.ends_with(feature.node_script) || basename(script) == feature.command
            }
            _ if runtime || shell => basename(script) == feature.command,
            _ => false,
        })
        .map(|feature| feature.kind)
        // Wrapper scripts keep the CLI's own name, like Cursor's Bash launcher.
        .or_else(|| {
            GENERIC
                .iter()
                .find(|agent| {
                    (runtime || shell)
                        && agent.commands.iter().chain(agent.scripts).any(|pattern| script_matches(pattern, script))
                })
                .map(|agent| agent.kind)
        })
        .map(|kind| (kind, args))
}

fn resume_arg<'a>(kind: AgentKind, mut args: impl Iterator<Item = &'a [u8]>) -> Option<SessionRef> {
    let first = args.next()?;
    let id = match (kind, first) {
        (AgentKind::Codex, b"resume") | (AgentKind::Claude, b"--resume" | b"-r") => args.next()?,
        (AgentKind::Claude, arg) => arg.strip_prefix(b"--resume=")?,
        _ => return None,
    };
    SessionRef::new(kind, std::str::from_utf8(id).ok()?)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Title {
    Working,
    Idle,
}

fn osc(features: &Features, raw: &[u8]) -> (Option<Title>, Option<AgentState>) {
    let text = String::from_utf8_lossy(raw);
    if let Some(("0" | "2", value)) = text.split_once(';') {
        let mut chars = value.trim_start().chars();
        if let Some(first) = chars.next().filter(|_| chars.next() == Some(' ')) {
            if features.busy_titles.contains(&first) {
                return (Some(Title::Working), None);
            }
            if features.idle_title == Some(first) {
                return (Some(Title::Idle), None);
            }
        }
    }
    let message = text
        .strip_prefix("9;")
        .or_else(|| {
            text.strip_prefix("777;notify;")
                .and_then(|value| value.split_once(';').map(|(_, body)| body))
        })
        .map(str::trim);
    let notice = message.and_then(|message| {
        let message = message.to_ascii_lowercase();
        if features
            .waiting_notice
            .iter()
            .any(|prefix| message.starts_with(prefix))
        {
            Some(AgentState::Waiting)
        } else if features
            .done_notice
            .iter()
            .any(|prefix| message.starts_with(prefix))
        {
            Some(AgentState::Done)
        } else {
            None
        }
    });
    (None, notice)
}

fn generic_notice(raw: &[u8]) -> Option<AgentState> {
    let text = String::from_utf8_lossy(raw);
    let (command, value) = text.split_once(';')?;
    let message = osc_notification(command, value)?.to_lowercase();
    Some(if GENERIC_WAITING_WORDS.iter().any(|word| message.contains(word)) {
        AgentState::Waiting
    } else {
        AgentState::Done
    })
}

fn approval(features: &Features, tail: &str) -> bool {
    let includes = |strings: &[&str]| strings.iter().any(|part| tail.contains(part));
    let question = includes(features.questions);
    let answer = includes(features.answers);
    let alternative = includes(features.alternatives);
    (question && answer) || (alternative && (question || answer))
}

#[derive(Default)]
struct Observation {
    changed: bool,
    title: Option<Title>,
    working_title: bool,
    notice: Option<AgentState>,
    bell: bool,
    approval: bool,
    prompt: bool,
    codex_working: bool,
}

#[derive(Default)]
struct Output {
    stream: EscapeStream,
    screen: Screen,
    approval: bool,
}

impl Output {
    fn feed(&mut self, kind: AgentKind, bytes: &[u8]) -> Observation {
        let features = kind.features();
        let mut seen = Observation::default();
        for &byte in bytes {
            match self.stream.event(byte) {
                EscapeEvent::Osc(raw) => {
                    let (title, notice) = match features {
                        Some(features) => osc(features, &raw),
                        None => (None, generic_notice(&raw)),
                    };
                    seen.working_title |= title == Some(Title::Working);
                    seen.title = title.or(seen.title);
                    seen.notice = notice.or(seen.notice);
                }
                EscapeEvent::Control if byte == 7 => seen.bell = true,
                EscapeEvent::Text | EscapeEvent::Control => seen.changed |= self.screen.text(byte),
                EscapeEvent::Csi(params) => seen.changed |= self.screen.csi(params, byte),
                EscapeEvent::Other => (),
            }
        }
        if let Some(features) = features.filter(|_| seen.changed) {
            self.approval = approval(features, &self.screen.tail());
            seen.prompt = self.screen.has_prompt(features.prompt);
            seen.codex_working = kind == AgentKind::Codex && self.screen.codex_working();
        }
        seen.approval = self.approval;
        seen
    }
}

pub struct Tracker {
    session: AgentSession,
    pub process: Option<AgentProcess>,
    ready: bool,
    output: Output,
    last_activity: u64,
    idle_at: Option<u64>,
    title: Option<Title>,
    detected_at: u64,
    typed_at: Option<u64>,
    burst: Option<(u64, u64)>,
}

impl Tracker {
    fn new(kind: AgentKind, now: u64, cols: u16, rows: u16) -> Self {
        let mut tracker = Self {
            session: AgentSession {
                kind,
                state: AgentState::Done,
                state_started_at: now,
                round_started_at: None,
                acknowledged: true,
            },
            process: None,
            ready: false,
            output: Output::default(),
            last_activity: now,
            idle_at: None,
            title: None,
            detected_at: now,
            typed_at: None,
            burst: None,
        };
        tracker.resize(cols, rows);
        tracker
    }

    fn generic(&self) -> bool {
        self.session.kind.features().is_none()
    }

    fn typed(&mut self, now: u64) {
        self.typed_at = Some(now);
    }

    pub fn acknowledge(&mut self) {
        if self.session.state == AgentState::Done {
            self.session.acknowledged = true;
        }
    }

    pub fn status(&self) -> Option<AgentSession> {
        self.ready.then_some(self.session)
    }

    fn state(&mut self, state: AgentState, now: u64, visible: bool) {
        if self.session.state == state {
            return;
        }
        if state == AgentState::Thinking
            && (self.session.state == AgentState::Done || self.session.round_started_at.is_none())
        {
            self.session.round_started_at = Some(now);
        }
        self.session.state = state;
        self.session.state_started_at = now;
        self.session.acknowledged = state == AgentState::Done && visible;
    }

    fn submit(&mut self, now: u64) {
        self.ready |= self.generic();
        if !self.ready {
            return;
        }
        self.session.round_started_at = Some(now);
        self.idle_at = None;
        self.burst = None;
        self.last_activity = now;
        self.output.screen.clear_text();
        self.output.approval = false;
        self.state(AgentState::Thinking, now, false);
    }

    pub fn observe(&mut self, bytes: &[u8], now: u64, visible: bool) {
        let had_approval = self.output.approval;
        let seen = self.output.feed(self.session.kind, bytes);
        if self.generic() {
            return self.observe_generic(&seen, now, visible);
        }
        self.ready |= seen.prompt
            || seen.approval
            || seen.title.is_some()
            || seen.notice.is_some()
            || seen.codex_working;
        if !self.ready {
            return;
        }
        self.title = seen.title.or(self.title);
        if seen.approval || seen.notice == Some(AgentState::Waiting) {
            self.idle_at = None;
            self.state(AgentState::Waiting, now, visible);
            return;
        }
        if seen.notice == Some(AgentState::Done) {
            self.idle_at = None;
            self.state(AgentState::Done, now, visible);
            return;
        }
        if self.session.state == AgentState::Waiting {
            // ANSI repaint can dismiss an approval via keyboard shortcuts without Enter.
            if !had_approval || !seen.changed {
                return;
            }
            self.last_activity = now;
            self.state(AgentState::Thinking, now, visible);
        }

        if self.session.kind == AgentKind::Codex {
            // Startup paint, cursor reports and idle redraws never start a turn.
            if seen.codex_working || (seen.changed && self.session.state == AgentState::Thinking) {
                self.last_activity = now;
                self.state(AgentState::Thinking, now, visible);
            }
        } else {
            if seen.working_title {
                self.idle_at = None;
                self.state(AgentState::Thinking, now, visible);
            }
            if self.session.state == AgentState::Thinking
                && (seen.title == Some(Title::Idle)
                    || (seen.bell && self.title != Some(Title::Working)))
            {
                self.idle_at.get_or_insert(now + CLAUDE_IDLE_SETTLE_MS);
            }
        }
    }

    pub fn tick(&mut self, now: u64, visible: bool) {
        self.ready |= self.generic() && now.saturating_sub(self.detected_at) >= GENERIC_READY_MS;
        if self.session.state == AgentState::Thinking
            && (match self.session.kind {
                AgentKind::Claude => self.idle_at.is_some_and(|idle| now >= idle),
                AgentKind::Codex => now.saturating_sub(self.last_activity) >= CODEX_QUIET_MS,
                _ => self.idle_at.is_some_and(|idle| now >= idle)
                    || now.saturating_sub(self.last_activity) >= GENERIC_QUIET_MS,
            })
        {
            self.idle_at = None;
            self.state(AgentState::Done, now, visible);
        }
    }

    fn resize(&mut self, cols: u16, rows: u16) {
        if self.output.screen.resize(cols, rows) {
            self.output.approval = false;
        }
    }

    fn observe_generic(&mut self, seen: &Observation, now: u64, visible: bool) {
        self.ready |= seen.notice.is_some() || now.saturating_sub(self.detected_at) >= GENERIC_READY_MS;
        if !self.ready {
            return;
        }
        if let Some(state) = seen.notice {
            self.idle_at = None;
            self.burst = None;
            self.state(state, now, visible);
            return;
        }
        if seen.changed {
            self.last_activity = now;
            // A bell ends the round only once the screen has settled.
            if let Some(idle) = self.idle_at.as_mut() {
                *idle = now + GENERIC_BELL_SETTLE_MS;
            }
            // Echoed keystrokes and prompt editing are not the CLI working.
            if self.typed_at.is_none_or(|typed| now.saturating_sub(typed) >= GENERIC_ECHO_MS) {
                self.burst = match self.burst {
                    Some((first, last)) if now.saturating_sub(last) < GENERIC_BURST_GAP_MS => Some((first, now)),
                    _ => Some((now, now)),
                };
            }
        }
        if seen.bell && self.session.state == AgentState::Thinking {
            self.idle_at.get_or_insert(now + GENERIC_BELL_SETTLE_MS);
        }
        let working = self.burst.is_some_and(|(first, last)| {
            last.saturating_sub(first) >= GENERIC_SUSTAIN_MS && now.saturating_sub(last) < GENERIC_BURST_GAP_MS
        });
        if working && self.session.state != AgentState::Thinking
            && now.saturating_sub(self.detected_at) >= GENERIC_STARTUP_MS
        {
            self.idle_at = None;
            self.state(AgentState::Thinking, now, visible);
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentProcess {
    pub kind: AgentKind,
    pub pid: u32,
    pub started_at: u64,
    pub resume: Option<SessionRef>,
}

pub struct Probe {
    foreground: Box<dyn Fn() -> Option<i32> + Send + Sync>,
    // Sample foreground and apply its result in the same order on the PTY
    // reader, input writer and the idle ticker.
    pub sampling: Mutex<Sample>,
}

pub struct Sample {
    detector: Detector,
    observed: Option<ForegroundAgent>,
    active: Option<ForegroundAgent>,
    cols: u16,
    rows: u16,
    pasting: bool,
}

impl Probe {
    pub fn new(
        foreground: impl Fn() -> Option<i32> + Send + Sync + 'static,
        cols: u16,
        rows: u16,
    ) -> Self {
        Self {
            foreground: Box::new(foreground),
            sampling: Mutex::new(Sample {
                detector: Detector::default(),
                observed: None,
                active: None,
                cols,
                rows,
                pasting: false,
            }),
        }
    }

    pub fn sample(&self, sample: &mut Sample, now: u64) {
        sample.observed = sample.detector.sample((self.foreground)(), now);
    }
}

impl Sample {
    pub fn sync(&mut self, tracker: &mut Option<Tracker>, now: u64) {
        if self.observed != self.active {
            self.active.clone_from(&self.observed);
            self.pasting = false;
            *tracker = self.active.as_ref().map(|agent| {
                let mut tracker = Tracker::new(agent.process.kind, now, self.cols, self.rows);
                tracker.process = Some(agent.process.clone());
                tracker
            });
        }
    }

    pub fn input(&mut self, tracker: &mut Tracker, mut bytes: &[u8], now: u64) {
        tracker.typed(now);
        // xterm sends each bracketed paste as one write. Native Enter sends CR,
        // while Shift+Enter / Ctrl+J supply a literal LF to the prompt editor.
        while let Some((&byte, rest)) = bytes.split_first() {
            if let Some(rest) = bytes.strip_prefix(b"\x1b[200~") {
                self.pasting = true;
                bytes = rest;
            } else if let Some(rest) = bytes.strip_prefix(b"\x1b[201~") {
                self.pasting = false;
                bytes = rest;
            } else {
                if byte == b'\r' && !self.pasting {
                    tracker.submit(now);
                }
                bytes = rest;
            }
        }
    }

    pub fn resize(&mut self, tracker: Option<&mut Tracker>, cols: u16, rows: u16) {
        self.cols = cols;
        self.rows = rows;
        if let Some(tracker) = tracker {
            tracker.resize(cols, rows);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use AgentKind::{Claude, Codex};
    use AgentState::{Done, Thinking, Waiting};

    fn identify_args(executable: &str, args: &[&str]) -> Option<AgentKind> {
        identify(executable.as_bytes(), args.iter().map(|arg| arg.as_bytes())).map(|(kind, _)| kind)
    }

    #[test]
    fn recognizes_native_commands_and_actual_interpreter_entries_only() {
        assert_eq!(
            identify_args(
                "/home/example/.local/share/claude/versions/2.1.322",
                &["claude"]
            ),
            Some(Claude)
        );
        assert_eq!(
            identify_args(
                "/opt/codex/vendor/bin/codex",
                &["/opt/codex/vendor/bin/codex"]
            ),
            Some(Codex)
        );
        assert_eq!(
            identify_args(
                "/usr/bin/node",
                &[
                    "node",
                    "--no-warnings",
                    "/pkg/@anthropic-ai/claude-code/cli.js"
                ]
            ),
            Some(Claude)
        );
        assert_eq!(
            identify_args(
                "/usr/bin/node",
                &["node", "/pkg/@openai/codex/bin/codex.js"]
            ),
            Some(Codex)
        );
        assert_eq!(
            identify_args("/bin/bash", &["bash", "/tmp/bin/codex"]),
            Some(Codex)
        );
        assert_eq!(
            identify_args(
                "/usr/bin/node",
                &["node", "server.js", "/pkg/@anthropic-ai/claude-code/cli.js"]
            ),
            None
        );
        assert_eq!(identify_args("/bin/zsh", &["zsh", "-c", "codex"]), None);
        assert_eq!(
            identify_args("/opt/codex-code-mode-host", &["codex-code-mode-host"]),
            None
        );
    }

    #[test]
    fn recognizes_generic_agent_clis_by_command_entry_script_or_process_title() {
        use AgentKind::{Aider, Copilot, Cursor, Devin, Droid, Gemini, Qwen};
        let cursor = "/Users/me/.local/share/cursor-agent/versions/2025.10.28-0a91dc2";
        for (executable, args, expected) in [
            ("/Users/me/.local/share/devin/cli/_versions/3000.14/bin/devin", &["devin"][..], Some(Devin)),
            ("/Users/me/.local/bin/droid", &["droid", "exec", "fix it"][..], Some(Droid)),
            ("/usr/local/bin/node", &["node", "/usr/local/bin/gemini"][..], Some(Gemini)),
            ("/usr/local/bin/node", &["node", "--max-old-space-size=8192", "/usr/local/lib/node_modules/@google/gemini-cli/dist/index.js"][..], Some(Gemini)),
            ("/bin/bash", &["bash", "/Users/me/.local/bin/cursor-agent"][..], Some(Cursor)),
            (&format!("{cursor}/node"), &[&format!("{cursor}/node")[..], "--use-system-ca", &format!("{cursor}/index.js")][..], Some(Cursor)),
            ("/usr/local/bin/node", &["node", "/usr/local/bin/copilot"][..], Some(Copilot)),
            ("/opt/homebrew/bin/python3.12", &["/opt/homebrew/bin/python3.12", "/Users/me/.local/bin/aider"][..], Some(Aider)),
            ("/usr/local/bin/node", &["qwen"][..], Some(Qwen)),
            ("/bin/sh", &["/bin/sh", "/tmp/fixture/devin"][..], Some(Devin)),
            ("/usr/local/bin/copilot", &["copilot", "deploy"][..], None),
            ("/usr/local/bin/node", &["node", "server.js", "/usr/local/bin/gemini"][..], None),
            ("/usr/local/bin/goose", &["goose", "up"][..], None),
            ("/usr/local/bin/devin-helper", &["devin-helper"][..], None),
        ] {
            assert_eq!(identify_args(executable, args), expected, "{executable} {args:?}");
        }
        assert_eq!((Devin.label(), Cursor.label(), AgentKind::IFlow.label()), ("Devin", "Cursor", "iFlow"));
        assert!(Claude.resumable() && Codex.resumable() && !Devin.resumable());
    }

    #[test]
    fn generic_notifications_ask_for_a_decision_or_end_the_round() {
        for (raw, expected) in [
            (&b"777;notify;Devin;Devin needs input"[..], Some(Waiting)),
            (b"9;Droid needs permission to run a tool", Some(Waiting)),
            (b"9;Droid is waiting for approval to run git push", Some(Waiting)),
            ("9;需要你确认是否继续".as_bytes(), Some(Waiting)),
            (b"777;notify;Devin;Devin finished", Some(Done)),
            (b"9;Droid finished its turn", Some(Done)),
            (b"9;4;1;50", None),
            (b"9;", None),
            (b"0;Devin", None),
        ] {
            assert_eq!(generic_notice(raw), expected, "{}", String::from_utf8_lossy(raw));
        }
    }

    fn spin(tracker: &mut Tracker, from: u64, to: u64) {
        for (index, at) in (from..=to).step_by(100).enumerate() {
            let frame = ['|', '/', '-', '\\'][index % 4];
            tracker.observe(format!("\r\x1b[2K{frame} working").as_bytes(), at, false);
            tracker.tick(at, false);
        }
    }

    #[test]
    fn generic_cli_rounds_follow_enter_notifications_and_sustained_output() {
        let probe = Probe::new(|| None, 80, 24);
        let mut sample = probe.sampling.lock().unwrap();
        let mut devin = Tracker::new(AgentKind::Devin, 0, 80, 24);
        devin.observe(b"Devin CLI\r\n> ", 200, true);
        assert_eq!(devin.status(), None);
        devin.tick(GENERIC_READY_MS, true);
        let idle = devin.status().unwrap();
        assert_eq!((idle.state, idle.acknowledged), (Done, true));
        sample.input(&mut devin, b"fix the build\r", 6_000);
        assert_eq!((devin.session.state, devin.session.round_started_at), (Thinking, Some(6_000)));
        spin(&mut devin, 6_400, 12_000);
        assert_eq!(devin.session.state, Thinking);
        devin.observe(b"\x07\x1b]9;Devin needs input\x07\x1b]777;notify;Devin;Devin needs input\x07", 12_050, false);
        assert_eq!(devin.session.state, Waiting);
        devin.tick(30_000, false);
        assert_eq!(devin.session.state, Waiting);
        sample.input(&mut devin, b"y", 30_000);
        devin.observe(b"\r\x1b[2Ky", 30_050, false);
        spin(&mut devin, 30_600, 32_500);
        assert_eq!(devin.session.state, Waiting);
        spin(&mut devin, 32_600, 32_700);
        assert_eq!((devin.session.state, devin.session.round_started_at), (Thinking, Some(6_000)));
        devin.tick(32_700 + GENERIC_QUIET_MS - 1, false);
        assert_eq!(devin.session.state, Thinking);
        devin.tick(32_700 + GENERIC_QUIET_MS, false);
        assert_eq!(devin.session.state, Done);
        assert!(devin.session.pending());
    }

    #[test]
    fn generic_cli_ignores_startup_paint_typing_and_progress_and_settles_bells() {
        let probe = Probe::new(|| None, 80, 24);
        let mut sample = probe.sampling.lock().unwrap();
        let mut droid = Tracker::new(AgentKind::Droid, 0, 80, 24);
        spin(&mut droid, 1_000, 4_900);
        assert_eq!(droid.status().unwrap().state, Done);
        for (index, at) in (10_000..14_000).step_by(150).enumerate() {
            sample.input(&mut droid, b"x", at);
            droid.observe(format!("\r\x1b[2K> {}", "x".repeat(index + 1)).as_bytes(), at + 20, false);
        }
        assert_eq!((droid.session.state, droid.session.round_started_at), (Done, None));
        sample.input(&mut droid, b"\r", 14_000);
        assert_eq!(droid.session.state, Thinking);
        droid.observe(b"\x1b]9;4;3;\x07", 14_100, false);
        assert_eq!(droid.session.state, Thinking);
        droid.observe(b"\x07", 15_000, false);
        droid.observe(b"\r\x1b[2Kfinal answer", 15_200, false);
        droid.tick(15_600, false);
        assert_eq!(droid.session.state, Thinking);
        droid.tick(15_200 + GENERIC_BELL_SETTLE_MS, false);
        assert_eq!(droid.session.state, Done);
        sample.input(&mut droid, b"next\r", 20_000);
        droid.observe(b"\x1b]9;Droid finished its turn\x07", 21_000, true);
        let done = droid.status().unwrap();
        assert_eq!((done.state, done.acknowledged), (Done, true));
    }

    #[test]
    fn claude_working_titles_and_notifications_survive_fragmented_streams() {
        let features = Claude.features().unwrap();
        assert_eq!(osc(features, b"9;4;50").1, None);
        assert_eq!(
            osc(
                features,
                b"777;notify;Claude Code;Claude needs your permission"
            )
            .1,
            Some(Waiting)
        );
        let mut claude = Tracker::new(Claude, 0, 80, 24);
        claude.observe("\x1b]2;◐ Review".as_bytes(), 10, false);
        assert_eq!(claude.session.state, Done);
        claude.observe(b"\x1b", 11, false);
        claude.observe(b"\\", 12, false);
        assert_eq!(claude.session.state, Thinking);
        assert_eq!(claude.session.round_started_at, Some(12));
        claude.observe(b"\x1b]9;Claude needs your ", 20, false);
        claude.observe(b"permission\x07", 22, false);
        assert_eq!(claude.session.state, Waiting);
        claude.observe("\x1b]0;✳ Review\x07".as_bytes(), 30, false);
        claude.tick(10_000, false);
        assert_eq!(claude.session.state, Waiting);
        claude.submit(10_010);
        assert_eq!(claude.session.round_started_at, Some(10_010));
        claude.observe(
            b"\x1b]777;notify;Claude Code;Claude is waiting for your input\x1b\\",
            10_040,
            false,
        );
        assert_eq!(claude.session.state, Done);
        assert!(claude.session.pending());
        claude.acknowledge();
        assert!(!claude.session.pending());
    }

    #[test]
    fn claude_permission_prompt_wins_over_idle_title_and_bel_has_context() {
        let mut claude = Tracker::new(Claude, 0, 80, 24);
        claude.observe(
            "\x1b]0;◑ Fixture\x07\x1b]0;✳ Fixture\x07".as_bytes(),
            100,
            false,
        );
        assert_eq!(claude.session.state, Thinking);
        claude.submit(200);
        claude.observe(
            "\x1b]0;✳ Fixture\x07Do you want to proceed?\r\n".as_bytes(),
            300,
            false,
        );
        claude.observe(b"\x1b[1m1. Ye", 350, false);
        claude.observe(b"s\x1b[0m\x07", 351, false);
        claude.tick(2_000, false);
        assert_eq!(claude.session.state, Waiting);
        claude.submit(2_100);
        claude.observe(b"\x07", 2_200, true);
        claude.tick(2_200 + CLAUDE_IDLE_SETTLE_MS, true);
        assert_eq!(claude.session.state, Done);
        assert!(claude.session.acknowledged);
    }

    #[test]
    fn codex_keeps_approval_during_quiet_repaints_and_finishes_after_dismissal() {
        let mut codex = Tracker::new(Codex, 0, 80, 24);
        codex.observe(b"\x1b[9;2HWorking", 100, false);
        assert_eq!(codex.session.state, Thinking);
        codex.observe(
            b"\x1b[12;2HWould you like to run the following comm",
            900,
            false,
        );
        codex.observe(
            b"and?\x1b[14;4H1. Yes, \x1b[1mproceed (y)\x1b[0m",
            950,
            false,
        );
        assert_eq!(codex.session.state, Waiting);
        codex.observe(b"\x1b[14;2H> ", 1_500, false);
        codex.tick(8_000, false);
        assert_eq!(codex.session.state, Waiting);
        codex.observe(b"\x1b[11;1H\x1b[J", 8_010, false);
        assert_eq!(codex.session.state, Thinking);
        assert_eq!(codex.session.round_started_at, Some(100));
        codex.tick(11_009, false);
        assert_eq!(codex.session.state, Thinking);
        codex.tick(11_010, false);
        assert_eq!(codex.session.state, Done);
        assert!(!codex.session.acknowledged);
        codex.submit(12_000);
        assert_eq!(codex.session.round_started_at, Some(12_000));
    }

    #[test]
    fn generic_text_control_traffic_and_multiline_paste_do_not_fake_agent_signals() {
        let mut codex = Tracker::new(Codex, 0, 80, 24);
        codex.observe(b"\x1b]2;approve\x07\x1b[?25h\x1b[6n", 1, false);
        assert_eq!(codex.status(), None);
        codex.observe(
            b"Working directory unavailable.\r\nPlease approve the change in your own time.",
            2,
            false,
        );
        codex.submit(3);
        codex.tick(4_000, false);
        assert_eq!(codex.status(), None);
        codex.observe("\r\n› Send a message".as_bytes(), 5_000, false);
        assert_eq!(codex.status().unwrap().state, Done);
        assert_eq!(codex.session.round_started_at, None);
        let probe = Probe::new(|| None, 80, 24);
        let mut sample = probe.sampling.lock().unwrap();
        sample.input(&mut codex, b"\x1b[200~first\rsecond\r\x1b[201~\n", 5_001);
        assert_eq!(codex.session.round_started_at, None);
        sample.input(&mut codex, b"new request\r", 5_002);
        assert_eq!(codex.session.round_started_at, Some(5_002));
        assert_eq!(codex.status().unwrap().state, Thinking);
        codex.tick(8_002, true);
        assert_eq!(codex.status().unwrap().state, Done);
    }

    #[test]
    fn resume_id_is_read_from_the_cli_entry_point_only() {
        let id = "383c4fef-fd6e-452a-9130-bdbdea62bc01";
        let inspect = |exe: &str, argv: &[&str]| {
            let (kind, args) = identify(exe.as_bytes(), argv.iter().map(|arg| arg.as_bytes()))?;
            resume_arg(kind, args)
        };
        assert_eq!(
            inspect(
                "/usr/bin/node",
                &["node", "/pkg/@openai/codex/bin/codex.js", "resume", id]
            ),
            SessionRef::new(Codex, id)
        );
        assert_eq!(
            inspect("/bin/claude", &["claude", "--resume", id]),
            SessionRef::new(Claude, id)
        );
        assert_eq!(inspect("/bin/codex", &["codex", "resume", "--last"]), None);
        assert_eq!(
            inspect("/bin/codex", &["codex", "exec", "resume", id]),
            None
        );
        assert_eq!(
            inspect("/bin/sh", &["sh", "-c", "codex", "resume", id]),
            None
        );
    }
}
