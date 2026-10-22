use std::collections::HashMap;
use std::ffi::{CStr, CString, OsString};
use std::io::{ErrorKind, Read};
use std::os::fd::AsRawFd;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static NEXT_CAPTURE: AtomicU64 = AtomicU64::new(1);

pub fn should_strip(key: &str) -> bool {
    matches!(
        key,
        "CLAUDECODE" | "CLAUDE_PID" | "CLAUDE_EFFORT" | "CODEX_THREAD_ID" | "CODEX_CI" | "INIT_CWD"
    ) || key.starts_with("CLAUDE_CODE_")
        || key.starts_with("CODEX_SANDBOX")
        || key.starts_with("npm_")
        || key.starts_with("PNPM_")
}

fn executable_shell(path: &Path) -> bool {
    path.is_absolute()
        && path.is_file()
        && CString::new(path.as_os_str().as_bytes())
            .is_ok_and(|path| unsafe { libc::access(path.as_ptr(), libc::X_OK) == 0 })
}

fn login_shell() -> PathBuf {
    if let Some(shell) = std::env::var_os("SHELL").map(PathBuf::from) {
        if executable_shell(&shell) {
            return shell;
        }
    }
    // getpwuid_r avoids sharing libc's passwd buffer with terminal spawning threads.
    let mut buffer = vec![0_u8; 16384];
    loop {
        let mut entry = std::mem::MaybeUninit::<libc::passwd>::uninit();
        let mut result = std::ptr::null_mut();
        let error = unsafe {
            libc::getpwuid_r(
                libc::getuid(),
                entry.as_mut_ptr(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut result,
            )
        };
        if error == libc::ERANGE {
            buffer.resize(buffer.len() * 2, 0);
            continue;
        }
        if error == 0 && !result.is_null() {
            let entry = unsafe { entry.assume_init() };
            if !entry.pw_shell.is_null() {
                let shell = PathBuf::from(OsString::from_vec(
                    unsafe { CStr::from_ptr(entry.pw_shell) }
                        .to_bytes()
                        .to_vec(),
                ));
                if executable_shell(&shell) {
                    return shell;
                }
            }
        }
        return PathBuf::from("/bin/zsh");
    }
}

fn parse(output: &[u8], start: &[u8], end: &[u8]) -> Result<HashMap<String, String>, String> {
    let start = output
        .windows(start.len())
        .position(|bytes| bytes == start)
        .map(|offset| offset + start.len())
        .ok_or("未找到环境起始标记")?;
    let len = output[start..]
        .windows(end.len())
        .position(|bytes| bytes == end)
        .ok_or("未找到环境结束标记")?;
    let mut env = HashMap::new();
    for entry in output[start..start + len].split(|byte| *byte == 0) {
        let Ok(entry) = std::str::from_utf8(entry) else {
            continue;
        };
        let Some((key, value)) = entry.split_once('=') else {
            continue;
        };
        let mut chars = key.chars();
        if !chars
            .next()
            .is_some_and(|ch| ch.is_ascii_alphabetic() || ch == '_')
            || !chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
            || matches!(key, "PWD" | "OLDPWD" | "SHLVL" | "_" | "TERM")
            || should_strip(key)
        {
            continue;
        }
        env.insert(key.to_string(), value.to_string());
    }
    Ok(env)
}

pub fn capture() -> Result<HashMap<String, String>, String> {
    capture_with(&login_shell(), &[], Duration::from_secs(5))
}

pub fn capture_with(
    shell: &Path,
    extra_env: &[(OsString, OsString)],
    timeout: Duration,
) -> Result<HashMap<String, String>, String> {
    let suffix = format!(
        "{}_{}",
        std::process::id(),
        NEXT_CAPTURE.fetch_add(1, Ordering::Relaxed)
    );
    let start = format!("__BENTO_ENV_START_{suffix}__");
    let end = format!("__BENTO_ENV_END_{suffix}__");
    let script = format!("printf '%s' '{start}'; /usr/bin/env -0; printf '%s' '{end}'");
    let mut env: HashMap<_, _> = std::env::vars_os().collect();
    env.extend(extra_env.iter().cloned());
    env.retain(|key, _| !key.to_str().is_some_and(should_strip));
    env.insert("TERM".into(), "dumb".into());
    let mut command = Command::new(shell);
    command
        .args(["-l", "-i", "-c", &script])
        .env_clear()
        .envs(&env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0);
    if let Some(home) = env.get(std::ffi::OsStr::new("HOME")) {
        command.current_dir(home);
    }
    let mut child = command
        .spawn()
        .map_err(|err| format!("启动 shell 失败：{err}"))?;
    let result = read_capture(&mut child, end.as_bytes(), timeout)
        .and_then(|output| parse(&output, start.as_bytes(), end.as_bytes()));
    // This group belongs only to the capture, including helpers started by rc files.
    unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
    let waited = child
        .wait()
        .map_err(|err| format!("回收 shell 失败：{err}"));
    waited?;
    result
}

fn read_capture(child: &mut Child, end: &[u8], timeout: Duration) -> Result<Vec<u8>, String> {
    let started = Instant::now();
    let mut stdout = child.stdout.take().ok_or("无法读取 shell 输出")?;
    // Drain while waiting: rc-file output can fill a pipe, and background helpers can hold it open.
    let fd = stdout.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(format!(
            "读取 shell 输出失败：{}",
            std::io::Error::last_os_error()
        ));
    }
    let mut output = Vec::new();
    let mut buffer = [0; 8192];
    let mut eof = false;
    loop {
        if started.elapsed() >= timeout {
            return Err(format!("读取超时（{:.1} 秒）", timeout.as_secs_f64()));
        }
        if !eof {
            match stdout.read(&mut buffer) {
                Ok(0) => eof = true,
                Ok(len) => {
                    output.extend_from_slice(&buffer[..len]);
                    continue;
                }
                Err(err) if err.kind() == ErrorKind::Interrupted => continue,
                Err(err) if err.kind() == ErrorKind::WouldBlock => {}
                Err(err) => return Err(format!("读取 shell 输出失败：{err}")),
            }
        }
        if let Some(status) = child
            .try_wait()
            .map_err(|err| format!("等待 shell 失败：{err}"))?
        {
            if !status.success() {
                return Err(format!("shell {status}"));
            }
            // A disowned rc helper may retain stdout after the shell has finished.
            // Reads above drain the pipe before accepting the complete capture.
            if eof || output.windows(end.len()).any(|bytes| bytes == end) {
                return Ok(output);
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        dir: PathBuf,
    }
    impl Fixture {
        fn new(rc: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "bento-shell-env-{}-{}",
                std::process::id(),
                NEXT_CAPTURE.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(".zshrc"), rc).unwrap();
            Self { dir }
        }
        fn env(&self) -> Vec<(OsString, OsString)> {
            vec![("ZDOTDIR".into(), self.dir.as_os_str().into())]
        }
        fn capture(&self, timeout: Duration) -> Result<HashMap<String, String>, String> {
            capture_with(Path::new("/bin/zsh"), &self.env(), timeout)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn parses_only_marked_nul_entries_and_skips_invalid_or_transient_values() {
        let output = b"noise=before\n__START__PATH=/first:/second\0VALUE=a=b\nnext\0EMPTY=\0=invalid\0BAD-KEY=x\0NO_EQUALS\0BAD_UTF8=\xff\0PWD=/x\0OLDPWD=/y\0SHLVL=2\0_=env\0TERM=dumb\0CLAUDECODE=1\0CLAUDE_PID=2\0CLAUDE_EFFORT=high\0CLAUDE_CODE_X=x\0CODEX_THREAD_ID=x\0CODEX_CI=1\0CODEX_SANDBOX_X=1\0npm_config_x=y\0PNPM_HOME=/x\0INIT_CWD=/x\0__END__noise=after\n";
        let env = parse(output, b"__START__", b"__END__").unwrap();
        assert_eq!(env.len(), 3);
        assert_eq!(env["PATH"], "/first:/second");
        assert_eq!(env["VALUE"], "a=b\nnext");
        assert_eq!(env["EMPTY"], "");
        assert!(parse(b"missing", b"__START__", b"__END__").is_err());
        assert!(parse(b"__START__incomplete", b"__START__", b"__END__").is_err());
    }

    #[test]
    fn interactive_rc_environment_is_fresh_and_cleans_inherited_markers() {
        let fixture = Fixture::new(
            "echo rc-noise\nexport BENTO_SAW_INHERITED=${CLAUDECODE-unset}\nexport BENTO_FIXTURE_VAR='from zshrc'\nexport PATH=\"$ZDOTDIR/bin:/usr/bin:/bin\"\nexport CLAUDECODE=reintroduced\ntrap 'echo exit-noise' EXIT\n",
        );
        std::fs::write(fixture.dir.join(".zprofile"), "echo profile-noise\n").unwrap();
        let mut extra = fixture.env();
        for key in [
            "CLAUDECODE",
            "CLAUDE_CODE_X",
            "CODEX_THREAD_ID",
            "CODEX_CI",
            "CODEX_SANDBOX_X",
            "npm_config_x",
            "PNPM_HOME",
            "INIT_CWD",
        ] {
            extra.push((key.into(), "inherited".into()));
        }
        let env = capture_with(Path::new("/bin/zsh"), &extra, Duration::from_secs(5)).unwrap();
        assert_eq!(env["BENTO_FIXTURE_VAR"], "from zshrc");
        assert_eq!(env["BENTO_SAW_INHERITED"], "unset");
        assert_eq!(
            env["PATH"],
            format!("{}/bin:/usr/bin:/bin", fixture.dir.display())
        );
        assert!(!env.keys().any(|key| should_strip(key)));
        std::fs::write(
            fixture.dir.join(".zshrc"),
            "export BENTO_FIXTURE_VAR=edited\n",
        )
        .unwrap();
        assert_eq!(
            fixture.capture(Duration::from_secs(5)).unwrap()["BENTO_FIXTURE_VAR"],
            "edited"
        );
    }

    #[test]
    fn blocked_rc_times_out_and_kills_the_capture_group() {
        let fixture = Fixture::new("echo $$ > \"$ZDOTDIR/capture-pgid\"\n/bin/sleep 30\n");
        let started = Instant::now();
        let error = fixture.capture(Duration::from_millis(300)).unwrap_err();
        assert!(error.contains("超时"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(2));
        let pgid: i32 = std::fs::read_to_string(fixture.dir.join("capture-pgid"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert_ne!(pgid, unsafe { libc::getpgrp() });
        let deadline = Instant::now() + Duration::from_secs(2);
        while unsafe { libc::kill(-pgid, 0) } == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            unsafe { libc::kill(-pgid, 0) },
            -1,
            "capture group {pgid} survived"
        );
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
    }

    #[test]
    fn rc_read_has_null_stdin_and_cannot_hang() {
        let fixture =
            Fixture::new("read -q 'answer?fixture prompt'\nexport BENTO_AFTER_READ=ready\n");
        let started = Instant::now();
        let env = fixture.capture(Duration::from_secs(1)).unwrap();
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(env["BENTO_AFTER_READ"], "ready");
    }

    fn capture_with_background_job(job: &str) {
        let fixture = Fixture::new(&format!(
            "echo $$ > \"$ZDOTDIR/capture-pgid\"\n{job}\necho $! > \"$ZDOTDIR/background-pid\"\n/bin/ps -o pgid= -p $! > \"$ZDOTDIR/background-pgid\"\nexport BENTO_FIXTURE_VAR=background-ready\n"
        ));
        let started = Instant::now();
        let env = fixture.capture(Duration::from_secs(2)).unwrap();
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(env["BENTO_FIXTURE_VAR"], "background-ready");
        let read_pid = |name| -> i32 {
            std::fs::read_to_string(fixture.dir.join(name))
                .unwrap()
                .trim()
                .parse()
                .unwrap()
        };
        let pgid = read_pid("capture-pgid");
        let pid = read_pid("background-pid");
        assert_eq!(read_pid("background-pgid"), pgid);
        let deadline = Instant::now() + Duration::from_secs(2);
        while unsafe { libc::kill(pid, 0) } == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            unsafe { libc::kill(pid, 0) },
            -1,
            "background job {pid} survived"
        );
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
        assert_eq!(
            unsafe { libc::kill(-pgid, 0) },
            -1,
            "capture group {pgid} survived"
        );
    }

    #[test]
    fn disowned_rc_job_does_not_hold_capture_open() {
        capture_with_background_job("/bin/sleep 30 &!");
    }

    #[test]
    fn nohup_rc_job_does_not_hold_capture_open() {
        capture_with_background_job("/usr/bin/nohup /bin/sleep 30 >&1 &");
    }
}
