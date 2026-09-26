//! Native boundary entry point; resource owners live in focused modules.
mod capture;
mod com;
mod compositor;
mod dimensions;
mod gdi;
mod handles;
mod message_loop;
mod ocr_bridge;
mod uia_cache;
mod window;
mod winrt;
pub(crate) use capture::PreparedCapture;
pub(crate) use com::ComApartment;
pub(crate) use compositor::{
    CompositorClockSignal, CompositorWait, DisplayOutput, boost_compositor_clock,
    display_output_for_monitor, dwm_composition_enabled, interrupt_compositor_clock,
    monitor_for_point, prefer_dynamic_vblank, wait_for_compositor_frame, wait_for_dwm_frame,
};
pub(crate) use dimensions::NativeDimensions;
pub(crate) use gdi::{GdiDibSurface, OwnedFont, ScreenDc};
pub(crate) use handles::KillOnCloseJob;
use handles::OwnedHandle;
pub(crate) use message_loop::{
    SessionNotifications, ThreadTimer, default_window_proc, get_window_message,
    post_thread_message, post_thread_wake, register_window_class,
};
pub(crate) use ocr_bridge::WechatBridge;
use std::path::Path;
pub(crate) use uia_cache::CachedElement;
pub(crate) use window::{
    OwnedWindow, OwnedWindowSpec, create_owned_window, reposition_owned_window,
};
use windows::Win32::Foundation::{HANDLE, HWND};
#[cfg(test)]
pub(crate) use winrt::create_system_ocr_engine;
pub(crate) use winrt::{
    SoftwareBitmapFactory, SystemOcrFactory, create_file_random_access_stream,
    create_png_bitmap_encoder_operation, probe_system_ocr_factory,
};

#[inline(always)]
pub(crate) fn window_long(
    hwnd: HWND,
    index: windows::Win32::UI::WindowsAndMessaging::WINDOW_LONG_PTR_INDEX,
) -> i32 {
    use windows::Win32::UI::WindowsAndMessaging::GetWindowLongW;

    // SAFETY: this reads one documented integer field from a borrowed HWND.
    unsafe { GetWindowLongW(hwnd, index) }
}

#[inline(always)]
pub(crate) fn call_next_hook(
    code: i32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::UI::WindowsAndMessaging::CallNextHookEx;

    // SAFETY: the low-level hook forwards the original callback arguments
    // unchanged and retains no pointer from `lparam`.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

pub(crate) fn set_console_control_handler(
    handler: windows::Win32::System::Console::PHANDLER_ROUTINE,
    enabled: bool,
) -> windows::core::Result<()> {
    use windows::Win32::System::Console::SetConsoleCtrlHandler;

    // SAFETY: the callback has process lifetime and the same value is used to
    // unregister it before backend teardown completes.
    unsafe { SetConsoleCtrlHandler(handler, enabled) }
}

pub(crate) fn install_foreground_event_hook(
    callback: windows::Win32::UI::Accessibility::WINEVENTPROC,
) -> windows::Win32::UI::Accessibility::HWINEVENTHOOK {
    use windows::Win32::UI::Accessibility::SetWinEventHook;
    use windows::Win32::UI::WindowsAndMessaging::{
        EVENT_SYSTEM_FOREGROUND, WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS,
    };

    // SAFETY: the callback accesses only process-lifetime atomics and the
    // returned owned hook is uninstalled by the matching wrapper below.
    unsafe {
        SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            None,
            callback,
            0,
            0,
            WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
        )
    }
}

pub(crate) fn uninstall_event_hook(hook: windows::Win32::UI::Accessibility::HWINEVENTHOOK) -> bool {
    use windows::Win32::UI::Accessibility::UnhookWinEvent;

    // SAFETY: `hook` came from `install_foreground_event_hook` and is consumed
    // once during owner Drop.
    unsafe { UnhookWinEvent(hook) }.as_bool()
}

/// Return the current foreground window, which may be null while focus changes.
#[inline(always)]
pub(crate) fn foreground_window() -> HWND {
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

    // SAFETY: this call has no arguments and returns a borrowed HWND value.
    unsafe { GetForegroundWindow() }
}

