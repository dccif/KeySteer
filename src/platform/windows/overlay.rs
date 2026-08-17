//! Windows overlay rendering.
//!
//! A per-monitor layered, transparent, click-through window presents the
//! scene. `WS_EX_NOACTIVATE | WS_EX_TRANSPARENT` keeps focus and clicks with
//! the application underneath, which matters because the overlay is up while
//! the user is aiming at that application.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, ANTIALIASED_QUALITY, BLENDFUNCTION, CLIP_DEFAULT_PRECIS,
    CreateFontW, DEFAULT_CHARSET, DT_CENTER, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, DeleteObject,
    DrawTextW, FF_DONTCARE, FW_BOLD, FW_NORMAL, GetTextExtentExPointW, HBRUSH, HFONT,
    OUT_DEFAULT_PRECIS, SetBkMode, SetTextColor, TRANSPARENT, UpdateWindow, ValidateRect,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CS_HREDRAW, CS_VREDRAW, CreateWindowExW, HCURSOR, HICON, HWND_TOPMOST, SW_SHOWNOACTIVATE,
    SWP_NOACTIVATE, SetWindowPos, ShowWindow, ULW_ALPHA, UpdateLayeredWindow, WM_DESTROY, WM_PAINT,
    WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::{PCWSTR, w};

use crate::api::geometry::{Point, Rect};
use crate::api::overlay::{
    Color, LabelStyle, LabelTextAnalysis, OverlayItems, OverlayLabel, OverlayScene, OverlayShape,
};

use super::native::{GdiDibSurface, NativeDimensions, OwnedWindow};

/// Renders scenes into layered click-through windows, one per monitor.
pub struct Overlay {
    /// Registered once per process.
    class_registered: bool,
    /// Window per screen region, keyed by the region's origin.
    windows: HashMap<(i32, i32), OwnedWindow>,
    /// Last uploaded bitmap expressed relative to its window origin. Cursor
    /// indicators can then follow the pointer by moving the native window,
    /// without rasterising and uploading the same pixels again.
    local_scene: Option<CachedScene>,
    /// One persistent top-down DIB. Rendering writes directly into the pixels
    /// consumed by `UpdateLayeredWindow`, avoiding a second full-screen buffer,
    /// a memcpy and repeated GDI allocation on every frame.
    dib: Option<GdiDibSurface>,
    /// Reuses the text DIB, UTF-16 buffer, mask and a strictly bounded set of
    /// native fonts across all labels in one overlay lifetime.
    text_rasterizer: TextRasterizer,
    visible: bool,
}

struct CachedScene {
    scene: Arc<OverlayScene>,
    area: Rect,
}

impl Overlay {
    pub fn new() -> Self {
        Self {
            class_registered: false,
            windows: HashMap::new(),
            local_scene: None,
            dib: None,
            text_rasterizer: TextRasterizer::new(),
            visible: false,
        }
    }

    pub(super) const CLASS_NAME: PCWSTR = w!("KeySteerOverlay");

    fn ensure_class(&mut self) -> Result<(), String> {
        if self.class_registered {
            return Ok(());
        }
        // SAFETY: a null module name yields this executable's handle.
        let instance =
            super::native::current_module().map_err(|e| format!("GetModuleHandleW failed: {e}"))?;

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

        super::native::register_window_class(&class)?;
        self.class_registered = true;
        Ok(())
    }

