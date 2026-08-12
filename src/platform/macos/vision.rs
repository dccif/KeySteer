use std::ffi::c_char;
use std::ptr::NonNull;

use crate::api::command::{UiScanStatus, VisionOptions};
use crate::api::geometry::{Rect, UiTarget};

#[repr(C)]
#[derive(Clone, Copy)]
struct NativePoint {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct NativeSize {
    width: f64,
    height: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct NativeRect {
    origin: NativePoint,
    size: NativeSize,
}

#[repr(C)]
struct NativeConfig {
    detect_text: bool,
    detect_rectangles: bool,
    timeout_ms: u64,
    minimum_confidence: f64,
    rectangle_max_candidates: u64,
    rectangle_min_size: f64,
    rectangle_min_aspect: f64,
    rectangle_max_aspect: f64,
}

#[repr(C)]
struct NativeRegion {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    confidence: f64,
    is_text: bool,
    label: *mut c_char,
    label_len: u64,
}

#[repr(C)]
struct NativeResult {
    abi_version: u32,
    result_size: u32,
    region_stride: u32,
    status: i32,
    regions: *mut NativeRegion,
    count: u64,
    message: *mut c_char,
    message_len: u64,
    captured_bounds: NativeRect,
}

const MAX_VISION_REGIONS: usize = 2_000;
const MAX_VISION_LABEL_BYTES: usize = 64 * 1024;
const MAX_VISION_MESSAGE_BYTES: usize = 4 * 1024;
const VISION_ABI_VERSION: u32 = 2;

struct OwnedVisionResult(NonNull<NativeResult>);

impl OwnedVisionResult {
    fn new(result: *mut NativeResult) -> Result<Option<Self>, String> {
        let Some(result) = NonNull::new(result) else {
            return Ok(None);
        };
        if result.as_ptr().addr() % std::mem::align_of::<NativeResult>() != 0 {
            return Err("Vision returned a misaligned result pointer".into());
        }
        Ok(Some(Self(result)))
    }

    fn as_ref(&self) -> &NativeResult {
        // SAFETY: the bridge returned an owned non-null result and this wrapper
        // retains it until Drop. No mutable access is exposed while borrowed.
        unsafe { self.0.as_ref() }
    }

