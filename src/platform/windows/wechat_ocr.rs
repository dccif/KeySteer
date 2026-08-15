//! Optional, auto-discovered WeChat OCR bridge isolated in a helper process.

use std::ffi::CString;
use std::fs::{File, Metadata};
use std::io::{Read, Write};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use semver::Version;
use serde::Deserialize;
use windows::Graphics::Imaging::{BitmapEncoder, SoftwareBitmap};
use windows::Storage::Streams::IRandomAccessStream;

use crate::api::geometry::{Rect, UiTarget};
use crate::app::worker::WorkerJoin;

use super::vision::{CapturedImage, image_to_desktop, valid_target_rect};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;
const MAX_MESSAGE: usize = 8 * 1024 * 1024;
const READY_TIMEOUT: Duration = Duration::from_secs(3);
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug)]
pub(crate) struct HelperPaths {
    pub(crate) bridge: PathBuf,
    pub(crate) component: PathBuf,
    pub(crate) runtime: PathBuf,
}

#[derive(Clone, Debug)]
pub(super) struct WechatDescriptor {
    paths: HelperPaths,
    files: Vec<FileIdentity>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FileIdentity {
    path: PathBuf,
    length: u64,
    modified: Option<std::time::SystemTime>,
}

impl FileIdentity {
    fn read(path: &Path) -> Result<Self, String> {
        let path = path
            .canonicalize()
            .map_err(|error| format!("cannot canonicalize {}: {error}", path.display()))?;
        let metadata = path
            .metadata()
            .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
        Self::from_metadata(path, &metadata)
    }

    fn from_metadata(path: PathBuf, metadata: &Metadata) -> Result<Self, String> {
        let modified = metadata
            .modified()
            .map_err(|error| format!("cannot read timestamp for {}: {error}", path.display()))?;
        Ok(Self {
            path,
            length: metadata.len(),
            modified: Some(modified),
        })
    }

    fn validate(&self) -> Result<(), String> {
        let metadata = self
            .path
            .metadata()
            .map_err(|error| format!("cannot revalidate {}: {error}", self.path.display()))?;
        let current = Self::from_metadata(self.path.clone(), &metadata)?;
        if current == *self {
            Ok(())
        } else {
            Err(format!(
                "cached WeChat OCR component changed: {} (restart KeySteer to rediscover)",
                self.path.display()
            ))
        }
    }
}

impl WechatDescriptor {
    fn from_paths(mut paths: HelperPaths) -> Result<Self, String> {
        paths.bridge = paths
            .bridge
            .canonicalize()
            .map_err(|error| format!("cannot canonicalize OCR bridge: {error}"))?;
        paths.component = paths
            .component
            .canonicalize()
            .map_err(|error| format!("cannot canonicalize OCR component: {error}"))?;
        paths.runtime = paths
            .runtime
            .canonicalize()
            .map_err(|error| format!("cannot canonicalize WeChat runtime: {error}"))?;
        let runtime_executable = ["Weixin.exe", "WeChat.exe"]
            .into_iter()
            .map(|name| paths.runtime.join(name))
            .find(|path| path.is_file())
            .ok_or_else(|| "cached WeChat runtime executable disappeared".to_string())?;
        let files = [
            paths.bridge.as_path(),
            paths.component.as_path(),
            runtime_executable.as_path(),
        ]
        .into_iter()
        .map(FileIdentity::read)
        .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { paths, files })
    }

    pub(super) fn validate(&self) -> Result<(), String> {
        self.files.iter().try_for_each(FileIdentity::validate)
    }

    pub(super) fn description(&self) -> String {
        format!(
            "bridge {}, component {}",
            self.paths.bridge.display(),
            self.paths.component.display()
        )
    }
}

pub(super) struct WechatOcr {
    helper: Option<Helper>,
}

impl WechatOcr {
    pub(super) fn start(descriptor: &WechatDescriptor) -> Result<Self, String> {
        descriptor.validate()?;
        Ok(Self {
            helper: Some(Helper::start(&descriptor.paths)?),
        })
    }

