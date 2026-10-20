use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn lsof() -> Command {
    #[cfg(debug_assertions)]
    crate::debug_e2e::record_lsof();
    Command::new("lsof")
}

pub fn parse_listening_ports(text: &str) -> HashMap<u16, Vec<u32>> {
    let mut ports: HashMap<u16, Vec<u32>> = HashMap::new();
    let mut pid = None;
    for line in text.lines() {
        if let Some(value) = line.strip_prefix('p') {
            pid = value.parse::<u32>().ok().filter(|pid| *pid > 1);
        } else if let Some(address) = line.strip_prefix('n') {
            if address.contains("->") {
                continue;
            }
            if let (Some(pid), Some(port)) = (
                pid,
                address
                    .rsplit_once(':')
                    .and_then(|(_, port)| port.parse::<u16>().ok())
                    .filter(|port| *port > 0),
            ) {
                let owners = ports.entry(port).or_default();
                if !owners.contains(&pid) {
                    owners.push(pid);
                }
            }
        }
    }
    ports
}

pub fn listening_ports() -> HashMap<u16, Vec<u32>> {
    lsof()
        .args(["-nP", "-iTCP", "-sTCP:LISTEN", "-Fpn"])
        .output()
        .ok()
        .map(|out| parse_listening_ports(&String::from_utf8_lossy(&out.stdout)))
        .unwrap_or_default()
}

pub fn probe_loopback(port: u16) -> bool {
    use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream};
    [
        SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
        SocketAddr::from((Ipv6Addr::LOCALHOST, port)),
    ]
    .iter()
    .any(|address| TcpStream::connect_timeout(address, Duration::from_millis(100)).is_ok())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProcessIdentity {
    seconds: u64,
    micros: u64,
}

// A birth timestamp prevents a recycled PID from inheriting permission to be killed.
pub fn identity(pid: u32) -> Option<ProcessIdentity> {
    if pid <= 1 || pid == std::process::id() {
        return None;
    }
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::uninit();
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
    let read = unsafe {
        libc::proc_pidinfo(
            pid as i32,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        )
    };
    if read != size {
        return None;
    }
    let info = unsafe { info.assume_init() };
    if info.pbi_status == libc::SZOMB {
        return None;
    }
    Some(ProcessIdentity {
        seconds: info.pbi_start_tvsec,
        micros: info.pbi_start_tvusec,
    })
}

#[derive(Clone, Copy, Debug)]
pub struct ProcessRow {
    pub pid: u32,
    pub ppid: u32,
    pub pgid: i32,
}

pub fn parse_process_table(text: &str) -> Vec<ProcessRow> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            Some(ProcessRow {
                pid: fields.next()?.parse().ok()?,
                ppid: fields.next()?.parse().ok()?,
                pgid: fields.next()?.parse().ok()?,
            })
        })
        .collect()
}

fn process_table() -> Vec<ProcessRow> {
    Command::new("ps")
        .args(["-ax", "-o", "pid=,ppid=,pgid="])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| parse_process_table(&String::from_utf8_lossy(&out.stdout)))
        .unwrap_or_default()
}

pub fn descendants(rows: &[ProcessRow], root: u32) -> HashSet<u32> {
    let mut found = HashSet::from([root]);
    loop {
        let count = found.len();
        for row in rows {
            if found.contains(&row.ppid) && row.pid > 1 {
                found.insert(row.pid);
            }
        }
        if found.len() == count {
            break;
        }
    }
    found.remove(&root);
    found
}

pub struct ManagedRoot {
    pub pid: u32,
    pub pgid: i32,
    pub identity: Option<ProcessIdentity>,
}

pub struct ProcessTree {
    pgid: i32,
    members: HashMap<u32, ProcessIdentity>,
}

impl ProcessTree {
    fn capture(root: &ManagedRoot, rows: &[ProcessRow]) -> Self {
        let mut members = HashMap::new();
        let own_group = unsafe { libc::getpgrp() };
        if root.pgid <= 1
            || root.pgid == own_group
            || root.identity.is_none()
            || identity(root.pid) != root.identity
        {
            return Self {
                pgid: root.pgid,
                members,
            };
        }
        let descendants = descendants(rows, root.pid);
        for row in rows {
            if row.pid == root.pid || descendants.contains(&row.pid) || row.pgid == root.pgid {
                if let Some(birth) = identity(row.pid) {
                    members.insert(row.pid, birth);
                }
            }
        }
        Self {
            pgid: root.pgid,
            members,
        }
    }

