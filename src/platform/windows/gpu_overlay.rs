//! DirectComposition/Direct2D overlay renderer.
//!
//! All COM interfaces and the HWND are owned by the render thread. Direct2D
//! draws straight into a DirectComposition surface, so this backend never
//! allocates a display-sized CPU pixel buffer.

use std::collections::HashMap;
use std::sync::Arc;

use windows::Win32::Foundation::{COLORREF, HMODULE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_BITMAP_OPTIONS_CANNOT_DRAW, D2D1_BITMAP_OPTIONS_TARGET,
    D2D1_BITMAP_PROPERTIES1, D2D1_DEVICE_CONTEXT_OPTIONS_NONE, D2D1_DRAW_TEXT_OPTIONS_NONE,
    D2D1_ROUNDED_RECT, D2D1CreateDevice, ID2D1DeviceContext, ID2D1SolidColorBrush,
};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION, D3D11CreateDevice, ID3D11Device,
};
use windows::Win32::Graphics::DirectComposition::{
    DCompositionCreateDevice, IDCompositionDevice, IDCompositionSurface, IDCompositionTarget,
    IDCompositionVisual,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
    DWRITE_FONT_WEIGHT_BOLD, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_MEASURING_MODE_NATURAL,
    DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_WORD_WRAPPING_NO_WRAP,
    DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM,
};
use windows::Win32::Graphics::Dxgi::{IDXGIDevice, IDXGISurface};
use windows::Win32::Graphics::Gdi::HBRUSH;
use windows::Win32::UI::WindowsAndMessaging::{
    CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow, HCURSOR, HICON,
    HWND_TOPMOST, LWA_ALPHA, RegisterClassExW, SW_SHOWNOACTIVATE, SWP_NOACTIVATE,
    SetLayeredWindowAttributes, SetWindowPos, ShowWindow, WNDCLASSEXW, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::{Interface, PCWSTR, w};
use windows_numerics::Vector2;

use crate::api::geometry::{Point, Rect};
use crate::api::overlay::{
    Color, CursorMarker, Indicator, LabelStyle, OverlayLabel, OverlayScene, OverlayShape,
};

use super::native::{NativeDimensions, OwnedWindow};

const MAX_BRUSHES: usize = 64;
const MAX_TEXT_FORMATS: usize = 32;

/// A GPU-rendered overlay. COM interface wrappers release themselves; the
/// window wrapper below provides the same single-owner rule for HWND.
pub(super) struct GpuOverlay {
    class_registered: bool,
    d2d: ID2D1DeviceContext,
    dcomp: IDCompositionDevice,
    dwrite: IDWriteFactory,
    content: Option<WindowContent>,
    brushes: HashMap<u32, ID2D1SolidColorBrush>,
    formats: Vec<FontEntry>,
    utf16: Vec<u16>,
    #[cfg(test)]
    begin_draws: usize,
}

struct WindowContent {
    window: OwnedWindow,
    _target: IDCompositionTarget,
    root: IDCompositionVisual,
    static_visual: Option<IDCompositionVisual>,
    cursor_visual: IDCompositionVisual,
    indicator_visual: IDCompositionVisual,
    static_surface: Option<IDCompositionSurface>,
    cursor_surface: Option<LayerSurface>,
    indicator_surface: Option<LayerSurface>,
    last_scene: Option<Arc<OverlayScene>>,
    area: Rect,
    width: u32,
    height: u32,
}

struct LayerSurface {
    surface: IDCompositionSurface,
    width: u32,
    height: u32,
}

struct FontEntry {
    family: String,
    size_bits: u64,
    bold: bool,
    format: IDWriteTextFormat,
}

/// Owns the native draw pairing after a successful DirectComposition
/// `BeginDraw`. Cloned COM interfaces only adjust reference counts; they do not
/// allocate another device or surface. Drop covers every early return and panic.
struct SurfaceDrawGuard {
    d2d: ID2D1DeviceContext,
    surface: IDCompositionSurface,
    d2d_started: bool,
    finished: bool,
}

impl SurfaceDrawGuard {
    fn new(d2d: &ID2D1DeviceContext, surface: &IDCompositionSurface) -> Self {
        Self {
            d2d: d2d.clone(),
            surface: surface.clone(),
            d2d_started: false,
            finished: false,
        }
    }

    fn begin_d2d(&mut self, bitmap: &windows::Win32::Graphics::Direct2D::ID2D1Bitmap1) {
        // SAFETY: the bitmap was created from this guard's active composition
        // surface and belongs to the same D2D device context.
        unsafe {
            self.d2d.SetTarget(bitmap);
            self.d2d.BeginDraw();
        }
        self.d2d_started = true;
    }

    fn finish(mut self) -> Result<(), String> {
        self.finish_inner()
    }

    fn finish_inner(&mut self) -> Result<(), String> {
        if self.finished {
            return Ok(());
        }
        self.finished = true;
        // SAFETY: each flag is set only after its matching BeginDraw succeeds.
        // Marking finished first makes this exactly-once even when EndDraw
        // reports an error; the D2D target is always detached before return.
        let (d2d_result, surface_result) = unsafe {
            let d2d_result = if self.d2d_started {
                let result = self.d2d.EndDraw(None, None);
                self.d2d.SetTarget(None);
                result.map_err(|error| format!("Direct2D EndDraw failed: {error}"))
            } else {
                Ok(())
            };
            let surface_result = self
                .surface
                .EndDraw()
                .map_err(|error| format!("DirectComposition EndDraw failed: {error}"));
            (d2d_result, surface_result)
        };
        d2d_result.and(surface_result)
    }
}

impl Drop for SurfaceDrawGuard {
    fn drop(&mut self) {
        let _ = self.finish_inner();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SceneChanges {
    static_changed: bool,
    cursor_changed: bool,
    cursor_repaint: bool,
    indicator_changed: bool,
    indicator_repaint: bool,
}

impl GpuOverlay {
    pub(super) const CLASS_NAME: PCWSTR = w!("KeySteerGpuOverlay");

    pub(super) fn new() -> Result<Self, String> {
        let mut d3d = None;
        // SAFETY: all out-pointers refer to initialized Options and the default
        // adapter is requested with a null module handle.
        unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut d3d),
                None,
                None,
            )
        }
        .map_err(|error| format!("D3D11CreateDevice failed: {error}"))?;
        let d3d: ID3D11Device = d3d.ok_or("D3D11 returned no device")?;
        let dxgi: IDXGIDevice = d3d
            .cast()
            .map_err(|error| format!("D3D11 device has no IDXGIDevice: {error}"))?;
        // SAFETY: `dxgi` is a live device owned for the renderer's lifetime.
        let d2d_device = unsafe { D2D1CreateDevice(&dxgi, None) }
            .map_err(|error| format!("D2D1CreateDevice failed: {error}"))?;
        // SAFETY: device creation is thread-affine and occurs on the renderer.
        let d2d = unsafe { d2d_device.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE) }
            .map_err(|error| format!("CreateDeviceContext failed: {error}"))?;
        // SAFETY: DirectComposition accepts the same live DXGI device.
        let dcomp: IDCompositionDevice = unsafe { DCompositionCreateDevice(&dxgi) }
            .map_err(|error| format!("DCompositionCreateDevice failed: {error}"))?;
        // SAFETY: the requested interface matches the generic return type.
        let dwrite: IDWriteFactory = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED) }
            .map_err(|error| format!("DWriteCreateFactory failed: {error}"))?;
        Ok(Self {
            class_registered: false,
            d2d,
            dcomp,
            dwrite,
            content: None,
            brushes: HashMap::with_capacity(16),
            formats: Vec::with_capacity(8),
            utf16: Vec::with_capacity(32),
            #[cfg(test)]
            begin_draws: 0,
        })
    }

    pub(super) fn present(&mut self, scene: Arc<OverlayScene>, area: Rect) -> Result<(), String> {
        let dimensions = NativeDimensions::from_f64(area.width, area.height)?;
        let width = dimensions.width_u32();
        let height = dimensions.height_u32();
        let area_changed = self.ensure_content(area, width, height)?;
        self.ensure_static_layer(scene_has_static_content(&scene), width, height)?;
        let (hwnd, changes) = self
            .content
            .as_ref()
            .map(|content| {
                (
                    content.window.raw(),
                    scene_changes(content.last_scene.as_deref(), scene.as_ref(), area_changed),
                )
            })
            .ok_or("DirectComposition content was not retained")?;
        if changes.static_changed {
            let static_surface = self
                .content
                .as_ref()
                .and_then(|content| content.static_surface.clone());
            if let Some(static_surface) = static_surface {
                self.draw_static(&static_surface, scene.as_ref(), area)?;
            }
        }
        if changes.cursor_changed {
            self.draw_cursor(scene.cursor_marker.as_ref(), area, changes.cursor_repaint)?;
        }
        if changes.indicator_changed {
            self.draw_indicator(scene.indicator.as_ref(), area, changes.indicator_repaint)?;
        }
        if let Some(content) = self.content.as_mut() {
            content.last_scene = Some(scene);
        }
        self.commit(Some(hwnd))
    }

    /// Move existing dynamic visuals without opening a Direct2D surface.
    pub(super) fn update_positions(
        &mut self,
        cursor: Option<Point>,
        indicator: Option<Point>,
    ) -> Result<(), String> {
        let content = self
            .content
            .as_ref()
            .ok_or("DirectComposition content is unavailable")?;
        if let Some(center) = cursor {
            let marker = content
                .last_scene
                .as_deref()
                .and_then(|scene| scene.cursor_marker.as_ref())
                .ok_or("cursor visual has no complete scene")?;
            if content.cursor_surface.is_none() {
                return Err("cursor visual surface is unavailable".into());
            }
            let bounds = cursor_bounds(marker, center);
            configure_visual(
                &content.cursor_visual,
                None,
                bounds.x - content.area.x,
                bounds.y - content.area.y,
            )?;
        }
        if let Some(position) = indicator {
            let item = content
                .last_scene
                .as_deref()
                .and_then(|scene| scene.indicator.as_ref())
                .ok_or("indicator visual has no complete scene")?;
            if content.indicator_surface.is_none() {
                return Err("indicator visual surface is unavailable".into());
            }
            let bounds = indicator_bounds(item, position);
            configure_visual(
                &content.indicator_visual,
                None,
                bounds.x - content.area.x,
                bounds.y - content.area.y,
            )?;
        }
        self.commit(None)
    }

    fn commit(&self, show: Option<HWND>) -> Result<(), String> {
        // SAFETY: all visual mutations were recorded on `self.dcomp`; an HWND
        // supplied by `present` remains owned by `self.content`.
        unsafe {
            self.dcomp
                .Commit()
                .map_err(|error| format!("DirectComposition commit failed: {error}"))?;
            if let Some(hwnd) = show {
                let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            }
        }
        Ok(())
    }

    /// Release all screen-sized GPU resources immediately. Device and compact
    /// brush/font caches remain warm for the next mode entry.
    pub(super) fn dismiss(&mut self) -> Result<(), String> {
        // SAFETY: detaching a target is valid outside BeginDraw. Clearing the
        // owned tree before Commit releases every compositor reference.
        unsafe {
            self.d2d.SetTarget(None);
            self.content = None;
            self.dcomp
                .Commit()
                .map_err(|error| format!("DirectComposition dismiss commit failed: {error}"))
        }
    }

    /// Ensure the compositor tree exists and report whether its screen origin
    /// changed. A stable frame must not issue a full-screen SetWindowPos.
    fn ensure_content(&mut self, area: Rect, width: u32, height: u32) -> Result<bool, String> {
        if let Some(content) = self.content.as_mut()
            && content.width == width
            && content.height == height
        {
            let area_changed = content.area != area;
            if area_changed {
                reposition(content.window.raw(), area)?;
                content.area = area;
            }
            return Ok(area_changed);
        }

        self.ensure_class()?;
        let window = OwnedWindow::new(create_window(area)?);
        // SAFETY: the HWND and DirectComposition device are both live and
        // owned by this render thread.
        let target = unsafe { self.dcomp.CreateTargetForHwnd(window.raw(), true) }
            .map_err(|error| format!("CreateTargetForHwnd failed: {error}"))?;
        // SAFETY: device methods return owned COM interfaces.
        let visual = unsafe { self.dcomp.CreateVisual() }
            .map_err(|error| format!("CreateVisual failed: {error}"))?;
        // SAFETY: every interface belongs to this device and stays alive in
        // WindowContent. Null reference visuals place static at the bottom and
        // dynamic visuals successively above it.
        let cursor_visual = unsafe { self.dcomp.CreateVisual() }
            .map_err(|error| format!("Create cursor visual failed: {error}"))?;
        // SAFETY: the compositor device returns a newly owned visual on this
        // renderer thread.
        let indicator_visual = unsafe { self.dcomp.CreateVisual() }
            .map_err(|error| format!("Create indicator visual failed: {error}"))?;
        // SAFETY: all visuals and the target belong to `self.dcomp`, remain
        // alive in WindowContent, and are attached outside BeginDraw.
        unsafe {
            visual
                .AddVisual(&cursor_visual, false, None)
                .and_then(|()| visual.AddVisual(&indicator_visual, true, None))
                .and_then(|()| target.SetRoot(&visual))
                .and_then(|()| self.dcomp.Commit())
        }
        .map_err(|error| format!("cannot attach DirectComposition visual: {error}"))?;
        self.content = Some(WindowContent {
            window,
            _target: target,
            root: visual,
            static_visual: None,
            cursor_visual,
            indicator_visual,
            static_surface: None,
            cursor_surface: None,
            indicator_surface: None,
            last_scene: None,
            area,
            width,
            height,
        });
        Ok(true)
    }

    fn ensure_static_layer(
        &mut self,
        required: bool,
        width: u32,
        height: u32,
    ) -> Result<(), String> {
        let Some(content) = self.content.as_mut() else {
            return Err("DirectComposition content was not retained".into());
        };
        if required && content.static_surface.is_none() {
            let (surface, visual) = create_static_layer(
                &self.dcomp,
                &content.root,
                &content.cursor_visual,
                width,
                height,
            )?;
            content.static_surface = Some(surface);
            content.static_visual = Some(visual);
        } else if !required && let Some(visual) = content.static_visual.take() {
            // SAFETY: the visual is currently a child of this root and drawing
            // is not active. Removing it before dropping the surface releases
            // the screen-sized compositor allocation.
            unsafe { content.root.RemoveVisual(&visual) }
                .map_err(|error| format!("cannot detach static visual: {error}"))?;
            content.static_surface = None;
        }
        Ok(())
    }

    fn ensure_class(&mut self) -> Result<(), String> {
        if self.class_registered {
            return Ok(());
        }
        // SAFETY: a null module name asks for the current executable.
        let instance = super::native::current_module()
            .map_err(|error| format!("GetModuleHandleW failed: {error}"))?;
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(window_proc),
            hInstance: instance.into(),
            lpszClassName: Self::CLASS_NAME,
            hIcon: HICON::default(),
            hCursor: HCURSOR::default(),
            hbrBackground: HBRUSH::default(),
            ..Default::default()
        };
        // SAFETY: all fields above are initialized and the callback obeys the
        // Win32 ABI without allowing a Rust panic to cross it.
        if unsafe { RegisterClassExW(&class) } == 0 {
            // SAFETY: GetLastError has no preconditions.
            let last = unsafe { windows::Win32::Foundation::GetLastError() };
            if last.0 != 1410 {
                return Err(format!(
                    "RegisterClassExW failed: {}",
                    windows::core::Error::from_hresult(last.to_hresult())
                ));
            }
        }
        self.class_registered = true;
        Ok(())
    }

    fn draw_surface(
        &mut self,
        surface: &IDCompositionSurface,
        coordinate_origin: Point,
        clear: Option<Color>,
        draw: impl FnOnce(&mut Self, Point) -> Result<(), String>,
    ) -> Result<(), String> {
        let mut offset = POINT::default();
        // SAFETY: the update offset is a valid out-parameter and the returned
        // interface is explicitly requested as IDXGISurface.
        let dxgi: IDXGISurface = unsafe { surface.BeginDraw(None, &mut offset) }
            .map_err(|error| format!("DirectComposition BeginDraw failed: {error}"))?;
        let mut draw_guard = SurfaceDrawGuard::new(&self.d2d, surface);
        let properties = D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: 96.0,
            dpiY: 96.0,
            bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
            ..Default::default()
        };
        // SAFETY: `dxgi` is the surface returned by BeginDraw and properties
        // describe that surface's exact format.
        let bitmap = unsafe {
            self.d2d
                .CreateBitmapFromDxgiSurface(&dxgi, Some(&properties))
        }
        .map_err(|error| format!("CreateBitmapFromDxgiSurface failed: {error}"))?;
        #[cfg(test)]
        {
            self.begin_draws = self.begin_draws.wrapping_add(1);
        }
        draw_guard.begin_d2d(&bitmap);
        let origin = Point::new(
            coordinate_origin.x - f64::from(offset.x),
            coordinate_origin.y - f64::from(offset.y),
        );
        let clear = clear.map(d2d_color);
        // SAFETY: drawing is active and the optional local color stays live
        // for the duration of the call.
        unsafe { self.d2d.Clear(clear.as_ref().map(std::ptr::from_ref)) };
        let draw_result = draw(self, origin);
        draw_result.and(draw_guard.finish())
    }

    fn draw_static(
        &mut self,
        surface: &IDCompositionSurface,
        scene: &OverlayScene,
        area: Rect,
    ) -> Result<(), String> {
        self.draw_surface(
            surface,
            Point::new(area.x, area.y),
            scene.backdrop,
            |renderer, origin| {
                for shape in &scene.shapes {
                    renderer.draw_shape(shape, origin)?;
                }
                for label in &scene.labels {
                    renderer.draw_label(label, origin)?;
                }
                Ok(())
            },
        )
    }

    fn draw_cursor(
        &mut self,
        marker: Option<&CursorMarker>,
        area: Rect,
        repaint: bool,
    ) -> Result<(), String> {
        let Some(marker) = marker else {
            if let Some(content) = self.content.as_mut() {
                // SAFETY: clearing content releases the visual's surface.
                unsafe { content.cursor_visual.SetContent(None) }
                    .map_err(|error| format!("cannot clear cursor visual: {error}"))?;
                content.cursor_surface = None;
            }
            return Ok(());
        };
        let bounds = cursor_bounds(marker, marker.center);
        let width = dimension(bounds.width)?;
        let height = dimension(bounds.height)?;
        let surface_to_paint = {
            let dcomp = &self.dcomp;
            let content = self
                .content
                .as_mut()
                .ok_or("DirectComposition content is unavailable")?;
            let recreated =
                ensure_layer_surface(dcomp, &mut content.cursor_surface, width, height)?;
            let layer = content
                .cursor_surface
                .as_ref()
                .ok_or("DirectComposition cursor surface was not retained")?;
            let bind = recreated.then(|| layer.surface.clone());
            configure_visual(
                &content.cursor_visual,
                bind.as_ref(),
                bounds.x - area.x,
                bounds.y - area.y,
            )?;
            (repaint || recreated).then(|| layer.surface.clone())
        };
        if let Some(surface) = surface_to_paint {
            self.draw_surface(
                &surface,
                Point::new(bounds.x, bounds.y),
                None,
                |renderer, origin| {
                    renderer.draw_shape(
                        &OverlayShape::Rect {
                            rect: Rect::new(
                                marker.center.x - marker.radius,
                                marker.center.y - marker.radius,
                                marker.radius * 2.0,
                                marker.radius * 2.0,
                            ),
                            fill: marker.fill,
                            stroke: marker.stroke,
                            stroke_width: marker.stroke_width,
                            corner_radius: marker.radius,
                            z_index: i32::MAX,
                        },
                        origin,
                    )
                },
            )?;
        }
        Ok(())
    }

    fn draw_indicator(
        &mut self,
        indicator: Option<&Indicator>,
        area: Rect,
        repaint: bool,
    ) -> Result<(), String> {
        let Some(indicator) = indicator else {
            if let Some(content) = self.content.as_mut() {
                // SAFETY: clearing content releases the visual's surface.
                unsafe { content.indicator_visual.SetContent(None) }
                    .map_err(|error| format!("cannot clear indicator visual: {error}"))?;
                content.indicator_surface = None;
            }
            return Ok(());
        };
        let first = indicator_rect(&indicator.text, indicator.position, &indicator.style);
        let held = indicator.held_text.as_deref().map(|text| {
            indicator_rect(
                text,
                Point::new(indicator.position.x, first.bottom() + 4.0),
                &indicator.style,
            )
        });
        let bounds = indicator_bounds(indicator, indicator.position);
        let width = dimension(bounds.width)?;
        let height = dimension(bounds.height)?;
        let surface_to_paint = {
            let dcomp = &self.dcomp;
            let content = self
                .content
                .as_mut()
                .ok_or("DirectComposition content is unavailable")?;
            let recreated =
                ensure_layer_surface(dcomp, &mut content.indicator_surface, width, height)?;
            let layer = content
                .indicator_surface
                .as_ref()
                .ok_or("DirectComposition indicator surface was not retained")?;
            let bind = recreated.then(|| layer.surface.clone());
            configure_visual(
                &content.indicator_visual,
                bind.as_ref(),
                bounds.x - area.x,
                bounds.y - area.y,
            )?;
            (repaint || recreated).then(|| layer.surface.clone())
        };
        if let Some(surface) = surface_to_paint {
            self.draw_surface(
                &surface,
                Point::new(bounds.x, bounds.y),
                None,
                |renderer, origin| {
                    renderer.draw_label_parts(
                        &indicator.text,
                        first,
                        &indicator.style,
                        0,
                        origin,
                    )?;
                    if let (Some(text), Some(rect)) = (&indicator.held_text, held) {
                        renderer.draw_label_parts(text, rect, &indicator.style, 0, origin)?;
                    }
                    Ok(())
                },
            )?;
        }
        Ok(())
    }

    fn draw_shape(&mut self, shape: &OverlayShape, origin: Point) -> Result<(), String> {
        match shape {
            OverlayShape::Rect {
                rect,
                fill,
                stroke,
                stroke_width,
                corner_radius,
                ..
            } => {
                let rect = local_rect(*rect, origin);
                let rounded = D2D1_ROUNDED_RECT {
                    rect,
                    radiusX: *corner_radius as f32,
                    radiusY: *corner_radius as f32,
                };
                if !fill.is_transparent() {
                    let brush = self.brush(*fill)?;
                    // SAFETY: drawing is active and both pointers are live.
                    unsafe { self.d2d.FillRoundedRectangle(&rounded, &brush) };
                }
                if !stroke.is_transparent() && *stroke_width > 0.0 {
                    let brush = self.brush(*stroke)?;
                    // SAFETY: drawing is active and both pointers are live.
                    unsafe {
                        self.d2d
                            .DrawRoundedRectangle(&rounded, &brush, *stroke_width as f32, None)
                    };
                }
            }
            OverlayShape::Line {
                from,
                to,
                color,
                width,
                ..
            } if !color.is_transparent() && *width > 0.0 => {
                let brush = self.brush(*color)?;
                let from = local_point(*from, origin);
                let to = local_point(*to, origin);
                // SAFETY: drawing is active and the brush belongs to this
                // device context.
                unsafe { self.d2d.DrawLine(from, to, &brush, *width as f32, None) };
            }
            OverlayShape::Line { .. } => {}
        }
        Ok(())
    }

    fn draw_label(&mut self, label: &OverlayLabel, origin: Point) -> Result<(), String> {
        self.draw_label_parts(
            &label.text,
            label.rect,
            &label.style,
            label.matched_prefix_len,
            origin,
        )
    }

    fn draw_label_parts(
        &mut self,
        text: &str,
        label_rect: Rect,
        style: &LabelStyle,
        matched_prefix_len: usize,
        origin: Point,
    ) -> Result<(), String> {
        let rect = local_rect(label_rect, origin);
        let rounded = D2D1_ROUNDED_RECT {
            rect,
            radiusX: style.border_radius as f32,
            radiusY: style.border_radius as f32,
        };
        if !style.background.is_transparent() {
            let brush = self.brush(style.background)?;
            // SAFETY: drawing is active and both objects are live.
            unsafe { self.d2d.FillRoundedRectangle(&rounded, &brush) };
        }
        if !style.border_color.is_transparent() && style.border_width > 0.0 {
            let brush = self.brush(style.border_color)?;
            // SAFETY: drawing is active and both objects are live.
            unsafe {
                self.d2d
                    .DrawRoundedRectangle(&rounded, &brush, style.border_width as f32, None)
            };
        }
        let format = self.text_format(style)?;
        let normal = self.brush(style.text_color)?;
        self.utf16.clear();
        self.utf16.extend(text.encode_utf16());
        // SAFETY: UTF-16 data, format, brush and layout rectangle remain live
        // for the complete call.
        unsafe {
            self.d2d.DrawText(
                &self.utf16,
                &format,
                &rect,
                &normal,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            )
        };
        if matched_prefix_len > 0 && !style.matched_text_color.is_transparent() {
            let total = text.chars().count().max(1);
            let matched = matched_prefix_len.min(total);
            let mut clip = rect;
            clip.right = clip.left + (clip.right - clip.left) * matched as f32 / total as f32;
            let matched_brush = self.brush(style.matched_text_color)?;
            // SAFETY: the clip is finite and Push/Pop are balanced.
            unsafe {
                self.d2d
                    .PushAxisAlignedClip(&clip, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
                self.d2d.DrawText(
                    &self.utf16,
                    &format,
                    &rect,
                    &matched_brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                    DWRITE_MEASURING_MODE_NATURAL,
                );
                self.d2d.PopAxisAlignedClip();
            }
        }
        Ok(())
    }

    fn brush(&mut self, color: Color) -> Result<ID2D1SolidColorBrush, String> {
        let key = color_key(color);
        if let Some(brush) = self.brushes.get(&key) {
            return Ok(brush.clone());
        }
        if self.brushes.len() >= MAX_BRUSHES {
            self.brushes.clear();
        }
        let color = d2d_color(color);
        // SAFETY: the color pointer is valid and default brush properties are
        // requested. The returned COM interface is owned.
        let brush = unsafe { self.d2d.CreateSolidColorBrush(&color, None) }
            .map_err(|error| format!("CreateSolidColorBrush failed: {error}"))?;
        self.brushes.insert(key, brush.clone());
        Ok(brush)
    }

    fn text_format(&mut self, style: &LabelStyle) -> Result<IDWriteTextFormat, String> {
        let size_bits = style.font_size.to_bits();
        if let Some(entry) = self.formats.iter().find(|entry| {
            entry.family == style.font_family
                && entry.size_bits == size_bits
                && entry.bold == style.bold
        }) {
            return Ok(entry.format.clone());
        }
        if self.formats.len() >= MAX_TEXT_FORMATS {
            self.formats.clear();
        }
        let family = if style.font_family.is_empty() {
            "Segoe UI"
        } else {
            &style.font_family
        };
        let mut family_wide: Vec<u16> = family.encode_utf16().chain([0]).collect();
        let locale = w!("en-us");
        // SAFETY: both strings are NUL-terminated and live through the call;
        // the shared factory retains all data it needs.
        let format = unsafe {
            self.dwrite.CreateTextFormat(
                PCWSTR(family_wide.as_mut_ptr()),
                None,
                if style.bold {
                    DWRITE_FONT_WEIGHT_BOLD
                } else {
                    DWRITE_FONT_WEIGHT_NORMAL
                },
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                style.font_size.max(1.0) as f32,
                locale,
            )
        }
        .map_err(|error| format!("CreateTextFormat failed: {error}"))?;
        // SAFETY: the format is newly owned and all enum values are valid.
        unsafe {
            format
                .SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)
                .and_then(|()| format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER))
                .and_then(|()| format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP))
        }
        .map_err(|error| format!("cannot configure DirectWrite format: {error}"))?;
        self.formats.push(FontEntry {
            family: style.font_family.clone(),
            size_bits,
            bold: style.bold,
            format: format.clone(),
        });
        Ok(format)
    }
}

