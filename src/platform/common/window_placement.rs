//! Pure geometry shared by native window movers.
use crate::api::command::WindowScreenTarget;
use crate::api::geometry::{Point, Rect, Screen};

/// Use the display containing the largest part of the window, matching native
/// window-monitor selection even when the pointer lies on its smaller portion.
pub(crate) fn destination(
    screens: &[Screen],
    window: Rect,
    target: WindowScreenTarget,
) -> Option<(usize, usize)> {
    let source = screens
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| {
            let overlap = |s: &Screen| {
                window
                    .intersect(&s.bounds)
                    .map_or(0.0, |r| r.width * r.height)
            };
            overlap(a).total_cmp(&overlap(b))
        })?
        .0;
    let index = match target {
        WindowScreenTarget::Previous => (source + screens.len() - 1) % screens.len(),
        WindowScreenTarget::Next => (source + 1) % screens.len(),
        WindowScreenTarget::Index(index) => index,
    };
    (index < screens.len() && index != source).then_some((source, index))
}

/// Equal display dimensions always preserve the screen-relative offset, even
/// when only one display has a taskbar. Otherwise map between usable work areas.
pub(crate) fn map_between_screens(window: Rect, source: &Screen, target: &Screen) -> Rect {
    if source.bounds.width == target.bounds.width && source.bounds.height == target.bounds.height {
        map_window(window, source.bounds, target.bounds)
    } else {
        map_window(window, source.work_area, target.work_area)
    }
}

/// Preserve the pointer's position within the moved window. Normal windows
/// translate exactly; maximized windows on different sizes preserve fractions.
/// Keep oversized/off-screen window portions from taking the pointer off-target.
pub(crate) fn following_pointer(cursor: Point, before: Rect, after: Rect, display: Rect) -> Point {
    Point::new(
        (after.x + (cursor.x - before.x) * after.width / before.width.max(1.0))
            .clamp(display.x, (display.right() - 1.0).max(display.x)),
        (after.y + (cursor.y - before.y) * after.height / before.height.max(1.0))
            .clamp(display.y, (display.bottom() - 1.0).max(display.y)),
    )
}

/// Keep window size and the fraction of the available travel on each axis.
/// Equal work areas produce an exact translation, including off-screen edges.
pub(crate) fn map_window(window: Rect, source: Rect, target: Rect) -> Rect {
    fn axis(position: f64, size: f64, from: f64, from_size: f64, to: f64, to_size: f64) -> f64 {
        if from_size == to_size {
            return to + position - from;
        }
        let fraction = if from_size > size {
            (position - from) / (from_size - size)
        } else {
            0.0
        };
        to + fraction.clamp(0.0, 1.0) * (to_size - size).max(0.0)
    }
    Rect::new(
        axis(
            window.x,
            window.width,
            source.x,
            source.width,
            target.x,
            target.width,
        ),
        axis(
            window.y,
            window.height,
            source.y,
            source.height,
            target.y,
            target.height,
        ),
        window.width,
        window.height,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pointer_follows_window_translation_at_the_same_local_offset() {
        assert_eq!(
            following_pointer(
                Point::new(250.0, 200.0),
                Rect::new(100.0, 80.0, 800.0, 600.0),
                Rect::new(-1820.0, 180.0, 800.0, 600.0),
                Rect::new(-1920.0, 100.0, 1920.0, 1080.0),
            ),
            Point::new(-1670.0, 300.0)
        );
    }

    #[test]
    fn pointer_preserves_fraction_when_a_maximized_window_changes_size() {
        assert_eq!(
            following_pointer(
                Point::new(500.0, 200.0),
                Rect::new(0.0, 0.0, 1000.0, 800.0),
                Rect::new(1000.0, 40.0, 2000.0, 1600.0),
                Rect::new(1000.0, 0.0, 2000.0, 1640.0),
            ),
            Point::new(2000.0, 440.0)
        );
    }

    #[test]
    fn oversized_windows_cannot_leave_pointer_outside_destination_display() {
        assert_eq!(
            following_pointer(
                Point::new(1900.0, 950.0),
                Rect::new(0.0, 0.0, 2000.0, 1000.0),
                Rect::new(-1280.0, 0.0, 2000.0, 1000.0),
                Rect::new(-1280.0, 0.0, 1280.0, 720.0),
            ),
            Point::new(-1.0, 719.0)
        );
    }
    #[test]
    fn equal_displays_preserve_offsets_even_with_different_taskbars() {
        let source = Screen {
            bounds: Rect::new(0.0, 0.0, 1920.0, 1080.0),
            work_area: Rect::new(0.0, 40.0, 1920.0, 1040.0),
            is_primary: true,
            scale: 1.0,
            name: None,
        };
        let target = Screen {
            bounds: Rect::new(-1920.0, 100.0, 1920.0, 1080.0),
            work_area: Rect::new(-1920.0, 100.0, 1920.0, 1080.0),
            is_primary: false,
            scale: 1.0,
            name: None,
        };
        assert_eq!(
            map_between_screens(Rect::new(200.0, 180.0, 600.0, 400.0), &source, &target),
            Rect::new(-1720.0, 280.0, 600.0, 400.0)
        );
    }
    #[test]
    fn equal_resolution_is_exact_translation_with_negative_origins() {
        assert_eq!(
            map_window(
                Rect::new(120.0, 80.0, 800.0, 600.0),
                Rect::new(0.0, 0.0, 1920.0, 1040.0),
                Rect::new(-1920.0, -100.0, 1920.0, 1040.0)
            ),
            Rect::new(-1800.0, -20.0, 800.0, 600.0)
        );
    }
    #[test]
    fn different_sizes_preserve_center_and_fit_oversized_windows() {
        assert_eq!(
            map_window(
                Rect::new(500.0, 300.0, 1000.0, 400.0),
                Rect::new(0.0, 0.0, 2000.0, 1000.0),
                Rect::new(2000.0, 40.0, 1600.0, 800.0)
            ),
            Rect::new(2300.0, 240.0, 1000.0, 400.0)
        );
        assert_eq!(
            map_window(
                Rect::new(10.0, 20.0, 2000.0, 1200.0),
                Rect::new(0.0, 0.0, 1920.0, 1080.0),
                Rect::new(-1280.0, 0.0, 1280.0, 720.0)
            )
            .x,
            -1280.0
        );
    }
    #[test]
    fn destinations_wrap_and_ignore_missing_or_same_display() {
        let screens: Vec<_> = [0.0, 1000.0, 2000.0]
            .into_iter()
            .map(|x| Screen {
                bounds: Rect::new(x, 0.0, 1000.0, 800.0),
                work_area: Rect::new(x, 0.0, 1000.0, 760.0),
                is_primary: x == 0.0,
                scale: 1.0,
                name: None,
            })
            .collect();
        let window = Rect::new(10.0, 10.0, 300.0, 200.0);
        assert_eq!(
            destination(&screens, window, WindowScreenTarget::Previous),
            Some((0, 2))
        );
        assert_eq!(
            destination(&screens, window, WindowScreenTarget::Next),
            Some((0, 1))
        );
        assert_eq!(
            destination(&screens, window, WindowScreenTarget::Index(9)),
            None
        );
        assert_eq!(
            destination(&screens[..1], window, WindowScreenTarget::Next),
            None
        );
        assert_eq!(destination(&[], window, WindowScreenTarget::Next), None);
    }
}