    /// Create (or reposition) the window covering `area`.
    fn ensure_window(&mut self, area: Rect) -> Result<HWND, String> {
        self.ensure_class()?;
        let key = (area.x.round() as i32, area.y.round() as i32);

        if let Some(window) = self.windows.get(&key) {
            let hwnd = window.raw();
            // SAFETY: `hwnd` is a window we created and have not destroyed.
            unsafe {
                SetWindowPos(
                    hwnd,
                    Some(HWND_TOPMOST),
                    key.0,
                    key.1,
                    area.width.round() as i32,
                    area.height.round() as i32,
                    SWP_NOACTIVATE,
                )
            }
            .map_err(|error| format!("SetWindowPos failed: {error}"))?;
            return Ok(hwnd);
        }

        // The current backend presents one composed scene. Reuse its window
        // when a cursor-follow clip moves instead of allocating one layered
        // window for every pointer coordinate.
        if let Some(old_key) = self.windows.keys().next().copied() {
            let window = self
                .windows
                .remove(&old_key)
                .ok_or("overlay window disappeared while repositioning")?;
            let hwnd = window.raw();
            // SAFETY: `hwnd` remains owned by `window` for the synchronous
            // move and the validated area converts to Win32 coordinates.
            unsafe {
                SetWindowPos(
                    hwnd,
                    Some(HWND_TOPMOST),
                    key.0,
                    key.1,
                    area.width.round() as i32,
                    area.height.round() as i32,
                    SWP_NOACTIVATE,
                )
                .map_err(|error| format!("SetWindowPos failed: {error}"))?;
            }
            self.windows.insert(key, window);
            return Ok(hwnd);
        }

        // SAFETY: the class is registered and all arguments are valid.
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_LAYERED
                    | WS_EX_TRANSPARENT
                    | WS_EX_NOACTIVATE
                    | WS_EX_TOOLWINDOW
                    | WS_EX_TOPMOST,
                Self::CLASS_NAME,
                w!("KeySteer"),
                WS_POPUP,
                key.0,
                key.1,
                area.width.round() as i32,
                area.height.round() as i32,
                None,
                None,
                None,
                None,
            )
        }
        .map_err(|e| format!("CreateWindowExW failed: {e}"))?;

        // SAFETY: `hwnd` was just created successfully.
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
        self.windows.insert(key, OwnedWindow::new(hwnd));
        Ok(hwnd)
    }

    pub fn present(&mut self, scene: Arc<OverlayScene>, area: Rect) -> Result<(), String> {
        let width = area.width.max(1.0).round() as usize;
        let height = area.height.max(1.0).round() as usize;
        let bitmap_is_reusable = self.is_visible()
            && self.local_scene.as_ref().is_some_and(|cached| {
                scene_matches_local(scene.as_ref(), area, cached.scene.as_ref(), cached.area)
            })
            && self
                .dib
                .as_ref()
                .is_some_and(|surface| surface.width() == width && surface.height() == height);
        // `ensure_window` also translates an existing layered window. Do this
        // before the cache check so a pointer-follow scene still moves.
        let hwnd = self.ensure_window(area)?;
        self.visible = true;
        if bitmap_is_reusable {
            return Ok(());
        }

        self.draw(hwnd, scene.as_ref(), area)?;
        self.local_scene = Some(CachedScene { scene, area });
        // SAFETY: `hwnd` is valid.
        unsafe { UpdateWindow(hwnd) }
            .ok()
            .map_err(|error| format!("UpdateWindow failed: {error}"))?;
        Ok(())
    }

    pub fn dismiss(&mut self) -> Result<(), String> {
        let mut first_error = None;
        for (_, window) in self.windows.drain() {
            if let Err(error) = window.destroy()
                && first_error.is_none()
            {
                first_error = Some(format!("DestroyWindow failed: {error}"));
            }
        }
        self.local_scene = None;
        self.dib = None;
        self.text_rasterizer.clear();
        self.visible = false;
        first_error.map_or(Ok(()), Err)
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// Paint one scene directly into the persistent layered-window DIB.
    ///
    /// The scene arrives fully resolved — concrete colors, rects and font
    /// sizes — so this is the only place Windows rendering needs completing.
    fn draw(&mut self, hwnd: HWND, scene: &OverlayScene, area: Rect) -> Result<(), String> {
        let width = area.width.max(1.0).round() as usize;
        let height = area.height.max(1.0).round() as usize;
        let needs_surface = self
            .dib
            .as_ref()
            .is_none_or(|surface| surface.width() != width || surface.height() != height);
        if needs_surface {
            self.dib = Some(GdiDibSurface::new(
                None,
                NativeDimensions::from_usize(width, height)?,
            )?);
        }
        let Some(dib) = self.dib.as_mut() else {
            return Err("overlay DIB was not retained after creation".into());
        };
        // Clearing and then alpha-blending a full-screen backdrop performs two
        // complete memory passes. The DIB starts transparent, so writing the
        // premultiplied backdrop directly is exactly equivalent and halves the
        // fixed per-frame pixel work for grid scenes.
        if let Some(backdrop) = scene.backdrop {
            dib.clear_with(backdrop);
        } else {
            dib.clear();
        }
        let text_rasterizer = &mut self.text_rasterizer;
        let mut surface = Surface::new(dib.pixels_mut(), width, height, area.x, area.y);
        for shape in &scene.shapes {
            surface.shape(shape);
        }
        for label in &scene.labels {
            surface.label(label, text_rasterizer)?;
        }
        if let Some(marker) = &scene.cursor_marker {
            surface.shape(&OverlayShape::Rect {
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
                z_index: 10_000,
            });
        }
        if let Some(indicator) = &scene.indicator {
            let label_size = |text: &str| {
                let width = (text.chars().count() as f64 * indicator.style.font_size * 0.7
                    + indicator.style.padding_x * 2.0)
                    .max(indicator.style.font_size * 2.0);
                let height = indicator.style.font_size + indicator.style.padding_y * 2.0;
                (width, height)
            };
            let (width, height) = label_size(&indicator.text);
            surface.label_parts(
                &indicator.text,
                Rect::new(
                    indicator.position.x - width,
                    indicator.position.y,
                    width,
                    height,
                ),
                &indicator.style,
                0,
                text_rasterizer,
            )?;
            if let Some(held_text) = &indicator.held_text {
                let (held_width, held_height) = label_size(held_text);
                surface.label_parts(
                    held_text,
                    Rect::new(
                        indicator.position.x - held_width,
                        indicator.position.y + height + 4.0,
                        held_width,
                        held_height,
                    ),
                    &indicator.style,
                    0,
                    text_rasterizer,
                )?;
            }
        }
        upload(hwnd, area, dib)
    }
}

/// Convert absolute desktop coordinates to coordinates relative to the
/// layered window. Two cursor-indicator scenes that differ only by translation
/// become equal and can share the already-uploaded bitmap.
/// Resolve logical label metrics to the target monitor's device pixels. The
/// simple style is preserved; only its existing dimensions change. Compact
/// labels grow around their centre, while grid-cell labels keep their container
/// rectangle and cap the scale so text cannot overflow it.
#[derive(Default)]
pub(super) struct DpiSceneCache {
    scale_bits: Option<u64>,
    source_labels: Option<OverlayItems<OverlayLabel>>,
    scaled_labels: Option<OverlayItems<OverlayLabel>>,
}

impl DpiSceneCache {
    pub(super) fn clear(&mut self) {
        *self = Self::default();
    }

    pub(super) fn scene_for_dpi<'a>(
        &mut self,
        scene: &'a OverlayScene,
        scale: f64,
    ) -> Cow<'a, OverlayScene> {
        let scale = normalized_scale(scale);
        if (scale - 1.0).abs() < f64::EPSILON {
            self.clear();
            return Cow::Borrowed(scene);
        }

        let mut scaled = scene.clone();
        if scene.labels.is_empty() {
            // Entering a lightweight/empty scene must not leave the previous
            // high-DPI grid retained solely by this cache.
            self.clear();
        } else {
            let cached_labels = self
                .source_labels
                .as_ref()
                .zip(self.scaled_labels.as_ref())
                .filter(|(source, _)| {
                    self.scale_bits == Some(scale.to_bits())
                        && source.shares_storage_with(&scene.labels)
                })
                .map(|(_, labels)| labels.clone());
            if let Some(labels) = cached_labels {
                scaled.labels = labels;
            } else {
                scale_labels(&mut scaled.labels, scale);
                self.scale_bits = Some(scale.to_bits());
                self.source_labels = Some(scene.labels.clone());
                self.scaled_labels = Some(scaled.labels.clone());
            }
        }
        if let Some(indicator) = &mut scaled.indicator {
            scale_style(&mut indicator.style, scale);
            indicator.position.x = indicator.position.x.round();
            indicator.position.y = indicator.position.y.round();
        }
        Cow::Owned(scaled)
    }
}

