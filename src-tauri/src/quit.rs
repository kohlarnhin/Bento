use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    AppHandle, Emitter, Manager,
};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

#[derive(Default)]
pub struct QuitPending(AtomicBool);

pub fn install_menu(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let menu = Menu::default(app)?;
    #[cfg(debug_assertions)]
    let before = crate::debug_e2e::menu_snapshot(&menu)?;
    // Tauri 2.11's default app submenu is first; compare against the native Quit's own text.
    let quit_text = PredefinedMenuItem::quit(app, None)?.text()?;
    let items = menu.items()?;
    let app_menu = items
        .first()
        .and_then(|item| item.as_submenu())
        .ok_or("无法找到系统应用菜单")?;
    let about = PredefinedMenuItem::about(app, None, None)?.text()?;
    let about_index = app_menu.items()?.iter().position(|item| {
        item.as_predefined_menuitem()
            .is_some_and(|item| item.text().ok().as_deref() == Some(&about))
    }).ok_or("无法找到系统菜单的关于项目")?;
    app_menu.insert(
        &MenuItem::with_id(app, "apphub-check-updates", "检查更新…", true, None::<&str>)?,
        about_index + 1,
    )?;
    app_menu.insert(
        &MenuItem::with_id(app, "apphub-settings", "设置…", true, Some("CmdOrCtrl+,"))?,
        about_index + 2,
    )?;
    let index = app_menu
        .items()?
        .iter()
        .position(|item| {
            item.as_predefined_menuitem()
                .is_some_and(|item| item.text().ok().as_deref() == Some(&quit_text))
        })
        .ok_or("无法找到系统菜单的退出项目")?;
    app_menu.remove_at(index)?;
    app_menu.insert(
        &MenuItem::with_id(app, "apphub-quit", "退出 Bento", true, Some("CmdOrCtrl+Q"))?,
        index,
    )?;
    #[cfg(debug_assertions)]
    crate::debug_e2e::record_menu(before, crate::debug_e2e::menu_snapshot(&menu)?);
    app.set_menu(menu)?;
    app.on_menu_event(|app, event| {
        if event.id().as_ref() == "apphub-quit" {
            request(app);
        } else if event.id().as_ref() == "apphub-check-updates" {
            crate::reopen_main_window(app);
            let _ = app.emit("bento-check-updates", ());
        } else if event.id().as_ref() == "apphub-settings" {
            crate::reopen_main_window(app);
            let _ = app.emit("bento-open-settings", ());
        } else {
            crate::tray::handle_menu(app, event.id().as_ref());
        }
    });
    Ok(())
}

pub fn request(app: &AppHandle) {
    if app.state::<QuitPending>().0.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let ids: HashSet<String> = {
            let state = app.state::<crate::lifecycle::ProcessMap>();
            let data = state.lock().await;
            data.managed
                .keys()
                .cloned()
                .chain(data.stopping.values().map(|(id, _)| id.clone()))
                .collect()
        };
        let terminals = app
            .state::<crate::terminal::TerminalManager>()
            .list()
            .iter()
            .filter(|session| session.alive)
            .count();
        if ids.is_empty() && terminals == 0 {
            app.exit(0);
            return;
        }
        let names = tauri::async_runtime::spawn_blocking(move || {
            let mut names = Vec::new();
            let mut remaining = ids;
            if let Ok(response) = crate::get_projects_sync() {
                for project in response.projects {
                    if remaining.remove(&project.id) {
                        names.push(project.display_name.unwrap_or(project.name));
                    }
                }
            }
            names.extend(remaining);
            names
        })
        .await
        .unwrap_or_else(|_| vec!["运行中的项目".into()]);
        let mut lines = Vec::new();
        if !names.is_empty() {
            lines.push(format!("正在运行的项目：{}（将被停止）", names.join("、")));
        }
        if terminals > 0 {
            lines.push(format!(
                "打开的终端：{terminals} 个（其中运行的命令会被终止）"
            ));
        }
        let message = lines.join("\n\n");
        crate::reopen_main_window(&app);
        #[cfg(debug_assertions)]
        if let Some(confirm) = crate::debug_e2e::quit_answer("退出 Bento？", &message) {
            // Keep the simulated dialog open like a real one, so repeated requests can be tested.
            tokio::time::sleep(crate::debug_e2e::quit_answer_delay()).await;
            finish(&app, confirm);
            return;
        }
        let dialog_app = app.clone();
        app.dialog()
            .message(message)
            .title("退出 Bento？")
            .kind(MessageDialogKind::Warning)
            .buttons(MessageDialogButtons::OkCancelCustom(
                "停止并退出".into(),
                "取消".into(),
            ))
            .show(move |confirmed| finish(&dialog_app, confirmed));
    });
}

fn finish(app: &AppHandle, confirmed: bool) {
    if confirmed {
        app.exit(0);
    } else {
        app.state::<QuitPending>().0.store(false, Ordering::SeqCst);
    }
}
