//! Deterministic visual-layer planning for overlapping UI Hint labels.

use smallvec::SmallVec;

use crate::api::geometry::Rect;

pub(crate) const INLINE_LABELS: usize = 128;
pub(crate) const COMPACT_UNSTACKED: u16 = u16::MAX;
pub(crate) const WIDE_UNSTACKED: u32 = u32::MAX;
pub(crate) const UNCOLORED: u16 = u16::MAX;

#[derive(Debug, Default)]
pub(crate) struct WideVisualLayerWorkspace {
    pub(crate) graph_rows: Vec<u64>,
    pub(crate) degrees: Vec<u16>,
    pub(crate) visited: Vec<bool>,
    pub(crate) packed: Vec<u32>,
    pub(crate) component: Vec<usize>,
    pub(crate) pending: Vec<usize>,
    pub(crate) best: Vec<u16>,
    pub(crate) colors: Vec<u16>,
    pub(crate) order: Vec<usize>,
    pub(crate) occupied: Vec<u64>,
    pub(crate) frontmost: Vec<usize>,
    pub(crate) classes: Vec<(u16, usize)>,
    pub(crate) remap: Vec<u16>,
    pub(crate) sweep_order: Vec<usize>,
    pub(crate) sweep_active: Vec<usize>,
}

#[derive(Debug)]
// Keeping the common <=128-Hint plan inline avoids a heap allocation on every
// rebuild; boxing the rare wide variant would only add another allocation.
#[allow(clippy::large_enum_variant)]
pub(crate) enum LayerStorage {
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
pub struct VisualLayerPlan {
    pub(crate) layers: LayerStorage,
    pub(crate) layer_count: usize,
    pub(crate) ready: bool,
    pub(crate) wide: Option<Box<WideVisualLayerWorkspace>>,
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

    #[cfg(test)]
    pub(crate) fn retained_graph_words(&self) -> usize {
        self.wide
            .as_ref()
            .map_or(0, |wide| wide.graph_rows.capacity())
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

    pub(crate) fn layer_info(&self, hint_index: usize) -> Option<(usize, usize)> {
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

    pub(crate) fn finish(
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
