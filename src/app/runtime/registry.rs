//! Runtime mode registry, route compilation, dispatch, and lifecycle.

use std::collections::{BTreeMap, HashMap};

use crate::api::backend::Backend;
use crate::api::binding::Binding;
use crate::api::command::{FocusedApp, HostContext, Mode, ModeEvent};
use crate::api::geometry::Rect;
use crate::api::input::{Key, KeyChord, ModeId};

use super::input_router::CompiledKeymap;
use super::plan::{Bindings, ModeRoute};
use super::{Engine, RuntimeError};

struct ModeSlot {
    id: ModeId,
    mode: Box<dyn Mode>,
    pointer_events: bool,
    keyboard_prompt_supported: bool,
    table: Option<CompiledKeymap>,
    window_help: Option<Box<super::key_help::WindowKeyHelpPlan>>,
    /// Owned compiled triggers; binding-table rebuilds leave them untouched.
    temporary_chords: Option<Vec<KeyChord>>,
}

pub(super) struct ModeRegistry {
    slots: Vec<ModeSlot>,
    indices: BTreeMap<ModeId, usize>,
    pub(super) routes: BTreeMap<ModeId, ModeRoute>,
    pub(super) active: ModeId,
    pub(super) active_slot: Option<usize>,
    pub(super) binding_profile_key: Vec<Bindings>,
    pub(super) prefixes_require_modifier: bool,
    pub(super) unbound_prefixes: BTreeMap<crate::api::Key, super::input_router::CompiledPrefix>,
    pub(super) character_keys: HashMap<char, crate::api::Key>,
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
            unbound_prefixes: BTreeMap::new(),
            character_keys: HashMap::new(),
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
        let mut registry = Self {
            slots: Vec::with_capacity(routes.len()),
            ..Self::default()
        };
        registry.replace_routes(routes);
        registry
    }

    pub(super) fn replace_routes(&mut self, mut routes: BTreeMap<ModeId, ModeRoute>) {
        fn expand(
            owner: &ModeId,
            routes: &BTreeMap<ModeId, ModeRoute>,
            stop: Option<&ModeId>,
            order: &mut Vec<ModeId>,
        ) {
            if order.contains(owner) {
                return;
            }
            order.push(owner.clone());
            if stop != Some(owner)
                && let Some(route) = routes.get(owner)
            {
                for source in &route.inherits {
                    expand(source, routes, stop, order);
                }
            }
        }
        // Read the original graph before replacing it. Store only source IDs;
        // tables and bindings retain their existing owners, including modes
        // registered later. Cycle protection runs here, never on a key event.
        let expanded: Vec<_> = routes
            .iter()
            .map(|(id, route)| {
                let mut inherits = Vec::new();
                expand(id, &routes, None, &mut inherits);
                inherits.remove(0);
                let claimed = route
                    .temporary_mode
                    .as_ref()
                    .filter(|owner| *owner != id)
                    .and_then(|owner| {
                        let mut order = Vec::new();
                        expand(owner, &routes, Some(id), &mut order);
                        order.remove(0);
                        order.contains(id).then(|| order.into_boxed_slice())
                    });
                (inherits, claimed)
            })
            .collect();
        for (route, (inherits, claimed)) in routes.values_mut().zip(expanded) {
            route.inherits = inherits.into_boxed_slice();
            route.temporary_inherits_when_claimed = claimed;
        }
        self.routes = routes;
    }

    pub(super) fn insert(
        &mut self,
        id: ModeId,
        mode: Box<dyn Mode>,
        pointer_events: bool,
    ) -> usize {
        let keyboard_prompt_supported = mode.keyboard_prompt_supported();
        if let Some(&index) = self.indices.get(&id) {
            self.slots[index].mode = mode;
            self.slots[index].pointer_events = pointer_events;
            self.slots[index].keyboard_prompt_supported = keyboard_prompt_supported;
            return index;
        }
        let index = self.slots.len();
        self.slots.push(ModeSlot {
            id: id.clone(),
            mode,
            pointer_events,
            keyboard_prompt_supported,
            table: None,
            window_help: None,
            temporary_chords: None,
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

    pub(super) fn keyboard_prompt_mode(&self, id: &ModeId) -> Option<&dyn Mode> {
        let index = self.index_of(id)?;
        let slot = self.slots.get(index)?;
        slot.keyboard_prompt_supported.then_some(slot.mode.as_ref())
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

    pub(super) fn temporary_chords(&self, id: &ModeId) -> Option<&[KeyChord]> {
        let index = self.index_of(id)?;
        self.slots.get(index)?.temporary_chords.as_deref()
    }

    pub(super) fn window_help_plan(
        &self,
        id: &ModeId,
    ) -> Option<&super::key_help::WindowKeyHelpPlan> {
        self.slots.get(self.index_of(id)?)?.window_help.as_deref()
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
    fn compile_window_help_plans(&mut self) {
        let temporary = self.compile_window_targeting_temporary_help();
        for index in 0..self.registry.slots.len() {
            let id = &self.registry.slots[index].id;
            if !id.is_window() {
                continue;
            }
            let mut entries = std::collections::BTreeSet::new();
            for (owner, table) in self.registry.tables() {
                for entry in table.iter_entries() {
                    if matches!(entry.binding.as_ref(), Binding::Disabled) {
                        continue;
                    }
                    // Fixed help is the Window layer, not a preview of
                    // every binding reachable through its temporary mode.
                    if self
                        .lookup_layer_reference::<true>(
                            id,
                            id,
                            entry.chord.activation_key(),
                            entry.chord.keys(),
                            Some(&entry.chord),
                        )
                        .is_some_and(|(source, matched)| {
                            source == owner
                                && matched.binding == entry.binding
                                && self.registry.get(id).is_none_or(|mode| {
                                    match matched.binding.window_action() {
                                        Some(
                                            crate::api::window::WindowAction::NextPage
                                            | crate::api::window::WindowAction::PreviousPage,
                                        ) => false,
                                        Some(action) => mode.window_action_supported(&action),
                                        _ => true,
                                    }
                                })
                        })
                    {
                        entries.insert(format!(
                            "{}  ·  {}",
                            entry.chord.canonical(),
                            entry.binding.canonical()
                        ));
                    }
                }
            }
            if *id == ModeId::window() {
                entries.extend(temporary.iter().cloned());
            }
            if let Some(mode) = self.registry.get(id) {
                for (key, action) in mode.fixed_key_help() {
                    if let Ok(chord) = KeyChord::parse(&key)
                        && self
                            .lookup_for_help_in(id, chord.activation_key(), chord.keys())
                            .is_none()
                    {
                        entries.insert(format!("{key}  ·  {action}"));
                    }
                }
            }
            let entries: Vec<_> = entries.into_iter().collect();
            let return_target = self
                .key_help_return_target_in(id, &entries)
                .map(str::to_owned);
            let move_help =
                std::sync::Arc::new(crate::presentation::key_help::compile_window_help(
                    &entries,
                    id.as_str(),
                    return_target.as_deref(),
                    false,
                ));
            let resize_help = if *id == ModeId::window() {
                std::sync::Arc::new(crate::presentation::key_help::compile_window_help(
                    &entries,
                    id.as_str(),
                    return_target.as_deref(),
                    true,
                ))
            } else {
                move_help.clone()
            };
            self.registry.slots[index].window_help =
                Some(Box::new(super::key_help::WindowKeyHelpPlan {
                    presentations: [move_help, resize_help],
                    entries,
                    return_target,
                }));
        }
    }
    /// Show temporary-layer entrances even before its activation keys are held.
    /// Validate each combined chord through the actual help resolver so local
    /// overrides, disabled bindings, aliases and inherited routes still win.
    fn compile_window_targeting_temporary_help(&self) -> Vec<String> {
        if !self.registry.contains_key(&ModeId::window()) {
            return Vec::new();
        }
        let mut entries = std::collections::BTreeSet::new();
        let Some(triggers) = self.registry.temporary_chords(&ModeId::window()) else {
            return Vec::new();
        };
        for (owner, table) in self.registry.tables() {
            for entry in table.iter_entries() {
                if !matches!(entry.binding.as_ref(), Binding::Mode(id)
                    if id.is_grid())
                {
                    continue;
                }
                for trigger in triggers {
                    let mut pressed = trigger.keys().to_vec();
                    for key in entry.chord.keys() {
                        if !pressed.iter().any(|part| Self::keys_match(key, part)) {
                            pressed.push(key.clone());
                        }
                    }
                    let Some(resolved) = self.lookup_for_help_in(
                        &ModeId::window(),
                        entry.chord.activation_key(),
                        &pressed,
                    ) else {
                        continue;
                    };
                    if resolved.owner != *owner || resolved.binding != entry.binding {
                        continue;
                    }
                    let text = pressed
                        .iter()
                        .map(Key::as_str)
                        .collect::<Vec<_>>()
                        .join("+");
                    if let Ok(chord) = KeyChord::parse(&text) {
                        entries.insert(format!(
                            "{}  ·  {}",
                            chord.canonical(),
                            entry.binding.canonical()
                        ));
                    }
                }
            }
        }
        entries.into_iter().collect()
    }

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
            for chord in &plugin.manifest().default_chords {
                self.registry
                    .plugin_bindings
                    .push((chord.clone(), Binding::Mode(id.clone())));
            }
            self.registry
                .plugin_bindings
                .extend(plugin.manifest().default_bindings.iter().cloned());
        }
        for verb in &plugin.manifest().verbs {
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
        if active.is_window() && !self.registry.active.is_window() {
            self.overlay.window_help_override = None;
        }
        if active != self.registry.active {
            self.window_presets
                .store
                .record_mode_entry(active.as_str(), self.settings.usage_save_after_entries);
            // Activation keys belong to the outgoing input context. A fresh
            // physical press arms the destination's temporary layer.
            self.input.temporary_entry_keys.clear();
            self.input
                .temporary_entry_keys
                .extend(self.input.pressed.iter().cloned());
        }
        self.overlay.key_help_cache = None;
        if active == ModeId::idle() || active == ModeId::text_input() {
            self.overlay.key_help_visible = false;
        } else if !active.is_window()
            && (self.registry.active == ModeId::idle() || self.registry.active.is_window())
        {
            self.overlay.key_help_visible = self.settings.key_help.mouse_key_help;
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

    pub(super) fn binding_mode_ids(&self) -> impl Iterator<Item = ModeId> + '_ {
        // Keep idle in the binding profile even before its instance registers.
        self.registry
            .keys()
            .cloned()
            .chain((!self.registry.contains_key(&ModeId::idle())).then(ModeId::idle))
    }

    pub(super) fn binding_profile_key_for(&self, app: Option<&FocusedApp>) -> Vec<Bindings> {
        if app.is_none()
            || self
                .registry
                .routes
                .values()
                .all(|route| route.app_overrides.is_empty())
        {
            return Vec::new();
        }
        let profile: Vec<Bindings> = self
            .binding_mode_ids()
            .map(|id| self.resolved_app_overrides(&id, app))
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
        let profile = self.binding_profile_key_for(self.focused_app.as_ref());
        self.rebuild_tables_for_profile(profile);
    }

    pub(super) fn rebuild_tables_for_profile(&mut self, binding_profile_key: Vec<Bindings>) {
        self.overlay.key_help_cache = None;
        // The registered ids are the prefix of binding_mode_ids in the same order.
        for (index, (id, &slot_index)) in self.registry.indices.iter().enumerate() {
            let slot = &mut self.registry.slots[slot_index];
            let Some(route) = self.registry.routes.get_mut(id) else {
                slot.table = None;
                slot.temporary_chords = None;
                continue;
            };
            // Take new triggers once; disabled routes clear the previous plan.
            if route.temporary_mode.is_none() || route.temporary_keys.is_some() {
                slot.temporary_chords = route.temporary_keys.take();
            }
            let aliases = &self.settings.resolved_key_aliases;
            slot.table = Some(
                match binding_profile_key
                    .get(index)
                    .filter(|patch| !patch.is_empty())
                {
                    Some(overrides) => {
                        let merged: BTreeMap<_, _> =
                            route.bindings.iter().chain(overrides).collect();
                        CompiledKeymap::compile(merged, aliases)
                    }
                    None => CompiledKeymap::compile(&route.bindings, aliases),
                },
            );
        }

        // A plugin's suggested chord applies in `normal`, which is where the
        // user works, and only if that chord is still free.
        self.registry.merge_plugin_bindings_into_normal();

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
        self.registry.unbound_prefixes =
            super::input_router::compile_unbound_prefixes(&continuations);
        for slot in &mut self.registry.slots {
            slot.table
                .get_or_insert_default()
                .compile_prefixes(&continuations);
        }
        self.input.pending_chords.clear();

        self.registry.binding_profile_key = binding_profile_key;
        self.compile_window_help_plans();
        #[cfg(test)]
        {
            self.registry.table_rebuild_count += 1;
        }
    }

    pub(super) fn sync_character_bindings(&self, backend: &mut dyn Backend) {
        let mut keys: Vec<_> = self.registry.character_keys.values().cloned().collect();
        if self.settings.quick_switch.enabled
            && self.settings.quick_switch.key.as_char().is_some()
            && !keys.contains(&self.settings.quick_switch.key)
        {
            keys.push(self.settings.quick_switch.key.clone());
        }
        if self.settings.quick_switch.enabled {
            for key in &self.settings.quick_switch.alphabet {
                if !keys.contains(key) {
                    keys.push(key.clone());
                }
            }
        }
        backend.set_character_bindings(&keys);
    }

    fn resolved_app_overrides(&self, mode_id: &ModeId, app: Option<&FocusedApp>) -> Bindings {
        let Some(app) = app else {
            return Bindings::new();
        };
        let mut resolved = Bindings::new();
        if let Some(route) = self.registry.routes.get(mode_id) {
            for entry in &route.app_overrides {
                if app.matches_pattern(&entry.pattern) {
                    for (key, binding) in &entry.bindings {
                        resolved.insert(key.clone(), binding.clone());
                    }
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
                        route.inherits,
                        route.temporary_mode,
                        self.registry.temporary_chords(mode).unwrap_or_default()
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

    /// Deliver an event, including pointer controls borrowed by another mode.
    /// Payload consumers and borrowed handlers share this dispatch path.
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
        // Frame and pointer notifications contain no owned buffers to reuse.
        // Keep their borrowed call while sharing lookup, context and command execution.
        let commands = if matches!(event, ModeEvent::Frame { .. } | ModeEvent::PointerMoved(_)) {
            mode.handle(&event, &context)
        } else {
            mode.handle_owned(event, &context)
        };
        crate::support::perf_probe::mark("mode_handled");
        crate::support::perf_probe::mark("commands_ready");
        self.execute_for(owner, commands, backend)
    }

    pub(super) fn push_mode(
        &mut self,
        target: ModeId,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        self.push_mode_in_region(target, None, backend)
    }

    pub(super) fn push_mode_in_region(
        &mut self,
        target: ModeId,
        bounds: Option<Rect>,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        if target == self.registry.active || !self.registry.contains_key(&target) {
            return Ok(());
        }
        if bounds.is_some_and(|bounds| {
            bounds.is_empty()
                || !bounds.x.is_finite()
                || !bounds.y.is_finite()
                || !bounds.width.is_finite()
                || !bounds.height.is_finite()
        }) {
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
        self.dispatch(ModeEvent::Suspended, backend)?;
        self.registry.modal_stack.push(previous.clone());
        self.set_active(target);
        self.dispatch(
            match bounds {
                Some(bounds) => ModeEvent::PushedInRegion { previous, bounds },
                None => ModeEvent::Pushed { previous },
            },
            backend,
        )
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
        let window_help = self.overlay.window_help_override;
        self.dispatch(ModeEvent::Deactivated, backend)?;
        self.cancel_scans_for_owner(&current, backend)?;
        self.scheduler.cancel_timers_for_owner(&current);
        self.set_active(previous);
        if self.registry.active == ModeId::window() {
            self.overlay.window_help_override = window_help;
        }
        self.dispatch(ModeEvent::Resumed, backend)
    }

    pub(super) fn restart_active(&mut self, backend: &mut dyn Backend) -> Result<(), String> {
        let active = self.registry.active.clone();
        self.scheduler.cancel_owner(&active);
        self.dispatch(ModeEvent::Restarted, backend)
    }
}