/// Submit activation of a borrowed identity; completion is confirmed by the worker.
pub(crate) fn try_activate_window(hwnd: HWND) -> bool {
    // SAFETY: the system validates the borrowed HWND; no Rust pointer or
    // ownership is transferred, and no foreign input queues are attached.
    unsafe { windows::Win32::UI::WindowsAndMessaging::SetForegroundWindow(hwnd) }.as_bool()
}

#[inline(always)]
pub(crate) fn current_process_id() -> u32 {
    use windows::Win32::System::Threading::GetCurrentProcessId;

    // SAFETY: this call has no arguments or failure mode.
    unsafe { GetCurrentProcessId() }
}

#[inline(always)]
pub(crate) fn current_thread_id() -> u32 {
    use windows::Win32::System::Threading::GetCurrentThreadId;

    // SAFETY: this call has no arguments or failure mode.
    unsafe { GetCurrentThreadId() }
}

#[inline(always)]
pub(crate) fn current_module() -> windows::core::Result<windows::Win32::Foundation::HMODULE> {
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;

    // SAFETY: a null module name requests the current executable module.
    unsafe { GetModuleHandleW(None) }
}

#[inline(always)]
pub(crate) fn is_window_visible(hwnd: HWND) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::IsWindowVisible;

    // SAFETY: the borrowed HWND is used only for this synchronous query.
    unsafe { IsWindowVisible(hwnd) }.as_bool()
}

#[inline(always)]
pub(crate) fn is_window_iconic(hwnd: HWND) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::IsIconic;

    // SAFETY: the borrowed HWND is used only for this synchronous query.
    unsafe { IsIconic(hwnd) }.as_bool()
}

#[inline(always)]
pub(crate) fn is_window(hwnd: HWND) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::IsWindow;

    // SAFETY: the borrowed HWND is used only for this synchronous query.
    unsafe { IsWindow(Some(hwnd)) }.as_bool()
}

#[inline(always)]
pub(crate) fn root_owner(hwnd: HWND) -> HWND {
    use windows::Win32::UI::WindowsAndMessaging::{GA_ROOTOWNER, GetAncestor};

    // SAFETY: the borrowed HWND is queried synchronously; the result is also
    // a borrowed handle and retains no Rust data.
    unsafe { GetAncestor(hwnd, GA_ROOTOWNER) }
}

#[inline(always)]
pub(crate) fn root_window(hwnd: HWND) -> HWND {
    use windows::Win32::UI::WindowsAndMessaging::{GA_ROOT, GetAncestor};
    // SAFETY: synchronous parent-chain query; neither borrowed HWND is retained.
    unsafe { GetAncestor(hwnd, GA_ROOT) }
}

#[inline(always)]
pub(crate) fn desktop_window() -> HWND {
    use windows::Win32::UI::WindowsAndMessaging::GetDesktopWindow;

    // SAFETY: this returns a borrowed process-global desktop handle.
    unsafe { GetDesktopWindow() }
}

#[inline(always)]
pub(crate) fn window_from_point(point: windows::Win32::Foundation::POINT) -> HWND {
    use windows::Win32::UI::WindowsAndMessaging::WindowFromPoint;

    // SAFETY: the point is a plain value and the result is a borrowed HWND.
    unsafe { WindowFromPoint(point) }
}

pub(crate) fn window_class_name(hwnd: HWND, buffer: &mut [u16]) -> usize {
    use windows::Win32::UI::WindowsAndMessaging::GetClassNameW;

    // SAFETY: `buffer` is writable for its full length and `hwnd` is borrowed
    // for this synchronous query.
    unsafe { GetClassNameW(hwnd, buffer) }.max(0) as usize
}

pub(crate) fn enum_windows(
    callback: windows::Win32::UI::WindowsAndMessaging::WNDENUMPROC,
    data: windows::Win32::Foundation::LPARAM,
) -> windows::core::Result<()> {
    use windows::Win32::UI::WindowsAndMessaging::EnumWindows;

    // SAFETY: callers keep callback state alive for the complete synchronous
    // enumeration and provide a callback with the documented ABI.
    unsafe { EnumWindows(callback, data) }
}

