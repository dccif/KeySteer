//! Runtime mode registry, route compilation, dispatch, and lifecycle.

use std::collections::{BTreeMap, HashMap};

use crate::api::backend::Backend;
use crate::api::binding::Binding;
use crate::api::command::{FocusedApp, HostContext, Mode, ModeEvent};
use crate::api::input::{KeyChord, ModeId};

use super::input_router::CompiledKeymap;
use super::plan::{Bindings, ModeRoute};
use super::{Engine, RuntimeError, chords_conflict};

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
    pub(super) prefixes_require_modifier: bool,
    pub(super) character_keys: HashMap<char, crate::api::Key>,
    pub(super) window_layout_table: CompiledKeymap,
    pub(super) window_motion_table: CompiledKeymap,
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
            prefixes_require_modifier: true,
            character_keys: HashMap::new(),
            window_layout_table: CompiledKeymap::default(),
            window_motion_table: CompiledKeymap::default(),
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
        // Input routing repeatedly asks for the active mode's table and
        // temporary chords. Reuse the dispatch slot, checking its identity so
        // a mode switch or plan reload cannot expose a stale cached index.
        if let Some(index) = self.active_slot
            && self.id_at(index) == Some(id)
        {
            return Some(index);
        }
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

impl Engine {
    /// Register a mode. Later registrations replace earlier ones with the same
    /// id, which is how a plugin can override a built-in mode.
    pub fn register(&mut self, mode: Box<dyn Mode>) {
        self.insert_mode(mode);
        self.rebuild_tables();
    }

    /// Bootstrap already owns the complete registration sequence. Deferring
    /// compilation lets the backend's initial focused application be folded
    /// into the first and only startup table build.
    pub(crate) fn register_deferred(&mut self, mode: Box<dyn Mode>) {
        self.insert_mode(mode);
    }

    fn insert_mode(&mut self, mode: Box<dyn Mode>) {
        let id = mode.id();
        let pointer_events = mode.wants_pointer_events();
        let index = self.registry.insert(id.clone(), mode, pointer_events);
        if id == self.registry.active {
            self.registry.active_slot = Some(index);
        }
    }

    /// Register a plugin and merge its default chords, without letting them
    /// override bindings the user configured explicitly.
    pub fn register_plugin<P>(&mut self, plugin: Box<P>) -> Result<(), String>
    where
        P: crate::api::plugin::Plugin + 'static,
    {
        self.register_plugin_dyn(plugin)
    }

    /// Same as [`Self::register_plugin`] for an already-boxed trait object,
    /// which is what a plugin loader produces.
    pub fn register_plugin_dyn(
        &mut self,
        plugin: Box<dyn crate::api::plugin::Plugin>,
    ) -> Result<(), String> {
        self.register_plugin_dyn_inner(plugin, true, true)
    }

    pub(crate) fn register_plugin_dyn_deferred(
        &mut self,
        plugin: Box<dyn crate::api::plugin::Plugin>,
    ) -> Result<(), String> {
        // A compiled plan already contains the complete configured keymap.
        // Reintroducing manifest suggestions here would resurrect keys that
        // the user removed or remapped in their bindings table.
        self.register_plugin_dyn_inner(plugin, false, false)
    }

    fn register_plugin_dyn_inner(
        &mut self,
        plugin: Box<dyn crate::api::plugin::Plugin>,
        rebuild: bool,
        include_suggestions: bool,
    ) -> Result<(), String> {
        plugin.manifest().validate()?;
        let id = plugin.id();

        if include_suggestions {
            for chord in plugin.manifest().default_chords.clone() {
                self.registry
                    .plugin_bindings
                    .push((chord, Binding::Mode(id.clone())));
            }
            self.registry
                .plugin_bindings
                .extend(plugin.manifest().default_bindings.clone());
        }
        for verb in plugin.manifest().verbs.clone() {
            if let Some(existing) = self.registry.plugin_verbs.insert(verb.clone(), id.clone()) {
                return Err(RuntimeError::Fatal(format!(
                    "plugin verb {verb:?} is already owned by {existing}"
                ))
                .into_message());
            }
        }
        self.insert_mode(plugin as Box<dyn Mode>);
        if rebuild {
            self.rebuild_tables();
        }
        Ok(())
    }

    pub fn active_mode(&self) -> &ModeId {
        &self.registry.active
    }

    pub(super) fn set_active(&mut self, active: ModeId) {
        self.overlay.key_help_cache = None;
        if active == ModeId::idle() {
            self.overlay.key_help_visible = false;
        }
        self.input.pending_chords.clear();
        self.registry.active_slot = self.registry.index_of(&active);
        self.registry.active = active;
    }

