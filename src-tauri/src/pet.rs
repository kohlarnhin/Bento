//! Bento desktop pet: plays on Bento's top edge (as a child window, so the two
//! move as one) while Bento is frontmost, runs to the bottom-right corner otherwise,
//! and reports project, terminal and AI CLI activity one line at a time.

use crate::lifecycle::{ManagedProject, Phase, ProcessMap, SHUTTING_DOWN};
use crate::terminal::{CompletionEvent, TerminalKey, TerminalManager};
use crate::terminal_agent::{AgentSession, AgentState};
use crate::ProjectConfig;
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::sync::{
    atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering},
    Mutex,
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu},
    AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};

/// The pet in the bottom-right corner, shown while Bento is not frontmost.
pub const PET: &str = "bento-pet";
/// The pet playing on Bento's top edge: a child window of Bento, so the two move as one.
pub const PET_DOCK: &str = "bento-pet-dock";
const LABELS: [&str; 2] = [PET_DOCK, PET];
const MENU_PREFIX: &str = "bento-pet:";

// Window layout in logical px; mirrored at the top of src/components/PetWindow.tsx.
const WIN_W: f64 = 320.0;
const WIN_H: f64 = 220.0;
/// The corner pet's window is taller (CORNER_H / CORNER_LEDGE in PetWindow.tsx): room above it
/// for its raised hands and the sword's hat, and below the feet for its little rug.
const CORNER_H: f64 = 260.0;
const CORNER_LEDGE: f64 = 190.0;
/// Where the character's feet rest inside the window when nothing pushes it.
const LEDGE: f64 = 150.0;
/// The lowest the ledge may sit when the window above Bento would leave the screen
/// (the character then sinks into Bento's top bar instead of being cut off).
const MIN_LEDGE: f64 = 66.0;
/// Distance from the window's right edge to the wall the character clings to.
const WALL: f64 = 20.0;
/// The character's horizontal centre inside the window.
const HOME_X: f64 = 240.0;
/// Keep the character this far from the main window's corners.
const EDGE_MARGIN: f64 = 56.0;
/// On the top edge the pet window spans Bento's whole width plus this much on each side
/// (DOCK_PAD in PetWindow.tsx). The character walks inside it (smooth, in the webview);
/// the window itself only follows Bento, as its child window.
const DOCK_PAD: f64 = 200.0;
/// Corner pet: its right side sits at this x in its window (CORNER_RIGHT in PetWindow.tsx),
/// CORNER_GAP logical px in from the screen's right edge.
const CORNER_RIGHT: f64 = 276.0;
const CORNER_GAP: f64 = 88.0;
/// Peeking from a side: room the character needs beside the window, the stretch
/// of the side it keeps to, and where the character sits in its window
/// (SIDE_Y in PetWindow.tsx).
const SIDE_ROOM: f64 = 64.0;
const SIDE_TOP: f64 = 40.0;
const SIDE_BOTTOM: f64 = 110.0;
const SIDE_Y: f64 = 18.0;
/// Hanging from the bottom edge: room it needs below the window, and where the
/// grip line sits in its window (HANG_Y in PetWindow.tsx).
const BOTTOM_ROOM: f64 = 90.0;
const HANG_Y: f64 = 10.0;

/// Bento losing focus for less than this keeps the pet on the window, so a
/// quick trip to another app does not make it jump back and forth.
const LEAVE_DELAY: Duration = Duration::from_millis(250);
const TICK: Duration = Duration::from_millis(50);
const LAYOUT_EVERY: u32 = 8;
/// Last seen frontmost state (0 unknown, 1 background, 2 front), checked every tick.
static LAST_FRONT: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
/// When the frontmost state last changed (ms), to keep re-placing while windows settle.
static FLIPPED_AT: AtomicU64 = AtomicU64::new(0);
const COMPLETION_TTL_MS: u64 = 3 * 60 * 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Dock {
    /// Playing on the main window's top edge.
    Top,
    /// Peeking out from behind the main window's left side.
    Left,
    /// Peeking out from behind the main window's right side.
    Right,
    /// Hanging from the main window's bottom edge.
    Bottom,
    /// Main window fullscreen: clinging to the inside of its right edge.
    Inside,
    /// Bento is not frontmost: waiting in the bottom-right corner of the screen.
    Corner,
}

