//! Native Windows notification-area controls on a dedicated UI thread.
//!
//! `TrackPopupMenu` runs a modal loop. Keeping the tray window on its own
//! thread prevents that loop from blocking keyboard-disposition handshakes.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread::JoinHandle;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    DispatchMessageW, GetCursorPos, GetForegroundWindow, GetMessageW, HCURSOR, HICON,
    IDI_APPLICATION, LoadIconW, MF_CHECKED, MF_SEPARATOR, MF_STRING, MSG, PostMessageW,
    PostThreadMessageW, RegisterClassExW, RegisterWindowMessageW, SetForegroundWindow,
    TPM_BOTTOMALIGN, TPM_LEFTALIGN, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu,
    TranslateMessage, WM_APP, WM_CANCELMODE, WM_DISPLAYCHANGE, WM_LBUTTONUP, WM_NULL, WM_QUIT,
    WM_RBUTTONUP, WM_SETTINGCHANGE, WNDCLASSEXW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP,
};
use windows::core::{PCWSTR, w};

use crate::api::Autostart;
use crate::api::backend::BackendEvent;

use super::EventSender;

const CALLBACK_MESSAGE: u32 = WM_APP + 0x4E;
const ICON_ID: u32 = 1;
const APP_ICON_RESOURCE_ID: usize = 1;
const CMD_TOGGLE: i32 = 1;
const CMD_RELOAD: i32 = 2;
const CMD_AUTOSTART: i32 = 3;
const CMD_QUIT: i32 = 4;

static SENDER: OnceLock<Mutex<Option<EventSender>>> = OnceLock::new();
static ENABLED: AtomicBool = AtomicBool::new(true);
static DISPLAY_CHANGED: AtomicBool = AtomicBool::new(false);
static APPEARANCE_CHANGED: AtomicBool = AtomicBool::new(false);
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

pub struct StatusItem {
    thread_id: u32,
    hwnd: HWND,
    join: Option<JoinHandle<()>>,
}

impl StatusItem {
    pub fn new(sender: EventSender) -> Result<Self, String> {
        *SENDER
            .get_or_init(|| Mutex::new(None))
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(sender);
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let join = std::thread::Builder::new()
            .name("keysteer-tray".into())
            .spawn(move || tray_thread(ready_tx))
            .map_err(|error| {
                clear_sender();
                format!("cannot start tray thread: {error}")
            })?;
        match ready_rx.recv() {
            Ok(Ok((thread_id, hwnd))) => Ok(Self {
                thread_id,
                hwnd: HWND(hwnd as *mut _),
                join: Some(join),
            }),
            Ok(Err(error)) => {
                let _ = join.join();
                clear_sender();
                Err(error)
            }
            Err(_) => {
                let _ = join.join();
                clear_sender();
                Err("tray thread stopped before reporting readiness".into())
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

    pub fn stop(&mut self) {
        let mut posted = false;
        if self.thread_id != 0 {
            if !self.hwnd.is_invalid() {
                let _ =
                    unsafe { PostMessageW(Some(self.hwnd), WM_CANCELMODE, WPARAM(0), LPARAM(0)) };
            }
            match unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) } {
                Ok(()) => posted = true,
                Err(error) => {
                    crate::log_warning!("windows-tray", "cannot request tray shutdown: {error}")
                }
            }
            self.thread_id = 0;
            self.hwnd = HWND::default();
        }
        if let Some(join) = self.join.take() {
            if posted {
                if join.join().is_err() {
                    crate::app::logging::report_error("windows-tray", "tray thread panicked");
                }
            } else {
                // Never block shutdown when Windows rejected the only wake-up
                // that can terminate this message-loop thread.
                drop(join);
            }
        }
        clear_sender();
    }
}

impl Drop for StatusItem {
    fn drop(&mut self) {
        self.stop();
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
        crate::log_warning!(
            "windows-tray",
            "Shell_NotifyIconW could not add the notification-area icon"
        );
    }
    let thread_id = unsafe { GetCurrentThreadId() };
    if ready.send(Ok((thread_id, hwnd.0 as isize))).is_err() {
        destroy_window(hwnd);
        return;
    }

