use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub const MANAGED_RUN_ENV: &str = "__BENTO_MANAGED_RUN";

pub fn new_launch_marker() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!(
        "{}-{}-{}",
        std::process::id(),
        time.as_nanos(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

fn contains_marker(data: &[u8], marker: &str) -> bool {
    let Some((count, data)) = data.split_at_checked(size_of::<i32>()) else {
        return false;
    };
    let argc = i32::from_ne_bytes(count.try_into().unwrap()).max(0) as usize;
    // Darwin returns argc, the executable path, alignment NULs, argv, and envp.
    let Some(path_end) = data.iter().position(|&byte| byte == 0) else {
        return false;
    };
    let Some(args_start) = data[path_end..].iter().position(|&byte| byte != 0) else {
        return false;
    };
    let expected = format!("{MANAGED_RUN_ENV}={marker}");
    data[path_end + args_start..]
        .split(|&byte| byte == 0)
        .skip(argc)
        .any(|entry| entry == expected.as_bytes())
}

pub fn matches(pid: u32, marker: &str) -> bool {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid as i32];
    let mut len = 0;
    // Some macOS processes do not expose their environment. Treat them as
    // unverified and rely on tracked ancestry instead.
    if unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            std::ptr::null_mut(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    } != 0
        || len < size_of::<i32>()
    {
        return false;
    }
    let mut data = vec![0; len];
    if unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            data.as_mut_ptr().cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    } != 0
        || len > data.len()
    {
        return false;
    }
    contains_marker(&data[..len], marker)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_matches_an_exact_environment_entry() {
        let mut data = Vec::from(2_i32.to_ne_bytes());
        data.extend_from_slice(b"/bin/test\0\0/bin/test\0__BENTO_MANAGED_RUN=argument\0HOME=/tmp\0__BENTO_MANAGED_RUN=fixture\0");
        assert!(contains_marker(&data, "fixture"));
        assert!(!contains_marker(&data, "argument"));
        assert!(!contains_marker(&data, "fix"));
        assert!(!contains_marker(b"short", "fixture"));
    }
}
