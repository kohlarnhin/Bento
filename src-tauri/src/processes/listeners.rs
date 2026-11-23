//! Minimal, SDK-checked libproc socket records. Opaque fields are never read.
use std::collections::HashMap;
use std::mem::{align_of, offset_of, size_of, MaybeUninit};

const PROC_PIDFDSOCKETINFO: i32 = 3;
const SOCKINFO_TCP: i32 = 2;
const TSI_S_LISTEN: i32 = 1;

#[repr(C)]
struct InSockInfo {
    _foreign_port: i32,
    local_port: i32,
    _generation_flags_flow: [u64; 2],
    version: u8,
    _addresses_and_options: [u8; 55],
}

#[repr(C)]
struct TcpSockInfo {
    internet: InSockInfo,
    state: i32,
    _timers_and_options: [u8; 36],
}

#[repr(C)]
struct SocketInfo {
    _stat_and_socket: [u64; 20],
    family: i32,
    _options_and_buffers: [u8; 68],
    kind: i32,
    _reserved: u32,
    tcp: TcpSockInfo,
    _protocol_union_tail: [u8; 408],
}

#[repr(C)]
struct SocketFdInfo {
    _file_info: [u64; 3],
    socket: SocketInfo,
}

// Verified with clang sizeof/offsetof against <sys/proc_info.h> (macOS SDK).
// Keep the complete record size: proc_pidfdinfo requires room for the union,
// including its much larger non-TCP variants.
const _: () = {
    assert!(size_of::<libc::proc_fdinfo>() == 8);
    assert!(align_of::<libc::proc_fdinfo>() == 4);
    assert!(offset_of!(libc::proc_fdinfo, proc_fd) == 0);
    assert!(offset_of!(libc::proc_fdinfo, proc_fdtype) == 4);
    assert!(size_of::<InSockInfo>() == 80);
    assert!(align_of::<InSockInfo>() == 8);
    assert!(offset_of!(InSockInfo, local_port) == 4);
    assert!(offset_of!(InSockInfo, version) == 24);
    assert!(size_of::<TcpSockInfo>() == 120);
    assert!(align_of::<TcpSockInfo>() == 8);
    assert!(offset_of!(TcpSockInfo, internet) == 0);
    assert!(offset_of!(TcpSockInfo, state) == 80);
    assert!(size_of::<SocketInfo>() == 768);
    assert!(align_of::<SocketInfo>() == 8);
    assert!(offset_of!(SocketInfo, family) == 160);
    assert!(offset_of!(SocketInfo, kind) == 232);
    assert!(offset_of!(SocketInfo, tcp) == 240);
    assert!(size_of::<SocketFdInfo>() == 792);
    assert!(align_of::<SocketFdInfo>() == 8);
    assert!(offset_of!(SocketFdInfo, socket) == 24);
};

fn buffer<T: Clone>(count: usize, initial: T) -> Result<Vec<T>, ()> {
    if count > i32::MAX as usize / size_of::<T>() {
        return Err(());
    }
    let mut buffer = Vec::new();
    buffer.try_reserve_exact(count).map_err(|_| ())?;
    buffer.resize(count, initial);
    Ok(buffer)
}

fn pids() -> Result<Vec<i32>, ()> {
    let count = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    if count <= 0 {
        return Err(());
    }
    let mut capacity = count as usize + 128;
    for _ in 0..4 {
        let mut buffer = buffer(capacity, 0_i32)?;
        let read = unsafe {
            libc::proc_listallpids(
                buffer.as_mut_ptr().cast(),
                (buffer.len() * size_of::<i32>()) as i32,
            )
        };
        if read <= 0 {
            return Err(());
        }
        if (read as usize) < buffer.len() {
            buffer.truncate(read as usize);
            return Ok(buffer);
        }
        capacity = capacity.checked_mul(2).ok_or(())?;
    }
    Err(())
}

pub(crate) fn fds(pid: i32) -> Option<Vec<libc::proc_fdinfo>> {
    let bytes =
        unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDLISTFDS, 0, std::ptr::null_mut(), 0) };
    if bytes <= 0 {
        return None;
    }
    let mut capacity = bytes as usize / size_of::<libc::proc_fdinfo>() + 32;
    for _ in 0..4 {
        let mut buffer = buffer(
            capacity,
            libc::proc_fdinfo {
                proc_fd: 0,
                proc_fdtype: 0,
            },
        )
        .ok()?;
        let bytes = buffer.len() * size_of::<libc::proc_fdinfo>();
        let read = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDLISTFDS,
                0,
                buffer.as_mut_ptr().cast(),
                bytes as i32,
            )
        };
        // EPERM, ESRCH, and descriptors disappearing during a scan are normal.
        if read <= 0 {
            return None;
        }
        if (read as usize) < bytes {
            buffer.truncate(read as usize / size_of::<libc::proc_fdinfo>());
            return Some(buffer);
        }
        capacity = capacity.checked_mul(2)?;
    }
    None
}

