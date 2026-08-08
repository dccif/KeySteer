//! Converts Windows console control events into the engine's normal quit path.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use windows::Win32::Foundation::{FALSE, LPARAM, TRUE, WPARAM};
use windows::Win32::System::Console::{
    CTRL_BREAK_EVENT, CTRL_C_EVENT, CTRL_CLOSE_EVENT, CTRL_LOGOFF_EVENT, CTRL_SHUTDOWN_EVENT,
    SetConsoleCtrlHandler,
};
use windows::Win32::System::Threading::Sleep;
use windows::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, WM_QUIT};
use windows::core::BOOL;

static ENGINE_THREAD: AtomicU32 = AtomicU32::new(0);
static SHUTDOWN_COMPLETE: AtomicBool = AtomicBool::new(false);

pub struct ConsoleControl;

impl ConsoleControl {
    pub fn new(engine_thread: u32) -> Result<Self, String> {
        SHUTDOWN_COMPLETE.store(false, Ordering::Release);
        ENGINE_THREAD.store(engine_thread, Ordering::Release);
        if let Err(error) = unsafe { SetConsoleCtrlHandler(Some(handler), true) } {
            ENGINE_THREAD.store(0, Ordering::Release);
            return Err(format!("SetConsoleCtrlHandler failed: {error}"));
        }
        Ok(Self)
    }

    pub fn mark_shutdown_complete(&self) {
        SHUTDOWN_COMPLETE.store(true, Ordering::Release);
    }
}

impl Drop for ConsoleControl {
    fn drop(&mut self) {
        ENGINE_THREAD.store(0, Ordering::Release);
        if let Err(error) = unsafe { SetConsoleCtrlHandler(Some(handler), false) } {
            crate::log_warning!(
                "windows-console",
                "cannot unregister console control handler: {error}"
            );
        }
    }
}

unsafe extern "system" fn handler(control: u32) -> BOOL {
    if !matches!(
        control,
        CTRL_C_EVENT
            | CTRL_BREAK_EVENT
            | CTRL_CLOSE_EVENT
            | CTRL_LOGOFF_EVENT
            | CTRL_SHUTDOWN_EVENT
    ) {
        return FALSE;
    }
    let thread = ENGINE_THREAD.load(Ordering::Acquire);
    if thread == 0 {
        return FALSE;
    }
    if unsafe { PostThreadMessageW(thread, WM_QUIT, WPARAM(0), LPARAM(0)) }.is_err() {
        return FALSE;
    }
    if matches!(
        control,
        CTRL_CLOSE_EVENT | CTRL_LOGOFF_EVENT | CTRL_SHUTDOWN_EVENT
    ) {
        // Windows may terminate the process shortly after this handler returns.
        // Give the engine a bounded window to release Hook/COM/HWND resources.
        for _ in 0..400 {
            if SHUTDOWN_COMPLETE.load(Ordering::Acquire) {
                break;
            }
            unsafe { Sleep(10) };
        }
    }
    TRUE
}
