use crate::lifecycle::{ManagedProject, Phase, ProcessMap, SHUTTING_DOWN};
use crate::ProjectConfig;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Mutex,
};
use tauri::{
    image::Image,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
    AppHandle, Emitter, Manager,
};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

pub const ID: &str = "apphub-tray";
const PROJECT_PREFIX: &str = "apphub-tray-project-";
const STOP_ALL: &str = "apphub-tray-stop-all";
const SHOW: &str = "apphub-tray-show";
const QUIT: &str = "apphub-tray-quit";

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrayProject {
    id: String,
    name: String,
    port: Option<u16>,
    starting: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TrayItem {
    pub id: String,
    pub text: String,
    pub enabled: bool,
    pub separator: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TrayView {
    pub title: String,
    pub tooltip: String,
    pub projects: Vec<TrayProject>,
    pub items: Vec<TrayItem>,
}

impl TrayView {
    fn from_snapshot(projects: &[ProjectConfig], managed: &[ManagedProject]) -> Self {
        let mut remaining: HashMap<_, _> = managed
            .iter()
            .map(|project| (project.id.as_str(), project))
            .collect();
        let mut entries = Vec::new();
        for project in projects {
            if let Some(info) = remaining.remove(project.id.as_str()) {
                entries.push(TrayProject {
                    id: project.id.clone(),
                    name: project
                        .display_name
                        .as_ref()
                        .filter(|name| !name.is_empty())
                        .unwrap_or(&project.name)
                        .clone(),
                    port: info.port,
                    starting: info.phase == Phase::Starting,
                });
            }
        }
        // A CLI deletion can leave a managed process without its configuration.
        // Keep it reachable and stoppable until the process actually ends.
        for info in managed
            .iter()
            .filter(|info| remaining.contains_key(info.id.as_str()))
        {
            entries.push(TrayProject {
                id: info.id.clone(),
                name: info.id.clone(),
                port: info.port,
                starting: info.phase == Phase::Starting,
            });
        }
        let count = entries.len();
        let item = |id: &str, text: String, enabled| TrayItem {
            id: id.into(),
            text,
            enabled,
            separator: false,
        };
        let separator = |index| TrayItem {
            id: format!("apphub-tray-separator-{index}"),
            text: String::new(),
            enabled: false,
            separator: true,
        };
        let mut items = vec![item(
            "apphub-tray-summary",
            if count == 0 {
                "没有运行中的项目".into()
            } else {
                format!("Bento 启动的项目 · {count} 个运行中")
            },
            false,
        )];
        for project in &entries {
            let port = project
                .port
                .map(|port| format!("  :{port}"))
                .unwrap_or_default();
            let phase = if project.starting {
                "（启动中）"
            } else {
                ""
            };
            items.push(item(
                &format!("{PROJECT_PREFIX}{}", project.id),
                format!("{}{port}{phase}", project.name),
                true,
            ));
        }
        items.extend([
            separator(1),
            item(STOP_ALL, "全部停止…".into(), count > 0),
            item(SHOW, "显示 Bento".into(), true),
            separator(2),
            item(QUIT, "退出 Bento".into(), true),
        ]);
        Self {
            title: if count == 0 {
                String::new()
            } else {
                count.to_string()
            },
            tooltip: format!("Bento · {count} 个项目运行中"),
            projects: entries,
            items,
        }
    }

    fn confirmation(&self) -> String {
        format!(
            "将停止以下由 Bento 启动的项目：{}。外部启动的进程不受影响。",
            self.projects
                .iter()
                .map(|project| project.name.as_str())
                .collect::<Vec<_>>()
                .join("、")
        )
    }
}

#[derive(Default)]
pub struct TrayState {
    generation: AtomicU64,
    stop_pending: AtomicBool,
    applied: Mutex<Option<(TrayView, Menu<tauri::Wry>)>>,
}

fn menu(app: &AppHandle, view: &TrayView) -> tauri::Result<Menu<tauri::Wry>> {
    let menu = Menu::new(app)?;
    for item in &view.items {
        if item.separator {
            menu.append(&PredefinedMenuItem::separator(app)?)?;
        } else {
            menu.append(&MenuItem::with_id(
                app,
                &item.id,
                &item.text,
                item.enabled,
                None::<&str>,
            )?)?;
        }
    }
    Ok(menu)
}

pub fn install(app: &AppHandle) -> tauri::Result<()> {
    let view = TrayView::from_snapshot(&[], &[]);
    let menu = menu(app, &view)?;
    // tray-icon scales the 36px image to 18pt on macOS, preserving Retina detail.
    TrayIconBuilder::with_id(ID)
        .icon(Image::from_bytes(include_bytes!(
            "../icons/tray-icon@2x.png"
        ))?)
        .icon_as_template(true)
        .title(&view.title)
        .tooltip(&view.tooltip)
        .show_menu_on_left_click(true)
        .menu(&menu)
        .build(app)?;
    *app.state::<TrayState>()
        .applied
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = Some((view, menu));
    Ok(())
}

async fn current(app: &AppHandle) -> Result<TrayView, String> {
    let managed = app.state::<ProcessMap>().lock().await.managed_snapshot();
    let projects = tauri::async_runtime::spawn_blocking(crate::get_projects_sync)
        .await
        .map_err(|error| error.to_string())??;
    Ok(TrayView::from_snapshot(&projects.projects, &managed))
}

pub fn refresh(app: &AppHandle) {
    let Some(state) = app.try_state::<TrayState>() else {
        return;
    };
    let generation = state.generation.fetch_add(1, Ordering::SeqCst) + 1;
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        if SHUTTING_DOWN.load(Ordering::SeqCst)
            || app.state::<TrayState>().generation.load(Ordering::SeqCst) != generation
        {
            return;
        }
        let view = match current(&app).await {
            Ok(view) => view,
            Err(error) => {
                eprintln!("Bento tray: {error}");
                return;
            }
        };
        let handle = app.clone();
        let _ = app.run_on_main_thread(move || {
            let state = handle.state::<TrayState>();
            if SHUTTING_DOWN.load(Ordering::SeqCst)
                || state.generation.load(Ordering::SeqCst) != generation
            {
                return;
            }
            if state
                .applied
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .as_ref()
                .is_some_and(|(previous, _)| previous == &view)
            {
                return;
            }
            let update = || -> tauri::Result<()> {
                let menu = menu(&handle, &view)?;
                if let Some(tray) = handle.tray_by_id(ID) {
                    tray.set_title(Some(&view.title))?;
                    tray.set_tooltip(Some(&view.tooltip))?;
                    tray.set_menu(Some(menu.clone()))?;
                }
                *state
                    .applied
                    .lock()
                    .unwrap_or_else(|error| error.into_inner()) = Some((view.clone(), menu));
                Ok(())
            };
            if let Err(error) = update() {
                eprintln!("Bento tray: {error}");
            }
        });
    });
}

