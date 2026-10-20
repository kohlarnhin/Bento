//! Real-webview verification only. This entire module and its commands are absent in release builds.
use serde_json::{json, Value};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use tauri::{AppHandle, Manager, Webview};

static COMMANDS: Mutex<Vec<String>> = Mutex::new(Vec::new());
static QUIT_CONFIRMATIONS: Mutex<Vec<Value>> = Mutex::new(Vec::new());
static STOP_ALL_CONFIRMATIONS: Mutex<Vec<Value>> = Mutex::new(Vec::new());
static LSOF_CALLS: AtomicU64 = AtomicU64::new(0);
static STATUS_POLLS: AtomicU64 = AtomicU64::new(0);
static LOG_BATCHES: AtomicU64 = AtomicU64::new(0);
static LOG_LINES: AtomicU64 = AtomicU64::new(0);
static SYSTEM_LOGS: AtomicU64 = AtomicU64::new(0);
static DB_INITIALIZATIONS: AtomicU64 = AtomicU64::new(0);
static OPENED_URLS: Mutex<Vec<String>> = Mutex::new(Vec::new());

pub fn record_open_url(url: &str) -> bool {
    if !enabled() { return false; }
    if let Ok(mut urls) = OPENED_URLS.lock() { urls.push(url.into()); }
    true
}

pub fn record_lsof() {
    if enabled() { LSOF_CALLS.fetch_add(1, Ordering::Relaxed); }
}
pub fn record_log_batch(lines: usize) {
    if enabled() {
        LOG_BATCHES.fetch_add(1, Ordering::Relaxed);
        LOG_LINES.fetch_add(lines as u64, Ordering::Relaxed);
    }
}
pub fn record_system_log() {
    if enabled() { SYSTEM_LOGS.fetch_add(1, Ordering::Relaxed); }
}
pub fn record_db_initialization() {
    if enabled() { DB_INITIALIZATIONS.fetch_add(1, Ordering::Relaxed); }
}

#[tauri::command]
pub fn e2e_metrics(reset: Option<bool>) -> Result<Value, String> {
    if !enabled() { return Err("E2E hook disabled".into()); }
    let read = |counter: &AtomicU64| {
        if reset == Some(true) { counter.swap(0, Ordering::Relaxed) } else { counter.load(Ordering::Relaxed) }
    };
    Ok(json!({
        "lsofCalls": read(&LSOF_CALLS), "statusPolls": read(&STATUS_POLLS),
        "logBatches": read(&LOG_BATCHES), "logLines": read(&LOG_LINES),
        "systemLogs": read(&SYSTEM_LOGS), "dbInitializations": read(&DB_INITIALIZATIONS),
    }))
}

pub fn quit_answer(title: &str, message: &str) -> Option<bool> {
    if !enabled() { return None; }
    let answer = std::env::var("APPHUB_E2E_QUIT_ANSWER").ok()?;
    let confirm = match answer.as_str() { "confirm" => true, "cancel" => false, _ => return None };
    if let Ok(mut records) = QUIT_CONFIRMATIONS.lock() {
        records.push(json!({ "title": title, "message": message, "answer": answer }));
        if let Some(path) = std::env::var_os("APPHUB_E2E_OUT") {
            let path = std::path::PathBuf::from(path).with_extension("quit.json");
            let _ = std::fs::write(path, serde_json::to_vec_pretty(&*records).unwrap_or_default());
        }
    }
    Some(confirm)
}

pub fn stop_all_answer(title: &str, message: &str) -> Option<bool> {
    if !enabled() { return None; }
    let answer = std::env::var("APPHUB_E2E_STOP_ALL_ANSWER").ok()?;
    let confirm = match answer.as_str() { "confirm" => true, "cancel" => false, _ => return None };
    if let Ok(mut records) = STOP_ALL_CONFIRMATIONS.lock() {
        records.push(json!({ "title": title, "message": message, "answer": answer }));
    }
    Some(confirm)
}

