//! Worker-owned AX windows, filtered against on-screen Quartz window metadata.
use super::*;
use crate::api::window::{WindowId, WindowInfo};
use crate::api::{Point, Screen};
use crate::platform::common::window_geometry;
use crate::platform::common::window_session::{Snapshot, WindowAccess};
use crate::platform::common::window_visibility::{Visible, visible_candidates};
use core_foundation::base::{CFEqual, CFRetain};
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_graphics::window::{
    copy_window_info, kCGWindowListExcludeDesktopElements, kCGWindowListOptionOnScreenOnly,
};
use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication};
use std::collections::BTreeMap;

const WINDOW_TIMEOUT_SECONDS: c_float = 0.25;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXUIElementGetPid(element: AXUIElementRef, pid: *mut libc::pid_t) -> c_int;
    fn AXUIElementIsAttributeSettable(
        element: AXUIElementRef,
        attribute: CFStringRef,
        settable: *mut bool,
    ) -> c_int;
    fn AXUIElementPerformAction(element: AXUIElementRef, action: CFStringRef) -> c_int;
}

struct Entry {
    window: MovableWindow,
    pid: i32,
    restored: Option<Rect>,
    maximized_frame: Option<Rect>,
    settling_until: Option<Instant>,
    number: Option<isize>,
    app: String,
}

impl crate::platform::macos::window_move::WindowAccess for &MovableWindow {
    fn snapshot(&self) -> Result<crate::platform::macos::window_move::Snapshot, String> {
        crate::platform::macos::window_move::WindowAccess::snapshot(*self)
    }
    fn set_position(&self, point: Point) -> Result<(), WriteError> {
        crate::platform::macos::window_move::WindowAccess::set_position(*self, point)
    }
    fn set_fullscreen(&self, enabled: bool) -> Result<(), WriteError> {
        crate::platform::macos::window_move::WindowAccess::set_fullscreen(*self, enabled)
    }
}

#[derive(Default)]
pub(in crate::platform::macos) struct MacWindows {
    include_minimized: bool,
    #[cfg(test)]
    probe_pid: Option<i32>,
    next: u64,
    entries: BTreeMap<WindowId, Entry>,
    closed: Vec<WindowId>,
    hidden: std::collections::BTreeSet<WindowId>,
    monitor: super::window_tabs::Monitor,
    dragging: std::collections::BTreeSet<WindowId>,
}

fn dictionary(value: &CFType) -> Option<CFDictionary<CFString, CFType>> {
    let dict = value.downcast::<CFDictionary>()?;
    // SAFETY: Quartz metadata dictionaries have CFString keys and CF object
    // values. The dictionary type was checked above; this wrapper retains it.
    Some(unsafe { CFDictionary::wrap_under_get_rule(dict.as_concrete_TypeRef()) })
}

pub(super) fn visible_windows() -> Result<Vec<Visible>, String> {
    let list = copy_window_info(
        kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements,
        0,
    )
    .ok_or("cannot enumerate on-screen windows")?;
    // SAFETY: Quartz returns a CFArray of live CF objects. This wrapper retains
    // the array; each element is type-checked before dictionary access.
    let list = unsafe { CFArray::<CFType>::wrap_under_get_rule(list.as_concrete_TypeRef()) };
    // Quartz field names are constant across every dictionary in this batch.
    let keys = [
        "kCGWindowLayer",
        "kCGWindowOwnerPID",
        "kCGWindowBounds",
        "X",
        "Y",
        "Width",
        "Height",
        "kCGWindowName",
        "kCGWindowNumber",
    ]
    .map(CFString::new);
    let mut result = Vec::new();
    for value in list.iter() {
        let Some(dict) = dictionary(&value) else {
            continue;
        };
        let number = |key: &CFString| {
            dict.find(key)
                .and_then(|v| v.downcast::<CFNumber>())
                .and_then(|v| v.to_i64())
        };
        if number(&keys[0]) != Some(0) {
            continue;
        }
        let Some(pid) = number(&keys[1]) else {
            continue;
        };
        if pid <= 0 || pid as u32 == std::process::id() {
            continue;
        }
        let Some(bounds) = dict.find(&keys[2]).and_then(|v| dictionary(&v)) else {
            continue;
        };
        let coordinate = |key: &CFString| {
            bounds
                .find(key)
                .and_then(|v| v.downcast::<CFNumber>())
                .and_then(|v| v.to_f64())
        };
        let (Some(x), Some(y), Some(width), Some(height)) = (
            coordinate(&keys[3]),
            coordinate(&keys[4]),
            coordinate(&keys[5]),
            coordinate(&keys[6]),
        ) else {
            continue;
        };
        let title = dict
            .find(&keys[7])
            .and_then(|v| v.downcast::<CFString>())
            .map(|s| s.to_string());
        result.push(Visible {
            pid: pid as i32,
            bounds: Rect::new(x, y, width, height),
            title,
            number: number(&keys[8]).and_then(|n| isize::try_from(n).ok()),
        });
    }
    Ok(result)
}

// Only called for AX elements retained by acquire or checked during enumeration.
fn install_window_timeout(window: &OwnedCf) -> Result<(), String> {
    // SAFETY: callers supply a retained AX element; timeout is finite.
    if unsafe { AXUIElementSetMessagingTimeout(window.as_ptr(), WINDOW_TIMEOUT_SECONDS) } == AX_OK {
        Ok(())
    } else {
        Err("cannot configure window messaging timeout".into())
    }
}

fn same_rect(a: Rect, b: Rect) -> bool {
    (a.x - b.x).abs() < 2.0
        && (a.y - b.y).abs() < 2.0
        && (a.width - b.width).abs() < 2.0
        && (a.height - b.height).abs() < 2.0
}

// A missing AX acknowledgement is not a rejected write. Callers confirm by
// bounded native readback before reporting geometry or minimized state.
fn observe_write(result: Result<(), WriteError>) -> Result<(), String> {
    match result {
        Ok(()) | Err(WriteError::Unconfirmed(_)) => Ok(()),
        Err(error) => Err(error.message().to_string()),
    }
}

/// Keep constrained split windows on the target display, including its menu bar.
fn adjusted_position(desired: Rect, actual: Rect, work: &Rect) -> Point {
    let center = desired.center();
    Point::new(
        (center.x - actual.width / 2.0).clamp(work.x, (work.right() - actual.width).max(work.x)),
        (center.y - actual.height / 2.0).clamp(work.y, (work.bottom() - actual.height).max(work.y)),
    )
}

fn wait_confirmation(monitor: &super::window_tabs::Monitor, timeout: Duration) {
    if !monitor.wait(Some(timeout)) {
        // A missing native wake source uses a bounded fallback, never spins.
        std::thread::park_timeout(timeout);
    }
}

