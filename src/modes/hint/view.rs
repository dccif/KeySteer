//! Deterministic visual-layer planning for overlapping UI Hint labels.

use smallvec::SmallVec;

use crate::api::geometry::Rect;

const INLINE_LABELS: usize = 128;
const COMPACT_UNSTACKED: u16 = u16::MAX;
const WIDE_UNSTACKED: u32 = u32::MAX;
const UNCOLORED: u16 = u16::MAX;

#[derive(Debug, Default)]
struct WideVisualLayerWorkspace {
    graph_rows: Vec<u64>,
    degrees: Vec<u16>,
    visited: Vec<bool>,
    packed: Vec<u32>,
    component: Vec<usize>,
    pending: Vec<usize>,
    best: Vec<u16>,
    colors: Vec<u16>,
    order: Vec<usize>,
    occupied: Vec<u64>,
    frontmost: Vec<usize>,
    classes: Vec<(u16, usize)>,
    remap: Vec<u16>,
    sweep_order: Vec<usize>,
    sweep_active: Vec<usize>,
}

#[derive(Debug)]
// Keeping the common <=128-Hint plan inline avoids a heap allocation on every
// rebuild; boxing the rare wide variant would only add another allocation.
#[allow(clippy::large_enum_variant)]
enum LayerStorage {
    Compact(SmallVec<[u16; INLINE_LABELS]>),
    Wide(Vec<u32>),
}

impl Default for LayerStorage {
    fn default() -> Self {
        Self::Compact(SmallVec::new())
    }
}

/// A full-Hint-indexed visual layer plan.
///
/// Isolated and currently filtered labels have no layer. Each entry packs the
/// component-local layer and component depth together, so cycling can wrap a
/// shallow overlap group independently without another allocation.
#[derive(Debug, Default)]
pub(crate) struct VisualLayerPlan {
    layers: LayerStorage,
    layer_count: usize,
    ready: bool,
    wide: Option<Box<WideVisualLayerWorkspace>>,
}

