//! Native Windows notification-area controls on a dedicated UI thread.
//!
//! `TrackPopupMenu` runs a modal loop. Keeping the tray window on its own
//! thread prevents that loop from blocking keyboard-disposition handshakes.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW, Shell_NotifyIconW,
    ShellExecuteW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DestroyMenu, DestroyWindow, DispatchMessageW,
    GetCursorPos, GetForegroundWindow, HCURSOR, HICON, IDI_APPLICATION, IDYES, LoadIconW,
    MB_ICONERROR, MB_ICONINFORMATION, MB_OK, MB_SETFOREGROUND, MB_YESNO, MF_CHECKED, MF_GRAYED,
    MF_SEPARATOR, MF_STRING, MSG, MessageBoxW, PostMessageW, RegisterWindowMessageW, SW_SHOWNORMAL,
    SetForegroundWindow, TPM_BOTTOMALIGN, TPM_LEFTALIGN, TPM_RETURNCMD, TPM_RIGHTBUTTON,
    TrackPopupMenu, TranslateMessage, WM_APP, WM_CANCELMODE, WM_DISPLAYCHANGE, WM_LBUTTONUP,
    WM_NULL, WM_QUIT, WM_RBUTTONUP, WM_SETTINGCHANGE, WNDCLASSEXW, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_POPUP,
};
use windows::core::{HSTRING, PCWSTR, w};

use crate::api::Autostart;
use crate::api::backend::{BackendEvent, UpdateCheckResult, UpdateProgress};
use crate::app::worker::WorkerJoin;

use super::EventSender;

const CALLBACK_MESSAGE: u32 = WM_APP + 0x4E;
const ICON_ID: u32 = 1;
const APP_ICON_RESOURCE_ID: usize = 1;
const CMD_TOGGLE: i32 = 1;
const CMD_RELOAD: i32 = 2;
const CMD_CONFIG_SIMULATOR: i32 = 3;
const CMD_AUTOSTART: i32 = 4;
const CMD_CHECK_UPDATES: i32 = 5;
const CMD_ABOUT: i32 = 6;
const CMD_QUIT: i32 = 7;

static SENDER: OnceLock<Mutex<Option<EventSender>>> = OnceLock::new();
static ENABLED: AtomicBool = AtomicBool::new(true);
static DISPLAY_CHANGED: AtomicBool = AtomicBool::new(false);
static APPEARANCE_CHANGED: AtomicBool = AtomicBool::new(false);
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);
static NATIVE_DIALOG_VISIBLE: AtomicBool = AtomicBool::new(false);
static UPDATE_MENU_STATE: OnceLock<Mutex<UpdateMenuState>> = OnceLock::new();
const NATIVE_DIALOG_THREAD_STACK_BYTES: usize = 256 * 1024;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
enum UpdateMenuState {
    #[default]
    Idle,
    Checking,
    Downloading {
        latest: String,
        percent: u8,
    },
    Downloaded {
        latest: String,
    },
    UpToDate {
        current: String,
    },
    Failed,
}

struct NativeDialogGuard;

impl NativeDialogGuard {
    fn acquire() -> Option<Self> {
        NATIVE_DIALOG_VISIBLE
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
            .then_some(Self)
    }
}

impl Drop for NativeDialogGuard {
    fn drop(&mut self) {
        NATIVE_DIALOG_VISIBLE.store(false, Ordering::Release);
    }
}

pub struct StatusItem {
    thread_id: u32,
    hwnd: HWND,
    worker: Option<WorkerJoin>,
}

const TRAY_STOP_TIMEOUT: Duration = Duration::from_secs(2);

impl StatusItem {
    pub fn new(sender: EventSender) -> Result<Self, String> {
        set_update_menu_state(UpdateMenuState::Idle);
        *SENDER
            .get_or_init(|| Mutex::new(None))
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(sender);
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let mut worker = WorkerJoin::spawn(
            "Windows tray",
            std::thread::Builder::new().name("keysteer-tray".into()),
            move || tray_thread(ready_tx),
        )
        .inspect_err(|_| {
            clear_sender();
        })?;
        match worker.wait_ready(&ready_rx, TRAY_STOP_TIMEOUT) {
            Ok(Ok((thread_id, hwnd))) => Ok(Self {
                thread_id,
                hwnd: HWND(hwnd as *mut _),
                worker: Some(worker),
            }),
            Ok(Err(error)) => {
                let _ = worker.join_timeout(TRAY_STOP_TIMEOUT);
                clear_sender();
                Err(error)
            }
            Err(error) => {
                let _ = worker.join_timeout(TRAY_STOP_TIMEOUT);
                clear_sender();
                Err(error)
            }
        }
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        ENABLED.store(enabled, Ordering::Release);
    }

