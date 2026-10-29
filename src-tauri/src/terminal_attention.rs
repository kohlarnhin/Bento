use crate::terminal::TerminalKey;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc, Mutex,
};
use std::time::{Duration, Instant};

pub const MIN_OUTPUT: Duration = Duration::from_secs(2);
pub const SILENCE: Duration = Duration::from_secs(3);
const TICK: Duration = Duration::from_millis(250);

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttentionEvent {
    pub project_id: String,
    pub slot: u8,
    pub session_id: String,
    pub attention: bool,
}

#[derive(Default)]
enum Mode {
    #[default]
    Ground,
    Escape,
    Osc,
    OscEscape,
    String,
    StringEscape,
}

struct Osc {
    command: Option<u16>,
    payload: bool,
    nonempty: bool,
    first_done: bool,
    first_len: usize,
    numeric: bool,
    notify: bool,
}

impl Default for Osc {
    fn default() -> Self {
        Self {
            command: Some(0),
            payload: false,
            nonempty: false,
            first_done: false,
            first_len: 0,
            numeric: true,
            notify: true,
        }
    }
}

impl Osc {
    fn byte(&mut self, byte: u8) {
        if !self.payload {
            if byte == b';' {
                self.payload = true;
            } else {
                self.command = self.command.and_then(|code| {
                    if byte.is_ascii_digit() {
                        code.checked_mul(10)?.checked_add((byte - b'0') as u16)
                    } else {
                        None
                    }
                });
            }
        } else {
            self.nonempty = true;
            if !self.first_done {
                if byte == b';' {
                    self.first_done = true;
                } else {
                    self.numeric &= byte.is_ascii_digit();
                    self.notify &= b"notify".get(self.first_len) == Some(&byte);
                    self.first_len = self.first_len.saturating_add(1);
                }
            }
        }
    }

    fn notification(&self) -> bool {
        self.payload
            && match self.command {
                Some(9) => self.nonempty && !(self.numeric && self.first_len > 0),
                Some(777) => self.notify && self.first_len == 6,
                _ => false,
            }
    }
}

#[derive(Default)]
struct Scanner {
    mode: Mode,
    osc: Osc,
}

impl Scanner {
    // No payload buffering: even arbitrarily long OSC strings use constant space.
    fn feed(&mut self, bytes: &[u8]) -> bool {
        let mut signal = false;
        for &byte in bytes {
            match self.mode {
                Mode::Ground => match byte {
                    7 => signal = true,
                    27 => self.mode = Mode::Escape,
                    _ => {}
                },
                Mode::Escape => match byte {
                    b']' => {
                        self.osc = Osc::default();
                        self.mode = Mode::Osc;
                    }
                    b'P' | b'X' | b'^' | b'_' => self.mode = Mode::String,
                    27 => {}
                    _ => self.mode = Mode::Ground,
                },
                Mode::Osc | Mode::OscEscape => {
                    let terminated =
                        byte == 7 || (matches!(self.mode, Mode::OscEscape) && byte == b'\\');
                    if terminated {
                        signal |= self.osc.notification();
                        self.mode = Mode::Ground;
                    } else if byte == 27 {
                        self.mode = Mode::OscEscape;
                    } else {
                        self.osc.byte(byte);
                        self.mode = Mode::Osc;
                    }
                }
                Mode::String => {
                    if byte == 27 {
                        self.mode = Mode::StringEscape;
                    }
                }
                Mode::StringEscape => match byte {
                    b'\\' => self.mode = Mode::Ground,
                    27 => {}
                    _ => self.mode = Mode::String,
                },
            }
        }
        signal
    }
}

#[derive(Default)]
struct Tracker {
    scanner: Scanner,
    episode: Option<(Duration, Duration)>,
    raised: bool,
}

impl Tracker {
    fn clear(&mut self) {
        self.raised = false;
        self.episode = None;
        // Keep VT state: a title's trailing BEL can arrive after focus changes.
    }

    fn output(&mut self, bytes: &[u8], now: Duration, watched: bool) {
        let explicit = self.scanner.feed(bytes);
        if watched {
            self.clear();
            return;
        }
        self.tick(now);
        if explicit {
            self.raised = true;
        }
        if !self.raised && !bytes.is_empty() {
            self.episode = Some((self.episode.map_or(now, |(first, _)| first), now));
        }
    }