impl VisualLayerPlan {
    pub(crate) fn clear(&mut self) {
        match &mut self.layers {
            LayerStorage::Compact(layers) => layers.clear(),
            LayerStorage::Wide(layers) => layers.clear(),
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

    #[cfg(test)]
    pub(crate) fn retained_capacity(&self) -> usize {
        let layers = match &self.layers {
            LayerStorage::Compact(layers) => layers.capacity(),
            LayerStorage::Wide(layers) => layers.capacity(),
        };
        layers.max(
            self.wide
                .as_ref()
                .map_or(0, |wide| wide.graph_rows.capacity()),
        )
    }

    pub(crate) fn release_retained(&mut self) {
        self.layers = LayerStorage::default();
        self.wide = None;
        self.layer_count = 0;
        self.ready = false;
    }

    #[cfg(test)]
    pub(crate) fn reserve_for_test(&mut self, additional: usize) {
        match &mut self.layers {
            LayerStorage::Compact(layers) => layers.reserve(additional),
            LayerStorage::Wide(layers) => layers.reserve(additional),
        }
    }

    #[cfg(test)]
    pub(crate) fn layer(&self, hint_index: usize) -> Option<usize> {
        self.layer_info(hint_index).map(|(layer, _)| layer)
    }

    #[cfg(test)]
    pub(crate) fn component_layer_count(&self, hint_index: usize) -> Option<usize> {
        self.layer_info(hint_index)
            .map(|(_, component_layer_count)| component_layer_count)
    }

    fn layer_info(&self, hint_index: usize) -> Option<(usize, usize)> {
        if !self.ready {
            return None;
        }
        match &self.layers {
            LayerStorage::Compact(layers) => layers
                .get(hint_index)
                .copied()
                .filter(|packed| *packed != COMPACT_UNSTACKED)
                .map(|packed| {
                    (
                        usize::from(packed & u16::from(u8::MAX)),
                        usize::from(packed >> u8::BITS),
                    )
                }),
            LayerStorage::Wide(layers) => layers
                .get(hint_index)
                .copied()
                .filter(|packed| *packed != WIDE_UNSTACKED)
                .map(|packed| {
                    (
                        (packed & u32::from(u16::MAX)) as usize,
                        (packed >> u16::BITS) as usize,
                    )
                }),
        }
    }

    /// Whether this Hint should be raised for a global selection.
    ///
    /// Layer zero is the normal draw order. Non-default selections wrap over
    /// each connected component's own non-default layers, so a two-label
    /// component never disappears merely because another component is deeper.
    #[cfg(test)]
    pub(crate) fn is_selected(&self, hint_index: usize, selected_layer: usize) -> bool {
        self.presentation(hint_index, selected_layer)
            .is_some_and(|(selected, _)| selected)
    }

    /// Component-local draw rank for the current global Shift selection.
    ///
    /// The selected layer is always highest. Every other layer keeps a
    /// distinct rank in its normal front-to-back order, so a component with
    /// three or more layers cannot collapse back into one native z value.
    pub(crate) fn draw_rank(&self, hint_index: usize, selected_layer: usize) -> Option<usize> {
        self.presentation(hint_index, selected_layer)
            .map(|(_, draw_rank)| draw_rank)
    }

    fn presentation(&self, hint_index: usize, selected_layer: usize) -> Option<(bool, usize)> {
        let (layer, component_layer_count) = self.layer_info(hint_index)?;
        let selected = selected_component_layer(component_layer_count, selected_layer);
        let local_rank = if layer == selected {
            component_layer_count
        } else {
            let remaining_index = if layer < selected { layer } else { layer - 1 };
            component_layer_count - 1 - remaining_index
        };
        // Align the selected rank across disconnected components. A shallow
        // two-layer group and a deeper group therefore reach the same top z,
        // while every component still retains its own compact ordering.
        Some((
            layer == selected,
            self.layer_count - component_layer_count + local_rank,
        ))
    }

    fn finish(
        &mut self,
        placements: &[(usize, Rect)],
        hint_count: usize,
        packed_component_layers: &[u32],
        layer_count: usize,
    ) {
        self.layer_count = layer_count;
        self.ready = true;
        if layer_count < usize::from(u8::MAX) {
            if !matches!(self.layers, LayerStorage::Compact(_)) {
                self.layers = LayerStorage::default();
            }
            let LayerStorage::Compact(layers) = &mut self.layers else {
                return;
            };
            layers.resize(hint_count, COMPACT_UNSTACKED);
            layers.fill(COMPACT_UNSTACKED);
            for ((hint_index, _), packed) in placements.iter().zip(packed_component_layers) {
                if *packed != WIDE_UNSTACKED {
                    let layer = (*packed & u32::from(u16::MAX)) as u16;
                    let component_layer_count = (*packed >> u16::BITS) as u16;
                    layers[*hint_index] = (component_layer_count << u8::BITS) | layer;
                }
            }
        } else {
            if !matches!(self.layers, LayerStorage::Wide(_)) {
                self.layers = LayerStorage::Wide(Vec::new());
            }
            let LayerStorage::Wide(layers) = &mut self.layers else {
                return;
            };
            layers.resize(hint_count, WIDE_UNSTACKED);
            layers.fill(WIDE_UNSTACKED);
            for ((hint_index, _), packed) in placements.iter().zip(packed_component_layers) {
                if *packed != WIDE_UNSTACKED {
                    layers[*hint_index] = *packed;
                }
            }
        }
    }
}

fn selected_component_layer(component_layer_count: usize, selected_layer: usize) -> usize {
    if selected_layer == 0 || component_layer_count <= 1 {
        0
    } else {
        (selected_layer - 1) % (component_layer_count - 1) + 1
    }
}

struct ConflictGraph {
    inline_rows: [u128; INLINE_LABELS],
    inline_degrees: [u16; INLINE_LABELS],
    dynamic_rows: Vec<u64>,
    dynamic_degrees: Vec<u16>,
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
            inline_degrees: [0; INLINE_LABELS],
            dynamic_rows: vec![0; len.saturating_mul(words)],
            dynamic_degrees: if words == 0 { Vec::new() } else { vec![0; len] },
            len,
            words,
        }
    }