fn create_static_layer(
    device: &IDCompositionDevice,
    root: &IDCompositionVisual,
    cursor: &IDCompositionVisual,
    width: u32,
    height: u32,
) -> Result<(IDCompositionSurface, IDCompositionVisual), String> {
    // SAFETY: dimensions were validated before content creation. All returned
    // interfaces and both existing visuals belong to this compositor device
    // and render thread; attachment occurs outside an active BeginDraw.
    unsafe {
        let surface = device
            .CreateSurface(
                width,
                height,
                DXGI_FORMAT_B8G8R8A8_UNORM,
                DXGI_ALPHA_MODE_PREMULTIPLIED,
            )
            .map_err(|error| format!("CreateSurface failed: {error}"))?;
        let visual = device
            .CreateVisual()
            .map_err(|error| format!("Create static visual failed: {error}"))?;
        visual
            .SetContent(&surface)
            .and_then(|()| root.AddVisual(&visual, false, Some(cursor)))
            .map_err(|error| format!("cannot attach static visual: {error}"))?;
        Ok((surface, visual))
    }
}

fn ensure_layer_surface(
    device: &IDCompositionDevice,
    slot: &mut Option<LayerSurface>,
    width: u32,
    height: u32,
) -> Result<bool, String> {
    if let Some(layer) = slot
        && layer.width == width
        && layer.height == height
    {
        return Ok(false);
    }
    // SAFETY: dimensions are positive and the format matches the renderer's
    // Direct2D bitmap target.
    let surface = unsafe {
        device.CreateSurface(
            width,
            height,
            DXGI_FORMAT_B8G8R8A8_UNORM,
            DXGI_ALPHA_MODE_PREMULTIPLIED,
        )
    }
    .map_err(|error| format!("cannot create tight DirectComposition surface: {error}"))?;
    *slot = Some(LayerSurface {
        surface,
        width,
        height,
    });
    Ok(true)
}

