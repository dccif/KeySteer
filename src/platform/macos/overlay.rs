//! Typed AppKit/Core Animation overlay renderer.
//!
//! The main thread owns an `NSPanel`, `NSView` and a reusable CALayer tree.
//! There are no raw Objective-C object pointers or manual retain/release calls
//! in this module; `Retained<T>` and Core Foundation retained wrappers encode
//! every native lifetime.

use std::collections::HashMap;
use std::sync::Arc;

use objc2::rc::{Retained, autoreleasepool};
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSPanel, NSScreenSaverWindowLevel, NSView,
    NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_core_foundation::{CFRetained, CFString};
use objc2_core_graphics::{CGColor, CGMutablePath};
use objc2_core_text::{CTFont, CTFontSymbolicTraits, CTFontUIFontType};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use objc2_quartz_core::{CALayer, CAShapeLayer, CATextLayer, CATransaction, kCAAlignmentCenter};

use crate::api::geometry::{Point, Rect, Screen};
use crate::api::overlay::{
    Color, CursorMarker, Indicator, LabelStyle, OverlayLabel, OverlayScene, OverlayShape,
};

const MAX_CACHED_COLORS: usize = 64;

/// A main-thread-only transparent overlay. Native objects are created lazily,
/// which keeps unit tests independent of a WindowServer session.
pub struct Overlay {
    scene: Option<Arc<OverlayScene>>,
    area: Option<Rect>,
    visible: bool,
    content: Option<WindowContent>,
    static_updates: u64,
}

struct WindowContent {
    window: Retained<NSPanel>,
    root_view: Retained<NSView>,
    root_layer: Retained<CALayer>,
    shapes: Vec<Retained<CAShapeLayer>>,
    labels: Vec<LabelLayers>,
    cursor: Retained<CALayer>,
    colors: HashMap<u32, Retained<CGColor>>,
    fonts: Vec<FontEntry>,
    scale: f64,
}

struct LabelLayers {
    base: Retained<CATextLayer>,
    matched: Retained<CATextLayer>,
    mask: Retained<CALayer>,
    text: String,
}

struct FontEntry {
    family: String,
    size_bits: u64,
    bold: bool,
    font: CFRetained<CTFont>,
}

struct LabelSpec<'a> {
    text: &'a str,
    rect: Rect,
    style: &'a LabelStyle,
    matched_prefix_len: usize,
    z_index: i32,
}

impl Overlay {
    pub fn new() -> Self {
        Self {
            scene: None,
            area: None,
            visible: false,
            content: None,
            static_updates: 0,
        }
    }

    pub fn present(&mut self, scene: Arc<OverlayScene>) -> Result<(), String> {
        autoreleasepool(|_| self.present_inner(scene))
    }

    fn present_inner(&mut self, scene: Arc<OverlayScene>) -> Result<(), String> {
        if self.visible && self.scene.as_ref() == Some(&scene) {
            return Ok(());
        }
        let area = match scene.clip {
            Some(area) if !area.is_empty() => area,
            _ if cfg!(test) => Rect::new(0.0, 0.0, 1920.0, 1080.0),
            _ => render_area()?,
        };
        let static_changed = self.area != Some(area)
            || self
                .scene
                .as_deref()
                .is_none_or(|previous| !static_scene_eq(previous, scene.as_ref()));

        if !cfg!(test) {
            self.ensure_window(area)?;
            let content = self
                .content
                .as_mut()
                .ok_or("macOS overlay window was not retained after creation")?;
            let _transaction = DisabledActions::begin();
            if static_changed {
                content.update_static(scene.as_ref(), area);
            }
            content.update_dynamic(scene.as_ref(), area);
            content.window.orderFrontRegardless();
        }
        if static_changed {
            self.static_updates = self.static_updates.wrapping_add(1);
        }
        self.area = Some(area);
        self.scene = Some(scene);
        self.visible = true;
        Ok(())
    }