/// Return a window's creating thread and optionally its owning process.
#[inline(always)]
pub(crate) fn window_thread_process_id(hwnd: HWND, process_id: Option<&mut u32>) -> u32 {
    use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;

    // SAFETY: the optional process id is a valid out-parameter for this call.
    unsafe { GetWindowThreadProcessId(hwnd, process_id.map(std::ptr::from_mut)) }
}

pub(crate) fn window_title(hwnd: HWND) -> String {
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowTextLengthW, GetWindowTextW};

    // SAFETY: the HWND is borrowed for the synchronous query.
    let length = unsafe { GetWindowTextLengthW(hwnd) }.max(0) as usize;
    let mut title = vec![0u16; length.saturating_add(1)];
    // SAFETY: the UTF-16 buffer is writable and includes room for the trailing
    // NUL requested by GetWindowTextW.
    let copied = unsafe { GetWindowTextW(hwnd, &mut title) }.max(0) as usize;
    String::from_utf16_lossy(&title[..copied])
}

pub(crate) fn process_executable_name(process_id: u32) -> Option<String> {
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    };
    use windows::core::PWSTR;

    // SAFETY: access is query-only. Every UTF-16 buffer is writable for its
    // advertised length, and OwnedHandle closes the successful process handle.
    unsafe {
        let process = OwnedHandle::new(
            OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id).ok()?,
        );
        for capacity in [512usize, 32_768] {
            let mut path = vec![0u16; capacity];
            let mut length = capacity as u32;
            if QueryFullProcessImageNameW(
                process.raw(),
                Default::default(),
                PWSTR(path.as_mut_ptr()),
                &mut length,
            )
            .is_ok()
            {
                return std::path::Path::new(&String::from_utf16_lossy(&path[..length as usize]))
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned());
            }
        }
        None
    }
}

fn integrity_name(rid: u32) -> &'static str {
    match rid {
        0x0000..=0x0FFF => "untrusted",
        0x1000..=0x1FFF => "low",
        0x2000..=0x20FF => "medium",
        0x2100..=0x2FFF => "medium-plus",
        0x3000..=0x3FFF => "high",
        0x4000..=0x4FFF => "system",
        0x5000.. => "protected",
    }
}