fn static_scene_eq(left: &OverlayScene, right: &OverlayScene) -> bool {
    left.backdrop == right.backdrop
        && left.clip == right.clip
        && left.shapes == right.shapes
        && left.labels == right.labels
}

fn cursor_content_eq(left: Option<&CursorMarker>, right: Option<&CursorMarker>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => {
            left.radius == right.radius
                && left.fill == right.fill
                && left.stroke == right.stroke
                && left.stroke_width == right.stroke_width
        }
        (None, None) => true,
        _ => false,
    }
}

fn indicator_content_eq(left: Option<&Indicator>, right: Option<&Indicator>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => {
            left.text == right.text
                && left.held_text == right.held_text
                && left.style == right.style
        }
        (None, None) => true,
        _ => false,
    }
}

fn scene_changes(
    previous: Option<&OverlayScene>,
    next: &OverlayScene,
    area_changed: bool,
) -> SceneChanges {
    SceneChanges {
        static_changed: area_changed
            || previous.is_none_or(|previous| !static_scene_eq(previous, next)),
        cursor_changed: area_changed
            || previous.is_none_or(|previous| {
                previous.cursor_marker.as_ref() != next.cursor_marker.as_ref()
            }),
        cursor_repaint: previous.is_none_or(|previous| {
            !cursor_content_eq(previous.cursor_marker.as_ref(), next.cursor_marker.as_ref())
        }),
        indicator_changed: area_changed
            || previous
                .is_none_or(|previous| previous.indicator.as_ref() != next.indicator.as_ref()),
        indicator_repaint: previous.is_none_or(|previous| {
            !indicator_content_eq(previous.indicator.as_ref(), next.indicator.as_ref())
        }),
    }
}

