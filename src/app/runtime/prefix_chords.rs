//! Release-to-commit arbitration for overlapping configured chords.
use super::input_router::ChordContinuation;
use super::input_state::ResolvedBinding;
use super::*;

#[derive(Debug)]
pub(super) struct PendingChord {
    key: Key,
    chord: Arc<KeyChord>,
    resolved: ResolvedBinding,
    continuations: Arc<[ChordContinuation]>,
}

impl Engine {
    fn continuation_is_available(&self, short: &KeyChord, candidate: &ChordContinuation) -> bool {
        // A generic short modifier must not promise a continuation on the
        // opposite physical side (right Alt held, left Alt required).
        if short.keys().iter().any(|key| {
            !self.input.pressed.iter().any(|physical| {
                Self::keys_match(key, physical)
                    && candidate
                        .chord
                        .keys()
                        .iter()
                        .any(|long| Self::keys_match(long, physical))
            })
        }) {
            return false;
        }

        let mut completed: SmallVec<[Key; 8]> = self.input.pressed.iter().cloned().collect();
        for key in candidate.chord.keys() {
            if !completed
                .iter()
                .any(|physical| Self::keys_match(key, physical))
            {
                completed.push(Self::injected_key(key));
            }
        }
        // Reuse the real resolver for inheritance, raw alphabets, temporary
        // modes, explicit `none`, and strict modifier ownership. Only candidates
        // already linked at compile time reach this check.
        self.lookup_for_pressed(candidate.chord.activation_key(), &completed)
            .is_some_and(|resolved| {
                resolved.owner == candidate.owner
                    && Arc::ptr_eq(&resolved.binding, &candidate.binding)
            })
    }

    pub(super) fn defer_prefix_chord(&mut self, resolved: &ResolvedBinding, key: &Key) -> bool {
        let Some(entry) = self
            .registry
            .table(&resolved.owner)
            .and_then(|table| table.prefix_entry(key, &resolved.binding))
        else {
            return false;
        };
        if !entry
            .continuations
            .iter()
            .any(|candidate| self.continuation_is_available(&entry.chord, candidate))
        {
            return false;
        }
        self.input.pending_chords.push(PendingChord {
            key: key.clone(),
            chord: entry.chord.clone(),
            resolved: resolved.clone(),
            continuations: entry.continuations.clone(),
        });
        true
    }

    pub(super) fn cancel_completed_prefixes(&mut self, resolved: &ResolvedBinding) {
        self.input.pending_chords.retain(|pending| {
            !pending.continuations.iter().any(|candidate| {
                candidate.owner == resolved.owner
                    && Arc::ptr_eq(&candidate.binding, &resolved.binding)
                    && candidate.chord.matches_pressed(&self.input.pressed)
            })
        });
    }

    pub(super) fn is_pending_chord_key(&self, key: &Key) -> bool {
        self.input
            .pending_chords
            .iter()
            .any(|pending| &pending.key == key)
    }

    pub(super) fn has_released_prefix(&self) -> bool {
        self.input
            .pending_chords
            .iter()
            .any(|pending| !pending.chord.matches_pressed(&self.input.pressed))
    }

    /// The caller has already disposed the native Up. Reuse normal action
    /// execution with a complete tap, including a release for held actions.
    pub(super) fn commit_released_prefixes(
        &mut self,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        let active = self.registry.active.clone();
        while let Some(index) = self
            .input
            .pending_chords
            .iter()
            .position(|pending| !pending.chord.matches_pressed(&self.input.pressed))
        {
            let pending = self.input.pending_chords.remove(index);
            let mut event = crate::api::input::InputEvent {
                key: pending.key.clone(),
                state: KeyState::Down,
                repeat: false,
                injected: false,
                timestamp_millis: 0,
            };
            self.apply_binding(pending.resolved.clone(), &event, backend)?;
            event.state = KeyState::Up;
            self.apply_binding(pending.resolved, &event, backend)?;
            let toggle = self.input.active_default_toggles.remove(&pending.key);
            self.finish_default_toggle(toggle, backend)?;
            if self.input.active_click_indicators.release(&pending.key) {
                self.refresh_overlay(backend)?;
            }
            if self.registry.active != active {
                break;
            }
        }
        Ok(())
    }
}