impl Dock {
    fn as_str(self) -> &'static str {
        match self {
            Self::Top => "top",
            Self::Left => "left",
            Self::Right => "right",
            Self::Bottom => "bottom",
            Self::Inside => "inside",
            Self::Corner => "corner",
        }
    }
    fn on_window(self) -> bool {
        matches!(self, Self::Top | Self::Left | Self::Right | Self::Bottom | Self::Inside)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Place {
    dock: &'static str,
    /// This window's pet is the one on screen.
    active: bool,
    /// Feet line inside the pet window, logical px.
    ledge: f64,
}

#[derive(Clone, Debug)]
struct ShellDone {
    project_id: String,
    slot: u8,
    command: String,
    exit_code: i32,
    elapsed_ms: u64,
    at: u64,
}

pub struct PetState {
    enabled: AtomicBool,
    /// Settings: whether the pet shows on Bento's top edge, and whether it shows in the corner.
    top_shown: AtomicBool,
    corner_shown: AtomicBool,
    running: AtomicBool,
    dock: Mutex<Option<Dock>>,
    ledge: Mutex<f64>,
    /// The pet on Bento is the one shown (None until first layout).
    on_bento: Mutex<Option<bool>>,
    last_corner: Mutex<Option<(i32, i32)>>,
    /// Which pet windows are currently drawn (native alpha), by label.
    shown: Mutex<HashMap<String, bool>>,
    /// Which pet is being dragged.
    drag_label: Mutex<Option<String>>,
    /// Where the user dropped the corner pet, if they moved it.
    corner_custom: Mutex<Option<(i32, i32)>>,
    /// The app that was frontmost while Bento was not, and when that was last true.
    previous_pid: AtomicI64,
    inactive_at: AtomicU64,
    /// Clicking the corner pet briefly activates Bento; ignore that until this time.
    hold_until: AtomicU64,
    last_set: Mutex<Option<(i32, i32)>>,
    /// Logical width last given to the pet window on Bento.
    dock_width: Mutex<f64>,
    /// Corner pet being dragged: the pointer's offset from the window origin (physical px).
    corner_grab: Mutex<Option<(f64, f64)>>,
    inactive_since: Mutex<Option<Instant>>,
    regions: Mutex<HashMap<String, Vec<[f64; 4]>>>,
    gaze: Mutex<HashMap<String, (f64, f64)>>,
    /// Which side of the main window it plays on (Top, Left or Right).
    side: Mutex<Dock>,
    /// Where along that side the character is: 0 = left/top end, 1 = right/bottom end.
    edge: Mutex<f64>,
    walk_generation: AtomicU64,
    ignoring: Mutex<HashMap<String, bool>>,
    dragging: AtomicBool,
    quiet_until: AtomicU64,
    save_generation: AtomicU64,
    completions: Mutex<VecDeque<ShellDone>>,
    /// Opacity (percent) picked in the menu; the webview keeps it. The pet has one fixed size.
    opacity: std::sync::atomic::AtomicU8,
    /// The corner pet steps aside while another app is fullscreen or a meeting app is in front.
    tucked: AtomicBool,
}

impl Default for PetState {
    fn default() -> Self {
        Self {
            enabled: AtomicBool::new(false),
            top_shown: AtomicBool::new(true),
            corner_shown: AtomicBool::new(true),
            running: AtomicBool::new(false),
            dock: Mutex::new(None),
            ledge: Mutex::new(LEDGE),
            on_bento: Mutex::new(None),
            last_corner: Mutex::new(None),
            shown: Mutex::new(HashMap::new()),
            drag_label: Mutex::new(None),
            corner_custom: Mutex::new(None),
            previous_pid: AtomicI64::new(0),
            inactive_at: AtomicU64::new(0),
            hold_until: AtomicU64::new(0),
            last_set: Mutex::new(None),
            dock_width: Mutex::new(WIN_W),
            corner_grab: Mutex::new(None),
            inactive_since: Mutex::new(None),
            regions: Mutex::new(HashMap::new()),
            gaze: Mutex::new(HashMap::new()),
            side: Mutex::new(Dock::Top),
            edge: Mutex::new(0.8),
            walk_generation: AtomicU64::new(0),
            ignoring: Mutex::new(HashMap::new()),
            dragging: AtomicBool::new(false),
            quiet_until: AtomicU64::new(0),
            save_generation: AtomicU64::new(0),
            completions: Mutex::new(VecDeque::new()),
            opacity: std::sync::atomic::AtomicU8::new(100),
            tucked: AtomicBool::new(false),
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}

// ---------------------------------------------------------------------------
// Lifecycle
// ---------------------------------------------------------------------------

pub fn install(app: &AppHandle) {
    let mut enabled = true;
    if let Ok(database) = crate::open_project_db() {
        enabled = crate::settings::enabled(&database, crate::settings::SHOW_PET).unwrap_or(true);
        let state = app.state::<PetState>();
        state.top_shown.store(crate::settings::enabled(&database, crate::settings::SHOW_PET_TOP).unwrap_or(true), Ordering::SeqCst);
        state.corner_shown.store(crate::settings::enabled(&database, crate::settings::SHOW_PET_CORNER).unwrap_or(true), Ordering::SeqCst);
    }
    if enabled {
        if let Err(error) = set_enabled(app, true) {
            eprintln!("Bento pet: {error}");
        }
    }
}



/// Settings: show the pet on Bento's top edge and/or in the corner. A place switched off keeps its
/// window (so switching it back on is instant) but is never drawn and lets every click through.
pub fn set_places(app: &AppHandle, top: bool, corner: bool) {
    let state = app.state::<PetState>();
    state.top_shown.store(top, Ordering::SeqCst);
    state.corner_shown.store(corner, Ordering::SeqCst);
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || layout(&handle));
}

pub fn set_enabled(app: &AppHandle, enabled: bool) -> Result<(), String> {
    let state = app.state::<PetState>();
    state.enabled.store(enabled, Ordering::SeqCst);
    if !enabled {
        for label in LABELS {
            if let Some(window) = app.get_webview_window(label) {
                window.destroy().map_err(|error| error.to_string())?;
            }
        }
        *lock(&state.dock) = None;
        *lock(&state.on_bento) = None;
        *lock(&state.last_set) = None;
        *lock(&state.last_corner) = None;
        lock(&state.ignoring).clear();
        lock(&state.shown).clear();
        return Ok(());
    }
    for label in LABELS {
        if app.get_webview_window(label).is_some() {
            continue;
        }
        let window = WebviewWindowBuilder::new(app, label, WebviewUrl::App("index.html".into()))
            .title("Bento Pet")
            .inner_size(WIN_W, if label == PET { CORNER_H } else { WIN_H })
            .decorations(false)
            .transparent(true)
            .shadow(false)
            .resizable(false)
            .skip_taskbar(true)
            .always_on_top(label == PET)
            .visible_on_all_workspaces(label == PET)
            .accept_first_mouse(true)
            .focusable(false)
            .focused(false)
            .visible(false)
            // Keep its timers running at full speed while Bento is in front: a throttled
            // webview fell behind the native walk (stepping on the spot, then sliding while sitting).
            .background_throttling(tauri::utils::config::BackgroundThrottlingPolicy::Disabled)
            .build()
            .map_err(|error| error.to_string())?;
        let _ = window.set_size(LogicalSize::new(WIN_W, if label == PET { CORNER_H } else { WIN_H }));
        // Transparent pixels pass clicks through until the pointer reaches the character.
        let _ = window.set_ignore_cursor_events(true);
        lock(&state.ignoring).insert(label.to_string(), true);
    }
    *lock(&state.dock) = None;
    *lock(&state.on_bento) = None;
    *lock(&state.last_set) = None;
    *lock(&state.last_corner) = None;
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(window) = handle.get_webview_window(PET) {
            make_nonactivating(&window);
        }
        layout(&handle);
        if let Some(window) = handle.get_webview_window(PET) {
            let _ = window.show();
        }
    });
    start_loop(app);
    Ok(())
}

fn start_loop(app: &AppHandle) {
    let state = app.state::<PetState>();
    if state.running.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut tick = 0u32;
        loop {
            tokio::time::sleep(TICK).await;
            let state = app.state::<PetState>();
            if SHUTTING_DOWN.load(Ordering::SeqCst) || !state.enabled.load(Ordering::SeqCst) {
                state.running.store(false, Ordering::SeqCst);
                break;
            }
            tick = tick.wrapping_add(1);
            let handle = app.clone();
            let periodic = tick % LAYOUT_EVERY == 0;
            let _ = app.run_on_main_thread(move || {
                track_pointer(&handle);
                // Bento coming to the front (or leaving) is acted on at the
                // next tick, not the next periodic pass, so the pet on Bento
                // shows up as soon as the corner one goes.
                let front = match frontmost() { Some(true) => 2, Some(false) => 1, None => 0 };
                let flipped = LAST_FRONT.swap(front, Ordering::SeqCst) != front;
                // Right after Bento comes forward (or leaves), keep re-placing for a
                // moment: under Stage Manager the window is still sliding into place,
                // so one pass would read a stale position and leave the pet behind.
                if flipped {
                    FLIPPED_AT.store(now_ms(), Ordering::SeqCst);
                }
                let settling = now_ms().saturating_sub(FLIPPED_AT.load(Ordering::SeqCst)) < 1500;
                if periodic || flipped || settling {
                    layout(&handle);
                }
            });
        }
    });
}

// ---------------------------------------------------------------------------
// Placement
// ---------------------------------------------------------------------------