    pub fn take_display_changed(&self) -> bool {
        DISPLAY_CHANGED.swap(false, Ordering::AcqRel)
    }

    pub fn take_appearance_changed(&self) -> bool {
        APPEARANCE_CHANGED.swap(false, Ordering::AcqRel)
    }

    pub fn stop(&mut self) -> Result<(), String> {
        if self.worker.is_none() {
            clear_sender();
            return Ok(());
        }
        let mut posted = false;
        if self.thread_id != 0 {
            if !self.hwnd.is_invalid() {
                // SAFETY: `hwnd` belongs to the tray thread; WM_CANCELMODE has
                // no borrowed payload and only ends a possible menu loop.
                let _ =
                    unsafe { PostMessageW(Some(self.hwnd), WM_CANCELMODE, WPARAM(0), LPARAM(0)) };
            }
            match super::native::post_thread_message(self.thread_id, WM_QUIT, 0) {
                Ok(()) => posted = true,
                Err(error) => {
                    crate::report_warning!("windows-tray", "cannot request tray shutdown: {error}")
                }
            }
            self.thread_id = 0;
            self.hwnd = HWND::default();
        }
        let result = self
            .worker
            .as_mut()
            .map_or(Ok(()), |worker| worker.join_timeout(TRAY_STOP_TIMEOUT));
        if result.is_ok() {
            self.worker.take();
        }
        clear_sender();
        if !posted && result.is_ok() {
            return Err("Windows rejected the tray shutdown wake-up".into());
        }
        result
    }
}

pub(super) fn set_update_progress(progress: &UpdateProgress) {
    let state = match progress {
        UpdateProgress::Checking => UpdateMenuState::Checking,
        UpdateProgress::Downloading { latest, percent } => UpdateMenuState::Downloading {
            latest: latest.clone(),
            percent: (*percent).min(100),
        },
    };
    set_update_menu_state(state);
}

impl Drop for StatusItem {
    fn drop(&mut self) {
        if let Err(error) = self.stop() {
            crate::app::logging::report_error("windows-tray", &error);
        }
    }
}

fn tray_thread(ready: std::sync::mpsc::SyncSender<Result<(u32, isize), String>>) {
    let hwnd = match create_window() {
        Ok(hwnd) => hwnd,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    if !add_icon(hwnd) {
        crate::report_warning!(
            "windows-tray",
            "Shell_NotifyIconW could not add the notification-area icon"
        );
    }
    let thread_id = super::native::current_thread_id();
    if ready.send(Ok((thread_id, hwnd.0 as isize))).is_err() {
        destroy_window(hwnd);
        return;
    }

    let mut message = MSG::default();
    loop {
        let status = super::native::get_window_message(&mut message);
        if status == 0 {
            break;
        }
        if status == -1 {
            // SAFETY: GetLastError has no arguments and is read immediately
            // after the failed message retrieval.
            let last_error = unsafe { windows::Win32::Foundation::GetLastError() };
            crate::app::logging::report_error(
                "windows-tray",
                format!("GetMessageW failed: {last_error:?}"),
            );
            break;
        }
        // SAFETY: `message` was initialized by GetMessageW and is dispatched
        // synchronously on this same tray thread.
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    destroy_window(hwnd);
}

fn create_window() -> Result<HWND, String> {
    // SAFETY: the static message name is NUL-terminated and lives forever.
    let taskbar_created = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
    if taskbar_created == 0 {
        return Err("RegisterWindowMessageW(TaskbarCreated) failed".into());
    }
    TASKBAR_CREATED.store(taskbar_created, Ordering::Release);
    let instance = super::native::current_module()
        .map_err(|error| format!("GetModuleHandleW failed: {error}"))?;
    let class = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(window_proc),
        hInstance: instance.into(),
        lpszClassName: w!("KeySteerStatusWindow"),
        hIcon: load_app_icon(),
        hIconSm: load_app_icon(),
        hCursor: HCURSOR::default(),
        ..Default::default()
    };
    super::native::register_window_class(&class)?;
    // SAFETY: the class was registered above, all strings are static, and the
    // returned HWND is owned by this tray thread until `destroy_window`.
    unsafe {
        CreateWindowExW(
            WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
            w!("KeySteerStatusWindow"),
            w!("KeySteer"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance.into()),
            None,
        )
    }
    .map_err(|error| format!("CreateWindowExW(status) failed: {error}"))
}

fn destroy_window(hwnd: HWND) {
    // SAFETY: `hwnd` owns the icon identified by `icon_data`; NIM_DELETE does
    // not retain the stack structure.
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &icon_data(hwnd));
    }
    if let Err(error) = super::native::OwnedWindow::new(hwnd).destroy() {
        crate::report_warning!("windows-tray", "cannot destroy tray event window: {error}");
    }
}