    pub fn dismiss(&mut self) -> Result<(), String> {
        autoreleasepool(|_| self.dismiss_inner())
    }

    fn dismiss_inner(&mut self) -> Result<(), String> {
        if let Some(mut content) = self.content.take() {
            {
                let _transaction = DisabledActions::begin();
                content.teardown();
            }
            // Keep the typed owners alive until the detach transaction has
            // committed; dropping `content` after this point releases them.
        }
        self.scene = None;
        self.area = None;
        self.visible = false;
        Ok(())
    }

    #[cfg(test)]
    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// The root view is a stable, typed display-link source for the lifetime
    /// of the visible overlay.
    pub fn display_link_source(&self) -> Result<&NSView, String> {
        self.content
            .as_ref()
            .map(|content| content.root_view.as_ref())
            .ok_or_else(|| "macOS overlay layer tree is not initialized".to_string())
    }

    fn ensure_window(&mut self, area: Rect) -> Result<(), String> {
        let frame = cocoa_frame(area)?;
        let view_frame = NSRect::new(NSPoint::new(0.0, 0.0), frame.size);
        if let Some(content) = self.content.as_mut() {
            // Reassert the WindowServer-level click-through invariant whenever
            // a retained panel is reused. This does not depend on AppKit
            // dispatching a hit-test message while the main thread is busy.
            content.window.setIgnoresMouseEvents(true);
            content.window.setFrame_display(frame, false);
            content.root_view.setFrame(view_frame);
            content.root_layer.setFrame(view_frame);
            content.scale = content.window.backingScaleFactor().max(1.0);
            return Ok(());
        }

        let mtm = MainThreadMarker::new()
            .ok_or_else(|| "macOS overlay must be created on the main thread".to_string())?;
        let window = NSPanel::initWithContentRect_styleMask_backing_defer(
            NSPanel::alloc(mtm),
            frame,
            NSWindowStyleMask::NonactivatingPanel,
            NSBackingStoreType::Buffered,
            false,
        );
        if window.isReleasedWhenClosed() {
            return Err(
                "macOS overlay NSPanel unexpectedly releases itself when closed; refusing ambiguous ownership"
                    .into(),
            );
        }
        let root_view = NSView::initWithFrame(NSView::alloc(mtm), view_frame);
        let root_layer = CALayer::new();
        root_layer.setFrame(view_frame);
        root_view.setWantsLayer(true);
        root_view.setLayer(Some(&root_layer));

        let clear = NSColor::clearColor();
        window.setOpaque(false);
        window.setBackgroundColor(Some(&clear));
        window.setHasShadow(false);
        window.setIgnoresMouseEvents(true);
        window.setHidesOnDeactivate(false);
        window.setLevel(NSScreenSaverWindowLevel);
        window.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::IgnoresCycle
                | NSWindowCollectionBehavior::FullScreenAuxiliary,
        );
        window.setContentView(Some(&root_view));

        let cursor = CALayer::new();
        cursor.setHidden(true);
        root_layer.addSublayer(&cursor);
        let scale = window.backingScaleFactor().max(1.0);
        self.content = Some(WindowContent {
            window,
            root_view,
            root_layer,
            shapes: Vec::new(),
            labels: Vec::new(),
            cursor,
            colors: HashMap::with_capacity(16),
            fonts: Vec::with_capacity(8),
            scale,
        });
        Ok(())
    }
}

impl Default for Overlay {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowContent {
    fn update_static(&mut self, scene: &OverlayScene, area: Rect) {
        let backdrop = scene.backdrop.map(|color| self.color(color));
        self.root_layer.setBackgroundColor(backdrop.as_deref());
        self.trim_shapes(scene.shapes.len());
        self.ensure_shapes(scene.shapes.len());
        for (index, shape) in scene.shapes.iter().enumerate() {
            self.configure_shape(index, shape, area);
        }
        for shape in self.shapes.iter().skip(scene.shapes.len()) {
            shape.setHidden(true);
        }

        self.ensure_labels(scene.labels.len());
        for (index, label) in scene.labels.iter().enumerate() {
            let spec = LabelSpec {
                text: &label.text,
                rect: effective_label_rect(label),
                style: &label.style,
                matched_prefix_len: label.matched_prefix_len,
                z_index: label.z_index,
            };
            self.configure_label(index, spec, area);
        }
    }

