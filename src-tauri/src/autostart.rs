use rusqlite::Connection;
use serde::Serialize;
use std::sync::Mutex;
use tauri::{AppHandle, Manager};
use tauri_plugin_autostart::ManagerExt;

#[derive(Default)]
pub struct AutostartState(Mutex<()>);

#[derive(Debug, Serialize)]
pub struct AutostartStatus {
    available: bool,
    enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<&'static str>,
}

pub fn plugin() -> tauri::plugin::TauriPlugin<tauri::Wry> {
    let builder = tauri_plugin_autostart::Builder::new().app_name("Bento").args(["--autostart"]);
    #[cfg(target_os = "macos")]
    let builder = builder.macos_launcher(tauri_plugin_autostart::MacosLauncher::LaunchAgent);
    builder.build()
}

// The opt-in cannot accidentally register the debug executable in the real account.
#[cfg(all(debug_assertions, unix))]
fn isolated_test_home() -> bool {
    use std::ffi::CStr;
    let Some(test_home) = std::env::var_os("HOME").map(std::path::PathBuf::from)
        .filter(|path| path.is_absolute()).and_then(|path| path.canonicalize().ok()) else { return false; };
    let mut buffer = vec![0_u8; 16384];
    let mut password = std::mem::MaybeUninit::<libc::passwd>::uninit();
    let mut result = std::ptr::null_mut();
    // getpwuid_r obtains the account home independently of the child process's HOME.
    unsafe {
        if libc::getpwuid_r(libc::getuid(), password.as_mut_ptr(), buffer.as_mut_ptr().cast(), buffer.len(), &mut result) != 0
            || result.is_null() { return false; }
        let password = password.assume_init();
        if password.pw_dir.is_null() { return false; }
        use std::os::unix::ffi::OsStrExt;
        let account_home = std::path::Path::new(std::ffi::OsStr::from_bytes(CStr::from_ptr(password.pw_dir).to_bytes()));
        account_home.canonicalize().is_ok_and(|account_home| test_home != account_home)
    }
}

#[cfg(all(debug_assertions, not(unix)))]
fn isolated_test_home() -> bool { false }

#[cfg(debug_assertions)]
fn debug_allowed(opt_in: bool, fixture: bool, isolated_home: bool) -> bool {
    opt_in && fixture && isolated_home
}

fn available() -> bool {
    #[cfg(debug_assertions)]
    { debug_allowed(std::env::var_os("APPHUB_E2E_AUTOSTART").is_some_and(|value| value == "1"),
        crate::debug_e2e::database_path().is_some(), isolated_test_home()) }
    #[cfg(not(debug_assertions))]
    { true }
}

const NOT_INSTALLED: &str = "请先把 Bento 拖到「应用程序」文件夹后再开启";

/// A copy run from a mounted DMG or an App Translocation path sits on a read-only file system
/// and disappears later, so login must never point at it.
fn running_from_read_only_location() -> bool {
    let Some(path) = std::env::current_exe().ok().and_then(|path| path.canonicalize().ok()) else { return false; };
    let Ok(path) = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()) else { return false; };
    let mut stats = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: statfs fills `stats` for a valid NUL-terminated path; it is read only on success.
    unsafe { libc::statfs(path.as_ptr(), stats.as_mut_ptr()) == 0 && stats.assume_init().f_flags & libc::MNT_RDONLY as u32 != 0 }
}

fn status(app: &AppHandle) -> Result<AutostartStatus, String> {
    if !available() {
        return Ok(AutostartStatus { available: false, enabled: false, reason: Some("开发模式下不可用") });
    }
    Ok(AutostartStatus { available: true, enabled: app.autolaunch().is_enabled().map_err(|error| error.to_string())?, reason: None })
}

#[derive(Debug, PartialEq)]
enum StartupAction { None, ApplyDefault, Refresh }

fn startup_action(available: bool, applied: bool, enabled: bool) -> StartupAction {
    if !available { StartupAction::None }
    else if !applied { StartupAction::ApplyDefault }
    else if enabled { StartupAction::Refresh }
    else { StartupAction::None }
}

