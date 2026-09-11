//! Read-only help derived from the same routing decisions as real input.
use super::*;
use crate::api::geometry::Rect;
use crate::api::overlay::{OverlayItems, OverlayLabel};

/// One visible-session cache. Source and decorated labels share the existing
/// scene storage; there is no history or map that can grow across inputs.
pub(super) struct KeyHelpCache {
    source_labels: OverlayItems<OverlayLabel>,
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
}

pub(super) struct WindowKeyHelpPlan {
    pub(super) entries: Arc<[String]>,
    return_target: Option<String>,
}

impl KeyHelpCache {
    pub(super) fn matches_screen(&self, screen: &Screen) -> bool {
        self.bounds == screen.bounds
            && self.work_area == screen.work_area
            && self.scale == screen.scale
    }
}

impl Engine {
    fn window_help_visible(&self) -> bool {
        self.display_mode().is_window()
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
    fn configured_key_help_entries(&self, stable: bool) -> Vec<String> {
        let mut entries = std::collections::BTreeSet::new();
        let contextual_keys: Vec<Key> = if stable {
            Vec::new()
        } else {
            self.input.pressed.iter().cloned().collect()
        };
        for id in self.registry.keys() {
            let Some(table) = self.registry.table(id) else {
                continue;
            };
            for entry in table.iter_entries() {
                if matches!(entry.binding.as_ref(), Binding::Disabled) {
                    continue;
                }
                let mut pressed = contextual_keys.clone();
                for key in entry.chord.keys() {
                    if !pressed
                        .iter()
                        .any(|physical| Self::keys_match(key, physical))
                    {
                        pressed.push(key.clone());
                    }
                }
                let Some(resolved) = (if stable {
                    self.lookup_for_help(entry.chord.activation_key(), &pressed)
                } else {
                    self.lookup_for_pressed(entry.chord.activation_key(), &pressed)
                }) else {
                    continue;
                };
                if resolved.owner != *id || resolved.binding != entry.binding {
                    continue;
                }
                // While a non-modifier chord prefix is held, show its continuations.
                if !stable
                    && !self.input.pending_chords.is_empty()
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
                entries.insert(format!(
                    "{}  ·  {}",
                    entry.chord.canonical(),
                    entry.binding.canonical()
                ));
            }
        }
        entries.into_iter().collect()
    }

    fn extra_key_help_entries(&self, stable: bool) -> Vec<String> {
        let mut entries = std::collections::BTreeSet::new();
        let contextual_keys: Vec<Key> = if stable {
            Vec::new()
        } else {
            self.input.pressed.iter().cloned().collect()
        };
        if self.display_mode() == self.registry.active
            && let Some(mode) = self.registry.get(&self.registry.active)
        {
            for (key, action) in mode.available_keys() {
                if stable && action == "Window number" {
                    continue;
                }
                let Ok(chord) = KeyChord::parse(&key) else {
                    continue;
                };
                let mut pressed = contextual_keys.clone();
                for key in chord.keys() {
                    if !pressed
                        .iter()
                        .any(|physical| Self::keys_match(key, physical))
                    {
                        pressed.push(key.clone());
                    }
                }
                if (if stable {
                    self.lookup_for_help(chord.activation_key(), &pressed)
                } else {
                    self.lookup_for_pressed(chord.activation_key(), &pressed)
                })
                .is_none()
                {
                    entries.insert(format!("{key}  ·  {action}"));
                }
            }
        }
        entries.into_iter().collect()
    }

    fn window_library_help(&self) -> bool {
        matches!(self.registry.active.as_str(), "window_restore")
    }

    fn new_window_help_plan(&self) -> WindowKeyHelpPlan {
        let mut entries = self.configured_key_help_entries(true);
        if !self.window_library_help() {
            entries.extend(self.extra_key_help_entries(true));
        }
        let return_target = self.key_help_return_target(&entries);
        WindowKeyHelpPlan {
            entries: entries.into(),
            return_target,
        }
    }

    pub(super) fn key_help_entries(&self) -> Vec<String> {
        let mut entries = if self.registry.active.is_window() {
            self.overlay.window_help_plan.as_ref().map_or_else(
                || self.new_window_help_plan().entries.to_vec(),
                |plan| plan.entries.to_vec(),
            )
        } else {
            self.configured_key_help_entries(false)
        };
        if !self.registry.active.is_window() || self.window_library_help() {
            entries.extend(self.extra_key_help_entries(self.registry.active.is_window()));
        }
        entries.sort();
        entries.dedup();
        entries
    }

    pub(super) fn decorate_key_help(&mut self, scene: &mut OverlayScene) {
        if (!self.overlay.key_help_visible && !self.window_help_visible())
            || self.registry.active == ModeId::idle()
            || self.window_presets.pending.is_some()
        {
            self.overlay.key_help_cache = None;
            return;
        }
        if self.registry.active.is_window() && self.overlay.window_help_plan.is_none() {
            self.overlay.window_help_plan = Some(self.new_window_help_plan());
        }
        let Some(screen) = self.help_screen() else {
            self.overlay.key_help_cache = None;
            return;
        };
        let display = self.display_mode();
        let mode = self.registry.get(&display);
        let anchor = mode.and_then(|m| m.help_anchor());
        let detail = mode.and_then(|m| m.indicator_detail());
        let previews = mode.map_or_else(Vec::new, |m| m.help_previews());
        if let Some(cache) = &self.overlay.key_help_cache
            && cache.active == self.registry.active
            && cache.display == display
            && cache.palette == self.palette
            && cache.anchor == anchor
            && cache.detail == detail
            && cache.previews == previews
            && cache.matches_screen(screen)
            && cache.source_clip == scene.clip
            && (cache.source_labels.shares_storage_with(&scene.labels)
                || (cache.source_labels.is_empty() && scene.labels.is_empty()))
        {
            scene.labels = cache.labels.clone();
            scene.clip = cache.clip;
            return;
        }
        let (bounds, work_area, scale) = (screen.bounds, screen.work_area, screen.scale);
        let source_labels = scene.labels.clone();
        let source_clip = scene.clip;
        self.build_key_help(scene);
        self.overlay.key_help_cache = Some(Box::new(KeyHelpCache {
            source_labels,
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
        }));
    }

    fn key_help_return_target(&self, entries: &[String]) -> Option<String> {
        let display_mode = self.display_mode();
        self.registry.table(&display_mode).and_then(|table| {
            table
                .iter_entries()
                .filter_map(|entry| {
                    let Binding::Mode(target) = entry.binding.as_ref() else {
                        return None;
                    };
                    if target == &display_mode
                        || (target.is_window() && target != &ModeId::window())
                    {
                        return None;
                    }
                    let effective = format!(
                        "{}  \u{b7}  {}",
                        entry.chord.canonical(),
                        entry.binding.canonical()
                    );
                    entries.contains(&effective).then(|| target.to_string())
                })
                .min_by_key(|target| (target == "idle", target.clone()))
        })
    }

    fn build_key_help(&self, scene: &mut OverlayScene) {
        let window_help = self.window_help_visible();
        if (!self.overlay.key_help_visible && !window_help)
            || self.registry.active == ModeId::idle()
        {
            return;
        }
        let Some(screen) = self.help_screen() else {
            return;
        };
        let display_mode = self.display_mode();
        let mode = self.registry.get(&display_mode);
        let (entries, return_target, extra_entries) = if let Some(plan) = self
            .overlay
            .window_help_plan
            .as_ref()
            .filter(|_| self.registry.active.is_window())
        {
            (
                Arc::clone(&plan.entries),
                plan.return_target.clone(),
                if self.window_library_help() {
                    self.extra_key_help_entries(true)
                } else {
                    Vec::new()
                },
            )
        } else {
            let entries = self.key_help_entries();
            let return_target = self.key_help_return_target(&entries);
            (entries.into(), return_target, Vec::new())
        };
        crate::presentation::key_help::compose(
            scene,
            crate::presentation::key_help::KeyHelpView {
                screen,
                ui: &self.settings.key_help,
                palette: &self.palette,
                entries,
                extra_entries,
                return_target,
                window_help,
                display_name: mode
                    .map_or_else(|| display_mode.to_string(), |mode| mode.display_name()),
                previews: mode.map_or_else(Vec::new, |mode| mode.help_previews()),
                detail: if window_help {
                    mode.and_then(|mode| mode.indicator_detail())
                } else {
                    None
                },
                anchor: mode.and_then(|mode| mode.help_anchor()),
                indicator_style: self
                    .build_indicator(&display_mode)
                    .map(|(indicator, _)| indicator.style),
            },
        );
    }
}