    fn update_dynamic(&mut self, scene: &OverlayScene, area: Rect) {
        self.configure_cursor(scene.cursor_marker.as_ref(), area);
        let static_labels = scene.labels.len();
        let dynamic_labels = scene.indicator.as_ref().map_or(0, |indicator| {
            1 + usize::from(indicator.held_text.is_some())
        });
        let required_labels = static_labels + dynamic_labels;
        self.trim_labels(required_labels);
        self.ensure_labels(required_labels);
        let mut used = static_labels;
        if let Some(indicator) = &scene.indicator {
            let (width, _, main, held) = indicator_layout(indicator);
            let translate = |rect: Rect| {
                Rect::new(
                    indicator.position.x - width + rect.x,
                    indicator.position.y + rect.y,
                    rect.width,
                    rect.height,
                )
            };
            self.configure_label(
                used,
                LabelSpec {
                    text: &indicator.text,
                    rect: translate(main),
                    style: &indicator.style,
                    matched_prefix_len: 0,
                    z_index: 20_001,
                },
                area,
            );
            used += 1;
            if let (Some(text), Some(rect)) = (&indicator.held_text, held) {
                self.configure_label(
                    used,
                    LabelSpec {
                        text,
                        rect: translate(rect),
                        style: &indicator.style,
                        matched_prefix_len: 0,
                        z_index: 20_002,
                    },
                    area,
                );
                used += 1;
            }
        }
        for label in self.labels.iter().skip(used) {
            label.base.setHidden(true);
        }
    }

    fn ensure_shapes(&mut self, count: usize) {
        while self.shapes.len() < count {
            let layer = CAShapeLayer::layer();
            layer.setContentsScale(self.scale);
            layer.setHidden(true);
            self.root_layer.addSublayer(&layer);
            self.shapes.push(layer);
        }
    }

    fn trim_shapes(&mut self, count: usize) {
        trim_pool(&mut self.shapes, count, |shape| {
            shape.removeFromSuperlayer()
        });
    }

    fn ensure_labels(&mut self, count: usize) {
        while self.labels.len() < count {
            let base = CATextLayer::layer();
            let matched = CATextLayer::layer();
            let mask = CALayer::new();
            // SAFETY: QuartzCore exports this process-lifetime immutable
            // alignment-mode object on every supported macOS version.
            let center = unsafe { kCAAlignmentCenter };
            base.setAlignmentMode(center);
            matched.setAlignmentMode(center);
            base.setWrapped(false);
            matched.setWrapped(false);
            base.setContentsScale(self.scale);
            matched.setContentsScale(self.scale);
            let opaque = self.color(Color::rgb(0, 0, 0));
            mask.setBackgroundColor(Some(&opaque));
            // SAFETY: `mask` is retained by LabelLayers and Core Animation;
            // it is a CALayer as required by the property contract.
            unsafe { matched.setMask(Some(&mask)) };
            base.addSublayer(&matched);
            base.setHidden(true);
            self.root_layer.addSublayer(&base);
            self.labels.push(LabelLayers {
                base,
                matched,
                mask,
                text: String::new(),
            });
        }
    }

    fn trim_labels(&mut self, count: usize) {
        trim_pool(&mut self.labels, count, LabelLayers::detach);
    }

    fn teardown(&mut self) {
        self.window.orderOut(None);
        self.trim_shapes(0);
        self.trim_labels(0);
        self.cursor.removeFromSuperlayer();
        self.root_layer.setBackgroundColor(None);
        self.root_view.setLayer(None);
        self.window.setContentView(None);
        self.colors.clear();
        self.fonts.clear();
        // NSPanel defaults to releasedWhenClosed=false, which is also
        // explicitly validated at construction. Retained<NSPanel> remains the
        // sole release owner after AppKit closes its WindowServer resources.
        self.window.close();
    }

