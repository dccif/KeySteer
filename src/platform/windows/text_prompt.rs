//! Modeless, IME-capable note entry owned by the existing tray UI thread.
use crate::api::{backend::BackendEvent, window_presets::TextPrompt};
use std::cell::RefCell;
use std::sync::Mutex;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::*;

pub(super) const MESSAGE: u32 = WM_APP + 0x51;
enum Job {
    Show(TextPrompt),
    Cancel(u64),
}
static JOBS: Mutex<Vec<Job>> = Mutex::new(Vec::new());
struct Active {
    request: TextPrompt,
    hwnd: HWND,
}
impl Drop for Active {
    fn drop(&mut self) {
        // SAFETY: Active is created and dropped only on the owning tray thread.
        if let Err(error) = unsafe { DestroyWindow(self.hwnd) } {
            crate::report_error!("windows-dialog", "Cannot destroy layout input: {error}");
        }
    }
}
thread_local! { static ACTIVE: RefCell<Option<Active>> = const { RefCell::new(None) }; }

pub(super) fn request(owner: HWND, prompt: TextPrompt) -> Result<(), String> {
    enqueue(owner, Job::Show(prompt))
}
pub(super) fn cancel(owner: HWND, id: u64) -> Result<(), String> {
    enqueue(owner, Job::Cancel(id))
}
fn enqueue(owner: HWND, job: Job) -> Result<(), String> {
    let mut jobs = JOBS.lock().unwrap_or_else(|e| e.into_inner());
    jobs.push(job);
    // SAFETY: the live tray owner receives a payload-free wake; jobs own their strings.
    if let Err(error) = unsafe { PostMessageW(Some(owner), MESSAGE, WPARAM(0), LPARAM(0)) } {
        jobs.pop();
        return Err(format!("Cannot wake layout note dialog: {error}"));
    }
    Ok(())
}
pub(super) fn process() {
    let jobs = std::mem::take(&mut *JOBS.lock().unwrap_or_else(|e| e.into_inner()));
    for job in jobs {
        match job {
            Job::Show(prompt) => {
                close(None);
                let id = prompt.id;
                if let Err(error) = show(prompt) {
                    ACTIVE.with(|a| a.borrow_mut().take());
                    super::status_item::emit(BackendEvent::TextPromptResult {
                        id,
                        value: Err(error),
                    });
                }
            }
            Job::Cancel(id) => {
                if ACTIVE.with(|a| a.borrow().as_ref().is_some_and(|a| a.request.id == id)) {
                    close(None);
                }
            }
        }
    }
}
pub(super) fn dispatch(message: &MSG) -> bool {
    let hwnd = ACTIVE.with(|a| a.borrow().as_ref().map(|a| a.hwnd));
    // SAFETY: dialog and message belong to this thread. No RefCell borrow spans callbacks.
    hwnd.is_some_and(|hwnd| unsafe { IsDialogMessageW(hwnd, message).as_bool() })
}
pub(super) fn shutdown() {
    JOBS.lock().unwrap_or_else(|e| e.into_inner()).clear();
    close(None);
}
fn close(value: Option<String>) {
    if let Some(active) = ACTIVE.with(|a| a.borrow_mut().take()) {
        let id = active.request.id;
        drop(active);
        super::status_item::emit(BackendEvent::TextPromptResult {
            id,
            value: Ok(value),
        });
    }
}
fn show(request: TextPrompt) -> Result<(), String> {
    let mut words = Vec::<u16>::new();
    fn dword(words: &mut Vec<u16>, value: u32) {
        words.extend([value as u16, (value >> 16) as u16]);
    }
    fn string(words: &mut Vec<u16>, value: &str) {
        words.extend(value.encode_utf16());
        words.push(0);
    }
    dword(&mut words, WS_POPUP.0 | DS_SETFONT as u32);
    dword(&mut words, (WS_EX_TOOLWINDOW | WS_EX_TOPMOST).0);
    words.extend([4, 0, 0, 300, 100, 0, 0]);
    string(&mut words, &request.title);
    words.push(9);
    string(&mut words, "Segoe UI");
    for (id, class, text, x, y, width, height, extra) in [
        (100u16, 0x82, request.message.as_str(), 10, 8, 280, 26, 0),
        (
            101,
            0x81,
            "",
            10,
            37,
            280,
            16,
            ES_AUTOHSCROLL as u32 | WS_BORDER.0 | WS_TABSTOP.0,
        ),
        (
            1,
            0x80,
            "Save",
            176,
            72,
            54,
            18,
            BS_DEFPUSHBUTTON as u32 | WS_TABSTOP.0,
        ),
        (2, 0x80, "Cancel", 236, 72, 54, 18, WS_TABSTOP.0),
    ] {
        if !words.len().is_multiple_of(2) {
            words.push(0);
        }
        dword(&mut words, (WS_CHILD | WS_VISIBLE).0 | extra);
        dword(&mut words, 0);
        words.extend([x, y, width, height, id, 0xffff, class]);
        string(&mut words, text);
        words.push(0);
    }
    if !words.len().is_multiple_of(2) {
        words.push(0);
    }
    let aligned: Vec<u32> = words
        .as_chunks::<2>()
        .0
        .iter()
        .map(|p| u32::from(p[0]) | (u32::from(p[1]) << 16))
        .collect();
    // SAFETY: DWORD-aligned DLGTEMPLATE and all variable entries remain live during
    // synchronous creation. The system copies them; callback retains no pointers.
    let hwnd = unsafe {
        CreateDialogIndirectParamW(
            None,
            aligned.as_ptr().cast(),
            None,
            Some(dialog_proc),
            LPARAM(0),
        )
    }
    .map_err(|e| format!("Cannot create layout note dialog: {e}"))?;
    let placeholder: Vec<u16> = request
        .placeholder
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let max_units = request.max_chars.saturating_mul(2);
    let bounds = request.bounds;
    ACTIVE.with(|a| *a.borrow_mut() = Some(Active { request, hwnd }));
    // SAFETY: this thread owns the newly created dialog and its edit child.
    unsafe {
        SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            bounds.x.round() as i32,
            bounds.y.round() as i32,
            bounds.width.round() as i32,
            bounds.height.round() as i32,
            SWP_NOACTIVATE,
        )
        .map_err(|e| format!("Cannot position layout input: {e}"))?;
        let unit = bounds.height / 100.0;
        for (id, x, y, width, height) in [
            (100, 12.0, 8.0, bounds.width / unit - 24.0, 28.0),
            (101, 12.0, 46.0, bounds.width / unit - 204.0, 34.0),
            (1, bounds.width / unit - 180.0, 46.0, 78.0, 34.0),
            (2, bounds.width / unit - 90.0, 46.0, 78.0, 34.0),
        ] {
            let child = GetDlgItem(Some(hwnd), id)
                .map_err(|e| format!("Cannot locate layout input: {e}"))?;
            MoveWindow(
                child,
                (x * unit) as i32,
                (y * unit) as i32,
                (width.max(1.0) * unit) as i32,
                (height * unit) as i32,
                true,
            )
            .map_err(|e| format!("Cannot size layout input: {e}"))?;
        }
        SendDlgItemMessageW(
            hwnd,
            101,
            windows::Win32::UI::Controls::EM_LIMITTEXT,
            WPARAM(max_units),
            LPARAM(0),
        );
        SendDlgItemMessageW(
            hwnd,
            101,
            windows::Win32::UI::Controls::EM_SETCUEBANNER,
            WPARAM(0),
            LPARAM(placeholder.as_ptr() as isize),
        );
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
    }
    Ok(())
}

