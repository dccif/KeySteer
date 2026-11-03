//! Read-only help derived from the same routing decisions as real input.
use super::*;
use crate::api::geometry::Rect;
use crate::api::overlay::{OverlayItems, OverlayLabel, OverlayShape};

/// One visible-session cache. Source and decorated labels share the existing
/// scene storage; there is no history or map that can grow across inputs.
pub(super) struct KeyHelpCache {
    source_labels: OverlayItems<OverlayLabel>,
    source_shapes: OverlayItems<OverlayShape>,
    shapes: OverlayItems<OverlayShape>,
    annotations: Option<Arc<crate::api::overlay::WindowAnnotations>>,
    source_annotations: Option<Arc<crate::api::overlay::WindowAnnotations>>,
    labels: OverlayItems<OverlayLabel>,
    source_clip: Option<Rect>,
    clip: Option<Rect>,
    bounds: Rect,
    work_area: Rect,
    scale: f64,
    active: ModeId,
    display: ModeId,
    palette: Palette,
    anchor: Option<Rect>,
    detail: Option<String>,
    previews: Vec<(String, String, Rect, bool)>,
    ruler: Option<crate::api::window_layout::QuickRuler>,
}

pub(super) struct WindowKeyHelpPlan {
    pub(super) presentations: [Arc<crate::presentation::key_help::PreparedWindowHelp>; 2],
    pub(super) entries: Vec<String>,
    pub(super) return_target: Option<String>,
}

impl KeyHelpCache {
    pub(super) fn matches_screen(&self, screen: &Screen) -> bool {
        self.bounds == screen.bounds
            && self.work_area == screen.work_area
            && self.scale == screen.scale
    }
}

impl Engine {
    pub(super) fn window_help_visible(&self) -> bool {
        self.display_mode().is_window()
            && (self
                .registry
                .keyboard_prompt_mode(&self.registry.active)
                .is_some_and(|mode| mode.keyboard_prompt_active())
                || self
                    .overlay
                    .window_help_override
                    .unwrap_or(self.settings.key_help.window_key_help))
    }

    fn help_visible(&self) -> bool {
        if self.display_mode().is_window() {
            self.window_help_visible()
        } else {
            self.overlay.key_help_visible
        }
    }

    pub(super) fn help_screen(&self) -> Option<&Screen> {
        let anchor = self
            .registry
            .get(&self.display_mode())
            .and_then(|m| m.help_anchor());
        anchor
            .and_then(|rect| Screen::containing(&self.screens, &rect.center()))
            .or_else(|| Screen::containing(&self.screens, &self.cursor))
    }
    fn key_help_binding(
        &self,
        chord: &KeyChord,
        stable: bool,
        pressed: &mut Vec<Key>,
    ) -> Option<ResolvedBinding> {
        // Reuse one scratch buffer per help build, preserving physical key sides.
        pressed.clear();
        if !stable {
            pressed.extend(self.input.pressed.iter().cloned());
        }
        for key in chord.keys() {
            if !pressed
                .iter()
                .any(|physical| Self::keys_match(key, physical))
            {
                pressed.push(key.clone());
            }
        }
        if stable {
            self.lookup_for_help(chord.activation_key(), pressed)
        } else {
            self.lookup_for_pressed(chord.activation_key(), pressed)
        }
    }

    fn configured_key_help_entries(&self) -> Vec<String> {
        let mut entries = Vec::new();
        let mut pressed = Vec::new();
        for (id, table) in self.registry.tables() {
            for entry in table.iter_entries() {
                if matches!(entry.binding.as_ref(), Binding::Disabled) {
                    continue;
                }
                let Some(resolved) = self.key_help_binding(&entry.chord, false, &mut pressed)
                else {
                    continue;
                };
                if resolved.owner != *id || resolved.binding != entry.binding {
                    continue;
                }
                // While a non-modifier chord prefix is held, show its continuations.
                if !self.input.pending_chords.is_empty()
                    && !self
                        .input
                        .pressed
                        .iter()
                        .filter(|key| !key.is_modifier())
                        .all(|key| {
                            entry
                                .chord
                                .keys()
                                .iter()
                                .any(|part| Self::keys_match(part, key))
                        })
                {
                    continue;
                }
                let contextual = if id == &self.registry.active {
                    self.registry
                        .get(id)
                        .and_then(|mode| mode.key_help_binding_label(&entry.chord, &entry.binding))
                } else {
                    None
                };
                let label = contextual.map_or_else(|| entry.binding.canonical(), str::to_owned);
                entries.push(format!("{}  ·  {}", entry.chord.canonical(), label));
            }
        }
        entries
    }