/// Expensive context captured only after `SendInput` has already failed.
/// This deliberately lives in the native boundary so token handles and
/// read-only process handles cannot leak into the portable input code.
pub(crate) fn send_input_failure_context(last_error: u32, input_size: usize) -> String {
    use windows::Win32::Security::{
        GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TOKEN_MANDATORY_LABEL,
        TOKEN_QUERY, TokenElevation, TokenIntegrityLevel, TokenUIAccess,
    };
    use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
    use windows::Win32::System::Threading::{
        OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    let error = if last_error == 0 {
        "not set (UIPI may leave it unset)".into()
    } else {
        format!("{}", std::io::Error::from_raw_os_error(last_error as i32))
    };
    let current_pid = current_process_id();
    let foreground = foreground_window();
    let mut foreground_pid = 0u32;
    let foreground_thread = window_thread_process_id(foreground, Some(&mut foreground_pid));

    // SAFETY: every native call below is read-only. Out-parameters point to
    // correctly sized live storage; query handles are immediately wrapped and
    // closed once this failure-only diagnostic snapshot is formatted. The SID
    // pointers originate from a successful TOKEN_MANDATORY_LABEL query and do
    // not outlive its aligned backing allocation.
    let (current_session, current_security, foreground_session, foreground_security) = unsafe {
        let process_session = |process_id: u32| -> Option<u32> {
            let mut session_id = 0;
            ProcessIdToSessionId(process_id, &mut session_id)
                .ok()
                .map(|()| session_id)
        };
        let token_u32 = |token: HANDLE,
                         class: windows::Win32::Security::TOKEN_INFORMATION_CLASS|
         -> Option<u32> {
            let mut value = 0u32;
            let mut returned = 0u32;
            GetTokenInformation(
                token,
                class,
                Some((&mut value as *mut u32).cast()),
                std::mem::size_of::<u32>() as u32,
                &mut returned,
            )
            .ok()
            .map(|()| value)
        };
        let token_integrity = |token: HANDLE| -> Option<u32> {
            let mut required = 0u32;
            let _ = GetTokenInformation(token, TokenIntegrityLevel, None, 0, &mut required);
            if required < std::mem::size_of::<TOKEN_MANDATORY_LABEL>() as u32 {
                return None;
            }
            let words = (required as usize).div_ceil(std::mem::size_of::<usize>());
            let mut storage = vec![0usize; words];
            let mut returned = 0u32;
            GetTokenInformation(
                token,
                TokenIntegrityLevel,
                Some(storage.as_mut_ptr().cast()),
                required,
                &mut returned,
            )
            .ok()?;
            if returned < std::mem::size_of::<TOKEN_MANDATORY_LABEL>() as u32 {
                return None;
            }
            let sid = (*(storage.as_ptr().cast::<TOKEN_MANDATORY_LABEL>()))
                .Label
                .Sid;
            if sid.is_invalid() {
                return None;
            }
            let count = GetSidSubAuthorityCount(sid).as_ref().copied()?;
            if count == 0 {
                return None;
            }
            GetSidSubAuthority(sid, u32::from(count - 1))
                .as_ref()
                .copied()
        };
        let process_security = |process_id: u32| -> String {
            let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id)
            else {
                return "security=unavailable".into();
            };
            let process = OwnedHandle::new(process);
            let mut token = HANDLE::default();
            if OpenProcessToken(process.raw(), TOKEN_QUERY, &mut token).is_err() {
                return "security=unavailable".into();
            }
            let token = OwnedHandle::new(token);
            let integrity = token_integrity(token.raw())
                .map(|rid| format!("{}(0x{rid:04X})", integrity_name(rid)))
                .unwrap_or_else(|| "unknown".into());
            let elevated = token_u32(token.raw(), TokenElevation)
                .map(|value| value != 0)
                .map_or_else(|| "unknown".into(), |value| value.to_string());
            let ui_access = token_u32(token.raw(), TokenUIAccess)
                .map(|value| value != 0)
                .map_or_else(|| "unknown".into(), |value| value.to_string());
            format!("integrity={integrity}, elevated={elevated}, ui_access={ui_access}")
        };

        (
            process_session(current_pid),
            process_security(current_pid),
            (foreground_pid != 0)
                .then(|| process_session(foreground_pid))
                .flatten(),
            (foreground_pid != 0).then(|| process_security(foreground_pid)),
        )
    };
    let current_session =
        current_session.map_or_else(|| "unknown".into(), |value| value.to_string());
    let foreground_context = if foreground.is_invalid() || foreground_pid == 0 {
        "foreground=none".into()
    } else {
        let executable =
            process_executable_name(foreground_pid).unwrap_or_else(|| "unknown".into());
        let session =
            foreground_session.map_or_else(|| "unknown".into(), |value| value.to_string());
        let security = foreground_security.unwrap_or_else(|| "security=unavailable".into());
        format!(
            "foreground={{hwnd=0x{:X}, thread={}, pid={}, exe={:?}, session={}, {}}}",
            foreground.0 as usize, foreground_thread, foreground_pid, executable, session, security
        )
    };

    format!(
        "last_error=0x{last_error:08X} ({error}), input_size={input_size}, pointer_width={}, current={{pid={current_pid}, session={current_session}, {current_security}}}, {foreground_context}",
        usize::BITS
    )
}

#[inline(always)]
pub(crate) fn apps_use_light_theme() -> bool {
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
    use windows::core::w;

    let mut value = 0u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: both out-parameters are correctly sized and live for the
    // synchronous registry query.
    let result = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut value as *mut u32).cast()),
            Some(&mut size),
        )
    };
    result.is_ok() && value != 0
}

