//! Optional, auto-discovered WeChat OCR bridge isolated in a helper process.

use std::ffi::CString;
use std::fs::File;
use std::io::{Read, Write};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use semver::Version;
use serde::Deserialize;
use windows::Graphics::Imaging::BitmapEncoder;
use windows::Storage::Streams::{Buffer, InMemoryRandomAccessStream, InputStreamOptions};

use crate::api::geometry::{Rect, UiTarget};

use super::vision::{CapturedImage, image_to_desktop, valid_target_rect};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const MAX_MESSAGE: usize = 8 * 1024 * 1024;
const READY_TIMEOUT: Duration = Duration::from_secs(3);
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug)]
pub(crate) struct HelperPaths {
    pub(crate) bridge: PathBuf,
    pub(crate) component: PathBuf,
    pub(crate) runtime: PathBuf,
}

#[derive(Clone)]
pub(super) struct WechatOcr {
    inner: Arc<Mutex<WechatState>>,
}

struct WechatState {
    paths: HelperPaths,
    helper: Option<Helper>,
    restart_budget: u8,
}

impl WechatOcr {
    pub(super) fn discover_and_start() -> Option<Self> {
        let paths = match discover() {
            Ok(Some(paths)) => paths,
            Ok(None) => {
                crate::log_info!("windows-vision", "WeChat OCR was not discovered");
                return None;
            }
            Err(error) => {
                crate::log_info!("windows-vision", "WeChat OCR unavailable: {error}");
                return None;
            }
        };
        let helper = match Helper::start(&paths) {
            Ok(helper) => {
                crate::log_info!(
                    "windows-vision",
                    "WeChat OCR helper prewarmed from {}",
                    paths.component.display()
                );
                Some(helper)
            }
            Err(error) => {
                crate::log_warning!(
                    "windows-vision",
                    "cannot prewarm WeChat OCR helper: {error}"
                );
                None
            }
        };
        Some(Self {
            inner: Arc::new(Mutex::new(WechatState {
                paths,
                helper,
                restart_budget: 1,
            })),
        })
    }

    pub(super) fn recognize(
        &self,
        image: &CapturedImage,
        timeout: Duration,
        minimum_confidence: f64,
    ) -> Result<Vec<UiTarget>, String> {
        let temporary = TemporaryPng::create(image)?;
        let response = {
            let mut state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
            if state.helper.is_none() && state.restart_budget != 0 {
                state.restart_budget -= 1;
                if let Ok(Some(paths)) = discover() {
                    state.paths = paths;
                }
                state.helper = Helper::start(&state.paths).ok();
            }
            let helper = state
                .helper
                .as_mut()
                .ok_or_else(|| "WeChat OCR helper is unavailable".to_string())?;
            match helper.request(&temporary.path, timeout) {
                Ok(response) => response,
                Err(error) => {
                    helper.terminate();
                    state.helper = None;
                    return Err(error);
                }
            }
        };
        parse_response(&response, image, minimum_confidence)
    }
}

struct Helper {
    child: Child,
    input: ChildStdin,
    responses: mpsc::Receiver<Result<Vec<u8>, String>>,
}