pub fn handle_menu(app: &AppHandle, id: &str) {
    match id {
        SHOW => crate::reopen_main_window(app),
        QUIT => crate::quit::request(app),
        STOP_ALL => request_stop_all(app),
        id if id.starts_with(PROJECT_PREFIX) => {
            crate::reopen_main_window(app);
            #[derive(Clone, Serialize)]
            #[serde(rename_all = "camelCase")]
            struct Selection {
                project_id: String,
            }
            let _ = app.emit(
                "apphub-select-project",
                Selection {
                    project_id: id[PROJECT_PREFIX.len()..].into(),
                },
            );
        }
        _ => {}
    }
}

fn request_stop_all(app: &AppHandle) {
    if app
        .state::<TrayState>()
        .stop_pending
        .swap(true, Ordering::SeqCst)
    {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let view = match current(&app).await {
            Ok(view) => view,
            Err(error) => {
                finish_stop_all(&app, false);
                eprintln!("Bento tray: {error}");
                return;
            }
        };
        if view.projects.is_empty() {
            finish_stop_all(&app, false);
            return;
        }
        let message = view.confirmation();
        #[cfg(debug_assertions)]
        if let Some(answer) = crate::debug_e2e::stop_all_answer("停止全部项目？", &message) {
            finish_stop_all(&app, answer);
            return;
        }
        let dialog_app = app.clone();
        app.dialog()
            .message(message)
            .title("停止全部项目？")
            .kind(MessageDialogKind::Warning)
            .buttons(MessageDialogButtons::OkCancelCustom(
                "全部停止".into(),
                "取消".into(),
            ))
            .show(move |confirmed| finish_stop_all(&dialog_app, confirmed));
    });
}

