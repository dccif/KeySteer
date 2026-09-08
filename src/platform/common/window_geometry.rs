//! Pure placement geometry, in the backend's screen coordinate system.
use crate::api::{Point, Rect, Screen};

pub(crate) fn screen_index(screens: &[Screen], rect: Rect) -> Option<usize> {
    screens
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| {
            let area = |screen: &Screen| {
                rect.intersect(&screen.bounds)
                    .map_or(0.0, |r| r.width * r.height)
            };
            area(a).total_cmp(&area(b))
        })
        .map(|(index, _)| index)
}

pub(crate) fn constrain_move(mut rect: Rect, area: Rect) -> Rect {
    rect.x = rect
        .x
        .clamp(area.x, (area.right() - rect.width).max(area.x));
    rect.y = rect
        .y
        .clamp(area.y, (area.bottom() - rect.height).max(area.y));
    rect
}

pub(crate) fn resize_center(rect: Rect, dw: f64, dh: f64, area: Rect, minimum: Point) -> Rect {
    let center = rect.center();
    let axis = |size: f64, delta: f64, center: f64, lo: f64, hi: f64, minimum: f64| {
        let maximum = (2.0 * (center - lo).min(hi - center)).max(0.0);
        if delta == 0.0 || maximum < minimum {
            return size;
        }
        (size + delta).clamp(minimum, maximum)
    };
    let width = axis(rect.width, dw, center.x, area.x, area.right(), minimum.x);
    let height = axis(rect.height, dh, center.y, area.y, area.bottom(), minimum.y);
    Rect::new(
        center.x - width / 2.0,
        center.y - height / 2.0,
        width,
        height,
    )
}

pub(crate) fn placement(area: Rect, index: usize, gap: f64) -> Option<Rect> {
    let (_, relative) = crate::api::window::LAYOUTS.get(index)?;
    let gap = gap.max(0.0).min(area.width.min(area.height) / 8.0);
    let rect = Rect::new(
        area.x + area.width * relative.x,
        area.y + area.height * relative.y,
        area.width * relative.width,
        area.height * relative.height,
    );
    Some(rect.inset(gap / 2.0, gap / 2.0))
}

/// Each row has a height proportional to its population, giving equally sized
/// areas even for a non-rectangular count. Pick the least distorted 16:10 cells.
pub(crate) fn tile(area: Rect, count: usize, gap: f64) -> Vec<Rect> {
    if count == 0 || area.width <= 0.0 || area.height <= 0.0 {
        return Vec::new();
    }
    let count = count.min(256);
    let best_rows = (1..=count)
        .min_by(|&a, &b| {
            let score = |rows: usize| {
                (0..rows)
                    .map(|row| {
                        let columns = count / rows + usize::from(row < count % rows);
                        let width = area.width / columns as f64;
                        let height = area.height * columns as f64 / count as f64;
                        (width / height / 1.6).ln().abs() * columns as f64
                    })
                    .sum::<f64>()
            };
            score(a).total_cmp(&score(b))
        })
        .unwrap_or(1);
    let mut result = Vec::with_capacity(count);
    let mut y = area.y;
    for row in 0..best_rows {
        let columns = count / best_rows + usize::from(row < count % best_rows);
        let height = area.height * columns as f64 / count as f64;
        let width = area.width / columns as f64;
        let inset = (gap / 2.0).max(0.0).min(width.min(height) / 4.0);
        for col in 0..columns {
            result
                .push(Rect::new(area.x + col as f64 * width, y, width, height).inset(inset, inset));
        }
        y += height;
    }
    result
}