#[tauri::command]
pub fn e2e_tray(app: AppHandle, id: Option<String>) -> Result<Value, String> {
    if !enabled() { return Err("E2E hook disabled".into()); }
    let (view, menu) = crate::tray::debug_state(&app).ok_or("Tray not installed")?;
    if let Some(id) = id {
        if !view.items.iter().any(|item| item.id == id && item.enabled && !item.separator) {
            return Err("Tray item missing or disabled".into());
        }
        crate::tray::handle_menu(&app, &id);
    }
    let native_items = menu.items().map_err(|error| error.to_string())?.into_iter().map(|item| {
        match item {
            tauri::menu::MenuItemKind::MenuItem(item) => Ok(json!({ "id": item.id().as_ref(), "text": item.text().map_err(|error| error.to_string())?, "enabled": item.is_enabled().map_err(|error| error.to_string())? })),
            item => Ok(json!({ "id": item.id().as_ref(), "separator": true })),
        }
    }).collect::<Result<Vec<Value>, String>>()?;
    let tray = app.tray_by_id(crate::tray::ID).ok_or("Tray not installed")?;
    let native = tray.with_inner_tray_icon(|inner| {
        let item = inner.ns_status_item()?;
        let button = item.button(objc2::MainThreadMarker::new()?)?;
        let image = button.image()?;
        Some(json!({ "title": button.title().to_string(), "tooltip": button.toolTip().map(|text| text.to_string()),
            "template": image.isTemplate(), "width": image.size().width, "height": image.size().height,
            "visible": item.isVisible() }))
    }).map_err(|error| error.to_string())?;
    Ok(json!({ "title": view.title, "tooltip": view.tooltip, "projects": view.projects, "items": view.items,
        "nativeItems": native_items, "installed": true, "native": native,
        "stopAllConfirmations": STOP_ALL_CONFIRMATIONS.lock().map(|records| records.clone()).unwrap_or_default() }))
}

pub fn menu_snapshot(menu: &tauri::menu::Menu<tauri::Wry>) -> tauri::Result<Vec<Value>> {
    fn items(entries: Vec<tauri::menu::MenuItemKind<tauri::Wry>>) -> tauri::Result<Vec<Value>> {
        use tauri::menu::MenuItemKind;
        entries.into_iter().map(|item| {
            Ok(match item {
                MenuItemKind::Submenu(submenu) => json!({ "id": submenu.id().as_ref(), "kind": "submenu", "text": submenu.text()?, "items": items(submenu.items()?)? }),
                MenuItemKind::Predefined(item) => json!({ "id": item.id().as_ref(), "kind": "predefined", "text": item.text()? }),
                MenuItemKind::MenuItem(item) => json!({ "id": item.id().as_ref(), "kind": "custom", "text": item.text()? }),
                MenuItemKind::Check(item) => json!({ "id": item.id().as_ref(), "kind": "check", "text": item.text()? }),
                MenuItemKind::Icon(item) => json!({ "id": item.id().as_ref(), "kind": "icon", "text": item.text()? }),
            })
        }).collect()
    }
    items(menu.items()?)
}

pub fn record_menu(before: Vec<Value>, after: Vec<Value>) {
    if !enabled() { return; }
    if let Some(path) = std::env::var_os("APPHUB_E2E_OUT") {
        let path = std::path::PathBuf::from(path).with_extension("menu.json");
        let _ = std::fs::write(path, serde_json::to_vec_pretty(&json!({ "before": before, "after": after })).unwrap_or_default());
    }
}

pub fn record_command(command: &str) {
    if enabled() && command == "get_projects_runtime_status" {
        STATUS_POLLS.fetch_add(1, Ordering::Relaxed);
    }
    if enabled()
        && matches!(
            command,
            "get_projects"
                | "add_project"
                | "reorder_projects"
                | "update_project"
                | "delete_project"
                | "inspect_project_dir"
                | "check_port_occupied"
                | "get_projects_runtime_status"
                | "start_project"
                | "stop_project"
                | "stop_all_projects"
                | "restart_project"
                | "get_port_owners"
                | "stop_external_process"
                | "open_url"
                | "reveal_path"
        )
    {
        if let Ok(mut commands) = COMMANDS.lock() {
            commands.push(command.to_string());
        }
    }
}