fn scene_has_static_content(scene: &OverlayScene) -> bool {
    scene.backdrop.is_some() || !scene.shapes.is_empty() || !scene.labels.is_empty()
}

fn create_window(area: Rect) -> Result<HWND, String> {
    let x = area.x.round() as i32;
    let y = area.y.round() as i32;
    let dimensions = NativeDimensions::from_f64(area.width, area.height)?;
    // SAFETY: the class is registered before this function is called. A
    // successfully created HWND belongs to this thread; initialization either
    // completes or destroys it before returning the error.
    let hwnd = unsafe {
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            GpuOverlay::CLASS_NAME,
            w!("KeySteer"),
            WS_POPUP,
            x,
            y,
            dimensions.width_i32(),
            dimensions.height_i32(),
            None,
            None,
            None,
            None,
        )
        .map_err(|error| format!("CreateWindowExW failed: {error}"))?;
        // A fully opaque global alpha enables the layered HWND while
        // DirectComposition supplies per-pixel alpha. WS_EX_TRANSPARENT then
        // routes mouse input to windows underneath.
        if let Err(error) = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA) {
            let _ = DestroyWindow(hwnd);
            return Err(format!("SetLayeredWindowAttributes failed: {error}"));
        }
        hwnd
    };
    Ok(hwnd)
}

