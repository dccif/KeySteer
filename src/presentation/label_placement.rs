//! Collision placement for explicitly grouped annotations, independent of paint layers.
use crate::api::overlay::{LabelPlacementRole as Role, OverlayLabel};
use crate::api::{OverlayScene, OverlayShape, Point, Rect, Screen};
use std::collections::BTreeMap;

// Test physical footprints before accepting a candidate. Clamping afterwards
// would collapse displaced cards back onto the same bottom-edge position.
pub(super) fn card_position(
    center: Point,
    width: f64,
    height: f64,
    area: Rect,
    occupied: &[Rect],
) -> Rect {
    let candidate = |x: f64, y: f64| {
        Rect::new(
            x.clamp(area.x, (area.right() - width).max(area.x)),
            y.clamp(area.y, (area.bottom() - height).max(area.y)),
            width,
            height,
        )
    };
    let initial = candidate(center.x - width / 2.0, center.y - height / 2.0);
    let free = |rect: &Rect| !occupied.iter().any(|old| old.intersect(rect).is_some());
    if free(&initial) {
        return initial;
    }
    let columns = (area.width / (width + 6.0)).floor().max(1.0) as usize;
    let rows = (area.height / (height + 6.0)).floor().max(1.0) as usize;
    let mut best = None;
    let mut distance = f64::INFINITY;
    for obstacle in occupied {
        for (x, y) in [
            (initial.x, obstacle.y - height - 6.0),
            (initial.x, obstacle.bottom() + 6.0),
            (obstacle.x - width - 6.0, initial.y),
            (obstacle.right() + 6.0, initial.y),
        ] {
            let rect = candidate(x, y);
            let d = (rect.center().x - center.x).powi(2) + (rect.center().y - center.y).powi(2);
            if free(&rect) && d < distance {
                best = Some(rect);
                distance = d;
            }
        }
    }
    for row in 0..rows {
        for col in 0..columns {
            let rect = candidate(
                area.x + col as f64 * (width + 6.0),
                area.y + row as f64 * (height + 6.0),
            );
            let d = (rect.center().x - center.x).powi(2) + (rect.center().y - center.y).powi(2);
            if free(&rect) && d < distance {
                best = Some(rect);
                distance = d;
            }
        }
    }
    best.unwrap_or(initial)
}
pub(super) fn card_positions(
    centers: &[Point],
    desired_width: f64,
    height: f64,
    area: Rect,
) -> (f64, Vec<Rect>) {
    let rows = (area.height / (height + 6.0)).floor().max(1.0) as usize;
    let columns = centers.len().div_ceil(rows).max(1);
    let width =
        desired_width.min(((area.width - (columns - 1) as f64 * 6.0) / columns as f64).max(1.0));
    let mut positions = Vec::with_capacity(centers.len());
    for center in centers {
        let rect = card_position(*center, width, height, area, &positions);
        if positions
            .iter()
            .any(|old: &Rect| old.intersect(&rect).is_some())
        {
            // Arbitrary center anchors can fragment otherwise sufficient space.
            // Repack the whole set deterministically instead of overlapping.
            positions.clear();
            for index in 0..centers.len() {
                positions.push(Rect::new(
                    area.x + (index / rows) as f64 * (width + 6.0),
                    area.y + (index % rows) as f64 * (height + 6.0),
                    width,
                    height,
                ));
            }
            return (width, positions);
        }
        positions.push(rect);
    }
    (width, positions)
}

pub(super) fn logical(rect: Rect, scale: f64) -> Rect {
    Rect::new(
        rect.center().x - rect.width / scale / 2.0,
        rect.center().y - rect.height / scale / 2.0,
        rect.width / scale,
        rect.height / scale,
    )
}

struct Annotation {
    group: u32,
    primary: usize,
    parts: Vec<usize>,
    bounds: Rect,
    content_start: f64,
    minimum_width: f64,
}

