//! Event-driven foreground-window notifications.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows::Win32::UI::WindowsAndMessaging::{
    EVENT_SYSTEM_FOREGROUND, PostThreadMessageW, WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS,
};

use super::WAKE_MESSAGE;

static FOCUS_CHANGED: AtomicBool = AtomicBool::new(false);
static OWNER_THREAD: AtomicU32 = AtomicU32::new(0);

pub struct ForegroundWatcher(HWINEVENTHOOK);

impl ForegroundWatcher {
    pub fn new(owner_thread: u32) -> Result<Self, String> {
        OWNER_THREAD.store(owner_thread, Ordering::Release);
        let hook = unsafe {
            SetWinEventHook(
                EVENT_SYSTEM_FOREGROUND,
                EVENT_SYSTEM_FOREGROUND,
                None,
                Some(win_event),
                0,
                0,
                WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
            )
        };
        if hook.is_invalid() {
            OWNER_THREAD.store(0, Ordering::Release);
            Err("SetWinEventHook(EVENT_SYSTEM_FOREGROUND) failed".into())
        } else {
            Ok(Self(hook))
        }
    }

    pub fn take_changed(&self) -> bool {
        FOCUS_CHANGED.swap(false, Ordering::AcqRel)
    }
}

impl Drop for ForegroundWatcher {
    fn drop(&mut self) {
        if !unsafe { UnhookWinEvent(self.0) }.as_bool() {
            crate::log_warning!(
                "windows-events",
                "UnhookWinEvent failed while stopping foreground notifications"
            );
        }
        OWNER_THREAD.store(0, Ordering::Release);
    }
}

unsafe extern "system" fn win_event(
    _hook: HWINEVENTHOOK,
    _event: u32,
    _hwnd: HWND,
    _object_id: i32,
    _child_id: i32,
    _thread_id: u32,
    _time: u32,
) {
    FOCUS_CHANGED.store(true, Ordering::Release);
    let owner = OWNER_THREAD.load(Ordering::Acquire);
    if owner != 0
        && let Err(error) = unsafe { PostThreadMessageW(owner, WAKE_MESSAGE, WPARAM(0), LPARAM(0)) }
    {
        crate::log_warning!(
            "windows-events",
            "cannot wake engine for foreground change: {error}"
        );
    }
}