pub(super) fn listening_ports() -> Result<HashMap<u16, Vec<u32>>, ()> {
    let mut ports: HashMap<u16, Vec<u32>> = HashMap::new();
    for pid in pids()?.into_iter().filter(|pid| *pid > 1) {
        let Some(fds) = fds(pid) else { continue };
        for fd in fds {
            if fd.proc_fdtype != libc::PROX_FDTYPE_SOCKET as u32 {
                continue;
            }
            let mut info = MaybeUninit::<SocketFdInfo>::zeroed();
            let read = unsafe {
                libc::proc_pidfdinfo(
                    pid,
                    fd.proc_fd,
                    PROC_PIDFDSOCKETINFO,
                    info.as_mut_ptr().cast(),
                    size_of::<SocketFdInfo>() as i32,
                )
            };
            if read != size_of::<SocketFdInfo>() as i32 {
                continue;
            }
            let info = unsafe { info.assume_init() };
            let socket = info.socket;
            if socket.kind != SOCKINFO_TCP
                || ![libc::AF_INET, libc::AF_INET6].contains(&socket.family)
                || socket.tcp.state != TSI_S_LISTEN
            {
                continue;
            }
            let port = u16::from_be(socket.tcp.internet.local_port as u16);
            if port > 0 {
                ports.entry(port).or_default().push(pid as u32);
            }
        }
    }
    for pids in ports.values_mut() {
        pids.sort_unstable();
        pids.dedup();
    }
    Ok(ports)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read};
    use std::net::{TcpListener, TcpStream, UdpSocket};
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::process::CommandExt;
    use std::process::{Child, Command, Stdio};

    fn reuse_listener(port: u16) -> TcpListener {
        let raw = unsafe { libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0) };
        assert!(raw >= 0);
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        assert_eq!(
            unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) },
            0
        );
        let enabled: i32 = 1;
        for option in [libc::SO_REUSEADDR, libc::SO_REUSEPORT] {
            assert_eq!(
                unsafe {
                    libc::setsockopt(
                        fd.as_raw_fd(),
                        libc::SOL_SOCKET,
                        option,
                        (&enabled as *const i32).cast(),
                        size_of::<i32>() as libc::socklen_t,
                    )
                },
                0
            );
        }
        let address = libc::sockaddr_in {
            sin_len: size_of::<libc::sockaddr_in>() as u8,
            sin_family: libc::AF_INET as u8,
            sin_port: port.to_be(),
            sin_addr: libc::in_addr {
                s_addr: u32::from_ne_bytes([127, 0, 0, 1]),
            },
            sin_zero: [0; 8],
        };
        assert_eq!(
            unsafe {
                libc::bind(
                    fd.as_raw_fd(),
                    (&address as *const libc::sockaddr_in).cast(),
                    size_of::<libc::sockaddr_in>() as libc::socklen_t,
                )
            },
            0
        );
        assert_eq!(unsafe { libc::listen(fd.as_raw_fd(), 16) }, 0);
        TcpListener::from(fd)
    }

    struct ChildGuard(Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            // This handle refers only to the child created by this test.
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[test]
    fn listener_child() {
        let Ok(port) = std::env::var("APPHUB_LISTENER_TEST_PORT") else {
            return;
        };
        let shared = reuse_listener(port.parse().unwrap());
        let own = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        println!(
            "LISTENER_READY:{}:{}",
            shared.local_addr().unwrap().port(),
            own.local_addr().unwrap().port()
        );
        let _ = std::io::stdin().read(&mut [0]);
    }

    #[test]
    fn native_listeners_match_lsof() {
        // Other cargo tests launch shells concurrently. Their short-lived
        // fork-before-exec children can temporarily inherit this test's FDs,
        // making two successive system snapshots differ. Run the complete
        // scenario in its own test process, with no other tests forking in it.
        if std::env::var_os("APPHUB_LISTENER_PARITY_ISOLATED").is_none() {
            let output = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "processes::listeners::tests::native_listeners_match_lsof",
                    "--nocapture",
                ])
                .env("APPHUB_LISTENER_PARITY_ISOLATED", "1")
                .process_group(0)
                .output()
                .unwrap();
            println!("{}", String::from_utf8_lossy(&output.stdout));
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        println!("listener parity process PID/PGID: {}", std::process::id());
        let listeners: Vec<_> = ["127.0.0.1", "0.0.0.0", "::1", "::"]
            .into_iter()
            .map(|address| TcpListener::bind((address, 0)).unwrap())
            .collect();
        let shared = reuse_listener(0);
        let shared_port = shared.local_addr().unwrap().port();
        let mut child = ChildGuard(
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "processes::listeners::tests::listener_child",
                    "--nocapture",
                ])
                .env("APPHUB_LISTENER_TEST_PORT", shared_port.to_string())
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .process_group(0)
                .spawn()
                .unwrap(),
        );
        println!("listener test child PID/PGID: {}", child.0.id());
        let mut reader = BufReader::new(child.0.stdout.take().unwrap());
        let mut line = String::new();
        let child_port = loop {
            line.clear();
            assert!(
                reader.read_line(&mut line).unwrap() > 0,
                "child did not open sockets"
            );
            if let Some(ports) = line.trim().strip_prefix("LISTENER_READY:") {
                break ports.rsplit(':').next().unwrap().parse::<u16>().unwrap();
            }
        };
        let connected = TcpStream::connect(listeners[0].local_addr().unwrap()).unwrap();
        let (accepted, _) = listeners[0].accept().unwrap();
        // UDP has a separate port namespace; reserve a free TCP port first so
        // an unrelated TCP listener cannot make the UDP-only assertion false.
        let udp_port = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let udp = UdpSocket::bind(udp_port.local_addr().unwrap()).unwrap();
        drop(udp_port);
        let mut ports: Vec<_> = listeners
            .iter()
            .map(|s| s.local_addr().unwrap().port())
            .collect();
        ports.extend([
            shared_port,
            child_port,
            connected.local_addr().unwrap().port(),
            udp.local_addr().unwrap().port(),
        ]);
        let native = listening_ports().expect("libproc enumeration must succeed without fallback");
        let output = Command::new("lsof")
            .args(["-nP", "-iTCP", "-sTCP:LISTEN", "-Fpn"])
            .output()
            .unwrap();
        assert!(output.status.success());
        let lsof = super::super::parse_listening_ports(&String::from_utf8_lossy(&output.stdout));
        for port in &ports {
            assert_eq!(
                native.get(port),
                lsof.get(port),
                "libproc/lsof mismatch on {port}"
            );
        }
        for listener in &listeners {
            assert_eq!(
                native.get(&listener.local_addr().unwrap().port()),
                Some(&vec![std::process::id()])
            );
        }
        let mut shared_pids = vec![std::process::id(), child.0.id()];
        shared_pids.sort_unstable();
        assert_eq!(native.get(&shared_port), Some(&shared_pids));
        assert_eq!(native.get(&child_port), Some(&vec![child.0.id()]));
        assert!(!native.contains_key(&connected.local_addr().unwrap().port()));
        assert!(!native.contains_key(&udp.local_addr().unwrap().port()));
        drop((accepted, connected));

        if std::env::var_os("APPHUB_LISTENER_BENCH").is_some() {
            fn cpu(who: i32) -> f64 {
                let mut usage = MaybeUninit::<libc::rusage>::uninit();
                assert_eq!(unsafe { libc::getrusage(who, usage.as_mut_ptr()) }, 0);
                let usage = unsafe { usage.assume_init() };
                (usage.ru_utime.tv_sec + usage.ru_stime.tv_sec) as f64 * 1000.0
                    + (usage.ru_utime.tv_usec + usage.ru_stime.tv_usec) as f64 / 1000.0
            }
            let mut samples = Vec::new();
            for _ in 0..20 {
                let before_cpu = cpu(libc::RUSAGE_SELF);
                let start = std::time::Instant::now();
                assert!(listening_ports().is_ok());
                let native_wall = start.elapsed().as_secs_f64() * 1000.0;
                let native_cpu = cpu(libc::RUSAGE_SELF) - before_cpu;
                let before_cpu = cpu(libc::RUSAGE_CHILDREN);
                let start = std::time::Instant::now();
                assert!(Command::new("lsof")
                    .args(["-nP", "-iTCP", "-sTCP:LISTEN", "-Fpn"])
                    .output()
                    .unwrap()
                    .status
                    .success());
                samples.push(serde_json::json!({"nativeWallMs":native_wall, "nativeCpuMs":native_cpu,
                    "lsofWallMs":start.elapsed().as_secs_f64()*1000.0, "lsofCpuMs":cpu(libc::RUSAGE_CHILDREN)-before_cpu}));
            }
            println!(
                "LISTENER_BENCH:{}",
                serde_json::to_string(&samples).unwrap()
            );
        }
    }
}
