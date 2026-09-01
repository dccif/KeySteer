//! Pure Rust fallback region detector.

use super::*;

pub(super) fn detect_regions(
    image: &FallbackInput,
    options: &crate::api::VisionOptions,
    scratch: &mut FallbackScratch,
    cancelled: impl Fn() -> bool,
) -> Vec<UiTarget> {
    let width = image.width;
    let height = image.height;
    if width < 3 || height < 3 || cancelled() {
        return Vec::new();
    }
    let FallbackScratch {
        edge,
        dilated,
        previous_runs,
        current_runs,
        components,
        next_components,
        root_remap,
        active_roots,
    } = scratch;
    let gray = &image.gray;
    edge.resize(width * height, false);
    edge.fill(false);
    for y in 1..height - 1 {
        if cancelled() {
            return Vec::new();
        }
        for x in 1..width - 1 {
            let i = y * width + x;
            let gx = i16::from(gray[i + 1]) - i16::from(gray[i - 1]);
            let gy = i16::from(gray[i + width]) - i16::from(gray[i - width]);
            let local_min = gray[i - width - 1..=i - width + 1]
                .iter()
                .chain(&gray[i - 1..=i + 1])
                .chain(&gray[i + width - 1..=i + width + 1])
                .copied()
                .min()
                .unwrap_or(gray[i]);
            let local_max = gray[i - width - 1..=i - width + 1]
                .iter()
                .chain(&gray[i - 1..=i + 1])
                .chain(&gray[i + width - 1..=i + width + 1])
                .copied()
                .max()
                .unwrap_or(gray[i]);
            edge[i] = gx.unsigned_abs() + gy.unsigned_abs() >= 48
                || local_max.saturating_sub(local_min) >= 42;
        }
    }
    // A small close operation joins anti-aliased borders while preserving
    // neighbouring controls as distinct components.
    dilated.clone_from(edge);
    for y in 1..height - 1 {
        if cancelled() {
            return Vec::new();
        }
        for x in 1..width - 1 {
            let i = y * width + x;
            dilated[i] = (-1isize..=1).any(|dy| {
                (-1isize..=1).any(|dx| {
                    edge[((y as isize + dy) as usize) * width + (x as isize + dx) as usize]
                })
            });
        }
    }
    // `edge` is dead after dilation; reuse it for the closed image.
    edge.fill(false);
    for y in 1..height - 1 {
        if cancelled() {
            return Vec::new();
        }
        for x in 1..width - 1 {
            let i = y * width + x;
            edge[i] = (-1isize..=1).all(|dy| {
                (-1isize..=1).all(|dx| {
                    dilated[((y as isize + dy) as usize) * width + (x as isize + dx) as usize]
                })
            });
        }
    }
    let candidate_limit = options.rectangle_max_candidates.min(2_000);
    let mut candidates = BinaryHeap::with_capacity(candidate_limit);
    let configured_minimum =
        (options.rectangle_min_size * width.min(height) as f64).ceil() as usize;
    let minimum_side = configured_minimum.max(6);
    previous_runs.clear();
    components.clear();
    for y in 1..height - 1 {
        if cancelled() {
            return Vec::new();
        }
        current_runs.clear();
        let mut x = 1usize;
        while x < width - 1 {
            if !edge[y * width + x] {
                x += 1;
                continue;
            }
            let start = x;
            while x + 1 < width - 1 && edge[y * width + x + 1] {
                x += 1;
            }
            current_runs.push(ComponentRun {
                start,
                end: x,
                label: usize::MAX,
            });
            x += 1;
        }

        let mut previous_start = 0usize;
        for run in current_runs.iter_mut() {
            while previous_start < previous_runs.len()
                && previous_runs[previous_start].end.saturating_add(1) < run.start
            {
                previous_start += 1;
            }
            let mut previous = previous_start;
            let mut label = None;
            while previous < previous_runs.len()
                && previous_runs[previous].start <= run.end.saturating_add(1)
            {
                let previous_label = previous_runs[previous].label;
                label = Some(match label {
                    Some(current) => union_components(components, current, previous_label),
                    None => component_root(components, previous_label),
                });
                previous += 1;
            }
            let label = label.unwrap_or_else(|| {
                let label = components.len();
                components.push(ActiveComponent::new(label, run.start, run.end, y));
                label
            });
            let root = component_root(components, label);
            components[root].stats.add_run(run.start, run.end, y);
            run.label = root;
        }

        active_roots.clear();
        active_roots.resize(components.len(), false);
        for run in current_runs.iter_mut() {
            let root = component_root(components, run.label);
            run.label = root;
            active_roots[root] = true;
        }
        for index in 0..components.len() {
            let root = component_root(components, index);
            if root == index && !active_roots[root] {
                consider_region(
                    components[root].stats,
                    image,
                    options,
                    minimum_side,
                    candidate_limit,
                    &mut candidates,
                );
            }
        }

        root_remap.clear();
        root_remap.resize(components.len(), usize::MAX);
        next_components.clear();
        next_components.reserve(current_runs.len());
        for run in current_runs.iter_mut() {
            let root = run.label;
            let mapped = if root_remap[root] == usize::MAX {
                let mapped = next_components.len();
                root_remap[root] = mapped;
                next_components.push(ActiveComponent {
                    parent: mapped,
                    stats: components[root].stats,
                });
                mapped
            } else {
                root_remap[root]
            };
            run.label = mapped;
        }
        std::mem::swap(components, next_components);
        std::mem::swap(previous_runs, current_runs);
    }
    for component in components.iter() {
        consider_region(
            component.stats,
            image,
            options,
            minimum_side,
            candidate_limit,
            &mut candidates,
        );
    }
    if cancelled() {
        return Vec::new();
    }
    candidates
        .into_sorted_vec()
        .into_iter()
        .map(|Reverse(candidate)| UiTarget {
            rect: candidate.rect,
            name: String::new(),
            role: candidate.role.into(),
            native_role: Some(candidate.native_role.into()),
        })
        .collect()
}

