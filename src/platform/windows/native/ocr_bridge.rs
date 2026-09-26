//! DLL lifetime, typed exports and bounded OCR callback copying.
use std::path::Path;

type OcrCallback = unsafe extern "C" fn(windows::core::PCSTR);
type WechatOcrFn = unsafe extern "C" fn(
    windows::core::PCWSTR,
    windows::core::PCWSTR,
    windows::core::PCSTR,
    OcrCallback,
) -> bool;
type StopOcrFn = unsafe extern "C" fn();
static WECHAT_CALLBACK_VALUE: std::sync::OnceLock<std::sync::Mutex<Vec<u8>>> =
    std::sync::OnceLock::new();
const MAX_WECHAT_RESPONSE: usize = 8 * 1024 * 1024;

unsafe extern "C" fn capture_wechat_callback(value: windows::core::PCSTR) {
    if value.is_null() {
        return;
    }
    // SAFETY: wcocr documents a readable NUL-terminated callback string valid
    // for this callback. Scan no farther than the IPC ceiling and copy before
    // returning to the bridge.
    let bytes = unsafe {
        let mut length = 0usize;
        while length <= MAX_WECHAT_RESPONSE && *value.0.add(length) != 0 {
            length += 1;
        }
        if length > MAX_WECHAT_RESPONSE {
            return;
        }
        std::slice::from_raw_parts(value.0, length)
    };
    let mut output = WECHAT_CALLBACK_VALUE
        .get_or_init(|| std::sync::Mutex::new(Vec::new()))
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    output.clear();
    if bytes.len() <= MAX_WECHAT_RESPONSE {
        output.extend_from_slice(bytes);
    }
}

#[must_use = "the module must stay loaded while exported function pointers are used"]
struct OwnedModule(windows::Win32::Foundation::HMODULE);

impl Drop for OwnedModule {
    fn drop(&mut self) {
        use windows::Win32::Foundation::FreeLibrary;

        // SAFETY: this guard uniquely owns the successful LoadLibraryExW
        // result and is destroyed only after all exported pointers are dead.
        if let Err(error) = unsafe { FreeLibrary(self.0) } {
            crate::support::logging::report_error(
                "windows-native",
                format!("cannot unload WeChat OCR bridge: {error}"),
            );
        }
    }
}

pub(crate) struct WechatBridge {
    _module: OwnedModule,
    recognize: WechatOcrFn,
    stop: Option<StopOcrFn>,
}

fn wechat_recognize_export(
    address: windows::Win32::Foundation::FARPROC,
) -> Result<WechatOcrFn, String> {
    const _: () = assert!(
        std::mem::size_of::<windows::Win32::Foundation::FARPROC>()
            == std::mem::size_of::<WechatOcrFn>()
    );
    #[repr(C)]
    union Export {
        raw: windows::Win32::Foundation::FARPROC,
        typed: WechatOcrFn,
    }
    if address.is_none() {
        return Err("wcocr.dll returned a null wechat_ocr export".into());
    }
    // SAFETY: GetProcAddress returned this exact symbol from the architecture-
    // checked bridge. The compile-time size assertion and dedicated union keep
    // the only ABI reinterpretation local to this audited loader.
    Ok(unsafe { Export { raw: address }.typed })
}

fn wechat_stop_export(address: windows::Win32::Foundation::FARPROC) -> Option<StopOcrFn> {
    const _: () = assert!(
        std::mem::size_of::<windows::Win32::Foundation::FARPROC>()
            == std::mem::size_of::<StopOcrFn>()
    );
    #[repr(C)]
    union Export {
        raw: windows::Win32::Foundation::FARPROC,
        typed: StopOcrFn,
    }
    address.map(|raw| {
        // SAFETY: this optional address is the exact `stop_ocr` export from
        // the same architecture-checked module and has the asserted size.
        unsafe { Export { raw: Some(raw) }.typed }
    })
}

impl WechatBridge {
    pub(crate) fn load(path: &Path) -> Result<Self, String> {
        use std::os::windows::ffi::OsStrExt;
        use windows::Win32::System::LibraryLoader::{
            GetProcAddress, LOAD_LIBRARY_SEARCH_DEFAULT_DIRS, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR,
            LoadLibraryExW,
        };
        use windows::core::{PCSTR, PCWSTR};

        let wide = path
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        // SAFETY: the absolute path and symbol names are NUL terminated.
        // Restricted search uses only the bridge directory and Windows safe
        // defaults; the typed pointers use the bridge's documented C ABI and
        // cannot outlive the returned module owner.
        let module = unsafe {
            OwnedModule(
                LoadLibraryExW(
                    PCWSTR(wide.as_ptr()),
                    None,
                    LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
                )
                .map_err(|error| format!("cannot load {}: {error}", path.display()))?,
            )
        };
        // SAFETY: symbol names are static NUL-terminated C strings and the
        // owned module remains loaded through all returned function pointers.
        let (recognize, stop) = unsafe {
            let address = GetProcAddress(module.0, PCSTR(c"wechat_ocr".as_ptr().cast()))
                .ok_or_else(|| "wcocr.dll lacks wechat_ocr".to_string())?;
            let recognize = wechat_recognize_export(Some(address))?;
            let stop =
                wechat_stop_export(GetProcAddress(module.0, PCSTR(c"stop_ocr".as_ptr().cast())));
            (recognize, stop)
        };
        Ok(Self {
            _module: module,
            recognize,
            stop,
        })
    }

    pub(crate) fn recognize(
        &self,
        component: &Path,
        runtime: &Path,
        image: &std::ffi::CStr,
    ) -> Result<Vec<u8>, String> {
        use std::os::windows::ffi::OsStrExt;
        use windows::core::{PCSTR, PCWSTR};

        let component = component
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let runtime = runtime
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        WECHAT_CALLBACK_VALUE
            .get_or_init(|| std::sync::Mutex::new(Vec::new()))
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clear();
        // SAFETY: all path buffers remain NUL terminated for the synchronous
        // call and the callback copies its result before returning.
        let success = unsafe {
            (self.recognize)(
                PCWSTR(component.as_ptr()),
                PCWSTR(runtime.as_ptr()),
                PCSTR(image.as_ptr().cast()),
                capture_wechat_callback,
            )
        };
        let mut value = WECHAT_CALLBACK_VALUE
            .get_or_init(|| std::sync::Mutex::new(Vec::new()))
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if success && !value.is_empty() {
            Ok(std::mem::take(&mut *value))
        } else {
            Err("WeChat OCR bridge returned no response".into())
        }
    }
}

impl Drop for WechatBridge {
    fn drop(&mut self) {
        // SAFETY: no recognition call remains active; all function pointers
        // still belong to the live `OwnedModule`, which is dropped afterwards.
        unsafe {
            if let Some(stop) = self.stop {
                stop();
            }
        }
    }
}
