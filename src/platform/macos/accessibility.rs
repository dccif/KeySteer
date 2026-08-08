//! Bounded asynchronous macOS Accessibility traversal for UI hints.

use std::collections::BTreeSet;
use std::ffi::{c_double, c_int, c_void};
use std::ptr;
use std::time::{Duration, Instant};

use core_foundation::array::CFArray;
use core_foundation::base::{CFTypeRef, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::string::{CFString, CFStringRef};
use core_graphics::geometry::{CGPoint, CGSize};
use objc2_app_kit::NSWorkspace;

use crate::api::command::UiScanRequest;
use crate::api::geometry::{Rect, UiTarget};

use super::native::OwnedCf;

const AX_OK: i32 = 0;
const AX_VALUE_CGPOINT: i32 = 1;
const AX_VALUE_CGSIZE: i32 = 2;
const SCAN_BUDGET: Duration = Duration::from_millis(500);
const MAX_TARGETS: usize = 2_000;
const NODE_TIMEOUT_SECONDS: c_double = 0.05;

type AXUIElementRef = *const c_void;
type AXValueRef = *const c_void;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXUIElementCreateApplication(pid: libc::pid_t) -> AXUIElementRef;
    fn AXUIElementCopyAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: *mut CFTypeRef,
    ) -> c_int;
    fn AXUIElementCopyActionNames(element: AXUIElementRef, names: *mut *const c_void) -> c_int;
    fn AXUIElementSetMessagingTimeout(element: AXUIElementRef, timeout: c_double) -> c_int;
    fn AXValueGetValue(value: AXValueRef, value_type: c_int, output: *mut c_void) -> bool;
}

pub fn ax_roles_for(semantic_roles: &[String]) -> Vec<String> {
    let mut roles = Vec::new();
    for role in semantic_roles {
        if let Some(native) = role.strip_prefix("ax:") {
            if !native.is_empty() && !roles.iter().any(|value| value == native) {
                roles.push(native.to_string());
            }
            continue;
        }
        let native = match role.as_str() {
            "button" => &["AXButton", "AXMenuButton", "AXPopUpButton"][..],
            "menu_button" => &["AXMenuButton"][..],
            "popup_button" => &["AXPopUpButton"][..],
            "link" => &["AXLink"][..],
            "checkbox" | "switch" => &["AXCheckBox"][..],
            "radio" => &["AXRadioButton"][..],
            "tab" => &["AXTabButton"][..],
            "menu_item" => &["AXMenuItem"][..],
            "text_field" => &["AXTextField", "AXTextArea", "AXSearchField"][..],
            "text_area" => &["AXTextArea"][..],
            "search_field" => &["AXSearchField"][..],
            "list_item" => &["AXRow", "AXOutlineRow"][..],
            "cell" => &["AXCell"][..],
            "tree_item" => &["AXDisclosureTriangle"][..],
            "combo_box" => &["AXComboBox"][..],
            "slider" => &["AXSlider"][..],
            "spinner" | "stepper" => &["AXIncrementor"][..],
            "scrollbar" => &["AXScrollBar"][..],
            "toolbar_button" => &["AXToolbarButton"][..],
            "image" => &["AXImage"][..],
            _ => &[][..],
        };
        for native_role in native {
            if !roles.iter().any(|value| value == native_role) {
                roles.push((*native_role).to_string());
            }
        }
    }
    roles
}

pub(crate) fn frontmost_pid() -> Option<libc::pid_t> {
    Some(
        NSWorkspace::sharedWorkspace()
            .frontmostApplication()?
            .processIdentifier(),
    )
}

pub(crate) fn focused_window_bounds(pid: libc::pid_t) -> Result<Rect, String> {
    let Some(application) =
        (unsafe { OwnedCf::from_create_rule(AXUIElementCreateApplication(pid).cast()) })
    else {
        return Err("cannot create AX application element".into());
    };
    unsafe { AXUIElementSetMessagingTimeout(application.as_ptr(), NODE_TIMEOUT_SECONDS) };
    let window = match copy_attribute(application.as_ptr(), "AXFocusedWindow") {
        Some(window) => window,
        None => return Err("frontmost application has no focused window".into()),
    };
    element_rect(window.as_ptr())
        .ok_or_else(|| "focused window does not expose valid bounds".to_string())
}