#[cfg(test)]
fn scene_for_dpi(scene: &OverlayScene, scale: f64) -> Cow<'_, OverlayScene> {
    DpiSceneCache::default().scene_for_dpi(scene, scale)
}

fn normalized_scale(scale: f64) -> f64 {
    if scale.is_finite() {
        scale.max(1.0)
    } else {
        1.0
    }
}

fn scale_labels(labels: &mut OverlayItems<OverlayLabel>, scale: f64) {
    for label in labels {
        let chars = label.text.chars().count().max(1) as f64;
        let expected_width = label.style.font_size * 0.75 * chars + label.style.padding_x * 2.0;
        let expected_height = label.style.font_size * 1.4 + label.style.padding_y * 2.0;
        let compact = label.rect.width <= expected_width * 1.75
            && label.rect.height <= expected_height * 1.75;
        let effective = if compact {
            let center = label.rect.center();
            label.rect.width = (label.rect.width * scale).round().max(1.0);
            label.rect.height = (label.rect.height * scale).round().max(1.0);
            label.rect.x = (center.x - label.rect.width / 2.0).round();
            label.rect.y = (center.y - label.rect.height / 2.0).round();
            scale
        } else {
            scale.min(
                (label.rect.width * 0.8 / expected_width.max(1.0))
                    .min(label.rect.height * 0.8 / expected_height.max(1.0))
                    .max(1.0),
            )
        };
        scale_style(&mut label.style, effective);
    }
}

fn scale_style(style: &mut LabelStyle, scale: f64) {
    style.font_size = (style.font_size * scale).round().max(1.0);
    style.padding_x = (style.padding_x * scale).round();
    style.padding_y = (style.padding_y * scale).round();
    style.border_width = (style.border_width * scale).round().max(1.0);
    style.border_radius = (style.border_radius * scale).round();
}

fn scene_matches_local(
    scene: &OverlayScene,
    area: Rect,
    cached: &OverlayScene,
    cached_area: Rect,
) -> bool {
    let same_area = area == cached_area;
    scene.backdrop == cached.backdrop
        && scene.shapes.len() == cached.shapes.len()
        && ((same_area && scene.shapes.shares_storage_with(&cached.shapes))
            || scene
                .shapes
                .iter()
                .zip(&cached.shapes)
                .all(|(shape, cached)| shape_matches_local(shape, area, cached, cached_area)))
        && scene.labels.len() == cached.labels.len()
        && ((same_area && scene.labels.shares_storage_with(&cached.labels))
            || scene
                .labels
                .iter()
                .zip(&cached.labels)
                .all(|(label, cached)| {
                    label.text == cached.text
                        && rect_relative_to_area(label.rect, area)
                            == rect_relative_to_area(cached.rect, cached_area)
                        && label.style == cached.style
                        && label.matched_prefix_len == cached.matched_prefix_len
                        && label.z_index == cached.z_index
                        && label.fit_to_text == cached.fit_to_text
                }))
        && match (&scene.cursor_marker, &cached.cursor_marker) {
            (Some(marker), Some(cached)) => {
                point_relative_to_area(marker.center, area)
                    == point_relative_to_area(cached.center, cached_area)
                    && marker.radius == cached.radius
                    && marker.fill == cached.fill
                    && marker.stroke == cached.stroke
                    && marker.stroke_width == cached.stroke_width
            }
            (None, None) => true,
            _ => false,
        }
        && match (&scene.indicator, &cached.indicator) {
            (Some(indicator), Some(cached)) => {
                indicator.text == cached.text
                    && indicator.held_text == cached.held_text
                    && point_relative_to_area(indicator.position, area)
                        == point_relative_to_area(cached.position, cached_area)
                    && indicator.style == cached.style
            }
            (None, None) => true,
            _ => false,
        }
        && scene.clip.map(|clip| rect_relative_to_area(clip, area))
            == cached
                .clip
                .map(|clip| rect_relative_to_area(clip, cached_area))
}

fn shape_matches_local(
    shape: &OverlayShape,
    area: Rect,
    cached: &OverlayShape,
    cached_area: Rect,
) -> bool {
    match (shape, cached) {
        (
            OverlayShape::Rect {
                rect,
                fill,
                stroke,
                stroke_width,
                corner_radius,
                z_index,
            },
            OverlayShape::Rect {
                rect: cached_rect,
                fill: cached_fill,
                stroke: cached_stroke,
                stroke_width: cached_stroke_width,
                corner_radius: cached_corner_radius,
                z_index: cached_z_index,
            },
        ) => {
            rect_relative_to_area(*rect, area) == rect_relative_to_area(*cached_rect, cached_area)
                && fill == cached_fill
                && stroke == cached_stroke
                && stroke_width == cached_stroke_width
                && corner_radius == cached_corner_radius
                && z_index == cached_z_index
        }
        (
            OverlayShape::Line {
                from,
                to,
                color,
                width,
                z_index,
            },
            OverlayShape::Line {
                from: cached_from,
                to: cached_to,
                color: cached_color,
                width: cached_width,
                z_index: cached_z_index,
            },
        ) => {
            point_relative_to_area(*from, area) == point_relative_to_area(*cached_from, cached_area)
                && point_relative_to_area(*to, area)
                    == point_relative_to_area(*cached_to, cached_area)
                && color == cached_color
                && width == cached_width
                && z_index == cached_z_index
        }
        _ => false,
    }
}

fn point_relative_to_area(point: Point, area: Rect) -> Point {
    Point::new(point.x - area.x, point.y - area.y)
}