/// Re-evaluate where both pets belong and which one is shown. Cheap when nothing changed.
pub fn layout(app: &AppHandle) {
    let state = app.state::<PetState>();
    if !state.enabled.load(Ordering::SeqCst) {
        return;
    }
    let main = app.get_webview_window("main");
    let shown = main.as_ref().is_some_and(|main| {
        main.is_visible().unwrap_or(false) && !main.is_minimized().unwrap_or(false)
    });
    let wanted = choose_dock(app, &state);
    let on_bento = wanted.on_window() && shown;
    let previous = *lock(&state.on_bento);
    let top_on = state.top_shown.load(Ordering::SeqCst);
    let corner_on = state.corner_shown.load(Ordering::SeqCst);
    // Each time it comes back onto Bento it picks a new spot to play.
    if on_bento && previous == Some(false) {
        *lock(&state.edge) = random_edge();
        *lock(&state.side) = random_side();
        state.walk_generation.fetch_add(1, Ordering::SeqCst);
    }

    // The pet on Bento: kept in place even while hidden, so showing it is instant.
    if let (Some(dock_pet), Some(main), true) = (app.get_webview_window(PET_DOCK), main.as_ref(), shown) {
        attach(main, &dock_pet);
        // Under Stage Manager (台前调度) the window's sides sit against the
        // strip and its own transform, so the pet only uses the top edge.
        let dock = if wanted == Dock::Inside {
            Dock::Inside
        } else if stage_manager() {
            Dock::Top
        } else {
            *lock(&state.side)
        };
        let dragging_dock = state.dragging.load(Ordering::SeqCst) && lock(&state.drag_label).as_deref() != Some(PET);
        if !dragging_dock {
            if let Some((dock, at, ledge, width)) = window_position(main, dock, *lock(&state.edge)) {
                let resized = {
                    let mut current = lock(&state.dock_width);
                    let changed = (*current - width).abs() > 0.5;
                    *current = width;
                    changed
                };
                if resized {
                    let _ = dock_pet.set_size(LogicalSize::new(width, WIN_H));
                }
                // The pet window is Bento's child window: AppKit carries it along while Bento
                // is dragged. Moving it ourselves from a position read mid-drag is what made it
                // fall behind and catch up, so while a mouse button is down (Bento being dragged)
                // it is left alone unless Bento's size changed.
                let attached = is_attached(&dock_pet);
                if resized || !mouse_pressed() || !attached {
                    let mut last = lock(&state.last_set);
                    if *last != Some(at) || resized {
                        *last = Some(at);
                        drop(last);
                        let _ = dock_pet.set_position(PhysicalPosition::new(at.0, at.1));
                    }
                }
                let changed_dock = lock(&state.dock).replace(dock) != Some(dock);
                let changed_ledge = {
                    let mut current = lock(&state.ledge);
                    let changed = (*current - ledge).abs() > 0.5;
                    *current = ledge;
                    changed
                };
                if changed_dock || changed_ledge || previous != Some(on_bento) {
                    let _ = app.emit_to(PET_DOCK, "pet-place", Place { dock: dock.as_str(), active: on_bento && top_on, ledge });
                }
            }
        }
    }

    // The pet in the corner.
    if let Some(corner) = app.get_webview_window(PET) {
        let dragging_corner = state.dragging.load(Ordering::SeqCst) && lock(&state.drag_label).as_deref() == Some(PET);
        let custom = *lock(&state.corner_custom);
        if let Some(at) = custom.or_else(|| corner_position(main.as_ref(), &corner)).filter(|_| !dragging_corner) {
            let mut last = lock(&state.last_corner);
            if *last != Some(at) {
                *last = Some(at);
                drop(last);
                let _ = corner.set_position(PhysicalPosition::new(at.0, at.1));
            }
        }
        if previous != Some(on_bento) {
            let _ = app.emit_to(PET, "pet-place", Place { dock: "corner", active: !on_bento && corner_on, ledge: CORNER_LEDGE });
        }
        let tucked = !on_bento && should_tuck();
        if state.tucked.swap(tucked, Ordering::SeqCst) != tucked {
            let _ = app.emit_to(PET, "pet-tuck", tucked);
        }
    }
    *lock(&state.on_bento) = Some(on_bento);

    // Only one pet is ever drawn. Enforced natively on every pass (not only on
    // change), so a missed event in the webview can never leave both on screen.
    for (label, visible) in [(PET_DOCK, on_bento && top_on), (PET, !on_bento && corner_on)] {
        let Some(window) = app.get_webview_window(label) else { continue; };
        let changed = lock(&state.shown).insert(label.to_string(), visible) != Some(visible);
        set_drawn(&window, visible);
        if changed {
            let place = if label == PET {
                Place { dock: "corner", active: visible, ledge: CORNER_LEDGE }
            } else {
                Place { dock: lock(&state.dock).map(Dock::as_str).unwrap_or("top"), active: visible, ledge: *lock(&state.ledge) }
            };
            let _ = app.emit_to(label, "pet-place", place);
        }
    }
}

/// Turn the corner pet into a non-activating panel: clicking it (to open its
/// menu) then never activates Bento, so Bento does not flash to the front.
/// Only a double-click brings Bento forward, on purpose.
#[cfg(target_os = "macos")]
fn make_nonactivating(window: &WebviewWindow) {
    use objc2::runtime::{AnyClass, AnyObject, Bool, ClassBuilder, Sel};
    use objc2::{class, sel};
    use objc2_app_kit::{NSWindow, NSWindowStyleMask};
    use std::sync::OnceLock;
    static PANEL: OnceLock<usize> = OnceLock::new();

    if objc2::MainThreadMarker::new().is_none() {
        return;
    }
    let Ok(ns) = window.ns_window() else { return; };
    if ns.is_null() {
        return;
    }
    extern "C-unwind" fn never(_this: &AnyObject, _sel: Sel) -> Bool {
        Bool::NO
    }
    let panel = *PANEL.get_or_init(|| {
        let class: &'static AnyClass = AnyClass::get(c"BentoPetPanel").unwrap_or_else(|| {
            let mut builder = ClassBuilder::new(c"BentoPetPanel", class!(NSPanel)).expect("BentoPetPanel");
            // SAFETY: the signatures match -canBecomeKeyWindow / -canBecomeMainWindow.
            unsafe {
                builder.add_method(sel!(canBecomeKeyWindow), never as extern "C-unwind" fn(_, _) -> _);
                builder.add_method(sel!(canBecomeMainWindow), never as extern "C-unwind" fn(_, _) -> _);
            }
            builder.register()
        });
        class as *const AnyClass as usize
    });
    // SAFETY: a live NSWindow owned by Tauri on the main thread; NSPanel is an
    // NSWindow subclass, the same swap tauri-nspanel performs.
    unsafe {
        objc2::ffi::object_setClass(ns as *mut AnyObject, panel as *const AnyClass);
        let ns = &*(ns as *const NSWindow);
        ns.setStyleMask(ns.styleMask() | NSWindowStyleMask::NonactivatingPanel);
        ns.setHidesOnDeactivate(false);
    }
}

#[cfg(not(target_os = "macos"))]
fn make_nonactivating(_window: &WebviewWindow) {}

#[cfg(target_os = "macos")]
fn set_drawn(window: &WebviewWindow, visible: bool) {
    use objc2_app_kit::NSWindow;
    if objc2::MainThreadMarker::new().is_none() {
        return;
    }
    let Ok(ns) = window.ns_window() else { return; };
    if ns.is_null() {
        return;
    }
    // SAFETY: a live NSWindow owned by Tauri, used on the main thread.
    let ns = unsafe { &*(ns as *const NSWindow) };
    let alpha = if visible { 1.0 } else { 0.0 };
    if (ns.alphaValue() - alpha).abs() > 0.01 {
        ns.setAlphaValue(alpha);
    }
}

#[cfg(not(target_os = "macos"))]
fn set_drawn(_window: &WebviewWindow, _visible: bool) {}

/// On Bento the pet only lives on the top edge (no sides, no bottom).
fn random_side() -> Dock {
    Dock::Top
}

fn random_edge() -> f64 {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().subsec_nanos();
    0.08 + f64::from(nanos % 1000) / 1000.0 * 0.84
}

struct Area {
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
}

fn area_of(monitor: &tauri::Monitor) -> Area {
    let work = monitor.work_area();
    let left = f64::from(work.position.x);
    let top = f64::from(work.position.y);
    Area { left, top, right: left + f64::from(work.size.width), bottom: top + f64::from(work.size.height) }
}

/// Apps whose being in front means a call or a meeting.
const MEETING_APPS: [&str; 8] = [
    "us.zoom.xos", "com.microsoft.teams", "com.microsoft.teams2", "com.tencent.meeting",
    "com.apple.FaceTime", "com.cisco.webexmeetingsapp", "Cisco-Systems.Spark", "com.tencent.wemeetapp",
];

/// Another app is fullscreen (or presenting with menu bar and Dock hidden), or a meeting app is in front.
#[cfg(target_os = "macos")]
fn should_tuck() -> bool {
    use objc2_app_kit::{NSApplication, NSApplicationPresentationOptions as Options, NSWorkspace};
    let Some(marker) = objc2::MainThreadMarker::new() else { return false; };
    let options = NSApplication::sharedApplication(marker).currentSystemPresentationOptions();
    let has = |flag: Options| (options & flag) == flag;
    if has(Options::FullScreen) || (has(Options::HideMenuBar) && has(Options::HideDock)) {
        return true;
    }
    NSWorkspace::sharedWorkspace().frontmostApplication()
        .and_then(|front| front.bundleIdentifier())
        .is_some_and(|id| MEETING_APPS.contains(&id.to_string().as_str()))
}

#[cfg(not(target_os = "macos"))]
fn should_tuck() -> bool {
    false
}

#[cfg(target_os = "macos")]
fn frontmost() -> Option<bool> {
    let marker = objc2::MainThreadMarker::new()?;
    Some(objc2_app_kit::NSApplication::sharedApplication(marker).isActive())
}