    pub(super) fn recognize(
        &mut self,
        image: &CapturedImage,
        bitmap: &SoftwareBitmap,
        timeout: Duration,
        minimum_confidence: f64,
        cancelled: impl Fn() -> bool,
    ) -> Result<Vec<UiTarget>, String> {
        if cancelled() {
            return Err("WeChat OCR cancelled".into());
        }
        let temporary = TemporaryPng::create(bitmap)?;
        if cancelled() {
            return Err("WeChat OCR cancelled".into());
        }
        let response = self
            .helper
            .as_mut()
            .ok_or_else(|| "WeChat OCR helper is unavailable".to_string())?
            .request(&temporary.path, timeout, &cancelled);
        let helper_cleanup = self.helper.as_mut().map_or(Ok(()), Helper::shutdown);
        let cleanup = temporary.cleanup();
        let mut result =
            response.and_then(|response| parse_response(&response, image, minimum_confidence));
        if let Err(helper_cleanup) = helper_cleanup {
            crate::app::logging::report_error("windows-vision", &helper_cleanup);
            result = result.and(Err(helper_cleanup));
        }
        if let Err(cleanup) = cleanup {
            crate::app::logging::report_error("windows-vision", &cleanup);
            return result.and(Err(cleanup));
        }
        result
    }

    pub(super) fn shutdown(&mut self) -> Result<(), String> {
        self.helper.as_mut().map_or(Ok(()), Helper::shutdown)?;
        self.helper.take();
        Ok(())
    }
}

impl Drop for WechatOcr {
    fn drop(&mut self) {
        if let Err(error) = self.shutdown() {
            crate::app::logging::report_error("windows-vision", error);
        }
    }
}

struct Helper {
    child: Child,
    input: Option<ChildStdin>,
    responses: mpsc::Receiver<Result<Vec<u8>, String>>,
    reader: Option<WorkerJoin>,
    stopped: bool,
}

struct StartingChild(Option<Child>);

impl StartingChild {
    fn child(&mut self) -> Result<&mut Child, String> {
        self.0
            .as_mut()
            .ok_or_else(|| "starting WeChat OCR helper child is missing".to_string())
    }

    fn finish(mut self) -> Result<Child, String> {
        self.0
            .take()
            .ok_or_else(|| "starting WeChat OCR helper child is missing".to_string())
    }
}

impl Drop for StartingChild {
    fn drop(&mut self) {
        let Some(child) = self.0.as_mut() else {
            return;
        };
        match child.try_wait() {
            Ok(Some(_)) => {}
            Ok(None) => {
                if let Err(error) = child.kill() {
                    crate::app::logging::report_error(
                        "windows-vision",
                        format!("cannot terminate unready WeChat OCR helper: {error}"),
                    );
                    return;
                }
                if let Err(error) = child.wait() {
                    crate::app::logging::report_error(
                        "windows-vision",
                        format!("cannot wait for unready WeChat OCR helper: {error}"),
                    );
                }
            }
            Err(error) => crate::app::logging::report_error(
                "windows-vision",
                format!("cannot query unready WeChat OCR helper: {error}"),
            ),
        }
    }
}