impl Helper {
    fn start(paths: &HelperPaths) -> Result<Self, String> {
        let executable = std::env::current_exe()
            .map_err(|error| format!("cannot locate KeySteer executable: {error}"))?;
        let mut child = Command::new(executable)
            .arg("--internal-wechat-ocr-helper")
            .arg(&paths.bridge)
            .arg(&paths.component)
            .arg(&paths.runtime)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|error| format!("cannot start hidden WeChat OCR helper: {error}"))?;
        let input = child
            .stdin
            .take()
            .ok_or_else(|| "WeChat OCR helper stdin was not piped".to_string())?;
        let mut output = child
            .stdout
            .take()
            .ok_or_else(|| "WeChat OCR helper stdout was not piped".to_string())?;
        let (tx, responses) = mpsc::channel();
        std::thread::spawn(move || {
            loop {
                let message = read_frame(&mut output);
                let stop = message.is_err();
                if tx.send(message).is_err() || stop {
                    break;
                }
            }
        });
        let mut helper = Self {
            child,
            input,
            responses,
        };
        match helper.responses.recv_timeout(READY_TIMEOUT) {
            Ok(Ok(message)) if message == b"ready" => Ok(helper),
            Ok(Ok(_)) => {
                helper.terminate();
                Err("WeChat OCR helper returned an invalid readiness frame".into())
            }
            Ok(Err(error)) => {
                helper.terminate();
                Err(error)
            }
            Err(error) => {
                helper.terminate();
                Err(format!("WeChat OCR helper readiness timed out: {error}"))
            }
        }
    }

    fn request(&mut self, path: &Path, timeout: Duration) -> Result<Vec<u8>, String> {
        let value = path.to_string_lossy();
        write_frame(&mut self.input, value.as_bytes())?;
        match self
            .responses
            .recv_timeout(timeout.max(Duration::from_millis(1)))
        {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                Err(format!("WeChat OCR helper exceeded {timeout:?}"))
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                Err("WeChat OCR helper crashed or closed its pipe".into())
            }
        }
    }

    fn terminate(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        let _ = write_frame(&mut self.input, &[]);
        match self.child.try_wait() {
            Ok(Some(_)) => {}
            _ => self.terminate(),
        }
    }
}

fn write_frame(writer: &mut impl Write, value: &[u8]) -> Result<(), String> {
    if value.len() > MAX_MESSAGE {
        return Err(format!("IPC frame exceeds {MAX_MESSAGE} bytes"));
    }
    writer
        .write_all(&(value.len() as u32).to_le_bytes())
        .and_then(|_| writer.write_all(value))
        .and_then(|_| writer.flush())
        .map_err(|error| format!("cannot write WeChat OCR IPC frame: {error}"))
}

fn read_frame(reader: &mut impl Read) -> Result<Vec<u8>, String> {
    let mut length = [0u8; 4];
    reader
        .read_exact(&mut length)
        .map_err(|error| format!("cannot read WeChat OCR IPC length: {error}"))?;
    let length = u32::from_le_bytes(length) as usize;
    if length > MAX_MESSAGE {
        return Err(format!(
            "WeChat OCR IPC response is too large: {length} bytes"
        ));
    }
    let mut value = vec![0; length];
    reader
        .read_exact(&mut value)
        .map_err(|error| format!("cannot read WeChat OCR IPC body: {error}"))?;
    Ok(value)
}

struct TemporaryPng {
    path: PathBuf,
}

impl TemporaryPng {
    fn create(image: &CapturedImage) -> Result<Self, String> {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "keysteer-ocr-{}-{sequence}.png",
            std::process::id()
        ));
        encode_png(image, &path)?;
        Ok(Self { path })
    }
}

impl Drop for TemporaryPng {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_file(&self.path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            crate::log_warning!(
                "windows-vision",
                "cannot remove temporary OCR image {}: {error}",
                self.path.display()
            );
        }
    }
}

fn encode_png(image: &CapturedImage, path: &Path) -> Result<(), String> {
    let bitmap = super::native::software_bitmap_bgra(&image.pixels, image.width, image.height)?;
    let stream = InMemoryRandomAccessStream::new()
        .map_err(|error| format!("cannot create WIC output stream: {error}"))?;
    let encoder = BitmapEncoder::CreateAsync(
        BitmapEncoder::PngEncoderId()
            .map_err(|error| format!("cannot query PNG encoder: {error}"))?,
        &stream,
    )
    .and_then(|operation| operation.join())
    .map_err(|error| format!("cannot create WIC PNG encoder: {error}"))?;
    encoder
        .SetSoftwareBitmap(&bitmap)
        .map_err(|error| format!("cannot supply WIC bitmap: {error}"))?;
    encoder
        .FlushAsync()
        .and_then(|operation| operation.join())
        .map_err(|error| format!("cannot encode temporary PNG: {error}"))?;
    let size = stream
        .Size()
        .map_err(|error| format!("cannot read temporary PNG size: {error}"))?;
    let size = u32::try_from(size).map_err(|_| "temporary PNG exceeds 4 GiB".to_string())?;
    let buffer = Buffer::Create(size).map_err(|error| format!("PNG buffer: {error}"))?;
    buffer
        .SetLength(size)
        .map_err(|error| format!("PNG buffer length: {error}"))?;
    stream
        .Seek(0)
        .map_err(|error| format!("cannot rewind temporary PNG: {error}"))?;
    let result = stream
        .ReadAsync(&buffer, size, InputStreamOptions::None)
        .and_then(|operation| operation.join())
        .map_err(|error| format!("cannot read temporary PNG: {error}"))?;
    let bytes = super::native::buffer_bytes(&result)?;
    File::create(path)
        .and_then(|mut file| file.write_all(&bytes))
        .map_err(|error| format!("cannot write temporary PNG {}: {error}", path.display()))
}

