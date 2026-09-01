//! Runtime responsibility extracted from the Engine composition root.

use super::*;

impl Engine {
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

        self.ui_hint_overlap_chord = ui_hint_overlap_chord;
        self.registry.binding_profile_key = binding_profile_key;
        #[cfg(test)]
        {
            self.registry.table_rebuild_count += 1;
        }
    }

    /// Apply the focused application's overrides to one mode's table.
    pub(super) fn merge_overrides(
        &self,
        mode_id: &ModeId,
        base: &Bindings,
    ) -> Vec<(String, Binding)> {
        let mut merged: BTreeMap<String, Binding> = base.clone();
        for (chord, binding) in self.resolved_app_overrides(mode_id, self.focused_app.as_ref()) {
            merged.insert(chord, binding);
        }
        merged.into_iter().collect()
    }

    pub(super) fn resolved_app_overrides(
        &self,
        mode_id: &ModeId,
        app: Option<&FocusedApp>,
    ) -> Bindings {
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
            .any(|e| e.eq_ignore_ascii_case(&app.bundle_id))
    }

    pub(super) fn is_excluded_app(&self) -> bool {
        self.focused_app_excluded
    }

    pub(super) fn context(&self) -> HostContext<'_> {
        HostContext {
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
}