/// Stage Manager (台前调度) is on. Read at most every two seconds.
#[cfg(target_os = "macos")]
fn stage_manager() -> bool {
    use std::sync::atomic::AtomicU8;
    static CACHED: AtomicU8 = AtomicU8::new(2);
    static READ_AT: AtomicU64 = AtomicU64::new(0);
    let now = now_ms();
    let cached = CACHED.load(Ordering::Relaxed);
    if cached != 2 && now.saturating_sub(READ_AT.load(Ordering::Relaxed)) < 2000 {
        return cached == 1;
    }
    let on = {
        use objc2::AnyThread;
        use objc2_foundation::{NSString, NSUserDefaults};
        let suite = NSString::from_str("com.apple.WindowManager");
        #[allow(unused_unsafe)]
        let defaults = unsafe { NSUserDefaults::initWithSuiteName(NSUserDefaults::alloc(), Some(&suite)) };
        defaults
            .is_some_and(|defaults| defaults.boolForKey(&NSString::from_str("GloballyEnabled")))
    };
    CACHED.store(u8::from(on), Ordering::Relaxed);
    READ_AT.store(now, Ordering::Relaxed);
    on
}

#[cfg(not(target_os = "macos"))]
fn stage_manager() -> bool {
    false
}

/// The topmost ordinary (layer 0) window on screen belongs to Bento. Read at most every 200 ms.
#[cfg(target_os = "macos")]
fn bento_on_top() -> bool {
    use std::ffi::c_void;
    use std::sync::atomic::AtomicU8;
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGWindowListCopyWindowInfo(option: u32, relative_to: u32) -> *const c_void;
        static kCGWindowLayer: *const c_void;
        static kCGWindowOwnerPID: *const c_void;
        static kCGWindowAlpha: *const c_void;
        static kCGWindowBounds: *const c_void;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFArrayGetCount(array: *const c_void) -> isize;
        fn CFArrayGetValueAtIndex(array: *const c_void, index: isize) -> *const c_void;
        fn CFDictionaryGetValue(dict: *const c_void, key: *const c_void) -> *const c_void;
        fn CFNumberGetValue(number: *const c_void, kind: isize, value: *mut c_void) -> u8;
        fn CFRelease(object: *const c_void);
    }
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGRectMakeWithDictionaryRepresentation(dict: *const c_void, rect: *mut [f64; 4]) -> u8;
    }
    static CACHED: AtomicU8 = AtomicU8::new(2);
    static READ_AT: AtomicU64 = AtomicU64::new(0);
    let now = now_ms();
    let cached = CACHED.load(Ordering::Relaxed);
    if cached != 2 && now.saturating_sub(READ_AT.load(Ordering::Relaxed)) < 200 {
        return cached == 1;
    }
    let me = std::process::id() as i64;
    // kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements, front to back.
    let on_top = unsafe {
        let list = CGWindowListCopyWindowInfo(1 | 1 << 4, 0);
        let mut found = None;
        if !list.is_null() {
            for index in 0..CFArrayGetCount(list) {
                let info = CFArrayGetValueAtIndex(list, index);
                let int = |key: *const c_void| {
                    let value = CFDictionaryGetValue(info, key);
                    let mut out: i64 = 0;
                    (!value.is_null() && CFNumberGetValue(value, 4 /* SInt64 */, &mut out as *mut i64 as *mut c_void) != 0).then_some(out)
                };
                if int(kCGWindowLayer) != Some(0) {
                    continue;
                }
                let alpha = {
                    let value = CFDictionaryGetValue(info, kCGWindowAlpha);
                    let mut out: f64 = 1.0;
                    if !value.is_null() {
                        CFNumberGetValue(value, 6 /* Float64 */, &mut out as *mut f64 as *mut c_void);
                    }
                    out
                };
                if alpha < 0.05 {
                    continue;
                }
                // Skip slivers (a pet window, a hairline panel): a real window is at least 200x120 pt.
                let bounds = CFDictionaryGetValue(info, kCGWindowBounds);
                let mut rect = [0.0f64; 4];
                if bounds.is_null() || CGRectMakeWithDictionaryRepresentation(bounds, &mut rect) == 0 || rect[2] < 200.0 || rect[3] < 120.0 {
                    continue;
                }
                found = int(kCGWindowOwnerPID).map(|pid| pid == me);
                break;
            }
            CFRelease(list);
        }
        found.unwrap_or(false)
    };
    CACHED.store(u8::from(on_top), Ordering::Relaxed);
    READ_AT.store(now, Ordering::Relaxed);
    on_top
}

#[cfg(not(target_os = "macos"))]
fn bento_on_top() -> bool {
    false
}

#[cfg(not(target_os = "macos"))]
fn frontmost() -> Option<bool> {
    None
}

/// Bento is the frontmost app. Clicking the pet (which never takes focus)
/// or opening the tray panel must not count as leaving.
fn app_active(app: &AppHandle, main: &WebviewWindow) -> bool {
    if let Some(active) = frontmost() {
        // Under Stage Manager Bento can be the window in the middle of the stage without being
        // the active app (opened from the pet, the tray, or as the stage settles). It counts as
        // in front when its window is the topmost ordinary window on screen.
        return active || (stage_manager() && bento_on_top());
    }
    main.is_focused().unwrap_or(false)
        || [PET, PET_DOCK, crate::tray::PANEL].iter().any(|label| {
            app.get_webview_window(label).is_some_and(|window| window.is_focused().unwrap_or(false))
        })
}

/// Fully automatic: on Bento while it is frontmost, in the corner otherwise.
fn choose_dock(app: &AppHandle, state: &PetState) -> Dock {
    let main = app.get_webview_window("main");
    let shown = main.as_ref().is_some_and(|main| {
        main.is_visible().unwrap_or(false) && !main.is_minimized().unwrap_or(false)
    });
    // A press on the corner pet activates Bento before the click reaches the
    // page; that is not the user coming back to Bento.
    let pressing_corner = mouse_pressed()
        && lock(&state.on_bento).is_some_and(|on| !on)
        && lock(&state.ignoring).get(PET).is_some_and(|ignoring| !ignoring);
    let held = pressing_corner || state.hold_until.load(Ordering::SeqCst) > now_ms();
    let active = !held && shown && main.as_ref().is_some_and(|main| app_active(app, main));
    let mut since = lock(&state.inactive_since);
    let was_on_window = lock(&state.dock).is_some_and(Dock::on_window);
    let on_window = if active {
        *since = None;
        true
    } else if shown && was_on_window {
        let started = *since.get_or_insert_with(Instant::now);
        started.elapsed() < LEAVE_DELAY
    } else {
        since.get_or_insert_with(Instant::now);
        false
    };
    if !on_window {
        return Dock::Corner;
    }
    if main.as_ref().is_some_and(|main| main.is_fullscreen().unwrap_or(false)) { Dock::Inside } else { Dock::Top }
}

fn window_position(main: &WebviewWindow, dock: Dock, edge: f64) -> Option<(Dock, (i32, i32), f64, f64)> {
    let s = main.scale_factor().ok()?;
    let position = main.outer_position().ok()?;
    let size = main.outer_size().ok()?;
    let monitor = main.current_monitor().ok()??;
    let area = area_of(&monitor);
    let screen_top = f64::from(monitor.position().y);
    let left = f64::from(position.x);
    let top = f64::from(position.y);
    let right = left + f64::from(size.width);
    let height = f64::from(size.height);
    let at = |x: f64, y: f64| (x.round() as i32, y.round() as i32);
    if dock == Dock::Inside {
        let y = top + (height * 0.45).clamp(60.0 * s, (height - WIN_H * s).max(60.0 * s));
        return Some((Dock::Inside, at(right - (WIN_W - WALL) * s, y), LEDGE, WIN_W));
    }
    // On a side, peeking out from behind the frame; with no room beside the
    // window (it reaches the screen edge) it plays on the top edge instead.
    if matches!(dock, Dock::Left | Dock::Right) {
        let room = SIDE_ROOM * s;
        let fits = if dock == Dock::Left { left - area.left >= room } else { area.right - right >= room };
        let span = height - (SIDE_TOP + SIDE_BOTTOM) * s;
        if fits && span > 0.0 {
            let y = top + SIDE_TOP * s + span * edge.clamp(0.0, 1.0) - SIDE_Y * s;
            let x = if dock == Dock::Left { left - (WIN_W - WALL) * s } else { right - WALL * s };
            return Some((dock, at(x, y), LEDGE, WIN_W));
        }
    }
    // Hanging under the bottom edge, when there is room below the window.
    let span = (right - left - 2.0 * EDGE_MARGIN * s).max(0.0);
    if dock == Dock::Bottom && area.bottom - (top + height) >= BOTTOM_ROOM * s {
        let x = left + EDGE_MARGIN * s + span * edge.clamp(0.0, 1.0) - HOME_X * s;
        return Some((Dock::Bottom, at(x, top + height - HANG_Y * s), LEDGE, WIN_W));
    }
    // On the top edge: one window spanning Bento's width (plus DOCK_PAD each side), so the
    // character walks inside it and the window only ever moves with Bento. When there is no
    // room above the window (it sits under the menu bar), the pet window starts at the top of
    // the screen and the feet line moves up; past MIN_LEDGE it sinks into the top bar.
    let width = f64::from(size.width) / s + 2.0 * DOCK_PAD;
    let ceiling = area.top.max(screen_top);
    let ledge = ((top - ceiling) / s).clamp(MIN_LEDGE, LEDGE);
    let y = top - ledge * s;
    Some((Dock::Top, at(left - DOCK_PAD * s, y), ledge, width))
}