#[derive(Deserialize)]
struct WechatResponse {
    #[serde(default)]
    errcode: i64,
    #[serde(default)]
    ocr_response: Vec<WechatItem>,
}

#[derive(Deserialize)]
struct WechatItem {
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
    #[serde(default)]
    text: String,
    #[serde(default = "default_confidence")]
    rate: f64,
}

fn default_confidence() -> f64 {
    1.0
}

fn parse_response(
    value: &[u8],
    image: &CapturedImage,
    minimum_confidence: f64,
) -> Result<Vec<UiTarget>, String> {
    let response: WechatResponse = serde_json::from_slice(value)
        .map_err(|error| format!("invalid WeChat OCR JSON: {error}"))?;
    if response.errcode != 0 {
        return Err(format!("WeChat OCR returned error {}", response.errcode));
    }
    let targets = response
        .ocr_response
        .into_iter()
        .filter_map(|item| {
            let text = item.text.trim();
            let rect = image_to_desktop(
                image,
                Rect::new(
                    item.left,
                    item.top,
                    item.right - item.left,
                    item.bottom - item.top,
                ),
            );
            (!text.is_empty()
                && item.rate >= minimum_confidence
                && valid_target_rect(rect, image.desktop_bounds))
            .then(|| UiTarget {
                rect,
                name: text.to_string(),
                role: "static_text".into(),
                native_role: Some("vision:wechat-ocr".into()),
            })
        })
        .collect();
    Ok(merge_text_lines(targets))
}

fn merge_text_lines(mut targets: Vec<UiTarget>) -> Vec<UiTarget> {
    targets.sort_by(|a, b| {
        a.rect
            .center()
            .y
            .total_cmp(&b.rect.center().y)
            .then_with(|| a.rect.x.total_cmp(&b.rect.x))
    });
    let mut merged: Vec<UiTarget> = Vec::with_capacity(targets.len());
    for target in targets {
        let Some(previous) = merged.last_mut() else {
            merged.push(target);
            continue;
        };
        let baseline_delta = (previous.rect.center().y - target.rect.center().y).abs();
        let gap = target.rect.x - previous.rect.right();
        let same_line = baseline_delta
            <= (previous.rect.height.min(target.rect.height) * 0.35).max(2.0)
            && gap >= -2.0
            && gap <= (previous.rect.height.min(target.rect.height) * 0.5).max(4.0);
        if same_line {
            if previous
                .name
                .chars()
                .next_back()
                .is_some_and(|character| character.is_ascii_alphanumeric())
                && target
                    .name
                    .chars()
                    .next()
                    .is_some_and(|character| character.is_ascii_alphanumeric())
            {
                previous.name.push(' ');
            }
            previous.name.push_str(&target.name);
            previous.rect = previous.rect.union(&target.rect);
        } else {
            merged.push(target);
        }
    }
    merged
}