pub(crate) fn scan_process_stream(
    pid: libc::pid_t,
    request: &UiScanRequest,
    mut on_batch: impl FnMut(Vec<UiTarget>),
) -> Result<Vec<UiTarget>, String> {
    let Some(application) =
        (unsafe { OwnedCf::from_create_rule(AXUIElementCreateApplication(pid).cast()) })
    else {
        return Err("cannot create AX application element".into());
    };
    unsafe {
        AXUIElementSetMessagingTimeout(application.as_ptr(), NODE_TIMEOUT_SECONDS);
    }

    let focused_window = copy_attribute(application.as_ptr(), "AXFocusedWindow");
    let root = focused_window
        .as_ref()
        .map_or(application.as_ptr(), OwnedCf::as_ptr);
    let allowed_roles = ax_roles_for(&request.roles);
    let mut scan = Scan {
        request,
        allowed_roles,
        deadline: Instant::now() + SCAN_BUDGET,
        targets: Vec::new(),
        emitted: 0,
        on_batch: &mut on_batch,
        seen: BTreeSet::new(),
    };
    scan.visit(root.cast(), 0);
    scan.flush();

    Ok(scan.targets)
}

struct Scan<'a> {
    request: &'a UiScanRequest,
    allowed_roles: Vec<String>,
    deadline: Instant,
    targets: Vec<UiTarget>,
    emitted: usize,
    on_batch: &'a mut dyn FnMut(Vec<UiTarget>),
    seen: BTreeSet<(i64, i64, i64, i64)>,
}

impl Scan<'_> {
    fn flush(&mut self) {
        if self.emitted < self.targets.len() {
            (self.on_batch)(self.targets[self.emitted..].to_vec());
            self.emitted = self.targets.len();
        }
    }

    fn visit(&mut self, element: AXUIElementRef, depth: u32) {
        if element.is_null()
            || depth > self.request.max_depth
            || self.targets.len() >= MAX_TARGETS
            || Instant::now() >= self.deadline
        {
            return;
        }

        let role = copy_string_attribute(element, "AXRole").unwrap_or_default();
        let role_allowed = self.request.roles.is_empty()
            || self.allowed_roles.iter().any(|allowed| allowed == &role);
        let rect = role_allowed.then(|| element_rect(element)).flatten();
        let in_bounds = rect.is_some_and(|rect| {
            self.request
                .bounds
                .is_none_or(|bounds| bounds.contains(&rect.center()))
        });
        let enabled = role_allowed && copy_bool_attribute(element, "AXEnabled").unwrap_or(true);
        let visible = role_allowed
            && (!self.request.visible_only
                || !copy_bool_attribute(element, "AXHidden").unwrap_or(false));
        // Standard controls are mouse-addressable even when an application does
        // not expose AXPress. Action discovery is the slower fallback for custom
        // controls and only runs for an otherwise eligible candidate.
        let clickable = !self.request.clickable_only
            || intrinsically_clickable(&role)
            || (enabled && visible && in_bounds && has_actions(element));
        let semantic_role = semantic_role(&role);

        if enabled
            && visible
            && clickable
            && in_bounds
            && let Some(rect) = rect
        {
            let key = normalized_rect(rect);
            if self.seen.insert(key) {
                self.targets.push(UiTarget {
                    rect,
                    name: accessible_name(element),
                    role: semantic_role.to_string(),
                    native_role: Some(role.clone()),
                });
                if self.targets.len() - self.emitted >= 8 {
                    self.flush();
                }
            }
        }

        if depth == self.request.max_depth {
            return;
        }
        let Some(children_ref) = copy_attribute(element, "AXChildren") else {
            return;
        };
        let children = unsafe {
            CFArray::<*const c_void>::wrap_under_create_rule(children_ref.into_raw().cast())
        };
        for child in &children {
            self.visit(*child, depth + 1);
            if self.targets.len() >= MAX_TARGETS || Instant::now() >= self.deadline {
                break;
            }
        }
    }
}

fn copy_attribute(element: AXUIElementRef, name: &str) -> Option<OwnedCf> {
    let name = CFString::new(name);
    let mut value: CFTypeRef = ptr::null();
    let error =
        unsafe { AXUIElementCopyAttributeValue(element, name.as_concrete_TypeRef(), &mut value) };
    if error != AX_OK {
        return None;
    }
    // SAFETY: AX CopyAttributeValue returns a +1 object on success.
    unsafe { OwnedCf::from_create_rule(value.cast()) }
}

fn copy_string_attribute(element: AXUIElementRef, name: &str) -> Option<String> {
    let value = copy_attribute(element, name)?;
    let value = unsafe { CFString::wrap_under_create_rule(value.into_raw().cast()) };
    Some(value.to_string())
}

fn copy_bool_attribute(element: AXUIElementRef, name: &str) -> Option<bool> {
    let value = copy_attribute(element, name)?;
    let value = unsafe { CFBoolean::wrap_under_create_rule(value.into_raw().cast()) };
    Some(bool::from(value))
}