fn enabled() -> bool {
    std::env::var_os("APPHUB_E2E_SCRIPT").is_some() && std::env::var_os("APPHUB_E2E_OUT").is_some()
}

pub fn database_path() -> Option<std::path::PathBuf> {
    enabled()
        .then(|| std::env::var_os("APPHUB_E2E_DB"))
        .flatten()
        .map(Into::into)
}

pub fn on_page_load(webview: &Webview, payload: &tauri::webview::PageLoadPayload<'_>) {
    if !enabled()
        || webview.label() != "main"
        || payload.event() != tauri::webview::PageLoadEvent::Finished
    {
        return;
    }
    if let Some(path) = std::env::var_os("APPHUB_E2E_SCRIPT") {
        match std::fs::read_to_string(path) {
            Ok(script) => {
                let _ = webview.eval(script);
            }
            Err(err) => eprintln!("Bento e2e script: {err}"),
        }
    }
}

#[tauri::command]
pub fn e2e_report(report: Value, name: Option<String>) -> Result<(), String> {
    if !enabled() {
        return Err("E2E hook disabled".into());
    }
    let mut path = std::path::PathBuf::from(
        std::env::var_os("APPHUB_E2E_OUT").ok_or("Missing E2E output path")?,
    );
    if let Some(name) = name {
        if name.is_empty()
            || !name
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '-')
        {
            return Err("Invalid E2E report name".into());
        }
        path.set_file_name(format!("{name}.json"));
    }
    let data = serde_json::to_vec_pretty(&report).map_err(|err| err.to_string())?;
    std::fs::write(path, data).map_err(|err| err.to_string())
}

#[tauri::command]
pub fn e2e_window(
    app: AppHandle,
    action: String,
    width: Option<f64>,
    height: Option<f64>,
) -> Result<Value, String> {
    if !enabled() {
        return Err("E2E hook disabled".into());
    }
    let window = app
        .get_webview_window("main")
        .ok_or("Missing main window")?;
    match action.as_str() {
        "close" => window.close().map_err(|err| err.to_string())?,
        "minimize" => window.minimize().map_err(|err| err.to_string())?,
        "reopen" => crate::reopen_main_window(&app),
        "dock-reopen" => window.with_webview(|_| {
            use objc2::MainThreadMarker;
            use objc2_app_kit::{NSApplication, NSApplicationDelegate};
            let application = NSApplication::sharedApplication(MainThreadMarker::new().unwrap());
            if let Some(delegate) = application.delegate() {
                // Replay the native delegate callback used by a Dock reopen, inside this app only.
                delegate.applicationShouldHandleReopen_hasVisibleWindows(&application, false);
            }
        }).map_err(|err| err.to_string())?,
        "paste" => window
            .with_webview(|_| {
                use objc2::{sel, MainThreadMarker};
                use objc2_app_kit::NSApplication;
                // This is the same responder-chain action used by the native Edit > Paste menu.
                let application =
                    NSApplication::sharedApplication(MainThreadMarker::new().unwrap());
                unsafe {
                    application.sendAction_to_from(sel!(paste:), None, None);
                }
            })
            .map_err(|err| err.to_string())?,
        "resize" => window
            .set_size(tauri::LogicalSize::new(
                width.ok_or("Missing width")?,
                height.ok_or("Missing height")?,
            ))
            .map_err(|err| err.to_string())?,
        "quit" => app.exit(0),
        "quit-request" => crate::quit::request(&app),
        "status" => {}
        _ => return Err("Unknown E2E window action".into()),
    }
    let monitor = window.current_monitor().map_err(|err| err.to_string())?;
    Ok(json!({
        "visible": window.is_visible().map_err(|err| err.to_string())?,
        "pid": std::process::id(),
        "version": app.package_info().version.to_string(),
        "home": std::env::var_os("HOME"),
        "dbPath": crate::get_db_path(),
        "size": window.inner_size().map_err(|err| err.to_string())?,
        "outerSize": window.outer_size().map_err(|err| err.to_string())?,
        "position": window.outer_position().map_err(|err| err.to_string())?,
        "scaleFactor": window.scale_factor().map_err(|err| err.to_string())?,
        "monitor": monitor.map(|monitor| json!({ "workArea": monitor.work_area(), "size": monitor.size(), "scaleFactor": monitor.scale_factor() })),
        "commands": COMMANDS.lock().map(|commands| commands.clone()).unwrap_or_default(),
        "quitConfirmations": QUIT_CONFIRMATIONS.lock().map(|records| records.clone()).unwrap_or_default(),
        "openedUrls": OPENED_URLS.lock().map(|urls| urls.clone()).unwrap_or_default(),
    }))
}

