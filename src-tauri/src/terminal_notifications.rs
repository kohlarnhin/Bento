use crate::terminal::{CompletionEvent, TerminalClosed, TerminalManager};
use crate::terminal_agent::AgentState;
use crate::terminal_attention::AgentEvent;
use crate::terminal_shell::elapsed_text;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, State};

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalTarget {
    project_id: String,
    slot: u8,
    session_id: String,
}

impl TerminalTarget {
    fn number(&self, terminals: &TerminalManager) -> Option<usize> {
        terminals.display_number(&self.project_id, self.slot, &self.session_id)
    }
}

#[derive(Default)]
pub struct PendingNotification(Mutex<Option<TerminalTarget>>);

#[tauri::command]
pub fn take_terminal_notification(
    pending: State<'_, PendingNotification>,
    terminals: State<'_, TerminalManager>,
) -> Option<TerminalTarget> {
    let target = pending.0.lock().ok()?.take()?;
    target.number(&terminals)?;
    Some(target)
}

fn reveal(app: &AppHandle, target: TerminalTarget) {
    if let Ok(mut pending) = app.state::<PendingNotification>().0.lock() {
        if target.number(&app.state::<TerminalManager>()).is_none() { return; }
        *pending = Some(target);
    }
    crate::reopen_main_window(app);
    let _ = app.emit_to("main", "terminal-notification-open", ());
}

pub fn remove(app: &AppHandle, closed: &TerminalClosed) {
    let target = TerminalTarget {
        project_id: closed.project_id.clone(), slot: closed.slot, session_id: closed.session_id.clone(),
    };
    if let Ok(mut pending) = app.state::<PendingNotification>().0.lock() {
        if pending.as_ref() == Some(&target) { pending.take(); }
    }
    let _ = app.run_on_main_thread(move || platform::remove(target));
}

pub fn post(app: &AppHandle, event: CompletionEvent) {
    let Some(completion) = event.completion else {
        return;
    };
    let Some(project_name) = project_name(&event.project_id) else {
        return;
    };
    let target = TerminalTarget { project_id: event.project_id, slot: event.slot, session_id: event.session_id };
    let Some(number) = target.number(&app.state::<TerminalManager>()) else { return; };
    let title = format!("{project_name} · 终端 {number}");
    let command = if completion.command.is_empty() {
        "命令"
    } else {
        &completion.command
    };
    let body = if completion.exit_code == 0 {
        format!(
            "{command} 已完成（{}）",
            elapsed_text(completion.elapsed_ms)
        )
    } else {
        format!("{command} 失败（退出码 {}）", completion.exit_code)
    };
    dispatch(app, target, completion.ended_at, title, body);
}

pub fn post_agent(app: &AppHandle, event: AgentEvent) {
    let Some(agent) = event.status.agent else { return; };
    let state = match agent.state {
        AgentState::Waiting => "等你确认",
        AgentState::Done => "已完成",
        AgentState::Thinking => return,
    };
    let Some(project_name) = project_name(&event.project_id) else { return; };
    let title = format!("{project_name} · {} {state}", agent.kind.label());
    let target = TerminalTarget { project_id: event.project_id, slot: event.slot, session_id: event.session_id };
    let Some(number) = target.number(&app.state::<TerminalManager>()) else { return; };
    let body = format!("终端 {number}");
    dispatch(app, target, agent.state_started_at, title, body);
}

fn project_name(project_id: &str) -> Option<String> {
    crate::get_projects_sync().ok()?.projects.into_iter().find(|project| project.id == project_id)
        .map(|project| project.display_name.filter(|name| !name.is_empty()).unwrap_or(project.name))
}

fn dispatch(app: &AppHandle, target: TerminalTarget, at: u64, title: String, body: String) {
    #[cfg(debug_assertions)]
    if crate::debug_e2e::enabled() {
        eprintln!("[Bento notification fixture] title={title:?} body={body:?} target={target:?}");
    }
    let main_app = app.clone();
    let _ = app.run_on_main_thread(move || {
        platform::post(&main_app, target, at, &title, &body);
    });
}

pub fn install(app: &AppHandle) {
    platform::install(app);
}

#[cfg(target_os = "macos")]
mod platform {
    use super::*;
    use block2::{DynBlock, RcBlock};
    use objc2::{
        define_class, msg_send,
        rc::Retained,
        runtime::{Bool, ProtocolObject},
        AnyThread, DefinedClass,
    };
    use objc2_foundation::{NSArray, NSBundle, NSError, NSObject, NSObjectProtocol, NSString};
    use objc2_user_notifications::{
        UNAuthorizationOptions, UNMutableNotificationContent,
        UNNotification, UNNotificationDefaultActionIdentifier, UNNotificationRequest, UNNotificationResponse,
        UNUserNotificationCenter, UNUserNotificationCenterDelegate,
    };

    const PREFIX: &str = "bento-terminal:";

    fn decode(identifier: &NSString) -> Option<TerminalTarget> {
        let identifier = identifier.to_string();
        let (_, json) = identifier.strip_prefix(PREFIX)?.split_once(':')?;
        serde_json::from_str(json).ok()
    }

