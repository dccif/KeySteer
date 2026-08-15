//! Event-driven foreground-window notifications.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::HWINEVENTHOOK;

use super::WAKE_MESSAGE;

static FOCUS_CHANGED: AtomicBool = AtomicBool::new(false);
static OWNER_THREAD: AtomicU32 = AtomicU32::new(0);

pub struct ForegroundWatcher(HWINEVENTHOOK);

impl ForegroundWatcher {
    pub fn new(owner_thread: u32) -> Result<Self, String> {
        OWNER_THREAD.store(owner_thread, Ordering::Release);
        let hook = super::native::install_foreground_event_hook(Some(win_event));
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
        if !super::native::uninstall_event_hook(self.0) {
            crate::report_error!(
                "windows-events",
                "UnhookWinEvent failed while stopping foreground notifications"
            );
        }
        OWNER_THREAD.store(0, Ordering::Release);
    }
}

extern "system" fn win_event(
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
        && let Err(error) = super::native::post_thread_message(owner, WAKE_MESSAGE, 0)
    {
        crate::report_error!(
            "windows-events",
            "cannot wake engine for foreground change: {error}"
        );
    }
}