    let mut message = MSG::default();
    loop {
        let status = unsafe { GetMessageW(&mut message, None, 0, 0) }.0;
        if status == 0 {
            break;
        }
        if status == -1 {
            crate::app::logging::report_error(
                "windows-tray",
                format!("GetMessageW failed: {:?}", unsafe {
                    windows::Win32::Foundation::GetLastError()
                }),
            );
            break;
        }
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    destroy_window(hwnd);
}

fn create_window() -> Result<HWND, String> {
    let taskbar_created = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
    if taskbar_created == 0 {
        return Err("RegisterWindowMessageW(TaskbarCreated) failed".into());
    }
    TASKBAR_CREATED.store(taskbar_created, Ordering::Release);
    let instance = unsafe { GetModuleHandleW(None) }
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
    if unsafe { RegisterClassExW(&class) } == 0 {
        const ERROR_CLASS_ALREADY_EXISTS: u32 = 1410;
        let last = unsafe { windows::Win32::Foundation::GetLastError() };
        if last.0 != ERROR_CLASS_ALREADY_EXISTS {
            return Err(format!("RegisterClassExW(status) failed: {last:?}"));
        }
    }
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
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &icon_data(hwnd));
    }
    if let Err(error) = unsafe { DestroyWindow(hwnd) } {
        crate::log_warning!("windows-tray", "cannot destroy tray event window: {error}");
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
    let embedded = unsafe {
        GetModuleHandleW(None).and_then(|module| {
            LoadIconW(
                Some(module.into()),
                PCWSTR(APP_ICON_RESOURCE_ID as *const u16),
            )
        })
    };
    embedded
        .or_else(|_| unsafe { LoadIconW(None, IDI_APPLICATION) })
        .unwrap_or_else(|error| {
            crate::log_warning!("windows-tray", "cannot load application icon: {error}");
            HICON::default()
        })
}

fn add_icon(hwnd: HWND) -> bool {
    unsafe { Shell_NotifyIconW(NIM_ADD, &icon_data(hwnd)) }.as_bool()
}

fn emit(event: BackendEvent) {
    let sender = SENDER
        .get()
        .and_then(|sender| sender.lock().ok())
        .and_then(|sender| sender.clone());
    if let Some(sender) = sender
        && sender.send(event).is_err()
    {
        crate::log_warning!("windows-tray", "tray event receiver is unavailable");
    }
}

fn show_menu(hwnd: HWND) {
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
                    autostart_flags,
                    CMD_AUTOSTART as usize,
                    w!("Start at Login"),
                )
            })
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
    if let Err(error) = unsafe { GetCursorPos(&mut point) } {
        crate::app::logging::report_error(
            "windows-tray",
            format!("cannot position tray menu: {error}"),
        );
    }
    let previous_foreground = unsafe { GetForegroundWindow() };
    unsafe {
        let _ = SetForegroundWindow(hwnd);
    }
    let command = unsafe {
        TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_LEFTALIGN | TPM_BOTTOMALIGN,
            point.x,
            point.y,
            None,
            hwnd,
            None,
        )
    };
    if let Err(error) = unsafe { DestroyMenu(menu) } {
        crate::log_warning!("windows-tray", "cannot destroy tray menu: {error}");
    }
    let _ = unsafe { PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0)) };
    if !previous_foreground.is_invalid() && unsafe { GetForegroundWindow() } == hwnd {
        unsafe {
            let _ = SetForegroundWindow(previous_foreground);
        }
    }
    match command.0 {
        CMD_TOGGLE => emit(BackendEvent::ToggleEnabled),
        CMD_RELOAD => emit(BackendEvent::ReloadConfig),
        CMD_AUTOSTART => emit(BackendEvent::ToggleAutostart),
        CMD_QUIT => emit(BackendEvent::Quit),
        _ => {}
    }
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
            crate::log_warning!(
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
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
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
        clear_sender();
    }
}