fn reposition(hwnd: HWND, area: Rect) -> Result<(), String> {
    let dimensions = NativeDimensions::from_f64(area.width, area.height)?;
    // SAFETY: `hwnd` is owned by the current render thread.
    unsafe {
        SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            area.x.round() as i32,
            area.y.round() as i32,
            dimensions.width_i32(),
            dimensions.height_i32(),
            SWP_NOACTIVATE,
        )
    }
    .map_err(|error| format!("SetWindowPos failed: {error}"))
}

fn dimension(value: f64) -> Result<u32, String> {
    NativeDimensions::from_f64(value, 1.0).map(NativeDimensions::width_u32)
}

fn d2d_color(color: Color) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: f32::from(color.r) / 255.0,
        g: f32::from(color.g) / 255.0,
        b: f32::from(color.b) / 255.0,
        a: f32::from(color.a) / 255.0,
    }
}

const fn color_key(color: Color) -> u32 {
    u32::from_be_bytes([color.r, color.g, color.b, color.a])
}

fn local_rect(rect: Rect, origin: Point) -> D2D_RECT_F {
    D2D_RECT_F {
        left: (rect.x - origin.x) as f32,
        top: (rect.y - origin.y) as f32,
        right: (rect.right() - origin.x) as f32,
        bottom: (rect.bottom() - origin.y) as f32,
    }
}