/// Fit all windows before falling back to overlapping minimum-sized cells.
/// Wide apps can use a wider column instead of being silently excluded.
pub(crate) fn tile_with_minimums(area: Rect, minimums: &[Point], gap: f64) -> Vec<Rect> {
    let count = minimums.len().min(256);
    if count == 0 {
        return Vec::new();
    }
    let preferred = tile(area, count, gap);
    if preferred
        .iter()
        .zip(minimums)
        .all(|(cell, min)| cell.width >= min.x && cell.height >= min.y)
    {
        return preferred;
    }
    let gap = gap.max(0.0).min(area.width.min(area.height) / 8.0);
    fn distribute(minimums: &[f64], total: f64) -> Option<Vec<f64>> {
        if minimums.iter().sum::<f64>() > total {
            return None;
        }
        let mut low = 0.0;
        let mut high = total;
        for _ in 0..40 {
            let level = (low + high) / 2.0;
            if minimums.iter().map(|v| v.max(level)).sum::<f64>() > total {
                high = level;
            } else {
                low = level;
            }
        }
        Some(minimums.iter().map(|v| v.max(low)).collect())
    }
    let mut best: Option<(f64, Vec<Rect>)> = None;
    for rows in 1..=count {
        let mut widths = Vec::new();
        let mut heights = Vec::new();
        let mut offset = 0;
        for row in 0..rows {
            let columns = count / rows + usize::from(row < count % rows);
            let cells = &minimums[offset..offset + columns];
            let mins: Vec<_> = cells.iter().map(|p| p.x.max(1.0) + gap).collect();
            let Some(row_widths) = distribute(&mins, area.width) else {
                break;
            };
            widths.push(row_widths);
            heights.push(cells.iter().map(|p| p.y.max(1.0) + gap).fold(0.0, f64::max));
            offset += columns;
        }
        if widths.len() != rows {
            continue;
        }
        let Some(heights) = distribute(&heights, area.height) else {
            continue;
        };
        let mut cells = Vec::with_capacity(count);
        let mut y = area.y;
        let mut score = 0.0;
        for (widths, height) in widths.into_iter().zip(heights) {
            let mut x = area.x;
            for width in widths {
                let cell = Rect::new(x, y, width, height).inset(gap / 2.0, gap / 2.0);
                score += (cell.width / cell.height / 1.6).ln().abs();
                cells.push(cell);
                x += width;
            }
            y += height;
        }
        if best.as_ref().is_none_or(|(previous, _)| score < *previous) {
            best = Some((score, cells));
        }
    }
    best.map_or_else(
        || {
            tile(area, count, gap)
                .into_iter()
                .zip(minimums)
                .map(|(cell, min)| {
                    constrain_move(
                        Rect::new(
                            cell.x,
                            cell.y,
                            cell.width.max(min.x),
                            cell.height.max(min.y),
                        ),
                        area,
                    )
                })
                .collect()
        },
        |(_, cells)| cells,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wide_app_participates_in_tiling_without_overlap_when_it_fits() {
        let area = Rect::new(0.0, 0.0, 1800.0, 1100.0);
        let mins = [Point::new(1000.0, 600.0), Point::new(400.0, 300.0)];
        let cells = tile_with_minimums(area, &mins, 8.0);
        assert_eq!(cells.len(), 2);
        for (cell, min) in cells.iter().zip(mins) {
            assert!(cell.width >= min.x && cell.height >= min.y);
            assert!(cell.right() <= area.right() && cell.bottom() <= area.bottom());
        }
        assert!(cells[0].intersect(&cells[1]).is_none());
    }

    #[test]
    fn impossible_minimums_still_produce_a_position_for_every_window() {
        let cells = tile_with_minimums(
            Rect::new(0.0, 0.0, 1000.0, 700.0),
            &[Point::new(800.0, 600.0); 3],
            8.0,
        );
        assert_eq!(cells.len(), 3);
        assert!(cells.iter().all(|c| c.width >= 800.0 && c.height >= 600.0));
    }
    #[test]
    fn centered_resize_keeps_center_and_respects_boundaries() {
        let area = Rect::new(-1920.0, -200.0, 1920.0, 1080.0);
        let rect = Rect::new(-1500.0, 100.0, 800.0, 400.0);
        let resized = resize_center(rect, 5000.0, -5000.0, area, Point::new(200.0, 100.0));
        assert_eq!(resized.center(), rect.center());
        assert_eq!(resized.height, 100.0);
        assert!(resized.x >= area.x && resized.right() <= area.right());
    }
    #[test]
    fn all_counts_tile_without_overlap_or_missing_cells() {
        for area in [
            Rect::new(-1920.0, 30.0, 1920.0, 1050.0),
            Rect::new(0.0, 0.0, 900.0, 1600.0),
        ] {
            for count in 1..=40 {
                let cells = tile(area, count, 8.0);
                assert_eq!(cells.len(), count);
                for (i, cell) in cells.iter().enumerate() {
                    assert!(cell.x >= area.x && cell.y >= area.y);
                    assert!(
                        cell.right() <= area.right() + 0.001
                            && cell.bottom() <= area.bottom() + 0.001
                    );
                    assert!(
                        cells[i + 1..]
                            .iter()
                            .all(|other| cell.intersect(other).is_none())
                    );
                }
            }
        }
    }
}
