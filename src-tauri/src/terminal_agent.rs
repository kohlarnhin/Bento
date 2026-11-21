mod foreground;
mod screen;
pub(crate) mod sessions;

use crate::terminal_escape::{EscapeEvent, EscapeStream};
use foreground::{Detector, ForegroundAgent};
use screen::Screen;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

const CODEX_QUIET_MS: u64 = 3_000;
// Claude can stop animating its title just before it paints the approval panel.
const CLAUDE_IDLE_SETTLE_MS: u64 = 500;

#[derive(Clone, Copy, Debug, Deserialize, Hash, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentKind {
    Claude,
    Codex,
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

impl AgentKind {
    fn features(self) -> &'static Features {
        match self {
            Self::Claude => &FEATURES[0],
            Self::Codex => &FEATURES[1],
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
            Self::Codex => "Codex",
        }
    }
}

fn basename(path: &[u8]) -> &[u8] {
    path.rsplit(|&byte| byte == b'/').next().unwrap_or_default()
}

fn identify<'a>(executable: &[u8], mut argv: impl Iterator<Item = &'a [u8]>) -> Option<AgentKind> {
    let executable = basename(executable);
    let command = basename(argv.next().unwrap_or_default());
    if let Some(feature) = FEATURES
        .iter()
        .find(|feature| feature.command == executable || feature.command == command)
    {
        return Some(feature.kind);
    }
    let interpreter = if executable.is_empty() {
        command
    } else {
        executable
    };
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
            b"sh" | b"bash" | b"zsh" | b"fish" | b"python" | b"python3" => {
                basename(script) == feature.command
            }
            _ => false,
        })
        .map(|feature| feature.kind)
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
                    let (title, notice) = osc(features, &raw);
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
        if seen.changed {
            self.approval = approval(features, &self.screen.tail());
        }
        seen.approval = self.approval;
        seen
    }
}

pub struct Tracker {
    pub session: AgentSession,
    pub process: Option<AgentProcess>,
    output: Output,
    last_activity: u64,
    idle_at: Option<u64>,
    title: Option<Title>,
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
            output: Output::default(),
            last_activity: now,
            idle_at: None,
            title: None,
        };
        tracker.resize(cols, rows);
        tracker
    }

    pub fn acknowledge(&mut self) {
        if self.session.state == AgentState::Done {
            self.session.acknowledged = true;
        }
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
        self.session.round_started_at = Some(now);
        self.idle_at = None;
        self.last_activity = now;
        self.output.screen.clear_text();
        self.output.approval = false;
        self.state(AgentState::Thinking, now, false);
    }

    pub fn observe(&mut self, bytes: &[u8], now: u64, visible: bool) {
        let had_approval = self.output.approval;
        let seen = self.output.feed(self.session.kind, bytes);
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
            if seen.changed {
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
        if self.session.state == AgentState::Thinking
            && (match self.session.kind {
                AgentKind::Claude => self.idle_at.is_some_and(|idle| now >= idle),
                AgentKind::Codex => now.saturating_sub(self.last_activity) >= CODEX_QUIET_MS,
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AgentProcess {
    pub kind: AgentKind,
    pub pid: u32,
    pub started_at: u64,
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
            self.active = self.observed;
            self.pasting = false;
            *tracker = self.active.map(|agent| {
                let mut tracker = Tracker::new(agent.kind, now, self.cols, self.rows);
                let pid = agent.pid as u32;
                tracker.process = Some(AgentProcess {
                    kind: agent.kind,
                    pid,
                    started_at: crate::processes::identity(pid)
                        .map_or(now, |identity| identity.started_at_millis()),
                });
                tracker
            });
        }
    }

    pub fn input(&mut self, tracker: &mut Tracker, mut bytes: &[u8], now: u64) {
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
        identify(executable.as_bytes(), args.iter().map(|arg| arg.as_bytes()))
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
    fn claude_working_titles_and_notifications_survive_fragmented_streams() {
        let features = Claude.features();
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
        assert_eq!(codex.session.state, Done);
        codex.observe(b"Please approve the change in your own time.", 2, false);
        assert_eq!(codex.session.state, Thinking);
        let probe = Probe::new(|| None, 80, 24);
        let mut sample = probe.sampling.lock().unwrap();
        sample.input(&mut codex, b"\x1b[200~first\rsecond\r\x1b[201~\n", 3);
        assert_eq!(codex.session.round_started_at, Some(2));
        sample.input(&mut codex, b"new request\r", 4);
        assert_eq!(codex.session.round_started_at, Some(4));
    }
}
