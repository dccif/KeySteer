//! Engine-side binding lookup and temporary-mode resolution.

use super::*;

impl Engine {
    pub(super) fn temporary_mode_is_active(&self, mode: &ModeId) -> bool {
        self.registry.temporary_chords(mode).is_some_and(|chords| {
            chords.iter().any(|entry| {
                let reserved_for_overlap = self.registry.active == ModeId::ui_hint()
                    && entry.conflicts_with_ui_hint_overlap;
                !reserved_for_overlap && entry.chord.matches_pressed(&self.input.pressed)
            })
        })
    }

    pub(super) fn key_may_change_temporary_mode(&self, key: &Key) -> bool {
        self.registry
            .temporary_chords(&self.registry.active)
            .is_some_and(|chords| {
                chords.iter().any(|entry| {
                    entry.chord.keys().iter().any(|configured| {
                        configured == key
                            || matches!(
                                (configured.as_str(), key.as_str()),
                                ("alt", "left_alt" | "right_alt")
                                    | ("ctrl", "left_ctrl" | "right_ctrl")
                                    | ("shift", "left_shift" | "right_shift")
                                    | ("win", "left_win" | "right_win")
                            )
                    })
                })
            })
    }

    pub(super) fn display_mode(&self) -> ModeId {
        if self.registry.active == ModeId::idle() {
            return self.registry.active.clone();
        }
        let Some(route) = self.registry.routes.get(&self.registry.active) else {
            return self.registry.active.clone();
        };
        let temporary_active = self.temporary_mode_is_active(&self.registry.active);
        if temporary_active
            && let Some(target) = route.temporary_mode.clone()
            && self.registry.contains_key(&target)
        {
            return target;
        }
        self.registry.active.clone()
    }

    pub(super) fn ui_hint_overlap_matches(&self, key: &Key) -> bool {
        self.ui_hint_overlap_chord
            .as_ref()
            .is_some_and(|chord| chord.activation_matches(key))
    }

    /// Find the binding for `key` using the compiled mode precedence rules.
    pub(super) fn lookup(&self, key: &Key) -> Option<ResolvedBinding> {
        let active_match = self.lookup_with_specificity_in(&self.registry.active, key);
        if self.registry.active == ModeId::idle() {
            return active_match.map(|(binding, _)| ResolvedBinding {
                binding,
                owner: self.registry.active.clone(),
            });
        }

        if self.registry.active == ModeId::ui_hint() && self.ui_hint_overlap_matches(key) {
            return None;
        }

        let Some(route) = self.registry.routes.get(&self.registry.active) else {
            return active_match.map(|(binding, _)| ResolvedBinding {
                binding,
                owner: self.registry.active.clone(),
            });
        };
        let temporary_active = self.temporary_mode_is_active(&self.registry.active);
        let temporary_match = temporary_active
            .then_some(route.temporary_mode.as_ref())
            .flatten()
            .cloned()
            .and_then(|owner| {
                self.lookup_with_specificity_in(&owner, key)
                    .map(|(binding, specificity)| (binding, owner, specificity))
            });

        if let Some((binding, owner, temporary_specificity)) = temporary_match
            && active_match
                .as_ref()
                .is_none_or(|(_, active_specificity)| *active_specificity <= temporary_specificity)
        {
            return Some(ResolvedBinding { binding, owner });
        }

        match active_match {
            Some((binding, _)) if matches!(binding.as_ref(), Binding::Disabled) => None,
            Some((binding, _)) => Some(ResolvedBinding {
                binding,
                owner: self.registry.active.clone(),
            }),
            None if self.active_claims_raw_key(key) => None,
            None => route.inherits.iter().find_map(|owner| {
                if *owner == self.registry.active {
                    return None;
                }
                self.lookup_inherited(owner, key, &mut SmallVec::new())
            }),
        }
    }

    pub(super) fn lookup_inherited(
        &self,
        owner: &ModeId,
        key: &Key,
        visited: &mut SmallVec<[ModeId; 8]>,
    ) -> Option<ResolvedBinding> {
        if visited.contains(owner) {
            return None;
        }
        visited.push(owner.clone());
        if let Some(binding) = self.lookup_in(owner, key) {
            return (binding.as_ref() != &Binding::Disabled).then(|| ResolvedBinding {
                binding,
                owner: owner.clone(),
            });
        }
        let sources = &self.registry.routes.get(owner)?.inherits;
        sources
            .iter()
            .find_map(|source| self.lookup_inherited(source, key, visited))
    }

    pub(super) fn active_claims_raw_key(&self, key: &Key) -> bool {
        self.registry
            .get(&self.registry.active)
            .is_some_and(|mode| mode.claims_key(key))
    }

    pub(super) fn lookup_in(&self, mode: &ModeId, key: &Key) -> Option<Arc<Binding>> {
        self.lookup_with_specificity_in(mode, key)
            .map(|(binding, _)| binding)
    }

    pub(super) fn lookup_with_specificity_in(
        &self,
        mode: &ModeId,
        key: &Key,
    ) -> Option<(Arc<Binding>, usize)> {
        let table = self.registry.table(mode)?;
        if self.strict_modifier_matching_enabled() {
            table.lookup_with_specificity_strict(key, &self.input.pressed, |modifier| {
                matches!(
                    self.input.key_dispositions.get(modifier),
                    Some(KeyDisposition::Consume)
                )
            })
        } else {
            table.lookup_with_specificity(key, &self.input.pressed)
        }
    }

