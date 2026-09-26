//! Physical input pairing, held gestures, synthetic latches, and their Engine handling.

use super::input_router::CompiledKeymap;
use super::*;

pub(super) enum MappedChordKeys<'a> {
    Compiled(&'a [Key]),
    Filtered(SmallVec<[Key; 8]>),
}

impl MappedChordKeys<'_> {
    fn as_slice(&self) -> &[Key] {
        match self {
            Self::Compiled(keys) => keys,
            Self::Filtered(keys) => keys,
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct PendingLongPressToggle {
    pub(super) fires_at: Instant,
    pub(super) key: Key,
    pub(super) target: InputTarget,
    pub(super) short_action: Option<ButtonAction>,
}

/// Normal-mode drag state created only by a long-pressed click binding.
///
/// Buttons and the eight concrete left/right modifiers fit in two bytes. The
/// deadline stays empty until a forwarded modifier is held and the pointer
/// actually moves, so the disabled and awaiting paths never read the clock.
#[derive(Debug, Default)]
pub(super) struct DragAutoRelease {
    pub(super) buttons: u8,
    pub(super) modifiers: u8,
    pub(super) fires_at: Option<Instant>,
}

impl DragAutoRelease {
    pub(super) fn clear(&mut self) {
        self.buttons = 0;
        self.modifiers = 0;
        self.fires_at = None;
    }
}

pub(super) type TargetBuffer = SmallVec<[InputTarget; 8]>;

/// Inputs owned by `press`/`toggle` are a tiny sorted set in practice. Keeping
/// the common case inline avoids one tree-node allocation per held target.
#[derive(Debug, Default)]
pub(super) struct LatchedTargets(TargetBuffer);

impl LatchedTargets {
    pub(super) fn insert(&mut self, target: InputTarget) -> bool {
        match self.0.binary_search(&target) {
            Ok(_) => false,
            Err(index) => {
                self.0.insert(index, target);
                true
            }
        }
    }

    pub(super) fn remove(&mut self, target: &InputTarget) -> bool {
        let Ok(index) = self.0.binary_search(target) else {
            return false;
        };
        self.0.remove(index);
        true
    }

    pub(super) fn contains(&self, target: &InputTarget) -> bool {
        self.0.binary_search(target).is_ok()
    }

    pub(super) fn iter(&self) -> std::slice::Iter<'_, InputTarget> {
        self.0.iter()
    }

    pub(super) fn extend(&mut self, targets: impl IntoIterator<Item = InputTarget>) {
        for target in targets {
            self.insert(target);
        }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.0.len()
    }
}

#[derive(Debug, Default)]
pub(super) struct DefaultToggleKeys(SmallVec<[(Key, bool); 2]>);

impl DefaultToggleKeys {
    pub(super) fn insert(&mut self, key: Key, used: bool) -> Option<bool> {
        if let Some((_, current)) = self.0.iter_mut().find(|(candidate, _)| candidate == &key) {
            return Some(std::mem::replace(current, used));
        }
        self.0.push((key, used));
        None
    }

    pub(super) fn get_mut(&mut self, key: &Key) -> Option<&mut bool> {
        self.0
            .iter_mut()
            .find(|(candidate, _)| candidate == key)
            .map(|(_, value)| value)
    }

    pub(super) fn remove(&mut self, key: &Key) -> Option<bool> {
        let index = self.0.iter().position(|(candidate, _)| candidate == key)?;
        Some(self.0.swap_remove(index).1)
    }

    pub(super) fn values_mut(&mut self) -> impl Iterator<Item = &mut bool> {
        self.0.iter_mut().map(|(_, value)| value)
    }

    pub(super) fn keys(&self) -> impl Iterator<Item = &Key> {
        self.0.iter().map(|(key, _)| key)
    }

    pub(super) fn clear(&mut self) {
        self.0.clear();
    }
}

#[cfg(test)]
impl PartialEq<BTreeSet<InputTarget>> for LatchedTargets {
    fn eq(&self, other: &BTreeSet<InputTarget>) -> bool {
        self.iter().eq(other.iter())
    }
}

/// What the engine decided to do with a key, so the caller can tell the
/// backend whether to swallow it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum KeyOutcome {
    Consumed,
    Forwarded,
}

/// A binding together with the mode that must receive its stateful events.
#[derive(Debug, Clone)]
pub(super) struct ResolvedBinding {
    pub(super) binding: Arc<Binding>,
    pub(super) owner: ModeId,
}

/// Remembers the receiving mode across the release half of a held gesture.
#[derive(Debug, Clone)]
pub(super) struct ActiveGesture {
    pub(super) binding: Arc<Binding>,
    pub(super) owner: ModeId,
}

/// Click feedback follows the physical activation key until it is released.
/// This is visual state only; pending and latched mouse-button ownership stays
/// in the Engine input state rather than in the decoration.
#[derive(Debug, Default)]
pub(super) struct ActiveClickIndicators(SmallVec<[(Key, Button); 2]>);

impl ActiveClickIndicators {
    pub(super) fn activate(&mut self, key: Key, button: Button) {
        if let Some(index) = self.0.iter().position(|(candidate, _)| candidate == &key) {
            self.0.remove(index);
        }
        self.0.push((key, button));
    }

    pub(super) fn release(&mut self, key: &Key) -> bool {
        let Some(index) = self.0.iter().position(|(candidate, _)| candidate == key) else {
            return false;
        };
        self.0.remove(index);
        true
    }

    pub(super) fn release_buttons(&mut self, buttons: u8) {
        self.0
            .retain(|(_, button)| buttons & drag_button_bit(*button) == 0);
    }

    pub(super) fn latest_button(&self) -> Option<Button> {
        self.0.last().map(|(_, button)| *button)
    }

    pub(super) fn clear(&mut self) -> bool {
        let changed = !self.0.is_empty();
        self.0.clear();
        changed
    }

    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Physical keys are a tiny set in practice. A preallocated linear set avoids
/// one tree-node allocation on every fresh key-down while keeping lookups
/// cache-local (chords rarely contain more than a handful of keys).
#[derive(Debug)]
pub(super) struct PressedKeys(Vec<Key>);

impl Default for PressedKeys {
    fn default() -> Self {
        Self(Vec::with_capacity(16))
    }
}

impl PressedKeys {
    #[cfg(test)]
    pub(super) fn insert(&mut self, key: Key) -> bool {
        if self.0.contains(&key) {
            return false;
        }
        self.0.push(key);
        true
    }

    pub(super) fn insert_ref(&mut self, key: &Key) -> bool {
        if self.0.contains(key) {
            return false;
        }
        self.0.push(key.clone());
        true
    }

    pub(super) fn remove(&mut self, key: &Key) -> bool {
        let Some(index) = self.0.iter().position(|candidate| candidate == key) else {
            return false;
        };
        self.0.swap_remove(index);
        true
    }

    pub(super) fn clear(&mut self) {
        self.0.clear();
    }
}

impl std::ops::Deref for PressedKeys {
    type Target = [Key];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Debug)]
pub(super) struct KeyMap<T>(Vec<(Key, T)>);

impl<T> Default for KeyMap<T> {
    fn default() -> Self {
        Self(Vec::with_capacity(16))
    }
}

impl<T> KeyMap<T> {
    pub(super) fn insert(&mut self, key: Key, value: T) -> Option<T> {
        if let Some((_, current)) = self.0.iter_mut().find(|(candidate, _)| candidate == &key) {
            return Some(std::mem::replace(current, value));
        }
        self.0.push((key, value));
        None
    }