    pub fn registered_modes(&self) -> impl Iterator<Item = &ModeId> {
        self.registry.keys()
    }

    /// Bindings active in `mode`, for diagnostics and tests.
    pub fn bindings_in(&self, mode: &ModeId) -> Vec<(String, Binding)> {
        self.registry
            .table(mode)
            .map(CompiledKeymap::entries)
            .unwrap_or_default()
    }

    pub(super) fn binding_mode_ids(&self) -> Vec<ModeId> {
        let mut ids: Vec<ModeId> = self.registry.keys().cloned().collect();
        // Idle must resolve even before any mode registers, so it is always
        // considered.
        if !ids.contains(&ModeId::idle()) {
            ids.push(ModeId::idle());
        }
        ids
    }

    pub(super) fn binding_profile_key_for(&self, app: Option<&FocusedApp>) -> Vec<Bindings> {
        let ids = self.binding_mode_ids();
        let profile: Vec<Bindings> = ids
            .iter()
            .map(|id| self.resolved_app_overrides(id, app))
            .collect();
        if profile.iter().all(Bindings::is_empty) {
            Vec::new()
        } else {
            profile
        }
    }

    /// Resolve every mode's binding table from the configuration.
    ///
    /// Per-app overrides for the focused application are folded in, so a
    /// binding can differ per application, and disabled entries are dropped.
    pub(super) fn rebuild_tables(&mut self) {
        self.overlay.key_help_cache = None;
        let ids = self.binding_mode_ids();
        let binding_profile_key = self.binding_profile_key_for(self.focused_app.as_ref());

        let ui_hint_overlap_chord = KeyChord::parse(&self.settings.ui_hint_overlap_key).ok();
        let mut routes = Vec::with_capacity(ids.len());
        for id in ids {
            let temporary_chords = self
                .registry
                .routes
                .get(&id)
                .map(|route| {
                    route
                        .temporary_keys
                        .iter()
                        .filter_map(|text| {
                            KeyChord::parse(text).ok().map(|chord| TemporaryChord {
                                conflicts_with_ui_hint_overlap: ui_hint_overlap_chord
                                    .as_ref()
                                    .is_some_and(|overlap| chords_conflict(overlap, &chord)),
                                chord,
                            })
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let table = self.registry.routes.get(&id).map(|route| {
                let merged = self.merge_overrides(&id, &route.bindings);
                CompiledKeymap::compile(merged, &self.settings.resolved_key_aliases)
            });
            routes.push((id, table, temporary_chords));
        }

        self.registry.clear_routes();
        for (id, table, temporary_chords) in routes {
            self.registry.set_routes(&id, table, temporary_chords);
        }

        // A plugin's suggested chord applies in `normal`, which is where the
        // user works, and only if that chord is still free.
        self.registry.merge_plugin_bindings_into_normal();

        // Compile the effective Normal movement language once per profile,
        // including inherited bindings, application overrides and key aliases.
        // Layout input never scans configuration or allocates an action per key.
        let mut directions = BTreeMap::new();
        for (_, table) in self.registry.tables() {
            for entry in table.iter_entries() {
                let Binding::Move(direction) = entry.binding.as_ref() else {
                    continue;
                };
                if self
                    .lookup_inherited(
                        &crate::api::ModeId::normal(),
                        entry.chord.activation_key(),
                        entry.chord.keys(),
                        &mut smallvec::SmallVec::new(),
                    )
                    .is_some_and(|resolved| resolved.binding == entry.binding)
                {
                    directions.insert(entry.chord.canonical(), *direction);
                }
            }
        }
        self.registry.window_motion_table = CompiledKeymap::compile(
            directions
                .iter()
                .map(|(key, direction)| {
                    use crate::api::Direction;
                    use crate::api::window::WindowAction as W;
                    let action = match direction {
                        Direction::Left => W::Left,
                        Direction::Right => W::Right,
                        Direction::Up => W::Up,
                        Direction::Down => W::Down,
                    };
                    (key.clone(), Binding::Window(action))
                })
                .collect(),
            &self.settings.resolved_key_aliases,
        );
        let mut bindings = BTreeMap::new();
        for (key, direction) in &directions {
            bindings.insert(
                key.clone(),
                Binding::Window(crate::api::window::WindowAction::Navigate(*direction)),
            );
        }
        for (key, direction) in directions {
            let Ok(chord) = KeyChord::parse(&key) else {
                continue;
            };
            for (modifier, action) in [
                ("shift", crate::api::window::WindowAction::Split(direction)),
                ("ctrl", crate::api::window::WindowAction::Ratio(direction)),
            ] {
                if !chord.keys().iter().any(|k| {
                    k.as_str() == modifier || k.as_str().ends_with(&format!("_{modifier}"))
                }) {
                    bindings
                        .entry(format!("{modifier}+{key}"))
                        .or_insert(Binding::Window(action));
                }
            }
        }
        self.registry.window_layout_table = CompiledKeymap::compile(
            bindings.into_iter().collect(),
            &self.settings.resolved_key_aliases,
        );

        // A literal character can only select a single-key chord. Keep one
        // interned Key per character instead of searching every mode per input.
        self.registry.character_keys = self
            .registry
            .tables()
            .flat_map(|(_, table)| table.iter_entries())
            .filter(|entry| entry.chord.keys().len() == 1)
            .filter_map(|entry| {
                let key = entry.chord.activation_key();
                Some((key.as_char()?, key.clone()))
            })
            .collect();

        let continuations: Vec<_> = self
            .registry
            .tables()
            .flat_map(|(owner, table)| table.continuation_candidates(owner))
            .collect();
        self.registry.prefixes_require_modifier = continuations.iter().all(|candidate| {
            candidate
                .chord
                .keys()
                .iter()
                .any(crate::api::Key::is_modifier)
        });
        for id in self.binding_mode_ids() {
            if let Some(table) = self.registry.table_mut_or_default(&id) {
                table.compile_prefixes(&continuations);
            }
        }
        self.input.pending_chords.clear();

        self.ui_hint_overlap_chord = ui_hint_overlap_chord;
        self.registry.binding_profile_key = binding_profile_key;
        #[cfg(test)]
        {
            self.registry.table_rebuild_count += 1;
        }
    }

    pub(super) fn sync_character_bindings(&self, backend: &mut dyn Backend) {
        let keys: Vec<_> = self.registry.character_keys.values().cloned().collect();
        backend.set_character_bindings(&keys);
    }

    /// Apply the focused application's overrides to one mode's table.
    fn merge_overrides(&self, mode_id: &ModeId, base: &Bindings) -> Vec<(String, Binding)> {
        let mut merged: BTreeMap<String, Binding> = base.clone();
        for (chord, binding) in self.resolved_app_overrides(mode_id, self.focused_app.as_ref()) {
            merged.insert(chord, binding);
        }
        merged.into_iter().collect()
    }

    fn resolved_app_overrides(&self, mode_id: &ModeId, app: Option<&FocusedApp>) -> Bindings {
        let Some(app) = app else {
            return Bindings::new();
        };
        let mut resolved = Bindings::new();
        if let Some(route) = self.registry.routes.get(mode_id) {
            for entry in &route.app_overrides {
                if app.matches_pattern(&entry.pattern) {
                    resolved.extend(entry.bindings.clone());
                }
            }
        }
        resolved
    }

    pub(super) fn excluded_app_matches(&self, app: Option<&FocusedApp>) -> bool {
        let Some(app) = app else {
            return false;
        };
        self.settings
            .excluded_apps
            .iter()
            .any(|entry| entry.eq_ignore_ascii_case(&app.bundle_id))
    }

    pub(super) fn is_excluded_app(&self) -> bool {
        self.focused_app_excluded
    }

    pub(super) fn context(&self) -> HostContext<'_> {
        HostContext {
            presenter: &crate::presentation::COMPOSER,
            screens: &self.screens,
            cursor: self.cursor,
            focused_app: self.focused_app.as_ref(),
            palette: &self.palette,
        }
    }

    pub(super) fn trace_binding_tables(&self) {
        if !self.settings.debug.enabled || !self.settings.debug.keys {
            return;
        }
        for (mode, table) in self.registry.tables() {
            let entries = table.entries();
            self.trace(
                true,
                "bindings",
                format!("mode={mode} listening={} bindings", entries.len()),
            );
            for (chord, action) in entries {
                self.trace(
                    true,
                    "bindings",
                    format!("  mode={mode} key={chord:?} -> {action:?}"),
                );
            }
            if let Some(route) = self.registry.routes.get(mode) {
                self.trace(
                    true,
                    "bindings",
                    format!(
                        "  inherits={:?} temporary_mode={:?} temporary_keys={:?}",
                        route.inherits, route.temporary_mode, route.temporary_keys
                    ),
                );
            }
        }
    }

    pub(super) fn dispatch(
        &mut self,
        event: ModeEvent,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        let owner = self.registry.active.clone();
        self.dispatch_to(&owner, event, backend)
    }

    fn mode_index_for_dispatch(&mut self, owner: &ModeId) -> Option<usize> {
        if owner == &self.registry.active {
            if let Some(index) = self.registry.active_slot
                && self.registry.id_at(index) == Some(owner)
            {
                return Some(index);
            }
            let index = self.registry.index_of(owner)?;
            self.registry.active_slot = Some(index);
            return Some(index);
        }
        self.registry.index_of(owner)
    }

    /// Deliver an event to a specific registered mode. This is used for normal
    /// pointer controls borrowed by an active label mode.
    pub(super) fn dispatch_to(
        &mut self,
        owner: &ModeId,
        event: ModeEvent,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        if self.overlay.key_help_visible
            && !matches!(event, ModeEvent::Frame { .. } | ModeEvent::PointerMoved(_))
        {
            self.overlay.key_help_cache = None;
        }
        let Some(mode_index) = self.mode_index_for_dispatch(owner) else {
            return Ok(());
        };
        let context = HostContext {
            presenter: &crate::presentation::COMPOSER,
            screens: &self.screens,
            cursor: self.cursor,
            focused_app: self.focused_app.as_ref(),
            palette: &self.palette,
        };
        let Some(mode) = self.registry.get_index_mut(mode_index) else {
            return Ok(());
        };
        let commands = mode.handle(&event, &context);
        crate::support::perf_probe::mark("mode_handled");
        crate::support::perf_probe::mark("commands_ready");
        self.execute_for(owner, commands, backend)
    }

    /// Deliver the one event family whose platform-owned buffers are worth
    /// consuming. Keeping this separate leaves frame, pointer and key dispatch
    /// on the original single-vtable-call hot path.
    pub(super) fn dispatch_owned_to(
        &mut self,
        owner: &ModeId,
        event: ModeEvent,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        self.overlay.key_help_cache = None;
        let Some(mode_index) = self.mode_index_for_dispatch(owner) else {
            return Ok(());
        };
        let context = HostContext {
            presenter: &crate::presentation::COMPOSER,
            screens: &self.screens,
            cursor: self.cursor,
            focused_app: self.focused_app.as_ref(),
            palette: &self.palette,
        };
        let Some(mode) = self.registry.get_index_mut(mode_index) else {
            return Ok(());
        };
        let commands = mode.handle_owned(event, &context);
        crate::support::perf_probe::mark("mode_handled");
        crate::support::perf_probe::mark("commands_ready");
        self.execute_for(owner, commands, backend)
    }

    pub(super) fn push_mode(
        &mut self,
        target: ModeId,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        if target == self.registry.active || !self.registry.contains_key(&target) {
            return Ok(());
        }
        if self.registry.active == ModeId::normal()
            && target != ModeId::normal()
            && !Self::releases_toggle_session_on_entry(&target)
        {
            let _ = self.release_drag_auto_release(backend)?;
        }
        if Self::releases_toggle_session_on_entry(&target) {
            self.release_toggle_session_for_safe_mode(backend)?;
        }
        let previous = self.registry.active.clone();
        if target == ModeId::window() {
            // Suspend motion as well as the visible mode. Keep physical key
            // dispositions so subsequent releases are still correctly paired.
            let gestures = std::mem::take(&mut self.input.active_gestures);
            for (key, gesture) in gestures {
                self.dispatch_to(
                    &gesture.owner,
                    ModeEvent::Binding {
                        binding: gesture.binding,
                        state: crate::api::KeyState::Up,
                        key,
                    },
                    backend,
                )?;
            }
        }
        self.dispatch(ModeEvent::Suspended, backend)?;
        self.registry.modal_stack.push(previous.clone());
        self.set_active(target);
        self.dispatch(ModeEvent::Pushed { previous }, backend)
    }

    pub(super) fn pop_mode(&mut self, backend: &mut dyn Backend) -> Result<(), String> {
        let Some(previous) = self.registry.modal_stack.last().cloned() else {
            return Ok(());
        };
        if self.registry.active == ModeId::normal()
            && previous != ModeId::normal()
            && !Self::releases_toggle_session_on_entry(&previous)
        {
            let _ = self.release_drag_auto_release(backend)?;
        }
        if Self::releases_toggle_session_on_entry(&previous) {
            self.release_toggle_session_for_safe_mode(backend)?;
        }
        self.registry.modal_stack.pop();
        let current = self.registry.active.clone();
        self.dispatch(ModeEvent::Deactivated, backend)?;
        self.cancel_scans_for_owner(&current, backend)?;
        self.scheduler.cancel_timers_for_owner(&current);
        self.set_active(previous);
        self.dispatch(ModeEvent::Resumed, backend)
    }

    pub(super) fn restart_active(&mut self, backend: &mut dyn Backend) -> Result<(), String> {
        let active = self.registry.active.clone();
        self.scheduler.cancel_owner(&active);
        self.dispatch(ModeEvent::Restarted, backend)
    }
}
