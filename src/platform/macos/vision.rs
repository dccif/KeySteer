use std::ffi::{CStr, c_char};

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
}

#[repr(C)]
struct NativeResult {
    status: i32,
    regions: *mut NativeRegion,
    count: u64,
    message: *mut c_char,
    captured_bounds: NativeRect,
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
    let result = NmkDetectVisionElements(native_bounds, config, scan_id);
    if result.is_null() {
        return (
            Vec::new(),
            UiScanStatus::Failed("Vision could not allocate a result".into()),
        );
    }
    let raw = unsafe { &*result };
    let coordinate_bounds =
        capture_bounds_or_window(native_rect_to_rect(raw.captured_bounds), bounds);
    let message = c_string(raw.message);
    let status = match raw.status {
        0 => UiScanStatus::Success,
        1 => UiScanStatus::PermissionDenied(message),
        2 => UiScanStatus::TimedOut,
        4 => UiScanStatus::ContextChanged,
        5 => UiScanStatus::Unsupported(message),
        _ => UiScanStatus::Failed(message),
    };
    let regions = if raw.regions.is_null() {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(raw.regions, raw.count as usize) }
    };
    let mut candidates = Vec::with_capacity(regions.len());
    for region in regions {
        if let Some(candidate) = classify(region, coordinate_bounds, options) {
            candidates.push(candidate);
        }
    }
    let targets = merge_candidates(candidates, options.merge_iou_threshold);
    unsafe { NmkFreeVisionResult(result) };
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

fn c_string(value: *const c_char) -> String {
    if value.is_null() {
        return "Vision scan failed".into();
    }
    unsafe { CStr::from_ptr(value) }
        .to_string_lossy()
        .into_owned()
}

fn classify(region: &NativeRegion, bounds: Rect, options: &VisionOptions) -> Option<Candidate> {
    let rect = normalized_to_global(
        bounds,
        Rect::new(region.x, region.y, region.width, region.height),
    );
    if rect.is_empty() || !rect.width.is_finite() || !rect.height.is_finite() {
        return None;
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
            return None;
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
        return None;
    };
    Some(Candidate {
        target: UiTarget {
            rect,
            name: c_string(region.label),
            role: role.into(),
            native_role: Some(native_role.into()),
        },
        confidence: region.confidence,
        is_text: region.is_text,
    })
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
        };
        assert_eq!(
            classify(&link, bounds, &options).unwrap().target.role,
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
        };
        assert_eq!(
            classify(&checkbox, bounds, &options).unwrap().target.role,
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
        };
        assert!(classify(&region, bounds, &options).is_none());
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