#[inline(always)]
pub(crate) fn wait_for_input(timeout_ms: u32) {
    use windows::Win32::UI::WindowsAndMessaging::{MsgWaitForMultipleObjects, QS_ALLINPUT};

    // SAFETY: no handles are supplied, so Windows waits only on queued input.
    unsafe {
        MsgWaitForMultipleObjects(None, false, timeout_ms, QS_ALLINPUT);
    }
}

/// Create the current thread's Win32 message queue and return its thread ID.
///
/// `PostThreadMessageW` fails until a thread has called a User32 message API,
/// so workers publish their ID only after this function returns.
pub(crate) fn prepare_thread_message_queue() -> u32 {
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::WindowsAndMessaging::{MSG, PM_NOREMOVE, PeekMessageW};

    let mut probe = MSG::default();
    // SAFETY: `probe` is a valid out-parameter. PM_NOREMOVE initializes the
    // queue without consuming a pending message.
    unsafe {
        let _ = PeekMessageW(&mut probe, None, 0, 0, PM_NOREMOVE);
        GetCurrentThreadId()
    }
}

/// Block for and dispatch one message on the current window-owning thread.
/// Returns `false` for `WM_QUIT`.
pub(crate) fn wait_and_dispatch_window_message() -> windows::core::Result<bool> {
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetMessageW, MSG, TranslateMessage,
    };

    let mut message = MSG::default();
    // SAFETY: `message` is a valid out-parameter. A positive GetMessageW
    // result fully initializes it for TranslateMessage/DispatchMessageW.
    unsafe {
        match GetMessageW(&mut message, None, 0, 0).0 {
            0 => Ok(false),
            -1 => Err(windows::core::Error::from_thread()),
            _ => {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
                Ok(true)
            }
        }
    }
}

/// Make an overlay HWND transparent to mouse hit testing.
pub(crate) fn click_through_hit_test(message: u32) -> Option<windows::Win32::Foundation::LRESULT> {
    use windows::Win32::Foundation::LRESULT;
    use windows::Win32::UI::WindowsAndMessaging::{HTTRANSPARENT, WM_NCHITTEST};

    (message == WM_NCHITTEST).then_some(LRESULT(HTTRANSPARENT as isize))
}

#[cfg(test)]
pub(crate) struct OverlayProbe {
    pub(crate) hit_test: windows::Win32::Foundation::LRESULT,
    pub(crate) ex_style: u32,
}

#[cfg(test)]
pub(crate) fn probe_overlay_hit_test(
    class_names: &[windows::core::PCWSTR],
    timeout_ms: u32,
) -> windows::core::Result<Option<OverlayProbe>> {
    use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        FindWindowW, GWL_EXSTYLE, GetWindowLongW, SMTO_ABORTIFHUNG, SendMessageTimeoutW,
        WM_NCHITTEST,
    };

    // SAFETY: class names are static NUL-terminated strings. A successfully
    // found HWND is used synchronously, and `result` is a valid out-parameter.
    unsafe {
        for class_name in class_names {
            let Ok(hwnd) = FindWindowW(*class_name, None) else {
                continue;
            };
            let mut result = 0usize;
            if SendMessageTimeoutW(
                hwnd,
                WM_NCHITTEST,
                WPARAM(0),
                LPARAM(0),
                SMTO_ABORTIFHUNG,
                timeout_ms,
                Some(&mut result),
            )
            .0 == 0
            {
                return Err(windows::core::Error::from_thread());
            }
            return Ok(Some(OverlayProbe {
                hit_test: LRESULT(result as isize),
                ex_style: GetWindowLongW(hwnd, GWL_EXSTYLE) as u32,
            }));
        }
        Ok(None)
    }
}

/// Drain the current render thread's window messages.
#[inline(always)]
pub(crate) fn pump_window_messages() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, TranslateMessage, WM_QUIT,
    };

    let mut message = MSG::default();
    // SAFETY: `message` is a valid out-parameter. Each successful PeekMessageW
    // initializes it before translation and dispatch.
    unsafe {
        while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
            if message.message == WM_QUIT {
                return false;
            }
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    true
}