    fn new_wide(len: usize, mut rows: Vec<u64>, mut degrees: Vec<u16>) -> Self {
        let words = len.div_ceil(64);
        rows.resize(len.saturating_mul(words), 0);
        rows.fill(0);
        degrees.resize(len, 0);
        degrees.fill(0);
        Self {
            inline_rows: [0; INLINE_LABELS],
            inline_degrees: [0; INLINE_LABELS],
            dynamic_rows: rows,
            dynamic_degrees: degrees,
            len,
            words,
        }
    }

    fn recycle(self, wide: &mut WideVisualLayerWorkspace) {
        wide.graph_rows = self.dynamic_rows;
        wide.degrees = self.dynamic_degrees;
    }

    fn add_edge(&mut self, left: usize, right: usize) {
        if self.words == 0 {
            self.inline_rows[left] |= 1u128 << right;
            self.inline_rows[right] |= 1u128 << left;
            self.inline_degrees[left] += 1;
            self.inline_degrees[right] += 1;
        } else {
            self.dynamic_rows[left * self.words + right / 64] |= 1u64 << (right % 64);
            self.dynamic_rows[right * self.words + left / 64] |= 1u64 << (left % 64);
            self.dynamic_degrees[left] += 1;
            self.dynamic_degrees[right] += 1;
        }
    }

    fn len(&self) -> usize {
        self.len
    }