fn clear_sender() {
    if let Some(sender) = SENDER.get() {
        *sender.lock().unwrap_or_else(|error| error.into_inner()) = None;
    }
}

fn icon_data(hwnd: HWND) -> NOTIFYICONDATAW {
    let icon = load_app_icon();
    let mut data = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: ICON_ID,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
        uCallbackMessage: CALLBACK_MESSAGE,
        hIcon: icon,
        ..Default::default()
    };
    let tip: Vec<u16> = "KeySteer".encode_utf16().collect();
    data.szTip[..tip.len()].copy_from_slice(&tip);
    data
}

fn load_app_icon() -> HICON {
    // SAFETY: the integer resource id is encoded using the Win32 MAKEINTRESOURCE
    // convention and the module handle remains live for the process.
    let embedded = unsafe {
        super::native::current_module().and_then(|module| {
            LoadIconW(
                Some(module.into()),
                PCWSTR(APP_ICON_RESOURCE_ID as *const u16),
            )
        })
    };
    embedded
        // SAFETY: IDI_APPLICATION is a system-owned shared icon; no destroy is
        // required or permitted for the returned handle.
        .or_else(|_| unsafe { LoadIconW(None, IDI_APPLICATION) })
        .unwrap_or_else(|error| {
            crate::report_warning!("windows-tray", "cannot load application icon: {error}");
            HICON::default()
        })
}

fn add_icon(hwnd: HWND) -> bool {
    // SAFETY: icon_data is fully initialized and remains live for the
    // synchronous Shell_NotifyIconW copy.
    unsafe { Shell_NotifyIconW(NIM_ADD, &icon_data(hwnd)) }.as_bool()
}

pub(super) fn open_https_url(url: &str) -> Result<(), String> {
    if !url.starts_with("https://") || url.contains('\0') {
        return Err("Windows refused an invalid HTTPS URL".into());
    }
    let url = HSTRING::from(url);
    // SAFETY: all strings are valid, live, NUL-terminated UTF-16 values for
    // the duration of the call. No owner HWND, parameters or working directory
    // are supplied. ShellExecuteW does not return an owned process handle.
    let result = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            &url,
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    let code = result.0 as isize;
    if code > 32 {
        Ok(())
    } else {
        Err(format!(
            "Windows could not open the default HTTPS browser (ShellExecuteW code {code})"
        ))
    }
}

fn emit(event: BackendEvent) {
    let sender = SENDER
        .get()
        .and_then(|sender| sender.lock().ok())
        .and_then(|sender| sender.clone());
    if let Some(sender) = sender
        && sender.send(event).is_err()
    {
        crate::report_warning!("windows-tray", "tray event receiver is unavailable");
    }
}