// Only the window worker waits. Do not interpret the first stale AX reply as
// an application's minimum size; finish on the requested frame or a stable
// changed frame, with a finite deadline and cancellation on every iteration.
fn wait_frame(
    monitor: &super::window_tabs::Monitor,
    window: &MovableWindow,
    desired: Rect,
    before: Rect,
    cancelled: &dyn Fn() -> bool,
) -> Result<Rect, String> {
    let deadline = Instant::now() + Duration::from_millis(250);
    let mut previous = before;
    let mut stable_since = Instant::now();
    loop {
        let actual = element_rect(window.window.as_ptr(), &window.attributes)
            .ok_or("cannot read adjusted window frame")?;
        if same_rect(actual, desired) || cancelled() || Instant::now() >= deadline {
            return Ok(actual);
        }
        if !same_rect(actual, previous) {
            previous = actual;
            stable_since = Instant::now();
        } else if !same_rect(actual, before) && stable_since.elapsed() >= Duration::from_millis(30)
        {
            return Ok(actual);
        }
        wait_confirmation(monitor, Duration::from_millis(10));
    }
}

impl MacWindows {
    fn retain(
        &mut self,
        window: MovableWindow,
        pid: i32,
        screens: &[Screen],
    ) -> Result<WindowInfo, String> {
        // Acquired windows originate in the short-timeout scanner too.
        install_window_timeout(&window.window)?;
        let existing = self
            .entries
            .iter()
            .find(|(_, entry)| {
                entry.pid == pid
            // SAFETY: both retained references are live CF objects on this worker.
            && unsafe { CFEqual(entry.window.window.as_ptr(), window.window.as_ptr()) } != 0
            })
            .map(|(id, _)| *id);
        let id = existing.unwrap_or_else(|| {
            self.next += 1;
            let id = WindowId(self.next);
            self.entries.insert(
                id,
                Entry {
                    window,
                    pid,
                    restored: None,
                    maximized_frame: None,
                    settling_until: None,
                    number: None,
                    app: NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
                        .and_then(|app| app.localizedName())
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| pid.to_string()),
                },
            );
            id
        });
        self.snapshot(id, screens).map(|s| s.info)
    }
    fn entry(&self, id: WindowId) -> Result<&Entry, String> {
        self.entries
            .get(&id)
            .ok_or_else(|| "window is no longer available".into())
    }
    fn resizable(window: &MovableWindow) -> Result<bool, c_int> {
        let mut value = false;
        // SAFETY: live retained AX reference, typed attribute, writable bool.
        let error = unsafe {
            AXUIElementIsAttributeSettable(
                window.window.as_ptr(),
                window.attributes.size.as_concrete_TypeRef(),
                &mut value,
            )
        };
        if error == AX_OK {
            Ok(value)
        } else {
            Err(error)
        }
    }
    fn prune_closed(&mut self, visible: &[WindowInfo], cancelled: &dyn Fn() -> bool) {
        // A timeout or Space/minimization change does not retire an identity.
        // Only an exited process or AX's explicit invalid-element error does.
        const AX_INVALID_UI_ELEMENT: c_int = -25202;
        let mut closed = Vec::new();
        for (id, entry) in &self.entries {
            if cancelled() {
                break;
            }
            if visible.iter().any(|w| w.id == *id) {
                continue;
            }
            if NSRunningApplication::runningApplicationWithProcessIdentifier(entry.pid)
                .is_none_or(|app| app.isTerminated())
                || Self::resizable(&entry.window) == Err(AX_INVALID_UI_ELEMENT)
            {
                closed.push(*id);
            }
        }
        for id in &closed {
            self.entries.remove(id);
        }
        self.closed.extend(closed);
    }
    fn set_minimized(
        &self,
        id: WindowId,
        minimized: bool,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), String> {
        if cancelled() {
            return Ok(());
        }
        let window = &self.entry(id)?.window;
        let attribute = &window.minimized;
        if copy_bool_attribute(window.window.as_ptr(), attribute) == Some(minimized) {
            return Ok(());
        }
        let value = CFBoolean::from(minimized);
        observe_write(window.set_attribute(attribute, value.as_CFTypeRef()))?;
        let started = Instant::now();
        let deadline = started + Duration::from_millis(1000);
        let mut retried = false;
        while !cancelled() {
            if copy_bool_attribute(window.window.as_ptr(), attribute) == Some(minimized) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "Timed out changing minimized state for window {} to {minimized}",
                    id.0
                ));
            }
            // AppKit can acknowledge but drop a request during a preceding
            // minimize animation. Retry the same idempotent state once.
            if !retried && started.elapsed() >= Duration::from_millis(250) {
                observe_write(window.set_attribute(attribute, value.as_CFTypeRef()))?;
                retried = true;
            }
            wait_confirmation(&self.monitor, Duration::from_millis(10));
        }
        Ok(())
    }
    fn set_size(window: &MovableWindow, width: f64, height: f64) -> Result<(), String> {
        let size = CGSize::new(width, height);
        // SAFETY: the type tag describes the stack CGSize; AX copies it into a
        // newly owned CF object, which stays live throughout set_attribute.
        let value = unsafe {
            OwnedCf::from_create_rule(AXValueCreate(
                AX_VALUE_CGSIZE,
                (&size as *const CGSize).cast(),
            ))
        }
        .ok_or("cannot create window size")?;
        observe_write(window.set_attribute(&window.attributes.size, value.as_ptr()))
    }
}

impl MacWindows {
    fn focused_window_id(&self) -> Option<WindowId> {
        let pid = NSWorkspace::sharedWorkspace()
            .frontmostApplication()?
            .processIdentifier();
        if !self.entries.values().any(|entry| entry.pid == pid) {
            return None;
        }
        let app = AxApplication::with_timeout(pid, WINDOW_TIMEOUT_SECONDS).ok()?;
        let focused = copy_attribute(app.as_ptr(), &CFString::new("AXFocusedWindow"))?;
        self.entries.iter().find_map(|(id, entry)| {
            // SAFETY: both retained AX elements remain live throughout this local comparison.
            (entry.pid == pid
                && unsafe { CFEqual(entry.window.window.as_ptr(), focused.as_ptr()) != 0 })
            .then_some(*id)
        })
    }
}

impl WindowAccess for MacWindows {
    fn focused_window(&self, _windows: &[WindowInfo]) -> Option<WindowId> {
        self.focused_window_id()
    }
    fn native_batch<R>(work: impl FnOnce() -> R) -> R {
        objc2::rc::autoreleasepool(|_| work())
    }