    fn degree(&self, vertex: usize) -> u16 {
        if self.words == 0 {
            self.inline_degrees[vertex]
        } else {
            self.dynamic_degrees[vertex]
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
        let packed_component_layers: SmallVec<[u32; INLINE_LABELS]> =
            SmallVec::from_elem(WIDE_UNSTACKED, placements.len());
        plan.finish(placements, hint_count, &packed_component_layers, 0);
        return;
    }

    if placements.len() > INLINE_LABELS {
        let mut wide = plan.wide.take().unwrap_or_default();
        build_wide_plan(placements, hint_count, &visually_stacked, plan, &mut wide);
        plan.wide = Some(wide);
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
    let mut packed_component_layers: SmallVec<[u32; INLINE_LABELS]> =
        SmallVec::from_elem(WIDE_UNSTACKED, graph.len());
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

        if component.len() == 2 {
            packed_component_layers[component[0]] = (2u32 << u16::BITS) | 1;
            packed_component_layers[component[1]] = 2u32 << u16::BITS;
            global_layer_count = global_layer_count.max(2);
            continue;
        }

        let (colors, layer_count) = color_component(&graph, placements, &component);
        global_layer_count = global_layer_count.max(layer_count);
        for &vertex in &component {
            packed_component_layers[vertex] =
                ((layer_count as u32) << u16::BITS) | u32::from(colors[vertex]);
        }
    }

    plan.finish(
        placements,
        hint_count,
        &packed_component_layers,
        global_layer_count,
    );
}

fn build_wide_plan(
    placements: &[(usize, Rect)],
    hint_count: usize,
    visually_stacked: &impl Fn(Rect, Rect) -> bool,
    plan: &mut VisualLayerPlan,
    wide: &mut WideVisualLayerWorkspace,
) {
    let len = placements.len();
    let mut graph = ConflictGraph::new_wide(
        len,
        std::mem::take(&mut wide.graph_rows),
        std::mem::take(&mut wide.degrees),
    );
    build_wide_edges(
        placements,
        visually_stacked,
        &mut graph,
        &mut wide.sweep_order,
        &mut wide.sweep_active,
    );
    wide.visited.resize(len, false);
    wide.visited.fill(false);
    wide.packed.resize(len, WIDE_UNSTACKED);
    wide.packed.fill(WIDE_UNSTACKED);
    wide.best.resize(len, UNCOLORED);
    wide.colors.resize(len, UNCOLORED);
    wide.component.clear();
    wide.pending.clear();
    wide.order.clear();
    let mut global = 0usize;
    for root in 0..len {
        if wide.visited[root] || graph.degree(root) == 0 {
            wide.visited[root] = true;
            continue;
        }
        wide.component.clear();
        wide.pending.clear();
        wide.visited[root] = true;
        wide.pending.push(root);
        while let Some(vertex) = wide.pending.pop() {
            wide.component.push(vertex);
            graph.for_each_neighbor(vertex, |neighbor| {
                if !wide.visited[neighbor] {
                    wide.visited[neighbor] = true;
                    wide.pending.push(neighbor);
                }
            });
        }
        wide.component.sort_unstable();
        let depth = if wide.component.len() == 2 {
            wide.best[wide.component[0]] = 1;
            wide.best[wide.component[1]] = 0;
            2
        } else {
            color_component_wide(&graph, placements, wide)
        };
        global = global.max(depth);
        for &vertex in &wide.component {
            wide.packed[vertex] = (depth as u32) << u16::BITS | u32::from(wide.best[vertex]);
        }
    }
    plan.finish(placements, hint_count, &wide.packed, global);
    graph.recycle(wide);
}

fn build_wide_edges(
    placements: &[(usize, Rect)],
    visually_stacked: &impl Fn(Rect, Rect) -> bool,
    graph: &mut ConflictGraph,
    order: &mut Vec<usize>,
    active: &mut Vec<usize>,
) {
    let valid = placements.iter().all(|(_, rect)| {
        rect.x.is_finite()
            && rect.y.is_finite()
            && rect.width.is_finite()
            && rect.height.is_finite()
            && rect.width >= 0.0
            && rect.height >= 0.0
    });
    if !valid {
        for right in 1..placements.len() {
            for left in 0..right {
                if visually_stacked(placements[left].1, placements[right].1) {
                    graph.add_edge(left, right);
                }
            }
        }
        return;
    }
    order.clear();
    order.extend(0..placements.len());
    order.sort_unstable_by(|left, right| {
        placements[*left]
            .1
            .x
            .total_cmp(&placements[*right].1.x)
            .then_with(|| left.cmp(right))
    });
    active.clear();
    for &right in order.iter() {
        let right_rect = placements[right].1;
        active.retain(|left| placements[*left].1.right() >= right_rect.x);
        for &left in active.iter() {
            let left_rect = placements[left].1;
            if left_rect.intersect(&right_rect).is_some() && visually_stacked(left_rect, right_rect)
            {
                graph.add_edge(left, right);
            }
        }
        active.push(right);
    }
}

fn color_component_wide(
    graph: &ConflictGraph,
    placements: &[(usize, Rect)],
    wide: &mut WideVisualLayerWorkspace,
) -> usize {
    let component = wide.component.as_slice();
    for &vertex in component {
        wide.best[vertex] = UNCOLORED;
        wide.colors[vertex] = UNCOLORED;
    }
    wide.occupied.resize(component.len().div_ceil(64), 0);
    let mut best_depth = usize::MAX;
    let mut best_agreement = 0usize;
    for candidate in CANDIDATE_ORDERS {
        wide.order.clear();
        wide.order.extend_from_slice(component);
        prepare_order_slice(candidate, placements, graph, &mut wide.order);
        for &vertex in component {
            wide.colors[vertex] = UNCOLORED;
        }
        greedy_color(graph, &wide.order, &mut wide.colors, &mut wide.occupied);
        for _ in 0..2 {
            compact_colors(graph, component, &mut wide.colors, &mut wide.occupied);
        }
        let depth = canonicalize_colors_wide(
            component,
            &mut wide.colors,
            &mut wide.frontmost,
            &mut wide.classes,
            &mut wide.remap,
        );
        let agreement = visual_agreement(graph, component, &wide.colors);
        let better = depth < best_depth
            || (depth == best_depth
                && (agreement > best_agreement
                    || (agreement == best_agreement
                        && lexicographically_better(component, &wide.colors, &wide.best))));
        if better {
            best_depth = depth;
            best_agreement = agreement;
            for &vertex in component {
                wide.best[vertex] = wide.colors[vertex];
            }
        }
    }
    best_depth
}

fn canonicalize_colors_wide(
    component: &[usize],
    colors: &mut [u16],
    frontmost: &mut Vec<usize>,
    classes: &mut Vec<(u16, usize)>,
    remap: &mut Vec<u16>,
) -> usize {
    let count = component
        .iter()
        .map(|vertex| usize::from(colors[*vertex]))
        .max()
        .map_or(0, |color| color + 1);
    frontmost.resize(count, usize::MAX);
    frontmost.fill(usize::MAX);
    for &vertex in component {
        let slot = &mut frontmost[usize::from(colors[vertex])];
        *slot = if *slot == usize::MAX {
            vertex
        } else {
            (*slot).max(vertex)
        };
    }
    classes.clear();
    classes.extend(
        frontmost
            .iter()
            .copied()
            .enumerate()
            .filter(|(_, front)| *front != usize::MAX)
            .map(|(color, front)| (color as u16, front)),
    );
    classes.sort_unstable_by(|(lc, lf), (rc, rf)| rf.cmp(lf).then_with(|| lc.cmp(rc)));
    remap.resize(count, UNCOLORED);
    remap.fill(UNCOLORED);
    for (new, (old, _)) in classes.iter().enumerate() {
        remap[usize::from(*old)] = new as u16;
    }
    for &vertex in component {
        colors[vertex] = remap[usize::from(colors[vertex])];
    }
    classes.len()
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
    prepare_order_slice(candidate, placements, graph, order);
}

fn prepare_order_slice(
    candidate: CandidateOrder,
    placements: &[(usize, Rect)],
    graph: &ConflictGraph,
    order: &mut [usize],
) {
    match candidate {
        CandidateOrder::FrontToBack => order.reverse(),
        CandidateOrder::BackToFront => {}
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

    fn quadratic_reference(placements: &[(usize, Rect)], plan: &mut VisualLayerPlan) {
        plan.clear();
        let mut graph = ConflictGraph::new(placements.len());
        for right in 1..placements.len() {
            for left in 0..right {
                if overlap(placements[left].1, placements[right].1) {
                    graph.add_edge(left, right);
                }
            }
        }
        let mut visited = vec![false; graph.len()];
        let mut packed = vec![WIDE_UNSTACKED; graph.len()];
        let mut global = 0usize;
        for root in 0..graph.len() {
            if visited[root] || graph.degree(root) == 0 {
                visited[root] = true;
                continue;
            }
            let mut component = Vec::new();
            let mut pending = vec![root];
            visited[root] = true;
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
            let (colors, depth) = color_component(&graph, placements, &component);
            global = global.max(depth);
            for &vertex in &component {
                packed[vertex] = (depth as u32) << u16::BITS | u32::from(colors[vertex]);
            }
        }
        plan.finish(placements, placements.len(), &packed, global);
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
        assert_eq!(plan.component_layer_count(1), Some(2));
        assert_eq!(plan.component_layer_count(3), Some(2));
        assert_eq!(plan.component_layer_count(4), None);
    }

    #[test]
    fn global_selection_wraps_each_components_non_default_layers() {
        let placements = [
            (0, Rect::new(0.0, 0.0, 20.0, 20.0)),
            (1, Rect::new(0.0, 0.0, 20.0, 20.0)),
            (2, Rect::new(100.0, 0.0, 20.0, 20.0)),
            (3, Rect::new(100.0, 0.0, 20.0, 20.0)),
            (4, Rect::new(100.0, 0.0, 20.0, 20.0)),
        ];
        let mut plan = VisualLayerPlan::default();
        build_visual_layer_plan(&placements, placements.len(), overlap, &mut plan);

        assert_eq!(plan.layer_count(), 3);
        for selected_layer in 1..=4 {
            assert_eq!(
                (0..2)
                    .filter(|index| plan.is_selected(*index, selected_layer))
                    .count(),
                1,
                "the shallow component must be switchable at global selection {selected_layer}"
            );
            assert_eq!(
                (2..5)
                    .filter(|index| plan.is_selected(*index, selected_layer))
                    .count(),
                1,
                "the deep component must select one local layer at {selected_layer}"
            );
        }
    }

    #[test]
    fn every_component_layer_has_a_distinct_rank_and_selection_is_topmost() {
        let placements = [
            (0, Rect::new(0.0, 0.0, 20.0, 20.0)),
            (1, Rect::new(0.0, 0.0, 20.0, 20.0)),
            (2, Rect::new(100.0, 0.0, 20.0, 20.0)),
            (3, Rect::new(100.0, 0.0, 20.0, 20.0)),
            (4, Rect::new(100.0, 0.0, 20.0, 20.0)),
        ];
        let mut plan = VisualLayerPlan::default();
        build_visual_layer_plan(&placements, placements.len(), overlap, &mut plan);

        for selected in 0..plan.layer_count() {
            let global_top = plan.layer_count();
            for component in [&[0, 1][..], &[2, 3, 4][..]] {
                let mut ranks = component
                    .iter()
                    .map(|index| plan.draw_rank(*index, selected).unwrap())
                    .collect::<Vec<_>>();
                assert!(ranks.contains(&global_top));
                ranks.sort_unstable();
                ranks.dedup();
                assert_eq!(ranks.len(), component.len());
            }
        }
    }

    #[test]
    fn wide_boundaries_cover_sparse_pairs_dense_and_dynamic_fallback() {
        for count in [127, 128, 129, 192, 256] {
            let placements = (0..count)
                .map(|index| (index, Rect::new(index as f64 * 100.0, 0.0, 20.0, 20.0)))
                .collect::<Vec<_>>();
            let mut plan = VisualLayerPlan::default();
            build_visual_layer_plan(&placements, count, overlap, &mut plan);
            assert_eq!(plan.layer_count(), 0, "count={count}");
        }
        let pairs = (0..128)
            .flat_map(|pair| {
                let rect = Rect::new(pair as f64 * 100.0, 0.0, 20.0, 20.0);
                [(pair * 2, rect), (pair * 2 + 1, rect)]
            })
            .collect::<Vec<_>>();
        let mut plan = VisualLayerPlan::default();
        build_visual_layer_plan(&pairs, pairs.len(), overlap, &mut plan);
        assert_eq!(plan.layer_count(), 2);
        for pair in 0..128 {
            assert_eq!(plan.layer(pair * 2), Some(1));
            assert_eq!(plan.layer(pair * 2 + 1), Some(0));
        }
        plan.release_retained();

        let dense = (0..256)
            .map(|index| (index, Rect::new(0.0, 0.0, 20.0, 20.0)))
            .collect::<Vec<_>>();
        build_visual_layer_plan(&dense, dense.len(), overlap, &mut plan);
        assert_eq!(plan.layer_count(), 256);

        let dynamic = (0..2_000)
            .map(|index| (index, Rect::new(index as f64 * 100.0, 0.0, 20.0, 20.0)))
            .collect::<Vec<_>>();
        build_visual_layer_plan(&dynamic, dynamic.len(), overlap, &mut plan);
        assert_eq!(plan.layer_count(), 0);
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
        let p50 = samples[(SAMPLES - 1) * 50 / 100];
        let p95 = samples[(SAMPLES - 1) * 95 / 100];
        let p99 = samples[(SAMPLES - 1) * 99 / 100];
        println!("visual_layer_128 samples={SAMPLES} p50={p50}ns p95={p95}ns p99={p99}ns");
        assert!(p99 < 100_000, "128-label visual-layer p99 was {p99}ns");
    }

    #[test]
    #[ignore = "microbenchmark probe; run in release with --test-threads=1"]
    fn common_256_pair_performance_probe() {
        const WARMUP: usize = 2_000;
        const SAMPLES: usize = 20_000;
        let placements = (0..128)
            .flat_map(|pair| {
                let rect = Rect::new(pair as f64 * 100.0, 0.0, 20.0, 20.0);
                [(pair * 2, rect), (pair * 2 + 1, rect)]
            })
            .collect::<Vec<_>>();
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
        let mut reference = VisualLayerPlan::default();
        let mut reference_samples = Vec::with_capacity(SAMPLES);
        for _ in 0..SAMPLES {
            let started = std::time::Instant::now();
            quadratic_reference(&placements, &mut reference);
            reference_samples.push(started.elapsed().as_nanos());
        }
        reference_samples.sort_unstable();
        println!(
            "visual_layer_256_pairs samples={SAMPLES} optimized_p50={}ns optimized_p95={}ns optimized_p99={}ns reference_p50={}ns reference_p95={}ns reference_p99={}ns",
            samples[(SAMPLES - 1) * 50 / 100],
            samples[(SAMPLES - 1) * 95 / 100],
            samples[(SAMPLES - 1) * 99 / 100],
            reference_samples[(SAMPLES - 1) * 50 / 100],
            reference_samples[(SAMPLES - 1) * 95 / 100],
            reference_samples[(SAMPLES - 1) * 99 / 100],
        );
    }
}