fn show_menu(hwnd: HWND) {
    // SAFETY: CreatePopupMenu has no borrowed arguments; this function destroys
    // the returned menu before leaving.
    let Ok(menu) = (unsafe { CreatePopupMenu() }) else {
        return;
    };
    let toggle = if ENABLED.load(Ordering::Acquire) {
        w!("Pause")
    } else {
        w!("Resume")
    };
    let autostart_checked = match super::autostart::WindowsAutostart::new().is_enabled() {
        Ok(enabled) => enabled,
        Err(error) => {
            crate::app::logging::report_error("windows-autostart", error);
            false
        }
    };
    let autostart_flags = if autostart_checked {
        MF_STRING | MF_CHECKED
    } else {
        MF_STRING
    };
    let (update_title, update_enabled) = update_menu_entry();
    let update_title = wide(&update_title);
    let update_flags = if update_enabled {
        MF_STRING
    } else {
        MF_STRING | MF_GRAYED
    };
    // SAFETY: `menu` remains live, all labels are static or NUL-terminated and
    // each AppendMenuW copies its data synchronously.
    let menu_result = unsafe {
        AppendMenuW(menu, MF_STRING, CMD_TOGGLE as usize, toggle)
            .and_then(|_| {
                AppendMenuW(
                    menu,
                    MF_STRING,
                    CMD_RELOAD as usize,
                    w!("Reload Configuration"),
                )
            })
            .and_then(|_| {
                AppendMenuW(
                    menu,
                    MF_STRING,
                    CMD_CONFIG_SIMULATOR as usize,
                    w!("Configuration & Simulator..."),
                )
            })
            .and_then(|_| {
                AppendMenuW(
                    menu,
                    autostart_flags,
                    CMD_AUTOSTART as usize,
                    w!("Start at Login"),
                )
            })
            .and_then(|_| {
                AppendMenuW(
                    menu,
                    update_flags,
                    CMD_CHECK_UPDATES as usize,
                    PCWSTR(update_title.as_ptr()),
                )
            })
            .and_then(|_| AppendMenuW(menu, MF_STRING, CMD_ABOUT as usize, w!("About KeySteer...")))
            .and_then(|_| AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null()))
            .and_then(|_| AppendMenuW(menu, MF_STRING, CMD_QUIT as usize, w!("Quit KeySteer")))
    };
    if let Err(error) = menu_result {
        crate::app::logging::report_error(
            "windows-tray",
            format!("cannot build tray menu: {error}"),
        );
    }
    let mut point = POINT::default();
    // SAFETY: `point` is a valid writable out-parameter.
    if let Err(error) = unsafe { GetCursorPos(&mut point) } {
        crate::app::logging::report_error(
            "windows-tray",
            format!("cannot position tray menu: {error}"),
        );
    }
    // SAFETY: `menu` and `hwnd` belong to this tray thread and remain live
    // throughout the modal call; no Rust pointer is retained.
    let (previous_foreground, command) = unsafe {
        let previous_foreground = GetForegroundWindow();
        let _ = SetForegroundWindow(hwnd);
        let command = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_LEFTALIGN | TPM_BOTTOMALIGN,
            point.x,
            point.y,
            None,
            hwnd,
            None,
        );
        (previous_foreground, command)
    };
    // SAFETY: menu and hwnd were created by this tray thread and remain valid
    // through this cleanup; the foreground HWND is only compared/restored.
    unsafe {
        if let Err(error) = DestroyMenu(menu) {
            crate::report_warning!("windows-tray", "cannot destroy tray menu: {error}");
        }
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        if !previous_foreground.is_invalid() && GetForegroundWindow() == hwnd {
            let _ = SetForegroundWindow(previous_foreground);
        }
    }
    match command.0 {
        CMD_TOGGLE => emit(BackendEvent::ToggleEnabled),
        CMD_RELOAD => emit(BackendEvent::ReloadConfig),
        CMD_CONFIG_SIMULATOR => emit(BackendEvent::OpenConfigSimulator),
        CMD_AUTOSTART => emit(BackendEvent::ToggleAutostart),
        CMD_CHECK_UPDATES => emit(BackendEvent::CheckForUpdates),
        CMD_ABOUT => present_about(),
        CMD_QUIT => emit(BackendEvent::Quit),
        _ => {}
    }
}