fn rect_relative_to_area(rect: Rect, area: Rect) -> Rect {
    Rect::new(rect.x - area.x, rect.y - area.y, rect.width, rect.height)
}

fn fitted_label_rect(text: &str, container: Rect, style: &LabelStyle) -> Rect {
    let characters = text.chars().count().max(1) as f64;
    let width = (style.font_size * 0.75 * characters + style.padding_x.max(0.0) * 2.0)
        .ceil()
        .clamp(1.0, container.width.max(1.0));
    let height = (style.font_size * 1.4 + style.padding_y.max(0.0) * 2.0)
        .ceil()
        .clamp(1.0, container.height.max(1.0));
    Rect::new(
        container.center().x - width / 2.0,
        container.center().y - height / 2.0,
        width,
        height,
    )
}

struct Surface<'a> {
    width: usize,
    height: usize,
    origin_x: f64,
    origin_y: f64,
    pixels: &'a mut [u8],
}

impl<'a> Surface<'a> {
    fn new(
        pixels: &'a mut [u8],
        width: usize,
        height: usize,
        origin_x: f64,
        origin_y: f64,
    ) -> Self {
        debug_assert_eq!(pixels.len(), width.saturating_mul(height).saturating_mul(4));
        Self {
            width,
            height,
            origin_x,
            origin_y,
            pixels,
        }
    }

    #[inline]
    fn blend_pixel(pixel: &mut [u8], color: Color, coverage: u8) {
        let alpha = color.a as u32 * coverage as u32 / 255;
        if alpha == 0 {
            return;
        }
        let inv = 255 - alpha;
        let source = [
            color.b as u32 * alpha / 255,
            color.g as u32 * alpha / 255,
            color.r as u32 * alpha / 255,
        ];
        for channel in 0..3 {
            pixel[channel] = (source[channel] + pixel[channel] as u32 * inv / 255).min(255) as u8;
        }
        pixel[3] = (alpha + pixel[3] as u32 * inv / 255).min(255) as u8;
    }

    fn solid_blend_tables(color: Color) -> [[u8; 256]; 4] {
        let alpha = color.a as u32;
        let inv = 255 - alpha;
        let source = [
            color.b as u32 * alpha / 255,
            color.g as u32 * alpha / 255,
            color.r as u32 * alpha / 255,
        ];
        let mut tables = [[0; 256]; 4];
        for destination in 0..=255_u32 {
            for channel in 0..3 {
                tables[channel][destination as usize] =
                    (source[channel] + destination * inv / 255).min(255) as u8;
            }
            tables[3][destination as usize] = (alpha + destination * inv / 255).min(255) as u8;
        }
        tables
    }

    #[inline]
    fn blend_solid_span(pixels: &mut [u8], tables: &[[u8; 256]; 4]) {
        for pixel in pixels.chunks_exact_mut(4) {
            for channel in 0..3 {
                pixel[channel] = tables[channel][pixel[channel] as usize];
            }
            pixel[3] = tables[3][pixel[3] as usize];
        }
    }

    #[inline]
    fn blend(&mut self, x: i32, y: i32, color: Color, coverage: u8) {
        if x < 0 || y < 0 || x as usize >= self.width || y as usize >= self.height {
            return;
        }
        let index = (y as usize * self.width + x as usize) * 4;
        Self::blend_pixel(&mut self.pixels[index..index + 4], color, coverage);
    }

    fn local_rect(&self, rect: Rect) -> Rect {
        Rect::new(
            rect.x - self.origin_x,
            rect.y - self.origin_y,
            rect.width,
            rect.height,
        )
    }

    fn fill(&mut self, rect: Rect, color: Color, radius: f64) {
        if color.is_transparent() {
            return;
        }
        let rect = self.local_rect(rect);
        let left = rect.x.floor() as i32;
        let top = rect.y.floor() as i32;
        let right = (rect.x + rect.width).ceil() as i32;
        let bottom = (rect.y + rect.height).ceil() as i32;
        let radius = radius.max(0.0).min(rect.width.min(rect.height) / 2.0);
        let clipped_left = left.clamp(0, self.width as i32);
        let clipped_right = right.clamp(0, self.width as i32);
        let clipped_top = top.clamp(0, self.height as i32);
        let clipped_bottom = bottom.clamp(0, self.height as i32);
        if clipped_left >= clipped_right || clipped_top >= clipped_bottom {
            return;
        }

        // Rectangular grid fills and rulings dominate the hot path. Blend one
        // clipped row-span at a time so bounds, source alpha, and premultiplied
        // channels are calculated once rather than once per pixel.
        if radius == 0.0 {
            let left = clipped_left as usize;
            let right = clipped_right as usize;
            if color.a == 255 {
                let pixel = [color.b, color.g, color.r, 255];
                for y in clipped_top as usize..clipped_bottom as usize {
                    let start = (y * self.width + left) * 4;
                    let end = (y * self.width + right) * 4;
                    for target in self.pixels[start..end].chunks_exact_mut(4) {
                        target.copy_from_slice(&pixel);
                    }
                }
            } else {
                // A 1 KiB stack lookup table replaces four integer divisions
                // per pixel with byte lookups. Grid backgrounds cover millions
                // of pixels, so the 256-entry setup cost is quickly amortised.
                let tables = Self::solid_blend_tables(color);
                for y in clipped_top as usize..clipped_bottom as usize {
                    let start = (y * self.width + left) * 4;
                    let end = (y * self.width + right) * 4;
                    Self::blend_solid_span(&mut self.pixels[start..end], &tables);
                }
            }
            return;
        }

        for y in clipped_top..clipped_bottom {
            for x in clipped_left..clipped_right {
                let cx = (x as f64 + 0.5 - rect.x).clamp(radius, (rect.width - radius).max(radius));
                let cy =
                    (y as f64 + 0.5 - rect.y).clamp(radius, (rect.height - radius).max(radius));
                let dx = x as f64 + 0.5 - rect.x - cx;
                let dy = y as f64 + 0.5 - rect.y - cy;
                if dx * dx + dy * dy > radius * radius {
                    continue;
                }
                self.blend(x, y, color, 255);
            }
        }
    }

