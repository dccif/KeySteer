//! Bounded asynchronous macOS Accessibility traversal for UI hints.

use std::collections::HashSet;
use std::ffi::{c_float, c_int, c_void};
use std::ptr;
use std::time::{Duration, Instant};

use core_foundation::ConcreteCFType;
use core_foundation::array::CFArray;
use core_foundation::base::{CFGetTypeID, CFType, CFTypeRef, TCFType};
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
// Apple declares AXUIElementSetMessagingTimeout's value as a C `float`.
// Keeping the constant typed prevents an accidental ABI-widening to `double`.
const NODE_TIMEOUT_SECONDS: c_float = 0.05;

type AXUIElementRef = *const c_void;
type AXValueRef = *const c_void;

struct AxApplication(OwnedCf);

impl AxApplication {
    fn new(pid: libc::pid_t) -> Result<Self, String> {
        // SAFETY: AX returns a +1 application object for the pid; the owned
        // wrapper takes that create-rule reference exactly once. The timeout
        // is finite and is installed before any synchronous attribute query.
        let application =
            unsafe { OwnedCf::from_create_rule(AXUIElementCreateApplication(pid).cast()) }
                .ok_or_else(|| "cannot create AX application element".to_string())?;
        // SAFETY: `application` is a live AXUIElement and the timeout is finite.
        let error =
            unsafe { AXUIElementSetMessagingTimeout(application.as_ptr(), NODE_TIMEOUT_SECONDS) };
        if error != AX_OK {
            return Err(format!(
                "cannot set the AX messaging timeout for pid {pid}: AXError {error}"
            ));
        }
        Ok(Self(application))
    }

    fn as_ptr(&self) -> *const c_void {
        self.0.as_ptr()
    }
}

struct AxAttributes {
    role: CFString,
    enabled: CFString,
    hidden: CFString,
    children: CFString,
    visible_children: CFString,
    visible_rows: CFString,
    position: CFString,
    size: CFString,
    title: CFString,
    description: CFString,
    help: CFString,
    focused_window: CFString,
}

impl AxAttributes {
    fn new() -> Self {
        Self {
            role: CFString::new("AXRole"),
            enabled: CFString::new("AXEnabled"),
            hidden: CFString::new("AXHidden"),
            children: CFString::new("AXChildren"),
            visible_children: CFString::new("AXVisibleChildren"),
            visible_rows: CFString::new("AXVisibleRows"),
            position: CFString::new("AXPosition"),
            size: CFString::new("AXSize"),
            title: CFString::new("AXTitle"),
            description: CFString::new("AXDescription"),
            help: CFString::new("AXHelp"),
            focused_window: CFString::new("AXFocusedWindow"),
        }
    }
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXUIElementCreateApplication(pid: libc::pid_t) -> AXUIElementRef;
    fn AXUIElementCopyAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: *mut CFTypeRef,
    ) -> c_int;
    fn AXUIElementCopyActionNames(element: AXUIElementRef, names: *mut *const c_void) -> c_int;
    fn AXUIElementSetMessagingTimeout(element: AXUIElementRef, timeout: c_float) -> c_int;
    fn AXUIElementGetTypeID() -> usize;
    fn AXValueGetTypeID() -> usize;
    fn AXValueGetType(value: AXValueRef) -> c_int;
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
    let application = AxApplication::new(pid)?;
    let attributes = AxAttributes::new();
    let window = match copy_attribute(application.as_ptr(), &attributes.focused_window) {
        Some(window) if is_ax_element(window.as_ptr()) => window,
        None => return Err("frontmost application has no focused window".into()),
        Some(_) => return Err("focused AX window has an unexpected Core Foundation type".into()),
    };
    element_rect(window.as_ptr(), &attributes)
        .ok_or_else(|| "focused window does not expose valid bounds".to_string())
}

