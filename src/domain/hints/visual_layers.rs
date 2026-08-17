//! Deterministic visual-layer planning for overlapping UI Hint labels.

use smallvec::SmallVec;

use crate::api::geometry::Rect;

const INLINE_LABELS: usize = 128;
const COMPACT_UNSTACKED: u8 = u8::MAX;
const WIDE_UNSTACKED: u16 = u16::MAX;
const UNCOLORED: u16 = u16::MAX;

#[derive(Debug)]
enum LayerStorage {
    Compact(SmallVec<[u8; INLINE_LABELS]>),
    Wide(Vec<u16>),
}

impl Default for LayerStorage {
    fn default() -> Self {
        Self::Compact(SmallVec::new())
    }
}

/// A full-Hint-indexed visual layer plan.
///
/// Isolated and currently filtered labels have no layer. Common scenes retain
/// one byte per Hint; only a scene that genuinely needs 255 or more layers
/// switches to the wide representation.
#[derive(Debug, Default)]
pub(crate) struct VisualLayerPlan {
    layers: LayerStorage,
    layer_count: usize,
    ready: bool,
}

impl VisualLayerPlan {
    pub(crate) fn clear(&mut self) {
        match &mut self.layers {
            LayerStorage::Compact(layers) => layers.clear(),
            LayerStorage::Wide(_) => self.layers = LayerStorage::default(),
        }
        self.layer_count = 0;
        self.ready = false;
    }

    pub(crate) fn is_ready(&self) -> bool {
        self.ready
    }

    pub(crate) fn layer_count(&self) -> usize {
        self.layer_count
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        match &self.layers {
            LayerStorage::Compact(layers) => layers.len(),
            LayerStorage::Wide(layers) => layers.len(),
        }
    }

    pub(crate) fn retained_capacity(&self) -> usize {
        match &self.layers {
            LayerStorage::Compact(layers) => layers.capacity(),
            LayerStorage::Wide(layers) => layers.capacity(),
        }
    }

    #[cfg(test)]
    pub(crate) fn reserve_for_test(&mut self, additional: usize) {
        match &mut self.layers {
            LayerStorage::Compact(layers) => layers.reserve(additional),
            LayerStorage::Wide(layers) => layers.reserve(additional),
        }
    }

    pub(crate) fn layer(&self, hint_index: usize) -> Option<usize> {
        if !self.ready {
            return None;
        }
        match &self.layers {
            LayerStorage::Compact(layers) => layers
                .get(hint_index)
                .copied()
                .filter(|layer| *layer != COMPACT_UNSTACKED)
                .map(usize::from),
            LayerStorage::Wide(layers) => layers
                .get(hint_index)
                .copied()
                .filter(|layer| *layer != WIDE_UNSTACKED)
                .map(usize::from),
        }
    }

    fn finish(
        &mut self,
        placements: &[(usize, Rect)],
        hint_count: usize,
        colors: &[u16],
        layer_count: usize,
    ) {
        self.layer_count = layer_count;
        self.ready = true;
        if layer_count < usize::from(COMPACT_UNSTACKED) {
            let mut layers = SmallVec::from_elem(COMPACT_UNSTACKED, hint_count);
            for ((hint_index, _), color) in placements.iter().zip(colors) {
                if *color != UNCOLORED {
                    layers[*hint_index] = *color as u8;
                }
            }
            self.layers = LayerStorage::Compact(layers);
        } else {
            let mut layers = vec![WIDE_UNSTACKED; hint_count];
            for ((hint_index, _), color) in placements.iter().zip(colors) {
                if *color != UNCOLORED {
                    layers[*hint_index] = *color;
                }
            }
            self.layers = LayerStorage::Wide(layers);
        }
    }
}

struct ConflictGraph {
    inline_rows: [u128; INLINE_LABELS],
    dynamic_rows: Vec<u64>,
    len: usize,
    words: usize,
}

impl ConflictGraph {
    fn new(len: usize) -> Self {
        let words = if len <= INLINE_LABELS {
            0
        } else {
            len.div_ceil(64)
        };
        Self {
            inline_rows: [0; INLINE_LABELS],
            dynamic_rows: vec![0; len.saturating_mul(words)],
            len,
            words,
        }
    }

    fn add_edge(&mut self, left: usize, right: usize) {
        if self.words == 0 {
            self.inline_rows[left] |= 1u128 << right;
            self.inline_rows[right] |= 1u128 << left;
        } else {
            self.dynamic_rows[left * self.words + right / 64] |= 1u64 << (right % 64);
            self.dynamic_rows[right * self.words + left / 64] |= 1u64 << (left % 64);
        }
    }

    fn len(&self) -> usize {
        self.len
    }