    define_class!(
        #[unsafe(super = NSObject)]
        #[name = "BentoTerminalNotificationDelegate"]
        #[ivars = AppHandle]
        struct Delegate;

        unsafe impl NSObjectProtocol for Delegate {}

        unsafe impl UNUserNotificationCenterDelegate for Delegate {
            #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
            fn respond(
                &self,
                _: &UNUserNotificationCenter,
                response: &UNNotificationResponse,
                complete: &DynBlock<dyn Fn()>,
            ) {
                if response
                    .actionIdentifier()
                    .isEqualToString(unsafe { UNNotificationDefaultActionIdentifier })
                {
                    if let Some(target) = decode(&response.notification().request().identifier()) {
                        let app = self.ivars().clone();
                        let window = app.clone();
                        let _ = app.run_on_main_thread(move || reveal(&window, target));
                    }
                }
                complete.call(());
            }
        }
    );

    thread_local! {
        static DELEGATE: std::cell::OnceCell<Retained<Delegate>> = const { std::cell::OnceCell::new() };
    }

    fn bundled() -> bool {
        let bundle = NSBundle::mainBundle();
        bundle.bundleIdentifier().is_some()
            && bundle
                .bundleURL()
                .pathExtension()
                .is_some_and(|extension| extension.to_string() == "app")
    }

    pub fn install(app: &AppHandle) {
        if !bundled() {
            return;
        }
        DELEGATE.with(|delegate| {
            let retained = delegate.get_or_init(|| {
                let allocated = Delegate::alloc().set_ivars(app.clone());
                unsafe { msg_send![super(allocated), init] }
            });
            UNUserNotificationCenter::currentNotificationCenter()
                .setDelegate(Some(ProtocolObject::from_ref(&**retained)));
        });
    }

    pub fn post(app: &AppHandle, target: TerminalTarget, ended_at: u64, title: &str, body: &str) {
        if !bundled() {
            #[cfg(debug_assertions)]
            if crate::debug_e2e::enabled() {
                eprintln!("[Bento notification fixture] macOS delivery unavailable for unbundled tauri dev executable");
            }
            return;
        }
        install(app);
        let Ok(json) = serde_json::to_string(&target) else {
            return;
        };
        let identifier = format!("{PREFIX}{ended_at}:{json}");
        let (title, body) = (title.to_owned(), body.to_owned());
        let center = UNUserNotificationCenter::currentNotificationCenter();
        let app = app.clone();
        let handler = RcBlock::new(move |granted: Bool, _error: *mut NSError| {
            if !granted.as_bool() { return; }
            let (handle, target) = (app.clone(), target.clone());
            let (identifier, title, body) = (identifier.clone(), title.clone(), body.clone());
            let _ = app.run_on_main_thread(move || {
                if target.number(&handle.state::<TerminalManager>()).is_some() {
                    let content = UNMutableNotificationContent::new();
                    content.setTitle(&NSString::from_str(&title));
                    content.setBody(&NSString::from_str(&body));
                    let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
                        &NSString::from_str(&identifier), &content, None,
                    );
                    UNUserNotificationCenter::currentNotificationCenter()
                        .addNotificationRequest_withCompletionHandler(&request, None);
                }
            });
        });
        center.requestAuthorizationWithOptions_completionHandler(
            UNAuthorizationOptions::Alert,
            &handler,
        );
    }

    pub fn remove(target: TerminalTarget) {
        if !bundled() { return; }
        use std::ptr::NonNull;
        let center = UNUserNotificationCenter::currentNotificationCenter();
        let pending_target = target.clone();
        center.getPendingNotificationRequestsWithCompletionHandler(&RcBlock::new(move |requests: NonNull<NSArray<UNNotificationRequest>>| {
            let ids: Vec<_> = unsafe { requests.as_ref() }.to_vec().iter().map(|request| request.identifier())
                .filter(|id| decode(id).as_ref() == Some(&pending_target)).collect();
            if !ids.is_empty() {
                UNUserNotificationCenter::currentNotificationCenter()
                    .removePendingNotificationRequestsWithIdentifiers(&NSArray::from_retained_slice(&ids));
            }
        }));
        center.getDeliveredNotificationsWithCompletionHandler(&RcBlock::new(move |notifications: NonNull<NSArray<UNNotification>>| {
            let ids: Vec<_> = unsafe { notifications.as_ref() }.to_vec().iter().map(|notice| notice.request().identifier())
                .filter(|id| decode(id).as_ref() == Some(&target)).collect();
            if !ids.is_empty() {
                UNUserNotificationCenter::currentNotificationCenter()
                    .removeDeliveredNotificationsWithIdentifiers(&NSArray::from_retained_slice(&ids));
            }
        }));
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::*;
    pub fn install(_: &AppHandle) {}
    pub fn post(_: &AppHandle, _: TerminalTarget, _: u64, _: &str, _: &str) {}
    pub fn remove(_: TerminalTarget) {}
}
