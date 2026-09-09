//! Retained HWND identities and bounded native window operations, worker-owned.
use super::window_mover::{read_bounds, read_placement, submit_frame, submit_placement};
use crate::api::window::{WindowId, WindowInfo};
use crate::api::{Point, Rect, Screen};
use crate::platform::common::window_geometry;
use crate::platform::common::window_session::{Snapshot, WindowAccess};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GWL_STYLE, GetPropW, MINMAXINFO, RemovePropW, SMTO_ABORTIFHUNG, SMTO_BLOCK,
    SW_MINIMIZE, SW_RESTORE, SW_SHOWMAXIMIZED, SW_SHOWNORMAL, SendMessageTimeoutW,
    SetForegroundWindow, SetPropW, ShowWindowAsync, SwitchToThisWindow, WINDOWPLACEMENT,
    WM_GETMINMAXINFO, WPF_ASYNCWINDOWPLACEMENT, WS_CHILD, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_MAXIMIZE, WS_THICKFRAME,
};
use windows::core::{BOOL, PCWSTR};

struct Identity {
    hwnd: HWND,
    pid: u32,
    thread: u32,
    app: String,
    cycle_restore: Option<Rect>,
}

pub(super) struct Windows {
    next: u64,
    windows: BTreeMap<WindowId, Identity>,
    property: Vec<u16>,
    handles: Vec<HWND>,
    closed: Vec<WindowId>,
}

impl Default for Windows {
    fn default() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let instance = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Self {
            next: 0,
            windows: BTreeMap::new(),
            handles: Vec::new(),
            closed: Vec::new(),
            property: format!(
                "KeySteer.WindowIdentity.{}.{instance}\0",
                std::process::id()
            )
            .encode_utf16()
            .collect(),
        }
    }
}

fn rect(r: RECT) -> Rect {
    Rect::new(
        r.left as f64,
        r.top as f64,
        f64::from(r.right) - r.left as f64,
        f64::from(r.bottom) - r.top as f64,
    )
}

fn eligible(hwnd: HWND) -> bool {
    let ex = super::native::window_long(hwnd, GWL_EXSTYLE) as u32;
    let style = super::native::window_long(hwnd, GWL_STYLE) as u32;
    ex & (WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0) == 0
        && style & WS_CHILD.0 == 0
        && super::accessibility::scannable_target(hwnd).is_some_and(|(root, _, _)| root == hwnd)
}

