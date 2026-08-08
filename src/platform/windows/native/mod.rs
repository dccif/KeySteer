//! Minimal Win32 safety boundary for process-wide utilities.

use std::path::Path;

use windows::Win32::Foundation::{HANDLE, HWND};
use windows::Win32::Graphics::Gdi::{HDC, HGDIOBJ};

unsafe extern "C" {
    fn keysteer_compositor_clock_create() -> isize;
    fn keysteer_compositor_clock_wait(stop_event: isize) -> isize;
    fn keysteer_compositor_clock_signal(stop_event: isize) -> isize;
    fn keysteer_compositor_clock_boost(enable: isize) -> isize;
}

enum CompositorCall {
    Create,
    Wait(isize),
    Signal(isize),
    Boost(bool),
}

/// Keep the C ABI and its dynamically resolved Windows 11 functions inside one
/// reviewed native boundary. The bridge normalizes every result to `isize`.
fn compositor_call(call: CompositorCall) -> isize {
    // SAFETY: the C bridge is compiled into this crate with matching signatures.
    // Handle tokens originate from `CreateEventW`, remain owned by
    // `CompositorClockSignal`, and outlive every synchronous call using them.
    unsafe {
        match call {
            CompositorCall::Create => keysteer_compositor_clock_create(),
            CompositorCall::Wait(stop_event) => keysteer_compositor_clock_wait(stop_event),
            CompositorCall::Signal(stop_event) => keysteer_compositor_clock_signal(stop_event),
            CompositorCall::Boost(enable) => keysteer_compositor_clock_boost(enable as isize),
        }
    }
}

/// A process or thread handle that is closed exactly once.
#[repr(transparent)]
struct OwnedHandle(HANDLE);

impl OwnedHandle {
    fn new(handle: HANDLE) -> Self {
        Self(handle)
    }

    fn raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for OwnedHandle {
    #[inline(always)]
    fn drop(&mut self) {
        use windows::Win32::Foundation::CloseHandle;

        // SAFETY: this wrapper is created only from an owned successful handle
        // and Drop is its sole close path.
        if let Err(error) = unsafe { CloseHandle(self.0) } {
            crate::log_warning!("windows-native", "CloseHandle failed: {error}");
        }
    }
}

/// A window created by KeySteer and destroyed on its owner thread.
#[repr(transparent)]
pub(crate) struct OwnedWindow(HWND);

impl OwnedWindow {
    #[inline(always)]
    pub(crate) fn new(hwnd: HWND) -> Self {
        Self(hwnd)
    }

    #[inline(always)]
    pub(crate) fn raw(&self) -> HWND {
        self.0
    }