impl Helper {
    fn start(paths: &HelperPaths) -> Result<Self, String> {
        let executable = std::env::current_exe()
            .map_err(|error| format!("cannot locate KeySteer executable: {error}"))?;
        let child = Command::new(executable)
            .arg("--internal-wechat-ocr-helper")
            .arg(&paths.bridge)
            .arg(&paths.component)
            .arg(&paths.runtime)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS)
            .spawn()
            .map_err(|error| format!("cannot start hidden WeChat OCR helper: {error}"))?;
        let mut starting = StartingChild(Some(child));
        let input = starting
            .child()?
            .stdin
            .take()
            .ok_or_else(|| "WeChat OCR helper stdin was not piped".to_string())?;
        let mut output = starting
            .child()?
            .stdout
            .take()
            .ok_or_else(|| "WeChat OCR helper stdout was not piped".to_string())?;
        let (tx, responses) = mpsc::sync_channel(1);
        let reader = WorkerJoin::spawn(
            "WeChat OCR helper reader",
            std::thread::Builder::new().name("keysteer-wechat-reader".into()),
            move || loop {
                let message = read_frame(&mut output);
                let stop = message.is_err();
                if tx.send(message).is_err() || stop {
                    break;
                }
            },
        )?;
        let mut helper = Self {
            child: starting.finish()?,
            input: Some(input),
            responses,
            reader: Some(reader),
            stopped: false,
        };
        match helper.responses.recv_timeout(READY_TIMEOUT) {
            Ok(Ok(message)) if message == b"ready" => Ok(helper),
            Ok(Ok(_)) => {
                if let Err(error) = helper.shutdown() {
                    crate::app::logging::report_error("windows-vision", error);
                }
                Err("WeChat OCR helper returned an invalid readiness frame".into())
            }
            Ok(Err(error)) => {
                if let Err(cleanup) = helper.shutdown() {
                    crate::app::logging::report_error("windows-vision", cleanup);
                }
                Err(error)
            }
            Err(error) => {
                if let Err(cleanup) = helper.shutdown() {
                    crate::app::logging::report_error("windows-vision", cleanup);
                }
                Err(format!("WeChat OCR helper readiness timed out: {error}"))
            }
        }
    }

    fn request(
        &mut self,
        path: &Path,
        timeout: Duration,
        cancelled: &impl Fn() -> bool,
    ) -> Result<Vec<u8>, String> {
        let value = path.to_string_lossy();
        write_frame(
            self.input
                .as_mut()
                .ok_or_else(|| "WeChat OCR helper input is closed".to_string())?,
            value.as_bytes(),
        )?;
        let deadline = Instant::now() + timeout.max(Duration::from_millis(1));
        loop {
            if cancelled() {
                return Err("WeChat OCR cancelled".into());
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(format!("WeChat OCR helper exceeded {timeout:?}"));
            }
            match self
                .responses
                .recv_timeout(remaining.min(Duration::from_millis(10)))
            {
                Ok(result) => return result,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("WeChat OCR helper crashed or closed its pipe".into());
                }
            }
        }
    }

    fn shutdown(&mut self) -> Result<(), String> {
        if self.stopped {
            return Ok(());
        }
        let mut first_error = None;
        if let Some(mut input) = self.input.take()
            && let Err(error) = write_frame(&mut input, &[])
        {
            first_error = Some(error);
        }
        let deadline = Instant::now() + Duration::from_millis(250);
        let mut exited = false;
        while Instant::now() < deadline {
            match self.child.try_wait() {
                Ok(Some(_)) => {
                    exited = true;
                    break;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(5)),
                Err(error) => {
                    first_error.get_or_insert_with(|| {
                        format!("cannot query WeChat OCR helper state: {error}")
                    });
                    break;
                }
            }
        }
        if !exited {
            let can_wait = match self.child.kill() {
                Ok(()) => true,
                Err(error) if error.kind() == std::io::ErrorKind::InvalidInput => true,
                Err(error) => {
                    first_error.get_or_insert_with(|| {
                        format!("cannot terminate WeChat OCR helper: {error}")
                    });
                    false
                }
            };
            if can_wait && let Err(error) = self.child.wait() {
                first_error
                    .get_or_insert_with(|| format!("cannot wait for WeChat OCR helper: {error}"));
            }
        }
        if let Some(reader) = self.reader.as_mut() {
            match reader.join_timeout(Duration::from_secs(2)) {
                Ok(()) => {
                    self.reader.take();
                }
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        self.stopped = self.reader.is_none();
        first_error.map_or(Ok(()), Err)
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        if let Err(error) = self.shutdown() {
            crate::app::logging::report_error("windows-vision", error);
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
    removed: bool,
}

impl TemporaryPng {
    fn create(bitmap: &SoftwareBitmap) -> Result<Self, String> {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "keysteer-ocr-{}-{sequence}.png",
            std::process::id()
        ));
        if let Err(error) = encode_png(bitmap, &path) {
            if let Err(cleanup) = std::fs::remove_file(&path)
                && cleanup.kind() != std::io::ErrorKind::NotFound
            {
                crate::app::logging::report_error(
                    "windows-vision",
                    format!(
                        "cannot remove incomplete temporary OCR image {}: {cleanup}",
                        path.display()
                    ),
                );
            }
            return Err(error);
        }
        Ok(Self {
            path,
            removed: false,
        })
    }

    fn cleanup(mut self) -> Result<(), String> {
        let result = self.remove();
        self.removed = true;
        result
    }

    fn remove(&mut self) -> Result<(), String> {
        if self.removed {
            return Ok(());
        }
        match std::fs::remove_file(&self.path) {
            Ok(()) => {
                self.removed = true;
                Ok(())
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.removed = true;
                Ok(())
            }
            Err(error) => Err(format!(
                "cannot remove temporary OCR image {}: {error}",
                self.path.display()
            )),
        }
    }
}

impl Drop for TemporaryPng {
    fn drop(&mut self) {
        if let Err(error) = self.remove() {
            crate::app::logging::report_error("windows-vision", error);
        }
    }
}

fn encode_png(bitmap: &SoftwareBitmap, path: &Path) -> Result<(), String> {
    let stream: IRandomAccessStream = super::native::create_file_random_access_stream(path)
        .map_err(|error| format!("cannot create temporary PNG stream: {error}"))?;
    let result = (|| {
        let encoder = BitmapEncoder::CreateAsync(
            BitmapEncoder::PngEncoderId()
                .map_err(|error| format!("cannot query PNG encoder: {error}"))?,
            &stream,
        )
        .and_then(|operation| operation.join())
        .map_err(|error| format!("cannot create WIC PNG encoder: {error}"))?;
        encoder
            .SetSoftwareBitmap(bitmap)
            .map_err(|error| format!("cannot supply WIC bitmap: {error}"))?;
        encoder
            .FlushAsync()
            .and_then(|operation| operation.join())
            .map_err(|error| format!("cannot encode temporary PNG: {error}"))?;
        stream
            .FlushAsync()
            .and_then(|operation| operation.join())
            .map_err(|error| format!("cannot flush temporary PNG: {error}"))?;
        Ok(())
    })();
    let close = stream
        .Close()
        .map_err(|error| format!("cannot close temporary PNG stream: {error}"));
    match (result, close) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(error), Err(close_error)) => {
            crate::app::logging::report_error("windows-vision", close_error);
            Err(error)
        }
    }
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