fn corner_position(main: Option<&WebviewWindow>, pet: &WebviewWindow) -> Option<(i32, i32)> {
    let monitor = main
        .and_then(|main| main.current_monitor().ok().flatten())
        .or_else(|| pet.primary_monitor().ok().flatten())
        .or_else(|| pet.current_monitor().ok().flatten())?;
    let s = monitor.scale_factor();
    let area = area_of(&monitor);
    // Keep a little air between the pet and the screen's right edge.
    let x = area.right - CORNER_GAP * s - CORNER_RIGHT * s;
    let y = area.bottom - 4.0 * s - CORNER_LEDGE * s;
    Some((x.round() as i32, y.round() as i32))
}

/// The pet on Bento is a child window of the main window, so AppKit moves the
/// two together with no lag when Bento is dragged, and it stacks with Bento
/// instead of floating over other apps. Re-attached if AppKit dropped it
/// (only while Bento is on screen, since attaching orders the child in).
#[cfg(target_os = "macos")]
fn attach(main: &WebviewWindow, pet: &WebviewWindow) {
    use objc2_app_kit::{NSWindow, NSWindowOrderingMode};
    if objc2::MainThreadMarker::new().is_none() {
        return;
    }
    let (Ok(parent), Ok(child)) = (main.ns_window(), pet.ns_window()) else { return; };
    if parent.is_null() || child.is_null() {
        return;
    }
    // SAFETY: both pointers are live NSWindows owned by Tauri, used on the main thread.
    let (parent, child) = unsafe { (&*(parent as *const NSWindow), &*(child as *const NSWindow)) };
    // The window is created hidden; addChildWindow alone does not order it in.
    if !child.isVisible() && parent.isVisible() {
        child.orderFrontRegardless();
    }
    if child.parentWindow().is_none() {
        unsafe { parent.addChildWindow_ordered(child, NSWindowOrderingMode::Above) };
    }
}

#[cfg(not(target_os = "macos"))]
fn attach(_main: &WebviewWindow, pet: &WebviewWindow) {
    let _ = pet.show();
}

/// The pet window on Bento is currently Bento's child window (AppKit moves it along).
#[cfg(target_os = "macos")]
fn is_attached(pet: &WebviewWindow) -> bool {
    use objc2_app_kit::NSWindow;
    if objc2::MainThreadMarker::new().is_none() {
        return false;
    }
    let Ok(child) = pet.ns_window() else { return false; };
    if child.is_null() {
        return false;
    }
    // SAFETY: a live NSWindow owned by Tauri, used on the main thread.
    let child = unsafe { &*(child as *const NSWindow) };
    child.parentWindow().is_some()
}

#[cfg(not(target_os = "macos"))]
fn is_attached(_pet: &WebviewWindow) -> bool {
    false
}

/// The pet window moved. Our own moves are recorded in `last_set`; we only
/// care about moves while the user drags it.
pub fn on_moved(app: &AppHandle, position: PhysicalPosition<i32>) {
    let state = app.state::<PetState>();
    if state.dragging.load(Ordering::SeqCst) {
        *lock(&state.last_set) = Some((position.x, position.y));
    }
}

/// Dropped: if it lands over Bento's top edge it stays at that spot (where the
/// pointer let go); anywhere else it hops back to where it was.
fn dropped(app: &AppHandle, _pet: &WebviewWindow) {
    let state = app.state::<PetState>();
    if let Some(main) = app.get_webview_window("main") {
        let spot = (|| {
            let s = main.scale_factor().ok()?;
            let cursor = app.cursor_position().ok()?;
            let position = main.outer_position().ok()?;
            let size = main.outer_size().ok()?;
            let left = f64::from(position.x);
            let width = f64::from(size.width);
            let span = width - 2.0 * EDGE_MARGIN * s;
            (cursor.x > left && cursor.x < left + width && span > 0.0)
                .then(|| ((cursor.x - left - EDGE_MARGIN * s) / span).clamp(0.0, 1.0))
        })();
        if let Some(edge) = spot {
            *lock(&state.edge) = edge;
            *lock(&state.side) = Dock::Top;
        }
    }
    *lock(&state.last_set) = None;
    layout(app);
    let _ = app.emit_to(PET_DOCK, "pet-edge", *lock(&state.edge));
}

// ---------------------------------------------------------------------------
// Pointer: click-through outside the character and its bubble
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
fn mouse_pressed() -> bool {
    objc2_app_kit::NSEvent::pressedMouseButtons() != 0
}

#[cfg(not(target_os = "macos"))]
fn mouse_pressed() -> bool {
    false
}

fn track_pointer(app: &AppHandle) {
    let state = app.state::<PetState>();
    if state.dragging.load(Ordering::SeqCst) && !mouse_pressed() {
        state.dragging.store(false, Ordering::SeqCst);
        let label = lock(&state.drag_label).take();
        *lock(&state.corner_grab) = None;
        if label.as_deref() == Some(PET) {
            // The corner pet stays where it was dropped.
            if let Some(at) = app.get_webview_window(PET).and_then(|pet| pet.outer_position().ok()) {
                *lock(&state.corner_custom) = Some((at.x, at.y));
                *lock(&state.last_corner) = Some((at.x, at.y));
            }
        } else if let Some(pet) = app.get_webview_window(PET_DOCK) {
            dropped(app, &pet);
        }
    }
    remember_previous_app(&state);
    let dragging = state.dragging.load(Ordering::SeqCst);
    let drag_label = lock(&state.drag_label).clone();
    let on_bento = lock(&state.on_bento).unwrap_or(false);
    for label in LABELS {
        let Some(pet) = app.get_webview_window(label) else { continue; };
        let place_on = if label == PET_DOCK { state.top_shown.load(Ordering::SeqCst) } else { state.corner_shown.load(Ordering::SeqCst) };
        let active = (label == PET_DOCK) == on_bento && place_on;
        let pointer = (|| {
            let cursor = app.cursor_position().ok()?;
            let origin = pet.outer_position().ok()?;
            let s = pet.scale_factor().ok()?;
            Some(((cursor.x - f64::from(origin.x)) / s, (cursor.y - f64::from(origin.y)) / s))
        })();
        // Let the pet's eyes follow the pointer, in window coordinates.
        if let (Some((x, y)), true) = (pointer, active) {
            let mut gaze = lock(&state.gaze);
            let last = gaze.entry(label.to_string()).or_insert((f64::NAN, f64::NAN));
            if last.0.is_nan() || (last.0 - x).abs() + (last.1 - y).abs() > 3.0 {
                *last = (x, y);
                drop(gaze);
                let _ = app.emit_to(label, "pet-gaze", (x.round(), y.round()));
            }
        }
        let tucked = label == PET && state.tucked.load(Ordering::SeqCst);
        // While it is being dragged the window must keep taking the mouse, wherever the pointer is.
        let held = dragging && drag_label.as_deref().map_or(label == PET_DOCK, |dragged| dragged == label);
        let inside = active && !tucked && (held || pointer.is_some_and(|(x, y)| {
            lock(&state.regions).get(label).is_some_and(|regions| regions.iter().any(|&[left, top, width, height]| {
                x >= left && x <= left + width && y >= top && y <= top + height
            }))
        }));
        let ignore = !inside;
        let changed = lock(&state.ignoring).insert(label.to_string(), ignore) != Some(ignore);
        if changed {
            let _ = pet.set_ignore_cursor_events(ignore);
        }
    }
}

