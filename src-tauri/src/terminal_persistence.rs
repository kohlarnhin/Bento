use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_BYTES: usize = 1024 * 1024;
const MAX_LINES: usize = 5000;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct SavedTerminal {
    pub slot: u8,
    pub cwd: PathBuf,
    pub title: Option<String>,
    pub history: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct SavedProject {
    pub active: Option<u8>,
    pub saved_at: u64,
    pub terminals: Vec<SavedTerminal>,
}

pub type SavedSessions = BTreeMap<String, SavedProject>;

pub fn bounded_history(raw: &str) -> String {
    let text = crate::terminal_escape::history_text(raw);
    let mut start = text.len().saturating_sub(MAX_BYTES);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    let lines = text.strip_suffix('\n').unwrap_or(&text);
    if let Some((boundary, _)) = lines.match_indices('\n').rev().nth(MAX_LINES - 1) {
        start = start.max(boundary + 1);
    }
    text[start..].to_owned()
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn replay(terminal: &SavedTerminal, saved_at: u64) -> String {
    let history = bounded_history(&terminal.history).replace('\n', "\r\n");
    let mut time: libc::tm = unsafe { std::mem::zeroed() };
    let timestamp = saved_at as libc::time_t;
    unsafe {
        libc::localtime_r(&timestamp, &mut time);
    }
    format!(
        "{history}\r\n\x1b[90m—— 已恢复 · 上次会话 {:02}:{:02} ——\x1b[0m\r\n",
        time.tm_hour, time.tm_min
    )
}

pub struct Store {
    directory: PathBuf,
    pub enabled: bool,
    previous: SavedSessions,
}

impl Store {
    pub fn new(directory: PathBuf, enabled: bool) -> Self {
        Self {
            directory,
            enabled,
            previous: BTreeMap::new(),
        }
    }

    fn file(&self) -> PathBuf {
        self.directory.join("sessions.json")
    }

    pub fn load(&mut self) -> io::Result<SavedSessions> {
        let file = match fs::File::open(self.file()) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
            Err(error) => return Err(error),
        };
        let sessions = serde_json::from_reader(io::BufReader::new(file.take(256 * 1024 * 1024)))?;
        self.previous = sessions;
        Ok(self.previous.clone())
    }

    pub fn save(&mut self, mut sessions: SavedSessions) -> io::Result<()> {
        if !self.enabled {
            return Ok(());
        }
        for (id, entry) in &mut sessions {
            if let Some(previous) = self.previous.get(id) {
                if previous.active == entry.active && previous.terminals == entry.terminals {
                    entry.saved_at = previous.saved_at;
                }
            }
        }
        if sessions == self.previous {
            return Ok(());
        }
        if sessions.is_empty() {
            self.clear()?;
            return Ok(());
        }
        DirBuilder::new()
            .mode(0o700)
            .recursive(true)
            .create(&self.directory)?;
        let temp = self
            .directory
            .join(format!(".sessions-{}.tmp", std::process::id()));
        let result = write_private(&temp, &sessions).and_then(|_| fs::rename(&temp, self.file()));
        if result.is_err() {
            let _ = fs::remove_file(&temp);
        }
        result?;
        self.previous = sessions;
        Ok(())
    }

    pub fn clear(&mut self) -> io::Result<()> {
        match fs::remove_file(self.file()) {
            Ok(()) => (),
            Err(error) if error.kind() == io::ErrorKind::NotFound => (),
            Err(error) => return Err(error),
        }
        self.previous.clear();
        Ok(())
    }
}

fn write_private(path: &Path, sessions: &SavedSessions) -> io::Result<()> {
    let file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)?;
    let mut writer = io::BufWriter::new(file);
    serde_json::to_writer(&mut writer, sessions)?;
    writer.flush()?;
    writer.get_ref().sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persists_tabs_active_slot_cwd_and_replays_bounded_read_only_history() {
        let directory = std::env::temp_dir().join(format!("bento-history-{}", std::process::id()));
        let mut writer = Store::new(directory.clone(), true);
        let mut terminal = SavedTerminal {
            slot: 2,
            cwd: PathBuf::from("/tmp/开发"),
            title: Some("终端 2".into()),
            history: bounded_history(&("一行\n".repeat(5050) + "last\x1b]133;D;0\x07")),
        };
        assert_eq!(terminal.history.lines().count(), 5000);
        let written = BTreeMap::from([(
            "project".into(),
            SavedProject {
                active: Some(2),
                saved_at: 1_795_123_400,
                terminals: vec![terminal.clone()],
            },
        )]);
        writer.save(written.clone()).unwrap();
        let mut reader = Store::new(directory.clone(), true);
        assert_eq!(reader.load().unwrap(), written);
        let loaded = &written["project"];
        let playback = replay(&loaded.terminals[0], loaded.saved_at);
        assert!(playback.contains("last\r\n\x1b[90m—— 已恢复 · 上次会话 "));
        assert!(!playback.contains("133;"));
        terminal.history = bounded_history(&"字".repeat(MAX_BYTES));
        assert!(terminal.history.len() <= MAX_BYTES && terminal.history.ends_with('字'));
        reader.save(BTreeMap::new()).unwrap();
        assert!(!reader.file().exists());
        let _ = fs::remove_dir(directory);
    }
}