fn discover() -> Result<Option<HelperPaths>, String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("cannot locate KeySteer executable: {error}"))?;
    let Some(application_dir) = executable.parent() else {
        return Ok(None);
    };
    let bridge = application_dir.join("wcocr.dll");
    if !bridge.is_file() {
        return Ok(None);
    }
    if pe_machine(&bridge)? != current_machine() {
        return Err(format!(
            "{} has a different PE architecture",
            bridge.display()
        ));
    }

    let wx4_component = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .map(|root| root.join("Tencent/xwechat/XPlugin/plugins/WeChatOcr"))
        .and_then(|root| highest_versioned_file(&root, Path::new("extracted/wxocr.dll")));
    let wx4_runtime = std::env::var_os("ProgramFiles")
        .map(PathBuf::from)
        .map(|root| root.join("Tencent/Weixin"))
        .and_then(|root| highest_runtime(&root, &["Weixin.exe"]));
    if let (Some(component), Some(runtime)) = (wx4_component, wx4_runtime)
        && pe_machine(&component).is_ok_and(|machine| machine == current_machine())
        && runtime_matches(&runtime, &["Weixin.exe"])
    {
        return Ok(Some(HelperPaths {
            bridge,
            component,
            runtime,
        }));
    }

    let wx3_root = std::env::var_os("ProgramFiles(x86)")
        .map(PathBuf::from)
        .map(|root| root.join("Tencent/WeChat"));
    if let Some(root) = wx3_root {
        let adjacent_component = application_dir.join("WeChatOCR.exe");
        let component = adjacent_component
            .is_file()
            .then_some(adjacent_component)
            .or_else(|| highest_versioned_file(&root, Path::new("WeChatOCR.exe")));
        let runtime = highest_runtime(&root, &["WeChat.exe"]);
        if let (Some(component), Some(runtime)) = (component, runtime)
            && pe_machine(&component).is_ok_and(|machine| machine == current_machine())
            && runtime_matches(&runtime, &["WeChat.exe"])
        {
            return Ok(Some(HelperPaths {
                bridge,
                component,
                runtime,
            }));
        }
    }
    Ok(None)
}

fn runtime_matches(runtime: &Path, expected: &[&str]) -> bool {
    expected.iter().any(|name| {
        pe_machine(&runtime.join(name)).is_ok_and(|machine| machine == current_machine())
    })
}

pub(super) fn diagnostic_line() -> String {
    match discover() {
        Ok(Some(paths)) => format!(
            "WeChat OCR: available (bridge {}, component {})",
            paths.bridge.display(),
            paths.component.display()
        ),
        Ok(None) => "WeChat OCR: unavailable (optional components not found)".into(),
        Err(error) => format!("WeChat OCR: unavailable ({error})"),
    }
}

fn highest_versioned_file(root: &Path, relative: &Path) -> Option<PathBuf> {
    versioned_directories(root)
        .into_iter()
        .filter_map(|(_, directory)| {
            let path = directory.join(relative);
            path.is_file().then_some(path)
        })
        .next()
}

fn highest_runtime(root: &Path, expected: &[&str]) -> Option<PathBuf> {
    versioned_directories(root)
        .into_iter()
        .find_map(|(_, directory)| {
            expected
                .iter()
                .any(|name| directory.join(name).is_file())
                .then_some(directory)
        })
}

fn versioned_directories(root: &Path) -> Vec<(Version, PathBuf)> {
    let mut values = std::fs::read_dir(root)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| {
            let raw = entry.file_name().to_string_lossy().to_string();
            parse_version(&raw).map(|version| (version, entry.path()))
        })
        .collect::<Vec<_>>();
    values.sort_by(|a, b| b.0.cmp(&a.0));
    values
}