    fn tick(&mut self, now: Duration) {
        if let Some((first, last)) = self.episode {
            if now.saturating_sub(last) >= SILENCE {
                self.episode = None;
                self.raised |= last.saturating_sub(first) >= MIN_OUTPUT;
            }
        }
    }
}

pub struct Sinks {
    pub change: Arc<dyn Fn(AttentionEvent) + Send + Sync>,
    pub badge: Arc<dyn Fn(usize) + Send + Sync>,
    pub inactive: Arc<dyn Fn() -> bool + Send + Sync>,
    pub bounce: Arc<dyn Fn() + Send + Sync>,
}

#[cfg(test)]
impl Default for Sinks {
    fn default() -> Self {
        Self {
            change: Arc::new(|_| {}),
            badge: Arc::new(|_| {}),
            inactive: Arc::new(|| false),
            bounce: Arc::new(|| {}),
        }
    }
}

struct Entry {
    session_id: String,
    tracker: Tracker,
}

#[derive(Default)]
struct State {
    watched: Option<TerminalKey>,
    sessions: HashMap<TerminalKey, Entry>,
}

enum Effect {
    Change(AttentionEvent, usize),
    Shutdown,
}

pub struct AttentionManager {
    state: Mutex<State>,
    clock: Arc<dyn Fn() -> Duration + Send + Sync>,
    effects: mpsc::Sender<Effect>,
    stopped: AtomicBool,
}

impl AttentionManager {
    pub fn new(sinks: Sinks) -> Arc<Self> {
        let started = Instant::now();
        Self::with_clock(sinks, Arc::new(move || started.elapsed()))
    }

    fn with_clock(sinks: Sinks, clock: Arc<dyn Fn() -> Duration + Send + Sync>) -> Arc<Self> {
        let (effects, receiver) = mpsc::channel();
        // All side effects run outside manager/PTY locks, in state-change order.
        std::thread::Builder::new()
            .name("terminal-attention-events".into())
            .spawn(move || {
                while let Ok(effect) = receiver.recv() {
                    match effect {
                        Effect::Change(event, count) => {
                            let raised = event.attention;
                            (sinks.change)(event);
                            (sinks.badge)(count);
                            if raised && (sinks.inactive)() {
                                (sinks.bounce)();
                            }
                        }
                        Effect::Shutdown => break,
                    }
                }
            })
            .expect("terminal attention event thread");
        let manager = Arc::new(Self {
            state: Mutex::new(State::default()),
            clock,
            effects,
            stopped: AtomicBool::new(false),
        });
        let weak = Arc::downgrade(&manager);
        std::thread::Builder::new()
            .name("terminal-attention-ticker".into())
            .spawn(move || loop {
                std::thread::sleep(TICK);
                let Some(manager) = weak.upgrade() else {
                    break;
                };
                if manager.stopped.load(Ordering::Relaxed) {
                    break;
                }
                manager.tick();
            })
            .expect("terminal attention ticker thread");
        manager
    }

    fn changed(&self, state: &State, event: AttentionEvent) {
        let count = state
            .sessions
            .values()
            .filter(|entry| entry.tracker.raised)
            .count();
        let _ = self.effects.send(Effect::Change(event, count));
    }

    pub fn register(&self, key: &TerminalKey, session_id: &str) {
        self.state.lock().unwrap().sessions.insert(
            key.clone(),
            Entry {
                session_id: session_id.into(),
                tracker: Tracker::default(),
            },
        );
    }

    pub fn raised(&self, key: &TerminalKey, session_id: &str) -> bool {
        self.state
            .lock()
            .unwrap()
            .sessions
            .get(key)
            .is_some_and(|entry| entry.session_id == session_id && entry.tracker.raised)
    }