    fn stroke_rect(&mut self, rect: Rect, color: Color, width: f64, radius: f64) {
        if color.is_transparent() || width <= 0.0 {
            return;
        }
        if radius > 0.0 {
            let rect = self.local_rect(rect);
            let inner = rect.inset(width, width);
            let radius = radius.min(rect.width.min(rect.height) / 2.0);
            let inner_radius = (radius - width).max(0.0);
            let contains = |bounds: Rect, corner: f64, x: f64, y: f64| {
                if x < bounds.left()
                    || x >= bounds.right()
                    || y < bounds.top()
                    || y >= bounds.bottom()
                {
                    return false;
                }
                let cx = x.clamp(bounds.left() + corner, bounds.right() - corner);
                let cy = y.clamp(bounds.top() + corner, bounds.bottom() - corner);
                let dx = x - cx;
                let dy = y - cy;
                dx * dx + dy * dy <= corner * corner
            };
            for y in rect.top().floor() as i32..rect.bottom().ceil() as i32 {
                for x in rect.left().floor() as i32..rect.right().ceil() as i32 {
                    let px = x as f64 + 0.5;
                    let py = y as f64 + 0.5;
                    if contains(rect, radius, px, py) && !contains(inner, inner_radius, px, py) {
                        self.blend(x, y, color, 255);
                    }
                }
            }
            return;
        }
        self.fill(Rect::new(rect.x, rect.y, rect.width, width), color, 0.0);
        self.fill(
            Rect::new(rect.x, rect.bottom() - width, rect.width, width),
            color,
            0.0,
        );
        self.fill(Rect::new(rect.x, rect.y, width, rect.height), color, 0.0);
        self.fill(
            Rect::new(rect.right() - width, rect.y, width, rect.height),
            color,
            0.0,
        );
    }

    fn line(&mut self, from: Point, to: Point, color: Color, width: f64) {
        let width = width.max(1.0);
        let half = width / 2.0;
        if (from.x - to.x).abs() < f64::EPSILON {
            self.fill(
                Rect::new(
                    from.x - half,
                    from.y.min(to.y) - half,
                    width,
                    (to.y - from.y).abs() + width,
                ),
                color,
                0.0,
            );
            return;
        }
        if (from.y - to.y).abs() < f64::EPSILON {
            self.fill(
                Rect::new(
                    from.x.min(to.x) - half,
                    from.y - half,
                    (to.x - from.x).abs() + width,
                    width,
                ),
                color,
                0.0,
            );
            return;
        }

        let from = Point::new(from.x - self.origin_x, from.y - self.origin_y);
        let to = Point::new(to.x - self.origin_x, to.y - self.origin_y);
        let left = (from.x.min(to.x) - half).floor() as i32;
        let right = (from.x.max(to.x) + half).ceil() as i32;
        let top = (from.y.min(to.y) - half).floor() as i32;
        let bottom = (from.y.max(to.y) + half).ceil() as i32;
        let vx = to.x - from.x;
        let vy = to.y - from.y;
        let length_sq = vx * vx + vy * vy;
        for y in top..=bottom {
            for x in left..=right {
                let px = x as f64 + 0.5 - from.x;
                let py = y as f64 + 0.5 - from.y;
                let t = if length_sq == 0.0 {
                    0.0
                } else {
                    ((px * vx + py * vy) / length_sq).clamp(0.0, 1.0)
                };
                let dx = px - vx * t;
                let dy = py - vy * t;
                if dx * dx + dy * dy <= half * half {
                    self.blend(x, y, color, 255);
                }
            }
        }
    }

    fn shape(&mut self, shape: &OverlayShape) {
        match shape {
            OverlayShape::Rect {
                rect,
                fill,
                stroke,
                stroke_width,
                corner_radius,
                ..
            } => {
                self.fill(*rect, *fill, *corner_radius);
                self.stroke_rect(*rect, *stroke, *stroke_width, *corner_radius);
            }
            OverlayShape::Line {
                from,
                to,
                color,
                width,
                ..
            } => self.line(*from, *to, *color, *width),
        }
    }

    fn label(
        &mut self,
        label: &OverlayLabel,
        rasterizer: &mut TextRasterizer,
    ) -> Result<(), String> {
        let rect = if label.fit_to_text {
            fitted_label_rect(&label.text, label.rect, &label.style)
        } else {
            label.rect
        };
        self.label_parts(
            &label.text,
            rect,
            &label.style,
            label.matched_prefix_len,
            rasterizer,
        )
    }

    fn label_parts(
        &mut self,
        text: &str,
        rect: Rect,
        style: &LabelStyle,
        matched_prefix_len: usize,
        rasterizer: &mut TextRasterizer,
    ) -> Result<(), String> {
        self.fill(rect, style.background, style.border_radius);
        self.stroke_rect(
            rect,
            style.border_color,
            style.border_width,
            style.border_radius,
        );
        self.text(text, rect, style, matched_prefix_len, rasterizer)
    }

    fn text(
        &mut self,
        text: &str,
        label_rect: Rect,
        style: &LabelStyle,
        matched_prefix_len: usize,
        rasterizer: &mut TextRasterizer,
    ) -> Result<(), String> {
        let rect = self.local_rect(label_rect);
        let width = rect.width.max(1.0).ceil() as usize;
        let height = rect.height.max(1.0).ceil() as usize;
        let (mask, matched_boundary) =
            rasterizer.rasterize(text, style, width, height, matched_prefix_len)?;
        for y in 0..height {
            for x in 0..width {
                let coverage = mask[y * width + x];
                if coverage == 0 {
                    continue;
                }
                let color = if matched_boundary.is_some_and(|boundary| x < boundary) {
                    style.matched_text_color
                } else {
                    style.text_color
                };
                self.blend(
                    rect.x.floor() as i32 + x as i32,
                    rect.y.floor() as i32 + y as i32,
                    color,
                    coverage,
                );
            }
        }
        Ok(())
    }
}

