//! Retained HWND identities and bounded native window operations, worker-owned.
use super::window_mover::{read_bounds, read_placement, submit_frame, submit_placement};
use crate::api::window::{WindowId, WindowInfo};
use crate::api::{Point, Rect, Screen};
use crate::platform::common::window_geometry;
use crate::platform::common::window_session::{Snapshot, WindowAccess};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, POINT, RECT, WPARAM};

use windows::Win32::Foundation::{COLORREF, GetLastError, SetLastError, WIN32_ERROR};
use windows::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GWL_STYLE, GetPropW, MINMAXINFO, RemovePropW, SMTO_ABORTIFHUNG, SMTO_BLOCK,
    SW_MINIMIZE, SW_RESTORE, SW_SHOWMAXIMIZED, SW_SHOWNORMAL, SendMessageTimeoutW,
    SetForegroundWindow, SetPropW, ShowWindowAsync, SwitchToThisWindow, WINDOWPLACEMENT,
    WM_GETMINMAXINFO, WPF_ASYNCWINDOWPLACEMENT, WS_CHILD, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_MAXIMIZE, WS_THICKFRAME,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetLayeredWindowAttributes, LAYERED_WINDOW_ATTRIBUTES_FLAGS, LWA_ALPHA,
    SetLayeredWindowAttributes, SetWindowLongPtrW, WS_EX_LAYERED,
};
use windows::core::{BOOL, PCWSTR};

struct Identity {
    hwnd: HWND,
    pid: u32,
    thread: u32,
    app: String,
    cycle_restore: Option<Rect>,
}

struct PreparedFrame {
    bounds: Rect,
    placement: WINDOWPLACEMENT,
}

pub(super) struct Windows {
    next: u64,
    windows: BTreeMap<WindowId, Identity>,
    property: Vec<u16>,
    handles: Vec<HWND>,
    closed: Vec<WindowId>,
    tabs: super::window_tabs::NativeTabs,
    tab_screens: Vec<Screen>,
    hidden: std::collections::BTreeSet<WindowId>,
    transparent: std::collections::BTreeSet<WindowId>,
    prepared: BTreeMap<WindowId, PreparedFrame>,
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
            tabs: super::window_tabs::NativeTabs::default(),
            tab_screens: Vec::new(),
            hidden: Default::default(),
            transparent: Default::default(),
            prepared: BTreeMap::new(),
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
        && super::accessibility::ordinary_window_target(hwnd, true)
            .is_some_and(|(root, _, _)| root == hwnd)
}

pub(super) fn tab_window_visible(hwnd: HWND) -> bool {
    if !super::native::is_window_visible(hwnd) || super::accessibility::is_cloaked(hwnd) {
        return false;
    }
    if super::native::window_long(hwnd, GWL_EXSTYLE) as u32 & WS_EX_LAYERED.0 == 0 {
        return true;
    }
    let mut alpha = 255;
    let mut flags = LAYERED_WINDOW_ATTRIBUTES_FLAGS::default();
    // SAFETY: read-only fixed-size opacity outputs from a borrowed native window.
    let result =
        unsafe { GetLayeredWindowAttributes(hwnd, None, Some(&mut alpha), Some(&mut flags)) };
    result.is_err() || !flags.contains(LWA_ALPHA) || alpha != 0
}