fn element_rect(element: AXUIElementRef) -> Option<Rect> {
    let position = copy_attribute(element, "AXPosition")?;
    let size = copy_attribute(element, "AXSize")?;
    let mut point = CGPoint::new(0.0, 0.0);
    let mut dimensions = CGSize::new(0.0, 0.0);
    let point_ok = unsafe {
        AXValueGetValue(
            position.as_ptr(),
            AX_VALUE_CGPOINT,
            (&mut point as *mut CGPoint).cast(),
        )
    };
    let size_ok = unsafe {
        AXValueGetValue(
            size.as_ptr(),
            AX_VALUE_CGSIZE,
            (&mut dimensions as *mut CGSize).cast(),
        )
    };
    let rect = Rect::new(point.x, point.y, dimensions.width, dimensions.height);
    (point_ok && size_ok && !rect.is_empty() && rect.width.is_finite() && rect.height.is_finite())
        .then_some(rect)
}

fn intrinsically_clickable(role: &str) -> bool {
    matches!(
        role,
        "AXButton"
            | "AXMenuButton"
            | "AXPopUpButton"
            | "AXToolbarButton"
            | "AXLink"
            | "AXCheckBox"
            | "AXRadioButton"
            | "AXTabButton"
            | "AXMenuItem"
            | "AXTextField"
            | "AXTextArea"
            | "AXSearchField"
            | "AXRow"
            | "AXOutlineRow"
            | "AXCell"
            | "AXDisclosureTriangle"
            | "AXComboBox"
            | "AXSlider"
            | "AXIncrementor"
            | "AXScrollBar"
    )
}

fn has_actions(element: AXUIElementRef) -> bool {
    let mut actions = ptr::null();
    if unsafe { AXUIElementCopyActionNames(element, &mut actions) } != AX_OK || actions.is_null() {
        return false;
    }
    // SAFETY: AX CopyActionNames returned a non-null +1 array above.
    let actions = unsafe { OwnedCf::from_create_rule(actions) };
    let Some(actions) = actions else {
        return false;
    };
    let actions =
        unsafe { CFArray::<*const c_void>::wrap_under_create_rule(actions.into_raw().cast()) };
    !actions.is_empty()
}

fn accessible_name(element: AXUIElementRef) -> String {
    ["AXTitle", "AXDescription", "AXHelp"]
        .into_iter()
        .find_map(|attribute| {
            copy_string_attribute(element, attribute).filter(|value| !value.trim().is_empty())
        })
        .unwrap_or_default()
}

fn semantic_role(native_role: &str) -> &'static str {
    match native_role {
        "AXButton" | "AXMenuButton" | "AXPopUpButton" | "AXToolbarButton" => "button",
        "AXLink" => "link",
        "AXCheckBox" => "checkbox",
        "AXRadioButton" => "radio",
        "AXTabButton" => "tab",
        "AXMenuItem" => "menu_item",
        "AXTextField" | "AXTextArea" | "AXSearchField" => "text_field",
        "AXRow" | "AXOutlineRow" => "list_item",
        "AXDisclosureTriangle" => "tree_item",
        "AXComboBox" => "combo_box",
        "AXSlider" => "slider",
        "AXIncrementor" => "spinner",
        "AXScrollBar" => "scrollbar",
        "AXImage" => "image",
        _ => "control",
    }
}

fn normalized_rect(rect: Rect) -> (i64, i64, i64, i64) {
    (
        (rect.x * 2.0).round() as i64,
        (rect.y * 2.0).round() as i64,
        (rect.width * 2.0).round() as i64,
        (rect.height * 2.0).round() as i64,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semantic_roles_expand_without_duplicates() {
        let roles = ax_roles_for(&[
            "button".into(),
            "link".into(),
            "cell".into(),
            "ax:AXCustomControl".into(),
            "button".into(),
        ]);
        assert!(roles.contains(&"AXButton".into()));
        assert!(roles.contains(&"AXLink".into()));
        assert!(roles.contains(&"AXCell".into()));
        assert!(roles.contains(&"AXCustomControl".into()));
        let unique: BTreeSet<_> = roles.iter().collect();
        assert_eq!(unique.len(), roles.len());
    }

    #[test]
    fn standard_ax_controls_do_not_require_action_discovery() {
        assert!(intrinsically_clickable("AXButton"));
        assert!(intrinsically_clickable("AXTextField"));
        assert!(intrinsically_clickable("AXCell"));
        assert!(!intrinsically_clickable("AXGroup"));
    }

    #[test]
    fn normalized_rect_deduplicates_subpixel_noise() {
        let first = normalized_rect(Rect::new(10.01, 20.01, 30.01, 40.01));
        let second = normalized_rect(Rect::new(10.02, 20.02, 30.02, 40.02));
        assert_eq!(first, second);
    }
}