    fn degree(&self, vertex: usize) -> u32 {
        if self.words == 0 {
            self.inline_rows[vertex].count_ones()
        } else {
            self.dynamic_rows[vertex * self.words..vertex * self.words + self.words]
                .iter()
                .map(|word| word.count_ones())
                .sum()
        }
    }

    fn for_each_neighbor(&self, vertex: usize, mut visit: impl FnMut(usize)) {
        if self.words == 0 {
            let mut neighbors = self.inline_rows[vertex];
            while neighbors != 0 {
                let neighbor = neighbors.trailing_zeros() as usize;
                neighbors &= neighbors - 1;
                visit(neighbor);
            }
        } else {
            let row = &self.dynamic_rows[vertex * self.words..vertex * self.words + self.words];
            for (word_index, word) in row.iter().copied().enumerate() {
                let mut neighbors = word;
                while neighbors != 0 {
                    let neighbor = word_index * 64 + neighbors.trailing_zeros() as usize;
                    neighbors &= neighbors - 1;
                    if neighbor < self.len {
                        visit(neighbor);
                    }
                }
            }
        }
    }
}

#[derive(Clone, Copy)]
enum CandidateOrder {
    FrontToBack,
    BackToFront,
    Degree,
    Rows,
    Columns,
}

const CANDIDATE_ORDERS: [CandidateOrder; 5] = [
    CandidateOrder::FrontToBack,
    CandidateOrder::BackToFront,
    CandidateOrder::Degree,
    CandidateOrder::Rows,
    CandidateOrder::Columns,
];

/// Build deterministic global layers without changing label geometry.
///
/// `placements` must follow final draw order and carries the corresponding
/// full Hint index. Disconnected overlap components are colored independently
/// and then share the same global depth numbers.
pub(crate) fn build_visual_layer_plan(
    placements: &[(usize, Rect)],
    hint_count: usize,
    visually_stacked: impl Fn(Rect, Rect) -> bool,
    plan: &mut VisualLayerPlan,
) {
    plan.clear();
    if placements.len() < 2 {
        let colors: SmallVec<[u16; INLINE_LABELS]> =
            SmallVec::from_elem(UNCOLORED, placements.len());
        plan.finish(placements, hint_count, &colors, 0);
        return;
    }

    let mut graph = ConflictGraph::new(placements.len());
    for right in 1..placements.len() {
        for left in 0..right {
            if visually_stacked(placements[left].1, placements[right].1) {
                graph.add_edge(left, right);
            }
        }
    }

    let mut visited: SmallVec<[bool; INLINE_LABELS]> = SmallVec::from_elem(false, graph.len());
    let mut final_colors: SmallVec<[u16; INLINE_LABELS]> =
        SmallVec::from_elem(UNCOLORED, graph.len());
    let mut global_layer_count = 0usize;

    for root in 0..graph.len() {
        if visited[root] || graph.degree(root) == 0 {
            visited[root] = true;
            continue;
        }
        let mut component: SmallVec<[usize; INLINE_LABELS]> = SmallVec::new();
        let mut pending: SmallVec<[usize; INLINE_LABELS]> = SmallVec::new();
        visited[root] = true;
        pending.push(root);
        while let Some(vertex) = pending.pop() {
            component.push(vertex);
            graph.for_each_neighbor(vertex, |neighbor| {
                if !visited[neighbor] {
                    visited[neighbor] = true;
                    pending.push(neighbor);
                }
            });
        }
        component.sort_unstable();

        let (colors, layer_count) = color_component(&graph, placements, &component);
        global_layer_count = global_layer_count.max(layer_count);
        for &vertex in &component {
            final_colors[vertex] = colors[vertex];
        }
    }

    plan.finish(placements, hint_count, &final_colors, global_layer_count);
}

fn color_component(
    graph: &ConflictGraph,
    placements: &[(usize, Rect)],
    component: &[usize],
) -> (SmallVec<[u16; INLINE_LABELS]>, usize) {
    let mut best_colors: SmallVec<[u16; INLINE_LABELS]> =
        SmallVec::from_elem(UNCOLORED, graph.len());
    let mut best_layer_count = usize::MAX;
    let mut best_agreement = 0usize;
    let mut order: SmallVec<[usize; INLINE_LABELS]> = SmallVec::new();
    let mut colors: SmallVec<[u16; INLINE_LABELS]> = SmallVec::from_elem(UNCOLORED, graph.len());
    let mut occupied: SmallVec<[u64; 4]> = SmallVec::from_elem(0, component.len().div_ceil(64));

    for candidate in CANDIDATE_ORDERS {
        prepare_order(candidate, placements, graph, component, &mut order);
        for &vertex in component {
            colors[vertex] = UNCOLORED;
        }
        greedy_color(graph, &order, &mut colors, &mut occupied);
        for _ in 0..2 {
            compact_colors(graph, component, &mut colors, &mut occupied);
        }
        let layer_count = canonicalize_colors(component, &mut colors);
        let agreement = visual_agreement(graph, component, &colors);
        let better = layer_count < best_layer_count
            || (layer_count == best_layer_count
                && (agreement > best_agreement
                    || (agreement == best_agreement
                        && lexicographically_better(component, &colors, &best_colors))));
        if better {
            best_layer_count = layer_count;
            best_agreement = agreement;
            for &vertex in component {
                best_colors[vertex] = colors[vertex];
            }
        }
    }

    (best_colors, best_layer_count)
}