fn finish_stop_all(app: &AppHandle, confirmed: bool) {
    if !confirmed {
        app.state::<TrayState>()
            .stop_pending
            .store(false, Ordering::SeqCst);
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(error) = crate::lifecycle::stop_all(&app, &app.state::<ProcessMap>()).await {
            app.dialog()
                .message(error)
                .title("停止项目失败")
                .kind(MessageDialogKind::Error)
                .show(|_| {});
        }
        app.state::<TrayState>()
            .stop_pending
            .store(false, Ordering::SeqCst);
    });
}

#[cfg(debug_assertions)]
pub fn debug_state(app: &AppHandle) -> Option<(TrayView, Menu<tauri::Wry>)> {
    app.state::<TrayState>()
        .applied
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(id: &str, display: Option<&str>) -> ProjectConfig {
        ProjectConfig {
            id: id.into(),
            name: id.into(),
            display_name: display.map(String::from),
            description: None,
            path: "/tmp".into(),
            port: None,
            start_command: "sleep 1".into(),
            project_type: Some("app".into()),
                env: Vec::new(), commands: Vec::new(), auto_open: false,
        }
    }

    #[test]
    fn derives_empty_title_and_disabled_stop_all() {
        let view = TrayView::from_snapshot(&[config("external", None)], &[]);
        assert_eq!(view.title, "");
        assert_eq!(view.tooltip, "Bento · 0 个项目运行中");
        assert_eq!(view.items[0].text, "没有运行中的项目");
        assert!(!view.items[0].enabled);
        assert!(
            !view
                .items
                .iter()
                .find(|item| item.id == STOP_ALL)
                .unwrap()
                .enabled
        );
        assert!(view.projects.is_empty());
    }

    #[test]
    fn derives_managed_names_ports_and_starting_suffix_only() {
        let projects = [
            config("running", Some("正在运行")),
            config("external", None),
            config("starting", None),
        ];
        let snapshot = [
            ManagedProject {
                id: "starting".into(),
                port: Some(5301),
                phase: Phase::Starting,
            },
            ManagedProject {
                id: "running".into(),
                port: None,
                phase: Phase::Running,
            },
        ];
        let view = TrayView::from_snapshot(&projects, &snapshot);
        assert_eq!(view.title, "2");
        assert_eq!(view.tooltip, "Bento · 2 个项目运行中");
        assert_eq!(view.items[0].text, "Bento 启动的项目 · 2 个运行中");
        assert_eq!(view.items[1].text, "正在运行");
        assert_eq!(view.items[2].text, "starting  :5301（启动中）");
        assert!(
            view.items
                .iter()
                .find(|item| item.id == STOP_ALL)
                .unwrap()
                .enabled
        );
        assert!(!view.items.iter().any(|item| item.text.contains("external")));
        assert_eq!(
            view.confirmation(),
            "将停止以下由 Bento 启动的项目：正在运行、starting。外部启动的进程不受影响。"
        );
        let orphan = TrayView::from_snapshot(&[], &snapshot);
        assert_eq!(orphan.projects.len(), 2);
    }
}
