//! Worker-owned AX windows, filtered against on-screen Quartz window metadata.
use super::*;
use crate::api::window::{WindowId, WindowInfo};
use crate::api::{Point, Screen};
use crate::platform::common::window_geometry;
use crate::platform::common::window_session::{Snapshot, WindowAccess};
use core_foundation::base::{CFEqual, CFRetain};
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_graphics::window::{
    copy_window_info, kCGWindowListExcludeDesktopElements, kCGWindowListOptionOnScreenOnly,
};
use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication};
use std::collections::BTreeMap;

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
    next: u64,
    entries: BTreeMap<WindowId, Entry>,
    closed: Vec<WindowId>,
    hidden: std::collections::BTreeSet<WindowId>,
    monitor: super::window_tabs::Monitor,
}

pub(super) struct Visible {
    pub(super) pid: i32,
    pub(super) bounds: Rect,
    title: Option<String>,
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
        });
        if result.len() == 256 {
            break;
        }
    }
    Ok(result)
}

fn same_rect(a: Rect, b: Rect) -> bool {
    (a.x - b.x).abs() < 2.0
        && (a.y - b.y).abs() < 2.0
        && (a.width - b.width).abs() < 2.0
        && (a.height - b.height).abs() < 2.0
}

impl MacWindows {
    fn retain(
        &mut self,
        window: MovableWindow,
        pid: i32,
        screens: &[Screen],
    ) -> Result<WindowInfo, String> {
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
        let attribute = CFString::new("AXMinimized");
        let value = CFBoolean::from(minimized);
        window
            .set_attribute(&attribute, value.as_CFTypeRef())
            .map_err(|e| e.message().to_string())?;
        let deadline = Instant::now() + Duration::from_millis(250);
        while !cancelled() {
            if copy_bool_attribute(window.window.as_ptr(), &attribute) == Some(minimized) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err("Timed out changing minimized state".into());
            }
            std::thread::sleep(Duration::from_millis(10));
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
        window
            .set_attribute(&window.attributes.size, value.as_ptr())
            .map_err(|e| e.message().to_string())
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
        let app = AxApplication::new(pid).ok()?;
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
        crate::platform::macos::window_tabs::publish_one(bar)?;
        Ok(true)
    }
    fn tab_bars(&mut self, bars: &[crate::api::window_tabs::TabBar]) -> Result<(), String> {
        crate::platform::macos::window_tabs::publish(bars);
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
        if copy_string_attribute(window.window.as_ptr(), &CFString::new("AXSubrole")).as_deref()
            != Some("AXStandardWindow")
        {
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
            let Ok(app) = AxApplication::new(pid) else {
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
                // SAFETY: owned is a live AX element; the timeout is finite.
                if unsafe { AXUIElementSetMessagingTimeout(owned.as_ptr(), NODE_TIMEOUT_SECONDS) }
                    != AX_OK
                {
                    continue;
                }
                if copy_string_attribute(owned.as_ptr(), &CFString::new("AXSubrole")).as_deref()
                    != Some("AXStandardWindow")
                {
                    continue;
                }
                let window = MovableWindow {
                    window: owned,
                    attributes: AxAttributes::new(),
                    fullscreen: CFString::new("AXFullScreen"),
                };
                if copy_bool_attribute(window.window.as_ptr(), &CFString::new("AXMinimized"))
                    == Some(true)
                {
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
            // Do not guess among identical windows in different Spaces. Match
            // public on-screen metadata only when one AX candidate is possible.
            for (rank, shown) in visible.iter().enumerate().filter(|(_, v)| v.pid == pid) {
                if cancelled() {
                    break;
                }
                let mut matches = candidates
                    .iter()
                    .enumerate()
                    .filter(|(_, (_, bounds, title))| {
                        same_rect(*bounds, shown.bounds)
                            && shown
                                .title
                                .as_ref()
                                .is_none_or(|t| t.is_empty() || t == title)
                    })
                    .map(|(index, _)| index);
                let unique = match (matches.next(), matches.next()) {
                    (Some(index), None) => Some(index),
                    _ => None,
                };
                if let Some(index) = unique {
                    let (window, _, _) = candidates.remove(index);
                    if let Ok(info) = self.retain(window, pid, screens) {
                        result.push((rank, info));
                    }
                }
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
        let maximized = entry.restored.is_some() && same_rect(bounds, screens[screen].work_area);
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
            restored: entry.restored.unwrap_or(bounds),
        })
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
        if before.info.fullscreen {
            return Err("Exit native fullscreen before adjusting this window".into());
        }
        if before.info.minimized && !self.hidden.contains(&id) {
            self.set_minimized(id, false, cancelled)?;
        }
        let window = &self.entry(id)?.window;
        if cancelled() {
            return Ok(before.info);
        }
        let resizing = (desired.width - before.info.bounds.width).abs() > 0.5
            || (desired.height - before.info.bounds.height).abs() > 0.5;
        if resizing {
            Self::set_size(window, desired.width, desired.height)?;
        }
        if cancelled() {
            return self.snapshot(id, screens).map(|s| s.info);
        }
        // Read back accepted dimensions before positioning, preserving the
        // requested centre when an application enforces its own size limits.
        let actual = element_rect(window.window.as_ptr(), &window.attributes)
            .ok_or("cannot read adjusted size")?;
        let center = desired.center();
        let position = if resizing {
            Point::new(
                center.x - actual.width / 2.0,
                center.y - actual.height / 2.0,
            )
        } else {
            Point::new(desired.x, desired.y)
        };
        if (position.x - actual.x).abs() > 0.5 || (position.y - actual.y).abs() > 0.5 {
            window
                .set_position(position)
                .map_err(|e| e.message().to_string())?;
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
        self.entries
            .get_mut(&snapshot.info.id)
            .ok_or("window was closed")?
            .restored = snapshot.info.maximized.then_some(snapshot.restored);
        self.set_frame(snapshot.info.id, snapshot.info.bounds, screens, cancelled)?;
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
        self.entries
            .get_mut(&id)
            .ok_or("window was closed")?
            .restored = Some(before.info.bounds);
        self.set_frame(
            id,
            screens
                .get(before.info.screen)
                .ok_or("display unavailable")?
                .work_area,
            screens,
            cancelled,
        )
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
            return Err(format!("cannot raise window: AXError {error}"));
        }
        Ok(())
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
            let timeout = AXUIElementSetMessagingTimeout(button.as_ptr(), NODE_TIMEOUT_SECONDS);
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
            std::thread::sleep(crate::platform::macos::window_move::POLL_INTERVAL);
        }
    }
    fn reset(&mut self) {
        for id in self.hidden.iter().copied().collect::<Vec<_>>() {
            if let Err(error) = self.tab_set_hidden(id, false) {
                crate::report_error!("window-tabs", "restore hidden window: {error}");
                return;
            }
        }
        crate::platform::macos::window_tabs::publish(&[]);
        self.monitor = Default::default();
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