fn prepare_order(
    candidate: CandidateOrder,
    placements: &[(usize, Rect)],
    graph: &ConflictGraph,
    component: &[usize],
    order: &mut SmallVec<[usize; INLINE_LABELS]>,
) {
    order.clear();
    order.extend_from_slice(component);
    match candidate {
        CandidateOrder::FrontToBack => order.sort_unstable_by(|left, right| right.cmp(left)),
        CandidateOrder::BackToFront => order.sort_unstable(),
        CandidateOrder::Degree => order.sort_unstable_by(|left, right| {
            graph
                .degree(*right)
                .cmp(&graph.degree(*left))
                .then_with(|| right.cmp(left))
        }),
        CandidateOrder::Rows => order.sort_unstable_by(|left, right| {
            let left_rect = placements[*left].1;
            let right_rect = placements[*right].1;
            left_rect
                .y
                .total_cmp(&right_rect.y)
                .then_with(|| left_rect.x.total_cmp(&right_rect.x))
                .then_with(|| left.cmp(right))
        }),
        CandidateOrder::Columns => order.sort_unstable_by(|left, right| {
            let left_rect = placements[*left].1;
            let right_rect = placements[*right].1;
            left_rect
                .x
                .total_cmp(&right_rect.x)
                .then_with(|| left_rect.y.total_cmp(&right_rect.y))
                .then_with(|| left.cmp(right))
        }),
    }
}

fn greedy_color(graph: &ConflictGraph, order: &[usize], colors: &mut [u16], occupied: &mut [u64]) {
    for &vertex in order {
        occupied.fill(0);
        graph.for_each_neighbor(vertex, |neighbor| {
            let color = colors[neighbor];
            if color != UNCOLORED {
                let color = usize::from(color);
                occupied[color / 64] |= 1u64 << (color % 64);
            }
        });
        colors[vertex] = first_free_color(occupied) as u16;
    }
}

fn compact_colors(
    graph: &ConflictGraph,
    component: &[usize],
    colors: &mut [u16],
    occupied: &mut [u64],
) {
    for &vertex in component.iter().rev() {
        occupied.fill(0);
        graph.for_each_neighbor(vertex, |neighbor| {
            let color = colors[neighbor];
            if color != UNCOLORED {
                let color = usize::from(color);
                occupied[color / 64] |= 1u64 << (color % 64);
            }
        });
        let first_free = first_free_color(occupied) as u16;
        if first_free < colors[vertex] {
            colors[vertex] = first_free;
        }
    }
}

fn first_free_color(occupied: &[u64]) -> usize {
    occupied
        .iter()
        .enumerate()
        .find_map(|(word_index, word)| {
            (*word != u64::MAX).then(|| word_index * 64 + (!word).trailing_zeros() as usize)
        })
        .unwrap_or(occupied.len() * 64)
}

fn canonicalize_colors(component: &[usize], colors: &mut [u16]) -> usize {
    let max_color = component
        .iter()
        .map(|vertex| colors[*vertex])
        .max()
        .unwrap_or(0) as usize;
    let mut frontmost: SmallVec<[Option<usize>; INLINE_LABELS]> =
        SmallVec::from_elem(None, max_color + 1);
    for &vertex in component {
        let entry = &mut frontmost[usize::from(colors[vertex])];
        *entry = Some(entry.map_or(vertex, |current| current.max(vertex)));
    }
    let mut classes: SmallVec<[(u16, usize); INLINE_LABELS]> = frontmost
        .into_iter()
        .enumerate()
        .filter_map(|(color, front)| front.map(|front| (color as u16, front)))
        .collect();
    classes.sort_unstable_by(|(left_color, left_front), (right_color, right_front)| {
        right_front
            .cmp(left_front)
            .then_with(|| left_color.cmp(right_color))
    });
    let mut remap: SmallVec<[u16; INLINE_LABELS]> = SmallVec::from_elem(UNCOLORED, max_color + 1);
    for (new_color, (old_color, _)) in classes.iter().enumerate() {
        remap[usize::from(*old_color)] = new_color as u16;
    }
    for &vertex in component {
        colors[vertex] = remap[usize::from(colors[vertex])];
    }
    classes.len()
}