    fn regions(&self) -> Result<&[NativeRegion], String> {
        let raw = self.as_ref();
        if raw.abi_version != VISION_ABI_VERSION
            || raw.result_size as usize != std::mem::size_of::<NativeResult>()
            || raw.region_stride as usize != std::mem::size_of::<NativeRegion>()
        {
            return Err(format!(
                "Vision ABI mismatch: version={}, result_size={}, region_stride={}",
                raw.abi_version, raw.result_size, raw.region_stride
            ));
        }
        let count = usize::try_from(raw.count)
            .map_err(|_| "Vision returned an unrepresentable region count".to_string())?;
        if count > MAX_VISION_REGIONS {
            return Err(format!(
                "Vision returned {count} regions; maximum is {MAX_VISION_REGIONS}"
            ));
        }
        if count == 0 {
            return Ok(&[]);
        }
        let regions = NonNull::new(raw.regions)
            .ok_or_else(|| "Vision returned regions with a null pointer".to_string())?;
        if regions.as_ptr().addr() % std::mem::align_of::<NativeRegion>() != 0 {
            return Err("Vision returned a misaligned region pointer".into());
        }
        let _byte_len = count
            .checked_mul(std::mem::size_of::<NativeRegion>())
            .filter(|&length| length <= isize::MAX as usize)
            .ok_or_else(|| "Vision returned an unrepresentable region buffer".to_string())?;
        // SAFETY: the native bridge allocates `count` contiguous NativeRegion
        // values, the count is capped by the shared ABI maximum, and the owner
        // keeps the allocation alive for the returned borrow.
        Ok(unsafe { std::slice::from_raw_parts(regions.as_ptr(), count) })
    }
}

impl Drop for OwnedVisionResult {
    fn drop(&mut self) {
        // SAFETY: this wrapper is constructed only from an owned bridge result
        // and Drop is the sole release path.
        unsafe { NmkFreeVisionResult(self.0.as_ptr()) };
    }
}

unsafe extern "C" {
    safe fn NmkSetLatestVisionScan(scan_id: u64);
    safe fn NmkDetectVisionElements(
        bounds: NativeRect,
        config: NativeConfig,
        scan_id: u64,
    ) -> *mut NativeResult;
    fn NmkFreeVisionResult(result: *mut NativeResult);
}

#[derive(Debug, Clone)]
struct Candidate {
    target: UiTarget,
    confidence: f64,
    is_text: bool,
}

pub(super) fn mark_latest(scan_id: u64) {
    NmkSetLatestVisionScan(scan_id);
}

pub fn detect(
    scan_id: u64,
    bounds: Rect,
    options: &VisionOptions,
) -> (Vec<UiTarget>, UiScanStatus) {
    let native_bounds = NativeRect {
        origin: NativePoint {
            x: bounds.x,
            y: bounds.y,
        },
        size: NativeSize {
            width: bounds.width,
            height: bounds.height,
        },
    };
    let config = NativeConfig {
        detect_text: options.detect_text,
        detect_rectangles: options.detect_rectangles,
        timeout_ms: options.request_timeout_ms,
        minimum_confidence: options.minimum_confidence,
        rectangle_max_candidates: options.rectangle_max_candidates as u64,
        rectangle_min_size: options.rectangle_min_size,
        rectangle_min_aspect: options.rectangle_min_aspect,
        rectangle_max_aspect: options.rectangle_max_aspect,
    };
    let result =
        match OwnedVisionResult::new(NmkDetectVisionElements(native_bounds, config, scan_id)) {
            Ok(Some(result)) => result,
            Ok(None) => {
                return (
                    Vec::new(),
                    UiScanStatus::Failed("Vision could not allocate a result".into()),
                );
            }
            Err(error) => return (Vec::new(), UiScanStatus::Failed(error)),
        };
    let raw = result.as_ref();
    let coordinate_bounds =
        capture_bounds_or_window(native_rect_to_rect(raw.captured_bounds), bounds);
    if raw.abi_version != VISION_ABI_VERSION
        || raw.result_size as usize != std::mem::size_of::<NativeResult>()
    {
        return (
            Vec::new(),
            UiScanStatus::Failed("Vision returned incompatible result metadata".into()),
        );
    }
    let message = match native_string(raw.message, raw.message_len, MAX_VISION_MESSAGE_BYTES) {
        Ok(message) if !message.is_empty() => message,
        Ok(_) => "Vision scan failed".into(),
        Err(error) => return (Vec::new(), UiScanStatus::Failed(error)),
    };
    let status = match raw.status {
        0 => UiScanStatus::Success,
        1 => UiScanStatus::PermissionDenied(message),
        2 => UiScanStatus::TimedOut,
        4 => UiScanStatus::ContextChanged,
        5 => UiScanStatus::Unsupported(message),
        _ => UiScanStatus::Failed(message),
    };
    let regions = match result.regions() {
        Ok(regions) => regions,
        Err(error) => return (Vec::new(), UiScanStatus::Failed(error)),
    };
    let mut candidates = Vec::with_capacity(regions.len());
    for region in regions {
        match classify(region, coordinate_bounds, options) {
            Ok(Some(candidate)) => candidates.push(candidate),
            Ok(None) => {}
            Err(error) => return (Vec::new(), UiScanStatus::Failed(error)),
        }
    }
    let targets = merge_candidates(candidates, options.merge_iou_threshold);
    (targets, status)
}

fn native_rect_to_rect(rect: NativeRect) -> Rect {
    Rect::new(
        rect.origin.x,
        rect.origin.y,
        rect.size.width,
        rect.size.height,
    )
}

fn capture_bounds_or_window(captured_bounds: Rect, window_bounds: Rect) -> Rect {
    if captured_bounds.is_empty()
        || !captured_bounds.x.is_finite()
        || !captured_bounds.y.is_finite()
        || !captured_bounds.width.is_finite()
        || !captured_bounds.height.is_finite()
    {
        window_bounds
    } else {
        captured_bounds
    }
}

fn native_string(value: *const c_char, length: u64, maximum: usize) -> Result<String, String> {
    let length = usize::try_from(length)
        .map_err(|_| "Vision returned an unrepresentable string length".to_string())?;
    if length > maximum {
        return Err(format!(
            "Vision returned a {length}-byte string; maximum is {maximum}"
        ));
    }
    if length == 0 {
        return Ok(String::new());
    }
    let value = NonNull::new(value.cast_mut())
        .ok_or_else(|| "Vision returned a non-empty string with a null pointer".to_string())?;
    // SAFETY: the ABI supplies an explicit bounded byte length, and the owning
    // result keeps every strdup allocation alive for this borrow.
    let bytes = unsafe { std::slice::from_raw_parts(value.as_ptr().cast::<u8>(), length) };
    Ok(String::from_utf8_lossy(bytes).into_owned())
}

fn classify(
    region: &NativeRegion,
    bounds: Rect,
    options: &VisionOptions,
) -> Result<Option<Candidate>, String> {
    let rect = normalized_to_global(
        bounds,
        Rect::new(region.x, region.y, region.width, region.height),
    );
    if rect.is_empty() || !rect.width.is_finite() || !rect.height.is_finite() {
        return Ok(None);
    }
    let aspect = rect.width / rect.height.max(f64::EPSILON);
    let (role, native_role) = if region.is_text {
        if aspect >= options.link_min_aspect
            && rect.height <= options.link_max_height
            && rect.width >= options.link_min_width
        {
            ("link", "vision:text-link")
        } else if region.confidence >= options.generic_clickable_min_confidence {
            ("control", "vision:text")
        } else {
            return Ok(None);
        }
    } else if rect.width <= options.checkbox_max_size
        && rect.height <= options.checkbox_max_size
        && (0.75..=1.35).contains(&aspect)
    {
        ("checkbox", "vision:checkbox")
    } else if region.confidence >= options.button_min_confidence
        && (options.button_min_aspect..=options.button_max_aspect).contains(&aspect)
    {
        ("button", "vision:button")
    } else if region.confidence >= options.generic_clickable_min_confidence
        && rect.width <= options.button_icon_max_size
        && rect.height <= options.button_icon_max_size
    {
        ("button", "vision:icon-button")
    } else if rect.width >= options.image_min_size && rect.height >= options.image_min_size {
        ("image", "vision:image")
    } else if region.confidence >= options.generic_clickable_min_confidence {
        ("control", "vision:rectangle")
    } else {
        return Ok(None);
    };
    Ok(Some(Candidate {
        target: UiTarget {
            rect,
            name: native_string(region.label, region.label_len, MAX_VISION_LABEL_BYTES)?,
            role: role.into(),
            native_role: Some(native_role.into()),
        },
        confidence: region.confidence,
        is_text: region.is_text,
    }))
}

fn normalized_to_global(bounds: Rect, normalized: Rect) -> Rect {
    Rect::new(
        bounds.x + normalized.x * bounds.width,
        bounds.y + (1.0 - normalized.y - normalized.height) * bounds.height,
        normalized.width * bounds.width,
        normalized.height * bounds.height,
    )
}

#[cfg(test)]
fn merge_targets(
    mut primary: Vec<UiTarget>,
    supplementary: Vec<UiTarget>,
    threshold: f64,
) -> Vec<UiTarget> {
    for target in supplementary {
        if primary
            .iter()
            .all(|existing| intersection_over_union(existing.rect, target.rect) < threshold)
        {
            primary.push(target);
        }
    }
    primary
}

fn merge_candidates(mut candidates: Vec<Candidate>, threshold: f64) -> Vec<UiTarget> {
    candidates.sort_by(|a, b| {
        b.is_text
            .cmp(&a.is_text)
            .then_with(|| b.confidence.total_cmp(&a.confidence))
    });
    let mut targets: Vec<UiTarget> = Vec::new();
    for candidate in candidates {
        if targets.iter().all(|existing| {
            intersection_over_union(existing.rect, candidate.target.rect) < threshold
        }) {
            targets.push(candidate.target);
        }
    }
    targets
}

fn intersection_over_union(a: Rect, b: Rect) -> f64 {
    let Some(intersection) = a.intersect(&b) else {
        return 0.0;
    };
    let intersection_area = intersection.width * intersection.height;
    let union = a.width * a.height + b.width * b.height - intersection_area;
    if union <= 0.0 {
        0.0
    } else {
        intersection_area / union
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_vision_bottom_left_coordinates_to_global_top_left() {
        let bounds = Rect::new(100.0, -200.0, 800.0, 600.0);
        let converted = normalized_to_global(bounds, Rect::new(0.25, 0.6, 0.5, 0.2));
        assert_eq!(converted, Rect::new(300.0, -80.0, 400.0, 120.0));
    }

    #[test]
    fn maps_vision_regions_to_the_actual_clipped_capture_bounds() {
        let window_bounds = Rect::new(-1600.0, 80.0, 1000.0, 700.0);
        let captured_bounds = Rect::new(-1600.0, 80.0, 600.0, 700.0);
        let bounds = capture_bounds_or_window(captured_bounds, window_bounds);
        assert_eq!(
            normalized_to_global(bounds, Rect::new(0.5, 0.25, 0.25, 0.5)),
            Rect::new(-1300.0, 255.0, 150.0, 350.0)
        );
    }

    #[test]
    fn falls_back_to_window_bounds_when_the_bridge_returns_no_capture_bounds() {
        let window_bounds = Rect::new(-1600.0, 80.0, 1000.0, 700.0);
        assert_eq!(
            capture_bounds_or_window(Rect::default(), window_bounds),
            window_bounds
        );
    }

    #[test]
    fn classifier_uses_link_and_checkbox_thresholds() {
        let bounds = Rect::new(0.0, 0.0, 1000.0, 800.0);
        let options = VisionOptions::default();
        let empty = std::ffi::CString::new("").unwrap();
        let link = NativeRegion {
            x: 0.1,
            y: 0.8,
            width: 0.3,
            height: 0.03,
            confidence: 0.9,
            is_text: true,
            label: empty.as_ptr().cast_mut(),
            label_len: 0,
        };
        assert_eq!(
            classify(&link, bounds, &options)
                .unwrap()
                .unwrap()
                .target
                .role,
            "link"
        );
        let checkbox = NativeRegion {
            x: 0.1,
            y: 0.7,
            width: 0.025,
            height: 0.03,
            confidence: 0.9,
            is_text: false,
            label: empty.as_ptr().cast_mut(),
            label_len: 0,
        };
        assert_eq!(
            classify(&checkbox, bounds, &options)
                .unwrap()
                .unwrap()
                .target
                .role,
            "checkbox"
        );
    }

    #[test]
    fn classifier_rejects_low_confidence_generic_rectangles() {
        let bounds = Rect::new(0.0, 0.0, 1000.0, 800.0);
        let options = VisionOptions::default();
        let empty = std::ffi::CString::new("").unwrap();
        let region = NativeRegion {
            x: 0.1,
            y: 0.5,
            width: 0.04,
            height: 0.1,
            confidence: 0.1,
            is_text: false,
            label: empty.as_ptr().cast_mut(),
            label_len: 0,
        };
        assert!(classify(&region, bounds, &options).unwrap().is_none());
    }

    #[test]
    fn native_strings_reject_malformed_metadata_without_scanning_for_nul() {
        assert_eq!(native_string(std::ptr::null(), 0, 8).unwrap(), "");
        assert!(native_string(std::ptr::null(), 1, 8).is_err());
        assert!(native_string(std::ptr::null(), 9, 8).is_err());
        let bytes = b"hello";
        assert_eq!(
            native_string(bytes.as_ptr().cast(), bytes.len() as u64, 8).unwrap(),
            "hello"
        );
    }

    #[test]
    fn iou_merge_prefers_primary_targets() {
        let target = |x| UiTarget {
            rect: Rect::new(x, 0.0, 100.0, 100.0),
            name: String::new(),
            role: "button".into(),
            native_role: None,
        };
        assert_eq!(
            merge_targets(vec![target(0.0)], vec![target(10.0)], 0.5).len(),
            1
        );
        assert_eq!(
            merge_targets(vec![target(0.0)], vec![target(80.0)], 0.5).len(),
            2
        );
    }
}