    #[inline(always)]
    pub(crate) fn destroy(mut self) -> windows::core::Result<()> {
        use windows::Win32::UI::WindowsAndMessaging::DestroyWindow;

        let hwnd = std::mem::take(&mut self.0);
        if hwnd.is_invalid() {
            return Ok(());
        }
        // SAFETY: ownership was transferred into this wrapper and the caller
        // invokes destroy on the thread that created the window.
        unsafe { DestroyWindow(hwnd) }
    }
}

impl Drop for OwnedWindow {
    #[inline(always)]
    fn drop(&mut self) {
        use windows::Win32::UI::WindowsAndMessaging::DestroyWindow;

        if !self.0.is_invalid() {
            // SAFETY: the wrapper is the sole owner and remains on the window
            // thread for its complete lifetime.
            if let Err(error) = unsafe { DestroyWindow(self.0) } {
                crate::log_warning!("windows-native", "DestroyWindow failed: {error}");
            }
        }
    }
}

/// Restores the previously selected GDI object when the guard leaves scope.
pub(crate) struct SelectedObject {
    dc: HDC,
    previous: HGDIOBJ,
}

impl SelectedObject {
    #[inline(always)]
    pub(crate) fn new(dc: HDC, object: HGDIOBJ) -> Result<Self, String> {
        use windows::Win32::Graphics::Gdi::SelectObject;

        // SAFETY: both handles are live for the guard lifetime. Drop restores
        // the exact object returned by this call.
        let previous = unsafe { SelectObject(dc, object) };
        if previous.0.is_null() || previous.0 as usize == usize::MAX {
            Err("SelectObject failed".into())
        } else {
            Ok(Self { dc, previous })
        }
    }
}

impl Drop for SelectedObject {
    #[inline(always)]
    fn drop(&mut self) {
        use windows::Win32::Graphics::Gdi::SelectObject;

        // SAFETY: `previous` came from selecting into this same live DC.
        let _ = unsafe { SelectObject(self.dc, self.previous) };
    }
}

/// Whether Desktop Window Manager composition is available.
pub(crate) fn dwm_composition_enabled() -> windows::core::Result<bool> {
    use windows::Win32::Graphics::Dwm::DwmIsCompositionEnabled;

    // SAFETY: the function has no pointer arguments and returns a BOOL value.
    unsafe { DwmIsCompositionEnabled() }.map(|enabled| enabled.as_bool())
}

/// Block the calling worker until DWM completes the next composition pass.
pub(crate) fn wait_for_dwm_frame() -> windows::core::Result<()> {
    use windows::Win32::Graphics::Dwm::DwmFlush;

    // SAFETY: DwmFlush has no arguments or caller-owned resources.
    unsafe { DwmFlush() }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CompositorWait {
    Frame,
    Interrupted,
    Failed,
}

/// Owner of the event used to interrupt the Windows 11 compositor clock.
pub(crate) struct CompositorClockSignal(OwnedHandle);

impl CompositorClockSignal {
    /// Return `None` on Windows 10 or when the compositor-clock export cannot
    /// be loaded. Callers then retain the DXGI/DWM compatibility path.
    pub(crate) fn try_new() -> Option<Self> {
        let token = compositor_call(CompositorCall::Create);
        (token != 0).then(|| Self(OwnedHandle::new(HANDLE(token as *mut _))))
    }

    pub(crate) fn token(&self) -> isize {
        self.0.raw().0 as isize
    }

    pub(crate) fn interrupt(&self) -> bool {
        compositor_call(CompositorCall::Signal(self.token())) != 0
    }
}

pub(crate) fn wait_for_compositor_frame(stop_event: isize) -> CompositorWait {
    match compositor_call(CompositorCall::Wait(stop_event)) {
        1 => CompositorWait::Frame,
        0 => CompositorWait::Interrupted,
        _ => CompositorWait::Failed,
    }
}

/// Ask Windows 11 to use its high dynamic-refresh cadence while movement is
/// active. Unsupported systems return false and continue normally.
pub(crate) fn boost_compositor_clock(enable: bool) -> bool {
    compositor_call(CompositorCall::Boost(enable)) != 0
}

/// Retained DXGI output selected for display-synchronised movement.
pub(crate) struct DisplayOutput(windows::Win32::Graphics::Dxgi::IDXGIOutput);

impl DisplayOutput {
    /// Block until this output reaches its next vertical blank.
    pub(crate) fn wait_for_vblank(&self) -> windows::core::Result<()> {
        // SAFETY: the retained COM interface stays alive for the synchronous
        // wait and is used only by the frame-clock worker that owns it.
        unsafe { self.0.WaitForVBlank() }
    }
}

/// Prefer actual Windows 11 dynamic-refresh VBlank cadence when supported.
pub(crate) fn prefer_dynamic_vblank() {
    use windows::Win32::Foundation::FreeLibrary;
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    use windows::core::{s, w};

    // SAFETY: dxgi.dll is a system component. The optional export has the same
    // no-argument system ABI as FARPROC, and its ignored HRESULT only reports
    // whether the preference was accepted. FreeLibrary balances our load.
    unsafe {
        let Ok(module) = LoadLibraryW(w!("dxgi.dll")) else {
            return;
        };
        if let Some(disable) = GetProcAddress(module, s!("DXGIDisableVBlankVirtualization")) {
            let _ = disable();
        }
        let _ = FreeLibrary(module);
    }
}

/// Map a desktop point to the nearest monitor without querying refresh rate.
pub(crate) fn monitor_for_point(x: f64, y: f64) -> isize {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONEAREST, MonitorFromPoint};

    let point = POINT {
        x: x.round() as i32,
        y: y.round() as i32,
    };
    // SAFETY: `point` is a value type and nearest-monitor fallback returns a
    // stable HMONITOR whenever a display is attached.
    unsafe { MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST) }.0 as isize
}

