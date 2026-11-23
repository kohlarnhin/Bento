use super::{identify, resume_arg, AgentProcess};
use crate::processes::{file_descriptors, identity};
use std::ffi::OsString;
use std::mem::MaybeUninit;
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;

const RECHECK_MS: u64 = 1_000;
const DISCOVERY_MS: u64 = 125;

#[derive(Clone, PartialEq, Eq)]
pub(super) struct ForegroundAgent {
    group: i32,
    pub(super) process: AgentProcess,
}

#[derive(Default)]
pub(super) struct Detector {
    group: Option<i32>,
    checked_at: u64,
    agent: Option<ForegroundAgent>,
}

impl Detector {
    pub(super) fn sample(&mut self, group: Option<i32>, now: u64) -> Option<ForegroundAgent> {
        let interval = if self.agent.is_some() {
            RECHECK_MS
        } else {
            DISCOVERY_MS
        };
        if self.group != group
            || (group.is_some() && now.saturating_sub(self.checked_at) >= interval)
        {
            self.group = group;
            self.checked_at = now;
            self.agent = group.and_then(inspect_group);
        }
        self.agent.clone()
    }
}

fn inspect_group(group: i32) -> Option<ForegroundAgent> {
    let inspect = |pid| process_agent(pid).map(|process| ForegroundAgent { group, process });
    // npm launchers and shell wrappers can have the actual CLI as a child in the same job.
    inspect(group).or_else(|| {
        group_members(group)
            .into_iter()
            .filter(|&pid| pid != group)
            .find_map(inspect)
    })
}

fn group_members(group: i32) -> Vec<i32> {
    // PROC_PGRP_ONLY from Darwin's <sys/proc_info.h>.
    const GROUP_ONLY: u32 = 2;
    let required =
        unsafe { libc::proc_listpids(GROUP_ONLY, group as u32, std::ptr::null_mut(), 0) };
    if required <= 0 {
        return Vec::new();
    }
    let mut pids = vec![0; required as usize / size_of::<i32>() + 8];
    let read = unsafe {
        libc::proc_listpids(
            GROUP_ONLY,
            group as u32,
            pids.as_mut_ptr().cast(),
            (pids.len() * size_of::<i32>()) as i32,
        )
    };
    pids.truncate(read.max(0) as usize / size_of::<i32>());
    pids.retain(|&pid| pid > 0);
    pids
}

fn process_agent(pid: i32) -> Option<AgentProcess> {
    let started_at = identity(pid as u32)?.started_at_millis();
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    let mut size = 0;
    let mut read = |buffer, size: &mut usize| unsafe {
        libc::sysctl(mib.as_mut_ptr(), 3, buffer, size, std::ptr::null_mut(), 0) == 0
    };
    if read(std::ptr::null_mut(), &mut size) && size >= size_of::<i32>() {
        let mut data = vec![0; size];
        if read(data.as_mut_ptr().cast(), &mut size)
            && (size_of::<i32>()..=data.len()).contains(&size)
        {
            let argc = i32::from_ne_bytes(data[..size_of::<i32>()].try_into().unwrap());
            let mut fields = data[size_of::<i32>()..size].split(|&byte| byte == 0);
            let executable = fields.next().unwrap_or_default();
            // KERN_PROCARGS2 puts alignment NULs between the executable path and argv.
            // Stop at argc, so environment entries can never identify a process.
            let (kind, args) = identify(
                executable,
                fields
                    .skip_while(|arg| arg.is_empty())
                    .take(argc.max(0) as usize),
            )?;
            return Some(AgentProcess {
                kind,
                pid: pid as u32,
                started_at,
                resume: resume_arg(kind, args),
            });
        }
    }
    let mut name = [0; 64];
    let read = unsafe { libc::proc_name(pid, name.as_mut_ptr().cast(), name.len() as u32) };
    let name = (read > 0).then(|| name.split(|&byte| byte == 0).next().unwrap_or_default())?;
    let (kind, _) = identify(name, std::iter::empty())?;
    Some(AgentProcess {
        kind,
        pid: pid as u32,
        started_at,
        resume: None,
    })
}

#[repr(C)]
struct VnodeFile {
    open_flags: u32,
    // Remainder of proc_fileinfo in Darwin's <sys/proc_info.h>.
    _status: u32,
    _offset: i64,
    _type: i32,
    _guard_flags: u32,
    vnode: libc::vnode_info_path,
}

pub(crate) fn agent_log_paths(process: &AgentProcess) -> Vec<PathBuf> {
    const PROC_PIDFDVNODEPATHINFO: i32 = 2;
    const FWRITE: u32 = 2;
    let pid = process.pid as i32;
    let same_process = || {
        identity(process.pid)
            .is_some_and(|current| current.started_at_millis() == process.started_at)
    };
    if !same_process() {
        return Vec::new();
    }
    let mut paths = Vec::new();
    for fd in file_descriptors(pid).unwrap_or_default() {
        if fd.proc_fdtype != libc::PROX_FDTYPE_VNODE as u32 {
            continue;
        }
        let mut info = MaybeUninit::<VnodeFile>::uninit();
        let size = size_of::<VnodeFile>() as i32;
        let read = unsafe {
            libc::proc_pidfdinfo(
                pid,
                fd.proc_fd,
                PROC_PIDFDVNODEPATHINFO,
                info.as_mut_ptr().cast(),
                size,
            )
        };
        if read != size {
            continue;
        }
        let info = unsafe { info.assume_init() };
        if info.open_flags & FWRITE == 0 {
            continue;
        }
        let bytes = info
            .vnode
            .vip_path
            .iter()
            .flatten()
            .take_while(|&&byte| byte != 0)
            .map(|&byte| byte as u8)
            .collect();
        let path = PathBuf::from(OsString::from_vec(bytes));
        if path
            .extension()
            .is_some_and(|extension| extension == "jsonl")
        {
            paths.push(path);
        }
    }
    if same_process() {
        paths
    } else {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal_agent::AgentKind;
    use std::process::Command;
    use std::time::{Duration, Instant};

    #[test]
    fn macos_reports_only_the_log_opened_for_writing_by_this_process() {
        let directory =
            std::env::temp_dir().join(format!("bento-agent-fds-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let writer = directory.join("writer.jsonl");
        let reader = directory.join("other.jsonl");
        std::fs::write(&reader, []).unwrap();
        let mut child = Command::new("/bin/sh")
            .args([
                "-c",
                "exec 3>>\"$1\" 4<\"$2\"; exec /bin/sleep 30",
                "fixture",
            ])
            .arg(&writer)
            .arg(&reader)
            .spawn()
            .unwrap();
        let process = AgentProcess {
            kind: AgentKind::Codex,
            pid: child.id(),
            started_at: identity(child.id()).unwrap().started_at_millis(),
            resume: None,
        };
        let deadline = Instant::now() + Duration::from_secs(2);
        let paths = loop {
            let paths = agent_log_paths(&process);
            if !paths.is_empty() || Instant::now() >= deadline {
                break paths;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        let expected = writer.canonicalize();
        let _ = child.kill();
        let _ = child.wait();
        std::fs::remove_dir_all(directory).unwrap();
        assert_eq!(paths, [expected.unwrap()]);
        assert!(agent_log_paths(&process).is_empty());
    }
}