    pub(super) fn get(&self, key: &Key) -> Option<&T> {
        self.0
            .iter()
            .find(|(candidate, _)| candidate == key)
            .map(|(_, value)| value)
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = (&Key, &T)> {
        self.0.iter().map(|(key, value)| (key, value))
    }

    pub(super) fn remove(&mut self, key: &Key) -> Option<T> {
        let index = self.0.iter().position(|(candidate, _)| candidate == key)?;
        Some(self.0.swap_remove(index).1)
    }

    pub(super) fn clear(&mut self) {
        self.0.clear();
    }

    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl<T> IntoIterator for KeyMap<T> {
    type Item = (Key, T);
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

#[derive(Debug, Default)]
pub(super) struct InputState {
    window_temporary_transition: Option<bool>,
    pub(super) pending_chords: SmallVec<[super::prefix_chords::PendingChord; 2]>,
    pub(super) pressed: PressedKeys,
    pub(super) temporary_entry_keys: SmallVec<[Key; 8]>,
    pub(super) key_dispositions: KeyMap<KeyDisposition>,
    // Physical edges consumed to replay an interrupted prefix before this key.
    pub(super) replayed_keys: PressedKeys,
    pub(super) active_gestures: KeyMap<ActiveGesture>,
    pub(super) active_click_indicators: ActiveClickIndicators,
    pub(super) latched: LatchedTargets,
    pub(super) active_default_toggles: DefaultToggleKeys,
    pub(super) pending_long_press_toggles: SmallVec<[PendingLongPressToggle; 4]>,
    pub(super) drag_auto_release: DragAutoRelease,
}

impl InputState {
    /// Clear plan-owned transient state while preserving physical key edges and
    /// their disposition until the corresponding real KeyUp arrives.
    pub(super) fn reset_for_plan_swap(&mut self) {
        self.window_temporary_transition = None;
        self.pending_chords.clear();
        self.active_gestures.clear();
        self.active_click_indicators.clear();
        self.active_default_toggles.clear();
        self.drag_auto_release.clear();
    }

    /// Capture loss means the matching physical KeyUp can no longer arrive.
    pub(super) fn forget_physical_capture(&mut self) {
        self.pending_chords.clear();
        self.pressed.clear();
        self.temporary_entry_keys.clear();
        self.key_dispositions.clear();
        // replayed_keys owns synthetic Downs; release_latched drains it even
        // when physical capture is gone and no matching Up can arrive.
    }
}

impl Engine {
    pub(super) fn fire_due_long_press_toggles(
        &mut self,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        if self.input.pending_long_press_toggles.is_empty() {
            return Ok(());
        }
        if self.should_quit || !self.enabled || self.is_excluded_app() {
            return self.cancel_transient_clicks(backend).map(|_| ());
        }
        let now = Instant::now();
        while self
            .input
            .pending_long_press_toggles
            .last()
            .is_some_and(|toggle| toggle.fires_at <= now)
        {
            let Some(toggle) = self.input.pending_long_press_toggles.pop() else {
                break;
            };
            if !self.input.pressed.contains(&toggle.key) {
                self.cancel_pending_long_press(toggle, backend)?;
                continue;
            }
            // A direct mouse click is primed with MouseDown on the physical
            // key edge. Reaching the deadline only transfers ownership of that
            // already-held button to the latch; it must not inject a second
            // press. An already-latched button has no short action, so its long
            // press still toggles the latch off here.
            let long_pressed_click = toggle.short_action.is_some();
            if long_pressed_click {
                self.input.latched.insert(toggle.target.clone());
            } else {
                self.toggle_targets(std::slice::from_ref(&toggle.target), backend)?;
            }
            // The physical activation key no longer owns ordinary click
            // feedback once its pending press has become a latch. The latch
            // keeps the same pressed decoration until it is released, without
            // letting a still-held activation key resurrect stale feedback.
            if let InputTarget::Mouse(button) = &toggle.target {
                self.input.active_click_indicators.release(&toggle.key);
                if long_pressed_click {
                    self.begin_drag_auto_release(*button);
                }
            }
            self.transfer_pending_long_press_targets(std::slice::from_ref(&toggle.target));
            if matches!(&toggle.target, InputTarget::Key(key) if key == &toggle.key)
                && let Some(used) = self.input.active_default_toggles.get_mut(&toggle.key)
            {
                *used = true;
            }
            self.refresh_overlay(backend)?;
        }
        Ok(())
    }

    pub(super) fn begin_drag_auto_release(&mut self, button: Button) {
        if self.settings.auto_release_ms == 0 || self.registry.active != ModeId::normal() {
            return;
        }
        let button_bit = drag_button_bit(button);
        if self.input.drag_auto_release.buttons == 0 {
            self.input.drag_auto_release.modifiers = self.forwarded_modifier_mask();
        }
        // All auto-managed buttons deliberately share one deadline. A newly
        // added button must still observe a real pointer movement of its own
        // generation, so an older button's nearly-expired deadline cannot
        // release it immediately.
        if self.input.drag_auto_release.buttons & button_bit == 0 {
            self.input.drag_auto_release.fires_at = None;
        }
        self.input.drag_auto_release.buttons |= button_bit;
    }

    pub(super) fn forwarded_modifier_mask(&self) -> u8 {
        self.input
            .key_dispositions
            .iter()
            .fold(0, |mask, (key, disposition)| {
                if matches!(disposition, KeyDisposition::Defer | KeyDisposition::Forward) {
                    mask | drag_modifier_bit(key).unwrap_or(0)
                } else {
                    mask
                }
            })
    }

    pub(super) fn note_drag_pointer_moved(&mut self) {
        if self.input.drag_auto_release.buttons == 0 || self.registry.active != ModeId::normal() {
            return;
        }
        // A modifier is required only to start the automatic release. Once
        // armed, releasing every modifier early leaves the existing deadline
        // active and later pointer movement continues to extend it.
        if self.input.drag_auto_release.fires_at.is_none()
            && self.input.drag_auto_release.modifiers == 0
        {
            return;
        }
        let delay_ms = self.settings.auto_release_ms;
        if delay_ms == 0 {
            return;
        }
        self.input.drag_auto_release.fires_at =
            Some(Instant::now() + Duration::from_millis(delay_ms));
    }

    pub(super) fn take_drag_release_targets(&mut self) -> TargetBuffer {
        let buttons = self.input.drag_auto_release.buttons;
        self.input.drag_auto_release.clear();
        [
            Button::Left,
            Button::Right,
            Button::Middle,
            Button::X1,
            Button::X2,
        ]
        .into_iter()
        .filter(|button| buttons & drag_button_bit(*button) != 0)
        .map(InputTarget::Mouse)
        .collect()
    }

    pub(super) fn forget_drag_button(&mut self, button: Button) {
        self.input.drag_auto_release.buttons &= !drag_button_bit(button);
        if self.input.drag_auto_release.buttons == 0 {
            self.input.drag_auto_release.clear();
        }
    }

    pub(super) fn release_drag_auto_release(
        &mut self,
        backend: &mut dyn Backend,
    ) -> Result<bool, String> {
        let buttons = self.input.drag_auto_release.buttons;
        if buttons == 0 {
            return Ok(false);
        }
        let targets = self.take_drag_release_targets();
        self.release_targets(&targets, false, backend)?;

        // Automatic release wins over an overlapping second long-press for
        // the same button. Otherwise that late deadline could immediately
        // latch the button again after the idle release. A primed short click
        // still owns a native MouseDown and must keep its later MouseUp path.
        self.input.pending_long_press_toggles.retain(|pending| {
            let InputTarget::Mouse(button) = &pending.target else {
                return true;
            };
            buttons & drag_button_bit(*button) == 0 || pending.short_action.is_some()
        });
        self.input.active_click_indicators.release_buttons(buttons);
        Ok(true)
    }

    pub(super) fn fire_due_drag_auto_release(
        &mut self,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        let Some(fires_at) = self.input.drag_auto_release.fires_at else {
            return Ok(());
        };
        if fires_at > Instant::now() {
            return Ok(());
        }
        if self.release_drag_auto_release(backend)? {
            // Position-only overlay updates intentionally reuse the previous
            // scene. Rebuild once here so held text and pressed colors
            // disappear instead of following the pointer as stale content.
            self.refresh_overlay(backend)?;
        }
        Ok(())
    }

    #[inline]
    pub(super) fn handle_key(
        &mut self,
        input: crate::api::input::InputEvent,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        let switch_was_visible = self
            .quick_switch
            .pending
            .as_ref()
            .is_some_and(|p| p.visible);
        // Window's help is always visible, independently of the optional `?`
        // panel. Its effective bindings must also refresh on key releases.
        if !self.overlay.key_help_visible && !self.registry.active.is_window() {
            self.handle_key_inner(input, backend)?;
            if switch_was_visible && self.quick_switch.pending.is_none() {
                self.refresh_overlay(backend)?;
            }
            return Ok(());
        }
        let refresh = !input.injected && !input.repeat;
        if refresh {
            self.overlay.key_help_cache = None;
        }
        self.handle_key_inner(input, backend)?;
        if (refresh && (self.overlay.key_help_visible || self.registry.active.is_window()))
            || (switch_was_visible && self.quick_switch.pending.is_none())
        {
            self.refresh_overlay(backend)?;
        }
        Ok(())
    }

    fn handle_key_inner(
        &mut self,
        input: crate::api::input::InputEvent,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        if self.quick_switch_key(&input, backend)? {
            return Ok(());
        }
        // Holding a key can generate dozens of repeats per second. The normal
        // debug stream records the physical down/up edges; opt into `motion`
        // only when every OS repeat is needed for a performance trace.
        let trace_key = self.settings.debug.keys && (!input.repeat || self.settings.debug.motion);
        // Never re-process our own injected events, or modes would loop.
        if input.injected {
            return self.dispose_input(&input, KeyOutcome::Forwarded, trace_key, backend);
        }

        let display_before = self
            .key_may_change_temporary_mode(&input.key)
            .then(|| self.display_mode());
        let (completed_default_toggle, pressed_changed) = match input.state {
            KeyState::Down => {
                let changed = self.input.pressed.insert_ref(&input.key);
                (None, changed)
            }
            KeyState::Up => {
                let changed = self.input.pressed.remove(&input.key);
                self.input
                    .temporary_entry_keys
                    .retain(|key| key != &input.key);
                (
                    self.input.active_default_toggles.remove(&input.key),
                    changed,
                )
            }
        };
        let click_indicator_released =
            input.state == KeyState::Up && self.input.active_click_indicators.release(&input.key);
        // A physical Down may have reached the foreground application before a
        // later `press`/`toggle` action took ownership of that same key. Its
        // matching physical Up will clear the application's synthetic state as
        // well, so remember that exact case before disposition bookkeeping
        // removes the Down decision. Consumed keys already keep the synthetic
        // Down alive and must not receive a redundant edge.
        let released_forwarded_key = input.state == KeyState::Up
            && !self.input.latched.is_empty()
            && matches!(
                self.input.key_dispositions.get(&input.key),
                Some(KeyDisposition::Defer | KeyDisposition::Forward)
            );
        let completed_long_press = (input.state == KeyState::Up)
            .then(|| self.take_pending_long_press_toggle(&input.key))
            .flatten();
        let captures_default_toggle_partner = input.state == KeyState::Down
            && !input.repeat
            && self
                .input
                .active_default_toggles
                .keys()
                .any(|key| key != &input.key && self.input.pressed.contains(key));
        let display_changed = pressed_changed
            && display_before.is_some_and(|display_before| display_before != self.display_mode());
        if display_changed && self.registry.active.is_window() {
            self.input.window_temporary_transition =
                Some(self.temporary_mode_is_active(&self.registry.active));
        }

        if self.input.replayed_keys.contains(&input.key) {
            let outcome = self.complete_key_disposition(&input, KeyOutcome::Consumed);
            self.dispose_input(&input, outcome, trace_key, backend)?;
            if input.state == KeyState::Up && self.has_released_prefix() {
                self.commit_released_prefixes(backend)?;
            }
            return Ok(());
        }

        if !self.enabled || self.is_excluded_app() || self.window_presets.pending.is_some() {
            self.input.pending_chords.clear();
            if let Some(pending) = completed_long_press
                && let Err(error) = self.cancel_pending_long_press(pending, backend)
            {
                self.report_action_error(error, backend);
            }
            let outcome = self.complete_key_disposition(&input, KeyOutcome::Forwarded);
            self.dispose_input(&input, outcome, trace_key, backend)?;
            self.reassert_latched_key_after_forwarded_release(
                &input,
                released_forwarded_key,
                backend,
            )?;
            return Ok(());
        }

        // Prefix keys were consumed on Down. Resolve only after publishing Up,
        // and still release any independent gesture owned by this same key.
        if input.state == KeyState::Up && self.has_released_prefix() {
            let outcome = self.complete_key_disposition(&input, KeyOutcome::Consumed);
            self.dispose_input(&input, outcome, trace_key, backend)?;
            if let Some(gesture) = self.input.active_gestures.remove(&input.key) {
                let resolved = ResolvedBinding {
                    binding: gesture.binding,
                    owner: gesture.owner,
                };
                if let Err(error) = self.apply_binding(resolved, &input, backend) {
                    self.report_action_error(error, backend);
                }
            }
            if let Some(pending) = completed_long_press
                && let Err(error) = self.complete_pending_mouse_click(pending, backend)
            {
                self.report_action_error(error, backend);
            }
            if let Err(error) = self.finish_default_toggle(completed_default_toggle, backend) {
                self.report_action_error(error, backend);
            }
            if let Err(error) = self.commit_released_prefixes(backend) {
                self.report_action_error(error, backend);
            }
            self.reassert_latched_key_after_forwarded_release(
                &input,
                released_forwarded_key,
                backend,
            )?;
            if display_changed || click_indicator_released {
                self.refresh_overlay(backend)?;
            }
            return Ok(());
        }
        if input.state == KeyState::Down
            && !pressed_changed
            && self.is_pending_chord_key(&input.key)
        {
            let outcome = self.complete_key_disposition(&input, KeyOutcome::Consumed);
            return self.dispose_input(&input, outcome, trace_key, backend);
        }

        if let Some(pending) = completed_long_press {
            let outcome = self.complete_key_disposition(&input, KeyOutcome::Consumed);
            self.dispose_input(&input, outcome, trace_key, backend)?;
            self.trace_key_resolution(&input, None, trace_key);
            if let Err(error) = self.finish_default_toggle(completed_default_toggle, backend) {
                self.report_action_error(error, backend);
            }
            if let Err(error) = self.complete_pending_mouse_click(pending, backend) {
                self.report_action_error(error, backend);
            }
            self.reassert_latched_key_after_forwarded_release(
                &input,
                released_forwarded_key,
                backend,
            )?;
            if display_changed || click_indicator_released {
                self.refresh_overlay(backend)?;
            }
            return Ok(());
        }

        // Resolve the key. On press we consult the active mode's table; on
        // release we use the gesture that press started, because the chord no
        // longer matches once the keys are up.
        let mut bound = match input.state {
            KeyState::Down if input.repeat => self
                .input
                .active_gestures
                .get(&input.key)
                // Volume repeats require the complete configured chord to stay
                // pressed. Releasing V must not continue changing app audio.
                .filter(|gesture| {
                    if matches!(gesture.binding.as_ref(), Binding::Send(_)) {
                        // The same compiled source binding must still match.
                        // Never fall back to bare H after C+H loses its prefix,
                        // even if both bindings happen to send the same key.
                        return self
                            .lookup_character(&input)
                            .or_else(|| self.lookup(&input.key))
                            .is_some_and(|resolved| {
                                resolved.owner == gesture.owner
                                    && Arc::ptr_eq(&resolved.binding, &gesture.binding)
                            });
                    }
                    !matches!(
                        gesture.binding.as_ref(),
                        Binding::Window(
                            crate::api::window::WindowAction::VolumeDown
                                | crate::api::window::WindowAction::VolumeUp
                                | crate::api::window::WindowAction::SystemVolumeDown
                                | crate::api::window::WindowAction::SystemVolumeUp
                        )
                    ) || self
                        .lookup(&input.key)
                        .is_some_and(|resolved| resolved.binding == gesture.binding)
                })
                .cloned()
                .map(|gesture| ResolvedBinding {
                    binding: gesture.binding,
                    owner: gesture.owner,
                }),
            KeyState::Down if input.character.is_some() => self
                .lookup_character(&input)
                .or_else(|| self.lookup(&input.key)),
            KeyState::Down => self.lookup(&input.key),
            KeyState::Up => self
                .input
                .active_gestures
                .remove(&input.key)
                .map(|gesture| ResolvedBinding {
                    binding: gesture.binding,
                    owner: gesture.owner,
                }),
        };

        // A key pressed after a parameterless toggle activation becomes that
        // toggle's target whether or not it has its own binding. Decide and
        // publish the consumed disposition before any target injection or
        // overlay work, so an unbound partner cannot leak through Normal's
        // passthrough path first.
        if captures_default_toggle_partner {
            let outcome = self.complete_key_disposition(&input, KeyOutcome::Consumed);
            self.dispose_input(&input, outcome, trace_key, backend)?;
            self.trace_key_resolution(&input, bound.as_ref(), trace_key);
            if let Err(error) = self.capture_default_toggle_partner(&input.key, backend) {
                self.report_action_error(error, backend);
            }
            return Ok(());
        }

        // During a long-press drag candidate, physical modifiers belong to the
        // foreground application rather than Normal's own shift/click
        // bindings. Disposition pairing preserves every forwarded Down/Up,
        // including arbitrary left/right modifier combinations.
        if self.update_drag_modifier(&input) {
            let outcome = self.complete_key_disposition(&input, KeyOutcome::Forwarded);
            self.dispose_input(&input, outcome, trace_key, backend)?;
            self.trace_key_resolution(&input, None, trace_key);
            if let Err(error) = self.finish_default_toggle(completed_default_toggle, backend) {
                self.report_action_error(error, backend);
            }
            self.reassert_latched_key_after_forwarded_release(
                &input,
                released_forwarded_key,
                backend,
            )?;
            if display_changed || click_indicator_released {
                self.refresh_overlay(backend)?;
            }
            return Ok(());
        }

        // Exact configured chords were resolved above. If none matched, only
        // borrow a bare/custom Normal movement binding while the drag's
        // forwarded modifiers are held; no other action becomes eligible.
        if bound.is_none() && input.state == KeyState::Down && !input.repeat {
            bound = self.drag_move_binding(&input.key);
        }

        if let Some(resolved) = bound {
            if input.state == KeyState::Down && !input.repeat {
                self.cancel_completed_prefixes(&resolved);
                // A modifier-based prefix needs at least two pressed keys.
                if (!self.registry.prefixes_require_modifier || self.input.pressed.len() > 1)
                    && self.defer_prefix_chord(&resolved, &input.key)
                {
                    let outcome = self.complete_key_disposition(&input, KeyOutcome::Consumed);
                    self.dispose_input(&input, outcome, trace_key, backend)?;
                    if display_changed {
                        self.refresh_overlay(backend)?;
                    }
                    return Ok(());
                }
            }
            // Remember both the binding and its recipient so a release stops a
            // normal gesture even while grid, recursive_grid or ui_hint remains
            // the active mode.
            if input.state == KeyState::Down && resolved.binding.repeats_on_key_down() {
                self.input.active_gestures.insert(
                    input.key.clone(),
                    ActiveGesture {
                        binding: resolved.binding.clone(),
                        owner: self.stateful_binding_owner(&resolved),
                    },
                );
            }
            // The hook only waits for this disposition. Send it before mouse
            // injection, process spawning or overlay painting can block.
            let outcome = self.complete_key_disposition(&input, KeyOutcome::Consumed);
            self.dispose_input(&input, outcome, trace_key, backend)?;
            self.trace_key_resolution(&input, Some(&resolved), trace_key);
            if let Err(error) = self.finish_default_toggle(completed_default_toggle, backend) {
                self.report_action_error(error, backend);
            }
            let pending_long_press = self.pending_long_press_toggle(&resolved, &input);
            let deferred_button = pending_long_press.as_ref().and_then(|pending| {
                let InputTarget::Mouse(button) = &pending.target else {
                    return None;
                };
                Some(*button)
            });
            let duplicate_pending = deferred_button.is_some()
                && pending_long_press.as_ref().is_some_and(|pending| {
                    self.input
                        .pending_long_press_toggles
                        .iter()
                        .any(|current| Self::targets_match(&current.target, &pending.target))
                });
            let applied = if let (Some(pending), Some(button)) =
                (pending_long_press.as_ref(), deferred_button)
            {
                if duplicate_pending || pending.short_action.is_none() {
                    true
                } else {
                    match self.inject_mouse_button(map_button(button), ButtonAction::Press, backend)
                    {
                        Ok(()) => true,
                        Err(error) => {
                            self.report_action_error(error, backend);
                            false
                        }
                    }
                }
            } else {
                match self.apply_binding(resolved, &input, backend) {
                    Ok(_) => true,
                    Err(error) => {
                        self.report_action_error(error, backend);
                        false
                    }
                }
            };
            if applied
                && !duplicate_pending
                && let Some(pending) = pending_long_press
            {
                self.input
                    .pending_long_press_toggles
                    .retain(|current| current.key != pending.key);
                let index = self
                    .input
                    .pending_long_press_toggles
                    .partition_point(|current| current.fires_at > pending.fires_at);
                self.input.pending_long_press_toggles.insert(index, pending);
                if let Some(button) = deferred_button
                    && let Err(error) = self.activate_click_indicator(&input, button, backend)
                {
                    self.report_action_error(error, backend);
                }
            }
            self.reassert_latched_key_after_forwarded_release(
                &input,
                released_forwarded_key,
                backend,
            )?;
            if display_changed || click_indicator_released {
                self.refresh_overlay(backend)?;
            }
            return Ok(());
        }

        let captures_temporary_trigger = self.registry.active == ModeId::text_input()
            && self.key_may_change_temporary_mode(&input.key)
            && self.temporary_mode_is_active(&self.registry.active);
        let captures = captures_temporary_trigger
            || (!input.key.is_mouse_side_button()
                && self
                    .registry
                    .get(&self.registry.active)
                    .map(|m| m.captures_keyboard())
                    .unwrap_or(false));

        if !captures
            && input.state == KeyState::Down
            && !input.repeat
            && pressed_changed
            && self.defer_unbound_prefix(&input.key)
        {
            let outcome = self.complete_key_disposition(&input, KeyOutcome::Consumed);
            return self.dispose_input(&input, outcome, trace_key, backend);
        }

        let outcome = if captures {
            KeyOutcome::Consumed
        } else {
            KeyOutcome::Forwarded
        };
        let outcome = self.complete_key_disposition(&input, outcome);
        self.dispose_input(&input, outcome, trace_key, backend)?;
        self.trace_key_resolution(&input, None, trace_key);
        if let Err(error) = self.finish_default_toggle(completed_default_toggle, backend) {
            self.report_action_error(error, backend);
        }

        // Raw-mode handling may redraw a large target scene, so it must also
        // happen after the hook has received its disposition.
        if !input.key.is_mouse_side_button() {
            self.dispatch(
                ModeEvent::Key {
                    key: input.key.clone(),
                    state: input.state,
                    repeat: input.repeat,
                },
                backend,
            )?;
        }
        self.reassert_latched_key_after_forwarded_release(&input, released_forwarded_key, backend)?;
        if display_changed || click_indicator_released {
            self.refresh_overlay(backend)?;
        }
        Ok(())
    }

    pub(super) fn dispose_input(
        &mut self,
        input: &crate::api::input::InputEvent,
        outcome: KeyOutcome,
        trace_key: bool,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        let flush_prefixes = !input.injected && self.passthrough_prefix_interrupted(input);
        let replay = !input.injected
            && (self.input.replayed_keys.contains(&input.key)
                || (flush_prefixes && matches!(outcome, KeyOutcome::Forwarded)));
        let disposition = match outcome {
            _ if replay => KeyDisposition::Consume,
            KeyOutcome::Consumed => KeyDisposition::Consume,
            KeyOutcome::Forwarded => KeyDisposition::Forward,
        };
        if replay && input.state == KeyState::Down {
            self.input
                .key_dispositions
                .insert(input.key.clone(), KeyDisposition::Consume);
        }
        backend
            .dispose_key(disposition)
            .map_err(|error| self.recoverable_input_error("keyboard disposition", error))?;
        // A text-entry temporary layer ends when its modifier is released,
        // even when a movement companion remains physically held.
        if !input.injected
            && input.state == KeyState::Up
            && input.key.is_modifier()
            && self.registry.active == ModeId::text_input()
            && self.key_may_change_temporary_mode(&input.key)
            && !self.temporary_mode_is_active(&self.registry.active)
        {
            let released: SmallVec<[(Key, ActiveGesture); 4]> = self
                .input
                .active_gestures
                .iter()
                .filter(|(_, gesture)| gesture.owner != self.registry.active)
                .map(|(key, gesture)| (key.clone(), gesture.clone()))
                .collect();
            for (key, gesture) in released {
                self.input.active_gestures.remove(&key);
                self.dispatch_to(
                    &gesture.owner,
                    ModeEvent::Binding {
                        binding: gesture.binding,
                        state: KeyState::Up,
                        key,
                    },
                    backend,
                )?;
            }
            self.release_toggle_session_for_safe_mode(backend)?;
        }
        // Publish the native decision before injection. An unrelated forwarded
        // key must join the replay so its Down cannot overtake the prefix.
        if flush_prefixes {
            self.flush_passthrough_prefixes(Some(&input.key), backend)?;
        }
        if replay {
            let target = Self::replay_target(&input.key);
            match input.state {
                KeyState::Down => {
                    self.input.replayed_keys.insert_ref(&input.key);
                    Self::inject_target(&target, KeyState::Down, backend).map_err(|error| {
                        self.recoverable_input_error("replayed key press", error)
                    })?;
                }
                KeyState::Up => {
                    // Explicit press/toggle ownership survives a consumed Up.
                    if self.matching_latched_target(&target).is_none() {
                        Self::inject_target(&target, KeyState::Up, backend).map_err(|error| {
                            self.recoverable_input_error("replayed key release", error)
                        })?;
                    }
                    self.input.replayed_keys.remove(&input.key);
                }
            }
        }
        if let Some(active) = self.input.window_temporary_transition.take()
            && self.registry.active.is_window()
        {
            self.dispatch(ModeEvent::TemporaryModeChanged { active }, backend)?;
        }
        self.trace_lazy(trace_key, "key", || {
            format!(
                "received key={} state={:?} repeat={} injected={} mode={} disposition={disposition:?}",
                input.key, input.state, input.repeat, input.injected, self.registry.active
            )
        });
        Ok(())
    }

    #[inline]
    pub(super) fn trace_key_resolution(
        &self,
        input: &crate::api::input::InputEvent,
        bound: Option<&ResolvedBinding>,
        trace_key: bool,
    ) {
        if trace_key {
            self.trace_key_resolution_enabled(input, bound);
        }
    }

    #[cold]
    #[inline(never)]
    fn trace_key_resolution_enabled(
        &self,
        input: &crate::api::input::InputEvent,
        bound: Option<&ResolvedBinding>,
    ) {
        self.trace_lazy(true, "resolve", || match bound {
            Some(resolved) => format!(
                "key={} pressed={:?} mode={} owner={} action={:?}",
                input.key,
                self.input.pressed,
                self.registry.active,
                resolved.owner,
                resolved.binding
            ),
            None => format!(
                "key={} pressed={:?} mode={} action=<unbound>",
                input.key, self.input.pressed, self.registry.active
            ),
        });
        if bound.is_none()
            && input.state == KeyState::Down
            && !input.key.is_modifier()
            && self.registry.active == ModeId::idle()
        {
            self.trace_lazy(true, "resolve", || {
                let available = self
                    .registry
                    .table(&ModeId::idle())
                    .map(CompiledKeymap::entries)
                    .unwrap_or_default();
                format!(
                    "idle chord did not match; pressed={:?}; configured launchers={available:?}",
                    self.input.pressed
                )
            });
        }
    }

    pub(super) fn complete_key_disposition(
        &mut self,
        input: &crate::api::input::InputEvent,
        current: KeyOutcome,
    ) -> KeyOutcome {
        match input.state {
            KeyState::Down => {
                if let Some(disposition) = self.input.key_dispositions.get(&input.key) {
                    return match disposition {
                        KeyDisposition::Consume => KeyOutcome::Consumed,
                        KeyDisposition::Defer | KeyDisposition::Forward => KeyOutcome::Forwarded,
                    };
                }
                let disposition = match current {
                    KeyOutcome::Consumed => KeyDisposition::Consume,
                    KeyOutcome::Forwarded => KeyDisposition::Forward,
                };
                self.input
                    .key_dispositions
                    .insert(input.key.clone(), disposition);
                current
            }
            KeyState::Up => match self.input.key_dispositions.remove(&input.key) {
                Some(KeyDisposition::Consume) => KeyOutcome::Consumed,
                Some(KeyDisposition::Defer | KeyDisposition::Forward) => KeyOutcome::Forwarded,
                None => current,
            },
        }
    }

    pub(super) fn pending_long_press_toggle(
        &self,
        resolved: &ResolvedBinding,
        input: &crate::api::input::InputEvent,
    ) -> Option<PendingLongPressToggle> {
        if self.settings.long_press_toggle_ms == 0
            || resolved.owner != ModeId::normal()
            || input.state != KeyState::Down
            || input.repeat
            || input.injected
        {
            return None;
        }
        let (target, short_action) = match resolved.binding.as_ref() {
            Binding::Click(button) => (InputTarget::Mouse(*button), Some(ButtonAction::Click)),
            Binding::DoubleClick(button) => {
                (InputTarget::Mouse(*button), Some(ButtonAction::DoubleClick))
            }
            Binding::Toggle(targets)
                if targets.is_empty() && !self.has_pressed_toggle_partner(&input.key) =>
            {
                (InputTarget::Key(input.key.clone()), None)
            }
            _ => return None,
        };
        let short_action = short_action.filter(|_| self.matching_latched_target(&target).is_none());
        Some(PendingLongPressToggle {
            fires_at: Instant::now() + Duration::from_millis(self.settings.long_press_toggle_ms),
            key: input.key.clone(),
            target,
            short_action,
        })
    }

    pub(super) fn take_pending_long_press_toggle(
        &mut self,
        key: &Key,
    ) -> Option<PendingLongPressToggle> {
        let index = self
            .input
            .pending_long_press_toggles
            .iter()
            .position(|pending| &pending.key == key)?;
        Some(self.input.pending_long_press_toggles.remove(index))
    }

    pub(super) fn complete_pending_mouse_click(
        &mut self,
        pending: PendingLongPressToggle,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        let (InputTarget::Mouse(button), Some(action)) = (pending.target, pending.short_action)
        else {
            return Ok(());
        };
        let target = InputTarget::Mouse(button);
        // MouseDown already succeeded on the physical activation edge. Move
        // that native ownership into `latched` before releasing it, so a
        // failed MouseUp remains visible to the common recovery/shutdown path.
        let button = map_button(button);
        self.release_primed_mouse_button(target, button, backend)?;
        if action == ButtonAction::DoubleClick {
            // MouseDown was sent on the activation edge and MouseUp above
            // completed the first click. One complete click now produces the
            // second click without adding a third semantic Clicked event.
            self.inject_mouse_button(button, ButtonAction::Click, backend)?;
        }
        self.dispatch(ModeEvent::Clicked { button, action }, backend)
    }

    pub(super) fn cancel_pending_long_press(
        &mut self,
        pending: PendingLongPressToggle,
        backend: &mut dyn Backend,
    ) -> Result<bool, String> {
        let PendingLongPressToggle {
            key,
            target,
            short_action,
            ..
        } = pending;
        if short_action.is_some()
            && let InputTarget::Mouse(button) = target
        {
            let target = InputTarget::Mouse(button);
            self.release_primed_mouse_button(target, map_button(button), backend)?;
        }
        Ok(self.input.active_click_indicators.release(&key))
    }

    pub(super) fn release_primed_mouse_button(
        &mut self,
        target: InputTarget,
        button: MouseButton,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        self.input.latched.insert(target.clone());
        self.inject_mouse_button(button, ButtonAction::Release, backend)?;
        self.input.latched.remove(&target);
        Ok(())
    }

    /// Cancel native pending clicks and clear every activation-key decoration.
    /// The caller can use the returned dirty bit to rebuild the overlay once.
    pub(super) fn cancel_transient_clicks(
        &mut self,
        backend: &mut dyn Backend,
    ) -> Result<bool, String> {
        let pending = std::mem::take(&mut self.input.pending_long_press_toggles);
        let mut errors = crate::support::errors::ErrorBundle::default();
        let mut feedback_changed = false;
        for pending in pending {
            match self.cancel_pending_long_press(pending, backend) {
                Ok(changed) => feedback_changed |= changed,
                Err(error) => errors.push("release pending mouse press", error),
            }
        }
        feedback_changed |= self.input.active_click_indicators.clear();
        errors.into_result().map(|()| feedback_changed)
    }

    pub(super) fn transfer_pending_long_press_targets(
        &mut self,
        targets: &[InputTarget],
    ) -> TargetBuffer {
        let mut primed = TargetBuffer::new();
        self.input.pending_long_press_toggles.retain(|pending| {
            let matched = targets
                .iter()
                .any(|target| Self::targets_match(target, &pending.target));
            if matched && pending.short_action.is_some() {
                primed.push(pending.target.clone());
            }
            !matched
        });
        self.input.latched.extend(primed.iter().cloned());
        primed
    }

    pub(super) fn unprimed_toggle_targets(&mut self, targets: &[InputTarget]) -> TargetBuffer {
        let primed = self.transfer_pending_long_press_targets(targets);
        targets
            .iter()
            .filter(|target| {
                !primed
                    .iter()
                    .any(|pressed| Self::targets_match(pressed, target))
            })
            .cloned()
            .collect()
    }

    pub(super) fn has_pressed_toggle_partner(&self, activation: &Key) -> bool {
        self.input.pressed.iter().any(|key| key != activation)
    }

    pub(super) fn push_unique_toggle_target(targets: &mut TargetBuffer, target: &InputTarget) {
        if !targets
            .iter()
            .any(|current| Self::targets_match(current, target))
        {
            targets.push(target.clone());
        }
    }

    pub(super) fn append_toggle_binding_targets(binding: &Binding, targets: &mut TargetBuffer) {
        match binding {
            Binding::Click(button) | Binding::DoubleClick(button) => {
                Self::push_unique_toggle_target(targets, &InputTarget::Mouse(*button));
            }
            Binding::Send(chord) => {
                for key in chord.keys() {
                    Self::push_unique_toggle_target(targets, &InputTarget::Key(key.clone()));
                }
            }
            Binding::Press(items) | Binding::Release(items) | Binding::Toggle(items)
                if !items.is_empty() =>
            {
                for target in items {
                    Self::push_unique_toggle_target(targets, target);
                }
            }
            Binding::Sequence(actions) => {
                for action in actions {
                    Self::append_toggle_binding_targets(action, targets);
                }
            }
            _ => {}
        }
    }

    pub(super) fn toggle_partner_targets(key: &Key, binding: Option<&Binding>) -> TargetBuffer {
        let mut targets = TargetBuffer::new();
        if let Some(binding) = binding {
            Self::append_toggle_binding_targets(binding, &mut targets);
        }
        if targets.is_empty() {
            targets.push(InputTarget::Key(key.clone()));
        }
        targets
    }

    pub(super) fn toggle_key_is_held(&self, configured: &Key) -> bool {
        self.input.pressed
            .iter()
            .any(|physical| Self::keys_match(configured, physical))
            || self.input.latched.iter().any(|target| {
                matches!(target, InputTarget::Key(key) if Self::keys_match(configured, key))
            })
    }

    pub(super) fn normal_toggle_partner_targets(&self, key: &Key) -> TargetBuffer {
        let binding = self.registry.table(&ModeId::normal()).and_then(|table| {
            table.lookup_ref_with_pressed(key, |configured| self.toggle_key_is_held(configured))
        });
        Self::toggle_partner_targets(key, binding)
    }

    pub(super) fn extend_unique_toggle_targets(targets: &mut TargetBuffer, incoming: TargetBuffer) {
        for target in incoming {
            if !targets
                .iter()
                .any(|current| Self::targets_match(current, &target))
            {
                targets.push(target);
            }
        }
    }

    pub(super) fn pressed_toggle_targets(&self, activation: &Key) -> TargetBuffer {
        let mut targets = TargetBuffer::new();
        for key in self.input.pressed.iter().filter(|key| *key != activation) {
            Self::extend_unique_toggle_targets(
                &mut targets,
                self.normal_toggle_partner_targets(key),
            );
        }
        targets
    }

    pub(super) fn capture_default_toggle_partner(
        &mut self,
        key: &Key,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        let targets = self.normal_toggle_partner_targets(key);
        if targets.is_empty() {
            return Ok(());
        }
        for used in self.input.active_default_toggles.values_mut() {
            *used = true;
        }
        self.input.pending_long_press_toggles.retain(
            |pending| {
                !matches!(&pending.target, InputTarget::Key(target) if target == &pending.key)
            },
        );
        self.transfer_pending_long_press_targets(&targets);
        self.press_targets(&targets, backend)?;
        self.refresh_overlay(backend)
    }

    pub(super) fn finish_default_toggle(
        &mut self,
        used: Option<bool>,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        if used == Some(false) && !self.input.latched.is_empty() {
            self.release_latched(backend)?;
            self.refresh_overlay(backend)?;
        }
        Ok(())
    }

    pub(super) fn activate_click_indicator(
        &mut self,
        input: &crate::api::input::InputEvent,
        button: Button,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        // Plugin/lifecycle actions have no physical release edge. A delayed
        // sequence retains its original key-down event, so also require that
        // the activation key is still physically held when the click runs.
        if input.injected
            || input.state != KeyState::Down
            || input.repeat
            || !self.input.pressed.contains(&input.key)
        {
            return Ok(());
        }
        self.input
            .active_click_indicators
            .activate(input.key.clone(), button);
        self.refresh_overlay(backend)
    }

    pub(super) fn inject_target(
        target: &InputTarget,
        state: KeyState,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        match target {
            InputTarget::Key(key) => backend.send_key(&Self::injected_key(key), state),
            InputTarget::Mouse(button) => backend.mouse_button(
                map_button(*button),
                match state {
                    KeyState::Down => ButtonAction::Press,
                    KeyState::Up => ButtonAction::Release,
                },
            ),
        }
    }

    pub(super) fn press_targets(
        &mut self,
        targets: &[InputTarget],
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        let mut pressed = TargetBuffer::new();
        for target in targets {
            if self.matching_latched_target(target).is_some() {
                // An explicit `press` or parameterless `toggle` takes over an
                // already-held auto-drag button without injecting a duplicate
                // Down. Remove only its automatic owner so the old idle
                // deadline cannot release the explicit latch later.
                if let InputTarget::Mouse(button) = target {
                    self.forget_drag_button(*button);
                }
                continue;
            }
            let actual = match target {
                InputTarget::Key(key) => InputTarget::Key(Self::injected_key(key)),
                InputTarget::Mouse(button) => InputTarget::Mouse(*button),
            };
            if let Err(error) = Self::inject_target(&actual, KeyState::Down, backend) {
                let mut errors = crate::support::errors::ErrorBundle::default();
                errors.push("press", error);
                for rollback in pressed.iter().rev() {
                    match Self::inject_target(rollback, KeyState::Up, backend) {
                        Ok(()) => {
                            self.input.latched.remove(rollback);
                        }
                        Err(rollback_error) => {
                            errors.push(format!("roll back {rollback:?}"), rollback_error)
                        }
                    }
                }
                return errors
                    .into_result()
                    .map_err(|error| self.recoverable_input_error("input press", error));
            }
            self.input.latched.insert(actual.clone());
            pressed.push(actual);
        }
        Ok(())
    }

    pub(super) fn release_targets(
        &mut self,
        targets: &[InputTarget],
        force: bool,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        let mut errors = crate::support::errors::ErrorBundle::default();
        for target in targets.iter().rev() {
            let matched = self.matching_latched_target(target);
            if !force && matched.is_none() {
                continue;
            }
            let actual = matched.unwrap_or_else(|| match target {
                InputTarget::Key(key) => InputTarget::Key(Self::injected_key(key)),
                InputTarget::Mouse(button) => InputTarget::Mouse(*button),
            });
            match Self::inject_target(&actual, KeyState::Up, backend) {
                Ok(()) => {
                    self.input.latched.remove(&actual);
                    if let InputTarget::Mouse(button) = &actual {
                        self.forget_drag_button(*button);
                    }
                }
                Err(error) => errors.push(format!("release {actual:?}"), error),
            }
        }
        errors
            .into_result()
            .map_err(|error| self.recoverable_input_error("input release", error))
    }

    pub(super) fn toggle_targets(
        &mut self,
        targets: &[InputTarget],
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        let original: TargetBuffer = self.input.latched.iter().cloned().collect();
        for target in targets {
            let result = if self.matching_latched_target(target).is_some() {
                self.release_targets(std::slice::from_ref(target), false, backend)
            } else {
                self.press_targets(std::slice::from_ref(target), backend)
            };
            if let Err(error) = result {
                if let Err(rollback_error) = self.restore_latched(&original, backend) {
                    // Keep diagnostic I/O at the recovery boundary, after all
                    // held inputs have had a chance to be released.
                    let message = format!("{error}; toggle rollback: {rollback_error}");
                    self.pending_runtime_error =
                        Some(RuntimeError::RecoverableInput(message.clone()));
                    return Err(message);
                }
                return Err(error);
            }
        }
        Ok(())
    }

    pub(super) fn restore_latched(
        &mut self,
        original: &[InputTarget],
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        let release: TargetBuffer = self
            .input
            .latched
            .iter()
            .filter(|target| original.binary_search(target).is_err())
            .cloned()
            .collect();
        self.release_targets(&release, false, backend)?;
        let press: TargetBuffer = original
            .iter()
            .filter(|target| !self.input.latched.contains(target))
            .cloned()
            .collect();
        self.press_targets(&press, backend)
    }

    pub(super) fn release_latched(&mut self, backend: &mut dyn Backend) -> Result<(), String> {
        // Transfer transient replay ownership only during cleanup; normal
        // typing must not look like a user-created toggle latch.
        self.input
            .latched
            .extend(self.input.replayed_keys.iter().map(Self::replay_target));
        self.input.replayed_keys.clear();
        let held: TargetBuffer = self.input.latched.iter().cloned().collect();
        self.release_targets(&held, false, backend)
    }

    fn replay_target(key: &Key) -> InputTarget {
        match key.as_str() {
            "mouse_x1" => InputTarget::Mouse(crate::api::binding::Button::X1),
            "mouse_x2" => InputTarget::Mouse(crate::api::binding::Button::X2),
            _ => InputTarget::Key(key.clone()),
        }
    }

    pub(super) fn release_toggle_session_for_safe_mode(
        &mut self,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        // A pending ordinary mouse binding already owns a native MouseDown.
        // Transfer that ownership to the common latch set before clearing the
        // deadline, so one reverse-order release path handles every target and
        // retains any failed MouseUp/KeyUp for recovery or shutdown retry.
        for pending in std::mem::take(&mut self.input.pending_long_press_toggles) {
            if pending.short_action.is_some() {
                self.input.latched.insert(pending.target);
            }
        }
        self.input.active_default_toggles.clear();
        self.input.active_click_indicators.clear();
        self.input.drag_auto_release.clear();
        self.release_latched(backend)
    }

    pub(super) fn releases_toggle_session_on_entry(target: &ModeId) -> bool {
        matches!(target.as_str(), "normal" | "idle" | "text_input")
    }

    pub(super) fn send_chord(
        &mut self,
        chord: &KeyChord,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        let (keys, suspended) = self.prepare_mapped_chord(chord);
        let result = if suspended.is_empty() {
            backend.send_chord(keys.as_slice())
        } else {
            backend.send_chord_suspending(keys.as_slice(), &suspended)
        };
        if let Err(error) = result {
            // A batch failure may mean that Windows accepted only a prefix.
            // Record every member conservatively; redundant key-up events are
            // harmless and safer than leaving a modifier held.
            self.input
                .latched
                .extend(keys.as_slice().iter().cloned().map(InputTarget::Key));
            return Err(self.recoverable_input_error("keyboard chord", error));
        }
        Ok(())
    }

    pub(super) fn prepare_mapped_chord<'a>(
        &self,
        chord: &'a KeyChord,
    ) -> (MappedChordKeys<'a>, SmallVec<[Key; 8]>) {
        // Only native-visible physical modifiers can leak into the target.
        // Explicit press/toggle latches intentionally remain active.
        let physical_modifiers: SmallVec<[&Key; 8]> = self
            .input
            .pressed
            .iter()
            .filter(|key| {
                key.is_modifier()
                    && (self.input.key_dispositions.get(key) == Some(&KeyDisposition::Forward)
                        || self.input.replayed_keys.contains(key))
            })
            .collect();
        let suspended: SmallVec<[Key; 8]> = physical_modifiers
            .iter()
            .copied()
            .filter(|key| {
                !self.latched_key_matches(key)
                    && !chord
                        .keys()
                        .iter()
                        .any(|target| Self::keys_match(target, key))
            })
            .cloned()
            .collect();
        // Do not emit an Up for a target modifier already held physically or
        // by press/toggle; it is already part of the application's key state.
        let omit = |key: &Key| {
            self.latched_key_matches(key)
                || (key.is_modifier()
                    && physical_modifiers
                        .iter()
                        .any(|held| Self::keys_match(key, held)))
        };
        let keys = if let Some(first) = chord.keys().iter().position(omit) {
            let mut filtered = SmallVec::new();
            filtered.extend(chord.injection_keys()[..first].iter().cloned());
            filtered.extend(
                chord.keys()[first + 1..]
                    .iter()
                    .zip(&chord.injection_keys()[first + 1..])
                    .filter(|(key, _)| !omit(key))
                    .map(|(_, concrete)| concrete.clone()),
            );
            MappedChordKeys::Filtered(filtered)
        } else {
            MappedChordKeys::Compiled(chord.injection_keys())
        };
        (keys, suspended)
    }
}

impl Engine {
    pub(super) fn temporary_mode_is_active(&self, mode: &ModeId) -> bool {
        self.temporary_mode_is_active_for_pressed(mode, &self.input.pressed)
    }

    fn temporary_chord_armed(&self, chord: &KeyChord) -> bool {
        !self.input.temporary_entry_keys.iter().any(|physical| {
            chord
                .keys()
                .iter()
                .any(|configured| Self::keys_match(configured, physical))
        })
    }

    fn temporary_mode_is_active_for_pressed(&self, mode: &ModeId, pressed: &[Key]) -> bool {
        self.temporary_mode_is_active_for_reference::<false>(mode, pressed)
    }

    fn temporary_mode_is_active_for_reference<const HELP: bool>(
        &self,
        mode: &ModeId,
        pressed: &[Key],
    ) -> bool {
        if !self.temporary_trigger_active::<HELP>(mode, pressed) {
            return false;
        }
        // Keep the base Window state available when its binding wins, while
        // lookup itself always considers the held trigger and both layers.
        !pressed.iter().any(|key| {
            !key.is_modifier()
                && self
                    .lookup_for_reference::<HELP>(mode, key, pressed)
                    .is_some_and(|resolved| resolved.owner == *mode)
        })
    }

    fn temporary_trigger_active<const HELP: bool>(&self, mode: &ModeId, pressed: &[Key]) -> bool {
        self.registry.temporary_chords(mode).is_some_and(|chords| {
            chords.iter().any(|entry| {
                let reserved_for_overlap =
                    *mode == ModeId::ui_hint() && entry.conflicts_with_ui_hint_overlap;
                !reserved_for_overlap
                    && (HELP || self.temporary_chord_armed(&entry.chord))
                    && entry.chord.matches_pressed(pressed)
            })
        })
    }

    pub(super) fn key_may_change_temporary_mode(&self, key: &Key) -> bool {
        if self.registry.active.is_window() {
            return true;
        }
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
        self.lookup_for_pressed(key, &self.input.pressed)
    }

    fn lookup_character(&self, input: &crate::api::input::InputEvent) -> Option<ResolvedBinding> {
        let character = input.character?.to_lowercase().next()?;
        if input.key.as_char() == Some(character) || character.is_control() {
            return None;
        }
        // Borrow the configured symbol itself. No symbol-to-keyboard map,
        // per-event String allocation, or synthetic key press is involved.
        let symbol = self.registry.character_keys.get(&character)?;
        // A produced character is already the result of the OS layout. Its
        // producer modifiers are not part of a literal character binding.
        // Keep the real input.key for disposition and held-action release.
        let mut literal: SmallVec<[Key; 8]> = self
            .input
            .pressed
            .iter()
            .filter(|key| {
                self.registry
                    .temporary_chords(&self.registry.active)
                    .is_some_and(|chords| {
                        chords.iter().any(|entry| {
                            self.temporary_chord_armed(&entry.chord)
                                && entry.chord.matches_pressed(&self.input.pressed)
                                && entry
                                    .chord
                                    .keys()
                                    .iter()
                                    .any(|configured| Self::keys_match(configured, key))
                        })
                    })
            })
            .cloned()
            .collect();
        literal.push(symbol.clone());
        let resolved = self.lookup_for_pressed(symbol, &literal)?;
        // Character production may strip shortcut modifiers on alternate
        // layouts. A generated grid key must not steal an application chord.
        // Shift remains available for producing literal symbols; temporary
        // activation modifiers retained in `literal` keep their normal routing.
        // `win` is the canonical Meta family, including macOS Command:
        // Key normalizes cmd aliases and the macOS backend emits *_win.
        if matches!(resolved.binding.as_ref(), Binding::TargetingKey(_))
            && self.input.pressed.iter().any(|key| {
                matches!(Self::modifier_family(key), Some("ctrl" | "alt" | "win"))
                    && !literal.contains(key)
            })
        {
            return None;
        }
        Some(resolved)
    }

    fn binding_available(&self, mode: &ModeId, binding: &Binding) -> bool {
        match binding {
            Binding::Window(action) => self
                .registry
                .get(mode)
                .is_none_or(|mode| mode.window_action_available(action)),
            _ => true,
        }
    }

    pub(super) fn lookup_for_pressed(&self, key: &Key, pressed: &[Key]) -> Option<ResolvedBinding> {
        self.lookup_for_reference::<false>(&self.registry.active, key, pressed)
    }

    pub(super) fn lookup_for_help(&self, key: &Key, pressed: &[Key]) -> Option<ResolvedBinding> {
        self.lookup_for_reference::<true>(&self.registry.active, key, pressed)
    }

    pub(super) fn lookup_for_help_in(
        &self,
        mode: &ModeId,
        key: &Key,
        pressed: &[Key],
    ) -> Option<ResolvedBinding> {
        self.lookup_for_reference::<true>(mode, key, pressed)
    }

    fn lookup_for_reference<const HELP: bool>(
        &self,
        active: &ModeId,
        key: &Key,
        pressed: &[Key],
    ) -> Option<ResolvedBinding> {
        let available = |binding: &Binding| {
            if HELP {
                match binding {
                    Binding::Window(action) => self
                        .registry
                        .get(active)
                        .is_none_or(|mode| mode.window_action_supported(action)),
                    _ => true,
                }
            } else {
                self.binding_available(active, binding)
            }
        };
        let active_match = self.lookup_reference_match::<HELP>(active, active, key, pressed);
        if *active == ModeId::idle() {
            return active_match.map(|(binding, _)| ResolvedBinding {
                binding,
                owner: active.clone(),
            });
        }

        if *active == ModeId::ui_hint() && self.ui_hint_overlap_matches(key) {
            return None;
        }
        let Some(route) = self.registry.routes.get(active) else {
            return active_match.map(|(binding, _)| ResolvedBinding {
                binding,
                owner: active.clone(),
            });
        };
        let temporary_active = self.temporary_trigger_active::<HELP>(active, pressed);
        if temporary_active && let Some(owner) = route.temporary_mode.as_ref() {
            let chords = self.registry.temporary_chords(active)?;
            let remaining: SmallVec<[Key; 8]> = pressed
                .iter()
                .filter(|physical| {
                    !chords.iter().any(|entry| {
                        !(*active == ModeId::ui_hint() && entry.conflicts_with_ui_hint_overlap)
                            && (HELP || self.temporary_chord_armed(&entry.chord))
                            && entry.chord.matches_pressed(pressed)
                            && entry
                                .chord
                                .keys()
                                .iter()
                                .any(|configured| Self::keys_match(configured, physical))
                    })
                })
                .cloned()
                .collect();
            if !remaining.contains(key) {
                return None;
            }
            let passthrough = route.temporary_passthrough.iter().any(|chord| {
                chord.activation_matches(key)
                    && [pressed, remaining.as_slice()]
                        .into_iter()
                        .any(|keys| chord.keys().len() == keys.len() && chord.matches_pressed(keys))
            });
            // Full physical chords outrank stripped chords. Within each tier,
            // the temporary layer wins unless this input explicitly passes through.
            for (keys, exact) in [(pressed, true), (remaining.as_slice(), false)] {
                for (temporary, source) in [(true, owner), (false, active)] {
                    // Window borrows the temporary mode with activation keys
                    // consumed; its own explicit full chords remain available.
                    if temporary && (passthrough || exact && active.is_window()) {
                        continue;
                    }
                    if let Some(resolved) = self.lookup_layer_reference::<HELP>(
                        active,
                        source,
                        key,
                        keys,
                        exact,
                        &mut SmallVec::new(),
                    ) {
                        return (resolved.binding.as_ref() != &Binding::Disabled
                            && available(&resolved.binding))
                        .then_some(resolved);
                    }
                    if !exact
                        && source == active
                        && self.registry.get(active).is_some_and(|mode| {
                            if HELP {
                                mode.claims_key_for_help(key)
                            } else {
                                mode.claims_key(key)
                            }
                        })
                    {
                        return None;
                    }
                }
            }
            return None;
        }

        if let Some((binding, _)) = &active_match
            && !available(binding)
        {
            return None;
        }

        match active_match {
            Some((binding, _)) if matches!(binding.as_ref(), Binding::Disabled) => None,
            Some((binding, _)) => Some(ResolvedBinding {
                binding,
                owner: active.clone(),
            }),
            None if self.registry.get(active).is_some_and(|mode| {
                if HELP {
                    mode.claims_key_for_help(key)
                } else {
                    mode.claims_key(key)
                }
            }) =>
            {
                None
            }
            None => route.inherits.iter().find_map(|owner| {
                if owner == active {
                    return None;
                }
                self.lookup_inherited_reference::<HELP>(
                    active,
                    owner,
                    key,
                    pressed,
                    &mut SmallVec::new(),
                )
                .filter(|resolved| available(&resolved.binding))
            }),
        }
    }

    /// Resolve a layer including inheritance, retaining `none` as a terminal match.
    pub(super) fn lookup_layer_reference<const HELP: bool>(
        &self,
        active: &ModeId,
        owner: &ModeId,
        key: &Key,
        pressed: &[Key],
        exact: bool,
        visited: &mut SmallVec<[ModeId; 8]>,
    ) -> Option<ResolvedBinding> {
        if visited.contains(owner) {
            return None;
        }
        visited.push(owner.clone());
        if let Some((binding, specificity)) =
            self.lookup_reference_match::<HELP>(active, owner, key, pressed)
            && (!exact || specificity == pressed.len())
        {
            return Some(ResolvedBinding {
                binding,
                owner: owner.clone(),
            });
        }
        if !exact
            && owner == active
            && self.registry.get(active).is_some_and(|mode| {
                if HELP {
                    mode.claims_key_for_help(key)
                } else {
                    mode.claims_key(key)
                }
            })
        {
            return None;
        }
        self.registry
            .routes
            .get(owner)?
            .inherits
            .iter()
            .find_map(|source| {
                self.lookup_layer_reference::<HELP>(active, source, key, pressed, exact, visited)
            })
    }

    pub(super) fn lookup_inherited(
        &self,
        owner: &ModeId,
        key: &Key,
        pressed: &[Key],
        visited: &mut SmallVec<[ModeId; 8]>,
    ) -> Option<ResolvedBinding> {
        self.lookup_inherited_reference::<false>(
            &self.registry.active,
            owner,
            key,
            pressed,
            visited,
        )
    }

    fn lookup_inherited_reference<const HELP: bool>(
        &self,
        active: &ModeId,
        owner: &ModeId,
        key: &Key,
        pressed: &[Key],
        visited: &mut SmallVec<[ModeId; 8]>,
    ) -> Option<ResolvedBinding> {
        if visited.contains(owner) {
            return None;
        }
        visited.push(owner.clone());
        if let Some((binding, _)) = self.lookup_reference_match::<HELP>(active, owner, key, pressed)
        {
            return (binding.as_ref() != &Binding::Disabled).then(|| ResolvedBinding {
                binding,
                owner: owner.clone(),
            });
        }
        let sources = &self.registry.routes.get(owner)?.inherits;
        sources.iter().find_map(|source| {
            self.lookup_inherited_reference::<HELP>(active, source, key, pressed, visited)
        })
    }

    fn lookup_reference_match<const HELP: bool>(
        &self,
        active: &ModeId,
        owner: &ModeId,
        key: &Key,
        pressed: &[Key],
    ) -> Option<(Arc<Binding>, usize)> {
        // Window help is compiled without an active input session. Its modes
        // use non-strict matching, independent of held keys or dispositions.
        if HELP && active.is_window() {
            self.registry
                .table(owner)?
                .lookup_with_specificity(key, pressed)
        } else {
            self.lookup_with_specificity_for_pressed(owner, key, pressed)
        }
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
        self.lookup_with_specificity_for_pressed(mode, key, &self.input.pressed)
    }

    fn lookup_with_specificity_for_pressed(
        &self,
        mode: &ModeId,
        key: &Key,
        pressed: &[Key],
    ) -> Option<(Arc<Binding>, usize)> {
        let table = self.registry.table(mode)?;
        if self.strict_modifier_matching_enabled() {
            table.lookup_with_specificity_strict(key, pressed, |modifier| {
                matches!(
                    self.input.key_dispositions.get(modifier),
                    Some(KeyDisposition::Consume)
                )
            })
        } else {
            table.lookup_with_specificity(key, pressed)
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
            || self.registry.active == ModeId::text_input()
            || (self.registry.active == ModeId::normal() && self.settings.passthrough_unbound_keys)
    }

    pub(super) fn injected_key(key: &Key) -> Key {
        key.injection_key().clone()
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