const MAX_CACHED_FONTS: usize = 8;

#[derive(PartialEq, Eq)]
struct FontKey {
    family: String,
    pixel_height: i32,
    bold: bool,
}

struct FontEntry {
    key: FontKey,
    font: OwnedFont,
}

struct OwnedFont(HFONT);

impl OwnedFont {
    fn new(key: &FontKey) -> Result<Self, String> {
        let family: Vec<u16> = key.family.encode_utf16().chain(Some(0)).collect();
        // SAFETY: `family` is NUL-terminated and remains live for the complete
        // CreateFontW call; all numeric parameters are initialized values.
        let font = unsafe {
            CreateFontW(
                -key.pixel_height,
                0,
                0,
                0,
                if key.bold {
                    FW_BOLD.0 as i32
                } else {
                    FW_NORMAL.0 as i32
                },
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                ANTIALIASED_QUALITY,
                FF_DONTCARE.0 as u32,
                PCWSTR(family.as_ptr()),
            )
        };
        if font.is_invalid() {
            Err("CreateFontW failed".into())
        } else {
            Ok(Self(font))
        }
    }
}

impl Drop for OwnedFont {
    fn drop(&mut self) {
        // SAFETY: this HFONT came from CreateFontW and this guard is its sole
        // owner, so DeleteObject is called exactly once.
        if !unsafe { DeleteObject(self.0.into()) }.as_bool() {
            crate::app::logging::report_error(
                "windows-overlay",
                "DeleteObject(font) failed during drop",
            );
        }
    }
}

struct TextRasterizer {
    scratch: Option<GdiDibSurface>,
    fonts: Vec<FontEntry>,
    utf16: Vec<u16>,
    advances: Vec<i32>,
    mask: Vec<u8>,
}

impl TextRasterizer {
    fn new() -> Self {
        Self {
            scratch: None,
            fonts: Vec::with_capacity(MAX_CACHED_FONTS),
            utf16: Vec::new(),
            advances: Vec::new(),
            mask: Vec::new(),
        }
    }

    fn clear(&mut self) {
        self.scratch = None;
        self.fonts.clear();
        self.utf16 = Vec::new();
        self.advances = Vec::new();
        self.mask = Vec::new();
    }

    fn rasterize(
        &mut self,
        text: &str,
        style: &LabelStyle,
        width: usize,
        height: usize,
        matched_prefix_len: usize,
    ) -> Result<(&[u8], Option<usize>), String> {
        self.ensure_scratch(width, height)?;
        self.utf16.clear();
        self.utf16.extend(text.encode_utf16());
        let analysis = LabelTextAnalysis::analyze(text, matched_prefix_len);

        let family = if style.font_family.is_empty() {
            "Segoe UI"
        } else {
            &style.font_family
        };
        let pixel_height = (style.font_size.round() as i32).max(1);
        let font = if let Some(index) = self.fonts.iter().position(|entry| {
            entry.key.family == family
                && entry.key.pixel_height == pixel_height
                && entry.key.bold == style.bold
        }) {
            self.fonts.remove(index)
        } else {
            let key = FontKey {
                family: family.into(),
                pixel_height,
                bold: style.bold,
            };
            FontEntry {
                font: OwnedFont::new(&key)?,
                key,
            }
        };

        let Some(scratch) = self.scratch.as_mut() else {
            return Err("text scratch DIB was not retained after creation".into());
        };
        scratch.clear_region(width, height);
        let mut matched_boundary = None;
        {
            let _selected_font = scratch.select_object(font.font.0.into())?;
            // SAFETY: the selected DC and font remain alive for this scope and
            // the color/background operations do not retain pointers.
            unsafe {
                if SetBkMode(scratch.dc(), TRANSPARENT) == 0 {
                    crate::app::logging::report_error(
                        "windows-overlay",
                        "SetBkMode failed while drawing text",
                    );
                }
                SetTextColor(scratch.dc(), COLORREF(0x00FF_FFFF));
            }
            let text_offset_y = super::label_text_offset_y(style, analysis).round() as i32;
            let mut draw_rect = windows::Win32::Foundation::RECT {
                left: 0,
                top: text_offset_y,
                right: width as i32,
                bottom: height as i32 + text_offset_y,
            };
            // SAFETY: `utf16`, `draw_rect`, and the selected scratch DC remain
            // valid and writable for this synchronous text draw.
            unsafe {
                let prefix_utf16_len = analysis.matched_utf16_len;
                if prefix_utf16_len > 0 {
                    let utf16_len = i32::try_from(self.utf16.len())
                        .map_err(|_| "overlay text is too long for GDI measurement")?;
                    self.advances.resize(self.utf16.len(), 0);
                    let mut text_size = SIZE::default();
                    if !GetTextExtentExPointW(
                        scratch.dc(),
                        PCWSTR(self.utf16.as_ptr()),
                        utf16_len,
                        i32::MAX,
                        None,
                        Some(self.advances.as_mut_ptr()),
                        &mut text_size,
                    )
                    .as_bool()
                    {
                        return Err("GetTextExtentExPointW failed while measuring hint text".into());
                    }
                    let prefix_width = self
                        .advances
                        .get(prefix_utf16_len - 1)
                        .copied()
                        .ok_or("GDI returned no matched-prefix advance")?;
                    let left = (width as i32 - text_size.cx) / 2;
                    matched_boundary = Some((left + prefix_width).clamp(0, width as i32) as usize);
                }
                DrawTextW(
                    scratch.dc(),
                    &mut self.utf16,
                    &mut draw_rect,
                    DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
                );
            }
        }

        if self.fonts.len() == MAX_CACHED_FONTS {
            self.fonts.remove(0);
        }
        self.fonts.push(font);

        self.mask.resize(width.saturating_mul(height), 0);
        let stride = scratch.width();
        let pixels = scratch.pixels();
        for y in 0..height {
            let source = &pixels[y * stride * 4..(y * stride + width) * 4];
            let target = &mut self.mask[y * width..(y + 1) * width];
            for (coverage, pixel) in target.iter_mut().zip(source.chunks_exact(4)) {
                *coverage = pixel[0].max(pixel[1]).max(pixel[2]);
            }
        }
        Ok((&self.mask, matched_boundary))
    }