pub(crate) fn scan_process_stream(
    pid: libc::pid_t,
    request: &UiScanRequest,
    is_current: impl Fn() -> bool,
    mut on_batch: impl FnMut(Vec<UiTarget>),
) -> Result<(), String> {
    let application = AxApplication::new(pid)?;

    let attributes = AxAttributes::new();
    let focused_window = copy_attribute(application.as_ptr(), &attributes.focused_window);
    let focused_window = match focused_window.as_ref() {
        Some(window) if is_ax_element(window.as_ptr()) => Some(window),
        // A window transition can briefly expose a stale or unexpected value.
        // Do not publish unbounded application-root targets in that state.
        Some(_) => return Ok(()),
        None => None,
    };
    let window_bounds =
        focused_window.and_then(|window| element_rect(window.as_ptr(), &attributes));
    if focused_window.is_some() && window_bounds.is_none() {
        return Ok(());
    }
    let Ok(scan_bounds) = visible_scan_bounds(window_bounds, request.bounds) else {
        return Ok(());
    };
    let root = focused_window.map_or(application.as_ptr(), |window| window.as_ptr());
    let allowed_roles = ax_roles_for(&request.roles).into_iter().collect();
    let mut scan = Scan {
        request,
        scan_bounds,
        attributes,
        allowed_roles,
        deadline: Instant::now() + SCAN_BUDGET,
        batch: Vec::with_capacity(24),
        target_count: 0,
        on_batch: &mut on_batch,
        is_current: &is_current,
        seen: HashSet::with_capacity(128),
    };
    scan.visit(root.cast(), 0);
    if (scan.is_current)() {
        scan.flush();
    }

    Ok(())
}

struct Scan<'a> {
    request: &'a UiScanRequest,
    scan_bounds: Option<Rect>,
    attributes: AxAttributes,
    allowed_roles: HashSet<String>,
    deadline: Instant,
    batch: Vec<UiTarget>,
    target_count: usize,
    on_batch: &'a mut dyn FnMut(Vec<UiTarget>),
    is_current: &'a dyn Fn() -> bool,
    seen: HashSet<(i64, i64, i64, i64)>,
}

impl Scan<'_> {
    fn flush(&mut self) {
        if !self.batch.is_empty() {
            let batch = std::mem::replace(&mut self.batch, Vec::with_capacity(24));
            (self.on_batch)(batch);
        }
    }

    fn visit(&mut self, element: AXUIElementRef, depth: u32) {
        if !is_ax_element(element)
            || depth > self.request.max_depth
            || self.target_count >= MAX_TARGETS
            || Instant::now() >= self.deadline
            || !(self.is_current)()
        {
            return;
        }

        let role = copy_string_attribute(element, &self.attributes.role).unwrap_or_default();
        let (prefer_visible_rows, prefer_visible_children) =
            visible_collection_preferences(&role, self.request.visible_only);
        let role_allowed =
            self.request.roles.is_empty() || self.allowed_roles.contains(role.as_str());
        let rect = role_allowed
            .then(|| element_rect(element, &self.attributes))
            .flatten();
        let in_bounds = rect.is_some_and(|rect| {
            self.scan_bounds
                .is_none_or(|bounds| bounds.contains(&rect.center()))
        });
        let enabled =
            role_allowed && copy_bool_attribute(element, &self.attributes.enabled).unwrap_or(true);
        let visible = role_allowed
            && (!self.request.visible_only
                || !copy_bool_attribute(element, &self.attributes.hidden).unwrap_or(false));
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
                self.batch.push(UiTarget {
                    rect,
                    name: accessible_name(element, &self.attributes),
                    role: semantic_role.to_string(),
                    native_role: Some(role),
                });
                self.target_count += 1;
                if self.batch.len() >= 24 {
                    self.flush();
                }
            }
        }

        if depth == self.request.max_depth {
            return;
        }
        let Some(children) = visible_children_for_role(
            element,
            prefer_visible_rows,
            prefer_visible_children,
            &self.attributes,
        ) else {
            return;
        };
        for child in &children {
            self.visit(*child, depth + 1);
            if self.target_count >= MAX_TARGETS
                || Instant::now() >= self.deadline
                || !(self.is_current)()
            {
                break;
            }
        }
    }
}