    fn event_waker(&self) -> Option<std::sync::Arc<dyn Fn() + Send + Sync>> {
        let wake = self.monitor.waker();
        crate::platform::macos::window_tabs::set_worker_waker(wake.clone());
        wake
    }
    fn wait_for_events(&self, timeout: Option<Duration>) -> bool {
        self.monitor.wait(timeout)
    }
    fn tab_interacting(&self, id: WindowId) -> bool {
        self.dragging.contains(&id)
    }
    fn tab_geometry(
        &self,
        id: WindowId,
        previous: &Snapshot,
        screens: &[Screen],
    ) -> Result<Snapshot, String> {
        debug_assert_eq!(id, previous.info.id);
        let mut current = previous.clone();
        self.refresh_geometry(&mut current, screens)?;
        Ok(current)
    }
    fn can_submit_frame(&self, id: WindowId) -> bool {
        !self.hidden.contains(&id)
    }
    fn refresh_frame(&self, now: &mut Snapshot, screens: &[Screen]) -> Result<(), String> {
        let entry = self.entry(now.info.id)?;
        let bounds = element_rect(entry.window.window.as_ptr(), &entry.window.attributes)
            .ok_or("window geometry unavailable")?;
        now.info.bounds = bounds;
        now.info.screen =
            window_geometry::screen_index(screens, bounds).ok_or("display unavailable")?;
        now.restored = bounds;
        Ok(())
    }
    fn validate_frame(&self, now: &mut Snapshot, _screens: &[Screen]) -> Result<(), String> {
        let entry = self.entry(now.info.id)?;
        let window = &entry.window;
        now.info.maximized = entry
            .maximized_frame
            .is_some_and(|frame| same_rect(now.info.bounds, frame));
        now.info.minimized = copy_bool_attribute(window.window.as_ptr(), &window.minimized)
            .unwrap_or(now.info.minimized);
        now.info.fullscreen = copy_bool_attribute(window.window.as_ptr(), &window.fullscreen)
            .unwrap_or(now.info.fullscreen);
        now.restored = if now.info.maximized {
            entry.restored.unwrap_or(now.info.bounds)
        } else {
            now.info.bounds
        };
        Ok(())
    }
    fn refresh_geometry(&self, now: &mut Snapshot, screens: &[Screen]) -> Result<(), String> {
        self.refresh_frame(now, screens)?;
        self.validate_frame(now, screens)
    }