/// Find the DXGI output whose native monitor handle matches `monitor`.
pub(crate) fn display_output_for_monitor(monitor: isize) -> Result<Option<DisplayOutput>, String> {
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};
    use windows::Win32::Graphics::Gdi::HMONITOR;

    // SAFETY: DXGI creates retained COM wrappers. Enumeration is read-only,
    // and every adapter/output/factory interface is released by RAII.
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1()
            .map_err(|error| format!("cannot create DXGI factory for frame clock: {error}"))?;
        let monitor = HMONITOR(monitor as *mut _);
        let mut adapter_index = 0;
        while let Ok(adapter) = factory.EnumAdapters1(adapter_index) {
            let mut output_index = 0;
            while let Ok(output) = adapter.EnumOutputs(output_index) {
                if let Ok(description) = output.GetDesc()
                    && description.Monitor == monitor
                {
                    return Ok(Some(DisplayOutput(output)));
                }
                output_index += 1;
            }
            adapter_index += 1;
        }
        Ok(None)
    }
}

/// Wake a thread whose Win32 message queue has already been initialized.
#[inline(always)]
pub(crate) fn post_thread_wake(thread: u32, message: u32) -> windows::core::Result<()> {
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::PostThreadMessageW;

    // SAFETY: the payload contains no pointers and the receiver treats this as
    // a wake-only application message.
    unsafe { PostThreadMessageW(thread, message, WPARAM(0), LPARAM(0)) }
}

/// Return the current foreground window, which may be null while focus changes.
#[inline(always)]
pub(crate) fn foreground_window() -> HWND {
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

    // SAFETY: this call has no arguments and returns a borrowed HWND value.
    unsafe { GetForegroundWindow() }
}

#[inline(always)]
pub(crate) fn current_process_id() -> u32 {
    use windows::Win32::System::Threading::GetCurrentProcessId;

    // SAFETY: this call has no arguments or failure mode.
    unsafe { GetCurrentProcessId() }
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

    // SAFETY: the requested access is query-only and the numeric process id is
    // supplied by Windows.
    let process = OwnedHandle::new(
        unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id) }.ok()?,
    );
    for capacity in [512usize, 32_768] {
        let mut path = vec![0u16; capacity];
        let mut length = capacity as u32;
        // SAFETY: the buffer is writable for `capacity` UTF-16 units and
        // `length` is a valid in/out parameter.
        if unsafe {
            QueryFullProcessImageNameW(
                process.raw(),
                Default::default(),
                PWSTR(path.as_mut_ptr()),
                &mut length,
            )
        }
        .is_ok()
        {
            return std::path::Path::new(&String::from_utf16_lossy(&path[..length as usize]))
                .file_name()
                .map(|name| name.to_string_lossy().into_owned());
        }
    }
    None
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
    use windows::Win32::System::Threading::{
        GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_HIGHEST,
    };

    // SAFETY: The pseudo-handle always identifies the calling thread and does
    // not need closing. `HIGHEST` remains below time-critical priority.
    unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST) }
        .map_err(std::io::Error::other)
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

#[cfg(test)]
mod tests {
    use super::*;
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
    fn compositor_clock_stop_event_is_immediately_interruptible_when_available() {
        let Some(signal) = CompositorClockSignal::try_new() else {
            // Windows 10 intentionally uses the DXGI compatibility path.
            return;
        };

        assert!(signal.interrupt());
        assert_eq!(
            wait_for_compositor_frame(signal.token()),
            CompositorWait::Interrupted
        );
    }
}