pub(super) fn present_update_result(result: &UpdateCheckResult) -> Result<(), String> {
    set_update_result(result);
    let Some(guard) = NativeDialogGuard::acquire() else {
        return Ok(());
    };
    let result = result.clone();
    std::thread::Builder::new()
        .name("keysteer-update-ui".into())
        .stack_size(NATIVE_DIALOG_THREAD_STACK_BYTES)
        .spawn(move || {
            let _guard = guard;
            let presentation = match result {
                UpdateCheckResult::UpdateDownloaded {
                    current,
                    latest,
                    path,
                } => show_message(
                        "KeySteer Update",
                        &format!(
                            "KeySteer {latest} was downloaded successfully.\n\nSaved to:\n{}\n\nQuit KeySteer, extract the ZIP, and replace version {current} when ready.\n\nOpen the download folder now?",
                            path.display()
                        ),
                        false,
                        true,
                    )
                    .and_then(|open| {
                        if open {
                            open_download_folder(&path)
                        } else {
                            Ok(())
                        }
                    }),
                UpdateCheckResult::UpToDate { current } => show_message(
                        "KeySteer Update",
                        &format!("KeySteer {current} is already the latest version."),
                        false,
                        false,
                    )
                .map(|_| ()),
                UpdateCheckResult::Failed(error) => {
                    show_message(
                        "KeySteer Update",
                        &format!("Could not check for updates.\n\n{error}"),
                        true,
                        false,
                    )
                    .map(|_| ())
                }
            };
            if let Err(error) = presentation {
                crate::app::logging::report_error("windows-update", error);
            }
        })
        .map(|_| ())
        .map_err(|error| format!("cannot start Windows update UI: {error}"))
}

fn present_about() {
    let Some(guard) = NativeDialogGuard::acquire() else {
        return;
    };
    if let Err(error) = std::thread::Builder::new()
        .name("keysteer-about-ui".into())
        .stack_size(NATIVE_DIALOG_THREAD_STACK_BYTES)
        .spawn(move || {
            let _guard = guard;
            if let Err(error) = show_message(
                "About KeySteer",
                &crate::app::about::details(),
                false,
                false,
            ) {
                crate::app::logging::report_error("windows-about", error);
            }
        })
    {
        crate::app::logging::report_error(
            "windows-about",
            format!("cannot start Windows About UI: {error}"),
        );
    }
}

fn update_menu_state() -> &'static Mutex<UpdateMenuState> {
    UPDATE_MENU_STATE.get_or_init(|| Mutex::new(UpdateMenuState::Idle))
}

fn set_update_menu_state(state: UpdateMenuState) {
    *update_menu_state()
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = state;
}

fn set_update_result(result: &UpdateCheckResult) {
    let state = match result {
        UpdateCheckResult::UpdateDownloaded { latest, .. } => UpdateMenuState::Downloaded {
            latest: latest.clone(),
        },
        UpdateCheckResult::UpToDate { current } => UpdateMenuState::UpToDate {
            current: current.clone(),
        },
        UpdateCheckResult::Failed(_) => UpdateMenuState::Failed,
    };
    set_update_menu_state(state);
}

fn update_menu_entry() -> (String, bool) {
    let state = update_menu_state()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    match &*state {
        UpdateMenuState::Idle => ("Check for Updates...".into(), true),
        UpdateMenuState::Checking => ("Checking for Updates...".into(), false),
        UpdateMenuState::Downloading { latest, percent } => (
            format!("Downloading KeySteer {latest}... {percent}%"),
            false,
        ),
        UpdateMenuState::Downloaded { latest } => (format!("KeySteer {latest} Downloaded"), true),
        UpdateMenuState::UpToDate { current } => {
            (format!("KeySteer {current} Is Up to Date"), true)
        }
        UpdateMenuState::Failed => ("Update Check Failed - Retry...".into(), true),
    }
}

fn show_message(
    title: &str,
    message: &str,
    is_error: bool,
    offer_open: bool,
) -> Result<bool, String> {
    // A detached MessageBox without an owner can remain inactive. This hidden
    // same-thread owner gives the dialog a complete native modal lifetime while
    // keeping the engine and tray threads unblocked.
    // SAFETY: the temporary owner and all UTF-16 buffers remain live through
    // the modal call; the same thread destroys the owner exactly once.
    let (response, destroy) = unsafe {
        let owner = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            w!("STATIC"),
            w!("KeySteer Dialog"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            None,
            None,
        )
        .map_err(|error| format!("cannot create Windows dialog owner: {error}"))?;
        let title = wide(title);
        let message = wide(message);
        let flags = (if offer_open { MB_YESNO } else { MB_OK })
            | MB_SETFOREGROUND
            | if is_error {
                MB_ICONERROR
            } else {
                MB_ICONINFORMATION
            };
        let response = MessageBoxW(
            Some(owner),
            PCWSTR(message.as_ptr()),
            PCWSTR(title.as_ptr()),
            flags,
        );
        let destroy = DestroyWindow(owner)
            .map_err(|error| format!("cannot destroy Windows update dialog owner: {error}"));
        (response, destroy)
    };
    if response.0 == 0 {
        return Err("MessageBoxW could not present the dialog".into());
    }
    destroy?;
    Ok(offer_open && response == IDYES)
}