fn visual_agreement(graph: &ConflictGraph, component: &[usize], colors: &[u16]) -> usize {
    let mut agreement = 0usize;
    for &back in component {
        graph.for_each_neighbor(back, |front| {
            if front > back && colors[front] < colors[back] {
                agreement += 1;
            }
        });
    }
    agreement
}

fn lexicographically_better(component: &[usize], candidate: &[u16], current: &[u16]) -> bool {
    component
        .iter()
        .map(|vertex| candidate[*vertex])
        .cmp(component.iter().map(|vertex| current[*vertex]))
        .is_lt()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overlap(left: Rect, right: Rect) -> bool {
        left.intersect(&right).is_some_and(|intersection| {
            let area = intersection.width * intersection.height;
            let smaller = (left.width * left.height).min(right.width * right.height);
            smaller > 0.0 && area >= smaller * 0.20
        })
    }

    #[test]
    fn path_counterexample_uses_two_layers_in_every_draw_order() {
        let source = [
            Rect::new(0.0, 0.0, 20.0, 20.0),
            Rect::new(15.0, 0.0, 20.0, 20.0),
            Rect::new(30.0, 0.0, 20.0, 20.0),
            Rect::new(45.0, 0.0, 20.0, 20.0),
        ];
        let permutations = [
            [0, 1, 2, 3],
            [0, 1, 3, 2],
            [0, 2, 1, 3],
            [0, 2, 3, 1],
            [0, 3, 1, 2],
            [0, 3, 2, 1],
            [1, 0, 2, 3],
            [1, 0, 3, 2],
            [1, 2, 0, 3],
            [1, 2, 3, 0],
            [1, 3, 0, 2],
            [1, 3, 2, 0],
            [2, 0, 1, 3],
            [2, 0, 3, 1],
            [2, 1, 0, 3],
            [2, 1, 3, 0],
            [2, 3, 0, 1],
            [2, 3, 1, 0],
            [3, 0, 1, 2],
            [3, 0, 2, 1],
            [3, 1, 0, 2],
            [3, 1, 2, 0],
            [3, 2, 0, 1],
            [3, 2, 1, 0],
        ];
        for permutation in permutations {
            let placements: Vec<_> = permutation
                .into_iter()
                .enumerate()
                .map(|(draw_index, source_index)| (draw_index, source[source_index]))
                .collect();
            let mut plan = VisualLayerPlan::default();
            build_visual_layer_plan(&placements, placements.len(), overlap, &mut plan);
            assert_eq!(plan.layer_count(), 2, "permutation {permutation:?}");
        }
    }

    #[test]
    fn plan_is_indexed_by_full_hint_list() {
        let placements = [
            (1, Rect::new(0.0, 0.0, 20.0, 20.0)),
            (3, Rect::new(0.0, 0.0, 20.0, 20.0)),
        ];
        let mut plan = VisualLayerPlan::default();
        build_visual_layer_plan(&placements, 5, overlap, &mut plan);

        assert_eq!(plan.len(), 5);
        assert_eq!(plan.layer(0), None);
        assert_eq!(plan.layer(2), None);
        assert!(plan.layer(1).is_some());
        assert!(plan.layer(3).is_some());
    }

    #[test]
    #[ignore = "microbenchmark probe; run in release with --test-threads=1"]
    fn common_128_label_performance_probe() {
        const WARMUP: usize = 2_000;
        const SAMPLES: usize = 20_000;
        let placements: Vec<_> = (0..128)
            .map(|index| {
                let column = index % 16;
                let row = index / 16;
                (
                    index,
                    Rect::new(column as f64 * 16.0, row as f64 * 16.0, 20.0, 20.0),
                )
            })
            .collect();
        let mut plan = VisualLayerPlan::default();
        for _ in 0..WARMUP {
            build_visual_layer_plan(&placements, placements.len(), overlap, &mut plan);
        }
        let mut samples = Vec::with_capacity(SAMPLES);
        for _ in 0..SAMPLES {
            let started = std::time::Instant::now();
            build_visual_layer_plan(&placements, placements.len(), overlap, &mut plan);
            samples.push(started.elapsed().as_nanos());
        }
        samples.sort_unstable();
        let p99 = samples[(SAMPLES - 1) * 99 / 100];
        println!("visual_layer_128 samples={SAMPLES} p99={p99}ns");
        assert!(p99 < 100_000, "128-label visual-layer p99 was {p99}ns");
    }
}