    fn update(&self, key: &TerminalKey, session_id: &str, update: impl FnOnce(&mut Tracker, bool)) {
        let mut state = self.state.lock().unwrap();
        let watched = state.watched.as_ref() == Some(key);
        let Some(entry) = state
            .sessions
            .get_mut(key)
            .filter(|entry| entry.session_id == session_id)
        else {
            return;
        };
        let before = entry.tracker.raised;
        update(&mut entry.tracker, watched);
        let raised = entry.tracker.raised;
        if before != raised {
            self.changed(
                &state,
                AttentionEvent {
                    project_id: key.project_id.clone(),
                    slot: key.slot,
                    session_id: session_id.into(),
                    attention: raised,
                },
            );
        }
    }

    pub fn output(&self, key: &TerminalKey, session_id: &str, bytes: &[u8]) {
        let now = (self.clock)();
        self.update(key, session_id, |tracker, watched| {
            tracker.output(bytes, now, watched)
        });
    }

    pub fn resize(&self, key: &TerminalKey, session_id: &str) {
        self.update(key, session_id, |tracker, _| tracker.episode = None);
    }

    pub fn focus(&self, key: Option<TerminalKey>) {
        let mut state = self.state.lock().unwrap();
        state.watched = key;
        let event = state.watched.clone().and_then(|id| {
            let entry = state.sessions.get_mut(&id)?;
            let raised = entry.tracker.raised;
            entry.tracker.clear();
            raised.then(|| AttentionEvent {
                project_id: id.project_id,
                slot: id.slot,
                session_id: entry.session_id.clone(),
                attention: false,
            })
        });
        if let Some(event) = event {
            self.changed(&state, event);
        }
    }

    pub fn remove(&self, key: &TerminalKey, session_id: &str) {
        let mut state = self.state.lock().unwrap();
        if !state
            .sessions
            .get(key)
            .is_some_and(|entry| entry.session_id == session_id)
        {
            return;
        }
        if let Some(entry) = state
            .sessions
            .remove(key)
            .filter(|entry| entry.tracker.raised)
        {
            self.changed(
                &state,
                AttentionEvent {
                    project_id: key.project_id.clone(),
                    slot: key.slot,
                    session_id: entry.session_id,
                    attention: false,
                },
            );
        }
    }

    fn tick(&self) {
        let now = (self.clock)();
        let mut state = self.state.lock().unwrap();
        let mut changes = Vec::new();
        for (key, entry) in &mut state.sessions {
            let before = entry.tracker.raised;
            entry.tracker.tick(now);
            if !before && entry.tracker.raised {
                changes.push(AttentionEvent {
                    project_id: key.project_id.clone(),
                    slot: key.slot,
                    session_id: entry.session_id.clone(),
                    attention: true,
                });
            }
        }
        for event in changes {
            self.changed(&state, event);
        }
    }