    fn extra_key_help_entries(&self, stable: bool, mut entries: Vec<String>) -> Vec<String> {
        let mut pressed = Vec::new();
        if self.display_mode() == self.registry.active
            && let Some(mode) = self.registry.get(&self.registry.active)
        {
            if self.window_library_help()
                && let Some(table) = self.registry.table(&self.registry.active)
            {
                for entry in table.iter_entries() {
                    if let Some(
                        action @ (crate::api::window::WindowAction::NextPage
                        | crate::api::window::WindowAction::PreviousPage),
                    ) = entry.binding.window_action()
                        && mode.window_action_available(&action)
                    {
                        entries.push(format!(
                            "{}  ·  {}",
                            entry.chord.canonical(),
                            entry.binding.canonical()
                        ));
                    }
                }
            }
            for (key, action) in mode.available_keys() {
                if stable && action == "Window number" {
                    continue;
                }
                let Ok(chord) = KeyChord::parse(&key) else {
                    continue;
                };
                if self
                    .key_help_binding(&chord, stable, &mut pressed)
                    .is_none()
                {
                    entries.push(format!("{key}  ·  {action}"));
                }
            }
        }
        entries.sort();
        entries.dedup();
        entries
    }

    fn window_library_help(&self) -> bool {
        matches!(self.registry.active.as_str(), "window_restore")
    }

    pub(super) fn key_help_entries(&self) -> Vec<String> {
        if let Some((owner, prompt)) = &self.scheduler.text_prompt
            && owner == &ModeId::text_select()
            && prompt.live_style.is_some()
            && !self.temporary_mode_is_active(owner)
            && let Some(mode) = self.registry.keyboard_prompt_mode(owner)
        {
            let mut entries = std::collections::BTreeSet::new();
            entries.extend(
                mode.available_keys()
                    .into_iter()
                    .map(|(key, action)| format!("{key}  ·  {action}")),
            );
            if let Some(table) = self.registry.table(owner) {
                for entry in table.iter_entries() {
                    if let Some(binding) = mode
                        .keyboard_prompt_binding(entry.chord.activation_key(), entry.chord.keys())
                        && !matches!(binding.as_ref(), Binding::Disabled)
                    {
                        entries.insert(format!(
                            "{}  ·  {}",
                            entry.chord.canonical(),
                            binding.canonical()
                        ));
                    }
                }
            }
            for (chord, action) in prompt.edit_keys.iter() {
                if mode
                    .keyboard_prompt_binding(chord.activation_key(), chord.keys())
                    .is_none()
                {
                    entries.insert(format!("{}  ·  {action:?}", chord.canonical()));
                }
            }
            return entries.into_iter().collect();
        }
        let entries = if self.registry.active.is_window() {
            self.registry
                .window_help_plan(&self.registry.active)
                .map_or_else(Vec::new, |plan| plan.entries.to_vec())
        } else {
            self.configured_key_help_entries()
        };
        if !self.registry.active.is_window() || self.window_library_help() {
            self.extra_key_help_entries(self.registry.active.is_window(), entries)
        } else {
            entries
        }
    }

