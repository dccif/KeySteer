//! A small independent tool window; it never owns an application window.
use super::enqueue;
use crate::api::window_tabs::{TabBar, TabDrop, TabNativeEvent, WindowTarget};
use crate::api::{Rect, Screen};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::OnceLock;
use windows::Win32::Foundation::{
    COLORREF, GetLastError, HWND, LPARAM, LRESULT, POINT, RECT, SetLastError, WIN32_ERROR, WPARAM,
};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetCapture, ReleaseCapture, SetCapture};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::w;

#[derive(Clone, Copy, PartialEq, Eq)]
struct Position {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    visible: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Hit {
    Target(WindowTarget),
    Close,
}
#[derive(Clone, Copy)]
struct Press {
    hit: Hit,
    start: (i32, i32),
    dragging: bool,
}

pub(super) struct Strip {
    hwnd: Cell<HWND>,
    data: RefCell<TabBar>,
    titles: RefCell<Vec<Vec<u16>>>,
    heading: RefCell<Vec<u16>>,
    background: HBRUSH,
    selected: HBRUSH,
    font: Cell<HFONT>,
    scale: Cell<f64>,
    canvas: Cell<HDC>,
    bitmap: Cell<HBITMAP>,
    stock_bitmap: Cell<HGDIOBJ>,
    canvas_size: Cell<(i32, i32)>,
    position: Cell<Option<Position>>,
    dirty: Cell<bool>,
    press: Cell<Option<Press>>,
    drop_marker: Cell<Option<i32>>,
    owner: Cell<HWND>,
    scroll: Cell<i32>,
    wheel_remainder: Cell<f64>,
}
impl Strip {
    #[cfg(test)]
    pub fn handle(&self) -> HWND {
        self.hwnd.get()
    }
    pub fn new(data: &TabBar, owner: HWND) -> Result<Rc<Self>, String> {
        static CLASS: OnceLock<Result<(), String>> = OnceLock::new();
        CLASS
            .get_or_init(|| {
                // SAFETY: shared system cursor is borrowed; callback/name remain live.
                let registered = unsafe {
                    let class = WNDCLASSW {
                        lpfnWndProc: Some(procedure),
                        lpszClassName: w!("KeySteerTabStrip"),
                        hCursor: LoadCursorW(None, IDC_ARROW).map_err(|e| e.to_string())?,
                        ..Default::default()
                    };
                    RegisterClassW(&class)
                };
                if registered == 0 {
                    Err("Cannot register tab strip".into())
                } else {
                    Ok(())
                }
            })
            .clone()?;
        // SAFETY: fixed colors create resources owned by this strip.
        let (background, selected) = unsafe {
            (
                CreateSolidBrush(COLORREF(0x00251f1b)),
                CreateSolidBrush(COLORREF(0x00584636)),
            )
        };
        let strip = Rc::new(Self {
            hwnd: Cell::new(HWND::default()),
            data: RefCell::new(data.clone()),
            titles: RefCell::new(Vec::new()),
            heading: RefCell::new(format!("~{}", data.group.0).encode_utf16().collect()),
            background,
            selected,
            font: Cell::new(HFONT::default()),
            scale: Cell::new(0.0),
            canvas: Cell::new(HDC::default()),
            bitmap: Cell::new(HBITMAP::default()),
            stock_bitmap: Cell::new(HGDIOBJ::default()),
            canvas_size: Cell::new((0, 0)),
            position: Cell::new(None),
            dirty: Cell::new(true),
            press: Cell::new(None),
            drop_marker: Cell::new(None),
            owner: Cell::new(owner),
            scroll: Cell::new(0),
            wheel_remainder: Cell::new(0.0),
        });
        // SAFETY: Rc retains a stable address through DestroyWindow on this thread.
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                w!("KeySteerTabStrip"),
                w!("KeySteer Tabs"),
                WS_POPUP,
                0,
                0,
                1,
                1,
                Some(owner),
                None,
                None,
                Some((&*strip as *const Self).cast()),
            )
        }
        .map_err(|e| format!("Cannot create tab strip: {e}"))?;
        strip.hwnd.set(hwnd);
        Ok(strip)
    }
    pub fn alive(&self) -> bool {
        !self.hwnd.get().0.is_null()
    }
    pub fn contains(&self, id: crate::api::window::WindowId) -> bool {
        self.data
            .borrow()
            .tabs
            .iter()
            .any(|(member, _, _)| *member == id)
    }
    pub fn set_owner(&self, owner: HWND) -> Result<(), String> {
        if self.owner.get() == owner || !self.alive() {
            return Ok(());
        }
        // SAFETY: change only OUR popup's owner, never an application window's
        // parent/style. Both handles are borrowed on this worker; no pointers escape.
        unsafe {
            SetLastError(WIN32_ERROR(0));
            if SetWindowLongPtrW(self.hwnd.get(), GWLP_HWNDPARENT, owner.0 as isize) == 0
                && GetLastError() != WIN32_ERROR(0)
            {
                return Err("Cannot associate the tab strip with its active window".into());
            }
        }
        self.owner.set(owner);
        self.position.set(None);
        Ok(())
    }
    fn update_scale(&self, scale: f64) {
        if self.scale.get() != scale {
            let font = LOGFONTW {
                lfHeight: -(14.0 * scale).round() as i32,
                lfWeight: 400,
                ..Default::default()
            };
            // SAFETY: initialized font descriptor; replace and release the owned font.
            unsafe {
                let previous = self.font.replace(CreateFontIndirectW(&font));
                if !previous.0.is_null() {
                    let _ = DeleteObject(previous.into());
                }
            }
            self.scale.set(scale);
        }
    }
    pub fn update(&self, data: &TabBar, screen: Option<&Screen>) -> Result<(), String> {
        let scale = screen.map_or(1.0, |screen| screen.scale);
        let reveal_active = {
            let current = self.data.borrow();
            self.scale.get() != scale
                || self.titles.borrow().is_empty()
                || current.active != data.active
                || current.bounds.width != data.bounds.width
                || !current
                    .tabs
                    .iter()
                    .map(|(id, _, _)| id)
                    .eq(data.tabs.iter().map(|(id, _, _)| id))
        };
        let repaint = {
            let current = self.data.borrow();
            self.scale.get() != scale
                || self.titles.borrow().is_empty()
                || current.tabs != data.tabs
                || current.active != data.active
                || current.bounds.width != data.bounds.width
        };
        self.update_scale(scale);
        {
            let mut current = self.data.borrow_mut();
            if current.tabs != data.tabs || self.titles.borrow().is_empty() {
                *self.titles.borrow_mut() = data
                    .tabs
                    .iter()
                    .map(|(_, n, title)| {
                        if *n == 0 {
                            title.clone()
                        } else {
                            format!("{n}  {title}")
                        }
                        .encode_utf16()
                        .collect()
                    })
                    .collect();
                current.tabs.clone_from(&data.tabs);
            }
            current.bounds = data.bounds;
            current.active = data.active;
            current.visible = data.visible;
            current.screen = data.screen;
        }
        let (_, end, width) = self.tab_layout(data.bounds.width.round() as i32);
        let heading = (44.0 * self.scale.get()).round() as i32;
        let viewport = (end - heading).max(1);
        let maximum = (width * data.tabs.len() as i32 - viewport).max(0);
        let mut scroll = self.scroll.get().min(maximum);
        if reveal_active
            && let Some(index) = data.tabs.iter().position(|(id, _, _)| *id == data.active)
        {
            let left = index as i32 * width;
            if left < scroll {
                scroll = left;
            }
            if left + width > scroll + viewport {
                scroll = left + width - viewport;
            }
        }
        self.scroll.set(scroll.clamp(0, maximum));
        self.position(data.bounds, data.visible, screen, repaint)
    }
    fn tab_layout(&self, total: i32) -> (i32, i32, i32) {
        let heading = (44.0 * self.scale.get()).round() as i32;
        let end = total - 30;
        let count = self.data.borrow().tabs.len().max(1) as i32;
        let width = ((end - heading).max(1) / count).max((140.0 * self.scale.get()).round() as i32);
        (heading, end, width.max(1))
    }
    fn scroll_tabs(&self, delta: i32, horizontal: bool) {
        let Some(position) = self.position.get() else {
            return;
        };
        let (heading, end, width) = self.tab_layout(position.width);
        let maximum =
            (width * self.data.borrow().tabs.len() as i32 - (end - heading).max(1)).max(0);
        let amount = self.wheel_remainder.get()
            + delta as f64 / 120.0 * 90.0 * self.scale.get() * if horizontal { 1.0 } else { -1.0 };
        let pixels = amount.trunc() as i32;
        self.wheel_remainder.set(amount - pixels as f64);
        let next = (self.scroll.get() + pixels).clamp(0, maximum);
        if self.scroll.replace(next) != next {
            self.invalidate();
        }
    }
    fn hit(&self, x: i32, y: i32) -> Option<Hit> {
        let position = self.position.get()?;
        if x < 0 || y < 0 || x >= position.width || y >= position.height {
            return None;
        }
        let data = self.data.borrow();
        if x >= position.width - 30 {
            return Some(Hit::Close);
        }
        let (heading, _, width) = self.tab_layout(position.width);
        if x < heading {
            return Some(Hit::Target(WindowTarget::Group(data.group)));
        }
        data.tabs
            .get(((x - heading + self.scroll.get()) / width) as usize)
            .map(|(id, _, _)| Hit::Target(WindowTarget::Window(*id)))
    }
    pub fn drop_at(&self, source: WindowTarget, point: POINT) -> Option<(TabDrop, i32)> {
        let position = self.position.get()?;
        let x = point.x - position.x;
        let y = point.y - position.y;
        if !position.visible || x < 0 || y < 0 || x >= position.width - 30 || y >= position.height {
            return None;
        }
        let data = self.data.borrow();
        if source == WindowTarget::Group(data.group) {
            return None;
        }
        let (heading, end, width) = self.tab_layout(position.width);
        let index = (((x - heading).max(0) + self.scroll.get() + width / 2) / width) as usize;
        let index = index.min(data.tabs.len());
        Some((
            TabDrop {
                source,
                target: data.group,
                before: data.tabs.get(index).map(|(id, _, _)| *id),
            },
            (heading + width * index as i32 - self.scroll.get()).clamp(heading, end),
        ))
    }
    pub fn is_handle(&self, hwnd: HWND) -> bool {
        self.hwnd.get() == hwnd
    }
    pub fn mark_drop(&self, marker: Option<i32>) {
        if self.drop_marker.replace(marker) != marker {
            self.invalidate();
        }
    }
    fn invalidate(&self) {
        self.dirty.set(true);
        // SAFETY: invalidate only this live, owned strip; no foreign messages.
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd.get()), None, false);
        }
    }
    /// Native geometry path: no model snapshots, title allocation or inactive
    /// application writes. Resize repaints only the small existing strip.
    pub fn follow(&self, hwnd: HWND, screens: &[Screen]) -> bool {
        if !super::super::window_manager::tab_window_visible(hwnd) {
            return false;
        }
        let Some(bounds) = super::super::accessibility::window_bounds(hwnd) else {
            return false;
        };
        let index = crate::platform::common::window_geometry::screen_index(screens, bounds);
        let screen = index.and_then(|index| screens.get(index));
        let scale = screen.map_or(1.0, |s| s.scale);
        let Ok(mut data) = self.data.try_borrow_mut() else {
            return false;
        };
        if !data.visible {
            return false;
        }
        let repaint = data.bounds.width != bounds.width || self.scale.get() != scale;
        data.bounds = bounds;
        if let Some(index) = index {
            data.screen = index;
        }
        drop(data);
        self.update_scale(scale);
        if repaint {
            let (heading, end, width) = self.tab_layout(bounds.width.round() as i32);
            let viewport = (end - heading).max(1);
            let data = self.data.borrow();
            let maximum = (width * data.tabs.len() as i32 - viewport).max(0);
            let mut scroll = self.scroll.get().min(maximum);
            if let Some(index) = data.tabs.iter().position(|(id, _, _)| *id == data.active) {
                let left = index as i32 * width;
                if left < scroll {
                    scroll = left;
                }
                if left + width > scroll + viewport {
                    scroll = left + width - viewport;
                }
            }
            self.scroll.set(scroll.clamp(0, maximum));
        }
        self.position(bounds, true, screen, repaint).is_ok()
    }
    fn position(
        &self,
        bounds: Rect,
        visible: bool,
        screen: Option<&Screen>,
        repaint: bool,
    ) -> Result<(), String> {
        let height = (30.0 * self.scale.get()).round() as i32;
        let above = bounds.y - height as f64;
        let y = above;
        // Never cover application content while a maximize notification is
        // pending. The worker reserves headroom, then this strip reappears above.
        let visible = visible && screen.is_none_or(|s| above >= s.work_area.y - 1.0);
        let position = Position {
            x: bounds.x.round() as i32,
            y: y.round() as i32,
            width: bounds.width.round().max(1.0) as i32,
            height,
            visible,
        };
        if repaint {
            self.dirty.set(true);
        }
        // SAFETY: only the owned strip is moved or hidden; no foreign ownership changes.
        unsafe {
            if self.position.get() != Some(position) {
                SetWindowPos(
                    self.hwnd.get(),
                    None,
                    position.x,
                    position.y,
                    position.width,
                    height,
                    SWP_NOACTIVATE
                        | SWP_NOZORDER
                        | if visible {
                            SWP_SHOWWINDOW
                        } else {
                            SWP_HIDEWINDOW
                        },
                )
                .map_err(|e| format!("Cannot position tab strip: {e}"))?;
                self.position.set(Some(position));
            }
            if repaint {
                self.dirty.set(true);
                let _ = InvalidateRect(Some(self.hwnd.get()), None, false);
            }
        }
        Ok(())
    }
    fn paint(&self) {
        let mut paint = PAINTSTRUCT::default();
        let mut rect = RECT::default();
        // SAFETY: paired paint lifecycle, initialized outputs and owned GDI resources.
        unsafe {
            let target = BeginPaint(self.hwnd.get(), &mut paint);
            let _ = GetClientRect(self.hwnd.get(), &mut rect);
            // Reuse a strip-sized back buffer. Pure movement never invalidates
            // content; selection/resize paints once, then copies the completed bar.
            let size = (rect.right.max(1), rect.bottom.max(1));
            if self.canvas.get().0.is_null() {
                self.canvas.set(CreateCompatibleDC(Some(target)));
            }
            if !self.canvas.get().0.is_null()
                && (self.canvas_size.get().0 < size.0 || self.canvas_size.get().1 < size.1)
            {
                // Capacity grows in small width buckets; shrinking reuses the
                // buffer, bounded by the largest strip seen during its lifetime.
                let capacity = (
                    size.0.saturating_add(255) / 256 * 256,
                    size.1.saturating_add(15) / 16 * 16,
                );
                let bitmap = CreateCompatibleBitmap(target, capacity.0, capacity.1);
                if !bitmap.0.is_null() {
                    let previous = SelectObject(self.canvas.get(), bitmap.into());
                    if self.bitmap.get().0.is_null() {
                        self.stock_bitmap.set(previous);
                    } else {
                        let _ = DeleteObject(self.bitmap.get().into());
                    }
                    self.bitmap.set(bitmap);
                    self.canvas_size.set(capacity);
                    self.dirty.set(true);
                }
            }
            let dc = if self.canvas_size.get().0 >= size.0 && self.canvas_size.get().1 >= size.1 {
                self.canvas.get()
            } else {
                target
            };
            if dc == target || self.dirty.replace(false) {
                FillRect(dc, &rect, self.background);
                let old = SelectObject(dc, self.font.get().into());
                SetBkMode(dc, TRANSPARENT);
                SetTextColor(dc, COLORREF(0x00f5f2ef));
                let data = self.data.borrow();
                let mut titles = self.titles.borrow_mut();
                let heading_width = (44.0 * self.scale.get()).round() as i32;
                let mut heading_rect = RECT {
                    right: heading_width,
                    ..rect
                };
                DrawTextW(
                    dc,
                    &mut self.heading.borrow_mut(),
                    &mut heading_rect,
                    DT_SINGLELINE | DT_CENTER | DT_VCENTER,
                );
                let (_, end, width) = self.tab_layout(rect.right);
                let clip = SaveDC(dc);
                IntersectClipRect(dc, heading_width, 0, end, rect.bottom);
                for (i, (id, _, _)) in data.tabs.iter().enumerate() {
                    let mut cell = RECT {
                        left: heading_width + i as i32 * width - self.scroll.get(),
                        right: heading_width + (i as i32 + 1) * width - self.scroll.get(),
                        ..rect
                    };
                    if cell.right <= heading_width || cell.left >= end {
                        continue;
                    }
                    if *id == data.active {
                        FillRect(dc, &cell, self.selected);
                    }
                    cell.left += 8;
                    if let Some(title) = titles.get_mut(i) {
                        DrawTextW(
                            dc,
                            title,
                            &mut cell,
                            DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX,
                        );
                    }
                }
                let _ = RestoreDC(dc, clip);
                let mut close = RECT {
                    left: rect.right - 28,
                    ..rect
                };
                DrawTextW(
                    dc,
                    &mut [0x00d7],
                    &mut close,
                    DT_SINGLELINE | DT_CENTER | DT_VCENTER,
                );
                if let Some(x) = self.drop_marker.get() {
                    SetDCBrushColor(dc, COLORREF(0x006fdfff));
                    FillRect(
                        dc,
                        &RECT {
                            left: x - 2,
                            right: x + 2,
                            top: 2,
                            bottom: rect.bottom - 2,
                        },
                        HBRUSH(GetStockObject(DC_BRUSH).0),
                    );
                }
                SelectObject(dc, old);
            }
            if dc != target {
                let _ = BitBlt(target, 0, 0, size.0, size.1, Some(dc), 0, 0, SRCCOPY);
            }
            let _ = EndPaint(self.hwnd.get(), &paint);
        }
    }
}
impl Drop for Strip {
    fn drop(&mut self) {
        // SAFETY: release only this strip and its resources; it has no child windows.
        unsafe {
            if !self.hwnd.get().0.is_null() {
                let _ = DestroyWindow(self.hwnd.get());
            }
            let _ = DeleteObject(self.background.into());
            let _ = DeleteObject(self.selected.into());
            if !self.font.get().0.is_null() {
                let _ = DeleteObject(self.font.get().into());
            }
            if !self.canvas.get().0.is_null() {
                if !self.bitmap.get().0.is_null() {
                    SelectObject(self.canvas.get(), self.stock_bitmap.get());
                    let _ = DeleteObject(self.bitmap.get().into());
                }
                let _ = DeleteDC(self.canvas.get());
            }
        }
    }
}
unsafe extern "system" fn procedure(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: WM_NCCREATE supplies CREATESTRUCT; stable Rc state outlives the HWND.
    unsafe {
        if message == WM_NCCREATE {
            let create = &*(lparam.0 as *const CREATESTRUCTW);
            (&*(create.lpCreateParams as *const Strip)).hwnd.set(hwnd);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
        }
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Strip;
        if !ptr.is_null() {
            let strip = &*ptr;
            match message {
                WM_PAINT => {
                    strip.paint();
                    return LRESULT(0);
                }
                WM_ERASEBKGND => return LRESULT(1),
                WM_SETCURSOR => {
                    if strip.press.get().is_none_or(|press| !press.dragging) {
                        SetCursor(Some(
                            HCURSOR(GetClassLongPtrW(hwnd, GCLP_HCURSOR) as *mut _),
                        ));
                    }
                    return LRESULT(1);
                }
                WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
                WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
                    strip.scroll_tabs(
                        (wparam.0 >> 16) as u16 as i16 as i32,
                        message == WM_MOUSEHWHEEL,
                    );
                    return LRESULT(0);
                }
                WM_CLOSE => {
                    enqueue(TabNativeEvent::Dissolve(strip.data.borrow().group));
                    return LRESULT(0);
                }
                WM_LBUTTONDOWN => {
                    let x = (lparam.0 as u16 as i16) as i32;
                    let y = ((lparam.0 >> 16) as u16 as i16) as i32;
                    if let Some(hit) = strip.hit(x, y) {
                        // A deliberate mouse press activates the owning app so
                        // capture also tracks a drag starting from a background group.
                        let _ = SetForegroundWindow(strip.owner.get());
                        let mut point = POINT { x, y };
                        let _ = ClientToScreen(hwnd, &mut point);
                        strip.press.set(Some(Press {
                            hit,
                            start: (point.x, point.y),
                            dragging: false,
                        }));
                        SetCapture(hwnd);
                    }
                    return LRESULT(0);
                }
                WM_MOUSEMOVE => {
                    if let Some(mut press) = strip.press.get()
                        && let Hit::Target(source) = press.hit
                    {
                        let mut point = POINT {
                            x: lparam.0 as u16 as i16 as i32,
                            y: (lparam.0 >> 16) as u16 as i16 as i32,
                        };
                        let _ = ClientToScreen(hwnd, &mut point);
                        press.dragging |= (point.x - press.start.0).abs()
                            >= GetSystemMetrics(SM_CXDRAG).max(4)
                            || (point.y - press.start.1).abs()
                                >= GetSystemMetrics(SM_CYDRAG).max(4);
                        strip.press.set(Some(press));
                        if press.dragging {
                            let valid =
                                super::drag_over(WindowFromPoint(point), point, source).is_some();
                            SetCursor(
                                LoadCursorW(None, if valid { IDC_SIZEALL } else { IDC_NO }).ok(),
                            );
                        }
                    }
                    return LRESULT(0);
                }
                WM_LBUTTONUP => {
                    let press = strip.press.take();
                    if GetCapture() == hwnd {
                        let _ = ReleaseCapture();
                    }
                    let x = lparam.0 as u16 as i16 as i32;
                    let y = (lparam.0 >> 16) as u16 as i16 as i32;
                    if let Some(press) = press {
                        if press.dragging {
                            if let Hit::Target(source) = press.hit {
                                let mut point = POINT { x, y };
                                let _ = ClientToScreen(hwnd, &mut point);
                                if let Some(drop) =
                                    super::drag_over(WindowFromPoint(point), point, source)
                                {
                                    enqueue(TabNativeEvent::Drop(drop));
                                }
                            }
                        } else if strip.hit(x, y) == Some(press.hit) {
                            match press.hit {
                                Hit::Close => {
                                    enqueue(TabNativeEvent::Dissolve(strip.data.borrow().group))
                                }
                                Hit::Target(WindowTarget::Window(id)) => {
                                    enqueue(TabNativeEvent::Activate(id))
                                }
                                Hit::Target(WindowTarget::Group(_)) => {}
                            }
                        }
                    }
                    super::clear_drag();
                    SetCursor(LoadCursorW(None, IDC_ARROW).ok());
                    return LRESULT(0);
                }
                WM_CANCELMODE | WM_CAPTURECHANGED | WM_RBUTTONDOWN => {
                    strip.press.set(None);
                    super::clear_drag();
                    if message != WM_CAPTURECHANGED && GetCapture() == hwnd {
                        let _ = ReleaseCapture();
                    }
                    SetCursor(LoadCursorW(None, IDC_ARROW).ok());
                    return LRESULT(0);
                }
                WM_NCDESTROY => {
                    strip.hwnd.set(HWND::default());
                    strip.position.set(None);
                    strip.press.set(None);
                    super::clear_drag();
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                }
                _ => {}
            }
        }
        DefWindowProcW(hwnd, message, wparam, lparam)
    }
}