    pub fn shutdown(&self) {
        self.stopped.store(true, Ordering::Relaxed);
        let _ = self.effects.send(Effect::Shutdown);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attention_focus_restart_remove_and_badges_are_isolated_between_project_slots() {
        let (badge, badges) = mpsc::channel();
        let manager = AttentionManager::new(Sinks { badge: Arc::new(move |count| { badge.send(count).unwrap(); }), ..Sinks::default() });
        let one = TerminalKey::new("a", 1);
        let two = TerminalKey::new("a", 2);
        manager.register(&one, "one");
        manager.register(&two, "two");
        manager.focus(Some(one.clone()));
        manager.output(&one, "one", b"\x07");
        manager.output(&two, "two", b"\x07");
        assert_eq!(badges.recv_timeout(Duration::from_secs(1)).unwrap(), 1);
        assert!(!manager.raised(&one, "one"));
        assert!(manager.raised(&two, "two"));
        manager.focus(None);
        manager.output(&one, "one", b"\x07");
        assert_eq!(badges.recv_timeout(Duration::from_secs(1)).unwrap(), 2);
        manager.focus(Some(two.clone()));
        assert_eq!(badges.recv_timeout(Duration::from_secs(1)).unwrap(), 1);
        manager.remove(&two, "two");
        manager.register(&two, "replacement");
        manager.remove(&two, "two");
        manager.output(&two, "two", b"\x07");
        assert!(!manager.raised(&two, "replacement"));
        assert!(manager.raised(&one, "one"));
        manager.remove(&one, "one");
        assert_eq!(badges.recv_timeout(Duration::from_secs(1)).unwrap(), 0);
        manager.shutdown();
    }

    #[test]
    fn scanner_distinguishes_notifications_from_control_string_bells_across_reads() {
        let cases: &[(&[u8], bool)] = &[
            (b"\x07", true),
            (b"\x1b]9;hello\x07", true),
            (b"\x1b]9;hello\x1b\\", true),
            (b"\x1b]777;notify;title;body\x07", true),
            (b"\x1b]777;notify;title;body\x1b\\", true),
            (b"\x1b]0;title\x07", false),
            (b"\x1b]2;title\x07", false),
            (b"\x1b]9;4;1;50\x07", false),
            (b"\x1b]9;123\x07", false),
            (b"\x1b]9;\x07", false),
            (b"\x1b]777;notification;title\x07", false),
            (b"\x1bPpayload\x07\x1b\\", false),
            (b"\x1b_payload\x07\x1b\\", false),
            (b"\x1bXpayload\x07\x1b\\", false),
            (b"\x1b^payload\x07\x1b\\", false),
            (b"\x1b]0;title\x07\x07", true),
            (b"\x1b_payload\x07\x1b\\\x07", true),
        ];
        for &(bytes, expected) in cases {
            for split in 0..=bytes.len() {
                let mut scanner = Scanner::default();
                let first = scanner.feed(&bytes[..split]);
                assert_eq!(
                    first | scanner.feed(&bytes[split..]),
                    expected,
                    "{bytes:?}, split {split}"
                );
            }
            let mut scanner = Scanner::default();
            let mut actual = false;
            for byte in bytes {
                actual |= scanner.feed(&[*byte]);
            }
            assert_eq!(actual, expected);
        }
    }

    fn at(ms: u64) -> Duration {
        Duration::from_millis(ms)
    }

    #[test]
    fn sustained_output_needs_two_seconds_then_three_seconds_of_silence() {
        let mut tracker = Tracker::default();
        tracker.output(b"start", at(0), false);
        tracker.output(b"work", at(1000), false);
        tracker.output(b"done", at(2000), false);
        tracker.tick(at(4999));
        assert!(!tracker.raised);
        tracker.tick(at(5000));
        assert!(tracker.raised);
        tracker.clear();
        tracker.output(b"start", at(6000), false);
        tracker.output(b"short", at(7999), false);
        tracker.tick(at(11000));
        assert!(!tracker.raised);
    }

    #[test]
    fn isolated_redraws_resize_and_watched_output_do_not_raise() {
        let mut tracker = Tracker::default();
        for time in [0, 4000, 8000, 12000] {
            tracker.output(b"redraw", at(time), false);
        }
        tracker.tick(at(16000));
        assert!(!tracker.raised);
        tracker.output(b"start", at(20000), false);
        tracker.output(b"work", at(22000), false);
        tracker.episode = None; // Bento resize starts a fresh episode.
        tracker.output(b"resize redraw", at(23000), false);
        tracker.tick(at(26000));
        assert!(!tracker.raised);
        for time in [30000, 31000, 32000] {
            tracker.output(b"\x07working", at(time), true);
        }
        tracker.tick(at(40000));
        assert!(!tracker.raised);
        tracker.output(b"\x1b]0;title", at(41000), true);
        tracker.clear();
        tracker.output(b"\x07", at(42000), false);
        tracker.tick(at(45000));
        assert!(!tracker.raised);
    }

    #[test]
    fn manager_uses_injected_clock_and_resize_restarts_the_episode() {
        use std::sync::atomic::AtomicU64;
        let time = Arc::new(AtomicU64::new(0));
        let clock = time.clone();
        let manager = AttentionManager::with_clock(
            Sinks::default(),
            Arc::new(move || at(clock.load(Ordering::Relaxed))),
        );
        manager.register(&TerminalKey::new("a", 1), "session");
        manager.output(&TerminalKey::new("a", 1), "session", b"start");
        time.store(2000, Ordering::Relaxed);
        manager.output(&TerminalKey::new("a", 1), "session", b"done");
        time.store(4999, Ordering::Relaxed);
        manager.tick();
        assert!(!manager.raised(&TerminalKey::new("a", 1), "session"));
        time.store(5000, Ordering::Relaxed);
        manager.tick();
        assert!(manager.raised(&TerminalKey::new("a", 1), "session"));
        manager.focus(Some(TerminalKey::new("a", 1)));
        manager.focus(None);
        time.store(6000, Ordering::Relaxed);
        manager.output(&TerminalKey::new("a", 1), "session", b"start");
        time.store(8000, Ordering::Relaxed);
        manager.output(&TerminalKey::new("a", 1), "session", b"work");
        manager.resize(&TerminalKey::new("a", 1), "session");
        manager.output(&TerminalKey::new("a", 1), "session", b"redraw");
        time.store(11000, Ordering::Relaxed);
        manager.tick();
        assert!(!manager.raised(&TerminalKey::new("a", 1), "session"));
        manager.shutdown();
    }

    #[test]
    fn badge_counts_unread_sessions_and_clears_each_independently() {
        let (badge, badges) = mpsc::channel();
        let manager = AttentionManager::new(Sinks {
            badge: Arc::new(move |count| {
                badge.send(count).unwrap();
            }),
            ..Sinks::default()
        });
        manager.register(&TerminalKey::new("a", 1), "one");
        manager.register(&TerminalKey::new("b", 1), "two");
        manager.output(&TerminalKey::new("a", 1), "one", b"\x07");
        assert_eq!(badges.recv_timeout(Duration::from_secs(1)).unwrap(), 1);
        manager.output(&TerminalKey::new("b", 1), "two", b"\x07");
        assert_eq!(badges.recv_timeout(Duration::from_secs(1)).unwrap(), 2);
        manager.focus(Some(TerminalKey::new("a", 1)));
        assert_eq!(badges.recv_timeout(Duration::from_secs(1)).unwrap(), 1);
        assert!(manager.raised(&TerminalKey::new("b", 1), "two"));
        manager.remove(&TerminalKey::new("b", 1), "two");
        assert_eq!(badges.recv_timeout(Duration::from_secs(1)).unwrap(), 0);
        manager.shutdown();
    }

    #[test]
    fn manager_focus_badges_and_bounce_are_session_scoped_and_not_repeated() {
        let (badge, badges) = mpsc::channel();
        let (bounce, bounces) = mpsc::channel();
        let inactive = Arc::new(AtomicBool::new(false));
        let window = inactive.clone();
        let manager = AttentionManager::new(Sinks {
            badge: Arc::new(move |count| {
                badge.send(count).unwrap();
            }),
            inactive: Arc::new(move || window.load(Ordering::Relaxed)),
            bounce: Arc::new(move || {
                bounce.send(()).unwrap();
            }),
            ..Sinks::default()
        });
        manager.register(&TerminalKey::new("a", 1), "one");
        manager.output(&TerminalKey::new("a", 1), "one", b"\x07");
        assert_eq!(badges.recv_timeout(Duration::from_secs(1)).unwrap(), 1);
        manager.focus(Some(TerminalKey::new("a", 1)));
        assert_eq!(badges.recv_timeout(Duration::from_secs(1)).unwrap(), 0);
        assert!(!manager.raised(&TerminalKey::new("a", 1), "one"));
        assert!(bounces.try_recv().is_err());
        manager.output(&TerminalKey::new("a", 1), "one", b"\x07");
        assert!(!manager.raised(&TerminalKey::new("a", 1), "one"));
        manager.focus(None);
        inactive.store(true, Ordering::Relaxed);
        manager.output(&TerminalKey::new("a", 1), "one", b"\x07");
        manager.output(&TerminalKey::new("a", 1), "one", b"\x07");
        assert_eq!(badges.recv_timeout(Duration::from_secs(1)).unwrap(), 1);
        bounces.recv_timeout(Duration::from_secs(1)).unwrap();
        manager.remove(&TerminalKey::new("a", 1), "one");
        assert_eq!(badges.recv_timeout(Duration::from_secs(1)).unwrap(), 0);
        assert!(bounces.try_recv().is_err());
        manager.register(&TerminalKey::new("a", 1), "two");
        manager.output(&TerminalKey::new("a", 1), "one", b"\x07");
        assert!(!manager.raised(&TerminalKey::new("a", 1), "two"));
        manager.shutdown();
    }
}