/// Prefer the AX server's viewport-aware collections for scroll containers.
/// Unsupported attributes fall back to AXChildren, preserving compatibility
/// with custom controls while avoiding traversal of off-screen table rows in
/// standard AppKit lists.
fn visible_children_for_role(
    element: AXUIElementRef,
    prefer_visible_rows: bool,
    prefer_visible_children: bool,
    attributes: &AxAttributes,
) -> Option<CFArray<*const c_void>> {
    let visible = if prefer_visible_rows {
        copy_array_attribute(element, &attributes.visible_children)
            .or_else(|| copy_array_attribute(element, &attributes.visible_rows))
    } else if prefer_visible_children {
        copy_array_attribute(element, &attributes.visible_children)
    } else {
        None
    };
    visible.or_else(|| copy_array_attribute(element, &attributes.children))
}

fn prefers_visible_rows(role: &str) -> bool {
    matches!(role, "AXTable" | "AXOutline" | "AXGrid")
}

fn prefers_visible_children(role: &str) -> bool {
    matches!(
        role,
        "AXScrollArea" | "AXList" | "AXBrowser" | "AXCollectionList" | "AXWebArea"
    )
}

fn visible_collection_preferences(role: &str, visible_only: bool) -> (bool, bool) {
    if visible_only {
        (prefers_visible_rows(role), prefers_visible_children(role))
    } else {
        (false, false)
    }
}

fn copy_array_attribute(
    element: AXUIElementRef,
    name: &CFString,
) -> Option<CFArray<*const c_void>> {
    downcast_cf(copy_attribute(element, name)?)
}

fn copy_attribute(element: AXUIElementRef, name: &CFString) -> Option<OwnedCf> {
    let mut value: CFTypeRef = ptr::null();
    // SAFETY: `element` and `name` are live for the call and `value` is a valid
    // out-pointer that receives a +1 object only on AX_OK.
    let error =
        unsafe { AXUIElementCopyAttributeValue(element, name.as_concrete_TypeRef(), &mut value) };
    if error != AX_OK {
        return None;
    }
    // SAFETY: AX CopyAttributeValue returns a +1 object on success.
    unsafe { OwnedCf::from_create_rule(value.cast()) }
}

fn copy_string_attribute(element: AXUIElementRef, name: &CFString) -> Option<String> {
    let value = copy_attribute(element, name)?;
    let value = downcast_cf::<CFString>(value)?;
    Some(value.to_string())
}

fn copy_bool_attribute(element: AXUIElementRef, name: &CFString) -> Option<bool> {
    let value = copy_attribute(element, name)?;
    let value = downcast_cf::<CFBoolean>(value)?;
    Some(bool::from(value))
}

fn downcast_cf<T: ConcreteCFType>(value: OwnedCf) -> Option<T> {
    // SAFETY: OwnedCf holds one live +1 Core Foundation object. Ownership is
    // transferred into CFType exactly once; downcast_into checks CFTypeID and
    // releases the object on mismatch.
    let value = unsafe { CFType::wrap_under_create_rule(value.into_raw().cast()) };
    value.downcast_into::<T>()
}

fn is_ax_element(value: *const c_void) -> bool {
    !value.is_null()
        // SAFETY: callers pass either a live Core Foundation object obtained
        // from AX or null, rejected above. Both type-id functions are pure.
        && unsafe { CFGetTypeID(value) == AXUIElementGetTypeID() }
}