    pub(super) fn decorate_key_help(&mut self, scene: &mut OverlayScene) {
        if !self.help_visible()
            || self.registry.active == ModeId::idle()
            || self.window_presets.pending.is_some()
            || self
                .scheduler
                .text_prompt
                .as_ref()
                .is_some_and(|(owner, prompt)| {
                    prompt.live_style.is_none()
                        || (owner != &ModeId::text_select()
                            && !self.temporary_mode_is_active(owner))
                })
        {
            self.overlay.key_help_cache = None;
            return;
        }
        let Some(screen) = self.help_screen() else {
            self.overlay.key_help_cache = None;
            return;
        };
        let display = self.display_mode();
        let mode = self.registry.get(&display);
        let anchor = mode.and_then(|m| m.help_anchor());
        let detail = mode.and_then(|m| m.indicator_detail());
        let ruler = mode.and_then(|m| m.quick_ruler());
        let previews = mode.map_or_else(Vec::new, |m| m.help_previews());
        if let Some(cache) = &self.overlay.key_help_cache
            && cache.active == self.registry.active
            && cache.display == display
            && cache.palette == self.palette
            && cache.anchor == anchor
            && cache.detail == detail
            && cache.previews == previews
            && cache.ruler == ruler
            && cache.matches_screen(screen)
            && cache.source_clip == scene.clip
            && cache.source_shapes == scene.shapes
            && cache.source_annotations == scene.window_annotations
            && (cache.source_labels.shares_storage_with(&scene.labels)
                || (cache.source_labels.is_empty() && scene.labels.is_empty()))
        {
            scene.labels = cache.labels.clone();
            scene.shapes = cache.shapes.clone();
            scene.window_annotations = cache.annotations.clone();
            scene.clip = cache.clip;
            return;
        }
        let (bounds, work_area, scale) = (screen.bounds, screen.work_area, screen.scale);
        let source_labels = scene.labels.clone();
        let source_shapes = scene.shapes.clone();
        let source_annotations = scene.window_annotations.clone();
        let source_clip = scene.clip;
        let window_help = self.window_help_visible();
        let plan = self.registry.window_help_plan(&self.registry.active);
        let dynamic_entries;
        let (entries, return_target, extra_entries) = if let Some(plan) = plan {
            (
                plan.entries.as_slice(),
                plan.return_target.as_deref(),
                if self.window_library_help() {
                    self.extra_key_help_entries(true, Vec::new())
                } else {
                    Vec::new()
                },
            )
        } else {
            dynamic_entries = self.key_help_entries();
            let return_target = self.key_help_return_target_in(&display, &dynamic_entries);
            (dynamic_entries.as_slice(), return_target, Vec::new())
        };
        crate::presentation::key_help::compose(
            scene,
            crate::presentation::key_help::KeyHelpView {
                prepared: plan.filter(|_| !self.window_library_help()).map(|plan| {
                    let resizing = display == ModeId::window()
                        && mode.is_some_and(|mode| !mode.accepts_window_targeting());
                    plan.presentations[usize::from(resizing)].as_ref()
                }),
                screen,
                ui: &self.settings.key_help,
                palette: &self.palette,
                entries,
                extra_entries,
                return_target,
                window_help,
                display_name: mode.map_or_else(|| display.to_string(), |mode| mode.display_name()),
                ruler: ruler.as_ref(),
                previews: &previews,
                detail: if window_help { detail.as_deref() } else { None },
                anchor,
                indicator_style: self
                    .build_indicator(&display)
                    .map(|(indicator, _)| indicator.style),
            },
        );
        self.overlay.key_help_cache = Some(Box::new(KeyHelpCache {
            source_labels,
            source_shapes,
            shapes: scene.shapes.clone(),
            source_annotations,
            annotations: scene.window_annotations.clone(),
            labels: scene.labels.clone(),
            source_clip,
            clip: scene.clip,
            bounds,
            work_area,
            scale,
            active: self.registry.active.clone(),
            display,
            palette: self.palette.clone(),
            anchor,
            detail,
            previews,
            ruler,
        }));
    }

    pub(super) fn key_help_return_target_in(
        &self,
        display_mode: &ModeId,
        entries: &[String],
    ) -> Option<&str> {
        self.registry.table(display_mode).and_then(|table| {
            table
                .iter_entries()
                .filter_map(|entry| {
                    let Binding::Mode(target) = entry.binding.as_ref() else {
                        return None;
                    };
                    if target == display_mode
                        || (target.is_window() && target != &ModeId::window())
                        || (*display_mode == ModeId::window() && target.is_grid())
                    {
                        return None;
                    }
                    let effective = format!(
                        "{}  \u{b7}  {}",
                        entry.chord.canonical(),
                        entry.binding.canonical()
                    );
                    entries.contains(&effective).then(|| target.as_str())
                })
                .min_by_key(|target| (*target == "idle", *target))
        })
    }
}