    fn configure_shape(&mut self, index: usize, shape: &OverlayShape, area: Rect) {
        let layer = self.shapes[index].clone();
        layer.setContentsScale(self.scale);
        layer.setHidden(false);
        match shape {
            OverlayShape::Rect {
                rect,
                fill,
                stroke,
                stroke_width,
                corner_radius,
                z_index,
            } => {
                let fill = (!fill.is_transparent()).then(|| self.color(*fill));
                let stroke =
                    (!stroke.is_transparent() && *stroke_width > 0.0).then(|| self.color(*stroke));
                layer.setPath(None);
                layer.setFrame(to_window_rect(*rect, area));
                layer.setBackgroundColor(fill.as_deref());
                layer.setBorderColor(stroke.as_deref());
                layer.setBorderWidth(if stroke.is_some() {
                    stroke_width.max(0.0)
                } else {
                    0.0
                });
                layer.setCornerRadius(corner_radius.max(0.0));
                layer.setFillColor(None);
                layer.setStrokeColor(None);
                layer.setZPosition(f64::from(*z_index) * 2.0);
            }
            OverlayShape::Line {
                from,
                to,
                color,
                width,
                z_index,
            } => {
                if color.is_transparent() || *width <= 0.0 {
                    layer.setHidden(true);
                    return;
                }
                let stroke = self.color(*color);
                let from = to_window_point(*from, area);
                let to = to_window_point(*to, area);
                let path = CGMutablePath::new();
                // SAFETY: a null transform means identity; `path` is a live
                // mutable Core Graphics path for both calls.
                unsafe {
                    CGMutablePath::move_to_point(Some(&path), std::ptr::null(), from.x, from.y);
                    CGMutablePath::add_line_to_point(Some(&path), std::ptr::null(), to.x, to.y);
                }
                layer.setFrame(NSRect::new(
                    NSPoint::new(0.0, 0.0),
                    self.root_layer.frame().size,
                ));
                layer.setBackgroundColor(None);
                layer.setBorderColor(None);
                layer.setBorderWidth(0.0);
                layer.setCornerRadius(0.0);
                layer.setFillColor(None);
                layer.setStrokeColor(Some(&stroke));
                layer.setLineWidth(width.max(0.0));
                layer.setPath(Some(&path));
                layer.setZPosition(f64::from(*z_index) * 2.0);
            }
        }
    }

    fn configure_label(&mut self, index: usize, spec: LabelSpec<'_>, area: Rect) {
        let background =
            (!spec.style.background.is_transparent()).then(|| self.color(spec.style.background));
        let border = (!spec.style.border_color.is_transparent() && spec.style.border_width > 0.0)
            .then(|| self.color(spec.style.border_color));
        let foreground = self.color(spec.style.text_color);
        let matched_foreground = self.color(spec.style.matched_text_color);
        let font = self.font(spec.style);
        let layer = &mut self.labels[index];
        let frame = to_window_rect(spec.rect, area);
        layer.base.setHidden(false);
        layer.base.setFrame(frame);
        layer.base.setBackgroundColor(background.as_deref());
        layer.base.setBorderColor(border.as_deref());
        layer.base.setBorderWidth(if border.is_some() {
            spec.style.border_width.max(0.0)
        } else {
            0.0
        });
        layer
            .base
            .setCornerRadius(spec.style.border_radius.max(0.0));
        layer.base.setForegroundColor(Some(&foreground));
        // SAFETY: CTFont is a documented CATextLayer font type. The layer
        // retains it and the cache also owns a CFRetained reference.
        unsafe {
            layer.base.setFont(Some(&font));
            layer.matched.setFont(Some(&font));
        }
        layer.base.setFontSize(spec.style.font_size.max(1.0));
        layer.base.setContentsScale(self.scale);
        layer.base.setZPosition(f64::from(spec.z_index) * 2.0 + 1.0);

        layer
            .matched
            .setFrame(NSRect::new(NSPoint::new(0.0, 0.0), frame.size));
        layer.matched.setForegroundColor(Some(&matched_foreground));
        layer.matched.setFontSize(spec.style.font_size.max(1.0));
        layer.matched.setContentsScale(self.scale);
        layer.matched.setBackgroundColor(None);
        if layer.text != spec.text {
            let string = NSString::from_str(spec.text);
            // SAFETY: NSString is one of CATextLayer's documented string
            // object types and is copied by the property setter.
            unsafe {
                layer.base.setString(Some(&string));
                layer.matched.setString(Some(&string));
            }
            layer.text.clear();
            layer.text.push_str(spec.text);
        }

        let total = spec.text.chars().count().max(1);
        let matched = spec.matched_prefix_len.min(total);
        layer.matched.setHidden(matched == 0);
        if matched > 0 {
            layer.mask.setFrame(NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(
                    frame.size.width * matched as f64 / total as f64,
                    frame.size.height,
                ),
            ));
        }
    }