/// Prefer the engine's synchronous input work without entering real-time
/// priority classes that could starve the compositor or system services.
pub(crate) fn prefer_input_latency() -> std::io::Result<()> {
    use windows::Win32::System::Threading::THREAD_PRIORITY_HIGHEST;

    set_current_thread_priority(THREAD_PRIORITY_HIGHEST)
}

/// Keep OCR and pixel analysis below interactive input/compositor work.
pub(crate) fn prefer_background_work() -> std::io::Result<()> {
    use windows::Win32::System::Threading::THREAD_PRIORITY_BELOW_NORMAL;

    set_current_thread_priority(THREAD_PRIORITY_BELOW_NORMAL)
}

fn set_current_thread_priority(
    priority: windows::Win32::System::Threading::THREAD_PRIORITY,
) -> std::io::Result<()> {
    use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority};

    // SAFETY: The pseudo-handle always identifies the calling thread and does
    // not need closing. Callers select non-realtime documented priorities.
    unsafe { SetThreadPriority(GetCurrentThread(), priority) }.map_err(std::io::Error::other)
}

/// Attach the parent console when possible, otherwise allocate one.
pub(crate) fn prepare_console_for_cli() {
    use windows::Win32::System::Console::{ATTACH_PARENT_PROCESS, AllocConsole, AttachConsole};

    // SAFETY: Both functions take no borrowed pointers. Failure to attach is
    // expected for Explorer launches and is handled by allocating a console.
    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS).is_err() {
            let _ = AllocConsole();
        }
    }
}