    fn alive(&self) -> bool {
        self.members
            .iter()
            .any(|(&pid, &birth)| identity(pid) == Some(birth))
    }

    fn signal(&self, signal: i32) {
        let mut group_alive = false;
        for (&pid, &birth) in &self.members {
            if identity(pid) != Some(birth) {
                continue;
            }
            let group = unsafe { libc::getpgid(pid as i32) };
            if group == self.pgid {
                group_alive = true;
            } else if pid > 1 && pid != std::process::id() {
                // An escaped descendant may now be reparented; its birth identity still matches.
                if identity(pid) == Some(birth) {
                    unsafe {
                        libc::kill(pid as i32, signal);
                    }
                }
            }
        }
        if group_alive && self.pgid > 1 && self.pgid != unsafe { libc::getpgrp() } {
            unsafe {
                libc::kill(-self.pgid, signal);
            }
        }
    }
}

pub fn capture_trees(roots: &[ManagedRoot]) -> Vec<Arc<ProcessTree>> {
    // Capture every tree BEFORE any parent is signalled and children can be reparented.
    let rows = process_table();
    roots
        .iter()
        .map(|root| Arc::new(ProcessTree::capture(root, &rows)))
        .collect()
}

pub fn terminate_trees_blocking(trees: &[Arc<ProcessTree>]) {
    for tree in trees {
        tree.signal(libc::SIGTERM);
    }
    let deadline = Instant::now() + Duration::from_millis(1500);
    while Instant::now() < deadline && trees.iter().any(|tree| tree.alive()) {
        std::thread::sleep(Duration::from_millis(40));
    }
    for tree in trees {
        if tree.alive() {
            tree.signal(libc::SIGKILL);
        }
    }
}