fn open_download_folder(download: &Path) -> Result<(), String> {
    let directory = download.parent().ok_or_else(|| {
        format!(
            "download path {} has no parent directory",
            download.display()
        )
    })?;
    std::process::Command::new("explorer.exe")
        .arg(directory)
        .spawn()
        .map(|_| ())
        .map_err(|error| {
            format!(
                "Windows could not open the download folder {}: {error}",
                directory.display()
            )
        })
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let taskbar_created = TASKBAR_CREATED.load(Ordering::Acquire);
    if taskbar_created != 0 && message == taskbar_created {
        if !add_icon(hwnd) {
            crate::report_warning!(
                "windows-tray",
                "cannot restore tray icon after Explorer restart"
            );
        }
        return LRESULT(0);
    }
    match message {
        CALLBACK_MESSAGE => {
            match lparam.0 as u32 {
                // Left and right click have the same harmless behaviour. Pause
                // remains an explicit menu action instead of an accidental click.
                WM_LBUTTONUP | WM_RBUTTONUP => show_menu(hwnd),
                _ => {}
            }
            LRESULT(0)
        }
        WM_DISPLAYCHANGE => {
            DISPLAY_CHANGED.store(true, Ordering::Release);
            LRESULT(0)
        }
        WM_SETTINGCHANGE => {
            APPEARANCE_CHANGED.store(true, Ordering::Release);
            LRESULT(0)
        }
        _ => super::native::default_window_proc(hwnd, message, wparam, lparam),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_actions_use_the_backend_event_channel() {
        let (sender, receiver) = std::sync::mpsc::channel();
        *SENDER.get_or_init(|| Mutex::new(None)).lock().unwrap() =
            Some(EventSender::without_wake(sender));
        emit(BackendEvent::ReloadConfig);
        assert!(matches!(
            receiver.recv().unwrap(),
            BackendEvent::ReloadConfig
        ));
        emit(BackendEvent::OpenConfigSimulator);
        assert!(matches!(
            receiver.recv().unwrap(),
            BackendEvent::OpenConfigSimulator
        ));
        emit(BackendEvent::CheckForUpdates);
        assert!(matches!(
            receiver.recv().unwrap(),
            BackendEvent::CheckForUpdates
        ));
        clear_sender();
    }

    #[test]
    fn native_dialog_is_single_instance_and_releases_its_guard() {
        let first = NativeDialogGuard::acquire().expect("first dialog should acquire the guard");
        assert!(NativeDialogGuard::acquire().is_none());

        drop(first);
        assert!(NativeDialogGuard::acquire().is_some());
    }

    #[test]
    fn shell_url_launcher_rejects_non_https_and_embedded_nul() {
        assert!(open_https_url("file:///C:/Windows").is_err());
        assert!(open_https_url("https://example.invalid/\0payload").is_err());
    }

    #[test]
    fn update_menu_reports_checking_download_progress_and_completion() {
        set_update_progress(&UpdateProgress::Checking);
        assert_eq!(
            update_menu_entry(),
            ("Checking for Updates...".into(), false)
        );

        set_update_progress(&UpdateProgress::Downloading {
            latest: "0.6.0".into(),
            percent: 42,
        });
        assert_eq!(
            update_menu_entry(),
            ("Downloading KeySteer 0.6.0... 42%".into(), false)
        );

        set_update_result(&UpdateCheckResult::UpdateDownloaded {
            current: "0.5.0".into(),
            latest: "0.6.0".into(),
            path: "KeySteer.zip".into(),
        });
        assert_eq!(
            update_menu_entry(),
            ("KeySteer 0.6.0 Downloaded".into(), true)
        );
        set_update_menu_state(UpdateMenuState::Idle);
    }
}