fn element_rect(element: AXUIElementRef, attributes: &AxAttributes) -> Option<Rect> {
    let position = copy_attribute(element, &attributes.position)?;
    let size = copy_attribute(element, &attributes.size)?;
    let mut point = CGPoint::new(0.0, 0.0);
    let mut dimensions = CGSize::new(0.0, 0.0);
    // SAFETY: both +1 objects are live Core Foundation values. CFGetTypeID
    // first proves they are AXValue objects before AXValueGetType inspects the
    // payload, and AXValueGetValue writes into correctly sized outputs.
    let (point_ok, size_ok) = unsafe {
        if CFGetTypeID(position.as_ptr()) != AXValueGetTypeID()
            || CFGetTypeID(size.as_ptr()) != AXValueGetTypeID()
            || AXValueGetType(position.as_ptr()) != AX_VALUE_CGPOINT
            || AXValueGetType(size.as_ptr()) != AX_VALUE_CGSIZE
        {
            return None;
        }
        (
            AXValueGetValue(
                position.as_ptr(),
                AX_VALUE_CGPOINT,
                (&mut point as *mut CGPoint).cast(),
            ),
            AXValueGetValue(
                size.as_ptr(),
                AX_VALUE_CGSIZE,
                (&mut dimensions as *mut CGSize).cast(),
            ),
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
    // SAFETY: `element` is live and `actions` is a valid out-pointer receiving
    // a +1 array only when AX_OK is returned.
    if unsafe { AXUIElementCopyActionNames(element, &mut actions) } != AX_OK || actions.is_null() {
        return false;
    }
    // SAFETY: AX CopyActionNames returned a non-null +1 array above.
    let actions = unsafe { OwnedCf::from_create_rule(actions) };
    let Some(actions) = actions else {
        return false;
    };
    let Some(actions) = downcast_cf::<CFArray<*const c_void>>(actions) else {
        return false;
    };
    !actions.is_empty()
}

fn accessible_name(element: AXUIElementRef, attributes: &AxAttributes) -> String {
    [&attributes.title, &attributes.description, &attributes.help]
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

/// Restrict AX targets to the visible portion of the focused window on the
/// requested display. `Err(())` means the two regions do not overlap at all.
fn visible_scan_bounds(window: Option<Rect>, requested: Option<Rect>) -> Result<Option<Rect>, ()> {
    match (window, requested) {
        (Some(window), Some(requested)) => window.intersect(&requested).map(Some).ok_or(()),
        (Some(window), None) => Ok(Some(window)),
        (None, requested) => Ok(requested),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

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
    fn scroll_containers_prefer_viewport_aware_ax_collections() {
        assert!(prefers_visible_rows("AXTable"));
        assert!(prefers_visible_rows("AXOutline"));
        assert!(!prefers_visible_rows("AXGroup"));
        assert!(prefers_visible_children("AXScrollArea"));
        assert!(prefers_visible_children("AXList"));
        assert!(!prefers_visible_children("AXGroup"));
        assert_eq!(
            visible_collection_preferences("AXTable", false),
            (false, false)
        );
    }

    #[test]
    fn normalized_rect_deduplicates_subpixel_noise() {
        let first = normalized_rect(Rect::new(10.01, 20.01, 30.01, 40.01));
        let second = normalized_rect(Rect::new(10.02, 20.02, 30.02, 40.02));
        assert_eq!(first, second);
    }

    #[test]
    fn visible_scan_bounds_clip_ax_targets_to_the_focused_window() {
        let window = Rect::new(100.0, 100.0, 800.0, 600.0);
        let display = Rect::new(0.0, 0.0, 600.0, 500.0);
        assert_eq!(
            visible_scan_bounds(Some(window), Some(display)),
            Ok(Some(Rect::new(100.0, 100.0, 500.0, 400.0)))
        );
        assert_eq!(visible_scan_bounds(Some(window), None), Ok(Some(window)));
        assert_eq!(
            visible_scan_bounds(Some(window), Some(Rect::new(0.0, 0.0, 50.0, 50.0))),
            Err(())
        );
    }
}