pub fn port_pids(port: u16) -> Vec<u32> {
    let mut pids: Vec<_> = lsof()
        .args(["-nP", &format!("-iTCP:{port}"), "-sTCP:LISTEN", "-t"])
        .output()
        .ok()
        .map(|out| {
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .filter_map(|s| s.trim().parse::<u32>().ok())
                .filter(|pid| *pid > 1)
                .collect()
        })
        .unwrap_or_default();
    pids.sort_unstable();
    pids.dedup();
    pids
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortOwner {
    pub pid: u32,
    command: String,
    cwd: Option<String>,
    in_project: bool,
}

pub fn port_owners(port: u16, project_path: &str) -> Vec<PortOwner> {
    let project = std::path::PathBuf::from(crate::expand_tilde(project_path))
        .canonicalize()
        .ok();
    port_pids(port)
        .into_iter()
        .map(|pid| {
            let command = Command::new("ps")
                .args(["-o", "command=", "-p", &pid.to_string()])
                .output()
                .ok()
                .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
                .unwrap_or_default();
            let cwd = lsof()
                .args(["-a", "-p", &pid.to_string(), "-d", "cwd", "-Fn"])
                .output()
                .ok()
                .and_then(|out| {
                    String::from_utf8_lossy(&out.stdout)
                        .lines()
                        .find_map(|line| line.strip_prefix('n').map(String::from))
                });
            let in_project = cwd
                .as_ref()
                .and_then(|path| std::path::Path::new(path).canonicalize().ok())
                .zip(project.as_ref())
                .is_some_and(|(cwd, project)| cwd.starts_with(project));
            PortOwner {
                pid,
                command,
                cwd: cwd.map(|path| collapse_home(&path)),
                in_project,
            }
        })
        .collect()
}

fn collapse_home(path: &str) -> String {
    if let Some(home) = dirs::home_dir() {
        if let Ok(rest) = std::path::Path::new(path).strip_prefix(home) {
            return if rest.as_os_str().is_empty() {
                "~".into()
            } else {
                format!("~/{}", rest.display())
            };
        }
    }
    path.into()
}

pub fn stop_external(port: u16, pids: Vec<u32>) -> Vec<String> {
    let mut messages = Vec::new();
    let mut unique = HashSet::new();
    for pid in pids {
        if !unique.insert(pid) {
            continue;
        }
        let birth = identity(pid);
        if birth.is_none() || !port_pids(port).contains(&pid) || identity(pid) != birth {
            messages.push(format!(
                "[Bento] 已跳过 PID {pid}：它已不再监听端口 {port}"
            ));
            continue;
        }
        unsafe {
            libc::kill(pid as i32, libc::SIGTERM);
        }
        messages.push(format!(
            "[Bento] 已确认结束端口 {port} 的外部进程 PID {pid}"
        ));
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline && identity(pid) == birth {
            std::thread::sleep(Duration::from_millis(50));
        }
        // Confirm both identity and current LISTEN ownership again before escalation.
        if identity(pid) == birth && port_pids(port).contains(&pid) && identity(pid) == birth {
            unsafe {
                libc::kill(pid as i32, libc::SIGKILL);
            }
        }
    }
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline && !port_pids(port).is_empty() {
        std::thread::sleep(Duration::from_millis(100));
    }
    let remaining = port_pids(port);
    if !remaining.is_empty() {
        messages.push(format!(
            "[Bento] 端口 {port} 仍被外部进程 PID {remaining:?} 占用"
        ));
    }
    messages
}

pub fn quote_shell(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub fn launch_script(
    path: &str,
    command: &str,
    env: &[crate::projects::EnvVar],
) -> Result<String, String> {
    crate::projects::validate_env(env)?;
    let script = format!("cd -- {} && {command}", quote_shell(path));
    if env.is_empty() {
        return Ok(script);
    }
    let assignments = env
        .iter()
        .map(|entry| format!("{}={}", entry.key, quote_shell(&entry.value)))
        .collect::<Vec<_>>()
        .join(" ");
    Ok(format!("export {assignments}; {script}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_listeners_across_addresses_and_processes() {
        let parsed = parse_listening_ports("n*:9999\np21\nf10\nn*:5173\nn127.0.0.1:3000\nn[::1]:1420\nn*:5173\np22\nn[::]:5173\nn0.0.0.0:5300\np23\nn127.0.0.1:5300\n");
        assert_eq!(
            parsed,
            HashMap::from([
                (5173, vec![21, 22]),
                (3000, vec![21]),
                (1420, vec![21]),
                (5300, vec![22, 23]),
            ])
        );
    }

    #[test]
    fn ignores_malformed_and_established_socket_records() {
        let parsed = parse_listening_ports("garbage\np0\nn*:5173\npbad\nn*:3000\np50\nn127.0.0.1:5300->127.0.0.1:6000\nnmissing\nn*:65536\nn*:0\nn*:wat\np\nn*:1420\np51\nn[::1]:5301\n");
        assert_eq!(parsed, HashMap::from([(5301, vec![51])]));
    }

    #[test]
    fn quote_paths_without_interpolation() {
        for path in [
            "/tmp/中文 空格",
            "/tmp/a'b",
            "/tmp/a\"b",
            "/tmp/$HOME/`whoami`",
        ] {
            let output = Command::new("zsh")
                .args(["-c", &format!("printf '%s' {}", quote_shell(path))])
                .output()
                .unwrap();
            assert!(output.status.success());
            assert_eq!(String::from_utf8(output.stdout).unwrap(), path);
        }
        assert_eq!(quote_shell("a'b"), "'a'\\''b'");
    }

    #[test]
    fn launch_exports_round_trip_after_login_profile() {
        use crate::projects::EnvVar;
        let dir =
            std::env::temp_dir().join(format!("apphub-env-' \"$ 中文-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(".zprofile"),
            "export APPHUB_TEXT='profile override'\n",
        )
        .unwrap();
        let value = " ' \" $HOME `whoami` 中文\n second line ";
        let script = launch_script(
            dir.to_str().unwrap(),
            "printf '%s\\0%s' \"$APPHUB_TEXT\" \"$APPHUB_EMPTY\"",
            &[
                EnvVar {
                    key: "APPHUB_TEXT".into(),
                    value: value.into(),
                },
                EnvVar {
                    key: "APPHUB_EMPTY".into(),
                    value: "".into(),
                },
            ],
        )
        .unwrap();
        let output = Command::new("zsh")
            .args(["-lc", &script])
            .env("ZDOTDIR", &dir)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, [value.as_bytes(), &[0]].concat());
        assert_eq!(
            launch_script("/tmp", "echo original", &[]).unwrap(),
            "cd -- '/tmp' && echo original"
        );
        assert!(launch_script(
            "/tmp",
            "echo",
            &[EnvVar {
                key: "BAD;echo".into(),
                value: "x".into()
            }]
        )
        .is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn collects_descendants_across_groups_and_sessions() {
        let rows =
            parse_process_table("1 0 1\n10 1 10\n11 10 10\n12 11 12\n13 12 13\n20 1 20\ninvalid\n");
        assert_eq!(descendants(&rows, 10), HashSet::from([11, 12, 13]));
        assert!(descendants(&rows, 20).is_empty());
    }
}