impl Windows {
    fn retain(&mut self, hwnd: HWND, screens: &[Screen]) -> Result<WindowInfo, String> {
        let mut pid = 0;
        let thread = super::native::window_thread_process_id(hwnd, Some(&mut pid));
        // The marker is also the reverse index; validate it against the owner
        // before using it. No scan of every retained window or PID lookup repeats.
        let id = self.marker(hwnd).filter(|id| {
            self.windows
                .get(id)
                .is_some_and(|w| w.hwnd == hwnd && w.pid == pid && w.thread == thread)
        });
        let id = if let Some(id) = id {
            id
        } else {
            self.next += 1;
            let id = WindowId(self.next);
            // SAFETY: unique NUL-terminated property name and opaque nonzero
            // integer token. Windows stores the token, never dereferences it.
            unsafe {
                SetPropW(
                    hwnd,
                    PCWSTR(self.property.as_ptr()),
                    Some(HANDLE(id.0 as usize as *mut _)),
                )
            }
            .map_err(|e| format!("cannot retain window identity: {e}"))?;
            let app = super::focused_app_for(hwnd, String::new()).bundle_id;
            self.windows.insert(
                id,
                Identity {
                    hwnd,
                    pid,
                    thread,
                    app,
                    cycle_restore: None,
                },
            );
            id
        };
        self.snapshot(id, screens).map(|s| s.info)
    }
    fn marked(&self, hwnd: HWND, id: WindowId) -> bool {
        self.marker(hwnd) == Some(id)
    }
    fn marker(&self, hwnd: HWND) -> Option<WindowId> {
        // SAFETY: live NUL-terminated name; opaque return value is compared only.
        let token = unsafe { GetPropW(hwnd, PCWSTR(self.property.as_ptr())) }.0 as usize;
        (token != 0).then_some(WindowId(token as u64))
    }
    fn prune_closed(&mut self) {
        let closed: Vec<_> = self
            .windows
            .iter()
            .filter_map(|(id, w)| {
                (!super::native::is_window(w.hwnd) || !self.marked(w.hwnd, *id)).then_some(*id)
            })
            .collect();
        for id in &closed {
            self.windows.remove(id);
        }
        self.closed.extend(closed);
    }
    fn hwnd(&self, id: WindowId) -> Result<HWND, String> {
        let w = self
            .windows
            .get(&id)
            .ok_or("window is no longer available")?;
        let mut pid = 0;
        let thread = super::native::window_thread_process_id(w.hwnd, Some(&mut pid));
        if !super::native::is_window(w.hwnd)
            || pid != w.pid
            || thread != w.thread
            || !self.marked(w.hwnd, id)
        {
            return Err("window was closed".into());
        }
        Ok(w.hwnd)
    }
    fn minimize(
        &mut self,
        id: WindowId,
        screens: &[Screen],
        cancelled: &dyn Fn() -> bool,
    ) -> Result<WindowInfo, String> {
        let before = self.snapshot(id, screens)?;
        if cancelled() {
            return Ok(before.info);
        }
        self.windows
            .get_mut(&id)
            .ok_or("window was closed")?
            .cycle_restore = Some(before.restored);
        let hwnd = self.hwnd(id)?;
        // SAFETY: validated borrowed HWND; asynchronous request retains no Rust data.
        if !unsafe { ShowWindowAsync(hwnd, SW_MINIMIZE) }.as_bool() {
            return Err("cannot minimize window".into());
        }
        let deadline = Instant::now() + Duration::from_millis(250);
        loop {
            let hwnd = self.hwnd(id)?;
            // SAFETY: identity was revalidated above.
            if super::native::is_window_iconic(hwnd) || cancelled() {
                return self.snapshot(id, screens).map(|s| s.info);
            }
            if Instant::now() >= deadline {
                return Err("Timed out minimizing window".into());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    fn placement(&self, hwnd: HWND) -> Result<WINDOWPLACEMENT, String> {
        read_placement(hwnd)
    }
    fn wait_frame(
        &self,
        id: WindowId,
        screens: &[Screen],
        desired: Option<Rect>,
        maximized: Option<bool>,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<WindowInfo, String> {
        let deadline = Instant::now() + Duration::from_millis(250);
        let mut previous = None;
        let mut stable = 0;
        loop {
            // Geometry polling must not query titles, executable names, restore
            // placements or accessibility metadata on every retry.
            let hwnd = self.hwnd(id)?;
            let bounds = super::accessibility::window_bounds(hwnd).ok_or("window was closed")?;
            if cancelled() {
                return self.snapshot(id, screens).map(|s| s.info);
            }
            let state_matches = maximized.is_none_or(|expected| {
                !super::native::is_window_iconic(hwnd)
                    && (super::native::window_long(hwnd, GWL_STYLE) as u32 & WS_MAXIMIZE.0 != 0)
                        == expected
            });
            if state_matches
                && desired.is_some_and(|d| {
                    (d.x - bounds.x).abs() < 2.0
                        && (d.y - bounds.y).abs() < 2.0
                        && (d.width - bounds.width).abs() < 2.0
                        && (d.height - bounds.height).abs() < 2.0
                })
            {
                return self.snapshot(id, screens).map(|s| s.info);
            }
            if previous == Some(bounds) {
                stable += 1;
            } else {
                stable = 0;
            }
            if state_matches && desired.is_none() && stable >= 5 || Instant::now() >= deadline {
                return self.snapshot(id, screens).map(|s| s.info);
            }
            previous = Some(bounds);
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl WindowAccess for Windows {
    fn logical_scale(&self, screen: &Screen) -> f64 {
        screen.scale
    }
    fn acquire(&mut self, point: Point, screens: &[Screen]) -> Result<Option<WindowInfo>, String> {
        let Some((hwnd, _, _)) = super::accessibility::movable_window_under_pointer(point)? else {
            return Ok(None);
        };
        if !eligible(hwnd) {
            return Ok(None);
        }
        self.retain(hwnd, screens).map(Some)
    }
    fn enumerate(
        &mut self,
        screens: &[Screen],
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Vec<WindowInfo>, String> {
        self.handles.clear();
        extern "system" fn collect(hwnd: HWND, data: LPARAM) -> BOOL {
            // SAFETY: EnumWindows is synchronous; data points to the live Vec
            // owned by enumerate, and callbacks run serially on this thread.
            let handles = unsafe { &mut *(data.0 as *mut Vec<HWND>) };
            if handles.len() < 4096 {
                handles.push(hwnd);
            }
            BOOL::from(handles.len() < 4096)
        }
        let enumeration = super::native::enum_windows(
            Some(collect),
            LPARAM((&mut self.handles as *mut Vec<HWND>) as isize),
        );
        if self.handles.len() < 4096 {
            enumeration.map_err(|e| format!("cannot enumerate windows: {e}"))?;
        }
        self.prune_closed();
        let mut windows = Vec::with_capacity(self.windows.len().min(256));
        for index in 0..self.handles.len() {
            let hwnd = self.handles[index];
            if cancelled() || windows.len() >= 256 {
                break;
            }
            if eligible(hwnd)
                && let Ok(info) = self.retain(hwnd, screens)
            {
                windows.push(info);
            }
        }
        Ok(windows)
    }
    fn snapshot(&self, id: WindowId, screens: &[Screen]) -> Result<Snapshot, String> {
        let hwnd = self.hwnd(id)?;
        let mut bounds =
            super::accessibility::window_bounds(hwnd).ok_or("cannot read window bounds")?;
        let screen = window_geometry::screen_index(screens, bounds).ok_or("no displays")?;
        let p = self.placement(hwnd)?;
        let mut restored = rect(p.rcNormalPosition);
        if super::native::window_long(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0 == 0 {
            restored.x += screens[screen].work_area.x - screens[screen].bounds.x;
            restored.y += screens[screen].work_area.y - screens[screen].bounds.y;
        }
        if restored.width <= 0.0 || restored.height <= 0.0 {
            restored = bounds;
        }
        // SAFETY: validated borrowed HWND.
        let minimized = super::native::is_window_iconic(hwnd);
        if !minimized && let Ok(outer) = read_bounds(hwnd) {
            restored.x += bounds.x - outer.x;
            restored.y += bounds.y - outer.y;
            restored.width += bounds.width - outer.width;
            restored.height += bounds.height - outer.height;
        }
        if let Some(original) = self.windows[&id].cycle_restore {
            restored = original;
        }
        if minimized {
            bounds = restored;
        }
        let screen = window_geometry::screen_index(screens, bounds).ok_or("no displays")?;
        let title = super::native::window_title(hwnd);
        let app = self.windows[&id].app.clone();
        let info = WindowInfo {
            id,
            title,
            app,
            bounds,
            screen,
            resizable: super::native::window_long(hwnd, GWL_STYLE) as u32 & WS_THICKFRAME.0 != 0,
            maximized: p.showCmd == SW_SHOWMAXIMIZED.0 as u32,
            minimized,
            fullscreen: false,
        };
        Ok(Snapshot { info, restored })
    }
    fn set_frame(
        &mut self,
        id: WindowId,
        desired: Rect,
        screens: &[Screen],
        cancelled: &dyn Fn() -> bool,
    ) -> Result<WindowInfo, String> {
        if ![desired.x, desired.y, desired.width, desired.height]
            .iter()
            .all(|v| v.is_finite() && v.abs() < i32::MAX as f64 / 2.0)
            || desired.width < 1.0
            || desired.height < 1.0
        {
            return Err("invalid window geometry".into());
        }
        let hwnd = self.hwnd(id)?;
        let before = self.snapshot(id, screens)?;
        if before.info.maximized || before.info.minimized {
            // SAFETY: validated borrowed HWND; asynchronous request retains no Rust data.
            if !unsafe {
                ShowWindowAsync(
                    hwnd,
                    if before.info.minimized {
                        SW_SHOWNORMAL
                    } else {
                        SW_RESTORE
                    },
                )
            }
            .as_bool()
            {
                return Err("cannot restore maximized window".into());
            }
            let restored =
                self.wait_frame(id, screens, Some(before.restored), Some(false), cancelled)?;
            if (restored.maximized || restored.minimized) && !cancelled() {
                return Err("Timed out restoring maximized window before resizing".into());
            }
        }
        if cancelled() {
            return self.snapshot(id, screens).map(|s| s.info);
        }
        let visible = if before.info.maximized || before.info.minimized {
            super::accessibility::window_bounds(hwnd).ok_or("window was closed")?
        } else {
            before.info.bounds
        };
        let raw = read_bounds(hwnd)?;
        let source = screens
            .get(before.info.screen)
            .ok_or("source display unavailable")?;
        let dest = window_geometry::screen_index(screens, desired)
            .and_then(|i| screens.get(i))
            .ok_or("destination display unavailable")?;
        let scale = dest.scale / source.scale;
        let native = Rect::new(
            desired.x + (raw.x - visible.x) * scale,
            desired.y + (raw.y - visible.y) * scale,
            desired.width + (raw.width - visible.width) * scale,
            desired.height + (raw.height - visible.height) * scale,
        );
        self.windows
            .get_mut(&id)
            .ok_or("window was closed")?
            .cycle_restore = None;
        submit_frame(hwnd, native)?;
        self.wait_frame(id, screens, Some(desired), Some(false), cancelled)
    }
    fn restore(
        &mut self,
        snapshot: &Snapshot,
        screens: &[Screen],
        cancelled: &dyn Fn() -> bool,
    ) -> Result<WindowInfo, String> {
        if snapshot.info.minimized {
            self.set_frame(snapshot.info.id, snapshot.restored, screens, cancelled)?;
            return self.minimize(snapshot.info.id, screens, cancelled);
        }
        if snapshot.info.maximized {
            self.set_frame(snapshot.info.id, snapshot.restored, screens, cancelled)?;
            if cancelled() {
                return self.snapshot(snapshot.info.id, screens).map(|s| s.info);
            }
            let hwnd = self.hwnd(snapshot.info.id)?;
            let mut p = self.placement(hwnd)?;
            p.showCmd = SW_SHOWMAXIMIZED.0 as u32;
            p.flags = WPF_ASYNCWINDOWPLACEMENT;
            let index = window_geometry::screen_index(screens, snapshot.info.bounds)
                .ok_or("display unavailable")?;
            let screen = &screens[index];
            // set_frame already established the correct outer/workspace normal
            // rectangle. Keep that native value instead of mixing visible DWM
            // bounds back into WINDOWPLACEMENT.
            p.ptMaxPosition = POINT { x: -1, y: -1 };
            submit_placement(hwnd, &p)?;
            self.wait_frame(
                snapshot.info.id,
                screens,
                Some(screen.work_area),
                Some(true),
                cancelled,
            )
        } else {
            self.set_frame(snapshot.info.id, snapshot.info.bounds, screens, cancelled)
        }
    }
    fn cycle_state(
        &mut self,
        id: WindowId,
        screens: &[Screen],
        cancelled: &dyn Fn() -> bool,
    ) -> Result<WindowInfo, String> {
        let before = self.snapshot(id, screens)?;
        if before.info.minimized {
            return self.set_frame(id, before.restored, screens, cancelled);
        }
        if before.info.maximized {
            return self.minimize(id, screens, cancelled);
        }
        if cancelled() {
            return Ok(before.info);
        }
        self.windows
            .get_mut(&id)
            .ok_or("window was closed")?
            .cycle_restore = Some(before.info.bounds);
        let hwnd = self.hwnd(id)?;
        // SAFETY: validated borrowed HWND; no Rust pointers retained.
        if !unsafe { ShowWindowAsync(hwnd, SW_SHOWMAXIMIZED) }.as_bool() {
            return Err("cannot maximize window".into());
        }
        let after = self.wait_frame(
            id,
            screens,
            Some(screens[before.info.screen].work_area),
            Some(true),
            cancelled,
        )?;
        if !after.maximized && !cancelled() {
            return Err("Timed out maximizing window".into());
        }
        Ok(after)
    }

    fn select(&self, id: WindowId) -> Result<(), String> {
        let hwnd = self.hwnd(id)?;
        // SAFETY: borrowed validated HWND; no pointers retained. This fallback
        // is used only for an explicit keyboard window-switch request, matching
        // SwitchToThisWindow's Alt/Tab semantics. Never attach input queues to
        // foreign UI threads: that can turn activation into an unbounded wait.
        unsafe {
            if !SetForegroundWindow(hwnd).as_bool() {
                SwitchToThisWindow(hwnd, true);
            }
        }
        let deadline = Instant::now() + Duration::from_millis(150);
        while super::native::foreground_window() != hwnd {
            self.hwnd(id)?;
            if Instant::now() >= deadline {
                return Err("Window selected · system denied keyboard focus".into());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        Ok(())
    }
    fn pointer(&self) -> Result<Point, String> {
        super::input::cursor_position()
    }
    fn minimum_size(&self, id: WindowId) -> Point {
        let Ok(hwnd) = self.hwnd(id) else {
            return Point::new(100.0, 80.0);
        };
        let mut info = MINMAXINFO::default();
        let mut result = 0;
        // SAFETY: WM_GETMINMAXINFO is a system message with marshalled struct
        // data. The initialized out-buffer lives for the bounded call.
        let completed = unsafe {
            SendMessageTimeoutW(
                hwnd,
                WM_GETMINMAXINFO,
                WPARAM(0),
                LPARAM((&mut info as *mut MINMAXINFO) as isize),
                SMTO_ABORTIFHUNG | SMTO_BLOCK,
                50,
                Some(&mut result),
            )
        };
        if completed.0 == 0 {
            crate::report_error!(
                "window-worker",
                "window {:?}: minimum-size query failed or timed out",
                id
            );
        }
        Point::new(
            f64::from(info.ptMinTrackSize.x).max(100.0),
            f64::from(info.ptMinTrackSize.y).max(80.0),
        )
    }
    fn reset(&mut self) {
        for (id, window) in &self.windows {
            if self.marked(window.hwnd, *id) {
                // SAFETY: remove only this adapter's verified opaque marker;
                // no pointers are owned by or reclaimed from the property.
                let removed = unsafe { RemovePropW(window.hwnd, PCWSTR(self.property.as_ptr())) };
                if let Err(error) = removed {
                    crate::report_error!(
                        "window-worker",
                        "release window {:?} identity: {error}",
                        id
                    );
                }
            }
        }
        self.windows.clear();
        self.handles.clear();
        self.closed.clear();
    }
    fn take_closed(&mut self) -> Vec<WindowId> {
        std::mem::take(&mut self.closed)
    }
}

impl Drop for Windows {
    fn drop(&mut self) {
        self.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    };

    struct Probe {
        hwnd: HWND,
        stop: Arc<AtomicBool>,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    impl Probe {
        fn start(area: Rect) -> Self {
            Self::with_style(area, WS_EX_NOACTIVATE)
        }
        fn with_style(
            area: Rect,
            ex_style: windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE,
        ) -> Self {
            let stop = Arc::new(AtomicBool::new(false));
            let stopped = stop.clone();
            let (tx, rx) = mpsc::channel();
            let thread = std::thread::spawn(move || {
                use windows::Win32::UI::WindowsAndMessaging::{
                    CreateWindowExW, DestroyWindow, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
                };
                use windows::core::w;
                // SAFETY: system STATIC class, static title, bounded geometry,
                // no parent or user data. This UI thread exclusively owns the
                // returned HWND until DestroyWindow below.
                let hwnd = unsafe {
                    CreateWindowExW(
                        ex_style,
                        w!("STATIC"),
                        w!("KeySteer disposable window test"),
                        WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                        area.x as i32,
                        area.y as i32,
                        480,
                        320,
                        None,
                        None,
                        None,
                        None,
                    )
                }
                .unwrap();
                tx.send(hwnd.0 as usize).unwrap();
                while !stopped.load(Ordering::Acquire) {
                    super::super::native::pump_window_messages();
                    std::thread::sleep(Duration::from_millis(2));
                }
                // SAFETY: this is the creating thread, it still owns hwnd, and
                // the test has completed all borrowed operations before stop.
                unsafe { DestroyWindow(hwnd) }.unwrap();
            });
            let hwnd = HWND(rx.recv_timeout(Duration::from_secs(5)).unwrap() as *mut _);
            Self {
                hwnd,
                stop,
                thread: Some(thread),
            }
        }
    }

    impl Drop for Probe {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Release);
            if let Some(thread) = self.thread.take() {
                thread.join().unwrap();
            }
        }
    }

    #[test]
    #[ignore = "creates and cycles only a disposable test-owned window"]
    fn native_window_three_state_cycle_and_undo() {
        use crate::api::window::{WindowChange, WindowOperation};
        use crate::platform::common::window_session::WindowSessionProbe;
        let _ = super::super::screens::enable_dpi_awareness();
        let screens = super::super::screens::list_screens().unwrap();
        let area = screens.iter().find(|s| s.is_primary).unwrap().work_area;
        let owned = Probe::start(area.inset(60.0, 60.0));
        let mut access = Windows::default();
        let info = access.retain(owned.hwnd, &screens).unwrap();
        let original = info.bounds;
        let mut session = WindowSessionProbe::new(info.id);
        for round in 0..2 {
            for phase in 0..3 {
                let result = session.execute(
                    &mut access,
                    WindowOperation::Adjust {
                        target: info.id,
                        change: WindowChange::CycleState,
                        group: round * 3 + phase + 1,
                    },
                    &screens,
                );
                assert!(result.message.is_none(), "{:?}", result.message);
                let target = result.target.unwrap();
                assert_eq!(target.id, info.id);
                assert_eq!(target.minimized, phase == 1);
                if phase == 0 {
                    assert!(target.maximized);
                }
                if phase == 1 {
                    assert!(result.pointer.is_none());
                }
                if phase == 2 {
                    assert!(!target.maximized);
                    assert!(
                        target.bounds.center().distance_to(&original.center()) < 3.0,
                        "{target:?} original={original:?}"
                    );
                    assert!((target.bounds.width - original.width).abs() < 3.0);
                    assert!((target.bounds.height - original.height).abs() < 3.0);
                }
            }
        }
        let undone = session.execute(&mut access, WindowOperation::Undo, &screens);
        assert_eq!(undone.skipped, 0, "{:?}", undone.message);
        assert_eq!(undone.changed, 1);
        assert!(undone.target.unwrap().minimized);
        let undone = session.execute(&mut access, WindowOperation::Undo, &screens);
        assert_eq!(undone.skipped, 0, "{:?}", undone.message);
        assert_eq!(undone.changed, 1);
        assert!(undone.target.unwrap().maximized);
    }

    #[test]
    #[ignore = "temporarily resizes KEYSTEER_PROBE_HWND, then restores its original placement"]
    fn native_window_minimum_tile_and_restore_probe() {
        let _ = super::super::screens::enable_dpi_awareness();
        let raw: usize = std::env::var("KEYSTEER_PROBE_HWND")
            .unwrap()
            .parse()
            .unwrap();
        let hwnd = HWND(raw as *mut _);
        let screens = super::super::screens::list_screens().unwrap();
        let mut access = Windows::default();
        let info = access.retain(hwnd, &screens).unwrap();
        let before = access.snapshot(info.id, &screens).unwrap();
        let area = screens[info.screen].work_area;
        let proposed = Rect::new(area.x + 20.0, area.y + 20.0, 500.0, 500.0);
        let adjusted = access.set_frame(info.id, proposed, &screens, &|| false);
        // Restore before any assertions: a failed verification must not leave
        // the user's application in the test geometry.
        let restored = access.restore(&before, &screens, &|| false);
        access.reset();
        let adjusted = adjusted.unwrap();
        let restored = restored.unwrap();
        crate::log_info!(
            "window_probe",
            "requested={proposed:?} accepted={:?} restored={:?}",
            adjusted.bounds,
            restored.bounds
        );
        assert_ne!(adjusted.bounds, before.info.bounds);
        assert!(
            restored
                .bounds
                .center()
                .distance_to(&before.info.bounds.center())
                < 3.0
        );
        assert!((restored.bounds.width - before.info.bounds.width).abs() < 3.0);
        assert!((restored.bounds.height - before.info.bounds.height).abs() < 3.0);
        assert_eq!(restored.maximized, before.info.maximized);
    }

    #[test]
    #[ignore = "read-only inventory of KEYSTEER_PROBE_HWND on the interactive desktop"]
    fn native_window_inventory_probe() {
        let _ = super::super::screens::enable_dpi_awareness();
        let raw: usize = std::env::var("KEYSTEER_PROBE_HWND")
            .unwrap()
            .parse()
            .unwrap();
        let hwnd = HWND(raw as *mut _);
        let screens = super::super::screens::list_screens().unwrap();
        crate::log_info!(
            "window_probe",
            "eligible={} visible={} iconic={} style={:x} ex={:x} root={:?} bounds={:?}",
            eligible(hwnd),
            super::super::native::is_window_visible(hwnd),
            super::super::native::is_window_iconic(hwnd),
            super::super::native::window_long(hwnd, GWL_STYLE),
            super::super::native::window_long(hwnd, GWL_EXSTYLE),
            super::super::native::root_owner(hwnd),
            super::super::accessibility::window_bounds(hwnd)
        );
        let mut access = Windows::default();
        let retained = access.retain(hwnd, &screens);
        if let Ok(info) = &retained {
            crate::log_info!(
                "window_probe",
                "title={} resizable={} bounds={:?} minimum={:?}",
                info.title,
                info.resizable,
                info.bounds,
                access.minimum_size(info.id)
            );
            let windows = access.enumerate(&screens, &|| false).unwrap();
            crate::log_info!(
                "window_probe",
                "enumerated={} included={}",
                windows.len(),
                windows.iter().any(|w| w.id == info.id)
            );
        } else {
            crate::log_info!("window_probe", "retain error: {retained:?}");
        }
        access.reset();
        assert!(retained.is_ok());
        assert!(eligible(hwnd));
    }

    #[test]
    #[ignore = "maximizes and tiles disposable owned windows, then restores their entry state"]
    fn native_maximized_window_can_tile_and_undo() {
        use crate::api::window::{WindowEditResult, WindowOperation as O};
        use crate::platform::common::window_session::WindowSessionProbe;
        let _ = super::super::screens::enable_dpi_awareness();
        let screens = super::super::screens::list_screens().unwrap();
        let area = screens.iter().find(|s| s.is_primary).unwrap().work_area;
        let first_probe = Probe::start(Rect::new(area.x + 40.0, area.y + 40.0, 480.0, 320.0));
        let second_probe = Probe::start(Rect::new(area.x + 100.0, area.y + 100.0, 480.0, 320.0));
        let mut access = Windows::default();
        let first = access.retain(first_probe.hwnd, &screens).unwrap();
        let second = access.retain(second_probe.hwnd, &screens).unwrap();
        let originals = [
            access.snapshot(first.id, &screens).unwrap(),
            access.snapshot(second.id, &screens).unwrap(),
        ];
        let outcome = (|| -> Result<(), String> {
            let maximized = access.cycle_state(first.id, &screens, &|| false)?;
            if !maximized.maximized {
                return Err("maximize did not settle".into());
            }
            let mut session = WindowSessionProbe::new(first.id);
            session.execute(
                &mut access,
                O::BeginEdit {
                    transaction: 91,
                    group: 91,
                    targets: vec![first.id, second.id],
                    screen: None,
                },
                &screens,
            );
            let applied = session.execute(
                &mut access,
                O::ApplyLayout {
                    transaction: 91,
                    revision: 1,
                    screen: first.screen,
                    gap: 8.0,
                    strict: true,
                    placements: vec![
                        (first.id, Rect::new(0.0, 0.0, 0.5, 1.0)),
                        (second.id, Rect::new(0.5, 0.0, 0.5, 1.0)),
                    ],
                },
                &screens,
            );
            if !matches!(
                applied.edit.as_deref(),
                Some(WindowEditResult::Applied { accepted: true, .. })
            ) {
                return Err(applied
                    .message
                    .unwrap_or_else(|| "maximized layout rejected".into()));
            }
            if access.snapshot(first.id, &screens)?.info.maximized {
                return Err("window remained maximized".into());
            }
            session.execute(
                &mut access,
                O::EndEdit {
                    transaction: 91,
                    commit: true,
                },
                &screens,
            );
            session.execute(&mut access, O::Undo, &screens);
            if !access.snapshot(first.id, &screens)?.info.maximized {
                return Err("undo did not restore maximization".into());
            }
            Ok(())
        })();
        for before in originals {
            access.restore(&before, &screens, &|| false).unwrap();
        }
        assert!(outcome.is_ok(), "{outcome:?}");
    }

    #[test]
    #[ignore = "moves KEYSTEER_PROBE_HWND (or an owned window) and an owned companion, then restores both"]
    fn native_window_layout_transaction_probe() {
        use crate::api::window::{WindowEditResult, WindowOperation as O};
        use crate::api::window_layout::LayoutTree;
        use crate::platform::common::window_session::WindowSessionProbe;
        let _ = super::super::screens::enable_dpi_awareness();
        let screens = super::super::screens::list_screens().unwrap();
        let area = screens.iter().find(|s| s.is_primary).unwrap().work_area;
        let owned = Probe::start(Rect::new(area.x + 30.0, area.y + 30.0, 500.0, 400.0));
        let companion = Probe::start(Rect::new(area.x + 70.0, area.y + 70.0, 320.0, 240.0));
        let hwnd = std::env::var("KEYSTEER_PROBE_HWND")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .map_or(owned.hwnd, |raw| HWND(raw as *mut _));
        let mut access = Windows::default();
        let first = access.retain(hwnd, &screens).unwrap();
        let second = access.retain(companion.hwnd, &screens).unwrap();
        let originals = [
            access.snapshot(first.id, &screens).unwrap(),
            access.snapshot(second.id, &screens).unwrap(),
        ];
        let mut session = WindowSessionProbe::new(first.id);
        let outcome = (|| -> Result<(), String> {
            let started = session.execute(
                &mut access,
                O::BeginEdit {
                    transaction: 10,
                    targets: vec![first.id, second.id],
                    screen: None,
                    group: 10,
                },
                &screens,
            );
            let Some(WindowEditResult::Started {
                minimums,
                gap_scale,
                ..
            }) = started.edit.as_deref()
            else {
                return Err("missing edit acknowledgement".into());
            };
            let area = screens[first.screen].work_area;
            let mut tree =
                LayoutTree::import(&[first.clone(), second.clone()], Some(first.id), area);
            let minimums = minimums.iter().copied().collect();
            tree.fit(&minimums, area, 8.0 * gap_scale)?;
            for revision in 1..=2 {
                if revision == 2 {
                    tree.resize(
                        crate::api::Direction::Left,
                        &crate::api::window_layout::DEFAULT_SPLIT_RATIOS,
                    );
                    tree.fit(&minimums, area, 8.0 * gap_scale)?;
                }
                let result = session.execute(
                    &mut access,
                    O::ApplyLayout {
                        transaction: 10,
                        revision,
                        screen: first.screen,
                        placements: tree
                            .slots()
                            .iter()
                            .filter_map(|s| s.window.map(|id| (id, s.rect)))
                            .collect(),
                        gap: 8.0,
                        strict: true,
                    },
                    &screens,
                );
                if !matches!(
                    result.edit.as_deref(),
                    Some(WindowEditResult::Applied { accepted: true, .. })
                ) {
                    return Err(result
                        .message
                        .unwrap_or_else(|| "native layout rejected".into()));
                }
            }
            session.execute(
                &mut access,
                O::EndEdit {
                    transaction: 10,
                    commit: true,
                },
                &screens,
            );
            let undo = session.execute(&mut access, O::Undo, &screens);
            if undo.changed != 2 {
                return Err(format!("batch undo restored {} windows", undo.changed));
            }
            session.execute(
                &mut access,
                O::BeginEdit {
                    transaction: 11,
                    targets: vec![first.id],
                    screen: None,
                    group: 11,
                },
                &screens,
            );
            let adjusted = session.execute(
                &mut access,
                O::ApplyLayout {
                    transaction: 11,
                    revision: 1,
                    screen: first.screen,
                    placements: vec![(first.id, Rect::new(0.0, 0.0, 0.5, 1.0))],
                    gap: 8.0,
                    strict: false,
                },
                &screens,
            );
            session.execute(
                &mut access,
                O::EndEdit {
                    transaction: 11,
                    commit: false,
                },
                &screens,
            );
            if !matches!(
                adjusted.edit.as_deref(),
                Some(WindowEditResult::Applied { accepted: true, .. })
            ) {
                return Err("quick placement failed".into());
            }
            for before in &originals {
                let actual = access.snapshot(before.info.id, &screens)?;
                if actual
                    .info
                    .bounds
                    .center()
                    .distance_to(&before.info.bounds.center())
                    >= 3.0
                    || (actual.info.bounds.width - before.info.bounds.width).abs() >= 3.0
                    || (actual.info.bounds.height - before.info.bounds.height).abs() >= 3.0
                    || actual.info.maximized != before.info.maximized
                {
                    return Err(format!(
                        "rollback mismatch: {:?} -> {:?}",
                        before.info.bounds, actual.info.bounds
                    ));
                }
            }
            Ok(())
        })();
        // Cleanup happens before assertions even when an intermediate operation
        // was refused. No failed probe may leave the user's application tiled.
        let restored: Vec<_> = originals
            .iter()
            .map(|s| access.restore(s, &screens, &|| false))
            .collect();
        access.reset();
        assert!(
            restored.iter().all(Result::is_ok),
            "native cleanup failed: {restored:?}"
        );
        assert!(outcome.is_ok(), "{outcome:?}");
        crate::log_info!(
            "window_probe",
            "layout transaction, divider adjustment, batch undo and quick rollback passed for {}",
            first.title
        );
    }

    #[test]
    #[ignore = "activates two owned disposable native windows; run explicitly on Windows"]
    fn native_tab_activation_visits_two_windows() {
        let _ = super::super::screens::enable_dpi_awareness();
        let screens = super::super::screens::list_screens().unwrap();
        let area = screens.iter().find(|s| s.is_primary).unwrap().work_area;
        let first = Probe::with_style(area.inset(70.0, 70.0), Default::default());
        let second = Probe::with_style(area.inset(100.0, 100.0), Default::default());
        let mut access = Windows::default();
        let first_id = access.retain(first.hwnd, &screens).unwrap().id;
        let second_id = access.retain(second.hwnd, &screens).unwrap().id;
        let mut session =
            crate::platform::common::window_session::WindowSessionProbe::new(first_id);
        for (id, hwnd) in [
            (first_id, first.hwnd),
            (second_id, second.hwnd),
            (first_id, first.hwnd),
        ] {
            let selected = session.execute(
                &mut access,
                crate::api::window::WindowOperation::Select(id),
                &screens,
            );
            assert!(selected.message.is_none(), "{:?}", selected.message);
            assert_eq!(
                selected.pointer,
                selected.target.as_ref().map(|w| w.bounds.center())
            );
            assert_eq!(super::super::native::foreground_window(), hwnd);
        }
        access.reset();
    }

    #[test]
    #[ignore = "creates an owned disposable native window; run explicitly on Windows"]
    fn native_window_adjust_restore_and_closed_identity() {
        let _ = super::super::screens::enable_dpi_awareness();
        let screens = super::super::screens::list_screens().unwrap();
        let area = screens.iter().find(|s| s.is_primary).unwrap().work_area;
        let probe = Probe::start(Rect::new(area.x + 40.0, area.y + 40.0, 480.0, 320.0));
        let mut access = Windows::default();
        // Bypass only the intentional self-process filter: every write below
        // targets this test's owned HWND, never another application's window.
        let info = access.retain(probe.hwnd, &screens).unwrap();
        let original = access.snapshot(info.id, &screens).unwrap();
        let desired = Rect::new(
            original.info.bounds.x + 90.0,
            original.info.bounds.y + 60.0,
            original.info.bounds.width + 80.0,
            original.info.bounds.height + 40.0,
        );
        let adjusted = access
            .set_frame(info.id, desired, &screens, &|| false)
            .unwrap();
        assert!(
            adjusted.bounds.center().distance_to(&desired.center()) < 3.0,
            "{adjusted:?} desired={desired:?}"
        );
        assert!((adjusted.bounds.width - desired.width).abs() < 3.0);
        let maximized = access.cycle_state(info.id, &screens, &|| false).unwrap();
        assert!(maximized.maximized);
        let max_snapshot = access.snapshot(info.id, &screens).unwrap();
        access
            .set_frame(info.id, desired, &screens, &|| false)
            .unwrap();
        let restored_max = access.restore(&max_snapshot, &screens, &|| false).unwrap();
        assert!(restored_max.maximized);
        let restored = access.restore(&original, &screens, &|| false).unwrap();
        assert!(!restored.maximized);
        assert!(
            restored
                .bounds
                .center()
                .distance_to(&original.info.bounds.center())
                < 3.0
        );
        let id = info.id;
        let mut owner = Windows::default();
        let owned_id = owner.retain(probe.hwnd, &screens).unwrap().id;
        let mut observer = Windows::default();
        observer.property.clone_from(&owner.property);
        assert!(observer.marked(probe.hwnd, owned_id));
        drop(owner);
        assert!(
            !observer.marked(probe.hwnd, owned_id),
            "Drop must release the marker on a live window"
        );
        drop(probe);
        assert!(
            access.snapshot(id, &screens).is_err(),
            "closed HWND identity must never retarget another window"
        );
        access.reset();
    }
}