fn physical(label: &OverlayLabel, scale: f64) -> Rect {
    if label.placement.is_some_and(|p| p.role == Role::Background) {
        return label.rect;
    }
    // Fixed/flexible card parts already use a logical rectangle around the
    // physical center. Standalone tags follow the renderer's compact geometry.
    if label.placement.is_some_and(|p| p.role == Role::Standalone) {
        #[cfg(target_os = "windows")]
        return crate::api::overlay::scaled_label_geometry(
            &label.text,
            label.rect,
            &label.style,
            scale,
        )
        .0;
        #[cfg(not(target_os = "windows"))]
        return label.rect;
    }
    Rect::new(
        label.rect.center().x - label.rect.width * scale / 2.0,
        label.rect.center().y - label.rect.height * scale / 2.0,
        label.rect.width * scale,
        label.rect.height * scale,
    )
}

fn annotations(scene: &OverlayScene, screen: &Screen, scale: f64) -> Vec<Annotation> {
    let mut members = BTreeMap::<u32, Vec<usize>>::new();
    for (index, label) in scene.labels.iter().enumerate() {
        if let Some(placement) = label.placement {
            members.entry(placement.group).or_default().push(index);
        }
    }
    members
        .into_iter()
        .filter_map(|(group, parts)| {
            let primary = *parts.iter().find(|index| {
                scene.labels[**index]
                    .placement
                    .is_some_and(|p| matches!(p.role, Role::Background | Role::Standalone))
            })?;
            let bounds = physical(&scene.labels[primary], scale);
            if !screen.bounds.contains(&bounds.center()) {
                return None;
            }
            let content_start = parts
                .iter()
                .filter_map(|index| {
                    let label = &scene.labels[*index];
                    (label.placement?.role == Role::Fixed)
                        .then(|| physical(label, scale).right() - bounds.x + 9.0 * scale)
                })
                .fold(0.0, f64::max);
            let flexible_font = parts
                .iter()
                .filter_map(|index| {
                    let label = &scene.labels[*index];
                    (label.placement?.role == Role::Flexible)
                        .then_some(label.style.font_size * scale)
                })
                .reduce(f64::max);
            let minimum_width = flexible_font.map_or(bounds.width, |font| {
                (content_start + font + 18.0 * scale).min(bounds.width)
            });
            Some(Annotation {
                group,
                primary,
                parts,
                bounds,
                content_start,
                minimum_width,
            })
        })
        .collect()
}

/// Disjoint free rectangles, used only when normal nearest-anchor placement
/// cannot fit the complete set. Their union is the available work area.
fn free_regions(area: Rect, exclusions: &[Rect]) -> Vec<Rect> {
    let mut regions = vec![area];
    for obstacle in exclusions {
        regions = regions
            .into_iter()
            .flat_map(|region| {
                let Some(cut) = region.intersect(obstacle) else {
                    return vec![region];
                };
                [
                    Rect::new(region.x, region.y, region.width, cut.y - region.y),
                    Rect::new(
                        region.x,
                        cut.bottom(),
                        region.width,
                        region.bottom() - cut.bottom(),
                    ),
                    Rect::new(region.x, cut.y, cut.x - region.x, cut.height),
                    Rect::new(cut.right(), cut.y, region.right() - cut.right(), cut.height),
                ]
                .into_iter()
                .filter(|r| r.width > 0.0 && r.height > 0.0)
                .collect()
            })
            .collect();
    }
    regions.sort_by(|a, b| (b.width * b.height).total_cmp(&(a.width * a.height)));
    regions
}

fn packed(
    annotations: &[Annotation],
    regions: &[Rect],
    gap: f64,
    fraction: f64,
) -> Option<Vec<Rect>> {
    let mut order: Vec<_> = (0..annotations.len()).collect();
    order.sort_by(|a, b| {
        annotations[*b]
            .bounds
            .height
            .total_cmp(&annotations[*a].bounds.height)
    });
    let mut positions = vec![Rect::default(); annotations.len()];
    let mut shelves: Vec<_> = regions
        .iter()
        .map(|area| (area.x, area.y, 0.0_f64))
        .collect();
    for index in order {
        let annotation = &annotations[index];
        let width = annotation.minimum_width
            + (annotation.bounds.width - annotation.minimum_width) * fraction;
        let height = annotation.bounds.height;
        let mut found = false;
        for (region, shelf) in regions.iter().zip(&mut shelves) {
            if width > region.width || height > region.height {
                continue;
            }
            let (mut x, mut y, mut row_height) = *shelf;
            if x + width > region.right() + 1e-6 {
                x = region.x;
                y += row_height + gap;
                row_height = 0.0;
            }
            if y + height > region.bottom() + 1e-6 {
                continue;
            }
            positions[index] = Rect::new(x, y, width, height);
            *shelf = (x + width + gap, y, row_height.max(height));
            found = true;
            break;
        }
        if !found {
            return None;
        }
    }
    Some(positions)
}

