//! Mode instances, compiled routes, plugin metadata, and modal state.

use super::*;

pub(super) struct TemporaryChord {
    pub(super) chord: KeyChord,
    pub(super) conflicts_with_ui_hint_overlap: bool,
}

struct ModeSlot {
    id: ModeId,
    mode: Box<dyn Mode>,
    pointer_events: bool,
    table: Option<CompiledKeymap>,
    temporary_chords: Vec<TemporaryChord>,
}

pub(super) struct ModeRegistry {
    slots: Vec<ModeSlot>,
    indices: BTreeMap<ModeId, usize>,
    pub(super) routes: BTreeMap<ModeId, ModeRoute>,
    pub(super) active: ModeId,
    pub(super) active_slot: Option<usize>,
    pub(super) binding_profile_key: Vec<Bindings>,
    #[cfg(test)]
    pub(super) table_rebuild_count: usize,
    pub(super) plugin_bindings: Vec<(KeyChord, Binding)>,
    pub(super) plugin_verbs: BTreeMap<String, ModeId>,
    pub(super) modal_stack: Vec<ModeId>,
}

impl Default for ModeRegistry {
    fn default() -> Self {
        Self {
            slots: Vec::new(),
            indices: BTreeMap::new(),
            routes: BTreeMap::new(),
            active: ModeId::idle(),
            active_slot: None,
            binding_profile_key: Vec::new(),
            #[cfg(test)]
            table_rebuild_count: 0,
            plugin_bindings: Vec::new(),
            plugin_verbs: BTreeMap::new(),
            modal_stack: Vec::new(),
        }
    }
}

impl ModeRegistry {
    pub(super) fn with_routes(routes: BTreeMap<ModeId, ModeRoute>) -> Self {
        Self {
            routes,
            ..Self::default()
        }
    }

    pub(super) fn insert(
        &mut self,
        id: ModeId,
        mode: Box<dyn Mode>,
        pointer_events: bool,
    ) -> usize {
        if let Some(&index) = self.indices.get(&id) {
            self.slots[index].mode = mode;
            self.slots[index].pointer_events = pointer_events;
            return index;
        }
        let index = self.slots.len();
        self.slots.push(ModeSlot {
            id: id.clone(),
            mode,
            pointer_events,
            table: None,
            temporary_chords: Vec::new(),
        });
        self.indices.insert(id, index);
        index
    }

    pub(super) fn contains_key(&self, id: &ModeId) -> bool {
        self.indices.contains_key(id)
    }

    pub(super) fn index_of(&self, id: &ModeId) -> Option<usize> {
        self.indices.get(id).copied()
    }

    pub(super) fn keys(&self) -> impl Iterator<Item = &ModeId> {
        self.indices.keys()
    }

    pub(super) fn get(&self, id: &ModeId) -> Option<&dyn Mode> {
        let index = self.index_of(id)?;
        self.slots.get(index).map(|slot| slot.mode.as_ref())
    }

    pub(super) fn get_mut(&mut self, id: &ModeId) -> Option<&mut Box<dyn Mode>> {
        let index = self.index_of(id)?;
        self.get_index_mut(index)
    }

    pub(super) fn get_index_mut(&mut self, index: usize) -> Option<&mut Box<dyn Mode>> {
        self.slots.get_mut(index).map(|slot| &mut slot.mode)
    }

    pub(super) fn id_at(&self, index: usize) -> Option<&ModeId> {
        self.slots.get(index).map(|slot| &slot.id)
    }

    pub(super) fn wants_pointer_events_at(&self, index: usize) -> Option<bool> {
        self.slots.get(index).map(|slot| slot.pointer_events)
    }

    pub(super) fn table(&self, id: &ModeId) -> Option<&CompiledKeymap> {
        let index = self.index_of(id)?;
        self.slots.get(index)?.table.as_ref()
    }

    pub(super) fn table_mut_or_default(&mut self, id: &ModeId) -> Option<&mut CompiledKeymap> {
        let index = self.index_of(id)?;
        Some(
            self.slots
                .get_mut(index)?
                .table
                .get_or_insert_with(CompiledKeymap::default),
        )
    }

    pub(super) fn temporary_chords(&self, id: &ModeId) -> Option<&[TemporaryChord]> {
        let index = self.index_of(id)?;
        Some(&self.slots.get(index)?.temporary_chords)
    }

    pub(super) fn clear_routes(&mut self) {
        for slot in &mut self.slots {
            slot.table = None;
            slot.temporary_chords.clear();
        }
    }

    pub(super) fn set_routes(
        &mut self,
        id: &ModeId,
        table: Option<CompiledKeymap>,
        temporary_chords: Vec<TemporaryChord>,
    ) {
        if let Some(index) = self.index_of(id)
            && let Some(slot) = self.slots.get_mut(index)
        {
            slot.table = table;
            slot.temporary_chords = temporary_chords;
        }
    }

    pub(super) fn merge_plugin_bindings_into_normal(&mut self) {
        let normal = ModeId::normal();
        let Self {
            slots,
            indices,
            plugin_bindings,
            ..
        } = self;
        let Some(&index) = indices.get(&normal) else {
            return;
        };
        let table = slots[index]
            .table
            .get_or_insert_with(CompiledKeymap::default);
        for (chord, binding) in plugin_bindings.iter() {
            if !table.contains_chord(chord) {
                table.insert(chord.clone(), binding.clone());
            }
        }
    }

    pub(super) fn tables(&self) -> impl Iterator<Item = (&ModeId, &CompiledKeymap)> {
        self.slots
            .iter()
            .filter_map(|slot| slot.table.as_ref().map(|table| (&slot.id, table)))
    }

    #[cfg(test)]
    pub(super) fn table_count(&self) -> usize {
        self.slots
            .iter()
            .filter(|slot| slot.table.is_some())
            .count()
    }
}