    pub(super) fn update_drag_modifier(&mut self, input: &crate::api::input::InputEvent) -> bool {
        if self.input.drag_auto_release.buttons == 0 || self.registry.active != ModeId::normal() {
            return false;
        }
        let Some(bit) = drag_modifier_bit(&input.key) else {
            return false;
        };
        match input.state {
            KeyState::Down => {
                // A modifier pressed before the drag candidate may already be
                // consumed by Normal (for example left_shift=slow). Its Down
                // cannot be retroactively forwarded, so preserve the paired
                // disposition until the user releases and presses it again.
                if matches!(
                    self.input.key_dispositions.get(&input.key),
                    Some(KeyDisposition::Consume)
                ) {
                    return false;
                }
                self.input.drag_auto_release.modifiers |= bit;
                true
            }
            KeyState::Up => {
                if self.input.drag_auto_release.modifiers & bit == 0 {
                    return false;
                }
                self.input.drag_auto_release.modifiers &= !bit;
                true
            }
        }
    }

    pub(super) fn drag_move_binding(&self, key: &Key) -> Option<ResolvedBinding> {
        if self.registry.active != ModeId::normal()
            || self.input.drag_auto_release.buttons == 0
            || self.input.drag_auto_release.modifiers == 0
        {
            return None;
        }
        let ignored_modifiers = self.input.drag_auto_release.modifiers;
        let table = self.registry.table(&ModeId::normal())?;
        // Preserve an explicit binding for the complete physical chord even
        // when that binding is `none`. `lookup()` intentionally represents
        // `none` as no action, but drag fallback must not reinterpret that as
        // permission to run a shorter bare movement binding.
        if self.normal_binding_chain_matches(key) {
            return None;
        }
        let binding = table.lookup_move_with_pressed(key, |configured| {
            self.input.pressed.iter().any(|physical| {
                if !Self::keys_match(configured, physical) {
                    return false;
                }
                drag_modifier_bit(physical).is_none_or(|bit| ignored_modifiers & bit == 0)
            })
        })?;
        Some(ResolvedBinding {
            binding,
            owner: ModeId::normal(),
        })
    }

    pub(super) fn normal_binding_chain_matches(&self, key: &Key) -> bool {
        self.binding_chain_matches(&ModeId::normal(), key, &mut SmallVec::new())
    }

    pub(super) fn binding_chain_matches(
        &self,
        owner: &ModeId,
        key: &Key,
        visited: &mut SmallVec<[ModeId; 8]>,
    ) -> bool {
        if visited.contains(owner) {
            return false;
        }
        visited.push(owner.clone());
        if self.lookup_with_specificity_in(owner, key).is_some() {
            return true;
        }
        let Some(route) = self.registry.routes.get(owner) else {
            return false;
        };
        route
            .inherits
            .iter()
            .any(|source| self.binding_chain_matches(source, key, visited))
    }

    pub(super) fn strict_modifier_matching_enabled(&self) -> bool {
        self.registry.active == ModeId::idle()
            || (self.registry.active == ModeId::normal() && self.settings.passthrough_unbound_keys)
    }

    pub(super) fn injected_key(key: &Key) -> Key {
        let concrete = match key.as_str() {
            "shift" => "left_shift",
            "ctrl" => "left_ctrl",
            "alt" => "left_alt",
            "win" => "left_win",
            _ => return key.clone(),
        };
        Key::new(concrete).unwrap_or_else(|_| key.clone())
    }

    pub(super) fn modifier_family(key: &Key) -> Option<&'static str> {
        match key.as_str() {
            "shift" | "left_shift" | "right_shift" => Some("shift"),
            "ctrl" | "left_ctrl" | "right_ctrl" => Some("ctrl"),
            "alt" | "left_alt" | "right_alt" => Some("alt"),
            "win" | "left_win" | "right_win" => Some("win"),
            _ => None,
        }
    }

    pub(super) fn keys_match(left: &Key, right: &Key) -> bool {
        if left == right {
            return true;
        }
        let left_generic = matches!(left.as_str(), "shift" | "ctrl" | "alt" | "win");
        let right_generic = matches!(right.as_str(), "shift" | "ctrl" | "alt" | "win");
        (left_generic || right_generic)
            && Self::modifier_family(left)
                .zip(Self::modifier_family(right))
                .is_some_and(|(left, right)| left == right)
    }

    pub(super) fn targets_match(left: &InputTarget, right: &InputTarget) -> bool {
        match (left, right) {
            (InputTarget::Key(left), InputTarget::Key(right)) => Self::keys_match(left, right),
            (InputTarget::Mouse(left), InputTarget::Mouse(right)) => left == right,
            _ => false,
        }
    }

    pub(super) fn matching_latched_target(&self, target: &InputTarget) -> Option<InputTarget> {
        self.input
            .latched
            .iter()
            .find(|latched| Self::targets_match(latched, target))
            .cloned()
    }

    pub(super) fn latched_key_matches(&self, key: &Key) -> bool {
        self.input.latched.iter().any(
            |target| matches!(target, InputTarget::Key(latched) if Self::keys_match(latched, key)),
        )
    }

    #[inline]
    pub(super) fn reassert_latched_key_after_forwarded_release(
        &mut self,
        input: &crate::api::input::InputEvent,
        released_forwarded_key: bool,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        if !released_forwarded_key {
            return Ok(());
        }
        let result = {
            let Some(key) = self.input.latched.iter().find_map(|target| match target {
                InputTarget::Key(latched) if Self::keys_match(latched, &input.key) => Some(latched),
                _ => None,
            }) else {
                return Ok(());
            };
            // Latched keyboard targets are made concrete by `press_targets`
            // before insertion, so reassertion can borrow the stored key
            // directly instead of cloning its Arc-backed name.
            backend.send_key(key, KeyState::Down)
        };
        result.map_err(|error| {
            self.recoverable_input_error("reassert latched key after physical release", error)
        })?;
        self.recoverable_input_succeeded();
        Ok(())
    }
}