/// Move each registered annotation as a unit, avoiding other annotations and
/// fixed panels. Membership and connectors survive z sorting and new label types.
pub(crate) fn avoid_overlaps(scene: &mut OverlayScene, screen: &Screen, panels: &[Rect]) {
    let scale = super::label_scale(screen.scale);
    let annotations = annotations(scene, screen, scale);
    if annotations.is_empty() {
        return;
    }
    let area = screen.work_area.inset(6.0 * scale, 6.0 * scale);
    let exclusions: Vec<_> = panels
        .iter()
        .map(|p| p.inset(-6.0 * scale, -6.0 * scale))
        .collect();
    let mut occupied = exclusions.clone();
    let mut placements = Vec::with_capacity(annotations.len());
    let mut fits = true;
    for annotation in &annotations {
        let old = annotation.bounds;
        let mut placed = card_position(old.center(), old.width, old.height, area, &occupied);
        if scene.labels[annotation.primary]
            .placement
            .is_some_and(|p| p.role == Role::Standalone)
        {
            placed.x = placed.x.round();
            placed.y = placed.y.round();
        }
        fits &= placed.right() <= area.right() + 1e-6
            && placed.bottom() <= area.bottom() + 1e-6
            && !occupied.iter().any(|r| r.intersect(&placed).is_some());
        occupied.push(placed.inset(-3.0 * scale, -3.0 * scale));
        placements.push(placed);
    }
    if !fits {
        let regions = free_regions(area, &exclusions);
        // Keep the largest feasible title width. Numbers and standalone tags
        // never shrink; only explicitly flexible text columns can be elided.
        for fraction in [1.0, 0.85, 0.7, 0.55, 0.4, 0.25, 0.1, 0.0] {
            if let Some(packed) = packed(&annotations, &regions, 6.0 * scale, fraction) {
                placements = packed;
                break;
            }
        }
    }
    for (annotation, mut placed) in annotations.into_iter().zip(placements) {
        if scene.labels[annotation.primary]
            .placement
            .is_some_and(|p| p.role == Role::Standalone)
        {
            placed.x = placed.x.round();
            placed.y = placed.y.round();
        }
        let old = annotation.bounds;
        let dx = placed.x - old.x;
        let dy = placed.y - old.y;
        let ratio = ((placed.width - annotation.content_start - 9.0 * scale)
            / (old.width - annotation.content_start - 9.0 * scale).max(1.0))
        .clamp(0.01, 1.0);
        for index in annotation.parts {
            let label = &mut scene.labels[index];
            match label.placement.map(|p| p.role) {
                Some(Role::Background) => label.rect = placed,
                Some(Role::Flexible) if ratio < 1.0 => {
                    let previous = physical(label, scale);
                    let rect = Rect::new(
                        placed.x
                            + annotation.content_start
                            + (previous.x - old.x - annotation.content_start) * ratio,
                        previous.y + dy,
                        previous.width * ratio,
                        previous.height,
                    );
                    label.rect = logical(rect, scale);
                    label.text =
                        super::elide_width(&label.text, rect.width / scale / label.style.font_size)
                            .into();
                }
                _ => {
                    label.rect.x += dx;
                    label.rect.y += dy;
                }
            }
        }
        if dx.abs() + dy.abs() > 1.0 || (old.width - placed.width).abs() > 1.0 {
            let mut connected = false;
            for shape in &mut scene.shapes {
                if let OverlayShape::Line {
                    to,
                    placement_group: Some(group),
                    ..
                } = shape
                    && *group == annotation.group
                {
                    *to = placed.center();
                    connected = true;
                }
            }
            if !connected {
                scene.push_shape(OverlayShape::label_connector(
                    old.center(),
                    placed.center(),
                    scene.labels[annotation.primary].style.border_color,
                    2.0 * scale,
                    annotation.group,
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::overlay::LabelStyle;

    #[test]
    fn all_annotation_kinds_avoid_each_other_and_panels_without_changing_keys_or_fonts() {
        for dpi in [1.0, 1.5, 2.0] {
            let scale = crate::presentation::label_scale(dpi);
            let screen = Screen {
                bounds: Rect::new(-1920.0, 0.0, 1920.0, 1080.0),
                work_area: Rect::new(-1920.0, 30.0, 1920.0, 1010.0),
                scale: dpi,
                is_primary: true,
                name: None,
            };
            let mut scene = OverlayScene::new();
            let card = Rect::new(-1350.0, 720.0, 310.0 * scale, 70.0 * scale);
            let style = LabelStyle {
                font_size: 28.0,
                ..LabelStyle::default()
            };
            scene.push_label(
                OverlayLabel::new(
                    "",
                    card,
                    LabelStyle {
                        font_size: 1.0,
                        ..style.clone()
                    },
                )
                .with_placement(1, Role::Background)
                .with_z_index(400),
            );
            scene.push_label(
                OverlayLabel::new(
                    "1",
                    logical(Rect::new(card.x, card.y, 60.0 * scale, card.height), scale),
                    style.clone(),
                )
                .with_placement(1, Role::Fixed)
                .with_z_index(-20),
            );
            scene.push_label(
                OverlayLabel::new(
                    "Editor",
                    logical(
                        Rect::new(
                            card.x + 72.0 * scale,
                            card.y + 4.0 * scale,
                            210.0 * scale,
                            40.0 * scale,
                        ),
                        scale,
                    ),
                    style.clone(),
                )
                .with_placement(1, Role::Flexible)
                .with_z_index(9),
            );
            for (group, text) in [(2, "`1"), (3, "`2"), (4, "~1"), (5, "~2")] {
                scene.push_label(
                    OverlayLabel::new(
                        text,
                        logical(Rect::new(card.x, card.y, 82.0 * scale, 55.0 * scale), scale),
                        style.clone(),
                    )
                    .with_placement(group, Role::Standalone)
                    .with_z_index(group as i32),
                );
            }
            // Unregistered labels are unaffected, even if they use old Window paint layers.
            scene.push_label(
                OverlayLabel::new(
                    "unrelated",
                    Rect::new(-1800.0, 50.0, 80.0, 30.0),
                    style.clone(),
                )
                .with_z_index(19),
            );
            let panels = [
                Rect::new(-1560.0, 650.0, 1200.0, 390.0),
                Rect::new(-1920.0, 30.0, 160.0, 200.0),
            ];
            scene.sort_in_place();
            avoid_overlaps(&mut scene, &screen, &panels);
            let groups = annotations(&scene, &screen, scale);
            assert_eq!(groups.len(), 5);
            for (index, group) in groups.iter().enumerate() {
                assert!(
                    panels
                        .iter()
                        .all(|panel| group.bounds.intersect(panel).is_none())
                );
                assert!(
                    groups[..index]
                        .iter()
                        .all(|other| group.bounds.intersect(&other.bounds).is_none())
                );
                assert!(
                    group.bounds.x >= screen.work_area.x
                        && group.bounds.right() <= screen.work_area.right()
                );
                for part in &group.parts {
                    assert!(group.bounds.contains(&scene.labels[*part].rect.center()));
                }
            }
            for key in ["1", "`1", "`2", "~1", "~2"] {
                let label = scene.labels.iter().find(|label| label.text == key).unwrap();
                assert_eq!(label.style.font_size, 28.0);
            }
            assert_eq!(
                scene
                    .labels
                    .iter()
                    .find(|l| l.text == "unrelated")
                    .unwrap()
                    .rect,
                Rect::new(-1800.0, 50.0, 80.0, 30.0)
            );
            for group in &groups {
                let ends: Vec<_> = scene
                    .shapes
                    .iter()
                    .filter_map(|shape| match shape {
                        OverlayShape::Line {
                            to,
                            placement_group: Some(id),
                            ..
                        } if *id == group.group => Some(*to),
                        _ => None,
                    })
                    .collect();
                assert_eq!(ends, [group.bounds.center()]);
            }
            scene.sort_in_place();
            let before = scene.clone();
            avoid_overlaps(&mut scene, &screen, &panels);
            assert_eq!(
                scene, before,
                "placement must remain stable for unchanged inputs"
            );
        }
    }
}