#[derive(Default)]
pub(super) struct FallbackScratch {
    edge: Vec<bool>,
    dilated: Vec<bool>,
    previous_runs: Vec<ComponentRun>,
    current_runs: Vec<ComponentRun>,
    components: Vec<ActiveComponent>,
    next_components: Vec<ActiveComponent>,
    root_remap: Vec<usize>,
    active_roots: Vec<bool>,
}

#[derive(Clone, Copy)]
struct ComponentRun {
    start: usize,
    end: usize,
    label: usize,
}

#[derive(Clone, Copy)]
struct ActiveComponent {
    parent: usize,
    stats: ComponentStats,
}

impl ActiveComponent {
    fn new(parent: usize, start: usize, end: usize, y: usize) -> Self {
        Self {
            parent,
            // The run is added after any unions so merged and new components
            // share the same update path.
            stats: ComponentStats {
                min_x: start,
                max_x: end,
                min_y: y,
                max_y: y,
                pixels: 0,
            },
        }
    }
}

#[derive(Clone, Copy)]
struct ComponentStats {
    min_x: usize,
    max_x: usize,
    min_y: usize,
    max_y: usize,
    pixels: usize,
}

impl ComponentStats {
    fn add_run(&mut self, start: usize, end: usize, y: usize) {
        self.min_x = self.min_x.min(start);
        self.max_x = self.max_x.max(end);
        self.min_y = self.min_y.min(y);
        self.max_y = self.max_y.max(y);
        self.pixels += end - start + 1;
    }

    fn merge(&mut self, other: Self) {
        self.min_x = self.min_x.min(other.min_x);
        self.max_x = self.max_x.max(other.max_x);
        self.min_y = self.min_y.min(other.min_y);
        self.max_y = self.max_y.max(other.max_y);
        self.pixels += other.pixels;
    }
}