    fn configure_cursor(&mut self, marker: Option<&CursorMarker>, area: Rect) {
        let Some(marker) = marker else {
            self.cursor.setHidden(true);
            return;
        };
        let fill = (!marker.fill.is_transparent()).then(|| self.color(marker.fill));
        let stroke = (!marker.stroke.is_transparent() && marker.stroke_width > 0.0)
            .then(|| self.color(marker.stroke));
        let rect = Rect::new(
            marker.center.x - marker.radius,
            marker.center.y - marker.radius,
            marker.radius * 2.0,
            marker.radius * 2.0,
        );
        self.cursor.setHidden(false);
        self.cursor.setFrame(to_window_rect(rect, area));
        self.cursor.setBackgroundColor(fill.as_deref());
        self.cursor.setBorderColor(stroke.as_deref());
        self.cursor.setBorderWidth(if stroke.is_some() {
            marker.stroke_width.max(0.0)
        } else {
            0.0
        });
        self.cursor.setCornerRadius(marker.radius.max(0.0));
        self.cursor.setZPosition(20_000.0);
        self.cursor.setContentsScale(self.scale);
    }

    fn color(&mut self, color: Color) -> Retained<CGColor> {
        let key = u32::from_be_bytes([color.r, color.g, color.b, color.a]);
        if let Some(cached) = self.colors.get(&key) {
            return cached.clone();
        }
        if self.colors.len() >= MAX_CACHED_COLORS {
            self.colors.clear();
        }
        let color = NSColor::colorWithSRGBRed_green_blue_alpha(
            f64::from(color.r) / 255.0,
            f64::from(color.g) / 255.0,
            f64::from(color.b) / 255.0,
            f64::from(color.a) / 255.0,
        )
        .CGColor();
        self.colors.insert(key, color.clone());
        color
    }

    fn font(&mut self, style: &LabelStyle) -> CFRetained<CTFont> {
        let size_bits = style.font_size.to_bits();
        if let Some(entry) = self.fonts.iter().find(|entry| {
            entry.family == style.font_family
                && entry.size_bits == size_bits
                && entry.bold == style.bold
        }) {
            return entry.font.clone();
        }
        if self.fonts.len() >= 32 {
            self.fonts.clear();
        }
        let size = style.font_size.max(1.0);
        // SAFETY: every optional transform is null (identity), language is
        // absent, and Core Text returns an owned font. The UI-font request is
        // preferred for an empty family so bold follows the system typeface.
        let font = unsafe {
            if style.font_family.is_empty() {
                CTFont::new_ui_font_for_language(
                    if style.bold {
                        CTFontUIFontType::EmphasizedSystem
                    } else {
                        CTFontUIFontType::System
                    },
                    size,
                    None,
                )
                .unwrap_or_else(|| {
                    CTFont::with_name(&CFString::from_str("Helvetica"), size, std::ptr::null())
                })
            } else {
                let base = CTFont::with_name(
                    &CFString::from_str(&style.font_family),
                    size,
                    std::ptr::null(),
                );
                if style.bold {
                    base.copy_with_symbolic_traits(
                        size,
                        std::ptr::null(),
                        CTFontSymbolicTraits::TraitBold,
                        CTFontSymbolicTraits::TraitBold,
                    )
                    .unwrap_or(base)
                } else {
                    base
                }
            }
        };
        self.fonts.push(FontEntry {
            family: style.font_family.clone(),
            size_bits,
            bold: style.bold,
            font: font.clone(),
        });
        font
    }
}