impl Windows {
    fn tab_opacity(&mut self, id: WindowId, hwnd: HWND, hidden: bool) -> Result<bool, String> {
        let owned = self.transparent.contains(&id);
        let style = super::native::window_long(hwnd, GWL_EXSTYLE);
        // Never replace another application's existing per-pixel or alpha setup.
        if !owned && style as u32 & WS_EX_LAYERED.0 != 0 {
            return Ok(false);
        }
        // SAFETY: modify only the compositing flag we lease, not parent/style or
        // input ownership. The shared opacity has no pointers or retained buffers.
        unsafe {
            if !owned {
                SetLastError(WIN32_ERROR(0));
                if SetWindowLongPtrW(hwnd, GWL_EXSTYLE, (style as u32 | WS_EX_LAYERED.0) as isize)
                    == 0
                    && GetLastError() != WIN32_ERROR(0)
                {
                    return Ok(false);
                }
                self.transparent.insert(id);
            }
            if let Err(error) = SetLayeredWindowAttributes(
                hwnd,
                COLORREF(0),
                if hidden { 0 } else { 255 },
                LWA_ALPHA,
            ) {
                if !owned {
                    self.restore_opacity(id)?;
                    return Ok(false);
                }
                return Err(format!("Cannot restore tab opacity: {error}"));
            }
        }
        if hidden {
            self.hidden.insert(id);
        } else {
            self.hidden.remove(&id);
        }
        Ok(true)
    }
    fn restore_opacity(&mut self, id: WindowId) -> Result<(), String> {
        if !self.transparent.contains(&id) {
            return Ok(());
        }
        if let Ok(hwnd) = self.hwnd(id) {
            let style = super::native::window_long(hwnd, GWL_EXSTYLE);
            // SAFETY: remove only this adapter's added compositing bit; preserve
            // all other extended styles, including later application changes.
            unsafe {
                SetLastError(WIN32_ERROR(0));
                if SetWindowLongPtrW(
                    hwnd,
                    GWL_EXSTYLE,
                    (style as u32 & !WS_EX_LAYERED.0) as isize,
                ) == 0
                    && GetLastError() != WIN32_ERROR(0)
                {
                    return Err("Cannot restore tab window compositing style".into());
                }
            }
        }
        self.transparent.remove(&id);
        Ok(())
    }
    fn geometry_hwnd(&self, id: WindowId) -> Result<HWND, String> {
        self.hwnd(id)
    }
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
            self.hidden.remove(id);
            self.transparent.remove(id);
            self.prepared.remove(id);
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
        let hwnd = self.geometry_hwnd(id)?;
        // SAFETY: validated borrowed HWND; asynchronous request retains no Rust data.
        if !unsafe { ShowWindowAsync(hwnd, SW_MINIMIZE) }.as_bool() {
            return Err("cannot minimize window".into());
        }
        let deadline = Instant::now() + Duration::from_millis(250);
        loop {
            let hwnd = self.geometry_hwnd(id)?;
            // SAFETY: identity was revalidated above.
            if super::native::is_window_iconic(hwnd) || cancelled() {
                return self.snapshot(id, screens).map(|s| s.info);
            }
            if Instant::now() >= deadline {
                return Err("Timed out minimizing window".into());
            }
            super::window_tabs::NativeTabs::dispatch_messages();
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
            let hwnd = self.geometry_hwnd(id)?;
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
            super::window_tabs::NativeTabs::dispatch_messages();
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl WindowAccess for Windows {
    fn tab_minimize(
        &mut self,
        id: WindowId,
        screens: &[Screen],
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), String> {
        self.minimize(id, screens, cancelled).map(|_| ())
    }
    fn tab_bar_height(&self, screen: &Screen) -> f64 {
        (30.0 * screen.scale).round()
    }
    fn tab_fit_frame(
        &mut self,
        id: WindowId,
        bounds: Rect,
        screens: &[Screen],
        cancelled: &dyn Fn() -> bool,
    ) -> Result<WindowInfo, String> {
        let before = self.snapshot(id, screens)?;
        if !before.info.maximized {
            return self.set_frame(id, bounds, screens, cancelled);
        }
        if cancelled() {
            return Err("Window grouping cancelled".into());
        }
        let hwnd = self.geometry_hwnd(id)?;
        // DWM clips a maximized frame's resize border at the work-area edge.
        // Once moved inward that clipping changes; remeasure the border before
        // one bounded correction instead of clearing the maximize state.
        for _ in 0..2 {
            let raw = read_bounds(hwnd)?;
            let visible = super::accessibility::window_bounds(hwnd).ok_or("window was closed")?;
            submit_frame(
                hwnd,
                Rect::new(
                    bounds.x + raw.x - visible.x,
                    bounds.y + raw.y - visible.y,
                    bounds.width + raw.width - visible.width,
                    bounds.height + raw.height - visible.height,
                ),
            )?;
            let applied = self.wait_frame(id, screens, Some(bounds), Some(true), cancelled)?;
            if (applied.bounds.x - bounds.x).abs() <= 1.0
                && (applied.bounds.y - bounds.y).abs() <= 1.0
                && (applied.bounds.width - bounds.width).abs() <= 1.0
                && (applied.bounds.height - bounds.height).abs() <= 1.0
            {
                return Ok(applied);
            }
            if cancelled() {
                break;
            }
        }
        Err("The maximized window refused space for the tab strip".into())
    }
    fn event_waker(&self) -> Option<std::sync::Arc<dyn Fn() + Send + Sync>> {
        Some(super::window_tabs::event_waker())
    }
    fn wait_for_events(&self, timeout: Option<Duration>) -> bool {
        super::window_tabs::wait_for_events(timeout.map_or(u32::MAX, |value| {
            value.as_millis().min((u32::MAX - 1) as u128) as u32
        }));
        true
    }
    fn tab_set_hidden(&mut self, id: WindowId, hidden: bool) -> Result<(), String> {
        if !hidden && !self.hidden.contains(&id) {
            if let Ok(hwnd) = self.hwnd(id) {
                self.tabs.member_shown(id, hwnd)?;
            }
            return Ok(());
        }
        let hwnd = match self.hwnd(id) {
            Ok(hwnd) => hwnd,
            Err(_) if !hidden => {
                self.hidden.remove(&id);
                self.transparent.remove(&id);
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        if hidden && self.tab_opacity(id, hwnd, true)? {
            return Ok(());
        }
        if !hidden && self.transparent.contains(&id) {
            if let Some(prepared) = self.prepared.get(&id) {
                let mut placement = prepared.placement;
                placement.showCmd =
                    windows::Win32::UI::WindowsAndMessaging::SW_SHOWNOACTIVATE.0 as u32;
                placement.flags = WPF_ASYNCWINDOWPLACEMENT;
                submit_placement(hwnd, &placement)?;
                let applied = self.wait_frame(
                    id,
                    &self.tab_screens,
                    Some(prepared.bounds),
                    Some(false),
                    &|| false,
                )?;
                if applied.maximized
                    || applied.minimized
                    || (applied.bounds.x - prepared.bounds.x).abs() > 2.0
                    || (applied.bounds.y - prepared.bounds.y).abs() > 2.0
                    || (applied.bounds.width - prepared.bounds.width).abs() > 2.0
                    || (applied.bounds.height - prepared.bounds.height).abs() > 2.0
                {
                    return Err("The application refused the prepared tab geometry".into());
                }
                self.prepared.remove(&id);
            }
            self.tabs.member_shown(id, hwnd)?;
            self.tab_opacity(id, hwnd, false)?;
            return Ok(());
        }
        if hidden && self.hidden.contains(&id) && !super::native::is_window_visible(hwnd) {
            return Ok(());
        }
        if !hidden && self.prepared.contains_key(&id) {
            let mut placement = self.prepared[&id].placement;
            placement.showCmd = windows::Win32::UI::WindowsAndMessaging::SW_SHOWNOACTIVATE.0 as u32;
            placement.flags = WPF_ASYNCWINDOWPLACEMENT;
            submit_placement(hwnd, &placement)?;
        } else {
            // SAFETY: validated identity; asynchronous visibility only, no parent/style changes.
            unsafe {
                let _ = ShowWindowAsync(
                    hwnd,
                    if hidden {
                        windows::Win32::UI::WindowsAndMessaging::SW_HIDE
                    } else {
                        windows::Win32::UI::WindowsAndMessaging::SW_SHOWNA
                    },
                );
            }
        }
        let deadline = Instant::now() + Duration::from_millis(250);
        while super::native::is_window_visible(hwnd) == hidden {
            super::window_tabs::NativeTabs::dispatch_messages();
            self.hwnd(id)?;
            if Instant::now() >= deadline {
                return Err("Timed out changing tab visibility".into());
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        if hidden {
            self.hidden.insert(id);
        } else {
            if let Some(prepared) = self.prepared.remove(&id) {
                let screens = self.tab_screens.clone();
                let applied =
                    match self
                        .wait_frame(id, &screens, Some(prepared.bounds), Some(false), &|| false)
                    {
                        Ok(applied) => applied,
                        Err(error) => {
                            self.tab_set_hidden(id, true)?;
                            return Err(error);
                        }
                    };
                if applied.maximized
                    || applied.minimized
                    || (applied.bounds.x - prepared.bounds.x).abs() > 2.0
                    || (applied.bounds.y - prepared.bounds.y).abs() > 2.0
                    || (applied.bounds.width - prepared.bounds.width).abs() > 2.0
                    || (applied.bounds.height - prepared.bounds.height).abs() > 2.0
                {
                    self.tab_set_hidden(id, true)?;
                    return Err("The application refused the prepared tab geometry".into());
                }
            }
            self.hidden.remove(&id);
            // Transfer ownership before the coordinator hides the former active
            // window, so its owned strip does not disappear during a tab switch.
            self.tabs.member_shown(id, hwnd)?;
        }
        Ok(())
    }
    fn tab_selected(&self, id: WindowId) -> bool {
        self.hwnd(id)
            .is_ok_and(|hwnd| hwnd == super::native::foreground_window())
    }
    fn tab_watch(&mut self, ids: &[WindowId]) -> Result<(), String> {
        let windows = ids
            .iter()
            .map(|id| self.hwnd(*id).map(|hwnd| (*id, hwnd)))
            .collect::<Result<Vec<_>, _>>()?;
        self.tabs.watch(&windows)
    }
    fn tab_events(&mut self) -> Vec<crate::api::window_tabs::TabNativeEvent> {
        self.tabs.events()
    }
    fn tab_bars(&mut self, bars: &[crate::api::window_tabs::TabBar]) -> Result<(), String> {
        let release: Vec<_> = self
            .transparent
            .iter()
            .copied()
            .filter(|id| {
                !bars
                    .iter()
                    .any(|bar| bar.tabs.iter().any(|(member, _, _)| member == id))
            })
            .collect();
        for id in release {
            self.restore_opacity(id)?;
        }
        self.tabs.show(bars, &self.tab_screens)
    }
    fn tab_visible(&self, id: WindowId) -> bool {
        self.geometry_hwnd(id).is_ok_and(tab_window_visible)
    }
    fn tab_eligible(&self, id: WindowId, screens: &[Screen]) -> Result<(), String> {
        if self.hidden.contains(&id) {
            return Ok(());
        }
        let window = self.snapshot(id, screens)?.info;
        let hwnd = self.hwnd(id)?;
        if !window.resizable
            || window.fullscreen
            || !eligible(hwnd)
            || super::native::window_long(hwnd, GWL_EXSTYLE) as u32
                & windows::Win32::UI::WindowsAndMessaging::WS_EX_TOPMOST.0
                != 0
        {
            return Err(
                "Only ordinary resizable windows on the current desktop can join a tab group"
                    .into(),
            );
        }
        Ok(())
    }
    fn logical_scale(&self, screen: &Screen) -> f64 {
        screen.scale
    }
    fn acquire(&mut self, point: Point, screens: &[Screen]) -> Result<Option<WindowInfo>, String> {
        self.tab_screens = screens.to_vec();
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
        self.tab_screens = screens.to_vec();
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
        // Invisible members retain their stable identity and remain selectable.
        for id in &self.hidden {
            if !windows.iter().any(|w| w.id == *id)
                && let Ok(snapshot) = self.snapshot(*id, screens)
            {
                windows.push(snapshot.info);
            }
        }
        Ok(windows)
    }
    fn snapshot(&self, id: WindowId, screens: &[Screen]) -> Result<Snapshot, String> {
        let application = self.hwnd(id)?;
        let hwnd = application;
        let mut bounds =
            super::accessibility::window_bounds(hwnd).ok_or("cannot read window bounds")?;
        let p = self.placement(hwnd)?;
        let minimized = super::native::is_window_iconic(hwnd);
        let mut restored = rect(p.rcNormalPosition);
        // Iconic window bounds may be the off-screen -32000 sentinel. Resolve
        // the display from its restore placement before applying workspace offsets.
        let screen =
            window_geometry::screen_index(screens, if minimized { restored } else { bounds })
                .ok_or("no displays")?;
        if super::native::window_long(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0 == 0 {
            restored.x += screens[screen].work_area.x - screens[screen].bounds.x;
            restored.y += screens[screen].work_area.y - screens[screen].bounds.y;
        }
        if restored.width <= 0.0 || restored.height <= 0.0 {
            restored = bounds;
        }
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
        let title = super::native::window_title(application);
        let app = self.windows[&id].app.clone();
        let mut info = WindowInfo {
            id,
            title,
            app,
            bounds,
            screen,
            resizable: super::native::window_long(hwnd, GWL_STYLE) as u32 & WS_THICKFRAME.0 != 0,
            maximized: p.showCmd == SW_SHOWMAXIMIZED.0 as u32,
            minimized,
            fullscreen: !minimized
                && p.showCmd != SW_SHOWMAXIMIZED.0 as u32
                && super::native::window_long(hwnd, GWL_STYLE) as u32
                    & windows::Win32::UI::WindowsAndMessaging::WS_CAPTION.0
                    == 0
                && bounds.width >= screens[screen].bounds.width
                && bounds.height >= screens[screen].bounds.height,
        };
        if let Some(prepared) = self.prepared.get(&id) {
            info.bounds = prepared.bounds;
            info.screen = window_geometry::screen_index(screens, prepared.bounds)
                .ok_or("display unavailable")?;
            info.maximized = false;
            info.minimized = false;
            restored = prepared.bounds;
        }
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
        let hwnd = self.geometry_hwnd(id)?;
        let before = self.snapshot(id, screens)?;
        if self.hidden.contains(&id)
            && (before.info.maximized || before.info.minimized || self.prepared.contains_key(&id))
        {
            // Prepare the normal rectangle without restoring a maximized window
            // on the desktop. The reveal commits normal state and geometry together.
            let mut placement = self.placement(hwnd)?;
            let source = screens
                .get(before.info.screen)
                .ok_or("display unavailable")?;
            let mut raw = rect(placement.rcNormalPosition);
            raw.x += source.work_area.x - source.bounds.x;
            raw.y += source.work_area.y - source.bounds.y;
            let destination = window_geometry::screen_index(screens, desired)
                .and_then(|index| screens.get(index))
                .ok_or("display unavailable")?;
            let scale = destination.scale / source.scale;
            let outer = Rect::new(
                desired.x + (raw.x - before.restored.x) * scale,
                desired.y + (raw.y - before.restored.y) * scale,
                desired.width + (raw.width - before.restored.width) * scale,
                desired.height + (raw.height - before.restored.height) * scale,
            );
            let x = outer.x - (destination.work_area.x - destination.bounds.x);
            let y = outer.y - (destination.work_area.y - destination.bounds.y);
            placement.rcNormalPosition = RECT {
                left: x.round() as i32,
                top: y.round() as i32,
                right: (x + outer.width).round() as i32,
                bottom: (y + outer.height).round() as i32,
            };
            placement.showCmd = windows::Win32::UI::WindowsAndMessaging::SW_HIDE.0 as u32;
            placement.flags = WPF_ASYNCWINDOWPLACEMENT;
            self.prepared.insert(
                id,
                PreparedFrame {
                    bounds: desired,
                    placement,
                },
            );
            self.windows
                .get_mut(&id)
                .ok_or("window was closed")?
                .cycle_restore = None;
            return self.snapshot(id, screens).map(|snapshot| snapshot.info);
        }
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
            let hwnd = self.geometry_hwnd(snapshot.info.id)?;
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
        let hwnd = self.geometry_hwnd(id)?;
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
            super::window_tabs::NativeTabs::dispatch_messages();
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
    fn close(&self, id: WindowId) -> Result<(), String> {
        let hwnd = self.hwnd(id)?;
        // SAFETY: post a pointer-free standard close request to the validated
        // target. The application owns save prompts and may decline to close.
        unsafe {
            windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                Some(hwnd),
                windows::Win32::UI::WindowsAndMessaging::WM_CLOSE,
                WPARAM(0),
                LPARAM(0),
            )
        }
        .map_err(|error| format!("Cannot request window close: {error}"))
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
        if let Err(error) = self.tabs.show(&[], &self.tab_screens) {
            crate::report_error!(
                "window-tabs",
                "restore members before releasing identities: {error}"
            );
            return;
        }
        for id in self.hidden.iter().copied().collect::<Vec<_>>() {
            if let Err(error) = self.tab_set_hidden(id, false) {
                crate::report_error!("window-tabs", "restore hidden window: {error}");
                return;
            }
        }
        for id in self.transparent.iter().copied().collect::<Vec<_>>() {
            if let Err(error) = self.restore_opacity(id) {
                crate::report_error!("window-tabs", "{error}");
                return;
            }
        }
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
        self.transparent.clear();
        self.prepared.clear();
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

    pub(super) const PROBE_MOVEMENTS: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 0x76;
    pub(super) const PROBE_VISIBLE_MUTATIONS: u32 =
        windows::Win32::UI::WindowsAndMessaging::WM_APP + 0x77;
    unsafe extern "system" fn tab_probe_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> windows::Win32::Foundation::LRESULT {
        use windows::Win32::Foundation::LRESULT;
        use windows::Win32::UI::WindowsAndMessaging::*;
        // SAFETY: this test-owned class stores an integer counter, not a pointer.
        // STYLESTRUCT is supplied and marshalled by Windows for WM_STYLECHANGING.
        unsafe {
            let property = windows::core::w!("KeySteer.Probe.VisibleMutations");
            let visible_top_level = tab_window_visible(hwnd)
                && GetWindowLongPtrW(hwnd, GWL_STYLE) as u32 & WS_CHILD.0 == 0;
            let visible_mutation = visible_top_level
                && match message {
                    WM_WINDOWPOSCHANGING if lparam.0 != 0 => {
                        let position = &*(lparam.0 as *const WINDOWPOS);
                        !position.flags.contains(SWP_NOMOVE | SWP_NOSIZE)
                    }
                    WM_STYLECHANGING if lparam.0 != 0 && wparam.0 as i32 == GWL_STYLE.0 => {
                        let style = &*(lparam.0 as *const STYLESTRUCT);
                        (style.styleOld ^ style.styleNew)
                            & (WS_CHILD | WS_CAPTION | WS_THICKFRAME).0
                            != 0
                    }
                    _ => false,
                };
            if visible_mutation {
                let count = GetPropW(hwnd, property).0 as usize;
                let _ = SetPropW(
                    hwnd,
                    property,
                    Some(windows::Win32::Foundation::HANDLE(
                        count.saturating_add(1) as *mut _
                    )),
                );
            }
            match message {
                WM_SHOWWINDOW => {
                    let key = windows::core::w!("KeySteer.Probe.ShowEvents");
                    let count = GetPropW(hwnd, key).0 as usize;
                    let _ = SetPropW(hwnd, key, Some(HANDLE(count.saturating_add(1) as *mut _)));
                }
                WM_WINDOWPOSCHANGED => {
                    let count = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, count.saturating_add(1));
                }
                PROBE_MOVEMENTS => return LRESULT(GetWindowLongPtrW(hwnd, GWLP_USERDATA)),
                PROBE_VISIBLE_MUTATIONS => return LRESULT(GetPropW(hwnd, property).0 as isize),
                WM_STYLECHANGING
                    if wparam.0 as i32 == GWL_STYLE.0
                        && lparam.0 != 0
                        && std::env::var_os("KEYSTEER_TABS_TEST_REJECT").is_some() =>
                {
                    let style = &mut *(lparam.0 as *mut STYLESTRUCT);
                    style.styleNew &= !WS_CHILD.0;
                }
                _ => {}
            }
            DefWindowProcW(hwnd, message, wparam, lparam)
        }
    }

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
            Self::create(area, ex_style, false)
        }
        fn observable(area: Rect) -> Self {
            Self::create(area, WS_EX_NOACTIVATE, true)
        }
        fn create(
            area: Rect,
            ex_style: windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE,
            observable: bool,
        ) -> Self {
            let stop = Arc::new(AtomicBool::new(false));
            let stopped = stop.clone();
            let (tx, rx) = mpsc::channel();
            let thread = std::thread::spawn(move || {
                use windows::Win32::UI::WindowsAndMessaging::{
                    CreateWindowExW, DestroyWindow, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
                };
                use windows::core::w;
                let class =
                    if observable || std::env::var_os("KEYSTEER_TABS_TEST_HANDLES").is_some() {
                        static CLASS: std::sync::OnceLock<()> = std::sync::OnceLock::new();
                        CLASS.get_or_init(|| {
                            let class = windows::Win32::UI::WindowsAndMessaging::WNDCLASSW {
                                lpfnWndProc: Some(tab_probe_proc),
                                lpszClassName: w!("KeySteerTabProbe"),
                                ..Default::default()
                            };
                            assert_ne!(
                                // SAFETY: test-only process-lifetime class and callback.
                                unsafe {
                                    windows::Win32::UI::WindowsAndMessaging::RegisterClassW(&class)
                                },
                                0
                            );
                        });
                        if std::env::var_os("KEYSTEER_TABS_TEST_SYSTEM_DPI").is_some() {
                            // SAFETY: changes only this disposable test UI thread before window creation.
                            unsafe {
                                windows::Win32::UI::HiDpi::SetThreadDpiAwarenessContext(
                                    windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_SYSTEM_AWARE,
                                );
                            }
                        }
                        w!("KeySteerTabProbe")
                    } else {
                        w!("STATIC")
                    };
                // SAFETY: system STATIC class, static title, bounded geometry,
                // no parent or user data. This UI thread exclusively owns the
                // returned HWND until DestroyWindow below.
                let hwnd = unsafe {
                    CreateWindowExW(
                        ex_style,
                        class,
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
                if super::super::native::is_window(hwnd) {
                    // SAFETY: this is the creating thread, it still owns hwnd,
                    // and the test has completed all borrowed operations.
                    unsafe { DestroyWindow(hwnd) }.unwrap();
                }
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
                // The disposable app may own our test strip on this thread.
                // Keep servicing its destruction messages until the app exits.
                while !thread.is_finished() {
                    super::super::window_tabs::NativeTabs::dispatch_messages();
                    std::thread::yield_now();
                }
                thread.join().unwrap();
            }
        }
    }

    #[test]
    #[ignore = "restores and redoes geometry only on a disposable test-owned window"]
    fn native_window_initial_restore_and_redo() {
        use crate::api::window::{WindowChange, WindowOperation};
        use crate::platform::common::window_session::WindowSessionProbe;
        let _ = super::super::screens::enable_dpi_awareness();
        let screens = super::super::screens::list_screens().unwrap();
        let area = screens.iter().find(|s| s.is_primary).unwrap().work_area;
        let owned = Probe::start(area.inset(70.0, 70.0));
        let mut access = Windows::default();
        let original = access.retain(owned.hwnd, &screens).unwrap();
        let mut session = WindowSessionProbe::new(original.id);
        for group in 1..=2 {
            let result = session.execute(
                &mut access,
                WindowOperation::Adjust {
                    target: original.id,
                    change: WindowChange::CycleState,
                    group,
                },
                &screens,
            );
            assert!(result.message.is_none(), "{:?}", result.message);
        }
        assert!(
            access
                .snapshot(original.id, &screens)
                .unwrap()
                .info
                .minimized
        );
        let reset = session.execute(
            &mut access,
            WindowOperation::ResetInitial { group: 3 },
            &screens,
        );
        assert_eq!(reset.skipped, 0, "{:?}", reset.message);
        let restored = reset.target.unwrap();
        assert!(!restored.minimized && !restored.maximized);
        assert!(
            restored
                .bounds
                .center()
                .distance_to(&original.bounds.center())
                < 3.0
        );
        assert!((restored.bounds.width - original.bounds.width).abs() < 3.0);
        assert!((restored.bounds.height - original.bounds.height).abs() < 3.0);
        let undo = session.execute(&mut access, WindowOperation::Undo, &screens);
        assert_eq!(undo.skipped, 0, "{:?}", undo.message);
        assert!(undo.target.unwrap().minimized);
        let redo = session.execute(&mut access, WindowOperation::Redo, &screens);
        assert_eq!(redo.skipped, 0, "{:?}", redo.message);
        let restored = redo.target.unwrap();
        assert!(!restored.minimized && !restored.maximized);
        assert!(
            restored
                .bounds
                .center()
                .distance_to(&original.bounds.center())
                < 3.0
        );
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
                    additional_screens: Vec::new(),
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
                        additional_screens: Vec::new(),
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
                    additional_screens: Vec::new(),
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
        let first = Probe::create(area.inset(70.0, 70.0), Default::default(), true);
        let second = Probe::create(area.inset(100.0, 100.0), Default::default(), true);
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

    /// Native geometry/visibility adapter restricted to this probe's windows.
    /// Production discovery deliberately excludes the current process, so the
    /// test uses the portable eligibility check on its explicit identity list.
    struct NativeProbeAccess {
        native: Windows,
        ids: Vec<WindowId>,
    }
    impl WindowAccess for NativeProbeAccess {
        fn tab_minimize(
            &mut self,
            id: WindowId,
            screens: &[Screen],
            cancelled: &dyn Fn() -> bool,
        ) -> Result<(), String> {
            self.native.tab_minimize(id, screens, cancelled)
        }
        fn acquire(&mut self, _: Point, screens: &[Screen]) -> Result<Option<WindowInfo>, String> {
            self.native
                .snapshot(self.ids[0], screens)
                .map(|s| Some(s.info))
        }
        fn enumerate(
            &mut self,
            screens: &[Screen],
            _: &dyn Fn() -> bool,
        ) -> Result<Vec<WindowInfo>, String> {
            self.ids
                .iter()
                .map(|id| self.native.snapshot(*id, screens).map(|s| s.info))
                .collect()
        }
        fn snapshot(&self, id: WindowId, screens: &[Screen]) -> Result<Snapshot, String> {
            self.native.snapshot(id, screens)
        }
        fn set_frame(
            &mut self,
            id: WindowId,
            rect: Rect,
            screens: &[Screen],
            cancelled: &dyn Fn() -> bool,
        ) -> Result<WindowInfo, String> {
            self.native.set_frame(id, rect, screens, cancelled)
        }
        fn restore(
            &mut self,
            snapshot: &Snapshot,
            screens: &[Screen],
            cancelled: &dyn Fn() -> bool,
        ) -> Result<WindowInfo, String> {
            self.native.restore(snapshot, screens, cancelled)
        }
        fn cycle_state(
            &mut self,
            id: WindowId,
            screens: &[Screen],
            cancelled: &dyn Fn() -> bool,
        ) -> Result<WindowInfo, String> {
            self.native.cycle_state(id, screens, cancelled)
        }
        fn select(&self, id: WindowId) -> Result<(), String> {
            self.native.select(id)
        }
        fn pointer(&self) -> Result<Point, String> {
            self.native.pointer()
        }
        fn minimum_size(&self, id: WindowId) -> Point {
            self.native.minimum_size(id)
        }
        fn logical_scale(&self, screen: &Screen) -> f64 {
            self.native.logical_scale(screen)
        }
        fn tab_bar_height(&self, screen: &Screen) -> f64 {
            self.native.tab_bar_height(screen)
        }
        fn tab_fit_frame(
            &mut self,
            id: WindowId,
            bounds: Rect,
            screens: &[Screen],
            cancelled: &dyn Fn() -> bool,
        ) -> Result<WindowInfo, String> {
            self.native.tab_fit_frame(id, bounds, screens, cancelled)
        }
        fn tab_set_hidden(&mut self, id: WindowId, hidden: bool) -> Result<(), String> {
            self.native.tab_set_hidden(id, hidden)
        }
        fn tab_watch(&mut self, ids: &[WindowId]) -> Result<(), String> {
            self.native.tab_watch(ids)
        }
        fn tab_bars(&mut self, bars: &[crate::api::window_tabs::TabBar]) -> Result<(), String> {
            self.native.tab_bars(bars)
        }
        fn tab_visible(&self, id: WindowId) -> bool {
            self.native.tab_visible(id)
        }
        fn tab_events(&mut self) -> Vec<crate::api::window_tabs::TabNativeEvent> {
            self.native.tab_events()
        }
        fn tab_selected(&self, id: WindowId) -> bool {
            self.native.tab_selected(id)
        }
        fn reset(&mut self) {
            self.native.reset();
        }
    }

    #[test]
    #[ignore = "tiles only disposable windows with the actual grouped native adapter"]
    fn native_grouped_layout_accounts_for_header_and_dissolves_maximized() {
        use crate::api::window::{WindowEditResult, WindowOperation as O};
        use crate::api::window_tabs::{TabOperation, WindowTarget};
        use crate::platform::common::{WindowGroupsProbe, window_session::WindowSessionProbe};
        let _ = super::super::screens::enable_dpi_awareness();
        let screens = super::super::screens::list_screens().unwrap();
        let index = screens.iter().position(|s| s.is_primary).unwrap();
        let area = screens[index].work_area;
        let first = Probe::create(area.inset(160.0, 140.0), Default::default(), true);
        let second = Probe::create(area.inset(220.0, 180.0), Default::default(), true);
        let third = Probe::create(area.inset(280.0, 220.0), Default::default(), true);
        let mut native = Windows::default();
        native.tab_screens = screens.clone();
        let ids: Vec<_> = [first.hwnd, second.hwnd, third.hwnd]
            .iter()
            .map(|hwnd| native.retain(*hwnd, &screens).unwrap().id)
            .collect();
        let header = native.tab_bar_height(&screens[index]);
        let mut grouped = WindowGroupsProbe::new(NativeProbeAccess {
            native,
            ids: ids.clone(),
        });
        for id in &ids[..2] {
            grouped
                .tab_operation(
                    TabOperation::Choose(WindowTarget::Window(*id)),
                    &screens,
                    &|| false,
                )
                .unwrap();
        }
        let original_group = grouped.snapshot(ids[0], &screens).unwrap().info.bounds;
        let mut session = WindowSessionProbe::new(ids[0]);
        session.execute(
            &mut grouped,
            O::BeginEdit {
                transaction: 1,
                targets: vec![ids[0], ids[2]],
                screen: Some(index),
                group: 1,
            },
            &screens,
        );
        let half = Rect::new(0.0, 0.0, 0.5, 1.0);
        let result = session.execute(
            &mut grouped,
            O::ApplyLayout {
                additional_screens: Vec::new(),
                transaction: 1,
                revision: 1,
                screen: index,
                placements: vec![(ids[0], half), (ids[2], Rect::new(0.5, 0.0, 0.5, 1.0))],
                gap: 8.0,
                strict: true,
            },
            &screens,
        );
        assert!(
            matches!(
                result.edit.as_deref(),
                Some(WindowEditResult::Applied { accepted: true, .. })
            ),
            "{:?}",
            result.message
        );
        let expected =
            crate::api::window_layout::placed_rect(area, half, 8.0 * screens[index].scale);
        assert_eq!(
            grouped.snapshot(ids[0], &screens).unwrap().info.bounds,
            expected
        );
        assert_eq!(
            super::super::accessibility::window_bounds(second.hwnd).unwrap(),
            Rect::new(
                expected.x,
                expected.y + header,
                expected.width,
                expected.height - header
            )
        );
        session.execute(
            &mut grouped,
            O::EndEdit {
                transaction: 1,
                commit: true,
            },
            &screens,
        );
        session.execute(&mut grouped, O::Undo, &screens);
        assert_eq!(
            grouped.snapshot(ids[0], &screens).unwrap().info.bounds,
            original_group
        );
        session.execute(&mut grouped, O::Redo, &screens);
        assert_eq!(
            grouped.snapshot(ids[0], &screens).unwrap().info.bounds,
            expected
        );
        let maximized = grouped.cycle_state(ids[0], &screens, &|| false).unwrap();
        assert!(maximized.maximized);
        assert_eq!(maximized.bounds, area);
        grouped
            .tab_operation(TabOperation::Dissolve, &screens, &|| false)
            .unwrap();
        assert_eq!(
            super::super::accessibility::window_bounds(second.hwnd).unwrap(),
            area
        );
        grouped
            .tab_operation(TabOperation::Undo, &screens, &|| false)
            .unwrap();
        assert_eq!(
            grouped.snapshot(ids[0], &screens).unwrap().info.bounds,
            area
        );
        assert_eq!(
            super::super::accessibility::window_bounds(second.hwnd)
                .unwrap()
                .y,
            area.y + header
        );
        let inactive_placement = rect(read_placement(first.hwnd).unwrap().rcNormalPosition);
        // SAFETY: simulate the minimize button only on this test's own window.
        unsafe {
            let _ = ShowWindowAsync(second.hwnd, SW_MINIMIZE);
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        while !super::super::native::is_window_iconic(second.hwnd) {
            super::super::window_tabs::NativeTabs::dispatch_messages();
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        // The hook excludes our own process; deliver the observed completion
        // through its real callback entry after the OS has minimized the probe.
        super::super::window_tabs::handle_window_event(
            windows::Win32::UI::WindowsAndMessaging::EVENT_SYSTEM_MINIMIZEEND,
            second.hwnd,
            0,
        );
        while !super::super::native::is_window_iconic(first.hwnd)
            || !super::super::native::is_window_iconic(second.hwnd)
        {
            grouped.pump(&screens, &|| false).unwrap();
            assert!(
                Instant::now() < deadline,
                "minimize did not reach the whole group"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(!super::super::native::is_window_iconic(third.hwnd));
        assert_eq!(
            rect(read_placement(first.hwnd).unwrap().rcNormalPosition),
            inactive_placement
        );
        // Foreground permission is independent of restoring the selected tab.
        let _ = grouped.activate_window(ids[0], &screens, &|| false);
        assert!(!grouped.snapshot(ids[0], &screens).unwrap().info.minimized);
        grouped.shutdown();
    }

    #[test]
    #[ignore = "reserves a tab header on a disposable maximized window"]
    fn native_maximized_tab_header_keeps_state_and_restore() {
        let _ = super::super::screens::enable_dpi_awareness();
        let screens = super::super::screens::list_screens().unwrap();
        let screen = screens.iter().find(|s| s.is_primary).unwrap();
        let probe = Probe::observable(screen.work_area.inset(160.0, 140.0));
        let mut native = Windows::default();
        let id = native.retain(probe.hwnd, &screens).unwrap().id;
        let before = native.snapshot(id, &screens).unwrap();
        let maximum = native.cycle_state(id, &screens, &|| false).unwrap();
        assert!(maximum.maximized);
        let height = native.tab_bar_height(screen);
        let bounds = Rect::new(
            screen.work_area.x,
            screen.work_area.y + height,
            screen.work_area.width,
            screen.work_area.height - height,
        );
        let fitted = native
            .tab_fit_frame(id, bounds, &screens, &|| false)
            .unwrap();
        assert!(
            fitted.maximized,
            "reserving header space must preserve maximize state"
        );
        assert_eq!(fitted.bounds, bounds);
        let minimized = native.cycle_state(id, &screens, &|| false).unwrap();
        assert!(minimized.minimized);
        let restored = native.cycle_state(id, &screens, &|| false).unwrap();
        assert!(!restored.minimized && !restored.maximized);
        assert_eq!(restored.bounds, before.info.bounds);
        native.reset();
    }

    #[test]
    #[ignore = "sends drag messages only to disposable test-owned tab strips"]
    fn native_tab_drag_drop_and_cancel() {
        let _ = super::super::screens::enable_dpi_awareness();
        use super::super::window_tabs::NativeTabs;
        use crate::api::window_tabs::{TabBar, TabDrop, TabGroupId, TabNativeEvent, WindowTarget};
        use windows::Win32::UI::WindowsAndMessaging::*;
        let screens = super::super::screens::list_screens().unwrap();
        let area = screens.iter().find(|s| s.is_primary).unwrap().work_area;
        let left = Rect {
            x: area.x + 40.0,
            y: area.y + 100.0,
            width: 300.0,
            height: 250.0,
        };
        let right = Rect {
            x: area.x + 400.0,
            ..left
        };
        let first = Probe::observable(left);
        let second = Probe::observable(right);
        let mut native = Windows::default();
        let a = native.retain(first.hwnd, &screens).unwrap().id;
        let b = native.retain(second.hwnd, &screens).unwrap().id;
        native.tab_screens = screens.clone();
        native.tab_watch(&[a, b]).unwrap();
        native
            .tab_bars(&[
                TabBar {
                    visible: true,
                    group: TabGroupId(1),
                    bounds: left,
                    screen: 0,
                    active: a,
                    tabs: vec![(a, 1, "First".into())],
                },
                TabBar {
                    visible: true,
                    group: TabGroupId(2),
                    bounds: right,
                    screen: 0,
                    active: b,
                    tabs: vec![(b, 2, "Second".into())],
                },
            ])
            .unwrap();
        NativeTabs::dispatch_messages();
        let source = native.tabs.strip_handle(TabGroupId(1));
        let destination = native.tabs.strip_handle(TabGroupId(2));
        // SAFETY: synchronous mouse messages and hit tests address only our own
        // disposable strips. No global cursor movement or input injection occurs.
        unsafe {
            let mut target = POINT { x: 8, y: 8 };
            let _ = windows::Win32::Graphics::Gdi::ClientToScreen(destination, &mut target);
            assert_eq!(WindowFromPoint(target), destination);
            let _ = windows::Win32::Graphics::Gdi::ScreenToClient(source, &mut target);
            let packed =
                |x: i32, y: i32| LPARAM(((y as u16 as u32) << 16 | x as u16 as u32) as isize);
            let send = |message, point| {
                SendMessageW(source, message, Some(WPARAM(0)), Some(point));
            };
            for (x, expected) in [
                (100, WindowTarget::Window(a)),
                (8, WindowTarget::Group(TabGroupId(1))),
            ] {
                native.tab_events();
                send(WM_LBUTTONDOWN, packed(x, 8));
                send(WM_MOUSEMOVE, packed(target.x, target.y));
                send(WM_LBUTTONUP, packed(target.x, target.y));
                assert!(native.tab_events().contains(&TabNativeEvent::Drop(TabDrop {
                    source: expected,
                    target: TabGroupId(2),
                    before: Some(b)
                })));
            }
            for cancel in [WM_CANCELMODE, WM_CAPTURECHANGED, WM_RBUTTONDOWN] {
                send(WM_LBUTTONDOWN, packed(100, 8));
                send(WM_MOUSEMOVE, packed(target.x, target.y));
                send(cancel, LPARAM(0));
                send(WM_LBUTTONUP, packed(target.x, target.y));
                assert!(
                    !native.tab_events().iter().any(|e| matches!(
                        e,
                        TabNativeEvent::Drop(_) | TabNativeEvent::Activate(_)
                    ))
                );
            }
            send(WM_LBUTTONDOWN, packed(100, 8));
            send(WM_MOUSEMOVE, packed(-20, -20));
            send(WM_LBUTTONUP, packed(-20, -20));
            assert!(
                !native
                    .tab_events()
                    .iter()
                    .any(|e| matches!(e, TabNativeEvent::Drop(_) | TabNativeEvent::Activate(_)))
            );
            let mut overflow = TabBar {
                visible: true,
                group: TabGroupId(1),
                bounds: left,
                screen: 0,
                active: a,
                tabs: std::iter::once((a, 1, "First".into()))
                    .chain(
                        (1..7).map(|i| (WindowId(1000 + i), i as u32 + 2, format!("Long tab {i}"))),
                    )
                    .chain(std::iter::once((b, 2, "Last".into())))
                    .collect(),
            };
            native.tab_bars(&[overflow.clone()]).unwrap();
            let wheel = |message, delta: i16| {
                SendMessageW(
                    source,
                    message,
                    Some(WPARAM((delta as u16 as usize) << 16)),
                    Some(LPARAM(0)),
                );
            };
            let x = (64.0 * screens[0].scale).round() as i32;
            wheel(WM_MOUSEWHEEL, -480);
            send(WM_LBUTTONDOWN, packed(x, 8));
            send(WM_LBUTTONUP, packed(x, 8));
            assert!(
                native
                    .tab_events()
                    .contains(&TabNativeEvent::Activate(WindowId(1002)))
            );
            overflow.tabs[0].2 = "Changed caption".into();
            native.tab_bars(&[overflow.clone()]).unwrap();
            send(WM_LBUTTONDOWN, packed(x, 8));
            send(WM_LBUTTONUP, packed(x, 8));
            assert!(
                native
                    .tab_events()
                    .contains(&TabNativeEvent::Activate(WindowId(1002)))
            );
            wheel(WM_MOUSEHWHEEL, -480);
            send(WM_LBUTTONDOWN, packed(x, 8));
            send(WM_LBUTTONUP, packed(x, 8));
            assert!(native.tab_events().contains(&TabNativeEvent::Activate(a)));
            overflow.active = b;
            native.tab_bars(&[overflow]).unwrap();
            send(WM_LBUTTONDOWN, packed(240, 8));
            send(WM_LBUTTONUP, packed(240, 8));
            assert!(
                native.tab_events().contains(&TabNativeEvent::Activate(b)),
                "selection must reveal an off-screen tab"
            );
        }
        native.reset();
    }

    #[test]
    #[ignore = "launches only disposable child-process windows to verify compositor visibility"]
    fn native_cross_process_tabs_keep_composed_content() {
        use std::process::{Child, Command, Stdio};
        struct ChildWindows {
            child: Child,
            path: std::path::PathBuf,
        }
        impl Drop for ChildWindows {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.path);
                let deadline = Instant::now() + Duration::from_secs(3);
                while self.child.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
                    super::super::window_tabs::NativeTabs::dispatch_messages();
                    std::thread::sleep(Duration::from_millis(2));
                }
                if self.child.try_wait().ok().flatten().is_none() {
                    let _ = self.child.kill();
                }
                let _ = self.child.wait();
            }
        }
        let path = std::env::temp_dir().join(format!(
            "keysteer-opacity-probe-{}-{}.txt",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "platform::windows::window_manager::tests::native_tabs_child_windows",
                "--ignored",
                "--test-threads=1",
            ])
            .env("KEYSTEER_TABS_TEST_HANDLES", &path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let _child = ChildWindows {
            child,
            path: path.clone(),
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        let handles = loop {
            let values: Vec<usize> = std::fs::read_to_string(&path)
                .unwrap_or_default()
                .split_whitespace()
                .filter_map(|v| v.parse().ok())
                .collect();
            if values.len() == 2 {
                break values
                    .into_iter()
                    .map(|raw| HWND(raw as *mut _))
                    .collect::<Vec<_>>();
            }
            assert!(Instant::now() < deadline, "child windows were not ready");
            std::thread::sleep(Duration::from_millis(2));
        };
        let screens = super::super::screens::list_screens().unwrap();
        let mut native = Windows::default();
        native.tab_screens = screens.clone();
        let ids: Vec<_> = handles
            .iter()
            .map(|hwnd| native.retain(*hwnd, &screens).unwrap().id)
            .collect();
        // SAFETY: read only the counter maintained by our disposable child class.
        let show_events = |hwnd| unsafe {
            if !tab_window_visible(hwnd) {
                let bounds = read_bounds(hwnd).unwrap();
                assert_ne!(
                    windows::Win32::UI::WindowsAndMessaging::WindowFromPoint(POINT {
                        x: (bounds.x + bounds.width / 2.0) as i32,
                        y: (bounds.y + bounds.height / 2.0) as i32,
                    }),
                    hwnd,
                    "transparent inactive windows must not intercept mouse input"
                );
            }
            GetPropW(hwnd, windows::core::w!("KeySteer.Probe.ShowEvents")).0 as usize
        };
        let before: Vec<_> = handles.iter().map(|hwnd| show_events(*hwnd)).collect();
        for _ in 0..12 {
            for (id, hwnd) in ids.iter().zip(&handles) {
                native.tab_set_hidden(*id, true).unwrap();
                assert!(
                    super::super::native::is_window_visible(*hwnd),
                    "opacity switching must retain WS_VISIBLE"
                );
                assert!(!tab_window_visible(*hwnd));
                assert_eq!(
                    show_events(*hwnd),
                    before[ids.iter().position(|v| v == id).unwrap()]
                );
                native.tab_set_hidden(*id, false).unwrap();
                assert!(tab_window_visible(*hwnd));
            }
        }
        assert_eq!(
            handles
                .iter()
                .map(|hwnd| show_events(*hwnd))
                .collect::<Vec<_>>(),
            before,
            "tab switching must not trigger fresh WM_SHOWWINDOW cycles"
        );
        let original = native.snapshot(ids[0], &screens).unwrap();
        native.minimize(ids[0], &screens, &|| false).unwrap();
        assert!(super::super::native::is_window_iconic(handles[0]));
        assert!(
            eligible(handles[0]),
            "minimized windows must remain inventory candidates"
        );
        assert!(
            super::super::accessibility::scannable_target(handles[0]).is_none(),
            "UI scanning must still exclude minimized windows"
        );
        let inventory = native.enumerate(&screens, &|| false).unwrap();
        let minimized = inventory.iter().find(|window| window.id == ids[0]).unwrap();
        assert!(minimized.minimized);
        assert_eq!(minimized.screen, original.info.screen);
        native.restore(&original, &screens, &|| false).unwrap();
        native.tab_set_hidden(ids[0], true).unwrap();
        native.reset();
        assert!(handles.iter().all(|hwnd| tab_window_visible(*hwnd)
            && super::super::native::window_long(*hwnd, GWL_EXSTYLE) as u32 & WS_EX_LAYERED.0
                == 0));
    }

    #[test]
    #[ignore = "child helper for the explicit disposable tab-group acceptance test"]
    fn native_tabs_child_windows() {
        let Ok(path) = std::env::var("KEYSTEER_TABS_TEST_HANDLES") else {
            return;
        };
        let _ = super::super::screens::enable_dpi_awareness();
        let screens = super::super::screens::list_screens().unwrap();
        let area = screens.iter().find(|s| s.is_primary).unwrap().work_area;
        let first = Probe::with_style(area.inset(70.0, 70.0), Default::default());
        let second = Probe::with_style(area.inset(100.0, 100.0), Default::default());
        std::fs::write(
            &path,
            format!("{} {}", first.hwnd.0 as usize, second.hwnd.0 as usize),
        )
        .unwrap();
        if let Some(pid) = std::env::var("KEYSTEER_TABS_TEST_PARENT")
            .ok()
            .and_then(|v| v.parse().ok())
        {
            // SAFETY: explicitly grants foreground permission to this test's
            // parent only; all target windows are disposable child-owned HWNDs.
            unsafe {
                let _ = windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(pid);
            }
        }
        let deadline = Instant::now() + Duration::from_secs(40);
        while std::path::Path::new(&path).exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    #[ignore = "creates an owned disposable native window; run explicitly on Windows"]
    fn native_close_request_only_closes_owned_target() {
        let _ = super::super::screens::enable_dpi_awareness();
        let screens = super::super::screens::list_screens().unwrap();
        let area = screens.iter().find(|s| s.is_primary).unwrap().work_area;
        let first = Probe::start(Rect::new(area.x + 40.0, area.y + 40.0, 480.0, 320.0));
        let second = Probe::start(Rect::new(area.x + 80.0, area.y + 80.0, 480.0, 320.0));
        let mut access = Windows::default();
        let id = access.retain(first.hwnd, &screens).unwrap().id;
        access.close(id).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while super::super::native::is_window(first.hwnd) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!super::super::native::is_window(first.hwnd));
        assert!(super::super::native::is_window(second.hwnd));
        assert!(
            access.close(id).is_err(),
            "stale target must not close another window"
        );
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

    #[test]
    #[ignore = "creates only disposable windows to verify lazy tab placement and independent lifetimes"]
    fn native_active_only_tabs_move_switch_dissolve_and_close() {
        let shown = tab_window_visible;
        use crate::api::window_tabs::{TabNativeEvent, TabOperation, WindowTarget};
        use crate::platform::common::WindowGroupsProbe as Grouped;
        use std::cell::{Cell, RefCell};
        use std::rc::Rc;
        use windows::Win32::UI::WindowsAndMessaging::*;
        struct OwnedAccess {
            native: Windows,
            ids: Vec<WindowId>,
            events: Rc<RefCell<Vec<TabNativeEvent>>>,
            selected: Cell<Option<WindowId>>,
            strip: Rc<Cell<HWND>>,
        }
        impl WindowAccess for OwnedAccess {
            fn acquire(&mut self, _: Point, s: &[Screen]) -> Result<Option<WindowInfo>, String> {
                self.native.snapshot(self.ids[0], s).map(|v| Some(v.info))
            }
            fn enumerate(
                &mut self,
                s: &[Screen],
                _: &dyn Fn() -> bool,
            ) -> Result<Vec<WindowInfo>, String> {
                Ok(self
                    .ids
                    .iter()
                    .filter_map(|id| self.native.snapshot(*id, s).ok().map(|v| v.info))
                    .collect())
            }
            fn snapshot(&self, id: WindowId, s: &[Screen]) -> Result<Snapshot, String> {
                self.native.snapshot(id, s)
            }
            fn set_frame(
                &mut self,
                id: WindowId,
                r: Rect,
                s: &[Screen],
                c: &dyn Fn() -> bool,
            ) -> Result<WindowInfo, String> {
                self.native.set_frame(id, r, s, c)
            }
            fn restore(
                &mut self,
                v: &Snapshot,
                s: &[Screen],
                c: &dyn Fn() -> bool,
            ) -> Result<WindowInfo, String> {
                self.native.restore(v, s, c)
            }
            fn cycle_state(
                &mut self,
                id: WindowId,
                s: &[Screen],
                c: &dyn Fn() -> bool,
            ) -> Result<WindowInfo, String> {
                self.native.cycle_state(id, s, c)
            }
            fn select(&self, id: WindowId) -> Result<(), String> {
                self.selected.set(Some(id));
                Ok(())
            }
            fn tab_selected(&self, id: WindowId) -> bool {
                self.selected.get() == Some(id)
            }
            fn pointer(&self) -> Result<Point, String> {
                Ok(Point::default())
            }
            fn tab_set_hidden(&mut self, id: WindowId, hidden: bool) -> Result<(), String> {
                self.native.tab_set_hidden(id, hidden)
            }
            fn tab_watch(&mut self, ids: &[WindowId]) -> Result<(), String> {
                self.native.tab_watch(ids)
            }
            fn tab_bars(&mut self, bars: &[crate::api::window_tabs::TabBar]) -> Result<(), String> {
                self.native.tab_bars(bars)?;
                self.strip.set(bars.first().map_or(HWND::default(), |bar| {
                    self.native.tabs.strip_handle(bar.group)
                }));
                Ok(())
            }
            fn tab_events(&mut self) -> Vec<TabNativeEvent> {
                let mut events = std::mem::take(&mut *self.events.borrow_mut());
                events.extend(self.native.tab_events());
                events
            }
            fn tab_visible(&self, id: WindowId) -> bool {
                self.native.tab_visible(id)
            }
            fn reset(&mut self) {
                self.native.reset();
            }
        }
        let _ = super::super::screens::enable_dpi_awareness();
        let screens = super::super::screens::list_screens().unwrap();
        let area = screens.iter().find(|s| s.is_primary).unwrap().work_area;
        let first = Probe::observable(area.inset(50.0, 60.0));
        let second = Probe::observable(area.inset(140.0, 120.0));
        let third = Probe::observable(area.inset(230.0, 160.0));
        let visible_mutations = |hwnd| {
            let mut count = 0;
            // SAFETY: bounded query handled by this test's disposable class.
            let result = unsafe {
                SendMessageTimeoutW(
                    hwnd,
                    PROBE_VISIBLE_MUTATIONS,
                    WPARAM(0),
                    LPARAM(0),
                    SMTO_ABORTIFHUNG | SMTO_BLOCK,
                    100,
                    Some(&mut count),
                )
            };
            assert_ne!(result.0, 0);
            count
        };
        let first_mutations = visible_mutations(first.hwnd);
        let third_mutations = visible_mutations(third.hwnd);
        let mut native = Windows::default();
        native.tab_screens = screens.clone();
        let ids: Vec<_> = [first.hwnd, second.hwnd, third.hwnd]
            .into_iter()
            .map(|hwnd| native.retain(hwnd, &screens).unwrap().id)
            .collect();
        let originals: Vec<_> = ids
            .iter()
            .map(|id| native.snapshot(*id, &screens).unwrap())
            .collect();
        let styles: Vec<_> = [first.hwnd, second.hwnd, third.hwnd]
            .map(|hwnd| super::super::native::window_long(hwnd, GWL_STYLE))
            .into_iter()
            .collect();
        let events = Rc::new(RefCell::new(Vec::new()));
        let strip_handle = Rc::new(Cell::new(HWND::default()));
        let mut grouped = Grouped::new(OwnedAccess {
            native,
            ids: ids.clone(),
            events: events.clone(),
            selected: Cell::new(None),
            strip: strip_handle.clone(),
        });
        let choose = |grouped: &mut Grouped<OwnedAccess>, id| {
            grouped
                .tab_operation(
                    TabOperation::Choose(WindowTarget::Window(id)),
                    &screens,
                    &|| false,
                )
                .unwrap()
        };
        choose(&mut grouped, ids[0]);
        choose(&mut grouped, ids[1]);
        assert!(!shown(first.hwnd));
        assert!(shown(second.hwnd));
        let hidden_original = super::super::accessibility::window_bounds(first.hwnd).unwrap();
        let mut moved = originals[0].info.bounds;
        for step in 0..40 {
            moved.x = originals[0].info.bounds.x + step as f64 * 3.0;
            grouped
                .set_frame(ids[1], moved, &screens, &|| false)
                .unwrap();
            events.borrow_mut().push(TabNativeEvent::Changed(ids[1]));
            grouped.pump(&screens, &|| false).unwrap();
            assert_eq!(
                super::super::accessibility::window_bounds(first.hwnd).unwrap(),
                hidden_original
            );
        }
        // Deliver a real geometry change to the native callback directly. The
        // strip must already be at the new location before the model is pumped.
        let mut raw = read_bounds(second.hwnd).unwrap();
        raw.x += 19.0;
        submit_frame(second.hwnd, raw).unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        while (read_bounds(second.hwnd).unwrap().x - raw.x).abs() > 1.0 {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        moved = super::super::accessibility::window_bounds(second.hwnd).unwrap();
        super::super::window_tabs::handle_window_event(EVENT_OBJECT_LOCATIONCHANGE, second.hwnd, 0);
        let strip_bounds = read_bounds(strip_handle.get()).unwrap();
        assert!((strip_bounds.x - moved.x).abs() <= 1.0);
        assert!((strip_bounds.bottom() - moved.y).abs() <= 1.0);
        assert_eq!(
            super::super::accessibility::window_bounds(first.hwnd).unwrap(),
            hidden_original
        );
        grouped.pump(&screens, &|| false).unwrap();
        grouped
            .activate_window(ids[0], &screens, &|| false)
            .unwrap();
        assert!(shown(first.hwnd));
        assert!(!shown(second.hwnd));
        assert_eq!(
            visible_mutations(first.hwnd),
            first_mutations,
            "the new tab moved while visible"
        );
        assert_eq!(
            super::super::accessibility::window_bounds(first.hwnd).unwrap(),
            moved
        );
        // Reenter and append after a drag: the new member takes the current frame.
        grouped.reset();
        choose(&mut grouped, ids[0]);
        choose(&mut grouped, ids[2]);
        assert_eq!(
            super::super::accessibility::window_bounds(third.hwnd).unwrap(),
            moved
        );
        assert_eq!(
            visible_mutations(third.hwnd),
            third_mutations,
            "append moved an incoming window while visible"
        );
        for (index, hwnd) in [first.hwnd, second.hwnd, third.hwnd]
            .into_iter()
            .enumerate()
        {
            // SAFETY: inspect only our disposable windows and their original native styles.
            unsafe {
                let strip = strip_handle.get();
                assert_eq!(GetWindow(strip, GW_OWNER).unwrap(), third.hwnd);
                assert_eq!(
                    GetWindowLongPtrW(strip, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST.0,
                    0
                );
                assert_ne!(
                    GetClassLongPtrW(strip, GCLP_HCURSOR),
                    0,
                    "tab strip must supply its arrow cursor"
                );
                assert_eq!(
                    SendMessageW(
                        strip,
                        WM_SETCURSOR,
                        Some(WPARAM(strip.0 as usize)),
                        Some(LPARAM(HTCLIENT as isize))
                    )
                    .0,
                    1
                );
                assert!(GetParent(hwnd).unwrap_or_default().0.is_null());
                assert!(
                    windows::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled(hwnd).as_bool()
                );
            }
            let actual = super::super::native::window_long(hwnd, GWL_STYLE);
            assert_eq!(
                actual as u32 & !WS_VISIBLE.0,
                styles[index] as u32 & !WS_VISIBLE.0
            );
        }
        // Closing the independent strip only dissolves the group.
        // SAFETY: find and post to the test-owned strip on this worker; no application close message.
        unsafe {
            let strip = strip_handle.get();
            PostMessageW(Some(strip), WM_CLOSE, WPARAM(0), LPARAM(0)).unwrap();
        }
        grouped.pump(&screens, &|| false).unwrap();
        assert!(!grouped.persistent());
        assert!(
            [first.hwnd, second.hwnd, third.hwnd]
                .iter()
                .all(|hwnd| shown(*hwnd))
        );
        grouped.cycle_state(ids[0], &screens, &|| false).unwrap();
        choose(&mut grouped, ids[0]);
        choose(&mut grouped, ids[1]);
        grouped
            .set_frame(ids[1], moved, &screens, &|| false)
            .unwrap();
        let before_max_switch = visible_mutations(first.hwnd);
        grouped
            .activate_window(ids[0], &screens, &|| false)
            .unwrap();
        assert_eq!(
            visible_mutations(first.hwnd),
            before_max_switch,
            "restoring a hidden maximized tab moved it while visible"
        );
        assert_eq!(
            super::super::accessibility::window_bounds(first.hwnd).unwrap(),
            moved
        );
        grouped
            .activate_window(ids[1], &screens, &|| false)
            .unwrap();
        drop(second);
        events.borrow_mut().push(TabNativeEvent::Closed(ids[1]));
        grouped.pump(&screens, &|| false).unwrap();
        assert!(!grouped.persistent());
        assert!(shown(first.hwnd));
        grouped.shutdown();
        assert!(shown(third.hwnd));
    }
}