pub(super) fn discover_descriptor() -> Result<Option<WechatDescriptor>, String> {
    discover()?.map(WechatDescriptor::from_paths).transpose()
}

fn runtime_matches(runtime: &Path, expected: &[&str]) -> bool {
    expected.iter().any(|name| {
        pe_machine(&runtime.join(name)).is_ok_and(|machine| machine == current_machine())
    })
}

pub(super) fn diagnostic_line() -> String {
    match discover_descriptor() {
        Ok(Some(descriptor)) => format!("WeChat OCR: available ({})", descriptor.description()),
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

    fn test_path(name: &str) -> PathBuf {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "keysteer-{name}-test-{}-{sequence}",
            std::process::id()
        ))
    }

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

    #[test]
    fn cached_component_identity_rejects_a_changed_file() {
        let path = test_path("ocr-identity");
        std::fs::write(&path, b"first").unwrap();
        let identity = FileIdentity::read(&path).unwrap();
        std::fs::write(&path, b"changed-length").unwrap();
        assert!(identity.validate().is_err());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn temporary_png_owner_removes_its_file_idempotently() {
        let path = test_path("ocr-image");
        std::fs::write(&path, b"png").unwrap();
        let mut temporary = TemporaryPng {
            path: path.clone(),
            removed: false,
        };
        temporary.remove().unwrap();
        temporary.remove().unwrap();
        assert!(!path.exists());
    }
}