/// Atomically replace `destination` with an already-written temporary file.
pub(crate) fn atomic_replace(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };
    use windows::core::PCWSTR;

    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    // SAFETY: Both UTF-16 buffers are NUL-terminated and remain alive for the
    // complete synchronous call. Flags request an atomic durable replacement.
    unsafe {
        MoveFileExW(
            PCWSTR(source.as_ptr()),
            PCWSTR(destination.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    }
    .map_err(std::io::Error::other)
}

/// Keep a process identity lease alive throughout the operation. Audio requests
/// cross workers as PID + creation time, never as HWND/COM references.
pub(crate) fn with_process_identity<T>(
    pid: u32,
    operation: impl FnOnce(u64) -> Result<T, String>,
) -> Result<T, String> {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::{
        GetExitCodeProcess, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    // SAFETY: a query-only owned handle is held until operation returns. All
    // outputs are initialized and bounded; the existing RAII owner closes it.
    let (process, started) = unsafe {
        let process = OwnedHandle::new(
            OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)
                .map_err(|e| e.to_string())?,
        );
        let (mut created, mut exited, mut kernel, mut user) = (
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
        );
        GetProcessTimes(
            process.raw(),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        )
        .map_err(|e| e.to_string())?;
        let mut code = 0;
        GetExitCodeProcess(process.raw(), &mut code).map_err(|e| e.to_string())?;
        if code != 259 {
            return Err("Application has exited".into());
        }
        (
            process,
            (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime),
        )
    };
    let result = operation(started);
    drop(process);
    result
}

/// Own the Toolhelp snapshot for the entire enumeration, including error/unwind
/// paths. Callers receive values and cannot retain or close the native handle.
pub(crate) fn audio_process_list() -> windows::core::Result<Vec<(u32, u32, String)>> {
    use windows::Win32::Foundation::ERROR_NO_MORE_FILES;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };
    // SAFETY: the read-only snapshot is owned by the existing RAII handle. Each
    // write targets an initialized, correctly sized stack entry. No native
    // pointer or handle escapes; only ERROR_NO_MORE_FILES means normal EOF.
    unsafe {
        let snapshot = OwnedHandle::new(CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)?);
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut result = Vec::new();
        let mut next = Process32FirstW(snapshot.raw(), &mut entry);
        loop {
            match next {
                Ok(()) => {}
                Err(error) if error.code() == ERROR_NO_MORE_FILES.to_hresult() => {
                    return Ok(result);
                }
                Err(error) => return Err(error),
            }
            let end = entry
                .szExeFile
                .iter()
                .position(|c| *c == 0)
                .unwrap_or(entry.szExeFile.len());
            result.push((
                entry.th32ProcessID,
                entry.th32ParentProcessID,
                String::from_utf16_lossy(&entry.szExeFile[..end]),
            ));
            next = Process32NextW(snapshot.raw(), &mut entry);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apartment_and_graphics_owners_cannot_cross_threads() {
        // If a type implements Send or Sync, inference becomes ambiguous and
        // compilation fails. These checks require no runtime allocation.
        trait AmbiguousIfSend<A> {
            fn check() {}
        }
        impl<T: ?Sized> AmbiguousIfSend<()> for T {}
        struct IsSend;
        impl<T: ?Sized + Send> AmbiguousIfSend<IsSend> for T {}
        trait AmbiguousIfSync<A> {
            fn check() {}
        }
        impl<T: ?Sized> AmbiguousIfSync<()> for T {}
        struct IsSync;
        impl<T: ?Sized + Sync> AmbiguousIfSync<IsSync> for T {}
        macro_rules! thread_bound {
            ($($owner:ty),+ $(,)?) => { $(
                let _ = <$owner as AmbiguousIfSend<_>>::check;
                let _ = <$owner as AmbiguousIfSync<_>>::check;
            )+ };
        }
        thread_bound!(
            ComApartment,
            ScreenDc,
            GdiDibSurface,
            PreparedCapture,
            OwnedWindow,
            SoftwareBitmapFactory,
            SystemOcrFactory
        );
    }
    use windows::Win32::UI::WindowsAndMessaging::{HTTRANSPARENT, WM_NCHITTEST, WM_PAINT};

    #[test]
    fn overlays_never_claim_mouse_hit_tests() {
        assert_eq!(
            click_through_hit_test(WM_NCHITTEST),
            Some(windows::Win32::Foundation::LRESULT(HTTRANSPARENT as isize))
        );
        assert_eq!(click_through_hit_test(WM_PAINT), None);
    }

    #[test]
    fn integrity_rids_are_labeled_for_input_diagnostics() {
        assert_eq!(integrity_name(0x1000), "low");
        assert_eq!(integrity_name(0x2000), "medium");
        assert_eq!(integrity_name(0x2100), "medium-plus");
        assert_eq!(integrity_name(0x3000), "high");
        assert_eq!(integrity_name(0x4000), "system");
        assert_eq!(integrity_name(0x5000), "protected");
    }

    #[test]
    fn compositor_clock_stop_event_is_immediately_interruptible_when_available() {
        let Some(signal) = CompositorClockSignal::try_new() else {
            // Windows 10 intentionally uses the DXGI compatibility path.
            return;
        };

        assert!(interrupt_compositor_clock(signal.token()));
        assert_eq!(
            wait_for_compositor_frame(signal.token()),
            CompositorWait::Interrupted
        );
    }

    #[test]
    #[ignore = "requires an interactive Windows desktop"]
    fn visual_capture_prepares_and_releases_thread_bound_surface() -> Result<(), String> {
        let mut capture = PreparedCapture::new(64, 64)?;
        let first = capture.capture_with(0, 0, 64, 64, |pixels, width, height| {
            Ok((pixels.len(), width, height))
        })?;
        let second = capture.capture_with(0, 0, 64, 64, |pixels, width, height| {
            Ok((pixels.len(), width, height))
        })?;
        drop(capture);
        assert_eq!(first, (64 * 64 * 4, 64, 64));
        assert_eq!(second, first);
        Ok(())
    }

    #[test]
    #[ignore = "requires the optional Windows OCR capability"]
    fn system_ocr_factory_survives_transient_apartments() -> Result<(), String> {
        for _ in 0..3 {
            std::thread::spawn(|| -> Result<(), String> {
                let apartment = ComApartment::initialise()?;
                let (maximum, _) = probe_system_ocr_factory()?;
                let engine = create_system_ocr_engine()?;
                assert!(maximum > 0);
                drop(engine);
                drop(apartment);
                Ok(())
            })
            .join()
            .map_err(|_| "transient OCR apartment thread panicked".to_string())??;
        }
        Ok(())
    }
}