impl LabelLayers {
    fn detach(self) {
        // Disconnect both parent relationships explicitly. The mask is owned
        // only by `matched` and this value, so it is released with the detached
        // label tree without another Objective-C mutation.
        self.matched.removeFromSuperlayer();
        self.base.removeFromSuperlayer();
    }
}

fn trim_pool<T>(items: &mut Vec<T>, count: usize, mut detach: impl FnMut(T)) {
    let keep = count.min(items.len());
    for item in items.drain(keep..) {
        detach(item);
    }
}

struct DisabledActions;

impl DisabledActions {
    fn begin() -> Self {
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        Self
    }
}

impl Drop for DisabledActions {
    fn drop(&mut self) {
        CATransaction::commit();
    }
}

fn static_scene_eq(left: &OverlayScene, right: &OverlayScene) -> bool {
    left.backdrop == right.backdrop
        && left.clip == right.clip
        && left.shapes == right.shapes
        && left.labels == right.labels
}

fn effective_label_rect(label: &OverlayLabel) -> Rect {
    if !label.fit_to_text {
        return label.rect;
    }
    let width = (label.text.chars().count().max(1) as f64 * label.style.font_size * 0.75
        + label.style.padding_x.max(0.0) * 2.0)
        .min(label.rect.width)
        .max(1.0);
    let height = (label.style.font_size * 1.4 + label.style.padding_y.max(0.0) * 2.0)
        .min(label.rect.height)
        .max(1.0);
    Rect::new(
        label.rect.center().x - width / 2.0,
        label.rect.center().y - height / 2.0,
        width,
        height,
    )
}

fn indicator_layout(indicator: &Indicator) -> (f64, f64, Rect, Option<Rect>) {
    let label_width = |text: &str| {
        (indicator.style.font_size * 0.75 * text.chars().count() as f64
            + indicator.style.padding_x.max(0.0) * 2.0)
            .max(indicator.style.font_size * 2.0)
            .ceil()
    };
    let line_height =
        (indicator.style.font_size * 1.4 + indicator.style.padding_y.max(0.0) * 2.0).ceil();
    let main_width = label_width(&indicator.text);
    let held_width = indicator
        .held_text
        .as_deref()
        .map(label_width)
        .unwrap_or_default();
    let gap = indicator.held_text.as_ref().map_or(0.0, |_| 4.0);
    let height = line_height + gap + indicator.held_text.as_ref().map_or(0.0, |_| line_height);
    let width = main_width.max(held_width);
    let main = Rect::new(width - main_width, 0.0, main_width, line_height);
    let held = indicator.held_text.as_ref().map(|_| {
        Rect::new(
            width - held_width,
            line_height + gap,
            held_width,
            line_height,
        )
    });
    (width, height, main, held)
}

fn render_area() -> Result<Rect, String> {
    let screens = super::screens::list_screens()?;
    let area = Screen::virtual_bounds(&screens);
    (!area.is_empty())
        .then_some(area)
        .ok_or_else(|| "macOS reports no usable display for overlay rendering".into())
}