fn apply_startup(conn: &Connection, available: bool, enabled: bool, enable: impl FnOnce() -> Result<(), String>) -> Result<(), String> {
    match startup_action(available, crate::settings::default_applied(conn)?, enabled) {
        StartupAction::None => Ok(()),
        StartupAction::ApplyDefault => { enable()?; crate::settings::record_default_applied(conn) },
        StartupAction::Refresh => enable(),
    }
}

pub fn initialize(app: &AppHandle) -> Result<(), String> {
    // Leave the default unapplied so it happens once Bento runs from an installed copy.
    if !available() || running_from_read_only_location() { return Ok(()); }
    let state = app.state::<AutostartState>();
    let _guard = state.0.lock().map_err(|error| error.to_string())?;
    let conn = crate::open_project_db()?;
    let manager = app.autolaunch();
    let enabled = manager.is_enabled().map_err(|error| error.to_string())?;
    apply_startup(&conn, true, enabled, || manager.enable().map_err(|error| error.to_string()))
}

#[tauri::command]
pub fn autostart_status(app: AppHandle) -> Result<AutostartStatus, String> {
    let state = app.state::<AutostartState>();
    let _guard = state.0.lock().map_err(|error| error.to_string())?;
    status(&app)
}

#[tauri::command]
pub fn set_autostart(app: AppHandle, enabled: bool) -> Result<AutostartStatus, String> {
    let state = app.state::<AutostartState>();
    let _guard = state.0.lock().map_err(|error| error.to_string())?;
    if !available() { return Err("开发模式下不可用".into()); }
    if enabled && running_from_read_only_location() { return Err(NOT_INSTALLED.into()); }
    let conn = crate::open_project_db()?;
    let manager = app.autolaunch();
    let previous = manager.is_enabled().map_err(|error| error.to_string())?;
    if enabled { manager.enable() } else { manager.disable() }.map_err(|error| error.to_string())?;
    // An explicit user choice also completes first-run initialization, including choosing off.
    if let Err(error) = crate::settings::record_default_applied(&conn) {
        let rollback = if previous { manager.enable() } else { manager.disable() };
        return Err(match rollback { Ok(()) => error, Err(rollback) => format!("{error}；恢复启动项失败：{rollback}") });
    }
    status(&app)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[cfg(debug_assertions)]
    #[test]
    fn debug_autostart_requires_opt_in_fixture_and_isolated_home() {
        for (opt_in, fixture, isolated, expected) in [
            (false, false, false, false), (false, false, true, false),
            (false, true, false, false), (false, true, true, false),
            (true, false, false, false), (true, false, true, false),
            (true, true, false, false), (true, true, true, true),
        ] {
            assert_eq!(debug_allowed(opt_in, fixture, isolated), expected);
        }
    }

    #[test]
    fn writable_build_directory_is_not_a_read_only_location() {
        assert!(!running_from_read_only_location());
    }

    #[test]
    fn default_on_is_once_and_disabled_choice_is_preserved() {
        for applied in [false, true] {
            for enabled in [false, true] {
                assert_eq!(startup_action(false, applied, enabled), StartupAction::None);
            }
        }
        assert_eq!(startup_action(true, false, false), StartupAction::ApplyDefault);
        assert_eq!(startup_action(true, false, true), StartupAction::ApplyDefault);
        assert_eq!(startup_action(true, true, false), StartupAction::None);
        assert_eq!(startup_action(true, true, true), StartupAction::Refresh);
    }

    #[test]
    fn failed_default_retries_and_success_refreshes_only_when_enabled() {
        let conn = Connection::open_in_memory().unwrap();
        crate::init_db(&conn).unwrap();
        assert!(apply_startup(&conn, true, false, || Err("fixture failure".into())).is_err());
        assert!(!crate::settings::default_applied(&conn).unwrap());
        let calls = Cell::new(0);
        let enable = || { calls.set(calls.get() + 1); Ok(()) };
        apply_startup(&conn, false, false, enable).unwrap();
        assert_eq!(calls.get(), 0);
        apply_startup(&conn, true, false, enable).unwrap();
        assert_eq!(calls.get(), 1);
        assert!(crate::settings::default_applied(&conn).unwrap());
        apply_startup(&conn, true, false, enable).unwrap();
        assert_eq!(calls.get(), 1);
        apply_startup(&conn, true, true, enable).unwrap();
        assert_eq!(calls.get(), 2);
    }
}