#[tauri::command]
pub async fn e2e_menu(app: AppHandle, action: Option<String>) -> Result<Value, String> {
    if !enabled() { return Err("E2E hook disabled".into()); }
    let window = app.get_webview_window("main").ok_or("Missing main window")?;
    let (sender, receiver) = tokio::sync::oneshot::channel();
    window.with_webview(move |webview| {
        use objc2::MainThreadMarker;
        use objc2_app_kit::{NSApplication, NSEventModifierFlags, NSMenu};
        use objc2_web_kit::WKWebView;
        fn dump(menu: &NSMenu) -> Value {
            json!({ "title": menu.title().to_string(), "items": menu.itemArray().iter().map(|item| {
                json!({ "title": item.title().to_string(), "key": item.keyEquivalent().to_string(), "modifiers": item.keyEquivalentModifierMask().bits(), "submenu": item.submenu().map(|menu| dump(&menu)) })
            }).collect::<Vec<_>>() })
        }
        fn perform(menu: &NSMenu, action: &str) -> bool {
            for (index, item) in menu.itemArray().iter().enumerate() {
                let key = item.keyEquivalent().to_string();
                let title = item.title().to_string();
                let command = item.keyEquivalentModifierMask().contains(NSEventModifierFlags::Command)
                    && !item.keyEquivalentModifierMask().contains(NSEventModifierFlags::Option);
                let matched = match action { "hide" => command && key == "h", "close" => command && key == "w", "quit" => command && key == "q", "about" => title.starts_with("About "), "check-updates" => title == "检查更新…", "settings" => command && key == "," && title == "设置…", _ => false };
                if matched { menu.performActionForItemAtIndex(index as _); return true; }
                if item.submenu().is_some_and(|submenu| perform(&submenu, action)) { return true; }
            }
            false
        }
        let result = (|| {
            let application = NSApplication::sharedApplication(MainThreadMarker::new().ok_or("Not on main thread")?);
            let menu = application.mainMenu().ok_or("Missing native menu")?;
            let mut performed = false;
            match action.as_deref().unwrap_or("status") {
                "status" => {},
                "unhide" => { application.unhide(None); performed = true; },
                "close-auxiliary" => {
                    let view = unsafe { &*webview.inner().cast::<WKWebView>() };
                    let main_number = view.window().map(|window| window.windowNumber());
                    for window in application.windows().iter() {
                        if Some(window.windowNumber()) != main_number { window.orderOut(None); }
                    }
                    performed = true;
                },
                action => { performed = perform(&menu, action); },
            }
            Ok::<_, String>(json!({ "menu": dump(&menu), "performed": performed, "applicationHidden": application.isHidden(), "windows": application.windows().iter().map(|window| json!({ "title": window.title().to_string(), "visible": window.isVisible() })).collect::<Vec<_>>() }))
        })();
        let _ = sender.send(result);
    }).map_err(|err| err.to_string())?;
    tokio::time::timeout(std::time::Duration::from_secs(10), receiver).await.map_err(|err| err.to_string())?.map_err(|err| err.to_string())?
}