/// Convert top-left virtual desktop coordinates into AppKit's bottom-left
/// global window coordinates.
fn cocoa_frame(area: Rect) -> Result<NSRect, String> {
    let screens = super::screens::list_screens()?;
    let primary = Screen::primary(&screens)
        .ok_or_else(|| "macOS reports no primary display for overlay rendering".to_string())?;
    Ok(rect(
        area.x,
        primary.bounds.bottom() - area.bottom(),
        area.width,
        area.height,
    ))
}

fn to_window_rect(value: Rect, area: Rect) -> NSRect {
    rect(
        value.x - area.x,
        area.height - (value.y - area.y) - value.height,
        value.width,
        value.height,
    )
}

fn to_window_point(value: Point, area: Rect) -> Point {
    Point::new(value.x - area.x, area.height - (value.y - area.y))
}

fn rect(x: f64, y: f64, width: f64, height: f64) -> NSRect {
    NSRect::new(
        NSPoint::new(x, y),
        NSSize::new(width.max(0.0), height.max(0.0)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scene() -> Arc<OverlayScene> {
        Arc::new(OverlayScene::new())
    }

    #[test]
    fn tracks_visibility_across_present_and_dismiss() {
        let mut overlay = Overlay::new();
        assert!(!overlay.is_visible());
        overlay.present(scene()).unwrap();
        assert!(overlay.is_visible());
        overlay.dismiss().unwrap();
        assert!(!overlay.is_visible());
        assert!(overlay.scene.is_none());
        assert!(overlay.area.is_none());
        assert!(overlay.content.is_none());
    }

    #[test]
    fn trimming_a_native_pool_detaches_only_surplus_slots() {
        let mut slots = vec![0, 1, 2, 3];
        let mut detached = Vec::new();

        trim_pool(&mut slots, 2, |slot| detached.push(slot));
        assert_eq!(slots, [0, 1]);
        assert_eq!(detached, [2, 3]);

        trim_pool(&mut slots, 8, |slot| detached.push(slot));
        assert_eq!(slots, [0, 1]);
        assert_eq!(detached, [2, 3]);

        trim_pool(&mut slots, 0, |slot| detached.push(slot));
        assert!(slots.is_empty());
        assert_eq!(detached, [2, 3, 0, 1]);
    }

    #[test]
    fn converts_top_left_scene_coordinates_to_window_coordinates() {
        let area = Rect::new(-100.0, -50.0, 2000.0, 1200.0);
        assert_eq!(
            to_window_rect(Rect::new(0.0, 0.0, 200.0, 100.0), area)
                .origin
                .y,
            1050.0
        );
        assert_eq!(to_window_point(Point::new(0.0, 0.0), area).y, 1150.0);
    }

    #[test]
    fn pointer_only_changes_do_not_rebuild_static_layers() {
        let mut overlay = Overlay::new();
        let mut first = OverlayScene::new();
        first.shapes.push(OverlayShape::fill(
            Rect::new(0.0, 0.0, 100.0, 100.0),
            Color::rgb(1, 2, 3),
        ));
        first.cursor_marker = Some(CursorMarker {
            center: Point::new(8.0, 9.0),
            radius: 13.0,
            fill: Color::rgb(4, 5, 6),
            stroke: Color::rgb(7, 8, 9),
            stroke_width: 2.0,
        });
        let mut second = first.clone();
        second.cursor_marker.as_mut().unwrap().center = Point::new(30.0, 40.0);
        overlay.present(Arc::new(first)).unwrap();
        overlay.present(Arc::new(second)).unwrap();
        assert_eq!(overlay.static_updates, 1);
    }

    #[test]
    fn held_inputs_use_a_second_badge() {
        let indicator = Indicator {
            text: "Normal".into(),
            held_text: Some("HELD: SHIFT".into()),
            position: Point::new(10.0, 20.0),
            style: Default::default(),
        };
        let (width, height, main, held) = indicator_layout(&indicator);
        let held = held.expect("held badge should have a layout");
        assert!(width >= main.width);
        assert!(height > main.height);
        assert!(held.y > main.y + main.height);
    }
}