extern "system" fn dialog_proc(hwnd: HWND, message: u32, wparam: WPARAM, _: LPARAM) -> isize {
    match message {
        WM_INITDIALOG => 1,
        WM_CLOSE => {
            close(None);
            1
        }
        WM_COMMAND if wparam.0 & 0xffff == 2 => {
            close(None);
            1
        }
        WM_COMMAND if wparam.0 & 0xffff == 1 => {
            let max = ACTIVE.with(|a| a.borrow().as_ref().map_or(80, |a| a.request.max_chars));
            let mut text = vec![0u16; max.saturating_mul(2).saturating_add(1)];
            // SAFETY: edit 101 is owned by this dialog; output slice is writable and bounded.
            let len = unsafe { GetDlgItemTextW(hwnd, 101, &mut text) } as usize;
            close(Some(
                String::from_utf16_lossy(&text[..len])
                    .chars()
                    .take(max)
                    .collect(),
            ));
            1
        }
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "Creates disposable native note dialogs; run explicitly on an interactive desktop"]
    fn native_note_dialog_preserves_unicode_and_cancels_owned_windows() {
        let (sender, receiver) = std::sync::mpsc::channel();
        *super::super::status_item::SENDER
            .get_or_init(|| Mutex::new(None))
            .lock()
            .unwrap() = Some(super::super::EventSender::without_wake(sender));
        struct Cleanup;
        impl Drop for Cleanup {
            fn drop(&mut self) {
                shutdown();
                *super::super::status_item::SENDER
                    .get()
                    .unwrap()
                    .lock()
                    .unwrap() = None;
            }
        }
        let _cleanup = Cleanup;
        let prompt = TextPrompt {
            bounds: crate::api::Rect::new(80.0, 600.0, 720.0, 100.0),
            id: 1,
            title: "KeySteer disposable note probe".into(),
            message: "Native note test".into(),
            placeholder: "Optional note".into(),
            max_chars: 80,
        };
        show(prompt.clone()).unwrap();
        let hwnd = ACTIVE.with(|a| a.borrow().as_ref().unwrap().hwnd);
        let expected = "中文备注 · Coding 🦀";
        // SAFETY: this ignored probe owns the same-thread dialog and edit child;
        // the temporary UTF-16 string lives for the synchronous text copy.
        unsafe {
            assert_eq!(
                GetWindowLongW(hwnd, GWL_STYLE) as u32 & WS_CAPTION.0,
                0,
                "inline input has no dialog title bar"
            );
            let mut bounds = windows::Win32::Foundation::RECT::default();
            GetWindowRect(hwnd, &mut bounds).unwrap();
            assert_eq!(
                (bounds.left, bounds.top, bounds.right, bounds.bottom),
                (80, 600, 800, 700)
            );
            SetDlgItemTextW(hwnd, 101, &windows::core::HSTRING::from(expected)).unwrap();
        }
        dialog_proc(hwnd, WM_COMMAND, WPARAM(1), LPARAM(0));
        assert!(ACTIVE.with(|a| a.borrow().is_none()));
        assert!(
            matches!(receiver.recv_timeout(std::time::Duration::from_secs(1)).unwrap(), BackendEvent::TextPromptResult { id: 1, value: Ok(Some(text)) } if text == expected)
        );
        show(TextPrompt { id: 2, ..prompt }).unwrap();
        shutdown();
        assert!(matches!(
            receiver
                .recv_timeout(std::time::Duration::from_secs(1))
                .unwrap(),
            BackendEvent::TextPromptResult {
                id: 2,
                value: Ok(None)
            }
        ));
        assert!(ACTIVE.with(|a| a.borrow().is_none()));
    }
}