fn parse_version(value: &str) -> Option<Version> {
    let mut parts = value
        .trim_start_matches(|character: char| !character.is_ascii_digit())
        .split(|character: char| !character.is_ascii_digit())
        .filter(|part| !part.is_empty());
    Some(Version::new(
        parts.next()?.parse().ok()?,
        parts.next().unwrap_or("0").parse().ok()?,
        parts.next().unwrap_or("0").parse().ok()?,
    ))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PeMachine {
    X86,
    X64,
    Arm64,
}

fn current_machine() -> PeMachine {
    #[cfg(target_arch = "x86")]
    return PeMachine::X86;
    #[cfg(target_arch = "x86_64")]
    return PeMachine::X64;
    #[cfg(target_arch = "aarch64")]
    return PeMachine::Arm64;
}

fn pe_machine(path: &Path) -> Result<PeMachine, String> {
    let mut file =
        File::open(path).map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
    let mut dos = [0u8; 64];
    file.read_exact(&mut dos)
        .map_err(|error| format!("cannot read PE header {}: {error}", path.display()))?;
    if &dos[..2] != b"MZ" {
        return Err(format!("{} is not a PE image", path.display()));
    }
    let offset = u32::from_le_bytes(dos[0x3c..0x40].try_into().unwrap_or([0; 4])) as u64;
    use std::io::Seek;
    file.seek(std::io::SeekFrom::Start(offset))
        .map_err(|error| format!("cannot seek PE header {}: {error}", path.display()))?;
    let mut header = [0u8; 6];
    file.read_exact(&mut header)
        .map_err(|error| format!("cannot read PE signature {}: {error}", path.display()))?;
    if &header[..4] != b"PE\0\0" {
        return Err(format!("{} has an invalid PE signature", path.display()));
    }
    match u16::from_le_bytes([header[4], header[5]]) {
        0x014c => Ok(PeMachine::X86),
        0x8664 => Ok(PeMachine::X64),
        0xaa64 => Ok(PeMachine::Arm64),
        machine => Err(format!(
            "{} has unsupported PE machine 0x{machine:04x}",
            path.display()
        )),
    }
}

pub(crate) fn run_helper(paths: HelperPaths) -> Result<(), String> {
    let bridge = super::native::WechatBridge::load(&paths.bridge)?;
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    write_frame(&mut output, b"ready")?;
    loop {
        let request = read_frame(&mut input)?;
        if request.is_empty() {
            break;
        }
        let path = CString::new(request)
            .map_err(|_| "temporary OCR path contains an interior NUL".to_string())?;
        match bridge.recognize(&paths.component, &paths.runtime, &path) {
            Ok(value) => write_frame(&mut output, &value)?,
            Err(_) => write_frame(&mut output, br#"{"errcode":-1,"ocr_response":[]}"#)?,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_parser_orders_wechat_style_names() {
        assert!(parse_version("4.1.2.8") > parse_version("4.0.12"));
        assert_eq!(parse_version("WeChat-3.9.12"), Some(Version::new(3, 9, 12)));
    }

    #[test]
    fn rejects_oversized_ipc_frames_before_allocation() {
        let encoded = ((MAX_MESSAGE as u32) + 1).to_le_bytes();
        let mut bytes = encoded.as_slice();
        assert!(read_frame(&mut bytes).is_err());
    }

    #[test]
    fn parses_and_maps_wechat_results() {
        let image = CapturedImage {
            pixels: Vec::new(),
            width: 100,
            height: 100,
            desktop_bounds: Rect::new(-100.0, 20.0, 200.0, 200.0),
            scale: 0.5,
        };
        let targets = parse_response(
            r#"{"errcode":0,"ocr_response":[{"left":10,"top":5,"right":30,"bottom":15,"text":"确定"}]}"#
                .as_bytes(),
            &image,
            0.0,
        )
        .unwrap();
        assert_eq!(targets[0].rect, Rect::new(-80.0, 30.0, 40.0, 20.0));
    }

    #[test]
    fn merges_adjacent_words_but_not_cross_line_text() {
        let target = |x, y, name: &str| UiTarget {
            rect: Rect::new(x, y, 20.0, 10.0),
            name: name.into(),
            role: "static_text".into(),
            native_role: Some("vision:wechat-ocr".into()),
        };
        let merged = merge_text_lines(vec![
            target(0.0, 0.0, "Open"),
            target(23.0, 0.0, "File"),
            target(0.0, 14.0, "下一行"),
        ]);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].name, "Open File");
        assert_eq!(merged[1].name, "下一行");
    }
}
