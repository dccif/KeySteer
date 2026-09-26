//! Native message delivery and scoped timer/session registration.
use super::OwnedWindow;
use windows::Win32::Foundation::HWND;

/// Wake a thread whose Win32 message queue has already been initialized.
#[inline(always)]
pub(crate) fn post_thread_wake(thread: u32, message: u32) -> windows::core::Result<()> {
    post_thread_message(thread, message, 0)
}

/// Post an integer payload to an initialized Win32 thread message queue.
#[inline(always)]
pub(crate) fn post_thread_message(
    thread: u32,
    message: u32,
    payload: usize,
) -> windows::core::Result<()> {
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::PostThreadMessageW;

    // SAFETY: the payload contains no pointers and the receiver treats this as
    // an integer generation attached to an application-owned message.
    unsafe { PostThreadMessageW(thread, message, WPARAM(payload), LPARAM(0)) }
}

/// Register a process-lifetime window class. Re-registering an existing class
/// is idempotent for independently constructed renderer/tray workers.
pub(crate) fn register_window_class(
    class: &windows::Win32::UI::WindowsAndMessaging::WNDCLASSEXW,
) -> Result<(), String> {
    use windows::Win32::UI::WindowsAndMessaging::RegisterClassExW;

    // SAFETY: callers provide a fully initialized class whose callback and
    // static strings remain alive for the process lifetime.
    if unsafe { RegisterClassExW(class) } != 0 {
        return Ok(());
    }
    let last = windows::core::Error::from_thread();
    if last.code() == windows::core::HRESULT::from_win32(1410) {
        Ok(())
    } else {
        Err(format!("RegisterClassExW failed: {last}"))
    }
}
pub(crate) fn default_window_proc(
    hwnd: HWND,
    message: u32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::UI::WindowsAndMessaging::DefWindowProcW;

    // SAFETY: this forwards the unchanged callback arguments to User32.
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

#[inline(always)]
pub(crate) fn get_window_message(
    message: &mut windows::Win32::UI::WindowsAndMessaging::MSG,
) -> i32 {
    use windows::Win32::UI::WindowsAndMessaging::GetMessageW;

    // SAFETY: `message` is a valid writable out-parameter owned by the caller.
    unsafe { GetMessageW(message, None, 0, 0) }.0
}

/// A message-only timer owned and destroyed on its creating thread.
pub(crate) struct ThreadTimer {
    id: usize,
    _thread: std::marker::PhantomData<std::rc::Rc<()>>,
}

impl ThreadTimer {
    pub(crate) fn new(interval_ms: u32) -> Result<Self, String> {
        // SAFETY: no HWND or callback is retained. Windows allocates the id;
        // this thread-bound guard receives WM_TIMER on the creating thread.
        let id = unsafe {
            windows::Win32::UI::WindowsAndMessaging::SetTimer(None, 0, interval_ms, None)
        };
        if id == 0 {
            return Err("cannot create Windows input recovery timer".into());
        }
        Ok(Self {
            id,
            _thread: std::marker::PhantomData,
        })
    }

    pub(crate) fn matches(&self, message: &windows::Win32::UI::WindowsAndMessaging::MSG) -> bool {
        message.message == windows::Win32::UI::WindowsAndMessaging::WM_TIMER
            && message.hwnd.is_invalid()
            && message.wParam.0 == self.id
    }
}

impl Drop for ThreadTimer {
    fn drop(&mut self) {
        // SAFETY: this guard owns the timer id and cannot leave its thread.
        let result = unsafe { windows::Win32::UI::WindowsAndMessaging::KillTimer(None, self.id) };
        if let Err(error) = result {
            crate::report_error!("windows-hook", "cannot stop recovery timer: {error}");
        }
    }
}

/// Borrows a tray HWND until notifications have been unregistered.
pub(crate) struct SessionNotifications<'a>(&'a OwnedWindow);

impl<'a> SessionNotifications<'a> {
    pub(crate) fn new(window: &'a OwnedWindow) -> Result<Self, String> {
        use windows::Win32::System::RemoteDesktop::{
            NOTIFY_FOR_THIS_SESSION, WTSRegisterSessionNotification,
        };
        // SAFETY: the borrowed HWND remains live until this guard unregisters.
        unsafe { WTSRegisterSessionNotification(window.raw(), NOTIFY_FOR_THIS_SESSION) }
            .map_err(|error| format!("cannot subscribe to session changes: {error}"))?;
        Ok(Self(window))
    }
}

impl Drop for SessionNotifications<'_> {
    fn drop(&mut self) {
        // SAFETY: registration belongs to this guard and its borrowed HWND
        // cannot be destroyed before the matching unregistration.
        if let Err(error) = unsafe {
            windows::Win32::System::RemoteDesktop::WTSUnRegisterSessionNotification(self.0.raw())
        } {
            crate::report_error!(
                "windows-events",
                "cannot unsubscribe from session changes: {error}"
            );
        }
    }
}
