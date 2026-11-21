use super::{identify, AgentKind};

const RECHECK_MS: u64 = 1_000;
const DISCOVERY_MS: u64 = 125;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct ForegroundAgent {
    pub(super) kind: AgentKind,
    group: i32,
    pid: i32,
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
        self.agent
    }
}

fn inspect_group(group: i32) -> Option<ForegroundAgent> {
    let inspect = |pid| process_kind(pid).map(|kind| ForegroundAgent { kind, group, pid });
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

fn process_kind(pid: i32) -> Option<AgentKind> {
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
            return identify(
                executable,
                fields
                    .skip_while(|arg| arg.is_empty())
                    .take(argc.max(0) as usize),
            );
        }
    }
    let mut name = [0; 64];
    let read = unsafe { libc::proc_name(pid, name.as_mut_ptr().cast(), name.len() as u32) };
    (read > 0)
        .then(|| name.split(|&byte| byte == 0).next().unwrap_or_default())
        .and_then(|name| identify(name, std::iter::empty()))
}