fn local_point(point: Point, origin: Point) -> Vector2 {
    Vector2 {
        X: (point.x - origin.x) as f32,
        Y: (point.y - origin.y) as f32,
    }
}

fn configure_visual(
    visual: &IDCompositionVisual,
    content: Option<&IDCompositionSurface>,
    x: f64,
    y: f64,
) -> Result<(), String> {
    // SAFETY: the visual and optional surface belong to the same live
    // DirectComposition device; offsets are local to the owned overlay HWND.
    unsafe {
        if let Some(content) = content {
            visual
                .SetContent(content)
                .map_err(|error| format!("cannot bind dynamic surface: {error}"))?;
        }
        visual
            .SetOffsetX2(x as f32)
            .and_then(|()| visual.SetOffsetY2(y as f32))
    }
    .map_err(|error| format!("cannot position dynamic visual: {error}"))
}

fn cursor_bounds(marker: &CursorMarker, center: Point) -> Rect {
    let padding = (marker.stroke_width.max(0.0) / 2.0).ceil() + 1.0;
    Rect::new(
        center.x - marker.radius - padding,
        center.y - marker.radius - padding,
        marker.radius * 2.0 + padding * 2.0,
        marker.radius * 2.0 + padding * 2.0,
    )
}

fn indicator_bounds(indicator: &Indicator, position: Point) -> Rect {
    let first = indicator_rect(&indicator.text, position, &indicator.style);
    indicator
        .held_text
        .as_deref()
        .map(|text| {
            indicator_rect(
                text,
                Point::new(position.x, first.bottom() + 4.0),
                &indicator.style,
            )
        })
        .map_or(first, |held| first.union(&held))
        .inset(-1.0, -1.0)
}