#[tauri::command]
pub fn pet_set_regions(app: AppHandle, window: WebviewWindow, regions: Vec<[f64; 4]>) {
    lock(&app.state::<PetState>().regions).insert(window.label().to_string(), regions.into_iter().take(6).collect());
}

/// Where along the top edge the character is now (0 = left end, 1 = right end).
/// The character walks inside its window (PetWindow.tsx); it reports where it got to.
#[tauri::command]
pub fn pet_set_edge(app: AppHandle, edge: f64) {
    if edge.is_finite() {
        *lock(&app.state::<PetState>().edge) = edge.clamp(0.0, 1.0);
    }
}

/// Stop walking where the character is.
#[tauri::command]
pub fn pet_stop(app: AppHandle) {
    app.state::<PetState>().walk_generation.fetch_add(1, Ordering::SeqCst);
}

/// Reappear somewhere else around Bento at once (the character fades out
/// first and back in after): another spot on the top edge or on a side.
/// `side` ("top", "left", "right") picks the side; otherwise it is random.
/// Returns false when it is not playing on Bento's outside.
#[tauri::command]
pub fn pet_jump(app: AppHandle, to: Option<f64>, side: Option<String>) -> bool {
    let state = app.state::<PetState>();
    let current = *lock(&state.dock);
    if !matches!(current, Some(Dock::Top | Dock::Left | Dock::Right | Dock::Bottom)) || state.dragging.load(Ordering::SeqCst)
        || !lock(&state.on_bento).unwrap_or(false)
    {
        return false;
    }
    state.walk_generation.fetch_add(1, Ordering::SeqCst);
    // Only the top edge: the sides and the bottom are not used any more.
    let _ = side;
    let next = Dock::Top;
    let from = *lock(&state.edge);
    let mut to = to.unwrap_or_else(random_edge).clamp(0.0, 1.0);
    if Some(next) == current && (to - from).abs() < 0.2 {
        to = if from > 0.5 { (from - 0.35).max(0.05) } else { (from + 0.35).min(0.95) };
    }
    *lock(&state.side) = next;
    *lock(&state.edge) = to;
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || layout(&handle));
    let _ = app.emit_to(PET_DOCK, "pet-edge", to);
    true
}

#[derive(Clone, Copy, Serialize)]
struct Style {
    opacity: u8,
}

fn style_of(state: &PetState) -> Style {
    Style { opacity: state.opacity.load(Ordering::SeqCst) }
}

/// The webview remembers the opacity picked in the menu and reports it on start.
#[tauri::command]
pub fn pet_style_init(app: AppHandle, opacity: u8) {
    let state = app.state::<PetState>();
    state.opacity.store(opacity.clamp(30, 100), Ordering::SeqCst);
}

/// Whether the corner pet is stepping aside right now (asked once on start).
#[tauri::command]
pub fn pet_tucked(app: AppHandle) -> bool {
    app.state::<PetState>().tucked.load(Ordering::SeqCst)
}

#[tauri::command]
pub fn pet_edge(app: AppHandle) -> f64 {
    *lock(&app.state::<PetState>().edge)
}

#[tauri::command]
pub fn pet_dock(app: AppHandle, window: WebviewWindow) -> Place {
    let state = app.state::<PetState>();
    let on_bento = lock(&state.on_bento).unwrap_or(false);
    if window.label() == PET_DOCK {
        let dock = *lock(&state.dock);
        let ledge = *lock(&state.ledge);
        Place { dock: dock.map(Dock::as_str).unwrap_or("top"), active: on_bento && state.top_shown.load(Ordering::SeqCst), ledge }
    } else {
        Place { dock: "corner", active: !on_bento && state.corner_shown.load(Ordering::SeqCst), ledge: CORNER_LEDGE }
    }
}

/// The corner pet was pressed. On macOS that activates Bento, which would bring
/// it to the front and swap the pets; only a double-click should do that, so
/// hand the focus straight back to the app that had it.
#[tauri::command]
pub fn pet_press(app: AppHandle, window: WebviewWindow) {
    let state = app.state::<PetState>();
    if window.label() != PET || now_ms().saturating_sub(state.inactive_at.load(Ordering::SeqCst)) > 1500 {
        return;
    }
    state.hold_until.store(now_ms() + 1500, Ordering::SeqCst);
    let pid = state.previous_pid.load(Ordering::SeqCst);
    let _ = app.run_on_main_thread(move || {
        if frontmost() == Some(true) {
            give_back_focus(pid);
        }
    });
}

#[cfg(target_os = "macos")]
fn remember_previous_app(state: &PetState) {
    use objc2_app_kit::{NSRunningApplication, NSWorkspace};
    if frontmost() != Some(false) {
        return;
    }
    state.inactive_at.store(now_ms(), Ordering::SeqCst);
    let me = NSRunningApplication::currentApplication().processIdentifier();
    if let Some(front) = NSWorkspace::sharedWorkspace().frontmostApplication() {
        let pid = front.processIdentifier();
        if pid > 0 && pid != me {
            state.previous_pid.store(i64::from(pid), Ordering::SeqCst);
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn remember_previous_app(_state: &PetState) {}

#[cfg(target_os = "macos")]
fn give_back_focus(pid: i64) {
    use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication};
    let Ok(pid) = i32::try_from(pid) else { return; };
    if pid <= 0 {
        return;
    }
    if let Some(previous) = NSRunningApplication::runningApplicationWithProcessIdentifier(pid) {
        previous.activateWithOptions(NSApplicationActivationOptions::ActivateAllWindows);
    }
}

#[cfg(not(target_os = "macos"))]
fn give_back_focus(_pid: i64) {}

fn activate_app(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    {
        let handle = app.clone();
        let _ = app.run_on_main_thread(move || {
            if let Some(marker) = objc2::MainThreadMarker::new() {
                #[allow(deprecated)]
                objc2_app_kit::NSApplication::sharedApplication(marker).activateIgnoringOtherApps(true);
            }
            FLIPPED_AT.store(now_ms(), Ordering::SeqCst);
            layout(&handle);
        });
    }
    #[cfg(not(target_os = "macos"))]
    let _ = app;
}

#[tauri::command]
pub fn pet_drag_start(app: AppHandle, window: WebviewWindow) {
    let state = app.state::<PetState>();
    *lock(&state.drag_label) = Some(window.label().to_string());
    state.walk_generation.fetch_add(1, Ordering::SeqCst);
    state.dragging.store(true, Ordering::SeqCst);
    if window.label() != PET {
        return;
    }
    // The corner pet is a non-activating panel and pressing it hands the focus back to the
    // other app, so AppKit's window drag does not take hold there. It follows the pointer
    // here instead, until the button is let go (track_pointer ends the drag).
    let grab = (|| {
        let cursor = app.cursor_position().ok()?;
        let origin = window.outer_position().ok()?;
        Some((cursor.x - f64::from(origin.x), cursor.y - f64::from(origin.y)))
    })();
    let Some(grab) = grab else { return; };
    let first = lock(&state.corner_grab).replace(grab).is_none();
    if !first {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(8)).await;
            let state = app.state::<PetState>();
            let Some((dx, dy)) = *lock(&state.corner_grab) else { break; };
            if !state.dragging.load(Ordering::SeqCst) {
                *lock(&state.corner_grab) = None;
                break;
            }
            let handle = app.clone();
            let _ = app.run_on_main_thread(move || {
                if let (Some(pet), Ok(cursor)) = (handle.get_webview_window(PET), handle.cursor_position()) {
                    let at = ((cursor.x - dx).round() as i32, (cursor.y - dy).round() as i32);
                    let state = handle.state::<PetState>();
                    let mut last = lock(&state.last_corner);
                    if *last != Some(at) {
                        *last = Some(at);
                        drop(last);
                        let _ = pet.set_position(PhysicalPosition::new(at.0, at.1));
                    }
                }
            });
        }
    });
}

