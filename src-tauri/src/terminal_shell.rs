use portable_pty::CommandBuilder;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const MIN_COMMAND_DURATION: Duration = Duration::from_secs(10);

pub fn install(path: &Path) -> std::io::Result<PathBuf> {
    let path = path.join("shell-integration");
    std::fs::create_dir_all(&path)?;
    let zsh_source = include_str!("shell-integration/zsh-source");
    let zsh = |file: &str, prefix: &str, suffix: &str| {
        format!("{prefix}_bento_rc_file={file}\n{zsh_source}{suffix}")
    };
    for (name, contents) in [
        (
            "common.sh",
            include_str!("shell-integration/common.sh").to_owned(),
        ),
        (
            ".zshenv",
            zsh(".zshenv", include_str!("shell-integration/zshenv"), ""),
        ),
        (".zprofile", zsh(".zprofile", "", "")),
        (".zshrc", zsh(".zshrc", "", "")),
        (
            ".zlogin",
            zsh(".zlogin", "", include_str!("shell-integration/zlogin")),
        ),
        (
            "bashrc",
            include_str!("shell-integration/bashrc").to_owned(),
        ),
    ] {
        let destination = path.join(name);
        if std::fs::read_to_string(&destination).as_deref().ok() == Some(contents.as_str()) {
            continue;
        }
        let temp = destination.with_extension(format!("{}.tmp", std::process::id()));
        std::fs::write(&temp, contents)?;
        std::fs::rename(temp, destination)?;
    }
    Ok(path)
}

pub fn command(
    integration: Option<&Path>,
    cwd: &Path,
    env: &[crate::projects::EnvVar],
) -> CommandBuilder {
    let prepare = |mut command: CommandBuilder| {
        command.cwd(cwd);
        crate::terminal::configure_environment(&mut command);
        for entry in env {
            command.env(&entry.key, &entry.value);
        }
        command
    };
    let login = prepare(CommandBuilder::new_default_prog());
    let Some(directory) = integration else {
        return login;
    };
    let shell = login.get_shell();
    match Path::new(&shell).file_name().and_then(|name| name.to_str()) {
        Some("zsh") => {
            let original = login.get_env("ZDOTDIR").unwrap_or_default().to_owned();
            let mut command = prepare(CommandBuilder::new(&shell));
            command.arg("-il");
            command.env("BENTO_USER_ZDOTDIR", original);
            command.env("ZDOTDIR", directory);
            command
        }
        Some("bash") => {
            let mut command = prepare(CommandBuilder::new(&shell));
            command.arg("--noprofile");
            command.arg("--rcfile");
            command.arg(directory.join("bashrc"));
            command.arg("-i");
            command
        }
        _ => login,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Completion {
    pub command: String,
    pub elapsed_ms: u64,
    pub exit_code: i32,
    pub ended_at: u64,
}

pub fn elapsed_text(millis: u64) -> String {
    let seconds = millis / 1000;
    if seconds >= 60 {
        format!("{}m {}s", seconds / 60, seconds % 60)
    } else {
        format!("{seconds}s")
    }
}

impl Completion {
    pub fn divider(&self) -> String {
        format!(
            "\r\n\x1b[90m—— 用时 {} · 退出码 {} ——\x1b[0m\r\n",
            elapsed_text(self.elapsed_ms),
            self.exit_code
        )
    }
}

#[derive(Default)]
pub struct Boundaries(Option<(Instant, String)>);

impl Boundaries {
    pub fn observe(&mut self, payload: &str, now: Instant) -> Option<Completion> {
        let (marker, value) = payload.split_once(';').unwrap_or((payload, ""));
        match marker {
            "A" => self.0 = None,
            "C" => {
                let command = value.split_whitespace().collect::<Vec<_>>().join(" ");
                self.0 = Some((
                    now,
                    command
                        .chars()
                        .filter(|ch| !ch.is_control())
                        .take(240)
                        .collect(),
                ));
            }
            "D" => {
                let (started, command) = self.0.take()?;
                let elapsed = now.saturating_duration_since(started);
                let exit_code = value.parse().ok()?;
                if elapsed < MIN_COMMAND_DURATION {
                    return None;
                }
                return Some(Completion {
                    command,
                    exit_code,
                    elapsed_ms: elapsed.as_millis() as u64,
                    ended_at: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64,
                });
            }
            _ => (),
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_osc_133_boundaries_report_exit_codes_at_ten_seconds_only() {
        use crate::terminal_escape::{EscapeEvent, EscapeStream};
        let mut parser = EscapeStream::default();
        let mut commands = Boundaries::default();
        let now = Instant::now();
        let mut feed = |data: &[u8], offset| {
            data.iter().find_map(|&byte| {
                let EscapeEvent::Osc(data) = parser.event(byte) else { return None; };
                let text = std::str::from_utf8(&data).ok()?.strip_prefix("133;")?;
                commands.observe(text, now + Duration::from_millis(offset))
            })
        };
        assert!(feed(b"\x1b]133;A\x07\x1b]13", 0).is_none());
        assert!(feed(b"3;C;sleep 12; false\x1b", 0).is_none());
        assert!(feed(b"\\\r\n", 0).is_none());
        let failed = feed(b"\x1b]133;D;1\x07", 12_000).unwrap();
        assert_eq!(failed.command, "sleep 12; false");
        assert_eq!(failed.exit_code, 1);
        assert!(failed.divider().contains("用时 12s · 退出码 1"));
        assert!(feed(b"\x1b]133;C;pwd\x07", 20_000).is_none());
        assert!(feed(b"\x1b]133;D;0\x07", 29_999).is_none());
        assert!(feed(b"\x1b]133;D;0\x07", 30_000).is_none());
        assert!(feed(b"\x1b]133;C;sleep 10\x07", 40_000).is_none());
        assert_eq!(
            feed(b"\x1b]133;D;0\x07", 50_000).unwrap().elapsed_ms,
            10_000
        );
    }
}