fn indicator_rect(text: &str, position: Point, style: &LabelStyle) -> Rect {
    let width = (text.chars().count() as f64 * style.font_size * 0.7 + style.padding_x * 2.0)
        .max(style.font_size * 2.0);
    let height = style.font_size + style.padding_y * 2.0;
    Rect::new(position.x - width, position.y, width, height)
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if let Some(result) = super::native::click_through_hit_test(message) {
        return result;
    }
    // SAFETY: no Rust state is accessed and `DefWindowProcW` receives the exact
    // arguments supplied by Windows; no unwind crosses the callback boundary.
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use std::time::Instant;

    fn percentiles(mut samples: Vec<u128>) -> (u128, u128, u128) {
        samples.sort_unstable();
        let last = samples.len() - 1;
        (
            samples[last * 50 / 100],
            samples[last * 95 / 100],
            samples[last * 99 / 100],
        )
    }

    fn report_process_metrics(label: &str) {
        let script = format!(
            "$p=Get-Process -Id {}; Write-Output ('native_metrics label={label} working_set=' + $p.WorkingSet64 + ' private_bytes=' + $p.PrivateMemorySize64 + ' handles=' + $p.HandleCount + ' threads=' + $p.Threads.Count)",
            std::process::id()
        );
        match Command::new(r"C:\Program Files\PowerShell\7\pwsh.exe")
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                &script,
            ])
            .output()
        {
            Ok(output) if output.status.success() => {
                print!("{}", String::from_utf8_lossy(&output.stdout));
            }
            Ok(output) => println!(
                "native_metrics label={label} unavailable: {}",
                String::from_utf8_lossy(&output.stderr)
            ),
            Err(error) => println!("native_metrics label={label} unavailable: {error}"),
        }
    }

    #[test]
    fn static_cache_ignores_cursor_only_changes() {
        let mut first = OverlayScene::new();
        first.shapes.push(OverlayShape::fill(
            Rect::new(0.0, 0.0, 100.0, 100.0),
            Color::rgb(1, 2, 3),
        ));
        first.cursor_marker = Some(crate::api::overlay::CursorMarker {
            center: Point::new(10.0, 20.0),
            radius: 8.0,
            fill: Color::rgb(4, 5, 6),
            stroke: Color::rgb(7, 8, 9),
            stroke_width: 1.0,
        });
        let mut second = first.clone();
        if let Some(marker) = second.cursor_marker.as_mut() {
            marker.center = Point::new(900.0, 700.0);
        }
        assert!(static_scene_eq(&first, &second));
        second.shapes.push(OverlayShape::fill(
            Rect::new(5.0, 5.0, 10.0, 10.0),
            Color::rgb(10, 11, 12),
        ));
        assert!(!static_scene_eq(&first, &second));
    }

    #[test]
    fn dynamic_layers_move_without_repainting_unchanged_content() {
        let mut first = OverlayScene::new();
        first.cursor_marker = Some(CursorMarker {
            center: Point::new(10.0, 20.0),
            radius: 8.0,
            fill: Color::rgb(4, 5, 6),
            stroke: Color::rgb(7, 8, 9),
            stroke_width: 1.0,
        });
        first.indicator = Some(Indicator {
            text: "Normal".into(),
            held_text: Some("Shift".into()),
            position: Point::new(30.0, 40.0),
            style: LabelStyle::default(),
        });

        let mut moved = first.clone();
        moved.cursor_marker.as_mut().unwrap().center = Point::new(100.0, 200.0);
        moved.indicator.as_mut().unwrap().position = Point::new(120.0, 240.0);
        assert_eq!(
            scene_changes(Some(&first), &moved, false),
            SceneChanges {
                static_changed: false,
                cursor_changed: true,
                cursor_repaint: false,
                indicator_changed: true,
                indicator_repaint: false,
            }
        );

        let area_moved = scene_changes(Some(&moved), &moved, true);
        assert!(area_moved.static_changed);
        assert!(area_moved.cursor_changed);
        assert!(!area_moved.cursor_repaint);
        assert!(area_moved.indicator_changed);
        assert!(!area_moved.indicator_repaint);
    }

    #[test]
    fn dynamic_layers_repaint_when_their_content_changes() {
        let mut first = OverlayScene::new();
        first.cursor_marker = Some(CursorMarker {
            center: Point::new(10.0, 20.0),
            radius: 8.0,
            fill: Color::rgb(4, 5, 6),
            stroke: Color::rgb(7, 8, 9),
            stroke_width: 1.0,
        });
        first.indicator = Some(Indicator {
            text: "Normal".into(),
            held_text: None,
            position: Point::new(30.0, 40.0),
            style: LabelStyle::default(),
        });

        let mut changed = first.clone();
        changed.cursor_marker.as_mut().unwrap().radius = 12.0;
        changed.indicator.as_mut().unwrap().held_text = Some("Ctrl".into());
        let changes = scene_changes(Some(&first), &changed, false);
        assert!(changes.cursor_changed);
        assert!(changes.cursor_repaint);
        assert!(changes.indicator_changed);
        assert!(changes.indicator_repaint);
    }

    #[test]
    fn native_surface_dimensions_are_validated() {
        assert_eq!(dimension(1.4), Ok(1));
        assert!(dimension(0.0).is_err());
        assert!(dimension(f64::NAN).is_err());
        assert!(dimension(f64::INFINITY).is_err());
    }

    #[test]
    #[ignore = "requires an interactive Windows compositor"]
    fn directcomposition_smoke() -> Result<(), String> {
        let mut renderer = GpuOverlay::new()?;
        let mut scene = OverlayScene::new();
        let area = Rect::new(0.0, 0.0, 256.0, 128.0);
        scene.clip = Some(area);
        scene.backdrop = Some(Color::rgba(0, 0, 0, 8));
        scene.shapes.push(OverlayShape::fill(
            Rect::new(8.0, 8.0, 64.0, 48.0),
            Color::rgba(20, 40, 80, 128),
        ));
        scene.shapes.push(OverlayShape::line(
            Point::new(8.0, 64.0),
            Point::new(240.0, 64.0),
            Color::rgb(255, 255, 255),
            1.0,
        ));
        scene.labels.push(OverlayLabel::new(
            "GPU",
            Rect::new(80.0, 8.0, 64.0, 32.0),
            LabelStyle::default(),
        ));
        scene.cursor_marker = Some(crate::api::overlay::CursorMarker {
            center: Point::new(180.0, 32.0),
            radius: 8.0,
            fill: Color::rgba(255, 255, 255, 32),
            stroke: Color::rgb(255, 255, 255),
            stroke_width: 1.0,
        });
        scene.indicator = Some(crate::api::overlay::Indicator {
            text: "DComp".into(),
            held_text: Some("GPU".into()),
            position: Point::new(248.0, 72.0),
            style: LabelStyle::default(),
        });
        renderer.present(Arc::new(scene), area)?;
        renderer.dismiss()
    }

    #[test]
    #[ignore = "native performance probe; requires an interactive Windows compositor"]
    fn native_performance_probe_directcomposition_motion() -> Result<(), String> {
        const WARMUP_FRAMES: usize = 100;
        const SAMPLES: usize = 2_000;

        let gpu_started = Instant::now();
        let mut renderer = GpuOverlay::new()?;
        println!("native_gpu_ready_ns={}", gpu_started.elapsed().as_nanos());
        report_process_metrics("gpu_ready");

        let area = Rect::new(0.0, 0.0, 3840.0, 2160.0);
        let mut scene = OverlayScene::new();
        scene.clip = Some(area);
        scene.backdrop = Some(Color::rgba(0, 0, 0, 8));
        for index in 0..1_000 {
            let column = index % 40;
            let row = index / 40;
            scene.labels.push(OverlayLabel::new(
                format!("{index:03}"),
                Rect::new(column as f64 * 96.0, row as f64 * 72.0, 88.0, 32.0),
                LabelStyle::default(),
            ));
        }
        scene.cursor_marker = Some(CursorMarker {
            center: Point::new(200.0, 200.0),
            radius: 8.0,
            fill: Color::rgba(255, 255, 255, 32),
            stroke: Color::rgb(255, 255, 255),
            stroke_width: 1.0,
        });
        scene.indicator = Some(Indicator {
            text: "Normal".into(),
            held_text: Some("Shift".into()),
            position: Point::new(280.0, 220.0),
            style: LabelStyle::default(),
        });

        let first_scene = Arc::new(scene);
        let first_started = Instant::now();
        renderer.present(Arc::clone(&first_scene), area)?;
        println!(
            "native_first_present_ns={}",
            first_started.elapsed().as_nanos()
        );
        report_process_metrics("first_present");

        let mut current = first_scene;
        let begin_draws = renderer.begin_draws;
        let mut samples = Vec::with_capacity(SAMPLES);
        for frame in 0..WARMUP_FRAMES + SAMPLES {
            let x = 200.0 + (frame % 1_000) as f64;
            let y = 200.0 + (frame % 500) as f64;
            let started = Instant::now();
            renderer
                .update_positions(Some(Point::new(x, y)), Some(Point::new(x + 80.0, y + 20.0)))?;
            let elapsed = started.elapsed().as_nanos();
            if frame >= WARMUP_FRAMES {
                samples.push(elapsed);
            }
        }
        assert_eq!(
            renderer.begin_draws, begin_draws,
            "position-only frames must not enter Direct2D BeginDraw"
        );
        let (p50, p95, p99) = percentiles(samples);
        println!("native_motion_present samples={SAMPLES} p50={p50}ns p95={p95}ns p99={p99}ns");

        // Exact shape of the previous Engine/backend route: construct a new
        // shared scene and let `present` rediscover that only positions moved.
        let mut complete_samples = Vec::with_capacity(SAMPLES);
        for frame in 0..WARMUP_FRAMES + SAMPLES {
            let mut next = current.as_ref().clone();
            let x = 300.0 + (frame % 1_000) as f64;
            let y = 300.0 + (frame % 500) as f64;
            next.cursor_marker.as_mut().unwrap().center = Point::new(x, y);
            next.indicator.as_mut().unwrap().position = Point::new(x + 80.0, y + 20.0);
            let next = Arc::new(next);
            let started = Instant::now();
            renderer.present(Arc::clone(&next), area)?;
            let elapsed = started.elapsed().as_nanos();
            if frame >= WARMUP_FRAMES {
                complete_samples.push(elapsed);
            }
            current = next;
        }
        let (complete_p50, complete_p95, complete_p99) = percentiles(complete_samples);
        println!(
            "native_complete_scene_motion samples={SAMPLES} p50={complete_p50}ns p95={complete_p95}ns p99={complete_p99}ns"
        );

        // Conservative in-process representation of the previous renderer:
        // force both tight dynamic surfaces to repaint on every position
        // update. It intentionally does not add the old per-frame full-screen
        // SetWindowPos or static-surface COM clone, so the comparison cannot
        // overstate the benefit of position-only visual updates.
        let mut repaint_samples = Vec::with_capacity(SAMPLES);
        for frame in 0..WARMUP_FRAMES + SAMPLES {
            let mut next = current.as_ref().clone();
            let x = 400.0 + (frame % 1_000) as f64;
            let y = 400.0 + (frame % 500) as f64;
            let alternate = frame.is_multiple_of(2);
            let marker = next.cursor_marker.as_mut().unwrap();
            marker.center = Point::new(x, y);
            marker.fill = if alternate {
                Color::rgba(255, 255, 255, 32)
            } else {
                Color::rgba(240, 240, 240, 32)
            };
            let indicator = next.indicator.as_mut().unwrap();
            indicator.position = Point::new(x + 80.0, y + 20.0);
            indicator.style.text_color = if alternate {
                Color::rgb(255, 255, 255)
            } else {
                Color::rgb(240, 240, 240)
            };
            let next = Arc::new(next);
            let started = Instant::now();
            renderer.present(Arc::clone(&next), area)?;
            let elapsed = started.elapsed().as_nanos();
            if frame >= WARMUP_FRAMES {
                repaint_samples.push(elapsed);
            }
            current = next;
        }
        let (repaint_p50, repaint_p95, repaint_p99) = percentiles(repaint_samples);
        println!(
            "native_forced_repaint_present samples={SAMPLES} p50={repaint_p50}ns p95={repaint_p95}ns p99={repaint_p99}ns"
        );
        report_process_metrics("steady_motion");

        let dismiss_started = Instant::now();
        renderer.dismiss()?;
        println!("native_dismiss_ns={}", dismiss_started.elapsed().as_nanos());
        drop(current);
        std::thread::sleep(std::time::Duration::from_millis(50));
        report_process_metrics("dismissed");
        Ok(())
    }
}