#[tauri::command]
pub async fn e2e_snapshot(app: AppHandle, path: String) -> Result<(), String> {
    use block2::RcBlock;
    use objc2::AnyThread;
    use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSImage};
    use objc2_foundation::{NSDictionary, NSError, NSString};
    use objc2_web_kit::WKWebView;
    use std::sync::Arc;

    if !enabled() {
        return Err("E2E hook disabled".into());
    }
    let window = app
        .get_webview_window("main")
        .ok_or("Missing main window")?;
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let sender = Arc::new(Mutex::new(Some(sender)));
    window
        .with_webview(move |webview| {
            let completion = RcBlock::new(move |image: *mut NSImage, error: *mut NSError| {
                // WebKit owns the callback arguments and invokes this block on the main thread.
                let result = (|| unsafe {
                    if let Some(error) = error.as_ref() {
                        return Err(error.localizedDescription().to_string());
                    }
                    let image = image.as_ref().ok_or("WebKit returned no snapshot")?;
                    let tiff = image.TIFFRepresentation().ok_or("Missing TIFF data")?;
                    let bitmap = NSBitmapImageRep::initWithData(NSBitmapImageRep::alloc(), &tiff)
                        .ok_or("Cannot decode snapshot")?;
                    let png = bitmap
                        .representationUsingType_properties(
                            NSBitmapImageFileType::PNG,
                            &NSDictionary::new(),
                        )
                        .ok_or("Cannot encode PNG")?;
                    if !png.writeToFile_atomically(&NSString::from_str(&path), true) {
                        return Err("Cannot write snapshot PNG".into());
                    }
                    Ok(())
                })();
                if let Some(sender) = sender.lock().unwrap().take() {
                    let _ = sender.send(result);
                }
            });
            // with_webview runs on the UI thread; the native pointer is a WKWebView.
            unsafe {
                let webview = &*webview.inner().cast::<WKWebView>();
                webview.takeSnapshotWithConfiguration_completionHandler(None, &completion);
            }
        })
        .map_err(|err| err.to_string())?;
    tokio::time::timeout(std::time::Duration::from_secs(15), receiver)
        .await
        .map_err(|_| "WebKit snapshot timed out".to_string())?
        .map_err(|err| err.to_string())?
}

/// Native events provide a real webview user gesture for clipboard permission checks.
#[tauri::command]
pub fn e2e_click(app: AppHandle, x: f64, y: f64, action: Option<String>) -> Result<(), String> {
    use objc2_app_kit::{NSEvent, NSEventModifierFlags, NSEventType};
    use objc2_foundation::NSPoint;
    use objc2_web_kit::WKWebView;

    if !enabled() {
        return Err("E2E hook disabled".into());
    }
    let events = match action.as_deref().unwrap_or("click") {
        "click" => vec![NSEventType::LeftMouseDown, NSEventType::LeftMouseUp],
        "down" => vec![NSEventType::LeftMouseDown],
        "move" => vec![NSEventType::LeftMouseDragged],
        "up" => vec![NSEventType::LeftMouseUp],
        _ => return Err("Unknown E2E pointer action".into()),
    };
    let window = app
        .get_webview_window("main")
        .ok_or("Missing main window")?;
    window.with_webview(move |webview| unsafe {
        let view = &*webview.inner().cast::<WKWebView>();
        if let Some(window) = view.window() {
            let y = if view.isFlipped() { y } else { view.bounds().size.height - y };
            let point = view.convertPoint_toView(NSPoint::new(x, y), None);
            for event_type in events {
                if let Some(event) = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
                    event_type, point, NSEventModifierFlags::empty(), 0.0, window.windowNumber(), None, 0, 1, 1.0,
                ) {
                    window.sendEvent(&event);
                }
            }
        }
    }).map_err(|err| err.to_string())
}