fn component_root(components: &mut [ActiveComponent], mut index: usize) -> usize {
    let mut root = index;
    while components[root].parent != root {
        root = components[root].parent;
    }
    while components[index].parent != index {
        let parent = components[index].parent;
        components[index].parent = root;
        index = parent;
    }
    root
}

fn union_components(components: &mut [ActiveComponent], first: usize, second: usize) -> usize {
    let first = component_root(components, first);
    let second = component_root(components, second);
    if first == second {
        return first;
    }
    let (root, merged) = if first < second {
        (first, second)
    } else {
        (second, first)
    };
    let merged_stats = components[merged].stats;
    components[merged].parent = root;
    components[root].stats.merge(merged_stats);
    root
}

fn consider_region(
    stats: ComponentStats,
    image: &FallbackInput,
    options: &crate::api::VisionOptions,
    minimum_side: usize,
    candidate_limit: usize,
    candidates: &mut BinaryHeap<Reverse<RegionCandidate>>,
) {
    let box_width = stats.max_x - stats.min_x + 1;
    let box_height = stats.max_y - stats.min_y + 1;
    let aspect = box_width as f64 / box_height as f64;
    let perimeter = (2 * (box_width + box_height)).max(1);
    if box_width < minimum_side
        || box_height < minimum_side
        || stats.pixels < 16
        || !(options.rectangle_min_aspect..=options.rectangle_max_aspect).contains(&aspect)
    {
        return;
    }
    let confidence = (stats.pixels as f64 / perimeter as f64).min(1.0);
    if confidence < options.minimum_confidence {
        return;
    }
    let rect = Rect::new(
        image.desktop_bounds.x
            + stats.min_x as f64 * image.desktop_bounds.width / image.width as f64,
        image.desktop_bounds.y
            + stats.min_y as f64 * image.desktop_bounds.height / image.height as f64,
        box_width as f64 * image.desktop_bounds.width / image.width as f64,
        box_height as f64 * image.desktop_bounds.height / image.height as f64,
    );
    if !valid_target_rect(rect, image.desktop_bounds) {
        return;
    }
    let Some((role, native_role)) = classify_region(rect, confidence, options) else {
        return;
    };
    let candidate = Reverse(RegionCandidate {
        confidence,
        rect,
        role,
        native_role,
    });
    if candidates.len() < candidate_limit {
        candidates.push(candidate);
    } else if candidates
        .peek()
        .is_some_and(|current| candidate.0 > current.0)
    {
        candidates.pop();
        candidates.push(candidate);
    }
}

#[derive(Clone, Copy)]
struct RegionCandidate {
    confidence: f64,
    rect: Rect,
    role: &'static str,
    native_role: &'static str,
}

impl PartialEq for RegionCandidate {
    fn eq(&self, other: &Self) -> bool {
        self.confidence.to_bits() == other.confidence.to_bits()
    }
}

impl Eq for RegionCandidate {}

impl PartialOrd for RegionCandidate {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for RegionCandidate {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.confidence.total_cmp(&other.confidence)
    }
}

fn classify_region(
    rect: Rect,
    confidence: f64,
    options: &crate::api::VisionOptions,
) -> Option<(&'static str, &'static str)> {
    let aspect = rect.width / rect.height.max(f64::EPSILON);
    if rect.width <= options.checkbox_max_size
        && rect.height <= options.checkbox_max_size
        && (0.75..=1.35).contains(&aspect)
    {
        Some(("checkbox", "vision:rust-checkbox"))
    } else if confidence >= options.button_min_confidence
        && (options.button_min_aspect..=options.button_max_aspect).contains(&aspect)
    {
        Some(("button", "vision:rust-button"))
    } else if rect.width >= options.image_min_size && rect.height >= options.image_min_size {
        Some(("image", "vision:rust-image"))
    } else if confidence >= options.generic_clickable_min_confidence {
        Some(("control", "vision:rust-rectangle"))
    } else {
        None
    }
}