// ---------------------------------------------------------------------------
// What the pet says
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PetItem {
    id: String,
    project_id: Option<String>,
    slot: Option<u8>,
    text: String,
    tone: &'static str,
    urgent: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PetView {
    mood: &'static str,
    running: usize,
    /// AI CLIs thinking right now: the visor's ring spins while any is.
    thinking: usize,
    errors: usize,
    done_at: Option<u64>,
    quiet: bool,
    items: Vec<PetItem>,
}

pub fn note_completion(app: &AppHandle, event: &CompletionEvent) {
    let Some(completion) = &event.completion else { return; };
    let Some(state) = app.try_state::<PetState>() else { return; };
    let mut list = lock(&state.completions);
    list.push_front(ShellDone {
        project_id: event.project_id.clone(),
        slot: event.slot,
        command: completion.command.clone(),
        exit_code: completion.exit_code,
        elapsed_ms: completion.elapsed_ms,
        at: completion.ended_at,
    });
    list.truncate(6);
    drop(list);
    refresh(app);
}

pub fn refresh(app: &AppHandle) {
    for label in LABELS {
        if app.get_webview_window(label).is_some() {
            let _ = app.emit_to(label, "pet-refresh", ());
        }
    }
}

fn short_command(command: &str) -> String {
    let command = command.trim();
    let mut out: String = command.chars().take(28).collect();
    if command.chars().count() > 28 {
        out.push('…');
    }
    out
}

fn view(
    projects: &[ProjectConfig],
    agents: &[(TerminalKey, AgentSession)],
    completions: &[ShellDone],
    now: u64,
) -> Vec<PetItem> {
    let name = |id: &str| projects.iter().find(|project| project.id == id).map(|project| {
        project.display_name.as_ref().filter(|name| !name.is_empty()).unwrap_or(&project.name).clone()
    }).unwrap_or_else(|| id.to_string());
    let item = |id: String, project: &str, slot: Option<u8>, text: String, tone, urgent| PetItem {
        id, project_id: Some(project.to_string()), slot, text, tone, urgent,
    };
    // The pet only reports what happens in your terminals while you are away:
    // agents waiting for you or finished, and commands that finished.
    // Starting, stopping and errors happen in front of you, so it stays out of those.
    let mut items = Vec::new();
    let mut sorted: Vec<_> = agents.iter().collect();
    sorted.sort_by_key(|(key, agent)| (agent.priority(), std::cmp::Reverse(agent.state_started_at), key.slot));
    for (key, agent) in &sorted {
        if agent.state == AgentState::Waiting {
            items.push(item(format!("wait:{}:{}", key.project_id, key.slot), &key.project_id, Some(key.slot),
                format!("{} · {} 等你确认", name(&key.project_id), agent.kind.label()), "notify", true));
        }
    }
    let mut finished = Vec::new();
    for (key, agent) in &sorted {
        if agent.state == AgentState::Done && !agent.acknowledged {
            finished.push((agent.state_started_at, item(format!("done:{}:{}:{}", key.project_id, key.slot, agent.state_started_at),
                &key.project_id, Some(key.slot),
                format!("{} · {} 完成了", name(&key.project_id), agent.kind.label()), "done", false)));
        }
    }
    for done in completions.iter().filter(|done| done.exit_code == 0 && now.saturating_sub(done.at) < COMPLETION_TTL_MS) {
        let elapsed = crate::terminal_shell::elapsed_text(done.elapsed_ms);
        finished.push((done.at, item(format!("shell:{}:{}:{}", done.project_id, done.slot, done.at),
            &done.project_id, Some(done.slot),
            format!("{} · {} 完成了（{elapsed}）", name(&done.project_id), short_command(&done.command)), "done", false)));
    }
    finished.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
    items.extend(finished.into_iter().map(|(_, item)| item));
    // Running projects are not reported one by one (the menu lists them); only
    // their count reaches the pet through PetView::running.
    items
}

fn mood(items: &[PetItem]) -> &'static str {
    if items.iter().any(|item| item.tone == "notify") { "notify" } else { "idle" }
}

#[tauri::command]
pub async fn get_pet_view(app: AppHandle) -> Result<PetView, String> {
    let managed = app.state::<ProcessMap>().lock().await.managed_snapshot();
    let projects = tauri::async_runtime::spawn_blocking(crate::get_projects_sync)
        .await
        .map_err(|error| error.to_string())??;
    let agents = app.state::<TerminalManager>().agents();
    let state = app.state::<PetState>();
    let now = now_ms();
    let completions: Vec<_> = lock(&state.completions).iter().cloned().collect();
    let items = view(&projects.projects, &agents, &completions, now);
    let agent_done = agents.iter()
        .filter(|(_, agent)| agent.state == AgentState::Done && !agent.acknowledged)
        .map(|(_, agent)| agent.state_started_at);
    let shell_done = completions.iter().filter(|done| done.exit_code == 0).map(|done| done.at);
    Ok(PetView {
        mood: mood(&items),
        running: managed.iter().filter(|info| info.phase == Phase::Running).count(),
        thinking: agents.iter().filter(|(_, agent)| agent.state == AgentState::Thinking).count(),
        errors: items.iter().filter(|item| item.tone == "error" && item.urgent).count(),
        done_at: agent_done.chain(shell_done).max(),
        quiet: state.quiet_until.load(Ordering::SeqCst) > now,
        items,
    })
}

// ---------------------------------------------------------------------------
// Actions: clicks and the right-click menu
// ---------------------------------------------------------------------------

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Selection {
    project_id: String,
    slot: Option<u8>,
    terminal: bool,
}

fn reveal(app: &AppHandle, project_id: Option<String>, slot: Option<u8>, terminal: bool) {
    app.state::<PetState>().hold_until.store(0, Ordering::SeqCst);
    crate::reopen_main_window(app);
    // Showing the window alone does not make Bento the active app (the pet never takes focus),
    // which under Stage Manager left the pet in the corner until Bento was clicked.
    activate_app(app);
    if let Some(project_id) = project_id {
        let _ = app.emit_to("main", "apphub-select-project", Selection { project_id, slot, terminal });
    }
}

#[tauri::command]
pub fn pet_action(app: AppHandle, action: String, project_id: Option<String>, slot: Option<u8>) -> Result<(), String> {
    match action.as_str() {
        "open" => reveal(&app, project_id, slot, false),
        "terminal" => reveal(&app, project_id, slot, true),
        _ => return Err("未知的桌宠操作".into()),
    }
    Ok(())
}

#[tauri::command]
pub async fn pet_menu(app: AppHandle, window: WebviewWindow) -> Result<(), String> {
    let managed = app.state::<ProcessMap>().lock().await.managed_snapshot();
    let projects = tauri::async_runtime::spawn_blocking(crate::get_projects_sync)
        .await
        .map_err(|error| error.to_string())??
        .projects;
    let menu = build_menu(&app, &projects, &managed).map_err(|error| error.to_string())?;
    window.popup_menu(&menu).map_err(|error| error.to_string())
}