    fn ensure_scratch(&mut self, width: usize, height: usize) -> Result<(), String> {
        let reusable = self
            .scratch
            .as_ref()
            .is_some_and(|surface| surface.width() >= width && surface.height() >= height);
        if !reusable {
            let old_width = self.scratch.as_ref().map_or(0, GdiDibSurface::width);
            let old_height = self.scratch.as_ref().map_or(0, GdiDibSurface::height);
            self.scratch = Some(GdiDibSurface::new(
                None,
                NativeDimensions::from_usize(old_width.max(width), old_height.max(height))?,
            )?);
        }
        Ok(())
    }
}

trait DibSurfaceExt {
    fn clear(&mut self);
    fn clear_with(&mut self, color: Color);
    fn clear_region(&mut self, width: usize, height: usize);
}

impl DibSurfaceExt for GdiDibSurface {
    fn clear(&mut self) {
        self.pixels_mut().fill(0);
    }

    fn clear_with(&mut self, color: Color) {
        if color.is_transparent() {
            self.clear();
            return;
        }
        let [red, green, blue, alpha] = color.premultiplied();
        let pixel = [blue, green, red, alpha];
        for target in self.pixels_mut().chunks_exact_mut(4) {
            target.copy_from_slice(&pixel);
        }
    }

    fn clear_region(&mut self, width: usize, height: usize) {
        debug_assert!(width <= self.width() && height <= self.height());
        let stride = self.width();
        let pixels = self.pixels_mut();
        for row in pixels.chunks_exact_mut(stride * 4).take(height) {
            row[..width * 4].fill(0);
        }
    }
}

fn upload(hwnd: HWND, area: Rect, surface: &GdiDibSurface) -> Result<(), String> {
    let screen = super::native::ScreenDc::acquire()?;
    let destination = POINT {
        x: area.x.round() as i32,
        y: area.y.round() as i32,
    };
    let size = SIZE {
        cx: surface.width() as i32,
        cy: surface.height() as i32,
    };
    let source = POINT { x: 0, y: 0 };
    let blend = BLENDFUNCTION {
        BlendOp: AC_SRC_OVER as u8,
        BlendFlags: 0,
        SourceConstantAlpha: 255,
        AlphaFormat: AC_SRC_ALPHA as u8,
    };
    // SAFETY: all handles and stack structures remain live for the synchronous
    // upload, and the validated DIB exactly matches the supplied size.
    unsafe {
        UpdateLayeredWindow(
            hwnd,
            Some(screen.raw()),
            Some(&destination),
            Some(&size),
            Some(surface.dc()),
            Some(&source),
            COLORREF(0),
            Some(&blend),
            ULW_ALPHA,
        )
    }
    .map_err(|e| format!("UpdateLayeredWindow failed: {e}"))
}

impl Default for Overlay {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        if let Err(error) = self.dismiss() {
            crate::app::logging::report_error(
                "windows-overlay",
                format!("cannot destroy overlay during drop: {error}"),
            );
        }
    }
}

extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if let Some(result) = super::native::click_through_hit_test(message) {
        return result;
    }
    match message {
        // Painting happens in `draw`; acknowledge to keep the queue clear.
        WM_PAINT => {
            // SAFETY: `hwnd` and the paint region were supplied by Windows for
            // this callback; no Rust pointer is retained.
            if !unsafe { ValidateRect(Some(hwnd), None) }.as_bool() {
                crate::app::logging::report_error(
                    "windows-overlay",
                    "ValidateRect failed for overlay window",
                );
            }
            LRESULT(0)
        }
        WM_DESTROY => LRESULT(0),
        _ => super::native::default_window_proc(hwnd, message, wparam, lparam),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgba_blending_is_premultiplied_bgra() {
        let mut pixels = vec![0; 4];
        let mut surface = Surface::new(&mut pixels, 1, 1, 0.0, 0.0);
        surface.blend(0, 0, Color::rgba(0x80, 0x40, 0x20, 0x80), 255);
        assert_eq!(surface.pixels, [0x10, 0x20, 0x40, 0x80]);
    }

    #[test]
    fn solid_blend_lookup_matches_the_per_pixel_formula() {
        let color = Color::rgba(0x91, 0x52, 0x17, 0x9D);
        let tables = Surface::solid_blend_tables(color);
        for destination in 0..=255_u8 {
            let mut expected = [destination, 255 - destination, destination / 2, destination];
            let mut actual = expected;
            Surface::blend_pixel(&mut expected, color, 255);
            Surface::blend_solid_span(&mut actual, &tables);
            assert_eq!(actual, expected, "destination {destination}");
        }
    }

    #[test]
    fn negative_desktop_coordinates_match_the_pixel_snapshot() {
        let mut pixels = vec![0; 6 * 4 * 4];
        let mut surface = Surface::new(&mut pixels, 6, 4, -3.0, -2.0);
        surface.fill(
            Rect::new(-2.0, -1.0, 3.0, 2.0),
            Color::rgba(0xFF, 0x00, 0x00, 0xFF),
            0.0,
        );
        let snapshot = (0..surface.height)
            .map(|y| {
                (0..surface.width)
                    .map(|x| {
                        if surface.pixels[(y * surface.width + x) * 4 + 3] == 0 {
                            '0'
                        } else {
                            '1'
                        }
                    })
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        assert_eq!(snapshot, ["000000", "011100", "011100", "000000"]);
    }

    #[test]
    fn rounded_stroke_is_a_transparent_ring() {
        let mut pixels = vec![0; 9 * 9 * 4];
        let mut surface = Surface::new(&mut pixels, 9, 9, 0.0, 0.0);
        surface.stroke_rect(
            Rect::new(1.0, 1.0, 7.0, 7.0),
            Color::rgba(0x40, 0x80, 0xFF, 0xFF),
            1.0,
            3.5,
        );
        let alpha = |x: usize, y: usize| surface.pixels[(y * surface.width + x) * 4 + 3];
        assert_eq!(alpha(1, 1), 0, "square corner must stay transparent");
        assert!(alpha(4, 1) > 0, "top of the ring must be visible");
        assert_eq!(alpha(4, 4), 0, "ring centre must stay transparent");
    }

    #[test]
    fn fitted_labels_rasterise_only_the_text_sized_center() {
        let container = Rect::new(10.0, 20.0, 400.0, 300.0);
        let fitted = fitted_label_rect("1", container, &LabelStyle::default());

        assert_eq!(fitted.center(), container.center());
        assert_eq!(fitted.width, 16.0);
        assert_eq!(fitted.height, 18.0);
        assert!(fitted.width * fitted.height < container.width * container.height / 100.0);
    }

    #[test]
    fn axis_aligned_line_uses_the_rectangular_fast_path() {
        let mut pixels = vec![0; 7 * 7 * 4];
        let mut surface = Surface::new(&mut pixels, 7, 7, 0.0, 0.0);
        surface.line(
            Point::new(3.0, 1.0),
            Point::new(3.0, 5.0),
            Color::rgb(255, 255, 255),
            1.0,
        );
        let alpha = |x: usize, y: usize| surface.pixels[(y * surface.width + x) * 4 + 3];

        assert_eq!(alpha(1, 3), 0);
        assert_eq!(alpha(2, 3), 255);
        assert_eq!(alpha(3, 3), 255);
        assert_eq!(alpha(4, 3), 0);
        assert_eq!(alpha(2, 0), 255, "flat grid line reaches its first edge");
        assert_eq!(alpha(2, 5), 255, "flat grid line reaches its last edge");
        assert_eq!(alpha(2, 6), 0);
    }

    #[test]
    fn translated_cursor_scene_reuses_the_same_local_bitmap() {
        fn cursor_scene(cursor: Point) -> (OverlayScene, Rect) {
            let area = Rect::new(cursor.x - 40.0, cursor.y - 20.0, 100.0, 60.0);
            let mut scene = OverlayScene::new();
            scene.clip = Some(area);
            scene.shapes.push(OverlayShape::outline(
                Rect::new(cursor.x - 8.0, cursor.y - 8.0, 16.0, 16.0),
                Color::rgb(20, 40, 80),
                2.0,
            ));
            scene.indicator = Some(crate::api::overlay::Indicator {
                text: "Normal".into(),
                held_text: None,
                position: Point::new(cursor.x - 12.0, cursor.y + 18.0),
                style: LabelStyle::default(),
            });
            (scene, area)
        }

        let (first, first_area) = cursor_scene(Point::new(-300.0, 500.0));
        let (second, second_area) = cursor_scene(Point::new(1700.0, 900.0));
        assert!(scene_matches_local(
            &first,
            first_area,
            &second,
            second_area
        ));
        assert!(scene_matches_local(
            &second,
            second_area,
            &first,
            first_area
        ));
    }

    #[test]
    fn native_dpi_borrows_the_existing_scene() {
        let scene = OverlayScene::new();
        assert!(matches!(scene_for_dpi(&scene, 1.0), Cow::Borrowed(_)));
    }

    #[test]
    fn compact_labels_are_rasterised_at_monitor_dpi() {
        let mut scene = OverlayScene::new();
        scene.labels.push(OverlayLabel::new(
            "AA",
            Rect::new(100.0, 100.0, 24.0, 16.0),
            LabelStyle::default(),
        ));
        let scaled = scene_for_dpi(&scene, 2.0);
        assert_eq!(scaled.labels[0].style.font_size, 20.0);
        assert_eq!(
            scaled.labels[0].rect.center(),
            scene.labels[0].rect.center()
        );
        assert!(scaled.labels[0].rect.width > scene.labels[0].rect.width);
    }

    #[test]
    fn cell_labels_keep_their_container_at_high_dpi() {
        let mut scene = OverlayScene::new();
        scene.labels.push(OverlayLabel::new(
            "1",
            Rect::new(0.0, 0.0, 400.0, 300.0),
            LabelStyle::default(),
        ));
        let scaled = scene_for_dpi(&scene, 2.0);
        assert_eq!(scaled.labels[0].rect, scene.labels[0].rect);
        assert_eq!(scaled.labels[0].style.font_size, 20.0);
    }

    #[test]
    fn dpi_cache_reuses_scaled_static_labels_and_releases_them() {
        let mut source = OverlayScene::new();
        source.labels.push(OverlayLabel::new(
            "AA",
            Rect::new(100.0, 100.0, 24.0, 16.0),
            LabelStyle::default(),
        ));
        let mut cache = DpiSceneCache::default();
        let first = cache.scene_for_dpi(&source, 1.5).into_owned();

        let mut next_frame = source.clone();
        next_frame.cursor_marker = Some(crate::api::overlay::CursorMarker {
            center: Point::new(10.0, 20.0),
            radius: 8.0,
            fill: Color::TRANSPARENT,
            stroke: Color::rgb(1, 2, 3),
            stroke_width: 1.0,
        });
        let second = cache.scene_for_dpi(&next_frame, 1.5).into_owned();

        assert!(first.labels.shares_storage_with(&second.labels));
        let empty = OverlayScene::new();
        let _ = cache.scene_for_dpi(&empty, 1.5);
        assert!(cache.source_labels.is_none());
        assert!(cache.scaled_labels.is_none());
    }
}