    fn set_scope(&mut self, scope: Option<crate::api::window::WindowScope>, _reset: bool) {
        self.include_minimized = scope.is_some_and(|scope| scope.include_minimized);
    }
    fn tab_minimize(
        &mut self,
        id: WindowId,
        _: &[Screen],
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), String> {
        self.set_minimized(id, true, cancelled)
    }
    fn tab_bar_height(&self, _screen: &Screen) -> f64 {
        30.0
    }
    fn tab_set_hidden(&mut self, id: WindowId, hidden: bool) -> Result<(), String> {
        if !hidden && !self.hidden.contains(&id) {
            return Ok(());
        }
        if self.entries.contains_key(&id) {
            // Public AX supports per-window minimization, not arbitrary per-window hiding.
            self.set_minimized(id, hidden, &|| false)?;
        }
        if hidden {
            self.hidden.insert(id);
        } else {
            self.hidden.remove(&id);
        }
        Ok(())
    }
    fn tab_watch(&mut self, ids: &[WindowId]) -> Result<(), String> {
        let windows = ids
            .iter()
            .map(|id| {
                self.entry(*id)
                    .map(|entry| (*id, entry.pid, entry.window.window.as_ptr()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.monitor.watch(&windows)
    }
    fn tab_bar_update(&mut self, bar: &crate::api::window_tabs::TabBar) -> Result<bool, String> {
        crate::platform::macos::window_tabs::publish_one(bar, self.entry(bar.active)?.number)?;
        Ok(true)
    }
    fn tab_bars(&mut self, bars: &[crate::api::window_tabs::TabBar]) -> Result<(), String> {
        crate::platform::macos::window_tabs::publish(
            bars,
            &self
                .entries
                .iter()
                .filter_map(|(id, entry)| entry.number.map(|number| (*id, number)))
                .collect::<Vec<_>>(),
        );
        Ok(())
    }
    fn tab_selected(&self, id: WindowId) -> bool {
        self.focused_window_id() == Some(id)
    }
    fn tab_visible(&self, id: WindowId) -> bool {
        self.entry(id).is_ok_and(|entry| {
            NSRunningApplication::runningApplicationWithProcessIdentifier(entry.pid)
                .is_some_and(|app| !app.isHidden())
        })
    }
    fn tab_events(&mut self) -> Vec<crate::api::window_tabs::TabNativeEvent> {
        self.monitor.pump();
        let mut events = crate::platform::macos::window_tabs::take_events();
        use crate::api::window_tabs::TabNativeEvent;
        if !crate::platform::macos::window_tabs::take_mouse_release()
            && objc2_app_kit::NSEvent::pressedMouseButtons() & 1 != 0
        {
            let moved: Vec<_> = events
                .iter()
                .filter_map(|event| {
                    if let TabNativeEvent::GeometryChanged(id) = event {
                        Some(*id)
                    } else {
                        None
                    }
                })
                .collect();
            for id in moved {
                if self.dragging.insert(id) {
                    events.push(TabNativeEvent::MoveResizeStarted(id));
                }
            }
        } else {
            events.extend(
                std::mem::take(&mut self.dragging)
                    .into_iter()
                    .map(TabNativeEvent::MoveResizeEnded),
            );
        }

        if events.contains(&crate::api::window_tabs::TabNativeEvent::VisibilityChanged)
            && let Some(id) = self.focused_window_id()
        {
            events.push(crate::api::window_tabs::TabNativeEvent::Focused(id));
        }
        events
    }
    fn acquire(&mut self, point: Point, screens: &[Screen]) -> Result<Option<WindowInfo>, String> {
        let Some(window) = window_under_pointer(point)? else {
            return Ok(None);
        };
        let mut pid = 0;
        // SAFETY: window is retained and pid is a writable out-parameter.
        if unsafe { AXUIElementGetPid(window.window.as_ptr(), &mut pid) } != AX_OK
            || pid as u32 == std::process::id()
        {
            return Ok(None);
        }
        if !is_ordinary_ax_window(window.window.as_ptr()) {
            return Ok(None);
        }
        self.retain(window, pid, screens).map(Some)
    }
    fn enumerate(
        &mut self,
        screens: &[Screen],
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Vec<WindowInfo>, String> {
        let visible = visible_windows()?;
        #[cfg(test)]
        let visible: Vec<_> = visible
            .into_iter()
            .filter(|w| self.probe_pid.is_none_or(|pid| w.pid == pid))
            .collect();
        let mut result = Vec::new();
        let mut visited = std::collections::BTreeSet::new();
        let mut processes: Vec<_> = visible.iter().map(|record| record.pid).collect();
        if self.include_minimized {
            processes.extend(
                NSWorkspace::sharedWorkspace()
                    .runningApplications()
                    .iter()
                    .map(|app| app.processIdentifier()),
            );
        }
        for pid in processes {
            if cancelled() {
                break;
            }
            if pid <= 0 || pid as u32 == std::process::id() || !visited.insert(pid) {
                continue;
            }
            #[cfg(test)]
            if self.probe_pid.is_some_and(|expected| expected != pid) {
                continue;
            }
            let Ok(app) = AxApplication::with_timeout(pid, WINDOW_TIMEOUT_SECONDS) else {
                continue;
            };
            let Some(windows) = copy_array_attribute(app.as_ptr(), &CFString::new("AXWindows"))
            else {
                continue;
            };
            let mut candidates = Vec::new();
            for raw in windows.iter() {
                if cancelled() {
                    break;
                }
                if !is_ax_element(*raw) {
                    continue;
                }
                // SAFETY: each array element was checked as an AX object; retain
                // it before releasing the array, then install a finite timeout.
                let Some(owned) = (unsafe { OwnedCf::from_create_rule(CFRetain(*raw)) }) else {
                    continue;
                };
                if install_window_timeout(&owned).is_err() {
                    continue;
                }
                if !is_ordinary_ax_window(owned.as_ptr()) {
                    continue;
                }
                let window = MovableWindow {
                    window: owned,
                    attributes: AxAttributes::new(),
                    fullscreen: CFString::new("AXFullScreen"),
                    minimized: CFString::new("AXMinimized"),
                };
                if copy_bool_attribute(window.window.as_ptr(), &window.minimized) == Some(true) {
                    if self.include_minimized
                        && let Ok(info) = self.retain(window, pid, screens)
                    {
                        result.push((usize::MAX, info));
                    }
                    continue;
                }
                let Some(bounds) = element_rect(window.window.as_ptr(), &window.attributes) else {
                    continue;
                };
                let title = copy_string_attribute(window.window.as_ptr(), &window.attributes.title)
                    .unwrap_or_default();
                candidates.push((window, bounds, title));
            }
            // Quartz can omit titles without Screen Recording permission. A
            // complete coincident cohort is still safe to enumerate, although
            // its individual Quartz window numbers cannot be identified.
            // AX can report a completed move before Quartz publishes it.
            // Retry unmatched candidates against one fresh snapshot, rather
            // than dropping a just-moved/restored window from Editor or Tabs.
            let mut refreshed;
            let mut current = &visible;
            let settling_until = self
                .entries
                .values()
                .filter(|e| e.pid == pid)
                .filter_map(|e| e.settling_until)
                .max();
            let mut pass = 0;
            loop {
                for (rank, shown) in current.iter().enumerate().filter(|(_, v)| v.pid == pid) {
                    if cancelled() {
                        break;
                    }
                    let metadata: Vec<_> = candidates
                        .iter()
                        .map(|(_, bounds, title)| (*bounds, title.as_str()))
                        .collect();
                    let matches = visible_candidates(&metadata, shown, &current[rank..]);
                    let number = (matches.len() == 1).then_some(shown.number).flatten();
                    for index in matches.into_iter().rev() {
                        let (window, _, _) = candidates.remove(index);
                        if let Ok(info) = self.retain(window, pid, screens) {
                            if let Some(entry) = self.entries.get_mut(&info.id) {
                                entry.number = number;
                            }
                            result.push((rank, info));
                        }
                    }
                }
                if candidates.is_empty() || cancelled() {
                    break;
                }
                if pass > 0 {
                    if settling_until.is_none_or(|until| Instant::now() >= until) {
                        break;
                    }
                    wait_confirmation(&self.monitor, Duration::from_millis(10));
                }
                pass += 1;
                refreshed = visible_windows()?;
                current = &refreshed;
            }
        }
        result.sort_by_key(|(rank, _)| *rank);
        let mut result: Vec<_> = result.into_iter().map(|(_, info)| info).collect();
        for id in &self.hidden {
            if !result.iter().any(|w| w.id == *id)
                && let Ok(snapshot) = self.snapshot(*id, screens)
            {
                result.push(snapshot.info);
            }
        }
        if !cancelled() {
            self.prune_closed(&result, cancelled);
        }
        Ok(result)
    }
    fn snapshot(&self, id: WindowId, screens: &[Screen]) -> Result<Snapshot, String> {
        let entry = self.entry(id)?;
        let window = &entry.window;
        let bounds = element_rect(window.window.as_ptr(), &window.attributes)
            .ok_or("window was closed or its bounds are unavailable")?;
        let screen = window_geometry::screen_index(screens, bounds).ok_or("display unavailable")?;
        let resizable = Self::resizable(window).unwrap_or(false);
        let maximized = entry
            .maximized_frame
            .is_some_and(|frame| same_rect(bounds, frame));
        Ok(Snapshot {
            info: WindowInfo {
                id,
                app: entry.app.clone(),
                title: copy_string_attribute(window.window.as_ptr(), &window.attributes.title)
                    .unwrap_or_default(),
                bounds,
                screen,
                resizable,
                maximized,
                minimized: copy_bool_attribute(
                    window.window.as_ptr(),
                    &CFString::new("AXMinimized"),
                )
                .unwrap_or(false),
                fullscreen: copy_bool_attribute(window.window.as_ptr(), &window.fullscreen)
                    .unwrap_or(false),
            },
            restored: if maximized {
                entry.restored.unwrap_or(bounds)
            } else {
                bounds
            },
        })
    }
    fn submit_maximize(
        &mut self,
        before: &Snapshot,
        maximize: bool,
        screens: &[Screen],
    ) -> Result<bool, String> {
        if before.info.fullscreen || self.hidden.contains(&before.info.id) {
            return Ok(false);
        }
        let id = before.info.id;
        if before.info.minimized {
            let window = &self.entry(id)?.window;
            observe_write(
                window.set_attribute(&window.minimized, CFBoolean::false_value().as_CFTypeRef()),
            )?;
        }
        if maximize {
            let desired = screens
                .get(before.info.screen)
                .ok_or("display unavailable")?
                .work_area;
            let mut normal = before.placement();
            normal.info.maximized = false;
            normal.info.minimized = false;
            self.submit_frame(&normal, desired, screens)?;
            let entry = self.entries.get_mut(&id).ok_or("window was closed")?;
            entry.restored = Some(before.restored);
            entry.maximized_frame = Some(desired);
        } else {
            let entry = self.entries.get_mut(&id).ok_or("window was closed")?;
            entry.maximized_frame = None;
        }
        Ok(true)
    }
    fn accept_maximized_frame(&mut self, observed: &mut Snapshot, _screens: &[Screen]) {
        if observed.info.minimized || observed.info.fullscreen {
            return;
        }
        if let Some(entry) = self.entries.get_mut(&observed.info.id)
            && entry.maximized_frame.is_some()
        {
            entry.maximized_frame = Some(observed.info.bounds);
            observed.info.maximized = true;
            observed.restored = entry.restored.unwrap_or(observed.info.bounds);
        }
    }
    fn confirmed_placement(
        &mut self,
        observed: &Snapshot,
        _screens: &[Screen],
    ) -> Result<(), String> {
        if observed.info.maximized {
            if let Some(entry) = self.entries.get_mut(&observed.info.id) {
                entry.maximized_frame = Some(observed.info.bounds);
                entry.settling_until = Some(Instant::now() + Duration::from_millis(250));
            }
        } else {
            self.confirmed_frame(observed.info.id);
        }
        Ok(())
    }
    fn submit_maximized_frame(
        &mut self,
        before: &Snapshot,
        desired: Rect,
        screens: &[Screen],
    ) -> Result<bool, String> {
        let mut normal = before.placement();
        normal.info.maximized = false;
        let accepted = self.submit_frame(&normal, desired, screens)?;
        if accepted {
            self.entries
                .get_mut(&before.info.id)
                .ok_or("window was closed")?
                .maximized_frame = Some(desired);
        }
        Ok(accepted)
    }
    fn submit_frame(
        &mut self,
        before: &Snapshot,
        desired: Rect,
        _screens: &[Screen],
    ) -> Result<bool, String> {
        use crate::platform::macos::window_move::WindowAccess as _;
        if ![desired.x, desired.y, desired.width, desired.height]
            .iter()
            .all(|v| v.is_finite())
            || desired.width < 1.0
            || desired.height < 1.0
        {
            return Err("invalid window geometry".into());
        }
        let id = before.info.id;
        if before.info.maximized
            || before.info.minimized
            || before.info.fullscreen
            || self.hidden.contains(&id)
        {
            return Ok(false);
        }
        let window = &self.entry(id)?.window;
        if (desired.x - before.info.bounds.x).abs() > 0.5
            || (desired.y - before.info.bounds.y).abs() > 0.5
        {
            observe_write(window.set_position(Point::new(desired.x, desired.y)))?;
        }
        if (desired.width - before.info.bounds.width).abs() > 0.5
            || (desired.height - before.info.bounds.height).abs() > 0.5
        {
            Self::set_size(window, desired.width, desired.height)?;
        }
        Ok(true)
    }
    fn confirmed_frame(&mut self, id: WindowId) {
        if let Some(entry) = self.entries.get_mut(&id) {
            entry.restored = None;
            entry.maximized_frame = None;
            entry.settling_until = Some(Instant::now() + Duration::from_millis(250));
        }
    }
    fn set_frame(
        &mut self,
        id: WindowId,
        desired: Rect,
        screens: &[Screen],
        cancelled: &dyn Fn() -> bool,
    ) -> Result<WindowInfo, String> {
        use crate::platform::macos::window_move::WindowAccess as _;
        if ![desired.x, desired.y, desired.width, desired.height]
            .iter()
            .all(|v| v.is_finite())
            || desired.width < 1.0
            || desired.height < 1.0
        {
            return Err("invalid window geometry".into());
        }
        let before = self.snapshot(id, screens)?;
        if cancelled() {
            return Ok(before.info);
        }
        if before.info.fullscreen {
            return Err("Exit native fullscreen before adjusting this window".into());
        }
        if before.info.minimized && !self.hidden.contains(&id) {
            self.set_minimized(id, false, cancelled)?;
        }
        if cancelled() {
            return self.snapshot(id, screens).map(|s| s.info);
        }
        let window = &self.entry(id)?.window;
        let resizing = (desired.width - before.info.bounds.width).abs() > 0.5
            || (desired.height - before.info.bounds.height).abs() > 0.5;
        // AX providers can constrain size to the space available at the old
        // origin. Move first, resize, then correct the accepted frame. A second
        // size write handles providers which apply position asynchronously.
        if (desired.x - before.info.bounds.x).abs() > 0.5
            || (desired.y - before.info.bounds.y).abs() > 0.5
        {
            observe_write(window.set_position(Point::new(desired.x, desired.y)))?;
        }
        if resizing && !cancelled() {
            Self::set_size(window, desired.width, desired.height)?;
        }
        let mut actual = wait_frame(
            &self.monitor,
            window,
            desired,
            before.info.bounds,
            cancelled,
        )?;
        if resizing && !cancelled() && !same_rect(actual, desired) {
            Self::set_size(window, desired.width, desired.height)?;
            actual = wait_frame(&self.monitor, window, desired, actual, cancelled)?;
        }
        if !cancelled() {
            let position = if resizing {
                adjusted_position(
                    desired,
                    actual,
                    &screens[window_geometry::screen_index(screens, desired)
                        .ok_or("display unavailable")?]
                    .work_area,
                )
            } else {
                Point::new(desired.x, desired.y)
            };
            if (position.x - actual.x).abs() > 0.5 || (position.y - actual.y).abs() > 0.5 {
                observe_write(window.set_position(position))?;
                wait_frame(
                    &self.monitor,
                    window,
                    Rect::new(position.x, position.y, actual.width, actual.height),
                    actual,
                    cancelled,
                )?;
            }
        }
        let accepted = self.snapshot(id, screens)?;
        #[cfg(test)]
        if self.probe_pid.is_some() && !same_rect(accepted.info.bounds, desired) {
            println!(
                "Frame {id:?}: requested {desired:?}, accepted {:?}",
                accepted.info.bounds
            );
        }
        if !cancelled() || !same_rect(accepted.info.bounds, before.info.bounds) {
            let entry = self.entries.get_mut(&id).ok_or("window was closed")?;
            entry.restored = None;
            entry.maximized_frame = None;
            entry.settling_until = Some(Instant::now() + Duration::from_millis(250));
        }
        self.snapshot(id, screens).map(|s| s.info)
    }
    fn tab_fit_frame(
        &mut self,
        id: WindowId,
        bounds: Rect,
        screens: &[Screen],
        cancelled: &dyn Fn() -> bool,
    ) -> Result<WindowInfo, String> {
        let before = self.snapshot(id, screens)?;
        self.set_frame(id, bounds, screens, cancelled)?;
        if before.info.maximized && !cancelled() {
            let accepted = self.snapshot(id, screens)?.info.bounds;
            let entry = self.entries.get_mut(&id).ok_or("window was closed")?;
            entry.restored = Some(before.restored);
            entry.maximized_frame = Some(accepted);
        }
        self.snapshot(id, screens).map(|s| s.info)
    }

    fn restore(
        &mut self,
        snapshot: &Snapshot,
        screens: &[Screen],
        cancelled: &dyn Fn() -> bool,
    ) -> Result<WindowInfo, String> {
        if snapshot.info.fullscreen {
            let index = window_geometry::screen_index(screens, snapshot.info.bounds)
                .ok_or("display unavailable")?;
            return self
                .move_fullscreen(
                    snapshot.info.id,
                    crate::api::command::WindowScreenTarget::Index(index),
                    screens,
                    cancelled,
                )
                .map(|(window, _)| window);
        }
        self.set_frame(snapshot.info.id, snapshot.info.bounds, screens, cancelled)?;
        if !cancelled() {
            let accepted = self.snapshot(snapshot.info.id, screens)?.info.bounds;
            let entry = self
                .entries
                .get_mut(&snapshot.info.id)
                .ok_or("window was closed")?;
            entry.restored = snapshot.info.maximized.then_some(snapshot.restored);
            entry.maximized_frame = snapshot.info.maximized.then_some(accepted);
        }
        self.set_minimized(snapshot.info.id, snapshot.info.minimized, cancelled)?;
        self.snapshot(snapshot.info.id, screens).map(|s| s.info)
    }
    fn cycle_state(
        &mut self,
        id: WindowId,
        screens: &[Screen],
        cancelled: &dyn Fn() -> bool,
    ) -> Result<WindowInfo, String> {
        let before = self.snapshot(id, screens)?;
        if cancelled() {
            return Ok(before.info);
        }
        if before.info.minimized {
            self.set_minimized(id, false, cancelled)?;
            let after = self.set_frame(id, before.restored, screens, cancelled)?;
            self.entries
                .get_mut(&id)
                .ok_or("window was closed")?
                .restored = None;
            return Ok(WindowInfo {
                maximized: false,
                ..after
            });
        }
        if before.info.maximized {
            self.set_minimized(id, true, cancelled)?;
            return self.snapshot(id, screens).map(|s| s.info);
        }
        let desired = screens
            .get(before.info.screen)
            .ok_or("display unavailable")?
            .work_area;
        let after = self.set_frame(id, desired, screens, cancelled)?;
        if !cancelled()
            && (same_rect(after.bounds, desired) || !same_rect(after.bounds, before.info.bounds))
        {
            let entry = self.entries.get_mut(&id).ok_or("window was closed")?;
            entry.restored = Some(before.info.bounds);
            entry.maximized_frame = Some(after.bounds);
        }
        self.snapshot(id, screens).map(|s| s.info)
    }

    fn audio_factory(&self) -> Option<crate::platform::common::audio_worker::AudioFactory> {
        Some(crate::platform::macos::window_audio::create_backend)
    }
    fn audio_process(
        &self,
        target: crate::api::audio::AudioTarget,
    ) -> Result<Option<crate::platform::common::audio_worker::AudioProcess>, String> {
        let crate::api::audio::AudioTarget::Application(id) = target else {
            return Ok(None);
        };
        let entry = self.entry(id)?;
        crate::platform::macos::window_move::WindowAccess::snapshot(&entry.window)?;
        crate::platform::macos::window_audio::process(entry.pid).map(Some)
    }
    fn select(&self, id: WindowId) -> Result<(), String> {
        let entry = self.entry(id)?;
        let app = NSRunningApplication::runningApplicationWithProcessIdentifier(entry.pid)
            .ok_or("application was closed")?;
        #[allow(deprecated)]
        if !app.activateWithOptions(NSApplicationActivationOptions::ActivateIgnoringOtherApps) {
            return Err("application activation was refused".into());
        }
        // SAFETY: the retained AX window and action string remain live for the
        // bounded AX action; no Rust pointers are retained by the provider.
        let error = unsafe {
            AXUIElementPerformAction(
                entry.window.window.as_ptr(),
                CFString::new("AXRaise").as_concrete_TypeRef(),
            )
        };
        if error != AX_OK {
            // Some AX providers do not expose Raise but do expose Main/Focused.
            let value = CFBoolean::true_value();
            for attribute in ["AXMain", "AXFocused"] {
                let _ = entry
                    .window
                    .set_attribute(&CFString::new(attribute), value.as_CFTypeRef());
            }
        }
        let deadline = Instant::now() + Duration::from_millis(250);
        loop {
            if self.focused_window_id() == Some(id) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                break;
            }
            wait_confirmation(&self.monitor, Duration::from_millis(10));
        }
        if error == AX_OK {
            Ok(())
        } else {
            Err(format!("cannot activate window: AXError {error}"))
        }
    }
    fn pointer(&self) -> Result<Point, String> {
        crate::platform::macos::input::cursor_position()
    }
    fn close(&self, id: WindowId) -> Result<(), String> {
        let entry = self.entry(id)?;
        let button = copy_attribute(
            entry.window.window.as_ptr(),
            &CFString::new("AXCloseButton"),
        )
        .ok_or("Window has no accessible close button")?;
        if !is_ax_element(button.as_ptr()) {
            return Err("Window close button is not an accessibility element".into());
        }
        // SAFETY: both retained AX objects remain live during the bounded action.
        let error = unsafe {
            let timeout = AXUIElementSetMessagingTimeout(button.as_ptr(), WINDOW_TIMEOUT_SECONDS);
            if timeout != AX_OK {
                timeout
            } else {
                AXUIElementPerformAction(
                    button.as_ptr(),
                    CFString::new("AXPress").as_concrete_TypeRef(),
                )
            }
        };
        if error != AX_OK {
            return Err(format!("Cannot request window close: AXError {error}"));
        }
        Ok(())
    }
    fn move_fullscreen(
        &mut self,
        id: WindowId,
        destination: crate::api::command::WindowScreenTarget,
        screens: &[Screen],
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(WindowInfo, Option<Point>), String> {
        let cursor = self.pointer()?;
        let (movement, pointer) = crate::platform::macos::window_move::WindowMove::start(
            &self.entry(id)?.window,
            cursor,
            screens,
            destination,
            Instant::now(),
        )?;
        let Some(mut movement) = movement else {
            return self.snapshot(id, screens).map(|s| (s.info, pointer));
        };
        loop {
            if cancelled() {
                movement.cancel()?;
                return Err("fullscreen move cancelled".into());
            }
            if let Some(result) = movement.poll(screens, Instant::now()) {
                let pointer = result?;
                return self.snapshot(id, screens).map(|s| (s.info, Some(pointer)));
            }
            wait_confirmation(
                &self.monitor,
                crate::platform::macos::window_move::POLL_INTERVAL,
            );
        }
    }
    fn reset(&mut self) {
        for id in self.hidden.iter().copied().collect::<Vec<_>>() {
            if let Err(error) = self.tab_set_hidden(id, false) {
                crate::report_error!("window-tabs", "restore hidden window: {error}");
                return;
            }
        }
        crate::platform::macos::window_tabs::publish(&[], &[]);
        self.monitor.clear();
        self.dragging.clear();
        self.entries.clear();
        self.closed.clear();
    }
    fn take_closed(&mut self) -> Vec<WindowId> {
        std::mem::take(&mut self.closed)
    }
}

impl Drop for MacWindows {
    fn drop(&mut self) {
        self.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constrained_splits_stay_inside_the_target_work_area() {
        let work = Rect::new(-1440.0, 25.0, 1440.0, 875.0);
        let left = Rect::new(-1440.0, 25.0, 360.0, 875.0);
        let accepted = Rect::new(-1440.0, 25.0, 500.0, 875.0);
        assert_eq!(
            adjusted_position(left, accepted, &work),
            Point::new(-1440.0, 25.0)
        );
        let right = Rect::new(-360.0, 25.0, 360.0, 875.0);
        assert_eq!(
            adjusted_position(right, accepted, &work),
            Point::new(-500.0, 25.0)
        );
        let oversized = Rect::new(0.0, 0.0, 2000.0, 1200.0);
        assert_eq!(
            adjusted_position(left, oversized, &work),
            Point::new(-1440.0, 25.0)
        );
    }

    #[test]
    #[ignore = "Creates disposable AppKit windows; requires Accessibility permission and clang"]
    fn native_macos_window_parity() -> Result<(), String> {
        native_probe(false)
    }

    #[test]
    #[ignore = "Native AX benchmark; use tools/test-macos-windows.py --performance --release"]
    fn native_macos_tabs_geometry_performance() -> Result<(), String> {
        native_probe(true)
    }

    fn native_probe(performance: bool) -> Result<(), String> {
        use crate::api::window_tabs::{TabOperation, WindowTarget};
        use crate::platform::common::WindowGroupsProbe as Grouped;
        let permissions = crate::platform::macos::permissions::is_trusted;
        if !permissions()
            && std::env::var("KEYSTEER_PROBE_REQUEST_ACCESSIBILITY").as_deref() == Ok("1")
        {
            crate::platform::macos::permissions::prompt_for_trust();
            // The system prompt is asynchronous. Granting access continues this
            // same test process; do not rebuild while the user is authorizing it.
            let deadline = Instant::now() + Duration::from_secs(120);
            while !permissions() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(250));
            }
        }
        if !permissions() {
            return Err(
                "Native parity probe requires Accessibility permission for the test host. Run with KEYSTEER_PROBE_REQUEST_ACCESSIBILITY=1 to request access and wait up to 120 seconds".into(),
            );
        }
        struct Child(std::process::Child);
        impl Drop for Child {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        struct Directory(std::path::PathBuf);
        impl Drop for Directory {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let path = std::env::temp_dir().join(format!(
            "keysteer-parity-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|e| e.to_string())?
                .as_nanos()
        ));
        std::fs::create_dir(&path).map_err(|e| e.to_string())?;
        let temp = Directory(path);
        let binary = temp.0.join("window-parity");
        let built = std::process::Command::new("/usr/bin/clang")
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/macos-window-parity.m"
            ))
            .args(["-fobjc-arc", "-framework", "AppKit", "-o"])
            .arg(&binary)
            .output()
            .map_err(|e| e.to_string())?;
        if !built.status.success() {
            return Err(String::from_utf8_lossy(&built.stderr).into_owned());
        }
        let mut child = Child(
            std::process::Command::new(binary)
                .stdout(std::process::Stdio::piped())
                .stdin(std::process::Stdio::piped())
                .spawn()
                .map_err(|e| e.to_string())?,
        );
        // Rust's test runs off the main thread; use the helper's AppKit work
        // areas, just as the production main-thread backend does (including Dock/menu).
        use std::io::BufRead;
        let mut line = String::new();
        std::io::BufReader::new(child.0.stdout.take().ok_or("missing helper stdout")?)
            .read_line(&mut line)
            .map_err(|e| e.to_string())?;
        let screens: Vec<_> = line
            .trim()
            .split(';')
            .filter(|s| !s.is_empty())
            .enumerate()
            .map(|(index, screen)| {
                let values: Vec<f64> = screen
                    .split(',')
                    .map(|v| v.parse().expect("screen coordinate"))
                    .collect();
                assert_eq!(values.len(), 9);
                Screen {
                    bounds: Rect::new(values[0], values[1], values[2], values[3]),
                    work_area: Rect::new(values[4], values[5], values[6], values[7]),
                    scale: values[8],
                    is_primary: index == 0,
                    name: None,
                }
            })
            .collect();
        assert!(!screens.is_empty());
        let mut native = MacWindows::default();
        let inventory = native.enumerate(&screens, &|| false)?;
        let mut counts = BTreeMap::<String, usize>::new();
        for window in &inventory {
            *counts.entry(window.app.clone()).or_default() += 1;
        }
        println!("Read-only desktop inventory: {counts:?}");
        native.probe_pid = Some(child.0.id() as i32);
        let deadline = Instant::now() + Duration::from_secs(5);
        let windows = loop {
            let windows: Vec<_> = native
                .enumerate(&screens, &|| false)?
                .into_iter()
                .filter(|w| {
                    native
                        .entries
                        .get(&w.id)
                        .is_some_and(|e| e.pid == child.0.id() as i32)
                })
                .collect();
            if windows.len() == 3 {
                break windows;
            }
            if Instant::now() >= deadline {
                return Err("Disposable AX windows unavailable; grant Accessibility permission to the test host".into());
            }
            std::thread::sleep(Duration::from_millis(50));
        };

        if performance {
            use crate::api::window_tabs::TabNativeEvent;
            use std::io::Write;
            let id = windows.iter().find(|w| w.title.ends_with(" 0")).unwrap().id;
            native.tab_watch(&windows.iter().map(|w| w.id).collect::<Vec<_>>())?;
            let percentile = |values: &mut Vec<u128>, fraction: usize| {
                values.sort_unstable();
                values[(values.len() - 1) * fraction / 100]
            };
            let mut rows = vec!["path,samples,p50_us,p95_us,p99_us".to_string()];
            let mut delivery = Vec::with_capacity(2000);
            for _ in 0..2000 {
                native.tab_events();
                let start = Instant::now();
                child
                    .0
                    .stdin
                    .as_mut()
                    .ok_or("missing helper stdin")?
                    .write_all(b"m")
                    .map_err(|e| e.to_string())?;
                loop {
                    // Deadline detects a broken fixture, never paces movement.
                    native.wait_for_events(Some(Duration::from_secs(2)));
                    if native
                        .tab_events()
                        .contains(&TabNativeEvent::GeometryChanged(id))
                    {
                        break;
                    }
                    assert!(
                        start.elapsed() < Duration::from_secs(2),
                        "missing native movement notification"
                    );
                }
                delivery.push(start.elapsed().as_micros());
            }
            rows.push(format!(
                "event_driven_delivery,2000,{},{},{}",
                percentile(&mut delivery, 50),
                percentile(&mut delivery, 95),
                percentile(&mut delivery, 99)
            ));
            let previous = native.snapshot(id, &screens)?;
            let mut full = Vec::with_capacity(20000);
            let mut geometry = Vec::with_capacity(20000);
            for step in 0..20200 {
                let start = Instant::now();
                let baseline = native.snapshot(id, &screens)?;
                let elapsed = start.elapsed().as_micros();
                let start = Instant::now();
                let actual = native.tab_geometry(id, &previous, &screens)?;
                let fast_elapsed = start.elapsed().as_micros();
                assert_eq!(baseline.info, actual.info);
                assert_eq!(baseline.restored, actual.restored);
                if step >= 200 {
                    full.push(elapsed);
                    geometry.push(fast_elapsed);
                }
            }
            for (name, values) in [
                ("full_snapshot", &mut full),
                ("geometry_only", &mut geometry),
            ] {
                rows.push(format!(
                    "{name},{},{},{},{}",
                    values.len(),
                    percentile(values, 50),
                    percentile(values, 95),
                    percentile(values, 99)
                ));
            }
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("target/native-window-tests/tabs-performance.csv");
            std::fs::write(&path, rows.join("\n")).map_err(|e| e.to_string())?;
            println!("{}\n{}", path.display(), rows.join("\n"));
            return Ok(());
        }
        let id = windows[0].id;
        let original = native.snapshot(id, &screens)?;
        let work = screens[original.info.screen].work_area;
        let split = Rect::new(work.x, work.y + 50.0, work.width / 2.0, work.height - 100.0);
        let tiled = native.set_frame(id, split, &screens, &|| false)?;
        assert!(
            same_rect(tiled.bounds, split),
            "accepted {:?}, expected {:?}",
            tiled.bounds,
            split
        );
        native.restore(&original, &screens, &|| false)?;
        println!("Probe: standalone F cycle");
        for _ in 0..2 {
            assert!(native.cycle_state(id, &screens, &|| false)?.maximized);
            assert!(native.cycle_state(id, &screens, &|| false)?.minimized);
            let restored = native.cycle_state(id, &screens, &|| false)?;
            assert!(!restored.maximized && !restored.minimized);
            assert!(same_rect(restored.bounds, original.info.bounds));
        }
        use crate::api::window::{WindowChange, WindowEditResult, WindowOperation};
        use crate::api::window_layout::{LayoutTree, placed_rect};
        use crate::platform::common::window_session::WindowSessionProbe;
        println!("Probe: move, center and Editor");
        let mut session = WindowSessionProbe::new(id);
        for change in [
            WindowChange::Move { dx: 45.0, dy: 30.0 },
            WindowChange::Center,
        ] {
            let result = session.execute(
                &mut native,
                WindowOperation::Adjust {
                    target: id,
                    change,
                    group: 1,
                },
                &screens,
            );
            assert_eq!(result.changed, 1, "{result:?}");
        }
        let centered = native.snapshot(id, &screens)?.info.bounds;
        assert!((centered.x + centered.width / 2.0 - work.x - work.width / 2.0).abs() < 2.0);
        assert!((centered.y + centered.height / 2.0 - work.y - work.height / 2.0).abs() < 2.0);
        let begun = session.execute(
            &mut native,
            WindowOperation::BeginEdit {
                transaction: 2,
                targets: vec![],
                screen: Some(original.info.screen),
                group: 2,
            },
            &screens,
        );
        let Some(WindowEditResult::Started {
            minimums,
            full_inventory: true,
            ..
        }) = begun.edit.as_deref()
        else {
            panic!("{begun:?}")
        };
        if minimums.len() != 3 {
            println!("Editor result: {begun:?}");
            for record in visible_windows()?
                .into_iter()
                .filter(|w| w.pid == child.0.id() as i32)
            {
                println!("Quartz: {:?} {:?}", record.bounds, record.title);
            }
            for member in &windows {
                println!("AX: {:?}", native.snapshot(member.id, &screens));
            }
        }
        assert_eq!(
            minimums.len(),
            3,
            "Editor must capture all disposable windows"
        );
        let tree = LayoutTree::automatic(
            begun.windows.as_ref().unwrap(),
            Some(id),
            work,
            &minimums.iter().copied().collect(),
            0.0,
        )?;
        let placements: Vec<_> = tree
            .slots()
            .into_iter()
            .filter_map(|s| s.window.map(|id| (id, s.rect)))
            .collect();
        let applied = session.execute(
            &mut native,
            WindowOperation::ApplyLayout {
                additional_screens: vec![],
                transaction: 2,
                revision: 1,
                screen: original.info.screen,
                placements: placements.clone(),
                gap: 0.0,
                strict: true,
            },
            &screens,
        );
        assert!(
            matches!(
                applied.edit.as_deref(),
                Some(WindowEditResult::Applied { accepted: true, .. })
            ),
            "{applied:?}"
        );
        for (member, rect) in placements {
            assert!(same_rect(
                native.snapshot(member, &screens)?.info.bounds,
                placed_rect(work, rect, 0.0)
            ));
        }
        session.execute(
            &mut native,
            WindowOperation::EndEdit {
                transaction: 2,
                commit: true,
            },
            &screens,
        );
        // Quick's normalized left/right halves must be real native geometry.
        for (index, member) in windows.iter().take(2).enumerate() {
            let half = Rect::new(
                work.x + index as f64 * work.width / 2.0,
                work.y,
                work.width / 2.0,
                work.height,
            );
            assert!(same_rect(
                native
                    .set_frame(member.id, half, &screens, &|| false)?
                    .bounds,
                half
            ));
        }
        let cycled = session.execute(&mut native, WindowOperation::Cycle, &screens);
        assert!(
            cycled.target.as_ref().is_some_and(|target| target.id != id),
            "{cycled:?}"
        );
        // A silent app accepts and retains volume/mute/output preferences.
        use crate::api::audio::AudioAction;
        use crate::platform::macos::window_audio::{AudioController, process};
        let mut audio = AudioController::default();
        let process = Some(process(child.0.id() as i32)?);
        assert!(audio.change(process, AudioAction::Down)?.contains("99%"));
        assert!(audio.maintain());
        assert!(
            audio
                .change(process, AudioAction::ToggleMute)?
                .contains("muted")
        );
        audio.change(process, AudioAction::ToggleMute)?;
        assert!(audio.change(process, AudioAction::Up)?.contains("100%"));
        assert!(!audio.maintain());
        audio.change(process, AudioAction::DeviceNext)?;
        audio.change(process, AudioAction::DevicePrevious)?;
        drop(audio);
        println!("Probe: automatic application tabs");
        let mut grouped = Grouped::new(native);
        grouped.tab_operation(
            TabOperation::Enter {
                screen: original.info.screen,
            },
            &screens,
            &|| false,
        )?;
        grouped.tab_operation(
            TabOperation::Choose(WindowTarget::Window(id)),
            &screens,
            &|| false,
        )?;
        assert_eq!(
            grouped.tab_state().ok_or("missing tab state")?.groups[0]
                .members
                .len(),
            3
        );
        assert!(grouped.cycle_state(id, &screens, &|| false)?.maximized);
        assert!(grouped.cycle_state(id, &screens, &|| false)?.minimized);
        assert!(!grouped.cycle_state(id, &screens, &|| false)?.minimized);
        grouped.tab_operation(TabOperation::Cycle { backwards: false }, &screens, &|| {
            false
        })?;
        grouped.tab_operation(TabOperation::Dissolve, &screens, &|| false)?;
        assert!(
            grouped
                .tab_state()
                .ok_or("missing tab state")?
                .groups
                .is_empty()
        );
        grouped.close(id)?;
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let remaining = grouped.enumerate(&screens, &|| false)?;
            if remaining.len() == 2 && remaining.iter().all(|w| w.id != id) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "Close did not retire the disposable window"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        println!(
            "Native parity: enumerate overlapping windows, move, center, F, left/right split, Editor autotile, Tab cycle, auto app groups, close and silent-app audio passed"
        );
        Ok(())
    }
}