fn build_menu(app: &AppHandle, projects: &[ProjectConfig], managed: &[ManagedProject]) -> tauri::Result<Menu<tauri::Wry>> {
    let id = |action: &str, value: &str| format!("{MENU_PREFIX}{action}:{value}");
    let name = |project: &ProjectConfig| project.display_name.as_ref().filter(|name| !name.is_empty()).unwrap_or(&project.name).clone();
    let running = managed.iter().filter(|info| info.phase == Phase::Running).count();
    let errors = managed.iter().filter(|info| info.phase == Phase::PortTimeout).count();
    let menu = Menu::new(app)?;
    let summary = if managed.is_empty() { "没有运行中的项目".to_string() }
        else if errors > 0 { format!("运行 {running} · 报错 {errors}") } else { format!("运行 {running} 个项目") };
    menu.append(&MenuItem::with_id(app, id("noop", "summary"), summary, false, None::<&str>)?)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;

    if !managed.is_empty() {
        let active = Submenu::new(app, "运行中", true)?;
        for info in managed {
            let label = projects.iter().find(|project| project.id == info.id).map(name).unwrap_or_else(|| info.id.clone());
            let phase = match info.phase { Phase::Running => "", Phase::Starting => "（启动中）", Phase::PortTimeout => "（报错）" };
            let port = info.port.map(|port| format!("  :{port}")).unwrap_or_default();
            let project = Submenu::new(app, format!("{label}{port}{phase}"), true)?;
            if let Some(port) = info.port {
                project.append(&MenuItem::with_id(app, id("url", &port.to_string()), format!("打开 localhost:{port}"), true, None::<&str>)?)?;
            }
            project.append(&MenuItem::with_id(app, id("open", &info.id), "在 Bento 中查看", true, None::<&str>)?)?;
            project.append(&MenuItem::with_id(app, id("terminal", &info.id), "打开终端", true, None::<&str>)?)?;
            project.append(&PredefinedMenuItem::separator(app)?)?;
            project.append(&MenuItem::with_id(app, id("restart", &info.id), "重启", true, None::<&str>)?)?;
            project.append(&MenuItem::with_id(app, id("stop", &info.id), "停止", true, None::<&str>)?)?;
            active.append(&project)?;
        }
        menu.append(&active)?;
    }

    let mut candidates: Vec<_> = projects.iter()
        .filter(|project| project.runnable && !project.archived && !managed.iter().any(|info| info.id == project.id))
        .collect();
    let recent = app.state::<crate::tray::TrayState>().recent_projects();
    candidates.sort_by_key(|project| (!project.pinned, recent.iter().position(|id| *id == project.id).unwrap_or(usize::MAX)));
    if !candidates.is_empty() {
        let start = Submenu::new(app, "快速启动", true)?;
        for project in candidates.into_iter().take(10) {
            start.append(&MenuItem::with_id(app, id("start", &project.id), name(project), true, None::<&str>)?)?;
        }
        menu.append(&start)?;
    }
    menu.append(&MenuItem::with_id(app, id("stop-all", ""), "全部停止…", !managed.is_empty(), None::<&str>)?)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;

    let state = app.state::<PetState>();
    let quiet = state.quiet_until.load(Ordering::SeqCst) > now_ms();
    let pet = Submenu::new(app, "桌宠", true)?;
    pet.append(&CheckMenuItem::with_id(app, id("quiet", ""), "勿扰 1 小时", true, quiet, None::<&str>)?)?;
    let style = style_of(&state);
    let opacity = Submenu::new(app, "透明度", true)?;
    for value in [100u8, 80, 60] {
        opacity.append(&CheckMenuItem::with_id(app, id("opacity", &value.to_string()), format!("{value}%"), true, style.opacity == value, None::<&str>)?)?;
    }
    pet.append(&opacity)?;
    pet.append(&MenuItem::with_id(app, id("hide", ""), "隐藏桌宠", true, None::<&str>)?)?;
    menu.append(&pet)?;
    menu.append(&MenuItem::with_id(app, id("show", ""), "打开 Bento", true, None::<&str>)?)?;
    menu.append(&MenuItem::with_id(app, id("settings", ""), "设置…", true, None::<&str>)?)?;
    Ok(menu)
}

pub fn is_menu(id: &str) -> bool {
    id.starts_with(MENU_PREFIX)
}

pub fn handle_menu(app: &AppHandle, id: &str) {
    let Some(rest) = id.strip_prefix(MENU_PREFIX) else { return; };
    let (action, value) = rest.split_once(':').unwrap_or((rest, ""));
    let value = value.to_string();
    match action {
        "url" => {
            let url = format!("http://localhost:{value}");
            tauri::async_runtime::spawn_blocking(move || { let _ = crate::open_url_sync(&url); });
        }
        "open" => reveal(app, Some(value), None, false),
        "terminal" => reveal(app, Some(value), None, true),
        "start" | "stop" | "restart" => run_project(app, action, value),
        "stop-all" => crate::tray::request_stop_all(app),
        "quiet" => {
            let state = app.state::<PetState>();
            let now = now_ms();
            let next = if state.quiet_until.load(Ordering::SeqCst) > now { 0 } else { now + 60 * 60 * 1000 };
            state.quiet_until.store(next, Ordering::SeqCst);
            refresh(app);
        }
        "opacity" => {
            let state = app.state::<PetState>();
            let Ok(number) = value.parse::<u8>() else { return; };
            state.opacity.store(number.clamp(30, 100), Ordering::SeqCst);
            let style = style_of(&state);
            for label in LABELS {
                let _ = app.emit_to(label, "pet-style", style);
            }
        }
        "hide" => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                let result = crate::settings::set_setting(app.clone(), crate::settings::SHOW_PET.into(), serde_json::json!(false));
                if let Err(error) = result { eprintln!("Bento pet: {error}"); }
            });
        }
        "show" => reveal(app, None, None, false),
        "settings" => {
            crate::reopen_main_window(app);
            let _ = app.emit("bento-open-settings", ());
        }
        _ => {}
    }
}

fn run_project(app: &AppHandle, action: &str, id: String) {
    let app = app.clone();
    let action = action.to_string();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<ProcessMap>();
        let result = match action.as_str() {
            "start" => crate::lifecycle::start_project(app.clone(), id.clone(), state).await,
            "stop" => crate::lifecycle::stop_project(app.clone(), id.clone(), state).await,
            _ => crate::lifecycle::restart_project(app.clone(), id.clone(), state).await,
        };
        match result {
            Ok(()) if action == "start" => { let _ = crate::tray::note_tray_project(app.clone(), id); }
            Ok(()) => {}
            Err(error) => {
                let title = match action.as_str() { "start" => "启动项目失败", "stop" => "停止项目失败", _ => "重启项目失败" };
                app.dialog().message(error).title(title).kind(MessageDialogKind::Error).show(|_| {});
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal_agent::AgentKind;

    fn config(id: &str) -> ProjectConfig {
        ProjectConfig {
            id: id.into(), name: id.into(), display_name: None, description: None, path: "/tmp".into(),
            runnable: true, port: None, services: Vec::new(), inject_ports: true, start_command: "sleep 1".into(),
            project_type: Some("app".into()), env: Vec::new(), commands: Vec::new(), auto_open: false,
            group_id: None, pinned: false, archived: false,
        }
    }

    #[test]
    fn reports_waiting_and_finished_only() {
        let projects = [config("web"), config("api"), config("ai")];
        let base = AgentSession { kind: AgentKind::Codex, state: AgentState::Thinking, state_started_at: 10,
            round_started_at: Some(10), acknowledged: false };
        let agents = [
            (TerminalKey::new("ai", 1), base),
            (TerminalKey::new("ai", 2), AgentSession { kind: AgentKind::Claude, state: AgentState::Waiting, ..base }),
            (TerminalKey::new("web", 1), AgentSession { state: AgentState::Done, state_started_at: 20, ..base }),
        ];
        let completions = [ShellDone { project_id: "web".into(), slot: 2, command: "pnpm build".into(), exit_code: 0,
            elapsed_ms: 12_000, at: 30 }];
        let items = view(&projects, &agents, &completions, 60);
        let tones: Vec<_> = items.iter().map(|item| item.tone).collect();
        assert_eq!(tones, ["notify", "done", "done"]);
        assert_eq!(items[0].text, "ai · Claude 等你确认");
        assert_eq!(items[1].text, "web · pnpm build 完成了（12s）");
        assert_eq!(items[2].text, "web · Codex 完成了");
        assert_eq!(mood(&items), "notify");
        assert_eq!(mood(&items[1..]), "idle");
    }

    #[test]
    fn failures_and_old_completions_stay_quiet() {
        let failed = ShellDone { project_id: "web".into(), slot: 1, command: "make".into(), exit_code: 2, elapsed_ms: 1, at: 0 };
        assert!(view(&[], &[], &[failed], 1000).is_empty());
        let done = ShellDone { project_id: "web".into(), slot: 1, command: "make".into(), exit_code: 0, elapsed_ms: 1, at: 0 };
        assert!(view(&[], &[], &[done.clone()], COMPLETION_TTL_MS + 1).is_empty());
        assert_eq!(view(&[], &[], &[done], 1000).len(), 1);
    }

}
